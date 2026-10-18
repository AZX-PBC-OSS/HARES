# I-08 Plan: ledger of changes

## 1. Purpose and how to use this ledger

I-08 optimizes HARES's runtime, telemetry and output reporting, memory
overhead, performance and efficiency, starting from HARES#28 (results
without paying for a disk write). It is not a general bug hunt. Bugs and
broken windows found along the way that fall outside that purpose are
tracked and fixed inside I-08 as separate entries, ordered after the
entries they depend on, each in its own commit.

This file is the index (`decision-log.md` `dl-i08-process.plan_form`). It
states the purpose, the binding design the ledger entries build on, and
the rules every entry follows. The entries
themselves live in `ledger/`, one file per area, so each can be worked on
alone. `ledger/00-schedule.md` gives the single global order (engine and
consumer, initiative work and defect fixes, interleaved) with each
row's commit message and, for a row that has run, its state and every
commit hash it landed as. Implementation continues in the implementer's
order (`decision-log.md`, `dl-i08-process.readiness`): the ledger follows
the code, each commit recorded in the schedule's State column as it lands
and audited independently by a reviewer who did not implement it. A
verification round over rows not yet executed corrects their entries
ahead of the implementer rather than gating when a row starts. A finding
raised by such a round is fixed in the ledger before the entry it
concerns is implemented; a finding against an entry that is already
committed becomes a new fix entry in the area that owns the code,
scheduled after the rows it depends on.

Ledger areas:

- `ledger/01-baseline-repair.md`: the head does not pass the verification
  block; these entries make it pass, then upgrade every dependency to its
  latest release, before anything is measured or optimized. BR-10 to
  BR-15 fix what verification found in opening rows that were already
  committed (CI triggers, the golden tool's manifest semantics and
  fixtures, test files no target compiles); BR-16 to BR-19 record the
  defects found and fixed in their own commits while the opening rows ran.
- `ledger/02-measurement.md`: the reference state the opening sequence
  starts from, the golden capture/compare tool and its fixtures (committed
  as per-column bit digests plus metrics) that every bitwise proof and
  recorded delta depends on, the baseline measurement, and the
  cross-platform reproducibility measurement.
- `ledger/03-output-reporting.md`: HARES#28, the variable catalogue,
  report requests, reporting frequency, destinations (including the batch
  callback), one `RunResult`, stepped-run finalisation, health recorded
  and physics invariants enforced in every build, `HealthPolicy`, the
  diagnostics report, `Time`, metrics computed only when requested,
  checkpoint/restore of the run's accumulators, `metrics_from_table` and
  replay export.
- `ledger/04-fleet.md`: fleet memory bound, status, aggregation, time
  alignment.
- `ledger/05-python-surface.md`: the package rename, one config type,
  Python export, the Python API surface, the callback and Python actor
  outputs, RL/gym, build identity.
- `ledger/06-runtime-memory.md`: the engine performance program (telemetry
  allocation, forcing reuse, envelope and thermal allocation, parse-once
  and content inputs, per-run state and threads, resolved-parameter
  read-back, per-step RL observation allocation).
- `ledger/07-profiler.md`: HARES#31 in full: phase attribution, the
  high-water mark read, the allocation counter, finalisation timing, the
  Python profiling binding, and the per-actor profiling binding.
- `ledger/08-consumer.md`: the calibration consumer's engine-under-test
  mechanism, gate, DE-trace baseline, migration, its gate repairs and its
  own performance program.
- `ledger/09-numeric-fixes.md`: the enumerated numeric-affecting defect
  fixes named in the decision log, the precedence change and the move of
  everything HARES stores and computes to SI units, each isolated and
  re-baselined.
- `ledger/10-slop-cleanup.md`: slop found in the code these entries
  touch (narrating or useless comments, planning/ticket IDs in code,
  verbose hedging docs, dead code, silent fallbacks, duplicated logic,
  misleading comments), and the move of planning and review records out of
  the public tree. Each item is fixed alongside the entry that touches that
  code and tracked here so nothing found is dropped; there is no separate
  global sweep (`dl-i08-architecture.slop_scope`).
- `ledger/11-other-defects.md`: confirmed defects outside the purpose
  above that are not already fixed as a consequence of an area above,
  including the test environment that kept the OCHRE comparison and the
  fleet conversion tests from running, the pint cap the OCHRE reference
  needed, and the triage of unexplained energy found by a test.
- `ledger/12-inputs.md`: input records and digests,
  `DwellingBlueprint::validate()`, the machine-readable input schema
  (including the HPXML elements the parser reads), `resample_weather`,
  the OS-HPXML setpoint-source rule, the removal of the OCHRE resampling
  overrides from the Rust core, and the envelope resistance and
  thermal-mass scales that never re-select a LUT row.

Every entry has the same shape: ID, title, kind, problem, evidence, change,
acceptance test(s), verification, one commit message (or "none" for a
measurement or operator step), dependencies, and, for anything that touches
a number or a hot path, a measurement (recorded, never asserted). Evidence is
`file:line` at `ebdb1a17`; a citation into a file that PR #27 changes
(`crates/hares-python/src/py_dwelling.rs`, `crates/hares-python/Cargo.toml`,
`python/ochre_next/data/weather.py`, the new
`crates/hares-core/examples/profile_sim.rs`) is re-verified at the Task 0
head (§3) before its entry starts. An entry added after execution started
cites its evidence at the commit it names. Consumer evidence is at `c71c542`. No
entry contains an implementation body; code is written during
implementation, test-first.

Every type an entry names is defined once, in §2, with the crate that owns
it; an entry cites §2 rather than restating a shape. Where two entries
build one capability (content inputs, `ResolvedModel`, per-run state,
`RunHealth`, `HealthPolicy`), §2 names the entry that owns each part and
the other entries cite it.

## 2. Design frame

The decisions below are binding (`decision-log.md`, entries `dl-i08-*`).
They are restated here only to the depth a ledger entry needs to cite them
precisely; the decision log is authoritative if this summary and it ever
disagree.

### 2.1 Results

Crate dependency graph (the `[dependencies]` tables of the manifests;
every type below lives in a crate that can see every type it names):

| Crate | Depends on (workspace crates) |
|---|---|
| `hares-types` | none |
| `hares-physics`, `hares-control`, `hares-tariff` | `hares-types` |
| `hares-envelope` | `hares-types`, `hares-physics` |
| `hares-equipment` | `hares-types`, `hares-physics`, `hares-control` |
| `hares-io` | `hares-types`, `hares-physics`, `hares-envelope`, `hares-control`, `hares-equipment` |
| `hares-core` | every crate above |
| `hares-fleet` | `hares-types`, `hares-io`, `hares-core` (`hares-core` names `hares-fleet` only as a dev-dependency) |
| `hares-python` | every crate above except `hares-envelope` |

The `Equipment` trait lives in `hares-equipment`
(`crates/hares-equipment/src/lib.rs:173`), below `hares-io`, so a type an
`Equipment` method returns or an equipment config implements lives in
`hares-types`. `hares-io` has no pyo3 dependency; only `hares-python` does.

Units: everything HARES stores or computes is SI (W, J, s, m, and
degrees Celsius for temperature; VA and VAR for apparent and reactive
power), the rule `dl-i08-architecture.si_units` states: equipment and
actor configuration, control signals and power setpoints, actor and
equipment telemetry, billing and metrics running state, per-step results
and gas rates. `ledger/09-numeric-fixes.md` B-14 brings the code there in
three numeric commits (equipment config; run state; control layer), and
the types below show their final form ("Snapshot units" and `StepResult`
below). Output records keep the reporting unit their consumer reads:
report columns carry each `VariableDef`'s declared `Unit` (kW, kWh, kVAR,
therms/hour, §2.2), and `RunMetrics`, `BillingPeriodSummary` and its
snapshot report kWh, kW, therms and currency. A value is converted once,
with `uom`, where it leaves the running state: when the row plan writes it
into a report row, when the metrics calculator writes `RunMetrics`, and
when the tariff evaluator prices it (rates are per kWh and per kW) or
writes a `BillingPeriodSummary`.

Serde: the workspace `serde` dependency gains the `rc` feature in
`ledger/03-output-reporting.md` OR-1's commit, because the new serde types
hold `Arc<str>` and `Arc<[Arc<str>]>` (`EnumLabels::Runtime`, reached
from `TelemetryField` through `EquipmentDescriptor` and from
`VariableKind::Enum`; `VariableSource::Actor`; `Catalogue::from_json`; the
checkpointed `Warning`). The hand-written
`ser_arc_str`/`de_arc_str` helpers (`crates/hares-control/src/dispatch.rs:102-110`)
are deleted in the same commit. `serde_json` gains `float_roundtrip` in
`ledger/03-output-reporting.md` OR-17, its own commit directly before
OR-10: every checkpoint is JSON (`checkpoint.rs:114,159-167`), and without
it `serde_json` parses floats best-effort, so a restored `f64` is not
guaranteed to equal the saved one on bits. Cargo unifies the feature into
every crate of a build, so it also changes how the Python wheel parses
tariff and override JSON; OR-17 proves that bitwise-neutral on every
golden and consumer input, or it is a numeric change (§2.8).

One typed `RunResult` is produced by `finish()`, for `simulate()` and for
stepped runs alike. `simulate()` is "step to the configured end, then
`finish()`". Stepped runs call `finish()` explicitly. There is no retained
step history (`SimulationResults.steps`, `mod.rs:1034-1035`, is deleted), no
two-column fallback frame, no `initialize()` checkpoint serialisation, and
no per-step callback. `step()` returns `StepResult` by value.

```rust
// crate: hares-core (crates/hares-core/src/result.rs, new)
pub struct RunResult {
    pub reports: Vec<ReportResult>,
    pub metrics: Option<RunMetrics>,       // Some exactly when SimulationConfig.metrics is true (§2.2 Metrics);
                                           // until 03 OR-8, Some exactly when the run has at least one report
    pub health: RunHealth,                 // §2.3
    pub warnings: Vec<Warning>,
    pub warnings_dropped: u64,             // past WarningLog::CAPACITY
    pub catalogue: Arc<Catalogue>,         // §2.2
    pub provenance: Provenance,
}

pub struct ReportResult {
    pub name: String,
    pub frequency: Frequency,
    pub schema: SchemaRef,
    pub rows: u64,
    pub output: ReportOutput,
}

pub enum ReportOutput {
    Memory(Vec<RecordBatch>),              // batches as built; Python export rechunks by default (05 PY-1)
    File(PathBuf),                         // the published final path
    Callback { batches_delivered: u64 },
}

pub struct Provenance {                    // final form; the entry that adds each field is listed below;
                                           // no serde derives (only BuildInfo serialises, into replay.toml)
    pub build: BuildInfo,
    pub catalogue_digest: [u8; 32],        // Catalogue::digest() (§2.2), defined in 03 OR-7
    pub settings: DwellingSettings,        // §2.7: the validated SimulationConfig, bldg_id, overrides, patches
                                           // and warm-up the dwelling ran with, the master seed in effect included
    pub inputs: InputRecords,              // §2.7
    pub tariff: Option<TariffRecord>,      // the electric tariff attached with Dwelling::set_tariff
    pub replay_blockers: Vec<ReplayBlocker>,  // each category once, in the order first seen (03 OR-16)
    pub site_utc_offset_s: i32,
    pub equipment: Vec<EquipmentProvenance>,
}

pub struct TariffRecord {
    pub tariff: ElectricTariff,            // hares-tariff, types.rs:264 (Serialize, Deserialize)
    pub zone: String,                      // IANA name of the Tz passed to set_tariff
}

pub struct EquipmentProvenance {
    pub name: String,
    pub id: EquipmentId,
    pub zone: Option<ZoneId>,
    pub zone_type: Option<String>,
    pub ocv_source: Option<String>,
    pub notes: Vec<String>,                // today's Equipment::provenance_lines()
}

pub struct BuildInfo {                     // Serialize only: &'static str fields cannot be deserialised
    pub version: &'static str,             // CARGO_PKG_VERSION
    pub commit: Option<&'static str>,      // None when built without a git work tree
    pub dirty: Option<bool>,
    pub profile: &'static str,
    pub features: &'static [&'static str],
    pub rustc: &'static str,
}

// existing type, crates/hares-core/src/dwelling/mod.rs:1020; returned by value; final form
pub struct StepResult {
    pub timestamp: NaiveDateTime,          // local, labelled at step start (§2.5)
    pub net_electric_power_w: f64,
    pub zone_temperatures_c: Vec<(ZoneId, f64)>,
    pub hvac_heating_w: f64,
    pub hvac_cooling_w: f64,
    pub gas_power_w: f64,
}

// crate: hares-types (crates/hares-types/src/warning.rs, new; 03 OR-11): equipment (hares-equipment)
// and the construction-time resolvers (hares-io) raise warnings, and neither can see hares-core
pub struct Warning {                       // Debug, Clone, PartialEq; Serialize, Deserialize from 03 OR-10
    pub step_index: Option<u64>,           // None for construction and warm-up; set by the dwelling, never by
                                           // the producer, for a warning drained after a step
    pub source: Arc<str>,                  // equipment, actor or subsystem name ("hpxml", "schedule", "tariff")
    pub message: String,
}
// the Equipment trait (hares-equipment) method that hands an equipment's warnings to the dwelling
fn drain_warnings(&mut self, out: &mut Vec<Warning>) {}  // default: none

// crate: hares-types (crates/hares-types/src/error.rs; 03 OR-16), carried by HaresError::NotReplayable
pub enum ReplayBlocker {                   // Serialize, Deserialize, Clone, PartialEq, as HaresError
    BlueprintEdited,                       // DwellingBlueprintBuilder mutators changed the parsed equipment
    EquipmentChanged,                      // equipment or battery and EV lookup tables changed after build
    CallerActor,                           // an actor registered by the caller (add_actor, add_actor_by_name)
    ExternalInput,                         // a control signal, dispatch, price signal, grid voltage or clamped-
                                           // action count pushed in by the caller (the gym's actions included)
    StateRestored,                         // load_checkpoint, restore_building_state or a caller's warm-up call
}
```

`Provenance` is built up by the entries that own its parts:
`ledger/03-output-reporting.md` OR-6 creates it with `config:
SimulationConfig` and `bldg_id: i64` in place of `settings`, plus
`site_utc_offset_s` and `equipment` (today's diagnostics equipment
preamble, `mod.rs:3083-3114`, becomes `EquipmentProvenance`);
`ledger/05-python-surface.md` PY-6 adds `build` and `catalogue_digest`;
`ledger/12-inputs.md` IN-1 replaces `config` and `bldg_id` with `settings`
(the `DwellingSettings` of `ledger/06-runtime-memory.md` RT-4, an earlier
row) and adds `inputs`; OR-16 adds `tariff` and `replay_blockers`. Replay
export reads only `Provenance` and the report outputs, so every field it
writes is one of these. `Provenance` and its parts derive no serde: replay
export writes its own manifest from them, so no row before OR-16 adds a
derive to a type only because `Provenance` holds it.

`Warning` is one type from the first warning a crate below `hares-core`
raises: `ledger/03-output-reporting.md` OR-11 creates it with the
`drain_warnings` equipment method, a `&mut Vec<Warning>` collector on the
construction-time resolvers, `Building.parse_warnings` and
`ElectricTariff.parse_warnings` (strings, source `tariff`, pushed by
`set_tariff`), while `WarningLog` still stores strings; OR-6 makes
`WarningLog` store `Warning` values and puts them into `RunResult`; OR-10
adds their serde derives for the checkpoint. Every later producer
(`ledger/06-runtime-memory.md` RT-5's plausibility sink and warn-once
flags, `ledger/12-inputs.md` IN-5's setpoint warnings) pushes through the
same channel; none adds a second one.

`StepResult.net_electric_power_kw` (today's field) mixes kW into a struct
whose other powers are W. `ledger/09-numeric-fixes.md` B-14's run-state
commit moves the electrical solver to W and renames the field
`net_electric_power_w`, with the Python step dict key renamed to match;
the `Total Electric Power (kW)` column keeps its name and unit, and the row
plan converts the W value once when it writes the row.

`RunMetrics` is today's `hares_io::output::metrics::FullSimulationMetrics`
(`metrics.rs:237`) renamed, with the `SimulationMetrics` fields and
`gas_energy` flattened into it and the `Deref` shim (`metrics.rs:245-250`)
deleted; it stays in `hares-io`. Its `Reliability` field (`metrics.rs:230`)
stays: it describes the rows the metrics were computed from, which matters
for `metrics_from_table` over external tables. There is no separate
`MetricsStatus` type; a metric that cannot be computed from the run's
variables is `None` in its own `Option` field, documented per field.

Lifecycle contract (`Dwelling`, `hares-core`):

- `finish(&mut self) -> Result<Arc<RunResult>>`. The first call finalises
  billing, flushes and publishes every destination, and assembles the
  result. Later calls return the same `Arc` (no copy, no re-run of
  billing). `finish()` before the first step is
  `HaresError::Simulation("finish() called before any step")`.
- `finish()` before the configured end is legal: it closes the partial
  billing period and the partial reporting periods (§2.6).
- `step()` after `finish()` is `HaresError::Simulation("step() called after
  finish()")`.
- `simulate(&mut self) -> Result<Arc<RunResult>>`.
- `SimulationEngine::run` and `run_dwelling` (`engine.rs:102,251`) return
  `Result<Arc<RunResult>, HaresError>` through one shared function; a panic
  caught at that boundary becomes `HaresError::Simulation` naming the panic
  payload. `engine::SimulationResults`, `empty_metrics`,
  `finalize_run_metrics`, `resolved_output_path` and
  `compute_metrics_from_batches` are deleted.

Checkpoint/restore resumes the run, not just its physics.
`DwellingCheckpoint` (`crates/hares-core/src/checkpoint.rs:74-108`) carries,
alongside the physics state: the tariff billing state and its step index,
the closed billing summaries, the metrics accumulators (present exactly
when the run requested metrics), the partial-period report reducers, the
`RunHealth` counters and the warning log. A run
restored with `Dwelling::load_checkpoint` and driven to its end produces the
same `RunResult.metrics`, billing summaries, `RunResult.health` and
warnings as an uninterrupted run, bitwise. On restore, `Memory` report rows
after the checkpoint step are discarded. Restoring a dwelling with a file
or `Callback` destination, or restoring a metrics section into a dwelling
that did not request metrics (or the reverse), is a typed error. The new
checkpoint sections are serde snapshot types owned by the crate that owns
the state: `TariffSnapshot` and `BillingPeriodSummarySnapshot`
(`hares-tariff`; the workspace `chrono-tz` has no `serde` feature, so no
`DateTime<Tz>` is serialised), `MetricsSnapshot` and `ReducerSnapshot`
(`hares-io`), and `RunHealth` plus the warning log (`hares-core`). The
closed billing summaries are the `DwellingCheckpoint` section
`billing_summaries: Vec<BillingPeriodSummarySnapshot>`, because the
dwelling holds them (`mod.rs:1705`), not the evaluator.
`ledger/03-output-reporting.md` OR-10 implements this.

```rust
// crate: hares-tariff (crates/hares-tariff/src/snapshot.rs, new)
pub struct LocalInstant {                  // a local wall-clock time, never a UTC instant
    pub local: NaiveDateTime,
    pub utc_offset_s: i32,                 // the zone's offset at that time; resolves a DST fold or gap exactly
}
pub struct TariffSnapshot {
    pub zone: String,                      // IANA name of the evaluator's Tz
    pub step_index: u64,
    pub finished: bool,
    pub finalized: bool,
    pub billing: BillingSnapshot,
}
pub struct BillingSnapshot {               // every mutable field of BillingState (billing.rs:75), final form;
    pub period_start: LocalInstant,        // billing_cycle and max_prior_periods come from the tariff
    pub period_end: LocalInstant,
    pub cumulative_import_j: f64,
    pub cumulative_export_j: f64,
    pub cumulative_energy_cost_usd: f64,
    pub cumulative_export_credit_usd: f64,
    pub peak_demand_w: f64,
    pub period_peak_demand_w: Vec<f64>,
    pub prior_peaks_w: Vec<f64>,
    pub prior_period_peaks_w: Vec<Vec<f64>>,
    pub demand_window: DemandWindowSnapshot,   // samples (W), head, count, running_sum (W), push_count
    pub running_cpp_event_hours: u32,
    pub last_cpp_hour: Option<u32>,
    pub cumulative_ev_j: f64,
}
pub struct DemandWindowSnapshot {
    pub samples: Vec<f64>, pub head: u32, pub count: u32, pub running_sum: f64, pub push_count: u64,
}
pub struct BillingPeriodSummarySnapshot {  // one closed BillingPeriodSummary (billing.rs:410-421), an output
    pub period_start: LocalInstant,        // record in its reporting units
    pub period_end: LocalInstant,
    pub energy_charge_usd: f64,
    pub demand_charge_usd: f64,
    pub fixed_charge_usd: f64,
    pub export_credit_usd: f64,
    pub net_bill_usd: f64,
    pub peak_demand_kw: f64,
    pub total_import_kwh: f64,
    pub total_export_kwh: f64,
}
impl BillingPeriodSummarySnapshot {
    pub fn from_summary(summary: &BillingPeriodSummary) -> Self;
    pub fn to_summary(&self, tz: Tz) -> Result<BillingPeriodSummary, SnapshotError>;
}
pub enum SnapshotError {                   // thiserror (03 OR-10 adds the dependency to hares-tariff)
    UnknownZone { zone: String },          // TariffSnapshot.zone is not an IANA name chrono-tz knows
    ZoneMismatch { snapshot: String, evaluator: String },  // restoring into an evaluator in another zone
    UnresolvedLocalInstant { local: NaiveDateTime, utc_offset_s: i32, zone: String },  // no time of the zone
}                                                                // at that wall clock has that offset

// crate: hares-io (crates/hares-io/src/output/snapshot.rs, new)
pub struct MetricsSnapshot {
    pub inputs: Vec<String>,               // the metrics input plan's variable names; must equal the restoring plan's
    pub accumulators: MetricsAccumulators, // see below
}
pub struct ReducerSnapshot {
    pub report: String,
    pub period_start_ms: i64,              // local, as the Time column
    pub step_count: u32,
    pub sums: Vec<f64>,                    // per projected variable: the running sum (Sum, Mean; Mean is
                                           // sum / valid_steps at period close) or the last valid value (Last)
    pub valid_steps: Vec<u32>,
    pub mode_counts: Vec<Vec<u32>>,        // per Enum variable, indexed by code; empty for other kinds
}
```

`MetricsAccumulators` is the serde struct of every running total, peak,
count and rolling buffer `MetricsCalculator` keeps today: the fields from
`total_electric_energy_kwh` (`metrics.rs:391`) through `nan_step_count`
(`:431`), less the two that are not running state: `has_envelope_columns`
(`:415`, a property of the input plan the restoring freeze rebuilds) and
`config` (`:428`, the restoring dwelling's own `SimulationConfig`). The
`observe`-only `end_use_equipment_counts` after them (`:434`) is a
diagnostic and is not carried. The calculator holds one and the snapshot
clones it, so the two cannot drift. The calculator's
column indices are not carried: the restoring dwelling's freeze rebuilds
them, and `inputs` makes a restore into a different metrics input plan a
typed error. The `observe`-only counters on `TariffEvaluator` are
diagnostics, not run state, and are not carried.

Snapshot units: a snapshot copies its live state field for field, so a
restore is bitwise. OR-10 creates `BillingSnapshot` and
`MetricsAccumulators` in the units `BillingState` and `MetricsCalculator`
hold at that row (`cumulative_import_kwh`, `peak_demand_kw` and their
siblings; the calculator's kWh, Wh and therm totals), with `CHECKPOINT_VERSION`
9. `ledger/09-numeric-fixes.md` B-14's run-state commit (commit 3, a
numeric row after OR-10) moves `BillingState`, the calculator's running
totals (gas energy included), the equipment run state and the electrical
summary to J and W, converting to kWh, kW and therms only when a value is
priced or written into `BillingPeriodSummary` or `RunMetrics`; it renames
the snapshot fields to the SI names shown above, gives
`MetricsAccumulators` the same SI fields, and raises `CHECKPOINT_VERSION`
to 10, so a version 9 checkpoint is rejected rather than misread. B-14's
control-layer commit (commit 4) changes actor state only, inside actor
blobs that carry their own `schema_version`, which each changed actor
raises.

### 2.2 Output model

```
catalogue → report request → row plan → destination
```

**Catalogue.** Every variable the built model can produce is a
`VariableDef`: unit, kind, aggregation, sampling, nullability, source. The
catalogue is built once at assembly from typed declarations, never from
parsing a display-name string: the dwelling-level static table, a static
per-capability table for the core-output columns every equipment produces
from `CoreOutput`/`CoreCapabilities` (electric, gas, SOC, mode), each
equipment's `descriptor().telemetry_fields` (the `EquipmentDescriptor`
field, `crates/hares-types/src/equipment.rs:1921`; the private
`fn telemetry_fields()` helpers in the equipment modules fill it), a
static per-zone-template table keyed by the zone display name
(`zone_display_name`, `crates/hares-core/src/dwelling/mod.rs:7980`),
and each actor's `Actor::output_variables()`. Duplicate
names are an assembly error. It does not depend on the selected reports,
and it is queryable before the run: `Dwelling::catalogue()` from a built
dwelling before its first step (`ledger/03-output-reporting.md` OR-1), and
`DwellingBlueprint::catalogue()` without building
(`ledger/12-inputs.md` IN-2, through the assembly that `validate()` runs
without warm-up). A blueprint's catalogue is the catalogue of a dwelling
built from it before any `set_tariff` or caller-registered actor, which
add actor variables.

Static tables hold only `'static` data: a static `VariableDef`'s name is
`Cow::Borrowed` and its enum labels are `EnumLabels::Static`; `Arc` appears
only in what is declared at runtime (a Python actor's labels, an actor
source name, a catalogue read back by `from_json`).

```rust
// crate: hares-types (crates/hares-types/src/output.rs, new)
pub enum Unit { /* one variant per unit string in use at HEAD; census in 03 OR-1 */ }
impl Unit {
    pub fn symbol(self) -> &'static str;
    pub fn from_si(self, value: f64) -> f64;   // SI value of this unit's dimension -> this unit, one uom conversion
    pub fn to_si(self, value: f64) -> f64;     // the inverse, for metrics_from_table over a table in reporting units
}                                              // (both identity for SI and dimensionless units); from 09 B-14 commit 3
pub enum VariableKind {
    Energy,                                // the energy of the step (a flow), never a stored amount
    Power, Temperature, Ratio,
    State,                                 // binary 0/1
    Enum { labels: EnumLabels },           // code i decodes to labels[i]
    Quantity { across_homes: AcrossHomes },  // any other physical quantity (flow, voltage, irradiance, duration,
}                                          // count, a stock or a running total)
pub enum EnumLabels {                      // serialises as a list of strings; deserialises as Runtime
    Static(&'static [&'static str]),       // the static tables and the equipment and built-in actor declarations
    Runtime(Arc<[Arc<str>]>),              // labels declared at runtime (a Python actor) or read by from_json
}
impl EnumLabels { pub fn get(&self, code: usize) -> Option<&str>; pub fn len(&self) -> usize; }
                                           // PartialEq compares the label strings, whichever variant holds them
pub enum AcrossHomes { Sum, Mean }         // a Quantity's fleet rule across homes (§2.6); weighted in both cases
pub enum Aggregation { Sum, Mean, Mode, Last }  // §2.6; Last: the value at the period's last valid step
pub enum Sampling { IntervalMean, InstantAtStart, InstantAtEnd }

// replaces today's TelemetryField (crates/hares-types/src/equipment.rs:1869-1874);
// equipment and actors declare with the same type
pub struct TelemetryField {
    pub name: Cow<'static, str>,
    pub unit: Unit,
    pub kind: VariableKind,
    pub aggregation: Aggregation,
    pub sampling: Sampling,
    pub nullable: bool,
    pub description: Cow<'static, str>,
}

// crate: hares-io (crates/hares-io/src/output/catalogue.rs, new)
pub struct VariableDef {
    pub name: Cow<'static, str>,
    pub unit: Unit,                        // serialises as Unit::symbol() (here, in FieldSchema and HpxmlElementSchema)
    pub kind: VariableKind,
    pub aggregation: Aggregation,
    pub sampling: Sampling,
    pub nullable: bool,
    pub source: VariableSource,
    pub description: Cow<'static, str>,    // from TelemetryField.description or the static table
}
pub enum VariableSource {
    Dwelling,
    Zone(ZoneId),
    Equipment(EquipmentId),
    Actor(Arc<str>),                       // the actor's registered name, Actor::name()
}
pub struct Catalogue { /* Vec<VariableDef> in schema order, name index */ }
impl Catalogue {
    pub fn variables(&self) -> &[VariableDef];
    pub fn resolve(&self, name: &str) -> Option<&VariableDef>;
    pub fn to_json(&self) -> String;
    pub fn from_json(s: &str) -> Result<Self, CatalogueError>;  // CatalogueError: thiserror enum in hares-io
                                                               // (Parse, DuplicateName, AggregationMismatch)
    pub fn digest(&self) -> [u8; 32];      // see "Catalogue digest" below
}

// crate: hares-core (crates/hares-core/src/actor.rs:44, required method, no default)
fn output_variables(&self) -> &[TelemetryField];
```

A variable's declared `Unit` is its reporting unit. From
`ledger/09-numeric-fixes.md` B-14's run-state commit on, every value an
equipment, an actor or the dwelling publishes to a variable is in the SI
unit of that variable's dimension, and the row plan writes
`unit.from_si(value)` into the report row; that is the one reporting
boundary for report columns, and a declared name such as
`Total Electric Power (kW)` or an actor's `charge_kw` keeps its name and
unit. Before that commit the published values are already in the
declared unit and `from_si` is not called.

`VariableDef::aggregation` is validated at catalogue build: `Energy` must
be `Sum`, `Enum` must be `Mode`, `State` must be `Mean`; `Last` is legal
only for a `Quantity`; other kinds may declare `Sum` (per-step counts,
costs, volumes) or `Mean`. Every variable whose unit is a power (`kW`,
`W`, `kVAR`, `therms/hour`) is declared `Power`. A variable in an energy
unit (`kWh`, `Wh`, `J`) is `Energy` only when it is the energy of the
step; otherwise it is a `Quantity` decided by what it measures, and so is
every count and duration:

| What the variable holds | Kind and aggregation over time | Sampling | Across homes |
|---|---|---|---|
| a stock or level (stored energy, a capacity, an energy-model bound, stages on) | `Quantity`, `Mean` | `InstantAtEnd` | `Sum` |
| a configuration constant (cell counts, rated capacities, minimum on and off times) | `Quantity`, `Mean` (the constant itself) | `InstantAtEnd` | `Sum` for a count or amount, `Mean` for a duration or intensity |
| a running total since init (a cumulative count or energy) | `Quantity`, `Last` (its value at period end) | `InstantAtEnd` | `Sum` |
| a count or amount of the step (signals sent this step) | `Quantity`, `Sum` | `IntervalMean` | `Sum` |
| a timer (time in the current mode or speed) | `Quantity`, `Mean` | `InstantAtEnd` | `Mean` |

`ledger/03-output-reporting.md` OR-1 classifies every such variable by
name in its pinned kind table. Every `Quantity` declares its rule across homes: intensive
quantities (irradiance, durations, voltage, specific heat, fuel economy)
`AcrossHomes::Mean`; extensive ones (mass and volume flows, event counts,
currents, charge capacities in Ah, byte counts) `AcrossHomes::Sum`
(`ledger/03-output-reporting.md` OR-1 declares them; `ledger/04-fleet.md`
FL-5 first folds by them). Dimensionless variables (unit `-`, `%`,
`fraction`, `ratio`) are classified by meaning, not by unit: fractions,
ratios, efficiencies, multipliers, COP, EIR and SHR are `Ratio`; 0/1 flags
are `State`; codes, identifiers, types and indices are `Enum`; event
counts are `Quantity { across_homes: Sum }`. An `Enum` declares a label at
every code it can publish, from 0: a code set that does not start at 0
declares a placeholder label at each unused code (`CHARGING_LEVEL`
publishes 1 and 2, so its labels are `["none", "L1", "L2"]`), and a code
with no name of its own is labelled by its decimal value.
`VariableDef::nullable` decides what a missing value means: for a nullable
variable (undefined in some states, or not reported by its source) the
cell is null; for a non-nullable variable a missing value is a wiring
defect that fails the run (§2.3). `Enum` is a multi-valued code (operating
mode, charging level, EV connection state, which has three values);
`State` is binary (defrost, on/off).
There is no
`ActorId` type; actors are identified by their registered name, as their
output columns (`actor:{name}:{key}`, `mod.rs:778-804`) already are. An
actor that publishes a telemetry key it did not declare fails the run
with `HaresError::InvariantViolation` naming the actor and the key: an
actor's `Telemetry` is built from its declarations and sealed
(`Telemetry::from_fields`, `ledger/03-output-reporting.md` OR-1), so an
undeclared key latches at the write, as OR-11's unknown-key latch does,
and registration rejects an actor whose telemetry is not sealed or holds
a key it did not declare.
Python-authored equipment does not exist and is not added
(`dl-i08-architecture.new_capability`); Python actors declare their output
variables through the actor API (`ledger/05-python-surface.md` PY-11).

**Catalogue digest.** `Catalogue::digest()` is SHA-256 (`sha2`, which
`hares-io` gains in `ledger/03-output-reporting.md` OR-7, the entry that
first writes it) over, for each `VariableDef` in catalogue order: the
name, `Unit::symbol()`, the kind tag and, for `Enum`, each label, for
`Quantity`, the across-homes tag (`sum` or `mean`), the aggregation tag,
the sampling tag, the nullable byte (`0` or `1`) and the source tag
(`dwelling`, `zone:<id>`, `equipment:<id>`, `actor:<name>`), each field
followed by one `0x00` byte. Tags are the lowercase variant names. The
description is not hashed: it documents a variable and changes no value. OR-7 writes it as the schema metadata `hares:catalogue_digest`
(hex); `ledger/05-python-surface.md` PY-6 copies it into
`Provenance.catalogue_digest`.

**Report request.** Selection, reporting frequency and destination are
independent axes:

```rust
// crate: hares-io (crates/hares-io/src/output/report.rs, new)
#[serde(deny_unknown_fields)]
pub struct ReportSpec {
    pub name: String,                      // unique within a config
    pub selection: Selection,
    #[serde(default)]                      // Timestep
    pub frequency: Frequency,
    pub destination: Destination,
}
#[serde(rename_all = "snake_case")]       // TOML: { verbosity = 2 }, { named = [..] }, { pattern = ".." }, "diagnostics"
pub enum Selection {
    Verbosity(u8),                         // 0..=8, the named tiers
    Named(Vec<String>),
    Pattern(String),
    Diagnostics,                           // today's diagnostics CSV columns, as catalogue variables (from 03 OR-4)
}
#[serde(rename_all = "snake_case")]       // TOML: "timestep", { minutes = 15 }, "hourly", ..., "run_period"
pub enum Frequency {
    #[default] Timestep,
    Minutes(NonZeroU32),                   // n divides 60; periods start on the local clock hour
    Hourly, Daily, Monthly, Annual, RunPeriod,
}
```

Every config enum is externally tagged with `rename_all = "snake_case"`: a
unit variant is a string, a variant with data is a one-key table. Each
variant arrives with the entry that implements it, so no commit carries a
variant that cannot run: OR-2 adds `Selection` with the `Verbosity`,
`Named` and `Pattern` variants and `Frequency::Timestep`, used inside the
row plan only; OR-3 adds `ReportSpec` and `Destination::{Memory, Parquet,
Csv}` with `SimulationConfig.reports`; OR-4 adds
`Selection::Diagnostics`; OR-13 adds the other frequencies, including
`Minutes(n)` (sub-hourly reporting, which also carries the fleet's
15-minute output, `ledger/04-fleet.md` FL-1); OR-14 adds
`Destination::Callback`. The Python constructor of each variant lands in
the same commit, from OR-3 on (`ledger/03-output-reporting.md` OR-3),
except `Destination.callback()`, which arrives with the Python attach in
`ledger/05-python-surface.md` PY-10: before it Python cannot attach a
callable, so it does not offer the marker.

`Selection::Verbosity(n)` is a preset built from the catalogue, not a
second column list; today's `output_verbosity` key is removed.
`Named` names that are not in the catalogue are an error listing them,
each with up to three catalogue names nearest by edit distance
(Levenshtein, ties by catalogue order, implemented without a new
dependency).
`Pattern` is a glob over `VariableDef.name` (`*` matches any run of
characters, `?` one character, everything else literal, case-sensitive),
implemented without a new dependency; a pattern matching nothing is an
error. `Diagnostics` selects the variables today's diagnostics writer emits
(`crates/hares-core/src/diagnostics.rs:90-114`), declared in the
catalogue with `VariableSource::Dwelling` or `Zone`; it works with every
destination, including `Memory`. HARES has no sub-timestep reporting
interval, so EnergyPlus's `Detailed` frequency has no meaning here and is
not a variant. `Minutes(n)` is a multiple of the timestep, never a
fraction of it: `SimulationConfig::validate()` rejects an `n` that does
not divide 60 and a period that is not a whole multiple of `time_res`.

**Row plan.** Built once at freeze: the union of every report's selected
variables mapped to producer slots, one reused `Vec<f64>` row, a validity
bitset and an `i64` local timestamp, addressed by index. Each report holds
a projection (indices into the union row) and a period reducer (§2.6).
Reports with equal selection and frequency share one batch builder, so the
recorder builds each batch once. There is no per-step string lookup or
hash on the output path; `Dwelling` no longer holds the name-to-column
map (`output_column_index`), so the step path cannot reach one.
**Freeze is once per run:** mutating equipment or registering an actor
after the row plan freezes is an `HaresError::InvariantViolation` (the
plan, its projections and the destinations are frozen with the first
recorded row, and both change the catalogue); before the first row, the
freeze is rebuilt on either.

**Metrics.** Metrics are computed only when the run requests them
(`SimulationConfig.metrics: bool`, default `false`;
`dl-i08-architecture.metrics_optional`). A run that does not request them
constructs no metrics accumulator and no metrics input plan, and its
`RunResult.metrics` is `None`. When requested, `RunMetrics` is computed per
step from a fixed metrics input plan built from the catalogue, independent
of verbosity, of the selected reports and of any destination.
`metrics_from_table(table, catalogue, time_res, setpoint_deadband_c) ->
Result<RunMetrics, MetricsError>` (`hares-io`; `MetricsError` is a
`thiserror` enum in `hares-io` naming the offending column) runs the same
accumulator over an Arrow table. `ledger/03-output-reporting.md` OR-8
owns this contract.

**Destinations.**

```rust
// crate: hares-io (serialisable; part of SimulationConfig)
#[serde(rename_all = "snake_case", deny_unknown_fields)]  // TOML: "memory", { parquet = { path = ".." } }, "callback"
pub enum Destination {
    Memory,
    Parquet { path: PathBuf, #[serde(default)] overwrite: bool },
    Csv { path: PathBuf, #[serde(default)] overwrite: bool },
    Callback,                              // marker: a callable is attached at runtime (from 03 OR-14)
}

// crate: hares-io (crates/hares-io/src/output/callback.rs, new); never in serde config
pub trait BatchCallback: Send + Sync {
    fn on_batch(&mut self, ctx: &BatchContext<'_>, batch: &RecordBatch) -> Result<(), CallbackError>;
}
pub struct BatchContext<'a> { pub report: &'a str, pub bldg_id: i64, pub batch_index: u64 }
pub struct CallbackError { pub message: String }

// crate: hares-core
impl Dwelling {
    pub fn attach_callback(&mut self, report: &str, callback: Box<dyn BatchCallback>) -> Result<()>;
}
```

A `Callback` report with nothing attached when the first step runs is a
typed error naming the report; attaching to a report whose destination is
not `Callback` is a typed error. `hares-python` implements `BatchCallback`
over `Py<PyAny>`; a Python exception makes the run fail with
`HaresError::Callback { report, message }`, and the Python binding raises
it with the original exception as `__cause__` (`ledger/05-python-surface.md`
PY-10). With no `Callback` report, no callback adapter exists and no Python
call is made.

Files: no rotation. A file is created lazily at the first batch, under a
unique partial name in the target's directory
(`.<file name>.partial-<pid>-<n>`), and published at `finish()`: with
`overwrite: false` by `std::fs::hard_link` to the target (which fails if
the target exists) and removal of the partial name; with `overwrite: true`
by `std::fs::rename`. The hard link plus removal is the no-clobber form of
the rename the destinations decision requires: the final name appears
atomically and complete, and never replaces a file the caller asked to
keep. On a filesystem without hard links the publish fails with
`HaresError::Io` naming the target and stating that `overwrite: false`
needs hard links there and `overwrite: true` publishes by rename; the
partial file is removed, as for any failed run. An existing target with
`overwrite: false` is an
error at freeze, before the first step, naming the path; a missing parent
directory is `HaresError::Io` naming the path; two reports naming the same
path is a `SimulationConfig::validate()` error, and two fleet homes whose
file destinations resolve to the same path is a fleet construction error
(`ledger/04-fleet.md` FL-1). A run that fails removes
its partial files. A run that produces zero rows creates no file. Nothing
is written unless a `ReportSpec` requests it; nothing defaults to the
working directory (`default_output_path`,
`crates/hares-core/src/dwelling/conversions.rs:595`, and the diagnostics
path derived from it at `mod.rs:3057-3061` are deleted). A destination
failure fails the run.

### 2.3 Health

```rust
// crate: hares-types
pub enum HealthEvent {
    PortRollback,            // an equipment step failed and its port writes were rolled back
    RejectedControlSignal,   // apply_control returned Err, or the target was not found or ambiguous
    ClampedAction,           // an RL action outside its declared bounds was clipped
    CurveIndexClamp,         // an HVAC performance-curve index was clamped
    WarmupNotConverged,
}
// new HaresError variants (crates/hares-types/src/error.rs:9)
HealthPolicyViolation {
    event: HealthEvent,
    step_index: Option<u64>,               // None for an event during warm-up or at its end, as Warning.step_index
    source: String, count: u64, limit: u64,
},
Callback { report: String, message: String },
// returned by a new Equipment trait method, take_health_counts(&mut self), default all zero
pub struct EquipmentHealthCounts { pub curve_index_clamps: u64 }

// crate: hares-io (a SimulationConfig field, `health_policy`)
#[derive(Default)]                         // the empty policy: nothing escalates (Python: HealthPolicy())
#[serde(default, deny_unknown_fields)]    // a partial [health_policy] table is legal
pub struct HealthPolicy {
    pub max_port_rollbacks: Option<u64>,
    pub max_rejected_control_signals: Option<u64>,
    pub max_clamped_actions: Option<u64>,
    pub max_curve_index_clamps: Option<u64>,
    pub require_warmup_convergence: bool,
}
impl HealthPolicy { pub fn strict() -> Self; } // every limit Some(0), require_warmup_convergence: true

// crate: hares-core (crates/hares-core/src/health.rs, new)
pub struct RunHealth {
    pub port_rollbacks: u64,
    pub rejected_control_signals: u64,
    pub clamped_actions: u64,
    pub curve_index_clamps: u64,
    pub warmup: WarmupOutcome,
}
pub enum WarmupOutcome {
    Disabled,                              // the run's warm-up is Warmup::Disabled (§2.7)
    Ran {
        days_run: u32,
        converged: bool,
        residuals: Option<WarmupResiduals>,  // None when only one warm-up day ran (no predecessor)
    },
}
pub struct WarmupResiduals {               // the last day's EnergyPlus residuals, worst zone first
    pub worst_zone: ZoneId,
    pub max_temperature_c: f64,            // |today's daily max - yesterday's|
    pub min_temperature_c: f64,            // |today's daily min - yesterday's|
    pub heating_load: f64,                 // relative change of the daily peak heating load
    pub cooling_load: f64,                 // relative change of the daily peak cooling load
}
impl Dwelling {
    pub fn health(&self) -> &RunHealth;    // live, at any step
    pub fn record_clamped_actions(&mut self, count: u64) -> Result<()>;  // for callers that clip actions (the gym)
}
```

Every health event is recorded unconditionally, in every build, as a run
total on the dwelling (never a process-global static, never reset per
step), and returned in `RunResult.health`. The dwelling itself counts
rejected control signals (an `apply_control` error, an unknown target or
an ambiguous target) and clamped actions (through `record_clamped_actions`); no process-global
counter feeds `RunHealth`. A limit is checked when its count changes: the
run fails with `HealthPolicyViolation` naming the event, the step, the
source (equipment or actor name), the count and the limit.
`require_warmup_convergence` is checked when warm-up ends. `HealthPolicy`
is empty by default; consumers that gate on health (the calibration
system) set `HealthPolicy::strict()` or their own limits.
`WarmupResiduals` are the four EnergyPlus convergence quantities
(`ledger/09-numeric-fixes.md` B-2); OR-5 records them from the first
commit while today's stopping rule still decides convergence, and B-2
makes them the stopping rule.

`Warning` (§2.1) carries every non-fatal event a run reports: a
plausibility warning from `validate_core_contract` (field, value,
equipment), the first occurrence per run of a warn-once condition
(island unserved, missing ampacity), each rejected control signal (an
`apply_control` error, an unknown target, an ambiguous target: one
warning per occurrence, as the dispatch sites push it since
`ledger/03-output-reporting.md` OR-5, `crates/hares-core/src/dwelling/mod.rs:1177-1195,1228-1233,1262-1279` at `ebdb1a17`,
and bounded by the warning log's capacity with its dropped count, §2.1),
warm-up non-convergence naming the zones, and the construction warnings
of the parsers, resolvers and equipment (an HPXML schema warning, a tariff
hemisphere guess, a schedule column HARES does not read, a multi-speed
system run single-speed, an implausible equipment configuration), which
reach the run through the channel of §2.1.

The warning log has two bounds. `RunResult.warnings` keeps the first
`WarningLog::CAPACITY` warnings of the run and counts the rest in
`warnings_dropped`. `Dwelling::take_warnings()` returns every warning
pushed since the previous call, bounded per call at `CAPACITY` with its
own dropped summary, so a caller that drains every step (the gym) keeps
receiving warnings after the run record is full.

Owners: `ledger/03-output-reporting.md` OR-5 records `RunHealth`,
`WarmupOutcome` and the warnings above; OR-11 creates `Warning` and its
channel; OR-6 gives the log its two bounds; OR-12 adds `HealthPolicy`;
`ledger/06-runtime-memory.md` RT-5 moves the remaining warn-once and
plausibility statics onto the run, pushing through OR-11's channel, and
cites OR-5 for the three counters OR-5 already moved.

NaN and physics invariant violations are not health events: they always
fail the run, in every build profile, including warm-up. Every check that
can fail a run belongs to one of three classes, all always on:

- **Physics or conservation** (energy, charge and moisture balance,
  bounds on physical state, non-negative delivered energy);
- **Non-finite** (NaN or infinity in any solver output, zone state, port,
  or telemetry value; a non-finite telemetry write is latched by
  `Telemetry` and fails the step with `NanDetected` naming the equipment
  or actor and the key; this replaces today's silent drop and its
  process-global `NON_FINITE_REJECTIONS` counter);
- **Wiring** (a missing expected observation, an unresolved expected
  variable, a stage-order or registration inconsistency).

A fourth class, **debug assertion**, is reserved for checks on code logic
that no input can trigger; it stays `cfg(debug_assertions)` with the
reason on the line. No check is exempt during warm-up. Every failing check
returns a typed `HaresError` (no panic on the run path).
`ledger/03-output-reporting.md` OR-5 records `RunHealth` and, with OR-11,
classifies every existing gated check; OR-12 adds `HealthPolicy`.

### 2.4 Fleet

Fleets never retain full frames; memory is bounded independent of fleet
size, by a deterministic ordered fold over fixed-size chunks of the
fleet's entry order (homes are folded one at a time in entry order, so the
fold sequence is the same for every chunk size and thread count). The
bound is the chunk size times one home's live result: a home's result is
dropped once folded, but while live it holds every report it requested, so
at `Timestep` frequency it is that home's full selected-column frame.
"Never retain full frames" therefore means never the frames of all homes;
`ledger/04-fleet.md` FL-1 records bytes per live home result. Output is
labelled in local site time. A fleet spanning more than one UTC offset
declares alignment: `Instant` (same physical moment, labelled in a declared
reference zone, which is a fixed UTC offset because HARES output time is
naive local standard time, §2.5) or `LocalClock` (same local wall clock).
Every home's coverage and status is reported; a flagged home is never
counted as a success.

```rust
// crate: hares-fleet (each home is run through hares-core and yields a RunResult)
pub struct FleetReport {                   // replaces AggregationResolution
    pub selection: Selection,              // §2.2; restricted to each home's catalogue (below)
    pub frequency: Frequency,              // §2.2; no default
    pub health_policy: HealthPolicy,       // §2.3; applied to every home's run
}
impl Fleet {
    pub fn simulate(&self, report: &FleetReport, n_threads: usize, chunk_size: NonZeroUsize)
        -> Result<FleetResult, FleetError>;
}
pub enum Alignment { Instant(FixedOffset), LocalClock }
pub struct FleetResult {
    pub aggregate: RecordBatch,            // local-time labelled, §2.5
    pub covered_weight: RecordBatch,       // same shape as aggregate: the weight behind each cell
    pub homes: RecordBatch,                // one row per home: bldg_id, weight, status, reason, steps,
                                           // periods_folded (+ the run metrics when requested)
    pub coverage: Coverage,
    pub peak_live_home_results: usize,     // bounded by the chunk size
}
pub struct Coverage {                      // final form; FL-1 carries weight_flagged, which FL-2
    pub weight_total: f64,                 // renames weight_degraded
    pub weight_ok: f64,
    pub weight_degraded: f64,
    pub weight_failed: f64,
}
pub enum HomeStatus { Ok, Degraded(String), Failed(String) }  // replaces SimStatus; no Skipped (unreachable)
pub enum FleetError {                      // thiserror
    NoCoveredWeight { ok: usize, degraded: usize, failed: usize, homes: RecordBatch },  // replaces ZeroWeightFleet
    AlreadyFinished { total_steps: u64 },
    HeterogeneousTimeGrid { bldg_id: i64 },
    AlignmentRequired { offsets_s: Vec<i32> },
    DuplicateDestination { path: PathBuf, bldg_ids: (i64, i64) },
    UnmatchedSelection { names: Vec<String> },  // a Named name or Pattern no home's catalogue matches
    UnresolvedWeather { bldg_id: i64, county: String },
    ReservedReportName { bldg_id: i64 },   // a home's own config carries a report named "fleet"
    NoSuchHome { index: usize, len: usize },
    Telemetry { bldg_id: i64, source: HaresError },
    // plus the construction errors that exist today
}
```

`AlignmentRequired` and `HeterogeneousTimeGrid` are raised by `simulate`
from a site-offset pre-pass that runs over every entry before the first
chunk starts: `hares_io::hpxml::read_site(path) -> Result<Site,
HpxmlError>`, `hares_io::read_weather_meta(path) -> Result<WeatherMeta,
WeatherError>`, `hares_core::dwelling::site_offset(&DwellingConfig) ->
Result<FixedOffset, HaresError>`. The time grid rule follows the
declared alignment: naive starts must agree under `LocalClock` and an
undeclared single offset; under `Instant`, start instants must fall on
the declared offset's step grid and the homes' runs must share at least
one whole reference period.

A home whose `RunResult.health` records any event (a non-zero counter or
an unconverged warm-up) or whose `warnings` is non-empty is `Degraded`,
never `Ok`. The fleet's `HealthPolicy` is `FleetReport.health_policy`
(fleet-level policy, applied to every home's `SimulationConfig` at the
per-home run; a `HealthPolicyViolation` under the fleet policy maps to
`Failed`); per-home policies come from each home's config.

Selection across homes is the one fleet exception to §2.2's rule that an
unknown `Named` name or an empty `Pattern` is an error: a home's fleet
report selects the fleet selection restricted to that home's catalogue,
and a variable a home does not declare is not applicable to it (the home
does not fail and is not part of that variable's covered weight). A name
or pattern that no home matches is `FleetError::UnmatchedSelection`,
raised once the last home has been built and before its chunk runs.
`ledger/04-fleet.md` implements this section.

### 2.5 Time

All times are local site time. Output time is a naive local timestamp
labelled at interval start, Arrow `Timestamp(Millisecond, None)`, replacing
today's Utf8 `Time` column (`crates/hares-io/src/output/columns.rs:425`,
formatted per row with chrono `"%+"` at `mod.rs:7532`, copied as an
already formatted `&str` at `writer.rs:379`, and rendered as RFC 3339 UTC
`Z` in the fleet path at `aggregation.rs:636-637` after parsing at `:505`
with `parse_timestamp`, `:444`, and keying the bucket by epoch second at
`:508`).
The site UTC offset is recorded in schema metadata as
`hares:utc_offset_s` (integer seconds), next to `hares:catalogue_digest`
(hex). `StepResult.timestamp` and the Python `step()` dict's `timestamp`
carry the same naive local value as the step's output row
(`crates/hares-python/src/conversions.rs:165-169` stops producing an
offset-aware `datetime`). Not-applicable values are null, never 0.0.
Schema metadata `hares_verbosity`, `hares_mode_map`,
`basement_lighting_created` and `garage_lighting_created`
(`columns.rs:915-927`) are deleted by `ledger/03-output-reporting.md` OR-7:
a report's selection need not be a verbosity tier, mode labels live in
`VariableKind::Enum`, and which equipment exists is in the catalogue and
`Provenance.equipment`. After OR-7 a report schema's metadata holds
exactly `hares:utc_offset_s` and `hares:catalogue_digest`.

Order of the change: the row plan carries an `i64` naive local timestamp
from OR-2, and the period reducer of OR-13 (an earlier row than OR-7)
buckets on it, but until OR-7 every report's `Time` column is today's
Utf8 string, so a period row's `Time` is its period start rendered as the
step rows are, and the reducer's batch feed parses that string; OR-7
converts both. The fleet aggregate's own `Time` stays a Utf8 RFC 3339 `Z`
label until `ledger/09-numeric-fixes.md` B-12 (`ledger/04-fleet.md` FL-1,
FL-4); from OR-7 the fleet reads each home's naive local `Time` and
subtracts `hares:utc_offset_s` to recover today's UTC bucket key.

These conventions are user documentation, not only code:
`ledger/03-output-reporting.md` OR-7 writes the "Time" section of
`docs/python.md` (naive local standard time labelled at interval start,
`hares:utc_offset_s`, the `Step Count` and `<name> [valid steps]` columns,
and how a schedule row maps to a step), with a doctest-style example the
Python suite runs.

### 2.6 Aggregation

Energy columns sum; power, temperature and ratio columns average; binary
state columns average (duty fraction); multi-valued enum columns use Mode
(most frequent value in the period, ties to the lowest code; across
dwellings, the value with the greatest summed weight). The aggregation kind
is a field of `VariableDef` (§2.2), not inferred at aggregation time.

Per-dwelling reporting frequency (`Frequency`, §2.2):

- `Timestep`: one row per step, no reduction.
- `Hourly`, `Daily`, `Monthly`, `Annual`: calendar periods in local site
  time; `Monthly` and `Annual` follow calendar months and years. The
  period length must be a whole multiple of the timestep
  (`SimulationConfig::validate()` rejects `Hourly` with a 7200 s step).
- `RunPeriod`: one row covering the whole run.
- Each period row is labelled at its nominal period start and carries a
  `Step Count` column (steps that fell in the period), so a partial first
  or last period is visible.
- A reduction is taken over the non-null step values of the period. A
  period with no non-null value is null. Each variable declared
  `nullable: true` gets a companion column `<name> [valid steps]` in every
  non-`Timestep` report; non-nullable variables get none.
- `Sum` is the sum of step values (energy per step, not power times time);
  `Mean` is the arithmetic mean over valid steps; `Mode` is as above;
  `Last` is the value at the period's last valid step (a running total's
  value at period end, §2.2). Step values are added in step order into one
  `f64` per period and variable, and `Mean` divides once.

The period reducer is one type, fed by a dwelling's report and by each
fleet home (`ledger/04-fleet.md` FL-1); `ledger/03-output-reporting.md`
OR-13 creates it:

```rust
// crate: hares-io (crates/hares-io/src/output/reduce.rs, new; workspace plumbing, §2.9)
pub struct PeriodCalendar;                 // the home's own local calendar (OR-13 creates it as a unit struct;
                                           // 04 FL-4 adds the period-origin offset Alignment::Instant needs)
pub struct TimeRule { pub aggregation: Aggregation, pub enum_codes: Option<usize> }  // from a VariableDef;
                                           // enum_codes: the label count of an Enum (Mode counts per code)
pub struct PeriodRow<'a> {                 // one closed period; borrowed, so emitting allocates nothing
    pub start_local_ms: i64,               // the nominal period start, naive local
    pub step_count: u32,
    pub values: &'a [f64],                 // per rule, in rule order
    pub valid_steps: &'a [u32],            // per rule; 0 means the period's value is null
}
pub struct PeriodReducer;
impl PeriodReducer {
    pub fn new(frequency: Frequency, calendar: PeriodCalendar, rules: Vec<TimeRule>) -> Result<Self, ReduceError>;
    pub fn push_row(&mut self, local_ms: i64, values: &[f64], valid: &[bool], emit: &mut dyn FnMut(PeriodRow<'_>));
    pub fn push_batch(&mut self, batch: &RecordBatch, emit: &mut dyn FnMut(PeriodRow<'_>)) -> Result<(), ReduceError>;
                                           // Time, then one column per rule; the same operations as push_row
    pub fn finish(self, emit: &mut dyn FnMut(PeriodRow<'_>));  // closes the partial last period
}
// ReduceError: thiserror enum in hares-io (Timestep frequency, a batch column count or type mismatch)
```

Fleets combine in two stages (`dl-i08-architecture.aggregation`): over
time within each home by the per-dwelling rule above, then across homes by
a rule declared per kind, weighted by each home's weight:

| Kind | Over time, per home | Across homes |
|---|---|---|
| `Energy` | `Sum` | weighted sum Σ wᵢ·vᵢ |
| `Power` | `Mean` | weighted sum (a fleet's power is the population's demand) |
| `Temperature`, `Ratio` | `Mean` | weighted mean Σ wᵢ·vᵢ / Σ wᵢ |
| `State` | `Mean` (duty fraction) | weighted mean |
| `Enum` | `Mode` | `Mode`: the code with the greatest summed weight, ties to the lowest code |
| `Quantity` | as `VariableDef.aggregation` declares | `VariableKind::Quantity { across_homes }` |

Across homes, each cell adds `v * w` and `w` home by home in the fleet's
entry order, and a weighted mean divides once after the last home. A null
home value contributes to neither the cell's sum nor its weight; a cell
whose covered weight is zero is null. `ledger/04-fleet.md` FL-1 builds the
fold bitwise-compatible with today's unit-keyed rules, FL-5 switches it to
the declared kinds, and `ledger/09-numeric-fixes.md` B-11 switches `Enum`
to `Mode` in both stages.

The calendar a home's fleet reduction uses is chosen before reduction
(`ledger/04-fleet.md` FL-4): for `Alignment::LocalClock`, and for a
single-offset fleet with no declared alignment, the home's own local
calendar; for `Alignment::Instant(offset)`, the declared offset's calendar,
on which each step's instant is bucketed, so homes at different offsets
reduce onto the same reference periods. Under `Instant`, a home period
whose `Step Count` is below the full period's is null for that home.

### 2.7 Config, inputs, package, reproducibility

**Config.** One Rust `SimulationConfig` (`hares-io`,
`crates/hares-io/src/config.rs:61`) with one
`validate(&self) -> Result<(), ConfigError>`. TOML (`from_toml`), synthetic
TOML and Python all build that type and call that `validate()`; the
checks now inline in `from_toml` (`config.rs:145-185`) move into it, and
so do the site-location range checks that today exist only in the Python
binding (`ledger/05-python-surface.md` PY-8), so a TOML or Rust config is
checked exactly as a Python one. PY-8 also adds
`SimulationConfig::to_toml(&self) -> Result<String, ConfigError>` in Rust
(`from_toml`'s inverse; the Python `to_toml()` wraps it), so a Rust test
round-trips a config the way a Python one does.
Synthetic TOML embeds a `SimulationConfig` table in place of
`SyntheticOutputConfig` (`crates/hares-core/src/dwelling/synthetic.rs:291-327`,
deleted). The Python `SimulationConfig` class wraps the Rust value
(`#[pyclass] struct PySimulationConfig(SimulationConfig)`), built from typed
keyword arguments whose defaults are the Rust defaults; the mirror struct
and its `to_sim_config`/`from_sim_config` (`crates/hares-python/src/py_config.rs:45-548`)
and the Python-side default constants (`py_config.rs:17-18`,
`py_dwelling.rs:61-62`) are deleted, and the same holds for
`PyDwellingConfig` (`py_config.rs:550`) over the Rust `DwellingConfig`
(`crates/hares-core/src/dwelling/mod.rs:210`). `Dwelling.from_hpxml` takes
the config only as `config=SimulationConfig(...)` plus the dwelling-level
keyword arguments (`bldg_id`, `initialization_duration` until B-2,
`defaults`, `overrides`); a simulation-config keyword next to it is a
`TypeError`.

The Python keyword of every `SimulationConfig` field is the Rust field
name, and every field is keyword-only (`ledger/05-python-surface.md` PY-8
owns the table of accepted Python types):

| Rust field | Python keyword and accepted values | Default |
|---|---|---|
| `start_time` | `start_time`: `datetime` (naive is local wall clock) or ISO 8601 string | required |
| `duration` | `duration`: `int` seconds or `timedelta` | required |
| `time_res` | `time_res`: `int` seconds or `timedelta` | Rust `default_time_res` (60 s) |
| `output_chunk_size` | `output_chunk_size`: `int` | Rust default |
| `setpoint_deadband_c` | `setpoint_deadband_c`: `float` or `None` | `None` |
| `master_seed` | `master_seed`: `int` | `0` |
| `civil_timezone` | `civil_timezone`: IANA name or `None` | `None` |
| `site_location` | `site_location`: `SiteLocationOverride(latitude_deg=None, longitude_deg=None, elevation_m=None, utc_offset_h=None)`, a frozen pyclass over the Rust type | empty override |
| `reports` | `reports`: `list[ReportSpec]` | `[]` |
| `metrics` | `metrics`: `bool` | `False` |
| `health_policy` | `health_policy`: `HealthPolicy` | `HealthPolicy()` |
| `warmup` (B-2) | `warmup`: `WarmupConfig` or `None` (`None` is `Warmup::Disabled`) | `WarmupConfig()` |
| `first_hour_interpolation` (B-1a) | `first_hour_interpolation`: `"hour24"` or `"hour1"` | `"hour24"` |
| `weather_year` (B-1c) | `weather_year`: `"typical"` or `"actual"` | `"typical"` |

String-valued enums take the serde spelling, so a Python value and its
TOML value are the same string.

Strict everywhere: `#[serde(deny_unknown_fields)]` on every config type.
A removed or renamed key is an error that names its replacement, from one
table in `hares-io`. A key is removed from one table, not everywhere (the
synthetic TOML `[simulation]` key `initialization_duration_s` is removed
while the dwelling-level key of the same name stays valid until
`ledger/09-numeric-fixes.md` B-2), so every row carries the scope it
applies to:

```rust
// crate: hares-io (crates/hares-io/src/config.rs)
#[serde(rename_all = "snake_case")]       // serialised into InputSchema.removed_keys (12 IN-3)
pub enum KeyScope {
    SimulationConfig,   // a SimulationConfig table (TOML, synthetic TOML [simulation]) or keyword
    DwellingTable,      // the top level of a synthetic TOML file; a DwellingConfig field
    PythonKeyword,      // a keyword only the Python constructors and functions ever accepted
    EquipmentConfig,    // an equipment override key, synthetic TOML equipment key or Python equipment keyword
    ActorParameter,     // a parameter of a registry actor (Dwelling.add_actor_by_name params)
    ControlSignal,      // a ControlSignal field: Python constructor keyword, HELICS signal body key, gym action key
    Observation,        // a name to_observation_vec or the gym's observation layout resolves
}
pub struct RemovedKey { pub scope: KeyScope, pub key: &'static str, pub replacement: &'static str }
pub static REMOVED_KEYS: &[RemovedKey];
pub fn removed_key(scope: KeyScope, key: &str) -> Option<&'static str>;  // the replacement
```

Each surface consults the scopes of what it accepts, in this order, and
reports the first hit: a TOML `SimulationConfig` table and the synthetic
`[simulation]` table consult `SimulationConfig`; the top level of a
synthetic TOML file consults `DwellingTable`; the Python
`SimulationConfig(...)` constructor consults `SimulationConfig`, then
`PythonKeyword`; the Python `DwellingConfig`, `Dwelling.from_hpxml` and
`DwellingBlueprint.from_hpxml` consult `DwellingTable`, `SimulationConfig`,
then `PythonKeyword`; `Fleet.from_resstock` and the PV-sizing functions
consult `PythonKeyword`; the equipment override merge, the synthetic TOML
equipment tables and the Python equipment constructors `Battery`, `PV`
and `EV` (and the Python `BmsMode` and `ChargingStrategy` constructors)
consult `EquipmentConfig`; `ActorRegistry::create` consults
`ActorParameter`; the Python control-signal constructors, the HELICS
signal reader and the gym's action layout consult `ControlSignal`;
`to_observation_vec` and the gym's observation keys consult
`Observation`. `ledger/12-inputs.md` IN-6 creates the types, the table
and the lookup with its first row, with the first four scopes;
`ledger/09-numeric-fixes.md` B-14 adds `Observation` in its run-state
commit and `ActorParameter` and `ControlSignal` in its control-layer
commit, each with its first rows and the surface that consults it. Every
later entry that removes or
renames a key adds its row in its own commit. A replacement names the
field by its path on the config type
(`SimulationConfig.site_location.latitude_deg`), which reads the same on
every surface.

Keys in the scopes that hold per-type keys are qualified by the type that
reads them, so a key shared by several types resolves to its own row:
- `EquipmentConfig`: `<ConfigType>.<key path>`. `<ConfigType>` is the
  config type, not the class name an override is keyed by: the override
  merge maps the equipment's class (`"PV"`, `"ASHP Cooler"`) to its config
  type through the registry entry that builds the class (the mapping
  `ledger/12-inputs.md` IN-3's `OverrideClassSchema` lists); raw equipment
  keys use the reading type's name (`EventBasedLoad`, `ScheduledLoad`).
  `<key path>` is the dotted path inside the payload, an array element is
  written `[]` (`PvConfig.arrays[].capacity_kw`), and a JSON-string payload
  is parsed and pathed under its variant
  (`BatteryConfig.bms_mode.SelfConsumption.surplus_deadband_kw`). Every
  surface of this scope, the Python constructors included, forms that key
  before the lookup.
- `ActorParameter`: `<actor_type>.<parameter>` (`EvDriver.capacity_kwh`).
- `ControlSignal`: `<Variant>.<field>` (`PowerSetpoint.active_power_kw`).
- A pattern row writes `<n>` where a key carries a number
  (`EventBasedLoad.phase_<n>_power_kw`); `removed_key` matches `<n>`
  against a run of ASCII digits and every other character exactly.
- `Observation`: the observation name itself (`total_power_kw`).

The rows, each with the entry that adds it:

| Removed key | Scope | Error names | Row added by |
|---|---|---|---|
| `resample_overrides` (a `DwellingConfig` field and Python keyword) | `DwellingTable` | none: the engine uses the EnergyPlus methods; OCHRE comparison mappings live in the Python adapter (`ledger/12-inputs.md` IN-6) | IN-6 |
| `duration_s`, `time_res_s` (synthetic TOML `[simulation]` keys; Python keywords) | `SimulationConfig` | `SimulationConfig.duration`, `SimulationConfig.time_res` (seconds) | PY-8 |
| `latitude`, `longitude`, `elevation_m`, `utc_offset_h` (Python keywords) | `PythonKeyword` | `SimulationConfig.site_location.latitude_deg`, `.longitude_deg`, `.elevation_m`, `.utc_offset_h` | PY-8 |
| `output_to_parquet` (Python keyword) | `PythonKeyword` | `SimulationConfig.output_format` until OR-3, which rewrites the row to name `SimulationConfig.reports` | PY-8 |
| `defaults_path` (Python keyword, `DwellingConfig` argument) | `PythonKeyword` | `defaults` | PY-2 |
| `output` (the synthetic TOML `[output]` table) | `DwellingTable` | `SimulationConfig.reports` | PY-8 |
| `initialization_duration_s` (a synthetic TOML `[simulation]` key) | `SimulationConfig` | the top-level `initialization_duration_s` (from B-2: `SimulationConfig.warmup`) | PY-8 |
| `write_output`, `retain_batches`, `output_format`, `output_path` (and the `output_to_parquet` row rewritten) | `SimulationConfig` | `SimulationConfig.reports` (a `ReportSpec` with a `Memory`, `Parquet` or `Csv` destination) | OR-3 |
| `rotation` | `SimulationConfig` | `SimulationConfig.reports` with `frequency` (file rotation is removed) | OR-3 |
| `output_verbosity` | `SimulationConfig` | `SimulationConfig.reports` with `selection = { verbosity = N }` | OR-3 |
| `initialization_duration` (Python keyword, `DwellingConfig` field) | `DwellingTable` | `SimulationConfig.warmup` | `ledger/09-numeric-fixes.md` B-2 |
| `initialization_duration_s` (the synthetic TOML top-level key) | `DwellingTable` | `SimulationConfig.warmup` | `ledger/09-numeric-fixes.md` B-2 |
| `hpxml_dir` (Python keyword of `Fleet.from_resstock`) | `PythonKeyword` | `buildings_dir` | `ledger/09-numeric-fixes.md` B-9a |
| `<ConfigType>.<key path>` for every equipment config field and raw equipment key stored in kW, kWh, kVA or kWh/mi (override keys, synthetic TOML keys and the Python equipment constructors' keywords), for example `BatteryConfig.capacity_kwh` | `EquipmentConfig` | `<ConfigType>.<SI key path>` with its unit, for example `BatteryConfig.capacity_j (J)` | `ledger/09-numeric-fixes.md` B-14, commit 2 |
| `ScheduledLoad.gas_profile_max`, `ScheduledLoad.gas_constant` (therms per hour) | `EquipmentConfig` | `ScheduledLoad.gas_profile_max_w (W)`, `ScheduledLoad.gas_constant_w (W)` | B-14, commit 3 |
| `target_kw`, `min_kw`, `max_kw`, `inverter_kw_ac`, `target_ac_kw` (PV-sizing keywords) | `PythonKeyword` | `target_w`, `min_w`, `max_w`, `inverter_ac_w`, `target_ac_w` (W) | B-14, commit 3 |
| `total_power_kw`, `reactive_power_kvar`, `island_unserved_kw`, `island_excess_kw` and every other renamed telemetry field | `Observation` | the SI name (`total_power_w`) | B-14, commit 3 |
| `<actor_type>.<parameter>` for every actor parameter in kW, kWh, miles or mph, for example `EvDriver.capacity_kwh` | `ActorParameter` | `<actor_type>.<SI parameter>` with its unit, for example `EvDriver.capacity_j (J)` | B-14, commit 4 |
| `<Variant>.<field>` for every control-signal field in kW, kVAR or kWh, for example `PowerSetpoint.active_power_kw` (Python keyword and HELICS signal body key) | `ControlSignal` | `<Variant>.<SI field>` with its unit, for example `PowerSetpoint.active_power_w (W)` | B-14, commit 4 |
| `estimate_pv_capacity`'s kW keyword arguments | `PythonKeyword` | its SI keyword arguments (W) | B-14, commit 3 |
| `TariffEvaluator.step`'s `net_power_kw` keyword | `PythonKeyword` | `net_power_w` | B-14, commit 3 |
| `EvConfig.max_charging_kw` | `EquipmentConfig` | `EvConfig.max_charging_power_w (W)` | B-14, commit 2 |
| `ControlSignal.from_dict`, `DispatchRequest`, `Signal` kW/kVAR/kWh fields; the Rust gym action reader's kW fields; the protocol bridge's kW/kVAR fields | `ControlSignal` | the SI field with its unit | B-14, commit 4 |

A Python keyword that names a current `SimulationConfig` field
(`master_seed=`) next to `config=` is not a removed key: it is a
`TypeError` naming `config=SimulationConfig(master_seed=...)`.

Every parameter `09-numeric-fixes.md` B-14 renames (a `*_kw`, `*_kwh` or
`*_kvar` keyword becoming its SI name) gains `#[pyo3(signature)]` with a
`*` before it, so it is keyword-only: under PyO3 0.29 a parameter with no
signature, and one before `*`, is positional-or-keyword, which would let
a positional call still pass the old value under the new name silently.

There is no `#[serde(flatten)]` swallowing unknown keys (as
`HeatPumpConfig` does today,
`crates/hares-equipment/src/hvac/heat_pump_config.rs:262,297,483`), no
silent fallback (as `time_res_secs_u32`'s `unwrap_or(3600)` does today,
`crates/hares-io/src/config.rs:139-141`), and no permissive mode.
`RotationPolicy` (`config.rs:36-48`) and `OutputFormat` are deleted.

`SimulationConfig` after I-08 holds: `start_time`, `duration`, `time_res`,
`output_chunk_size`, `setpoint_deadband_c`, `master_seed`,
`civil_timezone`, `site_location`, `reports: Vec<ReportSpec>`,
`metrics: bool` (default `false`, §2.2), `health_policy: HealthPolicy`,
`warmup: Warmup`, `first_hour_interpolation: FirstHour` and
`weather_year: WeatherYear`. The last three arrive with
`ledger/09-numeric-fixes.md` B-2, B-1a and B-1c:

```rust
// crate: hares-io (crates/hares-io/src/config.rs)
pub enum Warmup {                          // hand-written Serialize/Deserialize, below; Python: warmup=None disables
    Disabled,
    Converge(WarmupConfig),
}
impl Default for Warmup { /* Converge(WarmupConfig::default()) */ }
#[serde(default, deny_unknown_fields)]    // a partial [warmup] table is legal
pub struct WarmupConfig {                  // validate(): tolerances in (0, 0.5], days >= 1, min_days <= max_days
    pub temperature_tolerance_c: f64,      // default 0.4
    pub loads_tolerance: f64,              // default 0.04
    pub min_days: u32,                     // default 1
    pub max_days: u32,                     // default 25
}
#[serde(rename_all = "snake_case")]
pub enum FirstHour { Hour24, Hour1 }       // default Hour24 (EnergyPlus RunPeriod default)
#[serde(rename_all = "snake_case")]
pub enum WeatherYear { Typical, Actual }   // default Typical (EnergyPlus "Treat Weather as Actual" = No)

// crate: hares-io (crates/hares-io/src/weather.rs), replaces WeatherMeta.midpoint_offset_secs
pub enum RecordLabel { IntervalEnd, IntervalStart }   // EPW, TMY3, ResStock CSV: IntervalEnd; PSM3: IntervalStart
```

`Warmup` has a hand-written `Serialize` and `Deserialize`, because neither
derived form gives the TOML it needs (externally tagged writes
`[warmup.converge]`; untagged cannot read the string `"disabled"` for a
unit variant and loses `deny_unknown_fields`' field-naming errors). The
deserializer takes either a string, which must be `"disabled"` (any other
string is an error naming `"disabled"` and the table form), or a table,
which deserialises as `WarmupConfig` (so an unknown key is named);
`warmup` absent is `Warmup::default()`. The serializer writes the same
two forms. Test (B-2): `warmup_toml_forms_round_trip`.

The weather rules these select (EnergyPlus point and solar interpolation,
sub-hourly files, leap days, typical and actual years) are stated with
their EnergyPlus citations in `ledger/09-numeric-fixes.md` ("Weather: what
EnergyPlus specifies"), which owns them. The one departure is day types
after a skipped Feb 29: they follow the calendar, not EnergyPlus's shifted
weekdays (`dl-i08-architecture.leap_day_day_types`, B-1c).

**Inputs.** HPXML, schedule, weather and defaults can be supplied as
content, with parsed data shared immutably across dwellings: across every
dwelling built from one blueprint, and across blueprints, since a blueprint
can be built from handles already parsed for another (the calibration
system builds one blueprint per candidate HPXML over one parsed weather,
schedule and defaults). Plus read-back of the resolved parameters the
physics uses. No edit language now; the shared parsed inputs are the seam
for one later.

```rust
// crate: hares-io (crates/hares-io/src/input.rs, new)
pub enum InputSource { Path(PathBuf), Text(Arc<str>), Bytes(Arc<[u8]>) }
pub struct WeatherInput {
    pub source: InputSource,
    pub format: Option<WeatherFormat>,     // required for Text/Bytes; None for a Path keeps today's detection
}
pub struct DefaultsSource {
    pub base: PathBuf,                     // Python: the packaged defaults when `defaults` is omitted
    pub files: BTreeMap<String, InputSource>,  // path relative to base -> replacement content
}
pub struct InputRecord { pub source: InputSource, pub sha256: [u8; 32] }
pub struct InputRecords {
    pub hpxml: InputRecord,
    pub schedule: Option<InputRecord>,     // None when the schedule is generated from the HPXML (11 OD-6)
    pub weather: InputRecord,
    pub weather_format: WeatherFormat,     // the format the weather was parsed as (detected for a Path), never
                                           // absent, so replayed Text or Bytes content keeps its format
    pub defaults_files_sha256: [u8; 32],   // over every defaults file read, in load order: relative path, 0x00, bytes
    pub defaults_replacements: Vec<(String, InputRecord)>,
}

// crate: hares-io; immutable parsed-input handles, each shared as Arc (06 RT-4)
pub struct ParsedHpxml;     // from InputSource
pub struct ParsedWeather;   // from WeatherInput; caches resampled series and forcing tables
pub struct ParsedSchedule;  // from InputSource
pub struct ParsedDefaults;  // from DefaultsSource; with_file(relative_path, InputSource) replaces one file
impl ParsedHpxml    { pub fn record(&self) -> &InputRecord; }  // likewise for the other three

// crate: hares-core
pub struct ParsedInputs {
    pub hpxml: Arc<ParsedHpxml>,
    pub schedule: Option<Arc<ParsedSchedule>>,  // None: generated from the HPXML (11 OD-6)
    pub weather: Arc<ParsedWeather>,
    pub defaults: Arc<ParsedDefaults>,
}
impl ParsedInputs { pub fn parse(config: &DwellingConfig) -> Result<Self>; }
pub struct DwellingSettings {               // DwellingConfig less its four input sources
    pub sim_config: SimulationConfig,
    pub bldg_id: i64,
    pub overrides: Option<serde_json::Value>,
    pub patches: Option<HpxmlDataPatches>,
    pub initialization_duration: Option<StdDuration>,  // until 09 B-2 moves it to SimulationConfig.warmup
}

// crate: hares-io (crates/hares-io/src/forcing.rs, new; the forcing cache of 06 RT-2)
// ForcingKey and ForcingTable exist only if RT-2's measurement keeps the
// surface-independent table (the entry's keep-or-drop rule); if it drops
// the table, neither type is added and update_in_place stays as it is.
pub struct ForcingKey;      // time grid, naive local start, site UTC offset in seconds, horizon in steps,
                            // site lat/lon/elevation bits, civil timezone (06 RT-2); FirstHour and
                            // WeatherYear join it with 09 B-1a and B-1c. Plain values only, never a
                            // DateTime<FixedOffset>, whose Eq and Hash compare the UTC instant alone
pub struct ForcingTable;    // per-step surface-independent forcing; immutable, shared as Arc;
                            // hares-core fills it with the functions the step uses

// crate: hares-core
pub struct DwellingBlueprintBuilder;        // the only mutable stage; today's blueprint mutators move here
impl DwellingBlueprintBuilder {
    pub fn new(inputs: ParsedInputs, settings: DwellingSettings) -> Result<Self>;  // no parse: shares the handles
    pub fn from_config(config: &DwellingConfig) -> Result<Self>;  // ParsedInputs::parse, then new
    pub fn equipment_names(&self) -> Vec<&str>;                   // as today, blueprint.rs:185
    pub fn add_equipment_spec(&mut self, spec: EquipmentSpec) -> Result<()>;            // blueprint.rs:220
    pub fn remove_equipment(&mut self, name: &str) -> Result<()>;                       // blueprint.rs:194
    pub fn remove_equipment_by_end_use(&mut self, end_uses: &[EndUse]) -> usize;        // blueprint.rs:208
    pub fn freeze(self) -> Arc<DwellingBlueprint>;                // infallible: new already resolved and checked
}
impl DwellingBlueprint {
    pub fn from_config(config: &DwellingConfig) -> Result<Arc<DwellingBlueprint>>;  // Builder::from_config, then freeze
    pub fn build(&self) -> Result<Dwelling>;
    pub fn build_with_seed(&self, master_seed: u64) -> Result<Dwelling>;
    pub fn validate(&self) -> Result<ResolvedModel, HaresError>;
    pub fn catalogue(&self) -> Result<Arc<Catalogue>, HaresError>;  // assembly without warm-up, as validate()
}
impl Dwelling {
    pub fn catalogue(&self) -> Arc<Catalogue>;  // 03 OR-1; the catalogue the row plan freezes from
}
```

`DwellingConfig` (`hares-core`) holds `hpxml: InputSource`,
`schedule: Option<InputSource>` (`None` asks for a schedule generated from
the HPXML, `ledger/11-other-defects.md` OD-6), `weather: WeatherInput` and
`defaults: DefaultsSource` in place of the four path fields
(`mod.rs:210-215`), plus `settings: DwellingSettings`. A
`DwellingBlueprint` holds the four handles as `Arc`s and the resolved
`SiteLocation`; `ForcingTable`s (`ledger/06-runtime-memory.md` RT-2) are
cached on the `ParsedWeather` they came from, so two blueprints built over
one `Arc<ParsedWeather>` on one time grid and site share one table.
`DefaultsSource.base` is shown in its final form: RT-4 introduces the type
with `base: Option<PathBuf>`, keeping today's optional
`defaults_path`, and `ledger/09-numeric-fixes.md` B-10 makes it required.

Python mirrors the handles: `hares.ParsedHpxml`, `hares.ParsedWeather`,
`hares.ParsedSchedule` (each with `from_path(path)` and
`from_text(text)`; `ParsedWeather` also takes `format=`) and
`hares.ParsedDefaults(base, files=None)`, frozen pyclasses over the `Arc`.
`Dwelling.from_hpxml` and `DwellingBlueprint.from_hpxml` accept a parsed
handle anywhere they accept a path or content, and a handle is shared, not
re-parsed.

The handle types, `ForcingKey`, `ForcingTable`, `DwellingBlueprintBuilder`,
`ObservationPlan` and `ActorTimings` (§2.9) are opaque: their fields are private and their
public API is exactly the methods listed here and in the owning entry.

`Dwelling::from_config(config)` is
`DwellingBlueprint::from_config(config)?.build()`. A caller that changes
equipment before building takes the builder,
`DwellingBlueprintBuilder::from_config(config)?` (or `new` over shared
handles), calls its mutators, then `.freeze().build()`. Today's public
blueprint fields (`config`, `schedule`, `defaults_path`, `equipment_specs`,
`blueprint.rs:19-28`) become private; a test that reorders specs does so
through `remove_equipment` and `add_equipment_spec`, which appends.

Owners: `ledger/06-runtime-memory.md` RT-4 builds the content inputs, the
handles, `ParsedInputs`, `DwellingSettings`, the builder (its constructors,
mutators and `freeze`), the Python handle classes and
`build(&self)`/`build_with_seed`; RT-2 caches the forcing table on the
handle; RT-6 computes the `ResolvedModel` below; `ledger/12-inputs.md`
IN-1 adds the `InputRecord` digests and `weather_format` that
`Provenance.inputs` and replay export use, and IN-2 separates assembly
from warm-up and adds `validate()` and `DwellingBlueprint::catalogue()`
on that assembly; `ledger/03-output-reporting.md` OR-1 adds
`Dwelling::catalogue()`. No other entry restates these shapes.

```rust
// crate: hares-core (crates/hares-core/src/resolved.rs, new); SI units throughout
pub struct ResolvedModel {
    pub equipment: Vec<ResolvedEquipment>,
    pub boundaries: Vec<ResolvedBoundary>,
    pub windows: Vec<ResolvedWindow>,
    pub infiltration: ResolvedInfiltration,
    pub site: SiteLocation,                // hares-io, site_location.rs:159
    pub clock: ResolvedClock,              // 12 IN-2
}
pub struct ResolvedClock {
    pub start: NaiveDateTime,              // local standard time of the first step
    pub utc_offset_s: i32,
    pub time_res_s: u32,
    pub steps: u64,                        // simulated steps: duration / time_res until 09 B-1c,
}                                          // which subtracts the steps of a skipped Feb 29
pub struct ResolvedEquipment {
    pub name: String, pub id: EquipmentId, pub end_use: EndUse,
    pub parameters: Vec<ResolvedParameter>,
    pub heating_setpoint: Option<SetpointSource>,  // copied from EquipmentSpec (below)
    pub cooling_setpoint: Option<SetpointSource>,
}

// crate: hares-io (crates/hares-io/src/setpoint_source.rs, new; 12 IN-5): resolved in
// hares-io (schedule_resolve.rs:655-693), which cannot see hares-core
pub enum SetpointSource {
    ScheduleColumn { column: String },     // a schedule read from a file (DwellingConfig.schedule is Some)
    Hpxml { weekday_c: Vec<f64>, weekend_c: Vec<f64> },   // 24 hourly values each
    DefaultProfile { profile: String },
    GeneratedConstant { value_c: f64 },    // a generated schedule's constant column (12 IN-5); 11 OD-9 deletes it
}
// two new fields of EquipmentSpec (crates/hares-io/src/hpxml/equipment.rs:22), set by the
// schedule resolution for every heating or cooling equipment, None for every other spec
pub heating_setpoint_source: Option<SetpointSource>,
pub cooling_setpoint_source: Option<SetpointSource>,

// crate: hares-types (crates/hares-types/src/resolved.rs, new): the Equipment trait
// (hares-equipment) returns it, so it lives below hares-equipment
pub struct ResolvedParameter { pub name: &'static str, pub value: f64, pub unit: Unit }

// crate: hares-core (crates/hares-core/src/resolved.rs), as ResolvedModel above
pub struct ResolvedBoundary {
    pub id: String, pub kind: BoundaryType, pub area_m2: f64,  // BoundaryType: hares-io, hpxml/building.rs:60
    pub r_effective_m2k_w: f64,            // assembly R, films included
    pub films_m2k_w: f64,                  // interior plus exterior film resistance
    pub c_j_k: f64,
    pub layers: Vec<ResolvedLayer>,        // outside to inside; empty for slab (F-factor) boundaries
}
pub struct ResolvedLayer { pub r_m2k_w: f64, pub c_j_m2k: f64 }
pub struct ResolvedWindow { pub id: String, pub area_m2: f64, pub u_w_m2k: f64, pub shgc: f64 }
pub struct ResolvedInfiltration {
    pub ach50: Option<f64>, pub ela_m2: Option<f64>, pub constant_ach: Option<f64>,
}
impl Dwelling { pub fn resolved_model(&self) -> &ResolvedModel; }
```

The `ResolvedModel` is computed at assembly, after defaults, autosizing and
overrides and before warm-up. `Dwelling::resolved_model()`
(`ledger/06-runtime-memory.md` RT-6) and `DwellingBlueprint::validate()`
(`ledger/12-inputs.md` IN-2) return the same value. `validate()` runs
every construction-time check through the same code `build()` runs,
without warm-up and without stepping; its errors are the values `build()`
returns. Each equipment reports its parameters through a new `Equipment`
trait method `resolved_parameters(&self) -> Vec<ResolvedParameter>` (no
default implementation). Required names by end use (a class may add
more):

| End use | Required parameters |
|---|---|
| HVAC cooling | `rated_capacity_w`, `rated_eir`, `rated_shr` |
| HVAC heating, heat pump | `rated_capacity_w`, `rated_eir` |
| HVAC heating, fuel | `rated_capacity_w`, `afue` |
| HVAC heating, electric resistance | `rated_capacity_w`, `efficiency` |
| Water heating | `heating_capacity_w`, `setpoint_c`; tanks add `tank_volume_m3`, `ua_w_k`; heat pump water heaters add `rated_cop` |
| PV | `nameplate_w` |
| Battery | `energy_capacity_j`, `power_capacity_w`, `round_trip_efficiency` |
| EV | `battery_capacity_j`, `max_charging_power_w` |
| Other loads and lighting | `usage_multiplier` |

**Other built-now capabilities** (`dl-i08-architecture.new_capability`):

- machine-readable input schema: `hares_core::input_schema() -> InputSchema`
  (serde, JSON), `ledger/12-inputs.md` IN-3. The types live in `hares-io`;
  the assembly lives in `hares-core` (`crates/hares-core/src/input_schema.rs`,
  new), the lowest crate that sees every described type, `DwellingConfig`
  and `DwellingSettings` included:

  ```rust
  // crate: hares-io (crates/hares-io/src/input_schema.rs, new)
  pub struct InputSchema {
      pub schema_version: u32,
      pub simulation_config: Vec<FieldSchema>,
      pub dwelling_config: Vec<FieldSchema>,
      pub overrides: Vec<OverrideClassSchema>,  // one per equipment class, plus reserved keys ("zip", "envelope")
      pub schedule_columns: Vec<FieldSchema>,   // the columns the engine reads
      pub unused_schedule_columns: Vec<UnusedScheduleColumnSchema>,  // the NotUsed entries (03 OR-11): accepted,
                                                // deliberately not read, each with its reason
      pub defaults_files: Vec<String>,          // relative paths the defaults loader reads
      pub hpxml_elements: Vec<HpxmlElementSchema>,  // every element the parser reads, from HPXML_ELEMENTS
      pub rejected_hpxml_elements: Vec<RejectedHpxmlElementSchema>,  // from HPXML_REJECTED_ELEMENTS
      pub removed_keys: Vec<RemovedKeySchema>,  // REMOVED_KEYS, in table order
  }
  pub struct RemovedKeySchema { pub scope: KeyScope, pub key: String, pub replacement: String }
  pub struct HpxmlElementSchema {
      pub name: String,
      pub context: String,                      // the parent path the parser reads it under
      pub unit: Option<Unit>,
      pub description: String,
  }
  pub struct RejectedHpxmlElementSchema { pub name: String, pub context: String, pub reason: String }
  pub struct UnusedScheduleColumnSchema { pub column: String, pub reason: String }
  // crates/hares-io/src/hpxml/elements.rs (new): the parser's declarations
  pub struct HpxmlElement {
      pub name: &'static str,
      pub context: &'static str,
      pub unit: Option<Unit>,
      pub description: &'static str,
  }
  pub static HPXML_ELEMENTS: &[&HpxmlElement];
  pub struct RejectedHpxmlElement {        // an element whose presence fails the parse
      pub name: &'static str,
      pub context: &'static str,
      pub reason: &'static str,             // the error text: why, and the accepted form or element
  }
  pub static HPXML_REJECTED_ELEMENTS: &[&RejectedHpxmlElement];
  pub struct OverrideClassSchema { pub class: String, pub fields: Vec<FieldSchema> }

  // crate: hares-types (crates/hares-types/src/schema.rs, new): implemented next to
  // every config definition, including the equipment configs in hares-equipment
  // (HeatPumpHeaterConfig, heat_pump_config.rs:262; GasFurnaceConfig, heating_config.rs),
  // which cannot see hares-io; hares-types owns Unit, and IN-3 moves serde_json from its
  // [dev-dependencies] to [dependencies] for FieldSchema.default
  pub struct FieldSchema {
      pub name: String,
      pub value_type: ValueType,
      pub unit: Option<Unit>,
      pub default: Option<serde_json::Value>,
      pub bounds: Option<(f64, f64)>,
      pub description: String,
  }
  pub enum ValueType { Bool, Integer, Float, String, Timestamp, DurationSeconds, Table, Array }
  pub trait DescribeFields { fn fields() -> Vec<FieldSchema>; }

  // crate: hares-io (crates/hares-io/src/envelope_scales.rs, new; 12 IN-7): the reserved
  // `envelope` override key; IN-7 enforces the bounds in its own validation, and IN-3, which
  // creates DescribeFields, adds `impl DescribeFields for EnvelopeScales`
  #[serde(default, deny_unknown_fields)]
  pub struct EnvelopeScales {
      pub opaque_r_scale: f64,              // default 1.0, bounds (0, 10]: every opaque LUT or material layer R
      pub thermal_mass_scale: f64,          // default 1.0, bounds (0, 10]: every such layer's capacitance
  }
  impl EnvelopeScales { pub fn validate(&self) -> Result<(), HaresError>; }  // names the field and its bounds
  ```

- `hares_core::resample_weather(weather: &WeatherInput, site: &SiteLocation,
  config: &SimulationConfig) -> Result<RecordBatch, HaresError>`, the
  engine's own per-step weather channels, `ledger/12-inputs.md` IN-4;
- `metrics_from_table` (§2.2), `ledger/03-output-reporting.md` OR-15;
  its Python binding needs the Python `Catalogue` and the Arrow stream
  import, so `ledger/05-python-surface.md` PY-4 adds it;
- replay export, `ledger/03-output-reporting.md` OR-16, which also adds
  the Python binding (`RunResult.export_replay`, `hares.replay`):

  ```rust
  // crate: hares-core (crates/hares-core/src/replay.rs, new)
  impl RunResult { pub fn export_replay(&self, dir: &Path) -> Result<()>; }  // reads only provenance and reports;
                    // HaresError::NotReplayable { blockers } when Provenance.replay_blockers is not empty (§2.1)
  pub struct Replay {
      pub config: DwellingConfig,
      pub tariff: Option<TariffRecord>,      // §2.1; set on the built dwelling before its first step
      pub recorded_build: RecordedBuild,     // the build that produced the exported run
  }
  pub struct RecordedBuild {                 // BuildInfo as read back from replay.toml (owned strings)
      pub version: String, pub commit: Option<String>, pub dirty: Option<bool>,
      pub profile: String, pub features: Vec<String>, pub rustc: String,
  }
  pub fn load(dir: &Path, defaults_base: &Path) -> Result<Replay>;
  impl Replay { pub fn build(&self) -> Result<Dwelling>; }  // builds the config, then set_tariff when Some
  ```

Each built-now capability that has a Python surface adds it in the
entry's own commit, with its `_hares.pyi` stubs and its entry in the
`docs/python.md` changes section (`ledger/05-python-surface.md` PY-9).

**Package.** Python distribution `hares-py` (the name `hares` is taken on
PyPI by an unrelated project); import name `hares`; extension module
`hares._hares`; cache directory `~/.cache/hares`. No alias for
`ochre_next`. `ledger/05-python-surface.md` PY-7 implements this.

**Dependencies** (`dl-i08-architecture.dependencies`). Every Rust
dependency moves to its latest release (pyo3 and numpy 0.29, arrow and
parquet 59, the latest arrow that pyo3-arrow 0.19 accepts, nalgebra 0.35,
quick-xml 0.42 and the rest of the table in
`ledger/01-baseline-repair.md` BR-6b), the workspace `rust-version` rises
to 1.89 (BR-6a), both repositories' Python lockfiles and maturin move to
their latest releases (BR-6c, BR-6d), and the verification block's
`[advisories]` line runs cargo-deny's advisory check with no ignores and no
license policy. pyo3 0.29 carries the fixes for RUSTSEC-2026-0176 and
RUSTSEC-2026-0177.

Bitwise identical output per platform across processes, thread counts and
runs is tested (`ledger/06-runtime-memory.md` RT-5, `ledger/04-fleet.md`
FL-1). Cross-platform agreement is measured on a GitHub Actions macOS arm64
runner against Linux x86_64 and published in `docs/reproducibility.md`
(`ledger/02-measurement.md` M-4a, M-4b).

### 2.8 Numeric fixes (Part B)

Every confirmed defect that moves numbers is fixed, each in its own commit
with golden re-capture and a recorded delta; no numeric change is folded
into a structural commit (structural entries that touch a numeric path are
bug-compatible and bitwise-neutral, and the numeric half follows as its own
commit). The decision log names exactly these;
`ledger/09-numeric-fixes.md` has one entry per item:

1. weather-record alignment (B-1a point values and sub-hourly files, B-1b
   solar values, B-1c typical and actual years, wrap and Feb 29)
2. EnergyPlus-grade warm-up convergence, including NaN
3. Python warm-up default equal to Rust
4. billing month boundary
5. end-use rollup by typed kind
6. null for not-applicable values
7. metrics independent of verbosity
8. deterministic fluid-solver summation
9. ResStock weather year
10. missing defaults directory is an error (and Python defaults to the
    defaults shipped with the package, `dl-i08-architecture.defaults_path`)
11. Mode aggregation
12. fleet local time labels

Plus, filed separately under precedence: HPXML `AssemblyEffectiveRValue`
wins over precomputed RC, as its own declared numeric change
(`decision-log.md` → `dl-i08-architecture.precedence`), B-13: the matched
LUT row's layer resistances are scaled uniformly so layers plus films equal
the declared assembly R, and the row's layer capacitances are kept. A
caller that scales a construction's R (the calibration system) does so
through the override map's reserved `envelope` key (`opaque_r_scale`,
`thermal_mass_scale`, `ledger/12-inputs.md` IN-7), applied after row
selection, so the selected row and its capacitances never move with the R
scale (`dl-i08-architecture.calibration_r_factor`).

Found defects that move numbers follow the same rules (own commit, own
row in the numeric block, recorded delta): `ledger/09-numeric-fixes.md`
B-14 (everything HARES stores and computes in SI units,
`dl-i08-architecture.si_units`: equipment config, then the run state §2.1
names, then the control layer, three numeric commits in three rows), and,
living with their area, `ledger/04-fleet.md` FL-5 (fleet variables fold
by their declared kind), `ledger/11-other-defects.md` OD-9 (a generated
schedule keeps the HPXML setpoint profile), OD-2's site 6 (a constant
setpoint override replaces the schedule and HPXML setpoint sources),
OD-10's numeric commit (infiltration height, flue and shielding defaults
by the OpenStudio-HPXML rules) and OD-20 (PV roof coverage registered
from the typed PV config), `ledger/12-inputs.md` IN-8 (schedule-file
setpoint columns in degrees Fahrenheit) and IN-9 (HPXML vehicles and
their chargers are simulated), and `ledger/09-numeric-fixes.md` B-15
(OpenStudio-HPXML unavailable periods applied,
`dl-i08-architecture.unavailable_periods`), B-16 (the wet-appliance
triage, recorded, no commit of its own) and B-17, B-16's contingent
numeric commit (wet-appliance hot water by the OpenStudio-HPXML daily
volume, `dl-i08-architecture.wet_appliance_hot_water`). Day types after a
skipped Feb 29 follow the calendar
(`dl-i08-architecture.leap_day_day_types`), inside B-1c.

Users learn of numeric changes from the release documentation, not the
measurement log, which does not ship: every commit above that moves a
number adds one line to the "Numeric changes" list of `docs/python.md`
(created empty by `ledger/05-python-surface.md` PY-9) in that commit,
naming what moved, why, and the golden fixtures it re-captured (or that
none was). Each entry's Verification names its line.

Every entry that removes a silent path (`ledger/11-other-defects.md`) is
verified bitwise against the goldens; if a golden changes, the entry stops
and the difference is explained before it commits.

### 2.9 Engine performance

In scope, profile-driven: telemetry storage without per-step allocation,
exogenous forcing reuse shared through the parsed weather, envelope
solar/incidence reuse, thermal input preparation allocations, thread-safe
concurrent dwellings, and the per-step RL observation
(`dl-i08-architecture.rl_observation_allocation`). The coupled
factorization is not reused: it is unreachable (the implicit matrix is
always the identity) and is removed as dead code
(`ledger/10-slop-cleanup.md` SL-7).

```rust
// crate: hares-core (crates/hares-core/src/observation.rs, new; 06 RT-7)
pub struct ObservationPlan;                // field list resolved once to typed sources; unknown field is an error at build
impl Dwelling {
    pub fn observe_into(&self, plan: &ObservationPlan, out: &mut [f64]) -> Result<()>;  // no allocation
}

// crate: hares-core (profiling feature; 07 PR-1, PR-2, PR-3, PR-4, PR-6)
pub struct ActorTiming { pub name: Arc<str>, pub total: StdDuration, pub calls: u64 }
pub struct ActorTimings;                   // one ActorTiming per actor slot (07 PR-6)
impl ActorTimings {
    pub fn new(names: impl IntoIterator<Item = Arc<str>>) -> Self;  // one zeroed slot per name, in order
    pub fn record(&mut self, slot: usize, elapsed: StdDuration, called: bool);  // in place, no allocation
    pub fn timings(&self) -> &[ActorTiming];
}
// DwellingProfilingSummary (existing, dwelling/mod.rs:258-270) holds exactly: environment, control,
// ideal_capacity, actors, dispatch, equipment, envelope, invariants, state_snapshot, output,
// accounting, step_total (StdDuration each; the eleven phases sum to step_total, 07 PR-1);
// finish: Option<StdDuration> (None until finish() has run, 07 PR-4);
// memory_high_water_kb: Option<u64>; hot_path_allocations: Option<u64>;
// hot_path_alloc_violations: Option<u64>; per_actor: Vec<ActorTiming>

// crate: hares-types (crates/hares-types/src/alloc_count.rs, new; 07 PR-3)
pub struct CountingAllocator;              // GlobalAlloc over System, per-thread count
pub fn thread_allocations() -> Option<u64>; // None when no binary installed the allocator
```

The row plan (`RowPlan`, `ledger/03-output-reporting.md` OR-2) and the
period reducers (OR-13) are workspace plumbing: the reducer is a public
item of `hares-io` because `hares-fleet` constructs one per home
(`ledger/04-fleet.md` FL-1), but neither is a stability contract or part
of the documented API. Each change is bitwise
frame-equal before and after; allocation and factorization counts are
tests; performance shares are recorded measurements, never assertions. An
item the profile shows is not worth it is dropped with the measurement
recorded, not silently.

### 2.10 Not built now, and the seam each keeps

Per `dl-i08-architecture.new_capability` and `dl-i08-process.scope`:

- **Batched multi-variant evaluation.** Seam: `DwellingBlueprint` holds
  `Arc`-shared parsed inputs and `build(&self)` builds any number of
  dwellings from one parse (`ledger/06-runtime-memory.md` RT-4).
- **`SteppableFleet` rewind.** Seam: every home's `Dwelling` checkpoint
  resumes its whole run, billing, metrics, report reducers, health and
  warnings included (`ledger/03-output-reporting.md` OR-10), so a fleet
  rewind is the composition of its homes' checkpoints plus the fold state
  of `ledger/04-fleet.md` FL-1. `SteppableFleet` gains no save or restore
  method now, and has none to call: there is no rewind entry point to
  reject. `docs/python.md` states that fleet stepping does not rewind
  (`ledger/05-python-surface.md` PY-9).
- **A built-in reward enum.** Seam: the per-environment
  `reward_fn: Callable[[RewardContext], float]`, honoured identically on
  the Rust and Python paths (`ledger/05-python-surface.md` PY-5).
- **Per-home fleet files partitioned by building id** (requirement FL-11,
  COULD; not in the decision log's built-now list). Seam: each home's
  `RunResult` carries its own reports and `ReportSpec` destinations
  (§2.2), and FL-1 already resolves and de-duplicates per-home file paths,
  so a partitioned layout is a path rule over `bldg_id` on that
  resolution.
- **A command-line interface and a separate minimal Rust example**
  (requirement RS-9, COULD; not in the built-now list, and
  `dl-i08-process.purpose` is runtime, output and performance). No document
  in the tree describes a CLI (`README.md`, `docs/`), so there is nothing to
  correct. Seam: `frame_golden` (`ledger/02-measurement.md` M-1) is the
  maintained Rust driver of the public API, run by the verification block.

Requirement R24 (ResStock sample-weight diagnosis) is built, as the
found defect it is: `ledger/05-python-surface.md` PY-12. Requirement
PY-14 (lifecycle, output and dtype documentation that runs) is built in
PY-9's documentation and OR-7's "Time" section. Requirement R15 (a list of
every added, changed and removed Python callable) is built as the
`docs/python.md` changes section of PY-9.

An error-category taxonomy over `HaresError` is not built: the decision log
does not call for one, and typed errors that survive a process boundary
come from the Python exception classes and their pickling
(`ledger/11-other-defects.md` OD-1). `ledger/08-consumer.md` cites the
needs analysis by requirement ID where an entry exists because of it.

Seven `dl-i08-architecture` sub-decisions carry no citation of their own
in this design frame because each is implemented and cited at the
ledger-entry level instead: `consumer_gate`, `unavailable_periods`,
`wet_appliance_hot_water`, `setpoint_sources`, `battery_control`,
`consumer_inert_override_probe` and `cross_platform_measurement`. See
the ledger entry each decision names for its requirement-by-requirement
mapping.

## 3. Execution rules

**Branches** (`decision-log.md` `dl-i08-process.branch`). HARES work
happens on `feat/output-results` in `~/src/HARES`.
Consumer work happens on `i08-consumer` in `~/src/HARES_CALIBRATION_SYSTEM`,
from `c71c542`. No destructive git operation (`reset`, `revert`, `stash`,
`checkout .`, branch deletion, forced branch recreation) is used anywhere in
this initiative.

**Scope of the thermal solver.** The operator's scope decision
(`dl-i08-process.scope`: every file that needs touching is touched,
including the thermal solver) authorizes editing the thermal solver,
`crates/hares-envelope`, in this initiative, which the standing rule
against editing it otherwise forbids. The committed entries that edited
it are `ledger/03-output-reporting.md` OR-5 (the invariant checks it
un-gated, `dc033cee`), `ledger/01-baseline-repair.md` BR-6a (the let-chain
refactor, `39488581`) and BR-6b (the dependency upgrade, `872cf289`), and
`ledger/07-profiler.md` PR-3 (its two zero-allocation test binaries,
`26626710`). The entries that edit it from here on are
`ledger/03-output-reporting.md` OR-11 (its `crates/hares-envelope` census
rows in `thermal_solver/`, `boundary_rc.rs`, `state_space.rs`,
`fluid_solver.rs`, `humidity_solver.rs` and `electrical_solver.rs`, the
fallible `DomainSolver` resolve signature every domain solver implements,
and the fluid-loop declarations of its stop record),
`ledger/06-runtime-memory.md` RT-1 (the domain readers), RT-3 and RT-5,
`ledger/10-slop-cleanup.md` SL-7 and `ledger/09-numeric-fixes.md` B-8,
B-13 and B-14 (its run-state commit moves the electrical solver,
`electrical_solver.rs`, to W); `ledger/12-inputs.md` IN-7 changes the
envelope parameters it is given. Each is bitwise against the goldens or a
numeric entry under the rules below.

**Task 0.** Before any entry:

1. PR #27 (`fix/data-host-renamed-nlr` into `initial-implementation`: the
   NLR data-host fix, the Python `profiling_summary()` binding and the
   `profiling` feature of `hares-python`, the `profile_sim` example, a
   `cargo fmt` fix) is merged locally into `ebdb1a17` as `1d60f478`
   (`dl-i08-execution.task0-state`).
2. `feat/output-results`, which had no commits of its own, is
   fast-forwarded to `1d60f478`, the Task 0 head. Nothing is pushed: the
   GitHub merge of PR #27 and the matching `initial-implementation` update
   are operator checkpoints of the closing sequence.
3. With `c71c542` checked out in the consumer (its current HEAD),
   `git -C ~/src/HARES_CALIBRATION_SYSTEM switch -c i08-consumer`, which
   leaves the working tree untouched.
4. The stray output files are listed for the operator with
   `git -C ~/src/HARES status --porcelain --ignored --untracked-files=all | grep -E 'dwelling_[0-9]+(_diagnostics)?\.csv'`
   (fourteen at `ebdb1a17`, named in `ledger/01-baseline-repair.md` BR-7);
   after confirmation exactly those paths leave the tree (they were moved to
   `target/stray-quarantine/`, which `[stray]` excludes,
   `dl-i08-execution.task0-state`). The confirmation covers the same paths
   when block runs recreate them before BR-7 lands.
5. `ledger/02-measurement.md` M-0 records the reference state.

**Opening sequence.** Task 0, M-0, CN-0, CN-16, B0 (the consumer baseline,
captured before any engine change that can move numbers,
`ledger/08-consumer.md`), BR-1, BR-3, BR-2, BR-9, OR-5 (BR-5's repair,
`ledger/03-output-reporting.md`), BR-4, BR-7, BR-8, M-1, M-2, BR-6a, BR-6b,
BR-6c, BR-6d, BR-11, M-3, then the profiler's PR-1, PR-2, PR-3, PR-5 and
PR-6 (`ledger/07-profiler.md`), then OR-11 (row 19) and SL-7 (row 20).
Rows 0 to 20 are committed in that order (BR-11 `8b09ac4a`, PR-1
`d7f0e381`, PR-2 `21d90588`, PR-3 `26626710`, PR-5 `1d471677`, PR-6
`afa9a071`, OR-11 `b8195c83`, SL-7 `7b7a6a7d`; `ledger/00-schedule.md`
gives every row's commits). BR-11 precedes M-3 because M-3 measures
`consumer_shape_900s`, which must run without warm-up as the consumer does.

From there, implementation proceeds in the implementer's order (operator
ruling): the independent audits of rows 19 and 20
(`verification/audit-OR-11-b8195c83.md`,
`verification/audit-SL-7-7b7a6a7d.md`) and the fixes of their findings;
then RT-3 (row 21), RT-3a (row 21a) and OR-18 (row 21b), committed; then
OR-19 (row 21c, committed on branch `i08-fixes`, in
review); then rows 16a to 16h, in order:
CN-17 (gated on the latest engine commit when it runs, its contract-drift
census over every engine row up to that commit), BR-20 (row 16a1, the Rust toolchain and tool-version
pin), BR-10, BR-12, BR-13, BR-14, BR-15, the B0 re-check (row 16g) and
OD-15 (row 16h); then row 22 onward. The ledger follows the code: each
commit is recorded in `00-schedule.md`'s State column as it lands and
audited independently. Verification rounds keep running over rows not
yet executed, correcting their entries ahead of the implementer, rather
than gating when a row starts; row 94c's (B-14's first commits) own
verification round over B-14 runs the same way. CN-17 lands before any
further consumer gate.

Every committed row gets an audit by an auditor who did not implement,
review or red-team it: the row's entry checked requirement by requirement
against the diff, the named tests shown to fail without the change, the
verification block and the physics suites re-run in a worktree of the
commit, and the code checked for duplication, hot-path cost and writing
rules. The audit is `verification/audit-<entry>-<sha>.md`. A red team,
separate from the implementer, the reviewer and the auditor, then tries to
break the commit with reproduced cases (wrong physics, substituted values,
missed checks, panics, hot-path cost) in
`verification/redteam-<entry>-<sha>.md`. Findings of both become entries
or follow-up commits that the implementer picks up; a row's State reads
done only when they are fixed and the fix itself has been reviewed.

No committed engine row has a B0 check run through the engine-under-test
mechanism: the log (`ledger/02-measurement.md`) holds a check for rows 1
to 3 only, the recorded commands of every opening row ran the consumer
against an editable install of the main engine tree (`implementation.md`),
as did the gate and B0 checks of BR-6d and BR-11, and PR-1 to PR-6 have
none recorded; the B0 results logged for OR-11 and SL-7 name no
engine-under-test worktree. CN-17's gate on the gate engine (the latest
engine commit when it runs) is that row's check. Row 16g re-checks the
rest on the consumer at CN-17, through the mechanism, in row order:
worktrees of `8b09ac4a`, `d7f0e381`, `21d90588`, `26626710`, `1d471677`,
`afa9a071`, `b8195c83`, `7b7a6a7d`, `44da330c` and every later engine
commit before the gate engine, then the gate engine, whose capture is
CN-17's gate capture. BR-11 changed only `tools/frame_golden`, so the engine at
`8b09ac4a` is row 12's and its capture is the cumulative check of rows 1
to 12. A difference there is bisected over the engine rows' commits
`1d60f478`..`75a182a0` (worktrees, in row order) before any row is named;
a difference first seen at a later commit names that commit's row. After every engine row the B0 check of
`ledger/08-consumer.md` runs, through the engine-under-test mechanism, while
the consumer can still run the engine; no gate or capture runs against an
editable install of `/home/rich/src/HARES`. At the Task 0 head the block
fails on
`[clippy]`, `[test-observe]`, `[test-observe-profiling]`, `[test-release]`,
`[advisories]` and `[stray]`, and `[golden]` has no fixtures. Each opening entry runs every
block line that is green at that point, must leave it green, and names the
lines it turns green. M-1 and M-2 precede the dependency upgrade because the
decision log requires the upgrade's output change to be recorded against the
golden; they run every line except `[advisories]`, which BR-6b turns green.
From BR-6b on, every entry runs the whole block and leaves it green
(`ledger/00-schedule.md` lists which row turns each line green). No entry
narrows the block to make itself pass.

**Verification block.** Run with `bash -euo pipefail` from the repository
root twice per entry: on the staged change before the entry commits, and
again on the commit before the next entry starts. The `[writing]` line
reads the commit, so it runs on the second run only. Labels in brackets
are how entries name lines.

```
mkdir -p target
stray() { git status --porcelain --ignored --untracked-files=all -- . ':(exclude)target' \
  ':(exclude).venv' ':(exclude)*__pycache__*' ':(exclude)*.pytest_cache*' \
  ':(exclude)*.ruff_cache*' ':(exclude)python/*/_hares*.so' \
  ':(exclude).shipwright' ':(exclude).workstrees' ':(exclude).claude' | sort; }
census() { sed -E 's/^(SKIPPED \[[0-9]+\] [^:]+):[0-9]+: /\1: /' | sort; }
{ rustc -V; cargo nextest --version; cargo deny --version; uv --version; } | tee target/tools.txt   # [tools]
stray > target/stray-before.txt                                                   # [stray]
export TMPDIR="$PWD/target/block-tmp"; rm -rf "$TMPDIR"; mkdir -p "$TMPDIR"          # [stray]
cargo fmt --check --all                                                           # [fmt]
cargo clippy --workspace --all-targets --all-features -- -D warnings              # [clippy]
cargo clippy --workspace --all-targets -- -D warnings                             # [clippy-default]
cargo clippy --workspace --all-targets -F dst -- -D warnings                      # [clippy-dst]
cargo clippy --workspace --all-targets -F profiling -- -D warnings                # [clippy-profiling]
cargo clippy --workspace --all-targets -F observe -- -D warnings                  # [clippy-observe]
cargo clippy --workspace --all-targets -F observe,profiling -- -D warnings        # [clippy-observe-profiling]
cargo clippy --workspace --all-targets --release -- -D warnings                   # [clippy-release]
cargo clippy --workspace --all-targets --release -F dst -- -D warnings            # [clippy-release-dst]
cargo clippy --workspace --all-targets --release -F observe -- -D warnings        # [clippy-release-observe]
cargo nextest run --workspace --no-fail-fast                                      # [test-debug]
cargo nextest run --workspace --no-fail-fast -F observe                           # [test-observe]
cargo nextest run --workspace --no-fail-fast -F dst                               # [test-dst]
cargo nextest run --workspace --no-fail-fast -F profiling                         # [test-profiling]
cargo nextest run -p hares-core --no-fail-fast -F observe,profiling               # [test-observe-profiling]
cargo nextest run --workspace --release --no-fail-fast                            # [test-release]
cargo bench --workspace --no-run                                                  # [bench]
cargo deny --all-features check advisories                                        # [advisories]
cargo run --release -p frame_golden -- compare-all                                # [golden]
cargo run --release -p frame_golden -F dst -- compare-all                         # [golden]
uv run --locked --all-extras --all-groups maturin develop --release --uv          # [python-build]
uv run --all-extras --all-groups --no-sync pytest -m "" -rs -p no:cacheprovider | tee target/pytest.log   # [pytest]
{ grep '^SKIPPED' target/pytest.log || true; } | census \
  | diff - <(census < .shipwright/initiatives/I-08/ledger/pytest-skip-census.txt)  # [pytest]
stray > target/stray-after.txt                                                    # [stray]
if comm -13 target/stray-before.txt target/stray-after.txt | grep .; then exit 1; fi   # [stray]
if find "$TMPDIR" -mindepth 1 -path "$TMPDIR/pytest-of-$(id -un)" -prune -o ! -name 'uv-*.lock' -print | grep .; then exit 1; fi   # [stray]
ids='\b(BR|M|PR|OR|FL|PY|RT|CN|B|OD|SL|IN|T|CW|CFG|I|DV|RL|RS|X)-[0-9]+[a-z]?\b|\bR[0-9]{1,2}\b|\bV[0-9]{1,2}\b|\bdl-i08|\b[Rr]ound L?[0-9]|\bL[0-9]-([a-e]\b|resolution|integration)|\bTask [0-9]|\bFinding [0-9]|[Rr]eviewers?\b|\b[Tt]ickets?\b|\b(LLMs?|Claude|[Ss]ubagents?|AI agents?|coding agents?|language models?)\b'
writing() {   # $1: the repository whose HEAD commit is checked
  if git -C "$1" log -1 --format=%B | grep -nP "\x{2014}|$ids"; then return 1; fi
  if git -C "$1" show --format= --unified=0 HEAD -- . ':(exclude)tests/fixtures' ':(exclude)*.lock' \
    | grep -nP "^\+(?!\+\+).*(\x{2014}|$ids|HARES#[0-9]+)"; then return 1; fi
}
writing .                                                                         # [writing]
```

- `[pytest]`: `-m ""` replaces the `-m 'not slow and not ochre'` in
  `pyproject.toml` `addopts`, so slow and `ochre` tests run.
  `--all-extras --all-groups` installs every optional package (gymnasium,
  HELICS, the `ochre` group). `--no-sync` stops uv from reinstalling the
  project over the release build `[python-build]` just installed. A skip not
  in the census (`ledger/02-measurement.md` M-0), or a census skip that no
  longer happens, fails the line. `census` compares file, count and reason
  with the line number stripped from both sides, so an edit above a skip
  site does not change the census; the census file keeps the line numbers
  for reading. The census holds only skips the block's environment cannot
  avoid; `ledger/11-other-defects.md` OD-13 makes the OCHRE comparison
  modules and the fleet conversion tests run and empties it, after which
  any skip fails the line.
- `[stray]`: the exclusions are the build and cache directories the block
  itself creates (`target/` and `.venv/` also on a clean checkout) and the
  ignored planning, engine-under-test worktree and editor-session
  directories (`.shipwright/`, `.workstrees/`, `.claude/`), which change
  while a block runs for reasons other than the block; any other new path
  fails, which is the decision log's stray-output rule. The block runs
  with `TMPDIR` pointed at an empty `target/block-tmp`; anything a line
  leaves there fails, except the two tool-owned entries (uv's
  environment lock and pytest's empty per-user root; a child of that
  root fails). From `01-baseline-repair.md` BR-24 on.
- `[tools]`: records the toolchain and tool versions the run used. A row's
  record in the `ledger/02-measurement.md` log names them, and a version
  that differs from the previous row's is stated with the row that first
  ran under it, so a `[clippy]` or advisory change is attributable to the
  tool or to the code.
- `[golden]`: `compare-all` compares the manifests whose feature set equals
  the binary's, so the two lines together cover every fixture. Present from
  M-2 on.
- `[advisories]`: cargo-deny defaults; no `deny.toml`, no license policy, no
  ignores.
- `[writing]`: any of these in the commit message or in an added line
  fails the entry: an em dash; a ledger entry ID; a requirement ID of
  `exploration.md` (`DV-`, `RL-`, `RS-`, `X-`, `PY-` and `FL-` IDs, and `R1`
  to `R25`); a `review.md` V-ID; a decision key (`dl-i08`); a review round
  (`round L3`, `L3-a`) or another planning and review reference of
  `ledger/10-slop-cleanup.md` SL-2; a reference to a language model or a
  coding agent (`LLM`, `Claude`, `subagent`; the reinforcement-learning
  sense of "agent" is not matched). A public issue reference (`HARES#28`)
  fails in an added line only, since a commit message may carry it
  (Writing, below). Added lines under `tests/fixtures/` and in lockfiles
  are data and are not read. A line that needs one of these tokens for
  another reason is reworded. Steps with no commit (Task 0, M-0, B0, M-3,
  the row 16g B0 re-check, the operator rows) skip it. Every consumer
  commit runs the same `ids` and `writing` definitions as
  `writing ~/src/HARES_CALIBRATION_SYSTEM` in its gate
  (`ledger/08-consumer.md`); its `Engine:` trailer carries only a commit
  hash.
- `[test-observe-profiling]`: the allocation tests that need both the
  `observe` per-stage telemetry keys and the profiling allocator
  (`ledger/06-runtime-memory.md` RT-1).
- `[test-release]` covers every physics, conservation and non-finite
  check: `ledger/03-output-reporting.md` OR-11 (`b8195c83`) deleted the
  `check_invariants` feature and its release test line.
- `[clippy-*]`: lint coverage equals build coverage: one line per
  profile and feature set a test, golden or wheel line of the block
  builds (`ledger/01-baseline-repair.md` BR-21).
- An entry's Verification may name a focused subset (one crate, one test
  file) used while implementing; the whole block still runs after its
  commit.

**Continuous integration.** The block, run locally after every commit, is
the gate; nothing is pushed before the closing sequence. CI repeats the
block's cargo lines on every push to `main` or `initial-implementation` and
on every pull request into either (`ledger/01-baseline-repair.md` BR-4 and
BR-10), so the closing sequence's pull request runs them on GitHub's
runners before the operator merges it. The `[golden]` lines run in their
own job on `ubuntu-24.04` with the rustc release the goldens were captured
with, because bit-exact agreement across C library or compiler releases is
not guaranteed; moving that pin re-runs `compare-all` locally first. A CI
job that fails at the closing sequence's first push becomes a fix entry
committed before M-4b, and the operator pushes again.

**No regressions, and no worse physics.** Each rule names the check that
proves it; the entry's reviewer confirms the record exists and holds.

1. A row that is not a numeric entry is bitwise-neutral: `[golden]` passes
   unchanged and its consumer check (`ledger/00-schedule.md`, column
   "Consumer check") equals B0. Any drift stops the row; it is never
   re-baselined inside that row. The only golden changes outside the
   numeric entries are listed in the schedule's rules (a new fixture, a
   fixture correction, a product schema change under M-1's schema change
   rule, and the fleet product changes of `ledger/04-fleet.md` FL-2), each
   with every other product and column bitwise and its change recorded.
2. A numeric entry (`ledger/09-numeric-fixes.md` and the rows the schedule
   places in the numeric block) shows that the physics did not get worse.
   It records, at its parent commit and after its change, its golden delta
   (M-1's numeric entry procedure) and the physics comparison suites:

   ```
   make check-ashrae-reference      # the ASHRAE RC reference fixture matches its generator
   cargo nextest run -p hares-core --no-fail-fast --no-capture \
     --test bestest --test parity --test conditioned_oracle --test freefloat_oracle \
     --test structural_envelope_oracle --test envelope_oracle
   cargo nextest run -p hares-physics --no-fail-fast --no-capture \
     --test physics_validation_tests --test solar_parity
   uv run --all-extras --all-groups --no-sync pytest -m "" -rxX \
     tests/python/test_ochre_parity.py tests/python/test_thermal_trace.py
   ```

   `bestest` prints each ASHRAE 140 case metric's signed distance to its
   band (`tests/bestest/mod.rs`, `ratchet_baseline`); `parity` prints each
   OCHRE parity check against its tolerance (`tests/parity/mod.rs`); the
   Python OCHRE parity tests run from `ledger/11-other-defects.md` OD-13
   on. The record lists every such metric before and after. No metric may
   move away from its reference unless the entry names the reference
   (EnergyPlus engineering reference or source, ASHRAE 140, the
   OpenStudio-HPXML documentation) that shows the new value is more
   correct; a ratchet baseline that moves is updated in the entry's commit
   with that cause. Each numeric entry records its physics decision and the
   reference it rests on in `docs/PHYSICS_DECISIONS.md` (created by
   `ledger/09-numeric-fixes.md` B-1a), in its own commit. The entry's
   reviewer checks this evidence.
3. A performance row (a `perf:` commit, or an entry with a Measurement
   field) records before and after with M-3's method
   (`ledger/02-measurement.md`: `frame_golden run` of the manifest, five
   fresh constructions, the median, the 5% noise band), both sides
   measured in one session from worktrees of the parent and the row's
   commit, since absolute wall times drift between sessions. A regression
   beyond the noise band stops the row.

**Independence** (`decision-log.md` `dl-i08-process.independence`).
Implementer, reviewer and red team are always three
different parties; nobody reviews, red-teams or approves their own work.
Every finding at every severity is fixed and pinned by a test before the
next entry starts, and every fix goes back to a review by a party that did
not write it before the row's commit is final.

**Per-entry review.** Each entry, once implemented, gets one independent
review pass and one independent red-team pass before its commit is
considered final. Findings from either pass are fixed in the same entry,
not deferred to a later one. The review criteria include, besides
correctness and the rules above: idiomatic Rust and Python, maintainability
and extensibility of the changed code, and no slop. The reviewer lists
every behaviour change the diff makes that no test pins, with the test file
it belongs in; each is a finding. `[clippy]` with
`-D warnings` is the Rust lint bar (`clippy::pedantic` is enabled nowhere in
the workspace, and no entry relies on it); a new `#[allow(...)]` or
`#[expect(...)]` carries a comment stating why the lint does not apply.
Python files an entry changes gain no `ruff check` finding:
`uv run --no-sync ruff check --no-cache <changed .py files>` reports no
more findings than at the parent commit, and none on a changed line.

**Slop cleanup as you go.** Every entry's implementer fixes any slop in
the code that entry touches: narrating or useless comments,
planning or ticket IDs left in code, verbose hedging documentation, dead
code, silent fallbacks, duplicated logic, and misleading comments (for
example a comment asserting behavior the code next to it does not have,
such as the "reuse their f64 slots without reallocating" comment at
`crates/hares-core/src/dwelling/mod.rs:6961-6963`, above a `clone_from`
that clones the whole map; see
`ledger/10-slop-cleanup.md`). The entry's reviewer checks for slop in the
diff as part of the normal review pass, not as a separate pass. Slop found
in code an entry does not otherwise touch is recorded in
`ledger/10-slop-cleanup.md` and fixed by the nearest entry that touches
that file, or, if no entry ever touches it, as its own small commit at the
end of the area it belongs to.

**Golden capture and bitwise comparison.** `tools/frame_golden`
(`ledger/02-measurement.md` M-1, which owns the format) runs checked-in
manifests. The repository holds, per fixture, compact per-column bit
digests of the frame, billing and health products plus the metrics in
full, never full-year frames; the full products live under
`target/golden/` for local delta inspection
(`dl-i08-architecture.golden_storage`). `compare` checks schema, row count
and every column's digest (floats by `to_bits()`); `delta` reports
per-column differences against locally materialized frames without
failing. An entry that must be bitwise-neutral passes `[golden]` unchanged.
An entry that changes numbers follows M-1's numeric entry procedure
(`materialize-all` at its parent commit, `delta`, `capture`), records the
result in the measurement log, and re-captures in its own commit. An entry
that changes a product's schema follows M-1's schema change rule. An entry
that changes how results are obtained updates the tool's engine adapter in
the entry's commit, so every commit compiles and passes `[golden]`.

**Recorded measurements, never timing assertions.** Every performance
number (a percentage share of `simulate()`, a byte count, an allocation
count) is written into `ledger/02-measurement.md`'s running log, before and
after, for the same process pair; the stop rule for a measured regression
is rule 3 above. No test asserts a wall-clock bound.
Allocation counts and factorization counts are asserted (they are exact,
not timing-sensitive).

**Consumer engine under test, gate, baseline and pin.**
`ledger/08-consumer.md` owns them and every consumer step uses its text:
the engine-under-test mechanism (one engine worktree per gated engine
commit under `/home/rich/src/HARES/.workstrees/`, installed into the
consumer's venv with `uv pip install --no-deps` and `--build-constraints`
pinning maturin to the version in that commit's `uv.lock`, never
`maturin develop`, which also installs the engine's `dev` group; then, as
step 4b, the engine-derived schema `results/engine_schema.json`
regenerated against the engine under test before every gate); the
consumer gate (every output written under `scratch/i08/<sha>/`, no failing
case, skips exactly the seven environmental base skips, the contract-drift
check printing `[hares-contract] PASS`, `ruff check` and `build_bundle.py`
recorded, `[writing]` on every consumer commit (Verification block above),
`paths.summary()` and the wheel's `Generator:` line recorded with every
run; no gate or capture runs against an editable install of
`/home/rich/src/HARES`, and the dry-run line of step 4, which names the
`.workstrees/<sha>` engine, is recorded with every run); the DE-trace
baseline B0 and the B0 check after every engine row the consumer can still
run; and the pin rule (the
`./HARES` gitlink stays at `0ec757cb` in every intermediate `i08-consumer`
commit, each ending with an `Engine: <HARES commit>` trailer, and advances
once, in CN-15, after the operator's last push).

**Closing sequence.** After the last numeric and consumer entry (CN-12):
M-4a lands; the operator merges PR #27 on GitHub, creates the
`CODECOV_TOKEN` repository secret (`ledger/01-baseline-repair.md` BR-10),
pushes `feat/output-results` and opens a pull request into
`initial-implementation`; CI and the reproducibility workflow run on that
pull request (a failing job becomes a fix entry before M-4b); M-4b lands;
the operator pushes again; CN-15 advances the
consumer pin once, to that pushed commit; `i08-consumer` is pushed only
when its pin names a commit that is on origin.

**Operator checkpoints.** A human confirms before the step proceeds:

- merging PR #27 (Task 0 merged it locally; the GitHub merge comes before
  the closing sequence's first push);
- creating `i08-consumer` (Task 0);
- the `CODECOV_TOKEN` repository secret: confirmed when BR-10 runs and
  created before the closing sequence's first push
  (`ledger/01-baseline-repair.md` BR-10; if the operator declines it,
  BR-10 removes the Codecov upload instead);
- quarantining the stray `dwelling_*.csv` and `dwelling_*_diagnostics.csv`
  files listed in Task 0 to `target/stray-quarantine/` (deletion is
  blocked on the implementation host);
- any push to origin, in either repository (both engine pushes of the
  closing sequence, and the consumer push);
- the nested `git -C HARES checkout --detach` that moves the consumer pin
  (CN-15);
- creating each engine-under-test worktree under
  `/home/rich/src/HARES/.workstrees/` (`dl-i08-execution.worktrees`), and
  removing any of them or their build directories (removal is done by the
  operator only);
- the index changes of `ledger/10-slop-cleanup.md` SL-8: each
  `git rm -r --cached` that stops tracking the planning and review records,
  and the untracking of `.shipwright/constitution.md`;
- moving the download cache from `~/.cache/ochre_next` to `~/.cache/hares`
  (CN-13);
- deleting the consumer's `scratch/fitrun` (`ledger/08-consumer.md` CN-8:
  6.2 GB of the 7.0 GB `scratch/` at `c71c542`, re-measured at execution),
  by that named directory only, never through `calibration/paths.py`
  `sweep_scratch` or any other sweep;
- adding any new consumer dependency;
- running the surrogate-search trial (`ledger/08-consumer.md`), and, as a
  separate later decision, adopting any method it reports on.

**Writing** (`decision-log.md` `dl-i08-process.writing`). No em dashes, no stock filler or hedging phrases, and no
history of earlier drafts, in anything that ships (code, comments, docs,
commit messages) or in these documents. Anything that ships also carries no
planning ID (ledger entry IDs, requirement IDs, review IDs, ticket IDs), no
reference to reviewers, review rounds or agents. These documents may name
the readiness, review and red-team process the decision log defines, and
may cite, as cross-references, the ledger's entry IDs and the identifiers of
the source documents in this directory (`review.md` V-IDs, `exploration.md`
requirement IDs, `context.md` item numbers, decision keys). Where a
requirement ID is spelled like a ledger entry ID (`exploration.md` PY-1 to
PY-14 and FL-1 to FL-11 against the ledger's PY and FL entries), the ledger
writes it with its source ("requirement PY-1"). A public GitHub issue
reference (`HARES#28`, `HARES#31`) may appear in a commit message, where it
links the commit to the public issue it closes; never in code, comments or
docs. `[writing]` checks each commit's message and added lines.

## 4. Ledger areas and their entries

See each file under `ledger/` for entries. `ledger/00-schedule.md` gives
the single global order.

## 5. Open operator decisions

None is open. The operator checkpoints of the closing sequence (§3: the
`CODECOV_TOKEN` secret, the PR #27 merge and every push) are not
decisions but steps the operator performs.

The supported Python interpreter range stays `requires-python =
">=3.12,<3.14"`, matching the interpreters CI tests, 3.12 and 3.13
(`decision-log.md` `dl-i08-process.readiness`, `python_support`;
`.github/workflows/ci.yml` `python-test`). It is an interpreter-support
statement, not a dependency pin, so the dependency decision does not move
it. Widening it to 3.14 would be its own entry after
`01-baseline-repair.md` BR-10, with a 3.14 CI matrix entry and a check
that the locked pyo3, numpy, numba and the vendored OCHRE reference (whose
own metadata declares `<3.13`) build and pass there.

The SI scope (`dl-i08-architecture.si_units`) covers everything HARES
stores or computes, including actor configuration, control signals,
actor telemetry, billing and metrics running state, per-step results and
gas rates; `ledger/09-numeric-fixes.md` B-14 carries it out.
