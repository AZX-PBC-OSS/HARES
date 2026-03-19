---
id: HARES-030
title: "hares-equipment — EV (Stochastic + Behavioral Controls)"
kind: implement
depends_on: [HARES-018]
files_to_touch:
  - crates/hares-equipment/src/ev.rs
references:
  - docs/architecture/02-equipment-and-ports.md
  - docs/architecture/03-control-interfaces.md
  - docs/architecture/07-testing-and-verification.md
verification:
  - cargo check -p hares-equipment
  - cargo test -p hares-equipment
  - cargo clippy -p hares-equipment -- -D warnings
---

## Background/Context
Electric vehicles are a significant flexible load. The v1 model covers unidirectional charging (no V2G) with schedule-driven arrival/departure and realistic charging curves. Multi-instance support allows simulating households with multiple EVs.

**Scope**: This ticket covers the **stochastic `Ev`** type, where arrival time, initial SOC at arrival, and parking duration are sampled from probability distributions at each event. A separate `ScheduledEv` type exists in OCHRE as `ScheduledEV(ScheduledLoad)` — a pre-scheduled, non-controllable EV model driven by a user-supplied schedule. `ScheduledEv` is out of scope for this ticket but should be tracked as a follow-up; it maps naturally onto the `ScheduledLoad` infrastructure from HARES-019.

This ticket is expanded to capture practical household EV behaviors needed for realistic load profiles and controller studies:
- Driver archetypes (`commuter`, `shift_worker`, `work_from_home`, `weekend_warrior`, `senior_retiree`, `school_run_family`, `single_car_shared_household`) that influence charging-event frequency and event distributions.
- Charging behavior policies (always plug in vs. plug in only when SOC is low).
- Smart charging policies (delay-after-target, TOU peak avoidance, ready-by-time target SOC).
- Cold-weather charging constraints and optional battery-heater behavior that can consume power and reduce effective SOC gain.
- Optional LUT-driven charging curves from PyBaMM-derived data (Parquet preferred, CSV fallback).
- V2L (vehicle-to-load) support for behind-the-meter load offset with SOC reserve and no grid export.

## Work to Do
- [ ] Add HPXML data interface: parse `ElectricVehicle` elements for `BatteryCapacity` (kWh), `ChargingLevel` (L1/L2), `MaxChargingPower` (kW), and schedule CSV reference; populate `EquipmentConfig` from parsed values. Support vehicle-type-and-range-to-capacity derivation for ResStock HPXML compatibility: use OCHRE's `EV_FUEL_ECONOMY` constant (approximately 325 Wh/mi = 0.325 kWh/mi for a typical EV) and the `EV_MAX_POWER` 4×3 lookup table (vehicle_number × charging_level). **NOTE**: The OCHRE source (`EV.py:37`) expression `1/325*1000` is ambiguous due to operator precedence — verify the actual value against OCHRE before implementing. The intended value is likely 325 Wh/mi (0.325 kWh/mi), not 3.08 kWh/mi. Do not transcribe the formula literally; use the verified constant with an explicit unit annotation. When HPXML provides `vehicle_type` + `range_miles` + `charging_level` instead of direct capacity/power, derive capacity as `range_miles * EV_FUEL_ECONOMY_KWH_PER_MI` and power from the lookup table.
- [ ] Define `Ev` struct implementing `Equipment` with SOC tracking (`battery_capacity_kwh`, current SOC in `[0.0, 1.0]`)
- [ ] Implement stochastic arrival/departure from schedule: probability distributions for arrival time, initial SOC at arrival, and parking duration; sample from schedule CSV data passed via `EquipmentConfig`
- [ ] Use `rand_chacha::ChaCha8Rng` as the RNG for all stochastic sampling; seed from the hierarchical ChaCha8 scheme described in `docs/architecture/07-testing-and-verification.md` (per-dwelling RNG derived from master seed + building ID)
- [ ] Add deterministic stochastic fuzzing (seed-reproducible):
  - [ ] arrival-time jitter (`arrival_fuzz_minutes`)
  - [ ] departure/duration jitter (`departure_fuzz_minutes`, plus optional shift-duration jitter)
  - [ ] sampled daily drive miles (`daily_drive_miles_mean`, `daily_drive_miles_stddev`) to modulate SOC-at-arrival
  - [ ] shift rotation (`shift_rotation_days`, `shift_on_days`) for shift-worker day-on/day-off behavior
- [ ] Implement connected/disconnected state derived from schedule; zero power output when disconnected
- [ ] Implement L1 charging with selectable current behavior (e.g. 8A vs 12A) and voltage-based power conversion; preserve 1.4 kW default compatibility
- [ ] Implement L2 charging: configurable rated power (3.6–11.5 kW)
- [ ] Implement charging curve: continuous linear power taper as SOC approaches `soc_max` (default 1.0). Formula from OCHRE `EV.py` lines 290-305: `max_charge_power = (soc_max - soc) * capacity_kwh / dt_hours / efficiency`. There is no discrete step at 0.8 SOC — power decreases linearly from rated power starting when the taper formula yields less than rated power, which depends on `capacity_kwh`, `dt_hours`, and `efficiency`. Clamp to zero when `soc >= soc_max`.
- [ ] Add optional PyBaMM LUT charging-curve path: apply LUT power-fraction vs SOC to rated charging power (Parquet loader preferred; CSV fallback acceptable). Interpolate linearly between SOC points and clamp to [0, 1].
- [ ] Add driver archetype support in config (`driver_archetype`): `commuter`, `work_from_home`, `shift_worker`, `weekend_warrior`, `senior_retiree`, `school_run_family`, `single_car_shared_household`; when explicit schedule is not provided, use archetype defaults for event-day ratio and event distributions.
  - [ ] `commuter`: consistent weekday commute timing and moderate weekday miles.
  - [ ] `work_from_home`: low weekday miles with short/local trips and occasional midday charging windows.
  - [ ] `shift_worker`: non-standard arrival/departure with optional rotation cycle and shift-duration fuzzing.
  - [ ] `weekend_warrior`: lighter weekday miles and elevated weekend trip length/frequency.
  - [ ] `senior_retiree`: daytime errands, low/moderate miles, high dwell time at home.
  - [ ] `school_run_family`: morning and afternoon trip clusters with short, frequent drives.
  - [ ] `single_car_shared_household`: broader timing spread due to multiple drivers sharing one vehicle.
- [ ] Add charging behavior policy (`plug_in_policy`): `always` and `low_soc`, with configurable `plug_in_soc_threshold`.
- [ ] Add smart charging controls:
  - [ ] `immediate_target_soc` then `delay_until_hour`
  - [ ] TOU peak-avoidance window (`tou_avoid_peak`, `tou_peak_start_hour`, `tou_peak_end_hour`)
  - [ ] ready-by target (`ready_by_hour`, `ready_target_soc`) that can override delay/TOU when needed to meet departure readiness
- [ ] Add temperature-aware charging:
  - [ ] `min_charge_temp_c` lockout and linear derate up to `full_power_temp_c`
  - [ ] optional heater (`heater_power_w`, `heater_threshold_c`) that draws electric power and slows effective SOC gain
  - [ ] simple lumped thermal model (`thermal_mass_j_per_k`, `ua_w_per_k`)
- [ ] Write Electrical port (positive active power = load) when connected and SOC < 1.0; zero when disconnected or full
- [ ] Add V2L mode (no export): when enabled, allow EV to offset local house load as negative active power subject to SOC reserve and max discharge limit. This is distinct from V2G.
- [ ] Declare `control_capabilities`: `POWER_SETPOINT | SOC_TARGET | POWER_LIMIT`
- [ ] Declare `telemetry_fields`: `soc`, `active_power_kw`, `is_connected`, `charging_level`, `time_until_departure_s`, plus behavior diagnostics (`battery_temp_c`, `heater_power_w`, `charge_derate`, `v2l_active`, `v2l_power_kw`)
- [ ] Implement `save_state() -> Vec<u8>`: serialize SOC, connected state, RNG state (ChaCha8 stream position), and current event schedule entry
- [ ] Implement `load_state(&mut self, state: &[u8]) -> Result<()>`
- [ ] Ensure each `Ev` instance owns fully independent schedule and SOC state
- [ ] Assign `ExecutionStage::Electrical` (Stage 2) in `EquipmentDescriptor`
- [ ] No V2G export support in v1; document this limitation in a code comment. V2L is allowed only as behind-the-meter load offset.
- [ ] `apply_control` must reject negative `active_power_kw` in `PowerSetpoint` with an error message "V2G not supported in v1"
- [ ] Register in `EquipmentRegistry`

## Files to Touch
- `crates/hares-equipment/src/ev.rs`: new file — `Ev` struct and full `Equipment` implementation

## Measures of Success
- [ ] SOC ramps upward during connected period at the expected rate for the configured charging level
- [ ] No power draw when disconnected
- [ ] L1 power matches configured current mode (8A, 12A, etc.) and voltage; L2 power equals configured rated power during constant-power phase
- [ ] Charging taper test: at low SOC where `(soc_max - soc) * capacity / dt / efficiency >= rated_power`, charging power equals rated power; as SOC approaches `soc_max` the linear taper formula yields a value below rated power and the taper is applied; at `soc >= soc_max` power is exactly zero. Verify the taper is continuous with no discrete step.
- [ ] Two `Ev` instances with different schedules and initial SOCs evolve independently
- [ ] Determinism: same master seed + building ID produces identical arrival/departure sequence across runs
- [ ] `save_state` / `load_state` round-trip preserves SOC, RNG state, and produces identical subsequent trajectory
- [ ] Driver archetype behavior produces materially different annual charging profiles (frequency, timing, and SOC-at-arrival distributions)
- [ ] TOU/delay/ready-by logic behaves as expected in edge cases (conflicting windows, wrap-around windows, short remaining time)
- [ ] Cold-weather lockout and heater behavior are validated (heater load visible, SOC gain reduction observed)
- [ ] V2L reduces local net load without exporting to grid and respects SOC reserve

## Notes / Future Archetype Ideas (Non-Blocking for HARES-030)
- `delivery_gig`: high daily miles, midday top-ups, frequent urgent charging.
- `multi_job_irregular`: weak weekly periodicity, broad timing variance.
- `road_tripper`: infrequent but very deep discharge events followed by recovery charging.
- Keep archetypes composable with policy modifiers (`plug_in_policy`, TOU preferences, PV-following) so behavior calibration does not require new hard-coded archetypes for every scenario.

## Verification
- [ ] `cargo check -p hares-equipment` passes
- [ ] `cargo test -p hares-equipment` passes
- [ ] `cargo clippy -p hares-equipment -- -D warnings` passes
