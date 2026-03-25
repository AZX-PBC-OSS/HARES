---
id: ACTOR-010
title: Conditioned oracle integration test
kind: implement
depends_on:
  - ACTOR-007
  - ACTOR-009
files_to_touch:
  - tests/conditioned_oracle.rs
  - crates/hares-equipment/src/hvac/ideal_hvac.rs
  - crates/hares-core/src/dwelling/mod.rs
  - crates/hares-envelope/src/thermal_solver/infiltration.rs
references:
  - tests/freefloat_oracle.rs
  - docs/tickets/ACTOR-INDEX.md
verification:
  - cargo test --test conditioned_oracle --features observe
---

## Background/Context

With IdealHvac equipment wired and OCHRE conditioned reference CSVs generated, we can create integration tests comparing HARES conditioned behavior against OCHRE. This validates that the equipment-based ideal HVAC produces correct zone temperatures and loads.

## Work to Do

- [x] Create `tests/conditioned_oracle.rs` modeled on `freefloat_oracle.rs`
- [x] Build Dwelling from BEopt HPXML — for ideal mode, clear all equipment and inject IdealHvac with `IdealCapacityMode::On`
- [x] Use `dwelling.latest_env()` for equipment init (not fake environment)
- [x] Run simulation with observer enabled
- [x] Collect per-step: indoor temp, attic temp, hvac_heating_w, hvac_cooling_w from observer
- [x] Load OCHRE reference CSV from `tests/fixtures/conditioned_{mode}/{scenario}/`
- [x] Compare:
  - Indoor temperature MAE (< 0.5°C ideal, < 2.0°C dynamic)
  - HVAC heating load comparison (mean value tolerance 50%)
  - HVAC cooling load comparison (mean value tolerance 50%)
  - Attic temperature MAE (< 3.0°C)
- [x] Hourly comparison table with IdealHvac telemetry (mode, target, capacity, output)
- [x] Step-0 heat balance dump
- [x] Three scenarios x two modes = 6 test functions
- [x] Use `ResampleOverrides::ochre_compat()` for ZOH weather resampling (parity testing)

## Files Touched

- `tests/conditioned_oracle.rs`: **New** — conditioned oracle integration tests
- `crates/hares-equipment/src/hvac/ideal_hvac.rs`: Three bug fixes found during test development
- `crates/hares-core/src/dwelling/mod.rs`: Added `latest_env()` accessor
- `crates/hares-envelope/src/thermal_solver/infiltration.rs`: Fixed undefined diagnostic variables

## Bugs Found and Fixed

### Bug 1: `current_target_c` not updated on schedule setpoint changes mid-mode

**Root cause**: `update_mode()` only set `current_target_c` on mode transitions. When the heating schedule shifted from 21.67C (day) to 18.33C (night) while already in Heating mode, the solver kept back-calculating capacity for the stale daytime setpoint.

**Impact**: Winter MAE was 1.067C (FAIL) — HARES maintained 21.67C during night hours when it should have dropped to 18.33C.

**Fix**: Always update `current_target_c` from effective setpoints in `update_mode()`, including during `is_cycle_change_allowed` early-return. Covers both normal thermostat decisions and min-cycle lockout periods.

### Bug 2: Stale `ideal_capacity_w` used in Deadband mode

**Root cause**: `step()` used `self.ideal_capacity_w` when `use_ideal_cached` was true, ignoring the current mode. A leftover capacity value from a previous dispatch would continue driving thermal output after transitioning to Deadband.

**Fix**: `step()` checks mode first — Deadband always produces 0W. Also `set_mode()` clears `ideal_capacity_w` on transition to Deadband.

### Bug 3: Solver returns wrong-sign capacity (heating in Cooling mode)

**Root cause**: When in Cooling mode targeting 24.44C but outdoor temp drops below the zone temp, the solver correctly computes positive capacity (heating needed to maintain target). But the equipment should NOT heat in Cooling mode — it should let the zone float down and transition to Deadband naturally.

**Impact**: Summer MAE was 1.300C (FAIL) — HARES heated to 24.44C for 48 hours straight, never allowing zone to cool below cooling setpoint.

**Fix**: `step()` clamps capacity sign to match mode: Heating clamps to max(0.0), Cooling clamps to min(0.0). One-step underdelivery of 0W is preferable to injecting reversed thermal energy.

### Bug 4: Undefined variables in infiltration diagnostics

**Root cause**: `delta_t` and `forced_eff` referenced in diagnostic gain computation were undefined after code was modified.

**Fix**: Replaced with `dt_c = t_out - zone.temperature_c` and conditional `forced_sens_eff` (1.0 for unbalanced, `1-recovery_eff` for balanced).

## Measures of Success

- [x] Indoor temperature MAE < 0.5C for ideal mode:
  - Winter: 0.073C
  - Summer: 0.219C
  - Spring: 0.102C
- [x] Indoor temperature MAE < 2.0C for dynamic mode:
  - Winter: 0.314C
  - Summer: 0.478C
  - Spring: 0.358C
- [x] Attic temperature MAE < 3.0C (achieved: 0.86-1.26C)
- [x] HVAC loads are non-zero and have correct sign
- [x] Tests produce diagnostic output for CI visibility

## Tests Added

### IdealHvac Unit Tests
- `setpoint_schedule_change_updates_target_while_heating` — verifies `current_target_c` tracks day-night schedule shift while already in Heating mode
- `cooling_mode_clamps_positive_ideal_capacity_to_zero` — verifies wrong-sign clamping in Cooling mode
- `heating_mode_clamps_negative_ideal_capacity_to_zero` — verifies wrong-sign clamping in Heating mode
- `deadband_outputs_zero_despite_stale_ideal_capacity` — verifies Deadband produces 0W even with stale capacity

### Conditioned Oracle Integration Tests
- `conditioned_ideal_winter_48h` / `_summer_48h` / `_spring_72h` — IdealHvac with solver back-calculation vs OCHRE ideal fixture
- `conditioned_dynamic_winter_48h` / `_summer_48h` / `_spring_72h` — Native ASHP equipment vs OCHRE dynamic fixture

## Verification

- [x] `cargo test --test conditioned_oracle --features observe` passes all 6 tests
- [x] `cargo test --test freefloat_oracle --features observe` passes (no regression)
- [x] `cargo test --workspace --exclude hares-python` passes (no regression)
