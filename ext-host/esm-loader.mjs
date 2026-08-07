// Node ESM loader hook: makes `import ... from "vscode"` resolve for
// ESM-bundled extensions (esbuild `"type": "module"` output — Prettier's real
// build is the known one). Without this, only CommonJS `require("vscode")`
// worked (patched via `Module._load` in activation.js) — any extension whose
// bundle uses ESM `import` syntax got `ERR_MODULE_NOT_FOUND` and never
// activated (#prettier-esm-not-found).
//
// How it bridges to the SAME live shim singleton: `resolve()`/`load()` run in
// Node's internal loader thread, but the SOURCE TEXT they return is executed
// back in the main thread as a real ES module. So the generated module's own
// `require('vscode')` call (via `createRequire`) runs in the main thread and
// hits the process-wide `Module._load` patch `installVscodeModule()` installs
// there — the exact same object every CJS extension already gets.
import { createRequire } from 'node:module';
import { fileURLToPath, pathToFileURL } from 'node:url';
import path from 'node:path';

const hostDir = path.dirname(fileURLToPath(import.meta.url));
const hostMainUrl = pathToFileURL(path.join(hostDir, 'main.js')).href;

// Named exports (`import { commands } from "vscode"`) need static bindings —
// enumerate the shim's real key set once, from a throwaway instance (this one
// is discarded; it's only used to read property names, never to serve values).
function vscodeKeys() {
  try {
    const req = createRequire(import.meta.url);
    const { createVscode } = req('./vscode-shim.js');
    const { vscode } = createVscode({ request: () => Promise.resolve(null), notify: () => {} });
    return Object.keys(vscode).filter((k) => /^[A-Za-z_$][A-Za-z0-9_$]*$/.test(k));
  } catch (_) {
    return [];
  }
}
const KEYS = vscodeKeys();
const SHIM_URL = 'aether-vscode:shim';

export async function resolve(specifier, context, nextResolve) {
  if (specifier === 'vscode') {
    return { url: SHIM_URL, shortCircuit: true };
  }
  return nextResolve(specifier, context);
}

export async function load(url, context, nextLoad) {
  if (url === SHIM_URL) {
    const lines = [
      `import { createRequire } from 'node:module';`,
      `const require = createRequire(${JSON.stringify(hostMainUrl)});`,
      `const vscode = require('vscode');`,
      `export default vscode;`,
      ...KEYS.map((k) => `export const ${k} = vscode[${JSON.stringify(k)}];`),
    ];
    return { format: 'module', source: lines.join('\n'), shortCircuit: true };
  }
  return nextLoad(url, context);
}
