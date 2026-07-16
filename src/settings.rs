use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

const SETTINGS_FILE: &str = "settings.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsTab {
    #[default]
    Download,
    SteamCmd,
    Paths,
    Setup,
    Tray,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub use_ggnetwork: bool,
    pub use_steamcmd: bool,
    pub ask_before_steamcmd: bool,
    /// `None` = `{exe_dir}/WorkshopDownloads`
    pub workshop_download_dir: Option<PathBuf>,
    pub fetch_dlc_default: bool,
    pub fetch_achievements_default: bool,
    /// Close (✕) hides to tray instead of quitting.
    pub close_to_tray: bool,
    /// Start with Windows, minimized to tray (`--tray`).
    pub autostart_tray: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            use_ggnetwork: true,
            use_steamcmd: true,
            ask_before_steamcmd: true,
            workshop_download_dir: None,
            fetch_dlc_default: true,
            fetch_achievements_default: true,
            close_to_tray: true,
            autostart_tray: false,
        }
    }
}

/// `%LOCALAPPDATA%\GoldbergDrop\GoldbergDrop\` (same ProjectDirs as emulator/sendto).
pub fn app_data_dir() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("com", "GoldbergDrop", "GoldbergDrop")
        .context("Could not determine AppData directory")?;
    let dir = dirs.data_dir().to_path_buf();
    fs::create_dir_all(&dir).context("Failed to create app data directory")?;
    Ok(dir)
}

impl AppSettings {
    fn settings_path() -> Result<PathBuf> {
        Ok(app_data_dir()?.join(SETTINGS_FILE))
    }

    pub fn load() -> Self {
        Self::try_load().unwrap_or_default()
    }

    fn try_load() -> Result<Self> {
        let path = Self::settings_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        let settings: Self = serde_json::from_str(&text)
            .with_context(|| format!("Failed to parse {}", path.display()))?;
        Ok(settings)
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::settings_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self).context("Failed to serialize settings")?;
        fs::write(&path, text).with_context(|| format!("Failed to write {}", path.display()))?;
        Ok(())
    }

    /// Effective workshop download root (parent of per-game folders).
    pub fn workshop_root(&self) -> Result<PathBuf> {
        if let Some(dir) = &self.workshop_download_dir {
            if !dir.as_os_str().is_empty() {
                return Ok(dir.clone());
            }
        }
        Ok(crate::workshop::exe_dir()?.join("WorkshopDownloads"))
    }

    pub fn game_download_dir(&self, game_name: &str) -> Result<PathBuf> {
        let dir = self
            .workshop_root()?
            .join(crate::workshop::sanitize_folder_name(game_name));
        fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create {}", dir.display()))?;
        Ok(dir)
    }

    pub fn display_workshop_root(&self) -> String {
        self.workshop_root()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "(unavailable)".into())
    }

    pub fn steamcmd_dir() -> Result<PathBuf> {
        let dir = app_data_dir()?.join("steamcmd");
        fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create {}", dir.display()))?;
        Ok(dir)
    }

    pub fn steamcmd_exe() -> Result<PathBuf> {
        Ok(Self::steamcmd_dir()?.join("steamcmd.exe"))
    }

    pub fn steamcmd_installed() -> bool {
        Self::steamcmd_exe()
            .map(|p| p.is_file())
            .unwrap_or(false)
    }
}
