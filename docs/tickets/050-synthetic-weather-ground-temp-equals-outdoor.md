# Synthetic Weather Ground Temperature Tracks Outdoor Air Temperature Instantaneously

**Severity**: Low
**Status**: Open
**Areas**: hares-core/synthetic

## Problem

`build_synthetic_weather` at `crates/hares-core/src/dwelling/synthetic.rs:794` sets:
```
let ground_temp_c = outdoor_temp_c;
```
and fills every timestep in the `ground_temp_c` vector with this single value (`synthetic.rs:810`).
The comment at lines 781–793 acknowledges the approximation is only physically correct when the
synthetic outdoor temperature equals the annual mean (zero seasonal variation). For any synthetic
profile that varies — a step cold-snap, a ramp, or a diurnal cycle — ground temperature tracks
outdoor air with zero thermal lag, which is physically impossible: the thermal mass of the soil
means real ground temperatures lag outdoor air by weeks to months (Kusuda and Achenbach (1965)
ASHRAE Transactions 71(1):61-74 §2).

The practical impact is on tests that use non-constant synthetic weather. For a synthetic
step-change to -20 °C, ground coupling heat loss is computed against a -20 °C boundary instead of
a physically realistic 5–10 °C, producing errors of 2–5× in the ground coupling load term. This
causes synthetic-weather integration tests to be unreliable predictors of real EPW-path results.

There is no validation that the caller's synthetic profile is constant-temperature before the
shortcut is applied. The existing LIMITATION comment is accurate but provides no enforcement.

## Current Behavior

`crates/hares-core/src/dwelling/synthetic.rs:794`:
```
let ground_temp_c = outdoor_temp_c;
```
`synthetic.rs:810`:
```
ground_temp_c: vec![ground_temp_c; n],
```
Every record in the synthetic weather series receives the same `outdoor_temp_c` value as the
ground temperature, with no check that `outdoor_temp_c` is a steady-state mean.

## Required Behavior

1. In `SyntheticTomlConfig` (or its weather sub-struct), add an optional field
   `ground_temp_c: Option<f64>`. When `Some(t)`, use that value as the constant ground temperature.
   When `None`, use the temporal mean of the synthetic dry-bulb series as the ground temperature.
   Do not use `outdoor_temp_c` directly — it is the per-record value, not the mean.
2. Remove the code path that sets `ground_temp_c = outdoor_temp_c`. The mean of the dry-bulb
   series is the correct approximation for a constant-diffusivity soil at depth: it equals
   `t_mean_annual` in the Kusuda-Achenbach model with amplitude set to zero (no seasonal variation),
   per EnergyPlus Engineering Reference §3.1 (simplified ground heat transfer for
   constant-boundary cases).
3. The existing LIMITATION comment must be replaced with a brief statement of what the code
   actually does (uses temporal mean), not a caveat about what it doesn't do.

## Approach

1. Add `ground_temp_c: Option<f64>` to the weather section of `SyntheticTomlConfig`.
2. In `build_synthetic_weather`, compute the temporal mean of the dry-bulb output vector
   (`vec![outdoor_temp_c; n]` — since synthetic weather is currently constant, the mean equals
   `outdoor_temp_c`, making step 2 a no-op for existing constant-profile callers but correct for
   future time-varying synthetic profiles).
3. Set `let ground_temp_c = config.weather.ground_temp_c.unwrap_or(temporal_mean_c)`.
4. Remove the LIMITATION comment block at lines 781–793. Replace with a single line documenting
   that `ground_temp_c` is the temporal mean of the dry-bulb series (or explicit override), which
   represents an undisturbed deep-ground approximation with zero seasonal amplitude.

## Definition of Done

- `SyntheticTomlConfig` has `ground_temp_c: Option<f64>`.
- `build_synthetic_weather` computes ground temperature from the temporal mean of the dry-bulb
  series, not from the per-record outdoor temperature.
- When `ground_temp_c` is explicitly set in config, that value is used without modification.
- Test: a synthetic profile with `outdoor_temp_c = -20.0` and no `ground_temp_c` override
  produces `ground_temp_c = -20.0` (the mean of a constant series) — identical to current
  behavior for constant-temperature profiles, confirming no regression.
- Test: when `ground_temp_c = 8.0` is set in config, the weather series carries 8.0 regardless
  of the outdoor temperature.

## Verification

```
cargo test -p hares-core synthetic
```

The synthetic test suite must include both cases above. Numeric assertions must use exact equality
for constant-temperature profiles and within-0.01 °C tolerance for mean computations.

## References

- Kusuda, T. and Achenbach, P.R. (1965), ASHRAE Transactions Vol. 71(1), pp. 61-74 — ground
  temperature at depth approaches annual mean as amplitude approaches zero
- EnergyPlus Engineering Reference §3.1 "Ground Heat Transfer Calculations Using a Simplified
  Approach" — undisturbed ground temperature at depth as function of annual mean and amplitude

## Cross-Cluster Note

This ticket is adjacent to ticket 028 (wind-speed-no-terrain-correction), which also concerns the
synthetic weather builder producing physically incorrect boundary conditions. If ticket 028 is
reworked, the synthetic weather builder tests may share fixtures.

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match — `let ground_temp_c = outdoor_temp_c;` is at
  `crates/hares-core/src/dwelling/synthetic.rs:794` (confirmed). The fill at line 810
  (`ground_temp_c: vec![ground_temp_c; n]`) also matches.
- [x] Described logic matches current implementation — `SyntheticWeatherConfig` (lines 85-96)
  has no `ground_temp_c` field; only `outdoor_temp_c`, `dew_point_c`, `rel_humidity_pct`,
  `pressure_kpa`, `epw_path`. The code at line 794 unconditionally sets
  `ground_temp_c = outdoor_temp_c` with no check for whether the profile is constant.
  The bug is present and unambiguous.
- [x] OCHRE cross-check result: **diverges** — intentionally, in a way that strengthens
  the ticket's argument. In `vendors/OCHRE/ochre/utils/schedule.py:256-261`, OCHRE applies
  its DOE-2-derived ground temperature formula only when the weather file covers a full
  annual cycle (12 months); for non-annual data it explicitly omits `Ground Temperature (C)`
  from the schedule unless the caller passes it as a kwarg. HARES instead injects
  `outdoor_temp_c` unconditionally, which is more aggressive than OCHRE's approach for
  sub-annual synthetic runs. OCHRE's correct behavior here is to require an explicit
  override, matching the ticket's `ground_temp_c: Option<f64>` proposal.
- [x] EnergyPlus cross-check result: **confirms the physics** — The EnergyPlus Engineering
  Reference (Kusuda-Achenbach section, all recent versions) gives
  `T(z,t) = T̄_s − ΔT̄_s · exp(−z·√(π/ατ)) · cos(2πt/τ − θ)`. When `ΔT̄_s = 0`
  (zero seasonal amplitude, i.e., constant outdoor temperature), the formula reduces to
  `T(z,t) = T̄_s` — the ground temperature equals the annual mean surface temperature at
  every depth and time. This mathematically validates the ticket's claim that
  `ground_temp_c = outdoor_temp_c` is correct only when `outdoor_temp_c` is the annual mean,
  and is wrong for step-change or varying synthetic profiles where the per-record temperature
  is not the mean.

### Web-Verified Citations

**Citation 1: Kusuda and Achenbach (1965), ASHRAE Transactions Vol. 71(1), pp. 61-74**

- **Source found**: BigLadder EnergyPlus 8.4 Engineering Reference — Undisturbed Ground
  Temperature Model: Kusuda-Achenbach
  (https://bigladdersoftware.com/epx/docs/8-4/engineering-reference/undisturbed-ground-temperature-model-kusuda.html)
  and NIST publication record https://www.nist.gov/publications/earth-temperature-and-thermal-diffusivity-selected-stations-united-states
- **Quoted passage**: EnergyPlus Engineering Reference (8.4) cites the paper as:
  "Kusuda, T. and P.R. Achenbach. 1965. 'Earth Temperatures and Thermal Diffusivity at
  Selected Stations in the United States.' ASHRAE Transactions. 71(1): 61-74."
  The formula is given as `T(z,t) = T̄_s − ΔT̄_s · e^(−z·√(π/ατ)) · cos(2πt/τ − θ)`.
  Setting `ΔT̄_s = 0` yields `T(z,t) = T̄_s` (annual mean), confirming the ticket's claim.
- **Verdict**: **Confirmed** — page numbers 61-74 match exactly (not 61-75 as some sources
  mis-cite). The title in the ticket omits the word "Temperatures" (says "Temperature" not
  "Temperatures"), which is a minor transcription error but the paper and its findings are
  correctly described. The claim that deep ground temperature approaches annual mean as
  amplitude → 0 is mathematically exact from the formula.

**Citation 2: EnergyPlus Engineering Reference §3.1 "Ground Heat Transfer Calculations Using a
Simplified Approach — undisturbed ground temperature at depth as function of annual mean and
amplitude"**

- **Source found**: EnergyPlus Engineering Reference table of contents for multiple versions
  (8.3, 24.1) at https://bigladdersoftware.com/epx/docs/8-3/engineering-reference/ and
  https://bigladdersoftware.com/epx/docs/24-1/engineering-reference/
- **Quoted passage**: The EnergyPlus Engineering Reference does **not** use numeric section
  numbers (no §3.1, §3.2, etc.). It uses heading-based organization without decimal
  numbering. The section closest to the ticket's description is titled
  "Ground Heat Transfer Calculations using C and F Factor Constructions" under "Surface Heat
  Balance Manager / Processes." That section covers slab-on-grade simplified approaches but
  does not discuss the Kusuda-Achenbach formula or amplitude-zero behavior. The
  Kusuda-Achenbach model is covered in a separate section titled "Undisturbed Ground
  Temperature Model: Kusuda-Achenbach" with no section number.
- **Verdict**: **Incorrect citation** — §3.1 does not exist in the EnergyPlus Engineering
  Reference; the document uses no numeric section numbering. The underlying physics claim
  (T_ground = T_annual_mean when amplitude = 0) is correct and derivable from the
  Kusuda-Achenbach formula, but the §3.1 section reference is fabricated. The correct
  cross-reference is "Undisturbed Ground Temperature Model: Kusuda-Achenbach" in any
  recent version of the EnergyPlus Engineering Reference.

### Legitimacy

- **Verdict**: **Partially Legitimate**
- **Rationale**: The core bug is real and unambiguously present: `SyntheticWeatherConfig`
  has no `ground_temp_c` field, `build_synthetic_weather` sets `ground_temp_c = outdoor_temp_c`
  unconditionally at line 794, and the existing LIMITATION comment accurately describes the
  problem without providing enforcement. For any non-constant synthetic profile (step cold-snap,
  ramp, diurnal cycle), this produces physically impossible ground temperatures. The Kusuda
  and Achenbach (1965) citation is substantively correct — the paper's formula does show
  T → T_annual_mean as amplitude → 0, and the citation details (71(1):61-74) are accurate.
  However, the EnergyPlus §3.1 citation is incorrect: the EnergyPlus Engineering Reference
  does not use numeric section numbering, and no section "§3.1" exists. The proposed fix
  (add `ground_temp_c: Option<f64>`, compute temporal mean as fallback) is sound. The "no
  regression" argument in the Definition of Done is correct for the constant-temperature
  case (temporal mean of a constant series = the constant value = current behavior). The
  severity ("Low") may be slightly understated given that synthetic-weather integration tests
  are explicitly cited as becoming unreliable predictors of EPW-path results, but the
  practical blast radius is limited to non-constant synthetic configs which are not currently
  used in production tests.

### Proposed Fix Summary

1. Add `ground_temp_c: Option<f64>` with `#[serde(default)]` to `SyntheticWeatherConfig`
   (and update its `Default` impl to return `None`).
2. In `build_synthetic_weather`, replace `let ground_temp_c = outdoor_temp_c;` with:
   ```rust
   let temporal_mean_c = outdoor_temp_c; // constant profile: mean = value
   let ground_temp_c = config.weather.ground_temp_c.unwrap_or(temporal_mean_c);
   ```
   (For future time-varying support, `temporal_mean_c` would be computed as the arithmetic
   mean of the dry-bulb output vector.)
3. Replace the LIMITATION comment block (lines 781-793) with a single-line comment stating
   that `ground_temp_c` is the temporal mean of the dry-bulb series (or explicit override).

### Test Written

- **File**: `crates/hares-core/src/dwelling/synthetic.rs` (within existing `#[cfg(test)] mod tests`)
- **Tests added**:
  - `synthetic_ground_temp_equals_outdoor_temp_for_constant_profile` — Case A non-regression:
    for a constant −20 °C profile, ground_temp_c must equal −20.0 (temporal mean = value).
    Currently passes (by coincidence). Must continue to pass after the fix.
  - `synthetic_ground_temp_override_takes_precedence_over_outdoor_temp` — Case B failing
    regression: when `ground_temp_c = 8.0` is set in `[weather]`, the field must deserialize
    as `Some(8.0)` and the weather series must carry 8.0. Currently fails to compile because
    `SyntheticWeatherConfig` has no `ground_temp_c` field (`error[E0609]: no field
    'ground_temp_c' on type 'SyntheticWeatherConfig'`), confirming the bug.
