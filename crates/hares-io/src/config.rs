//! TOML configuration file loading and validation.
//!
//! [`SimulationConfig`] is the single source of truth for temporal settings,
//! output behaviour, and RNG seeding. It deserialises from TOML with
//! field-level defaults so that minimal config files work correctly.

use std::path::PathBuf;

use chrono::{DateTime, Duration, FixedOffset};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::site_location::SiteLocationOverride;

/// Output file format.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    /// CSV (default, for OCHRE compatibility).
    #[default]
    Csv,
    /// Apache Parquet (recommended for large datasets).
    Parquet,
}

/// File rotation policy for long simulations.
///
/// When set to a value other than [`None`](RotationPolicy::None), output files
/// are split at time interval boundaries. Each rotated file gets its own schema
/// header and a timestamp suffix derived from the base output path.
///
/// # Reference design
/// EnergyPlus's `ReportFreq` enum:
/// `vendors/EnergyPlus/src/EnergyPlus/OutputProcessor.cc:347-355`
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RotationPolicy {
    /// Single output file (default).
    #[default]
    None,
    /// Rotate every hour.
    Hourly,
    /// Rotate every day.
    Daily,
    /// Rotate every month.
    Monthly,
    /// Rotate every year.
    Yearly,
}

/// Errors produced during config deserialization and validation.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("TOML parse error: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("validation error: {0}")]
    Validation(String),
}

/// Simulation configuration controlling temporal loop, output, and RNG.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SimulationConfig {
    /// Simulation start time as **local wall-clock time**.
    ///
    /// HARES uses `.hour()`, `.weekday()`, `.month()` directly from this
    /// timestamp for schedule evaluation and daily profiles. Only the
    /// wall-clock digits matter -- the `FixedOffset` is carried but ignored
    /// unless `civil_timezone` is set for DST-aware reinterpretation.
    ///
    /// Pass the intended local hour: if you mean noon Denver, use `12:00`
    /// (with any offset), not `19:00 UTC`.
    pub start_time: DateTime<FixedOffset>,
    /// Total simulation length in seconds.
    #[serde(with = "duration_seconds")]
    pub duration: Duration,
    /// Timestep size in seconds (default: 60 s).
    #[serde(default = "default_time_res", with = "duration_seconds")]
    pub time_res: Duration,
    /// Output verbosity level 0-8 (default: 0).
    #[serde(default)]
    pub output_verbosity: u8,
    /// Output file path; `None` means auto-generated in cwd.
    #[serde(default)]
    pub output_path: Option<PathBuf>,
    /// Whether timeseries output should be written to disk.
    #[serde(default = "default_write_output")]
    pub write_output: bool,
    /// Output format (CSV or Parquet).
    #[serde(default)]
    pub output_format: OutputFormat,
    /// Rows per Arrow RecordBatch flush (default: 10 000).
    #[serde(default = "default_output_chunk_size")]
    pub output_chunk_size: usize,
    /// Optional global thermostat deadband in Celsius for comfort metrics.
    #[serde(default)]
    pub setpoint_deadband_c: Option<f64>,
    /// Master RNG seed for reproducibility (default: 0).
    #[serde(default)]
    pub master_seed: u64,
    /// Whether to retain flushed Arrow RecordBatches in memory for
    /// post-hoc access (e.g. Python dataframes, metrics computation).
    /// When `false`, peak memory is `O(chunk_size)`; when `true`,
    /// memory is `O(total_rows)`. Default: `false`.
    #[serde(default)]
    pub retain_batches: bool,
    /// IANA timezone for civil-time schedule indexing (e.g. "America/Denver").
    /// When set, occupancy schedules and utility rates are indexed by civil
    /// (wall-clock) time including DST transitions. Weather indexing is unaffected.
    /// Requires the `dst` cargo feature on `hares-core`; without it, setting this
    /// field will produce a runtime error.
    #[serde(default)]
    pub civil_timezone: Option<String>,
    /// Output file rotation policy for long simulations.
    ///
    /// When set to [`RotationPolicy::Hourly`], [`RotationPolicy::Daily`],
    /// [`RotationPolicy::Monthly`], or [`RotationPolicy::Yearly`], output
    /// is split into multiple files at the specified time interval boundary.
    /// Each file gets a timestamp suffix derived from the first timestamp
    /// written to it (e.g. `dwelling_42_2024-01-01.parquet`).
    #[serde(default)]
    pub rotation: RotationPolicy,
    /// Explicit site-location override. Any field set here takes precedence
    /// over both the HPXML `Site` element and the weather file's embedded
    /// metadata during site-location resolution (see
    /// [`crate::site_location`]). Mismatches with the other sources are
    /// warned about but honoured — this is the deliberate escape hatch for
    /// specifying coordinates/timezone explicitly.
    #[serde(default)]
    pub site_location: SiteLocationOverride,
}

impl SimulationConfig {
    /// Time resolution in seconds as `u32`, for consumers that require a
    /// bounded seconds count (metrics calculators). Seconds counts outside
    /// the `u32` range (negative, or beyond `u32::MAX`) fall back to 3600 —
    /// the hourly default. Unreachable in practice for parsed configs:
    /// `from_toml` validation rejects non-positive resolutions and
    /// simulations never span centuries.
    #[must_use]
    pub fn time_res_secs_u32(&self) -> u32 {
        u32::try_from(self.time_res.num_seconds()).unwrap_or(3600)
    }

    /// Parse and validate a `SimulationConfig` from a TOML string.
    pub fn from_toml(s: &str) -> Result<Self, ConfigError> {
        let cfg: SimulationConfig = toml::from_str(s)?;

        let duration_secs = cfg.duration.num_seconds();
        if duration_secs <= 0 {
            return Err(ConfigError::Validation(
                "duration must be greater than zero".into(),
            ));
        }

        let time_res_secs = cfg.time_res.num_seconds();
        if time_res_secs <= 0 {
            return Err(ConfigError::Validation(
                "time_res must be greater than zero".into(),
            ));
        }

        if cfg.output_verbosity > MAX_VERBOSITY {
            return Err(ConfigError::Validation(format!(
                "output_verbosity must be 0-{MAX_VERBOSITY}, got {}",
                cfg.output_verbosity
            )));
        }

        if duration_secs % time_res_secs != 0 {
            return Err(ConfigError::Validation(format!(
                "duration ({duration_secs}s) must be evenly divisible by time_res ({time_res_secs}s)"
            )));
        }

        if let Some(deadband_c) = cfg.setpoint_deadband_c
            && (!deadband_c.is_finite() || deadband_c < 0.0)
        {
            return Err(ConfigError::Validation(format!(
                "setpoint_deadband_c must be finite and >= 0, got {deadband_c}"
            )));
        }

        Ok(cfg)
    }

    /// Number of timesteps in the simulation.
    #[must_use]
    pub fn timestep_count(&self) -> usize {
        let steps = self.duration.num_seconds() / self.time_res.num_seconds();
        usize::try_from(steps).expect("validated positive timestep count")
    }
}

const DEFAULT_TIME_RES_SECS: i64 = 60;
const DEFAULT_CHUNK_SIZE: usize = 10_000;
const MAX_VERBOSITY: u8 = 8;

fn default_time_res() -> Duration {
    Duration::seconds(DEFAULT_TIME_RES_SECS)
}

fn default_output_chunk_size() -> usize {
    DEFAULT_CHUNK_SIZE
}

fn default_write_output() -> bool {
    true
}

mod duration_seconds {
    use chrono::Duration;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_i64(duration.num_seconds())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Duration, D::Error> {
        let seconds = i64::deserialize(deserializer)?;
        Ok(Duration::seconds(seconds))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_config_applies_defaults() {
        let toml = r#"
start_time = "2024-01-01T00:00:00Z"
duration = 3600
"#;
        let cfg = SimulationConfig::from_toml(toml).expect("parse minimal config");
        assert_eq!(cfg.time_res, Duration::seconds(60));
        assert_eq!(cfg.output_verbosity, 0);
        assert_eq!(cfg.output_format, OutputFormat::Csv);
        assert_eq!(cfg.output_chunk_size, 10_000);
        assert_eq!(cfg.setpoint_deadband_c, None);
        assert_eq!(cfg.master_seed, 0);
        assert!(cfg.output_path.is_none());
        assert!(cfg.write_output);
    }

    #[test]
    fn full_config_parses() {
        let toml = r#"
start_time = "2024-06-15T12:00:00Z"
duration = 7200
time_res = 300
output_verbosity = 5
output_path = "/tmp/sim_output.csv"
write_output = false
output_format = "parquet"
output_chunk_size = 5000
setpoint_deadband_c = 1.5
master_seed = 42
"#;
        let cfg = SimulationConfig::from_toml(toml).expect("parse full config");
        assert_eq!(cfg.time_res, Duration::seconds(300));
        assert_eq!(cfg.output_verbosity, 5);
        assert_eq!(cfg.output_format, OutputFormat::Parquet);
        assert_eq!(cfg.output_chunk_size, 5000);
        assert_eq!(cfg.setpoint_deadband_c, Some(1.5));
        assert_eq!(cfg.master_seed, 42);
        assert_eq!(cfg.output_path, Some(PathBuf::from("/tmp/sim_output.csv")));
        assert!(!cfg.write_output);
    }

    #[test]
    fn zero_time_res_rejected() {
        let toml = r#"
start_time = "2024-01-01T00:00:00Z"
duration = 3600
time_res = 0
"#;
        let err = SimulationConfig::from_toml(toml).unwrap_err();
        assert!(
            matches!(err, ConfigError::Validation(ref msg) if msg.contains("time_res")),
            "expected time_res validation error, got: {err}"
        );
    }

    #[test]
    fn verbosity_above_8_rejected() {
        let toml = r#"
start_time = "2024-01-01T00:00:00Z"
duration = 3600
output_verbosity = 9
"#;
        let err = SimulationConfig::from_toml(toml).unwrap_err();
        assert!(
            matches!(err, ConfigError::Validation(ref msg) if msg.contains("output_verbosity")),
            "expected verbosity validation error, got: {err}"
        );
    }

    #[test]
    fn misaligned_duration_rejected() {
        let toml = r#"
start_time = "2024-01-01T00:00:00Z"
duration = 3601
time_res = 60
"#;
        let err = SimulationConfig::from_toml(toml).unwrap_err();
        assert!(
            matches!(err, ConfigError::Validation(ref msg) if msg.contains("divisible")),
            "expected alignment validation error, got: {err}"
        );
    }

    #[test]
    fn timestep_count_correct() {
        let toml = r#"
start_time = "2024-01-01T00:00:00Z"
duration = 86400
time_res = 300
"#;
        let cfg = SimulationConfig::from_toml(toml).expect("parse config");
        assert_eq!(cfg.timestep_count(), 288); // 86400 / 300
    }

    #[test]
    fn zero_duration_rejected() {
        let toml = r#"
start_time = "2024-01-01T00:00:00Z"
duration = 0
"#;
        let err = SimulationConfig::from_toml(toml).unwrap_err();
        assert!(
            matches!(err, ConfigError::Validation(ref msg) if msg.contains("duration")),
            "expected duration validation error, got: {err}"
        );
    }

    #[test]
    fn invalid_toml_returns_parse_error() {
        let err = SimulationConfig::from_toml("not valid { toml").unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)));
    }

    #[test]
    fn verbosity_boundary_values() {
        for v in 0..=8 {
            let toml = format!(
                r#"
start_time = "2024-01-01T00:00:00Z"
duration = 60
output_verbosity = {v}
"#
            );
            assert!(
                SimulationConfig::from_toml(&toml).is_ok(),
                "verbosity {v} should be valid"
            );
        }
    }

    #[test]
    fn negative_deadband_rejected() {
        let toml = r#"
start_time = "2024-01-01T00:00:00Z"
duration = 3600
setpoint_deadband_c = -0.1
"#;
        let err = SimulationConfig::from_toml(toml).unwrap_err();
        assert!(
            matches!(err, ConfigError::Validation(ref msg) if msg.contains("setpoint_deadband_c")),
            "expected deadband validation error, got: {err}"
        );
    }

    #[test]
    fn rotation_defaults_to_none() {
        let toml = r#"
start_time = "2024-01-01T00:00:00Z"
duration = 3600
"#;
        let cfg = SimulationConfig::from_toml(toml).expect("parse config");
        assert_eq!(cfg.rotation, RotationPolicy::None);
    }

    #[test]
    fn rotation_daily_parses() {
        let toml = r#"
start_time = "2024-01-01T00:00:00Z"
duration = 3600
rotation = "daily"
"#;
        let cfg = SimulationConfig::from_toml(toml).expect("parse config");
        assert_eq!(cfg.rotation, RotationPolicy::Daily);
    }

    #[test]
    fn simulation_config_implements_serialize() {
        fn _assert_serialize<T: serde::Serialize>() {}
        _assert_serialize::<SimulationConfig>();
    }

    #[test]
    fn round_trip_minimal_config() {
        let toml = r#"
start_time = "2024-01-01T00:00:00+00:00"
duration = 3600
"#;
        let cfg = SimulationConfig::from_toml(toml).expect("parse minimal config");
        let serialized = toml::to_string(&cfg).expect("serialize");
        let round_tripped =
            SimulationConfig::from_toml(&serialized).expect("re-parse serialized config");
        assert_eq!(
            cfg, round_tripped,
            "minimal config differs after TOML round-trip"
        );
    }

    #[test]
    fn round_trip_full_config() {
        let toml = r#"
start_time = "2024-06-15T12:00:00+00:00"
duration = 7200
time_res = 300
output_verbosity = 5
output_path = "/tmp/sim_output.csv"
write_output = false
output_format = "parquet"
output_chunk_size = 5000
setpoint_deadband_c = 1.5
master_seed = 42
retain_batches = true
civil_timezone = "America/Denver"
rotation = "daily"

[site_location]
latitude_deg = 39.7392
longitude_deg = -104.9903
elevation_m = 1609.0
utc_offset_h = -7.0
"#;
        let cfg = SimulationConfig::from_toml(toml).expect("parse full config");
        let serialized = toml::to_string(&cfg).expect("serialize");
        let round_tripped =
            SimulationConfig::from_toml(&serialized).expect("re-parse serialized config");
        assert_eq!(
            cfg, round_tripped,
            "full config differs after TOML round-trip"
        );
    }

    #[test]
    fn round_trip_output_path() {
        let toml = r#"
start_time = "2024-01-01T00:00:00+00:00"
duration = 3600
output_path = "/some/path/output.csv"
"#;
        let cfg = SimulationConfig::from_toml(toml).expect("parse");
        let serialized = toml::to_string(&cfg).expect("serialize");
        let round_tripped = SimulationConfig::from_toml(&serialized).expect("re-parse");
        assert_eq!(
            cfg.output_path, round_tripped.output_path,
            "OutputPath does not round-trip"
        );
    }

    #[test]
    fn output_format_implements_serialize() {
        fn _assert_serialize<T: serde::Serialize>() {}
        _assert_serialize::<OutputFormat>();
    }

    #[test]
    fn output_format_csv_serializes_to_csv() {
        let serialized = serde_json::to_string(&OutputFormat::Csv).expect("serialize");
        assert_eq!(serialized, "\"csv\"");
    }

    #[test]
    fn output_format_parquet_serializes_to_parquet() {
        let serialized = serde_json::to_string(&OutputFormat::Parquet).expect("serialize");
        assert_eq!(serialized, "\"parquet\"");
    }

    #[test]
    fn output_format_round_trips_through_toml() {
        #[derive(Serialize, Deserialize)]
        struct Wrapper {
            output_format: OutputFormat,
        }
        for original in [OutputFormat::Csv, OutputFormat::Parquet] {
            let wrapper = Wrapper {
                output_format: original,
            };
            let serialized = toml::to_string(&wrapper).expect("serialize");
            let deserialized: Wrapper = toml::from_str(&serialized).expect("deserialize");
            assert_eq!(
                original, deserialized.output_format,
                "OutputFormat does not round-trip through TOML"
            );
        }
    }

    #[test]
    fn round_trip_output_path_none() {
        let toml = r#"
start_time = "2024-01-01T00:00:00+00:00"
duration = 3600
"#;
        let cfg = SimulationConfig::from_toml(toml).expect("parse");
        let serialized = toml::to_string(&cfg).expect("serialize");
        let round_tripped = SimulationConfig::from_toml(&serialized).expect("re-parse");
        assert_eq!(
            cfg.output_path, round_tripped.output_path,
            "None OutputPath does not round-trip"
        );
    }
}
