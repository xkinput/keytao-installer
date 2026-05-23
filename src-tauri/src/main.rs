// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(target_os = "linux")]
    if let Ok(desktop) = std::env::var("XDG_CURRENT_DESKTOP") {
        if desktop.to_lowercase().contains("kde") {
            if std::env::var_os("GTK_IM_MODULE").is_none() {
                std::env::set_var("GTK_IM_MODULE", "ibus");
            }
            if std::env::var_os("QT_IM_MODULE").is_none() {
                std::env::set_var("QT_IM_MODULE", "ibus");
            }
        }
    }
    keytao_app_lib::run()
}
