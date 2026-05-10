//! Entry point: detect display server, deploy librime, launch correct backend.

#[cfg(target_os = "linux")]
mod engine;
#[cfg(target_os = "linux")]
mod ibus_backend;
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
            // Wayland session with XWayland: run all three backends concurrently.
            // - zwp_input_method_v2 for native Wayland apps
            // - XIM for XWayland apps that use X11 input methods
            // - IBus D-Bus for Chromium/CEF apps (e.g. WeChatAppEx) that use IBus
            tracing::info!("display server: Wayland + XWayland — running all backends");
            let engine_xim = engine.clone();
            std::thread::spawn(move || {
                tracing::info!("X11/XIM backend started for XWayland apps");
                x11_backend::run(engine_xim);
                tracing::warn!("X11/XIM backend exited");
            });
            let engine_ibus = engine.clone();
            std::thread::spawn(move || {
                tracing::info!("IBus D-Bus backend started for Chromium/CEF apps");
                tokio::runtime::Runtime::new()
                    .expect("tokio runtime")
                    .block_on(ibus_backend::run(engine_ibus));
                tracing::warn!("IBus D-Bus backend exited");
            });
            wayland_backend::run(engine);
        } else if has_wayland {
            tracing::info!("display server: Wayland");
            wayland_backend::run(engine);
        } else if has_x11 {
            tracing::info!("display server: X11");
            // Run IBus backend alongside XIM for X11-only sessions.
            let engine_ibus = engine.clone();
            std::thread::spawn(move || {
                tracing::info!("IBus D-Bus backend started for Chromium/CEF apps");
                tokio::runtime::Runtime::new()
                    .expect("tokio runtime")
                    .block_on(ibus_backend::run(engine_ibus));
                tracing::warn!("IBus D-Bus backend exited");
            });
            x11_backend::run(engine);
        } else {
            eprintln!("Neither WAYLAND_DISPLAY nor DISPLAY is set.");
            std::process::exit(1);
        }
    }
}
