// Auto-update against GitHub Releases. Checks the latest release on a background
// thread; if newer than the running build, downloads the matching binary and
// replaces the running executable in place (via the `self_update` crate), then
// the app offers to restart. Assets are named `aether-<os>-x86_64[.exe]`, so the
// target string below matches our release asset naming.

use std::sync::mpsc::Sender;

use crate::marketplace::WorkerMsg;

const OWNER: &str = "actuallyroy";
const NAME: &str = "aether-editor";

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Substring our release assets contain for this platform + arch, matching the
/// workflow's asset names (`aether-windows-x86_64.exe`, `aether-macos-arm64`, …).
fn target() -> &'static str {
    if cfg!(windows) {
        "windows-x86_64"
    } else if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") {
            "macos-arm64"
        } else {
            "macos-x86_64"
        }
    } else {
        "linux-x86_64"
    }
}

/// Background-check for a newer release; sends `UpdateAvailable` if one exists.
/// When `manual` (user-triggered), also sends `UpdateNone` if already up to date,
/// so the UI can confirm the check ran.
pub fn check_async(tx: Sender<WorkerMsg>, manual: bool) {
    std::thread::spawn(move || match latest_newer() {
        Some(version) => {
            let _ = tx.send(WorkerMsg::UpdateAvailable { version });
        }
        None if manual => {
            let _ = tx.send(WorkerMsg::UpdateNone);
        }
        None => {}
    });
}

/// Re-check for a newer release every `interval` on a background thread, sending
/// `UpdateAvailable` each time one is found. Runs for the life of the process.
pub fn check_periodic(tx: Sender<WorkerMsg>, interval: std::time::Duration) {
    std::thread::spawn(move || loop {
        std::thread::sleep(interval);
        if let Some(version) = latest_newer() {
            let _ = tx.send(WorkerMsg::UpdateAvailable { version });
        }
    });
}

fn latest_newer() -> Option<String> {
    let releases = self_update::backends::github::ReleaseList::configure()
        .repo_owner(OWNER)
        .repo_name(NAME)
        .build()
        .ok()?
        .fetch()
        .ok()?;
    let latest = releases.first()?;
    let v = latest.version.trim_start_matches('v').to_string();
    match self_update::version::bump_is_greater(current_version(), &v) {
        Ok(true) => Some(v),
        _ => None,
    }
}

/// Background-download + replace the running binary; sends `UpdateDone { ok }`.
pub fn install_async(tx: Sender<WorkerMsg>) {
    std::thread::spawn(move || {
        let ok = install().is_ok();
        let _ = tx.send(WorkerMsg::UpdateDone { ok });
    });
}

/// True if Aether was installed by the system package manager (Linux, dpkg/apt)
/// — i.e. the running binary is managed by dpkg. Such installs live in a
/// root-owned location the in-app self-replace can't (and shouldn't) overwrite;
/// they must upgrade through apt instead. See `install_apt_async`.
#[cfg(target_os = "linux")]
pub fn is_apt_install() -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    // dpkg knows the path iff it was installed from our .deb / APT repo.
    std::process::Command::new("dpkg")
        .arg("-S")
        .arg(&exe)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
#[cfg(not(target_os = "linux"))]
pub fn is_apt_install() -> bool {
    false
}

/// Upgrade an apt-managed install through the package manager, asking the user
/// for authorization via PolicyKit (`pkexec` shows a graphical password prompt).
/// Refreshes only Aether's APT source (not every repo) then upgrades the package.
/// Sends `UpdateDone { ok }`; on success the app re-execs the new binary.
#[cfg(target_os = "linux")]
pub fn install_apt_async(tx: Sender<WorkerMsg>) {
    std::thread::spawn(move || {
        // Scope `apt-get update` to Aether's own list so we don't refresh (or fail
        // on) unrelated third-party repos, then upgrade just the aether package.
        let script = "set -e; \
            apt-get update \
              -o Dir::Etc::sourcelist=sources.list.d/aether.list \
              -o Dir::Etc::sourceparts=/dev/null \
              -o APT::Get::List-Cleanup=0; \
            apt-get install -y --only-upgrade aether";
        let ok = std::process::Command::new("pkexec")
            .args(["sh", "-c", script])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        let _ = tx.send(WorkerMsg::UpdateDone { ok });
    });
}
#[cfg(not(target_os = "linux"))]
pub fn install_apt_async(tx: Sender<WorkerMsg>) {
    install_async(tx);
}

fn install() -> Result<(), Box<dyn std::error::Error>> {
    self_update::backends::github::Update::configure()
        .repo_owner(OWNER)
        .repo_name(NAME)
        .bin_name("aether")
        .target(target())
        .show_download_progress(false)
        .no_confirm(true)
        .current_version(current_version())
        .build()?
        .update()?;
    Ok(())
}
