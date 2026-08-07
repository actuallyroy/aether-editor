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
  static from(...disposables) {
    return new Disposable(() => { for (const d of disposables) { try { d?.dispose(); } catch (_) {} } });
  }
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
  static from(o) {
    return new Uri(o.scheme || 'file', o.authority || '', o.path || '', o.query || '', o.fragment || '');
  }
  static parse(s) {
    const m = String(s).match(/^([a-zA-Z][a-zA-Z0-9+.-]*):\/\/([^/?#]*)([^?#]*)(?:\?([^#]*))?(?:#(.*))?$/);
    if (m) return new Uri(m[1], m[2], m[3] || '/', m[4] || '', m[5] || '');
    return new Uri('file', '', String(s));
  }
  static joinPath(base, ...parts) {
    const path = require('path');
    // resolve() (not join()) so `..` clamps at '/': extensions walk uris up the
    // tree until path === '/', and join('', '..') grows '../../..' forever
    // (MPE's workspace-folder search OOMed the host on a relative base).
    return new Uri(
      base.scheme, base.authority,
      path.posix.resolve(base.path || '/', ...parts.map(String)),
      base.query, base.fragment);
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
// Core LSP-conversion value types — vscode-languageclient's protocolConverter
// (used by most LSP-based extensions, e.g. Gemini Code Assist's language client)
// builds every diagnostic/symbol/link/hint through these; missing any one of them
// throws "Class extends value undefined" at require-time and aborts the whole
// extension's activate() before it registers anything (#gemini-blank-panel).
class Location {
  constructor(uri, rangeOrPosition) { this.uri = uri; this.range = rangeOrPosition instanceof Range ? rangeOrPosition : new Range(rangeOrPosition, rangeOrPosition); }
}
class LocationLink {
  constructor(targetUri, targetRange, targetSelectionRange, originSelectionRange) {
    this.targetUri = targetUri; this.targetRange = targetRange;
    this.targetSelectionRange = targetSelectionRange; this.originSelectionRange = originSelectionRange;
  }
}
class TextEdit {
  constructor(range, newText) { this.range = range; this.newText = newText; }
  static replace(range, newText) { return new TextEdit(range, newText); }
  static insert(position, newText) { return new TextEdit(new Range(position, position), newText); }
  static delete(range) { return new TextEdit(range, ''); }
  static setEndOfLine() { return new TextEdit(new Range(new Position(0, 0), new Position(0, 0)), ''); }
}
class Diagnostic {
  constructor(range, message, severity) { this.range = range; this.message = message; this.severity = severity ?? 0; }
}
class DocumentLink {
  constructor(range, target) { this.range = range; this.target = target; }
}
class CodeAction {
  constructor(title, kind) { this.title = title; this.kind = kind; }
}
class SymbolInformation {
  constructor(name, kind, containerNameOrRange, locationOrUri) {
    this.name = name; this.kind = kind;
    if (locationOrUri) { this.containerName = containerNameOrRange; this.location = new Location(locationOrUri, containerNameOrRange); }
    else { this.containerName = ''; this.location = containerNameOrRange; }
  }
}
class DocumentSymbol {
  constructor(name, detail, kind, range, selectionRange) {
    this.name = name; this.detail = detail; this.kind = kind;
    this.range = range; this.selectionRange = selectionRange; this.children = [];
  }
}
class FoldingRange {
  constructor(start, end, kind) { this.start = start; this.end = end; this.kind = kind; }
}
class SelectionRange {
  constructor(range, parent) { this.range = range; this.parent = parent; }
}
class CallHierarchyItem {
  constructor(kind, name, detail, uri, range, selectionRange) {
    this.kind = kind; this.name = name; this.detail = detail;
    this.uri = uri; this.range = range; this.selectionRange = selectionRange; this.tags = [];
  }
}
class TypeHierarchyItem {
  constructor(kind, name, detail, uri, range, selectionRange) {
    this.kind = kind; this.name = name; this.detail = detail;
    this.uri = uri; this.range = range; this.selectionRange = selectionRange; this.tags = [];
  }
}
class InlayHint {
  constructor(position, label, kind) { this.position = position; this.label = label; this.kind = kind; }
}
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
class CancellationError extends Error {
  constructor() { super('Canceled'); this.name = 'Canceled'; }
}

/// Split (options?, ...items) apart from a showXMessage rest-args array: an options
/// object is a plain (non-array) object, everything else is a button label (string
/// or MessageItem `{title}`).
function splitMessageArgs(message, rest) {
  const flat = rest.flat();
  let modal = false;
  const items = [];
  for (const it of flat) {
    if (it && typeof it === 'object' && !Array.isArray(it) && typeof it.title !== 'string') {
      modal = !!it.modal;
    } else {
      items.push(typeof it === 'string' ? it : it.title);
    }
  }
  return { message, items, modal };
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
  const formattingProviders = new Map(); // providerId -> {selector, provider}
  const commands = new Map();       // command -> callback
  const uriHandlers = [];           // window.registerUriHandler handlers (deep links)
  // Tree views: viewId -> { provider, elems: handle->element, cmds: handle->command, next }
  const treeViews = new Map();
  function registerTree(viewId, provider) {
    const t = { provider, elems: new Map(), cmds: new Map(), next: 1 };
    treeViews.set(viewId, t);
    rpc.notify('window/registerTreeView', { viewId });
    try {
      if (provider.onDidChangeTreeData) {
        provider.onDidChangeTreeData(() => rpc.notify('tree/didChange', { viewId }));
      }
    } catch (_) {}
    return new Disposable(() => treeViews.delete(viewId));
  }

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
    version: '1.106.0',
    Position, Range, Selection, MarkdownString, Hover, Disposable, Uri,
    EventEmitter: VsEvent,
    StatusBarAlignment,
    ThemeColor: class ThemeColor { constructor(id) { this.id = id; } },
    ThemeIcon,
    CancellationTokenSource,
    RelativePattern,
    ViewColumn: { Active: -1, Beside: -2, One: 1, Two: 2, Three: 3 },
    // Code-action kinds are dotted-string hierarchies in VSCode; extensions read
    // these constants at module load (Cline: CodeActionKind.QuickFix).
    CodeActionKind: (() => {
      const kind = (v) => ({ value: v, append: (p) => kind(v ? v + '.' + p : p), contains: (o) => String(o && o.value).startsWith(v) });
      return {
        Empty: kind(''),
        QuickFix: kind('quickfix'),
        Refactor: kind('refactor'),
        RefactorExtract: kind('refactor.extract'),
        RefactorInline: kind('refactor.inline'),
        RefactorMove: kind('refactor.move'),
        RefactorRewrite: kind('refactor.rewrite'),
        Source: kind('source'),
        SourceOrganizeImports: kind('source.organizeImports'),
        SourceFixAll: kind('source.fixAll'),
        Notebook: kind('notebook'),
      };
    })(),
    ProgressLocation: { SourceControl: 1, Window: 10, Notification: 15 },
    ExtensionMode: { Production: 1, Development: 2, Test: 3 },
    UIKind: { Desktop: 1, Web: 2 },
    ColorThemeKind: { Light: 1, Dark: 2, HighContrast: 3, HighContrastLight: 4 },
    ConfigurationTarget: { Global: 1, Workspace: 2, WorkspaceFolder: 3 },
    DiagnosticSeverity: { Error: 0, Warning: 1, Information: 2, Hint: 3 },
    TextEditorRevealType: { Default: 0, InCenter: 1, InCenterIfOutsideViewport: 2, AtTop: 3 },
    OverviewRulerLane: { Left: 1, Center: 2, Right: 4, Full: 7 },
    DecorationRangeBehavior: { OpenOpen: 0, ClosedClosed: 1, OpenClosed: 2, ClosedOpen: 3 },
    LanguageStatusSeverity: { Information: 0, Warning: 1, Error: 2 },
    FileType: { Unknown: 0, File: 1, Directory: 2, SymbolicLink: 64 },
    TreeItemCollapsibleState: { None: 0, Collapsed: 1, Expanded: 2 },
    TreeItem: class TreeItem { constructor(label, state) { this.label = label; this.collapsibleState = state || 0; } },
    CodeLens: class CodeLens { constructor(range, command) { this.range = range; this.command = command; } },
    CompletionItem: class CompletionItem {
      constructor(label, kind) { this.label = label; this.kind = kind; }
    },
    CompletionItemKind: {
      Text: 0, Method: 1, Function: 2, Constructor: 3, Field: 4, Variable: 5, Class: 6,
      Interface: 7, Module: 8, Property: 9, Unit: 10, Value: 11, Enum: 12, Keyword: 13,
      Snippet: 14, Color: 15, File: 16, Reference: 17, Folder: 18, EnumMember: 19,
      Constant: 20, Struct: 21, Event: 22, Operator: 23, TypeParameter: 24,
      User: 25, Issue: 26,
    },
    CompletionItemTag: { Deprecated: 1 },
    CompletionTriggerKind: { Invoke: 0, TriggerCharacter: 1, TriggerForIncompleteCompletions: 2 },
    CancellationError,
    Location, LocationLink, TextEdit, Diagnostic, DocumentLink, CodeAction,
    SymbolInformation, DocumentSymbol, FoldingRange, SelectionRange,
    CallHierarchyItem, TypeHierarchyItem, InlayHint,
    SymbolKind: {
      File: 0, Module: 1, Namespace: 2, Package: 3, Class: 4, Method: 5, Property: 6,
      Field: 7, Constructor: 8, Enum: 9, Interface: 10, Function: 11, Variable: 12,
      Constant: 13, String: 14, Number: 15, Boolean: 16, Array: 17, Object: 18,
      Key: 19, Null: 20, EnumMember: 21, Struct: 22, Event: 23, Operator: 24, TypeParameter: 25,
    },
    FoldingRangeKind: { Comment: 1, Imports: 2, Region: 3 },
    InlayHintKind: { Type: 1, Parameter: 2 },
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
      // Real signature is (message, options?, ...items) — options is only present
      // if the first arg after message is a non-string object ({modal, detail}).
      // Splitting it out here matters: leaving it in `items` shows a literal
      // "[object Object]"-shaped button (e.g. Gemini's `{"modal":true}` alongside
      // "Reload") instead of the actual button labels.
      showInformationMessage: (message, ...rest) =>
        rpc.request('window/showInformationMessage', splitMessageArgs(message, rest)),
      showErrorMessage: (message, ...rest) =>
        rpc.request('window/showErrorMessage', splitMessageArgs(message, rest)),
      showWarningMessage: (message, ...rest) =>
        rpc.request('window/showWarningMessage', splitMessageArgs(message, rest)),
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
      // The imperative QuickPick object (as opposed to the one-shot showQuickPick
      // promise above) — extensions build it up (title/items/placeholder) then call
      // .show(). No incremental/live-filter UI on aether's side yet, so `.show()`
      // is backed by the same modal dialog as showQuickPick; good enough for
      // extensions (Gemini Code Assist's own startup is the known one) that just
      // need the object to exist and its events to eventually fire.
      createQuickPick: () => {
        const selChange = new VsEvent();
        const activeChange = new VsEvent();
        const accept = new VsEvent();
        const hide = new VsEvent();
        const valueChange = new VsEvent();
        const triggerButton = new VsEvent();
        const qp = {
          items: [], selectedItems: [], activeItems: [],
          placeholder: '', title: undefined, value: '',
          canSelectMany: false, ignoreFocusOut: false,
          matchOnDescription: false, matchOnDetail: false,
          busy: false, enabled: true, step: undefined, totalSteps: undefined, buttons: [],
          onDidChangeSelection: selChange.event,
          onDidChangeActive: activeChange.event,
          onDidAccept: accept.event,
          onDidHide: hide.event,
          onDidChangeValue: valueChange.event,
          onDidTriggerButton: triggerButton.event,
          show() {
            const list = qp.items || [];
            rpc.request('window/showQuickPick', {
              items: list.map((it) => (typeof it === 'string' ? it : it.label)),
              placeHolder: qp.placeholder || '',
            }).then((picked) => {
              const found = picked == null ? undefined : list.find((it) => (typeof it === 'string' ? it : it.label) === picked);
              qp.selectedItems = found ? [found] : [];
              qp.activeItems = qp.selectedItems;
              activeChange.fire(qp.activeItems);
              selChange.fire(qp.selectedItems);
              if (found) accept.fire();
              hide.fire();
            });
          },
          hide() { hide.fire(); },
          dispose() {},
        };
        return qp;
      },
      createStatusBarItem: (alignment, priority) => makeStatusBarItem(alignment, priority),
      // Custom editors (webview-as-file-editor) aren't wired into aether yet —
      // register a no-op so activation doesn't crash (MPE registers one it only
      // uses for .ipynb).
      registerCustomEditorProvider: (viewType, provider, options) => ({ dispose() {} }),
      // ---- tree views: aether pulls items over RPC (tree/children) and shows
      // them with its native list widgets; item commands run via tree/invoke.
      registerTreeDataProvider: (viewId, provider) => registerTree(viewId, provider),
      createTreeView: (viewId, opts) => {
        const d = registerTree(viewId, (opts && opts.treeDataProvider) || { getChildren: () => [], getTreeItem: (x) => x });
        return {
          dispose: () => d.dispose(),
          onDidChangeSelection: new VsEvent().event,
          onDidChangeVisibility: new VsEvent().event,
          onDidCollapseElement: new VsEvent().event,
          onDidExpandElement: new VsEvent().event,
          reveal: () => Promise.resolve(),
          visible: true,
          selection: [],
          badge: undefined,
          title: '',
          description: '',
          message: '',
        };
      },
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
      // aether:// deep links (OAuth callbacks etc.) arrive via window/handleUri
      // and are fanned out to every registered handler (VSCode routes by owning
      // extension; with one host process the fan-out is equivalent in practice).
      registerUriHandler: (handler) => {
        uriHandlers.push(handler);
        return new Disposable(() => {
          const i = uriHandlers.indexOf(handler);
          if (i >= 0) uriHandlers.splice(i, 1);
        });
      },
      registerWebviewPanelSerializer: () => new Disposable(() => {}),
      createWebviewPanel: (viewType, title, _showOpts, _options) => {
        // A webview panel is just another webview instance; aether shows it in its
        // own webview-host window. Reuses the sidebar plumbing.
        const instanceId = nextPanelId--;
        const view = makeWebview(instanceId);
        view.viewType = viewType;
        rpc.notify('webview/createPanel', { instanceId, viewType, title });
        // IMPORTANT: the panel's dispose event must be the INSTANCE's event —
        // that's what webviewDisposed() fires when the user closes the tab in
        // aether. A separate event here left extensions (MPE) holding a stale
        // panel, so reopening the preview did nothing.
        const inst = webviewInstances.get(instanceId);
        const panel = {
          webview: view.webview,
          viewType, title, visible: true, active: true, viewColumn: 1,
          reveal: () => {},
          dispose: () => { rpc.notify('webview/disposePanel', { instanceId }); inst.onDispose.fire(); },
          get onDidDispose() { return view.onDidDispose; },
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
          // VSCode assembles intermediate objects from dotted leaf settings:
          // get('tree.buttons') -> { reveal: true, ... } built from
          // `todo-tree.tree.buttons.reveal` etc.
          const prefix = k + '.';
          const src = { ...configDefaults, ...ws.settings };
          let found = false;
          const out = {};
          for (const kk of Object.keys(src)) {
            if (!kk.startsWith(prefix)) continue;
            found = true;
            const rest = kk.slice(prefix.length).split('.');
            let node = out;
            for (let i = 0; i < rest.length - 1; i++) {
              if (typeof node[rest[i]] !== 'object' || node[rest[i]] === null) node[rest[i]] = {};
              node = node[rest[i]];
            }
            node[rest[rest.length - 1]] = src[kk];
          }
          return found ? out : undefined;
        };
        const cfg = {
          get: (key, dflt) => { const v = lookup(key); return v === undefined ? dflt : v; },
          has: (key) => lookup(key) !== undefined,
          inspect: (key) => ({ key: full(key), defaultValue: configDefaults[full(key)] }),
          update: (key, value) => {
            ws.settings[full(key)] = value;
            // Otherwise this only lives in THIS process's memory — a later
            // restart (extensions sometimes ask for one right after an update,
            // e.g. Gemini Code Assist's http.systemCertificatesNode) starts a
            // fresh host with none of it, so the setting silently reverts and
            // the extension re-prompts forever (#gemini-reload-loop).
            rpc.notify('workspace/didUpdateConfiguration', { key: full(key), value });
            return Promise.resolve();
          },
        };
        // VSCode also exposes settings as PROPERTIES of the config object
        // (`getConfiguration('todo-tree.general').tagGroups`) — materialize every
        // known key under this section, nesting the dotted remainder.
        if (section) {
          const prefix = section + '.';
          const keys = new Set([...Object.keys(configDefaults), ...Object.keys(ws.settings)]);
          for (const k of keys) {
            if (!k.startsWith(prefix)) continue;
            const rest = k.slice(prefix.length).split('.');
            let node = cfg;
            for (let i = 0; i < rest.length - 1; i++) {
              if (typeof node[rest[i]] !== 'object' || node[rest[i]] === null) node[rest[i]] = {};
              node = node[rest[i]];
            }
            const leaf = rest[rest.length - 1];
            if (!(leaf in node)) node[leaf] = lookup(k.slice(prefix.length));
          }
        }
        return cfg;
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
      uriScheme: 'aether', get appRoot() { return ws.appRoot || ''; }, shell: process.env.SHELL || '/bin/bash',
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
      registerDocumentFormattingEditProvider: (selector, provider) => {
        const providerId = nextProviderId++;
        const langs = (Array.isArray(selector) ? selector : [selector]).map((s) =>
          typeof s === 'string' ? s : (s && s.language) || '*');
        formattingProviders.set(providerId, { selector: langs, provider });
        rpc.notify('languages/registerFormattingProvider', { providerId, selector: langs });
        return new Disposable(() => formattingProviders.delete(providerId));
      },
      // Range-formatting isn't wired to Format Selection yet (only whole-document
      // Format Document is), but the provider must still register successfully —
      // Prettier's own registerGlobal() throws synchronously (killing activation
      // entirely) if this isn't at least a function.
      registerDocumentRangeFormattingEditProvider: () => new Disposable(() => {}),
      registerDocumentLinkProvider: () => new Disposable(() => {}),
      registerInlineCompletionItemProvider: () => new Disposable(() => {}),
      onDidChangeDiagnostics: () => new Disposable(() => {}),
      getDiagnostics: () => [],
      setTextDocumentLanguage: (doc) => Promise.resolve(doc),
      match: () => 0,
      createDiagnosticCollection: () => ({ set: () => {}, delete: () => {}, clear: () => {}, dispose: () => {} }),
      createLanguageStatusItem: (id, selector) => ({
        id, selector, name: '', text: '', detail: '', severity: 0,
        command: undefined, accessibilityInformation: undefined, busy: false,
        dispose: () => {},
      }),
    },
  };

  // ---- inbound dispatch (aether -> host), wired by main.js ----
  const dispatch = {
    // host/init + settings updates land here.
    setWorkspace({ root, settings, appRoot }) {
      // Defense in depth: a relative root breaks extensions' directory walks.
      if (root) ws.root = require('path').resolve(root);
      if (appRoot) ws.appRoot = appRoot; // VSCode-shaped dir carrying bundled ripgrep
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
    async provideFormatting({ providerId, uri, tabSize, insertSpaces }) {
      const entry = formattingProviders.get(providerId);
      if (!entry) return null;
      const doc = docs.get(uri);
      if (!doc) return null;
      const opts = { tabSize: tabSize || 2, insertSpaces: insertSpaces !== false };
      const res = await entry.provider.provideDocumentFormattingEdits(doc, opts, { isCancellationRequested: false });
      if (!Array.isArray(res)) return null;
      return res.map((e) => ({
        range: [e.range.start.line, e.range.start.character, e.range.end.line, e.range.end.character],
        newText: e.newText,
      }));
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
    // Fetch one level of a tree view: children of `handle` (0 = root). Elements
    // stay in the host keyed by handle; aether only sees display rows.
    async treeChildren({ viewId, handle }) {
      const t = treeViews.get(viewId);
      if (!t) return { items: [] };
      const parent = handle ? t.elems.get(handle) : undefined;
      const kids = (await t.provider.getChildren(parent)) || [];
      const items = [];
      for (const el of kids) {
        const h = t.next++;
        t.elems.set(h, el);
        let it;
        try { it = await t.provider.getTreeItem(el); } catch (e) { console.error('[tree] getTreeItem failed:', e && e.message); it = {}; }
        it = it || {};
        let label =
          typeof it.label === 'object' && it.label ? it.label.label || '' : String(it.label ?? '');
        // VSCode derives a missing label from resourceUri (file rows do this).
        if (!label && it.resourceUri) {
          const p = it.resourceUri.fsPath || String(it.resourceUri);
          label = require('path').basename(p);
        }
        if (it.command) t.cmds.set(h, it.command);
        items.push({
          handle: h,
          label,
          description: typeof it.description === 'string' ? it.description : '',
          collapsible: it.collapsibleState || 0, // 0 none, 1 collapsed, 2 expanded
          icon: (it.iconPath && it.iconPath.id) || null, // ThemeIcon name when present
        });
      }
      return { items };
    },
    async treeInvoke({ viewId, handle }) {
      const t = treeViews.get(viewId);
      const c = t && t.cmds.get(handle);
      if (!c) return;
      const cb = commands.get(c.command);
      if (cb) await cb(...(c.arguments || []));
    },
    // An aether:// deep link arrived (OAuth callback etc.) — hand it to every
    // registered uri handler as a vscode.Uri.
    handleUri({ uri }) {
      const u = Uri.parse(uri);
      for (const h of uriHandlers) {
        try { (h.handleUri || h)(u); } catch (e) { console.error('[uri-handler]', e.message); }
      }
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
