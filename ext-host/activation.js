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
    const context = {
      subscriptions: [],
      extensionPath: extPath,
      extensionUri: { fsPath: extPath, toString: () => 'file://' + extPath },
      globalState: { get: () => undefined, update: () => Promise.resolve(), setKeysForSync: () => {} },
      workspaceState: { get: () => undefined, update: () => Promise.resolve(), setKeysForSync: () => {} },
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
