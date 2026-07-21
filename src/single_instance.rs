//! Single-instance guard + lightweight AppData IPC for Send-to / second launches.
//!
//! First process creates a named mutex and owns it for its lifetime.
//! Later processes write a command file and exit; the owner polls and handles it.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

const MUTEX_NAME: &str = "Local\\GoldbergDrop.SingleInstance";
const IPC_FILE: &str = "ipc_command.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcCommand {
    /// Switch to GreenLuma tab (and route path there if set).
    pub greenluma: bool,
    /// Optional file from Send-to / CLI.
    pub path: Option<PathBuf>,
    /// Bring the main window to the front.
    pub show: bool,
}

#[cfg(windows)]
struct MutexHandle(*mut std::ffi::c_void);

#[cfg(windows)]
unsafe impl Send for MutexHandle {}

#[cfg(windows)]
impl Drop for MutexHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

/// Holds the process-wide mutex so a second GoldbergDrop cannot start.
pub struct InstanceGuard {
    #[cfg(windows)]
    _mutex: MutexHandle,
}

/// Result of claiming the single-instance lock.
pub enum Claim {
    /// We are the only instance — keep running.
    Primary(InstanceGuard),
    /// Another instance is already running; command was forwarded (or silent exit).
    Secondary,
}

/// Try to become the sole instance. If another is running, forward `cmd` (unless
/// this is a duplicate quiet `--tray` launch) and return [`Claim::Secondary`].
pub fn claim_or_forward(cmd: &IpcCommand, quiet_tray: bool) -> Claim {
    match try_acquire_mutex() {
        Ok(guard) => Claim::Primary(guard),
        Err(e) => {
            log::debug!("mutex busy ({e:#}); forwarding ipc={cmd:?} quiet_tray={quiet_tray}");
            if quiet_tray && cmd.path.is_none() && !cmd.greenluma {
                log::info!("secondary quiet --tray exit");
                return Claim::Secondary;
            }
            let mut forward = cmd.clone();
            forward.show = true;
            match write_command(&forward) {
                Ok(()) => log::info!("ipc forwarded: {forward:?}"),
                Err(e) => {
                    log::warn!("ipc write failed ({e:#}), retrying…");
                    std::thread::sleep(Duration::from_millis(150));
                    match write_command(&forward) {
                        Ok(()) => log::info!("ipc forwarded on retry: {forward:?}"),
                        Err(e2) => log::error!("ipc forward failed: {e2:#}"),
                    }
                }
            }
            Claim::Secondary
        }
    }
}

fn ipc_path() -> Result<PathBuf> {
    Ok(crate::settings::app_data_dir()?.join(IPC_FILE))
}

fn write_command(cmd: &IpcCommand) -> Result<()> {
    let path = ipc_path()?;
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_string(cmd).context("serialize ipc")?;
    fs::write(&tmp, json).context("write ipc tmp")?;
    fs::rename(&tmp, &path).context("rename ipc")?;
    Ok(())
}

/// Take a pending command left by a second process (if any).
pub fn take_command() -> Option<IpcCommand> {
    let path = ipc_path().ok()?;
    if !path.is_file() {
        return None;
    }
    let raw = fs::read_to_string(&path).ok()?;
    let _ = fs::remove_file(&path);
    match serde_json::from_str(&raw) {
        Ok(cmd) => {
            log::info!("ipc received: {cmd:?}");
            Some(cmd)
        }
        Err(e) => {
            log::error!("ipc parse failed: {e:#} raw={raw}");
            None
        }
    }
}

#[cfg(windows)]
fn try_acquire_mutex() -> Result<InstanceGuard> {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = std::ffi::OsStr::new(MUTEX_NAME)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let handle = unsafe { CreateMutexW(std::ptr::null_mut(), 1, wide.as_ptr()) };
    if handle.is_null() {
        anyhow::bail!("CreateMutexW failed");
    }
    let err = unsafe { GetLastError() };
    if err == ERROR_ALREADY_EXISTS {
        unsafe {
            CloseHandle(handle);
        }
        anyhow::bail!("already running");
    }
    Ok(InstanceGuard {
        _mutex: MutexHandle(handle),
    })
}

#[cfg(not(windows))]
fn try_acquire_mutex() -> Result<InstanceGuard> {
    Ok(InstanceGuard {})
}

#[cfg(windows)]
const ERROR_ALREADY_EXISTS: u32 = 183;

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn CreateMutexW(
        lp_mutex_attributes: *mut std::ffi::c_void,
        b_initial_owner: i32,
        lp_name: *const u16,
    ) -> *mut std::ffi::c_void;
    fn GetLastError() -> u32;
    fn CloseHandle(h_object: *mut std::ffi::c_void) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipc_round_trip_json() {
        let cmd = IpcCommand {
            greenluma: true,
            path: Some(PathBuf::from(r"C:\Games\foo.exe")),
            show: true,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        let parsed: IpcCommand = serde_json::from_str(&json).unwrap();
        assert!(parsed.greenluma);
        assert_eq!(parsed.path, cmd.path);
    }
}
