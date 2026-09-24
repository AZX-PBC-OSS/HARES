HARES Weather Interpolation Verification Report
Executive Summary
HARES's weather interpolation architecture is well-designed with generally sound choices for per-field interpolation methods. The PCHIP implementation is mathematically correct (Fritsch-Carlson monotonicity-preserving slopes). However, I found 4 bugs, 2 significant design concerns, and 5 test gaps that should be addressed.
---
1. Current Interpolation Methods Per Field
---
2. Solar Radiation Interpolation (GHI, DNI, DHI)
Current (HARES): ZOH (zero-order hold / forward fill)
All solar fields default to ZOH (weather.rs:477-491). This means each sub-timestep gets the hourly value from the EPW row, held constant for all sub-steps within that hour.
EnergyPlus: Special triangular solar interpolation
From the EnergyPlus source code (WeatherManager.cc), solar radiation uses a distinct weighting scheme from temperature:
For even TimeStepsInHour:
  - At the half-hour mark: SolarInterpolation = 1.0 (100% current hour)
  - Before half-hour: linear blend of previous hour and current hour  
  - After half-hour: linear blend of current hour and next hour
The key insight is that EPW reports hour-ending solar values — the value for hour 12 represents the average irradiance over 11:00–12:00. EnergyPlus's triangular weighting ensures:
- Before the half-hour: weight previous hour's solar (still in that hour's period)
- At the half-hour: 100% current hour
- After the half-hour: weight next hour's solar (entering next period)
This produces a smooth ramp at sunrise/sunset rather than a step function.
Assessment
HARES's ZOH is defensible but produces a known artifact: at sunrise, the first sub-timestep with non-zero solar gets the full hourly GHI value instantly (a step), whereas EnergyPlus produces a gradual ramp. For BESTEST cases this matters less (6-timestep-per-hour resolution), but for high-resolution simulations (60-second steps), the step artifact can cause:
- Sudden heat load spikes at the hour boundary
- Quantization of solar gains into hourly blocks
Recommendation: The current ZOH is acceptable for BESTEST parity with OCHRE (which also uses forward-fill). For EnergyPlus parity, implement the triangular solar interpolation scheme. The ResampleOverrides mechanism already supports per-field method changes — adding a SolarTriangular variant to ResampleMethod would be a clean extension.
Risk of PCHIP on solar: PCHIP could overshoot at night→day transitions (interpolating between 0 and 500 W/m² could produce intermediate values that are physically wrong for the solar geometry). ZOH is safer for solar. Linear would also be acceptable. PCHIP should NOT be the default for solar.
---
3. Sky Temperature: Bug — Interpolated Directly Instead of Recomputed
The Bug
File: weather.rs:465-469
Sky temperature is stored as a pre-computed value during EPW parsing and then interpolated via PCHIP during resampling. This is incorrect.
EnergyPlus Approach
From the EnergyPlus source (WeatherManager.cc:3113-3115):
EnergyPlus interpolates the inputs (dry-bulb, dew-point, opaque sky cover, horizontal IR) and then recomputes sky temperature from the interpolated inputs. The comment in the EnergyPlus source explicitly states: "Sky emissivity now takes interpolated timestep inputs rather than interpolated calculation esky results."
Why This Matters
Sky temperature is a non-linear function of its inputs (4th-root of IR/σ, or T_db × ε^0.25). Interpolating sky temperature directly violates the chain rule:
The error is small when IR is well-behaved, but can be significant during transitions (e.g., cloud clearing events where IR drops and T_db rises simultaneously).
Recommendation
Fix: Remove sky_temp_c from the pre-computed column in WeatherTimeSeries. Instead, compute sky temperature at each sub-timestep from the already-interpolated horizontal_infrared_w_m2, dry_bulb_c, dew_point_c, and opaque_sky_cover. This requires:
1. Keep sky_temp_c in the struct for backward compatibility
2. In resample_with(), after interpolating all input fields, recompute sky_temp_c by calling compute_sky_temp_c() on the interpolated values
3. When horizontal_infrared < 50 (the fallback threshold), ensure the fallback models also receive interpolated inputs
Alternatively (lower effort): Apply PCHIP to horizontal IR, then recompute sky_temp from the interpolated IR using the Stefan-Boltzmann inversion. This gives correct sky temp when IR ≥ 50 W/m², which covers the majority of EPW hours.
---
4. Ground Temperature: Interpolation Within Months vs. Monthly Constant
Current (HARES): Linear interpolation between mid-month anchors
File: epw.rs:604-659 — interpolate_ground_temp_c()
HARES computes 12 monthly ground temperatures (either from the EPW header or DOE-2 model), places them at mid-month day-of-year anchors, and linearly interpolates between them for each hourly timestamp. This produces a smooth annual curve.
EnergyPlus: Monthly constant (step function)
From the EnergyPlus source (WeatherManager.cc:2087-2088):
EnergyPlus uses the Site:GroundTemperature:BuildingSurface object, which specifies 12 monthly values. The ground temperature is held constant for each month — no interpolation between months.
Assessment
HARES's approach is physically superior to EnergyPlus's step function. Real ground temperature varies continuously. The DOE-2 model's sinusoidal output is inherently smooth, and linear interpolation between mid-month values is a reasonable approximation.
However, there is a BESTEST parity concern: ASHRAE 140 BESTTEST case 900FF specifies ground temperature. For EnergyPlus, this is typically input as a constant (often 10°C annual) or monthly values via Site:GroundTemperature:BuildingSurface. If HARES uses DOE-2-computed ground temps from the Denver TMY3 dry-bulb data, the February ground temp will differ from the EnergyPlus BESTTEST reference value.
BESTTEST 900FF Denver February Ground Temperature
For Denver TMY3 (lat 39.74°N, annual avg dry-bulb ~10°C):
- DOE-2 model: February ground temp ≈ annual_avg − amplitude × gm × cos(phase) ≈ 10 − 12 × 0.15 × cos(...) ≈ 8–9°C
- EnergyPlus BESTTEST default: typically 10°C constant (from the EPW header, which often uses a fixed 10°C)
- The ASHRAE 140 specification for case 900FF uses monthly ground temperatures that should match what EnergyPlus uses
Recommendation: For BESTTEST parity, ensure the EPW file's GROUND TEMPERATURES header is respected when present. HARES already does this (epw.rs:241), which is correct. The DOE-2 fallback is only used when the EPW header is absent. Document this choice clearly.
TMY3 Ground Temperature
File: tmy3.rs:177-187 — TMY3 always uses the DOE-2 model since TMY3 has no ground temperature header. This is appropriate but should be documented.
---
5. Wind Speed and Direction: Bugs and Divergence
Wind Speed: ZOH vs. EnergyPlus Linear
HARES uses ZOH (weather.rs:493-497), EnergyPlus uses linear interpolation. Wind speed is turbulent and doesn't interpolate smoothly, so ZOH is a reasonable engineering choice. However, the step change at hour boundaries can cause:
- Sudden infiltration rate changes (affecting HVAC load)
- Quantization of convective heat transfer coefficients
Recommendation: ZOH is acceptable; linear is also acceptable. Document the choice.
Wind Direction: Bug — ZOH Instead of Circular Interpolation
File: weather.rs:498-502
wind_dir_deg: resample_field(
    &self.wind_dir_deg,
    factor,
    overrides.wind_dir.unwrap_or(ResampleMethod::Zoh),
),
Wind direction is interpolated using ZOH. EnergyPlus uses circular linear interpolation (shortest-arc interpolation, interpolateWindDirection() at WeatherManager.cc:3183-3197). 
Why this is a bug: When wind direction transitions from 350° to 10°, ZOH produces a sudden 20° jump (the short way) or a 340° jump (the long way) at the hour boundary. EnergyPlus smoothly transitions through 360°→0°. For convection calculations, this matters.
Recommendation: Add a CircularZoh or CircularLinear variant to ResampleMethod, implementing shortest-arc interpolation for wind direction. At minimum, use the existing Linear method (which handles the 350→10 case poorly but is better than ZOH for small transitions).
---
6. PCHIP Overshoot Analysis
Does PCHIP create negative solar values?
No — solar fields use ZOH by default, so PCHIP is not applied to solar. The ResampleOverrides mechanism allows forcing PCHIP on solar, but the default is safe.
Does PCHIP create unrealistic temperature spikes?
Theoretically no — PCHIP is guaranteed to be monotone within each interval. The Fritsch-Carlson algorithm (weather.rs:571-648) correctly implements the monotonicity correction. However:
1. At extreme transitions (e.g., cold front: 25°C → -10°C in one hour), PCHIP can produce slightly steeper ramps than linear interpolation but won't overshoot the data values.
2. Boundary conditions: The code uses non-centered three-point formulas at boundaries (weather.rs:591-616) matching SLATEC pchim.f. This is correct.
3. Year boundary: The code uses flat extrapolation beyond the last knot (weather.rs:704, clamp to 0,1), which means December 31 values are held constant into January. This is a known simplification — no cyclic wrap.
Clamping
- RH is clamped to 0, 100 (weather.rs:429-431) ✅
- Opaque sky cover is clamped to 0, 10 (weather.rs:432-435) ✅
- Dry-bulb is NOT clamped — this is correct because dry-bulb should not be artificially limited
- Dew-point is NOT clamped — this is correct
- Pressure is NOT clamped — this is correct
- Sky temp is NOT clamped — could overshoot but unlikely with PCHIP
- Ground temp is NOT clamped — fine, it's smooth by construction
---
7. TMY3 Midpoint Offset Bug
File: tmy3.rs:240
midpoint_offset_secs: 0,
The TMY3 parser sets midpoint_offset_secs = 0, but TMY3 uses the same hour-ending convention as EPW (hour 12 covers 11:00–12:00). The TMY3 parser's parse_datetime function (tmy3.rs:286-318) explicitly states: "TMY3 uses 01:00–24:00 (end-of-interval convention)".
EPW correctly sets midpoint_offset_secs: 1800 (epw.rs:288). TMY3 should also set it to 1800. Without this, PCHIP knot positions for TMY3 weather don't align with the period midpoints, causing a systematic 30-minute offset in weather indexing for TMY3-derived simulations.
Severity: Bug — HIGH. This causes a systematic time shift in all TMY3-parsed weather data. The compute_annual_offset() function in environment.rs:641-662 uses midpoint_offset_secs to shift the weather index, so TMY3 data is read at the wrong time offset.
---
8. EPW Sky Temperature Computation: Verification
File: epw.rs:483-502
The cascade logic is correct:
1. IR ≥ 50 W/m² → Stefan-Boltzmann inversion (matches EnergyPlus default) ✅
2. IR < 50 + opaque_sky_cover > 0 → Berdahl-Martin + Walton cloud correction ✅
3. IR < 50 + opaque_sky_cover = 0 → Clark-Allen fallback ✅
The individual models are verified:
- berdahl_martin_sky_emissivity() matches EnergyPlus's CalcSkyEmissivity() with BerdahlMartin mode ✅
- walton_cloud_correction() matches EnergyPlus's cloud correction term ✅
- clark_allen_sky_temp_c() matches EnergyPlus's ClarkAllen mode ✅
One minor discrepancy: EnergyPlus uses min(DryBulb, DewPoint) for the Berdahl-Martin dew point input (WeatherManager.cc:3219), while HARES uses dew_point_c directly. Since the EPW parser validates dew_point <= dry_bulb (epw.rs:153-157), this is equivalent in practice.
---
9. DOE-2 Ground Temperature Model: Verification
File: epw.rs:423-467
The DOE-2 implementation is mathematically verified against the reference formula:
- beta = sqrt(π / (hours_per_year × diffusivity)) × depth_factor ✅
- x = exp(-beta) ✅  
- y = (x² - 2x·cos(β) + 1) / (2β²) ✅
- gm = sqrt(y) ✅
- z = (1 - x·(cos(β) + sin(β))) / (1 - x·(cos(β) - sin(β))) ✅
- phase = 0.6 + atan(z) ✅
- T_ground(day) = T_avg - ΔT × gm × cos(2π·day/365 - phase) ✅
The test doe2_ground_temp_matches_ochre_damped_formula (epw.rs:1173-1211) verifies this against a hand-computed reference. ✅
The constants match OCHRE's implementation:
- DOE2_GROUND_DIFFUSIVITY = 0.025 m²/hour ✅
- DOE2_GROUND_DEPTH_FACTOR = 10.0 m ✅
- DOE2_GROUND_PHASE_OFFSET_RAD = 0.6 rad ✅
---
10. Test Adequacy Assessment
Existing Tests
Test Gaps
1. No test for PCHIP overshoot on solar fields with forced PCHIP override: If a user sets overrides.ghi = Some(Pchip), there's no test verifying the result doesn't go negative. Add a test that forces PCHIP on GHI and verifies no negative values.
2. No test for TMY3 midpoint offset: There's no test verifying that TMY3-parsed weather data is indexed at the correct time. The TMY3 parser sets midpoint_offset_secs = 0, but there's no test that would catch this being wrong.
3. No test for sky temperature recomputation from interpolated inputs: The current tests check compute_sky_temp_c() in isolation, but no test verifies that after resampling, sky_temp_c is consistent with the interpolated IR/dry-bulb/dew-point.
4. No test for wind direction interpolation across 360°/0° boundary: If wind direction changes from 350° to 10°, ZOH produces a step. No test covers this case.
5. No test for ground temperature at year boundary: The interpolate_ground_temp_c() function handles December→January wrapping, but there's no unit test for this.
6. No test comparing HARES results against EnergyPlus for a standard case: The BESTEST cases should eventually serve this role, but currently there's no automated comparison.
7. No test for PCHIP on pressure with large altitude-derived transitions: Pressure at high-elevation sites can have significant sub-hourly variation from weather fronts. No test covers PCHIP behavior on pressure with sharp transitions.
8. No test for downsampling correctness on all fields: The resample_downsample_mean test only covers dry-bulb and GHI; it doesn't verify that sky_temp, ground_temp, wind_speed, etc. are correctly averaged.
---
11. Summary of Findings by Severity
🔴 HIGH — Bugs
🟡 MEDIUM — Design Concerns
#	Finding	File:Line	Impact
F4	Solar radiation uses ZOH while EnergyPlus uses triangular interpolation	weather.rs:477-491	Step-function solar at hour boundaries; diverges from EnergyPlus
F5	Ground temperature is interpolated between months while EnergyPlus holds it constant per month	epw.rs:604-659	HARES ground temps are smoother; may differ from EnergyPlus BESTTEST results
F6	PCHIP uses flat extrapolation at year boundary (no cyclic wrap)	weather.rs:704	Dec 31 values held into Jan instead of wrapping; affects multi-year simulations
Field	Default Method	File:Line	EnergyPlus Method	Assessment
Dry-bulb temp	PCHIP	weather.rs:444-448	Linear interpolation (prev_hr × w_prev + cur_hr × w_curr)	✅ PCHIP is superior (monotone, smoother)
Dew-point temp	PCHIP	weather.rs:449-453	Linear interpolation	✅ Appropriate
Rel. humidity	PCHIP + clamp [0,100]	weather.rs:428-431	Linear interpolation	✅ Appropriate with clamping
Pressure	PCHIP	weather.rs:455-459	Linear interpolation	✅ Appropriate
Horizontal IR	PCHIP	weather.rs:460-464	Linear (interpolated, then calcSky recomputes sky temp from interpolated inputs)	⚠️ Design concern (see §3)
Sky temp	PCHIP	weather.rs:465-469	Recomputed from interpolated dry-bulb, dew-point, opaque sky cover	🐛 Bug (see §3)
Ground temp	PCHIP	weather.rs:470-474	Monthly constant (no interpolation within month)	⚠️ Design concern (see §4)
Opaque sky cover	PCHIP + clamp [0,10]	weather.rs:432-435	Linear interpolation	✅ Appropriate with clamping
GHI	ZOH	weather.rs:477-481	Special solar interpolation (triangular weighting, see §2)	⚠️ Divergent (see §2)
DNI	ZOH	weather.rs:482-486	Special solar interpolation	⚠️ Divergent (see §2)
DHI	ZOH	weather.rs:487-491	Special solar interpolation	⚠️ Divergent (see §2)
Wind speed	ZOH	weather.rs:493-497	Linear interpolation	⚠️ Divergent (see §5)
Wind direction	ZOH	weather.rs:498-502	Circular linear interpolation (shortest arc)	🐛 Bug (see §5)
Liquid precip	Distribute (divide by factor)	weather.rs:504	Interpolated then divided by steps/hour	✅ Correct
Surface albedo	ZOH	weather.rs:506-509	Monthly constant	✅ Appropriate
sky_temp_c: resample_field(
    &self.sky_temp_c,
    factor,
    overrides.sky_temp.unwrap_or(ResampleMethod::Pchip),
),
calcSky(state,
        tomorrowTs.HorizIRSky,
        tomorrowTs.SkyTemp,
        tomorrowTs.OpaqueSkyCover,  // INTERPOLATED
        tomorrowTs.OutDryBulbTemp,  // INTERPOLATED
        tomorrowTs.OutDewPointTemp, // INTERPOLATED
        tomorrowTs.OutRelHum * 0.01,
        state.dataWeather->wvarsLastHr.HorizIRSky * wgtPrevHr + 
            wvarsH.HorizIRSky * wgtCurrHr);  // INTERPOLATED IR
T_sky = f(IR, T_db, T_dp, N)  — non-linear function
T_sky(t_interp) ≠ f(IR(t_interp), T_db(t_interp), T_dp(t_interp), N(t_interp))
state.dataEnvrn->GroundTemp[(int)GroundTempType::BuildingSurface] =
    siteBuildingSurfaceGroundTempsPtr->getGroundTempAtTimeInMonths(state, 0, Month);
Test	Location	What it covers
pchip_single_element_replicates	weather.rs:1003	Edge case
pchip_two_elements_linear	weather.rs:1011	2-element fallback
pchip_interpolates_through_knots	weather.rs:1025	Interpolation accuracy
pchip_preserves_monotonicity_in_monotone_run	weather.rs:1040	Monotonicity
pchip_nan_propagation	weather.rs:1061	NaN handling
pchip_factor_one_returns_original	weather.rs:1077	No-op
fritsch_carlson_flat_segment_slopes_zero	weather.rs:1083	Flat segments
fritsch_carlson_linear_data	weather.rs:1092	Linear data
resample_clamps_rel_humidity	weather.rs:1106	RH overshoot clamping
resample_clamps_opaque_sky_cover	weather.rs:1117	Sky cover clamping
resample_solar_fields_still_zoh	weather.rs:1128	Solar is ZOH
resample_pchip_on_dry_bulb_and_zoh_on_wind	weather.rs:875	Method dispatch
resample_distributes_precipitation_across_sub_slots	weather.rs:896	Precip preservation
full_pipeline_synthetic_weather	weather_integration.rs:259	End-to-end sanity
solar_irradiance_physical_bounds	weather_integration.rs:462	Solar bounds
resampled_weather_produces_smooth_environment	weather_integration.rs:537	Smoothness
#	Finding	File:Line	Impact
F1	TMY3 midpoint_offset_secs = 0 should be 1800	tmy3.rs:240	Systematic 30-min time offset for all TMY3 simulations
F2	Sky temperature interpolated directly instead of recomputed from interpolated inputs	weather.rs:465-469	Non-linear distortion of sky temp at sub-hourly resolution
F3	Wind direction uses ZOH instead of circular interpolation	weather.rs:498-502	Incorrect wind direction at hour boundaries (especially 350°→10° transitions)
🟢 LOW — Minor Issues / Enhancements
#	Finding	File:Line	Impact
F7	No clamping on solar fields after resampling (if user forces PCHIP)	weather.rs:477-491	Could produce negative GHI/DNI/DHI if PCHIP is forced on solar
F8	Horizontal IR could be clamped to [0, ∞) after PCHIP resampling	weather.rs:460-464	PCHIP on flat boundary could technically produce slight negative IR
---
12. Recommended Fixes (Priority Order)
Fix 1: TMY3 midpoint offset (F1) — One-line fix
// tmy3.rs:240
// BEFORE:
midpoint_offset_secs: 0,
// AFTER:
midpoint_offset_secs: 1800,
Fix 2: Sky temperature recomputation after resampling (F2)
In resample_with(), after all other fields are interpolated, recompute sky_temp_c from the interpolated inputs:
// After all resample_field calls, add:
let sky_temp_c: Vec<f64> = (0..total)
    .map(|i| compute_sky_temp_c(
        self.horizontal_infrared_w_m2.get(i).copied().unwrap_or(0.0),  // Use interpolated IR
        dry_bulb_c[i],        // Already interpolated
        dew_point_c[i],       // Already interpolated  
        opaque_sky_cover[i],  // Already interpolated and clamped
    ))
    .collect();
This requires making compute_sky_temp_c accessible from weather.rs (currently it's in epw.rs).
Fix 3: Wind direction circular interpolation (F3)
Add a CircularLinear variant to ResampleMethod and implement shortest-arc interpolation:
fn circular_linear_resample(values: &[f64], factor: usize) -> Vec<f64> {
    // Same as linear_resample but handles 350→10 transition correctly
    // by taking the shortest arc around the 360° circle
}
Then change the default for wind_dir from Zoh to CircularLinear.
Fix 4: Solar field clamping (F7) — Defensive
Add clamping after resampling for solar fields, similar to RH and sky cover:
let mut ghi_w_m2 = resample_field(&self.ghi_w_m2, factor, overrides.ghi.unwrap_or(ResampleMethod::Zoh));
for v in &mut ghi_w_m2 { *v = v.max(0.0); }
Fix 5: Add missing tests
Priority tests to add:
1. Test TMY3 midpoint offset produces correct weather indexing
2. Test sky temp consistency after resampling (interpolated IR → recomputed sky temp matches direct calculation)
3. Test wind direction ZOH at 350°→10° boundary
4. Test ground temperature at December→January boundary
5. Test PCHIP on solar fields with forced override (verify no negative values)
6. Test full downsampling for all fields
---
13. Confidence and Assumptions
Item	Confidence	Source
EnergyPlus uses linear interpolation for temp/humidity/pressure/wind	HIGH	Direct source code review (WeatherManager.cc:3099-3111)
EnergyPlus uses triangular solar interpolation	HIGH	Direct source code review (WeatherManager.cc:3063-3112, SetupInterpolationValues)
EnergyPlus recomputes sky temp from interpolated inputs	HIGH	Direct source code + explicit comment in source
EnergyPlus ground temp is monthly constant	HIGH	Source code (WeatherManager.cc:2087) + Context7 documentation
TMY3 midpoint_offset should be 1800	HIGH	TMY3 uses hour-ending convention (documented in parser comments)
DOE-2 model implementation is correct	HIGH	Cross-verified against OCHRE + hand calculation in test
PCHIP Fritsch-Carlson implementation is correct	HIGH	Cross-verified against SLATEC pchim.f, SciPy, MATLAB pchip