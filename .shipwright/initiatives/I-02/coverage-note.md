---
coverage_note_outcome: written
---

# Coverage rounding — I-02 (envelope_loads_kwh components don't balance)

## What changed (restated)

`opaque_solar_lwr_kwh` reported the gross radiant flux absorbed at the
exterior skin of the opaque envelope under a struct contract claiming every
field was a net zone load. The fix rewired the reported value to the net
interior-face conduction (renamed `opaque_conduction_kwh`), completed the
budget with `window_conduction_kwh` / `internal_mass_kwh`, removed the gross
term from "Net Sensible Heat Gain", un-gated the release-build accumulators,
fixed the engine's false zero-step flag, and corrected
`gross_consumption_kwh`. The fix phase pinned each instance; this round
covered what remained.

## Class measurement

Defect shape: "a reported value does not mean what its name/docs claim" in
the metrics/reporting layer. Search: `grep -rn "opaque_conduction\|
window_conduction\|internal_mass\|envelope_loads_kwh"` across `crates/`,
`tests/`, `python/`. Six instances of the shape were found and fixed in the
fix phase, each with a pinning test (`tests/envelope_opaque_loads.rs` gate +
net-sensible + nondegenerate-columns tests, `crates/hares-core/tests/
engine.rs` streaming/zero-step tests, `tests/exterior_skin_balance.rs`,
python PV gross-consumption tests, resstock smoke identity). Two gaps
remained; both are now covered.

## Tests added this round

1. `tests/envelope_opaque_loads.rs` →
   `tests::envelope_load_budget_is_signed_summable_and_closes`. The ticket's
   literal symptom — the components cannot be read as a budget — had no
   metrics-level assertion. The shared 24 h summer run's `OnceLock` now also
   retains the streaming-path `EnvelopeComponentLoadsKwh`, and the test
   asserts the invariant on the shared mechanism (the zone air heat
   balance), not per field: the signed sum of all net components (excluding
   the documented gross `interior_lwr_kwh`, cooling negated per its
   positive-magnitude storage convention) must nearly cancel —
   |sum| ≤ 0.5·Σ|component|. Observed on the fixture: +4.433 kWh vs
   32.715 kWh (ratio 0.135). Under the pre-fix gross value (13.58 kW mean =
   +326 kWh/24 h on this exact fixture, per the fix phase's measurement) the
   ratio would be ≈0.92 — the test goes red under the defect class in any of
   the ten fields. Includes a degenerate-accumulator guard (Σ|c| > 1.0 kWh,
   the release silent-zero class) and per-field finiteness.
   Exercises `crates/hares-io/src/output/metrics.rs:802-815` (accumulation)
   and `:1149-1165` (struct construction) end-to-end, previously covered
   only by synthetic-column unit tests.

2. `tests/python/test_py_metrics.py` → assertion inside
   `TestMetrics::test_envelope_loads_present_at_high_verbosity`:
   `not hasattr(env, "opaque_solar_lwr_kwh")`. The API rename was only
   hand-verified in the fix phase; scripts using the old name must fail
   loudly. Mirrors the existing `not hasattr(annual, "total")` pattern.
   Exercises the getter surface of `crates/hares-python/src/py_metrics.rs`.

## Paths deliberately not covered

- `window_conduction_kwh` / `internal_mass_kwh` individually vs. their CSV
  column integrals end-to-end: covered jointly by the budget sum and
  individually by synthetic unit tests (metrics.rs `envelope_loads_include_
  all_components`) and the streaming==batch equality test. A mis-wiring
  large enough to matter breaks the 0.5 ratio bound.
- Python-level budget magnitude: the Python getters are thin pass-throughs
  over the same struct asserted by test 1.
- `interior_lwr_kwh` non-summability: a documentation contract, nothing
  behavioral to assert.

## Verification (run, not recalled)

```
$ cargo test -p hares-core --test envelope_opaque_loads
test result: ok. 5 passed; 0 failed — including
  tests::envelope_load_budget_is_signed_summable_and_closes ... ok
  (envelope budget: signed sum +4.433 kWh, sum|component| 32.715 kWh)

$ uv run pytest tests/python/test_py_metrics.py -x -q
9 passed in 2.11s

$ cargo test -p hares-io / cargo test -p hares-core --test engine
all suites ok; cargo fmt --check clean; clippy clean.
```

Defect-falsification note: the new budget test was not executed against
pre-fix production code (reverting the fix is outside this role); the
discrimination argument is arithmetic on the fix phase's own measured
magnitudes (+326 kWh/24 h gross → ratio ≈0.92 ≫ 0.5 bound).

## Round 2 (after red-team pass and in-flight fixes)

The tree moved: the red-team pass (`red-team-note.md`) added
`envelope_load_fields_equal_their_csv_column_integrals`
(tests/envelope_opaque_loads.rs) closing the per-field gap this note
flagged, and its two deliberate reds (out-of-range `zone_output_indices`,
`outdoor_temp_input_indices`) are now green — the implementer landed the
shared construction-time range pass over the whole `StateSpaceWiring`
struct (`crates/hares-envelope/src/thermal_solver/mod.rs`, "Wiring range
validation" block).

That new production code had four unpinned branches. Added:

3. `crates/hares-envelope/src/thermal_solver/longwave.rs` →
   `construction_rejects_out_of_range_value_in_every_remaining_wiring_map`.
   One test over the shared validation pass, one case per remaining branch:
   `zone_state_indices` (deferred-panic mode), `zone_sensible_input_indices`
   and `indoor_temp_input_indices` (silent guard-and-skip modes: all
   zone-air injections discarded / residual corrupted), and
   `solar_input_indices` (silent). Each case asserts construction fails and
   the error names the wiring and says "out of range". Without any one
   branch the corresponding case returns `Ok` and the test goes red.

   The duplicate-`surface_id` case in `env.weather.solar_irradiance`
   (red-team note, Sweep 2) has since been given a defined contract and a
   pin by the implementer:
   `solar_slot_map_resolves_duplicate_surface_ids_first_wins`
   (`crates/hares-envelope/src/thermal_solver/mod.rs:7044`). This note's
   earlier "no defined contract" statement is stale; corrected here.

4. `crates/hares-envelope/src/thermal_solver/longwave.rs` →
   `construction_rejects_duplicate_dedicated_exterior_input_column`. The
   double-registration check immediately following the range pass
   (`crates/hares-envelope/src/thermal_solver/mod.rs:781-799` — two
   exterior surfaces sharing a non-zone injection column means one
   surface's flux lands in the other's column) had no pin;
   `config.rs:1118` documents that `validate()` deliberately cannot see it
   (wiring-aware), so the only guard is the constructor. Test builds two
   surfaces on dedicated column 1 with the zone sensible column elsewhere
   and asserts construction fails naming the duplicated column.

Round-2 verification (run, not recalled):

```
$ cargo test -p hares-envelope --lib thermal_solver::longwave::tests::construction
test result: ok. 7 passed; 0 failed   (incl. the new test)
$ cargo test -p hares-envelope --lib
test result: ok. 372 passed; 0 failed   (concurrent solar refactor converged;
                                         the two transient reds are gone)
$ cargo test -p hares-core --test envelope_opaque_loads
test result: ok. 6 passed; 0 failed
$ cargo fmt --check → clean
```
