mod corpus;
mod tolerance;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration as StdDuration;
use std::time::{SystemTime, UNIX_EPOCH};

use arrow::array::{Array, Float64Array};
use corpus::{DiscoveredFixture, ParityFixture, discover_fixtures};
use hares_core::{DwellingConfig, SimStatus, SimulationEngine};
use hares_io::{SimulationConfig, parse_hpxml, resolve_equipment};
use hares_io::{defaults::DefaultsStore, hpxml::ZoneType};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use serde::Deserialize;
use serde_json::json;
use tolerance::{
    ANNUAL_HVAC_ENERGY_REL_PCT_MAX, ANNUAL_TOTAL_SITE_ENERGY_REL_PCT_MAX,
    ANNUAL_WATER_HEATER_ENERGY_REL_PCT_MAX, BATTERY_SOC_MAE_ABS_MAX,
    EQUIPMENT_MODE_CYCLE_COUNT_REL_PCT_MAX, MetricCheck, PEAK_HVAC_POWER_REL_PCT_MAX,
    ZONE_TEMP_CONDITIONED_C_MAE_MAX, ZONE_TEMP_UNCONDITIONED_C_MAE_MAX, check_absolute_mae,
    check_relative_percent,
};

const METRIC_ZONE_TEMP_CONDITIONED: &str = "zone_temperature_conditioned_mae_c";
const METRIC_ZONE_TEMP_UNCONDITIONED: &str = "zone_temperature_unconditioned_mae_c";
const METRIC_ANNUAL_HVAC_ENERGY: &str = "annual_hvac_energy_relative_percent";
const METRIC_ANNUAL_WATER_HEATER_ENERGY: &str = "annual_water_heater_energy_relative_percent";
const METRIC_ANNUAL_TOTAL_SITE_ENERGY: &str = "annual_total_site_energy_relative_percent";
const METRIC_PEAK_HVAC_POWER: &str = "peak_hvac_power_relative_percent";
const METRIC_BATTERY_SOC: &str = "battery_soc_mae_absolute";
const METRIC_EQUIPMENT_MODE_CYCLES: &str = "equipment_mode_cycle_count_relative_percent";

#[derive(Debug, Deserialize, Default)]
struct FixtureConfig {
    #[serde(default)]
    simulation: Option<toml::Table>,
    #[serde(default)]
    bldg_id: Option<i64>,
    #[serde(default)]
    initialization_duration_seconds: Option<i64>,
    #[serde(default)]
    property_parity: Option<PropertyParityExpectations>,
}

#[derive(Debug, Deserialize, Default)]
struct PropertyParityExpectations {
    #[serde(default)]
    equipment_names: Vec<String>,
    #[serde(default)]
    equipment_count: Option<usize>,
    #[serde(default)]
    zone_count: Option<usize>,
    #[serde(default)]
    conditioned_zone_count: Option<usize>,
}

#[derive(Debug)]
struct FixtureRunResult {
    fixture_id: String,
    checks: Vec<MetricCheck>,
    skipped_metrics: Vec<&'static str>,
}

#[test]
fn parity_outputs_against_reference_corpus() {
    let discovered = discover_fixtures().expect("failed to discover parity fixtures");
    let mut complete = Vec::new();
    let mut incomplete = Vec::new();

    for fixture in discovered {
        match fixture {
            DiscoveredFixture::Complete(fixture) => complete.push(fixture),
            DiscoveredFixture::Incomplete(fixture) => incomplete.push(fixture),
        }
    }

    for missing in &incomplete {
        eprintln!(
            "[parity] skipping incomplete fixture {} (missing: {})",
            missing.id,
            missing.missing_files.join(", ")
        );
    }

    if complete.is_empty() {
        eprintln!("[parity] no complete fixtures found under parity corpus; nothing to validate");
        return;
    }

    let mut failures = Vec::new();
    for fixture in &complete {
        match run_and_compare_fixture(fixture) {
            Ok(result) => {
                eprintln!(
                    "[parity] fixture={} checks={}",
                    result.fixture_id,
                    result.checks.len()
                );
                for check in &result.checks {
                    eprintln!("  {}", check.summary_line());
                }
                if !result.skipped_metrics.is_empty() {
                    eprintln!(
                        "  SKIP {}",
                        result
                            .skipped_metrics
                            .iter()
                            .map(|m| m.to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }

                for check in result.checks {
                    if !check.passed {
                        failures.push(format!(
                            "fixture={} metric={} expected={} actual_deviation={:.6}",
                            result.fixture_id, check.metric, check.band, check.actual
                        ));
                    }
                }
            }
            Err(err) => failures.push(format!("fixture={} error={err}", fixture.id)),
        }
    }

    assert!(
        failures.is_empty(),
        "parity tolerance violations:\n{}",
        failures.join("\n")
    );
}

#[test]
fn parity_property_alignment_from_hpxml() -> Result<(), Box<dyn std::error::Error>> {
    let fixtures = discover_fixtures().expect("failed to discover parity fixtures");

    let mut failures = Vec::new();

    for fixture in fixtures {
        let Some((fixture_id, building_xml, config_toml)) = fixture_property_inputs(&fixture)
        else {
            continue;
        };

        let config_contents = match fs::read_to_string(&config_toml) {
            Ok(contents) => contents,
            Err(err) => {
                failures.push(format!(
                    "fixture={} could not read config '{}': {err}",
                    fixture_id,
                    config_toml.display()
                ));
                continue;
            }
        };

        let config = match parse_fixture_config(&config_contents) {
            Ok(cfg) => cfg,
            Err(err) => {
                failures.push(format!("fixture={} invalid config.toml: {err}", fixture_id));
                continue;
            }
        };

        let building = match parse_hpxml(&building_xml) {
            Ok(building) => building,
            Err(err) => {
                failures.push(format!("fixture={} HPXML parse failed: {err}", fixture_id));
                continue;
            }
        };
        let equipment = match resolve_equipment(&building, &DefaultsStore::empty(), &json!({})) {
            Ok(specs) => specs,
            Err(err) => {
                failures.push(format!(
                    "fixture={} equipment resolution failed: {err}",
                    fixture_id
                ));
                continue;
            }
        };
        let equipment_names: Vec<String> = equipment.iter().map(|spec| spec.name.clone()).collect();
        let conditioned = building
            .zones
            .iter()
            .filter(|zone| matches!(zone.zone_type, ZoneType::Conditioned))
            .count();

        let Some(expectations) = config.property_parity else {
            failures.push(format!(
                "fixture={} missing [property_parity]; suggested: equipment_count={}, zone_count={}, conditioned_zone_count={}",
                fixture_id,
                equipment_names.len(),
                building.zones.len(),
                conditioned
            ));
            continue;
        };

        if let Some(expected_count) = expectations.equipment_count
            && equipment_names.len() != expected_count
        {
            failures.push(format!(
                "fixture={} equipment count mismatch: expected={} actual={}",
                fixture_id,
                expected_count,
                equipment_names.len()
            ));
        }

        if !expectations.equipment_names.is_empty() {
            let actual: BTreeSet<String> = equipment_names.iter().cloned().collect();
            let expected: BTreeSet<String> = expectations.equipment_names.iter().cloned().collect();
            if actual != expected {
                failures.push(format!(
                    "fixture={} equipment set mismatch: expected={:?} actual={:?}",
                    fixture_id, expected, actual
                ));
            }
        }

        if let Some(expected_zone_count) = expectations.zone_count
            && building.zones.len() != expected_zone_count
        {
            failures.push(format!(
                "fixture={} zone count mismatch: expected={} actual={}",
                fixture_id,
                expected_zone_count,
                building.zones.len()
            ));
        }

        if let Some(expected_conditioned) = expectations.conditioned_zone_count {
            if conditioned != expected_conditioned {
                failures.push(format!(
                    "fixture={} conditioned zone count mismatch: expected={} actual={}",
                    fixture_id, expected_conditioned, conditioned
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "property parity violations:\n{}",
        failures.join("\n")
    );
    Ok(())
}

fn fixture_property_inputs(fixture: &DiscoveredFixture) -> Option<(String, PathBuf, PathBuf)> {
    match fixture {
        DiscoveredFixture::Complete(complete) => Some((
            complete.id.clone(),
            complete.building_xml.clone(),
            complete.config_toml.clone(),
        )),
        DiscoveredFixture::Incomplete(incomplete) => {
            let building_xml = incomplete.root.join("building.xml");
            let config_toml = incomplete.root.join("config.toml");
            if building_xml.exists() && config_toml.exists() {
                Some((incomplete.id.clone(), building_xml, config_toml))
            } else {
                None
            }
        }
    }
}

fn run_and_compare_fixture(fixture: &ParityFixture) -> Result<FixtureRunResult, String> {
    let config_contents = fs::read_to_string(&fixture.config_toml).map_err(|err| {
        format!(
            "failed to read fixture config '{}': {err}",
            fixture.config_toml.display()
        )
    })?;
    let config = parse_fixture_config(&config_contents)?;

    let mut sim_config = parse_simulation_config(&config_contents, &config)?;
    let output_path = unique_temp_path(&fixture.id, "parquet");
    sim_config.output_format = hares_io::OutputFormat::Parquet;
    sim_config.output_path = Some(output_path.clone());

    let defaults_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../defaults");
    let dwelling_config = DwellingConfig {
        hpxml_path: fixture.building_xml.clone(),
        schedule_path: fixture.schedule_csv.clone(),
        weather_path: fixture.weather_epw.clone(),
        sim_config,
        defaults_path: Some(defaults_path),
        overrides: None,
        bldg_id: config.bldg_id.unwrap_or(1),
        initialization_duration: config
            .initialization_duration_seconds
            .and_then(|seconds| u64::try_from(seconds).ok())
            .map(StdDuration::from_secs),
        resample_overrides: Some(hares_io::ResampleOverrides::ochre_compat()),
    };

    let engine = SimulationEngine::new();
    let outcome = engine
        .run(dwelling_config)
        .map_err(|err| format!("simulation failed: {err}"))?;

    eprintln!(
        "[parity] fixture={} status={:?} elapsed={:?} warnings={:?}",
        fixture.id, outcome.status, outcome.elapsed, outcome.warnings
    );

    if matches!(outcome.status, SimStatus::Failed(_)) {
        return Err(format!("simulation status failed: {:?}", outcome.status));
    }

    let actual_columns = read_parquet_columns(
        outcome
            .timeseries_path
            .as_deref()
            .unwrap_or(output_path.as_path()),
    )?;
    let reference_columns = read_parquet_columns(&fixture.reference_output_parquet)?;

    let checks = compare_metrics(&actual_columns, &reference_columns, &fixture.id);
    let skipped_metrics = expected_metrics()
        .into_iter()
        .filter(|metric| !checks.iter().any(|check| check.metric == *metric))
        .collect::<Vec<_>>();

    let _ = fs::remove_file(output_path);

    Ok(FixtureRunResult {
        fixture_id: fixture.id.clone(),
        checks,
        skipped_metrics,
    })
}

fn parse_fixture_config(contents: &str) -> Result<FixtureConfig, String> {
    toml::from_str::<FixtureConfig>(contents).map_err(|err| format!("TOML parse error: {err}"))
}

fn parse_simulation_config(
    contents: &str,
    config: &FixtureConfig,
) -> Result<SimulationConfig, String> {
    if let Some(sim_table) = &config.simulation {
        let sim_toml = toml::to_string(sim_table)
            .map_err(|err| format!("failed to serialize [simulation] table: {err}"))?;
        return SimulationConfig::from_toml(&sim_toml)
            .map_err(|err| format!("simulation config invalid: {err}"));
    }

    SimulationConfig::from_toml(contents).map_err(|err| format!("simulation config invalid: {err}"))
}

fn read_parquet_columns(path: &Path) -> Result<BTreeMap<String, Vec<f64>>, String> {
    let file = fs::File::open(path)
        .map_err(|err| format!("unable to open parquet '{}': {err}", path.display()))?;
    let mut reader = ParquetRecordBatchReaderBuilder::try_new(file)
        .map_err(|err| format!("unable to build parquet reader '{}': {err}", path.display()))?
        .build()
        .map_err(|err| format!("unable to read parquet '{}': {err}", path.display()))?;

    let mut columns: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for next_batch in &mut reader {
        let batch = next_batch.map_err(|err| {
            format!(
                "unable to consume parquet batch '{}': {err}",
                path.display()
            )
        })?;

        let schema = batch.schema();
        for (idx, field) in schema.fields().iter().enumerate() {
            if field.name() == "Time" {
                continue;
            }

            let Some(float_array) = batch.column(idx).as_any().downcast_ref::<Float64Array>()
            else {
                continue;
            };

            let values = columns.entry(field.name().to_string()).or_default();
            for row in 0..float_array.len() {
                if float_array.is_valid(row) {
                    values.push(float_array.value(row));
                }
            }
        }
    }

    Ok(columns)
}

fn compare_metrics(
    actual: &BTreeMap<String, Vec<f64>>,
    reference: &BTreeMap<String, Vec<f64>>,
    fixture_id: &str,
) -> Vec<MetricCheck> {
    let mut checks = Vec::new();

    if let (Some(a), Some(r)) = (
        first_matching_column(actual, &["Temperature - Indoor (C)"]),
        first_matching_column(reference, &["Temperature - Indoor (C)"]),
    ) {
        checks.push(check_absolute_mae(
            METRIC_ZONE_TEMP_CONDITIONED,
            a,
            r,
            ZONE_TEMP_CONDITIONED_C_MAE_MAX,
        ));
    }

    let a_unconditioned = first_matching_column(
        actual,
        &[
            "Temperature - Attic (C)",
            "Temperature - Garage (C)",
            "Temperature - Basement (C)",
            "Temperature - Foundation (C)",
        ],
    );
    let r_unconditioned = first_matching_column(
        reference,
        &[
            "Temperature - Attic (C)",
            "Temperature - Garage (C)",
            "Temperature - Basement (C)",
            "Temperature - Foundation (C)",
        ],
    );
    if let (Some(a), Some(r)) = (a_unconditioned, r_unconditioned) {
        checks.push(check_absolute_mae(
            METRIC_ZONE_TEMP_UNCONDITIONED,
            a,
            r,
            ZONE_TEMP_UNCONDITIONED_C_MAE_MAX,
        ));
    }

    if let (Some(a_hvac), Some(r_hvac)) = (
        annual_energy_for_prefixes(
            actual,
            &[
                "hvac",
                "air conditioner",
                "heat pump",
                "furnace",
                "ashp",
                "mshp",
                "baseboard",
            ],
        ),
        annual_energy_for_prefixes(
            reference,
            &[
                "hvac",
                "air conditioner",
                "heat pump",
                "furnace",
                "ashp",
                "mshp",
                "baseboard",
            ],
        ),
    ) {
        checks.push(check_relative_percent(
            METRIC_ANNUAL_HVAC_ENERGY,
            a_hvac,
            r_hvac,
            ANNUAL_HVAC_ENERGY_REL_PCT_MAX,
        ));
    }

    if let (Some(a_wh), Some(r_wh)) = (
        annual_energy_for_prefixes(actual, &["water heater", "hpwh"]),
        annual_energy_for_prefixes(reference, &["water heater", "hpwh"]),
    ) {
        checks.push(check_relative_percent(
            METRIC_ANNUAL_WATER_HEATER_ENERGY,
            a_wh,
            r_wh,
            ANNUAL_WATER_HEATER_ENERGY_REL_PCT_MAX,
        ));
    }

    if let (Some(a_site), Some(r_site)) = (
        first_matching_column(actual, &["Total Electric Power (kW)"]).map(integrate_kw_series),
        first_matching_column(reference, &["Total Electric Power (kW)"]).map(integrate_kw_series),
    ) {
        checks.push(check_relative_percent(
            METRIC_ANNUAL_TOTAL_SITE_ENERGY,
            a_site,
            r_site,
            ANNUAL_TOTAL_SITE_ENERGY_REL_PCT_MAX,
        ));
    }

    if let (Some(a_peak), Some(r_peak)) = (peak_hvac_power(actual), peak_hvac_power(reference)) {
        checks.push(check_relative_percent(
            METRIC_PEAK_HVAC_POWER,
            a_peak,
            r_peak,
            PEAK_HVAC_POWER_REL_PCT_MAX,
        ));
    }

    if let (Some(a_soc), Some(r_soc)) = (
        first_prefixed_soc_series(actual),
        first_prefixed_soc_series(reference),
    ) {
        checks.push(check_absolute_mae(
            METRIC_BATTERY_SOC,
            a_soc,
            r_soc,
            BATTERY_SOC_MAE_ABS_MAX,
        ));
    }

    if let (Some(a_cycles), Some(r_cycles)) = (
        aggregate_mode_cycle_count(actual),
        aggregate_mode_cycle_count(reference),
    ) {
        checks.push(check_relative_percent(
            METRIC_EQUIPMENT_MODE_CYCLES,
            a_cycles as f64,
            r_cycles as f64,
            EQUIPMENT_MODE_CYCLE_COUNT_REL_PCT_MAX,
        ));
    }

    if checks.is_empty() {
        eprintln!(
            "[parity] fixture={} produced zero comparable metrics (schema mismatch or low verbosity)",
            fixture_id
        );
    }

    checks
}

fn expected_metrics() -> Vec<&'static str> {
    vec![
        METRIC_ZONE_TEMP_CONDITIONED,
        METRIC_ZONE_TEMP_UNCONDITIONED,
        METRIC_ANNUAL_HVAC_ENERGY,
        METRIC_ANNUAL_WATER_HEATER_ENERGY,
        METRIC_ANNUAL_TOTAL_SITE_ENERGY,
        METRIC_PEAK_HVAC_POWER,
        METRIC_BATTERY_SOC,
        METRIC_EQUIPMENT_MODE_CYCLES,
    ]
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

fn annual_energy_for_prefixes(
    columns: &BTreeMap<String, Vec<f64>>,
    prefixes: &[&str],
) -> Option<f64> {
    let mut total_kwh = 0.0;
    let mut found = false;

    for (name, series) in columns {
        let lowered = name.to_ascii_lowercase();
        let is_candidate = lowered.ends_with("electric power (kw)")
            || lowered.ends_with("gas power (therms/hour)");
        if !is_candidate {
            continue;
        }

        if prefixes.iter().any(|prefix| lowered.contains(prefix)) {
            total_kwh += integrate_kw_series(series);
            found = true;
        }
    }

    if found { Some(total_kwh) } else { None }
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

fn first_prefixed_soc_series(columns: &BTreeMap<String, Vec<f64>>) -> Option<&[f64]> {
    for (name, series) in columns {
        let lowered = name.to_ascii_lowercase();
        if lowered.contains("battery") && lowered.ends_with("soc (-)") && !series.is_empty() {
            return Some(series.as_slice());
        }
    }
    None
}

fn aggregate_mode_cycle_count(columns: &BTreeMap<String, Vec<f64>>) -> Option<u64> {
    let mut cycles = 0_u64;
    let mut found = false;

    for (name, series) in columns {
        if !name.to_ascii_lowercase().ends_with("mode (-)") || series.len() < 2 {
            continue;
        }
        found = true;
        cycles += count_mode_cycles(series);
    }

    if found { Some(cycles) } else { None }
}

fn count_mode_cycles(series: &[f64]) -> u64 {
    let mut cycles = 0_u64;
    for pair in series.windows(2) {
        if pair[0] == 0.0 && pair[1] != 0.0 {
            cycles += 1;
        }
    }
    cycles
}

fn integrate_kw_series(series: &[f64]) -> f64 {
    const MINUTE_STEP_HOURS: f64 = 1.0 / 60.0;
    series.iter().sum::<f64>() * MINUTE_STEP_HOURS
}

fn unique_temp_path(fixture_id: &str, extension: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_nanos();
    path.push(format!("hares-parity-{fixture_id}-{nanos}.{extension}"));
    path
}
