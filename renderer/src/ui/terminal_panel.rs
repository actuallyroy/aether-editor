// Integrated terminal panel state. Each group is a tab (`+` adds one); within a
// tab, panes are shown side-by-side (split). Only the active group is visible; the
// rest keep running in the background. No shell is ever discarded.
//
// NOTE (refactor staging): this groups the terminal's *state* in one place
// (`self.terminal.*`). The pane/tab/split logic still lives on `App` and the pane
// glyph buffers (`gpu.ui.terminal_panes`/`term_tablist`) + the draw still live in
// `gpu`/`render.rs`, since they need direct `gpu` access. Moving that in is a
// follow-up.

use std::path::PathBuf;

use crate::layout::Layout;
use crate::terminal;
use crate::theme;
use crate::widgets::{Axis, Rect, Splitter};
use crate::{
    terminal_content, terminal_grid_size, terminal_header_button_rects, terminal_pane_area,
    terminal_pane_rects, terminal_tab_close_rect, terminal_tablist_rect,
};

pub struct TerminalPanel {
    pub groups: Vec<terminal::Group>,
    pub active: usize,         // active tab (group) index
    pub visible: bool,
    pub focused: bool,
    pub split: Splitter,       // draggable panel height
    pub maximized: bool,       // header maximize toggle (fills the content area)
    /// Workspace root new shells start in (like VSCode). The panel owns this so
    /// spawning doesn't have to thread it through every call; `App` keeps it in
    /// sync via `set_cwd` whenever the workspace root changes.
    cwd: PathBuf,
}

impl TerminalPanel {
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            groups: Vec::new(),
            active: 0,
            visible: false,
            focused: false,
            split: Splitter::new(
                theme::TERMINAL_HEIGHT,
                theme::TERMINAL_MIN_HEIGHT,
                theme::TERMINAL_MAX_HEIGHT,
                Axis::Vertical,
            ),
            maximized: false,
            cwd,
        }
    }

    /// Update the directory new shells will start in (called on Open Folder).
    pub fn set_cwd(&mut self, cwd: PathBuf) {
        self.cwd = cwd;
    }

    /// Requested panel height: huge when maximized (the layout clamps it to leave a
    /// sliver of editor), the splitter size otherwise, None when hidden.
    pub fn panel_height(&self) -> Option<f32> {
        if !self.visible {
            return None;
        }
        Some(if self.maximized { 100_000.0 } else { self.split.size() })
    }

    /// Number of split panes in the active tab (0 when there's no terminal).
    pub fn active_pane_count(&self) -> usize {
        self.groups.get(self.active).map_or(0, |g| g.panes.len())
    }

    /// Spawn a pane sized to fit when the active tab shows `count` side-by-side
    /// panes, with the shell starting in `cwd` (the workspace root).
    fn spawn_pane(&self, count: usize, panel: Option<Rect>, cell_w: f32) -> Option<terminal::Pane> {
        let panel = panel?;
        let area = terminal_pane_area(terminal_content(panel), self.groups.len().max(1));
        let rect = terminal_pane_rects(area, count.max(1))
            .into_iter()
            .next()
            .unwrap_or(area);
        let (rows, cols) = terminal_grid_size(rect, cell_w);
        terminal::Pane::spawn(rows, cols, &self.cwd)
    }

    /// Header `+`: open a new terminal tab (a fresh group). The previous tab keeps
    /// running in the background and stays reachable from the tab list.
    pub fn new_terminal_tab(&mut self, panel: Option<Rect>, cell_w: f32) {
        if let Some(p) = self.spawn_pane(1, panel, cell_w) {
            self.groups.push(terminal::Group::new(p));
            self.active = self.groups.len() - 1;
            self.focused = true;
            self.mark_dirty(); // tab list appearing reflows pane widths
        }
    }

    /// Header split: add a side-by-side pane to the active tab.
    pub fn split_terminal(&mut self, panel: Option<Rect>, cell_w: f32) {
        let count = self.active_pane_count() + 1;
        if let Some(p) = self.spawn_pane(count, panel, cell_w) {
            if let Some(g) = self.groups.get_mut(self.active) {
                g.panes.push(p);
                g.focused = g.panes.len() - 1;
                self.focused = true;
            }
            self.mark_dirty();
        }
    }

    /// Header trash: kill the focused pane; drop the tab if it was its last pane;
    /// hide the panel if that was the last tab.
    pub fn kill_terminal(&mut self) {
        let Some(g) = self.groups.get_mut(self.active) else {
            return;
        };
        if g.panes.is_empty() {
            return;
        }
        let i = g.focused.min(g.panes.len() - 1);
        g.panes.remove(i);
        if g.panes.is_empty() {
            self.groups.remove(self.active);
            if self.groups.is_empty() {
                self.visible = false;
                self.focused = false;
                self.maximized = false;
            } else {
                self.active = self.active.min(self.groups.len() - 1);
            }
        } else {
            g.focused = i.min(g.panes.len() - 1);
        }
        self.mark_dirty();
    }

    /// Switch the visible terminal tab.
    pub fn switch_tab(&mut self, i: usize) {
        if i < self.groups.len() {
            self.active = i;
            self.focused = true;
            self.mark_dirty();
        }
    }

    /// Tab-list × button: kill an entire tab (all its panes); hide the panel if it
    /// was the last tab.
    pub fn kill_tab(&mut self, i: usize) {
        if i >= self.groups.len() {
            return;
        }
        self.groups.remove(i);
        if self.groups.is_empty() {
            self.visible = false;
            self.focused = false;
            self.maximized = false;
        } else {
            self.active = self.active.min(self.groups.len() - 1);
        }
        self.mark_dirty();
    }

    /// Header maximize: grow the panel to fill the whole content area (toggle).
    pub fn toggle_max(&mut self) {
        self.maximized = !self.maximized;
        self.mark_dirty();
    }

    /// Mark every pane in every tab as needing a reshape (after a layout change).
    pub fn mark_dirty(&mut self) {
        for g in &mut self.groups {
            for p in &mut g.panes {
                p.dirty = true;
            }
        }
    }

    /// Show/hide the integrated terminal. Returns true if a first tab must be
    /// spawned (caller computes the panel rect *after* this flips `visible`, since
    /// the panel only has a height once visible — then calls `spawn_initial`).
    pub fn toggle(&mut self) -> bool {
        self.visible = !self.visible;
        self.focused = self.visible;
        self.visible && self.groups.is_empty()
    }

    /// Spawn the first tab on first open, using the now-visible panel rect.
    pub fn spawn_initial(&mut self, panel: Option<Rect>, cell_w: f32) {
        if let Some(p) = self.spawn_pane(1, panel, cell_w) {
            self.groups.push(terminal::Group::new(p));
            self.active = 0;
        }
    }

    // ---- Input (the panel owns its region's press/scroll/drag/hover) ----

    /// A pane scrollbar thumb/track press (overlay, claimed before region handlers).
    pub fn pane_scroll_press(&mut self, pt: (f32, f32)) -> bool {
        if !self.visible {
            return false;
        }
        if let Some(g) = self.groups.get_mut(self.active) {
            for i in 0..g.panes.len() {
                if g.panes[i].scroll.press(pt) {
                    g.panes[i].dirty = true;
                    g.focused = i;
                    return true;
                }
            }
        }
        false
    }

    /// Press in the terminal content/header: tab list (× kills / row switches), pane
    /// focus, or a header icon-button action. Returns true if consumed. Clicking
    /// outside the panel while visible just drops focus (not consumed).
    pub fn content_press(&mut self, pt: (f32, f32), layout: &Layout, cell_w: f32) -> bool {
        if !self.visible {
            return false;
        }
        let Some(panel) = layout.terminal_panel else { return false };
        let content = terminal_content(panel);
        if content.contains(pt) {
            // The right-side tab list: × kills that tab, the row body switches.
            if let Some(tl) = terminal_tablist_rect(content, self.groups.len()) {
                if tl.contains(pt) {
                    let idx = ((pt.1 - tl.y) / theme::TREE_ROW_HEIGHT) as usize;
                    if idx < self.groups.len() {
                        if terminal_tab_close_rect(tl, idx).contains(pt) {
                            self.kill_tab(idx);
                        } else {
                            self.switch_tab(idx);
                        }
                    }
                    return true;
                }
            }
            // Otherwise focus whichever split pane was clicked.
            let area = terminal_pane_area(content, self.groups.len());
            let rects = terminal_pane_rects(area, self.active_pane_count());
            if let Some(i) = rects.iter().position(|r| r.contains(pt)) {
                if let Some(g) = self.groups.get_mut(self.active) {
                    g.focused = i;
                }
            }
            self.focused = true;
            return true;
        }
        // Header strip (above content): right-side icon buttons.
        if panel.contains(pt) {
            let btns = terminal_header_button_rects(panel);
            if let Some(i) = btns.iter().position(|r| r.contains(pt)) {
                match i {
                    0 => self.new_terminal_tab(Some(panel), cell_w), // + new tab
                    1 => self.split_terminal(Some(panel), cell_w),   // ⊟ split active tab
                    2 => self.kill_terminal(),                       // 🗑 kill focused pane
                    4 => self.toggle_max(),                          // ⌃ maximize/restore
                    5 => {
                        self.toggle(); // × hide panel (groups exist, so no spawn)
                    }
                    _ => {} // 3 more — menu infra TBD
                }
            }
            return true;
        }
        self.focused = false; // clicked elsewhere while visible
        false
    }

    /// Mouse wheel over a terminal pane → scroll its scrollback. Returns true if
    /// consumed (cursor was over the terminal content).
    pub fn on_scroll(&mut self, pt: (f32, f32), layout: &Layout, dy: f32) -> bool {
        if !self.visible {
            return false;
        }
        let Some(panel) = layout.terminal_panel else { return false };
        let content = terminal_content(panel);
        if !content.contains(pt) {
            return false;
        }
        let area = terminal_pane_area(content, self.groups.len());
        let rects = terminal_pane_rects(area, self.active_pane_count());
        if let Some(i) = rects.iter().position(|r| r.contains(pt)) {
            if let Some(g) = self.groups.get_mut(self.active) {
                if g.panes[i].scroll.on_wheel(0.0, dy) {
                    g.panes[i].dirty = true;
                }
            }
        }
        true
    }

    /// Continue a pane scrollbar drag. Returns true if a drag was active.
    pub fn pane_scroll_drag(&mut self, pt: (f32, f32)) -> bool {
        if let Some(g) = self.groups.get_mut(self.active) {
            if let Some(p) = g.panes.iter_mut().find(|p| p.scroll.is_dragging()) {
                if p.scroll.drag(pt) {
                    p.dirty = true;
                }
                return true;
            }
        }
        false
    }

    /// Release any in-progress pane scrollbar drags.
    pub fn release_scrolls(&mut self) {
        for g in &mut self.groups {
            for p in &mut g.panes {
                p.scroll.release();
            }
        }
    }

    /// Drive each visible pane's scrollbar hover (auto-hide fade) and report whether
    /// the pointer is over a thumb. Returns (redraw_needed, over_scroll_thumb).
    pub fn hover_panes(&mut self, p: (f32, f32), layout: &Layout) -> (bool, bool) {
        let mut changed = false;
        let mut over_thumb = false;
        if self.visible {
            if let Some(panel) = layout.terminal_panel {
                let area = terminal_pane_area(terminal_content(panel), self.groups.len());
                let rects = terminal_pane_rects(area, self.active_pane_count());
                if let Some(g) = self.groups.get_mut(self.active) {
                    for (i, pane) in g.panes.iter_mut().enumerate() {
                        let inside = rects.get(i).map_or(false, |r| r.contains(p));
                        if pane.scroll.hover(inside) {
                            changed = true;
                        }
                        if inside && pane.scroll.cursor(p).is_some() {
                            over_thumb = true;
                        }
                    }
                }
            }
        }
        (changed, over_thumb)
    }
}
