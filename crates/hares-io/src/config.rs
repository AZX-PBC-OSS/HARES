//! TOML configuration file loading and validation.
//!
//! [`SimulationConfig`] is the single source of truth for temporal settings,
//! output behaviour, and RNG seeding. It deserialises from TOML with
//! field-level defaults so that minimal config files work correctly.

use std::path::PathBuf;

use chrono::{DateTime, Duration, FixedOffset};
use serde::{Deserialize, Deserializer};
use thiserror::Error;

/// Output file format.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    /// CSV (default, for OCHRE compatibility).
    #[default]
    Csv,
    /// Apache Parquet (recommended for large datasets).
    Parquet,
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
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct SimulationConfig {
    /// Simulation start time (local standard time with timezone offset).
    pub start_time: DateTime<FixedOffset>,
    /// Total simulation length in seconds.
    #[serde(deserialize_with = "deserialize_duration_seconds")]
    pub duration: Duration,
    /// Timestep size in seconds (default: 60 s).
    #[serde(
        default = "default_time_res",
        deserialize_with = "deserialize_duration_seconds"
    )]
    pub time_res: Duration,
    /// Output verbosity level 0-8 (default: 0).
    #[serde(default)]
    pub output_verbosity: u8,
    /// Output file path; `None` means auto-generated in cwd.
    #[serde(default)]
    pub output_path: Option<PathBuf>,
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
}

impl SimulationConfig {
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

fn deserialize_duration_seconds<'de, D>(deserializer: D) -> Result<Duration, D::Error>
where
    D: Deserializer<'de>,
{
    let seconds = i64::deserialize(deserializer)?;
    Ok(Duration::seconds(seconds))
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
    }

    #[test]
    fn full_config_parses() {
        let toml = r#"
start_time = "2024-06-15T12:00:00Z"
duration = 7200
time_res = 300
output_verbosity = 5
output_path = "/tmp/sim_output.csv"
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
}
