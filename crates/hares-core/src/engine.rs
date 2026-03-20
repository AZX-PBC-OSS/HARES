//! Simulation engine main loop.

use std::any::Any;
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
        let sim_outcome = panic::catch_unwind(AssertUnwindSafe(|| dwelling.simulate()));
        simulation_timer.stop();
        profile.record(KERNEL_SIMULATE, simulation_started.elapsed());

        let elapsed = run_started.elapsed();
        profile.record(KERNEL_TOTAL, elapsed);
        emit_profile_summary(&profile);

        let output_path = resolved_output_path(&config);
        let mut warnings = dwelling.take_warnings();
        let result = match sim_outcome {
            Ok(Ok(dwelling_results)) => {
                let batches = dwelling.flushed_batches().to_vec();
                let metrics = if batches.is_empty() {
                    metrics_from_steps(&dwelling_results.steps, &config.sim_config)
                } else {
                    compute_metrics_from_batches(&batches, &config.sim_config)
                };
                #[cfg(feature = "profiling")]
                emit_dwelling_profiling_summary(&dwelling.profiling_summary());
                let status = if warnings.is_empty() {
                    SimStatus::Ok
                } else {
                    SimStatus::Flagged(format!("{} warning(s)", warnings.len()))
                };
                SimulationResults {
                    timeseries_path: Some(output_path),
                    timeseries: if batches.is_empty() {
                        None
                    } else {
                        Some(batches)
                    },
                    metrics,
                    warnings,
                    status,
                    elapsed,
                }
            }
            Ok(Err(err)) => {
                warnings.push(format!("simulation error: {err}"));
                SimulationResults {
                    timeseries_path: Some(output_path),
                    timeseries: None,
                    metrics: empty_metrics(),
                    warnings,
                    status: SimStatus::Failed(err.to_string()),
                    elapsed,
                }
            }
            Err(payload) => SimulationResults {
                timeseries_path: Some(output_path),
                timeseries: None,
                metrics: empty_metrics(),
                warnings,
                status: SimStatus::Failed(panic_payload_to_string(payload)),
                elapsed,
            },
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
    validate_path_exists(&config.schedule_path, "schedule_path")?;
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

fn resolved_output_path(config: &DwellingConfig) -> PathBuf {
    if let Some(path) = &config.sim_config.output_path {
        return path.clone();
    }
    let ext = match config.sim_config.output_format {
        OutputFormat::Csv => "csv",
        OutputFormat::Parquet => "parquet",
    };
    PathBuf::from(format!("dwelling_{}.{}", config.bldg_id, ext))
}

fn metrics_from_steps(
    steps: &[crate::dwelling::StepResult],
    sim_config: &SimulationConfig,
) -> SimulationMetrics {
    let timestep_h = (sim_config.time_res.num_seconds() as f64 / 3600.0).max(0.0);
    let mut annual_total = 0.0;
    let mut peak_import_kw: f64 = 0.0;
    let mut peak_export_kw: f64 = 0.0;

    for step in steps {
        let power_kw = step.net_electric_power_kw;
        annual_total += power_kw * timestep_h;
        peak_import_kw = peak_import_kw.max(power_kw);
        peak_export_kw = peak_export_kw.max((-power_kw).max(0.0));
    }

    let mut annual_per_end_use = BTreeMap::new();
    annual_per_end_use.insert("total_electric_power_kw".to_string(), annual_total);
    let mut peak_per_end_use = BTreeMap::new();
    peak_per_end_use.insert(
        "total_electric_power_kw".to_string(),
        peak_import_kw.max(0.0),
    );

    SimulationMetrics {
        annual_energy_kwh: AnnualEnergyKwh {
            total: annual_total,
            per_end_use: annual_per_end_use,
        },
        peak_power_kw: PeakPowerKw {
            per_end_use: peak_per_end_use,
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
            peak_import_kw: peak_import_kw.max(0.0),
            peak_export_kw,
        },
    }
}

fn compute_metrics_from_batches(
    batches: &[RecordBatch],
    sim_config: &SimulationConfig,
) -> SimulationMetrics {
    if batches.is_empty() {
        return empty_metrics();
    }

    let schema = batches[0].schema();
    let time_res_secs = u32::try_from(sim_config.time_res.num_seconds()).unwrap_or(3600);

    let mut calculator = match MetricsCalculator::new(&schema, time_res_secs, sim_config) {
        Ok(calc) => calc,
        Err(err) => {
            tracing::warn!(
                "MetricsCalculator init failed: {err}, falling back to step-based metrics"
            );
            return empty_metrics();
        }
    };

    for batch in batches {
        calculator.accumulate(batch);
    }

    calculator.finish().metrics
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
    }
}

fn panic_payload_to_string(payload: Box<dyn Any + Send>) -> String {
    if let Some(msg) = payload.downcast_ref::<&'static str>() {
        return (*msg).to_string();
    }
    if let Some(msg) = payload.downcast_ref::<String>() {
        return msg.clone();
    }
    "simulation panicked with non-string payload".to_string()
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

    #[test]
    fn panic_payload_conversion_handles_string_types() {
        let string_payload = panic_payload_to_string(Box::new("boom"));
        assert_eq!(string_payload, "boom");

        let owned_payload = panic_payload_to_string(Box::new(String::from("owned")));
        assert_eq!(owned_payload, "owned");
    }
}
