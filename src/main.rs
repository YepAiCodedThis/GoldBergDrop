#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod emulator;
mod goldberg;
mod models;
mod sendto;
mod steam;

use app::GoldbergDropApp;
use std::path::PathBuf;

/// Square side length of the custom window, in logical points.
pub const WINDOW_SIZE: f32 = 420.0;

/// The app icon (mountain + "GBD"), baked in at compile time. Also embedded
/// as the exe's native icon via `build.rs`/`winresource` — this copy is used
/// for the runtime window/taskbar icon and the in-app title bar logo.
pub const APP_ICON_PNG: &[u8] = include_bytes!("../assets/app_icon_128.png");

fn main() -> eframe::Result<()> {
    // When launched via the Windows "Send to" shortcut, Explorer passes the
    // right-clicked file's path as the first argument.
    let initial_path: Option<PathBuf> = std::env::args().nth(1).map(PathBuf::from);

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
        .with_title("GoldbergDrop");
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
        Box::new(|cc| {
            cc.egui_ctx.set_visuals(GoldbergDropApp::build_visuals());
            install_symbol_fallback_font(&cc.egui_ctx);
            Ok(Box::new(GoldbergDropApp::new(initial_path)))
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
