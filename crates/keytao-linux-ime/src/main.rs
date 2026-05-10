//! Entry point: detect display server, deploy librime, launch correct backend.

#[cfg(target_os = "linux")]
mod engine;
#[cfg(target_os = "linux")]
mod panel;
#[cfg(target_os = "linux")]
mod wayland_backend;
#[cfg(target_os = "linux")]
mod x11_backend;

fn main() {
    if std::env::args().any(|a| a == "--version") {
        println!("keytao-ime {}", env!("CARGO_PKG_VERSION"));
        println!("librime {}", env!("RIME_VERSION"));
        return;
    }

    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("keytao-ime: this binary only runs on Linux.");
        std::process::exit(1);
    }

    #[cfg(target_os = "linux")]
    {
        use tracing_subscriber::EnvFilter;
        tracing_subscriber::fmt()
            .with_env_filter(
                EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
            )
            .init();

        let engine = engine::CoreEngine::new();
        if let Err(e) = engine.init() {
            tracing::error!("librime init failed: {e}");
            std::process::exit(1);
        }
        tracing::info!("librime ready");

        let has_wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
        let has_x11 = std::env::var_os("DISPLAY").is_some();

        if has_wayland && has_x11 {
            // Wayland session with XWayland: run both backends concurrently.
            // The X11/XIM backend serves XWayland apps (e.g. WeChat) that cannot
            // use zwp_input_method_v2 because they fall back to xcb/XIM.
            // CoreEngine is Arc<Mutex<...>> so it is safe to share across threads.
            tracing::info!("display server: Wayland + XWayland — running both backends");
            let engine_xim = engine.clone();
            std::thread::spawn(move || {
                tracing::info!("X11/XIM backend started for XWayland apps");
                x11_backend::run(engine_xim);
                tracing::warn!("X11/XIM backend exited");
            });
            wayland_backend::run(engine);
        } else if has_wayland {
            tracing::info!("display server: Wayland");
            wayland_backend::run(engine);
        } else if has_x11 {
            tracing::info!("display server: X11");
            x11_backend::run(engine);
        } else {
            eprintln!("Neither WAYLAND_DISPLAY nor DISPLAY is set.");
            std::process::exit(1);
        }
    }
}
