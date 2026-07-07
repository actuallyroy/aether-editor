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
class Uri {
  constructor(scheme, authority, path, query, fragment) {
    this.scheme = scheme || 'file';
    this.authority = authority || '';
    this.path = path || '';
    this.query = query || '';
    this.fragment = fragment || '';
  }
  get fsPath() { return this.path; }
  static file(p) { return new Uri('file', '', String(p)); }
  static parse(s) {
    const m = String(s).match(/^([a-zA-Z][a-zA-Z0-9+.-]*):\/\/([^/?#]*)([^?#]*)(?:\?([^#]*))?(?:#(.*))?$/);
    if (m) return new Uri(m[1], m[2], m[3] || '/', m[4] || '', m[5] || '');
    return new Uri('file', '', String(s));
  }
  static joinPath(base, ...parts) {
    const path = require('path');
    return new Uri(base.scheme, base.authority, path.posix.join(base.path, ...parts), base.query, base.fragment);
  }
  with(change) {
    return new Uri(
      change.scheme ?? this.scheme, change.authority ?? this.authority,
      change.path ?? this.path, change.query ?? this.query, change.fragment ?? this.fragment);
  }
  toString() {
    let s = `${this.scheme}://${this.authority}${this.path}`;
    if (this.query) s += '?' + this.query;
    if (this.fragment) s += '#' + this.fragment;
    return s;
  }
  toJSON() { return { scheme: this.scheme, authority: this.authority, path: this.path, query: this.query, fragment: this.fragment, fsPath: this.fsPath }; }
}
const StatusBarAlignment = { Left: 1, Right: 2 };
class Selection extends Range {
  constructor(a, b, c, d) {
    super(a, b, c, d);
    this.anchor = this.start; this.active = this.end;
    this.isEmpty = this.start.line === this.end.line && this.start.character === this.end.character;
  }
}
class ThemeIcon { constructor(id, color) { this.id = id; this.color = color; } }
class CancellationTokenSource {
  constructor() {
    const ev = new VsEvent();
    this.token = { isCancellationRequested: false, onCancellationRequested: ev.event };
    this._ev = ev;
  }
  cancel() { this.token.isCancellationRequested = true; this._ev.fire(); }
  dispose() {}
}
class RelativePattern {
  constructor(base, pattern) { this.base = (base && base.fsPath) || String(base); this.pattern = pattern; }
}

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
  // A full TextEditor wrapper. Many extensions dereference activeTextEditor (and
  // its .selection) at module load without null checks, so activeEditor always
  // holds a valid editor over an (initially empty) document.
  function makeEditor(doc) {
    return {
      document: doc,
      selection: new Selection(0, 0, 0, 0),
      selections: [new Selection(0, 0, 0, 0)],
      visibleRanges: [new Range(0, 0, doc.lineCount, 0)],
      options: { tabSize: 4, insertSpaces: true },
      viewColumn: 1,
      edit: () => Promise.resolve(false),
      insertSnippet: () => Promise.resolve(false),
      setDecorations: () => {},
      revealRange: () => {},
      show: () => {},
      hide: () => {},
    };
  }
  let activeEditor;

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

  activeEditor = makeEditor(makeDoc('untitled:Untitled', 'plaintext', 0, ''));

  // ---- hover providers + commands (called back BY aether) ----
  let nextProviderId = 1;
  const hoverProviders = new Map(); // providerId -> {selector, provider}
  const commands = new Map();       // command -> callback

  // ---- webviews (rendered by aether's webview-host process) ----
  const webviewProviders = new Map(); // viewId -> { provider, options }
  const webviewInstances = new Map(); // instanceId -> { onMessage: VsEvent, onDispose: VsEvent, view }
  let nextPanelId = -1; // panels get NEGATIVE instance ids (views get positive, from aether)
  function makeWebview(instanceId) {
    const onMessage = new VsEvent();
    const onDispose = new VsEvent();
    let html = '';
    const webview = {
      get html() { return html; },
      set html(v) { html = v; rpc.notify('webview/setHtml', { instanceId, html: v }); },
      options: {},
      // Windows (WebView2) can't intercept custom schemes — wry serves custom
      // protocols there as http://{scheme}.localhost/ instead.
      cspSource: process.platform === 'win32' ? 'http://aether-res.localhost' : 'aether-res:',
      asWebviewUri: (uri) => {
        const p = (uri && uri.fsPath) || String(uri).replace(/^file:\/\//, '');
        const s = process.platform === 'win32'
          ? 'http://aether-res.localhost/file' + p.replace(/\\/g, '/')
          : 'aether-res://file' + p;
        return { toString: () => s, scheme: 'aether-res', fsPath: p };
      },
      postMessage: (data) => { rpc.notify('webview/postMessage', { instanceId, data }); return Promise.resolve(true); },
      get onDidReceiveMessage() { return onMessage.event; },
    };
    const view = {
      webview,
      viewType: '',
      visible: true,
      title: '',
      description: '',
      badge: undefined,
      show: () => {},
      get onDidDispose() { return onDispose.event; },
      onDidChangeVisibility: () => new Disposable(() => {}),
    };
    webviewInstances.set(instanceId, { onMessage, onDispose, view });
    return view;
  }

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

  const onDidOpenTerminal = new VsEvent();
  const onDidCloseTerminal = new VsEvent();
  const terminals = [];
  function makeTerminal(nameOrOpts, shellPath, shellArgs) {
    const opts = typeof nameOrOpts === 'object' && nameOrOpts !== null ? nameOrOpts : { name: nameOrOpts, shellPath, shellArgs };
    const term = {
      name: opts.name || 'Terminal',
      creationOptions: opts,
      processId: Promise.resolve(0),
      exitStatus: undefined,
      state: { isInteractedWith: false },
      sendText: (text, addNewLine) =>
        rpc.notify('terminal/sendText', { name: opts.name || 'Terminal', text, addNewLine: addNewLine !== false }),
      show: () => rpc.notify('terminal/show', { name: opts.name || 'Terminal' }),
      hide: () => {},
      dispose: () => {
        const i = terminals.indexOf(term);
        if (i >= 0) terminals.splice(i, 1);
        onDidCloseTerminal.fire(term);
      },
    };
    terminals.push(term);
    onDidOpenTerminal.fire(term);
    return term;
  }

  const vscode = {
    version: '1.90.0',
    Position, Range, Selection, MarkdownString, Hover, Disposable, Uri,
    EventEmitter: VsEvent,
    StatusBarAlignment,
    ThemeColor: class ThemeColor { constructor(id) { this.id = id; } },
    ThemeIcon,
    CancellationTokenSource,
    RelativePattern,
    ViewColumn: { Active: -1, Beside: -2, One: 1, Two: 2, Three: 3 },
    ProgressLocation: { SourceControl: 1, Window: 10, Notification: 15 },
    ExtensionMode: { Production: 1, Development: 2, Test: 3 },
    UIKind: { Desktop: 1, Web: 2 },
    ColorThemeKind: { Light: 1, Dark: 2, HighContrast: 3, HighContrastLight: 4 },
    ConfigurationTarget: { Global: 1, Workspace: 2, WorkspaceFolder: 3 },
    DiagnosticSeverity: { Error: 0, Warning: 1, Information: 2, Hint: 3 },
    TextEditorRevealType: { Default: 0, InCenter: 1, InCenterIfOutsideViewport: 2, AtTop: 3 },
    OverviewRulerLane: { Left: 1, Center: 2, Right: 4, Full: 7 },
    FileType: { Unknown: 0, File: 1, Directory: 2, SymbolicLink: 64 },
    TreeItemCollapsibleState: { None: 0, Collapsed: 1, Expanded: 2 },
    TreeItem: class TreeItem { constructor(label, state) { this.label = label; this.collapsibleState = state || 0; } },
    CodeLens: class CodeLens { constructor(range, command) { this.range = range; this.command = command; } },
    NotebookCellOutputItem: class NotebookCellOutputItem {
      constructor(data, mime) { this.data = data; this.mime = mime; }
      static text(s, mime) { return new this(Buffer.from(String(s)), mime || 'text/plain'); }
      static json(v, mime) { return new this(Buffer.from(JSON.stringify(v)), mime || 'text/x-json'); }
      static error(e) { return new this(Buffer.from(JSON.stringify({ name: e && e.name, message: e && e.message, stack: e && e.stack })), 'application/vnd.code.notebook.error'); }
      static stdout(s) { return new this(Buffer.from(String(s)), 'application/vnd.code.notebook.stdout'); }
      static stderr(s) { return new this(Buffer.from(String(s)), 'application/vnd.code.notebook.stderr'); }
    },
    NotebookCellOutput: class NotebookCellOutput { constructor(items) { this.items = items || []; } },
    NotebookCellData: class NotebookCellData { constructor(kind, value, languageId) { this.kind = kind; this.value = value; this.languageId = languageId; } },
    NotebookCellKind: { Markup: 1, Code: 2 },
    NotebookEdit: class NotebookEdit { constructor(range, cells) { this.range = range; this.newCells = cells; } },
    NotebookRange: class NotebookRange { constructor(start, end) { this.start = start; this.end = end; } },
    WorkspaceEdit: class WorkspaceEdit {
      constructor() { this._edits = []; }
      replace(uri, range, text) { this._edits.push({ uri, range, text }); }
      insert(uri, pos, text) { this._edits.push({ uri, range: new Range(pos, pos), text }); }
      set() {}
    },

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
      registerWebviewViewProvider: (viewId, provider, options) => {
        webviewProviders.set(viewId, { provider, options: options || {} });
        rpc.notify('window/registerWebviewViewProvider', { viewId });
        return new Disposable(() => webviewProviders.delete(viewId));
      },
      createOutputChannel: (name) => {
        const log = (level) => (msg, ...rest) =>
          rpc.notify('log', { level, message: `[${name}] ${msg}${rest.length ? ' ' + rest.map((r) => JSON.stringify(r)).join(' ') : ''}` });
        return {
          name,
          append: log('info'), appendLine: log('info'),
          trace: log('trace'), debug: log('debug'), info: log('info'),
          warn: log('warn'), error: log('error'),
          logLevel: 3, onDidChangeLogLevel: () => new Disposable(() => {}),
          replace: () => {}, show: () => {}, hide: () => {}, clear: () => {}, dispose: () => {},
        };
      },
      setStatusBarMessage: () => new Disposable(() => {}),
      get activeTextEditor() { return activeEditor; },
      get visibleTextEditors() { return activeEditor ? [activeEditor] : []; },
      get onDidChangeActiveTextEditor() { return onDidChangeActive.event; },
      onDidChangeTextEditorSelection: () => new Disposable(() => {}),
      onDidChangeTextEditorVisibleRanges: () => new Disposable(() => {}),
      onDidChangeVisibleTextEditors: () => new Disposable(() => {}),
      onDidChangeWindowState: () => new Disposable(() => {}),
      onDidChangeActiveColorTheme: () => new Disposable(() => {}),
      state: { focused: true, active: true },
      activeColorTheme: { kind: 2 }, // Dark
      createTerminal: (nameOrOpts, shellPath, shellArgs) => makeTerminal(nameOrOpts, shellPath, shellArgs),
      get terminals() { return [...terminals]; },
      activeTerminal: undefined,
      get onDidOpenTerminal() { return onDidOpenTerminal.event; },
      get onDidCloseTerminal() { return onDidCloseTerminal.event; },
      onDidChangeActiveTerminal: () => new Disposable(() => {}),
      onDidChangeTerminalState: () => new Disposable(() => {}),
      registerUriHandler: () => new Disposable(() => {}),
      registerWebviewPanelSerializer: () => new Disposable(() => {}),
      createWebviewPanel: (viewType, title, _showOpts, _options) => {
        // A webview panel is just another webview instance; aether shows it in its
        // own webview-host window. Reuses the sidebar plumbing.
        const instanceId = nextPanelId--;
        const view = makeWebview(instanceId);
        view.viewType = viewType;
        rpc.notify('webview/createPanel', { instanceId, viewType, title });
        const onDispose = new VsEvent();
        const panel = {
          webview: view.webview,
          viewType, title, visible: true, active: true, viewColumn: 1,
          reveal: () => {},
          dispose: () => { rpc.notify('webview/disposePanel', { instanceId }); onDispose.fire(); },
          get onDidDispose() { return onDispose.event; },
          onDidChangeViewState: () => new Disposable(() => {}),
        };
        // The extension's panel icon becomes the editor-tab icon.
        let icon;
        Object.defineProperty(panel, 'iconPath', {
          get: () => icon,
          set: (v) => {
            icon = v;
            const one = v && (v.dark || v.light || v);
            const p = one && (one.fsPath || String(one).replace(/^file:\/\//, ''));
            if (p) rpc.notify('webview/setIcon', { instanceId, path: p });
          },
        });
        return panel;
      },
      registerFileDecorationProvider: () => new Disposable(() => {}),
      registerTerminalLinkProvider: () => new Disposable(() => {}),
      createTextEditorDecorationType: (opts) => ({ key: 'deco', dispose: () => {}, options: opts }),
      showTextDocument: (doc) =>
        rpc.request('window/showTextDocument', { uri: doc && doc.uri ? doc.uri.toString() : String(doc) })
          .then(() => activeEditor),
      showSaveDialog: () => Promise.resolve(undefined),
      showOpenDialog: () => Promise.resolve(undefined),
      tabGroups: {
        all: [], activeTabGroup: { tabs: [], activeTab: undefined },
        onDidChangeTabs: () => new Disposable(() => {}),
        onDidChangeTabGroups: () => new Disposable(() => {}),
        close: () => Promise.resolve(true),
      },
      withProgress: (_opts, task) => task({ report: () => {} }, { isCancellationRequested: false }),
    },

    commands: {
      registerCommand: (command, callback) => {
        commands.set(command, callback);
        rpc.notify('commands/registerCommand', { command });
        return new Disposable(() => commands.delete(command));
      },
      executeCommand: (command, ...args) => {
        // Commands registered IN this host run directly; everything else goes to aether.
        const local = commands.get(command);
        if (local) return Promise.resolve(local(...args));
        return rpc.request('commands/executeCommand', { command, args });
      },
      getCommands: () => Promise.resolve([...commands.keys()]),
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
      onWillSaveTextDocument: () => new Disposable(() => {}),
      onDidCreateFiles: () => new Disposable(() => {}),
      onDidDeleteFiles: () => new Disposable(() => {}),
      onDidRenameFiles: () => new Disposable(() => {}),
      onDidChangeTextEditorSelection: () => new Disposable(() => {}),
      registerTextDocumentContentProvider: () => new Disposable(() => {}),
      registerFileSystemProvider: () => new Disposable(() => {}),
      applyEdit: () => Promise.resolve(true),
      name: ws.root ? path.basename(ws.root) : undefined,
      workspaceFile: undefined,
      isTrusted: true,
      fs: {
        readFile: (uri) => Promise.resolve(new Uint8Array(fs.readFileSync(uri.fsPath || String(uri)))),
        writeFile: (uri, data) => { fs.mkdirSync(path.dirname(uri.fsPath), { recursive: true }); fs.writeFileSync(uri.fsPath, Buffer.from(data)); return Promise.resolve(); },
        stat: (uri) => {
          const s = fs.statSync(uri.fsPath || String(uri));
          return Promise.resolve({ type: s.isDirectory() ? 2 : 1, size: s.size, ctime: s.ctimeMs, mtime: s.mtimeMs });
        },
        readDirectory: (uri) =>
          Promise.resolve(fs.readdirSync(uri.fsPath, { withFileTypes: true }).map((e) => [e.name, e.isDirectory() ? 2 : 1])),
        createDirectory: (uri) => { fs.mkdirSync(uri.fsPath, { recursive: true }); return Promise.resolve(); },
        delete: (uri, opts) => { fs.rmSync(uri.fsPath, { recursive: !!(opts && opts.recursive), force: true }); return Promise.resolve(); },
        rename: (a, b) => { fs.renameSync(a.fsPath, b.fsPath); return Promise.resolve(); },
      },
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
      appName: 'Aether', appHost: 'desktop', language: 'en',
      machineId: 'aether', sessionId: 'aether-' + process.pid,
      uriScheme: 'aether', appRoot: '', shell: process.env.SHELL || '/bin/bash',
      remoteName: undefined, uiKind: 1, isNewAppInstall: false, isTelemetryEnabled: false,
      onDidChangeTelemetryEnabled: () => new Disposable(() => {}),
      asExternalUri: (uri) => Promise.resolve(uri),
      openExternal: (uri) => rpc.request('env/openExternal', { uri: String(uri) }).then(() => true),
      clipboard: { writeText: () => Promise.resolve(), readText: () => Promise.resolve('') },
    },

    debug: {
      onDidStartDebugSession: () => new Disposable(() => {}),
      onDidTerminateDebugSession: () => new Disposable(() => {}),
      registerDebugConfigurationProvider: () => new Disposable(() => {}),
      startDebugging: () => Promise.resolve(false),
      activeDebugSession: undefined,
    },

    tasks: {
      registerTaskProvider: () => new Disposable(() => {}),
      onDidStartTask: () => new Disposable(() => {}),
      onDidEndTask: () => new Disposable(() => {}),
    },

    authentication: {
      getSession: () => Promise.resolve(undefined),
      onDidChangeSessions: () => new Disposable(() => {}),
      registerAuthenticationProvider: () => new Disposable(() => {}),
    },

    comments: { createCommentController: () => ({ dispose: () => {} }) },

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
      registerCodeLensProvider: () => new Disposable(() => {}),
      registerCodeActionsProvider: () => new Disposable(() => {}),
      registerDocumentFormattingEditProvider: () => new Disposable(() => {}),
      registerDocumentLinkProvider: () => new Disposable(() => {}),
      registerInlineCompletionItemProvider: () => new Disposable(() => {}),
      onDidChangeDiagnostics: () => new Disposable(() => {}),
      getDiagnostics: () => [],
      setTextDocumentLanguage: (doc) => Promise.resolve(doc),
      match: () => 0,
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
      if (!uri) return; // keep the last (or default) editor — never undefined
      const doc = docs.get(uri) || makeDoc(uri, languageId || 'plaintext', 0, '');
      activeEditor = makeEditor(doc);
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
    // aether asks the provider to populate a fresh webview instance.
    async resolveWebview({ viewId, instanceId }) {
      const entry = webviewProviders.get(viewId);
      if (!entry) throw new Error('no webview provider: ' + viewId);
      const view = makeWebview(instanceId);
      view.viewType = viewId;
      await entry.provider.resolveWebviewView(view, { state: undefined }, { isCancellationRequested: false, onCancellationRequested: () => new Disposable(() => {}) });
      return { ok: true };
    },
    // A message arrived FROM the webview page (acquireVsCodeApi().postMessage).
    webviewMessage({ instanceId, data }) {
      const inst = webviewInstances.get(instanceId);
      if (inst) inst.onMessage.fire(data);
    },
    webviewDisposed({ instanceId }) {
      const inst = webviewInstances.get(instanceId);
      if (inst) { inst.onDispose.fire(); webviewInstances.delete(instanceId); }
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
