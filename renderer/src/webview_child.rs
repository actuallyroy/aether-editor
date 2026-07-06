// macOS/Windows extension-webview backend: an IN-PROCESS webview (WKWebView /
// WebView2) attached as a child view of the editor window. Unlike the Linux path
// (separate GTK process on a virtual display, frames captured to a GPU texture,
// input injected with XTEST), these platforms let wry attach a webview to a winit
// window directly — the OS composites it over our surface, and native
// input/scrolling/cursors come for free.
//
// The trade-off is stacking: the NSView sits ABOVE everything we draw, so the
// dock-sync code hides it whenever an editor overlay (menu, palette, dialog)
// would need to render over the pane — same compromise the pre-virtual-display
// Linux path used.

#![cfg(any(target_os = "macos", target_os = "windows"))]

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use winit::event_loop::EventLoopProxy;
use winit::window::Window;

use crate::marketplace::WorkerMsg;
use crate::webview_shim::VSCODE_API_SHIM;

pub struct ChildWebview {
    pub instance_id: i64,
    pub view_id: String,
    pub title: String,
    webview: wry::WebView,
    page_html: Arc<Mutex<String>>,
}

impl ChildWebview {
    pub fn create(
        view_id: &str,
        instance_id: i64,
        title: &str,
        roots: &[PathBuf],
        window: &Window,
        tx: Sender<WorkerMsg>,
        proxy: Option<EventLoopProxy<()>>,
    ) -> Option<ChildWebview> {
        let page_html = Arc::new(Mutex::new(String::from(
            "<html><body style='background:#1e1e1e'></body></html>",
        )));
        let emit = {
            let tx = tx.clone();
            move |value: Value| {
                let _ = tx.send(WorkerMsg::WebviewEvent { instance: instance_id, value });
                if let Some(p) = proxy.as_ref() {
                    let _ = p.send_event(());
                }
            }
        };
        let roots = roots.to_vec();
        let page_html_proto = page_html.clone();
        let webview = wry::WebViewBuilder::new()
            .with_initialization_script(VSCODE_API_SHIM)
            .with_visible(false)
            .with_ipc_handler({
                let emit = emit.clone();
                move |req| {
                    // acquireVsCodeApi().postMessage(data) lands here (JSON string body).
                    let body = req.body().as_str();
                    let data: Value =
                        serde_json::from_str(body).unwrap_or(Value::String(body.to_string()));
                    if data.get("__aetherFocus").is_some() {
                        return; // native child view: focus is the OS's job
                    }
                    // The editor window's cursor tracking covers the child view too,
                    // so mirror the page's cursor exactly like the Linux path.
                    if let Some(c) = data.get("__aetherCursor").and_then(|c| c.as_str()) {
                        emit(json!({"event": "cursor", "value": c}));
                        return;
                    }
                    if let Some(kind) = data.get("__aetherConsole").and_then(|k| k.as_str()) {
                        let msg = data.get("msg").and_then(|m| m.as_str()).unwrap_or("");
                        eprintln!("[webview:{instance_id} {kind}] {msg}");
                        return;
                    }
                    emit(json!({"event": "message", "data": data}));
                }
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
            .build_as_child(window)
            .map_err(|e| eprintln!("[webview] create failed: {e}"))
            .ok()?;
        // The view exists as soon as it's built — tell the App to open its tab
        // (the Linux host reports `ready` once its GTK window is up).
        emit(json!({"event": "ready", "xid": 0}));
        Some(ChildWebview {
            instance_id,
            view_id: view_id.to_string(),
            title: title.to_string(),
            webview,
            page_html,
        })
    }

    pub fn set_html(&self, html: &str) {
        *self.page_html.lock().unwrap() = html.to_string();
        let _ = self.webview.load_url("aether-res://page/__page__.html");
    }

    pub fn post(&self, data: &Value) {
        // Deliver as a `message` event, exactly how VSCode webviews get it.
        // U+2028/U+2029 are valid JSON but break inline JS — escape them.
        let payload = serde_json::to_string(data)
            .unwrap_or_else(|_| "null".into())
            .replace('\u{2028}', "\\u2028")
            .replace('\u{2029}', "\\u2029");
        let js =
            format!("window.dispatchEvent(new MessageEvent('message', {{ data: {payload} }}));");
        let _ = self.webview.evaluate_script(&js);
    }

    /// Position the child view over the editor pane. Coordinates are PHYSICAL
    /// pixels (aether's layout space, top-left origin); wry converts per platform.
    /// AppKit view origins are BOTTOM-left and wry passes y through unflipped —
    /// flip against the window height on macOS. Win32 is top-left: no flip.
    pub fn set_bounds(&self, x: i32, y: i32, w: u32, h: u32, zoom: f64, win_h: u32) {
        #[cfg(target_os = "macos")]
        let flipped_y = win_h as i32 - y - h as i32;
        #[cfg(not(target_os = "macos"))]
        let flipped_y = {
            let _ = win_h;
            y
        };
        let _ = self.webview.set_bounds(wry::Rect {
            position: wry::dpi::PhysicalPosition::new(x, flipped_y).into(),
            size: wry::dpi::PhysicalSize::new(w.max(1), h.max(1)).into(),
        });
        let _ = self.webview.zoom(zoom);
    }

    /// Return keyboard focus (first responder) to the editor window's view.
    pub fn focus_parent(&self) {
        let _ = self.webview.focus_parent();
    }

    pub fn set_visible(&self, visible: bool) {
        let _ = self.webview.set_visible(visible);
    }
}
