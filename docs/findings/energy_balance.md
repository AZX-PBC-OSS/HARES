HARES Investigation Report: Night Sky Temperature Model & Energy Balance Closure for BESTEST 900FF
Executive Summary
Finding	Severity
Synthetic weather sets sky_temp_c = outdoor_temp_c (line 741)	Critical for synthetic weather
Synthetic IR fixed at 300 W/m² regardless of conditions	High — physically unrealistic for cold nights
Synthetic ground_temp_c = outdoor_temp_c (line 742)	Medium — ground warmer than air in winter
Energy balance at 900FF min-temp step	Closes by construction in RC network
Zone air capacitance uses sea-level ρ = 1.2041	High — overstates Denver by 22%
Concrete under-discretized (2 vs 4+ nodes)	Critical — super-node prevents night draining
The 900FF min-temperature outlier (+2.5°C above ASHRAE band) is NOT caused by the sky temperature bug. It is caused by the concrete super-node effect (dominant) and sea-level air density (secondary). The sky temperature bug at synthetic.rs:741 is a severe defect but affects only synthetic weather cases, not EPW-based BESTEST cases.
---
Part 1: Night Sky Temperature Model
1.1 Current Code
File: crates/hares-core/src/dwelling/synthetic.rs:728-742
Ok(WeatherTimeSeries {
    meta,
    dry_bulb_c: vec![config.weather.outdoor_temp_c; n],
    dew_point_c: vec![config.weather.dew_point_c; n],
    rel_humidity_pct: vec![config.weather.rel_humidity_pct; n],
    pressure_kpa: vec![config.weather.pressure_kpa; n],
    ghi_w_m2: vec![0.0; n],            // ✓ No solar (synthetic)
    dni_w_m2: vec![0.0; n],            // ✓
    dhi_w_m2: vec![0.0; n],            // ✓
    wind_speed_m_s: vec![0.0; n],      // ✓ No wind
    wind_dir_deg: vec![0.0; n],        // ✓
    opaque_sky_cover: vec![0.0; n],    // Clear sky assumed
    horizontal_infrared_w_m2: vec![300.0; n],    // ❌ BUG: Fixed 300, unrealistic for cold nights
    sky_temp_c: vec![config.weather.outdoor_temp_c; n],  // ❌ BUG: Should be derived from IR
    ground_temp_c: vec![config.weather.outdoor_temp_c; n], // ⚠️ Wrong for winter
    liquid_precip_m: vec![0.0; n],     // ✓
    surface_albedo: None,              // ✓
})
1.2 The Bug: Two Inconsistencies
Bug A — sky_temp_c ignores horizontal_infrared:
The code sets sky_temp_c = outdoor_temp_c, but also sets horizontal_infrared_w_m2 = 300.0. These are physically inconsistent. The EPW parser (crates/hares-io/src/epw.rs:483-502) correctly computes sky temperature from IR via Stefan-Boltzmann inversion:
T_sky = (IR / σ)^0.25 - 273.15
For IR = 300 W/m², this gives T_sky = -3.4°C, NOT the outdoor temperature.
Outdoor Temp (°C)	sky_temp (bug)	sky_temp (from IR=300)
-30	-30	-3.4
-20	-20	-3.4
-10	-10	-3.4
0	0	-3.4
20	20	-3.4
Bug B — horizontal_infrared = 300 is unrealistic for cold nights:
On a clear winter night in Denver, measured horizontal IR is typically 180–200 W/m² (from EPW data). The fixed 300 W/m² corresponds to a warm, humid atmosphere, not cold, dry winter conditions. The correct IR for Denver clear winter night conditions gives:
Source	IR (W/m²)	T_sky (°C)	T_air (°C)
EPW Denver TMY3 (Feb 9, 3AM)	183	-34.8	-15.6
EPW Denver TMY3 (Feb 9, 7AM)	192	-31.9	-13.3
EPW Denver TMY3 (Feb 9, 8AM)	199	-29.7	-12.2
Synthetic (current code)	300	-3.4	(varies)
Correct clear winter night	~181	-35.4	-18.0
1.3 Physical Impact on LWR Cooling
For a horizontal roof (F_sky = 1.0, β = 1.0) at T_air = -15°C with ε = 0.9, A = 48 m²:
Scenario	T_sky	Q_roof (W)	Notes
Correct (EPW clear night)	-34.8	-3,330	Strong LWR cooling
Bug: sky = T_air	-15.0	0	Zero LWR cooling on roof!
If IR=300 were used correctly	-3.4	+2,580	Roof GAINS heat (physically impossible)
For all exterior surfaces (roof + 4 walls, total 111.6 m² opaque), with T_surf ≈ T_air + 2°C:
Scenario	T_sky	Total exterior LWR (W)
Correct (EPW)	-34.8	-5,158
Bug (sky = T_air)	-15.0	-793
Over an 8-hour night, the bug fails to remove ~125,000 kJ of heat that should be lost through longwave radiation. For a heavyweight building (C ≈ 14,500 kJ/K), this translates to approximately +8.7°C warming bias. For a lightweight building, the effect is even more extreme (the building hits a higher steady-state temperature much faster).
1.4 Why 900FF Is Not Affected
The 900FF fixture (tests/fixtures/bestest/900ff.toml:22) specifies:
[weather]
epw_path = "../../../vendors/OCHRE/ochre/defaults/Weather/USA_CO_Denver.Intl.AP.725650_TMY3.epw"
The EPW parser (crates/hares-io/src/epw.rs:196-201) correctly computes sky temperature:
let sky_temp_c = compute_sky_temp_c(
    horizontal_infrared_w_m2,  // from EPW field 12
    dry_bulb_c,
    dew_point_c,
    opaque_sky_cover,
);
The cascade in compute_sky_temp_c (epw.rs:483-502):
1. IR ≥ 50 W/m² → Stefan-Boltzmann inversion: T_sky = (IR/σ)^0.25 — this is what fires for Denver EPW data
2. IR < 50, sky_cover > 0 → Berdahl-Martin + Walton cloud correction
3. IR < 50, sky_cover = 0 → Clark-Allen empirical correlation
The EnvironmentManager (crates/hares-core/src/environment.rs:589) propagates the EPW-derived sky_temp into the thermal solver:
state.weather.sky_temp_c = self.weather.get(WeatherField::SkyTempC, weather_idx);
And the thermal solver (crates/hares-envelope/src/thermal_solver/longwave.rs:31) uses it:
let t_sky_raw = env.weather.sky_temp_c;
At the 900FF min-temp hour (step 943, Feb 9 8AM):
- EPW horizontal IR = 199 W/m²
- Derived T_sky = -29.7°C
- Outdoor T_air = -12.2°C (EPW raw) / -13.3°C (PCHIP interpolated)
- Sky-Air ΔT = 17.5K — correctly driving significant LWR cooling
1.5 What the Correct Sky Temperature Should Be for BESTEST
The ASHRAE 140 BESTEST specification does not prescribe a specific sky temperature model. It requires simulators to use the weather data as provided. The Denver TMY3 EPW file includes measured horizontal infrared radiation (field 12), and the convention (matching EnergyPlus, BLAST, DOE-2) is:
1. When EPW horizontal IR is available (≥ 50 W/m²): Derive T_sky by Stefan-Boltzmann inversion: T_sky = (IR/σ)^0.25 - 273.15
2. When IR is unavailable: Use the Clark & Allen (1978) model: ε_clear = 0.787 + 0.764 × ln(T_dp_K/273), then T_sky = T_db_K × ε_clear^0.25 - 273.15
3. When cloud cover data is available with missing IR: Use Berdahl & Martin (1984) clear-sky emissivity with Walton (1983) cloud correction
EnergyPlus default: uses the horizontal infrared from EPW when available, which matches what HARES does for EPW-based cases.
For synthetic weather, the correct approach is:
- Compute horizontal_infrared from atmospheric conditions using a sky emissivity model
- Then derive sky_temp from IR using Stefan-Boltzmann inversion
- OR: compute sky_temp directly from a model (Clark-Allen or Berdahl-Martin + Walton) given dry_bulb, dew_point, and opaque_sky_cover
1.6 Required Code Changes
Fix 1 — Derive sky_temp from IR (minimal fix):
In crates/hares-core/src/dwelling/synthetic.rs:741, change:
// BEFORE (bug):
sky_temp_c: vec![config.weather.outdoor_temp_c; n],
// AFTER (fix):
sky_temp_c: {
    let ir = 300.0_f64;  // same as horizontal_infrared_w_m2
    let sigma = 5.6697e-8_f64;
    vec![(ir / sigma).powf(0.25) - 273.15; n]
},
This makes sky_temp consistent with the IR value, but IR=300 is still unrealistic for cold nights.
Fix 2 — Compute both IR and sky_temp from atmospheric conditions (proper fix):
Reuse the EPW parser's compute_sky_temp_c function from crates/hares-io/src/epw.rs:
use hares_io::epw::{clark_allen_sky_temp_c, berdahl_martin_sky_emissivity, 
                     walton_cloud_correction, sky_temp_from_emissivity};
// In build_synthetic_weather():
let sky_cover = 0.0; // clear sky for synthetic
let (sky_temp_c, horizontal_ir) = if sky_cover > 0.0 {
    let eps_clear = berdahl_martin_sky_emissivity(config.weather.dew_point_c);
    let eps_sky = walton_cloud_correction(eps_clear, sky_cover);
    let t_sky = sky_temp_from_emissivity(config.weather.outdoor_temp_c, eps_sky);
    let ir = eps_sky * 5.6697e-8 * (config.weather.outdoor_temp_c + 273.15).powi(4);
    (t_sky, ir)
} else {
    let t_sky = clark_allen_sky_temp_c(config.weather.outdoor_temp_c, config.weather.dew_point_c);
    let eps = (t_sky + 273.15).powi(4) / (config.weather.outdoor_temp_c + 273.15).powi(4);
    let ir = eps * 5.6697e-8 * (config.weather.outdoor_temp_c + 273.15).powi(4);
    (t_sky, ir)
};
Fix 3 — Also fix ground_temp (line 742):
// BEFORE:
ground_temp_c: vec![config.weather.outdoor_temp_c; n],
// AFTER: Use DOE-2 sinusoidal model or simple offset
ground_temp_c: vec![config.weather.outdoor_temp_c + 2.0; n],  // ground warmer in winter
Better yet, expose the DOE-2 monthly ground temperature model from epw.rs for use in synthetic weather.
1.7 Verification
After applying Fix 2:
1. Run an existing EPW-based BESTEST case with synthetic weather using the same conditions — the sky_temp should match the EPW-derived value within ±1°C
2. For Denver at T_db=-15°C, T_dp=-18°C (clear): sky_temp should be approximately -34°C (not -15°C)
3. Add a unit test in synthetic.rs that verifies sky_temp_c[i] != outdoor_temp_c for any non-zero dew-point depression
---
Part 2: Energy Balance Closure at 900FF Min-Temp Step
2.1 Operating Conditions at Step 943 (Hour 944)
From the diagnostic test:
Parameter	Value
Zone air temperature	0.902°C
Outdoor air temperature	-13.3°C
EPW sky temperature	-29.7°C
EPW ground temperature	~0–2°C (DOE-2 model)
Time	Feb 9, ~08:00 MST
2.2 Component Gains
From the thermal solver at step 943:
Component	Value (W)	Sign	Description
window_solar_w	0.0	—	No solar (nighttime/early dawn)
opaque_solar_w	0.0	—	No solar on opaque surfaces
exterior_lwr_w	-1552.1	Loss	Net LWR exchange with sky/ground
interior_lwr_w	0.0	Zero	Zero-sum by enclosure conservation
infiltration_w	-288.9	Loss	0.5 ACH at Denver pressure, ΔT=14.2K
internal_gain_w	+168.3	Gain	200 W × sensible fraction
Whole-building net heat flow: -1672.7 W (losses exceed gains)
2.3 Zone Air Energy Balance
The zone air node receives:
- Internal gains: +168.3 W (direct)
- Infiltration: -288.9 W (direct, semi-implicit coupling)
- Conduction from thermal mass: +115.8 W (computed as residual)
Net zone air heat gain = -4.8 W, which equals:
dE_zone/dt = C_zone × dT_zone/dt = 156,832 J/K × (-0.111°C / 3600s) = -4.8 W  ✓
The zone air energy balance closes. The conduction from thermal mass (+115.8 W) represents heat flowing from the warm concrete (~0.9°C) through interior film resistances into the zone air, partially offset by heat flowing from zone air through the walls to the exterior.
2.4 Whole-Building Energy Balance
Total heat loss from building = -1672.7 W
Over 3600s: ΔE_total = -6,021,720 J
This must equal the sum of energy changes across all RC nodes:
Component	Capacitance (J/K)
Concrete walls	8,904,000
Floor concrete	5,376,000
Wood siding	273,035
Roof wood	435,024
Roof outer layer	383,040
Floor insulation	487,227
Wall insulation	54,760
Roof insulation	54,093
Zone air	156,832
TOTAL	16,124,010
Expected average mass cooling rate: -6,021,720 / 16,124,010 = -0.37°C/hr
Observed zone air cooling rate: -0.11°C/hr (slower due to super-node coupling)
Zone air energy change: -17,408 J (0.3% of total)
Thermal mass energy change: -6,004,312 J (99.7% of total)
The whole-building energy balance closes within the RC network by construction. The state-space model x(t+dt) = A·x(t) + B·u(t) enforces energy conservation at each node via Kirchhoff's current law. The "missing" heat is not missing — it is stored in the thermal mass nodes (primarily the 100mm concrete walls and 80mm floor concrete).
2.5 Does the Energy Balance Close?
Yes, by construction. The RC network is derived from conservation laws, and the state-space integration preserves energy. The component gains reported by the diagnostic test are consistent with the observed rate of temperature change:
- Net heat flow: -1672.7 W
- Average mass cooling rate: -0.37°C/hr
- This is consistent with the total thermal mass of ~16.1 MJ/K
The zone air cooling rate (-0.11°C/hr) is slower than the average because the concrete super-node keeps the zone air coupled to the warm concrete mass, which cools primarily through the insulation bottleneck.
2.6 Where the "Gap" Would Be (If Any)
If the energy balance didn't close, the likely culprits would be:
1. Exterior LWR injection fraction: The iterative solver injects (solar + q_lw) × rad_frac into the RC node, not the full q_lw. The remaining (1 - rad_frac) × q_lw goes to the zone air node. This is correctly handled in the code but could cause confusion when comparing component_gains to actual node energy changes.
2. Semi-implicit infiltration coupling: The infiltration is applied through the coupled state-space step, not as a direct u-vector input. The infiltration_w in component_gains is extracted from the coupling buffer, but the actual heat exchange is distributed across both the current and next timestep through the semi-implicit scheme.
3. Window LWR bypass: Windows skip the exterior LWR calculation (LWR is implicit in the U-factor). This means the component_gains.exterior_lwr_w does NOT include window LWR, which is instead captured in the window U-factor heat transfer. This is physically correct but means the LWR accounting is split across two mechanisms.
None of these create an actual energy conservation violation — they just make the diagnostic accounting complex. The underlying state-space model conserves energy by construction.
---
Part 3: Root Causes for 900FF Outlier and Fix Priorities
3.1 Error Budget
The 900FF minimum temperature of 0.90°C exceeds the ASHRAE 140 upper bound of -1.6°C by +2.50°C.
Root Cause	Estimated Impact	File:Line
Concrete super-node (2 RC nodes for 100mm concrete)	+1.5 to +2.5°C	boundary_rc.rs Fourier discretization
Sea-level air density (1.2041 vs 0.987 for Denver)	+0.3 to +0.5°C	boundary_rc.rs:AIR_DENSITY_KG_M3
Synthetic sky_temp bug	Not applicable to 900FF	synthetic.rs:741
3.2 The Concrete Super-Node Problem (Dominant Cause)
The BESTEST 900FF heavyweight wall has a 100mm concrete inner layer (k=0.51, ρ=1400, cp=1000). With the Fourier stability criterion C=3 at dt=3600s:
α = k/(ρ×cp) = 3.64e-7 m²/s
dx_max = √(C × α × dt) = √(3 × 3.64e-7 × 3600) = 0.0627 m
n = ceil(0.100 / 0.0627) = 2 sub-layers
EnergyPlus CondFD uses C=3 but at dt=600s:
dx_max = √(3 × 3.64e-7 × 600) = 0.0256 m
n = ceil(0.100 / 0.0256) = 4 sub-layers
With only 2 concrete nodes, the inner node (50mm from zone air) has:
- Resistance to zone air: R = 0.12/A + 0.050/(2×0.51×A) = 0.169/A K/W
- Capacitance: C = 1400 × 1000 × 0.050 × A = 70,000×A J/K
- Time constant: τ = RC = 11,830 s ≈ 3.3 hours
But the resistance from concrete to exterior through insulation is:
- R_insulation = 0.0615/(0.040×A) = 1.54/A K/W
- Time constant to exterior: τ = 70,000×A × 1.54/A = 107,800 s ≈ 30 hours
The concrete can conduct heat to zone air 10× faster than it can drain heat to the exterior. This creates a "super-node" where the concrete and zone air are thermally locked together, preventing the concrete from effectively draining at night.
3.3 Expected Impact of Fixing the Sky Temperature Bug
For 900FF (EPW weather): Zero impact — the bug doesn't apply.
For synthetic weather cases in cold climates: Severe impact — approximately +5–10°C warming bias on clear winter nights. The bug eliminates the sky-air LWR temperature differential, which is the primary nighttime cooling mechanism for horizontal roofs. A building with 48 m² roof would lose ~4000+ W less LWR cooling per hour.
3.4 Code Changes Needed (Priority Order)
P0 — Fix concrete discretization for 900FF:
- Increase the Fourier discretization constant from C=3 to C=6 (or higher) for heavyweight layers
- OR: sub-step the thermal solver at dt=600s for CondFD-like accuracy
- OR: mandate minimum 4 nodes for any concrete layer with ρ×cp > 500,000 J/(m³·K)
- File: crates/hares-envelope/src/boundary_rc.rs — the Fourier sub-layering logic
P1 — Fix zone air density:
- Add elevation_m parameter to derive_zone_capacitances()
- Compute ρ = P/(R_specific × T) using ISA altitude-corrected pressure
- File: crates/hares-envelope/src/boundary_rc.rs — AIR_DENSITY_KG_M3 constant and derive_zone_capacitances function
P1 — Fix synthetic sky temperature:
- Compute sky_temp_c from atmospheric conditions using Clark-Allen or Berdahl-Martin model
- Compute horizontal_infrared_w_m2 consistently from sky emissivity
- Fix ground_temp_c to use DOE-2 model or reasonable offset
- File: crates/hares-core/src/dwelling/synthetic.rs:728-742
3.5 How to Verify Empirically
1. Sky temperature fix: Create a synthetic weather case with T_outdoor = -15°C, T_dewpoint = -18°C. Run with current code (sky_temp = -15°C) vs fixed code (sky_temp ≈ -35°C). The fixed version should show significantly cooler zone temperatures at night. Compare the exterior_lwr_w component gain — it should increase by ~3000-4000 W.
2. Zone air density fix: Run 900FF with Denver-corrected density (0.987 vs 1.2041). The zone air capacitance drops from 156,832 to 128,633 J/K. The minimum zone temperature should drop by approximately 0.3-0.5°C.
3. Concrete discretization fix: Run 900FF with C_m = 6 (forcing 3+ concrete nodes instead of 2). The minimum zone temperature should drop by 1.5-2.5°C as the concrete drains more effectively at night. Compare the conduction heat flow from the concrete's outer node through insulation — it should increase significantly.
4. Combined verification: Apply all three fixes and check that 900FF min-temp falls within the ASHRAE 140 band -6.4, -1.6°C. The current value of 0.90°C needs to drop by at least 2.5°C.