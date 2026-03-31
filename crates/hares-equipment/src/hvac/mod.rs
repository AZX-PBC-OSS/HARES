//! HVAC equipment models.

pub(super) mod ac_config;
pub mod air_conditioner;
pub mod baseboard;
pub mod boiler;
pub(super) mod coil_physics;
pub(super) mod core_config;
pub mod dehumidifier;
pub(super) mod duct_distribution;
pub(super) mod equivalent_battery;
pub mod furnace;
pub mod heat_pump;
pub mod heating_config;
pub(crate) mod helpers;
pub(super) mod hvac_core;
pub mod ideal_hvac;
pub(super) mod latent_degradation;
pub(super) mod speed_control;
pub(super) mod staging;
pub(super) mod thermostat;

pub use ac_config::{
    CentralAirConditionerConfig, DehumidifierConfig, HeatPumpConfig, RoomAcConfig,
};
pub use equivalent_battery::EquivalentBatteryModel;
pub use hvac_core::{HvacEquipment, HvacEquipmentType};
pub use speed_control::{SpeedControlMode, SpeedSelection, StartupConfig};
pub use thermostat::{
    RuntimeSetpointOverride, ScheduleSetpoints, ThermalSetpoints, ThermostatConfig, ThermostatMode,
};
