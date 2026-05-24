//! Entry point: detect display server, deploy librime, launch correct backend.

#[cfg(target_os = "linux")]
mod engine;
#[cfg(target_os = "linux")]
mod gnome_ibus_engine;
#[cfg(target_os = "linux")]
mod ibus_backend;
#[cfg(target_os = "linux")]
mod panel;
#[cfg(target_os = "linux")]
mod tray;
#[cfg(target_os = "linux")]
mod wayland_backend;
#[cfg(target_os = "linux")]
mod wayland_backend_kde;
#[cfg(target_os = "linux")]
mod x11_backend;

#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug, Default)]
struct BackendSelection {
    wayland: bool,
    xim: bool,
    ibus: bool,
    /// Run as a standalone IBus engine that connects to an existing ibus-daemon
    /// (used on GNOME or when launched by ibus-daemon via a component XML).
    ibus_engine: bool,
}

#[cfg(target_os = "linux")]
impl BackendSelection {
    fn from_args(args: &[String]) -> Result<Self, String> {
        let mut selection = Self::default();
        let mut explicit = false;

        for arg in args {
            if arg == "--ibus-engine" {
                // Launched by ibus-daemon as a standalone engine process.
                // Override everything and run in IBus engine mode only.
                return Ok(Self {
                    ibus_engine: true,
                    ..Default::default()
                });
            }
            if let Some(value) = arg.strip_prefix("--backend=") {
                explicit = true;
                selection = Self::parse_list(value)?;
            } else if arg == "--wayland" {
                explicit = true;
                selection.wayland = true;
            } else if arg == "--xim" {
                explicit = true;
                selection.xim = true;
            } else if arg == "--ibus" {
                explicit = true;
                selection.ibus = true;
            }
        }

        if explicit {
            if !selection.any() {
                return Err("no backends selected".into());
            }
            Ok(selection)
        } else {
            Ok(Self::default())
        }
    }

    fn parse_list(value: &str) -> Result<Self, String> {
        let mut selection = Self::default();
        for raw in value.split(',') {
            let item = raw.trim();
            if item.is_empty() {
                continue;
            }
            match item {
                "wayland" => selection.wayland = true,
                "xim" | "x11" => selection.xim = true,
                "ibus" => selection.ibus = true,
                other => return Err(format!("unknown backend '{other}'")),
            }
        }
        if !selection.any() {
            return Err("no backends selected".into());
        }
        Ok(selection)
    }

    fn for_session(has_wayland: bool, has_x11: bool, is_gnome: bool, is_kde: bool) -> Self {
        if is_gnome {
            // GNOME does not support zwp_input_method_manager_v2.
            // Use the IBus engine backend which connects to GNOME's ibus-daemon,
            // plus XIM for any XWayland apps.
            return Self {
                ibus_engine: true,
                xim: has_x11,
                ..Default::default()
            };
        }
        if is_kde {
            // A normal KDE autostart daemon is not the KWin Virtual Keyboard owner,
            // so it must not try the native Wayland input-method path. KWin launches
            // the virtual-keyboard instance separately with WAYLAND_SOCKET.
            return Self {
                ibus: has_wayland || has_x11,
                xim: has_x11,
                ..Default::default()
            };
        }
        match (has_wayland, has_x11) {
            (true, true) => Self {
                wayland: true,
                xim: true,
                ibus: true,
                ..Default::default()
            },
            (true, false) => Self {
                wayland: true,
                ..Default::default()
            },
            (false, true) => Self {
                xim: true,
                ibus: true,
                ..Default::default()
            },
            (false, false) => Self::default(),
        }
    }

    fn any(self) -> bool {
        self.wayland || self.xim || self.ibus || self.ibus_engine
    }

    fn describe(self) -> String {
        let mut parts = Vec::new();
        if self.ibus_engine {
            parts.push("ibus-engine");
        }
        if self.wayland {
            parts.push("wayland");
        }
        if self.xim {
            parts.push("xim");
        }
        if self.ibus {
            parts.push("ibus");
        }
        parts.join(",")
    }
}

#[cfg(target_os = "linux")]
fn remove_legacy_kde_env_file() {
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let env_file = std::path::Path::new(&home)
        .join(".config")
        .join("plasma-workspace")
        .join("env")
        .join("keytao.sh");
    if env_file.exists() {
        match std::fs::remove_file(&env_file) {
            Ok(()) => tracing::info!(
                "Removed legacy KDE IBus env file {} to avoid overriding KWin Virtual Keyboard routing",
                env_file.display()
            ),
            Err(e) => tracing::warn!("Cannot remove KDE env file {}: {e}", env_file.display()),
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "--version") {
        println!("keytao-ime {}", env!("CARGO_PKG_VERSION"));
        println!("librime {}", env!("RIME_VERSION"));
        return;
    }

    #[cfg(target_os = "linux")]
    let requested_backends = match BackendSelection::from_args(&args) {
        Ok(selection) => selection,
        Err(err) => {
            eprintln!("keytao-ime: {err}");
            std::process::exit(2);
        }
    };

    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("keytao-ime: this binary only runs on Linux.");
        std::process::exit(1);
    }

    #[cfg(target_os = "linux")]
    {
        use tracing_subscriber::EnvFilter;
        let file_appender = tracing_appender::rolling::never("/tmp", "keytao-ime.log");
        tracing_subscriber::fmt()
            .with_writer(file_appender)
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

        // KWin launches the configured Virtual Keyboard with a private
        // WAYLAND_SOCKET. On KDE this socket advertises input-method-v1, not the
        // input-method-v2 manager used by wlroots-style compositors.
        let kwin_socket = std::env::var("WAYLAND_SOCKET")
            .ok()
            .and_then(|s| s.parse::<i32>().ok())
            .is_some();

        let has_wayland = kwin_socket || std::env::var_os("WAYLAND_DISPLAY").is_some();
        let has_x11 = std::env::var_os("DISPLAY").is_some();

        let desktop = std::env::var("XDG_CURRENT_DESKTOP")
            .unwrap_or_default()
            .to_lowercase();
        let is_gnome = desktop
            .split(':')
            .any(|s| matches!(s, "gnome" | "unity" | "budgie" | "pantheon" | "x-cinnamon"));
        // When launched via WAYLAND_SOCKET (KWin Virtual Keyboard), do not treat
        // the process as a normal KDE autostart daemon.
        let is_kde = !kwin_socket && desktop.split(':').any(|s| s == "kde");

        // A standalone KDE daemon plus plasma-workspace IBus env overrides
        // conflicts with KWin Virtual Keyboard mode. Clean up legacy files
        // instead of reasserting them.
        if is_kde && !kwin_socket && !requested_backends.any() {
            remove_legacy_kde_env_file();
        }

        let selected = if kwin_socket {
            // Launched by KWin as its registered Virtual Keyboard. The private
            // WAYLAND_SOCKET is used for KDE input-method-v1. We also start IBus
            // and XIM so fallback clients can connect directly.
            BackendSelection {
                wayland: true,
                ibus: true,
                xim: true,
                ..Default::default()
            }
        } else if requested_backends.ibus_engine {
            requested_backends
        } else if requested_backends.any() {
            requested_backends
        } else {
            BackendSelection::for_session(has_wayland, has_x11, is_gnome, is_kde)
        };

        if !selected.any() {
            eprintln!("Neither WAYLAND_DISPLAY nor DISPLAY is set.");
            std::process::exit(1);
        }

        if selected.xim {
            let engine_xim = engine.clone();
            std::thread::spawn(move || {
                tracing::info!("X11/XIM backend started for XWayland apps");
                x11_backend::run(engine_xim);
                tracing::warn!("X11/XIM backend exited");
            });
        }

        if selected.ibus {
            let engine_ibus = engine.clone();
            std::thread::spawn(move || {
                tracing::info!("IBus D-Bus backend started for Chromium/CEF apps");
                tokio::runtime::Runtime::new()
                    .expect("tokio runtime")
                    .block_on(ibus_backend::run(engine_ibus));
                tracing::warn!("IBus D-Bus backend exited");
            });
        }

        tracing::info!(
            "display server: wayland={} x11={} desktop={:?} — selected backends [{}]",
            has_wayland,
            has_x11,
            std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default(),
            selected.describe(),
        );

        tray::spawn_tray();

        if selected.ibus_engine {
            // Run as an IBus engine connecting to the existing ibus-daemon.
            // This is the correct path for GNOME Wayland.
            tracing::info!(
                "GNOME/IBus engine mode: connecting to existing ibus-daemon. \
                 Activate via GNOME Settings → Keyboard → Input Sources → Add (Other → Chinese → KeyTao), \
                 or via ibus-setup."
            );
            tokio::runtime::Runtime::new()
                .expect("tokio runtime")
                .block_on(gnome_ibus_engine::run(engine));
            return;
        }

        if selected.wayland {
            let result = if kwin_socket {
                wayland_backend_kde::run(engine)
            } else {
                wayland_backend::run(engine)
            };
            match result {
                Ok(()) => {}
                Err(e) => {
                    tracing::warn!("Wayland backend stopped: {e}");
                }
            }
        }
        // Keep the process alive so IBus/XIM threads can serve apps.
        loop {
            std::thread::park();
        }
    }
}
