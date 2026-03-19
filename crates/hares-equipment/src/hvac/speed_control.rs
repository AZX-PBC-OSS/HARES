//! Speed staging and control types for HVAC equipment.

use serde::{Deserialize, Serialize};

/// Dynamic speed-control mode for HVAC performance selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpeedControlMode {
    #[default]
    SingleSpeed,
    /// Setpoint-based two-speed: high speed selected when setpoint error is large enough.
    TwoSpeedSetpoint,
    /// Time-based two-speed (OCHRE "Time"): starts at low speed on turn-on;
    /// escalates to high speed if temperature continues moving away from setpoint
    /// after `min_time_per_speed_s` has elapsed.
    TwoSpeedTime,
    /// Time-based two-speed alternating (OCHRE "Time2"): always runs at high speed
    /// when on. Equivalent to cycling high/low each on-event.
    TwoSpeedAlternating,
    FourSpeed,
    VariableSpeedIdeal,
}

/// Startup capacity ramp configuration (Winkler 2011 / OCHRE exponential model).
///
/// The ramp multiplier follows:
///   t_full = 20.0 * c_d + 0.4  [minutes]
///   mult = clamp(0, 1, -1.025 * exp(-3.79936 * t / t_full) + 1.025)
///
/// When `c_d == 0.0` the ramp is bypassed and the multiplier is always 1.0,
/// which is the correct behaviour for variable-speed equipment.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct StartupConfig {
    /// AHRI degradation coefficient (Cd). 0.0 disables the ramp entirely.
    pub c_d: f64,
    /// Accumulated time [minutes] since the compressor last started.
    /// Reset to `0.5 * dt_min` on the first on-step; incremented by `dt_min`
    /// on subsequent on-steps. Zeroed on any off-step.
    pub time_since_start_min: f64,
}

impl Default for StartupConfig {
    fn default() -> Self {
        Self {
            c_d: 0.25,
            time_since_start_min: 0.0,
        }
    }
}

impl StartupConfig {
    pub fn validate(self) -> crate::Result<()> {
        use hares_types::HaresError;
        if !self.c_d.is_finite() || self.c_d < 0.0 {
            return Err(HaresError::Equipment(format!(
                "startup c_d must be finite and >= 0, got {}",
                self.c_d
            )));
        }
        Ok(())
    }

    /// Compute the startup capacity multiplier for the current timestep.
    ///
    /// `on_now`  — whether the compressor is commanding output this step.
    /// `dt_min`  — timestep duration in minutes.
    ///
    /// Returns a value in `[0.0, 1.0]`.
    pub fn capacity_multiplier(&mut self, on_now: bool, dt_min: f64) -> f64 {
        if !on_now {
            self.time_since_start_min = 0.0;
            return 1.0;
        }

        // c_d == 0 ⇒ variable-speed or no startup ramp configured.
        if self.c_d == 0.0 {
            return 1.0;
        }

        let t_full = 20.0 * self.c_d + 0.4;

        // First on-step: OCHRE assumes startup happened mid-step.
        if self.time_since_start_min == 0.0 {
            self.time_since_start_min = 0.5 * dt_min;
        } else {
            self.time_since_start_min += dt_min;
        }

        if self.time_since_start_min >= t_full {
            1.0
        } else {
            let t = self.time_since_start_min;
            (-1.025_f64 * (-3.799_36_f64 * t / t_full).exp() + 1.025).clamp(0.0, 1.0)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpeedSelection {
    pub speed_index: usize,
    pub part_load_ratio: f64,
    pub speed_fraction: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c_d_zero_always_returns_one() {
        let mut cfg = StartupConfig {
            c_d: 0.0,
            time_since_start_min: 0.0,
        };
        // First on-step.
        assert_eq!(cfg.capacity_multiplier(true, 1.0), 1.0);
        // Subsequent on-step.
        assert_eq!(cfg.capacity_multiplier(true, 1.0), 1.0);
    }

    #[test]
    fn off_step_resets_timer_and_returns_one() {
        let mut cfg = StartupConfig {
            c_d: 0.25,
            time_since_start_min: 5.0,
        };
        let mult = cfg.capacity_multiplier(false, 1.0);
        assert_eq!(mult, 1.0);
        assert_eq!(cfg.time_since_start_min, 0.0);
    }

    #[test]
    fn first_on_step_sets_half_dt() {
        let mut cfg = StartupConfig {
            c_d: 0.25,
            time_since_start_min: 0.0,
        };
        let mult = cfg.capacity_multiplier(true, 2.0);
        // time should be set to 0.5 * 2.0 = 1.0 minute
        assert!((cfg.time_since_start_min - 1.0).abs() < 1e-12);
        // mult < 1.0 at t=1.0 min for t_full=20*0.25+0.4=5.4 min
        assert!(mult < 1.0, "startup mult must be <1 at t=1 min: {mult}");
        assert!(mult > 0.0);
    }

    #[test]
    fn at_t_full_returns_one() {
        // c_d=0.25 → t_full = 5.4 min
        let mut cfg = StartupConfig {
            c_d: 0.25,
            time_since_start_min: 5.4,
        };
        // Advance one step with dt=1 min (time_since_start_min was already 5.4,
        // so after += dt_min it becomes 6.4 >= t_full).
        let mult = cfg.capacity_multiplier(true, 1.0);
        assert_eq!(mult, 1.0, "must return 1.0 once t >= t_full");
    }

    #[test]
    fn restart_after_off_ramps_from_zero_again() {
        let mut cfg = StartupConfig {
            c_d: 0.25,
            time_since_start_min: 0.0,
        };
        // Run past t_full.
        for _ in 0..10 {
            cfg.capacity_multiplier(true, 1.0);
        }
        assert_eq!(cfg.capacity_multiplier(true, 1.0), 1.0);
        // Turn off.
        cfg.capacity_multiplier(false, 1.0);
        assert_eq!(cfg.time_since_start_min, 0.0);
        // First step after restart should degrade.
        let restart_mult = cfg.capacity_multiplier(true, 2.0);
        assert!(
            restart_mult < 1.0,
            "must degrade on restart: {restart_mult}"
        );
    }

    #[test]
    fn winkler_formula_reference_value() {
        // c_d=0.25, t=0.5 min (half of 1-min dt), t_full=5.4 min
        // mult = -1.025 * exp(-3.79936 * 0.5 / 5.4) + 1.025
        let c_d = 0.25_f64;
        let t_full = 20.0 * c_d + 0.4;
        let t = 0.5_f64;
        let expected = (-1.025_f64 * (-3.799_36_f64 * t / t_full).exp() + 1.025).clamp(0.0, 1.0);
        let mut cfg = StartupConfig {
            c_d,
            time_since_start_min: 0.0,
        };
        let mult = cfg.capacity_multiplier(true, 1.0); // dt=1 min → time=0.5 min
        assert!(
            (mult - expected).abs() < 1e-9,
            "expected {expected}, got {mult}"
        );
    }
}
