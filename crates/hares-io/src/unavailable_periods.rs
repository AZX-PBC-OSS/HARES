//! OpenStudio-HPXML unavailable periods.
//!
//! Implements the slice of OpenStudio-HPXML v1.12.0 the schedule columns
//! need: the `SoftwareInfo/extension/UnavailablePeriods` input
//! (`HPXMLtoOpenStudio/resources/hpxml.rb:1365, 1387-1448`, the defaults at
//! `resources/defaults.rb:229-241`, the date checks at `hpxml.rb:1410` and
//! `hpxml.rb:12514-12540`), the end-use table
//! (`resources/data/unavailable_periods.csv:1-40`: a 1 in the period's column
//! zeroes that row's schedule on the period's steps), the half-open period
//! rule with the year-end wrap (`resources/schedules.rb:702-724` and the
//! timestep fill at `:1375-1408`), and the two-step column lookup
//! `set_unavailable_periods` uses (`schedules.rb:1433-1437`): the schedule
//! column's name is remapped to its energy-use row
//! (`HotWaterDishwasher` to `Dishwasher`, `HotWaterClothesWasher` to
//! `ClothesWasher`) before the table is consulted.

use chrono::{Datelike, Timelike};

use crate::hpxml::HpxmlError;
use crate::hpxml::building::XmlNode;
use crate::hpxml::xml_helpers::{child_f64, child_text, element_id};
use crate::schedule::ScheduleTimeSeries;
use hares_types::HaresError;

/// The four unavailable-period types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnavailableKind {
    Vacancy,
    PowerOutage,
    NoSpaceHeating,
    NoSpaceCooling,
}

impl UnavailableKind {
    /// The HPXML `ColumnName` spelling.
    pub fn column_name(self) -> &'static str {
        match self {
            Self::Vacancy => "Vacancy",
            Self::PowerOutage => "Power Outage",
            Self::NoSpaceHeating => "No Space Heating",
            Self::NoSpaceCooling => "No Space Cooling",
        }
    }

    pub(crate) fn parse(column: &str) -> Option<Self> {
        match column {
            "Vacancy" => Some(Self::Vacancy),
            "Power Outage" => Some(Self::PowerOutage),
            "No Space Heating" => Some(Self::NoSpaceHeating),
            "No Space Cooling" => Some(Self::NoSpaceCooling),
            _ => None,
        }
    }
}

/// What happens to natural ventilation during the period.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NaturalVentilation {
    RegularSchedule,
    AlwaysAvailable,
    AlwaysUnavailable,
}

impl NaturalVentilation {
    fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "regular schedule" => Some(Self::RegularSchedule),
            "always available" => Some(Self::AlwaysAvailable),
            "always unavailable" => Some(Self::AlwaysUnavailable),
            _ => None,
        }
    }
}

/// One unavailable period: the half-open interval from
/// `begin_month`/`begin_day` at `begin_hour` to `end_month`/`end_day` at
/// `end_hour`, both dates inclusive as days; an end before the begin wraps
/// through the year end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnavailablePeriod {
    pub kind: UnavailableKind,
    pub begin_month: u32,
    pub begin_day: u32,
    pub begin_hour: u32,
    pub end_month: u32,
    pub end_day: u32,
    pub end_hour: u32,
    pub natural_ventilation: NaturalVentilation,
}

/// One row of OpenStudio-HPXML's `unavailable_periods.csv`: the schedule
/// name and the four per-type flags.
struct UnavailablePeriodRow {
    schedule_name: &'static str,
    vacancy: bool,
    power_outage: bool,
    no_space_heating: bool,
    no_space_cooling: bool,
}

/// OpenStudio-HPXML v1.12.0 `resources/data/unavailable_periods.csv:1-40`.
const UNAVAILABLE_PERIOD_ROWS: &[UnavailablePeriodRow] = &[
    row("occupants", true, false, false, false),
    row("lighting_interior", true, true, false, false),
    row("lighting_exterior", true, true, false, false),
    row("lighting_garage", true, true, false, false),
    row("lighting_exterior_holiday", true, true, false, false),
    row("cooking_range", true, true, false, false),
    row("refrigerator", false, true, false, false),
    row("extra_refrigerator", false, true, false, false),
    row("freezer", false, true, false, false),
    row("dishwasher", true, true, false, false),
    row("clothes_washer", true, true, false, false),
    row("clothes_dryer", true, true, false, false),
    row("ceiling_fan", true, true, false, false),
    row("plug_loads_other", true, true, false, false),
    row("plug_loads_tv", true, true, false, false),
    row("plug_loads_vehicle", true, true, false, false),
    row("plug_loads_well_pump", true, true, false, false),
    row("fuel_loads_grill", true, true, false, false),
    row("fuel_loads_lighting", true, true, false, false),
    row("fuel_loads_fireplace", true, true, false, false),
    row("pool_pump", true, true, false, false),
    row("pool_heater", true, true, false, false),
    row("permanent_spa_pump", true, true, false, false),
    row("permanent_spa_heater", true, true, false, false),
    row("hot_water_fixtures", true, true, false, false),
    row("hot_water_recirculation_pump", true, true, false, false),
    row("general_water_use", true, false, false, false),
    row("space_heating", false, true, true, false),
    row("space_cooling", false, true, false, true),
    row("water_heater", false, true, false, false),
    row("dehumidifier", false, true, false, false),
    row("house_fan", false, true, false, false),
    row("kitchen_fan", true, true, false, false),
    row("bath_fan", true, true, false, false),
    row("whole_house_fan", true, true, false, false),
    row("battery_charging", false, true, false, false),
    row("battery_discharging", false, true, false, false),
    row("electric_vehicle_charging", true, true, false, false),
    row("electric_vehicle_discharging", true, true, false, false),
];

const fn row(
    schedule_name: &'static str,
    vacancy: bool,
    power_outage: bool,
    no_space_heating: bool,
    no_space_cooling: bool,
) -> UnavailablePeriodRow {
    UnavailablePeriodRow {
        schedule_name,
        vacancy,
        power_outage,
        no_space_heating,
        no_space_cooling,
    }
}

/// The table row a schedule column answers to, through the two-step lookup:
/// the column's name is remapped to its paired energy-use row's name before
/// the table is consulted.
fn unavailable_period_row_for_column(column: &str) -> Option<&'static UnavailablePeriodRow> {
    let remapped = match column {
        "hot_water_dishwasher" => "dishwasher",
        "hot_water_clothes_washer" => "clothes_washer",
        other => other,
    };
    UNAVAILABLE_PERIOD_ROWS
        .iter()
        .find(|row| row.schedule_name == remapped)
}

/// The day-of-year of a month and day on a non-leap calendar, `None` when the
/// date does not exist.
fn day_ordinal(month: u32, day: u32) -> Option<u32> {
    chrono::NaiveDate::from_ymd_opt(2007, month, day).map(|d| d.ordinal())
}

/// Whether the (day-of-year, hour) falls in the period's half-open interval,
/// with an end before the begin wrapping through the year end.
fn flagged_at(period: &UnavailablePeriod, day_ordinal_of_row: u32, hour: u32) -> bool {
    let begin = day_ordinal(period.begin_month, period.begin_day).expect("validated month and day");
    let end = day_ordinal(period.end_month, period.end_day).expect("validated month and day");
    let after_begin =
        day_ordinal_of_row > begin || (day_ordinal_of_row == begin && hour >= period.begin_hour);
    let before_end =
        day_ordinal_of_row < end || (day_ordinal_of_row == end && hour < period.end_hour);
    if end >= begin {
        after_begin && before_end
    } else {
        after_begin || before_end
    }
}

/// The per-row flags of a schedule: row `i` is flagged when its calendar hour
/// (the row timestamp's day-of-year and hour-of-day, the local standard time
/// the schedules read) falls in any period.
fn flagged_rows(schedule: &ScheduleTimeSeries, periods: &[UnavailablePeriod]) -> Vec<bool> {
    let mut flags = vec![false; schedule.len()];
    for (i, timestamp) in schedule.timestamps.iter().enumerate() {
        let ordinal = timestamp.ordinal();
        let hour = timestamp.hour();
        flags[i] = periods
            .iter()
            .any(|period| flagged_at(period, ordinal, hour));
    }
    flags
}

/// Zero every schedule column whose `unavailable_periods.csv` row a period
/// applies to, on the period's flagged steps, at the schedule's own
/// resolution. The schedule columns that carry setpoints or temperatures
/// have no table row and are left alone.
pub fn apply_unavailable_periods(
    schedule: &mut ScheduleTimeSeries,
    periods: &[UnavailablePeriod],
) -> Result<(), HaresError> {
    if periods.is_empty() {
        return Ok(());
    }
    let flags = flagged_rows(schedule, periods);
    for period in periods {
        let count = flags.iter().filter(|f| **f).count();
        let first = flags.iter().position(|f| *f);
        let last = flags.iter().rposition(|f| *f);
        tracing::debug!(
            period = period.kind.column_name(),
            flagged_rows = count,
            first_flagged_row = first,
            last_flagged_row = last,
            "unavailable period flags computed",
        );
    }
    let mut zeroed: Vec<&str> = Vec::new();
    for (col_idx, name) in schedule.column_names.iter().enumerate() {
        let Some(row) = unavailable_period_row_for_column(name) else {
            continue;
        };
        let applies = periods.iter().any(|period| match period.kind {
            UnavailableKind::Vacancy => row.vacancy,
            UnavailableKind::PowerOutage => row.power_outage,
            UnavailableKind::NoSpaceHeating => row.no_space_heating,
            UnavailableKind::NoSpaceCooling => row.no_space_cooling,
        });
        if !applies {
            continue;
        }
        let mut touched = false;
        for (i, flagged) in flags.iter().enumerate() {
            if *flagged {
                schedule.columns[col_idx][i] = 0.0;
                touched = true;
            }
        }
        if touched {
            zeroed.push(name);
        }
    }
    tracing::debug!(
        columns = ?zeroed,
        periods = ?periods
            .iter()
            .map(|p| p.kind.column_name())
            .collect::<Vec<_>>(),
        "unavailable periods applied to the schedule columns",
    );
    Ok(())
}

/// Parse `SoftwareInfo/extension/UnavailablePeriods` with OpenStudio-HPXML's
/// defaults (`BeginHourOfDay` 0, `EndHourOfDay` 24, `NaturalVentilation`
/// `regular schedule`) and checks: an unknown `ColumnName`, an hour outside
/// its range, a missing required field, or a date that does not exist fails
/// the parse naming the period.
pub(crate) fn parse_unavailable_periods(
    root: &XmlNode,
) -> Result<Vec<UnavailablePeriod>, HpxmlError> {
    let Some(periods_node) = root.path(&["SoftwareInfo", "extension", "UnavailablePeriods"]) else {
        return Ok(Vec::new());
    };
    let mut periods = Vec::new();
    for node in periods_node.children_named("UnavailablePeriod") {
        let id = element_id(node).unwrap_or_else(|| "unknown".to_string());
        let require_f64 = |name: &'static str| -> Result<f64, HpxmlError> {
            child_f64(node, name).ok_or_else(|| HpxmlError::MissingField {
                path: "SoftwareInfo/extension/UnavailablePeriods/UnavailablePeriod",
                system_kind: "Unavailable Period",
                system_id: id.clone(),
                reason: "the period's begin and end dates are required",
            })
        };
        let column = child_text(node, "ColumnName").ok_or_else(|| HpxmlError::MissingField {
            path: "SoftwareInfo/extension/UnavailablePeriods/UnavailablePeriod/ColumnName",
            system_kind: "Unavailable Period",
            system_id: id.clone(),
            reason: "the period's schedule column names its type; one of Vacancy, \
                     Power Outage, No Space Heating, No Space Cooling is required",
        })?;
        let kind =
            UnavailableKind::parse(column.trim()).ok_or_else(|| HpxmlError::InvalidField {
                path: "SoftwareInfo/extension/UnavailablePeriods/UnavailablePeriod/ColumnName",
                system_kind: "Unavailable Period",
                system_id: id.clone(),
                value_received: column.trim().to_string(),
                reason: "the four period types are Vacancy, Power Outage, \
                         No Space Heating, No Space Cooling (the table's \
                         Schedule Name values)",
            })?;
        let begin_month = require_f64("BeginMonth")? as u32;
        let begin_day = require_f64("BeginDayOfMonth")? as u32;
        let end_month = require_f64("EndMonth")? as u32;
        let end_day = require_f64("EndDayOfMonth")? as u32;
        let begin_hour = child_f64(node, "BeginHourOfDay").unwrap_or(0.0) as u32;
        let end_hour = child_f64(node, "EndHourOfDay").unwrap_or(24.0) as u32;

        if !(1..=12).contains(&begin_month) || !(1..=12).contains(&end_month) {
            return Err(HpxmlError::InvalidField {
                path: "UnavailablePeriod/{BeginMonth,EndMonth}",
                system_kind: "Unavailable Period",
                system_id: id,
                value_received: format!("{begin_month}, {end_month}"),
                reason: "months must be one of 1 to 12",
            });
        }
        for (month, day, name) in [
            (begin_month, begin_day, "BeginDayOfMonth"),
            (end_month, end_day, "EndDayOfMonth"),
        ] {
            if day_ordinal(month, day).is_none() {
                return Err(HpxmlError::InvalidField {
                    path: "UnavailablePeriod/{BeginDayOfMonth,EndDayOfMonth}",
                    system_kind: "Unavailable Period",
                    system_id: id,
                    value_received: format!("{month}/{day} ({name})"),
                    reason: "the date does not exist on a non-leap calendar; the day \
                             must be one the month has",
                });
            }
        }
        if begin_hour > 23 {
            return Err(HpxmlError::InvalidField {
                path: "UnavailablePeriod/BeginHourOfDay",
                system_kind: "Unavailable Period",
                system_id: id,
                value_received: begin_hour.to_string(),
                reason: "the begin hour must be 0 to 23",
            });
        }
        if !(1..=24).contains(&end_hour) {
            return Err(HpxmlError::InvalidField {
                path: "UnavailablePeriod/EndHourOfDay",
                system_kind: "Unavailable Period",
                system_id: id,
                value_received: end_hour.to_string(),
                reason: "the end hour must be 1 to 24",
            });
        }
        let natural_ventilation = match child_text(node, "NaturalVentilation") {
            None => NaturalVentilation::RegularSchedule,
            Some(raw) => {
                NaturalVentilation::parse(&raw).ok_or_else(|| HpxmlError::InvalidField {
                    path: "UnavailablePeriod/NaturalVentilation",
                    system_kind: "Unavailable Period",
                    system_id: id,
                    value_received: raw.clone(),
                    reason: "the options are regular schedule, always available, \
                         always unavailable",
                })?
            }
        };
        periods.push(UnavailablePeriod {
            kind,
            begin_month,
            begin_day,
            begin_hour,
            end_month,
            end_day,
            end_hour,
            natural_ventilation,
        });
    }
    Ok(periods)
}
