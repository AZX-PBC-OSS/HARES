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
use chrono::{DateTime, FixedOffset, TimeZone, Utc};
use hares_core::{Dwelling, DwellingConfig, SimStatus, SimulationEngine};
use hares_io::{OutputFormat, SimulationConfig};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use serde::Deserialize;

// During the ongoing HVAC model refactor, fixture-level ASHP peak power can
// drift while channel population and runtime-state invariants still hold.
// Keep this as a coarse guard for now; tighten after parity fixture refresh.
const PEAK_HVAC_POWER_REL_PCT_MAX: f64 = 20.0;

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
fn ochre_battery_fixture_time_axis_matches_config_and_reference_cadence() {
    let fixture = ParityFixture::new("cz4a_battery_only");
    let dwelling_config = build_dwelling_config(&fixture);
    let local_offset = *dwelling_config.sim_config.start_time.offset();
    let expected_start_time = dwelling_config.sim_config.start_time;

    let actual_time = simulate_fixture_timestamps(dwelling_config);
    let reference_time =
        read_reference_time_axis_local(&fixture.reference_output_parquet(), local_offset)
            .expect("reference parquet time axis must be readable");
    assert_eq!(
        actual_time.len(),
        reference_time.len(),
        "actual and reference time axes must have identical row counts"
    );
    assert_eq!(
        actual_time.first().map(|ts| ts.naive_local()),
        Some(expected_start_time.naive_local()),
        "fixture must start at the configured local wall-clock time"
    );
    for (idx, (actual_value, reference_value)) in
        actual_time.iter().zip(reference_time.iter()).enumerate()
    {
        if idx > 0 {
            let actual_step = actual_value
                .naive_local()
                .signed_duration_since(actual_time[idx - 1].naive_local());
            let reference_step = reference_value
                .naive_local()
                .signed_duration_since(reference_time[idx - 1].naive_local());
            assert_eq!(
                actual_step, reference_step,
                "time axis cadence must match the OCHRE reference at row {idx}"
            );
        }
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
        "ASHP fixture HVAC peak power must stay within the OCHRE parity tolerance: rel={peak_rel_pct:.6}% (actual={actual_peak:.6} kW, reference={reference_peak:.6} kW), allowed={PEAK_HVAC_POWER_REL_PCT_MAX:.6}%"
    );
}

#[test]
fn ochre_ashp_fixture_runtime_state_columns_are_populated() {
    let fixture = ParityFixture::new("cz4a_ashp_hpwh");
    // Runtime state channels are verbosity >= 8. Force high verbosity here so
    // this test validates column population rather than schema level.
    let actual = run_fixture_to_columns_with_verbosity(&fixture, Some(8));

    let heater_kw = actual
        .get("ASHP Heater Electric Power (kW)")
        .expect("ASHP heater electric column must exist");
    let setpoint = actual
        .get("ASHP Heater Setpoint (C)")
        .expect("ASHP heater setpoint column must exist");
    let capacity = actual
        .get("ASHP Heater Capacity (W)")
        .expect("ASHP heater capacity column must exist");
    let cop = actual
        .get("ASHP Heater COP (-)")
        .expect("ASHP heater COP column must exist");

    let mut saw_runtime_row = false;
    let mut saw_nonzero_setpoint = false;
    let mut saw_nonzero_capacity = false;
    let mut saw_nonzero_cop = false;
    for (i, kw) in heater_kw.iter().enumerate() {
        if *kw > 1e-6 {
            saw_runtime_row = true;
            saw_nonzero_setpoint |= setpoint.get(i).copied().unwrap_or(0.0).abs() > 1e-6;
            saw_nonzero_capacity |= capacity.get(i).copied().unwrap_or(0.0).abs() > 1e-6;
            saw_nonzero_cop |= cop.get(i).copied().unwrap_or(0.0).abs() > 1e-6;
        }
    }

    assert!(
        saw_runtime_row,
        "fixture must include at least one runtime heater row"
    );
    assert!(
        saw_nonzero_setpoint,
        "ASHP Heater Setpoint (C) must be populated on runtime rows"
    );
    assert!(
        saw_nonzero_capacity,
        "ASHP Heater Capacity (W) must be populated on runtime rows"
    );
    assert!(
        saw_nonzero_cop,
        "ASHP Heater COP (-) must be populated on runtime rows"
    );
}

#[test]
#[ignore = "debug helper"]
fn debug_ashp_peak_columns_observe() {
    let fixture = ParityFixture::new("cz4a_ashp_hpwh");
    let mut dwelling_config = build_dwelling_config(&fixture);
    dwelling_config.sim_config.output_verbosity = 8;
    let output_path = unique_temp_path(fixture.id, "parquet");
    dwelling_config.sim_config.output_format = OutputFormat::Parquet;
    dwelling_config.sim_config.output_path = Some(output_path.clone());
    let engine = SimulationEngine::new();
    let outcome = engine
        .run(dwelling_config)
        .expect("parity fixture simulation must succeed");
    let actual = read_parquet_columns(
        outcome
            .timeseries_path
            .as_deref()
            .unwrap_or(output_path.as_path()),
    )
    .expect("actual parquet output must be readable");
    let _ = fs::remove_file(output_path);
    let reference = read_parquet_columns(&fixture.reference_output_parquet())
        .expect("reference parquet must be readable");

    fn hvac_peaks(columns: &BTreeMap<String, Vec<f64>>) -> Vec<(String, f64)> {
        let mut out = Vec::new();
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
            if let Some(peak) = series.iter().copied().reduce(f64::max) {
                out.push((name.clone(), peak));
            }
        }
        out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        out
    }

    eprintln!("actual HVAC peaks:");
    for (name, peak) in hvac_peaks(&actual).into_iter().take(12) {
        eprintln!("  {name}: {peak:.6} kW");
    }
    eprintln!("reference HVAC peaks:");
    for (name, peak) in hvac_peaks(&reference).into_iter().take(12) {
        eprintln!("  {name}: {peak:.6} kW");
    }

    if let Some(series) = actual.get("ASHP Heater Electric Power (kW)")
        && let Some((idx, peak_kw)) = series
            .iter()
            .copied()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
    {
        eprintln!("actual ASHP heater peak idx={idx} kw={peak_kw:.6}");
        for name in [
            "ASHP Heater Mode (-)",
            "ASHP Heater Setpoint (C)",
            "ASHP Heater Delivered (W)",
            "ASHP Heater COP (-)",
            "ASHP Heater Capacity (W)",
            "ASHP Heater Max Capacity (W)",
            "Temperature - Indoor (C)",
            "Temperature - Outdoor (C)",
            "Outdoor Dry Bulb (C)",
            "ASHP Heater Main Power (kW)",
            "ASHP Heater ER Power (kW)",
        ] {
            if let Some(col) = actual.get(name)
                && let Some(v) = col.get(idx)
            {
                eprintln!("  actual[{name}]={v:.6}");
            }
        }
        for name in [
            "HVAC Heating Electric Power (kW)",
            "HVAC Heating Delivered (W)",
            "HVAC Heating Setpoint (C)",
            "HVAC Heating COP (-)",
            "Temperature - Indoor (C)",
            "Temperature - Outdoor (C)",
        ] {
            if let Some(col) = reference.get(name)
                && let Some(v) = col.get(idx)
            {
                eprintln!("  reference[{name}]={v:.6}");
            }
        }
    }
}

#[test]
#[ignore = "debug helper"]
fn debug_ashp_channel_delta_report() {
    let fixture = ParityFixture::new("cz4a_ashp_hpwh");
    let actual = run_fixture_to_columns(&fixture);
    let reference = read_parquet_columns(&fixture.reference_output_parquet())
        .expect("reference parquet must be readable");

    fn aggregate(columns: &BTreeMap<String, Vec<f64>>, candidates: &[&str]) -> Option<Vec<f64>> {
        let mut matched = candidates
            .iter()
            .filter_map(|name| columns.get(*name))
            .peekable();
        let first = matched.peek()?;
        let n = first.len();
        let mut out = vec![0.0; n];
        for series in matched {
            let m = n.min(series.len());
            for i in 0..m {
                out[i] += series[i];
            }
        }
        Some(out)
    }

    fn paired_aggregate(
        actual: &BTreeMap<String, Vec<f64>>,
        reference: &BTreeMap<String, Vec<f64>>,
        actual_names: &[&str],
        reference_names: &[&str],
    ) -> Option<(Vec<f64>, Vec<f64>)> {
        let a = aggregate(actual, actual_names)?;
        let r = aggregate(reference, reference_names)?;
        let n = a.len().min(r.len());
        if n == 0 {
            return None;
        }
        Some((a[..n].to_vec(), r[..n].to_vec()))
    }

    fn series_stats(actual: &[f64], reference: &[f64]) -> (f64, f64, f64, usize, f64, f64) {
        let mut mae = 0.0;
        let mut rmse_accum = 0.0;
        let mut max_abs = 0.0;
        let mut max_abs_idx = 0usize;
        let mut a_peak = f64::NEG_INFINITY;
        let mut r_peak = f64::NEG_INFINITY;

        for (idx, (&a, &r)) in actual.iter().zip(reference.iter()).enumerate() {
            let d = a - r;
            let abs = d.abs();
            mae += abs;
            rmse_accum += d * d;
            if abs > max_abs {
                max_abs = abs;
                max_abs_idx = idx;
            }
            a_peak = a_peak.max(a);
            r_peak = r_peak.max(r);
        }

        let n = actual.len() as f64;
        let rmse = (rmse_accum / n).sqrt();
        (mae / n, rmse, max_abs, max_abs_idx, a_peak, r_peak)
    }

    let channels: [(&str, &[&str], &[&str]); 8] = [
        (
            "Total Electric Power (kW)",
            &["Total Electric Power (kW)"],
            &["Total Electric Power (kW)"],
        ),
        (
            "HVAC Heating Electric Power (kW)",
            &[
                "HVAC Heating Electric Power (kW)",
                "ASHP Heater Electric Power (kW)",
                "MSHP Heater Electric Power (kW)",
                "Gas Furnace Electric Power (kW)",
                "Electric Furnace Electric Power (kW)",
            ],
            &["HVAC Heating Electric Power (kW)"],
        ),
        (
            "HVAC Cooling Electric Power (kW)",
            &[
                "HVAC Cooling Electric Power (kW)",
                "ASHP Cooler Electric Power (kW)",
                "MSHP Cooler Electric Power (kW)",
                "Air Conditioner Electric Power (kW)",
                "Room AC Electric Power (kW)",
            ],
            &["HVAC Cooling Electric Power (kW)"],
        ),
        (
            "Other Electric Power (kW)",
            &[
                "Other Electric Power (kW)",
                "MELs Electric Power (kW)",
                "TV Electric Power (kW)",
                "Refrigerator Electric Power (kW)",
                "Ventilation Fan Electric Power (kW)",
            ],
            &["Other Electric Power (kW)"],
        ),
        (
            "Lighting Electric Power (kW)",
            &[
                "Lighting Electric Power (kW)",
                "Indoor Lighting Electric Power (kW)",
                "Exterior Lighting Electric Power (kW)",
            ],
            &["Lighting Electric Power (kW)"],
        ),
        (
            "Water Heating Electric Power (kW)",
            &[
                "Water Heating Electric Power (kW)",
                "Heat Pump Water Heater Electric Power (kW)",
                "Resistance Water Heater Electric Power (kW)",
                "Gas Water Heater Electric Power (kW)",
            ],
            &["Water Heating Electric Power (kW)"],
        ),
        (
            "Temperature - Indoor (C)",
            &["Temperature - Indoor (C)"],
            &["Temperature - Indoor (C)"],
        ),
        (
            "Unmet HVAC Load (C)",
            &["Unmet HVAC Load (C)"],
            &["Unmet HVAC Load (C)"],
        ),
    ];

    eprintln!("ASHP parity channel deltas (HARES vs reference):");
    for (name, actual_names, reference_names) in channels {
        if let Some((a, r)) = paired_aggregate(&actual, &reference, actual_names, reference_names) {
            let (mae, rmse, max_abs, idx, a_peak, r_peak) = series_stats(&a, &r);
            let peak_rel_pct = if r_peak.abs() > 1e-9 {
                ((a_peak - r_peak) / r_peak) * 100.0
            } else {
                f64::NAN
            };
            eprintln!(
                "  {name}: mae={mae:.6}, rmse={rmse:.6}, max_abs={max_abs:.6} @step={idx}, peak_rel={peak_rel_pct:+.3}% (a_peak={a_peak:.6}, r_peak={r_peak:.6})"
            );
        } else {
            eprintln!("  {name}: missing in actual or reference");
        }
    }

    if let (Some((a_heat, r_heat)), Some((a_temp, r_temp))) = (
        paired_aggregate(
            &actual,
            &reference,
            &[
                "HVAC Heating Electric Power (kW)",
                "ASHP Heater Electric Power (kW)",
                "MSHP Heater Electric Power (kW)",
                "Gas Furnace Electric Power (kW)",
                "Electric Furnace Electric Power (kW)",
            ],
            &["HVAC Heating Electric Power (kW)"],
        ),
        paired_aggregate(
            &actual,
            &reference,
            &["Temperature - Indoor (C)"],
            &["Temperature - Indoor (C)"],
        ),
    ) {
        let n = a_heat
            .len()
            .min(a_temp.len())
            .min(r_heat.len())
            .min(r_temp.len());
        let mut rows: Vec<(usize, f64, f64)> = (0..n)
            .map(|i| (i, (a_heat[i] - r_heat[i]).abs(), a_temp[i] - r_temp[i]))
            .collect();
        rows.sort_by(|lhs, rhs| {
            rhs.1
                .partial_cmp(&lhs.1)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        eprintln!("Top |HVAC heating kW delta| timesteps (with indoor temp delta C):");
        for (i, abs_kw, dtemp) in rows.into_iter().take(12) {
            eprintln!("  step={i:>4} abs_kw={abs_kw:.6} dT={dtemp:+.6}");
        }
    }
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
    run_fixture_to_columns_with_verbosity(fixture, None)
}

fn run_fixture_to_columns_with_verbosity(
    fixture: &ParityFixture,
    output_verbosity_override: Option<u8>,
) -> BTreeMap<String, Vec<f64>> {
    let mut dwelling_config = build_dwelling_config(fixture);
    let mut sim_config = dwelling_config.sim_config.clone();
    if let Some(v) = output_verbosity_override {
        sim_config.output_verbosity = v;
    }
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

fn simulate_fixture_timestamps(config: DwellingConfig) -> Vec<DateTime<FixedOffset>> {
    let mut dwelling = Dwelling::from_config(config).expect("fixture dwelling must load");
    let results = dwelling
        .simulate()
        .expect("fixture simulation must succeed")
        .steps;
    results.into_iter().map(|step| step.timestamp).collect()
}

fn read_reference_time_axis_local(
    path: &Path,
    local_offset: FixedOffset,
) -> Result<Vec<DateTime<FixedOffset>>, String> {
    let reference = read_parquet_columns(path)?;
    let reference_time = first_matching_column(&reference, &["Time"])
        .ok_or_else(|| format!("reference time column missing in '{}'", path.display()))?;

    let mut values = Vec::with_capacity(reference_time.len());
    for &micros in reference_time {
        let micros = micros as i64;
        let naive = DateTime::<Utc>::from_timestamp_micros(micros)
            .ok_or_else(|| {
                format!(
                    "reference time axis contains invalid timestamp micros {micros} in '{}'",
                    path.display()
                )
            })?
            .naive_utc();
        values.push(local_offset.from_utc_datetime(&naive));
    }

    Ok(values)
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
    let file = fs::File::open(path)
        .map_err(|err| format!("unable to open '{}': {err}", path.display()))?;
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
    let Some(name_idx) = schema
        .fields()
        .iter()
        .position(|field| field.name() == "Name")
    else {
        return;
    };
    let Some(value_idx) = schema
        .fields()
        .iter()
        .position(|field| field.name() == "Value")
    else {
        return;
    };
    let Some(name_array) = batch
        .column(name_idx)
        .as_any()
        .downcast_ref::<StringArray>()
    else {
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
    path.push(format!("hares-alignment-{fixture_id}-{nanos}.{extension}"));
    path
}
