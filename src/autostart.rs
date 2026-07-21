//! Autostart GoldbergDrop minimized to tray (Windows Startup / Run key).
//! Also backs up / restores Steam's Run entry for GreenLuma auto-inject.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

const RUN_VALUE: &str = "GoldbergDrop";
const STEAM_RUN_VALUE: &str = "Steam";

/// Path for the Run registry value. Avoids `canonicalize()` — on Windows that
/// yields a `\\?\` extended path which the Run key fails to launch.
fn autostart_exe_path() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("current_exe")?;
    Ok(strip_unc_prefix(&exe))
}

#[cfg(windows)]
fn strip_unc_prefix(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(stripped) = s.strip_prefix(r"\\?\") {
        PathBuf::from(stripped)
    } else {
        path.to_path_buf()
    }
}

#[cfg(not(windows))]
fn strip_unc_prefix(path: &Path) -> PathBuf {
    path.to_path_buf()
}

fn format_run_command(exe: &Path) -> String {
    format!("\"{}\" --tray", exe.display())
}

#[cfg(windows)]
fn open_run_key(write: bool) -> Result<winreg::RegKey> {
    use winreg::enums::*;
    use winreg::RegKey;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if write {
        let (key, _) = hkcu.create_subkey(r"Software\Microsoft\Windows\CurrentVersion\Run")?;
        Ok(key)
    } else {
        Ok(hkcu.open_subkey_with_flags(
            r"Software\Microsoft\Windows\CurrentVersion\Run",
            KEY_READ,
        )?)
    }
}

#[cfg(windows)]
pub fn is_enabled() -> bool {
    open_run_key(false)
        .ok()
        .and_then(|key| key.get_value::<String, _>(RUN_VALUE).ok())
        .is_some()
}

#[cfg(not(windows))]
pub fn is_enabled() -> bool {
    false
}

#[cfg(windows)]
pub fn enable() -> Result<()> {
    let cmd = format_run_command(&autostart_exe_path()?);
    let key = open_run_key(true)?;
    key.set_value(RUN_VALUE, &cmd)
        .context("Failed to write autostart Run value")?;
    Ok(())
}

#[cfg(not(windows))]
pub fn enable() -> Result<()> {
    anyhow::bail!("Autostart is Windows-only");
}

#[cfg(windows)]
pub fn disable() -> Result<()> {
    if let Ok(key) = open_run_key(true) {
        let _ = key.delete_value(RUN_VALUE);
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn disable() -> Result<()> {
    Ok(())
}

/// If the GoldbergDrop Run entry exists but points at a moved exe (or uses
/// `\\?\`), rewrite it to the current quoted path + ` --tray`.
#[cfg(windows)]
pub fn ensure_gbd_run_valid() -> Result<bool> {
    let key = match open_run_key(true) {
        Ok(k) => k,
        Err(_) => return Ok(false),
    };
    let Ok(current) = key.get_value::<String, _>(RUN_VALUE) else {
        return Ok(false);
    };
    let expected = format_run_command(&autostart_exe_path()?);
    if current == expected {
        return Ok(false);
    }
    // Stale or UNC — rewrite whenever the value exists.
    key.set_value(RUN_VALUE, &expected)
        .context("Failed to refresh GoldbergDrop Run value")?;
    Ok(true)
}

#[cfg(not(windows))]
pub fn ensure_gbd_run_valid() -> Result<bool> {
    Ok(false)
}

/// Read Steam's Windows Run-key value, if present.
#[cfg(windows)]
pub fn read_steam_run() -> Option<String> {
    open_run_key(false)
        .ok()
        .and_then(|key| key.get_value::<String, _>(STEAM_RUN_VALUE).ok())
}

#[cfg(not(windows))]
pub fn read_steam_run() -> Option<String> {
    None
}

/// Delete Steam's Run-key value (disable Steam Windows autostart).
#[cfg(windows)]
pub fn delete_steam_run() -> Result<()> {
    if let Ok(key) = open_run_key(true) {
        let _ = key.delete_value(STEAM_RUN_VALUE);
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn delete_steam_run() -> Result<()> {
    Ok(())
}

/// Restore Steam's Run-key value from a previously saved backup string.
#[cfg(windows)]
pub fn write_steam_run(value: &str) -> Result<()> {
    let key = open_run_key(true)?;
    key.set_value(STEAM_RUN_VALUE, &value.to_string())
        .context("Failed to restore Steam Run value")?;
    Ok(())
}

#[cfg(not(windows))]
pub fn write_steam_run(_value: &str) -> Result<()> {
    Ok(())
}

#[allow(dead_code)]
pub fn shortcut_hint() -> PathBuf {
    PathBuf::from(r"%APPDATA%\…\Run\GoldbergDrop")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_unc_prefix() {
        assert_eq!(
            strip_unc_prefix(Path::new(r"\\?\C:\Games\1. GB\app.exe")),
            PathBuf::from(r"C:\Games\1. GB\app.exe")
        );
        assert_eq!(
            strip_unc_prefix(Path::new(r"C:\Games\app.exe")),
            PathBuf::from(r"C:\Games\app.exe")
        );
    }

    #[test]
    fn run_command_quotes_spaces() {
        let cmd = format_run_command(Path::new(r"C:\Games\1. GB\goldberg-drop.exe"));
        assert_eq!(cmd, r#""C:\Games\1. GB\goldberg-drop.exe" --tray"#);
        assert!(!cmd.contains(r"\\?\"));
    }
}
