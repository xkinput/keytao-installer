//! Pure librime engine wrapper — no Tauri, no D-Bus, no platform I/O.
//! Every platform frontend (Tauri app, ibus engine, macOS IMKit, Windows TSF)
//! links against this crate as its rime back-end.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// ── Public types ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImeState {
    pub preedit: String,
    pub cursor: usize,
    pub candidates: Vec<Candidate>,
    pub page: usize,
    pub is_last_page: bool,
    pub committed: Option<String>,
    pub select_keys: Option<String>,
    pub ascii_mode: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Candidate {
    pub text: String,
    pub comment: Option<String>,
}

impl ImeState {
    pub fn empty() -> Self {
        Self {
            preedit: String::new(),
            cursor: 0,
            candidates: vec![],
            page: 0,
            is_last_page: true,
            committed: None,
            select_keys: None,
            ascii_mode: false,
        }
    }
}

// ── Desktop-only engine (guarded at the module level) ────────────────────────

#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod desktop {
    use super::*;
    use rime_api::{
        create_session, full_deploy_and_wait, initialize, setup, DeployResult, KeyEvent, Traits,
    };
    use std::sync::OnceLock;

    // Deployment runs exactly once per process; further calls return the cached result.
    static RIME_INIT: OnceLock<Result<(), String>> = OnceLock::new();

    /// Initialize and fully deploy librime.
    /// Blocking — run inside `tokio::task::spawn_blocking` when called from async code.
    pub fn deploy(user_data_dir: String, shared_data_dir: String) -> Result<(), String> {
        RIME_INIT
            .get_or_init(|| {
                let mut traits = Traits::new();
                traits.set_user_data_dir(&user_data_dir);
                traits.set_shared_data_dir(&shared_data_dir);
                traits.set_distribution_name("KeyTao");
                traits.set_distribution_code_name("keytao");
                traits.set_distribution_version("1.0.0");
                setup(&mut traits);
                initialize(&mut traits);
                match full_deploy_and_wait() {
                    DeployResult::Success => Ok(()),
                    DeployResult::Failure => Err("Rime deployment failed".to_string()),
                }
            })
            .clone()
    }

    /// An active rime input session.
    pub struct Engine {
        session: rime_api::Session,
    }

    // SAFETY: Session holds only a usize (session_id).
    // librime's C API is documented as thread-safe across different sessions.
    unsafe impl Send for Engine {}
    unsafe impl Sync for Engine {}

    impl Engine {
        /// Create a new session. `deploy()` must have succeeded first.
        pub fn new() -> Result<Self, String> {
            let session = create_session().map_err(|e| format!("{e:?}"))?;
            Ok(Self { session })
        }

        pub fn process_key(&self, keycode: u32, mask: u32) -> ImeState {
            self.session.process_key(KeyEvent::new(keycode, mask));
            extract_state(&self.session)
        }

        pub fn select_candidate(&self, index: usize) -> ImeState {
            if index < 9 {
                let kc = b'1' as u32 + index as u32;
                self.session.process_key(KeyEvent::new(kc, 0));
            }
            extract_state(&self.session)
        }

        pub fn change_page(&self, backward: bool) -> ImeState {
            let kc = if backward { b'-' as u32 } else { b'=' as u32 };
            self.session.process_key(KeyEvent::new(kc, 0));
            extract_state(&self.session)
        }

        pub fn reset(&self) -> ImeState {
            self.session.process_key(KeyEvent::new(0xff1b_u32, 0)); // XK_Escape
            ImeState::empty()
        }

        pub fn current_schema_name(&self) -> String {
            self.session
                .status()
                .map(|s| s.schema_name().to_string())
                .unwrap_or_else(|_| "unknown".to_string())
        }

        pub fn is_ascii_mode(&self) -> bool {
            self.session
                .status()
                .map(|s| s.is_ascii_mode)
                .unwrap_or(false)
        }
    }

    fn extract_state(session: &rime_api::Session) -> ImeState {
        let committed = session.commit().map(|c| c.text().to_string());

        let Some(ctx) = session.context() else {
            return ImeState {
                committed,
                ..ImeState::empty()
            };
        };

        let comp = ctx.composition();
        let preedit = comp.preedit.unwrap_or("").to_string();
        let cursor = comp.cursor_pos;

        let menu = ctx.menu();
        let candidates = menu
            .candidates
            .iter()
            .map(|c| Candidate {
                text: c.text.to_string(),
                comment: c.comment.map(|s: &str| s.to_string()),
            })
            .collect();

        let ascii_mode = session.status().map(|s| s.is_ascii_mode).unwrap_or(false);

        ImeState {
            preedit,
            cursor,
            candidates,
            page: menu.page_no,
            is_last_page: menu.is_last_page,
            committed,
            select_keys: menu.select_keys.map(|s: &str| s.to_string()),
            ascii_mode,
        }
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub use desktop::{deploy, Engine};

// ── Platform path helpers (all platforms) ────────────────────────────────────

/// Dedicated keytao user data directory for this platform.
pub fn default_user_data_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        return dirs::home_dir().map(|h| h.join("Library/keytao"));
    }
    #[cfg(target_os = "windows")]
    {
        return dirs::config_dir().map(|c| c.join("keytao"));
    }
    #[cfg(target_os = "linux")]
    {
        return dirs::data_local_dir().map(|d| d.join("keytao"));
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        None
    }
}

/// Best-guess shared rime data directory (system-installed schemas/essay.txt).
pub fn default_shared_data_dir() -> String {
    #[cfg(target_os = "macos")]
    {
        let squirrel = "/Library/Input Methods/Squirrel.app/Contents/SharedSupport";
        if Path::new(squirrel).exists() {
            return squirrel.to_string();
        }
        for p in [
            "/opt/homebrew/share/rime-data",
            "/usr/local/share/rime-data",
        ] {
            if Path::new(p).exists() {
                return p.to_string();
            }
        }
        return String::new();
    }
    #[cfg(target_os = "linux")]
    {
        // Prefer user-local keytao schemas (e.g. installed via fcitx5/keytao-installer);
        // fall back to system-wide rime-data.
        let candidates = [
            dirs::data_local_dir().map(|d| d.join("keytao")),
            dirs::data_local_dir().map(|d| d.join("fcitx5/rime")),
            Some(std::path::PathBuf::from("/usr/share/rime-data")),
        ];
        for p in candidates.into_iter().flatten() {
            if crate::has_schemas(&p) {
                return p.to_string_lossy().into_owned();
            }
        }
        return "/usr/share/rime-data".to_string();
    }
    #[cfg(target_os = "windows")]
    {
        return std::env::var("WEASEL_ROOT")
            .map(|r| format!("{r}\\data"))
            .unwrap_or_else(|_| r"C:\Program Files\Rime\weasel-data".to_string());
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        String::new()
    }
}

/// Returns true if `dir` exists and contains at least one `.schema.yaml` file.
pub fn has_schemas(dir: &Path) -> bool {
    if !dir.exists() {
        return false;
    }
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .any(|e| e.file_name().to_string_lossy().ends_with(".schema.yaml"))
        })
        .unwrap_or(false)
}
