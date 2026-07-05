// JSON-RPC 2.0 over a newline-delimited socket — the transport shared by the
// extension host. One connection to aether; requests/notifications both ways.
// See /tmp/exthost-build/PROTOCOL.md (the frozen contract).
'use strict';

const EventEmitter = require('events');

class Rpc extends EventEmitter {
  constructor() {
    super();
    this.sock = null;
    this.buf = '';
    this.nextId = 1;
    this.pending = new Map(); // id -> {resolve, reject}
    // Handlers for inbound requests/notifications FROM aether (aether -> host).
    this.methods = new Map(); // method -> async (params) => result
  }

  attach(sock) {
    this.sock = sock;
    sock.setEncoding('utf8');
    sock.on('data', (chunk) => this._onData(chunk));
    sock.on('close', () => this.emit('close'));
    sock.on('error', (e) => this.emit('error', e));
  }

  // Register a handler for an inbound method (aether -> host).
  on_method(method, handler) {
    this.methods.set(method, handler);
  }

  // Fire a request TO aether; resolves with its result.
  request(method, params) {
    const id = this.nextId++;
    const msg = { jsonrpc: '2.0', id, method, params };
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this._send(msg);
    });
  }

  // Fire a notification TO aether (no reply expected).
  notify(method, params) {
    this._send({ jsonrpc: '2.0', method, params });
  }

  _send(obj) {
    if (!this.sock) return;
    this.sock.write(JSON.stringify(obj) + '\n');
  }

  _onData(chunk) {
    this.buf += chunk;
    let nl;
    while ((nl = this.buf.indexOf('\n')) >= 0) {
      const line = this.buf.slice(0, nl);
      this.buf = this.buf.slice(nl + 1);
      if (!line.trim()) continue;
      let msg;
      try {
        msg = JSON.parse(line);
      } catch (e) {
        continue; // ignore malformed lines rather than kill the connection
      }
      this._dispatch(msg);
    }
  }

  async _dispatch(msg) {
    // A response to one of our outbound requests.
    if (msg.id !== undefined && msg.method === undefined) {
      const p = this.pending.get(msg.id);
      if (!p) return;
      this.pending.delete(msg.id);
      if (msg.error) p.reject(new Error(msg.error.message || 'rpc error'));
      else p.resolve(msg.result);
      return;
    }
    // An inbound request/notification from aether.
    const handler = this.methods.get(msg.method);
    if (msg.id !== undefined) {
      // Request: must answer.
      if (!handler) {
        this._send({ jsonrpc: '2.0', id: msg.id, error: { code: -32601, message: 'method not found' } });
        return;
      }
      try {
        const result = await handler(msg.params || {});
        this._send({ jsonrpc: '2.0', id: msg.id, result: result === undefined ? null : result });
      } catch (e) {
        this._send({ jsonrpc: '2.0', id: msg.id, error: { code: -32000, message: String(e && e.message || e) } });
      }
    } else {
      // Notification: fire-and-forget; unknown ones are ignored.
      if (handler) {
        try { await handler(msg.params || {}); } catch (_) { /* ignore */ }
      }
    }
  }
}

module.exports = { Rpc };
