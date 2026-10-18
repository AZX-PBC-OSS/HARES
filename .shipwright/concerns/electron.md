---
description: Electron process-boundary patterns — contextBridge subscriptions, sandbox restrictions, IPC discipline
triggers:
  file_globs:
    - "**/preload/**/*"
    - "**/preload.ts"
    - "**/preload.js"
    - "**/preload.cjs"
    - "**/electron.vite.config.*"
    - "**/electron-builder.*"
    - "**/forge.config.*"
  content_patterns:
    - "contextIsolation:\\s*false"
    - "nodeIntegration:\\s*true"
    - "sandbox:\\s*false"
    - "webSecurity:\\s*false"
---

# Electron

Modern Electron apps run with `contextIsolation: true`, `sandbox: true`, `nodeIntegration: false`, and `webSecurity: true`. These are current defaults (`contextIsolation` since Electron 12, `sandbox` since 20), and they interact: setting `nodeIntegration: true` or `contextIsolation: false` disables sandboxing for that renderer regardless of the `sandbox` flag. The patterns below assume the secure defaults; weakening them is almost never the right answer to a problem you've hit.

## contextBridge subscribe / unsubscribe — closure-scoped wrapper

`ipcRenderer.removeListener` requires reference equality with the function passed to `ipcRenderer.on`. Functions crossing the contextBridge are proxied into a new identity in the receiving context, so the renderer never holds the reference `ipcRenderer.on` received and cannot later remove it (`electron/electron#33328`). The fix: keep the listener function inside the preload closure and return an unsubscribe closure to the renderer.

```ts
// preload
onSomeEvent: (callback: (event: SomeEvent) => void) => {
  const listener = (_event: IpcRendererEvent, ...args: unknown[]): void => {
    const [payload] = args
    callback(payload as SomeEvent)
  }
  ipcRenderer.on('app:some-event', listener)
  return (): void => {
    ipcRenderer.removeListener('app:some-event', listener)
  }
}
```

The wrapper stays in preload-closure scope; only the unsubscribe closure crosses the bridge. The single `as SomeEvent` cast is the documented IPC-boundary cast — TypeScript cannot verify cross-process payload types statically. Keep the cast confined to this one line.

Never expose `ipcRenderer.on` directly. Whatever shape the API takes, the renderer must end up holding something that can cancel the subscription — a returned closure, or a token a companion `off` method accepts. A subscribe method returning `void` leaks a listener per call.

## Node availability: renderer vs preload

Two different gates, routinely confused.

- **The renderer** has no Node environment at all under the default `nodeIntegration: false` — no `require`, no builtins, no `Buffer`, no `process` beyond Electron's shim. That is true independently of `sandbox`.
- **A sandboxed preload** does get a polyfilled `require`, but only over a fixed subset: `events`, `timers`, `url` (and their `node:`-prefixed forms), plus a subset of `electron` — `contextBridge`, `crashReporter`, `ipcRenderer`, `nativeImage`, `webFrame`, `webUtils`. Everything else — `fs`, `path`, `child_process`, `os`, `crypto`, `stream` — is unavailable there too.

Implications for renderer code:

- Node builtins are not importable. Depending on the bundler this fails at build time, or is externalised and throws on first property access — either way it is not a runtime you can rely on.
- Renderer code should not construct filesystem paths at all. Paths are main-process data: resolve them in main and pass them across the bridge already-formed. Joining strings in the renderer hand-rolls a platform assumption with no normalisation.
- Node's `crypto.randomUUID()` is unavailable. Use `globalThis.crypto.randomUUID()` (Web Crypto), which requires a secure context — satisfied by `file://` and `http://localhost`, but by a custom scheme only if it was registered with `secure: true`.
- File-system access only through main-process IPC handlers. The renderer never reads or writes disk directly.
- Use `Uint8Array` / `ArrayBuffer` and the Web `TextEncoder` / `TextDecoder` in place of `Buffer`.

If a renderer file needs Node functionality, the right answer is "expose it from main through the contextBridge," not "weaken the sandbox."

## Process-boundary imports — enforced at three layers

1. **Compiler configuration** — whatever the project uses to give each process its own module graph and lib set, so main-only globals are not in scope for renderer code.
2. **A lint rule that blocks cross-boundary imports** — renderer must not import from main or preload; main and preload must not import from renderer. Whichever linter the project runs, the boundary needs to fail the build, not rely on reviewer attention.
3. **Runtime contextBridge** — only declared methods are accessible from the renderer side.

Test code crosses the boundary as easily as production code, and a test that reaches across it couples the test's build config to the process it reached into. Where a suite runs against built output rather than source, that coupling is also a lie about what is being tested.

## `ipcMain.handle` does NOT respond to `ipcMain.emit`

`ipcMain.handle` registers an invoke/handle channel managed by Electron's IPC layer, not the bare `EventEmitter` contract. Calling `ipcMain.emit('channel', ...)` silently does nothing for a `handle`-style channel. Tests that try to trigger handlers via `emit` produce false negatives — handlers appear "unreachable" because they are.

The correct test pattern: drive the handler through the same path production uses — `page.evaluate(() => window.api.someMethod(...))` in Playwright `_electron`, or call the handler function directly in a unit test that bypasses IPC entirely.

## Push channels (main → renderer) vs invoke channels — keep them typed separately

Request/response (`invoke`/`handle`) and push (`webContents.send` → renderer subscription) are different shapes, and a channel-keyed type map that tries to cover both produces incoherent generics: for an invoke channel the interesting types are *arguments and return value*, for a push channel there is a single *payload* and no response at all. Give each its own map. Subscribe channels then go through the closure pattern above; invoke channels through a wrapper generic over the channel key, so arguments and result are bound together at the call site rather than asserted.

## Preload bridge: `satisfies <YourAPI>` at the declaration site

```ts
const api = {
  ping: (): Promise<'pong'> => ipcRenderer.invoke('app:ping'),
  // ...
} satisfies AppAPI
contextBridge.exposeInMainWorld('app', api)
```

Without `satisfies`, adding a member to `AppAPI` does not fail the preload typecheck — the missing method surfaces as a `TypeError` at the renderer call site (or in a test) instead. The `satisfies` operator catches drift at the declaration site, where the fix is local.

## Atomic writes for any file the watcher is observing

If you have a Chokidar (or any FS) watcher and your write path uses `fs.writeFile` directly, an editor reading mid-write will see partial bytes and the watcher will see two events for one logical save. The standard fix is write-to-temp + `fs.rename`:

```ts
async function writeAtomic(destPath: string, contents: string): Promise<void> {
  const tmpPath = `${destPath}.${randomUUID()}.tmp`
  await fs.writeFile(tmpPath, contents)
  await fs.rename(tmpPath, destPath)
}
```

Pair this with a filter in the watcher's event handler that skips whatever temp-name convention the writer uses, and lean on Chokidar's `atomic` option — enabled by default unless `usePolling` is on — which coalesces `unlink`+`add` on the same destination path within 100ms (settable by passing a number instead of `true`) into a `change` event. The two layers compose: `atomic` smooths destination-path event sequences but only auto-ignores editor swap files, so your own temp names still need the explicit filter.

Tests that exercise the watcher must write through the same atomic path the production code uses. Calling `fs.writeFile` directly bypasses it and will pass with the filter logic broken.

## Single-instance and command-line argv

Desktop apps that own persistent state usually want `app.requestSingleInstanceLock()` and a `second-instance` handler, so a second launch focuses the existing window rather than starting a rival one. The handler receives the second process's `argv` and working directory, plus any `additionalData` the second instance passed to the lock call — that is the supported channel for forwarding structured launch context, not just a command line. Without the lock, a user launching twice gets two windows and two competing watchers and handlers — confusing on Linux, routine on Windows where file associations launch a new instance per double-click.

## What NOT to flag

- A direct `ipcRenderer.invoke` in preload — that is where it belongs. The rule is that the *renderer* never touches `ipcRenderer`.
- The single cast at the IPC deserialisation point. Payloads arrive untyped; one narrow, located cast is the boundary working as designed, not an escape hatch.
- A push channel with no unsubscribe when the subscriber lives for the lifetime of the window and the listener count cannot grow. The leak is per-subscribe-call, so a module-scope subscription taken once is not one.
- `nodeIntegration` or `sandbox` settings on a `webContents` that loads no remote or user-supplied content, where the project has made that call deliberately and said so.
- Absence of a single-instance lock in an app that owns no exclusive resource — no watcher, no lock file, no local server. Multiple windows are then a feature.
- Main-process code using Node freely. The restrictions in this document are about the renderer and preload; main is a Node process.

## Footguns

- **`require('electron')` in a sandboxed renderer** — throws. Renderer-side code uses the contextBridge surface only.
- **Async `ipcMain.handle` that throws** — the rejection reaches the renderer's `invoke` Promise carrying the message and a stack marked at the IPC boundary. Any discriminant the caller needs to act on is lost. Catch in main and return a typed error shape.
- **`webContents.send(channel, arg1, arg2)` with multiple args** — the renderer-side listener receives `(_event, arg1, arg2, ...)` and the typed payload mapping breaks. Send a single payload object.
- **`win.webContents.send` after destruction** — throws `TypeError: Object has been destroyed`, on every platform. Guard with `isDestroyed()` on the object you are about to call; `webContents` can be gone while the window object is still reachable.
- **Disabling `contextIsolation` or `sandbox` to "fix" a problem** — the problem is somewhere else. Find it.
- **Bundling a sandboxed preload as ESM** — sandboxed preloads run as plain JavaScript with no ESM context, whatever the extension; `import` throws and the contextBridge never attaches. Bundle to CommonJS. Unsandboxed preloads *can* use ESM, but only via the `.mjs` extension — `"type": "module"` is ignored for preloads.
