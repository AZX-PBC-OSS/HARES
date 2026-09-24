Investigation Report: HARES Initial Zone Temperatures & Interior Convection Coefficient
Part 1: Initial Zone Temperature Model
1.1 Current Code: What Initial Temperature Does Each Zone Type Get?
File: crates/hares-core/src/environment.rs:753-938
The initial_zones() function (line 753) assigns initial temperatures per zone type:
Zone Type	Initial Temperature
Conditioned	determine_initial_indoor_temp_c() result
Foundation	ground_temp_c
Attic, Garage, Other	outdoor_temp_c
The determine_initial_indoor_temp_c() function (line 836) uses this logic:
1. If HVAC setpoints exist and outdoor > 12°C → cooling setpoint (with random deadband noise)
2. If HVAC setpoints exist and outdoor ≤ 12°C → heating setpoint (with random deadband noise)
3. If only one setpoint exists → use that setpoint
4. If no setpoints exist → DEFAULT_SETPOINT_C = 21.0°C (line 751, 930-935)
1.2 BESTEST 900FF Initial Temperature
For BESTEST 900FF (free-float, no HVAC):
- equipment_name = "None", heating_capacity = 0 → no setpoints
- Falls through to DEFAULT_SETPOINT_C = 21.0°C
- On January 1 in Denver, outdoor temperature is typically -5 to +5°C
- The 900FF zone starts at 21°C, which is ~16-26°C warmer than the true free-float equilibrium
1.3 RC Network Initial State Vector x₀
File: crates/hares-envelope/src/thermal_solver/initialization.rs:27-151
The initialize_steady_state() function:
1. Pins the conditioned zone to indoor_temp_c (21°C for 900FF)
2. Sets input vector u with outdoor and ground temperatures (no HVAC, no solar)
3. Removes the pinned zone state from the A matrix
4. Solves the reduced system for steady-state: -A_c_reduced · x_reduced = B_c · u + A_c[:,j] · T_fixed
5. Re-inserts the pinned zone temperature
Result: All RC nodes get a steady-state gradient consistent with indoor=21°C and the current outdoor/ground temperatures. This is NOT a flat profile — the inner concrete nodes are near 21°C and the outer nodes near outdoor temp, with the gradient determined by the insulation distribution.
However, for 900FF the floor concrete is special:
- Floor insulation is 1.007m of R-3.5/inch material (R ≈ 25.2 m²·K/W)
- Floor concrete is only 80mm (R = 0.071 m²·K/W)
- The steady-state gradient puts the concrete at ~20.9°C (nearly 21°C), because almost all the temperature drop is across the insulation
- The floor concrete carries an enormous amount of excess heat relative to the true free-float state
1.4 Warmup/Spinup Period
File: crates/hares-core/src/dwelling/mod.rs:1912-1925
HARES has an optional warmup mechanism via initialization_duration:
- Runs n timesteps forward, then clears simulation_results.steps
- Resets the clock to the simulation start time
BUT: The BESTEST 900FF fixture (tests/fixtures/bestest/900ff.toml) does NOT set initialization_duration. The DwellingConfig.initialization_duration defaults to None → no warmup period is applied.
What OCHRE does: OCHRE recommends initialization_time = 1 day (see vendors/OCHRE/ochre/cli.py:39 and docs/source/InputsAndArguments.rst:166). Its default CLI sets initialization_time=1 day.
What EnergyPlus does: EnergyPlus uses an iterative warmup procedure — it runs the first day repeatedly until zone temperatures converge (maximum temperature change < threshold between iterations). For heavyweight buildings, this can take many iterations. This produces self-consistent initial conditions where the zone temperature on day 1 is determined by the actual heat balance, not by an arbitrary setpoint.
1.5 Impact on BESTEST 900FF Annual Minimum Temperature
Time constant analysis:
Component	Capacitance [J/K]	Dominant R [K/W]
Zone air (Denver, 129.6 m³)	~130,000	0.003 (total UA300 W/K)
Wall concrete (63.6 m², 100mm)	~8,904,000	~0.025 (insulation/A)
Floor concrete (48 m², 80mm)	~5,376,000	~0.524 (insulation/A)
Roof (no concrete)	~small	~0.01
The floor concrete has a 33-day time constant due to the thick floor insulation (1.007 m). This means:
- At day 15-20 (typical minimum temperature period): exp(-15/33) = 0.64 → 64% of the initial excess heat remains in the floor concrete
- The warm floor concrete radiates and convects heat to the zone air continuously
- This provides a persistent warm bias that decays only slowly
Estimated impact on minimum temperature: The floor concrete at ~20.9°C (initial) vs. the true free-float equilibrium of perhaps 0-5°C represents an excess heat reservoir of:
- ΔQ = C × ΔT ≈ 5,376,000 × (20.9 - 5) ≈ 85 MJ
- At day 15-20, ~36% has decayed → ~54 MJ of excess heat remains
- This heat slowly leaks to the zone air through R_film_int over the floor area
- At ΔT ≈ 10K between floor surface and air: Q̇ = 10 / 0.115 × 48 ≈ 4,174 W (if floor surface were 10K warmer than air)
- In practice, the floor surface has also cooled, so ΔT might be 3-5K → Q̇ ≈ 1,250-2,100 W
This is 6-10× larger than the internal gains (200 W) and would keep the zone significantly warmer than the true free-float temperature. Estimated contribution to the 2.5°C error: 0.5-1.0°C.
1.6 First 24-48 Hours of Simulation
The steady-state initialization produces a proper thermal gradient through walls (not a flat profile), so the first 24-48 hours don't have the extreme transient oscillation that a flat-profile initialization would cause. However:
- The zone air cools rapidly from 21°C (small time constant, ~0.4 hours for air alone)
- The wall concrete cools more slowly (2.5-day time constant) but is in the right direction
- The floor concrete barely changes in 48 hours (33-day time constant)
- The zone air temperature in the first 2-3 days is dominated by the concrete super-node (both walls and floor), which keeps it warmer than the true free-float temperature
---
Part 2: Interior Convection Coefficient Model
2.1 What Value Does HARES Use for Interior Film Resistance?
File: crates/hares-physics/src/film_coefficients.rs:137-189
The film_resistances() function computes:
1. Typical zone temperatures from typical_zone_temps() (line 76-95):
   - Ground = avg_ground_c
   - Conditioned = 20°C
   - Outdoor = avg_ambient_c + 5
   - Foundation, Garage, Attic linearly interpolated
2. ΔT for TARP (line 158-160):
      const MIN_DELTA_T_TARP_NATURAL_C: f64 = 12.9;
   let delta_t = (t_ext - t_int).abs().max(MIN_DELTA_T_TARP_NATURAL_C);
      Critical: The actual zone temperature difference is floored at 12.9°C, preventing h from going to zero when zones are at similar temperatures.
3. TARP natural convection tarp_h_natural() (line 106-118):
   - Vertical (tilt=90°): h = 1.31 × ΔT^(1/3)
   - Enhanced (warm above): h = 9.482 × ΔT^(1/3) / (7.238 - |cos(tilt)|)
   - Reduced (warm below): h = 1.810 × ΔT^(1/3) / (1.382 + |cos(tilt)|)
4. Linearized radiation (line 168-173):
      h_rad = 4.0 × 0.9 × σ × 293.15³ = 5.14 W/(m²·K)
   
5. Interior film resistance (line 174-176):
      r_int = 1.0 / (h_conv + h_rad)
   
Computed values for BESTEST 900FF in Denver (avg_ambient ≈ 10.4°C, avg_ground ≈ 10°C, wind ≈ 2 m/s):
Surface	tilt°	h_conv	h_rad	h_combined
Walls (N/S/E/W)	90	3.08	5.14	8.22
Roof	0	1.78	5.14	6.92
Floor	180	3.57	5.14	8.71
These values are computed ONCE at init time and frozen for the entire simulation.
2.2 Film Coefficients Are NOT Per-Timestep
File: crates/hares-core/src/dwelling/conversions.rs:83-91
The building_to_boundary_inputs() function calls film_resistances() once during initialization and stores the results in BoundaryInput.r_film_interior_m2_k_w. These values are then embedded into the RC network resistance matrix at construction time and never updated.
This is in contrast to EnergyPlus, which evaluates TARP at every timestep using the actual surface-to-air ΔT. The BESTEST 900FF IDF specifies Inside Surface Convection = TARP, meaning EnergyPlus uses the per-timestep TARP evaluation.
2.3 OCHRE Comparison: Same Approach, Different R Definition
File: vendors/OCHRE/ochre/utils/envelope.py:342-402
OCHRE uses the identical approach:
- delta_t = max(12.9, abs(t_ext_zone - t_int_zone)) (line 374)
- Same TARP formulas (lines 378-388)
- Returns Interior Film Resistance = 1/h_natural (line 401) — convection ONLY
- Radiation is added separately via add_radiation_resistances() at T=20°C (line 1048-1061)
Key difference: OCHRE's Interior Film Resistance = 1/h_natural (convection only, ~0.325 m²·K/W for vertical surfaces). HARES's r_int = 1/(h_conv + h_rad) (convection + radiation combined, ~0.122 m²·K/W). OCHRE adds radiation as a separate parallel resistor in the RC network; HARES bakes it into R_int.
Both approaches yield equivalent total resistance when the radiation resistors are combined. The net effect is the same: R_total = 1/(h_conv + h_rad). However, HARES's lumped approach means the interior LWR module (which computes surface-to-surface radiation separately) operates on top of an R_int that already includes the surface-to-air radiation pathway. This is architecturally different from OCHRE where radiation is fully within the RC network and there is no separate LWR module.
2.4 TARP at Actual BESTEST 900FF Conditions
At the annual minimum temperature hour (~hour 700-800, a cold January night):
Assumptions: Zone air ≈ -2°C (correct free-float value), concrete inner surface ≈ 0°C (1-2K above air due to thermal mass lag)
ΔT [K]	TARP h_conv [W/m²K]	h_rad at ~271K [W/m²K]	h_combined
0.5	1.04	4.07	5.11
1.0	1.31	4.07	5.38
2.0	1.65	4.07	5.72
5.0	2.24	4.07	6.31
12.9 (frozen)	3.08	5.14	8.22
The frozen R_int (0.122) is 30-52% lower than what TARP would give at the actual small ΔT conditions during the minimum temperature hour.
2.5 Direction and Magnitude of Error
If h is too high (R too low): surfaces exchange heat too fast with zone air.
For BESTEST 900FF:
- The concrete inner surface and zone air are too tightly coupled through the underestimated R_int
- The concrete mass acts as a "super-capacitor" buffering the air temperature
- During nighttime cooling, the concrete releases heat to the air too quickly, slowing the air temperature drop
- Result: minimum zone temperature is too high → matches the observed 900FF warm bias (+2.50°C above ASHRAE band)
Quantifying the super-node coupling:
- For a wall (inner concrete half to zone air): R_total = R_film_int + R_inner_half_concrete = 0.122 + 0.049 = 0.171 m²·K/W (frozen)
- With correct TARP at ΔT=2K: R_total = 0.175 + 0.049 = 0.224 m²·K/W
- The frozen value gives 24% lower resistance → 24% faster heat exchange between concrete and air
- This directly exacerbates the super-node effect documented in bestest_900ff_root_cause.rs
Estimated contribution to 900FF error: The existing root-cause analysis estimates the concrete super-node contributes ~1.5-2.5°C of the 2.5°C error. The frozen film coefficient makes this super-node 24% stronger, contributing an additional 0.3-0.5°C on top of the discretization effect.
2.6 OCHRE Has the Same Frozen-Coeficient Limitation
OCHRE also evaluates TARP once at init time with the 12.9°C floor. Both HARES and OCHRE share this limitation relative to EnergyPlus. However, OCHRE's Interior Film R = 1/h_natural (convection only, ~0.325 for vertical) means its convective coupling is inherently weaker than HARES's lumped R_int = 1/(h_conv + h_rad) ≈ 0.122. OCHRE compensates by adding radiation resistors separately. The total resistance is equivalent, but the architecture differs in how LWR exchange is handled.
---
Part 3: Classification and Recommendations
3.1 Classification
Issue	Classification
Initial temperature 21°C for free-float zones	Shortcut (reasonable for conditioned buildings, wrong for free-float)
No warmup period for BESTEST	Shortcut (optional feature exists but not used for BESTEST)
Floor concrete 33-day time constant contaminates min-temp result	Consequence of above two issues
Interior film coefficient frozen at init-time TARP value	Shortcut (matches OCHRE but deviates from EnergyPlus/BESTEST spec)
MIN_DELTA_T_TARP_NATURAL_C = 12.9°C floor atypical for conditioned-to-outdoor boundaries	Acceptable (matches OCHRE and EnergyPlus source)
3.2 Required Code Changes
Change 1: Improve initial temperature for free-float zones
File: crates/hares-core/src/environment.rs:836-938
Current: DEFAULT_SETPOINT_C = 21.0°C when no setpoints exist.
Fix: For free-float buildings (no HVAC), use the outdoor temperature as the initial zone temperature rather than 21°C. This is closer to the true free-float equilibrium.
// In determine_initial_indoor_temp_c, when both setpoints are None:
(None, None) => {
    // For free-float (no HVAC), start near outdoor temperature
    // rather than 21°C, which creates a multi-week transient in
    // heavyweight buildings.
    let base_temp = if building.has_hvac() {
        DEFAULT_SETPOINT_C  // 21°C for conditioned buildings
    } else {
        outdoor_temp_c      // free-float: start at outdoor
    };
    // ... deadband noise ...
}
Change 2: Add warmup period for BESTEST cases
File: tests/fixtures/bestest/900ff.toml (and other BESTEST fixtures)
Add initialization_duration_s = 1209600 (14 days) to allow the floor concrete to approach equilibrium. This is particularly important for heavyweight cases (900, 900FF).
Alternatively, implement EnergyPlus-style iterative warmup:
- Run day 1 repeatedly until zone temperature converges
- This is more robust than a fixed-duration warmup
- Would require new code in crates/hares-core/src/dwelling/mod.rs
Change 3: Make interior film coefficients per-timestep (or at minimum, correct the operating point)
File: crates/hares-physics/src/film_coefficients.rs
Option A (Full fix — per-timestep TARP): Recompute film resistances every timestep using actual surface and air temperatures. This would require:
1. Moving film_resistances() evaluation from conversions.rs init-time to per-timestep in the thermal solver
2. Updating the RC network resistance matrix per-timestep (expensive)
3. Or, using a different approach: keep the RC network fixed but apply a correction factor to the heat injection at the surface node
Option B (Pragmatic fix — correct the operating point): Use a more realistic ΔT for the TARP evaluation at init time. The current 12.9°C floor is designed for the worst case (ΔT→0), but for conditioned-to-outdoor boundaries, the typical ΔT is 5-15°C. A better approach:
- For interior boundaries (conditioned-to-attic, conditioned-to-garage), keep the 12.9°C floor
- For exterior boundaries (conditioned-to-outdoor), use a lower floor (e.g., 5°C) since the actual ΔT is typically 5-15°C
- This would give h_conv ≈ 2.24 W/m²K for vertical walls → R_int ≈ 0.134 → a 10% increase in resistance
Option C (BESTEST-specific fix): For BESTEST validation, the IDF specifies TARP for inside convection. Implement per-timestep TARP evaluation for the interior convection component only, keeping the radiation component linearized. This means:
- Keep h_rad in R_int (linearized at 20°C)
- But adjust h_conv per-timestep based on actual surface-to-air ΔT
- This can be done by splitting R_int into R_conv and R_rad components, and only updating R_conv
Change 4: Separate convection and radiation in interior film resistance
File: crates/hares-physics/src/film_coefficients.rs
Following OCHRE's architecture, return convection-only R_int and handle radiation separately:
- film_resistances() returns (r_conv, r_rad, r_ext) instead of (r_int, r_ext)
- The RC network uses r_conv only for the conduction path
- Radiation is handled by the interior LWR module (already exists) and/or separate radiation resistors
This change would:
1. Align HARES with OCHRE's architecture
2. Make it easier to implement per-timestep TARP (only r_conv needs updating)
3. Eliminate any potential double-counting between R_int's h_rad and the LWR module
3.3 Expected Impact on BESTEST 900FF
Change	Expected Impact on Min-Temp Error
Free-float initial temp = outdoor	-0.5 to -1.0°C
14-day warmup period	-0.5 to -1.0°C
Per-timestep TARP interior h_conv	-0.3 to -0.5°C
Corrected TARP operating point (Option B)	-0.1 to -0.3°C
Separated conv/rad (Option C)	Enables per-timestep TARP
Combined (all changes)	-1.3 to -2.5°C
The observed error is +2.50°C. The existing root-cause analysis (air density + under-discretization) accounts for ~2.0-3.0°C. Adding the initial temperature and frozen film coefficient contributions would bring the total estimated bias to ~2.5-3.5°C, which is consistent with the observed 2.5°C error but suggests the air density and discretization effects may be slightly overestimated in the current root-cause analysis.
Priority recommendation: Implement Changes 1 and 2 (free-float initial temp + warmup) first, as they are low-effort and address the most clear-cut issues. Then measure the impact before pursuing the more complex film coefficient changes.
3.4 Impact on General Simulations
- Initial temperature at 21°C: Only affects free-float/unused zones. Conditioned zones correctly start near setpoints. Low risk for typical HPXML buildings.
- No warmup period: Affects all simulations for the first 1-5 days. For annual energy calculations, the impact is small (first week out of 52). For short simulations (e.g., design day), the impact can be significant. Medium risk for short-duration runs.
- Frozen interior film coefficients: Affects all buildings, but the impact is largest for heavyweight buildings with small surface-to-air ΔT. For typical lightweight residential buildings, the surfaces track the air closely and ΔT is small → the frozen h_conv overestimates the coupling → slight warm bias in winter, slight cool bias in summer. Medium risk for annual energy, high risk for peak temperature metrics in heavyweight buildings.