# Environment state management: weather, ground, water mains
**Review ID**: core-18
**Category**: core
**Date**: 2026-05-25

## Files Reviewed
- `crates/hares-core/src/environment.rs`
- `crates/hares-physics/src/ground.rs`
- `crates/hares-physics/src/water_mains.rs`

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc`

## Findings

### Finding 1: [Severity: medium]
**Description**: Water mains temperature model lacks the 0 °C (32 °F) minimum clamp present in the EnergyPlus reference implementation. In very cold climates the model can return physically impossible sub-freezing mains water temperatures.
**Code Location**: `crates/hares-physics/src/water_mains.rs:111`
**Root Cause**: The Burch-Christensen (2007) model was calibrated for contiguous US climates (annual average −5 °C to 30 °C). EnergyPlus adds a hard floor at 32 °F (0 °C) at `WeatherManager.cc:7206-7208`:
```cpp
if (CurrentWaterMainsTemp < 32.0) {
    CurrentWaterMainsTemp = 32.0;
}
```
HARES omits this clamp. The ratio clamping at line 98 (`clamp(0.0, 1.0)`) prevents phase inversion but does not prevent the final result from going below freezing. At T_avg = −20 °C (−4 °F) with ratio clamped to 0, the function returns −4 °F + 6 °F = 2 °F ≈ −16.7 °C — impermissible for liquid water. The arctic climate test at `water_mains.rs:201-212` explicitly accepts −36.67 °C as a valid output, confirming this is not merely a theoretical case.
**Impact**: For climates near or below the calibrated range, the cold-water supply temperature fed to water heater models will be unrealistically low, producing inflated water heating energy. A user simulating a northern-tier US climate near the Canadian border (e.g. Fargo, ND, T_avg ≈ 5 °C) could see winter mains temperatures below 0 °C under extreme weather, violating the physical constraint that liquid water in buried mains stays at or above freezing.

### Finding 2: [Severity: low]
**Description**: Soil thermal diffusivity default (0.05 m²/day) differs from EnergyPlus's CalcSoilSurfTemp default (0.0208 m²/day), causing faster amplitude decay and shallower phase shift in the Kusuda-Achenbach model.
**Code Location**: `crates/hares-physics/src/ground.rs:31`
**Root Cause**: The code explicitly documents this choice (lines 26-30) as reflecting "wetter conditions typical of foundation-adjacent soil." EnergyPlus uses a lower value for generic dry soil. The factor-of-2.4 difference means HARES ground temperatures at a given depth will be more attenuated and slightly phase-shifted compared to an EnergyPlus run with default soil parameters.
**Impact**: Users comparing HARES to EnergyPlus for foundation heat loss may see discrepancies. The higher diffusivity means HARES ground temperatures at 1-2 m depth will have smaller seasonal swings. This is a defensible physical assumption (moist soil near foundations) but differs from the vendor reference default.

### Finding 3: [Severity: low]
**Description**: Ground temperature model (Kusuda-Achenbach) and water mains model use the same annual mean and amplitude parameters derived from the weather file, but the water mains model's `dt_annual_range_c` input is the full peak-to-peak monthly range while the ground model's `t_amplitude_c` is half of that value. The relationship is correct but the naming and documentation risk confusion.
**Code Location**: `crates/hares-core/src/environment.rs:284-288`
**Root Cause**: At lines 284-288:
```rust
mains_t_annual_avg_c,
mains_dt_annual_range_c,
// ...
ground_t_mean_c: mains_t_annual_avg_c,
ground_t_amplitude_c: mains_dt_annual_range_c / 2.0,
```
`mains_dt_annual_range_c` is the full peak-to-peak difference (documented at `water_mains.rs:59-63`), while the Kusuda-Achenbach model expects half-amplitude (`t_amplitude_c`). The halving at line 287 is correct. However, the water mains function also internally uses `dt_annual_range_c / 2.0` at line 109, meaning the amplitude is effectively divided twice if a user directly supplies the HALF-range (OCHRE's `dt_monthly` convention) and sets `range/2` here. This parameter is computed internally from weather data via `compute_mains_inputs()`, so direct external misuse is unlikely — but the naming tension between `dt_annual_range_c` (HARES: full range) and OCHRE's `dt_monthly` (half-swing) is a latent footgun for integrators.
**Impact**: Low for current usage since parameters are auto-computed. Risk exists for future external API exposure or Python bindings where users might supply OCHRE-convention half-swing values.

### Finding 4: [Severity: informational]
**Description**: HARES pre-resamples all weather data to the simulation time resolution before the simulation loop begins, while EnergyPlus performs linear interpolation at each sub-hourly time step during simulation. For solar radiation, HARES defaults to zero-order hold (ZOH) — explicitly avoiding linear interpolation — which preserves hourly energy integrals but produces step-function sub-hourly irradiance.
**Code Location**: `crates/hares-io/src/weather.rs:659-673` (ZOH default for solar); `crates/hares-io/src/weather.rs:334-373` (Triangular method documentation)
**Root Cause**: The review concern that "solar radiation should not be linearly interpolated" is addressed: HARES defaults to ZOH for GHI/DNI/DHI during upsampling (lines 659-673). The code documents why at lines 646-658: EPW solar values are hourly period averages, not instantaneous midpoints. ZOH preserves the hourly energy integral exactly. EnergyPlus uses a custom `SolarInterpolation` weight array (`WeatherManager.cc:8311-8369`) that creates a triangular profile peaking at the hour midpoint. Both approaches are valid: EnergyPlus's triangular interpolation produces smoother sub-hourly profiles but does not strictly conserve the hourly energy integral; HARES's ZOH conserves energy at the cost of introducing step discontinuities.
**Impact**: None for correctness. Users comparing sub-hourly Perez tilted irradiance between HARES and EnergyPlus may observe differences due to the step-function vs triangular GHI profiles. The `Triangular` method is available via `ResampleOverrides` for users who prefer the EnergyPlus approach and accept documented energy conservation deviations.

### Finding 5: [Severity: informational]
**Description**: The Kusuda-Achenbach phase day default (35 = early February) for the Northern Hemisphere matches EnergyPlus. The Western Hemisphere phase is computed as a 182.5-day offset (365/2), which is correct for a sinusoidal model but uses a symmetric approximation rather than a climate-specific value.
**Code Location**: `crates/hares-physics/src/ground.rs:37-41`
**Root Cause**: Lines 37-41 define constants. The 35-day phase for NH is EnergyPlus's default. The SH offset of 182.5 days is the arithmetic half-year (365/2 days), which is an even 6-month offset but does not account for the asymmetry between NH and SH seasonal timing (e.g., differences in land mass distribution, oceanic influence). EnergyPlus does not appear to have a default Southern Hemisphere phase — it relies on user-supplied monthly ground temperatures for SH sites.
**Impact**: Low. The symmetric offset is a reasonable engineering approximation for SH sites. Users with site-specific soil temperature data should calibrate the phase day value directly rather than relying on the default.

## Summary
- Total findings: 5
- Critical: 0
- High: 0
- Medium: 1
- Low: 3
- Informational: 1

## Recommendations
1. **Add a 0 °C (32 °F) minimum clamp to `water_mains_temperature_c`** matching EnergyPlus `WeatherManager.cc:7206-7208`. The clamp should be at the end, after the Fahrenheit computation but before converting back to Celsius. This prevents physically impossible below-freezing mains water in cold climates and aligns with the EnergyPlus reference implementation.
2. **Consider adding an upper bound check** on `WaterMainsTemp` output to detect obvious model misuse (e.g., T_avg > 40 °C outside the calibrated range). A warning-level diagnostic would help users identify out-of-range inputs without silently producing poor results.
3. **Add a cautionary note to `EnvironmentManager` field documentation** clarifying that `ground_t_mean_c` and `mains_t_annual_avg_c` are the same parameter (annual mean dry-bulb), but the ground model expects half-amplitude while the water mains model expects full peak-to-peak range internally. The current `dt_annual_range_c / 2.0` at `environment.rs:287` is correct but not obviously connected to `water_mains_temperature_c`'s own internal division.
4. **Document the soil diffusivity divergence from EnergyPlus** in a user-facing configuration guide, with guidance on how to override it via `SourceTemperature::KusudaAchenbach { soil_diffusivity_m2_per_day }` when EnergyPlus parity is required.

## References / Citations
- EnergyPlus Engineering Reference: Ground Heat Transfer, "Undisturbed Ground Temperature Model: Kusuda-Achenbach" (Ch. 3.17)
- EnergyPlus `WeatherManager.cc:7121-7216` — `CalcWaterMainsTemp` / `WaterMainsTempFromCorrelation`
- EnergyPlus `WeatherManager.cc:8311-8369` — `SetupInterpolationValues` (sub-hourly interpolation weights)
- EnergyPlus `WeatherManager.cc:2030-2266` — `SetCurrentWeather` (runtime state update)
- Kusuda, T. and Achenbach, P.R. (1965), "Earth Temperatures and Thermal Diffusivity at Selected Stations in the United States", ASHRAE Transactions, Vol. 71(1), pp. 61-74.
- Burch, J. and Christensen, C. (2007). "Towards Development of an Algorithm for Mains Water Temperature." Proceedings of the 2007 ASES National Solar Conference.
- Hendron, R. et al. (2004). "Development of an Energy Savings Benchmark for All Residential End-Uses," SimBuild 2004.
- Fritsch, F.N. and Carlson, R.E. (1980). "Monotone Piecewise Cubic Interpolation," SIAM Journal on Numerical Analysis, 17(2), pp. 238-246. doi:10.1137/0717021
