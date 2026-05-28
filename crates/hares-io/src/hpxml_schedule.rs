//! Generate a ScheduleTimeSeries from HPXML building data when no schedule CSV is
//! available. Uses default schedule fraction profiles to produce time-varying
//! occupancy, power, water, and setpoint columns.

use std::collections::HashMap;
use std::path::Path;

use chrono::{Datelike, DateTime, Duration, FixedOffset, Timelike};

use crate::hpxml::building::Building;
use crate::schedule::{ColumnAggregation, ScheduleTimeSeries};
use crate::schedule_resolve::load_default_profiles;

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
    ("hot_water_fixtures", "Water Fixtures"),
];

/// Generate a complete schedule from HPXML building data and default profiles.
///
/// Columns are generated in the same order and with the same names as a standard
/// ResStock `in.schedules.csv`. Power and water columns use the weekday/weekend
/// fraction profiles from the defaults CSV; setpoints use HPXML values falling
/// back to HERS reference-home defaults; occupancy is constant 1.0.
pub fn generate_schedule_from_hpxml(
    building: &Building,
    start: DateTime<FixedOffset>,
    duration: Duration,
    interval: Duration,
    defaults_dir: Option<&Path>,
) -> ScheduleTimeSeries {
    let profiles = defaults_dir.map(load_default_profiles).unwrap_or_default();
    let n_steps = (duration.num_seconds() / interval.num_seconds()).max(1) as usize;
    let timestamps: Vec<DateTime<FixedOffset>> = (0..n_steps)
        .map(|i| start + Duration::seconds(i as i64 * interval.num_seconds()))
        .collect();

    let mut column_names: Vec<String> = Vec::new();
    let mut columns: Vec<Vec<f64>> = Vec::new();

    // Occupancy — constant 1.0
    column_names.push("occupants".to_string());
    columns.push(vec![1.0; n_steps]);

    // Power and water schedule columns — use default fraction profiles when available
    for (col_name, profile_name) in COLUMN_TO_PROFILE {
        let values = if let Some(profile) = profiles.get(*profile_name) {
            generate_profile_column(n_steps, &timestamps, profile)
        } else {
            vec![1.0; n_steps]
        };
        column_names.push(col_name.to_string());
        columns.push(values);
    }

    // Heating setpoint from HPXML building data
    let heating_sp = building
        .heating_weekday_setpoints_c
        .as_ref()
        .map(|v| v[0])
        .unwrap_or(20.0);
    column_names.push("heating_setpoint".to_string());
    columns.push(vec![heating_sp; n_steps]);

    // Cooling setpoint from HPXML building data
    let cooling_sp = building
        .cooling_weekday_setpoints_c
        .as_ref()
        .map(|v| v[0])
        .unwrap_or(24.0);
    column_names.push("cooling_setpoint".to_string());
    columns.push(vec![cooling_sp; n_steps]);

    let column_index: HashMap<String, usize> = column_names
        .iter()
        .enumerate()
        .map(|(i, name)| (name.clone(), i))
        .collect();

    let n_cols = column_index.len();
    ScheduleTimeSeries {
        timestamps,
        column_names,
        columns,
        column_index,
        source_step_secs: interval.num_seconds() as u32,
        column_aggregations: vec![ColumnAggregation::Mean; n_cols],
    }
}

fn generate_profile_column(
    n_steps: usize,
    timestamps: &[DateTime<FixedOffset>],
    profile: &crate::schedule_resolve::DefaultScheduleProfile,
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
