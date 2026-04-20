//! PV soiling models.
//!
//! Implements the Kimber soiling model, which tracks dust accumulation on PV
//! panels during dry periods and resets soiling after rain cleaning events.
//!
//! Reference:
//!   Kimber, A., Mitchell, L., Nogradi, S., Wenger, H. (2006).
//!   "The Effect of Soiling on Large Grid-Connected Photovoltaic Systems in
//!    California and the Southwest Region of the United States."
//!   IEEE 4th World Conference on Photovoltaic Energy Conversion.
//!   DOI: 10.1109/WCPEC.2006.279690
//!
//! Future consideration: the HSU model (Coello & Boyle, 2019) provides
//! physics-based deposition using PM2.5/PM10 concentrations and gravitational
//! settling velocities. Adding it would require air quality data inputs not
//! currently available in EPW files.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

/// Configuration parameters for the Kimber soiling model.
///
/// User-facing units (mm, per-day, days) should be converted to SI at the
/// config parsing boundary. All fields are stored in SI internally.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SoilingConfig {
    /// Rainfall accumulation window [s].
    /// Rain within this rolling window is summed to detect cleaning events.
    /// Kimber default: 86_400 (24 hours).
    pub rain_accum_period_s: f64,
    /// Minimum accumulated rainfall within the window to trigger cleaning [m].
    /// Kimber default: 0.006 (6 mm).
    pub cleaning_threshold_m: f64,
    /// Fractional energy loss rate during dry periods [1/s].
    /// Kimber default: 0.0015/day = 1.736e-8/s (suburban temperate).
    ///
    /// Published regional rates (Kimber 2006, Table 1):
    ///   Central Valley CA suburban: 0.0019/day
    ///   Desert SW:                  0.0030/day
    ///   Northern CA suburban:       0.0010/day
    pub soiling_loss_rate_per_s: f64,
    /// Grace period after a cleaning event [s].
    /// While ground remains damp, wind-blown dust is suppressed and soiling
    /// rate is effectively zero. Kimber default: 1_209_600 (14 days).
    pub grace_period_s: f64,
    /// Maximum soiling loss fraction (dimensionless, 0..1).
    /// Prevents unbounded accumulation in extended dry climates.
    /// Kimber default: 0.30 (30% maximum loss).
    pub max_soiling: f64,
    /// Initial soiling loss fraction at simulation start (dimensionless).
    pub initial_soiling: f64,
}

impl Default for SoilingConfig {
    fn default() -> Self {
        Self {
            rain_accum_period_s: 86_400.0,
            cleaning_threshold_m: 0.006,
            soiling_loss_rate_per_s: 0.0015 / 86_400.0,
            grace_period_s: 14.0 * 86_400.0,
            max_soiling: 0.30,
            initial_soiling: 0.0,
        }
    }
}

/// Runtime state for the Kimber soiling model.
///
/// Algorithm (per timestep, Kimber et al. 2006, Section III-B):
///   1. Advance elapsed-time counter
///   2. Push current rainfall into rolling buffer, evict oldest entry
///   3. Sum buffer → accumulated_rain
///   4. If accumulated_rain >= cleaning_threshold or manual wash →
///      reset counter and soiling_loss to 0
///   5. Else if outside grace period → soiling_loss += rate * dt,
///      clamped at max_soiling
///   6. Output soiling_ratio = 1 - soiling_loss
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SoilingState {
    soiling_loss: f64,
    rain_buffer: VecDeque<f64>,
    seconds_since_last_clean: f64,
}

impl SoilingState {
    /// Create a new soiling state sized for the given config and timestep.
    ///
    /// # Panics
    /// Panics if `dt_s` is not positive.
    pub fn new(config: &SoilingConfig, dt_s: f64) -> Self {
        assert!(
            dt_s > 0.0,
            "SoilingState::new requires positive dt_s, got {dt_s}"
        );
        let buffer_capacity = (config.rain_accum_period_s / dt_s).ceil() as usize;
        let mut rain_buffer = VecDeque::with_capacity(buffer_capacity);
        rain_buffer.resize(buffer_capacity, 0.0);

        // If starting clean, begin inside grace period so soiling doesn't
        // immediately accumulate. If starting dirty, begin outside grace.
        let seconds_since_last_clean = if config.initial_soiling == 0.0 {
            0.0
        } else {
            config.grace_period_s + 1.0
        };

        Self {
            soiling_loss: config.initial_soiling,
            rain_buffer,
            seconds_since_last_clean,
        }
    }

    /// Advance the soiling model by one timestep.
    ///
    /// Returns the soiling ratio in [0, 1] where 1.0 = clean panel.
    /// This ratio should multiply effective irradiance before the PV cell model.
    pub fn step(
        &mut self,
        config: &SoilingConfig,
        rainfall_m: f64,
        dt_s: f64,
        manual_wash: bool,
    ) -> f64 {
        let buffer_len = self.rain_buffer.len();

        // Advance elapsed time first, so the counter reflects time elapsed
        // *after* this timestep (Kimber 2006 counts days since last rain).
        self.seconds_since_last_clean += dt_s;

        // Update rolling rainfall buffer.
        if buffer_len > 0 {
            self.rain_buffer.pop_front();
        }
        self.rain_buffer.push_back(rainfall_m.max(0.0));

        // Check for rain cleaning event (Kimber threshold test).
        let accumulated_rain: f64 = self.rain_buffer.iter().sum();
        let rain_clean = accumulated_rain >= config.cleaning_threshold_m;

        if rain_clean || manual_wash {
            // Rain event or manual wash resets both counter and soiling.
            // During the grace period after rain, ground moisture suppresses
            // wind-blown dust resuspension (Kimber 2006, Section III-C).
            self.seconds_since_last_clean = 0.0;
            self.soiling_loss = 0.0;
        } else if self.seconds_since_last_clean <= config.grace_period_s {
            // Still within grace period -- panel stays clean.
            self.soiling_loss = 0.0;
        } else {
            // Accumulate soiling at constant rate during dry periods.
            self.soiling_loss += config.soiling_loss_rate_per_s * dt_s;
            self.soiling_loss = self.soiling_loss.min(config.max_soiling);
        }

        1.0 - self.soiling_loss
    }

    /// Current soiling ratio (1.0 = clean, decreasing = soiled).
    pub fn soiling_ratio(&self) -> f64 {
        1.0 - self.soiling_loss
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR_S: f64 = 3600.0;
    const DAY_S: f64 = 86_400.0;

    fn default_config() -> SoilingConfig {
        SoilingConfig::default()
    }

    #[test]
    fn clean_panel_stays_clean_during_grace_period() {
        let cfg = default_config();
        let mut state = SoilingState::new(&cfg, HOUR_S);
        let ratio = state.step(&cfg, 0.0, HOUR_S, false);
        assert_eq!(ratio, 1.0);
    }

    #[test]
    fn soiling_accumulates_linearly_after_grace_period() {
        let cfg = SoilingConfig {
            grace_period_s: 0.0,
            initial_soiling: 0.0,
            ..default_config()
        };
        let mut state = SoilingState::new(&cfg, HOUR_S);
        // Force past grace period.
        state.seconds_since_last_clean = cfg.grace_period_s + 1.0;

        let mut prev_ratio = 1.0;
        for _ in 0..48 {
            let ratio = state.step(&cfg, 0.0, HOUR_S, false);
            assert!(ratio < prev_ratio, "soiling should increase each step");
            prev_ratio = ratio;
        }

        let expected_loss = cfg.soiling_loss_rate_per_s * 48.0 * HOUR_S;
        let actual_loss = 1.0 - prev_ratio;
        assert!(
            (actual_loss - expected_loss).abs() < 1e-12,
            "expected loss {expected_loss}, got {actual_loss}"
        );
    }

    #[test]
    fn soiling_capped_at_max() {
        let cfg = SoilingConfig {
            grace_period_s: 0.0,
            max_soiling: 0.10,
            ..default_config()
        };
        let mut state = SoilingState::new(&cfg, HOUR_S);
        state.seconds_since_last_clean = cfg.grace_period_s + 1.0;

        for _ in 0..10_000 {
            state.step(&cfg, 0.0, HOUR_S, false);
        }

        assert!(
            (state.soiling_ratio() - (1.0 - cfg.max_soiling)).abs() < 1e-12,
            "soiling should be capped at max: got {}",
            state.soiling_ratio()
        );
    }

    #[test]
    fn rain_event_cleans_panel() {
        let cfg = SoilingConfig {
            grace_period_s: 0.0,
            ..default_config()
        };
        let mut state = SoilingState::new(&cfg, HOUR_S);
        state.seconds_since_last_clean = cfg.grace_period_s + 1.0;

        for _ in 0..(30 * 24) {
            state.step(&cfg, 0.0, HOUR_S, false);
        }
        assert!(state.soiling_ratio() < 1.0, "panel should be soiled");

        let ratio = state.step(&cfg, 0.006, HOUR_S, false);
        assert_eq!(ratio, 1.0, "rain event should clean the panel");
    }

    #[test]
    fn insufficient_rain_does_not_clean() {
        let cfg = SoilingConfig {
            grace_period_s: 0.0,
            ..default_config()
        };
        let mut state = SoilingState::new(&cfg, HOUR_S);
        state.seconds_since_last_clean = cfg.grace_period_s + 1.0;

        for _ in 0..(7 * 24) {
            state.step(&cfg, 0.0, HOUR_S, false);
        }

        // Deliver 2mm rain (below 6mm threshold). Panel must remain soiled.
        let ratio = state.step(&cfg, 0.002, HOUR_S, false);
        assert!(
            ratio < 1.0,
            "insufficient rain should not clean panel to 1.0: ratio={ratio}"
        );
    }

    #[test]
    fn grace_period_keeps_panel_clean_after_rain() {
        // 1-hour accumulation window isolates grace-period logic from buffer
        // clearing delays. Rain is evicted after 1 step.
        let cfg = SoilingConfig {
            grace_period_s: 3.0 * DAY_S,
            rain_accum_period_s: HOUR_S,
            ..default_config()
        };
        let mut state = SoilingState::new(&cfg, HOUR_S);
        state.seconds_since_last_clean = cfg.grace_period_s + 1.0;

        // Soil the panel.
        for _ in 0..240 {
            state.step(&cfg, 0.0, HOUR_S, false);
        }
        assert!(
            state.soiling_ratio() < 1.0,
            "panel must be soiled before rain"
        );

        // Rain event.
        state.step(&cfg, 0.010, HOUR_S, false);
        assert_eq!(state.soiling_ratio(), 1.0, "rain must clean the panel");

        // Panel stays clean for grace period (72 hours at 1-hour steps).
        // seconds_since_last_clean advances from 0 by HOUR_S each step.
        // Grace condition holds while counter <= grace_period_s.
        for step_idx in 0..72 {
            let ratio = state.step(&cfg, 0.0, HOUR_S, false);
            assert_eq!(
                ratio, 1.0,
                "should stay clean during grace period (step {step_idx})"
            );
        }

        // After grace period, soiling resumes.
        state.step(&cfg, 0.0, HOUR_S, false);
        assert!(
            state.soiling_ratio() < 1.0,
            "soiling should resume after grace period expires"
        );
    }

    #[test]
    fn ring_buffer_rolls_over_correctly() {
        let cfg = SoilingConfig {
            rain_accum_period_s: 6.0 * HOUR_S,
            cleaning_threshold_m: 0.005,
            grace_period_s: 0.0,
            ..default_config()
        };
        let mut state = SoilingState::new(&cfg, HOUR_S);
        state.seconds_since_last_clean = 1.0;

        // Deliver 1mm each hour for 5 hours.
        for _ in 0..5 {
            state.step(&cfg, 0.001, HOUR_S, false);
        }
        // Buffer: [0.0, 0.001, 0.001, 0.001, 0.001, 0.001] = 5mm >= 5mm threshold.
        assert_eq!(state.soiling_ratio(), 1.0, "5mm should meet 5mm threshold");

        // Advance 6 dry hours to flush rain out of buffer.
        for _ in 0..6 {
            state.step(&cfg, 0.0, HOUR_S, false);
        }
        assert!(
            state.soiling_ratio() < 1.0,
            "old rain should have rolled out of buffer"
        );
    }

    #[test]
    fn manual_wash_cleans_immediately() {
        let cfg = SoilingConfig {
            grace_period_s: 0.0,
            ..default_config()
        };
        let mut state = SoilingState::new(&cfg, HOUR_S);
        state.seconds_since_last_clean = cfg.grace_period_s + 1.0;

        for _ in 0..240 {
            state.step(&cfg, 0.0, HOUR_S, false);
        }
        assert!(state.soiling_ratio() < 1.0);

        let ratio = state.step(&cfg, 0.0, HOUR_S, true);
        assert_eq!(ratio, 1.0, "manual wash should clean immediately");
    }

    #[test]
    fn default_config_produces_expected_annual_loss() {
        let cfg = default_config();
        let mut state = SoilingState::new(&cfg, HOUR_S);
        state.seconds_since_last_clean = cfg.grace_period_s + 1.0;

        for _ in 0..(30 * 24) {
            state.step(&cfg, 0.0, HOUR_S, false);
        }

        let loss = 1.0 - state.soiling_ratio();
        let expected = cfg.soiling_loss_rate_per_s * 30.0 * DAY_S;
        assert!(
            (loss - expected).abs() < 1e-10,
            "30-day loss: got {loss:.6}, expected {expected:.6}"
        );
        assert!((loss - 0.045).abs() < 1e-10);
    }

    #[test]
    #[should_panic(expected = "positive dt_s")]
    fn zero_dt_panics() {
        let cfg = default_config();
        SoilingState::new(&cfg, 0.0);
    }
}
