# PSM3 and TMY3 Sky Temperature Bypasses Compute Path, Ignores Potential IR Column

**Severity**: Medium
**Impact on annual kWh**: Medium
**Status**: Open
**Areas**: hares-io/psm3, hares-io/tmy3

## Problem

Both parsers call `clark_allen_sky_temp_c` directly rather than routing through the shared `compute_sky_temp_c` function that selects between Stefan-Boltzmann inversion (when measured IR is available) and Clark-Allen fallback. This means:

1. If a future PSM3 file includes a longwave IR column (`Dhi_Modeled`, `Lwdown`, or `Radiation Modeled`), the data will be silently ignored and Clark-Allen will still be used — producing ±5–10 °C sky temperature error relative to the Stefan-Boltzmann inversion.
2. `horizontal_infrared_w_m2` is hardcoded to all-zeros for both formats, making the correct IR-present path permanently unreachable for these parsers.

Clark-Allen (1978) is a last-resort empirical correlation with no IR quality gating. EnergyPlus `CalcSkyTemp` (WeatherManager.cc) uses Stefan-Boltzmann inversion when measured horizontal IR ≥ 50 W/m², and falls back to Clark-Allen only when IR is absent. `compute_sky_temp_c` in `hares-io/src/epw.rs` implements this same branching. The parsers must route through it.

NSRDB PSM3 does not include `Opaque Sky Cover`; `opaque_sky_cover = 0.0` is therefore correct for the Walton cloud-cover correction path. The `Cloud Type` column present in some PSM3 product variants is currently unparsed — that is a separate issue.

## Current Behavior

`crates/hares-io/src/psm3.rs:245`: `clark_allen_sky_temp_c(db, dp)` called directly for every timestep.

`crates/hares-io/src/psm3.rs:270`: `horizontal_infrared_w_m2 = vec![0.0; n]` hardcoded.

`crates/hares-io/src/tmy3.rs:173`: same direct `clark_allen_sky_temp_c` call.

`crates/hares-io/src/tmy3.rs:190`: `horizontal_infrared_w_m2 = vec![0.0_f64; n]` hardcoded.

## Required Behavior

1. PSM3 parser must detect an optional longwave IR column (`Dhi_Modeled`, `Lwdown`, or `Radiation Modeled`, case-insensitive) in the column header map. When present, populate `horizontal_infrared_w_m2` from it. When absent, leave as 0.0.
2. Both parsers must call `compute_sky_temp_c(ir, db, dp, 0.0)` instead of `clark_allen_sky_temp_c(db, dp)`. When IR = 0.0, `compute_sky_temp_c` falls through to Clark-Allen — numerically identical to current behavior, but the routing is correct and future IR-present files activate the right physics path automatically.
3. No silent substitution: if a detected IR column contains non-finite values, return a parse error identifying the row and column.

Primary citation: EnergyPlus Engineering Reference §2.7.1 "Sky Radiation Modeling" — Stefan-Boltzmann inversion when IR ≥ 50 W/m², Clark-Allen (1978) as fallback.

Secondary citations:
- Clark, G. and Allen, C. (1978), "The Estimation of Atmospheric Radiation for Clear and Cloudy Skies", Proc. 2nd National Passive Solar Conference
- Berdahl, P. and Martin, M. (1984), "Emissivity of clear skies", Solar Energy, 32(5), 663–664
- ASHRAE Handbook of Fundamentals 2021 Ch. 14 §14.3 "Longwave Radiation"

## Approach

In `psm3.rs`: add `lwdown: Option<usize>` to `Psm3ColumnMap`. In the header-parsing loop, match column names case-insensitively against `dhi_modeled`, `lwdown`, `radiation modeled`. In the data loop, read the value when the index is present, fall back to 0.0 otherwise. Replace `clark_allen_sky_temp_c` call with `compute_sky_temp_c(ir, db, dp, 0.0)`.

In `tmy3.rs`: replace the direct `clark_allen_sky_temp_c` call with `compute_sky_temp_c(0.0, db, dp, 0.0)`.

## Definition of Done

- [ ] PSM3 parser calls `compute_sky_temp_c` for all timesteps
- [ ] TMY3 parser calls `compute_sky_temp_c` for all timesteps
- [ ] PSM3 parser optionally reads and populates `horizontal_infrared_w_m2` when an IR column is detected
- [ ] When IR = 0.0 (no column present), sky temperature output is numerically identical to current behavior
- [ ] Non-finite IR values produce a parse error with row and column identification

## Verification

```bash
cargo test -p hares-io psm3
cargo test -p hares-io tmy3
```

Tests must verify: (a) sky temp for a known db/dp pair matches `compute_sky_temp_c(0.0, db, dp, 0.0)` exactly, and (b) a synthetic PSM3 input with an `Lwdown` column produces sky temps matching `compute_sky_temp_c(lwdown_value, db, dp, 0.0)`.
