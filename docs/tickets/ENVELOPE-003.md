---
id: ENVELOPE-003
title: Fallback occupancy internal gains
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-core/src/dwelling/mod.rs
  - crates/hares-core/src/dwelling/conversions.rs
references:
  - vendors/OCHRE/ochre/Models/Dwelling.py (occupancy schedule handling)
  - crates/hares-physics/src/constants.rs (OCCUPANT_SENSIBLE_GAIN_W, OCCUPANT_LATENT_GAIN_W)
verification:
  - cargo test -p hares-core
  - cargo test --test conditioned_oracle --features observe -- conditioned_ideal_winter --nocapture
  - cargo clippy --all-targets -- -D warnings
---

## Background/Context

The BEopt_example schedule CSV has 7 columns (cooking, dishwasher, laundry, etc.)
but NO occupancy column. `Dwelling::apply_occupancy_gains()` returns early when
`occupancy_column_idx` is `None`, so HARES injects zero occupancy internal gains.

OCHRE injects ~66W sensible + ~51W latent during occupied hours regardless of whether
the schedule CSV has an occupancy column — it derives occupancy from the HPXML building
description (number of bedrooms => default occupant count). The conditioned oracle
OCHRE fixtures were generated with this default occupancy active.

Impact: ~8.3W mean internal gain in the OCHRE reference vs 0W in HARES. This is small
(~3% of heating load) but it's a systematic bias that inflates the measured HVAC
energy over-prediction and invalidates the comparison methodology.

## Options

1. **Verify OCHRE fixtures were generated with occupancy stripped** — check if
   `generate_conditioned_oracle.py` set `dwelling.equipment['Occupancy'].schedule = 0`.
   If yes, the comparison is valid and no HARES change needed. **Check this first.**

2. **Add a fallback occupancy schedule** — when no occupancy column is in the schedule
   CSV, derive a default from HPXML bedroom count:
   - Number of occupants = 0.5 * n_bedrooms + 1 (OCHRE default)
   - Always-occupied (constant occupancy fraction = 1.0)
   - Or use OCHRE's default schedule profile if available

3. **Strip occupancy from the OCHRE fixture** — regenerate with occupancy disabled.
   This is the cleanest short-term fix for parity testing, but it leaves HARES
   without occupancy for real simulations.

## Work to Do

- [ ] First: check `generate_conditioned_oracle.py` to determine if occupancy was
  stripped in the OCHRE fixtures. Grep for `Occupancy` in the equipment removal logic.
- [ ] If occupancy IS in the OCHRE fixtures (likely, since `Internal Heat Gain - Indoor (W)`
  shows 8.3W): implement option 2 or 3.
- [ ] If implementing fallback occupancy (option 2):
  - Parse bedroom count from HPXML `Building` struct
  - Compute default occupant count: `0.5 * n_bedrooms + 1`
  - When `occupancy_column_idx` is `None` in `Dwelling`, use the default count
    as a constant occupancy value
  - Apply via existing `apply_occupancy_gains()` path
- [ ] Add a test verifying that internal gains are non-zero when occupancy fallback is active

## Files to Touch

- `crates/hares-core/src/dwelling/mod.rs`: Add fallback occupancy logic to `apply_occupancy_gains()`
- `crates/hares-core/src/dwelling/conversions.rs`: Parse bedroom count for default occupant derivation
- Potentially `tests/python/generate_conditioned_oracle.py`: If option 3, strip occupancy

## Measures of Success

- [ ] HARES internal gains match OCHRE within 20% when occupancy fallback is active
- [ ] Conditioned oracle `Internal/occupancy gain (W)` is non-zero
- [ ] Fallback does not affect dwellings that have explicit occupancy schedules
- [ ] No regression in freefloat tests (which strip all equipment including occupancy)

## Verification

- [ ] `cargo test -p hares-core` passes
- [ ] `cargo test --test conditioned_oracle --features observe -- conditioned_ideal_winter --nocapture` shows non-zero internal gains
- [ ] `cargo test --test freefloat_oracle --features observe` passes (no regression)
- [ ] `cargo clippy --all-targets -- -D warnings` passes
