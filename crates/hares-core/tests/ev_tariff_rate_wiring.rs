//! The EV sub-meter reaches the dwelling-level bill: a tariff with an
//! `ev_tou_period_name` bills the EV's charging share at that period's
//! rate, so the bill moves against the same run without the EV rate. The
//! EV's signed flow reaches the evaluator's `step` from the dwelling's
//! per-end-use flow sum. Before the wiring it was hardcoded to zero and
//! every EV rider silently billed at the house rate.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use hares_core::Dwelling;
use hares_equipment::config::ConfigValue;
use hares_equipment::ev::Ev;
use hares_equipment::scheduled_load::ScheduledLoad;
use hares_equipment::{Equipment, EquipmentConfig, EvConfig};
use hares_tariff::{ElectricTariff, EnergyRate, ExportRate, FixedCharges};
use hares_types::{DayFilter, EndUse, SeasonFilter, TimeWindow, TouPeriod};

const EV_CAPACITY_J: f64 = 8.0 * 3.6e6;

fn write_minimal_toml(path: &Path) {
    let content = r#"building_id = 9401

[simulation]
start_time = "2024-06-15T12:00:00Z"
time_res = 60
duration = 7200
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
    fs::write(path, content).expect("failed to write synthetic TOML");
}

/// The dwelling: a 1.5 kW house load and an EV that charges the whole
/// window (the auto-registered driver's Immediate strategy targets full,
/// the pack starts at 0.3).
fn build_dwelling(tag: &str) -> Dwelling {
    let dir = tempfile::tempdir().expect("temp dir");
    let toml_path = dir.path().join(format!("{tag}.toml"));
    write_minimal_toml(&toml_path);
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

    let ev_cfg = EvConfig {
        equipment_id: None,
        capacity_j: EV_CAPACITY_J,
        charging_level: None,
        max_charging_power_w: 11_500.0,
        charging_efficiency: None,
        l1_current_a: None,
        l1_voltage_v: None,
        soc_max: None,
        initial_soc: Some(0.3),
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
        v2l_enabled: None,
        v2l_soc_reserve: None,
        v2l_max_discharge_w: None,
        v2g_enabled: None,
        v2g_soc_reserve: None,
        v2g_max_discharge_w: None,
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
    let ev_config =
        EquipmentConfig::from_typed("EV".to_string(), "EV".to_string(), ev_cfg).unwrap();
    let env = dwelling.latest_env().clone();
    let mut ev = Ev::new(ev_config.clone());
    ev.init(&ev_config, &env).expect("init EV");
    dwelling
        .add_equipment(Box::new(ev))
        .expect("add EV must succeed");
    dwelling
}

/// The test tariff: a flat 0.15 USD/kWh house rate covering 24x7, plus a
/// rate-carrier period for the EV sub-meter at 0.05. The carrier period
/// holds no windows of its own, so it never matches an hour for the house
/// price; the EV price resolves by the period's name alone.
fn tariff_with_ev_rate(ev_tou_period_name: Option<&str>) -> ElectricTariff {
    ElectricTariff {
        name: Some("ev-rate-wiring".into()),
        tou_schedule: vec![
            TouPeriod {
                name: "flat".into(),
                schedule: vec![TimeWindow::new(DayFilter::Any, 0, 1440, 0.0)],
                season: SeasonFilter::All,
            },
            TouPeriod {
                name: "ev".into(),
                schedule: Vec::new(),
                season: SeasonFilter::All,
            },
        ],
        energy_rates: vec![
            EnergyRate {
                period_name: "flat".into(),
                season: SeasonFilter::All,
                rate_per_kwh: 0.15,
            },
            EnergyRate {
                period_name: "ev".into(),
                season: SeasonFilter::All,
                rate_per_kwh: 0.05,
            },
        ],
        export_rate: ExportRate::default(),
        fixed_charges: FixedCharges {
            monthly_usd: 0.0,
            daily_usd: 0.0,
        },
        ev_tou_period_name: ev_tou_period_name.map(str::to_string),
        ..Default::default()
    }
}

/// The run's EV charging energy (kWh), summed from the equipment's
/// published per-step power.
fn ev_charged_kwh(dwelling: &mut Dwelling, steps: usize) -> f64 {
    let dt_s = dwelling.latest_env().time_step_secs();
    let mut joules = 0.0;
    for _ in 0..steps {
        dwelling.step().expect("the wired run steps");
        let power_w = dwelling
            .latest_env()
            .equipment_telemetry
            .get("EV")
            .and_then(|t| t.get("active_power_kw"))
            .expect("the EV publishes active power");
        joules += power_w.max(0.0) * dt_s;
    }
    joules / 3.6e6
}

/// A configured `ev_tou_period_name` changes the dwelling-level bill: the
/// same run with the EV rate bills the EV's kWh at 0.05 instead of the
/// 0.15 house rate, the house's own kWh are unaffected, and the saving is
/// exactly the EV energy times the rate difference.
#[test]
fn ev_tou_period_name_changes_the_dwelling_bill() {
    let steps = 120; // 2 h at 60 s
    let run = |ev_rate: Option<&str>, tag: &str| {
        let mut dwelling = build_dwelling(tag);
        dwelling
            .set_tariff(tariff_with_ev_rate(ev_rate), chrono_tz::Etc::UTC)
            .expect("the tariff attaches");
        let ev_kwh = ev_charged_kwh(&mut dwelling, steps);
        dwelling.finalize_billing();
        let summaries = dwelling.billing_summaries();
        assert_eq!(summaries.len(), 1, "the finalize closes the run's period");
        (
            ev_kwh,
            summaries[0].energy_charge_usd,
            summaries[0].total_import_kwh,
        )
    };

    let (ev_kwh, house_bill, house_import) = run(None, "ev-rate-house");
    let (ev_kwh_wired, ev_bill, ev_import) = run(Some("ev"), "ev-rate-wired");

    assert!(
        ev_kwh > 0.5,
        "the EV must charge in the window: {ev_kwh} kWh"
    );
    assert_eq!(
        house_import, ev_import,
        "the runs charge the EV identically: the same total import, on bits"
    );
    assert!(
        ev_bill < house_bill,
        "the EV rate (0.05) must undercut the house rate (0.15): {ev_bill} vs {house_bill}"
    );
    let expected_delta = ev_kwh_wired * (0.15 - 0.05);
    let actual_delta = house_bill - ev_bill;
    assert!(
        (actual_delta - expected_delta).abs() < 1e-6,
        "the bill saving must be the EV energy times the rate difference: \
         {actual_delta} vs {expected_delta}"
    );
}
