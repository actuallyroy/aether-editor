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
  parse: (s) => ({ scheme: 'file', toString: () => s, fsPath: s.replace(/^file:\/\//, ''), get path() { return this.fsPath; } }),
  file: (p) => ({ scheme: 'file', toString: () => 'file://' + p, fsPath: p, get path() { return p; } }),
};
const StatusBarAlignment = { Left: 1, Right: 2 };

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

// Convert a vscode glob (`**/*.html`, `{a,b}`) into a RegExp for findFiles.
function globToRegex(glob) {
  let re = '';
  for (let i = 0; i < glob.length; i++) {
    const c = glob[i];
    if (c === '*') {
      if (glob[i + 1] === '*') { re += '.*'; i++; if (glob[i + 1] === '/') i++; }
      else re += '[^/]*';
    } else if (c === '?') re += '[^/]';
    else if (c === '{') re += '(';
    else if (c === '}') re += ')';
    else if (c === ',') re += '|';
    else re += c.replace(/[.+^$()|[\]\\]/g, '\\$&');
  }
  return new RegExp('(^|/)' + re + '$');
}

function createVscode(rpc) {
  const fs = require('fs');
  const path = require('path');

  // ---- workspace state (root set by host/init; settings pushed by aether) ----
  const ws = { root: null, settings: {} };
  // Configuration DEFAULTS collected from every activated extension's
  // `contributes.configuration` (flat `section.key` -> default value).
  const configDefaults = {};
  // `publisher.name` (lowercase) -> extension record, for extensions.getExtension.
  const activatedExts = new Map();

  // ---- workspace document store ----
  const docs = new Map(); // uri -> TextDocument
  const onDidOpen = new VsEvent();
  const onDidChange = new VsEvent();
  const onDidSave = new VsEvent();
  const onDidChangeActive = new VsEvent();
  let activeEditor = undefined;

  function makeDoc(uri, languageId, version, text) {
    const lines = text.split('\n');
    return {
      uri: Uri.parse(uri),
      fileName: uri.replace(/^file:\/\//, ''),
      languageId,
      version,
      isDirty: false,
      isUntitled: false,
      save: () => Promise.resolve(true),
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

  // ---- status bar items (mirrored to aether's status bar over RPC) ----
  let nextStatusId = 1;
  function makeStatusBarItem(alignment, priority) {
    const id = nextStatusId++;
    const st = { text: '', tooltip: '', command: undefined, visible: false };
    const push = () =>
      rpc.notify('window/statusBar', {
        id, text: st.text, tooltip: typeof st.tooltip === 'string' ? st.tooltip : '',
        command: typeof st.command === 'string' ? st.command : (st.command && st.command.command) || null,
        visible: st.visible, alignment: alignment || StatusBarAlignment.Left, priority: priority || 0,
      });
    const item = {
      alignment, priority,
      show() { st.visible = true; push(); },
      hide() { st.visible = false; push(); },
      dispose() { st.visible = false; rpc.notify('window/statusBar', { id, visible: false, dispose: true }); },
    };
    // Property writes after show() must reach aether — mirror through setters.
    for (const key of ['text', 'tooltip', 'command', 'color', 'backgroundColor']) {
      Object.defineProperty(item, key, {
        get: () => st[key],
        set: (v) => { st[key] = v; if (st.visible) push(); },
      });
    }
    return item;
  }

  const vscode = {
    version: '1.0.0-aether',
    Position, Range, MarkdownString, Hover, Disposable, Uri,
    EventEmitter: VsEvent,
    StatusBarAlignment,
    ThemeColor: class ThemeColor { constructor(id) { this.id = id; } },

    window: {
      showInformationMessage: (message, ...items) =>
        rpc.request('window/showInformationMessage', { message, items: items.flat() }),
      showErrorMessage: (message, ...items) =>
        rpc.request('window/showErrorMessage', { message, items: items.flat() }),
      showWarningMessage: (message, ...items) =>
        rpc.request('window/showWarningMessage', { message, items: items.flat() }),
      showQuickPick: (items, options) =>
        Promise.resolve(items).then((list) =>
          rpc.request('window/showQuickPick', {
            items: (list || []).map((it) => (typeof it === 'string' ? it : it.label)),
            placeHolder: (options && options.placeHolder) || '',
          }).then((picked) => {
            if (picked == null) return undefined;
            return (list || []).find((it) => (typeof it === 'string' ? it : it.label) === picked);
          })),
      showInputBox: (options) =>
        rpc.request('window/showInputBox', {
          prompt: (options && options.prompt) || '', value: (options && options.value) || '',
        }).then((v) => (v == null ? undefined : v)),
      createStatusBarItem: (alignment, priority) => makeStatusBarItem(alignment, priority),
      createOutputChannel: (name) => ({
        name,
        append: (s) => rpc.notify('log', { level: 'info', message: `[${name}] ${s}` }),
        appendLine: (s) => rpc.notify('log', { level: 'info', message: `[${name}] ${s}` }),
        show: () => {}, hide: () => {}, clear: () => {}, dispose: () => {},
      }),
      setStatusBarMessage: () => new Disposable(() => {}),
      get activeTextEditor() { return activeEditor; },
      get onDidChangeActiveTextEditor() { return onDidChangeActive.event; },
      withProgress: (_opts, task) => task({ report: () => {} }, { isCancellationRequested: false }),
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
      // Synchronous, like vscode: user settings (pushed by aether into ws.settings)
      // override the defaults registered from each extension's contributes.
      getConfiguration: (section) => {
        const full = (key) => (section ? section + '.' + key : key);
        const lookup = (key) => {
          const k = full(key);
          if (k in ws.settings) return ws.settings[k];
          if (k in configDefaults) return configDefaults[k];
          return undefined;
        };
        return {
          get: (key, dflt) => { const v = lookup(key); return v === undefined ? dflt : v; },
          has: (key) => lookup(key) !== undefined,
          inspect: (key) => ({ key: full(key), defaultValue: configDefaults[full(key)] }),
          update: (key, value) => { ws.settings[full(key)] = value; return Promise.resolve(); },
        };
      },
      get workspaceFolders() {
        if (!ws.root) return undefined;
        return [{ uri: Uri.file(ws.root), name: path.basename(ws.root), index: 0 }];
      },
      get rootPath() { return ws.root || undefined; },
      getWorkspaceFolder: (uri) => {
        if (!ws.root || !uri) return undefined;
        const p = uri.fsPath || String(uri);
        return p.startsWith(ws.root) ? { uri: Uri.file(ws.root), name: path.basename(ws.root), index: 0 } : undefined;
      },
      asRelativePath: (u) => {
        const p = (u && u.fsPath) || String(u);
        return ws.root && p.startsWith(ws.root) ? p.slice(ws.root.length + 1) : p;
      },
      // Local recursive walk — enough for the include-glob + maxResults shape
      // extensions actually use (Live Server: find *.html).
      findFiles: (include, _exclude, maxResults) => {
        const out = [];
        if (!ws.root) return Promise.resolve(out);
        const re = globToRegex(String(include));
        const max = maxResults || 5000;
        const walk = (dir, depth) => {
          if (out.length >= max || depth > 12) return;
          let entries;
          try { entries = fs.readdirSync(dir, { withFileTypes: true }); } catch (_) { return; }
          for (const e of entries) {
            if (out.length >= max) return;
            if (e.name === 'node_modules' || e.name.startsWith('.')) continue;
            const p = path.join(dir, e.name);
            if (e.isDirectory()) walk(p, depth + 1);
            else if (re.test(p.slice(ws.root.length + 1))) out.push(Uri.file(p));
          }
        };
        walk(ws.root, 0);
        return Promise.resolve(out);
      },
      saveAll: () => rpc.request('workspace/saveAll', {}).then(() => true),
      openTextDocument: (arg) => {
        const p = typeof arg === 'string' ? arg : arg && arg.fsPath;
        let text = '';
        try { text = fs.readFileSync(p, 'utf8'); } catch (_) {}
        return Promise.resolve(makeDoc('file://' + p, 'plaintext', 1, text));
      },
      get onDidOpenTextDocument() { return onDidOpen.event; },
      get onDidChangeTextDocument() { return onDidChange.event; },
      get onDidSaveTextDocument() { return onDidSave.event; },
      onDidCloseTextDocument: () => new Disposable(() => {}),
      onDidChangeConfiguration: () => new Disposable(() => {}),
      onDidChangeWorkspaceFolders: () => new Disposable(() => {}),
      createFileSystemWatcher: () => ({
        onDidCreate: () => new Disposable(() => {}), onDidChange: () => new Disposable(() => {}),
        onDidDelete: () => new Disposable(() => {}), dispose: () => {},
      }),
      get textDocuments() { return [...docs.values()]; },
    },

    extensions: {
      // Extensions often read their OWN manifest via getExtension(id).packageJSON.
      // Activated extensions are registered here (see registerContributes); anything
      // else (e.g. Live Share integration probes) is genuinely absent.
      getExtension: (id) => activatedExts.get(String(id).toLowerCase()),
      get all() { return [...activatedExts.values()]; },
    },

    env: {
      appName: 'Aether', language: 'en', machineId: 'aether', sessionId: 'aether',
      openExternal: (uri) => rpc.request('env/openExternal', { uri: String(uri) }).then(() => true),
      clipboard: { writeText: () => Promise.resolve(), readText: () => Promise.resolve('') },
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
    // host/init + settings updates land here.
    setWorkspace({ root, settings }) {
      if (root) ws.root = root;
      if (settings && typeof settings === 'object') Object.assign(ws.settings, settings);
    },
    // activation.js registers each activated extension's contributed config defaults.
    registerContributes(pkg, extPath) {
      if (pkg && pkg.name) {
        const id = `${pkg.publisher || 'unknown'}.${pkg.name}`.toLowerCase();
        activatedExts.set(id, { id, packageJSON: pkg, extensionPath: extPath, isActive: true, exports: undefined });
      }
      const conf = pkg && pkg.contributes && pkg.contributes.configuration;
      const sections = Array.isArray(conf) ? conf : conf ? [conf] : [];
      for (const s of sections) {
        for (const [key, spec] of Object.entries(s.properties || {})) {
          if (spec && 'default' in spec) configDefaults[key] = spec.default;
        }
      }
    },
    didChangeActive({ uri, languageId }) {
      if (!uri) { activeEditor = undefined; onDidChangeActive.fire(undefined); return; }
      const doc = docs.get(uri) || makeDoc(uri, languageId || 'plaintext', 0, '');
      activeEditor = { document: doc, selection: undefined };
      onDidChangeActive.fire(activeEditor);
    },
    didSave({ uri }) {
      const doc = docs.get(uri);
      if (doc) onDidSave.fire(doc);
    },
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
