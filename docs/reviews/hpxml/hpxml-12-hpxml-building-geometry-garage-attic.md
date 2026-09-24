# HPXML garage and attic geometry computation correctness
**Review ID**: hpxml-12
**Category**: hpxml
**Date**: 2026-05-26

## Files Reviewed
crates/hares-io/src/hpxml/building.rs

## Vendor/Reference Files Consulted
vendors/OCHRE/ochre/utils/hpxml.py

## Findings
### Finding 1: [Severity: high]
**Description**: Attic↔Garage wall boundaries are not deleted from the thermal model after being consumed for geometric computation on Path A, diverging from OCHRE's explicit `del boundaries["Attic Garage Wall"]` workaround.

**Code Location**: `building.rs`:2640–2654 (Path A attic volume computation), `building.rs`:2389–2694 (`Boundary` list never culled).

**Root Cause**: OCHRE `hpxml.py`:583–596 enters Path A when `"Attic Garage Wall"` boundaries exist (walls between Garage and Attic zones). These wall areas are merged into `attic_wall_areas` for gable-area derivation, then explicitly deleted with `del boundaries["Attic Garage Wall"]` at line 596 (annotated `# FIXME: we need to be able to handle this at some point`). The OCHRE comment acknowledges the deletion is a workaround. HARES collects these walls in `attic_garage_walls` (`building.rs`:2589–2598), uses them to compute gable area (`building.rs`:2640–2654), but never removes them from the `boundaries` vector. Since `compute_attic_volume` receives `&[Boundary]` (immutable reference), the walls persist in the building's boundary list (`building.rs`:894).

**Impact**: When an HPXML file contains walls between the Garage and Attic (Path A), HARES retains those walls as thermal boundaries, creating an additional heat-transfer surface between the Garage and Attic zones. The compound attic volume formula already accounts for the geometric shape of the shared attic-garage space; keeping the wall boundary on top of that may double-count the thermal coupling. This affects all HPXML models with attached garages where the attic extends over the garage, producing inflated heat exchange between zones compared to OCHRE's behavior.

### Finding 2: [Severity: medium]
**Description**: Garage zone volume is computed as a simple rectangular prism (`floor_area * ceiling_height`) with no roof-space augmentation, diverging from OCHRE which adds a triangular roof prism term.

**Code Location**: `building.rs`:872 — `ZoneType::Garage => zone.floor_area_m2.map(|a| a * default_height_m)`.

**Root Cause**: OCHRE `hpxml.py`:730–734 computes garage volume as `garage_floor_area * ceiling_height + 1/2 * atan(garage_tilt) * garage_protruded_area` when garage roof boundaries exist. The extra term approximates the triangular roof volume above the protruding portion of the garage. HARES applies only the rectangular-floor volume formula, omitting any roof-space contribution.

**Impact**: For houses with pitched garage roofs (non-flat), HARES systematically underestimates garage air volume. For a 6:12 pitch roof with 15 m² protruded area, the missing volume is roughly `0.5 * tan(26.565°) * 15 ≈ 3.75 m³` (assuming the OCHRE `atan()` usage was intended as `tan()` — note: OCHRE `hpxml.py`:734 itself uses `math.atan(garage_tilt)` on a radian value, which may be a separate bug in the reference implementation). The volume underestimate affects garage zone thermal inertia, infiltration calculations, and HVAC sizing.

### Finding 3: [Severity: medium]
**Description**: Gable-wall-ordering dependency: HARES uses `gable_areas[1]` as the attic gable area on Path B (3-gable compound formula), matching OCHRE's convention. The correctness of the resulting volume depends on parse-order stability of boundary iteration, which is not guaranteed across HPXML parsing or boundary-collection order.

**Code Location**: `building.rs`:2666 — `let attic_gable_area = gable_areas[1];`.

**Root Cause**: OCHRE `hpxml.py`:609 selects `attic_gable_area = attic_wall_areas[1]` as the "second" gable wall in list order (comment at L608: "2 attic gables plus 1 garage gable, garage gable has area that is 'more different'"). The ordering is established by Python `list` concatenation at L598–600: `attic_wall_areas = boundaries.get("Attic Wall", {}).get("Area (m^2)", []) + boundaries.get("Adjacent Attic Wall", {}).get("Area (m^2)", [])`. HARES builds `gable_areas` by pushing attic outdoor walls first (`building.rs`:2578–2586), then adjacent attic walls (`building.rs`:2601–2610). This matches OCHRE ordering, but the indirect assumption that index-1 is the correct attic gable is implicit and fragile.

**Impact**: If boundary collection or HPXML element ordering changes, `gable_areas[1]` could silently pick a different gable wall (e.g., the garage gable instead of the attic gable), producing an incorrect attic height and a significantly wrong compound volume. The test at `building.rs`:4538 (`attic_volume_3_gable_index1_differs_from_median`) validates this behavior is intentional, but the implicit ordering dependency is a maintenance risk.

### Finding 4: [Severity: medium]
**Description**: HARES tolerates missing `<Pitch>` elements by defaulting to a 0° roof tilt (flat roof), which silently produces `attic_height = 0` and `volume = 0`, whereas OCHRE would fail with a TypeError.

**Code Location**: `building.rs`:1128–1129 — `let pitch = parse_value_with_units(node.child("Pitch"), ValueKind::Raw).unwrap_or(0.0); Some((pitch / 12.0).atan().to_degrees())`.

**Root Cause**: OCHRE `units.py`:19–21 — `pitch2deg` calls `math.atan(pitch / 12)`, which raises `TypeError` on `None / 12`. HARES silently falls back to a flat roof (0° tilt) when Pitch is absent. While more graceful, this changes a loud failure into a silent incorrect result for pitched-roof HPXML files that accidentally omit Pitch.

**Impact**: A pitched roof whose HPXML file is missing `<Pitch>` will be modeled as flat — attic volume becomes zero — with no warning beyond the eventual `compute_attic_volume` returning `None` only if the roof boundary itself is absent. The falloff chain is: `pitch=0` → `tilt=0°` → `tan(0)=0` → `attic_height=0` → `volume=0`. This propagates silently through the simulation, producing incorrect thermal mass and infiltration results. A `tracing::warn!` when Pitch is absent on a Roof boundary would make this diagnosable.

### Finding 5: [Severity: low]
**Description**: HARES's 0.5 m² threshold for rejecting asymmetric gable walls is more permissive than OCHRE's 0.2 m² assertion, and HARES returns `None` instead of crashing.

**Code Location**: `building.rs`:2699 — `if abs_diff > 0.5 { return None; }`.

**Root Cause**: OCHRE `hpxml.py`:604 uses `assert abs(attic_wall_areas[1] - attic_wall_areas[0]) < 0.2` for the standard 2-gable case, raising an `AssertionError` if gable walls differ beyond 0.2 m². HARES uses a 0.5 m² warning threshold and gracefully returns `None`. This is a deliberate design choice to fail gracefully rather than crash, but the relaxed tolerance means gable-wall area mismatches between 0.2 and 0.5 m² would pass in HARES but be flagged in OCHRE, producing volumes that OCHRE would consider unreliable.

**Impact**: Low-impact divergence. HARES correctly warns and returns `None` for mismatches > 0.5 m². The 0.2–0.5 m² gap is narrow (~1.9% of a 10 m² gable) and unlikely to cause physically significant errors. The graceful failure pattern is arguably better than OCHRE's assertion crash.

## Summary
- Total findings: 5
- Critical: 0
- High: 1
- Medium: 3
- Low: 1

## Recommendations
1. **Finding 1 (High)**: After `compute_attic_volume` Path A, remove Attic↔Garage wall boundaries from the `boundaries` vector, matching OCHRE's `del boundaries["Attic Garage Wall"]` behavior. Alternatively, implement the proper heat-transfer model across attic-garage internal walls (which OCHRE's FIXME comment indicates is the long-term goal) and keep the boundary with corrected area.

2. **Finding 2 (Medium)**: Add a roof-space augmentation term to the garage volume computation when garage roof boundaries with non-zero tilt exist, matching the OCHRE formula (with `tan()` corrected if needed for the unit mismatch in the OCHRE reference).

3. **Finding 3 (Medium)**: Add a `tracing::debug!` or assertion that validates the ordering assumption: verify `gable_areas[1]` is the Attic gable (and not the garage gable) when the 3-gable path is taken. At minimum, document the ordering dependency in the function doc comment.

4. **Finding 4 (Medium)**: Emit a `tracing::warn!` when `<Pitch>` is absent on a `BoundaryType::Roof` element, to make missing-pitch diagnoses traceable after the silent 0° fallback.

## References / Citations
- OCHRE `hpxml.py`:425–497 — `parse_hpxml_construction()`: garage wall height and protruded area derivation
- OCHRE `hpxml.py`:522–633 — `parse_hpxml_zones()`: attic volume (simple + compound), Path A/B gable handling
- OCHRE `hpxml.py`:726–743 — `parse_hpxml_zones()`: garage volume with roof-space term
- OCHRE `units.py`:19–21 — `pitch2deg`: `math.atan(pitch / 12)`
- HARES `building.rs`:1126–1135 — Roof tilt from Pitch: `(pitch / 12.0).atan().to_degrees()`
- HARES `building.rs`:2399–2515 — `compute_garage_geometry()`: protruded area algorithm
- HARES `building.rs`:2555–2724 — `compute_attic_volume()`: simple and compound volume formulas, Path A/B
- HARES `building.rs`:872 — Garage volume: `floor_area * default_height_m`
- HARES `building.rs`:4189–4339 — Garage geometry tests
- HARES `building.rs`:4341–4790+ — Attic volume tests (simple, 3-gable compound, index-vs-median, Path A, asymmetric)
