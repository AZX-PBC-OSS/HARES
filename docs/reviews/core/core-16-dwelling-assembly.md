# Dwelling assembly from HPXML: missing defaults and validation
**Review ID**: core-16
**Category**: core
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-core/src/dwelling/mod.rs` (orchestrator and assembly entry)
- `crates/hares-core/src/dwelling/conversions.rs` (zone/boundary conversion to RC inputs)
- `crates/hares-core/src/dwelling/solver_builder.rs` (solver construction and infiltration defaults)
- `crates/hares-io/src/hpxml/mod.rs` (HPXML parse entry and error types)
- `crates/hares-io/src/hpxml/building.rs` (HPXML geometry/zone/boundary parser)
- `crates/hares-io/src/hpxml/validation.rs` (range and schema validation)

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/utils/hpxml.py`

## Findings

### Finding 1: [Severity: high]
**Description**: When HPXML omits a `<FoundationType>` element or has no `<Foundations>` group, HARES creates no Foundation thermal zone. Without an explicit foundation definition, the building defaults to having no foundation zone at all — not slab-on-grade, not crawlspace, not basement. This means zero foundation thermal mass participates in the RC network. OCHRE raises `OCHREException` for unknown foundation types, forcing explicit configuration. If a user uploads an HPXML file without foundation elements, HARES silently runs with a fundamentally different thermal model than intended.

**Code Location**: `crates/hares-io/src/hpxml/building.rs:507-535` (foundation_name derivation) and `building.rs:1727-1755` (zone creation from `<Foundations>`)

**Root Cause**: The foundation_name derivation at lines 507-535 matches only specific child tags (`Crawlspace`, `Basement`, `SlabOnGrade`, `Ambient`, `AboveApartment`). When `<FoundationType>` is absent or matches none of these, `foundation_name` is `None` and no Foundation zone is created via `build_zone_map`. OCHRE (hpxml.py:272-290) explicitly asserts on unknown foundation types and manages `foundation_name = None` only for slab-on-grade/pier-and-beam (where no foundation zone is correct).

**Impact**: A building whose HPXML omits the foundation gets modeled with ground contact through Slab boundaries only, missing the foundation wall thermal mass, below-grade zone air buffering, and foundation-specific infiltration paths. Heat loss through the building floor is underestimated, particularly for basements which add significant conditioned-zone-adjacent thermal buffering.

**Comparison with OCHRE**: OCHRE hpxml.py:274-290:
```python
foundations = list(enclosure.get("Foundations", {}).values())
if not foundations:
    foundation = None
    foundation_name = None
else:
    assert len(foundations) == 1
    foundation = foundations[0]
    foundation_type = foundation.get("FoundationType")
    ...
    else:
        raise OCHREException(f"Unknown foundation type: {foundation_type}")
```
When foundations are absent entirely, OCHRE also creates no zone — consistent. But for unknown foundation types inside an existing `<Foundations>` group, OCHRE errors; HARES silently drops the foundation zone.

### Finding 2: [Severity: high]
**Description**: When an attic zone is created via `ensure_referenced_zones_exist` (because a boundary references it but no `<Attics>` group exists in the HPXML), the zone defaults to `vented: false`. However, when the attic zone is created through the normal `<Attics>` path in `build_zone_map`, the default is `vented: true` (line 1691). This inconsistency means an attic zone that exists only because a roof boundary happens to reference "attic vented" will be modeled as unvented, with sub-0.1 ACH infiltration instead of the physically correct vented-attic infiltration.

**Code Location**: `crates/hares-io/src/hpxml/building.rs:1672-1706` (build_zone_map), `building.rs:1761-1788` (ensure_referenced_zones_exist), `crates/hares-core/src/dwelling/solver_builder.rs:1293-1311` (attic_infiltration vented default)

**Root Cause**: `build_zone_map` uses `or_insert` semantics (line 1695), so if `ensure_referenced_zones_exist` already created the zone with `vented: false`, the later `build_zone_map` step does not override it. The zone created by `ensure_referenced_zones_exist` at line 1770 always has `vented: false`.

**Impact**: Vented attics modeled as unvented miss the passive ventilation path entirely, which can overstate attic temperatures by 5-15°C during summer, inflating ceiling heat gain into the conditioned zone and producing biased cooling loads. The attic infiltration default for an unvented attic is 0.1 ACH; for a vented attic, the default SLA=0.00333 produces significantly more ventilation.

### Finding 3: [Severity: medium]
**Description**: When a boundary has no R-value whatsoever (no `AssemblyEffectiveRValue`, no `<NominalRValue>` layers, and no envelope LUT match), `building_to_boundary_inputs` silently applies `DEFAULT_R_M2_K_W = 2.5` m²·K/W (equivalent to R-14 IP). This is non-trivial insulation for an unspecified wall assembly; an uninsulated framed wall is approximately R-1 to R-2 IP. The 14× overestimate of R-value systematically understates envelope heat loss when HPXML omits construction details.

**Code Location**: `crates/hares-core/src/dwelling/conversions.rs:20` and `conversions.rs:148-157`

**Root Cause**: The fallback path at line 155-156:
```rust
.unwrap_or(DEFAULT_R_M2_K_W)
```
applies the constant for any boundary where both `assembly_r_value_m2_k_w` is `None` AND `r_value_layers_m2_k_w` sums to zero. This includes the case where HPXML specifies no insulation information at all. No warning is emitted to flag the missing R-value. Compare with the window U-factor path, which returns an error at solver-builder time when U-factor is missing (solver_builder.rs:291-297).

**Impact**: Dwellings with uninsulated or minimally specified walls receive 2.5 m²·K/W thermal resistance by default, systematically understating conduction losses through those boundaries. For a 100 m² uninsulated wall at 20°C ΔT, this means ~800 W loss vs ~2000 W actual — a 60% underestimation.

### Finding 4: [Severity: medium]
**Description**: Foundation zone `vented` status defaults to `false` for all foundation types in `build_zone_map` (line 1740). OCHRE defaults crawlspaces to `vented: true` (hpxml.py:689). When HPXML specifies `<Crawlspace>` without explicit `Vented` status, HARES models it as unvented, applying 0.0 ACH infiltration instead of the ResStock default of 2.0 ACH for vented crawlspaces. The thermal behavior difference is significant: a vented crawlspace at 2.0 ACH exchanges its entire air volume twice per hour with outdoor air, while an unvented crawlspace has conduction-only coupling to the ground.

**Code Location**: `crates/hares-io/src/hpxml/building.rs:1733-1740` and `crates/hares-core/src/dwelling/solver_builder.rs:1250-1253`

**Root Cause**: The `unwrap_or(false)` default at building.rs:1740 does not differentiate between foundation types. OCHRE hpxml.py:689 explicitly checks: `zones["Foundation"]["Vented"] = foundation.get("FoundationType")["Crawlspace"].get("Vented", True)`. For basements, OCHRE defaults to `False` (line 691), which matches HARES. The discrepancy is crawlspace-specific.

**Impact**: Crawlspaces without explicit `Vented` tags are modeled as unvented, reducing foundation zone air exchange and potentially overstating underfloor insulation effectiveness. This primarily affects the ResStock sample where vented crawlspaces are common.

### Finding 5: [Severity: medium]
**Description**: The `building_to_boundary_inputs` function subtracts computed film resistances from the assembly R-value (conversions.rs:150) but only guards against negative results with `.max(1e-6)`. When an HPXML specifies an `AssemblyEffectiveRValue` that is less than the sum of the two film resistances (which can be ~0.17 m²·K/W interior + ~0.04 m²·K/W exterior = ~0.21 m²·K/W total), the material R-value becomes negative and is silently clamped to near-zero. No warning is emitted. Per HPXML §6.3, `AssemblyEffectiveRValue` includes both material layers and surface air films; assemblies below ~R-1.2 IP (0.21 SI) are physically implausible for occupied dwellings.

**Code Location**: `crates/hares-core/src/dwelling/conversions.rs:148-156`

**Root Cause**: The clamping is a safety net but should be accompanied by a diagnostic warning when the assembly R-value is less than the sum of the two film resistances, since this indicates either a data error or an assembly that is physically too thin to be occupied.

**Impact**: Silently corrected values can mask upstream data quality issues. A user who inadvertently enters R-0.5 for a wall would get a near-zero material R-value with no indication their input was problematic.

### Finding 6: [Severity: low]
**Description**: Window area reduction on walls uses `.max(0.0)` (building.rs:575) when subtracting window/door areas from attached walls. If a window's area equals or exceeds its host wall's area, the wall gets zero area with only a `tracing::warn!` — no hard error. A zero-area wall with RC nodes can create singular matrices in the state-space model. The validation layer (`validate_building_ranges`, validation.rs:287-311) checks the aggregate window-to-wall ratio but not per-wall exceedances.

**Code Location**: `crates/hares-io/src/hpxml/building.rs:573-584`

**Root Cause**: The window area subtraction loop (lines 549-586) uses `max(0.0)` to prevent negative wall areas, but the per-wall check for a specific window exceeding its specific wall is only logged, not enforced.

**Impact**: A misconfigured HPXML with wall ID mismatches could produce zero-area conditioned-exterior walls. These would still create RC nodes in the network but with zero conductance, effectively making the wall adiabatic.

### Finding 7: [Severity: low]
**Description**: The `SteelFrame` construction type path correctly errors when no LUT match is found and no `FramingFactor` is provided (conversions.rs:333-344). However, the default framing factor branch at building.rs:1294-1297 only provides a default for `WoodStud` → `Some(0.25)`. For any other construction type (e.g., `ConcreteMasonryUnit`, `DoubleWoodStud`, `StructuralInsulatedPanel`) with no explicit framing factor, no default is applied — the boundary gets `None` for framing_factor, which means no parallel-path thermal bridging correction. This is correct for CMU/SIP (which don't use the parallel-path method) but may silently omit thermal bridging for other wood-frame construction types that aren't explicitly tagged as "WoodStud" in the HPXML.

**Code Location**: `crates/hares-io/src/hpxml/building.rs:1294-1297` and `crates/hares-core/src/dwelling/conversions.rs:333-344`

**Root Cause**: The framing factor default lookup at line 1294 is restricted to exactly `"WoodStud"`. HPXML allows various WallType child element names beyond `WoodStud` (`StructuralBrick`, `DoubleWoodStud`, `SIP`, etc.) that may not match this string. While the structural types are deliberately excluded, `DoubleWoodStud` likely should receive a framing factor default.

**Impact**: Wall constructions not tagged as `WoodStud` bypass the parallel-path thermal bridging correction, computing effective R-value from insulation R-value alone, which overstates wall performance. For typical 2×6 at 24" OC advanced framing, this omits the ~22% framing fraction and understates the wall U-factor by ~10-15%.

### Finding 8: [Severity: low]
**Description**: The `find_zone_idx` function (conversions.rs:392-410) returns the first zone matching a given `ZoneType`. In the current implementation, this is adequate because each zone type appears at most once in the zone vector. However, if multi-zone dwellings are ever supported (e.g., two separated conditioned zones), the function would map both to the first conditioned zone index, silently connecting boundary thermal conductance to the wrong zone in the RC network. OCHRE has the same limitation (zone types are singleton keys in both codebases).

**Code Location**: `crates/hares-core/src/dwelling/conversions.rs:392-410`

**Root Cause**: The zone-index resolution uses `ZoneType` equality as a lookup key, which is a singleton mapping in the current single-zone-per-type architecture. This is not currently a bug but would become one if the zone model is extended.

**Impact**: Currently none — the architecture guarantees at most one zone per type. Should be documented and guarded if multi-zone support is planned.

## Summary
- Total findings: 8
- Critical: 0
- High: 2
- Medium: 3
- Low: 3

## Recommendations

1. **Add explicit foundation-type defaults** (`severity: high`): When `<Foundations>` is absent or `<FoundationType>` is missing, apply a conservative default (e.g., slab-on-grade) with a prominent warning. For unknown foundation types inside an existing `<Foundations>` element, error loudly matching OCHRE's behaviour. This prevents silent model degradation for poorly-specified HPXML files.

2. **Fix attic vented-status inconsistency** (`severity: high`): `ensure_referenced_zones_exist` should set `vented: true` for attic zones (matching line 1691), or should never create attic zones at all and defer entirely to `build_zone_map`. The divergence between the two creation paths means the modelled attic can differ depending on whether the `<Attics>` group is present in the HPXML.

3. **Emit warning for R-value fallback** (`severity: medium`): When `building_to_boundary_inputs` applies `DEFAULT_R_M2_K_W` (2.5 m²·K/W) because no R-value is specified, emit a `tracing::warn!` identifying the boundary ID. This gives users visibility when a boundary is using the generic fallback rather than an HPXML-derived value.

4. **Default crawlspace vented to `true`** (`severity: medium`): Align the foundation vented default with OCHRE and ResStock convention: crawlspaces default to vented (`vented: true`), basements default to unvented (`vented: false`). Replace the flat `unwrap_or(false)` at building.rs:1740 with type-specific logic.

5. **Warn on implausible assembly R-values** (`severity: medium`): When the assembly R-value after film subtraction is clamped to `1e-6`, emit a diagnostic warning noting that the HPXML-specified R-value is below the minimum credible value for a conditioned dwelling envelope surface.

6. **Error on zero-area wall from window subtraction** (`severity: low`): Upgrade the per-wall zero-area log from `tracing::warn!` to an `HpxmlError::Parse` when a window/door area equals or exceeds its host wall area. This catches HPXML referencing errors early and avoids generating zero-conductance RC branches.

7. **Expand framing factor defaults** (`severity: low`): Add a `DoubleWoodStud` → `Some(0.22)` entry to the default framing factor table at building.rs:1294 to cover the common advanced-framing case in ResStock.

8. **Document single-zone-per-type assumption** (`severity: low`): Add a doc comment on `find_zone_idx` noting that it assumes at most one zone per `ZoneType` and that multi-zone support would require zone-ID-based lookup rather than type-based lookup.

## References / Citations

- OCHRE `hpxml.py:274-290` — foundation type parsing and unknown-type error raising
- OCHRE `hpxml.py:635` — attic vented default (`Vented=True`)
- OCHRE `hpxml.py:689` — crawlspace vented default (`Vented=True`)
- OCHRE `hpxml.py:711-718` — unvented foundation defaults
- ASHRAE HoF 2021 Ch. 18.31 — F-factor perimeter heat loss method
- ASHRAE HoF 2021 Ch. 27 Table 6 — framing fraction assembly values
- HPXML 4.0 §6.3 — AssemblyEffectiveRValue includes surface air films
- HPXML 4.0 §6.5 — required UFactor and SHGC elements for windows
- ANSI/RESNET/ICC 301-2019 Table 4.2.2(1) — interior shading defaults
- Walker & Wilson (1998) — AIM-2 infiltration model coefficients
