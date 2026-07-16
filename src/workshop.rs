use crate::models::{GgItemResponse, PublishedFileMeta};
use anyhow::{anyhow, Context, Result};
use regex::Regex;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const GG_ITEM_URL: &str = "https://api.ggntw.com/steam.item/";
const PUBLISHED_FILE_DETAILS_URL: &str =
    "https://api.steampowered.com/ISteamRemoteStorage/GetPublishedFileDetails/v1/";
const BROWSER_UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
/// Steam workshop HTML: one page fetch per second (avoids HTTP 429).
const STEAM_PAGE_INTERVAL: Duration = Duration::from_secs(1);
/// Max IDs per GetPublishedFileDetails call.
pub const PUBLISHED_FILE_BATCH: usize = 50;

fn client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent(BROWSER_UA)
        .timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .context("Failed to build HTTP client")
}

fn wait_steam_page_slot() {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(prev) = *last {
        let elapsed = prev.elapsed();
        if elapsed < STEAM_PAGE_INTERVAL {
            std::thread::sleep(STEAM_PAGE_INTERVAL - elapsed);
        }
    }
    *last = Some(Instant::now());
}

/// Extracts a Steam Workshop item ID from a URL or bare numeric string.
pub fn parse_workshop_id(input: &str) -> Option<u64> {
    let trimmed = input.trim().trim_matches(|c: char| matches!(c, ',' | ';' | '|'));
    if trimmed.is_empty() {
        return None;
    }

    if let Ok(id) = trimmed.parse::<u64>() {
        return Some(id);
    }

    let re = Regex::new(r"(?i)filedetails/\?id=(\d+)").ok()?;
    if let Some(caps) = re.captures(trimmed) {
        return caps.get(1)?.as_str().parse().ok();
    }

    let re_gg = Regex::new(r"(?i)ggntw\.com/steam/(\d+)").ok()?;
    if let Some(caps) = re_gg.captures(trimmed) {
        return caps.get(1)?.as_str().parse().ok();
    }

    let re_id = Regex::new(r"\bid=(\d+)").ok()?;
    if let Some(caps) = re_id.captures(trimmed) {
        return caps.get(1)?.as_str().parse().ok();
    }

    None
}

/// Parse one workshop ID/URL per line. Returns `(ids, skipped_invalid_lines)`.
/// Empty lines are ignored. Duplicate IDs in the list are kept once (first wins).
pub fn parse_workshop_id_list(text: &str) -> (Vec<u64>, usize) {
    let mut ids = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut skipped = 0usize;

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match parse_workshop_id(line) {
            Some(id) if seen.insert(id) => ids.push(id),
            Some(_) => {}
            None => skipped += 1,
        }
    }

    (ids, skipped)
}

/// Rewrite a pasted list to one numeric ID per line (URLs → IDs).
/// Always ends with a trailing newline so the next ID can be typed/pasted cleanly.
pub fn normalize_to_id_list(text: &str) -> Option<String> {
    let (ids, _) = parse_workshop_id_list(text);
    if ids.is_empty() {
        None
    } else {
        let mut out = ids
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        out.push('\n');
        Some(out)
    }
}

/// Public GGNetwork download page for a workshop item.
pub fn gg_page_url(workshop_id: u64) -> String {
    format!("https://ggntw.com/steam/{workshop_id}")
}

/// Looks up a workshop item on GGNetwork (cached mods only).
pub fn lookup_item(workshop_id: u64) -> Result<GgItemResponse> {
    let client = client()?;
    let url = format!("{GG_ITEM_URL}{workshop_id}");
    let response = client
        .get(&url)
        .send()
        .context("GGNetwork lookup request failed")?;

    let item: GgItemResponse = response
        .json()
        .context("Failed to parse GGNetwork response")?;

    Ok(item)
}

/// Scrapes the public Steam Workshop HTML page (not the Steam Web API).
/// Returns `(mod_title, game_name, game_app_id)`.
pub fn fetch_workshop_page_info(
    workshop_id: u64,
) -> Result<(String, Option<String>, Option<u32>)> {
    wait_steam_page_slot();

    let client = client()?;
    let url = format!(
        "https://steamcommunity.com/sharedfiles/filedetails/?id={workshop_id}"
    );
    let response = client
        .get(&url)
        .header("Accept", "text/html,application/xhtml+xml;q=0.9,*/*;q=0.8")
        .header("Accept-Language", "en-US,en;q=0.9")
        .send()
        .context("Workshop page request failed")?;

    let status = response.status();
    if status.as_u16() == 429 {
        return Err(anyhow!("Steam rate-limited workshop page (HTTP 429)"));
    }
    if !status.is_success() {
        return Err(anyhow!("Workshop page HTTP {status}"));
    }

    let html = response
        .text()
        .context("Failed to read Workshop page")?;

    let mod_name = Regex::new(r#"(?is)class="[^"]*workshopItemTitle[^"]*"[^>]*>([^<]+)"#)
        .ok()
        .and_then(|re| re.captures(&html))
        .and_then(|c| c.get(1))
        .map(|m| decode_html_entities(m.as_str().trim()))
        .or_else(|| {
            // Fallback: <title>Steam Workshop::Mod Name</title>
            Regex::new(r"(?is)<title>\s*Steam Workshop::([^<]+?)\s*</title>")
                .ok()
                .and_then(|re| re.captures(&html))
                .and_then(|c| c.get(1))
                .map(|m| decode_html_entities(m.as_str().trim()))
        })
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("Workshop title not found on page"))?;

    let game_name = Regex::new(r#"(?is)<div class="apphub_AppName[^"]*">([^<]+)"#)
        .ok()
        .and_then(|re| re.captures(&html))
        .and_then(|c| c.get(1))
        .map(|m| decode_html_entities(m.as_str().trim()))
        .filter(|s| !s.is_empty());

    let game_app_id = Regex::new(r#"(?i)steamcommunity\.com/app/(\d+)"#)
        .ok()
        .and_then(|re| re.captures(&html))
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse().ok())
        .or_else(|| {
            Regex::new(r#"(?i)store\.steampowered\.com/app/(\d+)"#)
                .ok()
                .and_then(|re| re.captures(&html))
                .and_then(|c| c.get(1))
                .and_then(|m| m.as_str().parse().ok())
        })
        .or_else(|| {
            Regex::new(r#"(?i)data-appid="(\d+)""#)
                .ok()
                .and_then(|re| re.captures(&html))
                .and_then(|c| c.get(1))
                .and_then(|m| m.as_str().parse().ok())
        });

    Ok((mod_name, game_name, game_app_id))
}

fn decode_html_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

/// GGNetwork-only resolve (no Steam HTML). Fast path for the queue worker.
pub fn resolve_gg(
    workshop_id: u64,
) -> (String, String, u32, Option<GgItemResponse>, bool) {
    let item = lookup_item(workshop_id).ok();
    let gg_available = item.as_ref().is_some_and(|i| i.is_available());
    let mod_name = item
        .as_ref()
        .map(|i| i.name.clone())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| format!("Item {workshop_id}"));
    let game_app_id = item.as_ref().and_then(|i| i.game_app_id()).unwrap_or(0);
    let game_name = if game_app_id != 0 {
        format!("App {game_app_id}")
    } else {
        "Unknown game".to_string()
    };
    (mod_name, game_name, game_app_id, item, gg_available)
}

/// Batch workshop metadata via Steam Web API (no key required).
pub fn fetch_published_file_details(ids: &[u64]) -> Result<HashMap<u64, PublishedFileMeta>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }

    let client = client()?;
    let mut form: Vec<(String, String)> = Vec::with_capacity(ids.len() + 1);
    form.push(("itemcount".into(), ids.len().to_string()));
    for (i, id) in ids.iter().enumerate() {
        form.push((format!("publishedfileids[{i}]"), id.to_string()));
    }

    let envelope: crate::models::PublishedFileDetailsEnvelope = client
        .post(PUBLISHED_FILE_DETAILS_URL)
        .form(&form)
        .send()
        .context("GetPublishedFileDetails request failed")?
        .error_for_status()
        .context("GetPublishedFileDetails HTTP error")?
        .json()
        .context("GetPublishedFileDetails JSON parse failed")?;

    let mut out = HashMap::new();
    for item in envelope.response.publishedfiledetails {
        if item.result != 1 {
            continue;
        }
        let Ok(id) = item.publishedfileid.parse::<u64>() else {
            continue;
        };
        let app_id = if item.consumer_app_id != 0 {
            item.consumer_app_id
        } else {
            item.creator_app_id
        };
        let title = item.title.trim().to_string();
        if title.is_empty() && app_id == 0 {
            continue;
        }
        out.insert(id, PublishedFileMeta { title, app_id });
    }
    Ok(out)
}

/// Apply GetPublishedFileDetails fields onto a weak resolve result.
pub fn enrich_from_published_meta(
    workshop_id: u64,
    mut mod_name: String,
    mut game_name: String,
    mut game_app_id: u32,
    meta: Option<&PublishedFileMeta>,
) -> (String, String, u32) {
    let Some(meta) = meta else {
        return (mod_name, game_name, game_app_id);
    };
    if !meta.title.is_empty() && mod_name == format!("Item {workshop_id}") {
        mod_name = meta.title.clone();
    }
    if game_app_id == 0 && meta.app_id != 0 {
        game_app_id = meta.app_id;
    }
    if game_app_id != 0 && (game_name == "Unknown game" || game_name.starts_with("App ")) {
        game_name = format!("App {game_app_id}");
    }
    (mod_name, game_name, game_app_id)
}

/// Fill weak names / App ID via Steam workshop HTML (rate-limited to 1/s).
/// Returns `(mod_name, game_name, game_app_id)`.
pub fn enrich_from_steam(
    workshop_id: u64,
    mod_name: String,
    game_name: String,
    game_app_id: u32,
) -> (String, String, u32) {
    let Ok((steam_mod, steam_game, steam_app_id)) = fetch_workshop_page_info(workshop_id) else {
        return (mod_name, game_name, game_app_id);
    };
    let mod_name = if !steam_mod.trim().is_empty() {
        steam_mod
    } else {
        mod_name
    };
    let game_name = steam_game
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(game_name);
    let game_app_id = steam_app_id.unwrap_or(game_app_id);
    (mod_name, game_name, game_app_id)
}

/// GG → GetPublishedFileDetails → HTML scrape (last resort).
#[allow(dead_code)] // used by ignored network tests
pub fn resolve_queue_info(workshop_id: u64) -> Result<(String, String, Option<GgItemResponse>, bool)> {
    let (mut mod_name, mut game_name, mut game_app_id, item, gg_available) =
        resolve_gg(workshop_id);
    let weak = mod_name == format!("Item {workshop_id}")
        || game_name == "Unknown game"
        || game_name.starts_with("App ");
    if weak {
        let meta = fetch_published_file_details(&[workshop_id])
            .ok()
            .and_then(|mut m| m.remove(&workshop_id));
        let enriched =
            enrich_from_published_meta(workshop_id, mod_name, game_name, game_app_id, meta.as_ref());
        mod_name = enriched.0;
        game_name = enriched.1;
        game_app_id = enriched.2;
    }
    let still_weak = mod_name == format!("Item {workshop_id}")
        || game_name == "Unknown game"
        || game_name.starts_with("App ");
    if still_weak {
        let enriched = enrich_from_steam(workshop_id, mod_name, game_name, game_app_id);
        mod_name = enriched.0;
        game_name = enriched.1;
        if game_name == "Unknown game" && enriched.2 != 0 {
            game_name = format!("App {}", enriched.2);
        }
    }
    Ok((mod_name, game_name, item, gg_available))
}

/// Directory next to the running executable.
pub fn exe_dir() -> Result<PathBuf> {
    std::env::current_exe()
        .context("Could not locate running executable")?
        .parent()
        .map(Path::to_path_buf)
        .context("Executable has no parent directory")
}

/// `{root}/{game_name}/` — used by settings-aware callers.
#[allow(dead_code)]
pub fn game_download_dir(game_name: &str) -> Result<PathBuf> {
    let root = exe_dir()?.join("WorkshopDownloads");
    game_download_dir_in(&root, game_name)
}

#[allow(dead_code)]
pub fn game_download_dir_in(root: &Path, game_name: &str) -> Result<PathBuf> {
    let dir = root.join(sanitize_folder_name(game_name));
    fs::create_dir_all(&dir).with_context(|| format!("Failed to create {}", dir.display()))?;
    Ok(dir)
}

/// Replaces characters illegal in Windows file/folder names.
pub fn sanitize_folder_name(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| match c {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    out = out.trim().trim_end_matches('.').to_string();
    if out.len() > 80 {
        out.truncate(80);
        out = out.trim_end().to_string();
    }
    if out.is_empty() {
        "Unknown".to_string()
    } else {
        out
    }
}

fn sanitize_file_name(name: &str) -> String {
    let base = sanitize_folder_name(name);
    if base.contains('.') {
        base
    } else {
        format!("{base}.zip")
    }
}

/// Downloads a mod file into `dest_dir`.
pub fn download_mod(item: &GgItemResponse, dest_dir: &Path) -> Result<PathBuf> {
    if !item.is_available() {
        return Err(anyhow!("Mod is not available on GGNetwork"));
    }

    let download_url = item
        .download
        .as_deref()
        .filter(|u| !u.is_empty())
        .ok_or_else(|| anyhow!("GGNetwork returned no download URL"))?;

    fs::create_dir_all(dest_dir)
        .with_context(|| format!("Failed to create {}", dest_dir.display()))?;
    let file_name = sanitize_file_name(&item.name);
    let dest_path = dest_dir.join(&file_name);

    let bytes = fetch_mod_bytes(download_url)?;
    let mut file = fs::File::create(&dest_path)
        .with_context(|| format!("Failed to create {}", dest_path.display()))?;
    file.write_all(&bytes)
        .with_context(|| format!("Failed to write {}", dest_path.display()))?;

    Ok(dest_path)
}

fn fetch_mod_bytes(download_url: &str) -> Result<Vec<u8>> {
    let client = client()?;
    let response = client
        .get(download_url)
        .header("Referer", "https://ggntw.com/")
        .header("Origin", "https://ggntw.com")
        .send()
        .context("Download request failed")?;

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();

    let bytes = response.bytes().context("Failed to read download body")?;

    if looks_like_binary(&content_type, &bytes) {
        return Ok(bytes.to_vec());
    }

    let html = std::str::from_utf8(&bytes).unwrap_or("");
    if let Some(cdn_url) = extract_cdn_url(html) {
        let cdn_response = client
            .get(&cdn_url)
            .header("Referer", "https://ggntw.com/")
            .send()
            .context("CDN download request failed")?;
        return Ok(cdn_response.bytes()?.to_vec());
    }

    Err(anyhow!(
        "Download blocked — try downloading manually from ggntw.com"
    ))
}

fn looks_like_binary(content_type: &str, bytes: &[u8]) -> bool {
    if content_type.contains("text/html") || content_type.contains("text/plain") {
        return false;
    }
    if content_type.contains("octet-stream")
        || content_type.contains("zip")
        || content_type.contains("application/")
    {
        return true;
    }
    // Heuristic: HTML pages start with `<` or `<!`.
    !bytes.starts_with(b"<")
}

fn extract_cdn_url(html: &str) -> Option<String> {
    let re = Regex::new(r#"https?://[^"'\s]*cdn\.ggntw\.(?:com|ru)[^"'\s]*"#).ok()?;
    re.find(html).map(|m| m.as_str().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_workshop_url() {
        assert_eq!(
            parse_workshop_id("https://steamcommunity.com/sharedfiles/filedetails/?id=1710929351"),
            Some(1710929351)
        );
        assert_eq!(parse_workshop_id("1710929351"), Some(1710929351));
        assert_eq!(
            parse_workshop_id("https://ggntw.com/steam/1710929351"),
            Some(1710929351)
        );
        assert_eq!(parse_workshop_id("not a url"), None);
    }

    #[test]
    fn parse_workshop_id_list_mixed() {
        let text = "\
1710929351
https://steamcommunity.com/sharedfiles/filedetails/?id=222
https://ggntw.com/steam/333
not-valid
1710929351
";
        let (ids, skipped) = parse_workshop_id_list(text);
        assert_eq!(ids, vec![1710929351, 222, 333]);
        assert_eq!(skipped, 1);
    }

    #[test]
    fn normalize_to_id_list_strips_urls() {
        let text = "https://steamcommunity.com/sharedfiles/filedetails/?id=111\n222\n";
        assert_eq!(normalize_to_id_list(text).as_deref(), Some("111\n222\n"));
        assert_eq!(normalize_to_id_list("nope"), None);
    }

    #[test]
    fn sanitize_replaces_illegal_chars() {
        assert_eq!(sanitize_folder_name(r#"Test: Game?"#), "Test_ Game_");
        assert_eq!(sanitize_folder_name(""), "Unknown");
    }

    #[test]
    #[ignore]
    fn published_file_details_fast_crafting() {
        let map = fetch_published_file_details(&[1974431336]).expect("api");
        let meta = map.get(&1974431336).expect("meta");
        assert_eq!(meta.title, "Fast Crafting");
        assert_eq!(meta.app_id, 751780);
    }

    #[test]
    #[ignore]
    fn resolve_forager_fast_crafting() {
        let (m, g, item, gg_ok) = resolve_queue_info(1974431336).expect("resolve");
        assert_eq!(m, "Fast Crafting");
        assert_eq!(g, "Forager");
        assert!(!gg_ok);
        assert!(item.is_some());
        assert_eq!(item.as_ref().unwrap().result, 0);
    }

    #[test]
    #[ignore]
    fn fetch_names_for_uncached_gg_item() {
        let (mod_name, game_name) = fetch_workshop_page_info(3765697723)
            .map(|(m, g, _)| (m, g))
            .expect("html scrape");
        assert_eq!(mod_name, "CylunderTown");
        assert_eq!(game_name.as_deref(), Some("On Sight Playtest"));

        let (m, g, item, gg_ok) = resolve_queue_info(3765697723).expect("resolve");
        assert_eq!(m, "CylunderTown");
        assert_eq!(g, "On Sight Playtest");
        assert!(item.is_some());
        assert_eq!(item.unwrap().result, 0);
        assert!(!gg_ok);
    }
}