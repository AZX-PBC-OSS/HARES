# Dual-Stage Air-Source Heat Pump (ASHP) Heating Review - HARES

**Review Date**: March 31, 2026  
**Reviewer**: Code Reviewer  
**Scope**: Dual-Stage ASHP Heating Configuration in HARES

---

## Executive Summary

This review analyzes the Dual-Stage Air-Source Heat Pump (ASHP) heating implementation in HARES, comparing against OCHRE's implementation and assessing completeness of HPXML parsing, default/fallback values, wiring, control logic, telemetry, and thermal solver integration with staged capacity.

**Overall Assessment**: HARES provides a solid dual-stage ASHP implementation with comprehensive HPXML support, proper staging control logic matching OCHRE behavior, and good telemetry. The implementation correctly handles two-speed compressor staging with ThreeSpeedTime control and integrates with the thermal solver for ideal capacity mode. There are minor gaps in runtime telemetry by stage and some default differences vs OCHRE.

---

## 1. HPXML Parsing

### Files Analyzed
- `/home/rich/src/HARES/crates/hares-io/src/hpxml/resolve_hvac.rs` - HVAC resolution logic
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/heat_pump_config.rs` - Typed config struct
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/heat_pump/heater.rs` - Equipment implementation

### HPXML Attributes Parsed for Dual-Stage ASHP

| Attribute | Config Field | Notes |
|-----------|--------------|-------|
| `number_of_speeds` | `number_of_speeds` | From HPXML; defaults to 1 |
| `heating_capacity_w_stage_0` | `stage_heating_capacities_w[0]` | Stage 1 capacity via extract_stage_values() |
| `heating_capacity_w_stage_1` | `stage_heating_capacities_w[1]` | Stage 2 capacity |
| `heating_eir_stage_0` | `stage_heating_eirs[0]` | Stage 1 EIR |
| `heating_eir_stage_1` | `stage_heating_eirs[1]` | Stage 2 EIR |
| `CompressorType` | `number_of_speeds` | "two stage" → n_speeds=2 |
| `hp_lockout_temp_c` | `hp_lockout_temp_c` | HP compressor lockout temperature |
| `er_lockout_temp_c` | `er_lockout_temp_c` | Backup/ER lockout temperature |
| `BackupHeatingCapacity` | `backup_capacity_w` | Backup heating capacity |
| `BackupAnnualHeatingEfficiency` | `backup_eir` | Backup efficiency → EIR |

### Stage Capacity Extraction

The `extract_stage_values()` function (resolve_hvac.rs:440-450) iterates up to 32 stages looking for keys like:
- `heating_capacity_w_stage_0`, `heating_capacity_w_stage_1`, ...
- `heating_eir_stage_0`, `heating_eir_stage_1`, ...

### Findings

**Strengths**:
- Stage capacities and EIRs properly extracted from HPXML extension params
- Lockout temperatures (HP and ER) correctly parsed and wired
- Backup heating parameters properly handled

**Gaps/Issues**:
1. **Issue (Low Severity)**: HPXML doesn't natively contain per-stage capacities - only `CompressorType: two stage` and a single heating capacity. HARES relies on extension parameters (`heating_capacity_w_stage_0`, `heating_capacity_w_stage_1`) to get explicit stage values. Without these, HARES uses single capacity for both stages.

2. **Observation**: The HPXML parser defaults to `number_of_speeds=1` when not specified, but HPXML's `CompressorType` enum ("single stage", "two stage", "variable speed") isn't directly mapped to this field - it requires additional processing.

---

## 2. OCHRE Defaults

### Default Values in HARES

| Parameter | HARES Default | OCHRE Default | Source |
|-----------|---------------|---------------|--------|
| `hp_lockout_temp_c` | -17.78°C (0°F) | -17.78°C (0°F) | constants.rs:34 |
| `er_lockout_temp_c` | 4.44°C (40°F) | 4.44°C (40°F) | constants.rs:38 |
| `min_time_per_speed_s` | 300s (5 min) | 5 min (each stage) | hvac_core.rs:290 |
| `low_speed_capacity_fraction` | 0.5 (50%) | 0.5 (50%) | staging.rs:13 |
| `plf_degradation_coeff` | 0.25 | 0.25 (AHRI default) | staging.rs:17 |
| `default_heating_capacity_w` | 10,000 W | N/A | constants.rs:55 |
| `default_heating_eir` | 0.35 (COP ~2.86) | N/A | constants.rs:56 |

### Stage Transition Logic (OCHRE Reference)

OCHRE HVAC.py:859-916 implements three two-speed control types:
1. **Time-based**: High speed activates if temperature continues moving wrong direction after min_time expires
2. **Setpoint-based**: Uses overlapping deadband to determine high speed call
3. **Time2** (legacy): Always uses high speed when on

HARES uses `SpeedControlMode::TwoSpeedTime` (staging.rs:88-90), matching OCHRE's Time-based control.

### Findings

**Preserved from OCHRE**:
- Lockout temperatures match OCHRE defaults
- Low speed capacity fraction (50%) matches
- PLF degradation coefficient (0.25) matches AHRI 210/240 default

**Differences**:
1. **Minor**: OCHRE allows separate min time per stage ("Minimum Low Time (minutes)", "Minimum High Time (minutes)" - both default to 5). HARES uses a single `min_time_per_speed_s` for both stages - this is functionally equivalent for typical 2-speed equipment.

---

## 3. HARES Wiring: HPXML to Equipment

### Configuration Flow

```
HPXML XML
    ↓
resolve_hvac.rs: try_build_heat_pump_heater_config()
    ↓ (extract_stage_values)
HeatPumpHeaterConfig {
    stage_heating_capacities_w: Option<Vec<f64>>,
    stage_heating_eirs: Option<Vec<f64>>,
    number_of_speeds: u8,
    hp_lockout_temp_c,
    er_lockout_temp_c,
    backup_capacity_w, backup_eir
}
    ↓
EquipmentConfig::from_typed()
    ↓
ASHPHeater::init_from_typed() (heater.rs:417-543)
    ↓
hvac.heating_capacities_w = stage_heating_capacities_w or [single capacity]
hvac.eir_by_stage = stage_heating_eirs or [single eir]
hvac.speed_control_mode = TwoSpeedTime (for n_speeds=2)
```

### Key Wiring Logic (heater.rs:425-477)

```rust
// Stage capacities: use explicit stages or fall back to single capacity
self.hvac.heating_capacities_w = if let Some(stages) = &cfg.stage_heating_capacities_w {
    stages.clone()
} else if let Some(cap) = cfg.heating_capacity_w {
    vec![cap]  // Single stage fallback
} else {
    vec![DEFAULT_HEATING_CAPACITY_W]
};

// Stage EIRs: use explicit stages or fall back to single EIR
self.hvac.eir_by_stage = if let Some(stages) = &cfg.stage_heating_eirs {
    stages.clone()
} else {
    vec![default_eir]
};

// Speed control mode based on number_of_speeds
self.hvac.speed_control_mode = match cfg.number_of_speeds {
    0 | 1 => SpeedControlMode::SingleSpeed,
    2 => SpeedControlMode::TwoSpeedTime,
    _ => SpeedControlMode::MultiSpeedInterpolated,
};
```

### Findings

**Strengths**:
- Clean configuration flow from HPXML → Config → Equipment
- Proper fallback handling when stage values not provided
- Correct speed control mode assignment based on n_speeds

---

## 4. Control Logic: Dual-Stage Staging

### Stage Selection Algorithm (staging.rs:88-90, 152-202)

HARES uses `TwoSpeedTime` mode with the following logic:

1. **Initial State**: Fresh cycle (no previous zone temp) starts at low speed (stage 0)
2. **Load Calculation**:
   - `load_ratio = (setpoint - zone_temp) / deadband` clamped to [0, 1]
3. **Speed Decision** (after min_time_per_speed_s):
   - If zone temp moving "wrong direction" (heating: temp dropping) → escalate to stage 1
   - Otherwise stay at current stage
4. **Minimum Time Guard**: Stage changes blocked until `min_time_per_speed_s` (300s) expires

### Control Flow (heater.rs:916-1016)

```
resolve_control():
    1. Update thermostat mode (Heating/Off/Deadband)
    2. Check ER hard lockout (setpoint raise triggers 600s lockout)
    3. Check ER soft lockout (zone rising = HP winning)
    4. Calculate load_ratio from setpoint - zone_temp - deadband
    5. Call hvac.select_speed_with_zone_temp(load_ratio, zone_temp, is_heating)
    6. Update HP availability (OAT >= hp_lockout_temp_c)
    7. Determine ER allowed: OAT < er_lockout_temp_c AND OAT <= max_oat_supplemental_c
    8. ER thermostat: zone_temp <= (setpoint - er_setpoint_offset_c)
    9. Return HeaterControl { hp_on, er_on, speed_index, duty_cycle }
```

### ER (Backup) Control Logic

- **Temperature-based lockout**: ER disabled when OAT >= er_lockout_temp_c (default 40°F/4.44°C)
- **Hard lockout**: 600s after user raises setpoint (prevents expensive ER during HP ramp-up)
- **Soft lockout**: ER stays off while zone temp is rising (HP winning the load)
- **Cycle constraint**: ER requires `min_er_cycle_time_s` off-time between cycles (default 0s)

### Findings

**Strengths**:
- TwoSpeedTime control correctly implements OCHRE behavior
- Proper staging with minimum time guard prevents speed hunting
- Sophisticated ER lockout logic (hard + soft) prevents premature backup engagement

**Preserved Physics**:
- Zone temperature direction detection for staging decision
- Defrost capacity/efficiency degradation at low OAT
- PLF (Part-Load Factor) degradation for cycling equipment

---

## 5. Output Ports & Telemetry

### Telemetry Fields (heater.rs:666-695)

| Telemetry Key | Description | Unit |
|---------------|-------------|------|
| `ELECTRIC_KW` | Total electric power (HP + ER + fan) | kW |
| `THERMAL_OUTPUT_W` | Delivered heating to zone (post-DSE) | W |
| `OPERATING_MODE` | Mode code: 0=Off, 3=HP, 4=HP+ER, 5=ER | enum |
| `SPEED_INDEX` | Current compressor stage (0 or 1) | index |
| `DEFROST_ACTIVE` | Defrost correction active | bool |
| `COP` | Coefficient of Performance (thermal/compressor) | - |
| `RUNTIME_FRACTION` | Duty cycle / part-load ratio | - |
| `COMPRESSOR_KW` | Compressor-only electric power | kW |
| `DEFROST_TIME_FRACTION` | Fraction of timestep in defrost | - |
| `HEATING_SETPOINT_C` | Active heating setpoint | °C |
| `COOLING_SETPOINT_C` | Active cooling setpoint from thermostat | °C |

### Gaps/Issues

1. **Issue (Medium Severity)**: No runtime tracking by stage. The telemetry reports `SPEED_INDEX` but doesn't track cumulative runtime at each stage (e.g., "stage 1 runtime hours", "stage 2 runtime hours"). This is useful for equipment maintenance scheduling and performance monitoring.

2. **Observation**: `RUNTIME_FRACTION` is the PLR/duty cycle, not specifically stage runtime.

---

## 6. Dwelling Integration: Thermal Solver with Staged Capacity

### Ideal Capacity Mode Integration

When `use_ideal_capacity` is true (coarse timestep >= 300s OR explicitly configured):

1. **Solver → Equipment**: Thermal solver provides `IDEAL_CAPACITY_W` signal (negative for heating)
2. **Equipment → Solver**: Equipment writes thermal output to zone via `PortContribution::Thermal`

### Staged Capacity Handling (heater.rs:726-782)

```rust
// In compute_step():
let (stage_capacity_w, stage_eir) = if MultiSpeedInterpolated {
    (
        hvac.interpolated_capacity(&heating_capacities_w, speed_index, speed_frac),
        hvac.interpolated_eir(speed_index, speed_frac),
    )
} else {
    (
        hvac.capacity_at_stage(&heating_capacities_w, speed_index),
        hvac.eir_at_stage(speed_index),
    )
};

// Biquadratic correction
let (_, cap_ratio) = hvac.evaluate_biquadratic_with_flow(...);
let steady_capacity_w = (stage_capacity_w * cap_ratio).max(0.0);

// PLR derivation from solver's ideal capacity
if use_ideal && ideal_capacity_w.abs() > EPSILON {
    let total_available = if er_on { steady_capacity_w + backup_capacity_w } else { steady_capacity_w };
    let plr = (ideal_capacity_w / total_available.max(min_cap)).clamp(0.0, 1.0);
    hvac.duty_cycle = plr;  // Write back for telemetry
}
```

### Thermal Zone Contribution

The equipment writes thermal output to the dwelling's thermal zone:
```rust
hvac.write_zone_thermal_contributions(
    ports,
    step.thermal_output_w,
    0.0,
    ThermalCategory::HvacHeating,
)?;
```

### Findings

**Strengths**:
- Ideal capacity mode works correctly with staged equipment
- PLR derived from solver load correctly distributes across stages
- Biquadratic capacity correction applies to each stage independently
- DSE (Duct System Efficiency) properly applied to thermal output

**Preserved Physics**:
- Staged capacity properly accounts for partial-load operation
- ER backup properly included in total available capacity calculation
- Startup capacity degradation (Winkler 2011) applies to each stage

---

## Summary of Findings

### Strengths (HARES Better than OCHRE)
1. **Typed Config Architecture**: Better than OCHRE's dictionary-based config - compile-time type checking, serialization
2. **Stage Handling**: Clean vector-based stage capacity/EIR storage vs OCHRE's list indexing
3. **Defrost Physics**: More detailed defrost model with capacity and power multipliers
4. **Explicit Stage Values**: Supports explicit per-stage capacities from HPXML extension params

### Issues to Address

| Priority | Issue | Description |
|----------|-------|-------------|
| Medium | Runtime by stage | No telemetry for cumulative runtime at each stage |
| Low | HPXML stage mapping | No direct mapping from `CompressorType` enum to `number_of_speeds` - requires extension params |
| Low | ER fuel type | `backup_fuel` field not used - ER always assumes electric |

### Comparison to OCHRE

| Aspect | HARES | OCHRE | Assessment |
|--------|-------|-------|------------|
| Lockout temps | ✓ Matches | Default | Good |
| Stage control | ✓ TwoSpeedTime | Time-based | Good |
| Min time per stage | 300s uniform | 5min each | Equivalent |
| Low speed cap frac | 50% | 50% | Good |
| PLF degradation | 0.25 | 0.25 | Good |
| Telemetry | SPEED_INDEX | speed_idx | Equivalent |

---

## Recommendations

1. **Add stage runtime telemetry**: Consider adding `stage_1_runtime_s` and `stage_2_runtime_s` cumulative telemetry fields for maintenance scheduling

2. **Document HPXML stage mapping**: Clarify in docs that explicit stage capacities require HPXML extension params like `heating_capacity_w_stage_0`, `heating_capacity_w_stage_1`

3. **Verify default ER lockout**: The default 4.44°C (40°F) ER lockout is aggressive - may want to make this configurable or document clearly

4. **Test two-speed staging**: Add integration test verifying stage escalation on sustained low temperature conditions with TwoSpeedTime control
