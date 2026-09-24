Interior Longwave Radiation (LWR) Investigation Report: BESTEST 900FF Minimum Temperature Bias
Executive Summary
The diagnostic interior_lwr_w = 0.0 is a MISLEADING diagnostic, not evidence that interior LWR is inactive. The interior LWR model IS implemented, IS active for 900FF, and IS computing non-zero per-surface fluxes. However, two issues degrade its effectiveness:
1. Misleading diagnostic (low severity): interior_lwr_w reports the sum of all surface LWR fluxes, which is always zero by energy conservation (Σq_i = 0). This creates the false impression that interior LWR is doing nothing.
2. Window LWR flux dropped (BUG — wrong physics): Window surfaces' net LWR flux is computed but never applied to any thermal node. This drops ~100–200 W of heat redistribution at the 900FF min-temp hour.
3. Reduced LWR effectiveness due to RC under-discretization (significant): The super-node effect (2 concrete RC nodes instead of 4+) makes surface temperatures too similar, reducing the temperature differences that drive LWR exchange. This is the dominant cause of the 2.5°C warm bias.
Classification: The window LWR bug is a bug (wrong physics — violates energy conservation for the zone air node). The diagnostic is a design flaw. The under-discretization is a known accuracy limitation already documented in bestest_900ff_root_cause.rs.
---
1. WHY interior_lwr_w = 0.0 — Exact Code Path
1.1 The diagnostic is the zero-sum of an energy-conserving exchange
The diagnostic value interior_lwr_w is computed at:
File: crates/hares-envelope/src/thermal_solver/mod.rs:474–479
let interior_lwr_w = self
    .lwr_by_zone_buf
    .iter()
    .find(|(z, _)| *z == self.config.indoor_zone_id)
    .map(|(_, w)| *w)
    .unwrap_or(0.0);
The lwr_by_zone_buf is populated at:
File: crates/hares-envelope/src/thermal_solver/longwave.rs:316
self.lwr_by_zone_buf.push((zone_cfg.zone_id, zone_total));
Where zone_total accumulates each surface's net flux:
File: crates/hares-envelope/src/thermal_solver/longwave.rs:290–308
let mut zone_total = 0.0_f64;
for (_j, (info, &q)) in zone_cfg
    .surfaces
    .iter()
    .zip(self.lwr_net_flux_buf.iter())
    .enumerate()
{
    // ... apply flux to u ...
    zone_total += q;
}
The LWR model is energy-conserving by construction. Proof from longwave_radiation.rs:294–302:
q_net_i = (total_out × view_factor_i) - q_out_i
where view_factor_i = A_i·ε_i / Σ(A_j·ε_j) and Σ view_factor_i = 1.
Therefore:
Σ q_net_i = total_out × Σ view_factor_i - Σ q_out_i = total_out - total_out = 0
Result: zone_total = 0.0 ALWAYS, regardless of how large the individual surface fluxes are. The diagnostic is structurally incapable of showing non-zero interior LWR activity.
1.2 Interior LWR IS active for 900FF
The 900FF building has 6 opaque boundaries + 2 window boundaries, all in the same conditioned zone. The include_interior_lwr filter at solver_builder.rs:368–382 includes all of them:
- Walls: include_interior_lwr(false, true, true, false, area) → true (is_conditioned_interior)
- Roof: include_interior_lwr(false, true, true, false, area) → true
- Floor: include_interior_lwr(false, false, true, false, area) → true (is_conditioned_interior even though not exterior)
- Windows: include_interior_lwr(true, true, false, false, area) → true (is_window && is_exterior)
The surfaces are grouped into interior_lwr_zones at solver_builder.rs:767–777:
for (zid, surfaces) in surfaces_by_zone {
    if surfaces.len() >= 2 {
        let mut zone_cfg = hares_envelope::InteriorLwrZoneConfig { ... };
        zone_cfg.compute_scriptf();
        thermal_cfg.interior_lwr_zones.push(zone_cfg);
    }
}
The 900FF conditioned zone has 8 surfaces (≥ 2), so it IS configured with ScriptF coefficients.
---
2. The Window LWR Flux Bug
2.1 Description
At longwave.rs:297–307:
// Surfaces driven by an environmental temperature (windows without RC nodes)
// have no thermal capacitor. Their net LWR flux is conducted to the exterior
// via the window U-factor and does not enter zone air.
if info.driving_temp.is_none() && info.input_index < u.len() {
    u[info.input_index] += q * info.radiation_frac;
    if let Some(ai) = air_idx {
        if ai < u.len() {
            u[ai] += q * (1.0 - info.radiation_frac);
        }
    }
}
When info.driving_temp.is_some() (windows), the net LWR flux q is computed but never applied to any input. The comment claims "Their net LWR flux is conducted to the exterior via the window U-factor and does not enter zone air," but this is physically incorrect.
2.2 Why this is wrong physics
In reality, the window glass absorbs interior LWR from warm surfaces. This absorbed heat:
1. Warms the glass slightly (reducing the ΔT across the window, thus reducing conduction loss)
2. Re-radiates some heat back to the zone
3. Conducts some heat to the exterior
In the EnergyPlus model (confirmed by the zone air heat balance equation from the Context7 documentation):
C_z dT_z/dt = ΣQ_conv + Σ[h_i·A_i·(T_si - T_z)] + infiltration + HVAC
where the convective term Σ[h_i·A_i·(T_si - T_z)] includes ALL surfaces, including windows. The window's interior surface temperature is affected by LWR absorption, and the convective exchange with zone air depends on this temperature.
In HARES's simplified model (no RC node for windows), the correct approach is to route the window's net LWR flux to the zone air sensible input. This is because:
- The window has no thermal capacitance in the model
- The LWR heat absorbed by the window would immediately warm the glass
- The warmed glass would either conduct heat outside or convect heat to zone air
- Since the window U-factor already handles conduction, the LWR portion should go to zone air
2.3 Estimated magnitude for 900FF
At the 900FF min-temp hour (~-18°C outdoor, ~1°C zone):
Surface	Area (m²)	Emissivity
Walls (concrete)	63.6	0.90
Roof	48.0	0.90
Floor	48.0	0.90
Windows	12.0	0.84
MRT ≈ 0.5°C (area-weighted), ΔT_window ≈ 3.5°C.
Linearized h_r at ~270K: 4 × 0.84 × 5.67e-8 × 270³ ≈ 4.7 W/(m²·K)
Estimated window net LWR: 4.7 × 12 × 3.5 ≈ 197 W
This 197 W is currently dropped entirely.
2.4 Impact direction
The window gains LWR heat (positive q). If applied to zone air, it would WARM the zone by:
ΔT ≈ 197 W × (1/C_zone) × dt_s ≈ 197 / 44 ≈ 4.5°C/hour (instantaneous rate)
But this overstates the sustained impact because:
- The warm surfaces cool as they lose LWR, reducing the flux
- The window warms, reducing the ΔT
- The actual sustained impact over several hours is ~0.5–1.5°C
Fixing the window LWR bug would make 900FF WARMER, not cooler. This bug partially offsets the super-node warm bias but is still wrong physics that must be fixed.
---
3. How Interior LWR Should Work (Correct Model)
3.1 EnergyPlus approach
EnergyPlus uses ScriptF (Gebhart) factors for interior LWR, which HARES already implements correctly in ScriptFCoefficients. The key difference is in how the net flux is applied:
EnergyPlus applies ALL surface LWR fluxes to the zone heat balance:
- For surfaces with RC nodes: flux goes to the surface node AND zone air (convective fraction)
- For windows: the net LWR is split between:
  - The window glass absorbed fraction → contributes to window inside surface temperature → affects convective exchange with zone air
  - The zone air directly (for the portion that bypasses the glass)
In EnergyPlus's inside surface heat balance:
q_conv_i = h_conv_i × A_i × (T_surf_i - T_zone)
where T_surf_i is determined by the conduction + LWR + solar balance at the surface.
3.2 Required fix for HARES
For window surfaces (driving_temp.is_some()), the net LWR flux should be applied entirely to zone air:
if info.driving_temp.is_some() {
    // Window has no RC node. Its net LWR flux is absorbed by the glass
    // and immediately convected to zone air or conducted outside.
    // Route all of it to zone air sensible input.
    if let Some(ai) = air_idx {
        if ai < u.len() {
            u[ai] += q;
        }
    }
}
This matches the physical reality: the window absorbs LWR from warm surfaces, the glass temperature rises, and the absorbed heat is convected to zone air. The window U-factor already handles the steady-state conduction through the glass; the LWR perturbation is an additional transient effect.
---
4. Expected Impact on 900FF
4.1 Impact of fixing the window LWR bug alone
Direction: Zone becomes WARMER at min-temp hour.
Magnitude: ~0.5–1.0°C warmer at the annual minimum.
This makes the 900FF outlier worse, not better. But it's correct physics.
4.2 Impact of fixing the RC under-discretization (4+ concrete nodes)
Direction: Zone becomes COOLER at min-temp hour.
Mechanism: With 4 concrete nodes, the temperature gradient across the concrete is better resolved. The innermost node is no longer a "super-node" tightly coupled to zone air. Instead, the interior concrete surface temperature is properly intermediate between zone air and the exterior, allowing:
- Better heat drainage through the insulation bottleneck
- Larger surface temperature differences → more effective interior LWR redistribution
- More realistic concrete-to-zone-air coupling
Magnitude: ~1.5–2.5°C cooler at the annual minimum (per bestest_900ff_root_cause.rs:609–612).
4.3 Combined effect
Fixing both the window LWR bug AND the RC under-discretization:
- RC fix: -1.5 to -2.5°C
- Window LWR fix: +0.5 to +1.0°C (partially offsetting)
- Net: -1.0 to -2.0°C
This would bring 900FF from +0.9°C (2.5°C above upper bound) to approximately -0.1 to -1.1°C, potentially within the ASHRAE band -6.4, -1.6°C or at least much closer.
4.4 Additional factor: zone air density
Per bestest_900ff_root_cause.rs, the sea-level air density (1.2041 vs ~0.987 for Denver) overstates zone capacitance by 22%, contributing 0.3–0.5°C to the warm bias.
---
5. Code Changes Required
5.1 Fix the window LWR bug (Priority: HIGH — wrong physics)
File: crates/hares-envelope/src/thermal_solver/longwave.rs:297–307
Current code:
if info.driving_temp.is_none() && info.input_index < u.len() {
    u[info.input_index] += q * info.radiation_frac;
    if let Some(ai) = air_idx {
        if ai < u.len() {
            u[ai] += q * (1.0 - info.radiation_frac);
        }
    }
}
Fixed code:
if info.driving_temp.is_some() {
    // Window surfaces have no RC node. Their net interior LWR flux
    // is absorbed by the glass and convected to zone air. The window
    // U-factor handles steady-state conduction; the LWR perturbation
    // is an additional transient effect that goes to zone air.
    if let Some(ai) = air_idx {
        if ai < u.len() {
            u[ai] += q;
        }
    }
} else if info.input_index < u.len() {
    u[info.input_index] += q * info.radiation_frac;
    if let Some(ai) = air_idx {
        if ai < u.len() {
            u[ai] += q * (1.0 - info.radiation_frac);
        }
    }
}
5.2 Fix the misleading diagnostic (Priority: MEDIUM — diagnostic quality)
File: crates/hares-envelope/src/thermal_solver/longwave.rs:290–316
The zone_total should report the sum of absolute fluxes or the maximum absolute flux, not the algebraic sum (which is always zero):
let mut zone_total_abs = 0.0_f64;  // or rename to zone_max_abs_flux
// ...
zone_total_abs += q.abs();
// ...
self.lwr_by_zone_buf.push((zone_cfg.zone_id, zone_total_abs));
Or better: report the sum of fluxes applied to zone air specifically, which IS a meaningful non-zero quantity:
let mut zone_air_lwr_w = 0.0_f64;
// ...
if info.driving_temp.is_some() {
    zone_air_lwr_w += q;
} else {
    zone_air_lwr_w += q * (1.0 - info.radiation_frac);
}
5.3 Fix the RC under-discretization (Priority: HIGH — dominant cause)
This is already documented in bestest_900ff_root_cause.rs. Options:
- Use C_discretization = 1 instead of 3 (more nodes, smaller dx)
- Use a finer default timestep for concrete layers
- Override the split count for heavyweight layers
5.4 Fix the zone air density (Priority: MEDIUM — secondary contributor)
Pass site altitude/pressure to derive_zone_capacitances so it uses Denver-corrected density instead of sea-level constant.
---
6. How to Verify Empirically
6.1 Verify window LWR flux is non-zero
Add a temporary diagnostic in apply_interior_longwave_inputs that prints individual surface fluxes:
for (_j, (info, &q)) in zone_cfg.surfaces.iter().zip(self.lwr_net_flux_buf.iter()).enumerate() {
    if info.driving_temp.is_some() {
        tracing::warn!(surface_idx = _j, flux_w = q, "WINDOW LWR FLUX (currently dropped)");
    }
}
Run debug_900ff_min_temp_heat_balance test and check that window flux is non-zero.
6.2 Verify the fix
1. Apply the window LWR fix
2. Run the 900FF annual simulation
3. Check that interior_lwr_w is no longer zero (if using the corrected diagnostic)
4. Expect: zone minimum temperature INCREASES by ~0.5–1.0°C (warmer due to window LWR gain to zone air)
5. This confirms the fix is working correctly, even though it makes the outlier worse
6.3 Verify combined RC + LWR fix
1. Apply both the RC under-discretization fix AND the window LWR fix
2. Run the 900FF annual simulation
3. Expect: zone minimum temperature DECREASES by ~1.0–2.0°C net
4. The RC fix dominates, the LWR fix partially offsets
---
7. Classification
Issue	Classification
Window LWR flux dropped	BUG (wrong physics, violates energy balance)
Diagnostic reports zero-sum	DESIGN FLAW (misleading, not wrong)
RC under-discretization	ACCURACY LIMITATION (documented, dominant cause)
Zone air density	BUG (hardcoded sea-level)
The window LWR bug is a genuine bug — it violates the first law of thermodynamics for the zone air node. Heat is removed from warm surfaces (concrete) via LWR but the corresponding heat gained by cold surfaces (windows) is not deposited anywhere. This creates a net energy loss from the zone that shouldn't exist.
However, paradoxically, fixing this bug alone makes the 900FF problem worse. The bug is currently providing a small spurious cooling effect that partially offsets the warm bias from the RC under-discretization. The correct fix sequence is:
1. First fix the RC under-discretization (dominant cause, -1.5 to -2.5°C)
2. Then fix the window LWR bug (correct physics, +0.5 to +1.0°C offset)
3. Then fix the zone air density (secondary, -0.3 to -0.5°C)
4. Net expected improvement: -1.3 to -2.0°C, bringing 900FF closer to the ASHRAE band
---
8. References
- EnergyPlus Engineering Reference: "Inside Surface Heat Balance" — interior LWR exchange uses ScriptF (Gebhart) factors; window net LWR is included in the zone air heat balance via convective coupling.
- ASHRAE 140-2017 Table B8-3a: Case 900FF free-float temperature band -6.4, -1.6°C.
- bestest_900ff_root_cause.rs: Existing root cause analysis identifying RC under-discretization and air density as candidates.
- HARES codebase: longwave_radiation.rs (LWR physics), longwave.rs (solver integration), solver_builder.rs (surface configuration), config.rs (InteriorSurfaceInfo definition).