# ERV/HRV Equipment Review

## Summary

HARES provides a significantly more sophisticated ERV/HRV implementation than OCHRE, with EnergyPlus-grade physics for calculating supply air conditions. HARES computes actual supply temperature and humidity based on recovery effectiveness, whereas OCHRE uses a simplified model that reduces effective flow rates in the thermal solver.

## 1. HPXML Parsing

### Attributes Parsed by HARES

| HPXML Attribute | HARES Field | Notes |
|-----------------|-------------|-------|
| `RatedFlowRate` | `flow_rate_m3_s` | Converted from CFM to m³/s |
| `FanPower` | `fan_power_w` | In watts |
| `FanType` | `ventilation_type` | Maps to "erv", "hrv", "exhaust_fan" |
| `SensibleRecoveryEfficiency` | `sensible_effectiveness` | Only if > 0 |
| `TotalRecoveryEfficiency` | `latent_effectiveness` | Derived as (total - sensible) |
| `HoursInOperation` | `hours_in_operation` | Used to compute schedule fraction |
| `UsedForWholeBuildingVentilation` | - | Filter criteria |
| `UsedForSeasonalCoolingLoadReduction` | - | Filter criteria |

**Source**: `crates/hares-io/src/hpxml/resolve_loads.rs:567-639`

### Fan Type Mapping

```rust
match fan_type_lower.as_str() {
    "exhaust only" | "supply only" => "exhaust_fan",
    "energy recovery ventilator" => "erv",
    "heat recovery ventilator" | "balanced" => "hrv",
    _ => "hrv",  // default
}
```

### Default Fan Power (HPXML Parser)

When `FanPower` is not specified in HPXML, the parser applies defaults based on fan type:

| Fan Type | W/CFM |
|----------|-------|
| ERV/HRV/Balanced | 1.0 |
| Exhaust Only/Supply Only | 0.35 |
| Whole House Fan | 0.1 |
| Default | 0.35 |

## 2. OCHRE Defaults vs HARES Defaults

### Default Values Comparison

| Parameter | HARES Default | OCHRE Default | Notes |
|-----------|---------------|---------------|-------|
| `fan_power_w` | 50.0 | N/A (uses W/CFM) | HARES uses absolute watts |
| `flow_rate_m3_s` | 0.035 (~75 CFM) | From HPXML | Typical residential |
| `sensible_effectiveness` | 0.70 | 0 (if not specified) | Better HARES default |
| `latent_effectiveness` | 0.0 (HRV) | 0 | ERV gets derived value |
| `bypass_temp_min_c` | 18.0 | N/A | HARES-only feature |
| `bypass_temp_max_c` | 24.0 | N/A | HARES-only feature |
| `defrost_temp_c` | -5.0 | N/A | HARES-only feature |
| `defrost_effectiveness_fraction` | 0.5 | N/A | HARES-only feature |

**HARES Default Source**: `crates/hares-equipment/src/ventilation.rs:113-120`

**OCHRE Default Source**: `vendors/OCHRE/ochre/Models/Envelope.py:502-507`

### Issues Identified

1. **HARES Default Sensible Effectiveness (0.70)**: This is a reasonable typical value, but may not reflect actual equipment performance. Could be considered a "good" default.

2. **HARES Default Fan Power (50W)**: For a 75 CFM system, this equals ~0.67 W/CFM, which is reasonable. The HPXML parser would use 1.0 W/CFM for ERV/HRV if FanPower is not specified, which is more conservative.

3. **OCHRE Default Sensible Recovery (0)**: OCHRE defaults to no recovery if not specified, which is physically incorrect for HRV/ERV systems. HARES defaults to 70%, which is more realistic.

## 3. HARES Wiring from HPXML to Equipment

### Configuration Flow

1. **HPXML Parsing** (`resolve_loads.rs:567-639`): Extracts ventilation fan attributes
2. **Typed Config Creation**: Builds `VentilationConfig` struct
3. **Equipment Instantiation**: Creates `Ventilation` equipment with typed config
4. **Init**: Applies typed config via `init_typed()`

### Wiring Details

```rust
// From resolve_loads.rs
let cfg = VentilationConfig {
    equipment_id: None,
    zone_id: None,
    flow_rate_m3_s: flow_m3_s.unwrap_or(0.0),
    fan_power_w,
    sensible_effectiveness: (sensible_re > 0.0).then_some(sensible_re),
    latent_effectiveness: (latent_re > 0.0).then_some(latent_re),
    bypass_temp_min_c: None,  // HARES uses default
    bypass_temp_max_c: None,  // HARES uses default
    defrost_temp_c: None,     // HARES uses default
    defrost_effectiveness_fraction: None,
    ventilation_type: Some(ventilation_type.to_string()),
    balanced: Some(balanced),
    hours_in_operation: child_f64(fan, "HoursInOperation"),
};
```

**Note**: HPXML parsing does NOT pass through:
- `bypass_temp_min_c` (not in HPXML schema)
- `bypass_temp_max_c` (not in HPXML schema)
- `defrost_temp_c` (not in HPXML schema)
- `defrost_effectiveness_fraction` (not in HPXML schema)

These always use HARES defaults.

## 4. Control Logic

### Supported Control Signals

HARES Ventilation supports three control signals:

| Control Signal | Behavior |
|----------------|----------|
| `ModeOverride` | Sets operating mode (Off, Standby, etc.) |
| `DemandResponse` | Sets DR level and duration |
| `LoadFraction` | If ≤ 0: Off, If > 0: Standby |

**Source**: `crates/hares-equipment/src/ventilation.rs:504-527`

### Operating Modes

- **Off**: Fan off, no power, no recovery
- **Standby**: Ready but not running
- **On/Running**: Active operation with recovery

### Bypass Mode (HARES Enhancement)

When outdoor temperature is within comfort range (default 18-24°C), bypass mode is activated:
- Supply air = outdoor air (no heat recovery)
- Enables "free cooling" when beneficial
- Telemetry reports `bypass_active = 1.0`

```rust
// From ventilation.rs:244-260
fn effective_sensible_effectiveness(&self, t_outdoor_c: f64) -> f64 {
    if self.ventilation_type == VentilationType::ExhaustFan {
        return 0.0;
    }
    // Bypass: when outdoor is within comfort range, bypass recovery entirely.
    if t_outdoor_c >= self.bypass_temp_min_c && t_outdoor_c <= self.bypass_temp_max_c {
        return 0.0;
    }
    // ... defrost logic
}
```

### Defrost Derating (HARES Enhancement)

When outdoor temperature drops below threshold (default -5°C), effectiveness is reduced:
- Effective sensible effectiveness = base × defrost_fraction (default 0.5)
- Same for latent effectiveness (ERV only)

```rust
// From ventilation.rs:254-259
if t_outdoor_c < self.defrost_temp_c {
    base * self.defrost_effectiveness_fraction
} else {
    base
}
```

### OCHRE Control

OCHRE does not implement bypass or defrost logic. Ventilation runs continuously based on schedule.

### Scheduling

HARES supports scheduling via `hours_in_operation`:
- Converts daily hours to fraction (e.g., 8 hours → 0.333)
- Applies to both flow rate and fan power

```rust
let schedule_frac = if let Some(hours) = c.hours_in_operation {
    (hours / 24.0).clamp(0.0, 1.0)
} else {
    1.0  // Default: always on
};
```

## 5. Output Ports & Telemetry

### Electrical Port

- Reports fan power as active power consumption
- Unit: kW

### Telemetry Fields

| Telemetry Key | Unit | Description |
|---------------|------|-------------|
| `ELECTRIC_KW` | kW | Total electrical power |
| `FAN_POWER_W` | W | Fan mechanical power |
| `SENSIBLE_RECOVERY_W` | W | Sensible heat recovered |
| `LATENT_RECOVERY_W` | W | Latent heat recovered (ERV only) |
| `SUPPLY_TEMP_C` | °C | Supply air temperature after recovery |
| `BYPASS_ACTIVE` | - | Bypass mode active (1=yes, 0=no) |

**Source**: `crates/hares-equipment/src/ventilation.rs:550-577`

### OCHRE Telemetry

OCHRE reports:
- `inf_heat`, `nat_vent_heat`, `forced_vent_heat` - heat flow components
- Flow rates but not supply conditions
- No bypass or defrost telemetry

## 6. Dwelling/Thermal Solver Integration

### HARES Integration Architecture

HARES uses a two-model approach:

1. **Equipment Model** (`Ventilation`): Computes:
   - Supply air temperature: `T_supply = T_outdoor + ε_sensible × (T_indoor - T_outdoor)`
   - Supply humidity ratio: `W_supply = W_outdoor + ε_latent × (W_indoor - W_outdoor)`
   - Recovery amounts (sensible and latent)
   - Fan power consumption

2. **Thermal Solver** (`infiltration.rs`): Applies ventilation loads via:
   - `apply_infiltration_and_ventilation()` 
   - Uses `VentilationConfig` from envelope config
   - Receives flow rates and applies recovery efficiency to reduce effective flow

### Key Integration Points

**VentilationConfig in Thermal Solver** (`thermal_solver/config.rs:83-98`):
```rust
pub struct VentilationConfig {
    pub zone_flow_m3_s: HashMap<ZoneId, f64>,
    pub balanced: bool,                      // From HPXML
    pub sensible_recovery_efficiency: f64,   // From HPXML
    pub latent_recovery_efficiency: f64,     // From HPXML
}
```

**Important**: The thermal solver's `VentilationConfig` is separate from the equipment's `VentilationConfig`. They are populated from different sources:
- Solver config: From HPXML parsed directly in the envelope layer
- Equipment config: From equipment specification (also from HPXML)

### Comment in Source

```rust
// From ventilation.rs:448-450
// Ventilation thermal load is handled by the envelope solver's
// apply_infiltration_and_ventilation() — do NOT add it here to avoid
// double-counting. Equipment reports fan power and telemetry only.
```

This is the correct design: equipment handles recovery physics and fan power, thermal solver handles zone heat exchange.

### OCHRE Integration

OCHRE integrates ventilation directly in the Envelope model:

```python
# From Envelope.py:631-642
sensible_gain, sensible_flow, latent_flow, self.inf_flow, self.nat_vent_flow = _ventilation_flows_and_gain(
    self.inf_flow,
    self.nat_vent_flow,
    self.forced_vent_flow,
    self.balanced_ventilation,
    self.sens_recovery_eff,
    self.lat_recovery_eff,
    density,
    delta_t,
    h_limit_float,
    has_h_limit,
)
```

OCHRE's `_ventilation_flows_and_gain()` reduces effective flow rates based on recovery efficiency rather than computing supply conditions explicitly.

## 7. Physics Comparison: HARES vs OCHRE

### HARES Advantages (Better Physics Preserved)

1. **Explicit Supply Air Calculation**: HARES computes actual supply temperature using effectiveness equations, not just reducing flow rates

2. **Bypass Mode**: HARES implements free cooling when outdoor conditions are favorable

3. **Defrost Derating**: HARES reduces effectiveness at low temperatures to simulate frost formation

4. **Telemetry Quality**: HARES reports supply temperature, recovery amounts, and bypass state for observability

### HARES Issues / Concerns

1. **Duplicate Configuration**: Two `VentilationConfig` structs exist (equipment and solver) - potential for inconsistency

2. **Missing HPXML Attributes**: No HPXML attribute for:
   - Bypass temperature thresholds
   - Defrost temperature threshold
   - Defrost effectiveness fraction
   - These always use HARES defaults

3. **Default Sensible Effectiveness (0.70)**: While reasonable, it's not derived from HPXML data

4. **No Fan Power Curves**: Fan power is constant; does not model variable-speed fan curves

5. **Schedule Simplification**: Converts hours to constant fraction; does not support time-varying schedules

## 8. Recommendations

### Preserve These HARES Features
- Supply air temperature calculation
- Bypass mode for free cooling
- Defrost derating
- Rich telemetry (supply temp, recovery, bypass state)

### Issues to Address
1. Consider consolidating VentilationConfig between equipment and solver to avoid duplication
2. Document that bypass/defrost thresholds use HARES defaults (not from HPXML)
3. Consider adding fan power curves for variable-speed ERV/HRV units
4. The default 70% sensible effectiveness is good but could be made configurable

### OCHRE Parity Notes
- HARES physics is superior to OCHRE - no regression in functionality
- OCHRE does not have bypass or defrost - HARES enhancements are valid additions
- OCHRE uses simpler flow reduction model; HARES explicit calculation is more accurate
