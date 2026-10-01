//! The committed golden document: `<name>.golden.json`.
//!
//! Holds the per-product frame digests, the run metrics in full (f64 bit
//! patterns plus decimal), the health record, and the provenance (defaults
//! digest, feature set, git head, dirty flag). Serialization goes through
//! `serde_json::Value`, whose objects are `BTreeMap`s, so every JSON key is
//! sorted and one capture's bytes are stable.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::digest::FrameDigests;
use crate::error::{FrameGoldenError, FrameGoldenResult};

/// One metrics field: the f64 bit pattern in hex beside its decimal value.
/// `None` fields carry nulls on both.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricField {
    pub bits: Option<String>,
    pub value: Option<f64>,
}

/// A metrics value: an f64 field, a plain string (enums), an integer
/// counter, or a nested map (per-end-use tables).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MetricValue {
    Field(MetricField),
    Text(String),
    Int(u64),
    Map(BTreeMap<String, MetricValue>),
}

/// One row of the metrics product: one home, every field of the run
/// metrics.
pub type MetricsRow = BTreeMap<String, MetricValue>;

/// The full committed golden document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoldenDoc {
    pub kind: String,
    pub features: Vec<String>,
    pub defaults_dir: String,
    pub defaults_digest: String,
    pub git_head: String,
    pub git_dirty: Option<bool>,
    /// One row per home: every field of the run metrics as its f64 bit
    /// pattern in hex, with its decimal value beside it.
    pub metrics: Vec<MetricsRow>,
    /// Frame products by name: `frame` for a dwelling; `aggregate`,
    /// `weights` and `homes` for a fleet; `billing` when a tariff was
    /// configured.
    pub products: BTreeMap<String, FrameDigests>,
    /// Dwelling only today: the run's health record as JSON. Fleet
    /// outcomes do not expose health yet, so the field is null there.
    pub health: Option<serde_json::Value>,
}

fn f64_field(value: f64) -> MetricValue {
    MetricValue::Field(MetricField {
        bits: Some(format!("0x{:016x}", value.to_bits())),
        value: Some(value),
    })
}

fn opt_f64_field(value: Option<f64>) -> MetricValue {
    MetricValue::Field(MetricField {
        bits: value.map(|v| format!("0x{:016x}", v.to_bits())),
        value,
    })
}

fn f64_map(values: &BTreeMap<String, f64>) -> MetricValue {
    MetricValue::Map(
        values
            .iter()
            .map(|(key, value)| (key.clone(), f64_field(*value)))
            .collect(),
    )
}

/// Flattens every field of the run metrics into one metrics row.
pub fn flatten_metrics(metrics: &hares_io::output::metrics::SimulationMetrics) -> MetricsRow {
    use hares_io::output::metrics::{Reliability, SimulationCoverage};

    let mut row = MetricsRow::new();
    row.insert(
        "total_energy_kwh.net_energy_kwh".into(),
        f64_field(metrics.total_energy_kwh.net_energy_kwh),
    );
    row.insert(
        "total_energy_kwh.gross_consumption_kwh".into(),
        f64_field(metrics.total_energy_kwh.gross_consumption_kwh),
    );
    row.insert(
        "total_energy_kwh.gross_pv_generation_kwh".into(),
        f64_field(metrics.total_energy_kwh.gross_pv_generation_kwh),
    );
    row.insert(
        "total_energy_kwh.duration_hours".into(),
        f64_field(metrics.total_energy_kwh.duration_hours),
    );
    row.insert(
        "total_energy_kwh.per_end_use".into(),
        f64_map(&metrics.total_energy_kwh.per_end_use),
    );
    row.insert(
        "peak_power_kw.per_end_use".into(),
        f64_map(&metrics.peak_power_kw.per_end_use),
    );
    row.insert(
        "peak_power_kw.rolling.peak_15min_kw".into(),
        f64_field(metrics.peak_power_kw.rolling.peak_15min_kw),
    );
    row.insert(
        "peak_power_kw.rolling.peak_30min_kw".into(),
        f64_field(metrics.peak_power_kw.rolling.peak_30min_kw),
    );
    row.insert(
        "peak_power_kw.rolling.peak_60min_kw".into(),
        f64_field(metrics.peak_power_kw.rolling.peak_60min_kw),
    );
    row.insert("comfort_hours".into(), opt_f64_field(metrics.comfort_hours));
    row.insert(
        "unmet_load_hours".into(),
        opt_f64_field(metrics.unmet_load_hours),
    );
    row.insert(
        "renewable_energy_fraction".into(),
        opt_f64_field(metrics.renewable_energy_fraction),
    );
    row.insert(
        "grid_interaction_metrics.peak_import_kw".into(),
        f64_field(metrics.grid_interaction_metrics.peak_import_kw),
    );
    row.insert(
        "grid_interaction_metrics.peak_export_kw".into(),
        f64_field(metrics.grid_interaction_metrics.peak_export_kw),
    );
    match &metrics.envelope_loads_kwh {
        None => {
            row.insert("envelope_loads_kwh".into(), opt_f64_field(None));
        }
        Some(loads) => {
            let mut map = MetricsRow::new();
            map.insert("window_solar_kwh".into(), f64_field(loads.window_solar_kwh));
            map.insert(
                "window_conduction_kwh".into(),
                f64_field(loads.window_conduction_kwh),
            );
            map.insert(
                "opaque_conduction_kwh".into(),
                f64_field(loads.opaque_conduction_kwh),
            );
            map.insert("interior_lwr_kwh".into(), f64_field(loads.interior_lwr_kwh));
            map.insert("infiltration_kwh".into(), f64_field(loads.infiltration_kwh));
            map.insert("ventilation_kwh".into(), f64_field(loads.ventilation_kwh));
            map.insert("hvac_heating_kwh".into(), f64_field(loads.hvac_heating_kwh));
            map.insert("hvac_cooling_kwh".into(), f64_field(loads.hvac_cooling_kwh));
            map.insert(
                "internal_gains_kwh".into(),
                f64_field(loads.internal_gains_kwh),
            );
            map.insert("duct_loss_kwh".into(), f64_field(loads.duct_loss_kwh));
            map.insert(
                "internal_mass_kwh".into(),
                f64_field(loads.internal_mass_kwh),
            );
            row.insert("envelope_loads_kwh".into(), MetricValue::Map(map));
        }
    }
    row.insert(
        "efficiency.hvac_heating_cop".into(),
        opt_f64_field(metrics.efficiency.hvac_heating_cop),
    );
    row.insert(
        "efficiency.hvac_cooling_cop".into(),
        opt_f64_field(metrics.efficiency.hvac_cooling_cop),
    );
    row.insert(
        "efficiency.water_heater_cop".into(),
        opt_f64_field(metrics.efficiency.water_heater_cop),
    );
    row.insert(
        "efficiency.battery_round_trip_efficiency".into(),
        opt_f64_field(metrics.efficiency.battery_round_trip_efficiency),
    );
    row.insert(
        "rows_with_partial_setpoint_data_fraction".into(),
        opt_f64_field(metrics.rows_with_partial_setpoint_data_fraction),
    );
    row.insert(
        "simulation_duration_hours".into(),
        f64_field(metrics.simulation_duration_hours),
    );
    row.insert(
        "coverage".into(),
        MetricValue::Text(
            match metrics.coverage {
                SimulationCoverage::FullYear => "FullYear",
                SimulationCoverage::LeapYear => "LeapYear",
                SimulationCoverage::PartialYear => "PartialYear",
                SimulationCoverage::MultiYear => "MultiYear",
            }
            .to_string(),
        ),
    );
    row.insert(
        "nan_step_count".into(),
        MetricValue::Int(metrics.nan_step_count),
    );
    row.insert(
        "metrics_reliability".into(),
        MetricValue::Text(
            match metrics.metrics_reliability {
                Reliability::Reliable => "Reliable",
                Reliability::Degraded => "Degraded",
            }
            .to_string(),
        ),
    );
    row
}

/// Walks two metrics rows field by field and reports every field whose
/// values differ, on either side: keys only the actual side carries are
/// differences too, at every nesting level.
pub fn first_metrics_differences(
    expected: &MetricsRow,
    actual: &MetricsRow,
) -> Vec<(String, String, String)> {
    let mut differences = Vec::new();
    diff_maps("", expected, actual, &mut differences);
    differences
}

/// Maps (per-end-use tables) are recursed into, so a difference is
/// reported at the leaf field's path: field-by-field on bits, per ledger.
/// Keys present only on the actual side report at their path as well: the
/// sweep runs at every recursion level, or a fresh run carrying a nested
/// field the golden lacks would compare identical.
fn diff_maps(
    prefix: &str,
    expected: &MetricsRow,
    actual: &MetricsRow,
    differences: &mut Vec<(String, String, String)>,
) {
    for (key, expected_value) in expected {
        let path = join_path(prefix, key);
        match (expected_value, actual.get(key)) {
            (_, None) => differences.push((path, render(expected_value), "absent".to_string())),
            (MetricValue::Map(expected_map), Some(MetricValue::Map(actual_map))) => {
                diff_maps(&path, expected_map, actual_map, differences);
            }
            (_, Some(actual_value)) if actual_value != expected_value => {
                differences.push((path, render(expected_value), render(actual_value)))
            }
            (_, Some(_)) => {}
        }
    }
    for (key, actual_value) in actual {
        if !expected.contains_key(key) {
            differences.push((
                join_path(prefix, key),
                "absent".to_string(),
                render(actual_value),
            ));
        }
    }
}

fn join_path(prefix: &str, key: &str) -> String {
    if prefix.is_empty() {
        key.to_string()
    } else {
        format!("{prefix}.{key}")
    }
}

fn render(value: &MetricValue) -> String {
    match value {
        MetricValue::Field(field) => match (&field.bits, &field.value) {
            (Some(bits), Some(value)) => format!("{bits} ({value:?})"),
            _ => "null".to_string(),
        },
        MetricValue::Text(text) => text.clone(),
        MetricValue::Int(int) => int.to_string(),
        MetricValue::Map(map) => format!("map of {} entries", map.len()),
    }
}

/// `git rev-parse HEAD` and the tree's dirty flag for provenance. When git
/// is unavailable the head reads `unavailable` and the dirty flag is null,
/// both visible in the committed file.
pub fn git_info(root: &Path) -> (String, Option<bool>) {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("rev-parse")
        .arg("HEAD")
        .output();
    let head = match output {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        _ => "unavailable".to_string(),
    };
    let dirty = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("status")
        .arg("--porcelain")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| !out.stdout.is_empty());
    (head, dirty)
}

impl GoldenDoc {
    /// Serializes the document: JSON, sorted keys, pretty, trailing newline.
    pub fn to_bytes(&self) -> FrameGoldenResult<Vec<u8>> {
        let mut bytes = serde_json::to_vec_pretty(self)?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    /// Loads a committed golden document.
    pub fn from_bytes(bytes: &[u8]) -> FrameGoldenResult<Self> {
        Ok(serde_json::from_slice(bytes)?)
    }

    /// Writes the document next to its manifest as `<stem>.golden.json`.
    pub fn write_next_to(&self, manifest_path: &Path) -> FrameGoldenResult<std::path::PathBuf> {
        let stem = manifest_path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .ok_or_else(|| FrameGoldenError::Manifest {
                path: manifest_path.to_path_buf(),
                detail: "manifest path has no file stem".to_string(),
            })?;
        let golden_path = manifest_path
            .parent()
            .unwrap_or(Path::new("."))
            .join(format!("{stem}.golden.json"));
        std::fs::write(&golden_path, self.to_bytes()?)?;
        Ok(golden_path)
    }
}

/// Digests arbitrary bytes, for callers hashing provenance material.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    crate::digest::hex_digest(&hasher.finalize())
}
