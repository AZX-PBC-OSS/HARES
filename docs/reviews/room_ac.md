# Room Air Conditioner (Window Unit) Configuration Review

**Review Date**: 2026-03-31  
**Equipment Type**: Room AC (Window/Through-wall Unit)  
**Component**: Cooling Equipment

---

## 1. HPXML Parsing

### Attributes Parsed for Room AC

The following HPXML attributes are parsed from the `<CoolingSystem>` element when `CoolingSystemType` is "room air conditioner":

| HPXML Element | HARES Parameter | Notes |
|---------------|-----------------|-------|
| `CoolingCapacity` (Btu/h) | `cooling_capacity_w` | Converted to watts |
| `CoolingCapacity` (W) | `cooling_capacity_w` | Direct watt value |
| `AnnualEfficiency` (EER) | `eer` | Required - see note below |
| `FractionCoolLoadServed` | `fraction_load_served` | Optional |
| `SensibleHeatFraction` | `shr` | Optional |
| `extension/FanPowerWattsPerCFM` | `fan_power_w_per_cfm` | Optional |
| `extension/AirflowDefectRatio` | `airflow_defect_ratio` | Optional |
| `extension/CoolingAirflowCFM` | `cooling_airflow_cfm` | Optional |

**Important**: HARES enforces that EER must be provided in HPXML. SEER cannot be substituted for EER because the test conditions and cycling correction factors differ; treating SEER as EER would overestimate efficiency by ~10-15%.

**Source**: `crates/hares-io/src/hpxml/resolve_hvac.rs` lines 775-819, 1279-1335

---

## 2. OCHRE Defaults

### Default Values Used for Room AC

| Parameter | HARES Default | OCHRE Reference |
|-----------|---------------|-----------------|
| **EER** | None (required from HPXML) | Must be provided |
| **Capacity** | None (required from HPXML) | Must be provided |
| **Airflow** | 320 CFM/ton (4.29e-5 m³/s/W) | OCHRE uses 312 CFM/ton for central AC |
| **SHR** | 0.75 | OCHRE defaults to 1.0 if not provided |
| **Speed Control** | Single speed only | Enforced |
| **Duct DSE** | 1.0 (no ducts) | No duct simulation |
| **Duct Zone** | None | No duct zone |
| **PLF Degradation** | 0.22 | Startup C_D = 0.22 |
| **Crankcase Heater** | 50W default | OCHRE CRANKCASE_HEATER_KW = 0.05 kW |
| **Crankcase Threshold** | 12.8°C | OCHRE CRANKCASE_HEATER_THRESHOLD_C |

### Biquadratic Curve Defaults

HARES uses Room AC-specific default curves (different from central AC):

```rust
// From ac_config.rs
DEFAULT_ROOM_AC_CAPACITY_CURVE: [f64; 6] = 
    [0.6405, 0.01568, 0.0004531, 0.001615, -0.0001825, 0.00006614];
DEFAULT_ROOM_AC_EIR_CURVE: [f64; 6] = 
    [2.287, -0.1732, 0.004745, 0.01662, 0.000484, -0.001306];
```

**Source**: `crates/hares-equipment/src/hvac/ac_config.rs` lines 17-20

---

## 3. HARES Wiring: HPXML to Equipment

### Detection Logic

Room AC is identified in HPXML by the `CoolingSystemType` element:
- Value: `"room air conditioner"` 
- Maps to equipment name: `"Room AC"`

**Source**: `crates/hares-io/src/hpxml/resolve_hvac.rs` lines 1279-1335

### Configuration Build Flow

```
HPXML CoolingSystem (type=room air conditioner)
    ↓
try_build_room_ac_config()
    ↓
RoomAcConfig (typed config)
    ↓
RoomAC::new(config) with is_room_ac=true
    ↓
CoolingCore::init() - enforces single speed, duct_dse=1.0
```

### Key Configuration Enforcements

In `CoolingCore::init()` (lines 503-511):
```rust
if self.is_room_ac {
    // Enforce single speed
    if self.hvac.speed_control_mode != SpeedControlMode::SingleSpeed {
        return Err(HaresError::Equipment(
            "Room AC supports only single-speed mode".to_string(),
        ));
    }
    self.hvac.speed_control_mode = SpeedControlMode::SingleSpeed;
    // No ducts for window units
    self.hvac.duct_dse = 1.0;
    self.hvac.duct_zone_id = None;
}
```

**Source**: `crates/hares-equipment/src/hvac/air_conditioner.rs` lines 503-511

---

## 4. Control Logic

### Thermostat Control

Room AC uses standard thermostat FSM with deadband control:

1. **Setpoint Source**: Can be static (from HPXML) or time-varying (schedule CSV/daily profile)
2. **Deadband**: Default is derived from HVAC defaults; configurable via HPXML
3. **On/Off Logic**: 
   - Turn on when: `zone_temp < setpoint - deadband * (1 - deadband_offset)`
   - Turn off when: `zone_temp > setpoint + deadband * deadband_offset`
   - Maintain when: within deadband

### Supported Control Capabilities

| Capability | Description |
|------------|-------------|
| `THERMAL_SETPOINT` | Cooling setpoint temperature (C) |
| `THERMAL_SETPOINT_DELTA` | Deadband width (C) |
| `DUTY_CYCLE` | Runtime fraction control [0-1] |
| `LOAD_FRACTION` | Load fraction for multi-stage |
| `POWER_LIMIT` | Maximum power limit (kW) |
| `MODE_OVERRIDE` | Force specific operating mode |
| `DEMAND_RESPONSE` | DR event response |
| `IDEAL_CAPACITY` | Solver-driven capacity (physics-based) |

### Mode Transitions

- Minimum on time: 120 seconds (from OCHRE)
- Minimum off time: 180 seconds (from OCHRE)
- Enforced via `mode_start_at` timestamp tracking

---

## 5. Output Ports & Telemetry

### Port Declarations

```rust
ports: vec![
    PortDeclaration::electrical(),
    PortDeclaration::thermal(zone),  // ZoneId from config
]
```

### Telemetry Fields

| Field | Unit | Description |
|-------|------|-------------|
| `electric_kw` | kW | Total cooling electric power (compressor + fan + crankcase) |
| `sensible_cooling_w` | W | Delivered sensible cooling magnitude |
| `latent_cooling_w` | W | Delivered latent cooling magnitude |
| `shr` | - | Sensible heat ratio |
| `operating_mode` | enum | 0=Off, 2=Cooling |
| `cop` | - | Coefficient of performance (AHRI: excludes fan) |
| `runtime_fraction` | - | Runtime fraction (PLR/PLF) [0..1] |
| `compressor_kw` | kW | Compressor-only electric power |
| `fan_kw` | kW | Supply fan electric power |
| `supply_temp_c` | C | Supply air temperature leaving coil |
| `apparatus_dew_point_c` | C | Apparatus dew point at coil |
| `bypass_factor` | - | Coil bypass factor |

**Source**: `crates/hares-equipment/src/hvac/ac_config.rs` lines 22-102

---

## 6. Dwelling/Thermal Solver Integration

### Zone Heat Distribution

For Room AC, all thermal energy goes directly to the conditioned zone:

```rust
// Room AC zone_heat_fractions = [(ZoneId(1), 1.0)]
// (all heat goes to conditioned zone, no duct losses)
```

### Thermal Energy Calculation

```rust
// Delivered heat to zone (pre-DSE for Room AC)
let delivered_thermal_w = step.thermal_output_w * self.hvac.duct_dse.clamp(0.0, 1.0);
// For Room AC: duct_dse = 1.0, so no losses

// Components:
sensible_cooling_w = capacity * shr
latent_cooling_w = capacity * (1 - shr)
fan_power_w = airflow_m3_s * fan_power_per_m3_s
total = sensible + latent + fan_power (written to thermal port)
```

### Ideal Capacity Mode

Room AC supports ideal capacity mode where the solver calculates exact capacity needed to maintain setpoint:

- Solver provides `ideal_capacity_w` via `IdealCapacity` signal
- Equipment uses this to determine on/off state and delivered heat
- Compatible with direct zone cooling (no duct simulation)

### Solver Integration Points

1. **Step Result**: `hvac_cooling_w` field reports heat removed (positive = heat removed)
2. **Port Contribution**: Thermal energy written to zone via thermal port
3. **Control Integration**: Supports ideal capacity for physics-based control

---

## 7. Comparison with OCHRE

### Physics Preservation

| Aspect | HARES | OCHRE | Status |
|--------|-------|-------|--------|
| Single-speed only | ✓ | ✓ | Preserved |
| No duct DSE | ✓ | ✓ | Preserved |
| No duct zone | ✓ | ✓ | Preserved |
| Default PLF (0.22) | ✓ | ✓ | Preserved |
| Default airflow | 4.29e-5 m³/s/W | ~4.5e-5 m³/s/W | Minor diff (~5%) |
| Default SHR | 0.75 | 1.0 if not provided | HARES better |
| Biquadratic curves | Room AC specific | N/A | HARES improved |
| Crankcase heater | 50W, 12.8°C | Same | Preserved |
| Telemetry fields | 12 fields | Similar | Preserved |

### Issues Found

**No critical issues identified.** The HARES implementation:

1. Correctly enforces single-speed for Room AC (unlike central AC which allows multi-speed)
2. Correctly sets duct DSE to 1.0 and removes duct zone (no ducts for window units)
3. Uses Room AC-specific biquadratic curves (different from central AC)
4. Rejects SEER as EER substitute (important physics correctness)
5. Supports ideal capacity mode for solver-driven thermal integration

### Potential Improvements

1. **Default EER**: Could consider adding a fallback default EER (e.g., 8.5 for older units) if not provided in HPXML, but this would reduce accuracy compared to explicit specification.

2. **Airflow default**: HARES uses 320 CFM/ton while OCHRE uses 312 CFM/ton for central AC. The difference is minor (~2.5%) but could be aligned.

---

## 8. Summary

The Room AC implementation in HARES is well-aligned with OCHRE physics. Key characteristics:

- **Configuration**: Requires EER and capacity from HPXML; uses single-speed only
- **Defaults**: No ducts, default PLF 0.22, default SHR 0.75, Room AC-specific biquadratic curves
- **Control**: Thermostat with deadband, supports external setpoint/duty cycle/power limit controls
- **Telemetry**: 12 output fields including power, cooling, SHR, supply temp
- **Integration**: Direct zone cooling, no duct losses, supports ideal capacity mode

No significant issues found. The implementation correctly models window/wall-mounted AC units as single-zone equipment without duct distribution.
