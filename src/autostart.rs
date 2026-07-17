//! Autostart GoldbergDrop minimized to tray (Windows Startup / Run key).

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

const RUN_VALUE: &str = "GoldbergDrop";

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
pub fn is_enabled() -> bool {
    use winreg::enums::*;
    use winreg::RegKey;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let Ok(key) = hkcu.open_subkey_with_flags(
        r"Software\Microsoft\Windows\CurrentVersion\Run",
        KEY_READ,
    ) else {
        return false;
    };
    key.get_value::<String, _>(RUN_VALUE).is_ok()
}

#[cfg(not(windows))]
pub fn is_enabled() -> bool {
    false
}

#[cfg(windows)]
pub fn enable() -> Result<()> {
    use winreg::enums::*;
    use winreg::RegKey;

    let cmd = format_run_command(&autostart_exe_path()?);

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu.create_subkey(r"Software\Microsoft\Windows\CurrentVersion\Run")?;
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
    use winreg::enums::*;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu.open_subkey_with_flags(
        r"Software\Microsoft\Windows\CurrentVersion\Run",
        KEY_SET_VALUE,
    )?;
    // Missing value is fine.
    let _ = key.delete_value(RUN_VALUE);
    Ok(())
}

#[cfg(not(windows))]
pub fn disable() -> Result<()> {
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
    }
}
