# EV Actor Control Architecture Review

**Review Date**: 2026-03-31  
**Reviewer**: Code Review  
**Scope**: EV Actor-based control architecture in HARES vs OCHRE

---

## 1. Actor Architecture: What Actors Control EV Equipment

### HARES Architecture

In HARES, EV equipment is controlled by the **`EvDriverActor`** (`hares-core/src/actors/ev_driver/mod.rs`). This actor models human driver behavior and makes all high-level decisions about:

1. **Plug-in decisions**: When to plug in at home based on `PlugInPolicy`
2. **Charging strategy**: Which strategy to apply (Immediate, Nightly, TouAware, V2H, V2G, etc.)
3. **Departure times**: When the vehicle needs to be ready by
4. **Driving events**: When to depart, how far to drive, energy consumption

### State Machine

The `EvDriverActor` maintains a `DriverPhase` state machine:

```rust
enum DriverPhase {
    /// At home, plugged in (or waiting to plug in)
    HomePluggedIn,
    /// Currently driving (multi-step energy drain)
    Driving { remaining_kwh, total_steps, steps_done },
    /// Away from home (parked, not driving)
    Away,
}
```

### Decision Flow

Each timestep, the actor:
1. **Rolls daily event** (if new day): departure time, arrival time, drive energy
2. **Evaluates phase**: Based on current phase and time, decides what signals to emit
3. **Dispatches control signals**: Sends `ControlSignal` variants to EV equipment

---

## 2. Control Signals: What Actors Send to EV Equipment

### EV-Specific Control Signals

| Signal | Description | Actor Usage |
|--------|-------------|-------------|
| `EvPlugIn` | Set connection state (HomePluggedIn, AwayPluggedIn, Disconnected) | Driver arrival/departure |
| `EvDrive` | Deduct driving energy from SOC (only when Disconnected) | During driving phase |
| `EvAwayCharge` | Set external charger power for away charging | Workplace/public charging |
| `EvSetReadyBy` | Tell EV BMS to reach target_soc by departure_hour | PreDeparture strategy |
| `SOCTarget` | Target SOC with optional min/max bounds | All charging strategies |
| `PowerSetpoint` | Direct power control (kW), positive=charge, negative=discharge | V2H/V2G, solar surplus |
| `PowerLimit` | Maximum power cap from external source | Grid constraint handling |

### ChargingStrategy Enum

The actor selects from these strategies (`hares-types/src/equipment.rs:423-465`):

```rust
pub enum ChargingStrategy {
    Immediate { target_soc },           // Charge immediately to target
    Nightly { off_peak_start_hour, off_peak_end_hour, target_soc },
    LowSoc { threshold, target_soc },   // Charge when SOC < threshold
    QuickThenWait { partial_soc },      // Quick charge to partial, then wait
    PreDeparture { target_soc, departure_schedule },
    TouAware { target_soc, departure_schedule, charge_buffer_hours },
    SolarSurplus { min_charge_rate_kw, departure_schedule },
    V2H { discharge_threshold_soc, min_soc },
    V2G { min_soc, max_export_kw, price_threshold },
}
```

### Preference Stack

For each strategy, the actor builds a **preference stack** that guides per-step charging decisions:

- **Immediate**: `SocTarget` only
- **Nightly**: `TimeWindowPref` + `SocTarget`
- **LowSoc**: `SocGate` (block charging above threshold)
- **TouAware**: `PriceOptimizer` + `DepartureDeadline` + `SocTarget`
- **SolarSurplus**: `SolarTracking` + `DepartureDeadline`
- **V2H**: `SocTarget` + `V2HDischarge`
- **V2G**: `SocTarget` + `V2GExport`

---

## 3. OCHRE Comparison: How OCHRE Handles EV Differently

### OCHRE's Monolithic Approach

In OCHRE (`vendors/OCHRE/ochre/Equipment/EV.py`), the EV equipment class bundles **all** behavior:

1. **Plug-in decisions**: Built into `generate_events()` - uses EVI-Pro probability distributions to decide when vehicle arrives and plugs in
2. **Charging strategy**: Hardcoded as "charge to full during parking event" - no strategy selection
3. **Departure scheduling**: Derived from event end time - vehicle departs when event ends
4. **Charging control**: Simple max power / max SOC limits in `update_external_control()`

### Key OCHRE Code Patterns

```python
# OCHRE generates charging events from probability distributions
def generate_events(self, probabilities, event_data, ...):
    # Uses EVI-Pro PDFs for arrival time, start SOC, duration
    # Forces overnight charging every night
    ...

# OCHRE control is simple power/SOC limiting
def update_external_control(self, control_signal):
    if "P Setpoint" in control_signal:
        self.max_power_ctrl = setpoint
    if "Max SOC" in control_signal:
        self.soc_max_ctrl = max_soc
```

### OCHRE Limitations

| Aspect | OCHRE Behavior | HARES Advantage |
|--------|---------------|-----------------|
| Plug-in logic | Baked into event generation | Actor decides via PlugInPolicy |
| Charging strategy | Single "charge to full" mode | 9 different strategies available |
| Departure scheduling | Derived from event end | Explicit EvSetReadyBy control |
| V2H/V2G | Not supported | Full V2H/V2G with price/SOC triggers |
| TOU optimization | Not available | TouAware with price schedule |
| Solar coupling | Not available | SolarSurplus strategy |

---

## 4. HARES vs OCHRE: Architectural Superiority

### Separation of Concerns

**HARES Architecture:**
```
                    ┌─────────────────────┐
                    │   EvDriverActor     │  ← Decides: plug-in, strategy, departure
                    └──────────┬──────────┘
                               │ ControlSignal dispatch
                    ┌──────────▼──────────┐
                    │   EV Equipment      │  ← Executes: physics, SOC, power
                    └─────────────────────┘
```

**OCHRE Architecture:**
```
                    ┌─────────────────────┐
                    │   ElectricVehicle   │  ← Bundles: events, charging, departure
                    │   (equipment only)  │
                    └─────────────────────┘
```

### Why HARES Is Superior

1. **Explicit actor-based control separation**
   - Actor decides *what* to do (plug in? charge? discharge?)
   - Equipment does *how* (physics simulation, SOC tracking)

2. **Multiple charging strategies**
   - OCHRE: single "charge to full" mode
   - HARES: 9 strategies (Immediate, Nightly, TouAware, V2H, V2G, etc.)

3. **Decoupled strategy selection**
   - Strategy is a configuration parameter, not hardcoded
   - Can change strategy without changing equipment model

4. **Coordinated DER integration**
   - Actors can coordinate across multiple EVs and other DERs
   - V2H/V2G actors integrate with home/grid energy management

5. **Testability**
   - Actor logic can be unit tested independently
   - Equipment physics can be benchmarked separately

---

## 5. Equipment Reception: How EV Processes Actor Signals

### Signal Validation

The EV equipment validates signals before application (`ev/mod.rs:848-869`):

```rust
fn validate_signal(&self, signal: &ControlSignal) -> Result<()> {
    ensure_signal_supported(self.descriptor().control_capabilities, signal)?;
    match signal {
        ControlSignal::EvDrive { .. } => {
            if self.connection_state != EvConnectionState::Disconnected {
                return Err("EvDrive rejected: EV must be Disconnected");
            }
        }
        ControlSignal::EvAwayCharge { .. } => {
            if self.connection_state != EvConnectionState::AwayPluggedIn {
                return Err("EvAwayCharge rejected: EV must be AwayPluggedIn");
            }
        }
        _ => {}
    }
    Ok(())
}
```

### Signal Application

The `apply_control_unchecked()` method (`ev/mod.rs:871-1000`) processes each signal:

| Signal | Processing |
|--------|------------|
| `PowerSetpoint` | Stores in `power_setpoint_kw`, used in `compute_charging_power_kw()` |
| `PowerLimit` | Stores in `power_limit_kw`, caps charging power |
| `SOCTarget` | Stores in `soc_target`, `soc_target_min`, `soc_target_max` |
| `EvPlugIn` | Updates `connection_state`, resets away charger power |
| `EvDrive` | Decrements SOC by kwh amount |
| `EvAwayCharge` | Sets `away_charger_power_kw` for away charging |
| `EvSetReadyBy` | Sets `ready_by_hour` and `ready_by_soc` for BMS scheduling |

### BMS Ready-By Scheduling

The EV equipment implements smart charging (`ev/mod.rs:456-495`):

```rust
fn bms_ready_by_power(&self, now, derated_rated, soc_limit) -> f64 {
    // Calculate hours until departure deadline
    // If enough time, delay charging (idle)
    // If deadline near, charge at max rate
}
```

---

## 6. V2H/V2G: Actor Coordination for Discharge

### V2HDischarge (`v2h.rs`)

Vehicle-to-Home discharge coordinated by actor preferences:

```rust
impl ChargingPreference for V2HDischarge {
    fn constraint(&mut self, ctx: &DecisionContext) -> Constraint {
        // Override idle if SOC <= min_soc floor
        if ctx.current_soc <= self.min_soc {
            Constraint::Override(PreferenceVote::idle("v2h:soc_floor"))
        } else {
            Constraint::Inactive
        }
    }

    fn score(&mut self, ctx: &DecisionContext) -> PreferenceVote {
        let deficit = ctx.env.electrical.base_load_kw 
                    - ctx.env.electrical.pv_generation_kw;
        
        // Discharge when: SOC > threshold AND home has deficit
        if ctx.current_soc > self.threshold_soc && deficit > 0.0 {
            PreferenceVote {
                power_kw: Some(-deficit.min(self.max_discharge_kw)),
                min_soc: Some(self.min_soc),
                score: 2.0,
                label: "v2h:discharging",
            }
        } else {
            PreferenceVote::idle("v2h:idle")
        }
    }
}
```

**Trigger conditions:**
- SOC above `discharge_threshold_soc` (e.g., 0.8)
- Home electrical deficit (load > solar generation)
- SOC stays above `min_soc` floor (e.g., 0.2)

### V2GExport (`v2g.rs`)

Vehicle-to-Grid discharge for grid services:

```rust
impl ChargingPreference for V2GExport {
    fn score(&mut self, ctx: &DecisionContext) -> PreferenceVote {
        let price = ctx.env.price_signal.electricity_price.unwrap_or(0.0);
        
        // Export when electricity price exceeds threshold
        if price > self.price_threshold {
            PreferenceVote {
                power_kw: Some(-self.max_export_kw),
                min_soc: Some(self.min_soc),
                score: 3.0,  // Higher score than V2H
                label: "v2g:exporting",
            }
        } else {
            PreferenceVote::idle("v2g:idle")
        }
    }
}
```

**Trigger conditions:**
- SOC above `min_soc` floor (e.g., 0.3)
- Electricity price above `price_threshold` (e.g., $0.20/kWh)

### Equipment V2L/V2G Support

The EV equipment implements discharge physics (`ev/mod.rs:497-519`):

```rust
fn compute_v2l_discharge(&self, dt: Duration) -> f64 {
    // Only discharge if SOC > v2l_soc_reserve
    if self.soc <= self.v2l_soc_reserve {
        return 0.0;
    }
    // Cap by max discharge power and available energy
    let available_kwh = (self.soc - self.v2l_soc_reserve) * self.battery_capacity_kwh;
    let max_discharge_kw = available_kwh / dt_hours;
    -(setpoint.abs().min(self.v2l_max_discharge_kw).min(max_discharge_kw))
}
```

---

## 7. Summary: Architectural Advantages

| Aspect | OCHRE | HARES |
|--------|-------|-------|
| **Control model** | Equipment-only | Actor + Equipment |
| **Plug-in decisions** | Baked into event generation | Actor decides via PlugInPolicy |
| **Charging strategies** | Single "charge to full" | 9 strategies (Immediate, Nightly, TouAware, V2H, V2G, etc.) |
| **Departure scheduling** | Derived from event end | Explicit EvSetReadyBy control |
| **V2H/V2G** | Not supported | Full support with actor coordination |
| **TOU optimization** | Not available | TouAware with price schedule |
| **Solar coupling** | Not available | SolarSurplus strategy |
| **Testability** | Limited | Actor and equipment separately testable |
| **Coordination** | Single EV only | Multiple EVs and DERs via actor system |

### Key Architectural Benefits

1. **Clear separation**: Actors decide *what*, equipment does *how*
2. **Flexibility**: Easy to add new strategies without touching equipment
3. **Coordination**: Multiple actors can coordinate through shared environment state
4. **Testability**: Each component independently testable
5. **Extensibility**: V2H/V2G and other DER integrations build on actor framework

---

*End of Review*
