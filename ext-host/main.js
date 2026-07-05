// Aether extension host — entry point. Spawned by aether as:
//   node ext-host/main.js <port> <token>
// Connects to aether over TCP, provides the `vscode` shim to extensions, and bridges
// the two directions of PROTOCOL.md.
'use strict';

const net = require('net');
const { Rpc } = require('./rpc');
const { createVscode } = require('./vscode-shim');
const { installVscodeModule, activateExtension, deactivateExtension } = require('./activation');

const port = parseInt(process.argv[2], 10);
const token = process.argv[3] || '';

if (!port) {
  console.error('[ext-host] usage: node main.js <port> <token>');
  process.exit(2);
}

const rpc = new Rpc();
const { vscode, dispatch } = createVscode(rpc);
installVscodeModule(vscode);

const hostLog = (level, message) => rpc.notify('log', { level, message });

let workspaceRoot = null;

// ---- inbound handlers (aether -> host) ----
rpc.on_method('host/init', (p) => { workspaceRoot = p.root; dispatch.setWorkspace(p); });
rpc.on_method('workspace/didChangeConfiguration', (p) => dispatch.setWorkspace(p));
rpc.on_method('activate', ({ extensionPath }) => activateExtension(extensionPath, hostLog, dispatch));
rpc.on_method('deactivate', ({ extensionPath }) => deactivateExtension(extensionPath));
rpc.on_method('workspace/didOpenTextDocument', (p) => dispatch.didOpen(p));
rpc.on_method('workspace/didChangeTextDocument', (p) => dispatch.didChange(p));
rpc.on_method('workspace/didSaveTextDocument', (p) => dispatch.didSave(p));
rpc.on_method('editor/didChangeActive', (p) => dispatch.didChangeActive(p));
rpc.on_method('hover/provide', (p) => dispatch.provideHover(p));
rpc.on_method('command/invoke', (p) => dispatch.invokeCommand(p));
rpc.on_method('webview/resolve', (p) => dispatch.resolveWebview(p));
rpc.on_method('webview/message', (p) => dispatch.webviewMessage(p));
rpc.on_method('webview/disposed', (p) => dispatch.webviewDisposed(p));

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
