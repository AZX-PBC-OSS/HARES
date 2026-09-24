//! Shared constants for HARES actors.

/// Default freeze-protection threshold in °C.
///
/// ASHRAE Guideline 36-2021 §5.16: freeze-stat setpoint not to exceed 4.4°C (40°F).
/// 5°C is a conservative residential default aligned with freeze-protection setpoints
/// used in building simulation.
///
/// Note: EnergyPlus vendor source was searched for a matching freeze-protection
/// literal (4.4°C / 40°F) and freeze-stat call path, but the G36 §5.16.3.1
/// reference in `MixedAir.cc:4191` is outdoor-air minimum flow-rate limiting —
/// not freeze protection. No corresponding freeze-protection literal was found in
/// the EnergyPlus source tree.
pub const DEFAULT_FREEZE_THRESHOLD_C: f64 = 5.0;
