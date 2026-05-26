# Integration test coverage: full simulation scenarios, ResStock smoke, determinism
**Review ID**: test-02
**Category**: tests
**Date**: 2026-05-26

## Files Reviewed
- `tests/regression/mod.rs`, `helpers.rs`, `smoke_test.rs`, `determinism.rs`, `checkpoint_restart.rs`, `fleet_scale.rs`, `multi_instance.rs`
- `tests/resstock_smoke.rs`
- `crates/hares-core/tests/engine.rs`, `orchestration_parity.rs`, `core_output_regressions.rs`, `actor_telemetry_regressions.rs`, `equipment_ordering_tests.rs`, `port_accumulation_tests.rs`, `dispatch_ordering_regressions.rs`, `alignment_oracles_regressions.rs`, `named_control_wiring_regressions.rs`, `zone_state_ordering.rs`, `weather_integration.rs`, `timezone_weather_regressions.rs`, `window_u_factor_shgc_silent_defaults.rs`, `occupant_count_silent_zero.rs`, `port_radiant_diag.rs`, `actor_diagnostic_csv.rs`, `ct_boiler_diag.rs`, `resstock_observer_diag.rs`, `resstock_smoke.rs`, `integration.rs`

## Vendor/Reference Files Consulted
- `vendors/OCHRE/test/test_dwelling/test_dwelling.py` — OCHRE reference dwelling integration test (uses `BEopt_example.xml`, tests init/update/simulate lifecycle with 96-step 1-day simulation at 15-min resolution, control signal injection, timing assertion < 5s; does NOT test checkpoint/restart, determinism, or fleet)
- `vendors/OCHRE/test/test_dwelling/test_psychrometrics.py` — stub only (empty test class)
- `vendors/OCHRE/test/test_dwelling/run_dwelling_timing.py` — profiling script, not a test

## Findings

### Finding 1: Checkpoint/restart only verifies a single scalar metric, not full state fidelity
**Severity**: critical

**Description**: The checkpoint/restart test in `checkpoint_restart.rs:110–125` compares only `net_electric_power_kw` between the reference and restarted runs using a 1e-9 tolerance. It does not verify that zone temperatures, equipment states (operating modes, thermostat states, thermal accumulators), schedule position, RNG state, or any other internal state field is correctly serialized and restored. A bug that corrupts zone temperature but preserves net power would pass this test undetected.

**Code Location**: `tests/regression/checkpoint_restart.rs:110–125`

**Root Cause**: The comparison loop at lines 110–125 iterates over `StepResult` structs but examines only `net_electric_power_kw`. The `StepResult` struct contains `zone_temperatures_c` (BTreeMap), `hvac_heating_w`, `hvac_cooling_w`, `timestamp`, and other fields — none of which are compared.

**Impact**: A regression in equipment-state serialization (e.g., a missing `#[serde]` attribute on a new field, an RNG not being reseeded from checkpoint, or thermal accumulators resetting to defaults on load) would go undetected. The synthetic TOML dwelling used for this test has a single electric furnace with minimal state complexity, making the test even less sensitive to real-world serialization bugs.

**Comparison with vendor**: OCHRE has no checkpoint/restart functionality at all, so HARES's test is better than nothing — but only comparing a single scalar makes it a false reassurance.

### Finding 2: Determinism test compares aggregated per-end-use totals, not per-timestep trajectories
**Severity**: high

**Description**: The determinism test in `determinism.rs:38–47` collects `annual_energy_kwh.per_end_use` values, sorts them into a vector, and compares those sorted vectors across thread counts (1, 4, 8). It does NOT compare per-timestep power, zone temperature, or equipment state trajectories. Different thread counts could produce different timestep-by-timestep outputs (e.g., different equipment cycle timing, different zone temp paths) that coincidentally sum to the same per-end-use totals. This is a particularly risky gap because thread-count non-determinism in Rust often manifests as floating-point accumulation order differences (non-associativity of addition across rayon threads), which could produce different aggregated totals — but if the test passes, it masks trajectory-level divergence.

**Code Location**: `tests/regression/determinism.rs:38–47`

**Root Cause**: The test at lines 38–47 iterates `result.metrics.annual_energy_kwh.per_end_use.values()` and sorts the values. The comment at line 66 says `#13 verify determinism (same seed, different threads → identical outputs)`, but the check is metric-level only. No per-step timeseries CSV comparison occurs — there is no CSV output from this test at all (`output_verbosity: 0` in the config builders at `helpers.rs:80–91`).

**Impact**: A concurrency bug that scrambles equipment dispatch order across threads (producing different interleaving of thermal contributions per step but the same total energy) would pass this test and only surface in production fleet runs.

### Finding 3: Missing `aggregation_check.rs` module causes compilation failure in the regression suite
**Severity**: high

**Description**: `tests/regression/mod.rs:16` declares `mod aggregation_check;` and line 77 calls `aggregation_check::run_aggregation_check()`, but the file `tests/regression/aggregation_check.rs` does not exist. No file matching `tests/regression/aggregation_check*` was found by glob search.

**Code Location**: `tests/regression/mod.rs:16` and `tests/regression/mod.rs:77`

**Root Cause**: The `aggregation_check` module was referenced during development but the file was either never committed or was deleted without updating `mod.rs`.

**Impact**: `cargo test --test regression -- --ignored` (the advertised entry point for the regression suite) will fail to compile. The severity is high because it breaks the advertised CI gate and masks the fact that weighted-end-use aggregation — a critical metric computation — has no test coverage.

### Finding 4: Fleet scale test uses only identical synthetic dwellings with no heterogeneous configuration
**Severity**: high

**Description**: The fleet scale test in `fleet_scale.rs:17–27` creates 1000 identical synthetic dwelling configs, all using the same `build_dwelling_config()` helper with incrementing building IDs. All dwellings have identical synthetic schedule/weather, identical geometry, and — since `build_dwelling_config` from `helpers.rs` uses the real OCHRE HPXML — all have the same BEopt building. The test does not exercise heterogeneous dwellings (different climate zones, building types, equipment mixes) in the same fleet run. This means the test cannot detect bugs where Fleet fails on cross-dwelling interaction, climate-zone-specific logic, or mixed equipment-type dispatch.

**Code Location**: `tests/regression/fleet_scale.rs:17–27`

**Root Cause**: The loop at lines 17–27 creates configs with incrementing `idx as i64 + 1` for `bldg_id` but all other parameters are identical. There is no mechanism to vary the HPXML path, schedule, weather, or equipment configuration.

**Impact**: A bug where Fleet's parallel dispatch corrupts state when dwellings have different climate zones, schedule lengths, or equipment counts would pass this test. The test only validates that 1000 identical buildings don't OOM; it provides no multi-configuration regression protection.

### Finding 5: Checkpoint/restart tested only on synthetic TOML, not on real HPXML fixtures
**Severity**: high

**Description**: The checkpoint/restart test (`checkpoint_restart.rs:13–16`) uses `helpers::write_schedule_csv()` and `helpers::write_weather_epw()` — synthetic stubs — paired with `helpers::build_dwelling_config()` which uses the BEopt HPXML but with fixed-constant synthetic schedules. No checkpoint test exists for real HPXML fixtures (BEopt with real schedule, ResStock buildings, or parity fixtures with HVAC+WH+PV+battery combinations). Real HPXML buildings have much richer equipment state (ASHP heat pump with defrost cycles, gas furnace with multi-stage burners, water heaters with tank stratification) whose serialization is far more complex than the simplified synthetic path.

**Code Location**: `tests/regression/checkpoint_restart.rs:13–16`

**Root Cause**: The checkpoint test was designed to be fast and self-contained, avoiding external fixture dependencies. But this means it exercises a minimally-complex state graph.

**Impact**: A serialization bug in any equipment type beyond the basic electric furnace used in synthetic TOML (e.g., heat pump state, water heater tank layers, battery SOC tracking, PV inverter state) would only surface in production checkpoint/restart scenarios.

### Finding 6: Multi-instance test has zero equipment instances
**Severity**: medium

**Description**: The `multi_instance.rs` test writes a synthetic TOML config (`multi_instance.rs:52–84`) that contains a basic building geometry and weather but NO `[hvac]`, `[water_heater]`, `[pv]`, or `[battery]` sections. The test name and module-level comment ("multiple equipment instances of the same type in one dwelling must behave independently") imply equipment testing, but the actual config has zero equipment. The test only verifies that `simulate()` produces non-zero step count (`multi_instance.rs:29`).

**Code Location**: `tests/regression/multi_instance.rs:52–84`

**Root Cause**: The TOML config template at lines 52–84 has `[geometry]`, `[materials]`, `[weather]`, `[schedule]`, and `[output]` sections but omits all equipment sections.

**Impact**: The test provides zero coverage of multi-instance equipment behavior. Any bug where two heat pumps, two water heaters, or two batteries in the same dwelling fail to operate independently (e.g., sharing mutable state, cross-talking through ports, dispatch conflicts) will not be caught.

### Finding 7: No integration test exercises the complete off→on→cycling→off state machine for several equipment types
**Severity**: medium

**Description**: The review instruction requires that "every equipment type — heat pump, AC, furnace, electric resistance, gas water heater, heat pump water heater, PV, battery, EV — appears in at least one integration test that exercises its full state machine (off → on → cycling → off)." The following equipment types have gaps:

| Equipment Type | Fire/HVAC fixture exists | Dedicated integration test exercising full state machine |
|---|---|---|
| Heat pump (ASHP) | `cz4a_ashp_hpwh` | Partial — alignment oracles test timing/peak but no dedicated off→on→cycling→off test |
| AC (Air Conditioner) | Implicit in ResStock bldg 4 | Indirect — `summer_72h` test asserts indoor temp bounds and peak power (< 20 kW) but never explicitly asserts `hvac_cooling_w > 0` or cooling mode activation |
| Furnace (gas) | `cz2a_gas_furnace_ac_res_wh` | Yes — `orchestration_parity.rs:213–244` exercises gas furnace telemetry |
| Furnace (electric) | Via synthetic TOML | Yes — `orchestration_parity.rs:175–203` exercises electric furnace |
| Electric resistance | `cz6b_resistance_res_wh` | No — fixture exists but no dedicated integration test references it |
| Gas water heater | `cz2a_gas_furnace_ac_res_wh` | No — no test explicitly asserts gas WH cycling or state transitions |
| Heat pump water heater | `cz4a_ashp_hpwh` | No — only tested implicitly via alignment oracles |
| PV | `cz4a_pv_only`, `cz4a_pv_battery` | No dedicated test for PV-only generation cycle |
| Battery | `cz4a_battery_only` | Yes — `core_output_regressions.rs:174` exercises battery telemetry and charge/discharge |
| EV | `cz5a_ev_only`, `cz2a_pv_ev` | No — no dedicated integration test exercises EV charge/discharge cycle |

**Code Location**: Parity fixtures exist at `tests/fixtures/parity/cz*/` but integration tests in `crates/hares-core/tests/` only reference `cz4a_battery_only`, `cz2a_gas_furnace_ac_res_wh`, `cz4a_ashp_hpwh`, and `cz5a_minisplit_gas_wh` in `alignment_oracles_regressions.rs` and `core_output_regressions.rs`. Other fixtures (`cz4a_pv_only`, `cz4a_pv_battery`, `cz2a_pv_ev`, `cz5a_ev_only`, `cz6b_pv_battery_ev`, `cz6b_resistance_res_wh`) are present but unused by any integration test.

**Root Cause**: The parity fixtures were created for alignment oracle comparison against OCHRE reference data, but the alignment oracle tests (`alignment_oracles_regressions.rs`) only reference ASHP and minisplit fixtures. The battery fixture is tested in `core_output_regressions.rs`, but the PV, EV, electric resistance, and PV+battery+EV combo fixtures have no integration tests that step through their full operational lifecycle.

**Impact**: A regression in PV curtailment logic, EV charge scheduling, electric resistance heater cycling, or combined PV+battery+EV dispatch can only be caught by fleet-scale or production runs, not by CI.

### Finding 8: Seasonal variation tests assert generic bounds but not seasonal performance differences
**Severity**: medium

**Description**: The ResStock smoke tests (`crates/hares-core/tests/resstock_smoke.rs:248–274`) include `summer_72h` (July 15, bldg 4 Texas) and `winter_72h` (Jan 15, bldg 2 Idaho) tests, but these tests assert only generic physics bounds: indoor temp in range, peak power < 20 kW, and broad energy ranges ([5, 400] kWh summer, [5, 600] kWh winter). The tests do NOT assert that summer energy > winter energy (which would verify that HVAC responds differently to seasonal weather), that cooling dominates in summer while heating dominates in winter, or that the same building in different seasons produces measurably different output. The summer and winter tests use *different buildings* (bldg 4 vs bldg 2), making it impossible to isolate seasonal effects from building effects.

**Code Location**: `crates/hares-core/tests/resstock_smoke.rs:248–274`

**Root Cause**: The tests at lines 248–260 and 262–274 select different buildings for each season (bldg 4 Texas for summer, bldg 2 Idaho for winter) rather than running the *same* building in both seasons.

**Impact**: A regression that causes weather seasonal variation to be ignored (e.g., all simulations use the same constant weather regardless of date) would not be caught because different buildings are used, and no test compares same-building summer-vs-winter outputs.

### Finding 9: Determinism test uses coarse per-end-use metric comparison rather than on-disk output comparison
**Severity**: low

**Description**: The determinism test at `determinism.rs:38–47` collects per-end-use metrics into `Vec<f64>` and sorts them before comparison. This tolerates two different bugs: (a) if the output CSV writer produces different column ordering or values under different thread counts, the test won't catch it (CSV output is disabled with `output_verbosity: 0`); (b) sorting the metrics before comparison means if one thread count misattributes energy to the wrong end-use bucket (but the sorted multiset of bucket values remains the same), the test passes. The test's only effective check is that the *set* of per-end-use energy values is the same.

**Code Location**: `tests/regression/determinism.rs:38–47` and `tests/regression/helpers.rs:86–87`

**Root Cause**: The test sorts per-end-use values at line 46 (`trajectory.sort_by(|a, b| a.total_cmp(b))`) before comparison. The test configs have `output_verbosity: 0` and `write_output: false` (default), so no CSV comparison is possible.

**Impact**: An end-use bucket swap bug (e.g., HVAC heating energy reported as water heater energy) would not be detected if both buckets happen to have different values that sort identically. This is low severity because a bucket-swap would typically change the multiset of values.

### Finding 10: Checkpoint/restart tested with fixed 30-step checkpoint, not at equipment state boundaries
**Severity**: low

**Description**: The checkpoint is taken at a hardcoded step 30 (`checkpoint_restart.rs:9`) regardless of equipment state. This means the checkpoint may be taken while equipment is mid-cycle (furnace partially through a heating burst, thermostat mid-deadband, etc.), which is useful for testing serialization of transient states. However, it also means the test never explicitly checks that checkpointing works correctly when the furnace is "just about to fire" (zone temp near setpoint threshold) versus "at steady on-state" versus "just turned off." The behavior could differ across these states.

**Code Location**: `tests/regression/checkpoint_restart.rs:9` and `tests/regression/checkpoint_restart.rs:56–62`

**Root Cause**: The hardcoded `CHECKPOINT_AT_STEP: u64 = 30` does not adapt to equipment state or zone temperature conditions.

**Impact**: A serialization bug that only manifests when the furnace is in a specific transient state (e.g., lockout timer active, defrost mode mid-cycle) would not be caught unless step 30 happens to land in that state. Low severity because the synthetic TOML furnace state space is simple.

### Finding 11: No integration test for full-year continuous simulation
**Severity**: low

**Description**: The longest integration test runs 72 hours (`resstock_smoke.rs:248–274`). There is no test that runs a continuous full-year (8760-hour) simulation and validates annual energy totals, seasonal pattern correctness, or long-term solver stability. While full-year runs would be expensive for CI, a single smoke test could run at coarse resolution (e.g., 1-hour timestep for a full year) to catch cumulative drift, resource leaks, or end-of-year index wraparound bugs.

**Code Location**: No test in `tests/regression/`, `crates/hares-core/tests/`, or `tests/` runs a full-year simulation.

**Root Cause**: Full-year simulations are computationally expensive and were likely deferred in favor of shorter CI-friendly tests. OCHRE's reference test also runs only 1 day (`dwelling_args["duration"] = dt.timedelta(days=1)` at `test_dwelling.py:14`).

**Impact**: An accumulator overflow, memory leak, or weather/schedule index wraparound bug that only manifests after thousands of simulation steps would only surface in production fleet runs. The leap-year test in `weather_integration.rs` tests weather indexing in isolation but not through a full Dwelling simulation.

## Summary
- **Total findings**: 11
- **Critical**: 1 (Finding 1: checkpoint/restart validates only single scalar)
- **High**: 4 (Findings 2–5: determinism granularity, missing aggregation_check.rs, homogeneous fleet, checkpoint only on synthetic TOML)
- **Medium**: 4 (Findings 6–9: empty multi-instance test, equipment state-machine gaps, seasonal test design, missing full-year test)
- **Low**: 2 (Findings 10–11: hardcoded checkpoint step, no full-year simulation)

### Strengths relative to OCHRE vendor reference
HARES has significant coverage that OCHRE lacks entirely:
- **Checkpoint/restart** — OCHRE has no save/load mechanism; HARES's test is incomplete but the feature exists
- **Deterministic replay** — OCHRE tests no seed-based determinism; HARES verifies it at metric level
- **Fleet simulation** — OCHRE tests only single dwellings; HARES tests 1000-dwelling fleet
- **Control signal injection** — `named_control_wiring_regressions.rs` and `dispatch_ordering_regressions.rs` test 30+ control signal variants vs. OCHRE's ~4
- **HELICS co-simulation** — present in HARES Python test suite, not in OCHRE

### Areas where OCHRE has better coverage
- **Per-equipment unit tests** — OCHRE has 11 dedicated equipment test files (hvac, waterheater, battery, pv, ev, generator, scheduled loads, event-based loads, wet appliances, etc.) with isolated fixture injection. HARES tests most equipment only implicitly through full-dwelling integration.
- **Gas equipment suite** — OCHRE tests GasFurnace, GasBoiler, GasWaterHeater, GasTanklessWaterHeater, GasGenerator, and GasFuelCell individually. HARES has no dedicated gas equipment tests.
- **Water heater variants** — OCHRE tests HPWH 12-node tank model, tankless WH, and gas tankless WH. HARES tests none of these explicitly.
- **Wet appliances** — OCHRE tests Clothes Washer, Clothes Dryer, and Dishwasher with 837 lines of event-based test code. HARES has no equivalent.
- **Battery degradation/thermal model** — OCHRE tests these. HARES does not.
- **ZIP model** — OCHRE tests grid voltage sensitivity. HARES does not.

## Recommendations

1. **Expand checkpoint/restart comparison beyond `net_electric_power_kw`** (Finding 1, critical). Compare zone temperatures, equipment operating modes, thermal accumulators, schedule position, and RNG state between reference and restarted runs. Add a checkpoint test on a real HPXML fixture with multiple equipment types (e.g., `cz4a_ashp_hpwh`) to exercise complex state serialization.

2. **Add per-timestep trajectory comparison to determinism test** (Finding 2, high). Write output CSVs from each thread count run and compare them byte-by-byte, or at minimum compare per-timestep values for total power, zone temperature, and key equipment columns.

3. **Either create `tests/regression/aggregation_check.rs` or remove the module declaration** (Finding 3, high). If the weighted-aggregation logic is tested elsewhere, remove the reference from `mod.rs`. If not, implement it.

4. **Add heterogeneous fleet testing** (Finding 4, high). Create a fleet test with mixed dwellings using different parity fixtures (e.g., mix `cz4a_ashp_hpwh`, `cz2a_gas_furnace_ac_res_wh`, `cz6b_pv_battery_ev`) and verify correct per-dwelling outcomes in the same run.

5. **Add checkpoint/restart test on real HPXML fixtures** (Finding 5, high). Use at least one parity fixture (e.g., `cz4a_ashp_hpwh`) with real schedule and weather data, checkpoint at mid-simulation, restore, and verify identical continuation.

6. **Add equipment instances to the multi-instance test** (Finding 6, medium). Create a TOML dwelling with two furnaces, two water heaters, or two PV arrays and assert that they produce independent outputs.

7. **Add dedicated integration tests exercising the full state machine for each equipment type** (Finding 7, medium). Prioritize:
   - Electric resistance water heater (use `cz6b_resistance_res_wh` fixture, assert cycling behavior)
   - PV-only generation (use `cz4a_pv_only`, assert diurnal generation curve)
   - EV charge/discharge (use `cz5a_ev_only`, assert SOC evolution)
   - Heat pump water heater (use `cz4a_ashp_hpwh`, assert HPWH cycling independently of ASHP)
   - Gas water heater (use `cz2a_gas_furnace_ac_res_wh`, assert gas WH fuel consumption)
   - Combined PV+battery+EV (use `cz6b_pv_battery_ev`, assert coordinated dispatch)

8. **Fix seasonal variation test to use the same building in both seasons** (Finding 8, medium). Run one ResStock building for 72h in January and 72h in July and assert that summer cooling energy exceeds winter cooling energy (and vice versa for heating).

9. **Add a coarse-resolution full-year smoke test** (Finding 11, low). Run one ResStock building for 8760 hours at 1-hour timestep resolution and assert that annual totals are finite, non-negative, and within broad physical plausibility bounds.

10. **Add per-equipment unit tests following OCHRE's bottom-up test pyramid** (Findings 7–10, medium–low). Create isolated equipment tests with synthetic schedules, envelopes, and weather to exercise each equipment class's init → update → control → results lifecycle. OCHRE's `test/test_equipment/` directory structure (one file per equipment class) provides a good template.

## References / Citations
- `tests/regression/checkpoint_restart.rs:110–125` — single-scalar checkpoint comparison
- `tests/regression/determinism.rs:38–47` — per-end-use metric-only determinism
- `tests/regression/mod.rs:16,77` — missing `aggregation_check` module
- `tests/regression/fleet_scale.rs:17–27` — homogeneous fleet configs
- `tests/regression/multi_instance.rs:52–84` — equipment-less TOML config
- `tests/regression/smoke_test.rs:327–446` — BEopt 1h smoke (ASHP heater)
- `tests/regression/smoke_test.rs:454–632` — ResStock 1h smoke (gas furnace + gas WH)
- `crates/hares-core/tests/orchestration_parity.rs:117–164` — zone temperature evolution test
- `crates/hares-core/tests/orchestration_parity.rs:175–203` — electric furnace power test
- `crates/hares-core/tests/orchestration_parity.rs:213–244` — gas furnace telemetry test
- `crates/hares-core/tests/core_output_regressions.rs:174` — battery core output test
- `crates/hares-core/tests/resstock_smoke.rs:248–274` — seasonal 72h tests
- `crates/hares-core/tests/alignment_oracles_regressions.rs:186,372` — ASHP and minisplit parity tests
- `crates/hares-core/tests/alignment_oracles_regressions.rs:115–116` — battery parity (pending fixture)
- `tests/fixtures/parity/cz*/` — unused parity fixtures (pv, ev, resistance, pv+battery, pv+battery+ev)
- `vendors/OCHRE/test/test_dwelling/test_dwelling.py:41–111` — OCHRE dwelling integration test (DwellingTestCase)
- `vendors/OCHRE/test/test_dwelling/test_dwelling.py:130–194` — OCHRE dwelling with equipment (DwellingWithEquipmentTestCase)
