//! The run's result surface: one `RunResult` per run, built by
//! `Dwelling::finish()`, the single finalisation path `simulate()` and a
//! stepped run share. The caller reads the result it returns: the reports'
//! delivered outputs, the run's metrics, its health totals, its warning
//! record and its provenance.

use std::path::{Path, PathBuf};

use arrow::record_batch::RecordBatch;
use hares_io::Frequency;
use hares_io::output::metrics::{
    EfficiencyMetrics, EnvelopeComponentLoadsKwh, FullSimulationMetrics, GasEnergyMetrics,
    GridInteractionMetrics, PeakPowerKw, Reliability, RollingPeakKw, SimulationCoverage,
    SimulationMetrics, TotalEnergyKwh,
};
use hares_types::{HaresError, Warning, ZoneId};

use crate::dwelling::DwellingSettings;
use crate::health::RunHealth;

/// The metrics of a finished run: the full metric set, gas included
/// (`FullSimulationMetrics` flattened; the calculator's own type stays in
/// `hares-io`; every result surface carries this one).
#[derive(Debug, Clone, PartialEq)]
pub struct RunMetrics {
    pub total_energy_kwh: TotalEnergyKwh,
    pub peak_power_kw: PeakPowerKw,
    pub comfort_hours: Option<f64>,
    pub unmet_load_hours: Option<f64>,
    /// Fraction of the gross load (consumption behind the meter) covered by
    /// PV generation. Can exceed 1.0 when generation exceeds load (net-positive
    /// home); 0.0 when no consumption was recorded. `None` without a PV column.
    pub renewable_energy_fraction: Option<f64>,
    pub grid_interaction_metrics: GridInteractionMetrics,
    /// Envelope component loads. `None` if component gain columns not present.
    pub envelope_loads_kwh: Option<EnvelopeComponentLoadsKwh>,
    /// Per-equipment efficiency metrics. Always present (fields are Option).
    pub efficiency: EfficiencyMetrics,
    /// Fraction of rows that lacked valid setpoint data but still
    /// accumulated envelope/HVAC/battery energy. `None` if setpoints
    /// are not configured.
    pub rows_with_partial_setpoint_data_fraction: Option<f64>,
    /// Actual simulation duration in hours as computed from accumulated rows.
    pub simulation_duration_hours: f64,
    /// Whether the simulation period is a full calendar year.
    pub coverage: SimulationCoverage,
    /// Number of power-value timesteps skipped due to non-finite (NaN/Inf) values.
    pub nan_step_count: u64,
    /// Reliability status: `Degraded` if any power timesteps were skipped.
    pub metrics_reliability: Reliability,
    /// Gas energy metrics. `None` when no gas power column was present.
    pub gas_energy: Option<GasEnergyMetrics>,
}

impl From<FullSimulationMetrics> for RunMetrics {
    fn from(full: FullSimulationMetrics) -> Self {
        let FullSimulationMetrics {
            metrics:
                SimulationMetrics {
                    total_energy_kwh,
                    peak_power_kw,
                    comfort_hours,
                    unmet_load_hours,
                    renewable_energy_fraction,
                    grid_interaction_metrics,
                    envelope_loads_kwh,
                    efficiency,
                    rows_with_partial_setpoint_data_fraction,
                    simulation_duration_hours,
                    coverage,
                    nan_step_count,
                    metrics_reliability,
                },
            gas_energy,
        } = full;
        Self {
            total_energy_kwh,
            peak_power_kw,
            comfort_hours,
            unmet_load_hours,
            renewable_energy_fraction,
            grid_interaction_metrics,
            envelope_loads_kwh,
            efficiency,
            rows_with_partial_setpoint_data_fraction,
            simulation_duration_hours,
            coverage,
            nan_step_count,
            metrics_reliability,
            gas_energy,
        }
    }
}

impl RunMetrics {
    /// The zeroed set: the metrics a run with reports but nothing recorded
    /// reports, matching the pre-result surface's zeroed shape. A computed
    /// result is never these unless the run's numbers are genuinely zero.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            total_energy_kwh: TotalEnergyKwh {
                net_energy_kwh: 0.0,
                gross_consumption_kwh: 0.0,
                gross_pv_generation_kwh: 0.0,
                per_end_use: std::collections::BTreeMap::new(),
                duration_hours: 0.0,
            },
            peak_power_kw: PeakPowerKw {
                per_end_use: std::collections::BTreeMap::new(),
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
            efficiency: EfficiencyMetrics::default(),
            rows_with_partial_setpoint_data_fraction: None,
            simulation_duration_hours: 0.0,
            coverage: SimulationCoverage::PartialYear,
            nan_step_count: 0,
            metrics_reliability: Reliability::Reliable,
            gas_energy: None,
        }
    }

    /// Combined total energy: electric kWh + gas kWh equivalent.
    #[must_use]
    pub fn combined_total_energy_kwh(&self) -> f64 {
        self.total_energy_kwh.net_energy_kwh
            + self
                .gas_energy
                .as_ref()
                .map_or(0.0, |g| g.total_kwh_equivalent)
    }
}

/// One report's delivered output: what the run holds for that report.
#[derive(Debug, Clone)]
pub enum ReportOutput {
    /// The report's batches, retained by the `Memory` destination.
    Memory(Vec<RecordBatch>),
    /// The published file path: the `Parquet` or `Csv` destination's
    /// target, published at `finish()`.
    File(PathBuf),
    /// The report's batches went to the attached batch callback; nothing
    /// is retained.
    Callback,
}

/// One report's result as the run delivered it.
#[derive(Debug, Clone)]
pub struct ReportResult {
    /// The report's name (unique within the run's config).
    pub name: String,
    /// What the report delivered.
    pub output: ReportOutput,
    /// The reporting frequency the report reduced at.
    pub frequency: Frequency,
    /// Rows the report recorded over the run.
    pub rows: usize,
    /// Batches the report built (and delivered) over the run, the final
    /// partial batch included.
    pub batches_delivered: u64,
}

impl ReportResult {
    /// The report's retained batches; `None` for a file or callback report.
    #[must_use]
    pub fn batches(&self) -> &[RecordBatch] {
        match &self.output {
            ReportOutput::Memory(batches) => batches,
            ReportOutput::File(_) | ReportOutput::Callback => &[],
        }
    }

    /// The published file path; `None` for a memory or callback report.
    #[must_use]
    pub fn path(&self) -> Option<&PathBuf> {
        match &self.output {
            ReportOutput::File(path) => Some(path),
            ReportOutput::Memory(_) | ReportOutput::Callback => None,
        }
    }
}

/// The build's compile-time identity: what built the engine this run ran
/// on. Recorded once per build by the crate's build script (the git
/// resolution is file-based; absent git or `.git` leaves the commit and
/// the dirty flag `None`, never a guess) and copied into every result's
/// provenance, so two runs of one build record the same identity.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct BuildInfo {
    /// The crate version the build compiled (`CARGO_PKG_VERSION`).
    pub version: &'static str,
    /// The git commit the build compiled from; `None` when built without
    /// a git work tree.
    pub commit: Option<&'static str>,
    /// Whether the source tree the build compiled had tracked edits;
    /// `None` when git or `.git` is absent.
    pub dirty: Option<bool>,
    /// The build profile (`debug` or `release`).
    pub profile: &'static str,
    /// The features the build enabled, sorted.
    pub features: &'static [&'static str],
    /// The compiler's version line (`rustc -V`).
    pub rustc: &'static str,
}

impl BuildInfo {
    /// The identity of this build, as the build script recorded it.
    #[must_use]
    pub fn get() -> Self {
        include!(concat!(env!("OUT_DIR"), "/build_info.rs"))
    }
}

/// The catalogue's digest as the 32 bytes the hex string
/// `Catalogue::digest()` prints, decoded: the value every result's
/// provenance records beside the build identity.
pub(crate) fn catalogue_digest_bytes(catalogue: &hares_io::Catalogue) -> [u8; 32] {
    fn hex_value(c: u8) -> u8 {
        match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            other => panic!("the catalogue digest is lowercase hex: {other}"),
        }
    }
    let hex = catalogue.digest();
    assert_eq!(
        hex.len(),
        64,
        "the catalogue digest is a 64 char hex string: {hex}"
    );
    let bytes = hex.as_bytes();
    let mut digest = [0u8; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = (hex_value(bytes[index * 2]) << 4) | hex_value(bytes[index * 2 + 1]);
    }
    digest
}

/// One equipment's init-time provenance: the diagnostics equipment
/// preamble's records, one entry per equipment.
#[derive(Debug, Clone, PartialEq)]
pub struct EquipmentProvenance {
    /// The equipment's instance name.
    pub name: String,
    /// The zone the equipment serves.
    pub zone: Option<ZoneId>,
    /// The HPXML location label the zone type came from, when the
    /// equipment declares one.
    pub zone_type: Option<String>,
    /// OCV data provenance for battery and EV equipment; `None` for
    /// equipment that does not use OCV tables.
    pub ocv_source: Option<String>,
    /// The equipment's own init-time provenance lines: scaling choices,
    /// coefficient conventions, and other model-parameter metadata.
    pub notes: Vec<String>,
}

/// The run's provenance: what produced it.
#[derive(Debug, Clone)]
pub struct Provenance {
    /// The build's compile-time identity: the version, commit, dirty
    /// flag, profile, features and rustc of the build that produced the
    /// run.
    pub build: BuildInfo,
    /// The output catalogue's digest, the 32 bytes behind the hex string
    /// `Catalogue::digest()` prints (the same value the report schema's
    /// `hares:catalogue_digest` metadata carries).
    pub catalogue_digest: [u8; 32],
    /// The settings the run ran under: the validated `SimulationConfig`
    /// (the master seed in effect), the building id, the overrides, the
    /// HPXML patches and the warm-up duration. Every random stream the
    /// run drew derives from the seed and the building id
    /// (`derive_dwelling_rng`), so those two carry the seeds.
    pub settings: DwellingSettings,
    /// The site's UTC offset in seconds, from the simulation start's fixed
    /// offset.
    pub site_utc_offset_s: i32,
    /// One entry per equipment, in equipment order.
    pub equipment: Vec<EquipmentProvenance>,
    /// Every input the run read, recorded when it was parsed: the HPXML,
    /// the schedule when it is a file, the weather with the format it was
    /// parsed as, and the digest over the defaults files the load read.
    /// `None` for the synthetic TOML dwellings: that path reads the TOML,
    /// recorded nowhere in the records, and the absence discloses that.
    pub inputs: Option<hares_io::InputRecords>,
    /// The electric tariff attached with `Dwelling::set_tariff`, `None`
    /// when the run attached none.
    pub tariff: Option<TariffRecord>,
    /// Why the run cannot be exported for exact replay, each category
    /// once in the order first seen. Empty for a run a replay can
    /// rebuild from its settings, inputs and tariff.
    pub replay_blockers: Vec<hares_types::ReplayBlocker>,
}

/// The electric tariff a run attached, recorded at
/// `Dwelling::set_tariff`: the tariff (whose serde derives make it
/// exportable), the IANA zone it was attached in, and the step it
/// attached at (0 when attached before the first step). A mid-run
/// attach's billing semantics (the prorated period the attach joins)
/// reconstruct only from that step, so the replay export carries it.
#[derive(Debug, Clone, PartialEq)]
pub struct TariffRecord {
    /// The attached tariff.
    pub tariff: hares_tariff::ElectricTariff,
    /// The IANA name of the zone the tariff was attached in.
    pub zone: String,
    /// The step index the tariff attached at: the clock's current step
    /// when `set_tariff` succeeded, the first step the evaluator folds.
    pub attach_step: u64,
}

/// One simulation run's result: the finalisation every run shares, whether
/// it ran through `simulate()` or step by step to `finish()`.
#[derive(Debug, Clone)]
pub struct RunResult {
    /// One result per configured report, in `reports` order.
    pub reports: Vec<ReportResult>,
    /// The run's metrics; `None` when the run did not request them
    /// (`SimulationConfig.metrics`): without the request no metrics input
    /// plan is built and no accumulator is fed, so there is nothing to
    /// finalize.
    pub metrics: Option<RunMetrics>,
    /// The run's health totals, recorded unconditionally in every build.
    pub health: RunHealth,
    /// The run's warning record: the first `WarningLog::CAPACITY` warnings
    /// of the run.
    pub warnings: Vec<Warning>,
    /// Warnings dropped past the record's capacity.
    pub warnings_dropped: u64,
    /// What produced the run.
    pub provenance: Provenance,
}

impl RunResult {
    /// Writes the run's resolved configuration, inputs, seeds and build
    /// identity into `dir` for exact replay
    /// (`crates/hares-core/src/replay.rs`). Reads only the provenance and
    /// the report outputs; fails without writing anything when the run
    /// took inputs a replay cannot rebuild
    /// (`HaresError::NotReplayable`).
    ///
    /// # Errors
    ///
    /// [`HaresError::NotReplayable`] naming every recorded blocker, or
    /// [`HaresError::Io`] when the directory or a file write fails.
    pub fn export_replay(&self, dir: &Path) -> std::result::Result<(), HaresError> {
        crate::replay::export(self, dir)
    }
}
