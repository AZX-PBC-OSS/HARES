# Variable-Speed (Modulating) Gas Furnace Review - HARES

**Review Date**: March 31, 2026  
**Reviewer**: Code Reviewer  
**Scope**: Variable-Speed (Modulating) Gas Furnace configuration in HARES

---

## Executive Summary

This review analyzes the Variable-Speed (Modulating) Gas Furnace implementation in HARES, comparing against OCHRE's implementation and assessing HPXML parsing, default/fallback values, wiring, control logic, telemetry, and thermal solver integration.

**Overall Assessment**: **CRITICAL GAP IDENTIFIED** - HARES does NOT actually implement variable-speed (modulating) control for gas furnaces. The `GasFurnaceConfig` has a `number_of_speeds` field that defaults to 1, and the implementation only uses a single capacity stage with simple on/off duty cycling. Variable-speed capability exists in the `HvacEquipment` core infrastructure but is only wired to Air Conditioners and Heat Pumps, not furnaces.

This represents a significant deviation from OCHRE physics - OCHRE does support multi-speed furnaces via the DynamicHVAC class, but HARES has not connected this capability to the GasFurnace equipment type.

---

## 1. HPXML Parsing

### Files Analyzed
- `/home/rich/src/HARES/crates/hares-io/src/hpxml/resolve_hvac.rs` - Lines 401-406, 482-523
- `/home/rich/src/HARES/vendors/OCHRE/ochre/utils/hpxml.py` - Lines 861-876

### Attributes Parsed for Variable-Speed Gas Furnace

| HPXML Attribute | HARES Parameter | Parsing Function | Status |
|-----------------|-----------------|------------------|--------|
| `HeatingCapacity` | `heating_capacity_w` | `insert_capacity_w()` | Parsed |
| `AnnualHeatingEfficiency/Units=AFUE` | `afue` | `afue_from_params()` | Parsed |
| `extension/FanPowerWattsPerCFM` | `fan_power_w` | Extension parsing | Parsed |
| `extension/HeatingAirflowCFM` | `heating_airflow_cfm` | Extension parsing | Parsed |
| `number_of_speeds` | `number_of_speeds` | `n_speeds_from_params()` | **Parsed but not used** |
| Duct parameters | Multiple | ASHRAE 152 computation | Parsed |

### Key Finding: HPXML Does Not Define Variable-Speed Furnace

HPXML schema does not have a direct "Stage" or "Speed" attribute for furnaces like it does for compressors (cooling). The OCHRE HPXML parser determines number_of_speeds primarily from `CompressorType`, which applies to cooling/heat pumps, not heating furnaces:

```python
# OCHRE hpxml.py:861-876
speed_options = {
    "single stage": 1,
    "two stage": 2,
    "variable speed": 4,
}
# ...
elif hvac.get("CompressorType") in speed_options:
    number_of_speeds = speed_options[hvac.get("CompressorType")]
```

For heating systems, OCHRE defaults to single-speed unless explicitly configured otherwise.

### What is Parsed but Not Wired

HARES parses `number_of_speeds` from params but **does not use it for furnace control**:

```rust
// resolve_hvac.rs:492
let n_speeds = n_speeds_from_params(params);  // Parsed

// heating_config.rs:22
pub number_of_speeds: u8,  // Stored in config

// furnace.rs:330
self.hvac.heating_capacities_w = vec![self.rated_capacity_w];  // Always single element!
```

---

## 2. OCHRE Defaults

### Files Analyzed
- `/home/rich/src/HARES/vendors/OCHRE/ochre/Equipment/HVAC.py` - Lines 682-689, 735-831
- `/home/rich/src/HARES/defaults/HVAC Multispeed Parameters.csv`
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/heating_config.rs` - Lines 10-45

### OCHRE Multi-Speed Support

OCHRE has a `DynamicHVAC` class that supports multi-speed equipment including gas furnaces:

```python
# OCHRE HVAC.py:749-831
class DynamicHVAC(HVAC):
    """
    HVAC Equipment Class using dynamic capacity algorithm...
    - Variable: Variable speed equipment. Uses the ideal algorithm to determine
      capacity, but the dynamic algorithm for EIR.
    """
```

However, the **HVAC Multispeed Parameters.csv has NO entries for Gas Furnace**:

| HVAC Name | Has Multi-Speed Data |
|-----------|---------------------|
| ASHP Cooler | Yes (1, 2, 4 speeds) |
| ASHP Heater | Yes (1, 2, 4 speeds) |
| MSHP Cooler | Yes (4 speeds) |
| MSHP Heater | Yes (4 speeds) |
| **Gas Furnace** | **No entries** |

### Default Values Comparison

| Parameter | HARES Default | OCHRE Behavior | Assessment |
|-----------|---------------|----------------|------------|
| `afue` | 0.80 | 0.80 | Matches |
| `number_of_speeds` | 1 | 1 (default) | Matches on surface |
| Fan power curve | Not implemented | Per-speed fan curves available | **Missing in HARES** |
| Part-load efficiency | Constant AFUE | EIR varies with PLR | **Missing in HARES** |

### Critical: No Part-Load Efficiency Curve

OCHRE DynamicHVAC uses biquadratic curves to compute efficiency at part load:

```python
# OCHRE HVAC.py:721-732
def update_eir(self):
    # update EIR based on part load ratio, input/output temperatures
    plr = self.speed_idx  # part-load-ratio
    # Uses efficiency_coeff (biquadratic) to compute actual EIR
```

HARES GasFurnace uses constant AFUE at all load levels:

```rust
// furnace.rs:356-360
let fuel_input_w = if gross_capacity_w > 0.0 {
    gross_capacity_w / self.fuel_efficiency * sf  // Constant efficiency!
} else {
    0.0
};
```

---

## 3. HARES Wiring

### Files Analyzed
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/furnace.rs` - Lines 44-56, 262-476
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/heating_config.rs` - Lines 10-45
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/core_config.rs` - Lines 145-175

### Config Structure

```rust
// heating_config.rs:10-25
pub struct GasFurnaceConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    pub capacity_w: f64,           // Required
    pub afue: f64,                 // Required
    pub fan_power_w: Option<f64>,
    pub number_of_speeds: u8,      // Parsed but NOT USED
    pub ducts: DuctConfig,
}
```

### The Gap: Config Not Wired to Equipment

The `GasFurnace::init()` method never uses `number_of_speeds` to configure multi-stage capacity:

```rust
// furnace.rs:312-336
fn init(&mut self, config: &EquipmentConfig, env: &EnvironmentState) -> crate::Result<()> {
    self.hvac.init(config, env)?;
    let typed = config.require_typed::<GasFurnaceConfig>("Gas Furnace")?;
    self.rated_capacity_w = typed.capacity_w.max(0.0);
    self.fuel_efficiency = typed.afue;
    // ...
    self.hvac.heating_capacities_w = vec![self.rated_capacity_w];  // Always single stage!
    self.hvac.eir_by_stage = vec![1.0 / self.fuel_efficiency];
    // ...
}
```

Compare to HeatPumpHeater which DOES use multi-speed:

```rust
// heat_pump/heater.rs:425
self.hvac.heating_capacities_w = if let Some(stages) = &cfg.stage_heating_capacities_w {
    stages.clone()
} else {
    vec![self.rated_heating_capacity_w]
};
```

### Speed Control Mode

The `HvacEquipment` has a `speed_control_mode` field that is set in `core_config.rs`:

```rust
// core_config.rs:145-175
pub(super) fn parse_speed_control_mode(config: &EquipmentConfig) -> SpeedControlMode {
    // ... parsing logic for "variable", "two", "four", etc.
    // But this is NEVER called for GasFurnace
}
```

HARES correctly parses speed control mode, but **this logic is never connected to GasFurnace**. The furnace always uses the default `SpeedControlMode::SingleSpeed`.

---

## 4. Control Logic

### Files Analyzed
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/furnace.rs` - Lines 339-422
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/helpers.rs` - Lines 125-188
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/air_conditioner.rs` - Lines 333-431, 680-713

### Current Implementation: Simple On/Off

The GasFurnace uses simple on/off control:

```rust
// helpers.rs:130-145
pub fn update_heating_control(hvac: &mut HvacEquipment, env: &EnvironmentState) -> OperatingMode {
    match hvac.update_mode(env) {
        Ok(ThermostatMode::Heating) => {
            if !hvac.use_ideal_capacity(env) {
                hvac.duty_cycle = 1.0;  // Always full on for single-stage
            } else {
                hvac.duty_cycle = hvac.duty_cycle.clamp(0.0, 1.0);
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

### What Exists But Is Not Used

HARES already has `VariableSpeedIdeal` control logic that works for AC:

```rust
// air_conditioner.rs:690-694
self.hvac.duty_cycle = match self.hvac.speed_control_mode {
    SpeedControlMode::VariableSpeedIdeal => {
        self.select_variable_speed_cooling(load_fraction).part_load_ratio
    }
    // ...
};
```

This infrastructure could be adapted for furnaces but currently is not.

### Control Characteristics

| Aspect | HARES Current | OCHRE Variable-Speed | Assessment |
|--------|---------------|---------------------|------------|
| Capacity modulation | None | Yes (continuous) | **Missing** |
| Fan speed modulation | None | Yes (variable speed) | **Missing** |
| Part-load efficiency curve | Constant | Biquadratic curve | **Missing** |
| On/off cycling | duty_cycle = 1.0 | Can modulate | **Missing** |

---

## 5. Output Ports & Telemetry

### Files Analyzed
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/furnace.rs` - Lines 283-300, 501-512, 556-591

### Current Telemetry Fields

```rust
fn gas_furnace_telemetry_fields() -> Vec<TelemetryField> {
    vec![
        TelemetryField { name: "fan_kw", ... },
        TelemetryField { name: "electric_kw", ... },
        TelemetryField { name: "fuel_input_w", ... },
        TelemetryField { name: "thermal_output_w", ... },
        TelemetryField { name: "operating_mode", ... },
        TelemetryField { name: "supply_air_temp_c", ... },
        TelemetryField { name: "heating_setpoint_c", ... },
        TelemetryField { name: "cooling_setpoint_c", ... },
    ]
}
```

### Missing Telemetry for Variable-Speed

| Telemetry Key | Description | Status |
|---------------|-------------|--------|
| `speed_index` | Current speed stage | **Not reported for furnace** |
| `speed_frac` | Interpolation fraction | **Not reported** |
| `modulation_level` | Current capacity % | **Not reported** |
| `part_load_efficiency` | Efficiency at current PLR | **Not reported** |

Compare to HeatPumpHeater which DOES report speed:

```rust
// heat_pump/heater.rs:674
self.telemetry.set(tk::SPEED_INDEX, self.hvac.last_speed_index as f64);
```

---

## 6. Dwelling Integration

### Files Analyzed
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/furnace.rs` - Lines 344-422
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/hvac_core.rs` - Lines 1640-1700

### Current Integration

The furnace integrates with the thermal solver through zone thermal ports:

```rust
// furnace.rs:350-360
let duty = self.hvac.duty_cycle.clamp(0.0, 1.0);
let sf = self.hvac.space_fraction;
let gross_capacity_w = self.rated_capacity_w * duty;
```

### What Would Be Needed for Variable-Speed

To properly support variable-speed, HARES would need to:

1. **Multiple capacity stages** - `heating_capacities_w = vec![low, mid, high]`
2. **Speed selection logic** - Similar to `select_variable_speed_cooling()` but for heating
3. **Part-load efficiency** - Apply EIR curves based on PLR
4. **Variable fan power** - Fan power should scale with speed

---

## Issues and Findings

### Issue 1 (Critical): Variable-Speed Not Implemented for Gas Furnaces

**Files**:
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/furnace.rs:330-331`
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/heating_config.rs:22`

**Evidence**:

The `GasFurnaceConfig` has `number_of_speeds: u8` field that is parsed from HPXML but never used:

```rust
// furnace.rs:330-331 - Always single stage
self.hvac.heating_capacities_w = vec![self.rated_capacity_w];
self.hvac.eir_by_stage = vec![1.0 / self.fuel_efficiency];
```

Compare to AC which properly uses multi-speed:

```rust
// hvac_core.rs:1193 (test setup)
hvac.heating_capacities_w = vec![4000.0, 6000.0, 8000.0, 10000.0];
```

**Impact**: Gas furnaces always run at full capacity with on/off cycling. Variable-speed (modulating) furnaces cannot be accurately modeled. This is a significant gap for modern high-efficiency modulating furnaces.

**Recommended Fix**: Implement multi-stage capacity for GasFurnace:
1. Parse `stage_heating_capacities_w` from HPXML or generate from ratios
2. Wire `speed_control_mode` to furnace equipment
3. Implement speed selection based on load
4. Add part-load efficiency curves (biquadratic)

---

### Issue 2 (High): No Part-Load Efficiency Curve

**Files**:
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/furnace.rs:356-360`

**Evidence**:

OCHRE computes efficiency at part load:

```python
# OCHRE HVAC.py - GasBoiler example
def update_eir(self):
    plr = self.speed_idx
    eff_curve_output = c[0] + c[1] * plr + c[2] * plr**2 + ...
    return self.eir_max / eff_curve_output
```

HARES uses constant AFUE regardless of load:

```rust
// furnace.rs:356-360
let fuel_input_w = if gross_capacity_w > 0.0 {
    gross_capacity_w / self.fuel_efficiency * sf  // Constant!
};
```

**Impact**: Modern modulating furnaces are more efficient at lower capacities (due to longer runtimes and reduced cycling losses). HARES overestimates fuel consumption at low loads.

**Recommended Fix**: Add optional EIR PLR coefficients to `GasFurnaceConfig` and apply them in the step function.

---

### Issue 3 (Medium): No Fan Speed Modulation

**Files**:
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/furnace.rs:318-320, 355

**Evidence**:

Fan power is constant, scaled only by duty cycle:

```rust
// furnace.rs:355
let fan_kw = (self.fan_power_w * duty) / 1_000.0 * sf;
```

Variable-speed furnaces reduce fan speed at lower heating capacities to save electricity.

**Impact**: Overestimated auxiliary electric consumption.

**Recommended Fix**: Add per-speed fan power ratios and scale fan power with selected speed stage.

---

### Issue 4 (Low): Telemetry Missing Speed Information

**Files**:
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/furnace.rs:556-591`

**Evidence**:

GasFurnace doesn't report `speed_index` or modulation level. Heat pumps do.

**Impact**: Limited observability into furnace operation.

**Recommended Fix**: Add speed telemetry when multi-speed is implemented.

---

## Summary

| Aspect | HARES vs OCHRE | Assessment |
|--------|----------------|------------|
| HPXML parsing | Parses number_of_speeds but doesn't use | Incomplete |
| Default values | Matches on defaults but no multi-speed data | Incomplete |
| Control logic | **No variable-speed control** | **CRITICAL GAP** |
| Telemetry | No speed/modulation reporting | Missing |
| Thermal solver integration | Works but no modulation | Incomplete |
| Part-load efficiency | Constant efficiency | **Missing** |

---

## Recommendations

1. **High Priority**: Implement variable-speed control for GasFurnace
   - Wire `number_of_speeds` to equipment initialization
   - Add multi-stage capacity vectors
   - Implement speed selection logic

2. **High Priority**: Add part-load efficiency curves
   - Add optional `eir_plr_coefficients` to config
   - Apply in step function based on load

3. **Medium Priority**: Add fan speed modulation
   - Add per-speed fan power ratios
   - Scale fan power with speed selection

4. **Low Priority**: Add speed telemetry
   - Report `speed_index` for furnaces
   - Report `modulation_level` or `part_load_ratio`
