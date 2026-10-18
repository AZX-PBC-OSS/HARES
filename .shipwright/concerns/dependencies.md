---
description: "Dependencies and supply chain: choosing a maintained library over hand-rolled code, checking what the project already depends on, verifying a package is real before adding it, reproducible installs, letting new releases age before adoption, and keeping dependencies current against known vulnerabilities"
triggers:
  file_globs:
    - "**/package.json"
    - "**/pnpm-lock.yaml"
    - "**/package-lock.json"
    - "**/yarn.lock"
    - "**/bun.lockb"
    - "**/pyproject.toml"
    - "**/uv.lock"
    - "**/poetry.lock"
    - "**/requirements*.txt"
    - "**/Pipfile"
    - "**/Pipfile.lock"
    - "**/Cargo.toml"
    - "**/Cargo.lock"
    - "**/go.mod"
    - "**/go.sum"
    - "**/Gemfile"
    - "**/Gemfile.lock"
    - "**/composer.json"
    - "**/composer.lock"
    - "**/pom.xml"
    - "**/build.gradle"
    - "**/build.gradle.kts"
    - "**/*.csproj"
  content_patterns:
    - "^\\s*\"[@a-zA-Z0-9/._-]+\"\\s*:\\s*\"[\\^~>=<v ]*[0-9]+\\.[0-9]+"
---

# Dependencies and Supply Chain

Applies to any change that adds, removes, upgrades or pins a dependency, and to any change that implements a capability a well-known library already provides. It does NOT apply to changes that only use dependencies already present and unchanged.

Two failure modes sit here. One is re-implementing something the ecosystem solved years ago, shipping the bugs the library already fixed and creating maintenance the team never chose. The other is adding something unvetted, unpinned, or freshly published, and inheriting whatever is inside it.

## About this concern's content trigger

The single `content_patterns` entry above matches a dependency-manifest-shaped line (`"name": "^1.2.3"`) so a dependency added to a manifest this concern's globs do not name still selects it. It requires a `major.minor` value specifically, because a looser "starts with a digit" form matches any JSON field holding a timestamp or an id and fires on unrelated machine-written files. Even so it is the noisiest pattern in the bundled catalogue: any JSON mapping a name to a version-shaped string matches it. If it selects this concern on changes that have nothing to do with dependencies, delete the `content_patterns` block outright rather than tuning it — the file globs above already cover every mainstream manifest, and a trigger that fires on unrelated work costs more attention than it saves.

## Discover how this project manages dependencies

Before adding anything, find how the project declares dependencies, whether it commits a resolved lockfile, how versions are constrained, whether upgrades are automated, and whether an audit runs anywhere. Read what is already declared, including what those declarations pull in transitively. Follow the project's existing conventions for where a dependency is declared and how tightly it is constrained; do not introduce a second mechanism alongside them.

## Prefer a maintained solution over hand-rolled code

When a capability is genuinely provided by a well-maintained library — parsing, serialisation, date and time handling, retries and backoff, validation, cryptography, protocol clients, text processing — use it.

The reason is not that writing it is hard. It is that a library that has been used at scale has already found the edge cases you have not thought of yet, and that everything you write is maintenance the team is committed to forever without having decided to be. Cryptography and anything security-adjacent is not a judgement call at all: hand-rolled is a defect regardless of how simple it looks.

Hand-rolling *is* right in real cases, and pretending otherwise produces a rule that gets ignored:

- The need is genuinely trivial and the candidate library is far larger than the problem.
- No candidate is well-maintained: no recent activity, unresolved security reports, a single unresponsive maintainer, or no adoption to speak of.
- The fit is poor and using it would mean bending the project around the library, or carrying a dependency for a fraction of its surface.
- The dependency cannot be accepted here for stated reasons — footprint, licence, platform, or supply-chain policy.

Where you conclude hand-rolling is right, say which alternatives you assessed and why they lost. An unexplained hand-rolled utility reads as an unsearched one. Keep it small, keep it in one place, and put it behind a narrow interface so it can be swapped for a library later without touching its callers.

Duplication *within* the project — re-deriving a decision that already has a home in this codebase — is the same failure with a different source, and is covered by the wiring concern's search-before-you-build rule. Search both: the project first, then the ecosystem.

## Check what is already there before adding

A capability already reachable through an existing dependency is free; adding a second library that does the same job as one already present is a permanent inconsistency, where two parts of the codebase behave subtly differently forever and every contributor must learn which is used where.

Search the declared and resolved dependencies before adding. Where something is available only transitively, prefer declaring it directly rather than relying on another package's dependency, which can change without notice.

## Verify the package is real, and is the one you mean

A package name that seems obviously right is not evidence that it exists or that it is the project you are thinking of. Names are squatted, misspellings are registered deliberately, and packages recommended from memory sometimes do not exist at all — which attackers exploit by registering them, an attack that specifically targets machine-generated code.

Before adding a dependency, confirm from its registry listing and its source repository that it exists, that the name matches the project you intend, and that it shows signs of being maintained and used. Be most careful where the name is near a well-known package, where the package is new or little-used, and where you are recommending it from recollection rather than having just read about it.

## Installs must be reproducible

Commit the resolved lockfile — the artefact that records the exact resolved versions across the whole transitive graph — and make automated builds install from it without re-resolving. Without this, two builds of the same commit are different software, an upstream change lands with no diff, and a compromised release arrives silently.

Treat a lockfile change as a reviewable part of the change, not noise. Unexplained movement in the transitive graph is worth a question.

Where the installer offers both a strict and a lax no-resolve flag, take the strict one. `uv sync --locked` and `--frozen` both refuse to re-resolve, but only `--locked` fails when the manifest and the lockfile disagree; `--frozen` installs the lockfile anyway and produces a green build of an artifact missing a dependency somebody added and never re-locked. The ecosystem specifics are in the language packaging concerns.

## Let a new release age before adopting it

The window immediately after a version is published is the highest-risk moment in a package's life: a compromised maintainer account or build pipeline is usually found and the release pulled within hours to days, and the harm falls almost entirely on whoever installed it first. Waiting costs essentially nothing.

Do not adopt a version published within the last day, and prefer several days for anything with broad reach. The exception is a release that fixes an active exploited vulnerability, where the balance reverses — take it, and say why.

Where the ecosystem's tooling can enforce a minimum age at install or resolution time, configure it, and prefer that over relying on discipline. Where it cannot, make it a stated rule and check the publication date before adding or bumping — the date is on the registry listing.

## Keep dependencies current against known vulnerabilities

Currency is a security obligation, not housekeeping. Known vulnerabilities in dependencies are published, indexed, and trivially discoverable by anyone attacking the project; sitting on an old version is a decision to remain exploitable.

Your training data cannot tell you what is currently vulnerable — advisories are published continuously and the ones that matter are the ones newer than anything you know. Establish the current state at the time of the change: run the ecosystem's audit or advisory-check tooling, and where a specific dependency is in question, read its current advisories and release notes directly. Prefer running the tool over recalling anything.

Upgrade in small, reviewable steps rather than accumulating a wholesale bump nobody dares merge. Where an upgrade is not taken, that is a decision — record which version, why, and what compensates.

Running an audit in automation, and what it should block on, belongs with the project's gates; see the project quality infrastructure concern.

## Adding a dependency is a decision with an owner

New dependencies get named in the change and justified where reviewers will see it: what it does, why it rather than the alternatives or than writing it, and what it brings with it. A dependency slipped in without mention is a decision nobody made — and every dependency is permanent in practice, because removing one later means unpicking every use.

Weigh what arrives with it: its own dependency footprint, its licence against the project's obligations, its platform and runtime requirements, and whether it is maintained by more than one person.

## What NOT to flag

- A pinned or held-back version with a stated reason.
- A small, well-scoped internal utility where no suitable dependency exists, or where the assessed candidates genuinely lose.
- A dependency that is old but has no known vulnerability and no functional need to move.
- A deliberate, documented policy of minimising dependencies, applied consistently.
- Preferring a smaller library over a larger one you happen to know better.

## Footguns

- Adding a library for one small function while an already-declared dependency provides the same thing.
- Relying on a transitive dependency directly, so an unrelated upgrade removes it without warning.
- Loosening a version constraint to make an install resolve, converting a build failure into a silent behaviour change later.
- Regenerating a lockfile wholesale as a side effect of an unrelated change, hiding a large transitive movement in a diff nobody reads.
- Upgrading to a version published hours ago because the tooling reported it as available.
- Treating an audit tool's clean report as proof of safety: it covers published advisories for declared dependencies, and nothing else.
- Hand-rolling anything cryptographic, or anything that parses a format someone else's parser will also read.
- Vendoring or copying source out of a library to avoid a dependency, and inheriting its bugs with no upgrade path and no advisory coverage.
