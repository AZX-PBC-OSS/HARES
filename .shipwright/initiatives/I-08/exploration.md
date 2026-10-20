# I-08 Exploration: user needs and current pipeline behavior

What HARES users need from output, telemetry reporting, runtime and memory, and how the current
pipeline behaves. Checked against the HARES tree at `ebdb1a17`. Where a need conflicts with
`decision-log.md`, only the decision log's position is stated; decisions live in
`decision-log.md`, and `review.md` holds the reasoning behind them and the confirmed defect table
(V1-V50) those needs led to.

## 1. User classes and their requirements

Priority follows each source's own scale (MUST/SHOULD/COULD for the calibration consumer,
M/S/C for every other class). Acceptance criteria are the observable conditions that would
satisfy the requirement; several are already met by the decisions in `decision-log.md`.

### 1.1 Calibration consumer

The calibration system (`~/src/HARES_CALIBRATION_SYSTEM`) runs HARES as a black-box forward model
inside a differential-evolution loop: about 412-514 full-year simulations per fit, each reduced to
one output column, `Total Electric Power (kW)`. About 99% of each evaluation's wall time is
`simulate()`.

| ID | Priority | Requirement | Acceptance criterion |
|---|---|---|---|
| R1 | MUST | Full-fidelity in-memory results without a disk sink; disabling file output must not truncate the frame or disarm health checks | A no-sink run on the study home writes zero bytes; its frame hash equals the `write_output=True` run; health is still recorded: an injected NaN or unresolved column fails the run with a typed error |
| R2 | SHOULD | Column projection independent of the verbosity ladder; an unknown column name errors naming the valid set | A run projected to one column returns that column plus health, bitwise equal to the same column from a full-verbosity run, with no added wall time |
| R3 | MUST | Health and invariants are a first-class, typed, always-on result, never disabled as a side effect of an I/O setting | Health is readable with no output columns requested; an injected NaN fails the run in every build with a typed error naming the step and the zone; `HealthPolicy::strict()` escalates the counted events |
| R4 | MUST | A machine-readable, versioned catalogue of output columns and metrics, queryable without simulating; end-use rollup by declared end use | A renamed column in a test build reports a version change; a single-PV home's `PV` end use excludes `Other` |
| R5 | MUST | "Accepted implies applied": any input the engine accepts must move the resolved model or be rejected naming the key | `{'ASHP Heater': {'__nonsense__': 1.0}}` raises; a malformed `charging_strategy` raises |
| R6 | MUST | Read-back of resolved parameters (capacity, EIR/COP/AFUE, effective setpoints, infiltration, window U/SHGC, effective R/C, usage multipliers, PV nameplate), available without simulating | Scaling HPXML HSPF by 0.7 changes the read-back EIR in the expected direction |
| R7 | SHOULD | An in-memory parameterisation path equivalent to the file routes, deterministic, isolated, bitwise equal to the file-edit route for the same value | A full DE fit runs with zero files written under scratch, and pinned losses reproduce |
| R8 | SHOULD | A machine-readable input schema (equipment fields, kwargs, schedule columns, HPXML elements, defaults tables), enumerable from the API alone | The consumer's field classification can be driven from the API with no access to HARES source |
| R9 | SHOULD | Engine-enforced feasibility is queryable in milliseconds, before simulating | A validation call rejects an AFUE of 1.33 in under 100 ms with the same message as at run time |
| R10 | MUST | Loud, typed, classifiable, process-safe errors; no silent fallback on parse failure | Every exception type round-trips through `pickle.dumps`/`pickle.loads` in a spawned child |
| R11 | MUST | No silent run-configuration defaults; `config=` plus sibling kwargs is an error; path resolution is independent of the working directory | `from_hpxml(..., config=cfg, start_time=...)` raises; the same run from two working directories gives the same frame hash or raises |
| R12 | MUST | Bitwise determinism across runs, processes, thread counts and pool arrangement, including below-grade foundations | 20 cross-process runs of a below-grade home give one whole-frame hash |
| R13 | SHOULD | Numerics-change transparency: every change that can move output for unchanged input is identifiable before a pin bump | Release notes state whether the reference-home frame hash changed between two tagged versions |
| R14 | MUST | Self-describing build provenance (commit, dirty flag, build profile, features, API/schema versions), queryable at runtime | `build_info()` returns a commit equal to a clean build's `git rev-parse HEAD` |
| R15 | SHOULD | A documented Python API stability policy, including the package-name decision | A tagged release carries a changelog section listing every added, changed or removed callable |
| R16 | SHOULD | A reproducible benchmark matching the calibrator's shape, with regressions flagged; profiling does not perturb what it measures | Phase shares from `profiling_summary()` sum to within 5% of release wall-clock time without the profiler |
| R17 | SHOULD | Construction and simulation release the GIL for thread-level parallelism, or the engine documents process-level parallelism as the supported model; no engine call creates files or depends on the working directory | 8 threads each running a distinct dwelling reach at least 6x the throughput of 1 thread, with frame hashes identical to serial runs |
| R18 | SHOULD | Flat per-process memory across hundreds of runs in one worker | PSS/RSS after run 500 is within 1% of after run 50 |
| R19 | COULD | Batched multi-variant evaluation returning per-member series bitwise equal to individual runs | One call over 30 variants returns 30 series with hashes equal to 30 individual runs |
| R20 | SHOULD | Documented, observable warm-up semantics; the output frame contains exactly the requested window | A test asserts the frame's first timestamp equals `start_time` and its row count equals `duration / time_res` |
| R21 | MUST (docs) / SHOULD (API) | Documented time semantics (instant vs. interval, weather-to-step mapping, schedule indexing, local-time convention); an API to read the engine's own resampled weather channels | A resampling API returns an OAT series bitwise equal to a full run's `Outdoor Dry Bulb (C)` column at 900 s and 3600 s |
| R22 | SHOULD | Replay: export a run's fully resolved configuration plus input hashes; re-running from that export reproduces the frame hash | A `RunResult`'s export reproduces its frame hash after the original scratch inputs are gone |
| R23 | SHOULD | A DER instantiated from HPXML is live or loudly inert; PV/battery/EV power units are consistent or range-validated | `MaxChargingPower=11500` (a watts-for-kW mistake) is rejected as out of range |
| R24 | COULD | Correct ResStock `sample_weight` diagnosis, distinguishing "column missing" from "building missing" | A single-building fetch against 2025.1 metadata raises no misattributed warning |
| R25 | COULD | A stable co-simulation surface (`step`, `save_state`, `load_state`, `telemetry`); any fleet stepping surface documents whether it supports rewind | `save_state`/`load_state` round-trips to an identical next step |

### 1.2 Python API users and researchers

| ID | Priority | Requirement | Acceptance criterion |
|---|---|---|---|
| PY-1 | M | Output is identical whether the run was driven by `simulate()` or by `step()` | After N `step()` calls plus finish, results and metrics are bit-equal to `simulate()`, for N below and not a multiple of the chunk size |
| PY-2 | M | Full results are available in memory with no file I/O | A run configured without a file sink creates no file anywhere, diagnostics included |
| PY-3 | M | No implicit writes to the working directory; path collisions cannot happen silently | An empty temp working directory stays empty unless a path was given; colliding paths raise or require an explicit overwrite |
| PY-4 | M | `Time` is a typed, tz-consistent datetime with a documented interval-labelling convention | `df["Time"].dtype` is `pl.Datetime` on every path |
| PY-5 | M | A missing value is distinguishable from zero | A column not applicable to a step is null, not `0.0` |
| PY-6 | M | Config conflicts and unknown keys raise; stubs match the bindings | `config=` plus a conflicting kwarg raises; `test_py_stubs` covers every settable field |
| PY-7 | M | Metrics do not depend on retention or on output verbosity | `RunResult.metrics`, when requested, is identical whatever the selected reports and destinations; total and end-use kWh are available at every verbosity |
| PY-8 | S | Selection by variable or pattern, plus tiers, with a pre-run catalogue of columns, units and semantics | An unknown name raises and names close matches |
| PY-9 | S | Configurable reporting frequency with per-variable aggregation | Hourly kWh sum equals the timestep integral to 1e-9 relative |
| PY-10 | S | Units and variable semantics are machine-readable in Python | Every column has a unit and kind lookup; mode codes decode to names |
| PY-11 | S | Retrieval cost scales with what is requested; repeated reads do not re-copy | A second `results()` call does no O(rows) re-serialisation |
| PY-12 | S | A metrics function that works on any results frame | Computing metrics over a loaded file matches the in-run metrics |
| PY-13 | C | Opt-in provenance (config, version, seed, weather hash) alongside file output | The provenance file round-trips to an identical `SimulationConfig` |
| PY-14 | C | Docs cover `initialize()`/lifecycle, the output sink and dtypes | The docs example and notebook cells run as written in CI |

### 1.3 RL and control users

| ID | Priority | Requirement | Acceptance criterion |
|---|---|---|---|
| RL-1 | M | The step path does no file I/O and retains nothing unless requested | A gym or vec environment built with defaults creates no file; RSS stays flat over 10^6 steps |
| RL-2 | M | Per-step bookkeeping (step history, retained batches) is bounded or opt-in | Memory after 100 episodes equals memory after 1 episode |
| RL-3 | M | Reset restores a captured initialised state and re-seeds without rebuilding | Reset costs well under 10% of construction; reset plus the same actions gives bit-identical observations |
| RL-4 | M | Restore has defined semantics for recorded output and metrics: discard, fork, or error, never silent interleaving | After a mid-run restore, output timestamps are monotonic or an explicit error is raised |
| RL-5 | M | One reward contract across the Rust and Python paths | The same `reward_fn` gives the same rewards with and without the fast path |
| RL-6 | S | Observation fields resolved once into an index plan, written into a reused buffer | Zero heap allocations per step for a fixed field set |
| RL-7 | S | Every benchmark states its output configuration explicitly with distinct building ids | No two bench dwellings share a path |
| RL-8 | C | A per-step telemetry subset: only requested channels materialise | The cost of `telemetry()` scales with the number of requested fields |

### 1.4 Fleet and ResStock users

| ID | Priority | Requirement | Acceptance criterion |
|---|---|---|---|
| FL-1 | M | Fleet memory is bounded: each home folds into the aggregate as it completes | Peak RSS at N=10^4 is independent of N |
| FL-2 | M | Default fleet configs produce metrics and aggregates, or fail loudly | `from_resstock(...).simulate()` with defaults returns non-zero metrics or raises naming the cause |
| FL-3 | M | Aggregation handles heterogeneous columns with per-column coverage | A mixed battery/no-battery fleet gives a non-empty aggregate matching a hand-computed weighted sum |
| FL-4 | M | Results report coverage: excluded homes with reasons, weight covered versus total; a flagged home is never a success | The result carries `weight_covered/weight_total` and a per-home status table |
| FL-5 | M | Parallel homes never write to the same output path | Two homes with the same effective path raise, or paths are unique by construction |
| FL-6 | M | Fleet time is local building time, with no fixed UTC offset baked in | The ResStock config start uses local wall-clock time |
| FL-7 | S | Aggregated columns and resolutions are user-selectable | The aggregate schema equals the requested set, reduced by each variable's kind |
| FL-8 | S | Timestamps are typed end to end, no string parsing | The aggregation benchmark shows no string-parse cost |
| FL-9 | S | `SteppableFleet` returns step results without full telemetry or retained output | Per-step cost does not grow with history length |
| FL-10 | S | Benchmarks at 10^3 and 10^4 homes, including peak RSS | A documented bench target reports peak RSS and per-home throughput |
| FL-11 | C | Optional per-home files as Parquet partitioned by building id | A partitioned dataset reads back to the in-memory per-home results |

### 1.5 Rust library users

| ID | Priority | Requirement | Acceptance criterion |
|---|---|---|---|
| RS-1 | M | Metrics and results are obtainable with no filesystem side effects | With file output off, `engine.run` returns correct metrics with status `Ok` |
| RS-2 | M | No implicit working-directory output from any constructor, diagnostics included | `from_hpxml` plus `simulate` in an empty temp directory leaves it empty |
| RS-3 | M | Strict config in every config type, no silent numeric fallbacks | A typo in a config key fails to parse, naming the key |
| RS-4 | M | Failures keep their type across the engine boundary | A test matches `InvariantViolation{check_name: "thermal_balance"}` from `engine.run` |
| RS-5 | S | Result types distinguish "not requested", "requested but empty" and "present" | A streaming run's timeseries field reports "not retained", not `Some(vec![])` |
| RS-6 | S | Structured warnings and degradations, including warm-up and init | Warm-up non-convergence appears in the results |
| RS-7 | S | Step history is bounded or opt-in | Peak RSS of a streaming one-year run is independent of step count |
| RS-8 | S | A documented way to evaluate one parsed building repeatedly, with defined reset semantics | N evaluations of the same blueprint match N independent `run` calls, without re-parsing inputs |
| RS-9 | C | A working CLI, or corrected documentation; a minimal Rust example | The example compiles and runs in CI |

### 1.6 Developers and maintainers

| ID | Priority | Requirement | Acceptance criterion |
|---|---|---|---|
| DV-1 | M | Health counters are scoped to a run and returned with its results | Two concurrent runs, one with injected non-finite telemetry, report different counts |
| DV-2 | M | A real determinism check: bit-identical full timeseries and metrics across repeat runs, parallel fleet execution and restore-then-replay | The test fails when a nondeterministic hash-iteration order is injected |
| DV-3 | M | Observability channels keep their full content when enabled and cost nothing when disabled | Bench output-off numbers show no diagnostics cost |
| DV-4 | M | Tests produce no stray files in the repository tree | `git status --ignored` shows no new `dwelling_*.csv` after the test suite |
| DV-5 | S | Diagnostics and observer output can be captured in memory, activated independently of timeseries verbosity | Diagnostics can be enabled at verbosity 0 |
| DV-6 | S | Benchmarks separate simulation cost from output-encoding cost | Distinct bench groups: no output, in-memory, CSV, Parquet |
| DV-7 | S | Profiling summaries are returned as data | A test reads the per-subsystem split from the result |
| DV-8 | S | Output schema contract tests replace name-string classification | Adding an equipment type without column wiring fails a test |
| DV-9 | C | Fix documentation drift; remove dead config fields | Parsing `wall_area_m2` errors |

### 1.7 Cross-cutting requirements (all classes)

| ID | Priority | Requirement | Acceptance criterion |
|---|---|---|---|
| X-1 | M | Metrics, the in-memory frame and file writing are independent opt-ins | Every combination (no file with metrics; file without retention; memory only) returns identical metrics |
| X-2 | M | Nothing is written anywhere unless a sink is requested | Construction plus a default run leaves an empty temp working directory untouched |
| X-3 | M | Simulate-driven and step-driven runs finalise identically | Stepped and simulated runs produce identical outputs |
| X-4 | M | No O(steps) structure grows unless requested | RSS is flat over run length with defaults |
| X-5 | M | One output schema contract across frame, file and fleet aggregate | Schema equality holds across Parquet, CSV, polars and fleet paths |
| X-6 | M | Restore and reset semantics for output, metrics, billing and history are defined and tested | Restore-then-replay is bit-identical, with no interleaved rows |
| X-7 | M | Strict, single-source config across TOML, Python and synthetic front ends | One default table, tested across all three front ends |
| X-8 | M | Typed failures and coverage markers; a zeroed metric is never a success | Flagged zero-metric homes are not counted as successes |
| X-9 | S | Selection by variable and reporting frequency, independent of the physics being run | A subset request plus a frequency gives exactly those columns at that frequency |
| X-10 | S | Output cost scales with what is selected | Per-step time at "metrics only" is within noise of "no output" |
| X-11 | S | An extension seam for custom equipment/actor output channels | A Python-defined channel appears in frames and files with units |
| X-12 | S | OCHRE comparison through the Python adapter's mapping; no OCHRE preset in the Rust core (`ochre_compat`) | Parity tests pass using the adapter's held weather in place of the removed Rust-core preset |

Per decision-log's `new_capability` entry, Python-defined equipment does not exist and is not
added; X-11 is served by Python actors declaring their output variables through the actor API,
not by a general Python equipment mechanism.

### 1.8 Requirement-to-ledger-entry table

Every requirement ID above resolved to the ledger entry (`plan.md`'s `ledger/*.md`) that serves
it, or to the named extension seam for one deferred. Requirement IDs that are spelled like a
ledger entry ID (`requirement PY-n`, `requirement FL-n`) are written with that prefix to
distinguish them from the ledger entry of the same name.

**Calibration consumer (R1-R25)**

| ID | Entry |
|---|---|
| R1 | OR-3, OR-5 |
| R2 | OR-1, OR-2 |
| R3 | OR-5, OR-12, PY-9 |
| R4 | OR-1, B-5 |
| R5 | OD-2, OD-22, B-15 |
| R6 | RT-6, B-14 |
| R7 | RT-4 (content-input seam); CN-1, CN-10 partial |
| R8 | IN-3 |
| R9 | IN-2 (measured against the 100 ms target) |
| R10 | OD-1 |
| R11 | PY-2, B-10 |
| R12 | RT-5, B-8 |
| R13 | plan.md §2.8 (numeric-fixes design frame) |
| R14 | PY-6 |
| R15 | PY-9 (`docs/python.md` changes section) |
| R16 | PR-1 to PR-6, BR-10 |
| R17 | PY-3, RT-5, CN-10 |
| R18 | CN-10, RT-1, M-3 |
| R19 | plan.md §2.10 seam (`DwellingBlueprint::build(&self)` over RT-4's shared parse) |
| R20 | B-2, BR-11, OD-7; docs partial until B-2's warm-up section lands |
| R21 | IN-4, OR-7 |
| R22 | OR-16, CN-11 |
| R23 | OD-8, IN-9 |
| R24 | PY-12 |
| R25 | OR-10, RT-8 |

**Python API users (requirement PY-1 to requirement PY-14)**

| ID | Entry |
|---|---|
| requirement PY-1 | PY-9, OR-6 |
| requirement PY-2 | OR-3, PY-2 |
| requirement PY-3 | OR-3, B-10 |
| requirement PY-4 | OR-7 |
| requirement PY-5 | B-6 |
| requirement PY-6 | PY-2, PY-8 |
| requirement PY-7 | OR-8, CN-7 |
| requirement PY-8 | PY-4, OR-1 |
| requirement PY-9 | OR-13, PY-9 |
| requirement PY-10 | PY-4 |
| requirement PY-11 | PY-1 |
| requirement PY-12 | OR-15 |
| requirement PY-13 | OR-16, CN-11 |
| requirement PY-14 | PY-9, OR-7 |

**RL and control users (RL-1 to RL-8)**

| ID | Entry |
|---|---|
| RL-1 | OR-3 (`test_gym_default_writes_no_file`) |
| RL-2 | OR-6, OR-3 |
| RL-3 | RT-8 |
| RL-4 | OR-10 |
| RL-5 | PY-5 |
| RL-6 | RT-7 |
| RL-7 | PR-3 (`benches/rl_episode.rs`) |
| RL-8 | RT-7 |

**Fleet and ResStock users (requirement FL-1 to requirement FL-11)**

| ID | Entry |
|---|---|
| requirement FL-1 | FL-1 |
| requirement FL-2 | B-9a |
| requirement FL-3 | FL-5 |
| requirement FL-4 | FL-2 |
| requirement FL-5 | FL-1 (per-home path resolution and de-duplication) |
| requirement FL-6 | FL-4, B-12 |
| requirement FL-7 | FL-5 |
| requirement FL-8 | OR-7, B-12 |
| requirement FL-9 | FL-3 |
| requirement FL-10 | FL-1 (`benches/fleet.rs` `fleet_fold` at 100, 1 000 and 10 000 homes, peak RSS) |
| requirement FL-11 | plan.md §2.10 seam (per-home file partitioning over FL-1's path resolution) |

**Rust library users (RS-1 to RS-9)**

| ID | Entry |
|---|---|
| RS-1 | PY-9, OR-3, OR-8 |
| RS-2 | PY-2, B-10, OR-3 |
| RS-3 | PY-8, OD-2 |
| RS-4 | OD-1, OR-6 |
| RS-5 | OR-3 (`ReportOutput::Memory`/`File`/`Callback`) |
| RS-6 | OR-5 (`Warning`), B-2 |
| RS-7 | OR-6 |
| RS-8 | RT-4 |
| RS-9 | plan.md §2.10 seam (`frame_golden`, M-1, as the maintained Rust driver) |

**Developers and maintainers (DV-1 to DV-9)**

| ID | Entry |
|---|---|
| DV-1 | OR-5, RT-5 |
| DV-2 | RT-5, M-4a, M-4b, B-8, OR-17 |
| DV-3 | OR-5, RT-1 |
| DV-4 | BR-7, Task 0 |
| DV-5 | OR-4 |
| DV-6 | OR-3 (bench groups), M-2 |
| DV-7 | PR-6, PR-5 |
| DV-8 | OR-1, B-5 |
| DV-9 | OD-2 (`wall_area_m2` rejection) |

**Cross-cutting requirements (X-1 to X-12)**

| ID | Entry |
|---|---|
| X-1 | OR-8, OR-3 |
| X-2 | OR-3 |
| X-3 | PY-9, OR-6 |
| X-4 | OR-6, RT-1 |
| X-5 | FL-1, OR-1 |
| X-6 | OR-10 |
| X-7 | PY-8, PY-2 |
| X-8 | FL-2 |
| X-9 | OR-2, OR-13 |
| X-10 | OR-2, RT-1 |
| X-11 | PY-11 |
| X-12 | IN-6 |

## 2. The calibration system's pain points and workarounds

Each entry is a workaround the consumer built, the HARES behavior it works around, and the cost
of carrying it.

- **A dead parquet write on every evaluation.** Every run sets `write_output=True` and a
  per-worker output path, because `write_output=False` truncates the frame to two columns and
  disarms the NaN/unresolved health gates. The frame the consumer actually uses comes from
  in-memory Arrow batches (`retain_batches: true` is hard-coded on the kwargs path). The disk file
  is never read. Cost: 10-20 ms and 3.4-4 MB per evaluation, projected at roughly 189 GB for a
  100-home x 468-evaluation sweep. Workaround: a scratch-sweeper that deletes the unread files, one
  `Runner` per worker keyed on PID so two jobs never share a file, and a manual `mkdir` before
  construction because the engine will not create its own output directory.
- **Silent wrong runs closed only by convention.** Three engine behaviors are avoided by a single
  disciplined call site rather than by the engine refusing them: passing `config=` silently
  overrides every sibling kwarg; `defaults_path` defaults to the relative string `"defaults"`, so
  physics depends on the working directory; an empty batch produces a two-column frame instead of
  an error.
- **A reachability programme to learn what "accepted" does not mean "applied".** A thermostat
  override moves output by exactly 0.00%; water-heater efficiency overrides move neither meter;
  heat-pump configs swallow unknown override keys and run unchanged; a malformed
  `charging_strategy` used to silently disable EV charging (fixed upstream by `9f23024b`). The
  consumer built a reachability probe that runs the nominal and both bounds of every calibration
  factor and checks the output actually moved, because "error-free does not mean applied."
- **Input-surface discovery by scraping, not by asking the engine.** The consumer's `schema.py`
  enumerates roughly 2,000 accepted fields three ways: provoking serde errors and parsing the
  message, regex-parsing Rust struct bodies (including `#[serde(flatten)]` resolution), and
  harvesting quoted literals from the HPXML parser. This exists because the engine has no
  machine-readable input schema to query directly.
- **Parameterisation by rewriting files with regex.** HPXML is rewritten with regex rather than an
  XML library, to keep byte-exact diffs auditable. Opaque R and thermal mass are reachable only by
  rewriting a defaults CSV, which needs a symlinked per-worker copy of a 137 MB tree. A shared
  defaults tree was once found corrupted in place by one trial vector's edits, moving a co-
  simulation's energy by 4.3%: a direct instance of the workaround itself introducing a defect
  class the shared, immutable input path is meant to remove. Efficiency parameters are scaled
  through HPXML HSPF/SEER rather than a resolved absolute value, because the engine exposes no
  read-back of a resolved equipment parameter to scale from.
- **Errors that cannot cross a process boundary.** Engine exceptions are pyo3 types that fail to
  pickle back to a parent process (`import of module '_hares' failed`), so a single infeasible
  parameter vector could take down a whole study cell with an error naming the pickling machinery
  instead of the physics. Workaround: every exception in a worker is caught, stringified to 400
  characters, and returned as data, penalised in the objective. This also means an engine bug and
  an infeasible input corner look identical to the optimizer.
- **Provenance by hashing the compiled binary.** `engine_build_id()` is the SHA-256 prefix of the
  compiled extension file, because the engine reports no commit, build profile or feature set.
  This hash has no mapping back to a source commit, and a rebuild step that silently no-ops (`uv
  sync` does not always rebuild the engine on a submodule rebase) has produced a "still broken"
  report against a stale binary.
- **Weather and time alignment re-derived by experiment.** The consumer found a one-row weather
  offset by scanning candidate shifts until the error dropped from 11.9-19.0 degrees C to zero at
  a shift of +1; it applies a roll-plus-one correction rather than relying on the engine's own
  alignment. A separate approximation reproduces the engine's OAT resampling for real meter data,
  since there is no API to obtain the engine's own resampled weather channel directly, and the two
  disagree by up to 0.375 degrees C away from the hour and half-hour marks.
- **Output schema drift found the hard way.** Five silent failures followed an engine column
  rename, because the consumer's contract check greps source text for display strings rather than
  querying a versioned catalogue; one of them kept a known defect alive under a stale name that
  still happened to resolve.
- **Determinism restricted by construction.** The calibration study is limited to slab-only homes
  specifically to avoid the fluid solver's hash-map iteration-order nondeterminism for below-grade
  foundations, at a stated scientific cost (no cold-climate electric-heating homes in the study).
- **A profiling instrument that measured itself.** `profiling_summary()` once reported "other 74%",
  which was almost entirely the profiler's own `/proc/self/status` reads rather than engine work.

## 3. How today's pipeline behaves

### 3.1 Configuration

`SimulationConfig` (`crates/hares-io/src/config.rs:61-128`) has no `deny_unknown_fields`, so
unknown TOML keys are silently dropped. `time_res_secs_u32` falls back to 3600 seconds when the
configured resolution is out of `u32` range (`config.rs:139-141`). `write_output` defaults to
`true` in both TOML (`config.rs:204-206`) and Python (`crates/hares-python/src/py_config.rs:201`).
`retain_batches` defaults to `false` in the Rust config (`config.rs:101-106`) but `true` in Python
(`py_config.rs:211`) and is hard-coded `true` on the `from_hpxml` kwargs path
(`crates/hares-python/src/py_dwelling.rs:2408`); the Rust and Python defaults disagree.
`output_path` of `None` means `dwelling_{bldg_id}.{csv|parquet}` in the current working
directory, computed independently in two places
(`crates/hares-core/src/dwelling/conversions.rs:590-596` and `crates/hares-core/src/engine.rs:411-426`).
Synthetic TOML config (`crates/hares-core/src/dwelling/synthetic.rs:290-327`) repeats the same
gaps. `HeatPumpConfig` uses `serde(flatten)`, which is incompatible with `deny_unknown_fields`, so
it silently accepts unknown keys; a test asserts the acceptance
(`crates/hares-io/tests/config_round_trip.rs:803-833`).

### 3.2 Output and telemetry

`Time` is `DataType::Utf8` (`crates/hares-io/src/output/columns.rs:425`, `TIMESTAMP_COL` at
`:56`). Column tiers are cumulative and gated by `output_verbosity` (0-8); classification into
tiers and end uses is by matching substrings in the equipment name string
(`is_hvac_or_wh`, `is_cooling_equipment`, `is_ev`, `columns.rs:1089-1150`). Absent values are
written as `0.0`, not null (`record_scratch.fill(0.0)` at
`crates/hares-core/src/dwelling/mod.rs:6607`, further `unwrap_or(0.0)` sites at `:6753-6791` and
`:6955-6976`). Unit strings live in column name suffixes and are also copied into Arrow field
metadata, which survives Parquet and Arrow but is lost across CSV and the polars IPC round trip.
The diagnostics CSV writer opens whenever `output_verbosity >= 4`, independent of `write_output`,
and its path falls back to the current working directory
(`crates/hares-core/src/dwelling/mod.rs:2787-2843`). A file destination is created eagerly at
construction (`File::create`, `crates/hares-io/src/output/writer.rs:120-123`); there is no
memory-only destination, so without file rotation a file is always created. Health counters
(`nan_temperature_count`, `unresolved_column_count`, `port_rollback_count`) exist only under the
`observe` Cargo feature (`dwelling/mod.rs:1707-1721`), which the shipped Python wheel does not
compile with, so a production caller reads zeros for all of them.

### 3.3 Per-step recording and memory

Every step, whether or not output is requested, builds a `StepResult` including a cloned
`Vec` of zone temperatures (`dwelling/mod.rs:6474-6481`) and pushes it onto an in-memory `steps`
history that grows without bound for the life of the dwelling (`:6583`); `step()` clones the last
entry on every call (`:2967`), and `simulate()`/`results()` clone the entire history on every call
(`:2876`, `:2917-2919`). When recording is on, aggregate columns are looked up by string key in a
`HashMap` on every step (`:6610-6621`), and the timestamp is formatted into a `String` with one
allocation per row (`:7008-7016`). `StreamingRecorder` flushes column buffers to a `RecordBatch`
every `chunk_size` rows (`writer.rs:518-535`); with `retain_batches=true` every flushed batch
accumulates in memory with no bound, and the recorder itself logs a warning to this effect
(`writer.rs:150-158`).

### 3.4 Finalisation and lifecycle

`flush_and_close` is called only from `Dwelling::simulate()`
(`crates/hares-core/src/dwelling/mod.rs:2867-2871`), the single call site; there is no `Drop`
implementation on the recorder. Step-driven runs (Python `step()`, gym, `SimulationPlan`, HELICS,
`SteppableFleet`) never flush the tail and never close the file; there is no public finalize or
flush for those paths. This produces zero-byte `dwelling_{n}.csv` files at the repository root,
hidden by `.gitignore:47`. `DwellingCheckpoint`
(`crates/hares-core/src/checkpoint.rs:74-107`) holds the clock, RNG, envelope, humidity, fluid,
equipment and actor state, but not recorder rows, the metrics calculator, the `steps` history,
tariff or billing state, or the warning log; a restore appends rewound-timestamp rows to the same
file and batches, and metrics accumulate across both branches of the restore.

### 3.5 Metrics

`MetricsCalculator` (`crates/hares-io/src/output/metrics.rs:453-615`) locates its inputs by column
name in the output schema; `Total Electric Power (kW)` is required, and envelope, HVAC-delivered
and end-use columns are available only above certain verbosity tiers. Metrics therefore depend on
output verbosity and on output being enabled at all: with no recorder (`write_output=false`),
metrics are zeroed and the run status is `Flagged`
(`crates/hares-core/src/engine.rs:472-510`). Python's `metrics()` recomputes from retained batches
on every call and raises if none were flushed
(`crates/hares-python/src/py_dwelling.rs:1490-1518`); the streamed metrics calculator's output is
not exposed to Python at all.

### 3.6 Fleet aggregation and results

`Fleet::simulate` (`crates/hares-fleet/src/fleet.rs:313-411`) collects each home's full
`SimulationResults`, including every batch, before aggregation runs
(`crates/hares-python/src/py_fleet.rs:240`); memory is O(homes x steps x columns). A home whose
status is anything other than exactly `Failed` counts toward the aggregate
(`crates/hares-fleet/src/aggregation.rs:258-270`), so a `Flagged` home with zeroed metrics is
still counted as a success. Any column mismatch across homes empties the whole aggregate to an
empty batch (`aggregation.rs:571-583`), which is normal for a heterogeneous ResStock fleet.
Aggregation uses a weighted sum for extensive units (kWh, kW, therms/hour, kVAR, W) and a weighted
mean for intensive units (temperature, dimensionless ratios, state codes)
(`aggregation.rs:113-125`), with no `Mode` reducer for enum-valued columns.

### 3.7 Results and Python conversion

`SimulationResults` (`crates/hares-core/src/engine.rs:49-62`) has a `timeseries: Option<Vec<RecordBatch>>`
field documented as "disabled by default" but always `Some(batches)` on success, empty when not
retained; this makes "not retained", "zero rows" and "failed" indistinguishable from the type
alone. Errors and panics become `Ok(SimulationResults{status: Failed(string)})`, so the typed
`HaresError` variant is lost at this boundary. In Python,
`batches_or_steps_to_polars_df` (`crates/hares-python/src/conversions.rs:20-49`) serialises
retained batches through Arrow IPC when they exist, and otherwise silently falls back to a
two-column frame (`Time`, `Total Electric Power (kW)`) built from the `steps` history.

### 3.8 Determinism instrumentation

The existing determinism regression test sets `RAYON_NUM_THREADS` after the thread pool has
already initialised, which has no effect, and compares only sorted per-end-use totals with keys
dropped before comparison (`tests/regression/determinism.rs:17-93`); it does not exercise the
fluid solver's hash-map iteration-order sum or the fleet's completion-order fold, and is
effectively vacuous as a determinism guarantee.

## 4. Non-requirements

From the needs analysis of HARES users, cross-checked against `decision-log.md`:

- **File rotation by time.** No integration test, notebook, doc example, bench or fleet path uses
  a rotation policy other than none. Reporting frequency is the actual need; the decision log
  deletes rotation entirely rather than keeping it as an unused option.
- **String timestamps** anywhere in frames, files or aggregation. The decision log's `time` entry
  requires typed time throughout.
- **CSV as the default output format.** CSV stays available for OCHRE parity; no user class needs
  it as a default.
- **Changing the output schema mid-run.** The existing freeze after the first row is acceptable
  behavior, provided equipment added later is reported as a named error rather than silently
  omitted (decision log's `output_model`, row-plan freeze).
- **Metrics that depend on verbosity.** No user class wants a metric to disappear because fewer
  columns were selected; the decision log's `output_model` entry makes metrics independent of both
  verbosity and the selected reports.
- **The two-column `steps` fallback frame.** A workaround for the absence of an always-available
  results path, not a feature any user class relies on; the decision log deletes it along with the
  `steps` history itself.
- **Implicit `dwelling_{bldg_id}` naming in the working directory.** No path may default to the
  working directory per the decision log's `destinations` and `inputs` entries.
- **Backward compatibility for existing file layouts or configs.** HARES is a greenfield codebase;
  the only preserved compatibility is OCHRE comparison through the Python adapter's mapping; no
  OCHRE preset in the Rust core (`ochre_compat`).
- **Calibration concepts inside the engine.** Factors, group scalars, composites, bounds, priors,
  and the differential-evolution objective belong to the calibration consumer. The decision log's
  `inputs` entry gives the consumer an in-memory parameterisation path and read-back, not a
  calibration-specific API; the consumer's own edit rules stay in the consumer.
- **A Rust port of the calibration harness, gradients or adjoints, per-element parameters, or
  engine-side loss aggregation.** Explicitly rejected by the consumer itself: the objective is a
  black box over an engine with discrete internals, and the Python parent's overhead does not
  justify a port.
- **`SteppableFleet` speed for co-simulation, or a voltage output column.** Rejected by the
  consumer as not buying meaningful throughput, and as outside a building model's scope,
  respectively.
- **A general Python-defined-equipment mechanism.** Per decision-log's `new_capability` entry,
  Python-defined equipment does not exist and is not added; Python actors declare their output
  variables through the existing actor API instead.
- **Batched multi-variant evaluation and `SteppableFleet` rewind as built-now capabilities.** Kept
  as seams, not built now, per decision-log's `new_capability` entry: no consumer has asked for
  either, and engine-side batching risks oversubscribing a fleet's own thread pools.
