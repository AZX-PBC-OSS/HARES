# Load Equipment Review: HARES vs OCHRE

**Reviewer**: Code Review Agent  
**Date**: 2026-03-31  
**Scope**: HPXML parsing, OCHRE defaults, HARES wiring, control logic, output ports, thermal aggregation  
**Severity Key**: CRITICAL, HIGH, MEDIUM, LOW, INFO

---

## Executive Summary

This review covers load equipment implementation in HARES including:
- **ScheduledLoad**: Deterministic loads with time-indexed schedules
- **EventBasedLoad**: Stochastic loads with Idle/Active/Cooldown phases
- **WetAppliance**: Multi-phase cycle appliances (washer, dryer, dishwasher)
- **Lighting**: Indoor, exterior, garage, basement lighting
- **Appliances**: Refrigerator, freezer, cooking range, MELs, TV, well pump, etc.

Overall, HARES provides a well-structured, type-safe reimplementation of OCHRE loads with several improvements. However, some gaps and simplifications exist that should be documented.

---

## 1. HPXML Parsing

### 1.1 Attributes Parsed (from `crates/hares-io/src/hpxml/resolve_loads.rs`)

**Location**: Lines 22-565

| Equipment Type | Parsed Attributes | Notes |
|---------------|-------------------|-------|
| **Occupancy** | `number_of_occupants` | Via BuildingOccupancy |
| **Clothes Washer** | `annual_electric_kwh`, `capacity_m3`, `label_usage_cycles_per_week`, `imef`, `hot_water_draw_volume_l` (default 15.0L), `UsageMultiplier`, `FracSensible`, `FracLatent` | Includes complex rating-to-actual energy conversion (lines 122-159) |
| **Clothes Dryer** | `annual_electric_kwh`, `annual_gas_therms`, `combined_energy_factor`, `energy_factor`, `vented` (default true), `sensible_gain_fraction`, `latent_gain_fraction` | Handles vented/unvented, electric/gas (lines 161-228) |
| **Dishwasher** | `annual_electric_kwh`, `place_setting_capacity`, `label_usage_cycles_per_week`, `hot_water_draw_volume_l` (default 6.0L) | Rating-to-actual conversion (lines 230-272) |
| **Cooking Range** | `annual_electric_kwh`, `annual_gas_therms`, `IsInduction` | Defaults based on bedrooms (lines 273-299) |
| **Refrigerator** | `annual_electric_kwh` (default 637+18*beds) | Line 87 |
| **Freezer** | `annual_electric_kwh` (default 319.8) | Line 88 |
| **Lighting** | `annual_electric_kwh`, `month_multipliers`, location fractions (LED, CFL, fluorescent) | Derives from floor area if not explicit (lines 312-371) |
| **Ceiling Fan** | `count` (default beds+1), `annual_electric_kwh`, `month_multipliers` | Lines 373-413 |
| **MELs/TV/Well Pump** | `annual_electric_kwh`, schedule extension params | Lines 416-483 |
| **Gas Appliances** | `annual_gas_therms` for grill, fireplace, lighting | Lines 485-505 |
| **Pool/Spa** | `annual_electric_kwh`, `annual_gas_therms` | Lines 508-564 |
| **Ventilation Fan** | `RatedFlowRate`, `FanPower`, `FanType`, `SensibleRecoveryEfficiency`, `TotalRecoveryEfficiency`, `HoursInOperation` | Lines 567-640 |

**Schedule Extension Parameters Parsed** (lines 768-861):
- `WeekdayScheduleFractions` / `WeekendScheduleFractions` (24 values)
- `MonthlyScheduleMultipliers` (12 values)
- `UsageMultiplier`
- `FracSensible` / `FracLatent`

**Default Gain Fractions** (lines 642-678):
HARES correctly maps defaults for each equipment type including:
- Cooking Range: (0.64, 0.16) gas, (0.72, 0.08) electric
- Clothes Washer: (0.27, 0.03)
- Dishwasher: (0.30, 0.30)
- Refrigerator: (1.00, 0.00)
- Freezer: (0.00, 0.00) - note: placed in garage/basement, not conditioned zone
- MELs: (0.855, 0.045)
- TV: (0.00, 0.00) - no zone gains
- Lighting: (1.00, 0.00) - convective only

### 1.2 Findings

| Issue | Severity | Description |
|-------|----------|-------------|
| **Freezer zone assignment** | MEDIUM | Default gain fraction (0,0) means freezer contributes no thermal gains. This is OCHRE-consistent but could be configurable for garage modeling. |
| **No diversity factor support** | INFO | HPXML has no explicit diversity factor; HARES correctly mirrors OCHRE behavior. |

---

## 2. OCHRE Defaults/Fallbacks

### 2.1 Default Schedule Profiles (from `defaults/Default Schedule Parameters.csv`)

HARES loads default schedules from a comprehensive CSV file with 24-hour weekday/weekend fractions and 12-month multipliers.

**Key Default Schedules**:

| Equipment | Weekday Peak Hour | Weekend Peak Hour | Monthly Variation |
|-----------|-------------------|-------------------|-------------------|
| Indoor Lighting | 21:00 (0.110) | 21:00 (0.110) | Yes (0.80-1.20) |
| Exterior Lighting | 20:00 (0.070) | 20:00 (0.070) | Yes (0.80-1.20) |
| Refrigerator | 19:00 (0.050) | 19:00 (0.050) | Yes (0.84-1.10) |
| MELs | 19:00 (0.051) | 19:00 (0.051) | No |
| TV | 21:00 (0.132) | 21:00 (0.132) | No |
| Dishwasher | 20:00 (0.111) | 20:00 (0.111) | No |
| Clothes Washer | 9:00 (0.086) | 9:00 (0.086) | No |
| Clothes Dryer | 20:00 (0.081) | 20:00 (0.081) | No |
| Cooking Range | 18:00 (0.134) | 18:00 (0.134) | No |
| Ceiling Fan | 7:00-8:00, 19:00 | 7:00-8:00, 19:00 | No |
| Well Pump | 21:00 (0.086) | 21:00 (0.086) | No |
| Pool Pump | 12:00 (0.127) | 12:00 (0.127) | Yes (0.88-1.16) |
| Gas Grill | 18:00 (0.161) | 18:00 (0.161) | No |
| Gas Fireplace | 21:00 (0.086) | 21:00 (0.086) | No |

### 2.2 Equipment Default Values

**From `scheduled_load.rs`**:
- `sensible_gain_fraction`: defaults to 0.5 if not specified (line 262-268)
- `latent_gain_fraction`: defaults to 0.0 (line 270-273)
- ZIP coefficients: all zeros (pure constant power) (line 82-96)
- `load_fraction`: defaults to 1.0 (line 217)
- `mode_override`: defaults to None (line 218)
- `power_setpoint_override`: defaults to None (line 163)

**From `event_load.rs`**:
- `active_power_kw`: defaults to 1.0 (line 454)
- `active_duration_s`: defaults to 900.0 (15 min) (line 455)
- `cooldown_duration_s`: defaults to 0.0 (line 456-457)
- `sensible_gain_fraction`: defaults to 0.0 (line 459-466)
- `latent_gain_fraction`: defaults to 0.0 (line 463-466)
- Phase defaults for WetAppliance: 1.0 kW, 900.0s (line 714-717)

### 2.3 Fallback Hierarchy (from `schedule_resolve.rs`)

Priority for schedule resolution (lines 651-727):
1. **CSV column** - If schedule CSV has matching column, use it directly
2. **HPXML profile** - If HPXML has weekday/weekend fractions, use those
3. **Default profile** - From "Default Schedule Parameters.csv"
4. **Constant power** - annual_kwh / 8760 (with loud warning)

### 2.4 Findings

| Issue | Severity | Description |
|-------|----------|-------------|
| **Conservative sensible gain default** | LOW | ScheduledLoad defaults sensible_gain_fraction to 0.5 when not specified. HARES defaults to 0.0 for event-based loads. This is documented but could be surprising. |
| **Missing default for refrigerator schedule** | INFO | OCHRE has constant and temperature-coefficient schedules for refrigerator; HARES treats as regular power schedule. |

---

## 3. HARES Wiring: HPXML to Equipment

### 3.1 Mapping Pipeline

The wiring chain is:
1. **HPXML Parsing** (`resolve_loads.rs`) - Extracts annual_kwh, fuel type, gain fractions, schedules
2. **Schedule Resolution** (`schedule_resolve.rs`) - Converts annual energy + fractions to power schedules
3. **Equipment Init** (`scheduled_load.rs`, `event_load.rs`) - Creates and initializes equipment

**Key Config Keys** (from `scheduled_load.rs`):

| Config Key | Purpose | Example |
|------------|---------|---------|
| `power_schedule_source` | Source type: "column", "daily_profile", "constant" | "column" |
| `power_schedule_col` | CSV column index for column source | 5 |
| `power_profile_max_kw` | Peak kW for daily_profile | 0.5 |
| `power_profile_weekday` | 24-hour weekday fractions | [0.1, 0.2, ...] |
| `power_profile_weekend` | 24-hour weekend fractions | [...] |
| `power_profile_month` | 12-month multipliers | [1.0, 1.0, ...] |
| `power_constant_kw` | Constant power for constant source | 0.1 |
| `sensible_gain_fraction` | Fraction of power as sensible heat | 0.72 |
| `latent_gain_fraction` | Fraction of power as latent heat | 0.08 |
| `month_multipliers` | Per-month scaling factors | [0.8, 1.0, ...] |
| `usage_multiplier` | Overall energy multiplier | 1.2 |

**For Event-Based Loads** (from `event_load.rs`):

| Config Key | Purpose |
|------------|---------|
| `event_window_schedule_col` | Column index for event window |
| `event_window_source` | "constant" fallback |
| `event_power_kw_series` | Pre-extracted kW time series |
| `active_power_kw` | Default active power |
| `active_duration_s` | Event duration |
| `cooldown_duration_s` | Cooldown phase duration |
| `phase_N_power_kw`, `phase_N_duration_s` | Multi-phase cycle parameters |
| `n_units` | Number of parallel units |

### 3.2 Findings

| Issue | Severity | Description |
|-------|----------|-------------|
| **No standby power modeling** | MEDIUM | OCHRE has standby power for refrigerators (~1-5W). HARES uses pure schedule-based approach. No cycling/duty cycle modeling for refrigeration. |
| **No diversity factor in wiring** | INFO | Diversity factors are implicit in schedule resolution. Could be made explicit for large-scale simulations. |
| **Event extraction efficiency** | LOW | HARES extracts events from kW series in init (line 502-512). For long simulations with many events, this could be optimized. |

---

## 4. Control Logic

### 4.1 ScheduledLoad Control

**Control Signals Accepted** (`apply_control_unchecked`, lines 608-633):
- `LoadFraction`: Scales power (applied to both electric AND gas)
- `ModeOverride`: Forces Off/Standby
- `PowerSetpoint`: Per-step absolute power override

**Control Logic** (`step`, lines 349-534):
1. Check grid voltage - zero output if voltage_pu == 0
2. Check mode override - force zero if Off
3. Apply PowerSetpoint if present (per-step, cleared after)
4. Otherwise compute: `raw_schedule_kw * load_fraction * month_scale`
5. Apply ZIP model if configured
6. Compute thermal gains from total input energy

### 4.2 EventBasedLoad Control

**Control Signals** (lines 645-683):
- `LoadFraction`: Scales power during active phase
- `ModeOverride`: Forces Idle or Active
- `PowerSetpoint`: Overrides active_power_kw during active phase
- `EventDelay`: Delays event start (only when Idle)

**State Machine** (lines 272-355):
- **Idle**: Waiting for event trigger
- **Active**: Running at active_power_kw
- **Cooldown**: Optional post-event cooldown phase

### 4.3 WetAppliance Control

**Multi-phase cycles** (lines 739-814):
- Cycles through configured phases (e.g., wash, rinse, spin)
- Supports hot water draw to DHW loop
- Stochastic event starts based on probability schedule

### 4.4 Findings

| Issue | Severity | Description |
|-------|----------|-------------|
| **No occupancy-based control** | MEDIUM | HARES has no explicit occupancy-based load control. Occupancy is available in environment but not used for load modulation. OCHRE has limited occupancy coupling. |
| **No appliance cycling/duty cycle** | MEDIUM | Refrigerator cycling (compressor on/off) not modeled. OCHRE has duty cycle capability. |
| **EventDelay limited** | LOW | Can only delay when in Idle phase. No ability to reschedule active events. |

---

## 5. Output Ports & Telemetry

### 5.1 ScheduledLoad Telemetry (`scheduled_load.rs`, lines 774-802)

| Telemetry Field | Unit | Description |
|-----------------|------|-------------|
| `electric_kw` | kW | ZIP-adjusted active electrical power |
| `reactive_power_kvar` | kVAR | ZIP-adjusted reactive power |
| `sensible_gain_w` | W | Sensible thermal gain to zone |
| `latent_gain_w` | W | Latent thermal gain to zone |
| `fuel_input_w` | W | Gas fuel consumption in watts |

### 5.2 EventBasedLoad Telemetry (`event_load.rs`)

**Event Load**:
| Field | Unit | Description |
|-------|------|-------------|
| `active_power_kw` | kW | Electric power during active phase |
| `sensible_gain_w` | W | Sensible thermal gain |
| `latent_gain_w` | W | Latent thermal gain |
| `fuel_input_w` | W | Gas fuel consumption |
| `state` | - | 0=Idle, 1=Active, 2=Cooldown |

**WetAppliance**:
| Field | Unit | Description |
|-------|------|-------------|
| `active_power_kw` | kW | Electric power |
| `sensible_gain_w` | W | Sensible thermal gain |
| `latent_gain_w` | W | Latent thermal gain |
| `fuel_input_w` | W | Gas fuel consumption |
| `cycle_phase` | - | 0=idle, 1+=phase index |

### 5.3 Core Output

All loads produce `CoreOutput` with:
- `electric_kw`: ElectricPower::Consumption (positive) or Generation
- `reactive_power_kvar`: Optional for ZIP-modeled loads
- `fuel_w`: Optional FuelPower for gas equipment
- `operating_mode`: None for loads (no SOC, no mode tracking)
- `soc`: None for loads

### 5.4 Port Contributions

**Electrical Port**: Accumulates active_power_kw + reactive_power_kvar  
**Fuel Port**: Accumulates consumption_w by fuel type  
**Thermal Port**: Accumulates sensible_gain_w + latent_gain_w by zone

### 5.5 Findings

| Issue | Severity | Description |
|-------|----------|-------------|
| **No per-phase energy tracking** | INFO | WetAppliance doesn't report energy per phase, only current power. Could add for diagnostics. |
| **No cycling state telemetry** | INFO | Refrigerator cycling not modeled, so no on/off state telemetry. |

---

## 6. Dwelling/Thermal Solver Aggregation

### 6.1 Zone Assignment

HARES uses convention-based zone assignment (`scheduled_load.rs`, lines 174-189):

| Equipment Name Pattern | Zone Assignment |
|------------------------|-----------------|
| Contains "exterior" or "outdoor" | None (outdoor) |
| Contains "garage" | ZoneId(2) - Garage |
| Contains "basement" | ZoneId(3) - Foundation |
| Default (indoor) | ZoneId(1) - Conditioned |
| EV charging | None (outside envelope) |

### 6.2 Thermal Gains Aggregation

**ScheduledLoad** (lines 491-503):
```
total_gain_source_w = electric_power_kw * 1000 + gas_consumption_w
sensible_gain_w = total_gain_source_w * sensible_gain_fraction
latent_gain_w = total_gain_source_w * latent_gain_fraction

ports.accumulate(Thermal { zone, sensible_gain_w, latent_gain_w, category: InternalGain })
```

**EventBasedLoad** (lines 384-411):
- Similar calculation using `gain_source_w = active_power_kw * 1000`
- Supports both electric and fuel thermal contributions
- Category is `ThermalCategory::InternalGain`

### 6.3 Findings

| Issue | Severity | Description |
|-------|----------|-------------|
| **No radiant/convective split** | INFO | OCHRE has `convective_gain_fraction` and `radiative_gain_fraction` reserved but not used. HARES correctly mirrors this. |
| **Freezer not contributing to conditioned zone** | INFO | Default gain fractions (0,0) for freezer means no contribution to thermal model. This is OCHRE-consistent (garage/basement placement). |
| **No hot water waste heat recovery** | INFO | WetAppliance draws hot water but doesn't model waste heat from water heating to zone. OCHRE has some capability here. |

---

## 7. Comparison: HARES vs OCHRE

### 7.1 Where HARES Physics is Better

| Feature | HARES Improvement |
|---------|-------------------|
| **Type safety** | Rust type system prevents many runtime errors |
| **ZIP model** | Full ZIP coefficients (Z, I, P + reactive) with validation |
| **Checkpointing** | Explicit state serialization with postcard |
| **Port system** | Type-safe port contributions with accumulation |
| **Config validation** | Explicit validation of gain fraction sums |
| **Monthly multipliers** | Proper monthly scaling with day-weighted averages |

### 7.2 Where HARES Has Gaps/Simplifications

| Feature | OCHRE Capability | HARES Gap |
|---------|------------------|-----------|
| **Refrigerator cycling** | Duty cycle / on-off cycling | Pure schedule-based |
| **Standby power** | Explicit standby power for some appliances | Not explicitly modeled |
| **Occupancy coupling** | Some loads modulated by occupancy | No explicit coupling |
| **WetAppliance stochastic** | Full PDF-based event generation | Simplified probability + window |
| **Solar-aware loads** | Loads that respond to PV production | Not implemented for loads |

### 7.3 Findings

| Issue | Severity | Description |
|-------|----------|-------------|
| **OCHRE parity for wet appliances** | HIGH | WetAppliance in HARES uses simplified event extraction vs OCHRE's full MC Profile Generator. The OCHRE `WetAppliance.py` has more sophisticated stochastic modeling. |
| **No PV-solar-aware loads** | MEDIUM | ScheduleSource::SolarAware exists but loads don't use it. Could enable load shifting based on PV production. |

---

## 8. Summary

### Strengths
1. Comprehensive HPXML parsing with correct defaults mirroring OCHRE
2. Well-structured default schedule profiles from standards (ANSI/RESNET 301)
3. Proper ZIP model implementation with validation
4. Type-safe equipment registry and configuration
5. Good thermal zone assignment logic
6. Proper month multiplier handling

### Issues Requiring Attention
1. **HIGH**: WetAppliance stochastic modeling is simplified vs OCHRE
2. **MEDIUM**: No refrigerator cycling/duty cycle modeling
3. **MEDIUM**: No occupancy-based load modulation
4. **LOW**: Conservative sensible gain defaults differ between ScheduledLoad (0.5) and EventBasedLoad (0.0)

### Recommendations
1. Consider adding refrigerator duty cycle modeling for better thermal accuracy
2. Add optional occupancy coupling for demand response studies
3. Document the wet appliance simplification for users coming from OCHRE
4. Add explicit standby power defaults for relevant appliances

---

## File Locations Referenced

- `crates/hares-io/src/hpxml/resolve_loads.rs` - HPXML parsing
- `crates/hares-io/src/schedule_resolve.rs` - Schedule resolution
- `crates/hares-equipment/src/scheduled_load.rs` - ScheduledLoad implementation
- `crates/hares-equipment/src/event_load.rs` - EventBasedLoad + WetAppliance
- `defaults/Default Schedule Parameters.csv` - Default schedules
- `vendors/OCHRE/ochre/Equipment/ScheduledLoad.py` - OCHRE reference
- `vendors/OCHRE/ochre/Equipment/EventBasedLoad.py` - OCHRE reference
- `vendors/OCHRE/ochre/Equipment/WetAppliance.py` - OCHRE reference
