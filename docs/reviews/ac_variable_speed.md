# Variable-Speed (Inverter-Driven) Air Conditioner Review

## Executive Summary

This review examines the HARES implementation of variable-speed (inverter-driven) air conditioners, comparing against the OCHRE reference implementation to document preserved physics, identify gaps, and flag areas of concern.

**Key Finding**: HARES preserves most of OCHRE's variable-speed AC physics including the 4-speed model, biquadratic curves, ideal capacity algorithm, and stage interpolation. However, there is a significant **missing feature**: inverter efficiency (losses in the variable-speed drive) is not modeled.

---

## 1. HPXML Parsing

### What HPXML Attributes Are Parsed for Variable-Speed AC

**Location**: `crates/hares-io/src/hpxml/resolve_hvac.rs`

| HPXML Attribute | Parsed | Notes |
|-----------------|--------|-------|
| `SEER` / `SEER2` | ✅ | Converted to EIR via `3.412141633 / SEER`. SEER2 converted to SEER via 1/0.95 factor. |
| `AnnualCoolingEfficiency` (SEER/EER) | ✅ | Supports both HPXML 3.0 bare `<SEER>` and 4.x path with Units |
| `CoolingCapacity` | ✅ | Capacity in Btu/h → W |
| `CompressorType` | ✅ | `"variable speed"` → `number_of_speeds = 4` |
| Number of speeds fallback | ✅ | SEER > 21 → 4 speeds, 15 < SEER ≤ 21 → 2 speeds, SEER ≤ 15 → 1 speed |
| `stage_capacities_w` | ✅ | Via `extract_stage_values(params, "cooling_capacity_w_stage")` |
| `stage_eirs` | ✅ | Via `extract_stage_values(params, "cooling_eir_stage")` |
| `stage_shrs` | ✅ | Via `extract_stage_values(params, "shr")` |
| `FanPowerWattsPerCFM` | ✅ | Converted to W/m³/s for fan power calculation |
| `FanPowerWatts` | ✅ | Direct fan power in watts |
| `SensibleHeatFraction` | ✅ | SHR for non-heat pump ACs |
| `CoolingSensibleHeatFraction` | ✅ | SHR for heat pump cooling |
| EER (Room AC) | ✅ | Room ACs use EER, not SEER (correctly preserved from OCHRE) |

### Configuration Struct

**Location**: `crates/hares-equipment/src/hvac/cooling_config.rs`

```rust
pub struct CentralAirConditionerConfig {
    pub capacity_w: f64,           // Required
    pub eir: f64,                  // Required (1/COP)
    pub shr: Option<f64>,          // Sensible heat ratio at rated conditions
    pub number_of_speeds: u8,      // 1, 2, or 4 for variable-speed
    pub stage_capacities_w: Option<Vec<f64>>,
    pub stage_eirs: Option<Vec<f64>>,
    pub stage_shrs: Option<Vec<f64>>,
    pub fan_power_w: Option<f64>,
    pub fan_power_w_per_cfm: Option<f64>,
    pub startup_cd: Option<f64>,   // Capacity degradation coefficient
    // ... curve bounds, duct config, etc.
}
```

---

## 2. OCHRE Defaults Comparison

### Stage Capacity and EIR Defaults

**Location**: `defaults/HVAC Multispeed Parameters.csv` (identical to OCHRE)

HARES uses the same multispeed parameter CSV as OCHRE, which is the authoritative source for default stage ratios:

| Efficiency | Speeds | Stage 1 Ratio | Stage 2 Ratio | Stage 3 Ratio | Stage 4 Ratio |
|------------|--------|---------------|---------------|---------------|---------------|
| 20 SEER (Central AC) | 4 | 0.36 | 0.51 | 0.67 | 1.0 |
| 22 SEER (Central AC) | 4 | 0.36 | 0.51 | 0.67 | 1.0 |
| 24 SEER (Central AC) | 4 | 0.36 | 0.51 | 0.67 | 1.0 |
| 13 SEER (MSHP) | 4 | 0.489 | 0.667 | 0.844 | 1.2 |
| 16 SEER (MSHP) | 4 | 0.489 | 0.667 | 0.844 | 1.2 |

**COP by Stage** (for 22 SEER central AC): Stage 1 COP = 5.93, Stage 2 COP = 6.15, Stage 3 COP = 5.98, Stage 4 COP = 5.54

### Fan Power Defaults

| Parameter | HARES Default | OCHRE Default | Notes |
|-----------|---------------|---------------|-------|
| Fan power per CFM | 0.365 W/CFM | 0.365 W/CFM | ✅ Preserved |
| Calculation | `fan_power_w / airflow_m3_s` | Same | ✅ Preserved |

**Location of default**: `crates/hares-equipment/src/hvac/hvac_core.rs`
```rust
const DEFAULT_FAN_POWER_W_PER_CFM: f64 = 0.365;
const DEFAULT_FAN_POWER_W_PER_M3_S: f64 = DEFAULT_FAN_POWER_W_PER_CFM * CFM_PER_M3_S;
```

### Startup Capacity Degradation (Cd)

| Speed Control Mode | HARES Default Cd | OCHRE Default Cd |
|--------------------|------------------|------------------|
| VariableSpeedIdeal | 0.0 | 0.0 ✅ |
| TwoSpeedSetpoint | 0.11 | 0.11 ✅ |
| SingleSpeed | From SEER table | Same ✅ |

**Note**: Variable-speed defaults to Cd = 0.0 (no startup degradation), unlike single-speed which uses SEER-based defaults.

---

## 3. HARES Wiring: HPXML to Equipment

**Location**: `crates/hares-io/src/hpxml/resolve_hvac.rs:733-771`

The wiring flow:
1. **HPXML parsing** → Extract SEER, capacity, compressor type, fan power
2. **Number of speeds determination** → From CompressorType or SEER fallback
3. **Stage values extraction** → From params (or defaults if not in HPXML)
4. **Config struct creation** → `CentralAirConditionerConfig` with all parameters
5. **Equipment initialization** → `AirConditioner::new(config)`

Key code path:
```rust
fn try_build_central_ac_config(...) -> Option<EquipmentConfig> {
    let seer = seer_from_params(params)?;
    let eir = 3.412_141_633 / seer.max(1e-6);
    let n_speeds = n_speeds_from_params(params);
    // ...
    let cfg = CentralAirConditionerConfig {
        number_of_speeds: n_speeds,
        stage_capacities_w: extract_stage_values(params, "cooling_capacity_w_stage"),
        stage_eirs: extract_stage_values(params, "cooling_eir_stage"),
        // ...
    };
}
```

---

## 4. Control Logic

### Speed Modulation

**Location**: `crates/hares-equipment/src/hvac/air_conditioner.rs:333-431`

HARES implements three speed control modes:

| Mode | Triggered By | Behavior |
|------|--------------|----------|
| `SingleSpeed` | `number_of_speeds == 1` | On/off with runtime fraction |
| `TwoSpeedSetpoint` | `number_of_speeds == 2` | Low/high based on deadband |
| `VariableSpeedIdeal` | `number_of_speeds == 4` | Continuous modulation via ideal capacity |

### Variable Speed Selection Algorithm

```rust
fn select_variable_speed_cooling(&mut self, requested_capacity_fraction: f64) -> SpeedSelection {
    // 1. Normalize requested capacity to [0, 1]
    // 2. Compare against stage capacity fractions
    // 3. If below first stage → run at stage 1 with PLR < 1
    // 4. If above last stage → run at max stage (PLR = 1)
    // 5. Otherwise → interpolate between adjacent stages
}
```

### Capacity Curve and Interpolation

**Location**: `crates/hares-equipment/src/hvac/air_conditioner.rs:397-431`

When interpolating between stages:
- **Capacity**: Linear interpolation of `stage_capacities_w` based on `speed_frac`
- **EIR**: Linear interpolation of `stage_eirs` based on `speed_frac`
- **Part Load Ratio**: Set to 1.0 unless running below lowest stage

```rust
fn variable_speed_point(&self, selection: SpeedSelection) -> (f64, f64, f64) {
    let stage_capacity_w = self.hvac.interpolated_capacity(...);
    let stage_eir = self.hvac.interpolated_eir(selection.speed_index, selection.speed_frac);
    let part_load_ratio = if single_stage || below_lowest_stage { ... } else { 1.0 };
    (stage_capacity_w, stage_eir, part_load_ratio)
}
```

---

## 5. Output Ports & Telemetry

### Telemetry Fields

**Location**: `crates/hares-equipment/src/hvac/ac_config.rs:39-102`

| Telemetry Key | Unit | Description |
|---------------|------|-------------|
| `electric_kw` | kW | Total: compressor + fan + crankcase |
| `compressor_kw` | kW | Compressor-only power |
| `fan_kw` | kW | Supply fan power |
| `sensible_cooling_w` | W | Delivered sensible cooling |
| `latent_cooling_w` | W | Delivered latent cooling |
| `shr` | - | Sensible heat ratio |
| `cop` | - | Coefficient of performance (AHRI convention: excludes fan) |
| `runtime_fraction` | - | PLR/PLF [0..1] |
| `operating_mode` | enum | 0=Off, 2=Cooling |
| `speed_index` | - | Current speed index (0 = off, 1-4 = stages) |
| `supply_temp_c` | °C | Supply air temperature leaving coil |
| `apparatus_dew_point_c` | °C | ADP at coil |
| `bypass_factor` | - | Coil bypass factor |

### Missing Telemetry

| Key | Status | Notes |
|-----|--------|-------|
| `modulation` or `speed_frac` | ❌ Missing | `speed_index` is integer, fractional speed not exposed |
| `inverter_efficiency` | ❌ Missing | Not modeled (see Issue #1) |
| `part_load_cop` | ⚠️ Partial | `cop` is reported but not distinguished by PLR |

---

## 6. Dwelling/Thermal Solver Integration

### Ideal Capacity Algorithm

**Location**: `crates/hares-core/src/actors/solver_feedback.rs`

Variable-speed AC uses the **ideal capacity** mode, where the thermal solver calculates the exact capacity needed to maintain setpoint, then the equipment maps this to a speed selection.

```
Solver → IdealCapacity signal → Equipment:
1. Solver Feedback computes ideal_capacity_w to maintain setpoint
2. AC receives IdealCapacity(capacity_w) control signal
3. AC calculates requested_capacity_fraction = capacity_w / max_capacity
4. AC selects speed using select_variable_speed_cooling()
5. AC runs at selected speed, possibly with fractional duty cycle
```

### Use Ideal Capacity Condition

**Location**: `crates/hares-equipment/src/hvac/hvac_core.rs:642-665`

```rust
pub fn use_ideal_capacity(&self, env: &EnvironmentState) -> bool {
    // Variable-speed AC always uses ideal capacity
    if matches!(self.speed_control_mode, SpeedControlMode::VariableSpeedIdeal) {
        return true;
    }
    // Or coarse timestep (>5 min) with supported equipment
    let coarse_auto = env.time_res >= 300;  // 5 minutes
    ...
}
```

### Thermal Solver Interaction

- Variable-speed AC always uses ideal capacity algorithm (time_res ≥ 5 min OR explicit ideal mode)
- Capacity is solved to meet setpoint exactly
- If capacity < min stage capacity → run at low stage with PLR < 1
- If capacity > max stage → run at max stage (setpoint may not be met)

---

## Issues and Concerns

### Issue #1: Missing Inverter Efficiency Model

**Severity**: High

**Description**: Variable-speed (inverter-driven) air conditioners incur electrical losses in the inverter/DC compressor motor system. These losses are NOT modeled in HARES.

**OCHRE Reference**: OCHRE's variable-speed AC does not explicitly model inverter efficiency either - it's implicitly part of the EIR. However, modern high-efficiency inverters have ~95-97% efficiency that should be accounted for separately.

**Impact**: 
- Overestimation of efficiency at low speeds (inverter losses are a larger fraction of total power at partial load)
- Incorrect peak power calculations at low modulation
- Inaccurate energy consumption modeling for variable-speed systems

**Recommendation**: Add `inverter_efficiency` field to `CentralAirConditionerConfig` with default of 0.95-0.97, and apply as multiplicative loss to compressor power.

---

### Issue #2: Fan Power Curve Not Speed-Dependent

**Severity**: Medium

**Description**: HARES models fan power as linear with airflow (`fan_power_w_per_m3_s * flow_m3_s`), which is correct. However, the default fan power curve does not vary by speed stage.

**OCHRE Reference**: OCHRE uses per-stage fan power from multispeed CSV (Air Flow Ratio per stage).

**Current HARES behavior**: Single fan power ratio applied uniformly across all speeds.

**Impact**: 
- Over/under estimation of fan power at low speeds
- Incorrect total efficiency calculation at part-load

**Status**: This is partially addressed - HARES has `fan_power_w_per_m3_s` which gets applied to actual flow. But the multispeed CSV stage flow ratios aren't being used for fan power scaling.

---

### Issue #3: No Telemetry for Speed Fraction

**Severity**: Low

**Description**: The `speed_index` telemetry is only exposed as an integer (0=off, 1-4=stage), but variable-speed operates with fractional speed between stages.

**Impact**: Cannot observe how much the system is modulating within a stage.

**Recommendation**: Add `speed_frac` telemetry field.

---

### Issue #4: Biquadratic Curves Not Stage-Specific

**Severity**: Low

**Description**: HARES uses one set of biquadratic curves (capacity and EIR) for all stages, with stage-specific modifications via capacity ratios. However, in reality, each stage has different performance curves.

**OCHRE Reference**: OCHRE also uses a single curve set, but properly applies stage-specific corrections.

**Current HARES behavior**: The `DEFAULT_AC_CAPACITY_CURVE` and `DEFAULT_AC_EIR_CURVE` are single curves applied uniformly.

**Impact**: Secondary - the stage ratios provide most of the variation; biquadratic provides temperature-based degradation.

---

## Summary Table

| Aspect | HARES | OCHRE | Status |
|--------|-------|-------|--------|
| Number of speeds (4 for variable) | ✅ | ✅ | Preserved |
| Stage capacity ratios | ✅ | ✅ | Preserved |
| Stage EIR ratios | ✅ | ✅ | Preserved |
| Stage SHR values | ✅ | ✅ | Preserved |
| Biquadratic curves | ✅ | ✅ | Preserved |
| Fan power (W/CFM default) | ✅ | ✅ | Preserved |
| Ideal capacity algorithm | ✅ | ✅ | Preserved |
| Speed interpolation | ✅ | ✅ | Preserved |
| Startup Cd defaults | ✅ | ✅ | Preserved |
| Telemetry (speed_index) | ✅ | ✅ | Preserved |
| **Inverter efficiency** | ❌ | ❌ | **Missing (not in OCHRE either)** |
| Per-stage fan power curves | ⚠️ | ✅ | Partial |
| Speed fraction telemetry | ❌ | ❌ | Missing |

---

## Conclusion

The HARES variable-speed AC implementation **successfully preserves OCHRE physics** for the core modeling approach including:
- 4-speed stage model
- Capacity and EIR curves 
- Ideal capacity thermal solver integration
- Stage interpolation for continuous modulation

The primary gap is the lack of inverter efficiency modeling, which affects part-load efficiency accuracy. This is a known limitation in both HARES and OCHRE, but should be addressed for high-fidelity variable-speed AC simulation.

The multispeed parameter defaults file is identical between HARES and OCHRE, ensuring consistent default behavior.
