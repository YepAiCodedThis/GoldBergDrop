<p align="center">
  <img src="assets/app_icon_128.png" width="96" height="96" alt="GoldbergDrop icon">
</p>

<h1 align="center">GoldbergDrop</h1>

<p align="center">
  Drop a game <code>.exe</code> → find the Steam App ID → set up<br>
  the <a href="https://mr_goldberg.gitlab.io/goldberg_emulator/">Goldberg Steamworks emulator</a>. No manual config editing.
</p>

<p align="center">
  <img src="assets/screenshot-main.png" width="280" alt="GoldbergDrop setup — drop or browse an exe">
  &nbsp;
  <img src="assets/screenshot-select.png" width="280" alt="GoldbergDrop match picker">
</p>
<p align="center">
  <img src="assets/screenshot-workshop.png" width="280" alt="WorkshopDL tab">
  &nbsp;
  <img src="assets/screenshot-settings.png" width="280" alt="Settings — Download providers">
</p>

---

## What it does

1. **Drop** a game `.exe` (or Browse… / Explorer **Send to → GoldbergDrop**).
2. **Look up** the Steam App ID (exe name → folder → path). Pick from a list if needed, or type the ID.
3. **Apply Goldberg** in the game folder:
   - `steam_appid.txt`
   - `steam_settings/` (optional DLC list)
   - replace `steam_api.dll` / `steam_api64.dll` (searches subfolders too)

Games you set up also appear in the **tray launcher** for one-click start.

## Why

Goldberg is an offline Steamworks reimplementation. Useful when you want to:

- play without Steam running
- keep a game working after store/server shutdown
- LAN-test while modding

GoldbergDrop only automates setup — it never ships game files.

## Features

| Area | What you get |
| --- | --- |
| **Setup** | Drag & drop, Browse, Send to, CLI path · App ID lookup · DLC fetch · recursive DLL swap |
| **WorkshopDL** | Queue downloads via [GGNetwork](https://ggntw.com/steam) · SteamCMD fallback · bulk paste / list import |
| **Tray** | Launch tracked games · open / quit · close-to-tray · optional Windows autostart |
| **Settings** | Providers, SteamCMD, paths, Setup defaults, tray list |

## Download

- Get `goldberg-drop.exe` from **[Releases](../../releases)**
- Single portable file — no installer, no API key
- Needs internet for lookups, optional DLC/Workshop, and the one-time Goldberg download (then cached)

> SmartScreen may warn (unsigned) → **More info → Run anyway**

---

## Usage

### Goldberg setup

| Screen | Action |
| --- | --- |
| Idle | Drop a `.exe` or **Browse…** |
| Multiple matches | Pick a game or edit the App ID → **Apply** |
| No match | Enter the App ID manually |
| Done | **Set up another game** if you want |

Enable **"Send to" entry** once on the Setup tab to add GoldbergDrop to Explorer’s Send to menu.

### Tray & launching games

After a **successful Goldberg setup**, the game is tracked automatically (path + icon).

| Action | Result |
| --- | --- |
| **Right-click** tray icon → game name | Starts that game’s `.exe` |
| **Right-click** → Open GoldbergDrop | Shows the main window |
| **Right-click** → Quit | Exits the app |
| **Left-click** tray icon | Only restores the window (does not launch a game) |

- Remove games under **Settings → Tray**
- **Close to tray**: ✕ hides to the notification area instead of quitting
- **Autostart**: starts hidden with `--tray` at Windows logon

### WorkshopDL

1. Paste Workshop URLs or IDs (or use **List** for many).
2. **Add to Queue** / **Direct Download**.
3. Run **Download Queue**.

GGNetwork is tried first; SteamCMD is the fallback.  
Files go to `WorkshopDownloads/{Game}/` next to the exe (change under Settings → Paths).

### Settings

Open **⚙** on the ribbon:

**Download** · **SteamCMD** · **Paths** · **Setup** · **Tray**

Config and cache live in:

`%LOCALAPPDATA%\GoldbergDrop\GoldbergDrop\`

---

## Building from source

Requires a recent stable [Rust](https://www.rust-lang.org/) toolchain (MSVC) on Windows.

```bash
cargo build --release
```

Output: `target/release/goldberg-drop.exe`

## Built with

[egui](https://github.com/emilk/egui) / [eframe](https://github.com/emilk/egui) · [tray-icon](https://github.com/tauri-apps/tray-icon) · [Goldberg Emulator](https://mr_goldberg.gitlab.io/goldberg_emulator/) · see `Cargo.toml`

## Disclaimer

Setup helper for Goldberg only — use with games you own. Goldberg is a third-party project; see its site for licensing.
