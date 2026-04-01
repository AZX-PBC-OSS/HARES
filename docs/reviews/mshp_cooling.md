# Mini-Split Heat Pump (MSHP) Cooling Configuration Review - HARES

**Reviewer**: Code Review Agent  
**Date**: 2025-03-31  
**Scope**: Mini-Split Heat Pump Cooling (MSHP Cooler)  
**Files Reviewed**:
- `vendors/OCHRE/ochre/Equipment/HVAC.py` - OCHRE HVAC implementation
- `vendors/OCHRE/ochre/utils/hpxml.py` - HPXML parsing in OCHRE
- `crates/hares-io/src/hpxml/resolve_hvac.rs` - HPXML resolution in HARES
- `crates/hares-equipment/src/hvac/heat_pump/cooler.rs` - MSHP Cooler implementation
- `crates/hares-equipment/src/hvac/heat_pump_config.rs` - Heat pump cooler config
- `crates/hares-equipment/src/hvac/hvac_core.rs` - Core HVAC types and defaults
- `defaults/hvac_cooling/MSHP Cooler.csv` - OCHRE MSHP biquadratic coefficients
- `defaults/HVAC Multispeed Parameters.csv` - OCHRE MSHP default parameters

---

## 1. HPXML Parsing

### 1.1 Parsed Attributes for MSHP Cooling

**Source HPXML**: `base-hvac-mini-split-heat-pump-ductless.xml`

| HPXML Attribute | HARES Field | Notes |
|-----------------|-------------|-------|
| `HeatPumpType=mini-split` | `is_mini_split: true` | Triggers MSHP behavior |
| `CoolingCapacity` | `cooling_capacity_w` | BTU/h → watts conversion |
| `AnnualCoolingEfficiency/Value` with `Units=SEER` | `cooling_eir` | SEER → EIR (3.412/SEER) |
| `CoolingSensibleHeatFraction` | `shr` | Default 0.75 when not specified |
| `FractionCoolLoadServed` | `fraction_cooling_load_served` | |
| `FanPowerWatts` | `fan_power_w` | Optional, defaults to calculated |
| - | `number_of_speeds` | Forced to 4 for MSHP |
| - | `airflow_m3_s_per_w` | MSHP default (312 CFM/ton) |

### 1.2 Parsing Flow

1. **HPXML Detection** (`resolve_hvac.rs:1351`):
   ```rust
   "mini-split" => Some(("MSHP Heater", "MSHP Cooler")),
   ```

2. **Config Builder** (`resolve_hvac.rs:961-1067`):
   - `try_build_heat_pump_cooler_config()` handles MSHP cooling
   - Forces `n_speeds = 4` when `is_mini_split = true`
   - Uses MSHP-specific airflow (312 CFM/ton vs 400 CFM/ton for central AC)
   - Sets `duct = DuctConfig::default()` (no ducts for ductless MSHP)
   - Sets ochre_class = "MSHP Cooler"

### 1.3 Missing/Issues

**Issue 1: SEER2/HSPF2 Not Used for MSHP** (Severity: Low)
- **Location**: `crates/hares-io/src/hpxml/building.rs:774-775`
- **Problem**: Building-level `SEER2` and `HSPF2` are parsed but not propagated to MSHP config
- **OCHRE Behavior**: OCHRE primarily uses SEER/HSPF (not SEER2/HSPF2) from HPXML
- **Impact**: Minor - SEER2 is just a slightly different test method (0.95 factor), and MSHP efficiency defaults are well-handled

---

## 2. OCHRE Defaults

### 2.1 Key MSHP-Specific Defaults

| Parameter | Central AC Default | MSHP Default | HARES Implementation |
|-----------|-------------------|--------------|---------------------|
| Number of Speeds | 1-4 (from SEER) | 4 (forced) | `effective_number_of_speeds()` → 4 |
| Airflow (CFM/ton) | 400 | 312 | `AIRFLOW_MSHP_COOLING_M3_S_PER_W` (4.19e-5) |
| Crankcase Heater | 50W @ 12.8°C | 15W @ 0°C | `MSHP_CRANKCASE_HEATER_KW`, `MSHP_CRANKCASE_HEATER_THRESHOLD_C` |
| Startup Cd | 0.07-0.25 | 0.0 (no penalty) | `derived_cooling_startup_cd()` → Some(0.0) |
| Speed Control Mode | Single/Two/Variable | VariableSpeedIdeal | `cooling_speed_control_mode()` |
| Duct DSE | Computed | 1.0 (no ducts) | `DuctConfig::default()` |

### 2.2 MSHP Biquadratic Coefficients

HARES loads MSHP-specific curves from `defaults/hvac_cooling/MSHP Cooler.csv`:

```
Name: Variable_1, Variable_2, Variable_3, Variable_4
- Only Variable-speed stages (no Single_1, Double_1, etc.)
- PLF bounds: min_plf=0.48 for stage 1, min_plf=1.0 for stages 2-4
- EIR and capacity curves specific to MSHP performance
```

**Comparison**: Central AC has all speed variants (Single_1 through Variable_4), while MSHP only has Variable stages.

### 2.3 OCHRE Multispeed Parameters for MSHP

From `defaults/HVAC Multispeed Parameters.csv`:
- MSHP Cooler entries with SEER 13.0 to 33.0
- All MSHP entries have 4 speeds
- Capacity ratios: ~0.49 (stage 1) to 1.0 (stage 4)
- Airflow ratios: ~0.53 to 1.0
- COP values range from 3.6 to 10.3 depending on SEER

---

## 3. HARES Wiring

### 3.1 Equipment Construction

MSHP cooling is constructed via `HpCooler::mshp_cooler()` (`cooler.rs:84-86`):

```rust
#[must_use]
pub fn mshp_cooler(config: EquipmentConfig) -> Self {
    Self::build(config, "MSHP Cooler", true)  // is_mshp = true
}
```

The `build()` function (`cooler.rs:36-76`):
1. Creates an `AirConditioner` inner device
2. Sets `HvacEquipmentType::MiniSplitCool` equipment type
3. Applies MSHP airflow ratio
4. Sets MSHP crankcase heater defaults (15W @ 0°C)

### 3.2 Config Mapping

In `init()`, the MSHP cooler maps from `HeatPumpCoolerConfig` to `CentralAirConditionerConfig` (`cooler.rs:104-172`):

```rust
fn typed_hp_to_central_ac_config(...) -> EquipmentConfig {
    // ...
    system_type: hp_cfg.is_mini_split.then(|| "mini-split".to_string()),
    // Duct config: MSHP = default (no ducts), ASHP = computed
}
```

---

## 4. Control Logic

### 4.1 Speed Control Mode

MSHP uses `SpeedControlMode::VariableSpeedIdeal` (`heat_pump_config.rs:396-404`):

```rust
pub fn cooling_speed_control_mode(&self) -> SpeedControlMode {
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

### 4.3 No Backup Cooling

MSHP cooling has no backup heating - only the inverter-driven compressor. This is consistent with OCHRE.

### 4.4 Startup Degradation

HARES sets `c_d = 0.0` for MSHP (`heat_pump_config.rs:410-418`):

```rust
pub fn derived_cooling_startup_cd(&self) -> Option<f64> {
    match self.cooling_speed_control_mode() {
        SpeedControlMode::VariableSpeedIdeal => Some(0.0),
        // ...
    }
}
```

This correctly models inverter-driven compressors that ramp smoothly without cycling penalty.

---

## 5. Output Ports & Telemetry

### 5.1 Ports

MSHP Cooler declares two primary ports:
- **Thermal Port**: `sensible_cooling_w` (negative = heat removal), `latent_cooling_w`
- **Electrical Port**: Net active power draw

### 5.2 Telemetry Fields

Standard cooling telemetry (`defaults/hvac_cooling/MSHP Cooler.csv` defines curve keys):

| Telemetry Key | Description | MSHP Notes |
|---------------|-------------|------------|
| `electric_kw` | Total electrical power | Includes compressor + fan |
| `sensible_cooling_w` | Sensible cooling capacity | |
| `latent_cooling_w` | Latent cooling capacity | |
| `thermal_output_w` | Total cooling (sensible + latent) | |
| `speed_index` | Current speed stage (0-3) | 4 discrete stages |
| `runtime_fraction` | Runtime fraction (duty cycle) | Equals load_fraction for MSHP |
| `cop` | Coefficient of performance | |
| `operating_mode` | Operating mode code | |

### 5.3 MSHP-Specific Telemetry

- **Speed Index**: Reports 0-3 for the 4 MSHP speed stages
- **No inverter efficiency telemetry**: Unlike some modern inverters, HARES doesn't report inverter-specific efficiency losses
- **Crankcase heater**: Reports in `electric_kw` when active (15W threshold = 0°C outdoor)

---

## 6. Dwelling/Thermal Solver Integration

### 6.1 Zone Integration

MSHP cooling integrates directly with the thermal solver:

1. **No Duct Losses**: MSHP uses `DuctConfig::default()` which means:
   - `duct_dse = 1.0` (100% delivery efficiency)
   - No duct zone for loss deposition
   - All cooling delivered directly to conditioned zone

2. **Direct Zone Delivery** (`dwelling/mod.rs:2220-2227`):
   ```rust
   let hvac_cooling_w = gains.hvac_cooling_w.abs();
   // Applied directly to zone sensible gain
   ```

### 6.2 Thermal Solver Behavior

- MSHP cooling provides ideal (no loss) delivery to the zone
- Same energy balance calculation as other cooling equipment
- Sensible gain is negative (heat removal)

### 6.3 Comparison with OCHRE

**OCHRE (HVAC.py)**:
- MSHP: Direct zone delivery, no duct model
- Uses same biquadratic curves for capacity/EIR
- Inverter modulation modeled via variable-speed curves

**HARES**:
- Matches OCHRE: Direct zone delivery
- Uses OCHRE curves from `defaults/hvac_cooling/MSHP Cooler.csv`
- Matches OCHRE variable-speed behavior

---

## 7. Issues and Recommendations

### Issue 1: No Explicit SEER2/HSPF2 Usage (Severity: Low)
- HPXML building-level SEER2/HSPF2 are parsed but not used
- MSHP defaults work well without them
- Recommendation: Document that SEER (not SEER2) is used, or add SEER2 conversion

### Issue 2: Missing Inverter Efficiency Telemetry (Severity: Low)
- Modern MSHP inverters have variable efficiency (~95-98%)
- HARES assumes 100% electrical conversion efficiency
- Recommendation: Consider adding inverter efficiency telemetry or modeling losses

### Issue 3: Crankcase Heater Companion RTF (Severity: Info)
- MSHP cooler receives companion heating RTF from HP heater side
- Uses `max(cooling_rtf, heating_rtf)` for crankcase decision
- This is correct HP system behavior

---

## 8. Summary

| Aspect | HARES vs OCHRE | Assessment |
|--------|---------------|------------|
| HPXML Parsing | Complete | Good |
| MSHP-Specific Defaults | Preserved | Good |
| 4-Speed Variable Control | Preserved | Good |
| No Duct Losses | Preserved | Good |
| Zero Startup Cd | Preserved | Good |
| Crankcase Heater (15W/0°C) | Preserved | Good |
| Telemetry | Complete | Good |
| Thermal Solver Integration | Preserved | Good |

**Overall**: HARES correctly implements MSHP cooling with all OCHRE physics preserved. The configuration properly models ductless mini-split behavior with variable-speed inverter modulation, no duct losses, and MSHP-specific defaults.

---

## Appendix: Key Code Locations

- MSHP Cooler construction: `crates/hares-equipment/src/hvac/heat_pump/cooler.rs:84-86`
- HPXML resolution: `crates/hares-io/src/hpxml/resolve_hvac.rs:961-1067`
- MSHP defaults: `defaults/hvac_cooling/MSHP Cooler.csv`
- Airflow constants: `crates/hares-equipment/src/hvac/hvac_core.rs:69-72`
- Crankcase heater: `crates/hares-equipment/src/hvac/heat_pump/cooler.rs:19-22`
