# Equipment type registry: canonical name resolution, config dispatch, error messages
**Review ID**: config-io-03
**Category**: config-io
**Date**: 2026-05-26

## Files Reviewed
crates/hares-equipment/src/registry.rs crates/hares-equipment/src/config.rs

## Vendor/Reference Files Consulted
vendors/OCHRE/ochre/Equipment/__init__.py

## Findings
### Finding 1: [Severity: medium]
**Description**: Error message for unrecognized equipment type omits the list of valid names. The `create()` method returns `"unknown equipment class: {ochre_class}"` which includes the unrecognized name, but does not surface the alphabetically sorted list of valid names from `known_names()`, which already exists and is tested for sorted order.

**Code Location**: `crates/hares-equipment/src/registry.rs:170-172`

**Root Cause**: `known_names()` (line 154) is implemented and returns `Vec<&str>` in sorted order, with test coverage at line 249 (`known_names_returns_sorted_names`), but this method is never called in the error branch of `create()`.

**Impact**: When users specify an unrecognized equipment type in TOML config they see `"unknown equipment class: BadName"` with no guidance on valid alternatives. This wastes debugging time — they must search source code to discover supported types.

### Finding 2: [Severity: high]
**Description**: The `check-no-magic-config` Makefile target excludes `scheduled_load.rs` and `event_load.rs` from the raw-config-access scan. Both files contain `init()` methods that use raw config accessors directly (`config.get_f64()`, `config.get_str()`, `config.get_f64_array()`), bypassing typed config safety.

**Code Location**:
- `Makefile:13-21` (scan file list excludes `scheduled_load.rs`, `event_load.rs`)
- `crates/hares-equipment/src/scheduled_load.rs:249-292` (extensive `config.get_f64()` calls in `init_from_config`)
- `crates/hares-equipment/src/event_load.rs:463-516` (`EventBasedLoad::init()` uses raw accessors)
- `crates/hares-equipment/src/event_load.rs:946-980` (`WetAppliance::init()` uses raw accessors)

**Root Cause**: The Makefile scan file list at lines 13-21 enumerates directories/files explicitly (`hvac/`, `water_heater/`, `battery/`, `ev/`, `pv/`, `generator.rs`, `ventilation.rs`) and was not updated when `scheduled_load.rs` and `event_load.rs` were added. The grep patterns scan for raw config accessors (`config.get_f64(`, etc.) only within `fn init...` body extraction, so the linter is blind to raw-config usage in these files.

**Impact**: These equipment types (Lighting, Plug Loads, Refrigerator, EV, Clothes Washer, Dishwasher, Clothes Dryer, Cooking Range, and ~15 other appliance/scheduled load types) operate on raw string-keyed config with no compile-time type checking. Miscased keys (e.g. `"SensibleGainFraction"` vs `"sensible_gain_fraction"`) silently fall through to defaults. This is a regression risk if typed config migration is attempted for these types — the linter would not flag raw accessors left behind.

### Finding 3: [Severity: medium]
**Description**: `extract_numeric` and `extract_bool` in `hvac/core_config.rs` use `cfg`-gated dual implementations: typed-only in production (`#[cfg(not(test))]`, lines 138-140), but fall back to raw `config.get_f64()`/`config.get_bool()` in test builds (lines 143-146, 169-173). This is correct behavior, but the Makefile scan cannot detect it because the raw accessors are called inside helper functions defined outside init bodies, not directly within `fn init(...)` blocks.

**Code Location**: `crates/hares-equipment/src/hvac/core_config.rs:138-174`

**Root Cause**: The Makefile scan extracts `fn init...` bodies with awk and searches for literal `config.get_f64(` patterns. The `extract_numeric` test-path raw access (`config.get_f64(key)`) is inside the helper definition body, not inside an init function body. The scan correctly passes because non-test code uses `typed_value()` which reads exclusively from `ConfigPayload::Typed` (line 124-128).

**Impact**: Low current risk because the production path is typed-only. However, the scan's architectural dependency on pattern matching within init bodies means it would not catch a future refactor that inlines a raw accessor into an init body of a helper-definition file.

### Finding 4: [Severity: low]
**Description**: OCHRE reference registers equipment class names that HARES does not support: base classes `Heater`, `Cooler`, and `WaterHeater` in `EQUIPMENT_BY_NAME`. HARES maps `"Water Heating"` to a descriptive `register_error` (water_heater/mod.rs:51-57) but provides no analogous error guidance for bare `"Heater"` or `"Cooler"` class names. Also OCHRE uses the class name `MinisplitAHSPHeater`/`MinisplitAHSPCooler` whereas HARES uses `MSHP Heater`/`MSHP Cooler`.

**Code Location**:
- `vendors/OCHRE/ochre/Equipment/__init__.py:42-49` (OCHRE registers `Heater`, `Cooler` base classes)
- `crates/hares-equipment/src/water_heater/mod.rs:51-57` (only `"Water Heating"` has error guidance)
- `crates/hares-equipment/src/hvac/heat_pump.rs:14-43` (HARES registers "MSHP Heater"/"MSHP Cooler", no "Minisplit" aliases)

**Root Cause**: HARES intentionally removes abstract base classes from its registry (they have no constructor), but only `"Water Heating"` receives a user-facing error. The `register_error` facility exists but is underutilized.

**Impact**: If a config resolver or migration tool emits OCHRE base class names, users see a generic `"unknown equipment class"` error instead of the actionable guidance provided for `"Water Heating"`.

### Finding 5: [Severity: low]
**Description**: The `EquipmentConfig` struct contains a `test_extras` field gated on `#[cfg(test)]` (config.rs:153-155) that allows `raw_data()` and `raw_data_or_empty()` to return data even for `Typed` payloads during tests. This test-only bypass of the typed/raw barrier could cause tests to pass with values that would not be available in production.

**Code Location**: `crates/hares-equipment/src/config.rs:153-155, 220-230, 234-245`

**Root Cause**: The `test_extras` HashMap and the `#[cfg(test)]` guards on `raw_data()` and `raw_data_or_empty()` intentionally provide a controlled escape hatch for fixture-based tests that predate full typed config migration.

**Impact**: Low — confined to test builds. However, a test that populates `test_extras` and then asserts behavior relying on those values is testing a configuration path that does not exist in production. This creates false confidence that certain raw config keys will be read by typed-only equipment.

## Summary
- Total findings: 5
- Critical: 0
- High: 1
- Medium: 2
- Low: 2

## Recommendations
1. Enhance the `create()` error message in `registry.rs:170` to include the sorted list from `known_names()`. This closes the usability gap and satisfies the review requirement that errors include both the unrecognized name and a sorted list of valid names.
2. Add `scheduled_load.rs` and `event_load.rs` to the Makefile `check-no-magic-config` scan file list. Both files should either (a) be migrated to typed config with `require_typed()`, or (b) be explicitly excluded with a documented justification in the Makefile comment explaining why raw access is permissible for these types.
3. Add `register_error` entries for OCHRE base class names `"Heater"` and `"Cooler"` to match the existing pattern for `"Water Heating"`, providing actionable guidance rather than a generic unknown-class error.
4. Consider extending the `check-no-magic-config` scan to catch raw config accessors in all functions called from init bodies (not just directly in init bodies), or add a `#[deny]` annotation on the raw accessor methods when called from non-test code.
5. Audit existing tests that rely on `test_extras` / raw config access through `#[cfg(test)]` paths to ensure they are not testing behaviors unavailable in production.

## References / Citations
- `crates/hares-equipment/src/registry.rs:17-88` — CANONICAL_EQUIPMENT_NAMES constant
- `crates/hares-equipment/src/registry.rs:160-174` — `create()` with error message
- `crates/hares-equipment/src/registry.rs:154-158` — `known_names()` returns sorted names
- `crates/hares-equipment/src/registry.rs:225-246` — bidirectional name coverage tests
- `Makefile:10-65` — check-no-magic-config implementation
- `crates/hares-equipment/src/config.rs:106-115` — `EquipmentTypedConfig` trait requiring `#[serde(deny_unknown_fields)]`
- `crates/hares-equipment/src/hvac/core_config.rs:124-174` — `typed_value` / `extract_numeric` / `extract_bool` with cfg-gated dual paths
- `crates/hares-equipment/src/hvac/helpers.rs:26-28` — `first_f64` marked `#[doc(hidden)]` as compatibility-only
- `crates/hares-equipment/src/water_heater/mod.rs:51-57` — `register_error` for "Water Heating"
- `vendors/OCHRE/ochre/Equipment/__init__.py:36-101` — OCHRE `EQUIPMENT_BY_NAME` mapping
