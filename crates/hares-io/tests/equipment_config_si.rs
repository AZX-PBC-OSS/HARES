//! The equipment config fields are stored in SI: the acceptance walk over
//! the config types themselves. The test builds an instance of every
//! config type in the unit-move's table that lives below `hares-core`,
//! with every optional field set and, for the JSON-string payload fields
//! (`bms_mode`, `charging_strategy`), once per payload variant; it then
//! serialises the config, parses the string payloads and walks every key
//! path: no key matches `(^|_)(kw|kwh|kva|mi|miles|mph)(_|$)`.
//!
//! It calls nothing in `hares-core` (which `hares-io` does not depend on)
//! and sees neither `BatterySpec`/`VehicleSpec` (datasheet units are a
//! boundary that keeps them) nor the run state (the run-state commit
//! moves it).

use hares_equipment::BatteryConfig;
use hares_equipment::EvConfig;
use hares_equipment::GeneratorConfig;
use hares_equipment::PvConfig;
use hares_equipment::hvac::CentralAirConditionerConfig;
use hares_equipment::hvac::HeatPumpCoolerConfig;
use hares_equipment::hvac::RoomAcConfig;
use hares_equipment::pv::PvArraySpec;
use hares_types::BmsMode;
use hares_types::ChargingStrategy;

/// Whether a key's segment is a non-SI unit segment: a whole `kw`, `kwh`,
/// `kva`, `mi`, `miles` or `mph` token, at the segment's edges or alone
/// (case-insensitive), the spellings the configs' old names used.
fn is_non_si_segment(segment: &str) -> bool {
    let lower = segment.to_ascii_lowercase();
    let units = ["kw", "kwh", "kva", "mi", "miles", "mph"];
    units.iter().any(|unit| {
        lower == *unit
            || lower.ends_with(&format!("_{unit}"))
            || lower.starts_with(&format!("{unit}_"))
            || lower.contains(&format!("_{unit}_"))
    })
}

/// Every key path of a serialised config, the JSON-string payload fields
/// parsed and walked under their variant
/// (`BatteryConfig.bms_mode.SelfConsumption.surplus_deadband_w`). Values
/// are not walked; a payload value that carried a nested object is
/// recursed.
fn key_paths(value: &serde_json::Value, prefix: &str, out: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                key_paths(child, &path, out);
            }
        }
        // A JSON-string payload: parse and walk under the field's path.
        serde_json::Value::String(text)
            if prefix.ends_with("bms_mode") || prefix.ends_with("charging_strategy") =>
        {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(text) {
                key_paths(&parsed, prefix, out);
            }
        }
        serde_json::Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                key_paths(child, &format!("{prefix}[{index}]"), out);
            }
        }
        _ => out.push(prefix.to_string()),
    }
}

/// Serialises the config, parses the string payloads and fails when any
/// key path carries a non-SI unit segment.
fn assert_no_non_si_key(config: &str, label: &str) {
    let value: serde_json::Value =
        serde_json::from_str(config).unwrap_or_else(|err| panic!("{label}: {err}"));
    let mut paths = Vec::new();
    key_paths(&value, "", &mut paths);
    for path in paths {
        let name_only = path.replace('[', ".").replace(']', "");
        for segment in name_only.split('.') {
            assert!(
                !is_non_si_segment(segment),
                "{label}: key {path} carries the non-SI unit segment {segment}"
            );
        }
    }
}

fn full_pv_config() -> PvConfig {
    PvConfig {
        equipment_id: Some(1),
        zone_id: Some(1),
        capacity_w: 5000.0,
        tilt_deg: Some(30.0),
        azimuth_deg: Some(180.0),
        attached_boundary_id: None,
        module_type: Some("Standard".to_string()),
        noct_c: Some(45.0),
        array_type: Some("OpenRack".to_string()),
        system_losses_fraction: Some(0.14),
        inverter_efficiency: Some(0.96),
        inverter_capacity_w: Some(5000.0),
        power_factor: Some(1.0),
        surface_resolution_deg: Some(360.0),
        sam_lut_path: None,
        soiling: None,
        arrays: Some(vec![PvArraySpec {
            capacity_w: 3000.0,
            tilt_deg: Some(30.0),
            azimuth_deg: Some(180.0),
            module_type: None,
            noct_c: None,
            array_type: None,
            sam_lut_path: None,
            attached_boundary_id: Some(2),
        }]),
        der_1547: None,
    }
}

fn full_battery_config(bms_mode: Option<String>) -> BatteryConfig {
    BatteryConfig {
        equipment_id: Some(1),
        zone_id: Some(1),
        capacity_j: 13.5 * 3.6e6,
        max_charge_w: 5000.0,
        max_discharge_w: 5000.0,
        n_series: Some(16),
        n_parallel: Some(2),
        ah_cell: Some(100.0),
        v_cell: Some(3.2),
        cell_resistance_ohm: Some(0.005),
        pack_voltage_v: Some(51.2),
        chemistry: Some("nmc".to_string()),
        standby_power_w: Some(10.0),
        self_discharge_pct_per_day: Some(0.1),
        min_soc: Some(0.1),
        max_soc: Some(0.95),
        initial_soc: Some(0.5),
        initial_cell_temp_c: Some(20.0),
        import_limit_w: Some(6000.0),
        export_limit_w: Some(6000.0),
        heater_power_w: Some(100.0),
        heater_threshold_c: Some(5.0),
        min_discharge_temp_c: Some(-10.0),
        full_power_temp_c: Some(10.0),
        min_charge_temp_c: Some(0.0),
        cell_thermal_mass_j_per_k: Some(100.0),
        cell_ua_w_per_k: Some(2.0),
        inverter_efficiency: Some(0.97),
        charge_efficiency: Some(0.98),
        discharge_efficiency: Some(0.98),
        bms_mode,
        grid_export_rule: None,
        power_factor: Some(1.0),
        inverter_capacity_va: Some(5000.0),
        grid_forming: Some(true),
        min_dwell_steps: 0,
        der_1547: None,
    }
}

fn full_ev_config(strategy: Option<String>) -> EvConfig {
    EvConfig {
        equipment_id: Some(1),
        capacity_j: 75.0 * 3.6e6,
        charging_level: Some("L2".to_string()),
        max_charging_power_w: 11500.0,
        charging_efficiency: Some(0.9),
        l1_current_a: Some(12.0),
        l1_voltage_v: Some(120.0),
        soc_max: Some(1.0),
        initial_soc: Some(0.5),
        battery_temp_c: Some(20.0),
        min_charge_temp_c: Some(0.0),
        full_power_temp_c: Some(10.0),
        heater_power_w: Some(100.0),
        heater_threshold_c: Some(5.0),
        thermal_mass_j_per_k: Some(1000.0),
        ua_w_per_k: Some(5.0),
        n_series: Some(96),
        n_parallel: Some(43),
        cell_resistance_ohm: Some(0.005),
        v2l_enabled: Some(true),
        v2l_soc_reserve: Some(0.2),
        v2l_max_discharge_w: Some(3000.0),
        v2g_enabled: Some(true),
        v2g_soc_reserve: Some(0.3),
        v2g_max_discharge_w: Some(5000.0),
        chemistry: Some("nmc".to_string()),
        fuel_economy_j_per_m: Some(0.3 * 3.6e6 / 1609.344),
        ready_soc: Some(0.8),
        charging_strategy: strategy,
        plug_in_policy: None,
        power_limit_w: Some(11000.0),
        initial_connection_state: None,
        power_factor: Some(1.0),
        charger_capacity_va: Some(11500.0),
        cc_cv_transition_soc: Some(0.85),
        charging_priority: None,
        discharge_respects_deadline: true,
        der_1547: None,
    }
}

fn full_generator_config() -> GeneratorConfig {
    GeneratorConfig {
        equipment_id: Some(1),
        zone_id: Some(1),
        fuel_type: Some(hares_types::FuelType::Gas),
        rated_power_w: 10000.0,
        eta_electric: Some(0.3),
        eta_thermal: Some(0.6),
        eta_jacket_water: None,
        eta_lube_oil: None,
        eta_exhaust: None,
        efficiency_type: Some("constant".to_string()),
        efficiency_curve_points: None,
        delta_w_per_s: Some(1000.0),
        capacity_min_w: Some(1000.0),
        grid_import_limit_w: Some(15000.0),
        export_limit_w: Some(10000.0),
        loop_id: None,
        flow_rate_kg_s: None,
        supply_temp_c: None,
        return_temp_c: None,
        inverter_efficiency: None,
        stack_temp_c: None,
        stack_cooler_r0: None,
        stack_cooler_r1: None,
        stack_cooler_r2: None,
        stack_cooler_r3: None,
        stack_nominal_temp_c: None,
        heat_rec_max_temp_c: None,
        no_load_fuel_fraction: None,
        grid_forming: None,
        standby_mode: None,
        parasitic_fraction: None,
    }
}

fn full_central_ac_config() -> CentralAirConditionerConfig {
    CentralAirConditionerConfig {
        equipment_id: Some(1),
        zone_id: Some(1),
        capacity_w: 10000.0,
        eir: 0.3,
        shr: Some(0.7),
        ..serde_json::from_slice::<CentralAirConditionerConfig>(
            br#"{"capacity_w": 10000.0, "eir": 0.3}"#,
        )
        .unwrap()
    }
}

fn full_room_ac_config() -> RoomAcConfig {
    serde_json::from_slice::<RoomAcConfig>(br#"{"capacity_w": 3500.0, "eir": 0.3}"#).unwrap()
}

fn full_heat_pump_cooler_config() -> HeatPumpCoolerConfig {
    HeatPumpCoolerConfig::default()
}

#[test]
fn equipment_config_fields_are_si() {
    // PV, once per payload shape (single-array and multi-array).
    assert_no_non_si_key(
        &serde_json::to_string(&full_pv_config()).unwrap(),
        "PvConfig (arrays)",
    );
    let mut single = full_pv_config();
    single.arrays = None;
    assert_no_non_si_key(
        &serde_json::to_string(&single).unwrap(),
        "PvConfig (single-array)",
    );

    // Battery, once per BmsMode payload variant.
    let modes = [
        None,
        Some("Manual".to_string()),
        Some(
            serde_json::to_string(&BmsMode::SelfConsumption {
                min_soc: 0.1,
                max_soc: 0.95,
                solar_only_charging: false,
                surplus_deadband_w: 500.0,
            })
            .unwrap(),
        ),
    ];
    for mode in modes {
        assert_no_non_si_key(
            &serde_json::to_string(&full_battery_config(mode)).unwrap(),
            "BatteryConfig",
        );
    }

    // EV, once per ChargingStrategy payload variant.
    let strategies = [
        None,
        Some(
            serde_json::to_string(&ChargingStrategy::SolarSurplus {
                min_charge_rate_w: 1400.0,
                departure_schedule: Vec::new(),
            })
            .unwrap(),
        ),
        Some(
            serde_json::to_string(&ChargingStrategy::V2G {
                min_soc: 0.3,
                max_export_w: 5000.0,
                price_threshold: 0.25,
            })
            .unwrap(),
        ),
    ];
    for strategy in strategies {
        assert_no_non_si_key(
            &serde_json::to_string(&full_ev_config(strategy)).unwrap(),
            "EvConfig",
        );
    }

    assert_no_non_si_key(
        &serde_json::to_string(&full_generator_config()).unwrap(),
        "GeneratorConfig",
    );
    assert_no_non_si_key(
        &serde_json::to_string(&full_central_ac_config()).unwrap(),
        "CentralAirConditionerConfig",
    );
    assert_no_non_si_key(
        &serde_json::to_string(&full_room_ac_config()).unwrap(),
        "RoomAcConfig",
    );
    assert_no_non_si_key(
        &serde_json::to_string(&full_heat_pump_cooler_config()).unwrap(),
        "HeatPumpCoolerConfig",
    );
}
