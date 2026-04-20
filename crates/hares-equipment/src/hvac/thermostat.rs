//! Thermostat types and FSM logic shared across HVAC equipment.

use chrono::{DateTime, FixedOffset};
use hares_types::{EnvironmentState, HaresError, ZoneId};
use serde::{Deserialize, Serialize};

pub(super) const HEATING_DISABLED_SETPOINT_C: f64 = -999.0;
pub(super) const COOLING_DISABLED_SETPOINT_C: f64 = 999.0;
const MAX_CYCLE_TIME_S: f64 = 3600.0;

/// HVAC thermostat configuration shared across heating/cooling equipment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThermostatConfig {
    pub hysteresis_c: f64,
    pub cutout_ratio: f64,
    pub min_cycle_time_s: f64,
    pub use_ideal_capacity: bool,
    /// Fraction of `hysteresis_c` that offsets the turn-on and turn-off thresholds
    /// asymmetrically within the deadband.
    ///
    /// OCHRE HVAC.py line 222: `deadband_offset` defaults to 0.2.
    /// - turn_on  = setpoint − hvac_dir × hysteresis × (1 − offset)
    /// - turn_off = setpoint + hvac_dir × hysteresis × offset
    ///
    /// With offset=0 the thresholds are symmetric (standard hysteresis).
    /// With offset=0.2 (default) the setpoint sits near the top of the deadband
    /// for heating, matching lab measurements from OCHRE.
    ///
    /// Valid range: [0.0, 1.0]. Clamped on use; validated on init.
    pub deadband_offset: f64,
}

impl Default for ThermostatConfig {
    fn default() -> Self {
        Self {
            hysteresis_c: 1.0,
            cutout_ratio: 0.0,
            min_cycle_time_s: 0.0,
            use_ideal_capacity: false,
            deadband_offset: 0.2,
        }
    }
}

impl ThermostatConfig {
    pub fn validate(&mut self, env: &EnvironmentState) -> crate::Result<()> {
        if !(0.0..=1.0).contains(&self.cutout_ratio) {
            return Err(HaresError::Equipment(format!(
                "cutout_ratio must be in [0.0, 1.0], got {}",
                self.cutout_ratio
            )));
        }
        if !self.hysteresis_c.is_finite() || self.hysteresis_c < 0.0 {
            return Err(HaresError::Equipment(format!(
                "hysteresis_c must be finite and >= 0.0, got {}",
                self.hysteresis_c
            )));
        }
        if !self.min_cycle_time_s.is_finite() || self.min_cycle_time_s < 0.0 {
            return Err(HaresError::Equipment(format!(
                "min_cycle_time_s must be finite and >= 0.0, got {}",
                self.min_cycle_time_s
            )));
        }
        if self.min_cycle_time_s > MAX_CYCLE_TIME_S {
            return Err(HaresError::Equipment(format!(
                "min_cycle_time_s ({}) exceeds maximum of {MAX_CYCLE_TIME_S}s",
                self.min_cycle_time_s
            )));
        }
        if self.min_cycle_time_s > 0.0 {
            let time_res_s = env.time_res.num_milliseconds() as f64 / 1000.0;
            if self.min_cycle_time_s < time_res_s {
                // A cycle time shorter than the timestep is meaningless -- clamp
                // it up so that coarse-resolution simulations (e.g. 15-min) work
                // without requiring the user to override every thermostat spec.
                self.min_cycle_time_s = time_res_s;
            }
        }
        if !self.deadband_offset.is_finite() || !(0.0..=1.0).contains(&self.deadband_offset) {
            return Err(HaresError::Equipment(format!(
                "deadband_offset must be in [0.0, 1.0], got {}",
                self.deadband_offset
            )));
        }
        Ok(())
    }
}

/// Thermostat finite-state mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThermostatMode {
    Heating,
    Cooling,
    #[default]
    Deadband,
}

/// Setpoint pair in Celsius.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThermalSetpoints {
    pub heating_c: f64,
    pub cooling_c: f64,
}

impl ThermalSetpoints {
    pub fn validate_for_deadband(self, hysteresis_c: f64) -> crate::Result<()> {
        if self.cooling_c - self.heating_c < 2.0 * hysteresis_c {
            return Err(HaresError::Equipment(format!(
                "invalid setpoints: cooling-heating must be >= {}C, got {}",
                2.0 * hysteresis_c,
                self.cooling_c - self.heating_c
            )));
        }
        Ok(())
    }

    pub fn with_schedule_override(self, schedule: Option<ScheduleSetpoints>) -> Self {
        let Some(schedule) = schedule else {
            return self;
        };
        let mut merged = self;
        if schedule.no_space_heating {
            merged.heating_c = HEATING_DISABLED_SETPOINT_C;
        } else if let Some(value) = schedule.heating_c {
            merged.heating_c = value;
        }
        if schedule.no_space_cooling {
            merged.cooling_c = COOLING_DISABLED_SETPOINT_C;
        } else if let Some(value) = schedule.cooling_c {
            merged.cooling_c = value;
        }
        merged
    }

    pub fn with_control_override(self, control: Option<RuntimeSetpointOverride>) -> Self {
        let Some(control) = control else {
            return self;
        };
        Self {
            heating_c: control.heating_c.unwrap_or(self.heating_c),
            cooling_c: control.cooling_c.unwrap_or(self.cooling_c),
        }
    }
}

/// Time-varying schedule setpoints and ResStock no-space-conditioning sentinels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScheduleSetpoints {
    pub heating_c: Option<f64>,
    pub cooling_c: Option<f64>,
    pub no_space_heating: bool,
    pub no_space_cooling: bool,
}

/// Runtime control-signal override values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RuntimeSetpointOverride {
    pub heating_c: Option<f64>,
    pub cooling_c: Option<f64>,
}

pub(super) fn lookup_zone_temp(env: &EnvironmentState, zone: ZoneId) -> crate::Result<f64> {
    env.zones
        .iter()
        .find(|z| z.id == zone)
        .map(|z| z.temperature_c)
        .ok_or_else(|| HaresError::Equipment(format!("zone {zone:?} not found")))
}

pub(super) fn is_cycle_change_allowed(
    thermostat: &ThermostatConfig,
    last_mode_switch_at: Option<DateTime<FixedOffset>>,
    now: DateTime<FixedOffset>,
) -> bool {
    if thermostat.min_cycle_time_s <= 0.0 {
        return true;
    }
    let Some(last_switch) = last_mode_switch_at else {
        return true;
    };
    let elapsed_ms = (now - last_switch).num_milliseconds().max(0) as f64;
    elapsed_ms / 1000.0 >= thermostat.min_cycle_time_s
}
