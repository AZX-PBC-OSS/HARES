//! Generate a ScheduleTimeSeries from HPXML building data when no schedule CSV is
//! available. Uses default schedule fraction profiles to produce time-varying
//! occupancy, power, water, and setpoint columns.

use std::collections::HashMap;

use chrono::{DateTime, Datelike, Duration, FixedOffset, Timelike};
use hares_types::HaresError;

use crate::hpxml::building::Building;
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

/// Generate a complete schedule from HPXML building data and default profiles.
///
/// Columns are generated in the same order and with the same names as a standard
/// ResStock `in.schedules.csv`. Power, water, and occupancy columns use the
/// weekday/weekend fraction profiles from the defaults CSV (occupancy follows
/// the `Occupancy` profile); setpoints use HPXML values falling back to HERS
/// reference-home defaults. A missing profile is an error naming the profile
/// and the file, so a column is never a silent constant.
pub fn generate_schedule_from_hpxml(
    building: &Building,
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

    // Heating setpoint from HPXML building data
    let heating_sp = building
        .heating_weekday_setpoints_c
        .as_ref()
        .map(|v| v[0])
        .unwrap_or(20.0);
    generated.push((
        "heating_setpoint",
        vec![heating_sp; n_steps],
        ColumnAggregation::Mean,
    ));

    // Cooling setpoint from HPXML building data
    let cooling_sp = building
        .cooling_weekday_setpoints_c
        .as_ref()
        .map(|v| v[0])
        .unwrap_or(24.0);
    generated.push((
        "cooling_setpoint",
        vec![cooling_sp; n_steps],
        ColumnAggregation::Mean,
    ));

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
