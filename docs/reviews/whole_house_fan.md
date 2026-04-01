# Whole House Fan (WHF) Equipment Review

## Overview

Whole House Fans (WHF) in HARES are implemented as a special case of the **Ventilation** equipment type. When HPXML specifies a fan with `FanType="whole house fan"` or marks it with `UsedForSeasonalCoolingLoadReduction="true"`, it is parsed and wired as a Ventilation fan with type "exhaust_fan".

---

## 1. HPXML Parsing

**File**: `crates/hares-io/src/hpxml/resolve_loads.rs` (lines 567-640)

### Parsed Attributes

| HPXML Attribute | HARES Field | Notes |
|-----------------|-------------|-------|
| `RatedFlowRate` | `flow_rate_m3_s` | Converted from CFM to m³/s |
| `FanPower` | `fan_power_w` | Direct wattage |
| `SensibleRecoveryEfficiency` | `sensible_effectiveness` | Only for HRV/ERV |
| `TotalRecoveryEfficiency` | `latent_effectiveness` | Derived as `total - sensible` |
| `HoursInOperation` | `hours_in_operation` | Daily operating hours |
| `UsedForWholeBuildingVentilation` | - | Filter criterion |
| `UsedForSeasonalCoolingLoadReduction` | - | Identifies WHF |

### Fan Type Mapping

```rust
match fan_type_lower.as_str() {
    "exhaust only" | "supply only" => "exhaust_fan",
    "energy recovery ventilator" => "erv",
    "heat recovery ventilator" | "balanced" => "hrv",
    // "whole house fan" falls through to default:
    _ => "hrv",  // ISSUE: WHF mapped to exhaust_fan earlier at line 138-140
}
```

**Note**: In `parse_ventilation_type()` (ventilation.rs:136-144), "whole house fan" is explicitly mapped to `VentilationType::ExhaustFan`, which is correct. However, the HPXML resolver maps it to "hrv" as a fallback, which is inconsistent.

---

## 2. OCHRE Defaults

### Default Power (W/CFM)

From `vendors/OCHRE/ochre/utils/hpxml.py` (lines 1626-1633):

| Fan Type | OCHRE Default (W/CFM) | HARES Default |
|----------|----------------------|---------------|
| Whole House Fan | **0.1** | **0.1** (via match) |
| Exhaust Only | 0.35 | 0.35 |
| Supply Only | 0.35 | 0.35 |
| HRV | 1.0 | 1.0 |
| ERV | 1.0 | 1.0 |

### HARES Equipment Defaults

From `crates/hares-equipment/src/ventilation.rs` (lines 113-120):

```rust
const DEFAULT_FAN_POWER_W: f64 = 50.0;
const DEFAULT_FLOW_RATE_M3_S: f64 = 0.035; // ~75 CFM
const DEFAULT_SENSIBLE_EFFECTIVENESS: f64 = 0.70;
const DEFAULT_LATENT_EFFECTIVENESS: f64 = 0.0;
const DEFAULT_BYPASS_TEMP_MIN_C: f64 = 18.0;
const DEFAULT_BYPASS_TEMP_MAX_C: f64 = 24.0;
```

### Night Cooling Setpoint

**Finding**: OCHRE does NOT appear to have a dedicated night cooling setpoint for whole house fans. The `parse_vent_fan` function in OCHRE (hpxml.py:1615-1642) simply extracts power and flow rate. The "seasonal cooling" is handled through the ventilation schedule constant, not through temperature-triggered control.

---

## 3. HARES Wiring

**Flow**: HPXML -> `resolve_ventilation()` -> `VentilationConfig` -> `Ventilation` equipment

### Key Points

1. **Equipment Type**: "Ventilation Fan" (registered at ventilation.rs:530-537)
2. **Zone Assignment**: `zone_id` defaults to ZoneId(1) if not specified
3. **Schedule**: `hours_in_operation` converted to a `ScheduleSource::Constant(hours/24)` fraction
4. **Thermal Integration**: Equipment does NOT push thermal load to zone — envelope solver handles ventilation heat exchange via `apply_infiltration_and_ventilation()`

---

## 4. Control Logic

### Current Implementation

The WHF (as Ventilation) uses **simple scheduled control**:

```rust
// From ventilation.rs:308-318
let schedule_frac = if let Some(hours) = c.hours_in_operation {
    (hours / 24.0).clamp(0.0, 1.0)
} else {
    1.0
};
self.schedule_source = ScheduleSource::Constant(schedule_frac);
```

### Control Capabilities

From equipment descriptor (ventilation.rs:210-213):
- `MODE_OVERRIDE` — Can be turned on/off
- `DEMAND_RESPONSE` — Supports DR signals
- `LOAD_FRACTION` — Supports fractional load

### Issues

1. **No Temperature-Triggered Control**: Unlike typical WHF behavior (running when outdoor is cooler than indoor at night), HARES uses constant operation based on schedule fraction only.

2. **No Thermostat Integration**: WHF is not connected to cooling setpoint or thermostat logic.

3. **OCHRE Assertion Not Enforced**: OCHRE asserts `HoursInOperation == 24` (line 1622 in hpxml.py), but HARES accepts any value 0-24.

---

## 5. Output Ports & Telemetry

### Electrical Port
- Writes `active_power_kw` (fan_power_w / 1000)

### Telemetry Fields

| Key | Description | Units |
|-----|-------------|-------|
| `electric_kw` | Total electric power | kW |
| `fan_power_w` | Fan electrical power | W |
| `sensible_recovery_w` | Sensible heat recovered (HRV/ERV) | W |
| `latent_recovery_w` | Latent heat recovered (ERV) | W |
| `supply_temp_c` | Supply air temperature after recovery | C |
| `bypass_active` | Bypass mode active flag | - |

### Notes

- For WHF (ExhaustFan), `sensible_recovery_w` and `latent_recovery_w` will always be 0.0 because `effective_sensible_effectiveness()` returns 0.0 for `VentilationType::ExhaustFan` (line 246-248).

---

## 6. Dwelling/Thermal Solver Integration

### How It Works

1. **Ventilation Equipment** runs and computes:
   - Fan power consumption
   - Supply air temperature (outdoor temp for WHF/ExhaustFan)
   - Effective flow rate based on schedule fraction

2. **Thermal Solver** (`crates/hares-envelope/src/thermal_solver/mod.rs:389-400`) calls:
   ```rust
   apply_infiltration_and_ventilation(
       &self.config,
       env,
       hvac_active,
       &mut latent_by_zone,
       &mut self.infiltration_buf,
   );
   ```

3. **Infiltration Module** (`crates/hares-physics/src/infiltration.rs`) computes:
   - Forced ventilation flow (from equipment)
   - Natural ventilation flow (from operable windows)
   - Combined sensible load on zone

### Thermal Solver Config

From `crates/hares-envelope/src/thermal_solver/config.rs:82-98`:

```rust
pub struct VentilationConfig {
    pub zone_flow_m3_s: HashMap<ZoneId, f64>,
    pub balanced: bool,                    // Whether system is balanced (ERV/HRV)
    pub sensible_recovery_efficiency: f64, // Only applied for balanced systems
    pub latent_recovery_efficiency: f64,   // Derived from total - sensible
}
```

### Key Integration Detail

**HARES preserves OCHRE physics**: The ventilation equipment computes supply conditions, but the actual thermal load on the zone is computed by the envelope solver using the same `_ventilation_flows_and_gain` formula from OCHRE (Envelope.py:59-87). This ensures physics parity.

---

## Summary: HARES vs OCHRE

### Where HARES Physics is Better

1. **Bypass Mode**: HARES implements bypass logic (ventilation.rs:249-252) where HRV/ERV bypasses recovery when outdoor temp is in comfort range (18-24C). OCHRE has this in the Envelope model but not explicitly in equipment.

2. **Defrost Derating**: HARES reduces recovery effectiveness below -5C (ventilation.rs:253-259), matching OCHRE.

3. **Proper Separation**: HARES correctly separates equipment (fan power, supply temp) from envelope solver (ventilation heat exchange), avoiding double-counting.

### Issues / Concerns

1. **No Dedicated WHF Type**: WHF is a special case of Ventilation, which may be confusing. Consider a distinct "WholeHouseFan" equipment type with:
   - Default fan type: exhaust_fan (no recovery)
   - No sensible/latent effectiveness by default
   - Optional: temperature-triggered control logic

2. **Missing Temperature-Based Control**: Real WHF operation is typically temperature-triggered (run when outdoor < indoor, especially at night). HARES only supports schedule-based control. Consider adding:
   - `night_cooling_setpoint_c` parameter
   - Integration with thermostat/cooling setpoint
   - AutomaticOff when outdoor > indoor

3. **Inconsistent Fan Type Mapping**: HPXML resolver maps "whole house fan" to "hrv" (line 613), but `parse_ventilation_type()` maps it to "exhaust_fan". This inconsistency could cause unexpected behavior.

4. **No Validation of HoursInOperation**: OCHRE asserts 24 hours for ventilation fans. HARES accepts 0-24 but the semantics of partial-hours operation for WHF (which typically runs at night) may not be correctly modeled.

---

## Recommendations

1. Add a dedicated "WholeHouseFan" equipment type with appropriate defaults (exhaust fan, low power 0.1 W/CFM, no recovery)

2. Implement temperature-triggered control: WHF should run when:
   - Outdoor temperature < Indoor temperature
   - Outdoor temperature < night_cooling_setpoint (default ~18C)
   - Time is within operating hours (typically nighttime)

3. Fix the fan type mapping inconsistency between HPXML resolver and equipment parser

4. Document that WHF thermal cooling is handled by the envelope solver's natural ventilation model, not directly by the equipment
