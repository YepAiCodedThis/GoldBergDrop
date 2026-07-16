use anyhow::{anyhow, bail, Context, Result};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const STEAMCMD_ZIP_URL: &str =
    "https://steamcdn-a.akamaihd.net/client/installer/steamcmd.zip";

/// One workshop item to pull in a batched SteamCMD session.
pub struct WorkshopJob {
    pub app_id: u32,
    pub workshop_id: u64,
    pub dest_dir: PathBuf,
    pub mod_name: String,
}

/// Ensure `steamcmd/steamcmd.exe` exists under AppData (download + unzip if needed).
pub fn ensure_steamcmd() -> Result<PathBuf> {
    let dir = crate::settings::AppSettings::steamcmd_dir()?;
    let exe = dir.join("steamcmd.exe");
    if exe.is_file() {
        return Ok(exe);
    }

    fs::create_dir_all(&dir).with_context(|| format!("Failed to create {}", dir.display()))?;

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .context("Failed to build HTTP client for SteamCMD")?;

    let bytes = client
        .get(STEAMCMD_ZIP_URL)
        .send()
        .context("SteamCMD download request failed")?
        .error_for_status()
        .context("SteamCMD download HTTP error")?
        .bytes()
        .context("Failed to read SteamCMD zip")?;

    let zip_path = dir.join("steamcmd.zip");
    fs::write(&zip_path, &bytes)
        .with_context(|| format!("Failed to write {}", zip_path.display()))?;

    extract_zip(&zip_path, &dir)?;
    let _ = fs::remove_file(&zip_path);

    if !exe.is_file() {
        bail!("steamcmd.exe missing after extract in {}", dir.display());
    }

    // First run bootstraps SteamCMD itself (updates packages) — quiet, no window.
    let _ = run_steamcmd(&exe, &dir, &["+quit"], false);

    Ok(exe)
}

/// Delete the SteamCMD install folder under AppData.
pub fn remove_steamcmd() -> Result<()> {
    let dir = crate::settings::AppSettings::steamcmd_dir()?;
    if dir.is_dir() {
        fs::remove_dir_all(&dir)
            .with_context(|| format!("Failed to remove {}", dir.display()))?;
    }
    Ok(())
}

fn extract_zip(zip_path: &Path, dest: &Path) -> Result<()> {
    let file = fs::File::open(zip_path)
        .with_context(|| format!("Failed to open {}", zip_path.display()))?;
    let mut archive = zip::ZipArchive::new(file).context("Invalid SteamCMD zip")?;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .with_context(|| format!("SteamCMD zip entry {i}"))?;
        let out_path = match entry.enclosed_name() {
            Some(p) => dest.join(p),
            None => continue,
        };
        if entry.is_dir() {
            fs::create_dir_all(&out_path)?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut out = fs::File::create(&out_path)
            .with_context(|| format!("Failed to create {}", out_path.display()))?;
        std::io::copy(&mut entry, &mut out)?;
    }
    Ok(())
}

fn run_steamcmd(exe: &Path, dir: &Path, args: &[&str], visible: bool) -> Result<std::process::ExitStatus> {
    if visible {
        // eframe uses windows_subsystem=windows. Direct CreateProcess + CREATE_NEW_CONSOLE
        // still attaches invalid std handles (STARTF_USESTDHANDLES), so SteamCMD's
        // console stays blank/invisible. `start /WAIT` opens a real console window.
        let mut cmd = Command::new("cmd.exe");
        cmd.current_dir(dir)
            .arg("/C")
            .arg("start")
            .arg("/WAIT")
            .arg("SteamCMD") // window title (required first token for `start`)
            .arg(exe);
        for a in args {
            cmd.arg(a);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // Hide the short-lived cmd.exe wrapper; only the SteamCMD window shows.
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        cmd.status().context("Failed to start SteamCMD")
    } else {
        let mut cmd = Command::new(exe);
        cmd.current_dir(dir).args(args);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        cmd.status().context("Failed to start SteamCMD")
    }
}

fn resolve_app_id(app_id: u32, workshop_id: u64) -> u32 {
    if app_id != 0 {
        return app_id;
    }
    if let Ok(map) = crate::workshop::fetch_published_file_details(&[workshop_id]) {
        if let Some(meta) = map.get(&workshop_id) {
            if meta.app_id != 0 {
                return meta.app_id;
            }
        }
    }
    crate::workshop::fetch_workshop_page_info(workshop_id)
        .ok()
        .and_then(|(_, _, id)| id)
        .unwrap_or(0)
}

/// Download one workshop item (single SteamCMD session).
pub fn download_workshop_item(
    app_id: u32,
    workshop_id: u64,
    dest_dir: &Path,
    mod_name: &str,
) -> Result<PathBuf> {
    let results = download_workshop_items(&[WorkshopJob {
        app_id,
        workshop_id,
        dest_dir: dest_dir.to_path_buf(),
        mod_name: mod_name.to_string(),
    }])?;
    results
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("SteamCMD returned no result"))?
}

/// Download many workshop items in **one** SteamCMD login/session.
///
/// SteamCMD has no true parallel workshop API in a single process; chaining
/// `workshop_download_item` in one script is the supported batch approach
/// (avoids re-login + bootstrap per ID). Parallel SteamCMD processes fighting
/// over the same `steamapps` folder are unreliable.
pub fn download_workshop_items(jobs: &[WorkshopJob]) -> Result<Vec<Result<PathBuf>>> {
    if jobs.is_empty() {
        return Ok(Vec::new());
    }

    let steamcmd = ensure_steamcmd()?;
    let steamcmd_dir = steamcmd
        .parent()
        .ok_or_else(|| anyhow!("SteamCMD has no parent dir"))?
        .to_path_buf();

    let mut resolved: Vec<(u32, u64, &Path, &str)> = Vec::with_capacity(jobs.len());
    for job in jobs {
        let app_id = resolve_app_id(job.app_id, job.workshop_id);
        if app_id == 0 {
            // Still include a placeholder so result indices match jobs.
            resolved.push((0, job.workshop_id, &job.dest_dir, &job.mod_name));
        } else {
            resolved.push((app_id, job.workshop_id, &job.dest_dir, &job.mod_name));
        }
    }

    let runnable: Vec<_> = resolved
        .iter()
        .filter(|(app_id, _, _, _)| *app_id != 0)
        .collect();

    if !runnable.is_empty() {
        let script_path = steamcmd_dir.join(format!(
            "gd_workshop_batch_{}.txt",
            std::process::id()
        ));
        {
            let mut script = fs::File::create(&script_path)
                .with_context(|| format!("Failed to create {}", script_path.display()))?;
            // Continue after a single item failure so the rest of the queue still runs.
            writeln!(script, "@ShutdownOnFailedCommand 0")?;
            writeln!(script, "@NoPromptForPassword 1")?;
            writeln!(script, "login anonymous")?;
            for (app_id, workshop_id, _, _) in &runnable {
                writeln!(script, "workshop_download_item {app_id} {workshop_id}")?;
            }
            writeln!(script, "quit")?;
        }

        let script_name = script_path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow!("Invalid script path"))?
            .to_string();

        let status = run_steamcmd(
            &steamcmd,
            &steamcmd_dir,
            &["+runscript", &script_name],
            true,
        )?;
        let _ = fs::remove_file(&script_path);

        if !status.success() {
            // Individual items may still have landed; check folders below.
            eprintln!(
                "SteamCMD batch exited with status {}",
                status.code().unwrap_or(-1)
            );
        }
    }

    let mut out = Vec::with_capacity(jobs.len());
    for (app_id, workshop_id, dest_dir, mod_name) in resolved {
        if app_id == 0 {
            out.push(Err(anyhow!(
                "Missing App ID for SteamCMD download (workshop {workshop_id})"
            )));
            continue;
        }
        out.push(copy_downloaded_item(
            &steamcmd_dir,
            app_id,
            workshop_id,
            dest_dir,
            mod_name,
        ));
    }
    Ok(out)
}

fn copy_downloaded_item(
    steamcmd_dir: &Path,
    app_id: u32,
    workshop_id: u64,
    dest_dir: &Path,
    mod_name: &str,
) -> Result<PathBuf> {
    let content = steamcmd_dir
        .join("steamapps")
        .join("workshop")
        .join("content")
        .join(app_id.to_string())
        .join(workshop_id.to_string());

    if !dir_has_files(&content) {
        // Incomplete downloads sometimes sit under downloads/ instead of content/.
        let partial = steamcmd_dir
            .join("steamapps")
            .join("workshop")
            .join("downloads")
            .join(app_id.to_string())
            .join(workshop_id.to_string());
        if dir_has_files(&partial) {
            bail!(
                "SteamCMD left an incomplete download (not finalized): {}",
                partial.display()
            );
        }
        bail!(
            "SteamCMD finished but content folder missing or empty: {}",
            content.display()
        );
    }

    fs::create_dir_all(dest_dir)
        .with_context(|| format!("Failed to create {}", dest_dir.display()))?;

    let folder_name = crate::workshop::sanitize_folder_name(mod_name);
    let target = dest_dir.join(if folder_name.is_empty() {
        workshop_id.to_string()
    } else {
        folder_name
    });

    if target.exists() {
        fs::remove_dir_all(&target)
            .with_context(|| format!("Failed to clear {}", target.display()))?;
    }
    copy_dir_recursive(&content, &target)?;

    if !dir_has_files(&target) {
        bail!(
            "Copied SteamCMD output but destination is empty: {}",
            target.display()
        );
    }

    Ok(target)
}

/// True if `path` is a directory containing at least one non-empty file.
fn dir_has_files(path: &Path) -> bool {
    fn walk(p: &Path) -> bool {
        let Ok(entries) = fs::read_dir(p) else {
            return false;
        };
        for entry in entries.flatten() {
            let child = entry.path();
            if child.is_dir() {
                if walk(&child) {
                    return true;
                }
            } else if entry.metadata().map(|m| m.len() > 0).unwrap_or(false) {
                return true;
            }
        }
        false
    }
    path.is_dir() && walk(path)
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src).with_context(|| format!("Failed to read {}", src.display()))? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&entry.path(), &to)?;
        } else {
            fs::copy(entry.path(), &to)
                .with_context(|| format!("Failed to copy to {}", to.display()))?;
        }
    }
    Ok(())
}
