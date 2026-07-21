<p align="center">
  <img src="assets/app_icon_128.png" width="96" height="96" alt="GoldbergDrop icon">
</p>

<h1 align="center">GoldbergDrop</h1>

<table align="center" width="100%">
  <tr>
    <td align="center">
      <br />
      There’s a novel under this line. I won’t cry if you bounce.<br /><br />
      <a href="https://yepaicodedthis.github.io/GoldBergDrop/"><strong>Website</strong></a>
      = short version.
      <a href="../../releases/latest"><strong>Exe</strong></a>
      = skip the talking.<br /><br />
      I built this so the boring setup just runs.
      Hand-editing config files got annoying.<br /><br />
      And yes, this was built with AI — thanks
      <a href="https://cursor.com"><strong>Cursor</strong></a> team.
      I can’t code, and you made this possible.
      <br /><br />
    </td>
  </tr>
</table>

---

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

## Contents

- [What it does](#what-it-does)
- [Why](#why)
- [Features](#features)
- [Download](#download)
- [Usage](#usage)
  - [Goldberg setup](#goldberg-setup)
  - [GreenLuma](#greenluma)
  - [Tray](#tray--launching-games)
  - [WorkshopDL](#workshopdl)
  - [Settings](#settings)
- [Build](#building-from-source)

---

## What it does

1. **Drop** a game `.exe`  
   (or Browse… / Explorer **Send to → GoldbergDrop**)
2. **Look up** the Steam App ID  
   (exe name → folder → path). Pick from a list, or type the ID.
3. **Apply Goldberg** in the game folder:
   - `steam_appid.txt`
   - `steam_settings/` (optional DLC + achievements)
   - replace `steam_api.dll` / `steam_api64.dll` (subfolders too)

Set-up games also land in the **tray launcher**.

---

## Why

Goldberg = offline Steamworks reimplementation.

| You want… | It helps |
| --- | --- |
| Play without Steam running | ✓ |
| Keep a game alive after store/server death | ✓ |
| LAN-test while modding | ✓ |

GoldbergDrop only automates setup. **No game files in the download.**

---

## Features

| Area | What you get |
| --- | --- |
| **Setup** | Drag & drop · Browse · Send to · CLI path · App ID lookup · DLC / achievements · recursive DLL swap |
| **GreenLuma** | Install Steam006 (PW zip + SHA256) · CSF import · AppList · inject / plain Steam · download watch |
| **WorkshopDL** | Queue via [GGNetwork](https://ggntw.com/steam) · SteamCMD fallback · bulk paste / list import |
| **Tray** | Launch tracked games · Steam ± GreenLuma · close-to-tray · autostart · auto-inject |
| **Settings** | Providers · SteamCMD · Steam paths · Setup defaults · tray / auto-inject |

---

## Download

| | |
| --- | --- |
| **Website** | [yepaicodedthis.github.io/GoldBergDrop](https://yepaicodedthis.github.io/GoldBergDrop/) |
| **File** | `goldberg-drop.exe` from **[Releases](../../releases/latest)** |
| **Shape** | Portable · no installer · no API key |
| **Network** | Lookups · optional DLC/Workshop · one-time Goldberg fetch (then cached) |
| **7-Zip** | **Required** for GreenLuma install + CSF packs ([download](https://www.7-zip.org/)) |

> **SmartScreen** may warn (unsigned) → **More info → Run anyway**

---

## Usage

### Goldberg setup

| Screen | Action |
| --- | --- |
| Idle | Drop a `.exe` or **Browse…** |
| Multiple matches | Pick a game or edit App ID → **Apply** |
| No match | Enter App ID manually |
| Done | **Set up another game** if you want |

Enable **"Send to" entry** once on the Setup tab for Explorer’s Send to menu.

---

### GreenLuma

Alternative path: Steam + [GreenLuma](https://cs.rin.ru/forum/viewtopic.php?f=10&t=103709) (Steam006) via DLLInjector.

**Needs [7-Zip](https://www.7-zip.org/).** No `7z.exe` → install / CSF import fail.

#### Install & packs

1. **GreenLuma** ribbon tab → **Install GreenLuma**  
   Exclude AppData `greenluma` in Defender first (injector false-positives).
2. Bundled archive is password-protected + **SHA256-whitelisted** — modified binaries are rejected.
3. Drop a CSF / Orb pack to merge into Steam + AppList:
   - `steamapps/appmanifest_*.acf`
   - `steamapps/common/<installdir>/`
   - `depotcache/`
4. Optional archive password field (remembers working ones; tries `cs.rin.ru`).
5. Or drop a game `.exe` → AppID → AppList.
6. **Start Steam with GreenLuma** → runs `DLLInjector.exe`.

#### Extra hooks

- **"Send to" (GreenLuma)** → `GoldbergDrop (GreenLuma).lnk` (`--greenluma`)
- **Settings → Tray → Start Steam with GreenLuma when GoldbergDrop starts**  
  Swaps Steam’s Run-key for GoldbergDrop (`--tray`), injects on each launch.  
  Uncheck to restore the old Steam Run entry.
- Steam path: auto (registry → common paths), override under **Settings → Paths**

#### Play vs maintain

GreenLuma is great for AppList play. Updates/downloads often break while injected.

| Mode | How | For |
| --- | --- | --- |
| **Play** | Tray → *Start Steam with GreenLuma* (or auto-inject) | Playing AppList games |
| **Maintain** | Tray → *Start Steam without GreenLuma* | Updates · Workshop · Verify · Store |

If a download starts after a GreenLuma launch, GoldbergDrop asks to restart **plain Steam**.  
The prompt waits until you **quit the game** — no mid-session nag.

---

### Tray & launching games

After a successful Goldberg setup, the game is tracked (path + icon).

| Action | Result |
| --- | --- |
| Right-click → game name | Starts that `.exe` |
| Right-click → Steam with / without GreenLuma | Restarts Steam in that mode |
| Right-click → Open GoldbergDrop | Shows the window |
| Right-click → Quit | Exits |
| Left-click tray | Restores window only (no launch) |

Extras under **Settings → Tray**:

- Remove tracked games
- **Close to tray** — ✕ hides instead of quit
- **Autostart** — hidden with `--tray` at logon
- **Single instance** — second launch / Send to forwards to the running app

---

### WorkshopDL

1. Paste Workshop URLs or IDs (or **List** for many)
2. **Add to Queue** / **Direct Download**
3. **Download Queue**

GGNetwork first, SteamCMD as fallback.  
Default output: `WorkshopDownloads/{Game}/` next to the exe  
(change under **Settings → Paths**).

---

### Settings

Ribbon **⚙**:

| Tab | |
| --- | --- |
| Download | Providers |
| SteamCMD | SteamCMD setup |
| Paths | Steam / output folders |
| Setup | Setup defaults |
| Tray | Launcher / autostart / auto-inject |

Data directory:

```text
%APPDATA%\GoldbergDrop\GoldbergDrop\data\
```

---

## Building from source

Recent stable [Rust](https://www.rust-lang.org/) + MSVC on Windows:

```bash
cargo build --release
```

→ `target/release/goldberg-drop.exe`

---

## Built with

[egui](https://github.com/emilk/egui) / [eframe](https://github.com/emilk/egui)
· [tray-icon](https://github.com/tauri-apps/tray-icon)
· [Goldberg Emulator](https://mr_goldberg.gitlab.io/goldberg_emulator/)
· see `Cargo.toml`

---

## Disclaimer

Setup helper for Goldberg / GreenLuma — use with games you own.  
Both are third-party projects. GreenLuma binaries ship unmodified in a password-protected archive for integrity checks only.  
See their communities for licensing and terms.
