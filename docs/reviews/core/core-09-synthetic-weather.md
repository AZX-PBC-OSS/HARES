# Synthetic weather generation statistical fidelity
**Review ID**: core-09
**Category**: core
**Date**: 2026-05-26

## Files Reviewed
crates/hares-io/src/weather.rs crates/hares-io/src/tmy3.rs crates/hares-io/src/psm3.rs crates/hares-io/src/epw.rs

## Vendor/Reference Files Consulted
vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc

## Key Source of Synthetic Weather Generation
crates/hares-core/src/dwelling/synthetic.rs (function `build_synthetic_weather`, lines 795-890)
crates/hares-physics/src/solar.rs (function `clear_sky_irradiance`, lines 194-220)

## Findings
### Finding 1: [Severity: critical]
**Description**: Synthetic weather generates zero solar radiation (GHI/DNI/DHI = 0.0) for all 8760 hours. Solar radiation is the primary driver of envelope heat gains, cooling loads, and PV generation. With zero irradiance, any simulation using the synthetic weather path will see no solar-driven cooling load, no window solar gains, and no PV output — making the results physically meaningless for any purpose beyond conduction-only steady-state tests.

**Code Location**: `crates/hares-core/src/dwelling/synthetic.rs:878-880`
```rust
ghi_w_m2: vec![0.0; n],
dni_w_m2: vec![0.0; n],
dhi_w_m2: vec![0.0; n],
```

**Root Cause**: The synthetic weather generator is explicitly designed for BESTEST-style constant-condition analytical test cases, not for representative weather simulation. The `build_synthetic_weather` doc comment and module-level doc note this is "Synthetic (BESTEST-style) dwelling construction." However, the `SyntheticWeatherConfig` struct exposes only constant overrides (`outdoor_temp_c`, `dew_point_c`, `rel_humidity_pct`, `pressure_kpa`, `ground_temp_c`) with no mechanism to inject time-varying irradiance.

**Impact**: Any simulation using synthetic weather (i.e., TOML-based configurations without an `epw_path`) produces no solar gains at all. The clear-sky solar model in `hares-physics/src/solar.rs:194` (`clear_sky_irradiance`) exists and is fully functional — accepting latitude, day-of-year, and solar altitude — but is never invoked from the synthetic weather path. The model correctly accounts for extraterrestrial irradiance via `extraterrestrial_irradiance(day_of_year)`, airmass via `relative_airmass(zenith_deg)`, and atmospheric turbidity via ASHRAE 2013 optical depths (`BEAM_OPTICAL_DEPTH = 0.556`, `DIFFUSE_OPTICAL_DEPTH = 2.0`), but none of this infrastructure is wired into `build_synthetic_weather`.

**Reference comparison**: EnergyPlus does not have a built-in synthetic weather generator; it requires either weather files (EPW) or explicit design-day input with dry-bulb range, humidity type, and solar model specification (`WeatherManager.cc:2142-2175`). TMY3 methodology (Wilcox & Marion 2008) selects representative months from long-term records to preserve statistical distributions of all variables including solar.

---

### Finding 2: [Severity: high]
**Description**: No diurnal temperature cycle is generated. Temperature is constant for all 8760 hours at `outdoor_temp_c` (default 10 °C). Real weather has strong diurnal variation driven by solar radiation, with amplitude typically 5–15 °C depending on climate and season. The constant profile eliminates the thermal mass cycling and peak-hour cooling demand that diurnal temperature swings create.

**Code Location**: `crates/hares-core/src/dwelling/synthetic.rs:874`
```rust
dry_bulb_c: vec![outdoor_temp_c; n],
```

**Root Cause**: As with Finding 1, the synthetic weather is intentionally constant. There is no sinusoid, no hour-of-day dependence, and no month-of-year dependence. Even a basic model (sinusoidal diurnal profile with a seasonal amplitude envelope driven by sine of day-of-year, as used for ground temperature in the DOE-2 model at `epw.rs:482-538`) would add physical realism at minimal complexity.

**Impact**: Thermal mass effects, peak cooling/heating sizing, and all transient phenomena are eliminated. The `build_synthetic_weather` function is used by every TOML-based simulation at `crates/hares-core/src/dwelling/mod.rs:877`; this means all synthetic BESTEST cases currently run at constant temperature.

---

### Finding 3: [Severity: high]
**Description**: Cross-correlations between meteorological variables are absent. Temperature, humidity, solar radiation, and wind speed are generated independently at constant values. In real weather:
- Temperature and solar radiation have strong positive diurnal correlation (peak temperature lags peak solar by ~2–3 hours)
- Humidity (specifically dew point) is anti-correlated with temperature drying during the daytime; relative humidity is strongly anti-correlated with temperature
- Wind speed often has a diurnal pattern (higher during daytime convection)
- Cloud cover modulates all three

Independent constant generation produces no physically unrealistic combinations (because there is no diurnal variation to mismatch), but also produces no physically realistic variability whatsoever.

**Code Location**: `crates/hares-core/src/dwelling/synthetic.rs:871-889` — all weather fields are independent constant vectors.

**Reference comparison**: EnergyPlus handles this implicitly because it always operates from either weather files (where all variables are measured simultaneously, preserving cross-correlations) or design-day specifications where the user provides consistent parameters (`WeatherManager.cc:2142-2175`). TMY3 preserves cross-correlations by selecting complete hourly records from real measurements.

---

### Finding 4: [Severity: high]
**Description**: No cloud cover or stochastic sky conditions. `opaque_sky_cover` is set to zero for all hours. Cloud cover is the primary driver of PV variability — the difference between a clear-sky and cloudy sky can reduce irradiance by 60–90%. Without stochastic cloud cover, PV output is either zero (when solar is zero, as currently) or unrealistically smooth (if a clear-sky model were added).

**Code Location**: `crates/hares-core/src/dwelling/synthetic.rs:883`
```rust
opaque_sky_cover: vec![0.0; n],
```

**Root Cause**: No cloud model is implemented. The Walton cloud correction (`epw.rs:662`) exists for correcting clear-sky emissivity to all-sky emissivity when cloud cover data is available, but there is no mechanism to generate synthetic cloud cover.

**Impact**: Even if solar irradiance were added (see Finding 1), without cloud cover the synthetic weather would produce unrealistically smooth PV output. Real PV variability requires modeling the stochastic nature of cloud passage and, ideally, inter-day persistence (cloudy days tend to cluster in synoptic-scale weather patterns lasting 3–7 days).

---

### Finding 5: [Severity: medium]
**Description**: Synthetic weather sets `midpoint_offset_secs = 0` (hour-beginning convention) while EPW and TMY3 parsers correctly set it to 1800 (hour-ending convention). This is the correct choice for constant profiles where the midpoint is irrelevant, but it creates an inconsistency if a future diurnal model is added. The hour-ending convention is the EPW/TMY3/IWEC standard per the EPW Data Dictionary v9.6 and Wilcox & Marion 2008 §3.

**Code Location**: `crates/hares-core/src/dwelling/synthetic.rs:814`
```rust
midpoint_offset_secs: 0,
```

Compare: `epw.rs:330` and `tmy3.rs:248` both correctly set `midpoint_offset_secs: 1800`.

**Impact**: Low for current constant profiles (offset is irrelevant to constant data). Would cause a 30-minute phase shift in the diurnal temperature cycle if a sinusoidal model were added without also fixing the offset.

---

### Finding 6: [Severity: medium]
**Description**: No inter-day persistence or synoptic-scale variability. Real weather exhibits multi-day memory: warm days cluster together, cold snaps persist, cloudy periods last multiple days. The constant synthetic weather has no dynamics at any timescale.

**Code Location**: `crates/hares-core/src/dwelling/synthetic.rs:871-889` — all fields are static vectors.

**Reference comparison**: TMY3 preserves inter-day persistence by selecting contiguous months of data from real weather records. EnergyPlus has no built-in stochastic weather generator — it relies entirely on weather files which inherently preserve all statistical structure including persistence.

---

### Finding 7: [Severity: low]
**Description**: Recent improvements (labeled "B3 fix" in test comments) correctly compute horizontal infrared and sky temperature from physical models (Berdahl-Martin clear-sky emissivity + Stefan-Boltzmann inversion) rather than using hardcoded values. This is a significant improvement in thermodynamic consistency. Tests at `synthetic.rs:1028-1146` verify that sky temperature is below outdoor temperature (clear sky is 10–30 °C colder) and that IR is physically consistent with emissivity.

**Code Location**: `crates/hares-core/src/dwelling/synthetic.rs:820-857`

**Impact**: Positive. The previous code set `sky_temp_c = outdoor_temp_c` (eliminating all longwave radiative cooling to the sky) and hardcoded `horizontal_infrared_w_m2 = 300` (a plausible value for temperate conditions but wrong for cold clear skies). The current approach produces physically correct results for any input temperature/dew-point combination. This improvement is documented in tests `synthetic_sky_temp_is_below_outdoor_temp`, `synthetic_horizontal_ir_matches_clear_sky_emissivity`, and `synthetic_sky_temp_and_ir_are_physically_consistent`.

---

### Finding 8: [Severity: low]
**Description**: No synthetic weather documentation clarifies the scope and limitations of the feature. The term "synthetic weather" could mislead users into expecting a weather generator producing statistically representative data, when in fact `build_synthetic_weather` produces constant-condition steady-state data intended only for BESTEST-style analytical validation.

**Code Location**: `crates/hares-core/src/dwelling/synthetic.rs:1-2` — module doc reads "Synthetic (BESTEST-style) dwelling construction from TOML config" but does not explicitly state that the weather is constant-value.

---

## Summary
- Total findings: 8
- Critical: 1 (zero solar radiation)
- High: 3 (no diurnal cycle, no cross-correlations, no cloud cover)
- Medium: 2 (midpoint offset inconsistency, no inter-day persistence)
- Low: 2 (positive: thermodynamic consistency improvements, missing documentation about scope limitations)

## Recommendations
1. **Integrate the existing clear-sky solar model** (`hares-physics/src/solar.rs:clear_sky_irradiance`) into `build_synthetic_weather`. This would require accepting latitude and day-of-year, computing `solar_position` for each hour, and producing GHI/DNI/DHI from `clear_sky_irradiance`. The code already depends on `hares-physics` and all necessary functions are implemented and tested.

2. **Add a configurable diurnal temperature profile** driven by solar radiation. A simple sinusoidal model with configurable amplitude, pegged to solar noon with a 2–3 hour thermal lag, would capture the primary cross-correlation between temperature and solar.

3. **Add optional stochastic cloud cover** with configurable persistence, modeled as a Markov chain or autoregressive process. This could range from simple (configurable cloud fraction, independent draws) to sophisticated (two-state Markov model with seasonal transition probabilities). NREL's NSRDB uses a clear-sky index approach — this could be adopted as a lightweight option.

4. **Add configurable wind speed** — at minimum a constant non-zero value (currently 0.0 m/s, which eliminates all convective heat transfer from wind-dependent \(h_c\) coefficients).

5. **Fix `midpoint_offset_secs` to 1800** when a diurnal model is added, to match the EPW/TMY3 hour-ending convention.

6. **Add module-level documentation** clearly stating the scope and limitations of synthetic weather: that it is for BESTEST-style analytical validation only, and that proper building energy simulation requires EPW/TMY3/PSM3 weather files.

## References / Citations
- Wilcox, S. and Marion, W. (2008), "Users Manual for TMY3 Data Sets", NREL/TP-581-43156
- Clark, G. and Allen, C. (1978), "The Estimation of Atmospheric Radiation for Clear and Cloudy Skies", Proc. 2nd National Passive Solar Conference (AS/ISES), pp. 675-678
- Martin & Berdahl (1984), "Characteristics of Infrared Sky Radiation in the United States," Solar Energy 33(3/4):321-336
- Li, M., Jiang, Y. & Coimbra, C.F.M. (2017), "On the determination of atmospheric longwave irradiance under all-sky conditions," Solar Energy 144:40-48
- ASHRAE Handbook of Fundamentals 2013 Ch.33 Table 9.8 — clear-sky optical depths
- Fritsch, F.N. and Carlson, R.E. (1980), "Monotone Piecewise Cubic Interpolation", SIAM J. Numer. Anal. 17(2), pp. 238-246
- EnergyPlus Engineering Reference — Sky Radiation Modeling, Solar Interpolation, Weather File conventions
- EPW Data Dictionary v9.6 — DATA PERIODS, GROUND TEMPERATURES, Design Conditions fields
- NREL NSRDB PSM3 documentation: <https://developer.nrel.gov/docs/solar/nsrdb/psm3-download/>
