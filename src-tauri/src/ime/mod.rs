use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImeCandidate {
    pub text: String,
    pub comment: Option<String>,
}

/// State sent to the webview panel via "ime:panel" event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PanelState {
    pub visible: bool,
    pub preedit: String,
    pub cursor: usize,
    pub candidates: Vec<ImeCandidate>,
    pub page: usize,
    pub is_last_page: bool,
    pub select_keys: Option<String>,
    /// Cursor screen position (compositor coordinates) from text_input_rectangle.
    pub cursor_x: i32,
    pub cursor_y: i32,
    /// Text line height, used to offset panel below the cursor line.
    pub cursor_h: i32,
}

impl PanelState {
    pub fn hidden() -> Self {
        Self {
            visible: false,
            preedit: String::new(),
            cursor: 0,
            candidates: vec![],
            page: 0,
            is_last_page: true,
            select_keys: None,
            cursor_x: 0,
            cursor_y: 0,
            cursor_h: 20,
        }
    }
}

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "linux")]
pub use linux::{spawn, TrayArc, TrayShared};
