// MCP tool registry + executor. `list()` is the static `tools/list` payload (served on
// the WS thread); `execute()` runs a single `tools/call` on the UI thread against
// `&mut App` (so it can read live editor state and apply intents).

use std::path::PathBuf;

use serde_json::{json, Value};

use crate::commands::Command;
use crate::lsp::Severity;
use crate::mcp::relevance;
use crate::ui::Intent;

/// The `tools/list` response: tools surfaced to the agent as `mcp__ide__<name>`.
/// Native Claude-Code IDE tool names/shapes (see PROTOCOL.md) so the client uses them.
pub fn list() -> Value {
    json!([
        tool("getWorkspaceFolders", "Get the workspace folder paths currently open in the IDE", json!({"type":"object","properties":{}})),
        tool("getOpenEditors", "Get information about the currently open editor tabs", json!({"type":"object","properties":{}})),
        tool("getCurrentSelection", "Get the current text selection in the active editor", json!({"type":"object","properties":{}})),
        tool("getLatestSelection", "Get the most recent text selection (even if focus moved)", json!({"type":"object","properties":{}})),
        tool(
            "getDiagnostics",
            "Get language diagnostics (errors/warnings) from the IDE",
            json!({"type":"object","properties":{"uri":{"type":"string","description":"Optional file path/URI; omit for all open files."}}}),
        ),
        tool(
            "openFile",
            "Open a file in the editor and optionally place the cursor at a line",
            json!({"type":"object","properties":{
                "filePath":{"type":"string","description":"Path to the file to open"},
                "line":{"type":"number","description":"1-based line to place the caret"}
            },"required":["filePath"]}),
        ),
        tool(
            "setCommitMessage",
            "Set the text in Aether's Source Control commit message box",
            json!({"type":"object","properties":{
                "message":{"type":"string","description":"Commit message text to place in the box"}
            },"required":["message"]}),
        ),
        tool(
            "getActiveFile",
            "Get the active editor's path, language, full text, and cursor position",
            json!({"type":"object","properties":{}}),
        ),
        tool(
            "insertText",
            "Insert text at the cursor in the active editor (replaces the selection if any)",
            json!({"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}),
        ),
        tool(
            "saveActiveFile",
            "Save the active editor to disk",
            json!({"type":"object","properties":{}}),
        ),
        tool(
            "runCommand",
            "Run a named editor command. One of: save, find, undo, redo, selectAll, \
             formatDocument, toggleSidebar, toggleTerminal, newFile, toggleLineComment, \
             gotoDefinition, gotoReferences, nextProblem, prevProblem, renameSymbol, \
             nextEditor, prevEditor, markdownPreview, openSettings",
            json!({"type":"object","properties":{"command":{"type":"string"}},"required":["command"]}),
        ),
        tool(
            "gitStatus",
            "Get the working-tree git status (staged/worktree status per path)",
            json!({"type":"object","properties":{}}),
        ),
        tool(
            "gitCommit",
            "Commit staged changes (or everything with stageAll) with a message",
            json!({"type":"object","properties":{
                "message":{"type":"string"},
                "stageAll":{"type":"boolean","description":"Stage all changes first (default false)"},
                "push":{"type":"boolean","description":"Push after committing (default false)"}
            },"required":["message"]}),
        ),
        tool(
            "gitStageAll",
            "Stage all working-tree changes",
            json!({"type":"object","properties":{}}),
        ),
        tool(
            "terminalSend",
            "Send text to a terminal, then submit it — in one call. The text is pasted (so \
             TUIs like claude code receive it intact), then a follow-up key is pressed: by \
             default Enter, or pass `keys` for a custom sequence (e.g. [\"enter\"], \
             [\"down\",\"enter\"], [\"ctrl-c\"]). Set enter:false to send text with no \
             keypress. Targets a terminal by stable `id` (preferred), `index`, or the \
             focused terminal; doesn't change which terminal is focused.",
            json!({"type":"object","properties":{
                "text":{"type":"string"},
                "keys":{"type":"array","items":{"type":"string"},"description":"Keys pressed after the text, in order (e.g. [\"enter\"]). Overrides `enter`. Names: ctrl-a..ctrl-z, enter, escape, tab, backspace, up, down, left, right."},
                "enter":{"type":"boolean","description":"Press Enter after the text when `keys` is omitted (default true)"},
                "id":{"type":"number","description":"Stable terminal id (preferred)"},
                "index":{"type":"number","description":"Tab position index (fallback); omit for the focused terminal"}
            },"required":["text"]}),
        ),
        tool(
            "terminalSendKey",
            "Send a control key / keypress to a terminal (use this to stop a process — \
             `ctrl-c` — not terminalSend, which would type the text literally). Supports: \
             ctrl-a..ctrl-z, enter, escape, tab, backspace, up, down, left, right. Target \
             by stable `id` (preferred), `index`, or the focused terminal.",
            json!({"type":"object","properties":{
                "key":{"type":"string","description":"e.g. \"ctrl-c\", \"enter\", \"up\""},
                "id":{"type":"number","description":"Stable terminal id (preferred)"},
                "index":{"type":"number","description":"Tab position index (fallback); omit for the focused terminal"}
            },"required":["key"]}),
        ),
        tool(
            "terminalOutput",
            "Read the text a terminal is showing (history + visible screen), so you can see \
             a command's output or an interactive session's reply. Omit `lines` for the \
             full buffer, or pass it for just the last N lines. Target by stable `id` \
             (preferred), `index`, or the focused terminal.",
            json!({"type":"object","properties":{
                "lines":{"type":"number","description":"Last N lines (partial); omit for the full buffer"},
                "id":{"type":"number","description":"Stable terminal id (preferred)"},
                "index":{"type":"number","description":"Tab position index (fallback); omit for the focused terminal"}
            }}),
        ),
        tool(
            "newTerminal",
            "Create a new terminal tab (starts in the workspace root). Returns its stable \
             `id` (use it for later terminalSend/terminalOutput/etc.) and index. Does not \
             steal focus unless `focus` is true.",
            json!({"type":"object","properties":{
                "name":{"type":"string","description":"Optional title for the new tab"},
                "focus":{"type":"boolean","description":"Make the new tab active (default true)"}
            }}),
        ),
        tool(
            "focusTerminal",
            "Focus a terminal tab (shows the panel and makes it active). Target by stable \
             `id` (preferred) or `index`.",
            json!({"type":"object","properties":{
                "id":{"type":"number","description":"Stable terminal id (preferred)"},
                "index":{"type":"number","description":"Tab position index (fallback)"}
            }}),
        ),
        tool(
            "listTerminals",
            "List the terminal tabs (stable id, index, title, active). The `id` is the \
             shell's process id — stable across reordering, MCP reconnects, and Aether \
             restarts; use it for later calls (`index` is positional and shifts).",
            json!({"type":"object","properties":{}}),
        ),
        tool(
            "submitFeedback",
            "File feedback about Aether (this IDE / the aether-ide MCP) as a GitHub issue on \
             actuallyroy/aether-editor, authored under the user's account via their `gh` \
             CLI. Use for bugs, missing tools, or surprising behavior in Aether itself. \
             Pass `issue` to comment on an existing issue instead of opening a new one.",
            json!({"type":"object","properties":{
                "message":{"type":"string","description":"The feedback — be specific: what you tried, what happened, what you expected."},
                "title":{"type":"string","description":"Optional issue title (new issues only; auto-derived from the message otherwise)"},
                "issue":{"type":"number","description":"Comment on this existing issue number instead of opening a new one"}
            },"required":["message"]}),
        ),
        tool(
            "renameTerminal",
            "Rename a terminal tab. Target by stable `id` (preferred), `index`, or the \
             focused tab.",
            json!({"type":"object","properties":{
                "name":{"type":"string","description":"New tab title"},
                "id":{"type":"number","description":"Stable terminal id (preferred)"},
                "index":{"type":"number","description":"Tab position index (fallback); omits to the active tab"}
            },"required":["name"]}),
        ),
        tool(
            "listWindows",
            "List every OTHER running Aether window (each editing a different workspace), \
             found via the same discovery mechanism Claude Code uses to connect. Returns \
             each window's `pid` (pass to controlWindow/stopWindowVoice) and its workspace \
             path. Use this to find/control a different Aether window from this one.",
            json!({"type":"object","properties":{}}),
        ),
        tool(
            "controlWindow",
            "Run any other Aether MCP tool (openFile, closeEditor, gitStatus, insertText, \
             searchWorkspace, etc.) against a DIFFERENT Aether window instead of this one — \
             full remote control, same tool names/args as calling them locally. Get `pid` \
             from listWindows first.",
            json!({"type":"object","properties":{
                "pid":{"type":"number","description":"Target window's pid, from listWindows"},
                "tool":{"type":"string","description":"Tool name to run there (e.g. \"openFile\", \"gitStatus\")"},
                "arguments":{"type":"object","description":"Arguments for that tool, same shape as calling it directly"}
            },"required":["pid","tool"]}),
        ),
        tool(
            "stopVoice",
            "Stop THIS window's own active voice call, if any (no-op if none is active). To \
             stop another window's call, use controlWindow with tool: \"stopVoice\".",
            json!({"type":"object","properties":{}}),
        ),
        tool(
            "closeEditor",
            "Close an editor tab. Target by `filePath` (matches the end of the path) or \
             `index`; omit both to close the active tab. Unsaved changes prompt the normal \
             Save/Don't Save/Cancel dialog instead of closing immediately.",
            json!({"type":"object","properties":{
                "filePath":{"type":"string","description":"Path (or suffix of one) of the open tab to close"},
                "index":{"type":"number","description":"Tab position index"}
            }}),
        ),
        tool(
            "closeAllEditors",
            "Close every open editor tab. Clean tabs close immediately; the first dirty tab \
             (if any) prompts the Save/Don't Save/Cancel dialog and the rest are left open \
             until that's resolved.",
            json!({"type":"object","properties":{}}),
        ),
        tool(
            "searchWorkspace",
            "Relevance-ranked search across every file in the workspace (respects the same \
             ignore rules as the file tree — .git, node_modules, target, etc. skipped). Not \
             literal grep: plain words are ranked by relevance (common words matter less, \
             rare/repeated terms matter more); wrap an exact phrase or identifier in \
             \"double quotes\" for a required exact match, boosted above word-scored hits. \
             Use this over openFile+reading text when you don't already know which file has \
             what you're after.",
            json!({"type":"object","properties":{
                "query":{"type":"string","description":"Words and/or \"quoted phrases\""},
                "path":{"type":"string","description":"Optional subdirectory (relative to the workspace root) to restrict the search to"},
                "maxResults":{"type":"number","description":"Cap on ranked hits returned (default 30)"}
            },"required":["query"]}),
        ),
        tool(
            "searchTerminal",
            "Relevance-ranked search over one terminal's full scrollback (history + visible \
             screen) — for finding a specific past command/error/line in a long session \
             without reading the whole buffer. Same ranking as searchWorkspace: plain words \
             ranked by relevance, \"quoted phrases\" required exact + boosted. Target by \
             stable `id` (preferred), `index`, or the focused terminal.",
            json!({"type":"object","properties":{
                "query":{"type":"string","description":"Words and/or \"quoted phrases\""},
                "id":{"type":"number","description":"Stable terminal id (preferred)"},
                "index":{"type":"number","description":"Tab position index (fallback); omit for the focused terminal"},
                "maxResults":{"type":"number","description":"Cap on ranked hits returned (default 20)"}
            },"required":["query"]}),
        ),
    ])
}

fn tool(name: &str, desc: &str, schema: Value) -> Value {
    json!({ "name": name, "description": desc, "inputSchema": schema })
}

/// Resolve which terminal tab a call targets: prefer the stable `id`, fall back to the
/// position `index`, else the focused tab. Errors if a given id/index doesn't exist.
fn target_tab(app: &crate::App, args: &Value) -> Result<usize, String> {
    if let Some(id) = args.get("id").and_then(|v| v.as_u64()) {
        return app.terminal.tab_index_by_id(id).ok_or_else(|| format!("no terminal with id {id}"));
    }
    if let Some(i) = args.get("index").and_then(|v| v.as_u64()) {
        let i = i as usize;
        return (i < app.terminal.tab_count()).then_some(i).ok_or_else(|| format!("no terminal at index {i}"));
    }
    Ok(app.terminal.active)
}

/// Run one tool against the editor. Returns the structured JSON payload (the server
/// wraps it as MCP text content) or an error message.
pub fn execute(app: &mut crate::App, name: &str, args: &Value) -> Result<Value, String> {
    match name {
        "getWorkspaceFolders" => Ok(json!({
            "folders": [{
                "path": app.cwd.to_string_lossy(),
                "name": app.cwd.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
            }]
        })),

        "getOpenEditors" => {
            let editors: Vec<Value> = app
                .workspace
                .documents
                .iter()
                .enumerate()
                .map(|(i, d)| {
                    json!({
                        "filePath": d.path.as_ref().map(|p| p.to_string_lossy().to_string()),
                        "name": d.name,
                        "active": app.workspace.active == Some(i),
                        "dirty": d.dirty,
                    })
                })
                .collect();
            Ok(json!({ "editors": editors }))
        }

        "getCurrentSelection" | "getLatestSelection" => {
            let Some(d) = app.workspace.active_doc() else {
                return Ok(json!({ "text": "", "isEmpty": true }));
            };
            let (a, b) = d.sel.range();
            let (sl, sc) = d.lsp_pos(a);
            let (el, ec) = d.lsp_pos(b);
            Ok(json!({
                "text": d.selected_text().unwrap_or_default(),
                "filePath": d.path.as_ref().map(|p| p.to_string_lossy().to_string()),
                "isEmpty": a == b,
                "selection": {
                    "start": { "line": sl, "character": sc },
                    "end": { "line": el, "character": ec },
                },
            }))
        }

        "getDiagnostics" => {
            let want = args.get("uri").and_then(|u| u.as_str());
            let mut files = Vec::new();
            for d in &app.workspace.documents {
                let path = d.path.as_ref().map(|p| p.to_string_lossy().to_string());
                if let (Some(w), Some(p)) = (want, &path) {
                    if !p.ends_with(w) && p.as_str() != w {
                        continue;
                    }
                }
                if d.diagnostics.is_empty() {
                    continue;
                }
                let diags: Vec<Value> = d
                    .diagnostics
                    .iter()
                    .map(|g| {
                        json!({
                            "severity": severity_num(g.severity),
                            "message": g.message,
                            "source": g.source,
                            "range": {
                                "start": { "line": g.start_line, "character": g.start_char },
                                "end": { "line": g.end_line, "character": g.end_char },
                            },
                        })
                    })
                    .collect();
                files.push(json!({ "uri": path, "diagnostics": diags }));
            }
            Ok(json!({ "files": files }))
        }

        "openFile" => {
            let path = args
                .get("filePath")
                .and_then(|p| p.as_str())
                .ok_or("openFile requires filePath")?;
            let line = args.get("line").and_then(|l| l.as_u64()).unwrap_or(1).max(1) as usize;
            app.apply_intent(Intent::OpenFile { path: PathBuf::from(path), line, col: 0 });
            Ok(json!({ "ok": true, "filePath": path }))
        }

        "setCommitMessage" => {
            let text = args.get("message").and_then(|m| m.as_str()).unwrap_or("");
            match (app.gpu.as_mut(), app.source_control.as_mut()) {
                (Some(g), Some(scp)) => {
                    scp.set_generated_message(&mut g.font_system, Some(text));
                    Ok(json!({ "ok": true }))
                }
                _ => Err("source control panel not available".into()),
            }
        }

        "getActiveFile" => {
            let Some(d) = app.workspace.active_doc() else {
                return Ok(json!({ "open": false }));
            };
            let (line, ch) = d.lsp_pos(d.caret_byte());
            Ok(json!({
                "open": true,
                "path": d.path.as_ref().map(|p| p.to_string_lossy().to_string()),
                "name": d.name,
                "language": d.language_id(),
                "dirty": d.dirty,
                "cursor": { "line": line, "character": ch },
                "text": d.text(),
            }))
        }

        "insertText" => {
            let text = args.get("text").and_then(|t| t.as_str()).ok_or("insertText requires text")?;
            match (app.gpu.as_mut(), app.workspace.active_doc_mut()) {
                (Some(g), Some(d)) if !d.read_only => {
                    d.insert_str(text, &mut g.font_system);
                    Ok(json!({ "ok": true }))
                }
                (_, Some(_)) => Err("active document is read-only".into()),
                _ => Err("no active document".into()),
            }
        }

        "saveActiveFile" => {
            match app.workspace.active_doc_mut() {
                Some(d) => {
                    d.save().map_err(|e| format!("save failed: {e}"))?;
                    Ok(json!({ "ok": true }))
                }
                None => Err("no active document".into()),
            }
        }

        "runCommand" => {
            let id = args.get("command").and_then(|c| c.as_str()).unwrap_or("");
            let cmd = match id {
                "save" => Command::Save,
                "find" => Command::Find,
                "undo" => Command::Undo,
                "redo" => Command::Redo,
                "selectAll" => Command::SelectAll,
                "formatDocument" => Command::FormatDocument,
                "toggleSidebar" => Command::ToggleSidebar,
                "toggleTerminal" => Command::ToggleTerminal,
                "newFile" => Command::NewFile,
                "toggleLineComment" => Command::ToggleLineComment,
                "gotoDefinition" => Command::GotoDefinition,
                "gotoReferences" => Command::GotoReferences,
                "nextProblem" => Command::NextProblem,
                "prevProblem" => Command::PrevProblem,
                "renameSymbol" => Command::RenameSymbol,
                "nextEditor" => Command::NextEditor,
                "prevEditor" => Command::PrevEditor,
                "markdownPreview" => Command::MarkdownPreview,
                "openSettings" => Command::OpenSettings,
                other => return Err(format!("unknown command: {other}")),
            };
            app.exec_command(cmd);
            Ok(json!({ "ok": true }))
        }

        "gitStatus" => {
            let changes: Vec<Value> = crate::git::status(&app.cwd)
                .into_iter()
                .map(|c| json!({ "staged": c.staged.to_string(), "worktree": c.worktree.to_string(), "path": c.path }))
                .collect();
            Ok(json!({ "branch": crate::git::branch(&app.cwd), "changes": changes }))
        }

        "gitStageAll" => {
            app.apply_intent(Intent::GitStageAll);
            Ok(json!({ "ok": true }))
        }

        "gitCommit" => {
            let msg = args.get("message").and_then(|m| m.as_str()).ok_or("gitCommit requires message")?.to_string();
            let stage_all = args.get("stageAll").and_then(|b| b.as_bool()).unwrap_or(false);
            let push = args.get("push").and_then(|b| b.as_bool()).unwrap_or(false);
            if push {
                app.apply_intent(Intent::GitCommitPush { msg, stage_all });
            } else {
                app.apply_intent(Intent::GitCommit { msg, stage_all });
            }
            Ok(json!({ "ok": true }))
        }

        "terminalSend" => {
            let text = args.get("text").and_then(|t| t.as_str()).ok_or("terminalSend requires text")?;
            let enter = args.get("enter").and_then(|b| b.as_bool()).unwrap_or(true);
            app.terminal.visible = true;
            let idx = target_tab(app, args)?;
            // Paste the text the same way a real paste does (bracketed-paste wrapped when the
            // running app enabled it) so TUIs like claude code receive it intact. Then send
            // the follow-up key(s) as separate keypresses *outside* the paste markers, which
            // is what actually submits — a raw `text\r` write doesn't reliably submit a TUI.
            app.terminal.paste_to(idx, text);
            // Resolve the effective key sequence: explicit `keys` (e.g. ["enter"], or
            // ["down","enter"]); else a single Enter unless enter:false. Reported back so
            // the agent can see what was actually pressed (not a silent default).
            let sent_keys: Vec<String> = match args.get("keys").and_then(|k| k.as_array()) {
                Some(keys) => keys
                    .iter()
                    .map(|k| k.as_str().map(str::to_string).ok_or("keys entries must be strings"))
                    .collect::<Result<_, _>>()?,
                None if enter => vec!["enter".to_string()],
                None => vec![],
            };
            // Validate keys up front, then deliver them a tick AFTER the paste — sending
            // Enter in the same instant lets a TUI (claude code) swallow it while still
            // digesting the bracketed paste. Staggered a few ms apart to preserve order.
            let mut fire = std::time::Instant::now() + std::time::Duration::from_millis(60);
            for name in &sent_keys {
                let b = key_bytes(name).ok_or_else(|| format!("unknown key: {name}"))?;
                app.pending_term_keys.push((fire, idx, b));
                fire += std::time::Duration::from_millis(20);
            }
            app.redraw();
            Ok(json!({ "ok": true, "id": app.terminal.tab_id(idx), "text": text, "keys": sent_keys }))
        }

        "terminalSendKey" => {
            let key = args.get("key").and_then(|k| k.as_str()).ok_or("terminalSendKey requires key")?;
            let bytes = key_bytes(key).ok_or_else(|| format!("unknown key: {key}"))?;
            app.terminal.visible = true;
            let idx = target_tab(app, args)?;
            app.terminal.write_to(idx, &bytes);
            app.redraw();
            Ok(json!({ "ok": true, "id": app.terminal.tab_id(idx), "key": key }))
        }

        "terminalOutput" => {
            let idx = target_tab(app, args)?;
            // Omit `lines` → full buffer (a large cap; scrollback is bounded anyway).
            let lines = args.get("lines").and_then(|l| l.as_u64()).map(|l| l as usize).unwrap_or(usize::MAX);
            let output = app.terminal.read_tab(idx, lines).unwrap_or_default();
            Ok(json!({ "id": app.terminal.tab_id(idx), "index": idx, "output": output }))
        }

        "newTerminal" => {
            let focus = args.get("focus").and_then(|b| b.as_bool()).unwrap_or(true);
            let prev_active = app.terminal.active;
            app.terminal.visible = true;
            let panel = app.layout().terminal_panel;
            app.terminal.new_terminal_tab(panel, app.terminal_cell_w);
            let index = app.terminal.active; // new_terminal_tab makes it active
            // The terminal id is the shell's pid, which isn't known until the daemon
            // spawns it and replies. Pump the daemon connection briefly so we return the
            // real (stable) id instead of 0.
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(2000);
            while app.terminal.tab_id(index).unwrap_or(0) == 0 && std::time::Instant::now() < deadline {
                app.terminal.poll();
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let id = app.terminal.tab_id(index);
            if let Some(name) = args.get("name").and_then(|n| n.as_str()) {
                app.terminal.rename_tab(index, name);
            }
            if !focus {
                app.terminal.active = prev_active; // created in the background
            }
            app.redraw();
            Ok(json!({ "ok": true, "id": id, "index": index }))
        }

        "focusTerminal" => {
            let idx = target_tab(app, args)?;
            if !app.terminal.focus_tab(idx) {
                return Err("terminal no longer exists".to_string());
            }
            app.redraw();
            Ok(json!({ "ok": true, "id": app.terminal.tab_id(idx) }))
        }

        "submitFeedback" => {
            let message = args.get("message").and_then(|m| m.as_str()).ok_or("submitFeedback requires message")?;
            let repo = "actuallyroy/aether-editor";
            let body = format!(
                "{message}\n\n---\n_Filed via Aether's submitFeedback MCP tool — aether v{}_",
                env!("CARGO_PKG_VERSION")
            );
            // gh authors under the user's account (blocks briefly — a one-off, user-driven action).
            // Resolve gh via the platform-aware lookup: GUI-launched apps don't inherit
            // the shell PATH, so a bare "gh" often isn't found on macOS/Windows.
            let gh = crate::gh_program();
            let out = if let Some(n) = args.get("issue").and_then(|i| i.as_u64()) {
                std::process::Command::new(&gh)
                    .args(["issue", "comment", &n.to_string(), "--repo", repo, "--body", &body])
                    .output()
            } else {
                let title = args.get("title").and_then(|t| t.as_str()).map(String::from).unwrap_or_else(|| {
                    let snippet: String = message.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(80).collect();
                    format!("[feedback] {snippet}")
                });
                std::process::Command::new(&gh)
                    .args(["issue", "create", "--repo", repo, "--title", &title, "--body", &body])
                    .output()
            };
            match out {
                Ok(o) if o.status.success() => {
                    let url = String::from_utf8_lossy(&o.stdout).trim().lines().last().unwrap_or("").to_string();
                    Ok(json!({ "ok": true, "url": url }))
                }
                Ok(o) => Err(format!("gh failed: {}", String::from_utf8_lossy(&o.stderr).trim())),
                Err(e) => Err(format!(
                    "could not run gh — is the GitHub CLI installed and authenticated (`gh auth login`)? {e}"
                )),
            }
        }

        "listTerminals" => {
            let mut tabs = Vec::new();
            let mut i = 0;
            while let Some(title) = app.terminal.tab_title(i) {
                tabs.push(json!({
                    "id": app.terminal.tab_id(i),
                    "index": i,
                    "title": title,
                    "active": i == app.terminal.active,
                }));
                i += 1;
            }
            Ok(json!({ "terminals": tabs }))
        }

        "renameTerminal" => {
            let name = args.get("name").and_then(|n| n.as_str()).ok_or("renameTerminal requires name")?;
            let idx = target_tab(app, args)?;
            app.terminal.rename_tab(idx, name);
            Ok(json!({ "ok": true, "id": app.terminal.tab_id(idx) }))
        }

        "listWindows" => {
            let windows: Vec<Value> = crate::mcp::remote::list_windows(std::process::id())
                .into_iter()
                .map(|w| json!({ "pid": w.pid, "workspace": w.workspace }))
                .collect();
            Ok(json!({ "windows": windows }))
        }

        "controlWindow" => {
            let pid = args.get("pid").and_then(|p| p.as_u64()).ok_or("controlWindow requires pid")?;
            let tool_name = args.get("tool").and_then(|t| t.as_str()).ok_or("controlWindow requires tool")?;
            let tool_args = args.get("arguments").cloned().unwrap_or(json!({}));
            let win = crate::mcp::remote::list_windows(std::process::id())
                .into_iter()
                .find(|w| w.pid == pid)
                .ok_or_else(|| format!("no running window with pid {pid}"))?;
            crate::mcp::remote::call_remote(&win, tool_name, tool_args, std::time::Duration::from_secs(15))
        }

        "stopVoice" => {
            if let Some(c) = app.chat.as_mut() {
                if let Some(v) = c.voice.take() {
                    v.stop();
                }
            }
            app.redraw();
            Ok(json!({ "ok": true }))
        }

        "closeEditor" => {
            if args.get("filePath").is_none() && args.get("index").is_none() && app.workspace.documents.is_empty() {
                return Ok(json!({ "ok": true, "note": "no editors are open" }));
            }
            let idx = if let Some(path) = args.get("filePath").and_then(|p| p.as_str()) {
                // Non-file tabs (markdown preview, diff view, image) have no `path` —
                // fall back to matching the tab's display name too, so "close
                // readme.md" also finds a "Preview README.md" tab.
                app.workspace
                    .documents
                    .iter()
                    .position(|d| {
                        d.path.as_ref().map_or(false, |p| p.to_string_lossy().ends_with(path)) || d.name.ends_with(path)
                    })
                    .ok_or_else(|| format!("no open editor matches {path}"))?
            } else if let Some(i) = args.get("index").and_then(|v| v.as_u64()) {
                let i = i as usize;
                (i < app.workspace.documents.len()).then_some(i).ok_or_else(|| format!("no editor at index {i}"))?
            } else {
                app.workspace.active.ok_or("no active editor")?
            };
            app.request_close(idx);
            Ok(json!({ "ok": true, "closed": idx }))
        }

        "closeAllEditors" => {
            let dirty_remaining = loop {
                let Some(i) = app.workspace.documents.iter().position(|d| !d.dirty) else {
                    break app.workspace.documents.len();
                };
                app.workspace.close_idx(i);
            };
            if dirty_remaining > 0 {
                app.request_close(0); // prompts for the first (now only) dirty tab
            }
            app.redraw();
            Ok(json!({ "ok": true, "dirtyRemaining": dirty_remaining }))
        }

        "searchWorkspace" => {
            let query = args.get("query").and_then(|q| q.as_str()).ok_or("searchWorkspace requires query")?;
            let max_results = args.get("maxResults").and_then(|m| m.as_u64()).unwrap_or(30).max(1) as usize;
            let root = match args.get("path").and_then(|p| p.as_str()) {
                Some(p) => app.cwd.join(p),
                None => app.cwd.clone(),
            };
            let ranker = relevance::Ranker::new(query);
            if ranker.is_empty() {
                return Ok(json!({ "query": query, "results": [], "truncated": false }));
            }
            let (hits, truncated) = relevance::search_workspace(&root, &app.cwd, &ranker);
            let results: Vec<Value> = hits
                .into_iter()
                .take(max_results)
                .map(|h| json!({ "filePath": h.file, "line": h.line, "score": h.score, "text": h.text }))
                .collect();
            Ok(json!({ "query": query, "results": results, "truncated": truncated }))
        }

        "searchTerminal" => {
            let query = args.get("query").and_then(|q| q.as_str()).ok_or("searchTerminal requires query")?;
            let max_results = args.get("maxResults").and_then(|m| m.as_u64()).unwrap_or(20).max(1) as usize;
            let idx = target_tab(app, args)?;
            let output = app.terminal.read_tab(idx, usize::MAX).unwrap_or_default();
            let ranker = relevance::Ranker::new(query);
            if ranker.is_empty() {
                return Ok(json!({ "query": query, "id": app.terminal.tab_id(idx), "results": [] }));
            }
            let hits = relevance::search_lines(output.lines(), &ranker);
            let results: Vec<Value> = hits
                .into_iter()
                .take(max_results)
                .map(|h| json!({ "line": h.line, "score": h.score, "text": h.text }))
                .collect();
            Ok(json!({ "query": query, "id": app.terminal.tab_id(idx), "results": results }))
        }

        other => Err(format!("unknown tool: {other}")),
    }
}

/// Map a key name (case-insensitive) to the raw bytes a terminal expects. `ctrl-<letter>`
/// becomes the corresponding control byte (ctrl-c → 0x03); named keys cover the common
/// editing/navigation keys. Returns None for anything unrecognized.
fn key_bytes(key: &str) -> Option<Vec<u8>> {
    let k = key.trim().to_ascii_lowercase();
    // ctrl-<letter> / ^<letter> → control byte (letter & 0x1f).
    let ctrl = k.strip_prefix("ctrl-").or_else(|| k.strip_prefix("ctrl+")).or_else(|| k.strip_prefix('^'));
    if let Some(rest) = ctrl {
        let c = rest.chars().next()?;
        if rest.chars().count() == 1 && c.is_ascii_alphabetic() {
            return Some(vec![(c.to_ascii_uppercase() as u8) & 0x1f]);
        }
    }
    Some(match k.as_str() {
        "enter" | "return" | "cr" => vec![b'\r'],
        "escape" | "esc" => vec![0x1b],
        "tab" => vec![b'\t'],
        "backspace" | "bs" => vec![0x7f],
        "space" => vec![b' '],
        "up" => vec![0x1b, b'[', b'A'],
        "down" => vec![0x1b, b'[', b'B'],
        "right" => vec![0x1b, b'[', b'C'],
        "left" => vec![0x1b, b'[', b'D'],
        _ => return None,
    })
}

/// LSP severity → the 1–4 numbering MCP/LSP clients expect.
fn severity_num(s: Severity) -> u8 {
    match s {
        Severity::Error => 1,
        Severity::Warning => 2,
        Severity::Info => 3,
        Severity::Hint => 4,
    }
}
