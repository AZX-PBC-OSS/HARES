# HVAC Cooling Equipment Review - HARES

**Reviewer**: Code Review Agent  
**Date**: 2025-03-31  
**Scope**: Air Conditioners, Heat Pump Cooling (ASHP Cooler, MSHP Cooler, Room AC)  
**Files Reviewed**:
- `vendors/OCHRE/ochre/Equipment/HVAC.py` - OCHRE HVAC implementation
- `vendors/OCHRE/ochre/utils/hpxml.py` - HPXML parsing in OCHRE
- `crates/hares-io/src/hpxml/resolve_hvac.rs` - HPXML resolution in HARES
- `crates/hares-equipment/src/hvac/air_conditioner.rs` - HARES Air Conditioner
- `crates/hares-equipment/src/hvac/cooling_config.rs` - Cooling config structs
- `crates/hares-equipment/src/hvac/heat_pump_config.rs` - Heat pump cooler config
- `crates/hares-equipment/src/hvac/hvac_core.rs` - Core HVAC wrapper

---

## 1. HPXML Parsing

### 1.1 Parsed Attributes

**Central Air Conditioner** (`CoolingSystem`):
| HPXML Attribute | HARES Field | OCHRE Field |
|-----------------|-------------|-------------|
| `CoolingCapacity` | `cooling_capacity_w` | `Capacity (W)` |
| `SEER` (or `AnnualCoolingEfficiency` with units=SEER) | `efficiency_seer` -> `eir` | `AnnualCoolingEfficiency` -> COP |
| `CompressorType` | `number_of_speeds` | `Number of Speeds (-)` |
| `FractionCoolingLoadServed` | `fraction_load_served` | `Conditioned Space Fraction (-)` |
| `CoolingSensibleHeatFraction` | `shr` | `SHR (-)` |
| `FanPowerWatts` or `FanPowerWattsPerCFM` | `fan_power_w`, `fan_power_w_per_cfm` | `Rated Auxiliary Power (W)` |

**Room AC** (`CoolingSystemType=room air conditioner`):
- Same as Central AC except uses EER instead of SEER (correctly handled)
- No duct parameters (window units have no ducts)

**Heat Pump Cooling** (`HeatPump` with `HeatPumpType`=air-to-air or mini-split):
| HPXML Attribute | HARES Field | Notes |
|-----------------|-------------|-------|
| `CoolingCapacity` | `cooling_capacity_w` | |
| `SEER` | `efficiency_seer` -> `cooling_eir` | |
| `HeatingCapacity`, `HSPF` | Also parsed for heating side | |
| `CompressorLockoutTemperature` | Not passed to cooler | Only used for heating |

### 1.2 Missing Mappings / Issues

**Issue 1: Missing SEER/EER Default When Not in HPXML** (Severity: Medium)
- **Location**: `crates/hares-io/src/hpxml/resolve_hvac.rs`
- **Problem**: If HPXML does not provide SEER (for AC/ASHP) or EER (for Room AC), HARES returns `None` from `seer_from_params()`/`eer_from_params()`, causing the typed config builder to return `None`. This will cause equipment creation to fail.
- **OCHRE Behavior**: OCHRE has hardcoded default efficiencies (e.g., SEER 13 for AC) when not specified
- **Recommendation**: Add default SEER/EER fallback (e.g., SEER 13 for central AC, EER 10 for Room AC) when not specified in HPXML

**Issue 2: CoolingSensibleHeatFraction Not Always Passed** (Severity: Low)
- **Location**: HPXML parsing
- **Problem**: `CoolingSensibleHeatFraction` is optional in HPXML. HARES correctly defaults to 0.75 when not specified, but OCHRE also defaults to 0.75 - this is fine.

---

## 2. OCHRE Defaults/Fallbacks

### 2.1 Number of Speeds Fallback

OCHRE determines number of speeds as follows (`hpxml.py` lines 861-876):
```python
speed_options = {"single stage": 1, "two stage": 2, "variable speed": 4}
if name == "mini-split": number_of_speeds = 4
elif hvac.get("CompressorType") in speed_options: number_of_speeds = speed_options[...]
elif convert(cop, "W", "Btu/hour") <= 15: number_of_speeds = 1   # SEER <= 15
elif convert(cop, "W", "Btu/hour") <= 21: number_of_speeds = 2   # 15 < SEER <= 21  
else: number_of_speeds = 4  # SEER > 21
```

HARES uses the same logic in `resolve_hvac.rs` (tests at lines 367-422 verify this).

### 2.2 Startup Capacity Degradation (c_d) Default

OCHRE (`utils/equipment.py` lines 470-500):
```python
def calc_c_d(is_heater, name, cop, number_of_speeds):
    if is_heater:  # cooling equipment
        seer = convert(cop, "W", "Btu/hour")
        if name == "Room AC": c_d = 0.22
        elif number_of_speeds == 1:
            if seer < 13.0: c_d = 0.2
            else: c_d = 0.07
        elif number_of_speeds == 2: c_d = 0.11
        else: c_d = 0.0  # variable speed
    return c_d
```

HARES (`cooling_config.rs` lines 124-132):
```rust
pub fn derived_cooling_startup_cd(&self) -> Option<f64> {
    self.startup_cd.or(match self.cooling_speed_control_mode() {
        SpeedControlMode::VariableSpeedIdeal => Some(0.0),
        SpeedControlMode::TwoSpeedSetpoint | TwoSpeedTime => Some(0.11),
        SpeedControlMode::SingleSpeed | MultiSpeedInterpolated => None,  // No default!
    })
}
```

**Issue 3: Single-Speed AC Has No Default c_d in HARES** (Severity: Low)
- **Problem**: HARES doesn't set a default c_d for single-speed equipment, while OCHRE sets:
  - Room AC: c_d = 0.22
  - Single-speed AC with SEER < 13: c_d = 0.2
  - Single-speed AC with SEER >= 13: c_d = 0.07
- **Impact**: Equipment without explicit `startup_cd` in config won't apply cycling degradation
- **Recommendation**: Add default c_d logic based on equipment type and SEER

### 2.3 Other Defaults

| Parameter | OCHRE Default | HARES Default | Notes |
|-----------|---------------|---------------|-------|
| SHR | 1.0 (if not specified) | 0.75 | HARES is more realistic |
| Airflow (Central AC) | 312 cfm/ton | 400 cfm/ton * defect_multiplier | HARES uses higher airflow |
| Airflow (Room AC) | 312 cfm/ton | 320 cfm/ton | Similar |
| Airflow (MSHP) | 312 cfm/ton | 312 cfm/ton | Same |
| Min on time | 0 (no default) | 0 | Both allow configuration |
| Min off time | 0 (no default) | 0 | Both allow configuration |
| Deadband | 1.0°C | Must be configured | Should come from HPXML setpoints |

---

## 3. HARES Wiring

### 3.1 Configuration Flow

```
HPXML -> resolve_hvac.rs -> try_build_central_ac_config() / try_build_room_ac_config()
     -> CentralAirConditionerConfig / RoomAcConfig -> EquipmentConfig::from_typed()
     -> AirConditioner::init() -> HvacEquipment init
```

### 3.2 Correctly Wired Items

1. **Stage capacities and EIRs**: HARES correctly extracts multi-stage parameters:
   - `stage_capacities_w` from `cooling_capacity_w_stage_X`
   - `stage_eirs` from `cooling_eir_stage_X`
   - `stage_shrs` from `shr_stage_X`

2. **Duct DSE**: Correctly computes ASHRAE 152 DSE for central AC/ASHP (not for Room AC or MSHP - correct)

3. **Biquadratic curves**: Correctly loads from config or uses defaults:
   - Default AC curves: `[1.5509, -0.07505, 0.0031, 0.0024, -0.00005, -0.00043]` (capacity), `[-0.30428, 0.11805, -0.00342, -0.00626, 0.0007, -0.00047]` (EIR)

4. **Typed config fields**: All key fields correctly passed:
   - `capacity_w`, `eir`, `number_of_speeds`, `shr`, `fan_power_w`, `airflow_m3_s_per_w`, `duct`, `cooling_setpoint_source`

### 3.3 Missing/Incorrect Mappings

**Issue 4: Crankcase Heater Not Fully Configured** (Severity: Low)
- **Location**: `air_conditioner.rs` lines 32-33, 613-633
- **Problem**: HARES has hardcoded defaults (50W, 12.8°C threshold) but these are not populated into the typed config by HPXML parsing. The HPXML could specify `extension/CrankcaseHeater` but HARES doesn't parse it.
- **OCHRE Behavior**: OCHRE parses this from HPXML extension
- **Current State**: Works correctly with defaults, but HPXML extension not read

---

## 4. Control Logic

### 4.1 Thermostat Control

HARES implements a sophisticated thermostat Finite State Machine:

```rust
// From air_conditioner.rs update_control()
let mode = self.hvac.update_mode(env).unwrap_or(ThermostatMode::Deadband);
if mode == ThermostatMode::Cooling {
    let setpoint = self.hvac.effective_setpoints().cooling_c + dr_setpoint_offset_c;
    let load_fraction = if single_speed { 1.0 } else { (zone_temp - setpoint) / deadband };
    self.hvac.duty_cycle = match speed_control_mode {
        VariableSpeedIdeal => select_variable_speed_cooling(load_fraction).part_load_ratio,
        use_ideal => 1.0,
        _ => select_speed_with_zone_temp(load_fraction, ...).part_load_ratio,
    };
}
```

### 4.2 Speed Control Modes

| Mode | HARES Implementation | OCHRE Implementation |
|------|---------------------|---------------------|
| Single Speed | On/Off with duty cycle | On/Off with duty cycle |
| Two-Speed Setpoint | Uses setpoint-based stage selection | Uses setpoint-based control |
| Two-Speed Time | Not implemented in HARES | Time-based control |
| Variable Speed (Ideal) | Continuous modulation with ideal capacity | Ideal capacity algorithm |

**Issue 5: Two-Speed Time Control Not Implemented** (Severity: Low)
- **Location**: `speed_control.rs`
- **Problem**: OCHRE has "Time" and "Time2" control types for two-speed equipment that uses temperature change direction to determine speed. HARES only implements "Setpoint" mode.
- **Impact**: Some HPXML files may specify "Time" control which won't be honored

### 4.3 External Control Signals

HARES correctly implements:
- `Setpoint`: Override cooling setpoint
- `Deadband`: Override thermostat deadband
- `Duty Cycle`: Control runtime fraction
- `Load Fraction`: Force equipment off (0) or full (1)
- `Max Capacity Fraction`: Limit maximum capacity (ideal mode)
- `Mode Override`: Force specific operating mode
- `Demand Response`: Setpoint offset and load limiting

---

## 5. Output Ports & Telemetry

### 5.1 Port Declarations

```rust
ports: vec![
    PortDeclaration::electrical(),  // Electric power
    PortDeclaration::thermal(zone), // Thermal gains to zone
]
```

### 5.2 Telemetry Fields

| Telemetry Key | Unit | Description | OCHRE Equivalent |
|---------------|------|-------------|------------------|
| `electric_kw` | kW | Total: compressor + fan + crankcase | `HVAC Cooling Electric Power (kW)` |
| `sensible_cooling_w` | W | Delivered sensible cooling | `HVAC Cooling Delivered (W)` |
| `latent_cooling_w` | W | Delivered latent cooling | (implicit via SHR) |
| `shr` | - | Sensible heat ratio | `HVAC Cooling SHR (-)` |
| `operating_mode` | enum | 0=Off, 2=Cooling | `HVAC Cooling Mode` |
| `cop` | - | Coefficient of performance (AHRI, excludes fan) | `HVAC Cooling COP (-)` |
| `runtime_fraction` | - | PLR/PLF [0..1] | (calculated) |
| `compressor_kw` | kW | Compressor-only power | (subset of total) |
| `fan_kw` | kW | Supply fan power | `HVAC Cooling Fan Power (kW)` |
| `supply_temp_c` | °C | Supply air temp leaving coil | (not in OCHRE) |
| `apparatus_dew_point_c` | °C | ADP at coil | (not in OCHRE) |
| `bypass_factor` | - | Coil bypass factor | (not in OCHRE) |

### 5.3 Aggregation

The `EndUse::HVACCooling` aggregate correctly sums all cooling equipment:
- ASHP Cooler
- MSHP Cooler  
- Room AC
- Generic Cooler (Ideal HVAC cooling)

---

## 6. Dwelling/Thermal Solver Integration

### 6.1 Thermal Port Contributions

HARES correctly adds cooling thermal gains to zones:

```rust
// From air_conditioner.rs step()
let effective_cooling = sensible_cooling_w + latent_cooling_w;
ports.thermal[0].sensible_gain_w = -effective_cooling * sensible_fraction;  // Negative = cooling
ports.thermal[0].latent_gain_w = -effective_cooling * (1.0 - sensible_fraction);
```

### 6.2 Zone Heat Fractions

HARES correctly calculates zone heat fractions (from `hvac_core.rs`):

```rust
pub fn update_zone_heat_fractions(&mut self) {
    // DSE * (1 - basement_frac) to conditioned zone
    // DSE * basement_frac to basement zone  
    // (1 - DSE) to duct zone
}
```

### 6.3 Ideal Capacity Integration

HARES supports ideal capacity mode (time resolution >= 5 min or variable-speed):
- `ideal_capacity_w` set via `IdealCapacity` control signal
- Solver provides capacity to maintain setpoint exactly
- Equipment calculates part-load ratio for variable-speed operation

**Comparison**: HARES physics is equivalent to OCHRE for thermal integration. Both use zone heat fractions and DSE for distributing cooling to zones.

---

## Summary of Issues

| Issue | Severity | Description |
|-------|----------|-------------|
| 1 | Medium | Missing SEER/EER default fallback when not in HPXML |
| 2 | Low | Single-speed AC has no default c_d (startup degradation) |
| 3 | Low | Crankcase heater not parsed from HPXML extension |
| 4 | Low | Two-speed "Time" control type not implemented |

### Comparison: Where HARES is Better

1. **More realistic SHR default**: HARES defaults to 0.75 vs OCHRE's 1.0
2. **Better telemetry**: HARES reports ADP and bypass factor (not in OCHRE)
3. **Stronger validation**: Typed config validation catches invalid values
4. **Clearer separation**: HARES has better separation between AC and heat pump cooler configs

### Comparison: Where OCHRE is Better

1. **Default c_d for single-speed**: OCHRE calculates from SEER, HARES requires explicit config
2. **More control types**: OCHRE supports Time/Time2 control for two-speed
3. **Default efficiencies**: OCHRE has fallback when HPXML doesn't specify SEER

---

## Recommendations

1. **Add SEER/EER defaults**: Add fallback defaults (SEER 13 for AC, EER 10 for Room AC) in `resolve_hvac.rs` when not in HPXML
2. **Add c_d defaults**: Implement `calc_c_d()` equivalent logic based on equipment type and SEER
3. **Consider Time-based two-speed control**: Implement for compatibility with HPXML control types
4. **Parse crankcase heater from HPXML**: Read `extension/CrankcaseHeater` from HPXML if present

---

*End of Review*
