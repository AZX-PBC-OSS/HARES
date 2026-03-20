//! HVAC equipment models.

pub mod air_conditioner;
pub mod baseboard;
pub mod boiler;
pub(super) mod coil_physics;
pub mod common;
pub mod dehumidifier;
pub mod furnace;
pub mod heat_pump;
pub(crate) mod helpers;
pub(super) mod hvac_core;
pub(super) mod speed_control;
pub(super) mod thermostat;

pub use hvac_core::{EquivalentBatteryModel, HvacEquipment, HvacEquipmentType, IdealCapacitySolver};
pub use speed_control::{SpeedControlMode, SpeedSelection, StartupConfig};
pub use thermostat::{
    RuntimeSetpointOverride, ScheduleSetpoints, ThermalSetpoints, ThermostatConfig, ThermostatMode,
};
