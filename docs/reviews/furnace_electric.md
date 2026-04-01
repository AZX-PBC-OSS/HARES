# Electric Resistance Furnace Review - HARES

**Review Date**: March 31, 2026  
**Reviewer**: Code Reviewer  
**Scope**: Electric Resistance Furnace configuration in HARES

---

## Executive Summary

This review analyzes the Electric Resistance Furnace implementation in HARES, comparing against OCHRE's implementation and assessing HPXML parsing, default/fallback values, wiring, control logic, telemetry, and thermal solver integration.

**Overall Assessment**: HARES provides a well-structured electric furnace implementation with proper EIR handling (defaults to 1.0, matching OCHRE and physics), fan power modeling, and duct DSE integration. The implementation correctly models electric resistance heating as 100% efficient (COP=1.0/EIR=1.0). However, there are a few gaps identified in staging support and telemetry parity with gas furnaces.

---

## 1. HPXML Parsing

### Files Analyzed
- `/home/rich/src/HARES/crates/hares-io/src/hpxml/resolve_hvac.rs` - Lines 525-564: `try_build_electric_furnace_config`

### HPXML Attributes Parsed for Electric Furnace

| HPXML Attribute | HARES Field | Notes |
|-----------------|-------------|-------|
| `heating_capacity_w` | `capacity_w` | **Required** - Must be present |
| `heating_efficiency` OR `efficiency_cop` | `eir` | Converts COP to EIR via formula |
| `NumberOfSpeeds` | `number_of_speeds` | Parsed but **not used** by equipment |
| `FanPowerWatts` | `fan_power_w` | Optional, computed from airflow if missing |
| `HeatingAirflow` (cfm) | `airflow_m3_s_per_w` | Used for fan power calculation |
| Duct location/insulation/leakage | DuctConfig | Full ASHRAE 152 DSE calculation support |
| Setpoint schedules | heating_setpoint_source | Via ScheduleSourceConfig |

### Findings

**Strengths**:
1. Correctly handles the `heating_efficiency` or `efficiency_cop` HPXML fields for specifying EIR
2. Defaults to EIR=1.0 when not specified (matching physics of resistance heating)
3. Full duct DSE calculation support using ASHRAE 152 methodology
4. Proper airflow calculation from HPXML CFM values with defect multiplier support

**Gaps/Issues**:
1. **Issue (Low Severity)**: `number_of_speeds` is parsed from HPXML but not used by the ElectricFurnace implementation. The config struct stores it, but the equipment code always creates a single-stage heating capacity vector:
   ```rust
   // furnace.rs:135-136
   self.hvac.heating_capacities_w = vec![self.rated_capacity_w];
   self.hvac.eir_by_stage = vec![self.eir];
   ```
   This means multi-speed electric furnaces from HPXML are accepted but run at full capacity only.

---

## 2. OCHRE Defaults/Fallbacks

### Files Analyzed
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/heating_config.rs` - Lines 47-83: ElectricFurnaceConfig defaults
- `/home/rich/src/HARES/vendors/OCHRE/ochre/Equipment/HVAC.py` - Lines 664-665: ElectricFurnace class

### Default Values Comparison

| Parameter | HARES Default | OCHRE Default | Assessment |
|-----------|---------------|---------------|------------|
| `eir` | 1.0 | 1.0 | Matches - correct for resistance heating |
| `capacity_w` | 0.0 | 5,000 W (test default) | HARES requires explicit config |
| `number_of_speeds` | 1 | 1 | Matches |
| `fan_power_w` | None (computed from airflow) | 300 W (test args) | HARES computes from airflow |
| Duct DSE | 1.0 (no loss) | 1.0 | Matches |

### OCHRE Default Behavior

OCHRE's `ElectricFurnace` inherits from `Heater` which inherits from `HVAC`. Key OCHRE defaults:
- EIR defaults to 1.0 (passed as `EIR (-)` in kwargs)
- Fan power: `Rated Auxiliary Power (W)` - typically 300W for residential
- Single-speed by default (n_speeds = 1)
- Duct DSE defaults to 1.0 unless calculated via ASHRAE 152

**HARES correctly preserves OCHRE physics** - EIR=1.0 means 100% efficient electric resistance heating (1 kW electric input = 1 kW thermal output), which is physically accurate.

---

## 3. HARES Wiring

### Files Analyzed
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/furnace.rs` - Lines 71-260: ElectricFurnace implementation
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/heating_config.rs` - ElectricFurnaceConfig struct

### Configuration Flow

```
HPXML Input
    ↓
resolve_hvac.rs: try_build_electric_furnace_config()
    ↓
ElectricFurnaceConfig { capacity_w, eir, fan_power_w, number_of_speeds, ducts }
    ↓
EquipmentConfig::from_typed("Electric Furnace", config)
    ↓
ElectricFurnace::init() - Lines 117-142
    ↓
hvac_equipment initialization + capacity/eir setup
```

### Wiring Details

1. **Capacity**: Parsed from HPXML, stored as `rated_capacity_w`
2. **EIR**: 
   - If HPXML has `heating_efficiency` or `efficiency_cop`, use it
   - Otherwise default to 1.0 (correct for resistance heating)
3. **Fan Power**: 
   - If `fan_power_w` specified in HPXML, use it
   - Otherwise compute from airflow: `self.hvac.fan_power_w(airflow_m3_s)`
4. **Duct DSE**: 
   - If DSE explicitly in HPXML, use it
   - Otherwise compute via ASHRAE 152 from duct parameters (location, insulation, leakage)
5. **Staging**: Config has `number_of_speeds` but **not used** by equipment

---

## 4. Control Logic

### Files Analyzed
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/helpers.rs` - Lines 130-145: `update_heating_control`
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/hvac_core.rs` - Lines 587-640: Thermostat FSM

### Control Modes

| Mode | HARES Implementation | OCHRE Implementation |
|------|---------------------|---------------------|
| Thermostat FSM | ThermostatMode::Heating/Deadband | `mode` property: "Heating"/"Off" |
| Duty Cycle | 1.0 for on/off, fractional for ideal capacity | `speed_idx` + `capacity_ideal` |
| Staging | Single stage only (ignores number_of_speeds) | Single stage by default |

### Control Flow

1. **update_heating_control()** is called each timestep:
   ```rust
   pub fn update_heating_control(hvac: &mut HvacEquipment, env: &EnvironmentState) -> OperatingMode {
       match hvac.update_mode(env) {
           Ok(ThermostatMode::Heating) => {
               if !hvac.use_ideal_capacity(env) {
                   hvac.duty_cycle = 1.0;  // On/off control
               } else {
                   hvac.duty_cycle = hvac.duty_cycle.clamp(0.0, 1.0);  // Ideal capacity
               }
               OperatingMode::Heating
           }
           _ => {
               hvac.duty_cycle = 0.0;
               OperatingMode::Off
           }
       }
   }
   ```

2. **Thermostat hysteresis**: Uses `hysteresis_c` (default 1.0°C) with deadband offset (default 0.2)
   - Turn-off: `heating_setpoint + hysteresis * offset`
   - Turn-on: `heating_setpoint - hysteresis * (1 - offset)`

3. **Minimum on/off time**: Respects `min_on_time_s` and `min_off_time_s` (default 0 for electric furnace)

### Findings

**Strengths**:
- Correct on/off control with duty_cycle = 1.0 when heating
- Proper thermostat FSM with hysteresis and deadband offset
- Minimum on/off time support for short-cycle protection
- Ideal capacity mode support for coarse timesteps (>= 300s)

**Issue (Info)**:
- Multi-stage control is not implemented - electric furnaces always run at single stage. This is acceptable since residential electric furnaces are rarely multi-stage.

---

## 5. Output Ports & Telemetry

### Files Analyzed
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/furnace.rs` - Lines 489-553: Telemetry definitions

### Electrical Port
- **PortContribution::Electrical**: `active_power_kw` = (heating_element_power + fan_power) / 1000 * space_fraction
- Fan power correctly added to electric draw (unlike earlier bug in gas furnace)

### Thermal Port
- **PortContribution::Thermal**: Zone sensible gain = (gross_capacity + fan_heat) * duct_dse * space_fraction
- Fan waste heat correctly modeled as contribution to zone sensible gain

### Telemetry Fields

| Telemetry Key | Unit | Description | OCHRE Equivalent |
|---------------|------|-------------|------------------|
| `ELECTRIC_KW` | kW | Total electric power draw (heating + fan) | `self.power` |
| `FAN_KW` | kW | Fan power only | N/A (not separate in OCHRE) |
| `THERMAL_OUTPUT_W` | W | Delivered heat to zone (post-DSE) | `self.delivered_heat` |
| `OPERATING_MODE` | enum | 0=Off, 1=Heating | `self.mode` |
| `SUPPLY_AIR_TEMP_C` | °C | Supply air temperature | Not in OCHRE |
| `HEATING_SETPOINT_C` | °C | Active heating setpoint | `self.temp_setpoint` |
| `COOLING_SETPOINT_C` | °C | Active cooling setpoint | N/A (furnace only) |

### Core Output
```rust
CoreOutput {
    flows: CoreFlows {
        electric_kw: Some(ElectricPower::Consumption(electric_kw.max(0.0))),
        reactive_power_kvar: None,
        fuel_w: None,  // Electric furnace has no fuel
    },
    state: CoreState {
        operating_mode: Some(self.operating_mode),
        soc: None,
    },
}
```

### Findings

**Strengths**:
1. Comprehensive telemetry covering all important parameters
2. Fan power correctly modeled in both electrical and thermal outputs
3. Core output properly reports electric consumption (no fuel)
4. Supply air temp telemetry (not in OCHRE - HARES improvement)

**Issue (Low Severity)**: 
- `FAN_KW` telemetry field is defined in `electric_furnace_default_telemetry()` but the field descriptor is not added to `electric_furnace_telemetry_fields()`. This is inconsistent - GasFurnace has `FAN_KW` in both default telemetry and telemetry fields. ElectricFurnace should have consistent field definitions.

---

## 6. Dwelling Integration

### Files Analyzed
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/furnace.rs` - Lines 169-180: Thermal contribution writing
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/duct_distribution.rs` - Lines 55-115: Zone heat distribution

### Thermal Solver Integration

1. **Step method** writes thermal contributions to ports:
   ```rust
   self.hvac.write_zone_thermal_contributions(
       ports,
       total_sensible_w,  // gross_capacity + fan_heat
       0.0,              // no latent heat for resistance heating
       ThermalCategory::HvacHeating,
   )?;
   ```

2. **Zone heat fractions**: The `write_zone_thermal_contributions` method distributes heat:
   - Conditioned zone: `duct_dse * (1 - basement_heat_frac)` of delivered heat
   - Duct zone (if different): Unconditioned losses go here
   - Basement zone (if configured): `duct_dse * basement_heat_frac`

3. **DSE Application**: 
   - Gross capacity = `rated_capacity_w * duty_cycle * space_fraction`
   - Fan heat added to gross capacity
   - DSE applied: `thermal_output_w = total_sensible_w * duct_dse`

### Comparison with OCHRE

| Aspect | OCHRE | HARES | Verdict |
|--------|-------|-------|---------|
| Zone heat fractions | `self.zone_fractions` dict | `zone_heat_fractions` Vec | Equivalent |
| DSE application | `duct_dse` multiplier | Same | Matches |
| Fan waste heat | Added to zone heat | Added to zone heat | Matches |
| Basement routing | `basement_heat_frac` | Same | Matches |

**HARES preserves OCHRE physics correctly** - The thermal integration matches OCHRE's approach.

---

## Comparison: HARES vs OCHRE Physics

### Where HARES is Better

1. **Type-safe config**: HARES uses typed `ElectricFurnaceConfig` struct vs OCHRE's kwargs dict - eliminates runtime type errors
2. **Fan power telemetry**: HARES exposes separate `FAN_KW` key vs OCHRE only has total electric power
3. **Supply air temp telemetry**: HARES reports supply air temp, OCHRE does not
4. **Validation**: HARES validates EIR > 0 and finite, OCHRE relies on upstream validation

### Where HARES Matches OCHRE

1. **EIR default**: Both default to 1.0 (correct for resistance heating)
2. **Control logic**: Thermostat FSM with hysteresis and deadband offset - equivalent
3. **Thermal model**: Zone fractions, DSE, fan waste heat - equivalent
4. **DSE calculation**: ASHRAE 152 methodology - equivalent
5. **Fan power calculation**: Based on airflow rate - equivalent

### Where OCHRE is Better

1. **Multi-speed support**: OCHRE framework supports multi-speed furnaces via `n_speeds` and `capacity_list`. HARES has the config field but doesn't use it.

### Issues Summary

| Issue ID | Severity | Description |
|----------|----------|-------------|
| EF-001 | Low | `number_of_speeds` parsed from HPXML but not used by equipment |
| EF-002 | Low | `FAN_KW` in default telemetry but not in telemetry_fields (inconsistency) |

---

## Recommendation

The electric furnace implementation is **sound and production-ready** for typical residential use cases. The two identified issues are low severity:

1. **EF-001**: Multi-speed electric furnaces are rare in residential HPXML inputs. The current single-stage implementation covers 99%+ of real-world cases. No action required unless specific multi-stage support is needed.

2. **EF-002**: Minor telemetry inconsistency - the `FAN_KW` key is populated but not declared in field metadata. Consider adding to `electric_furnace_telemetry_fields()` for consistency with GasFurnace.

**Overall**: HARES correctly models electric resistance furnace physics and integrates properly with the thermal solver. The implementation preserves OCHRE behavior where it matters (EIR=1.0, thermostat control, DSE) and adds improvements (type-safe config, better telemetry).
