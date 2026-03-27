//! Fleet struct and parallel dwelling simulation.

use std::collections::HashMap;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{Duration, FixedOffset, TimeZone};
use hares_core::{DwellingConfig, SimStatus as CoreSimStatus, SimulationEngine, SimulationResults};
use hares_io::{OutputFormat, ResStockVersion, SimulationConfig, parse_resstock_metadata};
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

/// Errors returned while building a [`Fleet`].
#[derive(Debug, Error)]
pub enum FleetError {
    #[error("resstock metadata parse failed: {0}")]
    ResStock(String),
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

        let entries = buildings
            .into_iter()
            .filter(|building| matches_filter(&building.characteristics, filter.as_ref()))
            .map(|building| {
                let weather_path =
                    remap_weather_path(&building.weather_path, hpxml_dir, weather_dir);
                FleetEntry {
                    config: DwellingConfig {
                        hpxml_path: building.hpxml_path,
                        schedule_path: building.schedule_path,
                        weather_path,
                        defaults_path: None,
                        sim_config: default_resstock_sim_config(),
                        overrides: None,
                        bldg_id: building.bldg_id,
                        initialization_duration: None,
                        resample_overrides: None,
                    },
                    sample_weight: building.sample_weight,
                }
            })
            .collect();

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
    /// The weights vector length must match the number of entries.
    #[must_use]
    pub fn with_sample_weights(mut self, weights: Vec<f64>) -> Self {
        assert_eq!(
            weights.len(),
            self.entries.len(),
            "weights length must match fleet size"
        );
        for (entry, weight) in self.entries.iter_mut().zip(weights) {
            entry.sample_weight = weight;
        }
        self
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

        self.entries
            .par_iter()
            .map(|entry| {
                let outcome = match panic::catch_unwind(AssertUnwindSafe(|| {
                    let outcome = run_entry(entry);
                    if let Some(cb) = &progress {
                        let done = completed.fetch_add(1, Ordering::Relaxed) + 1;
                        cb(done, total);
                    }
                    outcome
                })) {
                    Ok(result) => result,
                    Err(payload) => Err(SimError::Panic {
                        bldg_id: entry.config.bldg_id,
                        message: panic_payload_to_string(payload),
                    }),
                };

                if let Err(err) = &outcome {
                    tracing::warn!(bldg_id = entry.config.bldg_id, error = %err, "dwelling simulation failed");
                }

                outcome
            })
            .collect()
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
        output_format: OutputFormat::Csv,
        output_chunk_size: 10_000,
        setpoint_deadband_c: None,
        master_seed: 0,
        civil_timezone: None,
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

fn remap_weather_path(path: &Path, hpxml_dir: &Path, weather_dir: &Path) -> PathBuf {
    if hpxml_dir == weather_dir {
        return path.to_path_buf();
    }

    match path.file_name() {
        Some(file_name) => weather_dir.join(file_name),
        None => weather_dir.to_path_buf(),
    }
}

fn panic_payload_to_string(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(msg) = payload.downcast_ref::<&'static str>() {
        return (*msg).to_string();
    }
    if let Some(msg) = payload.downcast_ref::<String>() {
        return msg.clone();
    }
    "simulation panicked with non-string payload".to_string()
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
            output_format: OutputFormat::Csv,
            output_chunk_size: 128,
            setpoint_deadband_c: None,
            master_seed: 0,
            civil_timezone: None,
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
    fn thread_count_configuration_and_global_pool_path_work() {
        let thread_ids = Arc::new(Mutex::new(HashSet::new()));
        let thread_ids_for_callback = Arc::clone(&thread_ids);

        let fleet =
            Fleet::from_buildings(build_missing_configs(64)).with_progress(move |_done, _total| {
                let id = thread::current().id();
                thread_ids_for_callback
                    .lock()
                    .expect("lock thread id set")
                    .insert(id);
                thread::yield_now();
            });

        let sequential = fleet.simulate(1);
        assert_eq!(sequential.len(), 64);
        let sequential_threads = thread_ids
            .lock()
            .expect("lock thread ids after sequential run")
            .len();
        assert_eq!(sequential_threads, 1);

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
        assert!(parallel_threads > 1);

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
    fn test_remap_weather_path_same_dir_passthrough() {
        let hpxml_dir = PathBuf::from("/data/buildings");
        let weather_dir = PathBuf::from("/data/buildings");
        let path = PathBuf::from("/data/buildings/USA_CO_Denver.epw");

        let result = remap_weather_path(&path, &hpxml_dir, &weather_dir);
        assert_eq!(result, PathBuf::from("/data/buildings/USA_CO_Denver.epw"));
    }

    #[test]
    fn test_remap_weather_path_different_dir_remaps() {
        let hpxml_dir = PathBuf::from("/data/buildings");
        let weather_dir = PathBuf::from("/data/weather");
        let path = PathBuf::from("/data/buildings/USA_CO_Denver.epw");

        let result = remap_weather_path(&path, &hpxml_dir, &weather_dir);
        assert_eq!(result, PathBuf::from("/data/weather/USA_CO_Denver.epw"));
    }

    #[test]
    fn test_remap_weather_path_empty_path_uses_weather_dir() {
        let hpxml_dir = PathBuf::from("/data/buildings");
        let weather_dir = PathBuf::from("/data/weather");
        let path = PathBuf::from("");

        let result = remap_weather_path(&path, &hpxml_dir, &weather_dir);
        assert_eq!(result, PathBuf::from("/data/weather"));
    }
}
