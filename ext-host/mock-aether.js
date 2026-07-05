// Standalone test harness — plays aether's side of PROTOCOL.md so the Node host can be
// exercised WITHOUT the Rust app. Listens on a port, spawns main.js, runs the handshake,
// activates the sample extension, then drives a command + a hover request and prints the
// round-trips. Exit code 0 on success.
'use strict';

const net = require('net');
const path = require('path');
const { spawn } = require('child_process');

const TOKEN = 'test-token-123';
let child;
let rpc;
let idc = 1;
const pending = new Map();
let passed = { info: false, hover: false };

function send(sock, obj) { sock.write(JSON.stringify(obj) + '\n'); }
function request(sock, method, params) {
  const id = idc++;
  send(sock, { jsonrpc: '2.0', id, method, params });
  return new Promise((res, rej) => pending.set(id, { res, rej }));
}

const server = net.createServer((sock) => {
  sock.setEncoding('utf8');
  let buf = '';
  sock.on('data', async (chunk) => {
    buf += chunk;
    let nl;
    while ((nl = buf.indexOf('\n')) >= 0) {
      const line = buf.slice(0, nl); buf = buf.slice(nl + 1);
      if (!line.trim()) continue;
      const msg = JSON.parse(line);
      // Response to one of our requests.
      if (msg.id !== undefined && msg.method === undefined) {
        const p = pending.get(msg.id); pending.delete(msg.id);
        if (p) (msg.error ? p.rej(new Error(msg.error.message)) : p.res(msg.result));
        continue;
      }
      // Inbound from the host.
      switch (msg.method) {
        case 'host/ready':
          if (msg.params.token !== TOKEN) { fail('bad token'); return; }
          console.log('✓ handshake (token ok)');
          send(sock, { jsonrpc: '2.0', method: 'host/init', params: { root: process.cwd() } });
          await drive(sock);
          break;
        case 'log':
          console.log(`  [host:${msg.params.level}] ${msg.params.message}`);
          break;
        case 'window/showInformationMessage':
          console.log(`✓ showInformationMessage → "${msg.params.message}"`);
          passed.info = true;
          send(sock, { jsonrpc: '2.0', id: msg.id, result: null });
          break;
        case 'commands/registerCommand':
          console.log(`  host registered command: ${msg.params.command}`);
          break;
        case 'languages/registerHoverProvider':
          console.log(`✓ registerHoverProvider → providerId ${msg.params.providerId}, selector ${JSON.stringify(msg.params.selector)}`);
          hoverProviderId = msg.params.providerId;
          break;
        default:
          if (msg.id !== undefined) send(sock, { jsonrpc: '2.0', id: msg.id, result: null });
      }
    }
  });
});

let hoverProviderId = null;

async function drive(sock) {
  // Activate the sample extension.
  const extPath = path.join(__dirname, 'sample-extension');
  const act = await request(sock, 'activate', { extensionPath: extPath });
  if (!act || !act.ok) return fail('activate failed: ' + JSON.stringify(act));
  console.log('✓ activate → ' + JSON.stringify(act));

  // Open a document so the provider has something to hover.
  send(sock, { jsonrpc: '2.0', method: 'workspace/didOpenTextDocument',
    params: { uri: 'file:///tmp/demo.txt', languageId: 'plaintext', version: 1, text: 'line zero\nline one\nline two\n' } });

  // Give the host a tick to register the hover provider from activate().
  await new Promise((r) => setTimeout(r, 50));
  if (hoverProviderId == null) return fail('hover provider was never registered');

  // Ask the host to run the hover provider.
  const hov = await request(sock, 'hover/provide', { providerId: hoverProviderId, uri: 'file:///tmp/demo.txt', line: 1, character: 3 });
  if (!hov || !hov.contents) return fail('hover returned nothing: ' + JSON.stringify(hov));
  console.log('✓ hover/provide → ' + JSON.stringify(hov));
  passed.hover = true;

  finish();
}

function finish() {
  const ok = passed.info && passed.hover;
  console.log(ok ? '\nALL CHECKS PASSED' : '\nFAILED: ' + JSON.stringify(passed));
  cleanup(ok ? 0 : 1);
}
function fail(m) { console.error('✗ ' + m); cleanup(1); }
function cleanup(code) { try { child && child.kill(); } catch (_) {} server.close(); process.exit(code); }

server.listen(0, '127.0.0.1', () => {
  const port = server.address().port;
  child = spawn('node', [path.join(__dirname, 'main.js'), String(port), TOKEN], { stdio: 'inherit' });
  child.on('exit', (c) => { if (c) console.error('host exited with ' + c); });
});

setTimeout(() => fail('timeout — no round-trip within 5s'), 5000);
