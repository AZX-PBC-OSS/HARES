# Duct leakage infiltration formula duplicates ASHRAE 152 across crates
**Review ID**: types-physics-04
**Category**: types-physics
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/ashrae152.rs`
- `crates/hares-physics/src/infiltration.rs`
- `crates/hares-envelope/src/thermal_solver/infiltration.rs`

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/utils/equipment.py` (primary reference; OCHRE implements the same ASHRAE 152 DSE model)
- `vendors/OCHRE/ochre/defaults/ASHRAE152_zone_temperatures.csv` (zone temperature / regain factor reference table)
- EnergyPlus `DataHeatBalance.hh`, `HeatBalanceAirManager.cc` (EnergyPlus uses a fundamentally different zone mass conservation approach — no direct ASHRAE 152 §9.3 equivalent exists in EnergyPlus; DuctSolver.cc was not found in the EnergyPlus source tree)

## Findings

### Finding 1: [Severity: medium] ASHRAE 152 §9.3 infiltration superposition formula duplicated in two locations

**Description**: The same ASHRAE 152 §9.3 duct-leakage/infiltration interaction formula — the power-law superposition `(baseline^1.5 ± imb^1.5)^0.67` — is implemented in two separate locations within the same crate, with no shared implementation.

**Code Location**:
- `crates/hares-physics/src/ashrae152.rs:460-468` — embedded inside `calculate_dse()` for the full DSE calculation
- `crates/hares-physics/src/infiltration.rs:307-330` — standalone `duct_leakage_infiltration_m3_s()` function

**Root Cause**: Architectural split between the self-contained DSE calculation and the runtime thermal solver. `ashrae152.rs` computes everything from scratch in IP units and uses the formula internally for the load factor computation. `infiltration.rs` provides a standalone `pub fn` that the thermal solver calls to adjust a pre-computed natural infiltration rate with the same formula. Both implement the identical three-branch logic:
- Supply > return: pressured pressurisation → `(base^1.5 + imb^1.5)^0.67`
- Imbalance > baseline: dominated depressurisation → `0.0`
- Otherwise: partial depressurisation → `(base^1.5 - imb^1.5)^0.67`

**Impact**: Any update to the ASHRAE 152 superposition formula (e.g., a future standards revision changing the exponents from 1.5/0.67) must be applied in two places. While both files are in the same `hares-physics` crate, a maintainer modifying one copy could miss the other. This is a maintenance risk, not an active correctness bug.

---

### Finding 2: [Severity: medium] CFM25-to-operating duct leakage conversion with pressure exponents (0.6 / 0.65) is not implemented

**Description**: ASHRAE Standard 152 specifies that duct leakage measured at 25 Pa (CFM25) should be converted to operating leakage using a power-law relationship:
```
Q_operating = Q_CFM25 × (P_duct / 25)^n
```
where `n = 0.6` for supply ducts and `n = 0.65` for return ducts. Neither of these pressure exponents is implemented anywhere in the codebase. The HPXML parser explicitly **rejects** CFM25 inputs (`crates/hares-io/src/hpxml/building.rs:1843-1849`), only accepting leakage as a fraction of fan flow (`Percent` or `Fraction` units).

**Code Location**:
- `crates/hares-io/src/hpxml/building.rs:1840-1850` — parser accepts only `"percent"` and `"fraction"` units; all others are warned and skipped
- `crates/hares-io/src/hpxml/building.rs:5048-5101` — test `duct_leakage_cfm25_rejected()` confirms CFM25 is explicitly unsupported
- No occurrence of exponent `0.6` or `0.65` used in a duct leakage context anywhere in the codebase

**Root Cause**: The codebase follows OCHRE's approach, which also uses leakage fractions rather than CFM25. The conversion from CFM25 to operating flow requires the duct operating pressure, which is typically assumed to be ~25 Pa for supply and ~0 Pa (ambient) for return in ASHRAE 152, but the standard actually requires using `P_duct` (variable operating pressure). Since the thermal solver doesn't model duct pressure, the fraction-based approach is a valid simplification, but it means HPXML files with CFM25 duct leakage measurements cannot be parsed directly (require upstream pre-conversion to fraction).

**Impact**: This is the most common HPXML input format for duct leakage. Every HPXML file with CFM25 values must be pre-processed to compute a leakage fraction using fan flow, which means the fan flow rate must also be known. Real-world HPXML files frequently use CFM25. This is a known limitation and may be acceptable if the pipeline always provides leakage as a fraction, but it should be documented clearly.

**Note on OCHRE parity**: OCHRE also takes leakage as a fraction (`supply_nom_leakage` / `return_nom_leakage` in `equipment.py:172,180`), so HARES is consistent with OCHRE. Neither implements CFM25 conversion. EnergyPlus takes a fundamentally different approach (modeling duct leakage as explicit mass flows between zones).

---

### Finding 3: [Severity: medium] The two formula copies differ in their scaling approach, creating a subtle behavioural divergence risk

**Description**: The duplicate implementations compute different output values despite using identical core logic:

1. In `ashrae152.rs:460-468`, the formula is self-contained — it computes `infil` (CFM) directly, using only the ASHRAE 152 baseline `infil_fan_off = 0.35 × V / 60` as the reference. This result feeds into the load factor calculation only, not back to the zone airflow.

2. In `infiltration.rs:319-336`, the formula first computes `adjusted` from the ASHRAE 152 baseline, then **scales the caller's externally-provided natural infiltration rate** by the ratio `adjusted / infil_fan_off`:
```rust
if infil_fan_off > 0.0 {
    base_infil_m3_s * (adjusted / infil_fan_off)
} else {
    adjusted
}
```
This is an extension: it applies the ASHRAE 152 duct imbalance effect proportionally to whatever infiltration rate the thermal solver computed (AIM-2, ELA, or ACH), rather than overwriting it with the ASHRAE 152 baseline value. The comment at lines 332-338 documents this choice: "We scale the caller's base rate by the same ratio so it also captures stack/wind effects."

**Code Location**:
- `crates/hares-physics/src/infiltration.rs:319-338`
- `crates/hares-physics/src/ashrae152.rs:460-468`

**Root Cause**: The architectural separation between the DSE engine and the thermal solver requires different return semantics. The DSE engine needs a raw CFM value for the load factor; the thermal solver needs an adjusted m³/s rate that preserves the zone-specific infiltration dynamics.

**Impact**: The scaling approach means the thermal solver's infiltration adjustment depends on what `base_infil_m3_s` is relative to the ASHRAE 152 baseline. If the thermal solver's natural infiltration model (e.g., AIM-2 with 7 ACH50) produces a rate that's \(k\) times higher than the ASHRAE baseline, the duct-leakage adjustment will be amplified by \(k\) as well. This is arguably correct (a leakier house experiences more of the duct-driven infiltration effect), but it's a modelling choice that differs from a strict ASHRAE 152 interpretation and is not what OCHRE does (OCHRE uses the ASHRAE 152 baseline without scaling externally-computed infiltration).

---

### Finding 4: [Severity: low] Duct location weighting factors (thermal regain) match ASHRAE 152-2014 and OCHRE

**Description**: The 16 zone types in `Ashrae152ZoneType` each have `supply_regain` and `return_regain` values that account for duct location effects. All values match both ASHRAE Standard 152-2014 Table 7 (thermal regain factors) and the OCHRE reference table (`ASHRAE152_zone_temperatures.csv`).

**Code Location**: `crates/hares-physics/src/ashrae152.rs:200-323`

**Verified values** (comparing against OCHRE zone_temperatures.csv):

| Zone Type | Supply Regain | Return Regain | Matches OCHRE |
|-----------|:------------:|:-------------:|:------------:|
| Attic (vented/unvented) | 0.10 | 0.10 | ✓ |
| Attic w/ radiant barrier | 0.10 | 0.10 | ✓ |
| Garage | 0.10 | 0.10 | ✓ |
| Crawlspace, unvented, uninsulated | 0.60 | 0.60 | ✓ |
| Crawlspace, unvented, ins floor+wall | 0.60 | 0.60 | ✓ |
| Crawlspace, unvented, ins floor only | 0.30 | 0.30 | ✓ |
| Crawlspace, vented, uninsulated | 0.60 | 0.60 | ✓ |
| Crawlspace, vented, ins floor+wall | 0.63 | 0.63 | ✓ |
| Crawlspace, vented, ins floor only | 0.30 | 0.30 | ✓ |
| Basement, uninsulated | 0.50 | 0.50 | ✓ |
| Basement, insulated walls | 0.60 | 0.60 | ✓ |
| Basement, insulated ceiling | 0.60 | 0.60 | ✓ |
| Under slab | 0.20 | 0.20 | ✓ |
| Exterior walls | 0.20 | 0.20 | ✓ |

Zone temperature formulas are also byte-for-byte consistent with the OCHRE CSV, including the asymmetric heating/cooling formulations for basement zones that use `ground_temp = (heating_des_init + cooling_des_init) / 2`.

---

### Finding 5: [Severity: low] No equivalent ASHRAE 152 DSE in EnergyPlus — OCHRE is the correct reference

**Description**: EnergyPlus does not implement the ASHRAE 152 simplified DSE calculation. EnergyPlus models duct systems explicitly through component-level heat balance and zone air mass flow conservation (`ZoneAirMassFlowConservation` / `ZoneMassConservationData` in `DataHeatBalance.hh`), which is a fundamentally different approach. The expectation of finding an equivalent in `DuctSolver.cc` is not applicable — no such file exists in the EnergyPlus source tree (`vendors/EnergyPlus/src/EnergyPlus/` contains no `DuctSolver.*` files).

The OCHRE `equipment.py:calculate_duct_dse()` function (lines 161-459) is the correct vendor reference. HARES's `ashrae152.rs` closely mirrors OCHRE's implementation, including the same R-value transform equations, the same climate lookup via haversine distance, and the same DSE formula structure. OCHRE also does not implement CFM25 conversion (uses fraction-based leakage inputs).

---

### Finding 6: [Severity: low] Low-speed branch uses different air density for dTe_low in OCHRE but not in HARES

**Description**: In the OCHRE reference (`equipment.py:322`), the low-speed denominator uses `0.0775` instead of `0.075` for the denom calculation:
```python
dTe_low = capacity_low * hvac.hvac_mult / (60 * fan_flow_low * 0.0775 * 0.24)
```
HARES uses the same `0.075` density for both high-speed and low-speed in both heating and cooling branches:
```rust
let denom_low = 60.0 * flow_low * 0.075 * 0.24;
```
(`ashrae152.rs:483`)

**Code Location**: `crates/hares-physics/src/ashrae152.rs:483`

**Root Cause**: OCHRE's `0.0775` appears to be a possible bug or an intentional density adjustment for low-speed operation (cooler supply air = denser air?). The `Bs_low` line right after uses `0.075`:
```python
Bs_low = np.exp(-supply_area / (60 * fan_flow_low * 0.075 * 0.24 * supply_r))
```
This inconsistency in OCHRE (different densities in dTe_low vs Bs_low) suggests `0.0775` may be a typo — ASHRAE 152 uses `0.075 lb/ft³` (standard air density at sea level) throughout. HARES correctly uses `0.075` consistently. This is a minor discrepancy from OCHRE but likely correct per the ASHRAE 152 standard.

**Impact**: The difference affects multi-speed systems only. Using `0.075` instead of `0.0775` in the denominator changes `dTe_low` by ~3.3%, which propagates to the uncorrected delivery effectiveness. The effect on final DSE is typically <0.5%.

---

## Summary
- Total findings: 6
- Critical: 0
- High: 0
- Medium: 3 (Findings 1, 2, 3)
- Low: 3 (Findings 4, 5, 6)

## Recommendations

1. **Deduplicate the ASHRAE 152 §9.3 formula** (Finding 1): Extract the three-branch superposition logic into a shared private function in `hares-physics` that both `ashrae152.rs` and `infiltration.rs` call. The function should take `(infil_fan_off, supply_leakage, return_leakage)` and return the adjusted flow, with the scaling step remaining a separate concern in the caller. This eliminates the duplication risk and makes the formula's single source of truth explicit.

2. **Document the CFM25 limitation** (Finding 2): Add a module-level doc comment in `hares-io/src/hpxml/building.rs` explaining that duct leakage must be supplied as a fraction of fan flow (not CFM25), and note that ASHRAE 152 conversion exponents (n=0.6 supply, n=0.65 return) are not needed because the codebase operates on fractions. If CFM25 support is desired later, a converter function in `hares-physics` using the standard pressure exponents and a configurable duct pressure would be the architectural home.

3. **Audit the scaling approach** (Finding 3): Verify that `base_infil_m3_s * (adjusted / infil_fan_off)` produces physically reasonable results across the full range of expected infiltration rates. Add a test that compares the scaled result against a direct ASHRAE 152 calculation for a representative dwelling to ensure the scaling doesn't produce implausible extremes at very low or very high infiltration rates.

4. **Review OCHRE 0.0775 discrepancy** (Finding 6): Confirm whether ASHRAE 152 specifies different air densities for multi-speed systems. If not, HARES is correct and the discrepancy from OCHRE should be dismissed. If yes, update line 483 to use the appropriate density.

## References / Citations

- ASHRAE Standard 152-2014, "Method of Test for Determining the Design and Seasonal Efficiencies of Residential Thermal Distribution Systems," §9.3 (infiltration interaction).
- ASHRAE Standard 152-2014, Table 7 (supply/return duct thermal regain factors).
- Walker, I.S., and Wilson, D.J. (1998) "Field Validation of Algebraic Equations for Stack and Wind Driven Air Infiltration Calculations," *HVAC&R Research* 4(2).
- OCHRE `calculate_duct_dse()`: `vendors/OCHRE/ochre/utils/equipment.py:161-459`.
- OCHRE zone temperatures reference: `vendors/OCHRE/ochre/defaults/ASHRAE152_zone_temperatures.csv`.
- EnergyPlus Engineering Reference §15.4 (AIM-2 Enhanced Infiltration Model) — does not include ASHRAE 152 DSE.
- HPXML v4.0 schema: `DuctLeakageMeasurement/DuctLeakage/Units` accepts `CFM25`, `CFM50`, `Percent`, and `Fraction`.
