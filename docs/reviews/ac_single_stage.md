# Single-Stage Air Conditioner Configuration Review - HARES

**Reviewer**: Code Review Agent  
**Date**: 2025-03-31  
**Scope**: Single-stage (single-speed) central air conditioner equipment  
**Files Reviewed**:
- `crates/hares-equipment/src/hvac/air_conditioner.rs` - Main AC implementation
- `crates/hares-equipment/src/hvac/ac_config.rs` - AC telemetry and config helpers
- `crates/hares-equipment/src/hvac/cooling_config.rs` - Typed config structs
- `crates/hares-io/src/hpxml/resolve_hvac.rs` - HPXML resolution
- `vendors/OCHRE/ochre/Equipment/HVAC.py` - OCHRE HVAC implementation
- `vendors/OCHRE/ochre/utils/equipment.py` - OCHRE equipment defaults

---

## 1. HPXML Parsing

### 1.1 Parsed Attributes for Single-Stage AC

The HPXML parser in `resolve_hvac.rs` extracts the following attributes for central air conditioners:

| HPXML Attribute | HARES Field | Required? | Notes |
|-----------------|-------------|-----------|-------|
| `CoolingCapacity` | `capacity_w` | **Required** | Cooling capacity in watts |
| `SEER` (or `AnnualCoolingEfficiency` with units=SEER) | `eir` | **Required** | Converted: EIR = 3.412141633 / SEER |
| `CompressorType` | `number_of_speeds` | No | Defaults to 1 (single-stage) |
| `CoolingSensibleHeatFraction` | `shr` | No | Defaults to 0.75 |
| `FanPowerWatts` | `fan_power_w` | No | Fan power in watts |
| `FanPowerWattsPerCFM` | `fan_power_w_per_cfm` | No | Fan power per CFM |
| `FractionCoolLoadServed` | `fraction_load_served` | No | Fraction of cooling load |
| Cooling extension elements | `startup_cd`, `airflow_defect_ratio`, `cooling_airflow_cfm` | No | Various |

### 1.2 HPXML-to-Config Mapping

The mapping happens in `try_build_central_ac_config()` at `resolve_hvac.rs:698-772`:

```rust
let seer = seer_from_params(params)?;                    // SEER required
let eir = 3.412_141_633 / seer.max(1e-6);                // Convert SEER to EIR
let capacity_w = params.get("cooling_capacity_w")...;   // Required
let n_speeds = n_speeds_from_params(params);             // Defaults to 1
let shr = params.get("shr").and_then(Value::as_f64);    // Optional, defaults later
let startup_cd = params.get("startup_cd").and_then(Value::as_f64); // Optional
```

### 1.3 Missing/Issues

**Issue 1: No Default SEER** (Severity: High)
- If HPXML does not specify SEER, `seer_from_params()` returns `None` and the config builder returns `None`, causing equipment creation to fail
- OCHRE has default SEER values based on equipment age/type
- **Recommendation**: Add default SEER fallback (e.g., SEER 13 for central AC)

---

## 2. OCHRE Defaults

### 2.1 SEER Default

OCHRE does **not** have a hardcoded default SEER in its HVAC model - SEER is always derived from the HPXML input. The SEER-to-EIR conversion is:
```
EIR = 1 / COP = 1 / (SEER * 0.293) = 3.412 / SEER
```

### 2.2 Startup Degradation Coefficient (c_d)

OCHRE calculates c_d in `utils/equipment.py:470-500`:

```python
def calc_c_d(is_heater, name, cop, number_of_speeds):
    if is_heater:  # cooling
        seer = convert(cop, "W", "Btu/hour")
        if name == "Room AC":
            c_d = 0.22
        elif number_of_speeds == 1:
            if seer < 13.0:
                c_d = 0.2       # Older, less efficient units
            else:
                c_d = 0.07      # Modern high-SEER units
        elif number_of_speeds == 2:
            c_d = 0.11
        else:
            c_d = 0.0          # Variable speed has no cycling penalty
    return c_d
```

For **single-stage AC**:
- SEER < 13: c_d = **0.2**
- SEER >= 13: c_d = **0.07**

### 2.3 SHR Default

OCHRE HVAC.py (lines 117-124):
```python
shr = kwargs.get("SHR (-)")
if shr is None:
    shr = 1  # Default to all sensible (no latent)
if isinstance(shr, list):
    shr_list = [shr[0]] + shr
else:
    shr_list = [0, shr]
```

OCHRE defaults SHR to **1.0** (all sensible, no latent) when not specified.

---

## 3. HARES Wiring

### 3.1 Config Structure

Single-stage AC uses `CentralAirConditionerConfig` with:
- `number_of_speeds: 1` → `SpeedControlMode::SingleSpeed`
- Single capacity in `cooling_capacities_w: Vec<f64>` (size 1)
- Single EIR in `eir_by_stage: Vec<f64>` (size 1)

### 3.2 Speed Control Mode Mapping

From `cooling_config.rs:115-122`:
```rust
pub fn cooling_speed_control_mode(&self) -> SpeedControlMode {
    match self.number_of_speeds {
        1 => SpeedControlMode::SingleSpeed,
        2 => SpeedControlMode::TwoSpeedSetpoint,
        4 => SpeedControlMode::VariableSpeedIdeal,
        _ => SpeedControlMode::SingleSpeed,
    }
}
```

### 3.3 Startup c_d Handling

**Issue 2: HARES Does Not Derive c_d Default for Single-Speed** (Severity: Medium)

From `cooling_config.rs:124-131`:
```rust
pub fn derived_cooling_startup_cd(&self) -> Option<f64> {
    self.startup_cd.or(match self.cooling_speed_control_mode() {
        SpeedControlMode::VariableSpeedIdeal => Some(0.0),
        SpeedControlMode::TwoSpeedSetpoint | ... => Some(0.11),
        SpeedControlMode::SingleSpeed | SpeedControlMode::MultiSpeedInterpolated => None,
    })
}
```

For single-speed (single-stage), HARES returns **None** - no startup degradation is applied by default.

**OCHRE applies** 0.07-0.2 depending on SEER.

**Impact**: HARES will overestimate capacity at startup for single-stage units. The startup degradation reduces effective capacity during the first ~5 minutes of operation.

### 3.4 Biquadratic Curves

Default curves in `ac_config.rs:13-20`:
```rust
pub(super) const DEFAULT_AC_CAPACITY_CURVE: [f64; 6] =
    [1.5509, -0.07505, 0.0031, 0.0024, -0.00005, -0.00043];
pub(super) const DEFAULT_AC_EIR_CURVE: [f64; 6] =
    [-0.30428, 0.11805, -0.00342, -0.00626, 0.0007, -0.00047];
```

These are used when HPXML doesn't provide biquadratic coefficients.

---

## 4. Control Logic

### 4.1 Thermostat Control

The `update_control()` method (`air_conditioner.rs:647-713`) implements:

1. **Mode Detection**: Uses thermostat FSM to determine Cooling/Deadband/Heating mode
2. **Load Fraction Calculation**: For single-speed, load_fraction = 1.0 (always full on)
3. **Duty Cycle**: 
   - Single-speed → always 1.0 when Cooling mode is active
   - Multi-stage → computed from zone temp vs setpoint + deadband

### 4.2 Single-Speed Behavior

```rust
let load_fraction = if self.hvac.speed_control_mode == SpeedControlMode::SingleSpeed {
    1.0  // Full capacity when running
} else {
    ((zone_temp - setpoint) / deadband).clamp(0.0, 1.0)
};
```

**Key Point**: Single-stage AC runs at either 0% (off) or 100% (full capacity). There is no modulation.

### 4.3 Ideal Capacity Support

HARES supports solver-driven ideal capacity at coarse timesteps (>= 300s):

```rust
self.use_ideal = self.hvac.use_ideal_capacity(env);
```

When `use_ideal` is true and `ideal_capacity_w` signal is received:
- For single-speed: PLR is derived from `ideal_capacity_w / steady_capacity_w`
- This allows the thermal solver to control AC output precisely

### 4.4 Demand Response

Supported via `apply_dr_level()`:
- **Normal**: No change
- **Moderate**: +1°C cooling setpoint offset
- **High**: +2°C offset, 80% load fraction
- **Critical**: +3°C offset, 50% load fraction
- **GridEmergency**: 0% load (equipment off)

---

## 5. Output Ports & Telemetry

### 5.1 Port Declarations

From `air_conditioner.rs:461-464`:
```rust
ports: vec![
    PortDeclaration::electrical(),
    PortDeclaration::thermal(zone),
],
```

### 5.2 Telemetry Fields

| Telemetry Key | Unit | Description | OCHRE Equivalent |
|---------------|------|-------------|------------------|
| `ELECTRIC_KW` | kW | Total electric: compressor + fan + crankcase | `Electric Power (kW)` |
| `SENSIBLE_COOLING_W` | W | Delivered sensible cooling (post-DSE) | `Sensible Cooling (W)` |
| `LATENT_COOLING_W` | W | Delivered latent cooling (post-DSE) | `Latent Cooling (W)` |
| `SHR` | - | Sensible heat ratio | `SHR (-)` |
| `OPERATING_MODE` | enum | 0=Off, 2=Cooling | `Mode` |
| `COP` | - | Coefficient of performance (excludes fan) | `COP (-)` |
| `RUNTIME_FRACTION` | - | RTF = PLR/PLF | `Runtime Fraction (-)` |
| `COMPRESSOR_KW` | kW | Compressor-only power | - |
| `FAN_KW` | kW | Supply fan power | `Fan Power (W)` |
| `SUPPLY_TEMP_C` | °C | Supply air temp leaving coil | - |
| `APPARATUS_DEW_POINT_C` | °C | Coil ADP temperature | - |
| `BYPASS_FACTOR` | - | Coil bypass factor | - |

### 5.3 Core Output

```rust
CoreOutput {
    flows: CoreFlows {
        electric_kw: Some(ElectricPower::Consumption(electric_kw.max(0.0))),
        reactive_power_kvar: None,
        fuel_w: None,
    },
    state: CoreState {
        operating_mode: Some(self.operating_mode),
        soc: None,
    },
}
```

---

## 6. Dwelling/Thermal Solver Integration

### 6.1 Duct Distribution System Efficiency (DSE)

DSE is computed via ASHRAE 152 in `compute_duct_config()`:
- Duct losses are distributed to duct zone (attic/garage/crawlspace)
- Remaining heat goes to conditioned zone

```rust
self.hvac.write_zone_thermal_contributions(
    ports,
    -sensible_cooling_w + fan_heat_w,  // Fan waste heat offset
    -latent_cooling_w,
    ThermalCategory::HvacCooling,
)?;
```

### 6.2 Latent/Sensible Split

The coil physics model calculates SHR from:
- Entering dry-bulb temperature
- Entering wet-bulb temperature (from zone humidity ratio)
- Coil airflow
- Rated SHR

The `calculate_shr()` function in `coil_physics.rs` performs the psychrometric calculation.

### 6.3 Latent Degradation at Part Load

HARES implements Henderson-Rengarajan latent degradation model (`coil_physics.rs`):
- Active when `latent_degradation.is_active()` (all params > 0)
- **Issue 3**: Latent degradation params are not populated by default - model is disabled
- This affects accuracy at low part-load ratios where cycling causes moisture re-evaporation

### 6.4 Ideal Capacity from Thermal Solver

The solver feedback actor dispatches `IdealCapacity` signals at coarse timesteps (>= 300s):

```rust
// In dwelling/step execution
// Step 1d: SolverFeedback emits IdealCapacity
// Step 2a: Equipment receives IdealCapacity in update_control
```

For single-stage AC receiving ideal capacity:
```rust
let plr = if self.use_ideal && self.ideal_capacity_w.abs() > f64::EPSILON {
    // Derive PLR from solver-provided capacity
    let p = (-self.ideal_capacity_w / steady_capacity_w.max(min_cap)).clamp(0.0, 1.0);
    self.hvac.duty_cycle = p;
    p
} else {
    self.hvac.duty_cycle.clamp(0.0, 1.0)
};
```

---

## 7. Summary of Issues

| Issue | Severity | Description |
|-------|----------|-------------|
| 1 | High | No default SEER when not in HPXML - equipment creation fails |
| 2 | Medium | No c_d default for single-speed AC - overestimates startup capacity |
| 3 | Low | Latent degradation model params not populated - disabled by default |
| 4 | Low | OCHRE defaults SHR=1.0, HARES defaults SHR=0.75 - HARES is more realistic |

---

## 8. Recommendations

1. **Add SEER default**: Add fallback SEER = 13.0 when not specified in HPXML
2. **Add c_d default for single-speed**: Derive from SEER (0.07 for SEER>=13, 0.2 for SEER<13)
3. **Consider latent degradation defaults**: Populate default params for residential AC
4. **Document SHR behavior**: Clarify that HARES uses 0.75 vs OCHRE's 1.0 default
