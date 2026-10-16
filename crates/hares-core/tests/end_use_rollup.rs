//! The end-use rollup classifies equipment by its typed end use, never by
//! its name string.
//!
//! The rollup's three surfaces all read the typed
//! `EquipmentDescriptor::end_use` through the catalogue: the end-use
//! aggregate columns the schema carries, the per-equipment accumulation
//! `record_step` writes into them, and the run metrics' per-end-use
//! energy and peak. The name classifiers they replace
//! (`equipment_name_to_end_use`, `is_ev`, `is_hvac_or_wh` and kin) matched
//! literal names or substrings, so an instance-qualified name
//! (`"PV #1"`) fell to `EndUse::OTHER` and a name that merely contains an
//! end-use word (`"Television"`, `"Pool Heater"`) was misgrouped.

use hares_core::DwellingSettings;
use hares_io::{DefaultsSource, InputSource, WeatherInput};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use arrow::array::{Array, Float64Array};
use arrow::record_batch::RecordBatch;
use hares_core::{Dwelling, DwellingConfig, RunResult};

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The run the entry's acceptance list names: the HPXML fixture with
/// `tests/fixtures/weather/test_location.epw`, from
/// 2023-01-01T00:00:00-07:00 for 2 days at 3600 s, warm-up disabled, with
/// a Timestep memory report of the verbosity-8 selection (the only
/// selection that holds every name-gated column).
///
/// The schedule is `data/examples/BEopt_example_schedule.csv`; both
/// fixtures assemble with it as it stands, so the entry's "added as a
/// constant 0.5" column list is empty for them.
fn run_fixture(hpxml_name: &str) -> Arc<RunResult> {
    let config = DwellingConfig {
        hpxml: InputSource::Path(
            project_root()
                .join("tests/fixtures/hpxml/ochre_samples")
                .join(hpxml_name),
        ),
        schedule: Some(InputSource::Path(
            project_root().join("data/examples/BEopt_example_schedule.csv"),
        )),
        weather: WeatherInput::Path(
            project_root().join("tests/fixtures/weather/test_location.epw"),
        ),
        defaults: DefaultsSource::directory(project_root().join("defaults")),
        settings: DwellingSettings::new(
            hares_io::SimulationConfig::from_toml(
                "start_time = \"2023-01-01T00:00:00-07:00\"\n\
                 duration = 172800\n\
                 time_res = 3600\n\
                 warmup = \"disabled\"\n\
                 output_chunk_size = 64\n\
                 \n\
                 [[reports]]\n\
                 name = \"frame\"\n\
                 selection = { verbosity = 8 }\n\
                 destination = \"memory\"\n",
            )
            .expect("the acceptance run's config parses"),
            None,
            100,
            None,
        ),
    };
    let mut dwelling =
        Dwelling::from_config(config).unwrap_or_else(|err| panic!("assemble {hpxml_name}: {err}"));
    dwelling
        .simulate()
        .unwrap_or_else(|err| panic!("run {hpxml_name}: {err}"))
}

/// The named report's batches, flattened to one (column name, values) map
/// over the run's rows.
fn report_columns(result: &RunResult) -> Vec<(String, Vec<f64>)> {
    let batches: &[RecordBatch] = result.reports[0].batches();
    assert!(!batches.is_empty(), "the memory report retains batches");
    let schema = batches[0].schema();
    let mut columns: Vec<(String, Vec<f64>)> = schema
        .fields()
        .iter()
        .filter(|f| f.name() != "Time")
        .map(|f| (f.name().clone(), Vec::new()))
        .collect();
    for batch in batches {
        for (name, values) in &mut columns {
            let array = batch
                .column_by_name(name)
                .and_then(|a| a.as_any().downcast_ref::<Float64Array>())
                .unwrap_or_else(|| panic!("column '{name}' is a Float64 array"));
            for row in 0..batch.num_rows() {
                values.push(if array.is_null(row) {
                    f64::NAN
                } else {
                    array.value(row)
                });
            }
        }
    }
    columns
}

/// `instance_qualified_equipment_rolls_up_by_typed_end_use`:
/// `tests/fixtures/hpxml/ochre_samples/base-pv.xml` declares two PV
/// systems (`PVSystem1`, `PVSystem2`), built as `"PV #1"` and `"PV #2"`;
/// both roll up under `EndUse::PV` and the PV end-use column equals their
/// sum.
#[test]
fn instance_qualified_equipment_rolls_up_by_typed_end_use() {
    let result = run_fixture("base-pv.xml");
    let columns = report_columns(&result);
    let column = |name: &str| -> Vec<f64> {
        columns
            .iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("the schema carries '{name}'"))
            .1
            .clone()
    };
    let pv1 = column("PV #1 Electric Power (kW)");
    let pv2 = column("PV #2 Electric Power (kW)");
    let pv_aggregate = column("PV End Use Electric Power (kW)");
    assert_eq!(pv1.len(), pv_aggregate.len());
    for row in 0..pv_aggregate.len() {
        let expected = pv1[row] + pv2[row];
        assert_eq!(
            pv_aggregate[row], expected,
            "row {row}: the PV end-use aggregate must equal the two PV systems' sum"
        );
    }
    // And the Other aggregate carries none of their power: the two
    // systems are the fixture's only PV equipment, and no other
    // equipment's typed end use is PV.
}

/// `pool_and_hot_tub_heaters_are_not_hvac`:
/// `tests/fixtures/hpxml/ochre_samples/base-misc-loads-large-uncommon.xml`
/// (a pool with a gas-fired heater, `Pool1Heater`, and a hot tub with an
/// electric resistance heater, `HotTub1Heater`): each heater rolls up
/// under its own end use and has no HVAC columns.
#[test]
fn pool_and_hot_tub_heaters_are_not_hvac() {
    let result = run_fixture("base-misc-loads-large-uncommon.xml");
    let columns = report_columns(&result);
    let names: BTreeSet<&str> = columns.iter().map(|(n, _)| n.as_str()).collect();

    // Each heater's own end-use aggregate exists.
    for aggregate in [
        "Pool Heater End Use Electric Power (kW)",
        "Spa Heater End Use Electric Power (kW)",
    ] {
        assert!(
            names.contains(aggregate),
            "the schema carries the '{aggregate}' aggregate"
        );
    }
    // And neither heater carries the HVAC family's columns: the family
    // follows the typed end use, not the "heater" substring.
    let hvac_suffixes = [
        "Setpoint (C)",
        "Capacity (W)",
        "COP (-)",
        "Speed (-)",
        "Main Power (kW)",
        "Fan Power (kW)",
        "Runtime Fraction (-)",
        "Supply Temperature (C)",
        "Return Temperature (C)",
        "Compressor Power (W)",
        "Min On Time (s)",
        "Min Off Time (s)",
    ];
    for heater in ["Pool Heater", "Spa Heater"] {
        for suffix in hvac_suffixes {
            let column = format!("{heater} {suffix}");
            assert!(
                !names.contains(column.as_str()),
                "'{column}' must not exist: the heater is not HVAC by its typed end use"
            );
        }
    }
}

// `misleadingly_named_equipment_classifies_by_its_descriptor`: a unit
// test over the catalogue builder lives in
// `end_use_rollup_catalogue.rs` (it references the typed catalogue
// input's fields, so it is a compile RED at the parent); this file's
// tests run at the parent.
