# Water Heater Actor Control Architecture Review

## Overview

This review examines how HARES implements control architecture for water heater (WH) equipment, comparing it with OCHRE's approach. The key architectural difference is that **HARES separates WH control logic into actors** while **OCHRE embeds scheduling and demand response logic directly within the WH equipment model**.

---

## 1. Actor Architecture: What Actors Control Water Heater Equipment?

### HARES Actor Structure

**No dedicated water heater actors exist in HARES.** The actor registry (`crates/hares-core/src/actors/mod.rs`) contains:

- `BatteryManagementActor` (BMS)
- `DrCompliance` (demand response compliance modeling)
- `EvDriverActor` (electric vehicle charging)
- `IdealThermostat` (HVAC thermostat emulation)
- `Occupant` (behavior modeling)
- `SolverFeedbackActor` (solver convergence feedback)

**Key Finding**: Water heaters in HARES are controlled through:
1. **Schedule-based dispatch** - Water heater setpoints from schedule data
2. **DR compliance actor** - Can target WH via `DrCompliance.with_load_target()`
3. **Direct dwelling control** - `dwelling.apply_control(name, signal)`

### Control Flow

```
Schedule Data → Dwelling Dispatcher → ControlSignal → Water Heater Equipment
       ↓
   DR Event → DrCompliance Actor → DispatchRequest (PriorityTier::Grid)
```

---

## 2. Control Signals: What Control Signals Do Actors Send to Water Heater Equipment?

### Supported Control Signals (HARES)

All storage water heaters (resistance, gas, HPWH) support:

| Control Signal | Description | Effect on WH |
|---------------|-------------|--------------|
| `ThermalSetpoint` | Override heating setpoint + deadband | Changes thermostat setpoint |
| `DutyCycle` | On fraction override [0-1] | Forces specific duty cycle |
| `ModeOverride` | Force Off/Heating/BackupElement | Direct mode control |
| `LoadFraction` | Transient power multiplier (resets each step) | Immediate curtailment |
| `PowerLimit` | Cap electrical/thermal input | Power cap enforcement |
| `DemandResponse` | Level + optional duration (auto-revert) | Pre-defined DR response table |

### Demand Response Response Table

All storage water heaters implement DR response identically:

| DR Level | Setpoint Offset | Load Fraction |
|----------|-----------------|---------------|
| Normal | 0°C | 1.0 |
| Moderate | -3°C | 1.0 |
| High | -6°C | 0.8 |
| Critical | -10°C | 0.5 |
| GridEmergency | 0°C | 0.0 (off) |

Source: `crates/hares-equipment/src/water_heater/resistance.rs:690-714`

### Control Signal Reception

Water heater equipment receives control signals through the `Equipment` trait's `apply_control()` method:

```rust
// resistance.rs:640-686
fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> Result<()> {
    match signal {
        ControlSignal::ThermalSetpoint { heating_setpoint_c, deadband_c, .. } => {
            // Apply setpoint with optional ramp rate limiting
        }
        ControlSignal::DemandResponse { level, duration_s } => {
            self.apply_dr_level(*level);
            self.dr_duration_remaining_s = *duration_s;
        }
        // ... other signals
    }
}
```

---

## 3. OCHRE Comparison: How Does OCHRE Handle Water Heater Control Differently?

### OCHRE's Embedded Approach

OCHRE embeds **all scheduling and DR logic within the WaterHeater equipment class**:

**Source**: `vendors/OCHRE/ochre/Equipment/WaterHeater.py:88-145`

```python
def update_external_control(self, control_signal):
    # Options for external control signals:
    # - Load Fraction: 1 (no effect) or 0 (forces WH off)
    # - Setpoint: Updates setpoint temperature from the default (in C)
    # - Deadband: Updates deadband temperature (in C)
    # - Max Power: Updates maximum allowed power (in kW)
    # - Duty Cycle: Forces WH on for fraction of external time step
    
    ext_setpoint = control_signal.get("Setpoint")
    if ext_setpoint is not None:
        if "Water Heating Setpoint (C)" in self.current_schedule:
            self.current_schedule["Water Heating Setpoint (C)"] = ext_setpoint
        else:
            # Note that this overrides the ramp rate
            self.setpoint_temp = ext_setpoint
```

### Key OCHRE Characteristics

1. **Schedule Integration**: WH reads setpoints directly from `current_schedule` dictionary
   - `update_setpoint()` reads "Water Heating Setpoint (C)" from schedule (line 172)
   - Deadband and max power also read from schedule

2. **Internal Control Logic**: OCHRE's WH has complete internal thermostat control
   - `update_internal_control()` (line 226) implements hysteresis thermostat
   - `run_thermostat_control()` (line 213) implements deadband logic

3. **Duty Cycle Control**: OCHRE supports complex duty cycle control (line 147-168)
   - Works with external controller time resolution
   - Mode priority calculation based on duty cycles

4. **No DR Actor**: OCHRE does not have a separate DR compliance actor
   - DR levels must be passed through schedule or external control signal
   - No compliance modeling (always complies if signal received)

---

## 4. HARES vs OCHRE: Architectural Differences in Control Separation

### HARES Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    Dwelling Orchestrator                    │
├─────────────────────────────────────────────────────────────┤
│  Actors:                                                    │
│  - DrCompliance (decides compliance, emits DR signals)     │
│  - Other actors (BMS, EV, etc.)                            │
│                                                             │
│  Dispatcher:                                               │
│  - Priority-based signal routing                           │
│  - Schedule/Grid/User tiers                                │
└─────────────────────────────────────────────────────────────┘
        │ ControlSignal dispatch
        ↓
┌─────────────────────────────────────────────────────────────┐
│              Water Heater Equipment                         │
├─────────────────────────────────────────────────────────────┤
│  - Physics: Stratified tank model (1-12 nodes)             │
│  - Internal thermostat: hysteresis control                 │
│  - Control reception: apply_control() method               │
│  - DR response: pre-defined level table                    │
│                                                             │
│  Does NOT contain:                                         │
│  - Schedule logic (reads from environment)                 │
│  - DR compliance modeling                                  │
└─────────────────────────────────────────────────────────────┘
```

### OCHRE Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                      Dwelling Controller                    │
├─────────────────────────────────────────────────────────────┤
│  - Reads schedule from CSV/files                           │
│  - Passes schedule dict to equipment                       │
│  - External control signals (if any)                       │
│  - No separate actor system                                │
└─────────────────────────────────────────────────────────────┘
        │ Schedule dict + control signals
        ↓
┌─────────────────────────────────────────────────────────────┐
│              Water Heater Equipment                         │
├─────────────────────────────────────────────────────────────┤
│  - Physics: Tank model (1-node to multi-node)              │
│  - Schedule: Reads setpoint/deadband/max_power from dict   │
│  - Internal thermostat: hysteresis control                 │
│  - External control: Setpoint/Deadband/MaxPower/DutyCycle  │
│  - DR: Passed through control signal or schedule           │
│                                                             │
│  Contains:                                                 │
│  - Schedule logic                                          │
│  - Internal control decisions                              │
│  - All thermostat behavior                                 │
└─────────────────────────────────────────────────────────────┘
```

### Summary Table

| Aspect | HARES | OCHRE |
|--------|-------|-------|
| Control separation | Actors + Equipment | Equipment only |
| Schedule source | Environment/Schedule data | `current_schedule` dict |
| DR compliance modeling | DrCompliance actor | None (always complies) |
| WH thermostat logic | Inside WH equipment | Inside WH equipment |
| Setpoint routing | Via dispatcher | Via schedule dict |
| DR signal handling | DrCompliance decides → dispatches | Direct external control |

---

## 5. Equipment Reception: How Does WH Equipment Receive Actor Control Signals?

### Reception Mechanism

1. **Signal Queuing**: Actors call `dwelling.queue_dispatch(DispatchRequest)`
   - Request contains: `target`, `signal`, `priority`

2. **Priority Tiers**:
   - `UserOverride` (highest)
   - `Grid` (DR events)
   - `Schedule` (default)
   - `Default` (lowest)

3. **Dispatcher Resolution**: `control_dispatcher.resolve()` applies signals
   - Highest priority signal for each equipment wins
   - Signals applied via `eq.apply_control(&signal)`

4. **Water Heater Apply Control**: `resistance.rs:640-686`
   ```rust
   fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> Result<()> {
       match signal {
           ControlSignal::ThermalSetpoint { ... } => {
               // Update setpoint with ramp rate limiting
           }
           ControlSignal::DemandResponse { level, duration_s } => {
               self.apply_dr_level(*level);
               self.dr_duration_remaining_s = *duration_s;
           }
           // ...
       }
   }
   ```

### Signal Validation

Signals can be validated before queuing via `dwelling.apply_control_validated()`:
- Checks equipment exists
- Validates signal against equipment's current state
- Applies immediate updates for idempotent signals (e.g., `EvPlugIn`)

---

## 6. DHW Demand: How Do Actors Coordinate Wet Appliance Draws with WH Response?

### DHW Demand Loop Architecture

**Well-known LoopId**: `DHW_DEMAND_LOOP = LoopId(u16::MAX - 1)`

Source: `crates/hares-equipment/src/water_heater/mod.rs:33`

### Coordination Flow

```
┌──────────────────┐     PortContribution::Fluid      ┌──────────────────┐
│  Wet Appliance   │ ───────────────────────────────→ │  PortSlots       │
│  (Washer/Dish)   │    loop_id: DHW_DEMAND_LOOP     │  (Accumulator)   │
└──────────────────┘                                  └────────┬─────────┘
                                                               │
                                                               │ read_dhw_demand_kg_s()
                                                               ↓
┌──────────────────┐                           ┌──────────────────┐
│ Water Heater     │ ←──────────────────────── │  Total Demand    │
│ Equipment        │     draw_rate_kg_s        │  (kg/s)          │
└──────────────────┘                           └──────────────────┘
```

### Implementation Details

1. **Wet Appliance Emission** (`crates/hares-equipment/src/event_load.rs:875`):
   ```rust
   PortContribution::Fluid {
       loop_id: crate::water_heater::DHW_DEMAND_LOOP,
       flow_rate_kg_s: draw_rate,
       ...
   }
   ```

2. **Water Heater Consumption** (`crates/hares-equipment/src/water_heater/mod.rs:36-43`):
   ```rust
   pub(crate) fn read_dhw_demand_kg_s(ports: &PortSlots) -> f64 {
       ports.fluid.iter()
           .find(|acc| acc.loop_id == DHW_DEMAND_LOOP)
           .map(|acc| acc.total_flow_kg_s.max(0.0))
           .unwrap_or(0.0)
   }
   ```

3. **Total Draw Calculation**:
   ```rust
   // From resistance.rs:476
   let appliance_demand_kg_s = super::read_dhw_demand_kg_s(ports);
   let total_draw = self.draw_flow_rate_kg_s + appliance_demand_kg_s;
   ```

### Testing Evidence

From `crates/hares-equipment/src/water_heater/mod.rs:862-974` (integration tests):

```rust
#[test]
fn end_to_end_wet_appliance_demand_reaches_water_heater() {
    // Step 1: washer runs (Independent stage, rank 0)
    washer.step(&env, Duration::from_secs(60), &mut slots).unwrap();
    
    // Verify the washer emitted demand
    let demand_after_washer = slots.fluid.iter()
        .find(|a| a.loop_id == DHW_DEMAND_LOOP)
        .map(|a| a.total_flow_kg_s);
    
    // Step 2: water heater runs (Thermal stage, rank 2) on same slots
    wh.step(&env, Duration::from_secs(60), &mut slots).unwrap();
    
    // WH telemetry shows combined draw
    let wh_draw = wh.telemetry().get("draw_flow_rate_kg_s");
    // equals: schedule_draw + washer_demand
}
```

---

## Key Findings Summary

1. **No dedicated WH actors** - HARES uses the DrCompliance actor which can target WH via `with_load_target()`, but no scheduling-specific actor exists for water heaters

2. **Signal-based control** - WH equipment receives control through `apply_control()` with support for ThermalSetpoint, DutyCycle, ModeOverride, LoadFraction, PowerLimit, and DemandResponse signals

3. **OCHRE embeds everything** - OCHRE's WH contains schedule reading, internal thermostat logic, and DR response without external actor involvement

4. **Demand coordination** - HARES uses the DHW_DEMAND_LOOP for coordinating wet appliance draws with water heater response through the PortSlots accumulation mechanism

5. **Architectural difference** - HARES separates concerns (actors for decision-making, equipment for physics/actuation), while OCHRE bundles control logic within equipment
