//! List/extract zip and 7z archives (via 7-Zip CLI) with password tries.

use anyhow::{anyhow, bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(windows)]
fn no_window(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn no_window(_cmd: &mut Command) {}

/// Locate `7z.exe` (common install dirs, then PATH).
pub fn find_7z() -> Option<PathBuf> {
    for candidate in [
        r"C:\Program Files\7-Zip\7z.exe",
        r"C:\Program Files (x86)\7-Zip\7z.exe",
    ] {
        let p = PathBuf::from(candidate);
        if p.is_file() {
            return Some(p);
        }
    }
    which_7z_on_path().ok()
}

fn which_7z_on_path() -> Result<PathBuf> {
    let mut cmd = Command::new("where");
    cmd.arg("7z.exe");
    no_window(&mut cmd);
    let out = cmd.output().context("where 7z.exe")?;
    if !out.status.success() {
        bail!("7z.exe not on PATH");
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().next().unwrap_or("").trim();
    if line.is_empty() {
        bail!("empty where output");
    }
    Ok(PathBuf::from(line))
}

/// List archive entry paths (forward slashes normalized).
pub fn list_archive(archive: &Path, password: Option<&str>) -> Result<Vec<String>> {
    let sevenz = find_7z().ok_or_else(|| {
        anyhow!("7-Zip not found. Install 7-Zip or add 7z.exe to PATH.")
    })?;
    log::debug!(
        "7z list archive={} sevenz={} password={}",
        archive.display(),
        sevenz.display(),
        if password.map(|p| !p.is_empty()).unwrap_or(false) {
            "set"
        } else {
            "none"
        }
    );
    let mut cmd = Command::new(&sevenz);
    cmd.arg("l").arg("-ba");
    if let Some(pw) = password {
        cmd.arg(format!("-p{pw}"));
    } else {
        cmd.arg("-p");
    }
    cmd.arg(archive);
    no_window(&mut cmd);
    let out = cmd.output().context("Failed to list archive")?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() {
        log::warn!(
            "7z list failed status={:?} stderr={}",
            out.status.code(),
            stderr.trim()
        );
        if looks_like_wrong_password(&stderr) {
            bail!("Wrong password or encrypted archive");
        }
        bail!("7z list failed: {stderr}");
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let paths = parse_7z_list_paths(&stdout);
    log::debug!("7z list ok entries={}", paths.len());
    Ok(paths)
}

fn looks_like_wrong_password(stderr: &str) -> bool {
    let s = stderr.to_lowercase();
    s.contains("wrong password") || s.contains("cannot open encrypted") || s.contains("data error")
}

/// Parse `7z l -ba` lines → entry names (last column).
fn parse_7z_list_paths(stdout: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for line in stdout.lines() {
        let line = line.trim_end();
        if line.len() < 54 {
            continue;
        }
        // -ba format: date time attr size compressed name
        let name = line[53..].trim();
        if name.is_empty() || name == "." || name == ".." {
            continue;
        }
        paths.push(name.replace('\\', "/"));
    }
    paths
}

/// Try passwords in order (empty string = no password). Returns working password (may be empty).
pub fn list_with_password_tries(
    archive: &Path,
    passwords: &[String],
) -> Result<(Vec<String>, String)> {
    let mut tried: Vec<&'static str> = Vec::new();
    // No password first if not already in list
    let mut candidates: Vec<String> = vec![String::new()];
    for p in passwords {
        if !p.is_empty() && !candidates.iter().any(|c| c == p) {
            candidates.push(p.clone());
        }
    }
    log::debug!(
        "list_with_password_tries archive={} candidates={}",
        archive.display(),
        candidates.len()
    );

    let mut last_err = None;
    for pw in &candidates {
        tried.push(if pw.is_empty() { "(none)" } else { "(set)" });
        match list_archive(archive, if pw.is_empty() { None } else { Some(pw) }) {
            Ok(paths) if !paths.is_empty() || pw.is_empty() => {
                // Empty archive is rare; encrypted wrong-pw sometimes returns empty with success.
                // Prefer success with paths, or empty only for unencrypted.
                if !paths.is_empty() || pw.is_empty() {
                    // For password tries: if paths empty and we used a password, keep trying
                    if paths.is_empty() && !pw.is_empty() {
                        continue;
                    }
                    log::info!(
                        "archive opened entries={} password={}",
                        paths.len(),
                        if pw.is_empty() { "none" } else { "ok" }
                    );
                    return Ok((paths, pw.clone()));
                }
            }
            Ok(paths) => {
                log::info!("archive opened entries={}", paths.len());
                return Ok((paths, pw.clone()));
            }
            Err(e) => {
                let msg = e.to_string();
                log::debug!("archive try failed: {msg}");
                if msg.contains("Wrong password") {
                    last_err = Some(e);
                    continue;
                }
                // Other errors: still try next password for encrypted archives
                last_err = Some(e);
            }
        }
    }
    log::error!("archive open failed after {} tries", tried.len());
    Err(last_err.unwrap_or_else(|| anyhow!("Could not open archive (tried {} passwords)", tried.len())))
}

/// Extract archive into `dest` (created if needed), using password if non-empty.
pub fn extract_archive(archive: &Path, dest: &Path, password: &str) -> Result<()> {
    let sevenz = find_7z().ok_or_else(|| {
        anyhow!("7-Zip not found. Install 7-Zip or add 7z.exe to PATH.")
    })?;
    log::info!(
        "7z extract archive={} dest={} password={}",
        archive.display(),
        dest.display(),
        if password.is_empty() { "none" } else { "set" }
    );
    std::fs::create_dir_all(dest)
        .with_context(|| format!("Failed to create {}", dest.display()))?;
    let mut cmd = Command::new(&sevenz);
    cmd.arg("x")
        .arg("-y")
        .arg(format!("-o{}", dest.display()));
    if password.is_empty() {
        cmd.arg("-p");
    } else {
        cmd.arg(format!("-p{password}"));
    }
    cmd.arg(archive);
    no_window(&mut cmd);
    let out = cmd.output().context("Failed to extract archive")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        log::error!(
            "7z extract failed status={:?} stderr={}",
            out.status.code(),
            stderr.trim()
        );
        if looks_like_wrong_password(&stderr) {
            bail!("Wrong password");
        }
        bail!("7z extract failed: {stderr}");
    }
    log::info!("7z extract ok");
    Ok(())
}

/// Extract with password tries; returns the password that worked.
pub fn extract_with_password_tries(
    archive: &Path,
    dest: &Path,
    passwords: &[String],
) -> Result<String> {
    let (paths, pw) = list_with_password_tries(archive, passwords)?;
    if paths.is_empty() {
        bail!("Archive is empty");
    }
    extract_archive(archive, dest, &pw)?;
    Ok(pw)
}

/// True if listing looks like a CSF / Steam-library pack (Orb-style).
pub fn is_steam_library_layout(paths: &[String]) -> bool {
    paths.iter().any(|p| {
        let lower = p.to_lowercase();
        lower.contains("steamapps/appmanifest_") && lower.ends_with(".acf")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_list_paths_extracts_names() {
        let sample = "\
2026-05-01 21:44:51 D....            0            0  steamapps
2026-05-01 21:44:51 ....A          814               steamapps\\appmanifest_4262310.acf
2026-05-01 21:44:50 ....A       667648     24250768  steamapps\\common\\Game\\Game.exe
";
        let paths = parse_7z_list_paths(sample);
        assert!(paths.iter().any(|p| p.contains("appmanifest_4262310.acf")));
        assert!(paths.iter().any(|p| p.contains("Game.exe")));
        assert!(is_steam_library_layout(&paths));
    }

    #[test]
    fn rejects_flat_layout() {
        let paths = vec!["Game.exe".into(), "data/foo.bin".into()];
        assert!(!is_steam_library_layout(&paths));
    }
}
