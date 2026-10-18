---
description: "Changes to linter, formatter, type-checker or coverage configuration, wherever it lives — reviewing a config diff as a change to the project's quality bar: a rule disabled, a severity downgraded, an ignore glob widened, a strictness flag removed, files excluded from the program, a threshold lowered; plus what current compiler strictness and maintained linting options actually are"
triggers:
  file_globs:
    - "**/tsconfig*.json"
    - "**/jsconfig*.json"
    - "**/eslint.config.*"
    - "**/.eslintrc*"
    - "**/.eslintignore"
    - "**/biome.json*"
    - "**/biome.jsonc"
    - "**/.prettierrc*"
    - "**/prettier.config.*"
    - "**/ruff.toml"
    - "**/.ruff.toml"
    - "**/mypy.ini"
    - "**/.mypy.ini"
    - "**/pyrightconfig.json"
    - "**/.editorconfig"
    - "**/clippy.toml"
    - "**/.golangci.y*ml"
  content_patterns:
    - "continue-on-error:\\s*true"
    - "\"strict\"\\s*:\\s*false"
    - "ignore_errors\\s*=\\s*True"
---

# Tooling Configuration

Applies when a change edits the configuration of a gate — linting, formatting, type checking,
or coverage — whether that lives in a dedicated file or in a tool section inside a manifest
(`[tool.ruff]`, `[tool.mypy]`, `[tool.coverage]`, `[lints]`, an `eslintConfig` block). Establishing
gates for a project that lacks them is the project quality infrastructure concern; this file is
about editing gates that already exist.

The failure this targets is quiet: the bar gets lowered in a file nobody reads as code, in a
diff that looks like configuration, and every future defect in the affected scope goes
unreported. Nothing ever fails, so nothing ever prompts a second look.

## Establish what the gate enforces now, and whether it blocks

Before changing anything, read the current configuration through to its base: extended or shared
configs, per-glob overrides, and ignore lists together decide what is actually checked, and any
one of them can already be exempting the code in question. Then find out how the project runs it
— in particular whether warnings fail the build. That single fact decides whether half the
changes below are suppressions or not.

Confirm the file pattern still matches the files you think it does. A checker whose pattern
matches nothing, and a runner that finds no tests, both report success.

## A config change is a change to the standard

Each of these lowers the bar, and each reads in a diff as routine configuration:

- A rule turned off, or removed from an enabled rule set.
- A severity downgraded from error to warning — **a suppression wherever warnings do not fail
  the build**, and one that reads as a compromise while behaving as an off switch.
- A glob added to an ignore or exclude list, removing a directory from checking entirely.
- Files removed from the type-checker's program, which stops checking them and everything they
  would have flagged in what remains.
- A strictness flag removed or set to false.
- A coverage threshold lowered, or a path added to an omit list, so untested code reports clean.
- A warning filter that hides the deprecation the upstream library is telling you to fix.

The rule is the same one that applies to an in-line suppression, applied more strictly rather
than less: config-level suppression is invisible at the site of the problem, applies to every
current and future line in scope, and outlives the reason for it. The typescript and python
concerns spell this out per ecosystem; the rule holds whether or not either is attached.

So: fix the finding. Where it genuinely cannot be fixed now, suppress the narrowest scope the
tool allows — one rule, one line, in-line, with a comment naming the reason — rather than
widening a config-level exemption. Never add code you just wrote to an ignore, exclude or omit
list; that removes it from every gate the project has. And never relax a project-wide flag to
clear one file's errors: that is a conversation about the project's contract, not an edit.

Legitimate config changes exist and look different. Adopting a *stronger* rule set, narrowing an
over-broad existing ignore, excluding genuinely generated or vendored files, migrating to a new
config format, or recording an upstream constraint that the tool cannot express — all fine, and
all improved by saying so in the change.

## Compiler and checker strictness

Verify the current option set from the tool's own documentation rather than recalling it; these
tools add options continuously, defaults change between majors, and options are occasionally
*removed* — a compiler upgrade can turn a long-tolerated setting into a hard error, so a config
that was valid under the previous major is not evidence it is valid now. Read the release notes
for the version the project actually pins.

For TypeScript, `strict: true` is the floor, not the ceiling, and the options beyond it are where
real defects are caught — `noUncheckedIndexedAccess` (index access may be `undefined`),
`exactOptionalPropertyTypes` (an optional property is absent, not present-and-undefined),
`noImplicitOverride`, `noFallthroughCasesInSwitch`, `noPropertyAccessFromIndexSignature`,
`noUncheckedSideEffectImports`, plus `isolatedModules`, `verbatimModuleSyntax` and
`erasableSyntaxOnly` where the project is transpiled by a tool other than `tsc`. Raising one of
these is worth its churn; removing one is a project-wide weakening.

`skipLibCheck` is the option most often misjudged in both directions: it is widely used and
frequently justified, because third-party declaration files are outside your control — but it is
still a reduction in what is checked, and turning it on to clear an error in your *own*
declarations is a suppression.

For Python, the equivalent axis is the type checker's strict mode and the breadth of the enabled
lint rule set. Note that the checker's model differs from TypeScript's: rather than a set of
boolean flags, the mainstream Python checkers set a baseline mode and then re-level individual
diagnostics on top of it, per-directory where needed. That makes the durable suppressions
mode-shaped rather than flag-shaped — a project silently left on a default mode below strict, a
diagnostic dropped to `none` repository-wide, or a per-module override switching checking off for
a package.

## Which linter

The maintained options in the JavaScript and TypeScript ecosystem are oxlint, Biome, and ESLint
(with typescript-eslint for TypeScript rules); all three are actively developed. The meaningful
difference is how each implements type-aware rules — the expensive ones, and also the ones that
catch what a syntax-only pass cannot see, such as a floating promise or an always-true condition.
The three now differ in kind, not just in speed: oxlint delegates to the TypeScript compiler
itself (via tsgolint), Biome infers types with its own engine (opt-in, and slower when enabled),
and ESLint uses typescript-eslint against a real program. Whichever the project runs, the question
to ask of the config is whether type-aware checking is switched on at all — it is opt-in
everywhere, and a gate running without it is missing the findings it most wanted.

Confirm the current state before choosing rather than recalling it: this part of the ecosystem has
changed substantially and recently, and rule coverage and stability claims from training data are
likely to be describing a previous era.

Whichever the project uses, running two linters with overlapping rule sets and no stated division
of labour is a maintenance cost with no gain — they disagree, and contributors learn which output
to ignore. If a change adds a second, it should say what each one owns.

## What NOT to flag

- A rule disabled with a stated reason at the point of disabling.
- An exclusion covering genuinely generated, vendored, or third-party files.
- `skipLibCheck` used for third-party declarations, as most projects do.
- A rule set the project deliberately chose and applies consistently, even if you would have
  picked differently.
- A new strict flag scoped so it blocks for changed code first, with a recorded plan to widen —
  that is how a strictness increase lands without a flag day.
- A suppression whose comment names a real upstream constraint. That is the resolution, not the
  defect.
- Formatter settings. Once formatting is automatic, its content is not worth an opinion.

## Footguns

- Downgrading a rule to warning in a project where warnings do not fail the build, and calling it
  a compromise.
- Adding the file you just wrote to an ignore list to get a gate green.
- Widening an ignore glob so it silently swallows files nobody intended to exempt.
- An `exclude` in a type-checker config that removes files from the program, so errors disappear
  from files that were never mentioned.
- A per-glob override that disables a rule family for a directory that then becomes where all new
  code is written.
- Enabling a strict flag repository-wide with no scoping, so the next contributor's first act is
  to switch it off.
- A suppression with no recorded reason: it can never be safely removed, so it never is.
- Different tool versions locally and in CI, producing findings nobody can reproduce.
