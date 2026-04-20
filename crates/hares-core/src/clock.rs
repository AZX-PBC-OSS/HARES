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

use chrono::{DateTime, Duration, FixedOffset};

/// Monotonic simulation clock over a fixed horizon.
///
/// Pass `start_time` with the intended local wall-clock time (see module docs).
/// The `.hour()` component is used directly by schedule-based equipment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimClock {
    pub start_time: DateTime<FixedOffset>,
    pub time_res: Duration,
    pub duration: Duration,
    pub(crate) current_step: u64,
}

impl SimClock {
    /// Create a simulation clock starting at `start_time` (local time).
    #[must_use]
    pub fn new(start_time: DateTime<FixedOffset>, time_res: Duration, duration: Duration) -> Self {
        Self {
            start_time,
            time_res,
            duration,
            current_step: 0,
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
}
