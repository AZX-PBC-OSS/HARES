# HPXML building→RoofPlane extraction — tilt fallbacks, boundary wiring, area inheritance

**Review ID**: pvsize-08
**Category**: pv-sizing
**Date**: 2026-05-26

## Files Reviewed

- `crates/hares-io/src/pv_sizing.rs` — `extract_roof_info()` (lines 14–47)
- `crates/hares-io/src/hpxml/building.rs` — `parse_boundary()` (lines 1096–1171), `parse_boundary_area()` (lines 1300–1327), `parse_boundaries()` (lines 942–973)
- `crates/hares-physics/src/pv_sizing.rs` — `infer_roof_shape()` (lines 476–521), `RoofPlane` struct (lines 17–30), `azimuth_production_factor_lut()` (lines 124–139), `resolve_azimuth()` (lines 163–188)

## Vendor/Reference Files Consulted

- `vendors/OCHRE/ochre/Equipment/PV.py` — SAM model conventions, azimuth translation (line 58)
- `vendors/OCHRE/ochre/utils/hpxml.py` — boundary parsing, Pitch processing (line 169), zone routing (lines 87–106)
- `vendors/EnergyPlus/src/EnergyPlus/PVWatts.hh` — azimuth default = 180°, tilt default = 20° (lines 188–189)
- `vendors/EnergyPlus/src/EnergyPlus/DataSurfaces.hh` — Compass4/Compass8 azimuth range conventions (lines 87–127)

---

## Findings

### Finding 1: [Severity: high] Missing Pitch element defaults to tilt=0°, misclassifies sloped roofs as flat

**Description**: When an HPXML `<Roof>` element lacks a `<Pitch>` child element, HARES defaults to pitch=0.0, yielding `tilt_deg = atan(0.0/12) = 0.0°`. This causes any roof plane without an explicit pitch to be classified as flat, even when the building geometry implies a sloped roof (e.g., cathedral ceiling or gable end walls present).

**Code Location**:
- `building.rs:1128` — `let pitch = parse_value_with_units(node.child("Pitch"), ValueKind::Raw).unwrap_or(0.0);`
- `pv_sizing.rs:22` — `tilt_deg: b.tilt_deg.unwrap_or(0.0),` (redundant/reachable only if boundary type changes without updating tilt logic)
- `pv_sizing.rs:490` — `roof.planes.iter().all(|p| p.tilt_deg < 1.0)` → classifies as `RoofShape::Flat`

**Root Cause**: HPXML v4.0 schema does not require `<Pitch>` — it is optional. Some HPXML generators (particularly for existing-building energy audits) omit `<Pitch>` because roof slope is implied by other geometry (e.g., gable end wall areas, attic volume, or `<RoofType>`). The `unwrap_or(0.0)` in the pitch parser assumes "absent = flat," which is incorrect for sloped roofs found in the majority of single-family detached HPXML models.

**Impact**:
1. **Flat roof misclassification**: `infer_roof_shape()` (line 490) checks `p.tilt_deg < 1.0` for all planes; a sloped roof with missing Pitch → all planes tilt=0 → classifies as Flat. This changes:
   - `usable_fraction()`: Flat=0.70 vs Gable=0.75 vs Hip=0.35
   - GCR (Ground Coverage Ratio) applied only to flat roofs, reducing panel count
   - Tilt selection: Flat path uses `latitude.min(25)` instead of the true roof pitch
2. **Silent correctness failure**: No warning is emitted when Pitch defaults to 0, so users receive a diagnostic-free incorrect PV sizing.
3. **OCHRE comparison**: OCHRE's `hpxml.py:162` uses `"Pitched" if bd_data.get("Pitch") > 0 else "Flat"`. Crucially, Python's `None > 0` raises `TypeError` — OCHRE would **crash** on a missing Pitch, which is arguably worse but at least prevents silent misclassification. OCHRE also computes `pitch2deg(bd_data.get("Pitch"))` at line 169 which would propagate `None` into `math.atan(None/12)`, also crashing.

### Finding 2: [Severity: low] `tilt_deg` double-default — parser already sets `Some(...)`, but `extract_roof_info` has a dead-code fallback

**Description**: `parse_boundary()` (building.rs:1125–1131) unconditionally wraps `tilt_deg` in `Some(...)` for all `BoundaryType::Roof` entries — even when Pitch is missing (yielding `Some(0.0)`). Meanwhile `extract_roof_info()` (pv_sizing.rs:22) applies `b.tilt_deg.unwrap_or(0.0)`, which will never execute the `unwrap_or` path for roofs (only for `_ => None` boundary types like `BoundaryType::Door` or `BoundaryType::Window`). The redundant `unwrap_or` is dead code for the roof path but creates a misleading impression that the caller may receive `None` tilt for roofs.

**Code Location**: `pv_sizing.rs:22`

**Impact**: Maintenance risk — a future refactor that makes `tilt_deg` truly optional for roofs (e.g., by not computing from Pitch) would silently fall through to 0.0 instead of flagging an error. No current behavioral impact, but the logic is fragile.

### Finding 3: [Severity: medium] No polygon-based area computation — HPXML `<Area>` element is sole source

**Description**: The review brief asks whether HARES computes area from boundary polygon vertices when `<Area>` is absent. HARES **never** parses `RoofBoundary` polygons at all — a grep for `RoofBoundary`, `polygon`, `shoelace`, or `vertex` in the `hares-io` crate returns zero results. Area is taken exclusively from the `<Area>` element via `parse_boundary_area()` (building.rs:1300), which **requires** a positive area for roof boundaries (enforced by parse error). There is no area inheritance from parent surfaces, no polygon fallback, and no zero-area path for roofs.

**Code Location**: `building.rs:1300–1327` — `parse_boundary_area()` enforces non-zero area for `BoundaryType::Roof`

**Impact**: This is architecturally correct — HPXML schema requires `<Area>` for enclosure surfaces. However, it means HARES has **no defense-in-depth** against area errors: if `<Area>` is present but inaccurate (wrong units, transposition error), the bad value passes through unchecked. There is no cross-validation against length×width, against the conditioned floor area, or against boundary polygon coordinates. Any area error propagates silently into PV sizing, affecting `max_panels` and `max_capacity_kw` linearly.

### Finding 4: [Severity: low] Azimuth convention is internally consistent (North=0°, clockwise) but lacks explicit documentation at the HPXML→RoofPlane boundary

**Description**: HPXML defines azimuth as "degrees clockwise from true north" (0°=North, 180°=South). HARES preserves this convention throughout:
- `parse_boundary()` (building.rs:1146) reads azimuth as `ValueKind::Raw` — no conversion
- `RoofPlane.azimuth_deg` docstring confirms "0 = north, 180 = south" (pv_sizing.rs:23)
- `azimuth_production_factor_lut()` maps 180°→1.0, 0°→0.45 (south=best, north=worst)
- `south_distance()` computes angular distance from 180°
- `is_north_facing()` checks ≥315° or ≤45°

No north-vs-south reference confusion exists within HARES. The EnergyPlus convention (outward normal azimuth) uses the same convention, though E+ internally converts for SAM (south=180°, per PVWatts default of `azimuth=180`).

**Code Location**: `pv_sizing.rs:124–139` (LUT), `building.rs:1146` (parsing)

**Impact**: No bug, but the convention should be documented at the `extract_roof_info()` function level and in the `Building` struct's `azimuth_deg` field docstring so that consumers of the `RoofPlane` struct do not inadvertently convert.

### Finding 5: [Severity: medium] Multi-building HPXML files are not supported — only first `<Building>` element is processed

**Description**: HARES `parse_hpxml_str()` returns a single `Building`, and `parse_building_from_node()` navigates `root.path(&["Building", "BuildingDetails"])` which accesses only the first `<Building>` element. There is no iteration over multiple `<Building>` elements, no `BuildingID` attribute parsing (confirmed by grep — zero references to `BuildingID` in hares-io Rust source), and no roof plane → building/dwelling unit affinity tracking.

**Code Location**:
- `mod.rs:89–119` — returns single `Building`
- `building.rs:421` — `details = root.path(&["Building", "BuildingDetails"])` — first element only
- No `BuildingID` parsing anywhere in the codebase

**Root Cause**: HARES currently targets single-family building geometry. Multi-family HPXML fixtures in the OCHRE test suite (`base-multiple-buildings.xml` with 3 BuildingIDs) are excluded from HARES tests.

**Impact**: Multi-family HPXML files cannot be processed. If loaded, only the first building's roof planes would be extracted; subsequent buildings (e.g., neighboring townhouse units) would be silently dropped along with their roof planes. This is a **known limitation** rather than a bug, but it means cross-wiring between units cannot occur because the path for it doesn't exist. Documented at `docs/reviews/hpxml/hpxml-10-hpxml-validation-coverage-gaps.md:125`.

### Finding 6: [Severity: medium] `infer_roof_shape()` depends on number of distinct azimuths but ignores planes with `azimuth_deg = None`

**Description**: The hip-detection heuristic in `infer_roof_shape()` (pv_sizing.rs:495–502) counts distinct azimuths from planes that have explicit `azimuth_deg` values:
```rust
let distinct_azimuths: HashSet<u32> = roof
    .planes
    .iter()
    .filter_map(|p| p.azimuth_deg.map(|a| (a / 45.0).round() as u32))
    .collect();
```
If any roof plane lacks an explicit azimuth (e.g., hip roof where one face has no `<Azimuth>` in HPXML), it is excluded from the count. A 4-face hip roof where one face is missing azimuth would produce only 3 distinct directions, skirting the `>= 3` threshold. The tile/slate material heuristic (≥2 planes) and low-latitude heuristic (<30°N with ≥2 planes) provide partial mitigation, but at mid-latitudes with composite/asphalt shingles, a hip roof could be misclassified as gable.

**Code Location**: `pv_sizing.rs:495–502`

**Impact**: If a plane with missing azimuth is north-facing and would have been filtered out anyway, no impact. But if the missing-azimuth plane is south/east/west-facing, the hip roof may be classified as gable, changing the usable area fraction (0.35→0.75), panel aggregation behavior (hip aggregates all planes; gable picks only the best one), and total capacity estimate.

---

## Summary

- Total findings: 6
- Critical: 0
- High: 1
- Medium: 3
- Low: 2

## Recommendations

1. **Add tilt fallback with warning** (Finding 1): When `<Pitch>` is absent for a roof boundary, emit a warning and attempt to infer tilt from other geometry (gable end wall area vs. attic floor area yields the tangent relationship used in `compute_attic_volume()`). If inference is impossible, default to a typical residential roof pitch (e.g., 4:12 ≈ 18.4°) rather than 0°.

2. **Add area validation** (Finding 3): Cross-validate roof `<Area>` values against conditioned floor area bounds (e.g., roof area should be between 50% and 200% of conditioned floor area for typical single-family homes), emitting warnings on outliers.

3. **Document azimuth convention** (Finding 4): Add an explicit comment at `extract_roof_info()` (pv_sizing.rs:14) stating "Azimuth is degrees clockwise from true north (0=N, 180=S), consistent with HPXML v4.0 convention."

4. **Tighten hip detection** (Finding 6): Before counting distinct azimuths, resolve missing azimuths via `resolve_azimuth()` using wall fallback within `infer_roof_shape()`, so that hip detection is not impaired by a few planes missing explicit azimuth.

5. **Multi-building HPXML** (Finding 5): If multi-family support becomes needed, add an alternative entry point `parse_hpxml_buildings()` that returns `Vec<Building>`, iterating over all `<Building>` children and tracking `BuildingID` → roof plane affinity.

## References / Citations

- HPXML v4.0 Schema: `<Pitch>` is an optional child of `<Roof>`, value is rise over 12 run (e.g., 4 = 4:12). `<Azimuth>` convention: degrees clockwise from true north.
- OCHRE `hpxml.py:162–169`: Pitch → tilt conversion via `pitch2deg()`, construction type inferred from Pitch=0 → "Flat".
- OCHRE `PV.py:58`: `azimuth = (azimuth + 180) % 360` converts HPXML convention (0=N) to SAM convention (south=180).
- EnergyPlus `PVWatts.hh:188–189`: Default constructor uses `tilt=20.0`, `azimuth=180.0` (south-facing).
- EnergyPlus `DataSurfaces.hh:99–100`: `Compass4AzimuthLo/Hi` defines North as 315°–45° range, matching HARES `is_north_facing()`.
- HARES `pv_sizing.rs:490`: `roof.planes.iter().all(|p| p.tilt_deg < 1.0)` — the flat-roof detection threshold.
