---
description: "Rust packaging and ecosystem practice: changes to Cargo manifests, the lockfile, the pinned toolchain, or Rust source — toolchain and edition pinning, dependency currency and advisory auditing, feature-flag hygiene, lint configuration in the manifest, where unsafe needs justification, and the build and test tooling the ecosystem actually uses"
triggers:
  file_globs:
    - "**/Cargo.toml"
    - "**/Cargo.lock"
    - "**/*.rs"
    - "**/rust-toolchain.toml"
    - "**/rust-toolchain"
    - "**/clippy.toml"
    - "**/.cargo/config.toml"
---

# Rust

Applies to changes touching Cargo manifests, the lockfile, the pinned toolchain, or Rust
source. General library-versus-hand-rolled judgement is in the dependencies concern; this file
covers what is specific to Cargo and to this ecosystem's practice.

## Discover the project's toolchain and conventions

Read `rust-toolchain.toml` (or `rust-toolchain`) for the pinned channel, the edition in
`Cargo.toml`, the `[lints]` and `[features]` tables, any `[workspace]` layout, `.cargo/config.toml`
for build configuration, and whether the project runs clippy, an advisory audit, or a dedicated
test runner. Match what is there. A workspace has one answer for these, inherited — adding a
divergent per-crate answer is churn, not improvement.

## Pin the toolchain

`rust-toolchain.toml` pins the channel (`stable`, a dated `nightly`, or an exact version) and can
also declare `components` (clippy, rustfmt) and `targets`, so contributors and CI compile with the
same compiler and the same lints. Without it, "works on my machine" is a compiler-version
difference and clippy findings appear and vanish per contributor.

Pinning an exact version rather than `stable` is a real choice with a cost — it must be raised
deliberately or the project ages into an unsupported compiler. Pinning `nightly` needs a stated
reason naming the unstable feature that requires it.

The edition in `Cargo.toml` is separate from the compiler version and changes language defaults;
do not raise it as a side effect of unrelated work.

## Dependencies, lockfile, and cooldown

Commit `Cargo.lock`. The Cargo team's guidance now recommends committing it as the starting point
for libraries as well as binaries, so an uncommitted lockfile in a library is a decision to
justify rather than a default. Have CI build against it with `--locked` so a build cannot silently
re-resolve.

Cargo's default caret requirements mean a `cargo update` can pull a fresh upstream release the
moment it is published — the highest-risk window in a version's life, since a compromised release
is normally caught and yanked within hours to days and the harm lands on whoever installed first.
Do not adopt a version published within the last day; prefer several days for anything with broad
reach. Cargo's own minimum-publish-age support was accepted as RFC 3923 and is not stable at the
time of writing — check whether it has landed before relying on it, and until then treat the
publication date as something to look at before bumping.

For advisories, run the tooling rather than recalling what is vulnerable: `cargo-audit` checks
`Cargo.lock` against the RustSec advisory database, and `cargo-deny` covers the same advisories
plus licence policy, duplicate-version bans and permitted sources in one configurable gate. Both
are current and maintained. Either belongs in automation; neither tells you anything about code
that is not a declared dependency.

For a published library, `cargo-semver-checks` catches accidental breaking changes in a version
bump — an API break shipped as a patch release breaks every consumer's build.

## Feature-flag hygiene

Cargo features are additive and unified across the graph: enabling a feature anywhere in a
workspace enables it everywhere that crate is built. So a feature that *removes* or *changes*
behaviour rather than adding it produces a build that depends on who else is in the tree, and two
crates in the same workspace cannot disagree.

Practical consequences worth checking on any manifest change:

- Depend with `default-features = false` and name what you need, where the default set carries
  weight you do not use — but only if the crate documents its defaults as optional; some are not
  usable without them.
- An optional dependency plus a feature that enables it is the way to make something opt-in;
  guard the corresponding code with `#[cfg(feature = "...")]` so a build without it still compiles.
- Every combination the project offers must build. Feature combinations that were never compiled
  together are broken by default, discovered by whoever first enables them.

## Make the most of the type system

Rust's type system is the strongest of the mainstream three, so the question here is rarely how to avoid escaping it — it is how much of the checking is actually being asked for. A check the compiler performs is a check that runs outside whoever wrote the code, deterministically, at every call site, forever. Every place a type is left vague is a check declined.

- **Newtypes over primitive obsession.** `struct UserId(u64)` and `struct Meters(f64)` cost nothing at runtime and make an argument swap a compile error. Two `u64` parameters in a signature are an outage waiting for a distracted caller; two newtypes cannot be transposed.
- **Enums over booleans and stringly-typed state.** `fn render(&self, draft: bool, preview: bool)` has four call sites and two of them are meaningless. An enum names the states that exist and makes the rest unwriteable. Same for a `String` holding one of five known values — that is an enum with the checking removed.
- **Make invalid states unrepresentable.** Prefer `enum Connection { Idle, Open(Socket) }` to a struct with an `is_open: bool` and an `Option<Socket>` that must agree. When two fields can contradict each other, some code path eventually makes them. Parse into the constrained type at the boundary rather than validating repeatedly downstream.
- **`#[must_use]`** on any type or function whose result must not be dropped — a `Result`-like type, a builder, a guard, a function that is pure and pointless if ignored. It converts "the caller has to remember" into a warning at every call site that forgets.
- **Exhaustive matching is the payoff, and `_ =>` spends it.** A catch-all arm on your own enum means adding a variant compiles clean everywhere and silently takes the fallback branch — the exact bug exhaustiveness exists to prevent. Match the variants explicitly, or bind and handle. `_` is right for genuinely open sets (a `#[non_exhaustive]` upstream enum, an integer range) and wrong for a closed enum you own.
- **`Box<dyn Any>` and heavy `dyn` discard what the compiler just gave you.** `Any` plus `downcast_ref` is runtime type checking bolted onto a language that does it statically — it is `any` with extra steps and a panic path. Reach for an enum over the known set, or a trait with the methods you actually call. `dyn Trait` is correct for genuine heterogeneity and plugin boundaries; it is a smell when the set of implementors is known and closed.

### `unwrap`, `expect`, and `panic!`

`unwrap()` converts a value the type system told you might not be there into a runtime abort. The `Option`/`Result` was the compiler reporting something; `unwrap` acknowledges the report and ships anyway.

The honest cases are real and common: after a check the type system cannot express (a regex literal that is known-valid, an index just bounds-checked), on a genuinely unrecoverable invariant violation, and throughout tests and prototypes where a panic *is* the failure report. What makes them acceptable is that the reason is stated — `expect("BUILTIN_RE is a compile-time literal")` documents the invariant at the site and produces a usable message when the assumption turns out to be wrong. Bare `unwrap()` gives a line number and nothing else.

The line worth holding is **library code**: a panic in a library removes the caller's ability to handle the failure at all. Return `Result` and let the binary decide where to `expect`. `clippy::unwrap_used` and `clippy::expect_used` exist for enforcing this — both are `restriction`-group and allow-by-default, with `allow-unwrap-in-tests` / `allow-expect-in-tests` in `clippy.toml` so test code is not caught ([Clippy lint configuration](https://doc.rust-lang.org/clippy/lint_configuration.html)).

## Where `unsafe` needs justification

`unsafe` moves a proof obligation from the compiler to the author, so it needs the proof written
down. Every `unsafe` block carries a comment stating the invariants that make it sound and why
they hold here; every `unsafe fn` documents what a caller must guarantee. From the 2024 edition
the compiler warns when an unsafe operation sits in an `unsafe fn` without its own `unsafe` block,
which is the same discipline enforced mechanically — keep the blocks narrow so each one has one
justification.

Where a crate has no need for it, `unsafe_code = "forbid"` in the manifest's `[lints.rust]` table
makes that structural rather than aspirational. Reaching for `unsafe` to silence a borrow-checker
error is the case to reject outright: the checker is reporting a design problem, and the `unsafe`
does not fix it, it hides it. FFI, and a measured, benchmarked hot path where the safe form was
tried first, are legitimate.

## Lints belong in the manifest

The `[lints.rust]` and `[lints.clippy]` tables set lint levels for the package in one place, with
levels `forbid`, `deny`, `warn` and `allow` — and a workspace can hold one policy that member
crates inherit. That is better than scattering crate-level attributes through source, because a
reader can see the whole policy and a reviewer sees a change to it in the diff.

That visibility is the point: changing a lint from `deny` to `allow`, or adding an `#[allow(...)]`
over a module, is a change to the project's quality bar and is reviewed as one. An `#[allow]` on a
single item, with a comment naming the reason, is the narrow form to prefer. Deny-by-default over
the whole warning set is the opposite failure — it makes every new compiler release a build break.

**Which level.** `forbid` cannot be overridden by an inner `allow` — attempting it is itself an
error — so it is the right level for a property that must hold with no exceptions anywhere in the
crate (`unsafe_code` in a crate that has no business having any). `deny` is an error that a
justified, visible `#[allow]` can still except; that is the level for a rule with real but rare
exceptions. `warn` is for a rule you intend to act on, and it only means that if warnings fail the
build in CI (`RUSTFLAGS="-Dwarnings"` on the CI invocation, not `#![deny(warnings)]` in source —
the crate-level attribute makes every future compiler release a build failure for downstream
consumers too).

**Which lints.** `cargo clippy` runs `clippy::all` by default: `correctness` (deny),
plus `suspicious`, `style`, `complexity` and `perf` (warn). Three groups are off entirely, and
that is where the additional value is — but not wholesale. Clippy's own documentation recommends
cherry-picking from `pedantic` rather than enabling the group, and states explicitly that
enabling all of `restriction` is *not* recommended (it will warn about a
`#![warn(clippy::restriction)]` attribute if it finds one), because those lints restrict language
features and many contradict each other. `nursery` is unfinished by definition. So: enable
`clippy::all` in CI, then name the specific `pedantic` and `restriction` lints the project wants
in `[lints.clippy]` — `unwrap_used`, `expect_used`, `panic` and `todo` for library crates being
the usual candidates ([Clippy lint groups](https://doc.rust-lang.org/clippy/lints.html)).

## A warning is a finding the compiler couldn't prove

A rustc or clippy warning means the tool found a shape that is *usually* wrong and cannot prove it
is wrong here. That inability is why it warns rather than errors — it is handing over a judgement
it is not entitled to make. An `#[allow]` with no reason answers that judgement with "I did not
look", which is the same trade as `unwrap` one level up: a mechanism converted back into trust. A
deprecation warning is the clearest instance — the crate is telling you for free, in advance,
exactly what the next upgrade breaks.

- **Warnings fail the gate; they are not a backlog.** A warning nobody must act on is one everyone
  learns to scroll past, and the useful ones drown in it within a release.
- **`#[allow]` sits exactly where the constraint is** — on the single item, with a comment naming
  the constraint rather than the lint: `#[allow(clippy::too_many_arguments)] // FFI shim mirrors
  the C signature verbatim`. A module-level or crate-level `allow` covers code that has not been
  written yet.
- **The same `#[allow]` repeated many times is not many exceptions.** It is one missing
  abstraction — introduce it once and allow there — or a lint that does not suit this project,
  which belongs in `[lints.clippy]` as a deliberate `allow` with a reason, where a reader sees it.
  Deciding once in the open is the good outcome; deciding never, invisibly, at fifteen sites is not.

## Build and test tooling

Two tools solve real, specific problems here, and both are current and actively maintained:

- **sccache** caches compiler invocations, locally or in shared remote storage (S3, GCS, Redis,
  Azure, GitHub Actions cache and others), so CI and colleagues stop recompiling identical inputs.
  Its documented limits matter before anyone promises a speedup: crates that invoke the system
  linker — `bin`, `dylib`, `cdylib` and `proc-macro` — are not cached, and incrementally compiled
  crates are not cached either, which by default includes workspace members in the debug profile.
  The gain is on dependency compilation, and it is largest in CI.
- **cargo-nextest** runs each test in its own process, which gives real isolation (one test's
  memory corruption or global-state assumption cannot affect another), reliable per-test timeouts
  and retries, and usually faster wall-clock runs on larger suites. Its one significant gap is
  doctests, which it does not run: a project that swaps `cargo test` for `cargo nextest run` and
  does not add a separate `cargo test --doc` step silently stops testing every doc example.

Neither is required. Adopt them where the cost they address is real, and where the caveat above
does not quietly remove coverage.

## What NOT to flag

- A pinned or held-back dependency, toolchain, or edition with a stated reason.
- `unsafe` in FFI or in a measured hot path, with its invariants documented.
- A crate that keeps default features because the dependency does not usefully work without them.
- `cargo test` rather than nextest, or no build cache, in a project small enough that neither buys
  anything.
- The project's choice of `cargo-audit` or `cargo-deny` — running one is the point, not which.
- A single `#[allow]` with a reason at the point it is written.
- A lint set to `allow` in `[lints.clippy]` with a stated reason because it genuinely does not fit
  the project — that is the decision made in the open, which is exactly what is being asked for.
  The finding is the same lint silenced at fifteen sites with nobody ever deciding.
- `unwrap` / `expect` in tests, build scripts, examples, and `main` — a panic there *is* the
  failure report. The rule is about library code taking the choice away from its caller.
- `expect("...")` after a check the type system cannot express, where the message names the
  invariant.
- `_ =>` on a `#[non_exhaustive]` upstream enum, an integer range, or any genuinely open set —
  exhaustive matching is only buying something on a closed enum the crate owns.
- `dyn Trait` at a genuine plugin or heterogeneous-collection boundary; a primitive that is not
  newtyped where there is only one of it in the signature and no swap is possible.
- Not running `pedantic` or `restriction` lints — Clippy itself recommends against enabling those
  groups wholesale. Cherry-picking is the correct posture, and picking none of them is a
  defensible one.

## Footguns

- Swapping in nextest without adding `cargo test --doc`, losing every doctest with no failure.
- Expecting sccache to speed up a workspace's own incremental debug builds — it does not cache them.
- A `cargo update` bundled into an unrelated change, moving the whole graph in a diff nobody reads.
- A feature that disables behaviour rather than adding it, so an unrelated crate in the workspace
  turns it on and changes your build.
- A feature combination that is offered but never compiled in CI.
- `unsafe` reached for to get past the borrow checker.
- An exact-version toolchain pin nobody owns raising, until the project cannot use current crates.
- `#![deny(warnings)]` at crate root, turning every new compiler release into a build failure —
  and for downstream consumers too. Fail warnings via `RUSTFLAGS` in CI instead.
- `_ =>` on an enum the crate owns, so a new variant compiles clean and silently takes the
  fallback — the one bug exhaustive matching exists to prevent.
- A `bool` parameter pair whose combinations include states that cannot legally occur.
- A struct whose fields can contradict each other (`is_open: bool` beside `Option<Socket>`).
- `Box<dyn Any>` plus `downcast_ref` over a set of types that is known and closed — runtime type
  checking re-implemented on top of a compiler that already did it, with a panic path added.
- `#[must_use]` missing on a builder or guard, so a dropped result is silent.
- CI running `cargo clippy` without `-- -D warnings`, so findings are printed and the job passes.
- CI building without `--locked`, so the lockfile is committed but not actually enforced.
