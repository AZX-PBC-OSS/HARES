//! Generate a ScheduleTimeSeries for an HPXML home when no schedule CSV is
//! available. Uses default schedule fraction profiles to produce time-varying
//! occupancy, power and water columns.

use std::collections::HashMap;

use chrono::{DateTime, Datelike, Duration, FixedOffset, Timelike};
use hares_types::HaresError;

use crate::schedule::{ColumnAggregation, ScheduleTimeSeries};
use crate::schedule_resolve::{DefaultProfiles, DefaultScheduleProfile};

/// Mapping from generated schedule column names to the OCHRE profile names used
/// in the Default Schedule Parameters.csv.
const COLUMN_TO_PROFILE: &[(&str, &str)] = &[
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
/// the `Occupancy` profile). A missing profile is an error naming the profile
/// and the file, so a column is never a silent constant. No setpoint column
/// is generated: a schedule column would override the HVAC's own setpoints,
/// which come from the HPXML's hourly weekday and weekend profiles, or
/// OS-HPXML's default when the HPXML has none
/// (`schedule_resolve::inject_setpoint_schedules`).
pub fn generate_default_schedule(
    start: DateTime<FixedOffset>,
    duration: Duration,
    interval: Duration,
    profiles: &DefaultProfiles,
) -> Result<ScheduleTimeSeries, HaresError> {
    let n_steps = (duration.num_seconds() / interval.num_seconds()).max(1) as usize;
    let timestamps: Vec<DateTime<FixedOffset>> = (0..n_steps)
        .map(|i| start + Duration::seconds(i as i64 * interval.num_seconds()))
        .collect();

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
