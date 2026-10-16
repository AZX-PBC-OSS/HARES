//! I/O layer: HPXML parsing, weather data, schedules, config, and output.

pub mod config;
pub mod defaults;
pub mod draw_profile;
pub mod envelope_lut;
pub mod envelope_scales;
pub mod epw;
pub mod forcing;
pub mod hpxml;
pub mod hpxml_schedule;
pub mod input;
pub mod input_schema;
pub mod inputs;
pub mod output;
pub mod parse_counts;
pub mod psm3;
pub mod pv_sizing;
pub mod resstock;
pub mod resstock_csv;
pub mod sample_weight;
pub mod schedule;
pub mod schedule_resolve;
pub mod setpoint_source;
pub mod site_location;
pub mod tmy3;
pub mod unavailable_periods;
pub mod weather;
pub mod wsf;

pub use config::{
    ConfigError, DEFAULT_LOADS_TOLERANCE, DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
    DEFAULT_MAX_WARMUP_DAYS, DEFAULT_MIN_WARMUP_DAYS, DEFAULT_OUTPUT_CHUNK_SIZE,
    DEFAULT_TEMPERATURE_TOLERANCE_C, DEFAULT_TIME_RES_SECS, HealthPolicy, KeyScope, MAX_VERBOSITY,
    REMOVED_KEYS, RemovedKey, SimulationConfig, Warmup, WarmupConfig, check_removed_toml_keys,
    removed_key, removed_key_message,
};
pub use defaults::{
    BiquadraticCoefficients, DefaultsCategory, DefaultsError, DefaultsStore, EquipmentDefaults,
    HvacCurveSet, HvacCurveVariant, PvPanelDefaults, ZipLoad,
};
pub use draw_profile::{
    DistributionSystem, FixtureEfficiency, ansi_resnet_daily_hot_water_l,
    combined_daily_hot_water_l, distribution_daily_hot_water_l, fixture_daily_hot_water_l,
    normalize_draw_profile,
};
pub use envelope_lut::{
    EnvelopeLookup, EnvelopeLookupResult, EnvelopeLutError, PrecomputedLayer, resolve_boundary_name,
};
pub use envelope_scales::{ENVELOPE_OVERRIDE_KEY, EnvelopeScales};
pub use epw::{
    DesignConditions, SkyTempModel, berdahl_martin_sky_emissivity, clark_allen_sky_emissivity,
    compute_sky_temp_c, monthly_day_counts, parse_epw, parse_epw_location_state,
    parse_epw_station_wmo, sky_temp_from_emissivity,
};
// Re-export canonical physical constants from hares-physics (preserving public API names).
pub use forcing::{
    FORCING_COMPUTATIONS, FORCING_TABLES_LIVE, ForcingKey, ForcingRow, ForcingTable,
};
pub use hares_physics::constants::CELSIUS_TO_KELVIN as KELVIN_OFFSET_C;
pub use hares_physics::constants::STEFAN_BOLTZMANN;
pub use hpxml::building::check_foundation_zone_invariant;
pub use hpxml::{
    Building, EquipmentSpec, HpxmlDataPatches, ValidationReport, parse_hpxml,
    parse_iecc_climate_zone, resolve_equipment,
};
pub use input::{InputRecord, InputRecordSource, InputRecords, defaults_files_digest};
pub use inputs::{
    DefaultsSource, InputSource, ParsedDefaults, ParsedHpxml, ParsedInputs, ParsedInputsCache,
    ParsedSchedule, ParsedWeather, ScheduleGeneration, WeatherInput, read_site_input,
    read_weather_meta_input,
};
pub use output::{
    CAPACITY_SUFFIX, COMPRESSOR_POWER_KW_SUFFIX, COMPRESSOR_POWER_W_SUFFIX, COP_SUFFIX, Catalogue,
    CatalogueActor, CatalogueEquipment, CatalogueError, CatalogueInput, DEFROST_STATE_SUFFIX,
    DIAGNOSTICS_METADATA_KEY, Destination, ELECTRIC_POWER_SUFFIX, ENERGY_SUFFIX,
    ER_CAPACITY_SUFFIX, ER_POWER_SUFFIX, EV_CHARGING_LEVEL_SUFFIX, EV_CONNECTION_STATE_SUFFIX,
    EfficiencyMetrics, EnvelopeComponentLoadsKwh, EquipmentColumnFamilies,
    FAN_ELECTRIC_POWER_SUFFIX, FAN_POWER_SUFFIX, FAN_POWER_W_SUFFIX, Frequency,
    FullSimulationMetrics, GAS_POWER_SUFFIX, HP_CAPACITY_SUFFIX, HVAC_DUCT_LOSSES_COL,
    LATENT_GAINS_SUFFIX, MAIN_POWER_SUFFIX, MIN_OFF_TIME_SUFFIX, MIN_ON_TIME_SUFFIX, MODE_SUFFIX,
    MetricsAccumulatorsSnapshot, MetricsCalculator, MetricsDeadbandSnapshot, MetricsInputPlan,
    MetricsPlanSnapshot, MetricsRollingPeakSnapshot, MetricsSetpointSnapshot, MetricsSnapshot,
    OutputSummary, PAN_HEATER_POWER_SUFFIX, POWER_FACTOR_SUFFIX, PV_DC_POWER_SUFFIX,
    PV_IRRADIANCE_SUFFIX, REACTIVE_POWER_SUFFIX, RETURN_TEMP_SUFFIX, RUNTIME_COOLING_SETPOINT_COL,
    RUNTIME_FRACTION_SUFFIX, RUNTIME_HEATING_SETPOINT_COL, ReportRowsSnapshot, ReportSpec, RowPlan,
    RowPlanError, RowWriter, SCHEDULE_SUFFIX, SCHEDULED_COOLING_SETPOINT_COL,
    SCHEDULED_HEATING_SETPOINT_COL,
    SETPOINT_SUFFIX, SHR_SUFFIX, SOC_SUFFIX, SPEED_SUFFIX, SUPPLY_AIR_TEMP_SUFFIX,
    SUPPLY_TEMP_SUFFIX, SchemaSurface, Selection, SimulationMetrics, StreamingRecorder,
    VariableDef, VariableSource, VariableSurface, build_catalogue, build_schema,
    build_schema_for_columns, build_schema_for_diagnostics, build_schema_for_specs,
    end_use_electric_power_column, expected_columns_at_verbosity, fuel_reports_gas_power_column,
    is_reserved_output_column_name, metrics_from_table, parse_end_use_electric_power_column_key,
    resolve_selection,
};
pub use psm3::parse_psm3;
pub use resstock::{
    ColumnMapper, ResStockBuilding, ResStockError, ResStockVersion, WeatherStationError,
    parse_resstock_metadata, parse_weather_station_fips,
};
pub use resstock_csv::parse_resstock_csv;
pub use sample_weight::{SampleWeightClass, classify_sample_weight};
pub use schedule::resolve_occupancy_column;
pub use schedule::{ColumnAggregation, ScheduleTimeSeries, parse_schedule_csv};
pub use schedule_resolve::check_hvac_setpoint_invariants;
pub use schedule_resolve::parse_default_profiles_str;
pub use schedule_resolve::{DefaultProfiles, DefaultScheduleProfile, load_default_profiles};
pub use schedule_resolve::{
    inject_schedule_into_specs, inject_schedule_into_specs_with_pre_period,
};
pub use setpoint_source::SetpointSource;
pub use site_location::{FieldSource, SiteLocation, SiteLocationOverride, resolve_site_location};
pub use tmy3::parse_tmy3;
pub use unavailable_periods::{
    NaturalVentilation, UnavailableKind, UnavailablePeriod, apply_unavailable_periods,
};
pub use weather::{
    DataPeriod, FirstHour, RecordLabel, RecordStamp, WeatherAddressing, WeatherField, WeatherMeta,
    WeatherTimeSeries, WeatherYear,
};
pub use weather::{
    WeatherFormat, detect_weather_format, parse_weather, parse_weather_with_elevation,
    parse_weather_with_location, read_weather_meta,
};
