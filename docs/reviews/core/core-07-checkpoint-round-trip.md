# Checkpoint round-trip serialization fidelity
**Review ID**: core-07
**Category**: core
**Date**: 2026-05-26

## Files Reviewed
crates/hares-core/src/checkpoint.rs crates/hares-equipment/src/ev/checkpoint.rs crates/hares-core/src/engine.rs

## Vendor/Reference Files Consulted
None

## Findings
### Finding 1: Actor state is NOT checkpointed — decision logic diverges after restore
**Severity**: high
**Description**: The `DwellingCheckpoint` captures equipment, solver, clock, and RNG state, but **actor state** (`Vec<Box<dyn Actor>>`) is entirely omitted from the checkpoint round-trip. Several actors maintain mutable decision state across timesteps that is critical for bitwise-identical restoration:

- `EvDriverActor` maintains `estimated_soc: f64` (`crates/hares-core/src/actors/ev_driver/mod.rs:239`) — a running estimate of EV battery SOC that can diverge from the actual equipment SOC due to thermal derating and charging losses. On restore, `estimated_soc` resets to `1.0` (the construction-time initial value at `:312`), while the actual EV SOC (correctly restored from equipment state) may be lower. The actor's `current_soc()` method (`:432-438`) reads `env.equipment_core` first, but on the first post-restore step `equipment_core` is empty (see Finding 3), causing fallback to the stale `estimated_soc`.
- `BmsActor` maintains `current_day_ordinal0: u32` (`crates/hares-core/src/actors/bms.rs:26`) and cached `daily_avg_price`/thresholds that persist across steps. After restore these are at their initial sentinel values (`u32::MAX`), causing a forced recomputation on the first step. While functionally self-healing, the recomputation may produce a different ordering of signals if any equipment state depends on the exact step at which BMS signals are first emitted.
- `Occupant` maintains `current_step: usize` (`crates/hares-core/src/actors/occupant.rs:130`) for indexing into its `presence_schedule`. After restore this is at `0`, though the actual dwelling clock is at step N. This means the occupant presence state is misaligned — it will replay presence from step 0 rather than step N — unless the environment schedule replay compensates. (The environment manager recomputes `schedule_idx` from the global `timestep_index`, which is correct, but the `Occupant` actor uses its own `current_step` field directly.)

**Code Location**:
- `DwellingCheckpoint` struct: `crates/hares-core/src/checkpoint.rs:14-30` — no actor state fields
- `save_checkpoint()`: `crates/hares-core/src/dwelling/mod.rs:1991-2019` — does not visit `self.actors`
- `load_checkpoint()`: `crates/hares-core/src/dwelling/mod.rs:2022-2068` — does not restore actor state
- `Actor` trait: `crates/hares-core/src/actor.rs:43-76` — no `save_state`/`load_state` methods
- `Dwelling` struct: `crates/hares-core/src/dwelling/mod.rs:712-713` — `actors: Vec<Box<dyn Actor>>`

**Root Cause**: The `Actor` trait (`crates/hares-core/src/actor.rs:43`) defines only `name()`, `interests()`, `decide()`, and `telemetry()` methods. There is no `save_state()` or `load_state()` contract, so no actor state can be checkpointed. The `DwellingCheckpoint` schema (`checkpoint.rs:14-30`) has no provision for actor state fields.

**Impact**: Any simulation with stateful actors (EV driver, BMS with price forecasting, occupancy-based actors) will produce **different outputs** after checkpoint restore compared to a continuous run. The divergence magnitude depends on how heavily decisions depend on the stale state. In the best case with no stateful actors, the existing regression test (`tests/regression/checkpoint_restart.rs`) passes because the BEopt example dwelling may not include EV/BMS/occupancy actors.

### Finding 2: `prior_electrical_summary` is NOT checkpointed — first post-restore step sees zero electrical state
**Severity**: high
**Description**: The Dwelling struct holds `prior_electrical_summary: ElectricalSummary` (`crates/hares-core/src/dwelling/mod.rs:659`) which captures the previous step's net grid power, PV generation, base load, battery power, and EV power. This field is populated at the **end** of each timestep (`:2800-2806`) and fed into `latest_env.electrical` at the **start** of the next timestep (`:2253`). Actors that make dispatch decisions based on `env.electrical` (e.g., BMS `clamp_discharge_for_export` at `crates/hares-core/src/actors/bms.rs:135, 141`, which reads `base_load_kw` and `pv_generation_kw`) will see an all-zeros `ElectricalSummary` on the first post-restore step. In the original continuous run at the same step, actors saw the actual prior-step electrical values.

**Code Location**:
- Initialization: `crates/hares-core/src/dwelling/mod.rs:1246` — `prior_electrical_summary: ElectricalSummary::default()`
- Set at end of step: `crates/hares-core/src/dwelling/mod.rs:2800-2806`
- Read at start of step: `crates/hares-core/src/dwelling/mod.rs:2253` — `self.latest_env.electrical = self.prior_electrical_summary.clone()`
- BMS usage: `crates/hares-core/src/actors/bms.rs:135, 141`

**Root Cause**: `prior_electrical_summary` is a Dwelling-level field that is never serialized into `DwellingCheckpoint`. On restore, a fresh `Dwelling` is constructed with `ElectricalSummary::default()` (all zeros).

**Impact**: Actors that condition their behavior on the previous step's electrical state (grid export rules, net zero strategies, DER dispatch) make decisions on stale data for the first post-restore step. The degree of divergence depends on the magnitude of step-to-step electrical variation. In the tested BEopt example (which likely lacks an active BMS), this goes undetected.

### Finding 3: `latest_env.equipment_core` and `latest_env.equipment_telemetry` are NOT checkpointed — actors see empty equipment state on first post-restore step
**Severity**: high
**Description**: The `EnvironmentState` struct holds `equipment_core: HashMap<EquipmentId, CoreOutput>` (`crates/hares-types/src/environment.rs:326`) and `equipment_telemetry: HashMap<String, Telemetry>` (`:322`) which provide the previous step's committed equipment outputs (SOC, power flows, connection state) for actor decision-making. These maps are populated at the end of each timestep in `run_timestep()` (`crates/hares-core/src/dwelling/mod.rs:2669-2706`) and read by actors during the next step. On checkpoint restore, `latest_env` retains its construction-time state with empty maps. The first `update_in_place()` call (`environment.rs:477`) does not populate these maps, and the first step's `equipment_core` population occurs **after** actors have already decided (`:2669`). The `EvDriverActor` explicitly depends on this: `current_soc()` (`ev_driver/mod.rs:432-438`) reads `env.equipment_core` for the actual SOC and falls back to the actor's own `estimated_soc` when the map is empty.

**Code Location**:
- `EnvironmentState.equipment_core`: `crates/hares-types/src/environment.rs:326`
- `EnvironmentState.equipment_telemetry`: `crates/hares-types/src/environment.rs:322`
- Populated after equipment steps: `crates/hares-core/src/dwelling/mod.rs:2669-2706`
- `EvDriverActor::current_soc()` read site: `crates/hares-core/src/actors/ev_driver/mod.rs:432-438`
- `BmsActor::read_soc()` read site: `crates/hares-core/src/actors/bms.rs:108-114`

**Root Cause**: These fields are marked `#[serde(skip_serializing)]` and are never included in `DwellingCheckpoint`. They are populated exclusively by the post-equipment snapshot at the end of `run_timestep`. On a fresh `Dwelling` after construction, they are empty. After `load_checkpoint()` restores equipment internal state but does **not** trigger a snapshot, the first step's actor decisions operate on empty maps.

**Impact**: Actors that read equipment core outputs for decision-making (EV driver SOC-based charging decisions, BMS SOC-based dispatch) get empty/fallback values on the first post-restore step. This can cause:
1. EV driver charging decisions based on estimated SOC rather than actual SOC
2. BMS charge/discharge decisions with no SOC visibility
3. Any DR/schedule actor that conditions on equipment telemetry making uninformed initial decisions

### Finding 4: `save_postcard` panics on serialization failure — checkpoint save can crash the simulation
**Severity**: medium
**Description**: The `save_postcard` helper function (`crates/hares-equipment/src/lib.rs:248-252`) calls `panic_serialize(e)` on postcard serialization errors instead of returning a `Result`. This function is called by every equipment's `save_state()` method, which is invoked from `Dwelling::save_checkpoint()` (`dwelling/mod.rs:2008`). A failure during postcard serialization of any single equipment's state will cause the entire simulation process to panic with no recovery path — the simulation state and any partial results are lost. In contrast, `load_postcard` (`:271-274`) properly returns a `Result`, and `DwellingCheckpoint::save()` (`checkpoint.rs:35-36`) does handle write failures via `map_err`.

**Code Location**: `crates/hares-equipment/src/lib.rs:248-252`
```rust
pub fn save_postcard<T: Serialize>(state: &T) -> Vec<u8> {
    match postcard::to_allocvec(state) {
        Ok(bytes) => bytes,
        Err(e) => panic_serialize(e),  // panics the process
    }
}
```

**Root Cause**: `save_postcard` is designed as an infallible helper matching the `Equipment::save_state() -> Vec<u8>` signature (which does not return a `Result`). The `try_save_postcard` variant exists at `:256` but is unused by equipment implementations. `save_state()` is called in a map closure inside `save_checkpoint()` where error propagation would require `try_for_each` or similar.

**Impact**: The simulation crashes irrecoverably if any equipment state cannot be serialized. While postcard failures are rare (typically only for non-finite floats or unsupported types), the asymmetry with the deserialization path (which properly returns `Result`) means a long-running simulation that successfully produces output for many steps could still crash during an attempted checkpoint save.

### Finding 5: Regression checkpoint restart test may not cover actor/electrical-dependent divergences
**Severity**: medium
**Description**: The existing checkpoint restart regression test (`tests/regression/checkpoint_restart.rs:12-136`) checks only `net_electric_power_kw` divergence with a tolerance of `1e-9` (`:115`). It uses a simple BEopt example dwelling that may not instantiate stateful actors (EV driver, BMS) or DER equipment (battery, PV) whose decision logic depends on `equipment_core` or `electrical` state. The test validates only the "happy path" where the missing actor/electrical state does not cause a detectable power divergence. This leaves Finders 1-3 unexercised and could allow future regressions.

**Code Location**: `tests/regression/checkpoint_restart.rs:110-126` — comparison logic; `tests/regression/checkpoint_restart.rs:18-24` — test dwelling configuration.

**Root Cause**: The test dwelling is configured from `benches/common.rs:108` which uses the OCHRE BEopt example HPXML. This example may not include actor-instantiated equipment. There is no dedicated test that exercises checkpoint round-trip with EV + BMS + battery configurations.

**Impact**: Latent checkpoint fidelity issues are not caught by automated testing. Code changes that affect actor or electrical state could introduce divergences that go unnoticed.

### Finding 6: EV checkpoint module is correctly integrated into the main checkpoint path
**Severity**: low
**Description**: The `EvCheckpoint` struct (`crates/hares-equipment/src/ev/checkpoint.rs:4-29`) is `pub(super)` scoped to the `ev` module and properly serialized/deserialized through the `Equipment` trait's `save_state()`/`load_state()` contract. The EV's `save_state()` (`ev/mod.rs:795-821`) wraps all mutable state fields into `EvCheckpoint` and encodes via `save_postcard`. The `load_state()` (`ev/mod.rs:823-855`) fully restores all fields and correctly resets runtime-only fields (`v2l_active`, `v2l_power_kw`, `core_output`). The EV checkpoint is stored as a postcard binary blob within the main `DwellingCheckpoint.equipment_states` vector, which is then JSON-serialized as part of the complete checkpoint file. There is no separate EV checkpoint file — integration is correct and complete at the equipment level.

**Code Location**:
- `EvCheckpoint` struct: `crates/hares-equipment/src/ev/checkpoint.rs:4-29`
- `Ev::save_state()`: `crates/hares-equipment/src/ev/mod.rs:795-821`
- `Ev::load_state()`: `crates/hares-equipment/src/ev/mod.rs:823-855`
- Integration into main path: `crates/hares-core/src/dwelling/mod.rs:2005-2009` (save), `:2036-2044` (restore)

**Root Cause**: N/A — this is a positive finding confirming correct integration.

**Impact**: The EV equipment checkpoint round-trip is well-structured. However, the EV's `away_charge_actual_kw: f64` field (`ev/mod.rs:91`) is NOT included in `EvCheckpoint` — it is a derived/telemetry value recomputed each step, so its omission is intentional and correct.

### Finding 7: `humidity_ratios` stored in `HashMap` with non-deterministic iteration order
**Severity**: low
**Description**: The `HumiditySolver.humidity_ratios` field (`crates/hares-envelope/src/humidity_solver.rs:43`) is a `HashMap<ZoneId, f64>`. The checkpoint serializes this by iterating the map into a `Vec<(ZoneId, f64)>` (`dwelling/mod.rs:1993-1998`), and the restore path collects it into a new `HashMap` (`:2051`). The HashMap's internal iteration ordering is non-deterministic across runs and platforms (SipHash randomization). **Currently**, the humidity solver only accesses entries via `HashMap::get()` (by ZoneId key), so the ordering has no functional effect. However, if any future code iterates the `humidity_ratios` HashMap directly and the iteration order influences computation, the result would be non-deterministic across checkpoint rounds.

**Code Location**:
- `HumiditySolver.humidity_ratios`: `crates/hares-envelope/src/humidity_solver.rs:43`
- Save iteration: `crates/hares-core/src/dwelling/mod.rs:1993-1998`
- Restore collection: `crates/hares-core/src/dwelling/mod.rs:2051`

**Root Cause**: The Rust standard library `HashMap` uses a per-process random seed (SipHash) for iteration ordering. The checkpoint serializes the map in whatever order iteration produces, and the deserialized `Vec` preserves that order. This is correct for the Vec but the restored HashMap will have a different internal layout.

**Impact**: No current functional impact (all access is keyed by `ZoneId`), but fragile. A future change that iterates `humidity_ratios` in a way that affects computation (e.g., generating a ZoneOrder-dependent payload) would introduce non-determinism.

## Summary
- Total findings: 7
- Critical: 0
- High: 3 (Findings 1, 2, 3)
- Medium: 2 (Findings 4, 5)
- Low: 2 (Findings 6, 7)

## Recommendations
1. Add `save_state()` / `load_state()` methods to the `Actor` trait and serialize actor state into `DwellingCheckpoint` (addresses Finding 1). At minimum, the `estimated_soc` in `EvDriverActor` and `current_step` in `Occupant` must be captured.
2. Add `prior_electrical_summary: ElectricalSummary` to `DwellingCheckpoint` and restore it in `load_checkpoint()` (addresses Finding 2).
3. After `load_checkpoint()` restores equipment state, call a snapshot method that populates `latest_env.equipment_core` and `latest_env.equipment_telemetry` from the restored equipment's current state so that the first post-restore step's actors see correct values (addresses Finding 3).
4. Change `Equipment::save_state()` return type from `Vec<u8>` to `Result<Vec<u8>>` and replace `save_postcard` usage with `try_save_postcard` throughout equipment implementations. Update `save_checkpoint()` to propagate errors instead of panicking (addresses Finding 4).
5. Extend the checkpoint restart regression test to cover a dwelling configuration with EV + battery + BMS actors to exercise the problematic code paths (addresses Finding 5).
6. Consider replacing `HashMap<ZoneId, f64>` with `BTreeMap<ZoneId, f64>` for `humidity_ratios` to guarantee deterministic iteration order (addresses Finding 7).

## References / Citations
- `DwellingCheckpoint` struct: `crates/hares-core/src/checkpoint.rs:14-30`
- `save_checkpoint()`: `crates/hares-core/src/dwelling/mod.rs:1991-2019`
- `load_checkpoint()`: `crates/hares-core/src/dwelling/mod.rs:2022-2068`
- `Dwelling` struct fields: `crates/hares-core/src/dwelling/mod.rs:631-730`
- `run_timestep()` flow: `crates/hares-core/src/dwelling/mod.rs:2209-2815`
- `Actor` trait: `crates/hares-core/src/actor.rs:43-76`
- `EvDriverActor`: `crates/hares-core/src/actors/ev_driver/mod.rs:206-300`
- `BmsActor`: `crates/hares-core/src/actors/bms.rs:1-150`
- `EnvironmentState`: `crates/hares-types/src/environment.rs:312-343`
- `EnvironmentManager::update_in_place()`: `crates/hares-core/src/environment.rs:477-663`
- `Equipment` trait save/load: `crates/hares-equipment/src/lib.rs:115-116`
- `save_postcard`/`load_postcard`: `crates/hares-equipment/src/lib.rs:248-274`
- `HumiditySolver`: `crates/hares-envelope/src/humidity_solver.rs:40-51`
- Checkpoint restart regression test: `tests/regression/checkpoint_restart.rs:1-136`
