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
    x11rb::X11rbServer, InputContext, InputStyle, Server, ServerError, ServerHandler,
    UserInputContext, XimConnections,
};

use crate::{
    engine::{CoreEngine, ImeSession},
    panel::{load_font, PanelRenderer},
};

const MOD_CONTROL: u32 = 0x0004;
const MOD_MOD1: u32 = 0x0008;

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

fn commit_text(server: &mut MyServer, ic: &InputContext, text: &str) -> Result<(), ServerError> {
    tracing::info!("XIM commit text: {text:?}");
    server.commit(ic, text)
}

fn client_supports_preedit(ic: &InputContext) -> bool {
    let style = ic.input_style();
    style.contains(InputStyle::PREEDIT_CALLBACKS) || style.contains(InputStyle::PREEDIT_POSITION)
}

fn draw_client_preedit(
    server: &mut MyServer,
    ic: &mut InputContext,
    text: &str,
) -> Result<(), ServerError> {
    if client_supports_preedit(ic) {
        server.preedit_draw(ic, text)?;
    }
    Ok(())
}

// ── IC per-context data ───────────────────────────────────────────────────────

// Spot location is stored directly on InputContext by xim (via preedit_spot()).
// No extra per-IC data needed.
struct IcData {
    session: ImeSession,
}

// ── Main handler type ─────────────────────────────────────────────────────────

type MyServer = X11rbServer<Arc<XCBConnection>>;

struct KeyTaoHandler {
    engine: CoreEngine,
    renderer: Option<PanelRenderer>,
    conn: Arc<XCBConnection>,
    root: Window,
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
        let renderer = load_font().and_then(PanelRenderer::new);
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
            root,
            panel_win,
            panel_depth: depth,
            gc,
            panel_visible: false,
            keycode_map,
            min_keycode,
            keysyms_per_keycode,
        }
    }

    fn show_panel(&mut self, user_ic: &UserInputContext<IcData>, state: &ImeState) {
        if state.candidates.is_empty() {
            self.hide_panel();
            return;
        }
        let Some(renderer) = &self.renderer else {
            return;
        };
        let (pixels, w, h) = renderer.render(state);
        let spot = user_ic.ic.preedit_spot();
        let anchor = user_ic
            .ic
            .app_focus_win()
            .or_else(|| user_ic.ic.app_win())
            .map(|window| window.get())
            .unwrap_or_else(|| user_ic.ic.client_win());
        let (root_x, root_y) = self
            .conn
            .translate_coordinates(anchor, self.root, spot.x, spot.y)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .map(|reply| (reply.dst_x as i32, reply.dst_y as i32))
            .unwrap_or((spot.x as i32, spot.y as i32));

        self.conn
            .configure_window(
                self.panel_win,
                &ConfigureWindowAux::new()
                    .x(root_x)
                    .y(root_y + 4)
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
        style: InputStyle,
    ) -> Result<IcData, ServerError> {
        tracing::info!("XIM NewICData style={style:?}");
        let session = self
            .engine
            .create_session()
            .map_err(ServerError::Internal)?;
        Ok(IcData { session })
    }

    fn input_styles(&self) -> Self::InputStyleArray {
        [
            // Electron/Chromium X11 clients can get stuck when XIM drives an
            // in-client preedit region. Keep composition in keytao's own panel
            // and use XIM only for key filtering + final commit.
            InputStyle::PREEDIT_NOTHING | InputStyle::STATUS_NOTHING,
            InputStyle::PREEDIT_NONE | InputStyle::STATUS_NONE,
        ]
    }

    fn filter_events(&self) -> u32 {
        1 // KeyPress
    }

    fn handle_connect(&mut self, _server: &mut MyServer) -> Result<(), ServerError> {
        tracing::info!("XIM client connected");
        Ok(())
    }

    fn handle_create_ic(
        &mut self,
        server: &mut MyServer,
        user_ic: &mut UserInputContext<IcData>,
    ) -> Result<(), ServerError> {
        tracing::info!(
            "XIM CreateIC client_win={} style={:?}",
            user_ic.ic.client_win(),
            user_ic.ic.input_style()
        );
        server.set_event_mask(&user_ic.ic, 1, 0)
    }

    fn handle_destroy_ic(
        &mut self,
        _server: &mut MyServer,
        user_ic: UserInputContext<IcData>,
    ) -> Result<(), ServerError> {
        tracing::info!("XIM DestroyIC client_win={}", user_ic.ic.client_win());
        user_ic.user_data.session.reset();
        self.hide_panel();
        Ok(())
    }

    fn handle_reset_ic(
        &mut self,
        _server: &mut MyServer,
        user_ic: &mut UserInputContext<IcData>,
    ) -> Result<String, ServerError> {
        tracing::info!("XIM ResetIC client_win={}", user_ic.ic.client_win());
        user_ic.user_data.session.reset();
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
        user_ic: &mut UserInputContext<IcData>,
    ) -> Result<(), ServerError> {
        tracing::info!("XIM SetFocus client_win={}", user_ic.ic.client_win());
        Ok(())
    }

    fn handle_unset_focus(
        &mut self,
        _server: &mut MyServer,
        user_ic: &mut UserInputContext<IcData>,
    ) -> Result<(), ServerError> {
        tracing::info!("XIM UnsetFocus client_win={}", user_ic.ic.client_win());
        user_ic.user_data.session.reset();
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
        tracing::info!(
            "XIM ForwardEvent client_win={} keycode={} keysym={keysym:#x} mods={mods:#x}",
            user_ic.ic.client_win(),
            xev.detail
        );

        let before_state = user_ic.user_data.session.state();
        if should_bypass_empty_composition(keysym, mods, &before_state) {
            self.hide_panel();
            draw_client_preedit(server, &mut user_ic.ic, "")?;
            return Ok(false);
        }
        if is_enter_key(keysym) && !before_state.preedit.is_empty() {
            draw_client_preedit(server, &mut user_ic.ic, "")?;
            commit_text(server, &user_ic.ic, &before_state.preedit)?;
            user_ic.user_data.session.reset();
            self.hide_panel();
            return Ok(true);
        }
        if is_candidate_select_key(keysym) && !before_state.candidates.is_empty() {
            let index = before_state
                .highlighted_candidate_index
                .min(before_state.candidates.len().saturating_sub(1));
            if let Some(ime_state) = user_ic.user_data.session.select_candidate(index) {
                if let Some(text) = &ime_state.committed {
                    draw_client_preedit(server, &mut user_ic.ic, "")?;
                    commit_text(server, &user_ic.ic, text)?;
                }
                draw_client_preedit(server, &mut user_ic.ic, "")?;
                if ime_state.candidates.is_empty() {
                    self.hide_panel();
                } else {
                    self.show_panel(user_ic, &ime_state);
                }
                return Ok(true);
            }
        }

        let result = match user_ic.user_data.session.process_key_result(keysym, mods) {
            Some(r) => r,
            None => return Ok(false),
        };

        let ime_state = result.state;

        let consumed = result.accepted;

        if let Some(text) = &ime_state.committed {
            draw_client_preedit(server, &mut user_ic.ic, "")?;
            commit_text(server, &user_ic.ic, text)?;
        }

        if !ime_state.preedit.is_empty() {
            draw_client_preedit(server, &mut user_ic.ic, &ime_state.preedit)?;
        } else if ime_state.committed.is_some() {
            draw_client_preedit(server, &mut user_ic.ic, "")?;
        }

        if ime_state.candidates.is_empty() {
            self.hide_panel();
        } else {
            self.show_panel(user_ic, &ime_state);
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
