//! Simulation engine main loop.

use std::collections::BTreeMap;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration as StdDuration, Instant};

use hares_io::SimulationConfig;
use hares_types::HaresError;
use hares_types::panic_hook::{self, PanicHookGuard, record_double_panic_prevented};

#[cfg(feature = "profiling")]
use crate::dwelling::DwellingProfilingSummary;
use crate::dwelling::{Dwelling, DwellingConfig};
use crate::result::RunResult;

const KERNEL_TOTAL: &str = "total";
const KERNEL_CONSTRUCT_DWELLING: &str = "construct_dwelling";
const KERNEL_SIMULATE: &str = "simulate";

/// Batch-runner entry point for CLI and non-Python workflows.
#[derive(Debug, Default, Clone, Copy)]
pub struct SimulationEngine;

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
    ///
    /// # Errors
    ///
    /// A missing input path, a construction failure, or the run's own
    /// typed error: an `InvariantViolation` from the simulation reaches
    /// the caller as that value, unchanged.
    pub fn run(&self, config: DwellingConfig) -> Result<Arc<RunResult>, HaresError> {
        validate_input_paths(&config)?;

        let run_started = Instant::now();
        let mut profile = RunProfile::default();

        let construction_timer = KernelTimer::start(KERNEL_CONSTRUCT_DWELLING);
        let construction_started = Instant::now();
        let mut dwelling = Dwelling::from_config(config)?;
        construction_timer.stop();
        profile.record(KERNEL_CONSTRUCT_DWELLING, construction_started.elapsed());

        let simulation_timer = KernelTimer::start(KERNEL_SIMULATE);
        let simulation_started = Instant::now();
        let result = self.run_finalisation(&mut dwelling);
        simulation_timer.stop();
        profile.record(KERNEL_SIMULATE, simulation_started.elapsed());

        let ok = result.is_ok();
        if ok {
            #[cfg(feature = "profiling")]
            emit_dwelling_profiling_summary(&dwelling.profiling_summary());
        }

        let elapsed = run_started.elapsed();
        profile.record(KERNEL_TOTAL, elapsed);
        emit_profile_summary(&profile);

        result
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
    ///
    /// # Errors
    ///
    /// The run's own typed error, unchanged: an `InvariantViolation` from
    /// the run reaches the caller as that value. A panic from the
    /// simulation becomes an error carrying the panic's message; the panic
    /// hook records the run's diagnostics first.
    pub fn run_dwelling(
        &self,
        dwelling: &mut Dwelling,
        _sim_config: &SimulationConfig,
    ) -> Result<Arc<RunResult>, HaresError> {
        self.run_finalisation(dwelling)
    }

    /// The one shared finalisation: step the run to its end through
    /// `simulate()` under the panic guard, and return the finished run's
    /// result. Typed errors are returned unchanged; a panic becomes an
    /// error carrying its message.
    fn run_finalisation(&self, dwelling: &mut Dwelling) -> Result<Arc<RunResult>, HaresError> {
        let _guard = PanicHookGuard::new();
        #[cfg(debug_assertions)]
        {
            assert!(
                panic_hook::is_installed(),
                "custom panic hook must be installed before simulation"
            );
        }
        let sim_outcome = panic::catch_unwind(AssertUnwindSafe(|| dwelling.simulate()));
        match sim_outcome {
            Ok(result) => result,
            Err(payload) => {
                // Guard against double-panic: the message construction
                // allocates. If the allocator is corrupted from a prior
                // near-panic, this allocation could panic and abort the
                // process; absorb it and return a minimal error.
                let handler = panic::catch_unwind(AssertUnwindSafe(|| {
                    HaresError::Simulation(format!(
                        "simulation panicked: {}",
                        panic_hook::panic_payload_to_string(payload)
                    ))
                }));
                match handler {
                    Ok(err) => Err(err),
                    Err(_) => {
                        record_double_panic_prevented();
                        tracing::error!(
                            "CRITICAL: error handling panicked \
                             (double-panic prevented) in the engine's run finalisation"
                        );
                        Err(HaresError::Simulation(
                            "simulation panicked (panic handling failed; double-panic prevented)"
                                .to_string(),
                        ))
                    }
                }
            }
        }
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
