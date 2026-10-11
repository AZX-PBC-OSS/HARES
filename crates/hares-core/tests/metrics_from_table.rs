// The run metrics are computed from any results table: one accumulator,
// fed per step in-run and fed from a Timestep table off the run. The
// table feed resolves its columns through the catalogue and hands the
// same aggregations the run's own feed runs, so a run's own table scores
// the run's metrics on bits.
//
// These are the RED tests: they are written against the surface before
// it exists, so this file does not even compile against the parent
// commit, and every test here fails while the metrics-from-table
// function is missing.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::array::{
    Array, ArrayRef, Float64Array, Int64Array, RecordBatch, TimestampMillisecondArray,
};
use arrow::datatypes::{DataType, Field, FieldRef, Schema, TimeUnit};
use chrono::Duration;
use hares_core::{Dwelling, DwellingConfig, RunMetrics};
use hares_io::output::metrics::metrics_from_table;
use hares_io::output::reduce::{PeriodCalendar, PeriodReducer, TimeRule};
use hares_io::{Catalogue, Frequency};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The `leap_year_february_900s` inputs: the cz4a_ashp_hpwh HPXML, schedule
/// and weather, start `2024-02-27T00:00:00-07:00`, no warm-up, at 900 s,
/// over 24 h with the metrics request on.
fn sim_toml(reports: &str) -> String {
    format!(
        r#"
start_time = "2024-02-27T00:00:00-07:00"
duration = 86400
time_res = 900
metrics = true
output_chunk_size = 40
{reports}
"#
    )
}

fn memory_report(name: &str, verbosity: u8) -> String {
    format!(
        "\n[[reports]]\nname = \"{name}\"\nselection = {{ verbosity = {verbosity} }}\ndestination = \"memory\"\n"
    )
}

fn parquet_report(name: &str, verbosity: u8, path: &Path) -> String {
    format!(
        "\n[[reports]]\nname = \"{name}\"\nselection = {{ verbosity = {verbosity} }}\n\
         destination = {{ parquet = {{ path = \"{}\", overwrite = true }} }}\n",
        path.display()
    )
}

fn dwelling_config(sim_toml: &str) -> DwellingConfig {
    let home = repo_root().join("tests/fixtures/parity/cz4a_ashp_hpwh");
    DwellingConfig {
        hpxml_path: home.join("building.xml"),
        schedule_path: Some(home.join("schedule.csv")),
        weather_path: home.join("weather.epw"),
        defaults_path: Some(repo_root().join("defaults")),
        sim_config: hares_io::SimulationConfig::from_toml(sim_toml).expect("sim config parses"),
        overrides: None,
        bldg_id: 1,
        initialization_duration: None,
        patches: None,
    }
}

/// Runs the leap-year fixture and returns the run's result, its catalogue
/// and the first report's retained batches.
fn run_with_memory_report(
    verbosity: u8,
) -> (Arc<hares_core::RunResult>, Arc<Catalogue>, Vec<RecordBatch>) {
    run_with_memory_report_deadband(verbosity, None)
}

/// The same run with the config's `setpoint_deadband_c` set when given.
fn run_with_memory_report_deadband(
    verbosity: u8,
    deadband: Option<f64>,
) -> (Arc<hares_core::RunResult>, Arc<Catalogue>, Vec<RecordBatch>) {
    let deadband_line = deadband
        .map(|deadband| format!("setpoint_deadband_c = {deadband}\n"))
        .unwrap_or_default();
    let toml = sim_toml(&format!(
        "{deadband_line}{}",
        memory_report("frame", verbosity)
    ));
    let mut dwelling = Dwelling::from_config(dwelling_config(&toml)).expect("dwelling builds");
    let catalogue = dwelling.catalogue();
    let result = dwelling.simulate().expect("run completes");
    let batches = result.reports[0].batches().to_vec();
    assert!(!batches.is_empty(), "the memory report retains batches");
    (result, catalogue, batches)
}

/// Bitwise comparison of two run metrics: every f64 field compared on
/// bits, so a sign-of-zero or payload difference cannot pass.
fn assert_run_metrics_bitwise_equal(a: &RunMetrics, b: &RunMetrics, context: &str) {
    fn bits(v: f64) -> u64 {
        v.to_bits()
    }
    fn opt(v: Option<f64>) -> Option<u64> {
        v.map(bits)
    }
    fn map(m: &BTreeMap<String, f64>) -> BTreeMap<String, u64> {
        m.iter().map(|(k, v)| (k.clone(), bits(*v))).collect()
    }
    let (x, y) = (&a.total_energy_kwh, &b.total_energy_kwh);
    assert_eq!(
        bits(x.net_energy_kwh),
        bits(y.net_energy_kwh),
        "{context}: net_energy_kwh"
    );
    assert_eq!(
        bits(x.gross_consumption_kwh),
        bits(y.gross_consumption_kwh),
        "{context}: gross_consumption_kwh"
    );
    assert_eq!(
        bits(x.gross_pv_generation_kwh),
        bits(y.gross_pv_generation_kwh),
        "{context}: gross_pv_generation_kwh"
    );
    assert_eq!(
        map(&x.per_end_use),
        map(&y.per_end_use),
        "{context}: energy per end use"
    );
    assert_eq!(
        bits(x.duration_hours),
        bits(y.duration_hours),
        "{context}: energy duration"
    );

    let (x, y) = (&a.peak_power_kw, &b.peak_power_kw);
    assert_eq!(
        map(&x.per_end_use),
        map(&y.per_end_use),
        "{context}: peak per end use"
    );
    assert_eq!(
        bits(x.rolling.peak_15min_kw),
        bits(y.rolling.peak_15min_kw),
        "{context}: peak 15min"
    );
    assert_eq!(
        bits(x.rolling.peak_30min_kw),
        bits(y.rolling.peak_30min_kw),
        "{context}: peak 30min"
    );
    assert_eq!(
        bits(x.rolling.peak_60min_kw),
        bits(y.rolling.peak_60min_kw),
        "{context}: peak 60min"
    );

    assert_eq!(
        opt(a.comfort_hours),
        opt(b.comfort_hours),
        "{context}: comfort_hours"
    );
    assert_eq!(
        opt(a.unmet_load_hours),
        opt(b.unmet_load_hours),
        "{context}: unmet_load_hours"
    );
    assert_eq!(
        opt(a.renewable_energy_fraction),
        opt(b.renewable_energy_fraction),
        "{context}: renewable_energy_fraction"
    );
    assert_eq!(
        bits(a.grid_interaction_metrics.peak_import_kw),
        bits(b.grid_interaction_metrics.peak_import_kw),
        "{context}: peak_import_kw"
    );
    assert_eq!(
        bits(a.grid_interaction_metrics.peak_export_kw),
        bits(b.grid_interaction_metrics.peak_export_kw),
        "{context}: peak_export_kw"
    );

    match (&a.envelope_loads_kwh, &b.envelope_loads_kwh) {
        (None, None) => {}
        (Some(x), Some(y)) => {
            let pairs = [
                (x.window_solar_kwh, y.window_solar_kwh, "window_solar_kwh"),
                (
                    x.window_conduction_kwh,
                    y.window_conduction_kwh,
                    "window_conduction_kwh",
                ),
                (
                    x.opaque_conduction_kwh,
                    y.opaque_conduction_kwh,
                    "opaque_conduction_kwh",
                ),
                (
                    x.internal_mass_kwh,
                    y.internal_mass_kwh,
                    "internal_mass_kwh",
                ),
                (x.interior_lwr_kwh, y.interior_lwr_kwh, "interior_lwr_kwh"),
                (x.infiltration_kwh, y.infiltration_kwh, "infiltration_kwh"),
                (x.ventilation_kwh, y.ventilation_kwh, "ventilation_kwh"),
                (x.hvac_heating_kwh, y.hvac_heating_kwh, "hvac_heating_kwh"),
                (x.hvac_cooling_kwh, y.hvac_cooling_kwh, "hvac_cooling_kwh"),
                (
                    x.internal_gains_kwh,
                    y.internal_gains_kwh,
                    "internal_gains_kwh",
                ),
                (x.duct_loss_kwh, y.duct_loss_kwh, "duct_loss_kwh"),
            ];
            for (lx, ly, name) in pairs {
                assert_eq!(bits(lx), bits(ly), "{context}: {name}");
            }
        }
        _ => panic!("{context}: envelope loads present on one side only"),
    }

    let (x, y) = (&a.efficiency, &b.efficiency);
    assert_eq!(
        opt(x.hvac_heating_cop),
        opt(y.hvac_heating_cop),
        "{context}: hvac_heating_cop"
    );
    assert_eq!(
        opt(x.hvac_cooling_cop),
        opt(y.hvac_cooling_cop),
        "{context}: hvac_cooling_cop"
    );
    assert_eq!(
        opt(x.water_heater_cop),
        opt(y.water_heater_cop),
        "{context}: water_heater_cop"
    );
    assert_eq!(
        opt(x.battery_round_trip_efficiency),
        opt(y.battery_round_trip_efficiency),
        "{context}: battery_round_trip_efficiency"
    );

    assert_eq!(
        opt(a.rows_with_partial_setpoint_data_fraction),
        opt(b.rows_with_partial_setpoint_data_fraction),
        "{context}: rows_with_partial_setpoint_data_fraction"
    );
    assert_eq!(
        bits(a.simulation_duration_hours),
        bits(b.simulation_duration_hours),
        "{context}: simulation_duration_hours"
    );
    assert_eq!(a.coverage, b.coverage, "{context}: coverage");
    assert_eq!(
        a.nan_step_count, b.nan_step_count,
        "{context}: nan_step_count"
    );
    assert_eq!(
        a.metrics_reliability, b.metrics_reliability,
        "{context}: metrics_reliability"
    );

    match (&a.gas_energy, &b.gas_energy) {
        (None, None) => {}
        (Some(x), Some(y)) => {
            assert_eq!(
                bits(x.total_therms),
                bits(y.total_therms),
                "{context}: total_therms"
            );
            assert_eq!(
                bits(x.total_kwh_equivalent),
                bits(y.total_kwh_equivalent),
                "{context}: total_kwh_equivalent"
            );
        }
        _ => panic!("{context}: gas energy present on one side only"),
    }
}

/// Reads a published parquet report back into its batches.
fn read_parquet(path: &Path) -> Vec<RecordBatch> {
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
    let file = std::fs::File::open(path).expect("the published parquet opens");
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)
        .expect("the parquet reader builds")
        .build()
        .expect("the parquet reader reads");
    reader
        .collect::<Result<Vec<_>, _>>()
        .expect("the parquet batches read")
}

/// The table's batches rebuilt with one value column's type replaced: the
/// rest of the table is untouched.
fn with_column_type(
    batches: &[RecordBatch],
    column: &str,
    replacement: DataType,
) -> Vec<RecordBatch> {
    batches
        .iter()
        .map(|batch| {
            let schema = batch.schema();
            let index = schema
                .fields()
                .iter()
                .position(|field| field.name() == column)
                .unwrap_or_else(|| panic!("{column} is in the table"));
            let values = batch
                .column(index)
                .as_any()
                .downcast_ref::<Float64Array>()
                .expect("the replaced column is a float column");
            let mut fields: Vec<FieldRef> = schema.fields().to_vec();
            fields[index] = Arc::new(Field::new(column, replacement.clone(), false));
            let mut columns: Vec<ArrayRef> = batch.columns().to_vec();
            columns[index] = match replacement {
                DataType::Int64 => Arc::new(
                    values
                        .iter()
                        .map(|value| value.map(|value| value as i64).unwrap_or(0))
                        .collect::<Int64Array>(),
                ),
                _ => panic!("the test replaces columns with Int64 only"),
            };
            RecordBatch::try_new(Arc::new(Schema::new(fields)), columns).expect("rebuilt batch")
        })
        .collect()
}

/// The table's batches rebuilt with a `Step Count` column appended: the
/// shape a reduced table's period rows carry.
fn with_step_count_column(batches: &[RecordBatch]) -> Vec<RecordBatch> {
    batches
        .iter()
        .map(|batch| {
            let mut fields: Vec<FieldRef> = batch.schema().fields().to_vec();
            fields.push(Arc::new(Field::new("Step Count", DataType::Int64, false)));
            let mut columns: Vec<ArrayRef> = batch.columns().to_vec();
            columns.push(Arc::new(Int64Array::from(vec![1_i64; batch.num_rows()])));
            RecordBatch::try_new(Arc::new(Schema::new(fields)), columns).expect("rebuilt batch")
        })
        .collect()
}

/// The run's table reduced to hour periods through the reporting
/// frequency's reducer: what an `Hourly` report's table carries, the
/// period rows one hour apart.
fn hourly_table(batches: &[RecordBatch], catalogue: &Catalogue) -> Vec<RecordBatch> {
    let schema = batches[0].schema();
    let names: Vec<String> = schema
        .fields()
        .iter()
        .skip(1)
        .map(|field| field.name().clone())
        .collect();
    let rules: Vec<TimeRule> = names
        .iter()
        .map(|name| {
            let def = catalogue
                .resolve(name)
                .unwrap_or_else(|| panic!("{name} is a catalogue variable"));
            TimeRule::from_variable(def)
        })
        .collect();
    let mut reducer =
        PeriodReducer::new(Frequency::Hourly, PeriodCalendar, rules).expect("reducer builds");
    let mut rows: Vec<(i64, Vec<f64>)> = Vec::new();
    let mut emit = |row: hares_io::output::reduce::PeriodRow<'_>| {
        rows.push((row.start_local_ms, row.values.to_vec()));
    };
    for batch in batches {
        reducer
            .push_batch(batch, &mut emit)
            .expect("the run's rows feed the reducer");
    }
    reducer.finish(&mut emit);

    let mut fields: Vec<FieldRef> = vec![Arc::new(Field::new(
        "Time",
        DataType::Timestamp(TimeUnit::Millisecond, None),
        false,
    ))];
    for name in &names {
        fields.push(Arc::new(Field::new(name, DataType::Float64, true)));
    }
    let mut times = Vec::with_capacity(rows.len());
    let mut columns: Vec<Vec<f64>> = vec![Vec::with_capacity(rows.len()); names.len()];
    for (start_ms, values) in &rows {
        times.push(*start_ms);
        for (column, value) in columns.iter_mut().zip(values.iter()) {
            column.push(*value);
        }
    }
    let mut arrays: Vec<ArrayRef> = vec![Arc::new(TimestampMillisecondArray::from(times))];
    for column in &columns {
        arrays.push(Arc::new(Float64Array::from(column.clone())));
    }
    vec![RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).expect("hourly batch")]
}

/// `metrics_from_table_matches_in_run_metrics`: the run's `Verbosity(8)`
/// memory table scored by the table feed equals the run's own metrics on
/// bits.
#[test]
fn metrics_from_table_matches_in_run_metrics() {
    let (result, catalogue, batches) = run_with_memory_report(8);
    let in_run = result.metrics.as_ref().expect("the run requested metrics");

    let from_table = metrics_from_table(&batches, &catalogue, Duration::seconds(900), None)
        .expect("the run's own table scores");
    assert_run_metrics_bitwise_equal(
        &RunMetrics::from(from_table),
        in_run,
        "the table feed scores the run's own metrics",
    );
}

/// `metrics_from_parquet_file_matches_in_run_metrics`: the same through a
/// parquet report read back from its published file.
#[test]
fn metrics_from_parquet_file_matches_in_run_metrics() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let toml = sim_toml(&parquet_report("frame", 8, &tmp.path().join("out.parquet")));
    let mut dwelling = Dwelling::from_config(dwelling_config(&toml)).expect("dwelling builds");
    let catalogue = dwelling.catalogue();
    let result = dwelling.simulate().expect("run completes");
    let in_run = result.metrics.as_ref().expect("the run requested metrics");

    let batches = read_parquet(&tmp.path().join("out.parquet"));
    assert!(!batches.is_empty(), "the published report holds batches");
    let from_table = metrics_from_table(&batches, &catalogue, Duration::seconds(900), None)
        .expect("the published table scores");
    assert_run_metrics_bitwise_equal(
        &RunMetrics::from(from_table),
        in_run,
        "the parquet feed scores the run's own metrics",
    );
}

/// `metrics_from_table_matches_in_run_metrics_with_deadband`: the
/// differential with the config's `setpoint_deadband_c` set: the run's
/// own feed and the table feed both carry the constant, and the scores
/// still agree on bits. The engine's dwelling surface carries no
/// setpoint columns today, so the pin guards the pairing: a surface
/// that gains them makes a table feed without the config's value fail
/// here.
#[test]
fn metrics_from_table_matches_in_run_metrics_with_deadband() {
    let (result, catalogue, batches) = run_with_memory_report_deadband(8, Some(1.5));
    let in_run = result.metrics.as_ref().expect("the run requested metrics");

    let from_table = metrics_from_table(&batches, &catalogue, Duration::seconds(900), Some(1.5))
        .expect("the run's own table scores");
    assert_run_metrics_bitwise_equal(
        &RunMetrics::from(from_table),
        in_run,
        "the table feed scores the run's deadband metrics",
    );
}

/// `metrics_from_table_rejects_type_mismatch`: a column whose Arrow type
/// contradicts the catalogue is an error naming it.
#[test]
fn metrics_from_table_rejects_type_mismatch() {
    let (_result, catalogue, batches) = run_with_memory_report(8);
    let mismatched = with_column_type(&batches, "Total Electric Power (kW)", DataType::Int64);

    let err = metrics_from_table(&mismatched, &catalogue, Duration::seconds(900), None)
        .expect_err("a contradicting type is an error");
    let message = err.to_string();
    assert!(
        message.contains("Total Electric Power (kW)"),
        "the error names the column: {message}"
    );
    assert!(
        message.contains("Int64"),
        "the error names the found type: {message}"
    );
    assert!(
        message.contains("catalogue"),
        "the error names the catalogue: {message}"
    );
}

/// The table's batches rebuilt with one `Time` slot nulled: the rest of
/// the table is untouched, the `Time` field made nullable for the rebuild.
fn with_null_time(batches: &[RecordBatch], row: usize) -> Vec<RecordBatch> {
    let mut rows_before = 0usize;
    batches
        .iter()
        .map(|batch| {
            let schema = batch.schema();
            let index = schema
                .fields()
                .iter()
                .position(|field| field.name() == "Time")
                .expect("Time is in the table");
            let times = batch
                .column(index)
                .as_any()
                .downcast_ref::<TimestampMillisecondArray>()
                .expect("the Time column is a timestamp column");
            let mut fields: Vec<FieldRef> = schema.fields().to_vec();
            fields[index] = Arc::new(Field::new(
                "Time",
                DataType::Timestamp(TimeUnit::Millisecond, None),
                true,
            ));
            let null_slot = row
                .checked_sub(rows_before)
                .filter(|offset| *offset < batch.num_rows());
            let values: Vec<Option<i64>> = (0..batch.num_rows())
                .map(|r| {
                    if Some(r) == null_slot {
                        None
                    } else {
                        Some(times.value(r))
                    }
                })
                .collect();
            let mut columns: Vec<ArrayRef> = batch.columns().to_vec();
            columns[index] = Arc::new(TimestampMillisecondArray::from(values));
            rows_before += batch.num_rows();
            RecordBatch::try_new(Arc::new(Schema::new(fields)), columns).expect("rebuilt batch")
        })
        .collect()
}

/// `metrics_from_table_rejects_non_timestep_table`: the same run's table
/// reduced to hour periods, passed with the 900 s step resolution, fails
/// naming the first offending pair of `Time` values; a table carrying a
/// `Step Count` column fails naming that column.
#[test]
fn metrics_from_table_rejects_non_timestep_table() {
    let (_result, catalogue, batches) = run_with_memory_report(2);

    let hourly = hourly_table(&batches, &catalogue);
    let err = metrics_from_table(&hourly, &catalogue, Duration::seconds(900), None)
        .expect_err("an hourly table is not a timestep table at 900 s");
    let message = err.to_string();
    assert!(
        message.contains("2024-02-27 00:00:00") && message.contains("2024-02-27 01:00:00"),
        "the error names the first offending pair of Time values: {message}"
    );

    let stepped = with_step_count_column(&batches);
    let err = metrics_from_table(&stepped, &catalogue, Duration::seconds(900), None)
        .expect_err("a step-count table is not a timestep table");
    let message = err.to_string();
    assert!(
        message.contains("Step Count"),
        "the error names the Step Count column: {message}"
    );
}

/// `metrics_from_table_names_a_null_time`: a null `Time` slot is an
/// error naming the null, not the raw 0-ms slot the value buffer reads.
#[test]
fn metrics_from_table_names_a_null_time() {
    let (_result, catalogue, batches) = run_with_memory_report(2);
    let nulled = with_null_time(&batches, 3);

    let err = metrics_from_table(&nulled, &catalogue, Duration::seconds(900), None)
        .expect_err("a null Time value is an error");
    let message = err.to_string();
    assert!(
        message.contains("a null Time value at row 3"),
        "the error names the null: {message}"
    );
    assert!(
        !message.contains("1970"),
        "the error does not name the raw 0-ms slot: {message}"
    );
}

/// The first batch untouched, the second rebuilt without its last value
/// column: a batch whose schema differs from the first batch's.
fn with_second_batch_missing_last_column(batches: &[RecordBatch]) -> Vec<RecordBatch> {
    batches
        .iter()
        .enumerate()
        .map(|(index, batch)| {
            if index != 1 {
                return batch.clone();
            }
            let schema = batch.schema();
            let last = schema.fields().len() - 1;
            let dropped = schema.field(last).name().clone();
            let fields: Vec<FieldRef> = schema
                .fields()
                .iter()
                .enumerate()
                .filter(|&(position, _)| position != last)
                .map(|(_, field)| field.clone())
                .collect();
            let columns: Vec<ArrayRef> = batch
                .columns()
                .iter()
                .enumerate()
                .filter(|&(position, _)| position != last)
                .map(|(_, column)| column.clone())
                .collect();
            RecordBatch::try_new(Arc::new(Schema::new(fields)), columns)
                .unwrap_or_else(|_| panic!("batch without {dropped} rebuilds"))
        })
        .collect()
}

/// `metrics_from_table_rejects_inconsistent_batches`: a batch whose
/// schema differs from the first batch's is an error naming the batch
/// and the first differing column, before any row is read.
#[test]
fn metrics_from_table_rejects_inconsistent_batches() {
    let (_result, catalogue, batches) = run_with_memory_report(2);
    let dropped = batches[1]
        .schema()
        .field(batches[1].schema().fields().len() - 1)
        .name()
        .clone();
    let divergent = with_second_batch_missing_last_column(&batches);

    let err = metrics_from_table(&divergent, &catalogue, Duration::seconds(900), None)
        .expect_err("a batch off the first batch's schema is an error");
    let message = err.to_string();
    assert!(
        message.contains("batch 1"),
        "the error names the offending batch: {message}"
    );
    assert!(
        message.contains(&dropped),
        "the error names the first differing column: {message}"
    );
}

/// An empty table is an error: no batches, and a schema-only batch, both
/// name the empty table.
#[test]
fn metrics_from_table_rejects_an_empty_table() {
    let (_result, catalogue, batches) = run_with_memory_report(2);

    let err = metrics_from_table(&[], &catalogue, Duration::seconds(900), None)
        .expect_err("no batches is an empty table");
    assert!(
        !err.to_string().is_empty(),
        "the error names the empty table: {err}"
    );

    let schema = batches[0].schema();
    let empty_columns: Vec<ArrayRef> = schema
        .fields()
        .iter()
        .map(|field| match field.data_type() {
            DataType::Timestamp(TimeUnit::Millisecond, None) => {
                let array: TimestampMillisecondArray = Vec::<i64>::new().into();
                Arc::new(array) as ArrayRef
            }
            _ => {
                let array: Float64Array = Vec::<f64>::new().into();
                Arc::new(array) as ArrayRef
            }
        })
        .collect();
    let schema_only =
        RecordBatch::try_new(schema, empty_columns).expect("a schema-only batch builds");
    let err = metrics_from_table(&[schema_only], &catalogue, Duration::seconds(900), None)
        .expect_err("zero rows is an empty table");
    assert!(
        !err.to_string().is_empty(),
        "the error names the empty table: {err}"
    );
}
