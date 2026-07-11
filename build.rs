fn main() {
    // Embeds assets/app_icon.ico as the compiled exe's icon (shown in
    // Explorer, the taskbar, Alt-Tab, and inherited by any shortcut that
    // points at the exe — including the "Send to" shortcut).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("assets/app_icon.ico")
            .compile()
            .expect("failed to embed the Windows exe icon");
    }
}
