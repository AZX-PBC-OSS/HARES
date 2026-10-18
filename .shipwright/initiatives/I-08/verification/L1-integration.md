# L1 integration

Inputs: `decision-log.md` (all entries, including `metrics_optional`,
`weather_edge_cases`, `ochre_compat`, `defaults_path`,
`calibration_r_factor`, `consumer_gate`, `profiler_scope`,
`golden_storage`, `setpoint_sources`, `battery_control`,
`rl_observation_allocation`, `cross_platform_measurement`,
`consumer_inert_override_probe`, `input_schema_scope`,
`public_planning_docs`, `slop_scope`, `dependencies`), `plan.md`, every
`ledger/*.md`, and `verification/L1-resolution-a.md` to `-d.md`.

Code claims added in this pass were read at HARES `ebdb1a17`, the consumer
at `c71c542`, `vendors/EnergyPlus` and `vendors/OCHRE`, OpenStudio-HPXML
master `57bd1b88` on GitHub, and the maturin documentation. Nothing in
either code repository was written; the `git mv` behaviour was measured in
a scratch repository.

## One definition per contract

| Contract | Owner | Cited by |
|---|---|---|
| Content inputs, parsed handles, `DwellingBlueprint` | `plan.md` §2.7 (types); RT-4 (implementation) | IN-1, IN-2, CN-10, PY-2, B-10 |
| `ResolvedModel` | `plan.md` §2.7 (types, required-parameter table); RT-6 | IN-2, IN-5, IN-7, CN-3 |
| Per-run statics | RT-5 table; OR-5 owns the three health counters | `plan.md` §2.3 |
| `RunHealth`, `WarmupOutcome` | `plan.md` §2.3; OR-5 | B-2, FL-2, CN-1, CN-2 |
| `HealthPolicy` | `plan.md` §2.3; OR-12 | FL-2, CN-2, PY-9 |
| Metrics on request | `plan.md` §2.2; OR-8 | B-7, OR-10, OR-15, CN-7, FL-1, FL-2, PY-9 |
| Engine under test, consumer gate, B0, pin | `08-consumer.md` | `plan.md` §3, BR-6d, M-0 |
| Weather rules (EnergyPlus) | `09-numeric-fixes.md` "Weather: what EnergyPlus specifies" | `plan.md` §2.7 |
| Golden format | `02-measurement.md` M-1 | `plan.md` §3, every entry that compares |

## Changes by file

### plan.md

- §1: area list rewritten (02 adds digests and cross-platform; 03 adds
  invariants, reporting frequency, callback, metrics on request,
  `metrics_from_table`, replay; 05 package rename and one config type; 06
  content inputs, resolved read-back, RL observation; 07 HARES#31 in full;
  08 gate repairs; 10 public-tree move and no global sweep; new
  `ledger/12-inputs.md`). Added the one-definition rule.
- §2.1: `RunResult.metrics` is `Option<RunMetrics>`; checkpoint metrics
  section present exactly when requested, mismatch a typed error.
- §2.2: `VariableDef.nullable` semantics; `Enum` vs `State`; metrics
  computed only when `SimulationConfig.metrics` is true (default `false`),
  owner OR-8; `MetricsError` named; fleet duplicate-path rule cites FL-1.
- §2.3: `WarmupOutcome` rewritten as `Disabled | Ran { days_run,
  converged, residuals }` with `WarmupResiduals` (OR-5 records, B-2
  decides); `HealthPolicy::default()` is the empty policy; dwelling counts
  rejected signals and clamps, no process-global counter; `Warning`
  contents; non-finite latch replaces `NON_FINITE_REJECTIONS`; owners.
- §2.4: fold order is entry order; `FleetReport`, `Alignment`,
  `FleetResult.covered_weight` and `peak_live_home_results`, `FleetError`
  variants; `Degraded` rule stated on health events.
- §2.7: removed-key rows for `initialization_duration` and
  `resample_overrides`; `SimulationConfig` field list adds `metrics`,
  `warmup`, `first_hour_interpolation`, `weather_year`; `Warmup`,
  `WarmupConfig`, `FirstHour`, `WeatherYear`, `RecordLabel` defined;
  `DefaultsSource.base` note; `InputRecords.schedule` optional;
  `DwellingConfig.schedule` optional; `ForcingKey`/`ForcingTable` in
  `hares-io`; ownership sentence; `ResolvedBoundary` gains `films_m2k_w`
  and `layers: Vec<ResolvedLayer>`; `InputSchema.hpxml_elements`,
  `HpxmlElementSchema`, `HpxmlElement`, `HPXML_ELEMENTS`; reserved
  `envelope` override key; dependency paragraph rewritten (resolution a
  contract 1); reproducibility cites RT-5, FL-1, M-4a, M-4b.
- §2.8: B-1a/B-1b/B-1c split named; B-10 names the packaged Python
  defaults; B-13 rule and the envelope-scale route stated.
- §2.9: coupled factorization removed (SL-7), RL observation in scope;
  `ObservationPlan`, `observe_into`, `ActorTiming`, the full
  `DwellingProfilingSummary` field set, `CountingAllocator`,
  `thread_allocations` defined.
- §2.10: the `SteppableFleet` rewind seam no longer cites a
  `load_state` that does not exist; the seam is the per-home checkpoint
  (OR-10) plus FL-1's fold state.
- §3: opening sequence adds CN-0, CN-16, B0 and M-3; block adds
  `[test-observe-profiling]`, marks `[test-release-invariants]` deleted by
  OR-11, and the `[writing]` pattern gains `IN` and SL-2's reference
  words; notes on the new lines and on focused Verification commands;
  "whole block from BR-6b on"; golden paragraph rewritten for digests;
  consumer mechanism, gate and pin now cite `08-consumer.md` (the measured
  `uv pip install --no-deps` mechanism replaces `maturin develop`);
  closing sequence M-4a, push, M-4b, push, CN-15; operator checkpoints add
  both pushes, the worktrees and the cache move.
- §5: unchanged, "No open operator decisions".

### ledger/00-schedule.md

Rewritten: rows 0 to 101, with lettered rows for steps without a commit
and for IN-7 and OD-9. Every entry appears
once; BR-5 rides OR-5's row, SL-1/SL-4/SL-5/SL-6 ride RT-1/CN-6/CN-5/CN-9,
SL-2 and SL-3 are standing rules. B0 is captured on the Task 0 head after
CN-0 and CN-16. A "Consumer check" column states per row whether the B0
check, the consumer-shaped golden, the consumer gate or a recorded delta
covers it. The block-green rule states which opening row turns which line
green and that the whole block is green from BR-6b on. The contingent
split rule for a dependency upgrade that fails its B0 check is stated.

### ledger/01-baseline-repair.md

- Intro: whole block from BR-6b on.
- BR-6b: materialize before `delta`; B0 check and consumer-shaped golden
  with the split rule.
- BR-6d: B0 captured before it, so its acceptance is the gate plus the B0
  check; split rule; `Engine:` trailer; depends on CN-0 and B0.

### ledger/02-measurement.md

- M-0 item 3: the consumer base is recorded through `08-consumer.md`; the
  committed Test_Report is stale (pin `7a3366a7`), the consumer's record of
  `0ec757cb` reads 329 passed, 0 failed, 7 skipped.
- M-1 rewritten for `golden_storage`: committed `<name>.golden.json` with
  per-column and per-96-row-block SHA-256 digests plus metrics in full,
  full products only under `target/golden/`; `materialize`,
  `materialize-all`, `diff`; `[defaults_files]` manifest table; defaults
  recorded as repository-relative path plus digest (no absolute path in
  the public repository); numeric entry procedure; tests rewritten for
  digests.
- M-2: `consumer_shape_900s` gains the consumer's defaults variant
  (resistance x1.3, capacitance x1.5); no frame committed.
- M-4a and M-4b (new): GitHub Actions `ubuntu-24.04` and `macos-15`
  (arm64) workflow and `docs/reproducibility.md`.

### ledger/03-output-reporting.md

- Intro order: OR-5 in the opening sequence, OR-11 after M-3.
- OR-5: `WarmupResiduals` recorded; statics named with lines; `check_soc`
  classified as vacuous and deleted (its input is a bounded `Soc`); the
  drift-error signature ripple through `refresh_equipment_caches` and its
  callers; `NON_FINITE_REJECTIONS` deleted; `soc_out_of_bounds_fails_in_release`
  replaced (it could not be constructed) by
  `warmup_residuals_recorded_without_changing_days_run`, the SOC case
  covered by the existing absent-SOC test; dependencies BR-1, BR-3, BR-2.
- OR-1: dependency wording.
- OR-7: M-1's schema change rule.
- OR-8 rewritten for `metrics_optional` (field, no accumulator when not
  requested, manifests set `metrics = true`, new tests).
- OR-10: metrics section only when requested; new test.
- OR-11: removes `[test-release-invariants]`; depends on M-3.
- OR-15: test requests metrics.

### ledger/04-fleet.md

FL-1 uses `FleetReport`, cites OR-13, metrics only on request, depends on
OR-13. FL-2 status mapping on health events (a policy violation is
`Failed`), no "metrics unavailable" clause, `HealthPolicy` cited as OR-12,
metrics test requests metrics.

### ledger/05-python-surface.md

Intro order and Python test lines tied to `[python-build]`/`[pytest]`;
PY-7 census has no exclusion list (SL-8 lands first), dependency on SL-8,
precedes CN-13; PY-2 drops `resample_overrides`, names `warmup` and the
packaged defaults, depends on IN-6; PY-1 `thiserror` note corrected (no
cargo-deny configuration exists), test replaced by a Rust round-trip test
plus a Python table test, depends on BR-6b; PY-9 `metrics` keyword and
`metrics_none_unless_requested`.

### ledger/06-runtime-memory.md

RT-1 and RT-3 use `CountingAllocator` and assert `Some(0)` (RT-3 now
depends on PR-3, which replaces the envelope's own allocator); RT-1 runs
under `[test-observe-profiling]`; RT-2 names the warm-up variant, key
without resample policy (IN-6), types from §2.7; RT-4 uses §2.7 names,
optional schedule, depends on OR-6; RT-5 rows cite OR-5 for the three
counters, cross-platform moved to M-4a/M-4b; RT-6 adopts §2.7 and IN-5,
adds two tests; RT-7 (new): per-step RL observation without allocation.

### ledger/07-profiler.md

Intro states HARES#31 in full. PR-6 (new): per-actor timings accumulated
over the run, `actor_profiling` folded into `profiling`, reported in the
Python summary, docs corrected.

### ledger/08-consumer.md

Preamble: B0 on the Task 0 head after CN-0 and CN-16; B0 check range
defined by what the consumer can run (CN-0/CN-16 before PY-7, CN-13 before
PY-8); split rule for dependency upgrades; consumer block without CN-13;
contract-drift rule (which consumer row updates the field set); `Engine:`
trailer. Gate: failing set empty from CN-16 on, skips exactly the seven.
CN-16 (new): the three recorded failures. B0 captured with CN-16. CN-1:
`config=SimulationConfig(...)`, `metrics=True`, rejection probe, TRAP 2
refusal, OCHRE plumbing removal, setpoint factor check, dependencies.
CN-2: OR-12, curve-index clamps, `WarmupConfig(max_days=1)`, after CN-3.
CN-3 rewritten for `calibration_r_factor` (envelope scales, defaults
copies deleted, contract-drift update). CN-4 includes OD-9. CN-7 engine
metrics not requested on the fit path. CN-10 dependencies. CN-13 gated on
the PY-7 engine, dependency rename deferred to CN-15 (the lock reads the
pinned submodule's name). CN-14 names B-1a to B-1c, IN-4 and the field-set
update. CN-15 renames the dependency, regenerates the lock and the stale
report, runs after the last push.

### ledger/09-numeric-fixes.md

Preamble: own row, M-1 procedure, split-out `B-0<package>` rule. EnergyPlus
section: Feb 29 source lines (`:8456`, `:1896-1906`, `:2816-2836`) and the
documentation quote in full. B-1a: sub-hourly interval mismatch is an
error (`WeatherManager.cc:715-722`), test added, IN-6 and PY-8 cited.
B-1c: Feb 29 skipped as EnergyPlus does, row count and tests, day types
by calendar. B-2: `Warmup` enum, residuals from OR-5, manifests migrate.
B-3: `SimulationConfig(warmup=...)`, measurement without Python-driven
goldens. B-5: deletes OR-1's transitional call. B-6: OR-13 and FL-1. B-7:
metrics when requested. B-10: packaged Python defaults
(`hares.defaults_dir()`, tree inside the package), empty directory an
error, two tests, depends on RT-4 and IN-1. B-11: fleet fold only, tests
moved to the fleet.

### ledger/10-slop-cleanup.md

Intro: em dashes, no global sweep. SL-2 points to SL-8. SL-3 is a standing
rule (no own commit). SL-8 (new, own commit): the `git mv` move with the
measured behaviour, review tooling moved, `.shipwright/constitution.md`
untracked, CI prune lists cleaned.

### ledger/11-other-defects.md

OD-1 iterates every exported exception. OD-2 dependencies (before PY-8,
no RT-4 or consumer dependency, consumer effect stated); site 6 and 7
cite IN-5 and OD-8. OD-8 (new): battery with no declared control is a
construction error; fixtures and the vacuous tariff test fixed. OD-9
(new, numeric): generated schedules flatten the HPXML setpoints.

### ledger/12-inputs.md

Intro lists IN-5 to IN-7 and the order. IN-1 optional schedule record.
IN-3 lists HPXML elements generated from the parser (typed accessors,
`HPXML_ELEMENTS`, two tests), depends on IN-5 and IN-7. IN-4 names
B-1a to B-1c. IN-5 (new): OpenStudio-HPXML setpoint rule, warning,
Fahrenheit columns. IN-6 (new): OCHRE resampling out of the Rust core,
`hold_weather` adapter, held-weather fixtures for the Rust parity harness
and oracles. IN-7 (new): envelope `opaque_r_scale` and
`thermal_mass_scale` after row selection.

## Operator questions, answered by the decision log

| Question | Decision | Where |
|---|---|---|
| a1 per-actor profiling binding | `profiler_scope` | PR-6 |
| a2 goldens in the public tree | `golden_storage` | M-1, M-2, §3 |
| b Q1, d4 metrics opt-out | `metrics_optional` | §2.2, OR-8, CN-7 |
| b Q2, c3 planning records | `public_planning_docs` | SL-8, PY-7 |
| b Q3 HPXML elements in the schema | `input_schema_scope` | IN-3 |
| c1 thermostat precedence | `setpoint_sources` | IN-5, OD-9, CN-1 |
| c2 battery without control | `battery_control` | OD-8, CN-1 |
| c4 em dashes | `slop_scope` | SL-3 |
| c5 observation allocation | `rl_observation_allocation` | RT-7 |
| c6 macOS measurement | `cross_platform_measurement` | M-4a, M-4b |
| c7, d wh_energy_factor probe | `consumer_inert_override_probe` | CN-1, OD-2 |
| d1 three gate failures | `consumer_gate` | CN-16, gate rule |
| d2 seven skips | `consumer_gate` | gate rule |
| d3 intermediate pin | `consumer_gate` | `08-consumer.md` preamble, CN-15 |
| d5 Feb 29 | `weather_edge_cases` | B-1c |
| d6 sub-hourly mismatch | `weather_edge_cases` | B-1a |
| d7 `ResampleOverrides` | `ochre_compat` | IN-6 |
| d8 Python defaults | `defaults_path` | B-10 |
| d9 CN-3 row re-selection | `calibration_r_factor` | IN-7, CN-3 |

## Choices made in integration (where resolutions conflicted)

- B0 timing: resolution a put B0 after BR-6d, resolution d before any
  engine entry. B0 is on the Task 0 head (the task's rule: before any
  engine change that can move numbers), and the dependency upgrades are
  covered by the B0 check with a split rule if an upgrade moves the
  consumer's numbers.
- Consumer mechanism: resolution a's `maturin develop` line was replaced by
  resolution d's measured worktree plus `uv pip install --no-deps`
  mechanism, which `08-consumer.md` owns.
- Gate failures: resolution d's "subset of base failures" and resolution
  a's "exactly base less fixed" are replaced by "empty from CN-16 on", per
  `consumer_gate`.
- Package rename placement: resolution b placed PY-7 right after OR-5;
  resolution d required it after the last B0 check. PY-7 lands after the
  profiler, performance and slop rows, with CN-13 directly after it gated
  on that engine commit, so the B0 check continues through the row before
  PY-8.
- CN-13 no longer renames the consumer's dependency: `uv lock` reads the
  distribution name from the pinned submodule, still `ochre-next` until
  CN-15, so the rename moves to CN-15.
- OD-2 placement: resolution c wanted it after the consumer gate,
  resolution b before PY-8. It lands before PY-8; the probe it affects is
  not on the DE-trace path, and CN-1 turns the probe into the rejection
  assertion.
- SL-3 is no longer an own-commit global sweep (`slop_scope`).
- The `actor_profiling` feature folds into `profiling` so its tests run
  under an existing block line.
- The consumer-shaped golden keeps one override (`Gas Water Heater`
  `ua_w_per_k`): resolution d also listed `ASHP Cooler`
  `charge_defect_ratio`, but `bldg0176227` has a central air conditioner
  and furnace, no heat pump, and that factor is not free
  (`registry.py:217-225`).
- The golden records the defaults directory as repository-relative plus a
  digest of the files read, not an absolute path (resolution d contract 8),
  so no local path enters the public repository and a different tree is
  still detected.

## Findings outside the resolutions, now entries

- The committed consumer `docs/Test_Report.md` is stale (pin `7a3366a7`);
  CN-16 and CN-15.
- `generate_schedule_from_hpxml` writes constant setpoint columns (hour-0
  value, silent 20 C / 24 C), which outrank the HPXML profile: OD-9.
- Schedule-file setpoint columns are read as Celsius; OpenStudio-HPXML and
  OCHRE read Fahrenheit: IN-5.
- `test_tariff_integration.py` passes vacuously on an idle battery: OD-8.
- `actor_timing()` returns only the last step: PR-6.
- `.shipwright/constitution.md` is tracked although ignored: SL-8.
- The check scripts' prune lists name the moved directories: SL-8.
- The maturin include places `defaults/**` outside the package and nothing
  resolves it: B-10.

## Sweep results

- Every entry ID referenced in `plan.md` and `ledger/` has a heading;
  `B-0<package>` is the named pattern for a contingent split entry.
- `00-schedule.md` lists every entry once (BR-5 and the SL items in the
  rows that fix them; SL-2, SL-3 as standing rules).
- No em dash in `plan.md` or `ledger/`; no reference to review rounds,
  drafts or tools that wrote the plan; no path outside the initiative
  directory except the two code repositories, `~/.cache`, the
  engine-under-test worktrees and public URLs.

## Unresolved

- None requiring an operator decision; `plan.md` §5 stays empty.
- Recorded, not acted on: one file under `docs/eplus/` is tracked although
  `.gitignore:65` ignores that directory. It is third-party reference
  material, not a planning record, so SL-8 leaves it; whether to untrack it
  can ride any later documentation change.
- Contingent at execution, with the rule already decided: whether a
  dependency upgrade moves the consumer's numbers (split rule), whether
  any of CN-16's three cases still fails on the Task 0 engine (fixed in
  CN-16 if so), and whether the DE-trace premises carry a stationary
  battery (OD-8's B0 check).
