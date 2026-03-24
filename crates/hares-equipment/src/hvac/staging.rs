//! Speed staging, part-load, and capacity interpolation for HVAC equipment.
//!
//! Extracted from `hvac_core.rs` to isolate speed selection algorithms from
//! thermostat control and duct distribution. No zone or duct knowledge here.

use hares_physics::biquadratic::quadratic;

use super::hvac_core::HvacEquipment;
use super::speed_control::{SpeedControlMode, SpeedSelection};
use super::thermostat::ThermostatMode;

/// Default low-speed capacity fraction for two-speed equipment.
pub(super) const DEFAULT_LOW_SPEED_CAPACITY_FRACTION: f64 = 0.5;

/// Default part-load factor degradation coefficient (Cd).
/// AHRI Standard 210/240-2023, S6.6.3 default when no test data available.
pub(super) const DEFAULT_PLF_DEGRADATION_COEFF: f64 = 0.25;

impl HvacEquipment {
    /// Number of discrete speed stages. For single-speed equipment this is 1.
    pub fn n_speed_stages(&self) -> usize {
        match self.speed_control_mode {
            SpeedControlMode::SingleSpeed => 1,
            SpeedControlMode::TwoSpeedSetpoint
            | SpeedControlMode::TwoSpeedTime
            | SpeedControlMode::TwoSpeedAlternating => 2,
            SpeedControlMode::MultiSpeedInterpolated => {
                let caps = self.heating_capacities_w.len().max(self.cooling_capacities_w.len());
                caps.max(1)
            }
            SpeedControlMode::VariableSpeedIdeal => 1,
        }
    }

    /// Dynamically disable or re-enable individual speed stages at runtime.
    ///
    /// If all speeds are disabled, `max_enabled_speed` falls back to the last stage.
    pub fn set_disabled_speeds(&mut self, disabled: &[bool]) {
        let n = self.n_speed_stages();
        self.disabled_speeds.resize(n, false);
        for (i, slot) in self.disabled_speeds.iter_mut().enumerate() {
            *slot = disabled.get(i).copied().unwrap_or(false);
        }
        self.max_enabled_speed = self
            .disabled_speeds
            .iter()
            .enumerate()
            .rev()
            .find(|&(_, d)| !d)
            .map(|(i, _)| i)
            .unwrap_or(n.saturating_sub(1));
    }

    /// Record the current zone temperature for the next step's `TwoSpeedTime` comparison.
    /// Pass `None` when the unit turns off to ensure the next on-cycle starts at low speed.
    pub fn update_prev_zone_temp(&mut self, zone_temp_c: Option<f64>) {
        self.prev_zone_temp_c = zone_temp_c;
    }

    /// Advance the speed-stage timer by `dt_s` seconds.
    pub fn advance_speed_timer(&mut self, dt_s: f64) {
        self.time_at_current_speed_s += dt_s;
    }

    pub fn select_speed(&mut self, load_fraction: f64) -> SpeedSelection {
        self.select_speed_with_zone_temp(load_fraction, None, false)
    }

    /// Speed selection that accepts the current zone temperature and heating
    /// direction for `TwoSpeedTime` mode.
    pub fn select_speed_with_zone_temp(
        &mut self,
        load_fraction: f64,
        zone_temp_c: Option<f64>,
        is_heating: bool,
    ) -> SpeedSelection {
        let load_fraction = load_fraction.clamp(0.0, 1.0);
        let selection = match self.speed_control_mode {
            SpeedControlMode::SingleSpeed => SpeedSelection {
                speed_index: 0,
                part_load_ratio: load_fraction,
                speed_frac: load_fraction,
            },
            SpeedControlMode::TwoSpeedSetpoint => {
                self.select_two_speed_setpoint(load_fraction)
            }
            SpeedControlMode::TwoSpeedTime => {
                self.select_two_speed_time(load_fraction, zone_temp_c, is_heating)
            }
            SpeedControlMode::TwoSpeedAlternating => {
                let desired_index = self.apply_disabled_speeds_two_speed(1);
                if desired_index != self.last_speed_index {
                    self.time_at_current_speed_s = 0.0;
                }
                SpeedSelection {
                    speed_index: desired_index,
                    part_load_ratio: load_fraction,
                    speed_frac: 1.0,
                }
            }
            SpeedControlMode::MultiSpeedInterpolated => {
                self.select_multi_speed(load_fraction)
            }
            SpeedControlMode::VariableSpeedIdeal => SpeedSelection {
                speed_index: 0,
                part_load_ratio: 1.0,
                speed_frac: load_fraction,
            },
        };
        self.last_speed_index = selection.speed_index;
        self.last_speed_frac = selection.speed_frac;
        selection
    }

    fn select_two_speed_setpoint(&mut self, load_fraction: f64) -> SpeedSelection {
        let low_cap = self.low_speed_capacity_fraction.clamp(0.01, 0.999);
        let desired_index = if load_fraction > low_cap { 1 } else { 0 };
        let desired_index = self.apply_disabled_speeds_two_speed(desired_index);
        let locked = self.time_at_current_speed_s < self.min_time_per_speed_s
            && desired_index != self.last_speed_index;
        let speed_index = if locked {
            self.last_speed_index
        } else {
            if desired_index != self.last_speed_index {
                self.time_at_current_speed_s = 0.0;
            }
            desired_index
        };
        if speed_index == 1 {
            SpeedSelection { speed_index: 1, part_load_ratio: load_fraction, speed_frac: 1.0 }
        } else {
            SpeedSelection {
                speed_index: 0,
                part_load_ratio: (load_fraction / low_cap).clamp(0.0, 1.0),
                speed_frac: low_cap,
            }
        }
    }

    fn select_two_speed_time(
        &mut self,
        load_fraction: f64,
        zone_temp_c: Option<f64>,
        is_heating: bool,
    ) -> SpeedSelection {
        let (desired_index, fresh_cycle) =
            if let (Some(current), Some(prev)) = (zone_temp_c, self.prev_zone_temp_c) {
                let moving_wrong_way = if is_heating {
                    current < prev
                } else {
                    current > prev
                };
                let idx = if moving_wrong_way
                    && self.time_at_current_speed_s >= self.min_time_per_speed_s
                {
                    1
                } else {
                    self.last_speed_index
                };
                (idx, false)
            } else {
                (0, true)
            };
        let desired_index = self.apply_disabled_speeds_two_speed(desired_index);
        let locked = !fresh_cycle
            && self.time_at_current_speed_s < self.min_time_per_speed_s
            && desired_index != self.last_speed_index;
        let speed_index = if locked {
            self.last_speed_index
        } else {
            if desired_index != self.last_speed_index {
                self.time_at_current_speed_s = 0.0;
            }
            desired_index
        };
        let low_cap = self.low_speed_capacity_fraction.clamp(0.01, 0.999);
        if speed_index == 1 {
            SpeedSelection { speed_index: 1, part_load_ratio: load_fraction, speed_frac: 1.0 }
        } else {
            SpeedSelection {
                speed_index: 0,
                part_load_ratio: (load_fraction / low_cap).clamp(0.0, 1.0),
                speed_frac: low_cap,
            }
        }
    }

    fn select_multi_speed(&self, load_fraction: f64) -> SpeedSelection {
        let cap_fracs = self.capacity_fractions();
        if cap_fracs.is_empty() || load_fraction <= 0.0 {
            return SpeedSelection { speed_index: 0, speed_frac: 0.0, part_load_ratio: 0.0 };
        }
        if load_fraction <= cap_fracs[0] {
            return SpeedSelection {
                speed_index: 0,
                speed_frac: 0.0,
                part_load_ratio: load_fraction / cap_fracs[0],
            };
        }
        if load_fraction >= *cap_fracs.last().unwrap() {
            return SpeedSelection {
                speed_index: cap_fracs.len() - 1,
                speed_frac: 0.0,
                part_load_ratio: 1.0,
            };
        }
        let hi = cap_fracs.partition_point(|&f| f < load_fraction);
        let lo = hi - 1;
        let span = cap_fracs[hi] - cap_fracs[lo];
        let frac = if span > f64::EPSILON {
            (load_fraction - cap_fracs[lo]) / span
        } else {
            0.0
        };
        SpeedSelection { speed_index: lo, speed_frac: frac, part_load_ratio: 1.0 }
    }

    fn apply_disabled_speeds_two_speed(&self, desired_index: usize) -> usize {
        if self.disabled_speeds.is_empty() {
            return desired_index;
        }
        if self
            .disabled_speeds
            .get(desired_index)
            .copied()
            .unwrap_or(false)
        {
            self.max_enabled_speed
        } else {
            desired_index
        }
    }

    /// Compute part-load factor at the current speed stage.
    pub fn part_load_factor(&mut self, plr: f64) -> f64 {
        self.part_load_factor_for_stage(plr, self.last_speed_index)
    }

    /// Compute PLF for an explicit speed stage index.
    pub fn part_load_factor_for_stage(&mut self, plr: f64, stage_index: usize) -> f64 {
        if matches!(self.speed_control_mode, SpeedControlMode::VariableSpeedIdeal) {
            self.plf_state = 1.0;
            return 1.0;
        }
        let plr = plr.clamp(0.0, 1.0);

        let plf_raw = if let Some(ref curves) = self.eir_plr_coefficients {
            let coeffs = curves
                .get(stage_index)
                .copied()
                .or_else(|| curves.last().copied())
                .unwrap_or([1.0, 0.0, 0.0]);
            quadratic(&coeffs, plr)
        } else {
            let cd = self.plf_cooling_degradation_coeff.clamp(0.0, 1.0);
            1.0 - cd * (1.0 - plr)
        };

        if plf_raw < 0.7 {
            tracing::warn!(
                plf_raw, plr, stage_index,
                "PLF curve returned value < 0.7; check eir_plr or cooling_cd. Clamping to max(0.7, PLR)."
            );
        }
        let plf = plf_raw.clamp(0.7_f64.max(plr), 1.0);
        self.plf_state = plf;
        plf
    }

    /// Apply the Winkler (2011) exponential startup capacity ramp.
    pub fn apply_startup_capacity_degradation(
        &mut self,
        steady_capacity_w: f64,
        dt_min: f64,
    ) -> f64 {
        let on_now = self.duty_cycle > 0.0;
        let mult = self.startup.capacity_multiplier(on_now, dt_min);
        steady_capacity_w * mult
    }

    pub fn rated_capacity_w(&self, mode: ThermostatMode) -> f64 {
        match mode {
            ThermostatMode::Heating => self
                .heating_capacities_w
                .first()
                .copied()
                .unwrap_or_default(),
            ThermostatMode::Cooling => self
                .cooling_capacities_w
                .first()
                .copied()
                .unwrap_or_default(),
            ThermostatMode::Deadband => 0.0,
        }
    }

    pub fn capacity_at_stage(capacities: &[f64], stage_index: usize) -> f64 {
        if capacities.is_empty() {
            return 0.0;
        }
        capacities[stage_index.min(capacities.len() - 1)]
    }

    pub fn eir_at_stage(&self, stage_index: usize) -> f64 {
        if self.eir_by_stage.is_empty() {
            return 1.0;
        }
        self.eir_by_stage[stage_index.min(self.eir_by_stage.len() - 1)]
    }

    /// Normalized capacity fractions `cap[i] / cap[last]` for the populated capacities array.
    pub fn capacity_fractions(&self) -> Vec<f64> {
        let caps = if self.heating_capacities_w.len() >= self.cooling_capacities_w.len() {
            &self.heating_capacities_w
        } else {
            &self.cooling_capacities_w
        };
        let max_cap = caps.last().copied().unwrap_or(0.0);
        if max_cap <= 0.0 {
            return vec![];
        }
        caps.iter().map(|&c| c / max_cap).collect()
    }

    /// Interpolate capacity between two bracket stages using `speed_frac`.
    pub fn interpolated_capacity(&self, capacities: &[f64], speed_index: usize, speed_frac: f64) -> f64 {
        let cap_lo = Self::capacity_at_stage(capacities, speed_index);
        if speed_frac > 0.0 {
            let cap_hi = Self::capacity_at_stage(capacities, speed_index + 1);
            cap_lo * (1.0 - speed_frac) + cap_hi * speed_frac
        } else {
            cap_lo
        }
    }

    /// Interpolate EIR between two bracket stages using `speed_frac`.
    pub fn interpolated_eir(&self, speed_index: usize, speed_frac: f64) -> f64 {
        let eir_lo = self.eir_at_stage(speed_index);
        if speed_frac > 0.0 {
            let eir_hi = self.eir_at_stage(speed_index + 1);
            eir_lo * (1.0 - speed_frac) + eir_hi * speed_frac
        } else {
            eir_lo
        }
    }

    pub fn airflow_m3_s_for_capacity_w(&self, capacity_w: f64) -> f64 {
        capacity_w.max(0.0) * self.airflow_m3_s_per_w
    }

    pub fn fan_power_w(&self, airflow_m3_s: f64) -> f64 {
        airflow_m3_s.max(0.0) * self.fan_power_w_per_m3_s
    }

    pub fn sensible_latent_from_shr(&self, total_cooling_w: f64) -> (f64, f64) {
        let shr = self.shr.clamp(0.0, 1.0);
        let sensible = total_cooling_w * shr;
        let latent = total_cooling_w - sensible;
        (sensible, latent)
    }
}
