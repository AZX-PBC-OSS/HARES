//! A DR pre-conditioning event through a ResStock home whose thermostat
//! schedule is the typical 70/76 F (21.1/24.4 C): in January weather the
//! event raises the heating setpoint, and no signal is rejected.

use std::path::PathBuf;

use chrono::{Duration, FixedOffset, TimeZone};
use hares_control::DispatchTarget;
use hares_core::actors::{AlwaysComply, DrAction, DrCompliance};
use hares_core::{Dwelling, DwellingConfig, SimulationConfig};
use hares_types::{DRLevel, EndUse};

const HEATING_SETPOINT_F: f64 = 70.0;
const COOLING_SETPOINT_F: f64 = 76.0;
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

/// The Massachusetts gas-furnace home with its thermostat schedule set to a
/// constant 70/76 F.
fn massachusetts_home() -> Dwelling {
    let fixture = project_root().join("tests/fixtures/resstock/2025.1/bldg0000003");
    let mut xml = std::fs::read_to_string(fixture.join("home.xml")).expect("read home.xml");
    let hourly = |f: f64| vec![format!("{f:.1}"); 24].join(", ");
    for season in ["Weekday", "Weekend"] {
        xml = set_element_text(
            &xml,
            &format!("{season}SetpointTempsHeatingSeason"),
            &hourly(HEATING_SETPOINT_F),
        );
        xml = set_element_text(
            &xml,
            &format!("{season}SetpointTempsCoolingSeason"),
            &hourly(COOLING_SETPOINT_F),
        );
    }
    let hpxml_path = std::env::temp_dir().join(format!(
        "hares-dr-preconditioning-{}.xml",
        std::process::id()
    ));
    std::fs::write(&hpxml_path, xml).expect("write home.xml");
    let config = DwellingConfig {
        hpxml_path: hpxml_path.clone(),
        schedule_path: Some(fixture.join("in.schedules.csv")),
        weather_path: project_root()
            .join("tests/fixtures/resstock/2025.1/weather/G2500170_2018.csv"),
        defaults_path: Some(project_root().join("defaults")),
        sim_config: SimulationConfig {
            start_time: FixedOffset::west_opt(5 * 3600)
                .expect("UTC-5")
                .with_ymd_and_hms(2018, 1, 15, 0, 0, 0)
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

#[test]
fn a_preconditioning_event_raises_the_heating_setpoint_and_nothing_is_rejected() {
    let mut dwelling = massachusetts_home();
    let heater = dwelling
        .equipment()
        .iter()
        .map(|eq| eq.descriptor())
        .find(|d| d.end_use == EndUse::HVAC_HEATING)
        .expect("the home has a heating unit")
        .clone();
    let mut actor = DrCompliance::new("DR")
        .with_compliance_model(AlwaysComply)
        .with_hvac_target(DispatchTarget::ByName(heater.name.as_str().into()))
        .with_hvac_action(DrAction::setpoint_delta(PRECONDITION_DELTA_C));
    actor.set_dr_level(DRLevel::Moderate);
    dwelling
        .add_actor(Box::new(actor))
        .expect("register the DR actor");

    for _ in 0..8 {
        dwelling.step().expect("step");
    }

    assert_eq!(dwelling.health().rejected_control_signals, 0);
    let heating_setpoint_c = (HEATING_SETPOINT_F - 32.0) * 5.0 / 9.0;
    let setpoint_c = dwelling.latest_env().equipment_core[&heater.id]
        .state
        .setpoint_c
        .expect("a heater publishes its setpoint");
    assert!(
        (setpoint_c - (heating_setpoint_c + PRECONDITION_DELTA_C)).abs() < 1e-6,
        "the event must pre-heat to {} C; the heater runs at {setpoint_c} C",
        heating_setpoint_c + PRECONDITION_DELTA_C
    );
}
