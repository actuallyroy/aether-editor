//! Per-folder Dock identity on macOS (CrossOver-style launcher bundles).
//!
//! The Dock/menu-bar name and icon of a running process come from the `.app` bundle that
//! *launched* it — read once at launch and not changeable at runtime. So to show a folder's
//! own name (and, later, icon) in the Dock, we launch Aether through a generated throwaway
//! `.app` whose `Info.plist` carries that folder's name. The bundle hardlinks the real
//! binary into `Contents/MacOS/`; macOS then reports the *bundle's* `CFBundleName` as the
//! process's LaunchServices/Dock name (verified: a hardlinked binary inside the bundle
//! inherits the enclosing bundle's identity).
//!
//! Lifecycle: a bundle is generated on Open Folder, removed on clean window close, and a
//! startup sweep garbage-collects any bundle with no live holder process (crash-safe).

#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};
use std::process::Command;

/// `~/Library/Application Support/Aether/launchers` — where generated bundles live.
fn launchers_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join("Library/Application Support/Aether/launchers")
}

/// Stable per-folder id (so reopening the same folder reuses one bundle/identifier).
fn bundle_id(folder: &Path) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    folder.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// What we stash inside a bundle so the launched process knows which folder to open and
/// which real binary to use when spawning further windows.
#[derive(serde::Serialize, serde::Deserialize)]
struct LaunchInfo {
    folder: String,
    real_exe: String,
}

fn launch_info_path(bundle: &Path) -> PathBuf {
    bundle.join("Contents/Resources/launch.json")
}

/// If this process was launched from one of our generated bundles, return its path.
/// Layout is `…/launchers/<id>/<FolderName>.app/Contents/MacOS/aether` — the `.app` is
/// named after the folder (the Dock tile label is the bundle's *filename*, not just its
/// `CFBundleName`), and the `<id>` parent keeps same-basename folders from colliding.
pub fn current_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let app = exe.parent()?.parent()?.parent()?; // MacOS → Contents → <name>.app
    let is_app = app.extension().map_or(false, |e| e == "app");
    // <name>.app → <id> → launchers
    let in_launchers = app
        .parent()
        .and_then(|id| id.parent())
        .and_then(|p| p.file_name())
        .map_or(false, |n| n == "launchers");
    if in_launchers && is_app {
        Some(app.to_path_buf())
    } else {
        None
    }
}

/// Read the folder this launcher bundle should open, and the real binary to spawn from.
fn read_launch(bundle: &Path) -> Option<(PathBuf, PathBuf)> {
    let raw = std::fs::read_to_string(launch_info_path(bundle)).ok()?;
    let info: LaunchInfo = serde_json::from_str(&raw).ok()?;
    Some((PathBuf::from(info.folder), PathBuf::from(info.real_exe)))
}

/// Called at startup. If we were launched from a generated bundle, returns the folder to
/// open. Otherwise returns None (canonical run).
pub fn startup_workspace() -> Option<PathBuf> {
    let bundle = current_bundle()?;
    let (folder, _real) = read_launch(&bundle)?;
    Some(folder)
}

/// The canonical (non-bundled) Aether binary to hardlink and to spawn folder-less windows
/// from. When running inside a launcher bundle, this is the `real_exe` recorded there;
/// otherwise the current executable.
pub fn canonical_exe() -> PathBuf {
    if let Some(bundle) = current_bundle() {
        if let Some((_folder, real)) = read_launch(&bundle) {
            return real;
        }
    }
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("aether"))
}

/// Generate (or refresh) the launcher bundle for `folder` and return its path.
fn ensure_bundle(folder: &Path, real_exe: &Path) -> std::io::Result<PathBuf> {
    let project = folder
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Aether".to_string());
    let name = format!("{} - Aether", project);
    // The Dock tile label is the `.app`'s filename, so name the bundle after the folder;
    // a per-folder `<id>` subdir disambiguates folders that share a basename.
    let dir = launchers_dir().join(bundle_id(folder));
    let bundle = dir.join(format!("{}.app", fs_safe(&name)));
    let macos = bundle.join("Contents/MacOS");
    let resources = bundle.join("Contents/Resources");
    std::fs::create_dir_all(&macos)?;
    std::fs::create_dir_all(&resources)?;

    // Hardlink the real binary in (same inode, near-free), but only when stale — a no-op
    // re-link makes macOS see a "new" executable and re-fires the TCC folder-access prompt.
    let exe_dst = macos.join("aether");
    relink_if_stale(real_exe, &exe_dst)?;

    // Reuse an existing bundle as-is. Regenerating its Info.plist/icon on every open would
    // make macOS treat it as a brand-new app and re-prompt for Desktop/Documents access;
    // building the identity-bearing files exactly once keeps that grant sticky.
    if bundle.join("Contents/Info.plist").exists() {
        write_launch(&bundle, folder, real_exe); // keep folder/real_exe current (TCC-neutral)
        return Ok(bundle);
    }

    // Icon: synchronously seed the canonical app icon (a fast file copy — release builds
    // show the Aether icon immediately on first launch). The per-folder letter-badge icon
    // is rendered on a background thread by the launched window (see `ensure_icon_async`),
    // so generating it never blocks Open Folder; it's ready by the next launch.
    if let Some(src) = real_exe
        .parent()
        .and_then(|p| p.parent())
        .map(|contents| contents.join("Resources/icon.icns"))
        .filter(|p| p.exists())
    {
        let _ = std::fs::copy(&src, resources.join("icon.icns"));
    }
    // Always reference icon.icns — the background pass will have written it by next launch.
    let icon_key = "\t<key>CFBundleIconFile</key><string>icon</string>\n".to_string();

    let plist = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n<dict>\n\
         \t<key>CFBundleName</key><string>{name}</string>\n\
         \t<key>CFBundleDisplayName</key><string>{name}</string>\n\
         \t<key>CFBundleExecutable</key><string>aether</string>\n\
         \t<key>CFBundleIdentifier</key><string>dev.aether.launcher.{id}</string>\n\
         {icon}\
         \t<key>CFBundlePackageType</key><string>APPL</string>\n\
         \t<key>CFBundleInfoDictionaryVersion</key><string>6.0</string>\n\
         \t<key>CFBundleShortVersionString</key><string>{ver}</string>\n\
         \t<key>LSMinimumSystemVersion</key><string>11.0</string>\n\
         \t<key>NSHighResolutionCapable</key><true/>\n\
         </dict>\n</plist>\n",
        name = xml_escape(&name),
        id = bundle_id(folder),
        icon = icon_key,
        ver = env!("CARGO_PKG_VERSION"),
    );
    std::fs::write(bundle.join("Contents/Info.plist"), plist)?;
    write_launch(&bundle, folder, real_exe);
    Ok(bundle)
}

/// (Re)write the bundle's sidecar mapping it back to its folder + the real binary. Cheap
/// and TCC-neutral (not part of the app's code identity), so safe to refresh every open.
fn write_launch(bundle: &Path, folder: &Path, real_exe: &Path) {
    let info = LaunchInfo {
        folder: folder.to_string_lossy().into_owned(),
        real_exe: real_exe.to_string_lossy().into_owned(),
    };
    let _ = std::fs::write(launch_info_path(bundle), serde_json::to_string(&info).unwrap_or_default());
}

/// Hardlink `real_exe` into the bundle at `dst`, but skip if `dst` already points at the
/// same inode (re-linking an identical binary would look like a new app to TCC). Falls back
/// to a copy across volumes.
fn relink_if_stale(real_exe: &Path, dst: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    if let (Ok(a), Ok(b)) = (std::fs::metadata(dst), std::fs::metadata(real_exe)) {
        if a.dev() == b.dev() && a.ino() == b.ino() {
            return Ok(()); // already the same file — leave it (no TCC churn)
        }
    }
    let _ = std::fs::remove_file(dst);
    if std::fs::hard_link(real_exe, dst).is_err() {
        std::fs::copy(real_exe, dst)?;
    }
    Ok(())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Make a folder name safe as a `.app` filename (the Finder/Dock show `:` as `/`).
fn fs_safe(s: &str) -> String {
    s.replace([':', '/'], "-")
}

/// The Aether logo (single source of truth, shared with the in-app window icon).
const LOGO_SVG: &str = include_str!("../assets/logo.svg");

/// (pixel size, iconset filename) pairs `iconutil` expects for a full `.icns`.
const ICON_SIZES: &[(u32, &str)] = &[
    (16, "icon_16x16.png"),
    (32, "icon_16x16@2x.png"),
    (32, "icon_32x32.png"),
    (64, "icon_32x32@2x.png"),
    (128, "icon_128x128.png"),
    (256, "icon_128x128@2x.png"),
    (256, "icon_256x256.png"),
    (512, "icon_256x256@2x.png"),
    (512, "icon_512x512.png"),
    (1024, "icon_512x512@2x.png"),
];

/// Compose the badge SVG: the Aether nova plus a bottom-right circular badge with `letter`
/// (reusing the logo's `novaGradient` for the badge fill).
fn compose_svg(letter: char) -> String {
    let badge = format!(
        "<g>\
         <circle cx=\"384\" cy=\"384\" r=\"114\" fill=\"#0D1117\"/>\
         <circle cx=\"384\" cy=\"384\" r=\"96\" fill=\"url(#novaGradient)\"/>\
         <text x=\"384\" y=\"428\" text-anchor=\"middle\" \
         font-family=\"Helvetica Neue, Helvetica, Arial, sans-serif\" font-size=\"130\" \
         font-weight=\"700\" fill=\"#FFFFFF\">{}</text>\
         </g></svg>",
        xml_escape(&letter.to_string())
    );
    LOGO_SVG.replacen("</svg>", &badge, 1)
}

/// Render the per-folder icon to `out` (an `icon.icns`). Returns false on any failure
/// (caller falls back to the canonical/generic icon).
fn build_icon(letter: char, out: &Path) -> bool {
    use resvg::{tiny_skia, usvg};
    let svg = compose_svg(letter);
    let mut opt = usvg::Options::default();
    // Load just the one font the badge needs — scanning every installed font is far slower
    // and unnecessary for a single glyph. Fall back to a full scan only if none are found.
    {
        let db = opt.fontdb_mut();
        for f in [
            "/System/Library/Fonts/Helvetica.ttc",
            "/System/Library/Fonts/Supplemental/Arial.ttf",
            "/Library/Fonts/Arial.ttf",
        ] {
            if Path::new(f).exists() {
                let _ = db.load_font_file(f);
            }
        }
        if db.is_empty() {
            db.load_system_fonts();
        }
    }
    let Ok(tree) = usvg::Tree::from_str(&svg, &opt) else {
        return false;
    };
    let base = tree.size().width().max(1.0);
    let work = out.with_extension("iconset");
    if std::fs::create_dir_all(&work).is_err() {
        return false;
    }
    for (sz, fname) in ICON_SIZES {
        let Some(mut pm) = tiny_skia::Pixmap::new(*sz, *sz) else {
            continue;
        };
        let scale = *sz as f32 / base;
        resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pm.as_mut());
        let _ = pm.save_png(work.join(fname));
    }
    let ok = Command::new("iconutil")
        .arg("-c")
        .arg("icns")
        .arg(&work)
        .arg("-o")
        .arg(out)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let _ = std::fs::remove_dir_all(&work);
    ok
}

/// Open `folder` in a NEW window whose Dock identity is the folder's own bundle. Returns
/// true if the bundled process was spawned (caller may then close a blank folder-less
/// window); false if we couldn't build/launch it (caller should fall back to in-place).
pub fn open_folder_windowed(folder: &Path) -> bool {
    let real = canonical_exe();
    let Ok(bundle) = ensure_bundle(folder, &real) else {
        return false;
    };
    // `open -n` launches a fresh instance of the (unregistered) bundle; the launched
    // process reads its folder back from launch.json via startup_workspace().
    Command::new("open").arg("-n").arg(&bundle).status().map(|s| s.success()).unwrap_or(false)
}

/// If this process is running inside a launcher bundle, render its per-folder letter-badge
/// icon on a background thread (once — guarded by a marker file). Kept off the Open Folder
/// path so launching never waits on `iconutil`; the icon is in place for the next launch.
pub fn ensure_icon_async() {
    let Some(bundle) = current_bundle() else {
        return;
    };
    let marker = bundle.join("Contents/Resources/.icon-built");
    if marker.exists() {
        return;
    }
    std::thread::spawn(move || {
        let Some((folder, _real)) = read_launch(&bundle) else {
            return;
        };
        let letter = folder
            .file_name()
            .and_then(|n| n.to_string_lossy().chars().next())
            .map(|c| c.to_uppercase().next().unwrap_or(c))
            .unwrap_or('A');
        if build_icon(letter, &bundle.join("Contents/Resources/icon.icns")) {
            let _ = std::fs::write(&marker, b"1");
        }
    });
}

/// Garbage-collect launcher bundles whose target folder no longer exists. Bundles are kept
/// persistently (one per folder) so the one-time TCC folder-access grant sticks across
/// reopens; we only reap bundles for folders that have been moved/deleted, never live ones.
/// Each entry in `launchers/` is an `<id>` dir holding one `<name>.app`.
pub fn sweep_stale() {
    let Ok(entries) = std::fs::read_dir(launchers_dir()) else {
        return;
    };
    for e in entries.flatten() {
        let id_dir = e.path();
        if !id_dir.is_dir() {
            continue;
        }
        let app = std::fs::read_dir(&id_dir)
            .ok()
            .and_then(|mut it| it.find_map(|x| x.ok().map(|x| x.path())))
            .filter(|p| p.extension().map_or(false, |x| x == "app"));
        let folder_gone = match app.as_deref().and_then(read_launch) {
            Some((folder, _)) => !folder.is_dir(),
            None => true, // malformed/empty bundle dir — reap it
        };
        if folder_gone {
            let _ = std::fs::remove_dir_all(&id_dir);
        }
    }
}
