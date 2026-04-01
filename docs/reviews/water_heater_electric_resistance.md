# Electric Resistance Water Heater (ERWH) Configuration Review

**Review Date**: March 31, 2026  
**Reviewer**: HARES Code Review Agent  
**Scope**: HPXML parsing, OCHRE defaults, HARES wiring, control logic, telemetry, and dwelling integration

---

## 1. HPXML Parsing: What HPXML attributes are parsed for ER water heater?

### Attributes Parsed (from `resolve_water_heater.rs`)

| HPXML Field | HARES Config Field | Notes |
|------------|-------------------|-------|
| `TankVolume` | `tank_volume_m3` | Converted from gallons with 0.9× correction factor for electric |
| `TankHeight` | `tank_height_m` | Optional, defaults to 4 ft if absent |
| `EnergyFactor` | `energy_factor` / `ua_w_per_k` | Used to derive UA via Burch & Erickson method |
| `UniformEnergyFactor` | `uniform_energy_factor` | Used for UEF path, converted to equivalent EF |
| `HeatingCapacity` | `element_power_w` / `heating_capacity_w` | Converted from BTU/hr to watts |
| `HotWaterTemperature` | `setpoint_c` | Temperature setpoint |
| `FirstHourRating` | `first_hour_rating_m3` | Used for UEF draw bin selection |

### Missing / Unparsed HPXML Attributes

1. **WaterHeaterInsulation/Jacket/JacketRValue** - Not parsed for ERWH. The HPXML schema supports tank insulation R-value but HARES ignores it. OCHRE parses this field (line 1040 in `ochre/utils/hpxml.py`). **Issue: Missing feature.**

2. **RecoveryEfficiency** - Not parsed for electric resistance. While technically not applicable (electric resistance has 100% recovery efficiency), this is inconsistent with gas water heater parsing.

3. **PerformanceAdjustment** - Parsed but applied differently than OCHRE. OCHRE applies this to the efficiency calculation; HARES stores it but doesn't use it for ERWH physics.

---

## 2. OCHRE Defaults: What defaults does OCHRE use for ERWH?

### OCHRE Default Values (from `WaterHeater.py` and `Water.py`)

| Parameter | OCHRE Default | HARES Default | Comparison |
|-----------|---------------|---------------|------------|
| Capacity | 4500 W (line 20) | 4500 W | **Matches** |
| Deadband | 5.56°C (10°F) (line 76) | 5.555...°F (line 42 in resistance.rs) | **Matches** (exact) |
| Efficiency | 1.0 (line 66) | N/A (always 1.0 for ER) | **Matches** |
| Setpoint | 51.67°C (125°F) (via convert(125, "degF", "degC")) | 51.667°C (line 15 in mod.rs) | **Matches** |
| Max Tank Temp | 60°C (140°F) (line 74) | 60°C (line 22 in mod.rs) | **Matches** |
| Tank Volume | 0.189 m³ (50 gal) (implied from defaults) | 0.189 m³ (line 21 in mod.rs) | **Matches** |
| Tank Height | 1.2 m (~4 ft) | 1.2 m (line 17 in mod.rs) | **Matches** |
| UA | 2.0 W/K (residential average) | 2.0 W/K (line 16 in mod.rs) | **Matches** |
| Conductivity | 0.6 W/m·K | 0.6 W/m·K (line 19 in mod.rs) | **Matches** |
| Tank Nodes | 2 (default in WaterHeater.__init__) | 6 (line 112-113 in resistance.rs) | **Differs** |

### Key Differences

1. **Tank Nodes Default**: OCHRE defaults to 2 nodes for ideal capacity mode. HARES defaults to 6 nodes. This affects:
   - Stratification resolution (HARES is more detailed)
   - Thermal mass calculation
   - Control accuracy (more nodes = more precise thermostat control)

2. **Element Priority**: OCHRE uses upper element priority implicitly in thermostat control. HARES explicitly models `ElementPriorityMode::MasterSlave` (default) with upper element locking out lower element when active.

---

## 3. HARES Wiring: How is ERWH configuration wired from HPXML to equipment?

### Wiring Flow

```
HPXML Input → resolve_water_heaters() → ElectricResistanceWaterHeaterConfig → EquipmentSpec → ResistanceWH::init()
```

### Key Wiring Points (`resolve_water_heater.rs`, lines 116-143)

1. **Fuel Type**: Electric is implicit from HPXML `<FuelType>electricity</FuelType>`
2. **Tank Volume**: Converted from gallons → m³ with 0.9× correction
3. **Tank Height**: Defaults to 4 ft if not specified
4. **Energy Factor / UEF**: Used to compute UA via `ua_from_energy_factor()`
5. **Heating Capacity**: Passed as `element_power_w` for ERWH
6. **Setpoint**: Parsed from `HotWaterTemperature`
7. **Draw Profile**: `avg_water_draw_l_per_day` computed from bedroom count and fixture efficiency

### Config Initialization (`resistance.rs`, lines 248-327)

1. Zone assignment from config or default ZoneId(1)
2. Loop ID from config or default LoopId(1)  
3. Tank geometry computed from volume/height
4. UA value from config or default (2.0 W/K)
5. Element power from config or default (4500 W)
6. Setpoint, deadband, max temp from config or defaults
7. Element priority mode (MasterSlave default)

### Issue: No insulation R-value wiring

The `WaterHeaterInsulation` HPXML element is not wired. This is a **missing feature** compared to OCHRE.

---

## 4. Control Logic: How does ERWH control work?

### Thermostat Control (`resistance.rs`, lines 343-405)

**Control Flow**:
1. Safety cutout: If any node exceeds `max_tank_temp_c`, force both elements off
2. DR/override check: If `dr_load_fraction <= 0` or `mode_override == Off`, force off
3. Thermostat calls computed via hysteresis:
   - Upper element: Compare upper node temp to `effective_setpoint_c`
   - Lower element: Compare averaged lower node temp (when n_nodes >= 3) to `effective_setpoint_c`
4. Element priority applied:
   - **MasterSlave (default)**: Upper element locks out lower element
   - **Simultaneous**: Both elements operate independently

### Hysteresis Logic (`mod.rs`, lines 186-198)

```
Off → On:  temperature < setpoint - deadband
On → Off:  temperature >= setpoint
```

Uses strict `<` comparison matching OCHRE's behavior.

### Demand Response Levels (`resistance.rs`, lines 690-714)

| DR Level | Setpoint Offset | Load Fraction |
|----------|----------------|---------------|
| Normal | 0°C | 1.0 |
| Moderate | -3°C | 1.0 |
| High | -6°C | 0.8 |
| Critical | -10°C | 0.5 |
| GridEmergency | 0°C | 0.0 |

### Element Staging

- **Upper element priority**: Heats top of tank first (hot water exits here)
- **Lower element**: Activates when upper is satisfied but lower portion of tank is below deadband
- This matches typical residential electric water heater wiring

### Control Signals Supported

- `ThermalSetpoint`: Set target temperature (with optional ramp rate)
- `DutyCycle`: Control element on/off fraction
- `ModeOverride`: Force Off/On mode
- `LoadFraction`: Transient load reduction (doesn't persist like DR)
- `PowerLimit`: Cap element power via `ctrl_load_fraction`
- `DemandResponse`: Apply DR level with optional duration

### Ideal Capacity Mode (`resistance.rs`, lines 418-446)

For timesteps >= 5 minutes, HARES computes ideal capacity matching OCHRE's `WaterHeater.solve_ideal_capacity()`:
- Predicts temperature with heater OFF
- Computes heat needed to reach setpoint
- Converts to duty cycle for time-averaged power
- Avoids on/off cycling spikes at coarse timesteps

**Issue**: OCHRE calculates ideal capacity for the lower node specifically (to achieve lower node setpoint), but HARES calculates for the active element's node. This is actually **more accurate** physically since each element only heats its local node.

---

## 5. Output Ports & Telemetry: What state/telemetry is reported?

### Port Declarations (`resistance.rs`, lines 153-158)

| Port Type | Description |
|-----------|-------------|
| Electrical | Active power draw (kW), reactive power (kVAR) |
| Thermal | Jacket loss to zone (sensible gain, category: JacketLoss) |
| Fluid (loop_id) | DHW distribution: flow rate, supply/return temps |
| Fluid (DHW_DEMAND_LOOP) | Input from wet appliances |

### Telemetry Fields (`resistance.rs`, lines 740-790)

| Field | Unit | Description |
|-------|------|-------------|
| `tank_avg_temp_c` | °C | Volume-weighted average tank temperature |
| `upper_element_power_w` | W | Upper element electric power |
| `lower_element_power_w` | W | Lower element electric power |
| `electric_kw` | kW | Total electric draw |
| `electric_power_w` | W | Total electric draw (duplicate, but W units) |
| `draw_flow_rate_kg_s` | kg/s | DHW draw flow rate (schedule + appliance demand) |
| `operating_mode` | enum | 0=Off, 1=Heating |
| `tank_node_X_c` | °C | Individual node temperatures (0 to n_nodes-1) |
| `skin_loss_w` | W | Tank jacket heat loss to zone |

### Core Output (`resistance.rs`, lines 538-550)

- `electric_kw`: Consumption power
- `operating_mode`: Heating/Off state

---

## 6. Dwelling/Thermal Solver Integration: How does it integrate with dwelling?

### Hot Water Demand Integration

1. **Schedule-based draw**: `draw_flow_rate_kg_s` from schedule column
2. **Appliance demand**: Wet appliances (washer, dishwasher) emit DHW demand to `DHW_DEMAND_LOOP` fluid port
3. **Total draw**: `schedule_draw + appliance_demand` (lines 476-478)

### Integration Points

1. **Fluid Port**: Water heater reads from DHW loop, adds to schedule-based draw
2. **Thermal Port**: Jacket losses (skin loss) emitted as sensible gain to conditioned zone
3. **Zone Temperature**: Used for standby loss calculation (ambient temp for UA losses)
4. **Mains Temperature**: From weather data or schedule, affects cold water inlet temp

### Internal Gains to Zone (`resistance.rs`, lines 507-518)

- Jacket losses (skin loss) → thermal port → zone sensible gain
- Element inefficiency heat (electric_kw - delivered_heat) → implicit in electric port (not separately counted as internal gain)

### Draw Resolution (`mod.rs`, lines 61-91)

Priority for mains temperature:
1. Weather data `mains_temp_c`
2. Schedule column
3. Default (15°C)

Priority for draw rate:
1. Schedule column (kg/s)
2. Config default

---

## Summary: OCHRE vs HARES Comparison

### Where HARES Physics is Better

1. **Node resolution**: HARES defaults to 6 nodes vs OCHRE's 2. More accurate stratification modeling.

2. **Element-specific ideal capacity**: HARES calculates ideal capacity per-element node rather than using lower node for entire tank - more physically accurate.

3. **End-cap UA**: HARES properly adds end-cap UA to top/bottom nodes (lines 157-166 in tank.rs), matching EnergyPlus methodology.

4. **DR levels**: More granular DR support with 5 levels vs OCHRE's 3.

5. **ZIP voltage model**: Built-in support for voltage-dependent power (though defaults to constant power).

6. **Element priority modes**: Explicit MasterSlave vs Simultaneous modes.

### Where HARES Has Issues/Bad Defaults

1. **Missing Jacket R-value parsing** - HPXML's `WaterHeaterInsulation/Jacket/JacketRValue` is not parsed. OCHRE handles this.

2. **Performance adjustment not applied** - HPXML `PerformanceAdjustment` is stored but not used in ERWH efficiency calculations.

3. **Default tank nodes (6 vs 2)** - While more accurate, this changes default behavior from OCHRE. Should be configurable.

4. **No recovery efficiency** - Electric resistance should implicitly have 100% recovery efficiency, but this isn't explicitly modeled (not a problem, just noting).

### Issues Summary

| Issue | Severity | Description |
|-------|----------|-------------|
| Missing jacket R-value | Medium | HPXML insulation not parsed |
| Performance adjustment unused | Low | Config stored but not used |
| Node count differs from OCHRE | Low | 6 vs 2 default, may affect parity |

---

## Recommendation

The ERWH implementation is generally sound and preserves OCHRE physics well. The main gaps are:

1. **Add jacket R-value parsing** from HPXML WaterHeaterInsulation
2. **Apply performance adjustment** to element efficiency calculation
3. Consider adding config option for default tank nodes to match OCHRE parity tests

The control logic and telemetry are comprehensive and well-integrated with the dwelling thermal solver.
