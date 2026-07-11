//! Integration with Windows' native "Send to" right-click menu.
//!
//! Rather than installing a full shell-extension context menu handler (which
//! needs a registered COM DLL and admin rights to install system-wide), we
//! hook into the "Send to" submenu that Windows Explorer already builds from
//! shortcut (`.lnk`) files in the per-user
//! `%APPDATA%\Microsoft\Windows\SendTo` folder. This is exactly what that
//! folder is designed for, needs no admin rights, and Explorer passes the
//! right-clicked file's path as `argv[1]` to the target — which is all we
//! need to auto-start a search/apply run.
//!
//! The tricky part is that a `.lnk` shortcut hard-codes an absolute path to
//! `goldberg-drop.exe`. If the user later moves this executable, the shortcut
//! silently starts pointing at a dead location. To catch that, we remember
//! which path we last pointed the shortcut at (in a small state file next to
//! the Goldberg cache) and compare it against [`std::env::current_exe`] on
//! every launch, so the UI can tell the user to re-enable it.

use anyhow::{Context, Result};
use mslnk::ShellLink;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

const SHORTCUT_FILE_NAME: &str = "GoldbergDrop.lnk";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendToStatus {
    /// Never enabled, or explicitly disabled by the user.
    Disabled,
    /// Enabled and the shortcut points at the currently running executable.
    Enabled,
    /// Was enabled, but this executable has since moved — the shortcut on
    /// disk still points at the old (now stale) location.
    Stale,
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
struct SendToState {
    enabled: bool,
    registered_exe: Option<PathBuf>,
}

fn state_file() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("com", "GoldbergDrop", "GoldbergDrop")
        .context("Could not determine AppData directory")?;
    let dir = dirs.data_dir();
    fs::create_dir_all(dir).context("Failed to create app data directory")?;
    Ok(dir.join("sendto_state.json"))
}

fn load_state() -> SendToState {
    state_file()
        .ok()
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn save_state(state: &SendToState) -> Result<()> {
    let path = state_file()?;
    let json = serde_json::to_string_pretty(state)?;
    fs::write(path, json).context("Failed to save Send To settings")?;
    Ok(())
}

/// Windows' per-user "Send to" folder, e.g.
/// `%APPDATA%\Microsoft\Windows\SendTo`.
fn sendto_dir() -> Result<PathBuf> {
    let base_dirs =
        directories::BaseDirs::new().context("Could not determine AppData directory")?;
    Ok(base_dirs
        .data_dir()
        .join("Microsoft")
        .join("Windows")
        .join("SendTo"))
}

fn shortcut_path() -> Result<PathBuf> {
    Ok(sendto_dir()?.join(SHORTCUT_FILE_NAME))
}

/// Checks whether the "Send to" entry is enabled, and whether it still points
/// at the currently running executable.
pub fn status() -> SendToStatus {
    let state = load_state();
    if !state.enabled {
        return SendToStatus::Disabled;
    }
    match (state.registered_exe, std::env::current_exe()) {
        (Some(registered), Ok(current)) if paths_match(&registered, &current) => {
            SendToStatus::Enabled
        }
        _ => SendToStatus::Stale,
    }
}

fn paths_match(a: &PathBuf, b: &PathBuf) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Creates/refreshes the "Send to" shortcut so it points at the currently
/// running executable.
pub fn enable() -> Result<()> {
    let exe = std::env::current_exe().context("Could not determine this executable's path")?;
    let dir = sendto_dir()?;
    fs::create_dir_all(&dir).context("Failed to create the Windows \"Send to\" folder")?;

    let exe_str = exe
        .to_str()
        .context("Executable path is not valid UTF-8")?;
    let link = ShellLink::new(exe_str).context("Failed to build the shortcut")?;
    link.create_lnk(shortcut_path()?)
        .context("Failed to write the \"Send to\" shortcut")?;

    save_state(&SendToState {
        enabled: true,
        registered_exe: Some(exe),
    })?;
    Ok(())
}

/// Removes the "Send to" shortcut, if present, and marks the feature as
/// disabled.
pub fn disable() -> Result<()> {
    if let Ok(path) = shortcut_path() {
        let _ = fs::remove_file(path);
    }
    save_state(&SendToState {
        enabled: false,
        registered_exe: None,
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_paths_match() {
        let p = PathBuf::from(r"C:\Games\Foo\GoldbergDrop.exe");
        assert!(paths_match(&p, &p.clone()));
    }

    #[test]
    fn different_paths_do_not_match() {
        let a = PathBuf::from(r"C:\Games\Foo\GoldbergDrop.exe");
        let b = PathBuf::from(r"D:\Tools\GoldbergDrop.exe");
        assert!(!paths_match(&a, &b));
    }

    /// Exercises the real Windows "Send to" folder and AppData state file.
    /// Not run by default (`cargo test`) since it touches real user state;
    /// run explicitly with `cargo test -- --ignored` to sanity-check on a
    /// real Windows machine.
    #[test]
    #[ignore]
    fn enable_then_disable_round_trip_on_real_machine() {
        disable().unwrap();
        assert_eq!(status(), SendToStatus::Disabled);
        assert!(!shortcut_path().unwrap().exists());

        enable().unwrap();
        assert_eq!(status(), SendToStatus::Enabled);
        assert!(shortcut_path().unwrap().exists());

        disable().unwrap();
        assert_eq!(status(), SendToStatus::Disabled);
        assert!(!shortcut_path().unwrap().exists());
    }

    #[test]
    fn state_round_trips_through_json() {
        let state = SendToState {
            enabled: true,
            registered_exe: Some(PathBuf::from(r"C:\Games\Foo\GoldbergDrop.exe")),
        };
        let json = serde_json::to_string(&state).unwrap();
        let parsed: SendToState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.enabled, state.enabled);
        assert_eq!(parsed.registered_exe, state.registered_exe);
    }
}
