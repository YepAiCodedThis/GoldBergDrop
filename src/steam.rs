use crate::models::{
    Achievement, AppDetailsEnvelope, DlcApp, SteamApp, StoreSearchResponse,
};
use anyhow::{Context, Result};
use regex::Regex;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

const USER_AGENT: &str = "GoldbergDrop/0.1";

fn client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(15))
        .build()
        .context("Failed to build HTTP client")
}

/// Builds the ordered list of search terms to try: exe name, folder name, full path.
pub fn build_search_terms(exe_path: &Path) -> Vec<String> {
    let mut terms = Vec::new();

    if let Some(stem) = exe_path.file_stem().and_then(|s| s.to_str()) {
        if !stem.trim().is_empty() {
            terms.push(stem.to_string());
        }
    }

    if let Some(folder_name) = exe_path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
    {
        if !folder_name.trim().is_empty()
            && !terms
                .iter()
                .any(|t| t.eq_ignore_ascii_case(folder_name))
        {
            terms.push(folder_name.to_string());
        }
    }

    if let Some(full) = exe_path.to_str() {
        if !terms.iter().any(|t| t.eq_ignore_ascii_case(full)) {
            terms.push(full.to_string());
        }
    }

    terms
}

/// Searches the Steam store for apps matching the given term.
/// Returns only items of type "app" (games), not DLC/bundles/etc.
pub fn store_search(term: &str) -> Result<Vec<SteamApp>> {
    if term.trim().is_empty() {
        return Ok(Vec::new());
    }

    let client = client()?;
    let url = "https://store.steampowered.com/api/storesearch/";
    let response = client
        .get(url)
        .query(&[("term", term), ("cc", "US"), ("l", "english")])
        .send()
        .context("Store search request failed")?;

    let body: StoreSearchResponse = response
        .json()
        .context("Failed to parse store search response")?;

    Ok(body
        .items
        .into_iter()
        .filter(|item| item.item_type.eq_ignore_ascii_case("app"))
        .map(|item| SteamApp {
            app_id: item.id,
            name: item.name,
        })
        .collect())
}

/// Tries to find a single Steam app for the given executable path by trying
/// progressively less specific search terms (exe name, folder name, full path).
/// Returns `Ok(Some(app))` on a confident single/exact match, `Ok(None)` if no
/// terms produced any results, or the full candidate list via `store_search`
/// for the caller to present a picker when a term is ambiguous.
pub fn find_app_by_exe(exe_path: &Path) -> Result<AppSearchOutcome> {
    let terms = build_search_terms(exe_path);

    for term in &terms {
        let results = store_search(term)?;
        match results.len() {
            0 => continue,
            1 => return Ok(AppSearchOutcome::Found(results[0].clone())),
            _ => {
                if let Some(exact) = results
                    .iter()
                    .find(|a| a.name.eq_ignore_ascii_case(term))
                {
                    return Ok(AppSearchOutcome::Found(exact.clone()));
                }
                return Ok(AppSearchOutcome::Ambiguous(results));
            }
        }
    }

    Ok(AppSearchOutcome::NotFound)
}

pub enum AppSearchOutcome {
    Found(SteamApp),
    Ambiguous(Vec<SteamApp>),
    NotFound,
}

/// Fetches basic app info via the app details endpoint. Used to validate a
/// manually entered App ID and to resolve its display name.
pub fn get_app_name(app_id: u32) -> Result<Option<String>> {
    let envelope = fetch_app_details(app_id)?;
    Ok(envelope.and_then(|e| e.data).and_then(|d| d.name))
}

/// Fetches the DLC app IDs for a given base game App ID, then resolves each
/// DLC's display name via the app details endpoint (falling back to a
/// generic "Unknown DLC {id}" label when no name can be found).
pub fn get_dlc_list(app_id: u32) -> Result<Vec<DlcApp>> {
    let data = match fetch_app_details(app_id)?.and_then(|e| e.data) {
        Some(d) => d,
        None => return Ok(Vec::new()),
    };

    let mut dlc_list = Vec::with_capacity(data.dlc.len());
    for dlc_id in data.dlc {
        let name = get_app_name(dlc_id)
            .ok()
            .flatten()
            .unwrap_or_else(|| format!("Unknown DLC {dlc_id}"));
        dlc_list.push(DlcApp {
            app_id: dlc_id,
            name,
        });
    }

    Ok(dlc_list)
}

/// Builds Goldberg `achievements.json` entries without a Steam Web API key:
/// API names from `GetGlobalAchievementPercentagesForApp`, display text +
/// icons from the public community achievements page, merged by list order
/// (Steam keeps those lists aligned by unlock %).
pub fn get_achievements(app_id: u32) -> Result<Vec<Achievement>> {
    let names = fetch_achievement_api_names(app_id)?;
    if names.is_empty() {
        return Ok(Vec::new());
    }

    let rows = scrape_community_achievements(app_id)?;
    let count = names.len().min(rows.len());
    if count == 0 {
        return Ok(Vec::new());
    }

    Ok(names
        .into_iter()
        .zip(rows)
        .take(count)
        .map(|(name, row)| Achievement {
            name,
            display_name: row.display_name,
            description: row.description,
            hidden: "0".into(),
            icon: row.icon.clone(),
            // Community page only exposes the unlocked art; reuse it for gray.
            icongray: row.icon,
        })
        .collect())
}

#[derive(Debug)]
struct CommunityAchRow {
    display_name: String,
    description: String,
    icon: String,
}

fn fetch_achievement_api_names(app_id: u32) -> Result<Vec<String>> {
    let client = client()?;
    let url = "https://api.steampowered.com/ISteamUserStats/GetGlobalAchievementPercentagesForApp/v2/";
    let response = client
        .get(url)
        .query(&[("gameid", app_id.to_string())])
        .send()
        .context("Achievement percentages request failed")?;

    let body: serde_json::Value = response
        .json()
        .context("Failed to parse achievement percentages")?;

    let list = body
        .pointer("/achievementpercentages/achievements")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    Ok(list
        .into_iter()
        .filter_map(|item| {
            item.get("name")
                .and_then(|n| n.as_str())
                .map(|s| s.to_string())
        })
        .collect())
}

fn scrape_community_achievements(app_id: u32) -> Result<Vec<CommunityAchRow>> {
    let client = client()?;
    let url = format!("https://steamcommunity.com/stats/{app_id}/achievements/?l=english");
    let html = client
        .get(&url)
        .send()
        .context("Community achievements request failed")?
        .text()
        .context("Failed to read community achievements page")?;

    Ok(parse_community_achievement_rows(&html))
}

fn community_row_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"(?s)<div class="achieveRow[^"]*">.*?<img src="([^"]+)".*?<h3>(.*?)</h3>.*?<h5>(.*?)</h5>"#,
        )
        .expect("community achievement regex")
    })
}

fn parse_community_achievement_rows(html: &str) -> Vec<CommunityAchRow> {
    community_row_re()
        .captures_iter(html)
        .map(|caps| CommunityAchRow {
            icon: caps[1].to_string(),
            display_name: decode_html_entities(caps[2].trim()),
            description: decode_html_entities(caps[3].trim()),
        })
        .collect()
}

fn decode_html_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#039;", "'")
        .replace("&apos;", "'")
}

/// Fetches and parses the `appdetails` envelope for a single App ID.
/// Returns `Ok(None)` if the API reports `success: false` for this ID.
fn fetch_app_details(app_id: u32) -> Result<Option<AppDetailsEnvelope>> {
    let client = client()?;
    let url = "https://store.steampowered.com/api/appdetails";
    let response = client
        .get(url)
        .query(&[("appids", app_id.to_string()), ("l", "english".into())])
        .send()
        .context("App details request failed")?;

    let body: serde_json::Value = response.json().context("Failed to parse app details")?;
    let entry = match body.get(app_id.to_string()) {
        Some(v) => v.clone(),
        None => return Ok(None),
    };
    let envelope: AppDetailsEnvelope =
        serde_json::from_value(entry).context("Failed to parse app details envelope")?;

    if !envelope.success {
        return Ok(None);
    }

    Ok(Some(envelope))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_community_rows_and_entities() {
        let html = r#"
<div class="achieveRow ">
  <div class="achieveImgHolder">
    <img src="https://example.com/a.jpg" width="64" height="64" border="0" />
  </div>
  <div class="achieveTxt">
    <h3>Title &amp; More</h3>
    <h5>Desc &#039;ok&#039;</h5>
  </div>
</div>
<div class="achieveRow ">
  <img src="https://example.com/b.jpg" />
  <h3>Second</h3>
  <h5></h5>
</div>
"#;
        let rows = parse_community_achievement_rows(html);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].display_name, "Title & More");
        assert_eq!(rows[0].description, "Desc 'ok'");
        assert_eq!(rows[0].icon, "https://example.com/a.jpg");
        assert_eq!(rows[1].display_name, "Second");
        assert!(rows[1].description.is_empty());
    }
}
