//! I/O layer: HPXML parsing, weather data, schedules, config, and output.

pub mod config;
pub mod defaults;
pub mod draw_profile;
pub mod envelope_lut;
pub mod epw;
pub mod hpxml;
pub mod hpxml_schedule;
pub mod output;
pub mod psm3;
pub mod pv_sizing;
pub mod resstock;
pub mod resstock_csv;
pub mod schedule;
pub mod schedule_resolve;
pub mod site_location;
pub mod tmy3;
pub mod weather;

pub use config::{ConfigError, OutputFormat, SimulationConfig};
pub use defaults::{
    BiquadraticCoefficients, DefaultsCategory, DefaultsError, DefaultsStore, EquipmentDefaults,
    HvacCurveSet, HvacCurveVariant, PvPanelDefaults, ZipParameters,
};
pub use draw_profile::{
    DistributionSystem, FixtureEfficiency, ansi_resnet_daily_hot_water_l,
    combined_daily_hot_water_l, distribution_daily_hot_water_l, fixture_daily_hot_water_l,
    normalize_draw_profile,
};
pub use envelope_lut::{
    EnvelopeLookup, EnvelopeLookupResult, EnvelopeLutError, PrecomputedLayer, resolve_boundary_name,
};
pub use epw::{
    DesignConditions, SkyTempModel, berdahl_martin_sky_emissivity, clark_allen_sky_emissivity,
    compute_sky_temp_c, monthly_day_counts, parse_epw, sky_temp_from_emissivity,
};
// Re-export canonical physical constants from hares-physics (preserving public API names).
pub use hares_physics::constants::CELSIUS_TO_KELVIN as KELVIN_OFFSET_C;
pub use hares_physics::constants::STEFAN_BOLTZMANN;
#[cfg(any(debug_assertions, feature = "check_invariants"))]
pub use hpxml::building::check_foundation_zone_invariant;
pub use hpxml::{
    Building, EquipmentSpec, HpxmlDataPatches, ValidationReport, parse_hpxml, resolve_equipment,
};
pub use output::{
    EfficiencyMetrics, EnvelopeComponentLoadsKwh, FullSimulationMetrics, MetricsCalculator,
    OutputSummary, SimulationMetrics, StreamingRecorder, build_schema, display_name_to_end_use_key,
    end_use_display_name, end_use_electric_power_column, equipment_name_to_end_use,
    expected_columns_at_verbosity, parse_end_use_electric_power_column,
    parse_end_use_electric_power_column_key,
};
pub use psm3::parse_psm3;
pub use resstock::{
    ColumnMapper, ResStockBuilding, ResStockError, ResStockVersion, parse_resstock_metadata,
};
pub use resstock_csv::parse_resstock_csv;
pub use schedule::{ColumnAggregation, ScheduleTimeSeries, parse_schedule_csv};
#[cfg(any(debug_assertions, feature = "check_invariants"))]
pub use schedule_resolve::check_hvac_setpoint_invariants;
pub use schedule_resolve::inject_schedule_into_specs;
pub use site_location::{FieldSource, SiteLocation, SiteLocationOverride, resolve_site_location};
pub use tmy3::parse_tmy3;
pub use weather::{
    ResampleMethod, ResampleOverrides, WeatherField, WeatherMeta, WeatherTimeSeries,
};
pub use weather::{
    WeatherFormat, parse_weather, parse_weather_with_elevation, parse_weather_with_location,
};
