//! Simulation engine main loop.

use std::collections::BTreeMap;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::time::{Duration as StdDuration, Instant};

use arrow::record_batch::RecordBatch;
use hares_io::output::metrics::{
    AnnualEnergyKwh, GridInteractionMetrics, MetricsCalculator, PeakPowerKw, RollingPeakKw,
    SimulationMetrics,
};
use hares_io::{OutputFormat, SimulationConfig};
use hares_types::HaresError;
use hares_types::panic_hook::{self, PanicHookGuard, record_double_panic_prevented};

/// Result of computing metrics from Arrow batches, including status context.
struct MetricsOutcome {
    metrics: SimulationMetrics,
    /// If set, indicates that metrics computation degraded (e.g., calculator init failed).
    warning: Option<String>,
}

#[cfg(feature = "profiling")]
use crate::dwelling::DwellingProfilingSummary;
use crate::dwelling::{Dwelling, DwellingConfig};

const KERNEL_TOTAL: &str = "total";
const KERNEL_CONSTRUCT_DWELLING: &str = "construct_dwelling";
const KERNEL_SIMULATE: &str = "simulate";

/// Batch-runner entry point for CLI and non-Python workflows.
#[derive(Debug, Default, Clone, Copy)]
pub struct SimulationEngine;

/// Execution status for a single simulation run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SimStatus {
    /// Simulation completed with no warnings.
    Ok,
    /// Simulation completed, but warnings or soft-limit violations occurred.
    Flagged(String),
    /// Simulation failed to complete.
    Failed(String),
}

/// Engine-level output payload.
#[derive(Debug, Clone, PartialEq)]
pub struct SimulationResults {
    /// Path to the flushed timeseries output file.
    pub timeseries_path: Option<PathBuf>,
    /// Optional in-memory timeseries batches (disabled by default).
    pub timeseries: Option<Vec<RecordBatch>>,
    /// Scalar summary metrics.
    pub metrics: SimulationMetrics,
    /// Non-fatal warnings accumulated during the run.
    pub warnings: Vec<String>,
    /// Status marker for this run.
    pub status: SimStatus,
    /// Wall-clock elapsed time.
    pub elapsed: StdDuration,
}

/// Scoped timer utility used by profiling instrumentation.
#[derive(Debug)]
pub struct KernelTimer {
    kernel_name: &'static str,
    started_at: Instant,
}

impl KernelTimer {
    #[must_use]
    pub fn start(kernel_name: &'static str) -> Self {
        Self {
            kernel_name,
            started_at: Instant::now(),
        }
    }

    pub fn stop(self) {
        #[cfg(feature = "profiling")]
        tracing::info!(
            kernel = self.kernel_name,
            elapsed_ms = self.started_at.elapsed().as_secs_f64() * 1_000.0,
            "kernel timing"
        );
        #[cfg(not(feature = "profiling"))]
        {
            let _ = self.kernel_name;
            let _ = self.started_at;
        }
    }
}

impl SimulationEngine {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Run a simulation from config -- creates the dwelling internally.
    pub fn run(&self, config: DwellingConfig) -> Result<SimulationResults, HaresError> {
        validate_input_paths(&config)?;

        let run_started = Instant::now();
        let mut profile = RunProfile::default();

        let construction_timer = KernelTimer::start(KERNEL_CONSTRUCT_DWELLING);
        let construction_started = Instant::now();
        let mut dwelling = Dwelling::from_config(config.clone())?;
        construction_timer.stop();
        profile.record(KERNEL_CONSTRUCT_DWELLING, construction_started.elapsed());

        let simulation_timer = KernelTimer::start(KERNEL_SIMULATE);
        let simulation_started = Instant::now();
        let _guard = PanicHookGuard::new();
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            assert!(
                panic_hook::is_installed(),
                "custom panic hook must be installed before simulation"
            );
        }
        let sim_outcome = panic::catch_unwind(AssertUnwindSafe(|| dwelling.simulate()));
        simulation_timer.stop();
        profile.record(KERNEL_SIMULATE, simulation_started.elapsed());

        let elapsed = run_started.elapsed();
        profile.record(KERNEL_TOTAL, elapsed);
        emit_profile_summary(&profile);

        let output_path = resolved_output_path(&config);
        let mut warnings = dwelling.take_warnings();
        let result = match sim_outcome {
            Ok(Ok(_dwelling_results)) => {
                let batches = dwelling.flushed_batches().to_vec();
                #[cfg(feature = "profiling")]
                emit_dwelling_profiling_summary(&dwelling.profiling_summary());

                if batches.is_empty() {
                    // Zero-step edge case: duration == 0 or duration == initialization_duration.
                    // No data was produced, so flag rather than return bogus metrics.
                    warnings.push(
                        "simulation produced zero output batches (zero-step run)".to_string(),
                    );
                    SimulationResults {
                        timeseries_path: output_path.clone(),
                        timeseries: Some(Vec::new()),
                        metrics: empty_metrics(),
                        warnings,
                        status: SimStatus::Flagged(
                            "zero-step simulation: no metrics computed".to_string(),
                        ),
                        elapsed,
                    }
                } else {
                    let outcome = compute_metrics_from_batches(&batches, &config.sim_config);
                    if let Some(w) = &outcome.warning {
                        warnings.push(w.clone());
                    }
                    let status = if warnings.is_empty() {
                        SimStatus::Ok
                    } else {
                        SimStatus::Flagged(format!("{} warning(s)", warnings.len()))
                    };
                    SimulationResults {
                        timeseries_path: output_path.clone(),
                        timeseries: Some(batches),
                        metrics: outcome.metrics,
                        warnings,
                        status,
                        elapsed,
                    }
                }
            }
            Ok(Err(err)) => {
                // Guard against double-panic: the format! and String::to_string
                // calls below allocate. If the allocator is corrupted from a
                // prior near-panic, these allocations could panic and abort the
                // process. The inner catch_unwind absorbs any such secondary
                // panic and returns a minimal fallback.
                let handler_result = panic::catch_unwind(AssertUnwindSafe(|| {
                    warnings.push(format!("simulation error: {err}"));
                    SimulationResults {
                        timeseries_path: output_path.clone(),
                        timeseries: None,
                        metrics: empty_metrics(),
                        warnings,
                        status: SimStatus::Failed(err.to_string()),
                        elapsed,
                    }
                }));
                match handler_result {
                    Ok(result) => result,
                    Err(_) => {
                        record_double_panic_prevented();
                        #[cfg(any(debug_assertions, feature = "check_invariants"))]
                        {
                            tracing::error!(
                                "CRITICAL: error handling panicked \
                                 (double-panic prevented) in engine::run"
                            );
                        }
                        SimulationResults {
                            timeseries_path: None,
                            timeseries: None,
                            metrics: empty_metrics(),
                            warnings: Vec::new(),
                            status: SimStatus::Failed(
                                "panic handling failed (double-panic prevented)".into(),
                            ),
                            elapsed,
                        }
                    }
                }
            }
            Err(payload) => {
                let handler_result = panic::catch_unwind(AssertUnwindSafe(|| SimulationResults {
                    timeseries_path: output_path,
                    timeseries: None,
                    metrics: empty_metrics(),
                    warnings,
                    status: SimStatus::Failed(panic_hook::panic_payload_to_string(payload)),
                    elapsed,
                }));
                match handler_result {
                    Ok(result) => result,
                    Err(_) => {
                        record_double_panic_prevented();
                        #[cfg(any(debug_assertions, feature = "check_invariants"))]
                        {
                            tracing::error!(
                                "CRITICAL: error handling panicked \
                                 (double-panic prevented) in engine::run"
                            );
                        }
                        SimulationResults {
                            timeseries_path: None,
                            timeseries: None,
                            metrics: empty_metrics(),
                            warnings: Vec::new(),
                            status: SimStatus::Failed(
                                "panic handling failed (double-panic prevented)".into(),
                            ),
                            elapsed,
                        }
                    }
                }
            }
        };

        Ok(result)
    }

    /// Run a simulation with a pre-configured dwelling.
    ///
    /// Use this when you need to configure the dwelling before simulation
    /// (e.g., enabling the observer, setting initial state, or injecting
    /// custom domain solvers).
    ///
    /// ```ignore
    /// let mut dwelling = Dwelling::from_config(config)?;
    /// dwelling.enable_observer(60);
    /// let result = engine.run_dwelling(&mut dwelling, &sim_config)?;
    /// let snapshots = dwelling.drain_observations();
    /// ```
    pub fn run_dwelling(
        &self,
        dwelling: &mut Dwelling,
        sim_config: &SimulationConfig,
    ) -> Result<SimulationResults, HaresError> {
        let run_started = Instant::now();

        let _guard = PanicHookGuard::new();
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            assert!(
                panic_hook::is_installed(),
                "custom panic hook must be installed before simulation"
            );
        }
        let sim_outcome = panic::catch_unwind(AssertUnwindSafe(|| dwelling.simulate()));
        let elapsed = run_started.elapsed();

        let mut warnings = dwelling.take_warnings();
        let result = match sim_outcome {
            Ok(Ok(_)) => {
                let batches = dwelling.flushed_batches().to_vec();
                if batches.is_empty() {
                    warnings.push(
                        "simulation produced zero output batches (zero-step run)".to_string(),
                    );
                    SimulationResults {
                        timeseries_path: None,
                        timeseries: Some(Vec::new()),
                        metrics: empty_metrics(),
                        warnings,
                        status: SimStatus::Flagged(
                            "zero-step simulation: no metrics computed".to_string(),
                        ),
                        elapsed,
                    }
                } else {
                    let outcome = compute_metrics_from_batches(&batches, sim_config);
                    if let Some(w) = &outcome.warning {
                        warnings.push(w.clone());
                    }
                    let status = if warnings.is_empty() {
                        SimStatus::Ok
                    } else {
                        SimStatus::Flagged(format!("{} warning(s)", warnings.len()))
                    };
                    SimulationResults {
                        timeseries_path: None,
                        timeseries: Some(batches),
                        metrics: outcome.metrics,
                        warnings,
                        status,
                        elapsed,
                    }
                }
            }
            Ok(Err(err)) => {
                let handler_result = panic::catch_unwind(AssertUnwindSafe(|| {
                    warnings.push(format!("simulation error: {err}"));
                    SimulationResults {
                        timeseries_path: None,
                        timeseries: None,
                        metrics: empty_metrics(),
                        warnings,
                        status: SimStatus::Failed(err.to_string()),
                        elapsed,
                    }
                }));
                match handler_result {
                    Ok(result) => result,
                    Err(_) => {
                        record_double_panic_prevented();
                        #[cfg(any(debug_assertions, feature = "check_invariants"))]
                        {
                            tracing::error!(
                                "CRITICAL: error handling panicked \
                                 (double-panic prevented) in engine::run_dwelling"
                            );
                        }
                        SimulationResults {
                            timeseries_path: None,
                            timeseries: None,
                            metrics: empty_metrics(),
                            warnings: Vec::new(),
                            status: SimStatus::Failed(
                                "panic handling failed (double-panic prevented)".into(),
                            ),
                            elapsed,
                        }
                    }
                }
            }
            Err(payload) => {
                let handler_result = panic::catch_unwind(AssertUnwindSafe(|| SimulationResults {
                    timeseries_path: None,
                    timeseries: None,
                    metrics: empty_metrics(),
                    warnings,
                    status: SimStatus::Failed(panic_hook::panic_payload_to_string(payload)),
                    elapsed,
                }));
                match handler_result {
                    Ok(result) => result,
                    Err(_) => {
                        record_double_panic_prevented();
                        #[cfg(any(debug_assertions, feature = "check_invariants"))]
                        {
                            tracing::error!(
                                "CRITICAL: error handling panicked \
                                 (double-panic prevented) in engine::run_dwelling"
                            );
                        }
                        SimulationResults {
                            timeseries_path: None,
                            timeseries: None,
                            metrics: empty_metrics(),
                            warnings: Vec::new(),
                            status: SimStatus::Failed(
                                "panic handling failed (double-panic prevented)".into(),
                            ),
                            elapsed,
                        }
                    }
                }
            }
        };

        Ok(result)
    }
}

#[cfg(feature = "profiling")]
fn emit_dwelling_profiling_summary(summary: &DwellingProfilingSummary) {
    let total = summary.envelope_solve
        + summary.hvac
        + summary.water_heater
        + summary.schedule_load
        + summary.io
        + summary.other;
    let total_secs = total.as_secs_f64();
    if total_secs <= 0.0 {
        return;
    }

    let pct = |duration: StdDuration| duration.as_secs_f64() * 100.0 / total_secs;
    tracing::info!(
        "envelope_solve: {:.0}% | hvac: {:.0}% | water_heater: {:.0}% | schedule_load: {:.0}% | io: {:.0}% | other: {:.0}%",
        pct(summary.envelope_solve),
        pct(summary.hvac),
        pct(summary.water_heater),
        pct(summary.schedule_load),
        pct(summary.io),
        pct(summary.other),
    );
    tracing::info!(
        memory_high_water_kb = summary.memory_high_water_kb,
        hot_path_alloc_violations = summary.hot_path_alloc_violations,
        "profiling stats"
    );
}

fn validate_input_paths(config: &DwellingConfig) -> Result<(), HaresError> {
    validate_path_exists(&config.hpxml_path, "hpxml_path")?;
    validate_path_exists(&config.weather_path, "weather_path")?;
    Ok(())
}

fn validate_path_exists(path: &Path, field_name: &str) -> Result<(), HaresError> {
    if path.exists() {
        return Ok(());
    }
    Err(HaresError::Io(format!(
        "missing `{field_name}`: {}",
        path.display()
    )))
}

fn resolved_output_path(config: &DwellingConfig) -> Option<PathBuf> {
    if !config.sim_config.write_output {
        return None;
    }
    if let Some(path) = &config.sim_config.output_path {
        return Some(path.clone());
    }
    let ext = match config.sim_config.output_format {
        OutputFormat::Csv => "csv",
        OutputFormat::Parquet => "parquet",
    };
    Some(PathBuf::from(format!(
        "dwelling_{}.{}",
        config.bldg_id, ext
    )))
}

fn compute_metrics_from_batches(
    batches: &[RecordBatch],
    sim_config: &SimulationConfig,
) -> MetricsOutcome {
    debug_assert!(
        !batches.is_empty(),
        "compute_metrics_from_batches called with empty batches"
    );

    let schema = batches[0].schema();
    let time_res_secs = u32::try_from(sim_config.time_res.num_seconds()).unwrap_or(3600);

    let mut calculator = match MetricsCalculator::new(&schema, time_res_secs, sim_config) {
        Ok(calc) => calc,
        Err(err) => {
            return MetricsOutcome {
                metrics: empty_metrics(),
                warning: Some(format!(
                    "MetricsCalculator init failed: {err} -- metrics are zeroed"
                )),
            };
        }
    };

    for batch in batches {
        calculator.accumulate(batch);
    }

    MetricsOutcome {
        metrics: calculator.finish().metrics,
        warning: None,
    }
}

fn empty_metrics() -> SimulationMetrics {
    SimulationMetrics {
        annual_energy_kwh: AnnualEnergyKwh {
            total: 0.0,
            per_end_use: BTreeMap::new(),
        },
        peak_power_kw: PeakPowerKw {
            per_end_use: BTreeMap::new(),
            rolling: RollingPeakKw {
                peak_15min_kw: 0.0,
                peak_30min_kw: 0.0,
                peak_60min_kw: 0.0,
            },
        },
        comfort_hours: None,
        unmet_load_hours: None,
        renewable_energy_fraction: None,
        grid_interaction_metrics: GridInteractionMetrics {
            peak_import_kw: 0.0,
            peak_export_kw: 0.0,
        },
        envelope_loads_kwh: None,
        efficiency: hares_io::EfficiencyMetrics::default(),
    }
}

#[derive(Debug, Default)]
struct RunProfile {
    segments: BTreeMap<&'static str, StdDuration>,
}

impl RunProfile {
    fn record(&mut self, kernel: &'static str, elapsed: StdDuration) {
        self.segments.insert(kernel, elapsed);
    }
}

fn emit_profile_summary(profile: &RunProfile) {
    #[cfg(feature = "profiling")]
    {
        let total = profile
            .segments
            .get(KERNEL_TOTAL)
            .copied()
            .unwrap_or_default()
            .as_secs_f64();
        if total <= 0.0 {
            return;
        }
        let construct_pct = profile
            .segments
            .get(KERNEL_CONSTRUCT_DWELLING)
            .copied()
            .unwrap_or_default()
            .as_secs_f64()
            * 100.0
            / total;
        let simulate_pct = profile
            .segments
            .get(KERNEL_SIMULATE)
            .copied()
            .unwrap_or_default()
            .as_secs_f64()
            * 100.0
            / total;

        tracing::info!(
            "{}: {:.0}% | {}: {:.0}%",
            KERNEL_CONSTRUCT_DWELLING,
            construct_pct,
            KERNEL_SIMULATE,
            simulate_pct
        );
    }
    #[cfg(not(feature = "profiling"))]
    {
        let _ = profile;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::{self, AssertUnwindSafe};

    #[test]
    fn error_handler_double_panic_returns_fallback_simulation_results() {
        // Verify the double-panic guard pattern used in run() and run_dwelling():
        // when the post-catch_unwind error handler itself panics, the nested
        // catch_unwind absorbs it and returns a minimal fallback SimulationResults.
        // This prevents a double-panic from aborting the entire process.
        let elapsed = StdDuration::from_secs(1);
        let fallback_status_msg = "panic handling failed (double-panic prevented)";

        // Outer catch_unwind: simulates catching a simulation panic.
        let outer = panic::catch_unwind(AssertUnwindSafe(|| {
            let sim_payload: Box<dyn std::any::Any + Send> = Box::new("sim panic");
            let warnings: Vec<String> = Vec::new();

            // Inner catch_unwind: guards the error-handling block.
            let handler_result = panic::catch_unwind(AssertUnwindSafe(|| {
                // Access sim_payload to force capture — this is the allocation-
                // heavy path that could panic under allocator corruption.
                let _msg = sim_payload
                    .downcast_ref::<&str>()
                    .unwrap_or(&"fallback")
                    .to_string();
                // Intentionally panic to simulate allocator failure.
                panic!("simulated allocation panic in error handler");
            }));

            match handler_result {
                Ok(result) => result,
                Err(_) => {
                    record_double_panic_prevented();
                    SimulationResults {
                        timeseries_path: None,
                        timeseries: None,
                        metrics: empty_metrics(),
                        warnings: Vec::new(),
                        status: SimStatus::Failed(fallback_status_msg.into()),
                        elapsed,
                    }
                }
            }
        }));

        assert!(outer.is_ok(), "outer catch_unwind should succeed");
        let result = outer.unwrap();
        assert_eq!(
            result.status,
            SimStatus::Failed(fallback_status_msg.into()),
            "fallback status should indicate double-panic was prevented"
        );
        assert!(
            result.warnings.is_empty(),
            "fallback should have no warnings"
        );
        assert_eq!(result.elapsed, elapsed);
        assert!(result.timeseries.is_none());
        assert!(result.timeseries_path.is_none());
    }

    #[test]
    fn error_handler_returns_normal_result_when_no_panic() {
        // Verify the guard does not interfere when the error handler succeeds
        // normally (the common case).
        let elapsed = StdDuration::from_secs(2);
        let warnings = vec!["test warning".to_string()];

        let outer = panic::catch_unwind(AssertUnwindSafe(|| {
            let handler_result = panic::catch_unwind(AssertUnwindSafe(|| SimulationResults {
                timeseries_path: None,
                timeseries: None,
                metrics: empty_metrics(),
                warnings,
                status: SimStatus::Failed("simulation error: test".into()),
                elapsed,
            }));

            match handler_result {
                Ok(result) => result,
                Err(_) => {
                    record_double_panic_prevented();
                    SimulationResults {
                        timeseries_path: None,
                        timeseries: None,
                        metrics: empty_metrics(),
                        warnings: Vec::new(),
                        status: SimStatus::Failed(
                            "panic handling failed (double-panic prevented)".into(),
                        ),
                        elapsed,
                    }
                }
            }
        }));

        assert!(outer.is_ok());
        let result = outer.unwrap();
        assert_eq!(
            result.status,
            SimStatus::Failed("simulation error: test".into())
        );
        assert_eq!(result.warnings, vec!["test warning".to_string()]);
        assert_eq!(result.elapsed, elapsed);
    }
}
