# Traceability audit of the I-08 ledger (L1, pass e)

Scope: `plan.md` and `ledger/00-schedule.md` .. `ledger/11-other-defects.md`, checked against
`decision-log.md` (all entries), the 45 `memory/feedback_*.md` rules, `context.md` items 1-21,
`review.md` V1-V50, `exploration.md` plus `needs-calibration.md` (R1-R25) and
`needs-hares-users.md` (PY/RL/FL/RS/DV/X), and every blocker and major in `verify-r1-v1..v5.md`
and `verify-r2-v1/v2/v4/v5.md`. The tables are my own; no mapping in the plan was trusted.
Read-only. HARES spot checks at `ebdb1a17` (`feat/output-results`): `#[ignore]` census,
`blueprint.rs:86-100`, `CHECKPOINT_VERSION = 8`, `SyntheticOutputConfig` (`synthetic.rs:291`),
warn-once statics (`hares-core/src/telemetry.rs:61,64`, `dwelling/mod.rs:144`), `custom_domains`
(`dwelling/mod.rs:4191,4959,7286`), `.gitignore:48-54`.

Severity: **blocker** = a binding decision or rule is contradicted, or a context item, confirmed
defect or in-scope must-requirement is missing. **major** = partial or unclear coverage, or an
earlier blocker/major not resolved. **minor** = everything else.

**Counts: 17 blocker, 26 major, 13 minor (56 findings).**

---

## Findings

### Blockers

**1. (blocker) The package decision is not implemented, and OD-1 contradicts it.**
- Source: `dl-i08-architecture.package` ("distribution hares-py, import name hares, cache
  directory ~/.cache/hares. No alias for ochre_next"); review V3 (package reasoning).
- Ledger: no entry renames the distribution, the import package, the extension module
  (`pyproject.toml` `module-name = "ochre_next._hares"`) or the cache directories
  (`python/ochre_next/data/resstock.py:126`, `data/weather.py:36`). OD-1 does the opposite:
  "register each exception under `ochre_next._hares`". `plan.md` §2.7 restates the decision but no
  entry owns it. The consumer side (import fallback `calibration/paths.py:72-93`, cache
  `paths.py:117`, `~/.cache/ochre_next` 5.4 GB with no auto re-download) has no CN entry.
- Correction: add an entry (for example PK-1, scheduled after BR-6 and before OD-1): rename
  `python/ochre_next` to `python/hares`, `name = "hares-py"`, `module-name = "hares._hares"`,
  cache dir `~/.cache/hares`, no alias, census by `git grep -i -E 'ochre[_-]next'` with a stated
  exclusion list; tests `import_ochre_next_is_absent`, `cache_dir_is_hares`. Rewrite OD-1 to
  register under `hares._hares` with `engine_exceptions_pickle_round_trip` across a spawned
  process. Add a consumer entry (before the B0 gate row) moving imports and `CACHE` to `hares`,
  with an operator-checkpointed re-provision (`tools/provision_data.py`) and a checksum
  assertion that the re-provisioned AMY/TMY3/metadata files equal the files B0 used.

**2. (blocker) The weather-record alignment defect (V4) is not fixed; B-1 declares the defective
path correct.**
- Source: `dl-i08-architecture.numeric_fixes` item "weather-record alignment";
  `dl-i08-architecture.weather`; review V4 (hourly reads lag one hour: `floor((t_start-1800)/step)`,
  `environment.rs:824-848`, row index `(step+offset) % len`, `:529-536`); consumer measured
  `ENGINE_ROW_OFFSET = 1` (`irradiance.py:48-67`).
- Ledger: `plan.md` §2.8 renames item 1 to "sub-hourly interpolation convention"; B-1 says the
  `compute_annual_offset` point-value path "is correct" and only changes solar interpolation.
  Every hourly run keeps the one-hour lag. B-1 also bundles wrap (V5) and leap-day handling into
  one commit, leaves the leap-day policy to implementation ("not invented here"), gives the
  `NoWrap`/cycle choice no config surface, does not classify PSM3 (`midpoint_offset_secs: 0`,
  start-labelled) or the other `WeatherField`s, does not say how wind direction interpolates,
  does not decide the fate of `ResampleOverrides`/`ResampleMethod`/`ochre_compat()`
  (`hares-io/src/weather.rs:317-445`) and the Python `resample_overrides` kwarg, and targets a
  `PHYSICS_DECISIONS.md` without the path (`docs/PHYSICS_DECISIONS.md` does not exist). No
  consumer entry removes the roll+1 workaround, so after B-1 the consumer double-shifts.
- Correction: split B-1 into (a) record alignment: state the record each step reads (at
  `dt = 3600`, the step `[00:00, 01:00)` on Jan 1 reads the record labelled 01:00, row 0), cite
  `environment.rs:529-536,811-849`, test `hourly_step_reads_hour_ending_record` and
  `first_step_of_nowrap_file_uses_declared_pre_record_rule`; (b) solar midpoint interpolation
  (as written) plus a per-format label table and a field-kind table covering every `WeatherField`,
  wind direction interpolated through u/v, test `wind_direction_interpolates_on_the_circle`;
  (c) overrun and cycle (V5): a `weather_cycle` field in `SimulationConfig` with the ResStock AMY
  CSV declared `NoWrap` by `Fleet::from_resstock`; (d) one leap-day policy shared by weather and
  schedules, stated in the ledger (EnergyPlus does not synthesise Feb 29), test
  `weather_and_schedule_agree_on_feb29`. Decide the resampling override surface (recommended:
  delete it and `ochre_compat()`, removed-key error naming the replacement). Name
  `docs/PHYSICS_DECISIONS.md` as created. Add a consumer entry after (a) that deletes
  `ENGINE_ROW_OFFSET` and replaces `ami2/metered.py` `oat_steps` with `resample_weather`
  (finding 5), recorded in the B1 attribution.

**3. (blocker) The schedule violates the consumer-ordering and own-commit decisions, and the B0
gate cannot run as written.**
- Source: `dl-i08-architecture.consumer` ("mechanical consumer migration lands before the engine
  numeric fixes, gated bitwise against baseline B0"); `numeric_fixes` ("each in its own commit").
- Ledger: B-7 (row 62) "folds into OR-8's commit", but OR-8 is row 26; B-12 (row 66) "folds into
  FL-4's commit", but FL-4 is row 34. Both numeric changes therefore land before the consumer
  migration (rows 46-54) and neither has its own commit. CN-1's acceptance requires the migrated
  consumer to run "against the same unmodified engine", which cannot work because the migration
  targets `ReportSpec`/`RunResult.health`, which exist only after rows 21-24. Engine rows 1-45
  also include changes that can move or break consumer runs before the gate: OD-2 (row 10) turns
  previously accepted overrides into errors, including the consumer's `wh_energy_factor`
  reachability control (`registry.py:189-200`, "must come back INERT"), OR-5 makes release-build
  wiring defects fatal, BR-6 upgrades nalgebra. A gate failure at row 54 cannot be attributed.
- Correction: make OR-8 structural and bug-compatible (reproduce today's verbosity-dependent
  metrics bit for bit) and give B-7 its own commit in the numeric block; same for FL-4 and B-12.
  Restate CN-1's gate: B0 is captured on `0ec757cb`; the gate compares the migrated consumer on
  the row-45 engine against B0; the precondition that rows 1-45 are consumer-neutral is proven
  engine-side by a golden of the consumer's shape (study home, 900 s, verbosity 2) captured on
  the pre-I-08 engine and compared after every engine row. Add a consumer entry before the gate
  that redesigns the reachability control to assert the typed rejection, and either move OD-2's
  override-rejection sites after the gate or record OD-2's effect as B0' before gating.

**4. (blocker) The health decision is contradicted in four places.**
- Source: `dl-i08-architecture.health` ("Every health event (port rollback, clamp, rejected
  signal, warm-up non-convergence) is recorded unconditionally in every build and returned in
  RunResult.health. NaN and physics invariant violations always fail the run, in every build
  profile, including warm-up. Other escalation is an explicit HealthPolicy, empty by default");
  `new_capability` ("strict health policy"); V25; context items 6 and 10; DV-1 (M); R3 (MUST);
  `feedback_pay_for_what_you_need`.
- Ledger:
  1. OR-5 keeps the warm-up exemption for zone-temperature bounds and may keep the
     `cfg(any(debug_assertions, feature = "check_invariants"))` gate on it "if a measurement shows
     its cost is worth gating". The other checks in `check_invariants()` and `invariants.rs`
     (about 30 sites, for example `thermal_balance`) are not classified at all.
  2. Non-finite telemetry is still silently dropped in plain release (`hares-types/src/telemetry.rs:33-86`,
     `NON_FINITE_REJECTIONS` at `:20`); no entry changes it.
  3. RT-5 says "a counter (already an atomic) is safe under threads as-is", keeping
     `REJECTED_SIGNAL_COUNT` (`hares-equipment/src/lib.rs:35`), `ACTIONS_CLIPPED`
     (`py_gym.rs:208`) and the range counters process-global. That contradicts V25, context
     item 6 ("counter statics made per-run") and DV-1. No entry wires rejected signals, clamps or
     warm-up non-convergence into `RunHealth`.
  4. No entry builds `HealthPolicy`, its escalation, or R3's "strict mode raises a typed error
     naming step and zone"; CN-2 depends on `HealthPolicy::Strict`.
- Correction: rewrite OR-5 so every physics invariant runs in every build profile including
  warm-up, with a table classifying each check in `check_invariants()`/`invariants.rs` as a
  physics invariant (always on) or a debug assertion (with a reason). If unconditioned-zone
  bounds during warm-up must stay exempt, that is an operator question, not an entry decision.
  Add an entry for a per-dwelling non-finite latch in `Telemetry` checked once per step, raising
  `NanDetected` with step, equipment and key (test `non_finite_telemetry_fails_run_in_plain_release`).
  Add an entry that moves each health counter to per-dwelling state returned in `RunHealth`
  (test `concurrent_runs_report_independent_counts`, DV-1). Add an entry that builds
  `HealthPolicy` (empty default; exact escalatable event set; typed error naming step and source)
  with `strict_policy_escalates_port_rollback_naming_step` and `empty_policy_never_escalates`.

**5. (blocker) Capabilities the decision log says are built now have no entry, and the not-now
seams are not named.**
- Source: `dl-i08-architecture.new_capability` ("Built now: pre-run catalogue and reporting
  frequency, resolved-parameter read-back, validate(), strict health policy, metrics_from_table,
  resample_weather, replay export, machine-readable input schema. Not now (seams kept): batched
  multi-variant evaluation, SteppableFleet rewind, a built-in reward enum"); `inputs` ("can be
  supplied as content ... defaults"); `config` ("one validate()"); `dl-i08-process.scope` ("each
  such item keeps a named extension seam").
- Ledger: nothing builds `validate()` (R9), `metrics_from_table` (PY-12), `resample_weather`
  (R21 API), an engine replay export (R22; CN-11 is a consumer-side tool over consumer-recorded
  data), a machine-readable input schema (R8), or content inputs `Source::{Path, Text, Bytes}`
  (named only in `plan.md` §2.7; R7, including the per-table defaults override the consumer
  needs for `Envelope Materials.csv`). None of the three not-now seams is named.
- Correction: add one entry per capability, each with a named deterministic test:
  `validate_rejects_afue_above_one_with_run_time_message`, `metrics_from_table_matches_in_run_metrics`,
  `resample_weather_equals_outdoor_dry_bulb_column_at_900_and_3600`,
  `replay_export_reproduces_frame_bitwise`, `input_schema_lists_every_deny_listed_class`,
  `content_source_run_equals_path_source_run_bitwise` (including a per-table defaults override).
  Name each seam in `plan.md` §2 (for example the `Blueprint` shared-input handle for batched
  evaluation; `SteppableFleet::load_state` returning a typed "not rewindable" error, with a
  test; the `reward_fn` callable as the reward seam).

**6. (blocker) Reporting frequency and per-variable time aggregation are not implemented.**
- Source: `dl-i08-architecture.output_model` ("report requests (selection, reporting frequency,
  destination chosen independently)"), `new_capability` ("reporting frequency"), `aggregation`;
  PY-9, X-9, FL-7, R2 (SHOULD), PY-8.
- Ledger: OR-2 adds the `Frequency` type but no entry reduces rows to Hourly/Daily/Monthly/
  RunPeriod/Annual periods, applies Sum/Mean/Mode per `VariableDef.aggregation` per dwelling,
  states null handling under aggregation or partial periods. B-11 covers only the fleet fold.
  Selection by name or pattern has no test that an unknown name errors naming the valid set, or
  that a one-column projection is bitwise equal to the same column at full verbosity (CN-1 relies
  on it).
- Correction: add an entry after OR-2 implementing period reduction with tests
  `hourly_energy_sum_equals_timestep_integral`, `hourly_temperature_is_mean`,
  `daily_mode_column_is_mode`, `partial_first_period_policy`, `null_cells_excluded_with_valid_count`
  (valid count as a catalogue companion variable, not schema metadata). Add to OR-2
  `unknown_selection_name_errors_naming_valid_set` and
  `single_column_projection_bitwise_equals_full_verbosity_column`.

**7. (blocker) The Callback destination has no entry, no Python surface and no tests.**
- Source: `dl-i08-architecture.destinations` ("a batch-level Python callback (opt-in, zero cost
  when unused, a callback error fails the run with the Python exception chained)").
- Ledger: `05-python-surface.md` says it "implements §2.2's Callback destination", but no PY entry
  does. OR-3 lists the variant only. `plan.md` §2.2 puts `Callback(Py<PyAny>)` in `hares-io`,
  which has no pyo3 dependency. No test covers error chaining, zero cost when unused, the batch
  context (report name, dwelling identity), concurrent fleet calls or serialisation in
  `Provenance`.
- Correction: add PY-7: `trait BatchSink: Send + Sync` in `hares-io`, the pyo3 implementation in
  `hares-python`, the Python constructor, and tests `callback_receives_every_row_once`,
  `callback_exception_chained_as_cause`, `no_callback_configured_allocates_nothing`,
  `fleet_callback_receives_dwelling_identity`, `provenance_records_callback_as_marker`.

**8. (blocker) The config decision is not carried out.**
- Source: `dl-i08-architecture.config` ("One Rust config type with one validate(). TOML,
  synthetic TOML and Python construct that type ... unknown or removed keys are errors that name
  the replacement. No twin config types"); `destinations` ("No file rotation");
  `feedback_no_silent_defaults`; R11, RS-3, X-7.
- Ledger: `SyntheticOutputConfig` (`synthetic.rs:291-327`) remains a twin with its own defaults.
  OR-3 removes `write_output`/`retain_batches`/`output_format` but no entry makes those keys (and
  `output_path`, `output_to_parquet`, `rotation`, `output_chunk_size` if renamed) errors naming
  their replacement. No entry deletes `RotationPolicy` (`config.rs:36-48`) or the consumer's
  `rotation` pass-through (`inject.py:157-158`, `engine.py:338-339`). The Python kwargs path's
  silent fallbacks for `start_time`, `duration_s`, `time_res_s`, `master_seed`
  (`py_dwelling.rs:2260-2345`) are untouched. `default_output_path` removal is only in
  `plan.md` §2.2, not in OR-3's change. No fixture/TOML/Python census for removed keys.
- Correction: add a config entry: one `SimulationConfig`, synthetic TOML built from it, a
  removed-key table with replacement names, `RotationPolicy` deleted, `default_output_path` and
  the two duplicated path computations (`conversions.rs:590-596`, `engine.rs:411-426`) deleted,
  Python kwargs either required or documented Rust defaults from one table; tests
  `removed_key_write_output_names_reports`, `synthetic_toml_rejects_unknown_key`,
  `python_and_toml_defaults_come_from_one_table`; plus a census command over TOML fixtures,
  Python tests and examples.

**9. (blocker) Envelope solar/incidence reuse and the envelope-side allocation items are missing.**
- Source: `dl-i08-architecture.engine_performance` ("envelope solar/incidence and coupled
  factorization reuse"); context item 4 (LU rebuilt twice per step, `m_scratch` reallocated per
  call, solar slot map rebuilt twice per step at `thermal_solver/mod.rs:1560-1568`).
- Ledger: RT-3 covers only `prepare_inputs_inner`'s clone and `HashMap`, and treats the LU as a
  measure-then-maybe-drop item. The solar slot-map rebuild, incidence recomputation and
  `m_scratch` reallocation appear nowhere.
- Correction: add RT-6: measure first (recorded), then build the solar slot map once at
  assembly, reuse incidence per surface per step, pre-allocate `m_scratch`; test
  `envelope_step_allocation_free_after_first_step` and factorization count
  `coupled_lu_factorizations_per_step` (asserting 0 when `m_is_identity()`), bitwise goldens,
  with the measurement-drop rule applied per sub-item.

**10. (blocker) The reproducibility decision is only partly tested, and FL-1 reuses the V30
defect.**
- Source: `dl-i08-architecture.reproducibility` ("Bitwise identical per platform across
  processes, thread counts and runs, tested. Cross-platform agreement is measured and
  published"); R12 (MUST); DV-2; V30.
- Ledger: no cross-process whole-frame test (R12: 20 cross-process runs of a below-grade home);
  no cross-platform measurement or publication; FL-1's `fleet_fold_is_order_independent` varies
  `RAYON_NUM_THREADS`, which is a no-op after the global pool exists (the V30 mechanism).
- Correction: add `whole_frame_identical_across_spawned_processes` on a below-grade fixture
  (OD-4's) and the ResStock fixture; FL-1 uses explicit `ThreadPoolBuilder::num_threads(1|4|N)`
  pools (the fleet already accepts a pool size, `fleet.rs:321,449`); add a recorded
  macOS arm64 versus Linux x86_64 frame-hash and metrics comparison with where it is published.

**11. (blocker) PY-5 keeps the hard-coded reward and breaks the one-reward-contract
requirement.**
- Source: review V15 decision (Rust hard-coded reward "is silently wrong, not a real default
  worth preserving"); `new_capability` ("a built-in reward enum" is not now); RL-5 (M: the same
  `reward_fn` gives the same rewards with and without the fast path).
- Ledger: PY-5 preserves `-net_electric_power_kw` "exactly" when `reward_fn` is unset, and calls
  `reward_fn` once per batch with batch telemetry, a different signature from the Python path's
  per-environment `RewardContext`.
- Correction: `batch_step` returns per-environment `RewardContext` inputs; `vec_env.step` calls
  `reward_fn` per environment in Python on both paths; the Rust reward and the `reward` key are
  deleted. Test `batch_step_and_fallback_give_identical_rewards_for_same_reward_fn`.

**12. (blocker) Confirmed defect V36 is missing, although `05-python-surface.md` claims OD-2
covers it.**
- Source: review V36 ("UTC and daylight-saving ambiguity resolve to silent fallbacks",
  `blueprint.rs:90-97`); verified: `.unwrap_or_else(|| FixedOffset::east_opt(0)...)` and
  `.single().unwrap_or_else(...)` at `blueprint.rs:90-98`.
- Ledger: OD-2's site list does not include it.
- Correction: add the site to OD-2: an invalid UTC offset or an ambiguous/nonexistent local
  start time is a construction error naming the value; tests
  `invalid_site_utc_offset_is_an_error`, `ambiguous_local_start_is_an_error`.

**13. (blocker) BR-8 contradicts `feedback_no_ignore_tests` and misstates the census.**
- Source: `feedback_no_ignore_tests` ("`#[ignore]` with any excuse ... is NEVER acceptable").
- Ledger: BR-8 says two tests are ignored and keeps both with a comment. HEAD has 11 `#[ignore]`
  attributes: `alignment_oracles_regressions.rs:515,709,770`, `solar.rs:2461`,
  `tests/bestest/mod.rs:404,506,619,665,882`, `bestest_diagnostic.rs:114`,
  `resstock_smoke.rs:299`.
- Correction: convert each diagnostic helper into an example or a feature-gated non-test
  function (no assertion to lose); any that asserts a physical result is un-ignored and fixed.
  Acceptance: `grep -rn '#\[ignore' --include=*.rs crates tests` returns only comment lines.

**14. (blocker) Several acceptance tests break the binding test rules.**
- Source: `feedback_deterministic_tests` (never depend on thread scheduling or timing),
  `feedback_test_quality` ("Don't test constants equal themselves").
- Ledger: PY-3 `from_hpxml_releases_the_gil` asserts a background thread's counter advanced
  during construction (timing and scheduling dependent). FL-1
  `fleet_memory_bounded_independent_of_home_count` asserts an RSS ratio (nondeterministic) and
  runs a 10,000-home fleet in the test suite. B-2 `warmup_threshold_matches_energyplus_default`
  pins the constant 0.4.
- Correction: PY-3 asserts the GIL is released with `PyGILState_Check`/`Python::with_gil`
  attempted from inside a Rust hook in the detached section (deterministic). FL-1 asserts retained
  bytes by accounting (peak count of live per-home accumulators, bounded by chunk size) and
  records RSS as a measurement. B-2 replaces the pin with a behavioural test (a fixture whose
  day-to-day delta is between 0.4 and 0.5 C runs one more warm-up day than it did at 0.5).

**15. (blocker) The verification block omits the decided stray-file check and its pytest line
cannot run.**
- Source: `dl-i08-architecture.stray_outputs` ("Verification compares ignored-file listings
  before and after each run; any new stray file fails").
- Ledger: `plan.md` §3's block has no before/after listing (BR-7 adds a CI check but the block
  every entry runs does not). `uv run pytest --all-extras` is not a pytest option; the block
  never runs `maturin develop`, so pytest tests a stale or missing extension; `addopts`
  deselects `slow`/`ochre`; `gymnasium` is not installed, so RL tests skip. Until OR-3, pytest's
  `conftest.make_dwelling` (`bldg_id=42`) writes `tests/python/dwelling_42.csv`, which BR-7's
  Rust-only census does not cover.
- Correction: add to the block `git status --porcelain --ignored --untracked-files=all > pre`
  / run / `> post` / `comm -13 pre post` empty, with a declared build-artifact allowlist; replace
  the pytest line with `uv sync --all-extras --all-groups`,
  `uv run --no-sync maturin develop --release`, `uv run --no-sync pytest tests/python`,
  `uv run --no-sync pytest tests/python -m "slow or ochre"`, failing on any skip outside a listed
  allowlist; extend BR-7 to Python tests and examples.

**16. (blocker) The documents carry superseded-draft history and cite sections that do not
exist.**
- Source: `dl-i08-process.documents` ("hold only current, correct content"), `writing` ("no
  planning IDs ... in these documents"), `feedback_shipwright_files_authoritative`.
- Ledger: `plan.md` §2.10 ("proposed in earlier drafts of this plan"); BR-7 ("an earlier draft's
  count"); B-1 ("an earlier draft's characterization"); B-2 ("an earlier, unreproduced
  measurement", "the earlier draft's"); RT-2 ("an earlier, wrong premise"); RT-3 ("An earlier ...
  framing"); RT-5 ("not carried over from an earlier count"); PY-6 ("an earlier defect table
  entry"); SL-2 ("an unrecorded prior plan"). References to `review.md` O-L, O-C, O-H, O-I, H1,
  H2, H3, §1.4, §3, §4, §5 and §"Measurement conditions" (RT-1..RT-5, CN-6, CN-7, CN-11, PR-2,
  PR-4, OR-2, PY-1, M-3): `review.md` has only §1 (V1-V50) and §2, so every one dangles, and the
  baseline percentages cited from them have no source. PY-3 cites a nonexistent
  "`06-runtime-memory.md` H-3". CN-1 cites "production O5 cells (`decision-log.md` § consumer)",
  which the decision log does not define.
- Correction: delete the history phrases; cite `context.md`'s baseline table or the
  `02-measurement.md` log for each baseline figure; replace review IDs with the V-ID or the
  file:line evidence; replace "H-3" with RT-5; define the O5 cells (homes, counties, seeds) in
  CN-1.

**17. (blocker) No entry gives Python the `finish()`/`RunResult`/`health()`/report surface the
consumer entries depend on.**
- Source: `dl-i08-architecture.results` ("Stepped and simulated runs finalise identically");
  R3 (MUST, health readable), PY-1 (M), X-3 (M).
- Ledger: OR-6 is Rust-only. CN-1 uses `RunResult.health`, CN-2 calls `dw.health()`, CN-7 reads
  `RunResult.metrics`, but no PY entry exposes `finish()`, `RunResult`, `health()`, `ReportSpec`
  construction or the Memory report table to Python. `SimulationPlan`, the gym/vec env and the
  HELICS wrapper (`python/ochre_next/helics/`) never call `finish()`. `SimulationEngine::run`/
  `run_dwelling` (`engine.rs:102,251`, which zero metrics and flag with no recorder) are not
  migrated.
- Correction: add PY-8 (Python `finish()`, `result()`, `health()`, `reports=[...]` kwarg built on
  the Rust `ReportSpec`, stubs in `_hares.pyi`), with `python_stepped_finish_equals_simulate`
  (N below and not a multiple of the chunk size) and `health_readable_with_no_columns_requested`;
  migrate `SimulationPlan`, gym, vec env and HELICS to call `finish()`; add an OR-6 bullet that
  `SimulationEngine::run`/`run_dwelling` return `RunResult` through one shared path with typed
  errors preserved (RS-1, RS-4).

### Majors

**18. (major) The branch step for PR #27 and the final push are missing (earlier r2-v5 M1/B4,
r2-v2 B1, r2-v4 M1).**
- Source: `dl-i08-process.branch` ("recreated from initial-implementation after the operator
  merges PR #27").
- Ledger: `plan.md` §3 lists the merge only as an operator checkpoint; no schedule row brings the
  merged `initial-implementation` into `feat/output-results`; all evidence is at `ebdb1a17`, but
  PR #27 changes `py_dwelling.rs`; nothing names the branch pushed at the end or how consumer pins
  become resolvable.
- Correction: add row 0: `git fetch origin`, confirm PR #27 merged, on `feat/output-results`
  `git merge --ff-only origin/initial-implementation` (no delete, no checkout), re-verify every
  `py_dwelling.rs` citation. Add a final operator-checkpoint row: push `feat/output-results`, open
  a PR into `initial-implementation`, then move the consumer pin to a pushed SHA.

**19. (major) The consumer engine-under-test and pin mechanics are inconsistent or incomplete
(r2-v2 B2, M5, M10, M11; r2-v5 M20; r1-v2 M13).**
- Ledger: `plan.md` §3 says `uv run --active --no-sync maturin develop --release`; `08-consumer.md`
  says `uvx --from maturin maturin develop --active --release`. No command fetches the worktree
  SHA into the nested `./HARES` clone or authorises the nested `checkout --detach` per pin
  (`feedback_no_git_checkout`). Consumer AGENTS rule 5 (gate plus Test_Report per pin bump) is
  neither honoured nor waived. `run_all` exits 0 on skips, so an engine import failure passes the
  gate. CN-9 switches `check_losses.py` to `float.hex` while `PINNED` still holds `round(...,4)`
  values, so the tool fails at once. The DE-trace payload excludes nothing, but `HomeCalibration`
  carries wall-clock fields.
- Correction: one mechanism in both files; per-pin commands (`git -C HARES fetch
  /home/rich/src/HARES <sha>`, operator-approved batch `checkout --detach`); gate =
  `run_all` exit 0 and "0 skipped" (or a listed allowlist); CN-9 re-pins `float.hex` values
  measured on the B0 engine; define the compared DE-trace payload (loss, trace, x, n_eval,
  n_rejected, flags) and exclude timing fields; state the AGENTS rule 5 handling.

**20. (major) CN-7 needs an engine opt-out for metrics that OR-8 forbids.**
- Ledger: OR-8 says `RunResult.metrics` "is always populated"; CN-7's test requires it `None` and
  `MetricsCalculator` never constructed. No engine entry adds a metrics request. CN-7's change
  does not remove the `dw.equipment()`/`equipment_zip()` calls its title names.
- Correction: either add `metrics: bool` to the report request in OR-3/OR-8 (default on, stated as
  consistent with "metrics do not depend on selected reports") with
  `metrics_off_constructs_no_calculator`, or restate CN-7 as removing only the consumer-side
  `metrics()`/equipment-metadata calls.

**21. (major) CN-2b's strict set is inconsistent with OR-5, and it adds a second baseline.**
- Ledger: CN-2b arms strict policy for "port rollbacks and unresolved columns", but OR-5 makes an
  unresolved column an `InvariantViolation` (always fatal) and `RunHealth` has no unresolved
  counter. CN-2b captures "its own new baseline" before B1, against "re-baselines once".
- Correction: list the exact escalation set from `RunHealth` fields (for example
  `port_rollbacks`, `rejected_control_signals`, `warmup` not converged); record CN-2b's delta as an
  attribution trace folded into B1, not as a separate baseline.

**22. (major) Four context items are covered only in part.**
- Item 3: RT-4 shares HPXML, weather and schedule but not the defaults store. Add defaults to the
  `Arc`-shared inputs and to `blueprint_shared_across_dwellings_saves_parse_time`.
- Item 15: nothing addresses the ~29% unattributed step time or R16's "phase shares sum to within
  5% of release wall-clock". Add a PR entry with a recorded check.
- Item 16: CN-1 does not replace `contract()`'s live probe with a schema-version (catalogue
  digest) assertion. Add it with a test.
- Item 17: CN-5 fixes the dead shrink only; "worker pools sized to the population for lone fits"
  is missing. Add it with `lone_fit_pool_equals_population_when_smaller_than_cores`.

**23. (major) The result contract types and warnings semantics are undefined (r2-v1 M4, M12;
r1-v1 minor3).**
- Ledger: `RunMetrics`, `Warning`, `Provenance`, `ReportResult`, `WarmupOutcome`,
  `MetricsStatus`, `StepOutcome` and per-column `Coverage` have no fields. Warnings must reconcile
  with today's drain-per-step `take_warnings()` that the gym relies on (`vec_env.py:208-213`);
  RS-6 (warm-up non-convergence in results) has no entry.
- Correction: add a `plan.md` §2 block with the fields each test asserts on, and state that
  `RunResult.warnings` accumulates while a per-step cursor serves the gym; test
  `warmup_nonconvergence_appears_in_run_result_warnings`.

**24. (major) `ResolvedModel` (R6, MUST) has no field list and no test.**
- Ledger: RT-4 names it; neither RT-4 nor any other entry tests it; CN-3 depends on it.
- Correction: list the R6 quantities in SI (capacity, COP/EIR, AFUE, effective setpoints per
  zone, infiltration ACH50 and ELA, window U/SHGC, effective R and C per boundary, usage
  multipliers, PV nameplate) and add `resolved_model_eir_moves_with_hspf_scale` and
  `resolved_model_available_without_simulating`.

**25. (major) The catalogue does not meet R4, DV-8 or its own migration test (r1-v1 M3, r1-v4 18).**
- Ledger: no test that the catalogue is queryable before a run; metric fields and sign
  conventions are not in it; DV-8's contract test is a "review pass"; OR-1's
  `catalogue_matches_todays_schema_at_every_verbosity` requires today's name-classifier outputs
  (Television gets EV columns), while B-5 says end use comes from the typed descriptor, and OR-1
  does not say the builder reproduces the classifiers until B-5.
- Correction: OR-1 builds end use from today's classifiers transitionally (bitwise schema), B-5
  switches to `EquipmentDescriptor.end_use`; add `catalogue_queryable_from_blueprint_before_run`
  and `every_written_channel_is_declared_and_every_declared_variable_is_written`.

**26. (major) Warm-up after B-2/B-3 departs from EnergyPlus and is not observable (r2-v5 M9,
r2-v4 M6).**
- Ledger: B-2 makes `initialization_duration` the maximum warm-up days; the Rust default is 7
  days (`mod.rs:1812`) and B-3 copies it to Python, so the effective maximum becomes 7, not the
  EnergyPlus 25 that B-2 cites. EnergyPlus also tests load convergence; B-2 checks temperature
  only. No entry fills `RunHealth.warmup`; R20's "first timestamp equals start_time and row count
  equals duration/time_res" has no test.
- Correction: default the maximum to 25 days (or record why 7 with a measurement); add load
  convergence or state why not; B-2 records `WarmupOutcome { days_run, converged, residual }`;
  add `frame_starts_at_start_time_with_exact_row_count`.

**27. (major) B-13 has no evidence and no layer-distribution rule (r2-v5 B2).**
- Ledger: B-13 cites no file:line for today's precedence; CN-3 relies on "the engine's own
  resolver" decomposing a declared assembly R into layers, a rule no entry states.
- Correction: cite the code that lets `precomputed_rc` win; state the rule (for example scale
  every layer's R by `(R_assembly - R_films) / (R_table - R_films)`, films fixed, C from the
  table) and test `assembly_r_distributes_across_layers_per_rule`.

**28. (major) B-8's defect is unreachable from any dwelling (r1-v1 B1, r2-v1 M18).**
- Ledger: B-8 says "add [a fixture] that does" reach the fallback; `solver_builder.rs:1655` builds
  every dwelling's fluid solver with empty `loop_topologies`, so no dwelling fixture can.
- Correction: a `hares-envelope` test builds a `LoopTopology` whose `Source` node has an incoming
  edge (so `pump_node_id` is `None`), with at least three resolved nodes of magnitudes
  `1e16, 1.0, -1e16`, and asserts bitwise equality across fresh `HashMap`s; state that the fix is
  golden-neutral for every dwelling.

**29. (major) B-10 leaves the CWD-relative default and production `None` callers (r2-v2 M16,
r1-v5 M18).**
- Ledger: `PathBuf::from("defaults")` remains, so results depend on the working directory unless
  the directory is missing; the audit covers tests only, not `fleet.rs:200,1174,1191` or
  `dwelling/mod.rs:1808,1888`; M-2 goldens do not pin an absolute defaults path, so B-10 moves
  every golden captured from the crate directory.
- Correction: `defaults_path` required (or resolved from package data, not the CWD); migrate the
  production callers and `tests/regression/helpers.rs:95,129,254` in B-10's commit; M-1/M-2
  capture with an absolute defaults path recorded in the sidecar.

**30. (major) V42 is only partly fixed.**
- Ledger: B-9 fixes the year and offset; the unresolved weather path that becomes
  `PathBuf::from("")` with only a log line (`fleet.rs:170-194`) and the hard-coded `{fips}_2018.csv`
  pattern (`fleet.rs:820-825`) are not addressed.
- Correction: add both to B-9 (error naming the building and county) with
  `unresolved_resstock_weather_is_an_error`.

**31. (major) V28 is only partly fixed.**
- Ledger: OR-10 checkpoints `BillingState`, closed summaries, metrics and health, but not the
  `TariffEvaluator.step_index` (`evaluator.rs:26`, named in V28) or the warning log.
- Correction: include both, with `restore_resumes_tariff_price_index` and
  `restore_resumes_warnings`.

**32. (major) Destination collisions, overwrite and parent-directory errors are untested (FL-5,
PY-3, R10; r1-v4 25, r2-v1 M22).**
- Ledger: `overwrite: bool` exists with no semantics; `std::fs::rename` overwrites on Unix; two
  homes with the same path race; FL-5 (M) has no entry.
- Correction: unique `.partial` names per writer; final publish via `hard_link` then remove
  (fails with `AlreadyExists`) unless `overwrite`; construction rejects duplicate paths inside a
  fleet; tests `existing_target_without_overwrite_errors`,
  `fleet_homes_with_same_path_are_rejected`, `missing_parent_dir_is_io_error_naming_path`.

**33. (major) Fleet coverage has gaps.**
- FL-1 retains up to 64 full per-home frames, against "Fleets never retain full frames"; fold each
  home into period accumulators at the fleet's frequency as it completes, within the ordered
  chunk.
- FL-2 (M, "default fleet configs produce metrics and aggregates, or fail loudly") has no test;
  review's raise-on-zero-covered-weight default is absent.
- FL-4 does not say that under `Instant` each home buckets on reference-zone period boundaries,
  or how edge periods with partial coverage are reported (r2-v5 M3).
- FL-7 (fleet selection and frequency) and FL-9 (`SteppableFleet` without full telemetry) have no
  entry.
- Correction: add these to FL-1, FL-2 and FL-4 with tests `resstock_default_produces_aggregate_or_raises`,
  `zero_covered_weight_raises`, `instant_alignment_buckets_on_reference_boundaries`,
  `fleet_report_selects_columns_and_frequency`.

**34. (major) Runtime items from the rules and requirements are missing (r2-v4 B5, r1-v4 22, 53).**
- `feedback_custom_domains_perf`: the per-step `custom_domains` retain/push
  (`dwelling/mod.rs:4191,4959,7286`) has no entry.
- RL-6 (observation index plan, zero allocations per step), RL-8, and `telemetry()`'s per-call
  allocations (`mod.rs:4160-4185`) have no entry.
- `HashMap` iteration in numeric build paths (`solver_builder.rs:1317,1345`) has no sweep or lint.
- RT-5's census omits `hares-core/src/telemetry.rs:61,64`, `dwelling/mod.rs:144` and
  `py_actor.rs:539-540`.
- Correction: add RT entries for each with allocation-count tests; complete RT-5's census with a
  per-static disposition.

**35. (major) Memory requirements are neither tested nor measured.**
- R18 (flat RSS over 500 construct-and-simulate runs), RL-1 (flat over 10^6 steps), RL-2 (100
  episodes equal 1) and RL-3 (reset restores a captured state without rebuilding;
  `reset_with_seed` rebuilds, `py_dwelling.rs:1518-1532`) have no entry.
- Correction: add recorded measurements to `02-measurement.md` for R18/RL-1/RL-2, and an entry for
  RL-3 on top of RT-4 and OR-10 with `gym_reset_does_not_reparse_or_touch_files`.

**36. (major) The golden tool and its ordering do not support the claims made on it.**
- M-1 captures frames only, but OR-8 compares metrics bitwise and OR-10 compares billing and
  health; add metrics, billing summaries and health to the capture.
- No entry ports `frame_golden` to the new API after OR-3/OR-7 (r2-v1 M19).
- M-2 says the DST year "has one fewer or one more row"; the clock is local standard time, so a
  year at 900 s has 35,040 rows with or without DST. Fix the criterion.
- There is no fixture of the consumer's shape (study home, 900 s, verbosity 2).
- BR-6 (row 5) upgrades nalgebra before M-1 (row 13) exists, so "any output change from the
  nalgebra upgrade is recorded before the baseline is measured"
  (`dl-i08-architecture.dependencies`) cannot be checked bitwise. Move M-1 and a pre-upgrade
  capture ahead of BR-6's nalgebra step, or split nalgebra out of BR-6 to after M-2.

**37. (major) Several entries leave design to the implementer or lack file:line evidence
(`dl-i08-process.plan_form`).**
- Undecided: PR-2 ("whichever the review ... finds"), PR-3 ("an implementation decision"), SL-4
  (reword or real LHS), OD-1 (two alternatives), OD-2 `wall_area_m2` (reword or remove; DV-9 wants
  an error), CN-6 (precondition "raises (or falls back ...)"), OR-5 (gate kept "only if a
  measurement shows"), B-1 ("Confirm the current behavior ... before writing the fix"), PY-6
  (`hares-core` "or `hares-types`"), OD-5 (own commit or folded), CN-2a (own commit or folded).
- No evidence: B-13, CN-6 ("confirmed by direct reading"), CN-7, CN-11, PR-4.
- Correction: decide each in the entry and add the citations.

**38. (major) `build_info().dirty` goes stale and its tests are infeasible (R14 MUST; r2-v1 M1,
M3; r2-v4 M8).**
- Correction: `rerun-if-changed` on the git index and crate sources (or a source-tree hash as
  identity); move git resolution into a pure function unit-tested against temporary repos
  (worktree, submodule, no `.git`); add `build_info_dirty_after_uncommitted_edit`.

**39. (major) The verification block is red from row 7 to row 22.**
- Ledger: the plain-release nextest line fails the three BR-5 tests until OR-5 (row 23), yet §3
  requires the full block after every row and forbids narrowing it. The `-F profiling` tests of
  PR-1..PR-4 are not in the block.
- Correction: move the wiring-integrity half of OR-5 into BR-5 as its own commit at row 7, and
  add `cargo nextest run --workspace -F profiling` to the block.

**40. (major) Numeric fixes lack the physics documentation, OCHRE comparison and tracing the
rules require (`feedback_best_physics`, `feedback_ochre_parity_rules` #1 and #5; r2-v4 M15).**
- Correction: add to the `09-numeric-fixes.md` preamble: every B entry adds a
  `docs/PHYSICS_DECISIONS.md` entry (reference, OCHRE behaviour, delta direction) and `tracing`
  for the changed path.

**41. (major) The one-schema contract and several Python-facing musts are uncovered.**
- X-5 (M): no entry makes units, kind and mode dictionary survive CSV (no sidecar or header),
  no cross-path schema equality test, and no replacement for `hares_mode_map`,
  `basement_lighting_created`, `garage_lighting_created`; PY-10 mode names.
- PY-4 (M): the `step()` dict's tz-aware `datetime` (`conversions.rs:165-169`) is not changed.
- PY-6 (M): stubs matching bindings and Rust warnings retrievable from Python.
- X-11 (S): no test that a Python actor's declared variable appears in frames and files with
  units.
- Correction: add entries or bullets with `schema_equal_across_parquet_csv_polars_fleet`,
  `step_time_is_naive_local`, `stubs_cover_every_settable_field`,
  `python_actor_output_variable_appears_with_unit`.

**42. (major) OR-4's diagnostics "Destination variant" is not in the §2.2 contract.**
- Ledger: `plan.md` §2.2's `Destination` has four variants; OR-4 invents a reserved-name report.
  DV-5 (in memory, at verbosity 0) is not tested.
- Correction: define the diagnostics report in §2.2 (a catalogue report with any destination) and
  add `diagnostics_in_memory_at_verbosity_0`.

**43. (major) BR-6 permits advisory exceptions the decision forbids.**
- Source: `dl-i08-architecture.dependencies` ("deny.toml carries no advisory exceptions").
- Ledger: "If a later upgrade leaves one, its entry names the advisory ID ...".
- Correction: delete the sentence; state that any new advisory is fixed in the entry that
  introduces it. Also add `cargo deny check` to CI.

### Minors

44. (minor) Schedule: BR-5, SL-1, SL-2, SL-4, SL-5 and SL-6 have no rows (they are fixed by other
    rows; say so in those rows); OD-5 (row 31) "or folds into row 30" cannot fold into an earlier
    commit; CN-2a has no decided commit.
45. (minor) `finish()` returns `RunResult` by value yet is idempotent; return `Arc<RunResult>` or
    state the clone cost.
46. (minor) RT-2 puts PV roof coverage both in the geometry key and "applied at read time"; pick
    one and state the cache memory budget.
47. (minor) RT-3 edits `hares-envelope`; cite `dl-i08-process.scope`, which lifts
    `feedback_no_touch_thermal_solver`.
48. (minor) A fleet with heterogeneous `start_time`/`time_res` still falls into today's
    intersection (`aggregation.rs:580-610`); make it an assembly error.
49. (minor) OD-2 omits V44's "legacy dict path" (`gym_env.py:111`) and has no R23 test
    (`MaxChargingPower=11500` rejected).
50. (minor) B-5 should name the remaining name classifiers it deletes:
    `display_name_to_end_use_key` (`columns.rs:321`), `parse_end_use_electric_power_column[_key]`
    (`columns.rs:374,382`), the `" #"` stripping (`metrics.rs:1254,1281`), `extract_unit_from_name`.
51. (minor) V45: name the deletion of `batches_or_steps_to_polars_df` and `steps_to_polars_df`
    (`conversions.rs:20-49`) in PY-1 or OR-6.
52. (minor) CI mirrors only part of the block (BR-4 adds `dst`); add `--all-targets
    --all-features` clippy, plain release and `cargo deny`.
53. (minor) B-6 says the validity bitset is "already part of OR-2's RowPlan"; OR-2's change does
    not mention it.
54. (minor) CN-2's step 2 sentence about `wh_energy_factor`-style probes is unclear; also fix the
    stale `registry.py:25-28` docstring.
55. (minor) R13: numeric deltas live only in gitignored `02-measurement.md`; add a plain public
    changelog line per numeric commit.
56. (minor) `plan.md` §2.10 should state why R10's error category is not built (the decision log
    is silent), so the omission is a recorded choice.

---

## Appendix A: decision log

| Decision | Ledger location | Status | Finding |
|---|---|---|---|
| dl-dcbe0baa (flow recipe) | n/a | n/a | |
| dl-i08-criteria | whole ledger | partial | 4, 9, 34 |
| process.purpose / found_defects | plan §1, 11 | resolved | |
| process.plan_form | all entries | partial | 37 |
| process.readiness | plan §1, §3 | resolved | |
| process.independence | plan §3 | resolved | |
| process.documents | plan, ledger | contradicted | 16 |
| process.writing | plan §3, SL-3 | partial (no em dashes found; history and dangling IDs) | 16 |
| process.branch | plan §3 | partial | 18 |
| process.scope (seams) | none | missing | 5 |
| architecture.results | OR-6, OR-9 | partial | 17, 23, 45 |
| architecture.output_model | OR-1, OR-2, OR-8 | partial | 6, 25 |
| architecture.destinations | OR-3, OR-4 | partial | 7, 32, 42 |
| architecture.python_export | PY-1 | resolved | 51 |
| architecture.config | OD-2, PY-2 | contradicted/missing | 8 |
| architecture.inputs | RT-4 | partial (content sources missing) | 5, 22, 24 |
| architecture.new_capability | none for 6 of 8 items | missing | 5, 6, 4 |
| architecture.health | OR-5, RT-5 | contradicted | 4 |
| architecture.time | OR-7, B-6 | resolved (Python step path partial) | 41 |
| architecture.weather | B-1 | contradicted | 2 |
| architecture.aggregation | B-11 (fleet only) | partial | 6 |
| architecture.fleet | FL-1..FL-4 | partial | 33, 10 |
| architecture.numeric_fixes | B-1..B-12 | contradicted (item 1 relabelled; B-7, B-12 no own commit) | 2, 3 |
| architecture.precedence | OD-2, B-13, CN-3 | partial | 27, 12 |
| architecture.reproducibility | FL-1, RT-5, B-8 | partial | 10 |
| architecture.package | none; OD-1 opposite | contradicted | 1 |
| architecture.engine_performance | RT-1..RT-5 | partial | 9, 34 |
| architecture.dependencies | BR-6 | partial | 43, 36 |
| architecture.stray_outputs | BR-7 | partial | 15 |
| architecture.consumer | schedule, CN-1, CN-4 | contradicted | 3, 21 |

## Appendix B: binding rules (`feedback_*.md`)

| Rule | Status | Finding |
|---|---|---|
| actor_telemetry | partial (Python actor channel untested) | 41 |
| always_review, no_self_review, fix_all_review_findings | resolved (plan §3) | |
| ashrae_not_ochre, best_physics, ochre_parity_rules, not_ochre_parity_target | partial | 2, 26, 40 |
| code_quality, hot_loop_minimal | partial | 9, 34 |
| custom_domains_perf | missing | 34 |
| design_for_all_users | resolved | |
| deterministic_tests | contradicted (PY-3, FL-1) | 14, 10 |
| equipment_self_contained | resolved (no change) | |
| fix_existing_not_new_features | resolved (operator-authorised capabilities only) | |
| hot_loop_minimal | partial | 34 |
| local_time_only | resolved (M-2 criterion wrong) | 36 |
| max_5_agents, no_parallel_same_files, opencode_stateless, no_blanket_kill | n/a to ledger | |
| no_ai_slop_public, no_ticket_refs, no_useless_comments | partial | 16 |
| no_backward_compat, no_legacy_shims | contradicted (OD-1 `ochre_next`, PY-5 default reward, `ochre_compat`) | 1, 11, 2 |
| no_broken_windows | partial | 9, 34 |
| no_code_fixes_from_reviews | n/a | |
| no_destructive_git, no_git_checkout, no_git_revert, no_git_stash | partial (nested pin checkout unapproved) | 19 |
| no_ignore_tests | contradicted | 13 |
| no_sed | n/a to ledger | |
| no_silent_defaults | partial | 8, 12, 30 |
| no_timelines | resolved | |
| no_touch_thermal_solver | superseded by `process.scope`; cite it | 47 |
| pay_for_what_you_need | partial | 4 |
| shipwright_files_authoritative | contradicted | 16 |
| si_units, si_units_strict, unit_boundary, use_uom | resolved (R6 list must be SI) | 24 |
| test_coverage_reporting | n/a | |
| test_quality | contradicted (B-2 constant pin) | 14 |
| tests_every_ticket | resolved (block after every row; red rows) | 39 |

## Appendix C: context.md items 1-21

| # | Item | Entry | Status | Finding |
|---|---|---|---|---|
| 1 | Telemetry clone, HashSet | RT-1, SL-1 | resolved: "replaces the per-step full clone with per-instance, index-addressed telemetry slots" | |
| 2 | Forcing reuse keyed on geometry | RT-2 | resolved | 46 |
| 3 | Parse once per home | RT-4 | partial (defaults) | 22 |
| 4 | Envelope solar/incidence, LU, m_scratch, slot map | RT-3 (LU only) | missing | 9 |
| 5 | Thermal input preparation | RT-3 | resolved (profile-gated) | |
| 6 | Threads, per-run statics | RT-5, CN-10 | contradicted ("safe under threads as-is") | 4, 34 |
| 7 | Unused checkpoint | OR-9 | resolved | |
| 8 | Output pipeline cost | OR-2, PY-1, PR-4 | resolved | |
| 9 | HARES#28 | OR-3 | resolved | |
| 10 | Health only in observe | OR-5 | partial/contradicted | 4 |
| 11 | Stepped runs lose output | OR-6 | partial (no Python surface) | 17 |
| 12 | Diagnostics CSV | OR-4 | resolved (contract gap) | 42 |
| 13 | Fleet memory and status | FL-1, FL-2 | partial | 33 |
| 14 | Time strings | OR-7, FL-4 | resolved | |
| 15 | Profiler HARES#31 | PR-1..PR-4 | partial (unattributed 29%) | 22 |
| 16 | Consumer call sites, contract() | CN-1 | partial | 22 |
| 17 | Worker pools | CN-5 | partial | 22 |
| 18 | Duplicate parent runs | CN-6 | resolved (failure mode undecided) | 37 |
| 19 | Fit path computes only what it reads | CN-7, CN-8 | partial | 20 |
| 20 | Provenance and replay | CN-11 | resolved | |
| 21 | Surrogate trial | CN-12 | resolved | |

## Appendix D: review.md V1-V50

| V | Entry | Status | V | Entry | Status |
|---|---|---|---|---|---|
| V1 | OR-5 | resolved | V26 | OR-6 | resolved |
| V2 | OR-5 | resolved (NaN fails) | V27 | RT-1 | resolved |
| V3 | OD-1 | contradicted (module path), 1 | V28 | OR-10 | partial, 31 |
| V4 | B-1 | missing (declared correct), 2 | V29 | OR-8, B-7 | resolved (ordering 3) |
| V5 | B-1 | partial (bundled, no surface), 2 | V30 | OD-5, FL-1 | partial (RAYON), 10 |
| V6 | B-4 | resolved | V31 | OD-4 | resolved |
| V7 | OR-9 | resolved | V32 | PY-3 | resolved (test rule), 14 |
| V8 | B-8 | partial (unreachable), 28 | V33 | PY-4 | resolved |
| V9 | B-5 | resolved | V34 | OR-3 | resolved |
| V10 | B-5 | resolved | V35 | OD-2 | resolved |
| V11 | OD-2 | resolved | V36 | none | missing, 12 |
| V12 | OR-6 | resolved | V37 | OD-2 | resolved |
| V13 | OR-4 | resolved | V38 | OD-2 | resolved |
| V14 | B-6 | resolved | V39 | OD-2 | resolved |
| V15 | PY-5 | contradicted, 11 | V40 | OD-2 | partial, 49 |
| V16 | FL-2, OR-8 | resolved | V41 | FL-3 | resolved |
| V17 | FL-2 | resolved | V42 | B-9 | partial, 30 |
| V18 | PY-6 | partial (dirty), 38 | V43 | OD-2 | partial (undecided), 37 |
| V19 | B-10 | partial, 29 | V44 | OD-2 | resolved (legacy path), 49 |
| V20 | PY-2 | resolved | V45 | OR-3, OR-6 | partial (implicit), 51 |
| V21 | refuted; regression test exists | n/a | V46 | OD-3 | resolved |
| V22 | B-2 | resolved | V47 | refuted | n/a |
| V23 | B-3 | resolved (max days), 26 | V48 | design | n/a |
| V24 | B-2 | partial (load criterion, outcome), 26 | V49 | planning artefact | n/a |
| V25 | RT-5 | contradicted, 4 | V50 | planning artefact | n/a |

## Appendix E: calibration requirements R1-R25

| R | Pri | Entry | Status | Finding |
|---|---|---|---|---|
| R1 | MUST | OR-3 `memory_only_produces_full_schema_no_file` | resolved | |
| R2 | SHOULD | OR-2 | partial | 6 |
| R3 | MUST | OR-5 | partial (policy, Python) | 4, 17 |
| R4 | MUST | OR-1, PY-6, B-5 | partial | 25 |
| R5 | MUST | OD-2, B-13 | resolved (V36 aside) | 12 |
| R6 | MUST | RT-4 | partial | 24 |
| R7 | SHOULD | none | missing | 5 |
| R8 | SHOULD | none | missing | 5 |
| R9 | SHOULD | none | missing | 5 |
| R10 | MUST | OD-1, OR-3 | partial (module path; parent dir) | 1, 32, 56 |
| R11 | MUST | PY-2, B-10 | partial | 8, 29 |
| R12 | MUST | OD-4, B-8, FL-1 | partial | 10 |
| R13 | SHOULD | measurement log | partial | 55 |
| R14 | MUST | PY-6 | partial | 38 |
| R15 | SHOULD | decision only | missing (rename) | 1 |
| R16 | SHOULD | PR-1..PR-4, M-3 | partial | 22 |
| R17 | SHOULD | PY-3, RT-5, CN-10 | resolved | 14 |
| R18 | SHOULD | none | missing | 35 |
| R19 | COULD | not now | seam unnamed | 5 |
| R20 | SHOULD | B-2 | partial | 26 |
| R21 | MUST docs / SHOULD API | B-1, B-4, OR-7 | partial | 2, 5 |
| R22 | SHOULD | CN-11 | partial (engine export) | 5 |
| R23 | SHOULD | OD-2 | partial | 49 |
| R24 | COULD | none | n/a (outside purpose, not decided) | |
| R25 | COULD | OR-10 | resolved | |

## Appendix F: other user requirements

| Req | Status | Finding | Req | Status | Finding |
|---|---|---|---|---|---|
| PY-1 | partial | 17 | FL-1 | partial | 33 |
| PY-2 | resolved (OR-3, OR-4) | | FL-2 | partial | 33 |
| PY-3 | partial | 32 | FL-3 | resolved (FL-2) | |
| PY-4 | partial | 41 | FL-4 | resolved (FL-2 Coverage) | 33 |
| PY-5 | resolved (B-6) | | FL-5 | missing | 32 |
| PY-6 | partial | 41 | FL-6 | resolved (B-9) | |
| PY-7 | resolved (OR-8) | 3 | FL-7 | missing | 33 |
| PY-8 | partial | 6 | FL-8 | resolved (OR-7, FL-4) | |
| PY-9 | missing | 6 | FL-9 | missing | 33 |
| PY-10 | partial | 41 | FL-10 | resolved (FL-1 measurement) | |
| PY-11 | resolved (PY-1) | | FL-11 | n/a (C, not decided) | |
| PY-12 | missing | 5 | RS-1 | partial | 17 |
| PY-13 | n/a (C) | | RS-2 | resolved | |
| PY-14 | n/a (C) | | RS-3 | partial | 8 |
| RL-1 | partial | 35 | RS-4 | partial | 17 |
| RL-2 | partial | 35 | RS-5 | partial | 23 |
| RL-3 | missing | 35 | RS-6 | missing | 23 |
| RL-4 | resolved (OR-10) | | RS-7 | resolved (OR-6) | |
| RL-5 | contradicted | 11 | RS-8 | resolved (RT-4) | |
| RL-6 | missing | 34 | RS-9 | n/a (C) | |
| RL-7 | resolved (BR-7) | | DV-1 | contradicted | 4 |
| RL-8 | missing (C) | 34 | DV-2 | partial | 10 |
| X-1 | resolved | 20 | DV-3 | resolved | |
| X-2 | resolved | | DV-4 | partial | 15 |
| X-3 | partial | 17 | DV-5 | partial | 42 |
| X-4 | resolved | | DV-6 | missing (S) | 34 |
| X-5 | missing | 41 | DV-7 | missing (S) | 22 |
| X-6 | resolved (OR-10) | 31 | DV-8 | partial | 25 |
| X-7 | partial | 8 | DV-9 | partial | 37 |
| X-8 | resolved (FL-2) | | X-10 | resolved (OR-2, OR-3) | |
| X-9 | missing | 6 | X-11 | partial | 41 |
| X-12 | resolved (verbosity presets) | | | | |

## Appendix G: earlier verification findings (blockers and majors)

Constructs that no longer exist in the ledger (plan-v3's `HaresError` contract, `ErrorKind`,
`Warmup` config enum, warm-up move to `Built -> Running`, `reset(seed, cp)`, OUT-/SCH- IDs, the
resolution log) make the findings about them not applicable.

| Finding | Status | Now |
|---|---|---|
| r2-v1 B1 stray diff red | partial | 15 |
| r2-v1 B2 clippy lints | resolved (BR-2, BR-3) | |
| r2-v1 B3 HaresError | n/a | |
| r2-v1 B4 BuildInfo crate | resolved in substance; crate undecided | 37 |
| r2-v1 B5 plain release fails | partial (red rows) | 39 |
| r2-v1 M1, M3 dirty, tests | unresolved | 38 |
| r2-v1 M2 | resolved (PY-6 after OR-1) | |
| r2-v1 M4, M12 undefined types | unresolved | 23 |
| r2-v1 M5 mode-flow | n/a | |
| r2-v1 M6, M7 invariants, non-finite | unresolved | 4 |
| r2-v1 M8 statics | partial | 34 |
| r2-v1 M9 declaration builder | unresolved | 25 |
| r2-v1 M10 valid count | unresolved | 6 |
| r2-v1 M11 finish contract | minor | 45 |
| r2-v1 M13, M14, M16, M21 | n/a | |
| r2-v1 M15 metrics before numeric | unresolved (OR-8/B-7) | 3 |
| r2-v1 M17 reward | unresolved | 11 |
| r2-v1 M18 fluid test | partial | 28 |
| r2-v1 M19 golden port | unresolved | 36 |
| r2-v1 M20 fleet reorder | resolved (chunked ordered fold) | |
| r2-v1 M22 no-clobber | unresolved | 32 |
| r2-v2 B1 push branch | unresolved | 18 |
| r2-v2 B2 venv mechanism | partial | 19 |
| r2-v2 B3 order | partial (schedule exists; B-7/B-12) | 3 |
| r2-v2 B4 opaque_r re-route | resolved (CN-3) | 27 |
| r2-v2 M1 solar rule | resolved (midpoint); scope open | 2 |
| r2-v2 M2 leap day | unresolved | 2 |
| r2-v2 M3 ResStock cycle | unresolved | 2 |
| r2-v2 M4 B0 identity | resolved (engine_build_id until PY-6) | |
| r2-v2 M5 pin mechanics | unresolved | 19 |
| r2-v2 M6 cache relocation | unresolved | 1 |
| r2-v2 M7 policy not bitwise | resolved (CN-2 split) | 21 |
| r2-v2 M8 charge_defect premise | resolved | 54 |
| r2-v2 M9 trace payload | unresolved | 19 |
| r2-v2 M10 check_losses re-pin | unresolved | 19 |
| r2-v2 M11 build_bundle | unresolved | 19 |
| r2-v2 M12 stale engine, deselection | unresolved | 15 |
| r2-v2 M13 H-1 cross-build | n/a (no H-1 claim) | |
| r2-v2 M14 LU dead, forcing cache | resolved (RT-3 measure, RT-2 defined) | |
| r2-v2 M15 fixtures creatable | resolved (`.arrow` not ignored) | |
| r2-v2 M16 defaults callers | unresolved | 29 |
| r2-v2 M17 classifiers | minor | 50 |
| r2-v2 M18 log claims | n/a | |
| r2-v2 M19 partitioned | n/a (FL-11 C) | |
| r2-v4 B1 consumer order | partial | 3 |
| r2-v4 B2 solar | resolved | |
| r2-v4 B3 weather boundaries | unresolved | 2 |
| r2-v4 B4 opaque R | resolved | 27 |
| r2-v4 B5 dropped perf scope | partial | 9, 34 |
| r2-v4 B6 Python equipment | resolved (actors only) | |
| r2-v4 B7 config twin | unresolved | 8 |
| r2-v4 M1 branch | unresolved | 18 |
| r2-v4 M2 HaresError | n/a | |
| r2-v4 M3 health gaps | unresolved | 4 |
| r2-v4 M4 callback | unresolved | 7 |
| r2-v4 M5 strict set | partial | 21 |
| r2-v4 M6 warm-up | unresolved | 26 |
| r2-v4 M7 InputSource | unresolved | 5 |
| r2-v4 M8 dirty | unresolved | 38 |
| r2-v4 M9 ResolvedModel, replay, schema | unresolved | 24, 5 |
| r2-v4 M10 null folding | unresolved | 6 |
| r2-v4 M11 FL-11 | n/a | |
| r2-v4 M12 changelog | minor | 55 |
| r2-v4 M13 DST docs | partial (standard-time rule stated nowhere; M-2 wrong) | 36 |
| r2-v4 M14 cross-platform | unresolved | 10 |
| r2-v4 M15 PHYSICS_DECISIONS | unresolved | 40 |
| r2-v4 M16 pin mechanics | unresolved | 19 |
| r2-v4 M17 rechunk | resolved (PY-1) | |
| r2-v4 M18 self-contained | resolved except dangling refs | 16 |
| r2-v4 M19 status model | resolved (`HomeStatus`); heterogeneous timing minor | 48 |
| r2-v4 M20 MUST tests absent | partial | 32, 35 |
| r2-v4 M21 log claims | n/a | |
| r2-v5 B1 solar | resolved | |
| r2-v5 B2 opaque R layers | unresolved | 27 |
| r2-v5 B3 order | partial | 3 |
| r2-v5 B4 push | unresolved | 18 |
| r2-v5 M1 Task 0.1 | unresolved | 18 |
| r2-v5 M2 fleet memory | resolved | |
| r2-v5 M3 Instant alignment | unresolved | 33 |
| r2-v5 M4 weather cycle | unresolved | 2 |
| r2-v5 M5 leap day | unresolved | 2 |
| r2-v5 M6 PSM3, wind | unresolved | 2 |
| r2-v5 M7 resample overrides | unresolved | 2 |
| r2-v5 M8 bundled numeric commits | unresolved (B-1) | 2 |
| r2-v5 M9 warm-up default | unresolved | 26 |
| r2-v5 M10, M11, M13, M14, M22, M24 | n/a | |
| r2-v5 M12 non-finite latch | unresolved | 4 |
| r2-v5 M15 metrics in structural task | unresolved | 3 |
| r2-v5 M16 valid count | unresolved | 6 |
| r2-v5 M17 callback details | unresolved | 7 |
| r2-v5 M18 verify detection | unresolved | 15 |
| r2-v5 M19 golden corpus | partial (M-2 added; defaults path open) | 29, 36 |
| r2-v5 M20 venv, B0 bisect | partial | 3, 19 |
| r2-v5 M21 defaults content | unresolved | 5 |
| r2-v5 M23 DST docs | partial | 36 |

Round 1 (verify-r1-v1..v5, 211 findings): every blocker and major was re-checked against the
ledger. Those still applicable are carried by the round-2 rows above or by: r1-v1 B1 (28),
B2 (15), B3 (resolved by BR-6), B5/F-B4 (n/a, no required kwargs), B6 (18; PR #27 fmt), M3 (25),
M4/M5 (8), M6/F-M10 (7), M7 (4), M8 (resolved: `save_checkpoint` stays fallible in OR-10),
M12 (6), M13 (41), M14 (n/a, diagnostics capture moves to OR-4), M15 (resolved BR-6), M16 (1);
r1-v2 B2 (19), B3 (1), M7 (2), M9 (3), M13 (19); r1-v3 F-B1 (resolved BR-1), F-M1 (resolved),
F-M4 (5), F-M6 (38), F-M9 (8), F-M11 (5), F-M12 (17); r1-v4 1 (4), 2 (3), 4 (resolved CN-3),
5 (2), 6 (21), 7 (24), 12 (5), 14 (22), 15 (34), 17 (17), 18 (25), 19 (38), 22 (34), 23 (5),
24 (41), 25 (32), 26 (33), 27 (6), 28 (35), 33 (40), 38 (2), 53 (34), 54 (resolved RT-2);
r1-v5 B1 (4), B2 (15), B4 (19), B5 (2), M1 (2), M2 (2), M3 (3), M4 (3), M5 (6), M6 (resolved
FL-4), M7 (33), M8 (6), M9 (6), M12 (n/a), M15 (1), M16 (38), M17 (15), M18 (29), M22 (18).

## Appendix H: schedule check (`ledger/00-schedule.md`)

| Check | Result |
|---|---|
| Every entry with its own commit appears exactly once | yes for BR-1..4, BR-6..8, SL-3, OD-1..4, M-1..3, PR-1..4, OR-1..10, FL-1..4, PY-1..6, RT-1..5, CN-1, CN-3..12, B-1..6, B-8..11, B-13 |
| Entries without a row | BR-5 (inside row 23's title), SL-1, SL-2, SL-4, SL-5, SL-6 (finding 44) |
| Rows that fold into an earlier row's commit | B-7 (row 62 into row 26), B-12 (row 66 into row 34), OD-5 (row 31 into row 30) (findings 3, 44) |
| Split entries | CN-2 as CN-2a (row 47, commit undecided) and CN-2b (row 69) |
| Dependencies respected by order | yes for all stated dependencies (OR-8 after OR-6; RT-2 after RT-4; RT-5 after RT-1/RT-4; PY-1 after BR-6/OR-3/OR-6; CN-3 after B-13; CN-10 after RT-5/PY-3; PR-4 after OR-6) |
| Ordering against decisions | violated: numeric changes at rows 26 and 34 precede the consumer gate; nalgebra (row 5) precedes the golden tool (row 13); OD-2 (row 10) precedes the B0 gate (findings 3, 36) |
| Missing rows | branch integration of PR #27 (row 0) and the final push (finding 18); entries required by findings 1, 4, 5, 6, 7, 8, 9, 17 |
