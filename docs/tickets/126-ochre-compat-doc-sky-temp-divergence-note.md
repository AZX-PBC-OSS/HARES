# `ochre_compat()` Doc Should Note Sky-Temperature Divergence

**Severity**: Nit
**Priority**: P4
**Status**: Open
**Areas**: hares-io/weather

## Problem

`ochre_compat()` at `crates/hares-io/src/weather.rs:320` is documented as the OCHRE-compatible weather-resampling configuration, but the doc does not note that the resulting sky temperature stream still diverges from OCHRE's pre-computed zero-order-hold (ZOH) sky temperature. HARES recomputes sky temperature from the interpolated dew-point and dry-bulb inputs at each resampled timestep, so the result is physically consistent with the (smooth) interpolated humidity inputs — but it is not bit-identical to OCHRE, which carries forward whatever sky temperature the source weather file declared at the hour boundary.

A user enabling `ochre_compat()` expecting bit-identical OCHRE behaviour will be surprised by sky-temperature differences that are entirely correct but not reproduced.

## Current Behavior

`crates/hares-io/src/weather.rs:320` (approximately):
```rust
/// OCHRE-compatible defaults: ZOH for all channels.
pub fn ochre_compat() -> Self { ... }
```

Doc does not mention sky temperature.

## Required Behavior

The doc comment must add a note explaining:

1. HARES recomputes sky temperature at each resampled timestep from interpolated dry-bulb and dew-point inputs (Berdahl-Martin form, see `epw.rs`).
2. OCHRE carries the source-file sky temperature forward via ZOH and never recomputes.
3. Therefore, even with `ochre_compat()` selected, sky temperature will differ from OCHRE between hour boundaries — HARES is more physically consistent (no humidity/sky-temperature mismatch) but not bit-identical.

## Approach

Update the doc comment at `crates/hares-io/src/weather.rs:320`:

```rust
/// OCHRE-compatible defaults: zero-order-hold for all interpolated channels.
///
/// Note: even with this configuration selected, the resampled sky temperature
/// stream is not bit-identical to OCHRE's. OCHRE carries the source-file sky
/// temperature forward via ZOH, while HARES recomputes sky temperature at
/// each resampled timestep from the interpolated dry-bulb and dew-point
/// inputs (see `epw::berdahl_martin_sky_temp`). HARES's behaviour is
/// physically more consistent — the sky temperature always agrees with the
/// humidity that produced it — but the resulting stream will differ from
/// OCHRE between hour boundaries.
pub fn ochre_compat() -> Self { ... }
```

## Definition of Done

- [ ] Doc comment at `crates/hares-io/src/weather.rs:320` updated with the divergence note
- [ ] Comment cites the recomputation site (`epw::berdahl_martin_sky_temp`) so a future reader can find the source

## Verification

```bash
cargo doc -p hares-io --no-deps
cargo test -p hares-io weather
```

## References

- HARES `crates/hares-io/src/epw.rs:529-531` — sky-temperature recomputation site (Berdahl-Martin form).
- HARES `vendors/OCHRE/ochre/utils/schedule.py` — OCHRE's ZOH-only sky-temperature handling.
- EnergyPlus Engineering Reference §3.5.6 "Sky Emissivity Calculations" — primary-source justification for the recomputation approach.

## Related Tickets

- 097-tmy3-midpoint-offset-regression-test (related ochre_compat regression coverage)
- 098-triangular-resample-docstring-or-mean-preserving (related resample-doc accuracy)

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match (with correction): ticket says `weather.rs:320` (approximately); actual location is `weather.rs:324`. The `(approximately)` qualifier in the ticket is correct — the function is at line 324, shifted 4 lines from the stated 320.
- [x] Described logic matches current implementation: `ochre_compat()` sets all fields to `ResampleMethod::Zoh` but does **not** list `sky_temp_c`, because `sky_temp_c` is always recomputed post-resample from the ZOH-interpolated inputs via `compute_sky_temp_c`. This is confirmed at `weather.rs:524-535`.
- [x] OCHRE cross-check result: **diverges — as described by the ticket**. `vendors/OCHRE/ochre/utils/schedule.py:553` shows OCHRE uses `df.resample(time_res).ffill()` (pandas forward-fill = ZOH) for all weather columns including `sky_temperature`. OCHRE computes `sky_temperature` once at load time from the EPW's `ghi_infrared` field (`schedule.py:182`): `df["sky_temperature"] = convert((df["ghi_infrared"].values / 5.6697e-8) ** 0.25, "K", "degC")`, then carries that pre-computed value forward via ZOH. HARES instead recomputes sky temperature from the ZOH-interpolated dew-point, dry-bulb, infrared, and sky-cover inputs at every resampled timestep. The divergence is intentional: HARES corrects a known limitation (recomputing from smooth inputs avoids the humidity/sky-temperature inconsistency EnergyPlus issue #8030 identifies).
- [x] EnergyPlus cross-check result: **confirms HARES approach**. EnergyPlus Engineering Reference "Sky Radiation Modeling" / "EnergyPlus Sky Temperature Calculation" sections (no numeric §3.5.6 designation exists — see citation note below) document the same formulas HARES uses. EnergyPlus issue #8030 ("Sky emissivity and sky temperature calculation should use timestep-wise inputs instead of hourly ones", merged as PR #8143) explicitly identifies that computing sky temperature from hourly inputs then interpolating — exactly what OCHRE does — produces errors of 0.15–1.6 °C, and proposes and merges the fix of recomputing sky temperature from already-interpolated inputs at each timestep. HARES implements this corrected approach. The HARES comment at `weather.rs:528` citing `WeatherManager.cc:3113` refers to the post-fix EnergyPlus implementation.
- [x] `epw.rs` reference: ticket cites `epw.rs:529-531` as the recomputation site; actual function `berdahl_martin_sky_emissivity` starts at line 535, but the Berdahl-Martin formula lines (coefficients) are at 536–537. The `compute_sky_temp_c` dispatcher that calls it is at `epw.rs:486–505`. The ticket's line reference is slightly off (529-531 vs 535-537) but points to the correct function.

### Web-Verified Citations

**Citation 1**: EnergyPlus Engineering Reference §3.5.6 "Sky Emissivity Calculations"

- **Source found**: [EnergyPlus Engineering Reference — Climate Calculations (v9.6)](https://bigladdersoftware.com/epx/docs/9-6/engineering-reference/climate-calculations.html) and [v24.1](https://bigladdersoftware.com/epx/docs/24-1/engineering-reference/climate-calculations.html)
- **Quoted passage**: "By default the Sky Temperature (Tsky) is calculated from the Horizontal Infrared Radiation Intensity (IRH): Tsky = (IRH/σ)^0.25 − 273.15" (section heading: "EnergyPlus Sky Temperature Calculation"). For sky emissivity under "Sky Radiation Modeling": "ϵsky,clear = 0.758 + 0.521(Tdp/100) + 0.625(Tdp/100)²" (Martin & Berdahl model).
- **Verdict**: **Partially correct**. The formulas and content are real and confirmed. However, **the section number §3.5.6 does not exist** in any version of the EnergyPlus Engineering Reference examined (v8.3, v9.3, v9.5, v9.6, v23.1, v24.1). The Climate Calculations chapter uses only prose headings ("Sky Radiation Modeling", "EnergyPlus Sky Temperature Calculation") with no numeric section designators. The section title "Sky Emissivity Calculations" also does not appear verbatim — the actual heading is "Sky Radiation Modeling". The citation should be updated to remove the §3.5.6 number and correct the heading.

**Citation 2**: HARES `crates/hares-io/src/epw.rs:529-531` — sky-temperature recomputation site (Berdahl-Martin form)

- **Source found**: Read directly from the codebase.
- **Quoted passage** (actual lines 535–537): `pub fn berdahl_martin_sky_emissivity(t_dp_c: f64) -> f64 { let x = t_dp_c / 100.0; 0.758 + 0.521 * x + 0.625 * x * x }`
- **Verdict**: **Partially correct**. The function exists and implements the correct Berdahl-Martin formula, but the cited lines 529–531 are off — they fall inside the `clark_allen_sky_temp_c` function body, not Berdahl-Martin. The function `berdahl_martin_sky_emissivity` starts at line 535. Additionally, the primary recomputation *site* (where HARES decides which formula to use) is `compute_sky_temp_c` at line 486, which routes to Berdahl-Martin as the second fallback. The ticket's proposed doc comment correctly names `epw::berdahl_martin_sky_emissivity` which is accurate; only the line number in the References section is slightly off.

**Citation 3**: `vendors/OCHRE/ochre/utils/schedule.py` — OCHRE's ZOH-only sky-temperature handling

- **Source found**: Read directly from the codebase at `/Users/rich/source/HARES/vendors/OCHRE/ochre/utils/schedule.py`.
- **Quoted passage** (line 553): `df = df.resample(time_res).ffill()` with comment "normally, just use pad (forward fill)". Sky temperature is computed at load time (line 182): `df["sky_temperature"] = convert((df["ghi_infrared"].values / 5.6697e-8) ** 0.25, "K", "degC")` and is subsequently ZOH-resampled like all other weather fields.
- **Verdict**: **Confirmed**. OCHRE does carry sky temperature forward via ZOH (forward fill) and does not recompute it from the interpolated humidity inputs at sub-hourly timesteps.

### Legitimacy

- **Verdict**: **Partially Legitimate**

- **Rationale**: The core claim is real and important: `ochre_compat()` does not document that sky temperature will still diverge from OCHRE's values even when ZOH is selected for all other fields. The divergence exists, is intentional, and follows the physically correct approach (endorsed by EnergyPlus issue #8030 / PR #8143). The OCHRE cross-reference at `schedule.py:553` confirms the ZOH forward-fill behavior. The Berdahl-Martin formula and the EnergyPlus climate calculation content cited are genuine and correctly described. However, two details need correction: (1) the EnergyPlus section reference "§3.5.6 'Sky Emissivity Calculations'" is fabricated — no such section number or title exists in any version of the Engineering Reference; the correct citation is the "Sky Radiation Modeling" / "EnergyPlus Sky Temperature Calculation" sections in the Climate Calculations chapter; (2) the `epw.rs:529-531` line reference is off by ~6 lines (correct location is 535–537 for `berdahl_martin_sky_emissivity`, or 486 for `compute_sky_temp_c`). The fix itself (updating the `ochre_compat()` doc comment) is correct and minimal.

### Proposed Fix Summary

Update the `ochre_compat()` doc comment at `weather.rs:324` to add the divergence note as described in the ticket. The proposed comment text in the ticket is accurate and sufficient. Optionally update the `epw.rs` line reference in the ticket's References section from `529-531` to `486-505` (for `compute_sky_temp_c`) or `535-537` (for `berdahl_martin_sky_emissivity`), and update the EnergyPlus citation to remove the non-existent §3.5.6 number and use the heading "Sky Radiation Modeling / EnergyPlus Sky Temperature Calculation" in the Climate Calculations chapter.

### Test Written

- File: `none needed`
- What it tests: N/A — regression tests covering the sky-temperature recomputation behavior already exist at `crates/hares-io/src/weather.rs:1567` (`sky_temp_recomputed_after_upsampling`) and `weather.rs:1617` (`sky_temp_recomputed_after_downsampling`). These tests verify that after resampling, `sky_temp_c` matches `compute_sky_temp_c` applied to the interpolated inputs, which is exactly the invariant the ticket describes. The ticket is a documentation-only change; no additional failing test is needed or appropriate.
