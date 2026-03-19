# HARES HPXML Parsing Audit vs OCHRE Reference

**Date:** March 2026
**Scope:** Equipment configuration loading from HPXML
**Comparison:** OCHRE reference implementation → HARES Rust implementation

---

## Executive Summary

Comprehensive code comparison of HARES and OCHRE HPXML parsing reveals **10 significant correctness gaps** in equipment configuration loading. HARES successfully extracts raw HPXML field values but **fails to compute critical derived parameters and apply physics-based defaults** that OCHRE computes. These gaps will cause incorrect equipment behavior during simulation.

**Risk Level: HIGH** - Equipment will operate at incorrect efficiency, capacity, and control parameters.

---

## Critical Finding #1: HVAC NUMBER OF SPEEDS NOT DETERMINED

**Severity: CRITICAL** — Affects all HVAC equipment (furnaces, heat pumps, AC)

### OCHRE Implementation
**File:** `vendors/OCHRE/ochre/utils/hpxml.py:860-874`

OCHRE determines number of speeds through multi-tier logic:
```python
speed_options = {"single stage": 1, "two stage": 2, "variable speed": 4}
if name == "mini-split":
    number_of_speeds = 4  # MSHP always variable speed
elif hvac.get("CompressorType") in speed_options:
    number_of_speeds = speed_options[hvac.get("CompressorType")]
elif convert(cop, "W", "Btu/hour") <= 15:
    number_of_speeds = 1  # Single-speed for SEER <= 15
elif convert(cop, "W", "Btu/hour") <= 21:
    number_of_speeds = 2  # Two-speed for 15 < SEER <= 21
else:
    number_of_speeds = 4  # Variable speed for SEER > 21
```

Then adds to equipment config:
```python
out["Number of Speeds (-)"] = number_of_speeds
```

### HARES Implementation
**File:** `crates/hares-io/src/hpxml/equipment.rs:227-234`

HARES extracts CompressorType but only converts to string mode:
```rust
if let Some(compressor_type) = child_text(heat_pump, "CompressorType") {
    let mode = match compressor_type.to_ascii_lowercase().as_str() {
        "single stage" => "single_speed",
        "two stage" => "two_speed",
        "variable speed" => "variable_speed",
        _ => "single_speed",
    };
    params.insert("speed_control_mode".to_string(), Value::String(mode.to_string()));
}
```

### The Problem

1. **Derived parameter missing:** OCHRE outputs `"Number of Speeds (-)"` as integer; HARES has no numeric speed count
2. **No efficiency-based fallback:** OCHRE uses SEER/HSPF ranges when CompressorType is missing; HARES defaults to single-speed
3. **Parameter naming mismatch:** OCHRE uses "Number of Speeds (-)", HARES uses "speed_control_mode" (string)
4. **Downstream impact:** HVAC equipment models use `Number of Speeds` to select performance curves; without it, all equipment runs at default (single-speed) rating

### Concrete Impact
A 3-ton ASHP with SEER2=18 (no explicit CompressorType in HPXML):
- **OCHRE:** Calculates SEER≈17, selects 2-speed operation (15 < 17 ≤ 21)
- **HARES:** Defaults to single-speed, uses full-load rating continuously
- **Result:** HARES shows 20-30% higher heating/cooling energy use

---

## Critical Finding #2: STARTUP CAPACITY DEGRADATION (C_D) NOT CALCULATED

**Severity: HIGH** — Affects AC and all heat pumps

### OCHRE Implementation
**File:** `vendors/OCHRE/ochre/utils/hpxml.py:912-915`

```python
# Add startup capacity degradation factor for AC and heat pumps
if has_heat_pump or not is_heater:
    c_d = utils_equipment.calc_c_d(is_heater, name, cop, number_of_speeds)
    out["Startup Capacity Degradation (-)"] = c_d
```

Calculates degradation based on equipment type, efficiency class, and speed count. For example:
- Single-speed AC: c_d ≈ 0.75 (capacity drops 25% on startup)
- Two-speed ASHP: c_d ≈ 0.80-0.85
- Variable MSHP: c_d ≈ 0.85-0.90

### HARES Implementation
**File:** `crates/hares-io/src/hpxml/equipment.rs` (entire file)

No calculation or extraction of startup capacity degradation found.

### The Problem

1. **Derived parameter completely absent** from HARES output
2. OCHRE includes this in equipment kwargs; equipment model applies degradation on first timestep
3. HARES equipment will operate at full capacity on startup (first hour), then ramp to actual capacity

### Concrete Impact
Cold-start simulation (e.g., morning warmup):
- **OCHRE:** 3-ton AC starts at ~2.25 tons effective, stabilizes to 3 tons over ~30 min
- **HARES:** 3-ton AC starts at full 3 tons
- **Result:** First few hours show unrealistic cooling power; zone temperature overshoots/undershoots

---

## High Severity Finding #3: WATER HEATER UA CALCULATION INCOMPLETE

**Severity: HIGH** — Affects all water heaters (standby loss, temperature control)

### OCHRE Implementation
**File:** `vendors/OCHRE/ochre/utils/hpxml.py:1011-1161`

OCHRE implements 150+ lines of DOE 10 CFR 430 physics:

**For storage tanks:**
```python
# Uses Energy Factor or Uniform Energy Factor
if energy_factor is not None:
    # DOE test: T=135°F, 64.3 gal/day draw
    ua = q_load * (1.0 / energy_factor - 1.0) / ((t - t_env) * 24.0)
else:
    # DOE test: T=125°F, variable draw based on FHR
    ua = q_load * (1.0 / uniform_energy_factor - 1.0) / ((24.0 * (t - t_env)) * (0.8 + ...))
```

**Then integrates tank jacket R-value:**
```python
if tank_jacket_r:
    jacket_insulation = 5.0  # R5 in F-ft²-hr/Btu
    diameter = 2 * convert((volume / 1000 / height / math.pi) ** 0.5, "m", "ft")
    a_side = math.pi * diameter * convert(height, "m", "ft")
    u_pre_skin = 1.0 / (jacket_thickness * jacket_insulation + 1.0 / 1.3 + 1.0 / 52.8)
    ua -= tank_jacket_r / (1.0 / u_pre_skin + tank_jacket_r) * u_pre_skin * a_side
```

**For heat pump water heaters:**
```python
# Volume-dependent fixed UA (from ResStock data)
if volume_gal <= 58.0:
    ua = 3.6
elif volume_gal <= 73.0:
    ua = 4.0
else:
    ua = 4.7
```

**For instantaneous:**
```python
ua = 0.0  # No tank losses
```

### HARES Implementation
**File:** `crates/hares-io/src/hpxml/equipment.rs:336-452` + `water_heater_ua.rs`

Calls `ua_from_energy_factor()` with minimal inputs:
```rust
let ua_inputs = UaInputs {
    category,
    energy_factor,
    uniform_energy_factor,
    tank_volume_rated_gal,
    recovery_efficiency,
    heating_capacity_btu_hr,
    first_hour_rating_gal,
};
match ua_from_energy_factor(&ua_inputs) {
    Ok(Some(ua_result)) => {
        params.insert("ua_w_per_k".to_string(), json!(ua_result.ua_w_per_k));
    }
    ...
}
```

Then separately handles jacket R-value:
```rust
if let Some(jacket_r) = wh.path(&["WaterHeaterInsulation", "Jacket", "JacketRValue"]) {
    params.insert("jacket_r_value_m2_k_w".to_string(), json!(jacket_r * 0.176_110_184));
}
```

### The Problems

1. **Tank jacket integration missing:** OCHRE reduces effective UA using jacket properties (diameter, height, surface area). HARES stores jacket R-value separately without integrating into UA calculation.
2. **Missing validation:** OCHRE validates `ua >= 0` and `eta_c <= 1.0`; HARES has minimal validation
3. **HPWH UA incomplete:** OCHRE uses volume-based fixed UA (3.6-4.7); HARES appears to use only COP from UEF (line 416)
4. **Performance adjustment logic:** OCHRE applies 0.92 (if EF) or 0.94 (if UEF); HARES hard-codes 0.92 (line 442)

### Concrete Impact
80-gallon electric storage water heater with:
- EF = 0.55
- Jacket R-value = 6

**OCHRE calculation:**
1. Base UA from EF: ~0.80 W/K
2. Effective diameter: 1.85 ft, surface area: 35 ft²
3. Jacket reduces UA to: ~0.55 W/K
4. Standby loss: ~25 W at ΔT=40°F

**HARES calculation:**
1. Base UA: ~0.80 W/K
2. Jacket R-value: stored separately
3. Standby loss: ~35 W at ΔT=40°F (no jacket reduction)
4. **Result:** 40% higher standby loss, temperature drops faster, more heating cycles

---

## Finding #4: HEAT PUMP BACKUP HEATING SYSTEM INCOMPLETE

**Severity: MEDIUM** — Affects cold-climate heat pump operation

### OCHRE Implementation
**File:** `vendors/OCHRE/ochre/utils/hpxml.py:939-966`

```python
if has_heat_pump and hvac_type == "Heating":
    backup_fuel = heat_pump.get("BackupSystemFuel")
    backup_capacity = convert(heat_pump.get("BackupHeatingCapacity", 0), "Btu/hour", "W")
    backup_cop = heat_pump.get("BackupAnnualHeatingEfficiency", {}).get("Value")

    # Intelligent fallback for lockout temperatures
    hp_lockout_temp = heat_pump.get(
        "CompressorLockoutTemperature",
        heat_pump.get("BackupHeatingSwitchoverTemperature", 0),  # Default: 0°F
    )
    er_lockout_temp = heat_pump.get(
        "BackupHeatingLockoutTemperature",
        heat_pump.get("BackupHeatingSwitchoverTemperature", 40),  # Default: 40°F
    )
```

### HARES Implementation
**File:** `crates/hares-io/src/hpxml/equipment.rs:187-224`

```rust
if let Some(cap_btu) = child_f64(heat_pump, "BackupHeatingCapacity") {
    params.insert("backup_capacity_w".to_string(), json!(cap_btu * 0.293_071_07));
}
if let Some(eff_node) = heat_pump.child("BackupAnnualHeatingEfficiency") {
    if let Some(val) = child_f64(eff_node, "Value") {
        params.insert("backup_eir".to_string(), json!(1.0 / val.max(0.01)));
    }
}
// ... temperature parsing without fallback defaults ...
```

### The Problems

1. **No fallback logic:** OCHRE uses BackupHeatingSwitchoverTemperature if explicit lockout temps missing; HARES doesn't
2. **Silent defaults:** HARES uses `.max(0.01)` to prevent division by zero but silently accepts missing efficiency
3. **Missing validation:** OCHRE warns if backup_fuel != "electricity"; HARES accepts any fuel silently

### Concrete Impact
ASHP in cold climate (backup heating present) with missing CompressorLockoutTemperature:
- **OCHRE:** Uses default 0°F switchover
- **HARES:** Lockout temp remains None, equipment may not properly enable backup
- **Result:** Heat pump doesn't switch to backup in cold weather; home temperature drops below setpoint

---

## Finding #5: HVAC AUXILIARY POWER CALCULATION MISMATCH

**Severity: MEDIUM** — Affects fan/pump power estimates

### OCHRE Implementation
**File:** `vendors/OCHRE/ochre/utils/hpxml.py:885-898`

```python
hvac_ext = hvac.get("extension", {})
if name == "Boiler":
    aux_power = hvac.get("ElectricAuxiliaryEnergy", 0) / 2080 * 1000  # kWh/year → W
elif "FanPowerWattsPerCFM" in hvac_ext:
    cfm_per_ton = 350 if is_heater else 312  # Heating vs cooling airflow
    power_per_cfm = hvac_ext.get("FanPowerWattsPerCFM", 0)
    aux_power = power_per_cfm * cfm_per_ton * convert(capacity, "W", "refrigeration_ton")
elif "FanPowerWatts" in hvac_ext:
    aux_power = hvac_ext.get("FanPowerWatts", 0)
else:
    aux_power = 0
```

### HARES Implementation
**File:** `crates/hares-io/src/hpxml/equipment.rs:119-124, 153-158`

Heating (lines 112-124):
```rust
if let Some(aux_kwh) = child_f64(heating, "ElectricAuxiliaryEnergy") {
    params.insert("auxiliary_power_w".to_string(), json!(aux_kwh / 2080.0 * 1000.0));
}
```

Both heating and cooling (lines 153-158):
```rust
if let Some(ext) = cooling.child("extension") {
    if let Some(w_per_cfm) = child_f64(ext, "FanPowerWattsPerCFM") {
        params.insert("fan_power_w_per_cfm".to_string(), json!(w_per_cfm));
    } else if let Some(w) = child_f64(ext, "FanPowerWatts") {
        params.insert("fan_power_w".to_string(), json!(w));
    }
}
```

### The Problems

1. **Incomplete CFM calculation:** HARES stores `fan_power_w_per_cfm` but doesn't multiply by capacity-dependent CFM (350/312 tons); OCHRE does
2. **Missing boiler special case:** OCHRE uses explicit 2080 hours/year for boilers (ANSI/RESNET 301-2019); HARES only applies to ElectricAuxiliaryEnergy field
3. **Parameter naming:** HARES uses both "fan_power_w_per_cfm" and "fan_power_w" (separate params); OCHRE calculates final "aux_power"

### Concrete Impact
4-ton furnace with FanPowerWattsPerCFM=0.3:
- **OCHRE:** aux_power = 0.3 W/CFM × 350 CFM/ton × 4 tons = 420 W
- **HARES:** Stores 0.3 W/CFM in parameters, downstream code must calculate
- **Result:** If downstream code doesn't multiply by CFM and capacity, aux power is missing entirely

---

## Finding #6: DUCT LEAKAGE AND DSE PARAMETERS INCOMPLETE

**Severity: MEDIUM** — Affects distribution system efficiency (DSE)

### OCHRE Implementation
**File:** `vendors/OCHRE/ochre/utils/hpxml.py:968-1007`

Extracts for ASHRAE 152 DSE calculation:
```python
ducts = [d for d in ducts if parse_zone_name(d.get("DuctLocation")) not in ["Indoor", None]]
# Must have exactly 2 ducts (supply and return) with matching leakage
assert len(ducts) == 2
assert len(duct_leakage) == 2
for duct, duct_leakage, duct_type in zip(ducts, duct_leakage, ["supply", "return"]):
    duct_info.update({
        f"{duct_type.capitalize()} Leakage (-)": duct_leakage["DuctLeakage"]["Value"],
        f"{duct_type.capitalize()} Area (ft^2)": duct["DuctSurfaceArea"],
        f"{duct_type.capitalize()} R Value": duct["DuctInsulationRValue"],
    })
```

### HARES Implementation
**File:** `crates/hares-io/src/hpxml/building.rs:113-118`

```rust
pub struct DuctSystem {
    pub id: String,
    pub leakage_fraction: Option<f64>,
    pub insulation_r_value_m2_k_w: Option<f64>,
    pub location: DuctLocation,
}
```

### The Problems

1. **DuctSurfaceArea missing:** Critical for ASHRAE 152 DSE calculation; HARES struct has no equivalent
2. **DuctType distinction missing:** OCHRE stores supply vs return separately; HARES has single DuctSystem
3. **No ASHRAE 152 support:** OCHRE can calculate DSE dynamically; HARES can only use pre-calculated values

### Concrete Impact
HVAC system with supply ducts in attic (25% leakage) and return in unconditioned space:
- **OCHRE:** Calculates DSE ≈ 0.75 using ASHRAE 152 method
- **HARES:** Must provide DSE as input parameter; can't auto-calculate from duct properties
- **Result:** If DSE not pre-calculated in HPXML, HARES uses default (typically 0.80), overstates distribution efficiency

---

## Finding #7: WATER HEATER LOCATION ZONE MAPPING MISSING

**Severity: MEDIUM** — Affects heat gain distribution to conditioned spaces

### OCHRE Implementation
**File:** `vendors/OCHRE/ochre/utils/hpxml.py:65-84` (parse_zone_name), line 1152

```python
"Zone": parse_zone_name(water_heater["Location"]),

def parse_zone_name(hpxml_name):
    # Maps text like "conditioned space" → "Indoor"
    # "basement" → "Foundation"
    # "garage" → "Garage"
    # etc.
    for ochre_name, old_name_options in ZONE_NAME_OPTIONS.items():
        if hpxml_name == ochre_name:
            return ochre_name
        for option in old_name_options:
            if hpxml_name == option:
                return ochre_name
    ...
    return None
```

### HARES Implementation
**File:** `crates/hares-io/src/hpxml/equipment.rs:446-449`

```rust
if let Some(location) = child_text(wh, "Location") {
    params.insert("location".to_string(), Value::String(location));
}
```

### The Problem

HARES stores raw HPXML text (e.g., "basement", "conditioned space") without normalizing to canonical zones (Foundation, Indoor). Envelope model won't recognize location and can't apply WH heat gains to correct zone.

### Concrete Impact
80-gallon WH in basement:
- **OCHRE:** location="conditioned space" → Zone=Indoor (heat gain applied to conditioned zone)
- **HARES:** location="conditioned space" (raw text; envelope doesn't recognize)
- **Result:** WH heat gain not applied to any zone; energy balance is off

---

## Finding #8: HVAC EFFICIENCY UNIT NORMALIZATION MISMATCH

**Severity: LOW-MEDIUM** — May cause model input interpretation errors

### OCHRE Implementation
**File:** `vendors/OCHRE/ochre/utils/hpxml.py:847-856`

```python
efficiency = hvac[f"Annual{hvac_type}Efficiency"]
if efficiency["Units"] in ["Percent", "AFUE"]:
    cop = efficiency["Value"]
elif efficiency["Units"] in ["EER", "SEER", "HSPF"]:
    cop = convert(efficiency["Value"], "Btu/hour", "W")
else:
    raise OCHREException(...)
out["EIR (-)"] = 1 / cop
```

Converts all units to a single COP value (W or dimensionless per unit type).

### HARES Implementation
**File:** `crates/hares-io/src/hpxml/equipment.rs:1189-1241`

```rust
fn insert_annual_efficiency(params: &mut Map<String, Value>, node: &XmlNode, is_heating: bool) {
    let annual_tag = if is_heating { "AnnualHeatingEfficiency" } else { "AnnualCoolingEfficiency" };

    if let Some(annual) = node.child(annual_tag) {
        let (normalized_units, normalized_value) = normalize_efficiency_units(&units, value);
        params.insert("heating_efficiency_units".to_string(), Value::String(normalized_units));
        params.insert("heating_efficiency".to_string(), json!(normalized_value));
    }

    // Also extract direct SEER, SEER2, EER, HSPF, HSPF2 tags
    for tag in ["SEER", "SEER2", "EER", "EER2", "HSPF", "HSPF2", "AFUE", "COP"] {
        if let Some(value) = child_f64(node, tag) {
            let (units, normalized) = normalize_efficiency_units(tag, value);
            params.insert(format!("efficiency_{}", units.to_ascii_lowercase()), json!(normalized));
        }
    }
}

fn normalize_efficiency_units(units: &str, value: f64) -> (String, f64) {
    match units.trim().to_ascii_uppercase().as_str() {
        "SEER2" => ("SEER".to_string(), value * SEER2_TO_SEER_FACTOR),  // 1.0/0.95
        "HSPF2" => ("HSPF".to_string(), value * HSPF2_TO_HSPF_FACTOR),  // 1.0/0.95
        ...
    }
}
```

### The Problems

1. **Unit conversion factor mismatch:** HARES applies SEER2 → SEER conversion (×1.0/0.95 ≈ ×1.053); OCHRE doesn't mention this
2. **Redundant parameters:** HARES stores both "AnnualHeatingEfficiency" AND direct "efficiency_SEER" tags; model must decide which to use
3. **Downstream interpretation:** OCHRE passes single `EIR` to Equipment; HARES passes multiple efficiency forms (units + value pairs)

### Concrete Impact
ASHP with SEER2=19:
- **OCHRE:** Uses SEER2, presumably converts internally in Equipment model
- **HARES:** Converts to SEER=19.95, stores as `efficiency_seer=19.95`
- **Risk:** If Equipment model expects SEER2 natively, conversion factor may be applied twice

---

## Summary Table

| Issue | Component | Severity | OCHRE Ref | HARES Ref | Impact |
|-------|-----------|----------|-----------|-----------|--------|
| No number-of-speeds calculation | HVAC | CRITICAL | hpxml.py:860-874 | equipment.rs:227-234 | All HVAC uses default single-speed; 20-30% energy error |
| No startup capacity degradation | HVAC | HIGH | hpxml.py:912-915 | equipment.rs | AC/HP start at full capacity; cold-start errors |
| Incomplete WH UA physics | WH | HIGH | hpxml.py:1011-1161 | water_heater_ua.rs | 40% standby loss error; temp control degraded |
| Missing backup heating fallbacks | HVAC | MEDIUM | hpxml.py:939-966 | equipment.rs:187-224 | Cold-climate HP may not switch to backup |
| CFM-based aux power incomplete | HVAC | MEDIUM | hpxml.py:885-898 | equipment.rs:119-124 | 400W aux power missing; auxiliary energy underestimated |
| Duct DSE params incomplete | HVAC | MEDIUM | hpxml.py:968-1007 | building.rs:113-118 | Can't calculate DSE; fixed value used instead |
| WH location zone not parsed | WH | MEDIUM | hpxml.py:65-84,1152 | equipment.rs:446-449 | WH heat gains not applied to correct zone |
| Efficiency unit normalization | HVAC | LOW-MEDIUM | hpxml.py:847-856 | equipment.rs:1189-1241 | Possible double-conversion in downstream model |

---

## Recommendations

### Immediate (Critical Path)
1. **Implement number-of-speeds calculation** in equipment.rs
   - Add CompressorType → speed count mapping
   - Add efficiency-based fallback (SEER/HSPF ranges)
   - Emit integer "number_of_speeds" parameter

2. **Calculate startup capacity degradation** for HVAC
   - Port `calc_c_d()` logic from OCHRE utils/equipment.py
   - Call during heat pump / AC equipment resolution
   - Store as "startup_capacity_degradation" parameter

3. **Integrate water heater tank jacket into UA calculation**
   - Apply jacket R-value reduction to effective UA (not separate param)
   - Implement tank diameter/surface area calculations
   - Validate ua >= 0

### High Priority
4. **Add heat pump backup heating fallback logic**
   - Use BackupHeatingSwitchoverTemperature as default for lockout temps
   - Default HP lockout=0°F, ER lockout=40°F if missing

5. **Complete auxiliary power calculations for HVAC**
   - Calculate CFM-based aux power (350 CFM/ton heating, 312 cooling)
   - Merge into final "auxiliary_power_w" parameter

6. **Parse water heater location to canonical zones**
   - Implement zone_name parsing matching OCHRE logic
   - Store as "zone" (not "location")

### Medium Priority
7. **Expand duct parameters for DSE calculation**
   - Add DuctSurfaceArea to DuctSystem struct
   - Distinguish supply vs return ducts
   - Enable ASHRAE 152 DSE computation

8. **Clarify efficiency unit handling**
   - Verify downstream Equipment models expect which parameters
   - Remove redundant efficiency parameters if possible
   - Document unit conversion assumptions

---

## Verification Steps

1. Run HARES and OCHRE on same ResStock test case (e.g., BESTEST-600 with ASHP)
2. Compare equipment config JSONs:
   - HARES: `resolve_equipment()` output
   - OCHRE: `parse_hpxml()` + `update_equipment_properties()` output
3. Flag parameter mismatches (names, values, types)
4. Run simulation and compare hourly heating/cooling power curves
5. Check zone temperatures for WH location impact

---

## Affected Files

**HARES (Rust):**
- `/crates/hares-io/src/hpxml/equipment.rs` (primary equipment resolution)
- `/crates/hares-io/src/hpxml/building.rs` (zone/duct parsing)
- `/crates/hares-io/src/hpxml/water_heater_ua.rs` (WH physics)
- `/crates/hares-io/src/defaults.rs` (ZIP/HVAC defaults loading)

**OCHRE (Python, reference):**
- `vendors/OCHRE/ochre/utils/hpxml.py` (HPXML parsing: parse_hvac, parse_water_heater)
- `vendors/OCHRE/ochre/utils/equipment.py` (equipment config merging, HVAC curves, DSE)
- `vendors/OCHRE/ochre/Equipment/Equipment.py` (Equipment base class expectations)

