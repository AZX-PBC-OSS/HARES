# Per-shape usable roof area fractions: Gable=0.75, Hip=0.35, Flat=0.70
**Review ID**: pvsize-01
**Category**: pv-sizing
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/pv_sizing.rs` (746 lines)

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/PVWatts.hh` — EnergyPlus PVWatts generator interface; takes DC system capacity as user input, does not estimate roof-to-capacity conversion.
- `vendors/EnergyPlus/src/EnergyPlus/PVWatts.cc` — Implementation; default `groundCoverageRatio = 0.4` at line 301 (used for one-axis tracking on ground, not for rooftop area estimation).
- `vendors/OCHRE/ochre/Equipment/PV.py` — OCHRE PV model; takes capacity directly as input (line 100-101), selects tilt/azimuth from envelope roof surfaces but performs no roof-area-to-capacity sizing.

## Findings

### Finding 1: [Severity: high] Hip usable fraction of 0.35 is not derived from the stated 15% + 12% components
**Description**: The doc comment at line 96 claims all three fractions "account for fire-code setbacks (~15%) and obstruction deductions (~12%)." However, 0.35 cannot be the product or sum of 15% and 12% reductions applied to a starting value of 1.0. For Gable, the arithmetic works: (1 − 0.15) × (1 − 0.12) = 0.748 ≈ 0.75. For Hip, the implied additional reduction beyond the Gable baseline is approximately 53%, which is unstated and unsubstantiated in the comment. The reader cannot trace how 0.35 was derived using the documented methodology.
**Code Location**: `crates/hares-physics/src/pv_sizing.rs:95-104`
**Root Cause**: The comment generalizes a single justification across all three shapes but the Hip fraction incorporates a much larger geometric loss (multiple ridge/hip/valley edge setbacks) that is not separately documented.
**Impact**: Users and maintainers cannot assess whether the 0.35 value is physically justified or simply an arbitrary heuristic. If it is correct, the justification needs to be documented. If it is incorrect, PV capacity for hip-roof buildings may be systematically underestimated by ~53% relative to what the stated methodology would produce.

### Finding 2: [Severity: medium] Obstruction allowance of 12% is at the low end of the NREL range (10–18%)
**Description**: The comment at line 96 states obstruction deductions are ~12%. NREL's "Rooftop Solar Photovoltaic Technical Potential in the United States" (Gagnon et al., NREL/TP-6A20-65298, 2016) found that small obstacles (vents, chimneys, skylights) account for 4–10% area loss, while total obstruction losses including roof geometry complexity (dormers, varying pitch) range from 10–18%. The 12% value sits at the low end of this range. A 2018 refinement (Sigrin & Mooney, NREL/TP-6A20-71021) used region-specific exclusion rates that averaged closer to 14–15%.
**Code Location**: `crates/hares-physics/src/pv_sizing.rs:95-98`
**Root Cause**: The obstruction allowance was likely chosen as a round number near the NREL range midpoint, but it biases toward the lower (optimistic) end.
**Impact**: PV capacity may be overestimated by 2–6 percentage points for roofs with above-average density of vents, chimneys, or dormers (common in older housing stock). Since usable area directly scales PV production estimates, a 3% optimistic bias in the obstruction factor results in a 3% overestimate of PV output for all roof shapes.

### Finding 3: [Severity: medium] Fire-code setback of 15% is not universally conservative
**Description**: The IFC 2018/2021 Section 1204 (and California Fire Code) requires a 3-foot setback from roof ridges and 3-foot-wide access pathways from eaves to ridge. For a fixed 3-foot setback, the fraction of roof area removed depends on roof dimensions. On a typical 30'×40' gable roof (face ≈15' from eave to ridge), a 3-foot ridge setback alone removes 3'/15' = 20% of each face. The 15% figure is only achieved on larger roofs (e.g., 40'×50', where face depth is 20', giving 3'/20' = 15%). For smaller homes, 15% understates the setback loss. Some jurisdictions (e.g., California Title 24, Part 9) also require additional setbacks that can increase the total to 20–25%.
**Code Location**: `crates/hares-physics/src/pv_sizing.rs:95-98`
**Root Cause**: The 15% value is based on a single representative roof size rather than accounting for roof dimension variation.
**Impact**: For smaller residential roofs (under ~1,500 sq ft footprint), the actual fire-code setback may remove 18–22% of face area, leading to PV capacity overestimation of 3–7 percentage points. This bias is systematic and scales with roof size: the smaller the roof, the larger the error.

### Finding 4: [Severity: medium] Hip shape selection logic in `infer_roof_shape` has weak heuristics
**Description**: The function `infer_roof_shape` at line 476 infers roof shape from available metadata. At line 516–518, when `distinct_azimuths.len() >= 2` AND `latitude < 30.0`, the function returns `RoofShape::Hip`. Two distinct azimuths (e.g., east and west) are equally consistent with a gable roof as with a hip roof. A latitude heuristic (<30°) is not a reliable predictor of roof shape — hip roofs are not notably more common at low latitudes. In the opposite direction, at line 511–513, the presence of tile/slate material with ≥2 azimuths infers Hip, but tile roofs are common on gable roofs in the southwestern US (Spanish/Mediterranean style).
**Code Location**: `crates/hares-physics/src/pv_sizing.rs:476-521`
**Root Cause**: Classification relies on weak proxy signals (azimuth count, material, latitude) rather than geometric analysis of the roof planes themselves.
**Impact**: Misclassification from Gable to Hip results in applying a 0.35 usable fraction instead of 0.75 — a 2.14× reduction in estimated PV capacity. False-positive Hip classification is a high-impact error. Given the inference rules, a single-family home in Florida (latitude < 30°) with east-west facing roof planes would be classified as Hip even if it has a simple gable roof, undercounting PV potential by over 50%.

### Finding 5: [Severity: low] Single-shape-per-building model cannot represent complex roofs
**Description**: The `RoofShape` enum (line 11) and `infer_roof_shape` function assign one shape to an entire building. Roofs with mixed geometry — e.g., a main gable section with a flat garage extension, or a gable roof with dormers — are modeled with a single fraction. The `RoofInfo` struct contains per-plane data (area, tilt, azimuth, material) but each plane shares the same global `RoofShape`. The `enumerate_pv_candidates` function (line 384) applies the same `usable_fraction(roof_shape)` to every candidate plane (line 414).
**Code Location**: `crates/hares-physics/src/pv_sizing.rs:10-14, 384-455`
**Root Cause**: The data model was designed for simple roof typologies and does not carry per-plane shape information. HPXML input data may not provide per-plane shape classification.
**Impact**: For mixed-shape roofs, either the entire roof is penalized (if classified as Hip) or overly credited (if classified as Gable). The error is bounded by the difference between fractions (up to 2.14×), though mixed-shape roofs are a minority of the building stock.

### Finding 6: [Severity: low] EnergyPlus PVWatts and OCHRE PV provide no roof-area-to-capacity methodology for comparison
**Description**: Both vendor references take PV system capacity as a direct user input rather than deriving it from roof geometry. EnergyPlus PVWatts (`PVWattsGenerator` constructor at `PVWatts.cc:77-169`) requires `dcSystemCapacity` as a parameter and has no roof-area estimation logic. Its `groundCoverageRatio` parameter (default 0.4, line 301) is passed through to the SAM PVWatts model for one-axis tracking ground-mount systems, not for rooftop area estimation. OCHRE's `PV.__init__` (`PV.py:88-131`) takes `capacity` as an optional parameter and falls back to a schedule-based approach — it performs no roof-area-to-capacity conversion. The HARES approach of estimating capacity from roof geometry is novel among the vendors examined and has no direct reference implementation to validate against.
**Code Location**: `vendors/EnergyPlus/src/EnergyPlus/PVWatts.cc:158-159`, `vendors/OCHRE/ochre/Equipment/PV.py:100`
**Root Cause**: N/A — this is an architectural observation, not a defect.
**Impact**: The HARES approach has no prior art to calibrate against, which increases the importance of getting the internal assumptions (fractions, setbacks, obstruction rates) right. Independent validation against real-world PV installation data or NREL's LiDAR-based rooftop studies would strengthen confidence.

## Summary
- Total findings: 6
- High: 1 (Hip fraction derivation undocumented/misstated)
- Medium: 3 (obstruction allowance at low end of NREL range; setback fraction varies by roof size; weak Hip inference heuristics)
- Low: 2 (single-shape model; no vendor reference for roof-to-capacity method)

## Recommendations
1. **Document the Hip fraction derivation.** The 0.35 value for Hip should have a separate justification explaining the geometric reasoning (multiple hip/ridge/valley lines, narrower usable faces, etc.) rather than implying it derives from the same 15%+12%-components as Gable. If derived from geometric analysis of representative hip roof dimensions, include the worked example.
2. **Increase obstruction allowance to at least 14%.** Align with the midpoint of NREL's 10–18% range or use NREL's 2018 region-specific rates. Even a 2 percentage point adjustment improves accuracy for average housing stock.
3. **Make fire-code setback fraction roof-size-dependent.** Consider computing the setback fraction from roof dimensions (face depth between eave and ridge) rather than using a fixed 15%. A minimum setback of 3 feet from ridges and 3-foot access pathways should be applied geometrically when dimensions are available.
4. **Improve Hip inference reliability.** Remove the latitude heuristic (line 516) and replace with geometric analysis — e.g., detecting triangular vs. rectangular plane shapes, or checking for diagonal ridge/valley edge counts from adjacent plane azimuth relationships.
5. **Consider per-plane shape classification.** If HPXML or geometric data supports it, allow different roof planes to carry different shape types (e.g., gable-on-gable, dormer faces). This is a lower-priority enhancement for future work.
6. **Validate against NREL rooftop potential data.** Cross-reference HARES capacity estimates against published NREL rooftop PV potential datasets (Gagnon et al. 2016; Joshi et al. 2023) for a sample of building archetypes to quantify aggregate bias.

## References / Citations
- IFC 2018 Section 1204 / IFC 2021 Section 1204 — Solar photovoltaic power systems, roof access and setbacks.
- California Fire Code 2019, Chapter 12 — Solar photovoltaic power systems (incorporates IFC 1204 with California amendments).
- Gagnon, P., et al. (2016). "Rooftop Solar Photovoltaic Technical Potential in the United States: A Detailed Assessment." NREL/TP-6A20-65298. Obstruction loss data from §3.2 and Table 3.
- Sigrin, B. & Mooney, M. (2018). "Rooftop Solar Technical Potential for Low-to-Moderate Income Households in the United States." NREL/TP-6A20-71021. Region-specific exclusion factors.
- Joshi, S., et al. (2023). "Rooftop Solar Technical Potential in the Continental United States." NREL/TP-6A20-84739. Updated methodology with higher-resolution exclusion layers.
- SAM PVWatts v8 Technical Reference, NREL/TP-7A40-80694 — Documents the PVWatts model but does not include roof-area estimation.
- EnergyPlus Engineering Reference, "Photovoltaic Arrays" section — Documents `Generator:PVWatts` object and GCR parameter.
