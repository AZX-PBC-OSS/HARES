# Electric Baseboard Heating Configuration Review

**Reviewer**: Code Review Agent  
**Date**: 2026-03-31  
**Scope**: HPXML parsing, OCHRE defaults, HARES wiring, control logic, output ports, dwelling integration

---

## 1. HPXML Parsing

### Attributes Parsed

| Attribute | HARES Parsing | Source Location |
|-----------|---------------|-----------------|
| `capacity_w` | Parsed from `heating_capacity_w` param | `resolve_hvac.rs:645` |
| `zone_id` | **NOT parsed** - defaults to `None` | `resolve_hvac.rs:649` |
| `eir` | Hardcoded to `1.0` (not parsed from HPXML) | `resolve_hvac.rs:651` |

### HPXML Resolution Flow

The HPXML resolver function `try_build_electric_baseboard_config` (line 641-658):
```rust
fn try_build_electric_baseboard_config(
    name: &str,
    params: &Map<String, Value>,
) -> Option<EquipmentConfig> {
    let capacity_w = params.get("heating_capacity_w").and_then(Value::as_f64)?;

    let cfg = ElectricBaseboardConfig {
        equipment_id: None,
        zone_id: None,        // <-- NOT parsed from HPXML
        capacity_w,
        eir: 1.0,             // <-- Hardcoded, not from HPXML
    };
    // ...
}
```

### Issue: Zone Assignment Not from HPXML

**Finding**: The HPXML resolver does NOT extract zone information for electric baseboard heating. The `zone_id` is set to `None`, which later falls back to `ZoneId(1)` in the equipment constructor:

```rust
// baseboard.rs:55
let zone = zone_id_from_config(&config).unwrap_or(ZoneId(1));
```

This means multi-zone baseboard systems in HPXML would not be correctly assigned to their respective zones in HARES.

---

## 2. OCHRE Defaults

### OCHRE Implementation

The OCHRE ElectricBaseboard class (HVAC.py:672-680):

```python
class ElectricBaseboard(Heater):
    name = "Electric Baseboard"

    def __init__(self, **kwargs):
        super().__init__(**kwargs)
        # force duct dse to 1
        self.duct_dse = 1
```

### HARES Defaults Comparison

| Parameter | OCHRE Default | HARES Default | Match? |
|-----------|---------------|---------------|--------|
| Duct DSE | 1.0 (forced) | 1.0 (forced in init) | YES |
| EIR | 1.0 (from Heater parent) | 1.0 (default in config) | YES |
| Fan | None (no fan) | No fan configured | YES |
| Zonal control | Per-zone thermostat | Per-zone thermostat | YES |
| Supply air temp | 0.0 (not applicable) | 0.0 (Baseboard type) | YES |

### OCHRE Physics Preserved in HARES

1. **No duct losses** - HARES correctly forces `duct_dse = 1.0` in `init()`:
   ```rust
   // baseboard.rs:99-103
   self.hvac.duct_dse = 1.0;
   self.hvac.duct_zone_id = None;
   self.hvac.basement_heat_frac = 0.0;
   ```

2. **Efficiency = 100%** - HARES uses default EIR of 1.0, matching electric resistance heating

3. **No air distribution** - No fan is configured, matching OCHRE's zonal baseboard behavior

---

## 3. HARES Wiring

### Config Structure

`ElectricBaseboardConfig` (heating_config.rs:178-203):
```rust
pub struct ElectricBaseboardConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    pub capacity_w: f64,        // Required
    pub eir: f64,               // Default: 1.0
}
```

### Initialization Flow

1. `ElectricBaseboard::new()` creates equipment with default zone (ZoneId(1) if not specified)
2. `init()` loads typed config:
   ```rust
   let typed = config.require_typed::<ElectricBaseboardConfig>("Electric Baseboard")?;
   self.rated_capacity_w = typed.capacity_w.max(0.0);
   self.eir = typed.eir;
   ```
3. Forces DSE = 1.0, clears duct/basement configuration

---

## 4. Control Logic

### Zonal Baseboard Control

HARES uses the shared heating control helper (`helpers.rs:130-145`):

```rust
pub fn update_heating_control(hvac: &mut HvacEquipment, env: &EnvironmentState) -> OperatingMode {
    match hvac.update_mode(env) {
        Ok(ThermostatMode::Heating) => {
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

The equipment declares (`baseboard.rs:64-66`):
- `THERMAL_SETPOINT` - Set heating setpoint
- `THERMAL_SETPOINT_DELTA` - Adjust setpoint by delta
- `IDEAL_CAPACITY` - Support solver-driven capacity (for coarse timesteps)

### Thermostat Behavior

- Uses `ThermostatConfig` with default hysteresis of 1.0°C
- Default heating setpoint: 20.0°C
- Supports schedule-based setpoints via `heating_setpoint_source`
- Minimum cycle time: 60 seconds (default)

### No Air Distribution

**Key difference from forced-air systems**: Electric baseboard has **no air distribution**. The thermostat controls whether the heating element is on/off - there is no fan, no supply air temperature, no airflow fraction. This matches OCHRE behavior.

---

## 5. Output Ports & Telemetry

### Port Declarations

```rust
// baseboard.rs:73-76
ports: vec![
    PortDeclaration::electrical(),
    PortDeclaration::thermal(zone),
],
```

### Telemetry Fields

| Field | Unit | Description |
|-------|------|-------------|
| `ELECTRIC_KW` | kW | Active power draw |
| `THERMAL_OUTPUT_W` | W | Delivered sensible zone heat |
| `OPERATING_MODE` | enum | 0=Off, 1=Heating |

### Step Calculation

```rust
// baseboard.rs:132-148
let duty = self.hvac.duty_cycle.clamp(0.0, 1.0);
let thermal_output_w = self.rated_capacity_w * duty;
let electric_kw = thermal_output_w * self.eir / 1_000.0 * self.hvac.space_fraction;

// Electrical port
ports.accumulate(&PortContribution::Electrical {
    active_power_kw: electric_kw,
    reactive_power_kvar: 0.0,
})?;

// Thermal port - direct zone delivery
self.hvac.write_zone_thermal_contributions(
    ports,
    thermal_output_w,
    0.0,
    ThermalCategory::HvacHeating,
)?;
```

### Core Output

```rust
// baseboard.rs:154-165
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

---

## 6. Dwelling Integration

### Direct Zone Heating

Electric baseboard integrates with the thermal solver via **direct zone heating** - no duct losses:

```rust
// baseboard.rs:99-103 (init)
self.hvac.duct_dse = 1.0;
self.hvac.duct_zone_id = None;
self.hvac.basement_heat_frac = 0.0;
self.hvac.update_zone_heat_fractions();
```

This produces `zone_heat_fractions = [(zone_id, 1.0)]` meaning 100% of thermal output goes directly to the conditioned zone.

### Thermal Contribution Path

1. Calculate duty cycle from thermostat mode
2. Compute thermal output: `thermal_output_w = rated_capacity_w * duty`
3. Write to thermal port with `ThermalCategory::HvacHeating`
4. Apply `space_fraction` scaling to electric power

### Space Fraction Support

The equipment respects `fraction_heating_load_served` config:
```rust
// baseboard.rs:134
let electric_kw = thermal_output_w * self.eir / 1_000.0 * self.hvac.space_fraction;
```

This allows partial-load served scenarios (e.g., baseboard serving 70% of zone heating load).

---

## Summary

### OCHRE Physics Preserved

1. **No duct losses** - DSE forced to 1.0 in both OCHRE and HARES
2. **100% efficiency** - EIR default of 1.0 for electric resistance
3. **Zonal control** - Per-zone thermostat, no air distribution
4. **Direct zone delivery** - Thermal output goes directly to zone

### Issues Found

| Issue | Severity | Description |
|-------|----------|-------------|
| HPXML zone_id not parsed | Medium | Zone assignment defaults to ZoneId(1), not from HPXML |
| HPXML efficiency not parsed | Low | EIR hardcoded to 1.0, not from HPXML efficiency spec |

### Recommendations

1. **Zone assignment from HPXML**: Enhance `try_build_electric_baseboard_config` to extract zone information from HPXML (similar to how duct zones are resolved for forced-air systems)

2. **Efficiency from HPXML**: Parse heating efficiency from HPXML annual heating efficiency specification if present

---

*Review generated from code analysis of HARES electric baseboard implementation*
