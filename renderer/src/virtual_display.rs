// Virtual-display webview backend. Each embedded webview renders into its own Xvfb
// server — a real display that exists only in memory, so WebKit runs completely
// normally (accelerated, un-throttled, believing it is visible) while nothing ever
// reaches the user's screen. The editor:
//   - captures frames with GetImage straight off the virtual display (~30fps)
//   - injects input with XTEST, giving NATIVE semantics (drag, select, hover, wheel)
//
// This is the standard headless-browser architecture (xvfb-run & friends).

#![cfg(target_os = "linux")]

use x11rb::connection::Connection;
use x11rb::protocol::xproto::ConnectionExt;
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

pub struct VirtualDisplay {
    conn: RustConnection,
    root: u32,
    /// keysym -> (keycode, needs_shift)
    keymap: std::collections::HashMap<u32, (u8, bool)>,
    shift_keycode: u8,
    ctrl_keycode: u8,
}

/// Pick a free X display number by probing /tmp/.X11-unix sockets.
pub fn free_display() -> u32 {
    for n in 90..190 {
        if !std::path::Path::new(&format!("/tmp/.X11-unix/X{n}")).exists() {
            return n;
        }
    }
    90
}

/// Spawn an Xvfb server for `display` (e.g. 99 → ":99"). The caller owns the child.
pub fn spawn_xvfb(display: u32) -> Option<std::process::Child> {
    std::process::Command::new("Xvfb")
        .arg(format!(":{display}"))
        .args(["-screen", "0", "2560x1600x24", "-nolisten", "tcp", "-noreset"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()
}

impl VirtualDisplay {
    /// Connect to the virtual display (retries while Xvfb boots).
    pub fn connect(display: u32) -> Option<VirtualDisplay> {
        let name = format!(":{display}");
        let mut conn = None;
        for _ in 0..50 {
            match x11rb::connect(Some(&name)) {
                Ok((c, screen)) => {
                    let root = c.setup().roots[screen].root;
                    conn = Some((c, root));
                    break;
                }
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(50)),
            }
        }
        let (conn, root) = conn?;
        // Build keysym → keycode(+shift) from the server's keyboard mapping.
        let setup = conn.setup();
        let (min_kc, max_kc) = (setup.min_keycode, setup.max_keycode);
        let mapping = conn
            .get_keyboard_mapping(min_kc, max_kc - min_kc + 1)
            .ok()?
            .reply()
            .ok()?;
        let per = mapping.keysyms_per_keycode as usize;
        let mut keymap = std::collections::HashMap::new();
        let mut shift_keycode = 50u8; // sane defaults
        let mut ctrl_keycode = 37u8;
        for (i, chunk) in mapping.keysyms.chunks(per).enumerate() {
            let keycode = min_kc + i as u8;
            for (col, &ks) in chunk.iter().enumerate().take(2) {
                if ks == 0 {
                    continue;
                }
                keymap.entry(ks).or_insert((keycode, col == 1));
                if ks == 0xffe1 {
                    shift_keycode = keycode; // XK_Shift_L
                }
                if ks == 0xffe3 {
                    ctrl_keycode = keycode; // XK_Control_L
                }
            }
        }
        Some(VirtualDisplay { conn, root, keymap, shift_keycode, ctrl_keycode })
    }

    /// Grab a window's pixels as RGBA. The window is on-screen on THIS display,
    /// so a plain GetImage works (BGRX → RGBA swizzle).
    pub fn capture(&self, window: u32) -> Option<(u32, u32, Vec<u8>)> {
        let geo = self.conn.get_geometry(window).ok()?.reply().ok()?;
        let img = self
            .conn
            .get_image(
                x11rb::protocol::xproto::ImageFormat::Z_PIXMAP,
                window,
                0,
                0,
                geo.width,
                geo.height,
                !0,
            )
            .ok()?
            .reply()
            .ok()?;
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

    // ---- XTEST input: indistinguishable from real user input on that display ----

    fn fake(&self, kind: u8, detail: u8, x: i16, y: i16) {
        let _ = self.conn.xtest_fake_input(kind, detail, 0, self.root, x, y, 0);
        let _ = self.conn.flush();
    }

    pub fn motion(&self, x: f64, y: f64) {
        self.fake(6 /* MotionNotify */, 0, x as i16, y as i16);
    }

    pub fn button(&self, x: f64, y: f64, button: u8, press: bool) {
        self.motion(x, y);
        self.fake(if press { 4 } else { 5 } /* ButtonPress/Release */, button, 0, 0);
    }

    /// Wheel scroll: X buttons 4/5 (up/down) and 6/7 (left/right), one per notch.
    pub fn scroll(&self, x: f64, y: f64, dx: f64, dy: f64) {
        self.motion(x, y);
        let notches_y = dy.abs().ceil() as u32;
        for _ in 0..notches_y.min(10) {
            let b = if dy < 0.0 { 4 } else { 5 };
            self.fake(4, b, 0, 0);
            self.fake(5, b, 0, 0);
        }
        let notches_x = dx.abs().ceil() as u32;
        for _ in 0..notches_x.min(10) {
            let b = if dx < 0.0 { 6 } else { 7 };
            self.fake(4, b, 0, 0);
            self.fake(5, b, 0, 0);
        }
    }

    /// Type a keysym (with automatic Shift, plus Ctrl when requested).
    pub fn key(&self, keysym: u32, ctrl: bool) {
        let Some(&(keycode, shift)) = self.keymap.get(&keysym) else { return };
        if ctrl {
            self.fake(2 /* KeyPress */, self.ctrl_keycode, 0, 0);
        }
        if shift {
            self.fake(2, self.shift_keycode, 0, 0);
        }
        self.fake(2, keycode, 0, 0);
        self.fake(3 /* KeyRelease */, keycode, 0, 0);
        if shift {
            self.fake(3, self.shift_keycode, 0, 0);
        }
        if ctrl {
            self.fake(3, self.ctrl_keycode, 0, 0);
        }
    }
}
