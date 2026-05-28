//! Shared constants for HARES actors.

/// Default freeze-protection threshold in °C.
///
/// ASHRAE Guideline 36-2021 §5.16: freeze-stat setpoint not to exceed 4.4°C (40°F).
/// 5°C is a conservative residential default aligned with freeze-protection setpoints
/// used in building simulation. EnergyPlus implements Guideline 36-2018 §5.16.3.1
/// freeze-protection logic at vendors/EnergyPlus/src/EnergyPlus/MixedAir.cc:4191.
pub const DEFAULT_FREEZE_THRESHOLD_C: f64 = 5.0;
