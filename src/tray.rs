//! System tray icon with a native context menu (muda via tray-icon).

use anyhow::{Context, Result};
use std::path::Path;
use std::sync::mpsc::{self, Receiver};

use crate::games::TrackedGame;

const ID_OPEN: &str = "open";
const ID_QUIT: &str = "quit";
const ID_EMPTY: &str = "empty";

pub enum TrayCmd {
    ShowWindow,
    Quit,
    LaunchGame(u32),
}

pub struct TrayHandle {
    _tray: tray_icon::TrayIcon,
    menu: tray_icon::menu::Menu,
    pub rx: Receiver<TrayCmd>,
}

impl TrayHandle {
    pub fn create(games: &[TrackedGame]) -> Result<Self> {
        enable_windows_dark_menus();

        let (cmd_tx, rx) = mpsc::channel();
        let icon = app_tray_icon().context("Failed to build tray icon image")?;
        let menu = build_menu(games)?;

        let tray = tray_icon::TrayIconBuilder::new()
            .with_tooltip("GoldbergDrop")
            .with_icon(icon)
            .with_menu(Box::new(menu.clone()))
            .with_menu_on_left_click(false)
            .build()
            .context("Failed to create tray icon")?;

        let tx_click = cmd_tx.clone();
        std::thread::spawn(move || {
            while let Ok(event) = tray_icon::TrayIconEvent::receiver().recv() {
                match event {
                    tray_icon::TrayIconEvent::Click {
                        button: tray_icon::MouseButton::Left,
                        button_state: tray_icon::MouseButtonState::Up,
                        ..
                    } => {
                        let _ = tx_click.send(TrayCmd::ShowWindow);
                    }
                    _ => {}
                }
            }
        });

        // Menu clicks must not depend on the egui frame loop — when the main
        // window is hidden, `App::ui` is not called and menu actions stall.
        std::thread::spawn(move || {
            while let Ok(event) = tray_icon::menu::MenuEvent::receiver().recv() {
                let id = event.id.as_ref();
                if id == ID_OPEN {
                    let _ = cmd_tx.send(TrayCmd::ShowWindow);
                } else if id == ID_QUIT {
                    let _ = cmd_tx.send(TrayCmd::Quit);
                } else if let Some(app_id_str) = id.strip_prefix("game_") {
                    if let Ok(app_id) = app_id_str.parse::<u32>() {
                        let _ = cmd_tx.send(TrayCmd::LaunchGame(app_id));
                    }
                }
            }
        });

        Ok(Self {
            _tray: tray,
            menu,
            rx,
        })
    }

    pub fn rebuild_menu(&mut self, games: &[TrackedGame]) -> Result<()> {
        populate_menu(&self.menu, games)
    }

    pub fn set_tooltip(&self, text: &str) {
        let _ = self._tray.set_tooltip(Some(text));
    }
}

fn build_menu(games: &[TrackedGame]) -> Result<tray_icon::menu::Menu> {
    let menu = tray_icon::menu::Menu::new();
    populate_menu(&menu, games)?;
    Ok(menu)
}

fn populate_menu(menu: &tray_icon::menu::Menu, games: &[TrackedGame]) -> Result<()> {
    use tray_icon::menu::{IconMenuItem, MenuItem, PredefinedMenuItem};

    while !menu.items().is_empty() {
        menu.remove_at(0);
    }

    let open = MenuItem::with_id(ID_OPEN, "Open GoldbergDrop", true, None);
    menu.append(&open)?;

    menu.append(&PredefinedMenuItem::separator())?;

    if games.is_empty() {
        let empty = MenuItem::with_id(ID_EMPTY, "No games tracked yet", false, None);
        menu.append(&empty)?;
    } else {
        for g in games {
            let id = format!("game_{}", g.app_id);
            let icon = g
                .icon_path
                .as_ref()
                .and_then(|p| load_menu_icon(p));
            let item = IconMenuItem::with_id(id, &g.name, true, icon, None);
            menu.append(&item)?;
        }
    }

    menu.append(&PredefinedMenuItem::separator())?;

    let quit = MenuItem::with_id(ID_QUIT, "Quit", true, None);
    menu.append(&quit)?;

    Ok(())
}

fn load_menu_icon(path: &Path) -> Option<tray_icon::menu::Icon> {
    let image = image::open(path).ok()?.into_rgba8();
    let (w, h) = image.dimensions();
    tray_icon::menu::Icon::from_rgba(image.into_raw(), w, h).ok()
}

fn app_tray_icon() -> Result<tray_icon::Icon> {
    let image = image::load_from_memory(crate::APP_ICON_PNG)
        .context("decode app icon")?
        .into_rgba8();
    let (w, h) = image.dimensions();
    tray_icon::Icon::from_rgba(image.into_raw(), w, h).context("tray Icon::from_rgba")
}

/// Launch a tracked game exe (detached).
pub fn launch_game(exe: &Path) -> Result<()> {
    let dir = exe.parent().unwrap_or_else(|| Path::new("."));
    std::process::Command::new(exe)
        .current_dir(dir)
        .spawn()
        .with_context(|| format!("Failed to start {}", exe.display()))?;
    Ok(())
}

/// Enable dark native menus on Windows (undocumented uxtheme APIs; Win10 1809+).
pub fn enable_windows_dark_menus() {
    #[cfg(windows)]
    dark_mode::enable();
    #[cfg(not(windows))]
    {}
}

#[cfg(windows)]
mod dark_mode {
    use std::ffi::c_void;
    use std::sync::OnceLock;

    type HMODULE = isize;

    const UXTHEME_SHOULDAPPSUSEDARKMODE_ORDINAL: u16 = 132;
    const UXTHEME_REFRESHIMMERSIVECOLORPOLICYSTATE_ORDINAL: u16 = 104;
    const UXTHEME_ALLOWDARKMODEFORAPP_ORDINAL: u16 = 135;

    #[repr(C)]
    enum PreferredAppMode {
        Default,
        AllowDark,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryA(name: *const u8) -> HMODULE;
        fn GetProcAddress(module: HMODULE, name: *const u8) -> *mut c_void;
    }

    fn uxtheme() -> HMODULE {
        static H: OnceLock<HMODULE> = OnceLock::new();
        *H.get_or_init(|| unsafe { LoadLibraryA(b"uxtheme.dll\0".as_ptr()) })
    }

    fn win10_build() -> u32 {
        static BUILD: OnceLock<u32> = OnceLock::new();
        *BUILD.get_or_init(|| {
            #[link(name = "ntdll")]
            unsafe extern "system" {
                fn RtlGetNtVersionNumbers(major: *mut u32, minor: *mut u32, build: *mut u32);
            }
            let mut major = 0u32;
            let mut minor = 0u32;
            let mut build = 0u32;
            unsafe { RtlGetNtVersionNumbers(&mut major, &mut minor, &mut build) };
            build & !0xF000_0000
        })
    }

    fn should_use_dark_mode() -> bool {
        if let Some(light) = read_apps_use_light_theme() {
            return !light;
        }
        type ShouldAppsUseDarkMode = unsafe extern "system" fn() -> bool;
        unsafe {
            let h = uxtheme();
            if h == 0 {
                return false;
            }
            let proc = GetProcAddress(
                h,
                UXTHEME_SHOULDAPPSUSEDARKMODE_ORDINAL as usize as *const u8,
            );
            if proc.is_null() {
                return false;
            }
            let f: ShouldAppsUseDarkMode = std::mem::transmute(proc);
            f()
        }
    }

    fn read_apps_use_light_theme() -> Option<bool> {
        use winreg::enums::HKEY_CURRENT_USER;
        use winreg::RegKey;
        let hk = RegKey::predef(HKEY_CURRENT_USER);
        let sub = hk
            .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
            .ok()?;
        sub.get_value::<u32, _>("AppsUseLightTheme")
            .ok()
            .map(|v| v != 0)
    }

    fn refresh_immersive_color_policy_state() {
        type Refresh = unsafe extern "system" fn();
        unsafe {
            let h = uxtheme();
            if h == 0 {
                return;
            }
            let proc = GetProcAddress(
                h,
                UXTHEME_REFRESHIMMERSIVECOLORPOLICYSTATE_ORDINAL as usize as *const u8,
            );
            if proc.is_null() {
                return;
            }
            let f: Refresh = std::mem::transmute(proc);
            f();
        }
    }

    fn allow_dark_mode_for_app(is_dark: bool) {
        let build = win10_build();
        if build < 17763 {
            return;
        }
        unsafe {
            let h = uxtheme();
            if h == 0 {
                return;
            }
            let ord135 = GetProcAddress(h, UXTHEME_ALLOWDARKMODEFORAPP_ORDINAL as usize as *const u8);
            if ord135.is_null() {
                return;
            }
            if build < 18362 {
                type AllowDarkModeForApp = unsafe extern "system" fn(bool) -> bool;
                let f: AllowDarkModeForApp = std::mem::transmute(ord135);
                f(is_dark);
            } else {
                type SetPreferredAppMode =
                    unsafe extern "system" fn(PreferredAppMode) -> PreferredAppMode;
                let f: SetPreferredAppMode = std::mem::transmute(ord135);
                let mode = if is_dark {
                    PreferredAppMode::AllowDark
                } else {
                    PreferredAppMode::Default
                };
                f(mode);
            }
            refresh_immersive_color_policy_state();
        }
    }

    pub fn enable() {
        allow_dark_mode_for_app(should_use_dark_mode());
    }
}
