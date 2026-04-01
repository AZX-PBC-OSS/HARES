# OCHRE Ideal Capacity Mode Implementation Review

## Overview

Ideal Capacity Mode in OCHRE/HARES means the thermal solver provides the exact capacity needed to meet the load, rather than equipment cycling on/off. This is used for:
1. Coarse timesteps where equipment cycling would be too granular
2. Equipment where cycling losses are significant
3. When solver needs exact heat/cool delivery to maintain setpoint

---

## 1. Equipment Types Supporting Ideal Capacity Mode

### OCHRE Equipment

| Equipment Type | File Location | Ideal Capacity Support |
|----------------|---------------|----------------------|
| **HVAC Heating** (Heater classes) | `HVAC.py` lines 63-641 | Full support via `HVAC` base class |
| **HVAC Cooling** (Cooler classes) | `HVAC.py` lines 63-641 | Full support via `HVAC` base class |
| **Heat Pump Heater** | `HVAC.py` lines 1112-1166 | Full support with defrost handling |
| **ASHP Heater (with backup)** | `HVAC.py` lines 1176-1478 | Full support with ER backup |
| **Mini-split HVAC** | `HVAC.py` lines 1094-1500 | Full support (variable speed) |
| **Water Heater** | `WaterHeater.py` lines 28-250 | Full support |
| **Electric Resistance WH** | `WaterHeater.py` lines 339-423 | Full support with upper/lower elements |
| **Heat Pump WH** | `WaterHeater.py` lines 425-702 | Full support with HP+ER modes |
| **Tankless Water Heater** | `WaterHeater.py` lines 727-779 | Always uses ideal (uses `IdealWaterModel`) |

### HARES Equipment

| Equipment Type | File Location | Ideal Capacity Support |
|----------------|---------------|----------------------|
| **Ideal HVAC** | `ideal_hvac.rs` lines 30-654 | Dedicated ideal capacity equipment |
| **Central AC** | `air_conditioner.rs` lines 83-1286 | Full support via `ideal_capacity_w` |
| **Heat Pump Heater** | `heat_pump/heater.rs` lines 108-1287 | Full support via `ideal_capacity_w` |
| **Baseboard** | `baseboard.rs` line 211 | Simple ideal capacity via helper |
| **Boiler** | `boiler.rs` line 311 | Simple ideal capacity via helper |
| **Electric Furnace** | `furnace.rs` line 257 | Simple ideal capacity via helper |
| **Electric Resistance WH** | `water_heater/resistance.rs` lines 418-446 | Time-averaged duty cycle |
| **Heat Pump WH** | `water_heater/heat_pump_wh.rs` | Partial support via tank model |

### Key Difference
- **HARES** has a dedicated `IdealHvac` equipment type for simple use cases
- **OCHRE** implements ideal capacity as a mode within the HVAC base class

---

## 2. Configuration

### OCHRE Configuration

The default is determined automatically based on timestep and equipment:

```python
# HVAC.py lines 237-241
if use_ideal_capacity is None:
    use_ideal_capacity = self.time_res >= dt.timedelta(minutes=5) or self.n_speeds >= 4
self.use_ideal_capacity = use_ideal_capacity
```

```python
# WaterHeater.py lines 51-54
if use_ideal_capacity is None:
    use_ideal_capacity = self.time_res >= dt.timedelta(minutes=5)
self.use_ideal_capacity = use_ideal_capacity
```

**Configuration Parameters:**
- `use_ideal_capacity`: Can be explicitly set to `True`/`False`
- **Auto-trigger conditions:**
  - Timestep >= 5 minutes (300 seconds)
  - Variable speed equipment (4+ speeds)

### HARES Configuration

```rust
// ideal_hvac.rs lines 205-214
fn use_ideal_capacity(&self, env: &EnvironmentState) -> bool {
    match self.ideal_capacity_mode {
        IdealCapacityMode::On => true,
        IdealCapacityMode::Off => false,
        IdealCapacityMode::Auto => {
            let time_res_s = env.time_res.num_seconds();
            time_res_s >= IDEAL_CAPACITY_TIME_RES_THRESHOLD_S || self.is_variable_speed
        }
    }
}
```

**HARES Configuration Options** (`IdealCapacityModeConfig`):
- `"on"`: Always use ideal capacity
- `"off"`: Never use ideal capacity  
- `"auto"`: Auto-enable at coarse timesteps (>=300s) or variable speed

```rust
// hvac_core.rs line 21
pub const IDEAL_CAPACITY_TIME_RES_THRESHOLD_S: i64 = 300;
```

---

## 3. Solver Interface

### OCHRE Solver Interface

OCHRE uses the envelope model's `solve_for_inputs()` method:

```python
# HVAC.py lines 411-429
def solve_ideal_capacity(self):
    x_desired = self.temp_setpoint
    zone_idxs = [zone.h_idx for zone in self.zone_fractions]
    zone_ratios = list(self.zone_fractions.values())
    
    # Solve for heat needed to reach setpoint
    h_desired = self.envelope_model.solve_for_inputs(
        self.zone.t_idx, zone_idxs, x_desired, zone_ratios
    )  # in W
    
    # Account for fan power and SHR
    if self.is_heater:
        return h_desired / (self.shr + self.eir * self.fan_power_ratio)
    else:
        return -h_desired / (self.shr - self.eir * self.fan_power_ratio)
```

For water heaters:
```python
# WaterHeater.py lines 193-211
def solve_ideal_capacity(self):
    self.model.update_model()
    off_states = self.model.next_states
    
    set_states = np.ones(len(off_states)) * self.setpoint_temp
    h_desired = (
        np.dot(set_states[:self.t_lower_idx+1] - off_states[:self.t_lower_idx+1],
               self.model.capacitances[:self.t_lower_idx+1])
        / self._dt_seconds
    )
    
    duty_cycle = min(max(h_desired / self.capacity_rated, 0), 1)
    self.duty_cycle_by_mode = {"On": duty_cycle, "Off": 1 - duty_cycle}
```

### HARES Solver Interface

HARES separates solver computation from equipment via the `SolverFeedbackActor`:

```rust
// solver_feedback.rs lines 47-79
pub fn collect_and_solve(&mut self, equipment: &[Box<dyn Equipment>], solver: &ThermalSolver) {
    self.collect_with(equipment, |zone, target_c| {
        solver.solve_ideal_capacity_for_target(zone, target_c)
    });
}

fn collect_with(&self, equipment: &[Box<dyn Equipment>], solve: impl Fn(ZoneId, f64) -> f64) {
    for (idx, eq) in equipment.iter().enumerate() {
        if let Some((zone, target_c)) = eq.ideal_target() {
            let capacity_w = solve(zone, target_c);
            self.pending.push((idx, capacity_w));
        }
    }
}
```

**The ideal_target() Method:**

Equipment exposes ideal capacity needs via the `ideal_target()` trait method:

```rust
// ideal_hvac.rs lines 648-653
fn ideal_target(&self) -> Option<(ZoneId, f64)> {
    if self.mode == ThermostatMode::Deadband || !self.use_ideal_cached {
        return None;
    }
    Some((self.zone_id, self.current_target_c))
}
```

```rust
// air_conditioner.rs lines 273-274
fn ideal_target(&self) -> Option<(ZoneId, f64)> {
    self.core.ideal_target()
}
```

The solver computes capacity needed:

```rust
// hares-envelope/thermal_solver/stepping.rs lines 24-60
pub fn solve_ideal_capacity_for_target(&self, zone: ZoneId, target_c: f64) -> f64 {
    // Uses pre-computed LU decomposition to solve for required heat input
    // Returns positive for heating, negative for cooling
}
```

---

## 4. Control Logic

### OCHRE Control Logic

**HVAC Capacity Update:**

```python
# HVAC.py lines 431-456
def update_capacity(self):
    if self.use_ideal_capacity:
        self.capacity_ideal = self.solve_ideal_capacity()
        capacity = self.capacity_ideal
        
        # External capacity override
        if self.ext_capacity is not None:
            capacity = self.ext_capacity
        
        # Enforce min/max bounds
        if capacity < self.capacity_min:
            capacity = 0
        elif capacity > self.capacity_max * self.ext_capacity_frac:
            capacity = self.capacity_max * self.ext_capacity_frac
        
        self.speed_idx = capacity / self.capacity_max
        return capacity
    else:
        return self.capacity_list[self.speed_idx]
```

**External Control Signals:**

```python
# HVAC.py lines 255-320
def update_external_control(self, control_signal):
    # - Capacity: Sets HVAC capacity directly (ideal only)
    # - Max Capacity Fraction: Limits max capacity (ideal only)
    # - Duty Cycle: Forces on for fraction of timestep (non-ideal)
```

### HARES Control Logic

**Ideal HVAC Step:**

```rust
// ideal_hvac.rs lines 462-543
fn step(&mut self, _env: &EnvironmentState, _dt: Duration, ports: &mut PortSlots) -> Result<(), HaresError> {
    let capacity_w = match self.mode {
        ThermostatMode::Deadband => 0.0,
        ThermostatMode::Heating if self.use_ideal_cached => {
            (self.ideal_capacity_w * self.load_fraction)
                .max(0.0)
                .min(self.rated_capacity_w)
        }
        ThermostatMode::Cooling if self.use_ideal_cached => {
            (self.ideal_capacity_w * self.load_fraction)
                .min(0.0)
                .max(-self.cooling_capacity_w)
        }
        ThermostatMode::Heating => self.rated_capacity_w * self.load_fraction,
        ThermostatMode::Cooling => -self.cooling_capacity_w * self.load_fraction,
    };
    
    // Write to thermal port
    if capacity_w.abs() > 0.0 {
        ports.accumulate(&PortContribution::Thermal { ... })?;
    }
}
```

**Control Signal Handling:**

```rust
// ideal_hvac.rs lines 580-646
fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> Result<()> {
    match signal {
        ControlSignal::IdealCapacity { capacity_w } => {
            self.ideal_capacity_w = *capacity_w;
        }
        ControlSignal::ThermalSetpoint { ... } => { ... }
        ControlSignal::LoadFraction { fraction } => {
            self.load_fraction = fraction.clamp(0.0, 1.0);
        }
        ControlSignal::IdealCapacityModeOverride { mode } => {
            self.ideal_capacity_mode = *mode;
        }
        _ => {}
    }
    Ok(())
}
```

---

## 5. HARES Comparison

### Key Differences

| Aspect | OCHRE | HARES |
|--------|-------|-------|
| **Architecture** | Single monolithic equipment with ideal mode flag | Dedicated `IdealHvac` equipment + ideal mode support in equipment classes |
| **Solver Coupling** | Direct envelope model reference | `SolverFeedbackActor` with explicit solve step |
| **Water Heater** | Uses tank model `solve_for_input()` | Uses tank's `ideal_capacity_for_node()` method |
| **Capacity Bounds** | Respects `capacity_min` and `capacity_max` | Clamps to `rated_capacity_w` and `cooling_capacity_w` |
| **Variable Speed** | Auto-enables at n_speeds >= 4 | Auto-enables at n_speeds >= 4 in `HvacEquipment` |
| **Load Fraction** | Via `ext_capacity` and `ext_capacity_frac` | Via `load_fraction` field |

### Similarities

1. **Same threshold**: 5 minute (300 second) timestep triggers auto ideal mode
2. **Same auto-condition**: Variable speed equipment (4+ speeds) always uses ideal
3. **Solver approach**: Both use linear solve to compute required capacity for setpoint
4. **External control**: Both support direct capacity override signals
5. **Thermostat logic**: Both use deadband-based thermostat with min on/off times

### HARES Advantages

1. **Clean separation**: Dedicated equipment type for simple cases
2. **Explicit solver actor**: Clear data flow, easier to test
3. **Typed control signals**: `IdealCapacityMode` enum for configuration
4. **State management**: Serialization with `save_state()`/`load_state()`

### OCHRE Advantages

1. **Unified model**: All HVAC in single class hierarchy
2. **More equipment types**: Gas boiler, furnace, etc. all support ideal mode
3. **Dynamic biquadratic**: Capacity/EIR curves for real equipment modeling

---

## Summary

Both OCHRE and HARES implement ideal capacity mode with the same core principles:
- Solver back-computes exact capacity needed for setpoint
- Equipment delivers that capacity directly (no cycling)
- Auto-enabled at coarse timesteps or for variable speed equipment

The main architectural difference is HARES uses an explicit `SolverFeedbackActor` to decouple solver computation from equipment, while OCHRE directly calls the envelope model. Both support the same range of equipment types (HVAC heating/cooling, water heaters) with similar control logic.
