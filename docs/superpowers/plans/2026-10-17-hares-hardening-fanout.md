# HARES Hardening Fan-out Implementation Plan (post-I-08)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the verified physics, grid, schedule and API defects found by the six-domain post-I-08 review, harden every silent fallback into an error or counted event, and ship the architecture and validation work that makes HARES a worthy successor to OCHRE and peer of EnergyPlus.

**Architecture:** Six read-only review agents (core physics, envelope, equipment, grid/EV/PV, schedules/weather, architecture/Python) produced ~60 verified findings, each with file:line evidence. This plan organizes them into five waves of independent work packages; packages within a wave touch disjoint files and can be dispatched to parallel subagents. `crates/hares-core/src/dwelling/mod.rs` is a shared conflict domain, so all packages touching it are serialized inside their wave.

**Tech Stack:** Rust workspace (10 crates, ~505k lines), PyO3/Maturin Python bindings (`hares-py`), pytest surface, frame-golden fixtures for physics verification, criterion benches.

**Spec:** The review findings are the spec. Each finding below carries its evidence citation; the six full review reports are the authoritative detail. This plan argues from those reports.

## Global Constraints (apply to every task)

- **Golden gate:** every behavior-affecting commit runs `frame_golden compare-all`; any fixture difference is explained in the commit body and re-captured deliberately. Physics fixes are *expected* to shift goldens - the shift IS the fix.
- **One defect, one commit**, message states the defect class, not a ticket ID.
- **No silent fallback:** a path that was silent becomes a typed error naming the object, or a counted event in `RunHealth`/component diagnostics. Never log-and-continue.
- **Slop rule (I-08 SL-3):** no em dashes, no ledger/ticket/review references in shipped code; every touched comment states the contract in its own words. Constants are imported from `hares-physics::constants`, never redefined locally.
- **Config rule (I-08 OD-2):** unknown keys are errors; an accepted override is applied.
- Gates per commit: `cargo clippy -- -D warnings`, `cargo fmt --check`, `cargo test`, `uv run pytest -m "not slow"`.
- Rust 1.89+, no new dependencies without stated justification.

## Review Focus (input classes most likely to bite a user; each pinned by its owning task)

1. **Run starting mid-year (e.g. Jul 1) with generated (no-CSV) schedules** - occupancy/plug/lighting must read the July profile, not a rotated January one. Pinned by Task 1.3's test.
2. **EV at its V2G reserve floor with a latched export setpoint during an outage** - the bus must stay dead; no island powered by nothing. Pinned by Task 1.4's test.
3. **NaN or negative grid-voltage override** - must be rejected at the setter, never silently un-gate every load. Pinned by Task 1.6's test.
4. **Sub-second or non-integral-second timestep** - construction must fail naming the resolution, never silently run zero warm-up days. Pinned by Task 2.1's test.
5. **Tariff TOU schedule with a coverage gap** - `validate()` must reject it; electricity is never silently free. Pinned by Task 1.7's test.

---

## Findings summary (from the six reviews)

- **Core physics:** thermal-balance invariant silently skipped when node capacitances are empty; tolerated equipment-step rollback keeps committed state but discards its energy; control-signal validation is debug-only and the external `apply_control` path bypasses even that; warm-up day arithmetic truncates and can silently yield zero warm-up days; natural-ventilation is an undocumented E+/OCHRE hybrid with a structurally different discharge coefficient; the moisture invariant re-derives solver algebra (second implementation).
- **Envelope:** precomputed same-zone boundary wires every resistor one index off (dynamics off by ~25% on a 4-layer probe; masked today because shipped LUT rows are single-layer); zone/layer node-ID collision at n_zones >= 1000 silently overwrites capacitance; exterior skin iteration converges by warm-start accretion at 60 s steps with silent non-converged acceptance; window ground-reflected irradiance doc/code divergence; `MIN_CAPACITANCE_J_K` floor silently inflates thin layers; two latent unbounded-index ceilings (99 ground depths).
- **Equipment:** defrost penalty double-counted (continuous and discrete FSM both apply, compounding ~50% capacity loss at tf=0.24); gas boiler deposits the entire flue loss into the zone (~17% free heat for non-condensing); MSHP pan heater gated on compressor operation (diverges from OCHRE, under-counts winter standby); discrete resistive defrost delivers 100% of element heat to zone, continuous path 0%; ER soft-lockout hardwired 2x hard; per-step `std::env::var` in the ASHP hot path; stage EIR count mismatch silently clamped; COP telemetry inflates during ER backup.
- **Grid/EV/PV/battery:** EV can form an island while delivering zero watts (availability keyed on commanded flag, not capability); islanded EV charging bypasses the surplus clamp (phantom energy credited to the pack); tariff import price silently falls back to $0/kWh on coverage gaps; the EV TOU rate is dead code at the dwelling integration; no voltage/frequency-dependent DER trip physics (brownouts invisible); `set_grid_voltage` accepts NaN which silently un-gates every load; V2G votes priced off import price with silent $0 fallback.
- **Schedules/actors/weather:** generated schedules rotated by the run's start day-of-year (whole schedule lands on the wrong month/weekday for mid-year starts); warmup carries stochastic schedule/actor state into production (only the dwelling RNG is restored; the Occupant actor indexes presence by a private step counter); EV-driver stochastic sources excluded from checkpoint/reseed (gym seed contract broken); setpoint columns read as Celsius though the OS-HPXML convention they follow is Fahrenheit; EV driver substitutes fabricated defaults on schedule read failure; shared RNG keystreams (day-roll RNG and miles source built from the identical key; noisy TimeWindows default to the all-zero seed); fleet hardcodes `master_seed: 0`; DR compliance re-rolls per step instead of latching per event; event-start probability not normalized by step length.
- **Architecture/Python:** `hares-io` is three crates in one (parsers + full output/analytics stack + the engine's config type; arrow/parquet linked into every consumer); `Dwelling` god object (mod.rs is 23,885 lines; owns recording, warm-up, dispatch, LUTs, profiling); fatal-error classing across PyO3 by string-prefix sniffing; batch path rebuilds per-step Python strings/dicts the single path already caches; gym telemetry dict built every step even with the default reward; `Vec<Vec<f64>>` action crossing; fleet Rayon pool rebuilt per call; unbounded in-memory Arrow report; `zip.rs` physics model living in the `hares-types` leaf.

---

## Wave 1 - Critical correctness (parallel packages; P0 physics first)

### Task 1.1: Defrost model consolidation (equipment C1 + M3)

**Files:**
- Modify: `crates/hares-equipment/src/hvac/heat_pump/heater.rs` (1927-2036, 2095-2115)
- Modify: `crates/hares-equipment/src/hvac/heat_pump/defrost.rs` (229-233 claim, 300-302, 560-565)
- Test: `crates/hares-equipment/tests/hvac_parity.rs` (new accumulated-energy test)

**Interfaces:**
- Produces: one defrost code path selected by `DefrostConfig`; `evaluate_defrost` remains the frost accumulator input; its capacity/power multipliers are applied exactly once per step.
- Consumes: existing tracker states (`Accumulating`, `Defrosting`) unchanged.

- [ ] **Step 1: Write the failing test** - N hours at 0-2 C / RH 85% with the discrete tracker enabled: total delivered heating energy equals the continuous-model-alone run within 1%, and total defrost electric equals the continuous model's `extra_power_w` integral within 1% (today the composed run delivers ~25% less heat and meters ~2x defrost energy).
- [ ] **Step 2:** `cargo test -p hares-equipment defrost` - expect FAIL (composed run diverges).
- [ ] **Step 3: Implement** - when the discrete tracker is enabled, `evaluate_defrost` feeds only the frost accumulator; capacity/power multipliers come from the FSM. Decide the resistive-defrost destination once (element heats the outdoor coil: configurable fraction to zone, default 0, remainder is frost-melt loss) and share it across both paths.
- [ ] **Step 4:** tests pass; `frame_golden compare-all`, explain and re-capture cold-climate fixture diffs.
- [ ] **Step 5: Commit** - `fix: the defrost penalty is applied by one model, not stacked`

### Task 1.2: Boiler flue loss split (equipment M2)

**Files:**
- Modify: `crates/hares-equipment/src/hvac/boiler.rs` (1009-1023, 738)
- Modify: `crates/hares-equipment/src/water_heater/gas.rs` (608-617) - extract the shared helper
- Create: `crates/hares-equipment/src/combustion.rs` - one `combustion_loss_split(fuel_input, useful, flue_fraction, skin_fraction) -> (zone_w, outdoor_w)` used by boiler, gas WH, gas furnace
- Test: `crates/hares-equipment/tests/` - boiler jacket-vs-flue destination test (zone gets only `skin_loss_fraction` of the loss; the ASHRAE-derived reference pattern `ashrae_rc_reference.json` is the model)

- [ ] **Step 1:** failing test: non-condensing boiler (EIR = 1/AFUE) at steady state - zone gain from jacket is `skin_fraction * loss`, not 100% of loss.
- [ ] **Step 2:** FAIL. **Step 3:** implement the shared module + boiler rewiring. **Step 4:** PASS + frame_golden (heating fixtures shift; explain). **Step 5:** commit `fix: the boiler routes flue loss outdoors, jacket loss to the zone`

### Task 1.3: Generated-schedule calendar anchoring (schedules C1)

**Files:**
- Modify: `crates/hares-io/src/inputs.rs` (1119-1124), `crates/hares-io/src/blueprint.rs` (353-357)
- Test: schedule-alignment test - generated schedule, mid-year start; value at step 0 equals the profile evaluated at the same calendar date; leap-year case; Dec 31 -> Jan 1 wrap

- [ ] **Step 1:** failing test as above (today a Jul 1 start reads the Jan 1 row at step 0).
- [ ] **Step 2:** FAIL. **Step 3:** generate schedules anchored to Jan 1 of the start year (matching the CSV path's 2007-anchored calendar) for `duration + start_ordinal` rows.
- [ ] **Step 4:** PASS + frame_golden (mid-year fixtures shift; explain). **Step 5:** commit `fix: generated schedules are addressed by calendar date like the CSV path`

### Task 1.4: EV island capability + islanded charging clamp (grid C1 + M1)

**Files:**
- Modify: `crates/hares-equipment/src/ev/mod.rs` (1754-1764, 1802-1808)
- Test: `crates/hares-core/tests/grid_outage_islanding.rs` - EV at its discharge floor with a latched negative setpoint during an outage: bus stays dead; and a battery+EV+PV island property test: `dE_pack <= E_sources` every step

- [ ] **Step 1:** failing tests as above. **Step 2:** FAIL. **Step 3:** `island_source_available` requires delivered export (`v2l_power_kw > 0.0`, SOC above the effective discharge floor, DR fraction > 0), mirroring `battery/mod.rs:1159-1176`; islanded (bus energized) EV charging is clamped to the visible Stage-1 surplus like the battery.
- [ ] **Step 4:** PASS + frame_golden. **Step 5:** commit `fix: the EV forms an island only when it can deliver power, and islanded charging pays the surplus clamp`

### Task 1.5: Warmup stochastic-state neutrality (schedules C2 + M1 + M6)

**Files:**
- Modify: `crates/hares-core/src/dwelling/mod.rs` (3383-3419 snapshot region)
- Modify: `crates/hares-core/src/actors/occupant.rs` (132-133, 274-311 - presence indexed by clock, not a private counter)
- Modify: `crates/hares-core/src/actors/ev_driver/mod.rs` (1618-1652 - capture the four schedule sources; 1269-1274 - reseed rebuilds them)
- Modify: `crates/hares-equipment/src/water_heater/{gas.rs,tankless.rs,indirect_tank.rs,heat_pump_wh.rs}` - add `Option<ScheduleSourceState>` to snapshots (helpers exist: `schedule_helpers.rs:32-52`)
- Test: warmup converged at day 2 vs day 4 (forced via tolerance) produces identical production telemetry; gym-style `reset(seed)` twice on an EV-archetype dwelling produces bit-identical episodes

- [ ] **Step 1:** failing test (warmup-day-count invariance). **Step 2:** FAIL. **Step 3:** after warmup, `reset()` every `ScheduleSource`, reset event-load `StartDraws`, reset actor step counters; rewire Occupant presence to the environment clock; extend EV-driver and WH snapshots with schedule-source state; make `reseed` re-derive every source from the dwelling stream.
- [ ] **Step 4:** PASS + frame_golden. **Step 5:** commit `fix: warmup restores every stochastic stream, not only the dwelling rng`

### Task 1.6: Grid-voltage override validation (grid M5)

**Files:**
- Modify: `crates/hares-core/src/dwelling/mod.rs` (4991-4998), `crates/hares-types/src/environment.rs` (1149-1151, 424-426)
- Test: NaN / negative / 1.8 pu overrides rejected with `HaresError`; `bus_energized()` NaN-robust (`> 0.0`)

- [ ] **Step 1:** failing test. **Step 2:** FAIL. **Step 3:** accept exactly `0.0` or a finite value in `[0.5, 1.5]` pu (the HELICS contract); typed error otherwise. **Step 4:** PASS. **Step 5:** commit `fix: the grid voltage override is validated at the setter`

### Task 1.7: Tariff coverage validation + EV rate wiring (grid M2 + M3)

**Files:**
- Modify: `crates/hares-tariff/src/types.rs` (356-448 - validate period coverage and 24x7 schedule coverage per season)
- Modify: `crates/hares-core/src/dwelling/mod.rs` (7991 - pass the summed EV signed flow, computed at 8007-8008, into `evaluator.step`)
- Test: TOU hour with no matching period fails at `validate()`; a configured `ev_tou_period_name` changes the dwelling-level bill

- [ ] **Step 1:** failing tests. **Step 2:** FAIL. **Step 3:** implement. **Step 4:** PASS + frame_golden. **Step 5:** commit `fix: the tariff rejects coverage gaps and the ev rate reaches the bill`

### Task 1.8: Same-zone precomputed resistor order (envelope MAJOR-1)

**Files:**
- Modify: `crates/hares-envelope/src/boundary_rc.rs` (1797-1799 `pop()` -> `remove(0)`; 1847 wiring)
- Test: replace `material_and_precomputed_same_zone_step_response_match` with a short-window (<= 5 min) step-response assertion at 1e-9 on a 4-layer two-material stack (the existing steady-state test cannot discriminate)

- [ ] **Step 1:** failing discriminating test. **Step 2:** FAIL. **Step 3:** drop the cut-surface stub, wire with `res_offset = 0`. **Step 4:** PASS + frame_golden (furniture-fin fixtures shift if any multi-layer row exists). **Step 5:** commit `fix: the precomputed same-zone boundary keeps the zone-facing half-layer at the zone`

### Task 1.9: Control-signal validation always-on (core M3)

**Files:**
- Modify: `crates/hares-control/src/signal.rs` (57-117 - debug_asserts become an unconditional `validate()`)
- Modify: `crates/hares-core/src/dwelling/mod.rs` (4654-4662 - `apply_control` validates; 4698)
- Test: inverted setpoints, NaN power, `min_soc > max_soc` rejected from `apply_control` in release

- [ ] **Step 1:** failing test. **Step 2:** FAIL. **Step 3:** implement `validate()` called at both queue sites, returning `HaresError::InvalidState` naming the field. **Step 4:** PASS. **Step 5:** commit `fix: control signals are validated in every build profile`

### Task 1.10: Conservation holes made impossible or measurable (core M1 + M2)

**Files:**
- Modify: `crates/hares-core/src/dwelling/mod.rs` (8762-8786 invariant skip; 6469-6540 rollback)
- Test: a production dwelling with zero node capacitances fails construction; a forced mid-`step()` water-heater failure leaves tank energy change equal to the metered draw (or the divergence counted in `RunHealth`)

- [ ] **Step 1:** failing tests. **Step 2:** FAIL. **Step 3:** empty `node_capacitances` on a non-test dwelling is an error (keep the skip behind `#[cfg(test)]` or an explicit fixture flag); snapshot the failed equipment's committed state before `step()` and restore it on rollback (checkpointing pattern), or count `discarded_state_energy_wh`.
- [ ] **Step 4:** PASS. **Step 5:** commit `fix: the thermal invariant cannot be silently skipped and a rollback no longer discards state`

## Wave 2 - Majors: hardening, silent-fallback elimination (parallel packages; Wave 1's mod.rs tasks must land first)

### Task 2.1: Warm-up arithmetic + timestep guards (core M4)
`crates/hares-core/src/dwelling/mod.rs:6151-6157` - compute `steps_per_day` from milliseconds; a non-integral or zero result is a construction error naming the resolution. Test: 60/900/3600 s values; sub-second and 1500 ms rejected. Commit `fix: a resolution that cannot divide the day fails at construction`.

### Task 2.2: Natural-ventilation contract decided (core M5)
`crates/hares-physics/src/infiltration.rs:379-444` - adopt the E+ auto-Cd (`0.40 + 0.0045|dT|`, one line) or record the fixed-Cd choice in `docs/PHYSICS_DECISIONS.md` and correct the docstring to name the hybrid. Pin with golden vectors against a port of `ZoneEquipmentManager.cc:5988-6060`. Commit `fix: the natural ventilation model states and implements one contract`.

### Task 2.3: Envelope latent-input guards (envelope M2, m5, m12, m8, m10)
`crates/hares-envelope/src/boundary_rc.rs` - assert `n_zones < LAYER_NODE_BASE` in `assemble_building_rc` (50, 665-669, 1526-1531); guard `unique_depths <= 99` (42-48, 638-653); `MIN_CAPACITANCE_J_K` violations counted in `EnvelopeDiagnostics` (31, 623, 1623, 1811); `solar.rs:100-105` assert -> construction-time typed error; `initialization.rs:194-198` fallback warns. Tests: 1000-zone construction error, 100-depth error. Commit `fix: envelope index ceilings and capacitance floors fail or count loudly`.

### Task 2.4: Exterior skin diagnostics (envelope MAJOR-3)
`crates/hares-envelope/src/thermal_solver/longwave.rs` (317-374) - count iteration-cap exits and report max skin residual per step in `EnvelopeComponentGains`; warn once on lag > 1 K. Commit `feat: the exterior skin solve reports its own convergence`.

### Task 2.5: Equipment stage validation + telemetry honesty (equipment M4, m3, m4, m5)
`heater.rs` - add `er_soft_lockout_time_s` (default = hard, matching OCHRE `HVAC.py:1203-1207`); validate `stage_heating_eirs` length like the furnace; report COP on HP-only heat during `HeatingHPAndER`; `tankless.rs:235-240` instantaneous UEF->EF constants or documented divergence. Commit `fix: equipment stage counts validate and backup heat stops inflating the reported COP`.

### Task 2.6: ASHP hot-path env read (equipment M5)
`heater.rs:2481-2500` - cache `HARES_DEBUG_ASHP_CONTROL` in `OnceLock` or gate behind `observe`. Commit `perf: the ashp debug read leaves the per-step path`.

### Task 2.7: Price-signal honesty (grid M6)
`ev_driver/v2g.rs:13`, `ev_driver/price.rs:76` - export votes read `export_price`; a missing price marks the actor unhealthy instead of substituting 0.0; `ev_driver/mod.rs:1093` carries a real discharge ceiling. Test: V2G responds to `export_price` under NetBilling. Commit `fix: export decisions price against the export rate and a missing price is not zero dollars`.

### Task 2.8: RNG stream hygiene (schedules M4)
`actor_registry.rs:388-441`, `ev/catalog.rs:540-644`, `crates/hares-types/src/schedule.rs:489` - every schedule source seed derived via the existing `derive_sub_rng` stream-nonce scheme; explicit-seed or dwelling-derived default instead of `[0u8; 32]`. Test: two noisy TimeWindows sources with unspecified seeds are not identical. Commit `fix: schedule sources never share a random keystream`.

### Task 2.9: Fleet seed + DR + event-rate contracts (schedules M5, m1, m2)
`crates/hares-fleet/src/fleet.rs` (2024-2025, 348-357) - `master_seed: Option<u64>` on `from_resstock`, threaded to the report; `dr_compliance.rs:933` latch the draw on the Normal->event transition; `event_load.rs:1718-1727` normalize start probability by dt or declare the basis. Tests per the review. Commit `fix: fleets seed from the caller and dr compliance is per event`.

### Task 2.10: EV driver schedule failures propagate (schedules M3)
`ev_driver/mod.rs:666-713` - `unwrap_or` fabricated defaults become `?` errors naming the actor (keep the documented re-sample-exhausted arrival guard). Commit `fix: the ev driver fails on unreadable schedules instead of inventing a day`.

### Task 2.11: Clamp census (core rearch #4, S)
Generalize `curve_index_clamps` into a counted `ClampSite` taxonomy (AIM-2 n, RH<=1, 20-ACH cap, film floors, `tank.rs:469` draw truncation) drained per step like `take_health_counts`. Commit `feat: every physics clamp is counted, none are silent`.

### Task 2.12: Duplicated-constant and dead-code sweep (core m1, equipment m6; S)
`autosize.rs:123` imports `BTU_PER_HR_PER_W` from physics constants; wire or delete the companion-RTF plumbing (`cooler.rs:110`, `heater.rs:1419-1429`, `coil_physics.rs:167-169`); delete `compute_schedule_offset` (`environment.rs:1774-1793`); fix stale docs (`weather-pipeline.md` PCHIP, `schedule-sources.md` SeededNoise, `thermal-envelope-solver.md:275` ground-reflected, `hvac.md:100` crankcase port, `python.md` release-notes -> CHANGELOG). Commit `chore: one home for conversion factors and docs that match the code`.

### Task 2.13: Setpoint unit + plausibility contract (schedules M2)
`crates/hares-io/src/schedule_resolve.rs` (955-958) - convert the OS-HPXML Fahrenheit convention at parse for the OsHpxml family (mirroring OCHRE `schedule.py:411-412`), plus construction-time plausibility validation (error > 40 C / < 5 C). Test: an F-valued fixture CSV resolves to C; an absurd setpoint is rejected. Commit `fix: setpoint columns follow the os-hpxml fahrenheit convention and validate their range`.

### Task 2.14: Schedule resample memory (schedules m4)
`crates/hares-types/src/schedule.rs:254-289` - hold source-resolution rows and index by division (or drop the per-row `DateTime<FixedOffset>` nothing reads at run time, `schedule_resolve.rs:1854`); the 15-min 16-column ResStock CSV at a 1-minute step currently costs ~50 MB/dwelling, multiplied across fleet workers. Gate: before/after memory number from the I-08 allocation-accounting counters. Commit `perf: the schedule table no longer materialises an upsampled copy`.

### Task 2.15: Grid and equipment minor hardening batch (grid m1-m5, equipment m1, m7)
- `pv/mod.rs:325` default `InverterPriority::Watt`; document P=0 semantics per mode.
- `battery/mod.rs:483-484` constructor default matches the post-init one-way efficiency convention; self-discharge applied after the SOC clamp (`:1290-1311`).
- `ev_driver/mod.rs:1093` real discharge ceiling in `DecisionContext`; `ev/mod.rs:1064-1079` proportional ready-by pacing and clearing the grace on trip cancellation.
- `scheduled_load.rs:433-435` `PowerSetpoint` override keeps Q from the ZIP model.
- `docs/alignment/DIVERGENCES.md` documents the `space_fraction` meter-vs-gains asymmetry and the boiler convention.
Tests: battery at min_soc does not regain self-discharged energy; PowerSetpoint reports nonzero Q for a nonzero-pf load. Commit `fix: grid and equipment edge behavior matches its documented contracts`.

## Wave 3 - Python surface and gym performance (parallel, after Wave 1 Task 1.9)

### Task 3.1: Typed step-failure across the batch boundary (arch M3)
`crates/hares-python/src/{py_dwelling.rs:2468,py_gym.rs:823,630}` - replace `FATAL_DWELLING_PREFIX` string sniffing with `struct StepFailure { msg: String, fatal: bool }` across threads; keep the Python exception mapping; add a prefix-collision test. Commit `fix: step failures cross the batch boundary typed, not sniffed`.

### Task 3.2: Batch fast pass (arch M4, M5, M8, M16)
`py_gym.rs` - cache zone keys per dwelling (field exists `py_dwelling.rs:718`); `include_step_dict: bool = False` on `batch_step`; `gym_env.py:678` lazy telemetry under the default reward; actions accept a C-contiguous `(N, A)` f64 buffer like the observation path; `warning_count` as int; `"seed"` omitted when unset. Gate: before/after numbers from `benches/rl_episode.rs`. Commit `perf: the batch step pays for no dicts it does not need`.

### Task 3.3: Fleet pool reuse + memory cap (arch m12, m13)
`fleet.rs:686-690` cache the Rayon pool keyed by thread count; `dwelling/mod.rs:4445` `max_memory_rows` soft cap on `ReportOutput::Memory` with a remediation error; document in `docs/python.md` RL section. Commit `perf: the fleet reuses its pool and the memory report is bounded`.

### Task 3.4: Gym lifecycle honesty (arch 3b; M)
`py_dwelling.rs` - `snapshot_state`/`restore_state`/pickle support on `PyDwelling`; `python/hares/rl/vec_env.py` masked auto-reset with per-env seeds from a caller `SeedSequence`; observation gains `schedule_row`, occupancy presence, clock fields. Test: pickle round-trip; masked reset leaves other envs bit-identical. Commit `feat: gym episodes snapshot, pickle and reset masked`.

## Wave 4 - Architecture (sequential; each gated by the golden suite)

### Task 4.1: Split `hares-io` into `hares-input` + `hares-output` (arch M1)
Move `hpxml/ epw/ resstock/ schedule/ defaults/ config.rs` -> `hares-input`; `output/` (writer, catalogue, reduce, metrics) -> `hares-output` with arrow/parquet; `SimulationConfig` lives in input behind the core re-export shim (`hares-core/src/lib.rs:60`). Pure module moves; `frame_golden compare-all` must be bitwise-clean. Commit `refactor: input and output are separate crates`.

### Task 4.2: Extract `Dwelling`'s recording layer (arch M2)
`mod.rs` `*Columns`/`OutputRowBindings`/`ZoneColumnCaches`/`DiagnosticsBindings` + the record tail of `run_timestep` -> a `recording` module, then migrate into `hares-output`; extract `WarmupRunner` (region 3147-4337 + `run_warmup_converged` 6141); move `ControlDispatcher` to its own file; extract the nine named phases of `run_timestep_inner` (6554-8096) into a `StepPipeline` so the documented ordering is structural. Commit `refactor: the dwelling delegates to a recording set, a warmup runner and a step pipeline`.

### Task 4.3: `hares-types` hygiene (arch m9)
`zip.rs` -> `hares-physics`; `alloc_count.rs`/`panic_hook.rs` behind `profiling` or into core. Commit `refactor: the types leaf holds only contracts`.

### Task 4.4: Envelope numerics upgrades (envelope improvements 1, 2, 3, 5; M-L each, bench-gated)
Newton exterior-skin solve (replaces the heavy-ball fixed point; `longwave.rs:321-335`); unify material and precomputed boundary builders into one node-centered path (`boundary_rc.rs:1564-1879`); window glass temperature node (collapses the solar-share, sky-deviation and interior-LWR special cases); star-mesh LWR linearized at the zone's mean radiant temperature from steady-state init. Cache the env-dependent half of `build_input_vector` (computed once per step instead of twice; `stepping.rs:734-783`) - bitwise-safe, the largest remaining per-step win. Each behind a `thermal_step.rs` before/after bench number. Commit `perf: the envelope solves its skin by newton and builds its input vector once`.

### Task 4.5: Calibration surface (arch 3a)
`hares/calibration.py` (`CalibrationDwelling.evaluate(overrides, observation_fields, periods, seed) -> CalibrationRun`), Rust `evaluate_overrides` returning Arrow IPC via the frozen observation plan; `warmup_residuals()` exposed; blueprint-rebuild semantics (never in-place mutation). Commit `feat: a calibration evaluate surface over the existing engine primitives`.

### Task 4.6: IEEE 1547 response layer (grid M4; L)
Trip/ride-through and frequency-droop tables for PV/battery/EV inverters evaluated from `bus_voltage_pu()`/`frequency_hz`; explicit "no trip tables configured" warning; per-DER trip state in telemetry. Test: 0.6 pu sag trips PV, ride-through window honored, frequency-Watt droop shape. Commit `feat: der inverters respond to voltage and frequency per ieee 1547`.

## Wave 5 - Validation and measurement (parallel with Wave 4)

### Task 5.1: Run-level energy closure report
End-of-run per-zone report: external gains = stored dE + envelope conduction + delivered end-uses, residual in kWh and ppm of gross (per-step tolerance `max(1 W, 1e-6 gross)` integrates to ~4.4 kWh/yr of silent bias). Fleet telemetry: residual distribution (mean/p99) and warm-up days-run. Commit `feat: the run reports its own energy closure`.

### Task 5.2: BESTEST + ISO 13786 expansion
Add cases 620, 650, 920, 950, the L-series low-mass pair (stresses the capacitance wiring Task 1.8 protects), floor-warmup 195/395; ASHRAE 140 bands pinned in-code; ISO 13786 areal capacity/decrement factor/phase lag per construction; timestep-sweep invariance (60/300/900/3600 s, monotone convergence); window IAM at 40/60/80 deg vs the E+ table; AIM-2 vs a Walker & Wilson worked example. Commit `test: the envelope answers to ashrae 140 and iso 13786`.

### Task 5.3: Stochastic lifecycle invariant tests
Warmup-day-count invariance; episode reproducibility end-to-end (EV archetype, day-boundary crossing); checkpoint roundtrip with a LogNormal draw source; event-rate invariance across `time_res`; DR event-scoped compliance pins; fleet seed propagation; sub-year Dec 31 -> Jan 1 wrap; grid island conservation property test (from Task 1.4, kept as a property test over random islands). Commit `test: the stochastic lifecycle is pinned end to end`.

### Task 5.4: OCHRE-parity assertion expansion
Add per-equipment end-use reconciliation (meter kW vs zone heat x space_fraction) to all 12 OCHRE fixtures; assert `Mode`, `ER Power`, `Defrost`, `COP`, `Runtime Fraction` columns on cold-climate ASHP fixtures (`cz4a_ashp_hpwh`, `cz6b_*`); pan-heater standby column. Would have caught Task 1.1 and the pan-heater divergence mechanically. Commit `test: ochre fixtures reconcile every end use`.

### Task 5.5: Validation bench for the OR-5 invariant cost (arch M6; S)
Criterion case isolating per-step validation/port-snapshot cost with the checks compiled out as the comparison arm; record the delta. If < 1-2%, close the question permanently. Commit `bench: the invariant cost is measured and recorded`.

## Execution order and fan-out map

- **Wave 1:** 10 parallel packages. Conflict domain: `dwelling/mod.rs` is touched by 1.5, 1.6, 1.7, 1.9, 1.10 - dispatch those five sequentially within the wave (order: 1.9 -> 1.6 -> 1.10 -> 1.5 -> 1.7); 1.1, 1.2, 1.3, 1.4, 1.8 are fully independent and run concurrently.
- **Wave 2:** 15 packages, all independent of each other; only 2.1 touches `mod.rs`.
- **Wave 3:** 3.1 and 3.2 touch `py_gym.rs` - serialize (3.1 -> 3.2); 3.3, 3.4 parallel after.
- **Wave 4:** strictly sequential (crate splits invalidate file paths for later tasks).
- **Wave 5:** fully parallel with Wave 4 except 5.2 needs Task 1.8 landed.

Self-review notes: every P0 task carries its pinning test from the review report; severity citations (C/M/m numbers) refer to the six review reports preserved in the session record; the Review Focus items map to Tasks 1.3, 1.4, 1.6, 2.1 and 1.7 respectively.
