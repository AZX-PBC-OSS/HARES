//! Simulation clock and time-step management.
//!
//! ## Local Time Convention
//!
//! HARES treats all timestamps as **local time**. The `.hour()`,
//! `.weekday()`, and `.month()` accessors on `start_time` are used
//! directly by schedule evaluation, daily profiles, and actor logic.
//! The timezone offset is carried through but **only the wall-clock
//! digits matter** -- `12:00 UTC+0` and `12:00 UTC-7` both read as
//! hour 12. The offset is effectively ignored unless `civil_timezone`
//! is set for DST-aware reinterpretation.
//!
//! Pass the intended local hour in `start_time`. If you mean noon
//! Denver time, pass `12:00` -- not `19:00 UTC`.
//!
//! `SimClock` iterates over timestep indices `0..total_steps()`.
//! After iterator exhaustion (`current_step == total_steps()`), `current_time()`
//! returns `start_time + total_steps() * time_res` (one step past the last
//! simulated timestep).

#[cfg(feature = "observe")]
use std::cell::Cell;

use chrono::{DateTime, Duration, FixedOffset};
#[cfg(all(feature = "dst", feature = "observe"))]
use chrono::{Datelike, Timelike};

/// Monotonic simulation clock over a fixed horizon.
///
/// Pass `start_time` with the intended local wall-clock time (see module docs).
/// The `.hour()` component is used directly by schedule-based equipment.
///
/// When `civil_tz` is set (requires `dst` feature), [`current_civil_time`](Self::current_civil_time)
/// provides DST-aware civil wall-clock time. The fixed-offset clock never adjusts
/// for DST — its `.ordinal()` / `.hour()` may produce wrong calendar-field values
/// during transitions. Use `current_civil_time()` for any calendar-sensitive computation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimClock {
    pub start_time: DateTime<FixedOffset>,
    pub time_res: Duration,
    pub duration: Duration,
    pub(crate) current_step: u64,
    /// DST-aware IANA timezone for civil time queries.
    /// When set, [`current_civil_time`](Self::current_civil_time) converts the
    /// fixed-offset clock position through DST rules.
    #[cfg(feature = "dst")]
    pub civil_tz: Option<chrono_tz::Tz>,
    /// Observer capture: last civil hour computed by `current_civil_time()`.
    #[cfg(feature = "observe")]
    pub obs_civil_hour: Cell<Option<u32>>,
    /// Observer capture: last civil ordinal (day of year) computed by `current_civil_time()`.
    #[cfg(feature = "observe")]
    pub obs_civil_ordinal: Cell<Option<u32>>,
    /// Observer capture: last fixed-offset hour from `current_time()` at civil query time.
    #[cfg(feature = "observe")]
    pub obs_fixed_offset_hour: Cell<Option<u32>>,
}

impl SimClock {
    /// Create a simulation clock starting at `start_time` (local time).
    ///
    /// The total number of steps is the integer floor `duration / time_res`.
    /// If `duration` is not an exact multiple of `time_res`, the remainder is
    /// truncated and a warning is emitted. Callers should ensure `duration`
    /// is divisible by `time_res` to avoid unintended horizon shortening.
    #[must_use]
    pub fn new(start_time: DateTime<FixedOffset>, time_res: Duration, duration: Duration) -> Self {
        let res = time_res.num_seconds();
        if res > 0 {
            let duration_secs = duration.num_seconds();
            if duration_secs > 0 {
                let remainder = duration_secs % res;
                if remainder != 0 {
                    let effective = duration_secs - remainder;
                    tracing::warn!(
                        duration_secs = duration_secs,
                        time_res_secs = res,
                        truncated_remainder_secs = remainder,
                        effective_duration_secs = effective,
                        "SimClock duration is not an exact multiple of time resolution; remainder will be truncated"
                    );
                }
            }
        }
        Self {
            start_time,
            time_res,
            duration,
            current_step: 0,
            #[cfg(feature = "dst")]
            civil_tz: None,
            #[cfg(feature = "observe")]
            obs_civil_hour: Cell::new(None),
            #[cfg(feature = "observe")]
            obs_civil_ordinal: Cell::new(None),
            #[cfg(feature = "observe")]
            obs_fixed_offset_hour: Cell::new(None),
        }
    }

    /// Current timestep index.
    #[must_use]
    pub fn current_step(&self) -> u64 {
        self.current_step
    }

    /// Current local time at the active step index.
    #[must_use]
    pub fn current_time(&self) -> DateTime<FixedOffset> {
        let time_res_secs = self.time_res.num_seconds();
        let step_secs = i64::try_from(self.current_step).unwrap_or(i64::MAX);
        self.start_time + Duration::seconds(time_res_secs.saturating_mul(step_secs))
    }

    /// DST-aware civil (wall-clock) time at the current step.
    ///
    /// Returns `None` when no civil timezone is configured. When set, the
    /// fixed-offset clock position is converted through the configured IANA
    /// timezone, applying DST rules so that calendar fields (`.ordinal()`,
    /// `.hour()`, `.minute()`) reflect the civil wall-clock time instead of the
    /// fixed-offset digits.
    ///
    /// Invariant: the conversion from `DateTime<FixedOffset>` → `DateTime<Tz>`
    /// is always unambiguous because both representations reference the same UTC
    /// instant. The naive-local hour of the fixed-offset clock may differ from the
    /// civil hour during DST transitions.
    ///
    /// Requires the `dst` cargo feature.
    #[cfg(feature = "dst")]
    #[must_use]
    pub fn current_civil_time(&self) -> Option<DateTime<chrono_tz::Tz>> {
        self.civil_tz.map(|tz| {
            let fixed = self.current_time();
            let civil = fixed.with_timezone(&tz);

            // Invariant: verify the conversion is plausible. The conversion
            // itself is always unambiguous (both representations reference the
            // same UTC instant), so this check documents intent rather than
            // catches a real failure path. It does verify that no internal
            // panic occurred in chrono's timezone machinery.
            #[cfg(any(debug_assertions, feature = "check_invariants"))]
            {
                let _ = &civil;
                debug_assert!(
                    civil.naive_utc() == fixed.naive_utc(),
                    "civil time UTC does not match fixed-offset UTC"
                );
            }

            #[cfg(feature = "observe")]
            {
                self.obs_civil_hour.set(Some(civil.hour()));
                self.obs_civil_ordinal.set(Some(civil.ordinal()));
                self.obs_fixed_offset_hour.set(Some(fixed.hour()));
            }

            civil
        })
    }

    /// Total number of simulation steps.
    ///
    /// Uses integer floor division `duration / time_res`.
    #[must_use]
    pub fn total_steps(&self) -> u64 {
        let res = self.time_res.num_seconds();
        if res <= 0 {
            return 0;
        }
        let duration_secs = self.duration.num_seconds();
        if duration_secs <= 0 {
            return 0;
        }
        u64::try_from(duration_secs / res).unwrap_or(0)
    }
}

impl Iterator for SimClock {
    type Item = u64;

    fn next(&mut self) -> Option<Self::Item> {
        let step = self.current_step;
        if step >= self.total_steps() {
            return None;
        }
        self.current_step = self.current_step.saturating_add(1);
        Some(step)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thirty_days_at_sixty_seconds_yields_43200_steps() {
        let start = DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z").expect("parse");
        let clock = SimClock::new(start, Duration::seconds(60), Duration::days(30));
        assert_eq!(clock.total_steps(), 43_200);
    }

    #[test]
    fn current_time_at_step_zero_equals_start_time() {
        let start = DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z").expect("parse");
        let clock = SimClock::new(start, Duration::minutes(5), Duration::hours(1));
        assert_eq!(clock.current_step(), 0);
        assert_eq!(clock.current_time(), start);
    }

    #[test]
    fn current_time_after_exhaustion_is_one_step_past_last_simulated_time() {
        let start = DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z").expect("parse");
        let mut clock = SimClock::new(start, Duration::minutes(15), Duration::hours(1));
        let produced: Vec<u64> = clock.by_ref().collect();
        assert_eq!(produced, vec![0, 1, 2, 3]);
        assert_eq!(clock.current_step(), clock.total_steps());
        assert_eq!(clock.current_time(), start + Duration::hours(1));
    }

    #[test]
    fn supports_arbitrary_duration_lengths() {
        let start = DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z").expect("parse");
        let clock = SimClock::new(start, Duration::minutes(30), Duration::weeks(2));
        assert_eq!(clock.total_steps(), 672);
    }

    #[test]
    fn total_steps_truncates_remainder_for_non_dividing_duration() {
        let start = DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z").expect("parse");
        // 86400s duration / 3300s resolution = 26.18... → 26 steps (600s truncated)
        let clock = SimClock::new(start, Duration::seconds(3300), Duration::days(1));
        assert_eq!(clock.total_steps(), 26);
    }

    #[test]
    fn total_steps_exact_for_evenly_dividing_duration() {
        let start = DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z").expect("parse");
        // 86400s duration / 3600s resolution = 24 steps exactly
        let clock = SimClock::new(start, Duration::hours(1), Duration::days(1));
        assert_eq!(clock.total_steps(), 24);
    }

    #[test]
    fn total_steps_zero_when_duration_shorter_than_resolution() {
        let start = DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z").expect("parse");
        let clock = SimClock::new(start, Duration::hours(1), Duration::minutes(30));
        assert_eq!(clock.total_steps(), 0);
    }
}
