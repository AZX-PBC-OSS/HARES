# Appliance default energy values: source citation and coverage
**Review ID**: equip-loads-05
**Category**: equipment-loads
**Date**: 2026-05-26

## Files Reviewed
crates/hares-io/src/hpxml/resolve_loads.rs

## Vendor/Reference Files Consulted
vendors/OCHRE/ochre/utils/hpxml.py

## Findings

### Finding 1: Missing explicit RESNET standard citation for appliance defaults [Severity: medium]
**Description**: The default `RatedAnnualkWh` values for major appliances (clothes washer=400, dishwasher=467, refrigerator=637+18\*beds, freezer=319.8) are used without any source citation beyond an inline comment that says "OCHRE falls back to these when HPXML omits the element." The OCHRE source file explicitly comments "From ResStock, using ERI Version >= '2019A'" for each appliance, which ties the defaults to ANSI/RESNET 301-2019. HARES does not state which RESNET edition the defaults originate from, and the only RESNET citation in the file references an older edition (ANSI/RESNET 301-2014 §4.2.2.5.2.7) for the dryer exhaust fraction — a different concern.
**Code Location**: `crates/hares-io/src/hpxml/resolve_loads.rs:101-108`
**Root Cause**: Default values were ported from OCHRE without carrying forward the OCHRE source comments identifying the standard edition.
**Impact**: Future maintainers cannot easily determine whether defaults should be updated when a new RESNET standard is published, risking drift from the normative reference.

### Finding 2: Refrigerator and freezer defaults ignore UsageMultiplier [Severity: high]
**Description**: When the HPXML file omits `AdjustedAnnualkWh` and `RatedAnnualkWh` for refrigerators and freezers, HARES falls back to the hardcoded defaults (`637.0 + 18.0 * n_bedrooms` for refrigerators, `319.8` for freezers) but does **not** apply the `UsageMultiplier`. In OCHRE, every path for refrigerator and freezer energy includes the multiplier:

- OCHRE `parse_refrigerator` (line 1394): `r_energy = (637.0 + 18.0 * n_bedrooms) * multiplier`
- OCHRE `parse_freezer` (line 1420): `annual_kwh = 319.8 * multiplier`

In HARES, the refrigerator and freezer fall into the wildcard match arm `_ => {}` (line 358), which performs no post-processing. The `UsageMultiplier` is extracted by `parse_schedule_extension_params` (lines 361–363) and added to `params`, but is never applied to `annual_electric_kwh`. In contrast, clothes washer, dishwasher, dryer, and cooking range all explicitly multiply energy by the usage multiplier in their match-arm post-processing.
**Code Location**: `crates/hares-io/src/hpxml/resolve_loads.rs:104-116` (default assignment), `resolve_loads.rs:358` (wildcard arm bypasses multiplier), `resolve_loads.rs:361-363` (multiplier extracted but unused for fridge/freezer)
**Root Cause**: The refrigerator and freezer tags are not given their own match-arm post-processing block, unlike the other appliances. The wildcard `_ => {}` on line 358 silently drops the multiplier.
**Impact**: When an HPXML dataset sets `UsageMultiplier` in the appliance extension (common in ResStock parametric runs), refrigerator and freezer energy will be undercounted by a factor of `UsageMultiplier`. For ResStock baseline multipliers of 1.0 this has no effect, but for regionally-scaled multipliers it introduces systematic error.

### Finding 3: Refrigerator default formula uses RESNET 301-2019 coefficients; 2022 edition may differ [Severity: medium]
**Description**: The refrigerator default formula `637.0 + 18.0 * n_bedrooms` matches the ANSI/RESNET 301-2019 (ERI "2019A") equation. ANSI/RESNET 301-2022 Appendix B was published with updated reference home specifications that reflect more recent federal minimum appliance efficiency standards. Similarly, the freezer default of `319.8` is a flat constant — in RESNET 301-2019, the "second refrigerator" allowance was 319.8 kWh/year, but newer editions may have updated this value or adopted a bedroom-dependent formula similar to the primary refrigerator.
**Code Location**: `crates/hares-io/src/hpxml/resolve_loads.rs:105-106`
**Root Cause**: The codebase aligns with the OCHRE "ERI Version >= '2019A'" baseline rather than RESNET 301-2022.
**Impact**: Using 2019-era coefficients may overestimate reference appliance energy consumption relative to updated federal standards, potentially skewing ERI scores and compliance ratings for new construction modeled under the 2022 standard.

### Finding 4: Default cooking range energy correctly accounts for electric vs. gas fuel [Severity: low]
**Description**: The cooking range default calculation (lines 304–328) correctly branches on fuel type:
- **Electric**: `331.0 + 39.0 * n_bedrooms` kWh/year
- **Gas**: `22.6 + 2.7 * n_bedrooms` kWh/year electric parasitic + `22.6 + 2.7 * n_bedrooms` therms/year combustion

It also correctly adjusts burner efficiency for induction ranges (`0.91` factor). These formulas match OCHRE's `parse_cooking_range` exactly (lines 1454–1459). However, the code does not apply a `UsageMultiplier` in the pre-populated `annual_electric_kwh` path (i.e., when HPXML explicitly provides the kWh/therm values via `child_load_kwh` / `child_load_therms`), only in the default-computation path (the `if !contains_key(...)` guard). This is consistent with HPXML convention where `Load` elements represent total annual energy, but it means the multiplier is only used when falling back to the bedroom-based formula.
**Code Location**: `crates/hares-io/src/hpxml/resolve_loads.rs:304-328`
**Root Cause**: Design choice matching OCHRE behavior.
**Impact**: Minimal; the multiplier is an extension parameter intended to scale the formula, not an HPXML-provided `Load` element.

### Finding 5: Dryer gas sensible gain factor is a static constant instead of BTU-weighted blend [Severity: low]
**Description**: HARES uses a pre-computed constant `DRYER_GAS_SENSIBLE_GAIN = 0.89` for the sensible heat gain fraction of gas clothes dryers. The comment (line 26) explains this is an approximation. OCHRE computes the gas dryer sensible gain fraction dynamically as a BTU-weighted blend:
```python
frac_sens = (1.0 - frac_lost) * ((0.90 * elec_btu + 0.8894 * gas_btu) / (elec_btu + gas_btu))
```
This blend uses the actual computed electric and gas contributions for the specific home, producing a value that can differ from the fixed 0.89 when the electric/gas split deviates from the assumed ~7%/93%. The discrepancy is small for most homes (OCHRE's own comment states the split is "stable across CEF values") but the approximation is not documented as such.
**Code Location**: `crates/hares-io/src/hpxml/resolve_loads.rs:25-28`, `resolve_loads.rs:204-205`
**Root Cause**: Pre-computation optimization that does not recalculate the weighted blend with actual energy values.
**Impact**: Negligible for most homes given the stable split; potential 1–2% deviation in sensible gain fraction for extreme cases.

### Finding 6: Dryer default washer parameters are hardcoded, not read from the parsed washer [Severity: low]
**Description**: When the HPXML omits `CombinedEnergyFactor`, `EnergyFactor`, and `RatedAnnualkWh` for the clothes dryer, HARES falls back to a default calculation that uses hardcoded washer parameters (`washer_rated_kwh=400`, `washer_imef=1.0`, `washer_capacity_ft3=3.0`) extracted at lines 76–84 from the `ClothesWasher` XML node. These values are correctly read from the actual parsed washer node, which matches OCHRE's approach of passing the washer dict to `parse_clothes_dryer`. However, the washer extraction at lines 76–84 uses `child_f64` which returns the raw value, not the post-processed (multiplied/adjusted) value that would be computed in the clothes washer match arm. This means the dryer default uses pre-processed washer numbers, which is actually correct per OCHRE since OCHRE passes the raw `clothes_washer` dict (with `get("RatedAnnualkWh", 400.0)`) into `parse_clothes_dryer`.
**Code Location**: `crates/hares-io/src/hpxml/resolve_loads.rs:76-84`, `resolve_loads.rs:235-258`
**Root Cause**: Architectural match with OCHRE's design.
**Impact**: Correct behavior — verified against OCHRE's reference implementation.

## Summary
- Total findings: 6
- Critical: 0 / High: 1 / Medium: 2 / Low: 3

## Recommendations
1. **Apply UsageMultiplier to refrigerator and freezer defaults** (Finding 2 — High). Add a match arm for `"Refrigerator"` and `"Freezer"` that reads the `UsageMultiplier` from the extension and scales `annual_electric_kwh` when it was derived from the default or `RatedAnnualkWh` (but not from `AdjustedAnnualkWh`, which already includes the multiplier per HPXML convention).

2. **Add explicit RESNET standard citations** (Finding 1 — Medium). Document in the code comment on lines 100–108 that the default values originate from ANSI/RESNET 301-2019 (ERI "2019A") as implemented in ResStock/OCHRE. Consider adding a doc comment on the `DRYER_EXHAUST_FRACTION_VENTED` constant clarifying that the 2014 reference is specifically for the exhaust fraction value (which has remained stable across editions).

3. **Evaluate migration to RESNET 301-2022 default coefficients** (Finding 3 — Medium). Compare the 2019-era default coefficients against the 2022 edition and determine whether HARES should adopt the newer reference home appliance values. This is particularly relevant if HARES is expected to model homes against current RESNET rating procedures.

4. **Document the dryer sensible gain approximation** (Finding 5 — Low). Clarify in the comment on line 28 that `DRYER_GAS_SENSIBLE_GAIN` is a fixed approximation of the OCHRE dynamic weighted blend, noting the assumed electric/gas split (~7%/93%) and the conditions under which the approximation may introduce error.

## References / Citations
- OCHRE `parse_clothes_washer`: `vendors/OCHRE/ochre/utils/hpxml.py:1243-1278`
- OCHRE `parse_clothes_dryer`: `vendors/OCHRE/ochre/utils/hpxml.py:1281-1333`
- OCHRE `parse_dishwasher`: `vendors/OCHRE/ochre/utils/hpxml.py:1336-1370`
- OCHRE `parse_refrigerator`: `vendors/OCHRE/ochre/utils/hpxml.py:1373-1410`
- OCHRE `parse_freezer`: `vendors/OCHRE/ochre/utils/hpxml.py:1413-1432`
- OCHRE `parse_cooking_range`: `vendors/OCHRE/ochre/utils/hpxml.py:1435-1483`
- ANSI/RESNET 301-2019: Standard for the Calculation and Labeling of the Energy Performance of Dwelling and Sleeping Units Using an Energy Rating Index
- ANSI/RESNET 301-2022: Updated edition (Appendix B — Reference Home specifications)
- DOE Appliance Standards Rulemaking: 10 CFR Parts 429–430 (refrigerator, freezer, dishwasher, clothes washer, clothes dryer federal minimum standards)
