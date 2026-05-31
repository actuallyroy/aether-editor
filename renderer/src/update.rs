// Auto-update against GitHub Releases. Checks the latest release on a background
// thread; if newer than the running build, downloads the matching binary and
// replaces the running executable in place (via the `self_update` crate), then
// the app offers to restart. Assets are named `nova-<os>-x86_64[.exe]`, so the
// target string below matches our release asset naming.

use std::sync::mpsc::Sender;

use crate::marketplace::WorkerMsg;

const OWNER: &str = "actuallyroy";
const NAME: &str = "nova-editor";

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Substring our release assets contain for this platform (`nova-windows-x86_64.exe`).
fn target() -> &'static str {
    if cfg!(windows) {
        "windows-x86_64"
    } else if cfg!(target_os = "macos") {
        "macos-x86_64"
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

fn install() -> Result<(), Box<dyn std::error::Error>> {
    self_update::backends::github::Update::configure()
        .repo_owner(OWNER)
        .repo_name(NAME)
        .bin_name("nova")
        .target(target())
        .show_download_progress(false)
        .no_confirm(true)
        .current_version(current_version())
        .build()?
        .update()?;
    Ok(())
}
