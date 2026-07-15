// Auto-register the `aether --mcp` bridge with agent CLIs so the user never runs
// `claude mcp add`. We write a *local-scope* server entry (machine-only, not committed,
// loaded without an approval prompt) keyed to the workspace.

use std::path::Path;

use serde_json::{json, Value};

/// Register the bridge for Claude Code by merging an `aether` entry into
/// `~/.claude.json` under `projects.<workspace>.mcpServers`. Idempotent; preserves
/// every other field. Best-effort (silently no-ops on any error).
pub fn register_claude(workspace: &Path) {
    let Ok(exe) = std::env::current_exe() else { return };
    let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else { return };
    let path = home.join(".claude.json");

    // Load the existing config (Claude Code owns this file — preserve it verbatim).
    let mut root: Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({}));
    if !root.is_object() {
        return;
    }

    let ws_key = workspace.to_string_lossy().to_string();
    let entry = json!({
        "type": "stdio",
        "command": exe.to_string_lossy(),
        "args": ["--mcp"],
    });

    // root.projects.<ws>.mcpServers.aether = entry
    let projects = root
        .as_object_mut()
        .unwrap()
        .entry("projects")
        .or_insert_with(|| json!({}));
    let Some(projects) = projects.as_object_mut() else { return };
    let proj = projects.entry(ws_key).or_insert_with(|| json!({}));
    let Some(proj) = proj.as_object_mut() else { return };
    let servers = proj.entry("mcpServers").or_insert_with(|| json!({}));
    let Some(servers) = servers.as_object_mut() else { return };

    // Only rewrite when our entry is missing or stale (avoids needless churn / races
    // with Claude Code writing the same file).
    if servers.get("aether") == Some(&entry) {
        return;
    }
    servers.insert("aether".to_string(), entry);

    // Atomic write (temp + rename) so a concurrent Claude Code read never sees a
    // half-written file.
    if let Ok(text) = serde_json::to_string_pretty(&root) {
        let tmp = path.with_extension("json.aether-tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

/// Register the bridge for Codex by writing an `[mcp_servers.aether]` section into
/// `~/.codex/config.toml`. Codex's MCP config is GLOBAL (not per-project); the
/// `--mcp` proxy resolves the right editor instance from its cwd at connect time.
/// Textual edit (no toml dependency): replace our section if present, else append.
/// Idempotent, best-effort, and touches nothing outside our own section.
pub fn register_codex() {
    let Ok(exe) = std::env::current_exe() else { return };
    let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else { return };
    let dir = home.join(".codex");
    if !dir.is_dir() {
        return; // codex not installed — don't create its config tree
    }
    let path = dir.join("config.toml");
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let section = format!(
        "[mcp_servers.aether]\ncommand = \"{}\"\nargs = [\"--mcp\"]\n",
        exe.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"")
    );
    let new_text = if let Some(start) = text.find("[mcp_servers.aether]") {
        // Replace up to the next section header (or EOF).
        let rest = &text[start..];
        let end = rest[1..].find("\n[").map(|i| start + 1 + i + 1).unwrap_or(text.len());
        format!("{}{}{}", &text[..start], section, &text[end..])
    } else {
        let sep = if text.is_empty() || text.ends_with("\n\n") {
            ""
        } else if text.ends_with('\n') {
            "\n"
        } else {
            "\n\n"
        };
        format!("{text}{sep}{section}")
    };
    if new_text != text {
        let tmp = path.with_extension("toml.aether-tmp");
        if std::fs::write(&tmp, new_text).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}
