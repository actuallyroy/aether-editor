// Out-of-process extension host bridge. aether binds a localhost TCP listener, spawns
// the Node host (`ext-host/main.js`) with the port + a one-time token, and speaks
// newline-delimited JSON-RPC 2.0 over the socket (see `ext-host/` + PROTOCOL). Mirrors
// `lsp.rs`'s process + reader-thread model: the reader posts every inbound message as a
// `WorkerMsg` the App drains on the UI thread, and the App answers host→aether requests
// through this handle.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use winit::event_loop::EventLoopProxy;

use crate::marketplace::WorkerMsg;

pub struct ExtHost {
    writer: Arc<Mutex<Option<TcpStream>>>,
    next_id: AtomicI64,
    _child: std::process::Child,
}

impl Drop for ExtHost {
    // Belt-and-braces: the host exits when its socket closes, but a wedged or
    // busy node (extension servers keep the event loop alive) must not outlive
    // the editor — orphaned hosts hold ports/lockfiles and confuse later runs.
    fn drop(&mut self) {
        let _ = self._child.kill();
        let _ = self._child.wait();
    }
}

impl ExtHost {
    /// Spawn the Node host for `root` and start the accept/reader thread. Returns None if
    /// Node isn't available or the host script can't be located. The connection completes
    /// asynchronously — the App gets `WorkerMsg::ExtHostReady` once the host handshakes.
    pub fn start(root: &Path, tx: Sender<WorkerMsg>, proxy: Option<EventLoopProxy<()>>) -> Option<ExtHost> {
        let node = crate::lsp::resolve_node()?;
        let script = host_script()?;
        let listener = TcpListener::bind(("127.0.0.1", 0)).ok()?;
        let port = listener.local_addr().ok()?.port();
        let token = gen_token();

        let mut cmd = crate::lsp::quiet_command(&node);
        cmd.arg(&script).arg(port.to_string()).arg(&token);
        // Run the host from the workspace root so relative extension paths resolve.
        if root.is_dir() {
            cmd.current_dir(root);
        }
        let child = cmd.spawn().ok()?;

        let writer = Arc::new(Mutex::new(None));
        {
            let writer = writer.clone();
            std::thread::spawn(move || {
                // Accept exactly one connection — the host we just spawned.
                let stream = match listener.accept() {
                    Ok((s, _)) => s,
                    Err(_) => return,
                };
                let _ = stream.set_nodelay(true);
                let read_half = match stream.try_clone() {
                    Ok(s) => s,
                    Err(_) => return,
                };
                let mut reader = BufReader::new(read_half);
                // First line must be `host/ready` carrying our token.
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    return;
                }
                let ready: Value = serde_json::from_str(line.trim()).unwrap_or(Value::Null);
                let ok = ready.get("method").and_then(|m| m.as_str()) == Some("host/ready")
                    && ready.get("params").and_then(|p| p.get("token")).and_then(|t| t.as_str())
                        == Some(token.as_str());
                if !ok {
                    return; // untrusted / malformed — drop it
                }
                *writer.lock().unwrap() = Some(stream);
                let _ = tx.send(WorkerMsg::ExtHostReady);
                wake(&proxy);
                loop {
                    let mut line = String::new();
                    match reader.read_line(&mut line) {
                        Ok(0) | Err(_) => {
                            let _ = tx.send(WorkerMsg::ExtHostExited);
                            wake(&proxy);
                            break;
                        }
                        Ok(_) => {
                            let t = line.trim();
                            if t.is_empty() {
                                continue;
                            }
                            if let Ok(value) = serde_json::from_str::<Value>(t) {
                                let _ = tx.send(WorkerMsg::ExtHostMsg { value });
                                wake(&proxy);
                            }
                        }
                    }
                }
            });
        }
        Some(ExtHost { writer, next_id: AtomicI64::new(1), _child: child })
    }

    fn send(&self, msg: Value) {
        if let Some(w) = self.writer.lock().unwrap().as_mut() {
            if let Ok(mut s) = serde_json::to_string(&msg) {
                s.push('\n');
                let _ = w.write_all(s.as_bytes());
                let _ = w.flush();
            }
        }
    }

    pub fn notify(&self, method: &str, params: Value) {
        self.send(json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    /// Fire an aether→host request; returns the id so the caller matches the response
    /// (which arrives via `WorkerMsg::ExtHostMsg`).
    pub fn request(&self, method: &str, params: Value) -> i64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        id
    }

    /// Answer a host→aether request by echoing its id.
    pub fn respond(&self, id: &Value, result: Value) {
        self.send(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
    }

    pub fn respond_err(&self, id: &Value, message: &str) {
        self.send(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": message } }));
    }

    // ---- convenience wrappers for the first-slice methods ----
    pub fn init(&self, root: &Path) {
        // ALWAYS absolute: a relative root ("." from `aether .`) poisons every
        // uri extensions derive from the workspace — directory-walk loops in
        // MPE/crossnote never terminate on relative paths (dirname('.') == '.').
        let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        self.notify("host/init", json!({ "root": root.to_string_lossy() }));
    }
    pub fn activate(&self, ext_path: &Path) -> i64 {
        self.request("activate", json!({ "extensionPath": ext_path.to_string_lossy() }))
    }
    pub fn did_open(&self, uri: &str, language_id: &str, version: i32, text: &str) {
        self.notify(
            "workspace/didOpenTextDocument",
            json!({ "uri": uri, "languageId": language_id, "version": version, "text": text }),
        );
    }
    pub fn did_change(&self, uri: &str, version: i32, text: &str) {
        self.notify(
            "workspace/didChangeTextDocument",
            json!({ "uri": uri, "version": version, "text": text }),
        );
    }
    pub fn request_hover(&self, provider_id: i64, uri: &str, line: u32, character: u32) -> i64 {
        self.request(
            "hover/provide",
            json!({ "providerId": provider_id, "uri": uri, "line": line, "character": character }),
        )
    }
    pub fn did_change_active(&self, uri: &str, language_id: &str) {
        self.notify("editor/didChangeActive", json!({ "uri": uri, "languageId": language_id }));
    }
    pub fn did_save(&self, uri: &str) {
        self.notify("workspace/didSaveTextDocument", json!({ "uri": uri }));
    }
    /// Run an extension-registered command (e.g. "Go Live"). Fire-and-forget id.
    pub fn invoke_command(&self, command: &str) -> i64 {
        self.request("command/invoke", json!({ "command": command, "args": [] }))
    }
}

/// One live extension webview: a spawned `aether --webview-host` process (GTK +
/// webkit2gtk) showing the view. Talks JSON lines over stdio; its stdout events are
/// posted as `WorkerMsg::WebviewEvent` for the App to bridge to the extension host.
pub struct WebviewProc {
    pub instance_id: i64,
    pub view_id: String,
    pub title: String,
    /// Frame exchange file (/dev/shm) the host writes BGRA frames into.
    pub shm_path: PathBuf,
    /// The private Xvfb display number this webview renders on (embed mode).
    pub display: u32,
    stdin: Mutex<std::process::ChildStdin>,
    child: std::process::Child,
    xvfb: Option<std::process::Child>,
}

impl WebviewProc {
    pub fn start(
        view_id: &str,
        instance_id: i64,
        title: &str,
        roots: &[PathBuf],
        embed: bool,
        tx: Sender<WorkerMsg>,
        proxy: Option<EventLoopProxy<()>>,
    ) -> Option<WebviewProc> {
        // The webview host (GTK/webkit) is Linux-only for now; on other platforms
        // `--webview-host` would just open another editor window.
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (view_id, instance_id, title, roots, embed, tx, proxy);
            return None;
        }
        #[cfg(target_os = "linux")]
        {
        let exe = std::env::current_exe().ok()?;
        let mut cmd = std::process::Command::new(exe);
        cmd.arg("--webview-host")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit()); // surface webkit crashes in our log
        // Embed mode: give the host its own PRIVATE Xvfb display — it renders
        // normally there (accelerated, un-throttled, never on the user's screen);
        // the editor captures frames and injects XTEST input on that display.
        let mut xvfb = None;
        let mut display = 0u32;
        if embed {
            display = crate::virtual_display::free_display();
            xvfb = crate::virtual_display::spawn_xvfb(display);
            if xvfb.is_none() {
                return None; // no Xvfb — embedding unavailable
            }
            cmd.env("DISPLAY", format!(":{display}"));
            cmd.env("GDK_BACKEND", "x11");
            cmd.env_remove("WAYLAND_DISPLAY");
        }
        let mut child = cmd.spawn().ok()?;
        let mut stdin = child.stdin.take()?;
        let stdout = child.stdout.take()?;
        let shm_path = std::path::Path::new("/dev/shm")
            .join(format!("aether-wv-{}-{}", std::process::id(), instance_id.unsigned_abs()));
        let init = json!({
            "cmd": "init",
            "title": title,
            "width": 520,
            "height": 760,
            "embed": embed,
            "shm": shm_path.to_string_lossy(),
            "roots": roots.iter().map(|r| r.to_string_lossy()).collect::<Vec<_>>(),
        });
        let mut line = init.to_string();
        line.push('\n');
        stdin.write_all(line.as_bytes()).ok()?;
        let iid = instance_id;
        std::thread::spawn(move || {
            let reader = std::io::BufReader::new(stdout);
            for l in reader.lines() {
                let Ok(l) = l else { break };
                if let Ok(value) = serde_json::from_str::<Value>(&l) {
                    let _ = tx.send(WorkerMsg::WebviewEvent { instance: iid, value });
                    wake(&proxy);
                }
            }
            let _ = tx.send(WorkerMsg::WebviewEvent { instance: iid, value: json!({"event": "closed"}) });
            wake(&proxy);
        });
        Some(WebviewProc {
            instance_id,
            view_id: view_id.to_string(),
            title: title.to_string(),
            shm_path,
            display,
            stdin: Mutex::new(stdin),
            child,
            xvfb,
        })
        }
    }

    fn send(&self, v: Value) {
        if let Ok(mut w) = self.stdin.lock() {
            let mut s = v.to_string();
            s.push('\n');
            let _ = w.write_all(s.as_bytes());
            let _ = w.flush();
        }
    }
    // ---- input forwarding (editor → offscreen webview) ----
    pub fn input_motion(&self, x: f64, y: f64, state: u32) {
        self.send(json!({"cmd": "input", "kind": "motion", "x": x, "y": y, "state": state}));
    }
    pub fn input_button(&self, x: f64, y: f64, button: u32, press: bool, state: u32) {
        self.send(json!({"cmd": "input", "kind": "button", "x": x, "y": y, "button": button, "press": press, "state": state}));
    }
    pub fn input_scroll(&self, x: f64, y: f64, dx: f64, dy: f64, state: u32) {
        self.send(json!({"cmd": "input", "kind": "scroll", "x": x, "y": y, "dx": dx, "dy": dy, "state": state}));
    }
    pub fn input_key(&self, keyval: u32, press: bool, state: u32) {
        self.send(json!({"cmd": "input", "kind": "key", "keyval": keyval, "press": press, "state": state}));
    }

    /// Size-sync a docked webview window; `zoom` matches the editor's UI zoom.
    pub fn set_bounds(&self, x: i32, y: i32, w: u32, h: u32, zoom: f64) {
        self.send(json!({"cmd": "bounds", "x": x, "y": y, "w": w, "h": h, "zoom": zoom}));
    }
    pub fn set_visible(&self, value: bool) {
        self.send(json!({"cmd": "visible", "value": value}));
    }
    pub fn set_html(&self, html: &str) {
        self.send(json!({"cmd": "html", "html": html}));
    }
    pub fn post(&self, data: &Value) {
        self.send(json!({"cmd": "post", "data": data}));
    }
}

impl Drop for WebviewProc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(x) = self.xvfb.as_mut() {
            let _ = x.kill();
            let _ = x.wait();
        }
        let _ = std::fs::remove_file(&self.shm_path);
    }
}

/// A discovered runnable extension (has a `main`, i.e. real code — not a theme/grammar).
#[derive(Clone)]
pub struct ExtInfo {
    pub path: PathBuf,
    pub name: String,
    pub activation_events: Vec<String>,
    /// Palette-visible commands from `contributes.commands`: `(command id, title)`.
    pub commands: Vec<(String, String)>,
    /// Webview views from `contributes.views`: `(view id, display name)`.
    pub views: Vec<(String, String)>,
    /// Editor-title buttons from `contributes.menus."editor/title"` (navigation group).
    pub title_buttons: Vec<TitleButton>,
}

/// One `editor/title` toolbar button an extension contributes (VSCode shows these
/// at the right end of the tab bar when the `when` clause matches the active editor).
#[derive(Clone)]
pub struct TitleButton {
    pub command: String,
    /// Raw when-clause, e.g. `editorLangId == markdown` or `editorLangId =~ /^(a|b)$/`.
    pub when: String,
    /// Absolute path to the button's icon (dark variant), when it's a file.
    pub icon: Option<PathBuf>,
    /// Command title — tooltip/fallback.
    pub title: String,
}

/// Scan each directory for immediate subfolders containing a `package.json` with a
/// `main` entry, and read their name + activationEvents. This is the extension registry
/// the App activates from.
pub fn discover(dirs: &[PathBuf]) -> Vec<ExtInfo> {
    let mut out = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            let pkg = p.join("package.json");
            if !pkg.exists() {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&pkg) else { continue };
            let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
            if v.get("main").and_then(|m| m.as_str()).is_none() {
                continue; // themes/grammars have no `main` — not runnable
            }
            // VSCode localization: "%some.key%" strings resolve via package.nls.json.
            let nls: Option<Value> = std::fs::read_to_string(p.join("package.nls.json"))
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok());
            let localize = |s: &str| -> String {
                if let (Some(k), Some(n)) = (s.strip_prefix('%').and_then(|x| x.strip_suffix('%')), nls.as_ref()) {
                    if let Some(t) = n.get(k).and_then(|t| t.as_str()) {
                        return t.to_string();
                    }
                }
                s.to_string()
            };
            let name = v.get("name").and_then(|n| n.as_str()).unwrap_or("ext").to_string();
            let activation_events = v
                .get("activationEvents")
                .and_then(|a| a.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                .unwrap_or_default();
            let commands = v
                .pointer("/contributes/commands")
                .and_then(|c| c.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|c| {
                            let cmd = c.get("command")?.as_str()?.to_string();
                            let title = localize(c.get("title").and_then(|t| t.as_str()).unwrap_or(&cmd));
                            Some((cmd, title))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let views = v
                .pointer("/contributes/views")
                .and_then(|c| c.as_object())
                .map(|containers| {
                    containers
                        .values()
                        .filter_map(|arr| arr.as_array())
                        .flatten()
                        .filter_map(|view| {
                            let id = view.get("id")?.as_str()?.to_string();
                            let name = localize(
                                view.get("name")
                                    .and_then(|n| n.as_str())
                                    .filter(|n| !n.is_empty())
                                    .unwrap_or(&id),
                            );
                            Some((id, name))
                        })
                        .collect()
                })
                .unwrap_or_default();
            // editor/title toolbar buttons (navigation group only — the icon row).
            // The icon comes from the command's `icon` field: a {light,dark} file
            // pair (use dark — aether's chrome is dark) or a `$(codicon)` string
            // (no file; skipped for now).
            let title_buttons = v
                .pointer("/contributes/menus/editor~1title")
                .and_then(|m| m.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|item| {
                            let command = item.get("command")?.as_str()?.to_string();
                            if item.get("group").and_then(|g| g.as_str()).map_or(true, |g| !g.starts_with("navigation")) {
                                return None;
                            }
                            let when = item.get("when").and_then(|w| w.as_str()).unwrap_or("").to_string();
                            let cmd_def = v
                                .pointer("/contributes/commands")
                                .and_then(|c| c.as_array())
                                .and_then(|arr| arr.iter().find(|c| c.get("command").and_then(|x| x.as_str()) == Some(&command)));
                            let icon = cmd_def
                                .and_then(|c| c.get("icon"))
                                .and_then(|i| i.get("dark").and_then(|d| d.as_str()).or_else(|| i.as_str()))
                                .filter(|s| !s.starts_with("$("))
                                .map(|rel| p.join(rel.trim_start_matches("./")));
                            let title = localize(
                                cmd_def
                                    .and_then(|c| c.get("title"))
                                    .and_then(|t| t.as_str())
                                    .unwrap_or(&command),
                            );
                            Some(TitleButton { command, when, icon, title })
                        })
                        .collect()
                })
                .unwrap_or_default();
            out.push(ExtInfo { path: p, name, activation_events, commands, views, title_buttons });
        }
    }
    out
}

/// `~/.aether/extensions` — where installed (`.vsix`) extensions live. Created if absent.
pub fn user_extensions_dir() -> Option<PathBuf> {
    let dir = dirs_home()?.join(".aether").join("extensions");
    let _ = std::fs::create_dir_all(&dir);
    Some(dir)
}

/// The bundled `ext-host/` directory (room for future built-in extensions),
/// derived from the host script location.
pub fn bundled_extensions_dir() -> Option<PathBuf> {
    host_script().and_then(|p| p.parent().map(|d| d.to_path_buf()))
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from).or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
}

fn wake(proxy: &Option<EventLoopProxy<()>>) {
    if let Some(p) = proxy {
        let _ = p.send_event(());
    }
}

/// Locate `ext-host/main.js`. Checks (in order) next to the executable, one level up
/// (dev `target/debug/`), and the current working directory.
fn host_script() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("ext-host/main.js"));
            if let Some(up) = dir.parent() {
                candidates.push(up.join("ext-host/main.js"));
                if let Some(up2) = up.parent() {
                    candidates.push(up2.join("ext-host/main.js"));
                }
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("ext-host/main.js"));
    }
    // macOS launcher bundles hardlink the binary into a per-folder .app shim —
    // ext-host lives next to the CANONICAL executable they point back to.
    #[cfg(target_os = "macos")]
    if let Some(dir) = crate::macos_launcher::canonical_exe().parent() {
        candidates.push(dir.join("ext-host/main.js"));
    }
    // Installed locations (deb: /usr/share/aether; AppImage: <root>/usr/share/aether).
    if let Ok(exe) = std::env::current_exe() {
        if let Some(prefix) = exe.parent().and_then(|d| d.parent()) {
            candidates.push(prefix.join("share/aether/ext-host/main.js"));
        }
    }
    candidates.push(PathBuf::from("/usr/share/aether/ext-host/main.js"));
    candidates.into_iter().find(|p| p.exists())
}

/// A random hex token so only our spawned host can claim the localhost socket.
fn gen_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mk = || {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(std::process::id() as u64);
        h.finish()
    };
    format!("{:016x}{:016x}", mk(), mk())
}
