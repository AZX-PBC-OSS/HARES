# Verification L2-a: plan.md §1, §3, §5; 00-schedule (to B0); 01-baseline-repair; 02-measurement; 07-profiler

Scope: `plan.md` §1, §3 and §5; `ledger/00-schedule.md` preamble and rows 0
to 0d; `ledger/01-baseline-repair.md`; `ledger/02-measurement.md`;
`ledger/07-profiler.md`; and every `verify-L1-a.md` finding that
`L1-resolution-a.md` marks fixed. Binding inputs: `decision-log.md` and
`plan.md` §2.

Method:

- Code claims read at `ebdb1a17` (`git show`). Consumer claims read at `c71c542`.
- PR #27 diff taken as `git diff ebdb1a17...origin/fix/data-host-renamed-nlr`.
  It applies cleanly to `ebdb1a17`, which gives the Task 0 head tree.
- Builds and tests ran only in an isolated `git archive HEAD` copy,
  with PR #27 applied and the `vendors/OCHRE` submodule contents
  copied in. The copy was `git init`-ed so that `[stray]` can run.
- Tools used: cargo 1.94.1, cargo-nextest 0.9.137, cargo-deny 0.20.2,
  uv 0.12.8, maturin 1.15.0 (project venv).
- Runs in the copy at the Task 0 head:
  - `cargo fmt --check --all`: exit 0.
  - `cargo check -p hares-core --tests -F observe,profiling`: E0425, exit 101.
  - `cargo nextest run --workspace --no-fail-fast -F profiling`: exit 0, 7037 passed.
  - `[python-build]` exactly as written: exit 0.
  - `[pytest]` exactly as written: exit 0, 1377 passed, 9 skipped.
  - `[stray]` around those runs.
  - After BR-1 was applied in the copy:
    `cargo clippy --workspace --all-targets --all-features --keep-going -- -D warnings`.
  - Python probes of the M-2 configurations and of the mixed-offset fleet.
- crates.io and PyPI versions were queried live.

The plan files changed while this check ran: `plan.md`, `00-schedule.md`,
`02-measurement.md` and `decision-log.md` all have mtime 11:03-11:04 today.
Everything in scope was re-read after that change. The findings below are
against the current text.

Counts: **2 blockers, 10 majors, 22 minors.**

---

## Blockers

### 1. BR-2 cannot turn `[clippy]` green at the Task 0 head: PR #27 adds two lints BR-2 does not list

- **Entry.** `01` BR-2 (and `plan.md` §3 / `00-schedule` "BR-2 `[clippy]`").
- **Evidence.** In the Task 0 copy, after BR-1, `cargo clippy --workspace --all-targets --all-features --keep-going -- -D warnings` reports:
  - the five BR-2 sites;
  - the BR-3 `cannot test inner items` error;
  - `error: writing &PathBuf instead of &Path` at
    `crates/hares-core/examples/profile_sim.rs:20:31` and `:20:50`.

  `profile_sim.rs` is added by PR #27. BR-2 says "`[clippy]` still fails on
  five lints" and its change touches only those five, so after BR-2 lands
  `[clippy]` is still red. PR-1 deletes the example, but only at row 14.
- **Correction.**
  - Add `crates/hares-core/examples/profile_sim.rs:20` (`ptr_arg`, two
    parameters) to BR-2's evidence and change: "take `&Path`".
  - Change "five lints" to "seven lints (five at `ebdb1a17`, two added by PR #27)".
  - Keep PR-1's deletion as is.

### 2. PR-6's acceptance grep cannot pass at row 18

- **Entry.** `07` PR-6, acceptance "`git grep -n 'actor_profiling' -- '*.rs' '*.toml' '*.md' '*.py' '*.pyi'` returns nothing".
- **Evidence.** At `ebdb1a17` the grep also matches:
  - `docs/reviews/infrastructure/infra-04-ci-no-test-execution.md:115`;
  - `docs/reviews/py-binding/py-binding-08-py-actor-binding.md:87`.

  Those files are planning records. `10` SL-8 moves them out of the tree at
  schedule row 25, after PR-6 at row 18. Editing them would rewrite review
  history, and PR-6's change does not touch them. The acceptance therefore
  fails as written.
- **Correction.** Change the acceptance to
  `git grep -n 'actor_profiling' -- '*.rs' '*.toml' '*.md' '*.py' '*.pyi' ':(exclude)docs/reviews'`
  returns nothing. Alternatively, move SL-8 before PR-6 in `00-schedule.md`.

---

## Majors

### 3. BR-7's change leaves the `dwelling_*_diagnostics.csv` strays, so `[stray]` stays red

- **Entry.** `01` BR-7.
- **Evidence.**
  - The diagnostics writer is created whenever `output_verbosity >= 4`,
    independent of `write_output` (`crates/hares-core/src/dwelling/mod.rs:2788-2800`).
  - Its path is derived from `output_path` or `default_output_path`
    (`mod.rs:2470-2474`, `conversions.rs:590-596`), so it lands in the
    working directory.
  - `crates/hares-core/tests/ev_thermal_regressions.rs:467-469` already sets
    `write_output: false, output_path: None, output_verbosity: 5` with
    `bldg_id: 100`. The copy's nextest run recreated
    `crates/hares-core/dwelling_100_diagnostics.csv`.
  - Python tests at verbosity 5 and 6 recreate the root
    `dwelling_42_diagnostics.csv`: `tests/python/test_actor_param_boundary.py:58,106`,
    `test_observer_diagnostics.py:68`, `test_py_metrics.py:139` and
    `test_ochre_parity.py:96,134,319`.
  - BR-7's rule ("passes `write_output: false`") does not stop either file,
    so BR-7's acceptance (an empty `[stray]` comparison) fails.
- **Correction.** Rewrite the change as: "every test, helper and example that
  builds a dwelling either passes an `output_path` inside a
  `tempfile`/`tmp_path` directory, or runs at `output_verbosity < 4` with
  `write_output: false`; the diagnostics file follows `output_path`
  (`mod.rs:2788-2800`)." Add both cited sites to the evidence.

### 4. BR-4's CI changes (and those of BR-6b and BR-7) never run on this initiative's branches

- **Entry.** `01` BR-4, and the CI parts of BR-6b and BR-7.
- **Evidence.**
  - `.github/workflows/ci.yml:3-8` triggers only on `push` to `main` and
    `pull_request` into `main`.
  - PRs #24 and #25 (base `initial-implementation`) have no CI runs.
  - PR #27's runs all predate its base change to `initial-implementation`
    (`base_ref_changed` at 2026-09-24T16:39:25Z; last run triggered
    16:39:28Z by the earlier push).
  - No workflow therefore runs on `feat/output-results` or on the closing
    pull request into `initial-implementation`.
  - BR-4's problem ("a regression ... reaches `initial-implementation`
    unseen") is real, but its change does not address it.
- **Correction.**
  - BR-4 also adds `initial-implementation` to both branch filters, plus
    `feat/output-results` to `push`.
  - Its acceptance adds "the workflow's `on:` lists `initial-implementation`
    for `push` and `pull_request`".
  - Alternatively, state explicitly that CI is not a gate for I-08, and drop
    the CI parts of BR-4, BR-6b and BR-7.

### 5. PR-1 puts actor `decide()` time in `control`; PR-6 says it is in `actor_dispatch`

- **Entry.** `07` PR-1 and PR-6; `plan.md` §2.9 field list.
- **Evidence.**
  - Every actor's `decide()` runs in "Steps 1e-1f: phase-ordered actor
    execution" (`mod.rs:5440-5594`): solver feedback `decide` at `:5480`, and
    `ExecutionPhase::ActorDecide` at `:5523`, with the `actor_profiling`
    timer at `:5520-5537`.
  - PR-1 assigns Steps 1b-1f to `control`, and Steps 2 and 2b
    (`:5595-5692`, dispatch of queued signals) to `actor_dispatch`.
  - PR-6 states "The reads around `decide()` stay inside PR-1's
    `actor_dispatch` phase", which is false under PR-1's own boundaries.
  - Step 2a (`:5693`) runs after Steps 2 and 2b, so `control` is also
    non-contiguous. PR-1 does not say that its phase clock closes `control`
    twice.
  - As specified, the phase HARES#31 cares most about (actor cost) is
    hidden inside `control`.
- **Correction.** Define the phases in code order in PR-1 and §2.9:
  - `environment`: entry to Step 1a';
  - `control`: Steps 1b-1d plus Step 2a, stated as two spans;
  - `actors`: the Steps 1e-1f scheduler loop, including solver feedback;
  - `dispatch`: Steps 2 and 2b;
  - then `equipment`, `envelope`, `invariants`, `state_snapshot`, `output`,
    `accounting`.

  Rename the field from `actor_dispatch` accordingly, and make PR-6 say
  the `decide()` reads lie inside `actors`.

### 6. M-1 does not say what `initialization_duration_s = 0` means, and the engine's own convention runs a warm-up

- **Entry.** `02` M-1 (manifest) and M-2 ("`initialization_duration_s = 0`", "`consumer_shape_900s` mirrors the consumer's call ... no warm-up").
- **Evidence.**
  - At HEAD any `Some(_)` runs the converging warm-up and ignores the value:
    `mod.rs:2725` `if let Some(_init_dur)` leads to
    `run_warmup_converged(0.5, 25)`.
  - The engine's TOML path maps `initialization_duration_s` with
    `.map(StdDuration::from_secs)` (`mod.rs:1892-1894`, `:2729-2732`), so
    `0` becomes `Some(0 s)` and a warm-up runs.
  - Python maps `0` to `None`, which means no warm-up
    (`crates/hares-python/src/py_dwelling.rs:2433-2439`, the path the
    consumer uses).
  - The adapter implementer must choose. The mirror of the engine's own
    TOML key would warm up and break the consumer-shape claim.
- **Correction.** In M-1's manifest: "`initialization_duration_s` absent or
  `0` builds `DwellingConfig.initialization_duration = None` (no warm-up,
  as the Python binding does); a positive value builds `Some`". Add the test
  `manifest_zero_initialization_duration_runs_no_warmup`, which asserts
  `health().warmup` is `WarmupOutcome::Disabled`.

### 7. M-1's fleet manifest and fleet products are under-specified at HEAD

- **Entry.** `02` M-1 (manifest, "for a fleet the aggregate and its covered weights; `homes`", adapter) and M-2 `fleet_mixed_offset` ("weight 1.0").
- **Evidence.**
  - The manifest has no `weight` key, and unknown keys are rejected.
  - HEAD's `aggregate(results, resolution: AggregationResolution)` requires
    `FifteenMin` or `Hourly` (`crates/hares-fleet/src/aggregation.rs:15-18, 248-251`),
    and the manifest has no key for it.
  - "Covered weights" do not exist at HEAD. `FleetResults` is
    `per_dwelling_metrics` plus `aggregate_timeseries`
    (`aggregation.rs:32-35`); covered weights arrive with `04` FL-1.
  - The adapter must invent all three.
- **Correction.** Add `weight` to `[[home]]` and a fleet-only
  `resolution = "hourly" | "fifteen_min"` key, which FL-1 migrates to
  `FleetReport.frequency`. Define the HEAD products:
  - `aggregate` = `aggregate_timeseries`;
  - `homes` = `per_dwelling_metrics` (`bldg_id`, weight, status, reason);
  - the covered-weight product is added by FL-1's adapter update, following
    the schema change rule.

### 8. `fleet_mixed_offset` pins an empty aggregate at HEAD, and FL-2 changes it under a "golden" check

- **Entry.** `02` M-2 (`fleet_mixed_offset` and its note "its aggregate row count is the one today's fleet path produces ... FL-4 changes it").
- **Evidence.**
  - A probe on the Task 0 copy built the fleet as specified (the three homes
    and their `_2018.csv`, 7 days at 3600 s, start `-06:00`, one thread,
    `Hourly`). The aggregate had **0 rows and 1 column**, with the warning
    "Fleet aggregation produced an empty timeseries despite successful
    dwellings".
  - Each home alone gives 168 rows (35, 34 and 39 columns). Two copies of
    `bldg0000002` give 168 rows.
  - The cause is `04-fleet.md`'s own "any column difference between homes
    empties the whole aggregate" (`aggregation.rs:572-584`).
  - So the fixture pins nothing about offsets or alignment.
  - `04` FL-2 (schedule row 51, consumer check "golden") makes the aggregate
    non-empty. That changes this golden in a row whose check requires it to
    stay unchanged, and no row re-captures it.
- **Correction.**
  - State in M-2 that the HEAD aggregate is empty because the column sets
    differ.
  - Name FL-2 as the entry that re-captures `fleet_mixed_offset` under the
    schema change rule (delta recorded), and change FL-2's consumer check to
    "golden, except `fleet_mixed_offset` re-captured".
  - Optionally add a second fleet fixture of homes with identical column
    sets across offsets, so that alignment is exercised before FL-2.

### 9. No golden carries billing, but B-4 depends on one

- **Entry.** `02` M-1 and M-2.
- **Evidence.**
  - M-1 lists "`billing` when a tariff is configured", but the manifest has
    no tariff key.
  - A tariff is attached through `Dwelling::set_tariff(ElectricTariff, Tz)`
    (`mod.rs:4123`), not through `SimulationConfig`.
  - No M-2 fixture has one.
  - `09` B-4 lists "Dependencies: M-1, M-2 (a golden with billing summaries
    spanning a month boundary)", and its measurement is "per-period billing
    delta on the month-boundary golden". That golden does not exist.
- **Correction.**
  - Add a `[tariff]` manifest table (tariff definition file,
    repository-relative, and IANA zone).
  - Give M-2 a fixture with a flat tariff that spans a month boundary: add
    it to `consumer_shape_900s` or `full_year_hourly`, or add a
    `month_boundary_billing_3600s` row.
  - Update the M-2 table and commit message.

### 10. M-1's defaults digest ("every defaults file the run read ... in load order") cannot be computed at HEAD

- **Entry.** `02` M-1 (committed golden).
- **Evidence.**
  - Nothing at HEAD records which defaults files a run reads, or in what
    order. `crates/hares-io/src/defaults.rs` reads through `read_dir` and
    `read_to_string` (`:732, 782-795, 821-834, 860, 1027, 2206-2217, 2286`)
    and keeps no record.
  - The ordered record is `InputRecords.defaults_files_sha256`, built by
    `12` IN-1 at row 63.
  - The adapter would have to invent it, for example by instrumenting
    `hares-io` at M-1.
- **Correction.** At M-1, digest every regular file under the resolved
  defaults directory: sorted relative path, `0x00`, bytes, following the
  `[defaults_files]` symlinks. When IN-1 lands, the adapter switches to
  `defaults_files_sha256` and re-captures that field. State whether
  `compare` fails on a defaults-digest mismatch (see minor 7).

### 11. BR-8 edits a test file that is never compiled; seven shadowed test files are untracked, and the count claim is wrong

- **Entry.** `01` BR-8.
- **Evidence.**
  - `crates/hares-core/Cargo.toml` declares
    `[[test]] name = "resstock_smoke" path = "../../tests/resstock_smoke.rs"`.
    `cargo metadata` lists `tests/resstock_smoke.rs` as the only
    `resstock_smoke` target.
  - So `crates/hares-core/tests/resstock_smoke.rs` (498 lines) is never
    compiled. That includes the ignored test BR-8 deletes (`:299`) and
    six non-ignored tests that never run: `resstock_2025_1_summer_72h`,
    `resstock_2025_1_winter_72h`, `resstock_2024_2_summer_72h`,
    `resstock_2024_2_winter_72h`, and others at `:473-495`.
  - Six more files there are dead two-line `#[path]` shims, shadowed by the
    same explicit targets: `parity.rs`, `bestest.rs`, `conditioned_oracle.rs`,
    `freefloat_oracle.rs`, `structural_envelope_oracle.rs`,
    `warmup_regression.rs`.
  - No ledger entry tracks any of this.
  - The bestest module compiles into both the `bestest` and `regression`
    targets (`cargo nextest list --run-ignored only` shows each bestest
    helper twice). The ignored set is therefore 12 test instances in the
    default build and 16 under `--all-features`, and the drop is not
    "eleven".
- **Correction.**
  - Extend BR-8, or add a BR entry before M-1: delete the six shims; for
    `crates/hares-core/tests/resstock_smoke.rs`, move its unique seasonal
    72 h tests into the compiled `tests/resstock_smoke.rs` or delete them
    with the reason recorded, then delete the file.
  - Acceptance: every `crates/*/tests/*.rs` is the `src_path` of a test
    target in `cargo metadata --format-version 1 --no-deps`.
  - Restate the count: "the ignored instances drop from 12 (default) and 16
    (`--all-features`) to 0".

### 12. PR-6's allocation acceptance test cannot isolate what it claims to measure

- **Entry.** `07` PR-6, `profiled_actor_timing_allocates_nothing_per_step` ("adding the per-actor accumulation adds no allocation to any step after the first").
- **Evidence.**
  - At row 18, steps still allocate: the telemetry `clone_from` and
    `insert` at `mod.rs:6436` and `:4924`. RT-1 removes them at row 22.
  - The accumulation is compiled in whenever `profiling` is, so there is no
    without-accumulation run to compare against.
  - PR-3's per-step counters include every allocation of the step.
  - No deterministic test matches the description.
- **Correction.** Test the accumulator in isolation. Make the per-actor
  store a small type (for example `ActorTimings`) with a unit test in
  `profiling_alloc.rs`: after construction for N actors, 96 × N `record`
  calls give `thread_allocations()` delta `Some(0)`. Alternatively, assert
  that the `actors` buffer pointer and capacity are unchanged across 96
  steps. Leave whole-step `Some(0)` to RT-1 and RT-3.

---

## Minors

1. **`plan.md` §3 / `00-schedule` preamble: the list of failing lines at the Task 0 head is incomplete.**
   `[test-observe-profiling]` also fails. `cargo check -p hares-core --tests -F observe,profiling`
   gives E0425 in `hares-equipment`, because `hares-core`'s `observe`
   enables `hares-equipment/observe` (`crates/hares-core/Cargo.toml:11`).
   *Correction:* add it to both lists, and to BR-1's "turns green".
2. **`00-schedule` preamble: the row range is off by one.** It says "rows 1 to 10 ... in this order: ... BR-6b `[advisories]`", but BR-6b is row 11.
   *Correction:* "rows 1 to 11".
3. **`02` M-1 evidence: wrong `.gitignore` line.** It now cites `.gitignore:50`; `*.parquet` is line 49 and line 50 is a comment.
   *Correction:* `:49`.
4. **`02` M-1: an unspecified gap in the defaults handling.** "Until OR-3 ... symlinks; from RT-4 ... `DefaultsSource.files`" says nothing about rows 38-61.
   *Correction:* "until RT-4".
5. **`02` M-1: the metrics flattening is unspecified.** The metrics product is "every field ... as its `f64` bit pattern". `FullSimulationMetrics` has nested structs, `Option`s, enums (`SimulationCoverage`, `Reliability`) and `u64`, and no `Serialize` (`crates/hares-io/src/output/metrics.rs:170-243`).
   *Correction:* specify dotted field paths, `None` as `null`, enums by variant name, and integers in decimal.
6. **`02` M-1: the `health` product layout is unspecified.** `RunHealth` holds counters plus the `WarmupOutcome` enum.
   *Correction:* a one-row table with flattened fields, using the same rules as minor 5.
7. **`02` M-1: behaviour on a defaults mismatch is unspecified.** It does not say whether `compare` fails when the recorded defaults digest differs from the current tree's.
   *Correction:* state that it does.
8. **`02` M-1: "unknown keys rejected" does not hold inside `[simulation]`.** The table is "a `SimulationConfig` table, verbatim", and `SimulationConfig` has no `deny_unknown_fields` until `11` OD-2 (`crates/hares-io/src/config.rs:60-61`).
   *Correction:* state that `[simulation]` strictness arrives with OD-2.
9. **`02` M-2: the EPW description is inaccurate.** It says parity EPWs are "TMY rows labelled 1995". The cz4a file mixes source years 1991-2005 (1991 ×2232, 1994 ×2136, 1995 ×744, ...). The non-leap claim holds: there are no Feb 29 rows.
   *Correction:* "TMY3 rows from several source years, none a Feb 29".
10. **`02` M-3 item 4: the defaults replacement is not provided.** It builds the Python dwelling "with `consumer_shape_900s`'s inputs and settings", but the replacement defaults file needs the symlinked defaults directory.
    *Correction:* say how it is made, for example by a `frame_golden` subcommand that prints a prepared defaults directory, or by building it in the script.
11. **`02` M-3 item 1: the wrong writer is measured.** "File written" is measured through `frame_golden run --output-dir`, which is the tool's Parquet writer, not the engine's. The engine's writer is the cost HARES#28 is about.
    *Correction:* measure `write_output: true` with an engine `output_path`.
12. **`07` PR-3: the rayon bench would under-count silently.** `benches/rl_episode.rs:279-301` counts allocations around `dwellings.par_iter_mut()`. A per-thread counter misses the rayon workers, so the printed "allocs/episode" becomes silently wrong.
    *Correction:* the bench keeps a process-wide count, or steps sequentially for the count.
13. **`07` PR-3: before and after timings run on different binaries.** `frame_golden` installs the counting allocator from row 16 on, but the M-3 wall times were taken without it.
    *Correction:* install it only under `-F profiling`, or record the binary difference in the measurement rule.
14. **`07` PR-2 and PR-4: test files are not named.** Their acceptance tests have no file.
    *Correction:* name `crates/hares-core/tests/profiling.rs`.
15. **`07` PR-5: the stub does not exist yet.** It says "`_hares.pyi` states the exception", but `python/ochre_next/_hares.pyi` has no `profiling_summary` stub at the Task 0 head.
    *Correction:* PR-1 or PR-5 adds the method stub with its keys and return type.
16. **`07` PR-6: the entry count depends on the fixture.** The test asserts "two `ActorTiming` entries", but `auto_register_actors` prepends built-in actors (`mod.rs:3165-3214`).
    *Correction:* name a fixture with no built-in actors, or assert on the two named entries.
17. **`01` BR-6c: "remove each upper cap" is ambiguous.** It also covers `requires-python = ">=3.12,<3.14"` and `maturin>=1.15.0,<2`.
    *Correction:* name the caps in scope, `pandas<3.0` and `pint<0.25`.
18. **`01` BR-6d evidence: the consumer source is quoted wrongly.** It is `ochre-next = { path = "HARES", editable = true }`.
19. **`01` BR-7: the census method misses TOML-built configs.** "Found by the compiler (the field is typed)" only finds struct literals. Configs built from TOML (`from_toml`, synthetic TOML, parity `config.toml`) default `write_output` to true and are not flagged.
    *Correction:* add a grep over TOML-building call sites.
20. **`01` BR-4: CI still misses block lines.** The title says "CI runs the block's cargo lines", but CI gets neither `[test-observe-profiling]` nor `[golden]`.
    *Correction:* add both, or narrow the title.
21. **`plan.md` §3: tool versions are not recorded.** No toolchain or tool version is recorded or pinned, so `[clippy]` results drift with `stable`.
    *Correction:* M-0 records `rustc -V`, `cargo nextest --version`, `cargo deny --version` and `uv --version` with the reference state.
22. **`plan.md` §3 `[stray]`: `.shipwright/` is not excluded.** Ledger or verification files written during a block run show up as new paths.
    *Correction:* add `':(exclude).shipwright'`.

---

## verify-L1-a findings marked fixed in L1-resolution-a

| ID | Status in the current text |
|---|---|
| B1 | Fixed: OR-5 runs in BR-5's slot (§3 opening sequence, `00-schedule` row 4, BR-5 "Commit: none; OR-5's commit"). |
| B2 | Fixed: order BR-1, BR-3, BR-2; BR-2 depends on BR-1 and BR-3. BR-2's lint list is still incomplete (blocker 1). |
| B3 | Fixed: `[python-build]` and `[pytest]` are valid. Both exit 0 in the Task 0 copy (1377 passed, 9 skipped). |
| B4 | Fixed: PR-3 deletes the `debug_assert_eq!` (`mod.rs:6510-6517`) and uses a per-thread counter. |
| B5 | Fixed in §2.2: `BatchCallback` trait, `Callback` marker variant. |
| M1 | Fixed: §3 Task 0 fast-forward commands are correct. `feat/output-results`, `initial-implementation` and `origin/initial-implementation` are all `ebdb1a17`. PR #27 is OPEN and MERGEABLE. |
| M3 | Fixed: BR-6a (MSRV and let chains) is separate. BR-6b names `build.rs:15` and `building.rs:363-406`. |
| M4 | Fixed: M-1 and M-2 come before BR-6a and BR-6b; BR-6b records a delta per fixture. |
| M5 | Fixed as stated: fourteen files match the Task 0 grep exactly, and the census covers `tests/python`. The diagnostics cause is not addressed (major 3). |
| M6 | Fixed: `[stray]` is in the block with exclusions, and BR-7 adds the CI steps. The exclusions left no false positive in the copy run: only `dwelling_*` paths were new. |
| M7 | Fixed: all eleven functions are deleted, and the acceptance grep and nextest command are valid. The dead-file issue is new (major 11). |
| M8 | Fixed: the manifest format is present. The warm-up semantics are new (major 6). |
| M9 | Partly fixed: fleet kind, `--columns`, schema change rule and `weather_wrap_900s` are present. The fleet manifest and products are under-specified (majors 7 and 8). |
| M10 | Fixed in substance: `[golden]` lines are in the block, `frame_golden` is in `[workspace] members`, and the acceptance tests are named. The names differ from those the resolution quotes (`digest_*`, `compare_*`, `delta_*`), with equivalent coverage. |
| M11 | Fixed: table with inputs, start, duration, step, features and rows. The Python probe on the copy gives 480, 384 and 192 rows for the leap, wrap and consumer configurations. |
| M12 | Fixed: M-3 uses `consumer_shape_900s` through `frame_golden run`, "Commit: none". |
| M13 | Fixed: phase clock, `water_heater`, `hvac` and `other` deleted, `DwellingProfilingSummary` named. Phase attribution is wrong (major 5). |
| M14 | Fixed: `[test-profiling]` is in the block, and BR-4 adds `-F profiling` to CI. |
| M15 | Fixed: a single `VmHWM` read per summary, `Option<u64>`, `None` off Linux. |
| M16 | Fixed: `crates/hares-types/src/alloc_count.rs`, thread-local counter, exact-count test. A release build does not elide `Box::new(black_box(n))`: 7 of 7 counted, checked. |
| M17 | Fixed in §2 (`RunMetrics`, `HealthPolicy::strict()`, `InputSource`, actor identity by name). |
| minors 1-2 | Fixed (BR-1 text). |
| minor 3 | Fixed as worded. The list misses PR #27's two lints (blocker 1). |
| minors 4-6 | Fixed (BR-3: rename, `T-0212` removal, `unnameable_test_items`, lines 607/608, 650-727, 728 correct). |
| minor 7 | Fixed (BR-4 `ci.yml:36` and `--profile ci`; `[profile.ci]` exists in `.config/nextest.toml`). |
| minors 8-9 | Fixed. |
| minors 10-11 | Fixed in §2. |
| minor 12 | Fixed: `mod.rs:6431-6433` is the misleading comment. |
| minor 13 | Fixed: 61 directories, re-measured at execution. |
| minor 14 | Fixed: `[writing]` commands. `grep -P '\x{2014}'` works under the `C.UTF-8` locale in use. `[bench]` is `--workspace`. |
| minor 15 | Fixed: no `review.md` or `plan-critique` citation remains in 01, 02 or 07. |
| minor 16 | Fixed: PR-4 `finish` / `finish_s`; PR-5 raises. |

---

## Verified correct

- **Task 0.**
  - The fourteen stray paths match `git status --porcelain --ignored --untracked-files=all | grep -E ...` exactly.
  - The consumer is on `main` at `c71c542`, the tree is clean and `i08-consumer` does not exist.
  - The consumer gitlink is `0ec757cb`.
  - PR #27 touches exactly the four files §1 names: `profile_sim.rs`, `hares-python/Cargo.toml`, `py_dwelling.rs`, `weather.py`.
  - It inserts 41 lines after `py_dwelling.rs:687` and 1 line after `Cargo.toml:10`.
- **Block commands.** Every block line's flags are accepted by the installed tools:
  - `cargo deny --all-features check advisories` (global `--all-features`);
  - `cargo nextest ... --run-ignored only`;
  - `maturin develop --release --uv`;
  - `uv run --locked --all-extras --all-groups`;
  - `--no-sync`;
  - `-m ""` overrides `addopts`;
  - `-F observe`, `-F dst` and `-F profiling` work workspace-wide even though `hares-control` has no features.
- **Line status at the Task 0 head.**
  - `[fmt]` passes.
  - `[test-profiling]` passes (7037 run, 7037 passed, 12 skipped).
  - `[python-build]` and `[pytest]` pass.
  - `[stray]` fails only on the `dwelling_*` paths.
- **BR-1.** Evidence exact (`battery/mod.rs:1457-1459, 1464-1472, 1494`; `degradation.rs:144` `pub(crate)`). The fix compiles in the copy, and with it `[test-observe-profiling]` passes: 1322 run, 1322 passed, 15 skipped.
- **BR-2.** The five cited sites and lint names are exact.
- **BR-3.** Lines and names are exact; the acceptance filter is valid.
- **BR-4 evidence.** `ci.yml:36` and `:135-137`, and `crates/hares-core/Cargo.toml:14` (`dst`), are exact.
- **BR-5.** Citations hold:
  - `mod.rs:7025` / `:6291`, `:658-662`, `:15224-15226`, `:376`, `:3794`, `:3167`, `:3574`, `:3605`;
  - `equipment_id_regressions.rs:2475, 2517`;
  - `invariants.rs` stubs at 704-788 and module doc 1-9.
- **BR-6a and BR-6b.**
  - Every "target" version in the table is the current crates.io maximum stable release: pyo3 and pyo3-build-config 0.29.2, numpy 0.29.0, arrow and parquet 60.0.0 (59 per pyo3-arrow 0.19.0), nalgebra 0.35.0, quick-xml 0.42.0, tzf-rs 2.1.2, criterion 0.8.2, strum 0.28.0, sha2 0.11.0, syn 3.0.6.
  - Every other direct dependency is on its latest major or minor, with only prereleases above (`libc` 1.0.0-alpha.4, `approx` 0.6.0-rc2).
  - Stated MSRVs are exact: nalgebra 1.89.0, tzf-rs 1.88, quick-xml and criterion 1.86. No target needs more than 1.89.
  - `Cargo.toml:29` is `1.87`.
- **BR-6c.** maturin 1.15.0 is the latest release; the `pyproject.toml` bounds and caps are as cited.
- **BR-6d.** Consumer bounds are as cited.
- **BR-7 evidence.**
  - `config.rs:86-87, 206-208`;
  - `py_config.rs:201`;
  - `py_dwelling.rs:2320-2325`;
  - `conftest.py:30-40` (`bldg_id=42`);
  - `benches/common.rs:126-127` (temp path);
  - `conversions.rs:590-596`.
- **BR-8.** The eleven functions and their lines are exact; `solar_parity.rs:187,264` are stale comments.
- **M-0.** Consumer report claims are exact: `docs/Test_Report.md:3` "325 passed, 3 failed, 7 skipped"; `Red_Team_Record.md:144-155` "329 passed, 0 failed, 7 skipped, 336 total".
- **M-2 inputs.**
  - Every input path exists.
  - bldg0176227 has a gas storage water heater in the garage, so the `Gas Water Heater` / `ua_w_per_k` override is live per the consumer registry.
  - Consumer citations hold: `engine.py` `build_kwargs` (verbosity 2, no `initialization_duration`), `grid.py:158` `STUDY_GRID`, `RESAMPLE_OVERRIDES = None`, `inject.py:148-153`, `registry.py:161-172, 202-203`, `defaults.py` columns 10 and 11.
  - The column names exist in `defaults/envelope/Envelope Materials.csv`, and the file has no em dash or `[writing]` token.
  - `clock.rs:27-56` is fixed-offset.
  - The parity EPWs have 8760 rows and no Feb 29.
- **M-3.** `perf` is installed. The Python APIs it names exist: `results`, `save_state`, `load_state`, `duration_s`, `output_to_parquet`, `write_output`.
- **07 evidence.**
  - `DwellingProfilingSummary` at `mod.rs:242-253`.
  - Profiling timers at `:5290-5317, 5619-5628, 5702-5703, 5957-5966, 5997-5998, 6155-6163, 6483-6496, 6499-6517`.
  - `synthetic.rs:2110-2121` and `:2127-2129`.
  - `engine.rs:365-391`.
  - Actor timing at `:1703-1706, 5462-5466, 5520-5537, 4089-4105`.
  - `Cargo.toml:10`.
  - `docs/development.md:239-261`.
  - The three counting-allocator copies are at the cited lines.
  - `profile_sim.rs` runs bldg0000007 against `G0100590_2018.csv` at `-07:00`.
  - `add_actor` calls `refresh_equipment_caches`.
  - `hares-python` has in-crate `Python::attach` tests, so PR-6's Rust test is feasible.
- **PR-1 acceptance.** Exact `Duration` equality of the partition is deterministic.
- **Decision-log consistency.** The entries are consistent with `dependencies`, `profiler_scope`, `golden_storage`, `stray_outputs` and `branch`. §5 "No open operator decisions" is consistent: the two L1 operator questions are answered by `profiler_scope` and `golden_storage`.
- **Operator checkpoints.** They cover every destructive or public step in scope: PR #27 merge, `i08-consumer` creation, stray removal, engine-under-test worktree creation for M-0, pushes, and added consumer dependencies.

---

## Outside this scope, noted for the integrator

These were observed while re-reading the edited files:

- `decision-log.md` now contains a `dl-i08-process.readiness` section with
  "verdict: ledger cleared for execution" and "Zero blockers, zero majors
  open". The blockers and majors above contradict that verdict, and the
  section contains an em dash.
- `plan.md:256` and `:582` (§2) gained em dashes, contrary to §3
  **Writing**.
- `12-inputs.md:382-387` plans changes to
  `crates/hares-core/tests/resstock_smoke.rs`, which is never compiled
  (major 11).
