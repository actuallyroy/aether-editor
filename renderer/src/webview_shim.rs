// JS injected into every extension webview (both the Linux webview-host process
// and the in-process macOS webview): acquireVsCodeApi, VSCode theme variables,
// insecure-context polyfills, console/error forwarding, and cursor reporting.

pub const VSCODE_API_SHIM: &str = r#"
(function () {
  let state;
  window.acquireVsCodeApi = function () {
    return {
      postMessage: (data) => window.ipc.postMessage(JSON.stringify(data)),
      getState: () => state,
      setState: (s) => { state = s; return s; },
    };
  };
  // VSCode injects theme classes + --vscode-* CSS variables into every webview;
  // extension UIs are unstyled (white-on-white) without them. Dark+ defaults.
  const VARS = {
    'font-family': "'Segoe UI', Ubuntu, 'Droid Sans', sans-serif",
    'font-size': '13px',
    'font-weight': 'normal',
    'foreground': '#cccccc',
    'disabledForeground': '#cccccc80',
    'errorForeground': '#f48771',
    'descriptionForeground': '#ccccccb3',
    'icon-foreground': '#c5c5c5',
    'focusBorder': '#007fd4',
    'textLink-foreground': '#3794ff',
    'textLink-activeForeground': '#3794ff',
    'textCodeBlock-background': '#0a0a0a66',
    'textBlockQuote-background': '#7f7f7f1a',
    'textBlockQuote-border': '#007acc80',
    'textPreformat-foreground': '#d7ba7d',
    'textSeparator-foreground': '#ffffff2e',
    'editor-background': '#1e1e1e',
    'editor-foreground': '#d4d4d4',
    'editor-font-family': "'Droid Sans Mono', monospace",
    'editor-font-size': '14px',
    'editorWidget-background': '#252526',
    'editorWidget-border': '#454545',
    'editorGroup-border': '#444444',
    'editorGroupHeader-tabsBackground': '#252526',
    'tab-activeBackground': '#1e1e1e',
    'tab-activeForeground': '#ffffff',
    'tab-inactiveBackground': '#2d2d2d',
    'tab-inactiveForeground': '#ffffff80',
    'tab-border': '#252526',
    'sideBar-background': '#252526',
    'sideBar-foreground': '#cccccc',
    'sideBar-border': '#00000000',
    'sideBarTitle-foreground': '#bbbbbb',
    'sideBarSectionHeader-background': '#00000000',
    'sideBarSectionHeader-foreground': '#cccccc',
    'panel-background': '#1e1e1e',
    'panel-border': '#80808059',
    'panelTitle-activeForeground': '#e7e7e7',
    'button-background': '#0e639c',
    'button-foreground': '#ffffff',
    'button-hoverBackground': '#1177bb',
    'button-border': '#00000000',
    'button-secondaryBackground': '#3a3d41',
    'button-secondaryForeground': '#ffffff',
    'button-secondaryHoverBackground': '#45494e',
    'input-background': '#3c3c3c',
    'input-foreground': '#cccccc',
    'input-border': '#00000000',
    'input-placeholderForeground': '#cccccc80',
    'inputOption-activeBackground': '#007fd466',
    'inputOption-activeBorder': '#007acc00',
    'inputOption-activeForeground': '#ffffff',
    'dropdown-background': '#3c3c3c',
    'dropdown-foreground': '#f0f0f0',
    'dropdown-border': '#3c3c3c',
    'checkbox-background': '#3c3c3c',
    'checkbox-foreground': '#f0f0f0',
    'checkbox-border': '#6b6b6b',
    'list-activeSelectionBackground': '#04395e',
    'list-activeSelectionForeground': '#ffffff',
    'list-inactiveSelectionBackground': '#37373d',
    'list-hoverBackground': '#2a2d2e',
    'list-focusOutline': '#007fd4',
    'list-highlightForeground': '#2aaaff',
    'badge-background': '#4d4d4d',
    'badge-foreground': '#ffffff',
    'scrollbar-shadow': '#000000',
    'scrollbarSlider-background': '#79797966',
    'scrollbarSlider-hoverBackground': '#646464b3',
    'scrollbarSlider-activeBackground': '#bfbfbf66',
    'progressBar-background': '#0e70c0',
    'widget-shadow': '#0000005c',
    'widget-border': '#303031',
    'notifications-background': '#252526',
    'notifications-foreground': '#cccccc',
    'notifications-border': '#303031',
    'quickInput-background': '#252526',
    'quickInput-foreground': '#cccccc',
    'menu-background': '#252526',
    'menu-foreground': '#cccccc',
    'menu-selectionBackground': '#04395e',
    'toolbar-hoverBackground': '#5a5d5e50',
    'keybindingLabel-background': '#8080802b',
    'keybindingLabel-foreground': '#cccccc',
    'keybindingLabel-border': '#33333399',
    'keybindingLabel-bottomBorder': '#44444499',
    'charts-blue': '#3794ff',
    'charts-red': '#f48771',
    'charts-green': '#89d185',
    'charts-yellow': '#cca700',
    'terminal-foreground': '#cccccc',
    'terminal-background': '#1e1e1e',
    'banner-background': '#04395e',
    'banner-foreground': '#cccccc',
  };
  function applyTheme() {
    const root = document.documentElement;
    for (const [k, v] of Object.entries(VARS)) root.style.setProperty('--vscode-' + k, v);
    root.style.colorScheme = 'dark';
    root.setAttribute('data-vscode-theme-kind', 'vscode-dark');
    root.setAttribute('data-vscode-theme-name', 'Dark+');
    if (document.body) {
      document.body.classList.add('vscode-dark');
      document.body.setAttribute('data-vscode-theme-kind', 'vscode-dark');
      if (!document.body.style.color) document.body.style.color = 'var(--vscode-foreground)';
      if (!document.body.style.fontFamily) document.body.style.fontFamily = 'var(--vscode-font-family)';
    }
  }
  applyTheme();
  document.addEventListener('DOMContentLoaded', applyTheme);
  // aether-res:// isn't a browser "secure context", so secure-only APIs are absent.
  // Polyfill the ones extension UIs actually use.
  if (!crypto.randomUUID) {
    crypto.randomUUID = function () {
      const b = crypto.getRandomValues(new Uint8Array(16));
      b[6] = (b[6] & 0x0f) | 0x40; b[8] = (b[8] & 0x3f) | 0x80;
      const h = [...b].map((x) => x.toString(16).padStart(2, '0')).join('');
      return `${h.slice(0,8)}-${h.slice(8,12)}-${h.slice(12,16)}-${h.slice(16,20)}-${h.slice(20)}`;
    };
  }
  // localStorage/sessionStorage throw "The operation is insecure" on custom-scheme
  // origins in WebKit — swap in in-memory replacements when unusable.
  function memStorage() {
    const m = new Map();
    return {
      get length() { return m.size; },
      key: (i) => [...m.keys()][i] ?? null,
      getItem: (k) => (m.has(String(k)) ? m.get(String(k)) : null),
      setItem: (k, v) => m.set(String(k), String(v)),
      removeItem: (k) => m.delete(String(k)),
      clear: () => m.clear(),
    };
  }
  for (const name of ['localStorage', 'sessionStorage']) {
    let broken = false;
    try { window[name].setItem('__aether_t', '1'); window[name].removeItem('__aether_t'); }
    catch (_) { broken = true; }
    if (broken) {
      try { Object.defineProperty(window, name, { value: memStorage(), configurable: true }); } catch (_) {}
    }
  }
  // Surface page errors/warnings to the host (→ aether's stderr) for debugging.
  const fwd = (kind) => (...args) => {
    try {
      window.ipc.postMessage(JSON.stringify({ __aetherConsole: kind, msg: args.map((a) => {
        if (a instanceof Error) return a.stack || a.message;
        try { return typeof a === 'string' ? a : JSON.stringify(a); } catch (_) { return String(a); }
      }).join(' ').slice(0, 2000) }));
    } catch (_) {}
  };
  // When embedded (X11 reparented), a click inside the page must pull keyboard
  // focus to this window — the editor can't see clicks on a child X window.
  window.addEventListener('pointerdown', () => {
    try { window.ipc.postMessage(JSON.stringify({ __aetherFocus: true })); } catch (_) {}
  }, true);
  // Report the page's effective cursor so the editor can mirror it.
  let __lastCursor = '';
  document.addEventListener('pointerover', (e) => {
    try {
      let c = getComputedStyle(e.target).cursor || 'default';
      if (c === 'auto') {
        const t = e.target;
        const editable = t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable);
        c = editable ? 'text' : 'default';
      }
      if (c !== __lastCursor) {
        __lastCursor = c;
        window.ipc.postMessage(JSON.stringify({ __aetherCursor: c }));
      }
    } catch (_) {}
  }, true);
  const origErr = console.error, origWarn = console.warn;
  console.error = (...a) => { fwd('error')(...a); origErr.apply(console, a); };
  console.warn = (...a) => { fwd('warn')(...a); origWarn.apply(console, a); };
  window.addEventListener('error', (e) => fwd('onerror')(e.message + ' @ ' + e.filename + ':' + e.lineno));
  window.addEventListener('unhandledrejection', (e) => {
    const r = e.reason;
    fwd('unhandledrejection')(r && r.message ? r.message + '\n' + (r.stack || '') : String(r));
  });
})();
"#;
