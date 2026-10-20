---
description: "Python packaging and dependency manifests: changes to pyproject.toml, requirements files, or a Python lockfile: runtime dependency versus dependency group, pins that need a stated reason, committed lockfiles and reproducible installs, verifying a package is real before adding it, letting a release age before adopting it, currency against advisories, suppression written into the manifest's tool sections, and uv's own semantics (--locked versus --frozen, dependency groups versus extras, which environment a command installs into, and the environment variables that change all three)"
triggers:
  file_globs:
    - "**/pyproject.toml"
    - "**/requirements*.txt"
    - "**/requirements/*.txt"
    - "**/uv.lock"
    - "**/uv.toml"
    - "**/.python-version"
    - "**/poetry.lock"
    - "**/setup.py"
    - "**/setup.cfg"
  content_patterns:
    - "UV_(FROZEN|LOCKED|NO_SYNC|PROJECT_ENVIRONMENT|PYTHON)\\b"
    - "\\buv (sync|run|add|remove|export|build|lock)\\b"
---

# Python Packaging and Dependencies

Applies to any change that edits a Python dependency manifest, lockfile, or the tool
configuration living inside one. General library-versus-hand-rolled judgement is in the
dependencies concern and general Python code style is in the python concern; this file is
about the manifest itself.

## Discover how this project declares and installs dependencies

Establish which manager owns the project before editing anything: `uv.lock`, `poetry.lock`,
a `requirements.txt` compiled from a `.in`, or a bare hand-maintained requirements file each
imply a different edit. Read the existing declarations, their constraint style, whether the
lockfile is committed, and how CI installs. Add through the manager's own command so the
lockfile moves with the manifest; a manifest edited by hand while the lockfile stays behind
is a build that installs something other than what the file says.

Do not introduce a second mechanism alongside the existing one — a `requirements.txt` added
to a `uv`-managed project, or a `[tool.poetry.dependencies]` block beside `[project]`, means
two sources of truth that disagree silently.

## Runtime dependency or dependency group

`[project.dependencies]` is what a consumer installing this package receives. Test runners,
linters, type checkers, docs builders and local tooling do not belong there — they belong in
`[dependency-groups]` (PEP 735, supported by uv, and by pip via `--group` and Poetry's recent
releases) or in the project's established equivalent. `[project.optional-dependencies]` is
for extras a *consumer* opts into, not for the team's own tooling; the two are frequently
confused.

The consequence of getting it wrong is not cosmetic: a test framework in runtime
dependencies is installed into every deployment and every downstream consumer, and every one
of its advisories becomes yours.

**Under uv the group-versus-extra choice decides whether `--no-dev` does anything at all.**
`--no-dev` prunes a *dependency group*. It has no effect on an extra, so a `dev` set declared
as `[project.optional-dependencies]` is installed by any build that names it and cannot be
excluded by the flag written to exclude it. Measured on a real deployment image: pytest, ruff,
pyright, mkdocs and testcontainers all shipped to production because `dev` was an extra rather
than a group. Moving the same set to `[dependency-groups]` took the image from 1.24GB to 968MB
with no other change. If a `--no-dev` in a Dockerfile or CI job is meant to be doing work, check
which table the excluded set actually lives in.

## A pin needs a stated reason

An upper bound or an exact pin is a promise the project will not receive that dependency's
fixes. That is sometimes right — a known incompatibility, a required ABI, a reproducibility
requirement — and when it is, say so where the pin is written. An unexplained upper bound
outlives its cause and nobody dares remove it, because nobody knows what it was for.

Constraining a library's own dependency ranges tightly is worse than constraining an
application's: it propagates into every consumer's resolution and causes conflicts the
consumer cannot fix.

## The lockfile is the pin; the manifest is the intent

Commit the resolved lockfile and have automation install from it without re-resolving
(`uv sync --locked`, `poetry install` against a checked lockfile, `pip install --require-hashes`
against a compiled requirements file). Without that, two builds of the same commit are
different software.

**Use `uv sync --locked`, not `--frozen`.** Both refuse to re-resolve, so neither can invent a
version the lockfile never described, so that is not the difference. The difference appears when
`pyproject.toml` and `uv.lock` disagree, which is what a dependency added to the manifest and
never re-locked looks like. `--frozen` ignores the disagreement and installs the lockfile,
producing a green build of an artifact silently missing the new dependency. `--locked` fails
and tells you to re-lock. Both files are already in the build context, so the check is local and
costs no network. `--frozen` is only for a case where you knowingly want the lockfile despite a
manifest that has moved, and that case should say so where it is written.

Review the lockfile diff. Unexplained transitive movement — new packages nobody asked for,
a large graph shift attached to a one-line manifest change — is the shape a supply-chain
incident takes, and it is invisible if lockfile diffs are waved through as noise.

## uv: which environment a command actually acts on

uv's convenience is that most commands sync the project environment before doing their work.
That implicit sync is where environments silently change, and the behaviours below were
measured against uv 0.11.32 rather than inferred from the documentation.

**A bare `uv run` does not prune extras, until the interpreter disagrees.** Its implicit sync
is inexact: it installs what is missing and never uninstalls, so an environment carrying an
extra keeps it. The destructive case is an interpreter *disagreement*. Reproduced:
`uv sync --python 3.13 --extra extra` gives an environment of 2 packages with the extra
present; a following bare `uv run` in a project whose `.python-version` says 3.12 prints
"Creating virtual environment", installs 1 package, and the import that worked a moment ago
fails. uv deleted the venv and rebuilt it from the *default* set.

The control that follows from this is to pin the interpreter for the whole job, with `UV_PYTHON`
in the environment, so no command can disagree with another. `uv run --no-sync` is defence in
depth on top of that, not a substitute for it. A per-command `--python` binds only that one
command; the next uv call resolves from `.python-version` again and you are back in the same
place.

**`uv add` does not prune; `uv remove` does.** `uv remove` re-syncs exactly and reports what it
took out (`- idna`), including packages belonging to an extra it was never asked to touch. Read
its output rather than assuming it removed only the name you gave it.

**`uv run --no-sync` with no environment present creates an empty one and runs.** So it can never
shrink an environment, which is the property you want in CI. It is also not evidence that
an environment exists. It will happily run your command against nothing.

**Environment variables, and which are safe to export job-wide:**

- `UV_NO_SYNC` is ignored by `uv sync`, `uv add` and `uv export`. That is exactly what makes it
  safe to export for a whole job: it suppresses the implicit syncs without being able to disable
  a deliberate install.
- `UV_FROZEN=1` combined with a `--locked` flag is a hard error, so a job that exports
  `UV_FROZEN` breaks every `--locked` command in it. Do not set it.
- `UV_LOCKED=1` breaks `uv lock` and `uv add`, because both need to write the lockfile. It
  belongs in CI and never in a developer shell or a devcontainer image, where a project that has
  no lockfile yet cannot bootstrap one.

**`UV_PROJECT_ENVIRONMENT` must point outside the repository.** Pointed inside the tree it breaks
`uv build` with "The source distribution includes a virtual environment". uv writes a
`.gitignore` inside the venv it creates, which hides the directory from `git status` but not
from the sdist builder, so the failure arrives at package time with nothing in the working tree
to explain it.

## Declare a build backend

A `pyproject.toml` with no `[build-system]` table falls back to an unpinned default backend that
is fetched from the network on every build, including every container image build. Declare the
backend and pin it like any other dependency; a build that reaches the network for its own
tooling is neither reproducible nor available offline.

## Verify the package exists and is the one you mean

The general principle — confirm the package is real, matches the project you mean, and watch for name confusion and model-invented names — is in the dependencies concern. Ecosystem-specific: import name and distribution name differ often enough (`PIL`/`pillow`, `yaml`/`PyYAML`) that recognising the import is not evidence the distribution name is right.

## Let a release age before adopting it

The general principle — wait before adopting a new release, and exempt actively exploited vulnerability fixes — is in the dependencies concern. Ecosystem-specific enforcement: uv's `exclude-newer` accepts a duration (`exclude-newer = "1 week"` under `[tool.uv]`, with `exclude-newer-package` to exempt individual packages), and pip's `--uploaded-prior-to` accepts an ISO 8601 datetime or a duration in days. Prefer configuring it over remembering it.

## Currency against advisories

Run the tooling rather than recalling — the general principle is in the dependencies concern. For this ecosystem: `pip-audit` (PyPA, checks an environment or a requirements file against the Python advisory database and OSV) and `osv-scanner` (Google, covers lockfiles across ecosystems including `uv.lock` and `poetry.lock`) are both current and maintained. A clean report covers published advisories for declared packages and nothing more.

## Suppression written into this file counts as suppression

`pyproject.toml` and `setup.cfg` hold the configuration for the project's gates, and an edit
there is the most durable way to hide a warning: it silences the rule for every current and
future line in scope, sits nowhere near the code it excuses, and reads in a diff as
configuration rather than as a lowered bar. **Hold it to a stricter standard than an in-line
`# noqa`, not a looser one.**

All of these are suppressions and need the justification an in-line ignore would need:
`[tool.ruff.lint] ignore`, `per-file-ignores` over a directory, `[tool.ruff] exclude`,
`[[tool.mypy.overrides]] ignore_errors`, `filterwarnings = ["ignore::DeprecationWarning"]`,
and `[tool.coverage.run] omit`. Never add code you just wrote to an exclude or omit list —
that removes it from every gate the project has. The python concern covers this in full,
including where a coverage exclusion is defensible; the rule holds whether or not that
concern is also attached.

## What NOT to flag

- A pin, upper bound, or held-back version with a stated reason.
- A project deliberately not committing a lockfile for a library, where that decision is
  recorded and consumers resolve for themselves.
- A dependency that is old but has no known advisory and no functional reason to move.
- `requirements.txt` in a project that has consistently chosen pip — matching the project's
  manager is correct, migrating it is a separate piece of work.
- A tool in a dependency group you would have placed in a different group, where the project
  groups things consistently.

## Footguns

- Editing the manifest by hand and leaving the lockfile stale, so the declared and installed
  graphs differ and nothing reports it.
- Regenerating the whole lockfile as a side effect of an unrelated change, burying a large
  transitive movement in a diff nobody reads.
- Loosening an upper bound to make resolution succeed, converting a resolver error into a
  silent behaviour change at runtime.
- Test or lint tooling declared in `[project.dependencies]`, shipped to every consumer.
- Adopting a version the tooling reported as available minutes after it was published.
- Adding a dependency for one helper that the standard library or an already-declared package
  provides.
- A `# type: ignore` or `noqa` moved into the manifest as a project-wide `ignore` entry to
  clear a gate, and never revisited.
- `uv sync --frozen` in CI, which passes on a manifest that was edited and never re-locked.
- Dev tooling in `[project.optional-dependencies]` where `--no-dev` cannot reach it, so the flag
  meant to exclude it does nothing and it ships.
- A bare `uv run` after a deliberate `uv sync --extra ...`, which rebuilds the environment from
  the default set the moment the interpreter disagrees.
- `UV_FROZEN` exported alongside `--locked` commands, which is a hard error rather than a
  redundancy.
- `UV_LOCKED` set in a developer shell or devcontainer, where it blocks `uv lock` and `uv add`
  and a project with no lockfile cannot bootstrap.
- `UV_PROJECT_ENVIRONMENT` pointing inside the repository, which builds fine until `uv build`
  refuses the sdist.
- No `[build-system]` table, so every image build downloads an unpinned build backend.
