# Baseline repair and dependency upgrade

These entries form the opening sequence (`plan.md` §3): they take the
fast-forwarded `feat/output-results` head from Task 0 to a head that passes
every line of the verification block, and they land the dependency upgrade
the decision log requires (`decision-log.md` → `dl-i08-architecture.dependencies`).

Order, fixed: BR-1, BR-3, BR-2, BR-9, then `03-output-reporting.md` OR-5
(which is BR-5's repair), BR-4, BR-7, BR-8, then `02-measurement.md` M-1 and
M-2 (the golden tool and the pre-upgrade captures the upgrade is compared
against), then BR-6a, BR-6b, BR-6c, BR-6d, BR-11 (before `02-measurement.md`
M-3), all committed. `07-profiler.md` PR-1 to PR-6 except PR-4 were
committed next, then `03-output-reporting.md` OR-11 (row 19, `b8195c83`)
and `10-slop-cleanup.md` SL-7 (row 20, `7b7a6a7d`). `08-consumer.md`
CN-17 (row 16a) lands gated on the latest engine commit when it runs,
and BR-10, BR-12, BR-13, BR-14 and BR-15 follow in that order
(`00-schedule.md` rows 16b to 16f), before the row 16g B0 re-check and
`11-other-defects.md` OD-15 (row 16h). BR-16 to BR-19 record the found
defects fixed in their own commits while the opening rows ran
(`00-schedule.md` rows 0e, 8a, 9a and 10a). Each entry names the block
lines (`plan.md` §3 labels) it turns green. Every entry runs every block
line that is already green at that point and must leave it green; from
BR-6b on every entry runs the whole block.

Evidence is `file:line` at `ebdb1a17`. PR #27 inserts lines into two files
cited here, so at the Task 0 head `crates/hares-python/src/py_dwelling.rs`
lines after 685 move down by 41 and `crates/hares-python/Cargo.toml` lines
after 10 move down by 1; every other citation holds unchanged.

## BR-1: `hares-equipment` does not compile under `observe`

**Kind:** fix.

**Problem:** the workspace fails to compile with the `observe` feature (and
therefore with `--all-features`): a `tracing::debug!` call names a local
`sum_sq_dod` that no binding provides. `check_invariants` alone compiles,
because `hares-core`'s `check_invariants` feature does not enable
`hares-equipment/observe`.

**Evidence:** `crates/hares-equipment/src/battery/mod.rs:1468`, inside the
`#[cfg(feature = "observe")]` block at 1464-1472. The `observe` binding at
1457-1459 captures only `q_li1_before` and `cell_temp_for_tafel`.

```
$ cargo check -p hares-equipment --all-features
error[E0425]: cannot find value `sum_sq_dod` in this scope
  --> crates/hares-equipment/src/battery/mod.rs:1468:21
```

**Change:** add `sum_sq_dod` to the `observe` binding at 1457-1459, read from
`self.rainflow.sum_squared_dod_daily()` (`degradation.rs:144`,
`pub(crate)`). Any read before `self.rainflow.reset_daily()`
(`battery/mod.rs:1494`) sees the day's value; `update_daily` only borrows the
counter. Diagnostics only; no physics change.

**Acceptance test:** `cargo check --workspace --all-targets -F observe` and
`cargo check --workspace --all-targets --all-features` exit 0.

**Verification:** turns `[test-observe]` and `[test-observe-profiling]`
green. `[clippy]` still fails on the BR-3 and BR-2 lints.

**Commit:** `fix: capture sum_sq_dod for the observe-build degradation log`

**Dependencies:** Task 0 (`plan.md` §3).

## BR-3: a nested `dst` test never runs

**Kind:** fix.

**Problem:** a `#[cfg(feature = "dst")] #[test]` function is nested inside
another test's body. The harness does not collect inner items, so it has
never run. It also carries planning IDs and a comment that stands in for an
assertion, and its name claims a solar/Perez consistency check it does not
make: it asserts the civil day of year and that irradiance is populated.

**Evidence:** `crates/hares-core/tests/timezone_weather_regressions.rs`: the
outer `spring_forward_day_of_year_at_15min_resolution_all_steps` has `#[test]`
at 607 and starts at 608; the nested function occupies 650-727 (the file ends
at 728). `T-0212` appears at 653-658 and 723-726, with "No separate assertion
needed; this comment documents the contract" at 726.

```
$ cargo check --workspace --all-targets -F dst
warning: cannot test inner items
   --> crates/hares-core/tests/timezone_weather_regressions.rs:651:5
```

**Change:** move the function to module scope as its own
`#[cfg(feature = "dst")] #[test]`, renamed
`spring_forward_day_of_year_at_hourly_boundary_steps`, with its assertions
unchanged. Delete the `T-0212` comments and the comment that replaces an
assertion. The standing guard against nested tests is the rustc lint
`unnameable_test_items`, which `[clippy]` (`--all-targets --all-features
-- -D warnings`) turns into an error.

**Acceptance test:** `cargo nextest run -p hares-core -F dst
spring_forward_day_of_year_at_hourly_boundary_steps` runs one test and it
passes.

**Verification:** `[test-dst]` now executes the test. `[clippy]` still fails
on the BR-2 lints.

**Commit:** `fix: un-nest the DST spring-forward hourly boundary test`

**Dependencies:** Task 0.

## BR-2: clippy fails under `observe`

**Kind:** fix.

**Problem:** once BR-1 and BR-3 land, `[clippy]` still fails on seven lints:
five at `ebdb1a17` and two that PR #27 adds in its profiling example. The
five compile only under `observe`, which `--all-features` enables; the
example's two fire under `--all-features` because the example builds with
`profiling` on.

**Evidence:**

- `crates/hares-core/tests/equipment_ordering_tests.rs:532` and `:536`:
  `unnecessary_map_or` (the file's `observe` gate is at 461).
- `crates/hares-core/tests/resstock_observer_diag.rs:30`: `ptr_arg` (`&PathBuf`
  where `&Path` suffices; the file is `#![cfg(feature = "observe")]` at 10).
- `crates/hares-core/src/observer_capture.rs:335` and `:345`:
  `field_reassign_with_default` (module gated at `lib.rs:16`).
- `crates/hares-core/examples/profile_sim.rs:20` (Task 0 head): two
  `ptr_arg`, the `fixture` and `weather` parameters of `config`.

**Change:** apply clippy's own fix at each site: simplify both `map_or`
calls, take `&Path` at `resstock_observer_diag.rs:30` and in
`profile_sim.rs`'s `config`, and fold both field assignments into their
struct literals. The example's `config` and the imports only it uses are
gated on `profiling` (`10-slop-cleanup.md` SL-9), so it compiles
warning-free under every feature set. No behavior change.

**Acceptance test:** `cargo clippy --workspace --all-targets --all-features
-- -D warnings` exits 0.

**Verification:** turns `[clippy]` green.

**Commit:** `fix: clear clippy lints in observe-only test helpers and observer capture`

**Dependencies:** BR-1, BR-3.

## BR-4: CI does not run the verification block's cargo lines

**Kind:** fix.

**Problem:** CI guards a subset of what the block checks, so a regression in
`dst`, `profiling`, a debug default-feature run or `--all-targets
--all-features` clippy reaches `initial-implementation` unseen.

**Evidence:** `.github/workflows/ci.yml:36` runs `cargo clippy --workspace
-- -D warnings` (no `--all-targets`, no `--all-features`). The `test` job
(`ci.yml:135-137`) runs debug `-F observe`, release `-F check_invariants`, and
release `-p hares-equipment`; there is no workspace debug default-feature run
and no `-F dst` or `-F profiling` run. `dst` gates `clock.rs`,
`environment.rs`, `blueprint.rs`, `dwelling/mod.rs` and the timezone tests
(`crates/hares-core/Cargo.toml:14`).

**Change:** `ci.yml:36` becomes the `[clippy]` line. The `test` job gains
`cargo nextest run --workspace --no-fail-fast --profile ci` (debug default),
`... -F dst`, `... -F profiling` and `cargo nextest run --workspace --release
--no-fail-fast --profile ci` (plain release, green since OR-5). The
advisory check joins CI in BR-6b and the stray check in BR-7, each in the
commit that makes it pass.

**Acceptance test:** each new CI command, run locally with `--profile ci`,
exits 0; the workflow file lists them.

**Verification:** no block line changes state.

**Commit:** `ci: run clippy on all targets and features, and test default, dst, profiling and plain release builds`

**Dependencies:** BR-2 (clippy must pass before CI enforces it), BR-3,
OR-5 (the plain release run passes).

## BR-5: invariants are compiled out of plain release builds

**Kind:** fix, implemented by `03-output-reporting.md` OR-5, which the
opening sequence runs in this position (after BR-2, before BR-4).

**Problem:** `cargo nextest run --workspace --release` (default features)
fails three tests, because the dwelling-level invariant pass and the
output-schema drift check exist only in debug and `check_invariants` builds.
The wheel ships with neither (`pyproject.toml` `[tool.maturin]` sets no
features), so a shipped build never checks SOC bounds, EV capacity telemetry,
zone temperature bounds, electrical and reactive balance, fuel coverage or
thermal balance. The decision log requires physics invariant violations to
fail the run in every build profile (`dl-i08-architecture.health`). The
repair is the whole of OR-5 (every gated check in `hares-core` classified and
made unconditional, including during warm-up); this entry records the
baseline failure and fixes OR-5's position.

```
FAIL hares-core dwelling::tests::build_equipment_column_map_still_panics_on_schema_drift_for_known_equipment
FAIL hares-core::equipment_id_regressions ev_soc_check_fails_loudly_when_core_output_soc_is_absent
FAIL hares-core::equipment_id_regressions ev_capacity_check_fails_loudly_when_ev_telemetry_keys_are_absent
```

**Evidence:**

- `Dwelling::check_invariants` is `#[cfg(any(debug_assertions, feature = "check_invariants"))]`
  at `crates/hares-core/src/dwelling/mod.rs:7025`, and its only call,
  `mod.rs:6291`, carries the same gate. The `soc_bounds` and
  `ev_capacity_degraded` errors the two `equipment_id_regressions` tests
  expect (`crates/hares-core/tests/equipment_id_regressions.rs:2475,2517`)
  are raised inside it.
- `InvariantChecker` (`crates/hares-core/src/invariants.rs`) has its real
  methods under `#[cfg(any(test, debug_assertions, feature = "check_invariants"))]`
  (`:52`) and a second `impl` of no-op stubs for every other build
  (`:705-788`); its fields are gated the same way (`:28-41`), and the module
  doc (`:1-9`) states that release builds compile the checks to nothing.
- `resolve_col`'s drift panic (`mod.rs:658-662`) is gated the same way, so
  `build_equipment_column_map_still_panics_on_schema_drift_for_known_equipment`
  (`#[should_panic]`, `mod.rs:15226`) finds no panic in release.

`resolve_col` feeds `build_equipment_column_map` (`mod.rs:376`), which
`refresh_equipment_caches` (`mod.rs:3794`) calls; three public methods that
call it are infallible today (`auto_register_actors` `:3167`,
`clear_equipment` `:3574`, `remove_equipment_by_end_use` `:3605`), so a
returned drift error changes their signatures and their callers
(`crates/hares-python/src/py_dwelling.rs`, `py_blueprint.rs`,
`crates/hares-core/tests/equipment_id_regressions.rs`,
`orchestration_parity.rs`, `tests/conditioned_oracle.rs`,
`tests/freefloat_oracle.rs`).

**Change:** none of its own: OR-5.

**Acceptance test:** OR-5's; in particular the two `equipment_id_regressions`
tests pass unchanged under `[test-release]`, and the `#[should_panic]` test is
replaced by `build_equipment_column_map_errors_on_schema_drift_for_known_equipment`.

**Verification:** OR-5 turns `[test-release]` green;
`[test-release-invariants]` stays green.

**Commit:** none; OR-5's commit, landing in this position.

**Dependencies:** BR-1, BR-3, BR-2 (OR-5's own dependencies).

## BR-7: tests write `dwelling_*.csv` files into the working tree

**Kind:** fix.

**Problem:** test helpers build dwellings with the `write_output: true`
default and no `output_path`, so output lands at a working-directory-relative
`dwelling_{bldg_id}.{ext}`. `cargo nextest` writes under `crates/hares-core/`;
pytest writes from its working directory; `.gitignore` (`dwelling_*.csv`) hides
the files from `git status` but not from the disk.

**Evidence:**

- `git status --porcelain --ignored --untracked-files=all` at `ebdb1a17` lists
  fourteen files: `crates/hares-core/dwelling_0.csv`, `dwelling_1.csv`,
  `dwelling_100_diagnostics.csv`; root `dwelling_0.csv`,
  `dwelling_0_diagnostics.csv`, `dwelling_1.csv`, `dwelling_1_diagnostics.csv`,
  `dwelling_2.csv`, `dwelling_3.csv`, `dwelling_42.csv`,
  `dwelling_42_diagnostics.csv`, `dwelling_88.csv`, `dwelling_99.csv`; and
  `tests/python/dwelling_42.csv`. Task 0 lists them for the operator and
  removes them (`plan.md` §3).
- Rust default: `crates/hares-io/src/config.rs:86-87` with
  `default_write_output` at `:206-208`; the fallback path is
  `default_output_path`, `crates/hares-core/src/dwelling/conversions.rs:590-596`.
- Python defaults: `crates/hares-python/src/py_config.rs:201`
  (`write_output.unwrap_or(true)`) and `crates/hares-python/src/py_dwelling.rs:2320-2325`.
  `tests/python/conftest.py` builds every shared fixture dwelling with
  `bldg_id=42` and no output settings (`:30-40`).
- `benches/common.rs:127` sets `write_output: true` with an explicit temporary
  `output_path` (`:126`), so benches do not write into the tree.

**Change:**

- Every Rust test, in-crate `#[cfg(test)]` helper, example and Python test
  that builds a dwelling for a purpose other than testing file output passes
  `write_output: false` (Python: `write_output=False`); tests of file output
  pass an explicit `tempfile`/`tmp_path` path. The census covers
  `crates/*/src`, `crates/*/tests`, `tests/`, `benches/`, `crates/*/examples`
  and `tests/python`, found by the compiler (the field is typed) and by
  `git grep -nE 'from_hpxml|make_dwelling|from_config|from_toml_config' -- tests/python`.
- Construction paths that read a fixture's TOML (`from_toml`, the synthetic
  TOML constructors, the parity harness's `config.toml`) set
  `write_output: false` on the parsed config, because the fixture files
  carry no output settings and the defaults write.
- The diagnostics writer at `output_verbosity >= 4` ignores `write_output`
  and derives its path from `output_path`, so every test at that verbosity
  also passes an `output_path` in a temporary directory.
- The defaults themselves are removed by `03-output-reporting.md` OR-3; this
  entry changes callers only.
- `.github/workflows/ci.yml`: the `test` and `python-test` jobs gain a stray
  check: after the build step, list ignored and untracked files with the
  `[stray]` exclusions (`plan.md` §3), run the tests, list again, and fail on
  any path in the second listing that is not in the first.

**Acceptance test:** block runs before this entry recreate files from the
Task 0 list; those paths (already approved by the operator in Task 0) are
removed first, so the tree holds no `dwelling_*` file. The whole block then
runs with an empty `[stray]` comparison: no line of the block writes a file
outside the excluded build directories. `[stray]` detects only new paths,
which is why the run must start from that clean tree.

**Verification:** turns `[stray]` green.

**Commit:** `fix: stop tests from writing dwelling output files into the working tree`

**Dependencies:** Task 0 (strays removed first, so the check starts clean).

## BR-8: eleven tests are `#[ignore]`d

**Kind:** fix.

**Problem:** eleven `#[test]` functions are ignored, so they never run in any
gate. Each is a print-only investigation helper: none asserts a physical
result (their only assertions are that a run produced the expected number of
steps or snapshots). An ignored test is dead code that reads like suppressed
coverage. Two comments in `solar_parity.rs` also claim ignored reference
tests that do not exist.

**Evidence** (`git grep -nE '#\[ignore' -- '*.rs'`):

- `crates/hares-core/tests/alignment_oracles_regressions.rs:515`
  `debug_ashp_channel_delta_report`, `:709` `debug_ashp_peak_step_details`,
  `:770` `debug_minisplit_channel_delta_report`;
- `crates/hares-core/tests/resstock_smoke.rs:299`
  `debug_bldg0000002_winter_heating`;
- `crates/hares-physics/src/solar.rs:2461`
  `debug_solar_poa_at_may_5_noon_denver`;
- `tests/bestest/bestest_diagnostic.rs:114`
  `bestest_diagnostic_900ff_600ff_annual` (the module's only test;
  `mod bestest_diagnostic;` at `tests/bestest/mod.rs:1`);
- `tests/bestest/mod.rs:404` `debug_bestest_600ff_observe_peak_terms`, `:506`
  `debug_bestest_warmup_observe_first_100_steps`, `:619`
  `debug_600ff_matrix_values`, `:665` `debug_600ff_heat_balance_at_peak`,
  `:882` `debug_900ff_min_temp_heat_balance`;
- `crates/hares-physics/tests/solar_parity.rs:187` ("full reference values are
  #[ignore]") and `:264` ("Marked #[ignore] until pvlib output is captured"),
  above tests that are not ignored and already hold pvlib values.

**Change:** delete the eleven functions, `tests/bestest/bestest_diagnostic.rs`
with its `mod` line, and every helper left unused (the compiler's dead-code
lint under `[clippy]` names them). Delete the two stale `solar_parity.rs`
comments. Inspection of a run is served by the `observe` capture and by
`frame_golden` (`02-measurement.md` M-1). The alignment oracle helper that
reads its run back from Parquet keeps its recorder on, writing to the
temporary path it already creates: BR-7's `write_output: false` would
otherwise leave it nothing to read.

**Acceptance test:** `git grep -nE '#\[ignore' -- '*.rs'` prints nothing, and
`cargo nextest list --workspace --all-features --run-ignored only` lists no
tests. Before the change the same listing holds 12 ignored test instances
under default features and 16 under `--all-features` (the `bestest` module
compiles into two targets, so its ignored tests count twice).

**Verification:** no block line changes state; the eleven functions leave
the suite and nothing else changes.

**Commit:** `test: remove print-only investigation helpers that were ignored tests`

**Dependencies:** Task 0.

## BR-9: the block's test lines run unbounded and exhaust memory

**Kind:** fix.

**Problem:** each nextest test runs as its own process, and nextest defaults
to one process per logical core (28 here). Several suites simulate a full
year, and the per-entry review runs block lines concurrently with the
implementer's runs, so concurrent invocations exhaust the host's memory
(observed: the host was killed during a review pass on 2026-09-30).

**Evidence:** `cargo nextest run --help`: `-j, --test-threads` (the number of
tests running at once, defaulting to the core count). The configuration
equivalent is `profile.<name>.test-threads`
(nexte.st/docs/configuration/reference); `test-groups` is a different
feature (per-group limits for named groups) and is rejected as an unknown
key under `profile.default` by nextest 0.9.137.

**Change:** `.config/nextest.toml` gains `test-threads = 8` under both
`[profile.default]` and `[profile.ci]`, with a comment naming the constraint.
No block line changes: the profile applies automatically. Measured after the
change: a run of `hares-core`'s fast tests peaks at 8 concurrent test
processes (plus nextest itself).

**Acceptance test:** `cargo nextest run --workspace --no-fail-fast` exits 0
with the peak concurrent test process count at or below 8; a nextest
invocation prints no unknown-configuration-key warning.

**Verification:** no block line changes state; runs stay inside memory.

**Commit:** `fix: bound test parallelism with a committed nextest profile`

**Dependencies:** none.

## BR-6a: raise the workspace `rust-version` to 1.89

**Kind:** initiative work (`dl-i08-architecture.dependencies`).

**Problem:** the upgraded crates require a newer toolchain than the workspace
declares. `nalgebra` 0.35.0 declares `rust-version` 1.89; `tzf-rs` 2.1.2
declares 1.88; `quick-xml` 0.42.0 and `criterion` 0.8.2 declare 1.86. The
workspace declares 1.87 (`Cargo.toml:29`). Raising it makes clippy's
MSRV-gated let-chain suggestion (`collapsible_if`, edition 2024 with MSRV
1.88 or later) fire at roughly 320 sites across `hares-equipment`,
`hares-io`, `hares-python`, `hares-core`, `hares-types`, `hares-envelope`,
`hares-tariff` and `hares-fleet`.

**Evidence:** `Cargo.toml:29` (`rust-version = "1.87"`); each crate's
published `rust_version` on crates.io.

**Change:** set `rust-version = "1.89"` and collapse every nested `if` clippy
reports into a let chain (`cargo clippy --fix --workspace --all-targets
--all-features`, then a read of every hunk). Fix any other lint the MSRV
change enables; suppress none.

**Acceptance test:** `[clippy]` exits 0; `frame_golden compare-all` (both
feature sets) passes: the refactor is bitwise-neutral on every M-2 fixture.

**Verification:** every green block line stays green.

**Commit:** `refactor: raise rust-version to 1.89 and collapse nested ifs into let chains`

**Dependencies:** BR-2, M-2.

## BR-6b: upgrade every Rust dependency to its latest release

**Kind:** initiative work (`dl-i08-architecture.dependencies`).

**Problem:** the workspace is behind on its direct dependencies, and
`cargo deny --all-features check advisories` fails with six advisories:

```
atomic-polyfill  unmaintained   RUSTSEC-2023-0089  (postcard -> heapless 0.7)
paste            unmaintained   RUSTSEC-2024-0436  (parquet 58; nalgebra 0.34 -> simba 0.9)
pyo3             vulnerability  RUSTSEC-2026-0176, RUSTSEC-2026-0177  (fixed in 0.29)
quick-xml        vulnerability  RUSTSEC-2026-0194, RUSTSEC-2026-0195  (fixed from 0.41)
```

**Evidence:** workspace requirements at `Cargo.toml:46-69`;
`crates/hares-python/Cargo.toml:19` (`pyo3-build-config`);
`crates/hares-core/Cargo.toml:39-49` (dev-dependencies). Latest releases on
crates.io on 2026-09-29, re-checked when the entry runs:

| Crate | Requirement now | Target |
|---|---|---|
| pyo3, pyo3-build-config | 0.28 | 0.29 |
| numpy | 0.28 | 0.29 |
| arrow, parquet | 58 | 59 (60.0.0 exists, but pyo3-arrow 0.19, its latest release, requires arrow 59) |
| nalgebra | 0.34 | 0.35 |
| quick-xml | 0.39 | 0.42 |
| tzf-rs | 1.3 | 2.1 |
| criterion | 0.5 | 0.8 |
| strum | 0.26 | 0.28 |
| sha2 | 0.10 | 0.11 |
| syn (hares-core dev) | 2 | 3 |

Every other direct dependency is already on its latest stable major or minor;
`cargo update` takes each to its latest patch. Pre-releases (`libc`
1.0.0-alpha, `approx` 0.6.0-rc) are not targets.

Call sites known to break: `crates/hares-python/build.rs:15`
(`InterpreterConfig::lib_dir`, deprecated in pyo3-build-config 0.29, fatal
under `-D warnings`); `crates/hares-io/src/hpxml/building.rs:363-406` (quick-xml
event, attribute and text-decoding API); every `hares-python` pyo3 call site
the 0.29 API changes.

**Change:**

- Raise every requirement in the table; `cargo update`.
- Declare `postcard` with `default-features = false`, keeping
  `features = ["alloc", "use-crc"]`: HARES calls only `to_allocvec`,
  `to_allocvec_crc`, `from_bytes` and `from_bytes_crc`, none of which needs
  the default `heapless-cas` feature that pulls `heapless` 0.7 and
  `atomic-polyfill`.
- Fix every compile error and every new warning or lint properly; suppress
  none.
- `.github/workflows/ci.yml` gains an `advisories` job running the
  `[advisories]` line (`taiki-e/install-action@cargo-deny`, as the file
  already installs nextest). No `deny.toml` is added: the check runs with
  cargo-deny's defaults, no license policy and no ignore list. A new advisory
  that appears later is fixed in the entry that surfaces it.

**Acceptance test:** `[advisories]` exits 0 with no ignores.
`cargo tree --workspace --all-features --target all -i paste` and
`-i atomic-polyfill` each exit 101 with `package ID specification ... did not
match any packages`. The whole block passes.

**Verification:** turns `[advisories]` green. `frame_golden materialize-all`
runs at the parent commit and `frame_golden delta` on every M-2 fixture
after the upgrade; any change (nalgebra backs the envelope and thermal
solvers; quick-xml parses HPXML; tzf-rs resolves site offsets) is recorded
per fixture and per column in the `02-measurement.md` log, then the
fixtures are re-captured in this commit. Parity and physics tests stay
green. The B0 check (`08-consumer.md` preamble) runs after this commit; if
it fails, or `consumer_shape_900s` changes, the upgrade of the crate that
moves the numbers is split out under the rule stated there.

**Commit:** `build: upgrade every Rust dependency to its latest release`

**Dependencies:** BR-6a, M-2.

## BR-6c: upgrade HARES's Python dependencies and maturin

**Kind:** initiative work (`dl-i08-architecture.dependencies`).

**Problem:** `uv.lock` pins older Python packages than the latest releases,
and `pyproject.toml`'s lower bounds and upper caps hold the lock there.

**Evidence:** `pyproject.toml` `[build-system] requires = ["maturin>=1.15.0,<2"]`,
`[project] dependencies`, `[project.optional-dependencies]`,
`[dependency-groups]` (`dev`, `ochre`: `pandas>=2.0,<3.0`, `pint>=0.24,<0.25`);
`uv.lock`. maturin 1.15.0 is the latest release on 2026-09-29.

**Change:** raise every lower bound in `pyproject.toml` to the latest release
(including maturin in `[build-system]` and both `dev` lists); remove the
upper caps; `uv lock --upgrade`. pandas moves to 3.0.6 and maturin keeps
1.15.0 without its `<2` cap. Two packages stay below their latest release,
each with a comment naming the reason: pytest-timeout at 2.4.0 (2.5.0 is
yanked upstream) and pint at `>=0.24.4,<0.25`, because pint 0.25 removed
`inch_H2O_39F` from its default registry and the vendored OCHRE
infiltration model converts with it at five sites
(`vendors/OCHRE/ochre/utils/envelope.py:551-623`; 0.24.4 converts,
0.25.2 and 0.26.1 raise `UndefinedUnitError` at OCHRE's `Dwelling`
construction). The pint cap is a suppression the dependency decision does
not allow; `11-other-defects.md` OD-14 registers the unit on the HARES side
and lifts it.

**Acceptance test:** `[python-build]` and `[pytest]` pass on the new lock.
Any change to the skip census (a newly installable optional package turns a
skip into a run) is written into `ledger/pytest-skip-census.txt` in the same
entry, with the reason in the `02-measurement.md` log. The census's three
OCHRE module skips hide the OCHRE comparison, so the packages that only the
OCHRE path exercises (pandas, pint, pvlib) are checked by running those
modules with OCHRE importable, recorded in the log
(`11-other-defects.md` OD-13 makes the block run them).

**Verification:** whole block.

**Commit:** `build: upgrade Python dependencies and maturin to their latest releases`

**Dependencies:** BR-6b (the extension builds against pyo3 0.29).

## BR-6d: upgrade the consumer's Python dependencies

**Kind:** initiative work (`dl-i08-architecture.dependencies`), in
`~/src/HARES_CALIBRATION_SYSTEM` on `i08-consumer`.

**Problem:** the consumer's `uv.lock` pins older packages than the latest
releases.

**Evidence:** consumer `pyproject.toml` (`numpy>=2.1`, `scipy>=1.14`,
`scikit-learn>=1.5`, `polars>=1.44`; extras `gridlabd`, `docs`; group `dev`
`ruff>=0.9`); `uv.lock`.

**Change:** raise every lower bound to the latest release; `uv lock --upgrade`
(the engine stays the path source
`ochre-next = { path = "HARES", editable = true }`).

**Acceptance test:** the consumer gate (`08-consumer.md`) passes against the
BR-6c engine, and the B0 check (`08-consumer.md` preamble) passes: the DE
trace on the upgraded lock equals B0 bitwise. If a numpy or scipy upgrade
moves the trace, that package's upgrade is split out under the rule stated
in `08-consumer.md` and the rest lands here.

The gate and capture of this entry's commit (consumer `5c651bc`) ran
against an editable install of the main engine tree
(`/home/rich/src/HARES`), not a worktree under `.workstrees/`, and no gate
record exists for it in `02-measurement.md`; they do not count as this
acceptance. `08-consumer.md` CN-17 establishes it: its gate and its
DE-trace gate against B0 run through the engine-under-test mechanism on the
consumer that carries this entry's lock. If that trace differs from B0, the
same consumer is captured against a worktree of the B0 engine `1d60f478`:
a difference there is this entry's lock, and the split rule above applies;
equality there puts the cause in an engine row, found by the row 16g
re-check (`00-schedule.md`): the committed rows' worktrees in order, with a
difference in the cumulative `8b09ac4a` capture bisected over the commits
of rows 1 to 12 before a row is named.

**Verification:** consumer gate; B0 check; HARES block unchanged.

**Commit:** `build: upgrade Python dependencies to their latest releases`
(consumer repository, with the `Engine:` trailer of `08-consumer.md`).

**Dependencies:** BR-6c; `08-consumer.md` CN-0 and B0; operator checkpoint
for any added dependency (`plan.md` §3; this entry adds none).

## BR-10: CI never runs on this initiative's branches and misses two block lines

**Kind:** fix (follow-up to BR-4, committed at `363b4607`).

**Problem:** BR-4 made CI run the block's cargo lines so that a regression
cannot reach `initial-implementation` unseen, but the workflow only
triggers for `main`, which holds only the initial commit. No push to
`initial-implementation` and no pull request into it (the closing
sequence's pull request included) starts a run, so none of BR-4's, BR-6b's
or BR-7's CI steps ever executes. The `test` job also still lacks
`[test-observe-profiling]` and both `[golden]` lines, so BR-4's title
("CI runs the block's cargo lines") holds only in part. Once CI runs on a
pull request into `initial-implementation`, the `bench` job fails: it
checks out `main` and benches targets that `main` does not contain. The
`coverage` job fails on any pull request from a branch of this repository:
it uploads to Codecov with `fail_ci_if_error: true` and no token, and
Codecov refuses tokenless uploads to a protected branch. The `test` job
uploads its JUnit report from a path nextest never writes.

**Evidence** (at `afa9a071`; `ci.yml` and `.config/nextest.toml` are
unchanged from `872cf289` through `afa9a071`). This entry's parent is
the latest engine commit when it starts (at least `7b7a6a7d`). OR-11's
commit `b8195c83` deleted the release `check_invariants` run at
`ci.yml:155` with the `check_invariants` feature; the implementer re-reads
the lines below at the parent commit before starting, and the Change
applies to the nextest runs present there.

- `.github/workflows/ci.yml:3-8`: `push: branches: [main]`,
  `pull_request: branches: [main]`, `workflow_dispatch`.
- `ci.yml:150-156`: the `test` job's nextest runs (default, `dst`,
  `profiling`, `observe`, plain release, release `check_invariants`,
  release `hares-equipment`); no `-p hares-core -F observe,profiling` run
  and no `frame_golden compare-all`.
- `ci.yml:271`, `:281-283`: the `bench` job reads the `main` SHA, runs
  `git checkout main`, and runs `cargo bench --bench single_building ...`
  there; `git ls-tree origin/main` lists only `.gitignore`, `LICENSE` and
  `README.md`.
- PRs #24, #25 and #27 (base `initial-implementation`) have no CI run after
  their base was set.
- `ci.yml:249-255`: `codecov/codecov-action@v4` with `files: lcov.info`
  and `fail_ci_if_error: true`, no `token`. The last CI run on PR #27
  (run 36028888222) failed its `coverage` job with `Token required because
  branch is protected`; the repository has no Actions secret
  (`gh api repos/:owner/:repo/actions/secrets`: `total_count` 0).
- `ci.yml:134`, `:141`: the `test` job runs on `ubuntu-latest` with
  `dtolnay/rust-toolchain@stable`, two labels that move: the Ubuntu release
  (and with it the glibc `libm` whose transcendental results the physics
  uses) and the rustc release. The goldens are per-column bit digests
  captured on Ubuntu 24.04 (glibc 2.39) with rustc 1.94.1, and bit-exact
  agreement across `libm` or LLVM versions is not guaranteed
  (`02-measurement.md` M-4a pins `ubuntu-24.04` for that reason).
- `ci.yml:167-171`: the `test` job uploads `nextest-results.xml` from the
  repository root, and both stray listings exclude that root path
  (`:149`, `:162`). nextest writes `junit.path` relative to
  `target/nextest/<profile>/` (`.config/nextest.toml` `[profile.ci]`), and
  run 36028888222 logged "No files were found with the provided path".
  Every `--profile ci` run writes the same
  `target/nextest/ci/nextest-results.xml`, so each run overwrites the
  report of the run before it.

**Change:**

- `on:` lists `[main, initial-implementation]` under both `push` and
  `pull_request`; `workflow_dispatch` stays. No feature-branch name is
  listed: the closing sequence's pull request into `initial-implementation`
  is what runs CI on this initiative's work.
- The `test` job gains, between the existing nextest runs and the stray
  check, `cargo nextest run -p hares-core --no-fail-fast --profile ci -F
  observe,profiling`. Each nextest step is followed by a step with
  `if: success() || failure()` that copies
  `target/nextest/ci/nextest-results.xml` to
  `target/nextest/ci/junit-<run>.xml`, `<run>` naming the step's package
  selection, feature set and profile (for example `default`, `dst`,
  `release`, `equipment-release`, `core-observe-profiling`), distinct for
  every step, so every run keeps its own report. The artifact upload reads `target/nextest/ci/junit-*.xml`, and
  the two stray listings drop the root `nextest-results.xml` exclusion.
- A new `golden` job runs the two `[golden]` lines
  (`cargo run --release -p frame_golden -- compare-all`, and the same with
  `-F dst`) on `runs-on: ubuntu-24.04` with
  the toolchain `rust-toolchain.toml` pins (BR-20, row 16a1), the
  platform and toolchain the committed goldens are captured or compared
  with (recorded in the `02-measurement.md` log). Moving that pin is a numeric event: the commit that moves it
  first runs `compare-all` locally on the new toolchain and, on any
  difference, follows M-1's numeric entry procedure and re-captures in the
  same commit.
- The `coverage` job's Codecov upload was removed at the operator's
  direction (2026-10-05, `f3abc65d`): the service is not used and no
  secret is required. `cargo llvm-cov` stays as a job that must pass and
  its lcov report uploads as a workflow artifact.
- The `bench` job takes its baseline commit from the pull request's base
  branch (`github.base_ref`) on a pull request, and from the commit before
  the push (`github.event.before`) on a push, in place of the hard-coded
  `main` at `:271` and `:281-283`; the cache key uses that commit's SHA. A
  push that creates a branch has the all-zero `before` and no baseline: the
  job takes the existing no-baseline path, which states in the step summary
  that no comparison ran.

**Acceptance test:** each new `test` and `golden` job command, run locally
exactly as written (with `--profile ci` for nextest), exits 0, and the
local nextest runs, each followed by its copy step, leave one
`target/nextest/ci/junit-<run>.xml` per run;
`ci.yml`'s `on:` lists `initial-implementation` under `push` and under
`pull_request`, no step names `main` as a fixed ref, and the Codecov step
names the secret. The CI run itself is checked at the closing sequence's
first push (`00-schedule.md` row 99a): every job, the `coverage` job
included, starts and passes, and the run URL is recorded in the
`02-measurement.md` log. A job that fails there becomes a fix entry
committed before M-4b, and the operator pushes again.

**Verification:** whole block; no block line changes state.

**Commit:** `ci: run on initial-implementation and its pull requests, and add the observe-profiling and golden lines`

**Dependencies:** BR-4, M-2, BR-6b, `03-output-reporting.md` OR-11 (its
parent). Runs after `08-consumer.md` CN-17 (`00-schedule.md` row 16a).

## BR-11: golden manifests with `initialization_duration_s = 0` run a warm-up

**Kind:** fix (follow-up to M-1 and M-2, committed at `979b8773` and
`7643b4e7`).

**Problem:** the golden tool turns every manifest's
`initialization_duration_s` into `Some(Duration::from_secs(n))`, and the
engine runs its converging warm-up for any `Some`, ignoring the value. So
`initialization_duration_s = 0`, which every committed manifest sets, runs a
warm-up. `consumer_shape_900s` exists to mirror the consumer's call, which
passes no `initialization_duration` through the Python binding and so runs
no warm-up; its golden records a two-day warm-up instead, and M-3 would
measure that shape.

**Evidence** (at `872cf289`):

- `tools/frame_golden/src/adapter.rs:174` and `:274-276`:
  `initialization_duration: Some(StdDuration::from_secs(home.initialization_duration_s))`.
- `crates/hares-core/src/dwelling/mod.rs:2753`:
  `if let Some(_init_dur) = config.initialization_duration` runs
  `run_warmup_converged(0.5, 25)`.
- `crates/hares-python/src/py_dwelling.rs:2473-2479`: the binding maps `0`
  to `None` (no warm-up).
- `tests/fixtures/golden/consumer_shape_900s.golden.json`, `health.warmup`:
  `Ran { converged: true, days_run: 2, ... }`; every other dwelling golden
  records the same kind of `Ran` outcome.
- All six manifests under `tests/fixtures/golden/` set
  `initialization_duration_s = 0`.

**Change:**

- The manifest key keeps its name and type; the adapter maps `0` to
  `initialization_duration: None` (no warm-up, as the Python binding does)
  and a positive value to `Some`. The manifest module's doc comment states
  the rule.
- `full_year_hourly`, `full_year_900s_dst`, `leap_year_february_900s`,
  `weather_wrap_900s` and the three `fleet_mixed_offset` homes set
  `initialization_duration_s = 604800` (the seven days
  `Dwelling::from_hpxml_with_write_output` uses, `mod.rs:1855`). The engine
  ignores the value today, so their
  goldens are unchanged; they keep a warm-up so that
  `09-numeric-fixes.md` B-2 has fixtures on which to record its delta.
- `consumer_shape_900s` keeps `0` and is re-captured by M-1's numeric entry
  procedure (`materialize` at the parent commit, `delta`, `capture`); the
  delta (a fixture correction, not an engine change) is recorded in the
  `02-measurement.md` log.

**Acceptance test:** in `tools/frame_golden/tests/`:

- `manifest_zero_initialization_duration_runs_no_warmup`: a one-day
  manifest of the cz2a parity inputs with `initialization_duration_s = 0`
  records `health.warmup` as `WarmupOutcome::Disabled`;
- `manifest_positive_initialization_duration_runs_warmup`: the same manifest
  with `86400` records `WarmupOutcome::Ran`.

`compare-all` passes in both feature sets after the re-capture, with only
`consumer_shape_900s.golden.json` changed among the committed goldens.

**Verification:** whole block; the B0 check (`08-consumer.md`) is
unaffected (no engine change).

**Commit:** `fix: a golden manifest with no initialization duration runs no warm-up`

**Dependencies:** M-2, BR-6d. Runs directly before M-3, which measures
`consumer_shape_900s`.

## BR-12: seven test files under `crates/hares-core/tests/` are never compiled

**Kind:** fix (follow-up to BR-8, committed at `b674a767`).

**Problem:** `crates/hares-core/Cargo.toml` declares explicit `[[test]]`
targets whose paths point at the repository-level `tests/` directory under
the same names as seven files in `crates/hares-core/tests/`. Cargo's
auto-discovery skips a file whose target name is already declared, so
those seven files compile into no target. Six are two-line `#[path]` shims
that duplicate the explicit targets. The seventh,
`crates/hares-core/tests/resstock_smoke.rs`, holds four seasonal 72-hour
tests that exist nowhere else and have never run; BR-8 edited this file
(its ignored test) without anything compiling it. The eleven tests BR-8
deleted were also deleted without a recorded run showing that none of them
failed.

**Evidence** (at `872cf289`):

- `crates/hares-core/Cargo.toml:50-84`: `[[test]]` targets `regression`,
  `parity`, `bestest`, `conditioned_oracle`, `freefloat_oracle`,
  `structural_envelope_oracle`, `warmup_regression`, `resstock_smoke`,
  `batch_step_tests`, `envelope_opaque_loads`, each with a
  `../../tests/...` path.
- `cargo metadata --format-version 1 --no-deps` lists no test target whose
  `src_path` is `crates/hares-core/tests/parity.rs`, `bestest.rs`,
  `conditioned_oracle.rs`, `freefloat_oracle.rs`,
  `structural_envelope_oracle.rs`, `warmup_regression.rs` or
  `resstock_smoke.rs`; every other `crates/*/tests/*.rs` and
  `tools/*/tests/*.rs` file is a target.
- `crates/hares-core/tests/parity.rs:1-2` (`#[path = "../../../tests/parity/mod.rs"] mod parity;`),
  and the same shape in the other five shims.
- `crates/hares-core/tests/resstock_smoke.rs:259-286` (`summer_72h`,
  `winter_72h`) and `:294-315` (`resstock_2025_1_summer_72h`,
  `resstock_2025_1_winter_72h`, `resstock_2024_2_summer_72h`,
  `resstock_2024_2_winter_72h`); its `smoke_1h` duplicates the compiled
  `tests/resstock_smoke.rs` smoke tests.
- `02-measurement.md`'s log holds no `--run-ignored only` result from before
  BR-8's deletion.

**Change:**

- Delete the six shims.
- Move the four seasonal tests into the compiled `tests/resstock_smoke.rs`
  with their inputs unchanged (2018-07-15 for `bldg0000004`, 2018-01-15 for
  `bldg0000002`, 72 h at 3600 s, start offset `-07:00`, one day of warm-up,
  each release), reusing that file's helpers (no second copy of
  `project_root`, `weather_path` or the temp-file guard). Their assertions
  are what each test's doc comment claims. The end-use energies are read
  from the run's `SimulationResults.metrics.total_energy_kwh.per_end_use`
  (`SimulationEngine::run`, the runner the compiled file uses): summer,
  `hvac_cooling` is above zero; winter, `hvac_heating` is above zero and
  the run's gas energy is above zero (`bldg0000002` heats with gas, a
  boiler in 2025.1 and a furnace in 2024.2, so `hvac_heating`, an electric
  end use, measures only the boiler auxiliary or fan electricity). The
  engine's `SimulationMetrics` carries no gas total, so the gas energy is
  the sum of the output file's `Total Gas Power (therms/hour)` column, read
  with the file's `parse_csv_columns` helper, times the 1 h step. Both
  seasons also run the existing `assert_physics_bounds` (finite values,
  zone temperatures inside -50 C to 80 C). The shadowed file's arbitrary
  bands (peak below 20 kW, energy between 5 and 400 or 600 kWh) assert no
  physical result and are not carried over. Output goes to a temporary
  CSV under the file's temp-file guard, as the compiled file's runner
  writes it. The 2024.2 winter case also books 3.45 kWh to
  `hvac_cooling` over its 72 January hours; `11-other-defects.md` OD-15
  triages that, and this entry asserts nothing about it.
- Delete `crates/hares-core/tests/resstock_smoke.rs`.
- In an archive copy of `b674a767^` under `target/` (`git archive`, with
  the `vendors/OCHRE` submodule contents copied in; no worktree is created),
  run `cargo nextest run --workspace --run-ignored only --no-fail-fast`
  (default features), then once more per feature that a `cfg` on an
  ignored test or on its enclosing module names, with that feature alone
  (`-F <feature>`), and record each then-ignored test's result per feature
  set in the `02-measurement.md` log. A test that failed there is a defect
  report, and its cause is found.
- One of the eleven, `debug_bldg0000002_winter_heating`
  (`#[cfg(feature = "observe")]`, `crates/hares-core/tests/resstock_smoke.rs:299`
  at `b674a767^`), lives in the shadowed file, which no target compiles, so
  neither run lists it. In the archive copy only, a temporary
  `[[test]] name = "resstock_smoke_shadow"`, `path =
  "tests/resstock_smoke.rs"` is added to `crates/hares-core/Cargo.toml`
  and that target is run with `-F observe --run-ignored only`. Its result,
  or its compile error, is the recorded result for that test: a compile
  error records that the test never built.
- A defect found by either run (the seasonal tests or the ignored-test run)
  whose cause lives in code that still exists at this entry's parent is
  handled by what its fix does to the numbers:
  - a fix that is bitwise-neutral (`[golden]` unchanged and the B0 check
    equal) is made in this entry and pinned by a test;
  - a fix that moves numbers becomes a new `09-numeric-fixes.md` entry in
    the numeric block (after `00-schedule.md` row 78a, with its own row,
    commit and recorded delta). The test that shows the defect lands in
    this entry asserting today's value, with a doc comment stating the
    physical expectation it violates; the new entry inverts the assertion
    in its own commit. The test is not ignored and not loosened.

**Acceptance test:**

- `cargo nextest run -p hares-core --test resstock_smoke` runs the four
  seasonal tests and they pass (a test that pins a number-moving defect
  passes by asserting today's value, as stated above).
- Every test source file compiles into a target:
  every `crates/*/tests/*.rs` and `tools/*/tests/*.rs` file is the
  `src_path` of a test target, and every repository-level `tests/*.rs` and
  `tests/*/mod.rs` file is the `src_path` of a test target or the
  `#[path]` of a file that is one. The census prints nothing:

  ```
  targets=$(cargo metadata --format-version 1 --no-deps | jq -r '.packages[].targets[] | select(.kind[] == "test") | .src_path' | xargs realpath -m | sort -u)
  shims=$(for t in $targets; do sed -nE 's/^#\[path = "([^"]+)"\]/\1/p' "$t" | while read -r p; do realpath -m "$(dirname "$t")/$p"; done; done | sort -u)
  comm -13 <(printf '%s\n' $targets $shims | sort -u) <(realpath crates/*/tests/*.rs tools/*/tests/*.rs tests/*.rs tests/*/mod.rs | sort -u)
  ```

  At `21d90588` it prints exactly the seven files this entry deletes.
- The ignored-test run at `b674a767^` is in the log with a result per test
  and feature set; `debug_bldg0000002_winter_heating` carries the result
  of the `resstock_smoke_shadow` target under `observe`, a compile error
  included.

**Verification:** whole block. Every test line the block has at the parent
commit (OR-11 deleted `[test-release-invariants]`) runs four more tests
than at the parent, plus one per defect pinned in this entry.

**Commit:** `test: compile the seasonal ResStock tests and delete test files no target builds`

**Dependencies:** BR-8. Runs after `08-consumer.md` CN-17 and BR-10
(`00-schedule.md` row 16c).

## BR-13: `compare` ignores the recorded defaults digest

**Kind:** fix (follow-up to M-1, committed at `979b8773`).

**Problem:** each golden records a SHA-256 over the defaults tree its run
read, so that a golden captured against another defaults tree is
detectable. `compare` never reads it: a defaults edit that leaves every
column of a fixture unchanged passes silently, and a defaults edit that
changes columns is reported only as column differences, with nothing
pointing at the defaults.

**Evidence** (at `872cf289`): `tools/frame_golden/src/golden.rs:47-48`
(`defaults_dir`, `defaults_digest`), written at `capture.rs:43-44`;
`defaults.rs:71-82` (`digest_tree`: sorted relative path, `0x00`, bytes);
`compare.rs:172-200` (`compare_products` takes frames, metrics and health,
not the defaults digest); no other reader of `defaults_digest` in
`tools/frame_golden/src/`.

**Change:** `compare` (and therefore `compare-all`) computes the defaults
digest of the tree the run used and compares it with the golden's
`defaults_digest`. The `defaults_dir` string is reported, not compared: the
digest already covers the content, and the same tree may be named
differently. A digest mismatch is a difference: it names the fixture, the
defaults directory and both digests, and the run exits non-zero.
`materialize` (`capture.rs:110-145`, which today checks products, metrics
and health) refuses a defaults-digest mismatch with the same message before
it writes any full product. An entry that edits any file under `defaults/`
therefore re-captures every fixture that reads it, in its own commit, with
the delta recorded (M-1's numeric entry procedure). `delta` reports a
defaults-digest change in its output without failing.

**Acceptance test:** in `tools/frame_golden/tests/compare.rs`:

- `compare_fails_on_defaults_digest_mismatch`: a one-day fixture captured
  into a temporary golden directory, then compared after one byte of a
  `[defaults_files]` replacement changes, reports the defaults difference
  with both digests and exits non-zero; unchanged, it passes. The
  replacement source is an absolute path in a `tempfile` directory (a copy
  of the `defaults/` file it replaces, then changed by one byte), which
  `repo_path` returns as given (`Path::join` with an absolute path), so the
  test writes nothing into the tree and edits no committed file.
- `materialize_refuses_defaults_digest_mismatch`: the same changed
  replacement makes `materialize` fail with that message and write no full
  product.

In `tools/frame_golden/tests/digest.rs`,
`zero_row_product_reads_back_as_its_schema` pins the read fixed at row
10a (BR-18), which no test covered: `write_frame` of one zero-row batch of
a two-column schema into a `tempfile` path, then `read_frame`, returns that
schema and one zero-row batch, and `digest_frame` of the result equals
`digest_frame` of the written batch.

`compare-all` passes on every committed fixture.

**Verification:** whole block.

**Commit:** `fix: golden comparison fails when the defaults tree differs from the capture`

**Dependencies:** M-1, M-2, BR-18. Runs after BR-12 (`00-schedule.md` row 16d).

## BR-14: no golden carries a billing product

**Kind:** fix (follow-up to M-1 and M-2, committed at `979b8773` and
`7643b4e7`).

**Problem:** M-1 defines a `billing` product "when a tariff is configured",
but a manifest cannot configure one, so no fixture produces billing.
`09-numeric-fixes.md` B-4 depends on a golden with billing summaries
spanning a month boundary and records its delta on it.

**Evidence:** `tools/frame_golden/src/adapter.rs:216-223`
(the billing product appears only when `billing_summaries()` is non-empty,
and the comment states that no manifest reaches `Dwelling::set_tariff`);
`crates/hares-core/src/dwelling/mod.rs:4475` (`set_tariff(&mut self,
tariff: ElectricTariff, tz: Tz)`, the only way to attach a tariff);
`tools/frame_golden/src/manifest.rs:53-67` (no tariff key);
`crates/hares-tariff/src/types.rs:263-306` (`ElectricTariff` derives
`Deserialize`; `validate()` at `:340`); no type in `types.rs` denies
unknown fields, so a misspelled key in a tariff file is dropped silently,
against `dl-i08-architecture.config` (unknown keys are errors), and the
Python `Tariff` JSON constructors (`crates/hares-python/src/py_tariff.rs:141`,
`:154`) inherit the same gap. The types a tariff's periods are made of
live in `crates/hares-types/src/schedule.rs` (at `afa9a071`): `TouPeriod`
(`:1146`, the elements of `tou_schedule` and `demand_tou_schedule`),
`TimeWindow` (`:56`, a period's `schedule`), `DistributionKind` (`:175`,
a window's `noise`) and `SeasonalSplit` (`:932`); none denies unknown
fields, so an attribute on the `types.rs` types alone still accepts an
extra key inside a period or its window. `tools/frame_golden/Cargo.toml`
has no `chrono-tz` dependency.

**Change:**

- Every `Deserialize` type in `crates/hares-tariff/src/types.rs`, and
  `TouPeriod`, `TimeWindow`, `DistributionKind` and `SeasonalSplit` in
  `crates/hares-types/src/schedule.rs`, gain `#[serde(deny_unknown_fields)]`
  (none flattens a field and none is internally tagged, so the attribute
  applies as written), so every reader of a tariff file, the Python
  constructors included, fails naming the unknown key wherever it sits.
  The `hares-types` types have other readers: equipment schedule
  configuration that embeds `TimeWindow` and `DistributionKind`, and the
  actor configurations that embed `TimeWindow` (the EV driver's among
  them). Every committed input any of these readers parses (tariff JSON,
  equipment and actor TOML and JSON, test strings) is checked for extra
  keys, and each one found is corrected in this commit.
- `tools/frame_golden/Cargo.toml` gains `chrono-tz = { workspace = true }`.
- The manifest gains an optional `[tariff]` table, accepted for
  `kind = "dwelling"` only, with unknown keys rejected: `file`, a
  repository-relative JSON file deserialized into
  `hares_tariff::ElectricTariff` and checked with `validate()`, and `zone`,
  an IANA zone name parsed as `chrono_tz::Tz`. A file that cannot be read
  or parsed (an unknown key included), a failed validation or an unknown
  zone is an error naming the manifest, the field and the cause. The
  checks split by what each step can see: `GoldenManifest::parse(path,
  text)` has no repository root, so it checks the table's shape (`file`
  and `zone` present, no other key, `kind = "dwelling"`) and parses
  `zone`. One function, `GoldenManifest::tariff(&self, root: &Path)`,
  reads `file` from `repo_path(root, file)`, deserializes it strictly into
  `ElectricTariff`, runs `validate()` and returns the tariff with its
  zone; `GoldenManifest::load` calls it with `repo_root(path)` so a bad
  tariff fails when the manifest loads, and the adapter calls it with the
  run's repository root and passes the result to `set_tariff` after
  construction and before the run.
- New fixture `month_boundary_billing_3600s`: the cz2a parity inputs
  (`tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh/`), start
  `2023-01-25T00:00:00-07:00`, 12 d at 3600 s (288 rows), features `[]`,
  `initialization_duration_s = 604800`, `master_seed = 0`,
  `output_verbosity = 2`, and `[tariff]` with
  `file = "tests/fixtures/golden/flat_tariff.json"` (one all-hours period,
  one energy rate of 0.15 USD/kWh for every season, a 10.00 USD monthly
  fixed charge, monthly billing cycle) and `zone = "America/Denver"`
  (standard time throughout the run, so the zone's month boundary is a
  step boundary of the `-07:00` clock). The golden carries the frame, the
  `billing` product and health.

**Acceptance test:**

- In `tools/frame_golden/tests/manifest.rs`:
  `manifest_tariff_rejected_for_fleet` and
  `manifest_tariff_rejects_unknown_zone` call `GoldenManifest::parse`;
  `manifest_tariff_rejects_invalid_file` (a tariff file with a misspelled
  key, and one that is not JSON, each fail naming the manifest, `file` and
  the cause) and `manifest_tariff_rejects_failed_validation` (the flat
  tariff with `demand_window_minutes = 2`, outside `validate()`'s 5 to 60,
  fails naming `validate()`'s error) call `GoldenManifest::load` on a
  manifest written into a `tempfile` directory whose `file` is the
  absolute path of a tariff file written beside it, which `repo_path`
  returns as given, so no test file lands in the tree.
- `tariff_json_rejects_unknown_key` in `crates/hares-tariff/src/types.rs`'s
  tests: `flat_tariff.json` with one extra key placed, in three separate
  cases, at the top level, inside its `tou_schedule` period and inside
  that period's `schedule` window; each fails naming the key.
- The `month_boundary_billing_3600s` golden lists a `billing` product with
  one row per billing period the run touches (January and February, as
  today's evaluator assigns the boundary step; `09-numeric-fixes.md` B-4
  changes and records that attribution), and `compare-all` passes in both
  feature sets.

**Verification:** whole block; record the new golden's size in the
`02-measurement.md` log.

**Commit:** `fix: read tariff files strictly and give golden manifests a tariff and a month-boundary billing fixture`

**Dependencies:** M-1, M-2, BR-11 (the manifest warm-up rule). Runs after
BR-13 (`00-schedule.md` row 16e); precedes `09-numeric-fixes.md` B-4 and
`03-output-reporting.md` OR-10.

## BR-15: the fleet manifest leaves home weights and aggregation resolution implicit

**Kind:** fix (follow-up to M-1 and M-2, committed at `979b8773` and
`7643b4e7`).

**Problem:** a fleet manifest cannot state a home's weight or the
aggregation resolution. The adapter takes the weight `Fleet::from_buildings`
assigns and derives the resolution from the step (hourly when the step is
an hour or longer, 15-minute otherwise), so what a fleet golden pins
depends on rules the manifest does not show, against the manifest's own
rule that nothing a golden pins is implicit.

**Evidence** (at `872cf289`): `tools/frame_golden/src/manifest.rs:40-51`
(`HomeEntry` has no weight) and `:53-67` (no resolution key);
`adapter.rs:282` (`Fleet::from_buildings(configs)`, no
`with_sample_weights`) and `:304-310` (the resolution rule);
`crates/hares-fleet/src/fleet.rs:263` (`with_sample_weights`);
`crates/hares-fleet/src/aggregation.rs` (`AggregationResolution::{Hourly,
FifteenMin}`).

**Change:**

- `[[home]]` gains `weight` (a finite, non-negative float), required for
  `kind = "fleet"` and rejected for `kind = "dwelling"`. A top-level
  `resolution = "hourly" | "fifteen_min"` is required for `kind = "fleet"`
  and rejected for `kind = "dwelling"`. The adapter passes the weights
  through `Fleet::with_sample_weights` and the resolution to `aggregate`;
  the step-derived rule is deleted.
- `fleet_mixed_offset.toml` gains `weight = 1.0` on each home and
  `resolution = "hourly"`, which is what the adapter used; its golden is
  unchanged.
- `04-fleet.md` FL-1 migrates `resolution` to the fleet report's frequency
  when it lands.

**Acceptance test:** in `tools/frame_golden/tests/manifest.rs`,
`manifest_fleet_requires_weight_and_resolution` and
`manifest_dwelling_rejects_fleet_keys`; in
`tools/frame_golden/tests/golden.rs`,
`fleet_manifest_weights_reach_the_weights_product` (two `bldg0000002`
homes from `tests/fixtures/resstock/2025.1/` with weather
`tests/fixtures/resstock/2025.1/weather/G0900090_2018.csv`, start
`2018-01-01T00:00:00-05:00` (the site offset of `G0900090`), one day at
3600 s, `initialization_duration_s = 0`, weights 1.0 and 3.0: the `weights`
product reads 1.0 and 3.0 in home order).
`compare-all` passes with `fleet_mixed_offset.golden.json` unchanged.

**Verification:** whole block.

**Commit:** `fix: fleet golden manifests state each home's weight and the aggregation resolution`

**Dependencies:** M-1, M-2. Runs after BR-14 (`00-schedule.md` row 16f).

## BR-16: the EV's daily `soh` binding breaks the plain release build under `-D warnings`

**Kind:** fix (found defect, `dl-i08-process.found_defects`; committed
directly after M-1). Executed: `00-schedule.md` row 8a.

**Problem:** the EV's daily update binds `soh` only for the
battery-capacity underflow check that follows it, and that check is
compiled only under `debug_assertions` or `check_invariants`. In a plain
release build the binding is unused, so the build warns, and CI's `test`
job, which sets `RUSTFLAGS: "-D warnings"` and runs the plain release
nextest line, fails to compile.

**Evidence** (at `979b8773`): `crates/hares-equipment/src/ev/mod.rs:1404`
(`let soh = 1.0 - self.degradation.capacity_fade_fraction();`, no `cfg`)
and the `#[cfg(any(debug_assertions, feature = "check_invariants"))]`
block after it; `.github/workflows/ci.yml:128` (the `test` job's
`RUSTFLAGS: "-D warnings"`) and `:146` (its plain release run).

**Change:** the binding carries the check's `cfg`. Nothing else changes; no
build computes a different value.

**Acceptance test:** `RUSTFLAGS="-D warnings" cargo build --release -p
hares-equipment` compiles. `03-output-reporting.md` OR-11 (row 19) makes
the check unconditional (its census row `ev/mod.rs:1404-1415`), after
which the binding is read in every build and carries no `cfg`.

**Verification:** whole block as green at that row; `[golden]` unchanged.

**Commit:** `fix: gate the EV daily soh binding to the builds that read it`

**Landed:** `4a450949`.

**Dependencies:** OR-5 (row 4).

## BR-17: the golden round-trip tests reject their own manifest in a `dst` build

**Kind:** fix (found defect, `dl-i08-process.found_defects`; committed
directly after M-2). Executed: `00-schedule.md` row 9a.

**Problem:** `golden_json_is_byte_stable` and
`capture_then_compare_round_trips` build their one-day manifest with
`features = []`. The golden tool refuses a manifest whose feature set
differs from the running binary's, so under `[test-dst]` (`-F dst`) both
tests fail on their own manifest. The tests check capture stability and
the round trip, not the feature gate, which has its own tests.

**Evidence** (at `7643b4e7`): `tools/frame_golden/tests/golden.rs:17-20`
(`one_day_manifest`, `features = []`), `:59` and `:80` (the two tests).

**Change:** the manifest names `frame_golden::manifest::running_features()`,
and `golden_json_is_byte_stable` asserts the captured document's feature
set equals it.

**Acceptance test:** both tests pass under `[test-debug]` and
`[test-dst]`.

**Verification:** whole block as green at that row; `[golden]` unchanged.

**Commit:** `test: the golden round-trip tests name the running feature set`

**Landed:** `59b060d6`.

**Dependencies:** M-1, M-2.

## BR-18: a zero-row golden product cannot be read back

**Kind:** fix (found defect, `dl-i08-process.found_defects`; found while
BR-6b ran its delta procedure, and reproduced at that row's parent).
Executed: `00-schedule.md` row 10a.

**Problem:** a frame product with no rows (the `fleet_mixed_offset`
aggregate, pinned at 0 rows by M-2) is written as a Parquet file with a
schema and no row groups. `read_frame` collects one batch per row group,
so it returns no batch, and `delta` then fails with "cannot digest an
empty batch list", although the product has a schema and a committed
digest.

**Evidence** (at `39488581`): `tools/frame_golden/src/frames.rs:41-51`
(`read_frame` collects the reader's batches and returns them as read);
`tools/frame_golden/src/digest.rs:422-425` (`digest_frame` refuses an
empty batch list); `02-measurement.md`, BR-6b record.

**Change:** `read_frame` returns one empty batch carrying the file's
schema when the reader yields none. The digest of that batch equals the
committed zero-row digest, so no golden changes.

**Acceptance test:** the commit added none; `delta` on every M-2 fixture
ran clean at BR-6b (`02-measurement.md`). The test that pins it,
`zero_row_product_reads_back_as_its_schema`, lands with BR-13 (row 16d).

**Verification:** whole block as green at that row; `[golden]` unchanged.

**Commit:** `fix: read zero-row golden products as a schema-only batch`

**Landed:** `f880a6d7`.

**Dependencies:** M-1, M-2.

## BR-19: a DE-trace capture can name an engine other than the one that ran

**Kind:** fix (found defect, `dl-i08-process.found_defects`; consumer
repository; found while B0 was captured). Executed: `00-schedule.md` row
0e, before B0.

**Problem:** `capture_de_trace.py capture` writes the engine commit of
`paths.HARES_REPO` into its header, while the engine that runs is the one
installed in the consumer's venv. Without `$HARES_REPO`, `paths.HARES_REPO`
is the pinned `./HARES` submodule, so a capture's header could name a tree
the running engine was not built from. The first two B0 captures did
(`02-measurement.md`, B0 record).

**Evidence** (at `a6d1345`): `tools/capture_de_trace.py:174`
(`cmd_capture` writes the header with no check of the installed engine);
`calibration/paths.py:24-41` (`HARES_REPO`: `$HARES_REPO`, else the
submodule).

**Change:** `cmd_capture` reads `paths.summary()` and refuses (exit 2,
naming both trees) when the installed engine's `engine_so` does not lie
under `paths.HARES_REPO`. A first commit required `$HARES_REPO` itself;
the second replaced that with the provenance check, which states the
invariant directly, and gave the existing capture case's summary stub the
real `engine_so` shape.

**Acceptance test:** `suite_scale`'s capture case passes with the stub. No
case drives the refusal: `suite_scale.capture_refuses_an_engine_built_outside_the_resolved_tree`
(an `engine_so` outside `paths.HARES_REPO` makes `capture` exit 2 and
write no file) lands with `08-consumer.md` CN-17 (row 16a), the next
consumer commit.

**Verification:** consumer gate as it ran at that row.

**Commit:** `fix: a capture requires HARES_REPO so its header names the engine under test`,
then `fix: a capture refuses an installed engine from outside the recorded tree`
(each with the trailer `Engine: 1d60f47879e9719b9e63cfc73bb3a1e621e8efe5`).

**Landed:** `f7131d7`, `aec2e3f`.

**Dependencies:** `08-consumer.md` CN-0.

## BR-21: every build the block runs is linted, and the physics gate runs from a fresh checkout

**Kind:** found defect (independent audit of RT-3 and RT-3a,
`verification/audit-RT-3-RT-3a-69950a98.md`).

**Problem** (at `69950a98`):
- `[clippy]` (`plan.md` §3) lints one configuration, the dev profile
  with `--all-features`. Code compiled out by `debug_assertions` or by a
  feature is never linted in the configurations the block tests and
  ships. `cargo clippy --workspace --release --all-targets -- -D warnings`
  fails: `BoundaryDiagnosticInfo` is imported at
  `crates/hares-envelope/src/thermal_solver/mod.rs:1858` and used only
  under `#[cfg(any(debug_assertions, feature = "observe_detailed"))]`
  (present since before `7b7a6a7d`).
- `make check-ashrae-reference` (`scripts/check_ashrae_reference.sh:35`)
  runs `uv run python`, which syncs the default groups; `xmltodict` is in
  the `ochre` extra (`pyproject.toml:30-32`), so on a fresh checkout the
  physics gate fails until the operator knows to run
  `uv sync --extra ochre` (`Makefile:72`).

**Change:**
1. A clippy census: `cargo clippy --workspace --all-targets -- -D
   warnings` under every profile and feature set a test line of the block
   builds (dev default, `dst`, `profiling`, `observe`,
   `observe,profiling`, `--all-features`, and `--release` default), each
   result recorded. Every warning is fixed at its site; an import or item
   used only under a `cfg` moves under that `cfg`. No `allow` is added.
2. The block gains `[clippy-release]`
   (`cargo clippy --workspace --all-targets --release -- -D warnings`)
   and one clippy line per feature set its test lines use, so lint
   coverage equals test coverage; BR-10 carries the same lines into CI.
3. `scripts/check_ashrae_reference.sh` runs
   `uv run --extra ochre python ...`, so the gate needs no prior sync;
   the Makefile comment goes.

**Acceptance test:** every new clippy line exits 0;
`make check-ashrae-reference` passes in a fresh worktree with no prior
`uv sync`.

**Verification:** the full block, including the new lines; `[golden]`
unchanged (no runtime code changes).

**Commit:** `build: lint every profile and feature set the tests build, and let the reference check sync its own dependency`

**Fold note** (`verification/fold-integrity-BR-21.md` L1, L2): the fold
`91815306` carries no hunk for the source's `MoistureZoneInvariant` import
gate or the `BoundaryDiagnosticInfo` test import because the fold's base
already had both (the gated import at `dwelling/mod.rs:102-103`; the test
uses the fully qualified path), and its message drops the source's
paragraph about the observe release build; `[clippy-release-observe]`
exiting 0 is the evidence that fix was not lost. The item-level gates of
`tests/conditioned_oracle.rs` are exercised only under `observe`; the
`[test-observe]` line of `verification/fix-headaudit.md` ran them.

**Dependencies:** none. Runs on branch `i08-fixes` (a defect in
committed code), before BR-10 sets CI's lines.

## BR-20: the Rust toolchain and the development tools at their latest releases, pinned

**Kind:** found defect (`dl-i08-architecture.dependencies`: every tool at
its latest release).

**Problem** (2026-10-02): BR-6b and BR-6c moved every crate and Python
dependency to its latest release, but not the toolchain or the tools that
build, test and audit them. `rustup check`: stable `1.94.1 (2026-03-25)`,
update available `1.99.0 (2026-09-28)`. `cargo search`: `cargo-nextest`
installed `0.9.137`, latest `0.9.146`. Nothing in the repository pins the
toolchain (no `rust-toolchain.toml`; CI uses `dtolnay/rust-toolchain@stable`
at nine sites in `.github/workflows/ci.yml`), so local, CI and golden
captures can each run a different compiler, and BR-10 would pin the golden
job to the outdated `1.94.1`.

**Change:**

1. Add `rust-toolchain.toml` at the repository root pinning the latest
   stable release (`channel = "1.99.0"` unless a newer stable exists when
   the row runs), with the `rustfmt`, `clippy` and `llvm-tools-preview`
   components CI uses. Every `dtolnay/rust-toolchain@stable` in `ci.yml`
   reads the same version; BR-10's golden job pin is this version.
2. Bring `cargo-nextest`, `cargo-deny`, `cargo-llvm-cov`, `uv` and
   `maturin` to their latest releases (each checked at run time with
   `cargo search`, `uv self update --dry-run` and PyPI); CI's
   `taiki-e/install-action` tool versions and the `pyproject.toml` build
   requirement name the same releases.
3. Fix every new clippy and rustc lint the new compiler reports, at its
   site; suppress none.
4. Goldens: `compare-all` in the default, `dst` and `profiling` builds on
   the new compiler. The compiler is a numeric event: any difference
   follows M-1's numeric entry procedure (bisect to the cause, record the
   delta per column in the `02-measurement.md` log, re-capture in this
   commit), and the physics suites (`plan.md` §3) run before and after.
5. The `[tools]` line of the `02-measurement.md` log records the new
   versions; the release timing of `consumer_shape_900s` is re-measured as
   an A/B against the old compiler (same session, interleaved).

**Operator step:** `rustup update stable` and the tool installs change the
shared machine; the operator runs them, or approves them, when the row
starts.

**Acceptance test:** every line of the verification block green on the
pinned toolchain; `rustc -V` inside the repository prints the pinned
version; `[golden]` identical or re-captured with the recorded delta; the
physics suites pass.

**Verification:** the full block of `plan.md` §3, plus the physics suites.

**Commit:** `build: pin the Rust toolchain at its latest release and upgrade the development tools`

**Dependencies:** BR-6b, BR-6c. Runs before BR-10 (row 16b), whose CI pin
it sets.

## BR-22: no lint is suppressed where the code can satisfy it

**Kind:** found defect (review of BR-21, `verification/review-BR-21-d41646e9.md`,
census of every non-vendor `allow(`).

**Problem** (at `d41646e9`): about 78 suppressions remain outside the
unused-import group BR-21's follow-up removes: about 20
`dead_code`, 5 `unused_variables`, 10 `needless_update` in tests, 8
single-site clippy allows, and about 35 `too_many_arguments`. Each hides
either dead code, a binding read in only some builds, or a signature
that should take a parameter struct. The review's classification is a
first pass.

**Change:** every suppression is removed and its cause fixed at the
site: dead code deleted; a cfg-dependent item moved under its cfg; a
long parameter list replaced by a parameter struct where that is the
cleaner API (for a pyo3 keyword-argument binding, the Python signature
stays as it is and the Rust side groups the values after parsing). A
suppression stays only where the lint is wrong for the site (a pyo3
binding whose Python signature is the API, an external macro), with a
one-line reason stating the constraint; each kept one is listed in the
commit body.

**Acceptance test:** `git grep -nE '^\s*#!?\[.*\b(allow|expect)\(' --
'*.rs' ':!vendors'` matches attribute lines only and lists exactly the kept
suppressions, 29 lines (the earlier `#!?\[(cfg_attr\([^)]*, *)?allow\(`
stopped at the first `)` and missed multi-line `cfg_attr` sites and every
`#[expect(`; a plain `\b(allow|expect)\(` also hits comments); every
`[clippy*]` line of the block exits 0.

**Verification:** the full block; `[golden]` identical (no runtime
change).

**State:** FIXED (2026-10-04, commit `8082592e`). The re-census at the
head found 95 code suppression sites (the review's ~78 was an estimate;
the unused_variables group was already gone with BR-21's folds).
64 sites fixed: 18 dead-code deletions (including the whole dead
StageSnapshot debug mechanism and the write-only telemetry counters),
3 items moved under the cfg that reads them, 4 stale allows deleted,
10 needless_update module allows removed with every firing literal
named, 23 parameter structs over roughly 150 call sites, and 4
single-site clippy fixes (the is_multiple_of swap sits in the tariff
evaluator behind its zero-interval guard, unreachable at zero). The
deletions orphaned inert `wall_area_m2` keys in 24 fixture and test
TOML literals; they went too. 31 suppressions kept with reasons: 22
pyo3 keyword-argument bindings (the Python signature is the public
API), 2 intentional panic sites under catch_unwind, 2
large_enum_variant on public schedule enums (boxing would change the
public shape), 1 deprecated backward-compat re-export, 1
BESTEST-roof approx_constant, 1 test-only state field required by a
single struct literal, 2 shared test-helper modules linked per binary.
Review SHIP (zero semantic drift across the ~45 sampled sites; the
if-let regated warmup binding probed compiling and value-identical);
red team SHIP (it ran the four profiles the implementer had skipped:
dst 7250, profiling 7243, release 7194, observe+profiling 7307, all
green; its scripted field-by-field diff of roughly 120 call sites
found zero value drift; both behavior probes proven; the kept-31 census
exact). Goldens bitwise identical in both profiles before and after the
cleanup; bench compile clean; the full default and observe nextest
suites green at the commit. Census correction (head audit,
`verification/audit-OD-13-BR-22-head.md` L8/L9): `8082592e` kept 32
`allow` sites, not 31 (the list above omits
`thermal_solver/longwave.rs:626`, a multi-line release-only
`cfg_attr(..., allow(clippy::unused_enumerate_index))` the old regex could
not see), plus five `#[expect(` sites outside the census. The fix branch
`fix-headaudit` (`verification/fix-headaudit.md`) removes the longwave
allow, the deprecated re-export, the ashrae152 test-only field allow and
all five expects; the kept census is now 29, every one with a one-line
reason at the site: 22 pyo3 keyword-argument bindings, 2 intentional
literal unwraps under catch_unwind in panic_hook tests, 2
large_enum_variant on the schedule enums, 1 BESTEST-roof
approx_constant, 2 shared test-helper modules compiled per binary.

**Commit:** `refactor: remove lint suppressions by fixing what they hid`

**Dependencies:** BR-21's follow-up (the unused-import group).

## BR-23: the Python package type-checks clean and the block checks it

**Kind:** found defect (editor type-check diagnostics during BR-21's
follow-up, 2026-10-04).

**Problem:** no block line type-checks the Python package or its
stubs, and pyright reports errors in shipped code: `gym_env.py:602`
assigns a `StepResult` to a `dict[str, Any]`; `dwelling.py:434-435`
pass an optional publication where `HelicsPublicationLike` is required,
and `:267`, `:305-307`, `:373-375` reach `helics` attributes pyright
cannot resolve; `_hares.pyi` (lines 17, 30, 33 and the same pattern
elsewhere) redeclares `__doc__`, `__module__` and `__qualname__` with
incompatible types; `tests/python/test_py_control.py:33` redeclares
`test_power_setpoint_rejects_nan`, so the first test of that name never
runs (`reportRedeclaration`). Tests are type-checked too, so a shadowed
test fails the line.

**Change:** pyright configured in `pyproject.toml` (strict for
`python/ochre_next`, the stub included; scripts at standard), run
through `uv run --locked --all-extras --all-groups pyright`; every
error fixed at its site (real `None` handling, correct types, a stub
generator fix rather than hand edits if the stub is generated, typed
protocols for the optional HELICS and gymnasium imports). No
`type: ignore`, no `reportX = false`. The block gains `[pyright]` and
CI runs it.

**Acceptance test:** `[pyright]` exits 0 with zero errors and zero
warnings; `[pytest]` unchanged.

**State:** FIXED (2026-10-04, commit `787487df`). The first run
reported 617 errors across the shipped package, the scripts and the
tests; every error fixed at its site, zero suppressions. The categories:
the helics wheel ships no py.typed (a local typings/ stub now models
the 3.6.1 surface, including the optional cli attribute and the
constructor's required handle, and the two call sites try the wheel's
factory first with the constructor as the typed fallback);
gymnasium's `Env.step` pins info at `dict[str, Any]` so the step
contract lives in runtime TypedDicts (`python/ochre_next/_hares_types.py`,
imported by the stub, re-exported by `rl`); the optional pybamm/PySAM
imports keep their ImportError fallbacks via `import_module` inside
the try (the find_spec intermediate would have raised on a
findable-but-broken PySAM install: the red team caught it, the
fallback restored and probed); the resstock async checksum counter
shadowed the sync accumulator (two same-named variables at HEAD;
renamed); a dead `df is not None` check removed (results() returns a
DataFrame or raises); the pvlib generator's `float(Series)` (removed
in pandas 3) and a latent NameError in diag_pv_trace.py fixed. The
stub is hand-edited (no generator exists) and was completed against
the built extension: five equipment classes, the SimulationConfig and
Dwelling members, the Telemetry island fields, Priority.from_str, the
EV surface. Review + red team found the shipped typing surface lying
in six places (six TypedDicts unimportable at runtime; the simulation
error's stub base ValueError against a runtime RuntimeError; two
third-party stub members the wheels do not have; a duplicated
assignment artifact); all fixed and runtime-verified (the exception
bases match the create_exception sites; every TypedDict's key set
checked against its Rust producer; the negative probe bites in all
three environments and the stub directory self-checks). Pyright: 0
errors, 0 warnings. Pytest: 1396 passed, 4 xfailed, 0 skipped. The
wheel builds and ships the new module.

**Commit:** `build: type-check the Python package and fix what it finds`

**Dependencies:** none.

## BR-24: tests leave nothing behind in the system temp directory

**Kind:** found defect (RT-9's implementer, 2026-10-04). BR-7 stopped
tests writing into the tree; the system temp directory is unguarded.

**Problem:** the regression, fleet, batch-step, PV lookup-table and
engine tests leave output files in the system temp directory (over
1,000 regression outputs per run), so repeated runs grow it without
bound and parallel runs can read each other's files.

**Change:** every test that writes a file writes under a per-test
temporary directory removed on drop (including on panic); no test
writes to a fixed or shared temp path. The block's `[stray]` check
gains the system temp directory: a file a block run leaves there fails
the line.

**Acceptance test:** a full block run leaves the system temp directory
with no new entries.

**Verification:** `[stray]` with the temp-directory check; the full
block. A failing Python test keeps its `tmp_path` directory for
debugging (`tmp_path_retention_policy = "failed"`); the keep is bounded
by `tmp_path_retention_count = 3`, pinned in `pyproject.toml`: only the
last three sessions' directories survive (measured: five failing
sessions, with and without xdist, leave three). A passing test's
directory is removed, so a green session leaves `pytest-of-<user>/`
empty. A failing session fails the block on its own; the kept directory
is not an extra leak.

**Commit:** `test: write every test output under a per-test temporary directory`

**Dependencies:** none. (A dwelling writing its diagnostics CSV with
`write_output` false is `03-output-reporting.md` OR-4's.)

## BR-25: the API docs build without warnings and the block checks it

**Kind:** found defect (found on `fix-headaudit`, 2026-10-05).

**Problem:** `cargo doc` reports about 8 broken intra-doc links in
`hares-equipment` alone, and many warnings from unit names written in
brackets (`[W]`, `[kWh]`), which rustdoc reads as links. `cargo doc` is
not a block line, so nothing stops new breaks.

**Change:** fix every broken link to name its real item, and write
units as `(W)` or in code spans, across every crate. Add a block line
`RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps
# [doc]` to `plan.md` §3 and to CI.

**Acceptance test:** the `[doc]` line exits 0; a deliberately broken
link makes it fail.

**Verification:** the full block.

**Commit:** `docs: fix broken API doc links and check the docs build in the block`

**Dependencies:** none.
