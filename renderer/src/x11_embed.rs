// X11 window embedding — docks webview-host windows (extension webviews) into the
// editor window via XReparentWindow. Only possible on the X11 backend (native X11 or
// XWayland); on pure Wayland the webview stays a separate floating window.

#![cfg(target_os = "linux")]

use x11rb::connection::Connection;
use x11rb::protocol::xproto::ConnectionExt;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

pub struct Embedder {
    conn: RustConnection,
    parent: u32,
}

impl Embedder {
    /// Connect to the X server for embedding into `parent` (the editor's X window).
    pub fn new(parent: u32) -> Option<Embedder> {
        let (conn, _) = x11rb::connect(None).ok()?;
        Some(Embedder { conn, parent })
    }

    /// Tie `child` to the editor window as a transient: the WM keeps it stacked
    /// above the editor (and only the editor). True XReparent doesn't work here —
    /// the editor's Vulkan swapchain presents over child windows under XWayland —
    /// so the child stays a toplevel and the editor position-syncs it instead.
    pub fn set_transient_for(&self, child: u32) {
        let _ = self.conn.change_property32(
            x11rb::protocol::xproto::PropMode::REPLACE,
            child,
            x11rb::protocol::xproto::AtomEnum::WM_TRANSIENT_FOR,
            x11rb::protocol::xproto::AtomEnum::WINDOW,
            &[self.parent],
        );
        let _ = self.conn.flush();
    }

    /// Hand keyboard focus to the embedded child (click-to-focus).
    pub fn focus(&self, child: u32) {
        let _ = self.conn.set_input_focus(
            x11rb::protocol::xproto::InputFocus::PARENT,
            child,
            x11rb::CURRENT_TIME,
        );
        let _ = self.conn.flush();
    }

    /// Return keyboard focus to the editor window.
    pub fn focus_parent(&self) {
        let _ = self.conn.set_input_focus(
            x11rb::protocol::xproto::InputFocus::PARENT,
            self.parent,
            x11rb::CURRENT_TIME,
        );
        let _ = self.conn.flush();
    }
}

/// The editor window's X11 window id, if running on the X11 backend.
pub fn window_xid(window: &winit::window::Window) -> Option<u32> {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Xlib(h) => Some(h.window as u32),
        RawWindowHandle::Xcb(h) => Some(h.window.get()),
        _ => None,
    }
}
