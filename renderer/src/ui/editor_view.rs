// Editor view interaction: mouse hit-testing → caret placement, multi-click
// word/line/document selection, drag-select, and keeping the caret in view. The
// heavy editing lives on `Document` (in `document.rs`); this owns only the
// view-interaction state and translates pointer input into `Document` calls.

use crate::document::Document;
use crate::layout::Layout;
use crate::theme;

#[derive(Default)]
pub struct EditorView {
    /// A mouse drag-select is in progress.
    pub dragging: bool,
    /// Consecutive-click count: 1 = place, 2 = word, 3 = line, 4 = document (cycles).
    pub click_count: u32,
}

impl EditorView {
    pub fn new() -> Self {
        Self::default()
    }

    /// Hit-test `(x, y)` against the document's shaped buffer and move the caret
    /// there, extending the selection when `extend`.
    fn place_caret(doc: &mut Document, layout: &Layout, x: f32, y: f32, extend: bool) {
        let buf_x = x - (layout.editor_text.x + theme::EDITOR_PAD) + doc.scroll_x();
        let buf_y = y - (layout.editor_text.y + theme::EDITOR_PAD) + doc.scroll_y();
        if let Some(hit) = doc.buffer.hit(buf_x, buf_y) {
            let line = hit.line;
            if line < doc.rope.len_lines() {
                let line_start = doc.rope.line_to_byte(line);
                let line_len = doc.rope.line(line).len_bytes();
                let col = hit.index.min(line_len);
                doc.place(line_start + col, extend);
            }
        }
    }

    /// Editor mouse-press: place the caret, then word/line/document-select on
    /// consecutive clicks (cycling). `consecutive` = within the double-click window.
    pub fn on_press(&mut self, doc: &mut Document, layout: &Layout, x: f32, y: f32, extend: bool, consecutive: bool) {
        self.click_count = if consecutive { (self.click_count % 4) + 1 } else { 1 };
        Self::place_caret(doc, layout, x, y, extend);
        if self.click_count >= 2 {
            let b = doc.sel.head;
            match self.click_count {
                2 => doc.select_word(b),
                3 => doc.select_line(b),
                _ => doc.select_all(),
            }
            self.dragging = false;
        } else {
            self.dragging = true;
        }
    }

    /// Drag-extend the selection while the mouse is held. Returns true if a drag
    /// was active (and thus the caret moved).
    pub fn on_drag(&mut self, doc: &mut Document, layout: &Layout, x: f32, y: f32) -> bool {
        if !self.dragging {
            return false;
        }
        Self::place_caret(doc, layout, x, y, true);
        true
    }

    pub fn on_release(&mut self) {
        self.dragging = false;
    }

    /// Scroll the document so the caret's line stays within the editor viewport.
    pub fn ensure_cursor_visible(doc: &mut Document, layout: &Layout) {
        let editor_inner_h = layout.editor_text.h - theme::EDITOR_PAD * 2.0;
        if editor_inner_h <= 0.0 {
            return;
        }
        let (line, _) = doc.head_line_col();
        let cursor_top = line as f32 * theme::LINE_HEIGHT();
        let cursor_bottom = cursor_top + theme::LINE_HEIGHT();
        let scroll_y = doc.scroll_y();
        if cursor_top < scroll_y {
            doc.scroll.scroll_to_y(cursor_top.max(0.0));
        } else if cursor_bottom > scroll_y + editor_inner_h {
            doc.scroll.scroll_to_y(cursor_bottom - editor_inner_h);
        }
    }
}
