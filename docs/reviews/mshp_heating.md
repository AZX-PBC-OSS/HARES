# Mini-Split Heat Pump (MSHP) Heating Configuration Review - HARES

**Reviewer**: Code Review Agent  
**Date**: 2025-03-31  
**Scope**: Mini-Split Heat Pump Heating (MSHP Heater)  
**Files Reviewed**:
- `vendors/OCHRE/ochre/Equipment/HVAC.py` - OCHRE HVAC implementation
- `vendors/OCHRE/ochre/utils/hpxml.py` - HPXML parsing in OCHRE
- `crates/hares-io/src/hpxml/resolve_hvac.rs` - HPXML resolution in HARES
- `crates/hares-equipment/src/hvac/heat_pump/heater.rs` - MSHP Heater implementation
- `crates/hares-equipment/src/hvac/heat_pump_config.rs` - Heat pump heater config
- `crates/hares-equipment/src/hvac/hvac_core.rs` - Core HVAC types and defaults
- `defaults/hvac_heating/MSHP Heater.csv` - OCHRE MSHP biquadratic coefficients
- `defaults/HVAC Multispeed Parameters.csv` - OCHRE MSHP default parameters

---

## 1. HPXML Parsing

### 1.1 Parsed Attributes for MSHP Heating

**Source HPXML**: `base-hvac-mini-split-heat-pump-ductless.xml`

| HPXML Attribute | HARES Field | Notes |
|-----------------|-------------|-------|
| `HeatPumpType=mini-split` | `is_mini_split: true` | Triggers MSHP behavior |
| `HeatingCapacity` | `heating_capacity_w` | BTU/h → watts conversion |
| `AnnualHeatingEfficiency/Value` with `Units=HSPF` | `heating_eir` | HSPF → EIR (3.412/HSPF) |
| `FractionHeatLoadServed` | `fraction_heating_load_served` | |
| `FanPowerWatts` | `fan_power_w` | Optional, defaults to calculated |
| - | `number_of_speeds` | Forced to 4 for MSHP |
| - | `speed_control_mode` | Forced to `variable_speed` for MSHP |
| `BackupHeatingCapacity` | `backup_capacity_w` | Optional backup heat |
| `BackupSystemFuel` | `backup_fuel` | Backup fuel type |
| `BackupAnnualHeatingEfficiency` | `backup_eir` | Backup EIR (1/efficiency) |
| `CompressorLockoutTemperature` | `hp_lockout_temp_c` | HP disabled below this OAT |
| `BackupHeatingSwitchoverTemperature` | `er_lockout_temp_c` | Backup enabled below this OAT |

### 1.2 Parsing Flow

1. **HPXML Detection** (`resolve_hvac.rs:1351`):
   ```rust
   "mini-split" => Some(("MSHP Heater", "MSHP Cooler")),
   ```

2. **Config Builder** (`resolve_hvac.rs:1483-1494`):
   - `try_build_heat_pump_heater_config()` handles MSHP heating
   - Forces `n_speeds = 4` when `is_mini_split = true`
   - Forces `speed_control_mode = "variable_speed"` for MSHP
   - Sets `duct = DuctConfig::default()` (no ducts for ductless MSHP)
   - Sets `ochre_class = "MSHP Heater"`

3. **Backup Heating Handling** (`resolve_hvac.rs:1377-1391`):
   - MSHP can have backup heating (furnace, electric resistance, etc.)
   - Backup is optional - if not specified, MSHP runs without backup
   - Backup capacity/eir are propagated to HeatPumpHeaterConfig

### 1.3 Missing/Issues

**Issue 1: HSPF2 Not Explicitly Used for MSHP** (Severity: Low)
- **Location**: `crates/hares-io/src/hpxml/building.rs:775`
- **Problem**: Building-level `HSPF2` is parsed but not propagated to MSHP config
- **OCHRE Behavior**: OCHRE primarily uses HSPF (not HSPF2) from HPXML
- **Impact**: Minor - HSPF2 is converted to HSPF via factor (1/0.95) at building level

---

## 2. OCHRE Defaults

### 2.1 Key MSHP-Specific Defaults

| Parameter | ASHP Default | MSHP Default | HARES Implementation |
|-----------|-------------|--------------|---------------------|
| Number of Speeds | 1-4 (from HSPF) | 4 (forced) | `effective_number_of_speeds()` → 4 |
| Speed Control Mode | Single/Two/Variable | VariableSpeedIdeal | `heating_speed_control_mode()` |
| Airflow (CFM/ton) | ~400 (varies) | ~333 (estimated) | MSHP-specific ratios in curves |
| Pan Heater | N/A | 150W @ 0°C | `MSHP_PAN_HEATER_DEFAULT_TEMP_C` |
| Startup Cd | 0.11-0.25 | 0.0 (no penalty) | `derived_heating_startup_cd()` → Some(0.0) |
| Duct DSE | Computed | 1.0 (no ducts) | `DuctConfig::default()` |
| Backup | Optional (default 4kW) | Optional (defaults to 4kW if specified) | Same as ASHP |

### 2.2 MSHP Biquadratic Coefficients

HARES loads MSHP-specific curves from `defaults/hvac_heating/MSHP Heater.csv`:

```
Name: Single_1, Variable_1, Variable_2, Variable_3, Variable_4
- Only Variable-speed stages (no Single_1, Double_1, etc. for heating)
- PLF bounds: min_plf=0.22 for stage 1, min_plf=1.0 for stages 2-4
- EIR and capacity curves specific to MSHP heating performance
- Single_1 and Variable_1 share identical curves (stage 1 baseline)
- Stages 2-4 have PLF=1.0 (no degradation - inverter-driven)
```

### 2.3 OCHRE Multispeed Parameters for MSHP Heating

From `defaults/HVAC Multispeed Parameters.csv`:
- MSHP Heater entries with HSPF 8.2 to 14.0
- All MSHP entries have 4 speeds
- Capacity ratios: ~0.4 (stage 1) to 1.0 (stage 4)
- Airflow ratios: ~0.56 to 1.0
- COP values range from 2.79 to 5.59 depending on HSPF

---

## 3. HARES Wiring

### 3.1 Equipment Construction

MSHP heating is constructed via `MinisplitHeater` (`heater.rs:48-50`):

```rust
pub struct MinisplitHeater {
    core: HeatPumpHeaterCore,
}
```

The `HeatPumpHeaterCore::new()` function (`heater.rs:288-378`) handles MSHP-specific initialization:
1. Sets `variant = HeaterVariant::Minisplit`
2. Uses `HvacEquipmentType::MiniSplitHeat` equipment type
3. Applies MSHP pan heater defaults (150W @ 0°C)

### 3.2 Config Mapping

In `init_from_typed()`, the MSHP heater maps from `HeatPumpHeaterConfig` (`heater.rs:417-530`):
- Reads `is_mini_split` flag to determine variant
- Sets heating capacities from config
- Reads backup capacity/eir (optional)

---

## 4. Control Logic

### 4.1 Speed Control Mode

MSHP uses `SpeedControlMode::VariableSpeedIdeal` (`heat_pump_config.rs:396-404`):

```rust
pub fn heating_speed_control_mode(&self) -> SpeedControlMode {
    let n = self.effective_number_of_speeds();
    match n {
        1 => SpeedControlMode::SingleSpeed,
        2 => SpeedControlMode::TwoSpeedSetpoint,
        n if n >= 4 => SpeedControlMode::VariableSpeedIdeal,
        _ => SpeedControlMode::SingleSpeed,
    }
}
```

### 4.2 Inverter Modulation

**VariableSpeedIdeal** behavior:
- `speed_index`: 0-3 (4 stages)
- `speed_frac`: Continuous [0,1] interpolation between stages
- `duty_cycle`: Equals load_fraction (no cycling penalty at partial loads)
- Direct mapping: Load fraction → compressor speed

### 4.3 Backup Heating Control

MSHP can have backup heating (furnace, electric resistance) - this is the same as ASHP:
- **HP Lockout**: HP disabled below `hp_lockout_temp_c` (default -17.78°C / 0°F)
- **ER Lockout**: Backup disabled above `er_lockout_temp_c` (default 4.44°C / 40°F)
- **ER Hard Lockout**: After setpoint increase, prevents ER for `er_hard_lockout_time_s`
- **ER Soft Lockout**: After hard lockout expires, keeps ER off while zone temp rises

### 4.4 Pan Heater (MSHP Specific)

MSHP has a pan heater for condensate management in cold weather (`heater.rs:866-873`):
- Pan heater: 150W rated
- Activation temperature: 0°C (32°F)
- Only active when outdoor temp < 0°C and HP is running

### 4.5 Zero Startup Cd

HARES sets `c_d = 0.0` for MSHP (`heat_pump_config.rs:410-418`):

```rust
pub fn derived_heating_startup_cd(&self) -> Option<f64> {
    match self.heating_speed_control_mode() {
        SpeedControlMode::VariableSpeedIdeal => Some(0.0),
        // ...
    }
}
```

This correctly models inverter-driven compressors that ramp smoothly without cycling penalty.

---

## 5. Output Ports & Telemetry

### 5.1 Ports

MSHP Heater declares two primary ports:
- **Thermal Port**: `sensible_heating_w` (positive = heat delivery)
- **Electrical Port**: Net active power draw

### 5.2 Telemetry Fields

Standard heating telemetry from `defaults/hvac_heating/MSHP Heater.csv`:

| Telemetry Key | Description | MSHP Notes |
|---------------|-------------|------------|
| `electric_kw` | Total electrical power | Includes compressor + fan + pan heater |
| `thermal_output_w` | Total heating capacity | Includes HP + ER backup |
| `operating_mode` | Operating mode code | HP On / HP+ER On / ER On / Off |
| `speed_index` | Current speed stage (0-3) | 4 discrete stages |
| `runtime_fraction` | Runtime fraction (duty cycle) | Equals load_fraction for MSHP |
| `cop` | COP (thermal_output / compressor_power) | Per AHRI convention |
| `compressor_kw` | Compressor-only power | Excludes fan and pan heater |
| `defrost_active` | Defrost correction active | Boolean flag |
| `defrost_time_fraction` | Fraction in defrost mode | [0..1] |
| `heating_setpoint_c` | Active heating setpoint | |
| `cooling_setpoint_c` | Active cooling setpoint | |

### 5.3 MSHP-Specific Telemetry

- **Speed Index**: Reports 0-3 for the 4 MSHP speed stages
- **Pan Heater**: Reports in `electric_kw` when active (150W at OAT < 0°C)
- **No inverter efficiency telemetry**: Unlike some modern inverters, HARES doesn't report inverter-specific efficiency losses

---

## 6. Dwelling/Thermal Solver Integration

### 6.1 Zone Integration

MSHP heating integrates directly with the thermal solver:

1. **No Duct Losses**: MSHP uses `DuctConfig::default()` which means:
   - `duct_dse = 1.0` (100% delivery efficiency)
   - No duct zone for loss deposition
   - All heating delivered directly to conditioned zone

2. **Direct Zone Delivery** (`dwelling/mod.rs:2219-2227`):
   ```rust
   let hvac_heating_w = gains.hvac_heating_w.max(0.0);
   // Applied directly to zone sensible gain
   ```

### 6.2 Thermal Solver Behavior

- MSHP heating provides ideal (no loss) delivery to the zone
- Same energy balance calculation as other heating equipment
- Sensible gain is positive (heat addition)

### 6.3 Comparison with OCHRE

**OCHRE (HVAC.py)**:
- MSHP: Direct zone delivery, no duct model
- Uses same biquadratic curves for capacity/EIR
- Inverter modulation modeled via variable-speed curves
- Pan heater (150W @ 0°C) added to electric power

**HARES**:
- Matches OCHRE: Direct zone delivery
- Uses OCHRE curves from `defaults/hvac_heating/MSHP Heater.csv`
- Matches OCHRE variable-speed behavior
- Matches OCHRE pan heater (150W @ 0°C)

---

## 7. Issues and Recommendations

### Issue 1: No Explicit HSPF2 Usage (Severity: Low)
- HPXML building-level HSPF2 is parsed but not used
- MSHP defaults work well without it
- Recommendation: Document that HSPF (not HSPF2) is used, or add HSPF2 conversion

### Issue 2: Missing Inverter Efficiency Telemetry (Severity: Low)
- Modern MSHP inverters have variable efficiency (~95-98%)
- HARES assumes 100% electrical conversion efficiency
- Recommendation: Consider adding inverter efficiency telemetry or modeling losses

### Issue 3: Backup Heating with MSHP (Severity: Info)
- MSHP can have backup heating per HPXML (furnace, electric resistance)
- When backup is specified, MSHP behaves identically to ASHP for backup logic
- This is correct HP system behavior - no issues

---

## 8. Summary

| Aspect | HARES vs OCHRE | Assessment |
|--------|---------------|------------|
| HPXML Parsing | Complete | Good |
| MSHP-Specific Defaults | Preserved | Good |
| 4-Speed Variable Control | Preserved | Good |
| No Duct Losses | Preserved | Good |
| Zero Startup Cd | Preserved | Good |
| Pan Heater (150W/0°C) | Preserved | Good |
| Backup Heating Support | Preserved | Good |
| Telemetry | Complete | Good |
| Thermal Solver Integration | Preserved | Good |

**Overall**: HARES correctly implements MSHP heating with all OCHRE physics preserved. The configuration properly models ductless mini-split behavior with variable-speed inverter modulation, no duct losses, MSHP-specific pan heater, and MSHP-specific defaults. Backup heating (when present) follows the same logic as ASHP.

---

## Appendix: Key Code Locations

- MSHP Heater construction: `crates/hares-equipment/src/hvac/heat_pump/heater.rs:48-50`
- HPXML resolution: `crates/hares-io/src/hpxml/resolve_hvac.rs:1483-1513`
- MSHP defaults: `defaults/hvac_heating/MSHP Heater.csv`
- Pan heater defaults: `crates/hares-equipment/src/hvac/heat_pump/heater.rs:350`
- Speed control mode: `crates/hares-equipment/src/hvac/heat_pump_config.rs:396-418`
- Zero startup Cd: `crates/hares-equipment/src/hvac/heat_pump_config.rs:410-418`
