// A minimal Debug Adapter Protocol (DAP) client. DAP uses the SAME Content-Length
// framing as LSP (see lsp::frame / lsp::read_message), but the bodies are
// `{seq,type,command/event,...}` instead of JSON-RPC. This mirrors lsp.rs exactly:
// spawn the adapter as a child process (stdio), drain outgoing frames on a writer
// thread, parse adapter→client traffic on a reader thread, and post results to the
// UI over the existing `WorkerMsg` channel. No async runtime — blocking I/O on
// dedicated threads; the UI only mutates state in the worker poll.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::mpsc::Sender;

use serde_json::{json, Value};

use crate::lsp::{frame, read_message, quiet_command};
use crate::marketplace::WorkerMsg;

/// One frame of the call stack (from a `stackTrace` response).
#[derive(Clone)]
pub struct StackFrame {
    pub id: i64,
    pub name: String,
    pub path: Option<String>,
    pub line: i64,
}

/// A variable scope (locals/globals) for a stack frame.
#[derive(Clone)]
pub struct Scope {
    pub name: String,
    pub var_ref: i64,
}

/// A single variable (or a structured value whose children are fetched on expand
/// via `var_ref`).
#[derive(Clone)]
pub struct Variable {
    pub name: String,
    pub value: String,
    pub var_ref: i64,
}

type VarRefMap = std::sync::Arc<std::sync::Mutex<std::collections::HashMap<i64, i64>>>;

pub struct DapClient {
    outgoing: Sender<Vec<u8>>,
    next_seq: i64,
    /// request seq → command, so a response can be routed to the right WorkerMsg.
    pending: std::collections::HashMap<i64, &'static str>,
    /// `variables` request seq → its variablesReference, so the response (which
    /// doesn't echo the ref) can be attributed to the right tree node. Shared with
    /// the reader thread.
    var_refs: VarRefMap,
}

impl DapClient {
    /// Spawn `program args…` as a DAP adapter rooted at `cwd`. Returns None if the
    /// adapter binary can't be spawned (caller surfaces an install hint).
    pub fn start(program: &str, args: &[String], cwd: &PathBuf, tx: Sender<WorkerMsg>) -> Option<DapClient> {
        let mut cmd = quiet_command(program);
        cmd.args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().ok()?;
        let mut stdin = child.stdin.take()?;
        let stdout = child.stdout.take()?;
        let stderr = child.stderr.take();

        let (out_tx, out_rx) = std::sync::mpsc::channel::<Vec<u8>>();

        // Writer thread: drain outgoing frames to the adapter's stdin.
        std::thread::spawn(move || {
            while let Ok(buf) = out_rx.recv() {
                if stdin.write_all(&buf).is_err() || stdin.flush().is_err() {
                    break;
                }
            }
        });

        // Drain stderr; surface it as a status line.
        if let Some(err) = stderr {
            let tx2 = tx.clone();
            std::thread::spawn(move || {
                let mut r = BufReader::new(err);
                let mut line = String::new();
                while r.read_line(&mut line).map(|n| n > 0).unwrap_or(false) {
                    let msg = line.trim_end().to_string();
                    if !msg.is_empty() {
                        let _ = tx2.send(WorkerMsg::DebugLog { text: msg });
                    }
                    line.clear();
                }
            });
        }

        // Reader thread: parse adapter→client messages. Responses carry their own
        // `command` field, so routing is done on that; `variables` responses are
        // attributed to a node via the shared seq→var_ref map.
        let reply_tx = out_tx.clone();
        let var_refs: VarRefMap = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let reader_refs = var_refs.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                match read_message(&mut reader) {
                    Some(msg) => handle_message(&msg, &reply_tx, &tx, &reader_refs),
                    None => {
                        let _ = tx.send(WorkerMsg::DebugExited);
                        break;
                    }
                }
            }
        });

        Some(DapClient { outgoing: out_tx, next_seq: 1, pending: std::collections::HashMap::new(), var_refs })
    }

    fn send(&self, msg: Value) {
        let _ = self.outgoing.send(frame(&msg));
    }

    /// Send a request, recording its command for response routing. Returns the seq.
    fn request(&mut self, command: &'static str, arguments: Value) -> i64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.pending.insert(seq, command);
        self.send(json!({ "seq": seq, "type": "request", "command": command, "arguments": arguments }));
        seq
    }

    /// Reply to an adapter reverse-request (e.g. runInTerminal).
    pub fn reply(&mut self, request_seq: i64, command: &str, success: bool, body: Value) {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.send(json!({
            "seq": seq, "type": "response", "request_seq": request_seq,
            "success": success, "command": command, "body": body,
        }));
    }

    pub fn initialize(&mut self) {
        self.request("initialize", json!({
            "clientID": "aether",
            "clientName": "Aether",
            "adapterID": "aether",
            "pathFormat": "path",
            "linesStartAt1": true,
            "columnsStartAt1": true,
            "supportsRunInTerminalRequest": true,
            "supportsVariableType": true,
            "locale": "en",
        }));
    }

    /// Send the config-supplied `launch` arguments verbatim (merged with sane defaults).
    pub fn launch(&mut self, args: Value) {
        self.request("launch", args);
    }

    /// Send the config-supplied `attach` arguments verbatim.
    pub fn attach(&mut self, args: Value) {
        self.request("attach", args);
    }

    pub fn set_breakpoints(&mut self, path: &str, lines: &[i64]) {
        let bps: Vec<Value> = lines.iter().map(|l| json!({ "line": l })).collect();
        self.request("setBreakpoints", json!({
            "source": { "path": path, "name": std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default() },
            "breakpoints": bps,
            "lines": lines,
        }));
    }

    pub fn configuration_done(&mut self) {
        self.request("configurationDone", json!({}));
    }

    pub fn continue_(&mut self, thread_id: i64) {
        self.request("continue", json!({ "threadId": thread_id }));
    }
    pub fn pause(&mut self, thread_id: i64) {
        self.request("pause", json!({ "threadId": thread_id }));
    }
    /// Ask for the thread list (used to pick a thread to pause when not yet stopped).
    pub fn threads(&mut self) {
        self.request("threads", json!({}));
    }
    pub fn next(&mut self, thread_id: i64) {
        self.request("next", json!({ "threadId": thread_id }));
    }
    pub fn step_in(&mut self, thread_id: i64) {
        self.request("stepIn", json!({ "threadId": thread_id }));
    }
    pub fn step_out(&mut self, thread_id: i64) {
        self.request("stepOut", json!({ "threadId": thread_id }));
    }
    pub fn stack_trace(&mut self, thread_id: i64) {
        self.request("stackTrace", json!({ "threadId": thread_id, "startFrame": 0, "levels": 20 }));
    }
    pub fn scopes(&mut self, frame_id: i64) {
        self.request("scopes", json!({ "frameId": frame_id }));
    }
    pub fn variables(&mut self, var_ref: i64) {
        let seq = self.request("variables", json!({ "variablesReference": var_ref }));
        if let Ok(mut m) = self.var_refs.lock() {
            m.insert(seq, var_ref);
        }
    }
    pub fn disconnect(&mut self) {
        self.request("disconnect", json!({ "terminateDebuggee": true }));
    }
}

/// Dispatch an adapter→client message: route responses to WorkerMsg by command,
/// forward events, and reply to reverse-requests the adapter blocks on.
fn handle_message(msg: &Value, reply_tx: &Sender<Vec<u8>>, tx: &Sender<WorkerMsg>, var_refs: &VarRefMap) {
    match msg.get("type").and_then(|t| t.as_str()) {
        Some("event") => handle_event(msg, tx),
        Some("response") => handle_response(msg, tx, var_refs),
        Some("request") => handle_reverse_request(msg, reply_tx, tx),
        _ => {}
    }
}

fn handle_event(msg: &Value, tx: &Sender<WorkerMsg>) {
    let body = msg.get("body");
    match msg.get("event").and_then(|e| e.as_str()) {
        // Adapter is ready for configuration (breakpoints + configurationDone).
        Some("initialized") => {
            let _ = tx.send(WorkerMsg::DebugConfigured);
        }
        Some("stopped") => {
            let thread_id = body.and_then(|b| b.get("threadId")).and_then(|v| v.as_i64()).unwrap_or(0);
            let reason = body.and_then(|b| b.get("reason")).and_then(|v| v.as_str()).unwrap_or("").to_string();
            let _ = tx.send(WorkerMsg::DebugStopped { thread_id, reason });
        }
        Some("continued") => {
            let _ = tx.send(WorkerMsg::DebugContinued);
        }
        Some("output") => {
            let category = body.and_then(|b| b.get("category")).and_then(|v| v.as_str()).unwrap_or("console").to_string();
            let text = body.and_then(|b| b.get("output")).and_then(|v| v.as_str()).unwrap_or("").to_string();
            let _ = tx.send(WorkerMsg::DebugOutput { category, text });
        }
        Some("terminated") | Some("exited") => {
            let _ = tx.send(WorkerMsg::DebugTerminated);
        }
        _ => {}
    }
}

fn handle_response(msg: &Value, tx: &Sender<WorkerMsg>, var_refs: &VarRefMap) {
    let command = msg.get("command").and_then(|c| c.as_str()).unwrap_or("");
    let body = msg.get("body");
    let success = msg.get("success").and_then(|s| s.as_bool()).unwrap_or(false);
    if !success {
        if let Some(m) = msg.get("message").and_then(|m| m.as_str()) {
            let _ = tx.send(WorkerMsg::DebugLog { text: format!("{command} failed: {m}") });
        }
    }
    match command {
        "initialize" => {
            let _ = tx.send(WorkerMsg::DebugInitialized);
        }
        "setBreakpoints" => {
            let lines: Vec<i64> = body
                .and_then(|b| b.get("breakpoints"))
                .and_then(|a| a.as_array())
                .map(|a| a.iter().filter_map(|b| b.get("line").and_then(|v| v.as_i64())).collect())
                .unwrap_or_default();
            let _ = tx.send(WorkerMsg::DebugBreakpointsVerified { path: String::new(), lines });
        }
        "stackTrace" => {
            let frames = body
                .and_then(|b| b.get("stackFrames"))
                .and_then(|a| a.as_array())
                .map(|a| {
                    a.iter()
                        .map(|f| StackFrame {
                            id: f.get("id").and_then(|v| v.as_i64()).unwrap_or(0),
                            name: f.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                            path: f.get("source").and_then(|s| s.get("path")).and_then(|v| v.as_str()).map(|s| s.to_string()),
                            line: f.get("line").and_then(|v| v.as_i64()).unwrap_or(0),
                        })
                        .collect()
                })
                .unwrap_or_default();
            let _ = tx.send(WorkerMsg::DebugStackTrace { frames });
        }
        "threads" => {
            let ids: Vec<i64> = body
                .and_then(|b| b.get("threads"))
                .and_then(|a| a.as_array())
                .map(|a| a.iter().filter_map(|t| t.get("id").and_then(|v| v.as_i64())).collect())
                .unwrap_or_default();
            let _ = tx.send(WorkerMsg::DebugThreads { ids });
        }
        "scopes" => {
            let scopes = body
                .and_then(|b| b.get("scopes"))
                .and_then(|a| a.as_array())
                .map(|a| {
                    a.iter()
                        .map(|s| Scope {
                            name: s.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                            var_ref: s.get("variablesReference").and_then(|v| v.as_i64()).unwrap_or(0),
                        })
                        .collect()
                })
                .unwrap_or_default();
            let _ = tx.send(WorkerMsg::DebugScopes { scopes });
        }
        "variables" => {
            let vars = body
                .and_then(|b| b.get("variables"))
                .and_then(|a| a.as_array())
                .map(|a| {
                    a.iter()
                        .map(|v| Variable {
                            name: v.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                            value: v.get("value").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                            var_ref: v.get("variablesReference").and_then(|x| x.as_i64()).unwrap_or(0),
                        })
                        .collect()
                })
                .unwrap_or_default();
            // Attribute to the requested reference via the seq→var_ref map.
            let req_seq = msg.get("request_seq").and_then(|s| s.as_i64()).unwrap_or(0);
            let var_ref = var_refs.lock().ok().and_then(|mut m| m.remove(&req_seq)).unwrap_or(0);
            let _ = tx.send(WorkerMsg::DebugVariables { var_ref, vars });
        }
        _ => {}
    }
}

fn handle_reverse_request(msg: &Value, _reply_tx: &Sender<Vec<u8>>, tx: &Sender<WorkerMsg>) {
    let seq = msg.get("seq").and_then(|s| s.as_i64()).unwrap_or(0);
    match msg.get("command").and_then(|c| c.as_str()) {
        Some("runInTerminal") => {
            let args = msg.get("arguments");
            let cwd = args.and_then(|a| a.get("cwd")).and_then(|v| v.as_str()).unwrap_or("").to_string();
            let argv: Vec<String> = args
                .and_then(|a| a.get("args"))
                .and_then(|a| a.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
                .unwrap_or_default();
            // The UI runs the command in a terminal and replies (App owns the dap client).
            let _ = tx.send(WorkerMsg::DebugRunInTerminal { seq, cwd, args: argv });
        }
        _ => {}
    }
}
