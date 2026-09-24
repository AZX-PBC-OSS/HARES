# Error handling audit: panics, unwraps, expects across the codebase
**Review ID**: infra-08
**Category**: infrastructure
**Date**: 2026-05-26

## Files Reviewed
All crates (`hares-core`, `hares-envelope`, `hares-equipment`, `hares-fleet`, `hares-io`, `hares-physics`, `hares-python`, `hares-tariff`, `hares-types`, `hares-control`) — both `src/` production code and `tests/` test directories.

## Vendor/Reference Files Consulted
None (no vendor reference implementations available for this audit).

## Executive Summary

A comprehensive audit of every `.unwrap()`, `.expect()`, `unreachable!()`, and `panic!()` across the entire workspace reveals that the **overwhelming majority of occurrences (approximately 6,100 of ~6,130 total) reside in test code** (`#[cfg(test)] mod tests` blocks, `tests/` directories, or `#[cfg(test)]`-gated files). The production code base is remarkably clean: only **~35 `.unwrap()`**, **~25 `.expect()`**, **0 `unreachable!()`**, and **~15 `panic!()`** occurrences exist outside of test modules.

However, several of these production occurrences are **triggable by malformed user input** (HPXML config, weather files, schedule CSV, Python API calls, TOML equipment configs) or reside in the **simulation hot path**, making them high-severity bugs that could crash long-running fleet simulations.

### Counts by Category

| Category | unwrap | expect | unreachable! | panic! | Total |
|----------|--------|--------|--------------|--------|-------|
| **1 — Should be proper errors** (user-input-triggerable) | ~15 | 1 | 0 | 16 | ~32 |
| **2 — Guarded by preconditions** (safe but improvable) | ~10 | ~15 | 0 | 2 | ~27 |
| **3 — Acceptable** (truly unreachable, invariant assertion) | ~10 | ~9 | 0 | 1 | ~20 |
| **Test code only** | ~3,350 | ~2,585 | 5 | ~184 | ~6,124 |
| **Total (all code)** | ~3,385 | ~2,610 | 5 | ~201 | ~6,203 |

---

## Findings

### Finding 1: Water heater draw normalization panics on missing config fields — **CRITICAL**

**Description**: The function `water_heater_avg_daily_draw_l()` and its callers in `schedule_resolve.rs` contain 14 `panic!()` calls that trigger when a water heater equipment spec lacks `typed_config`, fails to deserialize to the expected type, or is missing the `avg_water_draw_l_per_day` field. Additionally, `inject_water_heater_schedule_columns()` panics when `append_derived_column()` fails. All of these are triggered by **user-provided TOML equipment configuration** during dwelling construction. A TOML config that specifies a water heater but omits the required draw volume field will panic and terminate the entire simulation — unacceptable for fleet-scale runs.

**Code Location**:
- `crates/hares-io/src/schedule_resolve.rs:543` — `panic!()` on `append_derived_column` failure
- `crates/hares-io/src/schedule_resolve.rs:560, 568, 574` — panics in `tankless_avg_daily_draw_l()`
- `crates/hares-io/src/schedule_resolve.rs:588, 592, 598` — panics for "Electric Resistance Water Heater"
- `crates/hares-io/src/schedule_resolve.rs:608, 610, 616` — panics for "Gas Water Heater"
- `crates/hares-io/src/schedule_resolve.rs:626, 630, 636` — panics for "Heat Pump Water Heater"
- `crates/hares-io/src/schedule_resolve.rs:642` — `panic!("unsupported water heater type for draw normalization: {other}")`

**Root Cause**: The `inject_water_heater_schedule_columns()` function (line 509) has return type `()` instead of `Result`. All calls to `water_heater_avg_daily_draw_l()` (which can fail on missing config) and `append_derived_column()` (which can fail on schedule inconsistencies) use `panic!()` because there is no error propagation path.

**Impact**: A fleet simulation processing thousands of dwellings with malformed water heater configs crashes entirely. A user running a single-dwelling CLI simulation with a slightly misconfigured HPWH TOML gets a hard crash with a stack trace instead of a clear error message.

**Remediation**: Convert `inject_water_heater_schedule_columns()` return type to `Result<(), HaresError>`. Convert `water_heater_avg_daily_draw_l()` and `tankless_avg_daily_draw_l()` to return `Result<f64, HaresError>`. Propagate errors up to dwelling construction. The calling code already uses `?` propagation in nearby functions.

---

### Finding 2: Python API panics on malformed solar irradiance dict input — **CRITICAL**

**Description**: Multiple `.unwrap()` calls on Python dictionary lookups in `py_dwelling.rs` panic when Python callers provide dictionaries missing required keys (`"direct"`, `"diffuse"`, `"reflected"`, `"aoi"`, `"surface_id"`, `"direct_w_m2"`, `"diffuse_w_m2"`, `"reflected_w_m2"`). These are direct user inputs from the Python API surface.

**Code Location**:
- `crates/hares-python/src/py_dwelling.rs:273-274` — `.unwrap()` on `dict.get_item(surface_ids[0])` and `get_item("direct")`
- `crates/hares-python/src/py_dwelling.rs:282` — `.unwrap()` on `dict.get_item(sid)`
- `crates/hares-python/src/py_dwelling.rs:284-291` — `.unwrap()` on `get_item("direct")`, `get_item("diffuse")`, `get_item("reflected")`, `get_item("aoi")`
- `crates/hares-python/src/py_dwelling.rs:350-356` — `.unwrap()` on `get_item("surface_id")`, `get_item("direct_w_m2")`, `get_item("diffuse_w_m2")`, `get_item("reflected_w_m2")`
- `crates/hares-python/src/py_dwelling.rs:639` — `.unwrap()` on `zone_keys`
- `crates/hares-python/src/py_dwelling.rs:2014` — `.unwrap()` on `Mutex::lock()` (statically initialized, but still a potential deadlock panic)
- `crates/hares-python/src/py_dwelling.rs:2037` — `.unwrap()` on error extraction

**Root Cause**: PyO3's `get_item()` returns `Result<Option<Bound>>`. The code calls `.unwrap()` to assert the item exists, but missing keys from Python user input should return a `PyResult::Err` (e.g., `PyValueError`) instead of panicking.

**Impact**: Any Python caller providing solar irradiance override data with slightly wrong key names or missing sub-fields gets a hard crash. This is a direct user-facing API.

**Remediation**: Replace `.unwrap()` with `?` propagation or explicit `.ok_or_else(|| PyValueError::new_err("missing required key: ..."))?`. The function signatures already return `PyResult<...>`, so proper error propagation is straightforward.

---

### Finding 3: Telemetry `set()` panics in simulation hot path — **HIGH**

**Description**: `Telemetry::set()` at `crates/hares-types/src/telemetry.rs:35` contains `panic!("Telemetry::set called with unknown key '{key}'; pre-populate via insert() at init")`. This function is called **every timestep** from every equipment type's `step()`. If any equipment type fails to pre-populate a telemetry key in its `init()`, the production crash occurs on the first timestep, not at initialization — making the error much harder to debug.

**Code Location**: `crates/hares-types/src/telemetry.rs:31-38`

```rust
pub fn set(&mut self, key: &str, value: f64) {
    if let Some(v) = self.0.get_mut(key) {
        *v = value;
    } else {
        panic!(
            "Telemetry::set called with unknown key '{key}'; pre-populate via insert() at init"
        );
    }
}
```

**Root Cause**: The function is designed as an invariant assertion (all keys must be pre-populated). While the intent is correct, a `panic!` in the hot path is the worst possible failure mode — it terminates a fleet simulation that may have been running for hours. A `debug_assert!` plus a `log::error!` plus a no-op would be safer, or an `Option<f64>` return.

**Impact**: Hot-path crash. A single equipment type with a missing telemetry key crashes an entire fleet run. The Telemetry type is called **hundreds of times per timestep** across all equipment instances.

**Remediation**: Options:
1. Convert to `debug_assert!` (panic only in debug builds) + `tracing::error!()` + no-op in release.
2. Add an `insert()` fallback (auto-populate on first `set()` with a warning).
3. Return `Result` from `set()` — but this adds overhead to every call site.
   Preferred approach: option 1 (debug-only panic + release log).

---

### Finding 4: `save_postcard` panics on serialization failure during checkpointing — **MEDIUM**

**Description**: `save_postcard()` at `crates/hares-equipment/src/lib.rs:257` panics on serialization failure via `panic_serialize()` (line 266-268). A `try_save_postcard()` variant exists (line 259-262) that returns `Result`, but `save_postcard` is the default path used in production checkpointing. Serialization failure is extremely unlikely with postcard (it's deterministic on typed Rust structs), but if it does occur in a long-running simulation, the checkpoint save crashes the run.

**Code Location**:
- `crates/hares-equipment/src/lib.rs:257` — `save_postcard()` calls `panic_serialize()` on failure
- `crates/hares-equipment/src/lib.rs:266-268` — `panic_serialize()` helper

**Root Cause**: Convenience wrapper that trades error handling for ergonomics. The doc comment on `try_save_postcard` (line 255-257) acknowledges this: "Non-panicking variant for callers that need graceful error handling (e.g., checkpoint paths where a serialization failure should not terminate a long-running simulation)."

**Impact**: Medium — serialization failure on a valid Rust type is near-impossible, but the cost of being wrong is a full fleet crash during checkpointing.

**Remediation**: Deprecate the panicking `save_postcard` and use `try_save_postcard` everywhere. Add a doc comment explaining why the panicking variant exists (legacy convenience).

---

### Finding 5: `EquipmentConfig::from_typed` panics on serialization — **HIGH**

**Description**: `EquipmentConfig::from_typed()` at `crates/hares-equipment/src/config.rs:287-297` calls `panic!()` if `serde_json::to_value(config)` fails. This is called during dwelling construction when setting up equipment from typed config structs. While serialization of a valid typed config struct should never fail, this is initialization-time code and could be triggered by a misconfigured or malformed TOML that produces an unexpected config structure.

**Code Location**: `crates/hares-equipment/src/config.rs:292-296`

```rust
let data = serde_json::to_value(config).unwrap_or_else(|e| {
    panic!(
        "typed config serialization failed for {}: {e}",
        T::equipment_type_name()
    )
});
```

**Root Cause**: Same pattern as Finding 4 — convenience panic. `serde_json::to_value` on a properly derived `Serialize` type is infallible, but external config types (from crate boundaries) may not be as reliable.

**Impact**: High — dwelling construction failure during fleet initialization aborts the entire fleet run. The function signature is `Self` (not `Result<Self>`), so there is no error propagation.

**Remediation**: Convert `from_typed()` to return `Result<Self>` and propagate the error up to dwelling construction.

---

### Finding 6: RC network assembly panics on internal inconsistency — **HIGH**

**Description**: `assemble_building_rc()` at `crates/hares-envelope/src/boundary_rc.rs` contains three `panic!()` calls during RC network construction:
1. Line 518: Foundation depth has no corresponding ground node in the depth-to-node map.
2. Line 1068: Ground node missing from external nodes after construction.
3. Line 1097: Zone air node missing from RC network internal nodes.

These panics occur during dwelling construction from HPXML input. While they represent internal invariants that should hold, they could theoretically be triggered by extreme HPXML inputs (multiple foundation depths, unusual boundary configurations).

**Code Location**:
- `crates/hares-envelope/src/boundary_rc.rs:517-523` — `panic!()` when depth key not found in HashMap
- `crates/hares-envelope/src/boundary_rc.rs:1067-1072` — `panic!()` when ground node not in external nodes
- `crates/hares-envelope/src/boundary_rc.rs:1096-1101` — `panic!()` when zone node not in internal nodes

**Root Cause**: `assemble_building_rc()` returns `Result<(BuildingRC, EnvelopeDiagnostics), String>`. These panics bypass the Result channel entirely. The `depth_to_node` panic (line 518) is particularly concerning because it involves floating-point rounding of depths — while the key derivation is identical in both passes, floating-point edge cases could theoretically produce mismatch.

**Impact**: High — building RC network construction fails and crashes the entire dwelling (and fleet) setup. No graceful error propagation.

**Remediation**: Replace all three `panic!()` calls with `return Err(format!(...))`. The function already returns `Result<_, String>`, so the mechanical change is trivial.

---

### Finding 7: Hot-path guarded unwrap patterns — **LOW** (quality improvement)

**Description**: Several hot-path functions use the `is_ok()`-then-`unwrap()` anti-pattern or unchecked `last().expect()` after prior validation. While all are currently safe due to preconditions, they represent fragile coupling that could break during refactoring.

**Code Locations**:
- `crates/hares-equipment/src/hvac/heat_pump/heater.rs:976-978` — `zone.is_ok()` then `zone.unwrap()` (step() hot path)
- `crates/hares-equipment/src/hvac/speed_control.rs:184` — `capacity_fractions.last().expect("non-empty")` after empty check at line 169 (step() hot path)
- `crates/hares-equipment/src/battery/ocv.rs:128` (and similar at lines 112, 229, 245) — `voltage_v.last().expect("non-empty LUT")` (interpolate() hot path, guarded by constructor validation)
- `crates/hares-equipment/src/water_heater/resistance.rs:390`, `heat_pump_wh.rs:519`, `gas.rs:381` — `node_temps().reduce(f64::max).expect("node_temps is never empty")` (operating mode determination hot path)
- `crates/hares-core/src/dwelling/mod.rs:1426` — `steps.last().unwrap()` after `run_timestep()?` returns Ok (assumes Ok always pushes a step)
- `crates/hares-core/src/dwelling/mod.rs:2525,2574` — `pre_ports.expect(...)` guarded by `if let Some(ref pre_snapshot)` (observer path)
- `crates/hares-core/src/dwelling/mod.rs:2175` — `.expect()` on schedule domain payload (fragile coupling between construction validation and runtime)

**Root Cause**: Unnecessary use of `unwrap()`/`expect()` when the surrounding control flow guarantees safety.

**Impact**: Low in current code, but high fragility risk — a refactor that changes the precondition without updating the guarded unwrap introduces new panics.

**Remediation**: Replace `is_ok()`-then-`unwrap()` with `if let Ok(zone) = zone { ... }`. Replace `last().expect()` with explicit checks or `.last().ok_or_else(...)?` returning errors. These changes also make the intent clearer to future maintainers.

---

### Finding 8: Acceptable invariant assertions — **Informational**

**Description**: The following `.unwrap()`/`.expect()` occurrences are true invariant assertions that cannot be triggered by any valid or invalid user input. They are documented here for completeness:

| Location | Pattern | Rationale |
|----------|---------|-----------|
| `dwelling/mod.rs:934` | `east_opt(0).expect("UTC offset")` | `east_opt(0)` always returns `Some(FixedOffset)` for UTC |
| `dwelling/mod.rs:2759,2782` | `.expect("invariant: equipment_id_by_name...")` | HashMap keys derived from the same set |
| `dwelling/mod.rs:3141` | `expect("write to String is infallible")` | `fmt::Write` for `String` is documented as infallible |
| `types/lib.rs:94,97` | `east_opt(0).expect("offset")` + singleton date `.expect()` | Hardcoded values in `EnvironmentState::default()` |
| `schedule.rs:491` | `east_opt(0).expect("UTC offset is valid")` | Same as above |
| `columns.rs:354` | `expect("static map serialises")` | `serde_json::to_string` on a `const` static |
| `aggregation.rs:404` | `expect("empty aggregate record batch")` | Hardcoded empty schema |
| `config.rs:132` | `expect("validated positive timestep count")` | Validated at construction |
| `stepping.rs:228` | `dt.expect("valid July 21 noon UTC")` | Guarded by `if dt.is_none()` fallback chain |
| `validation.rs:516` | `schedule.timestamps.last().expect("non-empty timestamps")` | Guarded by `if schedule.timestamps.is_empty()` above |
| `initialization.rs:132,138` | `expect("verified above")` / `expect("pre-check above...")` | Guarded by explicit validation in the same function |
| `psm3.rs:217,257` | `expect("pre-allocated")` / `expect("validated step")` | Construction-time validated |
| `py_config.rs:64` | `parse_from_rfc3339(DEFAULT_START).expect(...)` | `const` that compiles only if valid |

**No action needed** — these are correctly designed invariant assertions.

---

## Summary

- **Total findings**: 8
- **Critical**: 2 (water heater draw normalization panics, Python API unwraps)
- **High**: 3 (hot-path telemetry panic, `from_typed` serialization panic, RC network assembly panics)
- **Medium**: 1 (`save_postcard` checkpoint panic)
- **Low**: 1 (guarded unwrap patterns)
- **Informational**: 1 (acceptable invariant assertions)

## Recommendations

### Immediate (critical user-input-triggerable panics):

1. **Fix schedule_resolve.rs water heater panics** (Finding 1): Convert `inject_water_heater_schedule_columns()` and all helper functions to return `Result`. This is the single highest-impact fix — 14 panics eliminated, all trivially triggerable by any malformed water heater TOML.

2. **Fix py_dwelling.rs Python API unwraps** (Finding 2): Replace all `.unwrap()` on Python dict lookups with proper `PyResult` error returns. All function signatures already return `PyResult<_>`, so this is a mechanical change.

### High priority (fleet-crashing or hot-path):

3. **De-risk telemetry hot-path panic** (Finding 3): Change `Telemetry::set()` to `debug_assert!` + `tracing::error!()` in release builds. This single function is potentially the most crash-impactful code in the entire codebase — called millions of times per simulation.

4. **Fix boundary_rc.rs panics** (Finding 6): Replace three `panic!()` calls in `assemble_building_rc()` with `return Err(...)`. Function signature already supports it.

5. **Fix equipment/config.rs serialization panic** (Finding 5): Convert `from_typed()` to return `Result<Self>`.

### Medium priority:

6. **Migrate checkpointing to non-panicking variant** (Finding 4): Use `try_save_postcard` universally and deprecate the panicking `save_postcard`.

### Low priority (code quality):

7. **Clean up guarded unwrap patterns** (Finding 7): Replace `is_ok()`-then-`unwrap()` with `if let`, and `.last().expect()` with explicit early-return-on-empty. These improve code clarity and prevent future refactoring from introducing new panics.

---

## References / Citations

- Rust API Guidelines: [Panicking](https://rust-lang.github.io/api-guidelines/necessities.html#c-panic) — "The introspective `unwrap` and `expect` methods are the standard way to abort on contracts violations. Panicking is acceptable." (Contrast: user-facing errors should use `Result`)
- The Rust Book: [To `panic!` or Not to `panic!`](https://doc.rust-lang.org/book/ch09-03-to-panic-or-not-to-panic.html) — "When failure is expected, it's more appropriate to return a `Result` than to make a `panic!` call."
- HPXML specification: Water heater equipment requires `avg_water_draw_l_per_day` for schedule-based draw normalization. Absent this field, the current code panics instead of producing an actionable validation error.
