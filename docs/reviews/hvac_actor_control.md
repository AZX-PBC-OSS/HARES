# HVAC Actor Control Architecture Review

## Overview

This document reviews the HVAC Actor control architecture in HARES, comparing it against OCHRE's embedded thermostat approach. The review covers:
1. Actor architecture for HVAC control
2. Control signals sent to HVAC equipment
3. How OCHRE handles HVAC differently
4. HARES vs OCHRE architectural differences
5. How HVAC equipment receives actor control signals
6. Multi-stage equipment control

---

## 1. Actor Architecture: What Actors Control HVAC Equipment?

HARES separates thermostat and control decisions into distinct actors. The following actors control HVAC equipment:

### IdealThermostat Actor
**Location**: `crates/hares-core/src/actors/ideal_thermostat.rs`

The `IdealThermostat` actor represents **external user override** behavior - a person walking to the thermostat and changing the setpoint, putting it on hold, or a smart thermostat program pushing a schedule change.

**Key characteristics**:
- Pushes `ThermalSetpoint` overrides via the control surface
- Models user-driven thermostat changes (hold mode, away mode, DR pre-conditioning)
- Emits signals at `PriorityTier::UserOverride` priority (higher than Schedule)
- When no override is active, emits nothing - equipment uses its internal schedule

```rust
// From ideal_thermostat.rs
impl Actor for IdealThermostat {
    fn decide(&mut self, env: &EnvironmentState, out: &mut Vec<DispatchRequest>) {
        if !self.override_state.is_active() {
            return;  // No override = emit nothing, equipment uses internal schedule
        }
        out.push(DispatchRequest {
            target: self.dispatch_target.clone(),
            signal: ControlSignal::ThermalSetpoint {
                heating_setpoint_c: self.override_state.heating_setpoint_c,
                cooling_setpoint_c: self.override_state.cooling_setpoint_c,
                deadband_c: self.override_state.deadband_c,
            },
            priority: PriorityTier::UserOverride,
        });
    }
}
```

### DrCompliance Actor
**Location**: `crates/hares-core/src/actors/dr_compliance.rs`

The `DrCompliance` actor models **demand response compliance decisions** - whether an occupant complies with a DR event and what equipment actions to take.

**Key characteristics**:
- Receives DR signals (from schedule data or external control)
- Models occupant compliance decisions (AlwaysComply, NeverComply, Probabilistic)
- Dispatches control signals at `PriorityTier::Grid` priority (highest)
- Actions include: setpoint adjustments, load curtailment, power limits, turn-off

```rust
// DrAction options
pub enum DrAction {
    LoadCurtailment { fraction: f64 },    // Reduce load by fraction
    SetpointAdjust { delta_c: f64 },      // Adjust setpoint by delta
    AbsoluteSetpoint { heating_c, cooling_c },  // Absolute override
    TurnOff,                              // Turn equipment off
    PowerLimit { max_kw: f64 },           // Limit power draw
    None,
}
```

### Occupant Actor
**Location**: `crates/hares-core/src/actors/occupant.rs`

The `Occupant` actor models **occupant presence and behavior** - setting schedules, triggering equipment usage.

### BatteryManagementActor (BMS)
**Location**: `crates/hares-core/src/actors/bms.rs`

While primarily for batteries, the BMS can also dispatch control signals to HVAC for load coordination.

---

## 2. Control Signals: What Do Actors Send to HVAC Equipment?

HARES defines a comprehensive set of `ControlSignal` types in `crates/hares-types/src/control_signal.rs`:

### HVAC-Specific Control Signals

| Signal | Description | Equipment Application |
|--------|-------------|----------------------|
| `ThermalSetpoint` | Absolute setpoints (heating/cooling/deadband in C) | Sets thermostat setpoints |
| `ThermalSetpointDelta` | Relative adjustment from current setpoints | DR setpoint adjustments |
| `DutyCycle` | On/off fraction for cycling control | Non-ideal capacity equipment |
| `LoadFraction` | Load multiplier (0=off, 1=full) | Curtailment control |
| `ModeOverride` | Operating mode (On/Off/Standby/Heat/Cool/Auto) | Force equipment mode |
| `DemandResponse` | DR level (Normal/Moderate/High/Critical/GridEmergency) | DR event handling |
| `IdealCapacity` | Direct capacity in watts | Solver-provided load |
| `PowerLimit` | Maximum power draw in kW | Peak shaving |
| `HumiditySetpoint` | Target relative humidity | Dehumidifier control |

### Priority Tiers

Control signals are prioritized to resolve conflicts:

```rust
// From hares-control/src/dispatch.rs
pub enum PriorityTier {
    #[default]
    Schedule = 0,      // Default equipment schedules
    UserOverride = 1,  // User thermostat overrides
    Grid = 2,          // Grid/DR signals (highest for equipment)
    Safety = 3,        // Safety overrides (future use)
}
```

Signal resolution: higher priority overwrites lower priority for the same signal type targeting the same equipment.

---

## 3. OCHRE Comparison: How Does OCHRE Handle HVAC?

OCHRE embeds thermostat logic directly in HVAC equipment - there is no actor separation.

### OCHRE HVAC Architecture

From `vendors/OCHRE/ochre/Equipment/HVAC.py`:

```python
class HVAC(Equipment):
    def __init__(self, envelope_model=None, use_ideal_capacity=None, **kwargs):
        # Thermostat parameters embedded in equipment
        self.temp_setpoint = initial_setpoint
        self.temp_deadband = kwargs.get("Deadband Temperature (C)", 1)
        self.deadband_offset = kwargs.get("Deadband Offset (C)", 0.2)
        self.mode_prev = "Off"
    
    def run_thermostat_control(self, setpoint=None):
        # Thermostat FSM is internal to equipment
        temp_turn_on = setpoint - self.hvac_mult * self.temp_deadband * (1 - self.deadband_offset)
        temp_turn_off = setpoint + self.hvac_mult * self.temp_deadband * (self.deadband_offset)
        
        if self.hvac_mult * (self.zone.temperature - temp_turn_on) < 0:
            return "On"
        elif self.hvac_mult * (self.zone.temperature - temp_turn_off) > 0:
            return "Off"
        else:
            return None  # Maintain current mode
    
    def update_external_control(self, control_signal):
        # External control is optional override, not primary control
        ext_setpoint = control_signal.get("Setpoint")
        if ext_setpoint is not None:
            self.current_schedule[f"{self.end_use} Setpoint (C)"] = ext_setpoint
```

### Key OCHRE Characteristics

1. **Embedded Thermostat**: The thermostat FSM is part of the HVAC equipment class, not a separate actor
2. **Schedule-Driven**: Primary setpoints come from `self.current_schedule` (CSV/daily profile)
3. **External Control Optional**: External control signals modify but don't replace internal behavior
4. **Built-in DR Response**: Equipment has internal DR handling via `update_external_control`
5. **Speed Control Internal**: Two-speed control algorithms (Time, Setpoint, Time2) are in `DynamicHVAC` class

---

## 4. HARES vs OCHRE: Architectural Differences in Control Separation

| Aspect | HARES | OCHRE |
|--------|-------|-------|
| **Thermostat Location** | Separate `IdealThermostat` actor | Embedded in HVAC equipment class |
| **Control Philosophy** | Actor dispatches signals, equipment responds | Equipment has internal control logic |
| **Setpoint Source** | Actors push setpoints; equipment has internal schedules | Equipment reads from schedule; external is override |
| **DR Handling** | Separate `DrCompliance` actor dispatches signals | Built into HVAC equipment |
| **Speed Control** | Actor-level speed staging decisions | Embedded `run_two_speed_control` method |
| **Priority Resolution** | `PriorityTier` enum with explicit resolution | Implicit via schedule override |
| **Separation of Concerns** | Clear actor/equipment boundary | Monolithic HVAC class |

### HARES Advantages

1. **Explicit Control Hierarchy**: Priority tiers clearly define signal precedence
2. **Flexible Actor Composition**: Easy to add new actors (RL agents, occupancy predictors)
3. **Testable Actors**: `IdealThermostat` and `DrCompliance` are unit-testable in isolation
4. **Decoupled Evolution**: Thermostat logic can evolve independently from equipment physics

### OCHRE Advantages

1. **Simpler Single-Model**: No actor dispatch overhead
2. **Tighter Integration**: Thermostat and equipment physics share state directly
3. **Proven in Production**: Battle-tested in residential energy analysis

---

## 5. Equipment Reception: How Does HVAC Equipment Receive Actor Control Signals?

### Signal Flow

```
Actor.decide() → DispatchRequest (signal + target + priority)
                        ↓
              ControlDispatcher.resolve()
                        ↓
              Equipment.apply_control(signal)
                        ↓
              HvacEquipment.apply_control_signal()
```

### HVAC Equipment Implementation

**Location**: `crates/hares-equipment/src/hvac/hvac_core.rs`

```rust
pub fn apply_control_signal(&mut self, signal: &ControlSignal) {
    match signal {
        ControlSignal::ThermalSetpoint {
            heating_setpoint_c,
            cooling_setpoint_c,
            ..
        } => {
            self.runtime_setpoints = Some(RuntimeSetpointOverride {
                heating_c: *heating_setpoint_c,
                cooling_c: *cooling_setpoint_c,
            });
        }
        ControlSignal::ThermalSetpointDelta {
            heating_delta_c,
            cooling_delta_c,
        } => {
            // Anchor to base (static + schedule), not compounding
            let base = self.static_setpoints
                .with_schedule_override(self.schedule_setpoints);
            let prior = self.runtime_setpoints.unwrap_or_default();
            self.runtime_setpoints = Some(RuntimeSetpointOverride {
                heating_c: heating_delta_c.map(|d| base.heating_c + d).or(prior.heating_c),
                cooling_c: cooling_delta_c.map(|d| base.cooling_c + d).or(prior.cooling_c),
            });
        }
        _ => {} // Other signals handled elsewhere
    }
}
```

### Setpoint Resolution Chain

Equipment resolves effective setpoints from multiple sources (in priority order):

```rust
pub fn effective_setpoints(&self) -> ThermalSetpoints {
    self.static_setpoints           // Base from config
        .with_schedule_override(self.schedule_setpoints)  // CSV/daily profile
        .with_control_override(self.runtime_setpoints)   // Actor signals
}
```

### AC Implementation Example

**Location**: `crates/hares-equipment/src/hvac/air_conditioner.rs`

```rust
fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
    match signal {
        ControlSignal::ThermalSetpoint { deadband_c, .. } => {
            self.hvac.apply_control_signal(signal);
            if let Some(db) = deadband_c {
                self.hvac.thermostat.hysteresis_c = *db;
            }
        }
        ControlSignal::DutyCycle { on_fraction, .. } => {
            self.ctrl_duty_cycle = on_fraction.clamp(0.0, 1.0);
        }
        ControlSignal::LoadFraction { fraction } => {
            self.ctrl_load_fraction = fraction.clamp(0.0, 1.0);
        }
        ControlSignal::DemandResponse { level, duration_s } => {
            self.apply_dr_level(*level);
            self.dr_duration_remaining_s = *duration_s;
        }
        // ... more signal handlers
    }
    Ok(())
}
```

---

## 6. Staging Control: How Do Actors Control Multi-Stage Equipment?

### Speed Control Modes

HARES defines speed control modes in `crates/hares-equipment/src/hvac/speed_control.rs`:

```rust
pub enum SpeedControlMode {
    SingleSpeed,              // On/off at rated capacity
    TwoSpeedSetpoint,         // Setpoint-based two-speed
    TwoSpeedTime,             // Time-based two-speed (OCHRE "Time")
    TwoSpeedAlternating,      // Time-based alternating (OCHRE "Time2")
    MultiSpeedInterpolated,   // Continuous inter-speed interpolation
    VariableSpeedIdeal,       // Ideal modulation
}
```

### Stage Selection

**Location**: `crates/hares-equipment/src/hvac/staging.rs`

```rust
pub fn select_speed(&mut self, load_fraction: f64) -> SpeedSelection {
    match self.speed_control_mode {
        SpeedControlMode::SingleSpeed => SpeedSelection {
            speed_index: 0,
            part_load_ratio: load_fraction,
            speed_frac: load_fraction,
        },
        SpeedControlMode::TwoSpeedSetpoint => self.select_two_speed_setpoint(load_fraction),
        SpeedControlMode::TwoSpeedTime => self.select_two_speed_time(load_fraction, zone_temp, is_heating),
        // ... etc
    }
}

// TwoSpeedSetpoint: high speed when load exceeds low_speed_capacity_fraction
fn select_two_speed_setpoint(&mut self, load_fraction: f64) -> SpeedSelection {
    let low_cap = self.low_speed_capacity_fraction.clamp(0.01, 0.999);
    let desired_index = if load_fraction > low_cap { 1 } else { 0 };
    
    // Apply min_time_per_speed_s lockout
    let locked = self.time_at_current_speed_s < self.min_time_per_speed_s
        && desired_index != self.last_speed_index;
    
    let speed_index = if locked { self.last_speed_index } else { desired_index };
    // ...
}
```

### DR Speed Disabling

HARES supports disabling speed stages for demand response:

```rust
pub fn set_disabled_speeds(&mut self, disabled: &[bool]) {
    // Update disabled_speeds vector
    // Recompute max_enabled_speed (highest non-disabled stage)
}

// Equipment uses max_enabled_speed to cap capacity during DR
pub fn update_capacity(&mut self) {
    let max_speed = self._max_enabled_speed;
    self.capacity_max = self.calculate_biquadratic_param(param="cap", speed_idx=max_speed);
}
```

### Current Actor Gap

**Note**: HARES currently does NOT have a dedicated actor that decides speed stage selection. The staging decision is made internally by the equipment based on:

1. Load fraction from solver feedback
2. Internal thermostat mode
3. DR level (speed disabling)

Future work could add a dedicated `StagingActor` that dispatches `LoadFraction` or speed-specific signals based on zone temperature dynamics.

---

## Summary: Key Architectural Differences

| Aspect | HARES | OCHRE |
|--------|-------|-------|
| **Thermostat** | `IdealThermostat` actor (separate) | `HVAC.run_thermostat_control()` (embedded) |
| **DR Response** | `DrCompliance` actor | `HVAC.update_external_control()` (internal) |
| **Control Signals** | Typed `ControlSignal` enum with priority tiers | Dictionary-based `control_signal` |
| **Speed Control** | Equipment internal (no actor yet) | `DynamicHVAC.run_two_speed_control()` (embedded) |
| **Setpoint Priority** | Explicit via `PriorityTier` | Implicit via schedule updates |
| **Testability** | Actors unit-testable | Equipment integration tests |

---

## References

- Actor trait: `crates/hares-core/src/actor.rs`
- IdealThermostat: `crates/hares-core/src/actors/ideal_thermostat.rs`
- DrCompliance: `crates/hares-core/src/actors/dr_compliance.rs`
- HVAC core: `crates/hares-equipment/src/hvac/hvac_core.rs`
- Thermostat: `crates/hares-equipment/src/hvac/thermostat.rs`
- Staging: `crates/hares-equipment/src/hvac/staging.rs`
- Speed control: `crates/hares-equipment/src/hvac/speed_control.rs`
- Control signal types: `crates/hares-types/src/control_signal.rs`
- Priority tiers: `crates/hares-control/src/dispatch.rs`
- OCHRE HVAC: `vendors/OCHRE/ochre/Equipment/HVAC.py`
