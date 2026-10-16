//! Simulation output writing (Arrow/Parquet/CSV).
//!
//! Output is written incrementally via [`StreamingRecorder`], which buffers
//! rows up to a configurable chunk size and flushes to disk, keeping peak
//! memory proportional to the chunk size rather than simulation length.

pub mod callback;
pub mod catalogue;
pub mod columns;
pub mod metrics;
pub mod reduce;
pub mod report;
pub mod writer;

use std::path::PathBuf;

pub use callback::{BatchCallback, BatchContext, CallbackAttachment, CallbackError};
pub use catalogue::{
    Catalogue, CatalogueActor, CatalogueEquipment, CatalogueError, CatalogueInput,
    DIAGNOSTICS_METADATA_KEY, EquipmentColumnFamilies, VariableDef, VariableSource,
    build_catalogue, build_schema, build_schema_for_columns, build_schema_for_diagnostics,
    build_schema_for_specs,
};
pub use columns::{
    CAPACITY_SUFFIX, COMPRESSOR_POWER_KW_SUFFIX, COMPRESSOR_POWER_W_SUFFIX, COP_SUFFIX,
    DEFROST_STATE_SUFFIX, ELECTRIC_POWER_SUFFIX, ENERGY_SUFFIX, ER_CAPACITY_SUFFIX,
    ER_POWER_SUFFIX, EV_CHARGING_LEVEL_SUFFIX, EV_CONNECTION_STATE_SUFFIX,
    FAN_ELECTRIC_POWER_SUFFIX, FAN_POWER_SUFFIX, FAN_POWER_W_SUFFIX, GAS_POWER_SUFFIX,
    HP_CAPACITY_SUFFIX, HVAC_DUCT_LOSSES_COL, LATENT_GAINS_SUFFIX, MAIN_POWER_SUFFIX,
    MIN_OFF_TIME_SUFFIX, MIN_ON_TIME_SUFFIX, MODE_SUFFIX, PAN_HEATER_POWER_SUFFIX,
    POWER_FACTOR_SUFFIX, PV_DC_POWER_SUFFIX, PV_IRRADIANCE_SUFFIX, REACTIVE_POWER_SUFFIX,
    RETURN_TEMP_SUFFIX, RUNTIME_COOLING_SETPOINT_COL, RUNTIME_FRACTION_SUFFIX,
    RUNTIME_HEATING_SETPOINT_COL, SCHEDULE_SUFFIX, SCHEDULED_COOLING_SETPOINT_COL,
    SCHEDULED_HEATING_SETPOINT_COL, SETPOINT_SUFFIX, SHR_SUFFIX, SOC_SUFFIX, SPEED_SUFFIX,
    SUPPLY_AIR_TEMP_SUFFIX, SUPPLY_TEMP_SUFFIX, end_use_electric_power_column,
    expected_columns_at_verbosity, fuel_reports_gas_power_column, is_reserved_output_column_name,
    parse_end_use_electric_power_column_key,
};
pub use metrics::{
    EfficiencyMetrics, EnvelopeComponentLoadsKwh, FullSimulationMetrics,
    MetricsAccumulatorsSnapshot, MetricsCalculator, MetricsDeadbandSnapshot, MetricsInputPlan,
    MetricsPlanSnapshot, MetricsRollingPeakSnapshot, MetricsSetpointSnapshot, MetricsSnapshot,
    SimulationMetrics, metrics_from_table,
};
pub use reduce::{PeriodCalendar, PeriodReducer, PeriodRow, ReduceError, TimeRule};
pub use report::{
    Destination, Frequency, ReportSpec, RowPlan, RowPlanError, SchemaSurface, Selection,
    VariableSurface, resolve_selection,
};
pub use writer::{OutputError, ReportRowsSnapshot, StreamingRecorder};

/// Summary returned by [`StreamingRecorder::finish`].
#[derive(Debug, Clone)]
pub struct OutputSummary {
    /// Total number of rows written to the report's destination.
    pub row_count: usize,
    /// Size of the published output file in bytes (0 for a `Memory`
    /// destination or a run that produced zero rows).
    pub byte_size: u64,
    /// The published output file's path.
    pub path: PathBuf,
}
