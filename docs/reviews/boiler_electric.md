# Electric Boiler Review

## Overview

This review examines the HARES Electric Boiler implementation, comparing it against OCHRE and documenting findings.

## 1. HPXML Parsing

### HPXML Attributes Parsed

The HPXML resolver (`crates/hares-io/src/hpxml/resolve_hvac.rs`, lines 602-638) parses the following attributes for electric boilers:

| Attribute | Source | Notes |
|-----------|--------|-------|
| `heating_capacity_w` | Required | Heating capacity in watts |
| `heating_efficiency` | Optional | Efficiency value (default 1.0) |
| `efficiency_cop` | Optional | COP as alternative to efficiency |
| `flow_rate_kg_s` | Optional | Default 0.5 kg/s |
| `return_temp_c` | Optional | Default 40.0 C |
| `number_of_speeds` | Optional | Default 1 |

### EIR Derivation

Efficiency is derived via `resistance_efficiency_from_params()` (lines 341-347):

```rust
fn resistance_efficiency_from_params(params: &Map<String, Value>) -> f64 {
    params
        .get("heating_efficiency")
        .and_then(Value::as_f64)
        .or_else(|| params.get("efficiency_cop").and_then(Value::as_f64))
        .unwrap_or(1.0)
}
```

**Issue**: The HPXML sample file (`base-hvac-boiler-elec-only.xml`) specifies AFUE=0.98 for electric boilers. However, the resolver does NOT parse AFUE for electric boilers - it only parses `heating_efficiency` or `efficiency_cop`. This means the 0.98 AFUE from HPXML is lost and defaults to EIR=1.0.

**OCHRE Behavior**: OCHRE uses EIR=1.0 for electric resistance heating by default, which matches the physics of resistive elements. However, HPXML allows specifying AFUE for electric boilers, which HARES ignores.

## 2. OCHRE Defaults

### OCHRE Electric Boiler Implementation

In OCHRE (`vendors/OCHRE/ochre/Equipment/HVAC.py`, lines 668-669):

```python
class ElectricBoiler(Heater):
    name = "Electric Boiler"
```

The OCHRE ElectricBoiler class is essentially empty - it only sets the name and inherits all behavior from the `Heater` base class. There are no boiler-specific defaults beyond what Heater provides.

### Heater Base Class Defaults

From the test file (`test/test_equipment/test_hvac.py`, lines 79-102), OCHRE uses:
- **EIR**: 1.0 (pure resistive conversion)
- **Capacity**: User-specified (5000W in test)
- **Duct DSE**: 1.0 (no duct losses for hydronic systems)
- **No hydronic distribution parameters** - OCHRE does not model hydronic loops explicitly

### HARES Defaults

HARES default values in `heating_config.rs` (lines 159-174):

| Parameter | Default | Source |
|-----------|---------|--------|
| `eir` | 1.0 | Matches OCHRE |
| `capacity_w` | 0.0 | Required, no default |
| `flow_rate_kg_s` | 0.5 | HARES-specific |
| `return_temp_c` | 40.0 | HARES-specific |
| `fluid_type` | Water | HARES-specific |
| `loop_id` | None | Must be configured |

**Comparison**: HARES correctly preserves OCHRE's EIR=1.0 default. The hydronic parameters (flow_rate, return_temp, fluid_type) are HARES extensions beyond OCHRE.

## 3. HARES Wiring

### Configuration Flow

The electric boiler configuration flows as follows:

1. **HPXML Parsing** (`resolve_hvac.rs`):
   - `try_build_electric_boiler_config()` extracts params
   - Creates `ElectricBoilerConfig` typed config

2. **Equipment Instantiation** (`boiler.rs`, line 612-615):
   ```rust
   registry.register(
       "Electric Boiler",
       Box::new(|config| Box::new(ElectricBoiler::new(config))),
   );
   ```

3. **Initialization** (`boiler.rs`, lines 171-195):
   ```rust
   fn init(&mut self, config: &EquipmentConfig, env: &EnvironmentState) -> crate::Result<()> {
       self.hvac.init(config, env)?;
       let typed = config.require_typed::<ElectricBoilerConfig>("Electric Boiler")?;
       self.rated_capacity_w = typed.capacity_w.max(0.0);
       self.eir = typed.eir;
       // ... hydronic parameters
   }
   ```

### Port Declarations

The electric boiler declares two ports (`boiler.rs`, lines 143-146):

```rust
ports: vec![
    PortDeclaration::electrical(),
    PortDeclaration::fluid(loop_id, FluidType::Water),
],
```

This correctly models the electric input and hydronic output.

## 4. Control Logic

### Thermostat Control

Control is handled via `update_heating_control()` in `helpers.rs` (lines 130-145):

```rust
pub fn update_heating_control(hvac: &mut HvacEquipment, env: &EnvironmentState) -> OperatingMode {
    match hvac.update_mode(env) {
        Ok(super::thermostat::ThermostatMode::Heating) => {
            if !hvac.use_ideal_capacity(env) {
                hvac.duty_cycle = 1.0;  // On/off cycling
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

### Control Capabilities

From `boiler.rs`, lines 134-137:

```rust
control_capabilities: ControlCapabilities::THERMAL_SETPOINT
    | ControlCapabilities::THERMAL_SETPOINT_DELTA
    | ControlCapabilities::IDEAL_CAPACITY,
```

Supports:
- Thermal setpoint control
- Deadband/delta control  
- Ideal capacity control (for coarse timesteps)

### Issues

1. **No Outdoor Reset**: There is no outdoor reset control logic for modulating supply temperature based on outdoor conditions. This is a common boiler optimization that HARES does not implement.

2. **No Setpoint Temperature**: The electric boiler does not model a supply water temperature setpoint. The supply temperature is purely a function of flow rate and thermal output (line 215-219):
   ```rust
   let supply_temp_c = if self.flow_rate_kg_s > 0.0 {
       return_temp_c + thermal_output_w / (self.flow_rate_kg_s * CP_LIQUID_WATER_J_KG_K)
   } else {
       return_temp_c
   };
   ```

3. **No Circulation Control**: There is no modeling of pump/fan control for hydronic circulation. The flow rate is fixed at initialization time.

## 5. Output Ports & Telemetry

### Telemetry Fields

From `boiler.rs`, lines 659-687:

| Field | Unit | Description |
|-------|------|-------------|
| `ELECTRIC_KW` | kW | Active power draw |
| `THERMAL_OUTPUT_W` | W | Thermal output to hydronic loop |
| `SUPPLY_TEMP_C` | C | Fluid supply temperature |
| `RETURN_TEMP_C` | C | Fluid return temperature |
| `OPERATING_MODE` | enum | 0=Off, 1=Heating |

### Core Output

From `boiler.rs`, lines 245-255:

```rust
self.core_output = CoreOutput {
    flows: CoreFlows {
        electric_kw: Some(ElectricPower::Consumption(electric_kw.max(0.0))),
        reactive_power_kvar: None,
        fuel_w: None,
    },
    state: CoreState {
        operating_mode: Some(self.operating_mode),
        soc: None,
    },
};
```

### Port Contributions

During `step()`, the boiler contributes to:
- **Electrical port**: Active power = `thermal_output_w * eir / 1000`
- **Fluid port**: Flow rate, supply temp, return temp

**Comparison with Gas Boiler**: The gas boiler additionally reports `FUEL_INPUT_W`, `JACKET_LOSS_W`, and `EIR` (instantaneous efficiency after polynomial adjustment). The electric boiler lacks these - but they're not applicable since electric boilers don't have fuel input or efficiency curves.

## 6. Dwelling Integration

### Hydronic Distribution

The electric boiler integrates with the dwelling through the **Fluid Domain**:

1. **Fluid Solver** (`hares-envelope/src/fluid_solver.rs`): Models energy balance on fluid loops
2. **Loop ID**: Each boiler is assigned a `loop_id` that connects to the fluid domain
3. **Return Temperature**: The boiler reads return temperature from the fluid domain (line 213-214):
   ```rust
   let return_temp_c =
       loop_return_temp_c(env, self.loop_id).unwrap_or(self.default_return_temp_c);
   ```

### Distribution System Mapping

From HPXML, the distribution system is specified as `HydronicDistribution` with type `baseboard` (or other hydronic types). This is mapped to the loop_id in HARES.

### Physics Model

The thermal output is calculated as:
```
thermal_output_w = rated_capacity_w * duty_cycle
electric_kw = thermal_output_w * eir / 1000 * space_fraction
supply_temp_c = return_temp_c + thermal_output_w / (flow_rate_kg_s * cp_water)
```

This correctly models the energy conversion and hydronic heat transfer.

## Summary

| Aspect | HARES | OCHRE | Notes |
|--------|-------|-------|-------|
| EIR Default | 1.0 | 1.0 | Matches OCHRE |
| Capacity | Required | Required | Both require input |
| Hydronic Parameters | Yes (flow, return temp, fluid type) | No | HARES extension |
| Outdoor Reset | No | No | Common optimization not implemented |
| Supply Temp Setpoint | No | No | Calculated from physics |
| Telemetry | Electric, thermal, temps | Limited | HARES more complete |

## Issues

1. **HPXML AFUE Ignored**: Electric boiler AFUE from HPXML is not parsed. The 0.98 AFUE in sample files is lost, defaulting to EIR=1.0. This is technically correct physics (resistive = 100%) but loses HPXML fidelity.

2. **No Outdoor Reset Control**: Electric boilers lack outdoor reset modulation for supply temperature. This is a common energy-saving control strategy.

3. **Fixed Flow Rate**: Flow rate is fixed at initialization and cannot vary with demand or control signals.

4. **No Pump Modeling**: Unlike gas boilers which model pump/fan power, electric boilers don't model circulation pump power (though this may be negligible for electric boilers).
