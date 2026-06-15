// The localhost WebSocket MCP server: bind a port, advertise via the lockfile, and
// serve MCP JSON-RPC (initialize / tools/list / tools/call). Tool calls are forwarded
// to the UI thread (see `mcp::McpRequest`).

use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Sender};
use std::time::Duration;

use serde_json::{json, Value};
use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use winit::event_loop::EventLoopProxy;

use super::{discovery, tools, McpRequest, McpServer};

const PROTOCOL_VERSION: &str = "2024-11-05";

/// Bind a loopback port, write the discovery lockfile, and spawn the accept loop.
pub fn start(
    workspace: PathBuf,
    req_tx: Sender<McpRequest>,
    proxy: EventLoopProxy<()>,
) -> Option<McpServer> {
    let (listener, port) = bind_port()?;
    let token = discovery::gen_token();
    let lock_path = discovery::write_lock(port, &workspace, &token)?;

    let accept_token = token.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let token = accept_token.clone();
            let req_tx = req_tx.clone();
            let proxy = proxy.clone();
            std::thread::spawn(move || serve_conn(stream, token, req_tx, proxy));
        }
    });

    eprintln!("[mcp] IDE server on 127.0.0.1:{port}");
    Some(McpServer { port, lock_path })
}

/// Try random ports in [10000, 65535] (the range Claude Code expects) until one binds.
fn bind_port() -> Option<(TcpListener, u16)> {
    let mut seed = discovery::gen_token();
    for _ in 0..32 {
        // Cheap pseudo-random port from the token hash, no rand dependency.
        let n: u64 = seed.bytes().fold(1469598103934665603u64, |h, b| {
            (h ^ b as u64).wrapping_mul(1099511628211)
        });
        let port = 10000 + (n % 55536) as u16;
        if let Ok(l) = TcpListener::bind(("127.0.0.1", port)) {
            return Some((l, port));
        }
        seed = format!("{seed}{port}");
    }
    None
}

fn serve_conn(stream: TcpStream, token: String, req_tx: Sender<McpRequest>, proxy: EventLoopProxy<()>) {
    // WebSocket upgrade, validating the auth header Claude Code sends.
    let mut authed = false;
    let check = |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
        let ok = req
            .headers()
            .get("x-claude-code-ide-authorization")
            .and_then(|v| v.to_str().ok())
            .map_or(false, |v| v == token);
        if ok {
            Ok(resp)
        } else {
            let err = ErrorResponse::new(Some("invalid ide auth token".into()));
            Err(err)
        }
    };
    let _ = &mut authed;
    let mut ws = match tungstenite::accept_hdr(stream, check) {
        Ok(ws) => ws,
        Err(e) => {
            eprintln!("[mcp] handshake rejected: {e}");
            return;
        }
    };

    loop {
        let msg = match ws.read() {
            Ok(m) => m,
            Err(_) => break,
        };
        let text = match msg {
            tungstenite::Message::Text(t) => t,
            tungstenite::Message::Close(_) => break,
            tungstenite::Message::Ping(p) => {
                let _ = ws.write(tungstenite::Message::Pong(p));
                let _ = ws.flush();
                continue;
            }
            _ => continue,
        };
        let Ok(req) = serde_json::from_str::<Value>(&text) else { continue };
        // Notifications (no id) get no reply.
        let id = req.get("id").cloned();
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = req.get("params").cloned().unwrap_or(Value::Null);

        let response = match method {
            "initialize" => Some(ok_result(
                &id,
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "Aether", "version": env!("CARGO_PKG_VERSION") },
                }),
            )),
            "notifications/initialized" => None,
            "ping" => Some(ok_result(&id, json!({}))),
            "tools/list" => Some(ok_result(&id, json!({ "tools": tools::list() }))),
            "tools/call" => Some(handle_call(&id, &params, &req_tx, &proxy)),
            _ if id.is_some() => Some(err_result(&id, -32601, "method not found")),
            _ => None,
        };
        if let Some(resp) = response {
            if ws.write(tungstenite::Message::Text(resp.to_string())).is_err() {
                break;
            }
            let _ = ws.flush();
        }
    }
}

/// Forward a `tools/call` to the UI thread and wrap the result as MCP tool content.
fn handle_call(id: &Option<Value>, params: &Value, req_tx: &Sender<McpRequest>, proxy: &EventLoopProxy<()>) -> Value {
    let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
    let args = params.get("arguments").cloned().unwrap_or(Value::Null);
    let (tx, rx) = channel();
    if req_tx.send(McpRequest { tool: name, args, reply: tx }).is_err() {
        return err_result(id, -32603, "ide event loop unavailable");
    }
    let _ = proxy.send_event(()); // wake the UI thread to drain the request
    match rx.recv_timeout(Duration::from_secs(15)) {
        Ok(Ok(value)) => {
            // MCP tool result: a single text block carrying the JSON payload (matches
            // how Claude Code's IDE tools return structured data).
            let text = match &value {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            ok_result(id, json!({ "content": [{ "type": "text", "text": text }], "isError": false }))
        }
        Ok(Err(msg)) => ok_result(
            id,
            json!({ "content": [{ "type": "text", "text": msg }], "isError": true }),
        ),
        Err(_) => err_result(id, -32603, "tool timed out"),
    }
}

fn ok_result(id: &Option<Value>, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id.clone().unwrap_or(Value::Null), "result": result })
}

fn err_result(id: &Option<Value>, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id.clone().unwrap_or(Value::Null), "error": { "code": code, "message": message } })
}
