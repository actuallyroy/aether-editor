// Cross-window control: one Aether window drives another over the SAME localhost
// MCP WebSocket server every window already runs for Claude Code (see `server.rs`).
// Every window advertises itself via a `~/.claude/ide/<port>.lock` file (pid, port,
// auth token, workspace); this module reads those locks to find other live windows
// and acts as an MCP *client* against them — connect, `tools/call`, done. No new
// server, no new protocol: a window controlling another window looks exactly like
// Claude Code controlling this one.

use std::time::Duration;

use serde_json::{json, Value};
use tungstenite::client::IntoClientRequest;

use super::discovery;

pub struct WindowInfo {
    pub pid: u64,
    pub port: u16,
    pub token: String,
    pub workspace: String,
}

/// Every other live Aether window (excludes `exclude_pid` — normally this process),
/// found via the same lockfiles Claude Code uses for discovery.
pub fn list_windows(exclude_pid: u32) -> Vec<WindowInfo> {
    discovery::list_locks()
        .into_iter()
        .filter_map(|path| {
            let body = std::fs::read_to_string(&path).ok()?;
            let v: Value = serde_json::from_str(&body).ok()?;
            if v.get("aetherNative").and_then(|b| b.as_bool()) != Some(true) {
                return None; // an extension's own lock, not a window we can control
            }
            let pid = v.get("pid").and_then(|p| p.as_u64())?;
            if pid == exclude_pid as u64 || !discovery::pid_alive(pid) {
                return None;
            }
            let port: u16 = path.file_stem()?.to_str()?.parse().ok()?;
            let token = v.get("authToken").and_then(|t| t.as_str())?.to_string();
            let workspace = v
                .get("workspaceFolders")
                .and_then(|w| w.as_array())
                .and_then(|a| a.first())
                .and_then(|w| w.as_str())
                .unwrap_or("")
                .to_string();
            Some(WindowInfo { pid, port, token, workspace })
        })
        .collect()
}

/// Run one MCP tool against another window and return its result — the same
/// `tools/call` any MCP client (Claude Code) would send, just dialed locally.
pub fn call_remote(win: &WindowInfo, tool: &str, args: Value, timeout: Duration) -> Result<Value, String> {
    let url = format!("ws://127.0.0.1:{}", win.port);
    let mut req = url.into_client_request().map_err(|e| e.to_string())?;
    let header_value = win.token.parse().map_err(|_| "invalid auth token".to_string())?;
    req.headers_mut().insert("x-claude-code-ide-authorization", header_value);
    let (mut ws, _) = tungstenite::connect(req).map_err(|e| format!("couldn't reach window (pid {}): {e}", win.pid))?;
    if let tungstenite::stream::MaybeTlsStream::Plain(s) = ws.get_ref() {
        let _ = s.set_read_timeout(Some(timeout));
    }
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": tool, "arguments": args },
    });
    ws.send(tungstenite::Message::Text(request.to_string())).map_err(|e| e.to_string())?;
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        match ws.read() {
            Ok(tungstenite::Message::Text(t)) => {
                let Ok(v) = serde_json::from_str::<Value>(&t) else { continue };
                if v.get("id").and_then(|i| i.as_i64()) != Some(1) {
                    continue; // not our reply (shouldn't happen on a fresh connection)
                }
                if let Some(err) = v.get("error") {
                    return Err(err.get("message").and_then(|m| m.as_str()).unwrap_or("remote error").to_string());
                }
                let result = v.get("result").cloned().unwrap_or(Value::Null);
                let is_error = result.get("isError").and_then(|b| b.as_bool()).unwrap_or(false);
                let text = result
                    .get("content")
                    .and_then(|c| c.as_array())
                    .and_then(|a| a.first())
                    .and_then(|b| b.get("text"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string();
                let parsed = serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text.clone()));
                return if is_error { Err(text) } else { Ok(parsed) };
            }
            Ok(_) => continue,
            Err(e) => return Err(format!("connection to pid {} dropped: {e}", win.pid)),
        }
    }
    Err(format!("timed out waiting for pid {}", win.pid))
}
