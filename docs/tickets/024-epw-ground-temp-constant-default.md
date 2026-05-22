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

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-20

### Code Confirmation

- [x] Referenced line numbers still match (or note corrected location)
  - Line 23: `DEFAULT_GROUND_TEMP_C = 10.0` ✓
  - Line 342: `let mut monthly = [DEFAULT_GROUND_TEMP_C; 12];` ✓
  - Line 354: `if valid && depth_m < best_depth {` ✓ — selects minimum depth, not closest to 0.5 m
  - Line 371: `const DOE2_GROUND_DEPTH_FACTOR: f64 = 10.0;` ✓
  - Line 224: `ground_temp_c: DEFAULT_GROUND_TEMP_C,` ✓ (interim placeholder before interpolation,
    overwritten at line 246 — not a silent substitution for valid data)
  - Line 424: `return [DEFAULT_GROUND_TEMP_C; 12];` ✓ (last-resort empty-slice fallback)
- [x] Described logic matches current implementation — confirmed
- [x] OCHRE cross-check result: **HARES matches OCHRE** for the DOE-2 fallback formula
  - `vendors/OCHRE/ochre/utils/schedule.py:248`:
    `beta = (np.pi / (8760 * 0.025)) ** 0.5 * 10` — identical depth factor (10) and
    diffusivity (0.025) as HARES constants.
  - OCHRE also does NOT parse or use EPW GROUND TEMPERATURES header data; it always
    uses the DOE-2 fallback formula.  Bug 1 (depth selection) is therefore unique to
    HARES; OCHRE is not exposed to it because OCHRE never reads EPW ground temp headers.
- [x] EnergyPlus cross-check result: **partial match / diverges on diffusivity and intended use**
  - EnergyPlus WeatherConverter uses `α = 2.3225760E-03 m²/day ≈ 9.677E-05 m²/hr` to
    generate EPW GROUND TEMPERATURES headers at three fixed depths: **0.5 m, 2.0 m, and
    4.0 m** (confirmed by BigLadder EnergyPlus 8.5 Auxiliary Programs docs and the
    Unmet Hours thread with Joe Huang).
  - HARES/OCHRE use `α = 0.025 m²/hr`, which is **258× larger** than the EnergyPlus
    value.  At that larger α, the attenuation at 10 m is ~0.55 (not 0.018 as the ticket
    claims), and at 0.5 m it is ~0.97 (not 0.82 as the ticket claims).
  - The ticket's numerical attenuation claims (0.018 at 10 m; 0.82 at 0.5 m) are only
    correct when using the EnergyPlus diffusivity.  They are incorrect for the
    HARES/OCHRE diffusivity of 0.025 m²/hr.
  - The 0.5 m depth is used by EnergyPlus for `GroundTemperatures:Surface` (shallow).
    The EPW spec note is: *"the 'undisturbed' ground temperatures calculated by the
    weather converter should not be used in building losses"* — which means the 0.5 m
    entry is appropriate for surface boundary conditions but not for floor/foundation
    losses without further processing.

### Web-Verified Citations

**Citation 1**: EPW Data Dictionary v9.6 §3 GROUND TEMPERATURES — recommends 0.5 m for
surface boundary conditions.

- **Source found**: BigLadder EnergyPlus 9.6 Auxiliary Programs —
  `bigladdersoftware.com/epx/docs/9-6/auxiliary-programs/energyplus-weather-file-epw-data-dictionary.html`
- **Quoted passage**: *"the 'undisturbed' ground temperatures calculated by the weather
  converter should not be used in building losses but are appropriate to be used in the
  GroundTemperatures:Surface and GroundTemperatures:Deep objects"*; and *"with the FC
  construction option, these are automatically selected (.5 depth) for use if the user
  does not include values"*
- **Verdict**: **Partially correct.** The EPW spec does associate 0.5 m with surface
  boundary conditions (`GroundTemperatures:Surface`) and the FC option defaults to 0.5 m.
  However, it does not say EnergyPlus's own EPW parser *selects* 0.5 m; it says these
  undisturbed temperatures should not be used directly for building heat loss. The
  standard EPW format provides three fixed depths: 0.5 m, 2.0 m, 4.0 m (confirmed by
  Joe Huang and BigLadder EPW CSV Format docs).

**Citation 2**: EnergyPlus Engineering Reference §3.1 "Ground Heat Transfer Calculations
Using a Simplified Approach" — selects 0.5 m EPW depth.

- **Source found**: BigLadder EnergyPlus Engineering Reference (multiple versions
  searched: 8.0–25.1).  The section titled "Ground Heat Transfer Calculations Using a
  Simplified Approach" describes the C/F-factor construction method, which creates a
  two-layer equivalent construction.  No web-accessible version of §3.1 explicitly
  states "select the 0.5 m EPW ground temperature entry."
- **Quoted passage**: N/A — the cited section describes a *construction* approach
  (concrete layer + fictitious insulation), not EPW depth selection logic.  The depth
  selection for the FC option is documented in the Auxiliary Programs guide (citation 1
  above), not the Engineering Reference §3.1.
- **Verdict**: **Citation section number is inaccurate.** The EnergyPlus Engineering
  Reference §3.1 (Ground Heat Transfer via C/F-factor) does not contain the claimed
  content.  The closest correct reference for depth selection is the EPW Data Dictionary
  Auxiliary Programs guide.

**Citation 3**: Kusuda, T. and Achenbach, P.R. (1965), ASHRAE Transactions Vol. 71(1),
pp. 61-74 — original depth-attenuation derivation.

- **Source found**: Semantic Scholar entry for *"Earth Temperature and Thermal Diffusivity
  at Selected Stations in the United States"*; EnergyPlus Engineering Reference —
  Undisturbed Ground Temperature Model: Kusuda-Achenbach
  (`bigladdersoftware.com/epx/docs/9-5/engineering-reference/undisturbed-ground-temperature-model-kusuda.html`)
- **Quoted passage**: *"T(z,t) = T̄s − Δ T̄s · e^(−z·√(π/ατ)) · cos(2πt/τ − θ)"* where
  *z* is depth, *α* is thermal diffusivity, *τ* = 365 days.
- **Verdict**: **Confirmed.** Paper title, journal, and year are correct. The formula is
  indeed the depth-attenuation basis for DOE-2 GTEMP and EnergyPlus.

**Additional finding — DOE-2 GTEMP original depth parameter**:
- Joe Huang (EnergyPlus support thread, archived at onebuilding.org) confirmed that the
  original DOE-2 GTEMP routine used *"5 ft down, 1.0 diffusivity for moist soil (IP
  units)"* held fixed.  5 ft ≈ 1.524 m, not 10 m and not 0.5 m.
- The HARES/OCHRE constant of 10 for `beta = (π / (8760 × 0.025))^0.5 × 10` is
  therefore neither the original DOE-2 depth (5 ft) nor the proposed fix (0.5 m).
  However, the unit of the multiplier 10 is unclear — it could be a dimensionless
  scaling factor rather than a depth in metres.  The OCHRE comment says "same correlation
  as DOE-2's src\WTH.f file, subroutine GTEMP" without explaining the factor.

### Legitimacy

- **Verdict**: **Partially Legitimate**
- **Rationale**:
  - **Bug 1 (depth selection) — Legitimate.** `depth_m < best_depth` at line 354
    selects the shallowest available depth rather than the closest to 0.5 m.  The Denver
    TMY3 EPW has depths [0.5, 2.0, 4.0] so this happens to be correct by accident.  An
    EPW with depths [0.1, 0.5, 2.0] would select 0.1 m (the diurnal layer), which is
    wrong.  The bug is real and the fix (`(depth_m - 0.5).abs() < (best - 0.5).abs()`)
    is correct.
  - **Bug 2 (silent sentinel) — Partially Legitimate.** Using `DEFAULT_GROUND_TEMP_C`
    to initialise the monthly array prior to the parse loop is a valid concern: a partial
    parse failure silently substitutes 10 °C.  However, the ticket's claim that
    `EpwRecord.ground_temp_c` at line 224 is a "silent default" is inaccurate — that
    field is a placeholder immediately overwritten by the interpolation loop at line 246.
    The genuine defect is in `parse_ground_temperatures` only.  The fix (NaN
    initialisation + None propagation) is appropriate.
  - **Bug 3 (depth factor 10.0) — Partially Legitimate with inaccurate numerics.**
    The constant `DOE2_GROUND_DEPTH_FACTOR = 10.0` is not the DOE-2 original (5 ft ≈
    1.524 m) and not the proposed target (0.5 m), so it represents an incorrect value.
    The fix to 0.5 m is physically motivated (matching the EPW reference depth).
    However, the ticket's specific attenuation claims (`exp(-10×...) ≈ 0.018`, `0.5 m ≈
    0.82`) are computed with the **EnergyPlus diffusivity** (`2.3225760E-03 m²/day`),
    not the HARES/OCHRE diffusivity (`0.025 m²/hr`).  With the HARES diffusivity, the
    10 m attenuation factor is ~0.55 and the 0.5 m factor is ~0.97 — still an
    over-damping relative to a shallow depth, but not "effectively eliminating all
    seasonal variation."  The cited formula for the 0.5 m attenuation (`exp(-0.5 × 0.4)
    ≈ 0.82`) is dimensionally inconsistent with the constants in the code.
  - **Citation §3.1 — Inaccurate.** The EnergyPlus Engineering Reference §3.1 does not
    contain the 0.5 m EPW depth selection guidance claimed.  The correct source is the
    EPW Data Dictionary (Auxiliary Programs guide).

### Proposed Fix Summary

1. **Bug 1 — depth selection**: Replace `if valid && depth_m < best_depth` with
   `if valid && (depth_m - 0.5_f64).abs() < (best_depth - 0.5_f64).abs()`, initialise
   `best_depth = f64::INFINITY`.  Add `const TARGET_DEPTH_M: f64 = 0.5_f64` for
   clarity.
2. **Bug 2 — silent sentinel**: Change the monthly initialisation array from
   `[DEFAULT_GROUND_TEMP_C; 12]` to `[f64::NAN; 12]`, and return `None` from
   `parse_ground_temperatures` if any slot remains `NaN` after the parse loop.  Remove
   `DEFAULT_GROUND_TEMP_C` from parse paths.
3. **Bug 3 — depth factor**: Rename `DOE2_GROUND_DEPTH_FACTOR` to
   `DOE2_GROUND_REFERENCE_DEPTH_M` and set to `0.5`.  This is the physically motivated
   choice (matches EPW surface reference depth).  Note that the correct value relative
   to the original DOE-2 code is closer to 1.524 m (5 ft); both 0.5 m and 1.524 m are
   substantially different from the current 10.0.  The ticket's rationale for 0.5 m
   (matching the EPW reference depth selection) is sound even if the attenuation
   arithmetic in the ticket contains errors due to diffusivity unit confusion.

### Test Written

- **File**: `crates/hares-io/src/epw.rs` (within existing `#[cfg(test)]` module)
- Tests added:
  1. `ground_temp_depth_selection_picks_0_5_m_from_0_5_2_4_depths` — verifies that
     with the standard [0.5, 2.0, 4.0] m depths the 0.5 m entry is selected (passes
     currently by luck; will validate the correct fix too).
  2. `ground_temp_depth_selection_picks_0_5_m_not_0_1_m` — `#[should_panic]` test:
     with depths [0.1, 0.5, 2.0] the current code picks 0.1 m instead of 0.5 m,
     causing the assertion (which checks for ≈5.0) to fire.  This test **must be
     changed from `#[should_panic]` to a normal passing test after the fix.**
  3. `doe2_ground_depth_factor_10m_overdamps_vs_0_5m` — verifies that the current
     depth factor (10 m) produces a ground-temperature amplitude ratio
     (`gm ≈ 0.551`) noticeably below what the correct 0.5 m reference depth would
     give (`gm ≈ 0.970`), confirming the over-damping defect.
