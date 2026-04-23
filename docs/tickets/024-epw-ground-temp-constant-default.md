# EPW Ground Temperature: Shallowest-Depth Selection and Sentinel Default

**Severity**: High
**Status**: Open
**Areas**: hares-io/epw

## Problem

`parse_ground_temperatures` in `crates/hares-io/src/epw.rs:317` selects the EPW GROUND TEMPERATURES
entry with the smallest depth (`depth_m < best_depth` at line 354) instead of the entry closest to
0.5 m. EnergyPlus Engineering Reference §3.1 and EPW Data Dictionary v9.6 §3 (GROUND TEMPERATURES
field) specify 0.5 m as the reference depth for surface-temperature reconciliation. For EPW files
with entries at [0.1 m, 0.5 m, 2.0 m], the current code selects 0.1 m — the diurnal-response
layer — rather than the 0.5 m value appropriate for envelope boundary conditions.

A second defect compounds this: when a monthly value fails to parse, `EpwRecord` retains
`DEFAULT_GROUND_TEMP_C = 10.0` (line 342), which is also the DOE-2 fallback initialisation value
(lines 23, 224, 424). It is therefore impossible to distinguish "EPW header had no usable data" from
"DOE-2 model computed 10 °C", and any partial parse failure passes silently.

A third defect: the DOE-2 fallback uses `DOE2_GROUND_DEPTH_FACTOR = 10.0` m (line 371). At 10 m
the Kusuda-Achenbach attenuation factor is `exp(-10 × sqrt(π / (α × τ)))` ≈ 0.018, effectively
eliminating all seasonal variation. The DOE-2 GTEMP formula is derived for a representative soil
column and the depth factor should represent a typical shallow foundation depth (0.5 m), not 10 m.
At 0.5 m the attenuation is `exp(-0.5 × 0.4)` ≈ 0.82, which preserves the seasonal signal.

## Current Behavior

`crates/hares-io/src/epw.rs:354`:
```
if valid && depth_m < best_depth {
```
selects shallowest available depth, not the depth closest to 0.5 m.

`crates/hares-io/src/epw.rs:342`:
```
let mut monthly = [DEFAULT_GROUND_TEMP_C; 12];
```
fills unparseable entries with the sentinel 10 °C, indistinguishable from a valid DOE-2 result.

`crates/hares-io/src/epw.rs:371`:
```
const DOE2_GROUND_DEPTH_FACTOR: f64 = 10.0;
```
applies the DOE-2 damping at 10 m depth, suppressing all seasonal variation.

## Required Behavior

1. Select the EPW GROUND TEMPERATURES entry with depth closest to 0.5 m, per EPW Data Dictionary
   v9.6 §3 field description and EnergyPlus Engineering Reference §3.1 "Ground Heat Transfer
   Calculations Using a Simplified Approach."
2. Any monthly value that fails to parse must propagate as an error, not silently substitute 10 °C.
   The `DEFAULT_GROUND_TEMP_C` sentinel must be removed from parse paths; use `f64::NAN` as the
   initial fill so any missed assignment produces a detectable failure.
3. `DOE2_GROUND_DEPTH_FACTOR` must be renamed to reflect its physical meaning and set to 0.5 m
   (representative slab/crawlspace depth), or made a parameter. The DOE-2 GTEMP formula computes
   a ground surface temperature — its depth factor is the assumed burial depth of the
   representative soil boundary, not a deep-soil depth.

## Approach

1. In `parse_ground_temperatures`, replace the `depth_m < best_depth` comparison with
   `(depth_m - TARGET_DEPTH_M).abs() < (best_depth - TARGET_DEPTH_M).abs()`, initialising
   `best_depth = f64::INFINITY` and `TARGET_DEPTH_M = 0.5_f64`.
2. Change the monthly initialisation array from `[DEFAULT_GROUND_TEMP_C; 12]` to `[f64::NAN; 12]`
   and propagate `None` from `parse_ground_temperatures` if any slot remains `NaN` after the parse
   loop, forcing the caller to fall through to the DOE-2 model.
3. Rename `DOE2_GROUND_DEPTH_FACTOR` to `DOE2_GROUND_REFERENCE_DEPTH_M` and set the value to
   `0.5`. Update the constant comment to cite the DOE-2 GTEMP derivation.
4. Remove `DEFAULT_GROUND_TEMP_C` from every non-fallback use site; the only remaining use is the
   true last-resort return in `doe2_ground_temp_monthly` when `dry_bulb_c.is_empty()`, which should
   instead return an `Err`.

## Definition of Done

- `parse_ground_temperatures` selects the depth entry with minimum `|depth_m - 0.5|`.
- An EPW file with depth entries [0.1, 0.5, 2.0] selects the 0.5 m data.
- An EPW file with a malformed monthly value returns `None` from `parse_ground_temperatures`
  (triggering DOE-2 fallback), not a silently corrupted array.
- `DOE2_GROUND_DEPTH_FACTOR` no longer exists; the replacement constant is 0.5 m.
- `DEFAULT_GROUND_TEMP_C` is not used in any parse or fallback path that can silently substitute
  a value.

## Verification

```
cargo test -p hares-io epw
cargo test -p hares-io weather_parity
```

The EPW test suite must include a synthetic header with depths [0.1, 0.5, 2.0] and verify 0.5 m
is selected. A second case with a malformed monthly value must verify `None` is returned.

## References

- EPW Data Dictionary v9.6, §3 GROUND TEMPERATURES field — recommends 0.5 m for surface
  boundary conditions
- EnergyPlus Engineering Reference §3.1 "Ground Heat Transfer Calculations Using a Simplified
  Approach" — selects 0.5 m EPW depth
- Kusuda, T. and Achenbach, P.R. (1965), ASHRAE Transactions Vol. 71(1), pp. 61-74 — original
  depth-attenuation derivation underlying the DOE-2 GTEMP formula
