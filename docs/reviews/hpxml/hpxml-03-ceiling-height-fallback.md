# Ceiling height defaults to 2.5m with only a warning
**Review ID**: hpxml-03
**Category**: hpxml
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-io/src/hpxml/building.rs`
- `crates/hares-core/src/dwelling/solver_builder.rs`

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/utils/hpxml.py`
- `vendors/OCHRE/ochre/utils/envelope.py`

## Findings

### Finding 1: [Severity: high] Ceiling height defaults to 2.5 m with only a `tracing::warn` — no hard error

**Description**:
When both `ConditionedBuildingVolume` and `ConditionedFloorArea` are absent from the HPXML, the parser computes `ceiling_height_m = None` (line 465–468) and then falls back to a hardcoded 2.5 m at line 790–798 with only a `tracing::warn`. This 2.5 m default silently propagates into three physically significant domains:

1. **Zone volumes**: `Conditioned` and `Garage` zone volumes are computed as `floor_area × 2.5` (lines 868, 872).
2. **Building height** for infiltration: `solver_builder.rs:955–956` derives `building_height_m = 2.5 × number_of_conditioned_zones`, feeding into AIM-2 wind/stack coefficients and attic infiltration calculations.
3. **Infiltration height** fallback: `solver_builder.rs:984–985` uses `2.5 × floors_above_grade` as the infiltration height parameter when `InfiltrationHeight` is not provided.

A user providing an HPXML file without conditioned floor area or volume receives a fully simulated building with fabricated geometry — ceiling height, zone volumes, and infiltration coefficients are all derived from an arbitrary dimension with no normative foundation.

**Code Location**: `crates/hares-io/src/hpxml/building.rs:790–798`
```rust
let default_height_m = match ceiling_height_m {
    Some(h) => h,
    None => {
        tracing::warn!(
            "ceiling height not derivable from conditioned volume/area; falling back to 2.5 m"
        );
        2.5
    }
};
```

**Root Cause**:
The `Building` struct (`building.rs:252–253`) stores both `conditioned_volume_m3` and `ceiling_height_m` as `Option<f64>`, making absence a recoverable state rather than a parse error. The fallback value 2.5 m has no basis in HPXML schema defaults, ASHRAE 140 reference geometry, or any national residential dataset — it appears to be an arbitrary engineering convenience.

By contrast, OCHRE (`hpxml.py:253–254`) accesses `construction["ConditionedFloorArea"]` and `construction["ConditionedBuildingVolume"]` directly as raw dictionary lookups. If either key is missing, Python raises a `KeyError`, halting execution. OCHRE does not downgrade missing geometry to a warning — it treats the fields as mandatory.

**Impact**:
- **Silent error propagation**: The fallback cascades through three separate code paths (zone volumes, building height, infiltration height) with no additional warnings at the downstream usage sites.
- **Non-reproducible results**: Two users running the same HPXML file may get different ceiling heights if one happens to include `ConditionedBuildingVolume` and the other does not — and the warning is only visible in tracing logs, not returned to the caller.
- **Second redundant fallback**: `solver_builder.rs:955` independently applies `unwrap_or(2.5)` to `ceiling_height_m`, duplicating the default logic in a different crate. If the parser logic changes but `solver_builder.rs` does not, the defaults diverge.
- **Volume fallback chain**: `solver_builder.rs:989` applies `conditioned_volume_m3.unwrap_or(400.0)` — a third magical constant (400 m³) that is similarly unqualified.

### Finding 2: [Severity: medium] `AverageCeilingHeight` from HPXML is neither parsed nor used

**Description**:
The HPXML schema defines `<AverageCeilingHeight>` as a first-class element under `<BuildingConstruction>`, providing a direct source of ceiling height independent of the volume/area ratio. OCHRE (`hpxml.py:259–260`) reads this element and asserts that the volume-derived ceiling height is within 0.1 m of it, serving as a consistency check. HARES does not parse `AverageCeilingHeight` at all — a search for the string across the entire `hpxml` crate returns zero matches.

If `AverageCeilingHeight` were parsed, it could serve as an alternative source when `ConditionedBuildingVolume` is absent, avoiding the 2.5 m fallback entirely for valid HPXML files that include explicit ceiling height but omit the volume element.

**Code Location**: `crates/hares-io/src/hpxml/building.rs` — no reference to `AverageCeilingHeight` anywhere in the module.

**Root Cause**: The parser focuses on deriving ceiling height from volume/area ratio as a convenience, but never implemented direct parsing of the HPXML element that defines this value explicitly.

### Finding 3: [Severity: low] Defensive fallback duplicated across crate boundary

**Description**:
The 2.5 m default appears in two independent locations:

| Location | Expression |
|---|---|
| `building.rs:790–798` | `match ceiling_height_m { None => 2.5, ... }` |
| `solver_builder.rs:955` | `building.ceiling_height_m.unwrap_or(2.5)` |

If the parser crate (`hares-io`) adds a different fallback or remodels `ceiling_height_m` as a non-optional field, the downstream crate (`hares-core`) will silently continue using 2.5 m. This is a DRY violation that increases the risk of inconsistent defaults across the codebase.

**Code Location**: `crates/hares-core/src/dwelling/solver_builder.rs:955`

## Summary
- Total findings: 3
- Critical: 0
- High: 1
- Medium: 1
- Low: 1

## Recommendations

1. **Treat missing geometry as a hard error (high)**. When both `ConditionedBuildingVolume` and `ConditionedFloorArea` are absent, return `Err(HpxmlError::Parse("missing both ConditionedBuildingVolume and ConditionedFloorArea; cannot derive ceiling height"))` instead of falling back to 2.5 m. This matches OCHRE's approach (KeyError on missing dictionary keys) and prevents silent fabrication of building geometry.

2. **Parse `<AverageCeilingHeight>` as an additional input (medium)**. If `ConditionedBuildingVolume` is absent but `AverageCeilingHeight` and `ConditionedFloorArea` are present, compute volume as `area × ceiling_height`. If `ConditionedFloorArea` is absent but `AverageCeilingHeight` and `ConditionedBuildingVolume` are present, compute area as `volume / ceiling_height`. Use `AverageCeilingHeight` as the ceiling height directly rather than deriving it — it is the authoritative element for this quantity.

3. **Remove the duplicate fallback in `solver_builder.rs` (low)**. Once ceiling height becomes a required (non-optional) quantity or the fallback is centralized in one location, eliminate the `unwrap_or(2.5)` in `solver_builder.rs:955` and either make `ceiling_height_m` a `f64` (not `Option<f64>`) or fail loudly if it remains absent at that stage.

4. **Audit `conditioned_volume_m3` fallbacks (low)**. The `unwrap_or(400.0)` at `solver_builder.rs:989` for AIM-2 infiltration volume shares the same pattern. If geometry is mandatory, this should never trigger; if it can trigger, the constant needs justification and documentation.

## References / Citations

- OCHRE `hpxml.py:253–258`: Direct dictionary access to `ConditionedFloorArea` and `ConditionedBuildingVolume` — raises `KeyError` if absent.
- OCHRE `hpxml.py:259–260`: `AverageCeilingHeight` used as a consistency assertion against the volume/area ratio.
- OCHRE `envelope.py:539`: `building_height = construction["Ceiling Height (m)"] * construction["Indoor Floors"]` — ceiling height is required for infiltration height computation.
- HARES `solver_builder.rs:955–985`: Downstream duplicate fallback and infiltration parameter derivation from ceiling height.
- HARES `building.rs:868–872`: Zone volume computation using `default_height_m`.
