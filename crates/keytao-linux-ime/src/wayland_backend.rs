//! Wayland backend: zwp_input_method_v2 + zwp_input_popup_surface_v2.
//!
//! Protocol stack:
//!   zwp_input_method_manager_v2      — get the input method handle
//!   zwp_input_method_v2              — activate/deactivate + commit/preedit
//!   zwp_input_method_keyboard_grab_v2 — exclusive keyboard grab
//!   zwp_input_popup_surface_v2       — auto-positioned candidate panel at cursor
//!   wl_shm                           — shared-memory pixel buffers

use std::{
    collections::HashSet,
    fs::File,
    io::{Seek, SeekFrom, Write},
    os::fd::{AsFd, AsRawFd},
    time::{Duration, Instant},
};

use keytao_core::ImeState;
use wayland_client::{
    delegate_noop,
    globals::{registry_queue_init, GlobalListContents},
    protocol::{
        wl_buffer::WlBuffer,
        wl_compositor::WlCompositor,
        wl_keyboard::{self, WlKeyboard},
        wl_registry,
        wl_seat::WlSeat,
        wl_shm::{self, WlShm},
        wl_shm_pool::WlShmPool,
        wl_surface::WlSurface,
    },
    Connection, Dispatch, QueueHandle, WEnum,
};
use wayland_protocols_misc::zwp_input_method_v2::client::{
    zwp_input_method_keyboard_grab_v2::{self, ZwpInputMethodKeyboardGrabV2},
    zwp_input_method_manager_v2::ZwpInputMethodManagerV2,
    zwp_input_method_v2::{self, ZwpInputMethodV2},
    zwp_input_popup_surface_v2::{self, ZwpInputPopupSurfaceV2},
};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1,
    zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
};
use xkbcommon::xkb;

use crate::{
    engine::{CoreEngine, ImeSession},
    panel::{load_font, PanelRenderer},
};

const MOD_SHIFT: u32 = 0x0001;
const MOD_CONTROL: u32 = 0x0004;
const MOD_MOD1: u32 = 0x0008;
const WL_KEYMAP_FORMAT_XKB_V1: u32 = 1;
const WL_KEY_RELEASED: u32 = 0;
const WL_KEY_PRESSED: u32 = 1;
const RELEASE_MASK: u32 = 1 << 30;
const MODE_HINT_DURATION: Duration = Duration::from_secs(3);

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

struct App {
    session: ImeSession,
    renderer: Option<PanelRenderer>,

    compositor: Option<WlCompositor>,
    shm: Option<WlShm>,
    seat: Option<WlSeat>,
    ime_manager: Option<ZwpInputMethodManagerV2>,
    virtual_keyboard_manager: Option<ZwpVirtualKeyboardManagerV1>,
    wayland_unavailable: bool,

    input_method: Option<ZwpInputMethodV2>,
    keyboard_grab: Option<ZwpInputMethodKeyboardGrabV2>,
    virtual_keyboard: Option<ZwpVirtualKeyboardV1>,
    virtual_keymap: Option<File>,
    forwarded_keys: HashSet<u32>,
    last_key_time: u32,
    started_at: Instant,
    serial: u32,
    active: bool,

    xkb_context: xkb::Context,
    xkb_keymap: Option<xkb::Keymap>,
    xkb_state: Option<xkb::State>,
    mods: u32,

    // Popup panel — compositor auto-positions it at the cursor
    panel_surface: Option<WlSurface>,
    panel_popup: Option<ZwpInputPopupSurfaceV2>,
    panel_buffer: Option<WlBuffer>,
    panel_visible: bool,

    ime_state: Option<ImeState>,
    ascii_mode: bool,
    mode_hint_until: Option<Instant>,
}

impl App {
    fn new(session: ImeSession) -> Self {
        let renderer = load_font().and_then(PanelRenderer::new);
        if renderer.is_none() {
            tracing::warn!("panel renderer unavailable: no font found");
        }
        Self {
            session,
            renderer,
            compositor: None,
            shm: None,
            seat: None,
            ime_manager: None,
            virtual_keyboard_manager: None,
            input_method: None,
            keyboard_grab: None,
            virtual_keyboard: None,
            virtual_keymap: None,
            forwarded_keys: HashSet::new(),
            last_key_time: 0,
            started_at: Instant::now(),
            serial: 0,
            active: false,
            xkb_context: xkb::Context::new(xkb::CONTEXT_NO_FLAGS),
            xkb_keymap: None,
            xkb_state: None,
            mods: 0,
            panel_surface: None,
            panel_popup: None,
            panel_buffer: None,
            panel_visible: false,
            ime_state: None,
            ascii_mode: false,
            mode_hint_until: None,
            wayland_unavailable: false,
        }
    }

    fn setup_ime(&mut self, qh: &QueueHandle<Self>) {
        let (Some(manager), Some(seat)) = (&self.ime_manager, &self.seat) else {
            return;
        };
        let im = manager.get_input_method(seat, qh, ());
        self.input_method = Some(im);
        tracing::debug!("input method registered");
    }

    fn setup_virtual_keyboard(&mut self, qh: &QueueHandle<Self>) {
        let (Some(manager), Some(seat)) = (&self.virtual_keyboard_manager, &self.seat) else {
            tracing::warn!("Wayland virtual-keyboard unavailable; shortcuts may not pass through");
            return;
        };
        self.virtual_keyboard = Some(manager.create_virtual_keyboard(seat, qh, ()));
        tracing::debug!("virtual keyboard registered for unhandled key forwarding");
    }

    fn install_virtual_keymap(&mut self, keymap: &[u8]) {
        let Some(vk) = &self.virtual_keyboard else {
            return;
        };

        let mut file = match tempfile() {
            Ok(file) => file,
            Err(e) => {
                tracing::warn!("failed to create virtual keyboard keymap fd: {e}");
                return;
            }
        };
        if file
            .set_len(keymap.len() as u64)
            .and_then(|_| file.write_all(keymap))
            .and_then(|_| file.seek(SeekFrom::Start(0)).map(|_| ()))
            .is_err()
        {
            tracing::warn!("failed to write virtual keyboard keymap");
            return;
        }

        vk.keymap(WL_KEYMAP_FORMAT_XKB_V1, file.as_fd(), keymap.len() as u32);
        self.virtual_keymap = Some(file);
        tracing::debug!("virtual keyboard keymap installed ({} bytes)", keymap.len());
    }

    fn create_panel_popup(&mut self, qh: &QueueHandle<Self>) {
        let (Some(compositor), Some(im)) = (&self.compositor, &self.input_method) else {
            return;
        };
        let surface = compositor.create_surface(qh, ());
        let popup = im.get_input_popup_surface(&surface, qh, ());
        self.panel_surface = Some(surface);
        self.panel_popup = Some(popup);
    }

    fn hide_panel_popup(&mut self) {
        if let Some(surface) = &self.panel_surface {
            surface.attach(None, 0, 0);
            surface.commit();
        }
        if let Some(buf) = self.panel_buffer.take() {
            buf.destroy();
        }
        self.panel_visible = false;
        self.ime_state = None;
    }

    fn redraw_panel(&mut self, qh: &QueueHandle<Self>) {
        let (Some(renderer), Some(shm), Some(surface)) = (
            self.renderer.as_ref(),
            self.shm.as_ref(),
            self.panel_surface.as_ref(),
        ) else {
            return;
        };

        let show_hint = self.mode_hint_active();
        let (pixels, w, h) = if let Some(state) = self
            .ime_state
            .as_ref()
            .filter(|state| !state.candidates.is_empty())
        {
            renderer.render(state)
        } else if show_hint {
            renderer.render_mode_hint(self.ascii_mode)
        } else {
            return;
        };
        if w == 0 || h == 0 {
            return;
        }
        let stride = w * 4;
        let pool_size = (stride * h) as usize;

        let mut tmp = tempfile().expect("tempfile");
        tmp.write_all(&pixels).expect("write shm");
        let pool = shm.create_pool(tmp.as_fd(), pool_size as i32, qh, ());
        let buffer = pool.create_buffer(
            0,
            w as i32,
            h as i32,
            stride as i32,
            wl_shm::Format::Argb8888,
            qh,
            (),
        );

        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, w as i32, h as i32);
        surface.commit();

        if let Some(old) = self.panel_buffer.replace(buffer) {
            old.destroy();
        }
        pool.destroy();
    }

    fn show_panel(&mut self, state: ImeState, qh: &QueueHandle<Self>) {
        // The application already renders preedit through set_preedit_string.
        // Keep the popup for actual candidate menus and short mode hints only.
        let has_content = !state.candidates.is_empty();
        let show_hint = self.mode_hint_active();
        self.ime_state = Some(state);
        if has_content || show_hint {
            if self.panel_surface.is_none() {
                self.create_panel_popup(qh);
            }
            self.panel_visible = true;
            self.redraw_panel(qh);
        } else {
            self.hide_panel_popup();
        }
    }

    fn mode_hint_active(&self) -> bool {
        self.mode_hint_until
            .is_some_and(|deadline| Instant::now() < deadline)
    }

    fn update_ascii_mode(&mut self, ascii_mode: bool, qh: &QueueHandle<Self>) {
        if ascii_mode == self.ascii_mode {
            return;
        }
        self.ascii_mode = ascii_mode;
        self.mode_hint_until = Some(Instant::now() + MODE_HINT_DURATION);
        self.show_panel(ImeState::empty(), qh);
        tracing::info!("IME mode changed: {}", if ascii_mode { "EN" } else { "CN" });
    }

    fn key_sym(&self, evdev_keycode: u32) -> u32 {
        let keycode = evdev_keycode + 8;
        self.xkb_state
            .as_ref()
            .map(|s| s.key_get_one_sym(xkb::Keycode::from(keycode)))
            .unwrap_or(xkb::Keysym::from(xkb::keysyms::KEY_NoSymbol))
            .into()
    }

    fn handle_key_event(
        &mut self,
        evdev_keycode: u32,
        key_state: wl_keyboard::KeyState,
        qh: &QueueHandle<Self>,
    ) {
        if key_state == wl_keyboard::KeyState::Released {
            self.handle_key_release(evdev_keycode, qh);
            return;
        }

        let keycode = evdev_keycode + 8;
        tracing::trace!(
            "key press: keycode={keycode} xkb_state={}",
            if self.xkb_state.is_some() {
                "ok"
            } else {
                "NONE"
            }
        );
        let sym_raw = self.key_sym(evdev_keycode);
        tracing::trace!("key sym: {sym_raw:#x}");
        if sym_raw == xkb::keysyms::KEY_NoSymbol {
            tracing::warn!("key dropped: NoSymbol (xkb_state likely None)");
            return;
        }

        let effective_mods = if is_shift_key(sym_raw) {
            self.mods & !MOD_SHIFT
        } else {
            self.mods
        };

        let before_state = self.session.state();
        if should_bypass_empty_composition(sym_raw, effective_mods, &before_state) {
            self.forward_unhandled_key(evdev_keycode, sym_raw);
            return;
        }
        if is_enter_key(sym_raw) && !before_state.preedit.is_empty() {
            if let Some(im) = &self.input_method {
                im.commit_string(before_state.preedit.clone());
                im.set_preedit_string(String::new(), 0, 0);
                im.commit(self.serial);
            }
            self.session.reset();
            self.show_panel(ImeState::empty(), qh);
            return;
        }
        if is_candidate_select_key(sym_raw) && !before_state.candidates.is_empty() {
            let index = before_state
                .highlighted_candidate_index
                .min(before_state.candidates.len().saturating_sub(1));
            if let Some(ime_state) = self.session.select_candidate(index) {
                if let Some(im) = &self.input_method {
                    if let Some(committed) = &ime_state.committed {
                        im.commit_string(committed.clone());
                    }
                    im.set_preedit_string(String::new(), 0, 0);
                    im.commit(self.serial);
                }
                self.show_panel(ime_state, qh);
                return;
            }
        }

        let result = match self.session.process_key_result(sym_raw, effective_mods) {
            Some(r) => r,
            None => {
                // librime says it cannot process this key at all.
                self.forward_unhandled_key(evdev_keycode, sym_raw);
                return;
            }
        };

        let ime_state = result.state;

        let consumed = result.accepted;
        self.update_ascii_mode(ime_state.ascii_mode, qh);

        tracing::trace!(
            "ime state: consumed={} ascii_mode={} commit={:?} preedit={:?} candidates={}",
            consumed,
            ime_state.ascii_mode,
            ime_state.committed,
            ime_state.preedit,
            ime_state.candidates.len(),
        );

        if let Some(im) = &self.input_method {
            if consumed {
                if let Some(committed) = &ime_state.committed {
                    im.commit_string(committed.clone());
                }
                let preedit = ime_state.preedit.clone();
                let len = preedit.len() as i32;
                im.set_preedit_string(preedit, len, len);
                im.commit(self.serial);
            } else {
                // librime processed the key but produced nothing — forward it.
                self.forward_unhandled_key(evdev_keycode, sym_raw);
                return;
            }
        }

        self.show_panel(ime_state, qh);
    }

    fn handle_key_release(&mut self, evdev_keycode: u32, qh: &QueueHandle<Self>) {
        let sym_raw = self.key_sym(evdev_keycode);
        if is_shift_key(sym_raw) {
            if let Some(result) = self.session.process_key_result(sym_raw, RELEASE_MASK) {
                self.update_ascii_mode(result.state.ascii_mode, qh);
            }
        }
        if self.forwarded_keys.remove(&evdev_keycode) {
            self.forward_physical_key(evdev_keycode, WL_KEY_RELEASED);
        }
    }

    fn forward_unhandled_key(&mut self, evdev_keycode: u32, sym: u32) {
        if self.virtual_keyboard.is_some() && self.virtual_keymap.is_some() {
            self.forwarded_keys.insert(evdev_keycode);
            self.forward_physical_key(evdev_keycode, WL_KEY_PRESSED);
        } else {
            self.forward_text_key(sym);
        }
    }

    fn forward_physical_key(&self, evdev_keycode: u32, state: u32) {
        let Some(vk) = &self.virtual_keyboard else {
            return;
        };
        vk.key(self.last_key_time, evdev_keycode, state);
    }

    // Forward a key to the application when IME does not consume it.
    // zwp_input_method_v2 has no explicit forward_key request; we use
    // commit_string for printable chars and delete_surrounding_text for
    // delete/backspace.  Arrow/cursor keys cannot be forwarded this way —
    // the keyboard grab must release them; wlroots-based compositors do
    // this automatically for keys the IME commits nothing for.
    fn forward_text_key(&self, sym: u32) {
        let Some(im) = &self.input_method else { return };
        match sym {
            // Space
            0x0020 => {
                im.commit_string(" ".into());
                im.commit(self.serial);
            }
            // Return / KP_Enter
            0xff0d | 0xff8d => {
                im.commit_string("\n".into());
                im.commit(self.serial);
            }
            // BackSpace: delete one char to the left
            0xff08 => {
                im.delete_surrounding_text(1, 0);
                im.commit(self.serial);
            }
            // Delete: delete one char to the right
            0xffff => {
                im.delete_surrounding_text(0, 1);
                im.commit(self.serial);
            }
            // Tab
            0xff09 => {
                im.commit_string("\t".into());
                im.commit(self.serial);
            }
            // Arrow keys and other navigation — cannot be forwarded via
            // commit_string; the compositor passes them through the grab
            // automatically when the IME does not call commit().
            _ => {}
        }
    }
}

// ── Dispatch implementations ──────────────────────────────────────────────────

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
                "wl_compositor" => {
                    state.compositor = Some(registry.bind(name, version.min(4), qh, ()));
                }
                "wl_shm" => {
                    state.shm = Some(registry.bind(name, version.min(1), qh, ()));
                }
                "wl_seat" => {
                    state.seat = Some(registry.bind(name, version.min(7), qh, ()));
                }
                "zwp_input_method_manager_v2" => {
                    state.ime_manager = Some(registry.bind(name, 1, qh, ()));
                }
                "zwp_virtual_keyboard_manager_v1" => {
                    state.virtual_keyboard_manager = Some(registry.bind(name, 1, qh, ()));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<ZwpInputMethodV2, ()> for App {
    fn event(
        state: &mut Self,
        proxy: &ZwpInputMethodV2,
        event: zwp_input_method_v2::Event,
        _: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            zwp_input_method_v2::Event::Activate => {
                state.active = true;
                // Always replace any existing grab so we don't accumulate stale proxies.
                state.keyboard_grab = None;
                let grab = proxy.grab_keyboard(qh, ());
                tracing::debug!("IME activated; keyboard grab requested");
                state.keyboard_grab = Some(grab);
            }
            zwp_input_method_v2::Event::Deactivate => {
                state.active = false;
                let had_grab = state.keyboard_grab.take().is_some();
                state.session.reset();
                state.mode_hint_until = None;
                state.hide_panel_popup();
                tracing::debug!("IME deactivated (had_grab={had_grab})");
            }
            zwp_input_method_v2::Event::Done => {
                state.serial = state.serial.wrapping_add(1);
            }
            zwp_input_method_v2::Event::Unavailable => {
                // KDE Plasma: this fires when another IME (Fcitx5, IBus, Maliit) already
                // holds the input-method slot, OR if "Virtual Keyboard" in System Settings
                // → Input Devices is not set to "None".  Signal the event loop to exit
                // gracefully so the IBus/XIM threads keep running for KDE apps.
                tracing::warn!(
                    "zwp_input_method_v2: Unavailable — Wayland backend shutting down. \
                     On KDE Plasma: disable the virtual keyboard under System Settings \
                     → Input Devices → Virtual Keyboard and make sure no other IME \
                     (fcitx5, ibus) is running. The IBus/XIM backends will continue \
                     serving apps via QT_IM_MODULE/XMODIFIERS."
                );
                state.wayland_unavailable = true;
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwpInputMethodKeyboardGrabV2, ()> for App {
    fn event(
        state: &mut Self,
        _proxy: &ZwpInputMethodKeyboardGrabV2,
        event: zwp_input_method_keyboard_grab_v2::Event,
        _: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            zwp_input_method_keyboard_grab_v2::Event::Keymap { format, fd, size } => {
                tracing::debug!("keyboard grab: keymap received format={format:?} size={size}");
                if format != WEnum::Value(wl_keyboard::KeymapFormat::XkbV1) {
                    tracing::warn!("keyboard grab: unexpected keymap format {format:?}, skipping");
                    return;
                }
                let mmap = memmap2::MmapOptions::new()
                    .len(size as usize)
                    .map_raw_read_only(&fd);
                if let Ok(mmap) = mmap {
                    let keymap_bytes =
                        unsafe { std::slice::from_raw_parts(mmap.as_ptr(), size as usize) };
                    let keymap_text = keymap_bytes.strip_suffix(&[0]).unwrap_or(keymap_bytes);
                    let s = String::from_utf8_lossy(keymap_text).into_owned();
                    if let Some(km) = xkb::Keymap::new_from_string(
                        &state.xkb_context,
                        s.clone(),
                        xkb::KEYMAP_FORMAT_TEXT_V1,
                        xkb::KEYMAP_COMPILE_NO_FLAGS,
                    ) {
                        state.xkb_state = Some(xkb::State::new(&km));
                        state.xkb_keymap = Some(km);
                        state.install_virtual_keymap(keymap_bytes);
                    }
                }
            }
            zwp_input_method_keyboard_grab_v2::Event::Key { key, time, state: ks, .. } => {
                state.last_key_time = time;
                if let WEnum::Value(key_state) = ks {
                    state.handle_key_event(key, key_state, qh);
                }
            }
            zwp_input_method_keyboard_grab_v2::Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
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
                if let (Some(vk), Some(_)) = (&state.virtual_keyboard, &state.virtual_keymap) {
                    vk.modifiers(mods_depressed, mods_latched, mods_locked, group);
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwpInputPopupSurfaceV2, ()> for App {
    fn event(
        state: &mut Self,
        _proxy: &ZwpInputPopupSurfaceV2,
        event: zwp_input_popup_surface_v2::Event,
        _: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let zwp_input_popup_surface_v2::Event::TextInputRectangle {
            x,
            y,
            width,
            height,
        } = event
        {
            tracing::trace!("cursor hint: {x},{y} {width}x{height}");
            // KWin (and other Wayland compositors) often require a re-commit of the surface
            // after sending TextInputRectangle, otherwise the popup might not be mapped or updated.
            let has_content = state.ime_state.as_ref().map_or(false, |s| !s.candidates.is_empty());
            let show_mode_hint = state.mode_hint_active();
            if state.panel_surface.is_some() && (has_content || show_mode_hint) {
                tracing::debug!("rerender_panel_after_rectangle");
                state.redraw_panel(qh);
            }
        }
    }
}

delegate_noop!(App: ignore WlCompositor);
delegate_noop!(App: ignore WlShm);
delegate_noop!(App: ignore WlShmPool);
delegate_noop!(App: ignore WlBuffer);
delegate_noop!(App: ignore WlSurface);
delegate_noop!(App: ignore WlSeat);
delegate_noop!(App: ignore WlKeyboard);
delegate_noop!(App: ignore ZwpInputMethodManagerV2);
delegate_noop!(App: ignore ZwpVirtualKeyboardManagerV1);
delegate_noop!(App: ignore ZwpVirtualKeyboardV1);

// ── Entry point ────────────────────────────────────────────────────────────────

/// Returns Ok(()) when the Wayland input-method slot is unavailable (e.g. KDE
/// virtual keyboard is active) so the caller can keep IBus/XIM threads alive.
pub fn run(engine: CoreEngine) -> Result<(), String> {
    let session = engine
        .create_session()
        .map_err(|e| format!("failed to create Wayland Rime session: {e}"))?;

    let conn = Connection::connect_to_env().map_err(|e| format!("Wayland connection: {e}"))?;
    let (globals, mut queue) =
        registry_queue_init::<App>(&conn).map_err(|e| format!("registry: {e}"))?;
    let qh = queue.handle();

    let compositor: WlCompositor = globals
        .bind(&qh, 1..=4, ())
        .map_err(|e| format!("wl_compositor not advertised: {e}"))?;
    let shm: WlShm = globals
        .bind(&qh, 1..=1, ())
        .map_err(|e| format!("wl_shm not advertised: {e}"))?;
    let seat: WlSeat = globals
        .bind(&qh, 1..=7, ())
        .map_err(|e| format!("wl_seat not advertised: {e}"))?;
    let ime_manager: ZwpInputMethodManagerV2 = globals.bind(&qh, 1..=1, ()).map_err(|_| {
        // KDE Plasma < 5.24 does not implement this protocol; Plasma 5.24+ does.
        "compositor does not advertise zwp_input_method_manager_v2 \
         (KDE Plasma < 5.24, GNOME Shell without the mutter fork, or a compositor \
         that does not implement the Wayland input-method-v2 protocol)"
            .to_string()
    })?;
    let virtual_keyboard_manager: Option<ZwpVirtualKeyboardManagerV1> = globals
        .bind(&qh, 1..=1, ())
        .map_err(|e| {
            tracing::warn!(
                "compositor does not advertise zwp_virtual_keyboard_manager_v1; \
                 shortcut forwarding will be limited: {e}"
            );
            e
        })
        .ok();

    let mut app = App::new(session);
    app.compositor = Some(compositor);
    app.shm = Some(shm);
    app.seat = Some(seat);
    app.ime_manager = Some(ime_manager);
    app.virtual_keyboard_manager = virtual_keyboard_manager;

    app.setup_ime(&qh);
    app.setup_virtual_keyboard(&qh);
    queue.roundtrip(&mut app).expect("initial roundtrip");

    tracing::info!("Wayland IME running (popup-surface positioning)");
    loop {
        if app.wayland_unavailable {
            tracing::info!("Wayland IME exiting gracefully (slot unavailable)");
            return Ok(());
        }

        if app
            .mode_hint_until
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            app.mode_hint_until = None;
            app.show_panel(ImeState::empty(), &qh);
        }

        if let Err(e) = queue.flush() {
            tracing::warn!("Wayland flush error: {e}");
        }
        let timeout_ms = if app.mode_hint_until.is_some() {
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
                tracing::warn!("Wayland dispatch error: {e}");
                return Err(format!("Wayland connection closed: {e}"));
            }
        } else if ready < 0 {
            tracing::warn!("Wayland poll failed");
        }
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────────

fn tempfile() -> std::io::Result<File> {
    use std::os::unix::io::FromRawFd;
    let name = c"keytao-shm";
    let fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}
