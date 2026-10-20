Concern documents live here — each file describes a quality dimension (security, accessibility, performance, i18n, etc.) that agents apply when the concern is relevant to a ticket's scope.

## Frontmatter

**`description`** drives agent-side selection: an agent chooses concerns by reading descriptions and nothing else. Write it so an agent can decide from that one line alone whether the concern applies to a given ticket — what it covers, and where its boundary with a neighbouring concern falls.

**`triggers.file_globs`** (optional) drives code-side selection. Before an implement, review, or fix phase runs, the files the worktree changes relative to the initiative's base branch are matched against these globs; a concern with a match is added to that phase's concern list even when no agent named it.

```yaml
---
description: "..."
triggers:
  file_globs:
    - "**/*.ts"
    - "**/*.tsx"
---
```

Globs are matched against repo-relative POSIX paths with dotfiles included.

**`triggers.content_patterns`** (optional) drives the same code-side selection from what a change *wrote* rather than which files it touched. Each entry is a regular expression, matched against the individual lines the change added — the added lines of every tracked file's diff, plus every line of a new file git has not seen before. A concern is selected when a glob matches **or** a pattern matches; the two arms are independent.

```yaml
---
description: "..."
triggers:
  file_globs:
    - "**/*.test.ts"
  content_patterns:
    - "vi\\.mock\\("
---
```

Patterns are matched **case-sensitively**, and inline flag groups (`(?i)`) are not supported — write the casing you want into the pattern itself (`[Pp]assword`, or an explicit alternation). Because YAML double-quoted strings consume one level of backslash, a regex `\.` is written `\\.` above. A pattern that does not compile is skipped with a warning; the concern's other patterns and all of its globs still apply.

Content patterns exist for what a path can never tell you — a `vi.mock(` added to a plain helper module that no test-file glob names. They are heuristics for pulling a concern into scope, not detectors: a match means "this concern is worth reading", never "there is a defect here". Keep them narrow enough that they fire on the material the concern actually covers.

Triggers only ever *add* a concern — they never remove one an agent selected, and nothing is silently dropped — so keep both arms specific. A change that trips an unusually large number of triggers is reported in the run trace as a signal that some concern's triggers are too broad. A concern with no `triggers` block is selected by description alone. No field other than these is read.

## How a concern reaches an agent

1. **Selection (agent).** An agent reads the `description` of every concern in the catalogue — workspace concerns, falling back to these bundled defaults for any stem the workspace does not override — and writes the ones it judges relevant into the ticket's `concern_paths` frontmatter, as bare filename stems (`typescript`) or paths (`resources/defaults/concerns/typescript.md`).
2. **Selection (triggers).** In the same step, the phase's changed files are matched against every catalogue concern's `triggers.file_globs` and the lines the change added are matched against its `triggers.content_patterns`; matches from either arm are appended to the list the ticket declared. Declared entries keep their order and win de-duplication; trigger-derived ones follow in catalogue order.
3. **Resolution.** Before an implementer, reviewer, or fixer agent runs, each entry is resolved to a file: a bare stem resolves workspace-first then bundled-default; a path-shaped entry resolves relative to the workspace, or is used as-is when absolute. Results are de-duplicated and keep their order.
4. **Injection.** The resolved absolute paths go into the agent's prompt as pointers. The agent reads the files itself — content is never copied into the prompt.

There is no priority ordering — file globs and content patterns are the whole of the automatic evaluation. A workspace without git, or a phase whose change set cannot be determined, falls back to the declared list with a warning in the run's trace. Content scanning is bounded: a file whose diff adds an unusually large number of lines is left out of the content pass (its path still matches globs), and a very large change stops accumulating lines once a budget is reached. Both degrade the guidance and are traced; neither fails the phase.

When a concern is selected by a trigger *after* an agent has already been given its prompt — because the agent's own work is what tripped the trigger — the concern's path is named in that phase's done-challenge, so the agent reads it before its completion is accepted. Concerns the ticket already declared are not repeated there.

An entry that resolves to no file is dropped from the list and recorded as a warning in the run's diagnostic trace; the remaining entries still resolve and the run continues. A misspelled stem therefore costs the agent that concern without stopping it, so check spelling against the filename stems in this directory.
