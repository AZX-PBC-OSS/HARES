//! The design level a schedule column resolves to ignores the steps an
//! unavailable period flags, and the equipment classes' declared
//! unavailable-period rows agree with the reference table.

use hares_io::defaults::DefaultsStore;
use hares_io::hpxml::equipment::EquipmentSpec;
use hares_io::{UnavailableKind, UnavailablePeriod, parse_schedule_csv};
use hares_types::UnavailablePeriodState;

fn synthetic_annual_schedule(columns: &[(&str, f64)]) -> hares_io::ScheduleTimeSeries {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut csv = columns
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join(",");
    csv.push('\n');
    // A full year of hourly rows: index-based schedules are validated as
    // annual data.
    for _ in 0..8760 {
        csv.push_str(
            &columns
                .iter()
                .map(|(_, value)| format!("{value}"))
                .collect::<Vec<_>>()
                .join(","),
        );
        csv.push('\n');
    }
    let path = dir.path().join("schedule.csv");
    std::fs::write(&path, &csv).expect("write schedule");
    parse_schedule_csv(&path, &[], None, None).expect("schedule parses")
}

fn lighting_spec() -> EquipmentSpec {
    let mut parameters = serde_json::Map::new();
    parameters.insert(
        "annual_electric_j".to_string(),
        serde_json::Value::from(1.44e9),
    );
    EquipmentSpec {
        name: "Indoor Lighting".to_string(),
        instance_name: None,
        fuel_type: hares_types::FuelType::Electric,
        parameters,
        typed_overrides: serde_json::Map::new(),
        zip_params: None,
        typed_config: None,
        system_id: None,
        related_hvac_idref: None,
        primary_role: None,
        heating_setpoint_source: None,
        cooling_setpoint_source: None,
    }
}

/// A column of 1.0 with half its steps zeroed under a `Vacancy` period
/// resolves to the design level of the unzeroed column: the design
/// level's mean comes from the pre-period column, so the period removes
/// energy instead of scaling the remaining hours up.
#[test]
fn design_level_ignores_flagged_steps() {
    let vacancy = UnavailablePeriod {
        kind: UnavailableKind::Vacancy,
        begin_month: 1,
        begin_day: 1,
        begin_hour: 0,
        end_month: 7,
        end_day: 1,
        end_hour: 24,
        natural_ventilation: hares_io::NaturalVentilation::RegularSchedule,
    };
    // The interior lighting column: its table row's Vacancy flag is 1,
    // so the period zeroes it (the refrigerator's row is not: a vacant
    // home keeps the refrigerator cold).
    let mut schedule = synthetic_annual_schedule(&[("lighting_interior", 1.0)]);
    let pre_period = hares_io::apply_unavailable_periods(&mut schedule, &[vacancy])
        .expect("periods apply")
        .expect("the period ran");

    let mut specs = vec![lighting_spec()];
    hares_io::inject_schedule_into_specs_with_pre_period(
        &mut specs,
        &mut schedule,
        Some(&pre_period),
        1,
        None,
        &DefaultsStore::empty(),
        &mut Vec::new(),
    )
    .expect("injection succeeds");

    // The design level: the annual energy over the year's hours divided
    // by the unzeroed column's mean (1.0), so the derived power column's
    // values are the fraction times 400 kWh / 8760 h in watts
    // (45.6621 W). The zeroed half must not have scaled the level up (a
    // zeroed mean of 0.5 would double it).
    let design_w = 1.44e9 / 8760.0 / 3600.0;
    let col_idx = schedule
        .column_names
        .iter()
        .position(|name| name.starts_with("power_schedule_w_"))
        .expect("the derived power column exists");
    let max_w = schedule.columns[col_idx]
        .iter()
        .copied()
        .fold(f64::MIN, f64::max);
    assert!(
        (max_w - design_w).abs() < 1e-7,
        "the design level must come from the unzeroed column's mean: got \
         {max_w} W, wanted {design_w} W"
    );
    // The zeroed half's shape is zero: the period removes energy.
    // The period's days: Jan 1 through Jul 1, both days inclusive as
    // days, the half-open hours covering Jul 1 entirely: 182 days.
    let zeroed_cells = schedule.columns[col_idx]
        .iter()
        .filter(|v| **v == 0.0)
        .count();
    assert_eq!(
        zeroed_cells, 4368,
        "the flagged steps' power is zero: got {zeroed_cells}"
    );
}

/// Every canonical equipment class's declared unavailable-period rows
/// agree with the reference table: the declared `zeroed_by` kinds are
/// exactly the table row's 1s, and every declared row is a row of the
/// table.
#[test]
fn equipment_rows_match_the_unavailable_periods_table() {
    for class in hares_equipment::CANONICAL_EQUIPMENT_NAMES {
        let declared = hares_equipment::unavailable_period_rows(class);
        for row in declared {
            for kind in UnavailableKind::all() {
                let table_flag =
                    hares_io::row_zeroed_by(row.schedule_name, kind).unwrap_or_else(|| {
                        panic!(
                            "class '{class}' declares row '{}' which is no row \
                             of the reference table",
                            row.schedule_name
                        )
                    });
                let declared_flag = row.zeroed_by.contains(&kind);
                assert_eq!(
                    table_flag,
                    declared_flag,
                    "class '{class}' row '{}': the table's flag for {} is \
                     {table_flag}, the declaration says {declared_flag}",
                    row.schedule_name,
                    kind.column_name(),
                );
            }
        }
    }
}

/// Every canonical equipment class appears in the declaration's explicit
/// arms: a class whose declaration is empty is in the declared-none list
/// (PV, generators and the loads whose columns are no rows of the table),
/// so a new class must choose.
#[test]
fn every_class_declares_its_unavailable_rows() {
    let declared_none = [
        "PV",
        "Gas Generator",
        "Gas Fuel Cell",
        "Protocol Bridge",
        "EventBasedLoad",
        "Microwave",
        "Basement Lighting",
    ];
    for class in hares_equipment::CANONICAL_EQUIPMENT_NAMES {
        let rows = hares_equipment::unavailable_period_rows(class);
        if rows.is_empty() {
            assert!(
                declared_none.contains(class),
                "class '{class}' declares no unavailable-period row and is \
                 not in the declared-none list: the exhaustive match must \
                 choose"
            );
        }
    }
}

/// The check columns: a schedule file's `No Space Heating` column that
/// disagrees with the HPXML periods fails construction naming the column
/// and the first differing row. The column lookup normalizes the header
/// spelling, so the reference's own `No Space Heating` spelling matches.
#[test]
fn schedule_flag_column_must_match_the_periods() {
    let period = UnavailablePeriod {
        kind: UnavailableKind::NoSpaceHeating,
        begin_month: 1,
        begin_day: 1,
        begin_hour: 0,
        end_month: 7,
        end_day: 1,
        end_hour: 24,
        natural_ventilation: hares_io::NaturalVentilation::RegularSchedule,
    };
    let mut schedule = synthetic_annual_schedule(&[("occupants", 1.0), ("No Space Heating", 0.0)]);
    // The file's column stays 0 where the period flags: a difference.
    hares_io::check_schedule_flag_columns(&schedule, &[period])
        .expect_err("the zero column must disagree with the flagged period");
    // With the column matching the periods, the check passes.
    let idx = schedule
        .column_names
        .iter()
        .position(|name| name == "No Space Heating")
        .expect("the column");
    for (row, flag) in hares_io::unavailable_period_flags(&schedule, &[period])
        .iter()
        .enumerate()
    {
        schedule.columns[idx][row] = if flag.no_space_heating { 1.0 } else { 0.0 };
    }
    hares_io::check_schedule_flag_columns(&schedule, &[period])
        .expect("the matching column passes");
}

/// The natural-ventilation option rides every period type: the flags
/// carry the availability the last flagged period with a non-regular
/// option set, whatever the kind.
#[test]
fn natural_ventilation_option_applies_to_every_period() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut csv = String::from("occupants\n");
    for _ in 0..8760 {
        csv.push_str("1.0\n");
    }
    let path = dir.path().join("schedule.csv");
    std::fs::write(&path, &csv).expect("write schedule");
    let schedule = parse_schedule_csv(&path, &[], None, None).expect("schedule parses");

    let make = |kind, option| UnavailablePeriod {
        kind,
        begin_month: 6,
        begin_day: 1,
        begin_hour: 0,
        end_month: 6,
        end_day: 2,
        end_hour: 24,
        natural_ventilation: option,
    };
    // A `No Space Cooling` period with `always unavailable` sets the
    // availability on its flagged rows, as does a `Power Outage` with
    // `always available`.
    let flags = hares_io::unavailable_period_flags(
        &schedule,
        &[
            make(
                UnavailableKind::NoSpaceCooling,
                hares_io::NaturalVentilation::AlwaysUnavailable,
            ),
            make(
                UnavailableKind::PowerOutage,
                hares_io::NaturalVentilation::AlwaysAvailable,
            ),
        ],
    );
    let flagged: Vec<&UnavailablePeriodState> = flags
        .iter()
        .filter(|f| f.no_space_cooling || f.power_outage)
        .collect();
    assert!(!flagged.is_empty(), "the periods flag their steps");
    // The last flagged period with a non-regular option wins: on the
    // shared window the `Power Outage`'s `always available` is the last.
    assert_eq!(flagged[0].natural_ventilation, Some(true));

    let flags = hares_io::unavailable_period_flags(
        &schedule,
        &[make(
            UnavailableKind::Vacancy,
            hares_io::NaturalVentilation::AlwaysUnavailable,
        )],
    );
    assert!(
        flags.iter().any(|f| f.natural_ventilation == Some(false)),
        "a vacancy period's always-unavailable sets the availability"
    );

    let flags = hares_io::unavailable_period_flags(
        &schedule,
        &[make(
            UnavailableKind::NoSpaceHeating,
            hares_io::NaturalVentilation::RegularSchedule,
        )],
    );
    assert!(
        flags.iter().all(|f| f.natural_ventilation.is_none()),
        "regular schedule leaves the availability unset"
    );
}

/// The zeroing targets the columns the period's own kind applies to: a
/// `No Space Heating` period zeroes the space-heating rows and leaves
/// the occupants column alone (the reference's per-period flag columns,
/// not a shared flag).
#[test]
fn a_period_zeroes_only_its_own_rows() {
    let period = UnavailablePeriod {
        kind: UnavailableKind::NoSpaceHeating,
        begin_month: 1,
        begin_day: 1,
        begin_hour: 0,
        end_month: 7,
        end_day: 1,
        end_hour: 24,
        natural_ventilation: hares_io::NaturalVentilation::RegularSchedule,
    };
    let mut schedule = synthetic_annual_schedule(&[("occupants", 1.0), ("space_heating", 1.0)]);
    hares_io::apply_unavailable_periods(&mut schedule, &[period]).expect("periods apply");
    let occupants = schedule.column_index["occupants"];
    let heating = schedule.column_index["space_heating"];
    assert!(
        schedule.columns[occupants].iter().all(|v| *v == 1.0),
        "a No Space Heating period must not zero the occupants column"
    );
    assert!(
        schedule.columns[heating]
            .iter()
            .take(4344)
            .all(|v| *v == 0.0),
        "the period zeroes the space_heating column on its flagged steps"
    );
}
