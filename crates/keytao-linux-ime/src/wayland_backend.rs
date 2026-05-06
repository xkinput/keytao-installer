//! Wayland backend: zwp_input_method_v2 + zwp_input_popup_surface_v2.
//!
//! Protocol stack:
//!   zwp_input_method_manager_v2      — get the input method handle
//!   zwp_input_method_v2              — activate/deactivate + commit/preedit
//!   zwp_input_method_keyboard_grab_v2 — exclusive keyboard grab
//!   zwp_input_popup_surface_v2       — auto-positioned candidate panel at cursor
//!   wl_shm                           — shared-memory pixel buffers

use std::{fs::File, io::Write, os::fd::AsFd};

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
use xkbcommon::xkb;

use crate::{
    engine::CoreEngine,
    panel::{load_font, PanelRenderer},
};

const MOD_SHIFT: u32 = 0x0001;
const MOD_CONTROL: u32 = 0x0004;
const MOD_MOD1: u32 = 0x0008;

struct App {
    engine: CoreEngine,
    renderer: Option<PanelRenderer>,

    compositor: Option<WlCompositor>,
    shm: Option<WlShm>,
    seat: Option<WlSeat>,
    ime_manager: Option<ZwpInputMethodManagerV2>,

    input_method: Option<ZwpInputMethodV2>,
    keyboard_grab: Option<ZwpInputMethodKeyboardGrabV2>,
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
}

impl App {
    fn new(engine: CoreEngine) -> Self {
        let renderer = load_font().map(PanelRenderer::new);
        if renderer.is_none() {
            tracing::warn!("panel renderer unavailable: no font found");
        }
        Self {
            engine,
            renderer,
            compositor: None,
            shm: None,
            seat: None,
            ime_manager: None,
            input_method: None,
            keyboard_grab: None,
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
        }
    }

    fn setup_ime(&mut self, qh: &QueueHandle<Self>) {
        let (Some(manager), Some(seat)) = (&self.ime_manager, &self.seat) else {
            return;
        };
        let im = manager.get_input_method(seat, qh, ());
        self.input_method = Some(im);
        tracing::info!("input method registered");
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

    fn destroy_panel_popup(&mut self) {
        if let Some(popup) = self.panel_popup.take() {
            popup.destroy();
        }
        if let Some(surface) = self.panel_surface.take() {
            surface.destroy();
        }
        if let Some(buf) = self.panel_buffer.take() {
            buf.destroy();
        }
        self.panel_visible = false;
        self.ime_state = None;
    }

    fn redraw_panel(&mut self, qh: &QueueHandle<Self>) {
        let (Some(state), Some(renderer), Some(shm), Some(surface)) = (
            self.ime_state.as_ref(),
            self.renderer.as_ref(),
            self.shm.as_ref(),
            self.panel_surface.as_ref(),
        ) else {
            return;
        };

        let (pixels, w, h) = renderer.render(state);
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
        let has_content = !state.candidates.is_empty() || !state.preedit.is_empty();
        self.ime_state = Some(state);
        if has_content {
            if self.panel_surface.is_none() {
                self.create_panel_popup(qh);
            }
            self.panel_visible = true;
            self.redraw_panel(qh);
        } else {
            self.destroy_panel_popup();
        }
    }

    fn handle_key_press(&mut self, keycode: u32, qh: &QueueHandle<Self>) {
        let keysym = self
            .xkb_state
            .as_ref()
            .map(|s| s.key_get_one_sym(xkb::Keycode::from(keycode)))
            .unwrap_or(xkb::Keysym::from(xkb::keysyms::KEY_NoSymbol));

        let sym_raw: u32 = keysym.into();
        if sym_raw == xkb::keysyms::KEY_NoSymbol {
            return;
        }

        let ime_state = match self.engine.process_key(sym_raw, self.mods) {
            Some(s) => s,
            None => {
                // librime says it cannot process this key at all.
                // Forward functional keys to the app as text or via protocol.
                self.forward_key(sym_raw);
                return;
            }
        };

        let consumed = ime_state.committed.is_some()
            || !ime_state.preedit.is_empty()
            || !ime_state.candidates.is_empty();

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
                self.forward_key(sym_raw);
                return;
            }
        }

        self.show_panel(ime_state, qh);
    }

    // Forward a key to the application when IME does not consume it.
    // zwp_input_method_v2 has no explicit forward_key request; we use
    // commit_string for printable chars and delete_surrounding_text for
    // delete/backspace.  Arrow/cursor keys cannot be forwarded this way —
    // the keyboard grab must release them; wlroots-based compositors do
    // this automatically for keys the IME commits nothing for.
    fn forward_key(&self, sym: u32) {
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
        if let wl_registry::Event::Global { name, interface, version } = event {
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
                let grab = proxy.grab_keyboard(qh, ());
                state.keyboard_grab = Some(grab);
                tracing::debug!("IME activated");
            }
            zwp_input_method_v2::Event::Deactivate => {
                state.active = false;
                state.keyboard_grab.take();
                state.engine.reset();
                state.destroy_panel_popup();
                tracing::debug!("IME deactivated");
            }
            zwp_input_method_v2::Event::Done => {
                state.serial = state.serial.wrapping_add(1);
            }
            zwp_input_method_v2::Event::Unavailable => {
                tracing::error!(
                    "input method unavailable — another IME is already running, \
                     or compositor does not support zwp_input_method_v2"
                );
                std::process::exit(1);
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
                if format != WEnum::Value(wl_keyboard::KeymapFormat::XkbV1) {
                    return;
                }
                let mmap = unsafe {
                    memmap2::MmapOptions::new()
                        .len(size as usize)
                        .map_raw_read_only(&fd)
                };
                if let Ok(mmap) = mmap {
                    let s = unsafe {
                        let bytes =
                            std::slice::from_raw_parts(mmap.as_ptr(), size as usize - 1);
                        std::str::from_utf8_unchecked(bytes).to_string()
                    };
                    if let Some(km) = xkb::Keymap::new_from_string(
                        &state.xkb_context,
                        s,
                        xkb::KEYMAP_FORMAT_TEXT_V1,
                        xkb::KEYMAP_COMPILE_NO_FLAGS,
                    ) {
                        state.xkb_state = Some(xkb::State::new(&km));
                        state.xkb_keymap = Some(km);
                    }
                }
            }
            zwp_input_method_keyboard_grab_v2::Event::Key { key, state: ks, .. } => {
                if ks == WEnum::Value(wl_keyboard::KeyState::Pressed) {
                    state.handle_key_press(key + 8, qh);
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
                    xkb_state.update_mask(
                        mods_depressed,
                        mods_latched,
                        mods_locked,
                        0,
                        0,
                        group,
                    );
                    let mut m = 0u32;
                    if xkb_state.mod_name_is_active(
                        xkb::MOD_NAME_SHIFT,
                        xkb::STATE_MODS_EFFECTIVE,
                    ) {
                        m |= MOD_SHIFT;
                    }
                    if xkb_state.mod_name_is_active(
                        xkb::MOD_NAME_CTRL,
                        xkb::STATE_MODS_EFFECTIVE,
                    ) {
                        m |= MOD_CONTROL;
                    }
                    if xkb_state.mod_name_is_active(
                        xkb::MOD_NAME_ALT,
                        xkb::STATE_MODS_EFFECTIVE,
                    ) {
                        m |= MOD_MOD1;
                    }
                    state.mods = m;
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwpInputPopupSurfaceV2, ()> for App {
    fn event(
        _state: &mut Self,
        _proxy: &ZwpInputPopupSurfaceV2,
        event: zwp_input_popup_surface_v2::Event,
        _: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let zwp_input_popup_surface_v2::Event::TextInputRectangle { x, y, width, height } =
            event
        {
            tracing::trace!("cursor hint: {x},{y} {width}x{height}");
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

// ── Entry point ────────────────────────────────────────────────────────────────

pub fn run(engine: CoreEngine) {
    let conn = Connection::connect_to_env().expect("Wayland connection");
    let (globals, mut queue) = registry_queue_init::<App>(&conn).expect("registry");
    let qh = queue.handle();

    let compositor: WlCompositor = globals
        .bind(&qh, 1..=4, ())
        .expect("wl_compositor not advertised");
    let shm: WlShm = globals.bind(&qh, 1..=1, ()).expect("wl_shm not advertised");
    let seat: WlSeat = globals
        .bind(&qh, 1..=7, ())
        .expect("wl_seat not advertised");
    let ime_manager: ZwpInputMethodManagerV2 =
        globals.bind(&qh, 1..=1, ()).unwrap_or_else(|_| {
            tracing::error!(
                "compositor does not advertise zwp_input_method_manager_v2; \
                 try a wlroots compositor (sway, niri, river, etc.)"
            );
            std::process::exit(1);
        });

    let mut app = App::new(engine);
    app.compositor = Some(compositor);
    app.shm = Some(shm);
    app.seat = Some(seat);
    app.ime_manager = Some(ime_manager);

    app.setup_ime(&qh);
    queue.roundtrip(&mut app).expect("initial roundtrip");

    tracing::info!("Wayland IME running (popup-surface positioning)");
    loop {
        queue.blocking_dispatch(&mut app).expect("dispatch");
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
