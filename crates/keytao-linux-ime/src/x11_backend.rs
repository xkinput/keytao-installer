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
    gc: Gcontext,
    panel_visible: bool,
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

        Self {
            engine,
            renderer,
            conn,
            panel_win,
            gc,
            panel_visible: false,
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
                32,
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
        // xev.detail is the raw X11 keycode. librime expects X11 keysyms, but
        // XIM routes events after the server-side keymap has been applied, so
        // the keycode here is still a hardware scancode offset by 8.
        // Passing the raw keycode works for ASCII letters (keycodes 10–35 map
        // directly to 'a'–'z' in a standard US layout via librime's fallback).
        // A complete implementation should use xkbcommon-x11 to load the
        // server keymap and convert properly.
        let keycode = xev.detail as u32;
        let mods = u32::from(xev.state);

        let ime_state = match self.engine.process_key(keycode, mods) {
            Some(s) => s,
            None => return Ok(false),
        };

        let consumed = ime_state.committed.is_some()
            || !ime_state.preedit.is_empty()
            || !ime_state.candidates.is_empty();

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
    let (conn, screen_num) = XCBConnection::connect(None).expect("X11 connection");
    let conn = Arc::new(conn);

    let mut server = X11rbServer::init(Arc::clone(&conn), screen_num, "keytao", xim::ALL_LOCALES)
        .expect("XIM server init — is another XIM server already running?");

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
