// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(target_os = "linux")]
    if let Ok(desktop) = std::env::var("XDG_CURRENT_DESKTOP") {
        if desktop.to_lowercase().contains("kde") {
            // Use native Wayland rendering for perfect hover styles, but force IBus
            // for text input because WebKitGTK Wayland IM module is horribly broken on KDE.
            std::env::set_var("GTK_IM_MODULE", "ibus");
            
            // Fix race condition: GTK initializes IBus immediately when `run()` is called.
            // If the local daemon isn't running yet, GTK fails to connect to D-Bus and never retries.
            // So we spawn the local daemon BEFORE GTK initializes, and give it 1 second to start.
            if std::env::var("KWIN_VIRTUAL_KEYBOARD").is_err() {
                if !std::process::Command::new("pgrep").arg("-x").arg("keytao-ime").status().map(|s| s.success()).unwrap_or(false) {
                    println!("Starting local keytao-ime daemon before GTK initializes...");
                    std::process::Command::new("cargo")
                        .args(["run", "-p", "keytao-linux-ime"])
                        .spawn()
                        .ok();
                    std::thread::sleep(std::time::Duration::from_millis(1500));
                }
            }
        }
    }
    keytao_app_lib::run()
}
