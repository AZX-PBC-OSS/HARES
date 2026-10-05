//! A wired boiler and its indirect tank share one fluid loop, including
//! when the boiler's capacity is autosized.
//!
//! The wiring pass runs at resolve time, before autosizing; an autosized
//! boiler's typed config is rebuilt from its spec's raw parameters after
//! sizing, so the wired loop id must travel in those parameters to survive
//! the rebuild. Both tests read the pair's loop from the built dwelling's
//! fluid port declarations.

use std::path::PathBuf;

use chrono::{Duration, FixedOffset, TimeZone};
use hares_core::{Dwelling, DwellingConfig, SimulationConfig};
use hares_equipment::DHW_DEMAND_LOOP;
use hares_io::OutputFormat;
use hares_types::{LoopId, PortType};

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn sim_config(duration: Duration) -> SimulationConfig {
    SimulationConfig {
        start_time: FixedOffset::west_opt(10 * 3600)
            .expect("UTC-10 offset is valid")
            .with_ymd_and_hms(2018, 1, 1, 0, 0, 0)
            .unwrap(),
        duration,
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
    }
}

fn dwelling_config(hpxml_path: PathBuf, sim: SimulationConfig) -> DwellingConfig {
    DwellingConfig {
        hpxml_path,
        schedule_path: project_root().join("data/examples/BEopt_example_schedule.csv"),
        weather_path: project_root().join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        defaults_path: Some(project_root().join("defaults")),
        sim_config: sim,
        overrides: None,
        bldg_id: 100,
        initialization_duration: None,
        resample_overrides: Some(hares_io::ResampleOverrides::ochre_compat()),
        patches: None,
    }
}

fn base_dhw_multiple_path() -> PathBuf {
    project_root().join("tests/fixtures/hpxml/ochre_samples/base-dhw-multiple.xml")
}

/// The OCHRE sample with the boiler's `HeatingCapacity` removed, so the
/// resolver flags the boiler for autosizing and its typed config is rebuilt
/// after sizing.
fn autosized_boiler_hpxml_path() -> PathBuf {
    let xml =
        std::fs::read_to_string(base_dhw_multiple_path()).expect("base-dhw-multiple.xml reads");
    let capacity = "<HeatingCapacity>36000.0</HeatingCapacity>";
    let edited = xml.replacen(capacity, "", 1);
    assert_ne!(
        edited, xml,
        "the boiler's HeatingCapacity must be present in the fixture so the \
         test can remove it"
    );
    let path = std::env::temp_dir().join("hares-autosized-boiler-indirect-tank.xml");
    std::fs::write(&path, edited).expect("the edited fixture writes");
    path
}

/// The OCHRE sample with the indirect tank's RelatedHVACSystem removed, so
/// the tank is unwired: the loop allocator assigns its loop id and the
/// WH-autosize rebuild must keep it.
fn unwired_tank_hpxml_path() -> PathBuf {
    let xml =
        std::fs::read_to_string(base_dhw_multiple_path()).expect("base-dhw-multiple.xml reads");
    let wired = "<RelatedHVACSystem idref='HeatingSystem1'/>";
    let edited = xml.replacen(wired, "", 1);
    assert_ne!(
        edited, xml,
        "the fixture's tank carries RelatedHVACSystem so the test can remove it"
    );
    let path = std::env::temp_dir().join("hares-unwired-indirect-tank.xml");
    std::fs::write(&path, edited).expect("the edited fixture writes");
    path
}

/// The declared fluid loop of the named equipment: its single fluid port,
/// excluding the shared DHW demand loop (which storage tanks declare in
/// addition to their supply loop).
fn supply_loop(dwelling: &Dwelling, equipment_type: &str) -> Option<LoopId> {
    dwelling
        .equipment()
        .iter()
        .filter(|eq| eq.descriptor().equipment_type.as_ref() == equipment_type)
        .flat_map(|eq| eq.ports().iter().copied())
        .filter(|p| p.port_type == PortType::Fluid)
        .filter(|p| p.loop_id != Some(DHW_DEMAND_LOOP))
        .map(|p| p.loop_id.expect("a fluid port declares a loop id"))
        .next()
}

fn assert_pair_shares_one_wired_loop(config: DwellingConfig) {
    let dwelling = Dwelling::from_config(config).expect("dwelling builds from the fixture");
    let boiler_loop = supply_loop(&dwelling, "Gas Boiler")
        .expect("the dwelling has a gas boiler with a fluid port");
    let tank_loop = supply_loop(&dwelling, "Indirect Tank")
        .expect("the dwelling has an indirect tank with a fluid port");
    assert_eq!(
        boiler_loop, tank_loop,
        "the boiler's fluid loop must be the tank's supply loop"
    );
    assert_ne!(
        boiler_loop,
        LoopId(0),
        "the pair's loop must be the wired loop, not the unassigned \
         LoopId(0) fallback"
    );
}

#[test]
fn autosized_boiler_keeps_its_indirect_tank_loop() {
    assert_pair_shares_one_wired_loop(dwelling_config(
        autosized_boiler_hpxml_path(),
        sim_config(Duration::hours(1)),
    ));
}

#[test]
fn wired_boiler_and_indirect_tank_share_the_wired_loop() {
    assert_pair_shares_one_wired_loop(dwelling_config(
        base_dhw_multiple_path(),
        sim_config(Duration::hours(1)),
    ));
}

/// An unwired autosized indirect tank keeps the loop id the allocator
/// assigned: the allocator mirrors the id into the tank's raw parameters
/// and the WH-autosize rebuild reads it back, instead of rebuilding the
/// typed config from the stale parameters and leaving the tank on the
/// unassigned LoopId(0).
#[test]
fn unwired_autosized_tank_keeps_its_allocated_loop() {
    let dwelling = Dwelling::from_config(dwelling_config(
        unwired_tank_hpxml_path(),
        sim_config(Duration::hours(1)),
    ))
    .expect("dwelling builds from the unwired-tank fixture");
    let tank_loop = supply_loop(&dwelling, "Indirect Tank")
        .expect("the dwelling has an indirect tank with a fluid port");
    assert_ne!(
        tank_loop,
        LoopId(0),
        "the tank's supply loop is the allocator's assigned id, not the \
         unassigned LoopId(0) sentinel"
    );
}

/// The sample's HPWH omits HeatingCapacity, so its typed config is rebuilt
/// after WH autosizing: the rebuild keeps the loop id the allocator
/// assigned, and the HPWH's supply port carries it.
#[test]
fn autosized_hpwh_keeps_its_allocated_supply_loop() {
    let dwelling = Dwelling::from_config(dwelling_config(
        base_dhw_multiple_path(),
        sim_config(Duration::hours(1)),
    ))
    .expect("dwelling builds from the sample");
    let hpwh_loop = supply_loop(&dwelling, "Heat Pump Water Heater")
        .expect("the dwelling has an HPWH with a fluid port");
    assert_ne!(
        hpwh_loop,
        LoopId(0),
        "the HPWH's supply loop is its allocated id, not the unassigned \
         LoopId(0) sentinel"
    );
}
