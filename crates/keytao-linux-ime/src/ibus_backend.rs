//! IBus D-Bus backend for keytao-ime.
//!
//! Implements enough of the IBus D-Bus protocol so that Chromium/CEF apps
//! (e.g. WeChatAppEx) can use keytao as their IME without requiring a real
//! IBus daemon.

use crate::engine::CoreEngine;
use std::{
    fs,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
};
use zbus::{connection, interface, object_server::SignalContext, zvariant};

// ── IBus text helper ─────────────────────────────────────────────────────────

/// Build an IBusText structure as an OwnedValue (variant).
/// IBus D-Bus type: v containing (sa{sv}sv)
///   ("IBusText", {}, text_string, v:("IBusAttrList",{},[]))
fn ibus_text_value(text: &str) -> zvariant::OwnedValue {
    use zvariant::{Array, Dict, OwnedValue, Signature, StructureBuilder, Value};

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

    // Wrap as variant (signals take `v`)
    let outer = Value::Value(Box::new(Value::Structure(ibus_text_struct)));
    OwnedValue::try_from(outer).expect("ibus_text_value")
}

// ── InputContext D-Bus object ─────────────────────────────────────────────────

struct InputContext {
    engine: CoreEngine,
}

#[interface(name = "org.freedesktop.IBus.InputContext")]
impl InputContext {
    async fn focus_in(&self) {
        tracing::debug!("IBus InputContext: FocusIn");
    }

    async fn focus_out(&self) {
        tracing::debug!("IBus InputContext: FocusOut");
        self.engine.reset();
    }

    async fn reset(&self) {
        tracing::debug!("IBus InputContext: Reset");
        self.engine.reset();
    }

    async fn set_cursor_location(&self, _x: i32, _y: i32, _w: i32, _h: i32) {}
    async fn set_cursor_location_relative(&self, _x: i32, _y: i32, _w: i32, _h: i32) {}
    async fn set_capabilities(&self, _caps: u32) {}

    /// Process a key event. Returns true if consumed by the IME.
    async fn process_key_event(
        &self,
        keyval: u32,
        _keycode: u32,
        state: u32,
        #[zbus(signal_context)] ctxt: SignalContext<'_>,
    ) -> bool {
        // Ignore key-release events (IBUS_RELEASE_MASK = bit 30)
        if state & (1 << 30) != 0 {
            return false;
        }

        tracing::debug!("IBus ProcessKeyEvent keyval={keyval:#x} state={state:#x}");

        let result = match self.engine.process_key_result(keyval, state) {
            Some(r) => r,
            None => return false,
        };

        let ime_state = result.state;

        let consumed = result.accepted;

        if let Some(ref text) = ime_state.committed {
            if !text.is_empty() {
                tracing::debug!("IBus CommitText: {text:?}");
                let ov = ibus_text_value(text);
                if let Ok(v) = zvariant::Value::try_from(&ov) {
                    let _ = Self::commit_text(&ctxt, v).await;
                }
            }
        }

        if ime_state.preedit.is_empty() {
            let _ = Self::hide_preedit_text(&ctxt).await;
        } else {
            let cursor = ime_state.cursor as u32;
            let ov = ibus_text_value(&ime_state.preedit);
            if let Ok(v) = zvariant::Value::try_from(&ov) {
                let _ = Self::update_preedit_text(&ctxt, v, cursor, true).await;
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

// ── IBusBus D-Bus object ──────────────────────────────────────────────────────

struct IBusBus {
    engine: CoreEngine,
    ctx_counter: Arc<AtomicU32>,
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

        tracing::info!("IBus CreateInputContext client={client_name:?} → {path_str}");

        let ctx = InputContext {
            engine: self.engine.clone(),
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
        Err(zbus::fdo::Error::Failed("no global engine".into()))
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
            tracing::info!("wrote IBus address file: {}", path.display());
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
    let builder = match builder.serve_at(
        "/org/freedesktop/IBus",
        IBusBus {
            engine,
            ctx_counter: Arc::new(AtomicU32::new(1)),
        },
    ) {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("IBus: failed to serve_at: {e}");
            return;
        }
    };

    let _conn = match builder.build().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("IBus D-Bus backend failed to connect: {e}");
            return;
        }
    };

    let dbus_address = std::env::var("DBUS_SESSION_BUS_ADDRESS")
        .unwrap_or_else(|_| "unix:path=/run/user/1000/bus".to_owned());

    write_ibus_address_files(&dbus_address);

    tracing::info!("IBus D-Bus backend ready ({})", dbus_address);

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
    }
}
