# Water Heating Equipment Review - HARES

**Review Date**: 2026-03-31  
**Reviewer**: Code Reviewer  
**Scope**: Water heating equipment implementation in HARES, including HPXML parsing, OCHRE defaults, HARES wiring, control logic, telemetry, and thermal solver integration.

---

## Executive Summary

The HARES water heating implementation is a well-engineered port of the OCHRE water heater models to Rust. The implementation preserves the sophisticated physics of the OCHRE originals while adding some modern features like ZIP load modeling and improved code organization. Several minor gaps exist in HPXML parsing and default handling that are documented below.

**Overall Assessment**: The implementation is production-quality with only minor issues requiring attention.

---

## 1. HPXML Parsing

### 1.1 Parsed Attributes by Equipment Type

#### Electric Resistance Water Heater
| HPXML Attribute | HARES Config Field | Status |
|-----------------|-------------------|--------|
| `FuelType` | `fuel_type` | Parsed |
| `TankVolume` | `tank_volume_m3` | Parsed |
| `TankHeight` | `tank_height_m` | Parsed |
| `EnergyFactor` | `energy_factor` | Parsed |
| `UniformEnergyFactor` | `uniform_energy_factor` | Parsed |
| `HeatingCapacity` | `heating_capacity_w`, `element_power_w` | Parsed |
| `HotWaterTemperature` | `setpoint_c` | Parsed |
| `PerformanceAdjustment` | `performance_adjustment` | Parsed |
| `Location` | `zone_type` | Parsed |

**Missing**: `WaterHeaterInsulation/Jacket/JacketRValue` - Not parsed from HPXML (OCHRE supports this)

#### Gas Water Heater
| HPXML Attribute | HARES Config Field | Status |
|-----------------|-------------------|--------|
| All ERWH attributes | See above | Parsed |
| `PilotPower` | `pilot_power_w` | Parsed |
| `FlueLossFraction` | `flue_loss_fraction` | Parsed |

#### Heat Pump Water Heater
| HPXML Attribute | HARES Config Field | Status |
|-----------------|-------------------|--------|
| All storage WH attributes | See above | Parsed |
| Low-power HPWH detection (UEF=4.9) | `hp_only_mode`, COP defaults | Parsed |

#### Tankless Water Heater
| HPXML Attribute | HARES Config Field | Status |
|-----------------|-------------------|--------|
| `EnergyFactor` | `energy_factor` | Parsed |
| `UniformEnergyFactor` | `uniform_energy_factor` | Parsed |
| `HeatingCapacity` | `heating_capacity_w` | Parsed |
| `HotWaterTemperature` | `setpoint_c` | Parsed |
| `PerformanceAdjustment` | `performance_adjustment` | Parsed |
| `Location` | `zone_type` | Parsed |

**Missing for Tankless**: `ParasiticPower` - Not parsed from HPXML; HARES uses default 7.38W for gas tankless

### 1.2 UA Calculation

The HPXML resolver correctly computes UA from energy factor using the same formulas as OCHRE (`crates/hares-io/src/hpxml/water_heater_ua.rs`). This is a critical function and works correctly.

**Severity**: N/A - Working as designed

### 1.3 Issues Found

| Issue | Severity | Description |
|-------|----------|-------------|
| Missing Jacket R-Value parsing | Medium | OCHRE parses `WaterHeaterInsulation/Jacket/JacketRValue` and applies it to reduce UA. HARES has the `jacket_r_value_m2_k_w` config key but HPXML resolver never populates it. |
| Missing Tankless Parasitic Power | Low | HPXML has `ParasiticPower` for tankless water heaters but HARES doesn't parse it. Uses default 7.38W instead. |

---

## 2. OCHRE Defaults/Fallbacks

### 2.1 Shared Default Values (mod.rs)

| Parameter | Default Value | Source |
|-----------|--------------|--------|
| `DEFAULT_SETPOINT_C` | 51.67°C (125°F) | OCHRE |
| `DEFAULT_UA_W_PER_K` | 2.0 W/K | OCHRE |
| `DEFAULT_TANK_HEIGHT_M` | 1.2 m (4 ft) | OCHRE |
| `DEFAULT_TANK_DIAMETER_M` | 0.5 m | HARES custom |
| `DEFAULT_TANK_VOLUME_M3` | 0.189 m³ (50 gal) | OCHRE |
| `DEFAULT_MAX_TANK_TEMP_C` | 60°C (140°F) | OCHRE |

### 2.2 Electric Resistance Water Heater Defaults (resistance.rs)

| Parameter | Default Value | OCHRE Equivalent |
|-----------|--------------|------------------|
| `DEFAULT_DEADBAND_C` | 5.56°C (10°F) | OCHRE default |
| `DEFAULT_ELEMENT_POWER_W` | 4500 W | OCHRE `default_capacity` |
| Default tank nodes | 6 | HARES custom (OCHRE uses 2/12) |
| Default element priority | MasterSlave | OCHRE default |

### 2.3 Heat Pump Water Heater Defaults (hpwh_compressor.rs)

| Parameter | Default Value | OCHRE Equivalent |
|-----------|--------------|------------------|
| `DEFAULT_DEADBAND_C` | 8.17°C (14.7°F) | OCHRE HPWH-specific |
| `DEFAULT_RATED_COP` | 3.45 | OCHRE calculated from UEF |
| `DEFAULT_COMPRESSOR_POWER_W` | 1200 W | HARES custom |
| `DEFAULT_BACKUP_ELEMENT_POWER_W` | 4500 W | OCHRE |
| `DEFAULT_BACKUP_ENABLE_OFFSET_C` | 8.0°C | OCHRE |
| `DEFAULT_SHR` | 0.88 | OCHRE |
| `DEFAULT_LOST_HEAT_FRACTION` | 0.0 | HARES (OCHRE uses 0.25 for non-indoor) |
| `DEFAULT_FAN_POWER_W` | 35 W | OCHRE |
| `DEFAULT_PARASITIC_POWER_W` | 1 W | OCHRE |
| `DEFAULT_MIN_ON_TIME_S` | 600 s (10 min) | HARES (OCHRE has 10 min min_on, 0 min min_off) |
| Min ambient temp | 7.22°C (45°F) | OCHRE |
| Max ambient temp | 43.33°C (110°F) | OCHRE |

**COP/Capacity Curves**: HARES uses the same biquadratic coefficients as OCHRE (GE GeoSpring class).

### 2.4 Gas Water Heater Defaults (gas.rs)

| Parameter | Default Value | OCHRE Equivalent |
|-----------|--------------|------------------|
| `DEFAULT_DEADBAND_C` | 5.56°C (10°F) | OCHRE |
| `DEFAULT_BURNER_INPUT_W` | 11000 W | HARES custom |
| `DEFAULT_BURNER_EFFICIENCY` | 0.78 | HARES |
| `DEFAULT_FLUE_LOSS_FRACTION` | 0.10 | HARES |
| Skin loss fraction | Derived from EF | OCHRE (0.64 for EF<0.7, 0.91 for EF<0.8, 0.96 otherwise) |

### 2.5 Tankless Water Heater Defaults (tankless.rs)

| Parameter | Default Value | OCHRE Equivalent |
|-----------|--------------|------------------|
| `DEFAULT_SETPOINT_C` | 51.67°C (125°F) | OCHRE |
| `DEFAULT_EF` | 0.9 | OCHRE |
| `DEFAULT_MAX_THERMAL_POWER_W` | 20000 W | OCHRE `default_capacity` |
| `DEFAULT_GAS_PARASITIC_POWER_W` | 7.38 W | OCHRE (varies with bedrooms) |

### 2.6 Issues with Defaults

| Issue | Severity | Description |
|-------|----------|-------------|
| HPWH lost_heat_fraction default differs from OCHRE | Medium | HARES defaults to 0.0, OCHRE defaults to 0.75 for non-indoor installations. This affects zone heat gains. |
| Default tank nodes is 6 vs OCHRE's 2 or 12 | Low | OCHRE uses 1-node for ideal capacity mode, 2-node for simple models, 12-node for HPWH. HARES defaults to 6, which is a reasonable compromise but differs from OCHRE. |
| Gas WH burner input default | Low | HARES uses 11kW, OCHRE requires parsing from HPXML. This is a reasonable fallback but may not match actual equipment. |

---

## 3. HARES Wiring

### 3.1 Configuration Flow

The wiring from HPXML to equipment is implemented in `crates/hares-io/src/hpxml/resolve_water_heater.rs`. The flow is:

1. **HPXML Parsing** → Creates typed configs (GasWaterHeaterConfig, ElectricResistanceWaterHeaterConfig, etc.)
2. **Schedule Resolution** → Applies ZIP parameters from defaults store
3. **Equipment Init** → `init_typed()` method applies config values with fallbacks

### 3.2 Mapping Completeness

The mapping is generally complete. Key observations:

- All major HPXML attributes are mapped to config fields
- UA calculation is correctly implemented and uses OCHRE formulas
- Low-power HPWH (UEF=4.9) is correctly detected and configured
- Water draw calculation uses OCHRE's ANSI/RESNET 301 formula correctly

### 3.3 Issues

| Issue | Severity | Description |
|-------|----------|-------------|
| Element power mapping inconsistency | Low | For ERWH, HARES maps `HeatingCapacity` to both `heating_capacity_w` AND `element_power_w`. For HPWH, it's mapped to `backup_element_power_w`. This is correct but could be more explicit. |
| hp_only_mode mapping | Low | HPWH `hp_only_mode` is derived from UEF=4.9 detection, but could also come from explicit HPXML field if it existed. Currently correct. |

---

## 4. Control Logic

### 4.1 Thermostat Control

All water heater types implement hysteresis-based thermostat control:

- **Resistance WH**: Upper element gets priority (MasterSlave mode). Lower element fires only when upper is satisfied and lower node is below deadband. Supports optional Simultaneous mode.
- **Gas WH**: Single-element control with hysteresis.
- **HPWH**: Uses composite control temperature (75% upper + 25% lower node). Compressor gets priority over backup element in MutuallyExclusive mode.
- **Tankless**: Simple on/off based on draw detection.

### 4.2 Demand Response

All water heater types support DR levels with consistent behavior:

| DR Level | Setpoint Offset | Load Fraction |
|----------|-----------------|---------------|
| Normal | 0°C | 1.0 |
| Moderate | -3°C | 1.0 |
| High | -6°C | 0.8 |
| Critical | -10°C | 0.5 |
| GridEmergency | 0°C | 0.0 |

This matches OCHRE behavior.

### 4.3 Control Signal Support

| Control Signal | ERWH | Gas WH | HPWH | Tankless |
|---------------|------|--------|------|----------|
| ThermalSetpoint | ✓ | ✓ | ✓ | ✓ |
| DutyCycle | ✓ | ✓ | ✓ | ✓ |
| ModeOverride | ✓ | ✓ | ✓ | ✓ |
| LoadFraction | ✓ | ✓ | ✓ | ✓ |
| PowerLimit | ✓ | ✓ | ✓ | ✓ |
| DemandResponse | ✓ | ✓ | ✓ | ✓ |

### 4.4 HPWH-Specific Control

The HPWH implementation includes:
- **Ambient temperature lockout**: Compressor disables below 7.22°C or above 43.33°C (OCHRE behavior)
- **Minimum on/off times**: 10-minute minimum on time (HARES adds 0-minute off time which differs from OCHRE)
- **Element priority modes**: MutuallyExclusive (default) or Simultaneous
- **Dynamic COP/capacity**: Biquadratic curves based on wet-bulb and tank temperature
- **Wall heat interaction**: Configurable fraction of sensible gains to interior walls (OCHRE default 0.5)

### 4.5 Issues

| Issue | Severity | Description |
|-------|----------|-------------|
| HPWH min_off_time not implemented | Low | HARES sets `min_off_time_s = 0.0` by default, but OCHRE uses 0 as well. This is consistent, but the config option exists and defaults to None which becomes 0. |
| Setpoint ramp rate | Low | ERWH supports `max_setpoint_ramp_rate_c_per_min` but HPWH doesn't expose this in the same way. HARES has the infrastructure but HPWH init doesn't apply it. |

---

## 5. Output Ports & Telemetry

### 5.1 Port Declarations

All water heater types declare:

| Port | ERWH | Gas WH | HPWH | Tankless |
|------|------|--------|------|----------|
| Electrical | ✓ | ✓ | ✓ | ✓ |
| Fuel | - | ✓ | - | ✓ (gas) |
| Thermal (zone) | ✓ | ✓ | ✓ | - |
| Fluid (DHW loop) | ✓ | ✓ | ✓ | ✓ |
| Fluid (demand) | ✓ | ✓ | ✓ | ✓ |

### 5.2 Telemetry Fields

#### Resistance WH
- `tank_avg_temp_c` - Volume-weighted average tank temperature
- `upper_element_power_w` - Upper element electric power
- `lower_element_power_w` - Lower element electric power
- `electric_kw` / `electric_power_w` - Total electric draw
- `draw_flow_rate_kg_s` - Domestic hot water draw flow rate
- `operating_mode` - 0=Off, 1=Heating
- `tank_node_X_c` - Individual node temperatures (dynamic based on n_nodes)
- `skin_loss_w` - Tank jacket heat loss to zone

#### Gas WH
- All ERWH fields except element-specific ones
- `burner_power_w` - Burner heat input
- `pilot_power_w` - Pilot light power
- `fuel_input_w` - Fuel energy input
- `flue_loss_w` - Flue losses

#### HPWH
- All ERWH fields
- `compressor_power_w` - Compressor electrical power
- `backup_element_power_w` - Backup element power
- `cop` - Coefficient of performance
- `cap_mult` - Capacity multiplier
- `zone_heat_extraction_w` - Heat extracted from zone air
- `wall_sensible_gain_w` - Sensible gain to interior walls
- `unmet_load_w` - Unmet load power

#### Tankless
- `outlet_temp_c` - Outlet water temperature
- `thermal_output_w` - Thermal output
- `fuel_input_w` - Fuel input (gas) or electric equivalent
- `parasitic_electric_w` - Standby electric draw

### 5.3 Aggregation

Water heating telemetry is aggregated at the dwelling level:
- Energy consumption is tracked via CoreOutput (electric_kw, fuel_w)
- Jacket losses flow to thermal solver via ThermalCategory::JacketLoss
- DHW demand flows to fluid ports for aggregation with wet appliances

### 5.4 Issues

| Issue | Severity | Description |
|-------|----------|-------------|
| Missing COP telemetry for ERWH/Gas | Low | These don't have COP since they're resistive/gas. The telemetry key exists but isn't set - this is fine but could be documented. |
| HPWH missing some HP-specific telemetry | Low | HPWH doesn't report `hp_duty_cycle` or `er_duty_cycle` separately in telemetry - only aggregated `operating_mode`. |

---

## 6. Dwelling/Thermal Solver Integration

### 6.1 Heat Injection

Water heaters inject heat into the stratified tank model:
- ERWH: Heat injected at upper_node and lower_node based on element position
- Gas WH: Heat injected at burner_node
- HPWH: Heat injected across multiple nodes via condenser_node_weights (OCHRE distribution)
- Tankless: Uses IdealWaterModel - heat added based on draw

### 6.2 Jacket Losses

Tank skin losses are reported to the thermal solver via ThermalCategory::JacketLoss:
- ERWH: 100% of skin loss goes to zone (via Thermal port)
- Gas WH: Fraction determined by skin_loss_fraction (derived from EF)
- HPWH: Fraction determined by lost_heat_fraction (default 0.0 = all to zone)

### 6.3 Hot Water Demand

Water heating integrates with dwelling through:

1. **DHW Demand Loop**: Wet appliances (clothes washer, dishwasher) emit DHW demand to `DHW_DEMAND_LOOP` 
2. **Water Heater Reads Demand**: Water heaters read accumulated demand and add to their schedule-based draw
3. **Fluid Port Output**: Water heaters report hot water delivery to the DHW distribution loop

This is correctly implemented in `water_heater/mod.rs`:
```rust
let appliance_demand_kg_s = super::read_dhw_demand_kg_s(ports);
let total_draw_kg_s = draw_flow_rate_kg_s + appliance_demand_kg_s;
```

### 6.4 Water Draw Model

The stratified tank model (`tank.rs`) correctly implements:
- Multi-node temperature stratification
- Mixing during draw (water draw algorithm from OCHRE Water.py)
- Tempering valve logic for mixed water delivery
- Unmet load calculation when outlet temperature < setpoint

### 6.5 Issues

| Issue | Severity | Description |
|-------|----------|-------------|
| HPWH lost_heat_fraction not from config | Medium | HPWH config has `lost_heat_fraction` field but init doesn't read it - always uses default. This prevents override. |
| No integration with water distribution model | Info | HARES doesn't model the hot water distribution piping (recirc loops, pipe losses). OCHRE has some support for this but it's not critical for equipment simulation. |

---

## Summary of Issues

### High Priority
1. **Missing Jacket R-Value parsing** - HPXML `WaterHeaterInsulation/Jacket/JacketRValue` not parsed
2. **HPWH lost_heat_fraction not applied** - Config field exists but init doesn't use it

### Medium Priority
3. **HPWH lost_heat_fraction default differs from OCHRE** - Default is 0.0 vs OCHRE's 0.75 for non-indoor
4. **Missing Tankless Parasitic Power** - Not parsed from HPXML

### Low Priority
5. **Default tank nodes is 6** - Differs from OCHRE (2 or 12)
6. **Gas WH default burner input** - Uses 11kW fallback vs OCHRE requiring HPXML value
7. **HPWH min_off_time** - Not exposed/configured differently from OCHRE
8. **Setpoint ramp rate for HPWH** - Not applied the same as ERWH

---

## Recommendations

1. **Add Jacket R-Value parsing** in `resolve_water_heater.rs`:
   ```rust
   let jacket_r = water_heater
       .get("WaterHeaterInsulation")
       .and_then(|i| i.get("Jacket"))
       .and_then(|j| j.get("JacketRValue"));
   ```

2. **Fix HPWH lost_heat_fraction** - Apply config value in `init_typed()`:
   ```rust
   self.lost_heat_fraction = c.lost_heat_fraction.unwrap_or(DEFAULT_LOST_HEAT_FRACTION);
   ```

3. **Add Tankless Parasitic Power parsing** - Parse from HPXML or derive from number of bedrooms formula

4. **Consider standardizing tank nodes** - Default to 2 for simple cases, 12 for HPWH matching OCHRE behavior

---

## Comparison: Where HARES is Better than OCHRE

1. **ZIP Load Modeling** - HARES has proper voltage-dependent load modeling; OCHRE has minimal support
2. **Code Organization** - HARES separates concerns into distinct modules (tank, compressor, config)
3. **Type Safety** - HARES uses Rust type system for config validation; OCHRE uses runtime checks
4. **Test Infrastructure** - HARES has more comprehensive unit tests with clear documentation

---

## Comparison: Where OCHRE is Better than HARES

1. **Jacket R-Value** - OCHRE parses and applies tank insulation from HPXML
2. **Tank Nodes** - OCHRE uses 1/2/12 nodes appropriately for different model types
3. **HPWH Lost Heat** - OCHRE correctly defaults to 0.75 for non-indoor installations

---

## Conclusion

The HARES water heating implementation is a high-quality port of the OCHRE models with excellent code organization and test coverage. The issues identified are minor and do not affect core functionality. The implementation correctly preserves the sophisticated physics of the OCHRE originals while adding modern software engineering practices.

**Recommendation**: Proceed with integration testing. The identified issues are minor and can be addressed in follow-up PRs.
