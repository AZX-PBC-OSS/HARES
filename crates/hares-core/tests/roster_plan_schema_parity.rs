//! Assembly and every later roster change build the output schema through
//! the same roster planner, so equipment present at construction and the
//! same equipment removed and added back before the first step produce the
//! same schema: the same columns, each with the same field metadata
//! (including telemetry-unit provenance), and the same actor columns.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use chrono::{Duration, TimeZone};
use hares_core::{Dwelling, DwellingConfig, SimulationConfig};
use hares_io::OutputFormat;
use hares_types::EndUse;

#[path = "../../../tests/support/denver_offset.rs"]
mod denver_offset;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn sim_config(output_path: PathBuf) -> SimulationConfig {
    SimulationConfig {
        start_time: denver_offset::denver_offset()
            .with_ymd_and_hms(2023, 1, 1, 0, 0, 0)
            .unwrap(),
        duration: Duration::hours(1),
        time_res: Duration::seconds(900),
        output_verbosity: 5,
        write_output: true,
        output_path: Some(output_path),
        output_format: OutputFormat::Csv,
        output_chunk_size: 1024,
        setpoint_deadband_c: None,
        master_seed: 0,
        civil_timezone: None,
        site_location: hares_io::SiteLocationOverride::default(),
        retain_batches: false,
        rotation: hares_io::RotationPolicy::None,
        max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
    }
}

/// An OCHRE sample HPXML fixture, paired with the standard BEopt example
/// schedule and Denver weather file every other ochre-sample test in this
/// crate uses.
fn ochre_dwelling(hpxml_name: &str, output_dir: &tempfile::TempDir) -> Dwelling {
    Dwelling::from_config(DwellingConfig {
        hpxml_path: project_root()
            .join("tests/fixtures/hpxml/ochre_samples")
            .join(hpxml_name),
        schedule_path: Some(project_root().join("data/examples/BEopt_example_schedule.csv")),
        weather_path: project_root().join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        defaults_path: Some(project_root().join("defaults")),
        sim_config: sim_config(output_dir.path().join("out.csv")),
        overrides: None,
        bldg_id: 100,
        initialization_duration: None,
        resample_overrides: Some(hares_io::ResampleOverrides::ochre_compat()),
        patches: None,
    })
    .unwrap_or_else(|err| panic!("assemble {hpxml_name}: {err}"))
}

/// Every schema column by name, with its field metadata in a stable order
/// (Arrow keeps field metadata in a `HashMap`).
fn schema_columns(dwelling: &Dwelling) -> BTreeMap<String, BTreeMap<String, String>> {
    dwelling
        .recorder
        .as_ref()
        .expect("write_output is true")
        .schema()
        .fields()
        .iter()
        .map(|f| {
            let metadata: &HashMap<String, String> = f.metadata();
            (
                f.name().clone(),
                metadata
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            )
        })
        .collect()
}

fn equipment_with_end_use(dwelling: &Dwelling, end_use: &EndUse) -> String {
    dwelling
        .equipment()
        .iter()
        .find(|e| &e.descriptor().end_use == end_use)
        .unwrap_or_else(|| panic!("the fixture assembles {end_use:?} equipment"))
        .descriptor()
        .name
        .clone()
}

fn assert_removed_and_re_added_equipment_restores_the_assembled_schema(
    hpxml_name: &str,
    end_use: EndUse,
) {
    let output_dir = tempfile::tempdir().expect("tempdir");
    let mut dwelling = ochre_dwelling(hpxml_name, &output_dir);
    let assembled = schema_columns(&dwelling);
    let name = equipment_with_end_use(&dwelling, &end_use);
    assert!(
        assembled.keys().any(|column| column.starts_with(&name)),
        "{hpxml_name}: '{name}' owns no schema column, so the comparison would be vacuous"
    );

    let equipment = dwelling
        .remove_equipment(&name)
        .unwrap_or_else(|err| panic!("{hpxml_name}: remove '{name}': {err}"));
    dwelling
        .add_equipment(equipment)
        .unwrap_or_else(|err| panic!("{hpxml_name}: re-add '{name}': {err}"));

    let re_added = schema_columns(&dwelling);
    let missing: Vec<&String> = assembled
        .keys()
        .filter(|c| !re_added.contains_key(*c))
        .collect();
    let extra: Vec<&String> = re_added
        .keys()
        .filter(|c| !assembled.contains_key(*c))
        .collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "{hpxml_name}: re-adding '{name}' must restore exactly the assembled columns; \
         missing {missing:?}, extra {extra:?}"
    );
    for (column, metadata) in &assembled {
        assert_eq!(
            &re_added[column], metadata,
            "{hpxml_name}: column '{column}' must carry the same metadata after '{name}' \
             is removed and added back"
        );
    }
}

#[test]
fn re_added_battery_restores_the_assembled_schema() {
    assert_removed_and_re_added_equipment_restores_the_assembled_schema(
        "base-battery.xml",
        EndUse::BATTERY,
    );
}

#[test]
fn re_added_pv_restores_the_assembled_schema() {
    assert_removed_and_re_added_equipment_restores_the_assembled_schema("base-pv.xml", EndUse::PV);
}

#[test]
fn re_added_water_heater_restores_the_assembled_schema() {
    assert_removed_and_re_added_equipment_restores_the_assembled_schema(
        "base.xml",
        EndUse::WATER_HEATING,
    );
}

#[test]
fn re_added_ev_restores_the_assembled_schema_and_its_driver_columns() {
    assert_removed_and_re_added_equipment_restores_the_assembled_schema(
        "base-misc-loads-large-uncommon.xml",
        EndUse::EV,
    );
}

/// The battery declares an `active_power_kw` telemetry field in kW, so the
/// assembled Electric Power column carries the telemetry unit provenance:
/// the metadata the parity tests above compare is not empty.
#[test]
fn assembled_battery_power_column_carries_its_telemetry_unit_provenance() {
    let output_dir = tempfile::tempdir().expect("tempdir");
    let dwelling = ochre_dwelling("base-battery.xml", &output_dir);
    let name = equipment_with_end_use(&dwelling, &EndUse::BATTERY);
    let column = format!("{name} Electric Power (kW)");

    let columns = schema_columns(&dwelling);
    let metadata = columns
        .get(&column)
        .unwrap_or_else(|| panic!("the assembled schema has '{column}'"));
    assert_eq!(
        metadata.get("unit_source").map(String::as_str),
        Some("telemetry_field")
    );
}
