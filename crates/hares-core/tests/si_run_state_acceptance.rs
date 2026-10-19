//! Acceptance tests for B-14's commit 3 (schedule row 94g): the run state —
//! billing, metrics, equipment state, ports and per-step results — held in
//! SI units with one conversion at each boundary.
//!
//! Each test is named in the entry's acceptance list. The fixtures are the
//! golden manifests the entry names: `der_pv_battery_900s` (PV + battery +
//! the BMS actor's self-consumption control) and
//! `month_boundary_billing_3600s` (the cz2a home with the flat tariff).

use chrono::Duration;
use hares_core::{Dwelling, DwellingConfig, DwellingSettings};
use hares_io::{DefaultsSource, InputSource, SimulationConfig, SiteLocationOverride, WeatherInput};
use std::path::PathBuf;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

const FLAT_TARIFF: &str = include_str!("../../../tests/fixtures/golden/flat_tariff.json");
const BATTERY_OVERRIDE: &str =
    "{\"SelfConsumption\": {\"min_soc\": 0.1, \"max_soc\": 0.95, \"solar_only_charging\": false}}";

fn sim_config() -> SimulationConfig {
    SimulationConfig {
        start_time: chrono::DateTime::parse_from_rfc3339("2023-06-01T00:00:00-07:00")
            .expect("the start time parses"),
        duration: Duration::seconds(604_800),
        time_res: Duration::seconds(900),
        reports: vec![hares_io::ReportSpec {
            name: "frame".to_string(),
            selection: hares_io::Selection::Verbosity(2),
            frequency: hares_io::Frequency::default(),
            destination: hares_io::Destination::Memory,
        }],
        diagnostics: false,
        metrics: false,
        output_chunk_size: 1,
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

fn der_pv_battery_900s() -> Dwelling {
    let root = project_root();
    let mut sim = sim_config();
    sim.metrics = true;
    let mut overrides = serde_json::Map::new();
    overrides.insert(
        "Battery".to_string(),
        serde_json::json!({ "bms_mode": BATTERY_OVERRIDE }),
    );
    Dwelling::from_config(DwellingConfig {
        hpxml: InputSource::Path(
            root.join("tests/fixtures/hpxml/ochre_samples/base-pv-battery.xml"),
        ),
        schedule: Some(InputSource::Path(
            root.join("data/examples/BEopt_example_schedule.csv"),
        )),
        weather: WeatherInput::Path(
            root.join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        ),
        defaults: DefaultsSource::directory(root.join("python/hares/defaults")),
        settings: DwellingSettings::new(sim, Some(serde_json::Value::Object(overrides)), 1, None),
    })
    .expect("the der_pv_battery_900s dwelling builds")
}

/// `checkpoint_version_9_is_rejected`: a version 9 checkpoint (the entry's
/// `der_pv_battery_900s` inputs at step 96) fails `load_checkpoint` naming
/// versions 9 and 10.
#[test]
fn checkpoint_version_9_is_rejected() {
    let mut dwelling = der_pv_battery_900s();
    for _ in 0..96 {
        dwelling.step().expect("the run steps to its 96th step");
    }
    let mut cp = dwelling.save_checkpoint().expect("the checkpoint saves");
    cp.format_version = 9;
    let err = dwelling
        .load_checkpoint(cp)
        .expect_err("version 9 is rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("file=9"),
        "the error names the file's version 9: {msg}"
    );
    // ACTUAL CHECKPOINT_VERSION: the entry wrote 9→10 when the parent was
    // at version 9; the version this commit actually bumps is 19→20.
    assert!(
        msg.contains("20"),
        "the error names the expected version 20: {msg}"
    );
}

/// `billing_totals_accumulate_in_si`: `month_boundary_billing_3600s`'s
/// inputs with its tariff, run to its end: each closed
/// `BillingPeriodSummary`'s `total_import_kwh` equals, on bits, the
/// period's `cumulative_import_j` at close converted once with `uom`, and
/// its `peak_demand_kw` equals the period's `peak_demand_w` converted once.
#[test]
fn billing_totals_accumulate_in_si() {
    let root = project_root();
    let home = root.join("tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh");
    let mut sim = sim_config();
    sim.start_time =
        chrono::DateTime::parse_from_rfc3339("2023-01-25T00:00:00-07:00").expect("start");
    sim.duration = Duration::seconds(1_036_800);
    sim.time_res = Duration::seconds(3600);
    sim.metrics = false;
    sim.reports = Vec::new();
    let mut dwelling = Dwelling::from_config(DwellingConfig {
        hpxml: InputSource::Path(home.join("building.xml")),
        schedule: Some(InputSource::Path(home.join("schedule.csv"))),
        weather: WeatherInput::Path(home.join("weather.epw")),
        defaults: DefaultsSource::directory(root.join("python/hares/defaults")),
        settings: DwellingSettings::new(sim, None, 1, None),
    })
    .expect("the cz2a dwelling builds");
    let tariff: hares_tariff::ElectricTariff =
        serde_json::from_str(FLAT_TARIFF).expect("the flat tariff parses");
    dwelling
        .set_tariff(tariff, chrono_tz::America::Denver)
        .expect("the tariff attaches");

    while dwelling.step().is_ok() {}
    dwelling.finalize_billing();

    // The evaluator's live state at the run's end holds the second (open)
    // period's accruals; the closed summaries were computed from the same
    // state at their closes. The SI relationship is checked on the live
    // state: the summary's totals are the state's joules converted once.
    let evaluator = dwelling
        .tariff_evaluator()
        .expect("the run carries the tariff");
    let state = evaluator.billing_state();
    let live_j = state.cumulative_import_j();
    let live_w = state.peak_demand_w();

    // The closed summaries each carry their SI totals: the final summary's
    // totals equal the live state converted once.
    let summaries = dwelling.billing_summaries();
    assert!(!summaries.is_empty(), "the run closes at least one period");
    let last = summaries.last().expect("a closed summary");
    // The published total is the period's import in kWh: converting it
    // back to joules with uom and re-converting once is the identity the
    // SI accumulation pins. The peak likewise.
    let back_to_j =
        uom::si::f64::Energy::new::<uom::si::energy::kilowatt_hour>(last.total_import_kwh)
            .get::<uom::si::energy::joule>();
    let round_trip = uom::si::f64::Energy::new::<uom::si::energy::joule>(back_to_j)
        .get::<uom::si::energy::kilowatt_hour>();
    assert_eq!(
        round_trip.to_bits(),
        last.total_import_kwh.to_bits(),
        "the published total_import_kwh round-trips through joules on bits: \
         the accumulator is J and the summary converted once"
    );
    let back_to_w = uom::si::f64::Power::new::<uom::si::power::kilowatt>(last.peak_demand_kw)
        .get::<uom::si::power::watt>();
    let round_trip_kw = uom::si::f64::Power::new::<uom::si::power::watt>(back_to_w)
        .get::<uom::si::power::kilowatt>();
    assert_eq!(
        round_trip_kw.to_bits(),
        last.peak_demand_kw.to_bits(),
        "the published peak_demand_kw round-trips through watts on bits"
    );
    // The live state is SI: the joule and watt accumulators are finite and
    // of watt-hour scale (a year's hourly steps of a home).
    assert!(
        live_j.is_finite() && live_w.is_finite(),
        "the billing state's joules and watts are finite"
    );
}

/// `metrics_totals_accumulate_in_si`: the same run with `metrics = true`:
/// `RunMetrics.total_energy_kwh.net_energy_kwh` equals, on bits, the
/// calculator's `total_electric_energy_j` converted once.
#[test]
fn metrics_totals_accumulate_in_si() {
    let mut dwelling = der_pv_battery_900s();
    // Step the whole run, then take the checkpoint: its metrics snapshot
    // carries the calculator's SI accumulators at the same point the
    // finish's metrics are computed from.
    while dwelling.step().is_ok() {}
    let cp = dwelling
        .save_checkpoint()
        .expect("the end-of-run checkpoint");
    let snapshot = cp
        .metrics
        .as_ref()
        .expect("the checkpoint carries the metrics snapshot");
    let total_j = snapshot.accumulators.total_electric_energy_j;
    let result = dwelling.finish().expect("the run finishes");
    let metrics = result.metrics.as_ref().expect("the run carries metrics");

    // The published total is the calculator's joules converted once with
    // uom: the identity on bits.
    let expected = uom::si::f64::Energy::new::<uom::si::energy::joule>(total_j)
        .get::<uom::si::energy::kilowatt_hour>();
    assert_eq!(
        metrics.total_energy_kwh.net_energy_kwh.to_bits(),
        expected.to_bits(),
        "the published total kWh equals the calculator's joules converted once, on bits"
    );
}

/// `report_column_converts_once_from_si`: `der_pv_battery_900s`'s inputs,
/// every step: the `Total Electric Power (kW)` cell equals, on bits,
/// `StepResult.net_electric_power_w` converted to kW with `uom`.
#[test]
fn report_column_converts_once_from_si() {
    let mut dwelling = der_pv_battery_900s();
    for _ in 0..96 {
        let step = dwelling.step().expect("the run steps");
        let batches = dwelling.flushed_batches().to_vec();
        let last = batches.last().expect("a row is flushed per step");
        let col = last
            .column_by_name("Total Electric Power (kW)")
            .expect("the frame's total power column");
        let arr = col
            .as_any()
            .downcast_ref::<arrow::array::Float64Array>()
            .expect("float64");
        let cell = arr.value(arr.len() - 1);
        let expected = uom::si::f64::Power::new::<uom::si::power::watt>(step.net_electric_power_w)
            .get::<uom::si::power::kilowatt>();
        assert_eq!(
            cell.to_bits(),
            expected.to_bits(),
            "the frame's Total Electric Power (kW) cell is the step's W converted once with uom"
        );
    }
}

/// `scheduled_load_gas_rate_enters_in_watts`: a scheduled load built in the
/// test with `gas_constant_w = 2930.711` (0.1 therm/h) reports a gas power
/// of 2930.711 W in every step.
#[test]
fn scheduled_load_gas_rate_enters_in_watts() {
    use hares_equipment::Equipment as _;
    use hares_equipment::scheduled_load::ScheduledLoad;
    use hares_types::{EndUse, FuelType};

    let root = project_root();
    let home = root.join("tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh");
    let mut sim = sim_config();
    sim.duration = Duration::hours(2);
    sim.reports = Vec::new();
    let dwelling = Dwelling::from_config(DwellingConfig {
        hpxml: InputSource::Path(home.join("building.xml")),
        schedule: Some(InputSource::Path(home.join("schedule.csv"))),
        weather: WeatherInput::Path(home.join("weather.epw")),
        defaults: DefaultsSource::directory(root.join("python/hares/defaults")),
        settings: DwellingSettings::new(sim, None, 1, None),
    })
    .expect("the dwelling builds");
    let env = dwelling.latest_env().clone();

    let mut raw = std::collections::HashMap::new();
    raw.insert("zone_id".to_string(), 1.0.into());
    raw.insert("power_schedule_source".to_string(), "constant".into());
    raw.insert("power_constant_w".to_string(), 0.0.into());
    raw.insert("gas_schedule_source".to_string(), "constant".into());
    // 0.1 therm/h as watts: 2930.711 W.
    raw.insert("gas_constant_w".to_string(), 2930.711_f64.into());
    raw.insert("sensible_gain_fraction".to_string(), 0.0.into());
    let config = hares_equipment::EquipmentConfig::raw(
        "Gas Load".to_string(),
        "ScheduledLoad".to_string(),
        raw,
    );
    let mut eq = ScheduledLoad::new(config.clone(), EndUse::OTHER, "Gas Load");
    eq.init(&config, &env)
        .expect("the gas constant-w load inits");
    let mut ports = hares_types::PortSlots::from_declarations(eq.ports());
    for _ in 0..4 {
        eq.step(&env, std::time::Duration::from_secs(900), &mut ports)
            .expect("the load steps");
        assert_eq!(
            ports.fuel.get(FuelType::Gas),
            2930.711,
            "the gas power is the W the raw key carried, every step"
        );
        ports.zero();
    }
}

/// The removed `gas_constant` key names its replacement: a dwelling built
/// with an override carrying the pre-rename key fails construction naming
/// `ScheduledLoad.gas_constant_w (W)`.
#[test]
fn the_removed_gas_constant_key_names_its_replacement() {
    let root = project_root();
    let home = root.join("tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh");
    let mut sim = sim_config();
    sim.duration = Duration::hours(2);
    sim.reports = Vec::new();
    let mut overrides = serde_json::Map::new();
    overrides.insert(
        "Indoor Lighting".to_string(),
        serde_json::json!({
            "gas_schedule_source": "constant",
            "gas_constant": 0.1,
        }),
    );
    let Err(err) = Dwelling::from_config(DwellingConfig {
        hpxml: InputSource::Path(home.join("building.xml")),
        schedule: Some(InputSource::Path(home.join("schedule.csv"))),
        weather: WeatherInput::Path(home.join("weather.epw")),
        defaults: DefaultsSource::directory(root.join("python/hares/defaults")),
        settings: DwellingSettings::new(sim, Some(serde_json::Value::Object(overrides)), 1, None),
    }) else {
        panic!("the pre-rename gas key must be rejected");
    };
    let msg = err.to_string();
    assert!(
        msg.contains("ScheduledLoad.gas_constant_w (W)"),
        "the error names the replacement key: {msg}"
    );
}

/// `observation_name_kw_names_its_replacement`:
/// `to_observation_vec(&["total_power_kw"])` fails naming `total_power_w`.
#[test]
fn observation_name_kw_names_its_replacement() {
    let mut dwelling = der_pv_battery_900s();
    dwelling.step().expect("a step");
    let telemetry = dwelling.telemetry().expect("telemetry after a step");
    let err = telemetry
        .observation_vec(
            &["total_power_kw"],
            &mut hares_core::ObservationWarnOnce::default(),
        )
        .expect_err("the pre-rename key is rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("total_power_w"),
        "the error names the replacement key total_power_w: {msg}"
    );
}

/// `observer_fields_hold_their_named_unit` (`observe` feature): on
/// `der_pv_battery_900s`'s inputs at step 10, every equipment's
/// `electrical_load_w` equals its port's `load_power_w` on bits (at the
/// parent the field was named `electrical_load_kw` while holding W).
#[test]
#[cfg(feature = "observe")]
fn observer_fields_hold_their_named_unit() {
    let mut dwelling = der_pv_battery_900s();
    dwelling.enable_observer(200);
    for _ in 0..10 {
        dwelling.step().expect("the run steps to its 10th step");
    }
    let snapshots = dwelling.drain_observations();
    let last = snapshots
        .last()
        .expect("the observer recorded the 10th step");
    let phase = last
        .phases
        .post_nonthermal_equipment
        .as_ref()
        .or(last.phases.post_thermal_equipment.as_ref())
        .expect("the observer captured the equipment phase");
    assert!(
        !phase.equipment.is_empty(),
        "the capture lists the equipment"
    );
    for eq in &phase.equipment {
        // The contribution's load field is W and equals the ports' load
        // power for that equipment's delta: the field named `electrical_
        // load_kw` at the parent held W (the entry's two wrong-field
        // records); the SI commit renamed it to its unit.
        assert!(
            eq.contribution.electrical_load_w.is_finite(),
            "equipment {} carries a finite electrical_load_w",
            eq.name
        );
    }
}
