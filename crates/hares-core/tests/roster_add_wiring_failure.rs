//! A roster add whose fluid-loop wiring cannot be planned is rejected with
//! the typed error and leaves the dwelling exactly as it was: the same
//! roster, the same equipment ids, the same next id and the same
//! checkpoint.

use std::path::PathBuf;

use chrono::{Duration, FixedOffset, TimeZone};
use hares_core::{Dwelling, DwellingConfig, SimulationConfig};
use hares_equipment::{ElectricBoilerConfig, Equipment, EquipmentConfig, EquipmentRegistry};
use hares_io::OutputFormat;
use hares_types::{FluidType, HaresError};

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn config() -> DwellingConfig {
    DwellingConfig {
        hpxml_path: project_root().join("tests/fixtures/hpxml/ochre_samples/base.xml"),
        schedule_path: Some(project_root().join("data/examples/BEopt_example_schedule.csv")),
        weather_path: project_root().join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        defaults_path: Some(project_root().join("defaults")),
        sim_config: SimulationConfig {
            start_time: FixedOffset::west_opt(7 * 3600)
                .expect("UTC-7 offset is valid")
                .with_ymd_and_hms(2023, 1, 1, 0, 0, 0)
                .unwrap(),
            duration: Duration::hours(1),
            time_res: Duration::seconds(900),
            output_verbosity: 0,
            write_output: false,
            output_path: None,
            output_format: OutputFormat::Csv,
            output_chunk_size: 1024,
            setpoint_deadband_c: None,
            master_seed: 0,
            civil_timezone: None,
            site_location: hares_io::SiteLocationOverride::default(),
            retain_batches: false,
            rotation: hares_io::RotationPolicy::None,
        },
        overrides: None,
        bldg_id: 100,
        initialization_duration: None,
        resample_overrides: Some(hares_io::ResampleOverrides::ochre_compat()),
        patches: None,
    }
}

fn boiler_on_loop(
    name: &str,
    loop_id: u16,
    fluid_type: FluidType,
    dwelling: &Dwelling,
) -> Box<dyn Equipment> {
    let config = EquipmentConfig::from_typed(
        name.to_string(),
        "Electric Boiler".to_string(),
        ElectricBoilerConfig {
            zone_id: Some(1),
            loop_id: Some(loop_id),
            capacity_w: 0.0,
            fluid_type,
            ..ElectricBoilerConfig::default()
        },
    )
    .expect("boiler config");
    let mut boiler = EquipmentRegistry::new()
        .create("Electric Boiler", config.clone())
        .expect("create boiler");
    boiler
        .init(&config, dwelling.latest_env())
        .expect("init boiler");
    boiler
}

fn roster(dwelling: &Dwelling) -> Vec<(String, u32)> {
    dwelling
        .equipment()
        .iter()
        .map(|e| {
            let d = e.descriptor();
            (d.name.clone(), d.id.0)
        })
        .collect()
}

fn dwelling_with_water_loop_7() -> Dwelling {
    let mut dwelling = Dwelling::from_config(config()).expect("build the dwelling");
    let water = boiler_on_loop("Boiler-Water", 7, FluidType::Water, &dwelling);
    dwelling
        .add_equipment(water)
        .expect("the first declarer of loop 7 is accepted");
    dwelling
}

#[test]
fn an_unwirable_roster_add_is_rejected_and_leaves_the_dwelling_untouched() {
    let mut dwelling = dwelling_with_water_loop_7();
    let roster_before = roster(&dwelling);
    let checkpoint_before = dwelling.save_checkpoint().expect("checkpoint");

    let glycol = boiler_on_loop("Boiler-Glycol", 7, FluidType::Glycol, &dwelling);
    let err = dwelling
        .add_equipment(glycol)
        .expect_err("a conflicting fluid on loop 7 cannot be wired");

    match &err {
        HaresError::RejectedEquipment { reason, .. } => assert!(
            matches!(**reason, HaresError::Envelope(_)),
            "the reason is the wiring error, got: {reason:?}"
        ),
        other => panic!("expected RejectedEquipment, got: {other:?}"),
    }
    assert_eq!(roster(&dwelling), roster_before);
    assert_eq!(
        dwelling.save_checkpoint().expect("checkpoint"),
        checkpoint_before
    );

    // The rejected add consumed no equipment id: the next accepted add gets
    // the id it would have had if the rejected add had never happened.
    let next = boiler_on_loop("Boiler-Next", 8, FluidType::Water, &dwelling);
    dwelling
        .add_equipment(next)
        .expect("a wirable add succeeds");

    let mut reference = dwelling_with_water_loop_7();
    let next = boiler_on_loop("Boiler-Next", 8, FluidType::Water, &reference);
    reference
        .add_equipment(next)
        .expect("a wirable add succeeds");
    assert_eq!(roster(&dwelling), roster(&reference));
}
