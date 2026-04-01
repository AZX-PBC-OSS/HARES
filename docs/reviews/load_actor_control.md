# Load (Appliance) Actor Control Architecture Review

**Reviewer**: Code Review Agent  
**Date**: 2026-03-31  
**Scope**: Actor control architecture, control signals, equipment reception, OCHRE comparison  
**Severity Key**: CRITICAL, HIGH, MEDIUM, LOW, INFO

---

## Executive Summary

This review documents the Load (Appliance) Actor control architecture in HARES and compares it to OCHRE's approach. Key findings:

1. **HARES uses a decoupled Actor-Equipment architecture** - Control decisions (scheduling, occupancy, demand response) are separated into distinct Actor implementations that emit `DispatchRequest`s to equipment.

2. **OCHRE embeds scheduling/diversity in load equipment** - The OCHRE `ScheduledLoad`, `EventBasedLoad`, and `WetAppliance` classes contain internal schedule handling and stochastic event generation.

3. **Control signal flow**: Actors emit signals → ControlDispatcher routes by priority → Equipment receives via `apply_control_unchecked()`

4. **Wet appliance + water heater coordination** uses the DHW_DEMAND_LOOP fluid port system for hot water draw coupling.

---

## 1. Actor Architecture for Load Control

### 1.1 Available Actors

HARES implements the following built-in actors (from `crates/hares-core/src/actors/mod.rs`):

| Actor | Purpose | Load Control Role |
|-------|---------|-------------------|
| **Occupant** | Models human behavior based on presence | Turns lights/appliances off when away, applies load fractions |
| **IdealThermostat** | HVAC setpoint control | Not directly used for appliance loads |
| **EvDriverActor** | EV charging scheduling | Controls EV charging loads only |
| **BatteryManagementActor** | Battery SOC management | Controls battery, not appliances |
| **DrCompliance** | Demand response compliance | Can apply DR signals to loads |
| **SolverFeedback** | Thermal solver coordination | Not load-specific |

### 1.2 Occupant Actor Details

The **Occupant actor** (`crates/hares-core/src/actors/occupant.rs`) is the primary load control actor:

```rust
pub struct Occupant {
    name: Arc<str>,
    presence_schedule: Vec<Presence>,  // Home/Away/Sleeping per timestep
    lighting: Option<(DispatchTarget, EquipmentBehavior)>,
    appliance: Option<(DispatchTarget, EquipmentBehavior)>,
    ev: Option<(DispatchTarget, EquipmentBehavior)>,
    plug_loads: Option<(DispatchTarget, EquipmentBehavior)>,
}
```

**Key behaviors** (`EquipmentBehavior`):
- `off_when_away`: Turn off equipment when occupant is away
- `on_when_home`: Turn on equipment when returning home  
- `power_setpoint_kw`: Override power setpoint when active
- `load_fraction`: Apply load fraction (0-1) when active

### 1.3 Scheduling Actor Gap

**Finding**: HARES lacks a dedicated scheduling actor for deterministic load scheduling.

| Issue | Severity | Description |
|-------|----------|-------------|
| **No ScheduleActor** | MEDIUM | Load schedules are baked into equipment at init time (`ScheduledLoad.power_schedule_source`). There is no actor that actively dispatches schedule-based control signals during simulation. |

**Contrast with OCHRE**: In OCHRE, scheduling is intrinsic to equipment (`ScheduledLoad.initialize_schedule()` reads schedule columns directly). HARES takes a more modular approach but lacks active schedule actors.

### 1.4 Demand Response Actor Gap

**Finding**: DR compliance exists as a module but not as a load-specific actor.

| Issue | Severity | Description |
|-------|----------|-------------|
| **No LoadDrActor** | MEDIUM | The `DrCompliance` enum (`dr_compliance.rs`) is used by HVAC and water heater equipment, but no dedicated actor exists to apply demand response signals to loads. |

---

## 2. Control Signals Sent to Load Equipment

### 2.1 Available Control Signals

From `hares_types::ControlSignal` (defined in types crate, used by hares_control):

| Signal | Applies to Loads | Description |
|--------|------------------|--------------|
| `LoadFraction { fraction: f64 }` | **Yes** | Scales power (0-1), applied to electric AND gas |
| `ModeOverride { mode: OperatingMode }` | **Yes** | Forces Off/Standby/On state |
| `PowerSetpoint { active_power_kw, reactive_power_kvar }` | **Yes** | Overwrites schedule-based power |
| `EventDelay { delay_s: f64 }` | **Yes (EventBasedLoad only)** | Delays event start when in Idle phase |
| `DemandResponse { level: DRLevel, duration_s }` | **No for loads** | Currently only HVAC and water heaters handle DR |
| `DutyCycle { on_fraction, period_s }` | **No for loads** | Not implemented for load equipment |

### 2.2 ScheduledLoad Control Signals

From `crates/hares-equipment/src/scheduled_load.rs` (lines 608-633):

```rust
fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
    match signal {
        ControlSignal::LoadFraction { fraction } => {
            self.load_fraction = *fraction;
            Ok(())
        }
        ControlSignal::ModeOverride { mode } => {
            self.mode_override = Some(*mode);
            Ok(())
        }
        ControlSignal::PowerSetpoint { active_power_kw, .. } => {
            self.power_setpoint_override = Some(active_power_kw.max(0.0));
            Ok(())
        }
        _ => Err(HaresError::Control(format!(
            "ScheduledLoad does not accept control signal: {signal:?}"
        ))),
    }
}
```

### 2.3 EventBasedLoad Control Signals

From `crates/hares-equipment/src/event_load.rs` (lines 645-683):

```rust
fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
    match signal {
        ControlSignal::LoadFraction { fraction } => {
            self.load_fraction = fraction.max(0.0);
        }
        ControlSignal::ModeOverride { mode } => {
            self.forced_mode = Some(mode_to_forced(*mode));
        }
        ControlSignal::PowerSetpoint { active_power_kw, .. } => {
            self.power_setpoint_override = Some(active_power_kw.max(0.0));
        }
        ControlSignal::EventDelay { delay_s } => {
            if self.phase == EventPhase::Active {
                return Err(HaresError::Control("cannot delay an active event".to_string()));
            }
            self.delay_remaining_s += delay_s;
        }
        _ => { /* error */ }
    }
    Ok(())
}
```

### 2.4 Findings

| Issue | Severity | Description |
|-------|----------|-------------|
| **No DR signal for loads** | HIGH | `ControlSignal::DemandResponse` is accepted by HVAC and water heaters but not by load equipment. Loads cannot participate in demand response programs. |
| **No DutyCycle for loads** | MEDIUM | `ControlSignal::DutyCycle` exists but is not handled by any load equipment. Could enable cycling control for refrigerators. |

---

## 3. OCHRE Load Control Approach

### 3.1 Embedded Scheduling in Equipment

OCHRE embeds scheduling logic directly in equipment classes:

**ScheduledLoad** (`vendors/OCHRE/ochre/Equipment/ScheduledLoad.py`):
```python
def update_internal_control(self):
    if self.is_electric:
        self.p_set_point = self.current_schedule["Power (kW)"]
    if self.is_gas:
        self.gas_set_point = self.current_schedule["Gas (therms/hour)"]
    return "On" if self.p_set_point + self.gas_set_point != 0 else "Off"

def update_external_control(self, control_signal):
    # Load Fraction, P Setpoint, Gas Setpoint applied here
    load_fraction = control_signal.get("Load Fraction")
    if load_fraction is not None:
        self.p_set_point *= load_fraction
        self.gas_set_point *= load_fraction
```

**Key OCHRE behavior**:
- Schedule is read from CSV columns during `initialize_schedule()`
- Internal control (`update_internal_control()`) pulls from schedule each step
- External control (`update_external_control()`) applies overrides

### 3.2 EventBasedLoad in OCHRE

From `vendors/OCHRE/ochre/Equipment/EventBasedLoad.py`:

```python
def initialize_schedule(self, event_schedule=None, **kwargs):
    # Generate event schedule from time-series, PDF, or event list
    if event_schedule is not None:
        self.all_events = event_schedule
    elif not ts_schedule.empty:
        self.all_events = self.extract_events(ts_schedule, **kwargs)
    elif "equipment_pdf_file" in kwargs:
        probabilities, event_data = self.import_from_pdf(**kwargs)
        self.all_events = self.generate_events(probabilities, event_data, **kwargs)
```

**Event handling**:
- Events extracted from power time series during init
- Or generated from probability density functions
- Or imported from external event list files

### 3.3 WetAppliance in OCHRE

OCHRE's `WetAppliance.py` uses the **MC Profile Generator** (Monte Carlo) for stochastic event generation:

```python
def MC_Profile_update(self):
    # Probability vector determines switch-on likelihood per minute
    self.Random_num = np.random.random()
    self.Binary = self.Binary_Mem * 1 + (1 - self.Binary_Mem * 1) * (
        self.Random_num < self.Switch_On_Prob_Vec[self.Switch_On_Time_Loc,]
    )
    if (self.Binary > 0) & (self.Profile_t < (self.Set_Profile.shape[0] - 1)):
        self.P_kW = self.Set_Profile[self.Profile_t, 0]
        self.Q_kVAr = self.Set_Profile[self.Profile_t, 1]
        self.Profile_t += 1
```

**Key differences from HARES**:
- OCHRE uses per-minute probability vectors for stochastic start times
- OCHRE has built-in scheduling delay capability (`Schedulable` mode)
- OCHRE tracks full demand profile (multi-phase power/duration)

---

## 4. HARES vs OCHRE Architectural Comparison

### 4.1 Architecture Overview

| Aspect | HARES | OCHRE |
|--------|-------|-------|
| **Scheduling** | Baked into equipment at init via `power_schedule_source` config | Embedded in `update_internal_control()` each step |
| **Stochastic Events** | Extracted once at init from kW series | Generated per-step via probability vectors |
| **Actor Model** | Explicit `Actor` trait with `decide()` method | Implicit via Dwelling update loop |
| **Control Signals** | Centralized `ControlSignal` enum + `DispatchRequest` | Dict-based signals passed to `update_external_control()` |
| **Priority Tiers** | 4 tiers: Schedule, UserOverride, Grid, Safety | Not explicitly modeled |

### 4.2 HARES Advantages

| Feature | Benefit |
|---------|---------|
| **Type-safe signals** | Rust enum prevents invalid signal types |
| **Actor interest filtering** | `ActorInterest` enables skip logic for sparse actors |
| **Priority tiers** | Explicit conflict resolution between signals |
| **Separation of concerns** | Schedules are config, not embedded logic |

### 4.3 OCHRE Advantages

| Feature | Benefit |
|---------|---------|
| **Embedded scheduling** | No external actor needed; self-contained |
| **Per-step stochastic** | Events can adapt during simulation |
| **PDF-based events** | More flexible stochastic modeling |
| **Built-in diversity** | MC Profile Generator handles probability-based diversity |

### 4.4 Findings

| Issue | Severity | Description |
|-------|----------|-------------|
| **HARES: No active schedule actor** | MEDIUM | Schedules are config-only; no actor actively dispatches time-based control |
| **OCHRE: Embedded logic** | INFO | Harder to swap scheduling algorithms without modifying equipment code |
| **HARES: Simplified wet appliance** | HIGH | HARES extracts events once at init; OCHRE generates per-step with probability vectors |

---

## 5. Equipment Reception of Actor Control Signals

### 5.1 Signal Flow Architecture

The control signal flow in HARES:

```
Actor::decide(env, &mut out) 
    → Vec<DispatchRequest> 
    → ControlDispatcher::dispatch() 
    → Equipment::apply_control_unchecked(signal) 
    → Equipment internal state updated
```

**Components**:

1. **Actor** (`hares-core/src/actor.rs`):
   - Trait: `fn decide(&mut self, env: &EnvironmentState, out: &mut Vec<DispatchRequest>)`
   - Emits requests without direct equipment access

2. **DispatchRequest** (`hares-control/src/dispatch.rs`):
   ```rust
   pub struct DispatchRequest {
       pub target: DispatchTarget,  // ByName or ByEndUse
       pub signal: ControlSignal,
       pub priority: PriorityTier,   // Schedule, UserOverride, Grid, Safety
   }
   ```

3. **ControlDispatcher** (in dwelling):
   - Routes signals to equipment by target
   - Applies priority tier conflict resolution

4. **Equipment** (`hares-equipment`):
   - Trait: `fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> Result<()>`
   - Updates internal state based on signal

### 5.2 Dispatch Target Routing

Equipment can be targeted by:
- **Name**: `DispatchTarget::ByName("Indoor Lighting")`
- **EndUse**: `DispatchTarget::ByEndUse(EndUse::PLUG_LOADS)`

This enables actors to emit signals to groups of equipment.

### 5.3 Findings

| Issue | Severity | Description |
|-----|---------|-------------|
| **No broadcast to load end-use** | LOW | The Occupant actor supports `ByEndUse` for plug loads but not all load types (no `LIGHTING`, `REFRIGERATION` end-use dispatch) |
| **Signal validation** | INFO | Equipment returns errors for unsupported signals; no graceful fallback |

---

## 6. Wet Appliance + Water Heater Coordination

### 6.1 DHW_DEMAND_LOOP System

HARES coordinates wet appliances with water heaters via the **DHW_DEMAND_LOOP** fluid port system (`crates/hares-equipment/src/water_heater/mod.rs`):

```rust
pub const DHW_DEMAND_LOOP: LoopId = LoopId(u16::MAX - 1);

pub fn get_dhw_demand(ports: &PortSlots) -> f64 {
    ports.fluid
        .iter()
        .find(|acc| acc.loop_id == DHW_DEMAND_LOOP)
        .map(|acc| acc.rate_kg_s)
        .unwrap_or(0.0)
}
```

### 6.2 WetAppliance Hot Water Draw

From `crates/hares-equipment/src/event_load.rs`:

```rust
pub struct CyclePhase {
    power_kw: f64,
    duration_s: f64,
    // Hot water draw during this phase (kg/s)
    hot_water_draw_rate_kg_s: Option<f64>,
}
```

When a cycle phase has hot water draw configured, the WetAppliance emits fluid to the DHW_DEMAND_LOOP port.

### 6.3 Water Heater Draw Integration

Water heaters (`crates/hares-equipment/src/water_heater/tank.rs`) read the accumulated DHW demand:

```rust
pub fn update_from_dhw_demand(&mut self, dhw_demand_kg_s: f64) {
    let draw_rate = dhw_demand_kg_s + self.schedule_draw_rate_kg_s;
    self.model.update_water_draw(draw_rate);
}
```

### 6.4 OCHRE Coordination

In OCHRE, wet appliance + water heater coordination is implicit:

- WetAppliance has `hot_water_draw_volume_l` parameter
- WaterHeater calculates draw from this plus other sources
- No explicit port system; coupled through shared simulation state

### 6.5 Findings

| Issue | Severity | Description |
|-----|---------|-------------|
| **HARES: Good separation** | INFO | DHW_DEMAND_LOOP cleanly decouples appliance from water heater |
| **OCHRE: Implicit coupling** | INFO | Harder to trace hot water flow; tightly coupled |
| **Limited phase configuration** | LOW | Only one hot water draw rate per phase; OCHRE supports multi-profile |

---

## 7. Summary

### 7.1 Architecture Summary

| Aspect | HARES | OCHRE |
|--------|-------|-------|
| Load scheduling | Config-based at init | Embedded in equipment |
| Actor model | Explicit `Actor` trait + `DispatchRequest` | Implicit in Dwelling loop |
| Control signals | Type-safe `ControlSignal` enum | Dict-based signals |
| Stochastic events | Extracted once at init | Per-step probability vectors |
| Water heater coupling | DHW_DEMAND_LOOP ports | Implicit shared state |

### 7.2 Key Findings

| Issue | Severity | Action |
|-------|----------|--------|
| No load demand response participation | HIGH | Add DR signal handling to ScheduledLoad/EventBasedLoad |
| No active schedule actor | MEDIUM | Consider ScheduleActor for time-based dispatch |
| Simplified wet appliance stochasticity | HIGH | Consider adding PDF-based event generation |
| No DutyCycle for loads | MEDIUM | Add to ScheduledLoad for refrigerator cycling |

### 7.3 Recommendations

1. **Add DemandResponse to load equipment**: Allow loads to participate in DR programs
2. **Document HARES actor philosophy**: Clarify that schedules are config, not runtime logic
3. **Consider OccupantActor for all load types**: Enable ByEndUse dispatch for lighting/refrigeration
4. **Evaluate stochastic event needs**: Determine if per-step generation is needed for wet appliances

---

## File Locations Referenced

- `crates/hares-core/src/actor.rs` - Actor trait definition
- `crates/hares-core/src/actors/mod.rs` - Built-in actors
- `crates/hares-core/src/actors/occupant.rs` - Occupant actor
- `crates/hares-control/src/dispatch.rs` - DispatchRequest types
- `crates/hares-control/src/signal.rs` - ControlSignal constructors
- `crates/hares-equipment/src/scheduled_load.rs` - ScheduledLoad control handling
- `crates/hares-equipment/src/event_load.rs` - EventBasedLoad/WetAppliance control
- `crates/hares-equipment/src/water_heater/mod.rs` - DHW_DEMAND_LOOP
- `vendors/OCHRE/ochre/Equipment/ScheduledLoad.py` - OCHRE reference
- `vendors/OCHRE/ochre/Equipment/EventBasedLoad.py` - OCHRE reference
- `vendors/OCHRE/ochre/Equipment/WetAppliance.py` - OCHRE reference
