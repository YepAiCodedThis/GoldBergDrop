use crate::goldberg::{self, SetupOptions};
use crate::models::{DlcApp, GgItemResponse, QueueItem, QueueStatus, SteamApp};
use crate::sendto::{self, SendToStatus};
use crate::settings::{AppSettings, SettingsTab};
use crate::{autostart, emulator, games, steam, steamcmd, tray, workshop};
use eframe::egui::{self, Color32, CornerRadius, RichText, Stroke};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

/// Colors for the custom dark/gold theme, tuned to match the app icon.
mod colors {
    use super::Color32;

    pub const PANEL_BG: Color32 = Color32::from_rgb(20, 20, 23);
    pub const PANEL_BORDER: Color32 = Color32::from_rgb(50, 49, 54);
    pub const SURFACE: Color32 = Color32::from_rgb(29, 29, 33);
    pub const SURFACE_HOVER: Color32 = Color32::from_rgb(38, 37, 42);
    pub const ACCENT: Color32 = Color32::from_rgb(227, 168, 58);
    pub const ACCENT_HOVER: Color32 = Color32::from_rgb(240, 184, 80);
    pub const ACCENT_ACTIVE: Color32 = Color32::from_rgb(200, 146, 42);
    pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(237, 236, 233);
    pub const TEXT_MUTED: Color32 = Color32::from_rgb(146, 144, 150);
    pub const SUCCESS: Color32 = Color32::from_rgb(96, 209, 132);
    pub const ERROR: Color32 = Color32::from_rgb(232, 96, 96);
    /// Slightly recessed field background — reads as an inset, not a raised pill.
    pub const INPUT_BG: Color32 = Color32::from_rgb(16, 16, 19);
    pub const INPUT_BORDER: Color32 = Color32::from_rgb(44, 43, 48);
    pub const INPUT_BORDER_FOCUS: Color32 = Color32::from_rgb(227, 168, 58);
}

const TITLE_BAR_HEIGHT: f32 = 36.0;
const RIBBON_HEIGHT: f32 = 32.0;
const CONTENT_MARGIN: f32 = 18.0;
const WINDOW_CORNER_RADIUS: u8 = 20;
/// Corner radius for card-level containers (drop zone, list group).
const CARD_RADIUS: u8 = 14;
/// Corner radius for buttons, inputs, and list rows.
const CONTROL_RADIUS: u8 = 6;
const CONTROL_HEIGHT: f32 = 34.0;
const INPUT_HEIGHT: f32 = 34.0;
const BUTTON_PAD_X: f32 = 16.0;
const QUEUE_ROW_HEIGHT: f32 = 26.0;
const QUEUE_LOADING: &str = "…";
const NO_DOWNLOAD_METHOD: &str =
    "None of the download methods worked (GGNetwork + SteamCMD).";
const ICON_OPEN: &str = "↗";

/// Fixed column widths for the workshop download queue table.
mod queue_cols {
    pub const DOT: f32 = 26.0;
    pub const MOD: f32 = 148.0;
    pub const GAME: f32 = 130.0;
    pub const ID: f32 = 80.0;
    pub const ACTION: f32 = 30.0;
    pub const TOTAL: f32 = DOT + MOD + GAME + ID + ACTION;
}

/// Fixed columns for the failed-download overlay table (same layout style as queue).
mod fail_cols {
    pub const MOD: f32 = 148.0;
    pub const GAME: f32 = 140.0;
    pub const ACTION: f32 = 30.0;
    pub const TOTAL: f32 = MOD + GAME + ACTION;
}

/// Top-level ribbon tab.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AppTab {
    Setup,
    WorkshopDl,
    Settings,
}

/// Which screen is currently shown below the drop zone.
#[derive(Clone, PartialEq)]
enum Screen {
    Idle,
    Working,
    ChooseMatch,
    ManualId,
    Done,
    Error,
}

/// Messages sent from background worker threads back to the UI thread.
enum WorkerMsg {
    SearchFound(SteamApp),
    SearchAmbiguous(Vec<SteamApp>),
    SearchNotFound,
    SearchFailed(String),
    ApplyDone {
        app_id: u32,
        name: String,
        dll_swapped: bool,
        dlc_count: usize,
        achievement_count: usize,
    },
    ApplyFailed(String),
    WorkshopDownloadDone {
        index: usize,
        path: PathBuf,
        mod_name: String,
        game_name: String,
    },
    WorkshopFailed {
        index: usize,
        error: String,
    },
    /// One SteamCMD session finished for many queued items.
    WorkshopSteamCmdBatch {
        results: Vec<(usize, Result<(PathBuf, String, String), String>)>,
    },
    WorkshopQueueInfo {
        workshop_id: u64,
        mod_name: String,
        game_name: String,
        game_app_id: u32,
        gg_item: Option<GgItemResponse>,
        gg_available: bool,
    },
    /// SteamCMD zip download + extract + first-run bootstrap finished.
    SteamCmdEnsureDone {
        error: Option<String>,
    },
}

pub struct GoldbergDropApp {
    exe_path: Option<PathBuf>,
    fetch_dlc: bool,
    fetch_achievements: bool,

    sendto_enabled: bool,
    sendto_stale: bool,
    sendto_notice: Option<(String, bool)>,

    screen: Screen,
    working_message: String,
    candidates: Vec<SteamApp>,
    manual_id_input: String,
    manual_id_error: Option<String>,
    /// Set when the current `manual_id_input` was filled in by clicking a
    /// candidate in the match list, so Apply can skip an extra name lookup.
    /// Cleared as soon as the field is edited to something else.
    selected_match_name: Option<(u32, String)>,
    result_message: String,

    /// The app icon, lazily uploaded to the GPU on the first frame (a
    /// `egui::Context` is only available once painting starts).
    icon_texture: Option<egui::TextureHandle>,

    active_tab: AppTab,
    settings: AppSettings,
    settings_tab: SettingsTab,
    /// Next queue runs use SteamCMD only (set by Failed-overlay Retry).
    force_steamcmd: bool,
    workshop_url_input: String,
    workshop_url_error: Option<String>,
    download_queue: Vec<QueueItem>,
    queue_processing: bool,
    /// Modal listing failed downloads with GGNetwork page links.
    failed_overlay_open: bool,
    /// After the user closes the overlay, don't reopen until a new failure.
    failed_overlay_suppress: bool,
    /// Modal for pasting a multi-line workshop URL/ID list.
    import_list_overlay_open: bool,
    import_list_input: String,
    import_list_error: Option<String>,
    /// After Import & Download: wait until all names resolve, then start queue.
    download_when_resolved: bool,
    /// Retry-with-SteamCMD waiting for ensure_steamcmd to finish.
    pending_steamcmd_retry: bool,
    /// Status line while SteamCMD is downloading/deploying (Failed overlay / Settings).
    steamcmd_setup_status: Option<String>,
    /// SteamCMD retry already ran — overlay shows exhausted message, no Retry button.
    steamcmd_exhausted: bool,
    /// System tray (games launcher + close-to-tray).
    tray: Option<tray::TrayHandle>,
    /// Show "running in tray" banner until this instant, then hide the window.
    tray_banner_until: Option<Instant>,
    /// Main window is hidden; only the tray icon is active.
    dormant_in_tray: bool,
    /// Serial name-lookup worker (Steam rate-limits parallel scrapes).
    resolve_tx: Sender<u64>,

    tx: Sender<WorkerMsg>,
    rx: Receiver<WorkerMsg>,
}

impl Default for GoldbergDropApp {
    fn default() -> Self {
        Self::new(None, false)
    }
}

impl GoldbergDropApp {
    pub fn new(initial_path: Option<PathBuf>, start_in_tray: bool) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let (resolve_tx, resolve_rx) = std::sync::mpsc::channel::<u64>();
        let resolve_ui_tx = tx.clone();
        std::thread::spawn(move || {
            // 1) GGNetwork per id
            // 2) GetPublishedFileDetails in batches (no key, no HTML 429)
            // 3) HTML scrape only for leftovers (1/s)
            let mut game_name_cache: std::collections::HashMap<u32, String> =
                std::collections::HashMap::new();
            let mut pending: std::collections::VecDeque<u64> =
                std::collections::VecDeque::new();
            type PendingMeta = (
                u64,
                String,
                String,
                u32,
                Option<GgItemResponse>,
                bool,
            );
            let mut api_q: std::collections::VecDeque<PendingMeta> =
                std::collections::VecDeque::new();
            let mut html_q: std::collections::VecDeque<PendingMeta> =
                std::collections::VecDeque::new();

            fn is_weak(workshop_id: u64, mod_name: &str, game_name: &str) -> bool {
                mod_name == format!("Item {workshop_id}")
                    || game_name == "Unknown game"
                    || game_name.starts_with("App ")
            }

            fn polish(
                workshop_id: u64,
                mut mod_name: String,
                mut game_name: String,
                game_app_id: u32,
                gg_item: Option<GgItemResponse>,
                gg_available: bool,
                game_name_cache: &mut std::collections::HashMap<u32, String>,
            ) -> (String, String, u32, Option<GgItemResponse>, bool) {
                if game_app_id != 0
                    && (game_name == "Unknown game" || game_name.starts_with("App "))
                {
                    if let Some(cached) = game_name_cache.get(&game_app_id) {
                        game_name = cached.clone();
                    } else if let Ok(Some(name)) = steam::get_app_name(game_app_id) {
                        game_name_cache.insert(game_app_id, name.clone());
                        game_name = name;
                    }
                }
                if mod_name == format!("Item {workshop_id}") {
                    if let Some(item) = &gg_item {
                        if !item.name.trim().is_empty() {
                            mod_name = item.name.clone();
                        }
                    }
                }
                (mod_name, game_name, game_app_id, gg_item, gg_available)
            }

            loop {
                while let Ok(id) = resolve_rx.try_recv() {
                    pending.push_back(id);
                }
                if pending.is_empty() && api_q.is_empty() && html_q.is_empty() {
                    match resolve_rx.recv() {
                        Ok(id) => pending.push_back(id),
                        Err(_) => break,
                    }
                    continue;
                }

                // Prefer new GG work over API/HTML.
                if let Some(workshop_id) = pending.pop_front() {
                    let raw = workshop::resolve_gg(workshop_id);
                    let (mod_name, game_name, game_app_id, gg_item, gg_available) = polish(
                        workshop_id,
                        raw.0,
                        raw.1,
                        raw.2,
                        raw.3,
                        raw.4,
                        &mut game_name_cache,
                    );
                    if is_weak(workshop_id, &mod_name, &game_name) {
                        api_q.push_back((
                            workshop_id,
                            mod_name,
                            game_name,
                            game_app_id,
                            gg_item,
                            gg_available,
                        ));
                    } else {
                        let _ = resolve_ui_tx.send(WorkerMsg::WorkshopQueueInfo {
                            workshop_id,
                            mod_name,
                            game_name,
                            game_app_id,
                            gg_item,
                            gg_available,
                        });
                    }
                    continue;
                }

                if !api_q.is_empty() {
                    let take = api_q.len().min(workshop::PUBLISHED_FILE_BATCH);
                    let mut batch = Vec::with_capacity(take);
                    for _ in 0..take {
                        if let Some(entry) = api_q.pop_front() {
                            batch.push(entry);
                        }
                    }
                    let ids: Vec<u64> = batch.iter().map(|e| e.0).collect();
                    let details = workshop::fetch_published_file_details(&ids).unwrap_or_default();

                    for (workshop_id, mod_name, game_name, game_app_id, gg_item, gg_available) in
                        batch
                    {
                        let (mod_name, game_name, game_app_id) =
                            workshop::enrich_from_published_meta(
                                workshop_id,
                                mod_name,
                                game_name,
                                game_app_id,
                                details.get(&workshop_id),
                            );
                        let (mod_name, game_name, game_app_id, gg_item, gg_available) = polish(
                            workshop_id,
                            mod_name,
                            game_name,
                            game_app_id,
                            gg_item,
                            gg_available,
                            &mut game_name_cache,
                        );
                        if is_weak(workshop_id, &mod_name, &game_name) {
                            html_q.push_back((
                                workshop_id,
                                mod_name,
                                game_name,
                                game_app_id,
                                gg_item,
                                gg_available,
                            ));
                        } else {
                            let _ = resolve_ui_tx.send(WorkerMsg::WorkshopQueueInfo {
                                workshop_id,
                                mod_name,
                                game_name,
                                game_app_id,
                                gg_item,
                                gg_available,
                            });
                        }
                    }
                    continue;
                }

                if let Some((
                    workshop_id,
                    mod_name,
                    game_name,
                    game_app_id,
                    gg_item,
                    gg_available,
                )) = html_q.pop_front()
                {
                    let (mod_name, game_name, game_app_id) =
                        workshop::enrich_from_steam(workshop_id, mod_name, game_name, game_app_id);
                    let (mod_name, game_name, game_app_id, gg_item, gg_available) = polish(
                        workshop_id,
                        mod_name,
                        game_name,
                        game_app_id,
                        gg_item,
                        gg_available,
                        &mut game_name_cache,
                    );
                    let _ = resolve_ui_tx.send(WorkerMsg::WorkshopQueueInfo {
                        workshop_id,
                        mod_name,
                        game_name,
                        game_app_id,
                        gg_item,
                        gg_available,
                    });
                }
            }
        });

        let sendto_status = sendto::status();
        let settings = AppSettings::load();
        let mut app = Self {
            exe_path: None,
            fetch_dlc: settings.fetch_dlc_default,
            fetch_achievements: settings.fetch_achievements_default,
            sendto_enabled: sendto_status != SendToStatus::Disabled,
            sendto_stale: sendto_status == SendToStatus::Stale,
            sendto_notice: None,
            screen: Screen::Idle,
            working_message: String::new(),
            candidates: Vec::new(),
            manual_id_input: String::new(),
            manual_id_error: None,
            selected_match_name: None,
            result_message: String::new(),
            icon_texture: None,
            active_tab: AppTab::Setup,
            settings,
            settings_tab: SettingsTab::Download,
            force_steamcmd: false,
            workshop_url_input: String::new(),
            workshop_url_error: None,
            download_queue: Vec::new(),
            queue_processing: false,
            failed_overlay_open: false,
            failed_overlay_suppress: false,
            import_list_overlay_open: false,
            import_list_input: String::new(),
            import_list_error: None,
            download_when_resolved: false,
            pending_steamcmd_retry: false,
            steamcmd_setup_status: None,
            steamcmd_exhausted: false,
            tray: None,
            tray_banner_until: None,
            // `--tray` at boot: stay icon-only unless a file was passed on the CLI.
            dormant_in_tray: start_in_tray && initial_path.is_none(),
            resolve_tx,
            tx,
            rx,
        };
        // Always create tray so tracked games are launchable from the icon.
        app.ensure_tray();
        if app.settings.autostart_tray {
            let _ = autostart::enable();
        }
        if start_in_tray {
            if let Some(t) = &app.tray {
                t.set_tooltip("GoldbergDrop — running in tray");
            }
        }
        // Launched via the "Send to" shortcut (or a file passed on the
        // command line) — start straight away instead of waiting for a drop.
        if let Some(path) = initial_path {
            app.accept_path(path);
        }
        app
    }

    fn ensure_tray(&mut self) {
        if self.tray.is_some() {
            return;
        }
        let games = games::load();
        match tray::TrayHandle::create(&games) {
            Ok(handle) => self.tray = Some(handle),
            Err(e) => eprintln!("Tray icon failed: {e:#}"),
        }
    }

    fn refresh_tray_menu(&mut self) {
        self.ensure_tray();
        let games = games::load();
        if let Some(t) = &mut self.tray {
            if let Err(e) = t.rebuild_menu(&games) {
                eprintln!("Tray menu refresh failed: {e:#}");
            }
        }
    }

    fn hide_to_tray_with_notice(&mut self) {
        self.ensure_tray();
        if let Some(t) = &self.tray {
            t.set_tooltip("GoldbergDrop — running in tray");
        }
        self.tray_banner_until = Some(Instant::now() + Duration::from_millis(1400));
    }

    fn show_from_tray(&mut self, ctx: &egui::Context) {
        self.tray_banner_until = None;
        self.dormant_in_tray = false;
        self.restore_main_viewport_size(ctx);
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        if let Some(t) = &self.tray {
            t.set_tooltip("GoldbergDrop");
        }
    }

    /// While dormant, keep the native window hidden. eframe calls
    /// `window.set_visible(true)` after the first frame paint, which would
    /// flash the UI on autostart (`--tray`) without this guard.
    fn enforce_tray_hidden(&self, ctx: &egui::Context, frame: &eframe::Frame) {
        if !self.dormant_in_tray {
            return;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(window) = frame.winit_window() {
            window.set_visible(false);
        }
    }

    fn restore_main_viewport_size(&self, ctx: &egui::Context) {
        let s = crate::WINDOW_SIZE;
        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(egui::vec2(s, s)));
        ctx.send_viewport_cmd(egui::ViewportCommand::MaxInnerSize(egui::vec2(s, s)));
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(s, s)));
    }

    fn poll_tray(&mut self, ctx: &egui::Context) {
        let cmds: Vec<tray::TrayCmd> = self
            .tray
            .as_ref()
            .map(|t| t.rx.try_iter().collect())
            .unwrap_or_default();
        for cmd in cmds {
            match cmd {
                tray::TrayCmd::ShowWindow => self.show_from_tray(ctx),
                tray::TrayCmd::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                tray::TrayCmd::LaunchGame(app_id) => {
                    let games = games::load();
                    if let Some(g) = games.iter().find(|g| g.app_id == app_id) {
                        if let Err(e) = tray::launch_game(&g.exe_path) {
                            eprintln!("Launch failed: {e:#}");
                        }
                    }
                }
            }
        }

        if self.tray.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn poll_tray_banner(&mut self, ctx: &egui::Context) {
        if let Some(until) = self.tray_banner_until {
            if Instant::now() >= until {
                self.tray_banner_until = None;
                self.dormant_in_tray = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            } else {
                ctx.request_repaint_after(Duration::from_millis(50));
            }
        }
    }

    /// Custom dark/gold visuals applied once at startup.
    pub fn build_visuals() -> egui::Visuals {
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = colors::PANEL_BG;
        visuals.window_fill = colors::PANEL_BG;
        visuals.extreme_bg_color = colors::INPUT_BG;
        visuals.faint_bg_color = colors::SURFACE;
        visuals.override_text_color = None;
        visuals.hyperlink_color = colors::ACCENT;
        visuals.selection.bg_fill = colors::ACCENT.linear_multiply(0.55);
        visuals.selection.stroke = Stroke::new(1.0, colors::ACCENT);
        visuals.window_corner_radius = CornerRadius::same(CARD_RADIUS);
        visuals.menu_corner_radius = CornerRadius::same(CONTROL_RADIUS);

        let w = &mut visuals.widgets;
        w.noninteractive.bg_fill = colors::PANEL_BG;
        w.noninteractive.weak_bg_fill = colors::PANEL_BG;
        w.noninteractive.bg_stroke = Stroke::new(1.0, colors::PANEL_BORDER);
        w.noninteractive.fg_stroke = Stroke::new(1.0, colors::TEXT_MUTED);
        w.noninteractive.corner_radius = CornerRadius::same(CONTROL_RADIUS);

        w.inactive.bg_fill = colors::SURFACE;
        w.inactive.weak_bg_fill = colors::SURFACE;
        w.inactive.bg_stroke = Stroke::new(1.0, colors::PANEL_BORDER);
        w.inactive.fg_stroke = Stroke::new(1.0, colors::TEXT_PRIMARY);
        w.inactive.corner_radius = CornerRadius::same(CONTROL_RADIUS);

        w.hovered.bg_fill = colors::SURFACE_HOVER;
        w.hovered.weak_bg_fill = colors::SURFACE_HOVER;
        w.hovered.bg_stroke = Stroke::new(1.0, colors::PANEL_BORDER);
        w.hovered.fg_stroke = Stroke::new(1.0, colors::TEXT_PRIMARY);
        w.hovered.corner_radius = CornerRadius::same(CONTROL_RADIUS);

        w.active.bg_fill = colors::SURFACE_HOVER;
        w.active.weak_bg_fill = colors::SURFACE_HOVER;
        w.active.bg_stroke = Stroke::new(1.0, colors::ACCENT);
        w.active.fg_stroke = Stroke::new(1.0, colors::TEXT_PRIMARY);
        w.active.corner_radius = CornerRadius::same(CONTROL_RADIUS);

        w.open.bg_fill = colors::SURFACE_HOVER;
        w.open.weak_bg_fill = colors::SURFACE_HOVER;
        w.open.bg_stroke = Stroke::new(1.0, colors::ACCENT);
        w.open.fg_stroke = Stroke::new(1.0, colors::TEXT_PRIMARY);

        visuals
    }

    /// Solid scrollbars with enough contrast to stay readable without hover.
    ///
    /// egui paints the track with `extreme_bg_color` and the handle with
    /// `widgets.inactive.bg_fill` (or fg_stroke when `foreground_color`).
    /// Our theme had both set to SURFACE, so the bar looked "hover-only".
    pub fn apply_style(ctx: &egui::Context) {
        ctx.set_theme(egui::ThemePreference::Dark);
        ctx.set_visuals(Self::build_visuals());
        ctx.all_styles_mut(|style| {
            let mut scroll = egui::style::ScrollStyle::solid();
            scroll.bar_width = 8.0;
            scroll.bar_inner_margin = 2.0;
            // Use fg_stroke for the handle so it contrasts with the track
            // (track = extreme_bg_color / INPUT_BG).
            scroll.foreground_color = true;
            style.spacing.scroll = scroll;
        });
    }

    fn game_dir(&self) -> Option<PathBuf> {
        self.exe_path
            .as_ref()
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf())
    }

    fn start_search(&mut self, path: PathBuf) {
        self.working_message = format!(
            "Searching for \"{}\"...",
            path.file_name().and_then(|n| n.to_str()).unwrap_or("?")
        );
        self.exe_path = Some(path.clone());
        self.screen = Screen::Working;

        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let outcome = steam::find_app_by_exe(&path);
            let msg = match outcome {
                Ok(steam::AppSearchOutcome::Found(app)) => WorkerMsg::SearchFound(app),
                Ok(steam::AppSearchOutcome::Ambiguous(list)) => WorkerMsg::SearchAmbiguous(list),
                Ok(steam::AppSearchOutcome::NotFound) => WorkerMsg::SearchNotFound,
                Err(e) => WorkerMsg::SearchFailed(e.to_string()),
            };
            let _ = tx.send(msg);
        });
    }

    fn start_apply(&mut self, app_id: u32, name: String) {
        let Some(game_dir) = self.game_dir() else {
            self.result_message = "No game folder selected.".to_string();
            self.screen = Screen::Error;
            return;
        };

        self.working_message = format!("Applying Goldberg setup for \"{name}\"...");
        self.screen = Screen::Working;
        let fetch_dlc = self.fetch_dlc;
        let fetch_achievements = self.fetch_achievements;
        let tx = self.tx.clone();

        std::thread::spawn(move || {
            let result = (|| -> anyhow::Result<(bool, usize, usize)> {
                let cache_dir = emulator::ensure_goldberg_available()?;

                let dlc_list: Vec<DlcApp> = if fetch_dlc {
                    steam::get_dlc_list(app_id).unwrap_or_default()
                } else {
                    Vec::new()
                };
                let dlc_count = dlc_list.len();

                let achievements = if fetch_achievements {
                    steam::get_achievements(app_id).unwrap_or_default()
                } else {
                    Vec::new()
                };
                let achievement_count = achievements.len();

                let swapped = goldberg::apply_setup(
                    &game_dir,
                    &cache_dir,
                    &SetupOptions {
                        app_id,
                        dlc_list,
                        achievements,
                    },
                )?;

                Ok((swapped, dlc_count, achievement_count))
            })();

            let msg = match result {
                Ok((dll_swapped, dlc_count, achievement_count)) => WorkerMsg::ApplyDone {
                    app_id,
                    name,
                    dll_swapped,
                    dlc_count,
                    achievement_count,
                },
                Err(e) => WorkerMsg::ApplyFailed(e.to_string()),
            };
            let _ = tx.send(msg);
        });
    }

    fn reset(&mut self) {
        self.exe_path = None;
        self.candidates.clear();
        self.manual_id_input.clear();
        self.manual_id_error = None;
        self.selected_match_name = None;
        self.screen = Screen::Idle;
    }

    fn poll_worker(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                WorkerMsg::SearchFound(app) => {
                    self.start_apply(app.app_id, app.name);
                }
                WorkerMsg::SearchAmbiguous(list) => {
                    self.candidates = list;
                    self.manual_id_input.clear();
                    self.manual_id_error = None;
                    self.selected_match_name = None;
                    self.screen = Screen::ChooseMatch;
                }
                WorkerMsg::SearchNotFound => {
                    self.manual_id_input.clear();
                    self.manual_id_error = None;
                    self.screen = Screen::ManualId;
                }
                WorkerMsg::SearchFailed(e) => {
                    self.result_message = format!("Search failed: {e}");
                    self.screen = Screen::Error;
                }
                WorkerMsg::ApplyDone {
                    app_id,
                    name,
                    dll_swapped,
                    dlc_count,
                    achievement_count,
                } => {
                    let dll_msg = if dll_swapped {
                        "steam_api(64).dll replaced with Goldberg build."
                    } else {
                        "No steam_api(64).dll found in the game folder or its subfolders — config written only."
                    };
                    let dlc_msg = if dlc_count > 0 {
                        format!(
                            " Fetched {dlc_count} DLC entr{}.",
                            if dlc_count == 1 { "y" } else { "ies" }
                        )
                    } else {
                        String::new()
                    };
                    let ach_msg = if achievement_count > 0 {
                        format!(" Wrote {achievement_count} achievements.")
                    } else {
                        String::new()
                    };
                    self.result_message = format!(
                        "Done! \"{name}\" (AppID {app_id}) is set up.\n{dll_msg}{dlc_msg}{ach_msg}"
                    );
                    self.screen = Screen::Done;
                    if let Some(exe) = self.exe_path.clone() {
                        match games::track(&exe, app_id, &name) {
                            Ok(_) => self.refresh_tray_menu(),
                            Err(e) => eprintln!("Failed to track game: {e:#}"),
                        }
                    }
                }
                WorkerMsg::ApplyFailed(e) => {
                    self.result_message = format!("Setup failed: {e}");
                    self.screen = Screen::Error;
                }
                WorkerMsg::WorkshopDownloadDone {
                    index,
                    path,
                    mod_name,
                    game_name,
                } => {
                    self.queue_processing = false;
                    if let Some(entry) = self.download_queue.get_mut(index) {
                        entry.status = QueueStatus::Done;
                        entry.mod_name = mod_name;
                        entry.game_name = game_name;
                        entry.download_path = Some(path);
                    }
                    self.try_process_queue();
                }
                WorkerMsg::WorkshopFailed { index, error } => {
                    self.queue_processing = false;
                    if let Some(entry) = self.download_queue.get_mut(index) {
                        entry.status = QueueStatus::Failed(error);
                    }
                    self.failed_overlay_suppress = false;
                    self.try_process_queue();
                }
                WorkerMsg::WorkshopSteamCmdBatch { results } => {
                    self.queue_processing = false;
                    self.force_steamcmd = false;
                    self.steamcmd_exhausted = true;
                    let mut any_fail = false;
                    for (index, result) in results {
                        if let Some(entry) = self.download_queue.get_mut(index) {
                            match result {
                                Ok((path, mod_name, game_name)) => {
                                    entry.status = QueueStatus::Done;
                                    entry.mod_name = mod_name;
                                    entry.game_name = game_name;
                                    entry.download_path = Some(path);
                                }
                                Err(_error) => {
                                    entry.status =
                                        QueueStatus::Failed(NO_DOWNLOAD_METHOD.to_string());
                                    any_fail = true;
                                }
                            }
                        }
                    }
                    if any_fail {
                        self.failed_overlay_suppress = false;
                        self.maybe_show_failed_overlay();
                    }
                }
                WorkerMsg::WorkshopQueueInfo {
                    workshop_id,
                    mod_name,
                    game_name,
                    game_app_id,
                    gg_item,
                    gg_available,
                } => {
                    self.apply_workshop_queue_info(
                        workshop_id,
                        mod_name,
                        game_name,
                        game_app_id,
                        gg_item,
                        gg_available,
                    );
                }
                WorkerMsg::SteamCmdEnsureDone { error } => {
                    self.on_steamcmd_ensure_done(error);
                }
            }
        }
    }

    fn apply_workshop_queue_info(
        &mut self,
        workshop_id: u64,
        mod_name: String,
        game_name: String,
        game_app_id: u32,
        gg_item: Option<GgItemResponse>,
        gg_available: bool,
    ) {
        if let Some(entry) = self
            .download_queue
            .iter_mut()
            .rev()
            .find(|e| e.workshop_id == workshop_id && e.mod_name == QUEUE_LOADING)
        {
            entry.mod_name = mod_name;
            entry.game_name = game_name;
            entry.game_app_id = game_app_id;
            entry.gg_item = gg_item;
            entry.gg_available = Some(gg_available);
        }

        if self.download_when_resolved && !self.queue_lookup_pending() {
            self.download_when_resolved = false;
            self.try_process_queue();
        }
    }

    fn queue_lookup_pending(&self) -> bool {
        self.download_queue
            .iter()
            .any(|e| e.mod_name == QUEUE_LOADING)
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if let Some(file) = dropped.first() {
            if let Some(path) = &file.path {
                self.accept_path(path.clone());
            }
        }
    }

    fn accept_path(&mut self, path: PathBuf) {
        let is_exe = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("exe"))
            .unwrap_or(false);

        if !is_exe {
            self.result_message = "Please select a valid .exe file.".to_string();
            self.screen = Screen::Error;
            return;
        }

        self.start_search(path);
    }

    fn browse_for_exe(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Game executable", &["exe"])
            .set_title("Select the game's .exe")
            .pick_file()
        {
            self.accept_path(path);
        }
    }

    /// Called whenever the "Enable right-click Send to" checkbox changes.
    fn on_sendto_checkbox_changed(&mut self) {
        if self.sendto_enabled {
            match sendto::enable() {
                Ok(()) => {
                    self.sendto_stale = false;
                    self.sendto_notice = Some((
                        "Right-click any game .exe → \"Send to\" → GoldbergDrop.".to_string(),
                        false,
                    ));
                }
                Err(e) => {
                    self.sendto_enabled = false;
                    self.sendto_notice = Some((format!("Couldn't enable Send to: {e}"), true));
                }
            }
        } else {
            match sendto::disable() {
                Ok(()) => {
                    self.sendto_stale = false;
                    self.sendto_notice = None;
                }
                Err(e) => {
                    self.sendto_notice = Some((format!("Couldn't disable Send to: {e}"), true));
                }
            }
        }
    }

    /// Uploads the embedded app icon as a texture the first time it's
    /// needed, so it can be drawn in the title bar.
    fn icon_texture(&mut self, ctx: &egui::Context) -> Option<&egui::TextureHandle> {
        if self.icon_texture.is_none() {
            if let Ok(image) = image::load_from_memory(crate::APP_ICON_PNG) {
                let image = image.into_rgba8();
                let (w, h) = image.dimensions();
                let color_image =
                    egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], image.as_raw());
                self.icon_texture = Some(ctx.load_texture(
                    "gd-app-icon",
                    color_image,
                    egui::TextureOptions::LINEAR,
                ));
            }
        }
        self.icon_texture.as_ref()
    }

    /// Re-points the "Send to" shortcut at this executable's current
    /// location (used when it went stale after the exe was moved).
    fn refresh_sendto(&mut self) {
        match sendto::enable() {
            Ok(()) => {
                self.sendto_stale = false;
                self.sendto_enabled = true;
                self.sendto_notice = Some(("\"Send to\" is fixed.".to_string(), false));
            }
            Err(e) => {
                self.sendto_notice = Some((format!("Couldn't fix \"Send to\": {e}"), true));
            }
        }
    }

    fn enqueue_workshop_url(&mut self, start_now: bool) {
        let input = self.workshop_url_input.trim();
        let workshop_id = match workshop::parse_workshop_id(input) {
            Some(id) => id,
            None => {
                self.workshop_url_error =
                    Some("Invalid Workshop URL or ID.".to_string());
                return;
            }
        };

        self.workshop_url_error = None;
        self.workshop_url_input.clear();
        self.enqueue_workshop_id(workshop_id, start_now);
    }

    fn enqueue_workshop_id(&mut self, workshop_id: u64, start_now: bool) {
        self.steamcmd_exhausted = false;
        self.download_queue.push(QueueItem {
            workshop_id,
            mod_name: QUEUE_LOADING.to_string(),
            game_app_id: 0,
            game_name: QUEUE_LOADING.to_string(),
            status: QueueStatus::Queued,
            gg_item: None,
            gg_available: None,
            download_path: None,
        });

        // Serial resolve worker — parallel scrapes hit Steam HTTP 429.
        let _ = self.resolve_tx.send(workshop_id);

        if start_now {
            self.download_when_resolved = true;
            if !self.queue_lookup_pending() {
                self.download_when_resolved = false;
                self.try_process_queue();
            }
        }
    }

    fn import_workshop_list(&mut self, start_now: bool) {
        let (ids, skipped) = workshop::parse_workshop_id_list(&self.import_list_input);
        if ids.is_empty() {
            self.import_list_error = Some(if skipped > 0 {
                "No valid Workshop URLs or IDs found.".to_string()
            } else {
                "Paste at least one URL or ID per line.".to_string()
            });
            return;
        }

        let imported = ids.len();
        self.import_list_error = None;
        self.import_list_input.clear();
        self.import_list_overlay_open = false;

        for id in ids {
            self.enqueue_workshop_id(id, false);
        }

        if start_now {
            // Wait until every name lookup finishes, then kick the queue.
            self.download_when_resolved = true;
            if !self.queue_lookup_pending() {
                self.download_when_resolved = false;
                self.try_process_queue();
            }
        }

        if skipped > 0 {
            self.workshop_url_error = Some(format!(
                "Imported {imported} item(s), skipped {skipped} invalid line(s)."
            ));
        } else {
            self.workshop_url_error = None;
        }
    }

    /// Ctrl+V / paste into the WorkshopDL window → queue (when not typing a single URL).
    fn handle_workshop_window_paste(&mut self, ui: &mut egui::Ui) {
        if self.import_list_overlay_open || self.failed_overlay_open {
            return;
        }

        let pastes: Vec<String> = ui.ctx().input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Paste(s) => Some(s.clone()),
                    _ => None,
                })
                .collect()
        });
        if pastes.is_empty() {
            return;
        }

        // Single-line paste into the URL field stays there for Add / Direct Download.
        let field_focused = ui.ctx().egui_wants_keyboard_input();

        for text in pastes {
            let multi = text.contains('\n') || text.contains('\r');
            let (ids, skipped) = workshop::parse_workshop_id_list(&text);
            if ids.is_empty() {
                if !field_focused {
                    self.workshop_url_error =
                        Some("Clipboard has no valid Workshop URLs or IDs.".to_string());
                }
                continue;
            }
            if field_focused && !multi && ids.len() == 1 {
                continue;
            }

            let imported = ids.len();
            for id in ids {
                self.enqueue_workshop_id(id, false);
            }
            // TextEdit also ate the paste — drop multiline junk from the single-line field.
            if field_focused {
                self.workshop_url_input.clear();
            }
            self.workshop_url_error = if skipped > 0 {
                Some(format!(
                    "Added {imported} item(s), skipped {skipped} invalid line(s)."
                ))
            } else {
                None
            };
        }
    }

    fn paste_import_list_from_clipboard(&mut self) {
        let text = match arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
            Ok(t) => t,
            Err(_) => {
                self.import_list_error = Some("Couldn't read clipboard.".to_string());
                return;
            }
        };
        match workshop::normalize_to_id_list(&text) {
            Some(ids) => {
                self.import_list_input = ids;
                self.import_list_error = None;
            }
            None => {
                self.import_list_error =
                    Some("Clipboard has no valid Workshop URLs or IDs.".to_string());
            }
        }
    }

    fn normalize_import_list_field(text: &mut String) {
        if let Some(ids) = workshop::normalize_to_id_list(text) {
            if ids != *text {
                *text = ids;
            }
        }
    }

    fn try_process_queue(&mut self) {
        if self.queue_processing {
            return;
        }

        // SteamCMD retry / steam-only mode: one login, many workshop_download_item.
        if self.force_steamcmd {
            self.try_process_steamcmd_batch();
            return;
        }

        let index = match self
            .download_queue
            .iter()
            .position(|item| item.status == QueueStatus::Queued)
        {
            Some(i) => i,
            None => {
                self.maybe_show_failed_overlay();
                return;
            }
        };

        self.queue_processing = true;
        if let Some(entry) = self.download_queue.get_mut(index) {
            entry.status = QueueStatus::Downloading;
        }

        let entry = &self.download_queue[index];
        let workshop_id = entry.workshop_id;
        let game_app_id = entry.game_app_id;
        let mut game_name = entry.game_name.clone();
        let mut mod_name = entry.mod_name.clone();
        if game_name == QUEUE_LOADING {
            game_name = format!("App {game_app_id}");
        }
        if mod_name == QUEUE_LOADING {
            mod_name = format!("Item {workshop_id}");
        }
        let settings = self.settings.clone();
        let force_steamcmd = self.force_steamcmd;
        let tx = self.tx.clone();

        std::thread::spawn(move || {
            let msg = match process_queue_item(
                workshop_id,
                index,
                game_app_id,
                mod_name,
                game_name,
                &settings,
                force_steamcmd,
            ) {
                Ok(QueueOutcome::Downloaded {
                    mod_name,
                    game_name,
                    path,
                }) => WorkerMsg::WorkshopDownloadDone {
                    index,
                    path,
                    mod_name,
                    game_name,
                },
                Err(e) => WorkerMsg::WorkshopFailed {
                    index,
                    error: e.to_string(),
                },
            };
            let _ = tx.send(msg);
        });
    }

    /// Run all currently Queued items through a single SteamCMD session.
    fn try_process_steamcmd_batch(&mut self) {
        let indices: Vec<usize> = self
            .download_queue
            .iter()
            .enumerate()
            .filter(|(_, item)| item.status == QueueStatus::Queued)
            .map(|(i, _)| i)
            .collect();

        if indices.is_empty() {
            self.force_steamcmd = false;
            self.maybe_show_failed_overlay();
            return;
        }

        self.queue_processing = true;
        let settings = self.settings.clone();
        let mut jobs = Vec::with_capacity(indices.len());
        let mut meta = Vec::with_capacity(indices.len());

        for &index in &indices {
            if let Some(entry) = self.download_queue.get_mut(index) {
                entry.status = QueueStatus::Downloading;
            }
            let entry = &self.download_queue[index];
            let workshop_id = entry.workshop_id;
            let game_app_id = entry.game_app_id;
            let mut game_name = entry.game_name.clone();
            let mut mod_name = entry.mod_name.clone();
            if game_name == QUEUE_LOADING {
                game_name = format!("App {game_app_id}");
            }
            if mod_name == QUEUE_LOADING {
                mod_name = format!("Item {workshop_id}");
            }
            let dest = match settings.game_download_dir(&game_name) {
                Ok(d) => d,
                Err(e) => {
                    // Mark this one failed immediately; still batch the rest.
                    if let Some(entry) = self.download_queue.get_mut(index) {
                        entry.status = QueueStatus::Failed(e.to_string());
                    }
                    continue;
                }
            };
            jobs.push(steamcmd::WorkshopJob {
                app_id: game_app_id,
                workshop_id,
                dest_dir: dest,
                mod_name: mod_name.clone(),
            });
            meta.push((index, mod_name, game_name));
        }

        if jobs.is_empty() {
            self.queue_processing = false;
            self.force_steamcmd = false;
            self.maybe_show_failed_overlay();
            return;
        }

        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let batch = match steamcmd::download_workshop_items(&jobs) {
                Ok(results) => results,
                Err(e) => {
                    let err = e.to_string();
                    let results = meta
                        .into_iter()
                        .map(|(index, _, _)| (index, Err(err.clone())))
                        .collect();
                    let _ = tx.send(WorkerMsg::WorkshopSteamCmdBatch { results });
                    return;
                }
            };

            let results = meta
                .into_iter()
                .zip(batch)
                .map(|((index, mod_name, game_name), result)| {
                    (
                        index,
                        result
                            .map(|path| (path, mod_name, game_name))
                            .map_err(|e| e.to_string()),
                    )
                })
                .collect();
            let _ = tx.send(WorkerMsg::WorkshopSteamCmdBatch { results });
        });
    }

    fn retry_failed_with_steamcmd(&mut self) {
        if !self.settings.use_steamcmd {
            return;
        }
        if self.steamcmd_setup_status.is_some() {
            return;
        }

        if !AppSettings::steamcmd_installed() {
            self.steamcmd_setup_status = Some(
                "SteamCMD is not installed yet — downloading and deploying…".to_string(),
            );
            self.pending_steamcmd_retry = true;
            self.start_steamcmd_ensure();
            return;
        }

        self.begin_steamcmd_retry();
    }

    fn start_steamcmd_ensure(&mut self) {
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let error = match steamcmd::ensure_steamcmd() {
                Ok(_) => None,
                Err(e) => Some(format!("{e:#}")),
            };
            let _ = tx.send(WorkerMsg::SteamCmdEnsureDone { error });
        });
    }

    fn on_steamcmd_ensure_done(&mut self, error: Option<String>) {
        let pending_retry = self.pending_steamcmd_retry;
        self.pending_steamcmd_retry = false;

        if let Some(err) = error {
            self.steamcmd_setup_status = Some(format!("SteamCMD setup failed: {err}"));
            return;
        }

        self.steamcmd_setup_status = None;
        if pending_retry {
            self.begin_steamcmd_retry();
        }
    }

    fn begin_steamcmd_retry(&mut self) {
        for item in &mut self.download_queue {
            if matches!(item.status, QueueStatus::Failed(_)) {
                item.status = QueueStatus::Queued;
            }
        }
        self.force_steamcmd = true;
        self.steamcmd_exhausted = false;
        self.failed_overlay_open = false;
        self.failed_overlay_suppress = true;
        self.steamcmd_setup_status = None;
        self.try_process_queue();
    }

    fn persist_settings(&mut self) {
        if let Err(e) = self.settings.save() {
            eprintln!("Failed to save settings: {e:#}");
        }
    }

    fn remove_queue_item(&mut self, index: usize) {
        if index < self.download_queue.len() {
            let removable = matches!(
                self.download_queue[index].status,
                QueueStatus::Queued | QueueStatus::Failed(_)
            );
            if removable {
                self.download_queue.remove(index);
            }
        }
    }

    fn clear_download_queue(&mut self) {
        self.download_queue
            .retain(|item| item.status == QueueStatus::Downloading);
        self.download_when_resolved = false;
        self.force_steamcmd = false;
        self.steamcmd_exhausted = false;
        self.failed_overlay_open = false;
    }
}

enum QueueOutcome {
    Downloaded {
        mod_name: String,
        game_name: String,
        path: PathBuf,
    },
}

/// Opens the download folder in Explorer via ShellExecute (not `explorer.exe`
/// argv — that parser breaks on spaces and opens Documents instead).
fn open_download_in_explorer(path: &std::path::Path) {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;

        let folder = if path.is_dir() {
            path.to_path_buf()
        } else {
            path.parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| path.to_path_buf())
        };

        let wide: Vec<u16> = folder
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let open: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();

        #[link(name = "shell32")]
        unsafe extern "system" {
            fn ShellExecuteW(
                hwnd: *mut core::ffi::c_void,
                lp_operation: *const u16,
                lp_file: *const u16,
                lp_parameters: *const u16,
                lp_directory: *const u16,
                n_show_cmd: i32,
            ) -> isize;
        }

        // >32 means success for ShellExecute.
        let _ = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                open.as_ptr(),
                wide.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1, // SW_SHOWNORMAL
            )
        };
    }
    #[cfg(not(windows))]
    {
        let dir = if path.is_dir() {
            path
        } else {
            path.parent().unwrap_or(path)
        };
        let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
    }
}

fn process_queue_item(
    workshop_id: u64,
    _index: usize,
    game_app_id: u32,
    mod_name: String,
    game_name: String,
    settings: &AppSettings,
    force_steamcmd: bool,
) -> anyhow::Result<QueueOutcome> {
    let try_steamcmd = |app_id: u32, mod_name: &str, game_name: &str| -> anyhow::Result<PathBuf> {
        let dest = settings.game_download_dir(game_name)?;
        steamcmd::download_workshop_item(app_id, workshop_id, &dest, mod_name)
    };

    let steamcmd_only = force_steamcmd || !settings.use_ggnetwork;

    if steamcmd_only {
        if !settings.use_steamcmd {
            anyhow::bail!("No download providers enabled");
        }
        let path = try_steamcmd(game_app_id, &mod_name, &game_name)?;
        return Ok(QueueOutcome::Downloaded {
            mod_name,
            game_name,
            path,
        });
    }

    // GGNetwork first
    let gg_result = (|| -> anyhow::Result<QueueOutcome> {
        let item = workshop::lookup_item(workshop_id)?;
        if !item.is_available() {
            anyhow::bail!("Not available on GGNetwork");
        }
        let app_id = item.game_app_id().unwrap_or(game_app_id);
        if app_id == 0 {
            anyhow::bail!("Invalid game App ID from GGNetwork");
        }
        let (resolved_mod, resolved_game) = match workshop::fetch_workshop_page_info(workshop_id) {
            Ok((m, g, _)) => (m, g.unwrap_or_else(|| format!("App {app_id}"))),
            Err(_) => {
                let m = if item.name.trim().is_empty() {
                    format!("Item {workshop_id}")
                } else {
                    item.name.clone()
                };
                (m, format!("App {app_id}"))
            }
        };
        let dest = settings.game_download_dir(&resolved_game)?;
        let path = workshop::download_mod(&item, &dest)?;
        Ok(QueueOutcome::Downloaded {
            mod_name: resolved_mod,
            game_name: resolved_game,
            path,
        })
    })();

    match gg_result {
        Ok(ok) => Ok(ok),
        Err(gg_err) => {
            if settings.use_steamcmd && !settings.ask_before_steamcmd {
                let path = try_steamcmd(game_app_id, &mod_name, &game_name).map_err(|sc_err| {
                    anyhow::anyhow!("{gg_err:#}; SteamCMD: {sc_err:#}")
                })?;
                Ok(QueueOutcome::Downloaded {
                    mod_name,
                    game_name,
                    path,
                })
            } else {
                Err(gg_err)
            }
        }
    }
}

impl eframe::App for GoldbergDropApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        Color32::TRANSPARENT.to_normalized_gamma_f32()
    }

    /// Runs even when the window is hidden — tray + worker must live here, not
    /// only in `ui`, or tray-menu actions stall until the window is focused.
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.poll_worker();
        self.poll_tray(ctx);
        self.poll_tray_banner(ctx);
        self.enforce_tray_hidden(ctx, frame);

        if self.dormant_in_tray || self.tray.is_some() {
            ctx.request_repaint_after(Duration::from_millis(200));
        }

        if self.screen == Screen::Working
            || self.queue_processing
            || self.queue_lookup_pending()
        {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Keep solid + contrasted scrollbars if style gets reset.
        if ui.style().spacing.scroll.floating || !ui.style().spacing.scroll.foreground_color {
            ui.ctx().all_styles_mut(|style| {
                let mut scroll = egui::style::ScrollStyle::solid();
                scroll.bar_width = 8.0;
                scroll.bar_inner_margin = 2.0;
                scroll.foreground_color = true;
                style.spacing.scroll = scroll;
            });
        }

        let ctx = ui.ctx().clone();
        self.handle_dropped_files(&ctx);

        let panel_frame = egui::Frame::new()
            .fill(colors::PANEL_BG)
            .stroke(Stroke::new(1.0, colors::PANEL_BORDER))
            .corner_radius(CornerRadius::same(WINDOW_CORNER_RADIUS))
            // A soft shadow we draw ourselves, following the panel's own
            // rounded shape — unlike the OS window shadow, which is a hard
            // rectangle and would poke out past the rounded corners.
            .shadow(egui::Shadow {
                offset: [0, 6],
                blur: 28,
                spread: 0,
                color: Color32::from_black_alpha(110),
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                panel_frame.show(ui, |ui| {
                    ui.set_min_size(ui.available_size());
                    let app_rect = ui.max_rect();

                    let title_bar_rect = egui::Rect::from_min_size(
                        app_rect.min,
                        egui::vec2(app_rect.width(), TITLE_BAR_HEIGHT),
                    );
                    self.draw_title_bar(ui, title_bar_rect);

                    let ribbon_rect = egui::Rect::from_min_max(
                        egui::pos2(app_rect.min.x, title_bar_rect.max.y),
                        egui::pos2(app_rect.max.x, title_bar_rect.max.y + RIBBON_HEIGHT),
                    );
                    self.draw_ribbon(ui, ribbon_rect);

                    let content_rect = egui::Rect::from_min_max(
                        egui::pos2(app_rect.min.x, ribbon_rect.max.y),
                        app_rect.max,
                    )
                    .shrink2(egui::vec2(CONTENT_MARGIN, CONTENT_MARGIN * 0.7));

                    let mut content_ui = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(content_rect)
                            .layout(egui::Layout::top_down(egui::Align::Min)),
                    );
                    self.draw_content(&ctx, &mut content_ui);

                    if self.import_list_overlay_open {
                        self.draw_import_list_overlay(ui, app_rect);
                    } else if self.failed_overlay_open {
                        self.draw_failed_overlay(ui, app_rect);
                    }

                    if self.tray_banner_until.is_some() {
                        let banner = egui::Rect::from_center_size(
                            app_rect.center(),
                            egui::vec2(app_rect.width() - 48.0, 52.0),
                        );
                        ui.painter().rect_filled(
                            banner,
                            CornerRadius::same(CARD_RADIUS),
                            colors::SURFACE,
                        );
                        ui.painter().rect_stroke(
                            banner,
                            CornerRadius::same(CARD_RADIUS),
                            Stroke::new(1.0, colors::ACCENT),
                            egui::StrokeKind::Outside,
                        );
                        ui.painter().text(
                            banner.center(),
                            egui::Align2::CENTER_CENTER,
                            "GoldbergDrop is running in the system tray",
                            egui::FontId::proportional(13.0),
                            colors::TEXT_PRIMARY,
                        );
                    }
                });
            });
    }
}

impl GoldbergDropApp {
    fn draw_title_bar(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let drag_response =
            ui.interact(rect, egui::Id::new("gd_title_bar"), egui::Sense::click_and_drag());
        if drag_response.drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }

        ui.painter().line_segment(
            [
                rect.left_bottom() + egui::vec2(1.0, 0.0),
                rect.right_bottom() + egui::vec2(-1.0, 0.0),
            ],
            Stroke::new(1.0, colors::PANEL_BORDER),
        );

        let icon = self.icon_texture(ui.ctx()).cloned();

        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
            ui.horizontal_centered(|ui| {
                ui.add_space(12.0);
                if let Some(icon) = icon {
                    ui.add(
                        egui::Image::new(&icon)
                            .max_size(egui::vec2(17.0, 17.0))
                            .corner_radius(4),
                    );
                    ui.add_space(7.0);
                } else {
                    ui.add_space(2.0);
                }
                ui.add(
                    egui::Label::new(
                        RichText::new("GOLDBERGDROP")
                            .color(colors::ACCENT)
                            .strong()
                            .size(12.5),
                    )
                    .selectable(false),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(6.0);
                    if Self::title_bar_button(ui, "✕").clicked() {
                        if self.settings.close_to_tray {
                            self.hide_to_tray_with_notice();
                        } else {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    }
                    if Self::title_bar_button(ui, "–").clicked() {
                        ui.ctx()
                            .send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                    }
                });
            });
        });
    }

    fn title_bar_button(ui: &mut egui::Ui, symbol: &str) -> egui::Response {
        Self::title_bar_button_colored(ui, symbol, colors::TEXT_MUTED)
    }

    fn title_bar_button_colored(ui: &mut egui::Ui, symbol: &str, color: Color32) -> egui::Response {
        let button = egui::Button::new(RichText::new(symbol).size(13.0).color(color))
            .frame(false)
            .min_size(egui::vec2(26.0, TITLE_BAR_HEIGHT - 4.0))
            .corner_radius(CONTROL_RADIUS);
        ui.add(button)
    }

    /// Custom button with horizontal padding and centered label.
    fn paint_button(
        ui: &mut egui::Ui,
        label: &str,
        fill: Color32,
        fill_hover: Color32,
        stroke: Stroke,
        text_color: Color32,
        height: f32,
        width: f32,
        font_size: f32,
    ) -> egui::Response {
        let galley = ui.painter().layout(
            label.to_owned(),
            egui::FontId::proportional(font_size),
            text_color,
            f32::INFINITY,
        );
        let button_width = if width > 0.0 {
            width
        } else {
            galley.size().x + BUTTON_PAD_X * 2.0
        };
        let size = egui::vec2(button_width, height);
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());

        if ui.is_rect_visible(rect) {
            let enabled = ui.is_enabled();
            let hovered = enabled && response.hovered();
            let bg = if !enabled {
                fill.linear_multiply(0.45)
            } else if hovered {
                fill_hover
            } else {
                fill
            };
            let label_color = if enabled {
                text_color
            } else {
                colors::TEXT_MUTED
            };

            ui.painter()
                .rect_filled(rect, CornerRadius::same(CONTROL_RADIUS), bg);
            if stroke.width > 0.0 {
                ui.painter().rect_stroke(
                    rect,
                    CornerRadius::same(CONTROL_RADIUS),
                    stroke,
                    egui::StrokeKind::Inside,
                );
            }
            ui.painter().galley(
                rect.center() - galley.size() / 2.0,
                galley,
                label_color,
            );
        }

        response
    }

    /// Primary action — accent fill, restrained radius, consistent height.
    fn primary_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
        Self::paint_button(
            ui,
            label,
            colors::ACCENT,
            colors::ACCENT_HOVER,
            Stroke::new(1.0, colors::ACCENT_ACTIVE),
            colors::PANEL_BG,
            CONTROL_HEIGHT,
            ui.available_width(),
            12.5,
        )
    }

    /// Secondary action — quiet surface button with a clear border.
    fn secondary_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
        Self::paint_button(
            ui,
            label,
            colors::SURFACE,
            colors::SURFACE_HOVER,
            Stroke::new(1.0, colors::PANEL_BORDER),
            colors::TEXT_PRIMARY,
            CONTROL_HEIGHT,
            ui.available_width(),
            12.5,
        )
    }

    /// Inset single-line text field with focus ring.
    fn text_field(ui: &mut egui::Ui, text: &mut String, hint: &str, width: f32) -> egui::Response {
        let outer = egui::Frame::new()
            .fill(colors::INPUT_BG)
            .stroke(Stroke::new(1.0, colors::INPUT_BORDER))
            .corner_radius(CornerRadius::same(CONTROL_RADIUS))
            .inner_margin(egui::Margin::symmetric(10, 7));

        let inner_width = (width - 20.0).max(40.0);
        let response = outer.show(ui, |ui| {
            ui.add_sized(
                [inner_width, INPUT_HEIGHT - 14.0],
                egui::TextEdit::singleline(text)
                    .hint_text(hint)
                    .frame(egui::Frame::NONE)
                    .margin(egui::Margin::ZERO),
            )
        });

        if response.response.has_focus() {
            ui.painter().rect_stroke(
                response.response.rect.expand(0.0),
                CornerRadius::same(CONTROL_RADIUS),
                Stroke::new(1.0, colors::INPUT_BORDER_FOCUS),
                egui::StrokeKind::Outside,
            );
        }

        response.response
    }

    fn field_label(ui: &mut egui::Ui, label: &str) {
        ui.add(
            egui::Label::new(RichText::new(label).color(colors::TEXT_MUTED).size(10.0))
                .selectable(false),
        );
        ui.add_space(4.0);
    }

    /// Colored status indicator for queue rows.
    /// `!` = not available on GGNetwork (instead of the usual queued dot).
    fn queue_status_dot(
        ui: &mut egui::Ui,
        status: &QueueStatus,
        gg_available: Option<bool>,
    ) -> egui::Response {
        let size = egui::vec2(queue_cols::DOT, QUEUE_ROW_HEIGHT);
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());

        let hover = match status {
            QueueStatus::Queued if gg_available == Some(false) => {
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "!",
                    egui::FontId::proportional(14.0),
                    colors::ERROR,
                );
                "Not available on GGNetwork"
            }
            QueueStatus::Queued => {
                ui.painter()
                    .circle_filled(rect.center(), 4.0, colors::ACCENT);
                "Queued"
            }
            QueueStatus::Downloading => {
                ui.painter()
                    .circle_filled(rect.center(), 4.0, colors::ACCENT);
                ui.painter().circle_stroke(
                    rect.center(),
                    6.0,
                    Stroke::new(1.0, colors::ACCENT_HOVER.linear_multiply(0.45)),
                );
                "Downloading"
            }
            QueueStatus::Done => {
                ui.painter()
                    .circle_filled(rect.center(), 4.0, colors::SUCCESS);
                "Done"
            }
            QueueStatus::Failed(msg) => {
                ui.painter()
                    .circle_filled(rect.center(), 4.0, colors::ERROR);
                msg.as_str()
            }
        };

        response.on_hover_text(hover)
    }

    fn queue_cell_label(ui: &mut egui::Ui, text: &str, width: f32) {
        ui.allocate_ui_with_layout(
            egui::vec2(width, QUEUE_ROW_HEIGHT),
            egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
            |ui| {
                ui.add(
                    egui::Label::new(RichText::new(text).color(colors::TEXT_PRIMARY).size(11.5))
                        .truncate(),
                );
            },
        );
    }

    fn queue_header_cell(ui: &mut egui::Ui, text: &str, width: f32) {
        ui.allocate_ui_with_layout(
            egui::vec2(width, 22.0),
            egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
            |ui| {
                ui.label(RichText::new(text).color(colors::TEXT_MUTED).size(10.5).strong());
            },
        );
    }

    fn queue_table_vlines(ui: &mut egui::Ui, rect: egui::Rect) {
        let stroke = Stroke::new(1.0, colors::PANEL_BORDER.linear_multiply(0.65));
        let mut x = rect.left() + queue_cols::DOT;
        for width in [queue_cols::MOD, queue_cols::GAME, queue_cols::ID] {
            ui.painter()
                .vline(x, rect.top()..=rect.bottom(), stroke);
            x += width;
        }
    }

    fn fail_table_vlines(ui: &mut egui::Ui, rect: egui::Rect) {
        let stroke = Stroke::new(1.0, colors::PANEL_BORDER.linear_multiply(0.65));
        let mut x = rect.left();
        for width in [fail_cols::MOD, fail_cols::GAME] {
            x += width;
            ui.painter()
                .vline(x, rect.top()..=rect.bottom(), stroke);
        }
    }

    fn fail_table_header(ui: &mut egui::Ui) {
        let header_top = ui.cursor().min.y;
        ui.painter().rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(ui.max_rect().left(), header_top),
                egui::vec2(fail_cols::TOTAL, 22.0),
            ),
            CornerRadius::ZERO,
            colors::SURFACE.linear_multiply(0.85),
        );

        ui.horizontal(|ui| {
            ui.set_width(fail_cols::TOTAL);
            ui.set_height(22.0);
            ui.spacing_mut().item_spacing.x = 0.0;
            Self::queue_header_cell(ui, "Mod", fail_cols::MOD);
            Self::queue_header_cell(ui, "Game", fail_cols::GAME);
            Self::queue_header_cell(ui, "", fail_cols::ACTION);
        });

        let header_rect = egui::Rect::from_min_size(
            egui::pos2(ui.max_rect().left(), header_top),
            egui::vec2(fail_cols::TOTAL, 22.0),
        );
        Self::fail_table_vlines(ui, header_rect);
        ui.painter().hline(
            header_rect.left()..=header_rect.right(),
            header_rect.bottom(),
            Stroke::new(1.0, colors::PANEL_BORDER),
        );
    }

    fn fail_table_row(ui: &mut egui::Ui, mod_name: &str, game_name: &str, page_url: &str) {
        let row_rect = ui.available_rect_before_wrap();
        let row_top = ui.cursor().min.y;

        ui.horizontal(|ui| {
            ui.set_width(fail_cols::TOTAL);
            ui.set_height(QUEUE_ROW_HEIGHT);
            ui.spacing_mut().item_spacing.x = 0.0;

            Self::queue_cell_label(ui, mod_name, fail_cols::MOD);
            Self::queue_cell_label(ui, game_name, fail_cols::GAME);
            ui.allocate_ui_with_layout(
                egui::vec2(fail_cols::ACTION, QUEUE_ROW_HEIGHT),
                egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                |ui| {
                    if Self::icon_button(ui, ICON_OPEN, colors::ACCENT)
                        .on_hover_text("Open GGNetwork page")
                        .clicked()
                    {
                        ui.ctx()
                            .open_url(egui::OpenUrl::new_tab(page_url.to_owned()));
                    }
                },
            );
        });

        let row_rect = egui::Rect::from_min_size(
            egui::pos2(row_rect.left(), row_top),
            egui::vec2(fail_cols::TOTAL, QUEUE_ROW_HEIGHT),
        );
        Self::fail_table_vlines(ui, row_rect);
        ui.painter().hline(
            row_rect.left()..=row_rect.right(),
            row_rect.bottom(),
            Stroke::new(1.0, colors::PANEL_BORDER.linear_multiply(0.65)),
        );
    }

    /// Empty placeholder row — only the table grid, no filler shapes.
    fn queue_skeleton_row(ui: &mut egui::Ui, _index: usize) {
        let row_rect = ui.available_rect_before_wrap();
        let row_top = ui.cursor().min.y;
        let table_width = queue_cols::TOTAL;

        ui.allocate_exact_size(
            egui::vec2(table_width, QUEUE_ROW_HEIGHT),
            egui::Sense::hover(),
        );

        let row_rect = egui::Rect::from_min_size(
            egui::pos2(row_rect.left(), row_top),
            egui::vec2(table_width, QUEUE_ROW_HEIGHT),
        );
        Self::queue_table_vlines(ui, row_rect);
        ui.painter().hline(
            row_rect.left()..=row_rect.right(),
            row_rect.bottom(),
            Stroke::new(1.0, colors::PANEL_BORDER.linear_multiply(0.65)),
        );
    }

    fn queue_table_header(ui: &mut egui::Ui) {
        let header_top = ui.cursor().min.y;
        ui.painter().rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(ui.max_rect().left(), header_top),
                egui::vec2(queue_cols::TOTAL, 22.0),
            ),
            CornerRadius::ZERO,
            colors::SURFACE.linear_multiply(0.85),
        );

        ui.horizontal(|ui| {
            ui.set_width(queue_cols::TOTAL);
            ui.set_height(22.0);
            ui.spacing_mut().item_spacing.x = 0.0;
            Self::queue_header_cell(ui, "", queue_cols::DOT);
            Self::queue_header_cell(ui, "Mod", queue_cols::MOD);
            Self::queue_header_cell(ui, "Game", queue_cols::GAME);
            Self::queue_header_cell(ui, "ID", queue_cols::ID);
            Self::queue_header_cell(ui, "", queue_cols::ACTION);
        });

        let header_rect = egui::Rect::from_min_size(
            egui::pos2(ui.max_rect().left(), header_top),
            egui::vec2(queue_cols::TOTAL, 22.0),
        );
        Self::queue_table_vlines(ui, header_rect);
        ui.painter().hline(
            header_rect.left()..=header_rect.right(),
            header_rect.bottom(),
            Stroke::new(1.0, colors::PANEL_BORDER),
        );
    }

    fn queue_table_row(
        ui: &mut egui::Ui,
        item: &QueueItem,
        index: usize,
        striped: bool,
    ) -> Option<usize> {
        let row_rect = ui.available_rect_before_wrap();
        let row_top = ui.cursor().min.y;
        let table_width = queue_cols::TOTAL;

        if striped {
            ui.painter().rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(row_rect.left(), row_top),
                    egui::vec2(table_width, QUEUE_ROW_HEIGHT),
                ),
                CornerRadius::ZERO,
                colors::PANEL_BG.linear_multiply(0.55),
            );
        }

        let mut remove_index = None;
        ui.horizontal(|ui| {
            ui.set_width(table_width);
            ui.set_height(QUEUE_ROW_HEIGHT);
            ui.spacing_mut().item_spacing.x = 0.0;

            ui.allocate_ui_with_layout(
                egui::vec2(queue_cols::DOT, QUEUE_ROW_HEIGHT),
                egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                |ui| {
                    Self::queue_status_dot(ui, &item.status, item.gg_available);
                },
            );
            Self::queue_cell_label(ui, &item.mod_name, queue_cols::MOD);
            Self::queue_cell_label(ui, &item.game_name, queue_cols::GAME);
            Self::queue_cell_label(ui, &item.workshop_id.to_string(), queue_cols::ID);
            ui.allocate_ui_with_layout(
                egui::vec2(queue_cols::ACTION, QUEUE_ROW_HEIGHT),
                egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                |ui| {
                    match item.status {
                        QueueStatus::Done => {
                            if item.download_path.is_some() && Self::queue_folder_button(ui).clicked()
                            {
                                if let Some(path) = &item.download_path {
                                    open_download_in_explorer(path);
                                }
                            }
                        }
                        QueueStatus::Queued | QueueStatus::Failed(_) => {
                            if Self::queue_remove_button(ui).clicked() {
                                remove_index = Some(index);
                            }
                        }
                        QueueStatus::Downloading => {}
                    }
                },
            );
        });

        let row_rect = egui::Rect::from_min_size(
            egui::pos2(row_rect.left(), row_top),
            egui::vec2(table_width, QUEUE_ROW_HEIGHT),
        );
        Self::queue_table_vlines(ui, row_rect);
        ui.painter().hline(
            row_rect.left()..=row_rect.right(),
            row_rect.bottom(),
            Stroke::new(1.0, colors::PANEL_BORDER.linear_multiply(0.65)),
        );

        remove_index
    }

    fn queue_remove_button(ui: &mut egui::Ui) -> egui::Response {
        Self::paint_button(
            ui,
            "✕",
            Color32::TRANSPARENT,
            colors::SURFACE_HOVER,
            Stroke::NONE,
            colors::TEXT_MUTED,
            20.0,
            queue_cols::ACTION,
            11.0,
        )
    }

    fn queue_folder_button(ui: &mut egui::Ui) -> egui::Response {
        Self::paint_button(
            ui,
            "📁",
            Color32::TRANSPARENT,
            colors::SURFACE_HOVER,
            Stroke::NONE,
            colors::TEXT_MUTED,
            20.0,
            queue_cols::ACTION,
            12.0,
        )
        .on_hover_text("Open download folder")
    }

    fn has_queued_items(&self) -> bool {
        self.download_queue
            .iter()
            .any(|item| item.status == QueueStatus::Queued)
    }

    fn has_failed_items(&self) -> bool {
        self.download_queue
            .iter()
            .any(|item| matches!(item.status, QueueStatus::Failed(_)))
    }

    fn maybe_show_failed_overlay(&mut self) {
        if !self.failed_overlay_suppress && self.has_failed_items() {
            self.failed_overlay_open = true;
        }
    }

    fn dismiss_failed_overlay(&mut self) {
        self.failed_overlay_open = false;
        self.failed_overlay_suppress = true;
    }

    /// A single row inside the candidate-match list card. Unselected rows
    /// blend into the card's own background (only a hover tint shows), so
    /// the list reads as one cohesive group instead of a stack of separate
    /// pill-shaped buttons.
    fn candidate_row(ui: &mut egui::Ui, label: &str, selected: bool) -> egui::Response {
        let size = egui::vec2(ui.available_width(), 32.0);
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());

        let bg = if selected {
            colors::ACCENT
        } else if response.hovered() {
            colors::SURFACE_HOVER
        } else {
            Color32::TRANSPARENT
        };
        if bg != Color32::TRANSPARENT {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(CONTROL_RADIUS), bg);
        }

        let text_color = if selected {
            colors::PANEL_BG
        } else {
            colors::TEXT_PRIMARY
        };
        ui.painter().with_clip_rect(rect).text(
            rect.left_center() + egui::vec2(10.0, 0.0),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(12.5),
            text_color,
        );

        response
    }

    /// Tertiary accent action (Browse…, Fix Send to).
    fn ghost_accent_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
        Self::paint_button(
            ui,
            label,
            colors::SURFACE,
            colors::SURFACE_HOVER,
            Stroke::new(1.0, colors::PANEL_BORDER),
            colors::ACCENT,
            28.0,
            0.0,
            12.5,
        )
    }

    fn draw_ribbon(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        ui.painter().line_segment(
            [
                rect.left_bottom() + egui::vec2(1.0, 0.0),
                rect.right_bottom() + egui::vec2(-1.0, 0.0),
            ],
            Stroke::new(1.0, colors::PANEL_BORDER),
        );

        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
            ui.horizontal_centered(|ui| {
                egui::Frame::new()
                    .fill(colors::SURFACE)
                    .stroke(Stroke::new(1.0, colors::PANEL_BORDER))
                    .corner_radius(CornerRadius::same(CONTROL_RADIUS))
                    .inner_margin(egui::Margin::same(2))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            if self.ribbon_tab_button(
                                ui,
                                "GoldbergDrop",
                                self.active_tab == AppTab::Setup,
                            ) {
                                self.active_tab = AppTab::Setup;
                            }
                            if self.ribbon_tab_button(
                                ui,
                                "WorkshopDL",
                                self.active_tab == AppTab::WorkshopDl,
                            ) {
                                self.active_tab = AppTab::WorkshopDl;
                            }
                        });
                    });

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.ribbon_tab_button(ui, "⚙", self.active_tab == AppTab::Settings) {
                        self.active_tab = AppTab::Settings;
                    }
                });
            });
        });
    }

    fn ribbon_tab_button(&self, ui: &mut egui::Ui, label: &str, selected: bool) -> bool {
        if selected {
            Self::paint_button(
                ui,
                label,
                colors::ACCENT,
                colors::ACCENT_HOVER,
                Stroke::new(1.0, colors::ACCENT_ACTIVE),
                colors::PANEL_BG,
                RIBBON_HEIGHT - 10.0,
                0.0,
                12.0,
            )
            .clicked()
        } else {
            Self::paint_button(
                ui,
                label,
                Color32::TRANSPARENT,
                colors::SURFACE_HOVER,
                Stroke::NONE,
                colors::TEXT_MUTED,
                RIBBON_HEIGHT - 10.0,
                0.0,
                12.0,
            )
            .clicked()
        }
    }

    fn draw_settings(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new("SETTINGS")
                .color(colors::TEXT_MUTED)
                .size(10.0),
        );
        ui.add_space(6.0);

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (tab, label) in [
                (SettingsTab::Download, "Download"),
                (SettingsTab::SteamCmd, "SteamCMD"),
                (SettingsTab::Paths, "Paths"),
                (SettingsTab::Setup, "Setup"),
                (SettingsTab::Tray, "Tray"),
            ] {
                if self.ribbon_tab_button(ui, label, self.settings_tab == tab) {
                    self.settings_tab = tab;
                }
            }
        });
        ui.add_space(10.0);

        let mut dirty = false;
        match self.settings_tab {
            SettingsTab::Download => {
                ui.label(
                    RichText::new("Download methods (order: GGNetwork → SteamCMD)")
                        .color(colors::TEXT_MUTED)
                        .size(11.0),
                );
                ui.add_space(6.0);
                if ui
                    .checkbox(&mut self.settings.use_ggnetwork, "GGNetwork")
                    .changed()
                {
                    dirty = true;
                }
                if ui
                    .checkbox(&mut self.settings.use_steamcmd, "SteamCMD")
                    .changed()
                {
                    dirty = true;
                }
                ui.add_space(4.0);
                ui.add_enabled_ui(self.settings.use_steamcmd, |ui| {
                    if ui
                        .checkbox(
                            &mut self.settings.ask_before_steamcmd,
                            "Ask before SteamCMD fallback",
                        )
                        .changed()
                    {
                        dirty = true;
                    }
                });
                ui.add_space(6.0);
                ui.label(
                    RichText::new(
                        "When ask is on, failed GG downloads open the overlay with a Retry button.",
                    )
                    .color(colors::TEXT_MUTED)
                    .size(11.0),
                );
            }
            SettingsTab::SteamCmd => {
                let installed = AppSettings::steamcmd_installed();
                ui.label(
                    RichText::new(if installed {
                        "SteamCMD: installed (%LOCALAPPDATA%\\GoldbergDrop\\…\\steamcmd)"
                    } else {
                        "SteamCMD: not installed yet — downloaded on first use to AppData"
                    })
                    .color(if installed {
                        colors::SUCCESS
                    } else {
                        colors::TEXT_MUTED
                    })
                    .size(12.0),
                );
                ui.add_space(8.0);
                ui.label(
                    RichText::new("v1 uses anonymous login only. Account login comes later.")
                        .color(colors::TEXT_MUTED)
                        .size(11.0),
                );
                ui.add_space(8.0);
                if let Some(status) = &self.steamcmd_setup_status {
                    ui.label(
                        RichText::new(status)
                            .color(if status.contains("failed") || status.contains("Remove failed")
                            {
                                colors::ERROR
                            } else {
                                colors::TEXT_MUTED
                            })
                            .size(11.0),
                    );
                    ui.add_space(6.0);
                }
                let ensuring = self
                    .steamcmd_setup_status
                    .as_deref()
                    .is_some_and(|s| s.contains("Downloading") || s.contains("deploying"));
                ui.add_enabled_ui(!ensuring, |ui| {
                    ui.columns(2, |columns| {
                        if Self::secondary_button(&mut columns[0], "Install / Update SteamCMD")
                            .clicked()
                        {
                            self.steamcmd_setup_status = Some(
                                "Downloading and deploying SteamCMD…".to_string(),
                            );
                            self.start_steamcmd_ensure();
                        }
                        columns[1].add_enabled_ui(installed, |ui| {
                            if Self::secondary_button(ui, "Remove SteamCMD").clicked() {
                                match steamcmd::remove_steamcmd() {
                                    Ok(()) => {
                                        // Clear shared status so the Failed overlay doesn't
                                        // show "removed" — next Retry will ask to download again.
                                        self.steamcmd_setup_status = None;
                                        self.pending_steamcmd_retry = false;
                                    }
                                    Err(e) => {
                                        self.steamcmd_setup_status =
                                            Some(format!("Remove failed: {e:#}"));
                                    }
                                }
                            }
                        });
                    });
                });
            }
            SettingsTab::Paths => {
                Self::field_label(ui, "WORKSHOP DOWNLOAD FOLDER");
                let path_text = self.settings.display_workshop_root();
                ui.label(
                    RichText::new(path_text)
                        .color(colors::TEXT_PRIMARY)
                        .size(11.5),
                );
                ui.add_space(8.0);
                ui.columns(2, |columns| {
                    if Self::secondary_button(&mut columns[0], "Browse…").clicked() {
                        if let Some(dir) = rfd::FileDialog::new()
                            .set_title("Workshop download folder")
                            .pick_folder()
                        {
                            self.settings.workshop_download_dir = Some(dir);
                            dirty = true;
                        }
                    }
                    if Self::secondary_button(&mut columns[1], "Reset default").clicked() {
                        self.settings.workshop_download_dir = None;
                        dirty = true;
                    }
                });
            }
            SettingsTab::Setup => {
                if ui
                    .checkbox(
                        &mut self.settings.fetch_dlc_default,
                        "Fetch DLCs by default",
                    )
                    .changed()
                {
                    self.fetch_dlc = self.settings.fetch_dlc_default;
                    dirty = true;
                }
                if ui
                    .checkbox(
                        &mut self.settings.fetch_achievements_default,
                        "Fetch achievements by default",
                    )
                    .changed()
                {
                    self.fetch_achievements = self.settings.fetch_achievements_default;
                    dirty = true;
                }
                ui.add_space(4.0);
                ui.label(
                    RichText::new("Applies to new sessions and syncs the Setup-tab checkboxes.")
                        .color(colors::TEXT_MUTED)
                        .size(11.0),
                );
            }
            SettingsTab::Tray => {
                if ui
                    .checkbox(
                        &mut self.settings.close_to_tray,
                        "Close to tray (✕ hides instead of quitting)",
                    )
                    .changed()
                {
                    dirty = true;
                    if self.settings.close_to_tray {
                        self.ensure_tray();
                    }
                }
                ui.add_space(6.0);
                let mut autostart = self.settings.autostart_tray;
                if ui
                    .checkbox(&mut autostart, "Autostart with Windows (in tray)")
                    .changed()
                {
                    self.settings.autostart_tray = autostart;
                    dirty = true;
                    let result = if autostart {
                        autostart::enable()
                    } else {
                        autostart::disable()
                    };
                    if let Err(e) = result {
                        eprintln!("Autostart change failed: {e:#}");
                    }
                }
                ui.add_space(4.0);
                ui.label(
                    RichText::new(if autostart::is_enabled() {
                        "Autostart registry entry is present."
                    } else {
                        "Autostart is off."
                    })
                    .color(colors::TEXT_MUTED)
                    .size(11.0),
                );
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "Tracked games appear in the tray right-click menu (icon + name).",
                    )
                    .color(colors::TEXT_MUTED)
                    .size(11.0),
                );
                ui.add_space(10.0);
                Self::field_label(ui, "TRACKED GAMES");
                let tracked = games::load();
                if tracked.is_empty() {
                    ui.label(
                        RichText::new("None yet — set up a game to add it here.")
                            .color(colors::TEXT_MUTED)
                            .size(11.5),
                    );
                } else {
                    let mut remove_id = None;
                    egui::ScrollArea::vertical()
                        .max_height(160.0)
                        .show(ui, |ui| {
                            for g in &tracked {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(&g.name)
                                            .color(colors::TEXT_PRIMARY)
                                            .size(12.0),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if Self::icon_button(ui, "✕", colors::TEXT_MUTED)
                                                .on_hover_text("Remove from tray list")
                                                .clicked()
                                            {
                                                remove_id = Some(g.app_id);
                                            }
                                        },
                                    );
                                });
                            }
                        });
                    if let Some(id) = remove_id {
                        let _ = games::remove(id);
                        self.refresh_tray_menu();
                    }
                }
            }
        }

        if dirty {
            self.persist_settings();
        }
    }

    fn draw_content(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        if self.active_tab == AppTab::Settings {
            self.draw_settings(ui);
            return;
        }

        if self.active_tab == AppTab::WorkshopDl {
            self.draw_workshop_dl(ui);
            return;
        }

        if self.screen == Screen::ChooseMatch {
            // Multiple matches need room for a scrollable list, so this
            // takes over the whole content area instead of squeezing in
            // below the drop zone.
            self.draw_choose_match_fullscreen(ui);
            return;
        }

        ui.add(
            egui::Label::new(
                RichText::new("Drop a game .exe here, or browse.")
                    .color(colors::TEXT_MUTED)
                    .size(12.5),
            )
            .selectable(false),
        );
        ui.add_space(10.0);

        self.draw_drop_zone(ctx, ui);

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if ui.checkbox(&mut self.fetch_dlc, "Fetch DLCs").changed() {
                self.settings.fetch_dlc_default = self.fetch_dlc;
                self.persist_settings();
            }
            ui.add_space(16.0);
            if ui
                .checkbox(&mut self.fetch_achievements, "Fetch achievements")
                .changed()
            {
                self.settings.fetch_achievements_default = self.fetch_achievements;
                self.persist_settings();
            }
            ui.add_space(16.0);
            if ui
                .checkbox(&mut self.sendto_enabled, "\"Send to\" entry")
                .changed()
            {
                self.on_sendto_checkbox_changed();
            }
        });

        if self.sendto_stale {
            ui.add_space(4.0);
            if Self::paint_button(
                ui,
                "GoldbergDrop was moved — Fix \"Send to\"",
                colors::SURFACE,
                colors::SURFACE_HOVER,
                Stroke::new(1.0, colors::ERROR.linear_multiply(0.55)),
                colors::ERROR,
                CONTROL_HEIGHT,
                ui.available_width(),
                12.0,
            )
            .clicked()
            {
                self.refresh_sendto();
            }
        }
        if let Some((message, is_error)) = &self.sendto_notice {
            ui.add_space(4.0);
            let color = if *is_error { colors::ERROR } else { colors::TEXT_MUTED };
            ui.label(RichText::new(message.as_str()).color(color).size(11.5));
        }

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(10.0);

        self.draw_screen(ui);
    }

    fn draw_drop_zone(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        let is_hovering_file = !ctx.input(|i| i.raw.hovered_files.is_empty());

        let frame = egui::Frame::new()
            .fill(if is_hovering_file {
                colors::ACCENT.linear_multiply(0.18)
            } else {
                colors::SURFACE
            })
            .stroke(Stroke::new(
                1.0,
                if is_hovering_file {
                    colors::ACCENT
                } else {
                    colors::PANEL_BORDER
                },
            ))
            .corner_radius(CornerRadius::same(CARD_RADIUS))
            // Roughly doubles the previous drop-zone footprint.
            .inner_margin(egui::Margin::symmetric(16, 62));

        frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.vertical_centered(|ui| {
                match &self.exe_path {
                    Some(path) => {
                        ui.add(
                            egui::Label::new(
                                RichText::new(
                                    path.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
                                )
                                .color(colors::TEXT_PRIMARY)
                                .strong()
                                .size(13.5),
                            )
                            .selectable(false),
                        );
                    }
                    None => {
                        ui.add(
                            egui::Label::new(
                                RichText::new("Drop .exe here")
                                    .color(colors::TEXT_PRIMARY)
                                    .size(13.5),
                            )
                            .selectable(false),
                        );
                    }
                }
                ui.add_space(10.0);
                if Self::ghost_accent_button(ui, "Browse...").clicked() {
                    self.browse_for_exe();
                }
            });
        });
    }

    fn draw_screen(&mut self, ui: &mut egui::Ui) {
        match self.screen {
            Screen::Idle => {
                ui.label(RichText::new("Ready.").color(colors::TEXT_MUTED));
            }
            Screen::Working => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(&self.working_message);
                });
            }
            // Drawn as a dedicated full-window view from `draw_content`
            // instead, since it needs much more room than the other screens.
            Screen::ChooseMatch => {}
            Screen::ManualId => self.draw_manual_id(ui),
            Screen::Done => {
                ui.colored_label(colors::SUCCESS, &self.result_message);
                ui.add_space(8.0);
                if Self::primary_button(ui, "Set up another game").clicked() {
                    self.reset();
                }
            }
            Screen::Error => {
                ui.colored_label(colors::ERROR, &self.result_message);
                ui.add_space(8.0);
                if Self::secondary_button(ui, "Try again").clicked() {
                    self.reset();
                }
            }
        }
    }

    /// Full-window "pick the right game" view for when the automatic search
    /// found several candidates. A candidate button just fills in the App ID
    /// field at the bottom (which is always visible and editable) — nothing
    /// is applied until the user presses Apply.
    fn draw_choose_match_fullscreen(&mut self, ui: &mut egui::Ui) {
        ui.add(
            egui::Label::new(
                RichText::new("Multiple matches found")
                    .strong()
                    .color(colors::TEXT_PRIMARY)
                    .size(14.0),
            )
            .selectable(false),
        );
        ui.add(
            egui::Label::new(
                RichText::new("Pick the right game, or type the App ID below.")
                    .color(colors::TEXT_MUTED)
                    .size(12.0),
            )
            .selectable(false),
        );
        ui.add_space(8.0);

        // Reserve room at the bottom for the ID field + buttons so the
        // scrollable list above never overlaps — and never pushes them —
        // past the bottom of the window. Capped at a fixed height rather
        // than filling all remaining space, since text/spacing rounding
        // made the previous "fill exactly what's left" math run a bit
        // short and clip the button row.
        let bottom_bar_height = if self.manual_id_error.is_some() {
            130.0
        } else {
            108.0
        };
        let list_height = (ui.available_height() - bottom_bar_height).clamp(40.0, 260.0);

        let current_id = self.manual_id_input.trim().parse::<u32>().ok();
        let mut chosen: Option<SteamApp> = None;

        let list_card = egui::Frame::new()
            .fill(colors::SURFACE)
            .stroke(Stroke::new(1.0, colors::PANEL_BORDER))
            .corner_radius(CornerRadius::same(CARD_RADIUS))
            .inner_margin(egui::Margin::same(6));
        list_card.show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::ScrollArea::vertical()
                .max_height(list_height)
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
                .show(ui, |ui| {
                    let count = self.candidates.len();
                    for (i, app) in self.candidates.iter().enumerate() {
                        let is_selected = current_id == Some(app.app_id);
                        let label = format!("{}   ·   AppID {}", app.name, app.app_id);
                        if Self::candidate_row(ui, &label, is_selected).clicked() {
                            chosen = Some(app.clone());
                        }
                        if i + 1 < count {
                            ui.add_space(2.0);
                        }
                    }
                });
        });

        if let Some(app) = chosen {
            self.manual_id_input = app.app_id.to_string();
            self.manual_id_error = None;
            self.selected_match_name = Some((app.app_id, app.name));
        }

        ui.add_space(10.0);

        Self::field_label(ui, "APP ID");
        let response = Self::text_field(
            ui,
            &mut self.manual_id_input,
            "",
            ui.available_width(),
        );
        if response.changed() {
            self.manual_id_error = None;
            let still_matches_selected = match (
                self.manual_id_input.trim().parse::<u32>().ok(),
                &self.selected_match_name,
            ) {
                (Some(id), Some((selected_id, _))) => id == *selected_id,
                _ => false,
            };
            if !still_matches_selected {
                self.selected_match_name = None;
            }
        }
        if let Some(err) = &self.manual_id_error {
            ui.add_space(3.0);
            ui.colored_label(colors::ERROR, err);
        }
        ui.add_space(8.0);

        let mut apply_clicked = false;
        let mut cancel_clicked = false;
        ui.columns(2, |columns| {
            if Self::primary_button(&mut columns[0], "Apply").clicked() {
                apply_clicked = true;
            }
            if Self::secondary_button(&mut columns[1], "Cancel").clicked() {
                cancel_clicked = true;
            }
        });

        if apply_clicked {
            match self.manual_id_input.trim().parse::<u32>() {
                Ok(app_id) => {
                    let name = match &self.selected_match_name {
                        Some((selected_id, name)) if *selected_id == app_id => name.clone(),
                        _ => steam::get_app_name(app_id)
                            .ok()
                            .flatten()
                            .unwrap_or_else(|| format!("App {app_id}")),
                    };
                    self.start_apply(app_id, name);
                }
                Err(_) => {
                    self.manual_id_error =
                        Some("Please enter a valid numeric App ID.".to_string());
                }
            }
        } else if cancel_clicked {
            self.reset();
        }
    }

    fn draw_manual_id(&mut self, ui: &mut egui::Ui) {
        ui.add(
            egui::Label::new(
                RichText::new("Couldn't find a matching Steam App ID automatically.")
                    .color(colors::TEXT_MUTED)
                    .size(12.0),
            )
            .selectable(false),
        );
        ui.add_space(8.0);
        Self::field_label(ui, "APP ID");
        Self::text_field(ui, &mut self.manual_id_input, "", ui.available_width());
        if let Some(err) = &self.manual_id_error {
            ui.add_space(3.0);
            ui.colored_label(colors::ERROR, err);
        }
        ui.add_space(8.0);

        let mut apply_id: Option<u32> = None;
        let mut cancelled = false;
        ui.columns(2, |columns| {
            if Self::primary_button(&mut columns[0], "Apply").clicked() {
                match self.manual_id_input.trim().parse::<u32>() {
                    Ok(app_id) => apply_id = Some(app_id),
                    Err(_) => {
                        self.manual_id_error =
                            Some("Please enter a valid numeric App ID.".to_string());
                    }
                }
            }
            if Self::secondary_button(&mut columns[1], "Cancel").clicked() {
                cancelled = true;
            }
        });

        if let Some(app_id) = apply_id {
            let name = steam::get_app_name(app_id)
                .ok()
                .flatten()
                .unwrap_or_else(|| format!("App {app_id}"));
            self.start_apply(app_id, name);
        } else if cancelled {
            self.reset();
        }
    }

    fn draw_workshop_dl(&mut self, ui: &mut egui::Ui) {
        Self::field_label(ui, "WORKSHOP URL");
        let list_btn_w = 56.0;
        let gap = 6.0;
        let field_w = (ui.available_width() - list_btn_w - gap).max(80.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            Self::text_field(
                ui,
                &mut self.workshop_url_input,
                "Paste URL/IDs here or into the window…",
                field_w,
            );
            if Self::paint_button(
                ui,
                "List",
                colors::SURFACE,
                colors::SURFACE_HOVER,
                Stroke::new(1.0, colors::PANEL_BORDER),
                colors::TEXT_PRIMARY,
                INPUT_HEIGHT,
                list_btn_w,
                12.0,
            )
            .on_hover_text("Paste a list of URLs or IDs")
            .clicked()
            {
                self.import_list_overlay_open = true;
                self.import_list_error = None;
            }
        });
        // After the URL field so we can clear multiline junk TextEdit already inserted.
        self.handle_workshop_window_paste(ui);
        if let Some(err) = &self.workshop_url_error {
            ui.add_space(3.0);
            ui.colored_label(colors::ERROR, err);
        }
        ui.add_space(8.0);

        let mut download_clicked = false;
        let mut queue_clicked = false;
        ui.columns(2, |columns| {
            if Self::primary_button(&mut columns[0], "Direct Download").clicked() {
                download_clicked = true;
            }
            if Self::secondary_button(&mut columns[1], "Add to Queue").clicked() {
                queue_clicked = true;
            }
        });

        if download_clicked {
            self.enqueue_workshop_url(true);
        } else if queue_clicked {
            self.enqueue_workshop_url(false);
        }

        ui.add_space(10.0);
        ui.label(
            RichText::new("DOWNLOAD QUEUE")
                .color(colors::TEXT_MUTED)
                .size(10.0),
        );
        ui.add_space(4.0);

        // Sticky bottom button: pin to content bottom, table fills the gap above.
        let bottom_bar = CONTROL_HEIGHT + 4.0;
        let remaining = ui.available_rect_before_wrap();
        let button_rect = egui::Rect::from_min_max(
            egui::pos2(remaining.left(), remaining.bottom() - bottom_bar),
            remaining.right_bottom(),
        );
        let table_rect = egui::Rect::from_min_max(
            remaining.min,
            egui::pos2(remaining.right(), button_rect.top() - 8.0),
        );

        ui.scope_builder(egui::UiBuilder::new().max_rect(table_rect), |ui| {
            let queue_card = egui::Frame::new()
                .fill(colors::SURFACE)
                .stroke(Stroke::new(1.0, colors::PANEL_BORDER))
                .corner_radius(CornerRadius::same(CARD_RADIUS))
                .inner_margin(egui::Margin::same(6));

            queue_card.show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_min_height(ui.available_height());

                let body_height = (ui.available_height() - 22.0).max(QUEUE_ROW_HEIGHT);
                let skeleton_slots =
                    ((body_height / QUEUE_ROW_HEIGHT).floor() as usize).max(3);

                egui::ScrollArea::vertical()
                    .max_height(ui.available_height())
                    .auto_shrink([false, false])
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
                    .show(ui, |ui| {
                        ui.set_min_width(queue_cols::TOTAL);
                        Self::queue_table_header(ui);

                        let mut remove_index: Option<usize> = None;
                        for (index, item) in self.download_queue.iter().enumerate() {
                            if let Some(idx) =
                                Self::queue_table_row(ui, item, index, index % 2 == 1)
                            {
                                remove_index = Some(idx);
                            }
                        }

                        // Fill remaining rows with a skeleton so the table always
                        // reads as a full grid, even when empty or short.
                        let filled = self.download_queue.len();
                        let fill_count = skeleton_slots.saturating_sub(filled);
                        for i in 0..fill_count {
                            Self::queue_skeleton_row(ui, filled + i);
                        }

                        if let Some(index) = remove_index {
                            self.remove_queue_item(index);
                        }
                    });
            });
        });

        let can_start = self.has_queued_items() && !self.queue_processing;
        let can_clear = self
            .download_queue
            .iter()
            .any(|item| item.status != QueueStatus::Downloading);
        let mut clear_clicked = false;
        let mut download_clicked = false;
        ui.scope_builder(egui::UiBuilder::new().max_rect(button_rect), |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let total = ui.available_width();
                let clear_w = ((total - 6.0) * 0.30).max(72.0);
                let download_w = (total - 6.0 - clear_w).max(120.0);

                ui.allocate_ui(egui::vec2(download_w, CONTROL_HEIGHT), |ui| {
                    ui.add_enabled_ui(can_start, |ui| {
                        if Self::primary_button(ui, "Download Queue").clicked() {
                            download_clicked = true;
                        }
                    });
                });
                ui.allocate_ui(egui::vec2(clear_w, CONTROL_HEIGHT), |ui| {
                    ui.add_enabled_ui(can_clear, |ui| {
                        if Self::secondary_button(ui, "Clear").clicked() {
                            clear_clicked = true;
                        }
                    });
                });
            });
        });
        if clear_clicked {
            self.clear_download_queue();
        } else if download_clicked {
            self.try_process_queue();
        }
        // Consume the remaining layout space so later widgets don't overlap.
        ui.advance_cursor_after_rect(remaining);
    }

    /// Overlay for pasting a multi-line list of workshop URLs / IDs.
    fn draw_import_list_overlay(&mut self, ui: &mut egui::Ui, app_rect: egui::Rect) {
        let backdrop = ui.interact(
            app_rect,
            egui::Id::new("import_list_overlay_backdrop"),
            egui::Sense::click(),
        );
        ui.painter().rect_filled(
            app_rect,
            CornerRadius::same(WINDOW_CORNER_RADIUS),
            Color32::from_black_alpha(180),
        );

        let card_width = 360.0;
        let outer_pad = 16.0;
        let max_card_h = (app_rect.height() - outer_pad * 2.0).max(220.0);
        let chrome_h = 12.0 * 2.0 // card padding
            + 28.0 // title
            + 4.0
            + 18.0 // hint
            + 8.0
            + CONTROL_HEIGHT // paste button
            + 6.0
            + 34.0 // action buttons
            + 8.0;
        let editor_h = (max_card_h - chrome_h).clamp(100.0, 220.0);
        let card_height = chrome_h + editor_h;
        let card_rect = egui::Rect::from_center_size(
            app_rect.center(),
            egui::vec2(card_width, card_height.min(max_card_h)),
        );

        let mut dismiss = false;
        let mut import_queue = false;
        let mut import_download = false;
        let mut paste_clicked = false;

        let did_paste = ui.ctx().input(|i| {
            i.events
                .iter()
                .any(|e| matches!(e, egui::Event::Paste(_)))
        });

        egui::Area::new(egui::Id::new("import_list_overlay_card"))
            .fixed_pos(card_rect.min)
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::new()
                    .fill(colors::PANEL_BG)
                    .stroke(Stroke::new(1.0, colors::ACCENT.linear_multiply(0.45)))
                    .corner_radius(CornerRadius::same(CARD_RADIUS))
                    .inner_margin(egui::Margin::same(12))
                    .show(ui, |ui| {
                        ui.set_width(card_width - 24.0);
                        ui.set_max_height(card_height - 24.0);

                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("Import workshop list")
                                    .color(colors::TEXT_PRIMARY)
                                    .size(12.0),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if Self::icon_button(ui, "✕", colors::TEXT_MUTED).clicked() {
                                        dismiss = true;
                                    }
                                },
                            );
                        });
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new("Paste URLs or IDs — only IDs are kept in the list.")
                                .color(colors::TEXT_MUTED)
                                .size(11.0),
                        );
                        ui.add_space(8.0);

                        if Self::secondary_button(ui, "Paste from clipboard").clicked() {
                            paste_clicked = true;
                        }
                        ui.add_space(6.0);

                        // Fixed-height editor: long lists scroll here, buttons stay on screen.
                        egui::Frame::new()
                            .fill(colors::INPUT_BG)
                            .stroke(Stroke::new(1.0, colors::INPUT_BORDER))
                            .corner_radius(CornerRadius::same(CONTROL_RADIUS))
                            .inner_margin(egui::Margin::symmetric(8, 6))
                            .show(ui, |ui| {
                                let w = ui.available_width();
                                let inner_h = (editor_h - 12.0).max(80.0);
                                egui::ScrollArea::vertical()
                                    .id_salt("import_list_editor_scroll")
                                    .max_height(inner_h)
                                    .min_scrolled_height(inner_h)
                                    .auto_shrink([false, false])
                                    .scroll_bar_visibility(
                                        egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded,
                                    )
                                    .show(ui, |ui| {
                                        ui.add(
                                            egui::TextEdit::multiline(
                                                &mut self.import_list_input,
                                            )
                                            .desired_width(w)
                                            .frame(egui::Frame::NONE)
                                            .hint_text("3765697723\n1710929351\n…"),
                                        );
                                    });
                            });

                        if did_paste {
                            Self::normalize_import_list_field(&mut self.import_list_input);
                        }

                        if let Some(err) = &self.import_list_error {
                            ui.add_space(4.0);
                            ui.colored_label(colors::ERROR, err);
                        }

                        ui.add_space(8.0);
                        ui.columns(2, |columns| {
                            if Self::primary_button(&mut columns[0], "Import & Download").clicked()
                            {
                                import_download = true;
                            }
                            if Self::secondary_button(&mut columns[1], "Add to Queue").clicked() {
                                import_queue = true;
                            }
                        });
                    });
            });

        if paste_clicked {
            self.paste_import_list_from_clipboard();
        } else if import_download {
            self.import_workshop_list(true);
        } else if import_queue {
            self.import_workshop_list(false);
        } else if dismiss || backdrop.clicked() {
            self.import_list_overlay_open = false;
            self.import_list_error = None;
        }
    }

    /// Overlay listing failed downloads with GG links and icon actions.
    fn draw_failed_overlay(&mut self, ui: &mut egui::Ui, app_rect: egui::Rect) {
        let failed: Vec<(String, String, u64)> = self
            .download_queue
            .iter()
            .filter(|item| matches!(item.status, QueueStatus::Failed(_)))
            .map(|item| {
                (
                    item.mod_name.clone(),
                    item.game_name.clone(),
                    item.workshop_id,
                )
            })
            .collect();

        if failed.is_empty() {
            self.failed_overlay_open = false;
            return;
        }

        let backdrop = ui.interact(
            app_rect,
            egui::Id::new("failed_overlay_backdrop"),
            egui::Sense::click(),
        );
        ui.painter().rect_filled(
            app_rect,
            CornerRadius::same(WINDOW_CORNER_RADIUS),
            Color32::from_black_alpha(180),
        );

        let show_steamcmd_retry =
            self.settings.use_steamcmd && !self.steamcmd_exhausted;
        let show_exhausted = self.steamcmd_exhausted;
        let card_width = fail_cols::TOTAL + 40.0;
        // Grow with failed count; use almost the full window before scrolling.
        let outer_pad = 16.0;
        let max_card_h = (app_rect.height() - outer_pad * 2.0).max(QUEUE_ROW_HEIGHT * 4.0);
        let footer_h = if show_steamcmd_retry {
            let status_h = if self.pending_steamcmd_retry && self.steamcmd_setup_status.is_some()
            {
                6.0 + 16.0
            } else {
                0.0
            };
            8.0 + status_h + CONTROL_HEIGHT
        } else if show_exhausted {
            8.0 + 32.0 // exhausted message (two lines)
        } else {
            0.0
        };
        let chrome_h = 12.0 * 2.0 // card frame padding
            + 28.0 // title + close
            + 4.0
            + 18.0 // hint line
            + 8.0
            + 4.0 * 2.0 // list frame padding
            + 22.0 // table header
            + footer_h;
        let max_rows_h = (max_card_h - chrome_h).max(QUEUE_ROW_HEIGHT * 3.0);
        let rows_h = (failed.len() as f32 * QUEUE_ROW_HEIGHT)
            .clamp(QUEUE_ROW_HEIGHT, max_rows_h);
        let card_height = chrome_h + rows_h;
        let card_rect = egui::Rect::from_center_size(
            app_rect.center(),
            egui::vec2(card_width, card_height),
        );

        let mut dismiss = false;
        let mut retry_steamcmd = false;
        egui::Area::new(egui::Id::new("failed_overlay_card"))
            .fixed_pos(card_rect.min)
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::new()
                    .fill(colors::PANEL_BG)
                    .stroke(Stroke::new(1.0, colors::ERROR.linear_multiply(0.55)))
                    .corner_radius(CornerRadius::same(CARD_RADIUS))
                    .inner_margin(egui::Margin::same(12))
                    .show(ui, |ui| {
                        ui.set_width(card_width - 24.0);

                        // Title + close
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            ui.add(
                                egui::Label::new(
                                    RichText::new("Following items could not be downloaded:")
                                        .color(colors::TEXT_PRIMARY)
                                        .size(12.0),
                                )
                                .truncate(),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if Self::icon_button(ui, "✕", colors::TEXT_MUTED).clicked() {
                                        dismiss = true;
                                    }
                                },
                            );
                        });
                        ui.add_space(4.0);

                        // Hint — one tight line
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 3.0;
                            if show_exhausted {
                                ui.label(
                                    RichText::new(
                                        "None of the download methods worked (GGNetwork + SteamCMD).",
                                    )
                                    .color(colors::ERROR)
                                    .size(11.0),
                                );
                            } else {
                                ui.label(
                                    RichText::new("Try again on")
                                        .color(colors::TEXT_MUTED)
                                        .size(11.0),
                                );
                                if ui
                                    .add(
                                        egui::Button::new(
                                            RichText::new("GGNetwork")
                                                .color(colors::ACCENT)
                                                .strong()
                                                .size(11.0),
                                        )
                                        .frame(false),
                                    )
                                    .on_hover_text("https://ggntw.com/steam")
                                    .clicked()
                                {
                                    ui.ctx().open_url(egui::OpenUrl::new_tab(
                                        "https://ggntw.com/steam",
                                    ));
                                }
                                ui.label(
                                    RichText::new("via")
                                        .color(colors::TEXT_MUTED)
                                        .size(11.0),
                                );
                                ui.label(
                                    RichText::new(ICON_OPEN).color(colors::ACCENT).size(12.0),
                                );
                            }
                        });
                        ui.add_space(8.0);

                        egui::Frame::new()
                            .fill(colors::SURFACE)
                            .stroke(Stroke::new(1.0, colors::PANEL_BORDER))
                            .corner_radius(CornerRadius::same(CONTROL_RADIUS))
                            .inner_margin(egui::Margin::symmetric(6, 4))
                            .show(ui, |ui| {
                                ui.set_width(fail_cols::TOTAL);
                                Self::fail_table_header(ui);

                                let scroll_needed = (failed.len() as f32) * QUEUE_ROW_HEIGHT > rows_h + 0.5;
                                let scroll = egui::ScrollArea::vertical()
                                    .min_scrolled_height(rows_h)
                                    .max_height(rows_h)
                                    .auto_shrink([false, false]);
                                let scroll = if scroll_needed {
                                    scroll.scroll_bar_visibility(
                                        egui::scroll_area::ScrollBarVisibility::AlwaysVisible,
                                    )
                                } else {
                                    scroll.scroll_bar_visibility(
                                        egui::scroll_area::ScrollBarVisibility::AlwaysHidden,
                                    )
                                };

                                scroll.show(ui, |ui| {
                                    ui.set_width(fail_cols::TOTAL);
                                    ui.spacing_mut().item_spacing.y = 0.0;
                                    for (mod_name, game_name, workshop_id) in &failed {
                                        let page_url = workshop::gg_page_url(*workshop_id);
                                        Self::fail_table_row(
                                            ui,
                                            mod_name,
                                            game_name,
                                            &page_url,
                                        );
                                    }
                                });
                            });

                        if show_steamcmd_retry {
                            ui.add_space(8.0);
                            // Only show install progress here — not Settings notices like "removed".
                            if self.pending_steamcmd_retry {
                                if let Some(status) = &self.steamcmd_setup_status {
                                    ui.label(
                                        RichText::new(status)
                                            .color(if status.contains("failed") {
                                                colors::ERROR
                                            } else {
                                                colors::TEXT_MUTED
                                            })
                                            .size(11.0),
                                    );
                                    ui.add_space(6.0);
                                }
                            }
                            let ensuring = self.pending_steamcmd_retry;
                            ui.add_enabled_ui(!ensuring, |ui| {
                                if Self::primary_button(ui, "Retry with SteamCMD").clicked() {
                                    retry_steamcmd = true;
                                }
                            });
                        }
                    });
            });

        if retry_steamcmd {
            self.retry_failed_with_steamcmd();
        } else if dismiss || backdrop.clicked() {
            self.dismiss_failed_overlay();
        }
    }

    fn icon_button(ui: &mut egui::Ui, icon: &str, color: Color32) -> egui::Response {
        Self::paint_button(
            ui,
            icon,
            Color32::TRANSPARENT,
            colors::SURFACE_HOVER,
            Stroke::NONE,
            color,
            24.0,
            24.0,
            13.0,
        )
    }
}
