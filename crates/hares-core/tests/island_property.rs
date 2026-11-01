//! The grid-island conservation property over RANDOM islands.
//!
//! The fixed witnesses pin the semantics: an island survives a DR shed
//! (a shed curtails grid interchange, and an island has none), and dies
//! to a latched IEEE 1547 trip (a source tripped while grid-connected is
//! not available to form the island). This file keeps those witnesses'
//! contract as a PROPERTY over randomized island configurations: for a
//! deterministic ensemble of islands (random source mix, battery state of
//! charge, discharge capability, load level, shed or not, trip or not):
//!
//! 1. CONSERVATION: every islanded step, no pack gains more energy than
//!    the island's sources delivered that step.
//! 2. DR SURVIVAL: whenever an island has formed, a DR shed of the
//!    island's source never collapses it, whatever the configuration.
//! 3. 1547 DEATH: a source latched-tripped by a grid-connected excursion
//!    is no island source: the bus stays dead through the outage.
//!
//! The ensemble draws from a fixed ChaCha8 stream (seed 71, the row that
//! fixed the island/DR/1547 semantics), so the property's cases are
//! reproducible and a failure names its case index and configuration.

use std::fs;
use std::path::Path;

use hares_core::Dwelling;
use hares_equipment::battery::Battery;
use hares_equipment::config::ConfigValue;
use hares_equipment::scheduled_load::ScheduledLoad;
use hares_equipment::{BatteryConfig, Equipment, EquipmentConfig};
use hares_types::EndUse;
use hares_types::telemetry_keys as tk;
use rand::RngExt;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

const BATTERY_CAPACITY_J: f64 = 10.0 * 3.6e6;

/// The synthetic single-zone dwelling the island tests build: no HVAC, a
/// constant-weather fixture, one 60 s step horizon long enough for the
/// scenario's steps.
fn write_minimal_toml(path: &Path, tag: &str) {
    let content = r#"building_id = 9401

[simulation]
start_time = "2024-06-15T12:00:00Z"
time_res = 60
duration = 3600
output_chunk_size = 1000
master_seed = 0
[geometry]
number_of_bedrooms = 3.0
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.8

[hvac]
equipment_name = "none"

[weather]
outdoor_temp_c = 20.0
dew_point_c = 10.0
rel_humidity_pct = 50.0
pressure_kpa = 101.325

[infiltration]
ach = 0.0

[schedule]
occupancy = 0.0
"#;
    let dir = path.join(tag);
    fs::create_dir_all(&dir).expect("the case directory creates");
    fs::write(dir.join("case.toml"), content).expect("failed to write synthetic TOML");
}

/// One ensemble case's configuration, drawn from the stream.
#[derive(Debug, Clone, Copy)]
struct IslandCase {
    battery_soc: f64,
    max_discharge_kw: f64,
    load_kw: f64,
    shed: bool,
    trip_1547: bool,
}

fn draw_cases(n: usize) -> Vec<(usize, IslandCase)> {
    let mut rng = ChaCha8Rng::seed_from_u64(71);
    let mut cases = Vec::with_capacity(n);
    for i in 0..n {
        // The battery's SOC floor is its min_soc (0.1); the discharge
        // capability spans below and above the load so both the served and
        // the unserved regimes appear; the load spans 0.5-2.0 kW.
        let battery_soc = 0.2 + 0.8 * rng.random::<f64>();
        let max_discharge_kw = 0.4 + 1.6 * rng.random::<f64>();
        let load_kw = 0.5 + 1.5 * rng.random::<f64>();
        let shed = rng.random::<f64>() < 0.5;
        let trip_1547 = rng.random::<f64>() < 0.5;
        cases.push((
            i,
            IslandCase {
                battery_soc,
                max_discharge_kw,
                load_kw,
                shed,
                trip_1547,
            },
        ))
    }
    cases
}

/// The dwelling one case builds: the constant-power scheduled load at the
/// case's level and the battery at the case's SOC, capability and trip.
fn build_case(scratch: &Path, case: (usize, IslandCase)) -> Dwelling {
    let (index, c) = case;
    let tag = format!("case-{index}");
    write_minimal_toml(scratch, &tag);
    let toml_path = scratch.join(&tag).join("case.toml");
    let mut dwelling = Dwelling::from_toml_config(&toml_path).expect("build dwelling");

    let env = dwelling.latest_env().clone();
    let mut raw: std::collections::HashMap<String, ConfigValue> = std::collections::HashMap::new();
    raw.insert("power_schedule_source".to_string(), "constant".into());
    raw.insert("power_constant_w".to_string(), (c.load_kw * 1000.0).into());
    raw.insert("sensible_gain_fraction".to_string(), 0.5.into());
    raw.insert("zone_id".to_string(), 1.0.into());
    let config = EquipmentConfig::raw("BaseLoad".to_string(), "ScheduledLoad".to_string(), raw);
    let mut eq = ScheduledLoad::new(config.clone(), EndUse::LIGHTING, "Lighting");
    eq.init(&config, &env).expect("init ScheduledLoad");
    dwelling
        .add_equipment(Box::new(eq))
        .expect("add_equipment must succeed");

    let battery_config = BatteryConfig {
        equipment_id: None,
        zone_id: None,
        capacity_j: BATTERY_CAPACITY_J,
        max_charge_w: 5000.0,
        max_discharge_w: c.max_discharge_kw * 1000.0,
        n_series: None,
        n_parallel: None,
        ah_cell: None,
        v_cell: None,
        cell_resistance_ohm: None,
        pack_voltage_v: None,
        chemistry: None,
        standby_power_w: None,
        self_discharge_pct_per_day: None,
        min_soc: Some(0.1),
        max_soc: None,
        initial_soc: Some(c.battery_soc),
        initial_cell_temp_c: None,
        import_limit_w: None,
        export_limit_w: None,
        heater_power_w: None,
        heater_threshold_c: None,
        min_discharge_temp_c: None,
        full_power_temp_c: None,
        min_charge_temp_c: None,
        cell_thermal_mass_j_per_k: None,
        cell_ua_w_per_k: None,
        inverter_efficiency: None,
        charge_efficiency: None,
        discharge_efficiency: None,
        bms_mode: None,
        grid_export_rule: None,
        power_factor: None,
        inverter_capacity_va: None,
        grid_forming: None,
        min_dwell_steps: 0,
        // The 1547 layer, configured only in the trip cases: a category II
        // inverter trips the sustained sub-0.9 pu excursion the scenario
        // drives while grid-connected.
        der_1547: if c.trip_1547 {
            Some(hares_equipment::der1547::Der1547Settings {
                category: hares_equipment::der1547::Der1547Category::CatII,
                overrides: None,
            })
        } else {
            None
        },
    };
    let config =
        EquipmentConfig::from_typed("Battery".to_string(), "Battery".to_string(), battery_config)
            .expect("typed battery config");
    let env = dwelling.latest_env().clone();
    let mut bat = Battery::new(config.clone());
    bat.init(&config, &env).expect("init Battery");
    dwelling
        .add_equipment(Box::new(bat))
        .expect("add_equipment must succeed");
    dwelling
}

/// The equipment id the battery's telemetry reads under (the SOC channel's
/// index in the dwelling telemetry's roster order). Reads the battery's own
/// descriptor id through the environment's telemetry keys.
fn battery_soc(dwelling: &Dwelling) -> f64 {
    dwelling
        .latest_env()
        .equipment_telemetry
        .get("Battery")
        .and_then(|t| t.get(tk::SOC))
        .unwrap_or_else(|| panic!("the battery publishes soc"))
}

/// The ensemble: every case runs the same scenario shape (nominal step,
/// the optional 1547 trip excursion, the outage, the island's formation,
/// the optional DR shed, ten islanded steps) and asserts the properties.
#[test]
fn the_island_properties_hold_over_the_random_ensemble() {
    let cases = draw_cases(24);
    let scratch = tempfile::tempdir().expect("scratch directory");

    for case in &cases {
        let (index, c) = *case;
        let mut dwelling = build_case(scratch.path(), *case);

        // The nominal step.
        dwelling.step().expect("nominal step");

        // The trip cases: the grid-connected excursion trips the
        // configured inverter and the trip latches.
        if c.trip_1547 {
            dwelling
                .set_grid_voltage(0.6)
                .expect("the sag override is in contract");
            dwelling.step().expect("the sag step");
            let tripped = dwelling
                .latest_env()
                .equipment_telemetry
                .get("Battery")
                .and_then(|t| t.get(tk::DER1547_TRIPPED))
                .unwrap_or_else(|| panic!("case {index}: the battery publishes its trip state"));
            assert_eq!(
                tripped, 1.0,
                "case {index}: the configured inverter must trip the 0.6 pu \
                 excursion for the death property to mean anything"
            );
            dwelling
                .set_grid_voltage(1.0)
                .expect("the restoration override is in contract");
            dwelling.step().expect("the restoration step");
        }

        // The outage and the island's formation window.
        dwelling
            .set_grid_voltage(0.0)
            .expect("the outage override is in contract");
        dwelling.step().expect("the outage's dead-bus step");
        dwelling.step().expect("the island's formation step");

        // The DR shed, armed mid-island for the shed cases.
        if c.shed {
            dwelling.apply_control(
                "Battery",
                hares_types::ControlSignal::DemandResponse {
                    level: hares_types::DRLevel::GridEmergency,
                    duration_s: Some(10.0 * 60.0),
                },
            );
        }

        // Ten islanded steps: the conservation and survival properties.
        let mut prev_soc = battery_soc(&dwelling);
        let mut islanded_steps = 0;
        for step in 0..10 {
            dwelling.step().expect("the islanded step");
            if c.trip_1547 {
                assert!(
                    !dwelling.latest_env().grid.bus_energized(),
                    "case {index} step {step}: a latched 1547 trip is no \
                     island source: the bus must stay dead"
                );
                continue;
            }
            assert!(
                dwelling.latest_env().grid.islanded(),
                "case {index} step {step}: the battery-backed home must \
                 island one step after the outage{}",
                if c.shed {
                    " and survive the DR shed"
                } else {
                    ""
                }
            );
            islanded_steps += 1;

            let soc = battery_soc(&dwelling);
            let summary = dwelling
                .save_checkpoint()
                .expect("checkpoint")
                .prior_electrical_summary;
            // The island's sources delivered: the battery's discharge (the
            // only source in this ensemble; the fixture registers no PV
            // surfaces and no generator). The pack's gain per step may not
            // exceed the delivery.
            let sources_j = (-summary.battery_power_w).max(0.0) * 60.0;
            let soc_gain_j = (soc - prev_soc).max(0.0) * BATTERY_CAPACITY_J;
            assert!(
                soc_gain_j <= sources_j + 1.0,
                "case {index} step {step}: the pack gained {soc_gain_j:.1} J \
                 in a step whose island sources delivered {sources_j:.1} J"
            );
            prev_soc = soc;
        }

        // The non-trip cases must actually island for the property to mean
        // anything: the SOC and the capability edge cases (the load at the
        // capability) still hold the bus; only the trip removes it.
        if !c.trip_1547 {
            assert!(
                islanded_steps == 10,
                "case {index}: the ensemble case must island through all ten \
                 steps (got {islanded_steps}): a config that cannot island \
                 (SOC at the floor, no capability) must not be in the ensemble"
            );
        }
    }
}

/// The ensemble's spread: the drawn cases must cover the configuration
/// surface (shed and unshed, tripped and not, served and unserved), so a
/// regression to a single witness's shape cannot pass the property vacuously.
#[test]
fn the_random_ensemble_covers_the_configuration_surface() {
    let cases = draw_cases(24);
    let sheds = cases.iter().filter(|(_, c)| c.shed).count();
    let trips = cases.iter().filter(|(_, c)| c.trip_1547).count();
    let unserved = cases
        .iter()
        .filter(|(_, c)| c.load_kw > c.max_discharge_kw)
        .count();
    assert!(sheds > 4, "the ensemble needs shed cases, got {sheds}/24");
    assert!(trips > 4, "the ensemble needs trip cases, got {trips}/24");
    assert!(
        unserved > 4,
        "the ensemble needs load-beyond-capability cases, got {unserved}/24"
    );
    // Every non-trip case must be island-capable: the battery's SOC above
    // its floor (0.1) and some discharge capability.
    for (index, c) in &cases {
        assert!(
            c.battery_soc > 0.1 && c.max_discharge_kw > 0.0,
            "case {index}: the ensemble draws island-capable batteries only"
        );
    }
}
