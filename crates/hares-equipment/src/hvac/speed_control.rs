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
    /// Multi-speed with continuous inter-speed interpolation (EnergyPlus VariableSpeed SpeedRatio model).
    /// Capacity and EIR are linearly interpolated between the bracketing speed stages.
    /// Cycling (PLR < 1) only occurs at the lowest speed when load is below stage-0 capacity.
    MultiSpeedInterpolated,
    VariableSpeedIdeal,
}

/// Startup capacity ramp configuration (Winkler 2009 / OCHRE exponential model).
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
    /// `on_now`  -- whether the compressor is commanding output this step.
    /// `dt_min`  -- timestep duration in minutes.
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

    /// Compute the startup capacity multiplier from the current timer state
    /// without advancing the timer.
    ///
    /// Returns 1.0 when the timer is zero (compressor is off or c_d == 0).
    /// Safe to call after `capacity_multiplier` has already been invoked for
    /// the current step — the timer has the correct value and this merely
    /// reports it without mutation.
    pub fn current_multiplier(&self) -> f64 {
        if self.c_d == 0.0 || self.time_since_start_min == 0.0 {
            return 1.0;
        }
        let t_full = 20.0 * self.c_d + 0.4;
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
    /// Lower bracket index (0-based). For inter-speed interpolation, capacity/EIR
    /// are interpolated between `speed_index` and `speed_index + 1`.
    pub speed_index: usize,
    /// Interpolation weight for the upper bracket stage `[0.0, 1.0]`.
    /// 0.0 = fully at `speed_index`; 1.0 = fully at `speed_index + 1`.
    pub speed_frac: f64,
    /// Part-load ratio. Only < 1.0 when at the lowest speed and cycling.
    pub part_load_ratio: f64,
}

/// Normalize absolute capacities `[W]` into fractions of the maximum capacity.
///
/// Returns an empty `Vec` when the maximum capacity is `<= 0`.
/// This is the same algorithm as the former `HvacEquipment::capacity_fractions_for`
/// but extracted as a free function so it can be shared across modules.
pub fn capacity_fractions_for(caps: &[f64]) -> Vec<f64> {
    let max_cap = caps.last().copied().unwrap_or(0.0);
    if max_cap <= 0.0 {
        return vec![];
    }
    caps.iter().map(|&c| c / max_cap).collect()
}

/// Bracket-interpolation of a requested load fraction against normalized
/// capacity fractions, returning a `SpeedSelection`.
///
/// `capacity_fractions` must be sorted ascending. When `capacity_fractions` is
/// empty or `load_fraction` (after optional clamping) is `<= 0`, returns a
/// zero `SpeedSelection` (speed_index=0, speed_frac=0, part_load_ratio=0).
///
/// When `clamp_input` is true the load fraction is clamped to `[0, 1]` before
/// processing (used by variable-speed paths); when false no clamping is applied
/// (used by multi-speed paths that rely on the caller's own bounds checks).
///
/// EnergyPlus Engineering Reference (v8.3 Air System Compound Component Groups):
///   SpeedRatio = ABS(Q_required − Q_{n−1}) / ABS(Q_n − Q_{n−1})
/// which is the same linear interpolation implemented here.
pub fn interpolate_speed_stages(
    load_fraction: f64,
    capacity_fractions: &[f64],
    clamp_input: bool,
) -> SpeedSelection {
    let lf = if clamp_input {
        load_fraction.clamp(0.0, 1.0)
    } else {
        load_fraction
    };
    if capacity_fractions.is_empty() || lf <= 0.0 {
        return SpeedSelection {
            speed_index: 0,
            speed_frac: 0.0,
            part_load_ratio: 0.0,
        };
    }
    if lf <= capacity_fractions[0] {
        let plr = (lf / capacity_fractions[0].max(f64::MIN_POSITIVE)).clamp(0.0, 1.0);
        return SpeedSelection {
            speed_index: 0,
            speed_frac: 0.0,
            part_load_ratio: plr,
        };
    }
    if lf >= *capacity_fractions.last().expect("non-empty") {
        return SpeedSelection {
            speed_index: capacity_fractions.len() - 1,
            speed_frac: 0.0,
            part_load_ratio: 1.0,
        };
    }
    let hi = capacity_fractions.partition_point(|&f| f < lf);
    let lo = hi - 1;
    let span = capacity_fractions[hi] - capacity_fractions[lo];
    let speed_frac = if span > f64::EPSILON {
        (lf - capacity_fractions[lo]) / span
    } else {
        0.0
    };
    SpeedSelection {
        speed_index: lo,
        speed_frac,
        part_load_ratio: 1.0,
    }
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

    #[test]
    fn capacity_fractions_empty_input() {
        assert!(capacity_fractions_for(&[]).is_empty());
    }

    #[test]
    fn capacity_fractions_zero_max_returns_empty() {
        assert!(capacity_fractions_for(&[0.0, 0.0]).is_empty());
    }

    #[test]
    fn capacity_fractions_normalizes_by_max() {
        let fracs = capacity_fractions_for(&[4_000.0, 6_000.0, 8_000.0, 10_000.0]);
        assert_eq!(fracs, vec![0.4, 0.6, 0.8, 1.0]);
    }

    #[test]
    fn capacity_fractions_single_stage() {
        let fracs = capacity_fractions_for(&[5_000.0]);
        assert_eq!(fracs, vec![1.0]);
    }

    #[test]
    fn interpolate_empty_fractions_returns_zero() {
        let sel = interpolate_speed_stages(0.5, &[], false);
        assert_eq!(
            sel,
            SpeedSelection {
                speed_index: 0,
                speed_frac: 0.0,
                part_load_ratio: 0.0
            }
        );
    }

    #[test]
    fn interpolate_zero_load_returns_zero() {
        let fracs = vec![0.4, 0.6, 0.8, 1.0];
        let sel = interpolate_speed_stages(0.0, &fracs, false);
        assert_eq!(
            sel,
            SpeedSelection {
                speed_index: 0,
                speed_frac: 0.0,
                part_load_ratio: 0.0
            }
        );
    }

    #[test]
    fn interpolate_below_lowest_returns_plr_only() {
        let fracs = vec![0.4, 0.6, 0.8, 1.0];
        let sel = interpolate_speed_stages(0.3, &fracs, false);
        assert_eq!(sel.speed_index, 0);
        assert_eq!(sel.speed_frac, 0.0);
        let expected_plr = 0.3 / 0.4;
        assert!((sel.part_load_ratio - expected_plr).abs() < 1e-12);
    }

    #[test]
    fn interpolate_at_first_stage_boundary() {
        let fracs = vec![0.4, 0.6, 0.8, 1.0];
        let sel = interpolate_speed_stages(0.4, &fracs, false);
        assert_eq!(sel.speed_index, 0);
        assert_eq!(sel.speed_frac, 0.0);
        assert!((sel.part_load_ratio - 1.0).abs() < 1e-12);
    }

    #[test]
    fn interpolate_between_stages_midpoint() {
        let fracs = vec![0.4, 0.6, 0.8, 1.0];
        let sel = interpolate_speed_stages(0.5, &fracs, false);
        assert_eq!(sel.speed_index, 0);
        assert!((sel.speed_frac - 0.5).abs() < 1e-12);
        assert_eq!(sel.part_load_ratio, 1.0);
    }

    #[test]
    fn interpolate_between_stages_asymmetric() {
        let fracs = vec![0.4, 0.6, 0.8, 1.0];
        let sel = interpolate_speed_stages(0.7, &fracs, false);
        assert_eq!(sel.speed_index, 1);
        assert!((sel.speed_frac - 0.5).abs() < 1e-12);
        assert_eq!(sel.part_load_ratio, 1.0);
    }

    #[test]
    fn interpolate_at_last_stage_returns_full() {
        let fracs = vec![0.4, 0.6, 0.8, 1.0];
        let sel = interpolate_speed_stages(1.0, &fracs, false);
        assert_eq!(
            sel,
            SpeedSelection {
                speed_index: 3,
                speed_frac: 0.0,
                part_load_ratio: 1.0
            }
        );
    }

    #[test]
    fn interpolate_above_last_stage_returns_full() {
        let fracs = vec![0.4, 0.6, 0.8, 1.0];
        let sel = interpolate_speed_stages(1.5, &fracs, false);
        assert_eq!(
            sel,
            SpeedSelection {
                speed_index: 3,
                speed_frac: 0.0,
                part_load_ratio: 1.0
            }
        );
    }

    #[test]
    fn interpolate_clamp_input_clamps_above_one() {
        let fracs = vec![0.4, 0.6, 0.8, 1.0];
        let sel = interpolate_speed_stages(1.5, &fracs, true);
        assert_eq!(
            sel,
            SpeedSelection {
                speed_index: 3,
                speed_frac: 0.0,
                part_load_ratio: 1.0
            }
        );
    }

    #[test]
    fn interpolate_clamp_input_clamps_below_zero() {
        let fracs = vec![0.4, 0.6, 0.8, 1.0];
        let sel = interpolate_speed_stages(-0.5, &fracs, true);
        assert_eq!(
            sel,
            SpeedSelection {
                speed_index: 0,
                speed_frac: 0.0,
                part_load_ratio: 0.0
            }
        );
    }

    #[test]
    fn interpolate_no_clamp_negative_load_returns_zero() {
        let fracs = vec![0.4, 0.6, 0.8, 1.0];
        let sel = interpolate_speed_stages(-0.5, &fracs, false);
        assert_eq!(
            sel,
            SpeedSelection {
                speed_index: 0,
                speed_frac: 0.0,
                part_load_ratio: 0.0
            }
        );
    }

    #[test]
    fn interpolate_zero_span_returns_zero_frac() {
        let fracs = vec![0.5, 0.5, 1.0];
        let sel = interpolate_speed_stages(0.5, &fracs, false);
        assert_eq!(sel.speed_index, 0);
        assert_eq!(sel.speed_frac, 0.0);
    }

    #[test]
    fn interpolate_two_stage_interpolation() {
        let fracs = vec![0.6, 1.0];
        let sel = interpolate_speed_stages(0.8, &fracs, false);
        assert_eq!(sel.speed_index, 0);
        assert!((sel.speed_frac - 0.5).abs() < 1e-12);
        assert_eq!(sel.part_load_ratio, 1.0);
    }
}
