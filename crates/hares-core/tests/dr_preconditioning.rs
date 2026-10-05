//! DR pre-conditioning events through ResStock homes. The event moves the
//! one setpoint its target serves: an air conditioner pre-cools on a
//! shoulder-season evening (the outdoor air below the zone, the home still
//! in cooling), a furnace pre-heats in January, and no signal is rejected.

use std::path::PathBuf;

use chrono::{Duration, FixedOffset, TimeZone};
use hares_control::DispatchTarget;
use hares_core::actors::{AlwaysComply, DrAction, DrCompliance};
use hares_core::{Dwelling, DwellingConfig, SimulationConfig};
use hares_types::telemetry_keys as tk;
use hares_types::{DRLevel, EndUse, HaresError, OperatingMode, ThermostatAxes};

const PRECONDITION_DELTA_C: f64 = 2.0;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Replaces the text of every `<tag>` element with `value`.
fn set_element_text(xml: &str, tag: &str, value: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let body = start + open.len();
        let end = body + rest[body..].find(&close).expect("closed element");
        out.push_str(&rest[..body]);
        out.push_str(value);
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

struct Home {
    bldg: &'static str,
    weather: &'static str,
    utc_offset_h: i32,
    start: (i32, u32, u32, u32),
    /// A constant (heating, cooling) schedule in °F, or the home's own.
    setpoints_f: Option<(f64, f64)>,
}

fn build(home: &Home) -> Dwelling {
    let fixture = project_root()
        .join("tests/fixtures/resstock/2025.1")
        .join(home.bldg);
    let mut xml = std::fs::read_to_string(fixture.join("home.xml")).expect("read home.xml");
    if let Some((heating_f, cooling_f)) = home.setpoints_f {
        let hourly = |f: f64| vec![format!("{f:.1}"); 24].join(", ");
        for season in ["Weekday", "Weekend"] {
            xml = set_element_text(
                &xml,
                &format!("{season}SetpointTempsHeatingSeason"),
                &hourly(heating_f),
            );
            xml = set_element_text(
                &xml,
                &format!("{season}SetpointTempsCoolingSeason"),
                &hourly(cooling_f),
            );
        }
    }
    let hpxml_path = std::env::temp_dir().join(format!(
        "hares-dr-preconditioning-{}-{}.xml",
        home.bldg,
        std::process::id()
    ));
    std::fs::write(&hpxml_path, xml).expect("write home.xml");
    let (year, month, day, hour) = home.start;
    let config = DwellingConfig {
        hpxml_path: hpxml_path.clone(),
        schedule_path: Some(fixture.join("in.schedules.csv")),
        weather_path: project_root()
            .join("tests/fixtures/resstock/2025.1/weather")
            .join(home.weather),
        defaults_path: Some(project_root().join("defaults")),
        sim_config: SimulationConfig {
            start_time: FixedOffset::east_opt(home.utc_offset_h * 3600)
                .expect("offset")
                .with_ymd_and_hms(year, month, day, hour, 0, 0)
                .unwrap(),
            duration: Duration::hours(3),
            time_res: Duration::seconds(900),
            output_verbosity: 0,
            write_output: false,
            output_path: None,
            output_format: hares_io::OutputFormat::Csv,
            output_chunk_size: 1024,
            setpoint_deadband_c: None,
            master_seed: 0,
            civil_timezone: None,
            site_location: hares_io::SiteLocationOverride::default(),
            retain_batches: false,
            rotation: hares_io::RotationPolicy::None,
            max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
        },
        overrides: None,
        bldg_id: 3,
        initialization_duration: None,
        resample_overrides: None,
        patches: None,
    };
    let dwelling = Dwelling::from_config(config);
    std::fs::remove_file(&hpxml_path).expect("remove home.xml");
    dwelling.expect("build the home")
}

/// Runs a pre-conditioning event on the home's `end_use` unit for `steps`
/// steps; returns per step the unit's (setpoint, schedule setpoint, mode)
/// for the event's axis. The actor decides after a step, so the event
/// reaches the unit from the second step on; the first step is dropped.
fn run_event(home: &Home, end_use: EndUse, steps: usize) -> (Dwelling, Vec<(f64, f64, f64)>) {
    let mut dwelling = build(home);
    let unit = dwelling
        .equipment()
        .iter()
        .map(|eq| eq.descriptor())
        .find(|d| d.end_use == end_use)
        .expect("the home has the unit")
        .name
        .clone();
    let mut actor = DrCompliance::new("DR")
        .with_compliance_model(AlwaysComply)
        .with_hvac_target(DispatchTarget::ByName(unit.as_str().into()))
        .with_hvac_action(DrAction::setpoint_delta(PRECONDITION_DELTA_C));
    actor.set_dr_level(DRLevel::Moderate);
    dwelling
        .add_actor(Box::new(actor))
        .expect("register the DR actor");
    let (setpoint_key, schedule_key) = if end_use == EndUse::HVAC_COOLING {
        (tk::COOLING_SETPOINT_C, tk::SCHEDULE_COOLING_SETPOINT_C)
    } else {
        (tk::HEATING_SETPOINT_C, tk::SCHEDULE_HEATING_SETPOINT_C)
    };
    let mut trace = Vec::with_capacity(steps);
    dwelling.step().expect("step");
    for _ in 0..steps {
        dwelling.step().expect("step");
        let eq = dwelling
            .equipment()
            .iter()
            .find(|eq| eq.descriptor().name == unit)
            .expect("the unit");
        let telemetry = eq.telemetry();
        trace.push((
            telemetry.get(setpoint_key).expect("setpoint telemetry"),
            telemetry.get(schedule_key).expect("schedule telemetry"),
            telemetry.get(tk::OPERATING_MODE).expect("mode telemetry"),
        ));
    }
    (dwelling, trace)
}

/// California, 2018-05-08 19:00 to 20:00: the outdoor air (21.5 then
/// 19 °C) is below the 26 °C zone, the AC is still cooling, and the event
/// lowers its cooling setpoint; it keeps cooling.
#[test]
fn a_preconditioning_event_on_an_air_conditioner_precools() {
    let home = Home {
        bldg: "bldg0176227",
        weather: "G0600770_2018.csv",
        utc_offset_h: -8,
        start: (2018, 5, 8, 19),
        setpoints_f: None,
    };
    let (dwelling, trace) = run_event(&home, EndUse::HVAC_COOLING, 4);
    assert_eq!(dwelling.health().rejected_control_signals, 0);
    for &(setpoint_c, schedule_c, _) in &trace {
        assert!(
            (setpoint_c - (schedule_c - PRECONDITION_DELTA_C)).abs() < 1e-6,
            "the AC must pre-cool to {} C, runs at {setpoint_c} C: {trace:?}",
            schedule_c - PRECONDITION_DELTA_C
        );
    }
    assert!(
        trace
            .iter()
            .any(|&(_, _, mode)| mode == OperatingMode::Cooling.as_code()),
        "the AC must run: {trace:?}"
    );
}

fn bestest_600() -> Dwelling {
    let path = project_root().join("tests/fixtures/bestest/600.toml");
    Dwelling::from_toml_config_with_write_output(&path, Some(false)).expect("build BESTEST 600")
}

fn ideal_unit(dwelling: &Dwelling) -> String {
    dwelling
        .equipment()
        .iter()
        .find(|eq| eq.thermostat_axes() == Some(ThermostatAxes::Both))
        .expect("BESTEST 600 runs an ideal unit")
        .descriptor()
        .name
        .clone()
}

/// BESTEST 600's ideal unit (heating and cooling) under a year-long
/// pre-cool event in Denver weather: its mode flips between heating and
/// cooling all year, and on every step only the cooling setpoint is
/// displaced, never the heating one, so the band is never squeezed.
#[test]
fn a_dual_mode_unit_displaces_only_the_named_axis_for_a_year() {
    let mut dwelling = bestest_600();
    let unit = ideal_unit(&dwelling);
    let mut actor = DrCompliance::new("DR")
        .with_compliance_model(AlwaysComply)
        .with_hvac_target(DispatchTarget::ByName(unit.as_str().into()))
        .with_hvac_action(DrAction::precool(PRECONDITION_DELTA_C));
    actor.set_dr_level(DRLevel::Moderate);
    dwelling
        .add_actor(Box::new(actor))
        .expect("register the DR actor");

    let (mut heating_steps, mut cooling_steps) = (0_u32, 0_u32);
    dwelling.step().expect("first step");
    for step in 1..8760 {
        dwelling.step().expect("step");
        let eq = dwelling
            .equipment()
            .iter()
            .find(|eq| eq.descriptor().name == unit)
            .expect("the unit");
        let t = eq.telemetry();
        let get = |key| t.get(key).expect("setpoint telemetry");
        let heating_moved = get(tk::HEATING_SETPOINT_C) != get(tk::SCHEDULE_HEATING_SETPOINT_C);
        let cooling_c = get(tk::COOLING_SETPOINT_C);
        let precooled_c = get(tk::SCHEDULE_COOLING_SETPOINT_C) - PRECONDITION_DELTA_C;
        assert!(
            !heating_moved,
            "step {step}: the heating setpoint left its schedule"
        );
        assert!(
            (cooling_c - precooled_c).abs() < 1e-9,
            "step {step}: cooling {cooling_c} C, pre-cooled {precooled_c} C"
        );
        match get(tk::OPERATING_MODE) {
            m if m == OperatingMode::Heating.as_code() => heating_steps += 1,
            m if m == OperatingMode::Cooling.as_code() => cooling_steps += 1,
            _ => {}
        }
    }
    assert!(
        heating_steps > 1000 && cooling_steps > 1000,
        "the unit must flip modes: {heating_steps} heating, {cooling_steps} cooling steps"
    );
    assert_eq!(dwelling.health().rejected_control_signals, 0);
}

/// An event that names no direction for a unit serving both setpoints is
/// refused when the actor is registered.
#[test]
fn an_event_without_a_direction_on_a_dual_mode_unit_is_refused() {
    let mut dwelling = bestest_600();
    let unit = ideal_unit(&dwelling);
    let actor = DrCompliance::new("DR")
        .with_hvac_target(DispatchTarget::ByName(unit.as_str().into()))
        .with_hvac_action(DrAction::setpoint_delta(PRECONDITION_DELTA_C));
    let err = dwelling
        .add_actor(Box::new(actor))
        .expect_err("no axis for the ideal unit");
    assert!(
        matches!(err, HaresError::PreconditioningAxis { .. }),
        "{err}"
    );
    assert_eq!(dwelling.actor_count(), 0);
}

/// Massachusetts gas-furnace home at a constant 70/76 °F in January: the
/// event raises the heating setpoint.
#[test]
fn a_preconditioning_event_on_a_furnace_preheats() {
    let home = Home {
        bldg: "bldg0000003",
        weather: "G2500170_2018.csv",
        utc_offset_h: -5,
        start: (2018, 1, 15, 0),
        setpoints_f: Some((70.0, 76.0)),
    };
    let (dwelling, trace) = run_event(&home, EndUse::HVAC_HEATING, 8);
    assert_eq!(dwelling.health().rejected_control_signals, 0);
    for &(setpoint_c, schedule_c, _) in &trace {
        assert!(
            (setpoint_c - (schedule_c + PRECONDITION_DELTA_C)).abs() < 1e-6,
            "the furnace must pre-heat to {} C, runs at {setpoint_c} C: {trace:?}",
            schedule_c + PRECONDITION_DELTA_C
        );
    }
    assert!(
        trace
            .iter()
            .any(|&(_, _, mode)| mode == OperatingMode::Heating.as_code()),
        "the furnace must run: {trace:?}"
    );
}
