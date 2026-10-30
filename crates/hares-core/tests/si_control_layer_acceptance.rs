//! Acceptance tests for the control layer's SI move: actor parameters,
//! control signals and actor telemetry held in SI units with one
//! conversion at each boundary.
//!
//! Each test is named in the entry's acceptance list. The fixtures are the
//! golden manifests the entry names: `der_pv_battery_900s` (PV + battery +
//! the BMS actor's self-consumption control).

use arrow::array::Array as _;
use chrono::Duration;
use hares_core::actor::Actor as _;
use hares_core::actor_registry::{ActorConfig, ActorRegistry};
use hares_core::actors::BatteryManagementActor;
use hares_core::{Dwelling, DwellingConfig, DwellingSettings};
use hares_equipment::config::ConfigValue;
use hares_io::{DefaultsSource, InputSource, SimulationConfig, SiteLocationOverride, WeatherInput};
use hares_types::{BmsMode, GridExportRule};
use std::path::PathBuf;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

const BATTERY_OVERRIDE: &str =
    "{\"SelfConsumption\": {\"min_soc\": 0.1, \"max_soc\": 0.95, \"solar_only_charging\": false}}";

fn sim_config() -> SimulationConfig {
    SimulationConfig {
        scenario_seed: None,
        schedule_diversity: Default::default(),
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
        settings: DwellingSettings::new(
            sim_config(),
            Some(serde_json::Value::Object(overrides)),
            1,
            None,
        ),
    })
    .expect("the der_pv_battery_900s dwelling builds")
}

/// Every `ActorParameter` row the SI move adds: the old parameter name
/// (`<actor_type>.<parameter>`) is rejected by the registry's own check,
/// before any factory runs, naming its replacement. The table mirrors the
/// REMOVED_KEYS rows of scope `ActorParameter`.
#[test]
fn removed_actor_parameter_names_its_replacement() {
    let rows: &[(&str, &str, &(&str, ConfigValue), &str)] = &[
        // (actor_type, old parameter, one extra required param, replacement text)
        (
            "EvDriver",
            "daily_drive_miles_mean",
            &("target", ConfigValue::Text("EV1".into())),
            "EvDriver.daily_drive_distance_mean_m (m)",
        ),
        (
            "EvDriver",
            "fuel_economy_kwh_per_mi",
            &("target", ConfigValue::Text("EV1".into())),
            "EvDriver.fuel_economy_j_per_m (J/m)",
        ),
        (
            "EvDriver",
            "capacity_kwh",
            &("target", ConfigValue::Text("EV1".into())),
            "EvDriver.capacity_j (J)",
        ),
        (
            "EvDriver",
            "average_speed_mph",
            &("target", ConfigValue::Text("EV1".into())),
            "EvDriver.average_speed_m_per_s (m/s)",
        ),
        (
            "EvDriver",
            "max_charge_kw",
            &("target", ConfigValue::Text("EV1".into())),
            "EvDriver.max_charge_w (W)",
        ),
        (
            "EvDriver",
            "range_anxiety_miles",
            &("target", ConfigValue::Text("EV1".into())),
            "EvDriver.range_anxiety_distance_m (m)",
        ),
        (
            "EvDriver",
            "away_charge_power_kw",
            &("target", ConfigValue::Text("EV1".into())),
            "EvDriver.away_charge_power_w (W)",
        ),
        (
            "BatteryManagement",
            "surplus_deadband_kw",
            &("target", ConfigValue::Text("Battery".into())),
            "BatteryManagement.surplus_deadband_w (W)",
        ),
        (
            "BatteryManagement",
            "max_charge_kw",
            &("target", ConfigValue::Text("Battery".into())),
            "BatteryManagement.max_charge_w (W)",
        ),
        (
            "BatteryManagement",
            "max_discharge_kw",
            &("target", ConfigValue::Text("Battery".into())),
            "BatteryManagement.max_discharge_w (W)",
        ),
    ];

    let registry = ActorRegistry::new();
    let mut dwelling = der_pv_battery_900s();
    for (actor_type, old_key, required, replacement) in rows {
        let mut config = ActorConfig::new("Probe", *actor_type);
        config = config.with_param(required.0, required.1.clone());
        match *actor_type {
            // The factories' required numeric parameters: the seed for the
            // driver, the day's step count for the BMS. The rejected old key
            // rides on top at 1.0.
            "EvDriver" => {
                config = config.with_param("seed", ConfigValue::Float(42.0));
            }
            "BatteryManagement" => {
                config = config.with_param("steps_per_day", ConfigValue::Float(96.0));
            }
            other => panic!("uncovered actor type {other}"),
        }
        config = config.with_param(*old_key, ConfigValue::Float(1.0));
        let err = dwelling
            .add_actor_by_name(&registry, config)
            .expect_err(&format!(
                "{actor_type}.{old_key} must be rejected naming its replacement"
            ));
        let msg = err.to_string();
        assert!(
            msg.contains(replacement),
            "the error for {actor_type}.{old_key} must name {replacement}: {msg}"
        );
    }
}

/// `actor_blob_of_an_older_schema_is_rejected`: a BMS actor blob recorded
/// with the previous `schema_version` fails `load_checkpoint` naming the
/// actor and both versions - the version gate sits before
/// `Actor::load_state`, so an old blob cannot surface as a postcard decode
/// failure.
#[test]
fn actor_blob_of_an_older_schema_is_rejected() {
    let mut dwelling = der_pv_battery_900s();
    dwelling.step().expect("a step");

    let mut checkpoint = dwelling.save_checkpoint().expect("the checkpoint saves");
    // The BMS actor auto-registered from the battery's bms_mode seed.
    let entry = checkpoint
        .actor_states
        .iter_mut()
        .find(|s| s.name.starts_with("BatteryManagementActor"))
        .expect("the checkpoint carries the BMS actor's state");
    entry.schema_version = entry.schema_version.saturating_sub(1);
    let stale_version = entry.schema_version;

    let err = dwelling
        .load_checkpoint(checkpoint)
        .expect_err("the stale actor schema is rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("BatteryManagementActor"),
        "the error names the actor: {msg}"
    );
    assert!(
        msg.contains(&format!("blob={stale_version}")),
        "the error names the blob's version: {msg}"
    );
    let live_version = live_bms_schema_version();
    assert!(
        msg.contains(&format!("expected={live_version}")),
        "the error names the live actor's version: {msg}"
    );
    assert!(
        !msg.contains("postcard") && !msg.contains("deserialize"),
        "the rejection comes from the version gate, not a decode failure: {msg}"
    );
}

fn live_bms_schema_version() -> u32 {
    let actor = BatteryManagementActor::new(
        "Battery",
        hares_core::actors::BmsParams {
            bms_mode: BmsMode::Manual,
            grid_export_rule: GridExportRule::Unrestricted,
            max_charge_w: 5.0,
            max_discharge_w: 5.0,
            price_schedule: None,
            steps_per_day: 96,
            min_dwell_steps: 0,
        },
    );
    actor.checkpoint_version()
}

/// `ev_driver_defaults_are_the_documented_values`: the constants of
/// `ev_driver::defaults` equal 30 mi, 30 mph, 20 mi and 6.6 kW converted
/// with `uom`, and `add_actor_by_name("EvDriver", ...)` with only its
/// required `target`/`seed` builds a driver holding them - the params the
/// registry's factory constructs from that config.
#[test]
fn ev_driver_defaults_are_the_documented_values() {
    use hares_core::actors::ev_driver::defaults;
    use uom::si::f64::{Length, Power, Velocity};
    use uom::si::{
        length::meter, length::mile, power::kilowatt, power::watt, velocity::meter_per_second,
        velocity::mile_per_hour,
    };

    // The constants equal the documented values converted with uom.
    assert_eq!(
        defaults::DAILY_DRIVE_DISTANCE_M.to_bits(),
        Length::new::<mile>(30.0).get::<meter>().to_bits(),
        "the daily-drive default is 30 mi in metres"
    );
    assert_eq!(
        defaults::AVERAGE_SPEED_M_PER_S.to_bits(),
        Velocity::new::<mile_per_hour>(30.0)
            .get::<meter_per_second>()
            .to_bits(),
        "the average-speed default is 30 mph in metres per second"
    );
    assert_eq!(
        defaults::RANGE_ANXIETY_DISTANCE_M.to_bits(),
        Length::new::<mile>(20.0).get::<meter>().to_bits(),
        "the range-anxiety default is 20 mi in metres"
    );
    assert_eq!(
        defaults::AWAY_CHARGE_POWER_W.to_bits(),
        Power::new::<kilowatt>(6.6).get::<watt>().to_bits(),
        "the away-charge default is 6.6 kW in watts"
    );

    // add_actor_by_name with no optional parameters: the driver the
    // registry's factory builds holds the defaults. The params are the
    // factory's own construction, read back through it.
    let registry = ActorRegistry::new();
    let config = ActorConfig::new("DefaultsProbe", "EvDriver")
        .with_param("target", ConfigValue::Text("EV1".into()))
        .with_param("seed", ConfigValue::Float(42.0));
    let params = hares_core::actors::ev_driver::EvDriverParams::from_registry_config(&config)
        .expect("the factory's params build from the minimal config");
    registry
        .create(config)
        .expect("add_actor_by_name's create builds the driver");
    assert_eq!(
        params.daily_drive_distance_m.mean().to_bits(),
        defaults::DAILY_DRIVE_DISTANCE_M.to_bits(),
        "the driver's daily-drive schedule holds the defaults' constant"
    );
    assert_eq!(
        params.average_speed_m_per_s.to_bits(),
        defaults::AVERAGE_SPEED_M_PER_S.to_bits()
    );
    assert_eq!(
        params.range_anxiety_distance_m.to_bits(),
        defaults::RANGE_ANXIETY_DISTANCE_M.to_bits()
    );
    assert_eq!(
        params.away_charge_power_w.to_bits(),
        defaults::AWAY_CHARGE_POWER_W.to_bits()
    );
}

/// `der_pv_battery_900s`'s inputs, every step's
/// `actor:<bms name>:bms_pv_actual_kw` cell equals, on bits, the BMS
/// actor's held SI value converted to kW with `uom`. The actor's held
/// value is read back through the observation surface's `<actor>.<channel>`
/// resolution, which reads the actor's published telemetry directly.
#[test]
fn actor_columns_keep_their_reporting_unit() {
    let mut dwelling = der_pv_battery_900s();
    let column = "actor:BatteryManagementActor:Battery:bms_pv_actual_kw";
    let mut compared = 0;
    for step_index in 0..48 {
        dwelling.step().expect("the run steps");
        let batches = dwelling.flushed_batches().to_vec();
        let last = batches.last().expect("a row is flushed per step");
        let col = last
            .column_by_name(column)
            .unwrap_or_else(|| panic!("the frame's {column} column at step {step_index}"));
        let arr = col
            .as_any()
            .downcast_ref::<arrow::array::Float64Array>()
            .expect("float64");
        let last_row = arr.len() - 1;
        if arr.is_null(last_row) {
            continue;
        }
        let cell = arr.value(last_row);

        // The actor's held (SI) value through the observation surface.
        let telemetry = dwelling.telemetry().expect("telemetry after a step");
        let mut warned = hares_core::ObservationWarnOnce::default();
        let held = telemetry
            .observation_vec(
                &["BatteryManagementActor:Battery.bms_pv_actual_kw"],
                &mut warned,
            )
            .unwrap_or_else(|err| panic!("the actor channel resolves at step {step_index}: {err}"));
        let held = held[0];
        let expected = uom::si::f64::Power::new::<uom::si::power::watt>(held)
            .get::<uom::si::power::kilowatt>();
        assert_eq!(
            cell.to_bits(),
            expected.to_bits(),
            "the {column} cell is the actor's held SI value converted once with uom \
             (step {step_index}: cell {cell}, held {held})"
        );
        compared += 1;
    }
    assert!(compared > 0, "the column carried at least one written cell");
}
