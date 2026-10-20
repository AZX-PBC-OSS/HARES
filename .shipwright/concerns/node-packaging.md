---
description: "Node and TypeScript packaging and dependency manifests: changes to package.json, a JavaScript lockfile, or installer configuration — dependencies versus devDependencies and what actually ships, the transitive surface, install scripts as an execution vector, engine and package-manager pinning, committed lockfiles and reproducible installs, verifying a package is real, letting a release age, and currency against advisories"
triggers:
  file_globs:
    - "**/package.json"
    - "**/pnpm-lock.yaml"
    - "**/package-lock.json"
    - "**/yarn.lock"
    - "**/npm-shrinkwrap.json"
    - "**/.npmrc"
    - "**/.yarnrc.yml"
    - "**/vite.config.*"
---

# Node and TypeScript Packaging

Applies to any change that edits a JavaScript or TypeScript dependency manifest, lockfile, or
installer configuration. General library-versus-hand-rolled judgement is in the dependencies
concern and language-level rules are in the typescript concern; this file is about the
manifest.

## Discover how this project installs

Establish which package manager owns the project from the lockfile present, and use that
manager's own commands to add or upgrade so the manifest and lockfile move together. Read the
existing `.npmrc` or `.yarnrc.yml` — hardening settings, registry scoping, and script policy
already established there are the project's decisions, not defaults to re-derive. Two
lockfiles in a repository is a defect: the second one is installed by nobody and audited by no
one, while contributors resolve against whichever their tool picks.

## The transitive surface is the dependency

This ecosystem's packages are small and deeply nested, so one direct addition routinely brings
in dozens of transitive packages, each with its own maintainers and its own publish access.
The direct name you evaluated is a fraction of what you accepted. Before adding, look at what
the package pulls in; a large tree for a small capability is a real cost, not a stylistic one.

Prefer declaring something directly over reaching it transitively — a transitive package can
vanish under an unrelated upgrade with no diff of yours to explain it. Where the project needs
to force a transitive version for a fix, use the manager's own override mechanism
(`overrides` / `resolutions`) and say why, rather than pinning the parent to an old release.

## Install scripts are arbitrary code execution

`preinstall`, `install` and `postinstall` scripts in dependencies run with your shell, your
environment, and your credentials, at install time, before any of your code runs. This is the
mechanism most recent registry compromises have used to reach developer machines and CI.

The ecosystem has moved to blocking them by default: pnpm has disabled automatic dependency
`postinstall` execution since v10, with `allowBuilds` to permit named packages, and Yarn's
`enableScripts` now defaults to `false`. npm still runs them unless `ignore-scripts=true` is
set, so on npm this is a decision the project has to make explicitly. Where a package genuinely
needs a build step — native modules, mostly — allow that package by name rather than
re-enabling the whole class.

A change that adds `dangerouslyAllowAllBuilds`, sets `enableScripts: true`, or removes
`ignore-scripts` is a security-relevant change to the project's posture, and is reviewed as
one.

## dependencies versus devDependencies

The distinction is load-bearing here in a way it is not everywhere. `dependencies` are
installed by every consumer of a published package and, for an application, are what a
production install carries: size, attack surface, and every advisory. Build tooling, test
runners, linters, type definitions and bundlers belong in `devDependencies`.

Both directions are defects. A build tool in `dependencies` ships a compiler to production. A
package imported by shipped code but declared in `devDependencies` produces an install that
works locally and fails only in the pruned production environment — which is a runtime failure,
not a build failure, so it surfaces late.

Where a build inlines its dependencies into an artefact, the boundary shifts; establish how
this project actually produces what it deploys before ruling on placement.

**The bundler config is the other half of this decision, which is why `vite.config.*` triggers
this concern.** `build.rollupOptions.external`, `ssr.noExternal`, and the equivalents in other
bundlers decide which declared dependencies are inlined into the artefact and which are left as
runtime imports the deployed environment must supply. A package moved to `external` becomes a
production install requirement whether or not `package.json` says so, and a package inlined from
`devDependencies` ships its code regardless of the pruning. Read the two together: the manifest
alone does not tell you what the artefact needs. (Where a `vite.config.*` also holds the Vitest
`test` block, the vitest concern covers that half.)

## Engines and package-manager version

Pin what the project is built with. `engines` states the Node versions supported; a
`packageManager` field states which manager and version. Be aware that Corepack — the shim
that made `packageManager` self-enforcing — is no longer distributed with Node from v25, so
the field alone no longer guarantees anyone gets that version. Check how the project actually
installs its manager (a pinned devDependency, a setup action, a documented install) and make
the manifest agree with it, rather than assuming the field is enforced.

Mismatched manager versions between contributors and CI produce lockfile churn and failures
nobody can reproduce locally.

## The lockfile is the pin

Commit the lockfile and have automation install from it without re-resolving — `npm ci`,
`pnpm install --frozen-lockfile`, `yarn install --immutable`. An install that re-resolves in CI
means the tested tree and the shipped tree are different software.

Review the lockfile diff. Unexplained transitive movement, a resolved URL pointing somewhere
other than the configured registry, or a package that appears with no manifest change to
explain it, are all worth a question — this is the shape a registry incident takes.

## Verify the package is real, and let it age

The general principle — confirm the package exists, matches the project you mean, and let a release age before adopting it — is in the dependencies concern; this section covers the ecosystem-specific enforcement. Every major manager now enforces release aging rather than leaving it to discipline: pnpm's `minimumReleaseAge` (minutes, defaulting to one day as of v11), Yarn's `npmMinimalAgeGate` (a duration string, defaulting to `1w`, with `npmPreapprovedPackages` for exemptions), and npm's `min-release-age` (days, unset by default, with `min-release-age-exclude`). The units differ per manager — read the setting's own documentation rather than copying a number across. A fix for an actively exploited vulnerability reverses the balance — take it, exempt it explicitly, and say why.

## Currency against advisories

Run the tooling rather than recalling — the general principle is in the dependencies concern. For this ecosystem: the manager's own `audit` command, or `osv-scanner`, which reads the lockfile directly. A clean report covers published advisories for what is declared, and nothing else.

Where a package publishes provenance attestations, prefer it — but absence of provenance is not by itself a finding; adoption is still partial across the registry.

## What NOT to flag

- A pinned or held-back version with a stated reason.
- An install script allowed by name for a package that genuinely needs to build.
- A large transitive tree for a package that genuinely earns it, where the project has
  considered it.
- A dependency that is old but has no known advisory and no functional need to move.
- The project's choice of package manager, or of a lockfile policy applied consistently.
- A package in `dependencies` that a bundled application does not strictly need pruned, where
  the project's deployment makes the distinction moot and that is understood.

## Footguns

- Hand-editing `package.json` and leaving the lockfile stale, so declared and installed trees
  differ silently.
- CI running `install` rather than the frozen-lockfile command, re-resolving on every build.
- Deleting and regenerating the lockfile to resolve a conflict, replacing a reviewable conflict
  with an unreviewable wholesale change.
- Widening a version range to make a peer-dependency warning go away, accepting an untested
  combination.
- An `overrides` / `resolutions` entry added to force a fix and never removed, silently pinning
  a transitive package for years.
- Re-enabling install scripts globally to fix one native module's build.
- A type-definitions package in `dependencies`, or a package imported by shipped code sitting
  in `devDependencies`.
- Treating an audit report's advisory count as the goal and upgrading past a breaking change to
  clear a finding on a package that is not reachable at runtime.
