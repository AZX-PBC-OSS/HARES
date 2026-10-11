//! The engine adapter: the only module in this crate that calls the
//! simulation engine.
//!
//! A dwelling is built from the manifest and run with the manifest's own
//! reports (a memory destination keeps every batch in memory; a run's
//! files go to a temp directory nothing of which survives). Products are
//! read from the run's retained batches, the run metrics and
//! `Dwelling::health()`. A fleet runs through `Fleet::simulate` (the
//! streaming ordered fold) on the pinned single-thread pool.
//!
//! When a later engine change moves the result surface, this module is the
//! only place that adapts.

use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::{Duration as StdDuration, Instant};

use arrow::array::{Float64Builder, Int64Array, RecordBatch, StringBuilder};
use arrow::datatypes::{DataType, Field, Schema};
use hares_core::{Dwelling, DwellingConfig, SimulationEngine};
use hares_fleet::{Fleet, FleetBuildReport};

use crate::defaults;
use crate::error::{FrameGoldenError, FrameGoldenResult};
use crate::golden::{MetricsRow, flatten_metrics};
use crate::manifest::{GoldenManifest, ManifestKind, repo_path};

/// The retained batches across the run result's memory reports, in
/// `reports` order: the frames a dwelling golden compares.
fn memory_batches(result: &hares_core::RunResult) -> Vec<RecordBatch> {
    result
        .reports
        .iter()
        .flat_map(|report| report.batches().to_vec())
        .collect()
}

/// Whether a run keeps its frames. `Full` runs the manifest's own reports
/// (a memory destination retains every product in memory); `Discard` runs
/// with no reports at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutput {
    Full,
    Discard,
}

/// One frame product held in memory.
#[derive(Debug, Clone)]
pub struct FrameProducts {
    pub schema: Arc<Schema>,
    pub batches: Vec<RecordBatch>,
}

impl FrameProducts {
    pub fn from_batches(batches: Vec<RecordBatch>) -> FrameGoldenResult<Self> {
        let Some(first) = batches.first() else {
            return Err(FrameGoldenError::Digest(
                "frame product has no batches".to_string(),
            ));
        };
        Ok(Self {
            schema: first.schema(),
            batches,
        })
    }
}

/// Wall time of one run, split where the ledger reports it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RunTiming {
    pub construct: StdDuration,
    pub simulate: StdDuration,
}

impl RunTiming {
    pub fn total(&self) -> StdDuration {
        self.construct + self.simulate
    }
}

/// Everything one run of a manifest produces.
#[derive(Clone)]
pub struct RunProducts {
    pub kind: ManifestKind,
    pub timing: RunTiming,
    /// Frame products by name. A dwelling produces `frame` (and `billing`
    /// when a tariff was configured); a fleet produces `aggregate`,
    /// `covered_weight` and `homes`.
    pub frames: BTreeMap<String, FrameProducts>,
    pub metrics_rows: Vec<MetricsRow>,
    /// Dwelling only today: the fleet outcome surface does not expose
    /// per-home health yet.
    pub health: Option<serde_json::Value>,
    /// The defaults directory the run used, repository-relative as written
    /// in the manifest.
    pub defaults_dir: String,
    /// Digest over every file in the defaults tree the run used.
    pub defaults_digest: String,
    /// Profiling summary, present only when built with `-F profiling`.
    #[cfg(feature = "profiling")]
    pub profiling: Option<hares_core::dwelling::DwellingProfilingSummary>,
}

/// One run request: everything the adapter needs besides the manifest.
pub struct RunRequest<'a> {
    pub manifest: &'a GoldenManifest,
    pub repo_root: &'a std::path::Path,
    pub output: RunOutput,
    pub duration_override_s: Option<i64>,
}

fn apply_duration_override(
    sim: &mut hares_io::SimulationConfig,
    override_s: Option<i64>,
) -> FrameGoldenResult<()> {
    let Some(secs) = override_s else {
        return Ok(());
    };
    let time_res = sim.time_res.num_seconds();
    if secs <= 0 || time_res <= 0 || secs % time_res != 0 {
        return Err(FrameGoldenError::SimulationConfig(format!(
            "--duration-s {secs} must be positive and divisible by the manifest's time_res ({time_res}s)"
        )));
    }
    sim.duration = chrono::Duration::seconds(secs);
    Ok(())
}

/// The manifest's warm-up setting as `DwellingConfig` wants it: `0` maps
/// to `None` (no warm-up) and a positive value to `Some`, the same rule
/// the Python binding applies (`crates/hares-python/src/py_dwelling.rs`).
/// The engine runs its converging warm-up for any `Some`, ignoring the
/// value's magnitude.
fn initialization_duration(secs: u64) -> Option<StdDuration> {
    if secs == 0 {
        None
    } else {
        Some(StdDuration::from_secs(secs))
    }
}

/// Runs the manifest: a single dwelling, a fleet of explicit homes, or a
/// fleet built from ResStock metadata.
pub fn run(req: RunRequest) -> FrameGoldenResult<RunProducts> {
    match req.manifest.kind {
        ManifestKind::Dwelling => run_dwelling(req),
        ManifestKind::Fleet => run_fleet(req),
        ManifestKind::ResstockFleet => run_resstock_fleet(req),
    }
}

fn prepare_sim_config(req: &RunRequest) -> FrameGoldenResult<hares_io::SimulationConfig> {
    let mut sim = req.manifest.simulation_config()?;
    apply_duration_override(&mut sim, req.duration_override_s)?;
    if req.output == RunOutput::Discard {
        sim.reports = Vec::new();
    }
    Ok(sim)
}

fn run_dwelling(req: RunRequest) -> FrameGoldenResult<RunProducts> {
    let home = req
        .manifest
        .home
        .first()
        .ok_or_else(|| FrameGoldenError::Engine("dwelling manifest has no home".to_string()))?;
    let sim = prepare_sim_config(&req)?;
    let defaults_tree = defaults::prepare(req.repo_root, req.manifest)?;
    let defaults_digest = defaults::digest_used(&defaults_tree)?;

    let config = DwellingConfig {
        hpxml_path: repo_path(req.repo_root, &home.hpxml),
        schedule_path: Some(repo_path(req.repo_root, &home.schedule)),
        weather_path: repo_path(req.repo_root, &home.weather),
        defaults_path: Some(defaults_tree.dir().to_path_buf()),
        sim_config: sim.clone(),
        overrides: Some(home.overrides.clone()),
        bldg_id: home.bldg_id,
        initialization_duration: initialization_duration(home.initialization_duration_s),
        patches: None,
    };

    let construct_start = Instant::now();
    let mut dwelling = Dwelling::from_config(config)
        .map_err(|err| FrameGoldenError::Engine(format!("dwelling construction failed: {err}")))?;
    let construct = construct_start.elapsed();

    // The manifest's tariff attaches after construction and before the run,
    // so the billing product appears when a tariff is configured.
    if let Some((tariff, tz)) = req.manifest.tariff(req.repo_root)? {
        dwelling
            .set_tariff(tariff, tz)
            .map_err(|err| FrameGoldenError::Engine(format!("set_tariff failed: {err}")))?;
    }

    let simulate_start = Instant::now();
    let result = SimulationEngine::new()
        .run_dwelling(&mut dwelling, &sim)
        .map_err(|err| FrameGoldenError::Engine(format!("simulation failed: {err}")))?;
    let simulate = simulate_start.elapsed();

    // A discarded run keeps nothing by design; a retained run that flushed
    // zero batches is a zero-step simulation, which a golden cannot pin.
    let batches = memory_batches(&result);
    if batches.is_empty() && req.output == RunOutput::Full {
        return Err(FrameGoldenError::Engine(format!(
            "dwelling {} retained no output batches: zero-step simulation",
            home.bldg_id
        )));
    }

    let mut frames = BTreeMap::new();
    if !batches.is_empty() {
        frames.insert("frame".to_string(), FrameProducts::from_batches(batches)?);
    }
    // The billing product appears only when a tariff is configured; the
    // manifest's [tariff] table attaches one through `Dwelling::set_tariff`.
    if !dwelling.billing_summaries().is_empty() {
        frames.insert(
            "billing".to_string(),
            billing_frame(dwelling.billing_summaries())?,
        );
    }

    let health = serde_json::to_value(dwelling.health())?;
    // The metrics product appears exactly when the run requested metrics
    // (the manifests set `metrics = true`); a run without the request
    // records none.
    let metrics_rows = result
        .metrics
        .as_ref()
        .map(|metrics| vec![flatten_metrics(metrics)])
        .unwrap_or_default();

    Ok(RunProducts {
        kind: ManifestKind::Dwelling,
        timing: RunTiming {
            construct,
            simulate,
        },
        frames,
        metrics_rows,
        health: Some(health),
        defaults_dir: req.manifest.defaults.clone(),
        defaults_digest,
        #[cfg(feature = "profiling")]
        profiling: Some(dwelling.profiling_summary()),
    })
}

fn run_fleet(req: RunRequest) -> FrameGoldenResult<RunProducts> {
    let base_sim = prepare_sim_config(&req)?;
    let defaults_tree = defaults::prepare(req.repo_root, req.manifest)?;
    let defaults_digest = defaults::digest_used(&defaults_tree)?;

    let construct_start = Instant::now();
    let configs: Vec<DwellingConfig> = req
        .manifest
        .home
        .iter()
        .map(|home| {
            let sim = base_sim.clone();
            DwellingConfig {
                hpxml_path: repo_path(req.repo_root, &home.hpxml),
                schedule_path: Some(repo_path(req.repo_root, &home.schedule)),
                weather_path: repo_path(req.repo_root, &home.weather),
                defaults_path: Some(defaults_tree.dir().to_path_buf()),
                sim_config: sim,
                overrides: Some(home.overrides.clone()),
                bldg_id: home.bldg_id,
                initialization_duration: initialization_duration(home.initialization_duration_s),
                patches: None,
            }
        })
        .collect();
    // The manifest states each home's weight and the fleet's report
    // request ([fleet]); both rule sets are checked when the manifest
    // parses, so the adapter hands them to the fleet untouched. The
    // declared time alignment, when the manifest states one, rides with
    // it.
    let weights: Vec<f64> = req
        .manifest
        .home
        .iter()
        .map(|home| {
            home.weight
                .expect("a fleet home carries a weight: the manifest validator rejects a fleet home without one")
        })
        .collect();
    // The capture's thread count is pinned, not ambient: one thread, and
    // the fold's chunk barrier at the Python default (64), which for the
    // fixtures' home counts is one chunk: the collect shape.
    let fleet_spec = req.manifest.fleet.as_ref().expect(
        "a fleet manifest carries a [fleet] table: the manifest validator rejects one without one",
    );
    let mut fleet = Fleet::from_buildings(configs)
        .map_err(|err| FrameGoldenError::Engine(err.to_string()))?
        .with_sample_weights(weights)
        .map_err(|err| FrameGoldenError::Engine(err.to_string()))?;
    if let Some(alignment) = fleet_spec.alignment {
        fleet = fleet.with_alignment(alignment.alignment());
    }
    let construct = construct_start.elapsed();
    let fleet_spec = fleet_spec.fleet_report();
    let simulate_start = Instant::now();
    let result = fleet
        .simulate(&fleet_spec, 1, NonZeroUsize::new(64).expect("64"))
        .map_err(|err| FrameGoldenError::Engine(err.to_string()))?;
    fleet_products(
        req.manifest,
        result,
        construct,
        simulate_start.elapsed(),
        defaults_digest,
        None,
    )
}

/// Runs a `resstock_fleet` manifest: the homes come from the metadata
/// through `Fleet::from_resstock`, their weights from the metadata's
/// weight column, and the run's products include the fleet's construction
/// report beside the simulation products.
fn run_resstock_fleet(req: RunRequest) -> FrameGoldenResult<RunProducts> {
    let manifest = req.manifest;
    let spec = manifest
        .fleet
        .as_ref()
        .expect("a resstock fleet manifest carries a [fleet] table: the manifest validator rejects one without one");
    let defaults_tree = defaults::prepare(req.repo_root, manifest)?;
    let defaults_digest = defaults::digest_used(&defaults_tree)?;

    // The manifest's duration (or the run's --duration-s override) is the
    // ResStock constructor's duration argument; the step and the start
    // stay the constructor's own (one minute, the constructor's fixed
    // start rule).
    let seconds: i64 = match req.duration_override_s {
        Some(override_s) => override_s,
        None => i64::try_from(manifest.duration.expect("validated")).map_err(|_| {
            FrameGoldenError::SimulationConfig("duration overflows i64".to_string())
        })?,
    };
    let resstock_step_seconds = 60;
    if seconds <= 0 || seconds % resstock_step_seconds != 0 {
        return Err(FrameGoldenError::SimulationConfig(format!(
            "duration {seconds} must be positive and divisible by the ResStock fleet's step ({resstock_step_seconds}s)"
        )));
    }

    let construct_start = Instant::now();
    let fleet = Fleet::from_resstock(
        &repo_path(
            req.repo_root,
            manifest.metadata.as_deref().expect("validated"),
        ),
        &repo_path(
            req.repo_root,
            manifest.buildings_dir.as_deref().expect("validated"),
        ),
        &repo_path(
            req.repo_root,
            manifest.weather_dir.as_deref().expect("validated"),
        ),
        manifest.version,
        None,
        Some(chrono::Duration::seconds(seconds)),
        Some(defaults_tree.dir()),
    )
    .map_err(|err| FrameGoldenError::Engine(err.to_string()))?;
    let report = *fleet
        .build_report()
        .expect("a fleet built from ResStock metadata carries its construction report");
    let construct = construct_start.elapsed();

    // The capture's thread count is pinned, not ambient: one thread, the
    // fold's chunk barrier at the Python default (64).
    let fleet_spec = spec.fleet_report();
    let mut fleet = fleet;
    if let Some(alignment) = spec.alignment {
        fleet = fleet.with_alignment(alignment.alignment());
    }
    let simulate_start = Instant::now();
    let result = fleet
        .simulate(&fleet_spec, 1, NonZeroUsize::new(64).expect("64"))
        .map_err(|err| FrameGoldenError::Engine(err.to_string()))?;
    fleet_products(
        manifest,
        result,
        construct,
        simulate_start.elapsed(),
        defaults_digest,
        Some(report),
    )
}

/// The shared fleet finish: the failed-home check, the fleet result's
/// products (`aggregate`, `covered_weight`, `homes`, plus `fleet_report`
/// when the run carries a construction report) and the per-home metrics
/// rows. The simulate timing includes the fold: the fleet's products do
/// not exist until it has run.
#[allow(clippy::too_many_arguments)]
fn fleet_products(
    manifest: &GoldenManifest,
    result: hares_fleet::FleetResult,
    construct: StdDuration,
    simulate_elapsed: StdDuration,
    defaults_digest: String,
    report: Option<FleetBuildReport>,
) -> FrameGoldenResult<RunProducts> {
    for home in &result.homes {
        if let hares_fleet::HomeStatus::Failed(message) = &home.status {
            return Err(FrameGoldenError::Engine(format!(
                "home {} failed: {message}",
                home.bldg_id
            )));
        }
    }
    let simulate = simulate_elapsed;

    let mut frames = BTreeMap::new();
    frames.insert(
        "aggregate".to_string(),
        FrameProducts::from_batches(vec![result.aggregate.clone()])?,
    );
    frames.insert(
        "covered_weight".to_string(),
        FrameProducts::from_batches(vec![result.covered_weight.clone()])?,
    );
    frames.insert(
        "homes".to_string(),
        FrameProducts::from_batches(vec![result.homes_table()])?,
    );
    if let Some(report) = report {
        frames.insert("fleet_report".to_string(), fleet_report_frame(report)?);
    }

    let metrics_rows = result
        .homes
        .iter()
        .filter_map(|home| home.metrics.as_ref())
        .map(flatten_metrics)
        .collect();

    Ok(RunProducts {
        kind: manifest.kind,
        timing: RunTiming {
            construct,
            simulate,
        },
        frames,
        metrics_rows,
        health: None,
        defaults_dir: manifest.defaults.clone(),
        defaults_digest,
        #[cfg(feature = "profiling")]
        profiling: None,
    })
}

/// The billing product: one row per billing period summary, timestamps
/// rendered as RFC 3339 seconds.
fn billing_frame(
    summaries: &[hares_tariff::BillingPeriodSummary],
) -> FrameGoldenResult<FrameProducts> {
    let mut period_start = StringBuilder::new();
    let mut period_end = StringBuilder::new();
    let mut energy = Float64Builder::new();
    let mut demand = Float64Builder::new();
    let mut fixed = Float64Builder::new();
    let mut export = Float64Builder::new();
    let mut net = Float64Builder::new();
    let mut peak = Float64Builder::new();
    let mut import = Float64Builder::new();
    let mut export_kwh = Float64Builder::new();

    for summary in summaries {
        period_start.append_value(
            summary
                .period_start
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        );
        period_end.append_value(
            summary
                .period_end
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        );
        energy.append_value(summary.energy_charge_usd);
        demand.append_value(summary.demand_charge_usd);
        fixed.append_value(summary.fixed_charge_usd);
        export.append_value(summary.export_credit_usd);
        net.append_value(summary.net_bill_usd);
        peak.append_value(summary.peak_demand_kw);
        import.append_value(summary.total_import_kwh);
        export_kwh.append_value(summary.total_export_kwh);
    }

    let schema = Arc::new(Schema::new(vec![
        Field::new("period_start", DataType::Utf8, false),
        Field::new("period_end", DataType::Utf8, false),
        Field::new("energy_charge_usd", DataType::Float64, false),
        Field::new("demand_charge_usd", DataType::Float64, false),
        Field::new("fixed_charge_usd", DataType::Float64, false),
        Field::new("export_credit_usd", DataType::Float64, false),
        Field::new("net_bill_usd", DataType::Float64, false),
        Field::new("peak_demand_kw", DataType::Float64, false),
        Field::new("total_import_kwh", DataType::Float64, false),
        Field::new("total_export_kwh", DataType::Float64, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(period_start.finish()),
            Arc::new(period_end.finish()),
            Arc::new(energy.finish()),
            Arc::new(demand.finish()),
            Arc::new(fixed.finish()),
            Arc::new(export.finish()),
            Arc::new(net.finish()),
            Arc::new(peak.finish()),
            Arc::new(import.finish()),
            Arc::new(export_kwh.finish()),
        ],
    )?;
    Ok(FrameProducts {
        schema,
        batches: vec![batch],
    })
}

/// The fleet construction report: one row of counts, the run's statement
/// of what the ResStock build parsed and built and how many homes' weather
/// files resolved.
fn fleet_report_frame(report: FleetBuildReport) -> FrameGoldenResult<FrameProducts> {
    let int = |value: usize| Int64Array::from(vec![value as i64]);
    let buildings_parsed = int(report.buildings_parsed);
    let dwellings_built = int(report.dwellings_built);
    let weather_resolved = int(report.weather_resolved);
    let weather_failed = int(report.weather_failed);
    let schema = Arc::new(Schema::new(vec![
        Field::new("buildings_parsed", DataType::Int64, false),
        Field::new("dwellings_built", DataType::Int64, false),
        Field::new("weather_resolved", DataType::Int64, false),
        Field::new("weather_failed", DataType::Int64, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(buildings_parsed),
            Arc::new(dwellings_built),
            Arc::new(weather_resolved),
            Arc::new(weather_failed),
        ],
    )?;
    Ok(FrameProducts {
        schema,
        batches: vec![batch],
    })
}
