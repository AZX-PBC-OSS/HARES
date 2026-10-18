# L1 resolution: plan.md §2, ledger/03, ledger/05 (and new ledger/12)

Scope: every finding in `verify-L1-b.md`; every finding in `verify-L1-e.md`
and `verify-L1-a.md` that concerns `plan.md` §2 or `ledger/03-output-reporting.md`
/ `ledger/05-python-surface.md`. Claims re-verified against HEAD `ebdb1a17`
(read-only). New file: `ledger/12-inputs.md` (IN-1..IN-4).

During this pass `ledger/06-runtime-memory.md` was revised in parallel and
now owns content inputs (RT-4), per-run statics (RT-5) and resolved
read-back (RT-6). `plan.md` §2.7 is written as the single contract those
entries and `ledger/12-inputs.md` build on; overlaps are listed under
cross-area dependencies.

## verify-L1-b.md

| ID | Resolution |
|---|---|
| 1 (blocker) | fixed: `plan.md` §2.2 + 03 OR-14 + 05 PY-10. "`pub trait BatchCallback: Send + Sync { fn on_batch(&mut self, ctx: &BatchContext<'_>, batch: &RecordBatch) -> Result<(), CallbackError>; }`" in hares-io; `Destination::Callback` is a serialisable marker; "`Dwelling::attach_callback(&mut self, report: &str, callback: Box<dyn BatchCallback>)`"; PY-10: "when the binding turns the resulting `HaresError::Callback` into `HaresCallbackError`, it takes the stored exception and sets it as `__cause__`". |
| 2 (blocker) | fixed: 03 OR-5. "delete the release stub block (`invariants.rs:705-800`) and the field gates (`invariants.rs:11-52`)"; a table classifies every gated check in `check_invariants()` plus the `check_nan_screen`, registration, step-order and assembly sites; release tests `thermal_balance_violation_fails_in_release`, `non_finite_telemetry_fails_in_release`, and for SOC bounds the existing `ev_soc_check_fails_loudly_when_core_output_soc_is_absent` (`check_soc` is deleted as vacuous: `Soc::try_from` bounds SOC to [0, 1], so an out-of-range SOC reaches the dwelling as an absent one, which `soc_bounds` fails in every profile); seams move to `cfg(any(test, debug_assertions))`. The measurement-contingent gate is gone: "never a reason to re-gate". OR-11 covers the other crates. |
| 3 | fixed: `plan.md` §2.2 + 03 OR-1. `Unit`, `VariableKind`, `Aggregation`, `Sampling` and the new `TelemetryField` in hares-types; "`fn output_variables(&self) -> &[TelemetryField]`" required on `Actor`; "a static per-capability table for the core-output columns"; "There is no `ActorId` type; actors are identified by their registered name"; zones from "a static per-zone-template table keyed by `zone_display_name`". |
| 4 | fixed: `plan.md` §2.1, §2.3. `HealthPolicy` (fields, `strict()`, empty default, evaluated "when its count changes"), `WarmupOutcome`, `RunMetrics` = "`FullSimulationMetrics` ... renamed, ... `Deref` shim deleted", "There is no separate `MetricsStatus` type", `Warning`, `Provenance`, `StepResult` kept by value; counters wired in 03 OR-5 from the named sites (`mod.rs:1177-1195`, `1228-1233`, `1262-1279`, `take_biquadratic_clamp_count()`). |
| 5 | fixed: 03 OR-13 + `plan.md` §2.6. "`Pattern` is a glob over `VariableDef.name` ... without a new dependency"; period rows "labelled at its nominal period start"; tests `hourly_energy_sum_equals_timestep_sum`, `restore_mid_hour_matches_uninterrupted_hourly_report` (OR-10, "one `ReducerSnapshot` per non-`Timestep` report"). |
| 6 | fixed: 03 OR-3 + `plan.md` §2.7 table + 05 PY-8. "`RotationPolicy`, `OutputFormat`, the rotating writer, `default_output_path` and `resolved_output_path` are deleted"; `removed_key_<key>_names_reports`; "`frame_golden`'s fixture manifests and its Rust API calls are migrated to `reports` in this commit. The migration is config-only". |
| 7 | fixed: 03 intro test rules + OR-3. "gives the run a fresh temporary directory as its only output root and asserts the directory is empty afterwards, and asserts structurally that no file backend was constructed"; the metrics test moved to OR-8 (`metrics_present_without_any_report`). |
| 8 | fixed: `plan.md` §2.2 + 03 OR-4. "`Diagnostics` selects the variables today's diagnostics writer emits ... it works with every destination, including `Memory`"; test `diagnostics_absent_without_a_diagnostics_report`. |
| 9 | fixed: 03 OR-5. "Every counter is a run total on the dwelling, incremented unconditionally in every build and never reset per step"; `port_rollbacks` "in `rollback_failed_equipment_ports`"; `nan_temperature_count`, `unresolved_column_count` and the three schema columns deleted; drift becomes an error in OR-5 and OR-2's `RowPlan` "returns `HaresError::InvariantViolation` for an expected variable it cannot place". |
| 10 | fixed (option a): 03 OR-5 "Scheduled directly after the baseline-repair entries so the plain-release verification line is green from then on"; `01-baseline-repair.md` already orders "BR-1, BR-3, BR-2, then ... OR-5". Schedule row move is cross-area. |
| 11 | fixed: 03 OR-8. "Structural and bug-compatible; `09-numeric-fixes.md` B-7 is the numeric change, in its own later commit"; "fed per step from the row plan's producer slots"; `metrics_bitwise_unchanged_for_every_golden_fixture`. |
| 12 | fixed: 03 OR-10. Metrics snapshot "exact at the checkpoint step because OR-8 feeds it per step"; rollback fixture "the rollback test equipment" (`mod.rs:13847`); "`DateTime<Tz>` encoded as IANA zone name plus UTC instant in seconds"; "`version_8_checkpoint_is_rejected`: through `DwellingCheckpoint::deserialize_gated`". |
| 13 | fixed: 05 PY-1. "`arrow(rechunk: bool = True) -> pyo3_arrow.Table`: a `PyTable` ... `concat_batches`"; `table()` returns polars; the four conversion helpers deleted; `py_fleet.rs:303` migrated; tests `capsule_export_matches_golden_bitwise`, `report_can_be_exported_twice`. |
| 14 | fixed: 05 PY-7 (rename), PY-8 (one config type, `validate()`), 12 IN-1..IN-4, 03 OR-15, OR-16; content inputs are 06 RT-4. |
| 15 | fixed: 05 PY-3. "`construct_detached` ... calls `py.detach(f)`" for `from_hpxml`, `reset_with_seed`, `DwellingBlueprint.from_hpxml`, `DwellingBlueprint.build`; test records "`pyo3::ffi::PyGILState_Check()`"; "No threads, no timing." |
| 16 | fixed: 05 PY-5. "delete the Rust reward and the `reward` key ... add `step`"; "both branches call `self._reward_fn(ctx)` per environment"; `rust_and_python_paths_give_identical_rewards`. |
| 17 | fixed: 05 PY-6. `build.rs` in hares-core; commit "read from `.git/HEAD` and the ref it names ... without running git"; "`None` when git or `.git` is absent, never a guess"; digest over a documented field encoding; no dirty-tree mutation test. |
| 18 | fixed: 03 OR-2. "`output_column_index` is removed from `Dwelling` ... (compile-enforced)"; test `row_plan_places_values_at_golden_column_indices`; validity bitset and `i64` timestamp in the change; RT-1 named for the telemetry lookups. |
| 19 | fixed: 03 OR-1. "`is_ev`, a substring `contains("ev")`"; pinned schemas `crates/hares-io/tests/fixtures/schema/v{0..8}.json` replace a `frame_golden` mode; fixture enumerated from `EquipmentRegistry::known_names()`. |
| 20 | fixed: 03 OR-5 cites `mod.rs:6258-6287` and `6175-6197`. |
| 21 | fixed: 03 OR-5 "`nan_zone_temperature_during_warmup_fails`: a test equipment adds a NaN sensible gain". |
| 22 | fixed: 03 OR-6 cites `finalize_billing` (`py_dwelling.rs:1827`); `plan.md` §2.1 "`step()` after `finish()` is `HaresError::Simulation`"; 05 PY-9 deletes Python `finalize_billing()`. |
| 23 | fixed: 03 OR-7 "passed as `&str` into `StreamingRecorder::push_row` (`writer.rs:368-379`, which copies it); the fleet path parses it back (`aggregation.rs:509`) and renders ... (`aggregation.rs:636`)". |
| 24 | fixed: 03 OR-9 "delete the field and all six sites"; "the field deletion is compile-enforced". |
| 25 | fixed: 05 PY-1 "`metrics()` ... clones batches but does not use IPC". |
| 26 | fixed: 05 PY-2 test uses `master_seed=`; dwelling-level keywords listed; "any other unknown keyword is Python's own `TypeError`". |
| 27 | fixed: 05 PY-4 cites `columns.rs:43-53`; "`variables() -> list[Variable]`". |
| 28 | fixed: `plan.md` §2.2 "An existing target with `overwrite: false` is an error at freeze"; "A run that fails removes its partial files". |
| 29 | fixed: same as 12 (OR-10 through `deserialize_gated`). |
| 30 | fixed: 05 PY-1 "brings `thiserror` 1 beside the workspace's `thiserror` 2, a duplicate version BR-6's `cargo deny` configuration must accept". |

## verify-L1-e.md (findings in scope)

| ID | Resolution |
|---|---|
| 1 (blocker) | fixed: 05 PY-7. `name = "hares-py"`, `module-name = "hares._hares"`, caches `<cache base>/hares/...`, "No `ochre_next` alias package", census `git grep -n -i -E 'ochre[_-]?next|\.cache/ochre'` with an explicit exclusion list; tests `import_ochre_next_fails`, `cache_dirs_are_under_hares`, `distribution_name_is_hares_py`. OD-1 retarget and consumer entry: cross-area. |
| 3 (blocker, OR-8 part) | fixed: 03 OR-8 bug-compatible, B-7 separate. B-12/FL-4, CN-1 gate and OD-2 placement are not in my files: cross-area. |
| 4 (blocker) | fixed: (1) 03 OR-5 classification table, "No check is exempt during warm-up" (`plan.md` §2.3), with a stop condition instead of an exemption; (2) non-finite telemetry: "a latch on `Telemetry` ... fails with `NanDetected` naming the source and key"; (3) process-global counters deleted, run totals on the dwelling (OR-5), test `rejected_signal_counted_per_run`; (4) 03 OR-12 `HealthPolicy` with `strict_policy_escalates_port_rollback_naming_step_and_source`, `empty_policy_never_escalates`. |
| 5 (blocker) | fixed: `validate()` 12 IN-2 (`validate_rejects_afue_above_one_with_run_time_message`); `metrics_from_table` 03 OR-15; `resample_weather` 12 IN-4 (`resample_weather_equals_run_weather_columns_at_900_and_3600`); replay 03 OR-16 (`replay_export_reproduces_frame_bitwise`); input schema 12 IN-3 (`input_schema_overrides_match_enforcement`); content inputs 06 RT-4 plus 12 IN-1 digests; seams named in `plan.md` §2.10. |
| 6 (blocker) | fixed: 03 OR-13 and `plan.md` §2.6 (Sum/Mean/Mode per dwelling, `Step Count`, "`<name> [valid steps]`" companions for nullable variables, partial periods); OR-2 adds `unknown_selection_name_errors_listing_unknown_names` and `single_column_projection_bitwise_equals_full_verbosity_column`. |
| 7 (blocker) | fixed: see L1-b 1; plus `callback_context_carries_bldg_id_and_report_name`, `provenance_records_callback_destination_marker`, `no_callback_report_constructs_no_adapter`. |
| 8 (blocker) | fixed: 05 PY-8 ("`SyntheticOutputConfig` and `SyntheticSimulationConfig` are deleted"; one defaults table; `python_and_toml_defaults_come_from_one_table`; `synthetic_toml_rejects_unknown_key`), `plan.md` §2.7 removed-key table, 03 OR-3 (removes `RotationPolicy`, `default_output_path`, `resolved_output_path`; census command over `*.rs *.toml *.py *.pyi *.md`). Consumer `rotation` pass-through: cross-area. |
| 11 (blocker) | fixed: see L1-b 16. |
| 14 (blocker, PY-3 part) | fixed: see L1-b 15. FL-1 and B-2 parts are not in my files. |
| 16 (blocker, my files) | fixed: `plan.md` §2.10 rewritten as "Not built now, and the seam each keeps" with no draft history; 03 OR-2 cites `context.md` item 8 instead of `review.md` §5; 05 PY-1 cites `context.md` for the conversion figures; PY-3 no longer cites "H-3"; PY-6 no longer cites "an earlier defect table entry". |
| 17 (blocker) | fixed: 05 PY-9 (`Dwelling.finish()`, `Dwelling.health()`, report classes, `SimulationPlan`/gym/vec env/HELICS call `finish()`, stubs) with `python_stepped_finish_equals_simulate` and `health_readable_with_no_reports_requested`; 03 OR-6 "`SimulationEngine::run`/`run_dwelling` return `Arc<RunResult>` through one shared function ... Typed errors are returned unchanged". |
| 20 | rejected: an engine metrics opt-out is a capability the decision log does not decide ("Metrics do not depend on verbosity or on the selected reports"); `RunResult.metrics` stays always computed. Recorded as operator question Q1. |
| 21 | fixed (engine part): `plan.md` §2.3 `HealthPolicy` names the escalatable set (`max_port_rollbacks`, `max_rejected_control_signals`, `max_clamped_actions`, `max_curve_index_clamps`, `require_warmup_convergence`); an unresolved variable is an error, not a count. CN-2b's baseline handling: cross-area. |
| 22 (item 3) | fixed by 06 RT-4 (`ParsedDefaults` shared) and 12 IN-1 (defaults digests); item 16 is 08's (the digest it needs is 05 PY-6's). |
| 23 | fixed: `plan.md` §2.1 defines every result type with fields; "`take_warnings()` returns the warnings added since the previous call, through a cursor, without removing them from the run record" (03 OR-6); `warmup_nonconvergence_recorded_and_warned` (03 OR-5). |
| 24 | fixed: `plan.md` §2.7 `ResolvedModel` with fields and the required-parameter table; 12 IN-2 tests; read-back tests in 06 RT-6. |
| 25 | fixed: 03 OR-1 item 4 ("keeps that outcome exactly by calling those same functions ... B-5 replaces those calls"), `catalogue_queryable_before_run`, `every_written_channel_is_declared_and_every_declared_variable_is_written`. |
| 26 (my part) | fixed: `WarmupOutcome` defined (`plan.md` §2.3) and recorded by 03 OR-5; `frame_starts_at_start_time_with_exact_row_count` in 03 OR-7. Default maximum days and load convergence are 09 B-2's. |
| 31 | fixed: 03 OR-10 `TariffSnapshot` includes `step_index`; warnings checkpointed; tests `restore_resumes_tariff_price_index`, `restore_resumes_warnings`. |
| 32 | fixed: `plan.md` §2.2 unique partial name, publish by `hard_link` unless `overwrite`, freeze-time check, duplicate paths a `validate()` error; 03 OR-3 tests `existing_target_without_overwrite_fails_before_first_step`, `missing_parent_dir_is_io_error_naming_path`, `duplicate_report_path_is_a_validate_error`. Fleet-wide duplicate paths: cross-area (04). |
| 36 (my part) | fixed: 03 OR-3 migrates `frame_golden` manifests and API; 03 OR-7 "`frame_golden` compares `Time` as `i64`"; 03 OR-8 records pre-change metrics bit patterns itself. M-1 metrics/health capture, DST criterion, consumer-shape fixture and nalgebra ordering are 02's. |
| 37 (my part) | fixed: OR-5 has no measurement-contingent gate; PY-6 puts `BuildInfo` in hares-core ("`crates/hares-core/build.rs` (new)"). |
| 38 | fixed: 05 PY-6 `rerun-if-changed` on `.git/HEAD`, the resolved ref, `.git/index`, `crates/`, `Cargo.toml`, `Cargo.lock`; resolution tested against synthetic `.git` directories. The proposed `build_info_dirty_after_uncommitted_edit` is replaced by `porcelain_parse_reports_dirty` because the binding test rules forbid running git inside tests. |
| 39 (my part) | fixed: OR-5 placement (see L1-b 10). The `-F profiling` block line is §3's. |
| 41 | fixed: `schema_equal_across_destinations` (03 OR-3); `step_time_is_naive_local` (03 OR-7); `stubs_cover_every_binding` (05 PY-9); `python_actor_output_variable_appears_with_unit` (05 PY-11); mode names via `VariableKind::Enum { labels }` and `enum_codes_decode_to_labels` (05 PY-4); `hares_mode_map`, `basement_lighting_created`, `garage_lighting_created` deleted (`plan.md` §2.5). The CSV sidecar sub-proposal is rejected: `Catalogue::to_json()` and `RunResult.catalogue` already carry units and kinds for every destination; a second per-file channel would duplicate it. |
| 42 | fixed: see L1-b 8; `diagnostics_in_memory_with_verbosity_0_report`. |
| 45 | fixed: `plan.md` §2.1 "`finish(&mut self) -> Result<Arc<RunResult>>` ... Later calls return the same `Arc`"; test `finish_is_idempotent_and_returns_the_same_arc`. |
| 51 | fixed: 05 PY-1 deletes `batches_or_steps_to_polars_df`, `steps_to_polars_df`, `record_batches_to_polars_df`, `record_batch_arc_to_polars_df`. |
| 53 | fixed: 03 OR-2 "one reused `Vec<f64>` row, a validity bitset and an `i64` local timestamp". |
| 56 | fixed: `plan.md` §2.10 "An error-category taxonomy over `HaresError` is not built: the decision log does not call for one". |

Not in scope of this pass (other owners): e2, e9, e10, e12, e13, e15, e18,
e19, e27-e30, e33-e35, e40, e43, e44, e46-e50, e52, e54, e55.

## verify-L1-a.md (findings in scope)

| ID | Resolution |
|---|---|
| B1 | fixed (option a): 03 OR-5 scheduled directly after BR-1, BR-3, BR-2; `01-baseline-repair.md` already states that order. |
| B5 | fixed: see L1-b 1. |
| M17 | fixed: every type used by the ledger is defined in `plan.md` §2 with its crate (`Unit`, `VariableKind`, `Aggregation`, `Sampling`, `Selection`, `Frequency`, `WarmupOutcome`, `HealthPolicy`, `HealthEvent`, `EquipmentHealthCounts`, `Coverage`, `ReportResult`, `ReportOutput`, `RunMetrics`, `Warning`, `Provenance`, `BuildInfo`, `InputSource`, `WeatherInput`, `DefaultsSource`, `InputRecords`, `ResolvedModel`, `InputSchema`, checkpoint snapshots); the two `Source`s are `VariableSource` and `InputSource`; no `ActorId` (registered name); `HealthPolicy` is a struct with `strict()`. |
| minor 10 | fixed: `plan.md` §2.7 "`crates/hares-equipment/src/hvac/heat_pump_config.rs:262,297,483`". |
| minor 11 | fixed: `plan.md` §2.2 "`crates/hares-core/src/dwelling/conversions.rs:590`". |

## Cross-area dependencies other files must honour

- `ledger/00-schedule.md`: order BR-1, BR-3, BR-2, OR-5, OR-11, PY-7, then
  the rest of baseline repair, OD-1 (after PY-7), OD-2, PY-8, PY-2, M-1,
  M-2, OR-1, OR-2, OR-3, OR-13, OR-14, OR-4, OR-12, OR-6, OR-7, OR-8, OR-9,
  OR-10, OR-15, PY-9, PY-1, PY-3, PY-4, PY-5, PY-6, PY-10, PY-11, RT-4,
  IN-1, RT-6, IN-2, IN-3, IN-4, OR-16, all before the B0 gate. New rows:
  OR-11..OR-16, PY-7..PY-11, IN-1..IN-4. Row titles and commits for OR-5,
  OR-8 and OR-12 changed. B-7 gets its own row and commit after the gate.
- `plan.md` §1: list `ledger/12-inputs.md`; 03 now also covers invariant
  enforcement, reporting frequency, the callback, `metrics_from_table`
  and replay; 05 covers the package rename and the one config type.
- `plan.md` §3: after OR-11 the `check_invariants` feature no longer
  exists; drop the `-F check_invariants` line (OR-11's commit does it).
  Python lines should match the commands the entries use
  (`uv run --no-sync maturin develop --release`,
  `uv run --no-sync pytest tests/python`, `rl` extra installed).
- `plan.md` §5: add operator questions Q1-Q3 below.
- `01-baseline-repair.md`: BR-5 closes with OR-5 (as stated); BR-6's
  `cargo deny` configuration must accept the duplicate `thiserror` from
  `pyo3-arrow`.
- `02-measurement.md`: "OR-5 adds `RunResult.health`" should read OR-6
  (which assembles `RunResult`; OR-5 records `RunHealth`); manifests using
  `output_verbosity` migrate to `reports` in OR-3's commit.
- `04-fleet.md`: `RunHealth` is OR-5 and `HealthPolicy` is OR-12
  (`04-fleet.md:160`); fleets consume `SimulationEngine` returning
  `Arc<RunResult>`; a home with non-empty health or warnings is
  `Degraded` (`plan.md` §2.4); `Coverage` fields per §2.4; duplicate
  per-home file paths are a fleet construction error; schema metadata key
  is `hares:utc_offset_s`.
- `06-runtime-memory.md`: RT-4 uses §2.7's names (`InputSource`,
  `WeatherInput`, `DefaultsSource.files`), not `Source::{Path, Text,
  Bytes}`; the parsed handles expose `record()` (12 IN-1). RT-5's rows for
  `REJECTED_SIGNAL_COUNT`, `ACTIONS_CLIPPED` and `NON_FINITE_REJECTIONS`
  are implemented by 03 OR-5 (RT-5 should cite them as done there, not
  repeat the change). RT-6 adopts §2.7's `ResolvedModel` shape
  (`SetpointSource`, `windows`, `c_j_k`) and required-parameter table, and
  computes it at assembly before warm-up.
- `08-consumer.md`: CN-2b arms `HealthPolicy::strict()` or explicit limits
  over the §2.3 fields and cites OR-12; CN-1 cites OR-5 for `RunHealth`;
  CN-7 cannot rely on `RunResult.metrics` being `None` unless Q1 is
  decided that way; the consumer's `rotation` and other removed keys
  migrate to `reports`; `from_hpxml` takes `config=SimulationConfig(...)`
  (PY-2); the consumer imports `hares` (PY-7 lands first).
- `09-numeric-fixes.md`: B-7 is its own commit after the gate (OR-8 is
  bug-compatible); B-5 deletes the transitional classifier call in the
  catalogue builder (03 OR-1 item 4) and the classifiers; B-11 covers the
  fleet fold only (per-dwelling Mode is OR-13); B-2 changes the criterion
  `WarmupOutcome` reports; B-10 makes `DefaultsSource.base` required; B-1
  re-runs IN-4's test; B-6 uses OR-2's validity bitset.
- `11-other-defects.md`: OD-1 registers exceptions under `hares._hares`
  after PY-7 and includes `HaresHealthPolicyError` and
  `HaresCallbackError` (PY-9) in its pickle test; OD-2 lands before PY-8
  with `deny_unknown_fields` on `SimulationConfig` and the
  `time_res_secs_u32` fallback removed, and its override acceptance uses
  the `DescribeFields` lists (IN-3); PY-8 migrates the gym dict path's
  keys, OD-2 owns its coercion fixes.
- `10-slop-cleanup.md`: OR-5 removes the dispatch-site
  "silent in release builds" error blocks.

## Operator questions

- **Q1.** May a run opt out of computing `RunResult.metrics`? Needs item
  X-1 asks for "independent opt-ins", and the consumer's fit path reads
  one column. The decision log says metrics do not depend on verbosity or
  reports but does not decide an opt-out. The plan computes them always
  and records their cost in OR-8.
- **Q2.** The dated records under `docs/reviews/`, `docs/tickets/`,
  `docs/findings/`, `docs/superpowers/` and `scripts/reviews/` name
  `ochre_next`. Keep them as they are (the plan's current exclusion),
  rewrite them, or remove them from the public repository?
- **Q3.** Should the machine-readable input schema also enumerate the
  HPXML elements the parser reads? R8 lists them; the plan's schema
  covers config keys, dwelling keys, override targets, schedule columns
  and defaults files.

## Counts

- verify-L1-b: 30 findings; 30 fixed, 0 rejected.
- verify-L1-e (in scope): 30 findings; 29 fixed (38 with a stated test
  substitution; 41 with its CSV sidecar sub-proposal rejected), 1
  rejected (20). 26 findings are other owners'.
- verify-L1-a (in scope): 5 findings; 5 fixed.
- Operator questions: 3.
