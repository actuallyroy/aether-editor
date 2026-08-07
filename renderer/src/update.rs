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
pub fn install_apt_async(_tx: Sender<WorkerMsg>) {}

/// True if installed under Program Files (the Inno Setup installer's target) —
/// an admin-owned location the in-app self-replace can't overwrite. Such installs
/// update by re-running the installer (which elevates via UAC). A portable exe
/// living elsewhere returns false and self-updates normally.
#[cfg(windows)]
pub fn is_program_files_install() -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    let exe = exe.to_string_lossy().to_lowercase();
    ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .filter(|p| !p.is_empty())
        .any(|pf| exe.starts_with(&pf.to_lowercase()))
}
#[cfg(not(windows))]
pub fn is_program_files_install() -> bool {
    false
}

/// Download the latest Windows installer and run it. Inno Setup's admin manifest
/// triggers the UAC consent prompt; with CloseApplications/RestartApplications
/// (Inno defaults) the Restart Manager closes this running instance, upgrades in
/// place, and relaunches it. Sends `UpdateDone { ok:false }` only if the launch
/// itself fails (on success the installer takes over and restarts us).
#[cfg(windows)]
pub fn install_windows_async(tx: Sender<WorkerMsg>) {
    std::thread::spawn(move || {
        if download_and_run_installer().is_err() {
            let _ = tx.send(WorkerMsg::UpdateDone { ok: false });
        }
    });
}
#[cfg(windows)]
fn download_and_run_installer() -> Result<(), Box<dyn std::error::Error>> {
    let url = format!(
        "https://github.com/{OWNER}/{NAME}/releases/latest/download/aether-windows-setup-x86_64.exe"
    );
    let mut reader = ureq::get(&url).call()?.into_reader();
    let tmp = std::env::temp_dir().join("aether-setup.exe");
    let mut f = std::fs::File::create(&tmp)?;
    std::io::copy(&mut reader, &mut f)?;
    drop(f);
    // /SILENT: progress bar, no wizard clicks. The installer self-elevates (UAC).
    // /FORCECLOSEAPPLICATIONS: this running instance locks aether.exe and doesn't
    // answer Restart Manager close requests — force-close, upgrade, relaunch.
    std::process::Command::new(&tmp)
        .args(["/SILENT", "/FORCECLOSEAPPLICATIONS"])
        .spawn()?;
    Ok(())
}

/// True when running from an installed Aether.app bundle — the self-updater's
/// bare-binary replace would leave bundled resources (ext-host/) stale, so the
/// whole .app is refreshed from the release DMG instead.
#[cfg(target_os = "macos")]
pub fn is_app_bundle_install() -> bool {
    // Check the CANONICAL binary, not `current_exe()` — a window opened via Open Folder
    // runs from a per-folder launcher bundle (`.../launchers/<id>/<Folder>.app/...`), whose
    // path never contains "/Aether.app/", so this used to say "not a bundle install" and
    // fall through to a plain self-replace that only patched that one hardlinked copy,
    // leaving the real install (and every other project window) on the old version
    // (#update-only-updates-one-instance).
    crate::macos_launcher::canonical_exe()
        .to_string_lossy()
        .contains("/Aether.app/Contents/MacOS/")
}
#[cfg(not(target_os = "macos"))]
pub fn is_app_bundle_install() -> bool {
    false
}

/// True if Aether was installed by a system installer/package manager whose
/// binary we can't self-replace — update through the manager instead.
pub fn is_managed_install() -> bool {
    is_apt_install() || is_program_files_install() || is_app_bundle_install()
}

/// Download the latest DMG and swap the installed Aether.app with the fresh
/// bundle (binary + ext-host + Info.plist together). Sends `UpdateDone`; on
/// success the app offers a restart which re-execs the new binary.
#[cfg(target_os = "macos")]
pub fn install_dmg_async(tx: Sender<WorkerMsg>) {
    std::thread::spawn(move || {
        let ok = install_dmg().is_ok();
        let _ = tx.send(WorkerMsg::UpdateDone { ok });
    });
}
#[cfg(target_os = "macos")]
fn install_dmg() -> Result<(), Box<dyn std::error::Error>> {
    let url = format!(
        "https://github.com/{OWNER}/{NAME}/releases/latest/download/Aether-macos-arm64.dmg"
    );
    let mut reader = ureq::get(&url).call()?.into_reader();
    let tmp = std::env::temp_dir().join("aether-update.dmg");
    let mut f = std::fs::File::create(&tmp)?;
    std::io::copy(&mut reader, &mut f)?;
    drop(f);
    // The installed app's location (…/Aether.app/Contents/MacOS/aether → the .app). Use
    // the canonical binary, not `current_exe()` — from a per-folder launcher-bundle window
    // that would resolve to the throwaway `<Folder>.app`, not the real Aether.app.
    let exe = crate::macos_launcher::canonical_exe();
    let app = exe
        .ancestors()
        .find(|p| p.extension().map_or(false, |e| e == "app"))
        .ok_or("not inside an .app bundle")?
        .to_path_buf();
    let mount = std::env::temp_dir().join("aether-update-mnt");
    let _ = std::fs::create_dir_all(&mount);
    let ok = std::process::Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-quiet", "-mountpoint"])
        .arg(&mount)
        .arg(&tmp)
        .status()?
        .success();
    if !ok {
        return Err("hdiutil attach failed".into());
    }
    // Replace the bundle. Removing a RUNNING app's files is fine on APFS — the
    // running process keeps its open inodes; the restart picks up the new one.
    let src = mount.join("Aether.app");
    let result: Result<(), Box<dyn std::error::Error>> = (|| {
        if !src.exists() {
            return Err("DMG has no Aether.app".into());
        }
        std::fs::remove_dir_all(&app)?;
        let status = std::process::Command::new("cp").arg("-R").arg(&src).arg(&app).status()?;
        if !status.success() {
            return Err("cp -R failed".into());
        }
        Ok(())
    })();
    let _ = std::process::Command::new("hdiutil").args(["detach", "-quiet"]).arg(&mount).status();
    let _ = std::fs::remove_file(&tmp);
    if result.is_ok() {
        // Every other project window's launcher bundle hardlinks the OLD binary until it
        // happens to reopen through Open Folder — relink them now so they're current the
        // next time they restart too, not just this window (#update-only-updates-one-instance).
        crate::macos_launcher::relink_all_launchers(&exe);
    }
    result
}

/// Update a managed install the right way for the platform: apt+pkexec on Linux,
/// re-run the installer (UAC) on Windows. Falls back to self-update elsewhere.
pub fn install_managed_async(tx: Sender<WorkerMsg>) {
    #[cfg(target_os = "linux")]
    {
        install_apt_async(tx);
    }
    #[cfg(windows)]
    {
        install_windows_async(tx);
    }
    #[cfg(target_os = "macos")]
    {
        if is_app_bundle_install() {
            install_dmg_async(tx);
        } else {
            install_async(tx); // portable binary: plain self-replace
        }
    }
    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    {
        install_async(tx);
    }
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
