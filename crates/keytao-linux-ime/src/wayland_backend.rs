//! Wayland backend: zwp_input_method_v2 + zwlr_layer_shell_v1 candidate panel.
//!
//! Protocol stack:
//!   zwp_input_method_manager_v2  — get the input method handle
//!   zwp_input_method_v2          — activate/deactivate + commit text/preedit
//!   zwp_input_method_keyboard_grab_v2 — exclusive keyboard grab
//!   zwlr_layer_shell_v1          — floating overlay surface for candidate panel
//!   wl_shm                       — shared-memory pixel buffers
//!
//! Compositor requirements: sway, river, KDE Plasma ≥ 5.24, or any wlroots
//! compositor that supports both protocols.  GNOME is NOT supported because
//! Mutter does not implement zwp_input_method_v2.

use std::{
    fs::File,
    io::Write,
    os::fd::AsFd,
    sync::{Arc, Mutex},
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
        wl_seat::{self, WlSeat},
        wl_shm::{self, WlShm},
        wl_shm_pool::WlShmPool,
        wl_surface::WlSurface,
    },
    Connection, Dispatch, QueueHandle, WEnum,
};
use wayland_protocols::unstable::input_method::v2::client::{
    zwp_input_method_keyboard_grab_v2::{self, ZwpInputMethodKeyboardGrabV2},
    zwp_input_method_manager_v2::ZwpInputMethodManagerV2,
    zwp_input_method_v2::{self, ZwpInputMethodV2},
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{self, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, ZwlrLayerSurfaceV1},
};
use xkbcommon::xkb;

use crate::{
    engine::CoreEngine,
    panel::{load_font, PanelRenderer},
};

// ── X11 modifier masks (what keytao-core expects) ────────────────────────────

const MOD_SHIFT: u32 = 0x0001;
const MOD_CONTROL: u32 = 0x0004;
const MOD_MOD1: u32 = 0x0008; // Alt

// ── App state ─────────────────────────────────────────────────────────────────

struct App {
    engine: CoreEngine,
    renderer: Option<PanelRenderer>,

    // Globals
    compositor: Option<WlCompositor>,
    shm: Option<WlShm>,
    seat: Option<WlSeat>,
    ime_manager: Option<ZwpInputMethodManagerV2>,
    layer_shell: Option<ZwlrLayerShellV1>,

    // Active IME objects
    input_method: Option<ZwpInputMethodV2>,
    keyboard_grab: Option<ZwpInputMethodKeyboardGrabV2>,
    serial: u32,
    active: bool,

    // XKB
    xkb_context: xkb::Context,
    xkb_keymap: Option<xkb::Keymap>,
    xkb_state: Option<xkb::State>,
    mods: u32,

    // Candidate panel surface
    panel_surface: Option<WlSurface>,
    panel_layer_surface: Option<ZwlrLayerSurfaceV1>,
    panel_configured: bool,
    panel_buffer: Option<WlBuffer>,
    panel_width: u32,
    panel_height: u32,
    panel_visible: bool,

    // Pending IME state to display
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
            layer_shell: None,
            input_method: None,
            keyboard_grab: None,
            serial: 0,
            active: false,
            xkb_context: xkb::Context::new(xkb::CONTEXT_NO_FLAGS),
            xkb_keymap: None,
            xkb_state: None,
            mods: 0,
            panel_surface: None,
            panel_layer_surface: None,
            panel_configured: false,
            panel_buffer: None,
            panel_width: 0,
            panel_height: 0,
            panel_visible: false,
            ime_state: None,
        }
    }

    /// Called once all globals are bound; create IME + panel surface.
    fn setup_ime(&mut self, qh: &QueueHandle<Self>) {
        let Some(manager) = &self.ime_manager else {
            return;
        };
        let Some(seat) = &self.seat else { return };

        let im = manager.get_input_method(seat, qh, ());
        self.input_method = Some(im);
        tracing::info!("input method registered");

        self.create_panel_surface(qh);
    }

    fn create_panel_surface(&mut self, qh: &QueueHandle<Self>) {
        let Some(compositor) = &self.compositor else {
            return;
        };
        let Some(layer_shell) = &self.layer_shell else {
            return;
        };

        let surface = compositor.create_surface(qh, ());
        let layer_surface = layer_shell.get_layer_surface(
            &surface,
            None, // output: None = active output
            zwlr_layer_shell_v1::Layer::Overlay,
            "keytao-candidate".to_string(),
            qh,
            (),
        );

        // Anchor to bottom-left, no exclusive zone, keyboard-passthrough
        layer_surface.set_anchor(
            zwlr_layer_surface_v1::Anchor::Bottom | zwlr_layer_surface_v1::Anchor::Left,
        );
        layer_surface.set_exclusive_zone(-1);
        layer_surface.set_size(300, 46);
        layer_surface
            .set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::None);

        surface.commit();

        self.panel_surface = Some(surface);
        self.panel_layer_surface = Some(layer_surface);
    }

    /// Re-render the candidate panel and present it.
    fn redraw_panel(&mut self, qh: &QueueHandle<Self>) {
        let (Some(state), Some(renderer), Some(shm), Some(surface)) = (
            self.ime_state.as_ref(),
            self.renderer.as_ref(),
            self.shm.as_ref(),
            self.panel_surface.as_ref(),
        ) else {
            return;
        };
        if !self.panel_configured {
            return;
        }

        let (pixels, w, h) = renderer.render(state);
        let stride = w * 4;
        let pool_size = (stride * h) as usize;

        // Write pixels to a temp file then wrap in wl_shm
        let mut tmp = tempfile().expect("tempfile");
        tmp.write_all(&pixels).expect("write shm");
        let pool = shm.create_pool(tmp.as_fd(), pool_size as i32, qh, ());
        let buffer = pool.create_buffer(
            0,
            w as i32,
            h as i32,
            stride as i32,
            wl_shm::Format::Bgr888, // BGRA = Argb8888 in wl_shm
            qh,
            (),
        );

        if let Some(ls) = &self.panel_layer_surface {
            ls.set_size(w, h);
        }

        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, w as i32, h as i32);
        surface.commit();

        // Keep buffer alive until next frame
        if let Some(old) = self.panel_buffer.replace(buffer) {
            old.destroy();
        }
        pool.destroy();

        self.panel_width = w;
        self.panel_height = h;
    }

    fn show_panel(&mut self, state: ImeState, qh: &QueueHandle<Self>) {
        let has_content = !state.candidates.is_empty() || !state.preedit.is_empty();
        self.ime_state = Some(state);
        if has_content {
            self.panel_visible = true;
            self.redraw_panel(qh);
        } else {
            self.hide_panel();
        }
    }

    fn hide_panel(&mut self) {
        if let (Some(surface), Some(ls)) = (&self.panel_surface, &self.panel_layer_surface) {
            ls.set_size(0, 0);
            surface.commit();
        }
        self.panel_visible = false;
        self.ime_state = None;
    }

    /// Process a key press: run through librime, then commit/preedit via protocol.
    fn handle_key_press(&mut self, keycode: u32, qh: &QueueHandle<Self>) {
        let keysym = self
            .xkb_state
            .as_ref()
            .map(|s| s.key_get_one_sym(keycode))
            .unwrap_or(xkb::KEY_NoSymbol);

        if keysym == xkb::KEY_NoSymbol {
            return;
        }

        let ime_state = match self.engine.process_key(keysym, self.mods) {
            Some(s) => s,
            None => return,
        };

        if let Some(im) = &self.input_method {
            if let Some(committed) = &ime_state.committed {
                im.commit_string(committed.clone());
            }
            // Preedit: cursor at end
            let preedit = ime_state.preedit.clone();
            let len = preedit.len() as i32;
            im.set_preedit_string(preedit, len, len);
            im.commit(self.serial);
        }

        self.show_panel(ime_state, qh);
    }
}

// ── Dispatch implementations ──────────────────────────────────────────────────

// Registry
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
                    let seat: WlSeat = registry.bind(name, version.min(7), qh, ());
                    state.seat = Some(seat);
                }
                "zwp_input_method_manager_v2" => {
                    state.ime_manager = Some(registry.bind(name, 1, qh, ()));
                }
                "zwlr_layer_shell_v1" => {
                    state.layer_shell = Some(registry.bind(name, version.min(4), qh, ()));
                }
                _ => {}
            }
        }
    }
}

// Input method
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
                if let Some(seat) = &state.seat {
                    let grab = proxy.grab_keyboard(seat, qh, ());
                    state.keyboard_grab = Some(grab);
                }
                tracing::debug!("IME activated");
            }
            zwp_input_method_v2::Event::Deactivate => {
                state.active = false;
                state.keyboard_grab.take();
                state.engine.reset();
                state.hide_panel();
                tracing::debug!("IME deactivated");
            }
            zwp_input_method_v2::Event::Done => {
                state.serial = state.serial.wrapping_add(1);
            }
            zwp_input_method_v2::Event::Unavailable => {
                tracing::error!(
                    "input method unavailable — is another IME running, or does \
                     the compositor not support zwp_input_method_v2?"
                );
                std::process::exit(1);
            }
            _ => {}
        }
    }
}

// Keyboard grab
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
                    let s = unsafe { std::str::from_utf8_unchecked(&mmap[..size as usize - 1]) };
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
            zwp_input_method_keyboard_grab_v2::Event::Key {
                key,
                state: key_state,
                ..
            } => {
                if key_state == WEnum::Value(wl_keyboard::KeyState::Pressed) {
                    // Wayland keycode is evdev; XKB keycode = evdev + 8
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
            }
            _ => {}
        }
    }
}

// Layer surface
impl Dispatch<ZwlrLayerSurfaceV1, ()> for App {
    fn event(
        state: &mut Self,
        ls: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                ls.ack_configure(serial);
                state.panel_configured = true;
                if width > 0 {
                    state.panel_width = width;
                }
                if height > 0 {
                    state.panel_height = height;
                }
                if state.panel_visible {
                    state.redraw_panel(qh);
                }
            }
            zwlr_layer_surface_v1::Event::Closed => {
                state.panel_configured = false;
                state.panel_surface = None;
                state.panel_layer_surface = None;
            }
            _ => {}
        }
    }
}

// No-op dispatches for globals we only keep alive
delegate_noop!(App: ignore WlCompositor);
delegate_noop!(App: ignore WlShm);
delegate_noop!(App: ignore WlShm);
delegate_noop!(App: ignore WlShmPool);
delegate_noop!(App: ignore WlBuffer);
delegate_noop!(App: ignore WlSurface);
delegate_noop!(App: ignore WlSeat);
delegate_noop!(App: ignore WlKeyboard);
delegate_noop!(App: ignore ZwpInputMethodManagerV2);
delegate_noop!(App: ignore ZwlrLayerShellV1);

// ── Entry point ───────────────────────────────────────────────────────────────

pub fn run(engine: CoreEngine) {
    let conn = Connection::connect_to_env().expect("Wayland connection");
    let (globals, mut queue) = registry_queue_init::<App>(&conn).expect("registry");
    let qh = queue.handle();

    let mut app = App::new(engine);

    // Bind globals
    globals.contents().with_list(|list| {
        for global in list {
            let qh2 = queue.handle();
            let _ = app.dispatch_event(
                &wl_registry::WlRegistry::from_id(&conn, global.name),
                wl_registry::Event::Global {
                    name: global.name,
                    interface: global.interface.clone(),
                    version: global.version,
                },
                &mut app,
                &conn,
                &qh2,
            );
        }
    });

    // Roundtrip to receive initial events
    queue.roundtrip(&mut app).expect("roundtrip");

    if app.ime_manager.is_none() {
        tracing::error!(
            "compositor does not advertise zwp_input_method_manager_v2; \
             try a wlroots compositor (sway, river, etc.)"
        );
        std::process::exit(1);
    }

    app.setup_ime(&qh);
    queue.roundtrip(&mut app).expect("roundtrip");

    tracing::info!("Wayland IME running");
    loop {
        queue.blocking_dispatch(&mut app).expect("dispatch");
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Create an anonymous temporary file for wl_shm pool.
fn tempfile() -> std::io::Result<File> {
    // Use memfd_create on Linux
    use std::os::unix::io::FromRawFd;
    let name = c"keytao-shm";
    let fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        // fallback: regular temp file
        return tempfile::tempfile();
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

// libc shim — only for memfd_create
mod libc {
    extern "C" {
        pub fn memfd_create(name: *const libc::c_char, flags: libc::c_uint) -> libc::c_int;
    }
    pub use ::libc::*;
}
