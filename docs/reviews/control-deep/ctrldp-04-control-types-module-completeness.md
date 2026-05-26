# Control types module: type definitions used by capabilities, dispatch, and compat layers
**Review ID**: ctrldp-04
**Category**: control-deep
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-control/src/types.rs` (primary: re-exports `PriceSignal` from `hares-types`)
- `crates/hares-types/src/control_signal.rs` (defines `ControlSignal`, `ControlCapabilities`, `DRLevel`, `DutyCycleComponent`, `InverterPriority`)
- `crates/hares-control/src/dispatch.rs` (defines `PriorityTier`, `DispatchTarget`, `DispatchRequest`, `PRIORITY_TIER_COUNT`)
- `crates/hares-control/src/signal.rs` (defines `ControlSignalConstructors` trait)
- `crates/hares-control/src/capabilities.rs` (defines `can_accept`, re-exports `ControlCapabilities`)
- `crates/hares-control/src/compat.rs` (OCHRE translation layer)
- `crates/hares-python/src/py_control.rs`, `crates/hares-python/src/py_actor.rs` (Python wrapper types)

## Vendor/Reference Files Consulted
None

## Findings

### Finding 1: Dead public API — `can_accept` has zero external callers [Severity: high]
**Description**: The `can_accept` function (`crates/hares-control/src/capabilities.rs:8`) is defined, unit-tested, and publicly re-exported from `lib.rs:9`, but has **zero external callers** across all crates (`hares-core`, `hares-equipment`, `hares-python`, `hares-fleet`, `hares-io`). Every usage of the function (4 references) is confined to its own definition and test module. Equipment models use `ensure_signal_supported` (`hares-types/src/control_signal.rs:230`) instead, which performs the same capability check but returns a `Result` rather than a `bool`. The `can_accept` function represents an abandoned or redundant API.
**Code Location**: `crates/hares-control/src/capabilities.rs:8-10`
**Root Cause**: Likely written as the original capability-checking entry point before `ensure_signal_supported` was added to `hares-types` for more ergonomic error handling. The function was never removed or deprecated after the replacement was adopted.
**Impact**: Dead public API that misleads consumers into thinking there are two ways to check capabilities. Increases maintenance surface (the function and its test `signals_with_caps()` must be kept in sync with new `ControlSignal` variants) without benefit.

### Finding 2: 13 of 25 `ControlSignal` variants lack ergonomic Rust constructors [Severity: medium]
**Description**: The `ControlSignalConstructors` trait (`crates/hares-control/src/signal.rs:14-43`) provides named constructor methods for only **12 of 25** `ControlSignal` variants. The remaining 13 variants (`CurtailmentPercent`, `ReactiveSetpoint`, `PowerFactorSetpoint`, `InverterPriorityMode`, `IdealCapacity`, `ThermalSetpointDelta`, `IdealCapacityModeOverride`, `EvPlugIn`, `EvDrive`, `EvAwayCharge`, `EvSetReadyBy`, `EventDelay`, `MaxCapacityFraction`) must be constructed directly using struct literal syntax (e.g., `ControlSignal::IdealCapacity { capacity_w: 3500.0 }`), creating an inconsistent API surface where some variants have convenient `ControlSignal::thermal_setpoint(...)` constructors and others require explicit enum struct construction.
**Code Location**: `crates/hares-control/src/signal.rs:14-43` (trait definition) and `crates/hares-control/src/signal.rs:45-126` (impl block). Missing variants can be identified by comparing the 12 trait methods against the 25 enum variants in `crates/hares-types/src/control_signal.rs:37-146`.
**Root Cause**: The 13 missing variants were added to the `ControlSignal` enum later (they appear together in a block from line 92 onward, after the 12 "original" variants on lines 39-88) but the constructor trait was never updated to match.
**Impact**: Forces downstream developers to reach into enum internals for 52% of the signal types. This breaks encapsulation — if a variant's fields are renamed or reorganized, code constructing it via struct syntax must be updated across every call site.

### Finding 3: `signals_with_caps()` capability test covers only 12 of 25 variants [Severity: medium]
**Description**: The `signals_with_caps()` test helper in `crates/hares-control/src/capabilities.rs:18-68` enumerates only 12 of the 25 `ControlSignal` variants, exactly the same 12 that have trait constructors. The remaining 13 variants (same list as Finding 2) are never exercised through the `can_accept` path, meaning capability-validation correctness for these 13 signal types is **not tested** via the `matching_capability_is_accepted` and `missing_capability_is_rejected_for_all_signal_types` test functions.
**Code Location**: `crates/hares-control/src/capabilities.rs:18-68`
**Root Cause**: Same drift as Finding 2 — the test helper was written for the original 12 variants and never updated when 13 more were added to the enum.
**Impact**: Capability validation for the 13 untested variants relies solely on `ensure_signal_supported` tests in `hares-types/tests/`. If a future refactor changes how `required_capability()` maps for these variants, the `can_accept` function would diverge silently because its test coverage is incomplete.

### Finding 4: Unit mismatch (Watts vs Kilowatts) across `ControlSignal` variants without newtype enforcement [Severity: medium]
**Description**: The `ControlSignal` enum mixes power units across variants:
- `IdealCapacity { capacity_w: f64 }` uses **Watts** (`capacity_w` suffix)
- `PowerSetpoint { active_power_kw: f64 }`, `PowerLimit { max_power_kw: f64 }`, `EvAwayCharge { power_kw: f64 }` use **kilowatts**

Both value domains flow into the same equipment handler structs. For example, `heat_pump/heater.rs` stores `ctrl_power_limit_kw: f64` (from `PowerLimit`) alongside `ideal_capacity_w: f64` (from `IdealCapacity`) in the same struct. Conversion happens at each handler via explicit `/ 1000.0` or `* 1000.0` (e.g., `heat_pump/heater.rs:1649`: `let total_kw = electric_kw + fuel_w / 1000.0;`), but there is no compiler-enforced newtype to prevent direct W-to-kW comparison errors. The `hares-physics` crate provides typed `Power`, `Temperature` wrappers (`crates/hares-physics/src/units.rs`) using the `uom` crate, but none are used in any `ControlSignal` variant.
**Code Location**: `crates/hares-types/src/control_signal.rs:104-105` (Watts) vs lines `50`, `54`, `128` (kW). Equipment-level storage: `crates/hares-equipment/src/hvac/heat_pump/heater.rs`, `crates/hares-equipment/src/hvac/air_conditioner.rs`.
**Root Cause**: `ControlSignal` was designed before `hares-physics` typed units were adopted, or the design intentionally avoided `uom` dependency in `hares-types` (which has no `uom` dependency in its `Cargo.toml`).
**Impact**: A future maintainer could mistakenly assign a kW value to `IdealCapacity.capacity_w` or a W value to `PowerLimit.max_power_kw`, causing off-by-1000x errors at runtime. The naming-suffix convention (`_w`, `_kw`) provides only documentation-level safety.

### Finding 5: Temperature unit consistency is documented but unenforced [Severity: low]
**Description**: All temperature fields in `ControlSignal` use Celsius (suffix `_c`): `heating_setpoint_c`, `cooling_setpoint_c`, `deadband_c`, `heating_delta_c`, `cooling_delta_c`. No Kelvin fields exist in the control domain (Kelvin only appears in `hares-physics` and `hares-envelope`). However, as with power units, temperature is a bare `f64` with no `Temperature` newtype from `hares-physics`. While this is less error-prone than the W/kW split (since Celsius is used consistently), direct comparison or arithmetic between Celsius-annotated `f64` values and raw `f64` values from environment code is possible without compiler checks.
**Code Location**: `crates/hares-types/src/control_signal.rs:39-43`, `110-113`
**Root Cause**: Same as Finding 4 — `hares-types` does not depend on `uom`.
**Impact**: Low current risk (no mixed unit domains within the temperature fields), but no guard against future addition of Fahrenheit or Kelvin fields.

### Finding 6: OCHRE compat layer maps only 6 of 25 signal types; field-splitting logic is fragile [Severity: medium]
**Description**: The `ochre_signal_to_control` function (`crates/hares-control/src/compat.rs:24-88`) maps OCHRE key-value pairs to typed `ControlSignal` variants, but only handles 6 signal types: `ThermalSetpoint`, `PowerSetpoint`, `DutyCycle`, `LoadFraction`, `SOCTarget`, and `SelfConsumption`. The remaining 19 signal types cannot be created from OCHRE input. Additionally, the ThermalSetpoint mapping (lines 55-62) uses a fragile heuristic: it checks whether the `equipment_type` string `.contains("cool")` (case-insensitive) to decide whether the temperature value maps to `heating_setpoint_c` or `cooling_setpoint_c`. Equipment named "Air Cooler" or "Passive Cooling" would correctly route to cooling, but "Coolant Loop Heater" would be misrouted. If neither "cool" nor a heating-related keyword matches, only the heating setpoint is set.
**Code Location**: `crates/hares-control/src/compat.rs:55-62` (fragile routing), `crates/hares-control/src/compat.rs:9-16` (only 6 key constants defined)
**Root Cause**: The OCHRE API uses a flat key-value namespace where `Setpoint Temperature (C)` means different things for heating vs cooling equipment. The compat layer has no explicit metadata (e.g., an `EndUse` or `ThermalCategory` discriminator) and falls back to string-matching on the equipment type name.
**Impact**: Misrouted thermal setpoints for ambiguously-named equipment. Unrecognized OCHRE keys are silently skipped (line 78) with only a `tracing::warn!`, so missing mappings are easy to overlook in production.

### Finding 7: `PyControlSignal` lacks ergonomic `@staticmethod` constructors for `EventDelay` and `MaxCapacityFraction` [Severity: low]
**Description**: The `PyControlSignal` class (`crates/hares-python/src/py_control.rs`) provides `@staticmethod` named constructors for 23 of 25 `ControlSignal` variants, but two variants have no dedicated Python constructor:
- `EventDelay` — no `event_delay()` static method
- `MaxCapacityFraction` — no `max_capacity_fraction()` static method

Both variants are supported through the `from_dict`/`to_dict` serialization path and through direct `PyControlSignal` field manipulation, but Python users cannot construct them with the same ergonomic API as other variants.
**Code Location**: `crates/hares-python/src/py_control.rs` (missing static methods), `python/ochre_next/_hares.pyi` (stubs should reflect this gap)
**Root Cause**: Incremental variant additions to `ControlSignal` were not consistently mirrored to all Python constructors.
**Impact**: Python actors that need to emit `EventDelay` or `MaxCapacityFraction` signals must construct them via `from_dict`, which is less discoverable and bypasses type-checking at the constructor level.

### Finding 8: `ControlSignalConstructors` trait is never imported by downstream crates [Severity: low]
**Description**: The `ControlSignalConstructors` trait (`crates/hares-control/src/signal.rs:14`) is publicly exported from `lib.rs:12`, but is never imported (`use hares_control::ControlSignalConstructors`) by any downstream crate (`hares-core`, `hares-equipment`, `hares-python`, `hares-fleet`, `hares-io`). All downstream usage of the constructors relies on the fact that the trait is implemented for `ControlSignal`, so calling `ControlSignal::thermal_setpoint(...)` works without importing the trait explicitly (Rust resolves the method call through the inherent `impl` block on the same type). The trait is only imported within `hares-control` itself (in `compat.rs`, `capabilities.rs` tests, `dispatch.rs` tests). It could be made `pub(crate)` without affecting any consumer.
**Code Location**: `crates/hares-control/src/signal.rs:14`, `crates/hares-control/src/lib.rs:12`
**Root Cause**: The trait was designed for public consumption but ended up being used only internally because `ControlSignal` re-exports the methods through the `impl` block.
**Impact**: Publishes an unnecessary API surface. If a future crate wanted to define a generic function over "anything that can construct control signals," this trait would be useful, but no such use exists.

### Finding 9: `PRIORITY_TIER_COUNT` has exactly one external consumer [Severity: low]
**Description**: The `PRIORITY_TIER_COUNT` constant (`crates/hares-control/src/dispatch.rs:9`) is used outside its definition file by exactly one consumer: the `ControlDispatcher` in `hares-core/src/dwelling/mod.rs` to size its `by_tier: [VecDeque<DispatchRequest>; PRIORITY_TIER_COUNT]` bucket array (2 references in that file). The constant is derived from the four variants of `PriorityTier` (`Schedule = 0`, `UserOverride = 1`, `Grid = 2`, `Safety = 3`), which has a compiler-known cardinality. The constant could be replaced with an inline expression `PriorityTier::VARIANTS.len()` or computed via a const generic, eliminating the need for manual synchronization between the enum and the constant.
**Code Location**: `crates/hares-control/src/dispatch.rs:9` (definition), `hares-core/src/dwelling/mod.rs` (sole consumer)
**Root Cause**: The `PriorityTier` enum uses `#[repr(u8)]` with explicit discriminants; there is no `strum::EnumCount` or similar derive to auto-compute cardinality. The hardcoded constant was the simplest approach at the time.
**Impact**: If a fifth priority tier is added to `PriorityTier`, the developer must remember to update `PRIORITY_TIER_COUNT` from `4` to `5`. Failure to do so would cause silent capacity errors in the dispatcher's bucket array.

### Finding 10: `PriceSignal` re-export in `types.rs` adds no value [Severity: low]
**Description**: The file `crates/hares-control/src/types.rs` contains only a re-export (`pub use hares_types::PriceSignal;`) and a JSON round-trip test. The same type is already available from `hares-types` directly. The re-export does not provide a different namespacing, wrapping, or type alias — it is a pure pass-through.
**Code Location**: `crates/hares-control/src/types.rs:3`
**Root Cause**: The file was likely scaffolded as a placeholder for control-specific types that were never needed, or it existed before `hares-types` was split out of the control crate.
**Impact**: If the `hares-control::PriceSignal` re-export path is used by consumers and later removed, they will need to update imports. There are 53 references to `PriceSignal` across the codebase, but all import from `hares_types::PriceSignal` directly — the re-export path has zero consumers (confirmed via grep).

## Summary
- Total findings: 10
- Critical: 0
- High: 1 (dead public API `can_accept`)
- Medium: 4 (missing constructors, incomplete test, unit mismatch W/kW, OCHRE compat gaps)
- Low: 5 (temperature newtype missing, missing Python constructors, pub trait never imported, single-consumer constant, redundant re-export)

## Recommendations
1. **Deprecate or remove `can_accept`** (`crates/hares-control/src/capabilities.rs:8`). All consumers use `ensure_signal_supported` in `hares-types` instead. If the function is retained for symmetry, remove its public re-export from `lib.rs` to signal deprecation.
2. **Add Rust constructors for the 13 missing `ControlSignal` variants** in the `ControlSignalConstructors` trait (`crates/hares-control/src/signal.rs`). This brings the trait to full 1:1 coverage with the enum and eliminates the need for struct literal construction outside the trait definition.
3. **Complete the `signals_with_caps()` test** (`crates/hares-control/src/capabilities.rs:18-68`) with the 13 missing variants so that all 25 capability validations are exercised.
4. **Adopt typed unit wrappers for `ControlSignal`** or, at minimum, add `const W_PER_KW: f64 = 1000.0;` to a shared location and use it consistently in equipment handlers instead of inline `/ 1000.0` and `* 1000.0` literals. Consider a doc comment block on the `ControlSignal` enum listing the unit convention for each numerical field.
5. **Harden the OCHRE compat layer** (`crates/hares-control/src/compat.rs:55-62`): replace string-matching on `equipment_type` with a structured discriminator (e.g., an `EndUse` or `ThermalCategory` parameter). Document the full list of OCHRE keys that are NOT yet mapped (19 signal types with no translation path).
6. **Add `@staticmethod` constructors for `EventDelay` and `MaxCapacityFraction`** on `PyControlSignal` (`crates/hares-python/src/py_control.rs`).
7. **Replace `PRIORITY_TIER_COUNT` with a derive** (e.g., `strum::EnumCount`) or compute it inline from `PriorityTier` variant count so the dispatcher array size stays in sync automatically.
8. **Consider removing `types.rs`** (`crates/hares-control/src/types.rs`) since the sole re-export (`PriceSignal`) has zero consumers through the `hares-control` path.

## References / Citations
- `crates/hares-types/src/control_signal.rs:37-146` — `ControlSignal` enum (25 variants)
- `crates/hares-types/src/control_signal.rs:156-186` — `ControlCapabilities` bitflags (25 flags, 1:1 with variants)
- `crates/hares-types/src/control_signal.rs:197-227` — `required_capability()` mapping
- `crates/hares-types/src/control_signal.rs:230-243` — `ensure_signal_supported()` (actually used by consumers)
- `crates/hares-control/src/capabilities.rs:8-10` — `can_accept()` (zero external callers)
- `crates/hares-control/src/capabilities.rs:18-68` — `signals_with_caps()` test helper (covers only 12/25)
- `crates/hares-control/src/signal.rs:14-43` — `ControlSignalConstructors` trait (12/25 constructors)
- `crates/hares-control/src/dispatch.rs:9` — `PRIORITY_TIER_COUNT = 4`
- `crates/hares-control/src/dispatch.rs:15-21` — `PriorityTier` enum
- `crates/hares-control/src/compat.rs:24-88` — OCHRE translation (6/25 signal types mapped)
- `crates/hares-control/src/types.rs:1-21` — `PriceSignal` re-export only
- `crates/hares-python/src/py_control.rs` — Python `PyControlSignal` (23/25 static constructors)
- `crates/hares-physics/src/units.rs` — Typed unit wrappers (unused by control types)
- `crates/hares-equipment/src/hvac/heat_pump/heater.rs:1649` — Example inline W→kW conversion
