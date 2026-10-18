---
description: "Component tests with Testing Library and user-event: query priority, async idioms, scoping, fake-timer integration — applies to any Testing Library adapter (@testing-library/react, @testing-library/vue, @testing-library/svelte, etc.)"
triggers:
  file_globs:
    - "**/*.test.tsx"
    - "**/*.spec.tsx"
    - "**/*.test.jsx"
    - "**/*.spec.jsx"
    - "**/*.test.vue"
    - "**/*.spec.vue"
    - "**/*.svelte.test.*"
    - "**/*.svelte.spec.*"
---

# Testing Library

Testing Library is a family of adapters — `@testing-library/react`, `@testing-library/vue`, `@testing-library/svelte`, `@testing-library/preact`, and others — that share the same query API and philosophy: "test the way a user uses your app." The rules below use React syntax for illustration; the query priority, async idioms, and fake-timer patterns are identical across adapters. Translate the render call and any framework-specific setup to the project's adapter.

Tests should look like a user. If your test imports implementation internals or reaches into a `ref` to get a value, it is not testing behavior — it is testing structure, and it will break on refactor.

Discover the project's test setup file (wherever `vitest.config.ts` points `setupFiles` at) before adding new global test wiring — jest-dom matchers, `window` stubs, and mock globals belong there, not duplicated per test file.

## jest-dom matchers: import the `/vitest` subpath

```ts
// In the file setupFiles points at
import '@testing-library/jest-dom/vitest'
```

`@testing-library/jest-dom` (v7+) publishes a dedicated `/vitest` entry point that registers matchers via Vitest's `expect.extend`, distinct from the bare `@testing-library/jest-dom` import written for Jest. Importing the Jest-targeted entry point under Vitest is a common source of "`toBeInTheDocument` is not a function" — use the `/vitest` subpath.

## Setup `userEvent` once per test

```tsx
import userEvent from '@testing-library/user-event'

it('does the thing', async () => {
  const user = userEvent.setup()
  render(<Comp />)
  await user.click(screen.getByRole('button', { name: /submit/i }))
})
```

Always `userEvent.setup()` per test. The direct calls (`userEvent.click(el)`) are not deprecated — they call `setup()` internally — but they discard the session between calls, so pointer and keyboard state does not carry across steps and a modifier held in one call is forgotten by the next. Use the instance.

`fireEvent` is not deprecated either, and it is the right tool for events a user cannot produce directly and `user-event` therefore does not model — `scroll`, `resize`, media events, `animationend` / `transitionend`, and the like. What it must not be used for is *simulating a user*: it dispatches one bare synthetic event and skips the pointer/keyboard state machine, so a `fireEvent.click` on an element the user could never reach (disabled, covered, `pointer-events: none`) passes happily while the real interaction fails.

## Query priority

1. `getByRole` / `findByRole` / `queryByRole` — mirrors how assistive tech finds elements. Always first.
2. `getByLabelText` — form inputs with associated labels.
3. `getByPlaceholderText` — form inputs without labels (rare; prefer labels).
4. `getByText` — non-interactive content.
5. `getByDisplayValue` — form inputs whose current value is the distinguishing feature.
6. `getByAltText` — images.
7. `getByTitle` — elements with a `title` attribute (last resort before testid).
8. `getByTestId` — escape hatch. Every `data-testid` is a structural coupling. Justify each one.

If you can't find an element by role, the element's accessibility is broken — fix the component, don't downgrade the query.

## `findBy` > `waitFor(getBy)`

```ts
// Preferred
const btn = await screen.findByRole('button', { name: /ok/i })

// Only when you need to assert multiple conditions together
await waitFor(() => {
  expect(screen.getByRole('button')).toBeEnabled()
  expect(screen.getByText(/ready/i)).toBeInTheDocument()
})
```

`findBy*` is `waitFor(getBy*)` with better defaults and clearer error messages. Reach for `waitFor` only when you need a composite assertion.

## `within` for scoping

```ts
const dialog = screen.getByRole('dialog')
const okBtn = within(dialog).getByRole('button', { name: /ok/i })
```

Avoid `getAllByRole('button')[0]` indexing. Scope with `within` to express intent and survive structural changes.

## Async queries in user-event chains

```ts
await user.click(screen.getByRole('button', { name: /open/i }))
expect(await screen.findByRole('dialog')).toBeInTheDocument()
```

After a user action that triggers async work, use `findBy*` to wait for the resulting state — never a bare `setTimeout` / `await new Promise(res => setTimeout(res, 100))`.

## Fake timers + user-event must be wired together

```ts
beforeEach(() => { vi.useFakeTimers() })
afterEach(() => { vi.useRealTimers() })

it('does a delayed thing', async () => {
  const user = userEvent.setup({ advanceTimers: (ms) => vi.advanceTimersByTime(ms) })
  render(<Comp />)
  await user.click(screen.getByRole('button'))
  await vi.advanceTimersByTimeAsync(1000)
  expect(await screen.findByText(/done/i)).toBeInTheDocument()
})
```

Without the `advanceTimers` option, any delay inside user-event (typing, debounce) will hang against frozen fake timers.

## `act` — almost never needed

`render`, `findBy*`, `waitFor`, and `user.*` already wrap in `act`. The only remaining use case is a synchronous state mutation you dispatch directly from the test:

```ts
act(() => { useStore.getState().setValue(42) })
expect(screen.getByText(/42/)).toBeInTheDocument()
```

Do NOT wrap `await user.click(...)` in `act`. It produces a console warning and does nothing.

## Assertions

Use `@testing-library/jest-dom` matchers for richer errors:

```ts
expect(el).toBeInTheDocument()
expect(el).toBeVisible()
expect(el).toBeDisabled()
expect(el).toHaveAccessibleName('Submit')
expect(el).toHaveTextContent(/loaded/i)
```

Do NOT assert on CSS classes (`toHaveClass`) to verify behavior — classes are structural. Test the outcome (is the button disabled? is the panel visible?), not the mechanism (does it have `.is-disabled`?).

## Dialogs

```tsx
<dialog open aria-labelledby="dialog-title">
  <h2 id="dialog-title">Confirm action</h2>
  ...
</dialog>
```

```ts
// Query by accessible name — resilient to structural changes
const dialog = await screen.findByRole('dialog', { name: /confirm action/i })
```

Never use `aria-label` on a `<dialog>` that contains a heading — the attribute overrides the heading and the heading's accessible-name contribution is lost.

jsdom does not implement `HTMLDialogElement.showModal()` — render dialogs in tests with the bare `open` attribute (`<dialog open>`), not via `showModal()`.

## Do not assert on `document.querySelector`

```ts
// Bad — coupled to DOM structure, not accessible
const items = document.querySelectorAll('[data-severity="fail"]')

// Good — query by role, filter by semantic attribute
const items = screen.getAllByRole('listitem').filter(
  (el) => el.getAttribute('data-severity') === 'fail',
)

// Better — expose semantics in text so queries are natural
const items = screen.getAllByText(/failed:/i)
```

`container.querySelector` / `document.querySelector` bypass Testing Library entirely. Treat them like `any` — a code smell.

## Cleanup is automatic — if `globals` is on

`@testing-library/react` registers its own `afterEach(cleanup)` automatically, but only when it detects a global `afterEach` in scope. That requires Vitest's `test.globals: true`; with `globals: false` (Vitest's own default) there is no ambient `afterEach` to hook, and cleanup silently stops happening between tests. Check the project's `vitest.config.ts` for `globals: true` before assuming cleanup is automatic; if it's off, call `cleanup()` in an explicit `afterEach` instead of guessing.

## What NOT to flag

- `fireEvent` used for events `user-event` doesn't model (`scroll`, `resize`, `animationend`) — that's the documented, correct use of `fireEvent`, not a downgrade from `user-event`.
- A single `getByTestId` on a genuinely non-semantic element (e.g. a canvas-drawn chart with no accessible role) — the escape hatch exists for exactly this case. Flag it only when a role/label/text query would have worked.
- `within(container)` on a container obtained from `render()`'s own return value, not `screen` — both are valid entry points; `screen` is the convenience default, not a requirement.
- Calling `cleanup()` explicitly in a file when the project's `globals` config makes it unnecessary elsewhere but this file's tests run under a runner/config where auto-cleanup isn't wired — don't assume the project-wide default applies to every test file without checking.

## Footguns

- `getBy*` throws if not found; `queryBy*` returns null — know which you want.
- `findBy*` is async — always await.
- `expect(screen.getByText(...))` — `getByText` already throws if missing; the `expect` is redundant. Use `toBeInTheDocument()` for clarity.
- `fireEvent` standing in for a user interaction — skips the pointer/keyboard state machine, so the test passes where the real interaction would not.
- `userEvent.click` called directly instead of on a `setup()` instance — each call gets a fresh session, so held modifiers and pointer state are lost between steps.
- Long `waitFor` with an expensive predicate — runs the predicate every poll interval (default 50ms); keep predicates cheap.
- Testing implementation — `expect(setState).toHaveBeenCalled()` is a red flag. Test the rendered outcome instead.
