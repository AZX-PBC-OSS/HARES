---
id: PARITY-015
title: "Interior LWR: Iterative non-linear T^4 solver"
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-envelope/src/longwave_radiation.rs
  - crates/hares-envelope/src/thermal_solver/mod.rs
  - crates/hares-envelope/src/thermal_solver/config.rs
references:
  - docs/equipment/ochre-parity-gaps.md (Gap 1)
  - vendors/OCHRE/ochre/models/Envelope.py (_solve_interior_radiation)
  - EnergyPlus Engineering Reference Ch. 3.5 (Interior Longwave Radiation Exchange)
verification:
  - cargo build --workspace
  - cargo test --workspace
  - cargo clippy --workspace -- -D warnings
---

## Prerequisites

THERMAL-002 (4-component exterior LWR) and THERMAL-004 (interior LWR energy balance tests) will have completed. THERMAL-002 improves exterior LWR in `longwave_radiation.rs`; THERMAL-004 adds baseline interior LWR tests. PARITY-001 (thermal solver split) will have extracted `thermal_solver/longwave.rs` as the landing zone for this ticket.

## Background/Context

After THERMAL-002/004, HARES has improved exterior LWR and baseline interior LWR tests. However, the interior model still uses a linearized `h_r = 4·ε·σ·T_avg³` approximation. EnergyPlus uses the iterative non-linear ScriptF method that converges surface temperatures each timestep. The linearized approach introduces error during transient conditions and in unconditioned spaces where surface-to-surface temperature deltas are large.

**Target**: EnergyPlus-grade ScriptF method (Hottel & Sarofim, "Radiative Transfer", Ch. 3, McGraw Hill 1967).

### EnergyPlus ScriptF Method (from Engineering Reference 25.1)

The radiation exchange between surfaces i and j is:
```
q_i,j = A_i * F_i,j * sigma * (T_i^4 - T_j^4)
```
where `F_i,j` is the ScriptF coefficient (grey interchange factor including all reflections).

**View factor approximation** (EnergyPlus method):
1. Determine total area "seen" by each surface (constraints: surfaces don't see themselves, surfaces within 10deg of opposing orientation don't see each other, all surfaces see floors/ceilings)
2. Approximate view factor from surface 1 to 2 as: `F_1→2 = A_2 / A_total_seen_by_1`
3. Fix reciprocity: `A_i * F_i→j = A_j * F_j→i`
4. Fix completeness: sum of view factors from each surface = 1.0

**For zones >= 4 surfaces**: iterative correction to enforce both reciprocity and completeness.
**For zones < 4 surfaces**: reciprocity enforced, completeness relaxed.

## Work to Do

- [ ] In `longwave_radiation.rs`, implement `interior_longwave_scriptf()`:
  - Accept per-surface emissivities, areas, orientations (tilt/azimuth), and current temperature estimates
  - Compute approximate view factors using EnergyPlus area-ratio method with orientation constraints
  - Apply reciprocity + completeness correction (iterative fixup for >= 4 surfaces)
  - Compute ScriptF grey interchange factors from view factors + emissivities
  - Iterate surface temperatures using heavy-ball damping (0.5x new + 0.1x momentum)
  - Convergence criterion: max surface temperature change < 0.01C
  - Max iterations: configurable, default 15
  - Return per-surface net radiation flux [W]
- [ ] In `thermal_solver/config.rs`, add `InteriorSurfaceInfo` struct:
  - `state_index: usize` — index into x vector for surface temperature
  - `radiation_frac: f64` — film-resistance weighting (1.0 = surface node, <1.0 = interpolate to zone air)
  - `area_m2: f64`, `emissivity: f64`
  - `zone: ZoneId`
- [ ] In `thermal_solver/mod.rs`, add `apply_interior_longwave_inputs()` method:
  - Extract current surface temperatures from state vector `x` using `InteriorSurfaceInfo.state_index`
  - Interpolate with zone air using `radiation_frac`
  - Call iterative solver
  - Inject per-surface LWR corrections into input vector `u`
- [ ] Wire into `resolve_internal()` between solar and port-sensible steps
- [ ] In `solver_builder.rs`, populate `InteriorSurfaceInfo` from building boundaries:
  - For each interior-facing surface, identify its outermost RC node index
  - Compute radiation_frac from film resistance ratio
- [ ] Retain `interior_longwave_linearised_w()` as fallback (behind config flag)
- [ ] Add unit tests: two-surface box (wall at 40°C, floor at 20°C), verify convergence and energy conservation

## Files to Touch

- `crates/hares-envelope/src/longwave_radiation.rs`: New iterative solver function
- `crates/hares-envelope/src/thermal_solver/config.rs`: New `InteriorSurfaceInfo` struct
- `crates/hares-envelope/src/thermal_solver/mod.rs`: Wire iterative LWR into resolve pipeline
- `crates/hares-core/src/dwelling/solver_builder.rs`: Populate interior surface info from building

## Measures of Success

- [ ] Iterative solver converges in <15 iterations for typical residential scenarios
- [ ] Energy conservation: sum of all surface radiation fluxes within a zone = 0 (within 0.1 W)
- [ ] Results diverge from linearized model by >1% for unconditioned spaces (proving the iterative solver adds value)
- [ ] No hot-path allocation (reuse surface temperature buffer)
- [ ] Existing tests continue to pass (linearized fallback available)

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
