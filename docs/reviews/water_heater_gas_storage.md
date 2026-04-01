# Gas Storage Water Heater Configuration Review

**Review Date**: March 31, 2026  
**Reviewer**: Code Review Agent  
**Scope**: HARES Gas Storage Water Heater (Natural Gas, Propane, Fuel Oil)

---

## 1. HPXML Parsing

### Attributes Parsed from HPXML

The HPXML parser (`resolve_water_heater.rs`) extracts the following attributes for gas storage water heaters:

| HPXML Field | HARES Config Field | Status |
|-------------|-------------------|--------|
| `FuelType` | `fuel_type` | Fully parsed |
| `EnergyFactor` | `energy_factor` | Fully parsed |
| `UniformEnergyFactor` | `uniform_energy_factor` | Fully parsed |
| `TankVolume` | `tank_volume_m3` | Parsed with 0.95× correction factor |
| `TankHeight` | `tank_height_m` | Parsed, defaults to 4 ft if absent |
| `HeatingCapacity` | `heating_capacity_w` | Fully parsed (Btu/hr → W conversion) |
| `RecoveryEfficiency` | Used for UA calculation | Used internally, not exposed in config |
| `HotWaterTemperature` | `setpoint_c` | Fully parsed (°F → °C) |
| `PerformanceAdjustment` | `performance_adjustment` | Fully parsed |
| `FirstHourRating` | `first_hour_rating_m3` | Parsed (gal → m³) |
| `Location` | `zone_type` | Parsed (maps to conditioned/unconditioned) |
| `PilotPower` | `pilot_power_w` | Parsed |
| `FlueLossFraction` | `flue_loss_fraction` | Parsed |

### Missing HPXML Attributes

**IGNITION TYPE NOT PARSED**: The `<IgnitionType>` element from HPXML is **NOT** parsed. The HARES config has an `ignition_type` field, but the HPXML resolver always passes `None`. This means:

- HPXML's `ElectronicIgnition` or `StandingPilot` values are ignored
- HARES defaults to standing pilot (5W) unless explicitly configured
- Users must manually configure `ignition_type: "electronic"` to get electronic ignition

**Location in code**: `crates/hares-io/src/hpxml/resolve_water_heater.rs` line 109

---

## 2. OCHRE Defaults Comparison

### OCHRE Defaults for Gas Storage WH

| Parameter | OCHRE Default | HARES Default | Match? |
|-----------|---------------|---------------|--------|
| Setpoint | 51.67°C (125°F) | 51.67°C (125°F) | ✓ Exact |
| Deadband | 5.56°C (10°F) | 5.56°C (10°F) | ✓ Exact |
| Burner Input | 4500 W (base class) | 11,000 W | **HARES larger** |
| Tank Nodes | 2 (default), supports up to 12 | 6 (default), supports 1-12 | HARES more stratified |
| Energy Factor | Required input | 0.78 (derived from RE) | HARES derives from RE |
| Recovery Efficiency | 0.78 (hardcoded default) | 0.78 | ✓ Exact |
| Pilot Power | Not explicitly modeled (implicit in efficiency) | 5 W (standing pilot default) | HARES explicit |
| Flue Loss Fraction | Derived from model | 0.10 (10%) | HARES explicit |
| Skin Loss Fraction | EF-based: 0.64/0.91/0.96 | EF-based: 0.64/0.91/0.96 | ✓ Exact OCHRE logic |
| Max Tank Temp | 60°C (140°F) | 60°C | ✓ Exact |

### Key Differences from OCHRE

1. **Higher default burner input**: HARES uses 11,000 W vs OCHRE's 4,500 W. This is more realistic for gas storage WH (typical 40,000 Btu/hr = ~11,700 W).

2. **More stratified default tank**: HARES uses 6 nodes by default vs OCHRE's 2-node default. HARES supports up to 12 nodes for finer stratification.

3. **Explicit pilot modeling**: HARES explicitly models pilot light consumption (5W default for standing pilot, 0W for electronic), which OCHRE does not.

---

## 3. HARES Wiring (HPXML → Equipment)

### Flow: HPXML → Resolver → Config → Equipment

```
HPXML WaterHeatingSystem
    ↓
resolve_water_heater.rs::resolve_water_heaters()
    ↓
GasWaterHeaterConfig (typed)
    ↓
EquipmentConfig::from_typed()
    ↓
GasWH::new() + init_typed()
```

### Mapping Details

The wiring is well-implemented with proper type safety:

1. **Fuel type mapping**: Natural gas, propane, and fuel oil all map to the "Gas Water Heater" equipment class (line 457-458 of `resolve_water_heater.rs`)

2. **UA calculation**: HARES implements the Burch & Erickson (2004) methodology for EF and Maguire & Roberts (2020) for UEF (`water_heater_ua.rs`)

3. **Volume correction**: Tank volume is reduced by 5% for gas units (vs 10% for electric) - matches OCHRE

4. **Zone wiring**: Location is parsed and mapped to zone type (conditioned/unconditioned/garage/pier)

### Issue: Ignition Type Not Wired

```rust
// resolve_water_heater.rs line 109
ignition_type: None,  // HPXML IgnitionType NOT read!
```

The HPXML `<IgnitionType>` field exists but is never read. Users must manually add `ignition_type` to config.

---

## 4. Control Logic

### Thermostat Control

HARES uses hysteresis-based thermostat control (`gas.rs::should_fire()`):

```
If burner is OFF:
    fire if tank_temp < setpoint - deadband
If burner is ON:
    fire if tank_temp < setpoint
```

This matches OCHRE's logic exactly (`WaterHeater.py::run_thermostat_control()`).

### Control Signals Supported

| Control Signal | Support | Implementation |
|---------------|---------|----------------|
| ThermalSetpoint | ✓ Full | Updates setpoint and deadband |
| DutyCycle | ✓ Full | Multiplies effective heating capacity |
| ModeOverride | ✓ Full | Forces Off/Heating modes |
| LoadFraction | ✓ Full | Multiplies dr_load_fraction |
| PowerLimit | ✓ Full | Caps burner power |
| DemandResponse | ✓ Full | 5 levels: Normal/Moderate/High/Critical/GridEmergency |

### Demand Response Implementation

HARES has a well-designed DR system:

| DR Level | Setpoint Offset | Load Fraction |
|----------|-----------------|---------------|
| Normal | 0°C | 1.0 |
| Moderate | -3°C | 1.0 |
| High | -6°C | 0.8 |
| Critical | -10°C | 0.5 |
| GridEmergency | 0°C | 0.0 |

This is more granular than many implementations.

### Safety Features

- **Max tank temperature cutout**: If any node exceeds `max_tank_temp_c` (default 60°C), burner is forced off
- **Thermal cutout responds to hottest point**: Uses `reduce(f64::max)` across all nodes (line 361-367)

---

## 5. Output Ports & Telemetry

### Port Declarations

```rust
ports: vec![
    PortDeclaration::fuel(),        // Gas consumption
    PortDeclaration::electrical(),  // Fan power (if present)
    PortDeclaration::thermal(zone), // Skin losses to zone
    PortDeclaration::fluid(loop_id, FluidType::Water),  // DHW supply
    PortDeclaration::fluid(DHW_DEMAND_LOOP, FluidType::Water), // Demand input
]
```

### Telemetry Fields

| Telemetry Key | Unit | Description |
|--------------|------|-------------|
| `tank_avg_temp_c` | °C | Volume-weighted average tank temperature |
| `tank_node_X_c` | °C | Individual node temperatures (0 to n_nodes-1) |
| `burner_power_w` | W | Gas burner thermal input |
| `pilot_power_w` | W | Pilot light fuel consumption (continuous) |
| `fuel_input_w` | W | Total fuel (burner + pilot) |
| `flue_loss_w` | W | Heat lost to flue (exits building) |
| `skin_loss_w` | W | Jacket losses to zone |
| `fan_electric_w` | W | Auxiliary electric fan |
| `draw_flow_rate_kg_s` | kg/s | DHW draw rate |
| `operating_mode` | enum | 0=Off, 1=Heating |

### Core Output

```rust
CoreOutput {
    flows: CoreFlows {
        electric_kw: Some(ElectricPower::Consumption(fan_electric_kw)),
        fuel_w: Some(FuelPower { fuel_type, consumption_w: fuel_input_w }),
    },
    state: CoreState {
        operating_mode: Some(mode),
    }
}
```

### What is NOT Telemetrized

- Recovery rate (W) - not directly reported
- Standby loss fraction - not directly reported
- Tank volume used for calculations - not directly reported

---

## 6. Dwelling/Thermal Solver Integration

### Hot Water Demand Integration

HARES uses a two-source hot water demand model:

1. **Schedule-based draw**: Configured via `draw_flow_rate_kg_s` or schedule column
2. **Appliance demand**: From wet appliances via `DHW_DEMAND_LOOP` fluid port

```
Wet Appliance → PortContribution::Fluid(DHW_DEMAND_LOOP)
                           ↓
Water Heater reads: read_dhw_demand_kg_s(ports)
                           ↓
Total draw = schedule_draw + appliance_demand
```

This matches OCHRE's architecture.

### Thermal Integration

**Skin losses (jacket losses)** are correctly routed to the zone thermal port:

```rust
// gas.rs lines 473-481
if skin_loss_to_zone_w > 0.0 {
    ports.accumulate(&PortContribution::Thermal {
        zone,
        sensible_gain_w: skin_loss_to_zone_w,
        category: ThermalCategory::JacketLoss,
    })?;
}
```

**Flue losses are NOT added to zone** - they correctly exit the building (line 403-404):

```rust
let flue_loss_w = gross_heat_w * self.flue_loss_fraction;
// Flue loss is NOT reported to zone thermal port
```

This matches OCHRE's `WaterHeater.py:722` note: "no sensible gains from heater (all is vented)".

### Mains Temperature Integration

HARES properly integrates with the thermal solver via:

1. **Weather mains temperature**: `env.weather.mains_temp_c` (canonical source)
2. **Schedule fallback**: If weather is unavailable, reads from schedule column
3. **Config default**: Falls back to init-time default (15°C in HARES vs OCHRE's 10°C)

---

## Summary of Findings

### What HARES Does Well

1. **Stratified tank model**: More sophisticated than OCHRE's default (6 nodes vs 2)
2. **Explicit pilot modeling**: Properly models standing pilot continuous consumption
3. **Comprehensive telemetry**: Full visibility into tank state, node temperatures
4. **Proper thermal routing**: Skin losses to zone, flue losses excluded
5. **DR support**: Five-level DR with setpoint offset and load fraction control
6. **Appliance integration**: DHW_DEMAND_LOOP properly connects wet appliances

### Issues Identified

| Issue | Severity | Description |
|-------|----------|-------------|
| HPXML IgnitionType not parsed | **High** | Standing pilot assumed by default; electronic ignition requires manual config |
| OCHRE HPXML pilot parsing commented out | Medium | OCHRE's hpxml.py had pilot parsing code that was never ported |
| Default mains temp 15°C vs OCHRE 10°C | Low | Slightly higher default may affect winter performance |

### Recommendations

1. **Add HPXML IgnitionType parsing**: Read `<IgnitionType>` from HPXML and map to `ignition_type` config field:
   - "electronic" → `pilot_power_w = 0`
   - "standing" → `pilot_power_w = 5` (or explicit value)
   - Map is: `Some("ElectronicIgnition") | Some("electronic") => 0.0` (line 285-286 of gas.rs)

2. **Consider matching OCHRE's 10°C default**: For mains temperature fallback, 10°C is more realistic for cold climates

---

## Comparison to OCHRE: Physics Preservation

| Physics Aspect | OCHRE | HARES | Preserved? |
|---------------|-------|-------|------------|
| Hysteresis thermostat | ✓ | ✓ | ✓ Exact |
| Skin loss fraction by EF | EF<0.7→0.64, <0.8→0.91, else→0.96 | Same | ✓ Exact |
| Flue losses excluded from zone | ✓ | ✓ | ✓ Exact |
| Tank stratification | 2-12 nodes | 1-12 nodes | ✓ Extended |
| Pilot continuous consumption | Implicit | Explicit (5W default) | ✓ Improved |
| Recovery efficiency | 0.78 default | 0.78 default | ✓ Exact |
| UA from EF/UEF | Burch & Erickson | Burch & Erickson | ✓ Exact |
| DHW demand from appliances | Via schedule | DHW_DEMAND_LOOP | ✓ Equivalent |

**Overall**: HARES preserves OCHRE physics and in some cases improves them (explicit pilot, more stratified default tank).

---

*End of Review*
