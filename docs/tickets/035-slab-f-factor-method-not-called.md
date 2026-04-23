# Slab-on-Grade Uses Area-UA Conduction Instead of ASHRAE F-Factor Perimeter Method

**Severity**: High
**Priority**: P2
**Status**: Open
**Areas**: hares-physics/ground.rs, hares-envelope/boundary_rc.rs, hares-core/dwelling/conversions.rs

## Problem

ASHRAE Handbook of Fundamentals 2021, Ch. 18.31 mandates the perimeter
F-factor method for slab-on-grade heat loss:

```
Q_slab = F2 × P × (T_indoor - T_ground_surface)
```

where F2 [W/(m·K)] is a perimeter heat loss coefficient that accounts for the
three-dimensional heat flow around the slab edge, and P is the exposed
perimeter length. This method is the EnergyPlus default (Engineering Reference
§12.4) and the ASHRAE 90.1 compliance method.

The functions `slab_perimeter_loss_w` and `f2_coefficient` exist in
`hares-physics/src/ground.rs:98–150` and are correct. However, they are never
called during simulation. The slab boundary (`BoundaryType::Slab`) is assembled
as a flat conductive boundary by `build_layered_boundary` / `build_precomputed_boundary`
in `boundary_rc.rs` — identical to a wall — and connected to `ExteriorTarget::Ground`
with an area-weighted conductance (U × A_slab).

### Physics Error

Area-UA modeling of a slab computes:
```
Q_slab = U_slab × A_slab × (T_indoor - T_ground)
```
using the full floor area. This is incorrect because:

1. Heat loss through the slab center is negligible (ASHRAE §18.31: deep soil
   has near-constant temperature; only the perimeter is thermally active).
2. Area-UA ignores the three-dimensional edge conduction that dominates slab
   heat loss.
3. U_slab for a concrete slab without insulation is high (~1.0–1.5 W/m²·K);
   multiplied by the full floor area this dramatically overstates heat loss.

For a 140 m² (1,500 ft²) slab with P = 50 m perimeter, F2 = 1.17 W/(m·K),
and ΔT = 15°C:
- Correct (F-factor): Q = 1.17 × 50 × 15 = 878 W
- Incorrect (area-UA at U=1.0): Q = 1.0 × 140 × 15 = 2,100 W — 2.4× too high.

## Evidence

```
crates/hares-physics/src/ground.rs:98–150
    pub fn slab_perimeter_loss_w(...) -> f64 { ... }  // ← correct but never called
    pub fn f2_coefficient(insulation_r_m2_k_w: f64) -> f64 { ... }  // ← correct but never called

crates/hares-envelope/src/boundary_rc.rs:446–453
    ExteriorTarget::Ground => ground_node,  // ← slab uses same path as wall
```

No caller in `solver_builder.rs`, `conversions.rs`, or `boundary_rc.rs`
invokes `slab_perimeter_loss_w` or `f2_coefficient`.

## HPXML Geometry Available

HPXML provides slab perimeter geometry:
- `<Perimeter>` element under `<Slab>` (explicit perimeter length [ft]).
- If absent: derive from `sqrt(area) × 4` for square approximation.
- `<PerimeterInsulation><Layer><InstallationType>` and `<NominalRValue>` for
  insulation depth/R-value needed to select F2 coefficient.

## Required Behavior

Per ASHRAE HoF 2021, Ch. 18 §31 and EnergyPlus Engineering Reference §12.4,
slab-on-grade heat loss must use the F-factor perimeter method:

```
Q_slab = F2 × P × (T_indoor - T_ground_surface)
```

where F2 [W/(m·K)] is selected from ANSI/ASHRAE 90.1-2022, Table A6.3.1
based on insulation depth and R-value. The area-UA method must not be used.

## Approach

1. Parse `<Perimeter>` from HPXML slab elements in `building.rs`; store as
   `perimeter_m: f64` on `Boundary`. If absent, derive from `4 × sqrt(area_m2)`
   as a square-plan approximation (no silent failure — log a warning).
2. Parse `<PerimeterInsulationDepth>` and `<PerimeterInsulation><Layer><NominalRValue>`
   to select the F2 coefficient via `f2_coefficient(insulation_r_m2_k_w)`.
3. In `solver_builder.rs`, for `BoundaryType::Slab`, call `slab_perimeter_loss_w`
   to produce a fixed conductance `G = F2 × P` [W/K] connected between the
   slab interior node and the ground node.
4. Retain slab thermal mass as a capacitance layer (concrete thickness × density × Cp).
5. Remove the area-UA resistor for the slab path in `boundary_rc.rs:446–453`.

## Definition of Done

- [ ] `slab_perimeter_loss_w` and `f2_coefficient` are called from the solver
      boundary construction path for `BoundaryType::Slab`.
- [ ] The area-UA resistor from slab interior to ground node is removed and
      replaced with a fixed conductance `G = F2 × P` [W/K].
- [ ] Slab thermal mass (concrete capacitance) is retained as a separate layer.
- [ ] Test: 140 m² slab, P=50 m, uninsulated F2≈1.17 W/(m·K), ΔT=15°C →
      Q ≈ 878 W (tolerance ±5%).
- [ ] Test: insulated slab (R-5 perimeter) produces lower Q than uninsulated
      at same ΔT.

## Verification

```bash
cargo test -p hares-physics slab_perimeter_loss
cargo test -p hares-envelope slab
```

Expected: `slab_perimeter_loss_w(1.17, 50.0, 15.0)` ≈ 878 W.

## References

- ASHRAE Handbook of Fundamentals 2021, Ch. 18 §31 (Slab-on-Grade Floors —
  F-factor perimeter method).
- ANSI/ASHRAE/IES 90.1-2022, Table A6.3.1 (Slab F-factors by climate zone and
  insulation depth).
- EnergyPlus Engineering Reference §12.4 (Slab-on-Grade Heat Transfer —
  F-factor method as default).
- OCHRE `utils/envelope.py`: F-factor pre-integrated into `SlabFloor` RC layer
  stack; heat loss dominated by perimeter nodes.
