// Aether extension host — entry point. Spawned by aether as:
//   node ext-host/main.js <port> <token>
// Connects to aether over TCP, provides the `vscode` shim to extensions, and bridges
// the two directions of PROTOCOL.md.
'use strict';

const net = require('net');
const path = require('path');
const { pathToFileURL } = require('url');
const { Rpc } = require('./rpc');
const { createVscode } = require('./vscode-shim');
const { installVscodeModule, activateExtension, deactivateExtension } = require('./activation');

// ESM-bundled extensions (esbuild `"type": "module"` output) use `import ... from
// "vscode"`, which the plain `Module._load` patch below can't intercept — that
// only covers CommonJS `require()`. Register a loader hook so ESM imports of
// "vscode" resolve too (see esm-loader.mjs for how it stays bridged to the same
// live shim singleton). Must happen before any extension activates.
try {
  require('module').register(pathToFileURL(path.join(__dirname, 'esm-loader.mjs')).href, pathToFileURL(__filename).href);
} catch (e) {
  console.error('[ext-host] ESM loader hook registration failed (ESM-bundled extensions, e.g. Prettier, may not activate):', e.message);
}

const port = parseInt(process.argv[2], 10);
const token = process.argv[3] || '';

if (!port) {
  console.error('[ext-host] usage: node main.js <port> <token>');
  process.exit(2);
}

// Block extension writes to ~/.claude/ide: the Claude Code extension advertises its
// own 12-tool MCP server there with the HOST's identity ("Aether" + the GUI pid),
// which collides with aether's native 22-tool lock — external clients (claude CLI
// /ide picker, aether --mcp proxy) then route to the wrong, poorer server. Aether
// writes its own lock natively; extensions must not.
{
  const fs = require('fs');
  const path = require('path');
  const os = require('os');
  const ideDir = path.join(os.homedir(), '.claude', 'ide') + path.sep;
  const blocked = (p) => {
    try { return typeof p === 'string' && path.resolve(p).startsWith(ideDir.slice(0, -1)); } catch (_) { return false; }
  };
  for (const name of ['writeFileSync', 'writeFile', 'createWriteStream']) {
    const orig = fs[name];
    fs[name] = function (p, ...rest) {
      if (blocked(p)) {
        console.error(`[ext-host] blocked extension ${name} to ${p} (IDE locks are aether-owned)`);
        if (name === 'writeFile' && typeof rest[rest.length - 1] === 'function') return rest[rest.length - 1](null);
        if (name === 'createWriteStream') return new (require('stream').Writable)({ write(c, e, cb) { cb(); } });
        return undefined;
      }
      return orig.call(this, p, ...rest);
    };
  }
  const origPromises = fs.promises.writeFile;
  fs.promises.writeFile = function (p, ...rest) {
    if (blocked(p)) {
      console.error(`[ext-host] blocked extension promises.writeFile to ${p} (IDE locks are aether-owned)`);
      return Promise.resolve();
    }
    return origPromises.call(this, p, ...rest);
  };
}

const rpc = new Rpc();
const { vscode, dispatch } = createVscode(rpc);
installVscodeModule(vscode);

const hostLog = (level, message) => rpc.notify('log', { level, message });

let workspaceRoot = null;

// ---- inbound handlers (aether -> host) ----
rpc.on_method('host/init', (p) => {
  workspaceRoot = p.root;
  // Follow the workspace: extensions spawn helpers (e.g. the claude binary) that
  // inherit our cwd.
  try { if (p.root) process.chdir(p.root); } catch (_) {}
  // Bundled ripgrep: synthesize a VSCode-shaped appRoot
  // (<appRoot>/node_modules/@vscode/ripgrep/bin/rg) pointing at aether's rg, so
  // extensions that resolve VSCode's bundled ripgrep (Todo Tree etc.) find it.
  if (p.rgPath) {
    try {
      const fs = require('fs');
      const path = require('path');
      const os = require('os');
      const appRoot = path.join(os.homedir(), '.aether', 'approot');
      const binDir = path.join(appRoot, 'node_modules', '@vscode', 'ripgrep', 'bin');
      const dest = path.join(binDir, process.platform === 'win32' ? 'rg.exe' : 'rg');
      fs.mkdirSync(binDir, { recursive: true });
      // Refresh the link/copy if it points elsewhere (updated install path).
      try { fs.unlinkSync(dest); } catch (_) {}
      if (process.platform === 'win32') fs.copyFileSync(p.rgPath, dest);
      else fs.symlinkSync(p.rgPath, dest);
      p.appRoot = appRoot;
    } catch (e) { console.error('[ext-host] ripgrep approot setup failed:', e.message); }
  }
  // Best-effort: some extensions (Gemini Code Assist is the known one) hard-require
  // `node-pty` from the VSCode-shaped appRoot and throw during activate() if it's
  // missing — which aborts activation before the extension registers anything, so
  // its whole panel stays blank. Provision it once in the background using the SAME
  // node/npm ext-host is running under, so the prebuilt native binary matches this
  // Node's ABI. Fire-and-forget: extensions that don't need node-pty are unaffected;
  // ones that do simply find it ready on a later activation once install finishes.
  try {
    const fs = require('fs');
    const path = require('path');
    const os = require('os');
    const { execFile } = require('child_process');
    const appRoot = p.appRoot || path.join(os.homedir(), '.aether', 'approot');
    const ptyDir = path.join(appRoot, 'node_modules', 'node-pty');
    const marker = path.join(appRoot, '.node-pty-install-attempted');
    if (!fs.existsSync(path.join(ptyDir, 'package.json')) && !fs.existsSync(marker)) {
      fs.mkdirSync(appRoot, { recursive: true });
      fs.writeFileSync(marker, String(Date.now()));
      const npmNextToNode = path.join(path.dirname(process.execPath), process.platform === 'win32' ? 'npm.cmd' : 'npm');
      const npmBin = fs.existsSync(npmNextToNode) ? npmNextToNode : 'npm';
      execFile(npmBin, ['install', '--no-audit', '--no-fund', 'node-pty'], { cwd: appRoot }, (err) => {
        if (err) {
          console.error('[ext-host] node-pty provisioning failed (extensions needing it, e.g. Gemini Code Assist, won\'t activate):', err.message);
          return;
        }
        // npm's default `allow-scripts` policy blocks node-pty's postinstall (which
        // would chmod these itself), leaving its prebuilt `spawn-helper` non-executable
        // — every pty spawn then fails with "posix_spawnp failed" even though the
        // require() succeeds. Fix the bit ourselves since we can't rely on the script.
        try {
          const prebuildsDir = path.join(ptyDir, 'prebuilds');
          for (const plat of fs.readdirSync(prebuildsDir)) {
            const helper = path.join(prebuildsDir, plat, 'spawn-helper');
            if (fs.existsSync(helper)) fs.chmodSync(helper, 0o755);
          }
        } catch (_) { /* best-effort */ }
        console.error('[ext-host] node-pty provisioned for extension use');
      });
    }
  } catch (e) { console.error('[ext-host] node-pty provisioning setup failed:', e.message); }
  dispatch.setWorkspace(p);
});
rpc.on_method('workspace/didChangeConfiguration', (p) => dispatch.setWorkspace(p));
rpc.on_method('activate', ({ extensionPath }) => activateExtension(extensionPath, hostLog, dispatch));
rpc.on_method('deactivate', ({ extensionPath }) => deactivateExtension(extensionPath));
rpc.on_method('workspace/didOpenTextDocument', (p) => dispatch.didOpen(p));
rpc.on_method('workspace/didChangeTextDocument', (p) => dispatch.didChange(p));
rpc.on_method('workspace/didSaveTextDocument', (p) => dispatch.didSave(p));
rpc.on_method('editor/didChangeActive', (p) => dispatch.didChangeActive(p));
rpc.on_method('hover/provide', (p) => dispatch.provideHover(p));
rpc.on_method('format/provide', (p) => dispatch.provideFormatting(p));
rpc.on_method('command/invoke', (p) => dispatch.invokeCommand(p));
rpc.on_method('webview/resolve', (p) => dispatch.resolveWebview(p));
rpc.on_method('webview/message', (p) => dispatch.webviewMessage(p));
rpc.on_method('webview/disposed', (p) => dispatch.webviewDisposed(p));
rpc.on_method('window/handleUri', (p) => dispatch.handleUri(p));
rpc.on_method('tree/children', (p) => dispatch.treeChildren(p));
rpc.on_method('tree/invoke', (p) => dispatch.treeInvoke(p));

const sock = net.connect(port, '127.0.0.1', () => {
  rpc.attach(sock);
  // Handshake: announce ourselves with the token so aether can trust the connection.
  rpc.notify('host/ready', { token });
});

sock.on('error', (e) => {
  console.error('[ext-host] socket error:', e.message);
  process.exit(1);
});
rpc.on('close', () => process.exit(0));

// Surface uncaught extension errors instead of dying silently.
process.on('uncaughtException', (e) => hostLog('error', 'uncaught: ' + (e && e.stack || e)));
process.on('unhandledRejection', (e) => hostLog('error', 'unhandledRejection: ' + (e && e.stack || e)));
