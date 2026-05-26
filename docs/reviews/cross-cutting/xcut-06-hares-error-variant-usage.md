# HaresError enum variant usage: each variant returned somewhere, no catch-all Physics(...) abuse
**Review ID**: xcut-06
**Category**: cross-cutting
**Date**: 2026-05-26

## Files Reviewed
crates/hares-types/src/error.rs

## Vendor/Reference Files Consulted
None

## Findings

### Finding 1: All production Physics(...) usages are misattributed — zero genuine physics errors [Severity: critical]
**Description**: The `HaresError::Physics` variant is documented as intended for "physically impossible conditions (negative absolute temperature, conservation law violation, negative mass flow)." However, every one of the 6 production-code usages (across 4 call sites) is a config-validation, type-conversion, or logic error, not a genuine physics violation. Zero genuine physics errors are captured by this variant anywhere in the codebase.

**Code Location**:
- `crates/hares-core/src/dwelling/conversions.rs:452` — `"time resolution must be positive, got {millis} ms"` — **config validation, should be `Io`**
- `crates/hares-core/src/dwelling/conversions.rs:457` — `"failed converting duration to u64 ms"` — **integer conversion error, should be `Io`**
- `crates/hares-core/src/dwelling/conversions.rs:609` — `"duration must be positive seconds, got {secs}"` — **config validation, should be `Io`**
- `crates/hares-core/src/dwelling/conversions.rs:614` — `"duration seconds exceed u32 range: {secs}"` — **integer overflow, should be `Io`**
- `crates/hares-core/src/dwelling/mod.rs:2145` — `"invalid time resolution"` — **config/time validation, should be `Io`**
- `crates/hares-core/src/dwelling/mod.rs:2211` — `"simulation already reached configured end"` — **logic/programming error (step-counter overflow), should be `InvariantViolation` or `Dwelling`**

**Root Cause**: Engineers defaulted to `Physics(...)` as a catch-all whenever they needed to return a `HaresError` and no variant seemed to fit, or when they thought any numeric-range check qualified as "physics." The variant name is misleadingly broad.

**Impact**: The `Physics` variant carries no semantic meaning — it is not distinguishable from `Io` or `Dwelling` in error-handling paths. Callers expecting to catch genuine physics violations (e.g., to clamp values, retry with smaller timesteps, or abort with a diagnostic dump) will also catch config parse failures and integer overflows. The variant's very existence is misleading documentation.

### Finding 2: hares-physics crate uses panics/debug_assert! rather than HaresError::Physics [Severity: high]
**Description**: The `hares-physics` crate — the natural home for genuine physics errors — never returns `HaresError::Physics`. Instead, it uses `debug_assert!()` (e.g., `solar.rs:702` "r_glass must be non-negative") and `unwrap()`/`expect()` for physically impossible conditions. This means:
- In debug builds, physics violations panic the entire process.
- In release builds, `debug_assert!` conditions are compiled away, silently allowing non-physical values to propagate.
- There is no recovery path: no call to `HaresError::Physics` anywhere in the physics crate.

**Code Location**:
- `crates/hares-physics/src/solar.rs:702-704` — `debug_assert!(r_glass >= 0.0, ...)`; `debug_assert!(r_int > 0.0, ...)`; `debug_assert!(r_ext > 0.0, ...)`
- `crates/hares-physics/src/water_mains.rs:88` — `debug_assert!(...)` on physically bounded ratio
- 63 `unwrap()`/`expect()` calls across the physics crate for physically-guaranteed values

**Root Cause**: The physics crate was written with an "assert-and-crash" philosophy rather than a fallible-error philosophy. The `HaresError::Physics` variant was added later as a cross-cutting concern without retrofitting the physics crate to use it.

**Impact**: Release builds silently propagate non-physical values (negative reflectance, impossible temperature ratios) through the simulation, producing garbage results without any error signal. Debug builds crash the entire simulation on a single-zone violation, which is inappropriate for fleet simulations.

### Finding 3: Dwelling variant not tested in JSON round-trip test [Severity: medium]
**Description**: The `all_error_variants_round_trip_through_json` test in `crates/hares-types/src/error.rs:55-74` exercises 7 of the 8 `HaresError` variants. The `Dwelling` variant is omitted.

**Code Location**: `crates/hares-types/src/error.rs:55-74` — test vec includes `Physics`, `Envelope`, `Equipment`, `Io`, `Control`, `Tariff`, `InvariantViolation` but not `Dwelling`.

**Root Cause**: Likely a copy-paste omission. The test was written when the variants were defined and `Dwelling` was simply overlooked.

**Impact**: While `#[derive(Serialize, Deserialize)]` makes serialization failure unlikely, there is no test asserting that the `Dwelling` variant actually round-trips correctly. If someone later adds a custom serializer for `Dwelling`, the omission could go undetected.

### Finding 4: Equipment variant used outside equipment crate for general type validation [Severity: low]
**Description**: The `HaresError::Equipment` variant is used 45 times in the `hares-types` crate (`equipment.rs`, `ports.rs`, `schedule.rs`) for shared type validation that is equipment-related but not inside the `hares-equipment` crate. This is defensible — the types crate defines equipment traits, port models, and stochastic schedule distributions used by equipment — but it means the variant scope is broader than "errors originating in the equipment crate."

**Code Location**:
- `crates/hares-types/src/equipment.rs` — 21 uses (KWarg extraction, fuel type parsing, config deserialization)
- `crates/hares-types/src/ports.rs` — 6 uses (port slot validation, negative flow rates, unconnected ports)
- `crates/hares-types/src/schedule.rs` — 17 uses (stochastic distribution parameter validation, boundary policy errors)

**Root Cause**: The types crate is a shared dependency. Equipment-related type validation lives there because equipment types and traits are defined there.

**Impact**: Low. The Equipment variant is consistently used for equipment-domain concerns. However, the stochastic distribution validation errors in `schedule.rs` (e.g., "Bernoulli requires finite p in [0, 1]") are arguably not equipment errors but schedule/distribution-model errors.

### Finding 5: Envelope variant used in hares-types fluid module, outside envelope crate [Severity: low]
**Description**: Three `HaresError::Envelope` usages exist in `crates/hares-types/src/fluid.rs:42,77,85` for fluid payload encode/decode validation. These are in the types crate, not the envelope crate.

**Code Location**: `crates/hares-types/src/fluid.rs:42,77,85` — fluid payload length validation, fluid type discriminator validation, loop ID validation.

**Root Cause**: Fluid loop state encoding/decoding is part of the envelope model but the types are defined in `hares-types` as shared types for checkpoint serialization.

**Impact**: Low. The errors are genuinely envelope-domain errors (fluid loop checkpoint payloads). The types module tightly couples to the envelope domain but this is by design for checkpointing.

### Finding 6: Display impl provides no file/line context for string-wrapping variants [Severity: medium]
**Description**: The `Display` implementation (via `thiserror` derive) for the 7 string-wrapping variants (`Physics`, `Envelope`, `Equipment`, `Io`, `Control`, `Dwelling`, `Tariff`) produces messages of the form `"<category> error: <inner_message>"`. The inner message must be manually constructed with context at each call site. There is no mechanism to capture file, line, or structured diagnostic information. The `InvariantViolation` variant is the only one with structured named fields.

**Code Location**: `crates/hares-types/src/error.rs:8-28` — all 8 variants defined.

**Impact**: Error messages vary widely in quality. Some call sites provide excellent context (e.g., `"window '{}' is missing required u_factor_w_m2_k (U-factor); <UFactor> must be present for every <Window> element per HPXML §6.5"`), while others provide terse messages (e.g., `"invalid time resolution"`). Inconsistent message quality makes debugging harder. Additionally, the lack of structured fields means downstream consumers cannot programmatically distinguish between different error sub-cases within the same variant without parsing strings.

### Finding 7: URDB errors bypass HaresError entirely [Severity: low]
**Description**: URDB tariff parsing errors use a standalone `UrdbParseError` type (`crates/hares-tariff/src/urdb.rs:11-14`) rather than `HaresError::Tariff` or `HaresError::Io`. The `from_urdb_json` Python binding (`crates/hares-python/src/py_tariff.rs:157-161`) converts `UrdbParseError` directly to a `PyValueError`, bypassing the `HaresError` to Python exception mapping in `to_py_err()`.

**Code Location**:
- `crates/hares-tariff/src/urdb.rs:11-14` — `UrdbParseError` struct definition
- `crates/hares-python/src/py_tariff.rs:160-161` — `parse_urdb(&contents).map_err(|e| PyValueError::new_err(format!("URDB parse error: {e}")))`

**Root Cause**: The `hares-tariff` crate was written with its own error type before being integrated into the `HaresError` taxonomy, or the author chose to keep the error type self-contained.

**Impact**: Low. URDB errors are handled correctly — they just take a different path. However, this means `HaresError` is not the universal error type across all crates. A unified error taxonomy would simplify error handling in the Python binding layer, where all error mapping could go through a single `to_py_err()` function.

### Finding 8: InvariantViolation "warning in production" behavior not implemented [Severity: medium]
**Description**: The review instructions note that `InvariantViolation` "may trigger a panic in development mode vs. a warning in production." The `invariants.rs` module uses `#[cfg(any(debug_assertions, feature = "check_invariants"))]` to gate invariant checks, but there is no "warning in production" pathway. In production without the `check_invariants` feature, invariant checks are compiled away entirely with no logging. When the feature IS enabled, violations return `Err(HaresError::InvariantViolation {...})` — there is no path that converts an invariant violation to a warning rather than an error.

**Code Location**:
- `crates/hares-core/src/invariants.rs:36-53` — `check_thermal` only runs when `cfg(any(debug_assertions, feature = "check_invariants"))`
- `crates/hares-python/src/py_dwelling.rs:1822` — `InvariantViolation` maps to `HaresSimulationError::new_err(msg)`, always an error, never a warning

**Root Cause**: The invariant checking system was designed as a compile-time feature gate rather than a runtime policy. There is no `WarningSeverity` or `StrictMode` configuration that would allow production builds to log violations without aborting.

**Impact**: In production fleet simulations, invariant violations either crash the simulation (if the feature is enabled) or are silently ignored (if disabled). There is no middle ground where violations are logged as warnings for post-hoc analysis.

### Finding 9: No missing variants identified for current architecture [Severity: informational]
**Description**: The review instructions asked whether variants are missing for network communication (HELICS) or benchmark-specific validation errors.

- **HELICS/Network**: No HELICS integration exists in the current codebase. No `reqwest`, `hyper`, `ureq`, or HTTP-related crates are used. No network fetch exists for URDB (JSON is loaded from file or passed from Python). A `Network` variant is not needed at this time but should be added when HELICS or remote data fetching is implemented.
- **Benchmark**: Benchmark code uses `criterion` for performance measurement and has no custom validation error type. Validation benchmarks (BESTEST, ASHRAE 140) are driven through test code that uses `panic!` and `assert!` rather than `HaresError`. No `Benchmark` variant is needed.

**Code Location**: N/A — informational finding.

**Impact**: None currently. A `Network` variant should be planned for when HELICS co-simulation or REST API data fetching is added.

## Summary
- Total findings: 9
- Critical: 1 (Finding 1 — all Physics usages misattributed)
- High: 1 (Finding 2 — physics crate uses panics instead of Physics errors)
- Medium: 4 (Findings 3, 6, 8, and 9 informational)
- Low: 3 (Findings 4, 5, 7)

## Variant Usage Coverage Summary

| Variant | Production Uses | Exists in Crate | Correctly Attributed? | Notes |
|----------|----------------|-----------------|----------------------|-------|
| Physics | 6 | hares-types | **NO** | All 6 are config/IO/logic errors, zero genuine physics errors |
| Envelope | 13 | hares-envelope + types | Mostly | 3 uses in hares-types/fluid.rs are borderline |
| Equipment | 336 | hares-equipment + types | Yes | 282 in equipment crate, 45 in types for equipment type validation |
| Io | 38 | hares-core + io | Yes | Parsing, checkpoint, config file errors all correctly use Io |
| Control | 75 | hares-equipment + core | Yes | All in control signal validation, actor registry, telemetry |
| Dwelling | 8 | hares-core + python | Yes | Occupancy, equipment lookup, window validation, lock poison |
| Tariff | 26 | hares-tariff + types | Yes | Tariff type validation, schedule validation |
| InvariantViolation | 13 | hares-core | Yes | Thermal/electrical/moisture balance, temperature bounds |

## Recommendations

1. **Reclassify all 6 Physics usages** to their correct variants: 4 to `Io` (config/parse errors) and 2 to `Dwelling`/`InvariantViolation` (step-counter overflow, time-resolution validation). If no genuine physics errors exist, consider deprecating the variant until the physics crate is retrofitted to use it.

2. **Retrofit the hares-physics crate** to return `HaresError::Physics` for physically impossible input conditions instead of using `debug_assert!`. Replace `debug_assert!` guards with `Result<_, HaresError>` returns so that release builds do not silently propagate garbage values.

3. **Add `Dwelling` variant to `all_error_variants_round_trip_through_json`** test to ensure complete serialization coverage.

4. **Consider adding structured context fields** (e.g., `file: &'static str`, `line: u32`) or adopting a `#[source]` pattern for the string-wrapping variants to improve error traceability. Alternatively, establish a convention for inner message formats (e.g., always include the function name and key values).

5. **Decide on invariant violation policy**: either add a `WarningSeverity` configuration flag that logs violations as `tracing::warn!` rather than returning `Err`, or update the documentation to reflect that invariant violations always abort.

6. **Consider consolidating URDB errors** into `HaresError::Tariff` or `HaresError::Io` so that the Python binding layer's `to_py_err()` function is the single error-mapping point.

## References / Citations
- `HaresError` enum definition: `crates/hares-types/src/error.rs:8-28`
- Python error mapping: `crates/hares-python/src/py_dwelling.rs:1810-1823`
- Invariant checker: `crates/hares-core/src/invariants.rs:1-191`
- RCNetworkError (envelope crate): `crates/hares-envelope/src/rc_network.rs:23-41`
- URDB parse error: `crates/hares-tariff/src/urdb.rs:11-14`
- Test gap for Dwelling: `crates/hares-types/src/error.rs:55-74`
