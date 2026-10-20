# Fold integrity audit: feat/output-results @ 5d66dafc

Verdict: NOTHING-LOST

Method: for each pair, `git range-diff` per commit, plus `git diff <base> <source-tip>` vs `git diff <fold-base> <fold-tip>` with index and hunk-header lines stripped and the two patches diffed against each other (so every added/removed line of the source is compared, not just file lists). Test inventories (`#[test]`, `#[tokio::test]`, `def test_`) were taken at every source tip, at the bases and at the head with `git grep`, and compared with `comm`. All added tests were run in a scratch worktree at 5d66dafc (`cargo test --workspace --no-fail-fast -j 8`).

## 1. OR-18 (base 1c18543a)

| Source | Fold | Status |
|---|---|---|
| 9d599117 | f719895b | identical (range-diff `=`) |
| f1caa8da | b7ea5d0a | identical (range-diff `=`) |
| 7aaaf53e | b3807202 | identical (range-diff `=`) |

Combined diff 1c18543a..7aaaf53e vs aab05da0..b3807202: zero differing lines (fold parent aab05da0 is 8 commits above 1c18543a; none of them touch these hunks).

## 2. OD-29 (base 1c18543a)

| Source | Fold | Status |
|---|---|---|
| c0c00fea | 3005ee1c | conflict-resolved-equivalent: range-diff `!`, but all +/- lines identical; only context lines differ (dwelling/mod.rs zone-capacitance block was rewritten on the fold base by OR-18) |
| 4e5bc5fa | a4b29269 | conflict-resolved-equivalent plus one defect: a4b29269 carries a duplicate copy of the `if eq.uses_rng_stream()` registration block in `build_from_blueprint_inner` (17 extra added lines vs source) |
| (none) | 59465557 | changed-intentionally (extra fix-forward): removes exactly that duplicate block (14 lines). Its message states the conflict resolution left two identical blocks whose second insert collided. |

After 59465557, combined diff vs source differs only in 3 context lines (the `if let Some(zone_id)` lines vs the comment lines OR-18 introduced). Head has a single registration block (dwelling/mod.rs:2600-2610), identical to the source's block. No lost hunk.

Tests removed by source (5 `derive_rng_seed_*` / `*injected_seed*` / `same_name_same_seed*` in hares-equipment event_load.rs) are removed on head too, as in the source.

## 3. RT-9 (base 1c18543a)

| Source | Fold | Status |
|---|---|---|
| 3c734c44 | 2a39a8ee | changed-intentionally: CHECKPOINT_VERSION 9 -> 10 (the two branches both claimed v9; the source's doc comment itself says the fold must bump to a fresh version); doc text rewritten; thermal_solver test import / `crate::BoundaryDiagnosticInfo` hunks differ only because the fold base (OR-18/other commits) already carried the equivalent import state |
| d7fe5684 | 89eae0ba | identical (range-diff `=`) |
| d2621536 | d09a3b64 | changed-intentionally: doc comment for v10 (mentions both schema changes and the independent snapshot `schema_version`); everything else (ventilation recovery, fallback state, atomic restore, PINNED_SCHEMA_SHA256 shape test) identical |
| (none) | 39113e2d | changed-intentionally: `PINNED_VERSION` 9 -> 10 in the shape test, the one-line follow-through of the bump. Test module of checkpoint.rs at 5d66dafc vs d2621536 differs only on that line. |

No source hunk lost. The only +/- differences are the version bump, its documentation, and the context/import differences from the different base.

## 4. Tests

Added by the sources and present on head (72 distinct test names): OR-18 45, OD-29 12, RT-9 15. All 72 pass on head. The scratch worktree has no vendors/OCHRE checkout, so 4 of the 72 (equipment_zone_resolution::hvac_and_water_heaters_without_a_zone_fail_init, hpxml_parsing_tests::parse_hpwh_wall_heat_fraction_is_explicit_per_location, unlocated_water_heater_rejects_an_unknown_iecc_zone, unlocated_water_heater_takes_the_os_hpxml_default_location) first failed with fixture-not-found; re-run with vendors/OCHRE symlinked to the main tree's checkout: `hpxml_parsing_tests` 59/59, `equipment_zone_resolution` 3/3 pass. Full workspace run otherwise: all other test targets pass (about 7232 passed; the 10 environmental failures were all the missing-fixture cases above and pass on re-run). Python tests (tests/python/test_blueprint_*.py) were not run (needs the built extension): patch identity above covers them; this is a verification gap.

Source-tip tests absent from head, all explained:

| Test | Explanation |
|---|---|
| environment.rs `ambient_other_space_series_follows_the_scheduled_space_table`, dehumidifier `new_sets_zone_id_explicit_only_when_config_carries_zone_id`, hvac_tests `zone_id_0_equipment_produces_nonzero_thermal_output`, resolve_hvac `conditioned_zone_id_returns_none_when_no_conditioned_zone` | removed/replaced by OR-18 itself (present at RT-9 and OD-29 tips only because those branch from 1c18543a); OR-18 adds replacements (`ambient_air_follows_the_scheduled_space_table_for_occupied_placements`, `zone_id_0_equipment_fails_init_naming_the_unit`, `conditioned_zone_id_errors_when_no_conditioned_zone`) |
| 5 event_load.rs rng-seed tests | removed by OD-29 (3005ee1c/a4b29269), replaced by `init_without_a_stream_is_rejected`, `injected_stream_survives_init_and_is_used_by_equipment` |
| thermal_solver `consecutive_nonconvergence_count_incremented_on_failure` | renamed by RT-9 to `ideal_capacity_failure_count_incremented_on_failure` |
| thermostat `apply_thermal_setpoint_signal_autocorrects_with_zero_deadband` | already gone in the fold base: removed by fe804b59 (in aab05da0 ancestry, outside these folds) |
| crates/hares-core/tests/resstock_smoke.rs (6 tests) | file deleted by 6646c801 ("delete test files no target builds"); the live copy tests/resstock_smoke.rs still has them |

No test at a source tip is missing from head without a citation.

## 5. i08-fixes vs fix-or19

| i08-fixes | fix-or19 | Status |
|---|---|---|
| 9ef347e0 | 12f753be | identical (range-diff `=`) |
| ffa9051f | a3a21a92 | identical (range-diff `=`) |

Combined patch 7b7a6a7d..ffa9051f equals 1c18543a..a3a21a92 line for line. fix-or19 additionally carries 1bb045c6 and f77054df. Tests at the i08-fixes tip absent from f77054df, all explained:
- ports.rs `validate_fluid_type_consistency_*` (4) and roster_plan_schema_parity `equipment_added_before_first_step_gets_the_same_schema_metadata_as_assembly`: removed/rewritten by 1bb045c6 (single `plan_loop_types` implementation; schema-parity test rewritten to remove/re-add equipment).
- helpers.rs `zone_id_from_config_or_default_*` (2), dehumidifier `init_updates_zone_id_explicit_when_config_lacks_zone_id`: removed by 1c18543a (OR-18 zone cleanup, in fix-or19 base).
- radiant_gain_weights_zero_alloc `radiant_gain_weight_distribution_zero_allocations`: removed/changed by 44da330c (perf: no heap allocation stepping, in fix-or19 base).

Result: i08-fixes is fully superseded by fix-or19; nothing in i08-fixes is missing from it.

## Verdict: NOTHING-LOST

Cleanup: scratch worktree .workstrees/foldaudit and target-foldaudit removed; main tree untouched.
