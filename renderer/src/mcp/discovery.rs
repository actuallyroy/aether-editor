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
