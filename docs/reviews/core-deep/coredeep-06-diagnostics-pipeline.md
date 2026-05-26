# Diagnostics pipeline: NaN detection, negative energy, telemetry consistency, zero overhead in production
**Review ID**: coredeep-06
**Category**: core-deep
**Date**: 2026-05-26

## Files Reviewed
crates/hares-core/src/diagnostics.rs

## Vendor/Reference Files Consulted
None

## Findings
### Finding 1: [Severity: critical]
**Description**: `diagnostics.rs` CSV-writing pipeline is entirely dead code. The module defines `write_header()` (line 59), `write_row()` (line 76), and `capture()` (line 117) with `StepDiagnostics`, `EnvelopeDiag`, and `EquipmentDiag`. None of these have callers anywhere in the codebase except `EnvelopeDiag` in one test (`tests/port_radiant_diag.rs:8`). The doc comment (line 3) claims "Enable by setting `output_verbosity >= 4` in SimulationConfig" but no code path in `dwelling/mod.rs`, `engine.rs`, or any output pipeline checks `output_verbosity >= 4` to invoke `diagnostics::write_header()` or `diagnostics::write_row()`. The actual verbosity-gated output uses a structured Arrow schema built in `hares-io/src/output/columns.rs:140`, not this CSV diagnostics module. The `output_verbosity` field is stored at `dwelling/mod.rs:744` but is only forwarded to `build_schema()` at line 1721; it never reaches `diagnostics::`.

**Code Location**: `crates/hares-core/src/diagnostics.rs:59-114`
**Root Cause**: The diagnostics CSV pipeline appears to have been designed but never wired into the dwelling simulation loop. No integration point exists in `Dwelling::run_timestep` or `Dwelling::step`.
**Impact**: Zero diagnostic output at any verbosity level. NaN detection, negative energy detection, and telemetry consistency checks that the module's doc comment suggests it provides are effectively absent from the production pipeline. Developers relying on the documented `output_verbosity >= 4` diagnostics surface will get no CSV output.

### Finding 2: [Severity: high]
**Description**: No NaN detection exists in the diagnostics module or its integration points. The `write_row` function (line 89) explicitly produces NaN as a sentinel: `unwrap_or(f64::NAN)` when a zone ID is not found. The `capture` function (line 117) copies zone temperatures, port accumulations, and electrical net power without any `is_finite()` or `is_nan()` check on any field. The `EnvelopeDiag` type (line 30) carries `window_solar_w`, `opaque_solar_lwr_w`, `interior_lwr_w`, `infiltration_by_zone`, `internal_gain_w`, `port_convective_w`, and `port_radiant_w` — all floating-point values that can originate from problematic solver states — and none are validated. The `DwellingTelemetry` construction (`dwelling/mod.rs:1922-1938`) similarly omits NaN validation on all float members.

**Code Location**: `crates/hares-core/src/diagnostics.rs:83-114`, `crates/hares-core/src/dwelling/mod.rs:1922-1938`
**Root Cause**: No design for systematic NaN detection was ever implemented. NaN checks exist only in the invariant checker (`invariants.rs`) at zone/tank temperature bounds and electrical net finiteness — far downstream from where NaN originates.
**Impact**: A NaN originating in the humidity solver's semi-implicit iteration, the longwave radiation exchange's matrix solve, or a solar Perez coefficients division-by-zero propagates silently through 5+ phase boundaries (equipment thermal ports → port accumulators → envelope component gains → zone temperature update). It surfaces only if it exceeds temperature bounds in `check_temperatures`, wasting hours of debugging time triangulating the root cause.

### Finding 3: [Severity: high]
**Description**: No negative energy detection exists. The review brief specifies "total delivered energy should never be negative over a billing period." The HVAC heating delivered value is clamped at line 2987-2988 (`hvac_heating_w.max(0.0)` and `hvac_cooling_w.abs()`) which silently hides sign errors rather than detecting and reporting them. The invariant checker validates electrical balance (`check_electrical`, invariants.rs:61) and thermal balance (`check_thermal`, invariants.rs:31) using absolute residuals, but has no check that accumulated delivered energy over time is non-negative. `MetricsCalculator` (`hares-io/src/output/metrics.rs`) accumulates `hvac_heating_wh` but never asserts non-negativity. There is no concept of a "billing period" accumulator with a negativity check.

**Code Location**: `crates/hares-core/src/invariants.rs:31-55`, `crates/hares-core/src/dwelling/mod.rs:2987-2988`
**Root Cause**: Energy accumulation is treated as a post-processing metric rather than a correctness invariant. The clamping at record time (`max(0.0)`, `.abs()`) prevents the data from being diagnostically useful.
**Impact**: A sign error in thermal port contribution (e.g., a heat pump heating mode producing a negative sensible gain due to a cop reciprocity bug) will produce zero heating in output while silently passing all invariant checks, because the electrical balance still sums correctly and temperature bounds remain within range. This masks physical bugs.

### Finding 4: [Severity: high]
**Description**: No telemetry consistency check exists that verifies "sum of per-equipment output equals dwelling total." The dwelling's electrical solver tracks `net_active_kw()` which is assigned to `DwellingTelemetry::total_power_kw` (`dwelling/mod.rs:1933`), and per-equipment `electric_kw` values are collected at line 1887 into `equipment_power_kw`. However, no verification is performed that `sum(equipment_power_kw) ≈ total_power_kw`. The electrical balance invariant (`check_electrical`, invariants.rs:61) only checks `|P_grid + Σ P_ports| < 0.001 kW` — it compares the electrical solver's bus result against the accumulated port values, not the per-equipment telemetry against the dwelling total telemetry. These are different accumulators: equipment telemetry is self-reported by each `Equipment::core_output()`, while port accumulation comes from `PortSlots`. A mismatch signals a telemetry recording bug that no diagnostic catches.

**Code Location**: `crates/hares-core/src/dwelling/mod.rs:3131-3133`, `crates/hares-core/src/telemetry.rs:22`
**Root Cause**: Dwelling total and per-equipment telemetry are populated independently from different sources (solver vs equipment self-report) with no cross-validation.
**Impact**: An equipment actor that under-reports its power in `core_output()` while contributing correctly to ports will produce a dwelling telemetry snapshot where `total_power_kw` does not match `sum(equipment_power_kw)`. Control and RL integrations consuming `DwellingTelemetry` will observe physically inconsistent data. The inconsistency is invisible to all existing diagnostics.

### Finding 5: [Severity: medium]
**Description**: Invariant violation errors lack actionable context. `HaresError::InvariantViolation` (`error.rs:23-28`) carries only `check_name: String`, `value: f64`, and `tolerance: f64`. It lacks `step_index`, `current_time`, `dwelling_id`, `zone_id`, or `equipment_name`. When a violation fires in a fleet simulation (multiple dwellings), the engine quarantines the dwelling (`dwelling/mod.rs:3049-3050` says "the engine then quarantines this dwelling") but the error message provides only a numeric value and tolerance — no indication of which dwelling, which timestep, or which specific zone/equipment triggered it.

**Code Location**: `crates/hares-types/src/error.rs:23-28`, `crates/hares-core/src/dwelling/mod.rs:3048-3267`
**Root Cause**: The `InvariantViolation` variant was designed as a generic numeric check result without considering downstream debugging workflow. The caller (`check_invariants` in `dwelling/mod.rs`) has access to `self.clock.current_step()`, `self.latest_env.current_time`, all zone IDs, and equipment names, but does not attach any of them to the error before propagating.
**Impact**: Users must re-run the simulation with `debug_assertions` enabled (or `check_invariants` feature) and add custom `tracing` instrumentation to identify the triggering dwelling, timestep, and sensor value. This multiplies debugging time in fleet-scale simulations where 1 in 10,000 dwellings triggers a violation.

### Finding 6: [Severity: medium]
**Description**: Intermediate NaN detection is missing at all phase boundaries. The observer pipeline (`observer.rs`, `observer_capture.rs`) captures full snapshots at 5 phase boundaries (post_environment, post_dispatch, post_nonthermal_equipment, post_thermal_equipment, post_solvers, post_zone_update) but performs zero finiteness validation during capture. The `capture_environment` function (`observer_capture.rs:16`) copies solar irradiance components, sky temperature, and zone humidity ratios — any of which can be NaN — without checking. The `capture_solvers` function (line 171) clones `DomainUpdate` objects that carry per-zone temperature and humidity state vectors without validating the underlying float arrays. The `diff_ports` function (line 64) computes per-equipment deltas but only filters by EPSILON — a NaN in either `before` or `after` port accumulator propagates silently.

**Code Location**: `crates/hares-core/src/observer_capture.rs:16-192`
**Root Cause**: The observer was designed as a data-recording layer, not a validation layer. It captures whatever values exist without asserting correctness.
**Impact**: When the `observe` feature is enabled for debugging, captured snapshots may contain NaN values at intermediate phases. This actually reduces the debug utility of the observer because a NaN at `post_thermal_equipment` could originate anywhere in the preceding phase without indication of the specific source.

### Finding 7: [Severity: low]
**Description**: Missing NaN tolerance in negative small-value filtering. The `diff_ports` function (`observer_capture.rs:69-73`) filters thermal deltas using `(ds.abs() > f64::EPSILON || dl.abs() > f64::EPSILON)`. This threshold is too tight: `f64::EPSILON` is ~2.2e-16, but legitimate equipment contributions in the 1e-6 W range (e.g., standby electronics) could be incorrectly filtered as zero. Conversely, accumulated floating-point roundoff from 50+ equipment steps could create spurious deltas just above this threshold. The fluid flow delta threshold (`MIN_DELTA_FLOW_KG_S = 1e-6`, line 86) uses an appropriately physical threshold. Similarly, the invariant checker's tolerance constants (`0.001 kW` electrical, `1e-6 kg` moisture, `1.0 W` thermal floor) are reasonable for physics but none incorporate a NaN-specific early-exit check — a NaN in the summation produces `residual >= tolerance` always being `false` (NaN comparison is always false), meaning NaN bypasses all invariant checks silently.

**Code Location**: `crates/hares-core/src/observer_capture.rs:73`, `crates/hares-core/src/invariants.rs:46`
**Root Cause**: The invariant checks compute `residual = (q_sum - delta_e_storage - q_loss).abs()` then test `residual >= tolerance`. If `q_sum` is NaN, `residual` is NaN, and `NaN >= 1.0` evaluates to `false`, bypassing the check. The check at line 40-43 uses `q_gains.iter().sum()` which sums NaN to NaN.
**Impact**: NaN values pass through the thermal balance invariant undetected. The NaN eventually surfaces as a temperature violation at line 163, several phase boundaries later, wasting debugging time.

### Finding 8: [Severity: low]
**Description**: `DwellingTelemetry` field `outdoor_rh` is misnamed: it holds `self.latest_env.weather.outdoor_humidity_ratio` (a mass ratio, kg/kg, typically 0.001–0.030) but is named `outdoor_rh` (relative humidity, typically 0–1 or 0–100%). This is a data correctness issue that affects any consumer expecting a relative humidity fraction.

**Code Location**: `crates/hares-core/src/dwelling/mod.rs:1936`
**Root Cause**: Field naming `outdoor_rh` does not match the value assigned (`outdoor_humidity_ratio`).
**Impact**: External consumers (RL agents, control loops, Python bindings) parsing `outdoor_rh` at line `telemetry.rs:43-44` receive a humidity ratio instead of relative humidity, producing incorrect setpoint adjustments or comfort metrics. No diagnostic catches this semantic mismatch.

## Summary
- Total findings: 8
- Critical / High / Medium / Low: 1 / 3 / 2 / 2

## Performance Assessment
The `diagnostics.rs` module imposes **zero production overhead** — it is dead code with no invocation path. The invariant checker (`invariants.rs`) correctly uses `#[cfg(any(debug_assertions, feature = "check_invariants"))]` on every method body, so in release builds without the feature flag, all check methods compile to `Ok(())` with zero runtime cost. The observer module (`observer.rs`, `observer_capture.rs`) is similarly gated behind `#[cfg(feature = "observe")]` and `#[cfg(any(debug_assertions, feature = "observe_detailed"))]` with compile-time elimination of all observation sites. The invariant checker additionally pre-allocates scratch buffers (`invariant_conditioned_temps`, `invariant_unconditioned_temps`, `invariant_tank_temps`, `invariant_infiltration_latent`) that live in the `Dwelling` struct — but these are also gated behind `#[cfg(any(debug_assertions, feature = "check_invariants"))]` (lines 727-739), so they do not allocate in production. No data is collected-and-discarded at runtime; the gating is at the type/field level.

**Branch predictability**: The invariant checks inside `InvariantChecker` methods use `#[cfg(...)]` compilation gating (not runtime `if`), so there is no branch prediction concern — the check bodies simply do not exist in the compiled binary. Had runtime gating been used, the check structure (call before every step, one comparison per check body) would be branch-predictable as always-taken (cold path).

## Recommendations
1. **Integrate the diagnostics CSV pipeline** or deprecate/remove it. If kept, wire `diagnostics::capture` and `diagnostics::write_row` into `Dwelling::run_timestep` gated by `self.output_verbosity >= 4`, and add a `BufWriter<File>` field on `Dwelling` behind `#[cfg(any(debug_assertions, feature = "observe"))]`. If the Arrow schema pipeline in `hares-io` supersedes it, update the doc comment and remove dead code.

2. **Add a NaN-screening pass at the top of `check_invariants`**: before computing any residuals, test `q_sum.is_finite()`, `net_kw.is_finite()`, `delta_m_water.is_finite()`, etc., and early-return with a distinct `invariant_error::nan_detected` variant that carries context (step_index, zone_id, value name).

3. **Add a negative-accumulate energy check**: maintain per-zone `total_heating_wh` and `total_cooling_wh` accumulators in the invariant checker (zero-cost when feature-disabled). At each step verify `total_heating_wh >= -1e-6 * self.clock.current_step() as f64` to tolerate floating-point drift while catching sign errors.

4. **Add telemetry consistency validation**: after `telemetry()` is built, compare `sum(equipment_power_kw)` against `total_power_kw` with a tolerance of `max(0.001, 1e-6 * abs(total_power_kw))` and emit a `tracing::warn!` on mismatch.

5. **Add context fields to `InvariantViolation`**: `step_index: u64`, `current_time: DateTime<FixedOffset>`, `dwelling_id: Option<String>`, `zone_id: Option<ZoneId>`. Populate from the caller in `check_invariants`.

6. **Fix NaN propagation through invariant guard**: replace `residual >= tolerance` with `!residual.is_finite() || residual >= tolerance` in `check_thermal` (line 46), `check_electrical` (line 71), and `check_moisture` (line 103). This ensures NaN is never silently accepted as within-tolerance.

7. **Fix `outdoor_rh` naming**: either rename to `outdoor_humidity_ratio` or convert the stored value to relative humidity using psychrometrics.

## References / Citations
- `crates/hares-core/src/diagnostics.rs:59-114` — dead CSV write functions
- `crates/hares-core/src/invariants.rs:31-191` — invariant checks with NaN-passing comparison bug
- `crates/hares-core/src/dwelling/mod.rs:3048-3267` — check_invariants integration with missing context
- `crates/hares-core/src/observer_capture.rs:16-192` — observer captures without NaN validation
- `crates/hares-types/src/error.rs:23-28` — context-poor InvariantViolation error variant
- `crates/hares-core/src/telemetry.rs:22-28,46-48` — telemetry consistency gap
