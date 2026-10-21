# Axis B: Monte Carlo at scale; Axis C: reinforcement learning and utility ML (the rows' full specs)

Source: the I-09 planning round, 2026-10-17, verified against the tree at
`0911b711`. "Task N.N" refers to the I-08 hardening fan-out plan
(.shipwright/initiatives/I-08/plans/2026-10-17-hares-hardening-fanout.md).

## Axis B: Monte Carlo at scale

### B1: stochastic occupancy and behavioral diversity (L; the big one)

**Problem.** Every no-CSV dwelling in a fleet is behaviorally identical:
`hares-io/src/hpxml_schedule.rs:30-90` generates schedule columns from
fixed weekday/weekend fraction profiles read once from the defaults CSV,
with no per-home variation. A 10,000-home Monte Carlo study gets
N-times-identical fleet peaks, wrong DR response aggregates, and a
degenerate training distribution for any ML tool.

**Change.** A stochastic occupancy and diversity layer on the
generated-schedule family: per-dwelling seeded draws (the existing stream
discipline, `derive_sub_rng` nonces) over presence timing, appliance
diversity factors and water-draw timing; the OS-HPXML/ResStock stochastic
occupancy family as the reference contract; diversity parameters calibrated
so fleet aggregates match ResStock's published behavior.

**Acceptance.** Same seed reproduces bit-identical schedules; two bldg_ids
under one master seed differ; the fleet's aggregate diversity matches the
ResStock reference within a stated tolerance; the warmup-day invariance
(Task 1.5) extends to the new sources.

**Measurement.** Mean and std of per-home annual occupancy-driven end uses
across 1,000 synthetic homes vs ResStock's published variance; the
generation cost per home.

**Dependencies.** Tasks 1.3, 1.5, 2.8 landed first.

### B2: the scenario spec and sampler (M)

**Problem.** Per-dwelling overrides reach typed configs
(`DwellingSettings::new`, `fleet.rs:496-509`) but there is no scenario
concept: no distribution spec, no sampler, no record of what was sampled.
The calibration system does this job home-by-home via HPXML rewriting
(`calibration/inject.py:48-233`).

**Change.** A typed `ScenarioSpec`: per-parameter distributions (uniform,
normal, lognormal, triangular, discrete, with correlations and units) over
envelope, HVAC and occupancy parameters; a sampler that given
`(master_seed, scenario_id)` produces each home's parameter set
deterministically; application through the existing overrides and patch
paths; the sampled values serialized into the run manifest.

**Acceptance.** Sampler determinism (same (seed, scenario, bldg_id) gives
the same parameters); moment/KS tests against the spec; an overrides
round-trip proving each sampled value reached the typed config it names.

**Measurement.** Sampling cost per 10k homes; the manifest size.

**Dependencies.** Task 2.9 (the fleet seed pass-through).

### B3: the scenario seed algebra, pinned (S)

**Problem.** Row 71a's reseed re-derives every stream; OD-29 gives each
event load its own stream; Tasks 2.8/2.9 close the shared-keystream and
seed-0 defects. The scenario dimension has no rule: how
`(master_seed, scenario_id, bldg_id, stream)` map into the stream space so
two scenarios are independent by construction and a single-scenario re-run
is bitwise stable.

**Change.** Extend the stream-nonce scheme (`rng.rs:16-50`) with a scenario
nonce or a scenario-mixed master key; document the algebra in the RNG
module's contract docs; generalize Task 2.8's invariant tests.

**Acceptance.** Scenarios A and B provably disjoint streams; re-running one
scenario after another leaves both bitwise unchanged; the gym's
per-episode seed composes with the scenario seed.

**Dependencies.** Tasks 2.8, 2.9, 1.5.

### B4: the scenario lake (M)

**Problem.** Single-run reports write `Destination::Parquet`
(`output/writer.rs:493-501`); fleet results fold in memory to three tables
exported through the row-55 capsule. A 10k-home x 20-scenario study has no
sink: no directory layout, no scenario manifest, no partition pruning, no
per-scenario `RunHealth` rollup.

**Change.** A `ScenarioLake` writer (in `hares-output` post Task 4.1): one
parquet dataset per (study, scenario), Hive-partitioned by scenario and
home chunk, each scenario carrying a manifest row (engine identity, seeds,
scenario-spec hash, `RunHealth` summary, row counts); read-back through the
capsule into polars.

**Acceptance.** Write-then-read bitwise through polars; an interrupted
study resumes without duplicating partitions (with B5); a 10k-home x
5-scenario smoke writes and reads within a stated time.

**Measurement.** Write throughput; the read-back of one column of one
scenario (the partition-pruning proof); dataset size per scenario.

**Dependencies.** Task 4.1; B2's manifest.

### B5: the Monte Carlo runner (L)

**Problem.** `Fleet::simulate` (`fleet.rs:683-697`) is one call,
all-or-nothing; `SteppableFleet` holds every dwelling with no persistence;
the dwelling checkpoint is the only resume primitive. A 10k-home study
that dies at hour nine starts over. At the fleet's 1-minute step the
schedule resample is ~50 MB/dwelling (review 5 m4) until Task 2.14 lands:
10k homes is ~500 GB.

**Change.** A study runner owning the (scenario, home-chunk) grid:
per-chunk dwelling checkpoints, a progress manifest of completed chunks
and their lake partitions, resume after crash, per-chunk failure
collection (`HomeStatus`-style) without losing the study, composing
B1-B4; batch and stepped modes.

**Acceptance.** Kill-and-resume: kill mid-study, resume, the final tables
bitwise the uninterrupted run's; one home's build failure does not lose
the study; the manifest reproduces the result bitwise.

**Measurement.** The checkpoint overhead per chunk as a fraction of wall;
the restart cost after a crash at 50 percent; the memory high-water with
chunking.

**Dependencies.** B2, B3, B4; Task 2.14 is hard at 10k homes; Task 3.3's
pool reuse.

### B6: the analysis surface (M)

**Problem.** `MetricsCalculator` is per-run; `FleetResult` gives
fleet-period aggregates plus the homes table. Nothing produces what ML
tools consume across scenarios: percentile bands per period,
peak-exceedance and duration curves, scenario-delta tables, per-home
feature frames keyed to join the sampled parameters.

**Change.** A cross-scenario fold extension plus a Python `hares.analysis`
layer over the capsule tables: typed distributional outputs (quantiles,
exceedance, duration curves, per-home feature frames including sampled
parameters and `RunHealth`), column contracts held to the telemetry-keys
discipline.

**Acceptance.** A two-scenario fixture produces the documented columns
bitwise-stably; the outputs round-trip through the lake; the per-home
feature frame joins the scenario manifest on (scenario_id, bldg_id).

**Measurement.** The analysis cost on a 10k-home scenario (polars lazy
over parquet).

**Dependencies.** B4; Task 4.1.

### B7: one evaluation surface shared by the fit and the scenario study (M)

**Problem.** The calibration fit binds to the engine through an externally
policed contract (`hares_contract_check.py`); what a scenario study shares
with a fit is exactly the missing primitive (`evaluate_overrides` with
blueprint-rebuild semantics returning Arrow IPC plus `RunHealth` plus
provenance). Task 4.5 lands that surface; nothing adopts it as the shared
contract.

**Change.** Land Task 4.5, then make both consumers use it: the MC runner
(B5) evaluates scenario draws through it; the calibration Fitter keeps its
injector but drives the same path; the contract suite becomes the shared
regression gate.

**Acceptance.** The calibration system's contract suite passes against the
new surface; a fit and a scenario study produce bitwise-equal frames for
the same (premise, overrides, seed).

**Measurement.** The per-evaluation overhead vs today's Runner path.

**Dependencies.** Task 4.5 (Wave 4).

## Axis C: reinforcement learning and utility ML tools

### C1: catalogue-derived spaces (M)

**Problem.** Action bounds are name-keyed type-wide tables
(`py_gym.rs:288-316` mirrored in `gym_env.py:191-231`, pinned two-sided)
with a broad fallback and hardcoded observation ranges
(`gym_env.py:234-262`): every "min_soc" is (0,1) and every power-ish field
gets the broad box whether the heater is rated 5 or 20 kW. The catalogue
describes report columns, not equipment capabilities; no capability
readback exists.

**Change.** A capability readback on the dwelling (rated powers,
capacities, storage limits, thermostat class ranges per installed
equipment); gym spaces derived from it at construction; the name-keyed
tables shrink to the documented fallback for fields no equipment can know
(weather, time features).

**Acceptance.** The derived action bounds match each equipment's rated
telemetry (a rated-5 kW heater's bound is 5); an unknown action field
still errors at construction; conformance holds with C6's checker.

**Measurement.** The one-time construction cost of the bounds resolution.

**Dependencies.** After Task 3.2 (avoid churn in the same files).

### C2: the missing actor archetypes (L; three actors)

**Problem.** Six builtins only (`actor_registry.rs:287-603`). No actor
dispatches reactive power though the channel exists
(`ControlSignal::power_setpoint(active, reactive)`); no thermal-flexibility
actor drives HPWH preheat against a price signal though PreHeat exists as
a DR action and tank state is in telemetry; no aggregator coordinates the
home's equipment as one HEMS decision though the DR vocabulary is exactly
its action space.

**Change.** Three builtin actors, staged: `InverterVarDispatch` (Q
setpoints from a volt-VAR or power-factor curve on `bus_voltage_pu`,
riding Task 4.6's IEEE 1547 layer); `ThermalFlexibility` (HPWH and
thermal-mass preheat against `PriceSignal` including `ghg_intensity`, with
comfort and depletion guards); `Aggregator` (one actor holding
per-equipment power-limit dispatch across the roster: the HEMS baseline
every RL paper currently reimplements). Each a registry factory with docs.

**Acceptance.** Per actor: a behavioral test (the volt-VAR shape at 0.95
and 1.05 pu; the preheat completes before the draw window with the tank
above its floor; the aggregator's limit binds home total power) plus a
checkpoint round-trip of its state.

**Measurement.** Sanity only: the episode throughput with the aggregator
active vs individual actors.

**Dependencies.** Task 1.9 first; actor (1) waits for Task 4.6; (2) and
(3) can start after Wave 1.

### C3: typed reward components and a declarative reward spec (M)

**Problem.** The reward is the default `-net_power` or an arbitrary Python
callable over a raw `RewardContext` TypedDict (`gym_env.py:73-77`): not
serializable, not composable, a per-step Python call on the hot path. The
calibration system owns the component machinery (Scorer, components,
`objective.py:252-447`); RL researchers reimplement comfort, cost, peak
and carbon from telemetry dicts in every paper.

**Change.** A Rust-side component library (comfort degree-hours from zone
temps vs setpoints; cost from the tariff evaluator; peak from a rolling
window; carbon from `ghg_intensity` x import) evaluated from the recorded
columns; a declarative `RewardSpec` (named components, weights, windows)
accepted by the gym and the batch path, serialized into the episode
manifest; the callable escape hatch stays.

**Acceptance.** A RewardSpec matches a reference Python implementation to
1e-12 per step on a fixture; unknown component names error at
construction; the spec survives pickle.

**Measurement.** Steps/s with a five-component spec vs the Python-callable
reward on the `rl_episode` bench.

**Dependencies.** After Task 3.2 (the spec lands on the fast path).

### C4: per-equipment attribution and the multi-agent view (M)

**Problem.** One merged action Box per dwelling (`gym_env.py:539`);
`batch_step_py` takes one action vector per dwelling. Per-agent rewards
are impossible (reward sees whole-home totals); the per-equipment end-use
columns exist in the report catalogue but are not exposed per-step on the
batch path.

**Change.** Named action slices over the existing Box plus per-step
per-equipment attribution (end-use power routed through the observation
plan into a second buffer behind a `per_agent_info` flag). Deliberately no
PettingZoo dependency: the decomposition surface is the product; the
framework binding is the researcher's.

**Acceptance.** Slices match the concatenated vector; attribution matches
the report frame's end-use columns on a fixture; the masked auto-reset
behavior preserved (Task 3.4's test).

**Measurement.** The extra buffer's per-step cost (a copy).

**Dependencies.** Tasks 3.2 and 3.4.

### C5: the episodes/second contract and the PPO-representative bench (S)

**Problem.** Today's number: the 8-env 1,000-step loop at ~0.9 ms per
batch-step (row 71's State), about 9k env-steps/s before Task 3.2's
removals. No published episodes/second target exists for a
PPO-representative configuration, so "is HARES fast enough for RL" is
folklore.

**Change.** Standard RL bench configurations (N envs, obs and action
widths, 900 s and 60 s episodes, the C3 reward spec) recorded with the
reference box named; then the next-tier hot-path items with numbers (the
`Vec<Vec<f64>>` action crossing, telemetry laziness). Own the next 10x
item with a row if the number says so.

**Acceptance.** The bench lands in the bench set; the target (for example
50k env-steps/s at 900 s, 8 or more envs) recorded with honest methodology.

**Dependencies.** Task 3.2; C3.

### C6: gymnasium conformance and the version contract (S)

**Problem.** The pin is a floor only (`gymnasium>=1.3.0`);
`env_checker.check_env` runs nowhere; the info contract leaks Rust
internals (a float `warning_count`, a `seed` key present when unset).

**Change.** Pin a tested range; add the env checker and a vec-env smoke
through standard wrappers to pytest; fix the info contract as Task 3.2
batches it; document the supported API surface.

**Acceptance.** `check_env` passes on `DwellingGymEnv`; the suite runs
against the pin's floor and the current gymnasium.

**Dependencies.** Batched with Task 3.2's contract changes.

## Deliberately not proposed

No Rust-side training loop (HARES is a simulator; the learner is the
researcher's stack); no PettingZoo/MARL framework binding (C4 is the
minimal surface; the binding would freeze the batch API before C2/C3
settle); no per-step Python-actor FFI (the registry factory model plus C2
serves "actors first-class"; revisit if C2 proves insufficient); no
stochastic weather realizations (weather has its own provenance chain;
ensembles over weather are study design: B2's spec can carry a weather-set
dimension); no GUI or dashboard for the lake (the capsule and polars read
path is the product).
