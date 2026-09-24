# Moist air density formula: verify p/(R_da·T·(1+1.6077687·W)) against ASHRAE HOF 2021 Ch.1 Eq.28; check humidity floor guard (1e-5)
**Review ID**: air-02
**Category**: air-properties
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/air_properties.rs:21-26` — `moist_air_density_kg_m3()` implementation
- `crates/hares-physics/src/air_properties.rs:55-162` — tests
- `crates/hares-physics/src/constants.rs:20` — `DRY_AIR_GAS_CONSTANT_J_KG_K = 287.058`
- `crates/hares-physics/src/constants.rs:34` — `MOLECULAR_WEIGHT_RATIO_WATER_AIR = 0.621_945`
- `crates/hares-physics/src/constants.rs:38` — `HUMIDITY_DENSITY_CORRECTION = 1.607_768_7`
- `crates/hares-physics/src/constants.rs:115-117` — `MIN_HUMIDITY_RATIO_DENSITY = 1e-5`

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/Psychrometrics.hh:538,571,582` — `PsyRhoAirFnPbTdbW` (three overloads)
- `vendors/OCHRE/ochre/utils/psychrolib_jit.py:6,11,96-109` — `get_moist_air_volume`, `get_moist_air_density`, `MIN_HUM_RATIO`

## Findings

### Finding 1: [Severity: low]
**Description**: The humidity density correction factor `1.6077687` differs from the published ASHRAE HOF 2021 Ch.1 Eq.28 value of `1.607858`. HARES derives its value as `1 / ε = 1 / 0.621945 ≈ 1.6077687`, which is mathematically consistent with its declared molecular masses (`M_da = 28.96546`, `M_w = 18.015268` from ASHRAE Eq. 11/20). ASHRAE's Equation 28 itself uses `1.607858` (based on `28.9645 / 18.01534`).  
**Code Location**: `crates/hares-physics/src/constants.rs:38` and `crates/hares-physics/src/air_properties.rs:25`  
**Root Cause**: HARES uses the inverse of epsilon (ε = 0.621945) to compute the enhancement factor rather than adopting the published Eq.28 constant directly. Different editions/tables of ASHRAE give slightly different molecular mass pairs.  
**Impact**: The relative difference between `1.6077687` and `1.607858` is `~5.5×10⁻⁵`. For typical indoor humidity ratio `W = 0.010 kg/kg`, the density correction term `(1 + 1.6078×W)` differs by `~9×10⁻⁷` relative, yielding a density difference of `~0.001%`. This is negligible for building energy simulation. EnergyPlus uses exactly the same value (`1.6077687`), while OCHRE uses `1.607858` (the ASHRAE Eq.28 published value).

### Finding 2: [Severity: low]
**Description**: The humidity floor guard `MIN_HUMIDITY_RATIO_DENSITY = 1e-5` is 100× larger than OCHRE's floor (`1e-7`), though identical to EnergyPlus. This prevents division-by-zero correctly without meaningfully distorting valid low-humidity physics.  
**Code Location**: `crates/hares-physics/src/constants.rs:117` applied at `crates/hares-physics/src/air_properties.rs:22`  
**Root Cause**: HARES follows the EnergyPlus convention exactly: both floor at `1e-5` and both use `w.max(floor)`. OCHRE uses a tighter `1e-7`.  
**Impact**: At `W = 1×10⁻⁵`, the humidity correction factor is `1.000016`, deviating from unity by `0.0016%`. At `W = 1×10⁻⁶` (valid desert conditions), the floor overrides this to `1×10⁻⁵`, adding a density error of `~0.0014%`. For `W ≥ 0.001 kg/kg` (typical building interior), the guard is `≥100×` below the actual value and has zero impact. The guard successfully prevents division-by-zero (the denominator is `R_da × T × (1 + 1.60777 × 0)` which is always finite; the guard prevents the pathological case of zero-pressure plus negative/zero humidity producing a sign change in the denominator). No practical simulation scenario is affected.

### Finding 3: [Severity: low]
**Description**: The test `dry_and_moist_density_are_consistent_at_low_humidity` uses `w = 1e-6` to verify consistency between dry and moist density, but the humidity floor guard converts this to `w_eff = 1e-5`. The test passes, but it does not exercise the `1e-6` input value — it unknowingly tests the floor guard behavior.  
**Code Location**: `crates/hares-physics/src/air_properties.rs:87-94`  
**Root Cause**: The test inputs `1e-6` which is below the `1e-5` floor; the code silently clamps it. The test's intent (validating low-humidity consistency) is still satisfied because `1e-5` is also low humidity, but the test is mildly misleading.  
**Impact**: No functional impact. The test correctly demonstrates that moist air density approximates dry air density at low humidity. Recommend either changing the test input to `1e-5` (matching the floor) or adding a comment noting the clamping.

### Finding 4: [Severity: low]
**Description**: `DRY_AIR_GAS_CONSTANT_J_KG_K = 287.058` differs from EnergyPlus (`287.0`) and OCHRE (`287.042`). HARES explicitly documents this as the ASHRAE 2017 value (vs. NIST CODATA `287.055`) and the choice is internally consistent with the molecular mass pair used elsewhere.  
**Code Location**: `crates/hares-physics/src/constants.rs:20` and `crates/hares-physics/src/air_properties.rs:23`  
**Root Cause**: ASHRAE has used multiple R_da values across editions (287.058 vs. 287.042). EnergyPlus rounds to `287.0` for simplicity.  
**Impact**: The difference between `287.058` and `287.042` is `~5.6×10⁻⁵` relative, translating to the same relative density difference. For a typical reference density of `1.2 kg/m³`, this is `~0.00007 kg/m³`. Negligible. HARES's choice is well-documented and consistent with its other constants.

## Cross-Reference Comparison

| Component | HARES | EnergyPlus | OCHRE |
|---|---|---|---|
| R_da [J/(kg·K)] | 287.058 | 287.0 | 287.042 |
| Humidity enhancement factor | 1.6077687 | 1.6077687 | 1.607858 |
| Humidity floor guard | `1e-5` | `1e-5` (max) | `1e-7` |
| Formula structure | `p / (R·T·(1+f·W))` | `p / (R·T·(1+f·W))` | `(1+W)·p / (R·T·(1+f·W))` |
| Density basis | kg_da/m³ (dry-air basis) | kg_da/m³ (dry-air basis) | kg_moist/m³ (total mass) |

**Note on OCHRE's formula**: OCHRE computes total moist-air density `ρ_moist = (1+W)·p/(R·T·(1+1.607858·W))`, whereas HARES and EnergyPlus compute dry-air-basis density `ρ_da = p/(R·T·(1+1.6077687·W))`. The two are related by `ρ_moist = ρ_da × (1+W)`. HARES's approach is correct for its declared purpose (kg_da/m³, as documented at `air_properties.rs:16-20`).

## Summary
- **Total findings**: 4
- **Critical / High / Medium / Low**: 0 / 0 / 0 / 4

## Recommendations
1. Consider adopting the ASHRAE HOF 2021 Eq.28 published factor `1.607858` directly instead of deriving `1.6077687` from `1/ε`, to eliminate the small numerical inconsistency. The practical impact is negligible, but it would align with the equation reference claimed in the doc comment.
2. Add a comment at `air_properties.rs:94` (the `w=1e-6` test) noting that the `1e-6` value is below the `1e-5` floor guard and is silently clamped, so the test validates the floor behavior.
3. No changes are required for correctness or safety: the formula, constants, and floor guard operate correctly and are consistent with EnergyPlus's implementation.

## References / Citations
- ASHRAE Handbook of Fundamentals, 2021, Chapter 1, Equation 28 (psychrometric specific volume)
- ASHRAE Handbook of Fundamentals, 2017, Chapter 1, Equations 11, 20 (molecular weight ratio)
- EnergyPlus `Psychrometrics.hh` `PsyRhoAirFnPbTdbW` — identical formula structure and floor guard
- OCHRE `psychrolib_jit.py` `get_moist_air_volume` / `get_moist_air_density` — alternative formula (moist-air basis)
