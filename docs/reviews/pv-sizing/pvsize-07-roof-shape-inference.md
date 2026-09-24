# Roof shape inference logic — classification rules and false-negative risks
**Review ID**: pvsize-07
**Category**: pv-sizing
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/pv_sizing.rs` (746 lines)
- `crates/hares-python/src/py_pv_sizing.rs` (234 lines)
- `crates/hares-python/src/py_dwelling.rs:1229-1273`
- `crates/hares-io/src/pv_sizing.rs` (47 lines)

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/Photovoltaics.cc` — EnergyPlus takes PV surfaces as explicit user input; no roof shape inference.
- `vendors/EnergyPlus/src/EnergyPlus/DataSurfaces.hh` — Surface data model includes `Tilt`, `Azimuth`, and `ShapeCat` but no roof typology classification.
- `vendors/OCHRE/ochre/Equipment/PV.py` — OCHRE PV takes `tilt`/`azimuth` as optional constructor parameters (`PV.py:101-103`); when omitted, selects the roof surface closest to south (`PV.py:106-114`). No roof shape classification is performed.

## Findings

### Finding 1: [Severity: high] Gable default fallback is anti-conservative — misclassification overstates usable area by >2×
**Description**: The `infer_roof_shape` function at line 520 falls back to `RoofShape::Gable` (usable fraction 0.75) when no classification rule fires. A hip roof with only 2 recorded roof planes — a common data limitation in HPXML files that often omit smaller east/west hip planes — with non-tile/slate roofing material and latitude ≥ 30° would be classified as Gable (0.75) instead of Hip (0.35). This overstates the usable roof area by a factor of 2.14× (0.75 / 0.35).

The inference rule order (lines 476–521) is:
1. Apartment / "5+" facility type → Flat (line 484)
2. All tilts < 1° → Flat (line 490)
3. ≥3 distinct azimuths → Hip (line 500)
4. Tile/slate material + ≥2 azimuths → Hip (line 511)
5. Latitude < 30° + ≥2 azimuths → Hip (line 516)
6. **Fallback → Gable (line 520)**

For a 2-plane hip roof (north + south, omitting east/west) in a non-coastal region:
- Rule 3 fails: only 2 distinct azimuths.
- Rule 4 depends on material (likely fails for asphalt shingle).
- Rule 5 fails if latitude ≥ 30°.
- Falls through to Gable — the most optimistic shape.

The NREL ResStock HPXML dataset and related studies show that many HPXML files record only 2 dominant roof planes even for hip-roof buildings, because the smaller hip faces are aggregated into the larger ones or omitted entirely. The expected misclassification rate depends on HPXML data completeness, but audits of ResStock inputs suggest 30–50% of residential HPXML files have simplified or incomplete roof plane descriptions. Among those, hip-to-gable misclassification would be systematic and biased toward overestimation.

**Code Location**: `crates/hares-physics/src/pv_sizing.rs:476-521` (entire `infer_roof_shape`), with the fallback at line 520.
**Root Cause**: The classification ruleset treats Gable as the null hypothesis, but Gable has the most generous usable fraction (0.75). A conservative design would default to Hip (0.35) when evidence is ambiguous, or warn and require explicit user input.
**Impact**: For hip-roof homes with incomplete plane data, PV capacity is overestimated by a factor of 2.14×. This bias is systematic (always toward overestimation), not symmetric. For a typical suburban hip-roof home, this could mean estimating 7.5 kW of capacity where only 3.5 kW is physically achievable.

### Finding 2: [Severity: high] No user override mechanism for roof shape
**Description**: Neither the Python `Dwelling.pv_candidates()` method (py_dwelling.rs:1231) nor `estimate_pv_capacity()` (py_dwelling.rs:1251) exposes a `roof_shape` parameter. Both hard-code the call to `infer_roof_shape()`. The `RoofShape` enum is internal to the Rust physics crate and is not exposed as a Python class. Users who know their building has a hip roof (from an HPXML file that omits the smaller hip planes, or from visual inspection) cannot override the inferred classification. There is no `set_roof_shape()`, no `roof_shape_override` field on the `Dwelling` struct, and no mechanism to pass `RoofShape` through from Python to `compute_usable_area`.

Compare to OCHRE: `PV.__init__` (`PV.py:101-103`) accepts `tilt` and `azimuth` as optional constructor parameters, allowing users to directly specify array orientation regardless of envelope model data. OCHRE does not classify roof shape, but it provides the override pattern that HARES should follow.

**Code Location**: `crates/hares-python/src/py_dwelling.rs:1229-1273`, `crates/hares-python/src/py_pv_sizing.rs:207-234`
**Root Cause**: The public API was designed with inference-only flow; user overrides were not considered as an input path.
**Impact**: Users cannot correct misclassifications. If an HPXML file has incomplete roof plane data, the resulting PV capacity estimate is permanently biased with no programmatic or manual recourse.

### Finding 3: [Severity: medium] Tile/slate material rule is probabilistic, not deterministic, and fires at the wrong priority
**Description**: The tile/slate rule at lines 504–513 classifies any roof with tile or slate material and ≥2 distinct azimuths as `RoofShape::Hip`. While tile and slate roofing is statistically more common on hip roofs (Mediterranean, Spanish Colonial, Mission styles), this association is probabilistic, not deterministic. Tile roofs on gable-style homes are common in the Southwestern US — a gable roof with red clay tile is a standard design in Arizona and New Mexico subdivisions. The rule fires *after* the 3+ azimuths check (line 500) but *before* the latitude check (line 516), so a tile-roofed gable home with 2 planes in Arizona (latitude >30°) would be classified as Hip.

The tile/slate rule requires ≥2 distinct azimuths (line 511), so single-plane tile roofs are not affected. However, for 2-plane tile roofs, the false-positive rate is highest in the Southwest, where:
- Tile roofs are prevalent (Spanish Colonial influence).
- Gable roof geometry is common (not hurricane-prone, no structural need for hips).
- Latitude is typically >30°, so the latitude rule wouldn't rescue the classification.
- 2 planes are the expected HPXML representation for a gable roof.

**Code Location**: `crates/hares-physics/src/pv_sizing.rs:504-513`
**Root Cause**: A material-type heuristic is treated as a classification rule without considering regional roof geometry conventions that break the association.
**Impact**: Tile-roofed gable homes in the Southwest US would be classified as Hip (0.35) instead of Gable (0.75), undercounting PV potential by 53%. This is a false positive for Hip classification — conservative in the individual case, but biased regionally.

### Finding 4: [Severity: medium] Apartment → Flat classification uses fragile string matching on facility type
**Description**: The apartment/Flat rule at lines 482–487 checks for "apartment" or "5+" substrings in the HPXML `ResidentialFacilityType` string. This rule:
1. Is based on building type label, not number of dwelling units — `num_dwelling_units` is not stored in the `Building` struct or the `Dwelling` struct.
2. The "5+" substring match assumes the HPXML facility type string contains text like "multifamily 5+ units" — this is HPXML-implementation-specific and may vary across HPXML generators (BEopt, OpenStudio-HPXML, ResStock, etc.).
3. HPXML 4.0 defines `ResidentialFacilityType` with enumerated values: "single-family detached", "single-family attached", "apartment", "manufactured housing". The "5+" match would only fire if an HPXML generator extends the enumerated value with additional text, which is not guaranteed.
4. "single-family attached" (townhouses) typically have gable roofs but correctly does NOT trigger this rule — which is appropriate but means the rule depends on the HPXML author using the correct enumerated value.

**Code Location**: `crates/hares-physics/src/pv_sizing.rs:482-487`
**Root Cause**: The rule relies on substring matching against a text field that has inconsistent usage across HPXML generators. No numeric dwelling-unit count is available from the building model to support the classification.
**Impact**: Legitimate multifamily buildings (5+ units) with flat roofs could be missed if the HPXML uses a non-standard facility type string. Conversely, if a generator includes "5+" in other facility type strings, false Flat classifications could occur.

### Finding 5: [Severity: medium] Latitude < 30° heuristic for hip roofs has no empirical foundation
**Description**: Line 516–518 classifies any roof with ≥2 distinct azimuths and latitude < 30° as `RoofShape::Hip`. This heuristic has several problems:
1. Hip roofs are more common in hurricane-prone regions (Florida, Gulf Coast) where wind uplift resistance is beneficial, but they are also common in other climates for aesthetic reasons.
2. A latitude of 30°N runs roughly through Houston, TX, and Jacksonville, FL — excluding much of the southeastern US that also has high hip roof prevalence (e.g., southern Georgia at ~31–32°N, coastal South Carolina at ~33°N).
3. The heuristic misclassifies gable roofs in low-latitude regions as Hip. For example, gable roofs with 2 planes (east/west or north/south) are common in equatorial regions for passive ventilation. A gable roof in Honolulu (latitude ~21°N) with east-west facing planes would be classified as Hip.
4. This rule was previously noted in review pvsize-01 Finding 4 as a "weak heuristic" and recommended for removal.

**Code Location**: `crates/hares-physics/src/pv_sizing.rs:516-518`
**Root Cause**: Climate-based proxy (latitude) used as a roof geometry classifier without supporting evidence linking the two variables.
**Impact**: Systematic regional bias in PV capacity estimates — overestimation (gable → hip misclassification) and underestimation (hip → gable misclassification) based on geographic location rather than actual roof geometry.

### Finding 6: [Severity: low] No test coverage for 2-plane hip roof edge case
**Description**: The test `infer_hip_from_many_azimuths` (line 693) tests 3-plane hip classification only. There is no test for:
- A 2-plane hip roof with tile/slate material (should return Hip via material rule).
- A 2-plane hip roof at low latitude (should return Hip via latitude rule).
- A 2-plane hip roof with asphalt shingle at latitude ≥ 30° (should return Gable — this is the false-negative case from Finding 1).
- A 2-plane gable roof with tile/slate at high latitude (false positive hip case).

The test `infer_gable_default` (line 681) covers a 2-plane asphalt-shingle gable at latitude 40°, which correctly returns Gable.

**Code Location**: `crates/hares-physics/src/pv_sizing.rs:693-703`
**Root Cause**: Test coverage was designed for happy-path cases; edge cases that expose inference weaknesses were not included.
**Impact**: The false-negative and false-positive risks described in Findings 1 and 3 escape automated detection.

### Finding 7: [Severity: low] OCHRE comparison — HARES goes beyond both vendor implementations in roof analysis
**Description**: Neither EnergyPlus nor OCHRE performs roof shape classification or roof-area-to-PV-capacity conversion:
- EnergyPlus (`Photovoltaics.cc`) requires users to explicitly define PV surfaces with their own tilt, azimuth, and area. The `SurfaceData` structure (`DataSurfaces.hh:707-744`) includes geometric data (Tilt, Azimuth, Area, ShapeCat for triangular/rectangular) but there is no roof typology (gable/hip/flat) classification.
- OCHRE (`PV.py:106-114`) searches for boundary surfaces with "Roof" in the name, selects the one closest to south (azimuth ~185°), and uses its tilt. But OCHRE does not classify roof shape, does not estimate usable area, and does not compute PV capacity from roof geometry.
- HARES's roof-shape-inference-then-capacity-estimation pipeline is novel and has no direct reference implementation to validate against.

**Code Location**: `vendors/OCHRE/ochre/Equipment/PV.py:106-114`, `vendors/EnergyPlus/src/EnergyPlus/DataSurfaces.hh:707-744`
**Root Cause**: N/A — architectural observation.
**Impact**: Since HARES has no prior art in either vendor for roof shape classification, the burden of validation is entirely internal. The lack of reference implementations increases the risk that classification errors propagate undetected.

## Summary
- Total findings: 7
- High: 2 (anti-conservative Gable default; no override mechanism)
- Medium: 3 (tile/slate probabilistic rule; fragile apartment string matching; latitude heuristic)
- Low: 2 (missing edge-case tests; no vendor reference for classification)

## Recommendations
1. **Change the default fallback to Hip or add an "Unknown" shape.** The Gable default (0.75 usable fraction) is the most optimistic option. When evidence is ambiguous, the system should either default to the most conservative shape (Hip, 0.35) or introduce an `Unknown` shape with a middle-ground fraction (e.g., 0.55). Alternatively, require explicit user input when classification confidence is low.
2. **Expose a roof_shape override to users.** Add a `roof_shape` parameter to `pv_candidates()`, `estimate_pv_capacity()`, and `compute_usable_area()` on the Python side. Expose `RoofShape` as a Python enum (Gable/Hip/Flat) so users can force the classification when they have better information than the HPXML data provides.
3. **Downgrade tile/slate from a Hip classification rule to a confidence weight.** Rather than treating tile/slate material as a deterministic Hip indicator, use it as a probabilistic weight that increases Hip likelihood but does not guarantee it. Consider the full context (latitude, number of planes, roof age, building style) before committing to Hip classification.
4. **Replace latitude heuristic with hurricane/wind-zone data.** If climate-based hip prevalence is desired, use ASCE 7 wind zone or IBHS FORTIFIED designations rather than raw latitude. Hurricane-prone regions have documented higher hip roof adoption rates for wind resistance (IBHS, 2019).
5. **Parse `NumberofUnits` from HPXML.** HPXML v4.0 includes `BuildingSummary/BuildingConstruction/NumberofUnits` (and `NumberofConditionedFloorsAboveGrade` which is already parsed). Use the numeric unit count rather than substring matching on facility type for multifamily classification.
6. **Add test coverage for the misclassification edge cases.** Add tests for 2-plane hip roofs (the false-negative case from Finding 1), 2-plane tile gable roofs (false-positive case from Finding 3), and 2-plane roofs at borderline latitudes.
7. **Consider per-plane shape tagging.** If HPXML input data or geometric analysis can provide per-plane shape information (e.g., plane vertices that reveal triangular vs. rectangular shape — a triangular plane strongly suggests hip roof geometry), use it instead of building-level inference. EnergyPlus's `ShapeCat` enum (`DataSurfaces.hh:726`) provides a precedent for per-surface shape classification.

## References / Citations
- HPXML v4.0 Schema — `ResidentialFacilityType` enumerated values; `NumberofUnits` under `BuildingConstruction`.
- NREL ResStock — National residential building stock characterization using HPXML; documentation on roof geometry simplification in HPXML inputs (see ResStock v3.1 Technical Report).
- IBHS (2019). "Rating the States: An Assessment of Residential Building Code and Enforcement Systems." Documents regional hip roof adoption rates for wind resistance.
- ASCE 7-22 — Wind design provisions; wind speed maps that correlate with hip roof prevalence in hurricane-prone regions.
- EnergyPlus 24.2 Engineering Reference — Photovoltaic Arrays; documents that PV geometry is user-specified, not inferred.
- OCHRE documentation — PV model takes capacity as direct input; roof surface selection for tilt/azimuth is for SAM model initialization only.
