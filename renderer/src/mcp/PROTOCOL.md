# Claude Code IDE integration protocol (captured from anthropic.claude-code 2.1.175)

Reverse-engineered from the VS Code extension's `extension.js`. Pin to this; re-verify on
Claude Code updates (protocol is undocumented and drifts).

## Discovery (lockfile)
- Dir: `~/.claude/ide/`  (created mode `0700`)
- File: `~/.claude/ide/<port>.lock`  (written mode `0600`)
- Port: random in **[10000, 65535]**  (`Math.floor(random()*55536)+10000`)
- JSON body:
  ```json
  {
    "pid": <process pid>,
    "workspaceFolders": ["<abs fsPath>", ...],
    "ideName": "<app display name>",   // e.g. "Aether"
    "transport": "ws",
    "runningInWindows": <bool>,
    "authToken": "<random token, e.g. UUID>"
  }
  ```

## Terminal env (auto-detect trigger)
- Inject `CLAUDE_CODE_SSE_PORT=<port>` into shells spawned by the editor's integrated
  terminal. `claude` reads it, finds the matching `<port>.lock`, connects.
- (`CLAUDE_CODE_ENTRYPOINT` is also set by the CLI; not required from the IDE side.)

## Transport + auth
- **WebSocket** server on `127.0.0.1:<port>` (the same port as the lockfile name).
- Client connects with HTTP header `x-claude-code-ide-authorization: <authToken>`.
- MCP JSON-RPC 2.0 over the socket; `initialize` advertises `protocolVersion: "2024-11-05"`.
- Editor is the MCP **server**; `claude` is the client. Tools surface to the model as
  `mcp__ide__<tool>`.

## Native tools the extension registers (name, schema, purpose)
- `openDiff(old_file_path, new_file_path, new_file_contents, tab_name)` — open a diff for
  review; reply blocks until the user accepts/rejects (FILE_SAVED / DIFF_REJECTED).
- `openFile(filePath, preview=false, startText?, endText?, ...)` — open + optional select.
- `getDiagnostics(uri?)` — diagnostics for a file or all.
- `getCurrentSelection()` / `getLatestSelection()` — active selection.
- `getOpenEditors()` — open tabs.
- `getWorkspaceFolders()` — workspace roots.
- `checkDocumentDirty(filePath)` / `saveDocument(filePath)`.
- `close_tab(tab_name)` / `closeAllDiffTabs()`.
- `executeCode(...)` — Jupyter only; out of scope for Aether.

## Notes for Aether
- Use our process pid; `workspaceFolders=[App.cwd]`; `ideName="Aether"`.
- Custom (non-VS-Code) tools added to `tools/list` are expected to surface as
  `mcp__ide__<tool>` — VERIFY empirically with one bogus tool before relying on it.
