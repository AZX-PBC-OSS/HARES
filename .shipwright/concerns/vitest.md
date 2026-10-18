---
description: Vitest 4 testing discipline — typed mocks, timer hygiene, module mocking, clear/reset/restore semantics
triggers:
  file_globs:
    - "**/*.test.ts"
    - "**/*.test.tsx"
    - "**/*.test.js"
    - "**/*.test.jsx"
    - "**/*.spec.ts"
    - "**/*.spec.tsx"
    - "**/*.spec.js"
    - "**/*.spec.jsx"
    - "**/vitest.config.*"
    - "**/vitest.workspace.*"
    - "**/vitest.setup.*"
    - "**/vite.config.*"
  content_patterns:
    - "vi\\.mock\\("
    - "vi\\.spyOn\\("
    - "vi\\.useFakeTimers\\("
---

# Vitest 4

Tests must be precise. Flaky or loosely typed tests create false confidence and mask real bugs. The rules below exist because each was an incident.

`vite.config.*` triggers this concern as well as `vitest.config.*`: a project with no separate Vitest config keeps its `test` block (including `coverage.thresholds`, `sequence.shuffle`, `pool`, and the setup files) in the Vite config, so a change lowering a coverage floor or disabling shuffle arrives in a file whose name says nothing about tests. The build half of that same file is the node packaging concern's.

## Typed mocks only — no bare `vi.fn()` for async

```ts
// NO — returns untyped mock, silently resolves to undefined
const mock = vi.fn()

// YES — full signature, autocompleted returns, compile-time contract
const mock = vi.fn<(id: string) => Promise<User>>()
mock.mockResolvedValue({ id: '1', name: 'Ada' })
```

A bare `vi.fn()` stubbing an async API produces `undefined` from the mocked call. `.then(...)` on `undefined` throws at runtime, not at typecheck — so a missing mock method blows up inside a React effect instead of failing the build. Every Promise-returning mock must be typed.

## `vi.mocked` after `vi.mock`

```ts
import { fetchUser } from '../api/users'
vi.mock('../api/users')

vi.mocked(fetchUser).mockResolvedValue({ id: '1', name: 'Ada' })

// Partial for large shapes where only one field matters
vi.mocked(fetchSomething, { partial: true }).mockResolvedValue({ ok: false })
```

## Module mock: spread the real module, never hand-list its surface

Prefer the import-expression form — it spreads the real surface and types `importOriginal` automatically:

```ts
vi.mock(import('../api/users'), async (importOriginal) => {
  const actual = await importOriginal()   // typed as typeof import('../api/users')
  return {
    ...actual,
    fetchUser: vi.fn<(id: string) => Promise<User>>()
      .mockResolvedValue({ id: '1', name: 'Ada' }),
  }
})
```

A factory that returns a hand-written object literal *instead of* spreading the original is a defect, not a style preference. The literal is a copy of the module's surface that nothing checks against the module: rename an export or add a sibling one, and the factory keeps serving the old shape. Production code then calls an export the double doesn't have, gets `TypeError: ... is not a function`, and the calling code's own error handling frequently catches it and converts it to some generic failure — so the suite goes red *somewhere else entirely*, as an unrelated assertion mismatch in a file that never mentions the renamed symbol. Nobody can see the cause from the failure. Spreading `await importOriginal()` and overriding named keys keeps the surface derived from the real module, so drift surfaces at the override rather than as mystery breakage downstream.

A wholly hand-written factory is still the right call in two cases: mocking a module you do not own (there is no surface of yours to stay in sync with), or one whose import-time side effects — opening connections, registering globals, reading env — are exactly what you are avoiding by never loading the original. Keep those literals minimal, and expect them to need manual updating.

The string-path form (`vi.mock('../api/users', ...)`) requires manual typing on `importOriginal` — avoid unless needed.

## `vi.spyOn` vs `vi.fn` vs `vi.mock`

- `vi.spyOn(obj, 'method')` — wraps an existing method, tracks calls, original still runs unless overridden. Use for partial instrumentation.
- `vi.fn<Sig>()` — standalone stub, no real implementation. Use for callbacks, event handlers, API doubles you construct.
- `vi.mock(module)` — replaces an entire imported module at the boundary. Use when every import of the module must be stubbed across the file.

## Clear / reset / restore — these are different

| Helper | What it does | When to use |
|---|---|---|
| `vi.clearAllMocks()` | Clears call history, keeps implementation | `beforeEach` when implementations are set once in the file |
| `vi.resetAllMocks()` | Clears history AND resets each mock to the implementation it was *created* with — `vi.fn(impl)` goes back to `impl`, `vi.fn()` goes back to returning `undefined`. Also drops `mockImplementationOnce` / `mockResolvedValueOnce` queues | `beforeEach` when per-test behaviour is layered on top of a baseline set at construction |
| `vi.restoreAllMocks()` | Does what `reset` does, and additionally restores the original property descriptors of `vi.spyOn` targets | `afterEach` in files that use `vi.spyOn` |

Note that these are **not** Jest's semantics: Jest's `mockReset` wipes the implementation unconditionally, Vitest's restores the construction-time one. What `reset` actually costs you is anything configured *after* construction — `mockResolvedValue` set in a `beforeAll`, or a `once` queue. So build the baseline into the factory (`vi.fn<Sig>(() => defaultValue)`) and `reset` is safe and self-cleaning; configure the baseline after construction and `reset` silently removes it.

Default: `afterEach(() => { vi.restoreAllMocks() })` if you use `vi.spyOn` anywhere in the file. `vi.clearAllMocks()` in `beforeEach` is the conservative choice when implementations are assigned once outside the mocks' construction.

## Fake timers — always paired

```ts
describe('time-sensitive code', () => {
  beforeEach(() => { vi.useFakeTimers() })
  afterEach(() => { vi.useRealTimers() })

  it('advances time', async () => {
    // ... code under test
    await vi.advanceTimersByTimeAsync(2000)
  })
})
```

**Never** call `vi.useFakeTimers()` inside a test without a guaranteed restore. A leak makes every subsequent test in the file (and potentially other files, depending on runner order) run against a frozen clock — `setTimeout` never fires, watchers hang, tests time out for apparently unrelated reasons.

Scope fake timers to the narrowest `describe` block that needs them. If only one test in a file needs them, wrap with `try { ... } finally { vi.useRealTimers() }` inside the test itself.

## Async timer variants for Promise chains

Use the async variants (`advanceTimersByTimeAsync`, `runAllTimersAsync`, `runOnlyPendingTimersAsync`) whenever the code under test chains Promises. Sync variants skip microtask flushes and silently deadlock.

## Control what you fake

```ts
vi.useFakeTimers({ toFake: ['Date', 'setTimeout', 'clearTimeout'] })
vi.setSystemTime(new Date('2026-04-16T00:00:00Z'))
```

If you only need to pin `Date.now()`, don't fake `setTimeout` too — fewer faked primitives means fewer accidental hangs.

## Timeouts are not a fix

If a test times out, find the real cause. Do NOT:

- Bump `testTimeout`
- Switch to `--sequential`
- Add `.skip` / `.todo`
- Sprinkle `vi.resetAllMocks()` until it passes

A hang is almost always one of: timer-mode leakage, unresolved promise, missing `advanceTimers` option on `userEvent.setup()`, or a mock that returns `undefined` instead of a Promise. Debug the actual cause.

## Centralize API mocks

If multiple test files need the same complex object stubbed (e.g. an injected API surface, a window-attached bridge), write a factory:

```ts
// test-utils/mock-api.ts
export function makeMockApi(overrides: Partial<AppAPI> = {}): AppAPI {
  return {
    ping: vi.fn<() => Promise<'pong'>>().mockResolvedValue('pong'),
    // ... every method typed explicitly
    ...overrides,
  } satisfies AppAPI
}
```

The `satisfies AppAPI` makes the factory fail the build when a new API method lands without being added to the mock. Every test imports from one place — no drift.

## Beyond passing and covered

Coverage percentage says nothing about whether a test would actually catch a regression.

- **Mutation testing** — [StrykerJS](https://stryker-mutator.io/) (`@stryker-mutator/core` + [`@stryker-mutator/vitest-runner`](https://stryker-mutator.io/docs/stryker-js/vitest-runner/)) reruns the suite against deliberately broken copies of the source; a mutant that survives (no test fails) means that line is covered but not actually protected. Worth running on code you'd bet on, not the whole repo — it's slow, one mutant-run per surviving line.
- **Randomized order** — `test.sequence.shuffle: true` in `vitest.config.ts` (pin a failure with `test.sequence.seed`) reorders test files and, within a file, tests, natively — no plugin needed. Catches a test that only passes because an earlier test left shared state behind.
- **Real dependencies over mocked semantics** — a hand-rolled mock replays what you believe a database/queue/service does, not what it does. [`testcontainers`](https://node.testcontainers.org/) spins up the real thing in Docker for the test's lifetime. Reach for it where the interaction under test is the dependency's actual behaviour (transactions, constraints, ordering) — keep a plain `vi.fn()` for boundaries where faking the shape honestly is enough.

## What NOT to flag

- `vi.clearAllMocks()` in `beforeEach` in a file that never uses `vi.spyOn` — there's nothing for `restoreAllMocks` to additionally restore, so `clearAllMocks` is not under-cleaning.
- A sync `vi.advanceTimersByTime` call where nothing in the code under test awaits a Promise — the async variant is only required when a microtask needs to flush.
- An untyped `vi.fn()` standing in for a synchronous, non-Promise callback prop (e.g. `onClick`) whose return value nothing asserts — the typed-mock rule targets Promise-returning APIs where a missing method silently resolves to `undefined`, not every stub in the file.
- `vi.mock` without `vi.hoisted` when the factory doesn't reference any top-level test variable — hoisting only bites when the factory closes over something declared later in the file.

## `vi.hoisted` for mock variables referenced in `vi.mock`

A `vi.mock` factory that references a top-level `const` will fail because `vi.mock` is hoisted above the `const` declaration. Use `vi.hoisted()` to lift the variable above the mock:

```ts
const { mockFn } = vi.hoisted(() => ({
  mockFn: vi.fn<() => Promise<string[]>>(),
}));

vi.mock('node:fs/promises', () => ({
  readdir: mockFn,
}));
```

This also solves the type mismatch between `vi.mocked()` after dynamic import and the original module's overloaded signatures. By referencing the mock variable directly in tests (`mockFn.mockResolvedValueOnce(...)`) instead of `const { readdir } = await import(...); vi.mocked(readdir).mockResolvedValueOnce(...)`, you avoid the type system seeing the original `fs/promises` overloads.

## Spread `importOriginal`, override by key

Never hand-list a module's exports. Always spread the original and override only what you need:

```ts
vi.mock('some-process-lib', async (importOriginal) => ({
  ...(await importOriginal<typeof import('some-process-lib')>()),
  spawn: vi.fn(() => mockHandle),
}));
```

This preserves all other exports at their correct types. A hand-written object literal silently drops new exports and serves stale types.

## Derive mock contracts from real interfaces

Don't hand-write mock interfaces. Derive from the real type:

```ts
type ProcessHandle = Pick<ChildProcess, 'pid' | 'on' | 'kill' | 'stdout' | 'stderr'>;

type WatchHandle = Pick<FSWatcher, 'close' | 'on'>;
```

When the mock needs extra test-only methods not on the real interface, extend via intersection:

```ts
interface MockProcess extends ProcessHandle, { _dataHandler: Handler | null; _exitHandler: Handler | null } {
  write: ReturnType<typeof vi.fn<(data: string | Buffer) => void>>;
}
```

## Type `vi.fn` with exact signatures

A bare `ReturnType<typeof vi.fn>` includes a constructor signature that won't satisfy a specific method type. Type the generic:

```ts
resize: ReturnType<typeof vi.fn<(columns: number, rows: number) => void>>;
```

## `ReturnType<typeof factory>` for mock result types

Let TypeScript infer the mock's shape from its factory function, then derive:

```ts
function makeWatcher() { /* ... */ }
type WatchResult = ReturnType<typeof makeWatcher>;
```

This guarantees the type always matches the implementation — no hand-maintained parallel type to drift.

## Don't `as unknown as` to bridge incompatible host types

Host types from different packages (e.g. Node's `EventEmitter` and a framework's `EventEmitter`) can have incompatible `this` types on `on()`/`emit()`. Don't double-cast through `unknown`. Define a structural contract that both satisfy:

```ts
interface Subscribable {
  on(event: string, listener: (...args: unknown[]) => void): void;
}
```

Then implement it directly in the mock without extending either EventEmitter class.

## Footguns

- `vi.fn()` with no type args → `.mockImplementation` returns `unknown`, blows up at runtime.
- `vi.useFakeTimers()` inside an arrow passed to `it(...)` without a restore → leaks forever.
- `waitFor(() => expect(...))` with fake timers and no `advanceTimers` on user-event → silent hang.
- `vi.spyOn` without `vi.restoreAllMocks()` in `afterEach` → spy state leaks between tests.
- `vi.mock(...)` calls are hoisted; module imports run BEFORE them — reference top-level test variables via `vi.hoisted(() => ...)`.
- Await timer advance calls — `vi.advanceTimersByTimeAsync` is async. `await` it.
