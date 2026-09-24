# OCHRE compat mapping coarse and ambiguous
**Review ID**: types-physics-06
**Category**: types-physics
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-control/src/compat.rs` (primary target)
- `crates/hares-control/src/types.rs`
- `crates/hares-control/src/signal.rs`
- `crates/hares-equipment/src/registry.rs`
- `crates/hares-io/src/schedule_resolve.rs`

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/utils/hpxml.py`
- `vendors/OCHRE/ochre/Equipment/Battery.py`
- `vendors/OCHRE/ochre/Equipment/Generator.py`
- `vendors/OCHRE/ochre/Equipment/PV.py`

## Findings

### Finding 1: [Severity: high] HVAC setpoint routing uses fragile substring heuristic

**Description**: The `ochre_signal_to_control` function decides whether a thermal setpoint maps to `heating_setpoint_c` or `cooling_setpoint_c` by checking if `equipment_type.to_ascii_lowercase().contains("cool")`. This is a coarse substring match with zero semantic understanding.

**Code Location**: `crates/hares-control/src/compat.rs:55-60`
```rust
let equipment = equipment_type.to_ascii_lowercase();
let (heat, cool) = if equipment.contains("cool") {
    (None, Some(*value))
} else {
    (Some(*value), None)
};
```

**Root Cause**: There is no equipment type enumeration or canonical tag to drive routing. The function relies solely on the presence/absence of the four-character substring "cool" in whatever string the caller passes.

**Impact**:
- **False-positive cooling**: Equipment named "Coolant Loop Heater", "Radiant Cooled Floor", or "Passive Evaporative Cooler" would route a temperature setpoint to `cooling_setpoint_c` instead of `heating_setpoint_c`. If the OCHRE-style schedule column `"Water Heating"` were passed but a water heater class contained "cool" in its name, the setpoint would be routed incorrectly.
- **False-negative cooling**: Equipment that provides only cooling but does not contain "cool" in its name string (e.g., "Room AC", "Dehumidifier", a hypothetical "Chiller") is routed to heating. The setpoint is effectively **discarded** because it lands in the wrong field.
- **Dual-mode ambiguity**: Equipment capable of both heating and cooling (heat pumps) can only receive a setpoint in one field per call. If a "Heat Pump" string is passed (without "Cool"), only `heating_setpoint_c` is set. The caller must know to pass two separate equipment_type strings (e.g., "ASHP Heater" and "ASHP Cooler") to receive both setpoints.
- **Namespace mismatch**: The `equipment_type` parameter has no defined namespace. The OCHRE schedule-column namespace uses names like `"HVAC Heating"`/`"HVAC Cooling"`, while the HARES equipment descriptor namespace uses `"Gas Furnace"`/`"Air Conditioner"`. A caller could pass either, and the heuristic gives different (possibly wrong) results depending on which namespace is used. The function provides no guidance and makes no distinction.

**Severity rationale**: Silent misrouting of thermal setpoints can disable heating or cooling equipment under dispatch control, causing comfort violations and potential freeze/overheat damage. The function has no validation that the equipment_type parameter comes from a recognized namespace.

### Finding 2: [Severity: high] Only 6 of 25 ControlSignal variants have OCHRE key mappings

**Description**: The `ochre_signal_to_control` function maps exactly 6 OCHRE-style string keys to 6 ControlSignal variants, but the `ControlSignal` enum has 25 variants. The remaining 19 variants cannot be constructed from OCHRE input at all.

**Code Location**: `crates/hares-control/src/compat.rs:52-85` (the match block)

**Mapped variants** (6):
| OCHRE Key | HARES ControlSignal |
|---|---|
| `"Setpoint Temperature (C)"` | `ThermalSetpoint` |
| `"P Setpoint"` | `PowerSetpoint` |
| `"Duty Cycle"` | `DutyCycle` |
| `"Load Fraction"` | `LoadFraction` |
| `"SOC"` / `"Min SOC"` / `"Max SOC"` | `SOCTarget` |
| `"Self Consumption Mode"` | `SelfConsumption` |

**Unmapped variants** (19):
`PowerLimit`, `ModeOverride`, `GridConnect`, `DemandResponse`, `ProtocolNative`, `CurtailmentPercent`, `ReactiveSetpoint`, `PowerFactorSetpoint`, `InverterPriorityMode`, `IdealCapacity`, `ThermalSetpointDelta`, `IdealCapacityModeOverride`, `EvPlugIn`, `EvDrive`, `EvAwayCharge`, `EvSetReadyBy`, `EventDelay`, `MaxCapacityFraction`, `HumiditySetpoint`

**Root Cause**: The compat module was built to support an initial subset of OCHRE controls that matched a narrow early test suite (HVAC + battery), and was never backfilled when the full ControlSignal enum was populated.

**Impact**: 
- Dispatch-capable equipment types register capabilities (e.g., PV advertises `POWER_FACTOR_SETPOINT | CURTAILLMENT_PERCENT | REACTIVE_SETPOINT`), but external controllers cannot actually issue those signal types through the OCHRE compat path.
- The OCHRE PV model in the vendor reference supports `"P Curtailment (kW)"`, `"P Curtailment (%)"`, `"Q Setpoint"`, `"Power Factor"`, and `"Priority"` keys (`vendors/OCHRE/ochre/Equipment/PV.py:174-203`). None have HARES compat mappings. Dispatchers controlling PV through the OCHRE compat layer can only set `"P Setpoint"` (which maps to `PowerSetpoint`), losing curtailable and reactive dispatch capability.
- The OCHRE `Generator.update_external_control` supports `"Max Import Limit"` and `"Max Export Limit"` keys (`vendors/OCHRE/ochre/Equipment/Generator.py:78-90`) — no HARES mapping exists.
- Self-consumption mode mapping (`compat.rs:72-73`) discards the `solar_only_charging` parameter, always passing `false`. OCHRE Battery supports this via `charge_solar_only` (`vendors/OCHRE/ochre/Equipment/Battery.py:233`).

### Finding 3: [Severity: medium] No production callers — unmasked silent failure risk

**Description**: `ochre_signal_to_control` is publicly exported and documented in architecture diagrams as "OCHRE Compat Layer", but a codebase-wide search found zero production callers. All invocations are in the test suite of the compat module itself.

**Code Location**: 
- Definition: `crates/hares-control/src/compat.rs:24`
- Public re-export: `crates/hares-control/src/lib.rs:10`
- Test-only callers: `crates/hares-control/src/compat.rs:116,165,184,186,207,228,248,266,284`

**Root Cause**: The compat layer was built as a forward-looking integration point but never wired into the actual dispatch pipeline. Control signals from OCHRE-style external controllers flow through other (unmapped) paths.

**Impact**: The gaps identified in Findings 1 and 2 are latent. When the function is eventually wired into production, every mapping gap simultaneously activates — thermal setpoints may be routed to the wrong equipment, 19 control signal types will be silently unreachable, and equipment with `ControlCapabilities` bits that have no mapping (e.g., PV reactive power, EV plug-in control) will never receive dispatches through this path. The lack of integration tests with real OCHRE controllers means the scope of breakage is unknown.

### Finding 4: [Severity: medium] OCHRE key matching is exact with no normalization

**Description**: The OCHRE control signal keys are matched via `key.as_str()` (line 53) with exact string equality against the constant key strings. There is no whitespace trimming, no case folding, and no fallback to fuzzy matching.

**Code Location**: `crates/hares-control/src/compat.rs:53`
```rust
match key.as_str() {
    KEY_SETPOINT_TEMPERATURE_C => { ... }
    KEY_POWER_SETPOINT_KW => { ... }
    ...
}
```

**Root Cause**: The matching mirrors OCHRE Python's pattern (`control_signal.get("SOC")`, exact dict key lookups), which is correct when the same system generates and consumes the keys. However, in a cross-system compat layer, there is no guarantee that external OCHRE controllers use the exact string constants HARES defines.

**Impact**: 
- A controller that sends `"setpoint temperature (c)"` (lowercase), `"P SETPOINT"` (uppercase), or `"Setpoint Temperature (C) "` (trailing whitespace) would trigger the `tracing::warn!` "unrecognized key" path and have its signal silently dropped.
- Legacy or third-party OCHRE controllers may use deprecated key names that are not handled (no alias resolution exists).
- The OCHRE test suite uses the exact keys HARES expects (`"Setpoint Temperature (C)"` at `OCHRE/ochre/utils/hpxml.py:1155`, `"Load Fraction"` at `OCHRE/test/test_equipment/test_scheduled.py:49`), but this is evidence of the OCHRE project's own conventions, not a formal specification.

### Finding 5: [Severity: medium] Phantom equipment types in heating equipment list

**Description**: The `HEATING_EQUIPMENT` constant in `schedule_resolve.rs` lists `"Oil Furnace"` and `"Oil Boiler"` as equipment that should receive heating setpoint schedule sources, but neither type is registered in the `CANONICAL_EQUIPMENT_NAMES` list or the `EquipmentRegistry`.

**Code Location**: 
- Registration list: `crates/hares-io/src/schedule_resolve.rs:415-425`
- `HEATING_EQUIPMENT` includes `"Oil Furnace"` (line 420) and `"Oil Boiler"` (line 424)
- `CANONICAL_EQUIPMENT_NAMES`: `crates/hares-equipment/src/registry.rs:17-88` — no "Oil Furnace" or "Oil Boiler"
- Registry `register()` calls on the same file — no oil equipment registrations exist

**Root Cause**: The heating equipment list was likely copied from an OCHRE reference (ResStock includes oil furnaces and boilers as a common equipment type) but the corresponding HARES equipment implementations were never created or registered.

**Impact**: Since these names never match actual equipment specs, no functional misbehavior occurs currently. However, if oil furnaces or boilers are later implemented and registered, the schedule resolver will silently fail to inject setpoint column references unless the names match exactly. This is a maintainability hazard — the phantom entries give false confidence that the equipment list is complete.

### Finding 6: [Severity: low] Water heater setpoint CSV column is defined but unhandled

**Description**: The `COLUMN_MAPPINGS` table maps `water_heater_setpoint` to the OCHRE name `"Water Heating"` with category `ScheduleCategory::Setpoint`, but `inject_water_heater_schedule_columns` (called from `inject_setpoint_schedules`) only processes `hot_water_fixtures` and `hot_water_mains_temperature` columns. The `water_heater_setpoint` column is never consumed.

**Code Location**:
- Column mapping: `crates/hares-io/src/schedule_resolve.rs:172-176`
- Handler: `crates/hares-io/src/schedule_resolve.rs:509-556` (only `hot_water_fixtures` and `hot_water_mains_temperature` are read)
- No code path references `water_heater_setpoint` after the mapping is defined.

**Root Cause**: The column mapping was defined for completeness but the handler code only processes draw flow rate and mains temperature for water heaters. The setpoint column was left unimplemented.

**Impact**: Water heater temperature setpoints from OCHRE-style schedule CSV files are silently ignored. Water heaters rely entirely on their configured static setpoint rather than schedule-driven temperature targets. This is a functional gap for demand response scenarios where water heater temperature setbacks are a primary control strategy.

### Finding 7: [Severity: low] Self-consumption solar-only flag always false

**Description**: The `SelfConsumption` signal constructor in the OCHRE compat path always passes `solar_only_charging: false`, discarding any ability to express solar-only charging through the OCHRE compat layer.

**Code Location**: `crates/hares-control/src/compat.rs:73`
```rust
KEY_SELF_CONSUMPTION_MODE => {
    out.push(ControlSignal::self_consumption(*value == 1.0, false));
}
```

**Root Cause**: The OCHRE `"Self Consumption Mode"` key is defined as a boolean (line 75 of `vendors/OCHRE/ochre/Equipment/Generator.py`: `self.self_consumption_mode = bool(control_signal["Self Consumption Mode"])`), and OCHRE tracks solar-only charging via a separate `charge_solar_only` attribute rather than a control signal key. HARES's `SelfConsumption` variant bundles `enabled` and `solar_only_charging` into a single signal, but the OCHRE mapping has no key to populate `solar_only_charging`.

**Impact**: Any dispatch scenario that relies on solar-only charging semantic cannot be expressed through the OCHRE compat path. The HARES `SelfConsumption` struct supports the field, but no OCHRE key exists to populate it.

## Summary
- Total findings: 7
- Critical: 0
- High: 2 (Findings 1, 2)
- Medium: 3 (Findings 3, 4, 5)
- Low: 2 (Findings 6, 7)

## Recommendations

1. **Replace the substring heuristic with explicit equipment-type enumeration** (`compat.rs:55-60`). Define a `ThermalRouting` enum with `Heating`, `Cooling`, `Both`, `Unknown` and resolve it via a mapping table keyed on canonical equipment type strings. This eliminates ambiguity and makes incorrect routing detectable at compile time.

2. **Backfill OCHRE key mappings for all dispatch-capable ControlSignal variants**, prioritizing signal types that map to existing OCHRE controller keys: `"P Curtailment (kW)"` / `"P Curtailment (%)"` → `CurtailmentPercent`, `"Q Setpoint"` → `ReactiveSetpoint`, `"Power Factor"` → `PowerFactorSetpoint`, `"Priority"` → `InverterPriorityMode`, `"Max Import Limit"` / `"Max Export Limit"` → `PowerLimit`, and EV-specific control keys.

3. **Wire `ochre_signal_to_control` into the production dispatch pipeline** with integration tests against OCHRE-style controller output. Until this is done, the compat mapping correctness cannot be validated end-to-end.

4. **Add key normalization** (trim whitespace, case-fold) with a warning when non-exact matches are encountered. Maintain a deprecated-key aliases table for backward compatibility with older OCHRE controller versions.

5. **Remove phantom `"Oil Furnace"` / `"Oil Boiler"` from `HEATING_EQUIPMENT`** or add `register_error()` entries for them so that attempts to construct these types produce informative errors rather than silent no-ops.

6. **Implement water heater setpoint column handling** in `inject_water_heater_schedule_columns` so that the `water_heater_setpoint` CSV column is consumed and routed to water heater equipment.

7. **Add an OCHRE key for solar-only charging** (e.g., `"Solar Only Charging"`), or split the OCHRE `"Self Consumption Mode"` key into two separate signals, so that the `SelfConsumption.solar_only_charging` field can be populated.

## References / Citations
- OCHRE Battery control signal keys: `vendors/OCHRE/ochre/Equipment/Battery.py:170-196`
- OCHRE Generator `update_external_control`: `vendors/OCHRE/ochre/Equipment/Generator.py:67-100`
- OCHRE PV control signal keys: `vendors/OCHRE/ochre/Equipment/PV.py:164-205`
- OCHRE test battery: `vendors/OCHRE/test/test_equipment/test_waterheater.py:78-140` (demonstrates OCHRE key convention matching HARES compat.rs constants)
- HARES registry canonical names: `crates/hares-equipment/src/registry.rs:17-88`
- HARES schedule column mappings: `crates/hares-io/src/schedule_resolve.rs:44-228`
- HARES heating/cooling equipment lists: `crates/hares-io/src/schedule_resolve.rs:415-428`
- Prior review noting the same fragility: `docs/reviews/control-deep/ctrldp-04-control-types-module-completeness.md:55` (the substring heuristic was flagged in a prior deep-dive review)
