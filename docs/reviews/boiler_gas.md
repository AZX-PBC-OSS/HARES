# Gas Boiler Configuration Review - HARES vs OCHRE

**Reviewer**: Code Review Agent  
**Date**: 2026-03-31  
**Scope**: Gas Boiler HVAC equipment in HARES

---

## 1. HPXML Parsing

### Attributes Parsed by HARES (from `resolve_hvac.rs`)

| HPXML Attribute | HARES Field | Default if Missing |
|-----------------|-------------|-------------------|
| `AnnualHeatingEfficiency/Value` (Units=AFUE) | `afue` | 0.80 |
| `HeatingCapacity` (converted to W) | `capacity_w` | **Required** |
| `ElectricAuxiliaryEnergy` (kWh/year → W) | `fan_power_w` | None |
| `number_of_speeds` | `number_of_speeds` | 1 |
| N/A (computed) | `flow_rate_kg_s` | 0.5 |
| N/A (computed) | `return_temp_c` | 40.0 |

### Parsing Functions (in `resolve_hvac.rs`)
- `afue_from_params()` (lines 324-338): Tries `efficiency_afue` first, then `heating_efficiency` with `heating_efficiency_units` = "AFUE"
- `fan_power_from_params()` (lines 453-455): Direct passthrough of `fan_power_w`
- `n_speeds_from_params()` (lines 401-406): Default 1

### Issue: Firing Rate Not Parsed
HARES does NOT parse any firing rate / turndown ratio from HPXML. Boiler is single-speed on/off only. This is a limitation compared to full modulating boiler capability.

---

## 2. OCHRE Defaults Comparison

| Parameter | OCHRE | HARES | Notes |
|-----------|-------|-------|-------|
| **AFUE Default** | None (from HPXML required) | 0.80 | HARES uses DOE federal minimum |
| **Standby Losses** | Not modeled | Not modeled | Both omit |
| **Jacket Losses** | Not modeled | **Modeled** | HARES enhancement |
| **Condensing Threshold** | AFUE > 0.90 | AFUE > 0.90 | Same |
| **Condensing Outlet Temp** | 65.56°C (150°F) | 65.56°C | Same |
| **Non-condensing Outlet Temp** | 82.22°C (180°F) | 82.22°C | Same |
| **Return Temp Default** | N/A | 40.0°C | HARES default |
| **EIR Polynomial (Condensing)** | 6-coeff | 6-coeff | Same values |
| **EIR Polynomial (Non-condensing)** | 10-coeff | 10-coeff | Same values |

### OCHRE EIR Coefficients (from `HVAC.py` lines 695-718)

**Condensing** (6 coefficients):
```python
[1.058343061, -0.052650153, -0.0087272, -0.001742217, 0.00000333715, 0.000513723]
```

**Non-condensing** (10 coefficients):
```python
[1.111720116, 0.078614078, -0.400425756, 0, -0.000156783, 0.009384599,
 0.234257955, 0.00000132927, -0.004446701, -0.0000122498]
```

HARES preserves these exactly in `boiler.rs` (lines 45-64).

---

## 3. HARES Wiring (HPXML → Equipment)

### Flow: HPXML → `GasBoilerConfig` → `GasBoiler` struct

1. **HPXML Parsing** (`resolve_hvac.rs` lines 566-600):
   - `try_build_gas_boiler_config()` builds typed config
   - Required: `capacity_w` (from HeatingCapacity)
   - Optional: `afue` (defaults 0.80), `fan_power_w`, `number_of_speeds`, `flow_rate_kg_s`, `return_temp_c`

2. **Equipment Initialization** (`boiler.rs` lines 406-438):
   ```rust
   self.rated_capacity_w = typed.capacity_w.max(0.0);
   self.pump_kw = typed.fan_power_w.unwrap_or(0.0) / 1_000.0;
   self.condensing = fuel_efficiency > 0.9;
   self.outlet_temp_c = if self.condensing {
       DEFAULT_CONDENSING_OUTLET_TEMP_C  // 65.56°C
   } else {
       DEFAULT_NON_CONDENSING_OUTLET_TEMP_C  // 82.22°C
   };
   self.eir_max = 1.0 / fuel_efficiency;
   ```

3. **Config Structure** (`heating_config.rs` lines 87-128):
   ```rust
   pub struct GasBoilerConfig {
       pub equipment_id: Option<u32>,
       pub zone_id: Option<u16>,
       pub loop_id: Option<u16>,
       pub capacity_w: f64,       // Required
       pub afue: f64,             // Default 0.80
       pub flow_rate_kg_s: f64,   // Default 0.5
       pub return_temp_c: f64,    // Default 40.0
       pub fluid_type: FluidType, // Default Water
       pub fan_power_w: Option<f64>,
       pub number_of_speeds: u8,  // Default 1
   }
   ```

---

## 4. Control Logic

### On/Off Control
- Uses shared `update_heating_control()` from `helpers.rs` (lines 130-145)
- Thermostat FSM with Heating/Off modes
- **Hysteresis**: Configurable via `deadband_c` in `ThermalSetpoint` control signal (lines 155-166)
- **Duty cycle**: Set to 1.0 for on/off cycling mode

### Modulation / Staging
- **NOT IMPLEMENTED**: HARES uses single-speed on/off only
- `number_of_speeds` field exists in config but is always 1
- No modulating control based on load

### Outdoor Reset
- **NOT IMPLEMENTED**: No outdoor reset curve in HARES
- OCHRE also lacks outdoor reset for boilers
- Fixed supply water temperature based on condensing status only

### Issues:
1. No modulating control - cannot reduce firing rate at low loads
2. No outdoor reset - cannot optimize supply temp based on outdoor conditions
3. Single-stage only - no multi-stage capability

---

## 5. Output Ports & Telemetry

### Ports (4 total)
1. **Fuel port**: Delivers `fuel_input_w` to fuel domain
2. **Electrical port**: Delivers `electric_kw` (pump power only)
3. **Thermal port**: Delivers `jacket_loss_w` as sensible gain to zone
4. **Fluid port**: Delivers thermal output to hydronic loop

### Telemetry Fields (`gas_boiler_telemetry_fields()` lines 689-732)

| Field | Key | Unit | Description |
|-------|-----|------|-------------|
| Electric Power | `electric_kw` | kW | Pump/fan auxiliary draw |
| Fuel Input | `fuel_input_w` | W | Fuel consumption |
| Thermal Output | `thermal_output_w` | W | Delivered to hydronic loop |
| Jacket Loss | `jacket_loss_w` | W | Heat loss to zone |
| EIR | `eir` | - | Instantaneous energy-input ratio |
| Supply Temp | `supply_temp_c` | °C | Fluid supply temperature |
| Return Temp | `return_temp_c` | °C | Fluid return temperature |
| Operating Mode | `operating_mode` | enum | 0=Off, 1=Heating |

### EIR Calculation (lines 367-394)
- **Condensing**: Uses zone air temperature (not return water temp)
  - Formula: `eir = eir_max / (c[0] + c[1]*plr + c[2]*plr² + c[3]*t_in + c[4]*t_in² + c[5]*plr*t_in)`
- **Non-condensing**: Uses outlet water temperature
  - Formula: 10-coeff polynomial with `t_out`

---

## 6. Dwelling Integration

### Jacket Loss to Zone (lines 507-523)
```rust
let thermal_output_sf_w = thermal_output_w * sf;
let jacket_loss_w = (fuel_input_w + electric_kw * 1e3 - thermal_output_sf_w).max(0.0);
if let Some(zone) = self.descriptor.zone {
    if jacket_loss_w > 0.0 {
        ports.accumulate(&PortContribution::Thermal {
            zone,
            sensible_gain_w: jacket_loss_w,
            latent_gain_w: 0.0,
            category: ThermalCategory::JacketLoss,
        })?;
    }
}
```

**Key behavior**:
- Jacket loss = fuel_input + pump_electric - thermal_output (when EIR > 1.0)
- For condensing boilers (EIR < 1.0), jacket loss is zero - extra efficiency is latent heat recovery, not room gain
- Losses go to the zone defined in equipment config
- Uses `ThermalCategory::JacketLoss` for categorization

### Hydronic Loop Integration
- Connects via fluid port with `LoopId`
- Supply/return temperatures calculated from flow rate and thermal output
- Return temperature can be overridden from fluid domain state (`loop_return_temp_c()` lines 622-634)

---

## Summary

### Where HARES Physics is Better than OCHRE
1. **Jacket losses modeled** - OCHRE does not model jacket losses; HARES correctly accounts for combustion losses going to conditioned space
2. **EIR polynomial with zone temperature** - HARES correctly uses zone air temp for condensing EIR (matches OCHRE reference)
3. **Telemetry fields** - More comprehensive telemetry than OCHRE baseline

### Issues / Missing Features
1. **No firing rate / turndown** - Single-speed on/off only, no modulating capability
2. **No outdoor reset** - Fixed supply water temperature, cannot optimize based on outdoor conditions
3. **No standby losses** - Not modeled (same as OCHRE, but would be a useful enhancement)
4. **No multi-stage control** - Ignores `number_of_speeds` > 1
5. **Default flow rate 0.5 kg/s may be unrealistic** - No calculation from boiler capacity
6. **No jacket loss when off** - Correct behavior, but no standby parasitic losses

### Verdict
HARES preserves OCHRE's core EIR polynomial physics while adding jacket loss modeling as an enhancement. The defaults (0.80 AFUE, 40°C return, 0.5 kg/s flow) are reasonable. Main gaps are lack of modulating control and outdoor reset, which are standard boiler features in practice.
