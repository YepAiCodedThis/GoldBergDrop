use crate::models::{Achievement, DlcApp};
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Names of the Steam API DLL that Goldberg can replace, along with their
/// "original backup" and "GUI backup" filenames.
const DLL_NAMES: [&str; 2] = ["steam_api", "steam_api64"];

/// How deep to recurse when the DLL isn't found directly next to the
/// executable. Deep enough for nested "Binaries/Win64"-style layouts
/// without scanning unrelated, arbitrarily deep trees.
const MAX_SEARCH_DEPTH: usize = 8;

pub struct SetupOptions {
    pub app_id: u32,
    pub dlc_list: Vec<DlcApp>,
    pub achievements: Vec<Achievement>,
}

/// Applies the full Goldberg setup: writes `steam_appid.txt` and the
/// `steam_settings` folder (including `DLC.txt` / `achievements.json` when
/// supplied) in `game_dir`, then swaps in the Goldberg build of
/// `steam_api(64).dll`.
///
/// If no `steam_api(64).dll` sits directly in `game_dir`, the whole
/// directory tree beneath it is searched (subfolders included, e.g.
/// `Binaries/Win64`) for the real DLL location. The same config files are
/// then also written beside each matching DLL, since Goldberg looks for
/// `steam_settings` next to the DLL it was loaded from.
///
/// Returns `true` if at least one DLL was swapped, `false` if only the
/// top-level config files were written (no `steam_api(64).dll` found
/// anywhere under `game_dir`).
pub fn apply_setup(
    game_dir: &Path,
    goldberg_cache_dir: &Path,
    options: &SetupOptions,
) -> Result<bool> {
    write_config(game_dir, options)?;

    let dll_dirs = find_dll_directories(game_dir);

    let mut swapped_any = false;
    for dir in &dll_dirs {
        if dir != game_dir {
            write_config(dir, options)?;
        }
        for name in DLL_NAMES {
            if dir.join(format!("{name}.dll")).exists() {
                swap_dll(dir, goldberg_cache_dir, name)?;
                swapped_any = true;
            }
        }
    }

    Ok(swapped_any)
}

fn write_config(dir: &Path, options: &SetupOptions) -> Result<()> {
    fs::write(dir.join("steam_appid.txt"), options.app_id.to_string())
        .context("Failed to write steam_appid.txt")?;

    let steam_settings_dir = dir.join("steam_settings");
    fs::create_dir_all(&steam_settings_dir).context("Failed to create steam_settings folder")?;

    write_dlc_file(&steam_settings_dir, &options.dlc_list)?;
    write_achievements_file(&steam_settings_dir, &options.achievements)
}

/// Finds every directory at or beneath `game_dir` that directly contains a
/// `steam_api.dll` or `steam_api64.dll`, searching subfolders when the DLL
/// isn't right next to the executable.
fn find_dll_directories(game_dir: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    for entry in WalkDir::new(game_dir)
        .max_depth(MAX_SEARCH_DEPTH)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let is_dll = entry
            .file_name()
            .to_str()
            .map(|name| DLL_NAMES.iter().any(|dll| name.eq_ignore_ascii_case(&format!("{dll}.dll"))))
            .unwrap_or(false);
        if !is_dll {
            continue;
        }
        if let Some(parent) = entry.path().parent() {
            let parent = parent.to_path_buf();
            if !dirs.contains(&parent) {
                dirs.push(parent);
            }
        }
    }

    dirs
}

fn write_dlc_file(steam_settings_dir: &Path, dlc_list: &[DlcApp]) -> Result<()> {
    let dlc_txt = steam_settings_dir.join("DLC.txt");
    if dlc_list.is_empty() {
        let _ = fs::remove_file(&dlc_txt);
        return Ok(());
    }

    let content = dlc_list
        .iter()
        .map(|dlc| format!("{}={}", dlc.app_id, dlc.name))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&dlc_txt, content + "\n").context("Failed to write steam_settings/DLC.txt")?;
    Ok(())
}

fn write_achievements_file(
    steam_settings_dir: &Path,
    achievements: &[Achievement],
) -> Result<()> {
    let path = steam_settings_dir.join("achievements.json");
    if achievements.is_empty() {
        let _ = fs::remove_file(&path);
        return Ok(());
    }

    let json = serde_json::to_string_pretty(achievements)
        .context("Failed to serialize achievements.json")?;
    fs::write(&path, json).context("Failed to write steam_settings/achievements.json")?;
    Ok(())
}

/// Backs up the original `{name}.dll` (once) and copies the Goldberg build
/// of the same DLL into the game folder, mirroring GoldbergGUI's behavior:
/// - First run: rename the original to `{name}_o.dll`.
/// - Subsequent runs: back up the current (Goldberg) DLL to a hidden
///   `.{name}.dll.GOLDBERGDROPBACKUP` file instead of overwriting the
///   original backup.
fn swap_dll(game_dir: &Path, goldberg_cache_dir: &Path, name: &str) -> Result<()> {
    let target_dll = game_dir.join(format!("{name}.dll"));
    let original_backup = game_dir.join(format!("{name}_o.dll"));
    let gui_backup = game_dir.join(format!(".{name}.dll.GOLDBERGDROPBACKUP"));
    let goldberg_dll = goldberg_cache_dir.join(format!("{name}.dll"));

    if !goldberg_dll.exists() {
        anyhow::bail!("Goldberg build is missing {name}.dll in its cache folder");
    }

    if !original_backup.exists() {
        fs::rename(&target_dll, &original_backup)
            .with_context(|| format!("Failed to back up original {name}.dll"))?;
    } else {
        fs::rename(&target_dll, &gui_backup)
            .with_context(|| format!("Failed to back up current {name}.dll"))?;
        set_hidden(&gui_backup);
    }

    fs::copy(&goldberg_dll, &target_dll)
        .with_context(|| format!("Failed to copy Goldberg {name}.dll into game folder"))?;

    Ok(())
}

/// Marks a file as hidden on Windows via the `attrib` command. Best-effort:
/// failures are ignored since this is a cosmetic detail of the backup file.
#[cfg(windows)]
fn set_hidden(path: &Path) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let _ = std::process::Command::new("attrib")
        .arg("+h")
        .arg(path)
        .creation_flags(CREATE_NO_WINDOW)
        .status();
}

#[cfg(not(windows))]
fn set_hidden(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("goldberg_drop_test_{label}_{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn finds_dll_directly_in_game_dir() {
        let game_dir = temp_dir("direct");
        fs::write(game_dir.join("steam_api64.dll"), b"dummy").unwrap();

        let dirs = find_dll_directories(&game_dir);

        assert_eq!(dirs, vec![game_dir.clone()]);
        fs::remove_dir_all(&game_dir).unwrap();
    }

    #[test]
    fn finds_dll_in_nested_subfolder() {
        let game_dir = temp_dir("nested");
        let nested = game_dir.join("Binaries").join("Win64");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("steam_api64.dll"), b"dummy").unwrap();

        let dirs = find_dll_directories(&game_dir);

        assert_eq!(dirs, vec![nested.clone()]);
        fs::remove_dir_all(&game_dir).unwrap();
    }

    #[test]
    fn returns_empty_when_no_dll_anywhere() {
        let game_dir = temp_dir("none");
        fs::write(game_dir.join("Game.exe"), b"dummy").unwrap();

        let dirs = find_dll_directories(&game_dir);

        assert!(dirs.is_empty());
        fs::remove_dir_all(&game_dir).unwrap();
    }

    #[test]
    fn apply_setup_swaps_nested_dll_and_writes_config_beside_it() {
        let game_dir = temp_dir("apply_nested");
        let nested = game_dir.join("Binaries").join("Win64");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("steam_api64.dll"), b"original").unwrap();

        let cache_dir = temp_dir("apply_nested_cache");
        fs::write(cache_dir.join("steam_api64.dll"), b"goldberg-build").unwrap();

        let options = SetupOptions {
            app_id: 4169770,
            dlc_list: vec![],
            achievements: vec![],
        };

        let swapped = apply_setup(&game_dir, &cache_dir, &options).unwrap();

        assert!(swapped);
        // Top-level config is always written.
        assert_eq!(
            fs::read_to_string(game_dir.join("steam_appid.txt")).unwrap(),
            "4169770"
        );
        // Config is also written beside the nested DLL.
        assert_eq!(
            fs::read_to_string(nested.join("steam_appid.txt")).unwrap(),
            "4169770"
        );
        // The original DLL was backed up and replaced with the Goldberg build.
        assert_eq!(
            fs::read_to_string(nested.join("steam_api64_o.dll")).unwrap(),
            "original"
        );
        assert_eq!(
            fs::read_to_string(nested.join("steam_api64.dll")).unwrap(),
            "goldberg-build"
        );

        fs::remove_dir_all(&game_dir).unwrap();
        fs::remove_dir_all(&cache_dir).unwrap();
    }

    #[test]
    fn apply_setup_is_config_only_when_no_dll_found() {
        let game_dir = temp_dir("apply_none");
        let cache_dir = temp_dir("apply_none_cache");

        let options = SetupOptions {
            app_id: 4169770,
            dlc_list: vec![],
            achievements: vec![],
        };

        let swapped = apply_setup(&game_dir, &cache_dir, &options).unwrap();

        assert!(!swapped);
        assert!(game_dir.join("steam_appid.txt").exists());
        assert!(game_dir.join("steam_settings").is_dir());

        fs::remove_dir_all(&game_dir).unwrap();
        fs::remove_dir_all(&cache_dir).unwrap();
    }
}
