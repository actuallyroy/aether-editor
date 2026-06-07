// Centered overlay shown in the editor area when the active tab is a binary /
// unsupported-encoding file (VSCode's "The file is not displayed … Open Anyway").
// Owns its own layout + hit-testing so the click region, button background, and
// label can never drift apart (single source of truth via `button_rect`).

use glyphon::{Color, FontSystem, TextArea};

use crate::quad::Quad;
use crate::theme;
use crate::widgets::{IconButton, Rect, TextLabel};

const WARN_GLYPH: char = '\u{ea6c}'; // codicon "warning"
const LINE1: &str = "The file is not displayed in the editor because it is";
const LINE2: &str = "either binary or uses an unsupported text encoding.";
const BTN_LABEL: &str = "Open Anyway";

pub struct BinaryPlaceholder {
    icon: IconButton,
    line1: TextLabel,
    line2: TextLabel,
    btn: TextLabel,
}

fn color4(c: [f32; 4]) -> Color {
    Color::rgb((c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8)
}

impl BinaryPlaceholder {
    pub fn new(fs: &mut FontSystem) -> Self {
        let icon = IconButton::new(fs, WARN_GLYPH, theme::icon_family(WARN_GLYPH), theme::zpx(56.0));
        let mut line1 = TextLabel::new(fs, theme::zpx(700.0), theme::UI_LINE_HEIGHT());
        let mut line2 = TextLabel::new(fs, theme::zpx(700.0), theme::UI_LINE_HEIGHT());
        let mut btn = TextLabel::new(fs, theme::zpx(200.0), theme::UI_LINE_HEIGHT());
        line1.set(fs, LINE1, theme::UI_FAMILY());
        line2.set(fs, LINE2, theme::UI_FAMILY());
        btn.set(fs, BTN_LABEL, theme::UI_FAMILY());
        Self { icon, line1, line2, btn }
    }

    /// Re-shape every label/icon at the current UI zoom (call once per frame before
    /// drawing). No-op unless the zoom epoch changed.
    pub fn prepare(&mut self, fs: &mut FontSystem) {
        self.icon.reshape(fs);
        self.line1.reshape(fs);
        self.line2.reshape(fs);
        self.btn.reshape(fs);
    }

    /// The clickable "Open Anyway" button rect, centered under the message. Single
    /// source of truth for hit-testing, the background quad, and the label.
    pub fn button_rect(&self, region: Rect) -> Rect {
        let w = self.btn.width() + theme::zpx(40.0);
        let h = theme::UI_LINE_HEIGHT() + theme::zpx(16.0);
        let cx = region.x + region.w * 0.5;
        let cy = region.y + region.h * 0.5;
        // Below the two message lines (which sit just under center).
        Rect { x: cx - w * 0.5, y: cy + theme::UI_LINE_HEIGHT() * 2.0 + theme::zpx(24.0), w, h }
    }

    pub fn hit_button(&self, region: Rect, p: (f32, f32)) -> bool {
        self.button_rect(region).contains(p)
    }

    pub fn draw_quads(&self, region: Rect, hovered: bool, out: &mut Vec<Quad>) {
        let bg = if hovered { theme::ACCENT_DIM() } else { theme::ACCENT() };
        out.push(self.button_rect(region).rounded_quad(bg, theme::zpx(4.0)));
    }

    pub fn draw<'a>(&'a self, region: Rect, areas: &mut Vec<TextArea<'a>>) {
        let cx = region.x + region.w * 0.5;
        let cy = region.y + region.h * 0.5;
        let lh = theme::UI_LINE_HEIGHT();
        // Warning icon, centered, above the text.
        let isz = theme::zpx(56.0);
        let icon_rect = Rect { x: cx - isz * 0.5, y: cy - isz - theme::zpx(24.0), w: isz, h: isz };
        self.icon.draw(icon_rect, color4(theme::DIAGNOSTIC_WARNING()), areas);
        // Two message lines, centered, just below the vertical midpoint.
        let l1 = Rect { x: region.x, y: cy, w: region.w, h: lh };
        let l2 = Rect { x: region.x, y: cy + lh, w: region.w, h: lh };
        self.line1.draw_center(l1, theme::FG_DIM(), areas);
        self.line2.draw_center(l2, theme::FG_DIM(), areas);
        // Button label, centered in the button rect.
        self.btn.draw_center(self.button_rect(region), Color::rgb(255, 255, 255), areas);
    }
}
