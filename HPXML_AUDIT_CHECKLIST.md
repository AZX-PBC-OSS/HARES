# HPXML Parsing Audit: Verification Checklist

Use this checklist to track remediation of audit findings.

---

## CRITICAL Issues (Must Fix)

### Issue #1: HVAC Number of Speeds Not Determined
- [ ] Add CompressorType → [1,2,4] speed count mapping
- [ ] Implement SEER/HSPF range-based fallback logic
  - [ ] SEER ≤ 15 → 1 speed
  - [ ] 15 < SEER ≤ 21 → 2 speeds
  - [ ] SEER > 21 → 4 speeds
- [ ] Add mini-split (MSHP) special case → 4 speeds
- [ ] Emit `number_of_speeds` integer parameter
- [ ] Verify against OCHRE on test case (BESTEST-600 with ASHP)
- [ ] Test file: `crates/hares-io/src/hpxml/equipment.rs:166-271`

**Acceptance:** EquipmentSpec contains `"number_of_speeds": 1|2|4` for all HVAC

---

### Issue #2: Startup Capacity Degradation (C_D) Not Calculated
- [ ] Port `calc_c_d()` function from OCHRE `utils/equipment.py`
  - [ ] Takes: is_heater, equipment_name, cop, number_of_speeds
  - [ ] Returns: 0.7-0.9 degradation factor
- [ ] Call during heat pump and AC resolution
- [ ] Emit `startup_capacity_degradation` parameter (0.7-0.9 range)
- [ ] Only for AC and heat pumps (not furnaces, boilers)
- [ ] Test file: `crates/hares-io/src/hpxml/equipment.rs:166-271` (heat pumps), lines 132-164 (cooling)

**Acceptance:** All AC/HP specs contain `"startup_capacity_degradation": 0.7..0.9`

---

## HIGH Severity Issues

### Issue #3: Water Heater UA Calculation Incomplete
- [ ] Integrate tank jacket R-value into UA reduction
  - [ ] Calculate tank diameter from volume and height
  - [ ] Calculate surface area (sides only, not ends)
  - [ ] Apply jacket insulation formula (OCHRE lines 1133-1136)
  - [ ] Reduce UA by effective jacket resistance
- [ ] Implement HPWH volume-dependent fixed UA
  - [ ] vol ≤ 58 gal → UA = 3.6 W/K
  - [ ] 58 < vol ≤ 73 gal → UA = 4.0 W/K
  - [ ] vol > 73 gal → UA = 4.7 W/K
- [ ] Add performance adjustment logic for instantaneous
  - [ ] If EF present (not UEF) → 0.92
  - [ ] If UEF present (not EF) → 0.94
- [ ] Add validation: ua >= 0, efficiency <= 1.0
- [ ] Test file: `crates/hares-io/src/hpxml/water_heater_ua.rs`

**Acceptance:**
- Storage WH with jacket R-value shows 20-30% lower standby loss than before
- HPWH params include volume-dependent UA, not calculated from EF
- Instantaneous tank has ua=0, correct performance adjustment

---

### Issue #4: Heat Pump Backup Heating System Incomplete
- [ ] Implement fallback logic for lockout temperatures
  - [ ] If CompressorLockoutTemperature missing → use BackupHeatingSwitchoverTemperature
  - [ ] If that also missing → HP lockout default = 0°F
  - [ ] If BackupHeatingLockoutTemperature missing → use BackupHeatingSwitchoverTemperature
  - [ ] If that also missing → ER lockout default = 40°F
- [ ] Add validation: backup_cop not 0 or None
- [ ] Add warning if backup_fuel != "electricity"
- [ ] Test file: `crates/hares-io/src/hpxml/equipment.rs:187-224`

**Acceptance:**
- Cold-climate ASHP without explicit lockout temps gets 0°F and 40°F defaults
- Backup EIR calculation never divides by zero

---

## MEDIUM Severity Issues

### Issue #5: HVAC Auxiliary Power Calculation Mismatch
- [ ] Complete CFM-based auxiliary power calculation
  - [ ] Extract FanPowerWattsPerCFM from extension
  - [ ] Use 350 CFM/ton for heating, 312 CFM/ton for cooling
  - [ ] Multiply by capacity in refrigeration tons
  - [ ] Merge into single `auxiliary_power_w` parameter
- [ ] Handle direct FanPowerWatts extraction (fallback)
- [ ] Handle ElectricAuxiliaryEnergy (kWh/year → W) with 2080 hrs/year
- [ ] Remove intermediate params like "fan_power_w_per_cfm" (merge into final value)
- [ ] Test file: `crates/hares-io/src/hpxml/equipment.rs:119-124, 153-158`

**Acceptance:**
- 4-ton furnace with 0.3 W/CFM shows aux_power = 420 W (not stored separately)
- Single auxiliary_power_w parameter in output, not multiple intermediate params

---

### Issue #6: HVAC Duct Parameters Incomplete
- [ ] Add DuctSurfaceArea to DuctSystem struct
- [ ] Add DuctType field (distinguish supply vs return)
- [ ] Enable validation: exactly 2 ducts with matching types
- [ ] Prepare for ASHRAE 152 DSE calculation in downstream HVAC model
- [ ] Test file: `crates/hares-io/src/hpxml/building.rs:113-118`

**Acceptance:**
- DuctSystem struct includes surface_area_ft2, duct_type fields
- No DSE calculation in hpxml module yet (defer to HVAC equipment model)

---

### Issue #7: Water Heater Location Zone Mapping Missing
- [ ] Implement or import zone_name parsing function
  - [ ] Match OCHRE parse_zone_name logic
  - [ ] Map HPXML text → canonical zones (Indoor, Foundation, Garage, Attic, etc.)
- [ ] Apply to WH location parameter
- [ ] Store as `"zone"` (not `"location"`)
- [ ] Test file: `crates/hares-io/src/hpxml/equipment.rs:446-449`

**Acceptance:**
- WH with Location="basement" → `zone="Foundation"` (canonical)
- WH with Location="conditioned space" → `zone="Indoor"` (canonical)

---

### Issue #8: ZIP Parameters Loading & Defaults Integration
- [ ] Verify ZIP params are merged into equipment config
  - [ ] Confirm parameter names (Zp, Ip, Pp, Zq, Iq, Pq, pf)
  - [ ] Check if loaded from `defaults/zip_parameters.toml`
  - [ ] Confirm merged into EquipmentSpec.parameters (not separate field)
- [ ] Verify HVAC biquadratic curve loading
  - [ ] From `defaults/hvac_heating/*.toml` and `defaults/hvac_cooling/*.toml`
  - [ ] Curves keyed by equipment type name (case-insensitive)
- [ ] Test file: `crates/hares-io/src/hpxml/equipment.rs:1097-1135`, `defaults.rs`

**Acceptance:**
- Equipment with name "Air Conditioner" gets ZIP params from AC row
- HVAC curves available by equipment type lookup

---

## LOW Severity Issues (Nice-to-Have)

### Issue #9: Efficiency Unit Normalization Clarification
- [ ] Verify downstream Equipment model expectations
  - [ ] Does it expect `heating_efficiency_units` + `heating_efficiency`?
  - [ ] Or single `EIR` parameter like OCHRE?
- [ ] Document which parameters the model actually uses
- [ ] Consider consolidating to single COP-based parameter if model allows
- [ ] Verify SEER2/HSPF2 conversion factor (1.0/0.95) is correct for model
- [ ] Test file: `crates/hares-io/src/hpxml/equipment.rs:1189-1241`

**Acceptance:** Equipment model documentation clarifies which efficiency parameters it reads

---

### Issue #10: HVAC Setpoint Scheduling Verification
- [ ] Confirm building.rs parses hourly setpoint arrays correctly
  - [ ] 24 values extracted (or constant value expanded to 24)
  - [ ] Correct F→C conversion
  - [ ] Stored in Building struct fields
- [ ] Verify inject_setpoint_profiles() applies to correct HVAC equipment
- [ ] Test with both hourly and constant setpoint HPXML inputs
- [ ] Test file: `crates/hares-io/src/hpxml/equipment.rs:1415-1468`, `building.rs:139-143`

**Acceptance:** HVAC equipment specs include `heating_weekday_setpoints_c` as 24-element array

---

## Testing Checklist

### Unit Test Coverage
- [ ] Test HVAC speed determination with all tier fallbacks
- [ ] Test C_D calculation for all equipment types
- [ ] Test WH UA with/without jacket R-value
- [ ] Test auxiliary power CFM calculation
- [ ] Test zone name parsing for all known location strings
- [ ] Test backup heating lockout with missing fields

### Integration Test Cases
- [ ] Parse BESTEST-600 (ASHP, no explicit CompressorType) → verify speed count
- [ ] Parse ResStock case (furnace with ElectricAuxiliaryEnergy) → verify aux power
- [ ] Parse case with HPWH → verify volume-based UA
- [ ] Parse case with WH in basement → verify zone="Foundation"
- [ ] Parse case with backup heating → verify lockout temps with fallbacks

### Comparison Against OCHRE
- [ ] Run same HPXML through both parsers
- [ ] Compare equipment config dicts:
  - [ ] All parameter names match (or documented alias)
  - [ ] All numeric values within tolerance (±0.1% for floats)
  - [ ] No missing required parameters
- [ ] Run simulation and compare:
  - [ ] Hourly heating/cooling power (±2% over day)
  - [ ] Energy consumption annual totals (±5%)
  - [ ] Zone temperatures (±0.5°C mean)

---

## Remediation Priority

### Phase 1 (Critical, ~1-2 days)
1. Speed count calculation (#1)
2. Startup C_D calculation (#2)
3. WH zone name parsing (#7)

### Phase 2 (High, ~2-3 days)
4. WH UA jacket integration (#3)
5. HP backup heating fallbacks (#4)

### Phase 3 (Medium, ~1-2 days)
6. Auxiliary power CFM calculation (#5)
7. Duct parameter expansion (#6)
8. ZIP/HVAC defaults verification (#8)

### Phase 4 (Polish, ~1 day)
9. Efficiency unit clarification (#9)
10. Setpoint scheduling verification (#10)

---

## Sign-Off Checklist

Once all issues are resolved:

- [ ] All CRITICAL issues fixed and unit tested
- [ ] All HIGH severity issues fixed and unit tested
- [ ] Integration tests passing (BESTEST suite)
- [ ] Parity test suite updated and passing
- [ ] Comparison against OCHRE reference case completed
- [ ] Documentation updated (equipment.rs comments, defaults loading notes)
- [ ] Code review completed by second team member
- [ ] Commit message references this audit report

**Final Status: ______ (PASS/FAIL)**
**Date Completed: ______**
**Verified By: ______**

