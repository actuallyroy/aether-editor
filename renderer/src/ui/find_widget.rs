// Floating find/replace widget anchored to the editor's top-right, modeled on
// VSCode's. Owns its two text inputs + icon buttons and computes its own geometry;
// every dimension scales with the UI zoom (theme::zpx), so it stays crisp and
// readable at any window scale. State (open/focused/options/matches) lives on the
// app's `FindBarState`; this component is the reactive view + hit-testing.

use glyphon::{FontSystem, TextArea};
use winit::window::CursorIcon;

use crate::quad::Quad;
use crate::theme;
use crate::widgets::{Rect, TextInput, TextLabel};

/// A clickable element of the widget.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FindBtn {
    Expand,     // toggle the replace row
    Case,       // match case
    Word,       // whole word
    Regex,      // use regex
    Prev,       // previous match
    Next,       // next match
    Close,      // close the widget
    Replace,    // replace current
    ReplaceAll, // replace all
}

/// Resolved rects for one frame (depends on the editor region + whether replace is open).
pub struct FindLayout {
    pub panel: Rect,
    pub expand: Rect,
    pub find_input: Rect, // full find box
    pub find_text: Rect,  // editable sub-rect (excludes the inline option toggles)
    pub case: Rect,
    pub word: Rect,
    pub regex: Rect,
    pub count: Rect,
    pub prev: Rect,
    pub next: Rect,
    pub close: Rect,
    pub replace_input: Option<Rect>,
    pub replace_btn: Option<Rect>,
    pub replace_all: Option<Rect>,
}

pub struct FindWidget {
    pub query: TextInput,
    pub replace: TextInput,
    ic_expand_open: TextLabel,
    ic_expand_closed: TextLabel,
    ic_case: TextLabel,
    ic_word: TextLabel,
    ic_regex: TextLabel,
    ic_prev: TextLabel,
    ic_next: TextLabel,
    ic_close: TextLabel,
    ic_replace: TextLabel,
    ic_replace_all: TextLabel,
    count: TextLabel,
    pub hover: Option<FindBtn>,
}

impl FindWidget {
    pub fn new(fs: &mut FontSystem) -> Self {
        let icon = |fs: &mut FontSystem, c: char| {
            let mut l = TextLabel::new(fs, 40.0, theme::zpx(26.0));
            l.set(fs, &c.to_string(), theme::ICON_FAMILY);
            l
        };
        let mut query = TextInput::new(fs, 600.0, 30.0);
        query.set_placeholder(fs, " Find");
        let mut replace = TextInput::new(fs, 600.0, 30.0);
        replace.set_placeholder(fs, " Replace");
        let mut count = TextLabel::new(fs, 200.0, theme::zpx(26.0));
        count.set(fs, "No results", theme::UI_FAMILY());
        Self {
            query,
            replace,
            ic_expand_open: icon(fs, theme::ICON_CHEVRON_DOWN),
            ic_expand_closed: icon(fs, theme::ICON_CHEVRON_RIGHT),
            ic_case: icon(fs, theme::ICON_CASE),
            ic_word: icon(fs, theme::ICON_WORD),
            ic_regex: icon(fs, theme::ICON_REGEX),
            ic_prev: icon(fs, theme::ICON_ARROW_UP),
            ic_next: icon(fs, theme::ICON_ARROW_DOWN),
            ic_close: icon(fs, theme::ICON_CLOSE),
            ic_replace: icon(fs, theme::ICON_REPLACE),
            ic_replace_all: icon(fs, theme::ICON_REPLACE_ALL),
            count,
            hover: None,
        }
    }

    pub fn set_count(&mut self, fs: &mut FontSystem, text: &str) {
        self.count.set(fs, text, theme::UI_FAMILY());
    }

    pub fn reshape(&mut self, fs: &mut FontSystem) {
        self.query.rezoom(fs);
        self.replace.rezoom(fs);
        for l in [
            &mut self.ic_expand_open,
            &mut self.ic_expand_closed,
            &mut self.ic_case,
            &mut self.ic_word,
            &mut self.ic_regex,
            &mut self.ic_prev,
            &mut self.ic_next,
            &mut self.ic_close,
            &mut self.ic_replace,
            &mut self.ic_replace_all,
            &mut self.count,
        ] {
            l.reshape(fs);
        }
    }

    /// Resolve the widget rects for the editor region `editor` (zoom-scaled).
    pub fn layout(editor: Rect, replace_open: bool) -> FindLayout {
        let pad = theme::zpx(6.0);
        let gap = theme::zpx(4.0);
        let row_h = theme::zpx(26.0);
        let btn = theme::zpx(26.0);
        let optw = theme::zpx(22.0);
        let input_w = theme::zpx(196.0);
        let expand_w = theme::zpx(18.0);
        let count_w = theme::zpx(64.0);

        let panel_w = expand_w + pad + input_w + gap + count_w + gap + btn * 3.0 + pad;
        let rows_h = if replace_open { row_h * 2.0 + gap } else { row_h };
        let panel_h = pad + rows_h + pad;
        let px = editor.x + editor.w - theme::zpx(16.0) - panel_w;
        let py = editor.y + theme::zpx(6.0);
        let panel = Rect { x: px, y: py, w: panel_w, h: panel_h };

        let r1y = py + pad;
        let expand = Rect { x: px + theme::zpx(1.0), y: py, w: expand_w, h: panel_h };
        let fi_x = px + expand_w + pad;
        let find_input = Rect { x: fi_x, y: r1y, w: input_w, h: row_h };
        // Three option toggles live inside the find box, right-aligned (VSCode style).
        let regex = Rect { x: find_input.x + find_input.w - optw - theme::zpx(2.0), y: r1y, w: optw, h: row_h };
        let word = Rect { x: regex.x - optw, ..regex };
        let case = Rect { x: word.x - optw, ..regex };
        let find_text = Rect { w: (case.x - find_input.x - theme::zpx(2.0)).max(theme::zpx(20.0)), ..find_input };
        let count = Rect { x: find_input.x + find_input.w + gap, y: r1y, w: count_w, h: row_h };
        let prev = Rect { x: count.x + count.w + gap, y: r1y, w: btn, h: row_h };
        let next = Rect { x: prev.x + btn, ..prev };
        let close = Rect { x: next.x + btn, ..prev };

        let (replace_input, replace_btn, replace_all) = if replace_open {
            let ry = r1y + row_h + gap;
            let ri = Rect { x: fi_x, y: ry, w: input_w, h: row_h };
            let rb = Rect { x: count.x + gap, y: ry, w: btn, h: row_h };
            let ra = Rect { x: rb.x + btn, y: ry, w: btn, h: row_h };
            (Some(ri), Some(rb), Some(ra))
        } else {
            (None, None, None)
        };
        FindLayout { panel, expand, find_input, find_text, case, word, regex, count, prev, next, close, replace_input, replace_btn, replace_all }
    }

    /// Chrome + input boxes + toggle/hover highlights + carets/selection.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_quads(
        &self,
        l: &FindLayout,
        focused: bool,
        on_replace: bool,
        opts: [bool; 3],
        blink: bool,
        bg: &mut Vec<Quad>,
        fg: &mut Vec<Quad>,
    ) {
        // Panel background + border (a floating card over the editor).
        bg.push(Rect { x: l.panel.x - 1.0, y: l.panel.y - 1.0, w: l.panel.w + 2.0, h: l.panel.h + 2.0 }.rounded_quad(theme::PALETTE_BORDER(), 4.0));
        bg.push(l.panel.rounded_quad(theme::PALETTE_BG(), 4.0));

        let mut input_box = |r: Rect, active: bool, bg: &mut Vec<Quad>| {
            let border = if active { [0.28, 0.55, 0.86, 1.0] } else { theme::SEARCH_BORDER() };
            bg.push(Rect { x: r.x - 1.0, y: r.y - 1.0, w: r.w + 2.0, h: r.h + 2.0 }.rounded_quad(border, 3.0));
            bg.push(r.rounded_quad(theme::SEARCH_BG(), 2.0));
        };
        input_box(l.find_input, focused && !on_replace, bg);
        if let Some(ri) = l.replace_input {
            input_box(ri, focused && on_replace, bg);
        }

        // Active option-toggle backgrounds.
        for (on, r) in [(opts[0], l.case), (opts[1], l.word), (opts[2], l.regex)] {
            if on {
                bg.push(r.rounded_quad(theme::DIALOG_BTN_HOVER(), 3.0));
            }
        }
        // Hover highlight on whichever button the pointer is over.
        if let Some(h) = self.hover {
            if let Some(r) = self.btn_rect(l, h) {
                bg.push(r.rounded_quad(theme::MENU_HOVER(), 3.0));
            }
        }
        // Carets + selection for the focused input.
        let (ir, inp) = if on_replace {
            (l.replace_input, &self.replace)
        } else {
            (Some(l.find_text), &self.query)
        };
        if let Some(r) = ir {
            inp.selection_quads(r, theme::zpx(6.0), bg);
            if focused && blink {
                fg.push(inp.caret_quad(r, theme::zpx(6.0)));
            }
        }
    }

    pub fn draw_text<'b>(&'b self, l: &FindLayout, replace_open: bool, opts: [bool; 3], areas: &mut Vec<TextArea<'b>>) {
        // Expand/collapse chevron.
        let exp = if replace_open { &self.ic_expand_open } else { &self.ic_expand_closed };
        exp.draw_center(l.expand, theme::FG_DIM(), areas);
        // Find input text.
        let qc = if self.query.text().is_empty() { theme::FG_DIM() } else { theme::FG_TEXT() };
        self.query.draw(l.find_text, theme::zpx(6.0), qc, areas);
        // Option toggles (bright when on).
        let on_col = theme::FG_ACTIVE();
        let off_col = theme::FG_DIM();
        self.ic_case.draw_center(l.case, if opts[0] { on_col } else { off_col }, areas);
        self.ic_word.draw_center(l.word, if opts[1] { on_col } else { off_col }, areas);
        self.ic_regex.draw_center(l.regex, if opts[2] { on_col } else { off_col }, areas);
        // Match count (centered).
        self.count.draw_center(l.count, theme::FG_DIM(), areas);
        // Nav + close.
        self.ic_prev.draw_center(l.prev, theme::FG_TEXT(), areas);
        self.ic_next.draw_center(l.next, theme::FG_TEXT(), areas);
        self.ic_close.draw_center(l.close, theme::FG_TEXT(), areas);
        // Replace row.
        if let (Some(ri), Some(rb), Some(ra)) = (l.replace_input, l.replace_btn, l.replace_all) {
            let rc = if self.replace.text().is_empty() { theme::FG_DIM() } else { theme::FG_TEXT() };
            self.replace.draw(ri, theme::zpx(6.0), rc, areas);
            self.ic_replace.draw_center(rb, theme::FG_TEXT(), areas);
            self.ic_replace_all.draw_center(ra, theme::FG_TEXT(), areas);
        }
    }

    fn btn_rect(&self, l: &FindLayout, b: FindBtn) -> Option<Rect> {
        Some(match b {
            FindBtn::Expand => l.expand,
            FindBtn::Case => l.case,
            FindBtn::Word => l.word,
            FindBtn::Regex => l.regex,
            FindBtn::Prev => l.prev,
            FindBtn::Next => l.next,
            FindBtn::Close => l.close,
            FindBtn::Replace => l.replace_btn?,
            FindBtn::ReplaceAll => l.replace_all?,
        })
    }

    /// Which button (if any) is under `p`.
    pub fn button_at(&self, l: &FindLayout, p: (f32, f32)) -> Option<FindBtn> {
        for b in [
            FindBtn::Expand,
            FindBtn::Case,
            FindBtn::Word,
            FindBtn::Regex,
            FindBtn::Prev,
            FindBtn::Next,
            FindBtn::Close,
            FindBtn::Replace,
            FindBtn::ReplaceAll,
        ] {
            if let Some(r) = self.btn_rect(l, b) {
                if r.contains(p) {
                    return Some(b);
                }
            }
        }
        None
    }

    /// Cursor for the point: pointer over buttons, text over the inputs.
    pub fn cursor(&self, l: &FindLayout, p: (f32, f32)) -> Option<CursorIcon> {
        if self.button_at(l, p).is_some() {
            return Some(CursorIcon::Pointer);
        }
        if l.find_text.contains(p) || l.replace_input.map_or(false, |r| r.contains(p)) {
            return Some(CursorIcon::Text);
        }
        if l.panel.contains(p) {
            return Some(CursorIcon::Default);
        }
        None
    }
}
