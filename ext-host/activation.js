// Extension loading + activation. Makes `require('vscode')` resolve to our shim
// instance, reads the extension's package.json, requires its `main`, and calls
// `activate(context)`. Activation *timing* (which activationEvents fire) is decided by
// aether — it sends `activate {extensionPath}` when appropriate; this just loads+runs.
'use strict';

const path = require('path');
const fs = require('fs');
const Module = require('module');

// Route every `require('vscode')` (from any extension) to our single shim instance.
function installVscodeModule(vscode) {
  const orig = Module._load;
  Module._load = function (request, parent, isMain) {
    if (request === 'vscode') return vscode;
    return orig.apply(this, arguments);
  };
}

// Load + activate an extension by directory. Returns { ok } or { ok:false, error }.
async function activateExtension(extPath, hostLog, dispatch) {
  try {
    const pkgPath = path.join(extPath, 'package.json');
    const pkg = JSON.parse(fs.readFileSync(pkgPath, 'utf8'));
    // Register contributed configuration defaults BEFORE activate() runs — the
    // extension may read its config during activation.
    if (dispatch && dispatch.registerContributes) dispatch.registerContributes(pkg, extPath);
    const mainRel = pkg.main || './extension.js';
    const mainPath = require.resolve(path.resolve(extPath, mainRel));
    const mod = require(mainPath);
    const vscode = require('vscode');
    const Uri = vscode.Uri;
    // Persistent JSON-file-backed state store (globalState/workspaceState/secrets).
    const storageDir = path.join(require('os').homedir(), '.aether', 'ext-storage', pkg.name || 'ext');
    fs.mkdirSync(storageDir, { recursive: true });
    const makeStore = (file) => {
      const p = path.join(storageDir, file);
      let data = {};
      try { data = JSON.parse(fs.readFileSync(p, 'utf8')); } catch (_) {}
      const flush = () => { try { fs.writeFileSync(p, JSON.stringify(data)); } catch (_) {} };
      return {
        get: (k, d) => (k in data ? data[k] : d),
        update: (k, v) => { if (v === undefined) delete data[k]; else data[k] = v; flush(); return Promise.resolve(); },
        keys: () => Object.keys(data),
        setKeysForSync: () => {},
        // secrets API shape rides on the same store:
        store: (k, v) => { data[k] = v; flush(); return Promise.resolve(); },
        delete: (k) => { delete data[k]; flush(); return Promise.resolve(); },
        onDidChange: () => ({ dispose: () => {} }),
      };
    };
    const context = {
      subscriptions: [],
      extensionPath: extPath,
      extensionUri: Uri.file(extPath),
      extensionMode: 1, // Production
      extension: {
        id: `${pkg.publisher || 'unknown'}.${pkg.name || 'ext'}`,
        packageJSON: pkg, extensionPath: extPath, extensionUri: Uri.file(extPath),
        isActive: true, exports: undefined,
      },
      globalState: makeStore('global.json'),
      workspaceState: makeStore('workspace.json'),
      secrets: makeStore('secrets.json'),
      globalStorageUri: Uri.file(path.join(storageDir, 'globalStorage')),
      storageUri: Uri.file(path.join(storageDir, 'workspaceStorage')),
      logUri: Uri.file(path.join(storageDir, 'logs')),
      globalStoragePath: path.join(storageDir, 'globalStorage'),
      storagePath: path.join(storageDir, 'workspaceStorage'),
      logPath: path.join(storageDir, 'logs'),
      environmentVariableCollection: {
        replace: () => {}, append: () => {}, prepend: () => {}, get: () => undefined,
        forEach: () => {}, delete: () => {}, clear: () => {}, persistent: false,
        getScoped: () => ({ replace: () => {}, append: () => {}, prepend: () => {} }),
      },
      languageModelAccessInformation: { canSendRequest: () => undefined, onDidChange: () => ({ dispose: () => {} }) },
      asAbsolutePath: (rel) => path.join(extPath, rel),
    };
    if (typeof mod.activate === 'function') {
      await mod.activate(context);
    }
    // Stash for deactivate.
    activateExtension._loaded.set(extPath, { mod, context });
    if (hostLog) hostLog('info', `activated ${pkg.name || extPath}`);
    return { ok: true };
  } catch (e) {
    if (hostLog) hostLog('error', `activate failed: ${e && e.stack || e}`);
    return { ok: false, error: String((e && e.message) || e) };
  }
}
activateExtension._loaded = new Map();

async function deactivateExtension(extPath) {
  const entry = activateExtension._loaded.get(extPath);
  if (entry && typeof entry.mod.deactivate === 'function') {
    try { await entry.mod.deactivate(); } catch (_) {}
  }
  activateExtension._loaded.delete(extPath);
  return { ok: true };
}

module.exports = { installVscodeModule, activateExtension, deactivateExtension };
