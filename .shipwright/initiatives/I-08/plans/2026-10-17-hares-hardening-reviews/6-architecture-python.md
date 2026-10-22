# Review 6/6: Architecture, performance, Python surface

Read-only architecture/performance review, post-I-08. No builds/tests run; all claims grounded in file reads with citations. Read `docs/architecture.md`, the full I-08 record set (`initiative.md`, `implementation.md`), all crate manifests; sampled the step pipeline, env manager, envelope solver, fleet, PyO3 layer, `python/hares`. I-08 already landed zero-alloc thermal solver buffers, port accumulator reuse, env-manager buffer swap (`environment.rs:1434-1506`), unconditional invariant enforcement (OR-5), allocation accounting (PR-3/PR-6), output row-plan work - none re-proposed.

## MAJOR

**M1. `hares-io` is three crates in one: parsers + the entire output/analytics stack + the engine's config type.**
- `crates/hares-io/src/hpxml/` totals 32,414 lines (parsers); `crates/hares-io/src/output/` totals 12,428 lines, including `output/metrics.rs` (4,149 lines of derived-metrics business logic, e.g. `MetricsCalculator`, `SimulationMetrics` at `output/metrics.rs:84-301`) and `output/catalogue.rs` (2,989 lines); `SimulationConfig` is defined in the IO crate (`crates/hares-io/src/config.rs:240`) and re-exported by core (`crates/hares-core/src/lib.rs:60`).
- `hares-io` depends on `hares-envelope`, `hares-equipment`, `hares-control` (`crates/hares-io/Cargo.toml:14-17`), so it sits above every domain crate and "io" is really "everything that isn't physics" (77.7k src lines). `arrow`/`parquet` are unconditional deps, so every consumer - including RL/gym builds that never record - compiles and links Arrow+Parquet. Metrics under "io" inverts the dependency story in `docs/architecture.md:71`.
- Recommendation: split into `hares-input` (hpxml/epw/resstock/schedule/defaults/config) and `hares-output` (arrow/parquet/writer/catalogue/reduce/metrics); `SimulationConfig`+`WarmupConfig` into input, core re-export as a compat shim. Payoff: input-only builds for RL fleets, honest boundaries, parallel compile units. Effort M (module move, no logic change; golden/frame_golden gates already pin behavior).

**M2. The `Dwelling` god object: one file, one struct owning recording bindings, dispatch, warm-up, LUTs, observation, billing, profiling.**
- `crates/hares-core/src/dwelling/mod.rs` is 23,885 lines (production code to `mod tests` at `:9287`, ~14.6k lines of inline tests). `Dwelling` (`:2525-2850`) owns `thermal_solver` + three more solvers, `ports`, `control_dispatcher` (`:1716`), the Arrow recording layer (`EquipmentColumns :591`, `DwellingColumns :1070`, `OutputRowBindings :1223`, `ZoneColumnCaches :1579`, `DiagnosticsBindings :1278`), warm-up (`run_warmup_converged :6141`), battery/EV LUT mutation (`:4819-4926`), observation planning (`:5276-5333`), profiling (`:420-517`), checkpointing.
- Every new output column, warm-up rule, or control feature edits the same 9k-line file; the 67 public methods are the de-facto API surface with no internal boundary; any recording tweak recompiles the 87k-line crate.
- Recommendation: extract within the crate first (no API break): `RecordingSet` (all `*Columns`/`OutputRowBindings`/`ZoneColumnCaches` + `run_timestep`'s record tail), `WarmupRunner` (`:3147-4337` region), `ControlDispatcher` to its own file. Then M1's `hares-output` split lets recording bindings migrate. Effort M; payoff high - calibration and gym additions target narrow types instead of `Dwelling`.

**M3. Fatal-error classing across the PyO3 boundary is done by string-prefix sniffing.**
- `pub(crate) const FATAL_DWELLING_PREFIX: &str = "[FATAL_DWELLING] "` (`crates/hares-python/src/py_dwelling.rs:630`); classification via `e.starts_with(FATAL_DWELLING_PREFIX)` in the parallel batch path (`py_gym.rs:823`); `step_core_string` returns `Result<_, String>` across Rayon threads (`py_dwelling.rs:2468`).
- This is the error-surfacing contract for the whole RL/fleet path. A rename, a user message legitimately beginning with the prefix, or a warning embedding it silently reclassifies a transient failure as a permanently-poisoned dwelling (or vice versa). A typed `HaresError` exists on the single-step path (`py_dwelling.rs:1013-1017`); the String+prefix exists only because `HaresError` wasn't Send-carried through the batch closure.
- Recommendation: `struct StepFailure { msg: String, fatal: bool }` (or a Serialize enum) crossing threads; keep the Python exception mapping. Effort S. Verify with the existing tests at `py_dwelling.rs:3576-3626` plus a prefix-collision test.

**M4. Vectorized batch path rebuilds per-step Python-visible strings and dicts that the single path already caches.**
- `batch_step_py` calls `zone_keys_for_step(&step)` inside the per-dwelling, per-step result loop (`py_gym.rs:864-865`); `zone_keys_for_step` does `format!("Temperature - Zone_{} (C)", ...)` per zone per call (`py_dwelling.rs:637-644`). The single path caches them once (`self.zone_keys`, `py_dwelling.rs:1038-1045`). The batch path also unconditionally builds the `"step"` dict per dwelling per step (`py_gym.rs:862-868`) though the observation buffer (`obs_out`, `:655-696`) is the fast channel RL consumers actually read. `info` values forced into `Vec<(&'static str, f64)>`, so `warning_count` is a float (`:761, 792`).
- For a 100-dwelling x 525,600-step episode: ~52.5M `format!` allocations plus 5.25M dicts built and dropped, on the GIL thread after every batch step - the largest remaining RL hot-path cost, not covered by I-08 (`benches/rl_step.rs`, `benches/rl_episode.rs` exercise the Rust side).
- Recommendation: (a) cache zone keys per dwelling (field exists, `py_dwelling.rs:718`); (b) `include_step_dict: bool = False` on `batch_step`; (c) warnings as a real list, count as int. Effort S. Gate with before/after bench numbers per the project's own rule (`crates/hares-envelope/benches/thermal_step.rs:1-3`).

**M5. Single-dwelling gym env builds full telemetry dicts every step even with the default reward.**
- `python/hares/rl/gym_env.py:678` - `telemetry = self._dwelling.telemetry()` runs unconditionally in `step()`; consumed only inside the `else` branch at `:684-692` (`reward_fn is not None`). Observation already uses the frozen-plan fast path (`:574-578`).
- `Dwelling.telemetry()` (`core dwelling/mod.rs:5043`) assembles zone + equipment telemetry dict trees per step; with the default `-net_power` reward this is built and discarded 525,600 times per episode. Make it lazy. Effort S.

**M6. Per-step validation and port snapshotting run in every release step - deliberate (I-08 OR-5), but never isolated in a bench.**
- Per equipment per step: `self.rollback_ports.copy_into(&self.ports)` (`dwelling/mod.rs:7122`), `validate_equipment_contract(idx)` (`:7130`), `validate_port_core_electrical_consistency(...)` (`:7131-7136`); repeated for Electrical (`:7197-7211`) and Thermal stages. OR-5 made these unconditional in every profile (`implementation.md:51`).
- The right correctness call, but no bench isolates it (`benches/single_building.rs` measures whole runs). Don't remove it - measure it: a criterion case with validation short-circuited as the comparison arm; record the delta in the I-08 measurement style. If < 1-2%, close the question permanently. Effort S.

**M7. The step-start `equipment_core` presence check does a hash lookup per equipment per step while its comment claims it doesn't.**
- `dwelling/mod.rs:6626-6645`: "no per-equipment hash lookups on the step path" followed by `self.latest_env.equipment_core.contains_key(&id)` per id (`equipment_core` is `HashMap<EquipmentId, CoreOutput>`, `hares-types/src/environment.rs:546`). Equipment ids are dense u32s (`dwelling/mod.rs:2536-2541`); a roster-sized `Vec<bool>` mirror makes the comment true. Effort S.

**M8. `batch_step` actions cross the boundary as `Vec<Vec<f64>>` and each conversion rebuilds lowercased HashMaps.**
- `py_gym.rs:634` signature; `build_control_signal` does `values.iter().map(|(k, v)| (k.to_lowercase(), *v)).collect()` per call (`:169`), invoked per dwelling per step while the GIL is held. Asymmetric with the observation path, which already uses a zero-copy `PyBuffer<f64>` (`:108-139`).
- Recommendation: accept an optional 2D C-contiguous f64 buffer (numpy already a dependency, `crates/hares-python/Cargo.toml:36`); hoist the lowercase map into the action layout resolved once per env. Effort S-M.

## MINOR

- **m9.** `hares-types` is drifting from leaf contracts toward a grab-bag: `equipment.rs` 4,285 lines carrying EV charging, BMS scheduling, battery-chemistry vocabulary (`equipment.rs:436-961`); `zip.rs` (982 lines) is a physics model ported from OCHRE (`zip.rs:1-15`) used only by equipment crates; `panic_hook.rs` (554 lines) and `alloc_count.rs` (107 lines) are runtime plumbing in the leaf; `telemetry_keys.rs` (1,582 lines) is a flat string-constant namespace. Move `zip.rs` -> `hares-physics`; `alloc_count`/`panic_hook` behind `profiling` or into core. Effort S.
- **m10.** `observe` feature costs 513 `cfg` sites with duplicated inline branches in the step pipeline (core 181, envelope 100, equipment 95, io 39, types 14; parallel observe/non-observe branches at `dwelling/mod.rs:6760-6768` and `:6986-7001`). Zero runtime cost when off, but each pipeline edit is made twice. Prefer `#[cfg]`-gapped helper methods so the pipeline body reads once. Effort S, incremental.
- **m11.** Envelope state-space uses dynamic `DMatrix`/`DVector` and HashMap index maps where residential networks are tiny and dense: `a_c: DMatrix<f64>`, `b_ext: DMatrix<f64>` (`boundary_rc.rs:428-430`); `solar_input_indices: HashMap<SurfaceId, usize>` documented as hot-path lookups (`docs/architecture.md:764-771`). Typical networks n ~= 10-30 states; a flat row-major `Vec<f64>` gemv and `Vec<Option<usize>>` keyed by dense surface index would avoid heap-backed dynamic matrices and hashing. Probable, not measured - bench-first on `thermal_step.rs`; adopt only on a measured win. Effort M.
- **m12.** `PyFleet::simulate` builds a fresh Rayon thread pool on every call (`fleet.rs:686-690`). Negligible for whole-horizon runs, real for RL/DR callers doing many short fleet episodes. Cache the pool keyed by thread count or accept an external `&ThreadPool`. Effort S.
- **m13.** `ReportOutput::Memory` accumulates Arrow batches without bound over long stepped runs: `dwelling/mod.rs:4445` `recorder.flushed_batches().to_vec()` (retained batches `:5880-5882`); the file writer is chunk-bounded (`hares-io/src/output/writer.rs:6-7`) but the in-memory destination is inherently unbounded. A calibration loop leaving default recording on and stepping a year in-process holds the whole year of columns in RAM per dwelling; in a 100-dwelling vector env that multiplies. Document loudly + `max_memory_rows` soft cap erroring with remediation. Effort S.
- **m14.** `docs/python.md` carries ~225 lines of release notes inside the reference doc (`:435-659` of 895). Move to CHANGELOG.md. Effort S.
- **m15.** Residual process-ledger references in shipped-code comments (SL-3 tail): `dwelling/conversions.rs:89`, `:190`, `:3417`; `dwelling/solver_builder.rs:96`. Otherwise the cleanup is in good shape: exactly one `allow(dead_code)` in the workspace, feature-justified (`hares-io/src/output/metrics.rs:1903`). Rewrite the comments to state the rule. Effort S.
- **m16.** Gym `info` contract leaks Rust internals: `warning_count` is a float (`py_gym.rs:792`, documented `gym_env.py:668-670`); `"seed"` present as `0` when unset (`gym_env.py:698-701`). Int count; `None`/omit when unset. Contract change - batch with M4 behind a version note.

## Ranked rearchitecture roadmap
| # | Action | Effort | Payoff | Why this order |
|---|---|---|---|---|
| 1 | Split `hares-io` -> `hares-input` + `hares-output`; `SimulationConfig` with input; core re-export shim preserved | M | High | Unblocks M1/M2; pure module moves pinned by the golden suite; makes input-only RL builds possible |
| 2 | Extract `Dwelling`'s recording layer (`*Columns`, `OutputRowBindings`, `ZoneColumnCaches`, `DiagnosticsBindings`, record tail) into a `recording` module, then into `hares-output` | M | High | Shrinks the god file by ~2-3k production lines; narrows the surface calibration/gym consumers touch |
| 3 | Python batch-path fast pass: cached zone keys, `include_step_dict=False`, lazy telemetry in `gym_env.py`, int warnings | S | High (RL) | Small, independent, benchmark-verifiable with the existing `rl_episode` bench; ships before structural work |
| 4 | Typed step-failure across the batch boundary replacing `FATAL_DWELLING_PREFIX` | S | Medium-high | Removes the one fragile error contract; prerequisite for trusting large fleet RL runs |
| 5 | Extract `WarmupRunner` (region `:3147-4337` + `:6141`) and the LUT setters into a `DwellingTuning` facade | S/M | Medium | Calibration consumers get a narrow tuning API |
| 6 | `hares-types` hygiene: `zip.rs` -> physics; profiling modules behind feature | S | Medium | Stops leaf drift before it ossifies |
| 7 | Envelope dense-index + small-matrix experiment, bench-gated on `thermal_step.rs` | M | Medium | Only after 1-6; adopt on measured win, otherwise close with a recorded number |
| 8 | Fleet pool reuse + memory-destination cap/doc | S | Low-medium | Opportunistic batch |

Deliberately not on the roadmap: merging `hares-control` (1.4k lines, harmless); breaking up `hares-equipment` (139k src lines but cohesive per-equipment modules); any SoA rewrite of equipment state (dwellings are independent - Rayon across dwellings already delivers scaling, `fleet.rs:647-651`; SIMD inside one home is second-order).

## Concrete API additions

### Calibration tooling
Primitives exist: overrides reach typed configs (OD-42, `py_config.rs:793-797`), `ObservationPlan`/`observe_into` give on-bits column reads (`dwelling/mod.rs:5276-5333`), `attach_callback` exists (`:5905`). Missing: a coherent residual-evaluation surface.

```python
# New: hares/calibration.py (thin layer over the existing Rust surface)
class CalibrationDwelling:
    """One HPXML premise, reused across parameter evaluations via the blueprint."""
    def __init__(self, hpxml: str, schedule: str, weather: str, **kwargs): ...
        # builds PyDwellingBlueprint once (already GIL-free: py_dwelling.rs:4238-4240)
    def evaluate(self, overrides: dict[str, Any],
                 observation_fields: Sequence[str],
                 periods: Sequence[tuple[dt, dt]] | None = None,
                 warmup: bool = True,
                 seed: int | None = None) -> CalibrationRun: ...
    class CalibrationRun:
        frame: pl.DataFrame | pd.DataFrame   # columns = observation_fields, from RunResult table
        metrics: Mapping[str, float]         # from hares-output MetricsCalculator
        health: RunHealthProxy               # RunHealth in RunResult (implementation.md:157-164)
        warnings: Sequence[Warning]
```

Rust-side additions (small, after roadmap items 2/5):
```rust
// PyDwelling
fn evaluate_overrides(&mut self, py: Python<'_>, overrides: HashMap<String, serde_json::Value>,
                      fields: Vec<String>) -> PyResult<PyCalibrationRun>;
//   = reset_with_seed + simulate() + observation_plan/observe_into over the frozen table,
//     all detached; returns Arrow IPC (pyo3-arrow already a dep) not dicts.
fn warmup_residuals(&self) -> Option<WarmupResiduals>;  // hares-core/src/health.rs exports the type
```
Key design point: overrides rebuild from the blueprint, never mutate in place (`reset_with_seed` already has the right semantics, `py_dwelling.rs:1872-1901`), so a calibration worker can evaluate parameters in parallel processes with bitwise frame-equality as the regression gate.

### DR/RL gym
Existing: episode reset/seed (`reset_with_seed`, `reseed`, `load_state` - `py_dwelling.rs:1872-1904`), buffer-protocol observations, GIL-free Rayon batch step, clamping + bounds. Additions, in priority order:

```python
# 1. Rust batch_step signature (py_gym.rs:632) - perf + contract (M4, M8, M16):
def batch_step(dwellings: list[Dwelling],
               actions: np.ndarray | None,        # CHANGED: (N, A) C-contiguous f64 buffer
               plans: list[ObservationPlan],
               action_layout: list[tuple[str, str]],
               signal_type_by_equipment: dict[str, str],
               action_bounds: list[tuple[float, float]],
               obs_out: np.ndarray,               # (N, F) unchanged
               include_step_dict: bool = False,   # NEW: default False; RL pays zero dict cost
               ) -> list[dict]                    # terminated, truncated, info(int warning_count,
                                                  # warnings: list[str], error_msg?) [+ "step" iff flag]

# 2. Episode/seed lifecycle made explicit on the Rust side:
class Dwelling:
    def snapshot_state(self) -> bytes: ...            # DwellingCheckpoint bytes (checkpoint.rs exists)
    def restore_state(self, state: bytes, seed: int | None = None) -> None: ...
    def __getstate__(self) -> bytes: ...              # pickle == snapshot_state; enables
    def __setstate__(self, state: bytes) -> None: ... #   multiprocessing vector envs

# 3. python/hares/rl/vec_env.py - masked auto-reset with per-env seeds from a caller SeedSequence:
class VecDwellingGymEnv:
    def __init__(..., seed_sequence: np.random.SeedSequence | None = None): ...
    def reset(self, *, mask: np.ndarray | None = None) -> np.ndarray: ...
        # only rebuild masked envs from their blueprint (reset_with_seed is detached: py_dwelling.rs:4407)
```

For EnergyPlus/OCHRE-crossing researchers: the one gap worth closing is a documented unit/column contract page mapping OCHRE `CoreFlows`/HPXML names to HARES telemetry keys (`hares-types/src/telemetry_keys.rs` is the machine-readable source; generate the table into `docs/python.md` at build time).

**Bottom line:** the engine's Rust hot path is in good shape post-I-08 (buffer-swap discipline is real and visible in `environment.rs:1434-1506` and the thermal solver); the highest-leverage remaining work is (1) the `hares-io` split, (2) carving the recording layer out of `Dwelling`, and (3) a small set of Python-boundary fixes (M3-M5, M8) the existing benches can verify immediately.
