use crate::goldberg::{self, SetupOptions};
use crate::models::{DlcApp, SteamApp};
use crate::sendto::{self, SendToStatus};
use crate::{emulator, steam};
use eframe::egui::{self, Color32, CornerRadius, RichText, Stroke};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};

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
}

const TITLE_BAR_HEIGHT: f32 = 36.0;
const CONTENT_MARGIN: f32 = 18.0;
const WINDOW_CORNER_RADIUS: u8 = 20;
/// Corner radius for card-level containers (drop zone, list group).
const CARD_RADIUS: u8 = 14;
/// Corner radius for buttons, inputs, and list rows.
const CONTROL_RADIUS: u8 = 8;

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
    },
    ApplyFailed(String),
}

pub struct GoldbergDropApp {
    exe_path: Option<PathBuf>,
    fetch_dlc: bool,

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

    tx: Sender<WorkerMsg>,
    rx: Receiver<WorkerMsg>,
}

impl Default for GoldbergDropApp {
    fn default() -> Self {
        Self::new(None)
    }
}

impl GoldbergDropApp {
    pub fn new(initial_path: Option<PathBuf>) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let sendto_status = sendto::status();
        let mut app = Self {
            exe_path: None,
            fetch_dlc: true,
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
            tx,
            rx,
        };
        // Launched via the "Send to" shortcut (or a file passed on the
        // command line) — start straight away instead of waiting for a drop.
        if let Some(path) = initial_path {
            app.accept_path(path);
        }
        app
    }

    /// Custom dark/gold visuals applied once at startup.
    pub fn build_visuals() -> egui::Visuals {
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = colors::PANEL_BG;
        visuals.window_fill = colors::PANEL_BG;
        visuals.extreme_bg_color = colors::SURFACE;
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
        w.hovered.bg_stroke = Stroke::new(1.0, colors::ACCENT);
        w.hovered.fg_stroke = Stroke::new(1.0, colors::ACCENT_HOVER);
        w.hovered.corner_radius = CornerRadius::same(CONTROL_RADIUS);

        w.active.bg_fill = colors::ACCENT;
        w.active.weak_bg_fill = colors::ACCENT;
        w.active.bg_stroke = Stroke::new(1.0, colors::ACCENT_ACTIVE);
        w.active.fg_stroke = Stroke::new(1.0, colors::PANEL_BG);
        w.active.corner_radius = CornerRadius::same(CONTROL_RADIUS);

        w.open.bg_fill = colors::ACCENT_ACTIVE;
        w.open.weak_bg_fill = colors::ACCENT_ACTIVE;
        w.open.bg_stroke = Stroke::new(1.0, colors::ACCENT);
        w.open.fg_stroke = Stroke::new(1.0, colors::TEXT_PRIMARY);

        visuals
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
        let tx = self.tx.clone();

        std::thread::spawn(move || {
            let result = (|| -> anyhow::Result<(bool, usize)> {
                let cache_dir = emulator::ensure_goldberg_available()?;

                let dlc_list: Vec<DlcApp> = if fetch_dlc {
                    steam::get_dlc_list(app_id).unwrap_or_default()
                } else {
                    Vec::new()
                };
                let dlc_count = dlc_list.len();

                let swapped = goldberg::apply_setup(
                    &game_dir,
                    &cache_dir,
                    &SetupOptions { app_id, dlc_list },
                )?;

                Ok((swapped, dlc_count))
            })();

            let msg = match result {
                Ok((dll_swapped, dlc_count)) => WorkerMsg::ApplyDone {
                    app_id,
                    name,
                    dll_swapped,
                    dlc_count,
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
                    self.result_message = format!(
                        "Done! \"{name}\" (AppID {app_id}) is set up.\n{dll_msg}{dlc_msg}"
                    );
                    self.screen = Screen::Done;
                }
                WorkerMsg::ApplyFailed(e) => {
                    self.result_message = format!("Setup failed: {e}");
                    self.screen = Screen::Error;
                }
            }
        }
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
}

impl eframe::App for GoldbergDropApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        Color32::TRANSPARENT.to_normalized_gamma_f32()
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_worker();
        let ctx = ui.ctx().clone();
        self.handle_dropped_files(&ctx);

        // Repaint periodically while a background task is running so the
        // UI picks up worker messages promptly.
        if self.screen == Screen::Working {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }

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

                    let content_rect = egui::Rect::from_min_max(
                        egui::pos2(app_rect.min.x, title_bar_rect.max.y),
                        app_rect.max,
                    )
                    .shrink2(egui::vec2(CONTENT_MARGIN, CONTENT_MARGIN * 0.7));

                    let mut content_ui = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(content_rect)
                            .layout(egui::Layout::top_down(egui::Align::Min)),
                    );
                    self.draw_content(&ctx, &mut content_ui);
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
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
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

    /// The single visually "loud" action on a screen — filled with the
    /// accent color, dark text, stretched to the available width. Used for
    /// Apply / Set up another game / etc.
    fn primary_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
        let width = ui.available_width();
        ui.add(
            egui::Button::new(RichText::new(label).color(colors::PANEL_BG).strong().size(13.0))
                .fill(colors::ACCENT)
                .stroke(Stroke::NONE)
                .corner_radius(CONTROL_RADIUS)
                .min_size(egui::vec2(width, 30.0)),
        )
    }

    /// A quiet, outlined action next to a primary button (e.g. Cancel).
    fn secondary_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
        let width = ui.available_width();
        ui.add(
            egui::Button::new(RichText::new(label).color(colors::TEXT_MUTED).size(13.0))
                .fill(Color32::TRANSPARENT)
                .stroke(Stroke::new(1.0, colors::PANEL_BORDER))
                .corner_radius(CONTROL_RADIUS)
                .min_size(egui::vec2(width, 30.0)),
        )
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

    /// A borderless, accent-colored text link (Browse..., Refresh).
    fn ghost_accent_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
        ui.add(
            egui::Button::new(RichText::new(label).color(colors::ACCENT).size(12.5))
                .frame(false)
                .corner_radius(CONTROL_RADIUS),
        )
    }

    fn draw_content(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
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
            ui.checkbox(&mut self.fetch_dlc, "Fetch DLCs");
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
            ui.horizontal(|ui| {
                ui.colored_label(colors::ERROR, "GoldbergDrop was moved.");
                if Self::ghost_accent_button(ui, "Fix \"Send to\"").clicked() {
                    self.refresh_sendto();
                }
            });
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

        ui.add(
            egui::Label::new(
                RichText::new("APP ID")
                    .color(colors::TEXT_MUTED)
                    .size(10.5),
            )
            .selectable(false),
        );
        ui.add_space(3.0);
        let response = ui.add_sized(
            [ui.available_width(), 30.0],
            egui::TextEdit::singleline(&mut self.manual_id_input),
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
        ui.add(
            egui::Label::new(
                RichText::new("APP ID")
                    .color(colors::TEXT_MUTED)
                    .size(10.5),
            )
            .selectable(false),
        );
        ui.add_space(3.0);
        ui.add_sized(
            [ui.available_width(), 30.0],
            egui::TextEdit::singleline(&mut self.manual_id_input),
        );
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
}
