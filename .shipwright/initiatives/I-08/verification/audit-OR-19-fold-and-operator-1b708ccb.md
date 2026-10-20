# Audit: OR-19 fold integrity and operator commits, head 1b708ccb

Scope: part A, the OR-19 fold `e5b9ee42..2f85338d`; part B, the operator commits in `8082592e..1b708ccb`.
Method: patch-id and line-set comparison against the reviewed source `fix-or19` tip `21c4fac0`; ledger entry against diff for each operator commit; a per-commit `cargo check --workspace --all-targets --all-features` walk; full nextest debug line and both golden lines on the head; assembly timing.
Scratch: worktrees under `.workstrees/` and `target-audit-head2` created and deleted. The main tree was not touched.

## Verdict

Part A: nothing is lost. Version 11 is consistent. The mutation record matches the code (static match only, see A3).
Part B: one high finding (goldens red at head), one high finding (test compile broken at head, same class as the known one), and several medium and low ones.

## Findings by severity

### H-1 `[golden]` fails on the head: defaults digest not re-captured after a defaults edit (952e5486)

- Commit 952e5486. File `defaults/Default Schedule Parameters.csv` (the Ceiling Fan `month_multipliers` row), no change under `tests/fixtures/golden/`.
- Scenario: any `frame_golden compare-all` run, default or `-F dst`.
- Evidence: `cargo run --release -p frame_golden -- compare-all` on the head. All 7 goldens report FAILED, each with exactly one difference: `defaults "defaults": digest mismatch: golden 04eb8f10..., run 3f9c3a95...`. The `-F dst` line fails the same way (`full_year_900s_dst`). `consumer_shape_900s` shows golden 4baeb1ba against run 5339acb2. `compare_run` folds the digest into the same comparison as the frames, and no frame delta is listed, so every product column is bitwise equal and the only difference is the digest. `git log 6c33e689..1b708ccb -- defaults` lists only 952e5486; `git log 6c33e689..1b708ccb -- tests/fixtures/golden` lists nothing.
- Why it matters: 6c33e689 re-captured the same goldens for the same cause (a deleted file under `defaults/`) one commit earlier, so the rule was known. The ledger's OD-17 verification says any golden difference stops the entry; nothing stopped it. The cause (the added CSV row) is real and bitwise neutral for frames, but is unrecorded.
- Fix: re-capture all seven goldens from a clean committed tree with 952e5486 in it, and state the cause in the commit.

### H-2 hares-io test target does not compile at 952e5486 and 1b708ccb

- Commit 952e5486, `crates/hares-io/tests/schedule_integration.rs:189`.
- 952e5486 adds `equipment_without_any_schedule_source_is_an_error` calling `inject_schedule_into_specs` with 6 arguments. 8c034e4f, earlier, gave the function a seventh (`garage_modeled: bool`).
- Evidence: the per-commit walk: `ok 1aa077ff`, `FAIL 952e5486`, `FAIL 1b708ccb`, `E0061 this function takes 7 arguments but 6 arguments were supplied`. The first `cargo nextest run --workspace --no-fail-fast` on the head aborts at build with the same error. It is fixed later by `a416b901` (a one-line test change).
- Impact: `cargo nextest` and `cargo clippy --all-targets` fail on the head and break bisects through that range.
- Same class as the two known cases. The full set found by the walk is in the compile table below.

### M-1 OD-42 "FIXED" but its Acceptance is not met for the blueprint bag channel (ee7bee85)

- `crates/hares-core/src/dwelling/conversions.rs`, `land_spec_parameters`: `if key == KEY_EQUIPMENT_ID || !landed.contains_key(key) { continue; }`.
- The ledger's Change: an override naming an unknown field is a typed error; no parallel untyped bag survives. Acceptance: for each equipment class an override of a typed field changes behaviour.
- Scenario: a blueprint spec whose bag overrides a typed field that is `None` in the payload. Most HVAC, heat-pump and water-heater configs serialize `Option` fields with `skip_serializing_if = "Option::is_none"` (for example `cooling_config.rs` `fan_power_w`, `shr`; 50 sites in that file alone). The payload then has no such key, so the override is skipped silently. The commit's own doc says so ("silently dropped ... known limitation").
- OD-46 describes only a typo'd key. It does not say that a legitimate override of an unset optional field is also dropped. Both are the silent-ignore class the entry was filed to close.
- Test coverage: only PV, water-heater setpoint, battery control and dehumidifier overrides are exercised (`equipment_override_typed_configs.rs`, `test_equipment_overrides.py`). No test for an override of a `None` optional field on any class.
- Fix: error when a bag key is not a payload field and not a known machinery key, and add a test per equipment class family, or fold the case into OD-46 explicitly.

### M-2 Goldens recaptured from a dirty tree with a provenance hash that is not in history (6c33e689)

- `tests/fixtures/golden/*.golden.json`: `"git_dirty": true`, `"git_head": "81818880..."`. 81818880 is a pre-rebase copy of 8c034e4f and is not an ancestor of the head (`git merge-base --is-ancestor` fails).
- Precedent: b7ea5d0a "re-capture the fleet golden from a clean committed tree".
- Cause of the digest move is recorded (two unread CSVs deleted) and frames are byte equal, so no numeric concern, only provenance.

### M-3 Pyright gate cannot catch the floor-version break; TypeIs confirmed (787487df)

- Confirmed: `python/ochre_next/data/resstock.py:20` and `python/ochre_next/helics/_types.py:3` import `typing.TypeIs` (3.13+). `python3.12 -c "from typing import TypeIs"` raises ImportError. `requires-python >=3.12`; CI pytest matrix runs 3.12.
- Root cause for the class: `[tool.pyright]` in `pyproject.toml` sets no `pythonVersion`, and the CI job runs on 3.13 only (`ci.yml` pyright job). Running `pyright --pythonversion 3.12` over `python typings tests scripts` reports exactly these two imports (plus consequences) and nothing else. Fix `4061da95` covers the symptom; add `pythonVersion = "3.12"` to close the class.

### M-4 Documented usage changes behaviour without being recorded (952e5486)

- `crates/hares-core/src/dwelling/blueprint.rs`: `defaults_path: Some(resolved_defaults_dir)` became `config.defaults_path.clone()`, and `mod.rs` no longer substitutes `"defaults"` for the schedule resolver. A config with no `defaults_path` now has no default schedule profiles, while `DefaultsStore` still loads from the `"defaults"` fallback.
- `Dwelling::from_hpxml` (no defaults) doc was edited, and the warmup regression had to pass a defaults path, because the BEopt example schedule does not name every mapped column. `docs/python.md` "Usage example" calls `Dwelling.from_hpxml(hpxml, schedule, weather)` with no defaults path, so a schedule that omits a mapped column now fails where it used to resolve from the fallback directory.
- The commit message does not mention this. It is in the spirit of the entry (no silent default), but it is a behaviour change on a public path and should be stated, with the docs example checked.

### L-1 Stale "non-critical init skip" text left after 8c034e4f and 1aa077ff

1aa077ff exists to correct comments about the removed skip. Still describing it at 1b708ccb:
- `crates/hares-core/src/dwelling/conversions.rs:862` ("an `init` that non-critical equipment survives by being skipped") and the same text inside the `expect_err` message at `:2018`.
- `crates/hares-equipment/src/config.rs:579`.
- `crates/hares-core/tests/equipment_id_regressions.rs:527` and `crates/hares-io/src/hpxml/equipment.rs:481` ("dropped non-critical equipment leaves gaps").
- Test `crates/hares-core/tests/equipment_identity_boundaries.rs:548` is still named `noncritical_init_failure_is_fatal_after_the_identity_checks`.

### L-2 Garage lighting column dropped without a trace (8c034e4f)

`crates/hares-io/src/schedule_resolve.rs` `ensure_specs_for_csv_columns`: a `lighting_garage` schedule column with no garage zone is now skipped silently. Before it produced a skip warning. The OCHRE rule is cited, but a declared schedule input is now ignored with no warning, in the commit whose purpose is to stop dropping declared loads. A debug-level record or a warning would keep the old observability.

### L-3 Boiler placeholder is the old default renamed (249912f9)

`crates/hares-equipment/src/hvac/boiler.rs:170` and `:554`: `unwrap_or_default()` became `unwrap_or(LoopId(0))` in both constructors, and the ports are declared with it in `new()`. The ledger said both constructors lose the default. `init` now fails when the typed id is absent, so no step runs on it, but the placeholder remains.

### L-4 `unwrap_or(0.0)` sentinel for the volume (18fbf72e)

`crates/hares-core/src/dwelling/solver_builder.rs` (about line 1340): `conditioned_volume_m3` is `building.conditioned_volume_m3.unwrap_or(0.0)` when no leakage input needs it. It is unreachable at the use site (constant ACH skips the AIM-2 branch), but a `0.0` stand-in would divide by zero if a future branch read it. An `Option` would make that a compile error. The ledger asked for a `MissingField`; the error is a `HaresError::Dwelling` string naming the element.

### L-5 Remaining `type: ignore` and an added one (787487df, 699cfad2)

BR-23's Change says no `type: ignore`. After 787487df, 16 remain in `tests/`, `scripts/` (35 before). 699cfad2 adds one at `tests/python/bench_4month_der.py:90`.

### L-6 Python test that needs `pvlib` is unguarded (901354e0)

`tests/python/test_pvlib_generator.py::test_pvlib_generator_fails_loudly_when_pvlib_call_fails` imports pvlib in a `sitecustomize` shim. pvlib is only in the `ochre` dependency group and the test has no `ochre` marker or `importorskip`, so a dev-only install fails it. Both tests pass here (4 passed with `test_ochre_units.py`).

### L-7 Nits

- `crates/hares-equipment/Cargo.toml` adds `serde_path_to_error = "0.1.20"` outside `[workspace.dependencies]` (ee7bee85).
- `heat_pump/heater.rs:516-519` `config.typed::<HeatPumpHeaterConfig>().ok()` swallows a typed-config error in the constructor; `init` fails on the same config, so the failure is not silent end to end. It is the pattern the entry set out to remove.
- `loop_allocator.rs` `allocate_loop_id`: a non-u16 `loop_id` already in the bag reads as absent and is overwritten with a fresh id; the counter uses `saturating_add`.
- `schedule_resolve.rs` `load_default_profiles`: a duplicate row for the same profile and element silently takes the last one.

## Not found

- Em or en dashes, ticket IDs, agent references: none in added lines or commit messages of any commit in `8082592e..1b708ccb`, including the fold commits, checked with the plan's `[writing]` pattern (one regex hit, `id="R1"` inside an XML test string in 18fbf72e, is a false positive).
- Hot-path allocation: none of the operator commits touches a step loop except `heater.rs` / `air_conditioner.rs` (a scalar `Option` and one local); no allocation added.
- Weakened or tautological tests: none found. `heating_only_heat_pump_contract.rs` uses `unwrap_or(0.0)` on telemetry but asserts a July cooling step occurred.
- BR-23 behaviour: the `gym_env.py` `output_verbosity is not None` to truthiness change is neutral (the Rust default for `None` is 0); the `generate_charging_curve_lut` restructure is equivalent.

## Compile walk, `cargo check --workspace --all-targets --all-features`

| Commit | Result |
|---|---|
| 787487df, 901354e0, 699cfad2, a7a9253d, ee7bee85, 249912f9 | ok |
| 13881b48 | FAIL `hares-python` lib test, `py_dwelling.rs:3707` (known) |
| e5b9ee42, b6a6ae68, 5929ceb6, c9361105, 54de33d2, 69de950a, 456df878, 17bd7291 | FAIL, same error (inherited from 13881b48) |
| 2f85338d | ok (fixes it) |
| 18fbf72e, 8c034e4f, 6c33e689, 1aa077ff | ok |
| 952e5486, 1b708ccb | FAIL `hares-io` test `schedule_integration`, `:189`, E0061 (H-2) |

Default-feature-only breaks were not walked (all-features only).

## Part A detail

### A1 Every hunk and test of `fix-or19` is on the head

- `edb83678` and `444f4c6e` (fold-or19c) have the same stable patch-id as `e5b9ee42` and `69de950a`. The other four differ because the fold rebased across about 35 commits of base change (ambient-location planning, event-load streams, thermal snapshot, atomic checkpoint load).
- Line-set comparison: every added line of `git diff 1c18543a 21c4fac0` (2851 distinct normalized lines) was searched in the whole 2f85338d tree; 42 are absent, all accounted for:
  - the EV driver cursor check, now `check_ev_driver_cursor` in `dwelling/checkpointing.rs:328-345` (same predicate, with the stream range from `EV_DRIVER_STREAM_COUNT` instead of `RNG_STREAM_EVENT_LOAD_BASE`, and now run inside `load_component_states` so it rolls back);
  - the last-step environment restore, now `last_step_environment` (`checkpointing.rs:146-157`, computed on a copy);
  - `CHECKPOINT_VERSION = 9`, superseded by 11;
  - import-list reflow, rewrapped doc and comment lines, test literals adapted to `Some(path)` schedules and `thermal.x` (the float test `checkpoint_file_round_trips_floats_exactly` still asserts bit equality), and the `tou()` helper now routed through `bms_priced_with`.
- All 161 function names added by `fix-or19` exist at the head (every test named in the review's table, plus `an_unwirable...` additions). The `ev_driver_v1_snapshot_blob_rejected_by_named_version_gate` rename is deliberate (456df878).
- Pins in the review table: `float_roundtrip` in `Cargo.toml:57` (workspace) and removed from `tools/frame_golden`; `current_day_ordinal0 = u32::MAX` at `bms.rs:74`; `check_declared_signals` at `bms.rs:171`, `ev_driver/mod.rs:526`, `dwelling/mod.rs:4613` and `:4915`; `actor_seed` uses `battery_capacity_kwh_rated` (`ev/mod.rs:1953`).
- Full nextest debug line on the head: 7342 tests run, 7342 passed (with `a416b901` applied in the scratch tree, because the head does not compile without it). The OR-19 tests pass in that run.

### A2 `CHECKPOINT_VERSION`

`checkpoint.rs:41` is 11; `PINNED_VERSION` is 11 (`:341`); the doc history lists v10 and v11 and the tripwire test passes. Version 9 was claimed by `fix-or19`; the fold's v10 note ("fold of the two v9-claiming branches") and v11 are consistent with the schema pin test.

### A3 Mutation record

`mutation-OR-19-roster-plan.md` names `plan_roster_caches` and `plan_loop_types(...)?`: present at `roster.rs:1019` and `:1041-1043`; the test and its `expect_err` at `roster_add_wiring_failure.rs:107` match the recorded panic location; `roster.rs` and the test are unchanged between 2f85338d and 1b708ccb. I did not re-run the mutation: the edit was refused by the permission classifier and I did not work around it. `fold-or19d.md` still says the mutation was "a prediction, not a run", which the later record supersedes; that line is stale.

### A4 Assembly time, before `13881b48` and after `2f85338d`

Release build, `Dwelling::from_config` (5 ResStock 2024.2 homes) and `from_toml_config_with_write_output` (10 BESTEST fixtures); each sample is the mean of 3 constructions after one warm-up; 10 alternating runs per side. Harness: a throwaway test file in each scratch tree (not committed). The machine was loaded by other jobs (load average 20 to 47 on 28 cores), so spreads are wide. Table is CPU time in ms, median [min-max].

| Case | Before 13881b48 | After 2f85338d | Change |
|---|---|---|---|
| resstock bldg0000002 | 47.6 [39.8-58.6] | 46.6 [39.0-77.2] | -2.1% |
| resstock bldg0000003 | 40.1 [36.3-61.8] | 45.5 [35.1-82.0] | +13.6% |
| resstock bldg0000004 | 54.3 [40.1-65.9] | 42.2 [37.9-73.5] | -22.3% |
| resstock bldg0000005 | 47.2 [39.5-69.1] | 41.7 [37.2-73.9] | -11.7% |
| resstock bldg0000006 | 48.5 [41.3-82.0] | 42.6 [36.8-73.2] | -12.3% |
| bestest 600 | 8.9 [8.1-16.1] | 8.2 [8.0-14.5] | -7.5% |
| bestest 600ff | 10.3 [8.1-14.7] | 9.2 [8.0-16.7] | -11.0% |
| bestest 610 | 81.1 [70.8-108.0] | 82.7 [69.2-105.2] | +2.0% |
| bestest 620 | 84.8 [63.2-98.3] | 74.5 [63.1-116.5] | -12.2% |
| bestest 640 | 12.4 [7.9-16.2] | 10.0 [8.1-17.5] | -19.1% |
| bestest 900 | 10.5 [8.4-18.9] | 9.7 [8.3-17.7] | -7.9% |
| bestest 900ff | 10.6 [8.7-16.0] | 9.5 [8.6-19.0] | -10.6% |
| bestest ce100 | 108.4 [98.5-134.5] | 104.2 [89.8-167.3] | -3.8% |
| bestest ce200 | 113.8 [87.5-143.8] | 104.6 [89.2-145.5] | -8.1% |
| bestest s54_heat_pump | 60.9 [47.3-89.2] | 61.9 [50.9-83.1] | +1.7% |

Wall time medians track the CPU figures (ResStock 44 to 60 ms before, 46 to 51 ms after). Every change is inside the run-to-run spread; the plan-then-install step shows no measurable assembly cost. The ±13% on bldg0000003 is within its own spread (35-82 ms). The result is not precise enough to bound a regression under about 15%; a quiet machine is needed for that.

## Verification run on the head

| Line | Result |
|---|---|
| `cargo nextest run --workspace --no-fail-fast` | does not compile at 1b708ccb (H-2); with `a416b901` applied in scratch: 7342 passed, 0 failed |
| `frame_golden compare-all` | FAIL, 7 of 7, digest only (H-1) |
| `frame_golden -F dst compare-all` | FAIL, 1 of 1, digest only (H-1) |
| `pytest test_ochre_units.py test_pvlib_generator.py` | 4 passed |
| pyright at `--pythonversion 3.12` | the two `TypeIs` imports (M-3) |

Not run: observe, dst, profiling and release nextest lines, clippy lines, the full pytest suite (no built extension at the head), and the extension-dependent Python tests of ee7bee85.

## Files

- `/home/rich/src/HARES/crates/hares-io/tests/schedule_integration.rs`
- `/home/rich/src/HARES/defaults/Default Schedule Parameters.csv`
- `/home/rich/src/HARES/tests/fixtures/golden/`
- `/home/rich/src/HARES/crates/hares-core/src/dwelling/conversions.rs`
- `/home/rich/src/HARES/crates/hares-core/src/dwelling/blueprint.rs`
- `/home/rich/src/HARES/pyproject.toml`
