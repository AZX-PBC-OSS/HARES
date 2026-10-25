//! Fleet struct and parallel dwelling simulation.

use std::collections::{BTreeMap, HashMap};
use std::num::NonZeroUsize;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, FixedOffset, TimeZone};
use hares_core::{
    Dwelling, DwellingBlueprintBuilder, DwellingConfig, DwellingSettings, DwellingTelemetry,
    RunResult, SimulationEngine, StepResult, WarmupOutcome,
};
use hares_io::Frequency;
use hares_io::output::reduce::PeriodCalendar;
use hares_io::{
    DefaultsSource, InputSource, ResStockBuilding, ResStockVersion, SampleWeightClass, Selection,
    SimulationConfig, WeatherInput, classify_sample_weight, parse_resstock_metadata,
};
use hares_types::ControlSignal;
use hares_types::HaresError;
use hares_types::panic_hook::{self, PanicHookGuard, record_double_panic_prevented};
use rayon::ThreadPoolBuilder;
use rayon::prelude::*;
use thiserror::Error;

use crate::fold::{
    Alignment, Coverage, FleetFold, FleetReport, FleetResult, FoldRows, HomeFoldAlignment,
    HomeFoldState, HomeResult, ReducerFeed, homes_batch,
};

const DEFAULT_RESSTOCK_VERSION: ResStockVersion = ResStockVersion::V2025_1;

/// The report name the fleet appends to every home's config; a home
/// whose own config already carries it is a construction error.
pub const FLEET_REPORT_NAME: &str = "fleet";

type ProgressCallback = Arc<dyn Fn(usize, usize) + Send + Sync + 'static>;

/// Result type for fleet construction operations.
pub type Result<T> = std::result::Result<T, FleetError>;

/// The outcome class of one home's run, decided by [`home_status`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HomeStatus {
    /// The run finished clean: no warning, no health event, the warm-up
    /// converged or disabled.
    Ok,
    /// The run finished but carried a degradation: the reason names the
    /// first warning and each non-zero health field. A degraded home is
    /// never a fleet success.
    Degraded(String),
    /// The run did not finish: the error or panic.
    Failed(String),
}

/// The one classification of a home's run. A clean run is
/// [`HomeStatus::Ok`]; a run that finished but carries a warning, a
/// non-zero health counter or an unconverged warm-up is
/// [`HomeStatus::Degraded`], its reason naming the first warning and each
/// non-zero health field; a run error or panic, a `HealthPolicyViolation`
/// under the fleet's health policy included, is [`HomeStatus::Failed`]
/// with the error.
pub fn home_status(run: &std::result::Result<RunResult, HaresError>) -> HomeStatus {
    let result = match run {
        Ok(result) => result,
        Err(err) => return HomeStatus::Failed(err.to_string()),
    };
    let mut events: Vec<String> = Vec::new();
    if let Some(warning) = result.warnings.first() {
        events.push(format!("warning: {warning}"));
    }
    let health = &result.health;
    let counters = [
        ("port_rollbacks", health.port_rollbacks),
        ("rejected_control_signals", health.rejected_control_signals),
        ("clamped_actions", health.clamped_actions),
        ("curve_index_clamps", health.curve_index_clamps),
        ("fluid_payload_absences", health.fluid_payload_absences),
        ("evicted_actors", health.evicted_actors),
    ];
    for (name, count) in counters {
        if count > 0 {
            events.push(format!("{name}: {count}"));
        }
    }
    if let WarmupOutcome::Ran {
        converged: false, ..
    } = health.warmup
    {
        events.push("the warm-up did not converge".to_string());
    }
    if events.is_empty() {
        HomeStatus::Ok
    } else {
        HomeStatus::Degraded(events.join("; "))
    }
}

/// The fleet's construction report: what a ResStock fleet build parsed and
/// built, and how many homes' weather files resolved.
///
/// The counts describe one [`Fleet::from_resstock`] build: every metadata
/// row is a parsed building, the dwellings that survived the filter are the
/// built ones, and the weather counts classify the built dwellings by
/// whether their weather file resolved to a readable file. A fleet built by
/// any other constructor carries no report, because no metadata parse or
/// weather resolution happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FleetBuildReport {
    /// Metadata rows parsed into buildings, before any filter.
    pub buildings_parsed: usize,
    /// Dwellings built from the parsed buildings (post-filter entries).
    pub dwellings_built: usize,
    /// Built dwellings whose weather file resolved to a readable file.
    pub weather_resolved: usize,
    /// Built dwellings whose weather file did not resolve (no FIPS, no
    /// file, or an unreadable file).
    pub weather_failed: usize,
    /// The master random seed every home's stochastic streams derive
    /// from: the caller's, or the documented default of 0 when the
    /// caller passed none.
    pub master_seed: u64,
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
    #[error(
        "all dwellings failed to initialize ({} failure(s)): {}",
        count,
        causes.join("; ")
    )]
    AllSteppableDwellingsFailed { count: usize, causes: Vec<String> },
    #[error("failed to build local rayon thread pool: {0}")]
    ThreadPoolBuild(String),
    #[error(
        "the fleet's ok weight is zero: {ok} ok home(s), {degraded} degraded, {failed} failed; \
         every home's status and reason is in the homes table"
    )]
    NoCoveredWeight {
        ok: usize,
        degraded: usize,
        failed: usize,
        homes: arrow::record_batch::RecordBatch,
    },
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
    #[error("verbosity {value} exceeds the maximum of {max}")]
    InvalidOutputVerbosity { value: u8, max: u8 },
    #[error(
        "dwelling {bldg_id} declares a report named '{FLEET_REPORT_NAME}': the name is \
         reserved for the report the fleet appends; rename the home's own report"
    )]
    ReservedReportName { bldg_id: i64 },
    #[error(
        "the schema field '{name}' carries no declared-kind tag: a report schema field \
         without the declared `kind` metadata is a contract violation (the fleet folds \
         every variable by its declared kind; no rule is guessed from the column name)"
    )]
    UndeclaredKind { name: String },
    #[error(
        "no home declares {names:?}: a selection name must be declared by at least one \
         home's catalogue (a home that does not declare a name folds no value for it)"
    )]
    UnmatchedSelection { names: Vec<String> },
    #[error(
        "homes {first} and {second} write file destinations to the same path ({path}): \
         every home's file destination must resolve to its own path"
    )]
    DuplicateDestination {
        first: i64,
        second: i64,
        path: String,
    },
    #[error(
        "fleet already finished: all {total_steps} configured steps have run; \
         stepping past the configured end is an error, not a failure per home"
    )]
    AlreadyFinished { total_steps: u64 },
    #[error("dwelling_index {index} out of range (len={len})")]
    NoSuchHome { index: usize, len: usize },
    #[error("dwelling at index {index} refused the grid voltage override: {message}")]
    InvalidGridVoltage { index: usize, message: String },
    #[error("dwelling {bldg_id} telemetry snapshot failed: {source}")]
    Telemetry {
        bldg_id: i64,
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error(
        "the fleet's homes span more than one UTC offset ({offsets_s:?} s) and declare no \
         alignment: declare one with `with_alignment` (an `Instant` offset the homes \
         reduce and label on, or `LocalClock`)"
    )]
    AlignmentRequired { offsets_s: Vec<i32> },
    #[error(
        "dwelling {bldg_id} breaks the fleet's time grid: the homes' runs must share \
         duration and resolution, and by the fleet's alignment a shared naive local \
         start (or, under an `Instant` alignment, start instants on the declared \
         offset's step grid covering at least one whole reference period)"
    )]
    HeterogeneousTimeGrid { bldg_id: i64 },
    #[error(
        "ResStock {version} ships typical (TMY3) weather: pass the run's calendar year \
         as `typical_year`; there is no built-in year"
    )]
    TypicalYearRequired { version: String },
    #[error(
        "ResStock {version} ships actual-year (AMY) weather and reads every file's own \
         year: `typical_year` must be None, got {year}"
    )]
    TypicalYearConflict { version: String, year: i32 },
    #[error(
        "the ResStock fleet's weather failed for {count} building(s); every building was \
         resolved before this error: {details}"
    )]
    ResStockWeatherUnresolved { count: usize, details: String },
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
    /// The declared time alignment; `None` keeps the homes' own local
    /// calendars and requires a single site offset across the homes.
    alignment: Option<Alignment>,
    /// The construction report, recorded when the fleet was built from
    /// ResStock metadata.
    build_report: Option<FleetBuildReport>,
}

impl std::fmt::Debug for Fleet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fleet")
            .field("entries", &self.entries)
            .field("progress", &self.progress.as_ref().map(|_| "..."))
            .field("alignment", &self.alignment)
            .field("build_report", &self.build_report)
            .finish()
    }
}

impl Fleet {
    /// Constructs a fleet from explicit dwelling configs.
    ///
    /// # Errors
    ///
    /// [`FleetError::ReservedReportName`] when a home's config already
    /// carries a report named `fleet`, and
    /// [`FleetError::DuplicateDestination`] when two homes' file
    /// destinations resolve to the same path.
    pub fn from_buildings(configs: Vec<DwellingConfig>) -> Result<Self> {
        check_home_configs(&configs)?;
        let entries = configs
            .into_iter()
            .map(|config| FleetEntry {
                config,
                sample_weight: 1.0,
            })
            .collect();
        Ok(Self {
            entries,
            progress: None,
            alignment: None,
            build_report: None,
        })
    }

    /// Constructs a fleet from ResStock metadata and local data directories.
    ///
    /// # Layout policy: the unpacked buildings directory is required
    ///
    /// `buildings_dir` must hold one directory per building, named
    /// `bldg<bldg_id:07>`, containing that building's `home.xml` and
    /// `in.schedules.csv`: the layout `hares.data.resstock` unpacks every
    /// dataset zip to, and the committed `tests/fixtures/resstock/<version>`
    /// buildings carry. The engine never reads HPXML or schedules out of the
    /// dataset's zip archives, so the fleet does not unpack either; a
    /// building whose directory or files are missing (as when the dataset
    /// root with its zip archives is passed) is a typed error naming the
    /// building, the missing element and the fix. The layout resolution
    /// covers every metadata row: a `filter` narrows the fleet, not the
    /// parse, so a filtered fleet still requires every metadata building
    /// to be unpacked.
    ///
    /// # The declared weather year and start: nothing hardcoded
    ///
    /// Each building's weather file is resolved from the FIPS code in its
    /// own HPXML (`WeatherStation/Name`) against `weather_dir`, and every
    /// building is resolved before any error is raised: a building whose
    /// weather file cannot be resolved or read is a fleet construction
    /// error, one error naming every failing building with its county.
    ///
    /// The release declares the weather-year rule (`hares_io::WeatherYear`):
    /// 2025.1 ships actual-meteorological-year (AMY 2018) CSV files and declares
    /// `Actual`; 2024.x ships typical (TMY3) EPW files and declares
    /// `Typical`. Every home starts Jan 1 00:00 local standard time at its
    /// own site's standard offset (the resolution the built dwelling runs:
    /// the HPXML `Site`, the weather file's header and the config's
    /// override through `resolve_site_location`). The start's year
    /// is the weather file's own year for an actual-year release (the
    /// calendar year whose hours the file's records cover, read from the
    /// file's dated stamps at construction) and `typical_year` for a
    /// typical-year release: there is no built-in year, so a typical-year
    /// release without the argument is a construction error naming
    /// `typical_year`, raised before any file is read.
    ///
    /// `resstock_version` defaults to the latest known schema when `None`.
    /// `duration` overrides the default 24-hour simulation duration;
    /// when `None`, the default of 24 hours is used for backward compatibility.
    /// `defaults_path` becomes every dwelling's
    /// [`DwellingConfig::defaults`], with the same meaning: the path is
    /// required, and a missing or unreadable defaults tree is a
    /// construction error naming it.
    ///
    /// `master_seed` is the fleet's master random seed: `Some(seed)` seeds
    /// every home's stochastic streams (each home's own streams derive
    /// from the seed and its building id), `None` keeps the documented
    /// default seed of 0. The build report names the seed the fleet runs
    /// under, so a run record is never silent about it.
    // The flat keyword-shaped argument list is the point: it mirrors the
    // Python `Fleet.from_resstock` keyword surface name for name.
    #[allow(clippy::too_many_arguments)]
    pub fn from_resstock(
        metadata_path: &Path,
        buildings_dir: &Path,
        weather_dir: &Path,
        resstock_version: Option<ResStockVersion>,
        typical_year: Option<i32>,
        filter: Option<HashMap<String, String>>,
        duration: Option<chrono::Duration>,
        defaults_path: &Path,
        master_seed: Option<u64>,
    ) -> Result<Self> {
        let version = resstock_version.unwrap_or(DEFAULT_RESSTOCK_VERSION);
        // The caller's seed, or the documented default when unspecified.
        // Every home's stochastic streams derive from it and the home's
        // own building id.
        let resolved_master_seed = master_seed.unwrap_or(0);
        // The release's declared weather-year rule, and the start's year
        // when the release is a typical-year one. Both are decided before
        // any file is read.
        let weather_year = resstock_weather_year(version);
        let typical_start_year = match (weather_year, typical_year) {
            (hares_io::WeatherYear::Typical, None) => {
                return Err(FleetError::TypicalYearRequired {
                    version: version.to_string(),
                });
            }
            (hares_io::WeatherYear::Actual, Some(year)) => {
                return Err(FleetError::TypicalYearConflict {
                    version: version.to_string(),
                    year,
                });
            }
            (hares_io::WeatherYear::Actual, None) => None,
            (hares_io::WeatherYear::Typical, Some(year)) => Some(year),
        };

        let buildings = parse_resstock_metadata(metadata_path, version, buildings_dir)
            .map_err(|err| FleetError::ResStock(err.to_string()))?;

        let buildings_parsed = buildings.len();
        let run_duration = duration.unwrap_or_else(|| Duration::hours(24));
        let duration_hours = run_duration.num_seconds() as f64 / 3600.0;

        tracing::info!(
            duration_hours,
            "ResStock fleet simulation duration: {:.1} hours",
            duration_hours,
        );

        let filtered: Vec<ResStockBuilding> = buildings
            .into_iter()
            .filter(|building| matches_filter(&building.characteristics, filter.as_ref()))
            .collect();

        // Every building's weather is resolved before the error is raised,
        // so the one error names every failing building with its county.
        let mut failures: Vec<String> = Vec::new();
        let mut entries = Vec::with_capacity(filtered.len());
        for building in filtered {
            let county = building
                .weather_fips
                .clone()
                .unwrap_or_else(|| "unread".to_string());
            let weather_path =
                match resolve_weather_path_for_building(&building, weather_dir, version) {
                    Some(path) if path_is_readable_weather_file(&path) => path,
                    Some(path) => {
                        failures.push(format!(
                            "bldg{:07} (county {county}): the weather file {} is not found or not \
                         readable",
                            building.bldg_id,
                            path.display()
                        ));
                        continue;
                    }
                    None => {
                        failures.push(format!(
                        "bldg{:07} (county {county}): the HPXML carries no weather station FIPS \
                         code; the weather file cannot be resolved",
                        building.bldg_id
                    ));
                        continue;
                    }
                };

            // The start's year: the weather file's own year for an
            // actual-year release, read from the file's dated stamps
            // (B-1c); the caller's `typical_year` for a typical-year one.
            let start_year = match weather_year {
                hares_io::WeatherYear::Actual => {
                    match hares_io::parse_weather_with_location(&weather_path, 0.0, 0.0, 0.0, 0.0) {
                        Ok(series) => match series.file_year() {
                            Some(year) => year,
                            None => {
                                failures.push(format!(
                                    "bldg{:07} (county {county}): the weather file carries no \
                                     dated records; the actual-year start needs them",
                                    building.bldg_id
                                ));
                                continue;
                            }
                        },
                        Err(err) => {
                            failures.push(format!(
                                "bldg{:07} (county {county}): the weather file cannot be read: \
                                 {err}",
                                building.bldg_id
                            ));
                            continue;
                        }
                    }
                }
                hares_io::WeatherYear::Typical => typical_start_year
                    .expect("checked above: a typical-year release requires the year"),
            };

            // The site's standard offset, the same resolution the built
            // dwelling runs.
            let offset = match hares_core::dwelling::site_offset_from_inputs(
                &InputSource::Path(building.hpxml_path.clone()),
                &WeatherInput::Path(weather_path.clone()),
                &hares_io::SiteLocationOverride::default(),
            ) {
                Ok(offset) => offset,
                Err(err) => {
                    failures.push(format!(
                        "bldg{:07} (county {county}): the site's standard offset cannot be \
                         resolved: {err}",
                        building.bldg_id
                    ));
                    continue;
                }
            };
            let start_time = offset
                .with_ymd_and_hms(start_year, 1, 1, 0, 0, 0)
                .single()
                .expect("Jan 1 midnight is a valid local start");
            tracing::debug!(
                bldg_id = building.bldg_id,
                county = county.as_str(),
                offset_s = offset.local_minus_utc(),
                %start_time,
                "ResStock home start and site offset resolved"
            );

            validate_fleet_building_zone(&building.hpxml_path, &weather_path, building.bldg_id);
            entries.push(FleetEntry {
                config: DwellingConfig {
                    hpxml: InputSource::Path(building.hpxml_path),
                    schedule: Some(InputSource::Path(building.schedule_path)),
                    weather: WeatherInput::Path(weather_path),
                    defaults: DefaultsSource::directory(defaults_path),
                    settings: DwellingSettings::new(
                        resstock_sim_config(
                            run_duration,
                            start_time,
                            weather_year,
                            resolved_master_seed,
                        ),
                        None,
                        building.bldg_id,
                        Some(hares_io::HpxmlDataPatches::from_resstock_characteristics(
                            &building.characteristics,
                        )),
                    ),
                },
                sample_weight: building.sample_weight,
            });
        }

        if !failures.is_empty() {
            return Err(FleetError::ResStockWeatherUnresolved {
                count: failures.len(),
                details: failures.join("; "),
            });
        }

        tracing::info!(
            total = entries.len(),
            resolved = entries.len(),
            unresolved = 0,
            "ResStock fleet: every building's weather file resolved and read"
        );

        let report = FleetBuildReport {
            buildings_parsed,
            dwellings_built: entries.len(),
            weather_resolved: entries.len(),
            weather_failed: 0,
            master_seed: resolved_master_seed,
        };
        Ok(Self {
            entries,
            progress: None,
            alignment: None,
            build_report: Some(report),
        })
    }

    /// Declares the fleet's time alignment: `Instant(offset)` buckets
    /// every home's reduction on the offset's period boundaries, aligns
    /// the rows by the period's start instant and labels them at the
    /// offset, so homes at different site offsets reduce onto the same
    /// reference periods; `LocalClock` aligns the rows by the naive local
    /// period start the homes share. A fleet with no declared alignment
    /// keeps the homes' own local calendars and fails when the homes span
    /// more than one UTC offset.
    #[must_use]
    pub fn with_alignment(mut self, alignment: Alignment) -> Self {
        self.alignment = Some(alignment);
        self
    }

    /// The fleet's construction report: what a ResStock build parsed and
    /// built, and how many homes' weather files resolved. `None` when the
    /// fleet was built by a constructor that parses no metadata and
    /// resolves no weather files.
    #[must_use]
    pub fn build_report(&self) -> Option<&FleetBuildReport> {
        self.build_report.as_ref()
    }

    /// The homes' dwelling configs, in entry order, cloned: exactly what
    /// `simulate` builds and runs, the ResStock constructor's configs
    /// included. A read-only view for tools that report a home's resolved
    /// model of the config the fleet runs; the fleet itself is unchanged.
    #[must_use]
    pub fn home_configs(&self) -> Vec<DwellingConfig> {
        self.entries
            .iter()
            .map(|entry| entry.config.clone())
            .collect()
    }

    /// The fleet's building identities, in fleet order: the order
    /// [`simulate`](Self::simulate) folds its homes in, so a caller can
    /// label the homes table's rows without re-reading the metadata.
    #[must_use]
    pub fn bldg_ids(&self) -> Vec<i64> {
        self.entries
            .iter()
            .map(|entry| entry.config.settings.bldg_id)
            .collect()
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
                        bldg_id = entry.config.settings.bldg_id,
                        sample_weight = weight,
                        "dwelling has invalid sample_weight (NaN, infinite, or negative)"
                    );
                    return Err(FleetError::InvalidSampleWeight {
                        bldg_id: entry.config.settings.bldg_id,
                        value: weight,
                    });
                }
                SampleWeightClass::Zero => {
                    tracing::warn!(
                        bldg_id = entry.config.settings.bldg_id,
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

    /// Simulates the fleet through the streaming ordered fold: the homes
    /// run in consecutive chunks of the fleet's entry order on the fleet's
    /// pool, and each finished chunk's homes fold into the running
    /// [`FleetResult`] one at a time in entry order before the next chunk
    /// starts. The global fold order is the entry order for every chunk
    /// size and thread count, and at most one chunk's home results are
    /// alive at once.
    ///
    /// The fleet appends one report to every home, named `fleet`: the
    /// report's selection restricted to the home's catalogue, at the
    /// `Timestep` frequency, to a callback destination. The callback feeds
    /// each batch into a period reducer the fleet owns and drops the
    /// batch, so a home's result in the fold is its reduced period table,
    /// never its frame.
    ///
    /// When `n_threads > 0`, a local Rayon pool is created for this call.
    /// When `n_threads == 0`, the global Rayon pool is used.
    ///
    /// # Errors
    ///
    /// [`FleetError::ThreadPoolBuild`] when the local pool cannot be
    /// built, [`FleetError::UnmatchedSelection`] when a selection name
    /// matches no home's catalogue, [`FleetError::InvalidAggregationWeight`]
    /// for a weight that is not finite and non-negative,
    /// [`FleetError::NoCoveredWeight`] when the fleet's ok weight is
    /// zero (every home failed, degraded, or carried no positive
    /// weight), [`FleetError::AlignmentRequired`] when the
    /// homes span more than one UTC offset and the fleet declares no
    /// alignment, and [`FleetError::HeterogeneousTimeGrid`] when a home
    /// breaks the declared alignment's time grid, the last two raised in
    /// the pre-pass before any home runs.
    pub fn simulate(
        &self,
        report: &FleetReport,
        n_threads: usize,
        chunk_size: NonZeroUsize,
    ) -> Result<FleetResult> {
        if n_threads == 0 {
            return self.simulate_folded(report, chunk_size);
        }
        let pool = ThreadPoolBuilder::new()
            .num_threads(n_threads)
            .build()
            .map_err(|err| FleetError::ThreadPoolBuild(err.to_string()))?;
        pool.install(|| self.simulate_folded(report, chunk_size))
    }

    fn simulate_folded(
        &self,
        report: &FleetReport,
        chunk_size: NonZeroUsize,
    ) -> Result<FleetResult> {
        let _guard = PanicHookGuard::new();
        #[cfg(debug_assertions)]
        {
            assert!(
                panic_hook::is_installed(),
                "custom panic hook must be installed before fleet simulation"
            );
        }

        let total = self.entries.len();
        let completed = AtomicUsize::new(0);
        let progress = self.progress.clone();

        // The alignment pre-pass, before any home runs: every home's site
        // offset, read without parsing a schedule, a defaults tree or a
        // weather row, and the declared alignment's time-grid check. A
        // home whose site cannot be resolved declares no offset here; its
        // run fails for the same cause, the way the selection's per-home
        // restriction treats a home whose inputs fail to parse.
        let home_offsets: Vec<Option<FixedOffset>> = self
            .entries
            .iter()
            .map(
                |entry| match hares_core::dwelling::site_offset(&entry.config) {
                    Ok(offset) => Some(offset),
                    Err(err) => {
                        tracing::debug!(
                            bldg_id = entry.config.settings.bldg_id,
                            error = %err,
                            "the fleet's site-offset pre-pass could not resolve the home; \
                             its run fails for the same cause"
                        );
                        None
                    }
                },
            )
            .collect();
        let fold_rows = decide_fold_rows(&home_offsets, self.alignment)?;
        // The alignment and offset the labels carry, at debug: the
        // declared alignment's, or the single-offset fleet's shared site
        // offset (B-12's local labels).
        match fold_rows {
            FoldRows::Instant(offset) => tracing::debug!(
                alignment = "instant",
                offset_s = offset.local_minus_utc(),
                "the fleet's rows align by the period's start instant, labelled \
                 in that instant's wall clock at the offset"
            ),
            FoldRows::LocalClock => tracing::debug!(
                alignment = "local_clock",
                "the fleet's rows align by the naive local period start, labelled \
                 by the homes' local wall clock"
            ),
        }
        check_time_grid(
            &self.entries,
            &home_offsets,
            self.alignment,
            report.frequency,
        )?;

        // The selection's per-home restriction and the unmatched-name
        // bookkeeping. A catalogue exists only once a home's inputs are
        // parsed, so a Named or Pattern selection probes each home's
        // catalogue as the home's chunk comes up; the check runs when the
        // last home has been built and before its chunk runs.
        let mut matcher = SelectionMatcher::new(&report.selection);

        // The per-home fold alignments, decided by the declared alignment
        // and the pre-pass's offsets, before any home runs.
        let home_alignments: Vec<HomeFoldAlignment> = self
            .entries
            .iter()
            .zip(&home_offsets)
            .map(|(entry, offset)| {
                home_fold_alignment(&entry.config, report.frequency, self.alignment, *offset)
            })
            .collect();

        let mut fold = FleetFold::new(fold_rows);
        let mut homes: Vec<HomeResult> = Vec::with_capacity(total);
        let mut folded_flags: Vec<bool> = Vec::with_capacity(total);
        let mut peak_live = 0usize;
        let mut chunk_start = 0usize;

        // The construction's shared parses: one parse per unique input.
        // The weather and the defaults are the fleet's (the homes share
        // them), the HPXML and the schedule documents one chunk's.
        let shared_parses = hares_io::ParsedInputsCache::new();

        for chunk in self.entries.chunks(chunk_size.get()) {
            let restrictions = matcher.restrictions(chunk, &shared_parses);
            let is_last_chunk = chunk_start + chunk.len() >= total;
            if is_last_chunk && let Some(names) = matcher.unmatched() {
                return Err(FleetError::UnmatchedSelection { names });
            }

            let mut outcomes: Vec<HomeRun> = chunk
                .par_iter()
                .zip(restrictions)
                .zip(&home_alignments[chunk_start..chunk_start + chunk.len()])
                .map(|((entry, restriction), alignment)| {
                    run_home(
                        entry,
                        report,
                        &restriction,
                        alignment,
                        &shared_parses,
                        &ProgressTracker {
                            callback: progress.as_ref(),
                            completed: &completed,
                            total,
                        },
                    )
                })
                .collect();
            shared_parses.evict_documents();

            peak_live = peak_live.max(outcomes.len());
            for (offset, outcome) in outcomes.drain(..).enumerate() {
                let index = chunk_start + offset;
                // A degraded home is never a success: it enters neither
                // the aggregate nor the covered weight, whatever its
                // reduced table holds.
                let folds = outcome.status == HomeStatus::Ok && !outcome.table.is_empty();
                if folds {
                    fold.fold_home(index, outcome.weight, &outcome.fields, &outcome.table)?;
                }
                let status = outcome.status;
                homes.push(HomeResult {
                    bldg_id: outcome.bldg_id,
                    weight: outcome.weight,
                    reason: match &status {
                        HomeStatus::Ok => None,
                        HomeStatus::Degraded(reason) | HomeStatus::Failed(reason) => {
                            Some(reason.clone())
                        }
                    },
                    status,
                    steps: outcome.steps,
                    periods_folded: 0,
                    metrics: outcome.metrics,
                });
                folded_flags.push(folds);
            }
            chunk_start += chunk.len();
        }

        let mut coverage = Coverage::default();
        let mut ok_count = 0usize;
        let mut degraded_count = 0usize;
        let mut failed_count = 0usize;
        for home in &homes {
            coverage.weight_total += home.weight;
            match home.status {
                HomeStatus::Ok => {
                    coverage.weight_ok += home.weight;
                    ok_count += 1;
                }
                HomeStatus::Degraded(_) => {
                    coverage.weight_degraded += home.weight;
                    degraded_count += 1;
                }
                HomeStatus::Failed(_) => {
                    coverage.weight_failed += home.weight;
                    failed_count += 1;
                }
            }
        }

        // A fleet whose ok weight is zero has nothing to fold: the
        // counts per status and the full homes table survive the error.
        if coverage.weight_ok == 0.0 {
            return Err(FleetError::NoCoveredWeight {
                ok: ok_count,
                degraded: degraded_count,
                failed: failed_count,
                homes: homes_batch(&homes),
            });
        }

        let (aggregate, covered_weight, periods) = fold.finish();
        for (home, folds) in homes.iter_mut().zip(folded_flags) {
            if folds {
                home.periods_folded = periods;
            }
        }
        let n_succeeded = ok_count;

        Ok(FleetResult {
            aggregate,
            covered_weight,
            homes,
            coverage,
            n_succeeded,
            peak_live_home_results: peak_live,
        })
    }
}

/// The restricted form of the fleet's selection for one home.
enum Restricted {
    /// The home's restricted selection holds no variable: the home gets
    /// no `fleet` report, folds no value and enters no cell's covered
    /// weight.
    NotApplicable,
    /// The restricted selection, as the report's own column list.
    Names(Vec<String>),
    /// A verbosity selection: the run resolves the home's own tier.
    Verbosity(u8),
}

/// The fleet selection's per-home restriction and the unmatched-name
/// bookkeeping. A `Named` name that no home declares, or a `Pattern` that
/// matches nothing in any home, is the unmatched check's error; a variable
/// a home does not declare is not applicable to it.
struct SelectionMatcher {
    selection: Selection,
    /// Per `Named` name, whether any home declared it.
    named_matched: Vec<bool>,
    /// Whether a `Pattern` matched any home's catalogue.
    pattern_matched: bool,
}

impl SelectionMatcher {
    fn new(selection: &Selection) -> Self {
        let named_matched = match selection {
            Selection::Named(names) => vec![false; names.len()],
            _ => Vec::new(),
        };
        Self {
            selection: selection.clone(),
            named_matched,
            pattern_matched: false,
        }
    }

    /// The per-home restrictions for one chunk. A `Verbosity` selection
    /// restricts to itself (the run resolves the home's own tier); a
    /// `Named` or `Pattern` selection probes the home's catalogue, which
    /// exists only once the home's inputs are parsed.
    fn restrictions(
        &mut self,
        chunk: &[FleetEntry],
        shared_parses: &hares_io::ParsedInputsCache,
    ) -> Vec<Restricted> {
        match &self.selection {
            Selection::Verbosity(verbosity) => chunk
                .iter()
                .map(|_| Restricted::Verbosity(*verbosity))
                .collect(),
            // The diagnostics block rides the home's own run; the fleet's
            // fold reads the report the selection resolves to.
            Selection::Diagnostics => chunk
                .iter()
                .map(|_| Restricted::Names(Vec::new()))
                .collect(),
            Selection::Named(_) | Selection::Pattern(_) => chunk
                .iter()
                .map(|entry| self.restrict_entry(entry, shared_parses))
                .collect(),
        }
    }

    /// Probes one home's catalogue: builds the home's inputs, restricts
    /// the selection to it, and drops the home. A home whose inputs fail
    /// to parse declares nothing; its run fails for the same cause.
    fn restrict_entry(
        &mut self,
        entry: &FleetEntry,
        shared_parses: &hares_io::ParsedInputsCache,
    ) -> Restricted {
        let dwelling = match build_dwelling(&entry.config, shared_parses) {
            Ok(dwelling) => dwelling,
            Err(_) => return Restricted::NotApplicable,
        };
        let catalogue = dwelling.catalogue();
        match &self.selection {
            Selection::Named(names) => {
                let mut kept = Vec::new();
                for (position, name) in names.iter().enumerate() {
                    if catalogue.resolve(name).is_some() {
                        self.named_matched[position] = true;
                        kept.push(name.clone());
                    }
                }
                restricted_from_names(kept)
            }
            Selection::Pattern(pattern) => {
                let matched =
                    hares_io::resolve_selection(&Selection::Pattern(pattern.clone()), &catalogue)
                        .unwrap_or_default();
                if matched.is_empty() {
                    Restricted::NotApplicable
                } else {
                    self.pattern_matched = true;
                    Restricted::Names(matched)
                }
            }
            // Handled by `restrictions`.
            Selection::Verbosity(_) | Selection::Diagnostics => Restricted::NotApplicable,
        }
    }

    /// The selection names that matched no home's catalogue, after the
    /// last home has been built.
    fn unmatched(&self) -> Option<Vec<String>> {
        match &self.selection {
            Selection::Named(names) => {
                let unmatched: Vec<String> = names
                    .iter()
                    .zip(&self.named_matched)
                    .filter(|(_, matched)| !**matched)
                    .map(|(name, _)| name.clone())
                    .collect();
                (!unmatched.is_empty()).then_some(unmatched)
            }
            Selection::Pattern(pattern) => (!self.pattern_matched).then(|| vec![pattern.clone()]),
            Selection::Verbosity(_) | Selection::Diagnostics => None,
        }
    }
}

fn restricted_from_names(kept: Vec<String>) -> Restricted {
    if kept.is_empty() {
        Restricted::NotApplicable
    } else {
        Restricted::Names(kept)
    }
}

/// The distinct site offsets in entry order, over the homes the pre-pass
/// resolved.
fn distinct_offsets(offsets: &[Option<FixedOffset>]) -> Vec<i32> {
    let mut distinct: Vec<i32> = Vec::new();
    for offset in offsets.iter().flatten() {
        let secs = offset.local_minus_utc();
        if !distinct.contains(&secs) {
            distinct.push(secs);
        }
    }
    distinct
}

/// The reference offsets' decision the undeclared fleet makes: the homes'
/// own local calendars and today's instant keys, labelled in the homes'
/// local wall clock (the shared site offset's), B-12's local labels. A
/// fleet whose homes span more than one UTC offset fails here instead:
/// [`FleetError::AlignmentRequired`] naming the distinct offsets, before
/// any home runs.
fn decide_fold_rows(
    offsets: &[Option<FixedOffset>],
    alignment: Option<Alignment>,
) -> Result<FoldRows> {
    match alignment {
        Some(Alignment::Instant(offset)) => Ok(FoldRows::Instant(offset)),
        Some(Alignment::LocalClock) => Ok(FoldRows::LocalClock),
        None => match distinct_offsets(offsets).as_slice() {
            // No home resolved an offset: every run fails for its own
            // cause, and the emptied tables still need a label offset.
            [] => Ok(FoldRows::Instant(
                FixedOffset::east_opt(0).expect("a zero offset is representable"),
            )),
            // The single-offset fleet labels in the homes' own local
            // wall clock: the shared site offset.
            [offset_s] => Ok(FoldRows::Instant(
                FixedOffset::east_opt(*offset_s).expect("the pre-pass offsets are representable"),
            )),
            _ => Err(FleetError::AlignmentRequired {
                offsets_s: distinct_offsets(offsets),
            }),
        },
    }
}

/// One home's fold alignment: the calendar its reducer buckets onto, the
/// key its period rows align by and the partial-period rule's inputs.
/// Under a declared `Instant` alignment the calendar is the declared
/// offset's (the home's steps reduce on the reference boundaries); under
/// `LocalClock`, and for the undeclared single-offset fleet, the home's
/// own local calendar. A home whose pre-pass offset is missing fails its
/// run and folds nothing; its calendar is the home default.
fn home_fold_alignment(
    config: &DwellingConfig,
    report_frequency: Frequency,
    alignment: Option<Alignment>,
    home_offset: Option<FixedOffset>,
) -> HomeFoldAlignment {
    let step_ms = config.settings.sim_config.time_res.num_milliseconds();
    let home_offset_s = home_offset.map_or(0, |offset| offset.local_minus_utc());
    match alignment {
        Some(Alignment::Instant(origin)) => {
            let calendar = match home_offset {
                Some(home) => PeriodCalendar::instant(origin, home.local_minus_utc()),
                None => PeriodCalendar::home(),
            };
            HomeFoldAlignment {
                frequency: report_frequency,
                step_ms,
                calendar,
                key_offset_s: origin.local_minus_utc(),
                instant_aligned: true,
            }
        }
        Some(Alignment::LocalClock) => HomeFoldAlignment {
            frequency: report_frequency,
            step_ms,
            calendar: PeriodCalendar::home(),
            key_offset_s: 0,
            instant_aligned: false,
        },
        None => HomeFoldAlignment {
            frequency: report_frequency,
            step_ms,
            calendar: PeriodCalendar::home(),
            key_offset_s: home_offset_s,
            instant_aligned: false,
        },
    }
}

/// The pre-pass's time-grid check, run before any home runs, by
/// alignment. Under `LocalClock`, and for a single-offset fleet with no
/// declared alignment, the homes share the naive local start, the
/// duration and the resolution. Under an `Instant` alignment the naive
/// starts are not compared: the start instants, the duration and the
/// resolution are, every start instant falls on a step boundary of the
/// declared offset's calendar, and the runs share at least one whole
/// reference period at the report's frequency. The first home, in entry
/// order, that breaks the rule is the error's.
///
/// A home whose pre-pass offset is missing takes no part in the check;
/// its run fails for the same cause.
fn check_time_grid(
    entries: &[FleetEntry],
    offsets: &[Option<FixedOffset>],
    alignment: Option<Alignment>,
    frequency: Frequency,
) -> Result<()> {
    let reference = entries
        .iter()
        .zip(offsets)
        .find(|(_, offset)| offset.is_some());
    let Some((reference_entry, reference_offset)) = reference else {
        // No home resolved an offset; every run fails for its own cause.
        return Ok(());
    };
    let instant_alignment = match alignment {
        Some(Alignment::Instant(offset)) => Some(offset),
        Some(Alignment::LocalClock) | None => None,
    };
    for (entry, offset) in entries.iter().zip(offsets) {
        let Some(offset) = offset else {
            continue;
        };
        let sim = &entry.config.settings.sim_config;
        if sim.duration != reference_entry.config.settings.sim_config.duration
            || sim.time_res != reference_entry.config.settings.sim_config.time_res
        {
            return Err(FleetError::HeterogeneousTimeGrid {
                bldg_id: entry.config.settings.bldg_id,
            });
        }
        match instant_alignment {
            None => {
                if sim.start_time.naive_local()
                    != reference_entry
                        .config
                        .settings
                        .sim_config
                        .start_time
                        .naive_local()
                {
                    return Err(FleetError::HeterogeneousTimeGrid {
                        bldg_id: entry.config.settings.bldg_id,
                    });
                }
            }
            Some(origin) => {
                // The start instant, in the declared offset's wall clock:
                // the naive start reinterpreted at the home's site offset,
                // re-expressed at the declared offset. It must fall on a
                // step boundary of the declared offset's calendar (a whole
                // number of steps from that offset's midnight), so every
                // step lands in one reference period.
                let home_offset_s = offset.local_minus_utc();
                let start_instant = sim.start_time.naive_local()
                    - FixedOffset::east_opt(home_offset_s).expect("a resolved offset");
                let wall_start = origin.from_utc_datetime(&start_instant);
                let step_s = sim.time_res.num_seconds();
                let midnight = wall_start
                    .date_naive()
                    .and_hms_opt(0, 0, 0)
                    .expect("midnight is a real time");
                let from_midnight = (wall_start.naive_local() - midnight).num_seconds();
                if step_s <= 0 || from_midnight % step_s != 0 {
                    return Err(FleetError::HeterogeneousTimeGrid {
                        bldg_id: entry.config.settings.bldg_id,
                    });
                }
                // The runs share at least one whole reference period at
                // the report's frequency, here against the reference
                // home's run.
                let reference_sim = &reference_entry.config.settings.sim_config;
                let reference_offset_s = reference_offset.expect("checked").local_minus_utc();
                let reference_start = reference_sim.start_time.naive_local()
                    - FixedOffset::east_opt(reference_offset_s).expect("a resolved offset");
                let shares = share_whole_reference_period(
                    wall_start.timestamp(),
                    origin
                        .from_utc_datetime(&(start_instant + sim.duration))
                        .timestamp(),
                    origin.from_utc_datetime(&reference_start).timestamp(),
                    origin
                        .from_utc_datetime(&(reference_start + reference_sim.duration))
                        .timestamp(),
                    &frequency,
                );
                if !shares {
                    return Err(FleetError::HeterogeneousTimeGrid {
                        bldg_id: entry.config.settings.bldg_id,
                    });
                }
            }
        }
    }
    Ok(())
}

/// Whether two runs, given their declared-offset wall-clock start and end
/// instants in epoch seconds, share at least one whole reference period
/// at the frequency: the runs' common window holds a whole period of the
/// declared offset's calendar. A `RunPeriod` is one period per run: the
/// runs must be the same window.
fn share_whole_reference_period(
    start_a: i64,
    end_a: i64,
    start_b: i64,
    end_b: i64,
    frequency: &Frequency,
) -> bool {
    let lo = start_a.max(start_b);
    let hi = end_a.min(end_b);
    if hi <= lo {
        return false;
    }
    if matches!(frequency, Frequency::RunPeriod) {
        return start_a == start_b && end_a == end_b;
    }
    // The first reference period that opens at or after `lo`: the floor
    // of `lo`, or the period after it when `lo` opens mid-period. When
    // that period closes by `hi`, a later one cannot either (the next
    // period's start is the earlier's end), so one check decides.
    let mut period = period_floor(lo, frequency);
    if period < lo {
        period = period + period_span(period, frequency);
    }
    period + period_span(period, frequency) <= hi
}

/// The reference period's nominal start containing `wall_s`, on the
/// declared offset's wall clock in epoch seconds (the same flooring the
/// reducer's period start performs on its local milliseconds).
fn period_floor(wall_s: i64, frequency: &Frequency) -> i64 {
    use chrono::Datelike;
    use chrono::NaiveDateTime;
    use chrono::Timelike;
    let Some(naive) = chrono::DateTime::from_timestamp(wall_s, 0).map(|t| t.naive_utc()) else {
        return wall_s;
    };
    let floor: Option<NaiveDateTime> = match frequency {
        Frequency::Timestep | Frequency::RunPeriod => Some(naive),
        Frequency::Minutes(every) => {
            let every = i64::from(every.get());
            let minute = i64::from(naive.minute());
            naive
                .with_minute((minute / every * every) as u32)
                .and_then(|n| n.with_second(0))
                .and_then(|n| n.with_nanosecond(0))
        }
        Frequency::Hourly => naive
            .with_minute(0)
            .and_then(|n| n.with_second(0))
            .and_then(|n| n.with_nanosecond(0)),
        Frequency::Daily => {
            chrono::NaiveDate::from_ymd_opt(naive.year(), naive.month(), naive.day())
                .and_then(|date| date.and_hms_opt(0, 0, 0))
        }
        Frequency::Monthly => chrono::NaiveDate::from_ymd_opt(naive.year(), naive.month(), 1)
            .and_then(|date| date.and_hms_opt(0, 0, 0)),
        Frequency::Annual => chrono::NaiveDate::from_ymd_opt(naive.year(), 1, 1)
            .and_then(|date| date.and_hms_opt(0, 0, 0)),
    };
    floor.unwrap_or(naive).and_utc().timestamp()
}

/// The reference period's span in seconds: the frequency's own span, the
/// calendar month's or year's for the calendar frequencies.
fn period_span(period_start: i64, frequency: &Frequency) -> i64 {
    use chrono::Datelike;
    match frequency {
        Frequency::Timestep | Frequency::RunPeriod => 0,
        Frequency::Minutes(every) => i64::from(every.get()) * 60,
        Frequency::Hourly => 3600,
        Frequency::Daily => 86_400,
        Frequency::Monthly | Frequency::Annual => {
            let Some(naive) =
                chrono::DateTime::from_timestamp(period_start, 0).map(|t| t.naive_utc())
            else {
                return 0;
            };
            let (next_month, next_year) = match frequency {
                Frequency::Monthly => {
                    let month = naive.month();
                    (
                        (if month == 12 { 1 } else { month + 1 }),
                        if month == 12 {
                            naive.year() + 1
                        } else {
                            naive.year()
                        },
                    )
                }
                _ => (1, naive.year() + 1),
            };
            let next = chrono::NaiveDate::from_ymd_opt(next_year, next_month, 1)
                .expect("a real period start")
                .and_hms_opt(0, 0, 0)
                .expect("midnight")
                .and_utc()
                .timestamp();
            next - period_start
        }
    }
}

/// The fleet report's `ReportSpec` for one home: the restricted selection
/// at the `Timestep` frequency, to a callback destination. `None` when the
/// home's restricted selection holds no variable.
fn fleet_report_spec(restriction: &Restricted) -> Option<hares_io::ReportSpec> {
    let selection = match restriction {
        Restricted::NotApplicable => return None,
        Restricted::Names(names) => Selection::Named(names.clone()),
        Restricted::Verbosity(verbosity) => Selection::Verbosity(*verbosity),
    };
    Some(hares_io::ReportSpec {
        name: FLEET_REPORT_NAME.to_string(),
        selection,
        frequency: hares_io::Frequency::Timestep,
        destination: hares_io::Destination::Callback,
    })
}

/// The config's step count: the run's duration tiled by its step. The
/// config's validation rejects a zero duration and one that is not a
/// multiple of the step.
fn configured_steps(config: &SimulationConfig) -> u64 {
    let step = config.time_res.num_seconds();
    if step > 0 {
        u64::try_from(config.duration.num_seconds() / step).unwrap_or(0)
    } else {
        0
    }
}

/// The fleet's construction-time config checks: a home whose config
/// already carries a report named `fleet`, and two homes whose file
/// destinations resolve to the same path.
fn check_home_configs(configs: &[DwellingConfig]) -> Result<()> {
    let mut seen: HashMap<String, i64> = HashMap::new();
    for config in configs {
        for spec in &config.settings.sim_config.reports {
            if spec.name == FLEET_REPORT_NAME {
                return Err(FleetError::ReservedReportName {
                    bldg_id: config.settings.bldg_id,
                });
            }
            if let Some(path) = spec.destination.path() {
                let key = path.to_string_lossy().to_string();
                match seen.get(&key).copied() {
                    Some(first) if first != config.settings.bldg_id => {
                        return Err(FleetError::DuplicateDestination {
                            first,
                            second: config.settings.bldg_id,
                            path: key,
                        });
                    }
                    _ => {
                        seen.insert(key, config.settings.bldg_id);
                    }
                }
            }
        }
    }
    Ok(())
}

/// One home's completed run: the identity, status and metrics the homes
/// table reads, and the reduced period table the fold consumes.
struct HomeRun {
    bldg_id: i64,
    weight: f64,
    status: HomeStatus,
    steps: u64,
    metrics: Option<hares_core::RunMetrics>,
    /// The home's reduced period table, keyed by the period start's
    /// instant (the epoch key), the `Time` column excluded.
    table: BTreeMap<i64, Vec<Option<f64>>>,
    /// The folded variables' fields, from the home's report schema.
    fields: Vec<arrow::datatypes::Field>,
}

/// Runs one home: its own reports plus the fleet's appended `fleet`
/// report, whose batches the home's period reducer reduces and drops. A
/// construction failure, a run error or a panic is that home's `Failed`
/// status; every other home's run completes and is classified by
/// [`home_status`] (`Ok`, or `Degraded` with its reason). The progress
/// callback fires once per completed home, whatever the outcome.
/// The fleet's progress reporting for one simulation: the callback, the
/// completed counter and the total, one struct so the per-home runner
/// takes one progress argument.
struct ProgressTracker<'a> {
    callback: Option<&'a ProgressCallback>,
    completed: &'a AtomicUsize,
    total: usize,
}

impl ProgressTracker<'_> {
    fn record_one(&self) {
        if let Some(cb) = self.callback {
            let done = self.completed.fetch_add(1, Ordering::Relaxed) + 1;
            cb(done, self.total);
        }
    }

    fn is_installed(&self) -> bool {
        self.callback.is_some()
    }
}

fn run_home(
    entry: &FleetEntry,
    report: &FleetReport,
    restriction: &Restricted,
    fold_alignment: &HomeFoldAlignment,
    shared_parses: &hares_io::ParsedInputsCache,
    progress: &ProgressTracker<'_>,
) -> HomeRun {
    let caught = panic::catch_unwind(AssertUnwindSafe(|| {
        run_home_inner(
            entry,
            report,
            restriction,
            fold_alignment,
            shared_parses,
            progress,
        )
    }));
    match caught {
        Ok(run) => run,
        Err(payload) => {
            // Guard against double-panic: the message construction
            // allocates, and the progress callback's completion counter
            // is a relaxed add.
            if progress.is_installed() {
                progress.completed.fetch_add(1, Ordering::Relaxed);
            }
            let handler = panic::catch_unwind(AssertUnwindSafe(|| HomeRun {
                bldg_id: entry.config.settings.bldg_id,
                weight: entry.sample_weight,
                status: HomeStatus::Failed(format!(
                    "panicked: {}",
                    panic_hook::panic_payload_to_string(payload)
                )),
                steps: 0,
                metrics: None,
                table: BTreeMap::new(),
                fields: Vec::new(),
            }));
            match handler {
                Ok(run) => run,
                Err(_) => {
                    record_double_panic_prevented();
                    tracing::error!(
                        bldg_id = entry.config.settings.bldg_id,
                        "CRITICAL: error handling panicked \
                         (double-panic prevented) in fleet::run_home"
                    );
                    HomeRun {
                        bldg_id: entry.config.settings.bldg_id,
                        weight: entry.sample_weight,
                        status: HomeStatus::Failed(
                            "panic handling failed (double-panic prevented)".into(),
                        ),
                        steps: 0,
                        metrics: None,
                        table: BTreeMap::new(),
                        fields: Vec::new(),
                    }
                }
            }
        }
    }
}

fn run_home_inner(
    entry: &FleetEntry,
    report: &FleetReport,
    restriction: &Restricted,
    fold_alignment: &HomeFoldAlignment,
    shared_parses: &hares_io::ParsedInputsCache,
    progress: &ProgressTracker<'_>,
) -> HomeRun {
    let run = run_home_body(entry, report, restriction, fold_alignment, shared_parses);
    progress.record_one();
    run
}

/// The many-home construction's one build: the config's blueprint through
/// the shared parses, one parse per unique input, and the dwelling built
/// from the frozen blueprint.
fn build_dwelling(
    config: &DwellingConfig,
    shared_parses: &hares_io::ParsedInputsCache,
) -> std::result::Result<Dwelling, HaresError> {
    let generation = hares_io::ScheduleGeneration {
        start: config.settings.sim_config.start_time,
        duration: config.settings.sim_config.duration,
        interval: std::time::Duration::from_secs(
            config
                .settings
                .sim_config
                .time_res
                .num_seconds()
                .clamp(1, i64::from(u32::MAX)) as u64,
        ),
    };
    let inputs = shared_parses.parsed_inputs(
        &config.hpxml,
        config.schedule.as_ref(),
        &config.weather,
        &config.defaults,
        Some(generation),
    )?;
    let builder = DwellingBlueprintBuilder::new(inputs, config.settings.clone())?;
    builder.freeze()?.build()
}

fn run_home_body(
    entry: &FleetEntry,
    report: &FleetReport,
    restriction: &Restricted,
    fold_alignment: &HomeFoldAlignment,
    shared_parses: &hares_io::ParsedInputsCache,
) -> HomeRun {
    let failed = |message: String| HomeRun {
        bldg_id: entry.config.settings.bldg_id,
        weight: entry.sample_weight,
        status: HomeStatus::Failed(message),
        steps: 0,
        metrics: None,
        table: BTreeMap::new(),
        fields: Vec::new(),
    };

    let mut config = entry.config.clone();
    // The fleet's health policy is applied to every home's run.
    config.settings.sim_config.health_policy = report.health_policy.clone();
    if let Some(spec) = fleet_report_spec(restriction) {
        config.settings.sim_config.reports.push(spec);
    }
    let steps = configured_steps(&config.settings.sim_config);

    let mut dwelling = match build_dwelling(&config, shared_parses) {
        Ok(dwelling) => dwelling,
        Err(err) => return failed(err.to_string()),
    };
    let has_fleet_report = config
        .settings
        .sim_config
        .reports
        .iter()
        .any(|spec| spec.name == FLEET_REPORT_NAME);
    let state = Arc::new(Mutex::new(HomeFoldState::new(fold_alignment)));
    if has_fleet_report
        && let Err(err) =
            dwelling.attach_callback(FLEET_REPORT_NAME, Box::new(ReducerFeed::new(state.clone())))
    {
        return failed(err.to_string());
    }

    let engine = SimulationEngine::new();
    // The one classification: a run error, a panic turned error and a
    // health-policy violation are all `Failed` with the error; a run
    // that finished carries its warnings and health into the status.
    let run: std::result::Result<RunResult, HaresError> = engine
        .run_dwelling(&mut dwelling, &config.settings.sim_config)
        .map(Arc::unwrap_or_clone);
    let status = home_status(&run);
    let result = match run {
        Ok(result) => result,
        Err(err) => return failed(err.to_string()),
    };
    let (table, fields) = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .finish();
    HomeRun {
        bldg_id: entry.config.settings.bldg_id,
        weight: entry.sample_weight,
        status,
        steps,
        metrics: result.metrics,
        table,
        fields,
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
        #[cfg(debug_assertions)]
        {
            assert!(
                panic_hook::is_installed(),
                "custom panic hook must be installed before dwelling construction"
            );
        }

        // The construction's shared parses: one parse per unique input
        // across the whole steppable fleet.
        let shared_parses = hares_io::ParsedInputsCache::new();

        for config in configs {
            let bldg_id = config.settings.bldg_id;
            let build_result =
                panic::catch_unwind(AssertUnwindSafe(|| build_dwelling(&config, &shared_parses)));
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
                            tracing::error!(
                                bldg_id,
                                "CRITICAL: error handling panicked \
                                 (double-panic prevented) in fleet::SteppableFleet::from_configs"
                            );
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
                causes: build_errors
                    .iter()
                    .map(|e| format!("dwelling {}: {}", e.bldg_id, e.message))
                    .collect(),
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
                causes: build_errors
                    .iter()
                    .map(|e| format!("dwelling {}: {}", e.bldg_id, e.message))
                    .collect(),
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
    ///
    /// A step past the fleet's configured end is one fleet-level error,
    /// [`FleetError::AlreadyFinished`] naming the finished step count: the
    /// fleet finished, no home failed, and no per-home results are
    /// returned.
    ///
    /// The per-dwelling results are built fresh per call (the by-value
    /// form, unchanged for every external caller); a caller that steps in
    /// a loop and holds its results buffer steps through the reused form
    /// ([`Self::step_into`]) instead.
    ///
    /// # Errors
    ///
    /// [`FleetError::AlreadyFinished`] when every configured step has
    /// already run.
    pub fn step(&mut self) -> Result<Vec<std::result::Result<StepResult, SimError>>> {
        let mut results = Vec::new();
        self.step_into(&mut results)?;
        Ok(results)
    }

    /// The by-value [`Self::step`]'s body, writing the per-dwelling
    /// results in place into `results`: the fleet's per-dwelling step
    /// loop steps through the caller's reused results, so a caller that
    /// holds the buffer across steps (the Python fleet binding) pays the
    /// zone-temperature allocation once, at the first step, and the
    /// loop's steady state allocates nothing for the results. `step()`
    /// keeps its by-value signature for every external caller.
    ///
    /// The reuse cannot carry a stale result across a mid-episode
    /// mutation: the fleet's roster is fixed at construction (no
    /// add/remove/restore surface exists on this type), the mutations the
    /// stepping path can hit mid-episode (`apply_control`,
    /// `set_grid_voltage`, a dwelling's panic failure) never read a
    /// slot's old content, and every slot is fully overwritten before any
    /// read on the success path while a failed step leaves the slot
    /// carrying the error instead.
    ///
    /// `results` is resized to the dwelling count: entries beyond the
    /// count are dropped, missing entries are seeded fresh (their first
    /// fill pays the buffer), and entries already in place are reused.
    ///
    /// # Errors
    ///
    /// [`FleetError::AlreadyFinished`] when every configured step has
    /// already run.
    pub fn step_into(
        &mut self,
        results: &mut Vec<std::result::Result<StepResult, SimError>>,
    ) -> Result<()> {
        if self.is_finished() {
            return Err(FleetError::AlreadyFinished {
                total_steps: self.total_steps,
            });
        }

        // The results' shape follows the fleet's fixed dwelling count:
        // already-present entries keep their buffers (the reuse), the
        // missing tail is seeded fresh.
        if results.len() > self.dwellings.len() {
            results.truncate(self.dwellings.len());
        }
        results.resize_with(self.dwellings.len(), || Ok(StepResult::default()));

        // Invariant: no dwelling that was ALREADY failed before this step had
        // step() called on it.  Capture pre-step failed state so that dwellings
        // that become failed during this step (via a panic caught in
        // step_dwellings_parallel) are not asserted — they were not failed when
        // step() was called, and a Panic result is valid for them.
        #[cfg(debug_assertions)]
        let was_already_failed: Vec<bool> = self.dwellings.iter().map(|d| d.failed).collect();

        if let Some(pool) = &self.step_pool {
            pool.install(|| {
                step_dwellings_parallel_into(&mut self.dwellings, results);
            });
        } else {
            step_dwellings_parallel_into(&mut self.dwellings, results);
        }

        #[cfg(debug_assertions)]
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
        Ok(())
    }

    /// Applies a grid voltage override to one dwelling.
    ///
    /// # Errors
    ///
    /// [`FleetError::NoSuchHome`] naming the index and the length when the
    /// index is out of range; the dwelling's own validation error when the
    /// override is not exactly `0.0` or a finite value in `[0.5, 1.5]` pu.
    pub fn set_grid_voltage(&mut self, dwelling_index: usize, voltage_pu: f64) -> Result<()> {
        let len = self.dwellings.len();
        let dwelling = self
            .dwellings
            .get_mut(dwelling_index)
            .ok_or(FleetError::NoSuchHome {
                index: dwelling_index,
                len,
            })?;
        dwelling
            .set_grid_voltage(voltage_pu)
            .map_err(|err| FleetError::InvalidGridVoltage {
                index: dwelling_index,
                message: err.to_string(),
            })
    }

    /// Applies a shared grid voltage override to all dwellings.
    ///
    /// # Errors
    ///
    /// The dwelling validation error of the first home the override fails
    /// at (every home runs the same validation, so one name is enough).
    pub fn set_grid_voltage_all(&mut self, voltage_pu: f64) -> Result<()> {
        for index in 0..self.dwellings.len() {
            self.set_grid_voltage(index, voltage_pu)?;
        }
        Ok(())
    }

    /// Queues a control signal for one dwelling by equipment name.
    ///
    /// # Errors
    ///
    /// [`FleetError::NoSuchHome`] naming the index and the length when the
    /// index is out of range.
    pub fn apply_control(
        &mut self,
        dwelling_index: usize,
        name: &str,
        signal: ControlSignal,
    ) -> Result<()> {
        let len = self.dwellings.len();
        let dwelling = self
            .dwellings
            .get_mut(dwelling_index)
            .ok_or(FleetError::NoSuchHome {
                index: dwelling_index,
                len,
            })?;
        dwelling.apply_control(name, signal);
        Ok(())
    }

    /// Returns telemetry for one dwelling.
    ///
    /// # Errors
    ///
    /// [`FleetError::NoSuchHome`] naming the index and the length when the
    /// index is out of range, and [`FleetError::Telemetry`] naming the
    /// home and the cause when the snapshot cannot be built: some
    /// snapshot-construction conditions are not step failures, so this is
    /// the only place the error would surface.
    pub fn telemetry(&self, dwelling_index: usize) -> Result<DwellingTelemetry> {
        let len = self.dwellings.len();
        let dwelling = self
            .dwellings
            .get(dwelling_index)
            .ok_or(FleetError::NoSuchHome {
                index: dwelling_index,
                len,
            })?;
        dwelling.telemetry().map_err(|err| FleetError::Telemetry {
            bldg_id: dwelling.bldg_id,
            source: Box::new(err),
        })
    }

    /// Returns the building ids of every home, in entry order.
    #[must_use]
    pub fn bldg_ids(&self) -> Vec<i64> {
        self.dwellings.iter().map(|d| d.bldg_id).collect()
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

/// The test-only site accumulator for the fleet's per-dwelling step
/// handoff: each per-dwelling closure brackets exactly the step call
/// with `thread_allocations()` reads and adds the difference here, so
/// the witness can attribute the handoff's allocations to the loop's
/// own site (the counting allocator's counts are per thread, and the
/// loop's closures may run on any worker).
#[cfg(test)]
static FLEET_STEP_HANDOFF_SITE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The stepping tests' exclusivity lock: the coverage build runs the
/// suite in one process on many threads, and the handoff site's
/// accumulator is process-global (the loop's closures run on any rayon
/// worker, so it cannot be thread-local). Every test that steps a fleet
/// holds this guard across its body, so the differential witness's
/// windows count only their own fleet's steps.
#[cfg(test)]
static FLEET_STEP_EXCLUSIVITY: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Resets the handoff site's accumulator. Test-only.
#[cfg(test)]
fn reset_fleet_step_handoff_site() {
    FLEET_STEP_HANDOFF_SITE.store(0, std::sync::atomic::Ordering::Relaxed);
}

/// Reads the handoff site's accumulator. Test-only.
#[cfg(test)]
fn fleet_step_handoff_site() -> u64 {
    FLEET_STEP_HANDOFF_SITE.load(std::sync::atomic::Ordering::Relaxed)
}

/// The fleet's per-dwelling step loop, writing each dwelling's step
/// result in place into its slot of `results`: the slots are the
/// caller's persistent per-dwelling results, so a caller that holds the
/// buffer across steps (the Python fleet binding) pays the
/// zone-temperature allocation once, at the first step, and the loop's
/// steady state allocates nothing for the result. The by-value
/// [`SteppableFleet::step`] hands the same loop a fresh buffer per call,
/// so its per-call allocations are unchanged.
///
/// Every field of a slot's result is overwritten on the success path
/// (the `run_timestep_into` contract), and a slot that does not step
/// successfully carries the error instead, so no mutation point can
/// leak a stale result: the slots are write-only within a step and
/// re-derived before every read.
fn step_dwellings_parallel_into(
    dwellings: &mut [Dwelling],
    results: &mut [std::result::Result<StepResult, SimError>],
) {
    debug_assert_eq!(dwellings.len(), results.len());
    let _guard = PanicHookGuard::new();
    #[cfg(debug_assertions)]
    {
        assert!(
            panic_hook::is_installed(),
            "custom panic hook must be installed before dwelling stepping"
        );
    }

    dwellings
        .par_iter_mut()
        .zip(results.par_iter_mut())
        .for_each(|(dwelling, slot)| {
            if dwelling.failed {
                *slot = Err(SimError::Skipped {
                    bldg_id: dwelling.bldg_id,
                    message: "dwelling permanently failed after prior panic".to_string(),
                });
                return;
            }
            #[cfg(test)]
            let site_before = hares_types::alloc_count::thread_allocations();
            let stepped = panic::catch_unwind(AssertUnwindSafe(|| {
                // The step writes the slot's Ok variant in place: the
                // reused result the loop steps through. A slot holding
                // Err from an earlier failed step has no buffer to
                // reuse; the slot is re-seeded fresh and the new
                // buffer's first fill is that recovery's own cost.
                match slot {
                    Ok(step_result) => dwelling.step_into(step_result),
                    Err(_) => {
                        *slot = Ok(StepResult::default());
                        match slot {
                            Ok(step_result) => dwelling.step_into(step_result),
                            Err(_) => unreachable!("the slot was just re-seeded as Ok"),
                        }
                    }
                }
            }));
            #[cfg(test)]
            if let (Some(before), Some(after)) =
                (site_before, hares_types::alloc_count::thread_allocations())
            {
                FLEET_STEP_HANDOFF_SITE.fetch_add(
                    after.wrapping_sub(before),
                    std::sync::atomic::Ordering::Relaxed,
                );
            }
            match stepped {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    *slot = Err(SimError::Engine {
                        bldg_id: dwelling.bldg_id,
                        message: err.to_string(),
                    });
                }
                Err(payload) => {
                    dwelling.failed = true;
                    let handler_result = panic::catch_unwind(AssertUnwindSafe(|| {
                        Err::<(), SimError>(SimError::Panic {
                            bldg_id: dwelling.bldg_id,
                            message: panic_hook::panic_payload_to_string(payload),
                        })
                    }));
                    let panic_result = match handler_result {
                        Ok(result) => result,
                        Err(_) => {
                            record_double_panic_prevented();
                            tracing::error!(
                                bldg_id = dwelling.bldg_id,
                                "CRITICAL: error handling panicked \
                                 (double-panic prevented) in fleet::step_dwellings_parallel"
                            );
                            Err(SimError::Panic {
                                bldg_id: dwelling.bldg_id,
                                message: "panic handling failed (double-panic prevented)".into(),
                            })
                        }
                    };
                    if let Err(err) = panic_result {
                        *slot = Err(err);
                    }
                }
            }
        });
}

/// The ResStock home's sim config: the constructor's one-minute step and
/// duration, the start and weather-year rule the fleet resolved from
/// the home's weather file and site (`from_resstock`), never a hardcoded
/// year or offset, and the fleet's master seed (the caller's, or the
/// documented default of 0).
fn resstock_sim_config(
    duration: chrono::Duration,
    start_time: DateTime<FixedOffset>,
    weather_year: hares_io::WeatherYear,
    master_seed: u64,
) -> SimulationConfig {
    SimulationConfig {
        start_time,
        duration,
        time_res: Duration::minutes(1),
        reports: Vec::new(),
        diagnostics: false,
        metrics: false,
        output_chunk_size: 10_000,
        setpoint_deadband_c: None,
        master_seed,
        civil_timezone: None,
        site_location: hares_io::SiteLocationOverride::default(),
        max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
        health_policy: hares_io::HealthPolicy::default(),
        first_hour_interpolation: hares_io::FirstHour::default(),
        weather_year,
        warmup: hares_io::Warmup::default(),
    }
}

/// The weather-year rule the release declares (B-1c's `WeatherYear`):
/// 2025.1 ships actual-meteorological-year (AMY 2018) CSV files and
/// declares `Actual`; 2024.x ships typical (TMY3) EPW files and declares
/// `Typical`.
fn resstock_weather_year(version: ResStockVersion) -> hares_io::WeatherYear {
    match version {
        ResStockVersion::V2024_1 | ResStockVersion::V2024_2 => hares_io::WeatherYear::Typical,
        ResStockVersion::V2025_1 => hares_io::WeatherYear::Actual,
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
    // One IECC resolution path for the dwelling and the fleet: the HPXML's
    // declared zone, else the weather station's zone (the OS-HPXML default
    // the dwelling build derives in `apply_climate_zone_default`).
    let station_wmo = hares_io::parse_epw_station_wmo(weather_path);
    let declared_zone = hares_io::parse_iecc_climate_zone(hpxml_path);
    let building_zone = hares_io::hpxml::climate_zone::resolve_iecc_climate_zone(
        declared_zone.as_deref(),
        station_wmo.as_deref(),
    );
    let Some(building_zone) = building_zone else {
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
#[path = "../../../tests/support/fixture_start.rs"]
mod fixture_start;

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::DateTime;
    use hares_control::DispatchRequest;
    use hares_core::{Actor, Provenance, RunHealth};
    use hares_types::{
        Aggregation, EnumLabels, EnvironmentState, Sampling, Telemetry, TelemetryField, Unit,
        VariableKind,
    };
    use std::collections::HashSet;
    use std::fs;
    use std::num::NonZeroU32;
    use std::path::PathBuf;
    use std::sync::{Mutex, PoisonError};
    use std::thread;

    use arrow::array::{
        Array, ArrayRef, Float64Array, Int64Array, StringArray, TimestampMillisecondArray,
    };
    use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
    use arrow::record_batch::RecordBatch;
    use parquet::arrow::arrow_writer::ArrowWriter;
    use std::sync::Arc;
    use tempfile::tempdir;

    /// The parse-count probes (`*_parses_each_unique_input_once`) sample the
    /// process-global `parse_counts` and assert exact deltas: the samples
    /// assume no sibling construction parses inputs between them. nextest's
    /// process isolation gives each probe its own process, where the lock is
    /// uncontended; in-process runners (the coverage job's llvm-cov cargo
    /// test) execute the tests on shared threads, so every test here holds
    /// the lock for its whole body and the samples read the counters alone.
    static PARSE_COUNT_EXCLUSIVITY: Mutex<()> = Mutex::new(());

    /// A root no test creates and a non-root user cannot: every path under it
    /// is missing, and nothing can be written there.
    const MISSING_ROOT: &str = "/nonexistent-hares-fleet-test";

    fn write_temp_file(path: &Path, contents: &str) {
        fs::write(path, contents).expect("failed to write temp file");
    }

    fn build_schedule_csv() -> String {
        // The columns the base fixture's event equipment and wet appliances
        // run on: a schedule file without an appliance's power column is a
        // construction error, and the washer and dishwasher draw their hot
        // water through their hot-water columns. No setpoint column: the
        // base fixture's HPXML declares thermostat setpoints, and a
        // schedule file that carries a setpoint column next to them is the
        // overlap the setpoint rule warns (the OS-HPXML convention), which
        // would degrade the home the fleet status counts.
        [
            "Time,occupants,clothes_washer,clothes_dryer,dishwasher,cooking_range,hot_water_clothes_washer,hot_water_dishwasher",
            "2021-01-01T00:00:00-07:00,1.0,0.10,0.10,0.10,0.10,0.10,0.10",
            "2021-01-01T00:15:00-07:00,1.0,0.20,0.20,0.20,0.20,0.20,0.20",
            "2021-01-01T00:30:00-07:00,1.0,0.30,0.30,0.30,0.30,0.30,0.30",
            "2021-01-01T00:45:00-07:00,1.0,0.40,0.40,0.40,0.40,0.40,0.40",
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

    /// The repo's defaults directory: equipment whose schedule source is
    /// missing (no schedule column, no HPXML fractions) resolves its default
    /// profile there instead of erroring.
    fn repo_defaults_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../python/hares/defaults")
    }

    fn simulation_config(csv_path: PathBuf) -> SimulationConfig {
        SimulationConfig {
            start_time: fixture_start::fixture_start(),
            duration: Duration::hours(1),
            time_res: Duration::minutes(1),
            reports: vec![hares_io::ReportSpec {
                name: "frame".to_string(),
                selection: hares_io::Selection::Verbosity(0),
                frequency: hares_io::Frequency::default(),
                destination: hares_io::Destination::Csv {
                    path: csv_path,
                    overwrite: false,
                },
            }],
            diagnostics: false,
            metrics: false,
            output_chunk_size: 128,
            setpoint_deadband_c: None,
            master_seed: 0,
            civil_timezone: None,
            site_location: hares_io::SiteLocationOverride::default(),
            max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
            health_policy: hares_io::HealthPolicy::default(),
            first_hour_interpolation: hares_io::FirstHour::default(),
            weather_year: hares_io::WeatherYear::default(),
            warmup: hares_io::Warmup::Disabled,
        }
    }

    /// Configs whose inputs and outputs all live in `dir`.
    fn build_valid_configs(dir: &Path, count: usize) -> Vec<DwellingConfig> {
        let schedule_path = dir.join("schedule.csv");
        let weather_path = dir.join("weather.epw");
        write_temp_file(&schedule_path, &build_schedule_csv());
        write_temp_file(&weather_path, &build_epw_8760());

        (0..count)
            .map(|idx| DwellingConfig {
                hpxml: InputSource::Path(fixture_hpxml_path()),
                schedule: Some(InputSource::Path(schedule_path.clone())),
                weather: WeatherInput::Path(weather_path.clone()),
                defaults: DefaultsSource::directory(repo_defaults_path()),
                settings: DwellingSettings::new(
                    simulation_config(dir.join(format!("output-{idx}.csv"))),
                    None,
                    idx as i64 + 1,
                    None,
                ),
            })
            .collect()
    }

    fn build_missing_configs(count: usize) -> Vec<DwellingConfig> {
        (0..count)
            .map(|idx| DwellingConfig {
                hpxml: InputSource::Path(Path::new(MISSING_ROOT).join("hpxml.xml")),
                schedule: Some(InputSource::Path(
                    Path::new(MISSING_ROOT).join("schedule.csv"),
                )),
                weather: WeatherInput::Path(Path::new(MISSING_ROOT).join("weather.epw")),
                defaults: DefaultsSource::directory(repo_defaults_path()),
                settings: DwellingSettings::new(
                    simulation_config(Path::new(MISSING_ROOT).join(format!("output-{idx}.csv"))),
                    None,
                    idx as i64 + 1,
                    None,
                ),
            })
            .collect()
    }

    /// The default fleet report for the unit tests: every variable the
    /// homes' tier 0 declares, folded to quarter hours.
    fn test_report() -> crate::fold::FleetReport {
        crate::fold::FleetReport {
            selection: hares_io::Selection::Verbosity(0),
            frequency: hares_io::Frequency::Minutes(std::num::NonZeroU32::new(15).expect("15")),
            health_policy: hares_io::HealthPolicy::default(),
        }
    }

    #[test]
    fn simulate_runs_three_dwellings() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let dir = tempdir().expect("temp dir");
        let fleet = Fleet::from_buildings(build_valid_configs(dir.path(), 3))
            .expect("the configs carry no reserved report or shared destination");
        let chunk = std::num::NonZeroUsize::new(2).expect("2");
        let result = fleet
            .simulate(&test_report(), 1, chunk)
            .expect("the homes run");

        assert_eq!(result.homes.len(), 3);
        for home in &result.homes {
            assert_eq!(home.status, HomeStatus::Ok, "the fixture homes are clean");
            assert!(home.weight > 0.0);
        }
        assert_eq!(result.n_succeeded, 3);
        assert_eq!(result.peak_live_home_results, 2);
    }

    #[test]
    fn from_buildings_defaults_to_valid_weight() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let fleet = Fleet::from_buildings(build_missing_configs(3))
            .expect("the configs carry no reserved report or shared destination");
        let weights: Vec<f64> = fleet.entries.iter().map(|e| e.sample_weight).collect();
        assert_eq!(weights, vec![1.0, 1.0, 1.0]);
    }

    #[test]
    fn with_sample_weights_accepts_zero_and_positive() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let fleet = Fleet::from_buildings(build_missing_configs(3))
            .expect("the configs carry no reserved report or shared destination")
            .with_sample_weights(vec![1.0, 0.0, 2.5])
            .expect("finite non-negative weights are accepted");
        let weights: Vec<f64> = fleet.entries.iter().map(|e| e.sample_weight).collect();
        assert_eq!(weights, vec![1.0, 0.0, 2.5]);
    }

    #[test]
    fn with_sample_weights_rejects_nan() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let err = Fleet::from_buildings(build_missing_configs(2))
            .expect("the configs carry no reserved report or shared destination")
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
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let err = Fleet::from_buildings(build_missing_configs(2))
            .expect("the configs carry no reserved report or shared destination")
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
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let err = Fleet::from_buildings(build_missing_configs(1))
            .expect("the configs carry no reserved report or shared destination")
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
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let err = Fleet::from_buildings(build_missing_configs(2))
            .expect("the configs carry no reserved report or shared destination")
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
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let dir = tempdir().expect("temp dir");
        let panic_on = 2usize;
        let fleet = Fleet::from_buildings(build_valid_configs(dir.path(), 3))
            .expect("the configs carry no reserved report or shared destination")
            .with_progress(move |done, _total| {
                if done == panic_on {
                    panic!("injected callback panic");
                }
            });

        let chunk = std::num::NonZeroUsize::new(1).expect("1");
        let result = fleet
            .simulate(&test_report(), 3, chunk)
            .expect("the fleet completes");
        assert_eq!(result.homes.len(), 3);

        let panicked = result
            .homes
            .iter()
            .filter(|home| matches!(home.status, HomeStatus::Failed(_)))
            .count();

        let success_count = result.n_succeeded;
        assert_eq!(panicked, 1);
        assert_eq!(success_count, 2);
    }

    #[test]
    fn panic_error_message_includes_file_and_line() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let dir = tempdir().expect("temp dir");
        let panic_on = 2usize;
        let fleet = Fleet::from_buildings(build_valid_configs(dir.path(), 3))
            .expect("the configs carry no reserved report or shared destination")
            .with_progress(move |done, _total| {
                if done == panic_on {
                    panic!("injected callback panic");
                }
            });

        let chunk = std::num::NonZeroUsize::new(1).expect("1");
        let result = fleet
            .simulate(&test_report(), 3, chunk)
            .expect("the fleet completes");
        assert_eq!(result.homes.len(), 3);

        let panic_msg = result
            .homes
            .iter()
            .find_map(|home| match &home.status {
                HomeStatus::Failed(message) => Some(message.clone()),
                _ => None,
            })
            .expect("one home should carry the panic");

        // Verifies the end-to-end chain: PanicHookGuard (installed in calling thread)
        // -> custom hook fires in Rayon worker -> thread-local PANIC_INFO populated
        // -> catch_unwind captures payload -> panic_payload_to_string enriches with file:line.
        assert!(
            panic_msg.contains("fleet.rs:"),
            "panic message should contain file:line location enrichment, got: {panic_msg}"
        );
    }

    #[test]
    fn thread_count_configuration_and_global_pool_path_work() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let thread_ids = Arc::new(Mutex::new(HashSet::new()));
        let thread_ids_for_callback = Arc::clone(&thread_ids);

        // Keep each unit of work non-trivial so Rayon has time to
        // schedule across workers: the homes run one real hour each. An
        // all-failed fleet is the NoCoveredWeight error, so the thread
        // assertions run over valid homes, whose file destinations
        // overwrite: the test reruns the same fleet into one directory.
        let dir = tempdir().expect("temp dir");
        let mut configs = build_valid_configs(dir.path(), 64);
        for config in &mut configs {
            for spec in &mut config.settings.sim_config.reports {
                if let hares_io::Destination::Csv { overwrite, .. } = &mut spec.destination {
                    *overwrite = true;
                }
            }
        }
        let fleet = Fleet::from_buildings(configs)
            .expect("the configs carry no reserved report or shared destination")
            .with_progress(move |_done, _total| {
                let id = thread::current().id();
                thread_ids_for_callback
                    .lock()
                    .expect("lock thread id set")
                    .insert(id);
                std::thread::sleep(std::time::Duration::from_millis(2));
            });

        let chunk = std::num::NonZeroUsize::new(8).expect("8");
        let report = test_report();
        let sequential = fleet
            .simulate(&report, 1, chunk)
            .expect("the fleet completes");
        assert_eq!(sequential.homes.len(), 64);
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

            let parallel = fleet
                .simulate(&report, 4, chunk)
                .expect("the fleet completes");
            assert_eq!(parallel.homes.len(), 64);

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

        let global_pool = fleet
            .simulate(&report, 0, chunk)
            .expect("the fleet completes");
        assert_eq!(global_pool.homes.len(), 64);
    }

    #[test]
    fn progress_callback_is_invoked_and_can_lock_shared_state() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let progress_count = Arc::new(AtomicUsize::new(0));
        let progress_count_cb = Arc::clone(&progress_count);
        let lock = Arc::new(Mutex::new(()));
        let lock_cb = Arc::clone(&lock);

        let dir = tempdir().expect("temp dir");
        let fleet = Fleet::from_buildings(build_valid_configs(dir.path(), 8))
            .expect("the configs carry no reserved report or shared destination")
            .with_progress(move |_done, _total| {
                let _guard = lock_cb.lock().expect("lock in callback");
                progress_count_cb.fetch_add(1, Ordering::Relaxed);
            });

        let chunk = std::num::NonZeroUsize::new(2).expect("2");
        let _ = fleet
            .simulate(&test_report(), 2, chunk)
            .expect("the fleet completes");
        assert!(progress_count.load(Ordering::Relaxed) > 0);
    }

    #[test]
    fn weather_filename_matches_dataset_conventions() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
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
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
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
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
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
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
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
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        // Even if the path doesn't exist, a .zip extension should be rejected.
        assert!(!path_is_readable_weather_file(Path::new("something.zip")));
    }

    #[test]
    fn resolve_weather_path_matches_fixture_files() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
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
        let _step_exclusive = FLEET_STEP_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let dir = tempdir().expect("temp dir");
        let (mut fleet, build_errors) =
            SteppableFleet::from_configs(build_valid_configs(dir.path(), 3), 2)
                .expect("build steppable fleet");
        assert!(build_errors.is_empty());
        assert_eq!(fleet.len(), 3);
        assert!(!fleet.is_finished());
        assert_eq!(fleet.current_step(), 0);
        assert!(fleet.total_steps() > 0);
        assert!(fleet.time_res_s() > 0.0);

        let first_step = fleet.step().expect("steps run");
        assert_eq!(first_step.len(), 3);
        assert!(first_step.iter().all(|result| result.is_ok()));
        for result in first_step {
            let step = result.expect("step succeeds");
            assert!(step.timestamp.and_utc().timestamp() > 0);
        }
        assert_eq!(fleet.current_step(), 1);
        assert!(!fleet.is_finished());

        while !fleet.is_finished() {
            let step = fleet.step().expect("steps run to the end");
            assert_eq!(step.len(), 3);
            assert!(step.iter().all(|result| result.is_ok()));
        }

        assert!(fleet.is_finished());
        assert_eq!(fleet.current_step(), fleet.total_steps());
    }

    /// Stepping a finished fleet is one fleet-level error naming the
    /// finished step count, not one failure per home: the fleet did not
    /// fail, it finished.
    #[test]
    fn step_after_finish_is_already_finished_error() {
        let _step_exclusive = FLEET_STEP_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let dir = tempdir().expect("temp dir");
        let (mut fleet, build_errors) =
            SteppableFleet::from_configs(build_valid_configs(dir.path(), 3), 2)
                .expect("build steppable fleet");
        assert!(build_errors.is_empty());

        while !fleet.is_finished() {
            let step = fleet.step().expect("steps run to the end");
            assert!(step.iter().all(|result| result.is_ok()));
        }

        let total_steps = fleet.total_steps();
        let err = fleet
            .step()
            .expect_err("a step past the end is the fleet's error");
        assert!(
            matches!(err, FleetError::AlreadyFinished { total_steps: finished } if finished == total_steps),
            "got: {err:?}"
        );
    }

    /// The fleet's step-handoff zone-temperature allocation, the witness
    /// for the census site the P1 record names (the by-value `step()`
    /// handoff at the fleet's per-dwelling step loop): each dwelling-step
    /// paid one fresh `StepResult`, one zone-temperature vector. The
    /// loop steps through the caller's reused results (the into-form),
    /// so the reused form's steady state pays nothing for the result.
    ///
    /// The site accumulator (`FLEET_STEP_HANDOFF_SITE`) brackets exactly
    /// the step call inside the per-dwelling closure, so the counts
    /// attribute to the handoff alone; the counting allocator is
    /// per-thread and the loop's closures may run on any worker, so the
    /// brackets feed one shared accumulator.
    ///
    /// The step's own allocations (every phase but the result's fill)
    /// are identical between the two legs: two fleets built from
    /// identical configs, stepped through the same step indices in the
    /// same sitting, the only difference between the legs being the
    /// result buffer the step writes into. The assertion is therefore
    /// exact: the by-value leg's site total exceeds the reused leg's by
    /// one zone-temperature vector per dwelling-step, and the reused
    /// leg's own per-step delta is flat across its steps. RED with the
    /// into-form delegating to the by-value body: the difference read
    /// zero (126 and 126 over the 8 dwelling-steps) and every
    /// dwelling-step still paid its fresh result. GREEN at the commit:
    /// the difference is exactly the dwelling-steps' vectors.
    #[test]
    fn fleet_step_handoff_zone_temperature_allocation_free_after_first_step() {
        let _step_exclusive = FLEET_STEP_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let dwellings_per_fleet = 2;
        let measured_steps = 4u64;
        let dir = tempdir().expect("temp dir");

        // One thread so the two legs' work runs the same closures on the
        // same worker; the site's brackets are self-consistent on any
        // thread either way.
        std::fs::create_dir_all(dir.path().join("by-value")).expect("the by-value config dir");
        std::fs::create_dir_all(dir.path().join("reused")).expect("the reused config dir");
        let by_value_configs =
            build_valid_configs(&dir.path().join("by-value"), dwellings_per_fleet);
        let reused_configs = build_valid_configs(&dir.path().join("reused"), dwellings_per_fleet);
        let (mut by_value_fleet, build_errors) =
            SteppableFleet::from_configs(by_value_configs, 1).expect("build the by-value fleet");
        let (mut reused_fleet, reused_build_errors) =
            SteppableFleet::from_configs(reused_configs, 1).expect("build the reused fleet");
        assert!(build_errors.is_empty());
        assert!(reused_build_errors.is_empty());

        // The reused leg's persistent results: the shape the Python fleet
        // binding steps in.
        let mut reused_results: Vec<std::result::Result<StepResult, SimError>> = Vec::new();

        // The first step pays every first-ever allocation (the reused
        // buffers' first fill included); the measured window follows it.
        by_value_fleet.step().expect("the by-value first step");
        reused_fleet
            .step_into(&mut reused_results)
            .expect("the reused first step");

        // Leg 1: the by-value form, a fresh result per dwelling-step.
        reset_fleet_step_handoff_site();
        for _ in 0..measured_steps {
            by_value_fleet.step().expect("the by-value steps run");
        }
        let by_value_site = fleet_step_handoff_site();

        // Leg 2: the reused form, the same step indices over an identical
        // fleet, the same site bracket.
        reset_fleet_step_handoff_site();
        for _ in 0..measured_steps {
            reused_fleet
                .step_into(&mut reused_results)
                .expect("the reused steps run");
        }
        let reused_site = fleet_step_handoff_site();

        let dwelling_steps = measured_steps * dwellings_per_fleet as u64;
        assert_eq!(
            by_value_site,
            reused_site + dwelling_steps,
            "the by-value handoff allocated {} times beyond the reused form's {} \
             over {dwelling_steps} dwelling-steps (expected exactly one \
             zone-temperature vector, {dwelling_steps}, per dwelling-step): \
             the loop must step through the reused result",
            by_value_site.saturating_sub(reused_site),
            reused_site,
        );

        // The reused form's per-step delta is flat: the legs' step
        // indices line up, so the reused leg's window carries no
        // first-fill or growth of its own beyond the step's own
        // allocations.
        reset_fleet_step_handoff_site();
        reused_fleet
            .step_into(&mut reused_results)
            .expect("the reused flatness step");
        let one_step = fleet_step_handoff_site();
        reset_fleet_step_handoff_site();
        reused_fleet
            .step_into(&mut reused_results)
            .expect("the reused flatness step");
        let next_step = fleet_step_handoff_site();
        assert_eq!(
            one_step, next_step,
            "the reused form's handoff site moved between steps ({one_step} then \
             {next_step}): the step result's buffer must be reused, not grown",
        );
    }

    /// The P1 reviewer's lockstep shape: two identical fleets, one
    /// stepped through the by-value form and one through the reused
    /// results, steps side by side, every per-dwelling `StepResult`
    /// content-identical every step.
    #[test]
    fn fleet_reused_results_match_the_by_value_form_step_for_step() {
        let _step_exclusive = FLEET_STEP_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let dir = tempdir().expect("temp dir");
        let (mut by_value_fleet, by_value_build_errors) =
            SteppableFleet::from_configs(build_valid_configs(dir.path(), 2), 2)
                .expect("build the by-value fleet");
        let (mut reused_fleet, reused_build_errors) =
            SteppableFleet::from_configs(build_valid_configs(dir.path(), 2), 2)
                .expect("build the reused fleet");
        assert!(by_value_build_errors.is_empty());
        assert!(reused_build_errors.is_empty());

        let mut reused_results: Vec<std::result::Result<StepResult, SimError>> = Vec::new();
        while !by_value_fleet.is_finished() {
            let by_value = by_value_fleet.step().expect("the by-value steps run");
            reused_fleet
                .step_into(&mut reused_results)
                .expect("the reused steps run");
            assert_eq!(
                by_value.len(),
                reused_results.len(),
                "the two forms return one result per dwelling"
            );
            for (index, (by_value_entry, reused_entry)) in
                by_value.iter().zip(reused_results.iter()).enumerate()
            {
                assert_eq!(
                    by_value_entry, reused_entry,
                    "dwelling {index}: the reused form's step result must be \
                     content-identical to the by-value form's"
                );
            }
        }
    }

    /// The reuse cannot carry a stale result across a mid-episode
    /// mutation. The mutations the fleet stepping path can hit
    /// mid-episode are a grid-voltage override (the electrical
    /// solution's input changes under the dwelling) and a dwelling's
    /// panic failure (the dwelling leaves the stepping path); the
    /// fleet's roster itself is fixed at construction, with no
    /// add/remove/restore surface. The proof is the by-value form
    /// itself: two identical fleets step the same episode side by side,
    /// the mutations applied to both at the same steps, and every slot
    /// of the reused form must stay content-identical to the by-value
    /// form's fresh result every step, the failed dwelling's slot
    /// carrying the panic and then the skip instead of any old content.
    #[test]
    fn fleet_reused_results_carry_nothing_across_a_mid_episode_mutation() {
        let _step_exclusive = FLEET_STEP_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let dir = tempdir().expect("temp dir");
        let (mut by_value_fleet, build_errors) =
            SteppableFleet::from_configs(build_valid_configs(dir.path(), 2), 1)
                .expect("build the by-value fleet");
        let (mut reused_fleet, reused_build_errors) =
            SteppableFleet::from_configs(build_valid_configs(dir.path(), 2), 1)
                .expect("build the reused fleet");
        assert!(build_errors.is_empty());
        assert!(reused_build_errors.is_empty());

        let mut reused_results: Vec<std::result::Result<StepResult, SimError>> = Vec::new();
        let mut step_index = 0u64;
        while !by_value_fleet.is_finished() {
            // The mutations, applied to both fleets at the same steps:
            // the grid-voltage override after the first step; a
            // dwelling's panic failure after the third (debug builds
            // only, the hook the panic tests use).
            step_index += 1;
            if step_index == 2 {
                by_value_fleet
                    .set_grid_voltage_all(1.5)
                    .expect("the by-value override applies");
                reused_fleet
                    .set_grid_voltage_all(1.5)
                    .expect("the reused override applies");
            }
            #[cfg(debug_assertions)]
            if step_index == 4 {
                by_value_fleet.dwellings[0].set_test_panic();
                reused_fleet.dwellings[0].set_test_panic();
            }

            let by_value = by_value_fleet.step().expect("the by-value steps run");
            reused_fleet
                .step_into(&mut reused_results)
                .expect("the reused steps run");
            assert_eq!(
                by_value.len(),
                reused_results.len(),
                "the two forms return one result per dwelling"
            );
            for (index, (by_value_entry, reused_entry)) in
                by_value.iter().zip(reused_results.iter()).enumerate()
            {
                assert_eq!(
                    by_value_entry, reused_entry,
                    "dwelling {index}, step {step_index}: the reused form's \
                     slot must stay content-identical to the by-value \
                     form's fresh result across the episode's mutations"
                );
            }
        }

        // The failed dwelling's slot carries the panic and then the
        // skip: never a stale step result.
        #[cfg(debug_assertions)]
        {
            assert!(
                matches!(&reused_results[0], Err(SimError::Panic { .. }))
                    || matches!(&reused_results[0], Err(SimError::Skipped { .. })),
                "the failed dwelling's slot must carry the error states, got {:?}",
                reused_results[0]
            );
        }
    }

    /// The per-home accessors reject a missing home with
    /// `NoSuchHome` naming the index and the length, instead of logging
    /// and returning or reporting `None`.
    #[test]
    fn steppable_fleet_accessors_reject_a_missing_home() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let dir = tempdir().expect("temp dir");
        let (mut fleet, build_errors) =
            SteppableFleet::from_configs(build_valid_configs(dir.path(), 2), 2)
                .expect("build steppable fleet");
        assert!(build_errors.is_empty());
        let len = fleet.len();

        let telemetry_err = fleet
            .telemetry(len)
            .expect_err("telemetry past the end is an error");
        assert!(
            matches!(telemetry_err, FleetError::NoSuchHome { index, len: l } if index == len && l == len),
            "got: {telemetry_err:?}"
        );

        let voltage_err = fleet
            .set_grid_voltage(len, 1.0)
            .expect_err("set_grid_voltage past the end is an error");
        assert!(
            matches!(voltage_err, FleetError::NoSuchHome { index, len: l } if index == len && l == len),
            "got: {voltage_err:?}"
        );

        let control_err = fleet
            .apply_control(len, "grid", ControlSignal::thermal_release())
            .expect_err("apply_control past the end is an error");
        assert!(
            matches!(control_err, FleetError::NoSuchHome { index, len: l } if index == len && l == len),
            "got: {control_err:?}"
        );
    }

    #[test]
    fn steppable_fleet_from_configs_returns_partial_success() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let dir = tempdir().expect("temp dir");
        let mut configs = build_valid_configs(dir.path(), 2);
        let mut bad = build_missing_configs(1);
        bad[0].settings.bldg_id = 999;
        configs.extend(bad);

        let (mut fleet, build_errors) =
            SteppableFleet::from_configs(configs, 0).expect("partial success should build");
        assert_eq!(fleet.len(), 2);
        assert_eq!(build_errors.len(), 1);
        assert_eq!(build_errors[0].bldg_id, 999);
        assert!(!build_errors[0].message.is_empty());

        let results = fleet.step().expect("steps run");
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|result| result.is_ok()));
    }

    #[test]
    fn steppable_fleet_empty_configs_returns_error() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
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
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let bad_configs = build_missing_configs(3);
        let result = SteppableFleet::from_configs(bad_configs, 0);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("all dwellings failed"));
        assert!(err_msg.contains("3 failure"));
    }

    #[test]
    fn steppable_fleet_skips_failed_dwelling() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let dir = tempdir().expect("temp dir");
        let (mut fleet, build_errors) =
            SteppableFleet::from_configs(build_valid_configs(dir.path(), 3), 2)
                .expect("build steppable fleet");
        assert!(build_errors.is_empty());
        assert_eq!(fleet.len(), 3);

        // Mark dwelling 1 as permanently failed (simulating post-panic state).
        fleet.dwellings[1].failed = true;

        // Step: dwellings 0 and 2 should succeed, dwelling 1 should be skipped.
        let results = fleet.step().expect("steps run");
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
        let results2 = fleet.step().expect("steps run");
        assert!(
            matches!(results2[1], Err(SimError::Skipped { .. })),
            "dwelling 1 should stay skipped on every subsequent step"
        );
    }

    #[test]
    fn steppable_fleet_ipc_isolation_after_failure() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let dir = tempdir().expect("temp dir");
        // Integration test: verify that a failed dwelling does not prevent
        // other dwellings from producing valid results over a full simulation.
        let (mut fleet, build_errors) =
            SteppableFleet::from_configs(build_valid_configs(dir.path(), 3), 2)
                .expect("build steppable fleet");
        assert!(build_errors.is_empty());
        assert_eq!(fleet.len(), 3);

        // Mark dwelling 1 as failed at the first step.
        fleet.dwellings[1].failed = true;

        let mut ok_steps = [0usize; 3];
        let mut skipped_steps = [0usize; 3];

        while !fleet.is_finished() {
            let results = fleet.step().expect("steps run");
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
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let dir = tempdir().expect("temp dir");
        // Regression test: a dwelling that fails during from_config is
        // never added to the fleet, so it cannot be marked as a runtime
        // failure.  The AssertUnwindSafe wrapping at from_configs line 343
        // is sound because construction failure (whether error or panic)
        // prevents fleet inclusion.
        let mut configs = build_valid_configs(dir.path(), 2);
        let mut bad = build_missing_configs(1);
        bad[0].settings.bldg_id = 999;
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
        let results = fleet.step().expect("steps run");
        assert_eq!(results.len(), 2);
        assert!(
            results.iter().all(|r| r.is_ok()),
            "valid dwellings should step without error"
        );
    }

    #[cfg(debug_assertions)]
    #[test]
    fn dwelling_panic_during_step_is_caught_and_marks_dwelling_failed() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let dir = tempdir().expect("temp dir");
        // End-to-end test of the panic→failed→skipped path in
        // step_dwellings_parallel.  A dwelling configured to panic during
        // step() must:
        //   (a) produce Err(SimError::Panic) from the panicking step
        //   (b) produce Err(SimError::Skipped) on every subsequent step
        //   (c) leave other dwellings unaffected
        // The invariant check in SteppableFleet::step() must NOT misfire
        // when the dwelling becomes failed during this step.
        let (mut fleet, build_errors) =
            SteppableFleet::from_configs(build_valid_configs(dir.path(), 3), 2)
                .expect("build steppable fleet");
        assert!(build_errors.is_empty());
        assert_eq!(fleet.len(), 3);

        // Configure dwelling 1 to panic on its next step() call.
        // set_test_panic is #[cfg(debug_assertions)] gated — available in
        // test builds.
        fleet.dwellings[1].set_test_panic();

        // First step: dwelling 1 panics, the panic is caught by
        // step_dwellings_parallel, failed is set to true.
        let results = fleet.step().expect("steps run");
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
        let results2 = fleet.step().expect("steps run");
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
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let dir = tempdir().expect("temp dir");
        // Verifies the end-to-end chain when equipment/dwelling code panics
        // via assert! (as opposed to panic!()). The hook must capture
        // PanicHookInfo::location() from the assert's expansion site
        // (the dwelling source file), not from the catch_unwind call site
        // in fleet code.
        let (mut fleet, build_errors) =
            SteppableFleet::from_configs(build_valid_configs(dir.path(), 3), 2)
                .expect("build steppable fleet");
        assert!(build_errors.is_empty());
        assert_eq!(fleet.len(), 3);

        fleet.dwellings[1].set_test_assert_panic();

        let results = fleet.step().expect("steps run");
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
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let dir = tempdir().expect("temp dir");
        // Integration test: verify that when one dwelling panics in a
        // steppable fleet, the process does not abort and the other
        // dwellings produce valid results for every remaining step.
        // This validates that the outer catch_unwind in step_dwellings_parallel
        // correctly isolates per-dwelling panics at fleet scale.
        let (mut fleet, build_errors) =
            SteppableFleet::from_configs(build_valid_configs(dir.path(), 3), 2)
                .expect("build steppable fleet");
        assert!(build_errors.is_empty());
        assert_eq!(fleet.len(), 3);

        // Trigger panic in dwelling 1 on step 0.
        fleet.dwellings[1].set_test_panic();

        // Step 0: dwelling 1 panics, the fleet survives.
        let results = fleet.step().expect("steps run");
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
            let step_results = fleet.step().expect("steps run");
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
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
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
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let dir = tempdir().expect("temp dir");
        let hpxml_path = dir.path().join("home.xml");
        write_temp_file(&hpxml_path, &build_hpxml_with_zone("5B"));

        let epw_path = dir.path().join("weather.epw");
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

        // Missing zone: the weather station's zone is derived (one IECC
        // resolution path for the dwelling and the fleet), so the derived
        // 5B matches CO and the validation proceeds.
        let no_zone_xml = r#"<?xml version='1.0'?>
<HPXML xmlns='http://hpxmlonline.com/2023/09' schemaVersion='4.0'>
  <Building/>
</HPXML>"#;
        write_temp_file(&hpxml_path, no_zone_xml);
        assert_eq!(
            validate_fleet_building_zone(&hpxml_path, &epw_path, 3),
            ZoneMatchStatus::Match {
                building_zone: "5B".to_string(),
                weather_state: "CO".to_string()
            }
        );

        // A station the OS-HPXML table does not carry derives no zone: skipped.
        let no_wmo_epw = dir.path().join("no-wmo.epw");
        write_temp_file(
            &no_wmo_epw,
            "LOCATION,USA_CO_Unknown,XZ,USA,TMY3,,39.83,-104.65,-7.0,1609.0",
        );
        assert_eq!(
            validate_fleet_building_zone(&hpxml_path, &no_wmo_epw, 5),
            ZoneMatchStatus::Skipped
        );

        // Non-EPW extension -> skipped (state parse returns None)
        let csv_path = dir.path().join("weather.csv");
        write_temp_file(&csv_path, "not,an,epw");
        assert_eq!(
            validate_fleet_building_zone(&hpxml_path, &csv_path, 4),
            ZoneMatchStatus::Skipped
        );
    }

    /// The 2024.2 metadata schema's one-row parquet (the 2024.2 column
    /// names: `building_id`, `sample_weight`, `upgrade`), weight 1.0.
    fn write_resstock_parquet_v2024_2(path: &Path, bldg_id: i64) {
        let schema = Arc::new(Schema::new(vec![
            Field::new("building_id", DataType::Int64, false),
            Field::new("sample_weight", DataType::Float64, false),
            Field::new("upgrade", DataType::Int64, false),
            Field::new("in.state", DataType::Utf8, false),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int64Array::from(vec![bldg_id])) as ArrayRef,
                Arc::new(Float64Array::from(vec![1.0])) as ArrayRef,
                Arc::new(Int64Array::from(vec![0])) as ArrayRef,
                Arc::new(StringArray::from(vec!["CO"])) as ArrayRef,
            ],
        )
        .expect("batch");
        let file = std::fs::File::create(path).expect("create parquet");
        let mut writer = ArrowWriter::try_new(file, batch.schema(), None).expect("writer");
        writer.write(&batch).expect("write");
        writer.close().expect("close");
    }

    fn write_resstock_parquet_v2025(path: &Path, bldg_id: i64) {
        let schema = Arc::new(Schema::new(vec![
            Field::new("building_id", DataType::Int64, false),
            Field::new("weight", DataType::Float64, false),
            Field::new("upgrade_id", DataType::Int64, false),
            Field::new("in.state", DataType::Utf8, false),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int64Array::from(vec![bldg_id])) as ArrayRef,
                Arc::new(Float64Array::from(vec![1.0])) as ArrayRef,
                Arc::new(Int64Array::from(vec![0])) as ArrayRef,
                Arc::new(StringArray::from(vec!["CO"])) as ArrayRef,
            ],
        )
        .expect("batch");
        let file = std::fs::File::create(path).expect("create parquet");
        let mut writer = ArrowWriter::try_new(file, batch.schema(), None).expect("writer");
        writer.write(&batch).expect("write");
        writer.close().expect("close");
    }

    /// Write one building in the extracted layout the constructor requires:
    /// `root/bldg{bldg_id:07}/home.xml` and `in.schedules.csv`. The home.xml
    /// carries the `BuildingSummary/Site` (the site's elevation, latitude,
    /// longitude and standard-time UTC offset) the fleet's site-offset
    /// resolution reads, as the released ResStock HPXML documents do.
    fn write_extracted_building(root: &Path, bldg_id: i64, weather_station_name: &str) {
        let dir = root.join(format!("bldg{bldg_id:07}"));
        fs::create_dir_all(&dir).expect("create building dir");
        write_temp_file(
            &dir.join("home.xml"),
            &format!(
                r#"<?xml version='1.0' encoding='UTF-8'?>
<HPXML xmlns='http://hpxmlonline.com/2023/09' schemaVersion='4.0'>
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site>
          <Elevation>30.0</Elevation>
          <Latitude>41.77</Latitude>
          <Longitude>-72.69</Longitude>
          <TimeZone>
            <UTCOffset>-5.0</UTCOffset>
          </TimeZone>
        </Site>
      </BuildingSummary>
      <ClimateandRiskZones>
        <WeatherStation>
          <SystemIdentifier id='WeatherStation'/>
          <Name>{weather_station_name}</Name>
        </WeatherStation>
      </ClimateandRiskZones>
    </BuildingDetails>
  </Building>
</HPXML>"#
            ),
        );
        write_temp_file(&dir.join("in.schedules.csv"), "occupants\n1\n");
    }

    fn resstock_fixtures_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/resstock")
    }

    /// The constructor's report counts the parsed buildings, the built
    /// dwellings and the resolved weather files of one build.
    #[test]
    fn fleet_report_counts_the_resstock_construction() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let tmp = tempdir().expect("tmp");
        let pq = tmp.path().join("test.parquet");
        write_resstock_parquet_v2025(&pq, 19_694);
        let fixtures = resstock_fixtures_root().join("2025.1");

        let fleet = Fleet::from_resstock(
            &pq,
            &fixtures,
            &fixtures.join("weather"),
            Some(ResStockVersion::V2025_1),
            None,
            None,
            None,
            &repo_defaults_path(),
            None,
        )
        .expect("fleet construction");

        let report = fleet
            .build_report()
            .expect("a ResStock build records its report");
        assert_eq!(report.buildings_parsed, 1, "one metadata row");
        assert_eq!(report.dwellings_built, 1, "one built dwelling");
        assert_eq!(
            report.weather_resolved, 1,
            "the building's HPXML station resolves the weather file"
        );
        assert_eq!(report.weather_failed, 0);
    }

    /// A fleet built from explicit configs parses no metadata and resolves
    /// no weather files, so it carries no construction report.
    #[test]
    fn from_buildings_carries_no_construction_report() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let config = DwellingConfig {
            hpxml: InputSource::Path(PathBuf::from("no.xml")),
            schedule: None,
            weather: WeatherInput::Path(PathBuf::from("no.epw")),
            defaults: DefaultsSource::directory(repo_defaults_path()),
            settings: DwellingSettings::new(
                resstock_sim_config(
                    Duration::hours(24),
                    FixedOffset::east_opt(0)
                        .expect("valid offset")
                        .with_ymd_and_hms(2018, 1, 1, 0, 0, 0)
                        .single()
                        .expect("valid start"),
                    hares_io::WeatherYear::Actual,
                    0,
                ),
                None,
                1,
                None,
            ),
        };
        let fleet = Fleet::from_buildings(vec![config])
            .expect("the configs carry no reserved report or shared destination");
        assert!(fleet.build_report().is_none());
        assert_eq!(fleet.bldg_ids(), vec![1], "the identities stay readable");
    }

    #[test]
    fn from_resstock_default_duration() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let tmp = tempdir().expect("tmp");
        let pq = tmp.path().join("test.parquet");
        write_resstock_parquet_v2025(&pq, 1);
        write_extracted_building(tmp.path(), 1, "G0900090");
        // The building's weather resolves against the committed weather
        // directory: the constructor requires every building's weather
        // file to resolve and read.
        let weather = resstock_fixtures_root().join("2025.1/weather");

        let fleet = Fleet::from_resstock(
            &pq,
            tmp.path(),
            &weather,
            Some(ResStockVersion::V2025_1),
            None,
            None,
            None,
            &repo_defaults_path(),
            None,
        )
        .expect("fleet construction");

        assert!(
            !fleet.entries.is_empty(),
            "fleet should have at least one entry"
        );
        for entry in &fleet.entries {
            assert_eq!(
                entry.config.settings.sim_config.duration,
                Duration::hours(24),
                "default duration should be 24 hours"
            );
        }
    }

    /// The ResStock constructor's `defaults_path` reaches every dwelling it
    /// configures, built through the public path: the committed fixture
    /// building's own HPXML, schedule and weather, with no swapped-in files.
    /// The constructor's defaults argument is required: the dwelling's
    /// config records the directory given, and the dwelling builds on it.
    #[test]
    fn from_resstock_passes_the_defaults_directory_to_each_dwelling() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let tmp = tempdir().expect("tmp");
        let pq = tmp.path().join("test.parquet");
        // bldg0019694 is a committed fixture building whose weather station
        // (G5300330) has a fixture weather file.
        write_resstock_parquet_v2025(&pq, 19_694);
        let fixtures = resstock_fixtures_root().join("2025.1");
        let defaults = repo_defaults_path();

        let fleet = Fleet::from_resstock(
            &pq,
            &fixtures,
            &fixtures.join("weather"),
            Some(ResStockVersion::V2025_1),
            None,
            None,
            None,
            &defaults,
            None,
        )
        .expect("fleet construction");
        assert_eq!(fleet.entries.len(), 1);

        let with_defaults = fleet.entries[0].config.clone();
        assert_eq!(
            with_defaults.defaults.base, defaults,
            "the constructor passes its defaults directory to every dwelling"
        );
        if let Err(err) = Dwelling::from_config(with_defaults) {
            panic!("a dwelling given the defaults directory must build, got: {err}");
        }
    }

    #[test]
    fn from_resstock_with_custom_duration() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let tmp = tempdir().expect("tmp");
        let pq = tmp.path().join("test.parquet");
        write_resstock_parquet_v2025(&pq, 1);
        write_extracted_building(tmp.path(), 1, "G0900090");
        let weather = resstock_fixtures_root().join("2025.1/weather");

        let fleet = Fleet::from_resstock(
            &pq,
            tmp.path(),
            &weather,
            Some(ResStockVersion::V2025_1),
            None,
            None,
            Some(Duration::days(365)),
            &repo_defaults_path(),
            None,
        )
        .expect("fleet construction");

        assert!(
            !fleet.entries.is_empty(),
            "fleet should have at least one entry"
        );
        for entry in &fleet.entries {
            assert_eq!(
                entry.config.settings.sim_config.duration,
                Duration::days(365),
                "custom duration should be {d} days",
                d = 365
            );
        }
    }

    /// The constructor resolves the committed fixture's extracted layout to
    /// real files and the building's own HPXML weather station to the weather
    /// file: no file swaps.
    #[test]
    fn resstock_fleet_reads_the_extracted_layout() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let tmp = tempdir().expect("tmp");
        let pq = tmp.path().join("test.parquet");
        write_resstock_parquet_v2025(&pq, 19_694);
        let fixtures = resstock_fixtures_root().join("2025.1");

        let fleet = Fleet::from_resstock(
            &pq,
            &fixtures,
            &fixtures.join("weather"),
            Some(ResStockVersion::V2025_1),
            None,
            None,
            None,
            &repo_defaults_path(),
            None,
        )
        .expect("fleet construction");
        assert_eq!(fleet.entries.len(), 1);

        let config = &fleet.entries[0].config;
        assert_eq!(
            config.hpxml.path(),
            Some(fixtures.join("bldg0019694/home.xml").as_path())
        );
        assert_eq!(
            config.schedule.as_ref().and_then(|source| source.path()),
            Some(fixtures.join("bldg0019694/in.schedules.csv").as_path())
        );
        let weather = config.weather.path().expect("the weather path resolves");
        assert_eq!(
            weather,
            fixtures.join("weather/G5300330_2018.csv").as_path(),
            "the FIPS read from the building's own HPXML must resolve the weather file"
        );
        assert!(weather.exists(), "the resolved weather file must exist");
    }

    /// The acceptance: a fleet built from the unpacked dataset fixture builds
    /// AND runs a dwelling end to end through the public path, the weather
    /// resolved (no file swaps).
    #[test]
    fn resstock_fleet_builds_and_runs_a_dwelling_with_resolved_weather() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let tmp = tempdir().expect("tmp");
        let pq = tmp.path().join("test.parquet");
        write_resstock_parquet_v2025(&pq, 19_694);
        let fixtures = resstock_fixtures_root().join("2025.1");

        let fleet = Fleet::from_resstock(
            &pq,
            &fixtures,
            &fixtures.join("weather"),
            Some(ResStockVersion::V2025_1),
            None,
            None,
            None,
            &repo_defaults_path(),
            None,
        )
        .expect("fleet construction");
        assert_eq!(fleet.entries.len(), 1);

        let chunk = std::num::NonZeroUsize::new(1).expect("1");
        let result = fleet
            .simulate(
                &crate::fold::FleetReport {
                    selection: hares_io::Selection::Verbosity(0),
                    frequency: hares_io::Frequency::Hourly,
                    health_policy: hares_io::HealthPolicy::default(),
                },
                1,
                chunk,
            )
            .expect("the fixture dwelling must run without a simulation error");
        assert_eq!(result.homes.len(), 1);
        // The fleet's sim config writes no output: the run's metrics are
        // None (no report), and no run warning accompanies that.
        match &result.homes[0].status {
            HomeStatus::Ok => {}
            status => panic!("the dwelling must run to completion, got: {status:?}"),
        }
        assert_eq!(
            result.homes[0].steps, 1440,
            "the default 24 h at the constructor's step"
        );
        assert!(
            result.homes[0].metrics.is_none(),
            "without the metrics request the run's result carries none"
        );
    }

    /// A stochastic fleet home's inputs: one building whose Microwave
    /// draws stochastically every idle step, the fleet's own fixtures for
    /// the weather the station name resolves to.
    fn stochastic_fleet() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempdir().expect("tmp");
        let pq = tmp.path().join("test.parquet");
        write_resstock_parquet_v2025(&pq, 19_694);
        write_stochastic_building(tmp.path(), 19_694);
        (tmp, pq)
    }

    /// The caller's seed reaches every home's sim config, and the build
    /// report names the seed the fleet runs under: `None` keeps the
    /// documented default (0, the value the per-home derivations used
    /// before the parameter existed), `Some(seed)` is the caller's.
    #[test]
    fn from_resstock_seed_reaches_the_configs_and_the_report() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (tmp, pq) = stochastic_fleet();
        let weather = resstock_fixtures_root().join("2025.1/weather");
        let defaults = repo_defaults_path();
        let build = |seed: Option<u64>| {
            Fleet::from_resstock(
                &pq,
                tmp.path(),
                &weather,
                Some(ResStockVersion::V2025_1),
                None,
                None,
                None,
                &defaults,
                seed,
            )
            .expect("fleet construction")
        };

        let caller = build(Some(9_2026));
        assert_eq!(
            caller
                .home_configs()
                .iter()
                .map(|config| config.settings.sim_config.master_seed)
                .collect::<Vec<_>>(),
            vec![9_2026],
            "the caller's seed is every home's master seed"
        );
        assert_eq!(
            caller
                .build_report()
                .expect("a ResStock build records it")
                .master_seed,
            9_2026,
            "the build report names the seed the fleet runs under"
        );

        let defaulted = build(None);
        assert_eq!(
            defaulted
                .home_configs()
                .iter()
                .map(|config| config.settings.sim_config.master_seed)
                .collect::<Vec<_>>(),
            vec![0],
            "None keeps the documented default master seed"
        );
        assert_eq!(
            defaulted
                .build_report()
                .expect("a ResStock build records it")
                .master_seed,
            0,
            "the build report names the default seed"
        );
    }

    /// Two fleets built with different caller seeds draw different
    /// stochastic series, and the same seed rebuilds the same series:
    /// the fleet's stochastic interior answers to the caller. (Before the
    /// parameter existed every fleet ran the hardcoded default seed, and
    /// no caller could vary a draw.)
    #[test]
    fn from_resstock_seed_partitions_the_stochastic_draws() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (tmp, pq) = stochastic_fleet();
        let weather = resstock_fixtures_root().join("2025.1/weather");
        let defaults = repo_defaults_path();
        let build = |seed: Option<u64>| {
            Fleet::from_resstock(
                &pq,
                tmp.path(),
                &weather,
                Some(ResStockVersion::V2025_1),
                None,
                None,
                None,
                &defaults,
                seed,
            )
            .expect("fleet construction")
        };
        let run = |fleet: &Fleet| {
            fleet
                .simulate(
                    &crate::fold::FleetReport {
                        selection: hares_io::Selection::Verbosity(2),
                        frequency: hares_io::Frequency::Minutes(
                            std::num::NonZeroU32::new(1).expect("1"),
                        ),
                        health_policy: hares_io::HealthPolicy::default(),
                    },
                    1,
                    std::num::NonZeroUsize::new(1).expect("1"),
                )
                .expect("the fleet runs")
        };
        let power = |result: &FleetResult| {
            let slot = result
                .aggregate
                .schema_ref()
                .fields()
                .iter()
                .position(|field| field.name() == "Total Electric Power (kW)")
                .expect("the power column folds at tier 2");
            let column = result
                .aggregate
                .column(slot)
                .as_any()
                .downcast_ref::<arrow::array::Float64Array>()
                .expect("a Float64 column");
            (0..column.len())
                .map(|row| column.value(row))
                .collect::<Vec<_>>()
        };

        let fleet_seven = build(Some(7));
        let fleet_nine = build(Some(9));
        let fleet_seven_again = build(Some(7));

        let seven = power(&run(&fleet_seven));
        let nine = power(&run(&fleet_nine));
        let seven_repeat = power(&run(&fleet_seven_again));

        assert_eq!(
            seven.len(),
            1440,
            "the default 24 h at the constructor's step"
        );
        assert!(
            seven.iter().any(|value| value != &0.0),
            "the stochastic home draws power in some step, the series is not vacuous"
        );
        assert!(
            seven
                .iter()
                .zip(&seven_repeat)
                .all(|(a, b)| a.to_bits() == b.to_bits()),
            "the same seed rebuilds the same series bitwise"
        );
        assert!(
            seven
                .iter()
                .zip(&nine)
                .any(|(a, b)| a.to_bits() != b.to_bits()),
            "two different caller seeds draw different series"
        );
    }

    /// The extracted building with one stochastic appliance: the committed
    /// fixture building's own HPXML and schedule, with a Microwave whose
    /// extension configures the event load's stochastic fallback added
    /// (the schedule carries no microwave column to replay, so every idle
    /// step with the window open draws). The shape is the event-load
    /// stream tests' probe appliance.
    fn write_stochastic_building(root: &Path, bldg_id: i64) {
        let dir = root.join(format!("bldg{bldg_id:07}"));
        fs::create_dir_all(&dir).expect("create building dir");
        let source = resstock_fixtures_root()
            .join("2025.1")
            .join(format!("bldg{bldg_id:07}"));
        let hpxml = fs::read_to_string(source.join("home.xml"))
            .expect("the fixture building's HPXML reads");
        let marker = "</Appliances>";
        assert!(
            hpxml.contains(marker),
            "the fixture building has appliances"
        );
        let microwave = r#"        <Microwave>
          <SystemIdentifier id='Microwave1'/>
          <extension>
            <active_power_w>1000.0</active_power_w>
            <active_duration_s>1800</active_duration_s>
            <cooldown_duration_s>900</cooldown_duration_s>
            <event_window_source>constant</event_window_source>
            <event_probability_source>constant</event_probability_source>
            <event_probability_constant>0.3</event_probability_constant>
            <sensible_gain_fraction>0.0</sensible_gain_fraction>
            <latent_gain_fraction>0.0</latent_gain_fraction>
          </extension>
        </Microwave>
"#;
        write_temp_file(
            &dir.join("home.xml"),
            &hpxml.replacen(marker, &format!("{microwave}{marker}"), 1),
        );
        let schedule = fs::read_to_string(source.join("in.schedules.csv"))
            .expect("the fixture building's schedule reads");
        write_temp_file(&dir.join("in.schedules.csv"), &schedule);
    }

    /// The dataset-as-distributed shape (zip archives where the extracted
    /// building directories belong) is a typed error naming the fix.
    #[test]
    fn resstock_fleet_rejects_the_dataset_zip() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let tmp = tempdir().expect("tmp");
        let pq = tmp.path().join("test.parquet");
        write_resstock_parquet_v2025(&pq, 19_694);
        fs::write(
            tmp.path().join("bldg0019694.zip"),
            "a zip archive, not the unpacked building",
        )
        .expect("write zip stand-in");

        let err = Fleet::from_resstock(
            &pq,
            tmp.path(),
            tmp.path(),
            Some(ResStockVersion::V2025_1),
            None,
            None,
            None,
            &repo_defaults_path(),
            None,
        )
        .expect_err("a zip archive is not the unpacked layout");
        let message = err.to_string();
        assert!(
            message.contains("19694"),
            "must name the building: {message}"
        );
        assert!(
            message.contains("unpacked buildings directory"),
            "must name the fix: {message}"
        );
    }

    // -------------------------------------------------------------------
    // B-9's acceptance: the ResStock fleet's year and offset are
    // declared, not hardcoded.
    // -------------------------------------------------------------------

    /// The B-9a fixture's inputs: the three WA homes (G5300330) and their
    /// AMY 2018 weather files.
    fn fleet3_inputs() -> (PathBuf, PathBuf, PathBuf) {
        let fixtures = resstock_fixtures_root().join("2025.1");
        (
            fixtures.join("metadata_fleet3.parquet"),
            fixtures.clone(),
            fixtures.join("weather"),
        )
    }

    /// A ResStock fleet on an actual-year (AMY) file runs the weather
    /// file's own year at the site's offset: every home of the B-9a
    /// fixture starts 2018-01-01T00:00:00-08:00 (Washington, G5300330,
    /// the HPXML Site's own UTCOffset) with `weather_year = Actual`,
    /// never the removed 2019-01-01 UTC+0 assumption.
    #[test]
    fn resstock_actual_year_fleet_runs_the_weather_year() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (metadata, buildings, weather) = fleet3_inputs();
        let fleet = Fleet::from_resstock(
            &metadata,
            &buildings,
            &weather,
            Some(ResStockVersion::V2025_1),
            None,
            None,
            None,
            &repo_defaults_path(),
            None,
        )
        .expect("the fleet builds");
        assert_eq!(fleet.entries.len(), 3);
        for entry in &fleet.entries {
            let sim = &entry.config.settings.sim_config;
            assert_eq!(
                sim.start_time,
                FixedOffset::east_opt(-8 * 3600)
                    .expect("valid offset")
                    .with_ymd_and_hms(2018, 1, 1, 0, 0, 0)
                    .single()
                    .expect("valid start"),
                "the home starts the weather file's own year at the site's offset"
            );
            assert_eq!(
                sim.weather_year,
                hares_io::WeatherYear::Actual,
                "the AMY release declares the actual-year rule"
            );
        }
    }

    /// A typical-year (TMY3) release has no built-in year: without the
    /// year argument the construction is an error naming the argument,
    /// raised before any file is read (the metadata path here does not
    /// exist, so a metadata read would name the file instead).
    #[test]
    fn resstock_typical_year_needs_a_year() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (_metadata, buildings, _) = fleet3_inputs();
        let err = Fleet::from_resstock(
            &PathBuf::from(MISSING_ROOT).join("metadata.parquet"),
            &buildings,
            &buildings,
            Some(ResStockVersion::V2024_2),
            None,
            None,
            None,
            &repo_defaults_path(),
            None,
        )
        .expect_err("a typical-year release without a year is an error");
        let message = err.to_string();
        assert!(
            message.contains("typical_year"),
            "the error must name the argument: {message}"
        );
    }

    /// A typical-year ResStock fleet's run wraps by day of year (B-1c):
    /// the constructor declares `Typical` and the caller's year, and a run
    /// longer than the file's year reads Jan 1's records again on the
    /// wrapped dates. The home runs the constructor's declared start
    /// (2023-01-01 00:00 at the site's offset) over 367 days at the hour
    /// step, and the last 24 hours - the steps ending 2024-01-01 01:00 to
    /// 2024-01-02 00:00 local - read the file's own Jan 1 records.
    #[test]
    fn resstock_typical_year_fleet_wraps_by_day_of_year() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let tmp = tempdir().expect("tmp");
        let pq = tmp.path().join("test.parquet");
        // One home at the metadata's weight 1.0, so the aggregate's
        // weighted mean is the home's own values bit for bit.
        write_resstock_parquet_v2024_2(&pq, 19_694);
        let buildings = resstock_fixtures_root().join("2025.1");
        // The weather: a typical-year EPW at the building's own station
        // name (2024.x resolves `{fips}.epw`).
        let weather_dir = tempdir().expect("weather dir");
        let epw = weather_dir.path().join("G5300330.epw");
        fs::copy(
            repo_root().join("tests/fixtures/parity/cz4a_ashp_hpwh/weather.epw"),
            &epw,
        )
        .expect("the cz4a typical file copies");

        let mut fleet = Fleet::from_resstock(
            &pq,
            &buildings,
            weather_dir.path(),
            Some(ResStockVersion::V2024_2),
            Some(2023),
            None,
            None,
            &repo_defaults_path(),
            None,
        )
        .expect("the typical-year fleet builds with its declared year");
        assert_eq!(fleet.entries.len(), 1);
        assert_eq!(
            fleet.entries[0].config.settings.sim_config.start_time,
            FixedOffset::east_opt(-8 * 3600)
                .expect("valid offset")
                .with_ymd_and_hms(2023, 1, 1, 0, 0, 0)
                .single()
                .expect("valid start"),
            "the declared year at the site's offset"
        );
        // The wrap run at the hour step (the file's own step): 367 days
        // crosses Dec 31 into Jan 1. The LocalClock alignment labels the
        // hourly rows at the home's own local period starts, so the
        // wrapped Jan 1 rows carry their local labels.
        for entry in &mut fleet.entries {
            entry.config.settings.sim_config.time_res = Duration::hours(1);
            entry.config.settings.sim_config.duration = Duration::days(367);
        }
        let fleet = fleet.with_alignment(crate::fold::Alignment::LocalClock);

        let result = fleet
            .simulate(
                &crate::fold::FleetReport {
                    selection: hares_io::Selection::Verbosity(2),
                    frequency: hares_io::Frequency::Hourly,
                    health_policy: hares_io::HealthPolicy::default(),
                },
                1,
                chunk(2),
            )
            .expect("the wrap run completes");
        let home = &result.homes[0];
        assert!(
            matches!(home.status, HomeStatus::Ok),
            "the wrap run is clean: {:?}",
            home.status
        );

        // The file's own Jan 1 records, parsed from the same EPW copy.
        let series = hares_io::parse_weather(&epw).expect("the typical file parses");
        let jan1: Vec<f64> = series.dry_bulb_c[..24].to_vec();

        let aggregate = &result.aggregate;
        let time_ms = time_labels_of(aggregate);
        let time_text: Vec<String> = time_ms.iter().map(|&ms| label_text(ms)).collect();
        let dry_bulb = aggregate
            .column(
                aggregate
                    .schema()
                    .fields()
                    .iter()
                    .position(|field| field.name() == "Outdoor Dry Bulb (C)")
                    .expect("the verbosity-2 aggregate carries the outdoor dry bulb"),
            )
            .as_any()
            .downcast_ref::<Float64Array>()
            .expect("a Float64 outdoor dry bulb column");
        let len = time_ms.len();
        assert_eq!(len, 367 * 24, "the run's hours, the wrap included");
        assert!(
            (0..len).any(|row| time_text[row].starts_with("2023-12-31T00:00:00")),
            "the run reaches the file's Dec 31 on its local labels"
        );
        // The run covers local 2023-01-01 to 2024-01-03; the wrapped Jan 1
        // is the 24 rows before the last 24 (the wrapped Jan 2).
        assert!(
            time_text[len - 24].starts_with("2024-01-02"),
            "the run crosses the wrapped Jan 1: {:?}",
            time_text[len - 24]
        );
        for (k, expected) in jan1.iter().enumerate() {
            let row = len - 48 + k;
            assert!(
                time_text[row].starts_with("2024-01-01"),
                "row {row} is on the wrapped date: {:?}",
                time_text[row]
            );
            assert_eq!(
                dry_bulb.value(row),
                *expected,
                "the wrapped Jan 1 reads the file's Jan 1 record {k}"
            );
        }
    }

    /// A building whose weather file cannot be resolved is a fleet
    /// construction error: one error naming every unresolved building
    /// with its county, not a warned empty path.
    #[test]
    fn unresolved_resstock_weather_is_an_error() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (metadata, buildings, _) = fleet3_inputs();
        let empty_weather = tempdir().expect("tmp");
        let err = Fleet::from_resstock(
            &metadata,
            &buildings,
            empty_weather.path(),
            Some(ResStockVersion::V2025_1),
            None,
            None,
            None,
            &repo_defaults_path(),
            None,
        )
        .expect_err("unresolved weather is a construction error");
        let message = err.to_string();
        for building in ["bldg0019694", "bldg0162486", "bldg0352714"] {
            assert!(
                message.contains(building),
                "must name {building}: {message}"
            );
        }
        assert_eq!(
            message.matches(" (county G5300330)").count(),
            3,
            "each building is named with its county: {message}"
        );
    }

    // -------------------------------------------------------------------
    // FL-1's acceptance: the streaming ordered fold.
    // -------------------------------------------------------------------

    /// The repo root the fixture paths resolve from.
    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn resstock_fixture(name: &str) -> PathBuf {
        repo_root()
            .join("tests/fixtures/resstock/2025.1")
            .join(name)
    }

    /// One home of the same-offset golden fleet: three copies of the
    /// bldg0176227 building, one weather county each, all UTC-8, so the
    /// homes share one column set and differ in value. The weight rides
    /// `with_sample_weights` after construction.
    fn golden_fleet_config(bldg_id: i64, weather: &str) -> DwellingConfig {
        DwellingConfig {
            hpxml: InputSource::Path(resstock_fixture("bldg0176227/home.xml")),
            schedule: Some(InputSource::Path(resstock_fixture(
                "bldg0176227/in.schedules.csv",
            ))),
            weather: WeatherInput::Path(resstock_fixture("weather").join(weather)),
            defaults: DefaultsSource::directory(repo_defaults_path()),
            settings: DwellingSettings::new(
                SimulationConfig {
                    start_time: FixedOffset::east_opt(-8 * 3600)
                        .expect("valid offset")
                        .with_ymd_and_hms(2018, 1, 1, 0, 0, 0)
                        .single()
                        .expect("valid start"),
                    duration: Duration::days(2),
                    time_res: Duration::seconds(300),
                    reports: vec![hares_io::ReportSpec {
                        name: "frame".to_string(),
                        selection: hares_io::Selection::Verbosity(2),
                        frequency: hares_io::Frequency::default(),
                        destination: hares_io::Destination::Memory,
                    }],
                    diagnostics: false,
                    metrics: true,
                    output_chunk_size: 10_000,
                    setpoint_deadband_c: None,
                    master_seed: 0,
                    civil_timezone: None,
                    site_location: hares_io::SiteLocationOverride::default(),
                    max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
                    health_policy: hares_io::HealthPolicy::default(),
                    first_hour_interpolation: hares_io::FirstHour::default(),
                    weather_year: hares_io::WeatherYear::default(),
                    warmup: hares_io::Warmup::default(),
                },
                None,
                bldg_id,
                None,
            ),
        }
    }

    /// The golden fleet: the same-offset manifest's homes and weights.
    fn golden_fleet() -> Fleet {
        Fleet::from_buildings(vec![
            golden_fleet_config(1, "G0600770_2018.csv"),
            golden_fleet_config(2, "G0600650_2018.csv"),
            golden_fleet_config(3, "G0600790_2018.csv"),
        ])
        .expect("the golden homes carry no reserved report or shared destination")
        .with_sample_weights(vec![1.0, 2.5, 0.5])
        .expect("the manifest's weights are finite and non-negative")
    }

    fn minutes_15() -> FleetReport {
        FleetReport {
            selection: hares_io::Selection::Verbosity(2),
            frequency: hares_io::Frequency::Minutes(std::num::NonZeroU32::new(15).expect("15")),
            health_policy: hares_io::HealthPolicy::default(),
        }
    }

    fn chunk(size: usize) -> NonZeroUsize {
        NonZeroUsize::new(size).expect("nonzero chunk size")
    }

    /// Compares two record batches column by column, floats on bits.
    fn assert_batches_bitwise_equal(left: &RecordBatch, right: &RecordBatch, label: &str) {
        assert_eq!(
            left.num_columns(),
            right.num_columns(),
            "{label}: column count"
        );
        assert_eq!(left.num_rows(), right.num_rows(), "{label}: row count");
        for (slot, (a, b)) in left.columns().iter().zip(right.columns()).enumerate() {
            assert_eq!(
                left.schema_ref().field(slot),
                right.schema_ref().field(slot),
                "{label}: column {slot} field"
            );
            assert_eq!(
                a.to_data(),
                b.to_data(),
                "{label}: column {} ({}) differs",
                left.schema_ref().field(slot).name(),
                slot
            );
        }
    }

    /// The ordered-merge differential: the streaming fold's aggregate is
    /// the same bits whether the fold crosses the chunk barrier between
    /// every home or runs one chunk, at the golden fleet's scale and at a
    /// 12-home scale the barrier actually stresses (six crossings).
    #[test]
    fn fleet_fold_matches_collect_then_aggregate() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let fleet = golden_fleet();
        let collected = fleet
            .simulate(&minutes_15(), 1, chunk(64))
            .expect("the golden fleet runs");
        let streamed = fleet
            .simulate(&minutes_15(), 1, chunk(1))
            .expect("the golden fleet runs");

        assert_eq!(
            collected.aggregate.num_rows(),
            192,
            "two days of quarter hours"
        );
        assert_eq!(
            collected.covered_weight.num_rows(),
            192,
            "the covered weight carries the same periods"
        );
        assert_batches_bitwise_equal(
            &collected.aggregate,
            &streamed.aggregate,
            "the golden fleet's aggregate",
        );
        assert_batches_bitwise_equal(
            &collected.covered_weight,
            &streamed.covered_weight,
            "the golden fleet's covered weight",
        );
        assert_batches_bitwise_equal(
            &collected.homes_table(),
            &streamed.homes_table(),
            "the golden fleet's homes table",
        );
        assert_eq!(collected.coverage, streamed.coverage);
        assert_eq!(collected.n_succeeded, streamed.n_succeeded);
        assert_eq!(collected.peak_live_home_results, 3);
        assert_eq!(streamed.peak_live_home_results, 1);
        assert_eq!(collected.n_succeeded, 3, "every golden home folds");

        // The 12-home scale: distinct weights, six chunk crossings at
        // chunk size 2, compared against the one-chunk fold on bits.
        let weights: Vec<f64> = (1..=12).map(f64::from).collect();
        let homes: Vec<DwellingConfig> = (1..=12)
            .map(|idx| {
                golden_fleet_config(
                    i64::try_from(idx).expect("home index"),
                    [
                        "G0600770_2018.csv",
                        "G0600650_2018.csv",
                        "G0600790_2018.csv",
                    ][idx % 3],
                )
            })
            .collect();
        let fleet = Fleet::from_buildings(homes)
            .expect("the homes carry no reserved report or shared destination")
            .with_sample_weights(weights)
            .expect("the weights are finite and non-negative");
        let collected = fleet
            .simulate(&minutes_15(), 1, chunk(64))
            .expect("the 12-home fleet runs");
        let streamed = fleet
            .simulate(&minutes_15(), 1, chunk(2))
            .expect("the 12-home fleet runs");
        assert_batches_bitwise_equal(
            &collected.aggregate,
            &streamed.aggregate,
            "the 12-home fleet's aggregate",
        );
        assert_batches_bitwise_equal(
            &collected.covered_weight,
            &streamed.covered_weight,
            "the 12-home fleet's covered weight",
        );
    }

    /// One fleet result across every thread count and chunk size: the
    /// aggregate, the covered weight, the homes table and the coverage,
    /// compared column by column with the floats on bits.
    #[test]
    fn fleet_result_identical_across_threads_and_chunks() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let fleet = golden_fleet();
        let reference = fleet
            .simulate(&minutes_15(), 1, chunk(64))
            .expect("the golden fleet runs");
        for &n_threads in [1usize, 2, 8].iter() {
            for &size in [1usize, 2, 64].iter() {
                let result = fleet
                    .simulate(&minutes_15(), n_threads, chunk(size))
                    .expect("the golden fleet runs");
                assert_batches_bitwise_equal(
                    &reference.aggregate,
                    &result.aggregate,
                    &format!("aggregate at {n_threads} threads, chunk {size}"),
                );
                assert_batches_bitwise_equal(
                    &reference.covered_weight,
                    &result.covered_weight,
                    &format!("covered weight at {n_threads} threads, chunk {size}"),
                );
                assert_batches_bitwise_equal(
                    &reference.homes_table(),
                    &result.homes_table(),
                    &format!("homes at {n_threads} threads, chunk {size}"),
                );
                assert_eq!(
                    reference.coverage, result.coverage,
                    "coverage at {n_threads} threads, chunk {size}"
                );
                assert_eq!(reference.n_succeeded, result.n_succeeded);
            }
        }
    }

    /// The memory bound's own read: the peak number of home results held
    /// at once is the chunk size.
    #[test]
    fn fleet_peak_live_home_results_bounded_by_chunk() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let fleet = golden_fleet();
        for (size, expected) in [(1usize, 1usize), (2, 2)] {
            let result = fleet
                .simulate(&minutes_15(), 2, chunk(size))
                .expect("the golden fleet runs");
            assert_eq!(result.peak_live_home_results, expected, "chunk size {size}");
        }
    }

    /// The aggregate's columns are the first folded home's own report
    /// schema: every column after Time has the name and the unit metadata
    /// of the variable of that name in the first home's Timestep memory
    /// report of the same selection, and the covered weight carries the
    /// same names in the same order.
    #[test]
    fn fleet_aggregate_columns_match_home_report_schema() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        // The first home's own Timestep memory report of the same
        // selection, run standalone.
        let mut home_config = golden_fleet_config(1, "G0600770_2018.csv");
        home_config.settings.sim_config.reports = vec![hares_io::ReportSpec {
            name: "frame".to_string(),
            selection: hares_io::Selection::Verbosity(2),
            frequency: hares_io::Frequency::Timestep,
            destination: hares_io::Destination::Memory,
        }];
        let mut dwelling = Dwelling::from_config(home_config).expect("the home builds");
        let engine = SimulationEngine::new();
        let sim_config = dwelling.sim_config().clone();
        let result = engine
            .run_dwelling(&mut dwelling, &sim_config)
            .expect("the home runs");
        let report = result
            .reports
            .iter()
            .find(|report| report.name == "frame")
            .expect("the memory report ran");
        let schema = report
            .batches()
            .first()
            .expect("the memory report retained batches")
            .schema();

        let fleet = golden_fleet();
        let fleet_result = fleet
            .simulate(&minutes_15(), 1, chunk(64))
            .expect("the golden fleet runs");
        let aggregate = &fleet_result.aggregate;

        assert_eq!(
            aggregate.num_columns(),
            schema.fields().len(),
            "Time plus every variable of the home's own report"
        );
        for slot in 1..aggregate.num_columns() {
            let fleet_field = aggregate.schema_ref().field(slot);
            let home_field = schema.field(slot);
            assert_eq!(fleet_field.name(), home_field.name(), "column {slot} name");
            assert_eq!(
                fleet_field.metadata().get("unit"),
                home_field.metadata().get("unit"),
                "column {} ({}) unit metadata",
                fleet_field.name(),
                slot
            );
            assert_eq!(
                *fleet_field.data_type(),
                arrow::datatypes::DataType::Float64,
                "column {} type",
                fleet_field.name()
            );
        }
        for slot in 0..fleet_result.covered_weight.num_columns() {
            assert_eq!(
                fleet_result.covered_weight.schema_ref().field(slot).name(),
                aggregate.schema_ref().field(slot).name(),
                "the covered weight's column {slot} names the same variable"
            );
        }
    }

    /// Fifteen-minute periods reduce one-minute steps: each power cell
    /// equals `a*w_a + b*w_b` where `a` and `b` are the two homes' own
    /// period means, added in step order and divided once by 15.
    #[test]
    fn fleet_minutes_15_reduces_one_minute_steps() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let examples = repo_root().join("data/examples");
        let home = |bldg_id: i64, schedule: PathBuf| DwellingConfig {
            hpxml: InputSource::Path(fixture_hpxml_path()),
            schedule: Some(InputSource::Path(schedule)),
            weather: WeatherInput::Path(examples.join("USA_CO_Denver.Intl.AP.725650_TMY3.epw")),
            defaults: DefaultsSource::directory(repo_defaults_path()),
            settings: DwellingSettings::new(
                SimulationConfig {
                    start_time: fixture_start::fixture_start(),
                    duration: Duration::hours(2),
                    time_res: Duration::minutes(1),
                    reports: Vec::new(),
                    diagnostics: false,
                    metrics: false,
                    output_chunk_size: 10_000,
                    setpoint_deadband_c: None,
                    master_seed: 0,
                    civil_timezone: None,
                    site_location: hares_io::SiteLocationOverride::default(),
                    max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
                    health_policy: hares_io::HealthPolicy::default(),
                    first_hour_interpolation: hares_io::FirstHour::default(),
                    weather_year: hares_io::WeatherYear::default(),
                    warmup: hares_io::Warmup::Disabled,
                },
                None,
                bldg_id,
                None,
            ),
        };

        // Each home's own Timestep memory report, for the hand-computed
        // period means; the fleet homes carry no own reports. The probes
        // run at the fleet homes' building ids: the random streams
        // partition by seed and building id.
        let standalone = |bldg_id: i64, schedule: PathBuf| {
            let mut config = home(bldg_id, schedule);
            config.settings.sim_config.reports = vec![hares_io::ReportSpec {
                name: "frame".to_string(),
                selection: hares_io::Selection::Verbosity(2),
                frequency: hares_io::Frequency::Timestep,
                destination: hares_io::Destination::Memory,
            }];
            let mut dwelling = Dwelling::from_config(config).expect("the home builds");
            let engine = SimulationEngine::new();
            let sim_config = dwelling.sim_config().clone();
            let result = engine
                .run_dwelling(&mut dwelling, &sim_config)
                .expect("the home runs");
            let batches: Vec<RecordBatch> = result
                .reports
                .iter()
                .flat_map(|report| report.batches().to_vec())
                .collect();
            // Per quarter hour: the mean of the fifteen step values, added
            // in step order and divided once by 15.
            let power_slot = batches
                .first()
                .expect("the memory report recorded")
                .schema_ref()
                .fields()
                .iter()
                .position(|field| field.name() == "Total Electric Power (kW)")
                .expect("the power column exists at tier 2");
            let mut means = Vec::new();
            let mut steps: Vec<f64> = Vec::new();
            for batch in &batches {
                let values = batch
                    .column(power_slot)
                    .as_any()
                    .downcast_ref::<arrow::array::Float64Array>()
                    .expect("a Float64 column");
                for row in 0..batch.num_rows() {
                    steps.push(values.value(row));
                    if steps.len() == 15 {
                        let sum = steps.iter().sum::<f64>();
                        means.push(sum / 15.0);
                        steps.clear();
                    }
                }
            }
            means
        };
        let stock = examples.join("BEopt_example_schedule.csv");
        let a = standalone(1, stock.clone());
        // Same schedule, different building id: the random streams
        // partition by seed and building id, so the profiles differ, and
        // a swapped home or weight cannot pass. The bldg0112631 schedule
        // this probe once used carries unmapped-column warnings, which
        // would degrade the home out of the fold.
        let b = standalone(2, stock.clone());
        assert_eq!(a.len(), 8, "two hours of quarter hours");
        assert!(
            a.iter().zip(&b).any(|(a, b)| a.to_bits() != b.to_bits()),
            "the two homes' load profiles differ, so a swapped home or weight cannot pass"
        );

        let fleet = Fleet::from_buildings(vec![home(1, stock.clone()), home(2, stock)])
            .expect("the homes carry no reserved report or shared destination")
            .with_sample_weights(vec![1.0, 3.0])
            .expect("the weights are finite and non-negative");
        let result = fleet
            .simulate(
                &FleetReport {
                    selection: hares_io::Selection::Verbosity(2),
                    frequency: hares_io::Frequency::Minutes(
                        std::num::NonZeroU32::new(15).expect("15"),
                    ),
                    health_policy: hares_io::HealthPolicy::default(),
                },
                1,
                chunk(64),
            )
            .expect("the fleet runs");

        let aggregate = &result.aggregate;
        assert_eq!(aggregate.num_rows(), 8);
        let power_slot = aggregate
            .schema_ref()
            .fields()
            .iter()
            .position(|field| field.name() == "Total Electric Power (kW)")
            .expect("the power column folded");
        let values = aggregate
            .column(power_slot)
            .as_any()
            .downcast_ref::<arrow::array::Float64Array>()
            .expect("a Float64 column");
        for row in 0..8 {
            let expected = a[row] * 1.0 + b[row] * 3.0;
            assert_eq!(
                values.value(row).to_bits(),
                expected.to_bits(),
                "row {row}: the weighted sum of the homes' period means"
            );
        }
    }

    /// A Named selection keeps the names the home declares: the
    /// aggregate's and the covered weight's columns are Time plus those
    /// two variables, and the daily frequency gives two rows.
    #[test]
    fn fleet_report_selects_columns_and_frequency() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let fleet = golden_fleet();
        let report = FleetReport {
            selection: hares_io::Selection::Named(vec![
                "Total Electric Power (kW)".to_string(),
                "Temperature - Indoor (C)".to_string(),
            ]),
            frequency: hares_io::Frequency::Daily,
            health_policy: hares_io::HealthPolicy::default(),
        };
        let result = fleet
            .simulate(&report, 1, chunk(64))
            .expect("the golden fleet runs");
        for table in [&result.aggregate, &result.covered_weight] {
            let names: Vec<String> = table
                .schema_ref()
                .fields()
                .iter()
                .map(|field| field.name().clone())
                .collect();
            assert_eq!(
                names,
                vec![
                    "Time".to_string(),
                    "Total Electric Power (kW)".to_string(),
                    "Temperature - Indoor (C)".to_string(),
                ],
                "the selection's two variables, in order"
            );
            assert_eq!(table.num_rows(), 2, "two days at the daily frequency");
        }
    }

    /// A variable a home does not declare is not applicable to it: the
    /// home neither fails nor folds a value for it.
    #[test]
    fn selection_missing_from_some_homes_does_not_fail_them() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let dir = tempdir().expect("temp dir");
        write_temp_file(&dir.path().join("schedule.csv"), &build_schedule_csv());
        write_temp_file(&dir.path().join("weather.epw"), &build_epw_8760());
        let fixtures = repo_root().join("tests/fixtures/hpxml/ochre_samples");
        let battery_home = DwellingConfig {
            hpxml: InputSource::Path(fixtures.join("base-battery.xml")),
            schedule: Some(InputSource::Path(dir.path().join("schedule.csv"))),
            weather: WeatherInput::Path(dir.path().join("weather.epw")),
            defaults: DefaultsSource::directory(repo_defaults_path()),
            settings: DwellingSettings::new(
                simulation_config(dir.path().join("battery.csv")),
                Some(serde_json::json!({"Battery": {"bms_mode": "\"Manual\""}})),
                2,
                None,
            ),
        };
        // A battery variable's name, from the battery home's own catalogue.
        let catalogue = Dwelling::from_config(battery_home.clone())
            .expect("the battery home builds")
            .catalogue();
        let battery_variable = catalogue
            .variables()
            .iter()
            .map(|def| def.name.to_string())
            .find(|name| name.contains("Battery"))
            .expect("the battery home declares battery variables");
        let plain_home = DwellingConfig {
            hpxml: InputSource::Path(fixture_hpxml_path()),
            schedule: Some(InputSource::Path(dir.path().join("schedule.csv"))),
            weather: WeatherInput::Path(dir.path().join("weather.epw")),
            defaults: DefaultsSource::directory(repo_defaults_path()),
            settings: DwellingSettings::new(
                simulation_config(dir.path().join("plain.csv")),
                None,
                1,
                None,
            ),
        };

        let fleet = Fleet::from_buildings(vec![plain_home, battery_home])
            .expect("the homes carry no reserved report or shared destination");
        let report = FleetReport {
            selection: hares_io::Selection::Named(vec![
                battery_variable.clone(),
                "Total Electric Power (kW)".to_string(),
            ]),
            frequency: hares_io::Frequency::Hourly,
            health_policy: hares_io::HealthPolicy::default(),
        };
        let result = fleet
            .simulate(&report, 1, chunk(64))
            .expect("neither home fails on the missing name");
        assert_eq!(result.n_succeeded, 2, "both homes succeed");
        for home in &result.homes {
            assert!(
                !matches!(home.status, HomeStatus::Failed(_)),
                "a name the home does not declare must not fail it: {:?}",
                home.status
            );
        }
    }

    /// A selection name no home declares, and a pattern that matches
    /// nothing in any home, are the unmatched-selection error.
    #[test]
    fn selection_matching_no_home_is_an_error() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let dir = tempdir().expect("temp dir");
        let fleet = Fleet::from_buildings(build_valid_configs(dir.path(), 2))
            .expect("the configs carry no reserved report or shared destination");

        let named = FleetReport {
            selection: hares_io::Selection::Named(vec!["No Such Variable (kW)".to_string()]),
            frequency: hares_io::Frequency::Hourly,
            health_policy: hares_io::HealthPolicy::default(),
        };
        let err = fleet
            .simulate(&named, 1, chunk(64))
            .expect_err("no home declares the name");
        assert!(
            matches!(err, FleetError::UnmatchedSelection { ref names } if names == &vec!["No Such Variable (kW)".to_string()]),
            "the error names the unmatched selection: {err}"
        );

        let pattern = FleetReport {
            selection: hares_io::Selection::Pattern("NoSuchVariable*".to_string()),
            frequency: hares_io::Frequency::Hourly,
            health_policy: hares_io::HealthPolicy::default(),
        };
        let err = fleet
            .simulate(&pattern, 1, chunk(64))
            .expect_err("the pattern matches nothing");
        assert!(
            matches!(err, FleetError::UnmatchedSelection { ref names } if names.first().map(String::as_str) == Some("NoSuchVariable*")),
            "the error names the unmatched pattern: {err}"
        );
    }

    /// Two homes whose file destinations resolve to the same path are
    /// rejected at construction, both building ids named.
    #[test]
    fn fleet_homes_with_same_file_destination_are_rejected() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let dir = tempdir().expect("temp dir");
        let shared = dir.path().join("shared.parquet");
        let report_for = |bldg_id: i64| DwellingConfig {
            hpxml: InputSource::Path(fixture_hpxml_path()),
            schedule: None,
            weather: WeatherInput::Path(dir.path().join("weather.epw")),
            defaults: DefaultsSource::directory(repo_defaults_path()),
            settings: DwellingSettings::new(
                SimulationConfig {
                    reports: vec![hares_io::ReportSpec {
                        name: format!("report-{bldg_id}"),
                        selection: hares_io::Selection::Verbosity(0),
                        frequency: hares_io::Frequency::default(),
                        destination: hares_io::Destination::Parquet {
                            path: shared.clone(),
                            overwrite: false,
                        },
                    }],
                    ..simulation_config(dir.path().join(format!("unused-{bldg_id}.csv")))
                },
                None,
                bldg_id,
                None,
            ),
        };
        let err = Fleet::from_buildings(vec![report_for(1), report_for(2)])
            .expect_err("two homes may not write one path");
        assert!(
            matches!(
                err,
                FleetError::DuplicateDestination {
                    first: 1,
                    second: 2,
                    ..
                }
            ),
            "the error names both building ids: {err}"
        );
    }

    /// A home whose own config carries a report named `fleet` is a
    /// construction error naming its building id.
    #[test]
    fn fleet_home_report_named_fleet_is_rejected() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let dir = tempdir().expect("temp dir");
        let mut config = build_valid_configs(dir.path(), 1).remove(0);
        config
            .settings
            .sim_config
            .reports
            .push(hares_io::ReportSpec {
                name: "fleet".to_string(),
                selection: hares_io::Selection::Verbosity(0),
                frequency: hares_io::Frequency::default(),
                destination: hares_io::Destination::Memory,
            });
        let err = Fleet::from_buildings(vec![config])
            .expect_err("the reserved report name is rejected at construction");
        assert!(
            matches!(err, FleetError::ReservedReportName { bldg_id: 1 }),
            "the error names the building: {err}"
        );
    }

    /// A home whose own config carries a callback-destination report fails
    /// that home at run start, naming the report: the fleet builds its
    /// dwellings internally, so nothing can attach the report's callback,
    /// and the run refuses a callback report with nothing attached instead
    /// of streaming its batches to nowhere. The other homes are unaffected.
    #[test]
    fn fleet_home_callback_report_fails_that_home_at_run_start() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let dir = tempdir().expect("temp dir");
        let mut configs = build_valid_configs(dir.path(), 2);
        configs[1]
            .settings
            .sim_config
            .reports
            .push(hares_io::ReportSpec {
                name: "mine".to_string(),
                selection: hares_io::Selection::Verbosity(0),
                frequency: hares_io::Frequency::default(),
                destination: hares_io::Destination::Callback,
            });
        let fleet = Fleet::from_buildings(configs)
            .expect("the configs carry no reserved report or shared destination");
        let report = FleetReport {
            selection: hares_io::Selection::Verbosity(0),
            frequency: hares_io::Frequency::Hourly,
            health_policy: hares_io::HealthPolicy::default(),
        };
        let result = fleet
            .simulate(&report, 1, chunk(64))
            .expect("the fleet runs; the refusal is that home's status");
        assert_eq!(result.homes.len(), 2);
        assert!(
            !matches!(result.homes[0].status, HomeStatus::Failed(_)),
            "the home without the callback report is unaffected: {:?}",
            result.homes[0].status
        );
        match &result.homes[1].status {
            HomeStatus::Failed(reason) => {
                assert!(
                    reason.contains("mine") && reason.contains("no callback is attached"),
                    "the refusal names the report and the missing attachment: {reason}"
                );
            }
            status => panic!("expected a failed home, got: {status:?}"),
        }
        assert_eq!(result.homes[1].steps, 0, "the home failed at run start");
        assert_eq!(result.n_succeeded, 1, "only the clean home succeeded");
    }

    /// Two identical homes at different UTC offsets (one site override),
    /// starting at the same instant, the fleet declaring the `Instant`
    /// alignment at UTC: the homes reduce on the declared offset's period
    /// boundaries and the fold's periods align on the instant, so the
    /// aggregate is the full common grid (no intersection loss) and the
    /// covered weight is the two homes' weight sum in every cell, on
    /// bits. At `+00:00` the labels are that instant's UTC wall clock,
    /// naive, the same instants the undeclared single-offset fleet
    /// labels in the homes' local wall clock (B-12).
    #[test]
    fn cross_offset_homes_align_on_the_start_instant() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let examples = repo_root().join("data/examples");
        let config = |bldg_id: i64,
                      start: chrono::DateTime<FixedOffset>,
                      forced: hares_io::SiteLocationOverride| {
            DwellingConfig {
                hpxml: InputSource::Path(fixture_hpxml_path()),
                schedule: Some(InputSource::Path(
                    examples.join("BEopt_example_schedule.csv"),
                )),
                weather: WeatherInput::Path(examples.join("USA_CO_Denver.Intl.AP.725650_TMY3.epw")),
                defaults: DefaultsSource::directory(repo_defaults_path()),
                settings: DwellingSettings::new(
                    SimulationConfig {
                        start_time: start,
                        duration: Duration::hours(2),
                        time_res: Duration::seconds(900),
                        reports: Vec::new(),
                        diagnostics: false,
                        metrics: false,
                        output_chunk_size: 10_000,
                        setpoint_deadband_c: None,
                        master_seed: 0,
                        civil_timezone: None,
                        site_location: forced,
                        max_consecutive_step_failures:
                            hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
                        health_policy: hares_io::HealthPolicy::default(),
                        first_hour_interpolation: hares_io::FirstHour::default(),
                        weather_year: hares_io::WeatherYear::default(),
                        warmup: hares_io::Warmup::Disabled,
                    },
                    None,
                    bldg_id,
                    None,
                ),
            }
        };
        // The Denver weather resolves UTC-7, and the second home's site
        // override forces UTC-5. The start instants are equal: local
        // midnight at -07:00 is 02:00 local at -05:00, so the two homes'
        // naive local clocks differ by the two hours their offsets do.
        let utc_minus_7 = FixedOffset::west_opt(7 * 3600).expect("valid offset");
        let utc_minus_5 = FixedOffset::west_opt(5 * 3600).expect("valid offset");
        let start_a = utc_minus_7
            .with_ymd_and_hms(2021, 1, 1, 0, 0, 0)
            .single()
            .expect("valid start");
        let start_b = utc_minus_5
            .with_ymd_and_hms(2021, 1, 1, 2, 0, 0)
            .single()
            .expect("valid start");
        assert_eq!(start_a, start_b, "the two starts name one instant");
        let forced = hares_io::SiteLocationOverride {
            utc_offset_h: Some(-5.0),
            ..hares_io::SiteLocationOverride::default()
        };
        let fleet = Fleet::from_buildings(vec![
            config(1, start_a, hares_io::SiteLocationOverride::default()),
            config(2, start_b, forced),
        ])
        .expect("the homes carry no reserved report or shared destination")
        .with_sample_weights(vec![1.0, 1.0])
        .expect("the weights are finite and non-negative")
        .with_alignment(Alignment::Instant(
            FixedOffset::east_opt(0).expect("a zero offset is representable"),
        ));

        let report = FleetReport {
            selection: hares_io::Selection::Verbosity(2),
            frequency: hares_io::Frequency::Hourly,
            health_policy: hares_io::HealthPolicy::default(),
        };
        let result = fleet
            .simulate(&report, 1, chunk(64))
            .expect("the fleet runs");
        for home in &result.homes {
            assert!(
                !matches!(home.status, HomeStatus::Failed(_)),
                "both homes run: {:?}",
                home.status
            );
            assert_eq!(home.periods_folded, 2, "every home folds both hours");
        }
        assert_eq!(
            result.aggregate.num_rows(),
            2,
            "the homes' hour rows align on the instant: no intersection loss"
        );
        let times_ms = time_labels_of(&result.aggregate);
        assert_eq!(times_ms[0], naive_ms(2021, 1, 1, 7, 0));
        assert_eq!(times_ms[1], naive_ms(2021, 1, 1, 8, 0));
        for slot in 1..result.covered_weight.num_columns() {
            let weights = result
                .covered_weight
                .column(slot)
                .as_any()
                .downcast_ref::<Float64Array>()
                .expect("a Float64 covered weight column");
            // A column no home produced (the unmet-load column no
            // producer claims) is uncovered in every row; every produced
            // hour carries both homes' weight.
            let column_name = result.covered_weight.schema_ref().field(slot).name();
            let uncovered = weights.null_count() == weights.len();
            for row in 0..2 {
                assert!(
                    uncovered || !weights.is_null(row),
                    "row {row}: every produced hour of {column_name} is fully covered"
                );
                if !uncovered && weights.value(row) != 0.0 {
                    assert_eq!(
                        weights.value(row).to_bits(),
                        2.0_f64.to_bits(),
                        "row {row}: the two homes' weights"
                    );
                }
            }
        }
    }

    /// One home requests the run metrics and one does not: the homes
    /// table's metrics columns ride (any home's request adds them), and
    /// the home without the request reads null in both.
    #[test]
    fn mixed_metrics_homes_keep_the_columns_and_read_null() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut configs = vec![
            golden_fleet_config(1, "G0600770_2018.csv"),
            golden_fleet_config(2, "G0600650_2018.csv"),
        ];
        configs[1].settings.sim_config.metrics = false;
        let fleet = Fleet::from_buildings(configs)
            .expect("the homes carry no reserved report or shared destination")
            .with_sample_weights(vec![1.0, 1.0])
            .expect("the weights are finite and non-negative");
        let result = fleet
            .simulate(&minutes_15(), 1, chunk(64))
            .expect("the fleet runs");
        assert!(
            result.homes[0].metrics.is_some(),
            "the first home requested metrics"
        );
        assert!(
            result.homes[1].metrics.is_none(),
            "the second home did not request them"
        );

        let table = result.homes_table();
        let names: Vec<&str> = table
            .schema_ref()
            .fields()
            .iter()
            .map(|field| field.name().as_str())
            .collect();
        assert!(
            names.contains(&"total_energy_kwh") && names.contains(&"peak_power_kw"),
            "the metrics columns ride: {names:?}"
        );
        let energy = table
            .column(
                names
                    .iter()
                    .position(|n| *n == "total_energy_kwh")
                    .expect("named"),
            )
            .as_any()
            .downcast_ref::<Float64Array>()
            .expect("a Float64 column");
        let peak = table
            .column(
                names
                    .iter()
                    .position(|n| *n == "peak_power_kw")
                    .expect("named"),
            )
            .as_any()
            .downcast_ref::<Float64Array>()
            .expect("a Float64 column");
        assert!(
            !energy.is_null(0) && !peak.is_null(0),
            "the requesting home carries both"
        );
        assert!(
            energy.is_null(1) && peak.is_null(1),
            "the other home reads null"
        );
    }

    // -------------------------------------------------------------------
    // A flagged or degraded home is never a success: the acceptance.
    // -------------------------------------------------------------------

    /// The fleet selection's status and reason columns read off the homes
    /// table, the one surface both the old and the new classification
    /// spell.
    fn table_string_column(table: &RecordBatch, name: &str) -> Vec<Option<String>> {
        let slot = table
            .schema_ref()
            .fields()
            .iter()
            .position(|field| field.name() == name)
            .unwrap_or_else(|| panic!("no {name} column"));
        let values = table
            .column(slot)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("a Utf8 column");
        (0..values.len())
            .map(|row| (!values.is_null(row)).then(|| values.value(row).to_string()))
            .collect()
    }

    fn table_float_column(table: &RecordBatch, name: &str) -> Vec<Option<f64>> {
        let slot = table
            .schema_ref()
            .fields()
            .iter()
            .position(|field| field.name() == name)
            .unwrap_or_else(|| panic!("no {name} column"));
        let values = table
            .column(slot)
            .as_any()
            .downcast_ref::<Float64Array>()
            .expect("a Float64 column");
        (0..values.len())
            .map(|row| (!values.is_null(row)).then(|| values.value(row)))
            .collect()
    }

    /// The stock schedule with one added column `unread_load` (every value
    /// 0.0), which no mapping reads: the run carries the unmapped-column
    /// `Warning`. Written under `dir` so the stock file is untouched.
    fn schedule_with_unread_column(dir: &Path) -> PathBuf {
        let stock =
            fs::read_to_string(repo_root().join("data/examples/BEopt_example_schedule.csv"))
                .expect("the stock schedule reads");
        let mut out = String::with_capacity(stock.len() + 8 * 8761);
        for (index, line) in stock.lines().enumerate() {
            if index == 0 {
                out.push_str(line);
                out.push_str(",unread_load\n");
            } else {
                out.push_str(line);
                out.push_str(",0.0\n");
            }
        }
        let dest = dir.join("unread_load_schedule.csv");
        fs::write(&dest, out).expect("the schedule copy writes");
        dest
    }

    /// The unread-load acceptance fleet: the base fixture at 24 h and
    /// 900 s, the first home on the stock schedule and the second on the
    /// unread-load copy, weights 1 and 2, folded at the hour.
    fn fl2_fleet(unread_schedule: &Path, metrics: bool) -> Fleet {
        let home = |bldg_id: i64, schedule: &Path| DwellingConfig {
            hpxml: InputSource::Path(fixture_hpxml_path()),
            schedule: Some(InputSource::Path(schedule.to_path_buf())),
            weather: WeatherInput::Path(
                repo_root().join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
            ),
            defaults: DefaultsSource::directory(repo_defaults_path()),
            settings: DwellingSettings::new(
                SimulationConfig {
                    start_time: fixture_start::fixture_start(),
                    duration: Duration::hours(24),
                    time_res: Duration::seconds(900),
                    reports: Vec::new(),
                    diagnostics: false,
                    metrics,
                    output_chunk_size: 10_000,
                    setpoint_deadband_c: None,
                    master_seed: 0,
                    civil_timezone: None,
                    site_location: hares_io::SiteLocationOverride::default(),
                    max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
                    health_policy: hares_io::HealthPolicy::default(),
                    first_hour_interpolation: hares_io::FirstHour::default(),
                    weather_year: hares_io::WeatherYear::default(),
                    warmup: hares_io::Warmup::Disabled,
                },
                None,
                bldg_id,
                None,
            ),
        };
        Fleet::from_buildings(vec![
            home(
                1,
                &repo_root().join("data/examples/BEopt_example_schedule.csv"),
            ),
            home(2, unread_schedule),
        ])
        .expect("the homes carry no reserved report or shared destination")
        .with_sample_weights(vec![1.0, 2.0])
        .expect("the weights are finite and non-negative")
    }

    fn report_24h_hourly() -> crate::fold::FleetReport {
        crate::fold::FleetReport {
            selection: hares_io::Selection::Verbosity(2),
            frequency: hares_io::Frequency::Hourly,
            health_policy: hares_io::HealthPolicy::default(),
        }
    }

    /// The clean home's own hourly period means of the power column, run
    /// standalone at the fleet home's building id, seed and inputs: the
    /// Timestep memory report grouped four 900 s steps to the hour, the
    /// mean dividing once, in bucket order.
    fn standalone_hourly_power_means(bldg_id: i64, schedule: &Path) -> Vec<f64> {
        let mut config = DwellingConfig {
            hpxml: InputSource::Path(fixture_hpxml_path()),
            schedule: Some(InputSource::Path(schedule.to_path_buf())),
            weather: WeatherInput::Path(
                repo_root().join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
            ),
            defaults: DefaultsSource::directory(repo_defaults_path()),
            settings: DwellingSettings::new(
                SimulationConfig {
                    start_time: fixture_start::fixture_start(),
                    duration: Duration::hours(24),
                    time_res: Duration::seconds(900),
                    reports: Vec::new(),
                    diagnostics: false,
                    metrics: false,
                    output_chunk_size: 10_000,
                    setpoint_deadband_c: None,
                    master_seed: 0,
                    civil_timezone: None,
                    site_location: hares_io::SiteLocationOverride::default(),
                    max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
                    health_policy: hares_io::HealthPolicy::default(),
                    first_hour_interpolation: hares_io::FirstHour::default(),
                    weather_year: hares_io::WeatherYear::default(),
                    warmup: hares_io::Warmup::Disabled,
                },
                None,
                bldg_id,
                None,
            ),
        };
        config.settings.sim_config.reports = vec![hares_io::ReportSpec {
            name: "frame".to_string(),
            selection: hares_io::Selection::Verbosity(2),
            frequency: hares_io::Frequency::Timestep,
            destination: hares_io::Destination::Memory,
        }];
        let mut dwelling = Dwelling::from_config(config).expect("the home builds");
        let engine = SimulationEngine::new();
        let sim_config = dwelling.sim_config().clone();
        let result = engine
            .run_dwelling(&mut dwelling, &sim_config)
            .expect("the home runs");
        let batches: Vec<RecordBatch> = result
            .reports
            .iter()
            .flat_map(|report| report.batches().to_vec())
            .collect();
        let power_slot = batches
            .first()
            .expect("the memory report recorded")
            .schema_ref()
            .fields()
            .iter()
            .position(|field| field.name() == "Total Electric Power (kW)")
            .expect("the power column exists at tier 2");
        let mut means = Vec::new();
        let mut steps: Vec<f64> = Vec::new();
        for batch in &batches {
            let values = batch
                .column(power_slot)
                .as_any()
                .downcast_ref::<Float64Array>()
                .expect("a Float64 column");
            for row in 0..batch.num_rows() {
                steps.push(values.value(row));
                if steps.len() == 4 {
                    means.push(steps.iter().sum::<f64>() / 4.0);
                    steps.clear();
                }
            }
        }
        means
    }

    /// A home that finishes with a warning is never a fleet success: the
    /// warned home is degraded with its reason naming the event, its
    /// weight sits outside weight_ok, it counts out of n_succeeded, and
    /// it enters no cell of the aggregate or the covered weight.
    #[test]
    fn flagged_home_is_not_counted_as_success() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let dir = tempdir().expect("temp dir");
        let unread = schedule_with_unread_column(dir.path());
        let fleet = fl2_fleet(&unread, false);

        let result = fleet
            .simulate(&report_24h_hourly(), 1, chunk(64))
            .expect("the fleet runs");
        assert_eq!(result.homes.len(), 2);

        // The canary: the stock home is ok, so a warning the fixture
        // carries fails here, naming it.
        let table = result.homes_table();
        let statuses = table_string_column(&table, "status");
        let reasons = table_string_column(&table, "reason");
        assert_eq!(statuses[0].as_deref(), Some("ok"), "the stock home is ok");
        assert_eq!(
            statuses[1].as_deref(),
            Some("degraded"),
            "the warned home is not a success"
        );
        let reason = reasons[1]
            .as_deref()
            .expect("the degraded home names its reason");
        assert!(
            reason.contains("unread_load"),
            "the reason names the event: {reason}"
        );

        // The success count is the ok homes alone, and the degraded
        // home's weight sits outside weight_ok.
        assert_eq!(result.n_succeeded, 1);
        assert_eq!(result.coverage.weight_ok, 1.0);

        // The degraded home enters no cell: the covered weight is the
        // clean home's 1.0 in every produced cell. A column no home
        // produced (the unmet-load column no producer claims) is
        // uncovered in every row.
        assert_eq!(result.aggregate.num_rows(), 24, "24 hourly periods");
        for slot in 1..result.covered_weight.num_columns() {
            let weights = table_float_column(
                &result.covered_weight,
                result.covered_weight.schema_ref().field(slot).name(),
            );
            if weights.iter().all(Option::is_none) {
                continue;
            }
            for (row, weight) in weights.iter().enumerate() {
                let weight = weight.expect("every produced cell is covered by the clean home");
                if weight == 0.0 {
                    // No home produced the variable here (the unmet-load
                    // column no producer claims): the covered weight's
                    // honest zero.
                    continue;
                }
                assert_eq!(
                    weight.to_bits(),
                    1.0_f64.to_bits(),
                    "row {row}: only the clean home's weight"
                );
            }
        }

        // Every aggregate cell is the clean home's own period value: the
        // power column equals its standalone hourly means on bits.
        let means = standalone_hourly_power_means(
            1,
            &repo_root().join("data/examples/BEopt_example_schedule.csv"),
        );
        assert_eq!(means.len(), 24);
        let power = table_float_column(&result.aggregate, "Total Electric Power (kW)");
        for (row, mean) in means.iter().enumerate() {
            let cell = power[row].expect("the cell is covered");
            assert_eq!(
                cell.to_bits(),
                mean.to_bits(),
                "row {row}: the clean home's period value alone"
            );
        }
    }

    /// The one classification: clean health with no warning and a
    /// disabled or converged warm-up is `Ok`; each health counter alone,
    /// an unconverged warm-up alone and one warning alone are each
    /// `Degraded` with a reason naming that event; an error, a
    /// `HealthPolicyViolation` included, is `Failed` naming it.
    #[test]
    fn home_status_follows_health_and_warnings() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let run = |health: RunHealth, warnings: Vec<hares_types::Warning>| RunResult {
            reports: Vec::new(),
            metrics: None,
            health,
            warnings,
            warnings_dropped: 0,
            provenance: Provenance {
                build: hares_core::BuildInfo::get(),
                catalogue_digest: [0; 32],
                settings: DwellingSettings::new(
                    simulation_config(PathBuf::from("unused.csv")),
                    None,
                    1,
                    None,
                ),
                site_utc_offset_s: 0,
                equipment: Vec::new(),
                inputs: None,
                tariff: None,
                replay_blockers: Vec::new(),
            },
        };
        let status = |run: &RunResult| home_status(&Ok(run.clone()));

        // Clean health, no warning, the warm-up disabled: Ok.
        assert_eq!(
            status(&run(RunHealth::default(), Vec::new())),
            HomeStatus::Ok
        );

        // A converged warm-up is not an event.
        let converged = RunHealth {
            warmup: WarmupOutcome::Ran {
                days_run: 2,
                converged: true,
                residuals: None,
            },
            ..RunHealth::default()
        };
        assert_eq!(status(&run(converged, Vec::new())), HomeStatus::Ok);

        // Each non-zero health counter alone is Degraded naming it.
        let counters = [
            "port_rollbacks",
            "rejected_control_signals",
            "clamped_actions",
            "curve_index_clamps",
            "fluid_payload_absences",
            "evicted_actors",
        ];
        for name in counters {
            let mut health = RunHealth::default();
            match name {
                "port_rollbacks" => health.port_rollbacks = 1,
                "rejected_control_signals" => health.rejected_control_signals = 1,
                "clamped_actions" => health.clamped_actions = 1,
                "curve_index_clamps" => health.curve_index_clamps = 1,
                "fluid_payload_absences" => health.fluid_payload_absences = 1,
                "evicted_actors" => health.evicted_actors = 1,
                other => panic!("no such counter: {other}"),
            }
            match status(&run(health, Vec::new())) {
                HomeStatus::Degraded(reason) => assert!(
                    reason.contains(name) && reason.contains('1'),
                    "the reason names the counter and its count: {reason}"
                ),
                other => panic!("{name} alone must be degraded, got {other:?}"),
            }
        }

        // An unconverged warm-up alone is Degraded naming it.
        let unconverged = RunHealth {
            warmup: WarmupOutcome::Ran {
                days_run: 5,
                converged: false,
                residuals: None,
            },
            ..RunHealth::default()
        };
        match status(&run(unconverged, Vec::new())) {
            HomeStatus::Degraded(reason) => assert!(
                reason.contains("warm-up"),
                "the reason names the warm-up: {reason}"
            ),
            other => panic!("an unconverged warm-up must be degraded, got {other:?}"),
        }

        // One warning alone is Degraded naming it.
        let warning = hares_types::Warning::new("schedule", "the unmapped column");
        match status(&run(RunHealth::default(), vec![warning])) {
            HomeStatus::Degraded(reason) => assert!(
                reason.contains("schedule") && reason.contains("the unmapped column"),
                "the reason names the warning: {reason}"
            ),
            other => panic!("a warning must be degraded, got {other:?}"),
        }

        // A run error is Failed with the error.
        let err = HaresError::Simulation("the run failed".to_string());
        match home_status(&Err(err)) {
            HomeStatus::Failed(reason) => {
                assert!(reason.contains("the run failed"), "{reason}");
            }
            other => panic!("an error must be failed, got {other:?}"),
        }

        // A health-policy violation is Failed naming it.
        let violation = HaresError::HealthPolicyViolation {
            event: hares_types::HealthEvent::ClampedAction,
            step_index: Some(7),
            origin: "the equipment".to_string(),
            count: 1,
            limit: 0,
        };
        match home_status(&Err(violation)) {
            HomeStatus::Failed(reason) => {
                assert!(
                    reason.contains("health policy") && reason.contains("the equipment"),
                    "the failure names the violation: {reason}"
                );
            }
            other => panic!("a policy violation must be failed, got {other:?}"),
        }
    }

    /// The battery fleet: the plain home and the battery home over the
    /// committed fixtures, at 24 h and 900 s, the battery declaring the
    /// `Manual` control its test requires. The plain home is entry 1 at
    /// weight 3, the battery home entry 2 at weight 1; the battery
    /// variable's name comes from the battery home's own catalogue.
    fn battery_fleet(order_battery_first: bool, metrics: bool) -> (Fleet, String) {
        let examples = repo_root().join("data/examples");
        let fixtures = repo_root().join("tests/fixtures/hpxml/ochre_samples");
        let home =
            |bldg_id: i64, hpxml: PathBuf, overrides: Option<serde_json::Value>| DwellingConfig {
                hpxml: InputSource::Path(hpxml),
                schedule: Some(InputSource::Path(
                    examples.join("BEopt_example_schedule.csv"),
                )),
                weather: WeatherInput::Path(examples.join("USA_CO_Denver.Intl.AP.725650_TMY3.epw")),
                defaults: DefaultsSource::directory(repo_defaults_path()),
                settings: DwellingSettings::new(
                    SimulationConfig {
                        start_time: fixture_start::fixture_start(),
                        duration: Duration::hours(24),
                        time_res: Duration::seconds(900),
                        reports: Vec::new(),
                        diagnostics: false,
                        metrics,
                        output_chunk_size: 10_000,
                        setpoint_deadband_c: None,
                        master_seed: 0,
                        civil_timezone: None,
                        site_location: hares_io::SiteLocationOverride::default(),
                        max_consecutive_step_failures:
                            hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
                        health_policy: hares_io::HealthPolicy::default(),
                        first_hour_interpolation: hares_io::FirstHour::default(),
                        weather_year: hares_io::WeatherYear::default(),
                        warmup: hares_io::Warmup::Disabled,
                    },
                    overrides,
                    bldg_id,
                    None,
                ),
            };
        let plain = home(1, fixtures.join("base.xml"), None);
        let battery = home(
            2,
            fixtures.join("base-battery.xml"),
            Some(serde_json::json!({"Battery": {"bms_mode": "\"Manual\""}})),
        );
        let battery_variable = Dwelling::from_config(battery.clone())
            .expect("the battery home builds")
            .catalogue()
            .variables()
            .iter()
            .map(|def| def.name.to_string())
            .find(|name| name.contains("Battery"))
            .expect("the battery home declares battery variables");
        let configs = if order_battery_first {
            // The battery home is entry 1 at weight 1, the plain home
            // entry 2 at weight 3.
            Fleet::from_buildings(vec![battery, plain])
                .expect("the homes carry no reserved report or shared destination")
                .with_sample_weights(vec![1.0, 3.0])
                .expect("the weights are finite and non-negative")
        } else {
            Fleet::from_buildings(vec![plain, battery])
                .expect("the homes carry no reserved report or shared destination")
                .with_sample_weights(vec![3.0, 1.0])
                .expect("the weights are finite and non-negative")
        };
        (configs, battery_variable)
    }

    /// A home whose restricted selection holds no variable is ok and
    /// uncovered: it folds nothing, keeps periods_folded 0, and counts in
    /// weight_ok and n_succeeded; the aggregate's rows are the folding
    /// home's, the battery column's covered weight its weight alone.
    #[test]
    fn home_with_no_selected_variable_is_ok_and_uncovered() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (fleet, battery_variable) = battery_fleet(false, false);
        let report = crate::fold::FleetReport {
            selection: hares_io::Selection::Named(vec![battery_variable]),
            frequency: hares_io::Frequency::Hourly,
            health_policy: hares_io::HealthPolicy::default(),
        };
        let result = fleet
            .simulate(&report, 1, chunk(64))
            .expect("the fleet runs");

        // The plain home is ok and uncovered; the battery home folds.
        assert_eq!(result.homes[0].status, HomeStatus::Ok);
        assert_eq!(result.homes[0].periods_folded, 0);
        assert_eq!(result.homes[1].status, HomeStatus::Ok);
        assert_eq!(result.homes[1].periods_folded, 24);
        assert_eq!(result.coverage.weight_ok, 4.0);
        assert_eq!(result.n_succeeded, 2);

        // The aggregate: the battery home's 24 hourly rows, its column's
        // covered weight 1 in each.
        assert_eq!(result.aggregate.num_rows(), 24);
        let weight_name = result.covered_weight.schema_ref().field(1).name().clone();
        let weights = table_float_column(&result.covered_weight, &weight_name);
        for (row, weight) in weights.iter().enumerate() {
            assert_eq!(
                weight.expect("covered").to_bits(),
                1.0_f64.to_bits(),
                "row {row}: the battery home's weight alone"
            );
        }
    }

    /// One standalone home's hourly period table: per variable column,
    /// the period value in bucket order, by the column's over-time rule
    /// (the four 900 s steps summed or meaned as the rule says), the
    /// column names alongside.
    fn standalone_home_hourly(config: &DwellingConfig) -> (Vec<String>, Vec<Vec<Option<f64>>>) {
        let mut config = config.clone();
        config.settings.sim_config.reports = vec![hares_io::ReportSpec {
            name: "frame".to_string(),
            selection: hares_io::Selection::Verbosity(2),
            frequency: hares_io::Frequency::Timestep,
            destination: hares_io::Destination::Memory,
        }];
        let mut dwelling = Dwelling::from_config(config).expect("the home builds");
        let engine = SimulationEngine::new();
        let sim_config = dwelling.sim_config().clone();
        let result = engine
            .run_dwelling(&mut dwelling, &sim_config)
            .expect("the home runs");
        let batches: Vec<RecordBatch> = result
            .reports
            .iter()
            .flat_map(|report| report.batches().to_vec())
            .collect();
        let schema = batches
            .first()
            .expect("the memory report recorded")
            .schema_ref()
            .clone();
        let names: Vec<String> = schema
            .fields()
            .iter()
            .skip(1)
            .map(|field| field.name().clone())
            .collect();
        let over_time: Vec<hares_types::Aggregation> = schema
            .fields()
            .iter()
            .skip(1)
            .map(|field| {
                crate::fold::field_fleet_rules(field)
                    .expect("the schema builder writes the kind tag")
                    .0
                    .aggregation
            })
            .collect();
        // Null-aware period building, the reducer's own rule: a null cell
        // (a variable no step produced: the unmet-load column no
        // producer claims) contributes to neither the sum nor the valid
        // count, the period closes on the row count (four rows to the
        // hour), a Sum period sums the valid steps, a Mean divides by the
        // valid count, and a period with no valid step is null.
        let mut columns: Vec<Vec<Option<f64>>> = names.iter().map(|_| Vec::new()).collect();
        let mut sums = vec![0.0; names.len()];
        let mut counts = vec![0_u32; names.len()];
        let mut rows_in_bucket = 0_usize;
        for batch in &batches {
            for row in 0..batch.num_rows() {
                for (slot, _) in columns.iter().enumerate() {
                    let values = batch
                        .column(slot + 1)
                        .as_any()
                        .downcast_ref::<Float64Array>()
                        .expect("a Float64 column");
                    if !values.is_null(row) {
                        sums[slot] += values.value(row);
                        counts[slot] += 1;
                    }
                }
                rows_in_bucket += 1;
                if rows_in_bucket == 4 {
                    for (slot, column) in columns.iter_mut().enumerate() {
                        // This helper's periods carry Sum and Mean rules
                        // only: a Mode column reaches the fleet's fold
                        // through the reducer's own mode rule upstream,
                        // not through this helper.
                        let reduced = match over_time[slot] {
                            hares_types::Aggregation::Sum if counts[slot] > 0 => Some(sums[slot]),
                            hares_types::Aggregation::Mean if counts[slot] > 0 => {
                                Some(sums[slot] / f64::from(counts[slot]))
                            }
                            _ => None,
                        };
                        column.push(reduced);
                        sums[slot] = 0.0;
                        counts[slot] = 0;
                    }
                    rows_in_bucket = 0;
                }
            }
        }
        (names, columns)
    }

    /// The battery home and the plain home aggregate together: the
    /// aggregate's columns are the union of the two homes' variables,
    /// each battery variable's cell the battery home's period value with
    /// covered weight 1 (the plain home contributes neither value nor
    /// weight to a variable it does not declare), and every shared
    /// variable the combination of both homes' period values by the rule
    /// in force.
    #[test]
    fn battery_home_and_plain_home_aggregate_together() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (fleet, _battery_variable) = battery_fleet(true, false);
        let report = crate::fold::FleetReport {
            selection: hares_io::Selection::Verbosity(2),
            frequency: hares_io::Frequency::Hourly,
            health_policy: hares_io::HealthPolicy::default(),
        };
        let result = fleet
            .simulate(&report, 1, chunk(64))
            .expect("the fleet runs");

        // Both homes are ok (the canary), both fold.
        for home in &result.homes {
            assert_eq!(home.status, HomeStatus::Ok, "{}", home.bldg_id);
        }
        assert_eq!(result.coverage.weight_ok, 4.0);
        assert_eq!(result.n_succeeded, 2);
        assert!(result.aggregate.num_rows() > 0, "a non-empty aggregate");

        // The two homes' standalone hourly tables: the source of the
        // hand-computed cells.
        let examples = repo_root().join("data/examples");
        let fixtures = repo_root().join("tests/fixtures/hpxml/ochre_samples");
        let plain_config = DwellingConfig {
            hpxml: InputSource::Path(fixtures.join("base.xml")),
            schedule: Some(InputSource::Path(
                examples.join("BEopt_example_schedule.csv"),
            )),
            weather: WeatherInput::Path(examples.join("USA_CO_Denver.Intl.AP.725650_TMY3.epw")),
            defaults: DefaultsSource::directory(repo_defaults_path()),
            settings: DwellingSettings::new(
                SimulationConfig {
                    start_time: fixture_start::fixture_start(),
                    duration: Duration::hours(24),
                    time_res: Duration::seconds(900),
                    reports: Vec::new(),
                    diagnostics: false,
                    metrics: false,
                    output_chunk_size: 10_000,
                    setpoint_deadband_c: None,
                    master_seed: 0,
                    civil_timezone: None,
                    site_location: hares_io::SiteLocationOverride::default(),
                    max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
                    health_policy: hares_io::HealthPolicy::default(),
                    first_hour_interpolation: hares_io::FirstHour::default(),
                    weather_year: hares_io::WeatherYear::default(),
                    warmup: hares_io::Warmup::Disabled,
                },
                None,
                1,
                None,
            ),
        };
        let battery_config = DwellingConfig {
            hpxml: InputSource::Path(fixtures.join("base-battery.xml")),
            settings: DwellingSettings {
                overrides: Some(serde_json::json!({"Battery": {"bms_mode": "\"Manual\""}})),
                bldg_id: 2,
                ..plain_config.settings.clone()
            },
            ..plain_config.clone()
        };
        let (plain_names, plain_columns) = standalone_home_hourly(&plain_config);
        let (battery_names, battery_columns) = standalone_home_hourly(&battery_config);

        // The union's first appearance is the battery home's schema (it
        // is entry 1 and declares the superset), then the plain home's
        // extras, if any.
        let aggregate_names: Vec<String> = result
            .aggregate
            .schema_ref()
            .fields()
            .iter()
            .skip(1)
            .map(|field| field.name().clone())
            .collect();
        for name in &plain_names {
            assert!(
                battery_names.contains(name),
                "the battery home declares the plain home's {name}"
            );
        }
        for (position, name) in aggregate_names.iter().enumerate() {
            let expected = battery_names
                .iter()
                .position(|candidate| candidate == name)
                .unwrap_or_else(|| {
                    plain_names
                        .iter()
                        .position(|candidate| candidate == name)
                        .expect("the union holds only the homes' variables")
                });
            assert_eq!(
                position, expected,
                "{name}: the union keeps the first appearance's position"
            );
        }

        // Every cell of every column, on bits: a battery-only variable
        // carries the battery home's value with covered weight 1; a
        // shared variable the weighted combination by its rule, with the
        // two homes' covered weight 4. A column no home produced (the
        // unmet-load column no producer claims) is null in the aggregate
        // and uncovered in every row.
        let rows = result.aggregate.num_rows();
        for (slot, name) in aggregate_names.iter().enumerate() {
            let battery_only = !plain_names.contains(name);
            let battery_slot = battery_names
                .iter()
                .position(|candidate| candidate == name)
                .expect("the battery home declares the column");
            let plain_slot = plain_names.iter().position(|candidate| candidate == name);
            let battery = &battery_columns[battery_slot];
            let field = result.aggregate.schema_ref().field(slot + 1);
            let across = crate::fold::field_fleet_rules(field)
                .expect("the schema builder writes the kind tag")
                .1;
            let aggregate_column = table_float_column(&result.aggregate, name);
            let weight_column = table_float_column(&result.covered_weight, name);
            for row in 0..rows {
                let weight = weight_column[row];
                let Some(weight) = weight else {
                    assert_eq!(
                        aggregate_column[row], None,
                        "{name} row {row}: an uncovered cell's aggregate is null"
                    );
                    continue;
                };
                if weight == 0.0 {
                    // No home produced the variable here (the unmet-load
                    // column no producer claims): the aggregate cell is
                    // null and the covered weight is its honest zero.
                    assert_eq!(
                        aggregate_column[row], None,
                        "{name} row {row}: a zero-covered cell's aggregate is null"
                    );
                    continue;
                }
                if battery_only {
                    assert_eq!(
                        weight.to_bits(),
                        1.0_f64.to_bits(),
                        "{name} row {row}: the battery home's weight alone"
                    );
                    assert_eq!(
                        aggregate_column[row].expect("covered").to_bits(),
                        battery[row]
                            .expect("the battery home's period value")
                            .to_bits(),
                        "{name} row {row}: the battery home's period value"
                    );
                } else {
                    let plain = &plain_columns[plain_slot.expect("the plain home declares it")];
                    let plain = plain[row].unwrap_or_else(|| {
                        panic!("{name} row {row}: the plain home's period value")
                    });
                    let battery = battery[row].unwrap_or_else(|| {
                        panic!("{name} row {row}: the battery home's period value")
                    });
                    let expected = match across {
                        hares_types::AcrossHomes::Sum => plain * 3.0 + battery,
                        hares_types::AcrossHomes::Mean => (plain * 3.0 + battery) / 4.0,
                    };
                    assert_eq!(
                        weight.to_bits(),
                        4.0_f64.to_bits(),
                        "{name} row {row}: both homes' weights"
                    );
                    assert_eq!(
                        aggregate_column[row].expect("covered").to_bits(),
                        expected.to_bits(),
                        "{name} row {row}: the weighted combination"
                    );
                }
            }
        }
    }

    /// The fleet's run metrics ride the homes' own request: with it, the
    /// aggregate is non-empty and every home's metrics are non-zero; the
    /// degraded home's included. Without it, the homes table carries no
    /// metrics columns.
    #[test]
    fn default_fleet_produces_aggregate_and_metrics() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let dir = tempdir().expect("temp dir");
        let unread = schedule_with_unread_column(dir.path());

        {
            let fleet = fl2_fleet(&unread, true);
            let result = fleet
                .simulate(&report_24h_hourly(), 1, chunk(64))
                .expect("the fleet runs");
            assert!(result.aggregate.num_rows() > 0, "a non-empty aggregate");
            for home in &result.homes {
                let metrics = home
                    .metrics
                    .as_ref()
                    .expect("the home's config requested metrics");
                assert!(
                    metrics.total_energy_kwh.net_energy_kwh != 0.0,
                    "home {}'s metrics are non-zero",
                    home.bldg_id
                );
            }
            let table = result.homes_table();
            let names: Vec<String> = table
                .schema_ref()
                .fields()
                .iter()
                .map(|field| field.name().clone())
                .collect();
            assert!(names.contains(&"total_energy_kwh".to_string()));
        }

        let fleet = fl2_fleet(&unread, false);
        let result = fleet
            .simulate(&report_24h_hourly(), 1, chunk(64))
            .expect("the fleet runs");
        let names: Vec<String> = result
            .homes_table()
            .schema_ref()
            .fields()
            .iter()
            .map(|field| field.name().clone())
            .collect();
        assert!(
            !names.contains(&"total_energy_kwh".to_string()),
            "no home requested metrics: the homes table carries none"
        );
    }

    /// A fleet whose every home is degraded has no covered ok weight: the
    /// NoCoveredWeight error carries the counts per status and the full
    /// homes table, so every home's status and reason survive the error.
    #[test]
    fn zero_ok_weight_raises() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let dir = tempdir().expect("temp dir");
        let unread = schedule_with_unread_column(dir.path());
        let home = |bldg_id: i64| DwellingConfig {
            hpxml: InputSource::Path(fixture_hpxml_path()),
            schedule: Some(InputSource::Path(unread.clone())),
            weather: WeatherInput::Path(
                repo_root().join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
            ),
            defaults: DefaultsSource::directory(repo_defaults_path()),
            settings: DwellingSettings::new(
                SimulationConfig {
                    start_time: fixture_start::fixture_start(),
                    duration: Duration::hours(24),
                    time_res: Duration::seconds(900),
                    reports: Vec::new(),
                    diagnostics: false,
                    metrics: false,
                    output_chunk_size: 10_000,
                    setpoint_deadband_c: None,
                    master_seed: 0,
                    civil_timezone: None,
                    site_location: hares_io::SiteLocationOverride::default(),
                    max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
                    health_policy: hares_io::HealthPolicy::default(),
                    first_hour_interpolation: hares_io::FirstHour::default(),
                    weather_year: hares_io::WeatherYear::default(),
                    warmup: hares_io::Warmup::Disabled,
                },
                None,
                bldg_id,
                None,
            ),
        };
        let fleet = Fleet::from_buildings(vec![home(1), home(2)])
            .expect("the homes carry no reserved report or shared destination");

        let err = fleet
            .simulate(&report_24h_hourly(), 1, chunk(64))
            .expect_err("no home is ok: the ok weight is zero");
        let message = err.to_string();
        assert!(
            message.contains("0 ok")
                && message.contains("2 degraded")
                && message.contains("0 failed"),
            "the message names the counts: {message}"
        );
        match err {
            FleetError::NoCoveredWeight {
                ok,
                degraded,
                failed,
                homes,
            } => {
                assert_eq!((ok, degraded, failed), (0, 2, 0));
                let ids = homes
                    .column(0)
                    .as_any()
                    .downcast_ref::<arrow::array::Int64Array>()
                    .expect("an Int64 bldg_id column");
                let statuses = table_string_column(&homes, "status");
                let reasons = table_string_column(&homes, "reason");
                assert_eq!(ids.value(0), 1);
                assert_eq!(ids.value(1), 2);
                for row in 0..2 {
                    assert_eq!(statuses[row].as_deref(), Some("degraded"));
                    let reason = reasons[row]
                        .as_deref()
                        .expect("the degraded home names its reason");
                    assert!(
                        reason.contains("unread_load"),
                        "the reason names the event: {reason}"
                    );
                }
            }
            other => panic!("expected NoCoveredWeight, got {other:?}"),
        }
    }

    // -------------------------------------------------------------------
    // The declared time alignment's acceptance.
    // -------------------------------------------------------------------

    /// One home of the mixed-offset fleet: the `fleet_mixed_offset`
    /// manifest's homes (bldg 2 at UTC-5, bldg 4 at UTC-6, bldg 176227 at
    /// UTC-8), weight 1.0, 7 d at 3600 s, each starting at its local
    /// `2018-01-01T00:00` unless `local_start_hour` moves it.
    fn mixed_offset_config(
        bldg_id: i64,
        home: &str,
        weather: &str,
        local_start_hour: u32,
    ) -> DwellingConfig {
        DwellingConfig {
            hpxml: InputSource::Path(resstock_fixture(&format!("{home}/home.xml"))),
            schedule: Some(InputSource::Path(resstock_fixture(&format!(
                "{home}/in.schedules.csv"
            )))),
            weather: WeatherInput::Path(resstock_fixture("weather").join(weather)),
            defaults: DefaultsSource::directory(repo_defaults_path()),
            settings: DwellingSettings::new(
                SimulationConfig {
                    start_time: FixedOffset::east_opt(0)
                        .expect("valid offset")
                        .with_ymd_and_hms(2018, 1, 1, local_start_hour, 0, 0)
                        .single()
                        .expect("valid start"),
                    duration: Duration::days(7),
                    time_res: Duration::seconds(3600),
                    reports: Vec::new(),
                    diagnostics: false,
                    metrics: false,
                    output_chunk_size: 10_000,
                    setpoint_deadband_c: None,
                    master_seed: 0,
                    civil_timezone: None,
                    site_location: hares_io::SiteLocationOverride::default(),
                    max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
                    health_policy: hares_io::HealthPolicy::default(),
                    first_hour_interpolation: hares_io::FirstHour::default(),
                    weather_year: hares_io::WeatherYear::default(),
                    warmup: hares_io::Warmup::default(),
                },
                None,
                bldg_id,
                None,
            ),
        }
    }

    /// The mixed-offset fleet: the manifest's three homes and weights.
    fn mixed_offset_fleet(start_hours: [u32; 3]) -> Fleet {
        Fleet::from_buildings(vec![
            mixed_offset_config(2, "bldg0000002", "G0900090_2018.csv", start_hours[0]),
            mixed_offset_config(4, "bldg0000004", "G4803030_2018.csv", start_hours[1]),
            mixed_offset_config(176227, "bldg0176227", "G0600770_2018.csv", start_hours[2]),
        ])
        .expect("the manifest's homes carry no reserved report or shared destination")
        .with_sample_weights(vec![1.0, 1.0, 1.0])
        .expect("the manifest's weights are finite and non-negative")
    }

    fn hourly_named_common() -> FleetReport {
        FleetReport {
            selection: hares_io::Selection::Named(vec!["Total Electric Power (kW)".to_string()]),
            frequency: hares_io::Frequency::Hourly,
            health_policy: hares_io::HealthPolicy::default(),
        }
    }

    fn daily_named_common() -> FleetReport {
        FleetReport {
            selection: hares_io::Selection::Named(vec!["Total Electric Power (kW)".to_string()]),
            frequency: hares_io::Frequency::Daily,
            health_policy: hares_io::HealthPolicy::default(),
        }
    }

    /// The fleet aggregate's `Time` labels, in milliseconds: the column
    /// is a naive `Timestamp(Millisecond, None)` (B-12).
    fn time_labels_of(batch: &RecordBatch) -> Vec<i64> {
        let values = batch
            .column(0)
            .as_any()
            .downcast_ref::<TimestampMillisecondArray>()
            .expect("a Timestamp Time column");
        (0..values.len()).map(|row| values.value(row)).collect()
    }

    /// The local wall clock a naive millisecond label denotes, as text
    /// for the label assertions.
    fn label_text(ms: i64) -> String {
        chrono::DateTime::from_timestamp_millis(ms)
            .expect("a representable label")
            .naive_utc()
            .format("%Y-%m-%dT%H:%M:%S")
            .to_string()
    }

    /// The naive millisecond label of a local wall clock.
    fn naive_ms(y: i32, m: u32, d: u32, h: u32, min: u32) -> i64 {
        chrono::NaiveDate::from_ymd_opt(y, m, d)
            .expect("a date")
            .and_hms_opt(h, min, 0)
            .expect("a time")
            .and_utc()
            .timestamp_millis()
    }

    /// The fleet tables' `hares:utc_offset_s` metadata: the offset (or
    /// the declared reference zone) the labels carry, absent under
    /// `LocalClock` (each home's own wall clock).
    fn offset_metadata_of(batch: &RecordBatch) -> Option<i32> {
        batch
            .schema_ref()
            .metadata()
            .get("hares:utc_offset_s")
            .map(|value| value.parse::<i32>().expect("an offset in seconds"))
    }

    /// A fleet whose homes span more than one UTC offset and declares no
    /// alignment fails before any home runs, naming the distinct offsets.
    #[test]
    fn mixed_offset_fleet_requires_declared_alignment() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let fleet = mixed_offset_fleet([0, 0, 0]).with_progress(move |_, _| {
            panic!("no home ran: the progress callback must stay silent");
        });
        let err = fleet
            .simulate(&hourly_named_common(), 1, chunk(64))
            .expect_err("a mixed-offset fleet without a declared alignment fails");
        assert!(
            matches!(
                &err,
                FleetError::AlignmentRequired { offsets_s }
                    if offsets_s == &vec![-18000, -21600, -28800]
            ),
            "got: {err:?}"
        );
    }

    /// The pre-pass's site offset equals the offset the built dwelling
    /// runs at: the same resolution reads the same inputs.
    #[test]
    fn site_offset_pre_pass_matches_built_dwellings() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        for (bldg_id, home, weather) in [
            (2, "bldg0000002", "G0900090_2018.csv"),
            (4, "bldg0000004", "G4803030_2018.csv"),
            (176227, "bldg0176227", "G0600770_2018.csv"),
        ] {
            let config = mixed_offset_config(bldg_id, home, weather, 0);
            let offset = hares_core::dwelling::site_offset(&config).expect("the site resolves");
            let dwelling = Dwelling::from_config(config).expect("the dwelling builds");
            assert_eq!(
                offset.local_minus_utc(),
                dwelling.clock.start_time.offset().local_minus_utc(),
                "home {bldg_id}: the pre-pass and the built dwelling agree"
            );
        }
    }

    /// The pre-pass reads only the site and the weather header: a config
    /// whose schedule and defaults are missing and whose weather file
    /// carries the header line only resolves the same offset.
    #[test]
    fn site_offset_pre_pass_reads_only_the_site_and_the_weather_header() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let config = mixed_offset_config(2, "bldg0000002", "G0900090_2018.csv", 0);
        let expected = hares_core::dwelling::site_offset(&config).expect("the site resolves");

        let dir = tempdir().expect("temp dir");
        let weather_path = dir.path().join("G0900090_2018.csv");
        let original =
            std::fs::read_to_string(config.weather.path().expect("the weather path resolves"))
                .expect("weather read");
        let header = original.lines().next().expect("a header line");
        write_temp_file(&weather_path, header);

        let stripped = DwellingConfig {
            weather: WeatherInput::Path(weather_path.clone()),
            schedule: Some(InputSource::Path(dir.path().join("missing_schedule.csv"))),
            defaults: DefaultsSource::directory(dir.path().join("missing_defaults")),
            ..config
        };
        let offset = hares_core::dwelling::site_offset(&stripped)
            .expect("the header-only inputs resolve the same site");
        assert_eq!(
            offset.local_minus_utc(),
            expected.local_minus_utc(),
            "the pre-pass never reads a schedule, a defaults tree or a weather row"
        );
    }

    /// Homes whose start instants are equal cover the same steps of every
    /// reference period: the aligned fold carries every period at full
    /// covered weight, labelled at the declared offset.
    #[test]
    fn instant_alignment_of_one_start_instant_covers_every_period() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        // 03:00 local at UTC-5, 02:00 at UTC-6 and 00:00 at UTC-8 are one
        // instant (08:00Z).
        let fleet = mixed_offset_fleet([3, 2, 0]).with_alignment(Alignment::Instant(
            FixedOffset::west_opt(8 * 3600).expect("valid offset"),
        ));
        let result = fleet
            .simulate(&daily_named_common(), 1, chunk(64))
            .expect("the fleet runs");
        for home in &result.homes {
            assert!(
                !matches!(home.status, HomeStatus::Failed(_)),
                "every home runs: {:?}",
                home.status
            );
        }
        let times = time_labels_of(&result.aggregate);
        let expected: Vec<i64> = (0..7)
            .map(|day| naive_ms(2018, 1, 1 + day as u32, 0, 0))
            .collect();
        assert_eq!(
            times, expected,
            "the labels are the declared offset's wall clock's midnights, naive"
        );
        for slot in 1..result.covered_weight.num_columns() {
            let weights = result
                .covered_weight
                .column(slot)
                .as_any()
                .downcast_ref::<Float64Array>()
                .expect("a Float64 covered weight column");
            for row in 0..times.len() {
                assert_eq!(
                    weights.value(row).to_bits(),
                    3.0_f64.to_bits(),
                    "row {row}: every home covers every reference hour at full weight"
                );
            }
        }
    }

    /// A start instant off the declared offset's step grid is the grid
    /// check's error, naming the home.
    #[test]
    fn instant_start_off_the_reference_step_grid_is_an_error() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut configs = vec![
            mixed_offset_config(2, "bldg0000002", "G0900090_2018.csv", 3),
            mixed_offset_config(4, "bldg0000004", "G4803030_2018.csv", 2),
            mixed_offset_config(176227, "bldg0176227", "G0600770_2018.csv", 0),
        ];
        // Break the UTC-8 home's start to 00:30 local: its start instant
        // lands a half hour off the declared offset's step grid.
        configs[2].settings.sim_config.start_time = FixedOffset::west_opt(8 * 3600)
            .expect("valid offset")
            .with_ymd_and_hms(2018, 1, 1, 0, 30, 0)
            .single()
            .expect("valid start");
        let fleet = Fleet::from_buildings(configs)
            .expect("the homes carry no reserved report or shared destination")
            .with_sample_weights(vec![1.0, 1.0, 1.0])
            .expect("the weights are finite and non-negative")
            .with_alignment(Alignment::Instant(
                FixedOffset::west_opt(8 * 3600).expect("valid offset"),
            ));
        let err = fleet
            .simulate(&hourly_named_common(), 1, chunk(64))
            .expect_err("a start off the reference step grid fails");
        assert!(
            matches!(&err, FleetError::HeterogeneousTimeGrid { bldg_id } if *bldg_id == 176227),
            "got: {err:?}"
        );
    }

    /// `LocalClock` aligns the rows by the naive local period start the
    /// homes share, labelled by that wall clock as a naive `Timestamp`
    /// (B-12): no offset applies, so the schema records none.
    #[test]
    fn local_clock_labels_are_shared_naive_local_time() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let fleet = mixed_offset_fleet([0, 0, 0]).with_alignment(Alignment::LocalClock);
        let result = fleet
            .simulate(&hourly_named_common(), 1, chunk(64))
            .expect("the fleet runs");
        for home in &result.homes {
            assert!(
                !matches!(home.status, HomeStatus::Failed(_)),
                "every home runs: {:?}",
                home.status
            );
        }
        let time_field = result.aggregate.schema_ref().field(0).clone();
        assert_eq!(
            time_field.data_type(),
            &DataType::Timestamp(TimeUnit::Millisecond, None)
        );
        assert_eq!(
            time_labels_of(&result.aggregate).first().expect("a row"),
            &naive_ms(2018, 1, 1, 0, 0),
            "the first row is the homes' shared local midnight, naive"
        );
        assert_eq!(
            offset_metadata_of(&result.aggregate),
            None,
            "each home's own wall clock: no single offset to record"
        );
        for slot in 1..result.covered_weight.num_columns() {
            let weights = result
                .covered_weight
                .column(slot)
                .as_any()
                .downcast_ref::<Float64Array>()
                .expect("a Float64 covered weight column");
            for row in 0..result.aggregate.num_rows() {
                assert_eq!(
                    weights.value(row).to_bits(),
                    3.0_f64.to_bits(),
                    "row {row}: every home covers the shared local hour at full weight"
                );
            }
        }
    }

    /// `Instant` buckets on the declared offset's reference boundaries:
    /// the homes' edge periods at the run's ends are partial for the homes
    /// that do not cover them whole, and fold at the reduced covered
    /// weight. The labels are the reference zone's wall clock, naive
    /// `Timestamp` (B-12).
    #[test]
    fn instant_alignment_buckets_on_reference_boundaries() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        // The homes start at their local midnights: instants 05:00Z (UTC-5),
        // 06:00Z (UTC-6) and 08:00Z (UTC-8) - 22:00, 23:00 and 01:00 on the
        // -07:00 reference clock.
        let fleet = mixed_offset_fleet([0, 0, 0]).with_alignment(Alignment::Instant(
            FixedOffset::west_opt(7 * 3600).expect("valid offset"),
        ));
        let report = FleetReport {
            selection: hares_io::Selection::Named(vec!["Total Electric Power (kW)".to_string()]),
            frequency: hares_io::Frequency::Daily,
            health_policy: hares_io::HealthPolicy::default(),
        };
        let result = fleet
            .simulate(&report, 1, chunk(64))
            .expect("the fleet runs");
        let labels = time_labels_of(&result.aggregate);
        let expected: Vec<i64> = (0..7)
            .map(|day| naive_ms(2018, 1, 1 + day as u32, 0, 0))
            .collect();
        assert_eq!(
            labels, expected,
            "the labels are the reference wall clock's midnights, naive"
        );
        assert_eq!(
            offset_metadata_of(&result.aggregate),
            Some(-7 * 3600),
            "the metadata names the declared reference zone"
        );
        let weights = result
            .covered_weight
            .column(1)
            .as_any()
            .downcast_ref::<Float64Array>()
            .expect("a Float64 covered weight column");
        let expected: [f64; 7] = [2.0, 3.0, 3.0, 3.0, 3.0, 3.0, 1.0];
        for (row, &want) in expected.iter().enumerate() {
            assert_eq!(
                weights.value(row).to_bits(),
                want.to_bits(),
                "row {row}: the partial homes fold no weight"
            );
        }
    }

    /// Declaring `Instant(+00:00)` keeps the undeclared single-offset
    /// fleet's numbers bitwise and names the same instants: the number
    /// columns are bitwise equal, both `Time` columns are naive
    /// `Timestamp`s, and the undeclared fleet's label (the homes' local
    /// wall clock) is the declared label (the +00:00 reference wall
    /// clock) shifted by the homes' offset (B-12's local labels: FL-4's
    /// bitwise label equality is gone, the instant equality stays). The
    /// `fleet_mixed_offset` golden under its declared `Instant(+00:00)`
    /// is the compare-all gate.
    #[test]
    fn instant_utc_equals_todays_output() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let undeclared = golden_fleet()
            .simulate(&minutes_15(), 1, chunk(64))
            .expect("the fleet runs");
        let declared = golden_fleet()
            .with_alignment(Alignment::Instant(
                FixedOffset::east_opt(0).expect("a zero offset is representable"),
            ))
            .simulate(&minutes_15(), 1, chunk(64))
            .expect("the fleet runs");
        for (label, left, right) in [
            ("the aggregate", &undeclared.aggregate, &declared.aggregate),
            (
                "the covered weight",
                &undeclared.covered_weight,
                &declared.covered_weight,
            ),
        ] {
            assert_eq!(left.num_rows(), right.num_rows(), "{label}: row count");
            for slot in 1..left.num_columns() {
                assert_eq!(
                    left.schema_ref().field(slot),
                    right.schema_ref().field(slot),
                    "{label}: column {slot} field"
                );
                assert_eq!(
                    left.column(slot).to_data(),
                    right.column(slot).to_data(),
                    "{label}: column {} differs",
                    left.schema_ref().field(slot).name()
                );
            }
        }
        assert_batches_bitwise_equal(
            &undeclared.homes_table(),
            &declared.homes_table(),
            "the homes table",
        );
        let home_offset_ms = i64::from(golden_fleet_offset_s()) * 1000;
        for (label, undeclared_ms, declared_ms) in [
            (
                "the aggregate",
                time_labels_of(&undeclared.aggregate),
                time_labels_of(&declared.aggregate),
            ),
            (
                "the covered weight",
                time_labels_of(&undeclared.covered_weight),
                time_labels_of(&declared.covered_weight),
            ),
        ] {
            assert_eq!(
                undeclared_ms.first().expect("a row"),
                &naive_ms(2018, 1, 1, 0, 0),
                "{label}: the undeclared fleet labels its homes' local midnight (UTC-8)"
            );
            assert_eq!(
                declared_ms.first().expect("a row"),
                &naive_ms(2018, 1, 1, 8, 0),
                "{label}: the declared +00:00 label is the same instant's UTC wall clock"
            );
            assert_eq!(
                declared_ms.len(),
                undeclared_ms.len(),
                "{label}: the same rows"
            );
            for (local, utc) in undeclared_ms.iter().zip(&declared_ms) {
                assert_eq!(
                    utc - local,
                    -home_offset_ms,
                    "{label}: the labels differ by the homes' offset, the instants equal"
                );
            }
        }
        assert_eq!(
            offset_metadata_of(&undeclared.aggregate),
            Some(golden_fleet_offset_s()),
            "the undeclared fleet's metadata names the homes' shared offset"
        );
        assert_eq!(
            offset_metadata_of(&declared.aggregate),
            Some(0),
            "the declared fleet's metadata names the +00:00 reference zone"
        );
    }

    /// The golden fleet's homes' shared site offset: UTC-8.
    fn golden_fleet_offset_s() -> i32 {
        -8 * 3600
    }

    // -------------------------------------------------------------------
    // B-12: the fleet's labels in local site time.
    // -------------------------------------------------------------------

    /// One UTC-5 home of the single-offset acceptance fleet.
    fn single_offset_config(bldg_id: i64, home: &str, weather: &str) -> DwellingConfig {
        DwellingConfig {
            hpxml: InputSource::Path(resstock_fixture(&format!("{home}/home.xml"))),
            schedule: Some(InputSource::Path(resstock_fixture(&format!(
                "{home}/in.schedules.csv"
            )))),
            weather: WeatherInput::Path(resstock_fixture("weather").join(weather)),
            defaults: DefaultsSource::directory(repo_defaults_path()),
            settings: DwellingSettings::new(
                SimulationConfig {
                    start_time: FixedOffset::west_opt(5 * 3600)
                        .expect("valid offset")
                        .with_ymd_and_hms(2018, 1, 1, 0, 0, 0)
                        .single()
                        .expect("valid start"),
                    duration: Duration::days(2),
                    time_res: Duration::seconds(3600),
                    reports: Vec::new(),
                    diagnostics: false,
                    metrics: false,
                    output_chunk_size: 10_000,
                    setpoint_deadband_c: None,
                    master_seed: 0,
                    civil_timezone: None,
                    site_location: hares_io::SiteLocationOverride::default(),
                    max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
                    health_policy: hares_io::HealthPolicy::default(),
                    first_hour_interpolation: hares_io::FirstHour::default(),
                    weather_year: hares_io::WeatherYear::default(),
                    warmup: hares_io::Warmup::default(),
                },
                None,
                bldg_id,
                None,
            ),
        }
    }

    /// A single-offset fleet's rows are labelled in the homes' local
    /// time: a naive `Timestamp(Millisecond, None)` at each period's
    /// start, the UTC-5 homes' local midnight first, with the schema
    /// metadata recording the offset (B-12).
    #[test]
    fn single_offset_fleet_is_labelled_in_local_time() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let fleet = Fleet::from_buildings(vec![
            single_offset_config(2, "bldg0000002", "G0900090_2018.csv"),
            single_offset_config(17, "bldg0000017", "G0900010_2018.csv"),
            single_offset_config(3, "bldg0000003", "G2500170_2018.csv"),
        ])
        .expect("the homes carry no reserved report or shared destination")
        .with_sample_weights(vec![1.0, 1.0, 1.0])
        .expect("the weights are finite and non-negative");
        let result = fleet
            .simulate(&hourly_named_common(), 1, chunk(64))
            .expect("the fleet runs");
        for home in &result.homes {
            assert!(
                !matches!(home.status, HomeStatus::Failed(_)),
                "every home runs: {:?}",
                home.status
            );
        }
        assert_eq!(result.aggregate.num_rows(), 48, "2 days at 3600 s, hourly");
        let time_field = result.aggregate.schema_ref().field(0).clone();
        assert_eq!(
            time_field.data_type(),
            &DataType::Timestamp(TimeUnit::Millisecond, None),
            "the Time column is a naive millisecond Timestamp, not FL-4's Utf8"
        );
        let labels = time_labels_of(&result.aggregate);
        assert_eq!(
            labels.first().expect("a row"),
            &naive_ms(2018, 1, 1, 0, 0),
            "the first row is the homes' local midnight, naive"
        );
        assert_ne!(
            labels.first().expect("a row"),
            &naive_ms(2018, 1, 1, 5, 0),
            "not FL-4's UTC rendering of the same instant (05:00Z)"
        );
        assert_eq!(
            labels.last().expect("a row"),
            &naive_ms(2018, 1, 2, 23, 0),
            "the last row is the second day's last local hour"
        );
        assert_eq!(
            offset_metadata_of(&result.aggregate),
            Some(-5 * 3600),
            "the schema metadata records the homes' shared offset"
        );
    }

    /// A mixed-offset fleet declared `Instant` with reference zone UTC-6
    /// labels in the reference zone's wall clock: the homes start at
    /// their own local midnights (the manifest's naive start read in
    /// each home's offset), so in reference time the UTC-5 home starts
    /// at 2017-12-31 23:00, the UTC-6 home at 00:00 and the UTC-8 home
    /// at 02:00, and the rows are the periods every folded home has a
    /// row for: the first row is 2018-01-01 02:00, naive (B-12).
    #[test]
    fn instant_fleet_is_labelled_in_the_reference_zone() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let fleet = mixed_offset_fleet([0, 0, 0]).with_alignment(Alignment::Instant(
            FixedOffset::west_opt(6 * 3600).expect("valid offset"),
        ));
        let result = fleet
            .simulate(&hourly_named_common(), 1, chunk(64))
            .expect("the fleet runs");
        for home in &result.homes {
            assert!(
                !matches!(home.status, HomeStatus::Failed(_)),
                "every home runs: {:?}",
                home.status
            );
        }
        let labels = time_labels_of(&result.aggregate);
        assert_eq!(
            labels.first().expect("a row"),
            &naive_ms(2018, 1, 1, 2, 0),
            "the first row every home covers is the UTC-8 home's first reference hour"
        );
        assert_ne!(
            labels.first().expect("a row"),
            &naive_ms(2018, 1, 1, 0, 0),
            "not the homes' local midnight: the label is the reference zone's"
        );
        // The labels continue hourly on the reference clock.
        for (row, label) in labels.iter().enumerate().skip(1).take(4) {
            assert_eq!(
                label,
                &(naive_ms(2018, 1, 1, 2, 0) + i64::try_from(row).expect("a row") * 3_600_000),
                "row {row}: the reference hours continue"
            );
        }
        assert_eq!(
            offset_metadata_of(&result.aggregate),
            Some(-6 * 3600),
            "the schema metadata names the declared reference zone"
        );
    }

    /// A mixed-offset fleet declared `LocalClock` labels every row by
    /// the local wall-clock time each folded home's step in that row
    /// starts at: the homes start at their own local midnights and run
    /// the same resolution, so the labels are the shared wall clock's
    /// hourly sequence, a naive `Timestamp` (B-12), not FL-4's text.
    #[test]
    fn local_clock_fleet_is_labelled_by_local_wall_clock() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let fleet = mixed_offset_fleet([0, 0, 0]).with_alignment(Alignment::LocalClock);
        let result = fleet
            .simulate(&hourly_named_common(), 1, chunk(64))
            .expect("the fleet runs");
        for home in &result.homes {
            assert!(
                !matches!(home.status, HomeStatus::Failed(_)),
                "every home runs: {:?}",
                home.status
            );
        }
        let time_field = result.aggregate.schema_ref().field(0).clone();
        assert_eq!(
            time_field.data_type(),
            &DataType::Timestamp(TimeUnit::Millisecond, None),
            "the Time column is a Timestamp, not FL-4's Utf8 text"
        );
        // Every home starts at its local midnight (the naive start read
        // in the home's own offset) and steps hourly, so the wall clock
        // each home's step in row j starts at is 2018-01-01T00:00 + j h.
        let labels = time_labels_of(&result.aggregate);
        assert_eq!(labels.len(), 168, "7 days at 3600 s, hourly");
        for (row, label) in labels.iter().enumerate() {
            let expected =
                naive_ms(2018, 1, 1, 0, 0) + i64::try_from(row).expect("a row") * 3_600_000;
            assert_eq!(
                label, &expected,
                "row {row}: the label is each home's step's local wall clock"
            );
        }
    }

    /// Two homes whose naive local starts differ, no alignment declared:
    /// the single-offset grid check fails before any home runs.
    #[test]
    fn heterogeneous_time_grid_is_an_error() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let examples = repo_root().join("data/examples");
        let config = |bldg_id: i64, start: DateTime<FixedOffset>| DwellingConfig {
            hpxml: InputSource::Path(fixture_hpxml_path()),
            schedule: Some(InputSource::Path(
                examples.join("BEopt_example_schedule.csv"),
            )),
            weather: WeatherInput::Path(examples.join("USA_CO_Denver.Intl.AP.725650_TMY3.epw")),
            defaults: DefaultsSource::directory(repo_defaults_path()),
            settings: DwellingSettings::new(
                SimulationConfig {
                    start_time: start,
                    duration: Duration::hours(2),
                    time_res: Duration::seconds(900),
                    reports: Vec::new(),
                    diagnostics: false,
                    metrics: false,
                    output_chunk_size: 10_000,
                    setpoint_deadband_c: None,
                    master_seed: 0,
                    civil_timezone: None,
                    site_location: hares_io::SiteLocationOverride::default(),
                    max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
                    health_policy: hares_io::HealthPolicy::default(),
                    first_hour_interpolation: hares_io::FirstHour::default(),
                    weather_year: hares_io::WeatherYear::default(),
                    warmup: hares_io::Warmup::Disabled,
                },
                None,
                bldg_id,
                None,
            ),
        };
        let offset = FixedOffset::west_opt(7 * 3600).expect("valid offset");
        let fleet = Fleet::from_buildings(vec![
            config(
                1,
                offset
                    .with_ymd_and_hms(2021, 1, 1, 0, 0, 0)
                    .single()
                    .expect("valid start"),
            ),
            config(
                2,
                offset
                    .with_ymd_and_hms(2021, 1, 1, 1, 0, 0)
                    .single()
                    .expect("valid start"),
            ),
        ])
        .expect("the homes carry no reserved report or shared destination");
        let fleet = fleet.with_progress(move |_, _| {
            panic!("no home ran: the progress callback must stay silent");
        });
        let err = fleet
            .simulate(&hourly_named_common(), 1, chunk(64))
            .expect_err("a differing naive local start fails the grid check");
        assert!(
            matches!(&err, FleetError::HeterogeneousTimeGrid { bldg_id } if *bldg_id == 2),
            "got: {err:?}"
        );
    }

    /// The many-home construction parses each unique input once: three
    /// homes sharing one HPXML file, one weather file and one defaults
    /// tree produce one weather parse, one defaults load and one profile
    /// load, and one HPXML parse per unique file. The parse counts are the
    /// proof instrument (`hares_io::parse_counts`).
    #[test]
    fn many_home_construction_parses_each_unique_input_once() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let dir = tempdir().expect("temp dir");
        let configs = build_valid_configs(dir.path(), 3);
        let before = hares_io::parse_counts::parse_counts();
        let (fleet, errors) =
            SteppableFleet::from_configs(configs, 0).expect("at least one home builds");
        assert!(errors.is_empty(), "no home errors: {errors:?}");
        drop(fleet);
        let after = hares_io::parse_counts::parse_counts();

        assert_eq!(
            after.weather - before.weather,
            1,
            "three homes share one weather file: one parse, not one per home"
        );
        assert_eq!(
            after.defaults - before.defaults,
            1,
            "three homes share one defaults tree: one load, not one per home"
        );
        assert_eq!(
            after.profiles - before.profiles,
            1,
            "three homes share one profile file: one load, not one per home"
        );
        assert_eq!(
            after.hpxml - before.hpxml,
            1,
            "the three homes share one HPXML file: one parse, not one per home"
        );
    }

    /// The streaming simulate shares the parses the same way: a fleet whose
    /// homes share every input parses each unique input once for the whole
    /// fleet, not once per home.
    #[test]
    fn streaming_simulate_parses_each_unique_input_once() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let dir = tempdir().expect("temp dir");
        let configs = build_valid_configs(dir.path(), 3);
        let before = hares_io::parse_counts::parse_counts();
        let fleet = Fleet::from_buildings(configs)
            .expect("the configs carry no reserved report or shared destination");
        let result = fleet
            .simulate(&test_report(), 0, chunk(3))
            .expect("the fleet runs");
        let after = hares_io::parse_counts::parse_counts();
        for home in &result.homes {
            assert_eq!(
                home.status,
                HomeStatus::Ok,
                "no home errors: {:?}",
                home.status
            );
        }

        assert_eq!(
            after.weather - before.weather,
            1,
            "three homes share one weather file: one parse, not one per home"
        );
        assert_eq!(
            after.defaults - before.defaults,
            1,
            "three homes share one defaults tree: one load, not one per home"
        );
        assert_eq!(
            after.hpxml - before.hpxml,
            1,
            "the three homes share one HPXML file: one parse, not one per home"
        );
    }

    // -------------------------------------------------------------------
    // The fleet's Enum columns fold by Mode in both stages: the
    // acceptance.
    // -------------------------------------------------------------------

    /// One of the `fleet_same_offset_300s` manifest's homes as a config:
    /// the manifest's inputs and sim config (2 days at 300 s from local
    /// midnight 2018-01-01 -08:00, warm-up disabled, the manifest's
    /// metrics request), the manifest's empty override map.
    fn fleet_same_offset_config(bldg_id: i64, weather: &str) -> DwellingConfig {
        let root = repo_root();
        DwellingConfig {
            hpxml: InputSource::Path(
                root.join("tests/fixtures/resstock/2025.1/bldg0176227/home.xml"),
            ),
            schedule: Some(InputSource::Path(
                root.join("tests/fixtures/resstock/2025.1/bldg0176227/in.schedules.csv"),
            )),
            weather: WeatherInput::Path(
                root.join(format!("tests/fixtures/resstock/2025.1/weather/{weather}")),
            ),
            defaults: DefaultsSource::directory(repo_defaults_path()),
            settings: DwellingSettings::new(
                SimulationConfig {
                    start_time: chrono::DateTime::parse_from_rfc3339("2018-01-01T00:00:00-08:00")
                        .expect("the manifest's start"),
                    duration: Duration::seconds(172800),
                    time_res: Duration::seconds(300),
                    reports: Vec::new(),
                    diagnostics: false,
                    metrics: true,
                    output_chunk_size: 10_000,
                    setpoint_deadband_c: None,
                    master_seed: 0,
                    civil_timezone: None,
                    site_location: hares_io::SiteLocationOverride::default(),
                    max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
                    health_policy: hares_io::HealthPolicy::default(),
                    first_hour_interpolation: hares_io::FirstHour::default(),
                    weather_year: hares_io::WeatherYear::default(),
                    warmup: hares_io::Warmup::Disabled,
                },
                Some(serde_json::json!({})),
                bldg_id,
                None,
            ),
        }
    }

    /// The mode probe: an actor declaring a three-label Enum variable and
    /// publishing a scripted code each step. The first 15-minute
    /// period's three 300 s steps report codes 0, 2 and 2, so the
    /// period's declared reduction is 2, the most frequent code, never
    /// the mean (4/3, which is no code).
    struct ModeProbeActor {
        steps: u64,
        telemetry: Telemetry,
    }

    impl ModeProbeActor {
        const NAME: &str = "mode_probe";
        const COLUMN: &str = "actor:mode_probe:mode";

        const OUTPUT_VARIABLES: &'static [TelemetryField] = &[TelemetryField::full(
            "mode",
            Unit::Code,
            VariableKind::Enum {
                labels: EnumLabels::Static(&["a", "b", "c"]),
            },
            Aggregation::Mode,
            Sampling::InstantAtEnd,
            "the mode probe's scripted code",
        )];

        fn code(steps: u64) -> f64 {
            match steps {
                1 => 0.0,
                2 | 3 => 2.0,
                _ => 0.0,
            }
        }
    }

    impl Actor for ModeProbeActor {
        fn name(&self) -> &str {
            Self::NAME
        }

        fn output_variables(&self) -> &[TelemetryField] {
            Self::OUTPUT_VARIABLES
        }

        fn telemetry(&self) -> Option<&Telemetry> {
            Some(&self.telemetry)
        }

        fn decide(&mut self, _env: &EnvironmentState, _out: &mut Vec<DispatchRequest>) {
            self.steps += 1;
            self.telemetry.set("mode", Self::code(self.steps));
        }
    }

    /// The per-home stage: the home's fleet reducer takes Mode over time
    /// for an Enum variable, as its declaration says. The probe's first
    /// period reports codes 0, 2 and 2 over its three steps; the reduced
    /// period is 2, the most frequent code with the tie to the lowest,
    /// never the mean 4/3.
    #[test]
    fn home_mode_over_time_is_the_most_frequent_code() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut config = fleet_same_offset_config(1, "G0600770_2018.csv");
        // Verbosity(0): the actor registers after construction, so the
        // report cannot name the probe's column; the actor columns ride
        // every verbosity tier, and the run resolves the tier after the
        // actor is in the catalogue.
        config
            .settings
            .sim_config
            .reports
            .push(hares_io::ReportSpec {
                name: "fleet".to_string(),
                selection: hares_io::Selection::Verbosity(0),
                frequency: hares_io::Frequency::Timestep,
                destination: hares_io::Destination::Callback,
            });
        let mut dwelling = Dwelling::from_config(config.clone()).expect("the home builds");
        dwelling
            .add_actor(Box::new(ModeProbeActor {
                steps: 0,
                telemetry: Telemetry::from_fields(ModeProbeActor::OUTPUT_VARIABLES),
            }))
            .expect("the probe actor registers");
        let state = Arc::new(Mutex::new(HomeFoldState::new(&HomeFoldAlignment {
            frequency: hares_io::Frequency::Minutes(NonZeroU32::new(15).expect("15 divides 60")),
            step_ms: 300_000,
            calendar: PeriodCalendar::home(),
            key_offset_s: 0,
            instant_aligned: false,
        })));
        dwelling
            .attach_callback("fleet", Box::new(ReducerFeed::new(state.clone())))
            .expect("the callback attaches");

        let engine = SimulationEngine::new();
        engine
            .run_dwelling(&mut dwelling, &config.settings.sim_config)
            .expect("the home runs");

        let (table, fields) = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .finish();
        let slot = fields
            .iter()
            .position(|field| field.name() == ModeProbeActor::COLUMN)
            .expect("the probe column folds");
        let (_, values) = table.iter().next().expect("periods reduced");
        assert_eq!(
            values[slot],
            Some(2.0),
            "the first period's value is the most frequent code of 0, 2, 2, \
             not the mean 4/3"
        );
    }

    /// The code with the greatest summed weight, ties to the lowest code:
    /// the test's own weighted Mode, computed independently of the fold's.
    fn weighted_mode(counts: &[f64]) -> usize {
        let mut best = 0usize;
        let mut best_count = counts.first().copied().unwrap_or(0.0);
        for (code, &count) in counts.iter().enumerate().skip(1) {
            if count > best_count {
                best = code;
                best_count = count;
            }
        }
        best
    }

    /// The across-homes stage: the fleet's Enum columns fold to the code
    /// with the greatest summed weight over the homes' period codes, each
    /// home's code the most frequent code of its own steps in the period
    /// by the declared rules, ties to the lowest code, nulls ignored. The
    /// selection holds equipment Enum columns, and every Enum column's
    /// every period folds to that combination on bits; at the mean the
    /// cells read non-codes instead.
    #[test]
    fn fleet_enum_cells_follow_mode() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let weathers = [
            "G0600770_2018.csv",
            "G0600650_2018.csv",
            "G0600790_2018.csv",
        ];
        let weights = [1.0, 2.5, 0.5];
        let configs: Vec<DwellingConfig> = weathers
            .iter()
            .enumerate()
            .map(|(index, weather)| fleet_same_offset_config(index as i64 + 1, weather))
            .collect();

        // The Enum columns the verbosity-3 selection holds and their
        // label counts, from a home's catalogue: the declared kinds the
        // fold must follow.
        let enum_labels: BTreeMap<String, usize> = {
            let dwelling = Dwelling::from_config(configs[0].clone()).expect("the home builds");
            let names = hares_io::resolve_selection(
                &hares_io::Selection::Verbosity(3),
                &dwelling.catalogue(),
            )
            .expect("the tier resolves");
            names
                .iter()
                .filter_map(|name| match dwelling.catalogue().resolve(name) {
                    Some(def) => match &def.kind {
                        VariableKind::Enum { labels } => Some((def.name.to_string(), labels.len())),
                        _ => None,
                    },
                    None => None,
                })
                .collect()
        };
        assert!(
            !enum_labels.is_empty(),
            "the selection holds at least one Enum variable: the equipment modes"
        );

        // Each home's own Timestep Memory run, reduced to period codes by
        // the declared rules: the source of the expected cells.
        let home_codes: Vec<BTreeMap<String, Vec<Option<f64>>>> = configs
            .iter()
            .map(|config| home_period_codes(config, &enum_labels))
            .collect();

        let fleet = Fleet::from_buildings(configs)
            .expect("the homes carry no reserved report or shared destination")
            .with_sample_weights(weights.to_vec())
            .expect("the weights are finite and non-negative");
        let report = crate::fold::FleetReport {
            selection: hares_io::Selection::Verbosity(3),
            frequency: hares_io::Frequency::Minutes(NonZeroU32::new(15).expect("15 divides 60")),
            health_policy: hares_io::HealthPolicy::default(),
        };
        let result = fleet
            .simulate(&report, 1, chunk(64))
            .expect("the fleet runs");
        assert_eq!(result.aggregate.num_rows(), 192, "48 h at 15 minutes");

        for (name, labels) in &enum_labels {
            let cells = table_float_column(&result.aggregate, name);
            let weight_cells = table_float_column(&result.covered_weight, name);
            assert_eq!(cells.len(), 192, "{name}: one cell per period");
            for period in 0..cells.len() {
                let mut counts = vec![0.0; *labels];
                let mut voted = 0.0;
                for (home, codes) in home_codes.iter().enumerate() {
                    if let Some(code) = codes[name][period] {
                        counts[code as usize] += weights[home];
                        voted += weights[home];
                    }
                }
                let Some(cell) = cells[period] else {
                    assert_eq!(
                        voted, 0.0,
                        "{name} period {period}: a null cell has no vote behind it"
                    );
                    continue;
                };
                let expected = weighted_mode(&counts) as f64;
                assert_eq!(
                    cell.to_bits(),
                    expected.to_bits(),
                    "{name} period {period}: the cell is the weighted Mode of the homes' \
                     period codes, not their mean"
                );
                assert_eq!(
                    weight_cells[period]
                        .unwrap_or_else(|| panic!("{name} period {period}"))
                        .to_bits(),
                    voted.to_bits(),
                    "{name} period {period}: the covered weight is the voting homes' weight"
                );
            }
        }
    }

    /// The count probe: an actor declaring a count quantity summed over
    /// time and publishing a scripted count each step. The first
    /// 15-minute period's three 300 s steps report 1, 2 and 3, so the
    /// period's declared reduction is 6, the sum, never the mean 2.
    struct CountProbeActor {
        steps: u64,
        telemetry: Telemetry,
    }

    impl CountProbeActor {
        const NAME: &str = "count_probe";
        const COLUMN: &str = "actor:count_probe:count";

        const OUTPUT_VARIABLES: &'static [TelemetryField] = &[TelemetryField::step_count(
            "count",
            "the count probe's per-step count",
        )];

        fn count(steps: u64) -> f64 {
            match steps {
                1 => 1.0,
                2 => 2.0,
                3 => 3.0,
                _ => 0.0,
            }
        }
    }

    impl Actor for CountProbeActor {
        fn name(&self) -> &str {
            Self::NAME
        }

        fn output_variables(&self) -> &[TelemetryField] {
            Self::OUTPUT_VARIABLES
        }

        fn telemetry(&self) -> Option<&Telemetry> {
            Some(&self.telemetry)
        }

        fn decide(&mut self, _env: &EnvironmentState, _out: &mut Vec<DispatchRequest>) {
            self.steps += 1;
            self.telemetry.set("count", Self::count(self.steps));
        }
    }

    /// The per-home stage: the home's fleet reducer takes the declared
    /// over-time rule, so a quantity declared `Sum` sums over time. The
    /// probe's first 15-minute period reports counts 1, 2 and 3 over its
    /// three 300 s steps; the reduced period is 6, the declared sum,
    /// never the mean 2.
    #[test]
    fn home_reducer_sums_a_quantity_declared_sum() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut config = fleet_same_offset_config(1, "G0600770_2018.csv");
        // Verbosity(0): the actor registers after construction, so the
        // report cannot name the probe's column; the actor columns ride
        // every verbosity tier, and the run resolves the tier after the
        // actor is in the catalogue.
        config
            .settings
            .sim_config
            .reports
            .push(hares_io::ReportSpec {
                name: "fleet".to_string(),
                selection: hares_io::Selection::Verbosity(0),
                frequency: hares_io::Frequency::Timestep,
                destination: hares_io::Destination::Callback,
            });
        let mut dwelling = Dwelling::from_config(config.clone()).expect("the home builds");
        dwelling
            .add_actor(Box::new(CountProbeActor {
                steps: 0,
                telemetry: Telemetry::from_fields(CountProbeActor::OUTPUT_VARIABLES),
            }))
            .expect("the probe actor registers");
        let state = Arc::new(Mutex::new(HomeFoldState::new(&HomeFoldAlignment {
            frequency: hares_io::Frequency::Minutes(NonZeroU32::new(15).expect("15 divides 60")),
            step_ms: 300_000,
            calendar: PeriodCalendar::home(),
            key_offset_s: 0,
            instant_aligned: false,
        })));
        dwelling
            .attach_callback("fleet", Box::new(ReducerFeed::new(state.clone())))
            .expect("the callback attaches");

        let engine = SimulationEngine::new();
        engine
            .run_dwelling(&mut dwelling, &config.settings.sim_config)
            .expect("the home runs");

        let (table, fields) = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .finish();
        let slot = fields
            .iter()
            .position(|field| field.name() == CountProbeActor::COLUMN)
            .expect("the probe column folds");
        let (_, values) = table.iter().next().expect("periods reduced");
        assert_eq!(
            values[slot],
            Some(6.0),
            "the first period's value is the declared sum 1 + 2 + 3, not the mean 2"
        );
    }

    /// The across-homes rule a variable's declared kind folds by: energy
    /// and power the weighted sum, temperature, ratio and state the
    /// weighted mean, a quantity its declared across-homes rule; `None`
    /// for an enum, whose across-homes fold is the weighted Mode.
    fn declared_across_homes(kind: &VariableKind) -> Option<hares_types::AcrossHomes> {
        match kind {
            VariableKind::Energy | VariableKind::Power => Some(hares_types::AcrossHomes::Sum),
            VariableKind::Temperature | VariableKind::Ratio | VariableKind::State => {
                Some(hares_types::AcrossHomes::Mean)
            }
            VariableKind::Quantity { across_homes } => Some(*across_homes),
            VariableKind::Enum { .. } => None,
        }
    }

    /// The run both declared-kind fold tests measure: the same-offset
    /// golden homes' inputs at 15-minute frequency, the selection naming
    /// every verbosity-8 variable the homes declare plus the three
    /// weather channels no tier preset carries. The weather channels are
    /// where the unit-keyed tables and the declared kinds disagree for
    /// these homes: the outdoor pressure (kPa), the outdoor humidity
    /// ratio (kg/kg, a ratio) and the wind speed (m/s) are intensive
    /// quantities declared to average across homes, which the unit-keyed
    /// fallback sums; the tier-8 columns' declared rules (energy and
    /// power summing, temperature, ratio, state and the dimensionless
    /// quantities meaning, the enums in Mode) agree with the unit-keyed
    /// ones, so the run carries the pinned columns beside the moving
    /// ones.
    fn declared_kinds_selection() -> (Vec<DwellingConfig>, hares_io::Selection) {
        let weathers = [
            "G0600770_2018.csv",
            "G0600650_2018.csv",
            "G0600790_2018.csv",
        ];
        let configs: Vec<DwellingConfig> = weathers
            .iter()
            .enumerate()
            .map(|(index, weather)| fleet_same_offset_config(index as i64 + 1, weather))
            .collect();
        let catalogue = Dwelling::from_config(configs[0].clone())
            .expect("the home builds")
            .catalogue()
            .clone();
        let mut names = hares_io::resolve_selection(&hares_io::Selection::Verbosity(8), &catalogue)
            .expect("the tier resolves");
        for channel in [
            "Outdoor Pressure (kPa)",
            "Outdoor Humidity Ratio (kg/kg)",
            "Wind Speed (m/s)",
        ] {
            assert!(
                catalogue.resolve(channel).is_some(),
                "{channel} is a catalogue variable of the home"
            );
            names.push(channel.to_string());
        }
        (configs, hares_io::Selection::Named(names))
    }

    /// The fleet's cells follow the declared kinds. The test first asserts
    /// that the selection holds at least one ratio and one intensive
    /// quantity (the outdoor humidity ratio, the wind speed and the
    /// outdoor pressure among them). For every aggregate variable that is
    /// not an enum and every period, the cell equals the declared
    /// combination of the three homes' period values, each computed from
    /// that home's own Timestep Memory report by the declared time rule,
    /// added in entry order and compared on bits. The enum variables are
    /// excluded: their across-homes fold is the weighted Mode, whose own
    /// tests carry it.
    #[test]
    fn fleet_cells_follow_declared_kinds() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let weights = [1.0, 2.5, 0.5];
        let (configs, selection) = declared_kinds_selection();

        // The declared kinds the selection holds, from a home's catalogue:
        // the kinds the fold must follow.
        let catalogue = Dwelling::from_config(configs[0].clone())
            .expect("the home builds")
            .catalogue()
            .clone();
        let names = hares_io::resolve_selection(&selection, &catalogue).expect("the names resolve");
        let mut ratios = 0_usize;
        let mut intensive = 0_usize;
        for name in &names {
            match &catalogue.resolve(name).expect("a catalogue variable").kind {
                VariableKind::Ratio => ratios += 1,
                VariableKind::Quantity {
                    across_homes: hares_types::AcrossHomes::Mean,
                } => intensive += 1,
                _ => {}
            }
        }
        assert!(
            ratios >= 1,
            "the selection holds at least one Ratio variable: the outdoor \
             humidity ratio and the runtime fractions"
        );
        assert!(
            intensive >= 1,
            "the selection holds at least one intensive Quantity: the wind \
             speed, the outdoor pressure and the minimum on and off times"
        );

        // Each home's own Timestep Memory run, reduced to period values
        // by the declared rules: the source of the expected cells.
        let home_values: Vec<BTreeMap<String, Vec<Option<f64>>>> = configs
            .iter()
            .map(|config| home_period_values(config, &selection))
            .collect();

        let fleet = Fleet::from_buildings(configs)
            .expect("the homes carry no reserved report or shared destination")
            .with_sample_weights(weights.to_vec())
            .expect("the weights are finite and non-negative");
        let report = crate::fold::FleetReport {
            selection,
            frequency: hares_io::Frequency::Minutes(NonZeroU32::new(15).expect("15 divides 60")),
            health_policy: hares_io::HealthPolicy::default(),
        };
        let result = fleet
            .simulate(&report, 1, chunk(64))
            .expect("the fleet runs");
        assert_eq!(result.aggregate.num_rows(), 192, "48 h at 15 minutes");

        for slot in 1..result.aggregate.num_columns() {
            let name = result.aggregate.schema_ref().field(slot).name().clone();
            let def = catalogue
                .resolve(&name)
                .unwrap_or_else(|| panic!("{name} is a catalogue variable"));
            let Some(across) = declared_across_homes(&def.kind) else {
                continue;
            };
            let cells = table_float_column(&result.aggregate, &name);
            let weight_cells = table_float_column(&result.covered_weight, &name);
            assert_eq!(cells.len(), 192, "{name}: one cell per period");
            for period in 0..cells.len() {
                let mut sum = 0.0;
                let mut weight = 0.0;
                for (home, values) in home_values.iter().enumerate() {
                    if let Some(value) = values[&name][period] {
                        sum += value * weights[home];
                        weight += weights[home];
                    }
                }
                match cells[period] {
                    None => assert_eq!(
                        weight, 0.0,
                        "{name} period {period}: a null cell has no weight behind it"
                    ),
                    Some(cell) => {
                        let expected = match across {
                            hares_types::AcrossHomes::Sum => sum,
                            hares_types::AcrossHomes::Mean => sum / weight,
                        };
                        assert_eq!(
                            cell.to_bits(),
                            expected.to_bits(),
                            "{name} period {period}: the cell is the declared combination of \
                             the homes' period values"
                        );
                    }
                }
                assert_eq!(
                    weight_cells[period]
                        .unwrap_or_else(|| panic!("{name} period {period}"))
                        .to_bits(),
                    weight.to_bits(),
                    "{name} period {period}: the covered weight is the covered homes' weight"
                );
            }
        }
    }

    /// The ratio and state cells are weighted means: in the same run,
    /// every ratio and state cell lies between the smallest and largest
    /// of the homes' period values, which a weighted sum of humidity
    /// ratios or of binary states is not. A period whose cell no home
    /// covers is null.
    #[test]
    fn fleet_ratio_and_state_are_weighted_means() {
        let _parse_count_exclusive = PARSE_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let weights = [1.0, 2.5, 0.5];
        let (configs, selection) = declared_kinds_selection();

        let catalogue = Dwelling::from_config(configs[0].clone())
            .expect("the home builds")
            .catalogue()
            .clone();
        let home_values: Vec<BTreeMap<String, Vec<Option<f64>>>> = configs
            .iter()
            .map(|config| home_period_values(config, &selection))
            .collect();

        let fleet = Fleet::from_buildings(configs)
            .expect("the homes carry no reserved report or shared destination")
            .with_sample_weights(weights.to_vec())
            .expect("the weights are finite and non-negative");
        let report = crate::fold::FleetReport {
            selection,
            frequency: hares_io::Frequency::Minutes(NonZeroU32::new(15).expect("15 divides 60")),
            health_policy: hares_io::HealthPolicy::default(),
        };
        let result = fleet
            .simulate(&report, 1, chunk(64))
            .expect("the fleet runs");

        let mut checked = 0_usize;
        for slot in 1..result.aggregate.num_columns() {
            let name = result.aggregate.schema_ref().field(slot).name().clone();
            let kind = &catalogue
                .resolve(&name)
                .unwrap_or_else(|| panic!("{name} is a catalogue variable"))
                .kind;
            match kind {
                VariableKind::Ratio | VariableKind::State => {}
                _ => continue,
            }
            let cells = table_float_column(&result.aggregate, &name);
            for period in 0..cells.len() {
                let home_period: Vec<f64> = home_values
                    .iter()
                    .filter_map(|values| values[&name][period])
                    .collect();
                let Some(cell) = cells[period] else {
                    assert!(
                        home_period.is_empty(),
                        "{name} period {period}: a covered cell is not null"
                    );
                    continue;
                };
                let smallest = home_period.iter().copied().fold(f64::INFINITY, f64::min);
                let largest = home_period
                    .iter()
                    .copied()
                    .fold(f64::NEG_INFINITY, f64::max);
                assert!(
                    smallest <= cell && cell <= largest,
                    "{name} period {period}: the cell {cell} lies between the homes' period \
                     values [{smallest}, {largest}]"
                );
                checked += 1;
            }
        }
        assert!(
            checked > 0,
            "the run holds at least one ratio or state cell to check"
        );
    }

    /// One home's period values per column: the home run standalone with
    /// a Timestep Memory report of the given selection, the batches
    /// reduced onto the report frequency by the declared rules (the
    /// reporting reducer, fed each column's catalogue declaration), each
    /// column's period value its declared reduction, null where no step
    /// reported.
    fn home_period_values(
        config: &DwellingConfig,
        selection: &hares_io::Selection,
    ) -> BTreeMap<String, Vec<Option<f64>>> {
        use hares_io::output::reduce::{PeriodReducer, TimeRule};

        let mut run_config = config.clone();
        run_config
            .settings
            .sim_config
            .reports
            .push(hares_io::ReportSpec {
                name: "frame".to_string(),
                selection: selection.clone(),
                frequency: hares_io::Frequency::Timestep,
                destination: hares_io::Destination::Memory,
            });
        let mut dwelling = Dwelling::from_config(run_config).expect("the home builds");
        let catalogue = dwelling.catalogue().clone();
        let engine = SimulationEngine::new();
        let sim_config = dwelling.sim_config().clone();
        let result = engine
            .run_dwelling(&mut dwelling, &sim_config)
            .expect("the home runs");
        let batches: Vec<RecordBatch> = result
            .reports
            .iter()
            .flat_map(|report| report.batches().to_vec())
            .collect();
        let schema = batches
            .first()
            .expect("the memory report recorded")
            .schema_ref()
            .clone();
        let rules: Vec<TimeRule> = schema
            .fields()
            .iter()
            .skip(1)
            .map(|field| {
                let def = catalogue
                    .resolve(field.name())
                    .unwrap_or_else(|| panic!("{} is a catalogue variable", field.name()));
                TimeRule::from_variable(def)
            })
            .collect();
        let mut reducer = PeriodReducer::new(
            hares_io::Frequency::Minutes(NonZeroU32::new(15).expect("15 divides 60")),
            PeriodCalendar::home(),
            rules,
        )
        .expect("the reducer builds");
        let mut rows: Vec<(Vec<f64>, Vec<u32>)> = Vec::new();
        for batch in &batches {
            reducer
                .push_batch(batch, &mut |row| {
                    rows.push((row.values.to_vec(), row.valid_steps.to_vec()));
                })
                .expect("the batches reduce");
        }
        reducer.finish(&mut |row| {
            rows.push((row.values.to_vec(), row.valid_steps.to_vec()));
        });

        let mut values: BTreeMap<String, Vec<Option<f64>>> = schema
            .fields()
            .iter()
            .skip(1)
            .map(|field| (field.name().clone(), Vec::new()))
            .collect();
        for (name, slots) in values.iter_mut() {
            let slot = schema
                .fields()
                .iter()
                .position(|field| field.name() == name)
                .expect("the column is in the home's report");
            *slots = rows
                .iter()
                .map(|(row_values, valid)| (valid[slot - 1] > 0).then_some(row_values[slot - 1]))
                .collect();
        }
        values
    }

    /// One home's period codes per Enum column: the period values of the
    /// named columns, each the most frequent code of the home's own steps
    /// in the period by the declared rules, ties to the lowest, null
    /// where no step reported.
    fn home_period_codes(
        config: &DwellingConfig,
        enum_labels: &BTreeMap<String, usize>,
    ) -> BTreeMap<String, Vec<Option<f64>>> {
        let all = home_period_values(config, &hares_io::Selection::Verbosity(3));
        enum_labels
            .keys()
            .map(|name| {
                let values = all
                    .get(name)
                    .unwrap_or_else(|| panic!("{name} is in the home's report"))
                    .clone();
                (name.clone(), values)
            })
            .collect()
    }
}
