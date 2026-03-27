# HARES Codebase Review — Consolidated Findings

Comprehensive review across 6 dimensions: OCHRE API parity, missing tests, missing
invariant checks, performance (allocations/clones/precomputation), test quality, and
code correctness. Findings are deduplicated and organized by priority tier.

---

## Tier 1 — Critical: Correctness Bugs & Silent Functional Gaps

### T1-1: `batch_step` RL action mapping silently discards all actions
**File:** `crates/hares-python/src/py_gym.rs:34,134`

Actions passed to `batch_step` are accepted but silently dropped (`let _ = actions.get(idx)`).
RL training loops do not influence the simulation. The `TODO(H-7)` is not visible to callers.

**Impact:** RL integration is non-functional for non-trivial policies with no error feedback.

### T1-2: `TariffEvaluator` accessor methods panic on out-of-bounds after simulation end
**File:** `crates/hares-tariff/src/evaluator.rs:198-237`

`current_price()`, `current_export_price()`, `current_period_name()`, and `tier_multiplier()`
index into arrays using `self.step_index` with no bounds check. After simulation completion,
these panic with index-out-of-bounds. The `advance()` method does not set `finished`, so
callers using `advance()` directly can leave `step_index` past the array end.

### T1-3: `GridExportRule` accepted but never enforced in BMS actor
**File:** `crates/hares-core/src/actors/bms.rs:21`

`grid_export_rule` is stored but marked `#[expect(dead_code)]`. The field is never consulted
in `evaluate_mode`. Users who configure export constraints will see incorrect behavior —
the battery will export without restriction regardless of the rule.

### T1-4: HVAC/water heater Duty Cycle control signal missing
**Files:** OCHRE `HVAC.py:266`, `WaterHeater.py:135` vs HARES `ControlSignal` enum

OCHRE's primary DR control mechanism is `Duty Cycle` — fractional on-time per timestep for
sub-minute DR dispatch. HARES `ControlSignal` has no `DutyCycle` variant. DR co-simulation
at sub-timestep resolution is not implementable.

### T1-5: `TariffEvaluator` silently drops ratchet configs for non-first demand rates
**File:** `crates/hares-tariff/src/evaluator.rs:166-169`

`find_map` returns the first demand rate's ratchet config and ignores all others. Tariffs
where only the second or later demand rate has a ratchet (common in PG&E, SCE) will have
that ratchet silently discarded, producing incorrect demand charges.

### T1-6: `finalize()` over-bills fixed charges for partial final billing periods
**File:** `crates/hares-tariff/src/evaluator.rs:360-362`

When simulation ends mid-period, `finalize()` charges `daily_usd * full_period_days` using
the scheduled end date, not the actual simulation end time. A simulation ending January 15
gets charged for all 31 days.

---

## Tier 2 — High: Hot-Loop Performance (525,600 steps/year)

### T2-1: `EnvironmentManager::update()` allocates 5+ fresh Vecs every timestep
**File:** `crates/hares-core/src/environment.rs:421-512`

Every call constructs a new `EnvironmentState` with:
- `solar_irradiance` Vec (6-10 surfaces) — sometimes allocated twice (PV path)
- `schedule_values` Vec (20-80 entries)
- `custom_domains` vec with inner Vecs
- `zones: self.zones.clone()`

At 525,600 steps this is ~7-10M alloc/free cycles per year. The returned struct replaces
`self.latest_env`, destroying all capacity from the previous allocation.

**Fix:** `update(&mut EnvironmentState)` — mutate in place, reuse Vec capacity via
`.clear()` + `.extend()`.

### T2-2: `equipment_telemetry` HashMap+String cloned every step with actors
**File:** `crates/hares-core/src/dwelling/mod.rs:1642-1648`

For each equipment: `desc.name.clone()` + `eq.telemetry().clone()` (HashMap). With 4-8
equipment and 10-20 telemetry keys each, this is ~60-160 String allocations per step.

**Fix:** Pre-build HashMap keys at init, use `clone_from` for value-only updates.

### T2-3: `format_domain_update()` allocates and sorts two Vecs every thermal solve
**File:** `crates/hares-envelope/src/thermal_solver/mod.rs:454-480`

`zone_temperatures_c` and `latent_pairs` are freshly allocated and sorted every step.
Zone ordering is fixed after init — no re-sort needed.

**Fix:** Pre-allocated scratch buffers on `ThermalSolver`, update values in place.

### T2-4: `StratifiedTank` allocates 6+ Vecs per step
**File:** `crates/hares-equipment/src/water_heater/tank.rs`

- `apply_conduction_and_standby()`: `old_temps_c.clone()` + `vec![0.0; n_nodes]`
- `apply_draw()`: `old_temps_c.clone()` + `vec![0.0; n_nodes]`
- `mix_inversions()`: 3 Vecs with `with_capacity(n_nodes)`
- `step()`: `node_temps_c.clone()`

At 6 nodes, 1-minute timesteps: ~3.15M heap allocations/year from this one component.

**Fix:** Pre-allocated scratch buffers as fields on `StratifiedTank`.

### T2-5: `StepResult` cloned unnecessarily when recording
**File:** `crates/hares-core/src/dwelling/mod.rs:1905-1928`

`step_result` is cloned to push into `simulation_results.steps` when it could be moved.

### T2-6: `thermal_update.clone()` immediately after construction
**File:** `crates/hares-core/src/dwelling/mod.rs:1831`

Cloned to upsert into `latest_env`, but the original is used for subsequent reads.
Restructure to read first, then move into `upsert_domain`.

### T2-7: `lwr_by_zone_buf.clone()` unconditionally every step
**File:** `crates/hares-envelope/src/thermal_solver/mod.rs:402`

Cloned into `EnvelopeComponentGains` every step. If callers only read by reference,
expose via borrow instead.

### T2-8: `to_rfc3339()` String allocation every step in recorder
**File:** `crates/hares-core/src/dwelling/mod.rs:2104`

525,600 String allocations/year for timestamp formatting.

### T2-9: 8 linear scans over `infiltration_buf` per step
**File:** `crates/hares-envelope/src/thermal_solver/mod.rs:338-435`

Eight separate `.iter().find()` calls to extract different fields from the same buffer
entry. Extract once, read all fields.

### T2-10: `GlazingCurve::from_u_shgc` recomputed per surface per step
**File:** `crates/hares-envelope/src/thermal_solver/solar.rs:23`

U-factor and SHGC are constant — precompute at init time.

---

## Tier 3 — High: Missing Tests for Physics Code

### T3-1: Cooling coil psychrometrics (`calculate_shr`, `coil_ao_factor`, `coil_bypass_factor`)
**File:** `crates/hares-equipment/src/hvac/coil_physics.rs:213,287,300`

Core cooling-coil SHR calculation with iterative root-finding (50-iteration convergence).
Zero unit tests. Convergence behavior at extreme conditions, non-convergence error path,
and round-trip consistency are all untested.

### T3-2: Battery degradation model (`RainflowCounter`, `DegradationState`)
**File:** `crates/hares-equipment/src/battery/degradation.rs`

ASTM E1049-85 rainflow counting, calendar aging, temperature weighting — all untested.
Only a "won't crash" smoke test exists.

### T3-3: HVAC speed staging (392 lines, zero tests)
**File:** `crates/hares-equipment/src/hvac/staging.rs`

`select_speed`, `part_load_factor`, `interpolated_capacity`, `interpolated_eir`,
`apply_startup_capacity_degradation` — all untested. These control compressor staging
decisions in every HVAC timestep.

### T3-4: Duct distribution zone heat fractions (88 lines, zero tests)
**File:** `crates/hares-equipment/src/hvac/duct_distribution.rs`

DSE heat routing with multi-zone fractions — untested. Wrong DSE cascades into all
parity tests.

### T3-5: Infiltration model — only 1 smoke test for density
**File:** `crates/hares-envelope/src/thermal_solver/infiltration.rs`

`apply_infiltration_and_ventilation` handles 5 `InfiltrationMethod` variants, duct
leakage interaction, natural ventilation. Only `density_is_dry_air_basis` is tested.

### T3-6: Humidity solver multi-zone coupling untested
**File:** `crates/hares-envelope/src/humidity_solver.rs`

All 10 inline tests are single-zone. Comment acknowledges "single-zone validated only."

### T3-7: EV driver end-to-end charging cycle untested
**File:** `tests/python/test_ev_archetypes.py:71`

Only runs 3 steps — never reaches a departure/drive/arrival/plug-in transition.

---

## Tier 4 — High: Unwired Invariant Checks

### T4-1: `tank_temperature_bounds` — defined, never called
**File:** `crates/hares-core/src/dwelling/mod.rs:2134`

`check_temperatures` is called with an empty slice for tank temps. Runaway tank node
temperatures (NaN, 200C) produce no invariant violation. The `[0, 100]C` check is dead code.

### T4-2: `soc_bounds` — defined, never called for Battery or EV
**File:** `crates/hares-core/src/dwelling/mod.rs:2108-2171`

Both equipment types write `"soc"` to telemetry but `check_soc` is never invoked.
The `.clamp()` calls silently normalize; an integration bug producing SoC=2.5 is invisible.

### T4-3: `thermal_balance` — defined, not wirable without ThermalSolver API addition
**File:** `crates/hares-core/src/dwelling/mod.rs:2108-2171`

Conservation-law check that would catch sign-flipped gains. Requires `ThermalSolver` to
expose zone thermal mass and previous-step temperatures. Design gap, not just missing call.

### T4-4: `moisture_balance` — defined, inputs available, not called
**File:** `crates/hares-core/src/dwelling/mod.rs:2150-2167`

Only `humidity_payload_finite` fires. The actual moisture conservation check
(`|dm - sum(Q_latent*dt/h_fg)| < 1e-6 kg`) is never invoked despite inputs being available.

---

## Tier 5 — Medium: OCHRE Parity Gaps

### T5-1: `RoomAC` equipment type missing
OCHRE has a `RoomAC` class (window units, `duct_dse=1`). Present in ~15% of ResStock.
HPXML files with room ACs will fail or map to wrong equipment type.

### T5-2: Resilience / islanded mode not implemented
HARES accepts `set_grid_voltage(0)` but does not re-run with all electric loads forced off
(OCHRE's grid-disconnect behavior at `Dwelling.py:248-272`).

### T5-3: Generic `Heater` and `Cooler` catch-all types missing
OCHRE's `EQUIPMENT_BY_NAME` has catch-all types for HPXML configs that don't match
a specific class.

### T5-4: Multi-zone humidity coupling unverified
`humidity_solver.rs` comment: "single-zone validated only." Simulations with garages,
attics, basements may produce incorrect humidity.

### T5-5: EVI-Pro stochastic EV schedule generation missing
OCHRE generates charging events from temperature-stratified PDFs. HARES uses fixed
archetype schedules only.

---

## Tier 6 — Medium: Test Quality Issues

### T6-1: Hardcoded temp-file paths cause parallel test collisions
**Files:** `tests/regression/smoke_test.rs:99,298`, `crates/hares-core/src/checkpoint.rs:96,120,147`,
`tests/envelope_oracle.rs:381,757`

Static filenames in `temp_dir()` with no unique suffix. Parallel `cargo test` causes
data races. The nanosecond pattern used elsewhere should be adopted consistently.

### T6-2: Smoke tests have zero numeric assertions
**File:** `tests/regression/smoke_test.rs:98,297`

Extensive `eprintln!` diagnostic output but only asserts `!SimStatus::Failed`. Will pass
regardless of physics regressions. Needs tolerance-bounded numeric assertions.

### T6-3: Stale-temp regression test uses `<=` — passes whether defect is present or not
**File:** `crates/hares-core/tests/orchestration_parity.rs:471`

### T6-4: Magic numbers in EV tests (connection_state=2.0, tolerance=0.301)
**Files:** `crates/hares-equipment/src/ev/tests.rs:113,266`

### T6-5: `thread::sleep` used to force parallelism detection
**File:** `crates/hares-fleet/src/fleet.rs:538`

Timing-dependent assertion; use a `Barrier` or channel instead.

### T6-6: Weak `>=` instead of `>` in urgency ordering assertion
**File:** `crates/hares-core/src/actors/ev_driver/departure.rs:183`

---

## Tier 7 — Low: Code Quality & Minor Issues

### T7-1: `usize::MAX` sentinel for missing zone env index
**File:** `crates/hares-core/src/dwelling/mod.rs:213-214`

Should be `Option<usize>` for compiler-enforced safety.

### T7-2: Summer months hardcoded as June-September in URDB parser
**File:** `crates/hares-tariff/src/urdb.rs:112-121`

Wrong for Arizona, Southern California utilities.

### T7-3: Python solar override type detection uses class name strings
**File:** `crates/hares-python/src/py_dwelling.rs:49-55`

Fragile `"DataFrame"`, `"NDArray"` string comparison. Use duck typing.

### T7-4: `save_postcard` panics on serialization failure
**File:** `crates/hares-equipment/src/lib.rs:167-177`

Should return `Result` for graceful checkpoint failure handling.

### T7-5: 15-minute demand window hardcoded, not per-tariff configurable
**File:** `crates/hares-tariff/src/evaluator.rs:172`

### T7-6: `InvariantChecker::new()` constructed every timestep
**File:** `crates/hares-core/src/dwelling/mod.rs:2116`

Zero-sized today but violates pre-allocation pattern. Store as a Dwelling field.

### T7-7: Pre-existing clippy failures on branch
**Files:** `crates/hares-core/src/actors/bms.rs`, `actors/ev_driver/`

6 clippy errors (unused imports/fields) that block `clippy -D warnings`.

### T7-8: `StormWatchTrigger::WeatherSignal` uses hardcoded 25.0 m/s threshold
**File:** `crates/hares-core/src/actors/bms.rs:317`

### T7-9: Zero `time_res_minutes` produces `u32::MAX` trip steps in EV driver
**File:** `crates/hares-core/src/actors/ev_driver/mod.rs:495-497`

---

## HARES-Only Advantages (No Action Required)

Features where HARES exceeds OCHRE with no equivalent in the reference:

- **Dehumidifier** equipment with biquadratic performance curves
- **ERV/HRV** with sensible/latent recovery, bypass, defrost derating
- **Battery electrochemical model** — OCV/UNeg tables, chemistry-swappable degradation
- **CHP thermal fluid ports** on generators
- **Full tariff system** — demand ratchets, net billing, gas tariffs, seasonal splits
- **Checkpoint/save-restore** with version checking
- **Rayon parallel fleet** with sample weights and progress callbacks
- **V2G/V2H** EV control with departure scheduling and solar-aligned charging
- **Battery storm watch** mode
- **Crank-Nicolson** implicit thermal solver (vs OCHRE's explicit Euler)
- **Per-timestep Perez** irradiance model (vs OCHRE's pre-computed schedules)
- **Composable EV actor** modules (departure, SOC gate, time window, price, solar, V2G/V2H)
- **Priority-tiered control dispatch** (Schedule < UserOverride < Grid < Safety)
