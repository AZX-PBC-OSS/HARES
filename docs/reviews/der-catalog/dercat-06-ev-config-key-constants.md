# EV config KEY_* string constants: verify match against HPXML parser
**Review ID**: dercat-06
**Category**: der-catalog
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-equipment/src/ev/config.rs` (KEY_* definitions, `EvConfig`, resolvers)
- `crates/hares-equipment/src/ev/mod.rs` (Raw config path `Ev::new()`, typed config path `Ev::init_typed()`)
- `crates/hares-equipment/src/ev/tests.rs` (test fixture `ev_config()`, HPXML-alias tests)
- `crates/hares-equipment/src/config.rs` (`EquipmentConfig` struct, `get_f64`/`get_str`/`get_bool` helpers)
- `crates/hares-io/src/hpxml/resolve_der.rs` (`resolve_ev()` HPXML parser)
- `crates/hares-io/src/hpxml/resolve_loads.rs` (PlugLoad-to-EV alternate path)

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Equipment/EV.py` (reference EV model: capacity, range, charging level, max power derivation)
- `vendors/EnergyPlus/src/EnergyPlus/` (no EV model in EnergyPlus; no directly relevant reference)

## Findings

### Finding 1: [Severity: critical]
**Description**: Six cold-charging configuration keys have their corresponding `KEY_*` constants gated behind `#[cfg(test)]`, and the Raw config path (`Ev::new`) hardcodes defaults without reading from the config map. Any user-supplied values for these fields in a `ConfigPayload::Raw` map are silently discarded.

**Code Location**:
- Constants gated at `crates/hares-equipment/src/ev/config.rs:26-37`:
  ```rust
  #[cfg(test)]
  pub(super) const KEY_MIN_CHARGE_TEMP_C: &str = "min_charge_temp_c";
  #[cfg(test)]
  pub(super) const KEY_FULL_POWER_TEMP_C: &str = "full_power_temp_c";
  #[cfg(test)]
  pub(super) const KEY_HEATER_POWER_W: &str = "heater_power_w";
  #[cfg(test)]
  pub(super) const KEY_HEATER_THRESHOLD_C: &str = "heater_threshold_c";
  #[cfg(test)]
  pub(super) const KEY_THERMAL_MASS_J_PER_K: &str = "thermal_mass_j_per_k";
  #[cfg(test)]
  pub(super) const KEY_UA_W_PER_K: &str = "ua_w_per_k";
  ```
- Hardcoded defaults at `crates/hares-equipment/src/ev/mod.rs:182-187`:
  ```rust
  min_charge_temp_c: DEFAULT_MIN_CHARGE_TEMP_C,
  full_power_temp_c: DEFAULT_FULL_POWER_TEMP_C,
  heater_power_w: DEFAULT_HEATER_POWER_W,
  heater_threshold_c: DEFAULT_HEATER_THRESHOLD_C,
  thermal_mass_j_per_k: DEFAULT_THERMAL_MASS_J_PER_K,
  ua_w_per_k: DEFAULT_UA_W_PER_K,
  ```
- Contrast with the Typed path at `mod.rs:278-285` which _does_ read these from the typed `EvConfig`:
  ```rust
  self.min_charge_temp_c = c.min_charge_temp_c.unwrap_or(DEFAULT_MIN_CHARGE_TEMP_C);
  self.full_power_temp_c = c.full_power_temp_c.unwrap_or(DEFAULT_FULL_POWER_TEMP_C);
  self.heater_power_w = c.heater_power_w.unwrap_or(DEFAULT_HEATER_POWER_W);
  self.heater_threshold_c = c.heater_threshold_c.unwrap_or(DEFAULT_HEATER_THRESHOLD_C);
  self.thermal_mass_j_per_k = c.thermal_mass_j_per_k.unwrap_or(DEFAULT_THERMAL_MASS_J_PER_K);
  self.ua_w_per_k = c.ua_w_per_k.unwrap_or(DEFAULT_UA_W_PER_K);
  ```

**Root Cause**: The `#[cfg(test)]` gating was presumably added as a safety measure to prevent accidental production use of constants whose corresponding parser logic was never written. However, this effectively *prevents* implementing the parser logic, creating a chicken-and-egg situation. The test fixture at `tests.rs:135-140` successfully reads these fields from a raw `HashMap` using the constants and constructs an `EvConfig` (typed), so the data path exists—it's just the `Ev::new()` Raw→typed bridge that's missing.

**Impact**: **Extremely high**—this is the exact bug class described in the review instructions. A Raw config map containing e.g. `"min_charge_temp_c": -10.0` is silently ignored; the EV will use 0.0°C instead of the intended -10°C. No warning is emitted. The simulation produces wrong results with no indication of misconfiguration.

**Recommendation**: Remove the `#[cfg(test)]` gates from all six constants and add corresponding `config.get_f64()` calls in `Ev::new()`. The test fixture already validates this path works.

---

### Finding 2: [Severity: high]
**Description**: `KEY_CHARGING_STRATEGY` is `#[cfg(test)]` gated, and the Raw config path hardcodes `ChargingStrategy::Immediate`. User-supplied charging strategies in a `ConfigPayload::Raw` map are silently ignored.

**Code Location**:
- Constant at `crates/hares-equipment/src/ev/config.rs:47`:
  ```rust
  #[cfg(test)]
  pub(super) const KEY_CHARGING_STRATEGY: &str = "charging_strategy";
  ```
- Hardcoded at `crates/hares-equipment/src/ev/mod.rs:221`:
  ```rust
  charging_strategy: ChargingStrategy::Immediate { target_soc: 1.0 },
  ```
- The typed path at `mod.rs:327-332` correctly parses the field from `c.charging_strategy`.

**Root Cause**: Same pattern as Finding 1—the constant exists but is only accessible in tests because the `Ev::new()` code path to read and parse it was never implemented.

**Impact**: **High**. An EV configured via Raw config with a non-default charging strategy (e.g., time-of-use, load-following) will silently use immediate charging. This is a behavioral correctness bug with no warning.

---

### Finding 3: [Severity: high]
**Description**: `battery_temp_c` uses different default values in the Raw and Typed config initialization paths—a semantic inconsistency that can produce different simulation results for equivalent configurations.

**Code Location**:
- Raw path at `crates/hares-equipment/src/ev/mod.rs:206`:
  ```rust
  battery_temp_c: config.get_f64(KEY_BATTERY_TEMP_C).unwrap_or(20.0),
  ```
- Typed path at `crates/hares-equipment/src/ev/mod.rs:325`:
  ```rust
  self.battery_temp_c = c.battery_temp_c.unwrap_or(env.weather.outdoor_temp_c);
  ```

**Root Cause**: No named default constant exists for `battery_temp_c` (unlike fields such as `DEFAULT_L1_VOLTAGE_V`). The Raw path uses a literal `20.0` (room temperature), while the Typed path falls back to the environment's outdoor temperature when no explicit value is provided.

**Impact**: **High**. Two identical EV models initialized via different config paths (Raw vs Typed) will behave differently on the same weather file unless the user explicitly provides `battery_temp_c`. For typical weather with outdoor temperatures far from 20°C, this manifests as a significant initialization difference for cold-climate or hot-climate simulations.

**Recommendation**: Define `DEFAULT_BATTERY_TEMP_C` in `config.rs` and use it consistently in both paths, or decide whether outdoor-ambient or room-temperature is the correct fallback and apply it uniformly.

---

### Finding 4: [Severity: medium]
**Description**: `EvConfig::validate()` uses string literals that duplicate `KEY_*` constant values. If a KEY_* constant were ever renamed (e.g., for schema consistency), these validation error messages would silently drift, producing misleading error output.

**Code Location**: `crates/hares-equipment/src/ev/config.rs:307-322`:
```rust
for (name, val) in [
    ("v2l_soc_reserve", self.v2l_soc_reserve),     // KEY_V2L_SOC_RESERVE = "v2l_soc_reserve"
    ("v2g_soc_reserve", self.v2g_soc_reserve),     // KEY_V2G_SOC_RESERVE = "v2g_soc_reserve"
    ("ready_soc", self.ready_soc),                  // KEY_READY_SOC = "ready_soc"
] { ... }
for (name, val) in [
    ("v2l_max_discharge_kw", self.v2l_max_discharge_kw),  // KEY_V2L_MAX_DISCHARGE_KW
    ("v2g_max_discharge_kw", self.v2g_max_discharge_kw),  // KEY_V2G_MAX_DISCHARGE_KW
] { ... }
```

**Root Cause**: These are error-message strings, not config-map lookups, so no silent-default bug exists currently. However, the duplication creates a maintenance hazard.

**Impact**: Low-to-medium. Currently correct (string values match constants), but a future rename of any of these five KEY_* constants would leave validation error messages referencing the old field name, potentially confusing users debugging configuration issues.

**Recommendation**: Replace the string literals with the corresponding `KEY_*` constants.

---

### Finding 5: [Severity: medium]
**Description**: Only 3 of the 27 EV config fields have HPXML-alias `KEY_*_HPXML` constants and corresponding HPXML parser paths. The remaining 24 fields are never parsed from HPXML.

**Code Location**:
- HPXML alias constants defined: `KEY_BATTERY_CAPACITY_HPXML_KWH` (line 11), `KEY_CHARGING_LEVEL_HPXML` (line 13), `KEY_MAX_CHARGING_POWER_HPXML_KW` (line 15)
- HPXML to EvConfig mapping at `crates/hares-io/src/hpxml/resolve_der.rs:200-229` sets all non-core fields to `None`
- No HPXML-to-config mappings exist for: `initial_soc`, `soc_max`, `charging_efficiency`, `chemistry`, `fuel_economy_kwh_per_mi`, `v2l_*`, `v2g_*`, `ready_soc`, `plug_in_policy`, `initial_connection_state`, `l1_current_a`, `l1_voltage_v`, `battery_temp_c`, `min_charge_temp_c`, `full_power_temp_c`, `heater_power_w`, `heater_threshold_c`, `thermal_mass_j_per_k`, `ua_w_per_k`, `charging_strategy`, `power_limit_kw`

**Root Cause**: The current HPXML schema for `<ElectricVehicle>` (as understood by the HARES parser) only defines `BatteryCapacity`, `MaxChargingPower`, and `ChargingLevel`. All other fields are HARES extensions or would come from a different HPXML path.

**Impact**: Medium. While this is likely by design (HPXML limitations), it means that EV configuration from real HPXML files will always use defaults for many behavior-affecting parameters (e.g., V2L/V2G enablement, charging strategy). No mechanism exists to surface a warning when an HPXML file contains unsupported EV elements.

**Verification against vendor reference**: OCHRE's `EV.py` similarly only consumes `vehicle_type`, `charging_level`, `capacity`/`range`, and `max_power` from configuration—consistent with the HARES approach. EVI-Pro assumptions (fuel economy, efficiency, power levels) embedded in OCHRE match HARES defaults exactly:
- `EV_FUEL_ECONOMY = 1/325*1000 = 3.077 mi/kWh` ↔ `DEFAULT_FUEL_ECONOMY_KWH_PER_MI = 0.325`
- `EV_EFFICIENCY = 0.9` ↔ `DEFAULT_EFFICIENCY = 0.9`
- `L1 max = 1.4 kW`, `L2 max = [3.6, 3.6, 7.2, 11.5]` ↔ `default_max_power_kw()` logic

---

### Finding 6: [Severity: low]
**Description**: No deprecated key migration mechanism exists for EV configuration. If any KEY_* constant were renamed (e.g., `capacity_kwh` → `battery_capacity_kwh` for schema consistency), old config files using the former key would silently fall through to defaults with no warning.

**Code Location**:
- No EV-specific code for alias resolution, migration warnings, or deprecated-key handling anywhere in `crates/hares-equipment/src/ev/` or `crates/hares-io/src/hpxml/`
- Other equipment modules (water heater, scheduled load) implement `if let Some(v) = config.get("new_key") { ... } else if let Some(v) = config.get("old_key") { warn!(...); ... }` patterns—EV has none

**Root Cause**: EV config keys have been stable since inception; no migration has been needed yet. The `#[serde(deny_unknown_fields)]` attribute on `EvConfig` (line 170) provides protection against _typos_ in typed config by rejecting unknown fields at deserialization, but it does not help with key renames—the old key simply becomes unknown.

**Impact**: Low. No current renamed keys, and the `deny_unknown_fields` guard catches additions/typos in the typed path. The Raw path lacks this guard entirely, relying on `get_f64` returning `None` for unknown keys, which makes it vulnerable to silent ignore of misspelled keys even without renames.

**Recommendation**: Consider adding a Raw-config validation pass that reports unrecognized keys as warnings (or errors in strict mode). This would catch both typos and future renames.

---

## Summary
- **Total findings**: 6
- **Critical**: 1 (cold-charging parameters silently ignored in Raw path)
- **High**: 2 (charging strategy silently ignored in Raw path; `battery_temp_c` default inconsistency)
- **Medium**: 2 (hardcoded strings in `validate()`; limited HPXML field coverage)
- **Low**: 1 (no deprecated key migration mechanism)

## All KEY_* Constants Verified Against Parser Paths

Each KEY_* constant's string value was verified for presence in at least one parser path and absence of hardcoded duplication:

| Constant | String Value | Used in `Ev::new()` | Used in resolvers | Notes |
|---|---|---|---|---|
| `KEY_BATTERY_CAPACITY_KWH` | `"capacity_kwh"` | via `resolve_capacity_kwh` | config.rs:77 | OK |
| `KEY_BATTERY_CAPACITY_HPXML_KWH` | `"BatteryCapacity"` | via `resolve_capacity_kwh` | config.rs:78 | Matches HPXML tag (s.x:182) |
| `KEY_CHARGING_LEVEL` | `"charging_level"` | mod.rs:34 | — | OK |
| `KEY_CHARGING_LEVEL_HPXML` | `"ChargingLevel"` | mod.rs:35 | — | Matches HPXML tag (s.x:203) |
| `KEY_MAX_CHARGING_POWER_KW` | `"max_charging_power_kw"` | via `resolve_rated_power_kw` | config.rs:100 | OK |
| `KEY_MAX_CHARGING_POWER_HPXML_KW` | `"MaxChargingPower"` | via `resolve_rated_power_kw` | config.rs:101 | Matches HPXML tag (s.x:191) |
| `KEY_VEHICLE_TYPE` | `"vehicle_type"` | via `vehicle_number_from_config` | config.rs:140 | OK |
| `KEY_RANGE_MILES` | `"range_miles"` | via `resolve_capacity_kwh` | config.rs:86,136 | OK |
| `KEY_INITIAL_SOC` | `"initial_soc"` | mod.rs:203 | — | OK |
| `KEY_INITIAL_CONNECTION_STATE` | `"initial_connection_state"` | mod.rs:154 | — | OK |
| `KEY_SOC_MAX` | `"soc_max"` | mod.rs:151 | — | OK |
| `KEY_EFFICIENCY` | `"charging_efficiency"` | mod.rs:175 | — | Key name ≠ field name; intentional alias |
| `KEY_POWER_LIMIT_KW` | `"power_limit_kw"` | mod.rs:226 | — | OK |
| `KEY_L1_CURRENT_A` | `"l1_current_a"` | mod.rs:176 | — | OK |
| `KEY_L1_VOLTAGE_V` | `"l1_voltage_v"` | mod.rs:178 | — | OK |
| `KEY_BATTERY_TEMP_C` | `"battery_temp_c"` | mod.rs:206 | — | Default 20.0 (literal, not const) |
| `KEY_MIN_CHARGE_TEMP_C` | `"min_charge_temp_c"` | **NOT USED** in prod | — | `#[cfg(test)]` — see Finding 1 |
| `KEY_FULL_POWER_TEMP_C` | `"full_power_temp_c"` | **NOT USED** in prod | — | `#[cfg(test)]` — see Finding 1 |
| `KEY_HEATER_POWER_W` | `"heater_power_w"` | **NOT USED** in prod | — | `#[cfg(test)]` — see Finding 1 |
| `KEY_HEATER_THRESHOLD_C` | `"heater_threshold_c"` | **NOT USED** in prod | — | `#[cfg(test)]` — see Finding 1 |
| `KEY_THERMAL_MASS_J_PER_K` | `"thermal_mass_j_per_k"` | **NOT USED** in prod | — | `#[cfg(test)]` — see Finding 1 |
| `KEY_UA_W_PER_K` | `"ua_w_per_k"` | **NOT USED** in prod | — | `#[cfg(test)]` — see Finding 1 |
| `KEY_V2L_ENABLED` | `"v2l_enabled"` | mod.rs:188 | — | OK |
| `KEY_V2L_SOC_RESERVE` | `"v2l_soc_reserve"` | mod.rs:190 | — | OK |
| `KEY_V2L_MAX_DISCHARGE_KW` | `"v2l_max_discharge_kw"` | mod.rs:193 | — | OK |
| `KEY_V2G_ENABLED` | `"v2g_enabled"` | mod.rs:195 | — | OK |
| `KEY_V2G_SOC_RESERVE` | `"v2g_soc_reserve"` | mod.rs:197 | — | OK |
| `KEY_V2G_MAX_DISCHARGE_KW` | `"v2g_max_discharge_kw"` | mod.rs:200 | — | OK |
| `KEY_READY_SOC` | `"ready_soc"` | mod.rs:212 | — | OK |
| `KEY_FUEL_ECONOMY_KWH_PER_MI` | `"fuel_economy_kwh_per_mi"` | mod.rs:164 | — | OK |
| `KEY_CHEMISTRY` | `"chemistry"` | mod.rs:159 | — | OK |
| `KEY_CHARGING_STRATEGY` | `"charging_strategy"` | **NOT USED** in prod | — | `#[cfg(test)]` — see Finding 2 |
| `KEY_PLUG_IN_POLICY` | `"plug_in_policy"` | mod.rs:223 | — | OK |

## Recommendations

1. **Remove `#[cfg(test)]` from all seven config-key constants** (`KEY_MIN_CHARGE_TEMP_C`, `KEY_FULL_POWER_TEMP_C`, `KEY_HEATER_POWER_W`, `KEY_HEATER_THRESHOLD_C`, `KEY_THERMAL_MASS_J_PER_K`, `KEY_UA_W_PER_K`, `KEY_CHARGING_STRATEGY`) and add the corresponding `config.get_f64()`/`config.get_str()` calls in `Ev::new()`. The test fixture in `tests.rs` already validates the data path works; only the production `Ev::new()` is missing the parser logic.

2. **Define `DEFAULT_BATTERY_TEMP_C`** and use it consistently in both `Ev::new()` and `Ev::init_typed()` instead of the diverging `20.0` literal and `env.weather.outdoor_temp_c` fallback.

3. **Replace hardcoded strings in `EvConfig::validate()`** (lines 307-322) with `KEY_V2L_SOC_RESERVE`, `KEY_V2G_SOC_RESERVE`, `KEY_READY_SOC`, `KEY_V2L_MAX_DISCHARGE_KW`, `KEY_V2G_MAX_DISCHARGE_KW` constants.

4. **Add a Raw config unknown-key warning mechanism** — iterate the raw data keys after construction and warn (or error) on any key not recognized by a `KEY_*` constant. This would catch typos, renamed keys, and the exact bug class described in this review.

5. **Add `KEY_RANGE_MILES_HPXML` and `KEY_VEHICLE_TYPE_HPXML`** if HPXML `/ElectricVehicle/RangeMiles` or `/ElectricVehicle/VehicleType` elements are ever added to the HPXML schema, to maintain consistency with the existing HPXML alias pattern.

## References / Citations
- OCHRE `EV.py` lines 10-16: EVI-Pro constants and `EV_MAX_POWER` dictionary — reference values verified as matching HARES defaults
- OCHRE `EV.py` lines 37-71: Config inputs (`vehicle_type`, `charging_level`, `capacity`, `range`, `max_power`) — consistent with HARES required HPXML fields
- HPXML `resolve_ev()` at `crates/hares-io/src/hpxml/resolve_der.rs:172-239` — XML tag names `"BatteryCapacity"`, `"MaxChargingPower"`, `"ChargingLevel"` verified to match `KEY_*_HPXML` constants
- `EquipmentConfig::get_f64()` at `crates/hares-equipment/src/config.rs:187-189` — returns `None` for unknown keys with no warning
- `EvConfig` `#[serde(deny_unknown_fields)]` at `crates/hares-equipment/src/ev/config.rs:170` — typed path rejects unknown fields; Raw path has no equivalent protection
