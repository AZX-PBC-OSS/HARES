# StarMesh radiation_frac derivation from first-principles UA
**Review ID**: envelope-01  
**Category**: envelope  
**Date**: 2026-05-25

## Files Reviewed
- `crates/hares-envelope/src/thermal_solver/config.rs`
- `crates/hares-envelope/src/thermal_solver/mod.rs`
- `crates/hares-envelope/src/thermal_solver/longwave.rs`
- `crates/hares-envelope/src/boundary_rc.rs`
- `crates/hares-envelope/src/rc_network.rs`
- `crates/hares-core/src/dwelling/solver_builder.rs`

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Models/Envelope.py`
- `vendors/OCHRE/ochre/utils/envelope.py`
- `vendors/EnergyPlus/src/EnergyPlus/HeatBalanceSurfaceManager.cc`
- `vendors/EnergyPlus/src/EnergyPlus/Construction.cc`

## Findings

### Finding 1: [Severity: low] Formula is locally correct as a steady-state voltage divider
**Description**: The formula `radiation_frac = R_film / (R_film + R_inner_half)` used in `solver_builder.rs:279-281` and `config.rs:242-256` is mathematically exact for computing the floating interior surface temperature from the innermost capacitor node temperature and zone air temperature. The derivation at `solver_builder.rs:256-267` correctly reduces the KCL at the surface node:

```
G_conv × (T_zone − T_surf) + G_half × (T_node − T_surf) = 0
⇒ T_surf = T_node × G_half/(G_conv + G_half) + T_zone × G_conv/(G_conv + G_half)
```

where `G_half/(G_conv + G_half)` simplifies to `R_film/(R_film + R_inner_half)`. For this local circuit, deeper wall layers are irrelevant — the surface node has only two connections (R_film and R_inner_half), so the formula is exact regardless of multi-layer complexity.

**Code Location**: `crates/hares-core/src/dwelling/solver_builder.rs:279-281` (interior), `crates/hares-core/src/dwelling/solver_builder.rs:238-241` (exterior)

**Root Cause**: N/A — no defect.

**Impact**: None. The local voltage divider is algebraically exact.

---

### Finding 2: [Severity: low] Formula matches OCHRE BoundarySurface.radiation_frac identically
**Description**: OCHRE computes `surface.radiation_frac = self.res_film / (self.res_film + res_material)` at `Envelope.py:254`, where `res_material = res_values[-1]` (the last entry in the processed RC resistance list). OCHRE's `create_rc_data()` (envelope.py:320-323) pads with zeros and averages adjacent resistances, producing output resistances that are exactly the half-resistances between capacitor nodes. The last entry is `R_layer_innermost / 2`, matching HARES's `r_inner_half = inner.thickness_m / (2.0 × k_inner_eff)` at `boundary_rc.rs:1326`. Both implementations compute the identical quantity.

**Code Location**: 
- HARES: `crates/hares-envelope/src/boundary_rc.rs:1326`
- OCHRE: `vendors/OCHRE/ochre/Models/Envelope.py:254`

**Root Cause**: N/A — compatibility is confirmed.

**Impact**: None. HARES and OCHRE are consistent.

---

### Finding 3: [Severity: medium] Diagnostic surface temperature does not account for Y-Δ redistribution in StarMesh mode
**Description**: In StarMesh mode, the `surface_node` is a floating node between `R_film_conv` and `R_inner_half`. After the star node and surface node are eliminated by `reduce_floating_nodes()` at `rc_network.rs:220-276`, radiation conductances are redistributed to `inner_node` and `zone_air` via Y-Δ transform. The diagnostic surface temperature computed at `stepping.rs:533` uses the pre-elimination formula `radiation_frac × T_node + (1 − radiation_frac) × T_zone`, which gives the temperature at the hypothetical surface node position. After elimination, this point no longer exists in the network — the Y-Δ transform merged the radiation conductance with the conduction and convection paths. The diagnostic temperature is still a reasonable approximation (first-order correct), but does not reflect the redistributed conductances that may slightly alter the effective thermal partitioning at the zone-air boundary.

**Code Location**: 
- Surface node creation: `crates/hares-envelope/src/boundary_rc.rs:1305-1315`
- Y-Δ elimination: `crates/hares-envelope/src/rc_network.rs:220-276`
- Diagnostic surface temp: `crates/hares-envelope/src/thermal_solver/stepping.rs:533`

**Root Cause**: The `radiation_frac` formula preserves pre-elimination surface node behavior, but the elimination step in `reduce_floating_nodes()` redistributes conductances between `inner_node`, `zone_air`, and the outdoor boundary in ways that do not preserve the exact surface node temperature. This is an intrinsic limitation of the floating-node elimination technique, not a HARES-specific defect.

**Impact**: The boundary diagnostic surface temperature may deviate by <1°C from the actual physical surface temperature for extreme cases (heavy walls with large R_inner_half and strong inter-surface radiation). This does NOT affect the A-matrix state evolution or zone air heat balance, which are correctly handled by the eliminated network. Only diagnostic outputs are affected.

---

### Finding 4: [Severity: low] EnergyPlus CTF approach uses fundamentally different surface temperature computation
**Description**: EnergyPlus computes interior surface temperature at `HeatBalanceSurfaceManager.cc:8021-8041` as:

```
T_surf = (Σ history_terms + HConv × T_air + ... ) / (CTFInside[0] − CTFCross[0] + HConv + damping)
```

where `CTFInside[0]` and `CTFCross[0]` are the zero-lag conduction transfer function coefficients representing the full-wall instantaneous conductive response. This differs from HARES's RC voltage-divider approach which computes `T_surf` from the innermost capacitor node temperature. The CTF method produces a surface temperature that incorporates the instantaneous response of ALL wall layers simultaneously, whereas the RC method filters multi-layer dynamics through the chain of capacitor nodes. The RC approach converges to the CTF result as the number of sub-layers increases (ISO 13786:2007 §6.2), and with typical diurnal-resolution discretization (1-3 sub-layers per heavy layer) the surface temperature difference is expected to be <2°C for residential applications.

**Code Location**: 
- E+ CTF: `vendors/EnergyPlus/src/EnergyPlus/HeatBalanceSurfaceManager.cc:8021`
- HARES RC: `crates/hares-envelope/src/thermal_solver/stepping.rs:533`

**Root Cause**: Fundamental architectural difference — lumped RC vs. CTF discretization. HARES intentionally chose the RC approach (matching OCHRE) over CTF for simplicity and consistency with the state-space solver framework.

**Impact**: Expected ≤2°C deviation in interior surface temperature for typical residential walls at 60s timestep. Zone air heat balance is unaffected because the A-matrix contains the full RC network, not the interpolated surface temperature. This difference only affects diagnostics and LWR surface temperature in ScriptF mode.

---

### Finding 5: [Severity: low] Transient lag in radiation_frac interpolation during rapid zone temperature changes
**Description**: The `radiation_frac` voltage divider is a steady-state formula. During rapid changes in zone air temperature (e.g., HVAC cycling with large ΔT between setpoint and setback), the interior surface temperature computed by the formula responds faster to T_zone changes than the physical surface temperature, because the thermal mass in the wall delays the response of T_node but the formula apportions (1 − radiation_frac) × ΔT_zone directly to T_surf. For a lightweight wall with radiation_frac ≈ 0.85 and a 5°C zone temperature step, the formula predicts an instantaneous surface temperature change of (1 − 0.85) × 5 = 0.75°C, while the physical surface may only change by ~0.3°C in the same timestep due to wall thermal mass. This is inherent to the local voltage divider approach and exists in OCHRE as well.

**Code Location**: `crates/hares-envelope/src/thermal_solver/longwave.rs:309` (ScriptF surface temp computation), `thermal_solver/stepping.rs:533` (diagnostic)

**Root Cause**: The formula solves only the two-resistor local KCL, ignoring the dT_node/dt term that couples surface temperature to the RC network's transient dynamics. A full solution would require the state-space derivative at the surface node, which is eliminated in both StarMesh and ScriptF modes.

**Impact**: Minor (sub-degree) during transient events. No impact on steady-state or slow-ramp conditions. This is a known limitation of the lumped-RC approach and is documented as T-0082 in the project's known limitations. The per-step TARP recomputation already addresses the frozen-film coefficient issue, but the surface temperature interpolation lag remains as a second-order effect.

---

### Finding 6: [Severity: low] Window radiation_frac uses EnergyPlus interior film decomposition, not the RC voltage divider
**Description**: At `solver_builder.rs:806-807`, window surfaces compute `radiation_frac` using the E+ window interior film decomposition:

```rust
let res_int = 1.0 / (0.359073 * u_window.ln() + 6.949915);
let rad_frac = (res_int / r_total).clamp(0.0, 1.0);
```

This differs from the opaque surface formula `r_film_int / (r_film_int + r_inner_half)`. For windows without RC nodes, there IS no `r_inner_half`, and the E+ polynomial `r_interior = 1/(0.359073·ln(U) + 6.949915)` at `Construction.cc` decomposes the combined h_si into convective and radiative portions. The formula `res_int / r_total` (where `r_total = 1/U`) distributes the window's total thermal coupling between the glass interior surface and the indoor zone, consistent with EnergyPlus Step 1 of the simple window model. This is correct for windows and matches OCHRE's `calculate_window_parameters`.

**Code Location**: `crates/hares-core/src/dwelling/solver_builder.rs:806-807`

**Root Cause**: N/A — the window path correctly uses E+ film decomposition. This is a different formula domain than the opaque-surface RC voltage divider.

**Impact**: None. The window path is consistent with EnergyPlus and OCHRE.

---

## Summary
- **Total findings**: 6
- **Critical**: 0
- **High**: 0
- **Medium**: 1 (Finding 3: post-elimination diagnostic surface temperature in StarMesh mode)
- **Low**: 5 (Findings 1, 2, 4, 5, 6)

## Recommendations

1. **Document the StarMesh diagnostic surface temperature limitation** (Finding 3). Add a comment in `BoundaryDiagnosticInfo::RCNode` at `config.rs:40-49` noting that the diagnostic `T_surface` formula reflects the pre-elimination surface node and may differ slightly from the true physical interior surface temperature after Y-Δ redistribution. Consider whether end-users need a post-elimination corrected surface temperature or whether the current ±1°C accuracy is sufficient for component-load reporting.

2. **Verify BESTEST compliance with the RC surface temperature**. The transient lag described in Finding 5 may contribute to small BESTEST deviations in hourly surface temperature outputs. Run a sensitivity check comparing HARES surface temperatures against EnergyPlus CTF for the BESTEST 600/610 light/mass wall cases to quantify the maximum deviation.

3. **Consider a higher-order approximation for surface temperature** if BESTEST hourly surface temperature outputs become a CI gating item. A simple extension would compute `T_surf` from the state derivative `dx/dt` at the innermost capacitor node, capturing the dT/dt lag. This is a minimal change to `stepping.rs:533` that would close the transient gap without restructuring the RC network.

## References / Citations

- OCHRE `BoundarySurface.__init__`: `vendors/OCHRE/ochre/Models/Envelope.py:254` — `self.radiation_frac = self.res_film / (self.res_film + res_material)`
- OCHRE `create_rc_data`: `vendors/OCHRE/ochre/utils/envelope.py:320-323` — resistance averaging producing half-resistances
- HARES `radiation_frac` interior: `crates/hares-core/src/dwelling/solver_builder.rs:279-281` — `r_film_int / (r_film_int + r_inner_half)`
- HARES `radiation_frac` exterior: `crates/hares-core/src/dwelling/solver_builder.rs:240` — `r_film_ext / (r_film_ext + r_outermost_half)`
- HARES `radiation_frac` window: `crates/hares-core/src/dwelling/solver_builder.rs:806-807` — EnergyPlus interior film decomposition
- HARES RC half-resistance: `crates/hares-envelope/src/boundary_rc.rs:1302,1326` — `thickness_m / (2.0 × k × area)`
- HARES Y-Δ elimination: `crates/hares-envelope/src/rc_network.rs:220-276` — `reduce_floating_nodes`
- E+ CTF surface temperature: `vendors/EnergyPlus/src/EnergyPlus/HeatBalanceSurfaceManager.cc:8021` — `TempDiv = 1/(CTFInside[0] − CTFCross[0] + HConv + damping)`
- E+ CTF coefficients: `vendors/EnergyPlus/src/EnergyPlus/Construction.cc:1028` — `CTFInside[0] = -s0(2,2) × CFU`
- E+ window film decomposition: `vendors/EnergyPlus/src/EnergyPlus/Construction.cc` — `r_interior = 1/(0.359073·ln(U) + 6.949915)`
- ISO 13786:2007 §6.2 — convergence of lumped-RC to CTF with increasing node count
- Walton (1983) NBSSIR 83-2655 — TARP natural convection model used for per-step h_nat recomputation
- ASHRAE HoF 2021 Ch. 4 — Convection and radiation in parallel from surface nodes
