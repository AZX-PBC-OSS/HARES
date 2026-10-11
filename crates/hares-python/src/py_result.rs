//! Python bindings for the run result surface: `RunResult`, `ReportResult`,
//! `RunHealth`, `WarmupOutcome`, `Warning` and `Provenance`, the frozen
//! pyclasses over the Rust types `Dwelling::finish()` produces.

use std::sync::Arc;

use pyo3::prelude::*;

use hares_core::{
    EquipmentProvenance as RustEquipmentProvenance, Provenance as RustProvenance, ReportOutput,
    ReportResult as RustReportResult, RunResult as RustRunResult,
    WarmupOutcome as RustWarmupOutcome, WarmupResiduals as RustWarmupResiduals,
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
            })
            .collect()
    }

    /// The run's metrics; `None` when the run declared no report.
    #[getter]
    fn metrics(&self) -> Option<PyRunMetrics> {
        self.inner.metrics.as_ref().map(|metrics| PyRunMetrics {
            inner: metrics.clone(),
        })
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
}

#[pymethods]
impl PyReportResult {
    /// The report's name (unique within the run's config).
    #[getter]
    fn name(&self) -> String {
        self.inner.name.clone()
    }

    /// The report's frame: the retained batches as a polars `DataFrame`.
    /// A file or callback report raises `HaresSimulationError` naming the
    /// report and its destination.
    pub fn table<'py>(&self, py: pyo3::Python<'py>) -> PyResult<pyo3::Bound<'py, pyo3::PyAny>> {
        match &self.inner.output {
            ReportOutput::Memory(batches) => {
                let df = crate::conversions::record_batches_to_polars_df(py, batches)?;
                Ok(df.into_bound(py))
            }
            ReportOutput::File(_) => Err(HaresSimulationError::new_err(format!(
                "report '{}' writes to a file destination: the file is the report's \
                 result (its path getter names it); a table() needs a Memory report",
                self.inner.name
            ))),
            ReportOutput::Callback => Err(HaresSimulationError::new_err(format!(
                "report '{}' delivers to the batch callback: nothing is retained; \
                 a table() needs a Memory report",
                self.inner.name
            ))),
        }
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

/// The run's provenance: what produced it.
#[pyclass(frozen, name = "Provenance")]
pub struct PyProvenance {
    inner: RustProvenance,
}

#[pymethods]
impl PyProvenance {
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
