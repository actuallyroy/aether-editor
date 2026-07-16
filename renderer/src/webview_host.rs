// Webview host process (`aether --webview-host`) — renders extension webviews
// (e.g. the Claude Code panel) with a real browser engine. It's a SEPARATE process
// because wry/webkit2gtk needs a GTK main loop, which can't share a thread with the
// winit loop; the GUI talks to it over stdio (JSON lines), mirroring the pty-host
// pattern.
//
//   stdin  → {"cmd":"init","title":..,"width":..,"height":..,"roots":[..]}
//            {"cmd":"html","html":".."}          — set the document (extension-provided)
//            {"cmd":"post","data":..}            — aether/extension → webview message
//   stdout ← {"event":"ready"}                   — GTK window + webview are up
//            {"event":"message","data":..}       — webview → extension message
//            {"event":"closed"}                  — user closed the window
//
// Inside the page, `acquireVsCodeApi()` is injected so unmodified VSCode webview
// bundles run: postMessage feeds the ipc channel; get/setState is in-page.
// `aether-res://` serves files from the allowed roots (asWebviewUri targets).

#![cfg(target_os = "linux")]

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::mpsc;

use serde_json::{json, Value};

fn out(v: Value) {
    let mut s = v.to_string();
    s.push('\n');
    let _ = std::io::stdout().write_all(s.as_bytes());
    let _ = std::io::stdout().flush();
}

use crate::webview_shim::VSCODE_API_SHIM;

pub fn run() -> anyhow::Result<()> {
    gtk::init()?;

    // ---- read the init command synchronously (first line) ----
    let stdin = std::io::stdin();
    let mut first = String::new();
    stdin.lock().read_line(&mut first)?;
    let init: Value = serde_json::from_str(first.trim()).unwrap_or(Value::Null);
    let title = init.get("title").and_then(|v| v.as_str()).unwrap_or("Aether Webview").to_string();
    let width = init.get("width").and_then(|v| v.as_i64()).unwrap_or(480) as i32;
    let height = init.get("height").and_then(|v| v.as_i64()).unwrap_or(720) as i32;
    let roots: Vec<PathBuf> = init
        .get("roots")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(PathBuf::from)).collect())
        .unwrap_or_default();
    // Embed mode: fully OFFSCREEN — the webview lives in a GtkOffscreenWindow that
    // never appears on any screen; frames are written to `shm` (a /dev/shm file) and
    // announced with `frame` events. The editor draws them as a GPU texture and
    // forwards input back as synthesized GDK events.
    let embed = init.get("embed").and_then(|v| v.as_bool()).unwrap_or(false);
    let shm_path = init.get("shm").and_then(|v| v.as_str()).map(String::from);

    // ---- GTK window ----
    // Embed mode: a plain undecorated window — but our DISPLAY is a private Xvfb
    // server, so it renders normally (accelerated, un-throttled) without ever
    // appearing on the user's screen. The editor captures frames from the virtual
    // display and injects input with XTEST.
    use gtk::prelude::*;
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title(&title);
    window.set_default_size(width, height);
    if embed {
        window.set_decorated(false);
        window.move_(0, 0);
    } else {
        // Floating mode: stay above the editor instead of vanishing behind it
        // whenever the editor takes focus.
        window.set_type_hint(gtk::gdk::WindowTypeHint::Utility);
        window.set_keep_above(true);
    }
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    window.add(&vbox);
    window.connect_delete_event(|_, _| {
        out(json!({"event": "closed"}));
        gtk::main_quit();
        glib::Propagation::Proceed
    });

    // The current document, served at aether-res://page/__page__.html. Navigating to a
    // URL (instead of load_html) keeps webview.uri() parseable — wry's ipc handler
    // panics (InvalidUri) when the page lives on an unparseable base like about:blank.
    let page_html = std::sync::Arc::new(std::sync::Mutex::new(String::from(
        "<html><body style='background:#1e1e1e'></body></html>",
    )));

    // ---- webview ----
    let theme_js = init.get("themeJs").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let webview = {
        let page_html_proto = page_html.clone();
        use wry::WebViewBuilderExtUnix;
        wry::WebViewBuilder::new()
            .with_initialization_script(&theme_js)
            .with_initialization_script(VSCODE_API_SHIM)
            .with_ipc_handler(move |req| {
                // acquireVsCodeApi().postMessage(data) lands here (JSON string body).
                let body = req.body().as_str();
                let data: Value = serde_json::from_str(body).unwrap_or(Value::String(body.to_string()));
                if data.get("__aetherFocus").is_some() {
                    out(json!({"event": "focus"}));
                    return;
                }
                if let Some(c) = data.get("__aetherCursor").and_then(|c| c.as_str()) {
                    out(json!({"event": "cursor", "value": c}));
                    return;
                }
                if let Some(kind) = data.get("__aetherConsole").and_then(|k| k.as_str()) {
                    // Page console/error forwarding — a debug event, not an extension message.
                    let msg = data.get("msg").and_then(|m| m.as_str()).unwrap_or("");
                    out(json!({"event": "console", "kind": kind, "msg": msg}));
                    return;
                }
                out(json!({"event": "message", "data": data}));
            })
            .with_custom_protocol("aether-res".into(), move |_id, request| {
                // aether-res://page/__page__.html — the extension-provided document.
                if request.uri().host() == Some("page") {
                    let body = page_html_proto.lock().unwrap().clone().into_bytes();
                    return wry::http::Response::builder()
                        .header("Content-Type", "text/html")
                        .body(std::borrow::Cow::Owned(body))
                        .unwrap();
                }
                // aether-res://file/<abs path> — only files under an allowed root.
                let path = PathBuf::from(request.uri().path());
                let allowed = roots.iter().any(|r| path.starts_with(r));
                let body = if allowed { std::fs::read(&path).ok() } else { None };
                let mime = match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
                    "js" | "mjs" => "text/javascript",
                    "css" => "text/css",
                    "html" => "text/html",
                    "svg" => "image/svg+xml",
                    "png" => "image/png",
                    "woff" | "woff2" => "font/woff2",
                    "json" => "application/json",
                    _ => "application/octet-stream",
                };
                match body {
                    Some(b) => wry::http::Response::builder()
                        .header("Content-Type", mime)
                        .header("Access-Control-Allow-Origin", "*")
                        .body(std::borrow::Cow::Owned(b))
                        .unwrap(),
                    None => wry::http::Response::builder()
                        .status(404)
                        .body(std::borrow::Cow::Owned(Vec::new()))
                        .unwrap(),
                }
            })
            .with_url("aether-res://page/__page__.html")
            .build_gtk(&vbox)?
    };

    window.show_all();
    // Report our X window id — the editor captures/targets it on the virtual display.
    let xid: u64 = {
        extern "C" {
            fn gdk_x11_window_get_xid(window: *mut std::ffi::c_void) -> u64;
        }
        use gtk::glib::translate::ToGlibPtr;
        match window.window() {
            Some(gdk_win)
                if gtk::gdk::Display::default()
                    .map_or(false, |d| d.type_().name().contains("X11")) =>
            {
                let ptr: *mut gtk::gdk::ffi::GdkWindow = gdk_win.to_glib_none().0;
                unsafe { gdk_x11_window_get_xid(ptr as *mut _) }
            }
            _ => 0,
        }
    };
    out(json!({"event": "ready", "xid": xid}));

    // ---- stdin command pump: a reader thread feeds the GTK main loop ----
    let (tx, rx) = mpsc::channel::<Value>();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                if tx.send(v).is_err() {
                    break;
                }
            }
        }
        // stdin closed → the GUI went away; exit with it.
        let _ = tx.send(json!({"cmd": "quit"}));
    });

    glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
        while let Ok(cmd) = rx.try_recv() {
            match cmd.get("cmd").and_then(|c| c.as_str()) {
                Some("html") => {
                    if let Some(html) = cmd.get("html").and_then(|h| h.as_str()) {
                        *page_html.lock().unwrap() = html.to_string();
                        let _ = webview.load_url("aether-res://page/__page__.html");
                    }
                }
                Some("post") => {
                    // Deliver as a `message` event, exactly how VSCode webviews get it.
                    // U+2028/U+2029 are valid JSON but break inline JS — escape them.
                    let data = cmd.get("data").cloned().unwrap_or(Value::Null);
                    let payload = serde_json::to_string(&data)
                        .unwrap_or_else(|_| "null".into())
                        .replace('\u{2028}', "\\u2028")
                        .replace('\u{2029}', "\\u2029");
                    let js = format!(
                        "window.dispatchEvent(new MessageEvent('message', {{ data: {payload} }}));"
                    );
                    let _ = webview.evaluate_script(&js);
                }
                Some("bounds") => {
                    let g = |k: &str| cmd.get(k).and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                    // Match the editor's UI zoom (pane rect is in physical pixels).
                    if let Some(z) = cmd.get("zoom").and_then(|v| v.as_f64()) {
                        use wry::WebViewExtUnix;
                        use webkit2gtk::WebViewExt;
                        webview.webview().set_zoom_level(z);
                    }
                    if embed {
                        // Virtual display: only the SIZE matters (frames must fit the pane).
                        window.resize(g("w").max(1), g("h").max(1));
                        window.set_size_request(g("w").max(1), g("h").max(1));
                    } else {
                        if !window.is_visible() {
                            window.show();
                        }
                        window.move_(g("x"), g("y"));
                        window.resize(g("w").max(1), g("h").max(1));
                        if let Some(gdk_win) = window.window() {
                            gdk_win.move_resize(g("x"), g("y"), g("w").max(1), g("h").max(1));
                            gdk_win.raise();
                        }
                    }
                }
                Some("visible") => {
                    if embed {
                        // Offscreen windows are never user-visible; nothing to do.
                    } else if cmd.get("value").and_then(|v| v.as_bool()).unwrap_or(true) {
                        window.present(); // show AND raise above the editor
                    } else {
                        window.hide();
                    }
                }
                Some("quit") => {
                    gtk::main_quit();
                    return glib::ControlFlow::Break;
                }
                _ => {}
            }
        }
        glib::ControlFlow::Continue
    });

    gtk::main();
    Ok(())
}
