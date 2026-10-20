# Verification L1-a: plan.md §1-3, 01-baseline-repair, 02-measurement, 07-profiler

Scope: `plan.md` §1-3, `ledger/01-baseline-repair.md`, `ledger/02-measurement.md`,
`ledger/07-profiler.md`, checked against HEAD `ebdb1a17` and `decision-log.md`.

Method: I read the cited code at `ebdb1a17` (`git show`). I ran every command
in an isolated `git archive HEAD` copy, never in the main
tree:

- `cargo check` and `clippy` under default, `check_invariants`,
  `--all-features`, and MSRV 1.89.
- The full nextest matrix: debug default, `-F observe`, `-F dst`,
  `--release -F check_invariants`, and plain `--release`.
- `cargo bench --no-run`, `cargo fmt --check`, and `cargo deny check` with no
  config.
- A trial of the BR-6 dependency upgrade (pyo3/numpy 0.29, arrow/parquet 59,
  quick-xml 0.42, nalgebra 0.35, postcard without default features,
  rust-version 1.89).

The archive copy has no `vendors/OCHRE` submodule, so
`hares-io::hpxml_parsing_tests::pool_and_hot_tub_loads_parsed_from_ochre_fixture`
fails in every copy run. That failure comes from the environment, not from
the plan, and is ignored below.

Counts: **5 blockers, 17 majors, 16 minors.**

---

## Blockers

### B1. plan.md §3 plus 01 BR-5 and BR-6: the verification block cannot pass between rows 5 and 22

- **Evidence.** §3 says the full block runs "after every entry's commit, before
  the next entry starts". `00-schedule.md` says the same. The block contains a
  plain `cargo nextest run --workspace --release`. BR-5 has no commit of its
  own and "folds into OR-5's commit", which is schedule row 23.
- **Confirmed live.** The plain-release run fails exactly the three BR-5 tests:
  - `dwelling::tests::build_equipment_column_map_still_panics_on_schema_drift_for_known_equipment`
  - `ev_soc_check_fails_loudly_when_core_output_soc_is_absent`
  - `ev_capacity_check_fails_loudly_when_ev_telemetry_keys_are_absent`

  The score was 6953/6957 passing; the fourth failure is the OCHRE-submodule
  artefact above.
- **Consequence.**
  - §3's claim that the block "passes at HEAD only once every entry in
    `01-baseline-repair.md` has landed" is false, because BR-5 lands at row 23.
  - Rows 8-22 cannot meet the gate, and "No entry may narrow this block"
    forbids the only way around it.
  - BR-6's acceptance ("The full verification block passes on the upgraded
    dependency set") cannot pass at row 5 either.
- **Correction.** Choose one of these:
  - (a) Move OR-5 (unconditional invariants) to directly after BR-8. Its
    physics-invariant half does not depend on OR-1's catalogue. If OR-5's
    health-counter half needs OR-1, split OR-5 into "OR-5a: invariants and
    `resolve_col` drift check unconditional" (early) and "OR-5b: health
    counters" (after OR-1).
  - (b) Add an explicit, named known-failure contract to §3: "until row 23 the
    plain-release line must fail with exactly these three test names and no
    others". Give the command that checks this, e.g. nextest's
    `--message-format libtest-json` output filtered with `jq`. Change BR-6's
    acceptance to "the block passes except that contract".

  Option (a) matches the decision log ("NaN and physics invariant violations
  always fail the run, in every build profile").

### B2. 01 BR-2: its acceptance command cannot pass until BR-3 lands, but BR-2 runs first

- **Evidence.** After BR-1, `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  fails first on the rustc lints `cannot test inner items` and `function ... is never used`:
  `crates/hares-core/tests/timezone_weather_regressions.rs:651:5, 652:8`. That
  is the BR-3 defect, because `--all-features` includes `dst`.
- **Order.** BR-2 is row 2 and BR-3 is row 3. BR-2 lists "Dependencies: BR-1"
  only.
- **Correction.** Make BR-2 depend on BR-3 and swap schedule rows 2 and 3 (BR-3
  has no dependencies). Alternatively, fold the BR-3 move into BR-2's commit.

### B3. plan.md §3 verification block: `uv run pytest --all-extras` is not a valid command

- **Evidence.** `--all-extras` is a `uv run` flag, not a pytest flag. Placed
  after `pytest` it is passed to pytest, which rejects it:
  `python -m pytest --all-extras` gives `error: unrecognized arguments: --all-extras`
  (pytest 9.x from the project venv). BR-6's verification repeats the command.
- **Missing build step.** The block also has no step that builds the extension
  before pytest runs. CI does `uv run maturin develop` first (`ci.yml:157-159`).
  Without it, pytest tests a stale `_hares*.so`.
- **Correction.** Replace the line with:
  ```
  uv run --all-extras maturin develop --release
  uv run --all-extras pytest
  ```
  Add `-m ""`, or state the intended marker expression, because
  `pyproject.toml` `addopts` always deselects `slow` and `ochre`. Apply the
  same fix to BR-6's verification text.

### B4. 07 PR-3: a real allocation counter makes every debug `-F profiling` step panic

- **Evidence.** `crates/hares-core/src/dwelling/mod.rs:6510-6517`, under
  `#[cfg(feature = "profiling")]`:
  ```rust
  let alloc_after = hot_path_alloc_counter();
  if alloc_after > alloc_before {
      self.profiling.hot_path_alloc_violations += 1;
      debug_assert_eq!(alloc_after, alloc_before, "hot-path allocation detected during timestep");
  }
  ```
  Today the counter is a stub that returns 0 (`synthetic.rs:2126-2129`), so the
  assert never fires.
- **What PR-3 triggers.** Once PR-3 lands (row 18), any step that allocates
  panics in a debug build with `-F profiling`. Steps do allocate today: the
  telemetry `HashMap` `clone_from`/insert at `mod.rs:4924` and `:6436`,
  recorder buffer growth, and so on. RT-1 and RT-3, which remove those
  allocations, come at rows 43-44. A process-global counter also counts other
  threads' allocations.
- **Tests that break.** PR-1's `profiling_water_heater_timer_nonzero` (row 16,
  debug `-F profiling`) and PR-4's `profiling_flush_timer_nonzero` (row 29)
  both start panicking once PR-3 lands.
- **Correction.** PR-3's change must:
  - remove the `debug_assert_eq!`, so violations are counted and reported,
    never panicked on;
  - define the counter as thread-local, e.g. a `const` `thread_local!`
    `Cell<u64>` incremented in `GlobalAlloc::alloc`, so parallel threads do not
    pollute it;
  - add a test that runs a multi-step dwelling under debug `-F profiling` and
    asserts it completes with `hot_path_alloc_violations > 0` at HEAD
    behaviour.

  RT-1 and RT-3 then assert `== 0`.

### B5. plan.md §2.2: `Destination::Callback(Py<PyAny>)` cannot live in `hares-io`

- **Evidence.** §2.2 places `pub enum Destination { ..., Callback(Py<PyAny>) }`
  in `// crate: hares-io`. `crates/hares-io/Cargo.toml` `[dependencies]` has no
  `pyo3`. The only pyo3 user is `hares-python`, which depends on `hares-io`, so
  the reverse edge would be a cycle or would force pyo3 into the core I/O crate.
- **Internal contradiction.** The inline comment ("behind a trait boundary in
  hares-io") contradicts the literal variant type.
- **Correction.** In `hares-io`:
  - define `pub trait BatchSink: Send { fn write_batch(&mut self, batch: &RecordBatch) -> Result<(), SinkError>; fn finish(&mut self) -> Result<(), SinkError>; }`
  - make the variant `Callback(Box<dyn BatchSink>)`.

  `hares-python` then implements `BatchSink` for a wrapper around `Py<PyAny>`
  that chains the Python exception into `SinkError`. Update §2.2's code block to
  match.

---

## Majors

### M1. plan.md §3 "Branch" contradicts decision-log `dl-i08-process.branch`, and PR #27 is not scheduled

- **What the decision log says.** "feat/output-results ... is recreated from
  initial-implementation after the operator merges PR #27 (NLR host fix,
  profiling bindings, profiling example, fmt fix)".
- **What §3 says.** "current HEAD: `ebdb1a17` ... do not delete and recreate
  it", and it lists the PR #27 merge as an operator checkpoint "if not already
  merged".
- **Live state.**
  - PR #27 is OPEN: base `initial-implementation`, head
    `fix/data-host-renamed-nlr` @ `89f32822`.
  - `feat/output-results` == `initial-implementation` == `ebdb1a17`.
  - No schedule row places the merge.
  - PR #27 changes profiling code that 07 depends on. It adds
    `profiling = ["hares-core/profiling"]` to `crates/hares-python/Cargo.toml`,
    so `--all-features` now also builds the Python crate with `profiling`. It
    adds `PyDwelling::profiling_summary()`, which exposes `water_heater_s` and
    `hot_path_alloc_violations`.
- **Correction.**
  - Add schedule row 0: "operator merges PR #27 into `initial-implementation`;
    `git -C HARES merge --ff-only initial-implementation` on
    `feat/output-results`". No recreation is needed because the branches are
    identical.
  - Rewrite `dl-i08-process.branch` to say "fast-forwarded", not "recreated",
    so the two documents agree.
  - State the resulting base commit that all `file:line` evidence is pinned to.
    PR #27 does not touch any file cited in my scope, so the citations survive.

### M2. 01 BR-6: `deny.toml` license policy is not specified, and there is an ODbL dependency

- **Evidence.** With no config, `cargo deny check` at HEAD gives
  `advisories FAILED, bans ok, licenses FAILED, sources ok`, with 158
  `error[rejected]` license errors. So the failure is not only the six
  advisories the evidence lists.
- **The decision the plan skips.** `cargo deny list` shows
  `tzf-dist@0.0.2026-b-fix1` (via `tzf-rs`, workspace `Cargo.toml:59`) under
  **ODbL-1.0**, a share-alike database licence shipped in a BSD-3-Clause
  project. That is a real licensing decision, not a mechanical step.
- **Licences present.** Apache-2.0, Apache-2.0 WITH LLVM-exception, MIT,
  BSD-2-Clause, BSD-3-Clause, BSL-1.0, CC0-1.0, Unicode-3.0, Unlicense, Zlib,
  ODbL-1.0. LGPL-2.1-or-later appears only as an alternative on `r-efi`, which
  is also MIT/Apache-2.0.
- **Correction.**
  - Give the exact `[licenses] allow` list above, minus LGPL.
  - Add either an operator-approved `[[licenses.exceptions]] crate = "tzf-dist", allow = ["ODbL-1.0"]`
    or an explicit decision-log entry on ODbL.
  - Give `[bans] multiple-versions = "warn"` (hashbrown and cpufeatures are
    duplicated today) and `[sources] allow-registry = ["https://github.com/rust-lang/crates.io-index"]`.
  - Put the ODbL item behind an operator checkpoint.

### M3. 01 BR-6: raising `rust-version` to 1.89 brings in about 322 `collapsible_if` clippy errors the entry never mentions

- **Evidence.** In a copy with only `rust-version = "1.89"` changed (plus the
  BR-1 fix), `cargo clippy --workspace --all-targets --all-features` reports
  `this if statement can be collapsed` in:

  | Crate | Count |
  |---|---|
  | hares-equipment | 92 |
  | hares-io | 67 |
  | hares-python | 60 |
  | hares-core | 41 |
  | hares-types | 30 |
  | hares-envelope | 22 |
  | hares-tariff | 8 |
  | hares-fleet | 2 |

  These are let-chain suggestions. Clippy gates them on MSRV ≥ 1.88 with
  edition 2024.
- **Other breakage from the upgrade.**
  - `crates/hares-python/build.rs:15` uses deprecated
    `InterpreterConfig::lib_dir` under pyo3-build-config 0.29. This is fatal
    under `-D warnings`.
  - `crates/hares-io/src/hpxml/building.rs:363-406` has 8 compile errors from
    the quick-xml 0.42 API change.
  - `hares-io` has 2 deprecation warnings.
- **Correction.** Split BR-6 into two commits.
  - BR-6a: the dependency and API changes.
  - BR-6b: `refactor: collapse nested ifs into let chains for MSRV 1.89`, a
    mechanical clippy `--fix` across roughly 322 sites, bitwise-neutral.
  - Name `build.rs:15`, `building.rs:363-406` and the MSRV lint surge in the
    entry, so the diff size and review load are known up front.

### M4. 01 BR-6 and 02 M-1: no tool can detect the nalgebra output change the decision log requires recording

- **What the decision log requires.** `dl-i08-architecture.dependencies`: "Any
  output change from the nalgebra upgrade is recorded before the baseline is
  measured." BR-6 repeats this ("investigated and recorded in
  `02-measurement.md`").
- **The gap.** BR-6 is row 5. The only bitwise capture tool (M-1) is row 13,
  and M-2's fixtures (row 14) are captured after the upgrade. There is no
  pre-upgrade capture to compare against, and the existing parity tests are
  tolerance-based, so they cannot see a bitwise change.
- **Correction.** Move M-1 before BR-6. Capture the M-2 fixture set on
  pre-upgrade HEAD, apply BR-6, run `frame_golden compare-all`, and record the
  per-column delta. Alternatively, split the nalgebra bump out of BR-6 into its
  own entry after M-1/M-2 with that procedure. Either way, the `paste` advisory
  stays open until then, so the verification-block `cargo deny` line needs the
  B1-style known-state treatment.

### M5. 01 BR-7: the stray-file census and fix miss the Python tests, and the evidence list is incomplete

- **Evidence.** At HEAD, `git status --porcelain --ignored` also lists these,
  which the entry omits:
  - `crates/hares-core/dwelling_100_diagnostics.csv`
  - root `dwelling_1_diagnostics.csv`
  - `tests/python/dwelling_42.csv`
- **Where the strays come from.** A nextest run in the copy creates only
  `crates/hares-core/dwelling_0.csv`, `dwelling_1.csv` and
  `dwelling_100_diagnostics.csv`. Every root-level file comes from something
  other than nextest: pytest runs from the repo root. The Python config also
  defaults to writing: `crates/hares-python/src/py_config.rs:201`
  `write_output.unwrap_or(true)`, and `py_dwelling.rs:2319`.
- **The gap.** BR-7's change and acceptance cover only `crates/*/tests/**` and
  `crates/*/benches/**` plus a nextest run.
- **Correction.**
  - Extend the census to `tests/python/**` and `benches/common.rs:127`, which
    sets `write_output: true`.
  - Make the operator-deletion list the exact current output of
    `git status --porcelain --ignored | grep -E 'dwelling_[0-9]+(_diagnostics)?\.csv'`.
  - Make the acceptance before/after comparison wrap both
    `cargo nextest run --workspace` and `uv run pytest`.

### M6. 01 BR-7: the proposed CI stray check fails on a clean CI checkout, and its placement is unspecified

- **Evidence.** On a fresh checkout, `target/` does not exist before the test
  run and appears afterwards as `!! target/`. So "diffed before and after ...
  must be empty" fails on the first run. pytest also creates `__pycache__/` and
  `.pytest_cache/`. The entry names no CI job or step.
- **Correction.**
  - Build first (`cargo nextest run --workspace --no-run`), then take the
    "before" listing.
  - Use `git status --porcelain --ignored --untracked-files=all -- . ':(exclude)target' ':(exclude)**/__pycache__' ':(exclude).pytest_cache'`.
  - Add it as named steps in the `test` job (`ci.yml:124-142`) and the
    `python-test` job (`ci.yml:144-164`).
  - Add the same pair of commands to the §3 verification block. §3 and the
    decision log say every run relies on the check, but the block omits it.

### M7. 01 BR-8: the evidence is wrong (10 ignored tests, not 2), the fix goes against codebase convention, and the grep false-passes

- **Evidence.** `git grep -n '#\[ignore' ebdb1a17 -- '*.rs'` finds 10 ignored
  tests.
  - Three use `#[ignore = "debug helper"]`:
    `crates/hares-core/tests/alignment_oracles_regressions.rs:515,709,770`.
  - Five in `tests/bestest/` already use reason strings: `bestest_diagnostic.rs:114`
    and `mod.rs:404,506,619,665,882` (the `bestest` test target is
    `crates/hares-core/Cargo.toml:58-60`).
  - Only `solar.rs:2461` and `resstock_smoke.rs:299` are bare `#[ignore]`.
  - `crates/hares-physics/tests/solar_parity.rs:187,264` have comments claiming
    reference values "are #[ignore]", but no such ignore exists.
- **Why the fix and check fall short.** A "one-line comment" is weaker than the
  convention the other 8 sites use (`#[ignore = "reason"]`), which nextest and
  libtest show in output. The verification grep
  `grep -rn -B2 '#\[ignore\]' --include=*.rs crates/` misses both
  `#[ignore = ...]` and `tests/` outside `crates/`.
- **Correction.**
  - Convert both bare sites to `#[ignore = "manual diagnostic: prints state, no assertion"]`.
  - Delete the two misleading `solar_parity.rs` comments.
  - Change the verification to `git grep -nE '#\[ignore\]' -- '*.rs'` returning
    no matches, meaning every ignore carries a reason.
  - Change the problem statement to "10 ignored, 2 without a reason".

### M8. 02 M-1: `compare` cannot re-run a fixture, because the inputs it needs are never stored

- **Evidence.** `capture <fixture-name> <hpxml> <schedule> <weather> [--config <toml>]`
  writes `<name>.arrow` and a sidecar holding only "schema + row count +
  SHA-256". `compare <fixture-name>` "re-runs the same inputs". The inputs,
  config, start time, duration, `time_res`, verbosity and required Cargo
  features (the DST fixture needs `-F dst`) are not persisted anywhere.
- **Correction.** Replace the positional CLI with a checked-in manifest,
  `tests/fixtures/golden/<name>.toml`, holding:
  - kind (`dwelling` or `fleet`) and input paths;
  - the `SimulationConfig` TOML;
  - required features.

  `capture <name>` reads the manifest, and `compare` refuses when the running
  binary lacks a required feature. The sidecar keeps the digest and schema.
  Compare validity bitmaps before `to_bits()`, and compare only valid slots.

### M9. 02 M-1: the tool lacks modes that later ledger entries depend on

- **No fleet mode.** M-2 `fleet_mixed_offset`, 04 FL-4 (line 143) and 09
  (lines 398, 476, 514) need fleet capture. M-1's CLI accepts one HPXML only.
- **No Python-driven capture.** 09 B-3 (line 170: "golden fixtures re-captured
  for every Python-driven golden fixture") needs it. M-1 links Rust crates
  only.
- **No column-subset or schema-evolution compare.** 03 line 131 ("compare
  bitwise equal for the columns every fixture already ...") and OR-7, which
  turns `Time` from Utf8 into Timestamp and so changes every fixture's schema,
  need it.
- **No weather-wrap fixture.** 06 line 100 needs "a weather-wrap fixture"; M-2
  does not produce one.
- **Correction.** Specify in M-1:
  - the fleet manifest kind;
  - a `--columns <list>` compare filter;
  - a declared schema-migration rule (a fixture is re-captured in the entry
    that changes the schema, with the diff recorded);
  - a `pytest`-side capture helper, or drop the Python-driven claim from 09.

  Add `weather_wrap` to M-2's fixture list.

### M10. 02 M-1 and plan.md §3: golden comparison never gates anything, and M-1's acceptance is manual

- **Evidence.** The §3 verification block has no `frame_golden compare-all`
  line. M-1's acceptance ("a deliberately perturbed rebuild (one changed
  constant) makes compare exit non-zero") is a manual procedure, not a named
  test in a test target.
- **Correction.**
  - Add `cargo run --release -p frame_golden -- compare-all` to the
    verification block. Run it again under `-F dst` if DST fixtures need that
    feature.
  - Replace M-1's acceptance with tests in `tools/frame_golden/tests/compare.rs`:
    - `compare_identical_batches_passes`;
    - `compare_reports_first_mismatch_row_and_column` (two in-memory batches
      differing in one f64 bit);
    - `compare_treats_nan_payloads_bitwise`;
    - `compare_rejects_validity_mismatch`.
  - Add `"tools/frame_golden"` to `[workspace] members` in `Cargo.toml:2-13`.

### M11. 02 M-2: fixture inputs and settings are not named, and no leap-year weather exists

- **Evidence.**
  - "one building", "a DST-observing timezone" and "a leap year": no HPXML,
    schedule, weather file, start date, verbosity or features are named.
  - Every checked-in EPW is TMY 1995: `tests/fixtures/weather/*.epw` and
    `tests/fixtures/parity/*/weather.epw`, each 8760 data rows with no Feb 29.
  - The ResStock weather is 2018:
    `tests/fixtures/resstock/2025.1/weather/*_2018.csv`.
  - The "leap_year_february" behaviour therefore depends on an unspecified
    TMY-over-leap-year policy, which is itself numeric fix B-1/B-9 territory.
- **Row-count claim is wrong.** The claim "the DST year has one fewer or one
  more row than a non-DST year at the same step size" is wrong for a fixture
  that spans both spring-forward and fall-back. The two transitions cancel.
- **Correction.** For each fixture give the exact manifest (see M8):
  - HPXML path, e.g. `tests/fixtures/parity/cz4a_ashp_hpwh/`;
  - weather file;
  - start (`2024-01-01T00:00` local for the leap fixture);
  - duration, step and verbosity (state 2, matching `context.md`);
  - features (`dst` for the DST fixture).

  State the expected row count exactly (35 040 for a 365-day 900 s run, 35 136
  for 366 days). Name the leap-year weather source, or state that the fixture
  pins today's TMY Feb-29 handling as the baseline B-1 will change.

### M12. 02 M-3: the measurement inputs and driver are not in the repo, and its "commit" has nothing to commit

- **Missing inputs and driver.** Buildings 8590 and 46363 appear nowhere under
  HARES `tests/fixtures`. They live in the calibration system:
  `~/src/HARES_CALIBRATION_SYSTEM/tools/provision_data.py`,
  `bench_calibration.py`. No driver binary is named for `perf record`. The
  obvious one, `crates/hares-core/examples/profile_sim.rs`, exists only in
  unmerged PR #27.
- **Nothing to commit.** The measurement log is
  `.shipwright/initiatives/I-08/ledger/02-measurement.md`. The whole
  initiatives directory is git-ignored (`git status --ignored` shows
  `!! .shipwright/initiatives/`), and planning artefacts must not ship in the
  public repo. So "Commit: `docs: record the re-measured I-08 baseline`" is
  impossible and would be wrong.
- **Correction.**
  - Name the exact input paths and the provisioning command.
  - Name the driver (`cargo run --release -p hares-core --example profile_sim -- <args>`
    after the M1 merge) with its full argv.
  - Mark M-3 "(no commit)", as `00-schedule.md` already does for other
    measurement rows.

### M13. 07 PR-1: the water-heater timer as specified double-counts with `hvac`

- **Evidence.** The `hvac` timer (`mod.rs:5702-5703` start, `5957-5966` stop)
  wraps the whole unified equipment step. That step includes the water heater:
  "Step 3: unified equipment step in stage_rank order". There is no separate
  water-heater phase call to wrap. Adding a `water_heater` timer inside the
  loop counts that time in both `hvac` and `water_heater`. The `other`
  computation at `mod.rs:6499-6508` then under-reports.
- **Name mismatch.** The struct is `DwellingProfilingSummary`
  (`mod.rs:242-252`), not `ProfilingSummary`.
- **Correction.** Specify the attribution rule. Either:
  - (a) rename `hvac` to `equipment` and delete `water_heater`, updating
    `engine.rs:365-391` and the PR #27 binding; or
  - (b) time water-heater `EndUse` equipment inside the loop, subtract it from
    `hvac`, and add a test that
    `hvac + water_heater + envelope_solve + schedule_load + io + other == total step time`
    within one `Instant` read.

  Option (a) is the honest minimum.

### M14. plan.md §3 and 07 PR-1 to PR-4: `-F profiling` never runs in the verification block or CI

- **Evidence.** The block's nextest lines cover default, `observe`,
  `check_invariants` release, plain release and `dst`. `profiling` is only
  compiled, by clippy `--all-features`. `ci.yml:135-137` never passes
  `profiling`. So every PR-* acceptance test, and the allocation-count
  assertions RT-1 and RT-3 depend on (per PR-3), would never run on the
  standing gate. This is the same defect class BR-4 fixes for `dst`.
- **Correction.** Add `cargo nextest run --workspace --no-fail-fast -F profiling`
  to §3 and to the CI `test` job. That can happen in PR-1's commit, the first
  entry that adds such a test.

### M15. 07 PR-2: the acceptance test cannot be implemented as written, and the change leaves the design open

- **Evidence.** The acceptance offers "test-only file-open counting, or a
  direct assertion that the new function uses `getrusage`". There is no
  in-process way to count `open(2)` calls without LD_PRELOAD or seccomp, and a
  "direct assertion that the function uses getrusage" is not a behavioural
  test.
- **Design left open.** The change also says "cache ... or consolidate ...,
  whichever the review ... finds correct".
- **Missing dependency.** `libc` is only a `[dev-dependencies]` entry of
  hares-core (`crates/hares-core/Cargo.toml:46`). `ru_maxrss` is KiB on Linux
  and bytes on macOS, and `getrusage` does not exist on Windows.
- **Correction.**
  - Consolidate to one read at step end. The high-water mark is monotone, so
    the per-phase reads add nothing.
  - Add `libc = { workspace = true, optional = true }` and
    `profiling = ["dep:libc"]`, gated `#[cfg(unix)]`, with a macOS
    bytes-to-KiB conversion and a Windows `compile_error!` or an explicit
    `None`.
  - Replace the acceptance with `profiling_hwm_matches_getrusage`: after a
    run, `memory_high_water_kb > 0` and `<=` a `getrusage(RUSAGE_SELF)` read
    taken immediately afterwards.
  - Remove `current_process_hwm_kb` (`synthetic.rs:2109-2121`) so no
    `/proc/self/status` reader remains. That is compiler-checked.

### M16. 07 PR-3: the mechanism is explicitly deferred ("an implementation decision ... not invented here")

- **Why that fails.** It breaks `dl-i08-process.plan_form` ("ready to execute")
  and §1 ("without inventing design").
- **Risk in the first option.** A `#[global_allocator]` in the `hares-core`
  library under a feature applies to every binary linking it, including the
  PR #27 Python cdylib. It causes a link error for any downstream binary that
  sets its own allocator.
- **Existing precedent.** A per-test-binary counting allocator already exists:
  `crates/hares-envelope/tests/synthetic_box_zero_alloc.rs:1-27` and
  `radiant_gain_weights_zero_alloc.rs:47`.
- **Correction.**
  - Specify a shared test-support module, e.g.
    `crates/hares-core/tests/support/counting_alloc.rs` or a small dev-only
    crate, with a thread-local counter.
  - Each allocation-counting test binary installs `#[global_allocator]`.
  - `hot_path_alloc_counter()` reads the thread-local through a hook that is a
    no-op outside those binaries.
  - Deduplicate the two hares-envelope copies onto it.
  - Tighten the acceptance to "delta equals N exactly", not "or monotonic".
  - See B4 for the `debug_assert` that must go.

### M17. plan.md §2: public types the frame promises to define are never defined anywhere

- **Evidence.** §2.1 says `ReportResult`, `RunMetrics`, `RunHealth`, `Warning`
  and `Provenance` "are defined where they are produced (§2.2–§2.4)". §2.2-§2.4
  do not define `ReportResult`, `RunMetrics`, `Warning` or `Provenance`.
- **Undefined types.** Also undefined in the frame and in every ledger file
  (grep over `ledger/*.md`):
  - `Unit`, `VariableKind`, `Aggregation`, `Sampling`, `Selection`,
    `Frequency`, `WarmupOutcome`, `MetricsStatus`, `HealthPolicy`, `Coverage`;
  - `ActorId`, which does not exist in the codebase. Only `ZoneId`
    (`hares-types/src/environment.rs:96`) and `EquipmentId`
    (`hares-types/src/equipment.rs:19`) exist.
- **Name conflicts.** `RunMetrics` does not exist; the current type is
  `hares_io::output::metrics::FullSimulationMetrics` (`metrics.rs:237`), and
  the frame does not say whether it is renamed. §2.2's `Source` (variable
  origin) and §2.7's `Source::{Path, Text, Bytes}` (input content) are two
  different types with the same name, both plausibly in `hares-io`.
- **HealthPolicy shape conflict.** It is "empty by default" in §2.3 but used as
  `HealthPolicy::Strict` in `08-consumer.md:93`.
- **Correction.** Add code blocks for each type with its crate.
  - Define `ActorId` in `hares-types`.
  - Rename one `Source`, e.g. `InputSource` for §2.7.
  - State `RunMetrics = FullSimulationMetrics` (renamed) or a new struct.
  - Define `HealthPolicy` as a struct of per-counter `Option<u64>` limits with
    a `strict()` constructor, and correct `08-consumer.md` to match.

---

## Minors

1. **01 BR-1: "fails under ... `check_invariants`" is false.**
   `cargo check --workspace --all-targets -F check_invariants` exits 0 at
   HEAD. The broken `tracing::debug!` is gated on `observe` only, and
   `hares-core`'s `check_invariants` does not enable `hares-equipment/observe`.
   *Correction:* say it fails under `observe` (and so under `--all-features`),
   and that it turns green the `-F observe` line only.
2. **01 BR-1: "before `update_daily` consumes the day's accumulator" is
   inaccurate.** `update_daily(&self.u_neg_table, &self.rainflow)` takes
   `&RainflowCounter` (`degradation.rs:510`). The day's DOD list is cleared
   later, by `self.rainflow.reset_daily()` (`battery/mod.rs:1494`).
   *Correction:* read `self.rainflow.sum_squared_dod_daily()`
   (`degradation.rs:144`, `pub(crate)`) any time before `reset_daily()`. The
   fix compiles; I checked it in the copy.
3. **01 BR-2: the problem statement is wrong.** It says "clippy fails at HEAD
   under default features, independent of BR-1". Default-feature
   `cargo clippy --workspace --all-targets -- -D warnings` exits 0 at HEAD.
   All four sites are `observe`-only:
   - `resstock_observer_diag.rs:10` `#![cfg(feature = "observe")]`;
   - `lib.rs:16` `#[cfg(feature = "observe")] mod observer_capture;`;
   - `equipment_ordering_tests.rs:461`.

   The lint list itself is exactly right; I confirmed it with `--keep-going`.
   *Correction:* retitle it "clippy fails under `observe`/`--all-features`
   once BR-1 compiles", with dependencies BR-1 and BR-3 (see B2).
4. **01 BR-3: the moved test keeps planning IDs and a performative
   assertion.** Lines 653-658 and 723-726 contain `T-0212` and "No separate
   assertion needed; this comment documents the contract". The test only
   asserts `day_of_year` and non-empty irradiance, so the name "consistent
   across solar and perez" over-claims. §3's writing rule and slop rule apply
   to a touched file. *Correction:* strip `T-0212` and the narrating comments,
   and rename to `spring_forward_day_of_year_at_hourly_boundary_steps`, or add
   a real Perez-vs-solar-position assertion. I confirmed the un-nested test
   passes under `-F dst`.
5. **01 BR-3: the ad-hoc grep "style review" is non-deterministic.** The rustc
   lint `unnameable_test_items` already catches nested tests, and clippy
   `--all-features -- -D warnings` turns it into an error. *Correction:* cite
   that as the standing check and delete the grep.
6. **01 BR-3: small citation drift.** The outer fn starts at line 608 (607 is
   `#[test]`). The nested block runs 650-727, and the file ends at 728.
7. **01 BR-4: the CI description is inaccurate.** `ci.yml:135-137` runs debug
   `-F observe`, release `-F check_invariants`, and release
   `-p hares-equipment`. There is no workspace default-feature run. The new
   line should carry `--profile ci` like its siblings. CI clippy
   (`ci.yml:36`) is `cargo clippy --workspace -- -D warnings`, without
   `--all-targets --all-features`, so the lints BR-2 fixes are not guarded in
   CI. *Correction:* BR-4 also changes `ci.yml:36` to the verification-block
   clippy line.
8. **01 BR-5: the interim acceptance is ambiguous.** "the three failures ...
   are the only differences between the default-feature and
   `check_invariants` nextest runs" is ambiguous because the test sets differ:
   6957 tests vs 6992, since 35 tests are cfg-gated. *Correction:* "the only
   failing tests in the plain-release run". I confirmed exactly those three.
9. **01 BR-6: the acceptance command's expected result is not stated.**
   `cargo tree ... -i paste` exits 101 with "did not match any packages" when
   the crate is absent. *Correction:* state the expected non-zero exit and
   message. I confirmed in the trial upgrade that `paste`, `atomic-polyfill`
   and `heapless` are all gone and `cargo deny check advisories` is `ok`.
10. **plan.md §2.7 cites the wrong path for the `serde(flatten)` example.**
    It cites `crates/hares-io/src/hpxml/heat_pump_config.rs:19-22`; the file is
    `crates/hares-equipment/src/hvac/heat_pump_config.rs`. Lines 19-22 are the
    comment, and the `#[serde(flatten)]` fields are at 262, 297 and 483.
11. **plan.md §2.2 cites the wrong file for `default_output_path`.** It is at
    `crates/hares-core/src/dwelling/conversions.rs:590`, not
    `dwelling/mod.rs`.
12. **plan.md §3 cites the wrong lines for the slop example.** The false "slot
    reuse" comment is at `mod.rs:6431-6433`. Lines 4922-4923 are
    `let telemetry = eq.telemetry(); match ...`, with no comment.
13. **plan.md §3 operator checkpoint count.** It says the consumer
    `scratch/` has "at least 62 top-level directories"; there are 61 today
    (7.0 GB is correct). *Correction:* "61 top-level directories at
    `c71c542`", or drop the count.
14. **plan.md §3 writing rule has no command.** It says "every entry's
    verification step includes a grep", but no entry in 01, 02 or 07 contains
    one and no pattern is defined. *Correction:* define one command in §3,
    e.g. `git diff --name-only HEAD~1 | xargs git grep -nE '\b(T|CW|CFG|I|OR|BR|PR|RT|CN|OD|SL|FL|PY|M|B)-[0-9]+\b|—' --`,
    and reference it from every entry. Also, `cargo bench --no-run` in the
    block covers only default-members; CI uses `--workspace`, so align them.
15. **Broken `review.md` citations.** PR-2 cites `review.md` §1.4 and
    "2.3-3.2×"; PR-4 cites `review.md` §3; M-3 cites `review.md`
    §"Measurement conditions". `review.md` has only §1 and §2, and none of
    those figures or headings exist in it. The 93 ms and 9 ms figures are in
    `plan-critique.md:62,72-73`. *Correction:* cite the real locations, or put
    the measurement-condition rule into M-3 itself.
16. **07 PR-4 and the PR #27 binding.** PR-4 does not name the new field
    (e.g. `flush: StdDuration`). After PR #27, the Python
    `profiling_summary()` must expose it too. The PR #27 binding returns
    zero-filled timings when the feature is compiled out, which is a silent
    fallback. *Correction:* name the field, add it to the binding, and make
    the non-`profiling` binding raise `NotImplementedError` naming the missing
    feature. That becomes a slop item in `10-slop-cleanup.md`.

---

## Verified correct

- **BR-1.** Defect confirmed live: `E0425 sum_sq_dod` at `battery/mod.rs:1468`
  under `--all-features`. The fix direction (capture
  `self.rainflow.sum_squared_dod_daily()` in the `observe` binding at
  1457-1459) compiles and changes no physics.
- **BR-2.** The lint list is exact and complete:
  `equipment_ordering_tests.rs:532,536` `unnecessary_map_or`,
  `resstock_observer_diag.rs:30` `ptr_arg`, and `observer_capture.rs:335,345`
  `field_reassign_with_default`. There are no other clippy findings under
  `--all-features --keep-going` once BR-1 and BR-3 are applied.
- **BR-3.** The nesting is confirmed. The un-nested test runs and passes under
  `-F dst`; all 5 dst tests in the file pass.
- **BR-4.** `dst` is absent from every CI nextest line. `dst` gates `clock.rs`,
  `environment.rs`, `blueprint.rs`, `dwelling/mod.rs` and the timezone tests.
  With BR-1 and BR-3 applied, a debug `-F dst` workspace run passes (7049/7050;
  the one failure is the submodule artefact).
- **BR-5.** Every citation is correct:
  - `resolve_col` panic gate at `mod.rs:658-662`;
  - test at `mod.rs:15226`;
  - `check_invariants` at `:7026`, called only at `:6291` under
    `cfg(any(debug_assertions, feature = "check_invariants"))`;
  - zone-NaN check skipped in warm-up (`:6280-6288`);
  - `pyproject.toml` `[tool.maturin]` with no features.

  The release `check_invariants` line passes at HEAD (6991/6992; the failure
  is the submodule artefact).
- **BR-6.** Every factual claim holds:
  - The six advisories and their IDs match `cargo deny check advisories`.
  - Patched ranges from the local advisory-db: pyo3 ≥ 0.29.0, quick-xml ≥
    0.41.0.
  - quick-xml 0.42.0 exists, with `rust-version` 1.86.
  - nalgebra 0.35.0 declares `rust-version` 1.89.0 and depends on simba ^0.10,
    which has no `paste`.
  - parquet 59.3.0 has no `paste`.
  - pyo3-arrow 0.19.0 requires arrow-* 59, pyo3 0.29 and numpy 0.29.
  - postcard 1.1.3 `default = ["heapless-cas"]`. HARES uses only
    `to_allocvec[_crc]` and `from_bytes[_crc]`.
  - The manifest line numbers are correct (`Cargo.toml:29,46,47,48,49,60,68,69`;
    `hares-python/Cargo.toml:19`).
  - The trial upgrade resolves, and `cargo tree -i` shows `paste`,
    `atomic-polyfill` and `heapless` absent.
- **BR-7.** The mechanism is correct: `write_output` defaults to true
  (`config.rs:86,206-208`) and output falls back to a CWD-relative path.
  Nextest creates strays under `crates/hares-core/`.
- **BR-8.** The two bare `#[ignore]` sites are print-only diagnostics, so
  keeping them ignored is correct.
- **M-1 premise.** `tests/fixtures/golden/` holds only `.gitkeep`, and no
  capture/compare tool exists.
- **M-3 citations.** The telemetry `clone_from`/insert sites at `mod.rs:4924`
  and `:6436` are correct. `record_batches_to_polars_df` exists at
  `crates/hares-python/src/conversions.rs:52`. `perf` is installed.
- **PR-1.** `water_heater` has no write site in `dwelling/mod.rs`. The envelope
  timer pattern at `:5997-5998` and `:6155-6163` is as described.
- **PR-2.** `current_process_hwm_kb` is at `synthetic.rs:2110-2121`, with 4
  per-step call sites at `mod.rs:5627,5965,6163,6496`.
- **PR-3.** The stub is at `synthetic.rs:2127-2129`.
- **PR-4.** It depends on OR-6 and is ordered after it (row 29 > row 24).
- **plan.md §2.1.** `hares-core` depends on `hares-io`.
  `DwellingBlueprint::build(self)` consumes the blueprint (`blueprint.rs:225`),
  and `from_parts` mutates `weather.meta` (`blueprint.rs:80-84`).
- **plan.md §2.2.** `record_step` starts at `mod.rs:6606`.
- **plan.md §2.5.** The Utf8 `Time` field is at `columns.rs:425`, formatted
  with `"%+"` at `mod.rs:7009-7010`, with `to_string()` at `writer.rs:379` and
  RFC 3339 `Z` at `aggregation.rs:636`.
- **plan.md §2.7.** `time_res_secs_u32` `unwrap_or(3600)` is at
  `config.rs:139-141`.
- **plan.md §3.**
  - `cargo fmt --check` and `cargo bench --no-run` pass at HEAD.
  - Workspace-wide `-F observe`, `-F dst` and `-F check_invariants` are
    accepted by cargo.
  - The consumer start commit `c71c542` exists and is the consumer HEAD.
  - `i08-consumer` does not exist yet, so creating it is non-destructive.
  - The operator checkpoint list covers every destructive step in my scope's
    entries, apart from the incomplete BR-7 file list (M5).
