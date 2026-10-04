//! The engine adapter: the only module in this crate that calls the
//! simulation engine.
//!
//! A dwelling is built from the manifest, run with `write_output: true`,
//! `retain_batches: true` and an engine output file inside a temp directory
//! that is deleted afterwards (the recorder needs a destination on disk,
//! but nothing of it survives the run). Products are read from the run's
//! retained batches, the run metrics and `Dwelling::health()`. A fleet runs
//! through `Fleet::simulate` on an explicit single-thread pool.
//!
//! When a later engine change moves the result surface, this module is the
//! only place that adapts.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration as StdDuration, Instant};

use arrow::array::{
    BooleanArray, Float64Array, Float64Builder, Int64Array, RecordBatch, StringArray, StringBuilder,
};
use arrow::datatypes::{DataType, Field, Schema};
use hares_core::{Dwelling, DwellingConfig, SimStatus, SimulationEngine};
use hares_fleet::aggregation::{AggregationResolution, aggregate};
use hares_fleet::{DwellingOutcome, Fleet};
use hares_io::OutputFormat;
use tempfile::TempDir;

use crate::defaults;
use crate::error::{FrameGoldenError, FrameGoldenResult};
use crate::golden::{MetricsRow, flatten_metrics};
use crate::manifest::{GoldenManifest, ManifestKind, ManifestResolution, repo_path};

/// Whether a run keeps its frames. `Full` retains every product in memory
/// (the engine's own file output goes to a deleted temp directory);
/// `Discard` runs with `write_output: false, retain_batches: false`.
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
pub struct RunProducts {
    pub kind: ManifestKind,
    pub timing: RunTiming,
    /// Frame products by name. A dwelling produces `frame` (and `billing`
    /// when a tariff was configured); a fleet produces `aggregate`,
    /// `weights` and `homes`.
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

/// Runs the manifest: a single dwelling, or a fleet on a single-thread pool.
pub fn run(req: RunRequest) -> FrameGoldenResult<RunProducts> {
    match req.manifest.kind {
        ManifestKind::Dwelling => run_dwelling(req),
        ManifestKind::Fleet => run_fleet(req),
    }
}

fn prepare_sim_config(req: &RunRequest) -> FrameGoldenResult<hares_io::SimulationConfig> {
    let mut sim = req.manifest.simulation_config()?;
    apply_duration_override(&mut sim, req.duration_override_s)?;
    match req.output {
        RunOutput::Full => {
            sim.write_output = true;
            sim.retain_batches = true;
            sim.output_format = OutputFormat::Parquet;
        }
        RunOutput::Discard => {
            sim.write_output = false;
            sim.retain_batches = false;
            sim.output_path = None;
        }
    }
    Ok(sim)
}

fn run_dwelling(req: RunRequest) -> FrameGoldenResult<RunProducts> {
    let home = req
        .manifest
        .home
        .first()
        .ok_or_else(|| FrameGoldenError::Engine("dwelling manifest has no home".to_string()))?;
    let mut sim = prepare_sim_config(&req)?;
    let defaults_tree = defaults::prepare(req.repo_root, req.manifest)?;
    let defaults_digest = defaults::digest_used(&defaults_tree)?;

    // The engine's own file output needs a destination on disk; it is
    // deleted with the temp directory when the run ends.
    let engine_dir = TempDir::new()?;
    if req.output == RunOutput::Full {
        sim.output_path = Some(engine_dir.path().join("engine_output.parquet"));
    }

    let config = DwellingConfig {
        hpxml_path: repo_path(req.repo_root, &home.hpxml),
        schedule_path: repo_path(req.repo_root, &home.schedule),
        weather_path: repo_path(req.repo_root, &home.weather),
        defaults_path: Some(defaults_tree.dir().to_path_buf()),
        sim_config: sim.clone(),
        overrides: Some(home.overrides.clone()),
        bldg_id: home.bldg_id,
        initialization_duration: initialization_duration(home.initialization_duration_s),
        resample_overrides: None,
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

    if let SimStatus::Failed(message) = &result.status {
        return Err(FrameGoldenError::Engine(format!(
            "dwelling {} failed: {message}",
            home.bldg_id
        )));
    }

    let mut frames = BTreeMap::new();
    match &result.timeseries {
        Some(batches) if !batches.is_empty() => {
            frames.insert(
                "frame".to_string(),
                FrameProducts::from_batches(batches.clone())?,
            );
        }
        // A discarded run keeps nothing by design; a retained run that
        // flushed zero batches is a zero-step simulation, which a golden
        // cannot pin.
        Some(_) | None if req.output == RunOutput::Full => {
            return Err(FrameGoldenError::Engine(format!(
                "dwelling {} retained no output batches: zero-step simulation",
                home.bldg_id
            )));
        }
        _ => {}
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
    let metrics_rows = vec![flatten_metrics(&result.metrics)];

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

    let engine_dir = TempDir::new()?;
    let construct_start = Instant::now();
    let configs: Vec<DwellingConfig> = req
        .manifest
        .home
        .iter()
        .enumerate()
        .map(|(index, home)| {
            let mut sim = base_sim.clone();
            if req.output == RunOutput::Full {
                sim.output_path = Some(
                    engine_dir
                        .path()
                        .join(format!("engine_output_{index}.parquet")),
                );
            }
            DwellingConfig {
                hpxml_path: repo_path(req.repo_root, &home.hpxml),
                schedule_path: repo_path(req.repo_root, &home.schedule),
                weather_path: repo_path(req.repo_root, &home.weather),
                defaults_path: Some(defaults_tree.dir().to_path_buf()),
                sim_config: sim,
                overrides: Some(home.overrides.clone()),
                bldg_id: home.bldg_id,
                initialization_duration: initialization_duration(home.initialization_duration_s),
                resample_overrides: None,
                patches: None,
            }
        })
        .collect();
    // The manifest states each home's weight and the aggregation
    // resolution; both rules are checked when the manifest parses, so the
    // adapter hands them to the fleet untouched.
    let weights: Vec<f64> = req
        .manifest
        .home
        .iter()
        .map(|home| {
            home.weight
                .expect("a fleet home carries a weight: the manifest validator rejects a fleet home without one")
        })
        .collect();
    let fleet = Fleet::from_buildings(configs)
        .with_sample_weights(weights)
        .map_err(|err| FrameGoldenError::Engine(err.to_string()))?;
    let construct = construct_start.elapsed();

    let simulate_start = Instant::now();
    // n_threads = 1 builds an explicit single-thread rayon pool for this
    // call: the capture's thread count is pinned, not ambient.
    let outcomes = fleet.simulate(1);
    let mut completed: Vec<DwellingOutcome> = Vec::with_capacity(outcomes.len());
    for outcome in outcomes {
        match outcome {
            Ok(outcome) => completed.push(outcome),
            Err(err) => return Err(FrameGoldenError::Engine(err.to_string())),
        }
    }
    for (home, outcome) in req.manifest.home.iter().zip(&completed) {
        if let hares_fleet::SimStatus::Failed(message) = &outcome.status {
            return Err(FrameGoldenError::Engine(format!(
                "home {} failed: {message}",
                home.bldg_id
            )));
        }
    }
    let resolution = match req
        .manifest
        .resolution
        .expect("a fleet manifest carries a resolution: the manifest validator rejects a fleet manifest without one")
    {
        ManifestResolution::Hourly => AggregationResolution::Hourly,
        ManifestResolution::FifteenMin => AggregationResolution::FifteenMin,
    };
    // The simulate timing includes the aggregate step: the fleet's
    // products do not exist until aggregation has run.
    let fleet_results = aggregate(&completed, resolution)
        .map_err(|err| FrameGoldenError::Engine(err.to_string()))?;
    let simulate = simulate_start.elapsed();

    let mut frames = BTreeMap::new();
    frames.insert(
        "aggregate".to_string(),
        FrameProducts::from_batches(vec![fleet_results.aggregate_timeseries.clone()])?,
    );
    frames.insert(
        "weights".to_string(),
        weights_frame(req.manifest, &completed)?,
    );
    frames.insert("homes".to_string(), homes_frame(req.manifest, &completed)?);

    let metrics_rows = completed
        .iter()
        .map(|outcome| flatten_metrics(&outcome.result.metrics))
        .collect();

    Ok(RunProducts {
        kind: ManifestKind::Fleet,
        timing: RunTiming {
            construct,
            simulate,
        },
        frames,
        metrics_rows,
        health: None,
        defaults_dir: req.manifest.defaults.clone(),
        defaults_digest,
        #[cfg(feature = "profiling")]
        profiling: None,
    })
}

/// The covered-weight stand-in: one row per home, the weight that home
/// contributes to the aggregate. The per-cell covered-weight batch arrives
/// with the fleet result refactor; its adapter update replaces this frame.
fn weights_frame(
    manifest: &GoldenManifest,
    outcomes: &[DwellingOutcome],
) -> FrameGoldenResult<FrameProducts> {
    let bldg_ids: Int64Array = manifest.home.iter().map(|home| home.bldg_id).collect();
    let weights: Float64Array = outcomes
        .iter()
        .map(|outcome| outcome.sample_weight)
        .collect();
    let schema = Arc::new(Schema::new(vec![
        Field::new("bldg_id", DataType::Int64, false),
        Field::new("sample_weight", DataType::Float64, false),
    ]));
    let batch = RecordBatch::try_new(schema.clone(), vec![Arc::new(bldg_ids), Arc::new(weights)])?;
    Ok(FrameProducts {
        schema,
        batches: vec![batch],
    })
}

/// One row per home: the identity, weight and per-home results behind the
/// aggregate. Status is the fleet's own rendering of the run outcome.
fn homes_frame(
    manifest: &GoldenManifest,
    outcomes: &[DwellingOutcome],
) -> FrameGoldenResult<FrameProducts> {
    let bldg_ids: Int64Array = manifest.home.iter().map(|home| home.bldg_id).collect();
    let total_energy: Float64Array = outcomes
        .iter()
        .map(|outcome| outcome.result.metrics.total_energy_kwh.net_energy_kwh)
        .collect();
    let peak_power: Float64Array = outcomes
        .iter()
        .map(|outcome| {
            outcome
                .result
                .metrics
                .grid_interaction_metrics
                .peak_import_kw
        })
        .collect();
    let sample_weight: Float64Array = outcomes
        .iter()
        .map(|outcome| outcome.sample_weight)
        .collect();
    let status: StringArray = outcomes
        .iter()
        .map(|outcome| format!("{:?}", outcome.status))
        .collect::<Vec<_>>()
        .into();
    let failed: BooleanArray = outcomes
        .iter()
        .map(|outcome| matches!(outcome.status, hares_fleet::SimStatus::Failed(_)))
        .collect::<Vec<bool>>()
        .into();
    let schema = Arc::new(Schema::new(vec![
        Field::new("bldg_id", DataType::Int64, false),
        Field::new("total_energy_kwh", DataType::Float64, false),
        Field::new("peak_power_kw", DataType::Float64, false),
        Field::new("sample_weight", DataType::Float64, false),
        Field::new("status", DataType::Utf8, false),
        Field::new("failed", DataType::Boolean, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(bldg_ids),
            Arc::new(total_energy),
            Arc::new(peak_power),
            Arc::new(sample_weight),
            Arc::new(status),
            Arc::new(failed),
        ],
    )?;
    Ok(FrameProducts {
        schema,
        batches: vec![batch],
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
