---
description: "React 19 component patterns — effects with abort/cleanup, state scope and derivation, server-state caching and query keys, error boundaries vs inline error state, accessibility, concurrent-safe subscriptions, and React Compiler adoption"
triggers:
  file_globs:
    - "**/*.tsx"
    - "**/*.jsx"
---

# React 19

Components should be small, typed, accessible, and testable. These rules cover the patterns that have the highest bug-prevention payoff.

## Effects with async external calls

```tsx
useEffect(() => {
  const controller = new AbortController()
  async function load(): Promise<void> {
    try {
      const result = await api.fetchItems({ signal: controller.signal })
      if (!controller.signal.aborted) setItems(result)
    } catch (err) {
      if (!controller.signal.aborted) logger.error('Failed to load items', { err })
    }
  }
  void load()
  return () => { controller.abort() }
}, [])
```

- Async IIFE inside the effect — `useEffect` itself cannot be async.
- `try / await / catch` — never `.then().catch().finally()`. Rejections are captured inline; the control flow reads top-to-bottom; a missing mock throws at the await point, not silently on `undefined.then`.
- `AbortController` — prevents "setState on unmounted component" warnings and stale data when the component unmounts while the promise is pending. Prefer it over a `let cancelled = false` flag: `controller.signal.aborted` is typed as `boolean`, which avoids an `@typescript-eslint/no-unnecessary-condition` false-positive on the literal-`false` initializer, and the same signal can be passed down to `fetch` / IPC calls that honour `AbortSignal`.
- `void` the IIFE — signals to ESLint / readers that the Promise is intentionally not awaited.
- Return a cleanup that calls `controller.abort()`.

## `use(promise)`: stable, but the promise's identity is the whole contract

`use()` is a stable React 19 API — it unwraps a Promise or reads a Context inside render, and it must be called under a `<Suspense>` boundary (or an error boundary, for rejections). It is not restricted to Server Components.

The footgun: `use()` keys on the Promise's *identity*, not its resolved value. Create a new Promise inside the component body on every render (`use(fetch(...))` written directly in render) and the component suspends every render — an infinite loop, not a one-time load. The Promise must be stable across renders: created outside the component, memoized (`useMemo`), owned by a cache/store, or passed down from a parent that only creates it once.

For an ad-hoc client-side fetch with no Suspense architecture in place, `useEffect + useState` (above) is still simpler and the safer default — `use()` earns its place once the component tree already suspends (route-level Suspense boundaries, a query library that hands `use()` a cached Promise).

## Error handling: inline state vs error boundary

- **Inline state (`useState<Error | null>`)** — for recoverable operational failures the UI should display: IPC errors, fetch failures, validation errors. Render an `<p role="alert">` with a message and a retry button.
- **Error boundary** — for unrecoverable render-time throws: programming errors, missing providers, corrupt props. A well-designed app has very few error boundaries — one at the root, maybe one per screen.

Do NOT use an error boundary to catch expected IPC rejection. That's inline state territory.

## State: scope it to its actual lifetime

- Component-local UI state → `useState` / `useReducer`.
- Cross-screen application state (current user, navigation, cached lists) → the project's existing store (Zustand, Redux, Jotai — whatever the project already runs), placed where the project already puts domain state. Use what is there; a feature ticket is not the place to introduce a second state library.
- Derived values → selectors, so a component re-renders only when the slice it reads changes. Select the narrowest slice you need: subscribing to the whole store re-renders the component on every unrelated field. A selector that builds an object or array of several fields returns a fresh reference every call and so defeats the comparison entirely — either take one field per selector, or wrap the multi-field selection in the store's shallow-equality helper.
- Never write to a store from inside a selector or during render.

### Derive, don't duplicate

If a value can be computed from other state, it is not state — storing it separately is two sources of truth that must be kept in sync by hand, and one update path will eventually be missed. Compute it inline or in a selector/`useMemo`, every render. This is not React-specific — the same rule applies to a component derived from props, a selector derived from store state, or a database column that duplicates another table's data — but it recurs constantly in component code as `const [filteredCount, setFilteredCount] = useState(...)` next to the `filtered` array it's the length of.

### Bookmarkable state belongs in the URL, not `useState`

Filters, tabs, pagination, sort order, a selected item — anything a user would reasonably expect to survive a reload or be shareable as a link — is derived state with the URL as its source of truth, not component state. Read it from the router's query params, write to it via navigation, and derive the rendered UI from what the URL currently says. `useState` for this class of value works until the first reload or shared link, then it's a bug report. Reserve component state for what is genuinely ephemeral (an open dropdown, an in-progress form draft, hover state) — not everything that happens to live in a component.

## Props: `readonly` on everything

```tsx
export interface ConfirmDialogProps {
  readonly title: string
  readonly onConfirm: () => Promise<void>
  readonly onCancel: () => void
}
```

Props that are objects or arrays must be `readonly`. React does not enforce this at runtime, but TS will catch accidental mutation.

**`ref` is an ordinary prop in React 19 — `forwardRef` is no longer needed.** Declare it in the props type (`readonly ref?: Ref<HTMLButtonElement>`) and read it like any other prop. New components should not wrap in `forwardRef`, and a component that already does is carrying a wrapper, a `displayName` assignment, and an extra type parameter for nothing. Existing `forwardRef` still works and is not itself a bug — it is worth unwrapping when you are already editing the component, not as its own sweep. Note the one case that still needs care: exposing an imperative handle to a parent remains a `useImperativeHandle` job, which the ref-as-prop change simplifies but does not remove.

## Explicit return type on components

```tsx
import type { ReactElement } from 'react'

export function ConfirmDialog(props: ConfirmDialogProps): ReactElement { ... }
```

Forces the author to ensure every code path returns valid JSX. Catches silent `undefined` returns that render nothing.

`@types/react` 19 removed the global `JSX` namespace — a bare `JSX.Element` annotation now fails with `Cannot find namespace 'JSX'`. Use `ReactElement`, or `React.JSX.Element` if you prefer the namespace form. A component that legitimately renders nothing on some paths returns `ReactNode` instead.

## Accessibility is correctness

- Semantic HTML first (`<main>`, `<button>`, `<dialog>`, `<nav>`) — role inferred, no `role="button"` on `<div>`.
- `type="button"` on every `<button>` that isn't a form submit — defaults are `submit`, which fires form submission.
- `<dialog>` with an inner heading uses `aria-labelledby` pointing to the heading's `id` — NOT a redundant `aria-label` (the two conflict; `aria-label` wins and the heading's accessible name is lost).
- Form inputs must have associated `<label>`s — either wrap the input or link via `htmlFor` + `id`.
- Never use `tabindex` to force tab order unless you're building a complex widget that demands it. Source order is almost always correct.

## Event handlers: `void` the async

```tsx
<button onClick={() => { void handleAsyncAction() }}>Do it</button>
```

An async handler returns a Promise. React ignores it; ESLint warns. `void` is the correct tell that "we intentionally don't await this." If you need to block UI during the action, track loading state instead.

## Effects should have narrow dependency arrays

- Each dependency is load-bearing — if you include something, every change re-runs the effect.
- Functions defined in the component body are a fresh reference each render — hoist them outside or wrap in `useCallback` with their own narrow deps.
- An effect whose only job is to set state from other state or props should not exist. Compute the value during render instead; where a value genuinely must reset when a prop changes, adjusting state during render (React's documented pattern) beats an effect, because an effect runs *after* paint and renders one frame of the stale value first.
- Suppressing the exhaustive-deps lint rule is almost always the wrong fix. The rule fires because the effect closes over a value it does not re-read; silencing it leaves the effect reading a stale closure. Fix the cause — stabilise the dependency, move it into a ref, or split the effect — and suppress only with a comment explaining which value is deliberately stale and why that is safe.

## Concurrent-safe patterns

React 19 can re-render at any time. Do not:

- Use `useRef` as a substitute for state that drives rendering.
- Mutate objects in a `useState` value — always produce a fresh object.
- Rely on `useState` updater function ordering when multiple effects fire in the same render.

## React Compiler: check adoption before reaching for manual memoization

React Compiler reached its stable 1.0 release; it optimizes components at build time and removes the need for manual `useMemo` / `useCallback` / `React.memo` in the cases it covers (cascading re-renders, expensive inline calculations). It requires strict Rules-of-Hooks compliance — a component with a conditional/early-return hook call or a lint-suppressed `react-hooks/rules-of-hooks` violation gets silently skipped, not optimized, so the compiler's win depends on the codebase already being lint-clean under `eslint-plugin-react-hooks`. Enable those rules in the same change that enables the compiler, never later: bailout is silent, so a project running the compiler without the lint rules has no signal telling it which components were skipped, and believes it is optimised where it is not.

Check whether the project has the compiler wired in (a Babel/Vite/Next plugin entry, `babel-plugin-react-compiler` or equivalent in the build config) before defaulting to manual memoization:

- **Compiler adopted** — do not add `useMemo`/`useCallback`/`React.memo` to fix a re-render problem the compiler already handles. Manual memoization still has a narrow legitimate role: stabilising a *reference* something outside render depends on (an effect's dependency array, a subscription callback) — the compiler optimizes render output, not effect deps.
- **Compiler not adopted** — manual memoization remains the correct tool: `useMemo` for expensive computed values, `useCallback` for callbacks passed to memoized children, `React.memo` on components that reliably re-render with unchanged props.

Either way, memoizing something that isn't measurably expensive is waste — the fix for a re-render problem is confirming it's real (profiler, not intuition) before reaching for either the compiler's config or manual memoization.

## Server state: don't hand-roll a cache

Data owned by a server (or IPC-exposed process) and merely mirrored in the UI is server state, not client state — it has staleness, concurrent-mutation, and error-retry concerns that a `useEffect` + `useState` fetch re-implements badly and incompletely. A `useEffect` fetch has no cross-component cache (two components fetching the same resource issue two requests), no de-duplication, no built-in retry/backoff, and no revalidation story beyond a manual re-fetch.

TanStack Query is the current mainstream, actively maintained library for this in the React ecosystem — reach for it (or the store's built-in query layer if the project already has one, e.g. RTK Query) once a screen has more than a trivial one-shot fetch: it owns caching, request de-duplication, background revalidation, retry/backoff, and loading/error state, so the component only describes what it needs and what to do with it. Keep the plain `useEffect` pattern above for genuinely one-off loads with no cache/mutation lifecycle (an IPC call made once at startup) — introducing a query library there is the inverse mistake.

Two details decide whether a query layer actually pays off:

- **Cache keys belong in one module, built by shared factories, not spelled out at each call site.** The key is what ties a fetch to the invalidation that must follow a mutation, and a key assembled by hand in two places is a stale-screen bug the moment one of them changes. Centralising them makes the relationship checkable rather than remembered.
- **Thread the cancellation signal the query library hands you into the request.** A query function that ignores it cannot be cancelled, so a superseded or unmounted query runs to completion and competes with the one that replaced it.

For mutations, invalidating the affected keys on success is the default; reach for an optimistic update only where the section below applies.

### Query key factories

The cache key is the whole identity of a query — it determines caching, de-duplication, and which invalidations affect it. A key factory centralises key construction so the relationship between a query and its invalidation is checkable, not remembered:

```ts
const userKeys = {
  all: ['users'] as const,
  lists: () => [...userKeys.all, 'list'] as const,
  list: (filters: UserFilters) => [...userKeys.lists(), filters] as const,
  details: () => [...userKeys.all, 'detail'] as const,
  detail: (id: string) => [...userKeys.details(), id] as const,
}

// query: useQuery({ queryKey: userKeys.detail(userId), queryFn: ... })
// invalidate after mutation: queryClient.invalidateQueries({ queryKey: userKeys.lists() })
```

- `invalidateQueries({ queryKey: userKeys.all })` invalidates every user query — the hierarchy makes partial invalidation natural.
- A key built ad hoc at each call site (`['user', id]` in one file, `['users', 'detail', id]` in another) makes the invalidation target ambiguous and drift-prone.

### Stale time and garbage collection time

- **`staleTime`** — how long data is considered fresh before a refetch is triggered on mount or window focus. The default of `0` (immediately stale) means every mount refetches; set it to a value that reflects how quickly the data actually changes (seconds for a live dashboard, minutes for a reference table, `Infinity` for data that never changes within a session).
- **`gcTime`** (formerly `cacheTime`) — how long unused query data stays in memory before garbage collection. The default of 5 minutes is reasonable; lower it only if the data is large and the query set is diverse.
- A `staleTime: Infinity` without a corresponding invalidation strategy is a stale-screen bug: the data never refetches because it is never stale, and nothing invalidates it. Use `Infinity` only for truly immutable data, or pair it with explicit `invalidateQueries` calls on mutations.

### Optimistic updates

For a mutation whose result the UI can predict (toggling a flag, adding a list item), don't wait for the round-trip before updating the screen — React 19's `useOptimistic` hook exists for exactly this: it renders a provisional value immediately, then reconciles with the real state once the action settles. Correctness has two parts, and both are load-bearing:

- **A rollback path.** If the underlying action rejects, the optimistic value must not persist as if it succeeded — surface the failure and let the real state (which `useOptimistic` reverts to) take over.
- **Reconciliation with the authoritative response**, not just the request that succeeded. If the server can return a value different from what was guessed (a generated ID, a computed field), the UI must settle on the response, not the guess.

Skip it for mutations where the outcome can't be predicted client-side (anything server-validated in a way the client can't replicate) — a wrong optimistic guess that then flips is worse UX than a brief loading state.

## Subscribing to external event sources: latest-ref `useEffect`, not `useSyncExternalStore`

Two superficially-similar patterns, two different jobs:

- **`useSyncExternalStore`** — for **reading and rendering** a snapshot value that lives outside React (Zustand/Redux/Jotai stores, browser APIs you observe like `window.matchMedia`). Returns a value the component renders. Tearing-safe under concurrent rendering because React integrates it with the scheduler.
- **Latest-ref `useEffect`** — for **fire-and-forget event subscriptions** where the callback runs on an event but the result is not rendered (re-fetch data, invalidate a cache, push to another store). No snapshot value. No render coupling.

Forcing a fire-and-forget subscription through `useSyncExternalStore` means inventing a synthetic snapshot (e.g. an event counter), wiring a `getSnapshot` the consumer ignores, and triggering a re-render on every event the consumer doesn't care about. Use the right tool.

```tsx
export function useEventSource<T>(
  subscribe: (cb: (event: T) => void) => () => void,
  callback: (event: T) => void,
  filter?: (event: T) => boolean,
): void {
  const callbackRef = useRef(callback)

  useLayoutEffect(() => {
    callbackRef.current = callback
  })

  useEffect(() => {
    const unsubscribe = subscribe((event) => {
      if (filter === undefined || filter(event)) {
        callbackRef.current(event)
      }
    })
    return unsubscribe
  }, [subscribe, filter])
}
```

Why this shape:

- **`useLayoutEffect` for the ref update** — runs synchronously after DOM mutations and before paint. Plain `useEffect` runs after paint, leaving a window where an event can fire and invoke the previous render's callback. Mutating the ref directly in the render body instead (`callbackRef.current = callback` with no effect) is what React Compiler's `immutability` lint rule (`eslint-plugin-react-hooks`, which now ships the compiler's diagnostics) flags — `useLayoutEffect` is the compiler-safe version of the canonical "latest ref" pattern used by libraries like `use-callback-ref`.
- **`callback` NOT in the effect deps** — the subscription stays stable. The callback identity changing on every render is the whole reason the ref pattern exists; depending on it would cause subscribe/unsubscribe churn.
- **`subscribe` and `filter` ARE in the effect deps** — these are structural; when they change, the subscription must re-establish. Stabilize them at the call site (module-level function, `useCallback`) so the effect doesn't churn on every render.
- **Return the unsubscribe closure directly from the subscribe API** — the effect cleanup contract handles the rest. No extra state, no listener-handle indirection.

## What NOT to flag

- Manual `useMemo`/`useCallback`/`React.memo` in a codebase that has *not* adopted React Compiler — that's still the correct tool, not a smell.
- A `useEffect` fetch for a genuine one-shot load with no cache, revalidation, or shared-consumer need — pulling in a query library there is over-engineering, not a fix.
- Ephemeral UI state (open/closed, hover, an in-progress form draft, a multi-step wizard's current step if it's not meant to be linkable) staying in `useState` instead of the URL — only *bookmarkable* state belongs there.
- A component missing an error boundary when it has no render-time throw risk — boundaries are for genuinely unrecoverable failures, not insurance.
- `use()` absent from a codebase with no Suspense architecture — it's not a required upgrade path, `useEffect + useState` remains correct there.
- A `useOptimistic` call missing for a mutation whose outcome genuinely can't be predicted client-side — a loading state is the right call, not every mutation should feel instant.

## Footguns

- **Async in event handlers without `void`** — ESLint warning, silent rejection.
- **Missing cleanup in `useEffect`** — stale listeners, memory leaks, "state update on unmounted component" warnings.
- **`useEffect` dependency arrays with functions defined in the body** — infinite re-renders.
- **`<form>` without `onSubmit={(e) => e.preventDefault()}`** — page reloads on submit.
- **State update during render** — throws "Cannot update during an existing state transition."
- **`key` prop missing from list items** — silent performance hit and reconciliation bugs.
- **`dangerouslySetInnerHTML` with user content** — XSS. Never.
- **`aria-label` on a `<dialog>` that also contains a heading** — duplicates the accessible name.
