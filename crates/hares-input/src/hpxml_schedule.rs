//! Generate a ScheduleTimeSeries for an HPXML home when no schedule CSV is
//! available. Uses default schedule fraction profiles to produce time-varying
//! occupancy, power and water columns.

use std::collections::HashMap;

use chrono::{DateTime, Datelike, Duration, FixedOffset, Timelike};
use hares_types::HaresError;

use crate::schedule::{ColumnAggregation, ScheduleTimeSeries};
use crate::schedule_resolve::{DefaultProfiles, DefaultScheduleProfile};

/// Mapping from generated schedule column names to the OCHRE profile names used
/// in the Default Schedule Parameters.csv. The schedule-diversity transform
/// walks the same list's profiles, so a column added here joins the diversity
/// layer or its pin fails.
pub(crate) const COLUMN_TO_PROFILE: &[(&str, &str)] = &[
    ("occupants", "Occupancy"),
    ("plug_loads_other", "MELs"),
    ("plug_loads_tv", "MELs"),
    ("lighting_interior", "Indoor Lighting"),
    ("dishwasher", "Dishwasher"),
    ("clothes_washer", "Clothes Washer"),
    ("clothes_dryer", "Clothes Dryer"),
    ("cooking_range", "Cooking Range"),
    ("hot_water_dishwasher", "Dishwasher"),
    ("hot_water_clothes_washer", "Clothes Washer"),
    ("hot_water_fixtures", "Water Heating"),
];

/// Generate a complete schedule from the default profiles.
///
/// Columns are generated in the same order and with the same names as a standard
/// ResStock `in.schedules.csv`. Power, water, and occupancy columns use the
/// weekday/weekend fraction profiles from the defaults CSV (occupancy follows
/// the `Occupancy` profile; its provenance, the 301-2022 Addendum C default
/// fractions, the vendored file's hash and the citations, is the occupancy
/// provenance contract on `schedule_resolve::inject_occupancy_schedule`). A
/// missing profile is an error naming the profile
/// and the file, so a column is never a silent constant. No setpoint column
/// is generated: a schedule column would override the HVAC's own setpoints,
/// which come from the HPXML's hourly weekday and weekend profiles, or
/// OS-HPXML's default when the HPXML has none
/// (`schedule_resolve::inject_setpoint_schedules`).
///
/// The rows are addressed by calendar date, the CSV path's convention: the
/// environment maps a step's calendar time onto a row through the 365-day
/// ordinal (`hares_types::schedule_day_ordinal`), with row 0 at Jan 1. The
/// table therefore starts at Jan 1 of the start's year (the year's own
/// weekday pattern, Feb 29 skipped) and runs to the run's end: the start's
/// ordinal plus `duration`. A run starting mid-year reads the profile
/// generated for its own calendar dates, never a rotated copy anchored at
/// the run start.
pub fn generate_default_schedule(
    start: DateTime<FixedOffset>,
    duration: Duration,
    interval: Duration,
    profiles: &DefaultProfiles,
) -> Result<ScheduleTimeSeries, HaresError> {
    let step_secs = u64::try_from(interval.num_seconds()).map_err(|_| {
        HaresError::Io(format!(
            "the generated schedule's interval must be a positive number of seconds, \
             got {}s",
            interval.num_seconds()
        ))
    })?;
    if step_secs == 0 {
        return Err(HaresError::Io(
            "the generated schedule's interval must be at least one second".to_string(),
        ));
    }
    // The run's first step's position on the 365-day calendar the rows are
    // addressed by: the start date's ordinal times the day length plus the
    // start's time of day.
    let start_offset_secs = (u64::from(hares_types::schedule_day_ordinal(&start.date_naive())) - 1)
        * 86_400
        + u64::from(start.num_seconds_from_midnight());
    let duration_secs = u64::try_from(duration.num_seconds()).map_err(|_| {
        HaresError::Io(format!(
            "the generated schedule's duration must be a non-negative number of \
             seconds, got {}s",
            duration.num_seconds()
        ))
    })?;
    let n_steps = ((start_offset_secs + duration_secs).div_ceil(step_secs)).max(1);

    // One real date per 365-day-calendar day of the table: Jan 1 of the
    // start's year on, any Feb 29 skipped, so a row's date is the calendar
    // date the ordinal addressing reads it at (Feb 29 itself maps to the
    // Mar 1 slot, the shared convention).
    let n_days = usize::try_from(n_steps * step_secs / 86_400 + 1).map_err(|_| {
        HaresError::Io("the generated schedule's span overflows the addressable days".to_string())
    })?;
    let offset = *start.offset();
    let mut dates = Vec::with_capacity(n_days);
    let mut day = chrono::NaiveDate::from_ymd_opt(start.year(), 1, 1)
        .ok_or_else(|| HaresError::Io(format!("the start year {} has no Jan 1", start.year())))?;
    for _ in 0..n_days {
        dates.push(day);
        let next = day.succ_opt().ok_or_else(|| {
            HaresError::Io("the generated schedule's calendar overflowed its dates".to_string())
        })?;
        day = if next.month() == 2 && next.day() == 29 {
            next.succ_opt().ok_or_else(|| {
                HaresError::Io("the generated schedule's calendar overflowed its dates".to_string())
            })?
        } else {
            next
        };
    }

    let timestamps: Vec<DateTime<FixedOffset>> = (0..n_steps)
        .map(|i| {
            let secs = i * step_secs;
            let naive = dates[(secs / 86_400) as usize]
                .and_hms_opt(
                    (secs % 86_400 / 3600) as u32,
                    (secs % 3600 / 60) as u32,
                    (secs % 60) as u32,
                )
                .expect("a valid time of day");
            DateTime::from_naive_utc_and_offset(naive - offset, offset)
        })
        .collect();
    let n_steps = usize::try_from(n_steps).map_err(|_| {
        HaresError::Io("the generated schedule's row count exceeds usize".to_string())
    })?;

    // Columns, names, and aggregations are pushed together, one per column,
    // so their lengths cannot differ.
    let mut generated: Vec<(&str, Vec<f64>, ColumnAggregation)> = Vec::new();

    for (col_name, profile_name) in COLUMN_TO_PROFILE {
        let profile = profiles.get(profile_name)?;
        let values = generate_profile_column(n_steps, &timestamps, profile);
        generated.push((col_name, values, ColumnAggregation::Mean));
    }

    let column_names: Vec<String> = generated
        .iter()
        .map(|(name, _, _)| (*name).to_string())
        .collect();
    let column_aggregations: Vec<ColumnAggregation> =
        generated.iter().map(|(_, _, agg)| *agg).collect();
    let columns: Vec<Vec<f64>> = generated.into_iter().map(|(_, values, _)| values).collect();

    let column_index: HashMap<String, usize> = column_names
        .iter()
        .enumerate()
        .map(|(i, name)| (name.clone(), i))
        .collect();

    Ok(ScheduleTimeSeries {
        timestamps,
        column_names,
        columns,
        column_index,
        source_step_secs: interval.num_seconds() as u32,
        column_aggregations,
    })
}

fn generate_profile_column(
    n_steps: usize,
    timestamps: &[DateTime<FixedOffset>],
    profile: &DefaultScheduleProfile,
) -> Vec<f64> {
    let mut values = Vec::with_capacity(n_steps);
    for ts in timestamps {
        let hour = ts.hour() as usize;
        let month = ts.month0() as usize;
        let is_weekend = ts.weekday().num_days_from_monday() >= 5;
        let frac = if is_weekend {
            profile.weekend_fractions[hour]
        } else {
            profile.weekday_fractions[hour]
        };
        values.push(frac * profile.month_multipliers[month]);
    }
    values
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn shipped_profiles() -> DefaultProfiles {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../python/hares/defaults");
        crate::load_default_profiles(&dir).expect("the shipped defaults CSV loads")
    }

    fn utc() -> chrono::FixedOffset {
        chrono::FixedOffset::east_opt(0).expect("offset")
    }

    fn at(year: i32, month: u32, day: u32, hour: u32) -> DateTime<FixedOffset> {
        utc()
            .with_ymd_and_hms(year, month, day, hour, 0, 0)
            .single()
            .expect("a real calendar time")
    }

    /// The generated table starts at Jan 1 of the start's year and runs to
    /// the run's end: the start's ordinal plus the duration, at the step
    /// resolution. A Jul 1 2023 start's first step reads the row whose
    /// timestamp is Jul 1, not row 0.
    #[test]
    fn generated_schedule_is_anchored_at_jan1_of_the_start_year() {
        let start = at(2023, 7, 1, 0);
        let schedule = generate_default_schedule(
            start,
            Duration::hours(24 * 7),
            Duration::seconds(900),
            &shipped_profiles(),
        )
        .expect("the shipped profiles generate the schedule");

        // Jan 1 00:00 through the run's last step: the start's ordinal is
        // 181 days past Jan 1 plus 7 days of duration, 96 steps a day.
        assert_eq!(schedule.timestamps.len(), (181 + 7) * 96);
        assert_eq!(schedule.timestamps[0], at(2023, 1, 1, 0));
        assert_eq!(schedule.timestamps[181 * 96], start);
        assert_eq!(
            schedule.timestamps[schedule.timestamps.len() - 1],
            utc()
                .with_ymd_and_hms(2023, 7, 7, 23, 45, 0)
                .single()
                .expect("time")
        );
        assert_eq!(schedule.source_step_secs, 900);
    }

    /// The generated calendar is a 365-day one: a leap start year's
    /// Feb 29 is skipped, so the row the environment addresses at March 1
    /// carries March 1's own date, and no row lands on Feb 29.
    #[test]
    fn generated_schedule_skips_feb29_of_the_start_year() {
        let start = at(2024, 1, 1, 0);
        let schedule = generate_default_schedule(
            start,
            Duration::hours(24 * 70),
            Duration::seconds(3600),
            &shipped_profiles(),
        )
        .expect("the shipped profiles generate the schedule");

        for ts in &schedule.timestamps {
            assert!(
                !(ts.month() == 2 && ts.day() == 29),
                "the generated calendar must not contain Feb 29, got {ts}"
            );
        }
        // Day 59 of the 365-day calendar is Mar 1 (Feb 29 skipped); the
        // environment addresses that row at Mar 1 (and Feb 29 shares it).
        // Hourly steps: the day's first row is the day index times 24.
        assert_eq!(schedule.timestamps[59 * 24], at(2024, 3, 1, 0));
        assert_eq!(schedule.timestamps[60 * 24], at(2024, 3, 2, 0));
    }

    /// A run spanning Dec 31 into Jan 1 is covered: the rows continue past
    /// the anchor year's Dec 31 into the next calendar year, and the step
    /// at Jan 1 00:00 addresses the schedule's Jan 1 row (the wrap to row
    /// 0 of the calendar, not the run's own first row).
    #[test]
    fn generated_schedule_covers_the_dec31_to_jan1_wrap() {
        let start = at(2023, 12, 31, 0);
        let schedule = generate_default_schedule(
            start,
            Duration::hours(48),
            Duration::seconds(900),
            &shipped_profiles(),
        )
        .expect("the shipped profiles generate the schedule");

        // doy0(Dec 31) is 364 days past Jan 1, plus 2 days of duration.
        assert_eq!(schedule.timestamps.len(), (364 + 2) * 96);
        assert_eq!(schedule.timestamps[364 * 96], at(2023, 12, 31, 0));
        assert_eq!(schedule.timestamps[365 * 96], at(2024, 1, 1, 0));
        assert_eq!(
            schedule.timestamps[schedule.timestamps.len() - 1],
            utc()
                .with_ymd_and_hms(2024, 1, 1, 23, 45, 0)
                .single()
                .expect("time")
        );
    }
}
