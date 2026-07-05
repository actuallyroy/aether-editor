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

    /// Redirect `child`'s rendering off-screen (XComposite) so its pixels stay
    /// capturable even while the window is parked outside the viewport.
    pub fn redirect(&self, child: u32) {
        use x11rb::protocol::composite::ConnectionExt as _;
        // AUTOMATIC — the compositing WM (mutter) already owns MANUAL redirection,
        // so we only ensure a name pixmap exists to capture from.
        let _ = self
            .conn
            .composite_redirect_window(child, x11rb::protocol::composite::Redirect::AUTOMATIC);
        // Hide the window from the screen WITHOUT stopping WebKit's painting:
        // fully transparent (the compositor drops its pixels) and an empty INPUT
        // shape (clicks pass through). The name pixmap still gets every frame.
        let opacity_atom = self
            .conn
            .intern_atom(false, b"_NET_WM_WINDOW_OPACITY")
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| r.atom);
        if let Some(atom) = opacity_atom {
            let _ = self.conn.change_property32(
                x11rb::protocol::xproto::PropMode::REPLACE,
                child,
                atom,
                x11rb::protocol::xproto::AtomEnum::CARDINAL,
                &[0u32], // 0 = fully transparent
            );
        }
        {
            use x11rb::protocol::shape::{self, ConnectionExt as _};
            let _ = self.conn.shape_rectangles(
                shape::SO::SET,
                shape::SK::INPUT,
                x11rb::protocol::xproto::ClipOrdering::UNSORTED,
                child,
                0,
                0,
                &[], // empty input region → click-through
            );
        }
        let _ = self.conn.flush();
    }

    /// Grab the child window's current pixels as RGBA (BGRX → RGBA swizzle).
    /// Reads the COMPOSITE name pixmap — GetImage on an off-screen window itself
    /// is a BadMatch; the redirected pixmap always holds the full rendering.
    pub fn capture(&self, child: u32) -> Option<(u32, u32, Vec<u8>)> {
        use x11rb::protocol::composite::ConnectionExt as _;
        let geo = self.conn.get_geometry(child).ok()?.reply().ok()?;
        let pixmap = self.conn.generate_id().ok()?;
        self.conn.composite_name_window_pixmap(child, pixmap).ok()?.check().ok()?;
        let img = self
            .conn
            .get_image(
                x11rb::protocol::xproto::ImageFormat::Z_PIXMAP,
                pixmap,
                0,
                0,
                geo.width,
                geo.height,
                !0,
            )
            .ok()
            .and_then(|c| c.reply().ok());
        let _ = self.conn.free_pixmap(pixmap);
        let _ = self.conn.flush();
        let img = img?;
        let mut rgba = img.data;
        if rgba.len() < (geo.width as usize) * (geo.height as usize) * 4 {
            return None;
        }
        for px in rgba.chunks_exact_mut(4) {
            px.swap(0, 2);
            px[3] = 255;
        }
        Some((geo.width as u32, geo.height as u32, rgba))
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
