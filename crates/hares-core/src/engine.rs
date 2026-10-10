//! Simulation engine main loop.

use std::collections::BTreeMap;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::time::{Duration as StdDuration, Instant};

use arrow::record_batch::RecordBatch;
use hares_io::SimulationConfig;
use hares_io::output::metrics::SimulationMetrics;
use hares_types::HaresError;
use hares_types::panic_hook::{self, PanicHookGuard, record_double_panic_prevented};

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
    /// The first report's published output file path; `None` when no
    /// report declares a file destination.
    pub timeseries_path: Option<PathBuf>,
    /// Optional in-memory timeseries batches (disabled by default).
    pub timeseries: Option<Vec<RecordBatch>>,
    /// The run's metrics, computed per step from a catalogue input plan
    /// when the run requested them. `None` when the run did not request
    /// metrics: it built no metrics input plan and no accumulator, and it
    /// paid nothing for them.
    pub metrics: Option<SimulationMetrics>,
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
        #[cfg(debug_assertions)]
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

        let timeseries_file = first_report_file_path(&config.sim_config);
        let mut warnings = dwelling.take_warnings();
        let result = match sim_outcome {
            Ok(Ok(_dwelling_results)) => {
                let batches = dwelling.flushed_batches().to_vec();
                #[cfg(feature = "profiling")]
                emit_dwelling_profiling_summary(&dwelling.profiling_summary());

                let (metrics, status) = finalize_run_success(&mut dwelling, &mut warnings)?;
                SimulationResults {
                    timeseries_path: timeseries_file.clone(),
                    timeseries: Some(batches),
                    metrics,
                    warnings,
                    status,
                    elapsed,
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
                        timeseries_path: timeseries_file.clone(),
                        timeseries: None,
                        metrics: None,
                        warnings,
                        status: SimStatus::Failed(err.to_string()),
                        elapsed,
                    }
                }));
                match handler_result {
                    Ok(result) => result,
                    Err(_) => {
                        record_double_panic_prevented();
                        tracing::error!(
                            "CRITICAL: error handling panicked \
                             (double-panic prevented) in engine::run"
                        );
                        SimulationResults {
                            timeseries_path: None,
                            timeseries: None,
                            metrics: None,
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
                    timeseries_path: timeseries_file,
                    timeseries: None,
                    metrics: None,
                    warnings,
                    status: SimStatus::Failed(panic_hook::panic_payload_to_string(payload)),
                    elapsed,
                }));
                match handler_result {
                    Ok(result) => result,
                    Err(_) => {
                        record_double_panic_prevented();
                        tracing::error!(
                            "CRITICAL: error handling panicked \
                             (double-panic prevented) in engine::run"
                        );
                        SimulationResults {
                            timeseries_path: None,
                            timeseries: None,
                            metrics: None,
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
        #[cfg(debug_assertions)]
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

                let (metrics, status) = finalize_run_success(dwelling, &mut warnings)?;
                SimulationResults {
                    timeseries_path: sim_config
                        .reports
                        .iter()
                        .find_map(|report| report.destination.path().cloned()),
                    timeseries: Some(batches),
                    metrics,
                    warnings,
                    status,
                    elapsed,
                }
            }
            Ok(Err(err)) => {
                let handler_result = panic::catch_unwind(AssertUnwindSafe(|| {
                    warnings.push(format!("simulation error: {err}"));
                    SimulationResults {
                        timeseries_path: None,
                        timeseries: None,
                        metrics: None,
                        warnings,
                        status: SimStatus::Failed(err.to_string()),
                        elapsed,
                    }
                }));
                match handler_result {
                    Ok(result) => result,
                    Err(_) => {
                        record_double_panic_prevented();
                        tracing::error!(
                            "CRITICAL: error handling panicked \
                             (double-panic prevented) in engine::run_dwelling"
                        );
                        SimulationResults {
                            timeseries_path: None,
                            timeseries: None,
                            metrics: None,
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
                    metrics: None,
                    warnings,
                    status: SimStatus::Failed(panic_hook::panic_payload_to_string(payload)),
                    elapsed,
                }));
                match handler_result {
                    Ok(result) => result,
                    Err(_) => {
                        record_double_panic_prevented();
                        tracing::error!(
                            "CRITICAL: error handling panicked \
                             (double-panic prevented) in engine::run_dwelling"
                        );
                        SimulationResults {
                            timeseries_path: None,
                            timeseries: None,
                            metrics: None,
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
    let total_secs = summary.step_total.as_secs_f64();
    if total_secs <= 0.0 {
        return;
    }

    let pct = |duration: StdDuration| duration.as_secs_f64() * 100.0 / total_secs;
    tracing::info!(
        "environment: {:.0}% | control: {:.0}% | ideal_capacity: {:.0}% | actors: {:.0}% | dispatch: {:.0}% | equipment: {:.0}% | envelope: {:.0}% | invariants: {:.0}% | state_snapshot: {:.0}% | output: {:.0}% | accounting: {:.0}%",
        pct(summary.environment),
        pct(summary.control),
        pct(summary.ideal_capacity),
        pct(summary.actors),
        pct(summary.dispatch),
        pct(summary.equipment),
        pct(summary.envelope),
        pct(summary.invariants),
        pct(summary.state_snapshot),
        pct(summary.output),
        pct(summary.accounting),
    );
    tracing::info!(
        step_total_s = summary.step_total.as_secs_f64(),
        finish_s = ?summary.finish.map(|d| d.as_secs_f64()),
        memory_high_water_kb = ?summary.memory_high_water_kb,
        hot_path_allocations = ?summary.hot_path_allocations,
        hot_path_alloc_violations = ?summary.hot_path_alloc_violations,
        "profiling stats"
    );
    // Per-actor run totals (one entry per registered actor, registration
    // order): each actor's share of the `actors` phase and its decide()
    // call count.
    for timing in &summary.per_actor {
        tracing::info!(
            actor = %timing.name,
            total_s = timing.total.as_secs_f64(),
            calls = timing.calls,
            "per-actor timing"
        );
    }
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

/// The first report's file destination path, in `reports` order: the file
/// the run's output lands at. A run with no file report writes no file.
fn first_report_file_path(config: &SimulationConfig) -> Option<PathBuf> {
    config
        .reports
        .iter()
        .find_map(|report| report.destination.path().cloned())
}

/// The success-path finalization, shared by `run` and `run_dwelling`: the
/// run's metrics from the one path (the dwelling's accumulator, fed per
/// step and finalized at simulate's end), then the status. `None` metrics
/// is the honest result for a run that did not request them: it built no
/// metrics input plan and no accumulator, and it paid nothing for them.
///
/// # Errors
/// A non-finite metrics accumulator is a failed run: the typed error
/// surfaces to the caller.
fn finalize_run_success(
    dwelling: &mut Dwelling,
    warnings: &mut Vec<String>,
) -> Result<(Option<SimulationMetrics>, SimStatus), HaresError> {
    let metrics = match dwelling.take_run_metrics() {
        None => None,
        Some(Ok(full)) => Some(full.metrics),
        Some(Err(err)) => return Err(err),
    };
    // The zero-step defense, unchanged by the metrics decoupling: a
    // recorder that recorded no rows flags the run.
    if dwelling.recorded_rows() == Some(0) {
        let reason = "zero-step simulation: no timesteps recorded";
        warnings.push(reason.to_string());
        return Ok((metrics, SimStatus::Flagged(reason.to_string())));
    }
    let status = if warnings.is_empty() {
        SimStatus::Ok
    } else {
        SimStatus::Flagged(format!("{} warning(s)", warnings.len()))
    };
    Ok((metrics, status))
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
            let _warnings: Vec<String> = Vec::new();

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
                        metrics: None,
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
                metrics: None,
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
                        metrics: None,
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
