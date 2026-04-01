# Single-Stage Gas Furnace Review - HARES

**Review Date**: March 31, 2026  
**Reviewer**: Code Reviewer  
**Scope**: Single-Stage Gas Furnace configuration in HARES

---

## Executive Summary

This review analyzes the Single-Stage Gas Furnace implementation in HARES, comparing against OCHRE's implementation and assessing HPXML parsing, default/fallback values, wiring, control logic, telemetry, and thermal solver integration.

**Overall Assessment**: HARES provides a well-structured gas furnace implementation with strong HPXML support, correct thermal modeling with ASHRAE 152 DSE calculations, and comprehensive telemetry. The implementation preserves OCHRE physics and in some areas improves upon it (typed config architecture). One significant bug was identified where HARES incorrectly applies startup capacity degradation to gas furnaces (OCHRE only applies this to heat pumps).

---

## 1. HPXML Parsing

### Files Analyzed
- `/home/rich/src/HARES/crates/hares-io/src/hpxml/resolve_hvac.rs` - Lines 482-523
- `/home/rich/src/HARES/crates/hares-io/src/hpxml/resolve_hvac.rs` - Lines 1191-1271

### Attributes Parsed for Single-Stage Gas Furnace

| HPXML Attribute | HARES Parameter | Parsing Function | Status |
|-----------------|-----------------|------------------|--------|
| `HeatingCapacity` | `heating_capacity_w` | `insert_capacity_w()` | Parsed |
| `AnnualHeatingEfficiency/Units=AFUE` | `afue` | `afue_from_params()` | Parsed |
| `AnnualHeatingEfficiency` (value) | `efficiency_afue` | `afue_from_params()` | Parsed |
| `extension/FanPowerWattsPerCFM` | `fan_power_w_per_cfm` | Extension parsing | Parsed |
| `extension/FanPowerWatts` | `fan_power_w` | Extension parsing | Parsed |
| `extension/HeatingAirflowCFM` | `heating_airflow_cfm` | Extension parsing | Parsed |
| `extension/AirflowDefectRatio` | `airflow_defect_ratio` | Extension parsing | Parsed |
| `FractionHeatLoadServed` | `fraction_load_served` | Direct parsing | Parsed |
| `ElectricAuxiliaryEnergy` | `auxiliary_power_w` | kWh/yr → W conversion | Parsed |
| Duct location/insulation/leakage | Multiple `duct_*` params | ASHRAE 152 computation | Parsed |
| Setpoint schedules (weekday/weekend) | heating_setpoint_source | `apply_building_setpoint_profiles()` | Parsed |

### What is Missing

**Critical**: None identified.

**Minor Gaps**:
1. **Pilot light** - Not parsed (OCHRE also skips this - commented out in OCHRE `parse_hvac()`). Parity with OCHRE.

### HPXML to Typed Config Wiring

```rust
// resolve_hvac.rs:485-523
fn try_build_gas_furnace_config(
    name: &str,
    params: &Map<String, Value>,
    duct_params: &DuctDseParams,
) -> Option<EquipmentConfig> {
    let afue = afue_from_params(params).unwrap_or(0.80);  // Default matches OCHRE
    let capacity_w = params.get("heating_capacity_w").and_then(Value::as_f64)?;  // Required
    let n_speeds = n_speeds_from_params(params);
    let fan_power_w = fan_power_from_params(params);
    // ... airflow and duct config computation
}
```

The resolver correctly extracts all required fields and builds a typed `GasFurnaceConfig`.

---

## 2. OCHRE Defaults

### Files Analyzed
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/heating_config.rs` - Lines 10-45
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/furnace.rs` - Lines 28-29

### Default Values Comparison

| Parameter | HARES Default | OCHRE Default | Source | Assessment |
|-----------|---------------|---------------|--------|------------|
| `afue` | 0.80 | 0.80 | DOE 10 CFR Part 430 federal minimum | **Matches OCHRE** |
| `capacity_w` | 0.0 | N/A (required in OCHRE) | N/A - HARES requires input | Correct |
| `number_of_speeds` | 1 | 1 | Gas furnaces always single-speed | **Matches OCHRE** |
| `fan_power_w` | Computed from airflow | Computed from airflow | ACCA Manual D default | **Matches OCHRE** |
| Airflow per capacity | 350 CFM/ton | 350 CFM/ton | ASHRAE 62.2 | **Matches OCHRE** |

### On/Off Cycling Behavior

OCHRE's `GasFurnace` class uses simple on/off cycling:
- `duty_cycle` is set to 1.0 when in Heating mode
- No modulation or multi-stage behavior for single-stage furnaces
- Fan runs continuously when furnace is on

HARES implements identical behavior in `furnace.rs:350-360`:
```rust
let duty = self.hvac.duty_cycle.clamp(0.0, 1.0);
// ...
let gross_capacity_w = self.rated_capacity_w * duty;
// When duty=1.0, full capacity is delivered
```

### Fuel Consumption Calculation

**OCHRE** (HVAC.py):
```python
# fuel_input = capacity / AFUE
fuel_input = self.capacity * self.duty_cycle / self.afue
```

**HARES** (furnace.rs:356-360):
```rust
let fuel_input_w = if gross_capacity_w > 0.0 {
    gross_capacity_w / self.fuel_efficiency * sf
} else {
    0.0
};
```

**Physics Preserved**: HARES correctly preserves OCHRE's first-law energy balance:
- Fuel consumption is independent of duct DSE (furnace burns fuel based on gross output)
- Only the thermal delivery to the zone is reduced by DSE
- Fan waste heat is added to zone sensible gain

---

## 3. HARES Wiring

### Files Analyzed
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/furnace.rs` - Lines 262-476
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/heating_config.rs` - Lines 10-45

### Config Structure

```rust
// heating_config.rs
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GasFurnaceConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    pub capacity_w: f64,        // Required
    pub afue: f64,              // Required
    pub fan_power_w: Option<f64>,
    pub number_of_speeds: u8,   // Defaults to 1
    pub ducts: DuctConfig,      // Contains DSE and duct zone config
}
```

### Equipment Initialization (furnace.rs:312-336)

```rust
fn init(&mut self, config: &EquipmentConfig, env: &EnvironmentState) -> crate::Result<()> {
    self.hvac.init(config, env)?;
    let typed = config.require_typed::<GasFurnaceConfig>("Gas Furnace")?;
    self.rated_capacity_w = typed.capacity_w.max(0.0);
    self.fuel_efficiency = typed.afue;
    // ... fan power computation
    // ... heating capacities and EIR by stage
}
```

### Wiring Assessment

**Strengths**:
- Typed config ensures all required fields are present
- Proper validation of efficiency values
- Correctly sets up single-stage (one element in `heating_capacities_w`)

**Issues**:
- None identified

---

## 4. Control Logic

### Files Analyzed
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/helpers.rs` - Lines 123-145
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/hvac_core.rs` - Lines 587-640
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/thermostat.rs` - Lines 90-97

### Single-Stage Control Implementation

The control logic uses a thermostat FSM with asymmetric deadband:

```rust
// helpers.rs:130-145
pub fn update_heating_control(hvac: &mut HvacEquipment, env: &EnvironmentState) -> OperatingMode {
    match hvac.update_mode(env) {
        Ok(ThermostatMode::Heating) => {
            if !hvac.use_ideal_capacity(env) {
                hvac.duty_cycle = 1.0;  // Simple on/off for single-stage
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

### Control Characteristics

| Aspect | HARES | OCHRE | Assessment |
|--------|-------|-------|------------|
| Modulation | None (single-stage) | None | **Matches OCHRE** |
| On/off cycling | duty_cycle = 1.0 when Heating | Full on/off | **Matches OCHRE** |
| Deadband type | Asymmetric (offset=0.2) | Asymmetric | **Matches OCHRE** |
| Minimum cycle time | Configurable (default 0.0) | Not enforced for furnaces | Parity |
| Ideal capacity support | Yes (for coarse timesteps) | Yes | **Improved** |

### Ideal Capacity Control

HARES supports ideal capacity control via `ControlSignal::IdealCapacity`:
```rust
// furnace.rs:471-474
fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
    apply_heating_control_unchecked(&mut self.hvac, signal, "Gas Furnace")?;
    apply_simple_heating_ideal_capacity_control(&mut self.hvac, signal, self.rated_capacity_w);
    Ok(())
}
```

This enables the thermal solver to specify a desired capacity that gets converted to a duty cycle fraction.

---

## 5. Output Ports & Telemetry

### Files Analyzed
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/furnace.rs` - Lines 283-300, 501-512, 556-591

### Port Declarations

```rust
// furnace.rs:283-289
ports: vec![
    PortDeclaration::fuel(),        // Natural gas input
    PortDeclaration::electrical(),  // Fan power only
    PortDeclaration::thermal(zone), // Delivered heat to zone
],
```

### Telemetry Fields

| Telemetry Key | Unit | Description | OCHRE Equivalent |
|---------------|------|-------------|------------------|
| `electric_kw` | kW | Fan-only electric power | `fan_kw` |
| `fan_kw` | kW | Fan electric power (alias) | `fan_kw` |
| `fuel_input_w` | W | Fuel input power (capacity/AFUE) | `fuel_rate` |
| `thermal_output_w` | W | Delivered heat (post-DSE) | `heat_delivered` |
| `operating_mode` | enum | 0=Off, 1=Heating | `operating_mode` |
| `supply_air_temp_c` | C | Configured supply-air temp | `supply_temp` |
| `heating_setpoint_c` | C | Active heating setpoint | `T_set_heat` |
| `cooling_setpoint_c` | C | Active cooling setpoint | `T_set_cool` |

### Default Telemetry Values

```rust
// furnace.rs:501-512
fn gas_furnace_default_telemetry() -> Telemetry {
    let mut telemetry = Telemetry::with_capacity(8);
    telemetry.insert(tk::FAN_KW, 0.0);
    telemetry.insert(tk::ELECTRIC_KW, 0.0);
    telemetry.insert(tk::FUEL_INPUT_W, 0.0);
    telemetry.insert(tk::THERMAL_OUTPUT_W, 0.0);
    telemetry.insert(tk::OPERATING_MODE, 0.0);
    telemetry.insert(tk::SUPPLY_AIR_TEMP_C, 0.0);
    telemetry.insert(tk::HEATING_SETPOINT_C, 0.0);
    telemetry.insert(tk::COOLING_SETPOINT_C, 0.0);
    telemetry
}
```

### Core Output

```rust
// furnace.rs:406-419
self.core_output = CoreOutput {
    flows: CoreFlows {
        electric_kw: Some(ElectricPower::Consumption(fan_kw.max(0.0))),
        reactive_power_kvar: None,
        fuel_w: Some(FuelPower {
            fuel_type: self.fuel_type,
            consumption_w: fuel_input_w.max(0.0),
        }),
    },
    state: CoreState {
        operating_mode: Some(self.operating_mode),
        soc: None,
    },
};
```

---

## 6. Dwelling Integration

### Files Analyzed
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/furnace.rs` - Lines 374-387
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/hvac_core.rs` - Lines 1640-1700

### Thermal Solver Integration

The gas furnace integrates with the thermal solver through the zone thermal ports:

```rust
// furnace.rs:376-387
// Fan waste heat contributes to zone sensible gain (OCHRE HVAC.py line 543).
let fan_heat_w = fan_kw * 1000.0;
let total_sensible_w = gross_capacity_w + fan_heat_w;

if total_sensible_w > 0.0 {
    self.hvac.write_zone_thermal_contributions(
        ports,
        total_sensible_w,
        0.0,
        ThermalCategory::HvacHeating,
    )?;
}
```

### Zone Heat Fractions

The furnace correctly implements zone heat fractions based on ASHRAE 152 DSE:
- Duct losses go to `duct_zone_id` (attic, garage, etc.)
- Remaining heat is delivered to the conditioned zone
- Basement routing is supported via `basement_heat_frac`

```rust
// furnace.rs:393-394
// Telemetry reports delivered capacity (post-DSE) for the conditioned zone.
let thermal_output_w = total_sensible_w * self.hvac.duct_dse.clamp(0.0, 1.0);
```

### Energy Conservation

HARES correctly models energy conservation:
- **Fuel input** = gross capacity / AFUE (independent of DSE)
- **Thermal to zones** = gross capacity * DSE
- **Duct losses** = gross capacity * (1 - DSE) → routed to duct zone

---

## Issues and Findings

### Issue 1 (Medium Severity): Startup Capacity Degradation Incorrectly Applied to Gas Furnaces

**Files**:
- `/home/rich/src/HARES/crates/hares-io/src/hpxml/resolve_hvac.rs:309` 
- `/home/rich/src/HARES/crates/hares-io/src/hpxml/resolve_hvac.rs:1256-1258`

**Evidence**:

OCHRE `parse_hvac()` line 914-917:
```python
# Add startup capacity degradation factor for AC and heat pumps
if has_heat_pump or not is_heater:
    c_d = utils_equipment.calc_c_d(...)
    out["Startup Capacity Degradation (-)"] = c_d
```

For a `HeatingSystem` entry (gas furnace), `has_heat_pump=False` and `is_heater=True`.
The condition is `False or not True = False`. OCHRE does **not** write `Startup Capacity
Degradation` for gas furnaces.

HARES `resolve_hvac.rs:309` calls `insert_startup_degradation(&mut params, &name, true)`
for every `HeatingSystem`, including gas furnaces. Inside, the code looks for
`"efficiency_hspf"` but gas furnaces have `"heating_efficiency"` (AFUE units). The
key is absent, so `efficiency_ip` defaults to `8.0`, and `calc_startup_degradation`
returns `0.11` (HSPF >= 7.0 branch). The value `startup_cd = 0.11` is inserted into
the spec.

**Impact**: Gas furnaces will have a startup capacity degradation ramp applied for
approximately `20 * 0.11 + 0.4 = 2.6 minutes` after each on-cycle. OCHRE applies
no such ramp. This degrades modeled heating output for the first ~2-3 minutes of
every furnace on-cycle.

**Recommended Fix**: In `resolve_hvac.rs`, gate `insert_startup_degradation` for heating equipment
the same way OCHRE does: only call it for heat pump heaters (`"ASHP Heater"`,
`"MSHP Heater"`), not for `"Gas Furnace"`, `"Electric Furnace"`, `"Gas Boiler"`,
`"Electric Boiler"`, or `"Electric Baseboard"`.

---

### Issue 2 (Low Severity): Telemetry Key Naming Inconsistency

**Files**:
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/furnace.rs:398`

**Evidence**:

HARES uses `FUEL_INPUT_W` key (`"fuel_input_w"`) for gas furnace fuel consumption.
This is consistent with other equipment (boilers). OCHRE uses `"fuel_rate"`.
HARES naming is more descriptive and consistent with the physics (input power in watts).

**Impact**: None - this is a naming preference, not a functional issue.

**Assessment**: Acceptable. The naming is clear and consistent within HARES.

---

## Summary

| Aspect | HARES vs OCHRE | Assessment |
|--------|----------------|------------|
| HPXML parsing | Complete | Good |
| Default values | Match OCHRE | Good |
| Control logic | Match OCHRE + ideal capacity support | Good |
| Telemetry | Comprehensive | Good |
| Thermal solver integration | Correct energy conservation | Good |
| Startup degradation | **BUG - incorrectly applied** | Needs fix |

---

## Recommendations

1. **Fix startup capacity degradation bug** (Medium Priority): Modify `resolve_hvac.rs` to exclude gas furnaces from startup degradation insertion, matching OCHRE behavior.

2. **No other changes needed**: The single-stage gas furnace implementation is well-designed and preserves OCHRE physics correctly.
