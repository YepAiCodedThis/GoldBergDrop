//! Persisted list of games the user has set up with GoldbergDrop.
//! Stored as `games.json` under AppData; icons cached as PNG next to it.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const GAMES_FILE: &str = "games.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackedGame {
    pub app_id: u32,
    pub name: String,
    pub exe_path: PathBuf,
    /// Cached PNG under AppData/game_icons/ (may be missing if extraction failed).
    #[serde(default)]
    pub icon_path: Option<PathBuf>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct GamesFile {
    games: Vec<TrackedGame>,
}

fn games_path() -> Result<PathBuf> {
    Ok(crate::settings::app_data_dir()?.join(GAMES_FILE))
}

fn icons_dir() -> Result<PathBuf> {
    let dir = crate::settings::app_data_dir()?.join("game_icons");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn load() -> Vec<TrackedGame> {
    games_path()
        .ok()
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str::<GamesFile>(&t).ok())
        .map(|f| f.games)
        .unwrap_or_default()
}

fn save(games: &[TrackedGame]) -> Result<()> {
    let path = games_path()?;
    let text = serde_json::to_string_pretty(&GamesFile {
        games: games.to_vec(),
    })?;
    fs::write(&path, text).with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(())
}

/// Upsert a game after successful Goldberg apply. Extracts/caches the exe icon.
pub fn track(exe_path: &Path, app_id: u32, name: &str) -> Result<Vec<TrackedGame>> {
    let icon_path = extract_and_cache_icon(exe_path, app_id).ok();
    let mut games = load();
    if let Some(existing) = games.iter_mut().find(|g| g.app_id == app_id) {
        existing.name = name.to_string();
        existing.exe_path = exe_path.to_path_buf();
        if icon_path.is_some() {
            existing.icon_path = icon_path;
        }
    } else {
        games.push(TrackedGame {
            app_id,
            name: name.to_string(),
            exe_path: exe_path.to_path_buf(),
            icon_path,
        });
    }
    // Keep most-recently-used first.
    if let Some(idx) = games.iter().position(|g| g.app_id == app_id) {
        let g = games.remove(idx);
        games.insert(0, g);
    }
    save(&games)?;
    Ok(games)
}

pub fn remove(app_id: u32) -> Result<Vec<TrackedGame>> {
    let mut games = load();
    games.retain(|g| g.app_id != app_id);
    save(&games)?;
    Ok(games)
}

fn extract_and_cache_icon(exe_path: &Path, app_id: u32) -> Result<PathBuf> {
    #[cfg(windows)]
    {
        let path_str = exe_path
            .to_str()
            .context("exe path is not valid UTF-8")?;
        let icon = win_icon_extractor::extract_icon(path_str)
            .map_err(|e| anyhow::anyhow!("icon extract: {e}"))?;
        let out = icons_dir()?.join(format!("{app_id}.png"));
        let img = image::RgbaImage::from_raw(icon.width, icon.height, icon.rgba)
            .context("Invalid icon RGBA buffer")?;
        img.save(&out)
            .with_context(|| format!("Failed to write {}", out.display()))?;
        Ok(out)
    }
    #[cfg(not(windows))]
    {
        let _ = (exe_path, app_id);
        anyhow::bail!("Icon extraction is Windows-only");
    }
}
