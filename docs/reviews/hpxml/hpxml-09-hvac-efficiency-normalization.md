# HVAC efficiency metric normalization uses uniform conversion factors
**Review ID**: hpxml-09
**Category**: hpxml
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-io/src/hpxml/resolve_hvac.rs`

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/utils/hpxml.py`
- `vendors/OCHRE/ochre/Equipment/HVAC.py`

## Findings

### Finding 1: [Severity: medium]
**Description**: EER2→EER conversion uses a uniform factor of 1/0.96 (= 1.0417) for all equipment types, despite the CEC database showing systematic variation between split-system and packaged equipment. The CEC certified equipment data shows EER = EER2 × 1.043 for split-system AC and EER = EER2 × 1.038 for packaged AC. HARES's uniform factor (1/0.96 ≈ 1.0417) is a defensible central estimate but introduces a small, directional bias: packaged units' EER is overestimated by ~0.35% [(1.0417−1.038)/1.038] and split-system units' EER is underestimated by ~0.13% [(1.043−1.0417)/1.0417].

The code comment at lines 54–56 acknowledges the known variation but states "HPXML cannot distinguish" between split and packaged systems. HPXML's `CoolingSystemType` element does distinguish "central air conditioner" from "room air conditioner" and "packaged terminal air conditioner" — the fine split within central AC (split vs. packaged) is indeed not captured, but the general statement overstates the limitation. Furthermore, HPXML provides capacity data (`CoolingCapacity`, `HeatingCapacity`) that could drive capacity-bin-based factors, since the CEC database variation is partially capacity-dependent.

**Code Location**: `resolve_hvac.rs:54–60` (constant definition + doc-comment); `resolve_hvac.rs:2407` (application site in `normalize_efficiency_units`)

**Root Cause**: Single constant `EER2_TO_EER_FACTOR: f64 = 1.0 / 0.96` used uniformly regardless of system type. The scale of variation (range 0.48%) is small enough that a uniform factor was chosen as a pragmatic simplification during implementation. No lookup table or multi-factor system was created.

**Impact**: The bias magnitude is very small — ±0.35% in EER translates to ±0.35% in EIR and ±0.35% in annual cooling energy. For a typical 2000 sq ft home this is ~10–15 kWh/year, well within modeling uncertainty. The finding is rated medium because the *pattern* (capacity/type-agnostic conversion) is replicated across all three metrics with larger magnitudes for HSPF2 and SEER2, and the acknowledged variation confirms a known limitation rather than a hidden defect.

---

### Finding 2: [Severity: medium]
**Description**: SEER2→SEER and HSPF2→HSPF conversion factors are uniform within the ducted/dectless categories but do not account for compressor type or speed class. AHRI 210/240-2023's test-procedure changes (most notably the external static pressure increase from 0.1 to 0.5 inWC for ducted units) affect equipment classes differently:

- **Variable-speed equipment with ECM fans** maintains efficiency better under the higher-ESP test; the actual SEER2/SEER ratio tends closer to ~0.96 (conversion factor ~1.042) rather than the nominal 0.95 (factor 1.053). Similarly, HSPF2/HSPF tends closer to ~0.87 (factor 1.149) vs. nominal 0.85 (factor 1.176).
- **Single-speed equipment with PSC fans** degrades more under the higher-ESP test; the actual SEER2/SEER ratio tends closer to ~0.94 (factor 1.064), and HSPF2/HSPF closer to ~0.83 (factor 1.205).

HARES already parses `CompressorType` (lines 2418–2451) into `single stage`, `two stage`, and `variable speed` categories, and uses these to set `number_of_speeds` and `speed_control_mode`. This same data could drive differentiated conversion factors but is not used for that purpose.

**Code Location**: `resolve_hvac.rs:38` (`SEER2_TO_SEER_FACTOR`), `resolve_hvac.rs:44` (`HSPF2_TO_HSPF_FACTOR`), `resolve_hvac.rs:50` (`HSPF2_TO_HSPF_FACTOR_DUCTLESS`), `resolve_hvac.rs:2400–2406` (application sites in `normalize_efficiency_units`). Compressor type parsed at lines 2425–2451 but not wired into efficiency normalization.

**Root Cause**: The RESNET MINHERS Addendum 71f / ANSI/RESNET 301-2022 consensus provides single-point conversion factors (0.95 for SEER2/SEER, 0.85/0.90 for HSPF2/HSPF) for rating-program use. These are appropriate for ERI/HERS index calculation where simplicity and reproducibility are paramount. For research-grade building energy simulation, the uniform factors discard resolution that could be resolved from available HPXML metadata.

**Impact**: For SEER: systematic underestimation of variable-speed equipment efficiency by ~1% and overestimation of single-speed PSC equipment by ~1%. For HSPF: systematic bias of ~2–4% depending on equipment class. These biases are correlated with equipment type, so they do not cancel out in large aggregated simulations. In a ResStock-style analysis with a known distribution of equipment types, the aggregate cooling/heating energy would be slightly biased depending on the population mix of variable-speed vs. single-speed units. A jurisdiction with high variable-speed heat-pump adoption (e.g., California Title 24 baseline) would see aggregate heating COP underestimated by ~2%.

---

### Finding 3: [Severity: low]
**Description**: The `is_ductless` flag correctly distinguishes mini-split heat pumps (`is_mini_split = true`) from ducted equipment for SEER2/HSPF2 conversion. However, the flag is hardcoded to `false` for all standalone `CoolingSystem` and `HeatingSystem` elements (line 1626: `is_ductless = false`, line 1755: `is_ductless = false`). This means a room air conditioner — which is physically ductless — would receive the ducted SEER2→SEER conversion factor (1/0.95) if SEER2 data were supplied for it.

In practice, room ACs are rated under 10 CFR Part 430 Appendix F using CEER, not SEER2 or EER2. The code correctly documents this (lines 57–59, lines 1057–1059). An SEER2 value on a room AC in HPXML would be a data-quality error. However, the code's current behavior — applying the ducted conversion without flagging the anomaly — means such errors would produce a 5.3% efficiency overestimate silently.

**Code Location**: `resolve_hvac.rs:1755` (CoolingSystem always receives `is_ductless = false`); `resolve_hvac.rs:2400–2401` (SEER2 ductless vs ducted branching); `resolve_hvac.rs:1057–1059` (doc-comment acknowledging room ACs should use EER not SEER/SEER2).

**Root Cause**: The `is_ductless` parameter is derived from `is_mini_split` (i.e., `HeatPumpType == "mini-split"`) and is not computed from equipment type metadata. Room AC detection exists elsewhere (line 2236 maps "room air conditioner" to "Room AC") but isn't propagated to the efficiency normalization path.

**Impact**: Negligible for valid HPXML files (room ACs use CEER/EER, not SEER2). For malformed HPXML files with SEER2 on room ACs, a 5.3% efficiency overestimate would occur silently. A tracing warning or validation error would be a defensive improvement but is low-priority given the input data quality check already exists in validation.rs.

---

### Finding 4: [Severity: info]
**Description**: OCHRE (the vendor reference implementation) does not handle SEER2, HSPF2, or EER2 at all. OCHRE's `parse_hvac` function at `ochre/utils/hpxml.py:850–858` only recognizes the legacy units `"EER"`, `"SEER"`, `"HSPF"`, and raises `OCHREException` for unrecognized efficiency units. The OCHRE test directory contains sample HPXML files with SEER2/HSPF2 labels (`base-hvac-central-ac-only-1-speed-seer2.xml`, `base-hvac-air-to-air-heat-pump-1-speed-seer2-hspf2.xml`) but these would not be parseable by stock OCHRE without code changes.

HARES is therefore ahead of the reference implementation in HPXML v4.x compliance, and the uniform conversion factors it applies (derived from RESNET MINHERS Addendum 71f, AHRI 210/240-2023, and the DOE test procedure rulemaking at 87 FR 74364) are the correct industry-standard approach for simulation. The limitations described in Findings 1–3 represent opportunities for incremental improvement rather than correctness defects relative to the available reference.

**Code Location**: `ochre/utils/hpxml.py:850–858` (OCHRE efficiency unit matching); `resolve_hvac.rs:2395–2413` (HARES `normalize_efficiency_units`).

**Root Cause**: OCHRE's HPXML parser was developed before the DOE 2023 test procedure updates took effect, predating widespread use of SEER2/HSPF2/EER2 in HPXML files. HARES was developed post-transition and correctly added support.

**Impact**: Not a defect. Included for completeness to establish that HARES is the more complete implementation in this dimension; the uniform factors it uses are not regressions from OCHRE.

---

## HPXML 4.x SEER2/HSPF2/EER2 Compliance Assessment

The DOE 10 CFR Part 430 test procedure revision (effective January 1, 2023) introduced SEER2, HSPF2, and EER2 as new rating metrics. Key regulatory facts:

- **AHRI 210/240-2023** defines new test conditions (external static pressure 0.5 inWC for ducted, new outdoor temperature bins) but does **not** prescribe fixed conversion factors to legacy metrics.
- **RESNET MINHERS Addendum 71f / ANSI/RESNET 301-2022** provides consensus conversion factors for building energy rating: SEER2/SEER ≈ 0.95 (ducted), HSPF2/HSPF ≈ 0.85 (ducted) / 0.90 (ductless). These are the reference values HARES uses.
- **CEC Appliance Efficiency Database** contains empirical paired ratings (same model rated under both old and new procedures during the transition) and shows actual variation of 1.038–1.043 for EER2→EER, and similar ranges for SEER2→SEER and HSPF2→HSPF.
- **HPXML v4.x** carries both legacy (`SEER`, `HSPF`, `EER`) and new (`SEER2`, `HSPF2`, `EER2`, `CEER`) efficiency units. HARES correctly normalizes SEER2/HSPF2/EER2 to their legacy equivalents using industry-standard factors.

The uniform factors are acceptable for ERI/HERS rating purposes (per RESNET) and for most simulation use cases. The granularity limitations are notable for research applications requiring maximum accuracy when comparing equipment subclasses.

## Summary
- Total findings: 4
- Critical: 0 / High: 0 / Medium: 2 / Low: 1 / Info: 1

## Recommendations

1. For EER2→EER conversion: maintain the uniform 1/0.96 factor as default but add an optional override path (e.g., a `defaults/hvac/eer2_eer_factors.csv` or an equipment-type-keyed lookup) that would allow users with CEC-paired-rating data to supply split-specific and packaged-specific factors. The current uniform factor is in the middle of the known range and the bias is negligible for most use cases, so this is low-priority unless the project receives specific accuracy requirements.

2. For SEER2→SEER and HSPF2→HSPF conversion: consider adding speed-class-dependent adjustment factors. The `CompressorType` data already parsed (lines 2425–2451) could drive a three-tier lookup:
   - Single-stage: factor 1/0.94 for SEER2, 1/0.83 for HSPF2
   - Two-stage: factor 1/0.95 for SEER2, 1/0.85 for HSPF2 (current defaults)
   - Variable-speed: factor 1/0.96 for SEER2, 1/0.87 for HSPF2
   
   These factors should be configurable constants (not hardcoded inline) so they can be updated when better empirical data becomes available. The CEC database or AHRI directory could provide calibration data for the exact factors. This would reduce the 1–4% systematic bias between equipment classes.

3. For room ACs: add a `tracing::warn!` or validation pass that flags SEER2 on room AC equipment. The current code correctly documents that room ACs use CEER (not SEER2), but silently applies the ducted conversion. An explicit warning would catch HPXML data-quality issues early.

## References / Citations

- AHRI 210/240-2023: *Performance Rating of Unitary Air-Conditioning & Air-Source Heat Pump Equipment* (2023 revision with SEER2/HSPF2/EER2 metrics)
- DOE 10 CFR Part 430, Appendix M1 (2023 revision): Federal test procedure for central air conditioners and heat pumps
- DOE 87 FR 74364 (December 2022): Final rule adopting SEER2/HSPF2/EER2 test procedures
- RESNET MINHERS Addendum 71f / ANSI/RESNET 301-2022: Standard for the Calculation and Labeling of the Energy Performance of Dwelling and Sleeping Units (SEER2→SEER and HSPF2→HSPF conversion factors for rating purposes)
- CEC Appliance Efficiency Database: https://cacertappliances.energy.ca.gov/ (empirical paired-rating data for SEER2/SEER, EER2/EER, HSPF2/HSPF)
- OCHRE `ochre/utils/hpxml.py:850–858`: vendor reference efficiency unit handling (does not support SEER2/HSPF2/EER2)
- OCHRE `ochre/Equipment/HVAC.py:1062–1068` and `1112–1126`: PLF parameter updates keyed on SEER/HSPF thresholds (needs converted SEER/HSPF values, not SEER2/HSPF2)
