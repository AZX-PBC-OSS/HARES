# Foundation wall area defaults to 0.0 for missing Area element
**Review ID**: hpxml-04
**Category**: hpxml
**Date**: 2026-05-26

## Files Reviewed
crates/hares-io/src/hpxml/building.rs

## Vendor/Reference Files Consulted
vendors/OCHRE/ochre/utils/hpxml.py

## Findings
### Finding 1: [Severity: high]
**Description**: FoundationWall and Slab boundaries silently default to area 0.0 when the `<Area>` child element is missing, producing zero heat loss with no diagnostic. All other boundary types (Wall, Roof, Floor, Window, Door, RimJoist) treat a missing or non-positive Area as a hard parse error.

**Code Location**: `crates/hares-io/src/hpxml/building.rs` — `parse_boundary_area()` at lines 1300–1327.

Specifically:  
- Line 1305: `let area = parse_value_with_units(node.child("Area"), ValueKind::Area);`  
- Line 1307: `BoundaryType::FoundationWall | BoundaryType::Slab => Ok(area.unwrap_or(0.0)),`  
- Lines 1309–1324: All other boundary types call `area.ok_or_else(|| HpxmlError::Parse(...))` to produce a hard error.

There is no `tracing::warn!` or any diagnostic message emitted for the fallback path.

**Root Cause**: The `parse_boundary_area` function explicitly special-cases FoundationWall and Slab to default area to 0.0 instead of requiring an Area element like other boundary types. The intent may have been to tolerate HPXML documents where foundation wall area is omitted (since these surfaces are sometimes derived from perimeter × height rather than declared), but the silent zero default is too permissive.

**Impact**:
- A malformed `<FoundationWall>` element missing `<Area>` produces zero heat loss through that surface with no warning in the log output. This can go undetected indefinitely.
- The downstream RC network assembly already skips zero-area boundaries without warning (`boundary_rc.rs:507`: `if bd.area_m2 <= 0.0 { continue; }`), compounding the issue — two layers of silent suppression.
- This is inconsistent with the behavior for above-grade walls, roofs, and floors, which all produce a hard `HpxmlError::Parse` error for the same defect. Users and validators cannot distinguish between an intentionally omitted area and a malformed input.
- OCHRE accesses `bd_data["Area"]` directly via dictionary key access (`hpxml.py:140`: `convert(bd_data["Area"], "ft^2", "m^2")`), which raises a `KeyError` if the key is missing — effectively treating it as a hard error. HARES is more permissive than its primary vendor reference.

## Summary
- Total findings: 1
- Critical / High / Medium / Low: 0 / 1 / 0 / 0

## Recommendations
1. Add a `tracing::warn!` diagnostic at minimum when a FoundationWall or Slab area defaults to 0.0, identifying the boundary by its `SystemIdentifier` id so the user knows which surface is affected.
2. Consider treating the missing Area case the same as other boundary types (hard parse error) for FoundationWall, since the HPXML specification considers `<Area>` a required child of `<FoundationWall>`. Slabs may warrant similar treatment, though HPXML permits perimeter-based slab models in some contexts.
3. If FoundationWall must tolerate a missing Area (e.g., because perimeter-based area derivation is planned), add a `tracing::info!` or `tracing::debug!` log indicating that the area was inferred/derived, distinguishing it from the silent-0.0 fallback.

## References / Citations
- HARES `parse_boundary_area()`: `crates/hares-io/src/hpxml/building.rs:1300-1327`
- HARES `parse_boundary()` calling convention: `crates/hares-io/src/hpxml/building.rs:1096-1098`
- HARES RC assembly zero-area skip: `crates/hares-envelope/src/boundary_rc.rs:507-509`
- OCHRE area access: `vendors/OCHRE/ochre/utils/hpxml.py:140` (`convert(bd_data["Area"], ...)`)
- OCHRE boundary parsing: `vendors/OCHRE/ochre/utils/hpxml.py:136-140` (`parse_hpxml_surface`)
- HARES wall missing area error test: `crates/hares-io/src/hpxml/building.rs:3021-3027` (confirms hard error for other boundary types)
