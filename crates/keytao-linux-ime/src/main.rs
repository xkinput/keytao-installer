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

        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            tracing::info!("display server: Wayland");
            wayland_backend::run(engine);
        } else if std::env::var_os("DISPLAY").is_some() {
            tracing::info!("display server: X11");
            x11_backend::run(engine);
        } else {
            eprintln!("Neither WAYLAND_DISPLAY nor DISPLAY is set.");
            std::process::exit(1);
        }
    }
}
