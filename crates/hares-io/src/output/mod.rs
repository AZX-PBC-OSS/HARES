//! Simulation output writing (Arrow/Parquet/CSV).
//!
//! Output is written incrementally via [`StreamingRecorder`], which buffers
//! rows up to a configurable chunk size and flushes to disk, keeping peak
//! memory proportional to the chunk size rather than simulation length.

pub mod columns;
pub mod metrics;
pub mod writer;

use std::path::PathBuf;

pub use columns::{
    CAPACITY_SUFFIX, COMPRESSOR_POWER_KW_SUFFIX, COMPRESSOR_POWER_W_SUFFIX, COP_SUFFIX,
    DEFROST_STATE_SUFFIX, ELECTRIC_POWER_SUFFIX, ENERGY_SUFFIX, ER_CAPACITY_SUFFIX,
    ER_POWER_SUFFIX, EV_CHARGING_LEVEL_SUFFIX, EV_CONNECTION_STATE_SUFFIX,
    FAN_ELECTRIC_POWER_SUFFIX, FAN_POWER_SUFFIX, FAN_POWER_W_SUFFIX, GAS_POWER_SUFFIX,
    HP_CAPACITY_SUFFIX, HVAC_DUCT_LOSSES_COL, LATENT_GAINS_SUFFIX, MAIN_POWER_SUFFIX, MODE_SUFFIX,
    PAN_HEATER_POWER_SUFFIX, POWER_FACTOR_SUFFIX, PV_DC_POWER_SUFFIX, PV_IRRADIANCE_SUFFIX,
    REACTIVE_POWER_SUFFIX, RETURN_TEMP_SUFFIX, RUNTIME_COOLING_SETPOINT_COL,
    RUNTIME_FRACTION_SUFFIX, RUNTIME_HEATING_SETPOINT_COL, SCHEDULE_SUFFIX,
    SCHEDULED_COOLING_SETPOINT_COL, SCHEDULED_HEATING_SETPOINT_COL, SETPOINT_SUFFIX, SHR_SUFFIX,
    SOC_SUFFIX, SPEED_SUFFIX, SUPPLY_AIR_TEMP_SUFFIX, SUPPLY_TEMP_SUFFIX, build_schema,
    display_name_to_end_use_key, end_use_display_name, end_use_electric_power_column,
    equipment_name_to_end_use, expected_columns_at_verbosity, has_soc, is_compressor_equipment,
    is_cooling_equipment, is_ev, is_heat_pump_heater, is_hvac_or_wh, is_pv,
    parse_end_use_electric_power_column, parse_end_use_electric_power_column_key,
};
pub use metrics::{
    EfficiencyMetrics, EnvelopeComponentLoadsKwh, FullSimulationMetrics, MetricsCalculator,
    SimulationMetrics,
};
pub use writer::{OutputError, StreamingRecorder};

/// Summary returned by [`StreamingRecorder::finish`].
#[derive(Debug, Clone)]
pub struct OutputSummary {
    /// Total number of rows written across all files.
    pub row_count: usize,
    /// Total size of all output files in bytes.
    pub byte_size: u64,
    /// Path to the first (or only) output file.
    pub path: PathBuf,
    /// Paths to all output files, including rotated successors.
    /// When `rotation` is `None`, this contains only the primary path.
    pub rotated_paths: Vec<PathBuf>,
}
