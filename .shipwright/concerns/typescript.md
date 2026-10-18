---
description: "TypeScript typing discipline: strict-mode patterns, escape hatches as defects, derived and branded types, narrowing, anti-drift rules for correctness-first codebases"
triggers:
  file_globs:
    - "**/*.ts"
    - "**/*.tsx"
    - "**/*.d.ts"
    - "**/*.mts"
    - "**/*.cts"
---

# TypeScript

Discover the project's `tsconfig` flags first; patterns marked `[noUncheckedIndexedAccess]` or `[exactOptionalPropertyTypes]` below apply only when those flags are enabled. Check `tsconfig.json` (and any extended configs) before assuming which strictness rules are active.

Code that compiles is still wrong if it leans on `any`, escape hatches, or hand-maintained parallel types. The rules below are the merge bar for correctness-first TypeScript codebases.

## The type system is a check that runs outside the author

The compiler's value is that it is not the person writing the code. It fires at authoring time, on every line, deterministically, without anyone having to remember to look. Nothing else in the toolchain covers that much surface that cheaply.

Every escape hatch trades that mechanism back for trust. `as any` does not fix an error — it removes the check that was reporting one. Widening a parameter until a call site compiles does not make the call correct; it stops the compiler saying it isn't. So when a type error appears, the first question is *what does the compiler know that the code does not* — never *how do I make the message go away*. Reaching for a hatch is a decision to ship unverified, and it should read like one in the diff.

## Escape hatches are defects

- **Never `any`** — not in annotations, not inside generic arguments (`Promise<any>`, `Record<string, any>`, `T = any`). `any` is contagious: every value derived from one is unchecked too, with no error anywhere to mark where checking stopped.
- **`unknown` is the honest `any`.** It accepts any value and permits no operation until you narrow. Use it everywhere `any` was tempting; the narrowing you're forced to write is the check you were about to skip.
- **`as unknown as X` is a double cast whose only purpose is to defeat the compiler's objection to the single cast.** TypeScript rejects `a as B` when the two types don't sufficiently overlap — that rejection is a finding. Routing through `unknown` makes a *stronger* claim in order to silence the complaint about a weaker one. If `as X` alone doesn't compile, that is information about the code.
- **`!` (non-null assertion)** is the same act on a narrower question. Narrow with a check, or handle both branches.
- **`object`, `{}`, `Function`** are nearly always the wrong reach. `object` permits nothing useful, `{}` means "anything except null/undefined", and `Function` erases the signature so any call typechecks. Write the shape, or `Record<string, unknown>`, or a real function type.
- No `@ts-ignore` / `@ts-expect-error`. If TS complains, the code is wrong, not TS.
- No `eslint-disable` (or the equivalent directive comment for the project's linter) to make a gate green.

### When a cast is legitimate

Casts exist because the type system genuinely cannot know some things you can, and those cases are real:

- **Parsing untrusted input.** `JSON.parse` returns `any`. Type the result `unknown` and validate it with a schema or type guard — then the narrowed type comes from a check that ran, not from an assertion about unvalidated bytes.
- **A third-party type that is simply wrong** — a `.d.ts` too narrow, too loose, or stale against the runtime. Cast at the one call site, name the package in a comment, report it upstream.
- **Establishing a boundary contract** — a `contextBridge` surface, a `document.querySelector` result. `satisfies` often does this better; prefer it where it fits.

An acceptable cast is **narrow** (one value, not a module), **documented** (why the compiler can't know), and **validated at the point it's made** (a check ran, or the source is genuinely opaque). A cast with none of the three is a suppression that doesn't look like one.

## A warning is a finding the tool couldn't prove

`tsc` has no warning level — every type error is an error. So the warning surface in a TypeScript project is the linter, and that is exactly where the temptation to scroll past lives.

**The rules worth having are the type-aware ones, and they are opt-in.** A syntax-only pass cannot see that a promise was never awaited, that a condition is always true, or that a value crossing an `any` boundary is unchecked — those need the type information only a real compiler has. `no-floating-promises`, `await-thenable`, `no-misused-promises`, `no-unnecessary-condition`, `no-unsafe-*` and `no-deprecated` are the highest-yield rules in the ecosystem and every one of them is in that set, so a project linting without type awareness is missing the findings it most wanted. Turning it on costs a flag and a slower pass: oxlint via `--type-aware` (or `"options": { "typeAware": true }`) with the `oxlint-tsgolint` package installed, or ESLint via typescript-eslint's type-checked configs, which need to be pointed at a real TypeScript program. Check which mode the gate actually runs before concluding the code is clean.

A lint warning means the tool found a shape that is *usually* wrong and cannot prove it is wrong *here*. That inability is the whole reason it warns instead of erroring — it is handing you a judgement it isn't entitled to make. Silencing it answers that judgement with "I didn't look", which is the same trade as `as any` one level up: a mechanism converted back into trust.

- **Run the gate with warnings as failures** — whatever the project's linter calls it (`--deny-warnings` on oxlint, `--max-warnings 0` on ESLint, or the equivalent flag otherwise). A warning nobody has to act on is a warning everyone learns to scroll past, and the useful ones drown in the backlog within a release.
- **A suppression is sited exactly where the constraint is** — one rule, one line, with a comment naming the *constraint*, not the rule code. `// oxlint-disable-next-line typescript/no-floating-promises -- fire-and-forget telemetry; failures are logged in the sink` (or the `eslint-disable-next-line` equivalent) is a decision. The same directive with no trailing text is a shrug.
- **The same suppression repeated many times is not many exceptions.** It is one missing abstraction — wrap the pattern once and suppress there — or a rule that does not fit this project, which should be turned off deliberately in configuration with a reason where a reader can see it. Deciding once, visibly, is the good outcome. Deciding never, invisibly, at fifteen call sites, is the bad one.

### Suppression in configuration counts as suppression

An in-line `// eslint-disable-next-line` or `@ts-expect-error` is at least visible: it
sits on the offending line and shows up in the diff that introduced it. A suppression
written into tool configuration does the same job invisibly — it silences the rule for
every current and future line in scope, sits nowhere near the code it excuses, and
outlives the reason for it. **Config-file suppression is the more durable way to hide a
warning and is held to a stricter standard than the in-line form, not a looser one.**

All of these are suppressions:

```jsonc
// linter-config.json / eslint.config.js — rule turned off, or downgraded to a non-blocking level
{ "rules": { "typescript/no-floating-promises": "off" } }
{ "rules": { "typescript/no-unsafe-assignment": "warn" } }  // when warnings don't fail CI

// type-aware checking switched off wholesale, silently retiring the rules that need it
{ "options": { "typeAware": false } }

// per-glob override — a whole directory exempted
{ "overrides": [{ "files": ["src/legacy/**"], "rules": { "typescript/no-explicit-any": "off" } }] }

// ignore list — files removed from linting entirely
{ "ignorePatterns": ["src/generated/**", "src/workers/**"] }

// tsconfig.json — strictness weakened, or files removed from the program
{ "compilerOptions": { "strict": false, "skipLibCheck": true, "noUncheckedIndexedAccess": false } }
{ "exclude": ["src/experimental"] }

// coverage thresholds lowered, or files omitted, so an untested module reports clean
{ "coverage": { "exclude": ["src/spawn/**"], "thresholds": { "lines": 0 } } }
```

Rules:

- **Never turn a rule off, downgrade its severity, or widen an ignore glob to make a
  gate green.** Fix the finding. If it genuinely cannot be fixed now, suppress the
  narrowest possible scope — one rule, one line, in-line — with a comment naming the
  reason.
- **Downgrading `error` to `warn` is a suppression** wherever warnings do not fail the
  build. It reads as a compromise and behaves as an off switch.
- **Never add code you just wrote to `ignores` / `exclude` / coverage omit lists.** That
  removes it from every gate the project has.
- **Weakening a `tsconfig` flag is a project-wide change**, never a fix for one file's
  errors. `strict`, `noUncheckedIndexedAccess`, `exactOptionalPropertyTypes`,
  `noImplicitAny` are the project's contract; a ticket that needs one relaxed needs a
  conversation, not an edit.
- **Review the config diff.** Changes to linter config, `tsconfig`, and coverage
  thresholds are changes to the project's quality bar and are reviewed as such — not
  waved through as configuration.
- **Prefer a dated, attributed entry over an anonymous one.** A suppression with no
  recorded reason can never be safely removed, so it never is.

## `satisfies` over annotations over `as`

```ts
// Widens: status loses the 'ok' literal
const r: CheckResult = { name: 'check', status: 'ok', message: '' }

// Validates shape, preserves 'ok' literal inference
const r = { name: 'check', status: 'ok', message: '' } satisfies CheckResult

// Only when TS genuinely can't know (JSON boundary, etc.)
const parsed = JSON.parse(raw) as UnknownShape
```

Use `satisfies` for config objects, lookup tables, preload bridges, and mock factories — anywhere you want both type-conformance and precise inference. Use annotations on function parameters and public APIs. Avoid `as`.

## Literal unions over `string`

```ts
type Status = 'ok' | 'warn' | 'fail'   // closed set — enforce
type RawInput = string                 // open — user-entered
```

If the value set is known at compile time, never use `string`. Misspellings become compile errors, autocomplete works, and exhaustive switches become possible.

## Derive, don't duplicate

```ts
export const IPC_CHANNELS = { ping: 'ping', ... } as const
export type IpcChannelKey = keyof typeof IPC_CHANNELS
export type IpcChannelValue = (typeof IPC_CHANNELS)[IpcChannelKey]
```

Never maintain a parallel hand-written union next to `as const` values — derive with `keyof` / indexed access. Same for:

- `Pick<T, K>` / `Omit<T, K>` — don't copy-paste fields.
- `ReturnType<typeof fn>` / `Parameters<typeof fn>` — pick up changes automatically.
- `Extract<U, V>` / `Exclude<U, V>` — narrow union types.

Every hand-maintained subset interface is a drift bug waiting to happen.

## Exhaustive unions with `satisfies never`

```ts
type Shape = 'circle' | 'square' | 'triangle'

function describe(kind: Shape): string {
  switch (kind) {
    case 'circle':   return 'Round'
    case 'square':   return 'Four-sided'
    case 'triangle': return 'Three-sided'
    default: kind satisfies never; throw new Error(`unhandled: ${String(kind)}`)
  }
}
```

A switch default that just throws does NOT catch the "added new variant" bug. `satisfies never` (or an `assertNever(x: never): never` helper) fails typecheck everywhere the variant isn't handled. Do this on every discriminated union.

## Branded types for IDs and paths

```ts
declare const _brand: unique symbol
type UserId = string & { readonly [_brand]: 'UserId' }
type AbsolutePath = string & { readonly [_brand]: 'AbsolutePath' }
```

When a function takes multiple `string` arguments that mean different things (ID vs path, user-input vs validated), brand them. The compiler then rejects `deleteUser(user.path)` at the call site when it expects a `UserId`. Apply at trust boundaries; skip for ephemeral locals.

## `readonly` everywhere it's load-bearing

- IPC return shapes — data crossing a process boundary must not be mutated.
- External store state — accidental mutation breaks selectors.
- React props — enforce at the interface level.
- Arrays — `readonly T[]` prevents `push`/`splice`/`sort` (which mutates!).

Paired with `noUncheckedIndexedAccess`, `readonly T[]` indexing returns `T | undefined` AND disallows mutation — two classes of bug prevented at once.

## Generics with constraints

```ts
function invokeIpc<K extends IpcChannelKey>(
  key: K,
  ...args: IpcInvokeArgs[K]
): Promise<IpcInvokeResult[K]>
```

Constrain generics to the narrowest type the function actually needs. `K extends keyof T` is the single most useful pattern — it ties the key parameter to the map it indexes.

A cast inside a function is often a generic the signature failed to express: the author knows the return type depends on an argument, couldn't say so, and asserted it instead. Reach for a constrained type parameter (or a conditional/mapped return type) before an overload set, and for an overload set before a cast.

## Explicit return types on exports

Always annotate exported function return types, even when inference is obvious. It catches silent contract changes during refactors and makes call-site reasoning local.

## Turn the compiler up — `strict` is the floor, not the ceiling

`strict: true` enables exactly nine checks: `noImplicitAny`, `noImplicitThis`, `strictNullChecks`, `strictFunctionTypes`, `strictBindCallApply`, `strictPropertyInitialization`, `strictBuiltinIteratorReturn`, `useUnknownInCatchVariables`, `alwaysStrict` ([tsconfig reference](https://www.typescriptlang.org/tsconfig/)). Several of the highest-value checks are *not* in that set and have to be asked for. Leaving them off is a choice with a named cost:

- **`noUncheckedIndexedAccess`** — without it, `arr[0]` and `record[key]` are typed `T` even when the index is out of range or the key absent. This is the single largest hole in a "strict" codebase: every lookup lies, and the resulting `undefined` surfaces at runtime as a property access on nothing.
- **`exactOptionalPropertyTypes`** — without it, `{ x?: T }` accepts `{ x: undefined }`, so "absent" and "present but undefined" are indistinguishable. Anything that round-trips through JSON, a spread, or `Object.keys` can tell them apart at runtime even though the types cannot.
- **`noImplicitOverride`** — without it, renaming a base-class method silently orphans the subclass override into a new, never-called method.
- **`noFallthroughCasesInSwitch`** — a missing `break` compiles clean.
- **`noImplicitReturns`** — a branch that forgets to return yields `undefined` under a declared return type.
- **`noPropertyAccessFromIndexSignature`** — forces `obj['key']` for index-signature access, so a typo in a dotted property name is an error rather than `undefined`.

Enable them on new projects. On an existing one, enabling a flag is a project-wide change with a migration behind it — propose it, don't slip it into a feature diff. And note the converse: **weakening any of these to clear errors in one file is never the fix** (see the suppression rules above).

`noUncheckedIndexedAccess` in particular changes how you write: if you find yourself reaching for `!` to get past it, that is the flag doing its job. Narrow with a check, or handle the missing case.

## What NOT to flag

- `as` casts at genuine boundaries (`JSON.parse`, `contextBridge`, a DOM query result, an untyped third-party return) — that's what the escape hatch exists for, not a violation to report.
- **`any[]` in a generic constraint over arbitrary functions** — `T extends (...args: any[]) => unknown` is the correct idiom and `unknown[]` does not substitute for it (parameter contravariance makes concrete functions unassignable). TypeScript's own `Parameters<T>` is defined this way. This is `any` in a *constraint*, where nothing flows through it, not `any` in a value position.
- **A double cast in a test constructing a deliberately invalid value** to exercise an error path — the point of the test is that the type is wrong.
- **A rule turned off in linter config with a stated reason**, where the rule genuinely doesn't fit the project's architecture. That is the decision being made in the open, which is the outcome the warning rules are asking for — not a suppression to report. What to flag is the same rule silenced at fifteen call sites without anyone ever deciding.
- **A codebase not running `noUncheckedIndexedAccess` / `exactOptionalPropertyTypes`** — these are worth proposing, but enabling one is a migration, not a ticket-scoped edit. Note the gap; don't fail the change over it.
- `string` for values whose set is genuinely open (user-entered text, an ID sourced from an external system) — the literal-union rule applies to closed, compile-time-known sets, not every string in the codebase.
- Missing explicit return types on unexported local helpers and closures — the "annotate exports" rule protects callers of the public surface; internal functions inferring their return type is normal and expected.
- Branding every string-typed local — apply at trust boundaries (function args that could be swapped, values crossing a serialization boundary), not to every ephemeral variable.
- A `readonly` omission on a local, non-exported, non-shared type — the rule targets IPC payloads, store state, and props: things another part of the codebase can hold a reference to and mutate out from under the owner.

## Derive structural contracts from concrete types

When a function only needs a subset of a large interface, derive the contract with `Pick<>` or `Omit<>` rather than hand-writing a parallel interface or accepting the full type:

```ts
// Instead of: function register(service: UserService)
function register(service: Pick<UserService, 'createUser' | 'sendEmail'>)
```

The narrowed parameter type means tests can provide a mock that satisfies only what the function actually calls. When host types from different packages (e.g. Node `EventEmitter` vs a framework's `EventEmitter`) have incompatible `this`-typed return types, `Pick<>` can capture the wrong `this`. Use broader structural types at process boundaries:

```ts
interface Listenable {
  on(event: string, listener: (...args: unknown[]) => void): void;
}
```

Prefer `unknown[]` over `any[]` for event-emitter-style callbacks — the linter treats them identically to value-position `any`. Narrow event arguments inside the callback with `as` casts at the call site.

## Use `typeof`, `ReturnType<>`, `Pick<>`, `Omit<>` — never re-list

The type system can derive everything you need from what already exists. Hand-maintained parallel types drift.

```ts
// Derive from the mock factory — always matches
type MockResult = ReturnType<typeof makeMock>;

// Narrow a host type to exactly what's used
type TerminalHandle = Pick<Terminal, 'pid' | 'write' | 'resize' | 'kill'>;
```

When a test mock needs internal fields not on the production type, use an intersection:

```ts
type InternalFields = { _buffer: string[]; _closed: boolean };
interface MockTerminal extends Pick<Terminal, 'write' | 'resize'>, InternalFields {
  write: ReturnType<typeof vi.fn<(data: string | Buffer) => void>>;
}
```

Narrow the mock methods to the exact signatures they must satisfy: `ReturnType<typeof vi.fn<(columns: number, rows: number) => void>>` instead of bare `ReturnType<typeof vi.fn>` which includes a constructor signature that won't match.

## `satisfies` over `as` for test data

```ts
// Validates shape at compile time, no suppression
const user = { id: '1', name: 'Ada' } satisfies User;
```

If a property doesn't exist on the target type, `satisfies` catches it — `as unknown as` would silently widen past it.

## Model optional/defaulted fields accurately

When a zod schema has `.default()`, make the TS field optional. Don't make tests clone and `delete` keys to simulate absent fields — use destructuring:

```ts
const { optionalField: _unused, ...rest } = baseConfig;
```

## Branded type constructors in tests

Never `'id' as unknown as UserId`. Use the branded constructor:

```ts
import { asUserId } from './branded';
const id = asUserId('test-user-id');
```

## Footguns

- **`Array.prototype.sort` mutates** — copy first: `[...arr].sort(...)` or use `arr.toSorted()` (ES2023; Node 20+, [MDN](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/Array/toSorted)).
- **`Date` is mutable** — `new Date()` is a fresh instance; `d.setHours(...)` mutates.
- **Object spread is shallow** — nested objects are still aliased.
- **`JSON.parse` returns `any`** — type as `unknown`, then validate with a schema (Zod, custom guard).
- **`as const` on a function's return value** widens to the declared type if the function's return type is broader — annotate the function's return type OR use `satisfies` inside.
