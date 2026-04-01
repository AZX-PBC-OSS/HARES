# Battery Actor Control Architecture Review - HARES

**Review Date**: 2026-03-31  
**Reviewer**: Code Review Agent  
**Scope**: Actor-based BMS control architecture, control signals, equipment reception, telemetry feedback, OCHRE comparison

---

## 1. Actor Architecture

### Primary Actor: BatteryManagementActor

The `BatteryManagementActor` (defined in `crates/hares-core/src/actors/bms.rs`) is the central decision-making entity for battery control. It implements the `Actor` trait and is responsible for:

1. **BMS Mode Selection**: Deciding which operating mode to apply based on configuration and environment
2. **Control Signal Dispatch**: Emitting typed `ControlSignal` messages to the battery equipment
3. **Grid Export Policy**: Enforcing `GridExportRule` (Unrestricted, Disabled, SolarOnly) as the authoritative policy layer

### BMS Modes Supported

| Mode | Description | HARES Implementation |
|------|-------------|---------------------|
| `Manual` | No autonomous control - equipment idle | Default mode, no dispatch |
| `SelfConsumption` | Discharge on net load, charge on PV surplus | Emits `SelfConsumption` signal |
| `TimeOfUseOptimization` | Charge on low price, discharge on high price | Emits `PowerSetpoint` with price thresholds |
| `BackupReserve` | Maintain target SOC for outage protection | Emits `SOCTarget` + `PowerLimit` |
| `DemandResponse` | Wrapper mode with DR constraints | Intercepts price signal, emits `PowerSetpoint` |
| `Scheduled` | Time-window based charge/discharge/hold | Matches windows, emits `PowerSetpoint` or `SOCTarget` |
| `StormWatch` | Maintain high SOC based on weather trigger | Emits `SOCTarget` when trigger active |

### Actor Decision Flow

```
EnvironmentState (price, PV, load, SOC, weather)
        │
        ▼
BatteryManagementActor.decide(env, &mut out)
        │
        ├──► Read SOC from env.equipment_core
        ├──► Read electrical summary (PV, load)
        ├──► Read price signal (for TOU, DR)
        ├──► Read weather (for StormWatch)
        │
        ▼
evaluate_mode(&bms_mode, env, out)
        │
        └──► Emit ControlSignal(s) to dispatch target
```

---

## 2. Control Signals

### Signals Emitted by BatteryManagementActor

The actor dispatches the following `ControlSignal` variants to the battery equipment:

| Signal | Fields | Purpose |
|--------|--------|---------|
| `PowerSetpoint` | `active_power_kw`, `reactive_power_kvar` | Direct charge/discharge power target (kW) |
| `SOCTarget` | `target_soc`, `min_soc`, `max_soc` | SOC-based control with bounds |
| `SelfConsumption` | `enabled`, `solar_only_charging` | Enable self-consumption mode |
| `GridConnect` | `connected` | Island/disconnect battery from grid |
| `PowerLimit` | `max_power_kw`, `ramp_rate_kw_per_s` | External power limit ceiling |
| `DemandResponse` | `level`, `duration_s` | DR event with severity and duration |

### Grid Export Rule Enforcement

The actor is the **authoritative layer** for grid export policy. It applies defense-in-depth clamping:

```rust
fn clamp_discharge_for_export(&self, discharge_kw: f64, env: &EnvironmentState) -> f64 {
    match self.grid_export_rule {
        GridExportRule::Unrestricted => discharge_kw,
        GridExportRule::Disabled => {
            // Clamp to home load only - battery never exports
            discharge_kw.min(env.electrical.base_load_kw.max(0.0))
        }
        GridExportRule::SolarOnly => {
            // Clamp to load + PV - net export <= PV generation
            let max_allowed = env.electrical.base_load_kw.max(0.0) 
                + env.electrical.pv_generation_kw.max(0.0);
            discharge_kw.min(max_allowed)
        }
    }
}
```

This is a **critical architectural distinction** from OCHRE: the actor makes policy decisions, the equipment simply executes.

---

## 3. Equipment Reception

### How Battery Receives Control Signals

The `Battery` equipment (in `crates/hares-equipment/src/battery/mod.rs`) receives control signals via the `apply_control_unchecked` method:

```rust
fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
    match signal {
        ControlSignal::PowerSetpoint { active_power_kw, .. } => {
            self.power_setpoint_kw = Some(*active_power_kw);
            self.soc_target = None;
            self.self_consumption_enabled = false;
        }
        ControlSignal::SOCTarget { target_soc, min_soc, max_soc } => {
            self.soc_target = Some(*target_soc);
            self.soc_target_min = *min_soc;
            self.soc_target_max = *max_soc;
            self.power_setpoint_kw = None;
            self.self_consumption_enabled = false;
        }
        ControlSignal::GridConnect { connected } => {
            self.grid_connected = *connected;
        }
        ControlSignal::SelfConsumption { enabled, solar_only_charging } => {
            self.self_consumption_enabled = *enabled;
            self.solar_only_charging = *solar_only_charging;
            self.power_setpoint_kw = None;
            self.soc_target = None;
        }
        ControlSignal::PowerLimit { max_power_kw, .. } => {
            self.external_power_limit_kw = Some(max_power_kw.max(0.0));
        }
        ControlSignal::DemandResponse { level, duration_s } => {
            self.dr_level = *level;
            self.dr_duration_remaining_s = *duration_s;
        }
        _ => return Err(HaresError::Control(...))
    }
    Ok(())
}
```

### Control Priority Order (in `determine_target_power`)

The equipment processes control signals in a fixed priority order:

1. **Explicit Power Setpoint** - Direct power target from actor
2. **SOC Target** - Proportional controller to reach target SOC
3. **Self-Consumption Mode** - Net load based charge/discharge
4. **Default** - Idle (0 kW)

### Power Clamping (in `clamp_power`)

The equipment applies multiple limit layers:

- Hardware limits (`max_charge_kw`, `max_discharge_kw`)
- Temperature derating (via `CapacityDerateModel`)
- DR power fraction (Normal=1.0, Moderate=0.8, High=0.5, Critical=0.25, GridEmergency=0.0)
- Import/export limits (from config)
- External power limit (from `PowerLimit` signal)
- Charging curve LUT (if configured)

---

## 4. Telemetry Feedback

### How Battery Reports State to Actors

The battery reports state through the `CoreOutput` struct, which actors read from `EnvironmentState.equipment_core`:

```rust
self.core_output = CoreOutput {
    flows: CoreFlows {
        electric_kw: Some(ElectricPower::Bidirectional(port_power_kw)),
        reactive_power_kvar: None,
        fuel_w: None,
    },
    state: CoreState {
        operating_mode: Some(self.mode),  // Charging/Discharging/Standby
        soc: Soc::try_from(self.soc).ok(),
    },
};
```

### Actor Reads SOC

```rust
fn read_soc(&self, env: &EnvironmentState) -> Option<f64> {
    let id = self.equipment_id?;
    env.equipment_core
        .get(&id)
        .and_then(|co| co.state.soc)
        .map(|s| s.get())
}
```

### Telemetry Fields Available

| Telemetry Key | Description | Used by Actor |
|---------------|-------------|---------------|
| `soc` | State of charge (0-1) | Yes - all modes check SOC |
| `active_power_kw` | Net grid power | For logging/debugging |
| `cell_temp_c` | Cell temperature | For temperature derating logic |
| `discharge_derate` | Temperature derate factor | Internal to equipment |
| `capacity_derate` | Capacity derate factor | Internal to equipment |
| `cycle_count` | Rainflow cycles | For degradation tracking |
| `capacity_fade_pct` | Capacity loss % | For degradation tracking |

---

## 5. OCHRE Comparison

### OCHRE's Architecture: Equipment as Decision Maker

In OCHRE, the `Battery` class (which inherits from `Generator`) **bundles BMS mode decisions inside the equipment itself**:

```python
# OCHRE Generator.py - update_internal_control()
if self.self_consumption_mode:
    net_power = self.current_schedule.get("net_power")
    if net_power is not None:
        desired_power = max(min(net_power, self.import_limit), -self.export_limit)
        self.power_setpoint = desired_power - net_power
else:
    # Charges or discharges based on schedule
    self.power_setpoint = self.current_schedule.get(f"{self.end_use} Electric Power (kW)", 0)
```

### OCHRE BMS Mode Implementation

| Mode | OCHRE Implementation |
|------|---------------------|
| `SelfConsumption` | `self_consumption_mode` flag in equipment |
| Scheduled | Schedule input (`Battery Electric Power (kW)`) |
| Manual | Default (no autonomous control) |

### What's Missing in OCHRE

OCHRE does **not** implement:
- Time-of-use optimization (price-based charge/discharge)
- Backup reserve mode (target SOC maintenance)
- Demand response with base mode delegation
- Storm watch (weather-triggered high SOC)
- Scheduled mode with multiple windows and actions

### Violation of Separation of Concerns

OCHRE's architecture violates the separation of concerns principle:

1. **Equipment does too much**: The battery equipment contains control logic (self-consumption mode, import/export limits, schedule processing)
2. **No clear policy layer**: Grid export rules are buried in the equipment's internal control
3. **Hard to test**: Control logic is coupled to equipment physics
4. **No composability**: Cannot easily add new modes without modifying equipment code

---

## 6. HARES vs OCHRE: Architectural Superiority

### Why HARES Actor-Based Control is Better

| Aspect | HARES | OCHRE | Winner |
|--------|-------|-------|--------|
| **Separation of Concerns** | Actor decides mode, equipment executes | Equipment decides and executes | HARES |
| **Policy Layer** | Actor enforces GridExportRule | Mixed in equipment | HARES |
| **Testability** | Actor logic tested independently | Coupled to equipment | HARES |
| **Composability** | Modes can wrap other modes (DR wraps base) | Single flag-based mode | HARES |
| **Extensibility** | Add new modes by implementing Actor | Requires equipment changes | HARES |
| **Mode Coverage** | 7 modes (SelfConsumption, TOU, Backup, DR, Scheduled, StormWatch, Manual) | 2 modes (SelfConsumption, Scheduled) | HARES |
| **Grid Export Policy** | Explicit `GridExportRule` with clamping | Implicit via import/export limits | HARES |

### Specific Architectural Advantages

1. **BatteryManagementActor is the Authoritative Policy Layer**
   - Grid export rules are enforced in the actor, not the equipment
   - Defense-in-depth: actor clamps discharge before sending signal, equipment also clamps
   - Clear audit trail: `last_action` field tracks all decisions

2. **Modes are Data, Not Code**
   - `BmsMode` is a Rust enum with variants containing configuration
   - Adding a new mode variant doesn't require changing equipment code
   - Modes can be serialized/deserialized (checkpointing)

3. **Composition via Boxing**
   - `DemandResponse { base_mode: Box<BmsMode> }` wraps any other mode
   - `StormWatch { base_mode: Box<BmsMode> }` conditionally enables base mode
   - This enables complex behaviors without equipment changes

4. **Environment-Driven Decisions**
   - Actor reads from `EnvironmentState` (price, PV, load, weather, SOC)
   - Decision logic is transparent and traceable
   - Same actor can work with different equipment implementations

---

## 7. Summary

### Key Findings

1. **HARES has explicit actor-based control separation** - The `BatteryManagementActor` makes all BMS mode decisions, the `Battery` equipment simply executes control signals.

2. **OCHRE bundles behavior into equipment** - The OCHRE `Battery` class contains control logic (`self_consumption_mode`, schedule processing) that belongs in a separate control layer.

3. **HARES supports 7 BMS modes** vs OCHRE's 2 - HARES implements SelfConsumption, TimeOfUseOptimization, BackupReserve, DemandResponse, Scheduled, StormWatch, and Manual modes.

4. **Actor is the authoritative grid export policy layer** - The `BatteryManagementActor` applies `GridExportRule` clamping as defense-in-depth, ensuring policy is enforced even if equipment has bugs.

5. **Equipment receives signals via `apply_control_unchecked`** - The battery processes PowerSetpoint, SOCTarget, SelfConsumption, GridConnect, PowerLimit, and DemandResponse signals with clear priority ordering.

6. **Telemetry flows back via CoreOutput** - The battery reports SOC and operating mode through `CoreOutput`, which actors read from `EnvironmentState.equipment_core`.

### Architectural Recommendation

HARES' actor-based control architecture is **superior** to OCHRE's equipment-internal control for the following reasons:

1. Clear separation of concerns (decisions vs execution)
2. Explicit policy layer (grid export rules enforced by actor)
3. Better testability (actor logic can be tested in isolation)
4. Better composability (modes wrap other modes)
5. Better extensibility (add modes without equipment changes)

This pattern should be applied to other equipment types in HARES where similar control decisions need to be made (e.g., HVAC, water heating).

---

## File References

- **Actor Implementation**: `crates/hares-core/src/actors/bms.rs`
- **Control Signal Types**: `crates/hares-types/src/control_signal.rs`
- **BMS Mode Types**: `crates/hares-types/src/equipment.rs` (line 665)
- **Battery Equipment**: `crates/hares-equipment/src/battery/mod.rs`
- **OCHRE Battery**: `vendors/OCHRE/ochre/Equipment/Battery.py`
- **OCHRE Generator**: `vendors/OCHRE/ochre/Equipment/Generator.py`
