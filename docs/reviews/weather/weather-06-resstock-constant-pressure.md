# ResStock CSV uses constant ISA atmospheric pressure for all timesteps
**Review ID**: weather-06
**Category**: weather
**Date**: 2026-05-26

## Files Reviewed
`crates/hares-io/src/resstock_csv.rs`

## Vendor/Reference Files Consulted
`vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc`
`vendors/OCHRE/ochre/utils/schedule.py`
`vendors/OCHRE/ochre/Models/Envelope.py`
`vendors/OCHRE/ochre/utils/psychrolib_jit.py`
`crates/hares-physics/src/constants.rs`
`crates/hares-physics/src/air_properties.rs`
`crates/hares-physics/src/psychrometrics.rs`
`crates/hares-envelope/src/thermal_solver/infiltration.rs`
`crates/hares-equipment/src/hvac/air_conditioner.rs`

## Findings

### Finding 1: [Severity: medium] All 8760 timesteps share a single ISA-derived atmospheric pressure
**Description**: The ResStock CSV parser computes pressure once from elevation via the ISA standard atmosphere model and writes the same value to every timestep. Real atmospheric pressure varies diurnally by ±2–3 kPa and can shift ±5 kPa with weather system passage. No per-hour pressure variation is achievable from the 8-column ResStock CSV format, but the downstream consequence is a systematic ±3% proportional error in every timestep's mass-flow-dependent calculations.

**Code Location**:
- `crates/hares-io/src/resstock_csv.rs:122-123` — constant `pressure_kpa` computed once: `let pressure_kpa = isa_pressure_kpa(elevation_m);`
- `crates/hares-io/src/resstock_csv.rs:298` — vector pre-allocated for N copies: `let mut pressure_kpa_vec = Vec::with_capacity(n);`
- `crates/hares-io/src/resstock_csv.rs:324` — same value pushed N times: `pressure_kpa_vec.push(pressure_kpa);`
- `crates/hares-io/src/resstock_csv.rs:49-55` — ISA formula: `101.325 * (1 - L*h)^E` with a 1.0 kPa safety floor

**Root Cause**: The 8-column ResStock CSV (`date_time`, dry bulb, RH, wind speed, wind direction, GHI, DNI, DHI) contains no measured pressure column. HARES correctly compensates for elevation via ISA but does not attempt any per-hour re-estimation (e.g. from dew-point suppression or synoptic-scale pressure data). The module-level docs at lines 25–27 acknowledge this: "ResStock CSV pressure is a constant ISA estimate ... per-row pressure variation is not achievable."

**Impact**: Three coupled downstream effects:

1. **Infiltration mass flow and thermal load (±3% per timestep relative to a measured-pressure baseline)**
   - In `crates/hares-envelope/src/thermal_solver/infiltration.rs:88-93`, outdoor air density is computed at each timestep: `let rho = moist_air_density_kg_m3(p_pa, t_out, w_out)`. This density multiplies volumetric infiltration flow to produce mass flow: `m_dot = rho * Q`. Air density is directly proportional to pressure via the ideal gas law: `ρ = P / (R·T·(1 + W/ε))`.
   - At the tested site (Denver, 1609 m, ISA pressure ~83.4 kPa), a ±3 kPa diurnal variation changes density by ±3.6%. The mass-flow-driven infiltration load `q_sens = m_dot * cp * (T_out - T_zone)` shifts proportionally.
   - The AIM-2 volumetric coefficients (`c_s`, `c_w`) embed a fixed `RHO = 1.2041 kg/m³` (standard sea-level density) as a setup-time constant in `crates/hares-physics/src/infiltration.rs:481`. No per-timestep recalibration occurs, so volumetric flow is pressure-invariant; **all per-timestep pressure sensitivity flows through the density conversion at runtime**.

2. **Psychrometric humidity ratio and derived properties (±3% per timestep)**
   - Humidity ratio from dew point: `w = ε * p_ws / (p - p_ws)` in `crates/hares-physics/src/psychrometrics.rs:72-73`. The denominator `(p - p_ws)` varies inversely with total pressure. At 20°C (p_ws ≈ 2338 Pa), a ±3 kPa pressure shift (e.g. 98→101 kPa) moves `(p - p_ws)` from 96.66 kPa to 100.16 kPa, a 3.6% change, producing a −3.5% change in `w` for fixed dew point / relative humidity.
   - Wet-bulb temperature, which depends on humidity ratio through `humidity_ratio_from_twb` at `psychrometrics.rs:83-99`, will shift secondarily.

3. **HVAC fan curve power draw (±3% proportional error)**
   - Fan power scales with mass flow rate, which scales with air density. At `crates/hares-equipment/src/hvac/air_conditioner.rs:1087` and `:1244`, fan power calculations use `env.weather.pressure_kpa * 1000.0` to convert to Pa. A ±3% pressure error propagates to ±3% fan power error on any given timestep.
   - Because all 8760 hours use the same value, the error is systematic (no hour-to-hour variation error), but the bias relative to true per-hour pressure affects every fan energy bin equally.

### Finding 2: [Severity: low] EnergyPlus uses per-hour EPW pressure; OCHRE uses a worse constant
**Description**: Both vendor reference implementations handle the ResStock 8-column format differently, and HARES's approach is the best of the three given the format constraint.

**EnergyPlus (WeatherManager.cc)**:
- Line 2949: Hourly barometric pressure `AtmPress` is read from the EPW weather file and assigned to `tomorrow.OutBaroPress`. The EPW file format includes `atmospheric_pressure` (Pa) as column 9 on each data line.
- Line 3096: For sub-hourly timesteps, pressure is linearly interpolated: `tomorrowTs.OutBaroPress = wvarsLastHr.OutBaroPress * wgtPrevHr + wvarsH.OutBaroPress * wgtCurrHr`.
- Line 1751: Only when the weather file data is missing does EnergyPlus fall back to `StdBaroPress` (ISA from elevation, identical formula at line 4467).
- Line 4467: `StdBaroPress = StdPressureSeaLevel * std::pow(1.0 - 2.25577e-05 * Elevation, 5.2559)` — same ISA model HARES uses, but with a slightly different (lower-precision) exponent.
- EnergyPlus **requires EPW files** for weather simulations and does not parse 8-column ResStock CSVs. It therefore always has access to per-hour measured pressure.

**OCHRE (schedule.py, Envelope.py)**:
- `vendors/OCHRE/ochre/utils/schedule.py:80`: The schedule expects an `"Ambient Pressure (kPa)"` column.
- `vendors/OCHRE/ochre/Models/Envelope.py:840-841`: When the column is absent, OCHRE issues a warning and falls back to **101.3 kPa** (sea-level) regardless of elevation: `self.warn("Ambient pressure not in schedule. Using standard pressure of 1 atm (101.3 kPa).")`.
- OCHRE's constant is **worse than HARES** for any site above sea level. At Denver (1609 m), OCHRE would use 101.3 kPa while the true ISA pressure is 83.4 kPa — an 21% overestimate of density and all density-dependent quantities.
- Like HARES, OCHRE cannot extract per-hour pressure from the 8-column ResStock format.

### Finding 3: [Severity: low] Infiltration coefficient setup embeds constant density, decoupling one error path
**Description**: The AIM-2 coefficient derivation in `crates/hares-physics/src/infiltration.rs:480-576` uses a fixed `RHO = 1.2041` (sea-level density at 20°C) and `T_IN_K = 296.15` for stack and wind coefficient calculations. This means the volumetric flow output of `ashrae_wind_stack()` is calibrated to sea-level air density assumptions. The only per-timestep pressure sensitivity path is the `moist_air_density_kg_m3()` call at `crates/hares-envelope/src/thermal_solver/infiltration.rs:93` that converts volumetric to mass flow.

**Code Location**:
- `crates/hares-physics/src/infiltration.rs:481`: `const RHO: f64 = 1.2041;`
- `crates/hares-physics/src/infiltration.rs:528`: `Cs = f_s * (RHO * G * params.infiltration_height_m / T_IN_K).powf(n_i)`
- `crates/hares-physics/src/infiltration.rs:548`: `Cw = f_w * (RHO / 2.0).powf(n_i)`

**Impact**: The constant coefficient setup breaks what would otherwise be a compounding error — if coefficients were recalibrated per timestep with varying density, the pressure exponent `n_i` would produce a super-linear response. As implemented, the single-path density conversion at runtime keeps the error linear, which is the less severe outcome. However, using sea-level density for coefficient calibration at high-elevation sites introduces an additional systematic offset that is not addressed by the per-timestep `moist_air_density_kg_m3` call at line 93 (which correctly uses elevation-adjusted pressure) — the two density references are inconsistent.

## Summary
- **Total findings**: 3
- **Medium**: 1 — All timesteps use the same ISA pressure value
- **Low**: 2 — Vendor comparison; coefficient consistency

## Recommendations
1. **Short-term (no format change)**: Consider computing ISA pressure once and documenting the limitation clearly in the output metadata. HARES already does this at lines 25–27 of `resstock_csv.rs`. The `isa_pressure_kpa` function at line 49 is correctly implemented and matches the EnergyPlus formula at `WeatherManager.cc:4467` to within 0.04 Pa. No code change needed.
2. **Medium-term**: If ResStock adopts a 9-column CSV adding atmospheric pressure (as NSRDB PSM3 format already does), update the parser to read column 9 when present and fall back to ISA when absent. The EPW parser at `crates/hares-io/src/epw.rs:174` already demonstrates the pattern: `let pressure_kpa = parse_f64(fields[IDX_PRESSURE_PA], row, "pressure_pa")? / 1000.0`.
3. **Long-term**: For sites where only the 8-column CSV is available, consider a synoptic pressure model that adjusts the ISA baseline using dry-bulb/dew-point data (e.g. the hydrostatic or barometric tendency approach). This is speculative and may not justify the added complexity for typical residential simulation accuracy targets.
4. **Infiltration coefficient consistency**: The AIM-2 coefficient setup at `crates/hares-physics/src/infiltration.rs:481` uses `RHO = 1.2041` (sea-level) regardless of site elevation. At Denver (83.4 kPa), actual outdoor density at 20°C is ~0.988 kg/m³, making the sea-level `RHO` ~22% too high. This inconsistency with the per-timestep `moist_air_density_kg_m3` call warrants a separate review (see recommendation 3).

## References / Citations
- ISO 2533:1975 §5 — ISA standard atmosphere tropospheric pressure model
- U.S. Standard Atmosphere 1976 (NOAA-S/T 76-1562) §1.2.5 — ISA pressure exponent derivation
- ASHRAE Handbook of Fundamentals 2021, Chapter 1 — psychrometric relationships (Eq. 28 for moist air specific volume)
- Walker & Wilson (1998) "Field Validation of Algebraic Equations for Stack and Wind Driven Air Infiltration Calculations," *HVAC&R Research* 4(2) — AIM-2 model and coefficient constants
- EnergyPlus Engineering Reference §15.4 — AIM-2 / Sherman-Grimsrud infiltration and weather data interpolation
- ASHRAE Standard 119 / ASTM E779 — power-law relationship for blower-door flow conversion
- NREL ResStock AMY 2018 documentation — 8-column CSV format specification (no pressure column)
- OCHRE `Envelope.py:841` — constant 101.3 kPa fallback when ambient pressure absent from schedule
- `crates/hares-io/src/epw.rs:174` — EPW pressure field parsing: `parse_f64(fields[IDX_PRESSURE_PA]) / 1000.0`
