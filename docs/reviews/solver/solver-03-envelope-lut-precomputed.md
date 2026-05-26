# Envelope LUT precomputed RC path: correctness and performance
**Review ID**: solver-03
**Category**: solver
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-io/src/envelope_lut.rs`
- `crates/hares-envelope/src/boundary_rc.rs`

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/utils/envelope.py` — `get_boundary_rc_values()`, `create_rc_data()`
- `vendors/OCHRE/ochre/Models/Envelope.py` — Boundary `__init__` RC wiring

## Findings

### Finding 1: [Severity: HIGH] Same-zone precomputed path removes wrong end of resistor chain vs OCHRE reference
**Description**: In `build_precomputed_boundary()`, the same-zone Step 4 removes the *first* resistor from the post-averaging chain (`res_list.remove(0)` at line 1411). OCHRE's `create_rc_data()` removes the *last* resistor (`res_list = res_list[:-1]` at envelope.py:337). This means the RC chain connects to the zone air node through the interior-most half-resistance rather than the cut-surface half-resistance. Additionally, Step 1 splits the layer list differently: HARES keeps the *last* (interior-facing) half via `split_off()` (lines 1367–1373), while OCHRE keeps the *first* (exterior-facing) half via `[:new_nodes]` (lines 312–314). OCHRE's Boundary class then *reverses* the node and resistor lists (`[::-1]` at Envelope.py:404-405) to make the cut-surface layer closest to the zone.

**Code Location**:
- `boundary_rc.rs:1363-1377` (Step 1 — splits opposite half vs OCHRE)
- `boundary_rc.rs:1410-1412` (Step 4 — removes `res_list[0]` instead of OCHRE's `res_list[-1]` / `res_list[:-1]`)
- `envelope.py:312-318` (OCHRE Step 1 — keeps first half)
- `envelope.py:336-337` (OCHRE Step 6 — removes last resistor)
- `Envelope.py:402-405` (OCHRE boundary reversal for same-zone)

**Root Cause**: The `build_precomputed_boundary` doc comment states it "Implements OCHRE's `create_rc_data` algorithm" but both the half-selection direction and the resistor-removal end are inverted. The material-layer path (`build_layered_boundary`) keeps the interior half (consistent with HARES's comment about keeping "the inner half") and wires layer[0] (the cut surface) to the zone via the exterior wiring path (line 1273), which is **not** gated on `same_zone`. This means the material path and precomputed path produce different topologies for identical same-zone boundaries.

**Impact**: For same-zone boundaries (internal mass, party walls, furniture elements), the thermal mass layers appear in reverse order relative to the zone air node compared to both OCHRE and HARES's own material-layer path. With typical layer stacks (e.g., gypsum nearest interior, insulation nearest exterior), the precomputed path places gypsum at the dead-end of the chain while the material path places it adjacent to the zone. This changes the effective RC time-constant spectrum, altering transient thermal response. The total steady-state UA is unaffected, but dynamic heat storage/release timing is shifted.

**Comparison with OCHRE**: OCHRE's `create_rc_data` keeps the first half of the layer list and removes the last resistor. The Boundary `__init__` then reverses the node and resistor lists for same-zone boundaries, so the final chain order is correct: cut-surface layer appears closest to the zone node. HARES diverges at both steps.

**Test gap**: None of the precomputed-path tests (lines 2212–2488) use `same_zone=true`. All precomputed tests use outdoor or inter-zone exterior targets (`ExteriorTarget::Outdoor` or `ExteriorTarget::Zone(i)` where `i != interior_zone_idx`). The same-zone codepath in `build_precomputed_boundary` is entirely uncovered.

### Finding 2: [Severity: MEDIUM] Nearest-neighbor LUT matching produces step discontinuities; no interpolation
**Description**: The `EnvelopeLookup::lookup()` method selects the closest matching construction variant using minimum absolute R-value distance (lines 210-215 in `envelope_lut.rs`). This is nearest-neighbor selection, not interpolation. When a building's assembly R-value falls between two LUT grid points (e.g., between R-30 and R-38 attic floor), the RC parameters jump discontinuously at the midpoint between the two grid R-values. There is no linear, bilinear, or multilinear interpolation of the per-layer R and C values.

**Code Location**: `envelope_lut.rs:207-215`

**Root Cause**: The LUT is pre-loaded as a discrete set of construction variants (e.g., R-13, R-19, R-30, R-38, R-49, R-60 for Attic Floor). The matching algorithm selects the single closest row with no blending of RC values from adjacent grid points. OCHRE uses the same nearest-neighbor approach (`envelope.py:257-258`), so this is a known limitation of the OCHRE-derived LUT data model rather than a HARES-specific bug.

**Impact**: Step discontinuities at grid boundaries cause physically unrealistic abrupt changes in building thermal performance across small changes in input R-value. For a parameter sweep study, two houses differing by 0.1 in assembly R-value could produce measurably different RC parameters if they happen to straddle a LUT grid boundary. The magnitude of the jump depends on the R-value gap between adjacent grid points, which for some construction types is as large as R-11 (attic floor R-19 → R-30).

### Finding 3: [Severity: MEDIUM] Validation assertions use `debug_assert!` — elided in release builds
**Description**: The precomputed path validates R_total > 0 and capacitance >= 0 exclusively via `debug_assert!` macros at lines 577-580 and 586-589. In Rust release builds (`--release`), `debug_assert!` is compiled away, meaning corrupted or invalid LUT data (e.g., negative capacitances from a damaged CSV) would silently produce non-physical RC networks with zero or negative node capacitances. The subsequent `RCNetwork::from_elements()` call at line 1051 may or may not catch these depending on how the matrices singularise. The material-layer path has the same `debug_assert!` pattern (lines 691-694).

**Code Location**:
- `boundary_rc.rs:577-580` — `debug_assert!(r_total > 0.0, ...)`
- `boundary_rc.rs:586-589` — `debug_assert!(cap_total >= 0.0, ...)`
- `boundary_rc.rs:691-694` — same for material path
- `boundary_rc.rs:750-753` — same for fallback path

**Root Cause**: The assertions guard against physically impossible values but are not present in release mode. Individual layer R and C values from the LUT are directly used without per-layer positivity checks at the lookup level (`envelope_lut.rs:227-233`). The `MIN_CAPACITANCE_J_K` clamp at line 1424 provides a floor but cannot detect a negative-capacitance LUT entry that exceeds `MIN_CAPACITANCE_J_K` in magnitude.

**Impact**: If a damaged or incorrectly formatted LUT CSV is loaded, release-mode simulations could silently produce incorrect results. This is low-probability (static CSV data) but high-impact (undetected model error).

**Comparison with OCHRE**: OCHRE's Python code uses `assert` statements in `create_rc_data` (line 333: `assert len(cap_list) == nodes`), which are also elided when Python runs with `-O` optimization. Both codebases share this fragility.

### Finding 4: [Severity: LOW] LUT matching uses OCHRE fixed film R-values; actual RC network uses TARP-computed film R-values
**Description**: The LUT matching algorithm uses OCHRE's legacy fixed film resistances for R-value comparison: `FILM_R_FLOOR_CEILING = 0.2642` (1.5 IP) and `FILM_R_WALL_ROOF = 0.1585` (0.9 IP) at `envelope_lut.rs:91-93`. These are added to the CSV `Assembly R Value` to compute `Boundary R Value` for closest-match selection (line 201 / line 211). However, the actual RC network construction uses TARP-computed film resistances from `BoundaryInput::r_film_interior_m2_k_w` and `r_film_exterior_m2_k_w`, which vary with surface tilt, zone temperatures, and wind speed. The selected LUT variant is therefore matched against a total R-value that may differ from the actual total R-value in the constructed network.

**Code Location**:
- `envelope_lut.rs:91-93` — fixed film R constants
- `envelope_lut.rs:201-213` — film R added to assembly R for matching
- `conversions.rs:144-150` — TARP film R subtracted from HPXML AssemblyEffectiveRValue

**Root Cause**: The LUT matching uses a different film R convention than the RC construction. The mismatch is bounded — OCHRE's 0.1585 SI for walls corresponds to h_si ≈ 6.31 W/(m²·K), while TARP-computed interior film resistances typically range from 0.12–0.18 m²·K/W for interior vertical surfaces. The error is typically <15% in film resistance, which translates to <5% in total R for most assemblies.

**Impact**: For boundaries with very low assembly R-values (e.g., uninsulated walls where film R is a significant fraction of total R), the matched variant may differ from the variant that would be selected using the actual TARP film R. The numeric effect is small for typical residential construction.

### Finding 5: [Severity: LOW] No numerical equivalence verification between LUT and full computation paths
**Description**: There are no tests that verify LUT-generated RC parameters produce equivalent results to the full material-layer computation for comparable assemblies. The LUT values are sourced from OCHRE's CSV material database, which was generated by aggregating raw material properties into per-layer R and C values. There is no runtime cross-validation that the LUT path's RC network matches the material-path's RC network for an equivalent layer stack. The two paths use fundamentally different data sources (pre-computed aggregate properties vs. thickness/conductivity/density/cp), so equivalence cannot be assumed.

**Code Location**: Test suite at `boundary_rc.rs:2188-2488` tests the precomputed path in isolation; `envelope_lut_tests.rs` tests LUT loading and matching but not numerical RC equivalence.

**Root Cause**: The LUT data sources and the raw-material computation path are independent. The LUT values represent OCHRE's pre-derived effective layer properties, which may bake in effects (e.g., framing bridging, air films in aggregate, non-uniform material properties) that the raw material path does not. There is no requirement that the two paths produce numerically identical output.

**Impact**: A parameter sensitivity analysis or regression test comparing a building modeled with explicit material layers vs. LUT-looked-up layers could show non-trivial differences. This is a documentation/verification gap rather than a correctness bug.

### Finding 6: [Severity: LOW] R-value clamping at R ≥ 17.6 m²·K/W (100 IP) bypasses progressive construction-type filtering
**Description**: When the assembly R-value is ≥17.6 m²·K/W, the lookup resets the candidate filter back to all rows for the boundary name and hardcodes `r_val = 88.0` (lines 196-199 in `envelope_lut.rs`). This intentionally bypasses any construction-type or finish-type filtering already applied, so the "Minimal" row (which has no construction/finish type constraints) can be reached. While this matches OCHRE's intent (envelope.py:217-222, where `r_value ≥ 100` IP triggers reset), it means a caller providing `construction_type="WoodStud"` with `assembly_r_value=20.0` will silently ignore the "WoodStud" constraint and match the generic "Minimal" row.

**Code Location**: `envelope_lut.rs:196-199`

**Root Cause**: OCHRE's design uses very high R-values as a flag for "minimal building insulation" scenarios. The filter reset is necessary because "Minimal" rows in the CSV have empty construction/finish/insulation fields and would otherwise be excluded by progressive filtering. However, this creates a silent override that could surprise callers.

**Impact**: Low — this is an edge case for extremely well-insulated buildings and is consistent with OCHRE's behavior. The test at `envelope_lut_tests.rs:308-325` confirms the behavior is intentional. The edge case of R-values just above 17.6 m²·K/W (100 IP) receiving the maximal R-value construction (88 m²·K/W, ~500 IP) can be a significant over-estimate of insulation effectiveness for buildings in the R-18 to R-30 range (IP).

## Summary
- Total findings: 6
- Critical: 0
- High: 1
- Medium: 2
- Low: 3

## Recommendations
1. **Fix the same-zone resistor removal in `build_precomputed_boundary`**: Change `res_list.remove(0)` to `res_list.pop()` (or `res_list.truncate(res_list.len() - 1)`), and align the half-selection with either OCHRE's convention (first half) or HARES's material-path convention (last half), making them consistent. Add test coverage for `same_zone=true` with the precomputed path.
2. **Consider adding linear interpolation** for R-value parameters between adjacent grid points in the LUT, blending per-layer R and C values proportionally to distance from each grid point. This would eliminate step discontinuities at grid boundaries.
3. **Replace `debug_assert!` with runtime checks**: Convert the R_total and capacitance assertions to `if !condition { return Err(...) }` or use `assert!` (which is retained in release builds) so that invalid LUT data is always detected.
4. **Document the film R mismatch**: Add a comment in `envelope_lut.rs` noting that the fixed film R values are used only for LUT matching (selecting the closest construction variant) and that the actual RC network uses TARP-computed film resistances.
5. **Add cross-verification test**: For at least one representative boundary type, construct the RC network via both the LUT path (using the CSV data) and the raw material path (using thickness/conductivity/density from the same CSV rows), and verify that total R and total C agree within a documented tolerance.
6. **Add same-zone precomputed test**: Create a test case with `ExteriorTarget::Zone(i)` where `i == interior_zone_idx` (or use the same node for exterior and interior) to exercise the same-zone codepath in `build_precomputed_boundary`.

## References / Citations
- OCHRE `create_rc_data()`: `vendors/OCHRE/ochre/utils/envelope.py:294-339`
- OCHRE Boundary `__init__` same-zone reversal: `vendors/OCHRE/ochre/Models/Envelope.py:402-405`
- HARES precomputed boundary builder: `crates/hares-envelope/src/boundary_rc.rs:1337-1492`
- HARES LUT lookup matching: `crates/hares-io/src/envelope_lut.rs:128-240`
- HARES conversions LUT invocation: `crates/hares-core/src/dwelling/conversions.rs:210-260`
- OCHRE LUT matching: `vendors/OCHRE/ochre/utils/envelope.py:188-291`
- Test gap (no same-zone precomputed tests): `boundary_rc.rs:2188-2488`
