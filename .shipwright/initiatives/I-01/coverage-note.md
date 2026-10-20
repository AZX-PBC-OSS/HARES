---
coverage_note_outcome: written
---

# I-01 coverage round-out

Tests were added; this note records the class measurement, what was added,
and the gaps that remain.

## What changed (per rca.md / reproduction.md)

No numeric defect: Rule R1 deliberately pins physics-equipment real-power
ZIP to `(0,0,1)`. The fix closes the observability gap with a
`real_power_zip_applies` flag on `ResolvedZip` (set at the shared
constructors `governing`/`reactive_only` in `crates/hares-types/src/zip.rs`)
exposed through `equipment_zip()`, plus a new `Dwelling::premise_zip()`
aggregate (`crates/hares-core/src/dwelling/premise_zip.rs`,
`crates/hares-python/src/py_dwelling.rs:1023`).

## Class measurement

The defect shape — a structurally pinned `(0,0,1)` indistinguishable from a
measured constant-power model — exists at every `resolved_zip()` call site:
~25 equipment modules under `crates/hares-equipment/src/` (HVAC ×7,
water heaters ×4, ventilation, scheduled/event loads, DER), all fed through
the two shared `ResolvedZip` constructors. The reproduction test already
iterates *every* equipment asserting flag presence; the additions below
cover the two integration-level invariants that remained untested rather
than repeating per-site assertions.

## Tests added

Both in `tests/python/test_equipment_zip_voltage_response.py`
(`TestPremiseVoltageResponse`), which went from 5 to 7 tests:

- `test_premise_roster_partitions_all_zip_equipment` — every ZIP-bearing
  equipment appears in exactly one `premise_zip()` roster, on the side its
  own `real_power_zip_applies` flag declares; no-ZIP equipment appears in
  neither. Exercises the rostering branch at
  `crates/hares-core/src/dwelling/premise_zip.rs:81-110` (including the
  `resolved_zip() is None → continue` skip) and the Python dict rendering
  at `crates/hares-python/src/py_dwelling.rs:344-375`.
- `test_premise_aggregate_is_weighted_mean_of_governing_coefficients` —
  the published aggregate coefficients equal the power-weighted mean
  recomputed from the per-equipment dicts and rostered weights, including
  the pf aggregation through the weighted mean of `tan(acos(pf))`.
  Exercises the weighted sums `premise_zip.rs:115-125`, the
  `w > 0.0 && w.is_finite()` skip branch, the pf aggregation
  `premise_zip.rs:130-136`, and the `real_power_zip_applies: True` key at
  `py_dwelling.rs:360`. Edge cases: `None`/zero-weight governing entries
  excluded from weighting; coefficient closure (`zp+ip+pp ≈ 1`).

## Verification

Latest full state (this checkout, all green): `cargo test -p
hares-equipment --lib` → 2182 passed, 0 failed (the ColumnRef finding made
green by the fix phase, plus this round's four additions); `cargo test -p
hares-core --lib` → 903 passed; `cargo test -p hares-io --lib` → 918
passed; `cargo test -p hares-types --lib` → 392 passed; `uv run pytest
tests/python/test_equipment_zip_voltage_response.py
tests/python/test_reactive_power.py tests/python/test_py_control.py -q` →
193 passed; `cargo fmt --check --all` clean; `cargo clippy -p
hares-equipment --tests` → 0 errors.

Earlier record: `uv run pytest tests/python/test_equipment_zip_voltage_response.py -q` →
`8 passed in 2.01s` (after `uv run maturin develop` rebuilt the extension
against the current working tree).
Neighboring suites also run: `uv run pytest tests/python/test_reactive_power.py -q`
→ 20 passed; `cargo test -p hares-core --lib` → 902 passed;
`cargo test -p hares-io --lib` → 916 passed; `cargo test -p hares-types
--lib` → 392 passed; 0 failed anywhere (current checkout, after the
boundary-hardening fix and this round's addition).

All tests pass; the earlier red finding test was made green by the fix
phase's `is_finite` guard (see the resolved-finding section).

## Resolved finding: NaN schedule data silently clamped to zero

**Status: fixed.** `schedule_column_mean_kw` now rejects non-finite column
data up front (`crates/hares-core/src/dwelling/premise_zip.rs:59`,
`!column.iter().all(|v| v.is_finite())` → `None`), and the finding test
`nan_schedule_data_must_not_degrade_to_a_silently_skewed_mean` passes.
This round added two more tests against the new guard:

- `infinite_schedule_data_is_also_not_a_computable_mean` — the ±inf arm of
  the `is_finite` guard (`premise_zip.rs:59`); `"inf"` parses through the
  CSV loader exactly like `"nan"`, so both infinities are pinned to `None`.
- `nan_column_rosters_equipment_without_weight_not_as_zero_draw` — the
  aggregate-level observable: an equipment with a NaN schedule column stays
  on the governing roster with `mean_power_kw: None` (degradation visible
  to the caller) and contributes no weight, rather than silently vanishing
  or being clamped to zero draw. Exercises the
  `ExpectedMeanPower::ScheduleColumn` → `None` path at
  `premise_zip.rs:104-108` through `aggregate_premise_zip`.

Original finding, for the record:

**Failing test that surfaced it (left red as evidence until fixed, per
constitution "a test you added that fails is a finding"):**
`crates/hares-core/src/dwelling/premise_zip.rs` test module,
`nan_schedule_data_must_not_degrade_to_a_silently_skewed_mean`.
`cargo test -p hares-core --lib premise_zip` → **1 failed, 7 passed**;
the failure prints `left: Some(2.0), right: None`.

This note previously misrecorded the mechanism as "NaN → `None` via the
`partial_cmp` guard". Coverage triage
(`coverage-note-quality.md`) corrected that: NaN schedule column values
never reach the guard. `schedule_column_mean_kw` (`premise_zip.rs:52-59`)
computes `column.iter().map(|v| v.max(0.0))`, and `f64::max` returns the
non-NaN operand, so NaN entries are **silently treated as zero draw**: a
partially-NaN column yields a silently undercounted mean (skewing the
published premise weight toward that equipment), and an all-NaN column
yields weight 0.0, silently dropping the equipment from the aggregate. The
NaN is concretely reachable: the CSV loader
(`crates/hares-io/src/schedule.rs:389`) uses `str::parse::<f64>()`, which
accepts `"nan"`, with no `is_nan`/`is_finite` check anywhere in that file.
Corrupted input therefore produces a plausible-but-wrong published premise
ZIP with no signal — the exact failure class this initiative exists to
eliminate. The test pins the honest degradation (NaN → `None`, i.e.
"expected draw not computable", rostered with `mean_power_kw: None` —
distinguishable from a real weight) and currently fails against the clamp.
The fix phase adopted exactly the semantic the test pinned (NaN → `None`,
i.e. "expected draw not computable", rostered with `mean_power_kw: None`).
The `partial_cmp` guard arm for `ExpectedMeanPower::Kw(NaN)` has no
producer in the codebase and needs no test.

## Boundary hardening round (non-finite schedule data)

The fix phase pushed the NaN defense to the earliest stages: rejection at
the CSV parse boundary (`crates/hares-io/src/schedule.rs:395-404`, naming
row and column) and at the shared consumption funnel
(`ScheduleSource::value_at`, `crates/hares-types/src/schedule.rs` — one
guard after `value_at_raw`, covering every source kind), each with unit
tests (`non_finite_schedule_values_are_rejected_at_the_parse_boundary`,
`non_finite_values_are_rejected_not_silently_absorbed`). The premise
aggregation guard remains as backstop.

This round added the remaining seam — end-to-end through the Python
bindings:

- `tests/python/test_equipment_zip_voltage_response.py::TestScheduleDataIntegrity::test_nan_schedule_value_fails_loudly_with_cause`
  — a schedule CSV with a `nan` field must make `Dwelling.from_hpxml`
  raise, with the message naming the defect (`non-finite`) and the
  offending column. Exercises `hares-io/src/schedule.rs:395-404` →
  `HaresError::Io` → `to_py_err` (`py_dwelling.rs:2533-2555`) end-to-end.

During development this test initially reported DID NOT RAISE — diagnosed
as a stale `python/ochre_next/_hares.*.so` (it predated the `hares-io`
guard; a nonsense `abc` value was correctly rejected, proving the build
was pre-guard), not a production defect. After `uv run maturin develop`,
the test passes against the guarded loader. The transient red usefully
proves the test distinguishes guarded from unguarded builds.

## Red-team follow-up round (expected-mean honesty, non-finite defaults)

The red team (`red-team-note.md`) pinned five findings; the fix phase made
all five green (verified on this checkout: `cargo test -p hares-equipment
--lib` → 2173 passed, then 2175 with this round's additions;
`cargo test -p hares-core --lib` → 903 passed; Python ZIP suites → 193
passed). This round rounded out the newly landed arms:

- `crates/hares-equipment/src/event_load.rs::tests::uniform_month_multipliers_scale_the_published_expectation_exactly`
  — the uniform-nonzero arm of the event-load fix
  (`event_load.rs:118-121`): uniform 0.5 multipliers halve the published
  expectation exactly.
- `crates/hares-equipment/src/event_load.rs::tests::partial_month_multipliers_are_honestly_not_computable`
  — the partial-multiplier arm (`event_load.rs:122-126`): the expectation
  is honestly `None`, not a guessed weight.
- `crates/hares-io/src/defaults.rs::tests::water_heating_csv_non_finite_values_are_skipped`
  and `...::vehicle_mapping_csv_non_finite_values_are_skipped` — the
  finiteness arms the fix added to the water-heating defaults CSV
  (`defaults.rs:1650-1658`) and EV vehicle-mapping CSV
  (`defaults.rs:1736-1800`) loaders: "nan"/"inf" parse as f64, so
  rejection must be explicit. Both pass.

**Status: fixed.** The fix phase made the `ColumnRef` arm honest
(`scheduled_load.rs:713-739`): all-zero multipliers → `Kw(0.0)`, identity
multipliers → deferral preserved, anything else → `None`. The finding test
passes, and this round pinned the fix's remaining arms:

- `crates/hares-equipment/src/scheduled_load.rs::tests::column_sourced_load_with_identity_month_multipliers_keeps_column_deferral`
  — the all-ones arm (`scheduled_load.rs:732-734`).
- `crates/hares-equipment/src/scheduled_load.rs::tests::column_sourced_load_with_partial_month_multipliers_is_not_computable`
  — the mixed-multiplier arm → `None` (`scheduled_load.rs:735-737`).

This round also covered the fix phase's hardening of
`parse_month_multipliers` (`crates/hares-equipment/src/schedule_helpers.rs:180-200`,
now `Result`-returning, rejecting non-finite multipliers that
`val.max(0.0)` would have silently dropped), which had no test at any
level (`schedule_helpers.rs` had no test module):

- `schedule_helpers::tests::non_finite_month_multipliers_are_rejected_naming_the_key`
  — NaN and ±inf rejected, error names the offending key; the shared
  mechanism used by both `scheduled_load` and `event_load`.
- `schedule_helpers::tests::absent_month_multipliers_are_none_and_finite_ones_parse`
  — the absent/parse boundary of the same helper.

Original finding record:

**Finding (was left red as evidence, rule 2a):**
`crates/hares-equipment/src/scheduled_load.rs::tests::column_sourced_load_with_zeroed_months_must_not_publish_unscaled_column_weight`
— `cargo test -p hares-equipment --lib` → **2175 passed, 1 failed**, the
failure printing `got Some(ScheduleColumn(1))`. Finding 2's fix scaled the
*constant-source* arm of `ScheduledLoad::expected_mean_power_kw`
(`scheduled_load.rs:717-735`) but not the `ColumnRef` arm
(`scheduled_load.rs:713-714`), which still defers to an unscaled
`ScheduleColumn` weight even though `step()` applies the month multipliers
to column draws identically (`scheduled_load.rs:575`). A column-sourced
load with all twelve months zeroed draws nothing (grounded in the test)
yet publishes a deferral the premise aggregate weights by the unscaled
column mean. Same reachability caveat as red-team Finding 2: no
HPXML/resolve producer emits `month_multiplier_N` today — supported config
API that step code honors. The fix phase owns the semantic (scale-aware
deferral vs explicit `None`).

## No-change round

No production file changed since the previous round (working-tree diff
unchanged at 46 paths). The only delta: the fix phase added an
equipment-init-level test for the month-multiplier guard
(`scheduled_load::tests::non_finite_month_multiplier_fails_loudly_at_init`),
complementing the helper-level tests from the previous round.

Correction to a previously flagged gap: the generator efficiency-curve
validation (`hares-io/src/defaults.rs:2283-2330`) is pre-existing code,
not part of this initiative's diff, and its core arms are already tested
(`malformed_generator_efficiency_curve_returns_none`); it is out of scope,
not a gap in this work.

Re-verified green on this checkout: `cargo test -p hares-equipment --lib`
→ 2182 passed, 0 failed; `cargo test -p hares-core --lib` → 904;
`cargo test -p hares-io --lib` → 918; `uv run pytest
tests/python/test_equipment_zip_voltage_response.py -q` → 10 passed;
`cargo fmt --check --all` clean (the fix phase also added its own
regression tests alongside the fixes, e.g.
`scheduled_load::tests::non_finite_month_multiplier_fails_loudly_at_init`).
No new tests were warranted from coverage this round.

## v0-validation round

The fix phase extended `validate_zip_sums`
(`crates/hares-equipment/src/config.rs:494-560`) with a non-positive-`v0`
rejection (`config.rs:524-529` — the red team's noticed-but-unpinned
division hazard) and a magnitude-bounds class guard
(`hares-types/src/zip.rs:140`, `validate_plausible_magnitudes`, with its
own tests: `magnitude_bounds_reject_overflow_class_rows`,
`magnitude_bounds_accept_every_literature_row`). The overflow arms were
covered by the fix phase; the zero/negative-`v0` arm was not. Added:

- `crates/hares-equipment/src/config.rs::tests::validate_zip_sums_rejects_non_positive_v0`
  — `v0 = 0.0` and `v0 = -1.0` (both finite, so only the positivity check
  guards them) rejected with the error naming `v0`; a sane `v0 = 0.98`
  stays accepted.

Verified green on this checkout: `cargo test -p hares-equipment --lib` →
2184 passed, 0 failed; `hares-core` → 904; `hares-types` → 394;
`hares-io` → 918; `uv run pytest
tests/python/test_equipment_zip_voltage_response.py -q` → 10 passed;
`cargo fmt --check --all` clean.

## Gaps not covered (recorded, not silently left)
- DER `real_power_zip_applies=False` at Python level: the `base.xml`
  fixture has no battery/EV/PV. Covered at Rust level
  (`crates/hares-equipment/src/battery/mod.rs:6313,6332`) and pf behavior
  in `tests/python/test_reactive_power.py`.
- `equipment_zip()` returning `None` at Python level: the fixture has no
  generator/bridge/indirect tank. Rust-side equivalent covered by
  `premise_zip.rs` test `equipment_without_zip_concept_is_silent`.
