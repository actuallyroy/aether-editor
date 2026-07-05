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

/// A discovered runnable extension (has a `main`, i.e. real code — not a theme/grammar).
#[derive(Clone)]
pub struct ExtInfo {
    pub path: PathBuf,
    pub name: String,
    pub activation_events: Vec<String>,
    /// Palette-visible commands from `contributes.commands`: `(command id, title)`.
    pub commands: Vec<(String, String)>,
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
                            let title = c.get("title").and_then(|t| t.as_str()).unwrap_or(&cmd).to_string();
                            Some((cmd, title))
                        })
                        .collect()
                })
                .unwrap_or_default();
            out.push(ExtInfo { path: p, name, activation_events, commands });
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

/// The bundled `ext-host/` directory (holds the sample extension), derived from the
/// host script location.
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
