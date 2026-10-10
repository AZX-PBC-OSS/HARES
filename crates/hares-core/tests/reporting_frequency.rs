//! The reporting-frequency acceptance tests over real runs: a report's
//! period rows at minute to run-period frequency, each variable reduced by
//! its declared aggregation, fed from the frozen row plan's step stream.

use std::path::{Path, PathBuf};

use arrow::array::{Float64Array, RecordBatch};
use arrow::datatypes::Schema;
use chrono::{DateTime, Duration, FixedOffset, TimeZone};
use hares_control::DispatchRequest;
use hares_core::{Actor, Dwelling, DwellingConfig};
use hares_io::output::reduce::{PeriodCalendar, PeriodReducer, PeriodRow, TimeRule};
use hares_io::{
    Catalogue, Frequency, OutputFormat, RotationPolicy, SimulationConfig, SiteLocationOverride,
};
use hares_types::{
    AcrossHomes, Aggregation, EnumLabels, EnvironmentState, Sampling, Telemetry, TelemetryField,
    Unit, VariableKind,
};

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn site_offset() -> FixedOffset {
    FixedOffset::east_opt(-7 * 3600).expect("offset")
}

fn sim_config(
    start: DateTime<FixedOffset>,
    duration_s: i64,
    step_s: i64,
    verbosity: u8,
    chunk: usize,
    dir: &Path,
) -> SimulationConfig {
    SimulationConfig {
        start_time: start,
        duration: Duration::seconds(duration_s),
        time_res: Duration::seconds(step_s),
        output_verbosity: verbosity,
        write_output: true,
        output_path: Some(dir.join("reporting_frequency.csv")),
        output_format: OutputFormat::Csv,
        output_chunk_size: chunk,
        setpoint_deadband_c: None,
        master_seed: 0,
        civil_timezone: None,
        site_location: SiteLocationOverride::default(),
        retain_batches: true,
        rotation: RotationPolicy::None,
        max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
    }
}

fn cz4a_dwelling(
    start: DateTime<FixedOffset>,
    duration_s: i64,
    step_s: i64,
    verbosity: u8,
    chunk: usize,
    dir: &Path,
) -> Dwelling {
    let fixture = project_root().join("tests/fixtures/parity/cz4a_ashp_hpwh");
    let config = DwellingConfig {
        hpxml_path: fixture.join("building.xml"),
        schedule_path: Some(fixture.join("schedule.csv")),
        weather_path: fixture.join("weather.epw"),
        sim_config: sim_config(start, duration_s, step_s, verbosity, chunk, dir),
        defaults_path: Some(project_root().join("defaults")),
        overrides: None,
        bldg_id: 1,
        initialization_duration: None,
        patches: None,
    };
    Dwelling::from_config(config).expect("cz4a dwelling builds")
}

/// The run's steps, the final partial batch flushed: the dwelling and the
/// run's Timestep record batches, retained in memory.
fn run_cz4a(
    start: DateTime<FixedOffset>,
    duration_s: i64,
    step_s: i64,
    verbosity: u8,
    chunk: usize,
    dir: &Path,
) -> Dwelling {
    let mut dwelling = cz4a_dwelling(start, duration_s, step_s, verbosity, chunk, dir);
    let steps = usize::try_from(duration_s / step_s).expect("steps");
    for _ in 0..steps {
        dwelling.step().expect("dwelling steps");
    }
    dwelling
        .recorder
        .as_mut()
        .expect("recorder with retained batches")
        .flush()
        .expect("final flush");
    dwelling
}

/// The step stream's naive local milliseconds: the run's fixed offset
/// applied to the step instant, the time the row's own timestamp column
/// carries.
fn local_ms_of_run(t: DateTime<FixedOffset>) -> i64 {
    t.timestamp_millis() + i64::from(t.offset().local_minus_utc()) * 1000
}

/// The period row's Time label: the naive local period start rendered by
/// the same function the step rows render with (chrono `%+` at the run's
/// site offset).
fn time_label(start_local_ms: i64, site: FixedOffset) -> String {
    use chrono::{Datelike, Timelike};
    let naive = DateTime::from_timestamp_millis(start_local_ms)
        .expect("ms in range")
        .naive_utc();
    site.with_ymd_and_hms(
        naive.year(),
        naive.month(),
        naive.day(),
        naive.hour(),
        naive.minute(),
        naive.second(),
    )
    .single()
    .expect("local time")
    .format("%+")
    .to_string()
}

/// One rule per value column of the recorder's schema, in schema order:
/// the rule each variable declares.
fn rules_for(schema: &Schema, catalogue: &Catalogue) -> Vec<TimeRule> {
    schema
        .fields()
        .iter()
        .skip(1)
        .map(|field| {
            let def = catalogue.resolve(field.name()).unwrap_or_else(|| {
                panic!("schema column {} is not a catalogue variable", field.name())
            });
            TimeRule::from_variable(def)
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
struct Collected {
    start_local_ms: i64,
    step_count: u32,
    values: Vec<f64>,
    valid_steps: Vec<u32>,
}

fn sink(rows: &mut Vec<Collected>) -> impl FnMut(PeriodRow<'_>) + '_ {
    |row| {
        rows.push(Collected {
            start_local_ms: row.start_local_ms,
            step_count: row.step_count,
            values: row.values.to_vec(),
            valid_steps: row.valid_steps.to_vec(),
        });
    }
}

fn collect(reducer: PeriodReducer, rows: &mut Vec<Collected>) {
    reducer.finish(&mut sink(rows));
}

fn reduce_batches(
    frequency: Frequency,
    batches: &[RecordBatch],
    rules: Vec<TimeRule>,
) -> Vec<Collected> {
    let mut reducer = PeriodReducer::new(frequency, PeriodCalendar, rules).expect("reducer builds");
    let mut rows = Vec::new();
    for batch in batches {
        reducer
            .push_batch(batch, &mut sink(&mut rows))
            .expect("the run's own batches feed the reducer");
    }
    collect(reducer, &mut rows);
    rows
}

fn column_index(schema: &Schema, predicate: impl Fn(&str) -> bool) -> usize {
    schema
        .fields()
        .iter()
        .position(|field| predicate(field.name()))
        .expect("the schema carries a matching column")
}

fn column_values(batches: &[RecordBatch], column: usize) -> Vec<f64> {
    batches
        .iter()
        .flat_map(|batch| {
            batch
                .column(column)
                .as_any()
                .downcast_ref::<Float64Array>()
                .expect("float64 value column")
                .iter()
                .map(|v| v.unwrap_or(0.0))
                .collect::<Vec<f64>>()
        })
        .collect()
}

#[test]
fn hourly_energy_sum_equals_timestep_sum() {
    let dir = tempfile::tempdir().expect("tempdir");
    let start = site_offset()
        .with_ymd_and_hms(2023, 1, 1, 0, 0, 0)
        .single()
        .expect("start");
    let dwelling = run_cz4a(start, 86_400, 900, 4, 96, dir.path());
    let batches: Vec<RecordBatch> = dwelling
        .recorder
        .as_ref()
        .expect("retained batches")
        .flushed_batches()
        .to_vec();
    let schema = dwelling
        .recorder
        .as_ref()
        .expect("recorder")
        .schema()
        .clone();
    let rules = rules_for(&schema, &dwelling.catalogue());

    let energy_column = column_index(&schema, |name| name.ends_with("Energy (kWh)"));
    let step_values = column_values(&batches, energy_column);
    assert_eq!(step_values.len(), 96, "24 h at 900 s");

    let hourly = reduce_batches(Frequency::Hourly, &batches, rules);
    assert_eq!(hourly.len(), 24);
    for (h, row) in hourly.iter().enumerate() {
        // The hour's energy: the four timestep rows' sum, added in step
        // order, to 1e-12 relative.
        let expected: f64 = step_values[h * 4..(h + 1) * 4].iter().sum();
        let got = row.values[energy_column - 1];
        assert!(
            (got - expected).abs() <= 1e-12 * expected.abs(),
            "hour {h}: the hourly energy {got} must equal the four steps' sum \
             {expected}"
        );
        assert_eq!(row.step_count, 4, "hour {h}");
        assert_eq!(row.valid_steps[energy_column - 1], 4, "hour {h}");
    }
}

#[test]
fn hourly_power_and_temperature_are_means() {
    let dir = tempfile::tempdir().expect("tempdir");
    let start = site_offset()
        .with_ymd_and_hms(2023, 1, 1, 0, 0, 0)
        .single()
        .expect("start");
    let dwelling = run_cz4a(start, 86_400, 900, 4, 96, dir.path());
    let batches: Vec<RecordBatch> = dwelling
        .recorder
        .as_ref()
        .expect("retained batches")
        .flushed_batches()
        .to_vec();
    let schema = dwelling
        .recorder
        .as_ref()
        .expect("recorder")
        .schema()
        .clone();
    let rules = rules_for(&schema, &dwelling.catalogue());

    let power_column = column_index(&schema, |name| name == "Total Electric Power (kW)");
    let temperature_column = column_index(&schema, |name| name.starts_with("Temperature - "));
    let power_steps = column_values(&batches, power_column);
    let temperature_steps = column_values(&batches, temperature_column);

    let hourly = reduce_batches(Frequency::Hourly, &batches, rules);
    assert_eq!(hourly.len(), 24);
    for (h, row) in hourly.iter().enumerate() {
        for (column, steps) in [
            (power_column, &power_steps),
            (temperature_column, &temperature_steps),
        ] {
            let expected: f64 = steps[h * 4..(h + 1) * 4].iter().sum::<f64>() / 4.0;
            let got = row.values[column - 1];
            assert!(
                (got - expected).abs() <= 1e-12 * expected.abs().max(1.0),
                "hour {h}, column {column}: the hourly mean {got} must equal \
                 the four steps' mean {expected}"
            );
        }
    }
}

/// The enum and running-total probe: an actor declaring an enum variable
/// with three labels and a running total, publishing a scripted mode code
/// and the 1-based step index each step.
struct PeriodProbeActor {
    steps: u64,
    telemetry: Telemetry,
}

impl PeriodProbeActor {
    const NAME: &str = "period_probe";

    /// The declarations the actor registers with: the enum probe and the
    /// running total.
    const OUTPUT_VARIABLES: &'static [TelemetryField] = &[
        TelemetryField::full(
            "mode",
            Unit::Code,
            VariableKind::Enum {
                labels: EnumLabels::Static(&["a", "b", "c"]),
            },
            Aggregation::Mode,
            Sampling::InstantAtEnd,
            "the enum probe's scripted mode code",
        ),
        TelemetryField::quantity(
            "running_total",
            Unit::Count,
            AcrossHomes::Sum,
            Aggregation::Last,
            Sampling::InstantAtEnd,
            "the step ordinal since init",
        ),
    ];

    /// The enum probe's script over two days at 900 s: day 1 splits 48 and
    /// 48 (a tie), day 2 splits 50 and 46.
    fn mode_code(steps: u64) -> f64 {
        match steps {
            1..=48 => 2.0,
            49..=96 => 1.0,
            97..=146 => 0.0,
            _ => 2.0,
        }
    }
}

impl Actor for PeriodProbeActor {
    fn name(&self) -> &str {
        Self::NAME
    }

    fn output_variables(&self) -> &[TelemetryField] {
        Self::OUTPUT_VARIABLES
    }

    fn telemetry(&self) -> Option<&Telemetry> {
        Some(&self.telemetry)
    }

    fn decide(&mut self, _env: &EnvironmentState, _out: &mut Vec<DispatchRequest>) {
        self.steps += 1;
        self.telemetry.set("mode", Self::mode_code(self.steps));
        self.telemetry.set("running_total", self.steps as f64);
    }
}

/// The two-day probe run: the dwelling with the probe actor registered,
/// stepped and flushed.
fn run_probe(duration_s: i64, dir: &Path) -> Dwelling {
    let start = site_offset()
        .with_ymd_and_hms(2023, 1, 1, 0, 0, 0)
        .single()
        .expect("start");
    let mut dwelling = cz4a_dwelling(start, duration_s, 900, 0, 192, dir);
    dwelling
        .add_actor(Box::new(PeriodProbeActor {
            steps: 0,
            telemetry: Telemetry::from_fields(PeriodProbeActor::OUTPUT_VARIABLES),
        }))
        .expect("the probe actor registers");
    let steps = usize::try_from(duration_s / 900).expect("steps");
    for _ in 0..steps {
        dwelling.step().expect("dwelling steps");
    }
    dwelling
        .recorder
        .as_mut()
        .expect("recorder with retained batches")
        .flush()
        .expect("final flush");
    dwelling
}

#[test]
fn daily_enum_column_is_mode_with_lowest_code_tie_break() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dwelling = run_probe(2 * 86_400, dir.path());
    let batches: Vec<RecordBatch> = dwelling
        .recorder
        .as_ref()
        .expect("retained batches")
        .flushed_batches()
        .to_vec();
    let schema = dwelling
        .recorder
        .as_ref()
        .expect("recorder")
        .schema()
        .clone();
    let rules = rules_for(&schema, &dwelling.catalogue());

    let mode_column = column_index(&schema, |name| name == "actor:period_probe:mode");
    let daily = reduce_batches(Frequency::Daily, &batches, rules);
    assert_eq!(daily.len(), 2);
    // Day 1: 48 steps at code 2 and 48 at code 1, a tie, so the lowest
    // code wins. Day 2: 50 at code 0 against 46 at code 2.
    assert_eq!(daily[0].values[mode_column - 1], 1.0);
    assert_eq!(daily[1].values[mode_column - 1], 0.0);
    assert_eq!(daily[0].step_count, 96);
    assert_eq!(daily[1].step_count, 96);
    assert_eq!(daily[0].valid_steps[mode_column - 1], 96);
    let site = site_offset();
    assert_eq!(
        time_label(daily[0].start_local_ms, site),
        "2023-01-01T00:00:00-07:00"
    );
    assert_eq!(
        time_label(daily[1].start_local_ms, site),
        "2023-01-02T00:00:00-07:00"
    );
}

#[test]
fn hourly_running_total_is_last() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dwelling = run_probe(2 * 86_400, dir.path());
    let batches: Vec<RecordBatch> = dwelling
        .recorder
        .as_ref()
        .expect("retained batches")
        .flushed_batches()
        .to_vec();
    let schema = dwelling
        .recorder
        .as_ref()
        .expect("recorder")
        .schema()
        .clone();
    let rules = rules_for(&schema, &dwelling.catalogue());

    let total_column = column_index(&schema, |name| name == "actor:period_probe:running_total");
    let hourly = reduce_batches(Frequency::Hourly, &batches, rules);
    assert_eq!(hourly.len(), 48);
    for (h, row) in hourly.iter().enumerate() {
        // The running total's value at the period's last valid step: the
        // index of the hour's fourth step.
        assert_eq!(
            row.values[total_column - 1],
            f64::from((h as u32 + 1) * 4),
            "hour {h}"
        );
        assert_eq!(row.valid_steps[total_column - 1], 4, "hour {h}");
    }
}

#[test]
fn minutes_15_energy_sum_equals_timestep_sum() {
    let dir = tempfile::tempdir().expect("tempdir");
    let start = site_offset()
        .with_ymd_and_hms(2023, 1, 1, 0, 0, 0)
        .single()
        .expect("start");
    let dwelling = run_cz4a(start, 86_400, 300, 4, 288, dir.path());
    let batches: Vec<RecordBatch> = dwelling
        .recorder
        .as_ref()
        .expect("retained batches")
        .flushed_batches()
        .to_vec();
    let schema = dwelling
        .recorder
        .as_ref()
        .expect("recorder")
        .schema()
        .clone();
    let rules = rules_for(&schema, &dwelling.catalogue());

    let energy_column = column_index(&schema, |name| name.ends_with("Energy (kWh)"));
    let step_values = column_values(&batches, energy_column);
    assert_eq!(step_values.len(), 288, "24 h at 300 s");

    let quarters = reduce_batches(
        Frequency::Minutes(15.try_into().expect("nonzero")),
        &batches,
        rules,
    );
    assert_eq!(quarters.len(), 96);
    for (p, row) in quarters.iter().enumerate() {
        let expected: f64 = step_values[p * 3..(p + 1) * 3].iter().sum();
        let got = row.values[energy_column - 1];
        assert!(
            (got - expected).abs() <= 1e-12 * expected.abs(),
            "quarter {p}: the period's energy {got} must equal the three \
             steps' sum {expected} to 1e-12 relative"
        );
        assert_eq!(row.step_count, 3, "quarter {p}");
    }
    // The labels: quarter hours at :00, :15, :30 and :45.
    let site = site_offset();
    for (p, expected) in [
        (0_usize, "2023-01-01T00:00:00-07:00"),
        (1, "2023-01-01T00:15:00-07:00"),
        (2, "2023-01-01T00:30:00-07:00"),
        (3, "2023-01-01T00:45:00-07:00"),
        (95, "2023-01-01T23:45:00-07:00"),
    ] {
        assert_eq!(time_label(quarters[p].start_local_ms, site), expected);
    }
}

#[test]
fn batch_fed_reducer_equals_row_fed_reducer() {
    let dir = tempfile::tempdir().expect("tempdir");
    let start = site_offset()
        .with_ymd_and_hms(2023, 1, 1, 0, 0, 0)
        .single()
        .expect("start");
    // One row per flushed batch, so every step's row is at hand as the
    // per-step feed runs.
    let mut dwelling = cz4a_dwelling(start, 86_400, 900, 4, 1, dir.path());
    let schema = dwelling
        .recorder
        .as_ref()
        .expect("recorder")
        .schema()
        .clone();
    let rules = rules_for(&schema, &dwelling.catalogue());

    let mut row_fed =
        PeriodReducer::new(Frequency::Hourly, PeriodCalendar, rules.clone()).expect("builds");
    let mut batch_fed =
        PeriodReducer::new(Frequency::Hourly, PeriodCalendar, rules).expect("builds");
    let mut row_rows = Vec::new();
    let mut batch_rows = Vec::new();

    let steps = 86_400_i64 / 900;
    for _ in 0..steps {
        dwelling.step().expect("dwelling steps");
        let batches = dwelling
            .recorder
            .as_ref()
            .expect("retained batches")
            .flushed_batches();
        let batch = batches.last().expect("chunk size one flushes per step");
        // The per-step feed: the run's fixed offset applied to the step's
        // instant, the values read off the frozen plan's recorded row.
        let step_time = dwelling.latest_env().current_time;
        let local_ms = local_ms_of_run(step_time);
        let values: Vec<f64> = (1..batch.num_columns())
            .map(|column| {
                batch
                    .column(column)
                    .as_any()
                    .downcast_ref::<Float64Array>()
                    .expect("float64 value column")
                    .value(0)
            })
            .collect();
        let valid = vec![true; values.len()];
        row_fed
            .push_row(local_ms, &values, &valid, &mut sink(&mut row_rows))
            .expect("the run's rows feed the reducer");
    }
    collect(row_fed, &mut row_rows);

    dwelling
        .recorder
        .as_mut()
        .expect("recorder")
        .flush()
        .expect("final flush");
    let batches: Vec<RecordBatch> = dwelling
        .recorder
        .as_ref()
        .expect("retained batches")
        .flushed_batches()
        .to_vec();
    for batch in &batches {
        batch_fed
            .push_batch(batch, &mut sink(&mut batch_rows))
            .expect("the same run's batches feed the batch side");
    }
    collect(batch_fed, &mut batch_rows);

    assert_eq!(row_rows.len(), 24);
    assert_eq!(
        row_rows, batch_rows,
        "both feeds, on bits: the same period \
        starts, step counts, values and valid steps"
    );
}

#[test]
fn partial_first_and_last_period_carry_step_count() {
    let dir = tempfile::tempdir().expect("tempdir");
    // 91 steps of 900 s from 00:30 to 23:15 local, the end exclusive.
    let start = site_offset()
        .with_ymd_and_hms(2023, 1, 1, 0, 30, 0)
        .single()
        .expect("start");
    let dwelling = run_cz4a(start, 91 * 900, 900, 0, 96, dir.path());
    let batches: Vec<RecordBatch> = dwelling
        .recorder
        .as_ref()
        .expect("retained batches")
        .flushed_batches()
        .to_vec();
    let schema = dwelling
        .recorder
        .as_ref()
        .expect("recorder")
        .schema()
        .clone();
    let rules = rules_for(&schema, &dwelling.catalogue());

    let hourly = reduce_batches(Frequency::Hourly, &batches, rules);
    assert_eq!(hourly.len(), 24);
    assert_eq!(hourly[0].step_count, 2, "the run starts at 00:30");
    for row in &hourly[1..23] {
        assert_eq!(row.step_count, 4);
    }
    assert_eq!(hourly[23].step_count, 1, "the run ends at 23:15 exclusive");
    let site = site_offset();
    assert_eq!(
        time_label(hourly[0].start_local_ms, site),
        "2023-01-01T00:00:00-07:00"
    );
    assert_eq!(
        time_label(hourly[23].start_local_ms, site),
        "2023-01-01T23:00:00-07:00"
    );
}

#[test]
fn run_period_report_has_one_row() {
    let dir = tempfile::tempdir().expect("tempdir");
    let start = site_offset()
        .with_ymd_and_hms(2023, 1, 1, 0, 0, 0)
        .single()
        .expect("start");
    let dwelling = run_cz4a(start, 86_400, 900, 0, 96, dir.path());
    let batches: Vec<RecordBatch> = dwelling
        .recorder
        .as_ref()
        .expect("retained batches")
        .flushed_batches()
        .to_vec();
    let schema = dwelling
        .recorder
        .as_ref()
        .expect("recorder")
        .schema()
        .clone();
    let rules = rules_for(&schema, &dwelling.catalogue());

    let run_rows = reduce_batches(Frequency::RunPeriod, &batches, rules);
    assert_eq!(run_rows.len(), 1);
    assert_eq!(run_rows[0].step_count, 96);
    let site = site_offset();
    assert_eq!(
        time_label(run_rows[0].start_local_ms, site),
        "2023-01-01T00:00:00-07:00"
    );
}
