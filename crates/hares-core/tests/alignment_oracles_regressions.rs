#[path = "../../../tests/bestest/cases.rs"]
mod bestest_cases;
#[path = "../../../tests/bestest/reference_bands.rs"]
mod bestest_reference_bands;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration as StdDuration, SystemTime, UNIX_EPOCH};

use arrow::array::{Array, Float64Array, StringArray, TimestampMicrosecondArray};
use arrow::record_batch::RecordBatch;
use hares_core::{Dwelling, DwellingConfig, SimStatus, SimulationEngine};
use hares_io::{OutputFormat, SimulationConfig};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use serde::Deserialize;

const PEAK_HVAC_POWER_REL_PCT_MAX: f64 = 2.0;

#[derive(Debug, Deserialize, Default)]
struct FixtureConfig {
    #[serde(default)]
    simulation: Option<toml::Table>,
    #[serde(default)]
    bldg_id: Option<i64>,
    #[serde(default)]
    initialization_duration_seconds: Option<i64>,
}

#[derive(Debug)]
struct ParityFixture {
    id: &'static str,
    root: PathBuf,
}

impl ParityFixture {
    fn new(id: &'static str) -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/parity")
            .join(id);
        Self { id, root }
    }

    fn building_xml(&self) -> PathBuf {
        self.root.join("building.xml")
    }

    fn schedule_csv(&self) -> PathBuf {
        self.root.join("schedule.csv")
    }

    fn weather_epw(&self) -> PathBuf {
        self.root.join("weather.epw")
    }

    fn reference_output_parquet(&self) -> PathBuf {
        self.root.join("reference_output.parquet")
    }

    fn config_toml(&self) -> PathBuf {
        self.root.join("config.toml")
    }
}

#[test]
fn ochre_battery_fixture_time_axis_matches_reference_exactly() {
    let fixture = ParityFixture::new("cz4a_battery_only");
    let reference = read_parquet_columns(&fixture.reference_output_parquet())
        .expect("reference parquet must be readable");

    let actual_time = simulate_fixture_timestamps(&fixture);
    let reference_time =
        first_matching_column(&reference, &["Time"]).expect("reference time column missing");
    assert_eq!(
        actual_time.len(),
        reference_time.len(),
        "actual and reference time axes must have identical row counts"
    );
    for (idx, (actual_value, reference_value)) in actual_time
        .iter()
        .zip(reference_time.iter())
        .enumerate()
    {
        assert_eq!(
            *actual_value as i64, *reference_value as i64,
            "time axis must match the OCHRE reference exactly at row {idx}"
        );
    }
}

#[test]
fn ochre_ashp_fixture_peak_hvac_power_aligns() {
    let fixture = ParityFixture::new("cz4a_ashp_hpwh");
    let actual = run_fixture_to_columns(&fixture);
    let reference = read_parquet_columns(&fixture.reference_output_parquet())
        .expect("reference parquet must be readable");

    let actual_peak = peak_hvac_power(&actual).expect("actual HVAC peak power missing");
    let reference_peak = peak_hvac_power(&reference).expect("reference HVAC peak power missing");
    let peak_rel_pct = relative_percent_deviation(actual_peak, reference_peak);
    assert!(
        peak_rel_pct <= PEAK_HVAC_POWER_REL_PCT_MAX,
        "ASHP fixture HVAC peak power must stay within the OCHRE parity tolerance: actual={peak_rel_pct:.6}%, allowed={PEAK_HVAC_POWER_REL_PCT_MAX:.6}%"
    );
}

#[test]
fn energyplus_bestest_core_cases_keep_fixture_and_reference_band_coverage() {
    for case in bestest_cases::core_cases() {
        let fixture_path = case.fixture_path();
        assert!(
            fixture_path.exists(),
            "BESTEST fixture must exist for case {} at {}",
            case.id,
            fixture_path.display()
        );

        let bands = bestest_reference_bands::core_reference_bands(case.id);
        assert!(
            !bands.is_empty(),
            "BESTEST case {} must retain at least one EnergyPlus/ASHRAE reference band",
            case.id
        );

        for band in bands {
            assert!(
                band.min.is_finite() && band.max.is_finite(),
                "BESTEST reference band must be finite for case {} metric {}",
                case.id,
                band.metric
            );
            assert!(
                band.min < band.max,
                "BESTEST reference band must have min < max for case {} metric {}",
                case.id,
                band.metric
            );
        }
    }
}

fn run_fixture_to_columns(fixture: &ParityFixture) -> BTreeMap<String, Vec<f64>> {
    let mut dwelling_config = build_dwelling_config(fixture);
    let mut sim_config = dwelling_config.sim_config.clone();
    let output_path = unique_temp_path(fixture.id, "parquet");
    sim_config.output_format = OutputFormat::Parquet;
    sim_config.output_path = Some(output_path.clone());
    dwelling_config.sim_config = sim_config;

    let engine = SimulationEngine::new();
    let outcome = engine
        .run(dwelling_config)
        .expect("parity fixture simulation must succeed");
    assert!(
        !matches!(outcome.status, SimStatus::Failed(_)),
        "fixture {} must not fail simulation: {:?}",
        fixture.id,
        outcome.status
    );

    let path = outcome
        .timeseries_path
        .as_deref()
        .unwrap_or(output_path.as_path());
    let columns = read_parquet_columns(path).expect("actual parquet output must be readable");

    let _ = fs::remove_file(output_path);
    columns
}

fn simulate_fixture_timestamps(fixture: &ParityFixture) -> Vec<f64> {
    let mut dwelling =
        Dwelling::from_config(build_dwelling_config(fixture)).expect("fixture dwelling must load");
    let results = dwelling
        .simulate()
        .expect("fixture simulation must succeed")
        .steps;
    results
        .into_iter()
        .map(|step| step.timestamp.timestamp_micros() as f64)
        .collect()
}

fn build_dwelling_config(fixture: &ParityFixture) -> DwellingConfig {
    let config_contents =
        fs::read_to_string(fixture.config_toml()).expect("fixture config.toml must be readable");
    let config = parse_fixture_config(&config_contents).expect("fixture config.toml must parse");
    let sim_config =
        parse_simulation_config(&config_contents, &config).expect("simulation config must parse");
    let defaults_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../defaults");

    DwellingConfig {
        hpxml_path: fixture.building_xml(),
        schedule_path: fixture.schedule_csv(),
        weather_path: fixture.weather_epw(),
        sim_config,
        defaults_path: Some(defaults_path),
        overrides: None,
        bldg_id: config.bldg_id.unwrap_or(1),
        initialization_duration: config
            .initialization_duration_seconds
            .and_then(|seconds| u64::try_from(seconds).ok())
            .map(StdDuration::from_secs),
        resample_overrides: None,
    }
}

fn parse_fixture_config(contents: &str) -> Result<FixtureConfig, toml::de::Error> {
    toml::from_str(contents)
}

fn parse_simulation_config(
    contents: &str,
    config: &FixtureConfig,
) -> Result<SimulationConfig, String> {
    if let Some(sim_table) = &config.simulation {
        let sim_toml =
            toml::to_string(sim_table).expect("fixture [simulation] table must serialize");
        return SimulationConfig::from_toml(&sim_toml)
            .map_err(|err| format!("simulation config invalid: {err}"));
    }

    SimulationConfig::from_toml(contents).map_err(|err| format!("simulation config invalid: {err}"))
}

fn read_parquet_columns(path: &Path) -> Result<BTreeMap<String, Vec<f64>>, String> {
    let file =
        fs::File::open(path).map_err(|err| format!("unable to open '{}': {err}", path.display()))?;
    let mut reader = ParquetRecordBatchReaderBuilder::try_new(file)
        .map_err(|err| format!("unable to build parquet reader '{}': {err}", path.display()))?
        .build()
        .map_err(|err| format!("unable to read parquet '{}': {err}", path.display()))?;

    let mut columns: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for next_batch in &mut reader {
        let batch = next_batch
            .map_err(|err| format!("unable to consume parquet '{}': {err}", path.display()))?;
        merge_numeric_columns(&mut columns, &batch);
        merge_named_timestamp_columns(&mut columns, &batch);
    }

    Ok(columns)
}

fn merge_numeric_columns(columns: &mut BTreeMap<String, Vec<f64>>, batch: &RecordBatch) {
    let schema = batch.schema();
    for (idx, field) in schema.fields().iter().enumerate() {
        if let Some(float_array) = batch.column(idx).as_any().downcast_ref::<Float64Array>() {
            let values = columns.entry(field.name().to_string()).or_default();
            for row in 0..float_array.len() {
                if float_array.is_valid(row) {
                    values.push(float_array.value(row));
                }
            }
        } else if let Some(timestamp_array) = batch
            .column(idx)
            .as_any()
            .downcast_ref::<TimestampMicrosecondArray>()
        {
            let values = columns.entry(field.name().to_string()).or_default();
            for row in 0..timestamp_array.len() {
                if timestamp_array.is_valid(row) {
                    values.push(timestamp_array.value(row) as f64);
                }
            }
        }
    }
}

fn merge_named_timestamp_columns(columns: &mut BTreeMap<String, Vec<f64>>, batch: &RecordBatch) {
    let schema = batch.schema();
    let Some(name_idx) = schema.fields().iter().position(|field| field.name() == "Name") else {
        return;
    };
    let Some(value_idx) = schema.fields().iter().position(|field| field.name() == "Value") else {
        return;
    };
    let Some(name_array) = batch.column(name_idx).as_any().downcast_ref::<StringArray>() else {
        return;
    };
    let Some(value_array) = batch
        .column(value_idx)
        .as_any()
        .downcast_ref::<TimestampMicrosecondArray>()
    else {
        return;
    };

    for row in 0..batch.num_rows() {
        if !name_array.is_valid(row) || !value_array.is_valid(row) {
            continue;
        }
        let name = name_array.value(row);
        if name == "Time" {
            continue;
        }

        // Convert microseconds since epoch to a numeric series so timestamp-like
        // columns can still participate in schema existence checks if needed.
        columns
            .entry(name.to_string())
            .or_default()
            .push(value_array.value(row) as f64);
    }
}

fn first_matching_column<'a>(
    columns: &'a BTreeMap<String, Vec<f64>>,
    exact_names: &[&str],
) -> Option<&'a [f64]> {
    for name in exact_names {
        if let Some(series) = columns.get(*name)
            && !series.is_empty()
        {
            return Some(series.as_slice());
        }
    }
    None
}

fn peak_hvac_power(columns: &BTreeMap<String, Vec<f64>>) -> Option<f64> {
    let mut peak = None::<f64>;

    for (name, series) in columns {
        let lowered = name.to_ascii_lowercase();
        if !lowered.ends_with("electric power (kw)") {
            continue;
        }
        if ![
            "hvac",
            "air conditioner",
            "heat pump",
            "furnace",
            "ashp",
            "mshp",
            "baseboard",
        ]
        .iter()
        .any(|needle| lowered.contains(needle))
        {
            continue;
        }

        for value in series {
            peak = Some(peak.map_or(*value, |curr| curr.max(*value)));
        }
    }

    peak
}

fn relative_percent_deviation(actual: f64, reference: f64) -> f64 {
    let denom = reference.abs();
    if denom <= f64::EPSILON {
        if actual.abs() <= f64::EPSILON {
            0.0
        } else {
            f64::INFINITY
        }
    } else {
        ((actual - reference).abs() / denom) * 100.0
    }
}

fn unique_temp_path(fixture_id: &str, extension: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_nanos();
    path.push(format!(
        "hares-alignment-{fixture_id}-{nanos}.{extension}"
    ));
    path
}
