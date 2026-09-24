# Interior film coefficients frozen at initialization (TARP convention)
**Review ID**: envelope-02
**Category**: envelope
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-envelope/src/thermal_solver/config.rs`
- `crates/hares-envelope/src/thermal_solver/mod.rs`
- `crates/hares-physics/src/film_coefficients.rs`

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Models/Envelope.py`
- `vendors/OCHRE/ochre/utils/envelope.py:342-402`
- `vendors/EnergyPlus/src/EnergyPlus/ConvectionCoefficients.cc:1902-1969`
- `vendors/EnergyPlus/src/EnergyPlus/ConvectionCoefficients.hh:394-456`

## Findings

### Finding 1: [Severity: high]
**Description**: Interior convective film coefficients (`h_ci`) are computed once at building initialization using annual-average temperatures and the ASHRAE "Simple" algorithm (fixed `h_conv` by surface orientation), then permanently baked into the A-matrix conductances. The coefficients are never updated during the simulation as surface-to-air ΔT evolves.

**Code Location**:
- `crates/hares-physics/src/film_coefficients.rs:209-225` — `ashrae_simple_interior_h_conv` returns **fixed values** by tilt angle only (e.g. 3.076 W/(m²·K) for vertical walls). The `_t_ext_c` and `_t_int_c` parameters are underscore-prefixed dead parameters, confirming no ΔT dependence.
- `crates/hares-core/src/dwelling/conversions.rs:130-142` — `film_resistances()` is called once per boundary during `building_to_boundary_inputs()`, using annual-average `avg_wind_m_s`, `avg_ground_c`, `avg_ambient_c`.
- `crates/hares-envelope/src/boundary_rc.rs:534-544` — The resulting `r_film_interior_m2_k_w` is stored in `BoundaryParams` and folded into the RC resistor graph.
- `crates/hares-envelope/src/boundary_rc.rs:1431-1445` — Film resistances are added to the first/last layer resistors in the conduction chain.
- `crates/hares-envelope/src/boundary_rc.rs:1287-1320` — In StarMesh mode, the interior film is split as a separate convection-only resistor between the surface node and the zone air node.
- `crates/hares-envelope/src/rc_network.rs:139-178` — `build_matrices()` writes the A-matrix with `a_c[(i, i)] -= 1/(R_ij * C_i)` terms where `R_ij` includes the baked-in film resistance. The A-matrix is **never recomputed** during simulation.

**Root Cause**: HARES intentionally mirrors OCHRE's initialization-time snapshot architecture. OCHRE also calls `calculate_film_resistances()` once per boundary at `__init__` (Envelope.py:348-349) and bakes `res_film` into the static RC resistor dictionary (Envelope.py:395-401). Both HARES and OCHRE diverge from EnergyPlus, which recomputes `h_conv` every timestep via `CalcASHRAEDetailedIntConvCoeff` (ConvectionCoefficients.cc:1950-1969) using the **actual** surface and zone-air temperatures with the full ΔT-dependent TARP formula.

**Impact**:
- **Quantitative**: For a vertical surface, the frozen ASHRAE Simple value of 3.076 W/(m²·K) overestimates convective coupling by ~135% at ΔT = 1°C (TARP gives ~1.31), and underestimates by ~6% at ΔT = 11°C (TARP gives ~2.88). The cubic-root dependence means the divergence grows rapidly at low ΔT.
- **Surface-dependent**: Surfaces with strong solar gain (windows adjacent walls, sun-exposed floors) settle at near-zero surface-to-air ΔT in steady-state, where the frozen coefficient gives the greatest bias (100%+ overestimate of convective heat transfer). Surfaces with large temperature swings (poorly insulated walls, attics) see the sign flip depending on whether ΔT is above or below the init-time assumed value.
- **Systemic**: Not merely a telemetry error — the frozen film resistance is an active participant in the A-matrix conductance terms. Zone-air energy balance, surface node temperatures, and radiation_frac-derived interior surface temperatures are all influenced by the fixed film conductance. A zone that transitions from heating (high ΔT, h_conv ≈ 2.8) to near-steady-state (low ΔT, h_conv ≈ 1.31) carries a ~2× error in the film-side conduction path.

### Finding 2: [Severity: medium]
**Description**: A per-step TARP diagnostic-only channel has been added as a partial fix, but it is gated behind debug builds and does not affect the actual heat-balance solution.

**Code Location**:
- `crates/hares-envelope/src/thermal_solver/config.rs:27-32` — Comment explicitly acknowledges: *"the A-matrix conductance still uses the frozen init-time film resistance; this diagnostic-only fix reports the physically correct convective flux without changing the state-space discretization."*
- `crates/hares-envelope/src/thermal_solver/config.rs:38-39` — `BoundaryDiagnosticInfo::RCNode` computes per-step TARP `h_conv` for `Q = h_tarp(ΔT) × area × (T_surface - T_zone)`, labeled *"[per-step TARP, not frozen R_film]"*.
- `crates/hares-envelope/src/thermal_solver/stepping.rs:471` — The diagnostic path is wrapped in `#[cfg(any(debug_assertions, feature = "observe_detailed"))]` — **compiled out in release builds**.
- `crates/hares-envelope/src/thermal_solver/stepping.rs:503-511` — TARP computation: `tarp_h_natural(*tilt_deg, delta_t_clamped, above_hotter)` with comment confirming *"the A-matrix conductance is still frozen"*.

**Root Cause**: The diagnostic channel was a deliberately scoped partial fix (see ticket T-0082) to report physically correct convective flux in telemetry without undertaking the invasive state-space matrix reassembly required for a complete fix.

**Impact**: In debug/observed builds, `EnvelopeComponentGains.wall_heat_gain_w`, `.floor_heat_gain_w`, etc. report physically correct convective gains. In release builds, these per-component gains are unavailable and the built-in thermal solution still uses the frozen coefficients. The diagnostic channel proves the fix is feasible — `tarp_h_natural` is already available in `hares_physics::film_coefficients` — but it doesn't influence the actual zone temperature evolution.

### Finding 3: [Severity: low]
**Description**: OCHRE applies a 12.9°C ΔT floor to its TARP computation, while HARES uses the ASHRAE Simple orientation-only model (no ΔT). The two models converge at the same anchor point (3.076 W/(m²·K) for vertical surfaces at ~12.9°C ΔT), but diverge at other ΔT values.

**Code Location**:
- `crates/hares-physics/src/film_coefficients.rs:209-225` — ASHRAE Simple returns fixed h_conv by orientation only.
- `crates/hares-physics/src/film_coefficients.rs:171-183` — `tarp_h_natural` correctly implements the ΔT^(1/3) EnergyPlus formulas (Eqs. 90-92) but is only used for the exterior convection path and the diagnostic channel.
- `vendors/OCHRE/ochre/utils/envelope.py:374` — `delta_t = max(12.9, abs(t_ext_zone - t_int_zone))` forces a minimum 12.9°C ΔT, preventing the divide-by-zero/h_conv=0 problem at ΔT=0.
- `vendors/OCHRE/ochre/utils/envelope.py:376-388` — OCHRE uses the same TARP formulas but with a large ΔT floor.
- `crates/hares-physics/src/film_coefficients.rs:498` — The stepping.rs diagnostic path uses `delta_t_clamped = delta_t_k.max(0.1)` (0.1 K floor), matching EnergyPlus's LowHConvLimit philosophy rather than OCHRE's 12.9°C anchor.

**Root Cause**: HARES adopted the ASHRAE Simple model (E+ default for backward compatibility) while OCHRE adopted the TARP model frozen at a 12.9°C anchor. The ticket T-0101 documents this model discrepancy and recommends switching to per-step TARP as a model upgrade, not merely a timing fix.

**Impact**: Corner 12.9°C ΔT, HARES and OCHRE agree (3.076 ≈ 3.0759). Away from this point, disagreement reflects the model-level difference (constant vs. cubic-root) rather than a defect in either implementation. A per-step TARP fix would align HARES with EnergyPlus and make the OCHRE divergence intentional and documented.

## Summary
- Total findings: 3
- Critical / High / Medium / Low: 0 / 1 / 1 / 1

## Recommendations

1. **Prioritize per-step TARP in the conduction path (A-matrix).** The diagnostic-only channel demonstrates feasibility. Replace the frozen `r_film_int` in the RC resistor graph with a per-step `r_film_int(t)` computed from `1 / tarp_h_natural(tilt, T_surface(t-Δt), T_zone(t-Δt))`. This requires one of: (a) partial A-matrix reassembly each step targeting only the film-resistance rows/columns, or (b) moving film resistance out of the A-matrix into explicit forcing terms updated per-step. Approach (b) is architecturally cleaner: treat the interior film convection as a per-step `u[zone_air] += h_conv * area * (T_surface - T_zone)` injection rather than a permanent conductance.

2. **Remove the `#[cfg(debug_assertions)]` gate** from the boundary diagnostics path so the per-step TARP values are always computed. The computation is ~3 floating-point operations per surface per timestep (a `cbrt()` and a few multiplies) — negligible cost and essential for production observability.

3. **Document the HARES/OCHRE divergence explicitly** in the `film_coefficients.rs` module docs once per-step TARP is implemented: HARES follows EnergyPlus (per-step TARP, 0.1 K ΔT floor) while OCHRE freezes at 12.9°C ΔT. This prevents future confusion about why the two codes produce different film coefficients.

4. **Retain the ASHRAE Simple path as a configurable option** for run-mode comparison and backward compatibility with any studies that depend on the current behavior, but default to per-step TARP.

## References / Citations
- EnergyPlus ConvectionCoefficients.cc `CalcASHRAEDetailedIntConvCoeff` (lines 1950-1969): recomputes TARP h_conv each timestep using actual `(T_surface, T_zone_air)`.
- EnergyPlus Engineering Reference, "Interior Convection / TARP Algorithm", Eqs. 90-92: `h = 1.31|ΔT|^(1/3)` (vertical), `h = 9.482|ΔT|^(1/3) / (7.238 − |cosΣ|)` (enhanced), `h = 1.810|ΔT|^(1/3) / (1.382 + |cosΣ|)` (reduced).
- Walton, G. N. 1983. *Thermal Analysis Research Program Reference Manual*, NBSSIR 83-2655, National Bureau of Standards.
- OCHRE `calculate_film_resistances` (`vendors/OCHRE/ochre/utils/envelope.py:342-402`): freezes film resistances at init with 12.9°C ΔT floor.
- OCHRE `BoundarySurface.__init__` (`vendors/OCHRE/ochre/Models/Envelope.py:232-249`): stores `res_film` once at construction; never recomputed.
- HARES ticket T-0101 (`docs/tickets/101-interior-film-coefficients-recompute-per-step.md`): documents the defect, S1 partial fix, and proposed full repair path.
