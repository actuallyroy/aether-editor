// The `~/.claude/ide/<port>.lock` discovery file Claude Code scans to find us.
// Shape pinned in PROTOCOL.md (captured from the VS Code extension).

use std::path::{Path, PathBuf};

use serde_json::json;

/// `~/.claude/ide`, created `0700` if missing.
fn ide_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let dir = home.join(".claude").join("ide");
    std::fs::create_dir_all(&dir).ok()?;
    set_mode(&dir, 0o700);
    Some(dir)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}
#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) {}

/// Write `<port>.lock` (mode `0600`) advertising this window. Returns the file path so
/// the caller can remove it on shutdown.
pub fn write_lock(port: u16, workspace: &Path, token: &str) -> Option<PathBuf> {
    let dir = ide_dir()?;
    let path = dir.join(format!("{port}.lock"));
    let body = json!({
        "pid": std::process::id(),
        "workspaceFolders": [workspace.to_string_lossy()],
        "ideName": "Aether",
        "transport": "ws",
        "runningInWindows": cfg!(windows),
        "authToken": token,
    });
    std::fs::write(&path, serde_json::to_vec(&body).ok()?).ok()?;
    set_mode(&path, 0o600);
    Some(path)
}

/// Remove `<port>.lock` files whose owning process is dead — stale entries from
/// crashed/force-quit/superseded windows otherwise make Claude Code try a dead port and
/// fail to connect (-32000), especially when two locks share a workspace. Run on startup.
pub fn clean_stale_locks() {
    let Some(dir) = ide_dir() else { return };
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    for e in entries.flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some("lock") {
            continue;
        }
        let pid = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| v.get("pid").and_then(|p| p.as_u64()));
        // Remove malformed locks (no pid) and locks for dead processes.
        if pid.map_or(true, |p| !pid_alive(p)) {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Whether `pid` is a live process (best-effort; never reaps a live window's lock).
fn pid_alive(pid: u64) -> bool {
    #[cfg(unix)]
    {
        // `kill -0` exits 0 if the process exists and is signalable, non-zero otherwise.
        std::process::Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .status()
            .map_or(true, |s| s.success()) // on spawn failure, keep the lock (don't reap)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

/// A non-cryptographic localhost token (the real gate is the 0600 file + loopback bind).
/// Avoids pulling in a rand/uuid dependency; mirrors `ptyhost::daemon::gen_token`.
pub fn gen_token() -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .hash(&mut h);
    std::process::id().hash(&mut h);
    let a = h.finish();
    a.hash(&mut h);
    let b = h.finish();
    format!("{a:016x}-{b:016x}")
}
