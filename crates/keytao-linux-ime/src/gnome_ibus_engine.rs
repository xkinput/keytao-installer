//! IBus engine backend for GNOME Wayland.
//!
//! GNOME/mutter does NOT support zwp_input_method_manager_v2.  Instead it
//! runs its own ibus-daemon and exposes input methods as IBus engines.
//!
//! This module implements the IBus Engine protocol:
//!   1. Connect to the existing ibus-daemon on the session bus.
//!   2. Serve org.freedesktop.IBus.Factory at /org/freedesktop/IBus/Factory.
//!   3. Call org.freedesktop.IBus.RegisterComponent so the daemon knows us.
//!   4. When the user switches to "keytao" in GNOME Settings / ibus-setup,
//!      the daemon calls Factory.CreateEngine("keytao").
//!   5. We create an engine object and return its path.
//!   6. The daemon routes ProcessKeyEvent calls to our engine object.
//!   7. Our engine emits CommitText / UpdatePreeditText / UpdateLookupTable
//!      signals which the daemon proxies to the focused application.

use crate::engine::{CoreEngine, ImeSession};
use keytao_core::ImeState;
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};
use zbus::{interface, object_server::SignalContext, proxy, zvariant};

// ── IBus constants ────────────────────────────────────────────────────────────

const IBUS_ORIENTATION_SYSTEM: i32 = 2;
const MOD_CONTROL: u32 = 0x0004;
const MOD_MOD1: u32 = 0x0008;
const RELEASE_MASK: u32 = 1 << 30;

fn is_shift_key(sym: u32) -> bool {
    matches!(sym, 0xffe1 | 0xffe2)
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

fn candidate_index_for_select_key(sym: u32, state: &ImeState) -> Option<usize> {
    if state.candidates.is_empty() {
        return None;
    }
    let keys = state.select_keys.as_deref().unwrap_or("1234567890");
    let ch = char::from_u32(sym)?;
    keys.chars().position(|k| k == ch)
}

// ── IBus type builders ────────────────────────────────────────────────────────

fn ibus_text_variant(text: &str) -> zvariant::Value<'static> {
    use zvariant::{Array, Dict, Signature, StructureBuilder, Value};
    let sig_s = Signature::try_from("s").unwrap();
    let sig_v = Signature::try_from("v").unwrap();
    let empty_dict1 = Dict::new(sig_s.clone(), sig_v.clone());
    let empty_array = Array::new(sig_v.clone());
    let attr_list = StructureBuilder::new()
        .add_field("IBusAttrList".to_owned())
        .append_field(Value::Dict(empty_dict1))
        .append_field(Value::Array(empty_array))
        .build();
    let attr_list_variant = Value::Value(Box::new(Value::Structure(attr_list)));
    let empty_dict2 = Dict::new(sig_s, sig_v);
    let ibus_text = StructureBuilder::new()
        .add_field("IBusText".to_owned())
        .append_field(Value::Dict(empty_dict2))
        .add_field(text.to_owned())
        .append_field(attr_list_variant)
        .build();
    Value::Structure(ibus_text)
}

fn ibus_text_value(text: &str) -> zvariant::OwnedValue {
    zvariant::OwnedValue::try_from(ibus_text_variant(text)).expect("ibus_text_value")
}

fn candidate_display_text(c: &keytao_core::Candidate) -> String {
    match c.comment.as_deref().filter(|s| !s.is_empty()) {
        Some(comment) => format!("{} {}", c.text, comment),
        None => c.text.clone(),
    }
}

fn candidate_label(index: usize, select_keys: Option<&str>) -> String {
    select_keys
        .and_then(|keys| keys.chars().nth(index))
        .or_else(|| "1234567890".chars().nth(index))
        .map(|ch| ch.to_string())
        .unwrap_or_else(|| (index + 1).to_string())
}

fn ibus_lookup_table_value(state: &ImeState) -> zvariant::OwnedValue {
    use zvariant::{Array, Dict, Signature, StructureBuilder, Value};
    let sig_s = Signature::try_from("s").unwrap();
    let sig_v = Signature::try_from("v").unwrap();
    let empty_dict = Dict::new(sig_s, sig_v.clone());
    let mut candidates = Array::new(sig_v.clone());
    for c in &state.candidates {
        let wrapped = Value::Value(Box::new(ibus_text_variant(&candidate_display_text(c))));
        candidates.append(wrapped).ok();
    }
    let mut labels = Array::new(sig_v);
    let select_keys = state.select_keys.as_deref();
    for i in 0..state.candidates.len() {
        let wrapped = Value::Value(Box::new(ibus_text_variant(&candidate_label(
            i,
            select_keys,
        ))));
        labels.append(wrapped).ok();
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
        .add_field(true) // cursor_visible
        .add_field(false) // round
        .add_field(IBUS_ORIENTATION_SYSTEM)
        .append_field(Value::Array(candidates))
        .append_field(Value::Array(labels))
        .build();
    zvariant::OwnedValue::try_from(Value::Structure(table)).expect("ibus_lookup_table_value")
}

/// Build an IBusEngineDesc OwnedValue.
fn ibus_engine_desc() -> zvariant::OwnedValue {
    use zvariant::{Dict, Signature, StructureBuilder, Value};
    let sig_s = Signature::try_from("s").unwrap();
    let sig_v = Signature::try_from("v").unwrap();
    let empty_dict = Dict::new(sig_s, sig_v);
    let engine = StructureBuilder::new()
        .add_field("IBusEngineDesc".to_owned())
        .append_field(Value::Dict(empty_dict))
        .add_field("keytao".to_owned())
        .add_field("键道".to_owned())
        .add_field("KeyTao Input Method".to_owned())
        .add_field("zh".to_owned())
        .add_field("".to_owned())
        .add_field("".to_owned())
        .add_field("".to_owned())
        .add_field("default".to_owned())
        .add_field(0u32)
        .add_field("".to_owned())
        .add_field("键".to_owned())
        .add_field("".to_owned())
        .add_field("".to_owned())
        .add_field("".to_owned())
        .add_field("".to_owned())
        .add_field("".to_owned())
        .build();
    zvariant::OwnedValue::try_from(Value::Structure(engine)).expect("ibus_engine_desc")
}

/// Build an IBusComponent OwnedValue containing the keytao engine descriptor.
/// D-Bus type: (sa{sv}ssssssssavavav)
fn ibus_component() -> zvariant::OwnedValue {
    use zvariant::{Array, Dict, OwnedValue, Signature, StructureBuilder, Value};

    let sig_s = Signature::try_from("s").unwrap();
    let sig_v = Signature::try_from("v").unwrap();
    let empty_dict = Dict::new(sig_s, sig_v.clone());

    // engines array: av of IBusEngineDesc (each wrapped as v inside av)
    let mut engines = Array::new(sig_v.clone());
    let engine_ov: OwnedValue = ibus_engine_desc();
    let engine_v = Value::try_from(engine_ov).expect("engine value");
    engines
        .append(Value::Value(Box::new(engine_v)))
        .expect("append engine");

    let empty_paths = Array::new(sig_v.clone());
    let empty_sub = Array::new(sig_v);

    let exec = std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(String::from))
        .unwrap_or_else(|| "keytao-ime".to_string());

    let component = StructureBuilder::new()
        .add_field("IBusComponent".to_owned())
        .append_field(Value::Dict(empty_dict))
        .add_field("org.freedesktop.IBus.KeyTao".to_owned())
        .add_field("KeyTao Input Method".to_owned())
        .add_field(env!("CARGO_PKG_VERSION").to_owned())
        .add_field("free".to_owned())
        .add_field("KeyTao Team".to_owned())
        .add_field("https://keytao.app".to_owned())
        .add_field(format!("{exec} --ibus-engine"))
        .add_field("keytao".to_owned())
        .append_field(Value::Array(empty_paths)) // observed_paths
        .append_field(Value::Array(empty_sub)) // sub_components
        .append_field(Value::Array(engines)) // engines
        .build();

    zvariant::OwnedValue::try_from(Value::Structure(component)).expect("ibus_component")
}

// ── IBusBus proxy (to call RegisterComponent on the existing daemon) ──────────

#[proxy(
    interface = "org.freedesktop.IBus",
    default_service = "org.freedesktop.IBus",
    default_path = "/org/freedesktop/IBus"
)]
trait IBusBusDaemon {
    async fn register_component(&self, component: &zvariant::Value<'_>) -> zbus::Result<()>;
}

// ── IBus Factory (we expose this so the daemon can call CreateEngine) ─────────

pub struct IBusFactory {
    engine: CoreEngine,
    counter: Arc<AtomicU32>,
}

#[interface(name = "org.freedesktop.IBus.Factory")]
impl IBusFactory {
    async fn create_engine(
        &self,
        engine_name: &str,
        #[zbus(object_server)] server: &zbus::ObjectServer,
    ) -> zbus::fdo::Result<zvariant::OwnedObjectPath> {
        tracing::info!("IBus Factory: CreateEngine({engine_name})");
        let n = self.counter.fetch_add(1, Ordering::SeqCst);
        let path = format!("/org/freedesktop/IBus/engines/keytao{n}");
        let session = self
            .engine
            .create_session()
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        server
            .at(path.clone(), IBusEngine { session })
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        zvariant::OwnedObjectPath::try_from(path)
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))
    }
}

// ── IBus Engine ───────────────────────────────────────────────────────────────

pub struct IBusEngine {
    session: ImeSession,
}

#[interface(name = "org.freedesktop.IBus.Engine")]
impl IBusEngine {
    // ── Signals (emitted by our engine to the daemon) ─────────────────────

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
    async fn show_preedit_text(ctxt: &SignalContext<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn hide_preedit_text(ctxt: &SignalContext<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn update_lookup_table(
        ctxt: &SignalContext<'_>,
        table: zvariant::Value<'_>,
        visible: bool,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn show_lookup_table(ctxt: &SignalContext<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn hide_lookup_table(ctxt: &SignalContext<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn forward_key_event(
        ctxt: &SignalContext<'_>,
        keyval: u32,
        keycode: u32,
        state: u32,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn register_properties(
        ctxt: &SignalContext<'_>,
        props: zvariant::Value<'_>,
    ) -> zbus::Result<()>;

    // ── Methods (called by the daemon on key/focus events) ────────────────

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
                    return result.accepted;
                }
            }
            return false;
        }

        tracing::debug!("IBus Engine ProcessKeyEvent keyval={keyval:#x} state={state:#x}");

        let before_state = self.session.state();
        if should_bypass_empty_composition(keyval, state, &before_state) {
            let _ = IBusEngine::hide_preedit_text(&ctxt).await;
            let _ = IBusEngine::hide_lookup_table(&ctxt).await;
            return false;
        }

        if is_enter_key(keyval) && !before_state.preedit.is_empty() {
            let ov = ibus_text_value(&before_state.preedit);
            if let Ok(v) = zvariant::Value::try_from(&ov) {
                let _ =
                    IBusEngine::update_preedit_text(&ctxt, ibus_text_variant(""), 0, false).await;
                let _ = IBusEngine::commit_text(&ctxt, v).await;
            }
            self.session.reset();
            let _ = IBusEngine::hide_lookup_table(&ctxt).await;
            return true;
        }

        // Candidate selection via space or number keys
        let candidate_select_index = if keyval == 0x0020 {
            // space: select highlighted candidate
            if !before_state.candidates.is_empty() {
                Some(
                    before_state
                        .highlighted_candidate_index
                        .min(before_state.candidates.len().saturating_sub(1)),
                )
            } else {
                None
            }
        } else {
            candidate_index_for_select_key(keyval, &before_state)
        };

        if let Some(index) = candidate_select_index {
            if !before_state.candidates.is_empty() {
                if let Some(ime_state) = self.session.select_candidate(index) {
                    if let Some(ref text) = ime_state.committed {
                        if !text.is_empty() {
                            let _ = IBusEngine::update_preedit_text(
                                &ctxt,
                                ibus_text_variant(""),
                                0,
                                false,
                            )
                            .await;
                            let ov = ibus_text_value(text);
                            if let Ok(v) = zvariant::Value::try_from(&ov) {
                                let _ = IBusEngine::commit_text(&ctxt, v).await;
                            }
                        }
                    }
                    let _ = IBusEngine::hide_preedit_text(&ctxt).await;
                    let _ = IBusEngine::hide_lookup_table(&ctxt).await;
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
                tracing::debug!("IBus Engine CommitText: {text:?}");
                let _ =
                    IBusEngine::update_preedit_text(&ctxt, ibus_text_variant(""), 0, false).await;
                let ov = ibus_text_value(text);
                if let Ok(v) = zvariant::Value::try_from(&ov) {
                    let _ = IBusEngine::commit_text(&ctxt, v).await;
                }
            }
        }

        if ime_state.preedit.is_empty() {
            let _ = IBusEngine::hide_preedit_text(&ctxt).await;
        } else {
            let cursor = ime_state.cursor as u32;
            let ov = ibus_text_value(&ime_state.preedit);
            if let Ok(v) = zvariant::Value::try_from(&ov) {
                let _ = IBusEngine::update_preedit_text(&ctxt, v, cursor, true).await;
            }
        }

        if ime_state.candidates.is_empty() {
            let _ = IBusEngine::hide_lookup_table(&ctxt).await;
        } else {
            let ov = ibus_lookup_table_value(&ime_state);
            if let Ok(v) = zvariant::Value::try_from(&ov) {
                let _ = IBusEngine::update_lookup_table(&ctxt, v, true).await;
            }
        }

        consumed
    }

    async fn focus_in(&self) {
        tracing::debug!("IBus Engine: FocusIn");
    }

    async fn focus_out(&self, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        self.session.reset();
        let _ = IBusEngine::hide_preedit_text(&ctxt).await;
        let _ = IBusEngine::hide_lookup_table(&ctxt).await;
    }

    async fn reset(&self, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        self.session.reset();
        let _ = IBusEngine::hide_preedit_text(&ctxt).await;
        let _ = IBusEngine::hide_lookup_table(&ctxt).await;
    }

    async fn enable(&self) {
        tracing::debug!("IBus Engine: Enable");
    }

    async fn disable(&self, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        self.session.reset();
        let _ = IBusEngine::hide_preedit_text(&ctxt).await;
        let _ = IBusEngine::hide_lookup_table(&ctxt).await;
    }

    async fn set_cursor_location(&self, _x: i32, _y: i32, _w: i32, _h: i32) {}
    async fn set_capabilities(&self, _caps: u32) {}
    async fn set_surrounding_text(&self, _text: zvariant::Value<'_>, _cursor: u32, _anchor: u32) {}
    async fn set_content_type(&self, _purpose: u32, _hints: u32) {}

    async fn page_up(&self, #[zbus(signal_context)] _ctxt: SignalContext<'_>) {}
    async fn page_down(&self, #[zbus(signal_context)] _ctxt: SignalContext<'_>) {}
    async fn cursor_up(&self, #[zbus(signal_context)] _ctxt: SignalContext<'_>) {}
    async fn cursor_down(&self, #[zbus(signal_context)] _ctxt: SignalContext<'_>) {}
    async fn candidate_clicked(
        &self,
        _index: u32,
        _button: u32,
        _state: u32,
        #[zbus(signal_context)] _ctxt: SignalContext<'_>,
    ) {
    }

    #[zbus(property)]
    fn name(&self) -> String {
        "keytao".to_string()
    }
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Connect to the existing IBus daemon (GNOME's ibus-daemon) and register
/// as an IBus engine.  Blocks until the connection drops or the process exits.
pub async fn run(engine: CoreEngine) {
    tracing::info!("IBus engine backend starting (GNOME mode)");

    let factory = IBusFactory {
        engine,
        counter: Arc::new(AtomicU32::new(0)),
    };

    let conn = match zbus::connection::Builder::session()
        .and_then(|b| b.serve_at("/org/freedesktop/IBus/Factory", factory))
        .map(|b| b.build())
    {
        Ok(fut) => match fut.await {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("IBus engine backend: failed to connect to session bus: {e}");
                return;
            }
        },
        Err(e) => {
            tracing::error!("IBus engine backend: builder error: {e}");
            return;
        }
    };

    tracing::info!(
        "IBus engine factory ready at /org/freedesktop/IBus/Factory (unique name: {})",
        conn.unique_name().map(|n| n.as_str()).unwrap_or("?")
    );

    // Register our component with the running IBus daemon so it knows about
    // the "keytao" engine and can call CreateEngine when the user selects it.
    let component_ov = ibus_component();
    match zvariant::Value::try_from(&component_ov) {
        Ok(component_v) => match IBusBusDaemonProxy::new(&conn).await {
            Ok(proxy) => match proxy.register_component(&component_v).await {
                Ok(()) => tracing::info!("IBus component registered with daemon"),
                Err(e) => tracing::warn!(
                    "IBus RegisterComponent failed (daemon may require XML component file): {e}"
                ),
            },
            Err(e) => tracing::warn!("IBus daemon proxy failed: {e}"),
        },
        Err(e) => tracing::warn!("Failed to serialize IBus component: {e}"),
    }

    tracing::info!(
        "IBus engine backend ready. \
         To activate: open ibus-setup or GNOME Settings → Keyboard → Input Sources \
         and add 'KeyTao' (Other → Chinese → KeyTao)."
    );

    let _conn = conn;
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
    }
}
