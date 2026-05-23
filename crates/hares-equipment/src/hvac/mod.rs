//! HVAC equipment models.

pub(super) mod ac_config;
pub mod air_conditioner;
pub mod baseboard;
pub mod boiler;
pub(super) mod coil_physics;
pub mod cooling_config;
pub(super) mod core_config;
pub mod dehumidifier;
pub(super) mod duct_distribution;
pub(super) mod equivalent_battery;
pub mod furnace;
pub mod heat_pump;
pub mod heat_pump_config;
pub mod heating_config;
pub(crate) mod helpers;
pub(super) mod hvac_core;
pub mod ideal_hvac;
pub(super) mod latent_degradation;
pub(super) mod speed_control;
pub(super) mod staging;
pub(super) mod thermostat;

pub use cooling_config::{CentralAirConditionerConfig, DehumidifierConfig, RoomAcConfig};
pub use equivalent_battery::EquivalentBatteryModel;
pub use heat_pump_config::{
    HeatPumpCommonConfig, HeatPumpConfig, HeatPumpCoolerConfig, HeatPumpHeaterConfig,
};
pub use hvac_core::{
    AIRFLOW_CENTRAL_AC_M3_S_PER_W, AIRFLOW_HEATING_M3_S_PER_W, AIRFLOW_MSHP_COOLING_M3_S_PER_W,
    AIRFLOW_ROOM_AC_M3_S_PER_W, HvacConfig, HvacControlState, HvacEquipment, HvacEquipmentType,
    HvacRuntimeState, MAX_SPEEDS,
};
pub use speed_control::{
    SpeedControlMode, SpeedSelection, StartupConfig, capacity_fractions_for,
    interpolate_speed_stages,
};
pub use thermostat::{
    RuntimeSetpointOverride, ScheduleSetpoints, ThermalSetpoints, ThermostatConfig, ThermostatFsm,
    ThermostatMode,
};
