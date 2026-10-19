//! Equipment parameter overrides reach the typed configs every equipment
//! initialises from.
//!
//! The dwelling-level override map (Python's `overrides=`, the synthetic
//! TOML's `[overrides]`, and `DwellingConfig.overrides`) and a spec's own
//! parameter bag must both land in the typed config the equipment is built
//! from: an override of a typed field changes the equipment's behaviour, an
//! unknown field is an error naming the equipment and field, and a value
//! the typed schema rejects fails the build at the point the override is
//! applied, before any equipment init runs.

use hares_core::DwellingSettings;
use hares_io::{DefaultsSource, InputSource, WeatherInput};
use std::path::{Path, PathBuf};

use chrono::{Duration, TimeZone};
use hares_core::actors::{BatteryManagementActor, BmsParams};
use hares_core::{Dwelling, DwellingConfig};
use hares_types::{BmsMode, GridExportRule};
use serde_json::{Value, json};

#[path = "../../../tests/support/denver_offset.rs"]
mod denver_offset;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture(name: &str) -> PathBuf {
    project_root()
        .join("tests/fixtures/hpxml/ochre_samples")
        .join(name)
}

fn config(hpxml: &Path, overrides: Option<Value>, start_hour: u32) -> DwellingConfig {
    let start_time = denver_offset::denver_offset()
        .with_ymd_and_hms(2023, 1, 15, start_hour, 0, 0)
        .single()
        .expect("valid start time");
    DwellingConfig {
        hpxml: InputSource::Path(hpxml.to_path_buf()),
        schedule: None, // No schedule file: the generated schedule is the test's condition.
        weather: WeatherInput::Path(
            project_root().join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        ),
        defaults: DefaultsSource::directory(project_root().join("python/hares/defaults")),
        settings: DwellingSettings::new(
            hares_io::SimulationConfig {
                start_time,
                duration: Duration::hours(24),
                time_res: Duration::seconds(3600),
                reports: Vec::new(),
                diagnostics: false,
                metrics: false,
                output_chunk_size: 1024,
                setpoint_deadband_c: None,
                master_seed: 0,
                civil_timezone: None,
                site_location: hares_io::SiteLocationOverride::default(),
                max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
                health_policy: hares_io::HealthPolicy::default(),
                first_hour_interpolation: Default::default(),
                weather_year: hares_io::WeatherYear::default(),
                warmup: hares_io::Warmup::Disabled,
            },
            overrides,
            1,
            None,
        ),
    }
}

fn step_n(dwelling: &mut Dwelling, n: usize) -> Vec<f64> {
    (0..n)
        .map(|_| {
            dwelling
                .step()
                .expect("the run must step without an error")
                .net_electric_power_w
        })
        .collect()
}

fn net_electric_for(overrides: Option<Value>, hpxml: &str, steps: usize) -> Vec<f64> {
    let mut dwelling = Dwelling::from_config(config(&fixture(hpxml), overrides, 10))
        .expect("the dwelling must build");
    step_n(&mut dwelling, steps)
}

#[test]
fn pv_capacity_kw_override_changes_generation() {
    let baseline = net_electric_for(None, "base-pv.xml", 4);
    let enlarged = net_electric_for(
        Some(json!({"PV": {"capacity_w": 9000.0}})),
        "base-pv.xml",
        4,
    );

    assert!(
        baseline != enlarged,
        "a PV capacity override of 9 kW must change the run against the \
         5 kW baseline; net electric baseline {baseline:?} vs override {enlarged:?}"
    );
}

#[test]
fn water_heater_setpoint_override_changes_behaviour() {
    let baseline = net_electric_for(None, "base.xml", 24);
    let hotter = net_electric_for(
        Some(json!({"Electric Resistance Water Heater": {"setpoint_c": 60.0}})),
        "base.xml",
        24,
    );

    assert!(
        baseline != hotter,
        "a water-heater setpoint override of 60 C must change the run \
         against the default setpoint; baseline {baseline:?} vs override {hotter:?}"
    );
}

#[test]
fn battery_control_override_makes_the_pack_cycle() {
    fn max_observed_soc(overrides: Option<Value>, csv: &PathBuf) -> f64 {
        let mut cfg = config(&fixture("base-battery.xml"), overrides, 10);
        cfg.settings.sim_config.reports = vec![hares_io::ReportSpec {
            name: "frame".to_string(),
            selection: hares_io::Selection::Verbosity(0),
            frequency: hares_io::Frequency::default(),
            destination: hares_io::Destination::Csv {
                path: csv.to_path_buf(),
                overwrite: false,
            },
        }];
        hares_core::SimulationEngine::new()
            .run(cfg)
            .expect("the engine run must succeed");
        let contents = std::fs::read_to_string(csv).expect("read output CSV");
        let header: Vec<&str> = contents
            .lines()
            .next()
            .expect("header")
            .split(',')
            .collect();
        let soc_col = header
            .iter()
            .position(|c| *c == "actor:BatteryManagementActor:Battery:soc")
            .unwrap_or_else(|| panic!("BMS soc column missing; header: {header:?}"));
        contents
            .lines()
            .skip(1)
            .filter_map(|l| l.split(',').nth(soc_col))
            .filter_map(|v| v.trim().parse::<f64>().ok())
            .fold(0.0_f64, f64::max)
    }

    let tmp = tempfile::tempdir().expect("temp dir");
    let bms_mode = json!({"SelfConsumption": {
        "min_soc": 0.1,
        "max_soc": 1.0,
        "solar_only_charging": false,
        "surplus_deadband_w": 0.0,
    }});
    let cycling = max_observed_soc(
        Some(json!({"Battery": {"bms_mode": bms_mode.to_string()}})),
        &tmp.path().join("cycling.csv"),
    );

    assert!(
        cycling > 0.0,
        "a SelfConsumption control override must seed the pack's management \
         actor and cycle the battery, got max observed soc {cycling}"
    );
}

#[test]
fn dehumidifier_invalid_override_value_fails_the_build() {
    let overrides = json!({"Dehumidifier": {"capacity_liters_per_day": "lots"}});
    let err = Dwelling::from_config(config(
        &fixture("base-appliances-dehumidifier.xml"),
        Some(overrides),
        10,
    ))
    .err()
    .expect(
        "an override value the typed schema rejects must fail the build, \
        not skip the equipment with a warning",
    );

    let msg = err.to_string();
    assert!(
        msg.contains("Dehumidifier"),
        "the error must name the equipment, got: {msg}"
    );
    assert!(
        msg.contains("capacity_liters_per_day"),
        "the error must name the field, got: {msg}"
    );
}

#[test]
fn blueprint_added_pv_applies_the_spec_bag_override() {
    fn pv_generation_w(bag_capacity_kw: f64) -> f64 {
        let mut blueprint = hares_core::dwelling::DwellingBlueprintBuilder::from_config(config(
            &fixture("base.xml"),
            None,
            10,
        ))
        .expect("blueprint");
        let typed_cfg = hares_equipment::PvConfig {
            equipment_id: None,
            zone_id: None,
            capacity_w: 5000.0,
            tilt_deg: Some(30.0),
            azimuth_deg: Some(180.0),
            attached_boundary_id: None,
            module_type: None,
            noct_c: None,
            array_type: None,
            system_losses_fraction: None,
            inverter_efficiency: None,
            inverter_capacity_w: None,
            power_factor: None,
            surface_resolution_deg: None,
            sam_lut_path: None,
            soiling: None,
            arrays: None,
        };
        let mut parameters = serde_json::to_value(&typed_cfg)
            .expect("serializable PV config")
            .as_object()
            .cloned()
            .expect("PV config object");
        parameters.insert("capacity_w".to_string(), json!(bag_capacity_kw * 1000.0));
        let mut typed_overrides = serde_json::Map::new();
        typed_overrides.insert("capacity_w".to_string(), json!(bag_capacity_kw * 1000.0));
        let spec = hares_io::EquipmentSpec {
            instance_name: None,
            name: "PV".to_string(),
            fuel_type: hares_types::FuelType::Electric,
            parameters,
            typed_overrides,
            zip_params: None,
            typed_config: Some(
                hares_equipment::EquipmentConfig::from_typed(
                    "PV".to_string(),
                    "PV".to_string(),
                    typed_cfg,
                )
                .expect("typed PV config"),
            ),
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
        };
        blueprint.add_equipment_spec(spec).expect("add PV spec");
        let mut dwelling = blueprint.build().expect("the dwelling must build");
        dwelling.step().expect("the run must step");
        let pv = dwelling
            .equipment()
            .iter()
            .find(|eq| eq.descriptor().name == "PV")
            .expect("the assembled dwelling must contain the added PV");
        match &pv.core_output().flows.electric_w {
            Some(hares_types::ElectricPower::Generation(kw)) => *kw,
            other => panic!("a midday PV step must publish a generation flow, got {other:?}"),
        }
    }

    let mirrored = pv_generation_w(5.0);
    let overridden = pv_generation_w(9.0);

    assert!(
        overridden > mirrored,
        "the spec bag's capacity override must reach the typed config the PV \
         initialises from and raise its own generation (5 kW fleet value {} \
         against the 9 kW override {})",
        mirrored,
        overridden
    );
}

// An HPXML battery whose control cannot be resolved is a construction
// error naming the input, not a silent default control. HPXML has no
// battery-control element, and OCHRE resolves a battery's control per
// step from a control signal or its schedule
// (ochre/Equipment/Generator.py:27, ochre/Equipment/Battery.py:205),
// never from HPXML, so an uncontrolled battery seeds no management
// actor and idles for the whole run while looking configured. The
// declared routes are the battery override's `bms_mode` (`"Manual"`
// included: the battery follows commands) and a `BatteryManagement`
// actor targeting the battery.
const MANUAL_BMS_MODE: &str = "\"Manual\"";

#[test]
fn battery_without_declared_control_is_a_construction_error() {
    let err = Dwelling::from_config(config(&fixture("base-battery.xml"), None, 10))
        .err()
        .expect("a battery with no declared control must fail the build");

    let msg = err.to_string();
    assert!(
        msg.contains("Battery1"),
        "the error must name the battery's HPXML SystemIdentifier, got: {msg}"
    );
    assert!(
        msg.contains("base-battery.xml"),
        "the error must name the input document, got: {msg}"
    );
    assert!(
        msg.contains("bms_mode"),
        "the error must name the declaration the input is missing, got: {msg}"
    );
}

#[test]
fn explicit_manual_battery_builds() {
    let overrides = json!({ "Battery": { "bms_mode": MANUAL_BMS_MODE } });
    let mut dwelling =
        Dwelling::from_config(config(&fixture("base-battery.xml"), Some(overrides), 10))
            .expect("an explicitly declared Manual battery is a declared control and must build");
    dwelling
        .step()
        .expect("the declared control must step without an error");
    let soc = bms_observed_soc(&dwelling);
    assert_eq!(
        soc, 0.0,
        "a declared Manual battery seeds no management actor of its own: it \
         follows commands, so no BMS channel may report for it"
    );
}

#[test]
fn bms_actor_counts_as_declared_control() {
    // The registered-actor route: a battery joined to a built dwelling and
    // driven by a BatteryManagement actor needs no bms_mode declaration,
    // because the actor is the control.
    let mut dwelling = Dwelling::from_config(config(&fixture("base.xml"), None, 10))
        .expect("the base dwelling must build");

    let battery_cfg: hares_equipment::BatteryConfig = serde_json::from_value(json!({
        "capacity_j": 13.5 * 3.6e6,
        "max_charge_w": 5000.0,
        "max_discharge_w": 5000.0,
    }))
    .expect("minimal BatteryConfig");
    let eq_config = hares_equipment::EquipmentConfig::from_typed(
        "Battery".to_string(),
        "Battery".to_string(),
        battery_cfg,
    )
    .expect("typed battery config");
    let mut battery = hares_equipment::EquipmentRegistry::new()
        .create("Battery", eq_config.clone())
        .expect("create the battery");
    battery
        .init(&eq_config, dwelling.latest_env())
        .expect("init the battery");
    dwelling
        .add_equipment(battery)
        .expect("the battery joins the built dwelling");

    dwelling
        .add_actor(Box::new(BatteryManagementActor::new(
            "Battery",
            BmsParams {
                bms_mode: BmsMode::SelfConsumption {
                    min_soc: 0.1,
                    max_soc: 1.0,
                    solar_only_charging: false,
                    surplus_deadband_w: 0.0,
                },
                grid_export_rule: GridExportRule::Unrestricted,
                max_charge_kw: 5.0,
                max_discharge_kw: 5.0,
                price_schedule: None,
                steps_per_day: 24,
                min_dwell_steps: 0,
            },
        )))
        .expect("the actor targeting the battery must attach");

    for _ in 0..6 {
        dwelling.step().expect("the run must step");
    }
    let soc = bms_observed_soc(&dwelling);
    assert!(
        soc > 0.0 && soc < 0.5,
        "the actor must drive the battery from its declared initial SOC \
         (self-consumption discharges into the load), got soc = {soc}"
    );
}

fn bms_observed_soc(dwelling: &Dwelling) -> f64 {
    dwelling
        .telemetry()
        .unwrap()
        .actor_telemetry
        .get("BatteryManagementActor:Battery")
        .and_then(|channels| channels.get("soc"))
        .copied()
        .unwrap_or(0.0)
}

/// A battery whose typed payload cannot be read fails the build naming the
/// battery, its SystemIdentifier and the input document: the control check
/// reads every battery's merged config before any equipment is constructed,
/// and a payload the schema rejects must surface with the same context the
/// missing-control error carries, not an anonymous deserialization failure.
#[test]
fn unreadable_battery_config_fails_the_build_naming_the_battery() {
    let mut blueprint = hares_core::dwelling::DwellingBlueprintBuilder::from_config(config(
        &fixture("base.xml"),
        None,
        10,
    ))
    .expect("the base dwelling must plan");
    blueprint
        .add_equipment_spec(hares_io::EquipmentSpec {
            name: "Battery".to_string(),
            instance_name: Some("House Battery".to_string()),
            fuel_type: hares_types::FuelType::Electric,
            parameters: serde_json::Map::new(),
            typed_overrides: serde_json::Map::new(),
            zip_params: None,
            typed_config: Some(hares_equipment::EquipmentConfig::with_payload(
                "House Battery".to_string(),
                "Battery".to_string(),
                hares_equipment::ConfigPayload::Typed {
                    type_name: "Battery".to_string(),
                    version: 1,
                    data: json!({ "capacity_j": "not a number" }),
                },
            )),
            system_id: Some("BAT-1".to_string()),
            related_hvac_idref: None,
            primary_role: None,
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
        })
        .expect("add the battery spec");

    let err = blueprint
        .build()
        .err()
        .expect("an unreadable battery config must fail the build");

    let msg = err.to_string();
    assert!(
        msg.contains("House Battery"),
        "the error must name the battery, got: {msg}"
    );
    assert!(
        msg.contains("BAT-1"),
        "the error must name the battery's SystemIdentifier, got: {msg}"
    );
    assert!(
        msg.contains("base.xml"),
        "the error must name the input document, got: {msg}"
    );
    assert!(
        msg.contains("typed config"),
        "the error must carry the deserialization cause, got: {msg}"
    );
}
