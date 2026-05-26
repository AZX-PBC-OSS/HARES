# Equipment macros: code generation correctness and error messages
**Review ID**: equip-util-03
**Category**: equipment-util
**Date**: 2026-05-26

## Files Reviewed
crates/hares-equipment/src/macros.rs

## Vendor/Reference Files Consulted
- vendors/OCHRE/ochre/Equipment/Equipment.py — base Equipment class; OCHRE uses inheritance, HARES uses composition + delegation
- vendors/OCHRE/ochre/Equipment/HVAC.py — HVAC variants modeled via subclassing, not macros
- vendors/OCHRE/ochre/Equipment/Generator.py — single class, no wrapper types
- vendors/OCHRE/test/test_equipment/test_equipment.py — OCHRE tests concrete Equipment subclasses directly

## Findings
### Finding 1: [Severity: medium] No dedicated unit tests for `delegate_equipment!`; errors only surface at integration level
**Description**: The `delegate_equipment!` macro has zero dedicated test coverage. It is exercised indirectly through lifecycle integration tests (`crates/hares-equipment/tests/lifecycle.rs`) and the `all_registered_equipment_core_output_matches_capabilities_and_ports` test, but there are no `trybuild`/`compile_fail` tests that verify incorrect usage produces helpful error messages. There are no standalone tests that generate Equipment trait impls via the macro and assert they compile and produce expected behavior for trivial parameter sets.
**Code Location**: `crates/hares-equipment/src/macros.rs:14–77` (macro definition); no corresponding test file
**Root Cause**: The macro was written as a convenience for boilerplate reduction but never given its own test suite. The only testing is via the full lifecycle harness, which tests behavior of the inner types (Generator, HeatPumpHeaterCore), not the macro-generated delegation layer itself.
**Impact**: Regressions in macro behavior (e.g., a renamed field on a wrapper struct, a trait method signature change) may only be caught by integration tests that create full equipment instances, making failures harder to diagnose. Incorrect macro usage (wrong inner field name, wrong type) has no test coverage for error message quality.

### Finding 2: [Severity: low] Macro documentation example references code that does not use it
**Description**: The macro doc comment includes the example `delegate_equipment!(HpCooler, inner)` at line 12, implying `HpCooler` uses the delegation macro. In reality, `HpCooler` has a manual `impl Equipment for HpCooler` block (`crates/hares-equipment/src/hvac/heat_pump/cooler.rs:189–269`). The manual impl is required because `HpCooler::step()` needs to pass `companion_heating_rtf` as an extra argument to the inner AC, which the macro's generic delegation cannot express.
**Code Location**: `crates/hares-equipment/src/macros.rs:12` (doc comment); `crates/hares-equipment/src/hvac/heat_pump/cooler.rs:238–243` (actual manual impl)
**Root Cause**: The doc example was written before the cooler's step method acquired the custom `companion_heating_rtf` parameter, or was written as a hypothetical without verification.
**Impact**: Misleading to developers reading the macro documentation — they may attempt to use the macro for types that require customized step delegation and encounter confusing errors.

### Finding 3: [Severity: low] Inconsistent method delegation patterns: heater wrapper types use macro, cooler wrapper types use manual impls
**Description**: Heat pump heater wrappers (`ASHPHeater`, `MinisplitHeater`, `GshpHeater`) use `delegate_equipment!` (heater.rs:376–378), while heat pump cooler wrappers (`HpCooler` at cooler.rs:189, `GshpCooler` at cooler.rs:410) have identical-structure manual `impl Equipment` blocks. Both patterns delegate to an inner type (`core: HeatPumpHeaterCore` vs `inner: AirConditioner`) and both override `ideal_target`. The cooler versions additionally maintain their own `descriptor` and `ports` fields separately from the inner type, then re-sync them during `init()` (cooler.rs:225). The heater versions do not maintain separate `descriptor`/`ports` — they rely entirely on the inner type's fields.
**Code Location**:
- `crates/hares-equipment/src/hvac/heat_pump/heater.rs:376–378` (macro use)
- `crates/hares-equipment/src/hvac/heat_pump/cooler.rs:189–269` (HpCooler manual impl)
- `crates/hares-equipment/src/hvac/heat_pump/cooler.rs:410–468` (GshpCooler manual impl)
**Root Cause**: The `HpCooler` wraps `AirConditioner` whose `descriptor()` returns a different equipment_type name than "ASHP Cooler", so the cooler must maintain and return its own descriptor. The macro documentation example (`HpCooler, inner`) was probably aspirational but couldn't be realized due to this descriptor issue plus the custom `step` argument.
**Impact**: Code duplication in the manual impls — every method is a one-line `self.inner.method()` call. If the Equipment trait gains a new required method, two places must be updated manually instead of one macro.

### Finding 4: [Severity: low] 125 methods not delegated; silent reliance on trait defaults
**Description**: The `delegate_equipment!` macro generates 11 required trait methods (`descriptor`, `ports`, `init`, `update_control`, `step`, `telemetry`, `core_output`, `save_state`, `load_state`, `apply_control_unchecked`, `ideal_target`). The `Equipment` trait has approximately 15 additional methods with default implementations (`core_capabilities`, `actor_seed`, `validate_signal`, `effective_ventilation_effectiveness`, 5 LUT injection methods, `has_charging_curve_lut`, `has_custom_ocv_table`, `has_custom_u_neg_table`, `apply_control`). All of these silently fall through to trait defaults. This is correct for the current wrapper types (none need LUT overrides, none are actors, none are ventilators), but there is no compile-time check to prevent a future inner type from implementing one of these methods only to have it masked by the trait default on the wrapper.
**Code Location**: `crates/hares-equipment/src/macros.rs:16–75` (generated methods); `crates/hares-equipment/src/lib.rs:101–235` (trait definition with defaults)
**Root Cause**: The macro was designed for simple wrappers that only need the 11 core methods. Delegating every trait method would require the inner type to have every method (even optional ones), which would break compilation for inner types that don't override those methods.
**Impact**: Low risk for current usage. If a developer adds an `actor_seed()` override to `Generator` (e.g., for fuel cells that want auto-registered actors), `GasGenerator` and `FuelCell` wrappers would silently return `None` from the trait default instead of the intended value. No warning is emitted.

### Finding 5: [Severity: low] Error messages from incorrect macro usage propagate through generated code rather than the invocation site
**Description**: The `delegate_equipment!` macro uses `macro_rules!` with minimal validation. When misused:
- `delegate_equipment!(MyType)` (missing second arg) → compiler error "expected 2 parameters" at the call site (acceptable)
- `delegate_equipment!(MyType, nonexistent_field)` → compiler error "no field `nonexistent_field` on type `MyType`" pointing into generated code (linker-vague)
- `delegate_equipment!(MyType, field_with_wrong_type)` → compiler error "no method named `descriptor` found for struct `WrongType`" pointing into generated code (very opaque — does not explain that `field_with_wrong_type` needs to be an `Equipment` impl or have matching inherent methods)

The macro cannot distinguish between "field doesn't exist" and "field exists but its type lacks the expected methods" because `macro_rules!` has no type introspection.
**Code Location**: `crates/hares-equipment/src/macros.rs:14–77`
**Root Cause**: `macro_rules!` macros in Rust have no mechanism for type checking or emitting custom compile-time error diagnostics beyond what the generated code naturally produces.
**Impact**: Developers unfamiliar with the macro's expectations may waste time tracing generated-code errors. A proc-macro could provide richer diagnostics but would increase build complexity.

### Finding 6: [Severity: low] `step` return type uses fully-qualified `std::result::Result<(), hares_types::HaresError>` while other methods use `$crate::Result<()>`
**Description**: Within the generated `impl` block, all methods use `$crate::Result<()>` as their return type EXCEPT `step`, which uses `std::result::Result<(), hares_types::HaresError>`. Both resolve to the same concrete type (`std::result::Result<(), hares_types::HaresError>`), so this is not a bug, but it creates a minor inconsistency that may confuse readers.
**Code Location**: `crates/hares-equipment/src/macros.rs:40–47` (step method); all other methods use `$crate::Result<()>` (e.g., line 29, 61, 68)
**Root Cause**: Likely a copy-paste artifact when the `step` method was added or modified separately from the other methods.
**Impact**: No runtime impact, but makes the generated code harder to audit for consistency.

### Finding 7: [Severity: medium] No compile-time field-usage check — if a wrapper struct gains a field, the macro silently ignores it
**Description**: The review instructions specify: "Check that generated code is free of silent default-forgetting — if an equipment type has a field in its struct, the macro must either use it in a trait method or emit a warning." The `delegate_equipment!` macro generates methods that only access `self.$inner`. Any additional fields on the wrapper struct would never be accessed by a trait method. Rust has no built-in mechanism for `macro_rules!` to inspect struct fields and emit warnings about unused fields. Currently, all wrapper structs using the macro (`ASHPHeater`, `MinisplitHeater`, `GshpHeater`, `GasGenerator`, `FuelCell`) have exactly one field named `core` or function as pure newtypes, so the risk is theoretical.
**Code Location**: `crates/hares-equipment/src/macros.rs:14–77`
**Root Cause**: `macro_rules!` cannot introspect struct definitions.
**Impact**: A future developer adding an `extra_field: SomeType` to a wrapper struct using `delegate_equipment!` would not receive any warning that the field is unused by the generated trait impl. The compiler would emit a dead-code warning for the field itself (if never accessed anywhere), but not from the macro.

## Summary
- Total findings: 7
- Critical: 0 / High: 0 / Medium: 2 / Low: 5

## Recommendations
1. **Add `trybuild` compile-fail tests** for the `delegate_equipment!` macro covering: missing arguments, nonexistent field names, and type-mismatch on the inner field. Document the expected error messages and verify they are comprehensible. (`Finding 1`, `Finding 5`)
2. **Add a standalone `delegate_equipment` test** that creates a minimal mock inner type, generates the Equipment impl via the macro, and asserts correct delegation for each method. This would catch regressions when the trait changes. (`Finding 1`)
3. **Fix the doc example** to reference a type that actually uses the macro (e.g., `ASHPHeater` or `GasGenerator`) instead of the misleading `HpCooler`. (`Finding 2`)
4. **Consider normalizing the `step` return type** to use `$crate::Result<()>` for consistency with the other generated methods. (`Finding 6`)
5. **Add a doc comment** listing which trait methods are NOT delegated by the macro and why (trait defaults suffice for optional functionality). This would prevent future developers from accidentally relying on defaults for methods that should be forwarded. (`Finding 4`)
6. **Document the intended usage pattern** for the macro: it is for pure newtype wrappers where the inner type has inherent methods matching the trait signatures. Explain the cooler vs heater asymmetry (custom step delegation requires manual impl). (`Finding 3`)

## References / Citations
- OCHRE Equipment base class: `vendors/OCHRE/ochre/Equipment/Equipment.py` — uses single-inheritance with `update_internal_control()` / `calculate_power_and_heat()` methods. Variants subclass the base class. HARES's delegation pattern is a Rust-idiomatic equivalent.
- OCHRE Generator: `vendors/OCHRE/ochre/Equipment/Generator.py` — single class, no wrapper types. HARES adds `GasGenerator`/`FuelCell` wrappers for registry discoverability.
- HARES Equipment trait: `crates/hares-equipment/src/lib.rs:101–235`
- `delegate_equipment!` usages:
  - `crates/hares-equipment/src/hvac/heat_pump/heater.rs:376–378` (ASHPHeater, MinisplitHeater, GshpHeater)
  - `crates/hares-equipment/src/generator.rs:967–968` (GasGenerator, FuelCell)
