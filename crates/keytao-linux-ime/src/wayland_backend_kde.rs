//! KDE/KWin Wayland backend: zwp_input_method_unstable_v1.
//!
//! KWin 6 still exposes input methods through input-method-v1. Applications talk
//! text-input-v1/v2/v3 to KWin; the IME process talks input-method-v1 to KWin.

use std::{
    os::fd::{AsFd, AsRawFd},
    time::{Duration, Instant},
};

use keytao_core::ImeState;
use wayland_client::{
    delegate_noop,
    globals::{registry_queue_init, GlobalListContents},
    protocol::{
        wl_keyboard::{self, WlKeyboard},
        wl_registry,
        wl_seat::WlSeat,
    },
    Connection, Dispatch, QueueHandle, WEnum,
};
use wayland_protocols::wp::input_method::zv1::client::{
    zwp_input_method_context_v1::{self, ZwpInputMethodContextV1},
    zwp_input_method_v1::{self, ZwpInputMethodV1},
};
use xkbcommon::xkb;

use crate::engine::{CoreEngine, ImeSession};

const MOD_SHIFT: u32 = 0x0001;
const MOD_CONTROL: u32 = 0x0004;
const MOD_MOD1: u32 = 0x0008;
const RELEASE_MASK: u32 = 1 << 30;
const DEACTIVATE_DEBOUNCE: Duration = Duration::from_millis(180);

fn is_shift_key(sym: u32) -> bool {
    sym == xkb::keysyms::KEY_Shift_L || sym == xkb::keysyms::KEY_Shift_R
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

fn should_forward_consumed_shortcut(sym_raw: u32, effective_mods: u32) -> bool {
    let ctrl_held = effective_mods & MOD_CONTROL != 0;
    ctrl_held
        && matches!(
            sym_raw,
            xkb::keysyms::KEY_grave | xkb::keysyms::KEY_asciitilde
        )
}

struct App {
    session: ImeSession,
    seat: Option<WlSeat>,
    input_method: Option<ZwpInputMethodV1>,
    context: Option<ZwpInputMethodContextV1>,
    keyboard: Option<WlKeyboard>,
    serial: u32,
    active: bool,
    deactivate_deadline: Option<Instant>,
    xkb_context: xkb::Context,
    xkb_keymap: Option<xkb::Keymap>,
    xkb_state: Option<xkb::State>,
    mods: u32,
    last_key_time: u32,
    ascii_mode: bool,
}

impl App {
    fn new(session: ImeSession) -> Self {
        Self {
            session,
            seat: None,
            input_method: None,
            context: None,
            keyboard: None,
            serial: 0,
            active: false,
            deactivate_deadline: None,
            xkb_context: xkb::Context::new(xkb::CONTEXT_NO_FLAGS),
            xkb_keymap: None,
            xkb_state: None,
            mods: 0,
            last_key_time: 0,
            ascii_mode: false,
        }
    }

    fn key_sym(&self, evdev_keycode: u32) -> u32 {
        let keycode = evdev_keycode + 8;
        self.xkb_state
            .as_ref()
            .map(|s| s.key_get_one_sym(xkb::Keycode::from(keycode)))
            .unwrap_or(xkb::Keysym::from(xkb::keysyms::KEY_NoSymbol))
            .into()
    }

    fn reset_context_state(&mut self) {
        self.active = false;
        self.deactivate_deadline = None;
        self.keyboard = None;
        self.context = None;
        self.session.reset();
    }

    fn commit_state_to_context(&self, state: &ImeState) {
        let Some(ctx) = &self.context else { return };
        if let Some(committed) = &state.committed {
            ctx.commit_string(self.serial, committed.clone());
        }
        let preedit = state.preedit.clone();
        let cursor = preedit.len() as i32;
        ctx.preedit_cursor(cursor);
        ctx.preedit_string(self.serial, preedit, String::new());
    }

    fn clear_context_preedit(&self) {
        if let Some(ctx) = &self.context {
            ctx.preedit_cursor(0);
            ctx.preedit_string(self.serial, String::new(), String::new());
        }
    }

    fn forward_key(&self, evdev_keycode: u32, state: u32) {
        if let Some(ctx) = &self.context {
            ctx.key(self.serial, self.last_key_time, evdev_keycode, state);
        }
    }

    fn forward_modifiers(
        &self,
        serial: u32,
        mods_depressed: u32,
        mods_latched: u32,
        mods_locked: u32,
        group: u32,
    ) {
        if let Some(ctx) = &self.context {
            ctx.modifiers(serial, mods_depressed, mods_latched, mods_locked, group);
        }
    }

    fn handle_key_event(
        &mut self,
        evdev_keycode: u32,
        key_state: wl_keyboard::KeyState,
        qh: &QueueHandle<Self>,
    ) {
        if key_state == wl_keyboard::KeyState::Released {
            self.handle_key_release(evdev_keycode);
            return;
        }

        if !self.active {
            self.forward_key(evdev_keycode, wl_keyboard::KeyState::Pressed as u32);
            return;
        }

        let sym_raw = self.key_sym(evdev_keycode);
        if sym_raw == xkb::keysyms::KEY_NoSymbol {
            tracing::warn!("KDE IME key dropped: NoSymbol");
            return;
        }

        let effective_mods = if is_shift_key(sym_raw) {
            self.mods & !MOD_SHIFT
        } else {
            self.mods
        };

        let before_state = self.session.state();
        if should_bypass_empty_composition(sym_raw, effective_mods, &before_state) {
            self.forward_key(evdev_keycode, wl_keyboard::KeyState::Pressed as u32);
            return;
        }

        if is_enter_key(sym_raw) && !before_state.preedit.is_empty() {
            if let Some(ctx) = &self.context {
                ctx.commit_string(self.serial, before_state.preedit.clone());
            }
            self.clear_context_preedit();
            self.session.reset();
            return;
        }

        if is_candidate_select_key(sym_raw) && !before_state.candidates.is_empty() {
            let index = before_state
                .highlighted_candidate_index
                .min(before_state.candidates.len().saturating_sub(1));
            if let Some(ime_state) = self.session.select_candidate(index) {
                self.commit_state_to_context(&ime_state);
                return;
            }
        }

        let Some(result) = self.session.process_key_result(sym_raw, effective_mods) else {
            self.forward_key(evdev_keycode, wl_keyboard::KeyState::Pressed as u32);
            return;
        };
        let ime_state = result.state;

        if ime_state.ascii_mode != self.ascii_mode {
            self.ascii_mode = ime_state.ascii_mode;
            tracing::info!(
                "IME mode changed: {}",
                if self.ascii_mode { "EN" } else { "CN" }
            );
        }

        if result.accepted {
            self.commit_state_to_context(&ime_state);
            if should_forward_consumed_shortcut(sym_raw, effective_mods) {
                self.forward_key(evdev_keycode, wl_keyboard::KeyState::Pressed as u32);
            }
        } else {
            self.clear_context_preedit();
            self.forward_key(evdev_keycode, wl_keyboard::KeyState::Pressed as u32);
        }

        let _ = qh;
    }

    fn handle_key_release(&mut self, evdev_keycode: u32) {
        let sym_raw = self.key_sym(evdev_keycode);
        if is_shift_key(sym_raw) {
            if let Some(result) = self.session.process_key_result(sym_raw, RELEASE_MASK) {
                if result.state.ascii_mode != self.ascii_mode {
                    self.ascii_mode = result.state.ascii_mode;
                    tracing::info!(
                        "IME mode changed: {}",
                        if self.ascii_mode { "EN" } else { "CN" }
                    );
                }
            }
        }
        self.forward_key(evdev_keycode, wl_keyboard::KeyState::Released as u32);
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for App {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_seat" => state.seat = Some(registry.bind(name, version.min(7), qh, ())),
                "zwp_input_method_v1" => {
                    state.input_method = Some(registry.bind(name, 1, qh, ()));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<ZwpInputMethodV1, ()> for App {
    fn event(
        state: &mut Self,
        _proxy: &ZwpInputMethodV1,
        event: zwp_input_method_v1::Event,
        _: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            zwp_input_method_v1::Event::Activate { id } => {
                tracing::debug!("KDE input-method-v1 activated");
                state.reset_context_state();
                state.keyboard = Some(id.grab_keyboard(qh, ()));
                state.context = Some(id);
                state.active = true;
            }
            zwp_input_method_v1::Event::Deactivate { context: _ } => {
                tracing::debug!("KDE input-method-v1 deactivate pending");
                state.deactivate_deadline = Some(Instant::now() + DEACTIVATE_DEBOUNCE);
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwpInputMethodContextV1, ()> for App {
    fn event(
        state: &mut Self,
        _proxy: &ZwpInputMethodContextV1,
        event: zwp_input_method_context_v1::Event,
        _: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            zwp_input_method_context_v1::Event::CommitState { serial } => {
                state.serial = serial;
            }
            zwp_input_method_context_v1::Event::Reset => {
                state.session.reset();
                state.clear_context_preedit();
            }
            _ => {}
        }
    }
}

impl Dispatch<WlKeyboard, ()> for App {
    fn event(
        state: &mut Self,
        _proxy: &WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Keymap { format, fd, size } => {
                if format != WEnum::Value(wl_keyboard::KeymapFormat::XkbV1) {
                    tracing::warn!("KDE keyboard grab: unexpected keymap format {format:?}");
                    return;
                }
                let Ok(mmap) = memmap2::MmapOptions::new()
                    .len(size as usize)
                    .map_raw_read_only(&fd)
                else {
                    return;
                };
                let keymap_bytes =
                    unsafe { std::slice::from_raw_parts(mmap.as_ptr(), size as usize) };
                let keymap_text = keymap_bytes.strip_suffix(&[0]).unwrap_or(keymap_bytes);
                let keymap_string = String::from_utf8_lossy(keymap_text).into_owned();
                if let Some(km) = xkb::Keymap::new_from_string(
                    &state.xkb_context,
                    keymap_string,
                    xkb::KEYMAP_FORMAT_TEXT_V1,
                    xkb::KEYMAP_COMPILE_NO_FLAGS,
                ) {
                    state.xkb_state = Some(xkb::State::new(&km));
                    state.xkb_keymap = Some(km);
                }
            }
            wl_keyboard::Event::Key {
                serial,
                time,
                key,
                state: ks,
            } => {
                state.serial = serial;
                state.last_key_time = time;
                if let WEnum::Value(key_state) = ks {
                    state.handle_key_event(key, key_state, qh);
                }
            }
            wl_keyboard::Event::Modifiers {
                serial,
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
            } => {
                if let Some(xkb_state) = &mut state.xkb_state {
                    xkb_state.update_mask(mods_depressed, mods_latched, mods_locked, 0, 0, group);
                    let mut m = 0u32;
                    if xkb_state.mod_name_is_active(xkb::MOD_NAME_SHIFT, xkb::STATE_MODS_EFFECTIVE)
                    {
                        m |= MOD_SHIFT;
                    }
                    if xkb_state.mod_name_is_active(xkb::MOD_NAME_CTRL, xkb::STATE_MODS_EFFECTIVE) {
                        m |= MOD_CONTROL;
                    }
                    if xkb_state.mod_name_is_active(xkb::MOD_NAME_ALT, xkb::STATE_MODS_EFFECTIVE) {
                        m |= MOD_MOD1;
                    }
                    state.mods = m;
                }
                state.forward_modifiers(serial, mods_depressed, mods_latched, mods_locked, group);
            }
            _ => {}
        }
    }
}

delegate_noop!(App: ignore WlSeat);

pub fn run(engine: CoreEngine) -> Result<(), String> {
    let session = engine
        .create_session()
        .map_err(|e| format!("failed to create KDE Wayland Rime session: {e}"))?;
    let conn = Connection::connect_to_env().map_err(|e| format!("KDE Wayland connection: {e}"))?;
    let (_globals, mut queue) =
        registry_queue_init::<App>(&conn).map_err(|e| format!("KDE Wayland registry: {e}"))?;

    let mut app = App::new(session);
    queue.roundtrip(&mut app).expect("initial KDE roundtrip");
    if app.input_method.is_none() {
        return Err(
            "KWin did not advertise zwp_input_method_v1 on the WAYLAND_SOCKET connection".into(),
        );
    }

    tracing::info!("KDE Wayland IME running (input-method-v1)");
    loop {
        if app
            .deactivate_deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            app.reset_context_state();
            tracing::debug!("KDE input-method-v1 deactivated after debounce");
        }

        if let Err(e) = queue.flush() {
            tracing::warn!("KDE Wayland flush error: {e}");
        }
        let timeout_ms = if app.deactivate_deadline.is_some() {
            100
        } else {
            -1
        };
        let raw_fd = conn.as_fd().as_raw_fd();
        let mut pfd = libc::pollfd {
            fd: raw_fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut pfd as *mut libc::pollfd, 1, timeout_ms) };
        if ready > 0 {
            if let Err(e) = queue.blocking_dispatch(&mut app) {
                tracing::warn!("KDE Wayland dispatch error: {e}");
                return Err(format!("KDE Wayland connection closed: {e}"));
            }
        } else if ready < 0 {
            tracing::warn!("KDE Wayland poll failed");
        }
    }
}
