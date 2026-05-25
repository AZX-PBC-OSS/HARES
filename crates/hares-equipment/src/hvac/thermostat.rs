//! Thermostat types and FSM logic shared across HVAC equipment.

use chrono::{DateTime, FixedOffset};
use hares_types::{EnvironmentState, HaresError, ScheduleSource, ZoneId};
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
    let elapsed_ms = (now - last_switch).num_milliseconds();
    if elapsed_ms <= 0 {
        return true;
    }
    elapsed_ms as f64 / 1000.0 >= thermostat.min_cycle_time_s
}

/// Thermostat finite-state machine owning the 11 fields and 5 methods that
/// were previously duplicated between [`HvacEquipment`](super::HvacEquipment)
/// and [`IdealHvac`](super::IdealHvac).
///
/// Both structs embed `pub thermostat_fsm: ThermostatFsm` and delegate to it.
/// Equipment-specific side-effects (e.g. `IdealHvac` clearing
/// `ideal_capacity_w` on Deadband entry) remain in wrapper methods on the
/// owning struct.
#[derive(Clone, Debug, PartialEq)]
pub struct ThermostatFsm {
    pub mode: ThermostatMode,
    pub mode_start_at: Option<DateTime<FixedOffset>>,
    pub last_mode_switch_at: Option<DateTime<FixedOffset>>,
    pub thermostat: ThermostatConfig,
    pub static_setpoints: ThermalSetpoints,
    pub schedule_setpoints: Option<ScheduleSetpoints>,
    pub runtime_setpoints: Option<RuntimeSetpointOverride>,
    pub heating_setpoint_source: Option<ScheduleSource>,
    pub cooling_setpoint_source: Option<ScheduleSource>,
    /// Minimum time [s] compressor must remain On before an Off transition is
    /// allowed. Prevents short-cycle wear. 0.0 = disabled (default).
    pub min_on_time_s: f64,
    /// Minimum time [s] compressor must remain Off before an On transition is
    /// allowed. Prevents short-cycle wear. 0.0 = disabled (default).
    pub min_off_time_s: f64,
}

impl ThermostatFsm {
    pub fn new(static_setpoints: ThermalSetpoints) -> Self {
        Self {
            mode: ThermostatMode::Deadband,
            mode_start_at: None,
            last_mode_switch_at: None,
            thermostat: ThermostatConfig::default(),
            static_setpoints,
            schedule_setpoints: None,
            runtime_setpoints: None,
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
            min_on_time_s: 0.0,
            min_off_time_s: 0.0,
        }
    }

    pub fn effective_setpoints(&self) -> ThermalSetpoints {
        self.static_setpoints
            .with_schedule_override(self.schedule_setpoints)
            .with_control_override(self.runtime_setpoints)
    }

    /// Resolve the current setpoint from config-owned schedule data and inject
    /// as `schedule_setpoints`. Priority:
    ///   1. Per-timestep schedule array (from CSV column)
    ///   2. 24-hour weekday/weekend profile (from HPXML thermostat)
    ///   3. None -- falls through to static_setpoints
    pub fn resolve_profile_setpoints(&mut self, env: &EnvironmentState) {
        if self.heating_setpoint_source.is_none() && self.cooling_setpoint_source.is_none() {
            return;
        }

        let heating_c = self
            .heating_setpoint_source
            .as_mut()
            .and_then(|source| source.value_at(env).ok());
        let cooling_c = self
            .cooling_setpoint_source
            .as_mut()
            .and_then(|source| source.value_at(env).ok());

        if heating_c.is_some() || cooling_c.is_some() {
            self.schedule_setpoints = Some(ScheduleSetpoints {
                heating_c,
                cooling_c,
                ..ScheduleSetpoints::default()
            });
        } else {
            self.schedule_setpoints = None;
        }
    }

    /// Set the thermostat mode and record the transition timestamp.
    ///
    /// This is the only correct way to change `mode`. It atomically updates
    /// `mode_start_at` to `when`, upholding the invariant that `mode_start_at`
    /// always reflects when the current mode began. Callers that bypass this
    /// method by assigning `mode` directly will silently break minimum on/off
    /// time enforcement in `can_transition_mode`.
    pub fn set_mode(&mut self, mode: ThermostatMode, when: DateTime<FixedOffset>) {
        if self.mode != mode {
            self.mode = mode;
            self.last_mode_switch_at = Some(when);
            self.mode_start_at = Some(when);
        }
    }

    /// Returns `false` when a minimum on-time or off-time constraint blocks the
    /// proposed mode transition.
    ///
    /// - Deadband → any On mode: blocked until the unit has been Off for at least
    ///   `min_off_time_s` (compressor short-cycle protection on restart).
    /// - Any On mode → Deadband: blocked until the unit has been On for at least
    ///   `min_on_time_s` (compressor short-cycle protection on shutdown).
    /// - On mode → different On mode (e.g. Heating↔Cooling reversal): blocked
    ///   until `min_on_time_s` in the current mode has elapsed.
    ///
    /// Returns `true` when `mode_start_at` is `None` (first transition ever) or
    /// when the minimum duration for the current mode has elapsed.
    pub fn can_transition_mode(
        &self,
        proposed: ThermostatMode,
        now: DateTime<FixedOffset>,
    ) -> bool {
        if self.mode == proposed {
            return true;
        }
        let Some(start) = self.mode_start_at else {
            return true;
        };
        let elapsed_s = (now - start).num_milliseconds().max(0) as f64 / 1000.0;
        let current_is_on = self.mode != ThermostatMode::Deadband;
        let min_s = if current_is_on {
            self.min_on_time_s
        } else {
            self.min_off_time_s
        };
        elapsed_s >= min_s
    }

    /// Core thermostat hysteresis + cycle-time logic. Returns the resolved mode.
    ///
    /// Callers that need to track the target temperature (e.g. `IdealHvac`
    /// maintaining `current_target_c`) should wrap this method and add their
    /// own pre/post hooks rather than modifying the FSM's internals.
    pub fn update_mode(
        &mut self,
        env: &EnvironmentState,
        zone_id: ZoneId,
    ) -> crate::Result<ThermostatMode> {
        self.resolve_profile_setpoints(env);
        let zone_temp = lookup_zone_temp(env, zone_id)?;
        let setpoints = self.effective_setpoints();

        tracing::debug!(
            zone_temp,
            heating_setpoint = setpoints.heating_c,
            cooling_setpoint = setpoints.cooling_c,
            current_mode = ?self.mode,
            "update_mode: evaluating mode transition"
        );

        if !is_cycle_change_allowed(&self.thermostat, self.last_mode_switch_at, env.current_time) {
            tracing::debug!(
                elapsed_s = self
                    .last_mode_switch_at
                    .map(|t| (env.current_time - t).num_milliseconds().max(0) as f64 / 1000.0)
                    .unwrap_or(0.0),
                min_cycle_time_s = self.thermostat.min_cycle_time_s,
                "is_cycle_change_allowed: blocked by min_cycle_time"
            );
            return Ok(self.mode);
        }

        let hysteresis = self.thermostat.hysteresis_c;
        let offset = self.thermostat.deadband_offset.clamp(0.0, 1.0);
        let cutout = self.thermostat.cutout_ratio;
        let next_mode = if offset > 0.0 {
            match self.mode {
                ThermostatMode::Heating => {
                    let turn_off = setpoints.heating_c + hysteresis * offset;
                    if zone_temp > turn_off {
                        ThermostatMode::Deadband
                    } else {
                        ThermostatMode::Heating
                    }
                }
                ThermostatMode::Cooling => {
                    let turn_off = setpoints.cooling_c - hysteresis * offset;
                    if zone_temp < turn_off {
                        ThermostatMode::Deadband
                    } else {
                        ThermostatMode::Cooling
                    }
                }
                ThermostatMode::Deadband => {
                    let heat_turn_on = setpoints.heating_c - hysteresis * (1.0 - offset);
                    let cool_turn_on = setpoints.cooling_c + hysteresis * (1.0 - offset);
                    if zone_temp < heat_turn_on {
                        ThermostatMode::Heating
                    } else if zone_temp > cool_turn_on {
                        ThermostatMode::Cooling
                    } else {
                        ThermostatMode::Deadband
                    }
                }
            }
        } else {
            match self.mode {
                ThermostatMode::Heating => {
                    if zone_temp > setpoints.heating_c + hysteresis * cutout {
                        ThermostatMode::Deadband
                    } else {
                        ThermostatMode::Heating
                    }
                }
                ThermostatMode::Cooling => {
                    if zone_temp < setpoints.cooling_c - hysteresis * cutout {
                        ThermostatMode::Deadband
                    } else {
                        ThermostatMode::Cooling
                    }
                }
                ThermostatMode::Deadband => {
                    if zone_temp < setpoints.heating_c - hysteresis {
                        ThermostatMode::Heating
                    } else if zone_temp > setpoints.cooling_c + hysteresis {
                        ThermostatMode::Cooling
                    } else {
                        ThermostatMode::Deadband
                    }
                }
            }
        };

        tracing::debug!(
            current_mode = ?self.mode,
            next_mode = ?next_mode,
            offset,
            hysteresis_c = hysteresis,
            "update_mode: computed next_mode"
        );

        if !self.can_transition_mode(next_mode, env.current_time) {
            tracing::debug!(
                current_mode = ?self.mode,
                proposed_mode = ?next_mode,
                elapsed_s = self
                    .mode_start_at
                    .map(|t| (env.current_time - t).num_milliseconds().max(0) as f64 / 1000.0)
                    .unwrap_or(0.0),
                min_on_time_s = self.min_on_time_s,
                min_off_time_s = self.min_off_time_s,
                "can_transition_mode: blocked by min on/off time"
            );
            return Ok(self.mode);
        }

        self.set_mode(next_mode, env.current_time);
        Ok(self.mode)
    }

    /// Apply `ThermalSetpoint` and `ThermalSetpointDelta` control signals.
    ///
    /// Returns `true` if the signal was handled, `false` if it was an unrelated
    /// signal type. Equipment-specific validation (e.g. `IdealHvac`'s
    /// `validate_runtime_override`) and deadband updates must be done by the
    /// caller before/after invoking this method.
    pub fn apply_thermal_setpoint_signal(&mut self, signal: &hares_types::ControlSignal) -> bool {
        use hares_types::ControlSignal;
        match signal {
            ControlSignal::ThermalSetpoint {
                heating_setpoint_c,
                cooling_setpoint_c,
                ..
            } => {
                self.runtime_setpoints = Some(RuntimeSetpointOverride {
                    heating_c: *heating_setpoint_c,
                    cooling_c: *cooling_setpoint_c,
                });
                true
            }
            ControlSignal::ThermalSetpointDelta {
                heating_delta_c,
                cooling_delta_c,
            } => {
                let base = self
                    .static_setpoints
                    .with_schedule_override(self.schedule_setpoints);
                let prior = self.runtime_setpoints.unwrap_or_default();
                self.runtime_setpoints = Some(RuntimeSetpointOverride {
                    heating_c: heating_delta_c
                        .map(|d| base.heating_c + d)
                        .or(prior.heating_c),
                    cooling_c: cooling_delta_c
                        .map(|d| base.cooling_c + d)
                        .or(prior.cooling_c),
                });
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::{FixedOffset, TimeZone};

    use super::*;

    fn thermostat_with_cycle_time(min_cycle_time_s: f64) -> ThermostatConfig {
        let mut cfg = ThermostatConfig::default();
        cfg.min_cycle_time_s = min_cycle_time_s;
        cfg
    }

    fn utc_time(day: u32, hour: u32) -> DateTime<FixedOffset> {
        FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2018, 1, day, hour, 0, 0)
            .unwrap()
    }

    #[test]
    fn cycle_change_allowed_when_cycle_time_is_zero() {
        let tstat = ThermostatConfig::default(); // min_cycle_time_s = 0.0
        // Should always allow regardless of timing
        assert!(is_cycle_change_allowed(&tstat, None, utc_time(15, 0)));
        assert!(is_cycle_change_allowed(&tstat, Some(utc_time(15, 0)), utc_time(15, 1)));
    }

    #[test]
    fn cycle_change_allowed_when_no_previous_switch() {
        let tstat = thermostat_with_cycle_time(60.0);
        assert!(is_cycle_change_allowed(&tstat, None, utc_time(15, 0)));
    }

    #[test]
    fn cycle_change_blocked_within_min_cycle_time() {
        let tstat = thermostat_with_cycle_time(300.0); // 5 minutes
        // Last switch 1 minute ago
        assert!(!is_cycle_change_allowed(
            &tstat,
            Some(utc_time(15, 0)),
            utc_time(15, 0) + chrono::Duration::minutes(1),
        ));
    }

    #[test]
    fn cycle_change_allowed_after_min_cycle_time() {
        let tstat = thermostat_with_cycle_time(300.0);
        // Last switch 6 minutes ago
        assert!(is_cycle_change_allowed(
            &tstat,
            Some(utc_time(15, 0)),
            utc_time(15, 0) + chrono::Duration::minutes(6),
        ));
    }

    #[test]
    fn cycle_change_allowed_after_clock_reset() {
        // Warmup resets clock.current_step = 0, taking current_time back
        // to the start of the day while last_mode_switch_at is a later
        // wall-clock time from the previous warmup iteration.
        let tstat = thermostat_with_cycle_time(60.0);
        // Last switch at 3pm on day 15, now is 12am on day 15 (clock reset)
        assert!(is_cycle_change_allowed(
            &tstat,
            Some(utc_time(15, 15)),
            utc_time(15, 0),
        ));
    }

    #[test]
    fn cycle_change_allowed_exactly_at_boundary() {
        let tstat = thermostat_with_cycle_time(60.0);
        assert!(is_cycle_change_allowed(
            &tstat,
            Some(utc_time(15, 0)),
            utc_time(15, 0) + chrono::Duration::seconds(60),
        ));
    }
}
