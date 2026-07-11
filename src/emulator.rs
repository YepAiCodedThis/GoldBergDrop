use anyhow::{bail, Context, Result};
use regex::Regex;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;
use zip::ZipArchive;

const GOLDBERG_PAGE_URL: &str = "https://mr_goldberg.gitlab.io/goldberg_emulator/";
const JOB_ARTIFACT_REGEX: &str =
    r"https://gitlab\.com/Mr_Goldberg/goldberg_emulator/-/jobs/(?P<jobid>\d+)/artifacts/download";

/// Directory used to cache the extracted Goldberg emulator release, e.g.
/// `%APPDATA%\GoldbergDrop\goldberg`.
pub fn cache_dir() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("com", "GoldbergDrop", "GoldbergDrop")
        .context("Could not determine AppData directory")?;
    let dir = dirs.data_dir().join("goldberg");
    fs::create_dir_all(&dir).context("Failed to create Goldberg cache directory")?;
    Ok(dir)
}

/// Ensures the latest Goldberg emulator release is downloaded and extracted
/// into the cache directory, downloading/updating it if necessary. Returns
/// the path to the cache directory containing `steam_api.dll` etc.
pub fn ensure_goldberg_available() -> Result<PathBuf> {
    let goldberg_path = cache_dir()?;
    let job_id_path = goldberg_path.join("job_id");
    let zip_path = goldberg_path.join("goldberg.zip");

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("Failed to build HTTP client")?;

    let page_body = client
        .get(GOLDBERG_PAGE_URL)
        .send()
        .context("Failed to reach the Goldberg emulator page")?
        .text()
        .context("Failed to read the Goldberg emulator page")?;

    let regex = Regex::new(JOB_ARTIFACT_REGEX).expect("valid regex");
    let matched = regex
        .captures(&page_body)
        .context("Could not find a download link on the Goldberg emulator page")?;
    let remote_job_id = &matched["jobid"];
    let download_url = matched.get(0).unwrap().as_str().to_string();

    let needs_download = match fs::read_to_string(&job_id_path) {
        Ok(local_job_id) if local_job_id.trim() == remote_job_id => false,
        _ => true,
    };

    if !needs_download && has_steam_api_dll(&goldberg_path) {
        return Ok(goldberg_path);
    }

    download_file(&client, &download_url, &zip_path)?;
    extract_zip(&zip_path, &goldberg_path)?;
    fs::write(&job_id_path, remote_job_id).context("Failed to write job_id cache file")?;
    let _ = fs::remove_file(&zip_path);

    Ok(goldberg_path)
}

fn has_steam_api_dll(dir: &Path) -> bool {
    dir.join("steam_api.dll").exists() || dir.join("steam_api64.dll").exists()
}

fn download_file(client: &reqwest::blocking::Client, url: &str, dest: &Path) -> Result<()> {
    let mut response = client
        .get(url)
        .send()
        .context("Failed to download the Goldberg emulator archive")?;

    if !response.status().is_success() {
        bail!("Download failed with status {}", response.status());
    }

    let mut file = fs::File::create(dest).context("Failed to create archive file")?;
    response
        .copy_to(&mut file)
        .context("Failed to write downloaded archive to disk")?;
    file.flush().ok();
    Ok(())
}

fn extract_zip(zip_path: &Path, dest_dir: &Path) -> Result<()> {
    let file = fs::File::open(zip_path).context("Failed to open downloaded archive")?;
    let mut archive = ZipArchive::new(file).context("Failed to read archive contents")?;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let Some(relative_path) = entry.enclosed_name() else {
            continue;
        };
        let out_path = dest_dir.join(relative_path);

        if entry.is_dir() {
            fs::create_dir_all(&out_path)?;
            continue;
        }

        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut out_file = fs::File::create(&out_path)
            .with_context(|| format!("Failed to write extracted file {out_path:?}"))?;
        std::io::copy(&mut entry, &mut out_file)?;
    }

    Ok(())
}
