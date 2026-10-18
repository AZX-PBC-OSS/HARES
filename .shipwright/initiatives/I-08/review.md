# I-08 Review: confirmed defects and architectural decisions

Checked against the HARES tree at `ebdb1a17`. `file:line` evidence below was verified by reading
the cited source. Decisions state the current position from `decision-log.md`; where a source
disagreed with the decision log, only the decision log's position is recorded here.

I-08's purpose is runtime, telemetry and output reporting, memory and performance
(`decision-log.md`, `dl-i08-process`). Defects that touch that core (results, output model,
health, time, lifecycle, fleet, errors) are fixed as part of the architecture below. Defects that
are physics/input-strictness fixes outside that core (weather alignment, warm-up convergence,
billing boundary, end-use rollup, DER strictness, and similar) are confirmed here and fixed as
separate ledger entries, each its own commit with a re-captured golden, per `dl-i08-numeric_fixes`
and `dl-i08-process`'s `found_defects` rule.

## 1. Confirmed defect table

Status: **confirmed** (the defect is present and reproducible as described), **partial** (part of
the claim holds; the rest is noted), **refuted** (the claim does not hold against the code).
"Ledger" marks whether the fix lands in the core architecture work below or as a separate
numbered fix entry.

| ID | Defect | Status | Evidence | Ledger |
|---|---|---|---|---|
| V1 | Health counters exist only under the `observe` feature; the shipped Python wheel reports zeros for all of them | confirmed | Fields `#[cfg(feature = "observe")]` at `crates/hares-core/src/dwelling/mod.rs:1707-1721`; increments gated the same way at `:5204-5207`, `:6274-6277`, `:6660-6682`; the Arrow columns are appended in every build regardless at `:3904-3918`; `hares-python` builds with no default features (`crates/hares-python/Cargo.toml:8-10`) and maturin passes none (`pyproject.toml:73-81`) | core (health) |
| V2 | `nan_temperature_count` can never be non-zero in a result a caller receives | confirmed | The counter is incremented and the call returns `Err(InvariantViolation)` in the same block, `crates/hares-core/src/dwelling/mod.rs:6258-6287`; the counter is also reset to 0 every step, `:5308-5310` | core (health) |
| V3 | Engine exceptions raised to Python do not pickle, because of an exception-module mismatch | confirmed | `create_exception!(_hares, ...)` at `crates/hares-python/src/py_dwelling.rs:30-37` while the wheel's module is `ochre_next._hares` (`pyproject.toml:77`); reproduces as `PicklingError: Can't pickle <class '_hares.HaresConfigError'>: import of module '_hares' failed` | core (errors) |
| V4 | Hourly weather reads lag by one hour; sub-hourly zero-order-hold fields lag by roughly 30 minutes | confirmed | Offset computed as `floor((t_start - 1800) / step)` at `crates/hares-core/src/environment.rs:824-848`; EPW midpoint offset of 1800 s at `epw.rs:427-429`; row index `(step + offset) % len` at `environment.rs:529-536` | separate fix |
| V5 | Running past the end of the weather file wraps silently instead of erroring | confirmed | `% weather_len` at `crates/hares-core/src/environment.rs:533-535` | separate fix |
| V6 | Billing assigns the first step of a new month to the previous billing period | confirmed (boundary); the specific period count claimed by a downstream consumer was not reproduced from code | `evaluator.step(...)` closes the period on `current_time >= period_end`, and `current_time` is the step start because the clock advances only after billing runs (`crates/hares-core/src/dwelling/mod.rs:6520-6528` billing, `:6581` clock advance) | separate fix |
| V7 | `initialize()` serialises a checkpoint that is never read back | confirmed | Written at `crates/hares-python/src/py_dwelling.rs:661-676`, cleared at `:1528`, never read; `simulate()` refuses to run without it having been called, `:703-707` | core (lifecycle) |
| V8 | The fluid solver sums a `HashMap` in iteration order | partial: the arm is unreachable; removed with zero delta (B-8) | `resolved_node_flows: Option<HashMap<FluidNodeId, f64>>` (default hasher) and `.values().sum()` when there is no pump node, `crates/hares-envelope/src/fluid_solver.rs:334`, `:785-787` | separate fix |
| V9 | An instance-qualified name such as `PV #1` rolls up to the `Other` end use | partial: latent (B-5) | Exact-match name classifier, `"PV"` only, defaulting to `OTHER`, `crates/hares-io/src/output/columns.rs:178-245`; rollup indices are built from the name, `mod.rs:599-610` | core (output model, seam); rollup fix separate |
| V10 | `Television` is classified as EV equipment; pool and spa heaters get HVAC columns | confirmed | `is_ev` matches on `contains("ev")`, which matches "tel**ev**ision", `columns.rs:1140-1143`; `is_hvac_or_wh` matches on `contains("heater")`, `columns.rs:1089-1103` | separate fix |
| V11 | Heat pump configs silently swallow unknown keys through `serde(flatten)` | confirmed | Documented in the struct's own comment, `crates/hares-equipment/src/hvac/heat_pump_config.rs:19-22` (`flatten` is incompatible with `deny_unknown_fields`, so neither struct carries it); round-trip tests assert the swallow, `config_round_trip.rs:804-832` | separate fix (input strictness) |
| V12 | Output flush and file close happen only inside `simulate()`; stepped runs lose the tail of the run | confirmed | Single caller, `crates/hares-core/src/dwelling/mod.rs:2867-2871`; no `Drop` implementation on the recorder | core (lifecycle) |
| V13 | The diagnostics CSV writer ignores the configured output mode | confirmed | Gated only on `output_verbosity >= 4`; writes with `File::create` at `mod.rs:2787-2800` from `output_path` or a current-working-directory default, `:2470-2474` | core (output model) |
| V14 | Not-applicable telemetry values are written as `0.0` instead of null | confirmed | `record_scratch.fill(0.0)` at `mod.rs:6607`; further `unwrap_or(0.0)` sites at `:6753-6791` and `:6955-6976` | core (output model, nulls); value change is separate numeric fix |
| V15 | `batch_step` ignores the caller's `reward_fn` and hard-codes a Rust reward | confirmed | `reward = -step.net_electric_power_kw` at `crates/hares-python/src/py_gym.rs:494`; the fast batch path uses this value; `reward_fn` is a required argument with no default in `vec_env.py:46`, used only on the fallback path at `:279` | core (RL reward contract) |
| V16 | Fleet aggregation counts flagged, zero-metric homes as successes | confirmed | `failed: matches!(.., Failed)` is the only exclusion, `crates/hares-fleet/src/aggregation.rs:258-270`; a no-recorder home is status `Flagged` with zeroed metrics still marked `Reliability::Reliable`, `engine.rs:500-507`, `:542` | core (fleet) |
| V17 | A heterogeneous column set across a fleet empties the whole aggregate | confirmed | `return empty_batch()` on any schema mismatch, `aggregation.rs:571-583` | core (fleet) |
| V18 | The reported engine version is a hard-coded constant, not a build identity | confirmed | Version string `0.1.0` at `crates/hares-core/Cargo.toml:3` and `pyproject.toml:7`; no `build.rs` in `hares-core`; no `__version__` or build-info accessor anywhere in the binding | core (build identity) |
| V19 | The defaults directory resolves to the relative path `"defaults"`; a missing directory silently runs on an empty defaults store | confirmed, worse than a bare default: physics results silently depend on the process's current working directory | `PathBuf::from("defaults")` at `crates/hares-core/src/dwelling/blueprint.rs:126`; an unavailable defaults store logs a warning and continues with `DefaultsStore::empty()`, `blueprint.rs:135-138` | separate fix (path resolution to package data; missing store becomes an error) |
| V20 | Passing `config=` silently overrides sibling keyword arguments instead of conflicting with them | confirmed | `if let Some(py_cfg)` short-circuits construction while `bldg_id` and `initialization_duration` are still read afterward, `crates/hares-python/src/py_dwelling.rs:2246-2258`, `:2413-2440`; the kwargs path also hard-codes `retain_batches: true`, `:2408` | core (config) |
| V21 | A malformed EV `charging_strategy` silently becomes `Immediate` | refuted | Already fixed by commit `9f23024b`: `parse_charging_strategy` now returns `Err` on a malformed value, `crates/hares-equipment/src/ev/config.rs:319-326`, pinned by `init_errors_on_malformed_charging_strategy_override` (`crates/hares-equipment/src/ev/tests.rs:8062`) | none (already fixed and tested; `ledger/11-other-defects.md` preamble) |
| V22 | `initialization_duration`'s value is never read; only its presence matters | confirmed | `if let Some(_init_dur) = config.initialization_duration` then a fixed `run_warmup_converged(0.5, 25)` regardless of the value, `crates/hares-core/src/dwelling/mod.rs:2725-2740` | separate fix (warm-up) |
| V23 | The Python default is framed as "no warm-up" against "the Rust default of 7 days" | partial: the gap between the two front ends is real, but the "7 days" side of the comparison does not do anything (see V22) | Python filters a duration of 0 to `None` (`py_config.rs:648`); Rust sets `Some(7 days)` (`mod.rs:1812`), but that duration value is never used; a missing warm-up call only logs, `mod.rs:2776-2784` | separate fix (warm-up) |
| V24 | The warm-up convergence criterion is NaN-blind and weaker than the EnergyPlus criterion | confirmed | `fold(0.0_f64, f64::max)` returns 0 for an all-NaN delta set, `mod.rs:5035-5042`; compares only end-of-day conditioned temperatures; non-convergence only logs a warning, `mod.rs:5064-5070` | separate fix (warm-up) |
| V25 | Several process-global static counters exist and can be shared across concurrent dwellings | confirmed | `REJECTED_SIGNAL_COUNT`, `crates/hares-equipment/src/lib.rs:35`; `MODE_FLOW_INCONSISTENCY_COUNT`, `hares-types/src/mode_flow_guard.rs:16`; `RANGE_*`, `hares-types/src/equipment.rs:1236-1239`; `GERSHGORIN_FALSE_POSITIVE_COUNT`, `state_space.rs:181`; `PANIC_*`, `panic_hook.rs:71-101`; `NON_FINITE_REJECTIONS`, `hares-types/src/telemetry.rs:20`; `ACTIONS_CLIPPED`, `py_gym.rs:208` | core (health) |
| V26 | The in-memory `steps` history grows without bound; `step()` clones the last entry on every call | confirmed | `mod.rs:6583` (unbounded push); `mod.rs:2966-2967` (clone) | core (results/lifecycle) |
| V27 | Per-step telemetry uses `clone_from` rather than an allocation-free write | confirmed | `mod.rs:4924`, `mod.rs:6436` | separate fix (engine performance) |
| V28 | The dwelling checkpoint excludes the recorder, metrics, billing, tariff index and warnings | confirmed | `checkpoint.rs:74-108`; the tariff evaluator's `step_index` (`evaluator.rs:26`) is never restored | core (lifecycle) |
| V29 | Metrics depend on which output columns happen to exist | confirmed | Metrics look columns up by name and gate on `has_envelope_columns`, `crates/hares-io/src/output/metrics.rs:593-604`; the engine's metric path repeats the same lookup, `engine.rs:478-510` | core (output model, metrics independence) |
| V30 | The determinism regression test is vacuous | confirmed | Sets `RAYON_NUM_THREADS` after the thread pool is already initialised, `determinism.rs:39-41`; values are sorted and keys dropped before comparison, `:45-52` | core (fleet/determinism instrumentation) |
| V31 | Below-grade external RC nodes are sorted, but the sort is untested | confirmed | `external_nodes.sort_unstable()`, `crates/hares-envelope/src/rc_network.rs:138-150`; no test exercises node-order sensitivity | none (already pinned; `11-other-defects.md` preamble) |
| V32 | `from_hpxml` and `initialize()` hold the GIL for their full duration | confirmed | No `py.detach` in `crates/hares-python/src/py_dwelling.rs:636-676`; `simulate()` does detach, `:708` | core (concurrency) |
| V33 | polars drops Arrow field metadata on import, so a unit attached only as field metadata is lost | confirmed | Reproduced against polars 1.44.2 / pyarrow 25.0.1: a field's `unit` metadata reads back as `None` after `pl.from_arrow(...).to_arrow()` | core (output model: units need a catalogue accessor, not field metadata alone) |
| V34 | File destinations call `File::create` eagerly, at construction, before any row is written | confirmed | `writer.rs:119-123` | core (lifecycle/output model) |
| V35 | `time_res` silently falls back to 3600 seconds on a bad value; `SimulationConfig` has no `deny_unknown_fields` | confirmed | Fallback at `config.rs:140-142`; the struct at `config.rs:62` carries no `deny_unknown_fields` attribute | core (config) |
| V36 | UTC and daylight-saving ambiguity resolve to silent fallbacks instead of errors | confirmed (offset half; the daylight-saving half cannot occur for a fixed offset) | `blueprint.rs:95-102` | `11-other-defects.md` OD-2 site 8 |
| V37 | A non-object HPXML override is a silent no-op; `all`/`*` targets are merged ambiguously; untyped override values are dropped; several `.ok()` calls swallow errors | confirmed | `conversions.rs:937-942`, `:1085-1099`; nine `.ok()` sites in `resolve_hvac.rs:316-350` | separate fix (input strictness) |
| V38 | A water-heater UA resolution failure silently falls back to a default value | partial: the fallback is confirmed; a broader claim about efficiency-factor precedence being baked in at parse time was not traced | `resolve_water_heater.rs:131-141` | separate fix (input strictness) |
| V39 | A thermostat setpoint override loses precedence to the schedule source | partial: the schedule-source assignment is confirmed; the full precedence chain was not traced end to end | `schedule_resolve.rs:673-690` | separate fix (precedence) |
| V40 | An HPXML battery with no declared BMS mode runs inert; EV charger power is read without unit conversion | partial: both specific reads are confirmed; a claimed missing upper-range check was not traced | `bms_mode: None`, `resolve_der.rs:163`; `MaxChargingPower` read without conversion, `resolve_der.rs:197` | separate fix (DER strictness) |
| V41 | `SteppableFleet` reports finished homes as failed | confirmed | `fleet.rs:566-578` | core (fleet) |
| V42 | ResStock fleet configuration hard-codes UTC offset 0, start year 2019, `write_output=false`, and AMY 2018 weather regardless of the building's actual weather year | confirmed | `fleet.rs:779-799`; weather filename pattern `{fips}_2018.csv`, `fleet.rs:820-825`; an unresolved weather path becomes `PathBuf::from("")` with only a log line, `fleet.rs:170-194` | B-9, with B-9a |
| V43 | `wall_area_m2` is parsed "for backward compatibility" and never used | confirmed | `synthetic.rs:137-143` | separate fix (input strictness) |
| V44 | Gym environment coercions: an unknown time-resolution type silently becomes 60 seconds; `master_seed=0` is dropped as falsy | confirmed | `gym_env.py:100-105`, `:152`; a "legacy dict path" at `:111` | separate fix (input strictness) |
| V45 | A two-column fallback frame exists as a second, narrower results representation | confirmed | `crates/hares-python/src/conversions.rs:20-49` (`batches_or_steps_to_polars_df`, falling back to `steps_to_polars_df` which emits only `Time` and `Total Electric Power (kW)`) | core (results model; deleted, not projected) |
| V46 | An indirect tank's loop-id resolution failure warns and continues instead of erroring | confirmed | `if let Ok(...) else tracing::warn`, `crates/hares-io/src/hpxml/equipment.rs:289-298` | separate fix (input strictness) |
| V47 | `StepResult` cannot be reproduced from recorder rows, so a second representation is structurally necessary | refuted as a necessity | `StepResult` is five scalars plus per-zone temperatures (`mod.rs:846-856`); every field is declarable as a catalogue variable written by the dwelling like any other output | none (this is why the results model below has one representation, not two) |
| V48 | Narrowing the recorder to only what is requested breaks metrics and fleet coverage | partial: true only if metrics does not declare its own demand against the full catalogue and coverage is checked against the narrowed schema; the output model below avoids both by construction | reasoning claim, no single file:line | none (addressed by the output model's catalogue/row-plan split) |
| V49 | Output-model requirements: no working-directory fallback for any file output, an explicit `Mode` aggregation kind, `reward_fn` reaching Python on every path, an enforced fleet column policy | design requirement | not a HARES source defect | addressed by the decisions below |
| V50 | File destinations must always carry an explicit path | design requirement | not a HARES source defect | addressed by the decisions below (destinations always require a path) |

## 2. Architectural decisions

Each decision below states the current position of the matching `dl-i08-architecture` key, the
reasoning, and the alternative that was rejected and why. The keys listed here (names) are stated
only in `decision-log.md`.

### Results

**Decision.** One typed `RunResult` produced by `finish()`, for both `simulate()` and stepped
runs: tables, metrics, health, warnings, provenance. Stepped and simulated runs finalise
identically. There is no retained step history, no two-column fallback frame, no
`initialize()` checkpoint serialisation, and no per-step callback. `step()` returns the
per-step observation directly.

**Reasoning.** `StepResult` (`crates/hares-core/src/dwelling/mod.rs:846-856`) is five scalars
plus per-zone temperatures, and every one of those fields is declarable as a catalogue variable
written by the dwelling like any other output (this is why V47 is refuted as a structural
necessity). Once that premise is refuted, there is no reason to keep two representations of a
run's output: an accessor path that produces frames from batches, and a second, narrower path
that produces frames from steps (V45). `finish()` also closes billing (V6) and finalises metrics,
so a stepped run and a `simulate()` run end in the same state.

**Rejected alternative.** Keeping a `finalize()`/`results()`/`metrics()` accessor split with a
separate retained-steps path, on the reasoning that a "results" object and a "raw steps" object
serve different callers. This was rejected because it keeps the two-column fallback frame's
premise alive: if `StepResult` fields are declarable, a raw-steps path is not a different
capability, it is an unfinished catalogue.

### Output model

**Decision.** Variable catalogue (every variable the built model can produce: unit, kind,
aggregation, interval or instant, nullability, source; queryable before the run; independent of
verbosity) produces report requests (selection, reporting frequency, destination chosen
independently), which produce a row plan, which produces destinations. Verbosity tiers are named
selections. The recorder builds each batch once. Metrics do not depend on verbosity or on the
selected reports. Metrics are computed only when the run requests them.

**Reasoning.** Today's recorder schema is gated by verbosity (`OutputSurface::build(specs,
verbosity, ...)`), and metrics look their inputs up by column name against whatever that gated
schema produced (V29). That makes a metric's value depend on which verbosity tier was requested,
which fails outright for any consumer that wants metrics without a matching high-verbosity
report. Splitting the catalogue (what the model *can* produce, fixed) from the report request
(what a caller *wants*, variable) removes that coupling: a variable's declaration does not change
with what is requested, so a metric can declare its own demand against the full catalogue
regardless of what reports exist.

**Rejected alternative.** A sink list observing a full recorder, with a trait such as
`OutputSink { on_step, on_batch }` and downcasting to retrieve results out of a sink after the
run. This was rejected on two grounds: downcasting a type-erased sink to fish out a result is
state retrieval, not a result contract; and it does not reduce cost for undemanded columns, since
the recorder still fills a schema sized by verbosity regardless of what any sink actually
consumes. A second rejected alternative kept the recorder always filling the full verbosity-gated
surface and pushed narrowing to per-sink projection; that still pays to compute columns nobody
asked for and keeps metrics tied to verbosity.

### Destinations

**Decision.** In memory, Parquet, CSV, and a batch-level Python callback (opt-in, zero cost when
unused; a callback error fails the run with the Python exception chained). No file rotation.
Files are written to a temporary name and renamed on `finish()`. Nothing is written unless
requested; nothing defaults to the working directory.

**Reasoning.** File destinations today call `File::create` eagerly at construction, before any
row exists (V34), so a run that fails before producing data still leaves a file under the final
name; a truncated file under the final name is a silent partial result. Writing to a temporary
name and renaming only on completion removes that failure class entirely, atomically. Path
defaults resolving to the current working directory are rejected as a class: the diagnostics CSV
writer already defaults to a CWD path when none is configured (V13), and the defaults directory
resolves the same way (V19); both are instances of a destination or input silently depending on
where the process happens to be launched from.

**Rejected alternative.** Keeping `Rotating<S>` with `Hourly | Daily | Monthly | Yearly` rotation.
This was rejected because no real (non-test) user exercises it, its cadence kwarg name has
already been misread by a downstream consumer as something else, and its boundary logic is
string-prefix matching rather than typed periods. Reporting frequency, which the output model
already needs for report selection, replaces the need it was serving; the destination type
remains the extension seam if file rotation is ever needed again.

### Python export

**Decision.** Arrow C stream export through pyo3-arrow; tables are rechunked to a single chunk
by default.

**Reasoning.** The IPC-versus-steps fallback in `crates/hares-python/src/conversions.rs:20-49`
exists because there was no single, always-available Arrow export path; a batches-or-steps
branch was needed to cover the case where no batches existed yet. With one `RunResult` and no
second representation, the branch has no reason to exist: a table is always exportable through
the same standard interface a caller's dataframe library already knows how to consume.

**Rejected alternative.** Continuing to hand back a `polars.DataFrame` built ad hoc inside the
binding (as `steps_to_polars_df` and `record_batches_to_polars_df` do today). This ties the
binding to one Python dataframe library's construction API and duplicates conversion logic per
representation; the C stream interface is consumed by any Arrow-compatible library without the
binding choosing one for the caller.

### Config

**Decision.** One Rust config type with one `validate()`. TOML, synthetic TOML, and a Python
construction path all build that one type; Python builds it from typed keyword arguments. Strict
everywhere: unknown or removed keys are errors that name the replacement. No twin config types,
no permissive mode.

**Reasoning.** `SimulationConfig` today has no `deny_unknown_fields` and a silent fallback for a
bad `time_res` (V35); heat pump configs use `serde(flatten)`, which is incompatible with
`deny_unknown_fields` and so silently swallows unknown keys (V11); passing `config=` alongside
other keyword arguments silently lets `config=` win for some fields while other kwargs are still
read for others (V20). Each of these is a different silent-acceptance path arriving at the same
underlying config type from a different entry point. One type with one strict validator, called
from every entry point, removes all three at once instead of patching each entry point
separately.

**Rejected alternative.** Per-entry-point strictness fixes (fix the TOML parser's fallback, fix
the heat pump struct, fix the kwargs conflict, each independently) while keeping separate parsing
code paths per front end. Rejected because separate parsing code paths are exactly how three
different silent-acceptance bugs accumulated at three different entry points into the same
logical config; a fourth entry point added later would need the same fix a fourth time.

### Inputs

**Decision.** HPXML, schedule, weather and defaults can be supplied as content, with parsed data
shared immutably across dwellings, plus read-back of the resolved parameters the physics uses. No
edit language now; the shared parsed inputs are the seam for one later. A reserved `envelope`
override key (`opaque_r_scale`, `thermal_mass_scale`) scales every opaque boundary's layer
resistance and capacitance independently, after LUT row selection, so scaling R for calibration
never re-selects the capacitance row and the fit surface stays continuous in the scale.

**Reasoning.** The defaults directory resolves relative to the current working directory and a
missing directory silently continues on an empty store (V19), which means the same input file
can produce different physics purely depending on where the calling process was launched from.
Sharing parsed input data immutably and requiring content or a resolved path removes that
dependency on process state. Read-back of the resolved parameters the physics actually used
(rather than an edit language that encodes a caller's own editing rules inside the engine) keeps
the engine responsible only for what it can validate: it can attest to what it resolved, it
should not encode any one caller's rules for producing an edited HPXML tree.

**Rejected alternative.** A typed override DSL (`ParamEdit` with `Scale`/`Shift`/`Set` operations)
applied over the parsed HPXML tree inside the engine, with a downstream consumer migrating onto
it and re-baselining its results. Rejected because it encodes one consumer's own editing science
(first-match scope, wrapping, carried elements, unit-specific shifts) as an engine feature, which
does not generalise to other user classes; the shared, immutable parsed-input path plus read-back
already gives a consumer everything it needs without moving its own edit rules into the engine or
forcing a re-baseline of its stored results.

### New capability

**Decision.** Built now: pre-run catalogue and reporting frequency, resolved-parameter read-back,
`validate()`, strict health policy, `metrics_from_table`, `resample_weather`, replay export,
machine-readable input schema. Not now (seams kept): batched multi-variant evaluation,
`SteppableFleet` rewind, a built-in reward enum. Python-defined equipment does not exist and is
not added; Python actors declare their output variables through the actor API.

**Reasoning.** Each item built now either replaces an existing informal mechanism with a typed
one (the catalogue replaces ad hoc schema metadata; reporting frequency replaces file rotation
and generalises the fleet's existing 15-minute/hourly resampling) or exposes an existing internal
capability through a stable entry point (`resample_weather` already exists internally;
`validate()` wraps existing checks). None of them is a new physics capability.

**Rejected alternative.** Adding a built-in reward enum for reinforcement-learning users, on the
reasoning that a default reward saves callers from writing their own. Rejected because
`reward_fn` is already a required argument today with no default (V15's Rust-side hard-coded
reward is the only default that exists, and it is silently wrong, not a real default worth
preserving); adding a built-in enum would be a new feature, not a fix to something existing. A
second rejected item was `SteppableFleet` save/load: `SteppableFleet` gains no save or restore
method, so there is no rewind entry point to reject, and `docs/python.md` states that fleet
stepping does not rewind (`plan.md` §2.10), because no consumer has asked for it and it risks
oversubscribing a fleet's own thread pools.

### Health

**Decision.** Every health event (port rollback, clamp, rejected signal, warm-up
non-convergence) is recorded unconditionally in every build and returned in `RunResult.health`.
NaN and physics invariant violations always fail the run, in every build profile, including
warm-up. Other escalation is an explicit `HealthPolicy`, empty by default; consumers that gate on
health set a strict policy.

**Reasoning.** Health counters today exist only under the `observe` feature, which the shipped
Python wheel does not compile with, so every health field a consumer reads back is zero (V1);
even under `observe`, the NaN counter is reset before a caller could read it, because the run
already returned an error in the same code path (V2). A consumer's health gate is vacuous against
that. Making health unconditional in every build closes both gaps in one change: there is exactly
one way health gets recorded, and it does not depend on a compile feature the production wheel
does not have.

**Rejected alternative.** Keeping Tier-0 per-dwelling counters for a curated subset (matching
today's `observe`-gated fields) with process-global totals "additionally" available for anyone
who wants a running total. Rejected because a process-global counter is a second source of truth
alongside the per-run counters (several already exist and are a defect in themselves, V25); two
counters that can drift apart is worse than one counter that is always right.

### Time

**Decision.** All times are local site time. Output time is a naive local timestamp labelled at
interval start; the site offset is recorded in metadata. Not-applicable values are null, never
`0.0`.

**Reasoning.** Interval-start labelling matches both OCHRE and polars' own interval-labelling
convention, so a downstream tool reading a HARES table needs no special-case interpretation; the
alternative, an EnergyPlus-style end-of-interval label, only ever needs documenting as a shift
from the start label. Weather is read from the correct interval only once the recorder's rows and
the weather rows share the same start-of-interval convention, which is also what fixes the
one-hour weather lag (V4).

**Rejected alternative.** Leaving the label convention to documentation rather than to the type,
with a bare `Timestamp(ms)` and no declared per-variable sampling. Rejected because "which instant
a producer's value actually represents" is not one universal answer: zone temperatures and
state-of-charge are end-of-step solver states in most producers, while others are interval means,
and a caller cannot tell the difference from an undeclared timestamp alone. Declaring `sampling`
(interval mean, instant at start, instant at end) per variable makes that explicit instead of
assumed.

### Weather

**Decision.** EnergyPlus conventions, specified per field kind (interval averages such as solar
versus point values such as temperature), from the EnergyPlus engineering reference and source,
including sub-hourly steps, typical-versus-actual year, wrap, and leap-day handling. One
exception: after a skipped Feb 29, day types follow the true calendar; EnergyPlus shifts every
later date one weekday behind, which its own documentation calls incorrect
(`leap_day_day_types`). Feb 29 on a typical-year file without one, and sub-hourly files whose
interval differs from the time step, follow EnergyPlus (`weather_edge_cases`).

**Reasoning.** The current weather index computes an offset of `floor((t_start - 1800) / step)`
against an EPW midpoint convention (V4), which reads the wrong hourly record for every step and
reads the wrong sub-hourly record by roughly half the resampling interval; running past the end
of the weather series silently wraps instead of erroring (V5). Both are fixed by aligning the
recorder's step convention to the same interval convention the weather record was written under,
per field kind, rather than applying one blanket offset to every field regardless of whether it is
an interval average or a point value.

**Rejected alternative.** Keeping the existing single offset formula and fixing only the wrap
(V5) in isolation. Rejected because the offset itself, not just the wrap, is confirmed wrong for
both hourly and sub-hourly fields (V4); fixing only the wrap would leave every weather-driven
result silently shifted.

### Aggregation

**Decision.** Energy columns sum; power, temperature and ratios average; binary state columns
average (duty fraction); multi-valued enum columns use Mode (most frequent value in the period,
ties to the lowest code; across dwellings, the value with the greatest summed weight). Fleets
combine in two stages: over time within a home by the rule above, then across homes by a
separately declared rule: energy and power are weighted sums; temperature, ratio and state are
weighted means; enums use Mode. Every quantity variable declares its across-homes rule.

**Reasoning.** Aggregation kind is declared per variable in the catalogue (`aggregation` field:
`Mean`, `Sum`, `Min`, `Max`, `Mode`, `Last`), so a reporting-frequency change (for example
timestep to hourly) applies the correct reducer automatically instead of a caller having to know
which columns are extensive and which are intensive. Today's aggregation logic has no `Mode`
reducer at all for enum-valued columns (V49), so an enum column aggregated to a coarser frequency
today either has no defined behavior or is silently treated as numeric.

**Rejected alternative.** Leaving aggregation kind undeclared and inferring it from column dtype
at aggregation time (numeric columns sum or average by convention, non-numeric columns pass
through unresolved). Rejected because dtype alone cannot distinguish an extensive quantity
(energy, which sums) from an intensive one (temperature, which averages); the declaration has to
live with the variable, not be inferred at the point of use.

### Fleet

**Decision.** Fleets never retain full frames; memory is bounded independent of fleet size.
Output is labelled in local site time. A fleet spanning more than one UTC offset must declare
alignment: instant (same physical moment, labelled in a declared reference zone) or local_clock
(same local wall clock). Every home's coverage and status is reported; a flagged home is never
counted as a success.

**Reasoning.** Fleet aggregation today collects every home's full result in fleet entry order
and folds afterwards, so memory grows with homes times steps times columns. The determinism test
is vacuous (V30): it varies a thread setting nothing reads, so it cannot show that the result is
independent of thread count and chunking. Aggregation today also only excludes homes whose
status is exactly `Failed` (V16) and empties the whole aggregate on any column mismatch across
homes (V17); a flagged, zero-metric home is still counted as reliable. Streaming each home
through a fleet-owned reducer, folding in fleet entry order independent of thread count and
chunk size, with each home's selection restricted to the variables it declares and per-column
coverage weight, fixes all of it: memory is bounded, the result is reproducible, a home missing
some columns still contributes to the columns it has, and only a home whose status is `Ok`
counts toward covered weight.

**Rejected alternative.** A streaming fold as each home's batches flush, with a status table,
`weight_covered`, and a rule to raise on zero covered weight. The status table, `weight_covered`
concept, and raise-on-zero-coverage rule are kept as the default coverage policy; the streaming,
completion-order fold itself is rejected because it is exactly the mechanism that makes the fleet
aggregate depend on thread scheduling.

### Numeric fixes

**Decision.** Every confirmed defect that moves numbers is fixed, each in its own commit with
golden re-capture and a recorded delta: weather-record alignment (V4), EnergyPlus-grade warm-up
convergence including NaN (V22-V24), Python warm-up default equal to Rust, billing month boundary
(V6), end-use rollup by typed kind (V9, V10), null for not-applicable values (V14), metrics
independent of verbosity (V29), deterministic fluid-solver summation (V8), ResStock weather year
(V42), missing defaults directory is an error (V19), Mode aggregation, fleet local time labels.

**Reasoning.** Each of these changes an actual output number for at least one fixture, which is
why each is isolated to its own commit rather than folded into the core architecture work: anyone
checking the commit history, or a downstream consumer, needs to be able to see exactly which
commit moved which number, re-run the golden comparison for that one change, and record the delta,
without an architecture change and a numeric change landing in the same diff and becoming
inseparable.

**Rejected alternative.** Treating input-strictness and physics fixes as a separate work item
outside I-08 entirely, deferred to a later initiative. Rejected under the standing rule that bugs
found along the way are fixed inside the same initiative as separate, ordered ledger entries, not
deferred elsewhere; every one of these defects was found while doing the runtime, telemetry, and
output-reporting work I-08 already exists to do.

### Precedence

**Decision.** Overrides the engine cannot apply are rejected (accepted means applied). HPXML
`AssemblyEffectiveRValue` wins over a precomputed RC value, as a declared numeric change in its
own commit.

**Reasoning.** Today, a non-object HPXML override is a silent no-op, `all`/`*` override targets
merge ambiguously, and untyped override values are dropped (V37); nine separate `.ok()` call
sites turn a resolution failure into a silent fallback rather than an error (same evidence). An
override that the engine cannot actually apply must be visible as a rejection, not absorbed
silently, so a caller who thinks an override took effect is never wrong about that. Between two
sources of the same physical quantity, the more direct physical input (the HPXML-declared
assembly R-value) wins over a value that was already precomputed from a lookup table, because the
lookup table is a nearest-row selector and produces a staircase rather than the physical value the
HPXML field states directly.

**Rejected alternative.** Keeping today's warn-and-continue behavior for resolution failures,
on the reasoning that a physically implausible override is rare enough not to warrant an error.
Rejected because "accepted means applied" cannot hold if some accepted overrides are silently
dropped; a caller has no way to distinguish an override that was rejected from one that was
applied, short of reading warning logs.

### Reproducibility

**Decision.** Bitwise identical per platform across processes, thread counts, and runs, tested.
Cross-platform agreement is measured and published.

**Reasoning.** The existing determinism regression test sets `RAYON_NUM_THREADS` after the
thread pool has already initialised (a no-op) and sorts values while dropping keys before
comparing (V30), so it cannot catch the fluid solver's `HashMap` iteration-order sum (V8) or the
fleet's completion-order fold. A real determinism test needs a whole-frame hash across an actual
thread-count change, keyed by name, run in separate processes.

**Rejected alternative.** Requiring bitwise-identical results across platforms (for example
macOS arm64 and Linux x86_64), which would forbid FMA contraction and any platform-specific libm
implementation in the hot path. Rejected as unnecessarily costly for the platforms actually in
use today; cross-platform agreement is measured and published instead, and the stricter
requirement is revisited only if a consumer needs it.

### Package

**Decision.** Python distribution `hares-py` (`hares` on PyPI is an unrelated project), import
name `hares`, cache directory `~/.cache/hares`. No alias for `ochre_next`.

**Reasoning.** The engine's Python exception hierarchy is already rooted under the module path
`ochre_next._hares` (V3), which is itself part of why exceptions fail to pickle; every caller of
the Python binding is already migrating for the other breaking changes in this initiative (the
config break, the results model, the error hierarchy), so renaming once in the same release costs
nothing additional. A caller who imports `hares` and hits an exception raised under
`ochre_next._hares` today would fail to catch it correctly even with the pickling fix in place, if
the module path were left as legacy naming.

**Rejected alternative.** Keeping `ochre_next` as an alias for backward compatibility alongside
the new `hares` name. Rejected outright: this is a greenfield codebase with no backward-
compatibility guards, and every Python caller migrates once across this initiative's breaking
changes regardless.

### Engine performance

**Decision.** In scope, profile-driven: telemetry storage without per-step allocation, exogenous
forcing reuse shared through the parsed weather, envelope solar/incidence reuse, thermal input
preparation allocations, thread-safe concurrent dwellings. The coupled factorization is removed
as dead code (the implicit matrix is always the identity, ledger SL-7), not reused. Each change is
bitwise frame-equal before and after; allocation and factorization counts are tests; performance
shares are recorded measurements. An item the profile shows is not worth it is dropped with the
measurement recorded.

**Reasoning.** Per-step telemetry today uses `clone_from` rather than an allocation-free write
(V27), and the in-memory step history grows unbounded while `step()` clones its last entry on
every call (V26); both are measurable, avoidable per-step costs. Because the recorder itself is
being rebuilt as part of the output model, its allocation profile should be measured after that
rebuild, not before, so the performance work is sequenced to run against the new recorder rather
than the old one.

**Rejected alternative.** Scoping the allocation-free telemetry clone narrowly to whichever
consumer motivated it, bundled together with template, forcing-cache, and threading changes as
one unit. Rejected because telemetry layout has no dependency on any one consumer's template path;
scoping it that narrowly would tie a general performance fix to one caller's usage pattern for no
reason.

### Stray outputs

**Decision.** The stray `dwelling_*.csv` and `dwelling_*_diagnostics.csv` files in the repository
root are listed for the operator, then removed. Verification compares ignored-file listings
before and after each run; any new stray file fails.

**Reasoning.** These files exist in the repository root today as a direct consequence of the
diagnostics CSV writer's current-working-directory fallback (V13) and are a visible symptom of
the same defect class as the defaults-directory CWD dependency (V19): something silently writing
relative to wherever the process was launched from. Removing them and adding a stray-file check to
verification turns "did this run write something nobody asked for" into a check that runs every
time, rather than something noticed only by inspecting the repository tree by hand.

**Rejected alternative.** None; there was no serious case for leaving these files in place or for
skipping the ongoing check once the underlying CWD-dependent writes are fixed.

### Consumer

**Decision.** The mechanical consumer migration lands before the engine numeric fixes, gated
bitwise against a baseline. The consumer re-baselines once, after all engine numeric fixes.

**Reasoning.** The consumer currently works around several of the confirmed defects itself: a
roll-plus-one workaround for the weather lag (V4), a SHA-256 build identifier standing in for a
real build identity (V18), `hasattr` probing standing in for a typed catalogue (matching V47's
resolution). Landing the mechanical migration (adapting to the new API shape, with no numeric
change) before the numeric fixes means the consumer's own re-baseline happens exactly once, after
every numeric fix has landed, instead of once per numeric fix.

**Rejected alternative.** A per-numeric-fix re-baseline on the consumer side, re-baselining after
each engine commit that moves a number. Rejected as unnecessary churn: since every numeric fix
identified is already known and ordered, a single re-baseline after all of them land verifies the
same end state with a fraction of the consumer-side work.
