// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(target_os = "linux")]
    if let Ok(desktop) = std::env::var("XDG_CURRENT_DESKTOP") {
        if desktop.to_lowercase().contains("kde") {
            // Use native Wayland rendering for perfect hover styles, but force IBus
            // for text input because WebKitGTK Wayland IM module is horribly broken on KDE.
            std::env::set_var("GTK_IM_MODULE", "ibus");
        }
    }
    keytao_app_lib::run()
}
