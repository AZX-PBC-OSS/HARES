//! Assembly and every later roster-mutating entrance build the output
//! schema through the same roster planner (`plan_roster_caches`), so
//! equipment present at construction and equipment added afterward with
//! `add_equipment` must produce identical schema field metadata, including
//! telemetry-unit provenance (`enrich_schema_with_telemetry_units`).

use std::path::PathBuf;

use chrono::{Duration, FixedOffset, TimeZone};
use hares_core::{Dwelling, DwellingConfig, SimulationConfig};
use hares_io::OutputFormat;
use hares_types::EndUse;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn sim_config(verbosity: u8) -> SimulationConfig {
    SimulationConfig {
        start_time: FixedOffset::west_opt(7 * 3600)
            .expect("UTC-7 offset is valid")
            .with_ymd_and_hms(2023, 1, 1, 0, 0, 0)
            .unwrap(),
        duration: Duration::hours(1),
        time_res: Duration::seconds(900),
        output_verbosity: verbosity,
        write_output: true,
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

/// An OCHRE sample HPXML fixture, paired with the standard BEopt example
/// schedule and Denver weather file every other ochre-sample test in this
/// crate uses.
fn ochre_dwelling_config(hpxml_name: &str, sim: SimulationConfig) -> DwellingConfig {
    DwellingConfig {
        hpxml_path: project_root()
            .join("tests/fixtures/hpxml/ochre_samples")
            .join(hpxml_name),
        schedule_path: Some(project_root().join("data/examples/BEopt_example_schedule.csv")),
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

/// `base-battery.xml` is `base.xml` plus one `Battery` HPXML block (the only
/// difference between the two fixtures): a battery assembled from the start
/// in one dwelling, the same battery instance moved into a battery-free
/// dwelling of the other through `add_equipment` before the first step, and
/// every schema column the battery owns must carry identical metadata in
/// both.
#[test]
fn equipment_added_before_first_step_gets_the_same_schema_metadata_as_assembly() {
    let verbosity = 5;
    let mut assembled = Dwelling::from_config(ochre_dwelling_config(
        "base-battery.xml",
        sim_config(verbosity),
    ))
    .expect("dwelling with the battery assembled from the start");
    let assembled_schema = assembled
        .recorder
        .as_ref()
        .expect("write_output is true")
        .schema()
        .clone();

    let battery_name = assembled
        .equipment()
        .iter()
        .find(|e| e.descriptor().end_use == EndUse::BATTERY)
        .expect("base-battery.xml assembles a battery")
        .descriptor()
        .name
        .clone();
    let battery = assembled
        .remove_equipment(&battery_name)
        .expect("remove the assembled battery before stepping");

    let mut runtime_added =
        Dwelling::from_config(ochre_dwelling_config("base.xml", sim_config(verbosity)))
            .expect("dwelling without the battery");
    // Clear the rest of the assembled roster first so the battery's
    // explicit, dwelling-assigned id (carried over from `assembled`) cannot
    // collide with an id already in `runtime_added`: only the battery's own
    // schema columns are under test here.
    runtime_added
        .clear_equipment()
        .expect("clear the battery-free roster");
    runtime_added
        .add_equipment(battery)
        .expect("add the extracted battery before the first step");
    let runtime_schema = runtime_added
        .recorder
        .as_ref()
        .expect("write_output is true")
        .schema()
        .clone();

    let battery_columns: Vec<&str> = assembled_schema
        .fields()
        .iter()
        .map(|f| f.name().as_str())
        .filter(|name| name.starts_with(&battery_name))
        .collect();
    assert!(
        !battery_columns.is_empty(),
        "the battery must own at least one schema column to make this test meaningful"
    );

    // The battery declares an `active_power_kw` telemetry field with unit
    // `kW`, so its Electric Power column must carry `unit_source` once
    // enrichment runs. Checking it on the assembled side first catches the
    // deeper break this fix closes: `auto_register_actors` runs right after
    // construction and, before this fix, rebuilt the schema through a path
    // that never enriched it, discarding assembly's own enrichment even
    // with no `add_equipment` call anywhere in the picture.
    let electric_power_column = format!("{battery_name} Electric Power (kW)");
    let assembled_power_field = assembled_schema
        .fields()
        .iter()
        .find(|f| f.name() == &electric_power_column)
        .unwrap_or_else(|| panic!("assembled schema is missing '{electric_power_column}'"));
    assert_eq!(
        assembled_power_field
            .metadata()
            .get("unit_source")
            .map(String::as_str),
        Some("telemetry_field"),
        "the assembled battery's '{electric_power_column}' column must carry unit_source: \
         telemetry_field, surviving the auto_register_actors schema rebuild that follows \
         construction"
    );

    for column_name in battery_columns {
        let assembled_field = assembled_schema
            .fields()
            .iter()
            .find(|f| f.name() == column_name)
            .expect("found above");
        let runtime_field = runtime_schema
            .fields()
            .iter()
            .find(|f| f.name() == column_name)
            .unwrap_or_else(|| {
                panic!("the runtime-added battery is missing column '{column_name}'")
            });
        assert_eq!(
            assembled_field.metadata(),
            runtime_field.metadata(),
            "column '{column_name}' must carry the same schema metadata whether the \
             battery was assembled or added via add_equipment before the first step"
        );
    }
}
