#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod archive;
mod autostart;
mod emulator;
mod games;
mod goldberg;
mod greenluma;
mod logging;
mod models;
mod sendto;
mod settings;
mod single_instance;
mod steam;
mod steamcmd;
mod tray;
mod workshop;

use app::GoldbergDropApp;
use single_instance::{Claim, IpcCommand};
use std::path::PathBuf;

/// Square side length of the custom window, in logical points.
pub const WINDOW_SIZE: f32 = 462.0;

/// The app icon (mountain + "GBD"), baked in at compile time. Also embedded
/// as the exe's native icon via `build.rs`/`winresource` — this copy is used
/// for the runtime window/taskbar icon and the in-app title bar logo.
pub const APP_ICON_PNG: &[u8] = include_bytes!("../assets/app_icon_128.png");

fn main() -> eframe::Result<()> {
    // Run-key / Send-to launches often start with cwd = System32 — use exe dir.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let _ = std::env::set_current_dir(dir);
        }
    }

    logging::init();

    let mut start_in_tray = false;
    let mut open_greenluma = false;
    let mut initial_path: Option<PathBuf> = None;
    for arg in std::env::args().skip(1) {
        if arg == "--tray" || arg == "-tray" {
            start_in_tray = true;
        } else if arg == "--greenluma" || arg == "-greenluma" {
            open_greenluma = true;
        } else if !arg.starts_with('-') && initial_path.is_none() {
            initial_path = Some(PathBuf::from(arg));
        }
    }

    log::info!(
        "parsed args: tray={start_in_tray} greenluma={open_greenluma} path={initial_path:?}"
    );

    let ipc = IpcCommand {
        greenluma: open_greenluma,
        path: initial_path.clone(),
        show: !start_in_tray || initial_path.is_some() || open_greenluma,
    };
    let quiet_tray = start_in_tray && initial_path.is_none() && !open_greenluma;
    let instance_guard = match single_instance::claim_or_forward(&ipc, quiet_tray) {
        Claim::Primary(guard) => {
            log::info!("single-instance: primary");
            guard
        }
        Claim::Secondary => {
            log::info!("single-instance: secondary (forwarded or quiet exit)");
            return Ok(());
        }
    };

    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_inner_size([WINDOW_SIZE, WINDOW_SIZE])
        .with_min_inner_size([WINDOW_SIZE, WINDOW_SIZE])
        .with_max_inner_size([WINDOW_SIZE, WINDOW_SIZE])
        .with_resizable(false)
        .with_decorations(false)
        .with_transparent(true)
        // A native drop shadow is drawn as a hard rectangle around the
        // *actual* (square) window bounds, which shows up past our
        // rounded panel's corners. We fake a softer shadow ourselves
        // instead, so the OS one must stay off.
        .with_has_shadow(false)
        .with_title("GoldbergDrop")
        .with_visible(!start_in_tray);
    if let Some(icon) = load_app_icon() {
        viewport = viewport.with_icon(icon);
    }

    let options = eframe::NativeOptions {
        viewport,
        // eframe's default `wgpu` renderer has long-standing, still-open
        // bugs on Windows where a "transparent" window actually renders
        // opaque black (see emilk/egui#4451) — which is exactly what showed
        // up as solid corners around our rounded panel. The `glow` (OpenGL)
        // renderer's transparency works correctly on Windows, so we use it
        // explicitly instead of relying on the default.
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };

    eframe::run_native(
        "GoldbergDrop",
        options,
        Box::new(move |cc| {
            GoldbergDropApp::apply_style(&cc.egui_ctx);
            install_symbol_fallback_font(&cc.egui_ctx);
            let boot_hidden = start_in_tray && initial_path.is_none();
            let mut app = GoldbergDropApp::new(initial_path, start_in_tray, open_greenluma);
            app.set_instance_guard(instance_guard);
            if boot_hidden {
                // eframe forces the window visible after the first paint; hide now
                // and keep enforcing in `logic()` while dormant.
                cc.egui_ctx
                    .send_viewport_cmd(eframe::egui::ViewportCommand::Visible(false));
            }
            Ok(Box::new(app))
        }),
    )
}

/// Egui's bundled default font doesn't cover many Unicode symbols (arrows,
/// dashes, dingbats, ...) — they silently render as blank "tofu" boxes
/// instead of falling back to anything. We register Windows' own "Segoe UI
/// Symbol" (present on every Windows 10/11 install) as a lowest-priority
/// fallback, so any glyph missing from the primary font (like the "→" in
/// our own messages, or "✕"/"–" in the title bar) still renders correctly,
/// while normal text keeps using egui's default font.
fn install_symbol_fallback_font(ctx: &eframe::egui::Context) {
    use eframe::egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};
    use eframe::egui::{FontData, FontFamily};

    let candidates = [
        ("system-symbol-fallback", r"C:\Windows\Fonts\seguisym.ttf"),
        ("system-emoji-fallback", r"C:\Windows\Fonts\seguiemj.ttf"),
        ("system-uni-fallback", r"C:\Windows\Fonts\arialuni.ttf"),
    ];

    // Register every font we can find, not just the first — Segoe UI Symbol
    // and Segoe UI Emoji cover different glyph ranges (e.g. dingbats vs.
    // trademark/copyright signs), so both are needed as fallbacks.
    for (name, path) in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            ctx.add_font(FontInsert::new(
                name,
                FontData::from_owned(bytes),
                vec![
                    InsertFontFamily {
                        family: FontFamily::Proportional,
                        priority: FontPriority::Lowest,
                    },
                    InsertFontFamily {
                        family: FontFamily::Monospace,
                        priority: FontPriority::Lowest,
                    },
                ],
            ));
        }
    }
}

/// Decodes the embedded app icon into the raw RGBA buffer eframe needs for
/// the OS window/taskbar icon.
fn load_app_icon() -> Option<eframe::egui::IconData> {
    let image = image::load_from_memory(APP_ICON_PNG).ok()?.into_rgba8();
    let (width, height) = image.dimensions();
    Some(eframe::egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    })
}
