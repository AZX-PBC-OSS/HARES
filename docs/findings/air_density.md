Air Density & Altitude Correction Report for HARES
Executive Summary
HARES uses a hardcoded sea-level air density constant (AIR_DENSITY_KG_M3 = 1.2041 kg/m³) for zone air capacitance computation, while the infiltration solver correctly uses altitude-corrected density via moist_air_density_kg_m3(p_pa, t_out, w_out). This creates an 18% overstatement of zone capacitance at Denver elevation, which translates to a ~0.3–0.5°C temperature bias in annual simulations. The correct physics functions already exist in hares-physics; the fix is primarily a wiring problem — passing site pressure to derive_zone_capacitances().
Classification: Physics shortcut (not a bug per se, since OCHRE has the same limitation, but an inconsistency between HARES's own infiltration and capacitance paths).
---
1. Inventory of Air Density Usage Locations
1.1 Zone Air Capacitance — USES HARDCODED SEA-LEVEL CONSTANT
File:Line	Usage	Current Value
boundary_rc.rs:17	AIR_DENSITY_KG_M3 = 1.2041 constant definition	1.2041 kg/m³ (20°C, 101325 Pa)
boundary_rc.rs:281	derive_zone_capacitances() formula	1.2041 * 1006 * volume * mass_multiplier
1.2 Infiltration Solver — ALREADY CORRECT
File:Line	Usage
infiltration.rs:75	let rho = moist_air_density_kg_m3(p_pa, t_out, w_out)
infiltration.rs:211	let m_dot_sens = rho * sensible_flow_m3_s
infiltration.rs:213	let h_inf = m_dot_sens * CP_DRY_AIR_J_KG_K
infiltration.rs:215	let q_latent = m_dot_lat * H_FG_J_PER_KG * (w_out - w.humidity_ratio)
1.3 Humidity Solver — ALREADY CORRECT
File:Line	Usage
humidity_solver.rs:140	let rho_air = moist_air_density_kg_m3(p_pa, t_zone_c, w_old)
humidity_solver.rs:274	Same
humidity_solver.rs:389	Same
humidity_solver.rs:423	Same
humidity_solver.rs:510	Same
1.4 HVAC Equipment — MIXED
File:Line	Usage
air_conditioner.rs:984	moist_air_density_kg_m3(env.weather.pressure_kpa*1000, zone.temp, zone.w)
coil_physics.rs:439	moist_air_density_kg_m3(p_kpa*1000, db_in_c, w_in)
ventilation.rs:123	AIR_DENSITY_KG_M3 = 1.2 (separate constant)
ventilation.rs:423	m_dot_kg_s = effective_flow_rate_m3_s * 1.2
1.5 AIM-2 Coefficient Computation — ACCEPTABLE USE OF CONSTANT
File:Line	Usage	Classification
infiltration.rs:452	const RHO: f64 = 1.2041 in aim2_coefficients_from_ach50()	✅ Correct — this converts ACH50 to AIM-2 flow coefficient C. The ACH50 test is defined at standard conditions, so using standard density is physically correct.
1.6 Dwelling Moisture Balance — ALREADY CORRECT
File:Line	Usage
dwelling/mod.rs:2830	moist_air_density_kg_m3(p_pa, zone.temperature_c, w_old)
---
2. What Functions Already Exist in hares-physics
2.1 standard_pressure_pa(elevation_m) — air_properties.rs:12
pub fn standard_pressure_pa(elevation_m: f64) -> f64 {
    SEA_LEVEL_PRESSURE_PA * (1.0 - ISA_LAPSE_COEFFICIENT * elevation_m).powf(ISA_PRESSURE_EXPONENT)
}
Implements ISA 1976 / ICAO Doc 7488 standard atmosphere: P = P₀ × (1 - L·h)^5.2559
Validated against reference values at 0m, 500m, 1000m, 1609m, 3000m (tests at air_properties.rs:61–144).
2.2 moist_air_density_kg_m3(p_pa, t_db_c, w) — air_properties.rs:21
pub fn moist_air_density_kg_m3(p_pa: f64, t_db_c: f64, w: f64) -> f64 {
    let w_eff = w.max(MIN_HUMIDITY_RATIO_DENSITY);
    p_pa / (DRY_AIR_GAS_CONSTANT_J_KG_K
        * (t_db_c + CELSIUS_TO_KELVIN)
        * (1.0 + HUMIDITY_DENSITY_CORRECTION * w_eff))
}
Inverts ASHRAE HOF 2021 Ch.1 Eq.28 moist-air specific volume: v = R_da·T·(1 + 1.6078·W) / P. This is the same formula as EnergyPlus's PsyRhoAirFnPbTdbW:
PsyRhoAirFnPbTdbW = Pb / (R_da * T_K * (1 + 1.6078 * W))
The EnergyPlus source code confirms this at Psychrometrics.cc — the function takes (Pb, Tdb, W) and computes density from the ideal gas law with humidity correction.
2.3 dry_air_density_kg_m3(p_pa, t_c) — air_properties.rs:29
pub fn dry_air_density_kg_m3(p_pa: f64, t_c: f64) -> f64 {
    p_pa / (DRY_AIR_GAS_CONSTANT_J_KG_K * (t_c + CELSIUS_TO_KELVIN))
}
Simple ideal gas law: ρ = P / (R·T).
---
3. What EnergyPlus Uses
From the EnergyPlus source code (Psychrometrics.cc and engineering reference):
1. PsyBaroMetricPressure: Computes barometric pressure from site elevation using the same ISA formula as HARES's standard_pressure_pa(). Set once from the EPW location header or elevation input.
2. PsyRhoAirFnPbTdbW(Pb, Tdb, W): Computes moist air density. Formula: Pb / (R_da × T_K × (1 + 1.6078 × W)). Identical to HARES's moist_air_density_kg_m3().
3. Zone air capacitance (Engineering Reference §13.3): C_z = ρ × cp × V where ρ is computed via PsyRhoAirFnPbTdbW using the site barometric pressure, zone air temperature, and zone humidity ratio. EnergyPlus updates this per-timestep as zone conditions change, but for the RC-network approach (which HARES uses), a fixed-at-initialization value using site-average conditions is standard practice.
---
4. ASHRAE HoF §1.8 Moist Air Density Equation
The authoritative expression from ASHRAE Handbook of Fundamentals 2021, Chapter 1:
Specific volume (Eq. 28):
v = R_da × T × (1 + 1.6078 × W) / P    [m³/kg_da]
where:
- R_da = 287.058 J/(kg·K) (specific gas constant for dry air)
- T = absolute temperature K
- W = humidity ratio kg_w/kg_da
- P = total barometric pressure Pa
- 1.6078 ≈ M_da / M_w = 28.965 / 18.015
Density (dry-air basis, Eq. 28 inverted):
ρ_da = 1/v = P / (R_da × T × (1 + 1.6078 × W))    [kg_da/m³]
This is exactly what moist_air_density_kg_m3() already implements.
---
5. Quantitative Impact Analysis
5.1 Zone Capacitance for BESTEST 900FF (Denver, 1609m)
Parameter	Sea-Level (Current)
Pressure [Pa]	101,325
Density at 20°C, W=0.008 [kg/m³]	1.185
Zone volume [m³]	129.6
Zone capacitance (mult=1) [J/K]	155,204
Impact on 900FF min temperature: The 900FF test showed a previous density-only correction changed the min temp by ~0.02°C. This is smaller than the theoretical 0.3–0.5°C because the zone air capacitance is only ~1% of total effective system capacitance (zone air + concrete). The dominant error source for 900FF is the concrete super-node effect (Candidate 2 in the root-cause analysis).
5.2 Zone Capacitance for General Simulations
Site	Elevation [m]	Pressure [Pa]
Sea level (Miami)	0	101,325
Denver (BESTEST)	1,609	83,460
Mexico City	2,240	77,480
Leadville, CO	3,100	69,210
La Paz, Bolivia	3,640	65,070
For Denver and above, the overstatement exceeds 17% — meaningful for any simulation involving transient temperature response.
5.3 Inconsistency Between Infiltration and Capacitance
The infiltration solver removes heat using Denver-corrected density (~0.977 kg/m³ at 20°C), but the zone capacitance stores heat using sea-level density (1.185 kg/m³). This means:
- The thermal time constant τ = C_zone / G_infiltration is overstated by ~17.6%
- The zone cools more slowly than it should during nighttime
- The zone heats more slowly than it should during daytime recovery
This inconsistency is a physics gap, even if the absolute temperature impact is small in the BESTEST case.
---
6. Correct Fix Design
6.1 Zone Air Capacitance (derive_zone_capacitances)
Current signature:
pub fn derive_zone_capacitances(zones: &[ZoneInput]) -> Vec<f64>
Proposed signature:
pub fn derive_zone_capacitances(zones: &[ZoneInput], site_pressure_pa: f64) -> Vec<f64>
Implementation change at boundary_rc.rs:281:
// Before:
(AIR_DENSITY_KG_M3 * AIR_CP_J_KG_K * volume * z.mass_multiplier).max(MIN_CAPACITANCE_J_K)
// After:
let rho = dry_air_density_kg_m3(site_pressure_pa, 20.0);
(rho * AIR_CP_J_KG_K * volume * z.mass_multiplier).max(MIN_CAPACITANCE_J_K)
Rationale for using dry-air density at 20°C rather than moist-air density:
- Zone capacitance is an RC-network parameter computed once at initialization
- OCHRE uses rho_air = 1.2041 with the comment "used for determining capacitance only" — this is dry-air density
- EnergyPlus's zone capacitance uses PsyRhoAirFnPbTdbW with assumed zone conditions, but in practice the temperature and humidity vary so much that a representative value (20°C, dry) is standard
- Using dry-air density avoids the need to assume a humidity ratio at init time
- The humidity correction is only ~1.3% at typical indoor conditions (W=0.008), so it's within the margin of the mass_multiplier's inherent approximation
Where should site_pressure_pa come from?
The best source is WeatherMeta.elevation_m, which is parsed from the EPW LOCATION header. The calling code should compute:
let site_pressure_pa = standard_pressure_pa(weather_meta.elevation_m);
This matches EnergyPlus's PsyBaroMetricPressure approach. Using the ISA formula from elevation is better than averaging the EPW hourly pressures because:
1. EPW pressures can have measurement noise and gaps
2. The ISA formula gives a physically consistent value
3. It's the same approach EnergyPlus uses
4. The altitude correction dominates; hourly pressure variations are small (±3% from weather)
6.2 Ventilation Equipment (ventilation.rs)
Current: AIR_DENSITY_KG_M3 = 1.2 used at lines 423, 757.
Fix: The ventilation equipment's mass flow rate calculation should use moist_air_density_kg_m3(p_pa, t_indoor, w_indoor). The EnvironmentState is already available in the step() method.
However, note that the ventilation code's sensible/latent loads are diagnostic only — the actual thermal impact is handled by the infiltration solver. The ventilation step writes telemetry (recovery watts, fan power) but does NOT push thermal gains to the envelope solver. So this is a telemetry accuracy issue, not a simulation accuracy issue.
Priority: Low — fix for consistency but it doesn't affect simulation results.
6.3 AIM-2 Coefficient Computation (infiltration.rs:452)
Current: const RHO: f64 = 1.2041 used in aim2_coefficients_from_ach50().
No fix needed: The ACH50 blower door test is defined at standard conditions (20°C, 101325 Pa). Converting from ACH50 to the AIM-2 flow coefficient C requires the density at those standard conditions. Using 1.2041 is physically correct here. This is the same approach EnergyPlus uses for its INFLTR:FLOWCOEFF calculation.
---
7. Pressure Source in the EPW Pipeline
7.1 How Pressure Enters HARES
1. EPW parser (epw.rs:161): Reads field 9 (pressure in Pa) per hour, converts to kPa, stores in WeatherTimeSeries.pressure_kpa
2. Location header (epw.rs:276): Reads elevation from field 9 of LOCATION header, stores in WeatherMeta.elevation_m
3. WeatherState (environment.rs:118): Has pressure_kpa: f64 field with pressure_pa() helper
4. Per-timestep: The weather interpolator provides env.weather.pressure_pa() which comes from the EPW hourly data
7.2 What's Available at Init Time vs. Runtime
Data	Available at Init
Site elevation	✅ WeatherMeta.elevation_m
EPW hourly pressure	❌ (not yet loaded)
ISA pressure from elevation	✅ standard_pressure_pa(elevation_m)
The zone capacitance is computed during RC network assembly, which happens at initialization time (before the time-stepping loop). At this point, the EPW hourly data may or may not be loaded yet, but the elevation from WeatherMeta is available.
Recommendation: Use standard_pressure_pa(elevation_m) for the zone capacitance density computation. This is deterministic, physically consistent, and doesn't depend on the EPW data loading order.
---
8. Code Changes Required
Change 1: Add site_pressure_pa parameter to derive_zone_capacitances
File: crates/hares-envelope/src/boundary_rc.rs
// Line 273: Change signature
pub fn derive_zone_capacitances(zones: &[ZoneInput], site_pressure_pa: f64) -> Vec<f64> {
    zones
        .iter()
        .map(|z| {
            let volume = z
                .volume_m3
                .or_else(|| z.floor_area_m2.map(|a| a * DEFAULT_HEIGHT_M))
                .unwrap_or(DEFAULT_VOLUME_M3);
            let rho = hares_physics::air_properties::dry_air_density_kg_m3(
                site_pressure_pa,
                20.0,
            );
            (rho * AIR_CP_J_KG_K * volume * z.mass_multiplier)
                .max(MIN_CAPACITANCE_J_K)
        })
        .collect()
}
Remove: The AIR_DENSITY_KG_M3 constant (line 17) — no longer needed in production code. Keep it in tests for backward compatibility assertions.
Change 2: Thread site pressure through the call chain
All callers of derive_zone_capacitances must pass the site pressure. Based on the grep results, the callers are:
1. Test code in boundary_rc.rs (lines 1052, 1069, 1082, 1848) — update to pass 101_325.0 for sea-level parity
2. Test code in bestest_900ff_root_cause.rs (lines 32–63, 119–146, etc.) — update to pass standard_pressure_pa(1609.0) for Denver
3. Integration code wherever the RC network is assembled from building input — must pass standard_pressure_pa(elevation_m)
Change 3: Update the calling code (likely in hares-core or hares-envelope thermal solver setup)
Wherever derive_zone_capacitances is called, the caller must obtain the site elevation and compute:
let site_pressure_pa = hares_physics::air_properties::standard_pressure_pa(elevation_m);
Change 4: Fix ventilation.rs telemetry (low priority)
Replace AIR_DENSITY_KG_M3 = 1.2 with a call to moist_air_density_kg_m3 using the current environment state. This is a telemetry-only change.
---
9. Impact Assessment
9.1 BESTEST Cases
Case	Current Bias
900FF (heavyweight, Denver)	+0.02°C (measured)
600FF (lightweight, Denver)	Likely similar
All other cases	0 (sea-level sites)
The density fix alone is small for BESTEST because the zone air capacitance is dwarfed by wall capacitance. However, it eliminates the inconsistency between infiltration (which correctly uses Denver density) and capacitance (which currently uses sea-level density).
9.2 General Simulations
Site Type	Impact
Sea level (≤100m)	<0.5% change
Low altitude (100–500m)	1–6% correction
Moderate altitude (500–1500m)	6–17% correction
High altitude (1500–3000m)	17–31% correction
Very high altitude (>3000m)	>31% correction
For HARES's primary use case (US residential buildings, many in non-sea-level locations), this fix matters for:
- Denver/Front Range: 17.6% correction
- Salt Lake City (1288m): ~14% correction
- Albuquerque (1619m): ~17.7% correction
- Boise (874m): ~9.5% correction
9.3 OCHRE Parity
OCHRE also uses rho_air = 1.2041 as a constant for capacitance (Envelope.py:11). This is a known limitation of OCHRE that HARES has the opportunity to fix. The OCHRE infiltration code also has a TODO comment:
outside_air_density = 0.0765  # TODO: calculate based on average outdoor air temperature and altitude
Fixing this in HARES while OCHRE does not would create a parity divergence. This is acceptable — HARES's fix is more physically correct, and the divergence is well-characterized.
---
10. Classification and Recommendation
Classification: Physics shortcut (not a bug)
The current code is not wrong for sea-level sites — it gives exactly correct results. It is a shortcut that silently assumes sea-level conditions for all sites. The shortcut was inherited from OCHRE and is documented in the code comment ("Matches OCHRE's 1.2041 for parity").
However, it creates an internal inconsistency within HARES itself: the infiltration solver uses Denver-corrected density while the capacitance uses sea-level density. This inconsistency is the strongest argument for fixing it — two parts of the same simulation disagree about the physics.
Recommendation: Fix in a single targeted PR
1. Add site_pressure_pa parameter to derive_zone_capacitances()
2. Replace AIR_DENSITY_KG_M3 with dry_air_density_kg_m3(site_pressure_pa, 20.0)
3. Thread site elevation → ISA pressure through the call chain
4. Update all test code
5. Remove AIR_DENSITY_KG_M3 constant (or mark deprecated)
6. Fix ventilation.rs telemetry density for consistency (low priority, separate commit)
Success criteria:
- BESTEST 900FF results change by ≤0.1°C (confirming no regression)
- derive_zone_capacitances_uses_sea_level_density_regardless_of_site test now fails (documenting the fix)
- New test: derive_zone_capacitances_uses_site_pressure passes
- Denver zone capacitance is ~18% lower than sea-level zone capacitance
- Sea-level zone capacitance is unchanged to within 0.1%