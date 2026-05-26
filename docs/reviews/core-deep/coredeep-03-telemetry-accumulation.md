# Telemetry accumulation: CoreOutput and CoreState aggregation, fuel_type indexing consistency
**Review ID**: coredeep-03
**Category**: core-deep
**Date**: 2026-05-26

## Files Reviewed
crates/hares-core/src/telemetry.rs

## Vendor/Reference Files Consulted
None

## Findings
### Finding 1: [Severity: medium]
**Description**: Observer capture functions silently drop Wood, Coal, and WoodPellet fuel contributions. The `capture_ports` function in `observer_capture.rs:137-142` only snapshots fuel types `[Electric, Gas, Propane, Oil]`, omitting `Wood`, `Coal`, and `WoodPellet`. Similarly, `diff_ports` at line 77 only diffs `[Gas, Propane, Oil]`. If any equipment contributes fuel of type Wood, Coal, or WoodPellet via `PortContribution::Fuel`, those contributions are absent from `EquipmentPhaseCapture` observer snapshots with no error or warning. Equipment such as boiler (`boiler.rs:535`), furnace (`furnace.rs:490`), and generator (`generator.rs:786`) write `PortContribution::Fuel` with a config-driven `fuel_type`, and `parse_fuel_type` (`helpers.rs:97-111`) already supports Wood, WoodPellet, and Coal variants. The `FuelAccumulator` correctly accepts these types (`ports.rs:276-286`), but the observer layer never reads them back.

**Code Location**: `crates/hares-core/src/observer_capture.rs:77` (`diff_ports` fuel_types array) and `crates/hares-core/src/observer_capture.rs:137-142` (`capture_ports` fuel_types array)
**Root Cause**: Hardcoded fuel-type arrays in `diff_ports` and `capture_ports` were not updated when the `FuelType` enum was expanded to include Wood, Coal, and WoodPellet.
**Impact**: Fuel consumption data for Wood/Coal/WoodPellet-burning equipment is silently absent from observer telemetry snapshots. The data is correctly accumulated in `PortSlots.fuel` via `FuelAccumulator::add`, but the export path discards it. Since config-driven equipment supports these fuel types, a user specifying a wood-burning boiler would see correct gas/electric metrics but missing wood fuel consumption in any observer/debug output.

### Finding 2: [Severity: medium]
**Description**: `DwellingTelemetry` in `telemetry.rs` lacks indoor humidity and fuel consumption telemetry fields. The `DwellingTelemetry` struct exposes `outdoor_rh` (line 25) but has no indoor humidity field and no fuel consumption channel. Indoor humidity ratio is available in `latest_env.zones.humidity_ratio` and in the humidity solver, but it is not forwarded to the dwelling-level telemetry payload used by control/RL integrations. Similarly, per-equipment or aggregate fuel consumption (gas, propane, oil, etc.) has no representation in `DwellingTelemetry`, despite being computed every step (e.g., `StepResult.gas_power_w` at `dwelling/mod.rs:316-317` and the `PortSlots.fuel` accumulator at `dwelling/mod.rs:2725`). Control policies and RL agents that need to track fuel consumption or indoor humidity cannot access these quantities through the telemetry interface.

**Code Location**: `crates/hares-core/src/telemetry.rs:11-28` (struct definition) and `crates/hares-core/src/dwelling/mod.rs:1850-1939` (construction)
**Root Cause**: `DwellingTelemetry` was designed primarily for electrical and thermal observability, without extending coverage to fuel and humidity domains.
**Impact**: Control policies and RL agents cannot observe indoor humidity conditions or fuel consumption rates. This limits the ability of demand-response or fuel-cost-optimization strategies that need to reason about condensation risk (humidity) or fuel-switching decisions (gas vs. electric heating).

### Finding 3: [Severity: medium]
**Description**: `DwellingTelemetry::outdoor_rh` field name is misleading — it is populated with outdoor humidity ratio, not relative humidity. The field is declared as `pub outdoor_rh: f64` (line 25), implying "relative humidity" (dimensionless fraction or percentage), but is assigned `self.latest_env.weather.outdoor_humidity_ratio` (`dwelling/mod.rs:1936`), which carries the humidity ratio (kg water / kg dry air). The output layer (`record_step`) has a column `"Outdoor Relative Humidity (0-1)"` (`dwelling/mod.rs` around line 4013) that similarly populates from `outdoor_humidity_ratio`. The two quantities differ significantly: a humidity ratio of 0.010 kg/kg at 10 °C corresponds to roughly 73% RH, and at 30 °C it's about 37% RH. Consumer code reading `outdoor_rh` expecting a 0.0–1.0 fraction would misinterpret the values.

**Code Location**: `crates/hares-core/src/telemetry.rs:25` (field declaration) and `crates/hares-core/src/dwelling/mod.rs:1936` (assignment)
**Root Cause**: The naming convention conflates humidity ratio (mass-based mixing ratio) with relative humidity (partial-pressure-based saturation fraction). The weather model stores humidity ratio, but the exposed field is named `rh`.
**Impact**: Any RL agent or control policy using `outdoor_rh` in its observation vector would receive a physically incorrect value — humidity ratio instead of relative humidity — potentially causing incorrect control decisions involving moisture or condensation management.

### Finding 4: [Severity: low]
**Description**: `FUEL_TYPE_COUNT` constant (7) in `ports.rs:274` is manually maintained and not validated against the `fuel_index` match arms. Adding a new `FuelType` variant triggers a compile error in the exhaustive `match` inside `fuel_index` (`ports.rs:276-286`), which is good. However, if the developer maps the new variant to index 7 while `FUEL_TYPE_COUNT` remains 7 (array size 7, indices 0-6), the `FuelAccumulator` array `totals: [f64; FUEL_TYPE_COUNT]` (`ports.rs:295`) silently compiles and an index-out-of-bounds panic occurs at runtime inside `FuelAccumulator::add` (`ports.rs:306`) when the new fuel type is written. There is no compile-time assertion or test linking the match-arm count to the constant.

**Code Location**: `crates/hares-types/src/ports.rs:274` (FUEL_TYPE_COUNT) and `crates/hares-types/src/ports.rs:295` (array declaration)
**Root Cause**: Manual maintenance of the constant without a compile-time assertion or const-generic derivation from the match-arm count.
**Impact**: Low risk in practice (all current 7 fuel types fit within the 7-element array), but a latent footgun for anyone adding an 8th fuel type. A `const _: () = assert!(FUEL_TYPE_COUNT == ...)` or a test that maps each `FuelType` variant through `fuel_index` and asserts all indices < `FUEL_TYPE_COUNT` would remove this risk.

### Finding 5: [Severity: low]
**Description**: `diff_ports` fuel types array `[Gas, Propane, Oil]` (`observer_capture.rs:77`) omits `FuelType::Electric` while `capture_ports` includes it (`observer_capture.rs:137-138`). No equipment writes `PortContribution::Fuel { fuel_type: FuelType::Electric, .. }` (electrical contributions use `PortContribution::Electrical`), so this asymmetry is functionally harmless. However, the `FuelAccumulator` does contain a slot for `FuelType::Electric` (index 0 in `fuel_index`), and `capture_ports` reads it. A maintainer or future equipment implementation could be confused by the inconsistency.

**Code Location**: `crates/hares-core/src/observer_capture.rs:77` vs `crates/hares-core/src/observer_capture.rs:137-142`
**Root Cause**: Intentional exclusion of Electric from fuel diffs (electric power is diffed separately in the electrical accumulator), but the arrays were defined independently rather than derived from a shared constant or `FuelType` iteration, allowing drift.
**Impact**: No current data loss, but increases maintenance burden and risk of future bugs if a developer copies one fuel array without realizing the other differs.

### Finding 6: [Severity: low]
**Description**: `validate_core_contract` in `equipment.rs:943-1061` validates `thermal_output_w` under the `THERMAL` capability bit, but does not validate `sensible_cooling_w` or `latent_cooling_w` from `CoreFlows`. Cooling equipment that populates `sensible_cooling_w` and `latent_cooling_w` (e.g., the heat pump cooler at `cooler.rs`) goes through validation for `thermal_output_w` but not for the cooling-specific fields. If a cooling-capable equipment sets `sensible_cooling_w` to `None` while `thermal_output_w` is `Some(negative)`, the contract check does not catch the inconsistency.

**Code Location**: `crates/hares-types/src/equipment.rs:943-1061`
**Root Cause**: `CoreCapabilities` has a single `THERMAL` bit that unifies heating and cooling; there is no separate `COOLING` capability bit to gate the cooling-specific flow fields.
**Impact**: A buggy equipment implementation could fail to populate cooling metrics while still passing contract validation, leading to silently zeroed cooling data in output columns that read `sensible_cooling_w` and `latent_cooling_w`.

## Summary
- Total findings: 6
- Medium: 3 (Findings 1, 2, 3)
- Low: 3 (Findings 4, 5, 6)
- Critical / High: 0

## Recommendations
1. Replace the hardcoded fuel-type arrays in `capture_ports` and `diff_ports` with one that includes all non-None `FuelType` variants (Electric, Gas, Propane, Oil, Wood, Coal, WoodPellet). Consider generating the array from a const slice defined alongside `FuelType` to prevent future drift.
2. Add `indoor_humidity_ratio` and `fuel_consumption_w` (per-type or aggregate) fields to `DwellingTelemetry` and populate them in `Dwelling::telemetry()`.
3. Rename `outdoor_rh` to `outdoor_humidity_ratio` or compute actual relative humidity from the humidity ratio and outdoor temperature before assigning to the field, so the name matches the semantics.
4. Add a compile-time assertion or a test that verifies all `FuelType` variants (except `None`) have indices within `[0, FUEL_TYPE_COUNT)`, so a new fuel type that exceeds the array size is caught before runtime.
5. Unify the fuel-type arrays used by `capture_ports` and `diff_ports` (or document the asymmetry clearly) to reduce maintenance confusion.
6. Consider adding a `COOLING` capability bit to `CoreCapabilities` to separately validate `sensible_cooling_w` and `latent_cooling_w`, or fold them into the existing `THERMAL` validation.

## References / Citations
- `crates/hares-core/src/telemetry.rs` — `DwellingTelemetry` struct and `to_observation_vec` method
- `crates/hares-core/src/observer_capture.rs:77,137-142` — fuel type arrays in `diff_ports` and `capture_ports`
- `crates/hares-types/src/ports.rs:274-317` — `FUEL_TYPE_COUNT`, `fuel_index`, `FuelAccumulator`
- `crates/hares-types/src/equipment.rs:147-157` — `FuelType` enum definition
- `crates/hares-types/src/equipment.rs:905-936` — `CoreState` and `CoreOutput` struct definitions
- `crates/hares-types/src/equipment.rs:943-1061` — `validate_core_contract` completeness
- `crates/hares-core/src/dwelling/mod.rs:1850-1939` — `Dwelling::telemetry()` construction
- `crates/hares-core/src/dwelling/mod.rs:2808-2810` — `ports.zero()` after `record_step` (timestep reset)
- `crates/hares-equipment/src/hvac/helpers.rs:97-111` — `parse_fuel_type` supporting all fuel types
