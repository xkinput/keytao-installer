//! KDE/KWin Wayland backend: zwp_input_method_unstable_v1.
//!
//! KWin 6 still exposes input methods through input-method-v1. Applications talk
//! text-input-v1/v2/v3 to KWin; the IME process talks input-method-v1 to KWin.

use std::{
    fs::File,
    os::fd::{AsFd, AsRawFd},
    time::{Duration, Instant},
    io::Write,
};

use keytao_core::ImeState;
use wayland_client::{
    delegate_noop,
    globals::{registry_queue_init, GlobalListContents},
    protocol::{
        wl_buffer::WlBuffer,
        wl_compositor::WlCompositor,
        wl_keyboard::{self, WlKeyboard},
        wl_region::WlRegion,
        wl_registry,
        wl_seat::WlSeat,
        wl_shm::{self, WlShm},
        wl_shm_pool::WlShmPool,
        wl_surface::WlSurface,
    },
    Connection, Dispatch, QueueHandle, WEnum,
};
use wayland_protocols::wp::input_method::zv1::client::{
    zwp_input_method_context_v1::{self, ZwpInputMethodContextV1},
    zwp_input_method_v1::{self, ZwpInputMethodV1},
    zwp_input_panel_surface_v1::ZwpInputPanelSurfaceV1,
    zwp_input_panel_v1::ZwpInputPanelV1,
};
use xkbcommon::xkb;

use crate::{
    engine::{CoreEngine, ImeSession},
    kimpanel::KimpanelHandle,
    panel::{load_font, PanelRenderer},
};

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
    kimpanel: Option<KimpanelHandle>,
    pending_kimpanel_state: Option<ImeState>,
    clear_kimpanel: bool,
    globals_seen: Vec<String>,

    renderer: Option<PanelRenderer>,
    compositor: Option<WlCompositor>,
    shm: Option<WlShm>,
    input_panel: Option<ZwpInputPanelV1>,
    panel_surface: Option<WlSurface>,
    panel_popup: Option<ZwpInputPanelSurfaceV1>,
    panel_buffer: Option<WlBuffer>,
    panel_visible: bool,
    ime_state: Option<ImeState>,
}

impl App {
    fn new(session: ImeSession, kimpanel: Option<KimpanelHandle>) -> Self {
        let renderer = load_font().and_then(PanelRenderer::new);
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
            kimpanel,
            pending_kimpanel_state: None,
            clear_kimpanel: true,
            globals_seen: Vec::new(),

            renderer,
            compositor: None,
            shm: None,
            input_panel: None,
            panel_surface: None,
            panel_popup: None,
            panel_buffer: None,
            panel_visible: false,
            ime_state: None,
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
        self.pending_kimpanel_state = None;
        self.clear_kimpanel = true;
        self.hide_panel_popup();
    }

    fn create_panel_popup(&mut self, qh: &QueueHandle<Self>) {
        let (Some(compositor), Some(panel_manager), Some(shm)) =
            (&self.compositor, &self.input_panel, &self.shm)
        else {
            return;
        };
        let surface = compositor.create_surface(qh, ());
        let popup = panel_manager.get_input_panel_surface(&surface, qh, ());
        popup.set_overlay_panel();

        // Set input region to empty so clicks pass through the candidate window.
        let region = compositor.create_region(qh, ());
        surface.set_input_region(Some(&region));
        region.destroy();

        // 1x1 transparent dummy buffer to make surface valid for KWin
        let mut fd = match tempfile() {
            Ok(file) => file,
            Err(e) => {
                tracing::warn!("failed to create dummy SHM file: {e}");
                return;
            }
        };
        if fd.set_len(4).is_err() {
            tracing::warn!("failed to truncate dummy SHM file");
            return;
        }
        if fd.write_all(&[0u8; 4]).is_err() {
            tracing::warn!("failed to write dummy SHM buffer");
            return;
        }
        let pool = shm.create_pool(fd.as_fd(), 4, qh, ());
        let buf = pool.create_buffer(0, 1, 1, 4, wl_shm::Format::Argb8888, qh, ());
        surface.attach(Some(&buf), 0, 0);
        surface.damage_buffer(0, 0, 1, 1);
        surface.commit();

        self.panel_buffer = Some(buf);
        self.panel_surface = Some(surface);
        self.panel_popup = Some(popup);
        pool.destroy();
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

        let (pixels, w, h) = if let Some(state) = self
            .ime_state
            .as_ref()
            .filter(|state| !state.candidates.is_empty())
        {
            renderer.render(state)
        } else {
            return;
        };
        if w == 0 || h == 0 {
            return;
        }
        let stride = w * 4;
        let pool_size = (stride * h) as usize;

        let mut tmp = match tempfile() {
            Ok(file) => file,
            Err(e) => {
                tracing::warn!("failed to create SHM tempfile: {e}");
                return;
            }
        };
        if tmp.set_len(pool_size as u64).is_err() {
            tracing::warn!("failed to truncate SHM tempfile");
            return;
        }
        if tmp.write_all(&pixels).is_err() {
            tracing::warn!("failed to write SHM buffer");
            return;
        }
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
        let has_content = !state.candidates.is_empty();
        self.ime_state = Some(state);
        if has_content {
            if self.panel_surface.is_none() {
                self.create_panel_popup(qh);
            }
            self.panel_visible = true;
            self.redraw_panel(qh);
        } else {
            self.hide_panel_popup();
        }
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

    fn update_kimpanel(&mut self, state: &ImeState) {
        self.pending_kimpanel_state = Some(state.clone());
        self.clear_kimpanel = false;
    }

    fn clear_kimpanel(&mut self) {
        self.pending_kimpanel_state = None;
        self.clear_kimpanel = true;
    }

    fn forward_key(&self, evdev_keycode: u32, state: u32) {
        tracing::info!("KDE forwarding key: key={evdev_keycode}, state={state}");
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
        tracing::info!("KDE handling key event: key={evdev_keycode}, state={key_state:?}, active={}", self.active);
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
            self.clear_kimpanel();
            self.hide_panel_popup();
            self.session.reset();
            return;
        }

        if is_candidate_select_key(sym_raw) && !before_state.candidates.is_empty() {
            let index = before_state
                .highlighted_candidate_index
                .min(before_state.candidates.len().saturating_sub(1));
            if let Some(ime_state) = self.session.select_candidate(index) {
                self.commit_state_to_context(&ime_state);
                self.update_kimpanel(&ime_state);
                self.show_panel(ime_state, qh);
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
            self.update_kimpanel(&ime_state);
            self.show_panel(ime_state, qh);
            if should_forward_consumed_shortcut(sym_raw, effective_mods) {
                self.forward_key(evdev_keycode, wl_keyboard::KeyState::Pressed as u32);
            }
        } else {
            self.clear_context_preedit();
            self.clear_kimpanel();
            self.hide_panel_popup();
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
            state.globals_seen.push(format!("{interface}@{version}#{name}"));
            match interface.as_str() {
                "wl_seat" => state.seat = Some(registry.bind(name, version.min(7), qh, ())),
                "wl_compositor" => {
                    state.compositor = Some(registry.bind(name, version.min(6), qh, ()));
                }
                "wl_shm" => state.shm = Some(registry.bind(name, version.min(2), qh, ())),
                "zwp_input_method_v1" => {
                    state.input_method = Some(registry.bind(name, 1, qh, ()));
                }
                "zwp_input_panel_v1" => {
                    state.input_panel = Some(registry.bind(name, 1, qh, ()));
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
                tracing::info!("KDE input-method-v1 context activated!");
                state.reset_context_state();
                state.keyboard = Some(id.grab_keyboard(qh, ()));
                state.context = Some(id);
                state.active = true;
            }
            zwp_input_method_v1::Event::Deactivate { context: _ } => {
                tracing::info!("KDE input-method-v1 context deactivated!");
                state.deactivate_deadline = Some(Instant::now() + DEACTIVATE_DEBOUNCE);
            }
            _ => {}
        }
    }

    wayland_client::event_created_child!(App, ZwpInputMethodV1, [
        0 => (ZwpInputMethodContextV1, ()),
    ]);
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
                state.clear_kimpanel();
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
                serial: _,
                time,
                key,
                state: ks,
            } => {
                tracing::info!("KDE keyboard Event::Key: key={key}, state={ks:?}");
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
                tracing::info!("KDE keyboard Event::Modifiers");
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
delegate_noop!(App: ignore WlCompositor);
delegate_noop!(App: ignore WlShm);
delegate_noop!(App: ignore WlShmPool);
delegate_noop!(App: ignore WlBuffer);
delegate_noop!(App: ignore WlSurface);
delegate_noop!(App: ignore ZwpInputPanelV1);
delegate_noop!(App: ignore ZwpInputPanelSurfaceV1);
delegate_noop!(App: ignore WlRegion);

pub fn run(engine: CoreEngine) -> Result<(), String> {
    let session = engine
        .create_session()
        .map_err(|e| format!("failed to create KDE Wayland Rime session: {e}"))?;
    let conn = Connection::connect_to_env().map_err(|e| format!("KDE Wayland connection: {e}"))?;
    let (globals, mut queue) =
        registry_queue_init::<App>(&conn).map_err(|e| format!("KDE Wayland registry: {e}"))?;
    let qh = queue.handle();

    let compositor: WlCompositor = globals
        .bind(&qh, 1..=6, ())
        .map_err(|e| format!("wl_compositor not advertised: {e}"))?;
    let shm: WlShm = globals
        .bind(&qh, 1..=2, ())
        .map_err(|e| format!("wl_shm not advertised: {e}"))?;
    let seat: WlSeat = globals
        .bind(&qh, 1..=7, ())
        .map_err(|e| format!("wl_seat not advertised: {e}"))?;
    let input_method: ZwpInputMethodV1 = globals
        .bind(&qh, 1..=1, ())
        .map_err(|e| format!("zwp_input_method_v1 not advertised: {e}"))?;
    let input_panel: ZwpInputPanelV1 = globals
        .bind(&qh, 1..=1, ())
        .map_err(|e| format!("zwp_input_panel_v1 not advertised: {e}"))?;

    let kimpanel_runtime =
        tokio::runtime::Runtime::new().map_err(|e| format!("Kimpanel runtime: {e}"))?;
    let kimpanel = kimpanel_runtime.block_on(KimpanelHandle::new());

    let mut app = App::new(session, kimpanel);
    app.compositor = Some(compositor);
    app.shm = Some(shm);
    app.seat = Some(seat);
    app.input_method = Some(input_method);
    app.input_panel = Some(input_panel);
    queue.roundtrip(&mut app).expect("initial KDE roundtrip");
    for g in globals.contents().clone_list() {
        tracing::info!("KDE Wayland global: {} v{}", g.interface, g.version);
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
        if let Some(kimpanel) = &app.kimpanel {
            if app.clear_kimpanel {
                kimpanel_runtime.block_on(kimpanel.clear());
                app.clear_kimpanel = false;
            } else if let Some(state) = app.pending_kimpanel_state.take() {
                kimpanel_runtime.block_on(kimpanel.update_state(&state));
            }
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

fn tempfile() -> std::io::Result<File> {
    use std::os::unix::io::FromRawFd;
    let name = c"keytao-shm";
    let fd = unsafe {
        libc::memfd_create(
            name.as_ptr(),
            libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}
