---
id: HARES-020
title: "hares-equipment — HVAC Common and Thermostat FSM"
kind: implement
depends_on: [HARES-018]
files_to_touch:
  - crates/hares-equipment/src/hvac/mod.rs
  - crates/hares-equipment/src/hvac/common.rs
references:
  - docs/architecture/02-equipment-and-ports.md
  - docs/PHYSICS_DECISIONS.md
verification:
  - cargo check -p hares-equipment
  - cargo test -p hares-equipment
  - cargo clippy -p hares-equipment -- -D warnings
---

## Background/Context
All HVAC equipment shares thermostat deadband logic, capacity management, fan power, SHR, and duct DSE calculations. Centralising these in `hvac/common.rs` prevents duplication across furnaces, heat pumps, and air conditioners and establishes the `HvacEquipment` wrapper type used by downstream tickets.

Note: HARES-006 (biquadratic curves) is **not** a dependency of this ticket. Biquadratic evaluation is first used in HARES-022 (dynamic HVAC). Do not add HARES-006 to `depends_on`.

## Work to Do
- [ ] Define `ThermostatConfig` struct: `hysteresis_c: f64` (default 1.0), `cutout_ratio: f64` (default 0.0, range 0.0–1.0), `min_cycle_time_s: f64` (defaults to `0`, meaning no minimum cycle enforcement; non-zero values must be > 0 and < simulation duration, matching OCHRE base class behavior), `use_ideal_capacity: bool` (default `false`)
- [ ] Validate `cutout_ratio` at `init`: return `Err` if not in `[0.0, 1.0]`
- [ ] Define `ThermostatMode` enum: `Heating`, `Cooling`, `Deadband`
- [ ] Implement thermostat FSM: Heat ON when `T < T_heat_sp - hysteresis`, OFF when `T > T_heat_sp + hysteresis * cutout_ratio`; Cool ON when `T > T_cool_sp + hysteresis`, OFF when `T < T_cool_sp - hysteresis * cutout_ratio`
- [ ] Implement min cycle time enforcement — hold current mode until `min_cycle_time_s` has elapsed since last switch (skipped when `min_cycle_time_s == 0`)
- [ ] Ideal-capacity mode: when `env.time_res >= Duration::from_secs(300)` (5 minutes) OR `use_ideal_capacity: bool` config flag is set, the thermostat operates in ideal-capacity mode instead of cycling mode. In ideal mode, call `DomainSolver::solve_ideal_capacity(env, zone)` to determine the exact heat rate needed to maintain setpoint, then set `duty_cycle = min(ideal_rate / rated_capacity, 1.0)`. This matches OCHRE's automatic switching at coarse timesteps (see OCHRE `HVAC.py:239-241`).
- [ ] Implement setpoint priority stack: HPXML static < schedule time-varying < runtime `ControlSignal::ThermalSetpoint`
- [ ] Enforce invariant at `init`: `T_cool_sp - T_heat_sp >= 2 * hysteresis`; return `Err` if violated
- [ ] Handle ResStock `No Space Heating` / `No Space Cooling` schedule columns: the schedule CSV contains the literal string `"No Space Heating"` or `"No Space Cooling"` in the setpoint column. During schedule parsing (not during `step()`), these strings are mapped to sentinel numeric values: `"No Space Heating"` → -999°C, `"No Space Cooling"` → +999°C. The parsed schedule stores these as `f64` sentinels; during `step()`, the thermostat FSM receives the sentinel value directly and never satisfies the ON condition, keeping the equipment off. Do not discard these rows silently as OCHRE does. See `docs/PHYSICS_DECISIONS.md` for the authoritative record of this decision.
- [ ] Add `supply_air_temp_c: f64` field to `HvacEquipment` base struct. Default values by equipment type (set at init, overridable via `EquipmentConfig`):
  - Gas furnace: 54.4°C (130°F)
  - Electric furnace: 48.9°C (120°F)
  - ASHP HP-only: 32.2°C (90°F) at 8.3°C OAT, with linear OAT-dependent variation: `T_supply = 32.2 + 0.15 * (T_outdoor_c - 8.3)`
  - ASHP HP+aux: 40.6°C (105°F)
  - Mini-split (heat): 43.3°C (110°F)
  - Baseboard: N/A (no forced air — field unused)
- [ ] Add `airflow_cfm_per_ton: f64` field to `HvacEquipment` base struct, defaulting to 375 (not OCHRE's 312 CFM/ton). Scale with `AirflowDefectRatio` from HPXML when present. Document this change in `docs/PHYSICS_DECISIONS.md` (OCHRE's 312 CFM/ton may be a ResStock calibration value; changing it affects energy totals).

Note: All references to physics decision documentation in this ticket use the path `docs/PHYSICS_DECISIONS.md`.
- [ ] Implement HVAC base helpers: capacity list management, EIR lookup, fan power calculation, SHR from config, duct DSE application, zone heat fraction distribution
- Note: `update_control()` must NOT be called inside `step()`. The engine calls `update_control()` separately before `step()`. This is the trait contract — equipment must not re-invoke control updates during stepping.
- [ ] Define `HvacEquipment` wrapper type composing thermostat state with base HVAC helpers

## Files to Touch
- `crates/hares-equipment/src/hvac/mod.rs`: module declaration and public re-exports
- `crates/hares-equipment/src/hvac/common.rs`: new file — `ThermostatConfig`, `ThermostatMode`, thermostat FSM, HVAC base helpers, `HvacEquipment`

**Downstream merge note**: HARES-022 also modifies `hvac/common.rs`. Coordinate field additions — fields added in this ticket (`supply_air_temp_c`, `airflow_cfm_per_ton`) should not conflict with speed-control state fields added in HARES-022. All new fields for HARES-022 belong in a separate extension block or are gated on a feature that HARES-022 introduces.

## Measures of Success
- [ ] Thermostat FSM: steps through a heating cycle — ON at `T < setpoint - hysteresis`, OFF at `T > setpoint + hysteresis * cutout_ratio`
- [ ] Min cycle time holds mode for the configured minimum duration regardless of temperature
- [ ] `ControlSignal::ThermalSetpoint` overrides static HPXML setpoint
- [ ] `init` returns `Err` when cool setpoint minus heat setpoint is less than `2 * hysteresis`
- [ ] `init` returns `Err` when `cutout_ratio` is outside `[0.0, 1.0]`
- [ ] When the schedule CSV setpoint column contains the string `"No Space Heating"`, the parser maps it to -999°C and the thermostat never triggers heating mode
- [ ] When the schedule CSV setpoint column contains the string `"No Space Cooling"`, the parser maps it to +999°C and the thermostat never triggers cooling mode
- [ ] DSE < 1.0 reduces effective capacity delivered to zone proportionally
- [ ] `supply_air_temp_c` defaults match the appendix §3 table for each equipment type
- [ ] `airflow_cfm_per_ton` defaults to 375; scales by `AirflowDefectRatio` when provided
- [ ] Two `HvacHeating` instances in the same zone with independent setpoints each apply their own thermostat independently; combined thermal port contributions sum correctly at the zone level

## Physics Decisions
- `airflow_cfm_per_ton` default changed from OCHRE's 312 to 375 (ACCA Manual S minimum). Record in `docs/PHYSICS_DECISIONS.md` with rationale and energy-total impact note.
- ResStock sentinel setpoint handling (rather than silently discarding `No Space Heating`/`No Space Cooling` rows). Record in `docs/PHYSICS_DECISIONS.md`.
- Ideal-capacity mode activation at `time_res >= 5 min` or explicit `use_ideal_capacity` flag. Record in `docs/PHYSICS_DECISIONS.md`.

## Verification
- [ ] `cargo check -p hares-equipment` passes
- [ ] `cargo test -p hares-equipment` passes
- [ ] `cargo clippy -p hares-equipment -- -D warnings` passes
