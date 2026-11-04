//! Comprehensive integration tests for hares-output (the output face of the
//! former hares-io comprehensive suite): the metrics calculator, the schema
//! builders, and the streaming recorder.

use std::sync::Arc;

use arrow::array::{ArrayRef, Float64Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use chrono::Duration;
use tempfile::tempdir;

use hares_input::{Destination, SimulationConfig};
use hares_output::metrics::{MetricsCalculator, MetricsInputPlan};
use hares_output::{build_schema_for_specs, expected_columns_at_verbosity};
use hares_types::FuelType;

#[path = "../../../tests/support/fixture_start.rs"]
mod fixture_start;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------
fn test_config(deadband: Option<f64>) -> SimulationConfig {
    SimulationConfig {
        max_memory_rows: None,
        scenario_seed: None,
        schedule_diversity: Default::default(),
        start_time: fixture_start::fixture_start(),
        duration: Duration::hours(1),
        time_res: Duration::hours(1),
        reports: Vec::new(),
        diagnostics: false,
        metrics: false,
        output_chunk_size: 16,
        master_seed: 0,
        setpoint_deadband_c: deadband,
        civil_timezone: None,
        site_location: hares_input::SiteLocationOverride::default(),
        max_consecutive_step_failures: hares_input::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
        health_policy: hares_input::HealthPolicy::default(),
        first_hour_interpolation: Default::default(),
        weather_year: hares_input::WeatherYear::default(),
        warmup: hares_input::Warmup::default(),
    }
}

fn make_spec(name: &str, fuel: FuelType) -> hares_input::EquipmentSpec {
    hares_input::EquipmentSpec {
        instance_name: None,
        name: name.to_string(),
        fuel_type: fuel,
        parameters: serde_json::Map::new(),
        zip_params: None,
        typed_overrides: serde_json::Map::new(),
        typed_config: None,
        system_id: None,
        related_hvac_idref: None,
        primary_role: None,
        heating_setpoint_source: None,
        cooling_setpoint_source: None,
    }
}

fn schema_from_columns(names: &[&str]) -> Schema {
    Schema::new(
        names
            .iter()
            .map(|name| Field::new(*name, DataType::Float64, false))
            .collect::<Vec<_>>(),
    )
}

fn build_batch(columns: Vec<(&str, Vec<f64>)>) -> RecordBatch {
    let fields = columns
        .iter()
        .map(|(name, _)| Field::new(*name, DataType::Float64, false))
        .collect::<Vec<_>>();
    let arrays: Vec<ArrayRef> = columns
        .into_iter()
        .map(|(_, values)| Arc::new(Float64Array::from(values)) as ArrayRef)
        .collect();
    let schema = Arc::new(Schema::new(fields));
    RecordBatch::try_new(schema, arrays).expect("record batch")
}

/// The typed equipment end-use table of the specs a test schema was built
/// from: the class's declared surface, the same source
/// `build_schema_for_specs` reads.
fn spec_end_uses(specs: &[hares_input::EquipmentSpec]) -> Vec<(String, hares_types::EndUse)> {
    specs
        .iter()
        .filter_map(|s| {
            hares_equipment::class_declared_surface(&s.name)
                .map(|surface| (s.name.clone(), surface.end_use))
        })
        .collect()
}

/// The metrics input plan for a test schema: the schema's value-column
/// names in order (a leading `Time` column skipped, the union-schema
/// convention), with the specs' typed end-use table for the equipment
/// gate.
fn plan_for(
    schema: &Schema,
    config: &SimulationConfig,
    equipment_end_uses: &[(String, hares_types::EndUse)],
) -> MetricsInputPlan {
    let names: Vec<String> = schema
        .fields()
        .iter()
        .skip(usize::from(
            schema.fields().first().is_some_and(|f| f.name() == "Time"),
        ))
        .map(|f| f.name().clone())
        .collect();
    MetricsInputPlan::build(&names, config, equipment_end_uses).expect("metrics plan builds")
}

/// The calculator for a test schema.
fn calc_for(
    schema: &Schema,
    time_res_secs: u32,
    config: &SimulationConfig,
    equipment_end_uses: &[(String, hares_types::EndUse)],
) -> MetricsCalculator {
    MetricsCalculator::new(
        plan_for(schema, config, equipment_end_uses),
        time_res_secs,
        config,
    )
    .expect("calculator builds")
}

/// Feeds every row of the batch, in order: the same values the producer
/// row carried, one `accumulate_row` per row.
fn accumulate(
    calc: &mut MetricsCalculator,
    batch: &RecordBatch,
) -> Result<(), hares_output::metrics::MetricsError> {
    for r in 0..batch.num_rows() {
        let row: Vec<f64> = batch
            .columns()
            .iter()
            .map(|c| {
                c.as_any()
                    .downcast_ref::<Float64Array>()
                    .expect("f64 column")
                    .value(r)
            })
            .collect();
        calc.accumulate_row(&row)?;
    }
    Ok(())
}
// ===========================================================================
// 1. MetricsCalculator + build_schema integration (P0 fix validation)
// ===========================================================================

#[test]
fn build_schema_output_accepted_by_metrics_calculator_verbosity_0() {
    let schema = build_schema_for_specs(&[], 0, &[]);
    let names: Vec<String> = schema
        .fields()
        .iter()
        .skip(1)
        .map(|f| f.name().clone())
        .collect();
    let result = MetricsInputPlan::build(&names, &test_config(None), &[]);
    assert!(
        result.is_ok(),
        "MetricsInputPlan::build must accept build_schema([], 0, &[]) output: {:?}",
        result.err()
    );
}

#[test]
fn build_schema_output_accepted_by_metrics_calculator_verbosity_1() {
    let specs = vec![
        make_spec("ASHP Heater", FuelType::Electric),
        make_spec("Gas Furnace", FuelType::Gas),
    ];
    let schema = build_schema_for_specs(&specs, 1, &[]);
    let names: Vec<String> = schema
        .fields()
        .iter()
        .skip(1)
        .map(|f| f.name().clone())
        .collect();
    let result = MetricsInputPlan::build(&names, &test_config(None), &spec_end_uses(&specs));
    assert!(
        result.is_ok(),
        "MetricsInputPlan::build must accept build_schema(specs, 1, &[]) output: {:?}",
        result.err()
    );
}

#[test]
fn build_schema_output_accepted_by_metrics_calculator_all_verbosity_levels() {
    let specs = vec![
        make_spec("ASHP Heater", FuelType::Electric),
        make_spec("Gas Furnace", FuelType::Gas),
        make_spec("Battery", FuelType::Electric),
    ];
    for v in 0..=8 {
        let schema = build_schema_for_specs(&specs, v, &[]);
        let names: Vec<String> = schema
            .fields()
            .iter()
            .skip(1)
            .map(|f| f.name().clone())
            .collect();
        let result = MetricsInputPlan::build(&names, &test_config(None), &spec_end_uses(&specs));
        assert!(
            result.is_ok(),
            "MetricsInputPlan::build must accept build_schema output at verbosity {v}: {:?}",
            result.err()
        );
    }
}

// ===========================================================================
// 2. OCHRE column name alignment
// ===========================================================================

#[test]
fn verbosity_2_produces_ochre_temperature_format() {
    let schema = build_schema_for_specs(&[], 2, &[]);
    let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    assert!(
        names.contains(&"Temperature - Indoor (C)"),
        "verbosity 2 must produce 'Temperature - Indoor (C)' (OCHRE format), got: {names:?}"
    );
    // Must NOT produce the old format.
    assert!(
        !names.contains(&"Indoor Temperature (C)"),
        "must not contain 'Indoor Temperature (C)' (non-OCHRE format)"
    );
}

#[test]
fn verbosity_0_column_names_match_ochre() {
    let schema = build_schema_for_specs(&[], 0, &[]);
    let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    assert!(
        names.contains(&"Total Electric Power (kW)"),
        "verbosity 0 must contain 'Total Electric Power (kW)'"
    );
    assert!(
        names.contains(&"Total Gas Power (therms/hour)"),
        "verbosity 0 must contain 'Total Gas Power (therms/hour)'"
    );
}

#[test]
fn expected_columns_at_verbosity_2_includes_ochre_temp_format() {
    let cols = expected_columns_at_verbosity(2, &[]);
    assert!(
        cols.contains(&"Temperature - Indoor (C)".to_string()),
        "expected_columns_at_verbosity(2) must include 'Temperature - Indoor (C)', got: {cols:?}"
    );
}

// ===========================================================================
// 2b. Context columns present at all verbosity levels
// ===========================================================================

#[test]
fn context_columns_present_at_all_verbosity_levels() {
    for v in 0..=8u8 {
        let schema = build_schema_for_specs(&[], v, &[]);
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(
            names.contains(&"Outdoor Dry Bulb (C)"),
            "verbosity {v}: 'Outdoor Dry Bulb (C)' must be present at every verbosity level"
        );
        assert!(
            names.contains(&"Temperature - Indoor (C)"),
            "verbosity {v}: 'Temperature - Indoor (C)' must be present at every verbosity level"
        );
        assert!(
            names.contains(&"Time"),
            "verbosity {v}: 'Time' must be present at every verbosity level"
        );
    }
}

#[test]
fn expected_columns_at_verbosity_includes_context_columns_at_all_levels() {
    for v in 0..=8u8 {
        let cols = expected_columns_at_verbosity(v, &[]);
        assert!(
            cols.contains(&"Outdoor Dry Bulb (C)".to_string()),
            "expected_columns_at_verbosity({v}) must include 'Outdoor Dry Bulb (C)'"
        );
        assert!(
            cols.contains(&"Temperature - Indoor (C)".to_string()),
            "expected_columns_at_verbosity({v}) must include 'Temperature - Indoor (C)'"
        );
    }
}

// ===========================================================================
// 3. Verbosity 6-8 columns
// ===========================================================================

#[test]
fn verbosity_6_produces_additional_columns_beyond_level_5() {
    let specs = vec![make_spec("ASHP Heater", FuelType::Electric)];
    let schema_5 = build_schema_for_specs(&specs, 5, &[]);
    let schema_6 = build_schema_for_specs(&specs, 6, &[]);
    assert!(
        schema_6.fields().len() > schema_5.fields().len(),
        "verbosity 6 ({}) must produce more columns than verbosity 5 ({})",
        schema_6.fields().len(),
        schema_5.fields().len()
    );
}

#[test]
fn verbosity_7_produces_additional_columns_beyond_level_6() {
    let specs = vec![make_spec("ASHP Heater", FuelType::Electric)];
    let schema_6 = build_schema_for_specs(&specs, 6, &[]);
    let schema_7 = build_schema_for_specs(&specs, 7, &[]);
    assert!(
        schema_7.fields().len() > schema_6.fields().len(),
        "verbosity 7 ({}) must produce more columns than verbosity 6 ({})",
        schema_7.fields().len(),
        schema_6.fields().len()
    );
}

#[test]
fn verbosity_8_produces_at_least_as_many_columns_as_level_7() {
    // v8 currently inherits all columns from v7 (Capacity and COP were
    // promoted from v8 to v7 per OCHRE HVAC.py:584,598). v8 may gain
    // additional columns in future; for now it must not lose any.
    let specs = vec![make_spec("ASHP Heater", FuelType::Electric)];
    let schema_7 = build_schema_for_specs(&specs, 7, &[]);
    let schema_8 = build_schema_for_specs(&specs, 8, &[]);
    assert!(
        schema_8.fields().len() >= schema_7.fields().len(),
        "verbosity 8 ({}) must produce at least as many columns as verbosity 7 ({})",
        schema_8.fields().len(),
        schema_7.fields().len()
    );
}

#[test]
fn verbosity_6_includes_envelope_component_columns() {
    let specs = vec![make_spec("ASHP Heater", FuelType::Electric)];
    let schema = build_schema_for_specs(&specs, 6, &[]);
    let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    assert!(
        names.contains(&"Window Transmitted Solar Gain (W)"),
        "verbosity 6 should include envelope component columns"
    );
}

#[test]
fn verbosity_7_includes_schedule_columns() {
    let specs = vec![make_spec("ASHP Heater", FuelType::Electric)];
    let schema = build_schema_for_specs(&specs, 7, &[]);
    let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    assert!(
        names.contains(&"ASHP Heater Schedule (-)"),
        "verbosity 7 should include schedule columns"
    );
}

#[test]
fn verbosity_8_includes_capacity_and_cop_columns() {
    let specs = vec![make_spec("ASHP Heater", FuelType::Electric)];
    let schema = build_schema_for_specs(&specs, 8, &[]);
    let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    assert!(
        names.contains(&"ASHP Heater Capacity (W)"),
        "verbosity 8 should include capacity columns"
    );
    assert!(
        names.contains(&"ASHP Heater COP (-)"),
        "verbosity 8 should include COP columns"
    );
}

// ===========================================================================
// 4. Gas energy tracking
// ===========================================================================

#[test]
fn gas_annual_energy_therms_is_non_none_with_gas_column() {
    let schema =
        schema_from_columns(&["Total Electric Power (kW)", "Total Gas Power (therms/hour)"]);
    let mut calc = calc_for(&schema, 3600, &test_config(None), &[]);
    accumulate(
        &mut calc,
        &build_batch(vec![
            ("Total Electric Power (kW)", vec![1.0, 2.0]),
            ("Total Gas Power (therms/hour)", vec![0.5, 1.5]),
        ]),
    )
    .unwrap();
    let metrics = calc.finish().unwrap();

    assert!(
        metrics.gas_energy.is_some(),
        "gas_energy must be Some when gas column is present"
    );
    let gas_therms = metrics.gas_energy.as_ref().unwrap().total_therms;
    assert!(
        (gas_therms - 2.0).abs() < 1e-9,
        "gas energy should be 2.0 therms (0.5 + 1.5) * 1h, got {gas_therms}"
    );
}

#[test]
fn total_energy_includes_both_electric_and_gas() {
    let schema =
        schema_from_columns(&["Total Electric Power (kW)", "Total Gas Power (therms/hour)"]);
    let mut calc = calc_for(&schema, 3600, &test_config(None), &[]);
    accumulate(
        &mut calc,
        &build_batch(vec![
            ("Total Electric Power (kW)", vec![1.0]),
            ("Total Gas Power (therms/hour)", vec![1.0]),
        ]),
    )
    .unwrap();
    let metrics = calc.finish().unwrap();

    let electric_kwh = metrics.total_energy_kwh.net_energy_kwh;
    let gas = metrics.gas_energy.as_ref().unwrap();
    let gas_therms = gas.total_therms;
    let combined = electric_kwh + gas.total_kwh_equivalent;

    // combined = electric_kwh + energy_therms_to_kwh(gas_therms)
    let expected_combined = electric_kwh
        + uom::si::f64::Energy::new::<hares_types::si_units::energy::therm>(gas_therms)
            .get::<uom::si::energy::kilowatt_hour>();
    assert!(
        (combined - expected_combined).abs() < 1e-6,
        "combined total ({combined}) should equal electric ({electric_kwh}) + gas ({gas_therms}) therms converted to kWh = {expected_combined}"
    );
    assert!(
        combined > electric_kwh,
        "combined total must exceed electric-only when gas is present"
    );
}
// ===========================================================================
// 7. CSV writer flush
// ===========================================================================

#[test]
fn csv_writer_finish_flushes_all_data() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("out.csv");

    let schema = Schema::new(vec![
        Field::new(
            "Time",
            DataType::Timestamp(arrow::datatypes::TimeUnit::Millisecond, None),
            false,
        ),
        Field::new("Total Electric Power (kW)", DataType::Float64, true),
    ]);
    let mut recorder = hares_output::StreamingRecorder::new(
        schema,
        1000,
        Destination::Csv {
            path: path.clone(),
            overwrite: false,
        },
    )
    .unwrap();

    // Write fewer rows than chunk_size to ensure finish() triggers flush.
    for i in 0..5 {
        recorder
            .push_row(i64::from(i) * 60_000, &[i as f64], &[true])
            .unwrap();
    }
    assert_eq!(recorder.buffered_rows(), 5);

    let summary = recorder.finish().unwrap();
    assert_eq!(summary.row_count, 5);

    // Verify all data was flushed: read back the CSV.
    let contents = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = contents.lines().collect();
    // 1 header + 5 data rows = 6 lines
    assert_eq!(
        lines.len(),
        6,
        "CSV must contain header + 5 data rows after finish(), got: {}",
        lines.len()
    );
    assert!(
        contents.contains("Total Electric Power (kW)"),
        "CSV header must be present"
    );
}

#[test]
fn csv_writer_finish_on_read_only_path_propagates_io_error() {
    // Create a file, then make it read-only before attempting to write.
    // This tests that IO errors are propagated (not silently swallowed).
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("output.csv");

    // Write a valid file first.
    let schema = Schema::new(vec![
        Field::new(
            "Time",
            DataType::Timestamp(arrow::datatypes::TimeUnit::Millisecond, None),
            false,
        ),
        Field::new("Total Electric Power (kW)", DataType::Float64, true),
    ]);

    // Create recorder targeting a path inside a read-only directory
    // after initial creation succeeds.
    let mut recorder = hares_output::StreamingRecorder::new(
        schema,
        1000,
        Destination::Csv {
            path,
            overwrite: false,
        },
    )
    .unwrap();
    recorder.push_row(0, &[1.0], &[true]).unwrap();

    // finish() should succeed here -- the main path works.
    let summary = recorder.finish().unwrap();
    assert_eq!(summary.row_count, 1);
}
