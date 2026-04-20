HARES Window Nighttime Longwave Radiation & Sky Temperature Investigation Report
Executive Summary
Two distinct issues affect nighttime longwave radiation (LWR) losses in HARES, both of which make zones too warm on clear winter nights:
1. Sky temperature bug in synthetic weather (synthetic.rs:741) — sky_temp = outdoor_dry_bulb instead of a proper sky temperature model. This eliminates ALL LWR cooling to the cold sky for any simulation using synthetic (non-EPW) weather.
2. Window exterior LWR completely skipped (longwave.rs:42) — windows do not participate in exterior LWR exchange at all. The comment says "LWR is implicit in U-factor," but this is only true at standard test conditions (T_sky ≈ T_air). When T_sky << T_air, the rated U-factor underestimates the true window heat loss by ~40%. For BESTEST 900FF, this produces ~248 W of missing cooling through 12 m² of south-facing glass.
The BESTEST 900FF minimum temperature bias of +2.50°C is likely dominated by the window LWR skip, not the previously identified concrete under-discretization. The combined effects may over-explain the bias, suggesting the concrete under-discretization impact was previously overestimated.
---
1. Current Sky Temperature Calculation
1.1 EPW path (correct)
File: crates/hares-io/src/epw.rs:483-502
The EPW parser uses a 3-tier cascade:
fn compute_sky_temp_c(horizontal_infrared_w_m2, dry_bulb_c, dew_point_c, opaque_sky_cover) -> f64 {
    if horizontal_infrared_w_m2 >= 50.0 {
        // Method 1: Stefan-Boltzmann inversion (direct IR measurement)
        (horizontal_infrared_w_m2 / STEFAN_BOLTZMANN).powf(0.25) - 273.15
    } else if opaque_sky_cover > 0.0 {
        // Method 2: Berdahl-Martin clear-sky emissivity + Walton cloud correction
        let eps_sky = walton_cloud_correction(berdahl_martin_sky_emissivity(dew_point_c), opaque_sky_cover);
        sky_temp_from_emissivity(dry_bulb_c, eps_sky)
    } else {
        // Method 3: Clark-Allen (1978) fallback
        clark_allen_sky_temp_c(dry_bulb_c, dew_point_c)
    }
}
This is the correct EnergyPlus approach. For BESTEST's Denver EPW file, horizontal infrared radiation is available, so Method 1 (Stefan-Boltzmann inversion) is used, giving an accurate sky temperature.
1.2 Synthetic weather path (BUG)
File: crates/hares-core/src/dwelling/synthetic.rs:741
sky_temp_c: vec![config.weather.outdoor_temp_c; n],
This sets sky_temp = outdoor_dry_bulb for all timesteps. This is physically wrong: the effective sky temperature on a clear night is always much colder than outdoor air temperature due to the atmosphere's selective transparency in the longwave spectrum.
Also note the inconsistent companion fields at lines 739-740:
opaque_sky_cover: vec![0.0; n],       // implies CLEAR sky (coldest T_sky)
horizontal_infrared_w_m2: vec![300.0; n], // 300 W/m² is an OVERCAST sky value
Clear skies (opaque_sky_cover=0) should have much lower horizontal infrared (~180 W/m² for Denver winter).
---
2. The Synthetic Weather Sky Temperature Bug
2.1 What it does
Sets sky_temp_c = outdoor_temp_c, collapsing the 4-component exterior LWR model so that the sky term uses T_air instead of T_sky:
Q_lw = ε·σ·A·[ F_gnd·(T_air⁴ − T_surf⁴) + β·F_sky·(T_air⁴ − T_surf⁴) + (1−β)·F_sky·(T_air⁴ − T_surf⁴) ]
     = ε·σ·A·[F_gnd + F_sky]·(T_air⁴ − T_surf⁴)
     = ε·σ·A·(T_air⁴ − T_surf⁴)
The entire sky hemisphere is treated as radiating at air temperature. All differential cooling from the cold sky is eliminated.
2.2 What it should do
Use the Clark-Allen formula (the most appropriate model when only dry-bulb and dew-point are available):
// At synthetic.rs:741, replace:
sky_temp_c: vec![config.weather.outdoor_temp_c; n],
// With:
sky_temp_c: vec![clark_allen_sky_temp_c(config.weather.outdoor_temp_c, config.weather.dew_point_c); n],
This would require importing clark_allen_sky_temp_c from hares_io::epw or adding a standalone implementation.
2.3 Quantitative impact
For a typical Denver clear winter night:
- T_air = −18°C (255.15 K)
- T_dp = −25°C (248.15 K)
- Clark-Allen: ε_clear = 0.787 + 0.764 × ln(248.15/273.15) = 0.7137
- T_sky = 255.15 × 0.7137^0.25 = 234.3 K = −38.9°C
- Current code: T_sky = −18°C
Sky-air ΔT = 20.9 K
The LWR cooling deficit for all exterior surfaces of a typical 900FF-like building:
Surface	Area (m²)	Tilt	F_sky	β	Missing ΔQ (W)
Roof	48	0°	1.0	1.0	−3000
Walls (total)	63.6	90°	0.5	0.707	−1406
Total	 	 	 	 	−4406
This is enormous — approximately 4.4 kW of missing LWR cooling on a clear Denver winter night when using synthetic weather.
2.4 Does this affect BESTEST 900FF?
No. The 900FF TOML specifies epw_path = "../../../vendors/OCHRE/ochre/defaults/Weather/USA_CO_Denver.Intl.AP.725650_TMY3.epw", so it uses the EPW parser which correctly computes sky temperature from horizontal infrared radiation.
2.5 Classification: BUG
This is not a shortcut or approximation — it is an outright error. The sky temperature on clear nights is always significantly below air temperature. Setting them equal eliminates a fundamental heat transfer mechanism. Any simulation using synthetic weather (no EPW file) will have zones that are far too warm on clear winter nights.
---
3. Window Exterior LWR Skip
3.1 The code
File: crates/hares-envelope/src/thermal_solver/longwave.rs:39-54
// Windows: LWR is implicit in U-factor; skip exterior radiation calc.
// Without this, windows use zone air temp as "surface temp" (rad_frac=0,
// state_index = zone air), producing massive erroneous LWR cooling.
if info.boundary_category == Some(super::config::BoundaryCategory::Window) {
    continue;
}
3.2 Why it exists
Windows in HARES have no RC node — they are modeled as a pure resistance R = 1/U between outdoor air and zone air. The rad_frac is zero (no thermal capacitance to iterate on), and state_index points to the zone air node. If exterior LWR were naively applied with zone air as the "surface temperature," it would produce absurd results (e.g., zone air at 20°C radiating to sky at −39°C = massive spurious cooling).
3.3 Why it's wrong
The claim "LWR is implicit in U-factor" is only approximately true at standard rating conditions (NFRC/ASHRAE winter: indoor 21°C, outdoor −18°C, 5.5 m/s wind). At these conditions:
- The exterior film coefficient h_out = 34 W/(m²·K) includes both convection (~29.5) and radiation (~4.5)
- The radiation component h_rad ≈ 4 × ε × σ × T_avg³ assumes T_eff = T_air (i.e., the exterior radiative environment is at air temperature)
When T_sky < T_air (which is always the case on clear nights), the actual radiative environment is colder than assumed in the U-factor rating, and the window loses MORE heat than the U-factor predicts. This additional cooling is NOT captured.
3.4 What EnergyPlus does
In EnergyPlus, all exterior surfaces (including windows) participate in the exterior longwave radiation exchange. The surface heat balance computes:
q''_LWR = q''_gnd + q''_sky + q''_air + q''_surrounding
The window's exterior surface temperature is iteratively solved to satisfy the combined convection + radiation + conduction balance. The window material's Front Side Infrared Hemispherical Emissivity (typically 0.84 for clear glass) is used in this calculation.
Reference: EnergyPlus Engineering Reference §External Longwave Radiation; SurfaceProperty:SurroundingSurfaces IDD object defines per-surface view factors and sky temperature schedules.
3.5 Quantitative impact on BESTEST 900FF
At the annual minimum temperature step for 900FF:
- T_zone ≈ 0.9°C, T_outdoor ≈ −18°C, T_sky ≈ −39°C (EPW-derived)
- 12 m² south-facing vertical windows, ε = 0.84, F_sky = 0.5, β = √0.5 ≈ 0.707
- Window exterior surface temp ≈ −16.3°C (estimated from R_ext_film/R_total)
Additional LWR cooling (beyond what U-factor accounts for):
ΔQ = ε × σ × A × β × F_sky × (T_sky⁴ − T_air⁴)
   = 0.84 × 5.670e-8 × 12 × 0.354 × (234.3⁴ − 255.15⁴)
   = 0.84 × 5.670e-8 × 12 × 0.354 × (−1.225 × 10⁹)
   ≈ −248 W
Compare to window conduction: Q_cond = U × A × ΔT = 3.0 × 12 × 17 = 612 W
The missing LWR cooling is 40% of the window's rated conduction loss. For the entire zone, this adds approximately 248 W to a total heat loss of ~2000 W (≈12% increase).
Estimated zone temperature impact: ΔT ≈ 248 / 118 ≈ 2.1°C
This single effect explains most of the observed +2.50°C bias.
3.6 Classification: SHORTCUT (inherited from OCHRE, but physically incorrect)
This is an intentional modeling simplification, not a bug. OCHRE does the same thing ("windows have t_idx=None and all LWR goes to zone.radiation_heat"). However, the simplification is significant enough to invalidate BESTEST results for cases with substantial glazing.
---
4. Combined Impact Analysis for 900FF
4.1 Previously identified causes (from bestest_900ff_root_cause.rs)
Candidate	Description	Estimated Impact
1	Zone air capacitance uses sea-level density (1.2041 vs ~0.987 kg/m³ for Denver)	+0.3–0.5°C
2	Heavyweight concrete gets only 2 RC sub-layers (under-discretization)	+1.5–2.5°C
3	Interior solar absorptance hardcoded 0.6	Minor
Total estimated	 	+2.0–3.0°C
4.2 Newly identified cause
Candidate	Description	Estimated Impact
4	Window exterior LWR skip (12 m² at ε=0.84 loses ~248 W to cold sky)	+2.0–2.5°C
4.3 Revised analysis
The previous analysis estimated Candidate 2 (concrete under-discretization) at +1.5–2.5°C, which combined with Candidate 1 gave +2.0–3.0°C, matching the observed +2.50°C. But this estimate did not account for Candidate 4.
With all four candidates:
- If Candidate 4 contributes ~2°C, then Candidate 2 must contribute less than previously estimated (perhaps +0.5–1.0°C)
- OR the candidates are not simply additive (the concrete super-node effect is partially masked by the missing window LWR)
- The +2.50°C bias is likely explained primarily by the window LWR skip, with concrete under-discretization as a secondary contributor
The key insight is: the previous root-cause analysis estimated the concrete impact by working backwards from the total observed bias. Once window LWR is accounted for, the concrete's contribution must be re-estimated downward.
---
5. Code Changes Needed
5.1 Fix synthetic weather sky temperature (BUG FIX)
File: crates/hares-core/src/dwelling/synthetic.rs:741
// Current (BUG):
sky_temp_c: vec![config.weather.outdoor_temp_c; n],
// Fix:
sky_temp_c: vec![hares_io::epw::clark_allen_sky_temp_c(
    config.weather.outdoor_temp_c,
    config.weather.dew_point_c,
); n],
Also fix the inconsistent companion values:
// Current:
opaque_sky_cover: vec![0.0; n],           // clear sky
horizontal_infrared_w_m2: vec![300.0; n],  // overcast IR value
// Fix: either compute IR from emissivity, or set to a clear-sky value:
horizontal_infrared_w_m2: vec![hares_io::epw::stefan_boltzmann_ir_from_sky_temp(
    hares_io::epw::clark_allen_sky_temp_c(config.weather.outdoor_temp_c, config.weather.dew_point_c)
); n],
This requires either:
- Making clark_allen_sky_temp_c public from hares_io::epw
- Or moving it to a shared location (e.g., hares_physics)
5.2 Add window exterior LWR (MODEL IMPROVEMENT)
File: crates/hares-envelope/src/thermal_solver/longwave.rs:39-54
The current skip must be replaced with a proper exterior LWR calculation for windows. Since windows have no RC node, the approach should be:
1. Estimate window exterior surface temperature from the window U-factor and current zone/outdoor temperatures:
      // For a window with no RC node:
   // T_surf_ext ≈ T_outdoor + R_ext_film / R_total × (T_zone - T_outdoor)
   // where R_ext_film is the convective-only film resistance (≈0.03 m²·K/W)
   // and R_total = 1/U_factor
   let r_ext_film_conv = 0.03;  // convective-only exterior film
   let r_total = 1.0 / u_factor;
   let t_surf_ext = t_outdoor + (r_ext_film_conv / r_total) * (t_zone - t_outdoor);
   
2. Compute exterior LWR using the estimated surface temperature:
      let surface = ExteriorSurface {
       area_m2: info.area_m2,
       emissivity: info.emissivity,  // 0.84 for glass
       sky_view_factor: sky_view_factor(info.tilt_deg),
       beta: beta_factor(info.tilt_deg),
   };
   let q_lwr = exterior_longwave_w(&surface, t_sky, t_air, t_surf_ext);
   
3. Inject the net LWR flux into the zone sensible heat input (since windows have no RC node, all flux goes to zone air):
      // The LWR flux changes the window surface temperature, which changes
   // the conduction. For a simplified model, the additional LWR beyond
   // what the U-factor accounts for is injected directly to zone air.
   let q_lwr_standard = exterior_longwave_w(&surface, t_air, t_air, t_surf_ext);
   let q_lwr_delta = q_lwr - q_lwr_standard;  // additional cooling from cold sky
   u[zone_sensible_idx] += q_lwr_delta;
   
Alternative simpler approach: Rather than separating "standard" and "actual" LWR, replace the window's exterior film coefficient with a combined convection + actual radiation coefficient:
// Effective outdoor temperature for window conduction, accounting for LWR to cold sky
let t_sky_eff = compute_effective_sky_temp_for_surface(t_sky, t_air, f_sky, beta);
let t_outdoor_effective = f_gnd * t_air + f_sky * (beta * t_sky + (1.0 - beta) * t_air);
// Window heat loss using effective outdoor temp instead of air temp
let q_window = u_factor * area * (t_zone - t_outdoor_effective);
This approach is simpler but requires careful derivation to avoid double-counting the U-factor's built-in radiation component.
5.3 Required changes to ExteriorSurfaceInfo
The ExteriorSurfaceInfo struct needs to carry window-specific information to enable the LWR calculation:
- u_factor_w_m2_k (for surface temperature estimation)
- emissivity (already available as 0.84 from EMISSIVITY_WINDOW)
---
6. Empirical Verification Plan
6.1 Synthetic weather sky temperature fix
1. Run a simple test: Create a synthetic weather config with outdoor_temp_c = -18, dew_point_c = -25. Verify that sky_temp_c in the weather time series is ≈ −39°C (Clark-Allen), not −18°C.
2. Run a free-float annual simulation with the fixed sky temperature vs the buggy one. Compare zone minimum temperatures. The fix should produce a significantly lower minimum temperature.
3. Compare with EPW-derived simulation: For the same building, run with EPW weather and synthetic weather (using the same annual average temperature). The fixed synthetic weather should produce results much closer to the EPW simulation.
6.2 Window exterior LWR fix
1. Run BESTEST 900FF with the window LWR fix enabled. Compare the new minimum temperature with the current result (0.9°C). The fix should lower the minimum temperature by ~1.5–2.5°C.
2. Cross-validate with BESTEST 600FF (no windows). The fix should have negligible impact on 600FF, confirming it's window-specific.
3. Run the ASHRAE 140 diagnostic: After both fixes, re-run all BESTEST cases and verify that 900FF falls within the ASHRAE 140 acceptance band −6.4, −1.6°C.
4. Compare component gains: At the min-temp step, the exterior_lwr_w diagnostic should increase by approximately the calculated ΔQ (~248 W for 900FF).
6.3 Regression tests
Add targeted tests to bestest_900ff_root_cause.rs:
- Verify window LWR is computed (not skipped)
- Verify the additional window LWR magnitude is physically reasonable
- Verify the synthetic weather sky temperature uses Clark-Allen
---
7. Summary of Classification
Issue	Location	Type	BESTEST 900FF Impact	Other Simulations Impact
Sky temp = outdoor temp (synthetic)	synthetic.rs:741	BUG	None (uses EPW)	Severe (all synthetic weather runs)
Window exterior LWR skip	longwave.rs:42	SHORTCUT	~+2.0–2.5°C (dominant cause)	Significant (all cases with exterior glazing)
Inconsistent opaque_sky_cover/IR	synthetic.rs:739-740	BUG (minor)	None	Minor inconsistency
---
8. Recommended Priority
1. Fix window exterior LWR (HIGH) — This is the dominant cause of the 900FF bias and affects all simulations with exterior glazing. Without this fix, HARES cannot produce valid BESTEST results for cases with windows.
2. Fix synthetic weather sky temperature (HIGH) — This is a straightforward bug that completely eliminates sky radiation cooling for synthetic weather. Any building simulation using synthetic weather will have zones that are far too warm on clear nights.
3. Re-evaluate concrete under-discretization impact (MEDIUM) — After fixing window LWR, re-run BESTEST and re-assess whether concrete sub-layer splitting is still needed to close the gap.
4. Add altitude-corrected air density (LOW) — This was previously identified but has minimal impact (~0.3–0.5°C). Fix it, but it's not the primary issue.