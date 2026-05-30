// Shared seams for self-contained UI panels.
//
// A "panel" is a struct that owns one feature's state + glyphon buffers, and knows
// how to shape itself (`update`), draw itself (`draw`/`draw_pass`), and handle its
// own input. Panels live directly on `App` and are driven by thin orchestrators in
// `render.rs` (drawing) and `main.rs` (input). Cross-cutting side-effects a panel
// can't perform itself (opening a file, toggling another panel) are returned as
// `Intent`s and applied centrally by `App::apply_intent`.
//
// This module only defines the shared types; each panel lives in its own file.

use std::path::PathBuf;
use std::time::Instant;

use glyphon::TextArea;

pub mod editor_view;
pub mod explorer_panel;
pub mod ext_detail_view;
pub mod extensions_panel;
pub mod search_panel;
pub mod terminal_panel;

use crate::extensions::OpenExt;
use crate::layout::Layout;
use crate::quad::Quad;

/// The main-pass draw lists a panel pushes into. `bg` draws under text, `fg` over it
/// (cursors, scrollbar thumbs). `areas` borrow the panel's own buffers, so a panel's
/// `draw` takes `&'buf self` and its areas live as long as those buffers.
pub struct Paint<'a, 'buf> {
    pub bg: &'a mut Vec<Quad>,
    pub fg: &'a mut Vec<Quad>,
    pub areas: &'a mut Vec<TextArea<'buf>>,
    pub now: Instant,
}

/// Read-only context handed to panels for layout + frame info. Theme is global
/// (accessor fns), so it isn't threaded here.
pub struct Ctx<'a> {
    pub layout: &'a Layout,
}

/// A side-effect a panel requests of `App` — kept as data so cross-cutting actions
/// (which touch shared state like the workspace) stay centralized in one place.
pub enum Intent {
    /// Open `path` and place the caret at (1-based `line`, byte `col`).
    OpenFile { path: PathBuf, line: usize, col: usize },
    /// Open an extension's detail page.
    OpenExtDetail(OpenExt),
    /// Open a settings JSON file in the editor.
    OpenSettings(PathBuf),
    /// Reload every open document from disk (after a Replace All rewrote files).
    ReloadOpenDocs,
    /// Request another full redraw next frame.
    Redraw,
}
