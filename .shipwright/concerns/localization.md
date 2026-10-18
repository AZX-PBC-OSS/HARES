---
description: "Localization (i18n): all user-facing text must be externalized, locale-aware formatting, layout resilience for text expansion"
triggers:
  file_globs:
    - "**/locales/**/*"
    - "**/locale/**/*"
    - "**/i18n/**/*"
    - "**/translations/**/*"
    - "**/messages/*.json"
    - "**/*.po"
    - "**/*.pot"
    - "**/*.ftl"
    - "**/*.arb"
    - "**/*.resx"
---

# Localization

This concern applies to any change that introduces, modifies, or removes user-facing text — visible labels, button text, error messages, tooltips, alt text, email/notifications, log messages directed at end users. It does NOT apply to internal developer-facing strings (debug traces, code comments, developer documentation) unless the project policy requires those to be localized too.

## Discover the project's localization tooling

Before reviewing or implementing, check the project's configuration for its localization system: message key format, translation file locations, compilation/build pipeline, and runtime API. Read how existing code externalizes strings, handles plural forms, and formats locale-aware values. Ensure new code follows the same patterns and uses the same functions. Do not invent a new i18n approach within the change — use what the project already has.

If the project has no localization system configured, flag user-facing hardcoded strings as a gap in the review findings but **do not set up a localization system yourself** — that is a project-infrastructure concern outside the scope of individual feature work. The right response is to note the gap; setting up i18n is a separate initiative.

Key things to verify against the project's existing code (when i18n is configured):
- Every user-facing string goes through the project's translation function (not hardcoded)
- Plural forms use the project's plural support (not `count === 1 ? singular : plural` — many languages have more than two plural forms)
- Dates, numbers, and currencies use the project's locale-aware formatting (not hand-rolled patterns)
- Translated text containers accommodate expansion (the project may already have established patterns for this)

## No hardcoded user-facing strings

Hardcoded source-language text in the rendered output is a finding — it cannot be translated without a code change. This includes: button labels, headings, paragraph text, form validation messages, error descriptions, empty-state text, placeholder text, tooltips, accessible names derived from visible text, and status messages.

## Message keys, not string values

Tests and mocks must reference the same localization functions as production code. A mock that hardcodes the English value for an `aria-label` or button text silently passes when the key is renamed or its translation changes. Call the same message function — this way a key rename fails the test at compile time.

Where the project's tooling can check keys at build time rather than at render time, use that mode. A lookup resolved at runtime turns a renamed or deleted key into a raw identifier on screen — or an empty string — discovered by whoever opens that page; a system that generates typed accessors from the message catalogue turns the same mistake into a build failure, and makes a missing interpolation parameter fail with it. If the project already has this, a hand-written string key passed to a generic `t('some.key')` lookup is bypassing it.

## Text expansion

Translated text is usually longer than the source, and short strings expand most: a label under ten characters can reach two to three times its original length, while a long paragraph typically grows by around a third (W3C i18n, *Text size in translation*). Design so any string can grow without clipping rather than budgeting a specific percentage — the figures are tendencies, not guarantees, and some translations are shorter.

UI layouts must accommodate text expansion. Common failures:
- Truncated text in fixed-width containers
- Buttons that overflow or clip with longer labels
- Side-by-side layouts that break when one label is longer
- Dropdowns or menus where text wraps unexpectedly

Flexible layouts (flexbox, grid, natural content flow) handle expansion. Fixed pixel widths and `overflow: hidden` on text containers are risks.

## Locale-aware formatting

Numbers, dates, currencies, and lists vary by locale. Use the platform's locale-aware formatting APIs, not hand-rolled patterns:

- Numbers: locale-appropriate decimal separators, grouping, and digit sets
- Dates: locale-appropriate order (MDY, DMY, YMD), separators, and 12/24-hour time
- Currency: symbol placement, decimal digits, grouping
- Plurals: many languages have more than two plural forms. Russian and Polish have four CLDR cardinal categories; Arabic uses all six (zero, one, two, few, many, other). Use the localization system's plural support, not `count === 1 ? 'item' : 'items'`.

Never assume a specific date format, number separator, or plural rule. The project's locale APIs handle this.

## String concatenation is a localization bug

```python
# Bad — word order varies by language
message = "Deleted " + count + " items from " + folder_name

# Good — interpolation with re-orderable placeholders
message = translate("deleted_items_from_folder", count=count, folder=folder_name)
```

Translators must be able to reorder placeholders. Concatenation locks the word order to the source language's grammar.

## Bidirectional text

If the project may be used in RTL locales (Arabic, Hebrew, Farsi), UI layout must use logical (start/end) properties rather than physical (left/right). In CSS the logical property names embed the axis and the side — `margin-inline-start`, `padding-inline-end`, `border-inline-start`, `inset-inline-start` — with `margin-inline` / `padding-inline` as the both-sides shorthands. `inline-start` and `inline-end` on their own are not properties; they are the flow-relative *values* used by properties such as `float` and `text-align` (`text-align: start` is the fix for a hardcoded `text-align: left`). Pair this with `dir="auto"` on user-generated content — but note it infers direction from the first strongly-directional character in the element and applies it to the whole, so text of unknown direction sitting inline with other content needs isolating in its own element rather than relying on the container.

## What NOT to flag

- Developer-facing strings: log messages, trace output, exception text read by operators, CLI diagnostics, code comments, test fixture data. Externalizing these buys nothing and costs a lookup on every line.
- Internal identifiers that happen to be English words — enum members, message keys, route names, database values. The rule is about what is *rendered*, not what it is called.
- A string not yet translated into every locale, when the project's fallback chain handles a missing translation. Untranslated is a translation-workflow state; unexternalized is the defect.
- Absence of RTL support in a project that has stated no RTL locale. Logical properties are still the better default; their absence is an observation there, not a finding.
- Text expansion headroom on a container whose content is bounded and non-linguistic — a numeric readout, a timestamp, an icon-only control.
- Missing i18n infrastructure. If the project has no localization system, that is one finding about the project, not one finding per string.

## Footguns

- Hardcoded English in a mock — silently passes when the real key changes.
- String concatenation for messages — breaks word order in translation.
- `count === 1 ? singular : plural` — only works for English. Many languages have more plural forms.
- Fixed-width containers for text — truncated translations.
- A date or number formatted with no explicit locale argument — falls back to the host environment's locale *and time zone*, not the locale the user chose in the application. The time zone is usually the sharper bug.
- Embedding format patterns in translation strings — "dd/mm/yyyy" is locale-specific. Let the locale API format dates.
