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

const VSCODE_API_SHIM: &str = r#"
(function () {
  let state;
  window.acquireVsCodeApi = function () {
    return {
      postMessage: (data) => window.ipc.postMessage(JSON.stringify(data)),
      getState: () => state,
      setState: (s) => { state = s; return s; },
    };
  };
})();
"#;

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

    // ---- GTK window ----
    use gtk::prelude::*;
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title(&title);
    window.set_default_size(width, height);
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    window.add(&vbox);
    window.connect_delete_event(|_, _| {
        out(json!({"event": "closed"}));
        gtk::main_quit();
        glib::Propagation::Proceed
    });

    // ---- webview ----
    let webview = {
        use wry::WebViewBuilderExtUnix;
        wry::WebViewBuilder::new()
            .with_initialization_script(VSCODE_API_SHIM)
            .with_ipc_handler(move |req| {
                // acquireVsCodeApi().postMessage(data) lands here (JSON string body).
                let body = req.body().as_str();
                let data: Value = serde_json::from_str(body).unwrap_or(Value::String(body.to_string()));
                out(json!({"event": "message", "data": data}));
            })
            .with_custom_protocol("aether-res".into(), move |_id, request| {
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
            .with_html("<html><body style='background:#1e1e1e'></body></html>")
            .build_gtk(&vbox)?
    };

    window.show_all();
    out(json!({"event": "ready"}));

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
                        let _ = webview.load_html(html);
                    }
                }
                Some("post") => {
                    // Deliver as a `message` event, exactly how VSCode webviews get it.
                    let data = cmd.get("data").cloned().unwrap_or(Value::Null);
                    let js = format!(
                        "window.dispatchEvent(new MessageEvent('message', {{ data: {} }}));",
                        serde_json::to_string(&data).unwrap_or_else(|_| "null".into())
                    );
                    let _ = webview.evaluate_script(&js);
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
