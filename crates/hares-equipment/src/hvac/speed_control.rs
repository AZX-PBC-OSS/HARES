//! Speed staging and control types for HVAC equipment.

use serde::{Deserialize, Serialize};
#[cfg(any(debug_assertions, feature = "check_invariants"))]
use tracing::warn;

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

/// Startup capacity ramp configuration (Winkler 2011 exponential model).
///
/// The ramp multiplier follows:
///   t_full = 20.0 * c_d + 0.4  [minutes]
///   mult = clamp(0, 1, -1.025 * exp(-3.79936 * t / t_full) + 1.025)
///
/// When `c_d == 0.0` the ramp is bypassed and the multiplier is always 1.0.
/// Default `c_d = 0.0` matches OCHRE's `"Startup Capacity Degradation (-)"` default
/// (HVAC.py:765) — startup ramp is opt-in via an explicit user override.
///
/// The PLF cycling degradation coefficient (AHRI 210/240) is held separately in
/// `HvacRuntimeState::plf_cooling_degradation_coeff` and is not conflated with
/// the startup ramp Cd.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct StartupConfig {
    /// Winkler (2011) startup capacity degradation coefficient (Cd). 0.0 disables the ramp entirely.
    pub c_d: f64,
    /// Accumulated time [minutes] since the compressor last started.
    /// Reset to `0.0` on the first on-step after an off-step; set to
    /// `0.5 * dt_min` immediately after the reset, then incremented by
    /// `dt_min` on subsequent on-steps. Preserved across off-steps.
    pub time_since_start_min: f64,
    /// Whether the compressor was running on the previous call. Used for
    /// edge-detection: the timer only resets on a true off→on transition,
    /// matching OCHRE's mode-transition-based reset (HVAC.py:978–979).
    pub was_on: bool,
    /// Cumulative count of startup-timer resets (true off→on transitions)
    /// since init. Diagnostic counter surfaced as the
    /// `STARTUP_TIMER_RESET_COUNT` telemetry column: under edge detection it
    /// increments exactly once per compressor restart, so differencing
    /// consecutive diagnostic-CSV rows yields the resets-per-hour rate.
    /// Like the biquadratic clamp counter, it is diagnostic-only and is not
    /// persisted across checkpoint restore (restarts from 0).
    #[serde(default)]
    pub reset_count: u64,
}

impl Default for StartupConfig {
    fn default() -> Self {
        Self {
            c_d: 0.0,
            time_since_start_min: 0.0,
            was_on: false,
            reset_count: 0,
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
    ///
    /// Edge detection (off→on transition) guards the timer reset, matching
    /// OCHRE's mode-transition-based pattern (HVAC.py:978–979). The timer is
    /// preserved across off-steps so repeated on/off cycling from PLR < 1.0
    /// duty-cycle modulation cannot trigger multiple ramp resets per cycle.
    ///
    /// Call contract: must be invoked exactly once per simulation timestep,
    /// including steps where the compressor is off. OCHRE's equivalent
    /// previous-mode state is updated unconditionally every timestep
    /// (`mode_prev = self.mode`, HVAC.py:616); the edge detector only works
    /// if `was_on` receives the same unconditional per-step bookkeeping.
    /// A caller that skips off-steps freezes `was_on` at `true` and the
    /// reset never fires on a genuine cold restart.
    ///
    /// Relationship to EnergyPlus: E+ DX coils fold compressor startup
    /// thermal lag into the PLF curve as an energy penalty (AHRI 210/240 Cd;
    /// DXCoils.cc "Part load factor, accounts for thermal lag at compressor
    /// startup") and time-resolve only the *latent* startup transient
    /// (Henderson-Rengarajan, DXCoils.cc `Tcl`). The time-resolved *sensible*
    /// ramp modelled here is the OCHRE/Winkler (2011) residential extension;
    /// HARES keeps the PLF cycling penalty separate in
    /// `HvacRuntimeState::plf_cooling_degradation_coeff`.
    pub fn capacity_multiplier(&mut self, on_now: bool, dt_min: f64) -> f64 {
        let transitioning_on = !self.was_on && on_now;
        self.was_on = on_now;

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        let prev_time = self.time_since_start_min;

        if !on_now {
            #[cfg(feature = "observe")]
            tracing::debug!(
                time_since_start_min = self.time_since_start_min,
                "startup ramp: compressor off, timer preserved"
            );
            return 1.0;
        }

        // Off→on transition: reset timer to model cold-start transient.
        // Consecutive on-steps accumulate without reset.
        if transitioning_on {
            self.time_since_start_min = 0.0;
            self.reset_count += 1;
            #[cfg(feature = "observe")]
            tracing::debug!(
                reset_count = self.reset_count,
                "startup ramp: off→on transition detected, timer reset"
            );
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

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            if self.time_since_start_min < prev_time {
                assert!(
                    transitioning_on,
                    "time_since_start_min decreased from {prev_time} to {} \
                     without an off→on transition",
                    self.time_since_start_min
                );
            }
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
    let fractions: Vec<f64> = caps.iter().map(|&c| c / max_cap).collect();
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    if (fractions.last().copied().unwrap_or(1.0) - 1.0).abs() > 1e-6 {
        warn!(
            max_entry = max_cap,
            normalized_max = fractions.last().copied().unwrap_or(1.0),
            "capacity_fractions_for normalizes max to {} instead of 1.0; \
             capacity ratios may have non-unity max fraction",
            fractions.last().unwrap()
        );
    }
    fractions
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
            was_on: false,
            reset_count: 0,
        };
        // First on-step.
        assert_eq!(cfg.capacity_multiplier(true, 1.0), 1.0);
        // Subsequent on-step.
        assert_eq!(cfg.capacity_multiplier(true, 1.0), 1.0);
    }

    #[test]
    fn off_step_preserves_timer_and_returns_one() {
        let mut cfg = StartupConfig {
            c_d: 0.25,
            time_since_start_min: 5.0,
            was_on: true,
            reset_count: 0,
        };
        let mult = cfg.capacity_multiplier(false, 1.0);
        assert_eq!(mult, 1.0);
        assert!(!cfg.was_on, "was_on must be false after off-step");
        assert!(
            (cfg.time_since_start_min - 5.0).abs() < 1e-12,
            "timer must be preserved across off-step, got {}",
            cfg.time_since_start_min
        );
    }

    #[test]
    fn first_on_step_sets_half_dt() {
        let mut cfg = StartupConfig {
            c_d: 0.25,
            time_since_start_min: 0.0,
            was_on: false,
            reset_count: 0,
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
            was_on: true,
            reset_count: 0,
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
            was_on: false,
            reset_count: 0,
        };
        // Run past t_full.
        for _ in 0..10 {
            cfg.capacity_multiplier(true, 1.0);
        }
        assert_eq!(cfg.capacity_multiplier(true, 1.0), 1.0);
        // Turn off: timer is preserved across off-steps (edge detection).
        cfg.capacity_multiplier(false, 1.0);
        assert!(
            cfg.time_since_start_min > 0.0,
            "timer must be preserved across off-step, got {}",
            cfg.time_since_start_min
        );
        // First step after restart should degrade (off→on transition resets timer).
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
            was_on: false,
            reset_count: 0,
        };
        let mult = cfg.capacity_multiplier(true, 1.0); // dt=1 min → time=0.5 min
        assert!(
            (mult - expected).abs() < 1e-9,
            "expected {expected}, got {mult}"
        );
    }

    /// Edge detection: alternating on/off steps. The first on-step produces
    /// a ramp (cold start). The first off-step returns 1.0 and records off
    /// state without resetting the timer. The second on-step produces a fresh
    /// ramp (off→on transition reset), but back-to-back on-steps do NOT reset.
    #[test]
    fn edge_detection_alternating_on_off_and_consecutive_on_no_reset() {
        // c_d=0.25 → t_full = 5.4 min
        let mut cfg = StartupConfig {
            c_d: 0.25,
            time_since_start_min: 0.0,
            was_on: false,
            reset_count: 0,
        };

        // Step 1: first on-step — ramp starts (transition detected).
        let m1 = cfg.capacity_multiplier(true, 1.0);
        assert!(m1 < 1.0, "first on-step must ramp: {m1}");
        assert!(
            (cfg.time_since_start_min - 0.5).abs() < 1e-12,
            "first on-step: timer must be 0.5 min, got {}",
            cfg.time_since_start_min
        );
        assert!(cfg.was_on, "was_on must be true after first on-step");

        // Step 2: off-step — returns 1.0, timer preserved, was_on → false.
        let m2 = cfg.capacity_multiplier(false, 1.0);
        assert!((m2 - 1.0).abs() < 1e-12, "off-step must return 1.0: {m2}");
        assert!(
            (cfg.time_since_start_min - 0.5).abs() < 1e-12,
            "off-step: timer must be preserved at 0.5 min, got {}",
            cfg.time_since_start_min
        );
        assert!(!cfg.was_on, "was_on must be false after off-step");

        // Step 3: second on-step — off→on transition resets timer,
        // fresh ramp starts.
        let m3 = cfg.capacity_multiplier(true, 1.0);
        assert!(m3 < 1.0, "second on-step must ramp (edge transition): {m3}");
        assert!(
            (cfg.time_since_start_min - 0.5).abs() < 1e-12,
            "second on-step: timer must be 0.5 min after reset+advance, got {}",
            cfg.time_since_start_min
        );
        assert!(cfg.was_on, "was_on must be true after second on-step");

        // Step 4: consecutive on-step — NO reset, timer advances.
        let prev_timer = cfg.time_since_start_min;
        let m4 = cfg.capacity_multiplier(true, 1.0);
        assert!(m4 < 1.0, "consecutive on-step must still be in ramp: {m4}");
        assert!(
            (cfg.time_since_start_min - (prev_timer + 1.0)).abs() < 1e-12,
            "consecutive on-step: timer must advance from {prev_timer} to {:.1}, got {}",
            prev_timer + 1.0,
            cfg.time_since_start_min
        );
        assert!(cfg.was_on, "was_on must stay true on consecutive on-step");

        // Step 5: another off-step confirms timer preserved.
        let timer_before_off = cfg.time_since_start_min;
        let m5 = cfg.capacity_multiplier(false, 1.0);
        assert!((m5 - 1.0).abs() < 1e-12);
        assert!(
            (cfg.time_since_start_min - timer_before_off).abs() < 1e-12,
            "off-step must preserve timer at {}, got {}",
            timer_before_off,
            cfg.time_since_start_min
        );
        assert!(!cfg.was_on);
    }

    /// `reset_count` (the `STARTUP_TIMER_RESET_COUNT` diagnostic source)
    /// increments exactly once per true off→on transition: cold start and
    /// restart each count one; consecutive on-steps and off-steps do not.
    /// The count reflects timer resets, so it advances even when `c_d == 0`
    /// (ramp disabled) — the off→on timer reset still occurs.
    #[test]
    fn reset_count_increments_only_on_off_to_on_transitions() {
        let mut cfg = StartupConfig {
            c_d: 0.25,
            time_since_start_min: 0.0,
            was_on: false,
            reset_count: 0,
        };

        cfg.capacity_multiplier(true, 1.0); // cold start: off→on
        assert_eq!(cfg.reset_count, 1, "cold start must count one reset");
        cfg.capacity_multiplier(true, 1.0); // consecutive on: no transition
        assert_eq!(cfg.reset_count, 1, "consecutive on-step must not count");
        cfg.capacity_multiplier(false, 1.0); // off: no transition
        assert_eq!(cfg.reset_count, 1, "off-step must not count");
        cfg.capacity_multiplier(false, 1.0); // still off: no transition
        assert_eq!(cfg.reset_count, 1, "sustained off must not count");
        cfg.capacity_multiplier(true, 1.0); // restart: off→on
        assert_eq!(cfg.reset_count, 2, "restart must count a second reset");

        // c_d == 0 disables the ramp multiplier but the off→on timer reset
        // still fires, so the diagnostic count still advances.
        let mut disabled = StartupConfig::default();
        disabled.capacity_multiplier(true, 1.0);
        disabled.capacity_multiplier(false, 1.0);
        disabled.capacity_multiplier(true, 1.0);
        assert_eq!(
            disabled.reset_count, 2,
            "c_d=0 must still count off→on timer resets"
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

    #[test]
    fn capacity_fractions_normalizes_overspeed_ratios() {
        let fracs = capacity_fractions_for(&[0.4, 0.6, 0.8, 1.2]);
        let expected = [0.4 / 1.2, 0.6 / 1.2, 0.8 / 1.2, 1.0];
        assert_eq!(fracs.len(), 4);
        for (i, (got, exp)) in fracs.iter().zip(expected.iter()).enumerate() {
            assert!(
                (got - exp).abs() < 1e-10,
                "index {i}: expected {exp}, got {got}"
            );
        }
        assert!(
            (fracs.last().unwrap() - 1.0).abs() < 1e-12,
            "last fraction must be 1.0"
        );
    }

    #[test]
    fn interpolate_at_exact_rated_capacity_selects_last_stage() {
        // After capacity_fractions_for normalizes by max, the last stage is
        // fraction 1.0. A load at exactly rated capacity (load_fraction=1.0)
        // must select the last stage with plr=1.0, not interpolate between
        // penultimate and ultimate stages.
        let fracs = capacity_fractions_for(&[0.4, 0.6, 0.8, 1.2]);
        // fracs = [0.333..., 0.5, 0.666..., 1.0]
        let sel = interpolate_speed_stages(1.0, &fracs, false);
        assert_eq!(
            sel.speed_index,
            fracs.len() - 1,
            "rated load must select last stage"
        );
        assert!(
            sel.speed_frac.abs() < 1e-12,
            "rated load must not interpolate between stages"
        );
        assert!(
            (sel.part_load_ratio - 1.0).abs() < 1e-12,
            "rated load must have full part-load ratio"
        );
    }

    // --- StartupConfig tests ---

    /// StartupConfig::default() Cd = 0.0 → multiplier is always 1.0
    /// (ramp disabled), matching OCHRE's opt-in startup behaviour.
    #[test]
    fn startup_config_default_disables_ramp() {
        let mut cfg = StartupConfig::default();
        assert!(
            cfg.c_d.abs() < 1e-12,
            "default startup Cd must be 0.0, got {}",
            cfg.c_d
        );
        // At any time step, multiplier must be 1.0.
        for dt_min in [0.5, 1.0, 5.0, 10.0] {
            let mult = cfg.capacity_multiplier(true, dt_min);
            assert!(
                (mult - 1.0).abs() < 1e-12,
                "c_d=0 must yield multiplier 1.0 at dt={dt_min} min, got {mult}"
            );
        }
        // Also verify warm-start: after several on-steps, still 1.0.
        let mut cfg2 = StartupConfig::default();
        for _ in 0..10 {
            cfg2.capacity_multiplier(true, 1.0);
        }
        assert!(
            (cfg2.capacity_multiplier(true, 1.0) - 1.0).abs() < 1e-12,
            "c_d=0 must yield multiplier 1.0 after warm-up"
        );
    }

    /// StartupConfig with explicit non-zero Cd produces a ramp multiplier
    /// < 1.0 at cold start, confirming the ramp is opt-in.
    /// Winkler (2011): t_full = 20*Cd + 0.4; mult = -1.025*exp(-3.79936*t/t_full) + 1.025.
    #[test]
    fn startup_config_explicit_cd_produces_ramp() {
        let mut cfg = StartupConfig {
            c_d: 0.25,
            time_since_start_min: 0.0,
            was_on: false,
            reset_count: 0,
        };
        let mult = cfg.capacity_multiplier(true, 1.0);
        assert!(
            mult < 1.0,
            "c_d=0.25 cold start must produce multiplier < 1.0, got {mult}"
        );
        // Winkler formula check: t_full = 5.4 min, t = 0.5 min (mid-step).
        let t_full = 20.0 * 0.25_f64 + 0.4;
        let expected = (-1.025_f64 * (-3.799_36_f64 * 0.5 / t_full).exp() + 1.025).clamp(0.0, 1.0);
        assert!(
            (mult - expected).abs() < 1e-12,
            "c_d=0.25 cold start multiplier {mult} != expected {expected}"
        );
    }
}
