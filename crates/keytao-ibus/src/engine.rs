//! ibus D-Bus engine implementation.
//!
//! IBus uses GVariant serialisation for text/lookup-table structs.
//! The helpers below produce the minimal variant format ibus-daemon accepts.
//!
//! Reference: https://github.com/ibus/ibus/blob/main/src/ibusenginedesc.h
//!            https://github.com/ibus/ibus/blob/main/src/ibusengine.h

use keytao_core::{default_shared_data_dir, default_user_data_dir, deploy, Engine, ImeState};
use std::sync::Mutex;
use tracing::{error, info};
use zbus::{connection, interface, object_server::SignalEmitter, zvariant::Value};

// ── D-Bus object ──────────────────────────────────────────────────────────────

struct KeyTaoEngine {
    engine: Mutex<Option<Engine>>,
}

impl KeyTaoEngine {
    fn new() -> Self {
        Self {
            engine: Mutex::new(None),
        }
    }

    fn with_engine<F, T>(&self, f: F) -> Option<T>
    where
        F: FnOnce(&Engine) -> T,
    {
        self.engine.lock().unwrap().as_ref().map(f)
    }
}

// ── ibus Engine D-Bus interface ───────────────────────────────────────────────
//
// This implements the org.freedesktop.IBus.Engine interface that ibus-daemon calls.
// Methods that change IME state must emit the appropriate signals back.

#[interface(name = "org.freedesktop.IBus.Engine")]
impl KeyTaoEngine {
    /// Called by ibus-daemon for every key press/release.
    /// Returns true if the key event was consumed (suppressed from the application).
    async fn process_key_event(
        &self,
        keyval: u32,
        _keycode: u32,
        state: u32,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<bool> {
        // Ignore key-release events (bit 30 set in IBus modifier state)
        if state & (1 << 30) != 0 {
            return Ok(false);
        }

        let ime_state = match self.with_engine(|e| e.process_key(keyval, state)) {
            Some(s) => s,
            None => return Ok(false),
        };

        let consumed = !ime_state.preedit.is_empty()
            || ime_state.committed.is_some()
            || !ime_state.candidates.is_empty();

        update_display(&emitter, &ime_state).await?;

        Ok(consumed)
    }

    async fn enable(
        &self,
        #[zbus(signal_emitter)] _emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        info!("engine enabled");
        Ok(())
    }

    async fn disable(
        &self,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        if let Some(state) = self.with_engine(|e| e.reset()) {
            update_display(&emitter, &state).await?;
        }
        Ok(())
    }

    async fn focus_in(
        &self,
        #[zbus(signal_emitter)] _emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        Ok(())
    }

    async fn focus_out(
        &self,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        if let Some(state) = self.with_engine(|e| e.reset()) {
            update_display(&emitter, &state).await?;
        }
        Ok(())
    }

    async fn reset(
        &self,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        if let Some(state) = self.with_engine(|e| e.reset()) {
            update_display(&emitter, &state).await?;
        }
        Ok(())
    }

    async fn set_capabilities(
        &self,
        _caps: u32,
        #[zbus(signal_emitter)] _emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        Ok(())
    }

    async fn cursor_up(
        &self,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        if let Some(state) = self.with_engine(|e| e.change_page(true)) {
            update_display(&emitter, &state).await?;
        }
        Ok(())
    }

    async fn cursor_down(
        &self,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        if let Some(state) = self.with_engine(|e| e.change_page(false)) {
            update_display(&emitter, &state).await?;
        }
        Ok(())
    }

    async fn candidate_clicked(
        &self,
        index: u32,
        _button: u32,
        _state: u32,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        if let Some(state) = self.with_engine(|e| e.select_candidate(index as usize)) {
            update_display(&emitter, &state).await?;
        }
        Ok(())
    }

    // ── Signals (declared so zbus generates them) ─────────────────────────────

    #[zbus(signal)]
    async fn commit_text(emitter: &SignalEmitter<'_>, text: Value<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn update_preedit_text(
        emitter: &SignalEmitter<'_>,
        text: Value<'_>,
        cursor_pos: u32,
        visible: bool,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn hide_preedit_text(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn update_lookup_table(
        emitter: &SignalEmitter<'_>,
        table: Value<'_>,
        visible: bool,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn hide_lookup_table(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

// ── Signal helpers ────────────────────────────────────────────────────────────

async fn update_display(
    emitter: &SignalEmitter<'_>,
    state: &ImeState,
) -> zbus::fdo::Result<()> {
    // Commit
    if let Some(ref text) = state.committed {
        let ibus_text = ibus_text(text);
        KeyTaoEngine::commit_text(emitter, ibus_text).await?;
    }

    // Preedit
    if state.preedit.is_empty() {
        KeyTaoEngine::hide_preedit_text(emitter).await?;
    } else {
        let ibus_text = ibus_text(&state.preedit);
        KeyTaoEngine::update_preedit_text(emitter, ibus_text, state.cursor as u32, true).await?;
    }

    // Candidates
    if state.candidates.is_empty() {
        KeyTaoEngine::hide_lookup_table(emitter).await?;
    } else {
        let table = ibus_lookup_table(state);
        KeyTaoEngine::update_lookup_table(emitter, table, true).await?;
    }

    Ok(())
}

/// Build a minimal IBusText GVariant: (sa{sv}sv) → ("IBusText", {}, "the-text", {})
fn ibus_text(text: &str) -> Value<'static> {
    // IBusText serialised as a D-Bus struct with type signature (sa{sv}sv)
    // For simplicity we use a plain string variant that ibus also accepts for CommitText.
    // A full IBusText would require zvariant::Structure; this covers the common path.
    Value::from(text.to_string())
}

/// Build a minimal IBusLookupTable variant from an ImeState.
fn ibus_lookup_table(state: &ImeState) -> Value<'static> {
    // Full IBusLookupTable is complex; returning candidate list as a string array
    // is sufficient for basic display. Replace with proper GVariant struct for
    // full feature parity (page cursor, orientation, round navigation, etc.).
    let candidates: Vec<String> = state.candidates.iter().map(|c| c.text.clone()).collect();
    Value::from(candidates)
}

// ── Entry point ───────────────────────────────────────────────────────────────

pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    // Resolve paths and deploy rime (blocking; fine at startup)
    let user = default_user_data_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let shared = default_shared_data_dir();

    info!("Deploying rime: user={user} shared={shared}");
    if let Err(e) = deploy(user, shared) {
        error!("Rime deployment failed: {e}");
        return Err(e.into());
    }

    let engine_obj = KeyTaoEngine::new();
    {
        let session = Engine::new()?;
        *engine_obj.engine.lock().unwrap() = Some(session);
    }

    info!("Connecting to ibus session D-Bus");
    let _conn = connection::Builder::session()?
        .name("org.freedesktop.IBus.KeyTao")?
        .serve_at("/org/freedesktop/IBus/Engine/KeyTao", engine_obj)?
        .build()
        .await?;

    info!("KeyTao ibus engine running");
    // Keep the process alive; ibus-daemon will call our methods via D-Bus.
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    }
}
