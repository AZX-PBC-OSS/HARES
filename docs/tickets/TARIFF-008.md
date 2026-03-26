---
id: TARIFF-008
title: Attach BmsMode to battery config and ChargingStrategy extensions to EV config
kind: implement
depends_on:
  - TARIFF-002
  - TARIFF-003
files_to_touch:
  - crates/hares-equipment/src/battery/mod.rs
  - crates/hares-equipment/src/ev/mod.rs
  - crates/hares-equipment/src/ev/config.rs
references:
  - docs/tickets/TARIFF-INDEX.md
  - docs/tickets/TARIFF-002.md
  - docs/tickets/TARIFF-003.md
verification:
  - cargo build --workspace
  - cargo test --workspace
  - cargo clippy --workspace
---

## Background/Context

The `BmsMode` and extended `ChargingStrategy` types are pure data — they tell actors how to control equipment, but equipment physics does not read them. This ticket wires the new types into the existing battery and EV configuration paths so they can be set from Python overrides and config JSON.

The key constraint is that default values (`BmsMode::Manual`, `ChargingStrategy::Immediate`) must preserve existing behavior exactly — no actor logic is triggered for these defaults, maintaining backward compatibility.

## Work to Do

- [ ] Add `bms_mode` config key to battery configuration in `crates/hares-equipment/src/battery/mod.rs`:
  - Parse `"bms_mode"` from `EquipmentConfig` as a JSON blob in a single key: `config.get_str("bms_mode")` → `serde_json::from_str::<BmsMode>(value)`. This handles the typed enum ↔ string key-value impedance: the value is a serialized JSON string containing the tagged enum (e.g., `{"SelfConsumption":{"min_soc":0.1,"max_soc":1.0,"solar_only_charging":false}}`)
  - Default to `BmsMode::Manual` if key absent
  - Store on battery state struct (or make accessible via a method on the equipment)
  - Add `grid_export_rule` config key, same JSON string approach, default `GridExportRule::Unrestricted`
- [ ] Expose `bms_mode` and `grid_export_rule` via battery's equipment descriptor or a new accessor method so actors can read them
- [ ] Verify EV already reads `ChargingStrategy` from config — if not, add config key parsing for the new variants (V2H, V2G, SolarSurplus)
- [ ] Ensure `#[serde(default)]` is used on new fields so existing config JSON without these keys still parses
- [ ] Verify existing battery and EV tests pass unchanged

## Files to Touch

- `crates/hares-equipment/src/battery/mod.rs`: Add bms_mode and grid_export_rule config parsing
- `crates/hares-equipment/src/ev/mod.rs`: Verify ChargingStrategy parsing handles new variants
- `crates/hares-equipment/src/ev/config.rs`: Update if EV config struct exists here

## Measures of Success

- [ ] Existing battery config JSON without `bms_mode` deserializes with `BmsMode::Manual`
- [ ] Existing EV config JSON without new strategy fields deserializes with current defaults
- [ ] `bms_mode: SelfConsumption { min_soc: 0.1, max_soc: 1.0, solar_only_charging: false }` in config JSON parses correctly
- [ ] `charging_strategy: V2G { min_soc: 0.3, max_export_kw: 7.2, price_threshold: 0.25 }` in config JSON parses correctly
- [ ] Battery physics behavior unchanged (bms_mode is not read by equipment step())
- [ ] All existing tests pass with no regressions

## Tests Added

**hares-equipment:**
- `battery_default_bms_mode_is_manual` — config without bms_mode → Manual
- `battery_bms_mode_from_config_json` — config with SelfConsumption variant parses
- `battery_grid_export_rule_default` — default is Unrestricted
- `ev_charging_strategy_v2g_from_config` — V2G variant parses from config JSON
- `ev_charging_strategy_backward_compat` — old config without new fields still works

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace` passes
