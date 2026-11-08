//! The calibration evaluation surface: one typed entry point that performs
//! the complete override-build-plan-run-return transaction.
//!
//! [`PyCalibrationDwelling`] wraps a dwelling's configuration and nothing
//! else. Each [`PyCalibrationDwelling::evaluate`] call builds a fresh
//! immutable dwelling blueprint from that configuration with the call's
//! overrides and seed, freezes ONE observation plan (the engine's own
//! [`hares_core::ObservationPlan`] machinery: an unknown field fails the
//! freeze, naming it, before any run step), runs the requested independent
//! periods to completion, and returns the observations as Arrow IPC whose
//! schema metadata carries the fields, their units, the timestamp
//! convention, the seed, the periods' warm-up outcomes and the periods'
//! provenance. The source dwelling is never acquired: an evaluation
//! cannot touch its state.
//!
//! The honesty the surface sells, and its failure shape: an unknown
//! override or observation field fails with zero run steps simulated
//! (the plan-freeze build runs no warm-up and no step); zero periods is
//! refused outright, never substituted with empty output; every failure
//! is the engine's own typed exception class, carrying the failing stage
//! and the run steps simulated as attributes. There is no in-place
//! parameter application, no hidden parallel run pool (the periods run
//! sequentially on the calling thread, one dwelling resident at a time),
//! and the schema ordering is the caller's field order, deterministic by
//! construction.

use std::collections::HashMap;
use std::sync::Arc;

use arrow::array::{Float64Array, RecordBatch, TimestampMillisecondArray, UInt32Array};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use arrow::ipc::writer::StreamWriter;
use chrono::{DateTime, FixedOffset};
use hares_core::{Dwelling, DwellingConfig, DwellingSettings, RunResult};
use hares_output::Catalogue;
use hares_types::panic_hook::PanicHookGuard;
use hares_types::{HaresError, Unit};
use pyo3::exceptions::{PyRuntimeError, PyTypeError};
use pyo3::prelude::*;
use pyo3::types::{PyAnyMethods, PyBytes, PyDict, PyType};
use pyo3_arrow::PyTable;

use crate::py_callback::{CallbackCauseSlot, PyBatchCallback};
use crate::py_dwelling::{PyDwelling, to_py_err_msg};
use crate::py_result::weather_format_name;

/// The metadata key the surface's IPC schema carries, one per contract
/// item: the surface's name, the seed, the periods' bounds, the fields
/// and their units, the warm-up outcomes and the provenance.
mod meta {
    pub const SURFACE: &str = "hares:surface";
    pub const SURFACE_NAME: &str = "calibration.evaluate";
    pub const SEED: &str = "hares:seed";
    pub const PERIODS: &str = "hares:periods";
    pub const FIELDS: &str = "hares:fields";
    pub const UNITS: &str = "hares:units";
    pub const WARMUP: &str = "hares:warmup";
    pub const PROVENANCE: &str = "hares:provenance";
    /// The timestamp column's convention, spelled in the metadata: the
    /// step's own time, the output frame's convention.
    pub const TIMESTAMP: &str = "hares:timestamp";
    pub const TIMESTAMP_CONVENTION: &str = "local wall clock at the step's interval start, milliseconds; \
         the site offset in hares:periods recovers the instant";
}

/// One evaluate failure: the stage of the transaction that failed, the
/// run steps simulated when it did, and the engine error that caused it.
/// The failure's Python exception class is the cause's own (the
/// classification `to_py_err` applies to every engine error); the stage
/// and the step count ride the exception as attributes and the count is
/// in the message, so a calibration sweep names how far it got.
struct EvaluationFailure {
    stage: &'static str,
    steps: u64,
    err: HaresError,
}

impl EvaluationFailure {
    fn new(stage: &'static str, steps: u64, err: HaresError) -> Self {
        Self { stage, steps, err }
    }
}

/// Raises the failure as the cause's own exception class, the stage and
/// the step count set as attributes and the count named in the message.
fn fail(py: Python<'_>, failure: EvaluationFailure) -> PyErr {
    let EvaluationFailure { stage, steps, err } = failure;
    let message = format!("{err}; {steps} run steps simulated");
    let py_err = to_py_err_msg(err, message);
    attach_failure(py, py_err, stage, steps)
}

/// Attaches the stage and the step count to an already-raised Python
/// error (the argument-extraction phases' own): the attributes are the
/// typed contract, the message stands.
fn fail_py(py: Python<'_>, err: PyErr, stage: &'static str, steps: u64) -> PyErr {
    attach_failure(py, err, stage, steps)
}

fn attach_failure(py: Python<'_>, py_err: PyErr, stage: &str, steps: u64) -> PyErr {
    let value = py_err.value(py);
    let _ = value.setattr("stage", stage.to_string());
    let _ = value.setattr("steps_simulated", steps);
    py_err
}

/// One input record as the provenance metadata spells it: the path it was
/// read from, or the kind it was supplied as when it is content, and the
/// hex digest. The content itself never enters the metadata: the record
/// names its input, the file carries the content.
fn record_json(record: &hares_input::InputRecord) -> serde_json::Value {
    let source = match &record.source {
        hares_input::InputRecordSource::Path(path) => {
            serde_json::json!({"path": path.display().to_string()})
        }
        hares_input::InputRecordSource::Text(_) => serde_json::json!({"kind": "text"}),
        hares_input::InputRecordSource::Bytes(_) => serde_json::json!({"kind": "bytes"}),
    };
    serde_json::json!({ "source": source, "sha256": record.hex() })
}

/// The digest as the 64 char lowercase hex string the provenance getters
/// print (`hares_input`'s own hex helper).
fn hex32(bytes: &[u8; 32]) -> String {
    hares_input::input::hex(bytes)
}

/// One period run's provenance as the IPC metadata's JSON: the build
/// identity, the catalogue digest, the settings the period ran under
/// (the merged overrides among them), the site offset, the equipment
/// provenance, the inputs' identities and digests, the attached tariff's
/// and script's identities, and the replay blockers. The content of no
/// input enters the metadata: the digests name the inputs.
fn provenance_json(provenance: &hares_core::Provenance) -> serde_json::Value {
    let inputs = provenance.inputs.as_ref().map(|inputs| {
        serde_json::json!({
            "hpxml": record_json(&inputs.hpxml),
            "schedule": inputs.schedule.as_ref().map(record_json),
            "weather": record_json(&inputs.weather),
            "weather_format": weather_format_name(inputs.weather_format),
            "defaults_files_sha256": hex32(&inputs.defaults_files_sha256),
        })
    });
    serde_json::json!({
        "build": serde_json::to_value(&provenance.build)
            .expect("a BuildInfo always serialises: plain strings and bools"),
        "catalogue_digest": hex32(&provenance.catalogue_digest),
        "settings": serde_json::to_value(&provenance.settings)
            .expect("DwellingSettings always serialises: configs, overrides and patches"),
        "site_utc_offset_s": provenance.site_utc_offset_s,
        "equipment": serde_json::to_value(&provenance.equipment)
            .expect("the equipment provenance always serialises: strings and ids"),
        "inputs": inputs,
        "tariff": provenance.tariff.as_ref().map(|record| {
            serde_json::json!({"zone": record.zone, "attach_step": record.attach_step})
        }),
        "script": provenance.script.as_ref().map(|record| {
            serde_json::json!({"name": record.name, "sha256": hex32(&record.sha256)})
        }),
        "replay_blockers": serde_json::to_value(&provenance.replay_blockers)
            .expect("the replay blockers always serialise: field-less unit variants"),
    })
}

/// The call's overrides merged over the configuration's own, the merge
/// rule the scenario-sample path applies: per top-level key (the
/// equipment name or wildcard), the caller's entry wins, replacing the
/// base entry wholesale. `None` on either side stands for "overrides
/// nothing": an absent call dict leaves the base's overrides, an absent
/// base takes the call's.
fn merge_overrides(
    base: &Option<serde_json::Value>,
    call: Option<serde_json::Value>,
) -> Option<serde_json::Value> {
    let Some(call) = call else {
        return base.clone();
    };
    let Some(base) = base else {
        return Some(call);
    };
    let (serde_json::Value::Object(base_map), serde_json::Value::Object(call_map)) = (base, &call)
    else {
        // The base is not an override map (no path constructs one); the
        // call's map is the honest resolution, replacing it.
        return Some(call);
    };
    let mut merged = base_map.clone();
    for (key, value) in call_map {
        merged.insert(key.clone(), value.clone());
    }
    Some(serde_json::Value::Object(merged))
}

/// One period's configuration: the wrapped configuration's inputs and
/// settings, with the period's start and duration, the seed and the
/// merged overrides applied. The configuration is validated here: a
/// period whose bounds the config rejects (a zero duration, a duration
/// that does not divide into whole steps) fails naming the period,
/// before any build.
fn period_config(
    base: &DwellingConfig,
    overrides: &Option<serde_json::Value>,
    start: DateTime<FixedOffset>,
    duration_s: i64,
    seed: u64,
    index: usize,
) -> Result<DwellingConfig, HaresError> {
    let mut sim = base.settings.sim_config.clone();
    sim.start_time = start;
    sim.duration = chrono::Duration::try_seconds(duration_s).ok_or_else(|| {
        HaresError::Dwelling(format!(
            "period {index}'s duration ({duration_s} s) is too large for the calendar"
        ))
    })?;
    sim.master_seed = seed;
    sim.validate()
        .map_err(|err| HaresError::Dwelling(format!("period {index}'s configuration: {err}")))?;
    Ok(DwellingConfig {
        hpxml: base.hpxml.clone(),
        schedule: base.schedule.clone(),
        weather: base.weather.clone(),
        defaults: base.defaults.clone(),
        settings: DwellingSettings::new(
            sim,
            overrides.clone(),
            base.settings.bldg_id,
            base.settings.patches.clone(),
        ),
    })
}

/// One period's completed run: the finished run's result and catalogue,
/// the provenance metadata, the period's bounds as they ran, and the
/// observation rows (the period index, the step's wall-clock
/// milliseconds, one row per step).
struct PeriodRun {
    result: Arc<RunResult>,
    catalogue: Arc<Catalogue>,
    provenance: serde_json::Value,
    start_rfc3339: String,
    duration_s: i64,
    steps: usize,
    utc_offset_s: i32,
    rows: Vec<(u32, i64, Vec<f64>)>,
}

/// The detached transaction's output: the frozen plan's field names and
/// units, the periods' runs, and the run steps simulated in total.
struct Evaluated {
    fields: Vec<String>,
    units: Vec<Unit>,
    runs: Vec<PeriodRun>,
    steps_simulated: u64,
}

/// One calibration evaluation's result: the observations of every period
/// as one Arrow table (the IPC stream the `ipc_bytes` surface serialises,
/// the schema's metadata carrying the contract items), the periods'
/// warm-up outcomes and the finished run results.
#[pyclass(frozen, name = "CalibrationRun")]
pub struct PyCalibrationRun {
    batch: RecordBatch,
    fields: Vec<String>,
    seed: u64,
    steps_simulated: u64,
    warmups: Vec<hares_core::WarmupOutcome>,
    results: Vec<(Arc<RunResult>, Arc<Catalogue>)>,
    /// The IPC stream's bytes, serialised at the run's construction: a
    /// run that cannot produce its stream is a failed evaluation, and
    /// every read of these bytes is the same bytes.
    ipc: Vec<u8>,
}

#[pymethods]
impl PyCalibrationRun {
    /// The observation fields, in column order (the caller's field
    /// order).
    #[getter]
    fn fields(&self) -> Vec<String> {
        self.fields.clone()
    }

    /// The seed the periods ran under.
    #[getter]
    fn seed(&self) -> u64 {
        self.seed
    }

    /// The periods that ran.
    #[getter]
    fn n_periods(&self) -> usize {
        self.results.len()
    }

    /// Observation rows over all periods: one per step.
    #[getter]
    fn rows(&self) -> usize {
        self.batch.num_rows()
    }

    /// The run steps the evaluation simulated, all periods together.
    #[getter]
    fn steps_simulated(&self) -> u64 {
        self.steps_simulated
    }

    /// The periods' warm-up outcomes, in period order: the engine's own
    /// `RunHealth.warmup` values, read off the finished runs. A disabled
    /// warm-up reads `disabled` with no residuals; a one-day warm-up
    /// reads `ran` with no residuals (no predecessor to diff against);
    /// an unconverged one reads its days and its final day's residuals.
    fn warmup_residuals(&self) -> Vec<crate::py_result::PyWarmupOutcome> {
        self.warmups
            .iter()
            .map(|warmup| crate::py_result::PyWarmupOutcome {
                inner: warmup.clone(),
            })
            .collect()
    }

    /// The observations as an Arrow stream: pyo3-arrow's `Table`, whose
    /// `__arrow_c_stream__` any Arrow consumer imports zero-copy. The
    /// schema's metadata carries the contract items (the surface, the
    /// seed, the periods, the fields and units, the warm-up outcomes and
    /// the provenance).
    fn arrow(&self) -> PyResult<PyTable> {
        crate::conversions::batches_to_py_table(vec![self.batch.clone()])
    }

    /// The observations as the Arrow IPC stream's bytes: the schema
    /// message, then every row, then the end. The serialization is
    /// deterministic: the same inputs and seed produce byte-identical
    /// bytes, schema ordering included.
    fn ipc_bytes<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        Ok(PyBytes::new(py, &self.ipc))
    }

    /// The finished runs' results, one per period in period order: the
    /// reports the configuration declared ride every period's run, and
    /// their delivered outputs read here.
    #[getter]
    fn results(&self) -> Vec<crate::py_result::PyRunResult> {
        self.results
            .iter()
            .map(|(result, catalogue)| crate::py_result::PyRunResult {
                inner: Arc::clone(result),
                catalogue: Arc::clone(catalogue),
            })
            .collect()
    }

    fn __repr__(&self) -> String {
        format!(
            "CalibrationRun(fields={}, periods={}, rows={}, seed={})",
            self.fields.len(),
            self.results.len(),
            self.batch.num_rows(),
            self.seed,
        )
    }
}

/// Serialises the batch as the Arrow IPC stream: the schema message, the
/// batches, the end.
fn serialize_ipc(batch: &RecordBatch) -> Result<Vec<u8>, HaresError> {
    let mut buffer = Vec::new();
    let mut writer = StreamWriter::try_new(&mut buffer, &batch.schema()).map_err(|err| {
        HaresError::Simulation(format!(
            "the calibration table's schema does not serialise: {err}"
        ))
    })?;
    writer.write(batch).map_err(|err| {
        HaresError::Simulation(format!("the calibration table did not serialise: {err}"))
    })?;
    writer.finish().map_err(|err| {
        HaresError::Simulation(format!("the calibration IPC stream did not finish: {err}"))
    })?;
    Ok(buffer)
}

/// The calibration evaluation surface over one dwelling's configuration.
#[pyclass(name = "CalibrationDwelling")]
pub struct PyCalibrationDwelling {
    /// The wrapped configuration: the inputs and settings every
    /// evaluation builds fresh blueprints from. Never mutated by an
    /// evaluate call: the call's overrides and seed apply to the fresh
    /// builds alone.
    config: DwellingConfig,
    /// The batch callbacks attached for the runs: the report name and
    /// the callable, attached to every period's dwelling before its
    /// steps. A callable's exception fails the evaluation as the typed
    /// callback error, chained as `__cause__`. The callable sits in an
    /// `Arc`: every period's attach shares it, no GIL needed.
    callbacks: Vec<(String, Arc<Py<PyAny>>)>,
    /// The callbacks' cause slot, one per surface: the adapter stores the
    /// callable's exception there and the failure mapping chains it.
    callback_cause: CallbackCauseSlot,
}

#[pymethods]
impl PyCalibrationDwelling {
    /// Wraps a dwelling's configuration: the evaluation runs that
    /// configuration (its inputs, settings, reports and warm-up policy),
    /// rebuilt fresh per call. The source dwelling is not acquired: its
    /// runtime state (registered actors, applied controls, an attached
    /// tariff, stepped history) is not part of what an evaluation runs,
    /// and its state cannot move.
    #[classmethod]
    pub fn from_dwelling(_cls: &Bound<'_, PyType>, source: &PyDwelling) -> PyResult<Self> {
        Ok(Self {
            config: source.config.clone(),
            callbacks: Vec::new(),
            callback_cause: Arc::new(std::sync::Mutex::new(None)),
        })
    }

    /// Attaches a Python callable as the runs' batch callback: the same
    /// attachment `Dwelling.attach_callback` performs, applied to every
    /// period's dwelling before its steps. The report must be the
    /// configuration's own and deliver to the callback destination; the
    /// callable's exception fails the evaluation as the typed callback
    /// error with the raised object chained as `__cause__`.
    pub fn attach_callback(&mut self, report: &str, callback: Bound<'_, PyAny>) -> PyResult<()> {
        if !callback.is_callable() {
            let repr = callback
                .repr()
                .map(|r| r.to_string())
                .unwrap_or_else(|_| "unprintable".to_string());
            return Err(PyTypeError::new_err(format!(
                "the batch callback must be callable, got {repr}"
            )));
        }
        self.callbacks
            .push((report.to_string(), Arc::new(callback.unbind())));
        Ok(())
    }

    /// The one typed evaluation entry point: `overrides` (a dict of
    /// equipment overrides, merged over the configuration's own, the
    /// caller's entries winning per equipment key), `observation_fields`
    /// (the fields the table carries, one column per field in this
    /// order), `periods` (a list of `(start, duration_s)` pairs, each an
    /// independent run) and `seed` (the master seed the periods' random
    /// streams derive from). Returns the run: the observations as Arrow
    /// IPC, the warm-up outcomes and the provenance in the schema's
    /// metadata.
    ///
    /// The transaction's order is the honesty contract: the overrides
    /// and the periods validate first (typed, no dwelling built), then
    /// one no-warm-up build freezes the observation plan (an unknown
    /// field fails here, with zero run steps simulated and zero steps of
    /// any kind simulated: the freeze build runs no warm-up), then the
    /// periods run, each from a fresh immutable blueprint carrying the
    /// overrides and the seed. The same seed, overrides and periods
    /// reproduce byte-identical IPC.
    #[pyo3(signature = (overrides, observation_fields, periods, seed))]
    pub fn evaluate(
        &self,
        py: Python<'_>,
        overrides: Option<&Bound<'_, PyDict>>,
        observation_fields: Vec<String>,
        periods: Vec<(String, i64)>,
        seed: u64,
    ) -> PyResult<PyCalibrationRun> {
        let _guard = PanicHookGuard::new();
        #[cfg(debug_assertions)]
        #[cfg(feature = "profiling")]
        {
            assert!(
                hares_types::panic_hook::is_installed(),
                "custom panic hook must be installed before simulation"
            );
        }

        // The overrides and the periods validate before any build: the
        // typed failures here simulated nothing.
        let call_overrides = crate::py_config::parse_overrides(overrides)
            .map_err(|err| fail_py(py, err, "overrides", 0))?;

        if periods.is_empty() {
            return Err(fail(
                py,
                EvaluationFailure::new(
                    "periods",
                    0,
                    HaresError::Dwelling(
                        "an evaluation with zero periods returns no run: name at least \
                         one (start, duration_s) period; empty output is never \
                         substituted for a run"
                            .to_string(),
                    ),
                ),
            ));
        }
        let mut parsed: Vec<(DateTime<FixedOffset>, i64)> = Vec::with_capacity(periods.len());
        for (index, (start, duration_s)) in periods.iter().enumerate() {
            if *duration_s <= 0 {
                return Err(fail(
                    py,
                    EvaluationFailure::new(
                        "periods",
                        0,
                        HaresError::Dwelling(format!(
                            "period {index}'s duration must be greater than zero, got \
                             {duration_s} s"
                        )),
                    ),
                ));
            }
            let start = match crate::utils::parse_datetime_str(start) {
                Ok(start) => start,
                Err(err) => {
                    return Err(fail(
                        py,
                        EvaluationFailure::new(
                            "periods",
                            0,
                            HaresError::Dwelling(format!(
                                "period {index}'s start '{start}' does not parse: {err}"
                            )),
                        ),
                    ));
                }
            };
            parsed.push((start, *duration_s));
        }

        let merged = merge_overrides(&self.config.settings.overrides, call_overrides);
        let cause_slot = Arc::clone(&self.callback_cause);
        // A stale cause from an earlier evaluation must not chain onto
        // this one's failure.
        *cause_slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;

        let callbacks: &[(String, Arc<Py<PyAny>>)] = &self.callbacks;
        let config = self.config.clone();
        let observation_fields = observation_fields.clone();

        // The detached transaction: the builds, the plan freeze, the
        // periods' runs and the table's inputs are Python-free. The
        // failure crosses as the stage, the step count and the engine
        // error; a panic crosses as the hook's message (the pattern
        // every run surface applies), claiming no step count.
        let evaluated: Result<Result<Evaluated, EvaluationFailure>, String> = py.detach(|| {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_transaction(
                    &config,
                    &merged,
                    &observation_fields,
                    &parsed,
                    seed,
                    callbacks,
                    &cause_slot,
                )
            })) {
                Ok(inner) => Ok(inner),
                Err(payload) => Err(format!(
                    "HARES internal panic: {}",
                    hares_types::panic_hook::panic_payload_to_string(payload)
                )),
            }
        });

        let evaluated = match evaluated {
            Ok(Ok(evaluated)) => evaluated,
            Ok(Err(failure)) => {
                // The callbacks' cause, when the failure is one: chained
                // as `__cause__`, the way every run surface maps it.
                let cause = cause_slot
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                let py_err = fail(py, failure);
                if let Some(cause) = cause {
                    let _ = py_err.value(py).setattr("__cause__", cause.value(py));
                }
                return Err(py_err);
            }
            Err(panic_message) => return Err(PyRuntimeError::new_err(panic_message)),
        };

        build_calibration_run(py, evaluated, seed)
    }

    fn __repr__(&self) -> String {
        format!(
            "CalibrationDwelling(overrides={}, reports={})",
            if self.config.settings.overrides.is_some() {
                "set"
            } else {
                "none"
            },
            self.config.settings.sim_config.reports.len(),
        )
    }
}

/// The table, its metadata and the IPC stream, from the transaction's
/// output: the columns in the plan's order, the metadata's values the
/// contract items.
fn build_calibration_run(
    py: Python<'_>,
    evaluated: Evaluated,
    seed: u64,
) -> PyResult<PyCalibrationRun> {
    let Evaluated {
        fields,
        units,
        runs,
        steps_simulated,
    } = evaluated;
    let warmups: Vec<hares_core::WarmupOutcome> = runs
        .iter()
        .map(|run| run.result.health.warmup.clone())
        .collect();

    // The table: the period index, the wall-clock timestamp, one
    // float64 column per field in the plan's (the caller's) order.
    let total_rows: usize = runs.iter().map(|run| run.rows.len()).sum();
    let mut period_col: Vec<u32> = Vec::with_capacity(total_rows);
    let mut stamp_col: Vec<i64> = Vec::with_capacity(total_rows);
    let mut value_cols: Vec<Vec<f64>> = (0..fields.len())
        .map(|_| Vec::with_capacity(total_rows))
        .collect();
    for run in &runs {
        for (period, stamp, row) in &run.rows {
            period_col.push(*period);
            stamp_col.push(*stamp);
            for (column, value) in value_cols.iter_mut().zip(row) {
                column.push(*value);
            }
        }
    }

    let mut schema_fields = vec![
        Field::new("period", DataType::UInt32, false),
        Field::new(
            "timestamp",
            DataType::Timestamp(TimeUnit::Millisecond, None),
            false,
        ),
    ];
    for name in &fields {
        schema_fields.push(Field::new(name.clone(), DataType::Float64, false));
    }

    // The metadata: the contract items, every key the schema serialises
    // sorted (the IPC writer sorts them), every value a deterministic
    // function of the inputs and seed.
    let mut units_map = serde_json::Map::new();
    for (name, unit) in fields.iter().zip(&units) {
        units_map.insert(name.clone(), serde_json::json!(unit.symbol()));
    }
    let periods_json: Vec<serde_json::Value> = runs
        .iter()
        .map(|run| {
            serde_json::json!({
                "start": run.start_rfc3339,
                "duration_s": run.duration_s,
                "steps": run.steps,
                "utc_offset_s": run.utc_offset_s,
            })
        })
        .collect();
    let provenance_json: Vec<serde_json::Value> =
        runs.iter().map(|run| run.provenance.clone()).collect();
    let mut metadata = HashMap::new();
    metadata.insert(meta::SURFACE.to_string(), meta::SURFACE_NAME.to_string());
    metadata.insert(meta::SEED.to_string(), seed.to_string());
    metadata.insert(
        meta::FIELDS.to_string(),
        serde_json::to_string(&fields).expect("a field list always serialises"),
    );
    metadata.insert(
        meta::UNITS.to_string(),
        serde_json::to_string(&serde_json::Value::Object(units_map))
            .expect("a unit map always serialises"),
    );
    metadata.insert(
        meta::PERIODS.to_string(),
        serde_json::to_string(&periods_json).expect("a period list always serialises"),
    );
    metadata.insert(
        meta::WARMUP.to_string(),
        serde_json::to_string(&warmups).expect("the warm-up outcomes always serialise"),
    );
    metadata.insert(
        meta::PROVENANCE.to_string(),
        serde_json::to_string(&provenance_json).expect("the provenance records always serialise"),
    );
    metadata.insert(
        meta::TIMESTAMP.to_string(),
        meta::TIMESTAMP_CONVENTION.to_string(),
    );

    let schema = Arc::new(Schema::new_with_metadata(schema_fields, metadata));
    let columns: Vec<Arc<dyn arrow::array::Array>> =
        std::iter::once(Arc::new(UInt32Array::from(period_col)) as Arc<dyn arrow::array::Array>)
            .chain(std::iter::once(
                Arc::new(TimestampMillisecondArray::from(stamp_col))
                    as Arc<dyn arrow::array::Array>,
            ))
            .chain(
                value_cols.into_iter().map(|column| {
                    Arc::new(Float64Array::from(column)) as Arc<dyn arrow::array::Array>
                }),
            )
            .collect();
    let batch = RecordBatch::try_new(schema, columns).map_err(|err| {
        fail(
            py,
            EvaluationFailure::new(
                "table",
                steps_simulated,
                HaresError::Simulation(format!("the calibration table failed to build: {err}")),
            ),
        )
    })?;

    // The IPC serialises at construction: a run that cannot produce its
    // stream is a failed evaluation, never a run object that fails on
    // read.
    let ipc_bytes = serialize_ipc(&batch)
        .map_err(|err| fail(py, EvaluationFailure::new("table", steps_simulated, err)))?;

    Ok(PyCalibrationRun {
        batch,
        fields,
        seed,
        steps_simulated,
        warmups,
        results: runs
            .into_iter()
            .map(|run| (run.result, run.catalogue))
            .collect(),
        ipc: ipc_bytes,
    })
}

/// The transaction's body, Python-free: the no-warm-up plan-freeze build,
/// then the periods' runs. Every failure carries its stage and the run
/// steps simulated when it happened.
#[allow(clippy::too_many_arguments)]
fn run_transaction(
    config: &DwellingConfig,
    merged_overrides: &Option<serde_json::Value>,
    observation_fields: &[String],
    periods: &[(DateTime<FixedOffset>, i64)],
    seed: u64,
    callbacks: &[(String, Arc<Py<PyAny>>)],
    cause_slot: &CallbackCauseSlot,
) -> Result<Evaluated, EvaluationFailure> {
    // The plan-freeze build: period 0's configuration with the warm-up
    // disabled, so the freeze simulates nothing. The assembly's override
    // validation and the plan's field resolution both fail here, before
    // any build that simulates, with zero run steps on the count.
    let (first_start, first_duration) = periods[0];
    let mut freeze_config = period_config(
        config,
        merged_overrides,
        first_start,
        first_duration,
        seed,
        0,
    )
    .map_err(|err| EvaluationFailure::new("dwelling build", 0, err))?;
    freeze_config.settings.sim_config.warmup = hares_input::Warmup::Disabled;
    let freeze_dwelling = Dwelling::from_config(freeze_config)
        .map_err(|err| EvaluationFailure::new("dwelling build", 0, err))?;
    let plan = freeze_dwelling
        .observation_plan(observation_fields)
        .map_err(|err| EvaluationFailure::new("observation plan", 0, err))?;
    let fields = plan.field_names();
    let units = plan.field_units();
    drop(freeze_dwelling);

    // The periods, sequentially on this thread, one dwelling resident at
    // a time: built, run, read, dropped.
    let mut steps_simulated: u64 = 0;
    let mut runs: Vec<PeriodRun> = Vec::with_capacity(periods.len());
    for (index, &(start, duration_s)) in periods.iter().enumerate() {
        let period_config = period_config(config, merged_overrides, start, duration_s, seed, index)
            .map_err(|err| EvaluationFailure::new("periods", steps_simulated, err))?;
        let mut dwelling = Dwelling::from_config(period_config)
            .map_err(|err| EvaluationFailure::new("dwelling build", steps_simulated, err))?;
        for (report, callable) in callbacks {
            dwelling
                .attach_callback(
                    report,
                    Box::new(PyBatchCallback::shared(
                        Arc::clone(callable),
                        Arc::clone(cause_slot),
                    )),
                )
                .map_err(|err| EvaluationFailure::new("callback attach", steps_simulated, err))?;
        }

        // The period's steps: the clock's own count, which is the run's
        // simulated step count: the calendar steps less every skipped
        // Feb 29's steps under the typical-year rule. Deriving the count
        // from the requested duration instead would loop past the
        // configured end on a span that contains one. One observation read
        // per step, after the step wrote its state; the step that
        // simulated but could not be read counts.
        let step_count = dwelling.clock.total_steps();
        let total_steps = usize::try_from(step_count).map_err(|_| {
            EvaluationFailure::new(
                "periods",
                steps_simulated,
                HaresError::Dwelling(format!(
                    "period {index}'s step count ({step_count}) does not fit the platform's \
                     usize"
                )),
            )
        })?;
        let mut scratch = hares_core::StepResult::default();
        let mut rows: Vec<(u32, i64, Vec<f64>)> = Vec::with_capacity(total_steps);
        for _ in 0..total_steps {
            dwelling
                .step_into(&mut scratch)
                .map_err(|err| EvaluationFailure::new("run", steps_simulated, err))?;
            steps_simulated += 1;
            let mut row = vec![0.0; plan.len()];
            dwelling
                .observe_into(&plan, &mut row)
                .map_err(|err| EvaluationFailure::new("run", steps_simulated, err))?;
            rows.push((
                index as u32,
                scratch.timestamp.and_utc().timestamp_millis(),
                row,
            ));
        }

        let result = dwelling
            .finish()
            .map_err(|err| EvaluationFailure::new("run", steps_simulated, err))?;
        let provenance = provenance_json(&result.provenance);
        let utc_offset_s = result.provenance.site_utc_offset_s;
        let catalogue = dwelling.catalogue();
        runs.push(PeriodRun {
            result,
            catalogue,
            provenance,
            start_rfc3339: start.to_rfc3339(),
            duration_s,
            steps: total_steps,
            utc_offset_s,
            rows,
        });
        // The dwelling is consumed: only the finished run's records stay.
        drop(dwelling);
    }

    Ok(Evaluated {
        fields,
        units,
        runs,
        steps_simulated,
    })
}

#[cfg(test)]
mod tests {
    use pyo3::prelude::*;
    use pyo3::types::PyString;

    use super::PyCalibrationDwelling;
    use crate::py_config::PySimulationConfig;
    use crate::py_dwelling::PyDwelling;
    use hares_types::alloc_count::{thread_alloc_bytes, thread_allocations};

    /// The cz4a fixture's PyDwelling, the calibration witnesses' own
    /// construction path: the two-day hourly horizon, the warm-up
    /// disabled (the pins measure the evaluation's steady state, not the
    /// warm-up's).
    fn cz4a_source(py: Python<'_>) -> PyDwelling {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let fixture = root.join("tests/fixtures/parity/cz4a_ashp_hpwh");
        let cls = py.get_type::<PyDwelling>();
        let config = PySimulationConfig::new(
            "2023-01-01T00:00:00-07:00",
            crate::py_config::WholeSeconds(2 * 86_400),
            crate::py_config::WholeSeconds(3600),
            None,
            false,
            false,
            hares_input::DEFAULT_OUTPUT_CHUNK_SIZE,
            None,
            None,
            7,
            None,
            "on",
            None,
            None,
            hares_input::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
            None,
            crate::py_config::PyWarmupArg::ExplicitNone,
            "hour24",
            "typical",
            None,
        )
        .expect("the sim config builds");
        let defaults = root.join("python/hares/defaults");
        PyDwelling::from_hpxml(
            &cls,
            PyString::new(py, fixture.join("building.xml").to_str().expect("utf8")).as_any(),
            Some(PyString::new(py, fixture.join("schedule.csv").to_str().expect("utf8")).as_any()),
            PyString::new(py, fixture.join("weather.epw").to_str().expect("utf8")).as_any(),
            config,
            1,
            Some(PyString::new(py, defaults.to_str().expect("utf8")).as_any()),
            None,
            None,
            None,
        )
        .expect("the cz4a fixture constructs")
    }

    /// Two evaluate calls with the same arguments return byte-identical
    /// IPC (the schema and the payload are functions of the inputs), and
    /// the steady-state repeated call's allocation count is the call
    /// before it's: no hidden state grows, and the run's allocations all
    /// land on the calling thread (a hidden parallel run pool would move
    /// them off it). The exact-count form of the repeated-call memory
    /// pin.
    #[test]
    fn repeated_evaluate_is_byte_identical_and_allocation_stable() {
        Python::attach(|py| {
            let source = cz4a_source(py);
            let cls = py.get_type::<PyDwelling>();
            let cal = Py::new(
                py,
                PyCalibrationDwelling::from_dwelling(&cls, &source)
                    .expect("the surface wraps the source"),
            )
            .expect("the surface wraps");

            let fields = vec!["total_power_w".to_string(), "outdoor_temp".to_string()];
            let periods = vec![("2023-01-01T00:00:00-07:00".to_string(), 2 * 86_400_i64)];

            let evaluate_once = |py: Python<'_>| -> (Vec<u8>, u64) {
                let bound = cal.bind(py);
                let before = thread_allocations().expect("the counting allocator");
                let run = bound
                    .borrow()
                    .evaluate(py, None, fields.clone(), periods.clone(), 7)
                    .expect("the evaluation runs");
                let ipc = run.ipc_bytes(py).expect("the IPC").as_bytes().to_vec();
                drop(run);
                let after = thread_allocations().expect("the counting allocator");
                (ipc, after - before)
            };
            let held_bytes = |py: Python<'_>| -> i64 {
                let before = thread_alloc_bytes().expect("the counting allocator");
                let _ = evaluate_once(py);
                thread_alloc_bytes().expect("the counting allocator") - before
            };

            let (first_ipc, d1) = evaluate_once(py);
            assert!(!first_ipc.is_empty());
            let (second_ipc, d2) = evaluate_once(py);
            let (third_ipc, d3) = evaluate_once(py);
            let (fourth_ipc, d4) = evaluate_once(py);
            let (_fifth_ipc, d5) = evaluate_once(py);

            // The bytes pin, exact: a completed evaluation whose run is
            // dropped leaves the thread's live requested bytes exactly
            // where they were. Nothing the evaluation built stays
            // resident: no pool, no accumulation, no leak.
            assert_eq!(held_bytes(py), 0, "the evaluation holds nothing back");
            assert_eq!(held_bytes(py), 0, "the second one either");

            assert_eq!(
                first_ipc, second_ipc,
                "the same seed and overrides reproduce byte-identical IPC"
            );
            assert_eq!(
                first_ipc, third_ipc,
                "the third evaluation reproduces the bytes too"
            );
            assert_eq!(first_ipc, fourth_ipc);

            // The count pin, bounded: the repeated call allocates what
            // the call before it allocated, within a handful of
            // allocations. The measured shape on this fixture is
            // 348 566 to 348 568, the drift ±2 on ~348.6k from the
            // engine's process-global content sharing's one-time lazy
            // initializations, no trend: growth or a hidden run pool
            // would move the count by the run's own size, not by two.
            for drift in [(d2, d3), (d3, d4), (d4, d5)] {
                assert!(
                    drift.0.abs_diff(drift.1) <= 4,
                    "the repeated call's allocation count drifted: {d1} {d2} \
                     {d3} {d4} {d5}"
                );
            }
        });
    }
}
