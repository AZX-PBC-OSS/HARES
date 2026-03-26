use std::sync::Arc;

use chrono::{Datelike, Timelike, Weekday};
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use rand_distr::{Distribution, StandardNormal};

use crate::{DomainId, EnvironmentState, HaresError};

/// Which days a [`TimeWindow`] applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DayFilter {
    /// Every day of the week.
    Any,
    /// Monday through Friday.
    Weekdays,
    /// Saturday and Sunday.
    Weekends,
    /// A specific day of the week.
    Day(Weekday),
}

impl DayFilter {
    /// Returns `true` if `weekday` is matched by this filter.
    pub fn matches(self, weekday: Weekday) -> bool {
        match self {
            Self::Any => true,
            Self::Weekdays => !matches!(weekday, Weekday::Sat | Weekday::Sun),
            Self::Weekends => matches!(weekday, Weekday::Sat | Weekday::Sun),
            Self::Day(d) => weekday == d,
        }
    }
}

/// A time-of-day window with an associated value.
///
/// Times are expressed as minutes from midnight.
/// `start_minute` is in `0..1440`, `end_minute` is in `0..=1440`.
/// The range is half-open: `[start, end)`.
/// `end_minute = 1440` represents end-of-day (24:00), allowing a full-day
/// window as `(0, 1440)`.
///
/// If `start_minute > end_minute` the window wraps across midnight
/// (e.g. 22:00–06:00 → `start_minute: 1320, end_minute: 360`).
/// For midnight-wrapping windows with a day-specific filter, the post-midnight
/// portion matches the *next* calendar day (e.g. `Day(Mon), 1320, 360` matches
/// Monday 22:00–23:59 and Tuesday 00:00–05:59).
///
/// `start_minute == end_minute` is invalid and will panic in debug builds.
///
/// When used inside [`ScheduleSource::TimeWindows`], windows are evaluated in
/// declaration order and the first match wins. Overlapping windows are allowed
/// — use ordering to express priority.
#[derive(Clone, Debug, PartialEq)]
pub struct TimeWindow {
    pub day: DayFilter,
    pub start_minute: u16,
    pub end_minute: u16,
    pub value: f64,
}

impl TimeWindow {
    /// Create a new time window.
    ///
    /// `start_minute` must be in `0..1440`, `end_minute` in `0..=1440`.
    /// `start_minute == end_minute` panics in debug (zero-width window never matches).
    pub fn new(day: DayFilter, start_minute: u16, end_minute: u16, value: f64) -> Self {
        debug_assert!(start_minute < 1440, "start_minute out of range");
        debug_assert!(end_minute <= 1440, "end_minute out of range");
        debug_assert!(
            start_minute != end_minute,
            "zero-width window never matches; use default instead"
        );
        Self {
            day,
            start_minute,
            end_minute,
            value,
        }
    }

    /// Does this window contain the given day and minute-of-day?
    pub fn contains(&self, weekday: Weekday, minute_of_day: u16) -> bool {
        if self.start_minute < self.end_minute {
            // Normal window: [start, end) on the anchor day
            self.day.matches(weekday)
                && minute_of_day >= self.start_minute
                && minute_of_day < self.end_minute
        } else {
            // Midnight-wrapping window: [start, 1440) on anchor day
            //                           ∪ [0, end) on the following day
            if minute_of_day >= self.start_minute {
                self.day.matches(weekday)
            } else if minute_of_day < self.end_minute {
                self.day.matches(weekday.pred())
            } else {
                false
            }
        }
    }
}

/// Canonical custom-domain id used for schedule payloads in `EnvironmentState.custom_domains`.
pub const SCHEDULE_DOMAIN_ID: DomainId = DomainId(u16::MAX);

/// Returns the canonical schedule custom-domain id.
pub const fn schedule_domain_id() -> DomainId {
    SCHEDULE_DOMAIN_ID
}

/// Out-of-range index behavior for schedule-backed sources.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoundaryPolicy {
    /// Clamp indices to the first/last valid sample.
    Clamp,
    /// Wrap indices modulo the source length.
    Wrap,
    /// Return an error when the index is out-of-bounds.
    Error,
}

/// A lazily-evaluated source for schedule values.
#[non_exhaustive]
#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum ScheduleSource {
    /// Fixed scalar value.
    Constant(f64),
    /// 24-hour weekday/weekend profile with monthly scaling and maximum value.
    DailyProfile {
        weekday: [f64; 24],
        weekend: [f64; 24],
        month_multipliers: [f64; 12],
        max_value: f64,
    },
    /// Column index in the schedule custom-domain payload.
    ColumnRef {
        col_idx: usize,
        boundary: BoundaryPolicy,
    },
    /// Solar-aware profile scaled by monthly multipliers and maximum value.
    SolarAware {
        daytime_fraction: f64,
        evening_fraction: f64,
        overnight_fraction: f64,
        month_multipliers: [f64; 12],
        max_value: f64,
        dusk_altitude_threshold_deg: f64,
    },
    /// Stateful pseudo-random source, deterministic by `seed` + call order.
    SeededNoise {
        base: f64,
        std_dev: f64,
        seed: [u8; 32],
        draw_count: u64,
        rng: ChaCha8Rng,
    },
    /// Shared data with cursor-based advancement.
    Shared {
        data: Arc<[f64]>,
        cursor: usize,
        boundary: BoundaryPolicy,
    },
    /// Window-based lookup by day-of-week and time-of-day.
    ///
    /// Windows are evaluated in order; **the first matching window wins**.
    /// Overlapping windows are permitted — use ordering to express priority
    /// (e.g. place a day-specific override before a broad weekday catch-all).
    ///
    /// If no window matches, `default` is used. If `default` is `None`
    /// and nothing matches, an error is returned.
    TimeWindows {
        windows: Vec<TimeWindow>,
        default: Option<f64>,
    },
}

impl PartialEq for ScheduleSource {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Constant(a), Self::Constant(b)) => a == b,
            (
                Self::DailyProfile {
                    weekday: a_wd,
                    weekend: a_we,
                    month_multipliers: a_mm,
                    max_value: a_max,
                },
                Self::DailyProfile {
                    weekday: b_wd,
                    weekend: b_we,
                    month_multipliers: b_mm,
                    max_value: b_max,
                },
            ) => a_wd == b_wd && a_we == b_we && a_mm == b_mm && a_max == b_max,
            (
                Self::ColumnRef {
                    col_idx: a_idx,
                    boundary: a_boundary,
                },
                Self::ColumnRef {
                    col_idx: b_idx,
                    boundary: b_boundary,
                },
            ) => a_idx == b_idx && a_boundary == b_boundary,
            (
                Self::SolarAware {
                    daytime_fraction: a_day,
                    evening_fraction: a_eve,
                    overnight_fraction: a_overnight,
                    month_multipliers: a_mm,
                    max_value: a_max,
                    dusk_altitude_threshold_deg: a_dusk,
                },
                Self::SolarAware {
                    daytime_fraction: b_day,
                    evening_fraction: b_eve,
                    overnight_fraction: b_overnight,
                    month_multipliers: b_mm,
                    max_value: b_max,
                    dusk_altitude_threshold_deg: b_dusk,
                },
            ) => {
                a_day == b_day
                    && a_eve == b_eve
                    && a_overnight == b_overnight
                    && a_mm == b_mm
                    && a_max == b_max
                    && a_dusk == b_dusk
            }
            (
                Self::SeededNoise {
                    base: a_base,
                    std_dev: a_std,
                    seed: a_seed,
                    draw_count: a_count,
                    rng: _,
                },
                Self::SeededNoise {
                    base: b_base,
                    std_dev: b_std,
                    seed: b_seed,
                    draw_count: b_count,
                    rng: _,
                },
            ) => a_base == b_base && a_std == b_std && a_seed == b_seed && a_count == b_count,
            (
                Self::Shared {
                    data: a_data,
                    cursor: a_cursor,
                    boundary: a_boundary,
                },
                Self::Shared {
                    data: b_data,
                    cursor: b_cursor,
                    boundary: b_boundary,
                },
            ) => a_data == b_data && a_cursor == b_cursor && a_boundary == b_boundary,
            (
                Self::TimeWindows {
                    windows: a_win,
                    default: a_def,
                },
                Self::TimeWindows {
                    windows: b_win,
                    default: b_def,
                },
            ) => a_win == b_win && a_def == b_def,
            _ => false,
        }
    }
}

impl ScheduleSource {
    /// Resolve the current value from this source.
    pub fn value_at(&mut self, env: &EnvironmentState) -> Result<f64, HaresError> {
        match self {
            Self::Constant(v) => Ok(*v),
            Self::DailyProfile {
                weekday,
                weekend,
                month_multipliers,
                max_value,
            } => {
                let hour = env.current_time.hour() as usize;
                let month_idx = env.current_time.month0() as usize;
                let is_weekend = env.current_time.weekday().num_days_from_monday() >= 5;
                let frac = if is_weekend {
                    weekend[hour]
                } else {
                    weekday[hour]
                };
                Ok(frac * month_multipliers[month_idx] * *max_value)
            }
            Self::ColumnRef { col_idx, boundary } => {
                let payload = env
                    .custom_domains
                    .iter()
                    .find(|d| d.domain_id == SCHEDULE_DOMAIN_ID)
                    .and_then(|d| d.custom_payload.as_ref())
                    .ok_or_else(|| {
                        HaresError::Equipment(
                            "schedule domain payload not found in environment custom domains"
                                .to_string(),
                        )
                    })?;
                let idx = resolve_index(*col_idx as i64, payload.len(), *boundary)?;
                Ok(payload[idx])
            }
            Self::SolarAware {
                daytime_fraction,
                evening_fraction,
                overnight_fraction,
                month_multipliers,
                max_value,
                dusk_altitude_threshold_deg,
            } => {
                let hour = env.current_time.hour() as usize;
                let month_idx = env.current_time.month0() as usize;
                let sun_alt = env.weather.solar_altitude_deg;

                let frac = if sun_alt > *dusk_altitude_threshold_deg {
                    *daytime_fraction
                } else if hour >= 5 {
                    *evening_fraction
                } else {
                    *overnight_fraction
                };

                Ok(frac * month_multipliers[month_idx] * *max_value)
            }
            Self::SeededNoise {
                base,
                std_dev,
                seed: _,
                draw_count,
                rng,
            } => {
                let z: f64 = StandardNormal.sample(rng);
                *draw_count = draw_count.checked_add(1).expect("draw_count overflow");
                Ok(*base + *std_dev * z)
            }
            Self::Shared {
                data,
                cursor,
                boundary,
            } => {
                let idx = resolve_index(*cursor as i64, data.len(), *boundary)?;
                let value = data[idx];
                *cursor = cursor.saturating_add(1);
                Ok(value)
            }
            Self::TimeWindows { windows, default } => {
                let weekday = env.current_time.weekday();
                let minute_of_day =
                    env.current_time.hour() as u16 * 60 + env.current_time.minute() as u16;
                for w in windows.iter() {
                    if w.contains(weekday, minute_of_day) {
                        return Ok(w.value);
                    }
                }
                default.ok_or_else(|| {
                    HaresError::Equipment(format!(
                        "no time window matches {} {:02}:{:02} and no default is set",
                        weekday,
                        env.current_time.hour(),
                        env.current_time.minute(),
                    ))
                })
            }
        }
    }

    /// Reset any internal mutable state (for checkpoint restart consistency).
    pub fn reset(&mut self) {
        match self {
            Self::SeededNoise {
                base: _,
                std_dev: _,
                seed,
                draw_count,
                rng,
            } => {
                *rng = ChaCha8Rng::from_seed(*seed);
                *draw_count = 0;
            }
            Self::Shared {
                data: _,
                cursor,
                boundary: _,
            } => {
                *cursor = 0;
            }
            Self::Constant(_)
            | Self::DailyProfile { .. }
            | Self::ColumnRef { .. }
            | Self::SolarAware { .. }
            | Self::TimeWindows { .. } => {}
        }
    }
}

fn resolve_index(raw_idx: i64, len: usize, boundary: BoundaryPolicy) -> Result<usize, HaresError> {
    if len == 0 {
        return Err(HaresError::Equipment(
            "schedule source data is empty".to_string(),
        ));
    }

    let len_i64 = len as i64;
    let idx = match boundary {
        BoundaryPolicy::Clamp => raw_idx.clamp(0, len_i64 - 1),
        BoundaryPolicy::Wrap => raw_idx.rem_euclid(len_i64),
        BoundaryPolicy::Error => {
            if raw_idx < 0 || raw_idx >= len_i64 {
                return Err(HaresError::Equipment(format!(
                    "schedule index out of bounds: idx={} len={}",
                    raw_idx, len
                )));
            }
            raw_idx
        }
    };

    Ok(idx as usize)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::{FixedOffset, TimeZone};
    use rand::SeedableRng;
    use rand_chacha::ChaCha8Rng;

    use crate::{DomainUpdate, ZoneId, test_utils::default_env};

    use super::{BoundaryPolicy, DayFilter, SCHEDULE_DOMAIN_ID, ScheduleSource, TimeWindow};

    #[test]
    fn constant_returns_constant() {
        let env = default_env();
        let mut source = ScheduleSource::Constant(7.25);
        assert_eq!(
            source.value_at(&env).expect("constant should resolve"),
            7.25
        );
    }

    #[test]
    fn daily_profile_varies_by_hour_weekday_and_month() {
        let mut env = default_env();
        let mut weekday = [0.0; 24];
        weekday[14] = 1.5;
        let mut weekend = [0.0; 24];
        weekend[14] = 0.75;
        let mut month = [1.0; 12];
        month[0] = 2.0;
        month[1] = 0.5;

        let mut source = ScheduleSource::DailyProfile {
            weekday,
            weekend,
            month_multipliers: month,
            max_value: 10.0,
        };

        let utc = FixedOffset::east_opt(0).expect("offset");
        env.current_time = utc
            .with_ymd_and_hms(2026, 1, 14, 14, 0, 0)
            .single()
            .expect("valid timestamp");
        assert_eq!(source.value_at(&env).expect("weekday value"), 30.0);

        env.current_time = utc
            .with_ymd_and_hms(2026, 1, 17, 14, 0, 0)
            .single()
            .expect("valid timestamp");
        assert_eq!(source.value_at(&env).expect("weekend value"), 15.0);

        // 2026-02-10 is a Tuesday — exercises the weekday+month_multiplier path.
        env.current_time = utc
            .with_ymd_and_hms(2026, 2, 10, 14, 0, 0)
            .single()
            .expect("valid timestamp");
        assert_eq!(source.value_at(&env).expect("month value"), 7.5);
    }

    #[test]
    fn column_ref_reads_schedule_domain_payload() {
        let mut env = default_env();
        env.custom_domains = vec![DomainUpdate {
            domain_id: SCHEDULE_DOMAIN_ID,
            zone_temperatures_c: vec![(ZoneId(1), 21.0)],
            custom_payload: Some(vec![2.0, 4.0, 6.0]),
        }];

        let mut source = ScheduleSource::ColumnRef {
            col_idx: 1,
            boundary: BoundaryPolicy::Clamp,
        };

        assert_eq!(source.value_at(&env).expect("column value"), 4.0);
    }

    #[test]
    fn boundary_policy_wrap_wraps() {
        let env = default_env();
        let data: Arc<[f64]> = Arc::from(vec![10.0, 20.0, 30.0]);
        let mut source = ScheduleSource::Shared {
            data,
            cursor: 5,
            boundary: BoundaryPolicy::Wrap,
        };

        assert_eq!(source.value_at(&env).expect("wrapped value"), 30.0);
    }

    #[test]
    fn boundary_policy_error_errors_when_out_of_bounds() {
        let env = default_env();
        let data: Arc<[f64]> = Arc::from(vec![10.0, 20.0]);
        let mut source = ScheduleSource::Shared {
            data,
            cursor: 5,
            boundary: BoundaryPolicy::Error,
        };

        let err = source
            .value_at(&env)
            .expect_err("out-of-bounds should error");
        assert!(
            err.to_string().contains("out of bounds"),
            "expected out-of-bounds error, got: {err}"
        );
    }

    #[test]
    fn seeded_noise_is_deterministic_given_seed() {
        let env = default_env();
        let seed = [7_u8; 32];

        let mut a = ScheduleSource::SeededNoise {
            base: 5.0,
            std_dev: 1.2,
            seed,
            draw_count: 0,
            rng: ChaCha8Rng::from_seed(seed),
        };
        let mut b = ScheduleSource::SeededNoise {
            base: 5.0,
            std_dev: 1.2,
            seed,
            draw_count: 0,
            rng: ChaCha8Rng::from_seed(seed),
        };

        let mut values = Vec::with_capacity(8);
        for _ in 0..8 {
            let va = a.value_at(&env).expect("value a");
            let vb = b.value_at(&env).expect("value b");
            assert_eq!(va, vb);
            values.push(va);
        }

        // reset() must rewind the RNG to the beginning of the sequence.
        a.reset();
        assert_eq!(
            a.value_at(&env).expect("value a after reset"),
            values[0],
            "reset() did not rewind to the first draw"
        );
    }

    #[test]
    fn shared_cursor_advances() {
        let env = default_env();
        let data: Arc<[f64]> = Arc::from(vec![1.0, 2.0, 3.0]);
        let mut source = ScheduleSource::Shared {
            data,
            cursor: 0,
            boundary: BoundaryPolicy::Clamp,
        };

        assert_eq!(source.value_at(&env).expect("first"), 1.0);
        assert_eq!(source.value_at(&env).expect("second"), 2.0);
        assert_eq!(source.value_at(&env).expect("third"), 3.0);
        assert_eq!(source.value_at(&env).expect("clamped"), 3.0);
    }

    // ── TimeWindows tests ──────────────────────────────────────────

    #[test]
    fn day_filter_matches_correctly() {
        use chrono::Weekday;

        assert!(DayFilter::Any.matches(Weekday::Mon));
        assert!(DayFilter::Any.matches(Weekday::Sat));

        assert!(DayFilter::Weekdays.matches(Weekday::Mon));
        assert!(DayFilter::Weekdays.matches(Weekday::Fri));
        assert!(!DayFilter::Weekdays.matches(Weekday::Sat));
        assert!(!DayFilter::Weekdays.matches(Weekday::Sun));

        assert!(!DayFilter::Weekends.matches(Weekday::Mon));
        assert!(DayFilter::Weekends.matches(Weekday::Sat));
        assert!(DayFilter::Weekends.matches(Weekday::Sun));

        assert!(DayFilter::Day(Weekday::Wed).matches(Weekday::Wed));
        assert!(!DayFilter::Day(Weekday::Wed).matches(Weekday::Thu));
    }

    #[test]
    fn time_window_contains_normal_range() {
        use chrono::Weekday;

        // 07:00–13:00 on any day
        let w = TimeWindow::new(DayFilter::Any, 420, 780, 21.0);

        assert!(!w.contains(Weekday::Mon, 419)); // 06:59
        assert!(w.contains(Weekday::Mon, 420)); // 07:00 inclusive
        assert!(w.contains(Weekday::Mon, 600)); // 10:00
        assert!(!w.contains(Weekday::Mon, 780)); // 13:00 exclusive
    }

    #[test]
    fn time_window_contains_midnight_wrap() {
        use chrono::Weekday;

        // 22:00–06:00 wrapping midnight
        let w = TimeWindow::new(DayFilter::Any, 1320, 360, 18.0);

        assert!(w.contains(Weekday::Tue, 1320)); // 22:00 inclusive
        assert!(w.contains(Weekday::Tue, 1439)); // 23:59
        assert!(w.contains(Weekday::Tue, 0)); // 00:00
        assert!(w.contains(Weekday::Tue, 359)); // 05:59
        assert!(!w.contains(Weekday::Tue, 360)); // 06:00 exclusive
        assert!(!w.contains(Weekday::Tue, 720)); // 12:00
    }

    #[test]
    fn time_windows_first_match_wins() {
        // default_env is 2026-01-01 00:00 UTC = Thursday
        let mut env = default_env();
        let utc = FixedOffset::east_opt(0).expect("offset");

        // Set to Thursday 10:30
        env.current_time = utc
            .with_ymd_and_hms(2026, 1, 1, 10, 30, 0)
            .single()
            .expect("valid timestamp");

        let mut source = ScheduleSource::TimeWindows {
            windows: vec![
                // Weekdays 07:00–13:00 → 21.0
                TimeWindow::new(DayFilter::Weekdays, 420, 780, 21.0),
                // Any day 00:00–23:59 → 18.0 (catch-all, lower priority)
                TimeWindow::new(DayFilter::Any, 0, 1440, 18.0),
            ],
            default: None,
        };

        assert_eq!(
            source.value_at(&env).expect("should match weekday window"),
            21.0
        );
    }

    #[test]
    fn time_windows_falls_through_to_later_window() {
        let mut env = default_env();
        let utc = FixedOffset::east_opt(0).expect("offset");

        // Saturday 10:30
        env.current_time = utc
            .with_ymd_and_hms(2026, 1, 3, 10, 30, 0)
            .single()
            .expect("valid timestamp");

        let mut source = ScheduleSource::TimeWindows {
            windows: vec![
                // Weekdays only → won't match Saturday
                TimeWindow::new(DayFilter::Weekdays, 420, 780, 21.0),
                // Any day catch-all
                TimeWindow::new(DayFilter::Any, 0, 1440, 18.0),
            ],
            default: None,
        };

        assert_eq!(
            source.value_at(&env).expect("should fall through to Any"),
            18.0
        );
    }

    #[test]
    fn time_windows_uses_default_when_no_match() {
        let mut env = default_env();
        let utc = FixedOffset::east_opt(0).expect("offset");

        // Thursday 03:00
        env.current_time = utc
            .with_ymd_and_hms(2026, 1, 1, 3, 0, 0)
            .single()
            .expect("valid timestamp");

        let mut source = ScheduleSource::TimeWindows {
            windows: vec![
                // Only 07:00–13:00
                TimeWindow::new(DayFilter::Any, 420, 780, 21.0),
            ],
            default: Some(15.0),
        };

        assert_eq!(source.value_at(&env).expect("should use default"), 15.0);
    }

    #[test]
    fn time_windows_errors_without_match_or_default() {
        let mut env = default_env();
        let utc = FixedOffset::east_opt(0).expect("offset");

        // Thursday 03:00
        env.current_time = utc
            .with_ymd_and_hms(2026, 1, 1, 3, 0, 0)
            .single()
            .expect("valid timestamp");

        let mut source = ScheduleSource::TimeWindows {
            windows: vec![TimeWindow::new(DayFilter::Any, 420, 780, 21.0)],
            default: None,
        };

        let err = source
            .value_at(&env)
            .expect_err("should error with no match and no default");
        assert!(
            err.to_string().contains("no time window matches"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn time_windows_specific_day_filter() {
        let mut env = default_env();
        let utc = FixedOffset::east_opt(0).expect("offset");

        // 2026-01-05 is a Monday
        env.current_time = utc
            .with_ymd_and_hms(2026, 1, 5, 8, 0, 0)
            .single()
            .expect("valid timestamp");

        let mut source = ScheduleSource::TimeWindows {
            windows: vec![
                // Monday 00:00–07:00 → 20.5
                TimeWindow::new(DayFilter::Day(chrono::Weekday::Mon), 0, 420, 20.5),
                // Monday 07:00–13:00 → 22.8
                TimeWindow::new(DayFilter::Day(chrono::Weekday::Mon), 420, 780, 22.8),
            ],
            default: Some(19.0),
        };

        // 08:00 Monday → should hit second window
        assert_eq!(source.value_at(&env).expect("Monday 08:00"), 22.8);

        // 06:00 Monday → should hit first window
        env.current_time = utc
            .with_ymd_and_hms(2026, 1, 5, 6, 0, 0)
            .single()
            .expect("valid timestamp");
        assert_eq!(source.value_at(&env).expect("Monday 06:00"), 20.5);

        // Tuesday 08:00 → neither window matches, use default
        env.current_time = utc
            .with_ymd_and_hms(2026, 1, 6, 8, 0, 0)
            .single()
            .expect("valid timestamp");
        assert_eq!(source.value_at(&env).expect("Tuesday 08:00"), 19.0);
    }

    #[test]
    fn time_windows_midnight_wrap_integration() {
        let mut env = default_env();
        let utc = FixedOffset::east_opt(0).expect("offset");

        let mut source = ScheduleSource::TimeWindows {
            windows: vec![
                // Overnight setback: 22:00–06:00 → 18.0
                TimeWindow::new(DayFilter::Any, 1320, 360, 18.0),
                // Daytime: 06:00–22:00 → 22.0
                TimeWindow::new(DayFilter::Any, 360, 1320, 22.0),
            ],
            default: None,
        };

        // 23:00 → overnight
        env.current_time = utc
            .with_ymd_and_hms(2026, 1, 1, 23, 0, 0)
            .single()
            .expect("valid timestamp");
        assert_eq!(source.value_at(&env).expect("23:00"), 18.0);

        // 02:00 → overnight
        env.current_time = utc
            .with_ymd_and_hms(2026, 1, 1, 2, 0, 0)
            .single()
            .expect("valid timestamp");
        assert_eq!(source.value_at(&env).expect("02:00"), 18.0);

        // 12:00 → daytime
        env.current_time = utc
            .with_ymd_and_hms(2026, 1, 1, 12, 0, 0)
            .single()
            .expect("valid timestamp");
        assert_eq!(source.value_at(&env).expect("12:00"), 22.0);
    }

    #[test]
    fn time_window_full_day_with_1440() {
        use chrono::Weekday;

        let w = TimeWindow::new(DayFilter::Any, 0, 1440, 20.0);
        assert!(w.contains(Weekday::Mon, 0));
        assert!(w.contains(Weekday::Mon, 720));
        assert!(w.contains(Weekday::Mon, 1439));
    }

    #[test]
    fn time_window_midnight_wrap_with_day_filter() {
        use chrono::Weekday;

        // Monday 22:00 – Tuesday 06:00
        let w = TimeWindow::new(DayFilter::Day(Weekday::Mon), 1320, 360, 18.0);

        // Monday 23:00 → anchor day matches
        assert!(w.contains(Weekday::Mon, 1380));
        // Tuesday 03:00 → post-midnight, predecessor is Monday
        assert!(w.contains(Weekday::Tue, 180));
        // Wednesday 03:00 → predecessor is Tuesday, not Monday
        assert!(!w.contains(Weekday::Wed, 180));
        // Monday 12:00 → outside the time range
        assert!(!w.contains(Weekday::Mon, 720));
    }

    #[test]
    fn time_window_midnight_wrap_weekday_to_weekend_boundary() {
        use chrono::Weekday;

        // Friday 22:00 – Saturday 06:00 with Weekdays filter
        let w = TimeWindow::new(DayFilter::Weekdays, 1320, 360, 18.0);

        // Friday 23:00 → weekday, matches
        assert!(w.contains(Weekday::Fri, 1380));
        // Saturday 03:00 → pred is Friday (weekday), matches
        assert!(w.contains(Weekday::Sat, 180));
        // Sunday 03:00 → pred is Saturday (weekend), no match
        assert!(!w.contains(Weekday::Sun, 180));
    }

    #[test]
    fn time_windows_empty_windows_with_no_default_errors() {
        let env = default_env();
        let mut source = ScheduleSource::TimeWindows {
            windows: vec![],
            default: None,
        };

        let err = source
            .value_at(&env)
            .expect_err("empty windows + no default should error");
        assert!(err.to_string().contains("no time window matches"));
    }

    #[test]
    fn time_windows_empty_windows_with_default() {
        let env = default_env();
        let mut source = ScheduleSource::TimeWindows {
            windows: vec![],
            default: Some(17.0),
        };

        assert_eq!(source.value_at(&env).expect("should use default"), 17.0);
    }

    #[test]
    #[should_panic(expected = "zero-width")]
    fn time_window_zero_width_panics_in_debug() {
        TimeWindow::new(DayFilter::Any, 420, 420, 21.0);
    }

    #[test]
    fn time_windows_weekday_weekend_split() {
        let mut env = default_env();
        let utc = FixedOffset::east_opt(0).expect("offset");

        let mut source = ScheduleSource::TimeWindows {
            windows: vec![
                TimeWindow::new(DayFilter::Weekdays, 0, 1440, 21.0),
                TimeWindow::new(DayFilter::Weekends, 0, 1440, 24.0),
            ],
            default: None,
        };

        // Thursday (weekday)
        env.current_time = utc
            .with_ymd_and_hms(2026, 1, 1, 12, 0, 0)
            .single()
            .expect("valid timestamp");
        assert_eq!(source.value_at(&env).expect("weekday"), 21.0);

        // Saturday (weekend)
        env.current_time = utc
            .with_ymd_and_hms(2026, 1, 3, 12, 0, 0)
            .single()
            .expect("valid timestamp");
        assert_eq!(source.value_at(&env).expect("weekend"), 24.0);
    }
}
