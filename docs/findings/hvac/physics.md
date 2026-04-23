HARES HVAC-Envelope Physics Coupling Audit Report
Date: 2026-04-23  
Scope: Envelope solver, HVAC→envelope data flow, coil physics, humidity/latent interaction, ventilation, physics crate  
Rating: B+ — Solid ASHRAE/OCHRE-aligned physics with a few significant inconsistencies that should be addressed
---
Findings by Severity
🔴 CRITICAL — Conservation / Consistency Violations
F1. Latent Heat of Vaporization Inconsistency Across Modules
Files: hares-equipment/src/hvac/dehumidifier.rs, hares-physics/src/constants.rs, hares-envelope/src/thermal_solver/mod.rs:35
The dehumidifier hardcodes LATENT_HEAT_VAPORIZATION_J_KG = 2_454_000 (≈ h_fg at 20°C), while the thermal solver uses H_FG_J_PER_KG = LATENT_HEAT_VAPORISATION_0C_KJ_KG * KJ_TO_J = 2_501_000 (h_fg at 0°C per ASHRAE). The ~1.9% discrepancy means:
- If the dehumidifier removes X kg of moisture, it reports X × 2,454,000 W of latent energy
- The thermal solver expects X × 2,501,000 W for the same moisture removal
- This creates a systematic energy imbalance whenever dehumidifier output flows into the thermal solver
Impact: ~1.9% latent energy accounting error in dehumidification scenarios. Small per-timestep but accumulates over a season.  
Fix: Unify on constants.rs::LATENT_HEAT_VAPORISATION_0C_KJ_KG (or a temperature-dependent function).
---
F2. HVAC Latent→Humidity Coupling May Be Incomplete
Files: hares-types/src/ports.rs, hares-equipment/src/hvac/ideal_hvac.rs:552-571, hares-equipment/src/hvac/air_conditioner.rs, hares-envelope/src/humidity_solver.rs
The PortContribution enum has no Humidity variant — only Thermal { latent_gain_w }. HVAC cooling equipment writes latent energy (W) but not humidity ratio change (kg/kg). The humidity solver must derive moisture removal from latent_gain_w, which requires:
1. A consistent h_fg for the W → kg/s conversion  
2. Knowledge of the zone air mass flow rate
If the humidity solver uses a different h_fg than the equipment that produced latent_gain_w, the derived moisture removal will be inconsistent with the reported energy. This is the same h_fg inconsistency as F1, but extended to ALL cooling equipment (AC, HP, Ideal HVAC), not just the dehumidifier.
Impact: Potential humidity ratio drift when cooling equipment is active — the thermal solver sees correct latent energy but the humidity solver may remove too much or too little moisture.  
Fix: Either (a) add a Humidity { zone, humidity_ratio_delta } variant to PortContribution and have equipment write it directly, or (b) ensure the humidity solver uses the exact same h_fg that equipment used to compute latent_gain_w.
---
🟠 HIGH — Numerical Stability / Coupling Concerns
F3. Infiltration Latent Treatment Is Fully Explicit
File: hares-envelope/src/thermal_solver/infiltration.rs
The infiltration sensible load uses semi-implicit coupling (implicit diagonal, explicit forcing), which is numerically stable. However, the latent component from infiltration is fully explicit — computed from current-timestep humidity ratios without any implicit stabilization. At large timesteps (≥5 min) with high infiltration and large indoor/outdoor humidity differences, this can cause oscillatory humidity behavior.
Impact: Humidity oscillation risk at coarse timesteps with high infiltration. The 15× moisture buffering multiplier in the humidity solver partially mitigates this by increasing effective moisture capacitance.  
Fix: Consider semi-implicit treatment of infiltration latent coupling (analogous to the sensible path), or document the timestep limitation.
---
F4. Ideal HVAC Fan Heat Added to Sensible During Cooling — Sign Ambiguity in Diagnostics
File: hares-equipment/src/hvac/ideal_hvac.rs:552-571
During cooling mode:
sensible_gain_w: sensible_w + fan_power_w,  // negative + positive
category: ThermalCategory::HvacCooling,
The fan heat correctly offsets cooling (physics is right), but the entire sum is categorized as HvacCooling. This means component_gains.hvac_cooling_w (read from sensible_for_category(HvacCooling)) will report a less-negative value than the true coil cooling output. Downstream diagnostics that compare "coil capacity" vs "net zone cooling" will see a mismatch that isn't documented.
Impact: Diagnostic confusion — the "HVAC cooling" category mixes coil cooling with fan heating. Not a physics error but a reporting inconsistency.  
Fix: Consider reporting fan heat under a separate category or sub-category, or document the mixing behavior.
---
F5. Biquadratic Default Bounds Allow Extreme Extrapolation
File: hares-equipment/src/hvac/hvac_core.rs:45-46
Default biquadratic bounds are (-100.0, 100.0) °C for both x1 (indoor wet-bulb) and x2 (outdoor dry-bulb). While this matches OCHRE's fallback, it allows the polynomial to extrapolate wildly at extreme conditions (e.g., -40°C outdoor with a polynomial calibrated for 17–35°C). The biquadratic coefficients are typically fit over a 15–20°C range.
Impact: Potential for unrealistic capacity/EIR values at temperature extremes, especially in cold-climate heat pump heating.  
Fix: Encourage config-driven bounds (already supported via biquadratic_x1_min/max), and consider tightening the default to ±50°C or the ASHRAE rating range.
---
🟡 MEDIUM — Modeling Assumptions / Minor Issues
F6. Window Ground-Reflected Solar Uses Diffuse IAM
File: hares-envelope/src/thermal_solver/solar.rs:34
Ground-reflected solar (reflected_w_m2) is bundled with diffuse and both use the hemispherical diffuse IAM:
let poa_diffuse = (irr.diffuse_w_m2 + irr.reflected_w_m2) * iam_diffuse;
Ground-reflected radiation arrives from below (around 0° altitude from the window's perspective), which has a different angular distribution than sky diffuse. EnergyPlus treats ground-reflected with a separate ground IAM factor in the FullInteriorAndExterior model, but the residential simplification bundles them. This matches OCHRE behavior.
Impact: Minor — ground-reflected is typically small (5–15% of total). The diffuse IAM is a reasonable approximation for residential windows.  
Fix: Low priority. Document the simplification.
---
F7. Exterior LWR Iteration Damping Parameters Are Fixed
File: hares-envelope/src/thermal_solver/longwave.rs:184
The heavy-ball damping uses fixed coefficients 0.5 (momentum) and 0.1 (previous-step acceleration):
let t_next = t_surf + 0.5 * (t_new - t_surf) + 0.1 * (t_surf - t_surf_prev);
These work well for typical residential surfaces but may not converge for extreme emissivity/area combinations or very large sky-air temperature differences. The ±2°C per-iteration clamp provides a safety net.
Impact: Rare non-convergence risk. The clamp prevents divergence.  
Fix: Low priority. Could add a convergence warning log if iterations hit the limit.
---
F8. Interior LWR ScriptF Damping Coefficients Differ from Exterior
File: hares-envelope/src/thermal_solver/longwave.rs:310
Interior LWR uses 0.3 (momentum) and 0.2 (previous-step), which are different from exterior's 0.5/0.1. The n_iter = floor(dt/300) + 3 provides enough iterations at typical timesteps (60s → 3 iterations, 300s → 4 iterations).
Impact: Minimal — interior LWR fluxes are typically small corrections.  
Fix: None needed. Different damping for different problem scales is reasonable.
---
F9. Constant Naming Inconsistency (British/American Spelling)
Files: hares-physics/src/constants.rs vs hares-equipment/src/hvac/dehumidifier.rs
- constants.rs: LATENT_HEAT_VAPORISATION_0C_KJ_KG (British -isation)  
- dehumidifier.rs: LATENT_HEAT_VAPORIZATION_J_KG (American -ization)
Impact: Cosmetic, but confusing when searching for the "right" constant.  
Fix: Standardize on one spelling across the codebase.
---
🟢 LOW — Documentation / Observations
F10. Zone Air Capacitance 7× Multiplier Is Intentional but Surprising
File: hares-envelope/src/boundary_rc.rs
AIR_DENSITY * AIR_CP * volume * INTERIOR_MASS_MULTIPLIER(7×) lumps furniture thermal mass into the air node. This matches OCHRE's approach and is documented, but it means the "zone air" node has 7× the thermal capacitance of pure air. This affects:
- The time constant of the zone response
- The ideal-capacity back-solve (which targets this combined node)
- The apparent sensitivity of zone temperature to HVAC inputs
Impact: None — this is a deliberate modeling choice with OCHRE provenance.  
Fix: Ensure documentation is clear that "zone air temperature" is really "zone air + furniture" temperature.
---
F11. Moisture Buffering 15× Multiplier
File: hares-envelope/src/humidity_solver.rs
The 15× multiplier on moisture capacitance accounts for hygroscopic sorption by interior materials. This matches OCHRE and is physically motivated, but the specific value is an empirical calibration rather than a first-principles derivation.
Impact: None — standard residential building simulation practice.  
Fix: None needed.
---
F12. Debug-Only Diagnostics in Release Builds
Files: Multiple #[cfg(any(debug_assertions, feature = "observe_detailed"))] guards
Per-exterior-surface diagnostics, per-interior-surface diagnostics, and per-window solar diagnostics are compiled out in release builds. This means energy-balance verification cannot be performed in production runs.
Impact: Operational — cannot verify physics correctness in release-mode simulations.  
Fix: Consider making observe_detailed a default-on feature, or adding a lightweight energy-balance check that's always compiled.
---
Overall Assessment by Focus Area
Area	Rating	Summary
Envelope Solver	A	ZOH discretization with semi-implicit infiltration is numerically sound. Steady-state initialization is correct. State-space formulation is well-structured.
HVAC→Envelope Data Flow	B+	Port system is clean and well-typed. Category routing works correctly. The missing humidity port variant (F2) is the main gap.
Coil Physics	A-	SHR bisection solver, bypass factor, and Henderson-Rengarajan latent degradation are correctly implemented per ASHRAE.
Humidity/Latent Interaction	B	h_fg inconsistency (F1) and missing humidity port (F2) are significant. Fully-explicit infiltration latent (F3) is a stability concern.
Ventilation	A-	HRV/ERV model with sensible+latent effectiveness and defrost derating is well-implemented. Mechanical ventilation params correctly passed from equipment to envelope.
Physics Crate	A	ASHRAE-sourced psychrometrics, correct unit conversions via uom, authoritative constants. The h_fg value at 0°C is the right reference choice.
---
## Recommended Fix Priority
1. **F1 + F2** (Critical): Unify h_fg across all modules and add explicit humidity coupling to the port system. This is the highest-impact fix.
2. **F3** (High): Add semi-implicit infiltration latent coupling or document timestep limitations.
3. **F4** (High): Separate fan heat from coil cooling in diagnostic categories.
4. **F5** (High): Tighten default biquadratic bounds.
5. **F9** (Medium): Standardize constant naming.
6. **F6–F8** (Medium/Low): Document simplifications; no code changes needed.
7. **F10–F12** (Low): Documentation and operability improvements.
---
Confidence: High for F1, F2, F4, F9, F10, F11, F12 (direct code evidence). Medium for F3 (inferred from architecture, needs humidity solver re-read to confirm exact coupling mechanism). Medium for F5, F6, F7, F8 (modeling judgment calls with OCHRE provenance).