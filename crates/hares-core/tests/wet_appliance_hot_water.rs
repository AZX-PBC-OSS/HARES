//! Wet-appliance hot water drawn through the water heater's scheduled draw:
//! the realised draw is the OpenStudio-HPXML daily volume at every step, so
//! it does not depend on the step length, and a vacant home draws nothing.

use hares_core::DwellingSettings;
use hares_io::{DefaultsSource, InputSource, WeatherInput};
use std::path::PathBuf;

use chrono::{DateTime, Duration, FixedOffset, TimeZone};
use hares_core::{Dwelling, DwellingConfig};
use hares_io::{SimulationConfig, SiteLocationOverride};
use hares_types::telemetry_keys as tk;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn sim_config(
    start_time: DateTime<FixedOffset>,
    duration: Duration,
    step_s: i64,
) -> SimulationConfig {
    SimulationConfig {
        scenario_seed: None,
        start_time,
        duration,
        time_res: Duration::seconds(step_s),
        reports: Vec::new(),
        diagnostics: false,
        metrics: false,
        output_chunk_size: 1024,
        setpoint_deadband_c: None,
        master_seed: 0,
        civil_timezone: None,
        site_location: SiteLocationOverride::default(),
        max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
        health_policy: hares_io::HealthPolicy::default(),
        first_hour_interpolation: Default::default(),
        weather_year: hares_io::WeatherYear::default(),
        warmup: hares_io::Warmup::Disabled,
    }
}

fn cz4a_config(
    start_time: DateTime<FixedOffset>,
    duration: Duration,
    step_s: i64,
) -> DwellingConfig {
    let fixture = project_root().join("tests/fixtures/parity/cz4a_ashp_hpwh");
    DwellingConfig {
        hpxml: InputSource::Path(fixture.join("building.xml")),
        schedule: Some(InputSource::Path(fixture.join("schedule.csv"))),
        weather: WeatherInput::Path(fixture.join("weather.epw")),
        defaults: DefaultsSource::directory(project_root().join("python/hares/defaults")),
        settings: DwellingSettings::new(sim_config(start_time, duration, step_s), None, 1, None),
    }
}

/// The water heater's per-step draw [kg/s] from its telemetry.
fn wh_draw_kg_s(dwelling: &Dwelling, wh_name: &str) -> Option<f64> {
    dwelling
        .latest_env()
        .equipment_telemetry
        .get(wh_name)
        .and_then(|t| t.get(tk::DRAW_FLOW_RATE_KG_S))
}

#[test]
fn wet_appliance_hot_water_is_independent_of_the_step() {
    use hares_physics::water_density_kg_m3;

    let start = FixedOffset::east_opt(-7 * 3600)
        .expect("offset")
        .with_ymd_and_hms(2023, 1, 1, 0, 0, 0)
        .single()
        .expect("start");
    let week = Duration::seconds(86_400 * 7);

    // The resolved daily volumes and the water heater's hot supply
    // temperature, read off the same inputs the run builds from.
    let defaults = hares_io::DefaultsStore::load(&project_root().join("python/hares/defaults"))
        .expect("defaults load");
    let building = hares_io::parse_hpxml(
        &project_root().join("tests/fixtures/parity/cz4a_ashp_hpwh/building.xml"),
    )
    .expect("cz4a building parses");
    let specs = hares_io::hpxml::equipment::resolve_equipment(
        &building,
        building.climate_zone_iecc.as_deref(),
        None,
        &defaults,
        None,
        &mut Vec::new(),
    )
    .expect("cz4a equipment resolves");
    let spec_f64 = |name: &str, key: &str| {
        specs
            .iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("no {name} spec"))
            .parameters
            .get(key)
            .and_then(serde_json::Value::as_f64)
            .unwrap_or_else(|| panic!("{name} has no {key}"))
    };
    let washer_daily_m3 = spec_f64("Clothes Washer", "daily_hot_water_m3");
    let dw_daily_m3 = spec_f64("Dishwasher", "daily_hot_water_m3");
    let setpoint_c = spec_f64("Heat Pump Water Heater", "setpoint_c");

    // The run window's share of each schedule column: the normalization is
    // the annual mean, so the window's drawn volume is the daily volume
    // weighted by the column's daily sums over the window against the
    // annual mean daily sum. The fixture schedule is hourly.
    let schedule = std::fs::read_to_string(
        project_root().join("tests/fixtures/parity/cz4a_ashp_hpwh/schedule.csv"),
    )
    .expect("schedule.csv readable");
    let mut rows = schedule.lines();
    let header: Vec<String> = rows
        .next()
        .expect("header")
        .split(',')
        .map(|c| c.trim().to_string())
        .collect();
    let col = |name: &str| header.iter().position(|c| c == name).expect("column");
    let mut hwcw_sums = vec![0.0_f64; 7];
    let mut hwdw_sums = vec![0.0_f64; 7];
    let mut annual_hwcw = 0.0_f64;
    let mut annual_hwdw = 0.0_f64;
    for (i, line) in rows.enumerate() {
        let value = |c: usize| {
            line.split(',')
                .nth(c)
                .and_then(|v| v.trim().parse::<f64>().ok())
                .expect("schedule value")
        };
        let hwcw = value(col("hot_water_clothes_washer"));
        let hwdw = value(col("hot_water_dishwasher"));
        annual_hwcw += hwcw;
        annual_hwdw += hwdw;
        if i < 168 {
            hwcw_sums[i / 24] += hwcw;
            hwdw_sums[i / 24] += hwdw;
        }
    }
    let weight = |sums: &[f64], annual: f64| {
        let mean_daily = annual / 365.0;
        sums.iter().sum::<f64>() / mean_daily
    };
    let rho_kg_per_l = water_density_kg_m3(setpoint_c) / 1000.0;
    let washer_mass = washer_daily_m3 * 1000.0 * rho_kg_per_l * weight(&hwcw_sums, annual_hwcw);
    let dw_mass = dw_daily_m3 * 1000.0 * rho_kg_per_l * weight(&hwdw_sums, annual_hwdw);
    // The appliance part of the water heater's scheduled draw. (The heat
    // pump water heater's tempered fixture draw is not wired to the schedule
    // today, a separate gap this row does not touch, so the telemetry draw
    // here is the appliance part alone.)
    let expected_mass = washer_mass + dw_mass;

    // The runs: identical daily water mass at 60 s, 900 s and 3600 s.
    let mut masses = Vec::new();
    for step_s in [60_i64, 900, 3600] {
        let mut dwelling =
            Dwelling::from_config(cz4a_config(start, week, step_s)).expect("cz4a dwelling builds");
        let mut mass = 0.0_f64;
        let steps = (7 * 86_400 / step_s) as usize;
        for _ in 0..steps {
            dwelling.step().expect("step");
            let draw = wh_draw_kg_s(&dwelling, "Heat Pump Water Heater")
                .expect("the water heater reports its draw");
            mass += draw * step_s as f64;
        }
        masses.push(mass);
    }
    for (i, mass) in masses.iter().enumerate() {
        let reference = masses[masses.len() - 1];
        let relative = (mass - reference).abs() / reference.max(1e-30);
        assert!(
            relative < 1e-9,
            "the appliance draw at step {} ({mass}) must equal the 3600 s draw \
             ({reference}); relative {relative}",
            [60, 900, 3600][i],
        );
    }
    let relative_to_expected = (masses[2] - expected_mass).abs() / expected_mass;
    assert!(
        relative_to_expected < 1e-9,
        "the 7-day draw {} must equal the daily volumes weighted by the \
         columns' daily sums ({expected_mass}); relative {relative_to_expected}",
        masses[2]
    );
}

#[test]
fn vacant_home_draws_no_appliance_hot_water() {
    // The vacant fixture: an all-year Vacancy period and no schedule file,
    // so the generated schedule carries the hot-water columns and the
    // periods zero them.
    let start = FixedOffset::east_opt(-5 * 3600)
        .expect("offset")
        .with_ymd_and_hms(2018, 1, 1, 0, 0, 0)
        .single()
        .expect("start");
    let config = DwellingConfig {
        hpxml: InputSource::Path(
            project_root().join("tests/fixtures/resstock/2025.1/bldg0527060/home.xml"),
        ),
        schedule: None,
        weather: WeatherInput::Path(
            project_root().join("tests/fixtures/resstock/2025.1/weather/G5300330_2018.csv"),
        ),
        defaults: DefaultsSource::directory(project_root().join("python/hares/defaults")),
        settings: DwellingSettings::new(
            sim_config(start, Duration::seconds(86_400 * 365), 3600),
            None,
            527060,
            None,
        ),
    };
    let mut dwelling = Dwelling::from_config(config).expect("vacant dwelling builds");
    let steps = 365 * 24;
    let mut nonzero_draw_steps = 0_usize;
    for _ in 0..steps {
        dwelling.step().expect("step");
        if wh_draw_kg_s(&dwelling, "Electric Resistance Water Heater")
            .is_some_and(|draw| draw > 0.0)
        {
            nonzero_draw_steps += 1;
        }
    }
    assert_eq!(
        nonzero_draw_steps, 0,
        "a vacant home draws no hot water on any step"
    );
}
