//! Equipment parameter overrides reach the typed configs every equipment
//! initialises from.
//!
//! The dwelling-level override map (Python's `overrides=`, the synthetic
//! TOML's `[overrides]`, and `DwellingConfig.overrides`) and a spec's own
//! parameter bag must both land in the typed config the equipment is built
//! from: an override of a typed field changes the equipment's behaviour, an
//! unknown field is an error naming the equipment and field, and a value
//! the typed schema rejects fails the build at the point the override is
//! applied (never an init that non-critical equipment survives by being
//! skipped and silently missing from the run).

use std::path::{Path, PathBuf};

use chrono::{Duration, FixedOffset, TimeZone};
use hares_core::{Dwelling, DwellingConfig};
use hares_io::OutputFormat;
use serde_json::{Value, json};

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture(name: &str) -> PathBuf {
    project_root()
        .join("tests/fixtures/hpxml/ochre_samples")
        .join(name)
}

fn config(hpxml: &Path, overrides: Option<Value>, start_hour: u32) -> DwellingConfig {
    let tz_offset = FixedOffset::west_opt(7 * 3600).expect("UTC-7 offset is valid");
    let start_time = tz_offset
        .with_ymd_and_hms(2023, 1, 15, start_hour, 0, 0)
        .single()
        .expect("valid start time");
    DwellingConfig {
        hpxml_path: hpxml.to_path_buf(),
        // No schedule file: the generated schedule is the test's condition.
        schedule_path: None,
        weather_path: project_root().join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        defaults_path: Some(project_root().join("defaults")),
        sim_config: hares_io::SimulationConfig {
            start_time,
            duration: Duration::hours(24),
            time_res: Duration::seconds(3600),
            output_verbosity: 0,
            write_output: false,
            output_path: None,
            output_format: OutputFormat::Csv,
            output_chunk_size: 1024,
            setpoint_deadband_c: None,
            master_seed: 0,
            civil_timezone: None,
            site_location: hares_io::SiteLocationOverride::default(),
            retain_batches: false,
            rotation: hares_io::RotationPolicy::None,
        },
        overrides,
        bldg_id: 1,
        initialization_duration: None,
        resample_overrides: Some(hares_io::ResampleOverrides::ochre_compat()),
        patches: None,
    }
}

fn step_n(dwelling: &mut Dwelling, n: usize) -> Vec<f64> {
    (0..n)
        .map(|_| {
            dwelling
                .step()
                .expect("the run must step without an error")
                .net_electric_power_kw
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
    let enlarged = net_electric_for(Some(json!({"PV": {"capacity_kw": 9.0}})), "base-pv.xml", 4);

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
        cfg.sim_config.write_output = true;
        cfg.sim_config.output_path = Some(csv.to_path_buf());
        let result = hares_core::SimulationEngine::new()
            .run(cfg)
            .expect("the engine run must succeed");
        assert!(
            !matches!(result.status, hares_core::SimStatus::Failed(_)),
            "the simulation must not fail: {:?}",
            result.status
        );
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
        "surplus_deadband_kw": 0.0,
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
    fn pv_generation_kw(bag_capacity_kw: f64) -> f64 {
        let mut blueprint = hares_core::dwelling::DwellingBlueprint::from_config(config(
            &fixture("base.xml"),
            None,
            10,
        ))
        .expect("blueprint");
        let typed_cfg = hares_equipment::PvConfig {
            equipment_id: None,
            zone_id: None,
            capacity_kw: 5.0,
            tilt_deg: Some(30.0),
            azimuth_deg: Some(180.0),
            module_type: None,
            noct_c: None,
            array_type: None,
            system_losses_fraction: None,
            inverter_efficiency: None,
            inverter_capacity_kw: None,
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
        parameters.insert("capacity_kw".to_string(), json!(bag_capacity_kw));
        let spec = hares_io::EquipmentSpec {
            instance_name: None,
            name: "PV".to_string(),
            fuel_type: hares_types::FuelType::Electric,
            parameters,
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
        };
        blueprint.add_equipment_spec(spec).expect("add PV spec");
        let mut dwelling = blueprint.build().expect("the dwelling must build");
        dwelling.step().expect("the run must step");
        let pv = dwelling
            .equipment()
            .iter()
            .find(|eq| eq.descriptor().name == "PV")
            .expect("the assembled dwelling must contain the added PV");
        match &pv.core_output().flows.electric_kw {
            Some(hares_types::ElectricPower::Generation(kw)) => *kw,
            other => panic!("a midday PV step must publish a generation flow, got {other:?}"),
        }
    }

    let mirrored = pv_generation_kw(5.0);
    let overridden = pv_generation_kw(9.0);

    assert!(
        overridden > mirrored,
        "the spec bag's capacity override must reach the typed config the PV \
         initialises from and raise its own generation (5 kW fleet value {} \
         against the 9 kW override {})",
        mirrored,
        overridden
    );
}
