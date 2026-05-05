//! Candidate panel renderer.
//!
//! Fixed dark theme (Catppuccin Mocha).  Renders to a raw BGRA pixel buffer
//! suitable for both X11 XCB image upload and Wayland wl_shm.
//!
//! Layout (single horizontal bar):
//!
//!   ┌──────────────────────────────────────────────────────┐
//!   │  [preedit]                                           │  ← 18px row
//!   │  1.候选  2.候选  3.候选  ...         ‹ page ›       │  ← 24px row
//!   └──────────────────────────────────────────────────────┘

use keytao_core::ImeState;
use tiny_skia::*;

// ── Catppuccin Mocha ──────────────────────────────────────────────────────────

const BG: [u8; 4] = [0x2e, 0x1e, 0x1e, 0xff]; // BGRA 0x1e1e2e
const FG: [u8; 4] = [0xf4, 0xd6, 0xcd, 0xff]; // 0xcdd6f4
const ACCENT: [u8; 4] = [0xf7, 0xa6, 0xcb, 0xff]; // mauve 0xcba6f7
const PREEDIT_COLOR: [u8; 4] = [0xeb, 0xdc, 0x89, 0xff]; // sky 0x89dceb
const DIM: [u8; 4] = [0x70, 0x5b, 0x58, 0xff]; // overlay0 0x585b70
const SEP: [u8; 4] = [0x50, 0x48, 0x45, 0xff]; // surface1 0x45475a → darker

const FONT_SIZE: f32 = 18.0;
const PREEDIT_SIZE: f32 = 13.0;
const PADDING: f32 = 10.0;
const CAND_GAP: f32 = 14.0;
const HEIGHT: u32 = 46;
const MIN_WIDTH: u32 = 260;

// ── Font loader ───────────────────────────────────────────────────────────────

/// Load the first available CJK-capable font, trying common paths then fc-match.
pub fn load_font() -> Option<fontdue::Font> {
    const PATHS: &[&str] = &[
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/noto/NotoSansCJKsc-Regular.otf",
        "/usr/share/fonts/wqy-zenhei/wqy-zenhei.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
    ];
    for p in PATHS {
        if let Ok(data) = std::fs::read(p) {
            if let Ok(f) =
                fontdue::Font::from_bytes(data.as_slice(), fontdue::FontSettings::default())
            {
                tracing::info!("loaded font: {p}");
                return Some(f);
            }
        }
    }
    // fall back to fc-match
    let out = std::process::Command::new("fc-match")
        .args(["--format=%{file}", "sans:lang=zh"])
        .output()
        .ok()?;
    if out.status.success() {
        let path = String::from_utf8(out.stdout).ok()?;
        let path = path.trim();
        if !path.is_empty() {
            if let Ok(data) = std::fs::read(path) {
                if let Ok(f) =
                    fontdue::Font::from_bytes(data.as_slice(), fontdue::FontSettings::default())
                {
                    tracing::info!("loaded font via fc-match: {path}");
                    return Some(f);
                }
            }
        }
    }
    tracing::warn!("no CJK font found; candidate text may be incomplete");
    None
}

// ── Renderer ──────────────────────────────────────────────────────────────────

pub struct PanelRenderer {
    font: fontdue::Font,
}

impl PanelRenderer {
    pub fn new(font: fontdue::Font) -> Self {
        Self { font }
    }

    /// Render panel to a BGRA byte buffer.  Returns (bytes, width, height).
    pub fn render(&self, state: &ImeState) -> (Vec<u8>, u32, u32) {
        let cand_width: f32 = state
            .candidates
            .iter()
            .enumerate()
            .map(|(i, c)| self.text_width(&format!("{}. {}", i + 1, c.text), FONT_SIZE) + CAND_GAP)
            .sum();
        let preedit_width = if state.preedit.is_empty() {
            0.0
        } else {
            self.text_width(&state.preedit, PREEDIT_SIZE) + PADDING * 2.0
        };
        let nav_width = if state.candidates.is_empty() {
            0.0
        } else {
            38.0
        };
        let width = (MIN_WIDTH as f32)
            .max(cand_width + PADDING * 2.0 + nav_width)
            .max(preedit_width) as u32;

        let mut pm = Pixmap::new(width, HEIGHT).expect("pixmap alloc");

        // Background
        pm.fill(Color::from_rgba8(BG[2], BG[1], BG[0], 255));

        // Preedit
        let cand_y = if state.preedit.is_empty() {
            HEIGHT as f32 / 2.0 + FONT_SIZE / 2.0 - 3.0
        } else {
            self.draw_text(
                &mut pm,
                &state.preedit,
                PADDING,
                14.0,
                PREEDIT_COLOR,
                PREEDIT_SIZE,
            );
            HEIGHT as f32 - 8.0
        };

        // Candidates
        let mut x = PADDING;
        for (i, cand) in state.candidates.iter().enumerate() {
            let label = format!("{}. ", i + 1);
            let color = if i == 0 { ACCENT } else { FG };
            self.draw_text(&mut pm, &label, x, cand_y, DIM, FONT_SIZE);
            x += self.text_width(&label, FONT_SIZE);
            self.draw_text(&mut pm, &cand.text, x, cand_y, color, FONT_SIZE);
            x += self.text_width(&cand.text, FONT_SIZE) + CAND_GAP;
        }

        // Page arrows
        if !state.candidates.is_empty() {
            let ax = width as f32 - nav_width + 4.0;
            let prev_color = if state.page == 0 { DIM } else { FG };
            self.draw_text(&mut pm, "‹", ax, cand_y, prev_color, FONT_SIZE);
            let next_color = if state.is_last_page { DIM } else { FG };
            self.draw_text(&mut pm, "›", ax + 18.0, cand_y, next_color, FONT_SIZE);
        }

        // Bottom separator
        let mut paint = Paint::default();
        paint.set_color_rgba8(SEP[2], SEP[1], SEP[0], 255);
        pm.fill_rect(
            Rect::from_xywh(0.0, HEIGHT as f32 - 1.0, width as f32, 1.0).unwrap(),
            &paint,
            Transform::identity(),
            None,
        );

        // Convert RGBA (tiny-skia) → BGRA (platform native)
        let mut out: Vec<u8> = pm.data().to_vec();
        for px in out.chunks_exact_mut(4) {
            px.swap(0, 2); // R↔B
        }

        (out, width, HEIGHT)
    }

    fn draw_text(
        &self,
        pm: &mut Pixmap,
        text: &str,
        mut x: f32,
        baseline: f32,
        color: [u8; 4],
        size: f32,
    ) {
        for ch in text.chars() {
            let (metrics, bitmap) = self.font.rasterize(ch, size);
            let gx = (x + metrics.xmin as f32) as i32;
            let gy = (baseline - metrics.height as f32 - metrics.ymin as f32) as i32;
            for row in 0..metrics.height {
                for col in 0..metrics.width {
                    let alpha = bitmap[row * metrics.width + col];
                    if alpha == 0 {
                        continue;
                    }
                    let px = gx + col as i32;
                    let py = gy + row as i32;
                    if px < 0 || py < 0 || px >= pm.width() as i32 || py >= pm.height() as i32 {
                        continue;
                    }
                    let a = alpha as f32 / 255.0;
                    let off = (py as usize * pm.width() as usize + px as usize) * 4;
                    let d = pm.data_mut();
                    // Blend onto background (tiny-skia stores RGBA)
                    d[off] = lerp(d[off], color[0], a); // R
                    d[off + 1] = lerp(d[off + 1], color[1], a); // G
                    d[off + 2] = lerp(d[off + 2], color[2], a); // B
                    d[off + 3] = 255;
                }
            }
            x += metrics.advance_width;
        }
    }

    fn text_width(&self, text: &str, size: f32) -> f32 {
        text.chars()
            .map(|c| self.font.metrics(c, size).advance_width)
            .sum()
    }
}

#[inline]
fn lerp(bg: u8, fg: u8, a: f32) -> u8 {
    (bg as f32 * (1.0 - a) + fg as f32 * a) as u8
}
