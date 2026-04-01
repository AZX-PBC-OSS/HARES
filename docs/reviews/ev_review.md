# EV Equipment Review for HARES

**Review Date**: 2026-03-31  
**Reviewer**: Code Review  
**Scope**: Electric Vehicle (EV) equipment implementation in HARES  
**Equipment Types**: `ElectricVehicle`, `ScheduledEV`

---

## 1. HPXML Parsing

### Attributes Parsed from HPXML

The HPXML parser in `crates/hares-io/src/hpxml/resolve_der.rs` resolves the following EV attributes:

| HPXML Attribute | HARES Config Key | Notes |
|-----------------|------------------|-------|
| `BatteryCapacity` | `capacity_kwh` | Defaults to 75.0 kWh if not specified |
| `ChargingLevel` | `charging_level` | "Level 1" or "Level 2" → L1/L2 |
| `MaxChargingPower` | `max_charging_power_kw` | Defaults to 11.5 kW for L2 |

### Attributes NOT Parsed from HPXML

The following attributes defined in `hares-types/src/equipment.rs` and `hares-equipment/src/ev/config.rs` are NOT parsed from HPXML and must be provided via explicit config:

- **VehicleType** (BEV/PHEV) - Not parsed from HPXML, HARES defaults to BEV behavior for power calculations
- **EvConnectionState** - Defaults to `HomePluggedIn`
- **ChargingStrategy** - Not parsed from HPXML, defaults to `Immediate { target_soc: 1.0 }`
- **PlugInPolicy** - Not parsed from HPXML, defaults to `Always`
- **ChargingEfficiency** - Not parsed from HPXML, defaults to 0.9
- **InitialSOC** - Not parsed from HPXML, defaults to 1.0
- **V2L/V2G parameters** - All not parsed, defaults to disabled
- **Battery chemistry, thermal parameters** - Not parsed

### Severity: MEDIUM - HPXML EV support is minimal

HPXML provides a limited EV representation. The most significant missing attribute is `VehicleType`, which affects default charging power selection (PHEV20 vs PHEV50 vs BEV100 vs BEV250 have different power levels in OCHRE).

---

## 2. OCHRE Defaults/Fallbacks

### OCHRE (vendors/OCHRE/ochre/Equipment/EV.py) Defaults

OCHRE's `ElectricVehicle` class uses these key defaults:

| Parameter | OCHRE Default | HARES Default |
|-----------|---------------|---------------|
| Charging efficiency | 0.90 | 0.90 |
| Fuel economy | 1/325*1000 = 3.077 mi/kWh | 0.325 kWh/mi |
| Default charging level | L2 | L2 |
| L1 max power | 1.4 kW | 1.4 kW |
| L2 max power (PHEV) | 3.6 kW | 3.6 kW |
| L2 max power (BEV100) | 7.2 kW | 7.2 kW |
| L2 max power (BEV250) | 11.5 kW | 11.5 kW |
| Default SOC | 1.0 (full) | 1.0 (full) |
| Default SOC max | 1.0 | 1.0 |

### Key Differences: OCHRE vs HARES

**OCHRE Event-Based Model:**
- Uses EVI-Pro charging probability density functions (PDFs) for:
  - Arrival time distribution
  - Arrival SOC distribution  
  - Parking duration distribution
- Forces overnight charging event every night
- Uses temperature and weekday to select appropriate event profile
- Calculates charging frequency based on vehicle capacity:
  - Large EVs (>70 kWh): charge every ~5 days
  - Medium EVs (35-70 kWh): charge every ~3 days
  - Small EVs/PHEVs (<35 kWh): charge every ~2 days
- Supports unmet load tracking when SOC doesn't reach target

**HARES Simplified Model:**
- No event-based schedule by default
- Uses actor system for charging strategies (see Section 4)
- No built-in EVI-Pro-style probability distributions
- Simpler continuous charging model

### Severity: LOW - Different modeling approaches

The two approaches are fundamentally different. OCHRE models driver behavior statistically, HARES models charging as controllable load. HARES approach is more suitable for smart charging/DER coordination.

---

## 3. HARES Wiring: Configuration Flow

### Configuration Path

```
HPXML Input
    ↓
resolve_ev() [resolve_der.rs:137-183]
    ↓
EvConfig (typed config)
    ↓
EV::new() [ev/mod.rs:124]
    ↓
EV::init_typed() [ev/mod.rs:242-359]
    ↓
Runtime EV instance
```

### Mapping Verification

The wiring is correctly implemented. The HPXML parser creates an `EvConfig` with:
- `capacity_kwh` from BatteryCapacity (or 75.0 default)
- `charging_level` from ChargingLevel (or "L2" default)
- `max_charging_power_kw` from MaxChargingPower (or 11.5 default)

All other fields remain `None`, and the `EV::init_typed()` method properly applies defaults from `config.rs` constants.

### Missing Configuration Mappings

| HARES Field | HPXML Source | Status |
|-------------|--------------|--------|
| `vehicle_type` | N/A | NOT MAPPED |
| `charging_strategy` | N/A | NOT MAPPED |
| `plug_in_policy` | N/A | NOT MAPPED |
| `initial_connection_state` | N/A | NOT MAPPED |
| `chemistry` | N/A | NOT MAPPED |

### Severity: LOW - Wiring is correct for what is parsed

The wiring is correct for the three attributes that are parsed. The others require explicit configuration.

---

## 4. Control Logic

### Supported Control Signals

HARES EV supports comprehensive control via `apply_control_unchecked()` in `ev/mod.rs:871-1000`:

| Control Signal | Description | Lines |
|----------------|-------------|-------|
| `PowerSetpoint` | Direct power control (kW), positive=charge, negative=discharge | 873-887 |
| `PowerLimit` | Maximum power cap from external source | 888-895 |
| `SOCTarget` | Target SOC with optional min/max bounds | 896-923 |
| `EvPlugIn` | Change connection state (Home, Away, Disconnected) | 924-946 |
| `EvDrive` | Simulate driving energy consumption | 947-965 |
| `EvAwayCharge` | Public charging power | 966-978 |
| `EvSetReadyBy` | Departure time + target SOC for BMS scheduling | 979-991 |

### Charging Strategies (from hares-types)

The `ChargingStrategy` enum (`equipment.rs:418-465`) defines:

| Strategy | Description | Implementation |
|----------|-------------|----------------|
| `Immediate` | Charge at max power immediately | Default, no actor needed |
| `Nightly` | Charge during off-peak hours | Requires TOU tariff integration |
| `LowSoc` | Start charging when SOC below threshold | Simple threshold-based |
| `QuickThenWait` | Fast charge to partial SOC, then wait | Two-phase |
| `PreDeparture` | Charge to target by departure time | Uses departure constraints |
| `TouAware` | TOU-optimized with departure constraints | Most sophisticated |
| `SolarSurplus` | Charge only with excess solar | Requires PV integration |
| `V2H` | Vehicle-to-Home discharge | Separate mode |
| `V2G` | Vehicle-to-Grid discharge | Separate mode |

### BMS Ready-By Scheduling

The EV implements smart scheduling (`ev/mod.rs:456-495`) that:
- Computes SOC deficit vs target
- Calculates hours until departure deadline
- Delays charging if sufficient time available
- Charges at max rate if deadline is near

### V2L/V2G Implementation

- V2L: `compute_v2l_discharge()` - Vehicle-to-load (e.g., home backup power)
- V2G: `compute_v2g_discharge()` - Vehicle-to-grid export
- Both respect SOC reserve floors and max discharge power limits

### Severity: LOW - Control logic is comprehensive

The control logic is well-implemented with all major strategies available. The only gap is that charging strategies cannot be specified via HPXML.

---

## 5. Output Ports & Telemetry

### Core Output (`CoreOutput`)

From `ev/mod.rs:751-761`:
```rust
CoreOutput {
    flows: CoreFlows {
        electric_kw: Some(ElectricPower::Bidirectional(active_power_kw)),
        reactive_power_kvar: None,
        fuel_w: None,
    },
    state: CoreState {
        operating_mode: Some(mode),  // Charging/Discharging/Off
        soc: Some(Soc::try_from(self.soc).ok()),
    },
}
```

### Telemetry Fields (14 total, from ev/telemetry.rs)

| Telemetry Key | Unit | Description |
|---------------|------|-------------|
| `soc` | - | Battery state of charge [0..1] |
| `active_power_kw` | kW | Grid-side charging power (+) or V2L/V2G discharge (-) |
| `connection_state` | code | 0=HomePluggedIn, 1=AwayPluggedIn, 2=Disconnected |
| `charging_level` | code | 1=L1, 2=L2 |
| `battery_temp_c` | C | Pack temperature for cold-charge derating |
| `heater_power_w` | W | Battery heater power when active |
| `charge_derate` | - | Temperature-based derate factor [0..1] |
| `v2l_active` | - | 1 if discharging to load, 0 otherwise |
| `v2l_power_kw` | kW | V2L discharge power magnitude |
| `capacity_fade_pct` | % | Cumulative capacity fade from degradation |
| `away_charge_power_kw` | kW | Non-residential charging power |
| `capacity_kwh` | kWh | Battery capacity |
| `fuel_economy_kwh_per_mi` | kWh/mi | For actor trip planning |

### Additional State Variables (internal)

- `battery_temp_c` - Battery pack temperature
- `heater_active` - Heater on/off state
- `v2l_power_kw` - Current V2L discharge
- `degradation` - Smith 2017 degradation state
- `rainflow` - Rainflow cycle counter for degradation

### Severity: LOW - Telemetry is comprehensive

All relevant metrics are exposed. Could add "departure_time" or "ready_by_hour" for debugging, but current set is sufficient.

---

## 6. Dwelling/Thermal Solver Aggregation

### Electrical Aggregation

EV integrates with dwelling electrical model via `PortContribution::Electrical`:

From `ev/mod.rs:707-712`:
```rust
if is_v2l_discharge || self.active_power_kw > 0.0 {
    ports.accumulate(&PortContribution::Electrical {
        active_power_kw: self.active_power_kw,
        reactive_power_kvar: 0.0,
    })?;
}
```

- Positive power → adds to `load_power_kw`
- Negative power (V2L/V2G) → adds to `generation_power_kw`

### Control Capabilities

From `ev/mod.rs:133-139`:
```rust
control_capabilities: ControlCapabilities::POWER_SETPOINT
    | ControlCapabilities::SOC_TARGET
    | ControlCapabilities::POWER_LIMIT
    | ControlCapabilities::EV_PLUG_IN
    | ControlCapabilities::EV_DRIVE
    | ControlCapabilities::EV_AWAY_CHARGE
    | ControlCapabilities::EV_SET_READY_BY,
```

### Actor System Integration

EV exposes `actor_seed()` method (`ev/mod.rs:773-784`) for the actor system:
- Non-Immediate strategies spawn an `EvDriverActor`
- Actor receives environment signals (price, solar, etc.)
- Actor issues control signals (SOCTarget, PowerSetpoint, etc.)

### Thermal Model

EV has simple thermal model:
- Battery temperature updates based on ambient + charging heat
- Heat loss: `q_loss_w = ua_w_per_k * (battery_temp_c - ambient_c)`
- Charging heat: ohmic losses + heater power
- Cold temperature derating reduces charging power

### No Direct Thermal Coupling

The EV does NOT contribute to dwelling thermal load:
- `zone_name = None` (no thermal zone)
- No sensible/latent gain fractions
- Battery heater power goes to electrical load, not thermal

### Severity: LOW - Aggregation is correct

EV correctly integrates with electrical aggregation. Could be enhanced to optionally contribute heater load to thermal model, but current behavior is appropriate for pure electrical simulation.

---

## Summary of Findings

### Issues Requiring Fix

| Issue | Severity | Description |
|-------|----------|-------------|
| VehicleType not parsed from HPXML | MEDIUM | Affects default power selection for PHEVs vs BEVs |
| ChargingStrategy not parsed from HPXML | LOW | Requires explicit config, not available in standard HPXML |
| PlugInPolicy not parsed from HPXML | LOW | Requires explicit config |
| No EVI-Pro-style event schedule | LOW | Different modeling approach than OCHRE - acceptable |

### What's Working Well

1. **Control logic is comprehensive** - All major strategies (Immediate, Nightly, TouAware, V2L, V2G, etc.) are implemented
2. **Telemetry is thorough** - 14 fields covering SOC, power, temperature, degradation
3. **V2L/V2G properly implemented** - Respects SOC reserves, power limits, connection state
4. **BMS scheduling works** - Ready-by deadlines properly delay or accelerate charging
5. **Actor system integration** - Non-Immediate strategies get proper actor coordination
6. **Checkpointing complete** - Full state serialization for simulation resilience

### Comparison: HARES vs OCHRE Physics

| Aspect | OCHRE | HARES | Verdict |
|--------|-------|-------|---------|
| Charging physics | CC-CV model with taper | CC-CV with taper + LUT support | HARES better (LUT option) |
| Default efficiency | 0.90 | 0.90 | Equal |
| Default power levels | EVI-Pro based by vehicle type | By charging level + capacity | Equal |
| Event-based schedules | EVI-Pro PDFs | Not implemented | OCHRE better (for driving patterns) |
| V2L/V2G | Not supported | Fully supported | HARES better |
| Temperature derating | Linear | Linear | Equal |
| Degradation | Not in EV model | Smith 2017 model | HARES better |
| Actor coordination | Not available | Full actor system | HARES better |

---

## Recommendations

1. **Consider parsing VehicleType from HPXML** - Would improve PHEV default power selection
2. **Document HPXML EV limitations** - Users should know ChargingStrategy requires explicit config
3. **No changes needed to physics** - HARES implementation is sound and in some areas superior to OCHRE

---

*End of Review*
