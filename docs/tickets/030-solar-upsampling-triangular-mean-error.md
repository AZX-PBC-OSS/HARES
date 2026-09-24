# Solar Upsampling: Triangular Default Does Not Preserve Hourly Energy Budget

**Severity**: Medium
**Impact on annual kWh**: Medium
**Status**: Open
**Areas**: hares-io/weather (resample)

## Problem

GHI, DNI, and DHI use `ResampleMethod::Triangular` as the default resampling method when upsampling hourly weather data to sub-hourly resolution (`crates/hares-io/src/weather.rs:553-566`).

The triangular tent function is centered at the midpoint of each hour so that `interpolant(midpoint) == hourly_value`. However, the mean of the triangular interpolant over the full hour is not equal to the original hourly value. For a tent centered at hour `i`:

```
hourly_mean = values[i-1]/4 + values[i]/2 + values[i+1]/4
```

This is a weighted blend with neighboring hours. At sunrise/sunset transitions, where one hour has significant irradiance and the adjacent hour has zero (nighttime), the method bleeds energy across the boundary. At 15-minute resolution, a last-sunlit-hour value of 400 W/m² produces sub-hourly values of [400, 300, 200, 100] W/m² and the nighttime hour receives [100, 0, 0, 0] W/m² — a 25 W/m² mean in an hour that should be zero.

Hourly solar irradiance values represent period averages, not instantaneous midpoint measurements (ASHRAE Handbook of Fundamentals 2021 Ch. 14 §14.6). Sub-hourly upsampling must preserve the hourly integral: the mean of all sub-hourly values within an hour must equal the original hourly value.

EnergyPlus WeatherManager.cc `SetupInterpolationValues` (approximately lines 8328–8380) uses a weighted blend that satisfies this conservation constraint exactly.

## Current Behavior

`crates/hares-io/src/weather.rs:553-566`: GHI, DNI, DHI default to `ResampleMethod::Triangular`.

At sunrise/sunset, triangular resampling bleeds approximately 10–25 W/m² of solar energy into adjacent nighttime hours. The nighttime hour that physically has zero irradiance receives nonzero sub-hourly values, biasing cooling loads in hours that should have no solar input.

OCHRE uses zero-order hold (`resample().ffill()`, equivalent to `ResampleMethod::Zoh`) for solar resampling, which preserves hourly averages exactly by holding each hourly value constant within its hour.

## Required Behavior

Change the default resampling method for GHI, DNI, and DHI from `ResampleMethod::Triangular` to `ResampleMethod::Zoh`. ZOH holds each hourly value constant within its interval, preserving the hourly energy integral exactly. The reduction in temporal smoothness is the correct tradeoff: physical solar irradiance is not smooth at sub-hourly scales, and the triangular method's smoothness comes at the cost of energy conservation.

`ResampleMethod::Triangular` must remain available via `ResampleOverrides` for users who explicitly want smooth sub-hourly profiles and accept the energy conservation deviation. The `Triangular` docstring must state explicitly that it does not preserve hourly energy integrals and quantify the expected boundary error (up to ±25 W/m² at sunrise/sunset).

No silent defaults: the ZOH default is a documented, citable choice. The `Triangular` override is a documented deviation requiring explicit user opt-in.

Primary citations:
- ASHRAE Handbook of Fundamentals 2021 Ch. 14 §14.6 "Solar Radiation" — hourly values are period averages; sub-hourly upsampling must preserve the hourly integral
- EnergyPlus Engineering Reference §2.5.5 "Solar Radiation" — energy-conservative upsampling approach

## Approach

In `crates/hares-io/src/weather.rs`, locate the `resample_with` function's default method selection for `ghi`, `dni`, `dhi` columns. Change from `ResampleMethod::Triangular` to `ResampleMethod::Zoh`. Update the `Triangular` docstring to add the energy conservation warning. Update any tests that assert `Triangular` as the solar default.

## Definition of Done

- [ ] Default resampling for GHI, DNI, DHI changed to `Zoh`
- [ ] Test: hourly mean of ZOH sub-hourly GHI values equals original hourly GHI for a representative sunrise/sunset sequence (max absolute error < 1e-9 W/m²)
- [ ] Test: triangular method at a sunset boundary produces a nonzero mean in the nighttime hour (regression guard documenting the known limitation)
- [ ] `ResampleMethod::Triangular` docstring states it does not preserve hourly energy integrals
- [ ] `ResampleOverrides` allows explicit `Triangular` selection for solar channels

## Verification

```bash
cargo test -p hares-io resample
cargo test -p hares-io weather_parity
```

Specific invariant: for a sequence `[400.0, 0.0, 0.0]` (W/m², last sunlit hour followed by two nighttime hours), ZOH sub-hourly means at 15-minute resolution must equal `[400.0, 0.0, 0.0]` exactly.

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-20

### Code Confirmation
- [x] Referenced line numbers still match — `crates/hares-io/src/weather.rs:554-571` contains the three `resample_field` calls defaulting to `ResampleMethod::Triangular` for GHI, DNI, DHI. (Ticket cited 553–566; actual span is 554–571 — off by one but functionally correct.)
- [x] Described logic matches current implementation — `triangular_resample` at `weather.rs:1065-1099` implements the midpoint-tent formula exactly as described.
- [x] OCHRE cross-check result: **matches ticket claim**. `vendors/OCHRE/ochre/utils/schedule.py:553` uses `df.resample(time_res).ffill()` (ZOH/forward-fill) as the default for all weather columns, including GHI/DNI/DHI. `vendors/OCHRE/ochre/Simulator.py:196` calls `resample_and_reindex` without `interpolate=True`, confirming ZOH is always used. HARES diverges intentionally (the comment at line 554 says "smoother than ZOH"), but the divergence causes energy non-conservation.
- [x] EnergyPlus cross-check result: **ticket's EnergyPlus claim is partially inaccurate**. EnergyPlus Engineering Reference, section "Weather File Solar Interpolation" (Climate Calculations chapter, all versions 8.2–25.1; bigladdersoftware.com) states: *"The solar values on the weather file are average values over the hour. For interpolation of hourly weather data (i.e., when the specified timestep is greater than 1), the average value is assumed to be the value at the midpoint of the hour. … [H]ourly reported values will not match the original values in the weather file, but the total solar for a day should agree."* This confirms solar values are period averages (supporting the ticket's core claim), but the EnergyPlus method is a **linear** last-hour/this-hour interpolation (`ValueTimeStep = LastHourValue × WeightLastHour + ThisHourValue × WeightThisHour`), not a "weighted blend that satisfies this conservation constraint exactly." EnergyPlus also does not conserve hourly energy — only daily totals. The ticket's characterisation of EnergyPlus as using an energy-conservative approach is incorrect.

### Web-Verified Citations

**Citation 1**: "ASHRAE Handbook of Fundamentals 2021 Ch. 14 §14.6 — Solar Radiation — hourly values are period averages"
- **Source found**: ASHRAE official site, [Table of Contents 2021 ASHRAE Handbook—Fundamentals](https://www.ashrae.org/technical-resources/ashrae-handbook/table-of-contents-2021-ashrae-handbook-fundamentals); also [Chapter 14 Climatic Design Information (2017 edition, same chapter)](https://handbook.ashrae.org/Handbooks/F17/IP/f17_ch14/f17_ch14_ip.aspx)
- **Quoted passage**: The 2021 (and 2025) ASHRAE Handbook—Fundamentals Chapter 14 is titled **"Climatic Design Information"** — covering design weather conditions, heating/cooling degree-days, and clear-sky solar radiation calculations for design days. Its sections are: Climatic Design Conditions; Calculating Clear-Sky Solar Radiation; Transposition to Receiving Surfaces; Generating Design-Day Data; Estimation of Degree-Days; Representativeness of Data; Other Sources. There is no "§14.6 Solar Radiation" section.
- **Verdict**: **Incorrect**. The citation is fabricated. Chapter 14 of the 2021 ASHRAE HoF is "Climatic Design Information," not "Solar Radiation," and it has no section 14.6 about period averages for hourly data. The underlying physical fact (EPW hourly solar values are period averages, not instantaneous measurements) is correct, but the cited source does not support it.

**Citation 2**: "EnergyPlus Engineering Reference §2.5.5 'Solar Radiation' — energy-conservative upsampling approach"
- **Source found**: EnergyPlus Engineering Reference, Climate Calculations chapter, "Weather File Solar Interpolation" section — fetched from [bigladdersoftware.com/epx/docs/9-5/engineering-reference/climate-calculations.html](https://bigladdersoftware.com/epx/docs/9-5/engineering-reference/climate-calculations.html) and confirmed across versions 8.2–25.1.
- **Quoted passage**: *"The solar values on the weather file are average values over the hour. For interpolation of hourly weather data (i.e., when the specified timestep is greater than 1), the average value is assumed to be the value at the midpoint of the hour. The reported values in the output are totals for each reporting period. So, hourly reported values will not match the original values in the weather file, but the total solar for a day should agree. A reference for this model is Ellis, Liesen and Pedersen (2003)."* The EnergyPlus I/O Reference (v9.6) also confirms the interpolation formula: `ValueTimeStep = LastHourValue × WeightLastHour + ThisHourValue × WeightThisHour` where `WeightThisHour = CurrentTimeStepNumber / NumberOfTimeStepsInHour`.
- **Verdict**: **Partially correct / mislabelled**. The section exists and confirms that EPW solar values are hourly averages and that EnergyPlus places them at the midpoint. However: (a) there is no "§2.5.5" — the document uses descriptive heading titles only; (b) the function name "SetupInterpolationValues" at "approximately lines 8328–8380" cannot be verified in the modern C++ WeatherManager.cc (the code has likely been substantially refactored); (c) EnergyPlus's interpolation method is a simple **linear** blend between the previous and current hour — it also does **not** conserve hourly energy integrals, only daily totals. The ticket's claim that EnergyPlus uses "a weighted blend that satisfies this conservation constraint exactly" is incorrect.

**Citation 3 (implicit)**: OCHRE uses `resample().ffill()` equivalent to `ResampleMethod::Zoh`
- **Source found**: `vendors/OCHRE/ochre/utils/schedule.py:553`; `vendors/OCHRE/ochre/Simulator.py:196`
- **Quoted passage**: `df = df.resample(time_res).ffill()` (line 553); `resample_and_reindex(schedule, self.time_res, ...)` without `interpolate=True` (Simulator.py:196–199).
- **Verdict**: **Confirmed**. OCHRE uses ZOH (forward-fill) for all weather resampling including solar.

### Numerical Errors in the Ticket

The ticket's specific numerical example contains two errors:

**Error 1 — Mean formula**: The ticket states the hourly mean of the triangular interpolant is `values[i-1]/4 + values[i]/2 + values[i+1]/4`. This would give weights (1/4, 1/2, 1/4) = (4/16, 8/16, 4/16). The actual formula (derived from integrating the piecewise-linear tent function over a unit interval at factor=4) yields weights **(3/16, 12/16, 1/16)** — not (4/16, 8/16, 4/16). The ticket's formula is incorrect (it describes the trapezoidal rule at three points, not the triangular tent mean). The correct hourly-mean formula for general factor F is: `prev*(F-1)/(4F) + i*(2F+1)/(4F) + next*(1-1/F)/(4)` — but numerically for F=4: `prev×3/16 + i×12/16 + next×1/16`.

**Error 2 — Numerical example**: The ticket claims that for values `[400, 0, 0]` at 15-minute resolution, the nighttime hour receives `[100, 0, 0, 0]` with a 25 W/m² mean. The actual sub-hourly values for the nighttime hour (hour index 1, prev=400, i=0, next=0) are:
- frac=0.00: `400×0.5 + 0×0.5 = 200`
- frac=0.25: `400×0.25 + 0×0.75 = 100`
- frac=0.50: `0×1.0 + 0×0.0 = 0`
- frac=0.75: `0×0.75 + 0×0.25 = 0`

Mean = (200+100+0+0)/4 = **75 W/m²**, not 25. The regression test confirms this value.

### Legitimacy
- **Verdict**: **Partially Legitimate**

**Rationale**: The core bug is real and confirmed by code inspection: `crates/hares-io/src/weather.rs:554–571` defaults GHI, DNI, DHI to `ResampleMethod::Triangular`, which does not preserve hourly energy integrals. OCHRE uses ZOH for these channels (`vendors/OCHRE/ochre/utils/schedule.py:553`), and the triangular method demonstrably leaks 75 W/m² of irradiance into a zero-irradiance nighttime hour in the [400, 0, 0] test case. The proposed fix (change the default to ZOH) is directionally sound. However, three material inaccuracies reduce the ticket's credibility: (1) the ASHRAE citation (Ch. 14 §14.6 "Solar Radiation") does not exist — Chapter 14 is "Climatic Design Information" with no section 14.6; (2) the EnergyPlus Engineering Reference section number "§2.5.5" does not exist — the actual section is an unnumbered heading "Weather File Solar Interpolation," and EnergyPlus's method also does not conserve hourly energy (only daily totals), making the claim that EnergyPlus uses "a weighted blend that satisfies this conservation constraint exactly" false; (3) the numerical example is wrong by a factor of 3 (25 W/m² stated vs 75 W/m² actual). The `Triangular` docstring comment at `weather.rs:289–292` also incorrectly claims the method achieves "hourly mean preservation" — it does not.

### Proposed Fix Summary
In `crates/hares-io/src/weather.rs`, change the three `.unwrap_or(ResampleMethod::Triangular)` calls (lines 560, 565, 570) to `.unwrap_or(ResampleMethod::Zoh)`. Update the comment at lines 554–556 to document the ZOH choice as the energy-conservative default. Update the `Triangular` variant docstring (lines 289–292) to remove the false claim of "hourly mean preservation" and add a warning that the method does not conserve hourly energy integrals, with the correct boundary error quantification (up to 75 W/m² at a 400→0 sunset transition at 15-min resolution, not 25 W/m² as the ticket states). Update the test `solar_fields_use_triangular_by_default` (weather_parity.rs:680) to reflect the new ZOH default.

### Test Written
- **File**: `crates/hares-io/tests/weather_parity.rs`
- **Tests added**:
  1. `triangular_sunset_boundary_bleeds_into_nighttime_hour` — regression guard confirming that `ResampleMethod::Triangular` (the current default) produces a 75 W/m² mean in the first nighttime hour for a [400, 0, 0] sequence. Verifies exact sub-hourly values [200, 100, 0, 0]. Passes against current code (documents the bug, will need update when the default changes).
  2. `zoh_preserves_hourly_energy_integral_at_sunset_boundary` — verifies that `ResampleMethod::Zoh` applied via `ResampleOverrides` gives exact hourly means for the same [400, 0, 0] sequence (max error < 1e-9 W/m²). Passes against current code.
