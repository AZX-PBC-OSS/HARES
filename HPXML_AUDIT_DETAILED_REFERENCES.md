# HPXML Parsing Audit: Detailed Code References

This document provides exact line numbers and code snippets for each finding.

---

## Finding 1: Number of Speeds Not Determined

### OCHRE: Multi-tier speed determination
**File:** `vendors/OCHRE/ochre/utils/hpxml.py`
**Lines:** 860-874

```python
860 │   # Get number of speeds
861 │   speed_options = {
862 │       "single stage": 1,
863 │       "two stage": 2,
864 │       "variable speed": 4,
865 │   }
866 │   if name == "mini-split":
867 │       number_of_speeds = 4  # MSHP always variable speed
868 │   elif hvac.get("CompressorType") in speed_options:
869 │       number_of_speeds = speed_options[hvac.get("CompressorType")]
870 │   elif convert(cop, "W", "Btu/hour") <= 15:
871 │       number_of_speeds = 1  # Single-speed for SEER <= 15
872 │   elif convert(cop, "W", "Btu/hour") <= 21:
873 │       number_of_speeds = 2  # Two-speed for 15 < SEER <= 21
874 │   else:
875 │       number_of_speeds = 4  # Variable speed for SEER > 21
```

**Output in equipment dict (line 908):**
```python
908 │       "Number of Speeds (-)": number_of_speeds,
```

### HARES: CompressorType only, no speed count
**File:** `crates/hares-io/src/hpxml/equipment.rs`
**Lines:** 227-234

```rust
227 │       // Compressor type → speed control mode
228 │       if let Some(compressor_type) = child_text(heat_pump, "CompressorType") {
229 │           let mode = match compressor_type.to_ascii_lowercase().as_str() {
230 │               "single stage" => "single_speed",
231 │               "two stage" => "two_speed",
232 │               "variable speed" => "variable_speed",
233 │               _ => "single_speed",
234 │           };
235 │           params.insert("speed_control_mode".to_string(), Value::String(mode.to_string()));
236 │       }
```

**Missing:** No equivalent to OCHRE line 908 (number_of_speeds parameter)

### Related HVAC resolution functions
**HARES:** `crates/hares-io/src/hpxml/equipment.rs:84-274` (resolve_hvac function)
- Lines 166-271: Heat pump handling (no speed count)
- Lines 94-130: Heating system handling (no speed count)
- Lines 132-164: Cooling system handling (no speed count)

---

## Finding 2: Startup Capacity Degradation Not Calculated

### OCHRE: Calculates C_D for AC/HP
**File:** `vendors/OCHRE/ochre/utils/hpxml.py`
**Lines:** 912-915

```python
912 │   # Add startup capacity degradation factor for AC and heat pumps
913 │   if has_heat_pump or not is_heater:
914 │       c_d = utils_equipment.calc_c_d(is_heater, name, cop, number_of_speeds)
915 │       out["Startup Capacity Degradation (-)"] = c_d
```

### HARES: No C_D calculation
**File:** `crates/hares-io/src/hpxml/equipment.rs`

Searched entire file for:
- `startup`
- `degradation`
- `c_d`
- `Startup Capacity`

Result: **No matches.** Parameter completely absent.

### OCHRE calc_c_d implementation reference
**File:** `vendors/OCHRE/ochre/utils/equipment.py`

The `calc_c_d()` function needs to be ported or called during HARES equipment resolution.

---

## Finding 3: Water Heater UA Calculation Incomplete

### OCHRE: Comprehensive DOE 10 CFR 430 physics
**File:** `vendors/OCHRE/ochre/utils/hpxml.py`
**Lines:** 1011-1161 (parse_water_heater function)

**Storage tank UA calculation (lines 1075-1124):**
```python
1075 │   elif water_heater_type == "storage water heater":
     │       ... [DOE test setup] ...
1081 │       if energy_factor is not None:
1082 │           t = 135.0  # F
1083 │           volume_drawn = 64.3  # gal/day
     │       else:
     │           t = 125.0  # F
     │           if first_hour_rating < 18.0:
     │               volume_drawn = 10.0  # gal
     │           ... [UEF case] ...
1116 │       if not is_electric:
     │           if energy_factor is not None:
     │               ua = (recovery_efficiency / energy_factor - 1.0) / (...)
     │           else:
     │               ua = ((recovery_efficiency / uniform_energy_factor) - 1.0) / (...)
     │       else:
     │           if energy_factor is not None:
     │               ua = q_load * (1.0 / energy_factor - 1.0) / ((t - t_env) * 24.0)
     │           else:
     │               ua = q_load * (1.0 / uniform_energy_factor - 1.0) / (...)
```

**Tank jacket integration (lines 1129-1137):**
```python
1129 │   # Increase insulation from tank jacket (reduces UA)
1130 │   if tank_jacket_r:
1131 │       jacket_insulation = 5.0  # R5, in F-ft2-hr/Btu
1132 │       jacket_thickness = 1 if is_electric and energy_factor < 0.7 else 2  # in inches
1133 │       diameter = 2 * convert((volume / 1000 / height / math.pi) ** 0.5, "m", "ft")  # in ft
1134 │       a_side = math.pi * diameter * convert(height, "m", "ft")
1135 │       u_pre_skin = 1.0 / (jacket_thickness * jacket_insulation + 1.0 / 1.3 + 1.0 / 52.8)
1136 │       ua -= tank_jacket_r / (1.0 / u_pre_skin + tank_jacket_r) * u_pre_skin * a_side
```

**HPWH fixed UA (lines 1063-1073):**
```python
1063 │   elif water_heater_type == "heat pump water heater":
1064 │       assert is_electric
1065 │       eta_c = 1
1066 │       # HPWH UA calculation taken from ResStock:
1067 │       if volume_gal <= 58.0:
1068 │           ua = 3.6
1069 │       elif volume_gal <= 73.0:
1070 │           ua = 4.0
1071 │       else:
1072 │           ua = 4.7
```

**HPWH COP calculation (lines 1165-1185):**
```python
1165 │   if water_heater_type == "heat pump water heater":
1166 │       # add HPWH COP, from ResStock, defaults to using UEF
1167 │       if uniform_energy_factor is None:
1168 │           uniform_energy_factor = (0.60522 + energy_factor) / 1.2101
1169 │
1170 │       # Add/update parameters for low power HPWH
1171 │       if uniform_energy_factor == 4.9:
1172 │           wh.update({
     │               "Low Power HPWH": True,
     │               "HPWH COP (-)": 4.2,
     │               "HPWH Capacity (W)": 1499.4,
     │               ...
     │           })
     │       else:
     │           wh["HPWH COP (-)"] = 1.174536058 * uniform_energy_factor
```

### HARES: Basic extraction + separate UA call
**File:** `crates/hares-io/src/hpxml/equipment.rs`
**Lines:** 336-452 (resolve_water_heaters function)

**Field extraction (lines 349-365):**
```rust
349 │       let energy_factor = child_f64(wh, "EnergyFactor");
350 │       let uniform_energy_factor = child_f64(wh, "UniformEnergyFactor");
351 │       let tank_volume_rated_gal = child_f64(wh, "TankVolume");
352 │       let first_hour_rating_gal = child_f64(wh, "FirstHourRating");
353 │       let heating_capacity_btu_hr = child_f64(wh, "HeatingCapacity");
354 │       let recovery_efficiency = child_f64(wh, "RecoveryEfficiency");
     │
     │       ... [other fields] ...
     │
363 │       if let Some(temp_c) = child_temperature_c(wh) {
364 │           params.insert("setpoint_c".to_string(), json!(temp_c));
365 │       }
```

**UA calculation call (lines 389-411):**
```rust
389 │       let category = wh_category(&wh_type, fuel);
390 │       let ua_inputs = UaInputs {
391 │           category,
392 │           energy_factor,
393 │           uniform_energy_factor,
394 │           tank_volume_rated_gal,
395 │           recovery_efficiency,
396 │           heating_capacity_btu_hr,
397 │           first_hour_rating_gal,
398 │       };
399 │       match ua_from_energy_factor(&ua_inputs) {
400 │           Ok(Some(ua_result)) => {
401 │               params.insert("ua_w_per_k".to_string(), json!(ua_result.ua_w_per_k));
402 │           }
     │           ...
```

**Tank jacket handling (lines 428-438):**
```rust
428 │       // Tank jacket R-value
429 │       if let Some(jacket_r) = wh
430 │           .path(&["WaterHeaterInsulation", "Jacket", "JacketRValue"])
431 │           .and_then(|n| n.text.trim().parse::<f64>().ok())
432 │       {
433 │           // Convert hr·ft²·°F/BTU → m²·K/W
434 │           params.insert(
435 │               "jacket_r_value_m2_k_w".to_string(),
436 │               json!(jacket_r * 0.176_110_184),
437 │           );
438 │       }
```

**HPWH COP (lines 413-426):**
```rust
413 │       // HPWH COP from UEF
414 │       if wh_type.contains("heat pump") {
415 │           if let Some(uef) = uniform_energy_factor {
416 │               params.insert("cop".to_string(), json!(1.174_536_058 * uef));
417 │           }
418 │           // HPWH tempering valve: storage at 60°C (140°F), delivery at 51.67°C (125°F)
419 │           let storage_setpoint_c = params
420 │               .get("setpoint_c")
421 │               .and_then(|v| v.as_f64())
422 │               .unwrap_or(60.0);
423 │           if storage_setpoint_c > 51.67 {
424 │               params.insert("tempering_valve_setpoint_c".to_string(), json!(51.67));
425 │           }
426 │       }
```

### Missing HPWH logic in HARES
- No volume-dependent fixed UA (3.6-4.7 W/K range)
- No special handling for 4.9 UEF (low-power HPWH)
- No UEF fallback calculation when EF present but UEF missing

### Instantaneous tank handling
**HARES (line 441-443):**
```rust
441 │       // Tankless performance adjustment
442 │       if wh_type.contains("instantaneous") {
443 │           let perf_adj = child_f64(wh, "PerformanceAdjustment").unwrap_or(0.92);
444 │           params.insert("performance_adjustment".to_string(), json!(perf_adj));
445 │       }
```

**OCHRE (lines 1054-1061):**
```python
1054 │   if water_heater_type == "instantaneous water heater":
1055 │       if energy_factor is None:
1056 │           energy_factor = uniform_energy_factor
1057 │           performance_adjustment = water_heater.get("PerformanceAdjustment", 0.94)
1058 │       else:
1059 │           performance_adjustment = water_heater.get("PerformanceAdjustment", 0.92)
1060 │       eta_c = energy_factor * performance_adjustment
1061 │       ua = 0.0
```

HARES hard-codes 0.92; OCHRE conditionally uses 0.92 or 0.94 based on EF/UEF presence.

---

## Finding 4: Heat Pump Backup Heating Incomplete

### OCHRE: Intelligent fallback for lockout temps
**File:** `vendors/OCHRE/ochre/utils/hpxml.py`
**Lines:** 939-966

```python
939 │   if has_heat_pump and hvac_type == "Heating":
940 │       backup_fuel = heat_pump.get("BackupSystemFuel")
941 │       backup_capacity = convert(heat_pump.get("BackupHeatingCapacity", 0), "Btu/hour", "W")
942 │       backup_cop = heat_pump.get("BackupAnnualHeatingEfficiency", {}).get("Value")
943 │       hp_lockout_temp = heat_pump.get(
944 │           "CompressorLockoutTemperature",
945 │           heat_pump.get("BackupHeatingSwitchoverTemperature", 0),  # Default: 0°F
946 │       )
947 │       hp_lockout_temp = convert(hp_lockout_temp, "degF", "degC")
948 │       er_lockout_temp = heat_pump.get(
949 │           "BackupHeatingLockoutTemperature",
950 │           heat_pump.get("BackupHeatingSwitchoverTemperature", 40),  # Default: 40°F
951 │       )
952 │       er_lockout_temp = convert(er_lockout_temp, "degF", "degC")
953 │       if backup_capacity:
954 │           if backup_fuel != "electricity":
955 │               print(f"WARNING: Using electric resistance backup for ASHP instead of {backup_fuel} backup")
956 │
957 │           out.update({
958 │               "Backup EIR (-)": 1 / backup_cop,
959 │               "Backup Capacity (W)": backup_capacity,
960 │               "Heat Pump Lockout Temperature (C)": hp_lockout_temp,
961 │               "Backup Lockout Temperature (C)": er_lockout_temp,
962 │           })
```

### HARES: Direct extraction without fallbacks
**File:** `crates/hares-io/src/hpxml/equipment.rs`
**Lines:** 187-224

```rust
187 │       // Backup heating parameters
188 │       if let Some(cap_btu) = child_f64(heat_pump, "BackupHeatingCapacity") {
189 │           params.insert("backup_capacity_w".to_string(), json!(cap_btu * 0.293_071_07));
190 │       }
191 │       if let Some(eff_node) = heat_pump.child("BackupAnnualHeatingEfficiency") {
192 │           if let Some(val) = child_f64(eff_node, "Value") {
193 │               // EIR = 1/efficiency for resistance backup
194 │               params.insert("backup_eir".to_string(), json!(1.0 / val.max(0.01)));
195 │           }
196 │       }
197 │       if let Some(fuel) = child_text(heat_pump, "BackupSystemFuel") {
198 │           params.insert("backup_fuel".to_string(), Value::String(fuel));
199 │       }
200 │
201 │       // Lockout temperatures (°F → °C)
202 │       for (xml_keys, param_key) in [
203 │           (
204 │               &[
205 │                   "CompressorLockoutTemperature",
206 │                   "BackupHeatingSwitchoverTemperature",
207 │               ][..],
207 │               "hp_lockout_temp_c",
208 │           ),
209 │           (
210 │               &[
211 │                   "BackupHeatingLockoutTemperature",
212 │                   "BackupHeatingSwitchoverTemperature",
213 │               ][..],
214 │               "er_lockout_temp_c",
215 │           ),
216 │       ] {
217 │           for xml_key in xml_keys {
218 │               if let Some(f_val) = child_f64(heat_pump, xml_key) {
219 │                   params.insert(param_key.to_string(), json!((f_val - 32.0) / 1.8));
220 │               break;
221 │               }
221 │           }
222 │       }
```

**Issue:** Loop (lines 217-221) checks for explicit fields but doesn't apply OCHRE's defaults (0°F for HP, 40°F for ER) when all fields are missing.

---

## Finding 5: HVAC Auxiliary Power Calculation Mismatch

### OCHRE: Multi-case aux power calculation
**File:** `vendors/OCHRE/ochre/utils/hpxml.py`
**Lines:** 885-898

```python
885 │   hvac_ext = hvac.get("extension", {})
886 │   if name == "Boiler":
887 │       # Note: ResStock assumes 2080 hours/year, see hvac.rb line 1754
888 │       aux_power = hvac.get("ElectricAuxiliaryEnergy", 0) / 2080 * 1000  # kWh/year → W
889 │   elif "FanPowerWattsPerCFM" in hvac_ext:
890 │       # Note: air flow rate is only used for non-dynamic HVAC models with fans
891 │       cfm_per_ton = 350 if is_heater else 312
892 │       power_per_cfm = hvac_ext.get("FanPowerWattsPerCFM", 0)
893 │       aux_power = power_per_cfm * cfm_per_ton * convert(capacity, "W", "refrigeration_ton")
894 │   else:
895 │       aux_power = hvac_ext.get("FanPowerWatts", 0)
896 │   else:
897 │       aux_power = 0
```

### HARES: Incomplete handling
**File:** `crates/hares-io/src/hpxml/equipment.rs`

**Heating (lines 112-124):**
```rust
112 │       if let Some(aux_kwh) = child_f64(heating, "ElectricAuxiliaryEnergy") {
113 │           // OCHRE: aux_power = kwh/year / 2080 * 1000 = W
114 │           params.insert(
115 │               "auxiliary_power_w".to_string(),
116 │               json!(aux_kwh / 2080.0 * 1000.0),
117 │           );
118 │       }
119 │       if let Some(ext) = heating.child("extension") {
120 │           if let Some(w_per_cfm) = child_f64(ext, "FanPowerWattsPerCFM") {
121 │               params.insert("fan_power_w_per_cfm".to_string(), json!(w_per_cfm));
122 │           } else if let Some(w) = child_f64(ext, "FanPowerWatts") {
123 │               params.insert("fan_power_w".to_string(), json!(w));
124 │           }
125 │       }
```

**Cooling (lines 153-158):**
```rust
153 │       if let Some(ext) = cooling.child("extension") {
154 │           if let Some(w_per_cfm) = child_f64(ext, "FanPowerWattsPerCFM") {
155 │               params.insert("fan_power_w_per_cfm".to_string(), json!(w_per_cfm));
156 │           } else if let Some(w) = child_f64(ext, "FanPowerWatts") {
157 │               params.insert("fan_power_w".to_string(), json!(w));
158 │           }
159 │       }
```

**Issue:** HARES stores `fan_power_w_per_cfm` as-is; doesn't multiply by CFM (350/312 × capacity_ton) like OCHRE does (line 893).

---

## Finding 6: Duct Parameters Incomplete

### OCHRE: Full duct extraction for ASHRAE 152
**File:** `vendors/OCHRE/ochre/utils/hpxml.py`
**Lines:** 968-1007

```python
968 │   # Get duct info for calculating DSE
969 │   distribution = hvac_all.get("HVACDistribution", {})
     │   ...
972 │   duct_leakage = air_distribution.get("DuctLeakageMeasurement")
973 │   ducts = air_distribution.get("Ducts", [])
974 │   if isinstance(ducts, dict):
975 │       ducts = list(ducts.values())
976 │   ducts = [d for d in ducts if parse_zone_name(d.get("DuctLocation")) not in ["Indoor", None]]
     │
977 │   if f"Annual{hvac_type}DistributionSystemEfficiency" in distribution:
978 │       # Note, ducts are assumed to be in ambient space, DSE losses aren't added to another zone
979 │       out["Ducts"] = {
980 │           "DSE (-)": distribution[f"Annual{hvac_type}DistributionSystemEfficiency"],
981 │           "Zone": None,
982 │       }
983 │   elif duct_leakage is not None and len(ducts):
984 │       # Get parameters to calculate DSE using ASHRAE 152
985 │       # Must be called within HVAC.__init__, as it requires multi-speed parameters
986 │       assert len(ducts) == 2
987 │       assert len(duct_leakage) == 2
988 │       duct_location = ducts[0]["DuctLocation"]
989 │       duct_zone = parse_zone_name(duct_location)
990 │       duct_info = {}
991 │       for duct, duct_leakage, duct_type in zip(ducts, duct_leakage, ["supply", "return"]):
992 │           assert duct["DuctType"] == duct_type
993 │           assert duct_leakage["DuctType"] == duct_type
994 │           assert duct_leakage["DuctLeakage"]["Units"] == "Percent" or duct_leakage["DuctLeakage"]["Value"] == 0
995 │           duct_info.update({
996 │               f"{duct_type.capitalize()} Leakage (-)": duct_leakage["DuctLeakage"]["Value"],
997 │               f"{duct_type.capitalize()} Area (ft^2)": duct["DuctSurfaceArea"],
998 │               f"{duct_type.capitalize()} R Value": duct["DuctInsulationRValue"],
999 │           })
1000│       out["Ducts"] = {
1001│           "Zone": duct_zone,
1002│           **duct_info,
1003│       }
```

### HARES: Minimal duct struct
**File:** `crates/hares-io/src/hpxml/building.rs`
**Lines:** 113-118

```rust
106 │ pub enum DuctLocation {
107 │     InsideConditionedSpace,
108 │     OutsideConditionedSpace,
109 │     Other(String),
110 │ }
111 │
112 │ pub struct DuctSystem {
113 │     pub id: String,
114 │     pub leakage_fraction: Option<f64>,
115 │     pub insulation_r_value_m2_k_w: Option<f64>,
116 │     pub location: DuctLocation,
117 │ }
```

**Missing fields:** `DuctSurfaceArea`, `DuctType` (supply vs return distinction)

---

## Finding 7: Water Heater Location Zone Mapping Missing

### OCHRE: Zone name parsing
**File:** `vendors/OCHRE/ochre/utils/hpxml.py`
**Lines:** 65-84

```python
65 │ def parse_zone_name(hpxml_name):
66 │     if hpxml_name is None:
67 │         return None
68 │
69 │     # Look through all zone name options, return OCHRE name
70 │     for ochre_name, old_name_options in ZONE_NAME_OPTIONS.items():
71 │         if hpxml_name == ochre_name:
72 │             return ochre_name
73 │         for option in old_name_options:
74 │             if hpxml_name == option:
75 │                 return ochre_name
76 │
77 │     # Note, multifamily zones (e.g. 'other housing unit') all include 'other' in the name
78 │     if "other" in hpxml_name:
79 │         return "Adjacent"
80 │
81 │     if hpxml_name is not None:
82 │         print(f'WARNING: Cannot parse zone name "{hpxml_name}". Setting zone to None.')
83 │
84 │     return None
```

**ZONE_NAME_OPTIONS mapping (lines 12-28):**
```python
12 │ ZONE_NAME_OPTIONS = {
13 │     "Indoor": ["conditioned space", "living space"],
14 │     "Foundation": [
15 │         "crawlspace",
16 │         "basement",
17 │         "finishedbasement",
18 │         "basement - conditioned",
19 │         "basement - unconditioned",
20 │         "crawlspace - vented",
21 │         "crawlspace - unvented",
22 │     ],
23 │     "Garage": ["garage"],
24 │     "Attic": ["unfinishedattic", "attic - vented", "attic - unvented"],
25 │     "Outdoor": ["exterior", "outside", "other exterior"],
26 │     "Ground": ["ground"],
27 │     "Adjacent": ["other"],
28 │ }
```

**Applied to WH (line 1152):**
```python
1152│   "Zone": parse_zone_name(water_heater["Location"]),
```

### HARES: Raw location string
**File:** `crates/hares-io/src/hpxml/equipment.rs`
**Lines:** 446-449

```rust
446 │       // Water heater location
447 │       if let Some(location) = child_text(wh, "Location") {
448 │           params.insert("location".to_string(), Value::String(location));
449 │       }
```

**Issue:** Stores raw HPXML text; doesn't call zone name parser.

---

## Finding 8: Efficiency Unit Normalization

### OCHRE: Single COP conversion
**File:** `vendors/OCHRE/ochre/utils/hpxml.py`
**Lines:** 847-856

```python
847 │   efficiency = hvac[f"Annual{hvac_type}Efficiency"]
848 │   if efficiency["Units"] in ["Percent", "AFUE"]:
849 │       cop = efficiency["Value"]
850 │       if efficiency["Units"] == "Percent":
851 │           efficiency["Value"] *= 100
852 │   elif efficiency["Units"] in ["EER", "SEER", "HSPF"]:
853 │       cop = convert(efficiency["Value"], "Btu/hour", "W")
854 │   else:
855 │       raise OCHREException(f"Unknown inputs for HVAC {hvac_type} efficiency: {efficiency}")
856 │   efficiency_string = f"{efficiency['Value']} {efficiency['Units']}"
857 │
858 │   out = {
     │       ...
904 │       "EIR (-)": 1 / cop,
```

### HARES: Multiple efficiency params + SEER2/HSPF2 conversion
**File:** `crates/hares-io/src/hpxml/equipment.rs`
**Lines:** 1189-1241

**normalize_efficiency_units (lines 1232-1241):**
```rust
1232 │ fn normalize_efficiency_units(units: &str, value: f64) -> (String, f64) {
1233 │     match units.trim().to_ascii_uppercase().as_str() {
1234 │         "SEER2" => ("SEER".to_string(), value * SEER2_TO_SEER_FACTOR),  // 1.0/0.95 ≈ 1.053
1235 │         "HSPF2" => ("HSPF".to_string(), value * HSPF2_TO_HSPF_FACTOR),  // 1.0/0.95
1236 │         "SEER" | "EER" | "EER2" | "HSPF" | "AFUE" | "PERCENT" | "COP" => {
1237 │             (units.trim().to_ascii_uppercase(), value)
1238 │         }
1239 │         other => (other.to_string(), value),
1240 │     }
1241 │ }
```

**Stored params (lines 1200-1216):**
```rust
1200 │       params.insert(
1201 │           if is_heating { "heating_efficiency_units".to_string() } else { "cooling_efficiency_units".to_string() },
1202 │           Value::String(normalized_units),
1203 │       );
1204 │       params.insert(
1205 │           if is_heating { "heating_efficiency".to_string() } else { "cooling_efficiency".to_string() },
1206 │           json!(normalized_value),
1207 │       );
```

**Plus direct field extraction (lines 1219-1229):**
```rust
1219 │       for tag in ["SEER", "SEER2", "EER", "EER2", "HSPF", "HSPF2", "AFUE", "COP"] {
1220 │           if let Some(value) = child_f64(node, tag) {
1221 │               let (units, normalized) = normalize_efficiency_units(tag, value);
1222 │               params.insert(
1223 │                   format!("efficiency_{}", units.to_ascii_lowercase()),
1223 │                   json!(normalized),
1224 │               );
1225 │           }
1226 │       }
```

**Constant factors (lines 16-17):**
```rust
16 │ const SEER2_TO_SEER_FACTOR: f64 = 1.0 / 0.95;
17 │ const HSPF2_TO_HSPF_FACTOR: f64 = 1.0 / 0.95;
```

**Issue:** HARES emits both `heating_efficiency_units`+`heating_efficiency` AND `efficiency_seer`, `efficiency_hspf`, etc. OCHRE emits single `EIR (-)`.

---

## Key Constants Comparison

### OCHRE conversion factors
**File:** `vendors/OCHRE/ochre/utils/units.py` (not shown, but referenced)
- Btu/hour to W (for capacity)
- Btu/hour to refrigeration_ton

### HARES conversion factors
**File:** `crates/hares-io/src/hpxml/equipment.rs:15-20`
```rust
15 │ const BTU_PER_HOUR_TO_KBTU_PER_HOUR: f64 = 0.001;
16 │ const SEER2_TO_SEER_FACTOR: f64 = 1.0 / 0.95;
17 │ const HSPF2_TO_HSPF_FACTOR: f64 = 1.0 / 0.95;
18 │ const M2_TO_FT2: f64 = 10.763_910_416_709_722;
19 │ const EV_FUEL_ECONOMY: f64 = 1000.0 / 325.0;
```

**Additional factors in building.rs:10-19**
```rust
10 │ const AREA_FT2_TO_M2: f64 = 0.092_903_04;
11 │ const U_BTU_HR_FT2_F_TO_W_M2_K: f64 = 5.678;
12 │ const R_HR_FT2_F_BTU_TO_M2_K_W: f64 = 0.176_1;
```

---

## Water Heater Category Mapping

**HARES (equipment.rs:604-610):**
```rust
604 │ fn wh_category(wh_type: &str, fuel: FuelType) -> WhCategory {
605 │     match (wh_type.trim(), fuel) {
606 │         ("heat pump water heater", FuelType::Electric) => WhCategory::HeatPump,
607 │         ("instantaneous water heater", _) => WhCategory::Instantaneous,
608 │         (_, FuelType::Electric) => WhCategory::StorageElectric,
609 │         _ => WhCategory::StorageGas,
610 │     }
611 │ }
```

This maps to `UaInputs` which calls `ua_from_energy_factor()` in `water_heater_ua.rs`.

---

## Setpoint Parsing Comparison

### OCHRE setpoint extraction
**File:** `vendors/OCHRE/ochre/utils/hpxml.py:917-937`

Handles both hourly arrays (24 values) and constant values with fallback.

### HARES setpoint extraction
**File:** `crates/hares-io/src/hpxml/equipment.rs:1415-1468`

Function `parse_hvac_setpoint_params()` reads from HVACControl child, converts to Celsius, handles both cases.

**Building struct storage (building.rs:139-143):**
```rust
139 │   /// HVAC thermostat setpoints: 24 hourly values in °C (weekday/weekend).
140 │   pub heating_weekday_setpoints_c: Option<Vec<f64>>,
141 │   pub heating_weekend_setpoints_c: Option<Vec<f64>>,
142 │   pub cooling_weekday_setpoints_c: Option<Vec<f64>>,
143 │   pub cooling_weekend_setpoints_c: Option<Vec<f64>>,
```

**Injection to equipment specs (equipment.rs:301-334):**
```rust
301 │ fn inject_setpoint_profiles(building: &Building, specs: &mut [EquipmentSpec]) {
302 │     for spec in specs.iter_mut() {
303 │         if is_heating_equipment(&spec.name) {
304 │             if let Some(ref wd) = building.heating_weekday_setpoints_c {
305 │                 spec.parameters.insert(
306 │                     "heating_weekday_setpoints_c".to_string(),
307 │                     json!(wd),
308 │                 );
309 │             }
     │             ...
```

This appears to correctly handle hourly arrays. **Status: LIKELY OK, needs verification**.

---

## Summary of Code Locations

| Finding | OCHRE File | OCHRE Lines | HARES File | HARES Lines | Gap |
|---------|-----------|------------|-----------|------------|-----|
| #1 Speed count | hpxml.py | 860-874 | equipment.rs | 227-234 | No int output |
| #2 C_D | hpxml.py | 912-915 | equipment.rs | (none) | Completely absent |
| #3 WH UA | hpxml.py | 1011-1161 | water_heater_ua.rs, equipment.rs | 336-452 | Missing jacket integration |
| #4 HP backup | hpxml.py | 939-966 | equipment.rs | 187-224 | No default fallbacks |
| #5 Aux power | hpxml.py | 885-898 | equipment.rs | 119-124, 153-158 | CFM calc missing |
| #6 Ducts | hpxml.py | 968-1007 | building.rs | 113-118 | DuctSurfaceArea missing |
| #7 WH zone | hpxml.py | 65-84, 1152 | equipment.rs | 446-449 | Raw string, no parsing |
| #8 Efficiency | hpxml.py | 847-856 | equipment.rs | 1189-1241 | Multiple params vs single |

