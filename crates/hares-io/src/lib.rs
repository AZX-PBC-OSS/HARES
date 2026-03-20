//! I/O layer: HPXML parsing, weather data, schedules, config, and output.

pub mod config;
pub mod defaults;
pub mod draw_profile;
pub mod envelope_lut;
pub mod epw;
pub mod hpxml;
pub mod output;
pub mod resstock;
pub mod schedule;
pub mod schedule_resolve;
pub mod weather;

pub use config::{ConfigError, OutputFormat, SimulationConfig};
pub use defaults::{
    BiquadraticCoefficients, DefaultsCategory, DefaultsError, DefaultsStore, EquipmentDefaults,
    HvacCurveSet, HvacCurveVariant, ZipParameters,
};
pub use draw_profile::{
    DistributionSystem, FixtureEfficiency, ansi_resnet_daily_hot_water_l,
    combined_daily_hot_water_l, distribution_daily_hot_water_l, fixture_daily_hot_water_l,
    normalize_draw_profile,
};
pub use envelope_lut::{
    EnvelopeLookup, EnvelopeLookupResult, EnvelopeLutError, PrecomputedLayer, resolve_boundary_name,
};
pub use epw::parse_epw;
pub use hpxml::{Building, EquipmentSpec, ValidationReport, parse_hpxml, resolve_equipment};
pub use output::{
    OutputSummary, StreamingRecorder, build_schema, expected_columns_at_verbosity, mode_to_ordinal,
};
pub use resstock::{
    ColumnMapper, ResStockBuilding, ResStockError, ResStockVersion, parse_resstock_metadata,
};
pub use schedule::{ColumnAggregation, ScheduleTimeSeries, parse_schedule_csv};
pub use schedule_resolve::inject_schedule_into_specs;
pub use weather::{WeatherField, WeatherMeta, WeatherTimeSeries};
