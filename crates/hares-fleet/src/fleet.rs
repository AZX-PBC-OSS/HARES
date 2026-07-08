//! Fleet struct and parallel dwelling simulation.

use std::collections::HashMap;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{Duration, FixedOffset, TimeZone};
use hares_core::{
    Dwelling, DwellingConfig, DwellingTelemetry, SimStatus as CoreSimStatus, SimulationEngine,
    SimulationResults, StepResult,
};
use hares_io::{
    OutputFormat, ResStockBuilding, ResStockVersion, SampleWeightClass, SimulationConfig,
    classify_sample_weight, parse_resstock_metadata,
};
use hares_types::ControlSignal;
use hares_types::panic_hook::{self, PanicHookGuard, record_double_panic_prevented};
use rayon::ThreadPoolBuilder;
use rayon::prelude::*;
use thiserror::Error;

const DEFAULT_RESSTOCK_VERSION: ResStockVersion = ResStockVersion::V2025_1;

type ProgressCallback = Arc<dyn Fn(usize, usize) + Send + Sync + 'static>;

/// Result type for fleet construction operations.
pub type Result<T> = std::result::Result<T, FleetError>;

/// Execution status for a single dwelling in fleet mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SimStatus {
    /// Simulation completed with no warnings.
    Ok,
    /// Simulation completed with non-fatal warnings.
    Flagged(String),
    /// Simulation failed to complete.
    Failed(String),
}

/// Fleet-owned simulation payload for one dwelling.
#[derive(Debug, Clone, PartialEq)]
pub struct DwellingOutcome {
    pub result: SimulationResults,
    pub sample_weight: f64,
    pub status: SimStatus,
}

#[derive(Debug, Clone)]
struct FleetEntry {
    config: DwellingConfig,
    sample_weight: f64,
}

/// Error captured when building an individual dwelling for [`SteppableFleet`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DwellingBuildError {
    pub bldg_id: i64,
    pub message: String,
}

/// Errors returned while building a [`Fleet`].
#[derive(Debug, Error)]
pub enum FleetError {
    #[error("resstock metadata parse failed: {0}")]
    ResStock(String),
    #[error("from_configs requires at least one dwelling config")]
    EmptySteppableFleetConfig,
    #[error("all dwellings failed to initialize ({count} failure(s))")]
    AllSteppableDwellingsFailed { count: usize },
    #[error("failed to build local rayon thread pool: {0}")]
    ThreadPoolBuild(String),
    #[error("fleet has no dwelling with positive sample_weight")]
    ZeroWeightFleet,
    #[error("sample_weights length ({provided}) must match fleet size ({expected})")]
    SampleWeightLengthMismatch { expected: usize, provided: usize },
    #[error(
        "dwelling {bldg_id} has invalid sample_weight {value} (must be finite and non-negative)"
    )]
    InvalidSampleWeight { bldg_id: i64, value: f64 },
    #[error(
        "dwelling at index {index} has invalid sample_weight {value} (must be finite and non-negative)"
    )]
    InvalidAggregationWeight { index: usize, value: f64 },
}

/// Errors returned by [`Fleet::simulate`].
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SimError {
    #[error("failed to build local rayon thread pool: {0}")]
    ThreadPoolBuild(String),
    #[error("dwelling {bldg_id} failed: {message}")]
    Failed { bldg_id: i64, message: String },
    #[error("dwelling {bldg_id} engine error: {message}")]
    Engine { bldg_id: i64, message: String },
    #[error("dwelling {bldg_id} panicked: {message}")]
    Panic { bldg_id: i64, message: String },
    #[error("dwelling {bldg_id} skipped due to prior failure: {message}")]
    Skipped { bldg_id: i64, message: String },
}

/// Fleet runner for parallel dwelling simulation.
#[derive(Clone)]
pub struct Fleet {
    entries: Vec<FleetEntry>,
    progress: Option<ProgressCallback>,
}

impl std::fmt::Debug for Fleet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fleet")
            .field("entries", &self.entries)
            .field("progress", &self.progress.as_ref().map(|_| "..."))
            .finish()
    }
}

impl Fleet {
    /// Constructs a fleet from explicit dwelling configs.
    #[must_use]
    pub fn from_buildings(configs: Vec<DwellingConfig>) -> Self {
        let entries = configs
            .into_iter()
            .map(|config| FleetEntry {
                config,
                sample_weight: 1.0,
            })
            .collect();
        Self {
            entries,
            progress: None,
        }
    }

    /// Constructs a fleet from ResStock metadata and local data directories.
    ///
    /// `resstock_version` defaults to the latest known schema when `None`.
    pub fn from_resstock(
        metadata_path: &Path,
        hpxml_dir: &Path,
        weather_dir: &Path,
        resstock_version: Option<ResStockVersion>,
        filter: Option<HashMap<String, String>>,
    ) -> Result<Self> {
        let version = resstock_version.unwrap_or(DEFAULT_RESSTOCK_VERSION);
        let buildings = parse_resstock_metadata(metadata_path, version, hpxml_dir)
            .map_err(|err| FleetError::ResStock(err.to_string()))?;

        let mut resolved = 0usize;
        let mut unresolved = 0usize;

        let entries: Vec<FleetEntry> = buildings
            .into_iter()
            .filter(|building| matches_filter(&building.characteristics, filter.as_ref()))
            .map(|building| {
                let weather_path =
                    resolve_weather_path_for_building(&building, weather_dir, version);

                match &weather_path {
                    Some(path) if path.exists() && path_is_readable_weather_file(path) => {
                        resolved += 1;
                    }
                    Some(_) => {
                        unresolved += 1;
                        tracing::warn!(
                            bldg_id = building.bldg_id,
                            fips = building.weather_fips.as_deref().unwrap_or("none"),
                            "weather file not found or not readable for building"
                        );
                    }
                    None => {
                        unresolved += 1;
                        tracing::error!(
                            bldg_id = building.bldg_id,
                            "no weather station FIPS code; cannot resolve weather file"
                        );
                    }
                }

                let weather_path = weather_path.unwrap_or_else(|| PathBuf::from(""));
                validate_fleet_building_zone(&building.hpxml_path, &weather_path, building.bldg_id);
                FleetEntry {
                    config: DwellingConfig {
                        hpxml_path: building.hpxml_path,
                        schedule_path: building.schedule_path,
                        weather_path,
                        defaults_path: None,
                        sim_config: default_resstock_sim_config(),
                        overrides: None,
                        bldg_id: building.bldg_id,
                        initialization_duration: Some(std::time::Duration::from_secs(
                            7 * 24 * 3600,
                        )),
                        resample_overrides: None,
                        patches: Some(hares_io::HpxmlDataPatches::from_resstock_characteristics(
                            &building.characteristics,
                        )),
                    },
                    sample_weight: building.sample_weight,
                }
            })
            .collect();

        tracing::info!(
            total = entries.len(),
            resolved,
            unresolved,
            "ResStock fleet: {} buildings with resolved weather paths, {} unresolved",
            resolved,
            unresolved,
        );

        Ok(Self {
            entries,
            progress: None,
        })
    }

    /// Installs a fleet progress callback.
    ///
    /// The callback is called as `(completed, total)` from worker threads.
    ///
    /// Python bindings should reacquire the GIL inside this callback (for example,
    /// using `Python::with_gil`) before touching Python objects like `tqdm`.
    #[must_use]
    pub fn with_progress(mut self, cb: impl Fn(usize, usize) + Send + Sync + 'static) -> Self {
        self.progress = Some(Arc::new(cb));
        self
    }

    /// Sets a fleet progress callback without consuming self.
    pub fn set_progress(&mut self, cb: impl Fn(usize, usize) + Send + Sync + 'static) {
        self.progress = Some(Arc::new(cb));
    }

    /// Patches sample weights for all fleet entries.
    ///
    /// Each weight is validated with the same rule the ResStock ingestion path
    /// uses ([`hares_io::classify_sample_weight`]): a NaN, infinite, or negative
    /// weight is rejected because it would silently corrupt fleet-level weighted
    /// aggregation, and a zero weight is accepted with a warning (it contributes
    /// nothing but the dwelling may still be useful for standalone simulation).
    ///
    /// # Errors
    ///
    /// Returns [`FleetError::SampleWeightLengthMismatch`] if `weights.len()`
    /// does not match the number of entries, or
    /// [`FleetError::InvalidSampleWeight`] if any weight is non-finite or
    /// negative.
    pub fn with_sample_weights(mut self, weights: Vec<f64>) -> Result<Self> {
        if weights.len() != self.entries.len() {
            return Err(FleetError::SampleWeightLengthMismatch {
                expected: self.entries.len(),
                provided: weights.len(),
            });
        }
        for (entry, weight) in self.entries.iter_mut().zip(weights) {
            match classify_sample_weight(weight) {
                SampleWeightClass::Invalid => {
                    tracing::error!(
                        bldg_id = entry.config.bldg_id,
                        sample_weight = weight,
                        "dwelling has invalid sample_weight (NaN, infinite, or negative)"
                    );
                    return Err(FleetError::InvalidSampleWeight {
                        bldg_id: entry.config.bldg_id,
                        value: weight,
                    });
                }
                SampleWeightClass::Zero => {
                    tracing::warn!(
                        bldg_id = entry.config.bldg_id,
                        "dwelling has zero sample_weight"
                    );
                }
                SampleWeightClass::Positive => {}
            }
            entry.sample_weight = weight;
        }
        Ok(self)
    }

    /// Returns the number of dwellings in the fleet.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns true if the fleet has no dwellings.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Simulates all dwellings in parallel.
    ///
    /// When `n_threads > 0`, a local Rayon pool is created for this call. When
    /// `n_threads == 0`, the global Rayon pool is used.
    #[must_use]
    pub fn simulate(
        &self,
        n_threads: usize,
    ) -> Vec<std::result::Result<DwellingOutcome, SimError>> {
        if n_threads == 0 {
            return self.simulate_parallel();
        }

        match ThreadPoolBuilder::new().num_threads(n_threads).build() {
            Ok(pool) => pool.install(|| self.simulate_parallel()),
            Err(err) => vec![Err(SimError::ThreadPoolBuild(err.to_string())); self.entries.len()],
        }
    }

    fn simulate_parallel(&self) -> Vec<std::result::Result<DwellingOutcome, SimError>> {
        let total = self.entries.len();
        let completed = AtomicUsize::new(0);
        let progress = self.progress.clone();

        // Outer guard ensures the custom hook is installed before any worker
        // starts. Each worker's engine.run() installs its own guard as well,
        // but this one covers the gap before worker guards are constructed and
        // serves as defense-in-depth in case a worker path skips the guard.
        let _guard = PanicHookGuard::new();
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            assert!(
                panic_hook::is_installed(),
                "custom panic hook must be installed before fleet simulation"
            );
        }

        self.entries
            .par_iter()
            .map(|entry| {
                let outcome = match panic::catch_unwind(AssertUnwindSafe(|| {
                    let result = run_entry(entry);
                    if let Some(cb) = &progress {
                        let done = completed.fetch_add(1, Ordering::Relaxed) + 1;
                        cb(done, total);
                    }
                    result
                })) {
                    Ok(result) => result,
                    Err(payload) => {
                        // Increment progress even on panic so the bar reaches 100%.
                        if progress.is_some() {
                            completed.fetch_add(1, Ordering::Relaxed);
                        }
                        // Guard against double-panic: SimError::Panic construction
                        // and panic_payload_to_string both involve String allocation.
                        // If the allocator is corrupted from a prior near-panic, these
                        // could panic and abort the process.
                        let handler_result = panic::catch_unwind(AssertUnwindSafe(|| {
                            Err(SimError::Panic {
                                bldg_id: entry.config.bldg_id,
                                message: panic_hook::panic_payload_to_string(payload),
                            })
                        }));
                        match handler_result {
                            Ok(result) => result,
                            Err(_) => {
                                record_double_panic_prevented();
                                #[cfg(any(debug_assertions, feature = "check_invariants"))]
                                {
                                    tracing::error!(
                                        bldg_id = entry.config.bldg_id,
                                        "CRITICAL: error handling panicked \
                                         (double-panic prevented) in fleet::simulate_parallel"
                                    );
                                }
                                Err(SimError::Panic {
                                    bldg_id: entry.config.bldg_id,
                                    message: "panic handling failed (double-panic prevented)"
                                        .into(),
                                })
                            }
                        }
                    }
                };

                if let Err(err) = &outcome {
                    // Guard tracing::warn! against Display panics. The %err
                    // formatting calls SimError::fmt which could panic if an
                    // inner Display implementation is unexpectedly fallible.
                    let _ = panic::catch_unwind(AssertUnwindSafe(|| {
                        tracing::warn!(
                            bldg_id = entry.config.bldg_id,
                            error = %err,
                            "dwelling simulation failed"
                        );
                    }));
                }

                outcome
            })
            .collect()
    }
}

/// Fleet runner that owns initialized dwellings and advances one timestep at a time.
pub struct SteppableFleet {
    dwellings: Vec<Dwelling>,
    step_pool: Option<rayon::ThreadPool>,
    time_res_s: f64,
    total_steps: u64,
    current_step: u64,
}

impl std::fmt::Debug for SteppableFleet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SteppableFleet")
            .field("dwellings", &self.dwellings.len())
            .field("step_pool", &self.step_pool.as_ref().map(|_| "..."))
            .field("time_res_s", &self.time_res_s)
            .field("total_steps", &self.total_steps)
            .field("current_step", &self.current_step)
            .finish()
    }
}

impl SteppableFleet {
    /// Builds and initializes a steppable fleet from dwelling configs.
    ///
    /// Returns partial success as `(fleet, errors)`. If every dwelling fails,
    /// returns `Err`.
    pub fn from_configs(
        configs: Vec<DwellingConfig>,
        n_threads: usize,
    ) -> Result<(Self, Vec<DwellingBuildError>)> {
        if configs.is_empty() {
            return Err(FleetError::EmptySteppableFleetConfig);
        }

        let step_pool = if n_threads > 0 {
            Some(
                ThreadPoolBuilder::new()
                    .num_threads(n_threads)
                    .build()
                    .map_err(|err| FleetError::ThreadPoolBuild(err.to_string()))?,
            )
        } else {
            None
        };

        let mut dwellings = Vec::with_capacity(configs.len());
        let mut build_errors = Vec::new();

        let _guard = PanicHookGuard::new();
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            assert!(
                panic_hook::is_installed(),
                "custom panic hook must be installed before dwelling construction"
            );
        }

        for config in configs {
            let bldg_id = config.bldg_id;
            let build_result =
                panic::catch_unwind(AssertUnwindSafe(|| Dwelling::from_config(config)));
            match build_result {
                Ok(Ok(dwelling)) => dwellings.push(dwelling),
                Ok(Err(err)) => build_errors.push(DwellingBuildError {
                    bldg_id,
                    message: err.to_string(),
                }),
                Err(payload) => {
                    let handler_result =
                        panic::catch_unwind(AssertUnwindSafe(|| DwellingBuildError {
                            bldg_id,
                            message: panic_hook::panic_payload_to_string(payload),
                        }));
                    match handler_result {
                        Ok(err) => build_errors.push(err),
                        Err(_) => {
                            record_double_panic_prevented();
                            #[cfg(any(debug_assertions, feature = "check_invariants"))]
                            {
                                tracing::error!(
                                    bldg_id,
                                    "CRITICAL: error handling panicked \
                                     (double-panic prevented) in fleet::SteppableFleet::from_configs"
                                );
                            }
                            build_errors.push(DwellingBuildError {
                                bldg_id,
                                message: "panic handling failed (double-panic prevented)".into(),
                            });
                        }
                    }
                }
            }
        }

        if dwellings.is_empty() {
            return Err(FleetError::AllSteppableDwellingsFailed {
                count: build_errors.len(),
            });
        }

        let first = &dwellings[0].clock;
        let total_steps = first.total_steps();
        let time_res_s = first.time_res.num_milliseconds() as f64 / 1000.0;

        // Remove dwellings whose timing config doesn't match the first.
        let mut i = 1;
        while i < dwellings.len() {
            let other_steps = dwellings[i].clock.total_steps();
            let other_res = dwellings[i].clock.time_res.num_milliseconds() as f64 / 1000.0;
            let steps_match = other_steps == total_steps;
            let res_match = (other_res - time_res_s).abs() <= f64::EPSILON;
            if !steps_match || !res_match {
                let removed = dwellings.remove(i);
                let reason = if !steps_match && !res_match {
                    format!(
                        "total_steps={other_steps} and time_res={other_res}s differ from dwelling 0 ({total_steps}, {time_res_s}s)",
                    )
                } else if !steps_match {
                    format!(
                        "total_steps={other_steps} differs from dwelling 0 total_steps={total_steps}",
                    )
                } else {
                    format!("time_res={other_res}s differs from dwelling 0 time_res={time_res_s}s",)
                };
                build_errors.push(DwellingBuildError {
                    bldg_id: removed.bldg_id,
                    message: reason,
                });
            } else {
                i += 1;
            }
        }

        if dwellings.is_empty() {
            return Err(FleetError::AllSteppableDwellingsFailed {
                count: build_errors.len(),
            });
        }

        Ok((
            Self {
                dwellings,
                step_pool,
                time_res_s,
                total_steps,
                current_step: 0,
            },
            build_errors,
        ))
    }

    /// Advances all dwellings exactly one timestep in parallel.
    #[must_use]
    pub fn step(&mut self) -> Vec<std::result::Result<StepResult, SimError>> {
        if self.is_finished() {
            return self
                .dwellings
                .iter()
                .map(|dwelling| {
                    Err(SimError::Failed {
                        bldg_id: dwelling.bldg_id,
                        message: "simulation already reached configured end".to_string(),
                    })
                })
                .collect();
        }

        // Invariant: no dwelling that was ALREADY failed before this step had
        // step() called on it.  Capture pre-step failed state so that dwellings
        // that become failed during this step (via a panic caught in
        // step_dwellings_parallel) are not asserted — they were not failed when
        // step() was called, and a Panic result is valid for them.
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        let was_already_failed: Vec<bool> = self.dwellings.iter().map(|d| d.failed).collect();

        let results = if let Some(pool) = &self.step_pool {
            pool.install(|| step_dwellings_parallel(&mut self.dwellings))
        } else {
            step_dwellings_parallel(&mut self.dwellings)
        };

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            for (i, (dwelling, result)) in self.dwellings.iter().zip(results.iter()).enumerate() {
                if was_already_failed[i] {
                    assert!(
                        matches!(result, Err(SimError::Skipped { .. })),
                        "invariant violation: dwelling {} was already marked failed before step but result was {:?}",
                        dwelling.bldg_id,
                        result
                    );
                }
            }
        }

        self.current_step = self.current_step.saturating_add(1);
        results
    }

    /// Applies a grid voltage override to one dwelling.
    pub fn set_grid_voltage(&mut self, dwelling_index: usize, voltage_pu: f64) {
        if let Some(dwelling) = self.dwellings.get_mut(dwelling_index) {
            dwelling.set_grid_voltage(voltage_pu);
        } else {
            tracing::warn!(dwelling_index, "set_grid_voltage index out of bounds");
        }
    }

    /// Applies a shared grid voltage override to all dwellings.
    pub fn set_grid_voltage_all(&mut self, voltage_pu: f64) {
        for dwelling in &mut self.dwellings {
            dwelling.set_grid_voltage(voltage_pu);
        }
    }

    /// Queues a control signal for one dwelling by equipment name.
    pub fn apply_control(&mut self, dwelling_index: usize, name: &str, signal: ControlSignal) {
        if let Some(dwelling) = self.dwellings.get_mut(dwelling_index) {
            dwelling.apply_control(name, signal);
        } else {
            tracing::warn!(dwelling_index, "apply_control index out of bounds");
        }
    }

    /// Returns telemetry for one dwelling, or `None` if the index is out of bounds.
    #[must_use]
    pub fn telemetry(&self, dwelling_index: usize) -> Option<DwellingTelemetry> {
        self.dwellings.get(dwelling_index).map(|d| d.telemetry())
    }

    /// Returns building id for one dwelling index.
    #[must_use]
    pub fn bldg_id(&self, dwelling_index: usize) -> Option<i64> {
        self.dwellings.get(dwelling_index).map(|d| d.bldg_id)
    }

    /// Returns fleet dwelling count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.dwellings.len()
    }

    /// Returns true when the fleet has no dwellings.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.dwellings.is_empty()
    }

    /// Returns true after all configured timesteps have been stepped.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.current_step >= self.total_steps
    }

    /// Returns timestep resolution in seconds.
    #[must_use]
    pub fn time_res_s(&self) -> f64 {
        self.time_res_s
    }

    /// Returns total simulation timesteps.
    #[must_use]
    pub fn total_steps(&self) -> u64 {
        self.total_steps
    }

    /// Returns current global step index (0-based).
    #[must_use]
    pub fn current_step(&self) -> u64 {
        self.current_step
    }
}

fn run_entry(entry: &FleetEntry) -> std::result::Result<DwellingOutcome, SimError> {
    let engine = SimulationEngine::new();
    match engine.run(entry.config.clone()) {
        Ok(result) => {
            let status = map_status(&result.status);
            if let SimStatus::Failed(message) = &status {
                return Err(SimError::Failed {
                    bldg_id: entry.config.bldg_id,
                    message: message.clone(),
                });
            }

            Ok(DwellingOutcome {
                result,
                sample_weight: entry.sample_weight,
                status,
            })
        }
        Err(err) => Err(SimError::Engine {
            bldg_id: entry.config.bldg_id,
            message: err.to_string(),
        }),
    }
}

fn step_dwellings_parallel(
    dwellings: &mut [Dwelling],
) -> Vec<std::result::Result<StepResult, SimError>> {
    let _guard = PanicHookGuard::new();
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    {
        assert!(
            panic_hook::is_installed(),
            "custom panic hook must be installed before dwelling stepping"
        );
    }

    dwellings
        .par_iter_mut()
        .map(|dwelling| {
            if dwelling.failed {
                return Err(SimError::Skipped {
                    bldg_id: dwelling.bldg_id,
                    message: "dwelling permanently failed after prior panic".to_string(),
                });
            }
            match panic::catch_unwind(AssertUnwindSafe(|| dwelling.step())) {
                Ok(Ok(step_result)) => Ok(step_result),
                Ok(Err(err)) => Err(SimError::Engine {
                    bldg_id: dwelling.bldg_id,
                    message: err.to_string(),
                }),
                Err(payload) => {
                    dwelling.failed = true;
                    let handler_result = panic::catch_unwind(AssertUnwindSafe(|| {
                        Err(SimError::Panic {
                            bldg_id: dwelling.bldg_id,
                            message: panic_hook::panic_payload_to_string(payload),
                        })
                    }));
                    match handler_result {
                        Ok(result) => result,
                        Err(_) => {
                            record_double_panic_prevented();
                            #[cfg(any(debug_assertions, feature = "check_invariants"))]
                            {
                                tracing::error!(
                                    bldg_id = dwelling.bldg_id,
                                    "CRITICAL: error handling panicked \
                                     (double-panic prevented) in fleet::step_dwellings_parallel"
                                );
                            }
                            Err(SimError::Panic {
                                bldg_id: dwelling.bldg_id,
                                message: "panic handling failed (double-panic prevented)".into(),
                            })
                        }
                    }
                }
            }
        })
        .collect()
}

fn map_status(status: &CoreSimStatus) -> SimStatus {
    match status {
        CoreSimStatus::Ok => SimStatus::Ok,
        CoreSimStatus::Flagged(message) => SimStatus::Flagged(message.clone()),
        CoreSimStatus::Failed(message) => SimStatus::Failed(message.clone()),
    }
}

fn default_resstock_sim_config() -> SimulationConfig {
    SimulationConfig {
        start_time: FixedOffset::east_opt(0)
            .expect("valid UTC offset")
            .with_ymd_and_hms(2019, 1, 1, 0, 0, 0)
            .single()
            .expect("valid constant simulation start timestamp"),
        duration: Duration::hours(24),
        time_res: Duration::minutes(1),
        output_verbosity: 0,
        output_path: None,
        write_output: false,
        output_format: OutputFormat::Csv,
        output_chunk_size: 10_000,
        setpoint_deadband_c: None,
        master_seed: 0,
        civil_timezone: None,
        site_location: hares_io::SiteLocationOverride::default(),
        retain_batches: true,
        rotation: hares_io::RotationPolicy::None,
    }
}

fn matches_filter(
    characteristics: &HashMap<String, String>,
    filter: Option<&HashMap<String, String>>,
) -> bool {
    filter.is_none_or(|criteria| {
        criteria.iter().all(|(key, expected)| {
            characteristics
                .get(key)
                .is_some_and(|actual| actual == expected)
        })
    })
}

/// Returns the weather filename for a building given its FIPS code and ResStock version.
///
/// Convention derived from the ResStock dataset and the Python `_fetch_weather` reference:
/// - 2024.x (TMY3): `{fips}.epw` — the Python `_VersionConfig` defaults to
///   `WeatherFormat.EPW` for TMY3 releases, producing EPW files via `get_epw_for_fips`.
/// - 2025.1 (AMY 2018): `{fips}_2018.csv` — the Python `_VersionConfig` explicitly sets
///   `weather_format=WeatherFormat.CSV` for this release.
fn weather_filename_for_building(fips: &str, version: ResStockVersion) -> String {
    match version {
        ResStockVersion::V2024_1 | ResStockVersion::V2024_2 => format!("{fips}.epw"),
        ResStockVersion::V2025_1 => format!("{fips}_2018.csv"),
    }
}

/// Resolve the weather file path for a ResStock building using its FIPS code.
///
/// Constructs a path of the form `weather_dir/FILENAME` where FILENAME is
/// version-dependent (e.g. `G0800130.epw` for 2024.x, `G0800130_2018.csv`
/// for 2025.1).
///
/// Returns `None` if the building has no FIPS code.
fn resolve_weather_path_for_building(
    building: &ResStockBuilding,
    weather_dir: &Path,
    version: ResStockVersion,
) -> Option<PathBuf> {
    let fips = building.weather_fips.as_ref()?;
    tracing::debug!(
        bldg_id = building.bldg_id,
        fips = fips.as_str(),
        "extracted FIPS code from weather station"
    );
    let filename = weather_filename_for_building(fips, version);
    Some(weather_dir.join(filename))
}

/// Check whether a path points to a readable weather file (not a ZIP).
fn path_is_readable_weather_file(path: &Path) -> bool {
    let Some(ext) = path.extension() else {
        return false;
    };
    if ext.eq_ignore_ascii_case("zip") {
        return false;
    }
    path.exists() && !path.is_dir()
}

/// IECC climate zone number ranges valid for each US state.
///
/// Source: IECC 2021 climate zone map (ASHRAE 169-2021 Table B-1).
/// Used for coarse cross-zone validation between building HPXML and weather EPW.
fn iecc_state_zones(state: &str) -> &[u8] {
    match state {
        "AL" => &[2, 3],
        "AK" => &[7, 8],
        "AZ" => &[2, 3, 4, 5],
        "AR" => &[3, 4],
        "CA" => &[2, 3, 4, 5, 6],
        "CO" => &[4, 5, 6, 7],
        "CT" => &[5],
        "DE" => &[4],
        "FL" => &[1, 2],
        "GA" => &[2, 3, 4],
        "HI" => &[1],
        "ID" => &[5, 6],
        "IL" => &[4, 5],
        "IN" => &[4, 5],
        "IA" => &[5, 6],
        "KS" => &[3, 4, 5],
        "KY" => &[4],
        "LA" => &[2, 3],
        "ME" => &[6, 7],
        "MD" => &[4],
        "MA" => &[5],
        "MI" => &[5, 6, 7],
        "MN" => &[6, 7],
        "MS" => &[2, 3],
        "MO" => &[4, 5],
        "MT" => &[6, 7],
        "NE" => &[5, 6],
        "NV" => &[3, 4, 5],
        "NH" => &[5, 6],
        "NJ" => &[4, 5],
        "NM" => &[3, 4, 5],
        "NY" => &[4, 5, 6],
        "NC" => &[3, 4, 5],
        "ND" => &[6, 7],
        "OH" => &[4, 5],
        "OK" => &[3, 4],
        "OR" => &[4, 5],
        "PA" => &[4, 5],
        "RI" => &[5],
        "SC" => &[2, 3],
        "SD" => &[5, 6],
        "TN" => &[3, 4],
        "TX" => &[1, 2, 3, 4],
        "UT" => &[5, 6, 7],
        "VT" => &[5, 6],
        "VA" => &[3, 4, 5],
        "WA" => &[4, 5, 6],
        "WV" => &[4, 5],
        "WI" => &[6, 7],
        "WY" => &[6, 7],
        "DC" => &[4],
        "PR" => &[1],
        _ => &[],
    }
}

/// Outcome of a climate-zone-to-weather-file validation check.
#[derive(Debug, PartialEq, Eq)]
enum ZoneMatchStatus {
    /// Building zone matches weather file location.
    Match {
        building_zone: String,
        weather_state: String,
    },
    /// Building zone does not match weather file location.
    Mismatch {
        building_zone: String,
        weather_state: String,
    },
    /// Validation was skipped (missing zone, missing state, unknown state,
    /// or non-EPW weather file).
    Skipped,
}

/// Validate that a building's IECC climate zone is compatible with the weather file's location.
///
/// Extracts the building's IECC zone from HPXML and the weather file's state from
/// the EPW LOCATION header. Returns a ``ZoneMatchStatus`` indicating the outcome,
/// and logs an `ERROR` via `tracing` when the zone is not valid for the weather
/// file's state.
fn validate_fleet_building_zone(
    hpxml_path: &Path,
    weather_path: &Path,
    bldg_id: i64,
) -> ZoneMatchStatus {
    let Some(building_zone) = hares_io::parse_iecc_climate_zone(hpxml_path) else {
        return ZoneMatchStatus::Skipped;
    };

    let Some(epw_state) = hares_io::parse_epw_location_state(weather_path) else {
        return ZoneMatchStatus::Skipped;
    };

    let valid_zones = iecc_state_zones(&epw_state);
    if valid_zones.is_empty() {
        return ZoneMatchStatus::Skipped;
    }

    let Some(zone_number) = building_zone.chars().next().and_then(|c| c.to_digit(10)) else {
        return ZoneMatchStatus::Skipped;
    };
    let zone_number = zone_number as u8;

    tracing::debug!(
        bldg_id = bldg_id,
        building_zone = %building_zone,
        "extracted IECC climate zone from building HPXML"
    );

    if valid_zones.contains(&zone_number) {
        #[cfg(feature = "observe")]
        {
            tracing::debug!(
                target: "observe",
                building_id = bldg_id,
                expected_zone = %building_zone,
                actual_zone = %epw_state,
                match_status = "match",
                "climate zone validated"
            );
        }
        return ZoneMatchStatus::Match {
            building_zone,
            weather_state: epw_state,
        };
    }

    tracing::error!(
        bldg_id = bldg_id,
        building_zone = %building_zone,
        weather_state = %epw_state,
        valid_state_zones = ?valid_zones,
        "building IECC climate zone does not match weather file location; \
         cross-zone pairings produce physically invalid results"
    );

    #[cfg(feature = "observe")]
    {
        tracing::debug!(
            target: "observe",
            building_id = bldg_id,
            expected_zone = %building_zone,
            actual_zone = %epw_state,
            match_status = "mismatch",
            "climate zone mismatch detected"
        );
    }

    ZoneMatchStatus::Mismatch {
        building_zone,
        weather_state: epw_state,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_path(suffix: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before UNIX epoch")
            .as_nanos();
        path.push(format!("hares-fleet-{nanos}.{suffix}"));
        path
    }

    fn write_temp_file(path: &Path, contents: &str) {
        fs::write(path, contents).expect("failed to write temp file");
    }

    fn build_schedule_csv() -> String {
        [
            "Time,Clothes Washer (kW),HVAC Heating (C)",
            "2021-01-01T00:00:00-07:00,0.10,20.0",
            "2021-01-01T00:15:00-07:00,0.20,20.5",
            "2021-01-01T00:30:00-07:00,0.30,21.0",
            "2021-01-01T00:45:00-07:00,0.40,21.5",
        ]
        .join("\n")
    }

    fn build_epw_8760() -> String {
        use chrono::{Datelike, Duration, NaiveDate, Timelike};

        let mut lines = vec![
            "LOCATION,Test Site,CO,USA,TMY3,999999,39.74,-104.99,-7.0,1609.3".to_string(),
            "DESIGN CONDITIONS,0".to_string(),
            "GROUND TEMPERATURES,0".to_string(),
            "TYPICAL/EXTREME PERIODS,0".to_string(),
            "HOLIDAYS/DAYLIGHT SAVINGS,No,0,0,0".to_string(),
            "COMMENTS 1,synthetic".to_string(),
            "COMMENTS 2,synthetic".to_string(),
            "DATA PERIODS,1,1,Data,Sunday, 1/ 1,12/31".to_string(),
        ];

        let start_date = NaiveDate::from_ymd_opt(2021, 1, 1).expect("valid date");
        let start_time = start_date.and_hms_opt(0, 0, 0).expect("valid time");
        for i in 0..8760 {
            let timestamp = start_time + Duration::hours(i as i64);
            let year = timestamp.year();
            let month = timestamp.month();
            let day = timestamp.day();
            let hour = timestamp.hour() + 1;

            let row = [
                year.to_string(),
                month.to_string(),
                day.to_string(),
                hour.to_string(),
                "0".to_string(),
                "A0A0A0A0*0*0*0*0*0*0*0*0*0*0".to_string(),
                "20.0".to_string(),
                "10.0".to_string(),
                "50".to_string(),
                "101325".to_string(),
                "0".to_string(),
                "0".to_string(),
                "300".to_string(),
                "100".to_string(),
                "200".to_string(),
                "50".to_string(),
                "0".to_string(),
                "0".to_string(),
                "0".to_string(),
                "0".to_string(),
                "180".to_string(),
                "3.5".to_string(),
                "4".to_string(),
                "4".to_string(),
                "0".to_string(),
                "0".to_string(),
                "0".to_string(),
                "0".to_string(),
                "0".to_string(),
                "0".to_string(),
                "0".to_string(),
                "0".to_string(),
                "0".to_string(),
                "0".to_string(),
                "0".to_string(),
            ];
            lines.push(row.join(","));
        }

        lines.join("\n")
    }

    fn fixture_hpxml_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/hpxml/ochre_samples/base.xml")
    }

    fn simulation_config(output_path: PathBuf) -> SimulationConfig {
        SimulationConfig {
            start_time: chrono::Utc::now().fixed_offset(),
            duration: Duration::hours(1),
            time_res: Duration::minutes(1),
            output_verbosity: 0,
            output_path: Some(output_path),
            write_output: true,
            output_format: OutputFormat::Csv,
            output_chunk_size: 128,
            setpoint_deadband_c: None,
            master_seed: 0,
            civil_timezone: None,
            site_location: hares_io::SiteLocationOverride::default(),
            retain_batches: false,
            rotation: hares_io::RotationPolicy::None,
        }
    }

    fn build_valid_configs(count: usize) -> Vec<DwellingConfig> {
        let schedule_path = unique_temp_path("csv");
        let weather_path = unique_temp_path("epw");
        write_temp_file(&schedule_path, &build_schedule_csv());
        write_temp_file(&weather_path, &build_epw_8760());

        (0..count)
            .map(|idx| DwellingConfig {
                hpxml_path: fixture_hpxml_path(),
                schedule_path: schedule_path.clone(),
                weather_path: weather_path.clone(),
                sim_config: simulation_config(unique_temp_path("csv")),
                defaults_path: None,
                overrides: None,
                bldg_id: idx as i64 + 1,
                initialization_duration: None,
                resample_overrides: None,
                patches: None,
            })
            .collect()
    }

    fn build_missing_configs(count: usize) -> Vec<DwellingConfig> {
        (0..count)
            .map(|idx| DwellingConfig {
                hpxml_path: PathBuf::from("/tmp/missing-hpxml.xml"),
                schedule_path: PathBuf::from("/tmp/missing-schedule.csv"),
                weather_path: PathBuf::from("/tmp/missing-weather.epw"),
                sim_config: simulation_config(unique_temp_path("csv")),
                defaults_path: None,
                overrides: None,
                bldg_id: idx as i64 + 1,
                initialization_duration: None,
                resample_overrides: None,
                patches: None,
            })
            .collect()
    }

    #[test]
    fn simulate_runs_three_dwellings() {
        let fleet = Fleet::from_buildings(build_valid_configs(3));
        let results = fleet.simulate(1);

        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|entry| entry.is_ok()));

        for entry in results {
            let outcome = entry.expect("successful outcome");
            assert!(matches!(
                outcome.status,
                SimStatus::Ok | SimStatus::Flagged(_)
            ));
            assert!(outcome.sample_weight > 0.0);
        }
    }

    #[test]
    fn with_sample_weights_accepts_zero_and_positive() {
        let fleet = Fleet::from_buildings(build_missing_configs(3))
            .with_sample_weights(vec![1.0, 0.0, 2.5])
            .expect("finite non-negative weights are accepted");
        let weights: Vec<f64> = fleet.entries.iter().map(|e| e.sample_weight).collect();
        assert_eq!(weights, vec![1.0, 0.0, 2.5]);
    }

    #[test]
    fn with_sample_weights_rejects_nan() {
        let err = Fleet::from_buildings(build_missing_configs(2))
            .with_sample_weights(vec![1.0, f64::NAN])
            .expect_err("NaN weight must be rejected");
        match err {
            FleetError::InvalidSampleWeight { bldg_id, value } => {
                assert_eq!(bldg_id, 2);
                assert!(value.is_nan());
            }
            other => panic!("expected InvalidSampleWeight, got {other:?}"),
        }
    }

    #[test]
    fn with_sample_weights_rejects_negative() {
        let err = Fleet::from_buildings(build_missing_configs(2))
            .with_sample_weights(vec![1.0, -3.0])
            .expect_err("negative weight must be rejected");
        match err {
            FleetError::InvalidSampleWeight { bldg_id, value } => {
                assert_eq!(bldg_id, 2);
                assert_eq!(value, -3.0);
            }
            other => panic!("expected InvalidSampleWeight, got {other:?}"),
        }
    }

    #[test]
    fn with_sample_weights_rejects_infinite() {
        let err = Fleet::from_buildings(build_missing_configs(1))
            .with_sample_weights(vec![f64::INFINITY])
            .expect_err("infinite weight must be rejected");
        match err {
            FleetError::InvalidSampleWeight { bldg_id, value } => {
                assert_eq!(bldg_id, 1);
                assert!(value.is_infinite());
            }
            other => panic!("expected InvalidSampleWeight, got {other:?}"),
        }
    }

    #[test]
    fn with_sample_weights_rejects_length_mismatch() {
        let err = Fleet::from_buildings(build_missing_configs(2))
            .with_sample_weights(vec![1.0])
            .expect_err("length mismatch must be rejected");
        match err {
            FleetError::SampleWeightLengthMismatch { expected, provided } => {
                assert_eq!(expected, 2);
                assert_eq!(provided, 1);
            }
            other => panic!("expected SampleWeightLengthMismatch, got {other:?}"),
        }
    }

    #[test]
    fn panic_in_callback_is_isolated_to_one_result() {
        let panic_on = 2usize;
        let fleet =
            Fleet::from_buildings(build_valid_configs(3)).with_progress(move |done, _total| {
                if done == panic_on {
                    panic!("injected callback panic");
                }
            });

        let results = fleet.simulate(3);
        assert_eq!(results.len(), 3);

        let panic_count = results
            .iter()
            .filter(|result| matches!(result, Err(SimError::Panic { .. })))
            .count();

        let success_count = results.iter().filter(|result| result.is_ok()).count();
        assert_eq!(panic_count, 1);
        assert_eq!(success_count, 2);
    }

    #[test]
    fn panic_error_message_includes_file_and_line() {
        let panic_on = 2usize;
        let fleet =
            Fleet::from_buildings(build_valid_configs(3)).with_progress(move |done, _total| {
                if done == panic_on {
                    panic!("injected callback panic");
                }
            });

        let results = fleet.simulate(3);
        assert_eq!(results.len(), 3);

        let panic_msg = results
            .iter()
            .find_map(|result| match result {
                Err(SimError::Panic { message, .. }) => Some(message.clone()),
                _ => None,
            })
            .expect("one result should be a Panic error");

        // Verifies the end-to-end chain: PanicHookGuard (installed in calling thread)
        // → custom hook fires in Rayon worker → thread-local PANIC_INFO populated
        // → catch_unwind captures payload → panic_payload_to_string enriches with file:line.
        assert!(
            panic_msg.contains("fleet.rs") && panic_msg.contains(" — "),
            "panic message should contain file:line location enrichment, got: {panic_msg}"
        );
    }

    #[test]
    fn thread_count_configuration_and_global_pool_path_work() {
        let thread_ids = Arc::new(Mutex::new(HashSet::new()));
        let thread_ids_for_callback = Arc::clone(&thread_ids);

        // Keep each unit of work non-trivial so Rayon has time to schedule
        // across workers, even when simulations fail quickly.
        let fleet =
            Fleet::from_buildings(build_missing_configs(64)).with_progress(move |_done, _total| {
                let id = thread::current().id();
                thread_ids_for_callback
                    .lock()
                    .expect("lock thread id set")
                    .insert(id);
                std::thread::sleep(std::time::Duration::from_millis(2));
            });

        let sequential = fleet.simulate(1);
        assert_eq!(sequential.len(), 64);
        let sequential_threads = thread_ids
            .lock()
            .expect("lock thread ids after sequential run")
            .len();
        assert_eq!(sequential_threads, 1);

        // Retry a few times and assert on the max observed thread usage to
        // avoid scheduler timing flakes.
        let mut max_parallel_threads = 0usize;
        for _ in 0..3 {
            thread_ids
                .lock()
                .expect("lock thread id set for clear")
                .clear();

            let parallel = fleet.simulate(4);
            assert_eq!(parallel.len(), 64);

            let parallel_threads = thread_ids
                .lock()
                .expect("lock thread ids after parallel run")
                .len();
            max_parallel_threads = max_parallel_threads.max(parallel_threads);

            if max_parallel_threads > 1 {
                break;
            }
        }

        assert!(
            max_parallel_threads > 1,
            "expected >1 worker thread with n_threads=4, observed {max_parallel_threads}"
        );

        let global_pool = fleet.simulate(0);
        assert_eq!(global_pool.len(), 64);
    }

    #[test]
    fn progress_callback_is_invoked_and_can_lock_shared_state() {
        let progress_count = Arc::new(AtomicUsize::new(0));
        let progress_count_cb = Arc::clone(&progress_count);
        let lock = Arc::new(Mutex::new(()));
        let lock_cb = Arc::clone(&lock);

        let fleet =
            Fleet::from_buildings(build_missing_configs(8)).with_progress(move |_done, _total| {
                let _guard = lock_cb.lock().expect("lock in callback");
                progress_count_cb.fetch_add(1, Ordering::Relaxed);
            });

        let _ = fleet.simulate(2);
        assert!(progress_count.load(Ordering::Relaxed) > 0);
    }

    #[test]
    fn weather_filename_matches_dataset_conventions() {
        assert_eq!(
            weather_filename_for_building("G0800130", ResStockVersion::V2024_1),
            "G0800130.epw"
        );
        assert_eq!(
            weather_filename_for_building("G0800130", ResStockVersion::V2024_2),
            "G0800130.epw"
        );
        assert_eq!(
            weather_filename_for_building("G0800130", ResStockVersion::V2025_1),
            "G0800130_2018.csv"
        );
    }

    #[test]
    fn test_resolve_weather_path_with_fips_2024_epw() {
        let building = ResStockBuilding {
            bldg_id: 1,
            upgrade: 0,
            sample_weight: 1.0,
            hpxml_path: PathBuf::from("/data/bldg.zip"),
            schedule_path: PathBuf::from("/data/bldg.zip"),
            weather_path: None,
            weather_fips: Some("G0800130".to_string()),
            characteristics: HashMap::new(),
        };
        let weather_dir = PathBuf::from("/data/weather");
        let result =
            resolve_weather_path_for_building(&building, &weather_dir, ResStockVersion::V2024_2);
        assert_eq!(result, Some(PathBuf::from("/data/weather/G0800130.epw")));
    }

    #[test]
    fn test_resolve_weather_path_with_fips_2025_csv() {
        let building = ResStockBuilding {
            bldg_id: 2,
            upgrade: 0,
            sample_weight: 1.0,
            hpxml_path: PathBuf::from("/data/bldg.zip"),
            schedule_path: PathBuf::from("/data/bldg.zip"),
            weather_path: None,
            weather_fips: Some("G0800130".to_string()),
            characteristics: HashMap::new(),
        };
        let weather_dir = PathBuf::from("/data/weather");
        let result =
            resolve_weather_path_for_building(&building, &weather_dir, ResStockVersion::V2025_1);
        assert_eq!(
            result,
            Some(PathBuf::from("/data/weather/G0800130_2018.csv"))
        );
    }

    #[test]
    fn test_resolve_weather_path_without_fips_returns_none() {
        let building = ResStockBuilding {
            bldg_id: 1,
            upgrade: 0,
            sample_weight: 1.0,
            hpxml_path: PathBuf::from("/data/bldg.zip"),
            schedule_path: PathBuf::from("/data/bldg.zip"),
            weather_path: None,
            weather_fips: None,
            characteristics: HashMap::new(),
        };
        let result = resolve_weather_path_for_building(
            &building,
            &PathBuf::from("/data/weather"),
            ResStockVersion::V2024_2,
        );
        assert_eq!(result, None);
    }

    #[test]
    fn path_is_readable_weather_file_rejects_zip() {
        // Even if the path doesn't exist, a .zip extension should be rejected.
        assert!(!path_is_readable_weather_file(Path::new("something.zip")));
    }

    #[test]
    fn resolve_weather_path_matches_fixture_files() {
        let fixture_root =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/resstock");

        // 2024.2 fixtures use {fips}.epw naming
        {
            let weather_dir = fixture_root.join("2024.2/weather");
            assert!(
                weather_dir.is_dir(),
                "fixture weather directory missing: {weather_dir:?}"
            );
            let building = ResStockBuilding {
                bldg_id: 1,
                upgrade: 0,
                sample_weight: 1.0,
                hpxml_path: PathBuf::from(""),
                schedule_path: PathBuf::from(""),
                weather_path: None,
                weather_fips: Some("G0800050".to_string()),
                characteristics: HashMap::new(),
            };
            let resolved = resolve_weather_path_for_building(
                &building,
                &weather_dir,
                ResStockVersion::V2024_2,
            )
            .expect("FIPS should produce a path");
            assert_eq!(
                resolved.file_name().unwrap(),
                std::ffi::OsStr::new("G0800050.epw"),
            );
            assert!(
                resolved.exists(),
                "resolved 2024.2 path does not exist: {resolved:?}"
            );
            assert!(
                path_is_readable_weather_file(&resolved),
                "resolved 2024.2 path is not a readable weather file: {resolved:?}"
            );
        }

        // 2025.1 fixtures use {fips}_2018.csv naming
        {
            let weather_dir = fixture_root.join("2025.1/weather");
            assert!(
                weather_dir.is_dir(),
                "fixture weather directory missing: {weather_dir:?}"
            );
            let building = ResStockBuilding {
                bldg_id: 2,
                upgrade: 0,
                sample_weight: 1.0,
                hpxml_path: PathBuf::from(""),
                schedule_path: PathBuf::from(""),
                weather_path: None,
                weather_fips: Some("G0100590".to_string()),
                characteristics: HashMap::new(),
            };
            let resolved = resolve_weather_path_for_building(
                &building,
                &weather_dir,
                ResStockVersion::V2025_1,
            )
            .expect("FIPS should produce a path");
            assert_eq!(
                resolved.file_name().unwrap(),
                std::ffi::OsStr::new("G0100590_2018.csv"),
            );
            assert!(
                resolved.exists(),
                "resolved 2025.1 path does not exist: {resolved:?}"
            );
            assert!(
                path_is_readable_weather_file(&resolved),
                "resolved 2025.1 path is not a readable weather file: {resolved:?}"
            );
        }
    }

    #[test]
    fn steppable_fleet_steps_and_finishes() {
        let (mut fleet, build_errors) =
            SteppableFleet::from_configs(build_valid_configs(3), 2).expect("build steppable fleet");
        assert!(build_errors.is_empty());
        assert_eq!(fleet.len(), 3);
        assert!(!fleet.is_finished());
        assert_eq!(fleet.current_step(), 0);
        assert!(fleet.total_steps() > 0);
        assert!(fleet.time_res_s() > 0.0);

        let first_step = fleet.step();
        assert_eq!(first_step.len(), 3);
        assert!(first_step.iter().all(|result| result.is_ok()));
        for result in first_step {
            let step = result.expect("step succeeds");
            assert!(step.timestamp.timestamp() > 0);
        }
        assert_eq!(fleet.current_step(), 1);
        assert!(!fleet.is_finished());

        while !fleet.is_finished() {
            let step = fleet.step();
            assert_eq!(step.len(), 3);
            assert!(step.iter().all(|result| result.is_ok()));
        }

        assert!(fleet.is_finished());
        assert_eq!(fleet.current_step(), fleet.total_steps());
        let post_end = fleet.step();
        assert_eq!(post_end.len(), 3);
        assert!(
            post_end
                .iter()
                .all(|result| matches!(result, Err(SimError::Failed { .. })))
        );
    }

    #[test]
    fn steppable_fleet_from_configs_returns_partial_success() {
        let mut configs = build_valid_configs(2);
        let mut bad = build_missing_configs(1);
        bad[0].bldg_id = 999;
        configs.extend(bad);

        let (mut fleet, build_errors) =
            SteppableFleet::from_configs(configs, 0).expect("partial success should build");
        assert_eq!(fleet.len(), 2);
        assert_eq!(build_errors.len(), 1);
        assert_eq!(build_errors[0].bldg_id, 999);
        assert!(!build_errors[0].message.is_empty());

        let results = fleet.step();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|result| result.is_ok()));
    }

    #[test]
    fn steppable_fleet_empty_configs_returns_error() {
        let result = SteppableFleet::from_configs(vec![], 0);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("at least one dwelling config")
        );
    }

    #[test]
    fn steppable_fleet_all_configs_fail_returns_error() {
        let bad_configs = build_missing_configs(3);
        let result = SteppableFleet::from_configs(bad_configs, 0);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("all dwellings failed"));
        assert!(err_msg.contains("3 failure"));
    }

    #[test]
    fn steppable_fleet_skips_failed_dwelling() {
        let (mut fleet, build_errors) =
            SteppableFleet::from_configs(build_valid_configs(3), 2).expect("build steppable fleet");
        assert!(build_errors.is_empty());
        assert_eq!(fleet.len(), 3);

        // Mark dwelling 1 as permanently failed (simulating post-panic state).
        fleet.dwellings[1].failed = true;

        // Step: dwellings 0 and 2 should succeed, dwelling 1 should be skipped.
        let results = fleet.step();
        assert_eq!(results.len(), 3);

        assert!(results[0].is_ok(), "dwelling 0 should succeed");
        assert!(results[2].is_ok(), "dwelling 2 should succeed");

        match &results[1] {
            Err(SimError::Skipped { bldg_id, message }) => {
                assert_eq!(*bldg_id, 2, "skipped dwelling should have bldg_id 2");
                assert!(
                    message.contains("prior panic"),
                    "skip message should mention prior panic"
                );
            }
            other => panic!("expected Skipped error for dwelling 1, got {:?}", other),
        };

        // Telemetry must reflect the failed flag.
        let tel = fleet.telemetry(1).expect("telemetry for dwelling 1");
        assert!(
            tel.dwelling_failed,
            "telemetry should mark dwelling 1 as failed"
        );

        // Subsequent step also skips the same dwelling.
        let results2 = fleet.step();
        assert!(
            matches!(results2[1], Err(SimError::Skipped { .. })),
            "dwelling 1 should stay skipped on every subsequent step"
        );
    }

    #[test]
    fn steppable_fleet_ipc_isolation_after_failure() {
        // Integration test: verify that a failed dwelling does not prevent
        // other dwellings from producing valid results over a full simulation.
        let (mut fleet, build_errors) =
            SteppableFleet::from_configs(build_valid_configs(3), 2).expect("build steppable fleet");
        assert!(build_errors.is_empty());
        assert_eq!(fleet.len(), 3);

        // Mark dwelling 1 as failed at the first step.
        fleet.dwellings[1].failed = true;

        let mut ok_steps = [0usize; 3];
        let mut skipped_steps = [0usize; 3];

        while !fleet.is_finished() {
            let results = fleet.step();
            assert_eq!(results.len(), 3);

            for (i, result) in results.iter().enumerate() {
                match result {
                    Ok(_) => ok_steps[i] += 1,
                    Err(SimError::Skipped { .. }) => skipped_steps[i] += 1,
                    other => panic!(
                        "unexpected result for dwelling {i}: expected Ok or Skipped, got {:?}",
                        other
                    ),
                }
            }
        }

        // Dwelling 0 and 2 produced results every step.
        assert!(ok_steps[0] > 0, "dwelling 0 should have successful steps");
        assert!(ok_steps[2] > 0, "dwelling 2 should have successful steps");
        assert_eq!(
            ok_steps[0], ok_steps[2],
            "both non-failed dwellings stepped the same number of times"
        );

        // Dwelling 1 was skipped on every step, never succeeded.
        assert_eq!(ok_steps[1], 0, "dwelling 1 should never produce an Ok step");
        assert!(
            skipped_steps[1] > 0,
            "dwelling 1 should be skipped every step"
        );

        // Telemetry reflects the failure.
        let tel = fleet.telemetry(1).expect("telemetry for dwelling 1");
        assert!(tel.dwelling_failed);

        // Non-failed dwellings are not flagged.
        assert!(!fleet.telemetry(0).unwrap().dwelling_failed);
        assert!(!fleet.telemetry(2).unwrap().dwelling_failed);
    }

    #[test]
    fn construction_panic_is_not_a_runtime_failure() {
        // Regression test: a dwelling that fails during from_config is
        // never added to the fleet, so it cannot be marked as a runtime
        // failure.  The AssertUnwindSafe wrapping at from_configs line 343
        // is sound because construction failure (whether error or panic)
        // prevents fleet inclusion.
        let mut configs = build_valid_configs(2);
        let mut bad = build_missing_configs(1);
        bad[0].bldg_id = 999;
        configs.extend(bad);

        let (mut fleet, build_errors) =
            SteppableFleet::from_configs(configs, 0).expect("partial success should build");
        assert_eq!(fleet.len(), 2);
        assert_eq!(build_errors.len(), 1);
        assert_eq!(build_errors[0].bldg_id, 999);

        // The two successfully-constructed dwellings are not marked failed.
        for i in 0..fleet.len() {
            assert!(
                !fleet.dwellings[i].failed,
                "dwelling {i} should not be marked failed at construction"
            );
        }

        // The failed construction never produced a dwelling, so there is
        // no runtime state to be corrupted.  The fleet has only the 2
        // valid dwellings, and both step normally.
        let results = fleet.step();
        assert_eq!(results.len(), 2);
        assert!(
            results.iter().all(|r| r.is_ok()),
            "valid dwellings should step without error"
        );
    }

    #[cfg(debug_assertions)]
    #[test]
    fn dwelling_panic_during_step_is_caught_and_marks_dwelling_failed() {
        // End-to-end test of the panic→failed→skipped path in
        // step_dwellings_parallel.  A dwelling configured to panic during
        // step() must:
        //   (a) produce Err(SimError::Panic) from the panicking step
        //   (b) produce Err(SimError::Skipped) on every subsequent step
        //   (c) leave other dwellings unaffected
        // The invariant check in SteppableFleet::step() must NOT misfire
        // when the dwelling becomes failed during this step.
        let (mut fleet, build_errors) =
            SteppableFleet::from_configs(build_valid_configs(3), 2).expect("build steppable fleet");
        assert!(build_errors.is_empty());
        assert_eq!(fleet.len(), 3);

        // Configure dwelling 1 to panic on its next step() call.
        // set_test_panic is #[cfg(debug_assertions)] gated — available in
        // test builds.
        fleet.dwellings[1].set_test_panic();

        // First step: dwelling 1 panics, the panic is caught by
        // step_dwellings_parallel, failed is set to true.
        let results = fleet.step();
        assert_eq!(results.len(), 3);

        // (a) Panicking dwelling returns Panic.
        match &results[1] {
            Err(SimError::Panic { bldg_id, message }) => {
                assert_eq!(*bldg_id, 2, "panicked dwelling should have bldg_id 2");
                assert!(
                    message.contains("test-induced panic"),
                    "panic message should contain 'test-induced panic', got: {message}"
                );
            }
            other => panic!("expected Panic error for dwelling 1, got {:?}", other),
        }

        // (c) Other dwellings continue normally.
        assert!(results[0].is_ok(), "dwelling 0 should succeed");
        assert!(results[2].is_ok(), "dwelling 2 should succeed");

        // The failed flag must be set on the panicked dwelling only.
        assert!(
            fleet.dwellings[1].failed,
            "dwelling 1 should be marked failed after panic"
        );
        assert!(!fleet.dwellings[0].failed);
        assert!(!fleet.dwellings[2].failed);

        // (b) Second step: dwelling 1 is now failed → Skipped.
        let results2 = fleet.step();
        assert_eq!(results2.len(), 3);

        match &results2[1] {
            Err(SimError::Skipped { bldg_id, message }) => {
                assert_eq!(*bldg_id, 2);
                assert!(message.contains("prior panic"));
            }
            other => panic!("expected Skipped error for dwelling 1, got {:?}", other),
        }

        assert!(results2[0].is_ok(), "dwelling 0 should succeed on step 2");
        assert!(results2[2].is_ok(), "dwelling 2 should succeed on step 2");

        // Telemetry reflects the failed state.
        let tel = fleet.telemetry(1).expect("telemetry for dwelling 1");
        assert!(tel.dwelling_failed);

        assert!(!fleet.telemetry(0).unwrap().dwelling_failed);
        assert!(!fleet.telemetry(2).unwrap().dwelling_failed);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn assert_panic_from_dwelling_step_includes_file_and_line() {
        // Verifies the end-to-end chain when equipment/dwelling code panics
        // via assert! (as opposed to panic!()). The hook must capture
        // PanicHookInfo::location() from the assert's expansion site
        // (the dwelling source file), not from the catch_unwind call site
        // in fleet code.
        let (mut fleet, build_errors) =
            SteppableFleet::from_configs(build_valid_configs(3), 2).expect("build steppable fleet");
        assert!(build_errors.is_empty());
        assert_eq!(fleet.len(), 3);

        fleet.dwellings[1].set_test_assert_panic();

        let results = fleet.step();
        assert_eq!(results.len(), 3);

        match &results[1] {
            Err(SimError::Panic { bldg_id, message }) => {
                assert_eq!(*bldg_id, 2);
                assert!(
                    message.contains("test-induced assert failure"),
                    "expected assert failure text in message: {message}"
                );
                assert!(
                    message.contains(" — "),
                    "expected location separator in message: {message}"
                );
                // The location should point to the dwelling source file,
                // not fleet.rs (the catch_unwind site).
                assert!(
                    message.contains("mod.rs") || message.contains("dwelling"),
                    "expected dwelling source file in message: {message}"
                );
            }
            other => panic!(
                "expected Panic error for assert-panicking dwelling, got {:?}",
                other
            ),
        }

        assert!(results[0].is_ok());
        assert!(results[2].is_ok());
    }

    // --- unit: double-panic prevention ---

    #[cfg(debug_assertions)]
    #[test]
    fn single_panic_does_not_abort_fleet_and_other_dwellings_intact() {
        // Integration test: verify that when one dwelling panics in a
        // steppable fleet, the process does not abort and the other
        // dwellings produce valid results for every remaining step.
        // This validates that the outer catch_unwind in step_dwellings_parallel
        // correctly isolates per-dwelling panics at fleet scale.
        let (mut fleet, build_errors) =
            SteppableFleet::from_configs(build_valid_configs(3), 2).expect("build steppable fleet");
        assert!(build_errors.is_empty());
        assert_eq!(fleet.len(), 3);

        // Trigger panic in dwelling 1 on step 0.
        fleet.dwellings[1].set_test_panic();

        // Step 0: dwelling 1 panics, the fleet survives.
        let results = fleet.step();
        assert_eq!(results.len(), 3);

        // Dwelling 1 returns a Panic error.
        match &results[1] {
            Err(SimError::Panic { bldg_id, message }) => {
                assert_eq!(*bldg_id, 2);
                assert!(
                    message.contains("test-induced panic"),
                    "expected 'test-induced panic' in message, got: {message}"
                );
            }
            other => panic!("expected Panic for dwelling 1, got {:?}", other),
        }

        // Other dwellings continue normally.
        assert!(results[0].is_ok(), "dwelling 0 should succeed");
        assert!(results[2].is_ok(), "dwelling 2 should succeed");

        // Dwelling 1 is marked failed.
        assert!(
            fleet.dwellings[1].failed,
            "dwelling 1 should be marked failed"
        );

        // Remaining steps: dwelling 1 is skipped, others complete normally.
        // Fleet simulation must run to completion without aborting.
        while !fleet.is_finished() {
            let step_results = fleet.step();
            assert_eq!(step_results.len(), 3);
            assert!(step_results[0].is_ok());
            assert!(matches!(step_results[1], Err(SimError::Skipped { .. })));
            assert!(step_results[2].is_ok());
        }

        // Verify fleet finished normally (did not abort).
        assert!(fleet.is_finished());
    }

    #[test]
    fn double_panic_guard_fallback_constructs_valid_sim_error() {
        // Unit test: verify the double-panic guard pattern used in
        // simulate_parallel and step_dwellings_parallel. When the post-
        // catch_unwind error handler itself panics, the nested catch_unwind
        // absorbs it and returns a SimError::Panic with the static fallback
        // message, preventing process abort.
        let bldg_id: i64 = 42;
        let fallback_message = "panic handling failed (double-panic prevented)";

        // Outer catch_unwind: simulates the fleet-level panic isolation.
        let outer = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let sim_payload = std::panic::catch_unwind(AssertUnwindSafe(|| panic!("test panic")));

            // Inner catch_unwind: guards the error-handling block
            // (SimError::Panic construction + panic_payload_to_string).
            let handler_result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                let _payload = sim_payload.unwrap_err();
                // Intentionally panic to simulate allocator failure during
                // error handling.
                panic!("error handler panic");
            }));

            match handler_result {
                Ok(_) => unreachable!(),
                Err(_) => {
                    record_double_panic_prevented();
                    Err::<(), SimError>(SimError::Panic {
                        bldg_id,
                        message: fallback_message.into(),
                    })
                }
            }
        }));

        assert!(
            outer.is_ok(),
            "outer catch_unwind should succeed — no process abort"
        );
        match outer.unwrap() {
            Err(SimError::Panic {
                bldg_id: id,
                message,
            }) => {
                assert_eq!(id, 42);
                assert_eq!(message, fallback_message);
            }
            other => panic!("expected SimError::Panic with fallback, got {:?}", other),
        }
    }

    fn build_hpxml_with_zone(zone: &str) -> String {
        format!(
            r#"<?xml version='1.0' encoding='UTF-8'?>
<HPXML xmlns='http://hpxmlonline.com/2023/09' schemaVersion='4.0'>
  <Building>
    <BuildingDetails>
      <ClimateandRiskZones>
        <ClimateZoneIECC>
          <Year>2006</Year>
          <ClimateZone>{}</ClimateZone>
        </ClimateZoneIECC>
        <WeatherStation>
          <SystemIdentifier id='WeatherStation'/>
          <Name>USA_CO_Denver</Name>
        </WeatherStation>
      </ClimateandRiskZones>
    </BuildingDetails>
  </Building>
</HPXML>"#,
            zone
        )
    }

    #[test]
    fn test_remap_weather_path_zone_validation() {
        let hpxml_dir = unique_temp_path("hpxml_dir");
        fs::create_dir_all(&hpxml_dir).expect("create temp hpxml dir");
        let hpxml_path = hpxml_dir.join("home.xml");
        write_temp_file(&hpxml_path, &build_hpxml_with_zone("5B"));

        let epw_path = unique_temp_path("epw");
        write_temp_file(
            &epw_path,
            "LOCATION,USA_CO_Denver.Intl.AP.725650_TMY3,CO,USA,TMY3,725650,39.83,-104.65,-7.0,1609.0",
        );

        let zone = hares_io::parse_iecc_climate_zone(&hpxml_path);
        assert_eq!(
            zone.as_deref(),
            Some("5B"),
            "should extract IECC climate zone from HPXML"
        );

        let state = hares_io::parse_epw_location_state(&epw_path);
        assert_eq!(
            state.as_deref(),
            Some("CO"),
            "should extract state from EPW LOCATION header"
        );

        assert!(
            iecc_state_zones("CO").contains(&5),
            "Colorado IECC zones should include zone 5"
        );

        assert!(
            !iecc_state_zones("CO").contains(&2),
            "Colorado IECC zones should NOT include zone 2"
        );

        // zone=5B + weather=CO -> valid match
        assert_eq!(
            validate_fleet_building_zone(&hpxml_path, &epw_path, 1),
            ZoneMatchStatus::Match {
                building_zone: "5B".to_string(),
                weather_state: "CO".to_string()
            }
        );

        // zone=2A + weather=CO -> mismatch
        write_temp_file(&hpxml_path, &build_hpxml_with_zone("2A"));
        assert_eq!(
            validate_fleet_building_zone(&hpxml_path, &epw_path, 2),
            ZoneMatchStatus::Mismatch {
                building_zone: "2A".to_string(),
                weather_state: "CO".to_string()
            }
        );

        // Missing zone -> skipped
        let no_zone_xml = r#"<?xml version='1.0'?>
<HPXML xmlns='http://hpxmlonline.com/2023/09' schemaVersion='4.0'>
  <Building/>
</HPXML>"#;
        write_temp_file(&hpxml_path, no_zone_xml);
        assert_eq!(
            validate_fleet_building_zone(&hpxml_path, &epw_path, 3),
            ZoneMatchStatus::Skipped
        );

        // Non-EPW extension -> skipped (state parse returns None)
        let csv_path = unique_temp_path("csv");
        write_temp_file(&csv_path, "not,an,epw");
        assert_eq!(
            validate_fleet_building_zone(&hpxml_path, &csv_path, 4),
            ZoneMatchStatus::Skipped
        );
    }
}
