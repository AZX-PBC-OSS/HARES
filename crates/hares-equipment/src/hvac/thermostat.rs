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

    pub fn reconcile_for_deadband(self, hysteresis_c: f64) -> Self {
        let min_gap = 2.0 * hysteresis_c;
        let gap = self.cooling_c - self.heating_c;
        if gap < min_gap {
            let avg = 0.5 * (self.heating_c + self.cooling_c);
            let half = 0.5 * min_gap;
            tracing::warn!(
                original_heating_c = self.heating_c,
                original_cooling_c = self.cooling_c,
                gap_c = gap,
                required_gap_c = min_gap,
                new_heating_c = avg - half,
                new_cooling_c = avg + half,
                "setpoints too close or inverted; reconciled to midpoint with {} C separation",
                min_gap,
            );
            Self {
                heating_c: avg - half,
                cooling_c: avg + half,
            }
        } else {
            self
        }
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
    /// Count of setpoint overrides that violated the deadband constraint
    /// and were auto-corrected. Gated on `observe` feature for
    /// diagnostic CSV output.
    pub setpoint_violation_count: u64,
    /// Count of deadband collisions where `heat_turn_on >= cool_turn_on`
    /// in `update_mode()`. Gated on `observe` for diagnostic CSV output.
    #[cfg(feature = "observe")]
    pub deadband_collision_count: u64,
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
            setpoint_violation_count: 0,
            #[cfg(feature = "observe")]
            deadband_collision_count: 0,
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
        let heat_turn_on = setpoints.heating_c - hysteresis * (1.0 - offset);
        let cool_turn_on = setpoints.cooling_c + hysteresis * (1.0 - offset);
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
                    if heat_turn_on >= cool_turn_on {
                        #[cfg(feature = "observe")]
                        {
                            self.deadband_collision_count =
                                self.deadband_collision_count.saturating_add(1);
                        }
                        tracing::warn!(
                            heat_turn_on,
                            cool_turn_on,
                            zone_temp,
                            "deadband collision: heat_turn_on >= cool_turn_on; returning Deadband as safe default"
                        );
                        ThermostatMode::Deadband
                    } else {
                        #[cfg(any(debug_assertions, feature = "check_invariants"))]
                        if heat_turn_on >= cool_turn_on {
                            return Err(HaresError::InvariantViolation {
                                check_name: "thermostat_deadband_turn_on_order".to_string(),
                                value: heat_turn_on,
                                tolerance: cool_turn_on,
                            });
                        }

                        if zone_temp < heat_turn_on {
                            ThermostatMode::Heating
                        } else if zone_temp > cool_turn_on {
                            ThermostatMode::Cooling
                        } else {
                            ThermostatMode::Deadband
                        }
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
                    if heat_turn_on >= cool_turn_on {
                        #[cfg(feature = "observe")]
                        {
                            self.deadband_collision_count =
                                self.deadband_collision_count.saturating_add(1);
                        }
                        tracing::warn!(
                            heat_turn_on,
                            cool_turn_on,
                            zone_temp,
                            "deadband collision: heat_turn_on >= cool_turn_on; returning Deadband as safe default"
                        );
                        ThermostatMode::Deadband
                    } else {
                        #[cfg(any(debug_assertions, feature = "check_invariants"))]
                        if heat_turn_on >= cool_turn_on {
                            return Err(HaresError::InvariantViolation {
                                check_name: "thermostat_deadband_turn_on_order".to_string(),
                                value: heat_turn_on,
                                tolerance: cool_turn_on,
                            });
                        }

                        if zone_temp < heat_turn_on {
                            ThermostatMode::Heating
                        } else if zone_temp > cool_turn_on {
                            ThermostatMode::Cooling
                        } else {
                            ThermostatMode::Deadband
                        }
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
    /// Returns `Ok(true)` if the signal was handled, `Ok(false)` if it was an
    /// unrelated signal type, or `Err` on a fatal structural error (e.g.
    /// impossible setpoint values).
    ///
    /// Violations of the deadband invariant (`cooling_c <= heating_c +
    /// deadband_c`) are auto-corrected rather than rejected: the cooling
    /// setpoint is raised to `heating_c + deadband_c + 1.0°C` and a warning is
    /// logged on first occurrence. The corrected values are what get stored,
    /// not the raw input.
    ///
    /// The deadband is taken from the `deadband_c` field of
    /// `ThermalSetpoint` when present, otherwise defaults to `2 ×
    /// hysteresis_c` (the thermostat's mechanical deadband).
    pub fn apply_thermal_setpoint_signal(
        &mut self,
        signal: &hares_types::ControlSignal,
    ) -> crate::Result<bool> {
        use hares_types::ControlSignal;
        let hysteresis = self.thermostat.hysteresis_c;
        let base = self
            .static_setpoints
            .with_schedule_override(self.schedule_setpoints);
        let prior = self.runtime_setpoints.unwrap_or_default();

        let mut candidate = match signal {
            ControlSignal::ThermalSetpoint {
                heating_setpoint_c,
                cooling_setpoint_c,
                ..
            } => RuntimeSetpointOverride {
                heating_c: *heating_setpoint_c,
                cooling_c: *cooling_setpoint_c,
            },
            ControlSignal::ThermalSetpointDelta {
                heating_delta_c,
                cooling_delta_c,
            } => RuntimeSetpointOverride {
                heating_c: heating_delta_c
                    .map(|d| base.heating_c + d)
                    .or(prior.heating_c),
                cooling_c: cooling_delta_c
                    .map(|d| base.cooling_c + d)
                    .or(prior.cooling_c),
            },
            _ => return Ok(false),
        };

        // Effective setpoints after merging with the base (schedule + static).
        let effective = base.with_control_override(Some(candidate));

        // Deadband constraint: cooling must exceed heating by at least the
        // deadband. For ThermalSetpoint signals, honour the per-signal
        // deadband_c; for ThermalSetpointDelta, use the thermostat default
        // (2 × hysteresis_c).
        let deadband_c = match signal {
            ControlSignal::ThermalSetpoint {
                deadband_c: Some(d),
                ..
            } => *d,
            _ => 2.0 * hysteresis,
        };

        if effective.cooling_c <= effective.heating_c + deadband_c {
            let raw_heating = effective.heating_c;
            let raw_cooling = effective.cooling_c;
            let corrected_cooling = raw_heating + deadband_c + 1.0;
            let corrected = ThermalSetpoints {
                heating_c: raw_heating,
                cooling_c: corrected_cooling,
            };

            // Store the corrected values by updating the candidate so the
            // RuntimeSetpointOverride reflects the auto-correction.
            candidate.cooling_c = Some(corrected_cooling);

            self.setpoint_violation_count = self.setpoint_violation_count.saturating_add(1);
            if self.setpoint_violation_count == 1 {
                tracing::warn!(
                    raw_heating_c = raw_heating,
                    raw_cooling_c = raw_cooling,
                    deadband_c = deadband_c,
                    corrected_heating_c = corrected.heating_c,
                    corrected_cooling_c = corrected.cooling_c,
                    "auto-corrected setpoint override: cooling-heating gap violated deadband; \
                     cooling raised to heating + deadband + 1.0°C",
                );
            }

            #[cfg(any(debug_assertions, feature = "check_invariants"))]
            if corrected.cooling_c <= corrected.heating_c + deadband_c {
                return Err(HaresError::InvariantViolation {
                    check_name: "thermostat_auto_corrected_setpoint_deadband".to_string(),
                    value: corrected.cooling_c - corrected.heating_c,
                    tolerance: deadband_c,
                });
            }
        }

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            let final_effective = base.with_control_override(Some(candidate));
            // Deadband to check against: for explicit-deadband signals this
            // may differ from 2 × hysteresis. The auto-correction guarantees
            // cooling > heating + deadband, so we assert that here.
            let check_deadband = match signal {
                ControlSignal::ThermalSetpoint {
                    deadband_c: Some(d),
                    ..
                } => *d,
                _ => 2.0 * hysteresis,
            };
            if final_effective.cooling_c <= final_effective.heating_c + check_deadband {
                return Err(HaresError::InvariantViolation {
                    check_name: "thermostat_final_setpoint_deadband".to_string(),
                    value: final_effective.cooling_c - final_effective.heating_c,
                    tolerance: check_deadband,
                });
            }
        }

        self.runtime_setpoints = Some(candidate);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};

    use super::*;
    use hares_types::{GridState, WeatherState, ZoneState};

    fn thermostat_with_cycle_time(min_cycle_time_s: f64) -> ThermostatConfig {
        ThermostatConfig {
            min_cycle_time_s,
            ..ThermostatConfig::default()
        }
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
        assert!(is_cycle_change_allowed(
            &tstat,
            Some(utc_time(15, 0)),
            utc_time(15, 1)
        ));
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

    #[test]
    fn apply_thermal_setpoint_signal_autocorrects_deadband_violation() {
        let mut fsm = ThermostatFsm::new(ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 24.0,
        });
        // Default hysteresis_c = 1.0 → deadband = 2.0.
        // heating=23.0, cooling=24.0 → gap=1.0 < 2.0 → auto-corrected.
        let result =
            fsm.apply_thermal_setpoint_signal(&hares_types::ControlSignal::ThermalSetpoint {
                heating_setpoint_c: Some(23.0),
                cooling_setpoint_c: Some(24.0),
                deadband_c: None,
            });
        assert!(result.is_ok());
        assert!(result.unwrap());
        // Cooling auto-corrected to heating + deadband + 1.0 = 23 + 2 + 1 = 26.0
        let stored = fsm.runtime_setpoints.unwrap();
        assert_eq!(stored.heating_c, Some(23.0));
        assert_eq!(stored.cooling_c, Some(26.0));
    }

    #[test]
    fn apply_thermal_setpoint_signal_autocorrects_inverted_heating_cooling() {
        let mut fsm = ThermostatFsm::new(ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 24.0,
        });
        // heating=30.0, cooling=25.0 → physically impossible inversion.
        // deadband = 2.0 → auto-corrected cooling = 30 + 2 + 1 = 33.0
        let result =
            fsm.apply_thermal_setpoint_signal(&hares_types::ControlSignal::ThermalSetpoint {
                heating_setpoint_c: Some(30.0),
                cooling_setpoint_c: Some(25.0),
                deadband_c: None,
            });
        assert!(result.is_ok());
        assert!(result.unwrap());
        let stored = fsm.runtime_setpoints.unwrap();
        assert_eq!(stored.heating_c, Some(30.0));
        assert_eq!(stored.cooling_c, Some(33.0));
    }

    #[test]
    fn apply_thermal_setpoint_delta_autocorrects_resulting_deadband_violation() {
        let mut fsm = ThermostatFsm::new(ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 24.0,
        });
        // Schedule setpoint narrows the gap: heating=22, cooling=23
        fsm.schedule_setpoints = Some(ScheduleSetpoints {
            heating_c: Some(22.0),
            cooling_c: Some(23.0),
            ..ScheduleSetpoints::default()
        });
        // Delta pushes heating up by 1.0 → effective: heating=23, cooling=23 → gap=0 < 2.0
        // deadband = 2.0 → auto-corrected cooling = 23 + 2 + 1 = 26.0
        let result =
            fsm.apply_thermal_setpoint_signal(&hares_types::ControlSignal::ThermalSetpointDelta {
                heating_delta_c: Some(1.0),
                cooling_delta_c: None,
            });
        assert!(result.is_ok());
        assert!(result.unwrap());
        let stored = fsm.runtime_setpoints.unwrap();
        // heating_delta + base = 1.0 + 22.0 = 23.0 (from effective)
        // But candidate heating is: delta + base = 1.0 + 22.0 = 23.0
        // cooling is auto-corrected: 23 + 2 + 1 = 26.0
        assert_eq!(stored.heating_c, Some(23.0));
        assert_eq!(stored.cooling_c, Some(26.0));
    }

    #[test]
    fn apply_thermal_setpoint_signal_uses_explicit_deadband() {
        let mut fsm = ThermostatFsm::new(ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 24.0,
        });
        // Explicit deadband_c=4.0 overrides hysteresis default.
        // heating=23.0, cooling=26.0 → gap=3.0 < 4.0 → auto-corrected.
        // corrected cooling = 23 + 4 + 1 = 28.0
        let result =
            fsm.apply_thermal_setpoint_signal(&hares_types::ControlSignal::ThermalSetpoint {
                heating_setpoint_c: Some(23.0),
                cooling_setpoint_c: Some(26.0),
                deadband_c: Some(4.0),
            });
        assert!(result.is_ok());
        assert!(result.unwrap());
        let stored = fsm.runtime_setpoints.unwrap();
        assert_eq!(stored.heating_c, Some(23.0));
        assert_eq!(stored.cooling_c, Some(28.0));
    }

    #[test]
    fn apply_thermal_setpoint_signal_autocorrects_with_zero_deadband() {
        let mut fsm = ThermostatFsm::new(ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 24.0,
        });
        // deadband_c=0 → enforce strict cooling > heating.
        // heating=25.0, cooling=25.0 → 25 <= 25 + 0 → violation.
        // corrected cooling = 25 + 0 + 1 = 26.0
        let result =
            fsm.apply_thermal_setpoint_signal(&hares_types::ControlSignal::ThermalSetpoint {
                heating_setpoint_c: Some(25.0),
                cooling_setpoint_c: Some(25.0),
                deadband_c: Some(0.0),
            });
        assert!(result.is_ok());
        assert!(result.unwrap());
        let stored = fsm.runtime_setpoints.unwrap();
        assert_eq!(stored.heating_c, Some(25.0));
        assert_eq!(stored.cooling_c, Some(26.0));
    }

    #[test]
    fn apply_thermal_setpoint_signal_autocorrects_cooling_only_override() {
        let mut fsm = ThermostatFsm::new(ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 24.0,
        });
        // Only cooling is set to 21.0 → effective: heating=20.0, cooling=21.0.
        // gap = 1.0 < 2.0 (default deadband) → auto-corrected.
        // corrected cooling = 20 + 2 + 1 = 23.0
        let result =
            fsm.apply_thermal_setpoint_signal(&hares_types::ControlSignal::ThermalSetpoint {
                heating_setpoint_c: None,
                cooling_setpoint_c: Some(21.0),
                deadband_c: None,
            });
        assert!(result.is_ok());
        assert!(result.unwrap());
        let stored = fsm.runtime_setpoints.unwrap();
        assert_eq!(stored.heating_c, None);
        assert_eq!(stored.cooling_c, Some(23.0));
    }

    #[test]
    fn apply_thermal_setpoint_signal_accepts_valid_setpoints() {
        let mut fsm = ThermostatFsm::new(ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 24.0,
        });
        let result =
            fsm.apply_thermal_setpoint_signal(&hares_types::ControlSignal::ThermalSetpoint {
                heating_setpoint_c: Some(18.0),
                cooling_setpoint_c: Some(22.0),
                deadband_c: None,
            });
        assert!(result.is_ok());
        assert!(result.unwrap()); // signal was handled
        assert_eq!(fsm.runtime_setpoints.unwrap().heating_c, Some(18.0));
        assert_eq!(fsm.runtime_setpoints.unwrap().cooling_c, Some(22.0));
    }

    #[test]
    fn apply_thermal_setpoint_signal_returns_false_for_unrelated_signal() {
        let mut fsm = ThermostatFsm::new(ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 24.0,
        });
        let result = fsm.apply_thermal_setpoint_signal(&hares_types::ControlSignal::DutyCycle {
            on_fraction: 1.0,
            period_s: None,
            component: None,
        });
        assert!(result.is_ok());
        assert!(!result.unwrap()); // signal was NOT handled
    }

    fn env_with_zone_temp(temp_c: f64) -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: temp_c,
                humidity_ratio: 0.008,
                volume_m3: 200.0,
            }],
            weather: WeatherState::default(),
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
                island_bus_voltage_pu: None,
            },
            custom_domains: vec![],
            equipment_telemetry: std::collections::HashMap::new(),
            equipment_core: Default::default(),
            current_time: FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(2026, 1, 15, 12, 0, 0)
                .unwrap(),
            time_res: ChronoDuration::seconds(60),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }

    #[test]
    fn update_mode_returns_deadband_on_collapsed_deadband() {
        let mut fsm = ThermostatFsm::new(ThermalSetpoints {
            heating_c: 25.0,
            cooling_c: 21.0,
        });
        fsm.thermostat = ThermostatConfig::default();

        // offset=0.2 (default): heat_turn_on = 25.0 - 1.0*(1.0-0.2) = 24.2
        // cool_turn_on = 21.0 + 1.0*(1.0-0.2) = 21.8
        // heat_turn_on(24.2) >= cool_turn_on(21.8) → collision → Deadband
        let env = env_with_zone_temp(23.0);
        let mode = fsm.update_mode(&env, ZoneId(1)).unwrap();
        assert_eq!(mode, ThermostatMode::Deadband);
    }

    #[test]
    fn update_mode_returns_heating_with_valid_deadband() {
        let mut fsm = ThermostatFsm::new(ThermalSetpoints {
            heating_c: 21.0,
            cooling_c: 23.0,
        });
        fsm.thermostat = ThermostatConfig::default();

        // offset=0.2 (default): heat_turn_on = 21.0 - 1.0*(1.0-0.2) = 20.2
        // cool_turn_on = 23.0 + 1.0*(1.0-0.2) = 23.8
        // heat_turn_on(20.2) < cool_turn_on(23.8), zone_temp=19.0 < 20.2 → Heating
        let env = env_with_zone_temp(19.0);
        let mode = fsm.update_mode(&env, ZoneId(1)).unwrap();
        assert_eq!(mode, ThermostatMode::Heating);
    }

    #[test]
    fn update_mode_returns_deadband_on_runtime_inverted_setpoints() {
        let mut fsm = ThermostatFsm::new(ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 24.0,
        });
        fsm.thermostat = ThermostatConfig::default();
        // Simulate inverted setpoints that bypassed deadband validation
        // (e.g. through a bug in the dispatch chain as described in T-0156/T-0157).
        fsm.runtime_setpoints = Some(RuntimeSetpointOverride {
            heating_c: Some(25.0),
            cooling_c: Some(21.0),
        });
        // offset=0.2 (default): heat_turn_on = 25.0 - 1.0*(1.0-0.2) = 24.2
        // cool_turn_on = 21.0 + 1.0*(1.0-0.2) = 21.8
        // heat_turn_on(24.2) >= cool_turn_on(21.8) → collision → Deadband
        let env = env_with_zone_temp(23.0);
        let mode = fsm.update_mode(&env, ZoneId(1)).unwrap();
        assert_eq!(mode, ThermostatMode::Deadband);
    }

    #[test]
    fn update_mode_collapsed_deadband_with_offset_does_not_mask_as_heating() {
        let mut fsm = ThermostatFsm::new(ThermalSetpoints {
            heating_c: 25.0,
            cooling_c: 21.0,
        });
        let config = ThermostatConfig {
            deadband_offset: 0.2,
            ..Default::default()
        };
        fsm.thermostat = config;

        // heat_turn_on = 25.0 - 1.0*(1.0-0.2) = 25.0 - 0.8 = 24.2
        // cool_turn_on = 21.0 + 1.0*(1.0-0.2) = 21.0 + 0.8 = 21.8
        // heat_turn_on >= cool_turn_on → collision → Deadband
        let env = env_with_zone_temp(23.0);
        let mode = fsm.update_mode(&env, ZoneId(1)).unwrap();
        assert_eq!(mode, ThermostatMode::Deadband);
    }
}
