//! Integration tests for grid-outage and islanding semantics at the
//! dwelling level.
//!
//! `Dwelling::set_grid_voltage(0.0)` signals a **utility** outage. The
//! dwelling resolves bus energization at the start of each timestep:
//! - no island-capable source → the bus is dead
//!   (`GridState::bus_energized() == false`) and every load force-offs at
//!   the control level;
//! - an island-capable source (battery with usable charge, generator,
//!   discharging V2G EV) → the bus is held at nominal
//!   (`GridState::island_bus_voltage_pu == Some(1.0)`) and loads keep
//!   running — battery-backed homes must NOT drop their loads.
//!
//! See docs/outage-behavior.md.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use hares_core::ChaCha8Rng;
use hares_core::Dwelling;
use hares_core::actor::Actor;
use hares_core::actors::{EvDriverActor, EvDriverParams};
use hares_equipment::battery::Battery;
use hares_equipment::config::ConfigValue;
use hares_equipment::ev::Ev;
use hares_equipment::scheduled_load::ScheduledLoad;
use hares_equipment::{BatteryConfig, Equipment, EquipmentConfig, EvConfig};
use hares_types::telemetry_keys as tk;
use hares_types::{
    ChargingStrategy, ControlSignal, DRLevel, EndUse, EnvironmentState, PlugInPolicy,
    ScheduleSource, TelemetryField,
};
use rand::SeedableRng;

const FLAT_TARIFF: &str = include_str!("../../../tests/fixtures/golden/flat_tariff.json");

/// The test EV pack: 8 kWh, big enough to drain to its reserve inside a
/// minute-resolution window and small enough to fill from an island
/// surplus.
const EV_CAPACITY_J: f64 = 8.0 * 3.6e6;
/// The test battery pack (the shared battery helper's capacity).
const BATTERY_CAPACITY_J: f64 = 10.0 * 3.6e6;

/// A supplied no-op driver: it holds the `EvDriver:<equipment>` name so the
/// built-in driver seed builds nothing, and dispatches nothing: the
/// manual-control configuration an externally latched setpoint needs to
/// survive. (The auto-registered driver re-asserts a standing SOCTarget
/// every plugged-in step at the Schedule tier, and the EV's SOCTarget arm
/// clears a latched PowerSetpoint, so an external setpoint cannot coexist
/// with the driver's own vote; the tier fight is resolved by
/// configuration, never by signal priority.)
struct SilentEvDriver {
    name: String,
}

impl SilentEvDriver {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
        }
    }
}

impl Actor for SilentEvDriver {
    fn name(&self) -> &str {
        &self.name
    }

    fn output_variables(&self) -> &[TelemetryField] {
        &[]
    }

    fn decide(&mut self, _env: &EnvironmentState, _out: &mut Vec<hares_control::DispatchRequest>) {}
}

/// The driver the V2G scenario runs with: its own export vote is the
/// negative setpoint (price above the threshold, pack above the strategy
/// floor), dispatched as the composer's rate-of-record every plugged-in
/// step.
fn v2g_driver() -> EvDriverActor {
    EvDriverActor::new(
        "EvDriver:EV",
        "EV",
        EvDriverParams {
            strategy: ChargingStrategy::V2G {
                min_soc: 0.3,
                max_export_w: 5000.0,
                price_threshold: 0.10,
            },
            plug_in_policy: PlugInPolicy::Always,
            daily_drive_distance_m: ScheduleSource::Constant(0.0),
            departure_time: ScheduleSource::Constant(360.0),
            trip_duration: ScheduleSource::Constant(60.0),
            arrival_time: None,
            event_day_ratio: 0.0,
            fuel_economy_j_per_m: 180_000.0,
            capacity_j: EV_CAPACITY_J,
            max_charge_w: 11_500.0,
            average_speed_m_per_s: 15.0,
            range_anxiety_distance_m: 0.0,
            away_charge_fraction: 0.0,
            away_charge_power_w: 0.0,
        },
        ChaCha8Rng::seed_from_u64(42),
    )
}

/// One dwelling run's island-unserved warnings: built once, stepped once,
/// islanded with `set_grid_voltage(0.0)` and stepped three more times, the
/// finished run's `RunResult.warnings`.
fn islanded_run_warnings(tag: &str) -> usize {
    let mut dwelling = build_dwelling_with_base_load(tag);
    add_battery_with_discharge_limit(&mut dwelling, 0.6, 0.5);

    dwelling.step().expect("nominal step");
    dwelling.set_grid_voltage(0.0);
    dwelling.step().expect("islanded step");
    dwelling.step().expect("second islanded step");
    dwelling.step().expect("third islanded step");

    let result = dwelling.finish().expect("the run finishes");
    result
        .warnings
        .iter()
        .filter(|w| w.source.as_ref() == "island")
        .count()
}

fn write_minimal_toml(path: &Path) {
    write_minimal_toml_of_duration(path, 1200);
}

fn write_minimal_toml_of_duration(path: &Path, duration_s: u64) {
    let content = format!(
        r#"building_id = 9401

[simulation]
start_time = "2024-06-15T12:00:00Z"
time_res = 60
duration = {duration_s}
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
    fs::write(path, content).expect("failed to write synthetic TOML");
}

/// Dwelling with a 1.5 kW constant-power scheduled load.
fn build_dwelling_with_base_load(tag: &str) -> Dwelling {
    build_dwelling_with_base_load_for(tag, 1200)
}

/// Dwelling with a 1.5 kW constant-power scheduled load and an explicit
/// run duration (the EV scenarios drain and refill packs over many
/// minute steps).
fn build_dwelling_with_base_load_for(tag: &str, duration_s: u64) -> Dwelling {
    let dir = tempfile::tempdir().expect("temp dir");
    let toml_path = dir.path().join(format!("{tag}.toml"));
    write_minimal_toml_of_duration(&toml_path, duration_s);
    let mut dwelling = Dwelling::from_toml_config(&toml_path).expect("build dwelling");

    let env = dwelling.latest_env().clone();
    let mut raw: HashMap<String, ConfigValue> = HashMap::new();
    raw.insert("power_schedule_source".to_string(), "constant".into());
    raw.insert("power_constant_w".to_string(), 1500.0.into());
    raw.insert("sensible_gain_fraction".to_string(), 0.5.into());
    raw.insert("zone_id".to_string(), 1.0.into());
    let config = EquipmentConfig::raw("BaseLoad".to_string(), "ScheduledLoad".to_string(), raw);
    let mut eq = ScheduledLoad::new(config.clone(), EndUse::LIGHTING, "Lighting");
    eq.init(&config, &env).expect("init ScheduledLoad");
    dwelling
        .add_equipment(Box::new(eq))
        .expect("add_equipment must succeed");
    dwelling
}

/// A test EV: an 8 kWh pack with an 11.5 kW EVSE, at home, pack heaters
/// off, the V2L/V2G legs and initial SOC per the callers' scenario.
fn add_ev(
    dwelling: &mut Dwelling,
    initial_soc: f64,
    v2l: bool,
    v2g: bool,
    actors: Vec<Box<dyn Actor>>,
) {
    let cfg = EvConfig {
        equipment_id: None,
        capacity_j: EV_CAPACITY_J,
        charging_level: None,
        max_charging_power_w: 11_500.0,
        charging_efficiency: None,
        l1_current_a: None,
        l1_voltage_v: None,
        soc_max: None,
        initial_soc: Some(initial_soc),
        battery_temp_c: Some(20.0),
        min_charge_temp_c: None,
        full_power_temp_c: None,
        heater_power_w: Some(0.0),
        heater_threshold_c: None,
        thermal_mass_j_per_k: None,
        ua_w_per_k: None,
        n_series: None,
        n_parallel: None,
        cell_resistance_ohm: None,
        v2l_enabled: Some(v2l),
        v2l_soc_reserve: None,
        v2l_max_discharge_w: None,
        v2g_enabled: Some(v2g),
        v2g_soc_reserve: None,
        v2g_max_discharge_w: Some(5000.0),
        chemistry: None,
        fuel_economy_j_per_m: None,
        ready_soc: None,
        charging_strategy: None,
        plug_in_policy: None,
        power_limit_w: None,
        initial_connection_state: None,
        power_factor: None,
        charger_capacity_va: None,
        cc_cv_transition_soc: None,
        charging_priority: None,
        discharge_respects_deadline: true,
    };
    let config = EquipmentConfig::from_typed("EV".to_string(), "EV".to_string(), cfg).unwrap();
    let env = dwelling.latest_env().clone();
    let mut ev = Ev::new(config.clone());
    ev.init(&config, &env).expect("init EV");
    dwelling
        .add_equipment_with_actors(Box::new(ev), actors)
        .expect("add EV");
}

/// The named equipment's published `soc` channel (a fraction).
fn equipment_soc(dwelling: &Dwelling, name: &str) -> f64 {
    dwelling
        .latest_env()
        .equipment_telemetry
        .get(name)
        .and_then(|t| t.get(tk::SOC))
        .unwrap_or_else(|| panic!("the '{name}' equipment publishes soc"))
}

/// The named equipment's published active power (W, signed: positive
/// import, negative export).
fn equipment_power_w(dwelling: &Dwelling, name: &str) -> f64 {
    dwelling
        .latest_env()
        .equipment_telemetry
        .get(name)
        .and_then(|t| t.get(tk::ACTIVE_POWER_KW))
        .unwrap_or_else(|| panic!("the '{name}' equipment publishes active power"))
}

fn add_battery(dwelling: &mut Dwelling, initial_soc: f64) {
    add_battery_with_discharge_limit(dwelling, initial_soc, 5.0);
}

fn add_battery_with_discharge_limit(
    dwelling: &mut Dwelling,
    initial_soc: f64,
    max_discharge_w: f64,
) {
    let cfg = BatteryConfig {
        equipment_id: None,
        zone_id: None,
        capacity_j: 10.0 * 3.6e6,
        max_charge_w: 5000.0,
        max_discharge_w: max_discharge_w * 1000.0,
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
        initial_soc: Some(initial_soc),
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
    };
    let config =
        EquipmentConfig::from_typed("Battery".to_string(), "Battery".to_string(), cfg).unwrap();
    let env = dwelling.latest_env().clone();
    let mut bat = Battery::new(config.clone());
    bat.init(&config, &env).expect("init Battery");
    dwelling
        .add_equipment(Box::new(bat))
        .expect("add_equipment must succeed");
}

/// Utility outage without any backup source: the bus is dead and the
/// scheduled load force-offs — the meter reads (near) zero.
#[test]
fn outage_without_backup_deenergizes_bus_and_drops_loads() {
    let mut dwelling = build_dwelling_with_base_load("no-backup");

    // Nominal step: base load flows.
    dwelling.step().expect("nominal step");
    let summary = dwelling
        .save_checkpoint()
        .expect("checkpoint")
        .prior_electrical_summary;
    assert!(
        summary.base_load_w > 1.0,
        "baseline base load must flow, got {}",
        summary.base_load_w
    );

    // Utility outage.
    dwelling.set_grid_voltage(0.0);
    dwelling.step().expect("outage step");

    let grid = &dwelling.latest_env().grid;
    assert!(
        grid.grid_outage(),
        "voltage_pu 0.0 signals a utility outage"
    );
    assert!(
        !grid.bus_energized(),
        "no island source → the bus must be de-energized"
    );
    assert_eq!(grid.island_bus_voltage_pu, None);

    let summary = dwelling
        .save_checkpoint()
        .expect("checkpoint")
        .prior_electrical_summary;
    assert_eq!(
        summary.base_load_w, 0.0,
        "dead bus: the scheduled load must draw nothing"
    );
    assert_eq!(
        summary.net_grid_w, 0.0,
        "dead bus: the meter must read zero"
    );

    // Restoration: the load returns.
    dwelling.set_grid_voltage(1.0);
    dwelling.step().expect("restored step");
    let summary = dwelling
        .save_checkpoint()
        .expect("checkpoint")
        .prior_electrical_summary;
    assert!(
        summary.base_load_w > 1.0,
        "load must return after restoration, got {}",
        summary.base_load_w
    );
}

/// Utility outage with a charged battery: the dwelling islands (bus held at
/// nominal), loads keep running, and the battery discharges to serve them.
#[test]
fn outage_with_battery_islands_and_keeps_loads_running() {
    let mut dwelling = build_dwelling_with_base_load("battery-backup");
    add_battery(&mut dwelling, 0.6);

    dwelling.step().expect("nominal step");

    // Utility outage: the battery islands the home.
    dwelling.set_grid_voltage(0.0);
    dwelling.step().expect("islanded step");

    let grid = &dwelling.latest_env().grid;
    assert!(grid.grid_outage());
    assert!(
        grid.islanded(),
        "charged battery must island the home (bus energized at nominal)"
    );
    assert_eq!(grid.island_bus_voltage_pu, Some(1.0));

    dwelling.step().expect("second islanded step");
    let summary = dwelling
        .save_checkpoint()
        .expect("checkpoint")
        .prior_electrical_summary;
    assert!(
        summary.base_load_w > 1.0,
        "islanded home must NOT drop its loads, got {} kW",
        summary.base_load_w
    );
    assert!(
        summary.battery_power_w < 0.0,
        "battery must discharge to serve the islanded load, got {} kW",
        summary.battery_power_w
    );
}

/// An islanded home whose load exceeds the battery's discharge capability
/// reports the shortfall as `island_unserved_w` in `DwellingTelemetry` —
/// honest accounting of the island power-balance violation instead of
/// phantom grid import. Nominal (grid-connected) operation reports 0.0.
#[test]
fn islanded_load_beyond_discharge_capability_reports_unserved() {
    let mut dwelling = build_dwelling_with_base_load("unserved-load");
    // 0.5 kW max discharge against a 1.5 kW base load: the battery can
    // island the home but cannot fully serve it.
    add_battery_with_discharge_limit(&mut dwelling, 0.6, 0.5);

    // Nominal operation: grid-connected, no island accounting.
    dwelling.step().expect("nominal step");
    let telem = dwelling.telemetry().unwrap();
    assert_eq!(
        telem.island_unserved_w, 0.0,
        "island_unserved_w must be 0.0 during nominal (grid-connected) operation"
    );
    assert_eq!(
        telem.island_excess_w, 0.0,
        "island_excess_w must be 0.0 during nominal (grid-connected) operation"
    );

    // Utility outage: the battery islands the home but its 0.5 kW discharge
    // limit leaves ~1 kW of the base load unserved.
    dwelling.set_grid_voltage(0.0);
    dwelling.step().expect("islanded step");
    assert!(
        dwelling.latest_env().grid.islanded(),
        "battery with usable SOC must island the home"
    );

    dwelling.step().expect("second islanded step");
    let telem = dwelling.telemetry().unwrap();
    assert!(
        telem.island_unserved_w > 0.5,
        "load beyond max_discharge_w must be reported as unserved, got {} kW",
        telem.island_unserved_w
    );
    assert_eq!(
        telem.island_excess_w, 0.0,
        "no surplus generation in this island, got {} kW",
        telem.island_excess_w
    );
}

/// A depleted battery (SOC at its floor) cannot island the home: the bus is
/// dead and loads drop, exactly as with no battery at all.
#[test]
fn outage_with_depleted_battery_does_not_island() {
    let mut dwelling = build_dwelling_with_base_load("depleted-battery");
    add_battery(&mut dwelling, 0.1); // at min_soc floor

    dwelling.step().expect("nominal step");
    dwelling.set_grid_voltage(0.0);
    dwelling.step().expect("outage step");

    let grid = &dwelling.latest_env().grid;
    assert!(
        !grid.bus_energized(),
        "a battery at its SOC floor cannot island the home"
    );

    dwelling.step().expect("second outage step");
    let summary = dwelling
        .save_checkpoint()
        .expect("checkpoint")
        .prior_electrical_summary;
    assert_eq!(
        summary.base_load_w, 0.0,
        "dead bus: loads must drop with a depleted battery"
    );
}

/// The island-unserved warning is per run: two runs of the same inputs in
/// one process (the first simulated to its finish, the second a reset, a
/// fresh build from the same inputs as `reset_with_seed` builds from the
/// kept blueprint) each carry exactly one island-unserved `Warning` in
/// their `RunResult.warnings`. A warn-once state inherited across runs
/// would leave the second run's record without the warning its own
/// unserved steps raised.
#[test]
fn warn_once_is_per_run() {
    let first = islanded_run_warnings("warn-once-first");
    assert_eq!(first, 1, "the first run records its own unserved warning");

    let second = islanded_run_warnings("warn-once-reset");
    assert_eq!(
        second, 1,
        "the reset run's record is fresh: its own unserved steps warn again, \
         not the first run's consumed warn-once state"
    );
}

/// An EV parked at its V2L reserve with an externally latched export
/// setpoint cannot hold the bus: the commanded discharge delivers zero
/// watts (the floor curtails it), so the island source is phantom and the
/// bus stays dead. The silent driver holds the `EvDriver:EV` name so the
/// auto-registered driver builds nothing and the external setpoint
/// survives (the driver's standing SOCTarget would clear it).
#[test]
fn ev_at_discharge_floor_with_latched_export_does_not_hold_bus() {
    let mut dwelling = build_dwelling_with_base_load("ev-floor");
    add_ev(
        &mut dwelling,
        0.2,
        true,
        false,
        vec![Box::new(SilentEvDriver::new("EvDriver:EV"))],
    );

    // Latch the V2L export command; the pack sits exactly at its reserve.
    dwelling.apply_control(
        "EV",
        ControlSignal::PowerSetpoint {
            active_power_w: -3000.0,
            reactive_power_var: None,
            min_soc: None,
            max_soc: None,
        },
    );
    dwelling.step().expect("nominal step");
    let delivered = equipment_power_w(&dwelling, "EV");
    assert!(
        delivered.abs() < 1e-9,
        "the floor curtails the commanded export: zero watts, got {delivered} W"
    );

    dwelling.set_grid_voltage(0.0);
    dwelling.step().expect("outage step");
    let grid = &dwelling.latest_env().grid;
    assert!(grid.grid_outage());
    assert!(
        !grid.bus_energized(),
        "an EV delivering zero watts must not hold the bus energized"
    );
    assert_eq!(grid.island_bus_voltage_pu, None);

    dwelling.step().expect("second outage step");
    assert!(
        !dwelling.latest_env().grid.bus_energized(),
        "the phantom source never energizes the bus"
    );
}

/// A DR full shed zeroes the EV's commanded export the same way: the
/// bus stays dead because the vehicle delivers nothing.
#[test]
fn dr_shed_ev_does_not_island() {
    let mut dwelling = build_dwelling_with_base_load("ev-dr-shed");
    add_ev(
        &mut dwelling,
        0.5,
        false,
        true,
        vec![Box::new(SilentEvDriver::new("EvDriver:EV"))],
    );

    dwelling.apply_control(
        "EV",
        ControlSignal::PowerSetpoint {
            active_power_w: -3000.0,
            reactive_power_var: None,
            min_soc: None,
            max_soc: None,
        },
    );
    dwelling.apply_control(
        "EV",
        ControlSignal::DemandResponse {
            level: DRLevel::GridEmergency,
            duration_s: Some(7200.0),
        },
    );
    dwelling.step().expect("nominal step");
    let delivered = equipment_power_w(&dwelling, "EV");
    assert!(
        delivered.abs() < 1e-9,
        "a GridEmergency shed zeroes the commanded export, got {delivered} W"
    );

    dwelling.set_grid_voltage(0.0);
    dwelling.step().expect("outage step");
    let grid = &dwelling.latest_env().grid;
    assert!(
        !grid.bus_energized(),
        "a DR-shed EV delivers nothing and must not island the home"
    );
    assert_eq!(grid.island_bus_voltage_pu, None);
}

/// The driver's own V2G vote is the negative setpoint: the island lives
/// exactly as long as the vehicle delivers export. The pack drains to its
/// strategy floor during the outage; from the first step delivery stops,
/// the bus is dead. Every energized step is preceded by delivered watts.
#[test]
fn island_follows_ev_delivery_and_collapses_when_it_stops() {
    let mut dwelling = build_dwelling_with_base_load_for("ev-v2g-collapse", 3600);
    add_ev(
        &mut dwelling,
        0.45,
        false,
        true,
        vec![Box::new(v2g_driver())],
    );

    // The flat tariff prices 0.15 USD/kWh: above the driver's 0.10 export
    // threshold, so the V2G vote stands for every step of the window.
    let tariff: hares_tariff::ElectricTariff =
        serde_json::from_str(FLAT_TARIFF).expect("the flat tariff parses");
    dwelling
        .set_tariff(tariff, chrono_tz::America::Denver)
        .expect("the tariff attaches");

    dwelling.step().expect("nominal step");
    let mut prev_delivered = equipment_power_w(&dwelling, "EV");
    assert!(
        prev_delivered < 0.0,
        "the driver's V2G vote dispatches export from the first step, got {prev_delivered} W"
    );

    dwelling.set_grid_voltage(0.0);
    let mut energized_steps = 0;
    for _ in 0..50 {
        dwelling.step().expect("outage step");
        if dwelling.latest_env().grid.islanded() {
            energized_steps += 1;
            assert!(
                prev_delivered < -1e-9,
                "an energized bus requires the EV's own delivery the step before, \
                 got {prev_delivered} W"
            );
        }
        prev_delivered = equipment_power_w(&dwelling, "EV");
    }

    assert!(
        energized_steps > 0,
        "the scenario must island while the pack delivers"
    );
    assert_eq!(
        prev_delivered, 0.0,
        "the pack ends at its strategy floor: delivery stopped"
    );
    assert!(
        !dwelling.latest_env().grid.bus_energized(),
        "the island collapses when the delivery stops"
    );
}

/// The island conservation property over a battery+EV island: every
/// islanded step, neither pack gains more energy than the island's
/// sources delivered. The battery holds the bus (full, available) and its
/// discharge is the island's only source; the EV charges, clamped to the
/// visible surplus like the battery's own charge, so its pack gains
/// nothing the sources did not deliver. Under the unclamped EVSE the
/// vehicle credited its pack the full 11.5 kW rate while the island's
/// sources delivered the base load's share of it. A standalone PV joins
/// the equipment-level clamp test instead: the synthetic fixture
/// registers no PV orientation surfaces, so PV cannot attach to this
/// dwelling; the invariant is source-agnostic either way.
#[test]
fn islanded_pack_energy_is_bounded_by_island_sources() {
    let mut dwelling = build_dwelling_with_base_load_for("ev-charge-clamp", 3600);
    add_battery(&mut dwelling, 1.0);
    add_ev(&mut dwelling, 0.3, false, false, Vec::new());

    dwelling.step().expect("nominal step");
    dwelling.set_grid_voltage(0.0);

    let mut prev_ev_soc = equipment_soc(&dwelling, "EV");
    let mut prev_battery_soc = equipment_soc(&dwelling, "Battery");
    let mut energized_steps = 0;
    for _ in 0..50 {
        dwelling.step().expect("islanded step");
        let islanded = dwelling.latest_env().grid.islanded();
        let ev_soc = equipment_soc(&dwelling, "EV");
        let battery_soc = equipment_soc(&dwelling, "Battery");
        if islanded {
            energized_steps += 1;
            let summary = dwelling
                .save_checkpoint()
                .expect("checkpoint")
                .prior_electrical_summary;
            let dt_s = 60.0;
            let sources_j =
                (summary.pv_generation_w.max(0.0) + (-summary.battery_power_w).max(0.0)) * dt_s;
            let ev_gain_j = (ev_soc - prev_ev_soc) * EV_CAPACITY_J;
            let battery_gain_j = (battery_soc - prev_battery_soc) * BATTERY_CAPACITY_J;
            assert!(
                ev_gain_j <= sources_j + 1.0,
                "the EV pack gained {ev_gain_j} J in a step whose sources delivered {sources_j} J"
            );
            assert!(
                battery_gain_j <= sources_j + 1.0,
                "the battery pack gained {battery_gain_j} J in a step whose sources delivered {sources_j} J"
            );
        }
        prev_ev_soc = ev_soc;
        prev_battery_soc = battery_soc;
    }
    assert!(
        energized_steps > 0,
        "the scenario must island: the full battery is available as the source"
    );
}
