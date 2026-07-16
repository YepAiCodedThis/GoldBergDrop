//! Autostart GoldbergDrop minimized to tray (Windows Startup / Run key).

use anyhow::{Context, Result};
use std::path::PathBuf;

const RUN_VALUE: &str = "GoldbergDrop";

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

    let exe = std::env::current_exe().context("current_exe")?;
    let exe = exe
        .canonicalize()
        .unwrap_or(exe)
        .to_string_lossy()
        .to_string();
    // Quotes for spaces; --tray starts hidden in the notification area.
    let cmd = format!("\"{exe}\" --tray");

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
