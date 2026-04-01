# Variable-Speed (Inverter-Driven) Air-Source Heat Pump (ASHP) Heating Review

**Review Date**: 2025-03-31  
**Equipment Type**: ASHP Heater (Variable Speed) / MSHP Heater  
**HARES Module**: `hares-equipment::hvac::heat_pump`  

---

## 1. HPXML Parsing

### 1.1 Parsed HPXML Attributes

HARES parses the following HPXML attributes for variable-speed ASHP heating:

| HPXML Element | HARES Parameter | Notes |
|---------------|-----------------|-------|
| `HeatPumpType` | Equipment type ("air-to-air" -> ASHP, "mini-split" -> MSHP) | Required |
| `HeatingCapacity` | `heating_capacity_w` | In watts |
| `CoolingCapacity` | `cooling_capacity_w` | In watts |
| `AnnualHeatingEfficiency` (HSPF) | `heating_eir` = 1/HSPF | Converted to EIR |
| `AnnualCoolingEfficiency` (SEER) | `cooling_eir` = 1/SEER | Converted to EIR |
| `CompressorType` | `number_of_speeds` | "variable speed" -> 4 speeds |
| `BackupHeatingCapacity` | `backup_capacity_w` | ER backup capacity |
| `BackupAnnualHeatingEfficiency` | `backup_eir` | ER efficiency (EIR = 1/COP) |
| `BackupSystemFuel` | `backup_fuel` | Backup fuel type |
| `CompressorLockoutTemperature` | `hp_lockout_temp_c` | HP lockout temperature |
| `BackupHeatingLockoutTemperature` | `er_lockout_temp_c` | ER lockout temperature |
| `CoolingSensibleHeatFraction` | `shr` | SHR for cooling |
| `FractionHeatingLoadServed` | `fraction_heating_load_served` | Load fraction |
| `extension/FanPowerWattsPerCFM` | `fan_power_w_per_cfm` | Fan power |
| `extension/AirflowDefectRatio` | `airflow_defect_ratio` | Airflow defect |

### 1.2 Speed Detection Logic

The number of compressor speeds is determined from HPXML:
- `"variable speed"` CompressorType -> 4 speeds
- Mini-split (any type) -> 4 speeds (forced)
- SEER > 21 -> 4 speeds (inferred fallback)

**Source**: `crates/hares-io/src/hpxml/resolve_hvac.rs` lines 861-876

### 1.3 Capacity Curves Parsed

When `number_of_speeds >= 2`, HARES loads multispeed parameters from `HVAC Multispeed Parameters.csv`:
- Capacity ratios per stage (e.g., 0.33, 0.56, 1.0, 1.17 for ASHP 10+ HSPF)
- Airflow ratios per stage
- COP per stage
- SHR per stage (cooling only)
- Biquadratic curve coefficients for capacity and EIR
- PLF (Part Load Factor) coefficients

**Source**: `crates/hares-io/src/hpxml/resolve_hvac.rs` lines 1831-1927

---

## 2. OCHRE Defaults

### 2.1 Default Values (HARES matches OCHRE)

| Parameter | Default Value | Source |
|-----------|---------------|--------|
| Default heating capacity | 10,000 W | HARES constants |
| Default heating EIR | 0.35 (COP ~2.86) | HARES constants |
| Default backup capacity | 5,000 W | HARES constants |
| Default backup EIR | 1.0 (COP = 1.0) | HARES constants |
| HP lockout temp | -17.78°C (0°F) | OCHRE parity |
| ER lockout temp | 4.44°C (40°F) | OCHRE parity |
| Min ER cycle time | 0.0 s | OCHRE default (no minimum) |
| ER hard lockout time | 0.0 s | OCHRE default |
| Max OAT supplemental | 21°C | EnergyPlus default |
| Fan power (default) | 0.365 W/CFM | ACCA Manual D |
| Airflow (heating) | 4.70e-5 m³/s/W | OCHRE/ResStock default |
| Speed control mode | MultiSpeedInterpolated | For 4+ speeds |

### 2.2 Multispeed Parameter Defaults (from CSV)

For variable-speed ASHP heating (4 speeds, HSPF >= 10):

| Stage | Capacity Ratio | Airflow Ratio | COP |
|-------|----------------|---------------|-----|
| 1 (lowest) | 0.33 | 0.63 | 5.67 |
| 2 | 0.56 | 0.76 | 4.84 |
| 3 | 1.0 | 1.0 | 4.09 |
| 4 (highest) | 1.17 | 1.19 | 3.91 |

**Source**: `/home/rich/src/HARES/defaults/HVAC Multispeed Parameters.csv` (matching OCHRE)

### 2.3 Part Load COP

OCHRE/HARES uses biquadratic curves to compute part-load COP:
- Capacity biquadratic: adjusts capacity based on indoor temp, outdoor temp, flow fraction
- EIR biquadratic: adjusts EIR based on conditions
- PLF (Part Load Factor): accounts for cycling degradation at low part-load ratios

Default PLF degradation coefficient: 0.25 (StartupConfig.c_d default)

**Note**: Variable-speed equipment uses c_d = 0.0 (no startup degradation), which is correct - inverter-driven equipment doesn't have the same cycling losses as single-stage compressors.

---

## 3. HARES Wiring (HPXML to Equipment)

### 3.1 Configuration Flow

```
HPXML Parse → resolve_hvac.rs → apply_multispeed_parameters()
            → try_build_heat_pump_heater_config()
            → HeatPumpHeaterConfig (typed)
            → ASHPHeater::new() / MinisplitHeater::new()
            → HeatPumpHeaterCore::init_from_typed()
```

### 3.2 Speed Control Mode Selection

HARES maps speed configuration to control modes:

| number_of_speeds | speed_control_mode | Control Type |
|------------------|-------------------|--------------|
| 0 or 1 | SingleSpeed | On/off only |
| 2 | TwoSpeedTime | Low/high with time-based transition |
| >=4 (or mini-split) | MultiSpeedInterpolated | Continuous variable speed |

**Source**: `crates/hares-equipment/src/hvac/heat_pump/heater.rs` lines 462-477

### 3.3 Equipment Type Mapping

- `ASHP Heater` (air-to-air, non-mini-split) → `HvacEquipmentType::AshpHeatPumpOnly` or `AshpHeatPumpAux`
- `MSHP Heater` (mini-split) → `HvacEquipmentType::MiniSplitHeat`

---

## 4. Control Logic

### 4.1 Capacity Modulation

For variable-speed (MultiSpeedInterpolated) mode:

1. **Speed Selection**: Uses zone temperature error to select speed index
   - `load_ratio = (setpoint - zone_temp) / deadband`
   - `select_speed_with_zone_temp()` interpolates between stages

2. **Capacity Interpolation**: Linear interpolation between stage capacities
   ```rust
   capacity = stage_capacity[speed_idx] * (1 - frac) + stage_capacity[speed_idx + 1] * frac
   ```

3. **Part Load Ratio (PLR)**: 
   - At lowest speed: PLR = duty_cycle (cycling below stage capacity)
   - At higher speeds: PLR derived from solver's ideal capacity

**Source**: `crates/hares-equipment/src/hvac/heat_pump/heater.rs` lines 733-782

### 4.2 Inverter Frequency Control

**Issue Identified**: HARES does NOT model inverter frequency directly. The "variable speed" control is achieved through:
- Speed index selection based on load ratio
- Linear interpolation between stage capacities
- Duty cycle (PLR) for part-load operation

This is an **abstraction** - actual inverter-driven ASHPs modulate compressor frequency continuously, but HARES models this as discrete speed stages with interpolation, matching OCHRE's approach.

### 4.3 Backup Staging Integration

ER (Electric Resistance backup) integration in HARES:

1. **HP-only operation**: When zone temp > (setpoint - er_setpoint_offset_c)
2. **HP + ER operation**: When zone temp < (setpoint - er_setpoint_offset_c) AND conditions allow:
   - Outdoor temp < er_lockout_temp_c
   - Outdoor temp <= max_oat_supplemental_c (21°C)
   - ER hard lockout expired
   - ER soft lockout not active (zone not rising)

3. **Capacity allocation**:
   - HP provides capacity up to its max
   - ER provides `ideal_capacity - hp_capacity` (ideal mode)
   - ER provides full backup capacity (static mode)

**Source**: `crates/hares-equipment/src/hvac/heat_pump/heater.rs` lines 855-1015

---

## 5. Output Ports & Telemetry

### 5.1 Available Telemetry Fields

| Telemetry Key | Description | Units |
|---------------|-------------|-------|
| `electric_kw` | Total electric power | kW |
| `thermal_output_w` | Delivered thermal output | W |
| `operating_mode` | Mode code (Off, HeatingHP, HeatingER, HeatingHPAndER) | - |
| `speed_index` | Current speed stage index | - |
| `defrost_active` | Defrost active flag | 0/1 |
| `cop` | Coefficient of Performance (thermal/compressor) | - |
| `runtime_fraction` | Part-load ratio (duty cycle) | - |
| `compressor_kw` | Compressor-only power (excludes fan) | kW |
| `defrost_time_fraction` | Defrost time fraction | - |
| `heating_setpoint_c` | Active heating setpoint | °C |
| `cooling_setpoint_c` | Active cooling setpoint | °C |

**Source**: `crates/hares-types/src/telemetry_keys.rs`

### 5.2 Missing Telemetry (Compared to Inverter Concept)

| Desired Telemetry | Status | Notes |
|-------------------|--------|-------|
| Inverter frequency | **NOT REPORTED** | No Hz telemetry for ASHP |
| Modulation level | **PARTIAL** | speed_index + runtime_fraction provide proxy |
| Inverter efficiency | **NOT REPORTED** | Unlike PV/Battery, ASHP doesn't report inverter efficiency |
| Part load COP (explicit) | **IMPLICIT** | COP is computed but not explicitly labeled as "part load" |

**Issue**: Variable-speed ASHP lacks explicit inverter efficiency telemetry. PV and Battery equipment have `inverter_efficiency` telemetry, but ASHP does not.

---

## 6. Dwelling Integration

### 6.1 Thermal Solver Integration

HARES uses **ideal capacity mode** for variable-speed ASHP:

1. **Solver provides ideal capacity**: `ideal_capacity_w` from thermal solver
2. **PLR derivation**: `PLR = ideal_capacity / (biquadratic_corrected_capacity + backup_capacity)`
3. **Speed selection**: Based on PLR, interpolates to appropriate speed
4. **Zone temperature feedback**: Solver uses delivered heat to compute next step

**Source**: `crates/hares-equipment/src/hvac/heat_pump/heater.rs` lines 765-782

### 6.2 Zone Heat Contributions

Heat is distributed to zones based on:
- `duct_dse`: Distribution system efficiency (losses to duct zone)
- `basement_heat_frac`: Fraction to basement zone
- Remaining to conditioned zone

### 6.3 Supply Air Temperature

- ASHP (no backup): `32.2 + 0.15 * (outdoor_temp - 8.3)` °C
- ASHP (with backup): 40.6°C fixed
- MSHP: 43.3°C fixed

---

## 7. Comparison Summary: HARES vs OCHRE

### 7.1 Where HARES Physics Preserves OCHRE

| Aspect | HARES Implementation | OCHRE Parity |
|--------|---------------------|--------------|
| Multispeed parameter tables | Identical CSV | MATCH |
| Biquadratic curves | Same coefficients | MATCH |
| Speed selection logic | Load-ratio based | MATCH |
| Defrost model | EnergyPlus on-demand | MATCH |
| ER staging logic | Hard/soft lockout | MATCH |
| HP lockout temperatures | -17.78°C / 4.44°C defaults | MATCH |
| DSE calculation | ASHRAE 152 method | MATCH |
| Supply air temp formula | Same equations | MATCH |

### 7.2 Where HARES Has Different/Unknown Defaults

| Aspect | HARES | Issue |
|--------|-------|-------|
| Inverter efficiency | Not modeled for ASHP | Missing physics - real inverter-driven ASHPs have inverter losses |
| Frequency reporting | Not available | Can't verify inverter is operating in optimal range |
| Speed control mode | Has `VariableSpeedIdeal` not in OCHRE | Extra mode, behavior unclear vs MultiSpeedInterpolated |
| Fan power curve | 0.365 W/CFM (default) | Same as OCHRE but hard to verify from HPXML |

### 7.3 Issues Documented

1. **No inverter efficiency modeling**: Real inverter-driven ASHPs have power electronics losses (typically 95-97% efficient). HARES treats the compressor power as directly proportional to capacity * EIR, ignoring inverter losses.

2. **No frequency telemetry**: Cannot verify the inverter is operating at optimal frequency. speed_index is a proxy but not equivalent to Hz.

3. **Speed control mode ambiguity**: HARES has both `MultiSpeedInterpolated` and `VariableSpeedIdeal` modes. The difference is not clearly documented.

4. **Part-load COP labeling**: COP is reported but not explicitly identified as "part-load" vs "full-load". At low speeds/COP values change significantly.

---

## 8. Recommendations

1. **Add inverter efficiency telemetry** for variable-speed ASHP (similar to PV/Battery). Typical values: 95-97%.

2. **Consider adding frequency proxy**: If min/max frequency bounds are available from HPXML, compute a normalized modulation level (0-100%).

3. **Document VariableSpeedIdeal vs MultiSpeedInterpolated**: Clarify when each mode is used.

4. **Verify startup degradation disabled for variable-speed**: Currently c_d=0.0 for MSHP which is correct, but could be made explicit in config validation.

---

## References

- HARES HVAC core: `crates/hares-equipment/src/hvac/hvac_core.rs`
- ASHP heater: `crates/hares-equipment/src/hvac/heat_pump/heater.rs`
- HPXML resolver: `crates/hares-io/src/hpxml/resolve_hvac.rs`
- Speed control: `crates/hares-equipment/src/hvac/speed_control.rs`
- OCHRE HVAC.py: `vendors/OCHRE/ochre/Equipment/HVAC.py`
- Multispeed defaults: `defaults/HVAC Multispeed Parameters.csv`
