// The localhost WebSocket MCP server: bind a port, advertise via the lockfile, and
// serve MCP JSON-RPC (initialize / tools/list / tools/call). Tool calls are forwarded
// to the UI thread (see `mcp::McpRequest`).

use std::io::ErrorKind;
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Sender, TryRecvError};
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
    Some(McpServer { port, lock_path, token })
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

    // Decouple tool execution from the read loop. The old design ran each
    // `tools/call` inline and blocked the connection for up to 15s waiting on the UI
    // thread — during which incoming WebSocket Pings went unanswered (tungstenite only
    // auto-replies to Pings when read()/flush() is called), so the client's keepalive
    // tore the connection down. It got worse under load, when the UI thread is busy and
    // replies are slow. See issues #43/#45.
    //
    // Now the socket is non-blocking: the loop polls reads frequently (so pings are
    // answered within ~10ms), and each `tools/call` runs on its own worker thread that
    // posts the finished JSON-RPC response back over `resp_rx`. Responses are written as
    // they complete (JSON-RPC matches by id, so out-of-order delivery is fine). The
    // connection stays alive and responsive under sustained multi-terminal load.
    if ws.get_ref().set_nonblocking(true).is_err() {
        return;
    }
    let (resp_tx, resp_rx) = channel::<Value>();
    loop {
        // 1) Write any tool responses that have completed since the last pass.
        loop {
            match resp_rx.try_recv() {
                Ok(v) => {
                    if !write_msg(&mut ws, tungstenite::Message::Text(v.to_string())) {
                        return;
                    }
                }
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
        // 2) Read the next client message (non-blocking; WouldBlock → idle nap).
        let msg = match ws.read() {
            Ok(m) => m,
            Err(tungstenite::Error::Io(e))
                if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut =>
            {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            Err(_) => break,
        };
        let text = match msg {
            tungstenite::Message::Text(t) => t,
            tungstenite::Message::Close(_) => break,
            // read() already enqueued the Pong; just push it out promptly.
            tungstenite::Message::Ping(_) => {
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

        // `tools/call` can block on the UI thread for up to 15s → run it off the read
        // loop so pings and other requests keep being serviced meanwhile.
        if method == "tools/call" {
            let params = params.clone();
            let req_tx = req_tx.clone();
            let proxy = proxy.clone();
            let resp_tx = resp_tx.clone();
            std::thread::spawn(move || {
                let _ = resp_tx.send(handle_call(&id, &params, &req_tx, &proxy));
            });
            continue;
        }

        // Cheap, non-blocking methods are handled inline.
        let response = match method {
            "initialize" => Some(ok_result(
                &id,
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "Aether", "version": env!("CARGO_PKG_VERSION") },
                    // Surfaced to the model on connect: orient it on the terminal tools and
                    // list the terminals already open (so it knows them without a tool call).
                    "instructions": startup_instructions(&req_tx, &proxy),
                }),
            )),
            "notifications/initialized" => None,
            "ping" => Some(ok_result(&id, json!({}))),
            "tools/list" => Some(ok_result(&id, json!({ "tools": tools::list() }))),
            _ if id.is_some() => Some(err_result(&id, -32601, "method not found")),
            _ => None,
        };
        if let Some(resp) = response {
            if !write_msg(&mut ws, tungstenite::Message::Text(resp.to_string())) {
                break;
            }
        }
    }
}

/// Send one WebSocket message on a non-blocking socket, retrying the flush while it
/// would block (the message is already queued inside tungstenite). Returns false on a
/// fatal connection error — the caller should drop the connection.
fn write_msg(ws: &mut tungstenite::WebSocket<TcpStream>, msg: tungstenite::Message) -> bool {
    match ws.write(msg) {
        Ok(()) => {}
        Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::WouldBlock => {}
        Err(_) => return false,
    }
    loop {
        match ws.flush() {
            Ok(()) => return true,
            Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(_) => return false,
        }
    }
}

/// Run one tool on the UI thread (via the bridge) and return its raw result Value.
fn call_tool(name: &str, args: Value, req_tx: &Sender<McpRequest>, proxy: &EventLoopProxy<()>) -> Result<Value, String> {
    let (tx, rx) = channel();
    if req_tx.send(McpRequest { tool: name.to_string(), args, reply: tx }).is_err() {
        return Err("ide event loop unavailable".into());
    }
    let _ = proxy.send_event(()); // wake the UI thread to drain the request
    rx.recv_timeout(Duration::from_secs(15)).unwrap_or_else(|_| Err("tool timed out".into()))
}

/// Build the `initialize` instructions: orient the model on the terminal tools and list
/// the terminals already open, so it starts knowing them without a `listTerminals` call.
fn startup_instructions(req_tx: &Sender<McpRequest>, proxy: &EventLoopProxy<()>) -> String {
    let mut s = String::from(
        "Aether IDE (aether-ide). You can drive the editor and its integrated terminals. \
         Terminals are addressed by a STABLE `id` (not position) — pass it to terminalSend / \
         terminalSendKey / terminalOutput / focusTerminal. terminalSend pastes text then \
         presses Enter by default; pass `keys` for a custom key sequence after the text \
         (e.g. [\"enter\"], [\"ctrl-c\"], [\"down\",\"enter\"]) or enter:false for none. \
         terminalOutput reads a terminal's content (omit `lines` for all, or pass N for the \
         last N). Call listTerminals anytime to refresh. Report bugs/feedback about Aether \
         itself with submitFeedback (files a GitHub issue via the user's gh CLI).\n\n",
    );
    let terms = call_tool("listTerminals", Value::Null, req_tx, proxy)
        .ok()
        .and_then(|v| v.get("terminals").and_then(|t| t.as_array()).cloned())
        .unwrap_or_default();
    if terms.is_empty() {
        s.push_str("Open terminals: none yet (use newTerminal to create one).");
    } else {
        s.push_str("Open terminals:");
        for t in &terms {
            let id = t.get("id").and_then(|v| v.as_u64());
            let title = t.get("title").and_then(|v| v.as_str()).unwrap_or("shell");
            let active = t.get("active").and_then(|v| v.as_bool()).unwrap_or(false);
            let id_s = id.map(|i| i.to_string()).unwrap_or_else(|| "?".into());
            s.push_str(&format!("\n  - id {id_s}: \"{title}\"{}", if active { " (focused)" } else { "" }));
        }
    }
    s
}

/// Forward a `tools/call` to the UI thread and wrap the result as MCP tool content.
fn handle_call(id: &Option<Value>, params: &Value, req_tx: &Sender<McpRequest>, proxy: &EventLoopProxy<()>) -> Value {
    let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
    let args = params.get("arguments").cloned().unwrap_or(Value::Null);
    match call_tool(&name, args, req_tx, proxy) {
        Ok(value) => {
            // MCP tool result: a single text block carrying the JSON payload (matches
            // how Claude Code's IDE tools return structured data).
            let text = match &value {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            ok_result(id, json!({ "content": [{ "type": "text", "text": text }], "isError": false }))
        }
        Err(msg) => ok_result(
            id,
            json!({ "content": [{ "type": "text", "text": msg }], "isError": true }),
        ),
    }
}

fn ok_result(id: &Option<Value>, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id.clone().unwrap_or(Value::Null), "result": result })
}

fn err_result(id: &Option<Value>, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id.clone().unwrap_or(Value::Null), "error": { "code": code, "message": message } })
}
