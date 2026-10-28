//! TOML configuration file loading and validation.
//!
//! [`SimulationConfig`] is the single source of truth for temporal settings,
//! output behaviour, and RNG seeding. It deserialises from TOML with
//! field-level defaults so that minimal config files work correctly.

use std::collections::HashSet;
use std::fmt;

use chrono::{DateTime, Duration, FixedOffset};
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use crate::output::report::{ReportSpec, Selection};
use crate::site_location::SiteLocationOverride;
use crate::weather::{FirstHour, WeatherYear};

pub use hares_types::removed_keys::{
    KeyScope, REMOVED_KEYS, RemovedKey, removed_key, removed_key_message,
};

/// Reject the removed keys among a TOML table's top-level keys before the
/// table deserialises. `document` names the surface in the error.
pub fn check_removed_toml_keys(
    table: &toml::Table,
    scope: KeyScope,
    document: &str,
) -> Result<(), ConfigError> {
    for key in table.keys() {
        if let Some(replacement) = removed_key(scope, key) {
            return Err(ConfigError::Validation(format!(
                "configuration key '{key}' was removed from {document}: {replacement}"
            )));
        }
    }
    Ok(())
}

/// Errors produced during config deserialization and validation.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("TOML parse error: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("validation error: {0}")]
    Validation(String),
}

/// The generated-schedule family's per-dwelling behavioral diversity: on
/// (the default), every no-CSV dwelling's generated schedule is
/// diversified under its own scenario-mixed per-dwelling key (the
/// presence timing, the occupancy level, the appliance diversity factors
/// and the water-draw timing and level; the parameter table and its
/// provenance live on `hares_types::diversity`); off restores the shared
/// default-profile columns every no-CSV home used to copy. A schedule
/// CSV is never diversified whatever the setting: the layer is the
/// generated family's alone.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScheduleDiversity {
    /// The diversified generated schedule (the default).
    #[default]
    On,
    /// The pre-diversity behavior: every no-CSV home runs the same
    /// default-profile columns.
    Off,
}

impl ScheduleDiversity {
    /// Whether the diversity layer applies.
    #[must_use]
    pub fn is_on(self) -> bool {
        self == Self::On
    }
}

/// Simulation configuration controlling temporal loop, output, and RNG.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
    /// The run's reports: what each records and where it goes. Every
    /// selected variable is recorded for every timestep; a `Memory`
    /// destination retains the batches in memory, a `Parquet` or `Csv`
    /// destination streams them to a file published at the run's end, and
    /// a run with no reports records nothing.
    #[serde(default)]
    pub reports: Vec<ReportSpec>,
    /// Whether the run requests the diagnostics report: the per-step
    /// diagnostics variables, declared in the catalogue and appended to
    /// the run's schema. Without it a run records no diagnostics variable
    /// and pays nothing for them (default: false).
    #[serde(default)]
    pub diagnostics: bool,
    /// Whether the run requests the run metrics (default: false). Without
    /// the request the run builds no metrics input plan and no accumulator,
    /// feeds nothing per step, and its result carries no metrics. With it,
    /// a fixed metrics input plan freezes from the catalogue, independent
    /// of the reports, and the accumulator is fed per step from the row
    /// plan's producer slots.
    #[serde(default)]
    pub metrics: bool,
    /// Rows per Arrow RecordBatch flush (default: 10 000).
    #[serde(default = "default_output_chunk_size")]
    pub output_chunk_size: usize,
    /// Optional global thermostat deadband in Celsius for comfort metrics.
    #[serde(default)]
    pub setpoint_deadband_c: Option<f64>,
    /// Master RNG seed for reproducibility (default: 0).
    #[serde(default)]
    pub master_seed: u64,
    /// The run's scenario seed (default: none). A declared scenario mixes
    /// its id into every per-dwelling stream key's spare bytes (the
    /// scenario-mixed master key): two scenarios under one master seed
    /// draw disjoint streams, a declared scenario 0 is not the absent
    /// scenario, and the absent scenario keeps the scenario-less
    /// derivation's exact bytes. The mix reaches every surface inside
    /// the per-dwelling key (the dwelling's own stream, the built-in EV
    /// drivers' streams, every event-load interior); the identity-keyed
    /// surfaces (an unspecified-seed noisy source's keystream key, the
    /// registry EV driver's key material) key on identity alone and are
    /// outside the scenario's reach: two scenarios share such a keystream
    /// only when both dwellings address the same building id, different
    /// building ids deriving different keys. The scope the RNG contract
    /// docs name.
    #[serde(default)]
    pub scenario_seed: Option<u64>,
    /// The generated-schedule family's behavioral diversity (default:
    /// on). On, every no-CSV dwelling's generated schedule carries its
    /// own behavioral draws under the dwelling's scenario-mixed key: the
    /// presence timing's shift, the occupancy level's scale, the
    /// appliance end uses' diversity factors and the water-draw timing
    /// and level. Off, every no-CSV home runs the same default-profile
    /// columns. A schedule CSV is never diversified whatever the
    /// setting; the draws' parameter table and its provenance record
    /// (the 301 Addendum C central profiles, the EULP report the
    /// fleet-aggregate acceptance is stated against) live on
    /// `hares_types::diversity` and the schedule-sources doc.
    #[serde(default)]
    pub schedule_diversity: ScheduleDiversity,
    /// IANA timezone for civil-time schedule indexing (e.g. "America/Denver").
    /// When set, occupancy schedules and utility rates are indexed by civil
    /// (wall-clock) time including DST transitions. Weather indexing is unaffected.
    /// Requires the `dst` cargo feature on `hares-core`; without it, setting this
    /// field will produce a runtime error.
    #[serde(default)]
    pub civil_timezone: Option<String>,
    /// Explicit site-location override. Any field set here takes precedence
    /// over both the HPXML `Site` element and the weather file's embedded
    /// metadata during site-location resolution (see
    /// [`crate::site_location`]). Mismatches with the other sources are
    /// warned about but honoured — this is the deliberate escape hatch for
    /// specifying coordinates/timezone explicitly.
    #[serde(default)]
    pub site_location: SiteLocationOverride,
    /// Most consecutive steps one equipment's step may fail before the run
    /// errors (default [`DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES`]).
    ///
    /// A failed step is rolled back: its port contributions are removed, it
    /// is counted in `RunHealth::port_rollbacks` and logged as a warning, and
    /// the step continues without that equipment. A failure that repeats on
    /// the next step comes from the equipment's state or configuration, not
    /// from one step's inputs, and would leave the equipment dead for the
    /// rest of the run, so the step after this many consecutive failures of
    /// one equipment returns an error naming it. `0` tolerates no failure.
    #[serde(default = "default_max_consecutive_step_failures")]
    pub max_consecutive_step_failures: u32,
    /// The run's health policy: which counted health events escalate to a
    /// run failure. The empty policy (the default) changes no run: every
    /// event stays counted and nothing escalates.
    #[serde(default)]
    pub health_policy: HealthPolicy,
    /// Which hour's record the run's first hour interpolates from, when
    /// the weather file is hourly and the step is shorter than an hour:
    /// hour 24 of the run's first day (`hour24`, the default, EnergyPlus's
    /// own) or hour 1 (`hour1`). The option only affects the first hour of
    /// the first day in the entire run period; every later day boundary
    /// reads the previous day's hour 24, and the same interpolation is
    /// used for repeated warm-up days. No effect at an hourly step or over
    /// a sub-hourly file (the records are read directly).
    #[serde(default)]
    pub first_hour_interpolation: FirstHour,
    /// Whether the weather file is a typical (TMY-style) year or an
    /// actual one (`typical`, the default, or `actual`): EnergyPlus's
    /// RunPeriod "Treat Weather as Actual", which defaults to No. The
    /// rule decides which records a step reads: a typical file is
    /// addressed by month, day and hour whatever its years (a run past
    /// Dec 31 wraps to Jan 1 of the file, and a Feb 29 the file has no
    /// records for is not simulated); an actual-year run must lie inside
    /// the file's own dated records.
    #[serde(default)]
    pub weather_year: WeatherYear,
    /// The run's warm-up: disabled, or a converging warm-up governed by the
    /// EnergyPlus warm-up convergence criteria (the default:
    /// `WarmupConfig::default()`, tolerances 0.4 delta C and 0.04, at least
    /// 1 and at most 25 warm-up days). Each warm-up day repeats the run's
    /// first day and the criteria compare every zone's daily temperature
    /// extremes and daily peak HVAC delivery day over day.
    #[serde(default)]
    pub warmup: Warmup,
}

/// Default for [`SimulationConfig::max_consecutive_step_failures`]: one
/// isolated failure is tolerated, so a single step's input excursion does
/// not end a long run, while the second consecutive failure of the same
/// equipment, which the rollback cannot clear, ends it.
pub const DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES: u32 = 1;

/// The config's fields as the input schema publishes them: the serde
/// field names, types, defaults and per-field bounds, derived from a
/// sample instance every field set (so an `Option`'s key never skips) and
/// the metadata rows beside this definition.
impl hares_types::DescribeFields for SimulationConfig {
    fn fields() -> Vec<hares_types::FieldSchema> {
        use hares_types::{FieldMeta, describe, field_schema};
        let sample = SimulationConfig {
            start_time: DateTime::parse_from_rfc3339("2023-01-01T00:00:00Z")
                .expect("a valid sample start time"),
            duration: Duration::hours(48),
            time_res: default_time_res(),
            reports: vec![ReportSpec {
                name: "sample".to_string(),
                selection: crate::output::report::Selection::Verbosity(2),
                frequency: Default::default(),
                destination: crate::output::report::Destination::Memory,
            }],
            diagnostics: false,
            metrics: false,
            output_chunk_size: default_output_chunk_size(),
            setpoint_deadband_c: Some(1.0),
            master_seed: 0,
            scenario_seed: None,
            schedule_diversity: ScheduleDiversity::default(),
            civil_timezone: Some("America/Denver".to_string()),
            site_location: SiteLocationOverride::sample(),
            max_consecutive_step_failures: DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
            health_policy: HealthPolicy::sample(),
            first_hour_interpolation: FirstHour::default(),
            weather_year: WeatherYear::default(),
            warmup: Warmup::default(),
        };
        let mut fields = describe(
            &sample,
            &[
                (
                    "start_time",
                    FieldMeta {
                        description: Some("the simulation's start, local wall-clock time"),
                        ..Default::default()
                    },
                ),
                (
                    "duration",
                    FieldMeta {
                        description: Some("the simulation's length in seconds"),
                        ..Default::default()
                    },
                ),
                (
                    "time_res",
                    FieldMeta {
                        description: Some("the timestep's length in seconds"),
                        ..Default::default()
                    },
                ),
                (
                    "reports",
                    FieldMeta {
                        description: Some("the run's reports: what each records and where it goes"),
                        ..Default::default()
                    },
                ),
                (
                    "diagnostics",
                    FieldMeta {
                        description: Some("whether the run requests the diagnostics report"),
                        ..Default::default()
                    },
                ),
                (
                    "metrics",
                    FieldMeta {
                        description: Some("whether the run requests the run metrics"),
                        ..Default::default()
                    },
                ),
                (
                    "output_chunk_size",
                    FieldMeta {
                        description: Some("rows per Arrow RecordBatch flush"),
                        ..Default::default()
                    },
                ),
                (
                    "setpoint_deadband_c",
                    FieldMeta {
                        description: Some(
                            "the global thermostat deadband in Celsius for comfort metrics",
                        ),
                        ..Default::default()
                    },
                ),
                (
                    "master_seed",
                    FieldMeta {
                        description: Some("the master RNG seed for reproducibility"),
                        ..Default::default()
                    },
                ),
                (
                    "scenario_seed",
                    FieldMeta {
                        description: Some(
                            "the run's scenario seed, mixed into every per-dwelling stream key; \
                             absent keeps the scenario-less derivation",
                        ),
                        ..Default::default()
                    },
                ),
                (
                    "schedule_diversity",
                    FieldMeta {
                        description: Some(
                            "the generated-schedule family's behavioral diversity: \"on\" (the \
                             default) draws every no-CSV home's own presence timing, occupancy \
                             level, appliance factors and water timing under its scenario-mixed \
                             key; \"off\" runs every no-CSV home on the same default-profile \
                             columns; a schedule CSV is never diversified",
                        ),
                        ..Default::default()
                    },
                ),
                (
                    "civil_timezone",
                    FieldMeta {
                        description: Some(
                            "the IANA timezone for civil-time schedule indexing (a dst-feature field)",
                        ),
                        ..Default::default()
                    },
                ),
                (
                    "site_location",
                    FieldMeta {
                        description: Some(
                            "the explicit site-location override; any set field wins over HPXML and the weather header",
                        ),
                        ..Default::default()
                    },
                ),
                (
                    "max_consecutive_step_failures",
                    FieldMeta {
                        description: Some(
                            "the consecutive failed steps one equipment tolerates before the run errors",
                        ),
                        ..Default::default()
                    },
                ),
                (
                    "health_policy",
                    FieldMeta {
                        description: Some("which counted health events escalate to a run failure"),
                        ..Default::default()
                    },
                ),
                (
                    "first_hour_interpolation",
                    FieldMeta {
                        description: Some(
                            "which hour's record the run's first hour interpolates from (hour24 or hour1)",
                        ),
                        ..Default::default()
                    },
                ),
                (
                    "weather_year",
                    FieldMeta {
                        description: Some(
                            "whether the weather file is a typical (TMY-style) year or an actual one (typical or actual)",
                        ),
                        ..Default::default()
                    },
                ),
                (
                    "warmup",
                    FieldMeta {
                        description: Some(
                            "the run's warm-up: \"disabled\", or a [warmup] table of the \
                             EnergyPlus convergence criteria (temperature_tolerance_c, \
                             loads_tolerance, min_days, max_days); the default converges",
                        ),
                        ..Default::default()
                    },
                ),
            ],
        );
        // The value types the serialisation cannot distinguish: an
        // Option<String>'s sample is a string, a duration's a number of
        // seconds, the start's an RFC 3339 timestamp.
        for field in &mut fields {
            match field.name.as_str() {
                "start_time" => field.value_type = hares_types::ValueType::Timestamp,
                "duration" | "time_res" => {
                    field.value_type = hares_types::ValueType::DurationSeconds
                }
                _ => {}
            }
        }
        let _ = field_schema;
        fields
    }
}

/// The run's health policy: which counted health events escalate to a run
/// failure, and which stay counted. The empty policy (the default) keeps
/// every event counted and nothing escalated, so a run built without one
/// behaves exactly as a run without a policy field would; a limit set
/// here fails the run at the first count that crosses it.
///
/// A partial table is legal (`#[serde(default, deny_unknown_fields)]`):
/// every field left unset keeps its side effect off.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HealthPolicy {
    /// The port-rollback run total the run fails at crossing: `None`
    /// keeps rollbacks counted only.
    #[serde(default)]
    pub max_port_rollbacks: Option<u64>,
    /// The rejected-control-signal run total the run fails at crossing:
    /// `None` keeps rejections counted only.
    #[serde(default)]
    pub max_rejected_control_signals: Option<u64>,
    /// The clamped-action run total the run fails at crossing: `None`
    /// keeps clamps counted only.
    #[serde(default)]
    pub max_clamped_actions: Option<u64>,
    /// The curve-index-clamp run total the run fails at crossing: `None`
    /// keeps clamps counted only.
    #[serde(default)]
    pub max_curve_index_clamps: Option<u64>,
    /// Whether a warm-up that ends without converging fails the run.
    #[serde(default)]
    pub require_warmup_convergence: bool,
}

/// The policy's fields as the input schema publishes them.
impl hares_types::DescribeFields for HealthPolicy {
    fn fields() -> Vec<hares_types::FieldSchema> {
        use hares_types::{FieldMeta, describe};
        describe(
            &Self::sample(),
            &[
                (
                    "max_port_rollbacks",
                    FieldMeta {
                        description: Some("the port-rollback run total the run fails at crossing"),
                        ..Default::default()
                    },
                ),
                (
                    "max_rejected_control_signals",
                    FieldMeta {
                        description: Some(
                            "the rejected-control-signal run total the run fails at crossing",
                        ),
                        ..Default::default()
                    },
                ),
                (
                    "max_clamped_actions",
                    FieldMeta {
                        description: Some("the clamped-action run total the run fails at crossing"),
                        ..Default::default()
                    },
                ),
                (
                    "max_curve_index_clamps",
                    FieldMeta {
                        description: Some(
                            "the curve-index-clamp run total the run fails at crossing",
                        ),
                        ..Default::default()
                    },
                ),
                (
                    "require_warmup_convergence",
                    FieldMeta {
                        description: Some(
                            "whether a warm-up that ends without converging fails the run",
                        ),
                        ..Default::default()
                    },
                ),
            ],
        )
    }
}

impl HealthPolicy {
    /// A sample instance every field set: the `DescribeFields` derive's
    /// serialisation input.
    #[must_use]
    pub fn sample() -> Self {
        Self {
            max_port_rollbacks: Some(10),
            max_rejected_control_signals: Some(10),
            max_clamped_actions: Some(10),
            max_curve_index_clamps: Some(10),
            require_warmup_convergence: false,
        }
    }

    /// The policy that rejects an evaluation on its first counted event:
    /// every limit `Some(0)` and warm-up convergence required.
    #[must_use]
    pub fn strict() -> Self {
        Self {
            max_port_rollbacks: Some(0),
            max_rejected_control_signals: Some(0),
            max_clamped_actions: Some(0),
            max_curve_index_clamps: Some(0),
            require_warmup_convergence: true,
        }
    }

    /// The counter limit the policy sets for `event`, if any. The warm-up
    /// non-convergence is not a counter: its requirement is
    /// [`Self::require_warmup_convergence`], so this returns `None` for it.
    #[must_use]
    pub fn limit_for(&self, event: hares_types::HealthEvent) -> Option<u64> {
        match event {
            hares_types::HealthEvent::PortRollback => self.max_port_rollbacks,
            hares_types::HealthEvent::RejectedControlSignal => self.max_rejected_control_signals,
            hares_types::HealthEvent::ClampedAction => self.max_clamped_actions,
            hares_types::HealthEvent::CurveIndexClamp => self.max_curve_index_clamps,
            hares_types::HealthEvent::WarmupNotConverged => None,
        }
    }
}

fn default_max_consecutive_step_failures() -> u32 {
    DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES
}

/// The default warm-up temperature tolerance [delta C]: EnergyPlus's
/// `Temperature Convergence Tolerance Value` default
/// (`idd/Energy+.idd.in:543-548`, Building field N3, `\units deltaC`,
/// `\default .4`, allowed `(0, 0.5]`).
pub const DEFAULT_TEMPERATURE_TOLERANCE_C: f64 = 0.4;

/// The default warm-up loads tolerance (a fraction, not watts): EnergyPlus's
/// `Loads Convergence Tolerance Value` default
/// (`idd/Energy+.idd.in:536-542`, Building field N2, `\default .04`,
/// allowed `(0, 0.5]`).
pub const DEFAULT_LOADS_TOLERANCE: f64 = 0.04;

/// The default minimum warm-up day count: EnergyPlus's
/// `Minimum Number of Warmup Days` default
/// (`idd/Energy+.idd.in:569-575`, Building field N5, `\default 1`).
pub const DEFAULT_MIN_WARMUP_DAYS: u32 = 1;

/// The default maximum warm-up day count: EnergyPlus's
/// `Maximum Number of Warmup Days` default
/// (`idd/Energy+.idd.in:562-568`, Building field N4, `\default 25`; the IDD
/// note says the value "should NOT be set less than 25").
pub const DEFAULT_MAX_WARMUP_DAYS: u32 = 25;

/// The run's warm-up: disabled, or a converging warm-up governed by the
/// EnergyPlus warm-up convergence criteria
/// (`vendors/EnergyPlus/src/EnergyPlus/HeatBalanceManager.cc:3309-3430`,
/// `CheckWarmupConvergence`). Each warm-up day repeats the run's first day;
/// after each day every zone's four day-over-day residuals are compared:
/// the daily maximum and the daily minimum zone air temperature against
/// `temperature_tolerance_c`, and the daily peak HVAC heating and cooling
/// delivered to the zone against `loads_tolerance` with EnergyPlus's 100 W
/// floor (`:3320`) and 1e-4 W pass-through (`:3350`, `:3366`). Warm-up ends
/// when every zone's checks pass on a day at or after `min_days`, or at
/// `max_days` days, whichever comes first.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Warmup {
    /// No warm-up: the run starts from the construction's initial state and
    /// the run health's warm-up outcome stays `Disabled`.
    Disabled,
    /// Warm up by the EnergyPlus criteria with the given configuration.
    Converge(WarmupConfig),
}

impl Default for Warmup {
    fn default() -> Self {
        Self::Converge(WarmupConfig::default())
    }
}

impl Warmup {
    /// The warm-up configuration when the warm-up converges; `None` when
    /// disabled.
    #[must_use]
    pub fn config(&self) -> Option<&WarmupConfig> {
        match self {
            Self::Disabled => None,
            Self::Converge(config) => Some(config),
        }
    }
}

impl Serialize for Warmup {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Disabled => serializer.serialize_str("disabled"),
            Self::Converge(config) => config.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for Warmup {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct WarmupVisitor;

        impl<'de> Visitor<'de> for WarmupVisitor {
            type Value = Warmup;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("\"disabled\" or a [warmup] table of the convergence criteria")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                match value {
                    "disabled" => Ok(Warmup::Disabled),
                    other => Err(E::custom(format!(
                        "unknown warmup spelling '{other}': the accepted forms are the string \
                         \"disabled\" or a table of the WarmupConfig fields \
                         (temperature_tolerance_c, loads_tolerance, min_days, max_days)"
                    ))),
                }
            }

            fn visit_map<A: MapAccess<'de>>(self, access: A) -> Result<Self::Value, A::Error> {
                let config =
                    WarmupConfig::deserialize(de::value::MapAccessDeserializer::new(access))?;
                Ok(Warmup::Converge(config))
            }
        }

        deserializer.deserialize_any(WarmupVisitor)
    }
}

/// The EnergyPlus warm-up convergence criteria: the tolerances and the
/// minimum and maximum warm-up day counts. A partial `[warmup]` table is
/// legal; every field left unset keeps its EnergyPlus default.
///
/// EnergyPlus silently resets the maximum day count when the minimum is
/// greater (`HeatBalanceManager.cc:686-697`); HARES does not substitute
/// values: `validate()` is an error instead.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WarmupConfig {
    /// The day-over-day temperature tolerance [delta C] on the daily maximum
    /// and the daily minimum zone air temperature, every zone. Default
    /// [`DEFAULT_TEMPERATURE_TOLERANCE_C`]; allowed `(0, 0.5]`
    /// (`idd/Energy+.idd.in:543-548`).
    pub temperature_tolerance_c: f64,
    /// The day-over-day loads tolerance (a fraction) on the daily peak HVAC
    /// heating and cooling delivered to the zone, every zone, with
    /// EnergyPlus's 100 W floor on both days' peaks when today's peak exceeds
    /// 1e-4 W (a smaller peak passes). Default [`DEFAULT_LOADS_TOLERANCE`];
    /// allowed `(0, 0.5]` (`idd/Energy+.idd.in:536-542`).
    pub loads_tolerance: f64,
    /// The least warm-up day count: a converged check before this day keeps
    /// running. Default [`DEFAULT_MIN_WARMUP_DAYS`] (`idd/Energy+.idd.in:569-575`).
    pub min_days: u32,
    /// The greatest warm-up day count: warm-up stops here whether or not
    /// every zone converged, and the run continues with the outcome recorded.
    /// Default [`DEFAULT_MAX_WARMUP_DAYS`] (`idd/Energy+.idd.in:562-568`).
    pub max_days: u32,
}

impl Default for WarmupConfig {
    fn default() -> Self {
        Self {
            temperature_tolerance_c: DEFAULT_TEMPERATURE_TOLERANCE_C,
            loads_tolerance: DEFAULT_LOADS_TOLERANCE,
            min_days: DEFAULT_MIN_WARMUP_DAYS,
            max_days: DEFAULT_MAX_WARMUP_DAYS,
        }
    }
}

impl WarmupConfig {
    /// Every check on the warm-up configuration, run by
    /// [`SimulationConfig::validate`] so the criteria are validated on every
    /// surface that validates a config.
    ///
    /// # Errors
    ///
    /// A tolerance outside `(0, 0.5]` (EnergyPlus's IDD bounds for both
    /// fields), a non-finite tolerance, a day count of 0, or a minimum
    /// greater than the maximum.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let tolerance_in_bounds = |name: &str, value: f64| {
            if value.is_finite() && value > 0.0 && value <= 0.5 {
                Ok(())
            } else {
                Err(ConfigError::Validation(format!(
                    "warmup {name} must be in (0, 0.5], got {value}"
                )))
            }
        };
        tolerance_in_bounds("temperature_tolerance_c", self.temperature_tolerance_c)?;
        tolerance_in_bounds("loads_tolerance", self.loads_tolerance)?;
        if self.min_days == 0 || self.max_days == 0 {
            return Err(ConfigError::Validation(format!(
                "warmup day counts must be at least 1, got min_days {}, max_days {}",
                self.min_days, self.max_days
            )));
        }
        if self.min_days > self.max_days {
            return Err(ConfigError::Validation(format!(
                "warmup min_days ({}) must not exceed max_days ({}): EnergyPlus resets the \
                 maximum to the minimum; HARES asks for the day counts it should run",
                self.min_days, self.max_days
            )));
        }
        Ok(())
    }
}

impl SimulationConfig {
    /// Time resolution in seconds as `u32`, for consumers that require a
    /// bounded seconds count (metrics calculators).
    ///
    /// # Errors
    ///
    /// A seconds count outside the `u32` range (negative, or beyond
    /// `u32::MAX`) is an error naming the resolution: falling back to the
    /// 3600 s default would silently rescale every derived metric.
    pub fn time_res_secs_u32(&self) -> Result<u32, ConfigError> {
        u32::try_from(self.time_res.num_seconds()).map_err(|_| {
            ConfigError::Validation(format!(
                "time_res {} s is outside the u32 seconds range the metrics \
                 calculators require",
                self.time_res.num_seconds()
            ))
        })
    }

    /// Parse and validate a `SimulationConfig` from a TOML string.
    pub fn from_toml(s: &str) -> Result<Self, ConfigError> {
        let table: toml::Table = toml::from_str(s)?;
        for key in table.keys() {
            if let Some(message) = removed_key_message(KeyScope::SimulationConfig, key) {
                return Err(ConfigError::Validation(message));
            }
        }
        let cfg: SimulationConfig = toml::from_str(s)?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Serialise the config to a TOML string: [`Self::from_toml`]'s inverse,
    /// so a config built through any surface round-trips the same way.
    pub fn to_toml(&self) -> Result<String, ConfigError> {
        toml::to_string(self)
            .map_err(|err| ConfigError::Validation(format!("serialization error: {err}")))
    }

    /// Every check on the config's fields, run by every construction path:
    /// `from_toml`, the dwelling constructors and the Python binding's
    /// constructor all call this, so a config is validated the same way no
    /// matter which surface built it.
    ///
    /// # Errors
    ///
    /// A duration or resolution that is not positive, a resolution the
    /// duration is not a multiple of, a resolution outside the `u32` seconds
    /// range the metrics calculators require, an out-of-range verbosity or
    /// deadband, or an out-of-range site-location field.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let duration_secs = self.duration.num_seconds();
        if duration_secs <= 0 {
            return Err(ConfigError::Validation(
                "duration must be greater than zero".into(),
            ));
        }

        let time_res_secs = self.time_res.num_seconds();
        if time_res_secs <= 0 {
            return Err(ConfigError::Validation(
                "time_res must be greater than zero".into(),
            ));
        }

        // The report requests: unique names, unique file paths, and a
        // verbosity selection inside the tier range.
        let mut report_names = HashSet::with_capacity(self.reports.len());
        let mut report_paths = HashSet::with_capacity(self.reports.len());
        for report in &self.reports {
            if !report_names.insert(report.name.as_str()) {
                return Err(ConfigError::Validation(format!(
                    "report name '{}' is not unique: report names must be unique \
                     within SimulationConfig.reports",
                    report.name
                )));
            }
            if let Some(path) = report.destination.path()
                && !report_paths.insert(path.clone())
            {
                return Err(ConfigError::Validation(format!(
                    "report '{}' destination path {} is not unique: two reports \
                     may not name the same file path",
                    report.name,
                    path.display()
                )));
            }
            if let Selection::Verbosity(verbosity) = &report.selection
                && *verbosity > MAX_VERBOSITY
            {
                return Err(ConfigError::Validation(format!(
                    "report '{}' selection verbosity must be 0-{MAX_VERBOSITY}, got {verbosity}",
                    report.name
                )));
            }
        }

        if duration_secs % time_res_secs != 0 {
            return Err(ConfigError::Validation(format!(
                "duration ({duration_secs}s) must be evenly divisible by time_res ({time_res_secs}s)"
            )));
        }

        // A step shorter than an hour must divide the hour: EnergyPlus
        // allows only timesteps per hour evenly divisible into 60
        // (`idd/Energy+.idd.in:925-936`, 1 to 60 including 1, 2, 3, 4, 5,
        // 6, 10, 12, 15, 20, 30, 60), so every sub-hourly step length
        // below divides 3600 s.
        if time_res_secs < 3600 && 3600 % time_res_secs != 0 {
            return Err(ConfigError::Validation(format!(
                "time_res ({time_res_secs}s) must divide the hour (3600 s): EnergyPlus allows \
                 only timesteps per hour evenly divisible into 60"
            )));
        }

        if let Some(deadband_c) = self.setpoint_deadband_c
            && (!deadband_c.is_finite() || deadband_c < 0.0)
        {
            return Err(ConfigError::Validation(format!(
                "setpoint_deadband_c must be finite and >= 0, got {deadband_c}"
            )));
        }

        self.time_res_secs_u32()?;

        self.site_location.validate()?;

        self.warmup
            .config()
            .map_or(Ok(()), WarmupConfig::validate)?;

        Ok(())
    }

    /// Number of timesteps in the simulation.
    #[must_use]
    pub fn timestep_count(&self) -> usize {
        let steps = self.duration.num_seconds() / self.time_res.num_seconds();
        usize::try_from(steps).expect("validated positive timestep count")
    }
}

/// The default `time_res` in seconds, the one default every surface
/// references: the serde default and the Python constructor's signature.
pub const DEFAULT_TIME_RES_SECS: i64 = 60;

/// The default rows-per-batch chunk size, shared by the serde default and
/// the Python constructor's signature.
pub const DEFAULT_OUTPUT_CHUNK_SIZE: usize = 10_000;

/// The highest `output_verbosity` a report's `Selection::Verbosity`
/// accepts. Public because every surface that builds a verbosity selection
/// (the fleet's report builder) applies the same bound.
pub const MAX_VERBOSITY: u8 = 8;

fn default_time_res() -> Duration {
    Duration::seconds(DEFAULT_TIME_RES_SECS)
}

fn default_output_chunk_size() -> usize {
    DEFAULT_OUTPUT_CHUNK_SIZE
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
    use std::path::PathBuf;

    #[test]
    fn removed_key_is_scoped() {
        // The removed key names the Python adapter that holds the weather
        // now, and is removed from the dwelling table only: the same key in
        // another scope stays valid (it was never a key there).
        let replacement = removed_key(KeyScope::DwellingTable, "resample_overrides")
            .expect("the dwelling table's resample_overrides is a removed key");
        assert!(
            replacement.contains("hares.adapters.ochre.hold_weather"),
            "the replacement must point at the Python adapter: {replacement}"
        );
        assert!(
            removed_key(KeyScope::SimulationConfig, "resample_overrides").is_none(),
            "the key is removed from the dwelling table, not the simulation config"
        );
    }

    #[test]
    fn removed_key_rejects_the_dwelling_table_only() {
        // The dwelling-table surface rejects the key before deserialising;
        // a SimulationConfig table never carried it, so that scope stays
        // silent (the scoped rule, not a blanket ban).
        let table: toml::Table =
            toml::from_str("resample_overrides = { dry_bulb = \"zoh\" }").expect("parse table");
        let err = check_removed_toml_keys(&table, KeyScope::DwellingTable, "the dwelling table")
            .expect_err("the removed key is an error");
        let message = err.to_string();
        assert!(
            message.contains("resample_overrides")
                && message.contains("hares.adapters.ochre.hold_weather"),
            "the error names the key and the adapter: {message}"
        );
        let sim_table: toml::Table = toml::from_str("duration = 3600").expect("parse table");
        check_removed_toml_keys(
            &sim_table,
            KeyScope::SimulationConfig,
            "the simulation config",
        )
        .expect("no removed keys in the simulation config scope");
    }

    #[test]
    fn minimal_config_applies_defaults() {
        let toml = r#"
start_time = "2024-01-01T00:00:00Z"
duration = 3600
"#;
        let cfg = SimulationConfig::from_toml(toml).expect("parse minimal config");
        assert_eq!(cfg.time_res, Duration::seconds(60));
        assert_eq!(cfg.output_chunk_size, 10_000);
        assert_eq!(cfg.setpoint_deadband_c, None);
        assert_eq!(cfg.master_seed, 0);
        assert!(
            cfg.reports.is_empty(),
            "a config with no reports records nothing"
        );
        assert_eq!(
            cfg.max_consecutive_step_failures,
            DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES
        );
    }

    #[test]
    fn step_failure_budget_is_configurable() {
        let toml = r#"
start_time = "2024-01-01T00:00:00Z"
duration = 3600
max_consecutive_step_failures = 0
"#;
        let cfg = SimulationConfig::from_toml(toml).expect("parse config");
        assert_eq!(cfg.max_consecutive_step_failures, 0);
    }

    #[test]
    fn full_config_parses() {
        let toml = r#"
start_time = "2024-06-15T12:00:00Z"
duration = 7200
time_res = 300
output_chunk_size = 5000
setpoint_deadband_c = 1.5
master_seed = 42

[[reports]]
name = "frame"
selection = { verbosity = 5 }
destination = "memory"

[[reports]]
name = "archive"
selection = { named = ["Total Electric Power (kW)"] }
destination = { parquet = { path = "/tmp/sim_output.parquet", overwrite = true } }
"#;
        let cfg = SimulationConfig::from_toml(toml).expect("parse full config");
        assert_eq!(cfg.time_res, Duration::seconds(300));
        assert_eq!(cfg.output_chunk_size, 5000);
        assert_eq!(cfg.setpoint_deadband_c, Some(1.5));
        assert_eq!(cfg.master_seed, 42);
        assert_eq!(cfg.reports.len(), 2);
        assert_eq!(cfg.reports[0].name, "frame");
        assert_eq!(
            cfg.reports[0].selection,
            Selection::Verbosity(5),
            "the verbosity selection parses"
        );
        assert_eq!(
            cfg.reports[0].destination,
            crate::output::report::Destination::Memory
        );
        assert_eq!(
            cfg.reports[1].destination,
            crate::output::report::Destination::Parquet {
                path: PathBuf::from("/tmp/sim_output.parquet"),
                overwrite: true,
            },
            "the parquet destination parses with its overwrite flag"
        );
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
    fn verbosity_report_above_8_rejected() {
        let toml = r#"
start_time = "2024-01-01T00:00:00Z"
duration = 3600

[[reports]]
name = "frame"
selection = { verbosity = 9 }
destination = "memory"
"#;
        let err = SimulationConfig::from_toml(toml).unwrap_err();
        assert!(
            matches!(err, ConfigError::Validation(ref msg) if msg.contains("verbosity")),
            "expected verbosity validation error, got: {err}"
        );
    }

    #[test]
    fn duplicate_report_name_is_a_validate_error() {
        let toml = r#"
start_time = "2024-01-01T00:00:00Z"
duration = 3600

[[reports]]
name = "frame"
selection = { verbosity = 2 }
destination = "memory"

[[reports]]
name = "frame"
selection = { verbosity = 3 }
destination = "memory"
"#;
        let err = SimulationConfig::from_toml(toml).unwrap_err();
        assert!(
            matches!(err, ConfigError::Validation(ref msg) if msg.contains("'frame'")),
            "expected duplicate-name validation error naming the report, got: {err}"
        );
    }

    #[test]
    fn duplicate_report_path_is_a_validate_error() {
        let toml = r#"
start_time = "2024-01-01T00:00:00Z"
duration = 3600

[[reports]]
name = "frame"
selection = { verbosity = 2 }
destination = { parquet = { path = "/tmp/sim_output.parquet" } }

[[reports]]
name = "copy"
selection = { verbosity = 2 }
destination = { csv = { path = "/tmp/sim_output.parquet" } }
"#;
        let err = SimulationConfig::from_toml(toml).unwrap_err();
        assert!(
            matches!(err, ConfigError::Validation(ref msg) if msg.contains("/tmp/sim_output.parquet")),
            "expected duplicate-path validation error naming the path, got: {err}"
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
    fn verbosity_report_boundary_values() {
        for v in 0..=8 {
            let toml = format!(
                r#"
start_time = "2024-01-01T00:00:00Z"
duration = 60

[[reports]]
name = "frame"
selection = {{ verbosity = {v} }}
destination = "memory"
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
output_chunk_size = 5000
setpoint_deadband_c = 1.5
master_seed = 42
civil_timezone = "America/Denver"

[[reports]]
name = "frame"
selection = { verbosity = 5 }
destination = "memory"

[[reports]]
name = "archive"
selection = { pattern = "Total *" }
destination = { parquet = { path = "/tmp/sim_output.parquet", overwrite = true } }

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
    fn simulation_config_rejects_unknown_key() {
        let toml = r#"
start_time = "2024-01-01T00:00:00+00:00"
duration = 3600
output_verboisty = 3
"#;
        let err = SimulationConfig::from_toml(toml)
            .expect_err("an unknown key is a typed error, not a silently ignored one");
        assert!(
            err.to_string().contains("output_verboisty"),
            "the error must name the key, got: {err}"
        );
    }

    #[test]
    fn health_policy_defaults_to_empty() {
        let toml = r#"
start_time = "2024-01-01T00:00:00+00:00"
duration = 3600
"#;
        let cfg = SimulationConfig::from_toml(toml).expect("parse minimal config");
        assert_eq!(
            cfg.health_policy,
            HealthPolicy::default(),
            "a config without a health policy carries the empty one"
        );
        let serialized = cfg.to_toml().expect("serialize");
        let round_tripped =
            SimulationConfig::from_toml(&serialized).expect("re-parse serialized config");
        assert_eq!(
            round_tripped.health_policy,
            HealthPolicy::default(),
            "the empty policy round-trips as the empty one"
        );
    }

    #[test]
    fn health_policy_toml_round_trip() {
        let toml = r#"
start_time = "2024-01-01T00:00:00+00:00"
duration = 3600

[health_policy]
max_port_rollbacks = 2
max_clamped_actions = 4
require_warmup_convergence = true
"#;
        let cfg = SimulationConfig::from_toml(toml).expect("parse config with a policy");
        assert_eq!(cfg.health_policy.max_port_rollbacks, Some(2));
        assert_eq!(cfg.health_policy.max_rejected_control_signals, None);
        assert_eq!(cfg.health_policy.max_clamped_actions, Some(4));
        assert_eq!(cfg.health_policy.max_curve_index_clamps, None);
        assert!(cfg.health_policy.require_warmup_convergence);
        let serialized = cfg.to_toml().expect("serialize config");
        let round_tripped =
            SimulationConfig::from_toml(&serialized).expect("re-parse serialized config");
        assert_eq!(cfg, round_tripped, "the policy round-trips through TOML");
    }

    #[test]
    fn health_policy_partial_table_is_legal() {
        let toml = r#"
start_time = "2024-01-01T00:00:00+00:00"
duration = 3600

[health_policy]
max_port_rollbacks = 0
"#;
        let cfg = SimulationConfig::from_toml(toml)
            .expect("a partial policy table parses, the other fields defaulting");
        assert_eq!(cfg.health_policy.max_port_rollbacks, Some(0));
        assert_eq!(cfg.health_policy.max_rejected_control_signals, None);
        assert_eq!(cfg.health_policy.max_clamped_actions, None);
        assert_eq!(cfg.health_policy.max_curve_index_clamps, None);
        assert!(!cfg.health_policy.require_warmup_convergence);
    }

    #[test]
    fn health_policy_unknown_key_is_an_error() {
        let toml = r#"
start_time = "2024-01-01T00:00:00+00:00"
duration = 3600

[health_policy]
max_evictions = 0
"#;
        let err = SimulationConfig::from_toml(toml)
            .expect_err("an unknown policy key is a typed error, not a silently ignored one");
        assert!(
            err.to_string().contains("max_evictions"),
            "the error must name the key, got: {err}"
        );
    }

    #[test]
    fn health_policy_strict_sets_every_limit_to_zero_and_requires_convergence() {
        let policy = HealthPolicy::strict();
        assert_eq!(policy.max_port_rollbacks, Some(0));
        assert_eq!(policy.max_rejected_control_signals, Some(0));
        assert_eq!(policy.max_clamped_actions, Some(0));
        assert_eq!(policy.max_curve_index_clamps, Some(0));
        assert!(policy.require_warmup_convergence);
        // The strict policy's limits read off the events they gate.
        assert_eq!(
            policy.limit_for(hares_types::HealthEvent::PortRollback),
            Some(0)
        );
        assert_eq!(
            policy.limit_for(hares_types::HealthEvent::WarmupNotConverged),
            None,
            "the warm-up requirement is its own field, not a counter limit"
        );
        assert_eq!(
            HealthPolicy::default().limit_for(hares_types::HealthEvent::ClampedAction),
            None,
            "the empty policy sets no limit: nothing escalates"
        );
    }

    #[test]
    fn time_res_beyond_u32_seconds_is_an_error() {
        let cfg = SimulationConfig {
            scenario_seed: None,
            schedule_diversity: Default::default(),
            start_time: DateTime::<FixedOffset>::parse_from_rfc3339("2024-01-01T00:00:00Z")
                .expect("start time parses"),
            duration: Duration::seconds(i64::from(u32::MAX) * 2),
            time_res: Duration::seconds(i64::from(u32::MAX) + 1),
            reports: Vec::new(),
            diagnostics: false,
            metrics: false,
            output_chunk_size: 10_000,
            setpoint_deadband_c: None,
            master_seed: 0,
            civil_timezone: None,
            site_location: SiteLocationOverride::default(),
            max_consecutive_step_failures: DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
            health_policy: crate::HealthPolicy::default(),
            first_hour_interpolation: FirstHour::default(),
            weather_year: WeatherYear::default(),
            warmup: crate::Warmup::default(),
        };
        let err = cfg
            .time_res_secs_u32()
            .expect_err("a resolution beyond u32 seconds is an error, not a 3600 s fallback");
        assert!(
            err.to_string().contains("time_res"),
            "the error must name the resolution, got: {err}"
        );
    }

    // Row 38: the report checks run on every construction path, so the
    // TOML path and a Rust-built config fail the same duplicate with the
    // message the Python constructor raises.
    #[test]
    fn built_config_validates_like_the_toml_path() {
        let report = |name: &str, path: Option<&str>| ReportSpec {
            name: name.to_string(),
            selection: Selection::Verbosity(2),
            frequency: crate::output::report::Frequency::default(),
            destination: path.map_or(crate::output::report::Destination::Memory, |p| {
                crate::output::report::Destination::Csv {
                    path: PathBuf::from(p),
                    overwrite: false,
                }
            }),
        };
        let toml = r#"
start_time = "2024-01-01T00:00:00Z"
duration = 3600

[[reports]]
name = "frame"
selection = { verbosity = 2 }
destination = { csv = { path = "/tmp/sim_output.csv" } }

[[reports]]
name = "copy"
selection = { verbosity = 2 }
destination = { csv = { path = "/tmp/sim_output.csv" } }
"#;
        let toml_err = SimulationConfig::from_toml(toml)
            .expect_err("a duplicate report path is a validate error on the TOML path too");
        let built = SimulationConfig {
            scenario_seed: None,
            schedule_diversity: Default::default(),
            start_time: DateTime::<FixedOffset>::parse_from_rfc3339("2024-01-01T00:00:00Z")
                .expect("start time parses"),
            duration: Duration::seconds(3600),
            time_res: Duration::seconds(60),
            reports: vec![
                report("frame", Some("/tmp/sim_output.csv")),
                report("copy", Some("/tmp/sim_output.csv")),
            ],
            diagnostics: false,
            metrics: false,
            output_chunk_size: 10_000,
            setpoint_deadband_c: None,
            master_seed: 0,
            civil_timezone: None,
            site_location: SiteLocationOverride::default(),
            max_consecutive_step_failures: DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
            health_policy: crate::HealthPolicy::default(),
            first_hour_interpolation: FirstHour::default(),
            weather_year: WeatherYear::default(),
            warmup: crate::Warmup::default(),
        };
        let built_err = built
            .validate()
            .expect_err("the same config built in Rust fails validate()");
        assert_eq!(
            built_err.to_string(),
            toml_err.to_string(),
            "one report check, one message, on every path"
        );
    }

    // Row 34: the site-location check runs on every construction path, so
    // the TOML path and a Rust-built config fail the same out-of-range
    // coordinate with the message the Python constructor raises.
    #[test]
    fn toml_site_location_out_of_range_is_a_validate_error() {
        let toml = r#"
start_time = "2024-01-01T00:00:00Z"
duration = 3600

[site_location]
latitude_deg = 95.0
"#;
        let toml_err = SimulationConfig::from_toml(toml)
            .expect_err("an out-of-range site coordinate is a validate error on the TOML path too");
        let built = SimulationConfig {
            scenario_seed: None,
            schedule_diversity: Default::default(),
            start_time: DateTime::<FixedOffset>::parse_from_rfc3339("2024-01-01T00:00:00Z")
                .expect("start time parses"),
            duration: Duration::seconds(3600),
            time_res: Duration::seconds(60),
            reports: Vec::new(),
            diagnostics: false,
            metrics: false,
            output_chunk_size: 10_000,
            setpoint_deadband_c: None,
            master_seed: 0,
            civil_timezone: None,
            site_location: SiteLocationOverride {
                latitude_deg: Some(95.0),
                ..SiteLocationOverride::default()
            },
            max_consecutive_step_failures: DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
            health_policy: crate::HealthPolicy::default(),
            first_hour_interpolation: FirstHour::default(),
            weather_year: WeatherYear::default(),
            warmup: crate::Warmup::default(),
        };
        let built_err = built
            .validate()
            .expect_err("the same config built in Rust fails validate()");
        assert_eq!(
            built_err.to_string(),
            toml_err.to_string(),
            "one site-location check, one message, on every path"
        );
        assert!(
            built_err.to_string().contains("latitude")
                && built_err.to_string().contains("[-90, 90]"),
            "the message names the field and its range, got: {built_err}"
        );
    }

    #[test]
    fn site_location_validation_covers_every_field_and_boundary() {
        let valid = [
            (Some(90.0), Some(180.0), Some(9000.0), Some(14.0)),
            (Some(-90.0), Some(-180.0), Some(-500.0), Some(-14.0)),
            (None, None, None, None),
        ];
        for (lat, lon, elev, off) in valid {
            let site = SiteLocationOverride {
                latitude_deg: lat,
                longitude_deg: lon,
                elevation_m: elev,
                utc_offset_h: off,
            };
            site.validate()
                .expect("boundary values are inside the physical ranges");
        }
        let rejected: [(&str, SiteLocationOverride); 9] = [
            (
                "latitude",
                SiteLocationOverride {
                    latitude_deg: Some(90.001),
                    ..SiteLocationOverride::default()
                },
            ),
            (
                "latitude",
                SiteLocationOverride {
                    latitude_deg: Some(f64::NAN),
                    ..SiteLocationOverride::default()
                },
            ),
            (
                "longitude",
                SiteLocationOverride {
                    longitude_deg: Some(-180.001),
                    ..SiteLocationOverride::default()
                },
            ),
            (
                "longitude",
                SiteLocationOverride {
                    longitude_deg: Some(f64::INFINITY),
                    ..SiteLocationOverride::default()
                },
            ),
            (
                "elevation_m",
                SiteLocationOverride {
                    elevation_m: Some(9000.1),
                    ..SiteLocationOverride::default()
                },
            ),
            (
                "elevation_m",
                SiteLocationOverride {
                    elevation_m: Some(-500.1),
                    ..SiteLocationOverride::default()
                },
            ),
            (
                "utc_offset_h",
                SiteLocationOverride {
                    utc_offset_h: Some(14.1),
                    ..SiteLocationOverride::default()
                },
            ),
            (
                "utc_offset_h",
                SiteLocationOverride {
                    utc_offset_h: Some(f64::NEG_INFINITY),
                    ..SiteLocationOverride::default()
                },
            ),
            (
                "latitude",
                SiteLocationOverride {
                    latitude_deg: Some(f64::NAN),
                    longitude_deg: Some(f64::NAN),
                    elevation_m: Some(f64::NAN),
                    utc_offset_h: Some(f64::NAN),
                },
            ),
        ];
        for (field, site) in rejected {
            let err = site
                .validate()
                .expect_err("an out-of-range field is an error");
            assert!(
                err.to_string().contains(field),
                "the message names the offending field {field}, got: {err}"
            );
        }
    }

    #[test]
    fn removed_key_messages_are_shared_by_every_surface() {
        // The message the synthetic [simulation] table's check raises and the
        // message a Python keyword surface raises are one string: the
        // replacement text the row carries.
        let duration_s = removed_key_message(KeyScope::SimulationConfig, "duration_s")
            .expect("duration_s is a removed key of the simulation config");
        assert!(
            duration_s.contains("duration (seconds)"),
            "the message names the replacement, got: {duration_s}"
        );
        let latitude = removed_key_message(KeyScope::PythonKeyword, "latitude")
            .expect("latitude is a removed keyword");
        assert!(
            latitude.contains("site_location") && latitude.contains("latitude_deg"),
            "the message names the replacement, got: {latitude}"
        );
        assert!(
            removed_key_message(KeyScope::SimulationConfig, "not_removed").is_none(),
            "an unknown key is no removed key"
        );
    }

    #[test]
    fn removed_keys_scopes_pin_this_rows_rows() {
        // Every row this entry adds, in its scope: the dwelling table's
        // [output] table, the [simulation] table's old keys (the output
        // booleans among them, removed by the destinations row), and the
        // Python keyword surface's old names.
        let expected: [(KeyScope, &str); 18] = [
            (KeyScope::DwellingTable, "output"),
            (KeyScope::DwellingTable, "initialization_duration"),
            (KeyScope::DwellingTable, "initialization_duration_s"),
            (KeyScope::SimulationConfig, "initialization_duration_s"),
            (KeyScope::SimulationConfig, "duration_s"),
            (KeyScope::SimulationConfig, "time_res_s"),
            (KeyScope::SimulationConfig, "write_output"),
            (KeyScope::SimulationConfig, "output_format"),
            (KeyScope::SimulationConfig, "output_path"),
            (KeyScope::SimulationConfig, "output_verbosity"),
            (KeyScope::SimulationConfig, "retain_batches"),
            (KeyScope::SimulationConfig, "rotation"),
            (KeyScope::PythonKeyword, "latitude"),
            (KeyScope::PythonKeyword, "longitude"),
            (KeyScope::PythonKeyword, "elevation_m"),
            (KeyScope::PythonKeyword, "utc_offset_h"),
            (KeyScope::PythonKeyword, "output_to_parquet"),
            (KeyScope::PythonKeyword, "defaults_path"),
        ];
        for (scope, key) in expected {
            assert!(
                removed_key(scope, key).is_some(),
                "{key:?} must be a removed key of the {scope:?} scope"
            );
        }
        // The warm-up keys are removed from the dwelling table's scope and
        // name the simulation config's warmup field.
        for key in ["initialization_duration", "initialization_duration_s"] {
            let replacement = removed_key(KeyScope::DwellingTable, key)
                .unwrap_or_else(|| panic!("{key} must be a removed key of the dwelling table"));
            assert!(
                replacement.contains("SimulationConfig.warmup"),
                "the {key} row must name SimulationConfig.warmup: {replacement}"
            );
        }
        // The output booleans' rows name the report list: every surface's
        // error points at SimulationConfig.reports.
        for key in [
            "write_output",
            "output_format",
            "output_path",
            "output_verbosity",
            "retain_batches",
            "rotation",
            "output_to_parquet",
        ] {
            let scope = if key == "output_to_parquet" {
                KeyScope::PythonKeyword
            } else {
                KeyScope::SimulationConfig
            };
            let replacement =
                removed_key(scope, key).unwrap_or_else(|| panic!("{key} must be a removed key"));
            assert!(
                replacement.contains("reports"),
                "the {key} row must name the report list, got: {replacement}"
            );
        }
    }

    #[test]
    fn to_toml_inverts_from_toml() {
        let toml = r#"
start_time = "2024-06-15T12:00:00+00:00"
duration = 7200
time_res = 300
output_chunk_size = 5000
setpoint_deadband_c = 1.5
master_seed = 42
civil_timezone = "America/Denver"
max_consecutive_step_failures = 2

[[reports]]
name = "frame"
selection = { verbosity = 5 }
destination = "memory"

[[reports]]
name = "archive"
selection = { pattern = "Total *" }
destination = { parquet = { path = "/tmp/sim_output.parquet", overwrite = true } }

[site_location]
latitude_deg = 39.7392
longitude_deg = -104.9903
elevation_m = 1609.0
utc_offset_h = -7.0
"#;
        let cfg = SimulationConfig::from_toml(toml).expect("parse config");
        let serialized = cfg.to_toml().expect("serialize config");
        let round_tripped =
            SimulationConfig::from_toml(&serialized).expect("re-parse serialized config");
        assert_eq!(cfg, round_tripped, "to_toml inverts from_toml");
        for key in [
            "start_time",
            "duration",
            "time_res",
            "output_chunk_size",
            "setpoint_deadband_c",
            "master_seed",
            "civil_timezone",
            "max_consecutive_step_failures",
            "site_location",
            "reports",
        ] {
            assert!(
                serialized.contains(key),
                "the serialized TOML carries the field name {key}: {serialized}"
            );
        }
    }

    #[test]
    fn from_toml_rejects_the_removed_simulation_keys() {
        for (key, value) in [
            ("duration_s", "3600"),
            ("time_res_s", "60"),
            ("initialization_duration_s", "86400"),
        ] {
            let toml = format!(
                "\nstart_time = \"2024-01-01T00:00:00Z\"\nduration = 3600\n{key} = {value}\n"
            );
            let err = SimulationConfig::from_toml(&toml)
                .expect_err("a removed [simulation] key is an error on the plain TOML path too");
            assert!(
                err.to_string().contains(key),
                "the error names the removed key {key}, got: {err}"
            );
        }
        // The output booleans' rows name the report list: the file path,
        // the format, the retention and the verbosity all move into the
        // reports.
        for (key, value) in [
            ("write_output", "false"),
            ("output_format", "\"parquet\""),
            ("output_path", "\"out.csv\""),
            ("output_verbosity", "2"),
            ("retain_batches", "true"),
            ("rotation", "\"daily\""),
        ] {
            let toml = format!(
                "\nstart_time = \"2024-01-01T00:00:00Z\"\nduration = 3600\n{key} = {value}\n"
            );
            let err = SimulationConfig::from_toml(&toml)
                .expect_err("a removed output key is an error on the plain TOML path too");
            assert!(
                err.to_string().contains(key) && err.to_string().contains("reports"),
                "the error names the removed key {key} and the report list, got: {err}"
            );
        }
    }

    /// A minimal config with no warmup key carries the default converging
    /// warm-up: EnergyPlus's defaults (0.4 delta C, 0.04, at least 1 and at
    /// most 25 warm-up days).
    #[test]
    fn minimal_config_warms_up_by_default() {
        let cfg =
            SimulationConfig::from_toml("start_time = \"2024-01-01T00:00:00Z\"\nduration = 3600\n")
                .expect("parse minimal config");
        assert_eq!(
            cfg.warmup,
            Warmup::Converge(WarmupConfig {
                temperature_tolerance_c: 0.4,
                loads_tolerance: 0.04,
                min_days: 1,
                max_days: 25,
            }),
            "an omitted warmup key means the default converging warm-up"
        );
    }

    /// The three warmup TOML forms round trip: the string "disabled", the
    /// omitted key, and a full [warmup] table, each parse to the expected
    /// `Warmup` and serialise back to the same TOML.
    #[test]
    fn warmup_toml_forms_round_trip() {
        let head = "start_time = \"2024-01-01T00:00:00Z\"\nduration = 3600\n";

        let disabled = SimulationConfig::from_toml(&format!("{head}warmup = \"disabled\"\n"))
            .expect("parse the string form");
        assert_eq!(disabled.warmup, Warmup::Disabled);
        let text = disabled.to_toml().expect("serialise");
        let table: toml::Table = toml::from_str(&text).expect("parse back");
        assert_eq!(
            table.get("warmup").and_then(|v| v.as_str()),
            Some("disabled"),
            "the string form serialises as the same string: {text}"
        );

        let defaulted = SimulationConfig::from_toml(head).expect("parse the omitted key");
        let text = defaulted.to_toml().expect("serialise");
        let again = SimulationConfig::from_toml(&text).expect("parse back");
        assert_eq!(again.warmup, defaulted.warmup, "the default round trips");
        assert_eq!(
            again.warmup,
            Warmup::Converge(WarmupConfig::default()),
            "the omitted key parses to the Rust default"
        );

        let full = SimulationConfig::from_toml(&format!(
            "{head}[warmup]\ntemperature_tolerance_c = 0.3\nloads_tolerance = 0.05\nmin_days = 2\nmax_days = 20\n"
        ))
        .expect("parse the table form");
        assert_eq!(
            full.warmup,
            Warmup::Converge(WarmupConfig {
                temperature_tolerance_c: 0.3,
                loads_tolerance: 0.05,
                min_days: 2,
                max_days: 20,
            })
        );
        let text = full.to_toml().expect("serialise");
        let again = SimulationConfig::from_toml(&text).expect("parse back");
        assert_eq!(again.warmup, full.warmup, "the table form round trips");
    }

    /// A partial [warmup] table is legal: every unset field keeps its
    /// EnergyPlus default, and an unknown field is named by the error.
    #[test]
    fn warmup_partial_table_and_unknown_field() {
        let head = "start_time = \"2024-01-01T00:00:00Z\"\nduration = 3600\n";
        let partial = SimulationConfig::from_toml(&format!("{head}[warmup]\nmax_days = 3\n"))
            .expect("a partial [warmup] table is legal");
        assert_eq!(
            partial.warmup,
            Warmup::Converge(WarmupConfig {
                max_days: 3,
                ..WarmupConfig::default()
            })
        );
        let toml = format!("{head}[warmup]\nno_such_field = 1\n");
        let err = SimulationConfig::from_toml(&toml).expect_err("an unknown field is an error");
        assert!(
            err.to_string().contains("no_such_field"),
            "the error names the unknown field: {err}"
        );
        let toml = format!("{head}warmup = \"converge\"\n");
        let err =
            SimulationConfig::from_toml(&toml).expect_err("an unknown string form is an error");
        assert!(
            err.to_string().contains("disabled"),
            "the error names the accepted spelling: {err}"
        );
    }

    /// A minimum above the maximum is a config error: EnergyPlus silently
    /// resets the maximum to the minimum
    /// (`HeatBalanceManager.cc:686-697`); HARES asks for the day counts it
    /// should run instead of substituting.
    #[test]
    fn min_days_above_max_days_is_a_config_error() {
        let toml = "\nstart_time = \"2024-01-01T00:00:00Z\"\nduration = 3600\n[warmup]\nmin_days = 3\nmax_days = 2\n";
        let err = SimulationConfig::from_toml(toml).expect_err("min above max is an error");
        let message = err.to_string();
        assert!(
            message.contains("min_days") && message.contains("max_days"),
            "the error names both: {message}"
        );
    }

    /// `validate()` rejects a tolerance outside EnergyPlus's IDD bounds
    /// `(0, 0.5]`, a day count of 0, and a minimum above the maximum.
    #[test]
    fn warmup_validation_bounds() {
        let build = |mutate: fn(&mut WarmupConfig)| {
            let mut config = WarmupConfig::default();
            mutate(&mut config);
            let cfg = config_for_warmup(config);
            cfg.validate().err().map(|err| err.to_string())
        };
        fn config_for_warmup(warmup: WarmupConfig) -> SimulationConfig {
            SimulationConfig {
                scenario_seed: None,
                schedule_diversity: Default::default(),
                start_time: chrono::DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z")
                    .expect("a valid sample start"),
                duration: Duration::seconds(3600),
                time_res: Duration::seconds(60),
                reports: Vec::new(),
                diagnostics: false,
                metrics: false,
                output_chunk_size: 10_000,
                setpoint_deadband_c: None,
                master_seed: 0,
                civil_timezone: None,
                site_location: SiteLocationOverride::default(),
                max_consecutive_step_failures: DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
                health_policy: HealthPolicy::default(),
                first_hour_interpolation: FirstHour::default(),
                weather_year: WeatherYear::default(),
                warmup: Warmup::Converge(warmup),
            }
        }
        fn zero_temperature(config: &mut WarmupConfig) {
            config.temperature_tolerance_c = 0.0;
        }
        fn above_half_temperature(config: &mut WarmupConfig) {
            config.temperature_tolerance_c = 0.51;
        }
        fn zero_loads(config: &mut WarmupConfig) {
            config.loads_tolerance = 0.0;
        }
        fn above_half_loads(config: &mut WarmupConfig) {
            config.loads_tolerance = 0.51;
        }
        fn nonfinite_loads(config: &mut WarmupConfig) {
            config.loads_tolerance = f64::NAN;
        }
        fn zero_min_days(config: &mut WarmupConfig) {
            config.min_days = 0;
        }
        fn zero_max_days(config: &mut WarmupConfig) {
            config.max_days = 0;
        }
        fn min_above_max(config: &mut WarmupConfig) {
            config.min_days = 3;
            config.max_days = 2;
        }
        for (name, mutate) in [
            ("temperature 0.0", zero_temperature),
            ("temperature 0.51", above_half_temperature),
            ("loads 0.0", zero_loads),
            ("loads 0.51", above_half_loads),
            ("loads NaN", nonfinite_loads),
            ("min_days 0", zero_min_days),
            ("max_days 0", zero_max_days),
            ("min above max", min_above_max),
        ] as [(&str, fn(&mut WarmupConfig)); 8]
        {
            let err = build(mutate).unwrap_or_else(|| panic!("{name} must be a config error"));
            assert!(
                err.contains("warmup"),
                "the {name} error names the warmup field: {err}"
            );
        }
        // The boundary values are accepted: (0, 0.5] inclusive of 0.5.
        let ok = config_for_warmup(WarmupConfig {
            temperature_tolerance_c: 0.5,
            loads_tolerance: 0.5,
            ..WarmupConfig::default()
        });
        ok.validate().expect("0.5 is inside the allowed range");
    }
}
