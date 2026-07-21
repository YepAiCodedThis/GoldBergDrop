//! Integration with Windows' native "Send to" right-click menu.
//!
//! Two shortcuts:
//! - `GoldbergDrop.lnk` — Setup tab (no args)
//! - `GoldbergDrop (GreenLuma).lnk` — GreenLuma tab (`--greenluma`)

use anyhow::{Context, Result};
use mslnk::ShellLink;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

const SHORTCUT_FILE_NAME: &str = "GoldbergDrop.lnk";
const SHORTCUT_GL_FILE_NAME: &str = "GoldbergDrop (GreenLuma).lnk";
const STATE_FILE: &str = "sendto_state.json";
const STATE_GL_FILE: &str = "sendto_greenluma_state.json";

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

fn state_path(name: &str) -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("com", "GoldbergDrop", "GoldbergDrop")
        .context("Could not determine AppData directory")?;
    let dir = dirs.data_dir();
    fs::create_dir_all(dir).context("Failed to create app data directory")?;
    Ok(dir.join(name))
}

fn load_state(name: &str) -> SendToState {
    state_path(name)
        .ok()
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn save_state(name: &str, state: &SendToState) -> Result<()> {
    let path = state_path(name)?;
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

fn shortcut_path(name: &str) -> Result<PathBuf> {
    Ok(sendto_dir()?.join(name))
}

fn paths_match(a: &PathBuf, b: &PathBuf) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

fn status_for(state_file: &str) -> SendToStatus {
    let state = load_state(state_file);
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

fn write_shortcut(lnk_name: &str, arguments: Option<&str>) -> Result<()> {
    let exe = std::env::current_exe().context("Could not determine this executable's path")?;
    let dir = sendto_dir()?;
    fs::create_dir_all(&dir).context("Failed to create the Windows \"Send to\" folder")?;

    let exe_str = exe
        .to_str()
        .context("Executable path is not valid UTF-8")?;
    let mut link = ShellLink::new(exe_str).context("Failed to build the shortcut")?;
    link.set_arguments(arguments.map(|s| s.to_string()));
    link.create_lnk(shortcut_path(lnk_name)?)
        .context("Failed to write the \"Send to\" shortcut")?;
    Ok(())
}

fn enable_for(state_file: &str, lnk_name: &str, arguments: Option<&str>) -> Result<()> {
    let exe = std::env::current_exe().context("Could not determine this executable's path")?;
    write_shortcut(lnk_name, arguments)?;
    save_state(
        state_file,
        &SendToState {
            enabled: true,
            registered_exe: Some(exe),
        },
    )?;
    Ok(())
}

fn disable_for(state_file: &str, lnk_name: &str) -> Result<()> {
    if let Ok(path) = shortcut_path(lnk_name) {
        let _ = fs::remove_file(path);
    }
    save_state(
        state_file,
        &SendToState {
            enabled: false,
            registered_exe: None,
        },
    )?;
    Ok(())
}

/// Checks whether the Goldberg "Send to" entry is enabled / stale.
pub fn status() -> SendToStatus {
    status_for(STATE_FILE)
}

/// Creates/refreshes the Goldberg "Send to" shortcut.
pub fn enable() -> Result<()> {
    enable_for(STATE_FILE, SHORTCUT_FILE_NAME, None)
}

/// Removes the Goldberg "Send to" shortcut.
pub fn disable() -> Result<()> {
    disable_for(STATE_FILE, SHORTCUT_FILE_NAME)
}

/// GreenLuma "Send to" status (separate shortcut + state).
pub fn status_greenluma() -> SendToStatus {
    status_for(STATE_GL_FILE)
}

pub fn enable_greenluma() -> Result<()> {
    enable_for(STATE_GL_FILE, SHORTCUT_GL_FILE_NAME, Some("--greenluma"))
}

pub fn disable_greenluma() -> Result<()> {
    disable_for(STATE_GL_FILE, SHORTCUT_GL_FILE_NAME)
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

    #[test]
    #[ignore]
    fn enable_then_disable_round_trip_on_real_machine() {
        disable().unwrap();
        assert_eq!(status(), SendToStatus::Disabled);
        assert!(!shortcut_path(SHORTCUT_FILE_NAME).unwrap().exists());

        enable().unwrap();
        assert_eq!(status(), SendToStatus::Enabled);
        assert!(shortcut_path(SHORTCUT_FILE_NAME).unwrap().exists());

        disable().unwrap();
        assert_eq!(status(), SendToStatus::Disabled);
        assert!(!shortcut_path(SHORTCUT_FILE_NAME).unwrap().exists());
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
