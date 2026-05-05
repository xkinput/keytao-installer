use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::sync::Mutex;

#[cfg(not(any(target_os = "android", target_os = "ios")))]
use keytao_core::{deploy, Engine};

// ── C-compatible state struct ─────────────────────────────────────────────────

/// Flat view of IME state returned to C callers.
/// All strings are null-terminated UTF-8. Free with keytao_free_state().
#[repr(C)]
pub struct KeytaoState {
    pub preedit: *mut c_char,
    pub cursor: u32,
    pub candidate_texts: *mut *mut c_char,
    pub candidate_comments: *mut *mut c_char,
    pub candidate_count: u32,
    pub page: u32,
    pub is_last_page: bool,
    pub committed: *mut c_char,
    pub select_keys: *mut c_char,
}

// ── Module-level singleton engine ─────────────────────────────────────────────

#[cfg(not(any(target_os = "android", target_os = "ios")))]
struct Global {
    engine: Option<Engine>,
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
static GLOBAL: Mutex<Global> = Mutex::new(Global { engine: None });

// ── Public C API ──────────────────────────────────────────────────────────────

/// Initialize the Rime engine. Must be called once before any other function.
/// Both `user_dir` and `shared_dir` must be non-null UTF-8 strings.
/// Returns true on success.
#[no_mangle]
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub extern "C" fn keytao_init(user_dir: *const c_char, shared_dir: *const c_char) -> bool {
    let user = unsafe { CStr::from_ptr(user_dir) }
        .to_str()
        .unwrap_or("")
        .to_string();
    let shared = unsafe { CStr::from_ptr(shared_dir) }
        .to_str()
        .unwrap_or("")
        .to_string();

    if let Err(e) = deploy(user, shared) {
        eprintln!("keytao_init: deploy failed: {e}");
        return false;
    }
    match Engine::new() {
        Ok(engine) => {
            let Ok(mut g) = GLOBAL.lock() else {
                return false;
            };
            g.engine = Some(engine);
            true
        }
        Err(e) => {
            eprintln!("keytao_init: Engine::new failed: {e}");
            false
        }
    }
}

#[no_mangle]
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub extern "C" fn keytao_is_initialized() -> bool {
    GLOBAL.lock().map(|g| g.engine.is_some()).unwrap_or(false)
}

/// Process a key event. Returns heap-allocated KeytaoState; caller must free
/// with keytao_free_state(). Returns null if the engine is not initialized.
#[no_mangle]
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub extern "C" fn keytao_process_key(keyval: u32, modifiers: u32) -> *mut KeytaoState {
    let Ok(g) = GLOBAL.lock() else {
        return std::ptr::null_mut();
    };
    let Some(ref engine) = g.engine else {
        return std::ptr::null_mut();
    };
    let state = engine.process_key(keyval, modifiers);
    Box::into_raw(Box::new(state_to_c(state)))
}

/// Select a candidate by 0-based index. Returns new state; caller must free.
#[no_mangle]
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub extern "C" fn keytao_select_candidate(index: u32) -> *mut KeytaoState {
    let Ok(g) = GLOBAL.lock() else {
        return std::ptr::null_mut();
    };
    let Some(ref engine) = g.engine else {
        return std::ptr::null_mut();
    };
    let state = engine.select_candidate(index as usize);
    Box::into_raw(Box::new(state_to_c(state)))
}

/// Flip to the next/previous candidate page. Returns new state; caller must free.
#[no_mangle]
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub extern "C" fn keytao_change_page(backward: bool) -> *mut KeytaoState {
    let Ok(g) = GLOBAL.lock() else {
        return std::ptr::null_mut();
    };
    let Some(ref engine) = g.engine else {
        return std::ptr::null_mut();
    };
    let state = engine.change_page(backward);
    Box::into_raw(Box::new(state_to_c(state)))
}

/// Clear current composition (Escape). Returns new state; caller must free.
#[no_mangle]
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub extern "C" fn keytao_reset() -> *mut KeytaoState {
    let Ok(g) = GLOBAL.lock() else {
        return std::ptr::null_mut();
    };
    let Some(ref engine) = g.engine else {
        return std::ptr::null_mut();
    };
    let state = engine.reset();
    Box::into_raw(Box::new(state_to_c(state)))
}

/// Free a KeytaoState returned by any keytao_* function.
#[no_mangle]
pub extern "C" fn keytao_free_state(ptr: *mut KeytaoState) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        let s = Box::from_raw(ptr);
        free_cstring(s.preedit);
        free_cstring(s.committed);
        free_cstring(s.select_keys);
        if !s.candidate_texts.is_null() {
            let texts = Vec::from_raw_parts(
                s.candidate_texts,
                s.candidate_count as usize,
                s.candidate_count as usize,
            );
            for t in texts {
                free_cstring(t);
            }
        }
        if !s.candidate_comments.is_null() {
            let comments = Vec::from_raw_parts(
                s.candidate_comments,
                s.candidate_count as usize,
                s.candidate_count as usize,
            );
            for c in comments {
                free_cstring(c);
            }
        }
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────────

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn state_to_c(state: keytao_core::ImeState) -> KeytaoState {
    let count = state.candidates.len();
    let (texts_ptr, comments_ptr) = if count == 0 {
        (std::ptr::null_mut(), std::ptr::null_mut())
    } else {
        let mut texts: Vec<*mut c_char> = state
            .candidates
            .iter()
            .map(|c| to_cstring(&c.text))
            .collect();
        let mut comments: Vec<*mut c_char> = state
            .candidates
            .iter()
            .map(|c| to_cstring(c.comment.as_deref().unwrap_or("")))
            .collect();
        let tp = texts.as_mut_ptr();
        let cp = comments.as_mut_ptr();
        std::mem::forget(texts);
        std::mem::forget(comments);
        (tp, cp)
    };

    KeytaoState {
        preedit: to_cstring(&state.preedit),
        cursor: state.cursor as u32,
        candidate_texts: texts_ptr,
        candidate_comments: comments_ptr,
        candidate_count: count as u32,
        page: state.page as u32,
        is_last_page: state.is_last_page,
        committed: to_cstring(state.committed.as_deref().unwrap_or("")),
        select_keys: to_cstring(state.select_keys.as_deref().unwrap_or("")),
    }
}

fn to_cstring(s: &str) -> *mut c_char {
    CString::new(s)
        .map(|cs| cs.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

unsafe fn free_cstring(ptr: *mut c_char) {
    if !ptr.is_null() {
        drop(CString::from_raw(ptr));
    }
}
