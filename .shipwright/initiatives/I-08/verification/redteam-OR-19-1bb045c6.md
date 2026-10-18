# Red team: OR-19 roster plan/install (1bb045c6)

Branch `fix-or19` at `1bb045c6` (12f753be and a3a21a92 = cherry-picks of
9ef347e0/ffa9051f, 1bb045c6 = review/red-team fixes), on top of `1c18543a`.

Verdict: ATTACK-SUCCEEDED

Counts: blocker 0, major 1, minor 4, info 4.

Method: detached worktree `.workstrees/redteam-or19` with scratch tests only
(an `include!("rt_scratch.rs")` inside the dwelling test module, a
`#[cfg(test)]` EV-driver snapshot decoder in `actors/ev_driver/mod.rs`, and an
`include!("rt_py.rs")` test module in `hares-python/src/py_dwelling.rs` run
under `Python::attach`). None committed; the worktree was removed afterwards.
"Fingerprint" below means: equipment names and ids, actor order, auto-registered
names, `next_equipment_id`, `next_ev_driver_stream`, `latest_env.equipment_core`
and `equipment_telemetry` keys, output column index, value count, scratch
length, equipment/actor column maps, recorder (rows, field count, schema
address), warning log, fluid loop-type map, `ports`, `rollback_ports`, tariff
presence, execution order, roster ids and the name map.

## Findings

### M-1 (major): the EV driver stream cursor is not checkpointed, so a resumed run gives two live drivers one RNG stream and diverges from the continuous run

The commit claims "Built-in EV drivers take RNG streams from a per-dwelling
cursor so no two live drivers share one". `next_ev_driver_stream`
(`dwelling/mod.rs:1764`) is not in `DwellingCheckpoint` (`save_checkpoint`,
`mod.rs:3769-3836`) and `load_checkpoint` does not touch it, while each restored
driver gets its stream back from its own blob (`EvDriverSnapshot.rng_stream`).
A dwelling rebuilt to resume from a checkpoint can only replay the surviving
roster, not its history, so its cursor is behind the streams it restores.

Reproduction (`rt_ev_cursor_checkpoint_resume`, BESTEST 600, write_output off):

- Continuous: add EV1 (stream 1), remove EV1, re-add it (stream 2), step 4,
  `save_checkpoint`, add EV2 -> EV2 on stream 3.
- Resumed: fresh dwelling, add EV1 (stream 1, cursor 1), `load_checkpoint` ->
  EV1's driver is restored to stream 2; add EV2 -> EV2 on stream 2.

Result: `continuous EV1=(2,2) EV2=(3,0); resumed EV1=(2,2) EV2=(2,0)`. The two
live drivers in the resumed dwelling share (seed, stream 2), so EV2 replays
EV1's draws from the start, and the 96 following steps' `net_electric_power_kw`
differ from the continuous run. Any remove/re-add or replace of an EV before a
checkpoint triggers it (as would a calibration loop swapping EV parameters).

Fix: carry `next_ev_driver_stream` in `DwellingCheckpoint` (bump
`CHECKPOINT_VERSION`) and restore it in `load_checkpoint`, or on restore set the
cursor to one past the highest `RNG_STREAM_EV_DRIVER_BASE`-relative stream any
restored built-in driver holds. Add a test of exactly this sequence: resumed
equals continuous and no two live drivers share a stream.

### m-1 (minor, pre-existing, Python entrance): a rejected `add_pv` leaves a registered irradiance surface

`PyDwelling::add_pv` (`hares-python/src/py_dwelling.rs:1169-1213`) calls
`dwelling.environment.register_surface(...)` before `add_equipment`. Reproduction
(`rt_python_paths`, cz2a fixture): `add_pv(PV("PV1", 5, 30, 180))` accepted,
then `add_pv(PV("PV1", 5, 45, 90))` raises `HaresEquipmentError` (duplicate
name) but `environment.surface_count()` goes 20 -> 21: the environment computes
irradiance for a surface no equipment uses on every step, and
`latest_env.weather.solar_irradiance` carries it. Fix: register the surface only
after `add_equipment` returns `Ok` (the init placeholder already uses a cloned
env), or make surface registration part of the roster change.

### m-2 (minor, regression of an error class): duplicate `add_ev_with_driver` now raises `HaresSimulationError` naming the actor

`add_equipment_with_actors` runs `ensure_actor_names_free` before
`validate_candidate_equipment` (`roster.rs:458-461`). For a second
`add_ev_with_driver` with the same vehicle the supplied `EvDriver:<label>`
collides first, so Python gets `HaresSimulationError: control error: duplicate
actor name 'EvDriver:Tesla Model Y LR AWD' is not allowed`. Before 1bb045c6 the
path called `add_equipment` first and raised `HaresEquipmentError: duplicate
equipment name ...`. The state is unchanged (verified), only the class and
message are wrong for the user's actual mistake. Fix: validate the candidate
equipment before the supplied actors' names.

### m-3 (minor, pre-existing semantics, contradicts "on Ok fully consistent"): replace-in-kind with another equipment type keeps the old built-in actor

`replace_equipment("EV1", <Electric Boiler named "EV1">)` returns `Ok` and keeps
`EvDriver:EV1` bound to the boiler (`roster.rs:815-820`: actors follow the
name). Every step then logs `control apply failed for 'EV1': unsupported control
signal: requires SOC_TARGET` (`rt_replace_in_kind_other_type`). Documented
behaviour, but the roster is not consistent: either evict built-in actors whose
seed the replacement does not produce (compare `actor_seed()` kinds), or reject
the replacement.

### m-4 (minor): `RejectedEquipment` makes the Rust error variant depend on incidental warnings

`reject_candidate` (`roster.rs:172-183`) wraps the reason only when the
candidate drained warnings, so the same rejection is `HaresError::Equipment(..)`
for one candidate and `HaresError::RejectedEquipment { .. }` for another
(`rt_rejected_equipment_shapes`). Python is insulated (`to_py_err` classifies
by `reason`; verified Equipment -> `HaresEquipmentError`, Io ->
`HaresConfigError`, Control -> `HaresSimulationError`, message `"<reason> (the
rejected equipment warned: w1; w2)"`), but a Rust caller matching
`HaresError::Equipment(_)` silently misses warned candidates. Fix: either always
return `RejectedEquipment` from roster entrances that reject a candidate, or
document the variant contract on `add_equipment`.

### info

- I-1: install is not unwind-atomic. An `Equipment::core_output()` that panics
  during `install_roster_caches` (`roster.rs:1137-1142`) leaves the candidate
  pushed, actors committed, the name map and roster ids installed, and ports,
  dispatch targets and zone/output caches stale (`rt_panic_mid_install`). All
  real implementations are field getters and `PyDwelling` poisons on panic, so
  not reachable today; the "cannot fail" claim should say "cannot return Err".
- I-2: any built-in actor rebuild (`set_tariff`, `auto_register_actors`)
  mid-run recreates every built-in EV driver from word position 0 and resets the
  cursor (`EvDriver:EV2` stream (1, 2) -> (1, 0) in
  `rt_ev_streams_two_evs_remove_readd_tariff`), discarding driver state and
  replaying draws. Same as before OR-19.
- I-3: equipment added after rows are recorded gets no columns (frozen schema),
  including its built-in actor (`actor_column_map = [[]]`). Documented.
- I-4 (pre-existing, outside OR-19): event loads receive only
  `sub_rng.get_seed()` (`mod.rs:2513-2515`), and `derive_sub_rng` keeps the
  parent's seed, so the per-load stream index is discarded: every event load
  builds `ChaCha8Rng::from_seed(dwelling seed)` on stream 0, the same stream as
  the dwelling RNG and as each other. Worth a separate ledger row.

## What held

- Recorder in the plan: a rejected plan (fluid conflict, CSV and Parquet, no
  rotation and daily rotation) leaves the output file's inode, mtime and size,
  the directory listing, the recorder (same schema address) and the
  fingerprint unchanged; with daily rotation nothing is created at all.
  Recorder creation is the last fallible step of the plan, and every entrance
  commits immediately after a successful plan, so no rejected change creates a
  file. Accepted pre-step re-plans on the same path (one, then five adds and a
  removal) truncate the same inode while the old recorder holds no rows; the
  final CSV header and every row have the schema's 57 columns and the Parquet
  file reads back with all 8 rows. Re-planning never truncates recorded rows:
  `total_rows` counts pushed rows, so a dwelling with rows takes `FrozenUpdate`.
- Post-step add with `retain_batches` false (chunk 3) and true (chunk 3 CSV,
  chunk 1 Parquet): the recorder is not replaced, all 8 rows are written,
  batches 0/3/8, streamed metrics present for false. A rejected post-step add
  (undeclared loop 9) leaves the fingerprint and `save_checkpoint` Debug output
  identical.
- EV cursor within one dwelling: rejected `add_equipment`,
  `add_equipment_with_actors` and `replace_equipment` (schema-drift dwelling)
  and rejected `auto_register_actors` leave the cursor and fingerprint unchanged;
  two EVs get streams 1 and 2, a removed and re-added EV gets stream 3,
  `set_tariff` rebuilds them on 1 and 2. A rejected Python
  `add_ev_with_driver` leaves equipment, actor count, surfaces and warnings
  unchanged; it never seeds a built-in driver, so it cannot move the cursor.
- Replace under a new name rejected (Glycol replacement on a Water loop) keeps
  `EvDriver:EV1` and the whole fingerprint; accepted, it evicts
  `EvDriver:EV1` and seeds `EvDriver:EV9`, and the dwelling steps; a post-step
  rejected renamed replace leaves fingerprint and checkpoint identical.
- Fluid conflicts formerly caught by `validate_fluid_type_consistency`: one
  equipment declaring loop 7 as Water and Glycol is rejected pre-step by
  `plan_loop_types` ("conflicting fluid types: Water and Glycol"), fingerprint
  unchanged; after a step it is rejected by `ensure_ports_satisfied`, as are a
  Glycol replace of the Water declarer and, after removing the sole Water
  declarer, a Glycol add on loop 7 (the frozen accumulator keeps Water); Water
  is re-accepted. Pre-step, replacing the sole declarer with Glycol is accepted
  (intended). Assembly reaches the same check through `auto_register_actors`.
- Panics in the plan (`actor_seed()` panicking, `ports()` panicking on its
  second call) caught with `catch_unwind` leave the fingerprint unchanged and
  the dwelling steps.
- `set_tariff`/`clear_equipment`/`remove_equipment_by_end_use` on the drift
  dwelling: clear and remove-by-end-use succeed legitimately (they remove the
  drifting battery), the others reject cleanly.
- Step path: the `mod.rs` step hunks are field moves into `self.roster` only.
  `frame_golden compare-all` identical for the default build (5 manifests),
  `-F dst` (1) and `-F profiling` (5), release. `profiling_alloc` passes in
  release and debug with `-F profiling`. `cargo test -p hares-core` (all
  binaries, scratch tests skipped): all pass, 0 failed.

## Residual risk

Not run: the Python test suite through maturin (the Python paths were driven
from a Rust `Python::attach` test against the cz2a fixture instead), clippy,
and other crates' suites. Warm-up resets `current_step` to 0, so a roster change
after a warm-up re-derives the port tables as pre-step layout; not attacked.
