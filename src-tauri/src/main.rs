// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(target_os = "linux")]
    if let Ok(desktop) = std::env::var("XDG_CURRENT_DESKTOP") {
        if desktop.to_lowercase().contains("kde") {
            // WebKitGTK Wayland IM is buggy on KDE, and IBus requires a GTK plugin
            // that may not be present in the dev shell. X11/XIM is built-in and works reliably.
            std::env::set_var("GDK_BACKEND", "x11");
            std::env::set_var("GTK_IM_MODULE", "xim");
            std::env::set_var("QT_IM_MODULE", "xim");
            std::env::set_var("XMODIFIERS", "@im=keytao");
        }
    }
    keytao_app_lib::run()
}
