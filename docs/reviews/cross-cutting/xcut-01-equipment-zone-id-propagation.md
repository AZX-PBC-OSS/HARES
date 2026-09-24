# Equipment zone_id propagation: HPXML → config → init → step → PortContribution, no zone_id=0 leakage
**Review ID**: xcut-01
**Category**: cross-cutting
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-core/src/dwelling/mod.rs` (6041 lines, dwelling orchestrator)
- `crates/hares-core/src/dwelling/conversions.rs` (equipment_config_from_spec, merged_equipment_config)
- `crates/hares-io/src/hpxml/resolve_hvac.rs` (lines 627–1220, HVAC typed config builders; 1580–1735, resolve_hvac; 88–146, DuctDseParams)
- `crates/hares-io/src/hpxml/resolve_water_heater.rs` (lines 20–230, water heater typed config builders)
- `crates/hares-io/src/hpxml/equipment.rs` (resolve_equipment orchestrator)
- `crates/hares-equipment/src/hvac/hvac_core.rs` (HvacConfig with zone_id, zone_heat_fractions)
- `crates/hares-equipment/src/hvac/duct_distribution.rs` (write_zone_thermal_contributions)
- `crates/hares-equipment/src/hvac/helpers.rs` (zone_id_from_config, validate_u16_id)
- `crates/hares-equipment/src/hvac/baseboard.rs` (ElectricBaseboard new/init/step)
- `crates/hares-equipment/src/water_heater/heat_pump_wh.rs` (HPWH new/init/step)
- `crates/hares-equipment/src/water_heater/resistance.rs` (ResistanceWH new/init)
- `crates/hares-equipment/src/water_heater/gas.rs` (GasWH new/init)
- `crates/hares-equipment/src/water_heater/tankless.rs` (TanklessWH new/init)
- `crates/hares-equipment/src/battery/mod.rs` (Battery new/step thermal routing)
- `crates/hares-equipment/src/pv/mod.rs` (PV zone=None)
- `crates/hares-equipment/src/config.rs` (EquipmentConfig, KEY_ZONE_ID)
- `crates/hares-types/src/ports.rs` (PortContribution enum)

## Vendor/Reference Files Consulted
None

## Findings

### Finding 1: [Severity: critical]
**Description**: Non-baseboard HVAC equipment `zone_id` is never propagated from HPXML — always hardcoded to `ZoneId(1)`.

**Code Location**:
- `crates/hares-io/src/hpxml/resolve_hvac.rs:1693–1697` — `zone_id` is injected into params ONLY for `"Electric Baseboard"`:
  ```rust
  if name == "Electric Baseboard" {
      if let Some(zone_id) = conditioned_zone_id(building) {
          params.insert("zone_id".to_string(), json!(zone_id));
      }
  }
  ```
  No equivalent injection exists for Gas Furnace, Electric Furnace, Gas Boiler, Electric Boiler, Ideal HVAC, Air Conditioner, Room AC, or any heat pump variant.
- `crates/hares-io/src/hpxml/resolve_hvac.rs:663–666` — `try_build_gas_furnace_config()` sets `zone_id: None`:
  ```rust
  let cfg = GasFurnaceConfig {
      equipment_id: None,
      zone_id: None,  // never populated from HPXML
      ...
  };
  ```
- Same pattern in: `try_build_electric_furnace_config` (line 714), `try_build_heat_pump_heater_config` (line 1199), `try_build_heat_pump_cooler_config`, `try_build_central_ac_config`, `try_build_ideal_hvac_config` (line 938).

**Root Cause**: The `conditioned_zone_id()` helper (`resolve_hvac.rs:1580–1586`) exists and correctly returns `(zone_index as u16) + 1`, but is only called for Electric Baseboard. All other HVAC config builders lack the parameter injection. No alternative path converts the HPXML `ZoneType::Conditioned` to a `zone_id` for insertion into params.

**Impact**: For any building with a single conditioned zone at index 0, the fallback `ZoneId(1)` is coincidentally correct. For multi-conditioned-zone HPXML models or buildings where the Conditioned zone is NOT the first zone in `Building::zones`, thermal contributions are routed to the wrong zone. This silently misattributes all heating/cooling energy to zone 1, corrupting per-zone temperature predictions and energy balance telemetry.

### Finding 2: [Severity: high]
**Description**: Water heater `zone_id` is parsed from HPXML as a string label (`zone_type`) but never resolved to a numeric `zone_id`. All water heaters hardcode `ZoneId(1)`.

**Code Location**:
- `crates/hares-io/src/hpxml/resolve_water_heater.rs:34–38` — HPXML `<Location>` element is parsed into a `zone_name` string:
  ```rust
  let location = child_text(wh, "Location");
  let zone_name = location.as_deref().map(|location| {
      let zone_type = super::building::parse_zone_label(location);
      super::building::zone_key(&zone_type)
  });
  ```
  But this string is stored as `zone_type: zone_name.clone()` in the config (lines 121, 151), while `zone_id: None` is used for the actual identity field (lines 98, 133, 169, 222).
- `crates/hares-equipment/src/water_heater/resistance.rs:114` — `zone_id_from_config` returns `None`, falls back to `ZoneId(1)`:
  ```rust
  let zone = zone_id_from_config(&config).unwrap_or(ZoneId(1));
  ```
- Same pattern in `gas.rs:108`, `heat_pump_wh.rs:166`, `tankless.rs:96`.

**Root Cause**: The HPXML `Location` element value (e.g., "conditioned space", "garage") is parsed and stored as a descriptive `zone_type` string but there is no step that maps this string back to an internal `ZoneId`. The `zone_key()` function used at line 37 produces a display key, not a zone index.

**Impact**: Water heater thermal losses (jacket loss, HPWH compressor waste heat) are always routed to zone 1. A water heater in a garage or basement will route its thermal output to the conditioned zone rather than the correct zone, corrupting temperature predictions for both the conditioned zone (receiving spurious gains) and the actual water heater location zone (missing the thermal gain).

### Finding 3: [Severity: high]
**Description**: The `zone_id_from_config()` → `unwrap_or(ZoneId(1))` fallback masks the HPXML parsing gap, making the bug invisible in single-zone test cases.

**Code Location**:
- `crates/hares-equipment/src/hvac/helpers.rs:34–42` — `zone_id_from_config()`:
  ```rust
  pub fn zone_id_from_config(config: &EquipmentConfig) -> Option<ZoneId> {
      let raw = config
          .get_f64(crate::config::KEY_ZONE_ID)
          .or_else(|| typed_f64(config, crate::config::KEY_ZONE_ID))?;
      if !validate_u16_id(raw) {
          return None;
      }
      Some(ZoneId(raw as u16))
  }
  ```
- Every HVAC constructor: `zone_id_from_config(&config).unwrap_or(ZoneId(1))` — `baseboard.rs:58`, `heater.rs:382`, `cooler.rs:51`, `air_conditioner.rs:417`, etc.
- Every water heater constructor: same pattern — `resistance.rs:114`, `gas.rs:108`, `tankless.rs:96`, `heat_pump_wh.rs:166`.

**Root Cause**: The `unwrap_or(ZoneId(1))` default is a convenient but incorrect fallback. It assumes the building always has a single conditioned zone at index 1. Since `zone_id_from_config` returns `None` when the typed config's `zone_id` field is `None` (which it always is per Finding 1), execution always takes this fallback path. No test exercises a multi-zone HPXML or an equipment placed in a non-zone-1 location, so the silent fallback is never detected.

**Impact**: Masks the true extent of Finding 1 and Finding 2. If the fallback were removed (or changed to error out when `zone_id` is missing from typed config), all current HPXML-parsed non-baseboard HVAC and all water heater specs would fail to initialize, revealing the parsing gap. Instead, the simulation runs silently with wrong zone assignments.

### Finding 4: [Severity: medium]
**Description**: Battery `zone_id` is correctly optional (not hardcoded to `ZoneId(1)`) but HPXML parsing never populates it, making indoor battery thermal routing unavailable from HPXML inputs.

**Code Location**:
- `crates/hares-equipment/src/battery/mod.rs:995–1005` — Battery step correctly guards thermal output on `descriptor.zone`:
  ```rust
  if let Some(zone) = self.descriptor.zone {
      if ohmic_loss_w > 0.0 {
          ports.accumulate(&PortContribution::Thermal {
              zone,
              sensible_gain_w: ohmic_loss_w,
              ...
          })?;
      }
  }
  ```
- The battery's `BatteryConfig` has `zone_id: Option<u16>` (config.rs:17). When not set, no thermal contribution is made. This is correct: the battery has no hardcoded zone fallback.
- `crates/hares-io/src/hpxml/resolve_batteries` (called from `resolve_equipment`) — no zone_id injection.

**Root Cause**: Battery handles the zone_id correctly at the equipment level (optional zone, guarded thermal output), but the HPXML → config bridge never provides `zone_id` for batteries. Indoor batteries (garage, basement, conditioned space) must use config overrides or synthetic TOML to set this.

**Impact**: Indoor-installed batteries (e.g., Powerwall in a garage) cannot route ohmic thermal losses to their location when parsed from HPXML alone. This is a moderate concern because battery ohmic losses are typically small (~1–5% of throughput), but omission still creates a systematic energy balance error.

### Finding 5: [Severity: low]
**Description**: Positive confirmation — no `zone_id=0` leakage exists in the codebase.

**Code Location**:
- `crates/hares-io/src/hpxml/resolve_hvac.rs:1585` — HPXML-derived zone IDs are always ≥ 1: `Some((idx as u16) + 1)`
- `crates/hares-equipment/src/hvac/heat_pump/constants.rs:3` — `pub const DEFAULT_ZONE_ID: u16 = 1;`
- All equipment constructors fall back to `ZoneId(1)`, never `ZoneId(0)`
- `crates/hares-equipment/src/hvac/helpers.rs:30–31` — `validate_u16_id` accepts 0.0 through positive integers, but 0 is never produced by any code path

**Root Cause**: Consistent 1-based zone indexing throughout. Zone 0 is syntactically valid but semantically unused.

**Impact**: No impact — this is a positive finding confirming the absence of the "silent outdoor routing" bug.

### Finding 6: [Severity: low]
**Description**: PV equipment correctly has `zone: None` and emits only electrical contributions.

**Code Location**:
- `crates/hares-equipment/src/pv/mod.rs:145` — `zone: None` in descriptor
- PV only emits `PortContribution::Electrical`; no thermal port

**Root Cause**: Design intent — PV panels are outdoor equipment.

**Impact**: Correct behavior. No change needed.

### Finding 7: [Severity: info]
**Description**: Multi-zone duct distribution routing works correctly once zone_ids are set.

**Code Location**:
- `crates/hares-equipment/src/hvac/duct_distribution.rs:22–70` — `update_zone_heat_fractions()` correctly partitions gross capacity:
  - Conditioned zone: `dse * (1 - basement_frac)`
  - Basement zone: `dse * basement_frac`
  - Duct zone: `1 - dse`
- `crates/hares-equipment/src/hvac/duct_distribution.rs:94–131` — `write_zone_thermal_contributions()` uses correct zone_id and category per entry
- `crates/hares-io/src/hpxml/resolve_hvac.rs:102–145` — `DuctDseParams::insert_into_map()` properly injects `duct_zone_id` into params
- Deduplication (lines 74–88) correctly merges entries when `basement_zone == duct_zone`

**Root Cause**: The duct distribution sub-chain (DuctDseParams → params → typed config → HvacConfig.duct_zone_id → zone_heat_fractions) is complete and correct.

**Impact**: Once the primary `zone_id` propagation gap (Finding 1) is fixed, the multi-zone routing will work correctly for ducted systems. The duct loss routing to attic/garage/basement zones is already functioning.

### Finding 8: [Severity: info]
**Description**: Zone topology is immutable at construction time; equipment zone references are fixed after `init()`.

**Code Location**:
- Zones are created from HPXML `Building::zones` at construction time (`from_preparsed`, `mod.rs:971–976`)
- Zone IDs are assigned as `index + 1` (1-based)
- Equipment `descriptor.zone` and `hvac.config.zone_id` are set during `new()`/`init()` and never mutated during `step()`
- No API exists to add/remove zones at runtime

**Root Cause**: Architectural constraint — zone topology is fixed.

**Impact**: No runtime zone topology mutation risk. Finding not actionable.

## Summary
- **Total findings**: 8
- **Critical**: 1 (HVAC zone_id never propagated from HPXML)
- **High**: 2 (Water heater zone_id hardcoded; fallback masks parsing gap)
- **Medium**: 1 (Battery zone_id not populated from HPXML)
- **Low**: 2 (No zone_id=0 leakage; PV zone=None is correct)
- **Info**: 2 (Duct multi-zone routing correct; zone topology immutable)

## Recommendations

1. **Fix the HPXML → config zone_id injection** (`resolve_hvac.rs`): Call `conditioned_zone_id(building)` for every HVAC system that serves a conditioned zone and inject the result as `"zone_id"` into the params map before typed config construction. This is a one-line addition after `duct_params.insert_into_map(&mut params)` (line 1684) or before each `try_build_*` call.

   Change from:
   ```rust
   duct_params.insert_into_map(&mut params);
   // ... basement params ...
   // zone_id only for Electric Baseboard
   if name == "Electric Baseboard" {
       if let Some(zone_id) = conditioned_zone_id(building) {
           params.insert("zone_id".to_string(), json!(zone_id));
       }
   }
   ```
   To:
   ```rust
   duct_params.insert_into_map(&mut params);
   // ... basement params ...
   // Inject zone_id for ALL equipment serving conditioned space
   if let Some(zone_id) = conditioned_zone_id(building) {
       params.insert("zone_id".to_string(), json!(zone_id));
   }
   ```

2. **Fix water heater zone_id resolution** (`resolve_water_heater.rs`): Resolve the parsed `Location` string to a numeric `ZoneId` by matching against `building.zones[].zone_type` and inject the result before typed config construction. Add a helper `fn zone_id_for_location(building: &Building, location: &str) -> Option<u16>` that iterates `building.zones` and returns `(index + 1)` for the matching zone type.

3. **Replace `unwrap_or(ZoneId(1))` with explicit validation or None**: Consider replacing the hardcoded fallback with a validation that emits a warning when `zone_id` is missing from config, or require it for HVAC/water heater equipment. This would make the HPXML parsing gap immediately visible rather than silently producing wrong results. A phased approach: (a) add a `tracing::warn!()` when falling back to `ZoneId(1)`, (b) after HPXML parsing is fixed, change the fallback to return an `Err`.

4. **Add HPXML zone_id injection for batteries** (`resolve_batteries`): When a battery's HPXML `<Location>` element indicates indoor installation, resolve the location to a `zone_id` and inject it into the config, enabling correct thermal loss routing for indoor batteries.

## References / Citations
- HPXML Data Dictionary v4.2: `<Location>` element on equipment specifies the thermal zone
- ASHRAE 152-2017: Duct distribution system efficiency (DSE) for multi-zone routing (`duct_distribution.rs:22–70`)
- ASHRAE HoF 2021 Ch. 18.31: F-factor method for slab-on-grade (`conversions.rs:166–209`)
- EnergyPlus I/O Reference: `ZoneCapacitanceMultiplier` and `InternalMass` mutual exclusion (`conversions.rs:49–63`)
