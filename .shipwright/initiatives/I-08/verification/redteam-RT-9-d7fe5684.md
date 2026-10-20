# Red team of RT-9 (fix-rt9 at d7fe5684: 3c734c44 + d7fe5684 on 1c18543a)

Claim attacked: "the thermal solver's restore covers every piece of state a
step reads, and its geometry cannot change after construction".

**Verdict: ATTACK-SUCCEEDED** (1 high, 3 medium, 3 low; plus info notes)

Worktree `.workstrees/redteam-rt9` (detached d7fe5684), scratch probes only:
`crates/hares-envelope/src/thermal_solver/rt9_probe.rs` (in-crate, reaches
private fields), `crates/hares-core/tests/rt9_probe_resume.rs`, and a
JSON-level test appended to `crates/hares-core/src/checkpoint.rs` tests. The
branch was not modified. The worktree and its target dir were removed when the
run finished.

## Findings

### 1. HIGH: ventilation recovery parameters are step state, but they are neither in `ThermalSnapshot` nor recomputed before the first resumed read

`ventilation_mut()` is the one config part RT-9 leaves mutable. It is mutable
because it is per-step state. At step k, `Dwelling::step` step 3c
(`dwelling/mod.rs` "propagate per-timestep ventilation recovery effectiveness")
writes the Ventilation equipment's *effective* sensible/latent effectiveness
into `config.ventilation`. These values are derated by bypass and defrost, and
they are 0 when the unit is off. Step k+1's `prepare_inputs` reads them in
`infiltration.rs:241-243,282` (balanced flow `forced * (1 - eff)`) **before**
step 3c refreshes them. That read builds `last_u`/`last_coupling` for every
ideal-capacity solve of the step. A dwelling restored from a checkpoint is
freshly built, so it holds the *rated* value from `solver_builder.rs:1455`.
The continuous run holds step k's effective value. `Ventilation::load_state`
does not restore `effective_*` either.

Repro (`rt9_probe_resume.rs::hrv_resume_equals_continuous`): bldg0176227 with
an added `heat recovery ventilator` fan (SRE 0.72, whole-building), weather
G0900090 (CT), 2018-01-02, 96 x 15 min. For each k it runs continuously,
checkpoints at k, restores into a fresh dwelling, and compares every output
column bitwise.

Result: at k = 8, 36, 40, 44 and 60 the solver's vent effectiveness at
checkpoint is 0.341 to 0.680 (defrost), and after load it is 0.72. **32 columns
diverge on the first resumed row.** At k=36: `Temperature - Indoor (C)` is
19.818 vs 19.501, `HVAC Heating Delivered (W)` is 9771.7 vs 8972.4, and gas,
electric, attic and crawl temperatures also differ. In Fresno April with the
same HRV (bypass, eff 0 vs 0.72) the state diverges the same way, but no column
differs because there is no HVAC load. The solver-level probe
(`probe_restore_completeness_ventilation_params`) shows the same thing: the
first post-`prepare_inputs` solve is 5349.77 W vs 6205.61 W.

Why the acceptance test misses it: every fixture in `tests/fixtures` that has a
`VentilationFan` uses `exhaust only`. For exhaust-only fans the effective
effectiveness is a constant 0 and equals the built value, so
`resumed_run_equals_continuous_run` cannot see this state.

Fix direction: add the ventilation recovery state (`sensible_recovery_efficiency`,
`latent_recovery_efficiency`, or the whole `MechanicalVentilationParams`) to
`ThermalSnapshot` and validate it in `restore_state` (finite, [0,1]). Bump the
checkpoint version. Add a resume test on an HRV/ERV fixture whose checkpoint
step is in defrost or bypass.

### 2. MEDIUM: ideal-capacity degraded-fallback state is read by the solve but is not snapshotted or reset by restore

`solve_ideal_capacity_for_target` reads `ideal_capacity_failure_counts` and
`last_good_capacity_w` (`stepping.rs` Err branch). Under `observe` it also
reads `consecutive_nonconvergence_count`. When failures reach
`ideal_capacity_degraded_threshold`, the solve returns last-good instead of 0.
None of these are in `ThermalSnapshot`, and `restore_state` does not clear
them. The `ThermalSnapshot` doc says it holds "every piece of mutable solver
state a step or an ideal-capacity solve reads". That is false.

Repro (`rt9_probe.rs::probe_restore_completeness_failure_streak`): the
reference solver gets two zero-gain failures (coupling d = 1e300), then is
snapshotted. A fresh solver restores that snapshot. Then the same failing
solve runs on both.

Result: continuous returns **5537.98 W** (degraded last-good) and restored
returns **0 W**. Reaching this needs |effective gain| <= 1e-12, which is a
pathological run. The documented invariant is still violated, and
`ideal_capacity_degraded_zones`/telemetry differ after resume.

Fix: snapshot the counts and the last-good map (sorted `Vec<(ZoneId, ..)>`),
or narrow the documented claim and reset these maps on restore.

### 3. MEDIUM: `checkpoint_shape_is_pinned_to_its_version` misses retypes inside the number class, tuple arity and order, unit-enum variant names, and Option inner types

`collect_shape` records one type tag per path: `number`, `string`, `bool` or
`null`. Every array element collapses to the path `[]`.

Repro (JSON-level, against the test's own `collect_shape`:
`checkpoint.rs::rt9_probe_shape_blind_spots`). One real source mutation was
also run against the tripwire.

| Mutation | Caught? |
|---|---|
| rename a field in a Vec element (`ActorStateCheckpoint.name` via `#[serde(rename)]`, real source mutation) | caught |
| add/rename a field in `ThermalSnapshot` / `ElectricalSummary` (path changes) | caught (by construction) |
| number <-> string retype | caught |
| f64 -> u64, u32 -> i64 (all "number") | **missed** |
| tuple arity in a Vec element, e.g. `last_coupling (usize,f64,f64)` -> `(usize,f64)` or a 4-tuple | **missed** (`.thermal.last_coupling[][]: number` is unchanged) |
| tuple reorder `(usize,f64,f64)` -> `(f64,usize,f64)` | **missed** |
| unit enum variant renamed (serialized as a string) | **missed** |
| externally tagged data enum variant swapped | caught (the key is in the path) |
| Option added, None in fixture | caught (a `null` leaf appears) |
| Option inner type changed while None | **missed** |

Fixture population: every Vec is non-empty (the test asserts it). The
checkpoint has no Option or enum fields today, so "every Option/enum branch
visited" holds only because there are none. Nothing forces a future Option to
be populated. The missed cases matter: a float->int retype or a tuple-arity
change makes an old checkpoint fail as a *parse* error with no version bump.
That is exactly what the version gate exists to prevent. The source-level
retype mutations (`rng_word_pos` u128->u64, serde re-serialization of
`equipment_id` and `last_coupling`) were blocked by this session's permission
classifier, so those rows use the JSON-level proof. `collect_shape` is a pure
function of the serialized `Value`, so the proof is exact.

Fix: tag numbers as `int`/`float` (`Number::is_f64`). Index fixed-arity arrays
by position. For example, pin the full serialized JSON of the populated
fixture as a golden string, or derive a schema (e.g. `schemars`) and pin that.
Require Options in the fixture to be `Some`.

### 4. MEDIUM (pre-existing, adjacent): `Dwelling::load_checkpoint` is not atomic

The solver-level atomicity holds. The dwelling does not. `load_checkpoint`
sets the clock, the RNG and the equipment states before
`thermal_solver.restore_state` validates.

Repro (`rt9_probe_resume.rs::load_checkpoint_with_bad_thermal_is_not_atomic`):
a checkpoint from step 5 with `thermal.x[0] = NaN` is loaded into a dwelling
at step 1.

Result: Err (correct). The thermal snapshot is unchanged. But
`clock.current_step()` is **5**, not 1, and the RNG and equipment were also
mutated. The dwelling is left half-restored. `restore_building_state` has the
same shape. Fix: validate the thermal snapshot (and the other parts) before
any mutation, e.g. a `validate_snapshot(&self, &ThermalSnapshot)` that
`restore_state` also uses.

### 5. LOW: `restore_state` accepts finite couplings that make the solve non-finite

`probe_accepted_edge_cases`: `(0, 0.1, f64::MAX)` is accepted and the solve
returns **-inf W**. `(0, f64::MAX, 1.0)` is accepted and the solve takes the
zero-gain failure path. Duplicate couplings and `d = -0.0` are accepted
correctly. Duplicates are legal: `d_agg` sums them. The comment says couplings
must "satisfy the same bounds the step's own couplings do", but those bounds
do not keep the result finite. Either bound them (e.g. `d` and `|forcing|`
below a physical ceiling) or document that the restore validates domain only.
The rejection error is also prefixed "failed to initialize steady-state
vector:" (`ThermalSolverError::Initialization`), which is misleading for a
restore failure.

### 6. LOW: the resume test's doc comment hard-codes file:line references

`checkpoint_regressions.rs:590-593` cites `dwelling/mod.rs:5888`, `:5947` and
`stepping.rs:714`, `:717`. At d7fe5684 the dwelling refs already point at the
"Step 1d"/"Step 1e" comments: the calls are at :5889-5890 and :5948-5949. The
stepping refs are exact. Any edit above those lines in a ~15k-line file moves
them, and main (`feat/output-results`) is already far ahead of the RT-9 base.
Cite symbols (`Dwelling::step` "Step 1d" `prepare_inputs` before "Step 1e"
`collect_and_solve`; `prepare_inputs_inner` rebuilding `last_coupling`)
instead of lines.

### 7. LOW: `Dwelling.thermal_solver` is a `pub` field, and `ventilation_mut` exposes more than recovery effectiveness

No other `&mut` path to `ThermalSolverConfig` exists. `config()` returns `&`.
`InteriorLwrZoneConfig::compute_scriptf` is reachable only on an owned config.
There is no Python binding, and no dwelling method replaces the solver. But
`Dwelling.thermal_solver` is `pub` (`dwelling/mod.rs:1663`), so any consumer
can replace the whole solver, swapping in mismatched geometry against the
dwelling's caches, or call `ventilation_mut()`. `ventilation_mut()` also
exposes `zone_flow_m3_s: HashMap<ZoneId, f64>`. Its keys are not validated
against modeled zones, and inserting into it allocates. The doc says "recovery
effectiveness" only. Consider a narrow `set_ventilation_recovery(eff_s, eff_l)`
and `pub(crate)` or accessor-only solver access.

## Info

- Zero-alloc coverage is single-zone. `make_solver` has one sensible zone, so
  "three solves per conditioned zone" is exercised for one zone only. A
  multi-zone coupled bracket is not covered.
- Pre-existing, not RT-9: the oracle suites leave a
  `hares_{freefloat,conditioned_dynamic}_*_diagnostics.csv` in `/tmp` on every
  run (dozens have accumulated).

## What held

- **Restore completeness outside ventilation and the fallback maps.** Probe
  `probe_restore_completeness_non_vent`: snapshot after 5 steps, then perturb
  every reachable non-snapshot field. That covers 4 steps under a different
  env and solar surface-id set (slot-map cache), `cached_outdoor_temp_c`, the
  LWR accumulators, `prev_zone_temps_c`, all scratch buffers, `coupling_buf`
  garbage, the degraded/warned sets, the balance outputs, and
  `shared_prefix_valid` forced true over a stale prefix. Then restore. Result:
  3 direct `solve_ideal_capacity_for_target` calls right after
  `restore_state`, then 10 steps (prepare, 3 solves, integrate; zone temps,
  payload, x, ext surface temps, stored energy), all **bitwise equal**. The
  coupling restore itself (`last_coupling`) is correct: the direct solves
  after restore match.
- **Validation.** 20 malformed snapshots all return Err with no panic, and the
  solver's full `Debug` is unchanged. Cases: x short/long/inf; `last_u`
  empty/long/-inf; lwr long; interior outer empty/long; inner short/long;
  prev NaN; coupling idx = n and `usize::MAX`; d NaN/inf/-0.1; forcing NaN; a
  good coupling followed by a bad one; and a combined bad case.
- **v8 rejection.** A real v9 checkpoint rewritten into the v8 layout with
  `format_version: 8` gives `checkpoint version mismatch: file=8, expected=9`
  through `deserialize_gated`. An in-memory `format_version = 8` passed to
  `load_checkpoint` gives the same error. The v8 layout labelled 9 gives a
  parse error (`missing field thermal`).
- **Free-float oracle change.** The HPXML copy goes in a per-call
  `tempfile::tempdir()`: unique, removed on drop and during unwinding. No
  stray dirs remained after the parallel run. Equivalence was checked with
  `freefloat_fixed_windows_equivalent_to_nulled_natvent`: the old path (build
  from the original HPXML, null `natural_ventilation`) and the new path
  (FractionOperable 0) build identical configs except `natural_ventilation`.
  The solver state outside config, `x0` (bitwise), the humidity solver and
  `latest_env` are also identical. HashMap order was neutralized by comparing
  line multisets. `fraction_operable` has one consumer (`solver_builder.rs:719`,
  natural-vent opening area) and `initialization_duration` is None, so there
  is no warm-up. The oracle passes 6/6 (`--features observe --release`).
  Indoor MAE: spring 0.518, summer 2.154, winter 2.736 °C. Attic MAE: 2.398,
  2.896, 1.189 °C.
- **Allocation.** `radiant_gain_weights_zero_alloc` passes in debug and
  release (three solves per zone, coupled and uncoupled). `profiling_alloc`
  passes 2/2 in debug and release.
- **Goldens.** `frame_golden compare-all` (release): 5/5 identical.
  `-F dst compare-all`: 1/1 identical.
- `checkpoint_regressions` 6/6 (release, including
  `resumed_run_equals_continuous_run`). `conditioned_oracle` 6/6
  (`--features observe --release`). The existing `checkpoint::tests` suite
  passes.
