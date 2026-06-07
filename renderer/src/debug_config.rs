// Debug launch configurations. Adapter-agnostic: a config names a debug "type"
// (python, node, coreclr, lldb, …) which maps to an external Debug Adapter binary
// via a small built-in table, plus a `request` (launch/attach) and the raw argument
// object that is forwarded verbatim to the adapter. Read from a VSCode-style
// launch.json (`.vscode/launch.json` then `.aether/launch.json`); if none exists,
// sensible entries are synthesized from the active file's extension.

use std::path::Path;

use serde_json::{json, Value};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Launch,
    Attach,
}

/// How to reach the debug adapter process (stdio).
#[derive(Clone)]
pub struct AdapterSpec {
    pub program: String,
    pub args: Vec<String>,
    /// A hint shown if the adapter can't be spawned (e.g. "pip install debugpy").
    pub install_hint: &'static str,
}

#[derive(Clone)]
pub struct LaunchConfig {
    pub name: String,
    pub request: Request,
    pub adapter: AdapterSpec,
    /// The full launch/attach arguments forwarded to the adapter (everything in the
    /// config except `name`/`type`/`request`, with a few defaults filled in).
    pub args: Value,
}

/// Pick a Python interpreter that actually has debugpy installed, so attach/launch
/// works whether it's in a venv, `python`, or `python3`. Falls back to `python3`
/// (the adapter then exits and the install hint is shown).
fn resolve_python() -> String {
    let mut candidates: Vec<String> = Vec::new();
    if let Ok(venv) = std::env::var("VIRTUAL_ENV") {
        candidates.push(format!("{venv}/bin/python"));
    }
    candidates.push("python3".into());
    candidates.push("python".into());
    for c in &candidates {
        let ok = std::process::Command::new(c)
            .args(["-c", "import debugpy"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            return c.clone();
        }
    }
    "python3".into()
}

/// Resolve a debug-adapter binary by name. Prefers a copy Aether manages under
/// `~/.aether/<name>/<name>` (so GUI launches with a minimal PATH still find it),
/// then falls back to the bare name on PATH.
fn resolve_managed(name: &str) -> String {
    if let Some(dir) = crate::settings::config_dir() {
        let p = dir.join(name).join(name);
        if p.is_file() {
            return p.to_string_lossy().into_owned();
        }
    }
    name.to_string()
}

/// Map a VSCode debug `type` to the adapter binary that speaks DAP for it.
fn adapter_for(debug_type: &str) -> Option<AdapterSpec> {
    let t = debug_type.to_lowercase();
    let spec = match t.as_str() {
        "python" | "debugpy" => AdapterSpec {
            program: resolve_python(),
            args: vec!["-m".into(), "debugpy.adapter".into()],
            install_hint: "The Python debugger isn't installed for this interpreter.\n\nInstall it:  python3 -m pip install debugpy\n(or activate a venv that has debugpy before launching Aether)",
        },
        "coreclr" | "netcoredbg" | "dotnet" => AdapterSpec {
            program: resolve_managed("netcoredbg"),
            args: vec!["--interpreter=vscode".into()],
            install_hint: "The .NET debugger (netcoredbg) isn't installed.\n\nGet it from https://github.com/Samsung/netcoredbg/releases and unpack it to ~/.aether/netcoredbg/, or put it on PATH.",
        },
        "lldb" | "codelldb" => AdapterSpec {
            program: "lldb-dap".into(),
            args: vec![],
            install_hint: "Install lldb-dap (ships with recent LLVM) and put it on PATH.",
        },
        "node" | "pwa-node" => AdapterSpec {
            program: "js-debug-adapter".into(),
            args: vec![],
            install_hint: "Install vscode-js-debug's js-debug-adapter and put it on PATH.",
        },
        _ => return None,
    };
    Some(spec)
}

/// The Python (debugpy) adapter — used for attach-by-PID where there's no config.
pub fn python_adapter() -> AdapterSpec {
    adapter_for("python").expect("python adapter is defined")
}

/// Build a LaunchConfig from one `configurations[]` entry of a launch.json.
fn from_json(v: &Value, ws: &Path) -> Option<LaunchConfig> {
    let obj = v.as_object()?;
    let name = obj.get("name").and_then(|n| n.as_str()).unwrap_or("Debug").to_string();
    let ty = obj.get("type").and_then(|t| t.as_str())?;
    let adapter = adapter_for(ty)?;
    let request = match obj.get("request").and_then(|r| r.as_str()) {
        Some("attach") => Request::Attach,
        _ => Request::Launch,
    };
    // Forward everything except the editor-only keys; fill cwd default.
    let mut args = serde_json::Map::new();
    for (k, val) in obj {
        if k == "name" || k == "type" || k == "request" {
            continue;
        }
        args.insert(k.clone(), substitute(val, ws));
    }
    if request == Request::Launch && !args.contains_key("cwd") {
        args.insert("cwd".into(), json!(ws.to_string_lossy()));
    }
    Some(LaunchConfig { name, request, adapter, args: Value::Object(args) })
}

/// Expand the common `${workspaceFolder}` / `${file}` variables in string values.
fn substitute(v: &Value, ws: &Path) -> Value {
    match v {
        Value::String(s) => Value::String(s.replace("${workspaceFolder}", &ws.to_string_lossy())),
        Value::Array(a) => Value::Array(a.iter().map(|x| substitute(x, ws)).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), substitute(x, ws))).collect()),
        other => other.clone(),
    }
}

/// Read launch configs for `ws`, falling back to entries synthesized from the
/// active file. Returns an empty Vec only if nothing applies.
pub fn load(ws: &Path, active_file: Option<&Path>) -> Vec<LaunchConfig> {
    for rel in [".vscode/launch.json", ".aether/launch.json"] {
        let path = ws.join(rel);
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let Ok(v) = serde_json::from_str::<Value>(&crate::settings::strip_jsonc(&text)) else { continue };
        if let Some(configs) = v.get("configurations").and_then(|c| c.as_array()) {
            let out: Vec<LaunchConfig> = configs.iter().filter_map(|c| from_json(c, ws)).collect();
            if !out.is_empty() {
                return out;
            }
        }
    }
    synthesize(ws, active_file)
}

/// Default configs when there's no launch.json — derived from the active file.
fn synthesize(ws: &Path, active_file: Option<&Path>) -> Vec<LaunchConfig> {
    let Some(file) = active_file else { return Vec::new() };
    let ext = file.extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext {
        "py" => {
            let adapter = adapter_for("python").unwrap();
            vec![
                LaunchConfig {
                    name: "Python: Launch current file".into(),
                    request: Request::Launch,
                    adapter: adapter.clone(),
                    args: json!({
                        "program": file.to_string_lossy(),
                        "cwd": ws.to_string_lossy(),
                        "console": "internalConsole",
                        "stopOnEntry": false,
                        "justMyCode": true,
                    }),
                },
                LaunchConfig {
                    name: "Python: Attach (127.0.0.1:5678)".into(),
                    request: Request::Attach,
                    adapter,
                    args: json!({ "connect": { "host": "127.0.0.1", "port": 5678 } }),
                },
            ]
        }
        _ => Vec::new(),
    }
}
