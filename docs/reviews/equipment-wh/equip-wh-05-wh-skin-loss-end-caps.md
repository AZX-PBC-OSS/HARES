# Water heater tank end-cap skin loss correction
**Review ID**: equip-wh-05
**Category**: equipment-wh
**Date**: 2026-05-26

## Files Reviewed
crates/hares-equipment/src/water_heater/tank.rs

## Vendor/Reference Files Consulted
vendors/EnergyPlus/src/EnergyPlus/WaterThermalTanks.cc
vendors/OCHRE/ochre/Models/Water.py

## Findings

### Finding 1: Total UA exceeds specified `ua_w_per_k` input [Severity: high]
**Description**: The end-cap correction adds `ua_w_per_k * 0.1` to each boundary node (top and bottom), making the effective total UA larger than the user-specified `ua_w_per_k`. For a multi-node tank the total UA becomes `1.2 × ua_w_per_k`; for a single-node tank it becomes `1.1 × ua_w_per_k`. This means the `ua_w_per_k` input field labelled as the "jacket heat-loss coefficient" does not represent the actual total UA used in the simulation.

**Code Location**: `crates/hares-equipment/src/water_heater/tank.rs:158-167`
```rust
let ua_end = config.ua_end_cap_w_per_k.unwrap_or(config.ua_w_per_k * 0.1);
let mut ua_per_node: Vec<f64> = node_volumes_m3
    .iter()
    .map(|&v| config.ua_w_per_k * v / total_volume_m3)
    .collect();
ua_per_node[0] += ua_end;
let last = config.n_nodes - 1;
if last != 0 {
    ua_per_node[last] += ua_end;
}
```

**Root Cause**: The base per-node UA sums to `ua_w_per_k` (proportional to volume fraction), but the end-cap contributions are *added* on top without adjusting the base allocation downward. Neither EnergyPlus nor OCHRE adds end-cap loss on top of a pre-specified total UA; both compute the total UA from geometry and a per-unit-area loss coefficient.

**Impact**: 
- All standby losses are 20% higher (multi-node) or 10% higher (single-node) than a user would expect from reading the `ua_w_per_k` field.
- Tank energy consumption and ambient zone gains from skin losses are systematically overestimated.
- Parity comparisons with EnergyPlus or OCHRE will show higher standby loss in HARES for the same nominal UA input.

**Verification** (6-node tank with `ua_w_per_k = 4.0` W/K):
- Base per-node UA = 4.0 / 6 = 0.6667 W/K
- Top node UA = 0.6667 + 0.4 = 1.0667 W/K
- Bottom node UA = 0.6667 + 0.4 = 1.0667 W/K
- Interior nodes × 4 = 2.6667 W/K
- **Total effective UA = 4.8 W/K > 4.0 W/K (20% excess)**

### Finding 2: Fixed 0.1 multiplier ignores tank aspect ratio [Severity: medium]
**Description**: The end-cap correction uses a fixed multiplier `ua_w_per_k * 0.1` for *each* end cap, regardless of the tank's actual height-to-diameter aspect ratio. Both EnergyPlus and OCHRE compute end-cap UA from actual geometry (cross-sectional area × heat transfer coefficient).

**Code Location**: `crates/hares-equipment/src/water_heater/tank.rs:158`
```rust
let ua_end = config.ua_end_cap_w_per_k.unwrap_or(config.ua_w_per_k * 0.1);
```

**EnergyPlus approach** (`WaterThermalTanks.cc:6076-6102`):
EnergyPlus computes skin area for boundary nodes as `Perimeter_loc × NodeHeight + EndArea` (adding the flat end-cap surface area) and for interior nodes as `Perimeter_loc × NodeHeight`. The loss coefficient per node is then `SkinLossCoeff × SkinArea + AdditionalLossCoeff`. The end cap is handled geometrically: `EndArea = Volume / TankHeight` = π(D/2)².

**OCHRE approach** (`vendors/OCHRE/ochre/Models/Water.py:246-258`):
OCHRE derives a per-unit-area heat transfer coefficient `u = UA / total_area` (W/m²-K), then computes:
- `r_side_tot = 1 / (u × side_area)` — side resistance
- `r_top = 1 / (u × top_area)` — end-cap resistance

Boundary nodes combine side and end-cap resistances in parallel. The end-cap UA contribution is `u × π(D/2)²`, which is physically derived from the tank diameter.

**Root Cause**: The 0.1 factor approximates the geometric ratio `R / (2H)` for a single end cap, which is approximately 0.1 for a tank with H/D ≈ 2.4 (e.g., H=1.2 m, D=0.5 m). This is a reasonable approximation for "typical" residential tanks but breaks down for non-standard geometries:

| Tank type | H (m) | D (m) | H/D | True end-cap UA fraction per end | HARES fixed 0.1 | Error |
|-----------|-------|-------|-----|----------------------------------|-----------------|-------|
| Typical   | 1.2   | 0.5   | 2.4 | 0.094                            | 0.1             | +6%   |
| Tall/skinny | 2.0 | 0.3   | 6.7 | 0.036                            | 0.1             | +178% |
| Short/wide  | 0.8 | 0.9   | 0.9 | 0.220                            | 0.1             | -55%  |

**Impact**: 
- Tall/skinny tanks will have end-cap losses overestimated by ~2.8× per end cap.
- Short/wide tanks will have end-cap losses underestimated by ~2.2× per end cap.
- Both EnergyPlus and OCHRE handle this correctly through their geometry-derived approach.

### Finding 3: Single-node tank double-counting guard is correct [Severity: low]
**Description**: For single-node tanks, the `if last != 0` guard at `tank.rs:165` correctly prevents adding the end-cap UA twice to the same node. The single node receives exactly one end-cap increment. The test `end_cap_ua_single_node_added_once` at `tank.rs:1087-1098` validates this.

**Code Location**: `crates/hares-equipment/src/water_heater/tank.rs:164-167`
```rust
if last != 0 {
    ua_per_node[last] += ua_end;
}
```

**Impact**: Correct behaviour for 1-node tanks. The effective UA is `1.1 × ua_w_per_k`, matching the expectation that both end caps are present but apply to the same (only) node.

### Finding 4: Two-node tank end-cap allocation is consistent with OCHRE [Severity: low]
**Description**: For 2-node tanks, the top node (index 0) and bottom node (index 1) each receive the end-cap addition. With the 1/3-2/3 volume split, the UA distribution is:
- Top node: `ua_w_per_k × 1/3 + ua_end`
- Bottom node: `ua_w_per_k × 2/3 + ua_end`

Both boundary nodes get equal end-cap contribution, which aligns with OCHRE Water.py:257-258 where both `WH1` and the last node get parallel end-cap resistance.

**Impact**: Directionally correct; same total-UA-exceeds-specification issue from Finding 1 applies here too.

### Finding 5: No verification that total UA after correction sums to `ua_w_per_k` [Severity: medium]
**Description**: There is no test or runtime assertion that confirms the sum of all `ua_per_node` values equals or is otherwise related to the user-specified `ua_w_per_k`. The end-cap addition silently inflates the total UA beyond the input value.

**Code Location**: `crates/hares-equipment/src/water_heater/tank.rs:158-167` (no post-condition check exists)

**Root Cause**: The current design treats `ua_w_per_k` as a "side-wall UA" with end caps as additional parallel paths. If this is intentional, the field name and documentation should clarify that `ua_w_per_k` is the *side-wall jacket UA only* and that end-cap losses are extra.

**Impact**: Users cannot explicitly control the total UA; they must reverse-engineer the end-cap contribution or use `ua_end_cap_w_per_k` to override it.

### Finding 6: Code comment references misrepresent OCHRE and EnergyPlus behaviour [Severity: low]
**Description**: The comment at `tank.rs:156-157` states:
```
// OCHRE Water.py:250-258: top/bottom nodes get additional parallel UA for
// flat end caps. Reference: EnergyPlus Engineering Reference, §14.8.
```

However, neither OCHRE nor EnergyPlus uses the 0.1 multiplier approach. Both compute end-cap UA from tank geometry:
- **EnergyPlus** (`WaterThermalTanks.cc:6096-6102`): Adds `EndArea = Volume/TankHeight` to the skin area of boundary nodes, then applies the uniform `SkinLossCoeff` (W/m²-K).
- **OCHRE** (`Water.py:250-258`): Computes `u = UA/total_area` (W/m²-K), then uses `r_top = 1/(u × top_area)` as a separate parallel resistance.

The 0.1 factor appears to be a HARES-original simplification, not a direct port from either reference implementation.

**Code Location**: `crates/hares-equipment/src/water_heater/tank.rs:154-157`
```rust
// Build per-node UA values from the uniform volume-fraction allocation,
// then add end-cap UA to the top (index 0) and bottom (last index) nodes.
// OCHRE Water.py:250-258: top/bottom nodes get additional parallel UA for
// flat end caps. Reference: EnergyPlus Engineering Reference, §14.8.
```

**Impact**: Misleading provenance may cause maintainers to assume the implementation is faithful to the references when it is not. The comment should acknowledge that the 0.1 factor is an approximation calibrated for typical aspect ratios.

## Summary
- Total findings: 6
- Critical: 0
- High: 1
- Medium: 2
- Low: 3

## Recommendations

1. **Decide on the UA model semantics** (Finding 1, Finding 5): Clarify whether `ua_w_per_k` represents the total tank UA (including end caps) or the side-wall-only UA. If it is meant to be the total, the end-cap contribution should be allocated *within* the specified total (e.g., reduce the proportional share of interior nodes so the sum equals `ua_w_per_k`). If it is side-wall only, rename the field to `ua_side_wall_w_per_k` and clearly document that end caps are additive.

2. **Consider geometry-derived end-cap UA** (Finding 2): Replace or supplement the fixed 0.1 multiplier with a geometry-derived ratio using the tank's actual dimensions. Since both `height_m` and `diameter_m` are already available in `StratifiedTankConfig`, the end-cap fraction could be computed as `(π(D/2)²) / (2π(D/2)H + 2π(D/2)²)` = `D / (4H + 2D)`. This would match both EnergyPlus and OCHRE exactly. If retaining the 0.1 default, add a runtime warning when the tank aspect ratio causes the actual geometric fraction to deviate from 0.1 by more than 50%.

3. **Add a post-construction assertion** (Finding 5): After building `ua_per_node`, assert or at least `tracing::debug!` when `sum(ua_per_node) / ua_w_per_k` exceeds some threshold (e.g., 1.01), so discrepancies are visible.

4. **Update code comments** (Finding 6): Revise the comment at lines 154-157 to accurately describe the approach. For example:
   ```
   // Build per-node UA values. The total side-wall UA (ua_w_per_k) is allocated
   // proportionally to node volume fractions. Flat end-cap losses are then added
   // as parallel thermal paths. By default each end cap contributes ua_w_per_k * 0.1,
   // which approximates the geometric end-cap-to-sidewall area ratio for typical
   // residential tanks (H/D ≈ 2.4). For non-standard geometries, the optional
   // ua_end_cap_w_per_k can override this.
   // Reference: OCHRE Water.py:250-258 (geometry-derived parallel resistance);
   // EnergyPlus WaterThermalTanks.cc:6096-6102 (geometry-derived skin area addition).
   ```

5. **Add a geometric end-cap test** (Finding 2): Consider adding a test that computes the geometric end-cap fraction `r / (2h)` from tank dimensions and verifies that the default 0.1 factor is within a reasonable tolerance for standard residential tank geometries (e.g., the test helper's 1.2 m × 0.5 m tank yields ~0.094, within 10% of 0.1).

## References / Citations

- EnergyPlus `WaterThermalTanks.cc:6076-6102`: Node initialization for vertical cylinder tanks. Boundary nodes get `SkinArea = Perimeter_loc × NodeHeight + EndArea` where `EndArea = Volume / TankHeight`. Interior nodes get `SkinArea = Perimeter_loc × NodeHeight` only. `OnCycLossCoeff = SkinLossCoeff × SkinArea + AdditionalLossCoeff`.
- EnergyPlus `WaterThermalTanks.cc:3326-3333`: `AdditionalLossCoeff` is a user-specified per-node W/K additive that defaults to 0.
- OCHRE `Water.py:246-258`: Per-unit-area heat transfer coefficient `u = UA / total_area`. End-cap resistance `r_top = 1/(u × top_area)`. Boundary nodes combine side resistance (proportional to volume fraction) with end-cap resistance in parallel.
- HARES `tank.rs:154-167`: End-cap correction implementation.
- HARES tests `tank.rs:1060-1098`: `end_cap_ua_boundary_nodes_higher_than_interior` and `end_cap_ua_single_node_added_once`.
