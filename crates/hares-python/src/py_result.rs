//! Python bindings for the run result surface: `RunResult`, `ReportResult`,
//! `RunHealth`, `WarmupOutcome`, `Warning` and `Provenance`, the frozen
//! pyclasses over the Rust types `Dwelling::finish()` produces.

use std::sync::Arc;
use std::sync::OnceLock;

use arrow::compute::concat_batches;
use arrow::record_batch::RecordBatch;
use pyo3::prelude::*;
use pyo3_arrow::PyTable;

use hares_core::{
    BuildInfo as RustBuildInfo, EquipmentProvenance as RustEquipmentProvenance,
    Provenance as RustProvenance, ReportOutput, ReportResult as RustReportResult,
    RunResult as RustRunResult, WarmupOutcome as RustWarmupOutcome,
    WarmupResiduals as RustWarmupResiduals,
};
use hares_types::Warning as RustWarning;

use crate::py_config::PySimulationConfig;
use crate::py_dwelling::HaresSimulationError;
use crate::py_metrics::PyRunMetrics;
use crate::py_report::PyFrequency;

/// One finished run's result: the reports' delivered outputs, the metrics,
/// the health totals, the warning record and the provenance.
#[pyclass(frozen, name = "RunResult")]
pub struct PyRunResult {
    pub(crate) inner: Arc<RustRunResult>,
    /// The run's catalogue: the dwelling's own, read at the finalisation
    /// (the run never changes it).
    pub(crate) catalogue: Arc<hares_io::Catalogue>,
}

#[pymethods]
impl PyRunResult {
    /// One result per configured report, in `reports` order.
    #[getter]
    fn reports(&self) -> Vec<PyReportResult> {
        self.inner
            .reports
            .iter()
            .map(|report| PyReportResult {
                inner: report.clone(),
                rechunked: OnceLock::new(),
            })
            .collect()
    }

    /// The run's metrics; `None` when the run did not request them
    /// (`SimulationConfig.metrics`).
    #[getter]
    fn metrics(&self) -> Option<PyRunMetrics> {
        self.inner.metrics.as_ref().map(|metrics| PyRunMetrics {
            inner: metrics.clone(),
        })
    }

    /// The catalogue the run's model declared: the same catalogue
    /// `Dwelling.catalogue()` reads, the run's variables with their units,
    /// kinds and labels.
    #[getter]
    fn catalogue(&self) -> crate::py_catalogue::PyCatalogue {
        crate::py_catalogue::PyCatalogue {
            inner: self.catalogue.clone(),
        }
    }

    /// The run's health totals, recorded unconditionally in every build.
    #[getter]
    fn health(&self) -> PyRunHealth {
        PyRunHealth {
            inner: self.inner.health.clone(),
        }
    }

    /// The run's warning record: the first 1 000 warnings of the run.
    #[getter]
    fn warnings(&self) -> Vec<PyWarning> {
        self.inner
            .warnings
            .iter()
            .map(|warning| PyWarning {
                inner: warning.clone(),
            })
            .collect()
    }

    /// Warnings dropped past the record's capacity.
    #[getter]
    fn warnings_dropped(&self) -> u64 {
        self.inner.warnings_dropped
    }

    /// What produced the run.
    #[getter]
    fn provenance(&self) -> PyProvenance {
        PyProvenance {
            inner: self.inner.provenance.clone(),
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "RunResult(reports={}, metrics={}, warnings={}, warnings_dropped={})",
            self.inner.reports.len(),
            if self.inner.metrics.is_some() {
                "RunMetrics"
            } else {
                "None"
            },
            self.inner.warnings.len(),
            self.inner.warnings_dropped,
        )
    }
}

/// One report's result as the run delivered it.
#[pyclass(frozen, name = "ReportResult")]
pub struct PyReportResult {
    inner: RustReportResult,
    /// The report's batches concatenated once, on the first
    /// `rechunk=True` export: every later export shares that batch's
    /// buffers and copies nothing.
    rechunked: OnceLock<RecordBatch>,
}

#[pymethods]
impl PyReportResult {
    /// The report's name (unique within the run's config).
    #[getter]
    fn name(&self) -> String {
        self.inner.name.clone()
    }

    /// The report's table as an Arrow stream: pyo3-arrow's `Table` class,
    /// whose repeatable `__arrow_c_stream__` any Arrow consumer (polars,
    /// pyarrow, pandas through pyarrow) imports with the report's buffers
    /// shared, no IPC serialize-and-parse round trip. With `rechunk=True`
    /// (the default) the report's batches are concatenated once, on the
    /// first export, and the one batch's buffers are shared by every
    /// later export; with `rechunk=False` the stream carries one chunk
    /// per built batch. A file or callback report holds no batches: its
    /// `arrow()` raises `HaresSimulationError` naming the report and its
    /// destination, never an empty table.
    #[pyo3(signature = (rechunk = true))]
    pub fn arrow(&self, rechunk: bool) -> PyResult<PyTable> {
        let batches = match &self.inner.output {
            ReportOutput::Memory(batches) => batches,
            ReportOutput::File(path) => {
                return Err(HaresSimulationError::new_err(format!(
                    "report '{}' writes to a file destination: the file is the report's \
                     result (its path getter names it, '{}'); a table() needs a Memory report",
                    self.inner.name,
                    path.display()
                )));
            }
            ReportOutput::Callback => {
                return Err(HaresSimulationError::new_err(format!(
                    "report '{}' delivers to the batch callback: nothing is retained \
                     ({} batches delivered); a table() needs a Memory report",
                    self.inner.name, self.inner.batches_delivered
                )));
            }
        };
        if rechunk {
            if let Some(batch) = self.rechunked.get() {
                return crate::conversions::batches_to_py_table(vec![batch.clone()]);
            }
            if let Some(first) = batches.first() {
                let batch = concat_batches(&first.schema(), batches).map_err(|err| {
                    pyo3::exceptions::PyValueError::new_err(format!(
                        "report '{}': the retained batches could not be concatenated: {}",
                        self.inner.name, err
                    ))
                })?;
                let _ = self.rechunked.set(batch.clone());
                return crate::conversions::batches_to_py_table(vec![batch]);
            }
        }
        crate::conversions::batches_to_py_table(batches.clone())
    }

    /// The report's frame: the `arrow()` export read as a polars
    /// `DataFrame`. A file or callback report raises
    /// `HaresSimulationError` naming the report and its destination.
    #[pyo3(signature = (rechunk = true))]
    pub fn table<'py>(
        &self,
        py: pyo3::Python<'py>,
        rechunk: bool,
    ) -> PyResult<pyo3::Bound<'py, pyo3::PyAny>> {
        let table = pyo3::Py::new(py, self.arrow(rechunk)?)?;
        let polars = py.import("polars")?;
        let df = polars.getattr("DataFrame")?.call1((table,))?;
        Ok(df)
    }

    /// The published file path; `None` for a memory or callback report.
    #[getter]
    fn path(&self) -> Option<String> {
        self.inner.path().map(|path| path.display().to_string())
    }

    /// Batches the report built (and delivered) over the run.
    #[getter]
    fn batches_delivered(&self) -> u64 {
        self.inner.batches_delivered
    }

    /// Rows the report recorded over the run.
    #[getter]
    fn rows(&self) -> usize {
        self.inner.rows
    }

    /// The reporting frequency the report reduced at.
    #[getter]
    fn frequency(&self) -> PyFrequency {
        PyFrequency(self.inner.frequency)
    }

    fn __repr__(&self) -> String {
        match &self.inner.output {
            ReportOutput::Memory(batches) => format!(
                "ReportResult(name='{}', memory, rows={})",
                self.inner.name,
                batches
                    .iter()
                    .map(arrow::record_batch::RecordBatch::num_rows)
                    .sum::<usize>(),
            ),
            ReportOutput::File(path) => format!(
                "ReportResult(name='{}', file='{}', rows={})",
                self.inner.name,
                path.display(),
                self.inner.rows,
            ),
            ReportOutput::Callback => format!(
                "ReportResult(name='{}', callback, rows={})",
                self.inner.name, self.inner.rows,
            ),
        }
    }
}

/// The run's health totals.
#[pyclass(frozen, name = "RunHealth")]
pub struct PyRunHealth {
    pub(crate) inner: hares_core::RunHealth,
}

#[pymethods]
impl PyRunHealth {
    /// Equipment steps whose port contributions were rolled back.
    #[getter]
    fn port_rollbacks(&self) -> u64 {
        self.inner.port_rollbacks
    }

    /// Control signals rejected by equipment or routed at a target that
    /// does not exist.
    #[getter]
    fn rejected_control_signals(&self) -> u64 {
        self.inner.rejected_control_signals
    }

    /// Equipment actions clamped to a safe bound.
    #[getter]
    fn clamped_actions(&self) -> u64 {
        self.inner.clamped_actions
    }

    /// Biquadratic curve-index clamps over the run.
    #[getter]
    fn curve_index_clamps(&self) -> u64 {
        self.inner.curve_index_clamps
    }

    /// Fluid-payload absences after the fluid domain's first publish.
    #[getter]
    fn fluid_payload_absences(&self) -> u64 {
        self.inner.fluid_payload_absences
    }

    /// Actors evicted by roster changes.
    #[getter]
    fn evicted_actors(&self) -> u64 {
        self.inner.evicted_actors
    }

    /// The warm-up convergence outcome.
    #[getter]
    fn warmup(&self) -> PyWarmupOutcome {
        PyWarmupOutcome {
            inner: self.inner.warmup.clone(),
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "RunHealth(port_rollbacks={}, rejected_control_signals={}, clamped_actions={}, \
             curve_index_clamps={}, fluid_payload_absences={}, evicted_actors={})",
            self.inner.port_rollbacks,
            self.inner.rejected_control_signals,
            self.inner.clamped_actions,
            self.inner.curve_index_clamps,
            self.inner.fluid_payload_absences,
            self.inner.evicted_actors,
        )
    }

    fn __eq__(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

/// How the warm-up convergence loop ended.
#[pyclass(frozen, name = "WarmupOutcome")]
pub struct PyWarmupOutcome {
    inner: RustWarmupOutcome,
}

#[pymethods]
impl PyWarmupOutcome {
    /// `"disabled"` when warm-up never ran, `"ran"` when it did.
    #[getter]
    fn outcome(&self) -> String {
        match &self.inner {
            RustWarmupOutcome::Disabled => "disabled".to_string(),
            RustWarmupOutcome::Ran { .. } => "ran".to_string(),
        }
    }

    /// Warm-up days simulated; `None` when warm-up never ran.
    #[getter]
    fn days_run(&self) -> Option<u32> {
        match &self.inner {
            RustWarmupOutcome::Disabled => None,
            RustWarmupOutcome::Ran { days_run, .. } => Some(*days_run),
        }
    }

    /// Whether the convergence threshold was met; `None` when warm-up
    /// never ran.
    #[getter]
    fn converged(&self) -> Option<bool> {
        match &self.inner {
            RustWarmupOutcome::Disabled => None,
            RustWarmupOutcome::Ran { converged, .. } => Some(*converged),
        }
    }

    /// The final warm-up day's residuals; `None` when warm-up never ran or
    /// only one warm-up day ran.
    #[getter]
    fn residuals(&self) -> Option<PyWarmupResiduals> {
        match &self.inner {
            RustWarmupOutcome::Disabled => None,
            RustWarmupOutcome::Ran { residuals, .. } => residuals
                .as_ref()
                .map(|r| PyWarmupResiduals { inner: r.clone() }),
        }
    }

    fn __repr__(&self) -> String {
        match &self.inner {
            RustWarmupOutcome::Disabled => "WarmupOutcome.disabled".to_string(),
            RustWarmupOutcome::Ran {
                days_run,
                converged,
                ..
            } => format!("WarmupOutcome.ran(days_run={days_run}, converged={converged})"),
        }
    }

    fn __eq__(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

/// The final warm-up day's day-over-day residuals.
#[pyclass(frozen, name = "WarmupResiduals")]
pub struct PyWarmupResiduals {
    inner: RustWarmupResiduals,
}

#[pymethods]
impl PyWarmupResiduals {
    #[getter]
    fn worst_zone(&self) -> u16 {
        self.inner.worst_zone.0
    }

    #[getter]
    fn max_temperature_c(&self) -> f64 {
        self.inner.max_temperature_c
    }

    #[getter]
    fn min_temperature_c(&self) -> f64 {
        self.inner.min_temperature_c
    }

    #[getter]
    fn heating_load(&self) -> f64 {
        self.inner.heating_load
    }

    #[getter]
    fn cooling_load(&self) -> f64 {
        self.inner.cooling_load
    }

    fn __repr__(&self) -> String {
        format!(
            "WarmupResiduals(worst_zone={}, max_temperature_c={:.3}, min_temperature_c={:.3})",
            self.inner.worst_zone.0, self.inner.max_temperature_c, self.inner.min_temperature_c,
        )
    }

    fn __eq__(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

/// One warning a run reports.
#[pyclass(frozen, name = "Warning")]
pub struct PyWarning {
    inner: RustWarning,
}

#[pymethods]
impl PyWarning {
    /// Simulation step the warning was raised in; `None` for
    /// construction-time warnings.
    #[getter]
    fn step_index(&self) -> Option<u64> {
        self.inner.step_index
    }

    /// Which producer raised the warning.
    #[getter]
    fn source(&self) -> String {
        self.inner.source.to_string()
    }

    /// The producer's message.
    #[getter]
    fn message(&self) -> String {
        self.inner.message.clone()
    }

    fn __str__(&self) -> String {
        self.inner.to_string()
    }

    fn __repr__(&self) -> String {
        format!("Warning({})", self.inner)
    }
}

/// The build's compile-time identity: what built the engine the
/// extension wraps. Recorded once per build by the crate's build script
/// and identical to every run's `Provenance.build`.
#[pyclass(frozen, name = "BuildInfo")]
pub struct PyBuildInfo {
    inner: RustBuildInfo,
}

#[pymethods]
impl PyBuildInfo {
    /// The crate version the build compiled.
    #[getter]
    fn version(&self) -> &str {
        self.inner.version
    }

    /// The git commit the build compiled from; `None` when built without
    /// a git work tree.
    #[getter]
    fn commit(&self) -> Option<&str> {
        self.inner.commit
    }

    /// Whether the source tree the build compiled had tracked edits;
    /// `None` when git or `.git` is absent.
    #[getter]
    fn dirty(&self) -> Option<bool> {
        self.inner.dirty
    }

    /// The build profile (`debug` or `release`).
    #[getter]
    fn profile(&self) -> &str {
        self.inner.profile
    }

    /// The features the build enabled, sorted.
    #[getter]
    fn features(&self) -> Vec<String> {
        self.inner
            .features
            .iter()
            .map(|feature| feature.to_string())
            .collect()
    }

    /// The compiler's version line (`rustc -V`).
    #[getter]
    fn rustc(&self) -> &str {
        self.inner.rustc
    }

    fn __repr__(&self) -> String {
        let features: Vec<String> = self
            .inner
            .features
            .iter()
            .map(|feature| format!("'{feature}'"))
            .collect();
        format!(
            "BuildInfo(version='{}', commit={}, dirty={}, profile='{}', features=[{}], \
             rustc='{}')",
            self.inner.version,
            self.inner
                .commit
                .map(|commit| format!("'{commit}'"))
                .unwrap_or_else(|| "None".to_string()),
            self.inner
                .dirty
                .map(|dirty| if dirty { "True" } else { "False" })
                .unwrap_or("None"),
            self.inner.profile,
            features.join(", "),
            self.inner.rustc,
        )
    }
}

/// The extension build's identity: the record every run's
/// `Provenance.build` carries, read once per build.
#[pyfunction]
pub fn build_info() -> PyBuildInfo {
    PyBuildInfo {
        inner: RustBuildInfo::get(),
    }
}

/// The run's provenance: what produced it.
#[pyclass(frozen, name = "Provenance")]
pub struct PyProvenance {
    inner: RustProvenance,
}

#[pymethods]
impl PyProvenance {
    /// The build's compile-time identity: the version, commit, dirty
    /// flag, profile, features and rustc of the build that produced the
    /// run.
    #[getter]
    fn build(&self) -> PyBuildInfo {
        PyBuildInfo {
            inner: self.inner.build.clone(),
        }
    }

    /// The output catalogue's digest, the 64 char lowercase hex string
    /// the schema's `hares:catalogue_digest` metadata carries.
    #[getter]
    fn catalogue_digest(&self) -> String {
        self.inner
            .catalogue_digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    /// The validated configuration the run ran under.
    #[getter]
    fn config(&self) -> PySimulationConfig {
        PySimulationConfig(self.inner.config.clone())
    }

    /// The building id the run ran for.
    #[getter]
    fn bldg_id(&self) -> i64 {
        self.inner.bldg_id
    }

    /// The site's UTC offset in seconds.
    #[getter]
    fn site_utc_offset_s(&self) -> i32 {
        self.inner.site_utc_offset_s
    }

    /// One entry per equipment, in equipment order.
    #[getter]
    fn equipment(&self) -> Vec<PyEquipmentProvenance> {
        self.inner
            .equipment
            .iter()
            .map(|entry| PyEquipmentProvenance {
                inner: entry.clone(),
            })
            .collect()
    }

    fn __repr__(&self) -> String {
        format!(
            "Provenance(bldg_id={}, site_utc_offset_s={}, equipment={})",
            self.inner.bldg_id,
            self.inner.site_utc_offset_s,
            self.inner.equipment.len(),
        )
    }
}

/// One equipment's init-time provenance.
#[pyclass(frozen, name = "EquipmentProvenance")]
pub struct PyEquipmentProvenance {
    inner: RustEquipmentProvenance,
}

#[pymethods]
impl PyEquipmentProvenance {
    /// The equipment's instance name.
    #[getter]
    fn name(&self) -> String {
        self.inner.name.clone()
    }

    /// The zone the equipment serves.
    #[getter]
    fn zone(&self) -> Option<u16> {
        self.inner.zone.map(|zone| zone.0)
    }

    /// The HPXML location label the zone type came from.
    #[getter]
    fn zone_type(&self) -> Option<String> {
        self.inner.zone_type.clone()
    }

    /// OCV data provenance for battery and EV equipment.
    #[getter]
    fn ocv_source(&self) -> Option<String> {
        self.inner.ocv_source.clone()
    }

    /// The equipment's own init-time provenance lines.
    #[getter]
    fn notes(&self) -> Vec<String> {
        self.inner.notes.clone()
    }

    fn __repr__(&self) -> String {
        format!(
            "EquipmentProvenance(name='{}', zone={:?}, zone_type={:?})",
            self.inner.name,
            self.inner.zone.map(|z| z.0),
            self.inner.zone_type,
        )
    }
}

#[cfg(test)]
mod tests {
    use hares_core::DwellingSettings;
    use hares_io::{DefaultsSource, InputSource, WeatherInput};
    use std::path::PathBuf;
    use std::sync::OnceLock;

    use arrow::record_batch::RecordBatch;
    use hares_core::{Dwelling, DwellingConfig, SimulationEngine};
    use pyo3::Python;
    use pyo3::types::{PyAnyMethods, PyCapsule};
    use pyo3_arrow::PyTable;

    use super::PyReportResult;

    /// The `full_year_hourly` manifest's run, its report retained in
    /// memory: the run the capsule export's acceptance tests read.
    /// `output_chunk_size` overrides the config's default (10 000,
    /// which holds this frame's 8 760 rows in one batch): a test
    /// asserting the rechunk cache's buffer sharing needs a
    /// multi-batch report, because `concat_batches` short-circuits one
    /// input to a slice and the pointers are equal with or without the
    /// cache.
    fn full_year_hourly_report(output_chunk_size: usize) -> hares_core::ReportResult {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let manifest = frame_golden::GoldenManifest::load(
            &root.join("tests/fixtures/golden/full_year_hourly.toml"),
        )
        .expect("the manifest loads");
        let mut sim = manifest
            .simulation_config()
            .expect("the manifest's sim config");
        sim.output_chunk_size = output_chunk_size;
        let home = &manifest.home[0];
        let config = DwellingConfig {
            hpxml: InputSource::Path(frame_golden::manifest::repo_path(&root, &home.hpxml)),
            schedule: Some(InputSource::Path(frame_golden::manifest::repo_path(
                &root,
                &home.schedule,
            ))),
            weather: WeatherInput::Path(frame_golden::manifest::repo_path(&root, &home.weather)),
            defaults: DefaultsSource::directory(root.join(&manifest.defaults)),
            settings: DwellingSettings::new(
                sim.clone(),
                Some(home.overrides.clone()),
                home.bldg_id,
                if home.initialization_duration_s == 0 {
                    None
                } else {
                    Some(std::time::Duration::from_secs(
                        home.initialization_duration_s,
                    ))
                },
                None,
            ),
        };
        let mut dwelling = Dwelling::from_config(config).expect("the dwelling builds");
        let result = SimulationEngine::new()
            .run_dwelling(&mut dwelling, &sim)
            .expect("the run finishes");
        result
            .reports
            .iter()
            .find(|report| report.name == "frame")
            .expect("the manifest's memory report")
            .clone()
    }

    /// The table exported through `__arrow_c_stream__` and imported back
    /// through pyo3-arrow's `from_arrow_pycapsule` equals the report's
    /// batches column by column, floats by bit pattern.
    #[test]
    fn capsule_export_is_bitwise_identical() {
        let report = full_year_hourly_report(10_000);
        let expected: Vec<RecordBatch> = report.batches().to_vec();
        Python::attach(|py| {
            let wrapper = pyo3::Py::new(
                py,
                PyReportResult {
                    inner: report,
                    rechunked: OnceLock::new(),
                },
            )
            .expect("the wrapper constructs");
            let table = wrapper
                .bind(py)
                .borrow()
                .arrow(true)
                .expect("the report exports");
            let table = pyo3::Py::new(py, table).expect("the table wraps");
            let capsule = table
                .bind(py)
                .call_method1("__arrow_c_stream__", (py.None(),))
                .expect("the table exposes __arrow_c_stream__");
            let capsule = capsule
                .cast::<PyCapsule>()
                .expect("the stream is a capsule");
            let imported = PyTable::from_arrow_pycapsule(capsule).expect("the capsule imports");
            crate::conversions::assert_batches_bitwise_equal(
                "the capsule import",
                &expected,
                imported.batches(),
            );
        });
    }

    /// Two `arrow()` calls on one `ReportResult` share the rechunked
    /// batch: every column buffer's data pointer is equal, nothing is
    /// recopied. The report is multi-batch (`output_chunk_size` 40
    /// flushes 219 batches over the year): on a single-batch report
    /// this pin is vacuous, arrow's `concat` short-circuits one input
    /// to a slice and the pointers are equal with or without the
    /// cache.
    #[test]
    fn second_export_does_not_recopy() {
        let report = full_year_hourly_report(40);
        assert!(
            report.batches().len() > 1,
            "the fixture report is multi-batch: on one batch the concat \
             short-circuits and the pin is vacuous"
        );
        Python::attach(|py| {
            let wrapper = pyo3::Py::new(
                py,
                PyReportResult {
                    inner: report,
                    rechunked: OnceLock::new(),
                },
            )
            .expect("the wrapper constructs");
            let bound = wrapper.bind(py);
            let first = bound.borrow().arrow(true).expect("the first export");
            let second = bound.borrow().arrow(true).expect("the second export");

            let buffer_ptrs = |table: &PyTable| -> Vec<usize> {
                let mut ptrs = Vec::new();
                for batch in table.batches() {
                    for column in batch.columns() {
                        let data = column.to_data();
                        for buffer in data.buffers() {
                            ptrs.push(buffer.as_ptr() as usize);
                        }
                    }
                }
                ptrs
            };
            assert!(
                !buffer_ptrs(&first).is_empty(),
                "the first export has buffers"
            );
            assert_eq!(
                buffer_ptrs(&first),
                buffer_ptrs(&second),
                "the second export shares the first one's buffers"
            );
        });
    }

    /// A rechunk=False export keeps one chunk per built batch while the
    /// rechunked export holds one batch, every value in order.
    #[test]
    fn rechunk_false_keeps_batch_boundaries_rust() {
        use arrow::array::{Float64Array, TimestampMillisecondArray};
        use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
        use std::sync::Arc;

        let schema = Arc::new(Schema::new(vec![
            Field::new(
                "Time",
                DataType::Timestamp(TimeUnit::Millisecond, None),
                false,
            ),
            Field::new("v", DataType::Float64, false),
        ]));
        let batches: Vec<RecordBatch> = [3_usize, 3, 2]
            .iter()
            .enumerate()
            .map(|(index, rows)| {
                let base = (index * 3) as i64;
                RecordBatch::try_new(
                    schema.clone(),
                    vec![
                        Arc::new(TimestampMillisecondArray::from(
                            (0..*rows as i64)
                                .map(|step| base + step)
                                .collect::<Vec<_>>(),
                        )),
                        Arc::new(Float64Array::from(
                            (0..*rows as i64)
                                .map(|step| (base + step) as f64)
                                .collect::<Vec<_>>(),
                        )),
                    ],
                )
                .unwrap()
            })
            .collect();
        let report = hares_core::ReportResult {
            name: "rechunk".to_string(),
            output: hares_core::ReportOutput::Memory(batches.clone()),
            frequency: hares_io::Frequency::Timestep,
            rows: 8,
            batches_delivered: 3,
        };
        Python::attach(|py| {
            let wrapper = pyo3::Py::new(
                py,
                PyReportResult {
                    inner: report,
                    rechunked: OnceLock::new(),
                },
            )
            .expect("the wrapper constructs");
            let bound = wrapper.bind(py);
            let rechunked = bound.borrow().arrow(true).expect("the rechunked export");
            assert_eq!(rechunked.batches().len(), 1, "rechunk=True holds one batch");
            let unchunked = bound.borrow().arrow(false).expect("the unchunked export");
            assert_eq!(
                unchunked.batches().len(),
                3,
                "rechunk=False keeps one chunk per built batch"
            );
            let expected = arrow::compute::concat_batches(&schema, &batches).unwrap();
            crate::conversions::assert_batches_bitwise_equal(
                "the rechunked batch",
                &[expected],
                rechunked.batches(),
            );
        });
    }
}
