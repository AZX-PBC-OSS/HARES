# Handoff: fix-od30 review fixes (review-OD-30-7b86f9d4.md)

- **Handoff branch `handoff/fix-od30`**, worktree `/home/rich/src/HARES/.workstrees/fix-od30` (checked out on the handoff branch, clean). Its commit is `a246f071`.
- It sits on `fix-od30` at `b50d66fc`, which sits on the reviewed tip `7b86f9d4`, whose base is `1b708ccb`. The branch `fix-od30` itself still points at `b50d66fc`.
- `a246f071` (L2, partial) **does not build**: see L2. Finish it, then squash or reword it before any fold.
- Engine head `feat/output-results` = `db5efe80` (not yet rebased onto it; the orchestrator says `fix-testhyg` and `fix-headbreaks` fold first).

New commits:

| SHA | State |
|---|---|
| `4a67ba54` test: raw read off its list panics; every listed parameter has a reader | M1, builds, tested |
| `43895391` fix: negative month multiplier is a parameter error; D-008 divergence | L1 + L6, builds, tested |
| `b50d66fc` fix: schedule column that creates no equipment is a dwelling construction warning | M2, builds, tested |
| `a246f071` refactor: resolve_equipment takes no overrides (on `handoff/fix-od30`) | L2, partial, **does not build** |

## Findings

**M1: done and tested** (`4a67ba54`).
- `crates/hares-equipment/src/config.rs`: a `#[cfg(test)]` thread-local `RAW_READS` (after `KEY_EQUIPMENT_ID`). `raw_value` records every key it is asked for in test builds.
- `crates/hares-equipment/src/raw_params.rs` tests:
  - `a_read_off_its_list_panics_in_a_debug_build` and `a_listed_key_read_as_another_kind_panics_in_a_debug_build` (both `cfg(debug_assertions)`, `catch_unwind`, one class per kind).
  - `a_listed_key_read_as_its_kind_is_answered`.
  - `every_listed_parameter_has_a_reader`: inits each kind across all read-path variants and asserts that every listed param is read.
- I proved by mutation that a dead list entry fails `every_listed_parameter_has_a_reader`. I could not prove the panic tests fail when the assertion is disabled: the permission classifier denied that temporary edit. The reviewer should run that mutation.

**M2: done and tested** (`b50d66fc`).
- `crates/hares-io/src/schedule_resolve.rs` `ensure_specs_for_csv_columns` now takes `warnings: &mut Vec<Warning>`. It pushes `Warning::new("schedule", "schedule CSV column '{col}' is not read: the building declares no {eq} and there is no default annual energy to create one with, so no {eq} is created")` and also logs `warn!`.
- The message prefix matches `fix-headbreaks` `c71a360b` ("schedule CSV column '..' is not read: ..").
- Tests:
  - `columns_for_equipment_without_energy_create_no_equipment` now asserts the warning.
  - `unknown_schedule_column_warns_naming_it` now filters on "no entry in COLUMN_MAPPINGS" (its spec-less input now also warns per equipment column).
  - New dwelling-level test `crates/hares-core/tests/schedule_columns.rs::a_column_for_equipment_the_home_lacks_is_a_dwelling_warning` (bldg0176775 plus an added `lighting_basement` column). The existing test was refactored through the helper `bldg0176775_schedule_warnings`.
- hares-io and hares-core nextest: 2826/2826 passed at this commit. Pytest has not been run; check Python tests that count warnings.

**L1: done and tested** (`43895391`).
- `schedule_helpers.rs` `parse_month_multipliers`: a negative or non-finite value now returns `HaresError::InvalidEquipmentParameter { equipment, key, .. }`. The clamp is gone.
- Test: `negative_and_non_finite_month_multipliers_are_parameter_errors`. The `scheduled_load` test `non_finite_month_multiplier_fails_loudly_at_init` was updated to match the typed error.

**L6: done** (`43895391`).
- Comments in `scheduled_load.rs` (the field doc near line 145, `init_from_config` near 246, step near 422) no longer cite OCHRE for the scaling; they cite D-008.
- Added `## D-008` to `docs/alignment/DIVERGENCES.md`; it cites OCHRE `ScheduledLoad.py:36-40`.
- Added a note to `docs/equipment/loads.md`.
- No other branch has a D-008 (checked at head, fix-headbreaks and fix-testhyg).

**L2: partial, committed as `a246f071`; the workspace does not build.**
- Decision: remove the parameter (every caller passed `{}`).
- What `a246f071` changes:
  - `crates/hares-io/src/hpxml/equipment.rs`:
    - `resolve_equipment` loses its `overrides: &Value` parameter and gains a doc comment saying the dwelling applies the checked overrides.
    - The call to `apply_overrides` and the function itself are deleted.
    - Every `&json!({}),` argument in the test module is removed. One call site is left with 4 extra spaces of indentation; `cargo fmt` fixes it.
  - `crates/hares-io/tests/hpxml_parsing_tests.rs`: the `&json!({})` / `&serde_json::json!({})` argument is removed from all 25 calls.
  - `crates/hares-io/tests/hpxml_parity.rs`: the argument is removed from all 16 calls, one-line and multi-line.
  - `crates/hares-io/tests/hpxml_defaults_regressions.rs`: the argument is removed from all 5 calls.
- Remaining: drop the `&json!({})` / `&overrides` argument at:
  - `tests/parity/mod.rs:326`
  - `crates/hares-io/tests/silent_default_regressions.rs:48`
  - `crates/hares-io/tests/hvac_wiring_regressions.rs:53`
  - `crates/hares-io/tests/schedule_integration.rs:476`
  - `crates/hares-core/src/dwelling/autosize.rs:2197`
  - `crates/hares-core/tests/equipment_zone_resolution.rs:217`
  - `crates/hares-io/src/schedule_resolve.rs:4336` (also delete `let overrides` at 4312)
  - `crates/hares-core/src/dwelling/blueprint.rs:145` (delete `empty_overrides`)
  - `docs/superpowers/plans/2026-06-08-hpxml-equipment-swapping.md`, if it shows the signature.
- Then:
  - Remove imports of `json`, `Value` and `Map` that are now unused (clippy will name them).
  - Check whether `override_layers` and `nested_update` still have users.
  - Run `cargo fmt`: one replace_all left 4 extra spaces of indentation in `equipment.rs`.
  - Commit with a proper message, plus a test if one is wanted (for example, the public signature has no override channel left; the compile itself pins this).

**L3: not started.** The appliance zone id takes the first zone of a type (`crates/hares-io/src/hpxml/resolve_loads.rs:~1166`, `.position`), while the zone map keeps the last (`crates/hares-core/src/dwelling/mod.rs:~2477-2505`, `map.insert`). Make them one rule, or make a duplicate zone type an error, with a test.

**L4: not started.** Add a hares-core dwelling test, for example in `crates/hares-core/tests/appliance_zone_gains.rs`, with a garage refrigerator whose heat lands in the garage zone. It must fail when `appliance_site` uses `idx` instead of `idx + 1` (`resolve_loads.rs:~1168`).

**L8: not started.** The goldens moved by event replay following the schedule date (`a71f0af8`):
- `fleet_mixed_offset` dwelling 0: `cooking` 7.93 to 15.90 kWh, `other` 52.52 to 29.14 kWh.
- `leap_year_february_900s`: water heating 4.23 to 5.89 kWh.
- `weather_wrap_900s`: water heating 3.64 to 4.76 kWh.

Investigate each: confirm the replayed day's cycles explain the delta, or fix a defect. Then add a cause sentence per golden to the OD-30 section of `ledger/02-measurement.md` (around line 1542).

**Wildcard-over-corpus test: not started.** Apply a wildcard override (for example `{"all": {"sensible_gain_fraction": ...}}`) to every ochre sample and ResStock fixture build, and assert it builds or fails only with a typed parameter error.

**Leave alone:** the `resolve_hvac.rs` `.ok()?` builders (OD-48), and L7.

## Unverified

- The M1 panic tests have not been shown to fail with the `raw_value` assertion disabled; the permission classifier refused that mutation.
- The L1 and M2 tests failing before their fixes is established by reading, not by running the old code. The old code returned `Ok` for a negative multiplier and pushed no warning.
- Pytest, clippy and the full §3 block have not been run on any of the new commits. Only `cargo fmt` and targeted nextest were run: hares-equipment `raw_params`/`month` filters, and hares-io plus hares-core in full (2826 passed) at `b50d66fc`.
- `a246f071` has not been compiled; it is known not to build.

## Remaining steps

1. Finish L2, then do L3, L4, L8 and the corpus test. Each needs a test that fails before its fix. Run targeted nextest (hares-equipment, hares-io, hares-core), pytest on the touched areas, clippy (`-D warnings`, all features) and fmt.
2. Wait for the orchestrator's message, then rebase onto the new head. Known conflicts (review section 9):
   - `schedule_resolve.rs`: 3 hunks. Keep the branch's schedule-column rule, which has no garage or foundation gate.
   - From head `7ac4d4d9`: keep `resolve_max_kw`.
   - From `fix-headbreaks` `c71a360b`: its gate-based warnings collapse into M2's warning, so drop its `foundation_name`/`garage_modeled` gate and keep one warning per unused column. Its tests `basement_lighting_column_without_finished_basement_is_a_warning` and `garage_lighting_column_without_a_garage_is_a_warning` must be re-expressed without the removed arguments; their messages assert "Finished Basement" and "no garage", which M2's message does not contain.
   - `schedule_integration.rs`.
   - The 7 goldens.
   - The head's 5 tests in `schedule_resolve.rs` that pass the removed `foundation_name` and `garage_modeled` arguments (E0061): delete those two arguments.
   - From `fix-headbreaks`: `conversions.rs`, `equipment_id_regressions.rs`, `equipment.rs` (it will also meet L2's signature change) and `215398f3`.
   - New `resolve_equipment` call sites on the head will need the overrides argument dropped.
3. Make every commit build on its own, or squash (fold-process lesson, `02-measurement.md:1512`).
4. Re-capture the goldens once, from a clean committed tree at the folded tip.
5. Run the full `plan.md` §3 block once on the final tip.

## Traps

- Edit only with Edit/Write. `replace_all` matches substrings: an indented pattern also matches lines with deeper indentation.
- The permission classifier blocks temporary mutations that weaken a check, such as disabling the debug assertion. Ask the reviewer to run such mutations.
- The M2 warning fires for every schedule equipment column when there are no specs. Tests that count schedule warnings must filter by message form.
- `crates/hares-core/tests/schedule_columns.rs` asserts that the golden home `consumer_shape_900s` and bldg0176775 carry no schedule warning. They pass now (both have the equipment), but recheck after the rebase.
- `.workstrees/target-fix-od30` (64 GB) is not mine. It looks like a previous round's target dir; the orchestrator should decide whether to delete it.
- Out of scope but silent: `crates/hares-core/src/dwelling/blueprint.rs:~131` substitutes an empty `DefaultsStore` with a warn when the defaults fail to load.
- My target dir and scratch directory are deleted.
