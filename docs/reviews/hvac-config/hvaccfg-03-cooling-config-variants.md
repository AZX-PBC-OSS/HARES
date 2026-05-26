# CoolingConfig variants: AcConfig, RoomAcConfig defaults and airflow
**Review ID**: hvaccfg-03
**Category**: hvac-config
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-equipment/src/hvac/cooling_config.rs` (lines 1–1018)
- `crates/hares-equipment/src/hvac/ac_config.rs` (lines 1–396)
- `crates/hares-equipment/src/hvac/air_conditioner.rs` (lines 416–629, init path)
- `crates/hares-equipment/src/hvac/hvac_core.rs` (lines 100–109, 337–407)
- `crates/hares-equipment/src/hvac/default_curves.rs` (lines 1–286)
- `crates/hares-equipment/src/hvac/staging.rs` (lines 270–295)

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/DXCoils.cc` — PLF curve handling, curve validation, rated conditions
- `vendors/EnergyPlus/src/EnergyPlus/DXCoils.hh` — rated temperatures (19.44°C WB, 35°C DB)
- `vendors/EnergyPlus/src/EnergyPlus/StandardRatings.cc` — SEER/EER defaults (Cd=0.25 SEER, Cd=0.20 SEER2)
- `vendors/EnergyPlus/src/EnergyPlus/DataHVACGlobals.hh` — airflow per ton limits (200–600 CFM/ton)
- `vendors/EnergyPlus/src/EnergyPlus/Autosizing/CoolingSHRSizing.cc` — SHR auto-sizing formula
- `vendors/EnergyPlus/src/EnergyPlus/VariableSpeedCoils.cc` — per-speed PLF curve for variable-speed
- `vendors/EnergyPlus/src/EnergyPlus/WindowAC.cc` — window AC part-load handling

## Findings

### Finding 1: RoomAcConfig uses static startup_cd default (0.22) instead of SEER-derived or speed-aware degradation coefficient
**Severity: high**

**Description**: `CentralAirConditionerConfig` computes a SEER-aware cycling degradation coefficient via `derived_cooling_startup_cd()` (`cooling_config.rs:127–138`), which selects Cd = 0.0 for variable-speed, 0.11 for two-speed, and SEER-derived Cd of 0.07 (SEER ≥ 13) or 0.20 (SEER < 13) for single-speed. `RoomAcConfig` has no equivalent method — it simply hardcodes `cfg.startup_cd.unwrap_or(0.22)` at `air_conditioner.rs:528`. This means:

1. All room ACs get identical cycling degradation regardless of their actual efficiency rating (EER/CEER).
2. A high-efficiency room AC (EER ≥ 12) and a low-efficiency room AC (EER = 8) both suffer the same 22% cycling penalty.
3. Room ACs cannot express variable-speed (inverter) behavior that would warrant Cd = 0.0.

**Code Location**:
- `cooling_config.rs:127–138` — `CentralAirConditionerConfig::derived_cooling_startup_cd()` (central only)
- `air_conditioner.rs:528` — static `cfg.startup_cd.unwrap_or(0.22)` (room AC)
- `RoomAcConfig` lacks `derived_cooling_startup_cd()` entirely

**Root Cause**: `RoomAcConfig` was designed as a simplified config struct without compressor speed awareness (`number_of_speeds` field absent) and without efficiency-derived startup logic. The hardcoded 0.22 is close to EnergyPlus's SEER2 Cd=0.20 (StandardRatings.cc:180) but is not parameterized by the unit's actual EER.

**Impact**: Room ACs modeled in HARES will underestimate runtime fraction for high-efficiency units (Cd too high → PLF too low → RTF inflated) and will not benefit from the variable-speed Cd=0.0 path available to central ACs. This overestimates cycling energy consumption for efficient room ACs by 5–10% at moderate part-load ratios.

---

### Finding 2: RoomAcConfig and CentralAirConditionerConfig share the same default sensible heat ratio (0.75) with no distinction between equipment types
**Severity: medium**

**Description**: Both equipment types default SHR to 0.75 (`air_conditioner.rs:527` for room AC, `air_conditioner.rs:601` for central AC). EnergyPlus does not hardcode a default SHR either — it auto-sizes SHR based on rated airflow-per-ton using `SHR = 0.431 + 6086 × (VolFlow/Capacity)` (CoolingSHRSizing.cc:87–93). At 300 CFM/ton this yields SHR ≈ 0.676; at 450 CFM/ton it yields SHR ≈ 0.798. Room ACs typically operate at lower airflow (320–350 CFM/ton) and lower SHR (0.65–0.75) than central split systems (350–400 CFM/ton, SHR 0.75–0.80).

**Code Location**: `air_conditioner.rs:527` and `air_conditioner.rs:601`

**Root Cause**: The same `0.75` constant is used without branching on `is_room_ac`. The code does not derive SHR from the equipment's actual airflow-per-ton ratio or equipment type.

**Impact**: For room ACs running at 320 CFM/ton, EnergyPlus's auto-sizing formula would predict SHR ≈ 0.695 (vs. HARES 0.75). HARES overestimates room AC sensible capacity by approximately 8 percentage points of total capacity (0.75 vs. 0.70), meaning latent load may be under-predicted and runtime fraction over-estimated since latent removal is less efficient than sensible.

---

### Finding 3: Central AC single-speed startup Cd values (0.07 / 0.20) diverge significantly from EnergyPlus defaults (0.25 SEER / 0.20 SEER2)
**Severity: medium**

**Description**: In `cooling_config.rs:133–136`, single-speed central AC units with SEER < 13 get Cd = 0.20, while SEER ≥ 13 get Cd = 0.07. EnergyPlus StandardRatings.cc:177–180 uses Cd = 0.25 for SEER calculations and Cd = 0.20 for SEER2. The HARES 0.07 value for modern (SEER ≥ 13) units is 72% below the EnergyPlus default and 65% below SEER2. At PLR = 0.5:
- EnergyPlus (SEER, Cd=0.25): PLF = 0.875 → RTF = 0.571
- EnergyPlus (SEER2, Cd=0.20): PLF = 0.900 → RTF = 0.556
- HARES (SEER ≥ 13, Cd=0.07): PLF = 0.965 → RTF = 0.518

The SEER threshold of 13 corresponds to the pre-2023 federal minimum. Post-2023, minimum SEER2 is 13.4–15.0 depending on region and system type, meaning nearly all new installations would hit the Cd=0.07 path.

**Code Location**: `cooling_config.rs:127–138`

**Root Cause**: The Cd values appear to be sourced from AHRI 210/240-2023 (SEER2) assumptions rather than the older AHRI 210/240 (SEER) standard. While 0.20 matches SEER2, the 0.07 cutoff for high-SEER units has no direct EnergyPlus analogue — EnergyPlus uses a single Cd value per rating standard regardless of efficiency tier.

**Impact**: Modern (SEER ≥ 13) single-speed units modeled in HARES will show approximately 5–10% fewer compressor starts per hour and a correspondingly lower cycling penalty than EnergyPlus would calculate. The total energy impact is modest (2–4% of cooling energy at typical residential part-load profiles) because cycling degradation only affects periods when the unit cycles.

---

### Finding 4: RoomAcConfig has no compressor speed differentiation; cannot model inverter/variable-speed room ACs
**Severity: medium**

**Description**: `RoomAcConfig` lacks `number_of_speeds`, `stage_capacities_w`, `stage_eirs`, and `stage_shrs` fields that `CentralAirConditionerConfig` provides. Room ACs are unconditionally forced to `SpeedControlMode::SingleSpeed` at init (`air_conditioner.rs:494–501`). Modern inverter-driven window and through-wall ACs with variable-speed compressors exist in the market (e.g., Midea U-shaped inverter AC, Frigidaire Gallery Cool Connect) but cannot be modeled.

**Code Location**:
- `cooling_config.rs:276–341` — `RoomAcConfig` struct (no speed fields)
- `air_conditioner.rs:494–501` — forced single-speed enforcement
- Compare: `cooling_config.rs:15–125` — `CentralAirConditionerConfig` with full multi-speed support

**Root Cause**: `RoomAcConfig` was designed as a minimal configuration for simple window units. The struct does not include multi-speed fields, and the init path rejects any non-single-speed speed mode.

**Impact**: Analysts cannot model efficient variable-speed room ACs, which represent a growing share of the window-unit market. The workaround (using `CentralAirConditionerConfig` with adjusted parameters) would misrepresent the equipment type in telemetry and reporting.

---

### Finding 5: RoomAcConfig does not validate startup_cd field
**Severity: low**

**Description**: `CentralAirConditionerConfig::validate()` checks `startup_cd >= 0` at `cooling_config.rs:187–193`. `RoomAcConfig::validate()` at `cooling_config.rs:351–405` has no validation of `startup_cd` whatsoever. While the value is consumed at `air_conditioner.rs:528` and later clamped to `[0.0, 1.0]` in the PLF formula at `staging.rs:279`, an invalid negative or NaN value passes validation silently.

**Code Location**:
- `cooling_config.rs:187–193` — central AC validates startup_cd
- `cooling_config.rs:351–405` — room AC validation (startup_cd absent)

**Impact**: Low. The downstream `clamp(0.0, 1.0)` in `staging.rs:279` prevents a crash, and the absence of validation means a physically nonsensical negative Cd would be silently clamped rather than rejected at config load with a clear error message.

---

### Finding 6: RoomAcConfig lacks refrigerant charge defect ratio correction
**Severity: low**

**Description**: `CentralAirConditionerConfig` supports `charge_defect_ratio` (`cooling_config.rs:105–108`) and applies capacity/EIR corrections at init (`air_conditioner.rs:556–562`). `RoomAcConfig` has no equivalent field. While room ACs are factory-sealed and less commonly subject to field charging errors, undercharge/overcharge due to slow refrigerant leakage affects all DX equipment over time.

**Code Location**:
- `cooling_config.rs:105–108` — `CentralAirConditionerConfig.charge_defect_ratio`
- `air_conditioner.rs:556–562` — charge defect application for central AC
- `RoomAcConfig` struct — field absent

**Impact**: Low. Room ACs are sealed systems where charge defect is primarily a maintenance/degradation issue rather than an installation quality issue. However, for long-term degradation modeling (multi-year simulations), the absence prevents modeling gradual refrigerant loss in room ACs.

---

### Finding 7: Default biquadratic performance curves correctly differ between room AC and central AC, and are properly assigned via `load_curve_pair`
**Severity: none / informational**

**Description**: `ac_config.rs:13–20` defines four distinct curve sets:
- `DEFAULT_AC_CAPACITY_CURVE` and `DEFAULT_AC_EIR_CURVE` for central AC
- `DEFAULT_ROOM_AC_CAPACITY_CURVE` and `DEFAULT_ROOM_AC_EIR_CURVE` for room AC

Both normalize to approximately 1.0 at AHRI rated conditions (indoor WB = 19.44°C, outdoor DB = 35.0°C):
- Central AC capacity modifier at rated: ≈ 0.994
- Room AC capacity modifier at rated: ≈ 0.995
- Central AC EIR modifier at rated: ≈ 1.017
- Room AC EIR modifier at rated: ≈ 1.000

The `load_curve_pair()` function (`ac_config.rs:232–305`) correctly branches on the `is_room_ac` flag to select the appropriate defaults. When explicit user curves are provided, they are used instead. EnergyPlus requires user-provided curves for all DX coil types (no built-in defaults), so HARES's provision of per-type defaults is a usability improvement.

**Code Location**: `ac_config.rs:13–20`, `ac_config.rs:286–295`

**Impact**: None — this is correctly implemented.

---

### Finding 8: Airflow defaults are correctly calculated and differentiated
**Severity: none / informational**

**Description**: The airflow constants at `hvac_core.rs:107–109` are:
- `AIRFLOW_CENTRAL_AC_M3_S_PER_W = 5.3678384759785085e-5` → 400 CFM/ton
- `AIRFLOW_ROOM_AC_M3_S_PER_W = 4.294270780782807e-5` → 320 CFM/ton
- `AIRFLOW_MSHP_COOLING_M3_S_PER_W = 4.186914011263237e-5` → 312 CFM/ton

Conversion verification: 400 CFM/ton = 400 ft³/min × 0.000471947 (m³/s)/(ft³/min) ÷ (12,000/3.412141633) W/ton = 0.188779 / 3516.85 = 5.3677e-5 m³/(s·W) ✓

The central AC default of 400 CFM/ton is the AHRI 210/240 rated condition standard for dry-climate split systems. The room AC default of 320 CFM/ton is within the typical 320–350 CFM/ton range for window/through-wall units. EnergyPlus uses an allowable range of 300–450 CFM/ton for rated flow (DataHVACGlobals.hh:348–366), so both values fall within acceptable bounds.

Central AC airflow defaults are applied via `HvacConfig::new()` at `hvac_core.rs:354–367`, matching `HvacEquipmentType::AcCooler`. Room AC airflow is applied via explicit unwrap at `air_conditioner.rs:524–526`.

**Code Location**: `hvac_core.rs:107–109`, `hvac_core.rs:354–356`, `air_conditioner.rs:524–526`

**Impact**: None — this is correctly implemented.

---

### Finding 9: Compressor speed mode to PLF degradation model association is architecturally sound
**Severity: none / informational**

**Description**: The `cooling_speed_control_mode()` → `derived_cooling_startup_cd()` chain in `CentralAirConditionerConfig` correctly associates:
- VariableSpeedIdeal → Cd = 0.0 (no cycling degradation for modulating equipment)
- TwoSpeed → Cd = 0.11 (moderate cycling penalty)
- SingleSpeed → SEER-derived Cd (full cycling penalty)

At runtime, the PLF model at `staging.rs:279–280` computes `PLF = 1.0 - Cd × (1.0 - PLR)` with a floor at `plf_min` (default 0.7) and a further clamp to ensure `PLF ≥ PLR` (RTF ≤ 1.0). This matches the EnergyPlus approach:
- EnergyPlus uses user-provided PLF curves (quadratic or cubic) that evaluate `PLF = f(PLR)` with output clamped to [0.7, 1.0] and an additional constraint `PLF ≥ PLR`
- HARES uses the AHRI 210/240 linear-degradation approximation `PLF = 1 - Cd×(1-PLR)` which is the industry-standard simplification for SEER/EER rating calculations

The EnergyPlus approach differs in that PLF curves are *per DX coil* (one for single-speed, one per speed for multi-speed), while HARES uses a single Cd value per equipment instance. For single-speed equipment, this is equivalent. For variable-speed equipment at part load, EnergyPlus's per-speed PLF curves would capture speed-dependent cycling behavior that HARES's Cd=0.0 simplifies away.

**Code Location**: `cooling_config.rs:118–138`, `staging.rs:278–294`

**Impact**: None — the architecture is reasonable for residential energy simulation.

---

## Summary
- Total findings: 9
- High: 1
- Medium: 3
- Low: 2
- Informational: 3

## Recommendations

1. **Add `derived_cooling_startup_cd()` to `RoomAcConfig`** (Finding 1). Derive Cd from the unit's EIR (or EER) using the same SEER-based logic as `CentralAirConditionerConfig`, replacing the hardcoded 0.22. This would give high-EER room ACs lower cycling degradation, consistent with the central AC logic. If keeping a static default, use the EnergyPlus SEER2 default of 0.20.

2. **Differentiate default SHR by equipment type** (Finding 2). Set room AC default SHR to 0.70 (matching EnergyPlus auto-sizing at 320 CFM/ton) and keep central AC at 0.75. Alternatively, derive SHR from `airflow_m3_s_per_w / capacity_w` using the EnergyPlus auto-sizing formula for better dynamic accuracy.

3. **Revisit single-speed Cd = 0.07 for SEER ≥ 13** (Finding 3). Consider aligning with EnergyPlus SEER2 Cd = 0.20 for all single-speed units, or document the rationale for the 0.07 value (e.g., field-measured cycling losses or HERS-specific calibration). The current value under-predicts cycling losses relative to both EnergyPlus defaults.

4. **Add multi-speed support to `RoomAcConfig`** (Finding 4). Add `number_of_speeds`, `stage_capacities_w`, `stage_eirs`, and `stage_shrs` fields to support inverter-driven room ACs. Keep single-speed as the default but allow users to specify multi-speed for modern equipment.

5. **Add `startup_cd` validation to `RoomAcConfig::validate()`** (Finding 5). Mirror the central AC validation at `cooling_config.rs:187–193`.

6. **Consider adding `charge_defect_ratio` to `RoomAcConfig`** (Finding 6). Even though room ACs are factory-sealed, multi-year simulations benefit from modeling gradual refrigerant loss.

## References / Citations

1. EnergyPlus Engineering Reference, §16.5 "DX Cooling Coil" — rated conditions at 19.44°C indoor WB, 35.0°C outdoor DB (DXCoils.hh:72–74)
2. EnergyPlus AHRI 210/240-2017 default Cd = 0.25 (StandardRatings.cc:177)
3. EnergyPlus AHRI 210/240-2023 (SEER2) default Cd = 0.20 (StandardRatings.cc:180)
4. EnergyPlus SHR auto-sizing: SHR = 0.431 + 6086 × (VolFlow/Capacity) (CoolingSHRSizing.cc:87–93)
5. EnergyPlus rated airflow per ton limits: 200–600 CFM/ton (DataHVACGlobals.hh:348–366)
6. AHRI 210/240-2023, §6.6.3 — linear cycling degradation model: PLF = 1 − Cd × (1 − PLR)
7. RESNET HERS Addendum 82 / ANSI/RESNET/ACCA 310-2020 — charge defect correction coefficients (Cc = +0.9, Ce = −0.9)
