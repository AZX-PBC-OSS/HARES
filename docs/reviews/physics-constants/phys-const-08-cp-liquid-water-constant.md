# CP_LIQUID_WATER_J_KG_K (4180) — verify against EnergyPlus Psychrometrics.hh CPHW; NIST gives 4182-4186 for 15-20°C
**Review ID**: phys-const-08
**Category**: physics-constants
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/constants.rs:86` — `CP_LIQUID_WATER_J_KG_K = 4_180.0`
- `crates/hares-physics/src/constants.rs:70-86` — doc comment block

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/Psychrometrics.hh:1609-1633` — `CPCW()` and `CPHW()` functions, both return `4180.0`
- `vendors/EnergyPlus/src/EnergyPlus/RuntimeLanguageProcessor.cc:2304,2309` — confirms CPCW = 4180.d0, CPHW = 4180.d0
- `vendors/OCHRE/ochre/Models/Water.py:10` — `water_cp = 4.183` kJ/kg·K (= 4183 J/kg·K)
- `vendors/OCHRE/ochre/utils/warmup_jit.py:122,127` — warmup call using `4186.0` J/kg·K

## Findings

### Finding 1: HARES 4180 matches EnergyPlus exactly — no deviation from primary reference [Severity: low]
**Description**: The HARES `CP_LIQUID_WATER_J_KG_K` constant (4180.0) is an exact match for EnergyPlus `CPHW`/`CPCW`, which both return `4180.0` since April 1992 (author Russell D. Taylor).

**Code Location**: `crates/hares-physics/src/constants.rs:86` ↔ `vendors/EnergyPlus/src/EnergyPlus/Psychrometrics.hh:1619,1632`

**Root Cause**: N/A — intentional design choice for consistency with EnergyPlus.

**Impact**: Zero divergence from the primary building energy simulation reference. EnergyPlus itself uses 4180 across all water heating calculations (WaterUse, WaterThermalTanks, DXCoils, VariableSpeedCoils, NodeInputManager, IceThermalStorage, etc.). HARES will produce identical energy balance results to EnergyPlus for water-based systems.

### Finding 2: -0.05% to -0.14% deviation from NIST reference values [Severity: low]
**Description**: NIST reference data gives water specific heat of 4182 J/(kg·K) at 20°C and 4186 J/(kg·K) at 15°C. HARES' 4180 represents an under-estimate of 2–6 J/(kg·K), or -0.05% to -0.14%.

**Code Location**: `crates/hares-physics/src/constants.rs:83-86`

**Root Cause**: EnergyPlus (and ASHRAE HoF) use the round value 4180 as a fixed engineering approximation for the full domestic hot water temperature range, rather than a temperature-dependent polynomial.

**Impact**: Negligible for building energy simulation. At a 55°C temperature rise (e.g., 5°C mains to 60°C DHW), the energy error using 4180 vs. the NIST 4182-4186 range is <0.1%. This is far below instrument accuracy, envelope load uncertainty, and weather data error bars typical in whole-building simulation.

### Finding 3: OCHRE uses 4183 kJ/kg·K — a closer NIST match but minor divergence from HARES [Severity: low]
**Description**: OCHRE's `water_cp = 4.183` kJ/kg·K (`ochre/Models/Water.py:10`) = 4183 J/(kg·K), which is closer to the NIST 20°C value (~4182). OCHRE's JIT warmup code also passes `4186.0` as a parameter, matching the NIST 15°C value. HARES uses 4180 for EnergyPlus compatibility; OCHRE uses a slightly different constant.

**Code Location**: `crates/hares-physics/src/constants.rs:86` vs `vendors/OCHRE/ochre/Models/Water.py:10`

**Root Cause**: Different reference standards — HARES targets EnergyPlus compatibility, OCHRE appears to target NIST values.

**Impact**: Low. The 3 J/(kg·K) difference between HARES (4180) and OCHRE (4183) maps to a ~0.07% energy difference. Cross-comparison between tools would not reveal this as meaningful given other modeling differences.

## Summary
- Total findings: 3
- Critical / High / Medium / Low: 0 / 0 / 0 / 3

## Recommendations
1. **No change needed.** The constant `CP_LIQUID_WATER_J_KG_K = 4180.0` is correct for the stated purpose: consistency with EnergyPlus and ASHRAE HoF building energy simulation conventions. The existing doc comment at `constants.rs:76-85` already thoroughly documents the rationale, the NIST reference range, and the deviation.
2. If HARES later adopts temperature-dependent water properties (e.g., for high-accuracy DHW stratification models), the NIST polynomial at 15-20°C could be used. EnergyPlus itself does not do this for water specific heat (though it does temperature-dependent density via `RhoH2O`), suggesting this is not necessary for the current simulation fidelity target.

## References / Citations
- EnergyPlus Psychrometrics.hh, `CPHW` function (line 1622-1633): both chilled and hot water specific heat = 4180.0 J/(kg·K), authored April 1992, Russell D. Taylor
- EnergyPlus RuntimeLanguageProcessor.cc (line 2304, 2309): documents CPCW/CPHW = 4180.d0
- OCHRE Water.py (line 10): `water_cp = 4.183` kJ/kg·K (= 4183 J/kg·K)
- ASHRAE Handbook of Fundamentals, 2021: round value 4180 J/(kg·K) for water specific heat in building energy calculations
- NIST Chemistry WebBook, SRD 69: water isobaric specific heat 4182 J/(kg·K) at 20°C, 4186 J/(kg·K) at 15°C
