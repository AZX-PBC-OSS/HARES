# CoreOutput contract validation: invariants, bitflag checks, test coverage
**Review ID**: test-04
**Category**: tests
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-types/tests/core_output_invariants.rs`
- `crates/hares-types/src/telemetry.rs`
- `crates/hares-types/src/equipment.rs` (validate_core_contract, ElectricPower, Soc, FuelPower, OperatingMode)

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/OutputProcessor.cc`

The EnergyPlus OutputProcessor is concerned with output variable setup, reporting frequency management, and meter composition. It performs input-level validation (duplicate names, units mismatches, invalid resource references) but does not validate physical plausibility of output values at runtime. HARES's runtime `validate_core_contract` serves a different purpose — fail-fast post-step contract enforcement — and the comparison is limited to testing philosophy: EnergyPlus's defensive input validation at registration time contrasts with HARES's limited post-step presence/absence check.

## Findings

### Finding 1: Missing nan / finite validation on bare-f64 CoreOutput fields [Severity: high]
**Description**: `validate_core_contract` (`equipment.rs:943-1062`) only verifies that fields declared in `CoreCapabilities` are present (`Some`) or absent (`None`). It does NOT validate the actual numeric values embedded in those fields. Four fields — `reactive_power_kvar`, `thermal_output_w`, `sensible_cooling_w`, and `latent_cooling_w` — are `Option<f64>` with no type-level validation wrapper. Equipment can emit `Some(f64::NAN)` or `Some(f64::INFINITY)` into any of these and `validate_core_contract` will accept it. Two additional fields — `setpoint_c` (`Option<f64>`) and `cop` / `main_power_kw` (`Option<f64>`) — likewise have no finite checks.

**Code Location**:
- `equipment.rs:887-901` — `CoreFlows` definition; bare `f64` fields with no validation
- `equipment.rs:905-914` — `CoreState` definition; `setpoint_c: Option<f64>` unprotected
- `equipment.rs:918-926` — `CorePerformance` definition; `cop`, `main_power_kw` unprotected
- `equipment.rs:943-1062` — `validate_core_contract` — no NaN/infinity guard

**Root Cause**: The refactoring that introduced `ElectricPower`, `Soc`, and `FuelPower` as validated newtypes was only partially applied. `reactive_power_kvar`, `thermal_output_w`, `sensible_cooling_w`, `latent_cooling_w`, `setpoint_c`, `cop`, and `main_power_kw` were left as bare `f64` without validation wrappers or inline checks.

**Impact**: Numerical instability in equipment physics (e.g., division by zero in COP calculation, sqrt of negative in reactive power estimation) can inject NaN into `CoreOutput` without detection. The simulation loop (`dwelling/mod.rs:2534,2582`) calls `validate_core_contract` as a fail-fast gate but silently passes corrupted data, which propagates to downstream telemetry consumers, port accumulators, and checkpoint serialization.

**Vendor Reference**: EnergyPlus guards against invalid frequencies and duplicate names at registration but has no equivalent runtime physical-validity gate; this finding is about HARES-specific consequences.

---

### Finding 2: No bitflag / operating-mode consistency checks [Severity: high]
**Description**: `validate_core_contract` does not check for physically impossible operating-mode combinations or mode-versus-power contradictions. The `OperatingMode` enum (`equipment.rs:175-190`) includes modes that are mutually exclusive (e.g., `HeatingHPAndER` explicitly combines two heat sources, but nothing prevents equipment from reporting both `Heating` and `Cooling` on separate fields or across sub-components). More critically:

- **Zero power with active mode**: Equipment with `operating_mode != Off` and `operating_mode != Standby` can report zero `electric_kw`, zero `thermal_output_w`, and zero `fuel_w` — a physical impossibility for an actively-running device.
- **Mode-watt mismatch**: An `OperatingMode::Cooling` can be paired with positive `thermal_output_w` (heating) — the sign convention says positive = heating, negative = cooling.
- **No charging/discharging conflict check**: No validation that `CHARGING` and `DISCHARGING` aren't conflated.
- **No standalone On validation**: `OperatingMode::On` (code 12) was added in commit 594c9a2 ("checkpoint") without corresponding contract rules: what power flows are valid in this generic mode?

**Code Location**:
- `equipment.rs:943-1062` — `validate_core_contract` — no mode-vs-flows consistency checks
- `equipment.rs:175-190` — `OperatingMode` enum — contains mutually-conflicting modes without guard rules
- `equipment.rs:887-936` — `CoreFlows`, `CoreState`, `CorePerformance` — no cross-field validation

**Root Cause**: The contract validation was designed as a capabilities-presence check (what fields should be `Some`/`None`) rather than a semantic consistency check. Mode/flow consistency was deferred to per-equipment validation that was never implemented.

**Impact**: Equipment with coding bugs or uninitialized memory can report `OperatingMode::Heating` with zero thermal flow, or `OperatingMode::Cooling` with zero electric consumption, producing physically inconsistent observability data that misleads downstream analysis.

---

### Finding 3: OperatingMode::On = 12 is unreachable via TryFrom<u8> [Severity: critical]
**Description**: The `OperatingMode::On = 12` variant was added to the enum (`equipment.rs:189`) but the `TryFrom<u8>` implementation (`equipment.rs:198-220`) was never updated and still only handles discriminants 0-11. The catch-all `_ => Err(...)` arm rejects discriminant 12. Three separate tests have incomplete mode lists missing `On`, and the `operating_mode_try_from_u8_round_trip` test (`equipment.rs:2505-2512`) explicitly asserts that `try_from(12)` returns an error — which is the opposite of what it should do given `On = 12`.

**Code Location**:
- `equipment.rs:189` — `On = 12` declared
- `equipment.rs:198-220` — `TryFrom<u8>` — no arm for 12 → falls to wildcard error
- `equipment.rs:2510` — `assert!(OperatingMode::try_from(12).is_err())` — incorrect assertion
- `core_output_invariants.rs:103-138` — `operating_mode_codes_are_stable_and_unique` — missing `On`
- `equipment.rs:1214-1229` — `new_operating_mode_variants_round_trip_through_json` — missing `On`
- `equipment.rs:2160-2173` — `operating_mode_numeric_codes_are_stable` — missing `On`

**Root Cause**: `On = 12` was added in commit 594c9a2 ("checkpoint") as a late addition to the development branch. The `TryFrom` impl and all mode-enumerating tests were never updated to include it. The `operating_mode_try_from_u8_round_trip` test was actively updated to assert 12 is an error (the loop was changed from `0..=12` to `0..=11` with a separate `assert_err(12)` line), which is a test that encodes the bug as expected behavior.

**Impact**: Any code path that constructs `OperatingMode` from a raw `u8` discriminant — including checkpoint deserialization formats, Helix/UAPI protocol messages, Python FFI integer-to-mode conversions, and the `operating_mode_codes_are_stable_and_unique` property tests — will reject `On`. Equipment that sets `operating_mode = Some(OperatingMode::On)` directly via the Rust variant works fine, but the inconsistency between the Rust discriminant (`as u8 == 12`) and `TryFrom<u8>` creates a data loss trap: writing `On` as `12_u8` to a file and reading it back silently fails. The `as_code()` method returns `12.0` but `try_from(12)` fails.

---

### Finding 4: No physical-range validation on CoreOutput numeric fields [Severity: medium]
**Description**: `validate_core_contract` does not verify that values fall within physically plausible ranges. The review instructions specify that temperatures should be within -50°C to 80°C, SOC within [0,1], and power/energy values should be non-negative where required. Only `Soc` and `ElectricPower` have per-type range validation:

- `Soc::try_from` enforces [0.0, 1.0] — COVERED at type level
- `ElectricPower::consumption/generation` enforce non-negative finite — COVERED at type level
- `FuelPower::new` enforces non-negative finite — COVERED at type level
- `setpoint_c` — NO range check (no -50°C to 80°C guard)
- `thermal_output_w` — NO physical bounds
- `cop` — NO plausible range (should be > 0 for most equipment, physically < 20 for commercial)
- `main_power_kw` — NO non-negative check (negative main power is physically impossible)
- `sensible_cooling_w` — NO sign validation (documented as "negative or zero" but unenforced)
- `latent_cooling_w` — NO sign validation (same)

**Code Location**: `equipment.rs:943-1062` — `validate_core_contract` — no range/sign checks

**Root Cause**: Range validation was left to per-type constructors. Where constructors exist (`ElectricPower`, `Soc`, `FuelPower`) it works. Where bare `f64` is used (`thermal_output_w`, `setpoint_c`, `cop`, etc.) there is no enforcement layer.

**Impact**: Equipment that produces a `cop` of -2.3 (e.g., from a division-by-zero in a degenerate performance curve) passes contract validation and enters the telemetry/port pipeline undetected. A `setpoint_c` of 150.0 from a Fahrenheit-to-Celsius conversion bug would likewise pass.

**Vendor Reference**: EnergyPlus (OutputProcessor.cc:284-345) validates input at registration time (e.g., `CheckReportVariable` matches keys and frequencies) but has no equivalent runtime value-range check. HARES's `validate_core_contract` is the natural place for this defense-in-depth check.

---

### Finding 5: core_output_invariants.rs is mis-scoped — tests types, not CoreOutput [Severity: medium]
**Description**: The file `crates/hares-types/tests/core_output_invariants.rs` contains three tests: `electric_power_property_invariants_hold_for_random_finite_inputs`, `soc_property_invariants_hold_for_random_finite_inputs`, and `operating_mode_codes_are_stable_and_unique`. Despite its filename suggesting CoreOutput contract invariant testing, none of these tests exercise `CoreOutput`, `CoreFlows`, `CoreState`, `CorePerformance`, or `validate_core_contract`. The tests verify the property invariants of `ElectricPower`, `Soc`, and `OperatingMode` in isolation — type-level unit tests that belong in `equipment.rs`'s `#[cfg(test)] mod tests` block, not in a file named after the CoreOutput contract.

**Code Location**: `core_output_invariants.rs:1-139` — entire file

**Root Cause**: The file was created early in development as a placeholder for CoreOutput invariant tests. When type-level validation was implemented via `ElectricPower`/`Soc`/`FuelPower` newtypes, the individual type tests were added here instead of being placed alongside their types in `equipment.rs`, and the intended CoreOutput contract integration tests were never written.

**Impact**:
1. Misleading file organization: developers looking for CoreOutput contract tests find type-level tests instead.
2. Gaps the file should cover are entirely absent:
   - No NaN injection into `CoreOutput` fields
   - No empty `CoreOutput::default()` contract validation test
   - No single-zone / multi-zone output scenarios
   - No boundary timestep (first/last step) validation
   - No out-of-range `setpoint_c` rejection test
   - No cross-field consistency test (mode-zero vs power-zero)
3. Redundant coverage: the `operating_mode_codes_are_stable_and_unique` test duplicates the `operating_mode_numeric_codes_are_stable` test in `equipment.rs:2160-2173` and the `operating_mode_codes_are_stable_and_unique` test in `equipment.rs:2504-2512`.

---

### Finding 6: Telemetry type has zero value validation [Severity: medium]
**Description**: The `Telemetry` type (`telemetry.rs:8-64`) wraps `HashMap<String, f64>` and provides `insert`, `set`, and `get` accessors. None of these methods validate the input value for finiteness, NaN, or any physical range. Any `f64` value — including `NaN`, `f64::INFINITY`, `f64::NEG_INFINITY`, or `-9999.0` — is accepted silently. The `get` method returns `Option<f64>` with no post-read validation either.

**Code Location**:
- `telemetry.rs:22-38` — `insert` and `set` — no validation on `f64` input
- `telemetry.rs:43-45` — `get` — no validation on read

**Root Cause**: `Telemetry` was designed as a simple key-value store for equipment-equipment communication. Value validation was expected to happen at the producer (equipment) side, but without a validation layer at the consumer side or in the storage layer, corrupted values propagate silently.

**Impact**: Equipment that produces NaN telemetry (e.g., from numerical instability in heat-pump performance calculations) injects valid-NaN entries into the `Telemetry` map. Downstream consumers (actors reading SOC, the dwelling output reporter reading equipment states) get NaN without any indication of corruption. The dwelling output code already works around this defensively with `unwrap_or(0.0)` at `dwelling/mod.rs:3005-3038`, which converts NaN to 0.0 silently — masking the root cause.

---

### Finding 7: EnergyPlus reference does not provide comparable runtime validation [Severity: low]
**Description**: The EnergyPlus `OutputProcessor.cc` reference file validates input at registration time (reporting frequency validity, meter duplicate detection, resource type matching, units consistency) but contains no runtime validation of output variable values for physical plausibility, NaN, infinity, or range. EnergyPlus relies on the individual component models to produce physically valid outputs. HARES's `validate_core_contract` serves a different purpose — a post-step contract gate — but is currently limited to presence/absence checks.

**Code Location**: `OutputProcessor.cc:34-1032` (entire file, read offset 0-1032)

**Impact**: The reference code does not offer a solution to HARES's gap. The validation responsibility must be designed from first principles within HARES's contract framework. This finding is informational rather than actionable.

---

### Finding 8: validate_core_contract tests don’t exercise NaN-injection or out-of-range simulation state [Severity: low]
**Description**: The five `validate_core_contract_*` tests in `equipment.rs:2322-2494` all use well-formed inputs. No test injects NaN into `reactive_power_kvar`, infinity into `thermal_output_w`, or out-of-range values into `setpoint_c` to confirm the contract validator catches them (or that downstream code appropriately handles the rejection). The contract validator currently would accept all of these, so the tests implicitly encode the incomplete validation as the expected behavior.

**Code Location**:
- `equipment.rs:2321-2494` — all `validate_core_contract_*` tests

**Root Cause**: The test suite was written to match the implemented validation logic, not the desired contract spec. Since range/sanity checks were never implemented, no test exercises them.

**Impact**: If range/sanity validation is added to `validate_core_contract`, new tests must be written from scratch. The existing tests provide good coverage for the present/absence dimension but the file-location (in `equipment.rs` rather than `core_output_invariants.rs`) means the intended test file for comprehensive CoreOutput validation is effectively empty.

---

## Summary
- **Total findings**: 8
- **Critical**: 1 (Finding 3: `OperatingMode::On` unreachable via `TryFrom<u8>`)
- **High**: 2 (Finding 1: missing NaN/finite checks; Finding 2: no mode-power consistency checks)
- **Medium**: 4 (Finding 4: no physical-range validation; Finding 5: mis-scoped test file; Finding 6: Telemetry no validation; Finding 8: no NaN-injection test coverage)
- **Low**: 1 (Finding 7: vendor reference not relevant)

## Recommendations

1. **Fix `TryFrom<u8>` for `OperatingMode::On`** (`equipment.rs:198-220`): Add `12 => Ok(Self::On)` arm. Update `operating_mode_try_from_u8_round_trip` loop to `0u8..=12` and remove the negative assertion on 12. Add `On` to all mode-enumerating tests: `operating_mode_codes_are_stable_and_unique` in `core_output_invariants.rs`, `operating_mode_numeric_codes_are_stable` in `equipment.rs:2160-2173`, and `new_operating_mode_variants_round_trip_through_json` in `equipment.rs:1214-1229`.

2. **Add finite/NaN guards to `validate_core_contract`** (`equipment.rs:943-1062`): After the presence/absence check, validate that all `Some(f64)` fields are finite. For `reactive_power_kvar`, `thermal_output_w`, `sensible_cooling_w`, `latent_cooling_w`, `setpoint_c`, `cop`, and `main_power_kw`, reject NaN and infinity. Consider wrapping `reactive_power_kvar` in a validated newtype (like `ElectricPower`) to centralize constraints.

3. **Add physical-range validation to `validate_core_contract`**: Enforce `setpoint_c` in [-50.0, 80.0], `cop` > 0.0, `main_power_kw` >= 0.0, `sensible_cooling_w` <= 0.0, `latent_cooling_w` <= 0.0. Document the bounds with domain citations (ASHRAE, AHRI).

4. **Add mode-versus-flows consistency checks to `validate_core_contract`**: When `operating_mode` is set to an active mode (anything other than `Off` or `Standby`), require at least one flow (electric, thermal, or fuel) to be non-zero. When `operating_mode` is `Off`, require all flows to be zero. When `thermal_output_w` is positive, require mode to be a heating variant; when negative, require mode to be a cooling variant.

5. **Relocate and expand `core_output_invariants.rs`**: Move the existing isolated-type tests to the `#[cfg(test)]` block in `equipment.rs` where they belong. Repurpose `core_output_invariants.rs` as a proper property-based and edge-case test suite for `validate_core_contract`, covering: empty `CoreOutput`, single-zone output, maximum zone count, first/last timestep boundary, NaN injection into each bare-f64 field, out-of-range temperature/setpoint, cross-field inconsistencies.

6. **Add `insert`/`set` validation to `Telemetry`** (`telemetry.rs:22-38`): Reject `f64::NAN`, `f64::INFINITY`, and `f64::NEG_INFINITY`. Consider adding a `try_insert` that returns `Result` or making `insert` panic on non-finite input (matching the `set` method's defensive design).

## References / Citations

- `crates/hares-types/src/equipment.rs:943-1062` — `validate_core_contract` implementation
- `crates/hares-types/src/equipment.rs:175-190` — `OperatingMode` enum with `On = 12`
- `crates/hares-types/src/equipment.rs:198-220` — `TryFrom<u8> for OperatingMode` — missing arm 12
- `crates/hares-types/src/equipment.rs:2505-2512` — `operating_mode_try_from_u8_round_trip` — incorrect assertion
- `crates/hares-types/src/equipment.rs:887-936` — `CoreFlows`, `CoreState`, `CorePerformance` struct definitions
- `crates/hares-types/src/telemetry.rs:8-64` — `Telemetry` type with no value validation
- `crates/hares-types/tests/core_output_invariants.rs:1-139` — mis-scoped invariant test file
- `crates/hares-core/src/dwelling/mod.rs:2534,2582` — `validate_core_contract` call sites in simulation loop
- `vendors/EnergyPlus/src/EnergyPlus/OutputProcessor.cc:1-1032` — vendor reference (no runtime value validation)
