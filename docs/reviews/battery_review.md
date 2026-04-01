# Battery Equipment Review - HARES

**Review Date**: 2026-03-31  
**Reviewer**: Code Review Agent  
**Scope**: HPXML parsing, OCHRE defaults/fallbacks, HARES wiring, control logic, telemetry, dwelling integration

---

## 1. HPXML Parsing

### Attributes Parsed from HPXML

| HPXML Attribute | HARES Config Field | Notes |
|-----------------|-------------------|-------|
| `NominalCapacity` | `capacity_kwh` | kWh, default 13.5 if missing |
| `RatedPowerOutput` | `max_charge_kw`, `max_discharge_kw` | Assumed symmetric, default 5.0 |
| `RoundTripEfficiency` | `inverter_efficiency` | Converted via `sqrt()` to one-way efficiency |

**Coverage**: Basic attributes only. Missing from HPXML parsing:
- `chemistry` - not available in HPXML, correctly defaults to NMC
- `min_soc`, `max_soc` - HPXML has no equivalent, correctly uses defaults
- `initial_soc` - HPXML has no equivalent
- `standby_power_w`, `self_discharge_pct_per_day` - not in HPXML
- Heater config, thermal model params - not in HPXML

**Issue**: The HPXML resolver only maps 3 attributes. This is acceptable as HPXML schema is limited, but users cannot override critical battery parameters through HPXML.

---

## 2. OCHRE Defaults/Fallbacks

### OCHRE Default Parameters (`defaults/Battery/default_parameters.csv`)

| Parameter | OCHRE Default | HARES Default | Comparison |
|-----------|--------------|---------------|------------|
| `capacity_kwh` | 10.0 | 13.5 | HARES is more realistic (Powerwall-class) |
| `capacity` (power) | 5.0 kW | 5.0 kW | Matches |
| `soc_init` | 0.5 | 0.5 | Matches |
| `soc_max` | 0.95 | 0.95 | Matches |
| `soc_min` | 0.15 | 0.15 | Matches |
| `efficiency_charge` | 0.98 | sqrt(0.97) = 0.985 | HARES slightly higher |
| `efficiency` (discharge) | 0.98 | sqrt(0.97) = 0.985 | HARES slightly higher |
| `efficiency_inverter` | 0.97 | 0.97 (round-trip) | Matches |
| `discharge_pct` | 0% per day | 0% per day | Matches |
| `v_cell` | 3.6 V | 3.6 V (NMC OCV table) | HARES uses lookup tables |
| `ah_cell` | 70 Ah | 50 Ah (NMC default) | HARES uses catalog values |
| `r_cell` | 0.002 ohm | 0.005 ohm | HARES is more conservative |
| `thermal_r` | 0.5 K/W | 5.0 W/K (UA) | OCHRE uses R, HARES uses UA |
| `thermal_c` | 90000 J/K | 90000 J/K | Matches |

### OCHRE Fallback Logic

1. **Capacity**: Required param, no fallback
2. **Power**: Uses `capacity` as max power (both charge/discharge)
3. **Efficiency**: Uses constant efficiency model unless `efficiency_type="advanced"` passed
4. **SOC bounds**: Hardcoded in code as 0.15/0.95 if not in params
5. **Thermal model**: Only enabled if `zone_name` is provided

### HARES Improvements over OCHRE

1. **Better default capacity**: 13.5 kWh vs 10 kWh reflects actual residential products
2. **OCV-based voltage model**: HARES uses full SOC-OCV lookup tables vs OCHRE's optional advanced model
3. **Temperature derating**: HARES has explicit `CapacityDerateModel` with Arrhenius or piecewise-linear
4. **Charging curve LUT**: HARES supports 4D charging curve lookup tables from PyBaMM
5. **Product catalog**: HARES includes 11 pre-defined battery specs (Tesla PW3, Enphase IQ5P, etc.)
6. **Standalone BMS modes**: HARES has full `BatteryManagementActor` for autonomous control

---

## 3. HARES Wiring

### Configuration Flow

```
HPXML → resolve_batteries() → BatteryConfig → Battery::new() → Battery::init_typed()
```

**Mappings Verified**:

| HPXML | BatteryConfig | Battery struct field | Status |
|-------|--------------|---------------------|--------|
| `NominalCapacity` | `capacity_kwh` | `capacity_kwh` | ✓ Correct |
| `RatedPowerOutput` | `max_charge_kw`, `max_discharge_kw` | `max_charge_kw`, `max_discharge_kw` | ✓ Correct |
| `RoundTripEfficiency` | `inverter_efficiency` (sqrt) | `charge_efficiency`, `discharge_efficiency` | ✓ Correct |

**Missing/Defaulted Fields**:
- `chemistry`: Not in HPXML → defaults to NMC, can be overridden via typed config
- `min_soc`/`max_soc`: Not in HPXML → defaults 0.15/0.95 from constants
- `n_series`/`n_parallel`: Not in HPXML → derived from `ah_cell`/`v_cell` if provided, else defaults 96/1
- All thermal/heater params: Not in HPXML → use HARES defaults

**Wiring Assessment**: The wiring is correct. HPXML parsing is limited by schema design, but all parsed values correctly map to typed config. Users needing more control should use typed config directly.

---

## 4. Control Logic

### BMS Modes Supported

| Mode | HARES Implementation | OCHRE Equivalent |
|------|---------------------|------------------|
| `Manual` | Default - no autonomous control | Default |
| `SelfConsumption` | Discharges on net load, charges on surplus | `self_consumption_mode` |
| `TimeOfUseOptimization` | Charge on low price, discharge on high price | Not implemented |
| `BackupReserve` | Maintains target SOC, optional grid charging | Not implemented |
| `DemandResponse` | Wraps another mode with DR constraints | Not implemented |
| `Scheduled` | Time-window based charge/discharge/hold | Schedule input |
| `StormWatch` | Maintains high SOC for outage | Not implemented |

### Control Signals Handled

From `ControlSignal` enum, Battery handles:

| Signal | Handler | Notes |
|--------|---------|-------|
| `PowerSetpoint` | `apply_control_unchecked` | Direct power target |
| `SOCTarget` | `apply_control_unchecked` | SOC-based control |
| `GridConnect` | `apply_control_unchecked` | Island/disconnect |
| `SelfConsumption` | `apply_control_unchecked` | Enable self-consumption mode |
| `PowerLimit` | `apply_control_unchecked` | External limit ceiling |
| `DemandResponse` | `apply_control_unchecked` | DR level + duration |

### Charge/Discharge Logic

1. **Priority order** (in `determine_target_power`):
   - Explicit power setpoint
   - SOC target (proportional controller)
   - Self-consumption (net load based)
   
2. **Power clamping** (in `clamp_power`):
   - Hardware limits (max_charge/discharge_kw)
   - Temperature derating (via `CapacityDerateModel`)
   - Demand response fraction
   - Import/export limits
   - Charging curve LUT if present
   - External power limit

3. **SOC Management**:
   - Tracks `soc` state (0-1 normalized)
   - Clamps to `min_soc`/`max_soc` after each step
   - Supports `soc_target_min`/`max` for operational windows

### BMS Mode Control Assessment

**Good**: Complete BMS mode implementation with all major modes  
**Issue**: TOU optimization requires external price schedule - not self-contained

---

## 5. Output Ports & Telemetry

### Output Ports

| Port | Type | Purpose |
|------|------|---------|
| Electrical | `PortDeclaration::electrical()` | Grid power (positive=charge, negative=discharge) |
| Thermal (optional) | `PortDeclaration::thermal(zone)` | Ohmic losses as internal gain to zone |

### Telemetry Fields

From `telemetry_keys.rs` and `battery/mod.rs`:

| Telemetry Key | Description | Units |
|---------------|-------------|-------|
| `soc` | State of charge | fraction (0-1) |
| `active_power_kw` | Net power including standby/heater | kW |
| `ohmic_loss_w` | I²R losses in cells | W |
| `standby_power_w` | BMS/inverter parasitic draw | W |
| `cell_temp_c` | Cell temperature | °C |
| `heater_power_w` | Cell heater power | W |
| `discharge_derate` | Temperature-based power derate | fraction |
| `capacity_derate` | Temperature-based capacity derate | fraction |
| `cycle_count` | Total rainflow-accumulated cycles | count |
| `capacity_fade_pct` | Degradation-based capacity loss | % |
| `terminal_voltage_v` | Pack terminal voltage | V |
| `current_a` | Pack current | A |

### Core Output

```rust
CoreOutput {
    flows: CoreFlows {
        electric_kw: Some(ElectricPower::Bidirectional(port_power_kw)),
        reactive_power_kvar: None,
        fuel_w: None,
    },
    state: CoreState {
        operating_mode: Some(self.mode),  // Charging/Discharging/Standby/Off
        soc: Soc::try_from(self.soc).ok(),
    },
}
```

### Checkpointing

Full state serialized in `BatteryCheckpoint`:
- SOC, cell temperature, heater state, mode
- Degradation state, rainflow counter
- Control state (setpoints, targets, limits)
- DR state

**Telemetry Coverage**: Comprehensive - all important electrical, thermal, and degradation metrics are exposed.

---

## 6. Dwelling/Thermal Solver Aggregation

### Integration Points

1. **Electrical Aggregation**:
   - Battery accumulates to `PortContribution::Electrical` on the electrical port
   - Power includes: actual charge/discharge + standby power + heater power
   - Positive = consumption (grid import), negative = generation (grid export)

2. **Thermal Integration**:
   - Optional thermal port if `zone_id` is configured
   - Only ohmic losses (I²R) go to thermal zone - heater energy enters via thermal model
   - Uses `ThermalCategory::InternalGain`

3. **Self-Consumption Control**:
   - Reads `net_load_kw` from Stage 1 accumulator (base load - PV)
   - Charges when net load < 0 (PV surplus)
   - Discharges when net load > 0 (load > PV)

4. **BMS Actor Communication**:
   - `BatteryManagementActor` reads `env.electrical` for PV and load
   - Dispatches control signals to battery based on mode logic
   - Supports grid export rules: `Unrestricted`, `Disabled`, `SolarOnly`

### Load Shifting / Self-Consumption

- **Self-consumption mode**: Automatically shifts load by storing PV surplus
- **TOU optimization**: Schedules charge during off-peak, discharge during peak
- **Backup reserve**: Keeps battery at target SOC for outage protection

**Assessment**: Battery integrates well with dwelling - electrical port aggregates with house load, optional thermal port adds internal gains, BMS actor provides autonomous control.

---

## Summary of Findings

### Issues Requiring Fix

| Severity | Issue | Location | Recommendation |
|----------|-------|----------|----------------|
| **Medium** | HPXML parsing limited to 3 attributes | `resolve_der.rs:83-135` | Document that HPXML cannot override advanced params; users must use typed config |
| **Low** | Default max_charge equals max_discharge (symmetric) | `resolve_der.rs:98-99` | Consider asymmetric defaults (e.g., 5kW charge, 11kW discharge for Powerwall 3) |

### Comparison: HARES vs OCHRE

| Aspect | HARES | OCHRE | Winner |
|--------|-------|-------|--------|
| Default capacity | 13.5 kWh | 10 kWh | HARES |
| Default power | 5 kW symmetric | 5 kW | Tie |
| OCV model | Full lookup tables | Optional advanced | HARES |
| Temperature derating | Arrhenius + piecewise | Basic | HARES |
| Charging curve LUT | PyBaMM-generated 4D | None | HARES |
| BMS modes | 7 modes fully implemented | 2 (manual + schedule) | HARES |
| Product catalog | 11 specs (Tesla, Enphase, etc.) | None | HARES |
| Thermal model | RC lumped, optional | RC lumped, optional | Tie |
| Degradation | Rainflow + Arrhenius | Rainflow + Arrhenius | Tie |
| HPXML support | Basic 3-attribute | Unknown | Unknown |

### Conclusion

**HARES battery implementation is superior to OCHRE** in almost every aspect:
- More realistic defaults (13.5 kWh vs 10 kWh)
- More sophisticated electrical model (OCV-based vs simple efficiency)
- Comprehensive BMS mode support
- Product catalog with real-world specs
- Optional PyBaMM integration for physics-based modeling

The HPXML parsing is intentionally limited by the HPXML schema - users needing advanced configuration must use HARES typed config directly. This is by design and not a bug.

**Overall Assessment**: Production-ready with minor documentation improvements needed.
