# Dry air density formula: verify p/(R_da·T) and ISA sea-level 15°C reference density 1.2250 kg/m³
**Review ID**: air-03
**Category**: air-properties
**Date**: 2026-05-26

## Files Reviewed
crates/hares-physics/src/air_properties.rs:29-31

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/Psychrometrics.hh:547-572` — `PsyRhoAirFnPbTdbW` constexpr dry-bulb density
- `vendors/EnergyPlus/src/EnergyPlus/DataGlobalConstants.hh:611` — `Constant::Kelvin = 273.15`
- `vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc:4467-4469` — `StdRhoAir` initialisation at 20°C from `StdBaroPress`
- `vendors/EnergyPlus/src/EnergyPlus/DataEnvironment.hh:170` — `StdRhoAir` definition
- `vendors/OCHRE/ochre/utils/psychrolib_jit.py:11,96-108` — `R_DA_SI = 287.042`, `get_moist_air_density`, `get_moist_air_volume`
- `vendors/OCHRE/ochre/Models/Humidity.py:60-65` — `get_dry_air_density` via psychrolib
- `crates/hares-physics/src/constants.rs:20` — `DRY_AIR_GAS_CONSTANT_J_KG_K = 287.058`
- `crates/hares-physics/src/constants.rs:224` — `CELSIUS_TO_KELVIN = 273.15`

## Findings

### Finding 1: [Severity: low]
**Description**: Three implementations use three different values for the dry-air gas constant.
**Code Location**:
  - HARES: `crates/hares-physics/src/constants.rs:20` → `R_da = 287.058` (ASHRAE 2017 HoF Ch.1)
  - EnergyPlus: `Psychrometrics.hh:571` → `R = 287.0` (Wright 1994, citing ASHRAE 1985 HoF)
  - OCHRE: `psychrolib_jit.py:11` → `R_DA_SI = 287.042` (psychrolib's own constant)
**Root Cause**: Each codebase cites a different authoritative source. The ASHRAE Handbook has revised the constant over editions (1985 → 2017), and psychrolib uses its own derivation. None of the three values is *wrong* per se.
**Impact**: At ISA sea-level (p = 101325 Pa, T = 288.15 K), the computed dry-air density is:
  - HARES: 101325 / (287.058 × 288.15) = **1.22497 kg/m³**
  - EnergyPlus: 101325 / (287.0 × 288.15) = **1.22523 kg/m³**
  - OCHRE: 101325 / (287.042 × 288.15) = **1.22508 kg/m³**
  - ISA 1976 reference: **1.2250 kg/m³**

  The maximum spread is ~0.0003 kg/m³ (0.02%). This is negligible for all building-energy applications. The HARES value is the most precisely documented (with an explanatory note distinguishing ASHRAE from NIST CODATA in `constants.rs:17-19`).

### Finding 2: [Severity: low]
**Description**: The test `isa_sea_level_dry_air_density` expects `1.2250 ± 0.0005`, which passes correctly but the test comment and the computed value are slightly inconsistent due to the chosen R_da.
**Code Location**: `crates/hares-physics/src/air_properties.rs:117-124`
**Root Cause**: The test uses the rounded ISA 1976 canonical value `1.2250` as the expected value. However, the formula produces `1.22497` with `R_da = 287.058`. The tolerance `±0.0005` is generous enough to absorb this difference (`|1.22497 − 1.2250| = 0.00003`).
**Impact**: The test passes but would benefit from documenting the expected value as the *formula-computed* value rather than the rounded ISA reference. A self-consistency check (e.g., computing `ρ_expected = 101325.0 / (287.058 × 288.15)` inline) would be more precise and would flag accidental constant changes.

### Finding 3: [Severity: high] (Design — positive finding)
**Description**: HARES cleanly separates dry-air density (`dry_air_density_kg_m3`, line 29-31) from moist-air density (`moist_air_density_kg_m3`, line 21-26). The dry function applies **only** `p / (R_da·T)` — no humidity term.
**Code Location**: `crates/hares-physics/src/air_properties.rs:29-31`
**Root Cause**: Intentional design choice, not a bug.
**Impact**: By contrast, EnergyPlus's `PsyRhoAirFnPbTdbW` always applies the humidity factor `(1.0 + 1.6077687 × max(w, 1e-5))`, so even with `w = 0` the denominator is scaled by `1.000016`. HARES's `dry_air_density_kg_m3` avoids this spurious correction and yields the exact ideal-gas-law density. The typed wrapper `dry_air_density` (line 51-53) correctly delegates to this function. This is a clean, correct design.

## Summary
- Total findings: 3
- High: 1 (positive design finding)
- Medium: 0
- Low: 2

## Recommendations
1. The R_da = 287.058 constant and the formula `p / (R_da × T)` at `air_properties.rs:29-31` are **verified correct**. No changes needed.
2. The test at `air_properties.rs:117-124` could be tightened by using a self-consistency check: `let expected = 101325.0 / (287.058 × (15.0 + 273.15));` rather than the hardcoded `1.2250`. This would guard against accidental constant drift.
3. Document in the test comment that the 1.2250 value is the rounded ISA 1976 reference and that the actual formula-computed value is ~1.22497 with the ASHRAE 2017 R_da.

## References / Citations
- ISA 1976 / ICAO Doc 7488: standard sea-level density 1.2250 kg/m³ at p₀ = 101325 Pa, T₀ = 288.15 K
- ASHRAE 2017 Handbook of Fundamentals, Ch. 1: `R_da = 287.058 J/(kg·K)`
- ASHRAE 1985 Handbook of Fundamentals, Ch. 6 (cited by EnergyPlus `PsyRhoAirFnPbTdbW`): `R = 287 J/(kg·K)`
- NIST CODATA 2018: `R*/M_air = 287.055 J/(kg·K)` (for comparison — HARES explicitly notes this deviation in `constants.rs:17`)
- OCHRE psychrolib: `R_DA_SI = 287.042` (source not documented in-code)
