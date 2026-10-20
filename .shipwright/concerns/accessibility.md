---
description: "Accessibility (WCAG): semantic structure, keyboard navigation, screen-reader compatibility, color contrast, focus management"
triggers:
  file_globs:
    - "**/*.tsx"
    - "**/*.jsx"
    - "**/*.vue"
    - "**/*.svelte"
    - "**/*.html"
    - "**/*.css"
    - "**/*.scss"
---

# Accessibility

Accessibility is not a separate feature — it is a quality attribute of every user-facing change. Code that renders UI must be usable by people using keyboards, screen readers, voice control, switch access, and magnification. This concern applies to any change that touches rendered output the user can see or interact with. It does NOT apply to internal APIs, build tooling, test infrastructure, or backend logic with no UI surface.

## Discover the project's accessibility tooling

Before reviewing or implementing, check the project's configuration for accessibility tooling: lint rules that flag accessibility violations in templates or components, an automated audit engine wired into the test suite, component libraries with accessibility built into their primitives, and assertion helpers for accessible names and roles. Read how existing code uses these tools. Ensure new code follows the same patterns and passes the same checks. If the project has no accessibility tooling configured, flag that — but do not assume a specific tool or set one up yourself. Project infrastructure setup is a separate initiative, not per-ticket work.

## Semantic structure over presentational markup

Use the platform's semantic elements first. A `<button>` is a button. A `<nav>` is navigation. A `<dialog>` is a dialog. Semantic elements carry role, keyboard behavior, and accessible-name derivation for free. Rebuilding them from generic elements (`<div onclick>`) requires manually adding `role`, `tabindex`, keyboard handlers, focus management, and accessible names — every one of which is a maintenance burden and a common omission.

If you can use a semantic element, use it. If the framework provides an accessible component primitive, use that instead of building from scratch.

## Keyboard navigation

Every interactive element must be reachable via keyboard (Tab / Shift+Tab) and operable via keyboard (Enter / Space for buttons, arrow keys for listboxes, Escape for dialogs). If you add an interactive element that isn't a native control, you must implement keyboard interaction matching the relevant ARIA authoring practice pattern.

Focus must be visible. Never suppress focus indicators globally. Custom focus styles are fine; `outline: none` without a replacement is not. Visible is not the same as unobscured — a focused element scrolled behind a sticky header or footer fails WCAG 2.4.11 (AA) even though its indicator is drawn.

Anything operable by dragging needs a single-pointer alternative that is not a drag — a click target, a menu action, keyboard reordering (WCAG 2.5.7, AA). Pointer targets need at least 24×24 CSS pixels, or adequate spacing between them (WCAG 2.5.8, AA); the criterion has documented exceptions, including inline targets within a sentence.

## Accessible names

Every interactive element needs an accessible name — the text a screen reader announces when the element receives focus. The accessible name computation (W3C accname 1.2, §4.3.2 "Computation steps") consults sources in this order and stops at the first that yields a name:

1. `aria-labelledby` (step 2B) — resolves the referenced elements' text
2. `aria-label` (step 2C) — overrides everything below it
3. The native host-language label (step 2D) — a `<label>` for a form control, `alt` on an image
4. Name from content (step 2F) — the element's own text, e.g. `<button>Save</button>`

That is precedence, not preference, and the two run opposite. **Prefer** the sources at the bottom of the list: visible text is the name a sighted user reads, a keyboard user hears, and a voice-control user speaks, and it cannot drift out of sync with itself. Reach up the list only when there is no suitable visible text — `aria-labelledby` when the name already exists elsewhere on screen, `aria-label` last, because it is invisible and silently outranks the content it sits on.

The direct consequence: never put `aria-label` on an element that already has visible text content. The attribute wins, the visible text is discarded from the name, and the two diverge with nothing to flag it.

## Color contrast and visual indicators

Information must not be conveyed by color alone. Error states, validation feedback, status indicators — each needs a non-color signal (icon, text label, pattern). This is WCAG 1.4.1 Use of Color, Level A.

Text must meet contrast ratios: 4.5:1 for normal text, 3:1 for large text (WCAG 1.4.3, AA). "Large" means at least 18pt, or 14pt bold. If you cannot verify contrast at review time, flag it as a concern for manual verification.

## Focus management

When content appears or disappears dynamically (dialogs, toasts, route changes, list filtering), focus must move to the new content or remain on the triggering element. A dialog opens → focus moves into the dialog. A dialog closes → focus returns to the trigger. A toast appears → it should have `role="status"` and `aria-live="polite"` so screen readers announce it without moving focus.

Focus must never be trapped unintentionally. Modal dialogs must trap focus within their bounds. Non-modal overlays must not.

## Forms and validation

- Every input must have an associated label (not just placeholder text — placeholders are not accessible names).
- Error messages must be associated with their input (`aria-describedby` pointing to the error element).
- Error messages must be announced. Default to `aria-live="polite"` for inline field errors: they are already associated with the input via `aria-describedby`, so they are read when the user reaches the field, and validation that re-runs as the user types will otherwise interrupt them on every keystroke. Reserve `assertive` (and `role="alert"`, which is assertive) for the submission-time summary, where interrupting is the point.
- Required fields should use `aria-required="true"` if not expressed by the native `required` attribute.

## Images and media

- Decorative images must be removed from the accessibility tree, and an empty `alt` is the reliable way to do it. `role="presentation"` on an image that still lacks `alt` does not behave consistently.
- Informative images: descriptive `alt` text that conveys the meaning, not the appearance.
- Complex images (charts, diagrams): `alt` for summary + long description or linked detailed text.
- Never omit `alt` — missing `alt` causes screen readers to announce the filename, which is never useful.

## ARIA: use sparingly and correctly

ARIA is a last resort when semantic HTML cannot express the pattern. ARIA does not add behavior — it only exposes semantics. `role="button"` does not make a `<div>` respond to Enter/Space; you must add those handlers yourself. This is why native elements are preferred: they carry both semantics and behavior.

Common ARIA mistakes that MUST be flagged:
- `role` that duplicates the native element's implicit role (`<button role="button">`)
- `aria-hidden="true"` on a focusable element (screen reader skips it but keyboard users can still tab to it — a contradiction)
- `aria-label` on an element with visible inner text (overrides the visible text)
- `role="presentation"` or `aria-hidden="true"` on an element with focusable children

## Responsive and zoom

Users must be able to zoom to 200% without loss of content or functionality (WCAG 1.4.4). Fixed-width layouts that break at 200% zoom are a finding. Horizontal scrolling at 320px viewport width (WCAG 1.4.10 Reflow) is a finding for content that should reflow.

## What NOT to flag

- ARIA absent from markup that is already semantic — no ARIA is the correct amount of ARIA when the native element carries the role, and adding it is the finding.
- A component library primitive whose internals you did not audit, when the project has adopted it deliberately — flag the usage, not the dependency.
- A missing `aria-live` region for content the user just caused to appear at the point of focus — announcement is already handled by the focus move, and a live region there duplicates it.
- Contrast on disabled controls, decorative graphics, logotypes, and text over a photograph that also appears as real text elsewhere — WCAG 1.4.3 exempts these.
- Absence of a WCAG AAA criterion, unless the project has stated AAA as its target. AA is the usual conformance level; citing AAA as a defect is a finding against a bar nobody set.
- A keyboard pattern that differs from the ARIA authoring practices but is internally consistent and reachable — the practices are non-normative guidance, not a conformance requirement.
- Missing accessibility work on surfaces the change did not touch. Report the gap if it is severe; do not scope the change to fix the whole screen.

## Footguns

- `<div onclick>` without `role="button"` + `tabindex="0"` + keyboard handlers — not a button.
- `placeholder` as the only label — not an accessible name.
- `aria-live` region that updates on every keystroke — announcement spam.
- Focus trap in a non-modal overlay — keyboard users are stuck.
- `alt` text that describes the image file ("photo of team.jpg") instead of the content ("engineering team at 2024 offsite").
- Audio that starts automatically and runs beyond three seconds with no pause, stop, or independent volume control — WCAG 1.4.2. Silent motion that autoplays is a different criterion: automatically moving or updating content needs a pause control under WCAG 2.2.2.
