// `aether --mcp`: a stdio↔WebSocket MCP proxy.
//
// Claude Code (and other agents) allowlist the IDE channel's tools, so custom
// "drive the IDE" tools must be exposed via a *regular* MCP server. This process is
// that server: it finds the running Aether GUI (via `~/.claude/ide/<port>.lock`),
// connects to its WebSocket MCP endpoint, and transparently proxies MCP JSON-RPC
// between the agent's stdio and the GUI. The agent surfaces every tool as
// `mcp__<name>__*` (no filtering, unlike the IDE channel).
//
// MCP stdio transport is newline-delimited JSON. The flow is client-driven
// (request → response), so a synchronous pump is sufficient; server-initiated
// notifications are forwarded opportunistically after each request.

use std::io::{BufRead, Write};
use std::net::TcpStream;
use std::path::PathBuf;

use serde_json::Value;
use tungstenite::client::IntoClientRequest;
use tungstenite::WebSocket;

/// Entry point for `aether --mcp`. Returns when stdin closes or the GUI disconnects.
pub fn run_stdio() -> std::io::Result<()> {
    let cwd = std::env::current_dir().unwrap_or_default();
    let (port, token) = match find_server(&cwd) {
        Some(v) => v,
        None => {
            eprintln!("[aether --mcp] no running Aether window found for {cwd:?} (open this folder in Aether first)");
            return Ok(());
        }
    };

    let mut req = format!("ws://127.0.0.1:{port}/")
        .into_client_request()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
    req.headers_mut().insert(
        "x-claude-code-ide-authorization",
        token.parse().map_err(|_| std::io::Error::new(std::io::ErrorKind::Other, "bad token"))?,
    );
    let (mut ws, _) = tungstenite::connect(req)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("connect: {e}")))?;
    eprintln!("[aether --mcp] connected to GUI on 127.0.0.1:{port}");

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        // Forward the client message to the GUI.
        if ws.send(tungstenite::Message::Text(line.clone())).is_err() {
            break;
        }
        let _ = ws.flush();
        // A request (has "id") expects a response; a notification does not.
        let is_request = serde_json::from_str::<Value>(&line)
            .ok()
            .and_then(|v| v.get("id").cloned())
            .map_or(false, |id| !id.is_null());
        if !is_request {
            continue;
        }
        // Pump GUI messages back until the matching response arrives.
        loop {
            match ws.read() {
                Ok(tungstenite::Message::Text(t)) => {
                    writeln!(stdout, "{t}")?;
                    stdout.flush()?;
                    // The reply to our request ends this turn; any preceding
                    // notifications were already forwarded above.
                    let done = serde_json::from_str::<Value>(&t)
                        .ok()
                        .map_or(true, |v| v.get("id").map_or(false, |id| !id.is_null()));
                    if done {
                        break;
                    }
                }
                Ok(tungstenite::Message::Ping(p)) => {
                    let _ = ws.send(tungstenite::Message::Pong(p));
                    let _ = ws.flush();
                }
                Ok(tungstenite::Message::Close(_)) | Err(_) => return Ok(()),
                Ok(_) => {}
            }
        }
    }
    Ok(())
}

/// Find a running Aether GUI whose workspace best matches `cwd`. Prefers a window
/// whose `workspaceFolders` contains `cwd`; falls back to any live lockfile.
fn find_server(cwd: &std::path::Path) -> Option<(u16, String)> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let dir = home.join(".claude").join("ide");
    let mut fallback = None;
    let mut best: Option<(usize, u16, String)> = None; // (match len, port, token)
    for entry in std::fs::read_dir(&dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("lock") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
        // Only consider Aether's own lockfiles (VS Code etc. also write here).
        if v.get("ideName").and_then(|n| n.as_str()) != Some("Aether") {
            continue;
        }
        let Some(port) = path.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<u16>().ok()) else {
            continue;
        };
        let Some(token) = v.get("authToken").and_then(|t| t.as_str()) else { continue };
        if fallback.is_none() {
            fallback = Some((port, token.to_string()));
        }
        if let Some(folders) = v.get("workspaceFolders").and_then(|f| f.as_array()) {
            for f in folders {
                if let Some(ws) = f.as_str() {
                    if cwd.starts_with(ws) {
                        let len = ws.len();
                        if best.as_ref().map_or(true, |(l, _, _)| len > *l) {
                            best = Some((len, port, token.to_string()));
                        }
                    }
                }
            }
        }
    }
    best.map(|(_, p, t)| (p, t)).or(fallback)
}

// Keep the type import meaningful even if unused on some platforms.
type _Ws = WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>;
