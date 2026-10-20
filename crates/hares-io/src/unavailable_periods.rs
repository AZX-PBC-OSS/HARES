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
use crate::hpxml::elements::*;
use crate::hpxml::xml_helpers::{child_f64, child_text, element_id};
use crate::schedule::ScheduleTimeSeries;
use hares_types::{HaresError, UnavailablePeriodState};

pub use hares_types::UnavailableKind;

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

/// The per-row flags of a schedule: row `i` carries the kinds flagging it
/// (the row's 365-day-calendar day, `schedule_day_ordinal` of the row
/// timestamp: the period dates live on the non-leap calendar the schedule
/// rows are indexed by, and the hour-of-day) and the natural-ventilation
/// availability the flagged periods set, the last flagged period with a
/// non-regular option winning.
fn flagged_rows(
    schedule: &ScheduleTimeSeries,
    periods: &[UnavailablePeriod],
) -> Vec<UnavailablePeriodState> {
    let mut flags: Vec<UnavailablePeriodState> =
        vec![UnavailablePeriodState::default(); schedule.len()];
    for period in periods {
        for (i, timestamp) in schedule.timestamps.iter().enumerate() {
            let ordinal = hares_types::schedule_day_ordinal(timestamp);
            let hour = timestamp.hour();
            if !flagged_at(period, ordinal, hour) {
                continue;
            }
            let flag = &mut flags[i];
            match period.kind {
                UnavailableKind::Vacancy => flag.vacancy = true,
                UnavailableKind::PowerOutage => flag.power_outage = true,
                UnavailableKind::NoSpaceHeating => flag.no_space_heating = true,
                UnavailableKind::NoSpaceCooling => flag.no_space_cooling = true,
            }
            if let Some(available) = natural_ventilation_availability(period.natural_ventilation) {
                flag.natural_ventilation = Some(available);
            }
        }
    }
    flags
}

/// The availability a period's `NaturalVentilation` option sets on its
/// flagged steps, `None` when it leaves the regular schedule.
fn natural_ventilation_availability(option: NaturalVentilation) -> Option<bool> {
    match option {
        NaturalVentilation::RegularSchedule => None,
        NaturalVentilation::AlwaysAvailable => Some(true),
        NaturalVentilation::AlwaysUnavailable => Some(false),
    }
}

/// The periods' per-schedule-row flags over a schedule series: row `i`
/// carries the kinds flagging the row and the natural-ventilation
/// availability the flagged periods set. The per-step state the
/// environment publishes beside `bus_energized` reads these by the step's
/// schedule row.
pub fn unavailable_period_flags(
    schedule: &ScheduleTimeSeries,
    periods: &[UnavailablePeriod],
) -> Vec<UnavailablePeriodState> {
    flagged_rows(schedule, periods)
}

/// The table rows the kind zeroes: the `Schedule Name`s whose flag under
/// the kind is 1.
pub fn rows_zeroed_by_kind(kind: UnavailableKind) -> Vec<&'static str> {
    UNAVAILABLE_PERIOD_ROWS
        .iter()
        .filter(|row| match kind {
            UnavailableKind::Vacancy => row.vacancy,
            UnavailableKind::PowerOutage => row.power_outage,
            UnavailableKind::NoSpaceHeating => row.no_space_heating,
            UnavailableKind::NoSpaceCooling => row.no_space_cooling,
        })
        .map(|row| row.schedule_name)
        .collect()
}

/// Whether the named `unavailable_periods.csv` row is zeroed by `kind`
/// (the row's flag for the kind), `None` when the name is no row of the
/// table.
pub fn row_zeroed_by(schedule_name: &str, kind: UnavailableKind) -> Option<bool> {
    let row = UNAVAILABLE_PERIOD_ROWS
        .iter()
        .find(|row| row.schedule_name == schedule_name)?;
    Some(match kind {
        UnavailableKind::Vacancy => row.vacancy,
        UnavailableKind::PowerOutage => row.power_outage,
        UnavailableKind::NoSpaceHeating => row.no_space_heating,
        UnavailableKind::NoSpaceCooling => row.no_space_cooling,
    })
}

/// The four unavailable-period flag columns a schedule file may carry
/// (`Vacancy`, `Power Outage`, `No Space Heating`, `No Space Cooling`,
/// OpenStudio-HPXML's own export spellings) must match the HPXML periods:
/// on every row of the file's own resolution (before resampling) the
/// column's value must be 1 exactly when a period of the column's kind
/// flags the row, and 0 exactly otherwise. A difference, including a
/// nonzero column with no HPXML period, fails construction naming the
/// column, the first differing row and the periods: HARES reads the
/// column and ignores nothing. Each checked column's verdict is traced
/// at debug.
pub fn check_schedule_flag_columns(
    schedule: &ScheduleTimeSeries,
    periods: &[UnavailablePeriod],
) -> Result<(), HaresError> {
    let flags = flagged_rows(schedule, periods);
    for column in UnavailableKind::all() {
        // The file's header spelling normalizes to the kind's name
        // ("Vacancy" and "vacancy" alike): the same lowercase-and-
        // underscore rule the column mappings match by.
        let wanted = column
            .column_name()
            .to_ascii_lowercase()
            .replace([' ', '-'], "_");
        let Some(key) = schedule
            .column_index
            .keys()
            .find(|name| name.to_ascii_lowercase().replace([' ', '-'], "_") == wanted)
            .cloned()
        else {
            continue;
        };
        let idx = schedule.column_index[&key];
        for (row, value) in schedule.columns[idx].iter().enumerate() {
            let expected = match column {
                UnavailableKind::Vacancy => flags[row].vacancy,
                UnavailableKind::PowerOutage => flags[row].power_outage,
                UnavailableKind::NoSpaceHeating => flags[row].no_space_heating,
                UnavailableKind::NoSpaceCooling => flags[row].no_space_cooling,
            };
            let flagged = match value {
                1.0 => true,
                0.0 => false,
                other => {
                    tracing::debug!(
                        column = %key,
                        kind = column.column_name(),
                        row,
                        value = *other,
                        verdict = "differs",
                        "schedule flag column checked against the \
                         unavailable periods"
                    );
                    return Err(HaresError::Dwelling(format!(
                        "schedule column '{}' row {row}: the value {other} is \
                         neither 0 nor 1; the column records the unavailable \
                         periods {} as OpenStudio-HPXML's export writes them",
                        column.column_name(),
                        describe_periods(periods),
                    )));
                }
            };
            if flagged != expected {
                tracing::debug!(
                    column = %key,
                    kind = column.column_name(),
                    row,
                    file_flag = flagged,
                    period_flag = expected,
                    verdict = "differs",
                    "schedule flag column checked against the \
                     unavailable periods"
                );
                return Err(HaresError::Dwelling(format!(
                    "schedule column '{}' row {row}: the file's flag is \
                     {flagged} where the HPXML periods give {expected}; the \
                     periods are {}",
                    column.column_name(),
                    describe_periods(periods),
                )));
            }
        }
        tracing::debug!(
            column = %key,
            kind = column.column_name(),
            rows = schedule.columns[idx].len(),
            verdict = "matches",
            "schedule flag column checked against the unavailable periods"
        );
    }
    Ok(())
}

/// The periods as an error names them.
fn describe_periods(periods: &[UnavailablePeriod]) -> String {
    if periods.is_empty() {
        return "none".to_string();
    }
    periods
        .iter()
        .map(|p| {
            format!(
                "{} {}/{} {}h to {}/{} {}h",
                p.kind.column_name(),
                p.begin_month,
                p.begin_day,
                p.begin_hour,
                p.end_month,
                p.end_day,
                p.end_hour,
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// Zero every schedule column whose `unavailable_periods.csv` row a period
/// applies to, on the period's flagged steps, at the schedule's own
/// resolution. The schedule columns that carry setpoints or temperatures
/// have no table row and are left alone.
///
/// Returns the columns as they were before the zeroing (`None` when no
/// period ran): OpenStudio-HPXML computes every peak and design level from
/// these pre-period schedules and zeroes the columns after, so the caller
/// hands the snapshot to `inject_schedule_into_specs_with_pre_period` and
/// a period removes volume instead of spreading it over the remaining days.
pub fn apply_unavailable_periods(
    schedule: &mut ScheduleTimeSeries,
    periods: &[UnavailablePeriod],
) -> Result<Option<Vec<Vec<f64>>>, HaresError> {
    if periods.is_empty() {
        return Ok(None);
    }
    let pre_period_columns = schedule.columns.clone();
    let flags = flagged_rows(schedule, periods);
    for period in periods {
        let count = flags
            .iter()
            .filter(|f| kind_flagged(f, period.kind))
            .count();
        let first = flags.iter().position(|f| kind_flagged(f, period.kind));
        let last = flags.iter().rposition(|f| kind_flagged(f, period.kind));
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
        // The column zeroes on the steps a period of a kind that applies
        // to its table row flags.
        let kinds: Vec<UnavailableKind> = [
            (row.vacancy, UnavailableKind::Vacancy),
            (row.power_outage, UnavailableKind::PowerOutage),
            (row.no_space_heating, UnavailableKind::NoSpaceHeating),
            (row.no_space_cooling, UnavailableKind::NoSpaceCooling),
        ]
        .iter()
        .filter(|(applies, _)| *applies)
        .map(|(_, kind)| *kind)
        .collect();
        if kinds.is_empty() {
            continue;
        }
        let mut touched = false;
        for (i, flagged) in flags.iter().enumerate() {
            if kinds.iter().any(|kind| kind_flagged(flagged, *kind)) {
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
    Ok(Some(pre_period_columns))
}

/// Whether the row flags carry `kind`.
fn kind_flagged(flag: &UnavailablePeriodState, kind: UnavailableKind) -> bool {
    match kind {
        UnavailableKind::Vacancy => flag.vacancy,
        UnavailableKind::PowerOutage => flag.power_outage,
        UnavailableKind::NoSpaceHeating => flag.no_space_heating,
        UnavailableKind::NoSpaceCooling => flag.no_space_cooling,
    }
}

/// Parse `SoftwareInfo/extension/UnavailablePeriods` with OpenStudio-HPXML's
/// defaults (`BeginHourOfDay` 0, `EndHourOfDay` 24, `NaturalVentilation`
/// `regular schedule`) and checks: an unknown `ColumnName`, an hour outside
/// its range, a missing required field, or a date that does not exist fails
/// the parse naming the period.
pub(crate) fn parse_unavailable_periods(
    root: &XmlNode,
) -> Result<Vec<UnavailablePeriod>, HpxmlError> {
    let Some(periods_node) = root.path(&[&SOFTWARE_INFO, &EXTENSION, &UNAVAILABLE_PERIODS]) else {
        return Ok(Vec::new());
    };
    let mut periods = Vec::new();
    for node in periods_node.children_named(&UNAVAILABLE_PERIOD) {
        let id = element_id(node).unwrap_or_else(|| "unknown".to_string());
        let require_f64 =
            |element: &'static crate::hpxml::elements::HpxmlElement| -> Result<f64, HpxmlError> {
                child_f64(node, element).ok_or_else(|| HpxmlError::MissingField {
                    path: "SoftwareInfo/extension/UnavailablePeriods/UnavailablePeriod",
                    system_kind: "Unavailable Period",
                    system_id: id.clone(),
                    reason: "the period's begin and end dates are required",
                })
            };
        let column = child_text(node, &COLUMN_NAME).ok_or_else(|| HpxmlError::MissingField {
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
        let begin_month = require_f64(&BEGIN_MONTH)? as u32;
        let begin_day = require_f64(&BEGIN_DAY_OF_MONTH)? as u32;
        let end_month = require_f64(&END_MONTH)? as u32;
        let end_day = require_f64(&END_DAY_OF_MONTH)? as u32;
        let require_hour = |element: &'static crate::hpxml::elements::HpxmlElement,
                            path: &'static str,
                            default: f64,
                            lower: f64,
                            upper: f64,
                            range_reason: &'static str|
         -> Result<u32, HpxmlError> {
            match child_f64(node, element) {
                None => Ok(default as u32),
                // The schematron bounds the hour and the field is a whole
                // hour of day: a negative value saturating to 0 or a
                // fractional one truncating in the cast would silently
                // shift the period, so both are validation errors.
                Some(raw)
                    if !raw.is_finite() || raw.fract() != 0.0 || raw < lower || raw > upper =>
                {
                    Err(HpxmlError::InvalidField {
                        path,
                        system_kind: "Unavailable Period",
                        system_id: id.clone(),
                        value_received: raw.to_string(),
                        reason: range_reason,
                    })
                }
                Some(raw) => Ok(raw as u32),
            }
        };
        let begin_hour = require_hour(
            &BEGIN_HOUR_OF_DAY,
            "UnavailablePeriod/BeginHourOfDay",
            0.0,
            0.0,
            23.0,
            "the begin hour must be a whole number from 0 to 23",
        )?;
        let end_hour = require_hour(
            &END_HOUR_OF_DAY,
            "UnavailablePeriod/EndHourOfDay",
            24.0,
            1.0,
            24.0,
            "the end hour must be a whole number from 1 to 24",
        )?;

        if !(1..=12).contains(&begin_month) || !(1..=12).contains(&end_month) {
            return Err(HpxmlError::InvalidField {
                path: "UnavailablePeriod/{BeginMonth,EndMonth}",
                system_kind: "Unavailable Period",
                system_id: id.clone(),
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
                    system_id: id.clone(),
                    value_received: format!("{month}/{day} ({name})"),
                    reason: "the date does not exist on a non-leap calendar; the day \
                             must be one the month has",
                });
            }
        }
        let natural_ventilation = match child_text(node, &NATURAL_VENTILATION) {
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
