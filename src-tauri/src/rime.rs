//! Tauri adapter layer — thin glue between keytao-core and Tauri's IPC.
//! All platform logic, IME state types, and librime calls live in `keytao-core`.

use keytao_core::{default_shared_data_dir, default_user_data_dir, deploy, Engine, ImeState};
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Mutex,
};
use tauri::Manager;

// ── Managed state ─────────────────────────────────────────────────────────────

pub struct RimeEngine {
    pub engine: Mutex<Option<Engine>>,
}

impl Default for RimeEngine {
    fn default() -> Self {
        Self {
            engine: Mutex::new(None),
        }
    }
}

unsafe impl Send for RimeEngine {}
unsafe impl Sync for RimeEngine {}

// ── Overlay: track the PID to restore focus after text injection ──────────────

static INJECTION_TARGET_PID: AtomicU32 = AtomicU32::new(0);

pub fn set_injection_target(pid: u32) {
    INJECTION_TARGET_PID.store(pid, Ordering::Relaxed);
}

// ── Tauri commands ────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn rime_setup(
    user_data_dir: String,
    shared_data_dir: Option<String>,
    state: tauri::State<'_, RimeEngine>,
) -> Result<(), String> {
    let user = if user_data_dir.is_empty() {
        default_user_data_dir()
            .ok_or("Could not determine KeyTao data dir")?
            .to_string_lossy()
            .into_owned()
    } else {
        user_data_dir
    };
    let shared = shared_data_dir.unwrap_or_else(default_shared_data_dir);

    tokio::task::spawn_blocking(move || deploy(user, shared))
        .await
        .map_err(|e| e.to_string())??;

    let engine = Engine::new()?;
    *state.engine.lock().unwrap() = Some(engine);
    Ok(())
}

#[tauri::command]
pub fn rime_process_key(
    keycode: i32,
    mask: i32,
    state: tauri::State<'_, RimeEngine>,
) -> Result<ImeState, String> {
    let guard = state.engine.lock().unwrap();
    let engine = guard.as_ref().ok_or("Rime session not initialised")?;
    Ok(engine.process_key(keycode as u32, mask as u32))
}

#[tauri::command]
pub fn rime_select_candidate(
    index: usize,
    state: tauri::State<'_, RimeEngine>,
) -> Result<ImeState, String> {
    let guard = state.engine.lock().unwrap();
    let engine = guard.as_ref().ok_or("Rime session not initialised")?;
    Ok(engine.select_candidate(index))
}

#[tauri::command]
pub fn rime_change_page(
    backward: bool,
    state: tauri::State<'_, RimeEngine>,
) -> Result<ImeState, String> {
    let guard = state.engine.lock().unwrap();
    let engine = guard.as_ref().ok_or("Rime session not initialised")?;
    Ok(engine.change_page(backward))
}

#[tauri::command]
pub fn rime_reset(state: tauri::State<'_, RimeEngine>) -> ImeState {
    let guard = state.engine.lock().unwrap();
    guard
        .as_ref()
        .map(|e| e.reset())
        .unwrap_or_else(ImeState::empty)
}

#[tauri::command]
pub fn rime_is_ready(state: tauri::State<'_, RimeEngine>) -> bool {
    state.engine.lock().unwrap().is_some()
}

#[tauri::command]
pub fn rime_get_data_dir() -> Option<String> {
    default_user_data_dir().map(|p| p.to_string_lossy().into_owned())
}

#[tauri::command]
pub fn rime_has_schemas() -> bool {
    match default_user_data_dir() {
        Some(p) => keytao_core::has_schemas(&p),
        None => false,
    }
}

#[tauri::command]
pub fn rime_memory_usage() -> u64 {
    use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, System};
    let pid = Pid::from_u32(std::process::id());
    let mut sys = System::new_with_specifics(
        RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing().with_memory()),
    );
    sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), false);
    sys.process(pid).map(|p| p.memory()).unwrap_or(0)
}

#[tauri::command]
pub async fn rime_inject_text(text: String, app: tauri::AppHandle) -> Result<(), String> {
    let target_pid = INJECTION_TARGET_PID.load(Ordering::Relaxed);

    if let Some(w) = app.get_webview_window("ime-overlay") {
        w.hide().map_err(|e| e.to_string())?;
    }

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    restore_focus(target_pid);
    tokio::time::sleep(std::time::Duration::from_millis(120)).await;

    tokio::task::spawn_blocking(move || {
        use enigo::{Enigo, Keyboard, Settings};
        let mut enigo = Enigo::new(&Settings::default()).map_err(|e| e.to_string())?;
        enigo.text(&text).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

// ── Overlay OS helpers ────────────────────────────────────────────────────────

pub fn get_frontmost_pid() -> u32 {
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("osascript")
            .args([
                "-e",
                "tell application \"System Events\" to get unix id of \
                 (first application process whose frontmost is true)",
            ])
            .output();
        match out {
            Ok(o) => String::from_utf8_lossy(&o.stdout).trim().parse().unwrap_or(0),
            Err(_) => 0,
        }
    }
    #[cfg(target_os = "linux")]
    {
        let out = std::process::Command::new("xdotool")
            .args(["getactivewindow", "getwindowpid"])
            .output();
        match out {
            Ok(o) => String::from_utf8_lossy(&o.stdout).trim().parse().unwrap_or(0),
            Err(_) => 0,
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        0
    }
}

fn restore_focus(pid: u32) {
    if pid == 0 {
        return;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("osascript")
            .args([
                "-e",
                &format!(
                    "tell application \"System Events\" to set frontmost of \
                     (first application process whose unix id is {pid}) to true"
                ),
            ])
            .output()
            .ok();
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdotool")
            .args(["search", "--pid", &pid.to_string(), "windowfocus", "--sync"])
            .output()
            .ok();
    }
}
