# Configuration serialization round-trip correctness
**Review ID**: config-io-04
**Category**: config-io
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-io/src/config.rs` — `SimulationConfig`, `OutputFormat`, deserializer helpers
- `crates/hares-io/tests/config_round_trip.rs` — equipment config round-trip tests

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/utils/base.py` — config loading, `nested_update`, `save_json` export
- `vendors/OCHRE/ochre/utils/equipment.py` — equipment property merging
- `vendors/OCHRE/ochre/Dwelling.py` — dwelling init, JSON export
- `vendors/OCHRE/ochre/utils/hpxml.py` — HPXML parsing and defaults
- `vendors/OCHRE/ochre/utils/units.py` — unit conversion layer

## Findings

### Finding 1: [Severity: critical]
**Description**: `SimulationConfig` does not derive `Serialize`. It is the primary configuration struct for controlling temporal loop, output, and RNG seeding (doc: "single source of truth for temporal settings"). It only derives `Deserialize` and therefore cannot be serialized back to TOML for round-trip verification, programmatic config generation, or config migration.

**Code Location**: `crates/hares-io/src/config.rs:34-35`
```rust
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct SimulationConfig { ... }
```
Note the absence of `Serialize` in the derive macro. The `use serde::...` import at line 10 imports only `Deserialize, Deserializer`, confirming this was an intentional omission, not an oversight in the imports.

**Root Cause**: The struct was designed for one-directional TOML consumption (`from_toml()`) only. There is no `to_toml()` method and no serialization path. Callers that need TOML output (regression tests at `crates/hares-core/tests/core_output_regressions.rs:34`, `named_control_wiring_regressions.rs:21`, `alignment_oracles_regressions.rs:1048`) work around this by extracting the `[simulation]` section as a generic `toml::Table` and calling `toml::to_string(&sim_table)` on that — bypassing the struct entirely.

**Impact**:
- No automated round-trip test is possible for `SimulationConfig` (versus equipment configs which all support `Serialize + Deserialize`).
- Any tool that generates config files programmatically (e.g., a UI, a migration tool, a schema diff) must construct TOML by hand rather than using serde.
- Breaking changes in field serialization go undetected since `Serialize` is never derived and thus never exercised.
- Contrasts with OCHRE, which also lacks a formal round-trip path but uses Python dicts where serialization to JSON is trivial (`save_json()`). OCHRE at least can re-export its in-memory config to a machine-readable format for inspection.

---

### Finding 2: [Severity: critical]
**Description**: `OutputFormat` enum does not derive `Serialize`. It is the `output_format` field of `SimulationConfig` and has `#[serde(rename_all = "lowercase")]` for stable deserialization, but cannot be re-serialized.

**Code Location**: `crates/hares-io/src/config.rs:14-22`
```rust
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    #[default]
    Csv,
    Parquet,
}
```

**Root Cause**: Same as Finding 1 — serialization was never needed for the original TOML-input-only design, so `Serialize` was omitted. The `rename_all` attribute already sets the correct wire format for serialization (it applies to both directions), so the fix is a one-word addition to the derive macro.

**Impact**: Even if `SimulationConfig` gained `Serialize`, the `OutputFormat` field would still be opaque to serde because the enum itself doesn't implement `Serialize`. Any downstream re-serialization of a `SimulationConfig` (e.g., through a `toml::Value` round-trip) would fail to encode `output_format`.

---

### Finding 3: [Severity: high]
**Description**: Round-trip tests in `config_round_trip.rs` use `serde_json` (JSON), not `toml` (TOML). The actual production config format is TOML. JSON and TOML have different type semantics — most critically, TOML has native date-time types while JSON serializes them as strings, and TOML's table/array-of-tables structure differs from JSON's object/array structure. A JSON round-trip passing does not guarantee a TOML round-trip would pass for the same structs.

**Code Location**: `crates/hares-io/tests/config_round_trip.rs:613-619`
```rust
fn assert_round_trip<T>(cfg: T)
where
    T: EquipmentTypedConfig + PartialEq + Clone + std::fmt::Debug,
{
    let serialized = serde_json::to_value(&cfg).unwrap();
    let deserialized: T = serde_json::from_value(serialized).unwrap();
    assert_eq!(cfg, deserialized);
}
```
All 22 equipment config round-trip tests use this function, which round-trips through `serde_json::Value`, not `toml::Value`. The `assert_deny_unknown` helper at line 622 similarly uses JSON.

**Root Cause**: The `EquipmentTypedConfig` trait requires `Serialize + for<'de> Deserialize<'de>` (at `crates/hares-equipment/src/config.rs:106`), which enables JSON round-trips but doesn't specify the format. JSON was likely chosen because `serde_json::Value` is more ergonomic for programmatic manipulation (e.g., injecting `nonexistent_key` for deny-unknown testing). TOML round-trip testing would require a different helper that round-trips through `toml::to_string` / `toml::from_str`.

**Impact**:
- TOML-specific issues (e.g., date-time serialization format, enum table vs. string representation, float precision) are never tested.
- If an upstream dependency (chrono, toml crate) changes how a type serializes to TOML but is consistent across JSON, the test would pass while the production path breaks.
- OCHRE also uses JSON for its one-way export, but HARES should test the actual persistence format.

---

### Finding 4: [Severity: high]
**Description**: No round-trip test exists for `SimulationConfig` — the top-level configuration struct that owns the TOML input path. The `config_round_trip.rs` file exclusively tests equipment configs that implement `EquipmentTypedConfig`. `SimulationConfig` does not implement that trait and has no round-trip test of any kind (JSON or TOML).

**Code Location**: 
- `crates/hares-io/tests/config_round_trip.rs` — covers 19 equipment config types plus `HeatPumpConfig` (manually), zero `SimulationConfig` tests.
- `crates/hares-io/src/config.rs:160-308` — unit tests cover deserialization + validation only (minimal config, full config, error cases, boundary values). No test serializes a `SimulationConfig` and deserializes it back.

**Root Cause**: `SimulationConfig` cannot be tested because it lacks `Serialize` (Finding 1). Even if it gained `Serialize`, it doesn't implement `EquipmentTypedConfig`, so a separate test helper would be needed.

**Impact**:
- Default values, `Option<PathBuf>`, `Option<f64>`, `Option<String>` (the `civil_timezone` field), and the custom `Duration` deserializer are never verified to round-trip.
- The `Default` trait is not implemented on `SimulationConfig`, so the task requirement "Default values from the Default trait implementation must be preserved through the round-trip" cannot be met for the primary config struct.
- The regression tests at `crates/hares-core/tests/` perform an *indirect* round-trip by re-serializing a `toml::Table` and re-parsing, but this does not test `SimulationConfig`'s own serialization behavior.

---

### Finding 5: [Severity: medium]
**Description**: Many configuration enums lack an explicit `#[serde(rename_all = "...")]` attribute, relying on serde's default PascalCase convention. If a variant name is refactored, the serialized representation changes silently, breaking existing config files and stored configuration payloads.

**Code Location**: The following enums in `crates/hares-types/src/equipment.rs` and `crates/hares-types/src/schedule.rs` derive `Serialize + Deserialize` without any `rename_all`:

| Enum | File:Line | Default Style |
|------|-----------|---------------|
| `FuelType` | equipment.rs:146 | PascalCase |
| `OperatingMode` | equipment.rs:174 | PascalCase |
| `ExecutionStage` | equipment.rs:163 | PascalCase |
| `FluidType` | equipment.rs:241 | PascalCase |
| `ChargingLevel` | equipment.rs:303 | PascalCase |
| `VehicleType` | equipment.rs:331 | PascalCase |
| `EvConnectionState` | equipment.rs:359 | PascalCase |
| `GridExportRule` | equipment.rs:579 | PascalCase |
| `IdealCapacityMode` | equipment.rs:134 | PascalCase |
| `BoundaryPolicy` | schedule.rs:162 | PascalCase |
| `DayFilter` | schedule.rs:12 | PascalCase |
| `SeasonFilter` | schedule.rs:861 | PascalCase |
| `BillingCycle` | schedule.rs:947 | PascalCase |

In contrast, three enums do use explicit `rename_all`: `OutputFormat` (`"lowercase"`), `ScheduleSourceConfig` (`"snake_case"`), and `IdealCapacityModeConfig` (`"lowercase"`). The inconsistency suggests a convention was started but not universally applied.

**Root Cause**: The codebase does not enforce a policy on enum serialization stability. The `EquipmentTypedConfig` trait comment at `config.rs:104-105` mentions `#[serde(deny_unknown_fields)]` but says nothing about enum naming stability.

**Impact**: Medium — the existing PascalCase convention is stable *as long as Rust variant names are not refactored*. However, Rust encourages `snake_case` for variant names, so a future refactor from `GridExportRule::Unrestricted` to `GridExportRule::Unrestricted` has no impact, but changing `Unrestricted` to `NoLimits` would silently change the wire format. An explicit `rename_all = "PascalCase"` documents the intent and exists in serde for exactly this purpose. Without it, there's no protective layer between the Rust identifier and the wire format.

---

### Finding 6: [Severity: low]
**Description**: `SimulationConfig` does not implement the `Default` trait, so the review requirement "Default values from the Default trait implementation must be preserved through the round-trip" cannot be verified. The struct has field-level defaults (`#[serde(default = "...")]`) for individual fields, but the struct itself has no `Default` impl.

**Code Location**: `crates/hares-io/src/config.rs:34` — no `Default` in the derive list, no manual `impl Default`.

**Impact**: Low — a `Default` impl could be added to construct a config with all defaults, which would then be useful for round-trip testing (serialize default, deserialize, assert equality). Currently, constructing a "fully default" config requires writing a minimal TOML string with only the required fields (`start_time`, `duration`) and relying on field-level defaults for the rest.

---

### Finding 7: [Severity: low]
**Description**: `EnvironmentState` uses `#[serde(skip_serializing)]` on two runtime fields (`equipment_telemetry`, `equipment_core`), causing data loss on round-trip. If an `EnvironmentState` is serialized and deserialized, these fields come back as empty `HashMap`s via `#[serde(default)]`.

**Code Location**: `crates/hares-types/src/environment.rs:311-326`
```rust
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EnvironmentState {
    // ... serialized fields ...
    #[serde(skip_serializing, default)]
    pub equipment_telemetry: HashMap<String, Telemetry>,
    #[serde(skip_serializing, default)]
    pub equipment_core: HashMap<EquipmentId, CoreOutput>,
}
```

**Impact**: Low — these are per-timestep runtime caches, not persistent configuration. The design is intentional. However, if `EnvironmentState` is ever checkpointed (serialized mid-simulation for restart), telemetry and core data would be lost. This is noted for awareness but not a current bug.

---

### Finding 8: [Severity: low]
**Description**: Heat pump configs (`HeatPumpCommonConfig`, `HeatPumpHeaterConfig`, `HeatPumpCoolerConfig`) cannot reject unknown fields due to `#[serde(flatten)]` incompatibility with `#[serde(deny_unknown_fields)]`. All other equipment configs use `deny_unknown_fields` to catch typo'd field names.

**Code Location**: 
- `crates/hares-equipment/src/hvac/heat_pump_config.rs:23` (`HeatPumpCommonConfig`)
- `crates/hares-equipment/src/hvac/heat_pump_config.rs:179` (`HeatPumpHeaterConfig`)
- `crates/hares-equipment/src/hvac/heat_pump_config.rs:384` (`HeatPumpCoolerConfig`)
- Test confirming the limitation: `config_round_trip.rs:709-717`

**Root Cause**: Known serde limitation (serde-rs/serde#2384) — `flatten` captures all unrecognized keys, making `deny_unknown_fields` effectively impossible.

**Impact**: Low — the codebase documents this with a test at lines 709-717 and has separate tests that verify heater-only fields are ignored by cooler configs. A mistyped field name in a heat pump config will be silently dropped rather than producing a parse error.

---

## Summary
- **Total findings**: 8
- **Critical**: 2 (`SimulationConfig` lacks `Serialize`, `OutputFormat` lacks `Serialize`)
- **High**: 2 (JSON-only round-trip tests, no `SimulationConfig` round-trip test)
- **Medium**: 1 (enums lack explicit `rename_all`)
- **Low**: 3 (no `Default` impl, `EnvironmentState` data loss, heat pump unknown-field tolerance)

### Overall Assessment
The equipment config layer has strong serialization hygiene: all 22 `EquipmentTypedConfig` implementors derive both `Serialize` and `Deserialize`, use `deny_unknown_fields`, and have per-type JSON round-trip tests. The gap is at the simulation configuration layer (`SimulationConfig` / `OutputFormat`), where the TOML input path was designed as a one-way parse-only pipeline. A conversion from `Deserialize`-only to `Serialize + Deserialize` on these two types would enable end-to-end round-trip verification of the top-level config format.

### Comparison to OCHRE
OCHRE has no serialization round-trip concept at all — it uses Python dicts with `json.dump()` for one-way export. HARES's equipment config layer is significantly more rigorous than OCHRE in this regard, with typed structs, `deny_unknown_fields`, and automated round-trip tests. The `SimulationConfig` gap is the sole area where HARES falls short compared to what its own equipment layer already achieves.

## Recommendations
1. **Add `Serialize` to `SimulationConfig` and `OutputFormat`** (`crates/hares-io/src/config.rs:14,34`). Add `Serialize` to both derive macros. `OutputFormat`'s existing `#[serde(rename_all = "lowercase")]` attribute will apply correctly in both directions. For `SimulationConfig`, the custom `deserialize_with` attributes only affect deserialization; serialization of `Duration` fields will use chrono's default serde impl, which serializes to seconds as `i64` — compatible with the `deserialize_duration_seconds` deserializer.

2. **Add a `SimulationConfig` round-trip test** using `toml::to_string` / `toml::from_str` and `from_toml()`. At minimum, test that a fully-specified config and a minimal config with defaults both round-trip correctly. Also implement `Default` for `SimulationConfig` and test that the default value round-trips intact.

3. **Add TOML round-trip tests for all `EquipmentTypedConfig` implementors** alongside the existing JSON round-trip tests. A second helper function `assert_toml_round_trip<T>` would serialize through `toml::to_string` and deserialize through `toml::from_str`. This validates the actual production serialization format.

4. **Apply `#[serde(rename_all = "PascalCase")]` to all config-facing enums** to document that the wire format is explicitly pinned to PascalCase. This makes the intent clear and prevents silent format changes if variant names are ever refactored. Prefer PascalCase (not snake_case) to avoid breaking existing config files.

5. **Consider implementing a CI check** that iterates over all `EquipmentTypedConfig` implementors and asserts they pass both JSON and TOML round-trip, using the trait's static methods as a registry (similar to how `config_round_trip.rs` already enumerates them, but generalized).

## References / Citations
- `SimulationConfig` definition: `crates/hares-io/src/config.rs:34-83`
- `OutputFormat` definition: `crates/hares-io/src/config.rs:14-22`
- `EquipmentTypedConfig` trait bounds: `crates/hares-equipment/src/config.rs:103-115`
- JSON round-trip helper: `crates/hares-io/tests/config_round_trip.rs:613-620`
- Regression test TOML workaround: `crates/hares-core/tests/core_output_regressions.rs:30-35`
- Heat pump flatten limitation test: `crates/hares-io/tests/config_round_trip.rs:708-717`
- `ConfigPayload` tagged enum: `crates/hares-equipment/src/config.rs:123-137`
- `EnvironmentState` skip_serializing: `crates/hares-types/src/environment.rs:311-326`
- OCHRE config loading: `vendors/OCHRE/ochre/utils/base.py:22-46` (nested_update, load_csv), `vendors/OCHRE/ochre/utils/hpxml.py` (HPXML parsing with inline defaults)
- OCHRE JSON export: `vendors/OCHRE/ochre/utils/base.py:159-200` (save_json, one-way export only)
