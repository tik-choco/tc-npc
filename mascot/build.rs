// Standard Tauri build script: embeds the Windows .ico resource into the
// exe (so `app.default_window_icon()` has something to hand the tray icon
// at runtime -- see src/main.rs's `setup_tray`) and compiles the permissions
// ACL from tauri.conf.json + capabilities/*.json into the binary. Every
// Tauri 2 application needs exactly this; mirrors
// tc-assistant2/src-tauri/build.rs, which already builds cleanly here.
fn main() {
    tauri_build::build()
}
