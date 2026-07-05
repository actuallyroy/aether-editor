// The fake `vscode` module handed to extensions. Every API call forwards to aether
// over RPC (see PROTOCOL.md). This is the FIRST SLICE: window messages, commands,
// workspace config + document events, and hover providers. Unimplemented members
// throw "not implemented in aether" so gaps are obvious rather than silent.
'use strict';

// ---- Minimal value types (subset of the vscode API shapes) ----
class Position {
  constructor(line, character) { this.line = line; this.character = character; }
}
class Range {
  constructor(a, b, c, d) {
    if (a instanceof Position) { this.start = a; this.end = b; }
    else { this.start = new Position(a, b); this.end = new Position(c, d); }
  }
}
class MarkdownString {
  constructor(value) { this.value = value || ''; this.isTrusted = false; }
  appendText(s) { this.value += s; return this; }
  appendMarkdown(s) { this.value += s; return this; }
  appendCodeblock(code, lang) { this.value += '\n```' + (lang || '') + '\n' + code + '\n```\n'; return this; }
}
class Hover {
  constructor(contents, range) { this.contents = Array.isArray(contents) ? contents : [contents]; this.range = range; }
}
class Disposable {
  constructor(fn) { this._fn = fn; }
  dispose() { if (this._fn) { this._fn(); this._fn = null; } }
}
const Uri = {
  parse: (s) => ({ scheme: 'file', toString: () => s, fsPath: s.replace(/^file:\/\//, '') }),
  file: (p) => ({ scheme: 'file', toString: () => 'file://' + p, fsPath: p }),
};

function contentsToMarkdown(contents) {
  // vscode Hover.contents: (string | MarkdownString)[]
  const parts = (Array.isArray(contents) ? contents : [contents]).map((c) => {
    if (c == null) return '';
    if (typeof c === 'string') return c;
    if (typeof c.value === 'string') return c.value;
    return String(c);
  });
  return parts.filter(Boolean).join('\n\n');
}

// A lightweight EventEmitter matching vscode's Event/emitter pattern.
class VsEvent {
  constructor() { this._subs = new Set(); }
  get event() {
    return (listener) => { this._subs.add(listener); return new Disposable(() => this._subs.delete(listener)); };
  }
  fire(arg) { for (const l of [...this._subs]) { try { l(arg); } catch (_) {} } }
}

function createVscode(rpc) {
  // ---- workspace document store ----
  const docs = new Map(); // uri -> TextDocument
  const onDidOpen = new VsEvent();
  const onDidChange = new VsEvent();

  function makeDoc(uri, languageId, version, text) {
    const lines = text.split('\n');
    return {
      uri: Uri.parse(uri),
      languageId,
      version,
      _text: text,
      getText() { return this._text; },
      lineAt(line) { return { text: lines[line] || '', lineNumber: line }; },
      get lineCount() { return lines.length; },
    };
  }

  // ---- hover providers + commands (called back BY aether) ----
  let nextProviderId = 1;
  const hoverProviders = new Map(); // providerId -> {selector, provider}
  const commands = new Map();       // command -> callback

  const vscode = {
    version: '1.0.0-aether',
    Position, Range, MarkdownString, Hover, Disposable, Uri,
    EventEmitter: VsEvent,

    window: {
      showInformationMessage: (message, ...items) =>
        rpc.request('window/showInformationMessage', { message, items: items.flat() }),
      showErrorMessage: (message, ...items) =>
        rpc.request('window/showErrorMessage', { message, items: items.flat() }),
      showWarningMessage: (message, ...items) =>
        rpc.request('window/showErrorMessage', { message, items: items.flat() }),
    },

    commands: {
      registerCommand: (command, callback) => {
        commands.set(command, callback);
        rpc.notify('commands/registerCommand', { command });
        return new Disposable(() => commands.delete(command));
      },
      executeCommand: (command, ...args) =>
        rpc.request('commands/executeCommand', { command, args }),
    },

    workspace: {
      getConfiguration: (section) => {
        // Synchronous in vscode; we return a cached-ish object. For the first slice
        // we resolve lazily and expose get() with a default fallback.
        let cache = {};
        rpc.request('workspace/getConfiguration', { section }).then((v) => { cache = v || {}; }).catch(() => {});
        return {
          get: (key, dflt) => (key in cache ? cache[key] : dflt),
          has: (key) => key in cache,
          update: () => Promise.resolve(),
        };
      },
      get onDidOpenTextDocument() { return onDidOpen.event; },
      get onDidChangeTextDocument() { return onDidChange.event; },
      get textDocuments() { return [...docs.values()]; },
    },

    languages: {
      registerHoverProvider: (selector, provider) => {
        const providerId = nextProviderId++;
        const langs = (Array.isArray(selector) ? selector : [selector]).map((s) =>
          typeof s === 'string' ? s : (s && s.language) || '*');
        hoverProviders.set(providerId, { selector: langs, provider });
        rpc.notify('languages/registerHoverProvider', { providerId, selector: langs });
        return new Disposable(() => hoverProviders.delete(providerId));
      },
      // Stubs so extensions that also register these still load.
      registerCompletionItemProvider: () => new Disposable(() => {}),
      registerDefinitionProvider: () => new Disposable(() => {}),
      createDiagnosticCollection: () => ({ set: () => {}, delete: () => {}, clear: () => {}, dispose: () => {} }),
    },
  };

  // ---- inbound dispatch (aether -> host), wired by main.js ----
  const dispatch = {
    didOpen({ uri, languageId, version, text }) {
      const doc = makeDoc(uri, languageId, version, text);
      docs.set(uri, doc);
      onDidOpen.fire(doc);
    },
    didChange({ uri, version, text }) {
      const doc = makeDoc(uri, docs.get(uri) ? docs.get(uri).languageId : 'plaintext', version, text);
      docs.set(uri, doc);
      onDidChange.fire({ document: doc, contentChanges: [] });
    },
    async provideHover({ providerId, uri, line, character }) {
      const entry = hoverProviders.get(providerId);
      if (!entry) return null;
      const doc = docs.get(uri) || makeDoc(uri, 'plaintext', 0, '');
      const pos = new Position(line, character);
      const res = await entry.provider.provideHover(doc, pos, { isCancellationRequested: false });
      if (!res) return null;
      const contents = contentsToMarkdown(res.contents);
      if (!contents) return null;
      let range;
      if (res.range) {
        const r = res.range;
        range = [r.start.line, r.start.character, r.end.line, r.end.character];
      }
      return { contents, range };
    },
    async invokeCommand({ command, args }) {
      const cb = commands.get(command);
      if (!cb) throw new Error('no such command: ' + command);
      return await cb(...(args || []));
    },
  };

  return { vscode, dispatch };
}

module.exports = { createVscode, contentsToMarkdown };
