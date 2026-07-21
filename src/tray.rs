//! System tray icon with a native context menu (muda via tray-icon).
//!
//! Menu actions must not wait for egui's frame loop: when the main window is
//! hidden (`Visible(false)`), Windows stops delivering redraws so `App::logic`
//! stalls. Game launches run on the menu thread; Open/Quit wake the HWND.

use anyhow::{Context, Result};
use std::path::Path;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};

use crate::games::TrackedGame;

const ID_OPEN: &str = "open";
const ID_QUIT: &str = "quit";
const ID_EMPTY: &str = "empty";
const ID_STEAM_GL: &str = "steam_greenluma";
const ID_STEAM_PLAIN: &str = "steam_plain";

pub enum TrayCmd {
    ShowWindow,
    Quit,
    /// Kept for completeness; games are launched on the menu thread already.
    #[allow(dead_code)]
    LaunchGame(u32),
}

/// Shared wake targets so tray threads can revive a hidden eframe window.
pub struct TrayWake {
    pub ctx: Mutex<Option<egui::Context>>,
    /// Native Win32 HWND (0 = unknown).
    pub hwnd: AtomicIsize,
}

impl TrayWake {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            ctx: Mutex::new(None),
            hwnd: AtomicIsize::new(0),
        })
    }

    pub fn set_context(&self, ctx: &egui::Context) {
        if let Ok(mut slot) = self.ctx.lock() {
            *slot = Some(ctx.clone());
        }
    }

    pub fn set_hwnd_from_window(&self, window: &winit::window::Window) {
        #[cfg(windows)]
        {
            use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
            if let Ok(handle) = window.window_handle() {
                if let RawWindowHandle::Win32(win32) = handle.as_raw() {
                    self.hwnd
                        .store(win32.hwnd.get() as isize, Ordering::SeqCst);
                }
            }
        }
        #[cfg(not(windows))]
        {
            let _ = window;
        }
    }

    /// Force the hidden main window visible (Win32) and ask egui to process cmds.
    pub fn show_window_now(&self) {
        #[cfg(windows)]
        {
            let hwnd = self.hwnd.load(Ordering::SeqCst);
            if hwnd != 0 {
                win32::show_and_focus(hwnd);
            }
        }
        if let Ok(slot) = self.ctx.lock() {
            if let Some(ctx) = slot.as_ref() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                ctx.request_repaint();
            }
        }
    }

    fn close_window_now(&self) {
        #[cfg(windows)]
        {
            let hwnd = self.hwnd.load(Ordering::SeqCst);
            if hwnd != 0 {
                win32::post_close(hwnd);
            }
        }
        if let Ok(slot) = self.ctx.lock() {
            if let Some(ctx) = slot.as_ref() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                ctx.request_repaint();
            }
        }
    }
}

pub struct TrayHandle {
    _tray: tray_icon::TrayIcon,
    menu: tray_icon::menu::Menu,
    pub rx: Receiver<TrayCmd>,
    pub wake: Arc<TrayWake>,
}

impl TrayHandle {
    pub fn create(games: &[TrackedGame]) -> Result<Self> {
        enable_windows_dark_menus();

        let (cmd_tx, rx) = mpsc::channel();
        let wake = TrayWake::new();
        let icon = app_tray_icon().context("Failed to build tray icon image")?;
        let menu = build_menu(games, crate::greenluma::is_installed())?;

        let tray = tray_icon::TrayIconBuilder::new()
            .with_tooltip("GoldbergDrop")
            .with_icon(icon)
            .with_menu(Box::new(menu.clone()))
            .with_menu_on_left_click(false)
            .build()
            .context("Failed to create tray icon")?;

        let tx_click = cmd_tx.clone();
        let wake_click = wake.clone();
        std::thread::spawn(move || {
            while let Ok(event) = tray_icon::TrayIconEvent::receiver().recv() {
                match event {
                    tray_icon::TrayIconEvent::Click {
                        button: tray_icon::MouseButton::Left,
                        button_state: tray_icon::MouseButtonState::Up,
                        ..
                    } => {
                        let _ = tx_click.send(TrayCmd::ShowWindow);
                        wake_click.show_window_now();
                    }
                    _ => {}
                }
            }
        });

        // Menu clicks must not depend on the egui frame loop — when the main
        // window is hidden, `App::logic` may not run until something wakes it.
        let wake_menu = wake.clone();
        std::thread::spawn(move || {
            while let Ok(event) = tray_icon::menu::MenuEvent::receiver().recv() {
                let id = event.id.as_ref();
                if id == ID_OPEN {
                    log::info!("tray menu: Open");
                    let _ = cmd_tx.send(TrayCmd::ShowWindow);
                    wake_menu.show_window_now();
                } else if id == ID_QUIT {
                    log::info!("tray menu: Quit");
                    let _ = cmd_tx.send(TrayCmd::Quit);
                    wake_menu.close_window_now();
                } else if id == ID_STEAM_PLAIN {
                    log::info!("tray menu: Start Steam without GreenLuma");
                    std::thread::spawn(|| {
                        if let Err(e) = crate::greenluma::start_steam_plain() {
                            log::error!("tray start Steam plain failed: {e:#}");
                        }
                    });
                } else if id == ID_STEAM_GL {
                    log::info!("tray menu: Start Steam with GreenLuma");
                    std::thread::spawn(|| {
                        if let Err(e) = crate::greenluma::restart_steam_injected() {
                            log::error!("tray start Steam GreenLuma failed: {e:#}");
                        }
                    });
                } else if let Some(app_id_str) = id.strip_prefix("game_") {
                    if let Ok(app_id) = app_id_str.parse::<u32>() {
                        // Launch immediately — do not wait for egui.
                        log::info!("tray menu: LaunchGame {app_id}");
                        let games = crate::games::load();
                        if let Some(g) = games.iter().find(|g| g.app_id == app_id) {
                            if let Err(e) = launch_game(&g.exe_path) {
                                log::error!("tray launch failed: {e:#}");
                            }
                        } else {
                            log::warn!("tray launch: app_id {app_id} not in tracked games");
                        }
                    }
                }
            }
        });

        Ok(Self {
            _tray: tray,
            menu,
            rx,
            wake,
        })
    }

    pub fn rebuild_menu(&mut self, games: &[TrackedGame]) -> Result<()> {
        populate_menu(&self.menu, games, crate::greenluma::is_installed())
    }

    pub fn set_tooltip(&self, text: &str) {
        let _ = self._tray.set_tooltip(Some(text));
    }
}

fn build_menu(games: &[TrackedGame], greenluma_installed: bool) -> Result<tray_icon::menu::Menu> {
    let menu = tray_icon::menu::Menu::new();
    populate_menu(&menu, games, greenluma_installed)?;
    Ok(menu)
}

fn populate_menu(
    menu: &tray_icon::menu::Menu,
    games: &[TrackedGame],
    greenluma_installed: bool,
) -> Result<()> {
    use tray_icon::menu::{IconMenuItem, MenuItem, PredefinedMenuItem};

    while !menu.items().is_empty() {
        menu.remove_at(0);
    }

    let open = MenuItem::with_id(ID_OPEN, "Open GoldbergDrop", true, None);
    menu.append(&open)?;

    if greenluma_installed {
        menu.append(&PredefinedMenuItem::separator())?;
        let with_gl = MenuItem::with_id(
            ID_STEAM_GL,
            "Start Steam with GreenLuma",
            true,
            None,
        );
        menu.append(&with_gl)?;
        let plain = MenuItem::with_id(
            ID_STEAM_PLAIN,
            "Start Steam without GreenLuma",
            true,
            None,
        );
        menu.append(&plain)?;
    }

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
mod win32 {
    use std::ffi::c_void;

    type HWND = *mut c_void;
    const SW_SHOW: i32 = 5;
    const SW_RESTORE: i32 = 9;
    const WM_CLOSE: u32 = 0x0010;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn ShowWindow(hwnd: HWND, n_cmd_show: i32) -> i32;
        fn SetForegroundWindow(hwnd: HWND) -> i32;
        fn PostMessageW(hwnd: HWND, msg: u32, w: usize, l: isize) -> i32;
        fn IsIconic(hwnd: HWND) -> i32;
    }

    pub fn show_and_focus(hwnd: isize) {
        let hwnd = hwnd as HWND;
        if hwnd.is_null() {
            return;
        }
        unsafe {
            if IsIconic(hwnd) != 0 {
                ShowWindow(hwnd, SW_RESTORE);
            } else {
                ShowWindow(hwnd, SW_SHOW);
            }
            SetForegroundWindow(hwnd);
        }
    }

    pub fn post_close(hwnd: isize) {
        let hwnd = hwnd as HWND;
        if hwnd.is_null() {
            return;
        }
        unsafe {
            PostMessageW(hwnd, WM_CLOSE, 0, 0);
        }
    }
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
