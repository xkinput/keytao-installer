//! X11 backend: XIM server (@server=keytao) + XCB candidate overlay window.
//!
//! Set XMODIFIERS=@im=keytao in your session before launching apps.
//! The server registers under the name "keytao"; XIM clients that respect
//! XMODIFIERS will connect automatically.

use std::sync::Arc;

use keytao_core::ImeState;
use x11rb::{
    connection::Connection as _,
    protocol::{
        xproto::{
            AtomEnum, ConfigureWindowAux, ConnectionExt as _, CreateWindowAux, EventMask, Gcontext,
            ImageFormat, PropMode, Window, WindowClass,
        },
        Event,
    },
    wrapper::ConnectionExt as _,
    xcb_ffi::XCBConnection,
};
use xim::{
    x11rb::X11rbServer, InputStyle, Server, ServerError, ServerHandler, UserInputContext,
    XimConnections,
};

use crate::{
    engine::CoreEngine,
    panel::{load_font, PanelRenderer},
};

// ── IC per-context data ───────────────────────────────────────────────────────

// Spot location is stored directly on InputContext by xim (via preedit_spot()).
// No extra per-IC data needed.
struct IcData;

// ── Main handler type ─────────────────────────────────────────────────────────

type MyServer = X11rbServer<Arc<XCBConnection>>;

struct KeyTaoHandler {
    engine: CoreEngine,
    renderer: Option<PanelRenderer>,
    conn: Arc<XCBConnection>,
    panel_win: Window,
    panel_depth: u8,
    gc: Gcontext,
    panel_visible: bool,
    // Keycode → keysym table fetched from the X11 server at init time.
    // Layout: flat array, each keycode has `keysyms_per_keycode` slots.
    // keysym index 0 = unshifted, 1 = shifted.
    keycode_map: Vec<u32>,
    min_keycode: u8,
    keysyms_per_keycode: u8,
}

impl KeyTaoHandler {
    fn new(engine: CoreEngine, conn: Arc<XCBConnection>, screen_num: usize) -> Self {
        let renderer = load_font().map(PanelRenderer::new);
        let setup = conn.setup();
        let screen = &setup.roots[screen_num];
        let root = screen.root;
        let visual = screen.root_visual;
        let depth = screen.root_depth;

        let panel_win = conn.generate_id().expect("gen panel window id");
        conn.create_window(
            depth,
            panel_win,
            root,
            0,
            0,
            300,
            46,
            0,
            WindowClass::INPUT_OUTPUT,
            visual,
            &CreateWindowAux::new()
                .override_redirect(1)
                .background_pixel(0x1e1e2e)
                .event_mask(EventMask::EXPOSURE),
        )
        .expect("create panel window");

        let class = b"keytao-candidate\0keytao-candidate\0";
        conn.change_property8(
            PropMode::REPLACE,
            panel_win,
            AtomEnum::WM_CLASS,
            AtomEnum::STRING,
            class,
        )
        .ok();

        let gc = conn.generate_id().expect("gen gc");
        conn.create_gc(gc, panel_win, &Default::default()).ok();
        conn.flush().ok();

        // Fetch keycode→keysym mapping from the X11 server.
        // This lets us convert raw XIM keycodes to keysyms without needing XKB.
        let setup = conn.setup();
        let min_keycode = setup.min_keycode;
        let max_keycode = setup.max_keycode;
        let count = (max_keycode - min_keycode) as u8 + 1;

        let (keycode_map, keysyms_per_keycode) = conn
            .get_keyboard_mapping(min_keycode, count)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .map(|reply| {
                let kpk = reply.keysyms_per_keycode;
                let syms: Vec<u32> = reply.keysyms.iter().map(|&s| s).collect();
                (syms, kpk)
            })
            .unwrap_or_else(|| {
                tracing::warn!("GetKeyboardMapping failed; keysym lookup disabled");
                (Vec::new(), 1)
            });

        Self {
            engine,
            renderer,
            conn,
            panel_win,
            panel_depth: depth,
            gc,
            panel_visible: false,
            keycode_map,
            min_keycode,
            keysyms_per_keycode,
        }
    }

    fn show_panel(&mut self, state: &ImeState, spot_x: i16, spot_y: i16) {
        if state.candidates.is_empty() && state.preedit.is_empty() {
            self.hide_panel();
            return;
        }
        let Some(renderer) = &self.renderer else {
            return;
        };
        let (pixels, w, h) = renderer.render(state);

        self.conn
            .configure_window(
                self.panel_win,
                &ConfigureWindowAux::new()
                    .x(spot_x as i32)
                    .y(spot_y as i32 - h as i32 - 4)
                    .width(w)
                    .height(h),
            )
            .ok();

        if !self.panel_visible {
            self.conn.map_window(self.panel_win).ok();
            self.panel_visible = true;
        }

        self.conn
            .put_image(
                ImageFormat::Z_PIXMAP,
                self.panel_win,
                self.gc,
                w as u16,
                h as u16,
                0,
                0,
                0,
                self.panel_depth,
                &pixels,
            )
            .ok();
        self.conn.flush().ok();
    }

    fn hide_panel(&mut self) {
        if self.panel_visible {
            self.conn.unmap_window(self.panel_win).ok();
            self.conn.flush().ok();
            self.panel_visible = false;
        }
    }
}

// ── ServerHandler impl ────────────────────────────────────────────────────────

impl ServerHandler<MyServer> for KeyTaoHandler {
    type InputContextData = IcData;
    type InputStyleArray = [InputStyle; 2];

    fn new_ic_data(
        &mut self,
        _server: &mut MyServer,
        _style: InputStyle,
    ) -> Result<IcData, ServerError> {
        Ok(IcData)
    }

    fn input_styles(&self) -> Self::InputStyleArray {
        [
            InputStyle::PREEDIT_CALLBACKS | InputStyle::STATUS_NOTHING,
            InputStyle::PREEDIT_POSITION | InputStyle::STATUS_NOTHING,
        ]
    }

    fn filter_events(&self) -> u32 {
        1 // KeyPress
    }

    fn handle_connect(&mut self, _server: &mut MyServer) -> Result<(), ServerError> {
        Ok(())
    }

    fn handle_create_ic(
        &mut self,
        server: &mut MyServer,
        user_ic: &mut UserInputContext<IcData>,
    ) -> Result<(), ServerError> {
        server.set_event_mask(&user_ic.ic, 1, 0)
    }

    fn handle_destroy_ic(
        &mut self,
        _server: &mut MyServer,
        _user_ic: UserInputContext<IcData>,
    ) -> Result<(), ServerError> {
        self.engine.reset();
        self.hide_panel();
        Ok(())
    }

    fn handle_reset_ic(
        &mut self,
        _server: &mut MyServer,
        _user_ic: &mut UserInputContext<IcData>,
    ) -> Result<String, ServerError> {
        self.engine.reset();
        self.hide_panel();
        Ok(String::new())
    }

    fn handle_set_ic_values(
        &mut self,
        _server: &mut MyServer,
        _user_ic: &mut UserInputContext<IcData>,
    ) -> Result<(), ServerError> {
        // xim stores SpotLocation internally; read via user_ic.ic.preedit_spot()
        Ok(())
    }

    fn handle_set_focus(
        &mut self,
        _server: &mut MyServer,
        _user_ic: &mut UserInputContext<IcData>,
    ) -> Result<(), ServerError> {
        Ok(())
    }

    fn handle_unset_focus(
        &mut self,
        _server: &mut MyServer,
        _user_ic: &mut UserInputContext<IcData>,
    ) -> Result<(), ServerError> {
        self.engine.reset();
        self.hide_panel();
        Ok(())
    }

    fn handle_forward_event(
        &mut self,
        server: &mut MyServer,
        user_ic: &mut UserInputContext<IcData>,
        xev: &x11rb::protocol::xproto::KeyPressEvent,
    ) -> Result<bool, ServerError> {
        // Convert the X11 hardware keycode to a keysym using the keyboard mapping
        // fetched at init time.  xev.detail is the raw X11 keycode.
        // Shift bit (bit 0) in xev.state selects between keysym index 0 (unshifted)
        // and index 1 (shifted).
        let shift = u32::from(xev.state) & 0x0001 != 0;
        let keysym: u32 = if self.keycode_map.is_empty() {
            xev.detail as u32 // fallback: broken, but better than crashing
        } else {
            let kc = xev.detail as usize;
            let min = self.min_keycode as usize;
            let kpk = self.keysyms_per_keycode as usize;
            if kc >= min && kpk > 0 {
                let base = (kc - min) * kpk;
                let idx = if shift && kpk > 1 { base + 1 } else { base };
                self.keycode_map.get(idx).copied().unwrap_or(0)
            } else {
                0
            }
        };

        if keysym == 0 {
            return Ok(false);
        }

        let mods = u32::from(xev.state);

        let result = match self.engine.process_key_result(keysym, mods) {
            Some(r) => r,
            None => return Ok(false),
        };

        let ime_state = result.state;

        let consumed = result.accepted;

        if let Some(text) = &ime_state.committed {
            server.commit(&user_ic.ic, text)?;
        }

        if !ime_state.preedit.is_empty() {
            server.preedit_draw(&mut user_ic.ic, &ime_state.preedit)?;
        } else if ime_state.committed.is_some() {
            server.preedit_draw(&mut user_ic.ic, "")?;
        }

        let spot = user_ic.ic.preedit_spot();
        if ime_state.committed.is_some() && ime_state.preedit.is_empty() {
            self.hide_panel();
        } else {
            self.show_panel(&ime_state, spot.x, spot.y);
        }

        Ok(consumed)
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

pub fn run(engine: CoreEngine) {
    // XWayland starts lazily in niri: DISPLAY may be set in the environment but
    // the actual server socket may not exist yet.  Retry until it is available.
    let (conn, screen_num) = loop {
        match XCBConnection::connect(None) {
            Ok(pair) => break pair,
            Err(e) => {
                tracing::debug!(
                    "X11 connect failed ({e}), retrying in 1 s (XWayland not ready yet?)"
                );
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        }
    };
    let conn = Arc::new(conn);

    let server = match X11rbServer::init(Arc::clone(&conn), screen_num, "keytao", xim::ALL_LOCALES)
    {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("XIM server init failed: {e} (another XIM server named 'keytao' may already be running)");
            return;
        }
    };

    let mut server = server;
    let mut connections = XimConnections::new();
    let mut handler = KeyTaoHandler::new(engine, Arc::clone(&conn), screen_num);

    tracing::info!("X11 XIM server running as @server=keytao");
    tracing::info!("Set XMODIFIERS=@im=keytao in your session to use this IME");

    loop {
        match conn.wait_for_event() {
            Ok(event) => {
                if let Err(e) = server.filter_event(&event, &mut connections, &mut handler) {
                    tracing::error!("XIM error: {e}");
                }
                if let Event::Expose(ev) = &event {
                    // Candidate window was covered and re-exposed.
                    // The next key event will re-render; nothing to do here.
                    let _ = ev;
                }
            }
            Err(e) => {
                tracing::error!("X11 connection error: {e}");
                break;
            }
        }
    }
}
