//! The envelope's mechanical-ventilation flow is the equipment's own
//! per-step flow, not a rated constant: `HoursInOperation` scales the fan
//! energy, the reported flow and the envelope's ventilation load
//! together, and the `balanced` field's resolved value is the one
//! derivation the envelope's recovery gate reads.

use arrow::array::Float64Array;
use arrow::record_batch::RecordBatch;
use hares_core::dwelling::DwellingBlueprintBuilder;
use hares_core::{Dwelling, DwellingConfig, DwellingSettings};
use hares_input::{DefaultsSource, EquipmentSpec, InputSource, SimulationConfig, WeatherInput};
use hares_types::{EndUse, FuelType};
use std::path::PathBuf;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn sim_toml() -> String {
    // One winter day at 900 s: the fixture's January weather keeps the
    // indoor-outdoor difference far from zero, so the forced-ventilation
    // load is readable.
    r#"
start_time = "2023-01-01T00:00:00-07:00"
duration = 86400
time_res = 900
output_chunk_size = 128

[[reports]]
name = "frame"
selection = { verbosity = 8 }
destination = "memory"
"#
    .to_string()
}

fn dwelling_config() -> DwellingConfig {
    let fixture = project_root().join("tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh");
    DwellingConfig {
        hpxml: InputSource::Path(fixture.join("building.xml")),
        schedule: Some(InputSource::Path(fixture.join("schedule.csv"))),
        weather: WeatherInput::Path(fixture.join("weather.epw")),
        defaults: DefaultsSource::directory(project_root().join("python/hares/defaults")),
        settings: DwellingSettings::new(
            SimulationConfig::from_toml(&sim_toml()).expect("sim config parses"),
            None,
            1,
            None,
        ),
    }
}

/// A dwelling whose ventilation fan is a typed HRV with the given
/// `hours_in_operation` and `balanced` fields; every other input is the
/// fixture's own.
fn built_dwelling(hours_in_operation: Option<f64>, balanced: Option<bool>) -> Dwelling {
    let mut data = serde_json::json!({
        "flow_rate_m3_s": 0.035,
        "fan_power_w": 50.0,
        "ventilation_type": "hrv",
        "sensible_effectiveness": 0.70,
        "latent_effectiveness": 0.0
    });
    if let Some(hours) = hours_in_operation {
        data["hours_in_operation"] = serde_json::json!(hours);
    }
    if let Some(balanced) = balanced {
        data["balanced"] = serde_json::json!(balanced);
    }
    let typed = hares_equipment::EquipmentConfig::with_payload(
        "Ventilation Fan".to_string(),
        "Ventilation Fan".to_string(),
        hares_equipment::ConfigPayload::Typed {
            type_name: "Ventilation".to_string(),
            version: 1,
            data: data.clone(),
        },
    );
    let mut builder =
        DwellingBlueprintBuilder::from_config(dwelling_config()).expect("blueprint builds");
    builder.remove_equipment_by_end_use(&[EndUse::VENTILATION]);
    builder
        .add_equipment_spec(EquipmentSpec {
            name: "Ventilation Fan".to_string(),
            instance_name: None,
            fuel_type: FuelType::Electric,
            // The typed config serialized to its parameters, the form the
            // HPXML resolver's own specs carry: the schedule-source power
            // resolution reads the parameters map.
            parameters: data.as_object().cloned().unwrap_or_default(),
            zip_params: None,
            typed_overrides: serde_json::Map::new(),
            typed_config: Some(typed),
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
        })
        .expect("the ventilation spec joins the assembly");
    builder
        .freeze()
        .and_then(|blueprint| blueprint.build())
        .expect("dwelling builds")
}

/// The run's per-step envelope forced-ventilation heat gain (W) for the
/// indoor zone, from the memory report's frames.
fn forced_ventilation_series(hours: Option<f64>, balanced: Option<bool>) -> Vec<f64> {
    let mut dwelling = built_dwelling(hours, balanced);
    let _catalogue = dwelling.catalogue();
    let result = dwelling.simulate().expect("run completes");
    let batches: Vec<RecordBatch> = result.reports[0].batches().to_vec();
    assert!(!batches.is_empty(), "the memory report retains batches");
    let mut out = Vec::new();
    for batch in &batches {
        let column = batch
            .column_by_name("Forced Ventilation Heat Gain - Indoor (W)")
            .unwrap_or_else(|| {
                panic!(
                    "the report carries the forced-ventilation heat gain column: {:?}",
                    batch
                        .schema()
                        .fields()
                        .iter()
                        .map(|f| f.name())
                        .collect::<Vec<_>>()
                )
            });
        let column = column
            .as_any()
            .downcast_ref::<Float64Array>()
            .expect("the heat gain column is f64");
        out.extend(column.iter().flatten());
    }
    out
}

/// A unit running 12 of 24 hours loads the envelope at half the
/// full-schedule rate: the equipment's schedule fraction scales the
/// envelope's forced flow as it scales the fan power, so the 12-hour
/// run's ventilation load integral halves against the otherwise
/// identical `None` run. The band's width is the runs' own feedback: the
/// two zone-temperature trajectories differ because the loads differ,
/// so the ratio concentrates near 0.5 without being pinned to it.
#[test]
fn hours_in_operation_scales_the_envelope_ventilation_load() {
    let full = forced_ventilation_series(None, None);
    let half = forced_ventilation_series(Some(12.0), None);

    let energy_full: f64 = full.iter().map(|v| v.abs()).sum();
    let energy_half: f64 = half.iter().map(|v| v.abs()).sum();
    assert!(
        energy_full > 0.0,
        "the full-schedule run must carry a nonzero forced-ventilation load, got {energy_full}"
    );
    let ratio = energy_half / energy_full;
    assert!(
        (0.3..=0.7).contains(&ratio),
        "the 12-hour schedule must halve the envelope's ventilation load \
         (the fan power already halves), got ratio {ratio}"
    );
}

/// A typed HRV with the `balanced` field unset behaves exactly as the
/// same config with the field set to the type's own derivation (an HRV
/// is balanced): the two runs agree on bits. The unset field resolves
/// from `ventilation_type`, so nothing reads the raw `None` as
/// unbalanced.
#[test]
fn the_unset_balanced_field_agrees_with_the_type_derivation() {
    let unset = forced_ventilation_series(None, None);
    let explicit = forced_ventilation_series(None, Some(true));
    assert_eq!(
        unset.len(),
        explicit.len(),
        "the two runs record the same number of steps"
    );
    for (step, (a, b)) in unset.iter().zip(explicit.iter()).enumerate() {
        assert_eq!(
            a.to_bits(),
            b.to_bits(),
            "step {step}: the unset balanced field must resolve to the HRV's \
             own balance, so the runs agree on bits: {a} vs {b}"
        );
    }
}
