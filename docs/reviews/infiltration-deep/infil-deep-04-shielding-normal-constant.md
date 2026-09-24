# SHIELDING_NORMAL=0.5/3.0 constant and attic/garage ELA coefficient formulas
**Review ID**: infil-deep-04
**Category**: infiltration-deep
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/infiltration.rs` (lines 1–1728)

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/ZoneEquipmentManager.cc:6771–6792` — Sherman-Grimsrud and AIM-2 runtime formulas
- `vendors/EnergyPlus/src/EnergyPlus/HeatBalanceAirManager.cc:925–1088` — ZoneInfiltration:EffectiveLeakageArea and ZoneInfiltration:FlowCoefficient input processing
- `vendors/EnergyPlus/src/EnergyPlus/DataHeatBalance.hh:158–164, 1095–1139` — InfiltrationModelType enum, InfiltrationData struct
- `vendors/EnergyPlus/doc/input-output-reference/src/overview/group-airflow.tex:190–395` — Shelter class tables, Sherman-Grimsrud and AIM-2 documentation
- `crates/hares-core/src/dwelling/solver_builder.rs:1005–1040, 1260–1359` — Per-zone infiltration method dispatch

## Findings

### Finding 1: SHIELDING_NORMAL = 0.5/3.0 is correct but uses poor notation [Severity: low]
**Description**: The constant `SHIELDING_NORMAL = 0.5 / 3.0` (line 674) evaluates to `1/6 ≈ 0.1667`, which is the Walker & Wilson (1998) Table 3 local shielding parameter C' for "normal (typical suburban)" conditions. The numeric value is correct. However, the expression `0.5 / 3.0` is confusing — it reads as "0.5 divided by 3.0" rather than the standard mathematical representation "1/6."

**Code Location**: `crates/hares-physics/src/infiltration.rs:674`

**Root Cause**: The value 0.5 is `ShieldingClass::Normal.raw()` (line 415), and dividing by 3.0 converts from the AIM-2 "ShelterFactor" scale (used in EnergyPlus `ZoneInfiltration:FlowCoefficient`) to the Walker-Wilson local shielding parameter C' scale (used in the Sherman-Grimsrud ELA model). The relationship `C' = s/3` is exact for all three shielding classes:

| Class | ShieldingClass.raw() (s) | C' = s/3 | Walker & Wilson Table 3 |
|-------|--------------------------|----------|--------------------------|
| Exposed | 0.9 | 0.30 | 0.30 |
| Normal | 0.5 | 0.1667 ≈ 1/6 | ≈0.167 (1/6) |
| Well-shielded | 0.3 | 0.10 | 0.10 |

The derivation s/3 is correct, but neither the constant expression (`0.5 / 3.0`) nor the doc comment (line 673: "s_g = 0.5/3") explains this conversion. Readers unfamiliar with the relationship between the two shielding scales may misunderstand the value's origin.

**Impact**: Low — the computed value is correct. The risk is maintainability: a future developer might see `0.5 / 3.0` and wonder whether the 0.5 is a typo or whether the 3.0 should be changed.

### Finding 2: AIM-2 ShieldingClass and ELA SHIELDING_NORMAL live in separate, undocumented scales [Severity: medium]
**Description**: The codebase maintains two parallel shielding scales without documenting their relationship:

1. **AIM-2 shelter-factor scale** (`ShieldingClass::raw()` at lines 413–419): values 0.3 (WellShielded), 0.5 (Normal), 0.9 (Exposed). Used in `aim2_coefficients_from_ach50()` line 564 as `params.shielding.raw()`.
2. **ELA local-shielding scale** (doc comments at lines 600–603): values 0.10, 1/6≈0.167, 0.30. Used in `calculate_ela_coefficients()` line 659 as the `shielding` parameter within `f_w = shielding × (1−R)^{1/3} × f_t`.

These correspond to two different physical parameters from Walker & Wilson (1998):
- The AIM-2 scale (call it `s_w`) is the dimensionless "shelter factor" used directly in the AIM-2 wind term: `Q_wind ∝ (s_w × v)^{2n}`.
- The ELA scale (call it `C'`) is the "local shielding parameter" from Walker & Wilson Table 3, representing the ratio of local wind speed at the building to wind speed at a reference open site.

EnergyPlus documentation confirms this distinction. The `ZoneInfiltration:FlowCoefficient` (AIM-2) object uses `ShelterFactor` with values 0.30–1.00 (lines 346–362 of group-airflow.tex), while the `ZoneInfiltration:EffectiveLeakageArea` (Sherman-Grimsrud) object uses pre-tabulated wind coefficients with shelter classes 1–5 (lines 214–231 of group-airflow.tex).

**Code Location**: `crates/hares-physics/src/infiltration.rs:413–419`, `line 564`, `line 659`, `line 674`

**Root Cause**: The code correctly implements the two different shielding parameters but never states the conversion factor `C' = s_w / 3`. A reader encountering `SHIELDING_NORMAL` for the first time has no way to understand why the normal value for the ELA model (≈0.167) differs from the normal value for the AIM-2 model (0.5).

**Impact**: Medium — risk of misconfiguration or incorrect patch if a developer applies the wrong shielding scale to the wrong model. For example, passing `0.5` as the `shielding` argument to `calculate_ela_coefficients()` (treating it as if it were the AIM-2 scale) would overstate the wind coefficient by a factor of 9 (since 0.5²/0.167² = 9).

### Finding 3: Attic and garage ELA coefficients hardcode SHIELDING_NORMAL and Suburban terrain [Severity: medium]
**Description**: Both `attic_ela_coefficients()` (lines 680–688) and `garage_ela_coefficients()` (lines 693–701) hardcode `SHIELDING_NORMAL` (~0.167) and `TerrainClass::Suburban`. These convenience functions ignore the building's actual shielding class and terrain, which are independently configurable for the conditioned zone through `Aim2Params.shielding` and `Aim2Params.terrain`.

In the solver builder, the conditioned zone can receive shielding-dependent coefficients via `aim2_coefficients_from_ach50()` or custom ELA coefficients from the thermal config. However, the attic and garage zones ALWAYS receive coefficients computed with `SHIELDING_NORMAL` and `TerrainClass::Suburban`, regardless of:
- Whether the building is in an exposed rural site or a densely-shielded urban setting
- Whether the conditioned zone uses AIM-2 or ELA-based infiltration with different shielding

**Code Location**: `crates/hares-physics/src/infiltration.rs:680–688`, `lines 693–701`; called from `crates/hares-core/src/dwelling/solver_builder.rs:1285`, `line 1305`, `line 1353`

**Root Cause**: The attic and garage convenience functions were written with fixed defaults for simplicity. There is no mechanism to propagate the building-level shielding and terrain configuration from the solver builder into these functions.

**Impact**: Medium. In an exposed rural house, the attic wind coefficients would be correct for a suburban-normal site, understating wind-driven attic infiltration by roughly:
- Wind coefficient ratio: `C'_exposed/C'_normal = 0.30/0.167 = 1.8` (shielding factor alone)
- Combined with terrain (rural vs suburban): `0.30/0.167 × (f_t_rural/f_t_suburban)` at a given height
This means the model will systematically mischaracterize attic and garage infiltration for non-suburban-normal configurations. The magnitude depends on wind dominance and building height.

### Finding 4: Attic and garage infiltration treated as independent exterior zones with no interzonal leakage coupling [Severity: medium]
**Description**: The solver builder creates separate `InfiltrationMethod` entries for each zone type (conditioned, attic, garage — lines 1008–1036 of solver_builder.rs). Each zone's infiltration is computed independently using `ela_infiltration()` (or `ashrae_wind_stack()`). There is no mechanism for inter-zonal air leakage:

- **Attic-to-conditioned ceiling leakage**: No flow path exists from the conditioned zone through the ceiling to the attic (or vice versa).
- **Garage-to-conditioned wall/door leakage**: No flow path exists between the garage and conditioned space.
- **Foundation-to-conditioned floor leakage**: No explicit coupling between foundation air and conditioned zone infiltration.

EnergyPlus handles interzonal flows through the `AirflowNetwork:MultiZone:Surface:EffectiveLeakageArea` components (AirflowNetwork/Elements.cpp lines 2000–2109). The EnergyPlus AirflowNetwork allows a dwelling model to specify per-surface ELAs between zones, enabling coupled multi-zone airflow balances. HARES does not implement this multi-zone airflow network.

**Code Location**: `crates/hares-core/src/dwelling/solver_builder.rs:1005–1036`, `crates/hares-physics/src/infiltration.rs:165–175`

**Root Cause**: The HARES thermal model treats zones as thermally coupled via boundary conduction but hydraulically independent for infiltration. This is an architectural simplification.

**Impact**: Medium for energy predictions. The model cannot capture:
1. Stack-driven air exchange between attic and conditioned space (significant in cold climates where warm indoor air leaks into a cold attic, driving moisture and heat loss)
2. Garage-to-house air exchange through the shared wall (important when garage equipment or vehicles emit pollutants)
3. The mutual buffering effect: a leaky attic can reduce the wind-driven infiltration of the conditioned zone because the attic acts as a pressure buffer

For purely thermal energy predictions in typical climates, this simplification may be acceptable since heat exchange through conduction dominates. However, for detailed infiltration analysis or IAQ modeling, the decoupling introduces error.

### Finding 5: Hor_lk_frac values match Walker & Wilson (1998) Table 2 [Severity: low — positive finding]

**Description**: The horizontal leakage fractions used for each zone type match the reference:

| Zone | hor_lk_frac | Walker & Wilson Table 2 | Code reference |
|------|-------------|--------------------------|----------------|
| Conditioned | 0.0 | Vertical-dominated leakage | Line 462 (solver_builder) |
| Garage | 0.4 | Mixed leakage | Line 695 |
| Attic | 0.75 | Ceiling-dominated leakage | Line 682 |

The stack shape factor `f_s` increases with `hor_lk_frac` (more horizontal leakage → stronger stack effect), and the wind shape factor `f_w` decreases with `hor_lk_frac` (horizontal surfaces are more sheltered from wind). The tests at lines 1125–1165 confirm this monotonicity. These are correct implementations of Walker & Wilson (1998) Eqs. 12–13.

**Impact**: Low positive — confirms reference fidelity.

## Summary
- Total findings: 5
- Critical: 0
- High: 0
- Medium: 3 (Findings 2, 3, 4)
- Low: 2 (Findings 1, 5)

## Recommendations

1. **Clarify SHIELDING_NORMAL derivation (Finding 1)**: Replace the constant definition with `1.0 / 6.0` and add a doc comment explaining the conversion from the AIM-2 shelter factor scale: `C' = ShieldingClass::Normal.raw() / 3.0 = 0.5 / 3.0 = 1/6`.

2. **Document the two shielding scales (Finding 2)**: Add an explicit comment block near `ShieldingClass` and `SHIELDING_NORMAL` explaining the relationship between the AIM-2 ShelterFactor scale (used in `aim2_coefficients_from_ach50`) and the Walker-Wilson local shielding parameter C' (used in `calculate_ela_coefficients`), including the conversion factor C' = s_w / 3 derived from Walker & Wilson (1998) Table 3.

3. **Parameterize attic/garage ELA functions with shielding and terrain (Finding 3)**: Consider extending `attic_ela_coefficients` and `garage_ela_coefficients` to accept `ShieldingClass` and `TerrainClass` parameters, or propagate the building-level shielding class from the solver builder. At minimum, document the hardcoded assumption explicitly in the function doc comments.

4. **Assess interzonal leakage coupling priority (Finding 4)**: For energy-only simulations, the decoupled approach may be sufficient. If the model will be used for detailed infiltration, IAQ, or moisture analysis, plan for interzonal airflow paths in a future iteration. The EnergyPlus AirflowNetwork provides a reference architecture.

## References / Citations
- Walker, I.S. & Wilson, D.J. (1998). "Field Validation of Algebraic Equations for Stack and Wind Driven Air Infiltration Calculations." *HVAC&R Research*, 4(2), 119–139. Tables 2–3; Equations 9–25.
- ASHRAE Handbook of Fundamentals (2021), Chapter 16: Ventilation and Infiltration.
- EnergyPlus Engineering Reference, §15.4: AIM-2 Enhanced Infiltration Model.
- EnergyPlus Input-Output Reference: `ZoneInfiltration:EffectiveLeakageArea` (Sherman-Grimsrud), `ZoneInfiltration:FlowCoefficient` (AIM-2).
- OCHRE `envelope.py:488–633` (`calculate_ashrae_infiltration_params`) — AIM-2 coefficient pipeline.
- ResStock `airflow.get_aim2_shelter_coefficient` — shelter class mapping.
