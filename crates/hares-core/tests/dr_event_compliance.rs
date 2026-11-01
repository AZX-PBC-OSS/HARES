//! The DR event's step-scoped compliance.
//!
//! A `DemandResponse` event commands a level and a wall-clock duration;
//! the classes that keep a countdown shed every step whose interval
//! overlaps the window, the countdown advances once per step at the
//! equipment's own on/off decision, and the release lands on the first
//! control pass ahead of the solve. The compliance pins, across the
//! timestep resolutions the fleet runs:
//!
//! 1. The event sheds exactly the steps whose intervals overlap the
//!    duration: at 60, 300 and 900 s a 720 s event covers 12, 3 and 1
//!    steps.
//! 2. The compliance is event-scoped: the published DR fraction reads
//!    released (1.0) before the event, shed (0.0) for every event step,
//!    and released again from the first step past the window; nothing
//!    outside the event's steps moves.
//! 3. The release lands ahead of the solve: the first post-event step
//!    dispatches the equipment's own control again (the battery resumes
//!    serving the load), the step after the last shed one.

use std::fs;
use std::path::Path;

use hares_core::Dwelling;
use hares_equipment::battery::Battery;
use hares_equipment::config::ConfigValue;
use hares_equipment::scheduled_load::ScheduledLoad;
use hares_equipment::{BatteryConfig, Equipment, EquipmentConfig};
use hares_types::EndUse;
use hares_types::telemetry_keys as tk;

const BATTERY_CAPACITY_J: f64 = 10.0 * 3.6e6;

/// The synthetic dwelling: no HVAC, a constant-weather fixture, the
/// battery at a mid SOC with more capability than the load, one zone. The
/// 3-hour horizon leaves every resolution enough steps for the event's
/// window plus its release tail.
fn write_minimal_toml(path: &Path, tag: &str, time_res_s: u64) {
    let content = format!(
        r#"building_id = 9401

[simulation]
start_time = "2024-06-15T12:00:00Z"
time_res = {time_res_s}
duration = 10800
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
"#
    );
    let dir = path.join(tag);
    fs::create_dir_all(&dir).expect("the case directory creates");
    fs::write(dir.join("case.toml"), content).expect("failed to write synthetic TOML");
}

/// The dwelling: a 1.5 kW constant-power load and a battery that can serve
/// it (2 kW discharge capability, mid SOC).
fn build_dwelling(scratch: &Path, tag: &str, time_res_s: u64) -> Dwelling {
    write_minimal_toml(scratch, tag, time_res_s);
    let toml_path = scratch.join(tag).join("case.toml");
    let mut dwelling = Dwelling::from_toml_config(&toml_path).expect("build dwelling");

    let env = dwelling.latest_env().clone();
    let mut raw: std::collections::HashMap<String, ConfigValue> = std::collections::HashMap::new();
    raw.insert("power_schedule_source".to_string(), "constant".into());
    raw.insert("power_constant_w".to_string(), 1500.0.into());
    raw.insert("sensible_gain_fraction".to_string(), 0.5.into());
    raw.insert("zone_id".to_string(), 1.0.into());
    let config = EquipmentConfig::raw("BaseLoad".to_string(), "ScheduledLoad".to_string(), raw);
    let mut eq = ScheduledLoad::new(config.clone(), EndUse::LIGHTING, "Lighting");
    eq.init(&config, &env).expect("init ScheduledLoad");
    let equipment_id = eq.descriptor().id;
    dwelling
        .add_equipment(Box::new(eq))
        .expect("add_equipment must succeed");
    let _ = equipment_id;

    let battery_config = BatteryConfig {
        equipment_id: None,
        zone_id: None,
        capacity_j: BATTERY_CAPACITY_J,
        max_charge_w: 5000.0,
        max_discharge_w: 2000.0,
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
        initial_soc: Some(0.6),
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
        // The battery's own control: self-consumption (no PV: it serves
        // the home's import), so the release's "dispatches again" has an
        // observable delivery. The config carries the mode as its JSON
        // form, parsed at init.
        bms_mode: Some(
            serde_json::to_string(&hares_types::BmsMode::SelfConsumption {
                min_soc: 0.1,
                max_soc: 1.0,
                solar_only_charging: false,
                surplus_deadband_w: 0.0,
            })
            .expect("the BMS mode serializes"),
        ),
        grid_export_rule: None,
        power_factor: None,
        inverter_capacity_va: None,
        grid_forming: None,
        min_dwell_steps: 0,
        der_1547: None,
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

/// The battery's published DR fraction (1.0 released, 0.0 shed).
fn dr_fraction(dwelling: &Dwelling) -> f64 {
    dwelling
        .latest_env()
        .equipment_telemetry
        .get("Battery")
        .and_then(|t| t.get(tk::DR_POWER_FRACTION))
        .unwrap_or_else(|| panic!("the battery publishes its DR fraction"))
}

/// One resolution's compliance run: the number of shed steps for a 720 s
/// event must equal the duration's ceiling in steps, and the fraction must
/// read released-shed-released in sequence.
#[test]
fn the_event_sheds_exactly_the_steps_its_window_overlaps() {
    let scratch = tempfile::tempdir().expect("scratch directory");
    let duration_s = 720.0_f64;

    for time_res_s in [60_u64, 300, 900] {
        let mut dwelling = build_dwelling(scratch.path(), &format!("res-{time_res_s}"), time_res_s);

        // Two nominal steps: released, released.
        dwelling.step().expect("the first nominal step");
        dwelling.step().expect("the second nominal step");
        assert_eq!(
            dr_fraction(&dwelling),
            1.0,
            "res {time_res_s}: the battery reads released before the event"
        );

        // Arm the event; the next step is the first whose interval
        // overlaps the window.
        dwelling.apply_control(
            "Battery",
            hares_types::ControlSignal::DemandResponse {
                level: hares_types::DRLevel::GridEmergency,
                duration_s: Some(duration_s),
            },
        );

        let expected_shed_steps = (duration_s / time_res_s as f64).ceil() as usize;
        let mut shed_steps = 0;
        for step in 0..(expected_shed_steps + 3) {
            dwelling.step().expect("the event's step");
            let fraction = dr_fraction(&dwelling);
            if step < expected_shed_steps {
                assert_eq!(
                    fraction, 0.0,
                    "res {time_res_s} step {step}: the event's window overlaps \
                     this step: the battery must read shed"
                );
                shed_steps += 1;
            } else {
                assert_eq!(
                    fraction, 1.0,
                    "res {time_res_s} step {step}: past the window the \
                     compliance must release"
                );
            }
        }
        assert_eq!(
            shed_steps, expected_shed_steps,
            "res {time_res_s}: a {duration_s} s event sheds exactly its \
             window's steps"
        );

        // The release lands ahead of the solve: the first post-event step
        // dispatches the battery's own control again (it serves the 1.5 kW
        // load), the release not delayed a step.
        let summary = dwelling
            .save_checkpoint()
            .expect("checkpoint")
            .prior_electrical_summary;
        assert!(
            summary.battery_power_w < -1000.0,
            "res {time_res_s}: after the release the battery serves the load \
             again on its own solve, got {} W",
            summary.battery_power_w
        );
    }
}
