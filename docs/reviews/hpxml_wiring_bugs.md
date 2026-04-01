# HPXML Wiring Bugs in HARES

This document catalogs HPXML attributes that are parsed but NOT correctly wired to equipment config, meaning the parsed values are stored but never used in equipment initialization or operation.

---

## Category: HVAC - Heat Pumps

### ASHP / MSHP Heater (backup_fuel)

| HPXML Field | HARES Config Field | Parsed? | Wired? | Issue |
|-------------|-------------------|---------|--------|-------|
| `BackupSystemFuel` | `backup_fuel` | ✓ (line 1390-1392) | ✗ | Parsed and stored in config, but NEVER read in `HeatPumpHeaterCore::init_from_typed()` |

**Details:**
- **Parsed in**: `resolve_hvac.rs` lines 1390-1392
  ```rust
  if let Some(fuel) = child_text(heat_pump, "BackupSystemFuel") {
      params.insert("backup_fuel".to_string(), Value::String(fuel));
  }
  ```
- **Stored in config**: `HeatPumpHeaterConfig.backup_fuel` (line 917 of resolve_hvac.rs)
- **NOT wired in equipment**: `heater.rs` lines 514-518 only read `backup_capacity_w` and `backup_eir`:
  ```rust
  // Backup heating from typed config.
  self.backup_capacity_w = cfg.backup_capacity_w.unwrap_or(DEFAULT_BACKUP_CAPACITY_W).max(0.0);
  self.backup_eir = cfg.backup_eir.unwrap_or(DEFAULT_BACKUP_EIR).max(0.0);
  ```
- **Impact**: Backup heating always uses electric (EIR=1.0), even when HPXML specifies gas/propane backup. The fuel type is ignored.

---

## Category: HVAC - Furnaces

### Electric Furnace (fan_power_w from auxiliary_power_w)

| HPXML Field | HARES Config Field | Parsed? | Wired? | Issue |
|-------------|-------------------|---------|--------|-------|
| `ElectricAuxiliaryEnergy` | `auxiliary_power_w` | ✓ | ✗ | Not used as fallback for `fan_power_w` |
| `extension/FanPowerWatts` | `fan_power_w` | ✓ | ✓ | Works correctly |

**Details:**
- **Parsed in**: `resolve_hvac.rs` lines 1226-1231
  ```rust
  if let Some(aux_kwh) = child_f64(heating, "ElectricAuxiliaryEnergy") {
      params.insert("auxiliary_power_w".to_string(), json!(aux_kwh / 2080.0 * 1000.0));
  }
  ```
- **NOT wired in equipment**: `try_build_electric_furnace_config` line 534 only reads `fan_power_w`:
  ```rust
  let fan_power_w = fan_power_from_params(params);  // Only looks at "fan_power_w"
  ```
- **Missing fallback**: `fan_power_from_params()` should also check `auxiliary_power_w` as OCHRE does
- **Impact**: Electric furnace blower power from HPXML `ElectricAuxiliaryEnergy` is silently dropped

### Gas Furnace (fan_power_w from auxiliary_power_w)

| HPXML Field | HARES Config Field | Parsed? | Wired? | Issue |
|-------------|-------------------|---------|--------|-------|
| `ElectricAuxiliaryEnergy` | `auxiliary_power_w` | ✓ | ✗ | Same issue as Electric Furnace |

**Details:**
- Same wiring issue as Electric Furnace - `auxiliary_power_w` is parsed but not used as fallback for `fan_power_w`
- Only `fan_power_w` from extension element is wired

---

## Category: DER - PV

### PV System (inverter_capacity_kw)

| HPXML Field | HARES Config Field | Parsed? | Wired? | Issue |
|-------------|-------------------|---------|--------|-------|
| `<Inverter><MaxPowerOutput>` | `inverter_capacity_kw` | ✗ | ✗ | Not parsed at all |
| Existing: hardcoded to `None` | `inverter_capacity_kw` | N/A | ✗ | Always None |

**Details:**
- **Parsed in**: N/A - not implemented
- **Hardcoded in resolve**: `resolve_der.rs` line 66
  ```rust
  let cfg = PvConfig {
      ...
      inverter_capacity_kw: None,  // Always None!
      ...
  };
  ```
- **Impact**: PV inverter clipping cannot be modeled - DC/AC ratio is always unlimited. This prevents accurate simulation of systems with inverter undersizing.
- **Note**: This is documented in `pv_review.md` as Issue #4 (Low severity)

---

## Category: Water Heater (documented from earlier reviews)

### Electric Resistance Water Heater

| HPXML Field | HARES Config Field | Parsed? | Wired? | Issue |
|-------------|-------------------|---------|--------|-------|
| `WaterHeaterInsulation/R-value` | (none) | ✗ | N/A | Not parsed at all |

**Details:**
- This is documented in `water_heater_electric_resistance.md` as a missing feature
- OCHRE parses and uses insulation R-value for standby losses
- HARES does not parse this HPXML element

---

## Previously Documented Wiring Issues (from equipment reviews)

| Equipment | Issue | Location |
|-----------|-------|----------|
| PV | Inverter capacity not wired from HPXML | `pv_review.md` Issue #4 |
| ER Water Heater | WaterHeaterInsulation not wired | `water_heater_electric_resistance.md` |
| Electric Furnace | auxiliary_power_w not used for fan_power_w | `docs/tickets/review/CW-002.md` DEFECT 1 |

---

## Summary

| # | Category | Field | Severity | Status |
|---|----------|-------|----------|--------|
| 1 | Heat Pump | backup_fuel | **High** | Not wired - fuel type ignored |
| 2 | Electric Furnace | auxiliary_power_w → fan_power_w | Medium | Not used as fallback |
| 3 | Gas Furnace | auxiliary_power_w → fan_power_w | Medium | Not used as fallback |
| 4 | PV | inverter_capacity_kw | Low | Not parsed/wired |
| 5 | Water Heater | insulation R-value | Medium | Not parsed (feature gap) |
