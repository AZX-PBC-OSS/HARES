---
description: "Project bootstrap and quality infrastructure: establishing or fixing the automated gates a project is checked by — type/contract checking, linting, formatting, tests, and the automation that runs them — so that a green signal is evidence rather than decoration"
triggers:
  file_globs:
    - "**/.github/workflows/*.yml"
    - "**/.github/workflows/*.yaml"
    - "**/.gitlab-ci.yml"
    - "**/.pre-commit-config.yaml"
    - "**/Makefile"
    - "**/justfile"
    - "**/tsconfig*.json"
    - "**/eslint.config.*"
    - "**/.eslintrc*"
    - "**/biome.json"
    - "**/.prettierrc*"
    - "**/prettier.config.*"
    - "**/ruff.toml"
    - "**/.ruff.toml"
    - "**/mypy.ini"
    - "**/setup.cfg"
    - "**/tox.ini"
    - "**/.editorconfig"
---

# Project Quality Infrastructure

Applies when a change establishes or alters what checks a project is held to: a new project or a new component within one, adding or configuring a checker, linter, formatter or test runner, wiring checks into automation, or responding to a finding that the existing gates could not have caught. It does NOT apply to ordinary feature work in a project whose gates already exist and already work — follow those, do not extend them mid-feature.

The failure this concern targets is a project that reports success throughout while being broken, because the checks that existed could not see the defects and no one ever owned establishing checks that could. Setup work is the cheapest work in the project's life: it decides whether every later defect is caught in seconds by a machine or found expensively by a human, or by a user.

## Discover what the project already has

Establish the current state before proposing anything. Find the project's existing configuration for building, checking, formatting and testing, whatever files those live in; find whether automation runs them and which of them block; find how contributors are told to run them locally. Read enough of the existing source to see which conventions are actually followed rather than merely configured.

A project mid-life has conventions, and adding a second toolchain beside them is worse than the gap you were trying to close: two configurations disagree, contributors learn which one to ignore, and the disagreement itself becomes maintenance. Extend or repair what exists. Replace it only as a deliberate, stated migration that removes the old one in the same change.

## Verify currency before choosing or configuring anything

Your knowledge of this ecosystem's tooling is stale by construction. Versions, defaults, recommended configuration, and which tool the ecosystem has converged on all move faster than any model's training data, and tools are routinely superseded by successors that did not exist when you last learned about them.

Before selecting a tool, pinning a version, or writing a configuration file: search for the current state and read the tool's own current documentation. Prefer official documentation and the project's own release notes over recalled defaults and over third-party tutorials, which age silently. This is a standing obligation on every setup decision, not a step to skip when you feel confident — a confidently-stated stale default is more damaging than admitting uncertainty, because it is adopted without review.

State in the change what you verified and when. A configuration nobody can date is a configuration nobody dares change.

## The gates worth having

Establish, at minimum, the fastest feedback the ecosystem offers on each of:

- **Correctness of contracts** — type checking, or the strongest static analysis of interfaces and shapes the language supports. Where the language offers levels of strictness, choose the strictest the project can actually pass, and record the intent to raise it if it starts below that.
- **Common defects and dangerous constructs** — the ecosystem's standard static analysis, configured to a real rule set rather than the empty default.
- **Formatting** — automatic and non-negotiable, so that formatting is never discussed again and diffs contain only meaning.
- **Tests** — a runner invoked by one documented command, with a way to run the whole suite and a single case.
- **Code that nothing reaches** — whatever the ecosystem offers for unreferenced exports, unimported files, and declared-but-unused dependencies. Local checks cannot see an unused edge between two files, which is why dead and unwired code survives type checking, linting and per-file review untouched.
- **Supply chain and secrets** — known vulnerabilities in dependencies, and credentials reaching version control. Both are cheap to check continuously and expensive to discover late.

Each one established once, run automatically, and **blocking**. A check that reports without blocking is advice, and advice is ignored under deadline. If a check cannot block yet because the existing code fails it, scope it so that it blocks for changed code immediately and record the plan to widen it — never leave it purely advisory with no path to enforcement.

## A green signal must mean something

Gates configured so permissively that a broken project passes are worse than no gates, because they manufacture confidence and stop anyone looking. The configuration is the deliverable, not the presence of the tool.

Establish that each gate can fail. Confirm it by observing a known-bad change being rejected — an obvious type violation, a deliberate rule breach, a failing assertion — rather than by reasoning that it ought to be. A gate never seen to be red has not been shown to work; misconfiguration, an unmatched file pattern, an over-broad ignore file, or a runner silently finding no tests all present as success.

Blanket suppression at the configuration level deserves the same scrutiny as suppression in the code: a rule disabled project-wide, a directory excluded from checking, or a failure tolerated by the automation removes a whole class of defect from view permanently and silently. Narrow it, or justify it where it is written.

## Local and automated checks must be the same checks

A check that runs only in the pipeline moves discovery to the slowest, most expensive point. A check that runs only on one contributor's machine protects nothing and drifts immediately. Define the checks once, invoke them the same way in both places, and make the local invocation trivial to find and to run.

Where the ecosystem supports running checks before code is committed or pushed, prefer that for the fast ones. Keep the pre-commit set fast enough that nobody is tempted to bypass it; leave the slow suite to automation.

## What NOT to flag

- A mature project's deliberate, documented tooling choices, including choosing a different tool than you would have.
- The absence of a category of tool the ecosystem genuinely does not have.
- A rule disabled with a stated reason at the point of disabling.
- A gap in a feature ticket's own project infrastructure — flag it, but building the infrastructure is its own piece of work, not a silent expansion of the feature.
- Style preferences the project's formatter does not enforce.

## Footguns

- A checker configured over a file pattern that matches nothing, or a test runner that finds no tests: both report success.
- Adopting a configuration recalled from memory, or copied from a tutorial, that the tool's current version no longer supports or has changed the meaning of.
- Generated, vendored, or third-party code included in the checked set, producing noise that trains everyone to ignore the output — or an exclusion so broad it removes real source alongside it.
- Automation that runs the checks but does not fail the build on their exit status.
- Adding a gate the existing codebase cannot pass, with no scoping and no plan, so the first act of the next contributor is to switch it off.
- Different tool versions locally and in automation, producing failures nobody can reproduce.
- Treating the presence of a configuration file as the goal. The goal is that a bad change is rejected.
