//! GreenLuma install (AppData stealth any-folder), Steam path, AppList, CSF merge.

use crate::archive;
use crate::settings::{self, AppSettings};
use anyhow::{anyhow, bail, Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

const BUNDLED_HASHES: &str = include_str!("../assets/greenluma/hashes.json");
/// Embedded archive bytes (password in hashes.json).
const BUNDLED_ARCHIVE: &[u8] = include_bytes!("../assets/greenluma/GreenLuma_2026.7z");

const REQUIRED_FILES: &[&str] = &[
    "GreenLuma_2026_x64.dll",
    "GreenLuma_2026_x86.dll",
    "DLLInjector.exe",
];

#[derive(Debug, Clone, Deserialize)]
struct HashManifest {
    version: String,
    archive: String,
    password: String,
    files: std::collections::HashMap<String, String>,
}

fn manifest() -> &'static HashManifest {
    static M: OnceLock<HashManifest> = OnceLock::new();
    M.get_or_init(|| {
        serde_json::from_str(BUNDLED_HASHES).expect("bundled greenluma hashes.json")
    })
}

pub fn bundled_password() -> &'static str {
    &manifest().password
}

pub fn bundled_version() -> &'static str {
    &manifest().version
}

pub fn install_dir() -> Result<PathBuf> {
    let dir = settings::app_data_dir()?.join("greenluma");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn applist_dir() -> Result<PathBuf> {
    let dir = install_dir()?.join("AppList");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// True when required files exist and match SHA256 whitelist.
pub fn is_installed() -> bool {
    verify_install().is_ok()
}

pub fn verify_install() -> Result<()> {
    let dir = install_dir()?;
    log::debug!("verify_install dir={}", dir.display());
    let m = manifest();
    for name in REQUIRED_FILES {
        let path = dir.join(name);
        if !path.is_file() {
            log::error!("verify_install missing {name}");
            bail!("Missing {name} — GreenLuma not installed (or removed by antivirus)");
        }
        let expected = m
            .files
            .get(*name)
            .ok_or_else(|| anyhow!("No hash entry for {name}"))?;
        let actual = sha256_file(&path)?;
        if !actual.eq_ignore_ascii_case(expected) {
            log::error!("SHA256 mismatch {name} expected={expected} actual={actual}");
            bail!(
                "SHA256 mismatch for {name} — not an original GreenLuma file (expected {expected}, got {actual})"
            );
        }
    }
    log::debug!("verify_install ok");
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect())
}

/// Extract bundled PW archive into install dir and verify hashes.
pub fn install_from_bundled() -> Result<()> {
    let dir = install_dir()?;
    log::info!("install_from_bundled → {}", dir.display());
    // Clean previous install (keep AppList)
    let applist = dir.join("AppList");
    let saved_applist = if applist.is_dir() {
        let tmp = settings::app_data_dir()?.join("greenluma_applist_bak");
        let _ = fs::remove_dir_all(&tmp);
        fs::rename(&applist, &tmp).ok();
        Some(tmp)
    } else {
        None
    };

    for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
        let p = entry.path();
        if p.file_name().and_then(|n| n.to_str()) == Some("AppList") {
            continue;
        }
        let _ = if p.is_dir() {
            fs::remove_dir_all(&p)
        } else {
            fs::remove_file(&p)
        };
    }

    let archive_path = dir.join(manifest().archive.as_str());
    fs::write(&archive_path, BUNDLED_ARCHIVE)
        .context("Failed to write bundled GreenLuma archive")?;
    archive::extract_archive(&archive_path, &dir, bundled_password())
        .context("Failed to extract GreenLuma (check Defender exclusions)")?;
    let _ = fs::remove_file(&archive_path);

    if let Some(bak) = saved_applist {
        let _ = fs::remove_dir_all(&applist);
        let _ = fs::rename(&bak, &applist);
    }

    match verify_install() {
        Ok(()) => {
            write_injector_ini()?;
            log::info!("install_from_bundled ok version={}", bundled_version());
            Ok(())
        }
        Err(e) => {
            log::error!("install_from_bundled verify failed: {e:#}");
            Err(e)
        }
    }
}

/// Install from a user-dropped archive (must contain required files with matching SHA).
/// Returns the password that unlocked the archive (may be empty).
pub fn install_from_archive(archive: &Path, passwords: &[String]) -> Result<String> {
    let dir = install_dir()?;
    let staging = settings::app_data_dir()?.join("greenluma_staging");
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging)?;

    let mut try_pws = passwords.to_vec();
    try_pws.insert(0, bundled_password().to_string());
    try_pws.insert(0, "cs.rin.ru".into());

    let pw = archive::extract_with_password_tries(archive, &staging, &try_pws)?;

    // Find folder that contains DLLInjector.exe (may be nested)
    let root = find_dir_with_file(&staging, "DLLInjector.exe")
        .ok_or_else(|| anyhow!("Archive does not contain DLLInjector.exe"))?;

    for name in REQUIRED_FILES {
        let src = root.join(name);
        if !src.is_file() {
            // search recursively
            let found = find_file_named(&staging, name)
                .ok_or_else(|| anyhow!("Archive missing required file: {name}"))?;
            let expected = manifest()
                .files
                .get(*name)
                .ok_or_else(|| anyhow!("No hash for {name}"))?;
            let actual = sha256_file(&found)?;
            if !actual.eq_ignore_ascii_case(expected) {
                let _ = fs::remove_dir_all(&staging);
                bail!("SHA256 mismatch for {name} — modified/unofficial file rejected");
            }
            fs::copy(&found, dir.join(name))?;
        } else {
            let expected = manifest()
                .files
                .get(*name)
                .ok_or_else(|| anyhow!("No hash for {name}"))?;
            let actual = sha256_file(&src)?;
            if !actual.eq_ignore_ascii_case(expected) {
                let _ = fs::remove_dir_all(&staging);
                bail!("SHA256 mismatch for {name} — modified/unofficial file rejected");
            }
            fs::copy(&src, dir.join(name))?;
        }
    }

    // Optional extras
    for name in ["GreenLumaSettings_2026.exe", "AppListManager.exe"] {
        if let Some(found) = find_file_named(&staging, name) {
            if let Some(expected) = manifest().files.get(name) {
                if sha256_file(&found)?.eq_ignore_ascii_case(expected) {
                    let _ = fs::copy(&found, dir.join(name));
                }
            } else {
                let _ = fs::copy(&found, dir.join(name));
            }
        }
    }
    if let Some(files_dir) = find_dir_named(&staging, "GreenLuma2026_Files") {
        let dest = dir.join("GreenLuma2026_Files");
        let _ = fs::remove_dir_all(&dest);
        copy_dir_recursive(&files_dir, &dest)?;
    }

    let _ = fs::remove_dir_all(&staging);
    write_injector_ini()?;
    verify_install()?;
    Ok(pw)
}

fn find_file_named(root: &Path, name: &str) -> Option<PathBuf> {
    walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(|e| e.ok())
        .find(|e| e.file_name().eq_ignore_ascii_case(name))
        .map(|e| e.path().to_path_buf())
}

fn find_dir_named(root: &Path, name: &str) -> Option<PathBuf> {
    walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(|e| e.ok())
        .find(|e| e.file_type().is_dir() && e.file_name().eq_ignore_ascii_case(name))
        .map(|e| e.path().to_path_buf())
}

fn find_dir_with_file(root: &Path, file_name: &str) -> Option<PathBuf> {
    find_file_named(root, file_name).and_then(|p| p.parent().map(|p| p.to_path_buf()))
}

fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;
    for entry in walkdir::WalkDir::new(src).into_iter().filter_map(|e| e.ok()) {
        let rel = entry.path().strip_prefix(src).unwrap_or(entry.path());
        let target = dest.join(rel);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target)?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

// --- Steam path ---

#[derive(Debug, Clone)]
pub struct SteamPaths {
    pub steam_exe: PathBuf,
    #[allow(dead_code)]
    pub steam_dir: PathBuf,
    pub library_root: PathBuf,
}

pub fn detect_steam(settings: &AppSettings) -> Result<SteamPaths> {
    if let Some(exe) = &settings.steam_exe_override {
        if exe.is_file() {
            let steam_dir = exe.parent().unwrap_or(exe.as_path()).to_path_buf();
            let library_root = settings
                .steam_library_override
                .clone()
                .filter(|p| p.is_dir())
                .unwrap_or_else(|| steam_dir.clone());
            log::info!(
                "steam override exe={} library={}",
                exe.display(),
                library_root.display()
            );
            return Ok(SteamPaths {
                steam_exe: exe.clone(),
                steam_dir,
                library_root,
            });
        }
        log::warn!(
            "steam_exe_override set but missing: {}",
            exe.display()
        );
    }

    let steam_dir = detect_steam_dir()?;
    let steam_exe = steam_dir.join("steam.exe");
    if !steam_exe.is_file() {
        log::error!("Steam.exe missing under {}", steam_dir.display());
        bail!("Steam.exe not found under {}", steam_dir.display());
    }
    let library_root = settings
        .steam_library_override
        .clone()
        .filter(|p| p.is_dir())
        .or_else(|| primary_library(&steam_dir))
        .unwrap_or_else(|| steam_dir.clone());

    log::debug!(
        "steam detected exe={} library={}",
        steam_exe.display(),
        library_root.display()
    );
    Ok(SteamPaths {
        steam_exe,
        steam_dir,
        library_root,
    })
}

fn detect_steam_dir() -> Result<PathBuf> {
    #[cfg(windows)]
    {
        use winreg::enums::*;
        use winreg::RegKey;
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if let Ok(key) = hkcu.open_subkey(r"Software\Valve\Steam") {
            if let Ok(path) = key.get_value::<String, _>("SteamPath") {
                let p = PathBuf::from(path.replace('/', "\\"));
                if p.is_dir() {
                    return Ok(p);
                }
            }
        }
        for (hive, sub) in [
            (HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Valve\Steam"),
            (HKEY_LOCAL_MACHINE, r"SOFTWARE\Valve\Steam"),
        ] {
            let root = RegKey::predef(hive);
            if let Ok(key) = root.open_subkey(sub) {
                if let Ok(path) = key.get_value::<String, _>("InstallPath") {
                    let p = PathBuf::from(path);
                    if p.is_dir() {
                        return Ok(p);
                    }
                }
            }
        }
    }
    for candidate in [
        r"C:\Program Files (x86)\Steam",
        r"C:\Program Files\Steam",
    ] {
        let p = PathBuf::from(candidate);
        if p.join("steam.exe").is_file() {
            return Ok(p);
        }
    }
    bail!("Could not detect Steam. Set the path in Settings.")
}

fn primary_library(steam_dir: &Path) -> Option<PathBuf> {
    let vdf = steam_dir.join("steamapps").join("libraryfolders.vdf");
    if !vdf.is_file() {
        return Some(steam_dir.to_path_buf());
    }
    let text = fs::read_to_string(&vdf).ok()?;
    // "path"		"D:\\SteamLibrary"
    let re = Regex::new(r#"(?i)"path"\s+"([^"]+)""#).ok()?;
    for caps in re.captures_iter(&text) {
        let path = PathBuf::from(caps[1].replace("\\\\", "\\"));
        if path.join("steamapps").is_dir() {
            return Some(path);
        }
    }
    Some(steam_dir.to_path_buf())
}

pub fn is_steam_running() -> bool {
    process_running("steam.exe")
}

/// True while a Steam/library game process is running — defer download alerts.
pub fn is_game_session_active() -> bool {
    #[cfg(windows)]
    {
        if tracked_game_process_running() {
            return true;
        }
        if steamapps_common_process_running() {
            return true;
        }
    }
    false
}

#[cfg(windows)]
fn tracked_game_process_running() -> bool {
    for g in crate::games::load() {
        let Some(name) = g.exe_path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if !process_running(name) {
            continue;
        }
        // Confirm full path when possible (avoids common exe-name collisions).
        if let Some(paths) = process_image_paths(name) {
            let want = g.exe_path.to_string_lossy().replace('/', "\\").to_ascii_lowercase();
            if paths.iter().any(|p| p == &want || p.ends_with(&want)) {
                return true;
            }
            // Fallback: same file name under steamapps\common
            if paths.iter().any(|p| p.contains("\\steamapps\\common\\")) {
                return true;
            }
        } else {
            return true;
        }
    }
    false
}

#[cfg(windows)]
fn steamapps_common_process_running() -> bool {
    let Some(paths) = all_process_image_paths() else {
        return false;
    };
    for p in paths {
        if !p.contains("\\steamapps\\common\\") {
            continue;
        }
        if is_non_game_steamapps_path(&p) {
            continue;
        }
        log::debug!("game session: steamapps process {p}");
        return true;
    }
    false
}

#[cfg(windows)]
fn is_non_game_steamapps_path(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    // Helpers / redistribs that live under common but aren't "in a game".
    const SKIP: &[&str] = &[
        "\\steamworks shared\\",
        "\\steamworks redistributable",
        "\\directx",
        "\\vcredist",
        "\\dotnet",
        "\\easyanticheat",
        "\\battleye",
        "\\uninstall",
        "crashhandler",
        "crashreport",
        "redist\\",
    ];
    SKIP.iter().any(|s| p.contains(s))
}

fn process_running(image: &str) -> bool {
    let filter = format!("IMAGENAME eq {image}");
    let mut cmd = Command::new("tasklist");
    cmd.args(["/FI", &filter, "/NH"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.output()
        .ok()
        .map(|o| {
            let s = String::from_utf8_lossy(&o.stdout).to_lowercase();
            s.contains(&image.to_lowercase())
        })
        .unwrap_or(false)
}

fn steam_still_alive() -> bool {
    process_running("steam.exe") || process_running("steamwebhelper.exe")
}

#[cfg(windows)]
fn process_image_paths(image_name: &str) -> Option<Vec<String>> {
    let want = image_name.to_ascii_lowercase();
    let all = all_process_image_paths()?;
    let matched: Vec<String> = all
        .into_iter()
        .filter(|p| {
            Path::new(p)
                .file_name()
                .and_then(|s| s.to_str())
                .map(|n| n.eq_ignore_ascii_case(&want))
                .unwrap_or(false)
        })
        .collect();
    if matched.is_empty() {
        None
    } else {
        Some(matched)
    }
}

#[cfg(windows)]
fn all_process_image_paths() -> Option<Vec<String>> {
    use std::mem::{size_of, zeroed};
    use std::os::windows::ffi::OsStringExt;

    #[repr(C)]
    struct ProcessEntry32W {
        dw_size: u32,
        cnt_usage: u32,
        th32_process_id: u32,
        th32_default_heap_id: usize,
        th32_module_id: u32,
        cnt_threads: u32,
        th32_parent_process_id: u32,
        pc_pri_class_base: i32,
        dw_flags: u32,
        sz_exe_file: [u16; 260],
    }

    extern "system" {
        fn CreateToolhelp32Snapshot(flags: u32, process_id: u32) -> *mut std::ffi::c_void;
        fn Process32FirstW(snapshot: *mut std::ffi::c_void, entry: *mut ProcessEntry32W) -> i32;
        fn Process32NextW(snapshot: *mut std::ffi::c_void, entry: *mut ProcessEntry32W) -> i32;
        fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
        fn OpenProcess(access: u32, inherit: i32, process_id: u32) -> *mut std::ffi::c_void;
        fn QueryFullProcessImageNameW(
            process: *mut std::ffi::c_void,
            flags: u32,
            exe_name: *mut u16,
            size: *mut u32,
        ) -> i32;
    }

    const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const INVALID: *mut std::ffi::c_void = -1isize as *mut std::ffi::c_void;

    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap.is_null() || snap == INVALID {
            return None;
        }

        let mut entry: ProcessEntry32W = zeroed();
        entry.dw_size = size_of::<ProcessEntry32W>() as u32;
        let mut out = Vec::new();

        if Process32FirstW(snap, &mut entry) != 0 {
            loop {
                let pid = entry.th32_process_id;
                if pid != 0 {
                    let proc = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
                    if !proc.is_null() {
                        let mut buf = [0u16; 512];
                        let mut len = buf.len() as u32;
                        if QueryFullProcessImageNameW(proc, 0, buf.as_mut_ptr(), &mut len) != 0
                            && len > 0
                        {
                            let path = std::ffi::OsString::from_wide(&buf[..len as usize]);
                            out.push(path.to_string_lossy().replace('/', "\\").to_ascii_lowercase());
                        }
                        CloseHandle(proc);
                    }
                }
                if Process32NextW(snap, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
        Some(out)
    }
}

fn shutdown_steam(steam_exe: &Path) -> Result<()> {
    if !steam_still_alive() {
        return Ok(());
    }
    log::info!("sending steam -shutdown to {}", steam_exe.display());
    let mut cmd = Command::new(steam_exe);
    cmd.arg("-shutdown");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let _ = cmd.spawn();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(45);
    while steam_still_alive() {
        if std::time::Instant::now() > deadline {
            log::warn!("Steam still alive after -shutdown; forcing taskkill");
            for image in ["steam.exe", "steamwebhelper.exe"] {
                let mut kill = Command::new("taskkill");
                kill.args(["/F", "/IM", image, "/T"]);
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
                    kill.creation_flags(CREATE_NO_WINDOW);
                }
                let _ = kill.output();
            }
            std::thread::sleep(std::time::Duration::from_millis(1500));
            if steam_still_alive() {
                bail!("Steam did not exit in time after -shutdown");
            }
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(400));
    }
    log::info!("Steam exited; settling");
    std::thread::sleep(std::time::Duration::from_millis(1200));
    Ok(())
}

/// Quit Steam if needed, then start via DLLInjector so a fresh AppList is loaded.
pub fn restart_steam_injected() -> Result<()> {
    log::info!("restart_steam_injected");
    verify_install()?;
    write_injector_ini()?;
    let _ = ensure_applist_synced();
    let settings = AppSettings::load();
    let steam = detect_steam(&settings)?;
    shutdown_steam(&steam.steam_exe)?;
    spawn_injector()?;
    set_steam_mode_greenluma(true);
    Ok(())
}

/// Quit Steam if needed, then start plain `steam.exe` (no GreenLuma inject).
pub fn start_steam_plain() -> Result<()> {
    log::info!("start_steam_plain");
    let settings = AppSettings::load();
    let steam = detect_steam(&settings)?;
    shutdown_steam(&steam.steam_exe)?;
    log::info!("spawning plain Steam {}", steam.steam_exe.display());
    let mut cmd = Command::new(&steam.steam_exe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.spawn()
        .with_context(|| format!("Failed to start {}", steam.steam_exe.display()))?;
    set_steam_mode_greenluma(false);
    Ok(())
}

pub(crate) fn set_steam_mode_greenluma(active: bool) {
    let mut s = AppSettings::load();
    s.steam_last_mode_greenluma = active;
    if let Err(e) = s.save() {
        log::warn!("failed to persist steam_last_mode_greenluma: {e:#}");
    }
    log::info!("steam mode greenluma={active}");
}

/// AppIDs with an active Steam download/update (ACF bytes and/or downloading/).
pub fn scan_active_downloads(settings: &AppSettings) -> Vec<(u32, String)> {
    let Ok(steam) = detect_steam(settings) else {
        return Vec::new();
    };
    let mut roots = library_roots(&steam.steam_dir);
    if let Some(ovr) = &settings.steam_library_override {
        if ovr.is_dir() && !roots.iter().any(|r| r == ovr) {
            roots.push(ovr.clone());
        }
    }

    let mut out: Vec<(u32, String)> = Vec::new();
    for root in &roots {
        let steamapps = root.join("steamapps");
        push_downloading_hits(&steamapps.join("downloading"), root, &mut out);
        push_downloading_hits(&steamapps.join("temp"), root, &mut out);
        push_acf_download_hits(&steamapps, &mut out);
    }
    out
}

fn push_hit(out: &mut Vec<(u32, String)>, app_id: u32, name: String) {
    if !out.iter().any(|(id, _)| *id == app_id) {
        out.push((app_id, name));
    }
}

fn push_downloading_hits(dir: &Path, library_root: &Path, out: &mut Vec<(u32, String)>) {
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        if !ft.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let Some(id_str) = name.to_str() else {
            continue;
        };
        let Ok(app_id) = id_str.parse::<u32>() else {
            continue;
        };
        if !dir_has_any_entry(entry.path()) {
            continue;
        }
        let display =
            acf_display_name(library_root, app_id).unwrap_or_else(|| format!("App {app_id}"));
        push_hit(out, app_id, display);
    }
}

fn dir_has_any_entry(path: PathBuf) -> bool {
    fs::read_dir(path)
        .ok()
        .map(|mut i| i.next().is_some())
        .unwrap_or(false)
}

fn push_acf_download_hits(steamapps: &Path, out: &mut Vec<(u32, String)>) {
    let Ok(rd) = fs::read_dir(steamapps) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let Some(fname) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let lower = fname.to_ascii_lowercase();
        if !lower.starts_with("appmanifest_") || !lower.ends_with(".acf") {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        if !acf_looks_downloading(&text) {
            continue;
        }
        let app_id = capture_acf(&text, "appid")
            .and_then(|s| s.parse().ok())
            .or_else(|| {
                lower
                    .trim_start_matches("appmanifest_")
                    .trim_end_matches(".acf")
                    .parse()
                    .ok()
            });
        let Some(app_id) = app_id else {
            continue;
        };
        let name = capture_acf(&text, "name")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("App {app_id}"));
        push_hit(out, app_id, name);
    }
}

/// True when the manifest indicates an in-progress download/update.
fn acf_looks_downloading(text: &str) -> bool {
    let to = capture_acf_u64(text, "BytesToDownload").unwrap_or(0);
    let got = capture_acf_u64(text, "BytesDownloaded").unwrap_or(0);
    if to > 0 && got < to {
        return true;
    }
    let stage_to = capture_acf_u64(text, "BytesToStage").unwrap_or(0);
    let staged = capture_acf_u64(text, "BytesStaged").unwrap_or(0);
    if stage_to > 0 && staged < stage_to {
        return true;
    }
    // SteamKit EAppState: UpdateStarted = 512; some builds also use 1024.
    let flags = capture_acf_u64(text, "StateFlags").unwrap_or(0);
    (flags & 512) != 0 || (flags & 1024) != 0
}

fn capture_acf_u64(text: &str, key: &str) -> Option<u64> {
    capture_acf(text, key)?.parse().ok()
}

fn library_roots(steam_dir: &Path) -> Vec<PathBuf> {
    let mut roots = vec![steam_dir.to_path_buf()];
    let vdf = steam_dir.join("steamapps").join("libraryfolders.vdf");
    let Ok(text) = fs::read_to_string(&vdf) else {
        return roots;
    };
    let Ok(re) = Regex::new(r#"(?i)"path"\s+"([^"]+)""#) else {
        return roots;
    };
    for caps in re.captures_iter(&text) {
        let raw = caps[1].replace("\\\\", "\\");
        let path = PathBuf::from(raw);
        if path.is_dir() && !roots.iter().any(|r| r == &path) {
            roots.push(path);
        }
    }
    roots
}

fn acf_display_name(library_root: &Path, app_id: u32) -> Option<String> {
    let acf = library_root
        .join("steamapps")
        .join(format!("appmanifest_{app_id}.acf"));
    let text = fs::read_to_string(acf).ok()?;
    capture_acf(&text, "name").map(|s| s.trim().to_string())
}

fn spawn_injector() -> Result<()> {
    let dir = install_dir()?;
    let injector = dir.join("DLLInjector.exe");
    log::info!(
        "spawning DLLInjector cwd={} exe={}",
        dir.display(),
        injector.display()
    );
    let mut cmd = Command::new(&injector);
    cmd.current_dir(&dir);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.spawn()
        .with_context(|| format!("Failed to start {}", injector.display()))?;
    Ok(())
}

pub fn write_injector_ini() -> Result<()> {
    let dir = install_dir()?;
    let settings = AppSettings::load();
    let steam = detect_steam(&settings)?;
    let dll = dir.join("GreenLuma_2026_x64.dll");
    // Full Steam006 template — DLLInjector errors with "GetPrivateProfileInt Failed!"
    // if the mitigation / BootImage integer keys are missing.
    let ini = format!(
        r#"[DllInjector]
AllowMultipleInstancesOfDLLInjector = 0
UseFullPathsFromIni = 1

Exe = {exe}
CommandLine = -inhibitbootstrap

Dll = {dll}
Export = Init
CheckReturnValue = 0
WaitForProcessTermination = 0

EnableFakeParentProcess = 0
FakeParentProcess = explorer.exe
EnableMitigationsOnChildProcess = 0

DEP = 1
SEHOP = 1
HeapTerminate = 1
ForceRelocateImages = 1
BottomUpASLR = 1
HighEntropyASLR = 1
RelocationsRequired = 1
StrictHandleChecks = 0
Win32kSystemCallDisable = 0
ExtensionPointDisable = 1
CFG = 1
CFGExportSuppression = 1
StrictCFG = 1
DynamicCodeDisable = 0
DynamicCodeAllowOptOut = 0
BlockNonMicrosoftBinaries = 0
FontDisable = 1
NoRemoteImages = 1
NoLowLabelImages = 1
PreferSystem32 = 0
RestrictIndirectBranchPrediction = 1
SpeculativeStoreBypassDisable = 0
ShadowStack = 0
ContextIPValidation = 0
BlockNonCETEHCONT = 0
BlockFSCTL = 0

CreateFiles = 1
FileToCreate_1 = NoQuestion.bin
FileToCreate_2 =

Use4GBPatch = 0
FileToPatch_1 =

BootImage = GreenLuma2026_Files\BootImage.bmp
BootImageWidth = 500
BootImageHeight = 500
BootImageXOffest = 240
BootImageYOffest = 280
"#,
        exe = steam.steam_exe.display(),
        dll = dll.display(),
    );
    fs::write(dir.join("DLLInjector.ini"), ini)?;
    ensure_no_question_bin(&steam.steam_dir)?;
    log::debug!(
        "wrote DLLInjector.ini exe={} dll={} (+ NoQuestion.bin)",
        steam.steam_exe.display(),
        dll.display()
    );
    Ok(())
}

/// GreenLuma shows "Use saved AppList?" unless `NoQuestion.bin` exists next to
/// AppList (Steam006: CreateFile1 NoQuestion.bin / same file on disk).
fn ensure_no_question_bin(steam_dir: &Path) -> Result<()> {
    let empty: &[u8] = &[];
    let our = install_dir()?.join("NoQuestion.bin");
    fs::write(&our, empty)?;
    let steam_marker = steam_dir.join("NoQuestion.bin");
    if let Err(e) = fs::write(&steam_marker, empty) {
        log::warn!(
            "Could not write {} ({e:#}) — dialog may still appear",
            steam_marker.display()
        );
    } else {
        log::debug!("NoQuestion.bin at {} and {}", our.display(), steam_marker.display());
    }
    Ok(())
}

/// Start Steam via DLLInjector (stealth any-folder). No-op if Steam already runs.
pub fn start_steam_injected() -> Result<()> {
    log::info!("start_steam_injected");
    verify_install()?;
    write_injector_ini()?;
    let _ = ensure_applist_synced();
    if is_steam_running() {
        log::info!("Steam already running — skip inject");
        return Ok(());
    }
    spawn_injector()?;
    set_steam_mode_greenluma(true);
    Ok(())
}

// --- AppList ---
//
// Official format: AppList/0.txt, 1.txt, … each file contains only the AppID.
// GreenLuma (injected into Steam) also reads Steam\AppList — keep both in sync
// by merging, never wiping one for the other. That avoids the “use existing
// AppList?” prompt when the two folders disagree.

const APPLIST_NAMES_FILE: &str = "applist_names.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppListEntry {
    pub app_id: u32,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct AppListAddResult {
    /// True if this AppID was newly appended.
    pub added: bool,
    /// True if Steam was restarted (or started) so GreenLuma reloads AppList.
    pub restarted: bool,
}

fn steam_applist_dir() -> Option<PathBuf> {
    let settings = AppSettings::load();
    detect_steam(&settings)
        .ok()
        .map(|s| s.steam_dir.join("AppList"))
}

fn read_ids_from_dir(dir: &Path) -> Vec<u32> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<_> = rd.filter_map(|e| e.ok()).collect();
    files.sort_by_key(|e| {
        e.file_name()
            .to_string_lossy()
            .trim_end_matches(".txt")
            .parse::<u32>()
            .unwrap_or(u32::MAX)
    });
    let mut ids = Vec::new();
    for e in files {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some("txt") {
            continue;
        }
        if let Ok(text) = fs::read_to_string(&path) {
            let id_line = text.lines().next().unwrap_or("").trim();
            if let Ok(app_id) = id_line.parse::<u32>() {
                if !ids.contains(&app_id) {
                    ids.push(app_id);
                }
            }
        }
    }
    ids
}

/// Union of our AppList + Steam\AppList (Steam order first, then ours).
pub fn collect_merged_ids() -> Result<Vec<u32>> {
    let mut ids = Vec::new();
    if let Some(steam_dir) = steam_applist_dir() {
        for id in read_ids_from_dir(&steam_dir) {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
    }
    if let Ok(our) = applist_dir() {
        for id in read_ids_from_dir(&our) {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
    }
    Ok(ids)
}

fn names_path() -> Result<PathBuf> {
    Ok(install_dir()?.join(APPLIST_NAMES_FILE))
}

fn load_names_meta() -> std::collections::HashMap<u32, String> {
    let Ok(path) = names_path() else {
        return Default::default();
    };
    fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_names_meta(map: &std::collections::HashMap<u32, String>) -> Result<()> {
    let path = names_path()?;
    let text = serde_json::to_string_pretty(map)?;
    fs::write(path, text)?;
    Ok(())
}

fn rewrite_applist_dir(dir: &Path, ids: &[u32]) -> Result<()> {
    fs::create_dir_all(dir)?;
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) == Some("txt") {
                let _ = fs::remove_file(&path);
            }
        }
    }
    for (i, id) in ids.iter().enumerate() {
        // Official GreenLuma format: AppID only.
        fs::write(dir.join(format!("{i}.txt")), format!("{id}\n"))?;
    }
    Ok(())
}

fn write_applist_everywhere(ids: &[u32]) -> Result<()> {
    log::info!("AppList sync ids={ids:?} count={}", ids.len());
    rewrite_applist_dir(&applist_dir()?, ids)?;
    if let Some(steam_dir) = steam_applist_dir() {
        // Best-effort — Program Files may need elevation on locked installs.
        match rewrite_applist_dir(&steam_dir, ids) {
            Ok(()) => log::debug!("synced Steam AppList {}", steam_dir.display()),
            Err(e) => {
                log::warn!(
                    "Could not sync Steam\\AppList ({e:#}); using GoldbergDrop AppList only"
                );
            }
        }
    } else {
        log::debug!("no Steam AppList dir (Steam undetected)");
    }
    Ok(())
}

/// Merge both AppList folders and rewrite them identically (ID-only files).
pub fn ensure_applist_synced() -> Result<()> {
    let ids = collect_merged_ids()?;
    write_applist_everywhere(&ids)
}

pub fn read_applist() -> Vec<AppListEntry> {
    let names = load_names_meta();
    collect_merged_ids()
        .unwrap_or_default()
        .into_iter()
        .map(|app_id| AppListEntry {
            name: names
                .get(&app_id)
                .cloned()
                .unwrap_or_else(|| format!("App {app_id}")),
            app_id,
        })
        .collect()
}

/// Append AppID to the existing list (never replaces other entries).
/// Restarts Steam with GreenLuma when the ID is new, or when `force_restart`
/// is set (CSF install always needs a fresh Steam so the library reload sticks).
pub fn add_to_applist(app_id: u32, name: &str, force_restart: bool) -> Result<AppListAddResult> {
    log::info!("add_to_applist app_id={app_id} name={name} force_restart={force_restart}");
    let mut names = load_names_meta();
    if !name.trim().is_empty() {
        names.insert(app_id, name.trim().to_string());
        let _ = save_names_meta(&names);
    }

    let mut ids = collect_merged_ids()?;
    log::debug!("merged AppList before add: {ids:?}");
    let added = !ids.contains(&app_id);
    if added {
        ids.push(app_id);
    }
    write_applist_everywhere(&ids)?;

    let should_restart = added || force_restart;
    if !should_restart {
        log::info!("add_to_applist duplicate {app_id}, no restart");
        return Ok(AppListAddResult {
            added: false,
            restarted: false,
        });
    }

    match restart_steam_injected() {
        Ok(()) => {
            log::info!(
                "add_to_applist ok app_id={app_id} added={added} restarted=true"
            );
            Ok(AppListAddResult {
                added,
                restarted: true,
            })
        }
        Err(e) => {
            log::error!("AppList updated but Steam restart failed: {e:#}");
            Err(e).context("AppList written but Steam restart failed")
        }
    }
}

// --- ACF / CSF ---

#[derive(Debug, Clone)]
pub struct AcfInfo {
    pub app_id: u32,
    pub name: String,
    pub installdir: String,
    #[allow(dead_code)]
    pub buildid: Option<String>,
}

pub fn parse_acf(text: &str) -> Result<AcfInfo> {
    let app_id = capture_acf(text, "appid")
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| anyhow!("ACF missing appid"))?;
    let name = capture_acf(text, "name")
        .unwrap_or_else(|| format!("App {app_id}"))
        .trim()
        .to_string();
    let installdir = capture_acf(text, "installdir")
        .ok_or_else(|| anyhow!("ACF missing installdir"))?;
    let buildid = capture_acf(text, "buildid");
    Ok(AcfInfo {
        app_id,
        name,
        installdir,
        buildid,
    })
}

fn capture_acf(text: &str, key: &str) -> Option<String> {
    let re = Regex::new(&format!(r#"(?i)"{key}"\s+"([^"]*)""#)).ok()?;
    re.captures(text).map(|c| c[1].to_string())
}

/// Validate CSF listing: has appmanifest and matching common/installdir.
pub fn validate_csf_layout(paths: &[String]) -> Result<AcfInfo> {
    if !archive::is_steam_library_layout(paths) {
        bail!("Not a Steam library pack (need steamapps/appmanifest_*.acf)");
    }
    // We need the ACF content — caller should extract or we parse path only for app id
    let acf_path = paths
        .iter()
        .find(|p| {
            let l = p.to_lowercase();
            l.contains("steamapps/appmanifest_") && l.ends_with(".acf")
        })
        .cloned()
        .ok_or_else(|| anyhow!("No appmanifest in archive"))?;

    // App id from filename appmanifest_4262310.acf
    let re = Regex::new(r"(?i)appmanifest_(\d+)\.acf").unwrap();
    let app_id: u32 = re
        .captures(&acf_path)
        .and_then(|c| c[1].parse().ok())
        .ok_or_else(|| anyhow!("Could not parse app id from {acf_path}"))?;

    // installdir: look for steamapps/common/<something>/
    let common_re = Regex::new(r"(?i)steamapps/common/([^/]+)/").unwrap();
    let installdir = paths
        .iter()
        .find_map(|p| common_re.captures(p).map(|c| c[1].to_string()))
        .ok_or_else(|| anyhow!("No steamapps/common/<game>/ in archive"))?;

    if !paths.iter().any(|p| {
        let l = p.replace('\\', "/").to_lowercase();
        l.contains(&format!(
            "steamapps/common/{}/",
            installdir.to_lowercase()
        ))
    }) {
        bail!("installdir folder missing under steamapps/common/");
    }

    Ok(AcfInfo {
        app_id,
        name: installdir.clone(),
        installdir,
        buildid: None,
    })
}

/// Extract CSF archive into Steam library and add AppList entry.
pub fn import_csf_archive(
    archive: &Path,
    settings: &mut AppSettings,
    passwords: &[String],
) -> Result<(AcfInfo, AppListAddResult)> {
    let steam = detect_steam(settings)?;
    let (paths, pw) = archive::list_with_password_tries(archive, passwords)?;
    let mut info = validate_csf_layout(&paths)?;

    log::info!(
        "import_csf archive={} → library={}",
        archive.display(),
        steam.library_root.display()
    );
    archive::extract_archive(archive, &steam.library_root, &pw)?;

    // Prefer name from extracted ACF
    let acf_on_disk = steam
        .library_root
        .join("steamapps")
        .join(format!("appmanifest_{}.acf", info.app_id));
    if let Ok(text) = fs::read_to_string(&acf_on_disk) {
        if let Ok(parsed) = parse_acf(&text) {
            info = parsed;
        }
    }

    let add = add_to_applist(info.app_id, &info.name, true)?;
    remember_password(settings, &pw)?;
    Ok((info, add))
}

fn remember_password(settings: &mut AppSettings, pw: &str) -> Result<()> {
    if pw.is_empty() {
        return Ok(());
    }
    if !settings.archive_passwords.iter().any(|p| p == pw) {
        settings.archive_passwords.push(pw.to_string());
        settings.save()?;
    }
    Ok(())
}

pub fn default_archive_passwords(settings: &AppSettings) -> Vec<String> {
    let mut v = vec!["cs.rin.ru".into(), bundled_password().to_string()];
    for p in &settings.archive_passwords {
        if !v.iter().any(|x| x == p) {
            v.push(p.clone());
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_orb_acf() {
        let text = r#""AppState"
{
	"appid"		"4262310"
	"name"		" All Hail the Orb"
	"installdir"		"All Hail the Orb"
	"buildid"		"23025459"
}"#;
        let info = parse_acf(text).unwrap();
        assert_eq!(info.app_id, 4262310);
        assert_eq!(info.installdir, "All Hail the Orb");
        assert!(info.name.contains("Orb"));
    }

    #[test]
    fn validate_orb_paths() {
        let paths = vec![
            "depotcache/4262311_x.manifest".into(),
            "steamapps/appmanifest_4262310.acf".into(),
            "steamapps/common/All Hail the Orb/All Hail the Orb.exe".into(),
        ];
        let info = validate_csf_layout(&paths).unwrap();
        assert_eq!(info.app_id, 4262310);
    }

    #[test]
    fn acf_download_heuristic() {
        assert!(!acf_looks_downloading(
            r#""BytesToDownload" "0" "BytesDownloaded" "0" "StateFlags" "4""#
        ));
        assert!(acf_looks_downloading(
            r#""BytesToDownload" "1000" "BytesDownloaded" "100" "StateFlags" "6""#
        ));
        assert!(acf_looks_downloading(r#""StateFlags" "1026""#));
        assert!(acf_looks_downloading(r#""StateFlags" "516""#)); // 512|4
    }
}
