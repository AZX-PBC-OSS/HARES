//! The billing month boundary: each step's energy bills to the billing
//! period whose span holds it.
//!
//! The monthly cycle's periods are anniversary-based: the run's first
//! period opens at the start and closes at the start's day of month in
//! the following month, midnight in the tariff's zone, and every later
//! close keeps that anchor day, clamped to the month's length. The
//! dwelling hands the tariff evaluator the step's end time, so the
//! evaluator folds the step in and closes its period when that time has
//! reached the period end: a step spanning the boundary books to the
//! period it opens, never to the one closing.
//!
//! Fixture: the HPXML, schedule and weather of
//! `tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh/`, start
//! `2023-01-25T00:00:00-07:00`, 35 days at 3600 s, with the flat tariff
//! `tests/fixtures/golden/flat_tariff.json` (one all-hours period, one
//! 0.15 USD/kWh energy rate, a 10.00 USD monthly fixed charge, monthly
//! billing cycle) in zone `America/Denver`. The first anniversary
//! boundary is February 25 midnight, so the run spans it: 744 steps label
//! January 25 00:00 through February 24 23:00, and 96 steps label
//! February 25 00:00 through February 28 23:00.

use hares_core::DwellingSettings;
use hares_io::{DefaultsSource, InputSource, WeatherInput};
use std::path::PathBuf;

use chrono::{DateTime, Duration, FixedOffset, TimeZone};
use hares_core::{Dwelling, DwellingConfig};
use hares_io::{SimulationConfig, SiteLocationOverride};
use hares_tariff::ElectricTariff;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

const FLAT_TARIFF: &str = include_str!("../../../tests/fixtures/golden/flat_tariff.json");

/// The run's start instant, in the fixed offset the fixture's manifest
/// starts it at.
fn start() -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339("2023-01-25T00:00:00-07:00").expect("the start time parses")
}

fn dwelling() -> Dwelling {
    let root = project_root();
    let home = root.join("tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh");
    let config = SimulationConfig {
        scenario_seed: None,
        schedule_diversity: Default::default(),
        start_time: start(),
        duration: Duration::days(35),
        time_res: Duration::seconds(3600),
        reports: Vec::new(),
        diagnostics: false,
        metrics: false,
        output_chunk_size: 10_000,
        setpoint_deadband_c: None,
        master_seed: 0,
        civil_timezone: None,
        site_location: SiteLocationOverride::default(),
        max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
        health_policy: hares_io::HealthPolicy::default(),
        first_hour_interpolation: Default::default(),
        weather_year: hares_io::WeatherYear::default(),
        warmup: hares_io::Warmup::Disabled,
    };
    config.validate().expect("config validates");

    let mut dwelling = Dwelling::from_config(DwellingConfig {
        hpxml: InputSource::Path(home.join("building.xml")),
        schedule: Some(InputSource::Path(home.join("schedule.csv"))),
        weather: WeatherInput::Path(home.join("weather.epw")),
        defaults: DefaultsSource::directory(root.join("python/hares/defaults")),
        settings: DwellingSettings::new(config, None, 1, None),
    })
    .expect("the cz2a dwelling builds");

    let tariff: ElectricTariff = serde_json::from_str(FLAT_TARIFF).expect("the flat tariff parses");
    tariff.validate().expect("the flat tariff validates");
    dwelling
        .set_tariff(tariff, chrono_tz::America::Denver)
        .expect("the tariff attaches");
    dwelling
}

/// The run spans the February 25 anniversary boundary exactly once: the
/// first period closes at the boundary holding its own 744 steps, the
/// second opens there and holds its own 96, and each summary's import
/// total is, on bits, the step-order sum of its own steps' import energy
/// (the positive part of the step's net electric power in kW times one
/// hour, computed as the evaluator's update computes it).
#[test]
fn dwelling_billing_boundary_step_belongs_to_its_own_month() {
    let denver = chrono_tz::America::Denver;
    let start = denver
        .with_ymd_and_hms(2023, 1, 25, 0, 0, 0)
        .single()
        .expect("start");
    let boundary = denver
        .with_ymd_and_hms(2023, 2, 25, 0, 0, 0)
        .single()
        .expect("boundary");
    let sim_end = denver
        .with_ymd_and_hms(2023, 3, 1, 0, 0, 0)
        .single()
        .expect("sim end");

    let mut dwelling = dwelling();
    let dt_secs = dwelling.latest_env().time_step_secs();
    let mut import_j: Vec<f64> = Vec::with_capacity(35 * 24);
    for _ in 0..35 * 24 {
        let result = dwelling.step().expect("the boundary run steps");
        import_j.push(result.net_electric_power_w.max(0.0) * dt_secs);
    }
    dwelling.finalize_billing();

    let summaries = dwelling.billing_summaries();
    assert_eq!(
        summaries.len(),
        2,
        "the run spans the anniversary boundary once: one close in-run and one finalize"
    );
    let first = &summaries[0];
    let second = &summaries[1];

    assert_eq!(
        first.period_start, start,
        "the first period opens at the run's start"
    );
    assert_eq!(
        first.period_end, boundary,
        "the first period closes on the start's day-of-month anniversary"
    );
    assert_eq!(
        second.period_start, boundary,
        "the second period opens at the boundary"
    );
    assert_eq!(
        second.period_end, sim_end,
        "the finalize closes the second period at the run's end"
    );

    // Membership is read from the totals: the summary records no per-step
    // membership, so the split is pinned through the sums. The evaluator
    // accumulates each period's import in joules and converts once at the
    // close, so the sums convert once too.
    let mut first_sum = 0.0f64;
    for energy in &import_j[..744] {
        first_sum += energy;
    }
    let mut second_sum = 0.0f64;
    for energy in &import_j[744..] {
        second_sum += energy;
    }
    let first_sum = uom::si::f64::Energy::new::<uom::si::energy::joule>(first_sum)
        .get::<uom::si::energy::kilowatt_hour>();
    let second_sum = uom::si::f64::Energy::new::<uom::si::energy::joule>(second_sum)
        .get::<uom::si::energy::kilowatt_hour>();
    assert_eq!(
        first.total_import_kwh.to_bits(),
        first_sum.to_bits(),
        "the first period's total is its own 744 steps' import, on bits"
    );
    assert_eq!(
        second.total_import_kwh.to_bits(),
        second_sum.to_bits(),
        "the second period's total is its own 96 steps' import, on bits"
    );

    // The exact proration: the first period closed at its own boundary
    // bills the full monthly charge, and the finalize bills the open
    // period's full monthly charge plus the daily charge for the days
    // elapsed (the flat tariff's daily charge is zero).
    assert_eq!(
        first.fixed_charge_usd.to_bits(),
        10.0f64.to_bits(),
        "the boundary close bills the full monthly fixed charge"
    );
    assert_eq!(
        second.fixed_charge_usd.to_bits(),
        10.0f64.to_bits(),
        "the finalize bills the full monthly fixed charge plus zero daily days"
    );
}
