//! IBus D-Bus backend for keytao-ime.
//!
//! Implements enough of the IBus D-Bus protocol so that Chromium/CEF apps
//! (e.g. WeChatAppEx) can use keytao as their IME without requiring a real
//! IBus daemon.

use crate::engine::{CoreEngine, ImeSession};
use keytao_core::{Candidate, ImeState};
use std::{
    fs,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
};
use zbus::{connection, interface, object_server::SignalContext, zvariant};

// ── IBus text helper ─────────────────────────────────────────────────────────

const IBUS_ORIENTATION_SYSTEM: i32 = 2;
const MOD_CONTROL: u32 = 0x0004;
const MOD_MOD1: u32 = 0x0008;
const RELEASE_MASK: u32 = 1 << 30;

fn is_shift_key(sym: u32) -> bool {
    matches!(sym, 0xffe1 | 0xffe2)
}

fn is_candidate_select_key(sym: u32) -> bool {
    sym == 0x0020
}

fn is_enter_key(sym: u32) -> bool {
    matches!(sym, 0xff0d | 0xff8d)
}

fn should_bypass_empty_composition(sym: u32, mods: u32, state: &ImeState) -> bool {
    if !state.preedit.is_empty() || !state.candidates.is_empty() {
        return false;
    }
    if mods & (MOD_CONTROL | MOD_MOD1) != 0 {
        return true;
    }
    matches!(
        sym,
        0x0020 | 0xff08 | 0xffff | 0xff09 | 0xff0d | 0xff1b | 0xff50..=0xff58 | 0xff8d
    )
}

/// Map a keyval to a candidate index based on the engine's select_keys config.
/// Returns None if the key is not a candidate selection key for the current state.
fn candidate_index_for_select_key(sym: u32, state: &ImeState) -> Option<usize> {
    if state.candidates.is_empty() {
        return None;
    }
    let keys = state.select_keys.as_deref().unwrap_or("1234567890");
    // Convert keysym to the char it represents (basic ASCII range only).
    let ch = char::from_u32(sym)?;
    keys.chars().position(|k| k == ch)
}

/// Build an IBusText structure as a variant.
/// IBus D-Bus type: v containing (sa{sv}sv)
///   ("IBusText", {}, text_string, v:("IBusAttrList",{},[]))
fn ibus_text_variant(text: &str) -> zvariant::Value<'static> {
    use zvariant::{Array, Dict, Signature, StructureBuilder, Value};

    // Build IBusAttrList: ("IBusAttrList", {}, [])
    let sig_s = Signature::try_from("s").unwrap();
    let sig_v = Signature::try_from("v").unwrap();
    let empty_dict1 = Dict::new(sig_s.clone(), sig_v.clone());
    let empty_array = Array::new(sig_v.clone());
    let attr_list = StructureBuilder::new()
        .add_field("IBusAttrList".to_owned())
        .append_field(Value::Dict(empty_dict1))
        .append_field(Value::Array(empty_array))
        .build();

    // Wrap attr_list as variant
    let attr_list_variant = Value::Value(Box::new(Value::Structure(attr_list)));

    // Build IBusText: ("IBusText", {}, text, v:attr_list)
    let empty_dict2 = Dict::new(sig_s, sig_v);
    let ibus_text_struct = StructureBuilder::new()
        .add_field("IBusText".to_owned())
        .append_field(Value::Dict(empty_dict2))
        .add_field(text.to_owned())
        .append_field(attr_list_variant)
        .build();

    // Return the structure directly so callers used as `v` signal parameters get
    // single-wrapped (v(sa{sv}sv)).  For av array elements callers must wrap
    // explicitly with Value::Value(Box::new(ibus_text_variant(…))).
    Value::Structure(ibus_text_struct)
}

/// Wrap an IBusText structure inside a variant for use in `av` arrays.
fn ibus_text_as_variant(text: &str) -> zvariant::Value<'static> {
    zvariant::Value::Value(Box::new(ibus_text_variant(text)))
}

fn ibus_text_value(text: &str) -> zvariant::OwnedValue {
    zvariant::OwnedValue::try_from(ibus_text_variant(text)).expect("ibus_text_value")
}

/// Build an IBusEngineDesc value for the "keytao" engine.
/// Structure: (sa{sv} name longname description language license author icon layout rank hotkeys symbol setup layout_variant layout_option version textdomain)
fn ibus_engine_desc_value() -> zvariant::OwnedValue {
    use zvariant::{Dict, Signature, StructureBuilder, Value};

    let sig_s = Signature::try_from("s").unwrap();
    let sig_v = Signature::try_from("v").unwrap();
    let empty_dict = Dict::new(sig_s, sig_v);

    let engine = StructureBuilder::new()
        .add_field("IBusEngineDesc".to_owned())
        .append_field(Value::Dict(empty_dict))
        .add_field("keytao".to_owned()) // name
        .add_field("KeyTao".to_owned()) // longname
        .add_field("KeyTao Input Method".to_owned()) // description
        .add_field("zh".to_owned()) // language
        .add_field("".to_owned()) // license
        .add_field("".to_owned()) // author
        .add_field("".to_owned()) // icon
        .add_field("default".to_owned()) // layout
        .add_field(0u32) // rank
        .add_field("".to_owned()) // hotkeys
        .add_field("键".to_owned()) // symbol
        .add_field("".to_owned()) // setup
        .add_field("".to_owned()) // layout_variant
        .add_field("".to_owned()) // layout_option
        .add_field("".to_owned()) // version
        .add_field("".to_owned()) // textdomain
        .build();

    zvariant::OwnedValue::try_from(Value::Structure(engine)).expect("ibus_engine_desc_value")
}

fn candidate_display_text(candidate: &Candidate) -> String {
    match candidate
        .comment
        .as_deref()
        .filter(|comment| !comment.is_empty())
    {
        Some(comment) => format!("{} {}", candidate.text, comment),
        None => candidate.text.clone(),
    }
}

fn candidate_label(index: usize, select_keys: Option<&str>) -> String {
    select_keys
        .and_then(|keys| keys.chars().nth(index))
        .or_else(|| "1234567890".chars().nth(index))
        .map(|ch| ch.to_string())
        .unwrap_or_else(|| (index + 1).to_string())
}

/// Build an IBusLookupTable value.
/// Serialized shape: ("IBusLookupTable", a{sv}, u, u, b, b, i, av, av).
fn ibus_lookup_table_value(state: &ImeState) -> zvariant::OwnedValue {
    use zvariant::{Array, Dict, Signature, StructureBuilder, Value};

    let sig_s = Signature::try_from("s").unwrap();
    let sig_v = Signature::try_from("v").unwrap();
    let empty_dict = Dict::new(sig_s, sig_v.clone());

    let mut candidates = Array::new(sig_v.clone());
    for candidate in &state.candidates {
        candidates
            .append(ibus_text_as_variant(&candidate_display_text(candidate)))
            .expect("append IBus lookup candidate");
    }

    let mut labels = Array::new(sig_v);
    let select_keys = state.select_keys.as_deref();
    for index in 0..state.candidates.len() {
        labels
            .append(ibus_text_as_variant(&candidate_label(index, select_keys)))
            .expect("append IBus lookup label");
    }

    let page_size = state.candidates.len().clamp(1, 16) as u32;
    let cursor_pos = state
        .highlighted_candidate_index
        .min(state.candidates.len().saturating_sub(1)) as u32;

    let table = StructureBuilder::new()
        .add_field("IBusLookupTable".to_owned())
        .append_field(Value::Dict(empty_dict))
        .add_field(page_size)
        .add_field(cursor_pos)
        .add_field(true)
        .add_field(false)
        .add_field(IBUS_ORIENTATION_SYSTEM)
        .append_field(Value::Array(candidates))
        .append_field(Value::Array(labels))
        .build();

    zvariant::OwnedValue::try_from(Value::Structure(table)).expect("ibus_lookup_table_value")
}

// ── InputContext D-Bus object ─────────────────────────────────────────────────

struct InputContext {
    session: ImeSession,
    kimpanel_ctxt: Option<SignalContext<'static>>,
    cursor_x: Arc<AtomicI32>,
    cursor_y: Arc<AtomicI32>,
}

#[interface(name = "org.freedesktop.IBus.InputContext")]
impl InputContext {
    async fn focus_in(&self) {
        tracing::debug!("IBus InputContext: FocusIn");
    }

    async fn focus_out(&self, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        tracing::debug!("IBus InputContext: FocusOut");
        self.session.reset();
        clear_input_context_ui(&ctxt, &self.kimpanel_ctxt).await;
    }

    async fn reset(&self, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        tracing::debug!("IBus InputContext: Reset");
        self.session.reset();
        clear_input_context_ui(&ctxt, &self.kimpanel_ctxt).await;
    }

    async fn set_cursor_location(&self, x: i32, y: i32, _w: i32, _h: i32) {
        self.cursor_x.store(x, Ordering::Relaxed);
        self.cursor_y.store(y, Ordering::Relaxed);
        if let Some(kctxt) = &self.kimpanel_ctxt {
            let _ = Kimpanel::update_spot_location(kctxt, x, y).await;
        }
    }
    async fn set_cursor_location_relative(&self, _x: i32, _y: i32, _w: i32, _h: i32) {}
    async fn set_capabilities(&self, _caps: u32) {}

    async fn destroy(
        &self,
        #[zbus(object_server)] server: &zbus::ObjectServer,
        #[zbus(signal_context)] ctxt: SignalContext<'_>,
    ) -> zbus::fdo::Result<()> {
        tracing::debug!("IBus InputContext: Destroy");
        self.session.reset();
        clear_input_context_ui(&ctxt, &self.kimpanel_ctxt).await;
        server
            .remove::<InputContext, _>(ctxt.path().to_owned())
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        Ok(())
    }

    /// Process a key event. Returns true if consumed by the IME.
    async fn process_key_event(
        &self,
        keyval: u32,
        _keycode: u32,
        state: u32,
        #[zbus(signal_context)] ctxt: SignalContext<'_>,
    ) -> bool {
        if state & RELEASE_MASK != 0 {
            if is_shift_key(keyval) {
                if let Some(result) = self.session.process_key_result(keyval, RELEASE_MASK) {
                    tracing::debug!(
                        "IBus mode after Shift release: ascii_mode={}",
                        result.state.ascii_mode
                    );
                    return result.accepted;
                }
            }
            return false;
        }

        tracing::debug!("IBus ProcessKeyEvent keyval={keyval:#x} state={state:#x}");

        let before_state = self.session.state();
        if should_bypass_empty_composition(keyval, state, &before_state) {
            clear_input_context_ui(&ctxt, &self.kimpanel_ctxt).await;
            return false;
        }
        if is_enter_key(keyval) && !before_state.preedit.is_empty() {
            clear_preedit(&ctxt, &self.kimpanel_ctxt).await;
            let ov = ibus_text_value(&before_state.preedit);
            if let Ok(v) = zvariant::Value::try_from(&ov) {
                let _ = Self::commit_text(&ctxt, v).await;
            }
            self.session.reset();
            let _ = Self::hide_lookup_table(&ctxt).await;
            if let Some(kctxt) = &self.kimpanel_ctxt {
                let _ = Kimpanel::show_lookup_table(kctxt, false).await;
            }
            return true;
        }
        let candidate_select_index = if is_candidate_select_key(keyval) {
            Some(
                before_state
                    .highlighted_candidate_index
                    .min(before_state.candidates.len().saturating_sub(1)),
            )
        } else {
            candidate_index_for_select_key(keyval, &before_state)
        };
        if let Some(index) = candidate_select_index {
            if !before_state.candidates.is_empty() {
                if let Some(ime_state) = self.session.select_candidate(index) {
                    if let Some(ref text) = ime_state.committed {
                        if !text.is_empty() {
                            clear_preedit(&ctxt, &self.kimpanel_ctxt).await;
                            let ov = ibus_text_value(text);
                            if let Ok(v) = zvariant::Value::try_from(&ov) {
                                let _ = Self::commit_text(&ctxt, v).await;
                            }
                        }
                    }
                    clear_input_context_ui(&ctxt, &self.kimpanel_ctxt).await;
                    return true;
                }
            }
        }

        let result = match self.session.process_key_result(keyval, state) {
            Some(r) => r,
            None => return false,
        };

        let ime_state = result.state;

        let consumed = result.accepted;

        if let Some(ref text) = ime_state.committed {
            if !text.is_empty() {
                tracing::debug!("IBus CommitText: {text:?}");
                clear_preedit(&ctxt, &self.kimpanel_ctxt).await;
                let ov = ibus_text_value(text);
                if let Ok(v) = zvariant::Value::try_from(&ov) {
                    let _ = Self::commit_text(&ctxt, v).await;
                }
            }
        }

        if ime_state.preedit.is_empty() {
            let _ = Self::hide_preedit_text(&ctxt).await;
            if let Some(kctxt) = &self.kimpanel_ctxt {
                let _ = Kimpanel::show_preedit_text(kctxt, false).await;
            }
        } else {
            let cursor = ime_state.cursor as u32;
            let ov = ibus_text_value(&ime_state.preedit);
            if let Ok(v) = zvariant::Value::try_from(&ov) {
                let _ = Self::update_preedit_text(&ctxt, v, cursor, true).await;
            }
            if let Some(kctxt) = &self.kimpanel_ctxt {
                let _ = Kimpanel::update_preedit_text(kctxt, &ime_state.preedit, "").await;
                let _ = Kimpanel::show_preedit_text(kctxt, true).await;
            }
        }

        if ime_state.candidates.is_empty() {
            let _ = Self::hide_lookup_table(&ctxt).await;
            if let Some(kctxt) = &self.kimpanel_ctxt {
                let _ = Kimpanel::show_lookup_table(kctxt, false).await;
            }
        } else {
            let ov = ibus_lookup_table_value(&ime_state);
            if let Ok(v) = zvariant::Value::try_from(&ov) {
                let _ = Self::update_lookup_table(&ctxt, v, true).await;
            }
            if let Some(kctxt) = &self.kimpanel_ctxt {
                let mut labels = Vec::new();
                let mut cands = Vec::new();
                let select_keys = ime_state.select_keys.as_deref();
                for (i, c) in ime_state.candidates.iter().enumerate() {
                    labels.push(candidate_label(i, select_keys));
                    cands.push(candidate_display_text(c));
                }
                let labels_ref: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
                let cands_ref: Vec<&str> = cands.iter().map(|s| s.as_str()).collect();
                let attrs: Vec<&str> = vec![];
                let _ = Kimpanel::update_lookup_table(
                    kctxt,
                    &labels_ref,
                    &cands_ref,
                    &attrs,
                    false, // has_prev
                    false, // has_next
                ).await;
                let _ = Kimpanel::show_lookup_table(kctxt, true).await;
                let _ = Kimpanel::update_spot_location(kctxt, self.cursor_x.load(Ordering::Relaxed), self.cursor_y.load(Ordering::Relaxed)).await;
            }
        }

        consumed
    }

    // ── Signals ──────────────────────────────────────────────────────────────

    #[zbus(signal)]
    async fn commit_text(ctxt: &SignalContext<'_>, text: zvariant::Value<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn update_preedit_text(
        ctxt: &SignalContext<'_>,
        text: zvariant::Value<'_>,
        cursor_pos: u32,
        visible: bool,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn hide_preedit_text(ctxt: &SignalContext<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn show_preedit_text(ctxt: &SignalContext<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn update_lookup_table(
        ctxt: &SignalContext<'_>,
        table: zvariant::Value<'_>,
        visible: bool,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn hide_lookup_table(ctxt: &SignalContext<'_>) -> zbus::Result<()>;
}

async fn clear_input_context_ui(ctxt: &SignalContext<'_>, kctxt: &Option<SignalContext<'static>>) {
    let _ = InputContext::hide_preedit_text(ctxt).await;
    let _ = InputContext::hide_lookup_table(ctxt).await;
    if let Some(kc) = kctxt {
        let _ = Kimpanel::show_preedit_text(kc, false).await;
        let _ = Kimpanel::show_lookup_table(kc, false).await;
    }
}

/// Send an empty UpdatePreeditText to tell the client the composition ended
/// before committing. This is the sequence Chromium/CEF requires so that it
/// can correctly place the committed text without conflating it with the
/// still-active preedit region.
async fn clear_preedit(ctxt: &SignalContext<'_>, kctxt: &Option<SignalContext<'static>>) {
    let ov = ibus_text_value("");
    if let Ok(v) = zvariant::Value::try_from(&ov) {
        let _ = InputContext::update_preedit_text(ctxt, v, 0, false).await;
    }
    if let Some(kc) = kctxt {
        let _ = Kimpanel::update_preedit_text(kc, "", "").await;
        let _ = Kimpanel::show_preedit_text(kc, false).await;
    }
}

// ── IBusBus D-Bus object ──────────────────────────────────────────────────────

struct Kimpanel;

#[interface(name = "org.kde.kimpanel.inputmethod")]
impl Kimpanel {
    #[zbus(signal)]
    async fn update_spot_location(ctxt: &SignalContext<'_>, x: i32, y: i32) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn update_lookup_table(
        ctxt: &SignalContext<'_>,
        labels: &[&str],
        candidates: &[&str],
        attrs: &[&str],
        has_prev: bool,
        has_next: bool,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn show_lookup_table(ctxt: &SignalContext<'_>, b: bool) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn update_preedit_text(ctxt: &SignalContext<'_>, text: &str, attr: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn show_preedit_text(ctxt: &SignalContext<'_>, b: bool) -> zbus::Result<()>;
}

struct IBusBus {
    engine: CoreEngine,
    ctx_counter: Arc<AtomicU32>,
    kimpanel_ctxt: Option<SignalContext<'static>>,
}

#[interface(name = "org.freedesktop.IBus")]
impl IBusBus {
    /// CreateInputContext(client_name) → object_path
    async fn create_input_context(
        &self,
        client_name: &str,
        #[zbus(object_server)] server: &zbus::ObjectServer,
    ) -> zbus::fdo::Result<zbus::zvariant::OwnedObjectPath> {
        let n = self.ctx_counter.fetch_add(1, Ordering::SeqCst);
        let path_str = format!("/org/freedesktop/IBus/InputContext_{n}");
        let path = zbus::zvariant::OwnedObjectPath::try_from(path_str.clone())
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;

        tracing::debug!("IBus CreateInputContext client={client_name:?} -> {path_str}");

        let session = self
            .engine
            .create_session()
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        let ctx = InputContext {
            session,
            kimpanel_ctxt: self.kimpanel_ctxt.clone(),
            cursor_x: Arc::new(AtomicI32::new(0)),
            cursor_y: Arc::new(AtomicI32::new(0)),
        };
        server
            .at(path.clone(), ctx)
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;

        Ok(path)
    }

    async fn is_global_engine(&self) -> bool {
        false
    }

    async fn get_engines(&self) -> Vec<zvariant::OwnedValue> {
        vec![]
    }

    async fn list_active_engines(&self) -> Vec<zvariant::OwnedValue> {
        vec![]
    }

    async fn get_global_engine(&self) -> zbus::fdo::Result<zvariant::OwnedValue> {
        Ok(ibus_engine_desc_value())
    }

    #[zbus(signal)]
    async fn global_engine_changed(ctxt: &SignalContext<'_>, name: &str) -> zbus::Result<()>;
}

// ── IBus address file management ──────────────────────────────────────────────

fn write_ibus_address_files(dbus_address: &str) {
    let machine_id = read_machine_id();
    let pid = std::process::id();

    let bus_dir = match dirs::config_dir() {
        Some(d) => d.join("ibus").join("bus"),
        None => {
            tracing::warn!("cannot determine config dir; skipping IBus address files");
            return;
        }
    };
    if let Err(e) = fs::create_dir_all(&bus_dir) {
        tracing::warn!("failed to create {}: {e}", bus_dir.display());
        return;
    }

    let content = format!(
        "# This file is created by keytao-ime (IBus compatible)\nIBUS_ADDRESS={dbus_address}\nIBUS_DAEMON_PID={pid}\n"
    );

    let display_num = display_number();
    let wayland_num = wayland_display_number();

    let mut names = vec![
        format!("{machine_id}-unix-{display_num}"),
        format!("{machine_id}-unix-wayland-0"),
        format!("{machine_id}-unix-wayland-1"),
    ];
    if let Some(wn) = wayland_num {
        names.push(format!("{machine_id}-unix-wayland-{wn}"));
    }
    names.sort();
    names.dedup();

    for name in names {
        let path = bus_dir.join(&name);
        if let Err(e) = fs::write(&path, &content) {
            tracing::warn!("failed to write {}: {e}", path.display());
        } else {
            tracing::debug!("wrote IBus address file: {}", path.display());
        }
    }
}

fn read_machine_id() -> String {
    for path in &["/etc/machine-id", "/var/lib/dbus/machine-id"] {
        if let Ok(s) = fs::read_to_string(path) {
            let id = s.trim().to_owned();
            if !id.is_empty() {
                return id;
            }
        }
    }
    "unknown".to_owned()
}

fn display_number() -> u32 {
    std::env::var("DISPLAY")
        .ok()
        .and_then(|d| {
            d.rsplit(':')
                .next()
                .and_then(|s| s.split('.').next())
                .and_then(|s| s.parse().ok())
        })
        .unwrap_or(0)
}

fn wayland_display_number() -> Option<u32> {
    std::env::var("WAYLAND_DISPLAY")
        .ok()
        .and_then(|d| d.rsplit('-').next().and_then(|s| s.parse().ok()))
}

// ── Public entry point ────────────────────────────────────────────────────────

pub async fn run(engine: CoreEngine) {
    tracing::info!("IBus D-Bus backend starting");

    let builder = match connection::Builder::session() {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("IBus: failed to get session bus builder: {e}");
            return;
        }
    };
    let builder = match builder.name("org.freedesktop.IBus") {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("IBus: failed to request name: {e}");
            return;
        }
    };
    
    let engine_clone = engine.clone();
    let builder = match builder.serve_at("/org/kde/kimpanel/inputmethod", Kimpanel) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("Failed to serve Kimpanel: {e}");
            return; // We need builder back, but serve_at consumes it on Err too! Wait, zbus serve_at returns Result<Builder, Error>. No, in older zbus it might return Result<Builder, Error>. If it returns Err, we can't easily recover the builder without matching. Let's just use it directly.
        }
    };

    let builder = match builder.serve_at(
        "/org/freedesktop/IBus",
        IBusBus {
            engine,
            ctx_counter: Arc::new(AtomicU32::new(1)),
            kimpanel_ctxt: None, // Will fill after build
        },
    ) {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("IBus: failed to serve_at: {e}");
            return;
        }
    };

    let conn = match builder.build().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("IBus D-Bus backend failed to connect: {e}");
            return;
        }
    };

    let dbus_address = std::env::var("DBUS_SESSION_BUS_ADDRESS")
        .unwrap_or_else(|_| "unix:path=/run/user/1000/bus".to_owned());

    write_ibus_address_files(&dbus_address);

    let kimpanel_ctxt = SignalContext::new(&conn, "/org/kde/kimpanel/inputmethod").ok();
    
    // We need to update the IBusBus instance with the kimpanel_ctxt.
    // However, IBusBus is owned by the ObjectServer. Instead of mutating it, we just set
    // it properly before serving if possible, or use a shared state.
    // Actually, we can just create the SignalContext from `conn` and share it!
    
    // Let's re-register IBusBus with the valid kimpanel_ctxt.
    let _ = conn.object_server().remove::<IBusBus, _>("/org/freedesktop/IBus").await;
    let _ = conn.object_server().at("/org/freedesktop/IBus", IBusBus {
        engine: engine_clone,
        ctx_counter: Arc::new(AtomicU32::new(1)),
        kimpanel_ctxt,
    }).await;

    // Notify any already-connected IBus clients that the keytao engine is active.
    // Chromium/CEF clients that connected before this signal can use GetGlobalEngine instead.
    if let Ok(signal_ctx) = SignalContext::new(&conn, "/org/freedesktop/IBus") {
        IBusBus::global_engine_changed(&signal_ctx, "keytao")
            .await
            .ok();
    }

    tracing::info!("IBus D-Bus backend ready ({})", dbus_address);
    let _conn = conn; // keep connection alive

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
    }
}
