# Boiler auxiliary hours/year (2080) — verify against ANSI/RESNET/ICC 301-2019 Eq. 4.4-5
**Review ID**: phys-const-06
**Category**: physics-constants
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/constants.rs:161-168`
- `crates/hares-io/src/hpxml/resolve_hvac.rs:1647-1658` (usage site)
- `crates/hares-io/src/hpxml/resolve_hvac.rs:5158-5208` (unit test)

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/utils/hpxml.py:889-892` — OCHRE uses identical 2080-hour divisor, citing same standard equation
- `vendors/OCHRE/ochre/Equipment/HVAC.py:148` — downstream consumer of auxiliary power
- `vendors/EnergyPlus/src/EnergyPlus/Boilers.cc:317-319, 441-458, 1004-1008, 1045-1046` — EnergyPlus boiler parasitic electric model (no fixed annual hours)
- `vendors/EnergyPlus/src/EnergyPlus/Boilers.hh:114-142` — EnergyPlus boiler data members
- OpenStudio-HPXML `hvac.rb` (NREL/NatLabRockies repo, master) — `get_default_boiler_eae` function no longer present; may have been removed/refactored

## Findings

### Finding 1: [Severity: low]
**Description**: The constant `BOILER_AUXILIARY_HOURS_PER_YEAR = 2_080.0` at `constants.rs:168` is consistent with the OCHRE vendored reference implementation (`hpxml.py:889-892`), which uses the same divisor (2080) and cites the same standard equation (ANSI/RESNET/ICC 301-2019 Eq. 4.4-5). However, the canonical standard document is a paid publication and could not be independently verified in this review. The value `2080` is numerically `40 hours/week × 52 weeks/year` (standard full-time equivalent hours), representing boiler pump/control operation during the heating season rather than year-round continuous duty.

**Code Location**: `crates/hares-physics/src/constants.rs:168`
**Root Cause**: The constant is derived from the ANSI/RESNET/ICC 301-2019 standard equation 4.4-5 and the ResStock/OCHRE convention. The doc comment at lines 161-167 clearly documents the provenance.
**Impact**: Minimal. The value is cross-validated by OCHRE's identical usage, and the unit test at `resolve_hvac.rs:5158-5208` confirms correct arithmetic: `ElectricAuxiliaryEnergy = 2080 kWh/yr → 2080 / 2080 × 1000 = 1000.0 W`. If the standard were revised to a different value, both HARES and OCHRE would need updating.

### Finding 2: [Severity: low]
**Description**: EnergyPlus (`Boilers.cc`) does not use a fixed annual-hours convention for boiler auxiliary electric loads. Instead, boiler parasitic electric power is modeled as `ParasiticElecPower = ParasiticElecLoad × BoilerPLR` (on-cycle, proportional to part-load ratio), accumulated per timestep (`ParasiticElecConsumption = ParasiticElecPower × ReportingConstant`). This is a fundamentally different modeling approach from the HPXML/ResStock convention of converting annual kWh via a fixed-hours divisor. The HARES implementation follows the HPXML pathway, which is appropriate for an HPXML-driven tool.

**Code Location**: `vendors/EnergyPlus/src/EnergyPlus/Boilers.cc:1004-1008, 1045-1046` vs `crates/hares-io/src/hpxml/resolve_hvac.rs:1647-1658`
**Root Cause**: Different modeling paradigms: EnergyPlus performs sub-hourly simulation with explicit parasitic load tracking; HPXML-derived tools use annual energy totals divided by fixed operating hours.
**Impact**: No direct impact on HARES correctness. Noted for awareness when comparing HARES outputs against EnergyPlus simulation results.

### Finding 3: [Severity: low]
**Description**: The OCHRE vendored code references `get_default_boiler_eae` at `hvac.rb:1754` in the ResStock codebase. This function no longer exists in the current OpenStudio-HPXML master branch (`hvac.rb`). The removal may indicate the 2080-hour convention has been superseded or relocated in recent ResStock/OpenStudio-HPXML versions. The HARES doc comment at `constants.rs:166` references the same (possibly stale) function location.

**Code Location**: `crates/hares-physics/src/constants.rs:166`
**Root Cause**: Out-of-date reference to a removed vendor function location.
**Impact**: Low. The doc comment reference may be misleading; recommend updating or removing the OCHRE `hvac.rb:1754` reference if it no longer exists in the upstream source.

## Summary
- Total findings: 3
- Critical / High / Medium / Low: 0 / 0 / 0 / 3

## Recommendations
1. Verify the constant value (2080) against the actual ANSI/RESNET/ICC 301-2019 or 301-2022 standard text if access to the document becomes available. The value could not be independently confirmed in this review; the standard is a paid publication.
2. Update or remove the doc comment reference to `OCHRE hvac.rb:1754` at `constants.rs:166`, as the `get_default_boiler_eae` function no longer exists in the current OpenStudio-HPXML master branch.
3. Consider citing a stable, publicly available reference (e.g., the ResStock/OpenStudio-HPXML GitHub tag or commit hash) rather than a mutable line number.
4. No code changes required. The constant value and its usage are internally consistent, cross-validated against the OCHRE vendored reference, and verified by the existing unit test.

## References / Citations
- `crates/hares-physics/src/constants.rs:161-168` — constant definition and provenance
- `crates/hares-io/src/hpxml/resolve_hvac.rs:1647-1655` — boiler-specific hours divisor application
- `crates/hares-io/src/hpxml/resolve_hvac.rs:5158-5208` — unit test `boiler_electric_auxiliary_energy_uses_2080_hour_divisor`
- `vendors/OCHRE/ochre/utils/hpxml.py:889-892` — OCHRE identical 2080-hour divisor with same standard citation
- `vendors/EnergyPlus/src/EnergyPlus/Boilers.cc:1004-1008` — EnergyPlus parasitic electric model (PLR-based, no fixed annual hours)
- ANSI/RESNET/ICC 301-2019 Equation 4.4-5 (cited by both HARES and OCHRE; document not available for direct verification)
- ResStock convention / OCHRE `get_default_boiler_eae` reference (function no longer present in OpenStudio-HPXML master `hvac.rb`)
