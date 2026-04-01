# Dual-Stage Air Conditioner Configuration Review

## Executive Summary

This review examines the implementation of dual-stage (two-speed) air conditioners in HARES, comparing the implementation against OCHRE defaults. The review identifies several issues where HARES physics diverges from OCHRE and documents missing telemetry for staged operation.

---

## 1. HPXML Parsing (OCHRE Side)

### Attributes Parsed for Dual-Stage AC

**Location**: `vendors/OCHRE/ochre/utils/hpxml.py`, lines 827-1010 (`parse_hvac` function)

| HPXML Attribute | Parsed Value | Notes |
|-----------------|--------------|-------|
| `CoolingCapacity` | Capacity in W | Full-load capacity |
| `AnnualCoolingEfficiency` | SEER rating | Units: Btu/hour → converted to COP |
| `NumberOfSpeeds` | Derived from SEER or CompressorType | See logic below |
| `CompressorType` | Override for speed count | "single stage", "two stage", "variable speed" |
| `CoolingSensibleHeatFraction` | SHR | Optional |
| `FractionCoolingLoadServed` | Space fraction | Optional |

### Speed Determination Logic

The number of speeds is determined by SEER rating (lines 862-876):

```python
if name == "mini-split":
    number_of_speeds = 4  # MSHP always variable speed
elif hvac.get("CompressorType") in speed_options:
    number_of_speeds = speed_options[hvac.get("CompressorType")]
elif convert(cop, "W", "Btu/hour") <= 15:
    number_of_speeds = 1  # Single-speed for SEER <= 15
elif convert(cop, "W", "Btu/hour") <= 21:
    number_of_speeds = 2  # Two-speed for 15 < SEER <= 21
else:
    number_of_speeds = 4  # Variable speed for SEER > 21
```

**Issue**: HPXML does not parse per-stage SEER values. The SEER is a composite rating; OCHRE uses a lookup table (see Section 2) to derive per-stage efficiency.

---

## 2. OCHRE Defaults for Dual-Stage AC

### Source: `defaults/HVAC Multispeed Parameters.csv`

| Parameter | 2-Speed AC Value | Notes |
|-----------|------------------|-------|
| Capacity Ratio 1 (Low) | **0.72** | Low speed is 72% of high speed |
| Capacity Ratio 2 (High) | 1.0 | High speed = rated capacity |
| Air Flow Ratio 1 | 0.86 | Low speed airflow 86% of high |
| COP 1 (Low) | 4.3-5.2 | Variable by SEER rating |
| COP 2 (High) | 4.0-4.9 | Slightly lower than low speed |
| SHR 1 | ~0.72 | Low stage SHR |
| SHR 2 | ~0.73 | High stage SHR |

### Default Control Parameters

**Location**: `vendors/OCHRE/ochre/Equipment/HVAC.py`, lines 749-761

```python
# Default control timing
self.control_type = control_type  # 'Time', 'Time2', or 'Setpoint'
self.min_time_in_speed = [
    dt.timedelta(minutes=min_time_in_low),   # Default: 5 minutes
    dt.timedelta(minutes=min_time_in_high),  # Default: 5 minutes
]
```

- **Default control type**: "Time"
- **Minimum low time**: 5 minutes
- **Minimum high time**: 5 minutes

### Default PLF (Part-Load Factor) Degradation

**Location**: `vendors/OCHRE/ochre/utils/equipment.py`, line 471+

The PLF degradation coefficient (Cd) is calculated based on equipment type and SEER:
- Default Cd = 0.25 for standard efficiency equipment
- Lower Cd (0.11) used for two-speed equipment per AHRI guidelines

---

## 3. HARES Wiring: HPXML to Equipment

### Configuration Flow

1. **HPXML → OCHRE parsing** (`hpxml.py`): Extracts SEER, capacity, number_of_speeds
2. **OCHRE → HARES conversion** (`dwelling/conversions.rs`): Merges equipment specs
3. **HARES equipment config** (`cooling_config.rs`): Creates `CentralAirConditionerConfig`

### HARES AC Configuration Fields

**Location**: `crates/hares-equipment/src/hvac/cooling_config.rs`, lines 17-106

```rust
pub struct CentralAirConditionerConfig {
    pub capacity_w: f64,           // Rated capacity
    pub eir: f64,                  // Energy input ratio
    pub number_of_speeds: u8,      // 1, 2, or 4
    pub stage_capacities_w: Option<Vec<f64>>,   // Per-stage capacities
    pub stage_eirs: Option<Vec<f64>>,           // Per-stage EIRs  
    pub stage_shrs: Option<Vec<f64>>,           // Per-stage SHRs
    // ... additional config options
}
```

### Mapping: Number of Speeds → SpeedControlMode

```rust
pub fn cooling_speed_control_mode(&self) -> SpeedControlMode {
    match self.number_of_speeds {
        1 => SpeedControlMode::SingleSpeed,
        2 => SpeedControlMode::TwoSpeedSetpoint,  // HARES default!
        4 => SpeedControlMode::VariableSpeedIdeal,
        _ => SpeedControlMode::SingleSpeed,
    }
}
```

**Issue**: HARES defaults to `TwoSpeedSetpoint` mode for 2-speed AC, while OCHRE defaults to "Time" mode. This is a behavioral difference.

---

## 4. Control Logic: Stage Transition

### HARES Speed Selection Modes

**Location**: `crates/hares-equipment/src/hvac/staging.rs`

| Mode | Trigger Condition | Implementation |
|------|-------------------|----------------|
| `TwoSpeedSetpoint` | Load > low_speed_capacity_fraction | Lines 123-150 |
| `TwoSpeedTime` | Zone temp moving wrong way after min_time | Lines 152-202 |
| `TwoSpeedAlternating` | Cycle between stages | Lines 91-110 |
| `MultiSpeedInterpolated` | Load-based interpolation | Lines 204-251 |

### TwoSpeedSetpoint Logic

```rust
fn select_two_speed_setpoint(&mut self, load_fraction: f64) -> SpeedSelection {
    let low_cap = self.low_speed_capacity_fraction.clamp(0.01, 0.999);
    let desired_index = if load_fraction > low_cap { 1 } else { 0 };
    // Enforce minimum time per speed
    // ...
}
```

### TwoSpeedTime Logic (OCHRE-compatible)

```rust
fn select_two_speed_time(&mut self, load_fraction: f64, zone_temp_c: Option<f64>, is_heating: bool) -> SpeedSelection {
    // If zone temp moving wrong way (heating: temp falling, cooling: temp rising)
    // AND min_time_per_speed_s has elapsed → escalate to high speed
    // Otherwise → stay at current or start at low
}
```

**Finding**: HARES supports both control modes (Setpoint and Time), but defaults to Setpoint. OCHRE defaults to Time.

---

## 5. Output Ports & Telemetry

### AC Telemetry Fields

**Location**: `crates/hares-equipment/src/hvac/ac_config.rs`, lines 22-37

```rust
pub(super) fn default_telemetry() -> Telemetry {
    telemetry.insert(tk::ELECTRIC_KW, 0.0);
    telemetry.insert(tk::SENSIBLE_COOLING_W, 0.0);
    telemetry.insert(tk::LATENT_COOLING_W, 0.0);
    telemetry.insert(tk::SHR, 1.0);
    telemetry.insert(tk::OPERATING_MODE, 0.0);
    telemetry.insert(tk::COP, 0.0);
    telemetry.insert(tk::RUNTIME_FRACTION, 0.0);
    telemetry.insert(tk::COMPRESSOR_KW, 0.0);
    telemetry.insert(tk::FAN_KW, 0.0);
    telemetry.insert(tk::SUPPLY_TEMP_C, 0.0);
    telemetry.insert(tk::APPARATUS_DEW_POINT_C, 0.0);
    telemetry.insert(tk::BYPASS_FACTOR, 0.0);
}
```

### Missing: Speed Index Telemetry for AC

**Finding**: The AC equipment does NOT report `speed_index` in telemetry, while the Heat Pump Heater does.

| Equipment | SPEED_INDEX in Telemetry |
|-----------|-------------------------|
| AC Cooler | **NO** |
| ASHP Heater | **YES** (line 674 in heater.rs) |

The internal state `last_speed_index` is tracked (air_conditioner.rs, line 906), but not exposed in telemetry output.

### Missing: Runtime by Stage

**Issue**: There is no telemetry for:
- Runtime at low speed stage
- Runtime at high speed stage  
- Total runtime at each stage

This makes it impossible to analyze staging behavior from output data.

---

## 6. Dwelling/Thermal Solver Integration

### How HVAC Gains Reach the Thermal Solver

**Location**: `crates/hares-core/src/dwelling/mod.rs`, lines 2219-2227

```rust
let hvac_heating_w = gains.hvac_heating_w.max(0.0);
let hvac_cooling_w = gains.hvac_cooling_w.abs();

// Passed to thermal solver:
thermal_solver.update(
    // ...
    hvac_heating_w,
    hvac_cooling_w,
)
```

### Staged Capacity Handling

The thermal solver receives the **actual delivered capacity** from the HVAC equipment:

1. HVAC calculates stage selection based on load fraction
2. Capacity is computed: `capacity_at_stage(stage_index) * duty_cycle`
3. This capacity is added to the thermal model as internal gains
4. The solver does not explicitly know which stage is active

**Finding**: The thermal solver integration is correct - it receives the actual (staged) capacity. There is no special handling needed for staged equipment because the equipment model already produces the correct capacity value.

---

## Issues Summary

### Issues Found

| # | Issue | Severity | Location |
|---|-------|----------|----------|
| 1 | **Missing SPEED_INDEX telemetry for AC** | Medium | `ac_config.rs` - not included in default_telemetry() |
| 2 | **Missing runtime-by-stage telemetry** | Medium | No tracking of time spent in each speed stage |
| 3 | **Different default low-speed capacity fraction** | Low | HARES: 0.50 (staging.rs:13), OCHRE: 0.72 (Multispeed Parameters.csv) |
| 4 | **Different default control mode** | Low | HARES: TwoSpeedSetpoint, OCHRE: Time |
| 5 | **HPXML per-stage SEER not parsed** | Low | hpxml.py - only composite SEER is read |

### Where HARES Physics is Better

1. **More control modes**: HARES supports Setpoint, Time, Alternating, and MultiSpeed modes vs OCHRE's two
2. **Explicit stage capacities**: HARES config allows explicit per-stage capacity specification
3. **PLF clamping**: HARES properly clamps PLF to min(0.7, PLR) per AHRI guidelines (staging.rs:310)
4. **Startup degradation**: HARES has explicit Winkler (2011) model implementation

### Where OCHRE Physics is Better Preserved

1. **Default capacity ratio**: OCHRE's 0.72 low-speed ratio is more realistic for typical dual-stage ACs
2. **Control mode default**: OCHRE's "Time" mode is more representative of typical thermostat behavior

---

## Recommendations

1. **Add SPEED_INDEX to AC telemetry** - Align with heat pump heater implementation
2. **Add runtime-by-stage tracking** - Add fields `low_stage_runtime_s` and `high_stage_runtime_s` to telemetry
3. **Consider updating default low-speed capacity fraction** - Change from 0.50 to 0.72 to match OCHRE defaults
4. **Consider changing default control mode** - From TwoSpeedSetpoint to TwoSpeedTime for OCHRE parity
5. **Document the differences** - If the differences are intentional, document in architecture decision record
