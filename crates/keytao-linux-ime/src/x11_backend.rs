//! X11 backend: XIM server (via `xim` crate) + XCB candidate overlay window.
//!
//! Architecture:
//!   keytao-ime registers as an XIM server named "@server=keytao".
//!   Applications that use XIM (Gtk2, Qt4, terminal emulators) send key events
//!   here via the X11 XIM protocol.  We process them with librime and commit
//!   the resulting text back through XIM.
//!
//!   The candidate panel is an override-redirect XCB window rendered with the
//!   same PanelRenderer used on Wayland.

use std::collections::HashMap;

use keytao_core::ImeState;
use x11rb::{
    connection::Connection as _,
    protocol::{
        xproto::{
            Atom, AtomEnum, ConfigureWindowAux, ConnectionExt as _, CreateWindowAux, EventMask,
            Gcontext, ImageFormat, PropMode, Rectangle, VisualClass, Window, WindowClass,
        },
        Event,
    },
    xcb_ffi::XCBConnection,
    COPY_FROM_PARENT,
};
use xim::{
    x11rb::{HasConnection, X11rbServer},
    AHashMap, InputStyle, Server, ServerHandler,
};

use crate::{engine::CoreEngine, panel::{load_font, PanelRenderer}};

// ── IC (Input Context) state ──────────────────────────────────────────────────

struct Ic {
    spot_x: i16,
    spot_y: i16,
}

// ── XIM handler ───────────────────────────────────────────────────────────────

struct KeyTaoHandler {
    engine: CoreEngine,
    renderer: Option<PanelRenderer>,

    // XCB
    conn: std::sync::Arc<XCBConnection>,
    screen_num: usize,
    panel_win: Window,
    gc: Gcontext,
    panel_visible: bool,

    // Per-connection, per-IC state
    ics: HashMap<(u16, u16), Ic>,
}

impl KeyTaoHandler {
    fn new(engine: CoreEngine, conn: std::sync::Arc<XCBConnection>, screen_num: usize) -> Self {
        let renderer = load_font().map(PanelRenderer::new);
        let setup = conn.setup();
        let screen = &setup.roots[screen_num];
        let root = screen.root;
        let visual = screen.root_visual;
        let depth = screen.root_depth;

        // Create the candidate overlay window (hidden initially)
        let panel_win = conn.generate_id().expect("gen id");
        conn.create_window(
            depth,
            panel_win,
            root,
            0, 0,            // x, y (will be moved before showing)
            300, 46,         // w, h
            0,               // border
            WindowClass::INPUT_OUTPUT,
            visual,
            &CreateWindowAux::new()
                .override_redirect(1)
                .background_pixel(0x1e1e2e) // Catppuccin base
                .event_mask(EventMask::EXPOSURE),
        )
        .expect("create panel window");

        // Set WM_CLASS so compositors know what it is
        let class = b"keytao-candidate\0keytao-candidate\0";
        conn.change_property8(
            PropMode::REPLACE, panel_win,
            AtomEnum::WM_CLASS, AtomEnum::STRING,
            class,
        ).ok();

        let gc = conn.generate_id().expect("gen gc");
        conn.create_gc(gc, panel_win, &Default::default()).ok();

        Self {
            engine, renderer,
            conn, screen_num,
            panel_win, gc,
            panel_visible: false,
            ics: HashMap::new(),
        }
    }

    fn show_panel(&mut self, state: &ImeState, spot_x: i16, spot_y: i16) {
        let has_content = !state.candidates.is_empty() || !state.preedit.is_empty();
        if !has_content {
            self.hide_panel();
            return;
        }

        let Some(renderer) = &self.renderer else { return };
        let (pixels, w, h) = renderer.render(state);

        // Move/resize window to spot position
        self.conn.configure_window(
            self.panel_win,
            &ConfigureWindowAux::new()
                .x(spot_x as i32)
                .y(spot_y as i32 - h as i32 - 4)
                .width(w)
                .height(h),
        ).ok();

        if !self.panel_visible {
            self.conn.map_window(self.panel_win).ok();
            self.panel_visible = true;
        }

        // Blit pixels (BGRA → XCB put_image with 32bpp)
        self.conn.put_image(
            ImageFormat::Z_PIXMAP,
            self.panel_win,
            self.gc,
            w as u16, h as u16,
            0, 0,
            0, 32,
            &pixels,
        ).ok();

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

impl ServerHandler for KeyTaoHandler {
    type IMAttributes = AHashMap<xim::Attr, xim::AttrValue>;
    type ICAttributes = AHashMap<xim::Attr, xim::AttrValue>;
    type ICAttributeList = Vec<xim::Attr>;
    type InputStyleList = [InputStyle; 2];

    fn input_styles(&self) -> &Self::InputStyleList {
        &[
            InputStyle::PREEDIT_CALLBACKS | InputStyle::STATUS_NOTHING,
            InputStyle::PREEDIT_POSITION  | InputStyle::STATUS_NOTHING,
        ]
    }

    fn filter_events(&self) -> u32 {
        // We want KeyPress + KeyRelease
        0x0003
    }

    fn handle_connect(
        &mut self,
        _server: &mut xim::x11rb::X11rbServer<XCBConnection>,
        _conn_id: u16,
    ) {
    }

    fn handle_open(
        &mut self,
        _server: &mut xim::x11rb::X11rbServer<XCBConnection>,
        _conn_id: u16,
        _locale: xim::Locale,
    ) -> bool {
        true
    }

    fn handle_close(
        &mut self,
        _server: &mut xim::x11rb::X11rbServer<XCBConnection>,
        _conn_id: u16,
    ) {
    }

    fn handle_create_ic(
        &mut self,
        _server: &mut xim::x11rb::X11rbServer<XCBConnection>,
        conn_id: u16,
        ic_id: u16,
        input_style: InputStyle,
        _ic_attrs: Self::ICAttributes,
    ) -> bool {
        self.ics.insert((conn_id, ic_id), Ic { spot_x: 0, spot_y: 0 });
        tracing::debug!("create IC ({conn_id}, {ic_id}) style={input_style:?}");
        true
    }

    fn handle_destroy_ic(
        &mut self,
        _server: &mut xim::x11rb::X11rbServer<XCBConnection>,
        conn_id: u16,
        ic_id: u16,
    ) {
        self.ics.remove(&(conn_id, ic_id));
        self.engine.reset();
        self.hide_panel();
    }

    fn handle_reset_ic(
        &mut self,
        _server: &mut xim::x11rb::X11rbServer<XCBConnection>,
        _conn_id: u16,
        _ic_id: u16,
    ) -> String {
        self.engine.reset();
        self.hide_panel();
        String::new()
    }

    fn handle_set_ic_values(
        &mut self,
        _server: &mut xim::x11rb::X11rbServer<XCBConnection>,
        conn_id: u16,
        ic_id: u16,
        ic_attrs: Self::ICAttributes,
    ) {
        // Store spot location for candidate window positioning
        if let Some(ic) = self.ics.get_mut(&(conn_id, ic_id)) {
            if let Some(xim::AttrValue::Spot(p)) = ic_attrs.get(&xim::Attr::SpotLocation) {
                ic.spot_x = p.x;
                ic.spot_y = p.y;
            }
        }
    }

    fn handle_get_ic_values(
        &mut self,
        _server: &mut xim::x11rb::X11rbServer<XCBConnection>,
        _conn_id: u16,
        _ic_id: u16,
    ) -> Self::ICAttributeList {
        vec![]
    }

    fn handle_set_focus(
        &mut self,
        _server: &mut xim::x11rb::X11rbServer<XCBConnection>,
        _conn_id: u16,
        _ic_id: u16,
    ) {
    }

    fn handle_unset_focus(
        &mut self,
        _server: &mut xim::x11rb::X11rbServer<XCBConnection>,
        _conn_id: u16,
        _ic_id: u16,
    ) {
        self.engine.reset();
        self.hide_panel();
    }

    fn handle_forward_event(
        &mut self,
        server: &mut xim::x11rb::X11rbServer<XCBConnection>,
        conn_id: u16,
        ic_id: u16,
        _serial: u16,
        event: &xim::KeyEvent,
    ) -> bool {
        // Ignore key-release
        if !event.is_press { return false; }

        let keysym = event.keysym;
        let mods = event.state as u32;

        let ime_state = match self.engine.process_key(keysym, mods) {
            Some(s) => s,
            None => return false,
        };

        let consumed = ime_state.committed.is_some()
            || !ime_state.preedit.is_empty()
            || !ime_state.candidates.is_empty();

        // Commit text to client
        if let Some(text) = &ime_state.committed {
            let _ = server.commit(conn_id, ic_id, xim::CommitData::Chars {
                syncronous: false,
                string: text.clone(),
            });
        }

        // Update preedit callbacks
        if !ime_state.preedit.is_empty() {
            let _ = server.preedit_draw(conn_id, ic_id, xim::PreeditDrawData {
                caret: ime_state.cursor as i32,
                chg_first: 0,
                chg_length: -1,
                status: 0,
                text: ime_state.preedit.clone(),
            });
        } else {
            let _ = server.preedit_done(conn_id, ic_id);
        }

        // Update candidate panel
        let ic = self.ics.get(&(conn_id, ic_id)).cloned().unwrap_or(Ic { spot_x: 0, spot_y: 600 });
        if ime_state.committed.is_some() && ime_state.preedit.is_empty() {
            self.hide_panel();
        } else {
            self.show_panel(&ime_state, ic.spot_x, ic.spot_y);
        }

        consumed
    }

    fn handle_trigger_notify(
        &mut self,
        _server: &mut xim::x11rb::X11rbServer<XCBConnection>,
        _conn_id: u16,
        _ic_id: u16,
    ) {
    }
}

impl Clone for Ic {
    fn clone(&self) -> Self {
        Ic { spot_x: self.spot_x, spot_y: self.spot_y }
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

pub fn run(engine: CoreEngine) {
    let (conn, screen_num) = XCBConnection::connect(None).expect("X11 connection");
    let conn = std::sync::Arc::new(conn);

    let mut server = X11rbServer::init(
        conn.clone(),
        screen_num,
        b"@server=keytao",
        xim::InputStyleList::new(&[
            InputStyle::PREEDIT_CALLBACKS | InputStyle::STATUS_NOTHING,
        ]),
        (),
    )
    .expect("XIM server init — ensure XMODIFIERS=@im=keytao and no other XIM server is running");

    let handler = KeyTaoHandler::new(engine, conn.clone(), screen_num);

    tracing::info!("X11 XIM server running as @server=keytao");
    tracing::info!("Set XMODIFIERS=@im=keytao in your session to use this IME");

    // Event loop: interleave XIM messages with panel expose events
    loop {
        if let Err(e) = server.poll(&mut KeyTaoHandlerWrapper(std::cell::RefCell::new(&mut ()),)) {
            tracing::error!("XIM poll error: {e}");
            break;
        }
        // Also drain pending XCB events (e.g. Expose on panel window)
        while let Ok(Some(event)) = conn.poll_for_event() {
            if let Event::Expose(_) = event {
                // Re-blit on expose if needed
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}
