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

#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug, Default)]
struct BackendSelection {
    wayland: bool,
    xim: bool,
    ibus: bool,
}

#[cfg(target_os = "linux")]
impl BackendSelection {
    fn from_args(args: &[String]) -> Result<Self, String> {
        let mut selection = Self::default();
        let mut explicit = false;

        for arg in args {
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

    fn for_session(has_wayland: bool, has_x11: bool) -> Self {
        match (has_wayland, has_x11) {
            (true, true) => Self {
                // In mixed Wayland + XWayland sessions, keep the standalone helper
                // on XIM + IBus by default. The Wayland path should be owned by the
                // compositor-integrated frontend, otherwise the keyboard grab steals
                // shortcuts from the whole desktop.
                wayland: false,
                xim: true,
                ibus: true,
            },
            (true, false) => Self {
                wayland: true,
                xim: false,
                ibus: false,
            },
            (false, true) => Self {
                wayland: false,
                xim: true,
                ibus: true,
            },
            (false, false) => Self::default(),
        }
    }

    fn any(self) -> bool {
        self.wayland || self.xim || self.ibus
    }

    fn describe(self) -> String {
        let mut parts = Vec::new();
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
        let selected = if requested_backends.any() {
            requested_backends
        } else {
            BackendSelection::for_session(has_wayland, has_x11)
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
            "display server: wayland={} x11={} — selected backends [{}]",
            has_wayland,
            has_x11,
            selected.describe(),
        );

        if selected.wayland {
            wayland_backend::run(engine);
        } else {
            loop {
                std::thread::park();
            }
        }
    }
}
