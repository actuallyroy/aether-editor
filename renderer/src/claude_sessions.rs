// Claude-session liveness tracking + one-click restore.
//
// Aether owns the integrated terminal, so it can see exactly which panes have a
// `claude` process running (via the pty-host's process-tree query). We persist the
// set of sessions that are *currently running*, rewriting it whenever the set
// changes. The consequences:
//
//   * Claude exits back to a shell prompt, or the user closes the tab  →  the set
//     shrinks and we rewrite  →  that session is NOT a restore candidate
//     (the user ended it on purpose).
//   * Aether is force-killed / its pty-host daemon dies  →  no final rewrite, so
//     the file still lists what was running  →  those are restore candidates.
//
// On a normal quit or an Aether-only crash the daemon survives and re-offers the
// shells, which reattach on their own — so restore only kicks in when the shells
// are genuinely gone. A reboot (boot time newer than our last save) is excluded,
// per the user's intent ("not when shutting down or rebooting").

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::ptyhost::TermId;
use crate::settings::config_dir;

/// One tracked Claude session in the current window.
#[derive(Clone, Debug)]
pub struct LiveSession {
    pub cwd: PathBuf,
    /// Newest transcript stem under `~/.claude/projects/<encoded cwd>/`, used to
    /// `claude --resume <id>`. `None` ⇒ fall back to `claude --continue`.
    pub session_id: Option<String>,
}

/// A restore candidate offered to the user on launch.
#[derive(Clone, Debug)]
pub struct RestoreItem {
    pub cwd: PathBuf,
    pub session_id: Option<String>,
}

impl RestoreItem {
    /// The shell command that brings the session back.
    pub fn command(&self) -> String {
        match &self.session_id {
            Some(id) => format!("claude --resume {id}"),
            None => "claude --continue".to_string(),
        }
    }
}

/// Tracks per-terminal Claude liveness and maintains the persisted running-set.
pub struct ClaudeWatcher {
    workspace: PathBuf,
    /// term id → its running session. Only running sessions are kept here; an entry
    /// is dropped the moment Claude exits or the pane disappears.
    live: HashMap<TermId, LiveSession>,
    dirty: bool,
}

impl ClaudeWatcher {
    pub fn new(workspace: PathBuf) -> Self {
        Self { workspace, live: HashMap::new(), dirty: false }
    }

    pub fn set_workspace(&mut self, workspace: PathBuf) {
        if workspace != self.workspace {
            self.workspace = workspace;
            self.live.clear();
            self.dirty = false;
        }
    }

    /// Reconcile against the daemon's view: `live_ids` are the panes with Claude
    /// running now, `terms` is `(id, cwd)` for every bound pane. Returns true if the
    /// running-set changed (the caller should persist).
    pub fn observe(&mut self, live_ids: &[TermId], terms: &[(TermId, PathBuf)]) -> bool {
        let cwd_of: HashMap<TermId, PathBuf> = terms.iter().cloned().collect();
        // Session ids already claimed by a still-live pane, per cwd — so multiple Claude
        // terminals in the SAME directory each map to a DISTINCT transcript (newest
        // unclaimed first) instead of all resuming the same (newest) session on restore.
        let mut claimed: HashMap<PathBuf, std::collections::HashSet<String>> = HashMap::new();
        for (id, s) in &self.live {
            if live_ids.contains(id) && cwd_of.get(id) == Some(&s.cwd) {
                if let Some(sid) = &s.session_id {
                    claimed.entry(s.cwd.clone()).or_default().insert(sid.clone());
                }
            }
        }
        // Add / refresh sessions whose pane currently runs Claude.
        for id in live_ids {
            let Some(cwd) = cwd_of.get(id) else { continue };
            match self.live.get(id) {
                Some(s) if &s.cwd == cwd => {}
                _ => {
                    let taken = claimed.entry(cwd.clone()).or_default();
                    let session_id = session_ids(cwd).into_iter().find(|sid| !taken.contains(sid));
                    if let Some(sid) = &session_id {
                        taken.insert(sid.clone());
                    }
                    self.live.insert(*id, LiveSession { cwd: cwd.clone(), session_id });
                    self.dirty = true;
                }
            }
        }
        // Drop sessions that stopped running, or whose pane is gone (tab closed) —
        // both count as an intentional end, so they leave the restore set.
        let before = self.live.len();
        self.live.retain(|id, _| live_ids.contains(id) && cwd_of.contains_key(id));
        if self.live.len() != before {
            self.dirty = true;
        }
        std::mem::take(&mut self.dirty)
    }

    /// The current running-set, for persistence.
    pub fn snapshot(&self) -> Vec<LiveSession> {
        self.live.values().cloned().collect()
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }
}

/// Encode an absolute path the way Claude Code names its project transcript dir:
/// every `/`, space and `.` becomes `-` (e.g. `/Users/me/My Proj` →
/// `-Users-me-My-Proj`).
fn encode_cwd(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|c| if c == '/' || c == ' ' || c == '.' { '-' } else { c })
        .collect()
}

/// All transcript session ids for `cwd`, newest (most recently modified) first.
pub fn session_ids(cwd: &Path) -> Vec<String> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    let dir = home.join(".claude").join("projects").join(encode_cwd(cwd));
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut items: Vec<(std::time::SystemTime, String)> = Vec::new();
    for entry in rd.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else { continue };
        let Ok(mtime) = entry.metadata().and_then(|m| m.modified()) else { continue };
        items.push((mtime, stem.to_string()));
    }
    items.sort_by(|a, b| b.0.cmp(&a.0)); // newest first
    items.into_iter().map(|(_, id)| id).collect()
}

/// Seconds-since-epoch of the last system boot (macOS/BSD `kern.boottime`; Linux
/// `/proc/stat btime`). `None` if it can't be read — callers then skip the reboot
/// gate (fail open: better to offer a restore than silently drop it).
pub fn boot_time() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("sysctl").args(["-n", "kern.boottime"]).output().ok()?;
        let s = String::from_utf8_lossy(&out.stdout);
        // e.g. "{ sec = 1718000000, usec = 0 } Mon Jun 10 ..."
        let sec = s.split("sec = ").nth(1)?.split(|c: char| !c.is_ascii_digit()).next()?;
        sec.parse().ok()
    }
    #[cfg(target_os = "linux")]
    {
        let stat = std::fs::read_to_string("/proc/stat").ok()?;
        stat.lines().find_map(|l| l.strip_prefix("btime ").and_then(|v| v.trim().parse().ok()))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

fn store_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("claude-sessions.json"))
}

/// Persist the running-set for `workspace`, stamped with the current boot time.
/// Other workspaces' entries in the file are preserved.
pub fn save(workspace: &Path, sessions: &[LiveSession]) {
    let Some(path) = store_path() else { return };
    let mut root = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    let entries: Vec<Value> = sessions
        .iter()
        .map(|s| {
            json!({
                "cwd": s.cwd.to_string_lossy(),
                "sessionId": s.session_id,
            })
        })
        .collect();
    root.insert(
        workspace.to_string_lossy().to_string(),
        json!({ "boot": boot_time(), "sessions": entries }),
    );
    if let Ok(text) = serde_json::to_string_pretty(&Value::Object(root)) {
        let _ = std::fs::write(&path, text);
    }
}

/// Load the restore candidates for `workspace`, or empty if nothing was persisted.
///
/// Unlike a generic terminal process — which is unrecoverable once killed — a Claude
/// session lives in its on-disk transcript (`~/.claude/projects/…`), so `--resume`
/// reconstructs it even across a reboot. We therefore restore regardless of boot
/// time; the launch flow still drops any session the surviving daemon already
/// reattached, so a clean quit doesn't produce duplicates.
pub fn load_candidates(workspace: &Path) -> Vec<RestoreItem> {
    let Some(path) = store_path() else { return Vec::new() };
    let Ok(text) = std::fs::read_to_string(&path) else { return Vec::new() };
    let Ok(root) = serde_json::from_str::<Value>(&text) else { return Vec::new() };
    let Some(entry) = root.get(workspace.to_string_lossy().as_ref()) else { return Vec::new() };
    entry
        .get("sessions")
        .and_then(|s| s.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|s| {
                    let cwd = s.get("cwd").and_then(|c| c.as_str()).filter(|c| !c.is_empty())?;
                    Some(RestoreItem {
                        cwd: PathBuf::from(cwd),
                        session_id: s.get("sessionId").and_then(|i| i.as_str()).map(String::from),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}
