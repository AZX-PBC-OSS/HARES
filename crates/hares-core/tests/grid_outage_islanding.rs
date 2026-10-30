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
    dwelling
        .set_grid_voltage(0.0)
        .expect("the override is in contract");
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
        der_1547: None,
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
        der_1547: None,
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
    dwelling
        .set_grid_voltage(0.0)
        .expect("the override is in contract");
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
    dwelling
        .set_grid_voltage(1.0)
        .expect("the override is in contract");
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

/// Utility outage with a charged battery: the dwelling islands one step
/// after the outage (the formation condition is end-of-previous-step
/// state, the documented lag), loads keep running from then on, and the
/// battery discharges to serve them.
#[test]
fn outage_with_battery_islands_and_keeps_loads_running() {
    let mut dwelling = build_dwelling_with_base_load("battery-backup");
    add_battery(&mut dwelling, 0.6);

    dwelling.step().expect("nominal step");

    // Utility outage: the battery islands the home one step later.
    dwelling
        .set_grid_voltage(0.0)
        .expect("the override is in contract");
    dwelling.step().expect("the outage's dead-bus step");
    let grid = &dwelling.latest_env().grid;
    assert!(grid.grid_outage());
    assert!(
        !grid.islanded(),
        "the outage's first step is a dead-bus step (the documented \
         formation lag)"
    );
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

    // Utility outage: the battery islands the home one step later (the
    // documented formation lag), but its 0.5 kW discharge limit leaves
    // ~1 kW of the base load unserved.
    dwelling
        .set_grid_voltage(0.0)
        .expect("the override is in contract");
    dwelling.step().expect("the outage's dead-bus step");
    assert!(
        !dwelling.latest_env().grid.islanded(),
        "the outage's first step is a dead-bus step"
    );

    dwelling.step().expect("islanded step");
    assert!(
        dwelling.latest_env().grid.islanded(),
        "battery with usable SOC must island the home one step after the outage"
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
    dwelling
        .set_grid_voltage(0.0)
        .expect("the override is in contract");
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

    dwelling
        .set_grid_voltage(0.0)
        .expect("the override is in contract");
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

    dwelling
        .set_grid_voltage(0.0)
        .expect("the override is in contract");
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

    // The V2G tariff prices 0.15 USD/kWh on BOTH legs: above the driver's
    // 0.10 export threshold, so the V2G vote stands for every step of the
    // window. The export leg is what the vote prices - a tariff crediting
    // exports nothing correctly leaves the vote idle.
    let v2g_tariff = FLAT_TARIFF.replace(r#""mode": "None""#, r#""mode": {"FlatRate": 0.15}"#);
    let tariff: hares_tariff::ElectricTariff =
        serde_json::from_str(&v2g_tariff).expect("the v2g tariff parses");
    dwelling
        .set_tariff(tariff, chrono_tz::America::Denver)
        .expect("the tariff attaches");

    dwelling.step().expect("nominal step");
    let mut prev_delivered = equipment_power_w(&dwelling, "EV");
    assert!(
        prev_delivered < 0.0,
        "the driver's V2G vote dispatches export from the first step, got {prev_delivered} W"
    );

    dwelling
        .set_grid_voltage(0.0)
        .expect("the override is in contract");
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
    dwelling
        .set_grid_voltage(0.0)
        .expect("the override is in contract");

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

/// The island forms one step AFTER the outage begins: the formation
/// condition reads the previous step's outage and the source
/// availability (both end-of-previous-step state) beside the step's own
/// utility voltage, so the outage's first step is a dead-bus step (every
/// load force-offs) and the second step is islanded. The documented
/// one-step lag (docs/cosim.md, docs/outage-behavior.md).
#[test]
fn island_forms_one_step_after_the_outage() {
    let mut dwelling = build_dwelling_with_base_load("formation-lag");
    add_battery(&mut dwelling, 0.6);

    dwelling.step().expect("nominal step");
    dwelling
        .set_grid_voltage(0.0)
        .expect("the override is in contract");

    // The outage's first step: the island has not formed yet.
    dwelling.step().expect("the outage's first step");
    let grid = &dwelling.latest_env().grid;
    assert!(grid.grid_outage());
    assert!(
        !grid.islanded(),
        "the outage's first step is a dead-bus step: the island's \
         formation takes effect with a one-step lag"
    );
    assert_eq!(grid.island_bus_voltage_pu, None);
    let summary = dwelling
        .save_checkpoint()
        .expect("checkpoint")
        .prior_electrical_summary;
    assert_eq!(
        summary.base_load_w, 0.0,
        "the dead-bus step force-offs every load"
    );

    // The second step: the island forms and the loads return.
    dwelling.step().expect("the island's formation step");
    let grid = &dwelling.latest_env().grid;
    assert!(
        grid.islanded(),
        "an available battery islands the home one step after the outage"
    );
    assert_eq!(grid.island_bus_voltage_pu, Some(1.0));
    let summary = dwelling
        .save_checkpoint()
        .expect("checkpoint")
        .prior_electrical_summary;
    assert!(
        summary.base_load_w > 1.0,
        "the islanded home runs its loads, got {} W",
        summary.base_load_w
    );
}

/// A full DR shed of the battery does not collapse a formed island: the
/// DR is a grid-supporting curtailment of grid interchange, and an
/// island has none. The island stays formed (capability, not dispatch,
/// holds the bus), the battery keeps serving the home through the event,
/// and the DR's shed resumes when the utility returns.
#[test]
fn dr_shed_during_island_keeps_the_bus_alive() {
    let mut dwelling = build_dwelling_with_base_load("island-dr-shed");
    add_battery(&mut dwelling, 0.6);

    dwelling.step().expect("nominal step");
    dwelling
        .set_grid_voltage(0.0)
        .expect("the override is in contract");
    dwelling.step().expect("the outage's dead-bus step");
    dwelling.step().expect("the island's formation step");
    assert!(dwelling.latest_env().grid.islanded(), "the island formed");

    // The full shed, mid-island: ten steps, so the event still runs when
    // the utility returns and the resumed shed is observable.
    dwelling.apply_control(
        "Battery",
        ControlSignal::DemandResponse {
            level: DRLevel::GridEmergency,
            duration_s: Some(10.0 * 60.0),
        },
    );
    for step in 0..4 {
        dwelling.step().expect("the island's shed step");
        let grid = &dwelling.latest_env().grid;
        assert!(
            grid.islanded(),
            "step {step}: a DR shed must not collapse a formed island"
        );
        assert_eq!(
            grid.island_bus_voltage_pu,
            Some(1.0),
            "step {step}: the island's bus stays at nominal through the shed"
        );
        let battery_dr = dwelling
            .latest_env()
            .equipment_telemetry
            .get("Battery")
            .and_then(|t| t.get(tk::DR_POWER_FRACTION))
            .unwrap_or_else(|| panic!("step {step}: the battery publishes its DR fraction"));
        assert_eq!(
            battery_dr, 0.0,
            "step {step}: the event is in force (the arrange must be \
             load-bearing: a run without the signal would pass every other \
             assertion in this test)"
        );
        let summary = dwelling
            .save_checkpoint()
            .expect("checkpoint")
            .prior_electrical_summary;
        assert!(
            summary.base_load_w > 1.0,
            "step {step}: the islanded home keeps its loads through a DR \
             shed, got {} W",
            summary.base_load_w
        );
        assert!(
            summary.battery_power_w < 0.0,
            "step {step}: the battery keeps serving the islanded load through \
             the shed, got {} W",
            summary.battery_power_w
        );
        let telem = dwelling.telemetry().expect("telemetry");
        assert!(
            telem.island_unserved_w <= 50.0,
            "step {step}: the island's dispatch is uncurtailed: any imbalance \
             is the standby-draw accounting noise below the documented 0.05 kW \
             warning threshold, got {} W",
            telem.island_unserved_w
        );
    }

    // The utility returns while the event still runs: the island collapses,
    // and the shed it exists to curtail resumes on the grid-connected
    // dispatch. The battery's own control would serve the 1.5 kW base load;
    // the event holds it at standby and the load imports instead.
    dwelling
        .set_grid_voltage(1.0)
        .expect("the restoration override is in contract");
    dwelling.step().expect("the restored step");
    assert!(
        !dwelling.latest_env().grid.islanded(),
        "the restoration collapses the island"
    );
    let summary = dwelling
        .save_checkpoint()
        .expect("checkpoint")
        .prior_electrical_summary;
    assert!(
        summary.battery_power_w.abs() <= 50.0,
        "the DR shed resumes on the restored grid: the battery must not serve \
         the load while the event holds it at standby, got {} W",
        summary.battery_power_w
    );
    assert!(
        summary.base_load_w > 1.0 && summary.net_grid_w > 1.0,
        "the restored home draws its load from the grid while the battery is \
         shed: base load {} W, meter {} W",
        summary.base_load_w,
        summary.net_grid_w
    );

    // The event ends; the battery's self-consumption resumes.
    for _ in 0..7 {
        dwelling.step().expect("the post-event step");
    }
    let summary = dwelling
        .save_checkpoint()
        .expect("checkpoint")
        .prior_electrical_summary;
    assert!(
        summary.battery_power_w < 0.0,
        "after the event the battery serves the load again, got {} W",
        summary.battery_power_w
    );
}

/// The island's survival through a DR shed is a capability boundary, not
/// a headroom one: a battery whose discharge limit exactly equals the
/// island's load still holds the bus and serves the load while a full
/// shed holds its grid-side dispatch at standby. Source availability keys
/// on capacity (grid-forming, dischargeable above the floor), never on
/// surplus over the load, and the islanded dispatch is uncurtailed.
#[test]
fn dr_shed_island_survives_at_the_source_capacity_edge() {
    let mut dwelling = build_dwelling_with_base_load("island-dr-edge");
    // The 1.5 kW base load exactly: zero discharge headroom.
    add_battery_with_discharge_limit(&mut dwelling, 0.6, 1.5);

    dwelling.step().expect("nominal step");
    dwelling
        .set_grid_voltage(0.0)
        .expect("the override is in contract");
    dwelling.step().expect("the outage's dead-bus step");
    dwelling.step().expect("the island's formation step");
    assert!(dwelling.latest_env().grid.islanded(), "the island formed");

    dwelling.apply_control(
        "Battery",
        ControlSignal::DemandResponse {
            level: DRLevel::GridEmergency,
            // Four steps at the fixture's 60 s timestep.
            duration_s: Some(4.0 * 60.0),
        },
    );
    for step in 0..4 {
        dwelling.step().expect("the island's shed step");
        assert!(
            dwelling.latest_env().grid.islanded(),
            "step {step}: the island survives at the source-capacity edge"
        );
        let battery_dr = dwelling
            .latest_env()
            .equipment_telemetry
            .get("Battery")
            .and_then(|t| t.get(tk::DR_POWER_FRACTION))
            .unwrap_or_else(|| panic!("step {step}: the battery publishes its DR fraction"));
        assert_eq!(
            battery_dr, 0.0,
            "step {step}: the event is in force (without it the battery \
             would serve the load anyway and the edge would prove nothing)"
        );
        let summary = dwelling
            .save_checkpoint()
            .expect("checkpoint")
            .prior_electrical_summary;
        // The pack discharges at its 1.5 kW limit less its 5 W standby
        // draw: the load equals the capability exactly, so the residual
        // imbalance is the standby alone.
        assert!(
            summary.battery_power_w < -1_400.0,
            "step {step}: the battery serves the 1.5 kW load at its rated \
             discharge edge, got {} W",
            summary.battery_power_w
        );
        let telem = dwelling.telemetry().expect("telemetry");
        assert!(
            telem.island_unserved_w <= 50.0,
            "step {step}: the load exactly at the source's capacity is \
             served (the only imbalance is the pack's standby draw), got \
             {} W unserved",
            telem.island_unserved_w
        );
    }
}
