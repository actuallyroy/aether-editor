// MCP tool registry + executor. `list()` is the static `tools/list` payload (served on
// the WS thread); `execute()` runs a single `tools/call` on the UI thread against
// `&mut App` (so it can read live editor state and apply intents).

use std::path::PathBuf;

use serde_json::{json, Value};

use crate::commands::Command;
use crate::lsp::Severity;
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
            "Type text into the focused terminal (append a newline to run it)",
            json!({"type":"object","properties":{
                "text":{"type":"string"},
                "enter":{"type":"boolean","description":"Append Enter to run (default true)"}
            },"required":["text"]}),
        ),
    ])
}

fn tool(name: &str, desc: &str, schema: Value) -> Value {
    json!({ "name": name, "description": desc, "inputSchema": schema })
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
            let mut bytes = text.as_bytes().to_vec();
            if enter {
                bytes.push(b'\r');
            }
            app.terminal.write_focused(&bytes);
            Ok(json!({ "ok": true }))
        }

        other => Err(format!("unknown tool: {other}")),
    }
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
