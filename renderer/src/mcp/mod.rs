// Claude Code IDE-integration MCP server.
//
// Aether runs a localhost WebSocket MCP server (this module). When the user runs
// `claude` in Aether's integrated terminal, it reads `CLAUDE_CODE_SSE_PORT` (injected
// by the pty-host into the shell env), finds our `~/.claude/ide/<port>.lock`, connects,
// and calls IDE tools. Aether is the MCP *server*; `claude` is the client.
//
// Threading: the WS server runs on background threads (one per connection). Tool calls
// that touch editor state can't run there, so each `tools/call` is forwarded to the UI
// thread as an `McpRequest` (carrying a one-shot reply channel) and the event loop is
// woken via the `EventLoopProxy`. `App::about_to_wait` drains the queue, runs the tool
// against `&mut App` (see `mcp::tools::execute`), and sends the JSON result back.
//
// Protocol details are pinned in `PROTOCOL.md` (reverse-engineered from the VS Code
// extension; re-verify on Claude Code updates).

use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use serde_json::Value;
use winit::event_loop::EventLoopProxy;

pub mod agents;
pub mod discovery;
pub mod proxy;
pub mod server;
pub mod tools;

/// A tool invocation forwarded from a WS connection thread to the UI thread. The
/// handler runs `tools::execute` and sends the JSON result (or error) back on `reply`.
pub struct McpRequest {
    pub tool: String,
    pub args: Value,
    /// One-shot reply: `Ok(result_value)` or `Err(message)`.
    pub reply: Sender<Result<Value, String>>,
}

/// A running MCP server: the port we bound (for terminal env injection) and the
/// discovery lockfile path (removed on drop).
pub struct McpServer {
    pub port: u16,
    lock_path: PathBuf,
    token: String,
}

impl McpServer {
    /// Re-advertise this window for a new workspace (the user opened a folder in an
    /// existing/folder-less window). Rewrites the discovery lockfile in place — same
    /// port and token, so any live `claude` connection is undisturbed — but with the
    /// new `workspaceFolders`, so Claude Code can match the IDE to the session.
    pub fn set_workspace(&self, workspace: &Path) {
        let _ = discovery::write_lock(self.port, workspace, &self.token);
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.lock_path);
    }
}

/// Start the IDE MCP server for `workspace`. Binds a random localhost port, writes the
/// discovery lockfile, and spawns the accept loop. Tool calls arrive on `req_tx`; the
/// loop is woken via `proxy`. Returns `None` if we couldn't bind/advertise.
pub fn start(
    workspace: PathBuf,
    req_tx: Sender<McpRequest>,
    proxy: EventLoopProxy<()>,
) -> Option<McpServer> {
    server::start(workspace, req_tx, proxy)
}
