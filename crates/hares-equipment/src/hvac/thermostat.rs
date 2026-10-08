//! Thermostat types and FSM logic shared across HVAC equipment.

use chrono::{DateTime, FixedOffset};
use hares_types::{
    ControlSignal, EnvironmentState, HaresError, ScheduleSource, ThermostatBandClass, ZoneId,
    thermal_setpoint_band_c, validate_thermostat_band_c,
};
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
    /// The thermostat class `hysteresis_c` is held to: a cycling unit's
    /// thermostat, or an ideal controller with no band of its own.
    pub band_class: ThermostatBandClass,
}

impl Default for ThermostatConfig {
    fn default() -> Self {
        Self {
            hysteresis_c: 1.0,
            cutout_ratio: 0.0,
            min_cycle_time_s: 0.0,
            use_ideal_capacity: false,
            deadband_offset: 0.2,
            band_class: ThermostatBandClass::Cycling,
        }
    }
}

impl ThermostatConfig {
    /// The configured input the band comes from: an ideal unit's
    /// `deadband_c`, a cycling unit's `hysteresis_c`.
    fn band_field(&self) -> &'static str {
        match self.band_class {
            ThermostatBandClass::Ideal => "deadband_c",
            ThermostatBandClass::Cycling | ThermostatBandClass::Tank => "hysteresis_c",
        }
    }

    /// Checks a band for this thermostat's class.
    fn validate_band(&self, band_c: f64) -> crate::Result<()> {
        validate_thermostat_band_c(self.band_class, self.band_field(), band_c)
    }

    pub fn validate(&mut self, env: &EnvironmentState) -> crate::Result<()> {
        if !(0.0..=1.0).contains(&self.cutout_ratio) {
            return Err(HaresError::Equipment(format!(
                "cutout_ratio must be in [0.0, 1.0], got {}",
                self.cutout_ratio
            )));
        }
        self.validate_band(self.hysteresis_c)?;
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
/// and [`IdealHvac`](super::ideal_hvac::IdealHvac).
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
    /// Minimum time (s) compressor must remain On before an Off transition is
    /// allowed. Prevents short-cycle wear. 0.0 = disabled (default).
    pub min_on_time_s: f64,
    /// Minimum time (s) compressor must remain Off before an On transition is
    /// allowed. Prevents short-cycle wear. 0.0 = disabled (default).
    pub min_off_time_s: f64,
    /// Count of setpoint overrides whose unnamed axis was moved to keep the
    /// required deadband from the named one.
    pub setpoint_violation_count: u64,
    /// The configured band, which the release form restores after a named
    /// signal set the band for an event.
    configured_hysteresis_c: f64,
    /// Count of deadband collisions where `heat_turn_on >= cool_turn_on`
    /// in `update_mode()`. Gated on `observe` for diagnostic CSV output.
    #[cfg(feature = "observe")]
    pub deadband_collision_count: u64,
}

impl ThermostatFsm {
    /// The thermostat's turn-on/turn-off threshold temperatures per axis,
    /// the thresholds `update_mode_at` switches on (thermostat.rs:432-435).
    /// Returns `((heat_turn_on, heat_turn_off), (cool_turn_on, cool_turn_off))`.
    pub fn band_edges(&self) -> ((f64, f64), (f64, f64)) {
        let setpoints = self.effective_setpoints();
        let hysteresis = self.thermostat.hysteresis_c;
        // `deadband_offset` is validated at thermostat construction
        // (`ThermostatConfig::validate`), so no clamp is needed here.
        let offset = self.thermostat.deadband_offset;
        (
            (
                setpoints.heating_c - hysteresis * (1.0 - offset),
                setpoints.heating_c + hysteresis * offset,
            ),
            (
                setpoints.cooling_c + hysteresis * (1.0 - offset),
                setpoints.cooling_c - hysteresis * offset,
            ),
        )
    }

    /// The band a cycling unit's duty fraction modulates over, per axis:
    /// `(zero_delivery_edge, full_delivery_edge)`.
    ///
    /// The zero-delivery edge is the axis setpoint, the same target the
    /// ideal path's solve holds the zone to, not the FSM's release edge:
    /// the release edge is the latch boundary, and anchoring the fraction
    /// there biases the controller's settling point a `deadband_offset`
    /// past the setpoint, toward over-delivery. The full-delivery edge
    /// stays the call edge. `cycling_load_fraction` applies the minimum
    /// span floor itself.
    pub fn duty_bands(&self) -> ((f64, f64), (f64, f64)) {
        let setpoints = self.effective_setpoints();
        let ((heat_on, _), (cool_on, _)) = self.band_edges();
        (
            (setpoints.heating_c, heat_on),
            (setpoints.cooling_c, cool_on),
        )
    }

    pub fn new(static_setpoints: ThermalSetpoints) -> Self {
        let thermostat = ThermostatConfig::default();
        Self {
            mode: ThermostatMode::Deadband,
            mode_start_at: None,
            last_mode_switch_at: None,
            configured_hysteresis_c: thermostat.hysteresis_c,
            thermostat,
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

    /// Validates the configured thermostat and records its band as the one
    /// the release form restores. Owners call it at init, after setting
    /// `thermostat` from their configuration.
    pub fn validate_configuration(&mut self, env: &EnvironmentState) -> crate::Result<()> {
        self.thermostat.validate(env)?;
        self.configured_hysteresis_c = self.thermostat.hysteresis_c;
        Ok(())
    }

    /// Restores a checkpointed band, held to the thermostat's class like
    /// any other source of it.
    pub fn restore_hysteresis(&mut self, hysteresis_c: f64) -> crate::Result<()> {
        self.thermostat.validate_band(hysteresis_c)?;
        self.thermostat.hysteresis_c = hysteresis_c;
        Ok(())
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
    ///
    /// A source whose read fails is an error: a configured schedule the
    /// step cannot read is corrupt input, not the absence of a schedule.
    pub fn resolve_profile_setpoints(&mut self, env: &EnvironmentState) -> crate::Result<()> {
        if self.heating_setpoint_source.is_none() && self.cooling_setpoint_source.is_none() {
            return Ok(());
        }

        let heating_c = match self.heating_setpoint_source.as_mut() {
            Some(source) => Some(source.value_at(env)?),
            None => None,
        };
        let cooling_c = match self.cooling_setpoint_source.as_mut() {
            Some(source) => Some(source.value_at(env)?),
            None => None,
        };

        if heating_c.is_some() || cooling_c.is_some() {
            self.schedule_setpoints = Some(ScheduleSetpoints {
                heating_c,
                cooling_c,
                ..ScheduleSetpoints::default()
            });
        } else {
            self.schedule_setpoints = None;
        }
        Ok(())
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
        let zone_temp = lookup_zone_temp(env, zone_id)?;
        self.update_mode_at(env, zone_temp)
    }

    /// [`Self::update_mode`] at a zone temperature the caller already read.
    ///
    /// A setpoint source whose read fails is an error; the caller names the
    /// equipment that owns the source.
    pub fn update_mode_at(
        &mut self,
        env: &EnvironmentState,
        zone_temp: f64,
    ) -> crate::Result<ThermostatMode> {
        self.resolve_profile_setpoints(env)?;
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
    /// Returns `Ok(true)` if the signal was handled and `Ok(false)` for an
    /// unrelated signal type. Every check runs before any state changes, so
    /// a rejected signal leaves the thermostat as it was.
    ///
    /// A signal that names no axis (the `ThermalSetpoint` release form, or a
    /// delta carrying neither delta) is stored as is, with no gap check: the
    /// release hands both axes back to the schedule and the band back to its
    /// configured value. Otherwise the effective cooling-heating gap must be
    /// at least the required deadband: the signal's `deadband_c` when it
    /// carries one, else `2 × hysteresis_c`. A named axis is never moved.
    /// When one axis is named, a gap violation moves the other one away from
    /// it, as a thermostat in auto mode pushes the opposite setpoint, and is
    /// logged; when both are named, it is rejected. A `ThermalSetpoint`
    /// deadband, held to the thermostat's class, becomes the hysteresis for
    /// the event, until another named band or the release replaces it.
    pub fn apply_thermal_setpoint_signal(&mut self, signal: &ControlSignal) -> crate::Result<bool> {
        let base = self
            .static_setpoints
            .with_schedule_override(self.schedule_setpoints);
        let prior = self.runtime_setpoints.unwrap_or_default();

        let (mut candidate, band_c, heating_named, cooling_named) = match signal {
            ControlSignal::ThermalSetpoint {
                heating_setpoint_c,
                cooling_setpoint_c,
                deadband_c,
            } => (
                RuntimeSetpointOverride {
                    heating_c: *heating_setpoint_c,
                    cooling_c: *cooling_setpoint_c,
                },
                thermal_setpoint_band_c(
                    self.thermostat.band_class,
                    *heating_setpoint_c,
                    *cooling_setpoint_c,
                    *deadband_c,
                )?,
                heating_setpoint_c.is_some(),
                cooling_setpoint_c.is_some(),
            ),
            ControlSignal::ThermalSetpointDelta {
                heating_delta_c,
                cooling_delta_c,
            } => (
                RuntimeSetpointOverride {
                    heating_c: heating_delta_c
                        .map(|d| base.heating_c + d)
                        .or(prior.heating_c),
                    cooling_c: cooling_delta_c
                        .map(|d| base.cooling_c + d)
                        .or(prior.cooling_c),
                },
                None,
                heating_delta_c.is_some(),
                cooling_delta_c.is_some(),
            ),
            _ => return Ok(false),
        };

        if heating_named || cooling_named {
            let required_gap_c = band_c.unwrap_or(2.0 * self.thermostat.hysteresis_c);
            let effective = base.with_control_override(Some(candidate));
            let gap_c = effective.cooling_c - effective.heating_c;
            if gap_c < required_gap_c {
                match (heating_named, cooling_named) {
                    (true, true) => {
                        return Err(HaresError::Control(format!(
                            "{signal:?} sets heating {} °C and cooling {} °C: the gap {gap_c} °C \
                             is below the required deadband {required_gap_c} °C",
                            effective.heating_c, effective.cooling_c
                        )));
                    }
                    (true, false) => {
                        candidate.cooling_c = Some(effective.heating_c + required_gap_c);
                    }
                    (false, _) => {
                        candidate.heating_c = Some(effective.cooling_c - required_gap_c);
                    }
                }
                self.setpoint_violation_count = self.setpoint_violation_count.saturating_add(1);
                tracing::warn!(
                    heating_c = effective.heating_c,
                    cooling_c = effective.cooling_c,
                    required_gap_c,
                    pushed_heating_c = ?candidate.heating_c,
                    pushed_cooling_c = ?candidate.cooling_c,
                    "setpoint override closer than the required deadband: the unnamed \
                     setpoint was moved away from the named one",
                );
            }
        }

        let is_release = matches!(signal, ControlSignal::ThermalSetpoint { .. })
            && !heating_named
            && !cooling_named;
        if let Some(band_c) = band_c {
            self.thermostat.hysteresis_c = band_c;
        } else if is_release {
            self.thermostat.hysteresis_c = self.configured_hysteresis_c;
        }
        self.runtime_setpoints = Some(candidate);
        Ok(true)
    }
}

#[cfg(test)]
pub(super) mod tests {
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

    fn fsm_20_24() -> ThermostatFsm {
        ThermostatFsm::new(ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 24.0,
        })
    }

    fn thermal_setpoint(
        heating_setpoint_c: Option<f64>,
        cooling_setpoint_c: Option<f64>,
        deadband_c: Option<f64>,
    ) -> hares_types::ControlSignal {
        hares_types::ControlSignal::ThermalSetpoint {
            heating_setpoint_c,
            cooling_setpoint_c,
            deadband_c,
        }
    }

    /// Asserts a rejected signal left the override and the hysteresis as
    /// they were.
    fn assert_rejected_without_state_change(
        fsm: &mut ThermostatFsm,
        signal: &hares_types::ControlSignal,
    ) -> HaresError {
        let before = fsm.clone();
        let err = fsm
            .apply_thermal_setpoint_signal(signal)
            .expect_err("the signal must be rejected");
        assert_eq!(*fsm, before, "a rejected signal must change no state");
        err
    }

    #[test]
    fn both_named_setpoints_violating_the_gap_are_rejected() {
        let mut fsm = fsm_20_24();
        // Gap 1 against the required 2 x hysteresis (2), and an inversion.
        for (h, c) in [(23.0, 24.0), (30.0, 25.0)] {
            let err = assert_rejected_without_state_change(
                &mut fsm,
                &thermal_setpoint(Some(h), Some(c), None),
            );
            assert!(err.to_string().contains("gap"), "{err}");
        }
        let err = assert_rejected_without_state_change(
            &mut fsm,
            &hares_types::ControlSignal::ThermalSetpointDelta {
                heating_delta_c: Some(2.0),
                cooling_delta_c: Some(-2.0),
            },
        );
        assert!(err.to_string().contains("gap"), "{err}");
    }

    #[test]
    fn a_gap_equal_to_the_required_deadband_is_accepted_unchanged() {
        let mut fsm = fsm_20_24();
        fsm.apply_thermal_setpoint_signal(&thermal_setpoint(Some(21.0), Some(23.0), None))
            .unwrap();
        assert_eq!(
            fsm.runtime_setpoints,
            Some(RuntimeSetpointOverride {
                heating_c: Some(21.0),
                cooling_c: Some(23.0),
            })
        );
    }

    #[test]
    fn a_named_heating_setpoint_pushes_the_unnamed_cooling_setpoint() {
        let mut fsm = fsm_20_24();
        fsm.schedule_setpoints = Some(ScheduleSetpoints {
            heating_c: Some(22.0),
            cooling_c: Some(23.0),
            ..ScheduleSetpoints::default()
        });
        // Heating 23 by delta against cooling 23: the named heating stays and
        // the cooling setpoint moves to keep the 2 x hysteresis gap.
        fsm.apply_thermal_setpoint_signal(&hares_types::ControlSignal::ThermalSetpointDelta {
            heating_delta_c: Some(1.0),
            cooling_delta_c: None,
        })
        .unwrap();
        let effective = fsm.effective_setpoints();
        assert_eq!(effective.heating_c, 23.0);
        assert_eq!(effective.cooling_c, 25.0);
        assert_eq!(fsm.setpoint_violation_count, 1);
    }

    #[test]
    fn a_named_cooling_setpoint_is_honoured_and_pushes_heating_down() {
        let mut fsm = fsm_20_24();
        // A pre-cool to 21 against heating 20: the named cooling setpoint
        // holds and heating moves down to keep the gap.
        fsm.apply_thermal_setpoint_signal(&thermal_setpoint(None, Some(21.0), None))
            .unwrap();
        let effective = fsm.effective_setpoints();
        assert_eq!(effective.cooling_c, 21.0);
        assert_eq!(effective.heating_c, 19.0);

        // With a named band, the gap is that band and it becomes the
        // hysteresis.
        let mut fsm = fsm_20_24();
        fsm.apply_thermal_setpoint_signal(&thermal_setpoint(None, Some(21.0), Some(0.5)))
            .unwrap();
        let effective = fsm.effective_setpoints();
        assert_eq!(effective.cooling_c, 21.0);
        assert_eq!(effective.heating_c, 20.0);
        assert_eq!(fsm.thermostat.hysteresis_c, 0.5);
        assert_eq!(fsm.setpoint_violation_count, 0);
    }

    #[test]
    fn named_deadband_below_a_thermostat_band_is_rejected_without_state_change() {
        let mut fsm = fsm_20_24();
        for db in [0.0, -0.0, 5e-324, f64::MIN_POSITIVE, 1e-17, 1e-16, f64::NAN] {
            assert_rejected_without_state_change(
                &mut fsm,
                &thermal_setpoint(Some(21.0), None, Some(db)),
            );
        }
    }

    #[test]
    fn release_form_carrying_a_deadband_is_rejected_without_state_change() {
        let mut fsm = fsm_20_24();
        for db in [0.0, 1.0, 5.0] {
            assert_rejected_without_state_change(&mut fsm, &thermal_setpoint(None, None, Some(db)));
        }
    }

    #[test]
    fn release_form_hands_control_back_without_touching_anything_else() {
        // Base 22.5/23.5 as reconciled at init: 22/24, a gap of exactly
        // 2 x hysteresis.
        let mut fsm = ThermostatFsm::new(
            ThermalSetpoints {
                heating_c: 22.5,
                cooling_c: 23.5,
            }
            .reconcile_for_deadband(ThermostatConfig::default().hysteresis_c),
        );
        fsm.apply_thermal_setpoint_signal(&thermal_setpoint(None, None, None))
            .unwrap();
        let effective = fsm.effective_setpoints();
        assert_eq!(effective.heating_c, 22.0);
        assert_eq!(effective.cooling_c, 24.0);
        assert_eq!(
            fsm.runtime_setpoints,
            Some(RuntimeSetpointOverride::default())
        );
        assert_eq!(fsm.setpoint_violation_count, 0);
    }

    /// The band a named signal sets is part of the event, so the release
    /// hands back the configured band with the schedule.
    #[test]
    fn the_release_restores_the_configured_band() {
        let mut fsm = fsm_20_24();
        let configured = fsm.thermostat.hysteresis_c;
        fsm.apply_thermal_setpoint_signal(&thermal_setpoint(Some(18.0), None, Some(0.25)))
            .unwrap();
        assert_eq!(fsm.thermostat.hysteresis_c, 0.25);
        fsm.apply_thermal_setpoint_signal(&hares_types::ControlSignal::thermal_release())
            .unwrap();
        assert_eq!(fsm.effective_setpoints().heating_c, 20.0);
        assert_eq!(fsm.thermostat.hysteresis_c, configured);
    }

    /// Under a schedule narrower than the required gap, the release installs
    /// no override: it is never pushed like a one-axis signal.
    #[test]
    fn a_release_under_a_narrow_schedule_installs_nothing() {
        let mut fsm = fsm_20_24();
        fsm.schedule_setpoints = Some(ScheduleSetpoints {
            heating_c: Some(22.0),
            cooling_c: Some(23.0),
            ..ScheduleSetpoints::default()
        });
        fsm.apply_thermal_setpoint_signal(&hares_types::ControlSignal::thermal_release())
            .unwrap();
        assert_eq!(
            fsm.runtime_setpoints,
            Some(RuntimeSetpointOverride::default())
        );
        assert_eq!(fsm.setpoint_violation_count, 0);
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

    /// The duty fraction's band anchors the zero-delivery edge at each
    /// axis setpoint (the target the ideal solve holds) and keeps the
    /// call edge as the full-delivery edge.
    #[test]
    fn duty_bands_anchor_the_zero_edge_at_the_setpoints() {
        let fsm = ThermostatFsm::new(ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 24.0,
        });
        // Defaults: hysteresis 1.0 C, offset 0.2. The call edges sit at
        // 19.2 (heating turn-on) and 24.8 (cooling turn-on).
        assert_eq!(fsm.duty_bands(), ((20.0, 19.2), (24.0, 24.8)));
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

    pub(in crate::hvac) fn env_with_zone_temp(temp_c: f64) -> EnvironmentState {
        EnvironmentState {
            ambient_other_space_c: hares_types::AmbientOtherSpaceTemps::default(),
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
            schedule_row: None,
            domains: hares_types::DomainSlots::default(),
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
    fn configured_hysteresis_below_a_thermostat_band_is_rejected() {
        let env = env_with_zone_temp(21.0);
        for hysteresis_c in [0.0, 5e-324, 1e-17, 1e-16, 0.05, f64::NAN, 11.0] {
            let mut config = ThermostatConfig {
                hysteresis_c,
                ..ThermostatConfig::default()
            };
            assert!(config.validate(&env).is_err(), "{hysteresis_c}");
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
