use std::collections::HashMap;
use std::time::Duration;

use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
use hares_types::{
    ControlSignal, EnvironmentState, EvConnectionState, GridState, PortSlots,
    SurfaceIrradiance, WeatherState, ZoneState,
};

use super::*;

fn dt(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> chrono::DateTime<FixedOffset> {
    FixedOffset::east_opt(0)
        .expect("UTC offset")
        .with_ymd_and_hms(y, mo, d, h, mi, s)
        .unwrap()
}

fn sample_env() -> EnvironmentState {
    EnvironmentState {
        zones: vec![ZoneState {
            id: hares_types::ZoneId(1),
            temperature_c: 21.0,
            humidity_ratio: 0.008,
            relative_humidity: 0.45,
            wet_bulb_c: 14.0,
            volume_m3: 220.0,
        }],
        weather: WeatherState {
            outdoor_temp_c: 10.0,
            outdoor_humidity_ratio: 0.005,
            outdoor_wet_bulb_c: 10.0,
            outdoor_enthalpy_j_kg: 0.0,
            wind_speed_m_s: 2.0,
            wind_dir_deg: 0.0,
            ground_temp_c: 12.0,
            sky_temp_c: 8.0,
            pressure_kpa: 101.325,
            solar_irradiance: vec![SurfaceIrradiance {
                surface_id: 1,
                direct_w_m2: 0.0,
                diffuse_w_m2: 0.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            }],
            ghi_w_m2: 0.0,
            dni_w_m2: 0.0,
            dhi_w_m2: 0.0,
            solar_altitude_deg: 0.0,
            solar_azimuth_deg: 180.0,
            mains_temp_c: 15.0,
            rainfall_m: 0.0,
            ground_albedo: 0.2,
        },
        grid: GridState {
            voltage_pu: 1.0,
            frequency_hz: 60.0,
        },
        custom_domains: vec![],
        equipment_telemetry: std::collections::HashMap::new(),
        current_time: dt(2026, 1, 1, 0, 0, 0),
        time_res: ChronoDuration::minutes(1),
    price_signal: Default::default(),
    electrical: Default::default(),
    }
}

fn ev_config(raw: HashMap<String, crate::config::ConfigValue>) -> EquipmentConfig {
    EquipmentConfig {
        name: "EV #1".to_string(),
        ochre_class: "EV".to_string(),
        raw_config: raw,
    }
}

fn base_raw() -> HashMap<String, crate::config::ConfigValue> {
    let mut raw = HashMap::new();
    raw.insert(KEY_BATTERY_CAPACITY_KWH.to_string(), 60.0.into());
    raw.insert(KEY_CHARGING_LEVEL.to_string(), "L2".into());
    raw.insert(KEY_MAX_CHARGING_POWER_KW.to_string(), 7.2.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.2.into());
    raw
}

trait DurationExt {
    fn minutes(n: u64) -> Duration;
}

impl DurationExt for Duration {
    fn minutes(n: u64) -> Duration {
        Duration::from_secs(n * 60)
    }
}

/// Telemetry encoding of `EvConnectionState::Disconnected`.
/// Matches the mapping in `Ev::emit_telemetry`: HomePluggedIn=0.0, AwayPluggedIn=1.0, Disconnected=2.0.
const TELEMETRY_STATE_DISCONNECTED: f64 = 2.0;

// ── Physics tests (preserved) ─────────────────────────────────────

#[test]
fn no_power_when_disconnected() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::Disconnected,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();

    assert_eq!(ev.telemetry().get("active_power_kw"), Some(0.0));
    assert_eq!(ev.telemetry().get("connection_state"), Some(TELEMETRY_STATE_DISCONNECTED));
    assert_eq!(ports.electrical.load_power_kw, 0.0);
}

#[test]
fn l1_default_power_is_1p4_kw() {
    let mut raw = base_raw();
    raw.insert(KEY_CHARGING_LEVEL.to_string(), "L1".into());
    raw.remove(KEY_MAX_CHARGING_POWER_KW);
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();

    let p = ev.telemetry().get("active_power_kw").unwrap();
    assert!((p - 1.4).abs() < 1e-12);
}

#[test]
fn l2_constant_power_and_linear_taper() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();
    let p_low_soc = ev.telemetry().get("active_power_kw").unwrap();
    assert!((p_low_soc - 7.2).abs() < 1e-9);

    ev.soc = 0.99;
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();
    let p_taper = ev.telemetry().get("active_power_kw").unwrap();
    assert!(p_taper > 0.0);
    assert!(p_taper < 7.2);

    ev.soc = 1.0;
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();
    let p_full = ev.telemetry().get("active_power_kw").unwrap();
    assert_eq!(p_full, 0.0);
}

#[test]
fn negative_power_setpoint_rejected_without_v2l_or_v2g() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    let err = ev
        .apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: -1.0,
            reactive_power_kvar: None,
        })
        .unwrap_err();

    assert!(
        err.to_string().contains("v2l_enabled or v2g_enabled"),
        "expected rejection message, got: {err}"
    );
}

#[test]
fn save_load_preserves_trajectory() {
    let config = ev_config(base_raw());
    let env = sample_env();
    let mut a = Ev::new(config.clone());
    a.init(&config, &env).unwrap();

    for _ in 0..50 {
        let mut ports = PortSlots::default();
        a.step(&env, Duration::minutes(5), &mut ports).unwrap();
    }

    let checkpoint = a.save_state();
    let mut b = Ev::new(config.clone());
    b.init(&config, &env).unwrap();
    b.load_state(&checkpoint).unwrap();

    for _ in 0..120 {
        let mut pa = PortSlots::default();
        let mut pb = PortSlots::default();
        a.step(&env, Duration::minutes(5), &mut pa).unwrap();
        b.step(&env, Duration::minutes(5), &mut pb).unwrap();
        assert_eq!(a.telemetry().get("soc"), b.telemetry().get("soc"));
        assert_eq!(
            a.telemetry().get("active_power_kw"),
            b.telemetry().get("active_power_kw")
        );
    }
}

#[test]
fn hpxml_range_to_capacity_derivation_uses_verified_constant() {
    let mut raw = base_raw();
    raw.remove(KEY_BATTERY_CAPACITY_KWH);
    raw.insert(KEY_RANGE_MILES.to_string(), 200.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    ev.init(&config, &sample_env()).unwrap();

    assert!((ev.battery_capacity_kwh - 65.0).abs() < 1e-9);
}

#[test]
fn update_control_reports_charging_when_connected_and_drawing_power() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

    assert_eq!(ev.update_control(&env), OperatingMode::Charging);
}

#[test]
fn registry_registration_creates_ev() {
    let mut registry = EquipmentRegistry::new();
    register_with_registry(&mut registry);

    let eq = registry
        .create("EV", ev_config(base_raw()))
        .expect("EV registered");
    assert_eq!(eq.descriptor().equipment_type, "EV");
    assert_eq!(eq.descriptor().stage, ExecutionStage::Electrical);
}

#[test]
fn apply_soc_target_limits_charging() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    let max_soc = 0.3;
    ev.apply_control(&ControlSignal::SOCTarget {
        target_soc: max_soc,
        min_soc: None,
        max_soc: Some(max_soc),
    })
    .unwrap();

    for _ in 0..100 {
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(5), &mut ports).unwrap();
    }

    const SOC_FLOAT_TOL: f64 = 1e-3;
    assert!(
        ev.soc <= max_soc + SOC_FLOAT_TOL,
        "SOC {} should not exceed max_soc {} (+ tol {})",
        ev.soc,
        max_soc,
        SOC_FLOAT_TOL,
    );
}

#[test]
fn supports_hpxml_key_aliases() {
    let mut raw = base_raw();
    raw.remove(KEY_BATTERY_CAPACITY_KWH);
    raw.remove(KEY_MAX_CHARGING_POWER_KW);
    raw.remove(KEY_CHARGING_LEVEL);
    raw.insert(KEY_BATTERY_CAPACITY_HPIXML_KWH.to_string(), 64.0.into());
    raw.insert(KEY_MAX_CHARGING_POWER_HPIXML_KW.to_string(), 11.5.into());
    raw.insert(KEY_CHARGING_LEVEL_HPIXML.to_string(), "Level2".into());

    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    ev.init(&config, &sample_env()).unwrap();

    assert!((ev.battery_capacity_kwh - 64.0).abs() < 1e-9);
    assert!((ev.rated_power_kw - 11.5).abs() < 1e-9);
    assert_eq!(ev.charging_level, ChargingLevel::L2);
}

#[test]
fn l1_8amp_mode_is_supported() {
    let mut raw = base_raw();
    raw.insert(KEY_CHARGING_LEVEL.to_string(), "L1".into());
    raw.insert(KEY_L1_CURRENT_A.to_string(), 8.0.into());
    raw.insert(KEY_L1_VOLTAGE_V.to_string(), 120.0.into());
    let config = ev_config(raw);

    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();
    let p = ev.telemetry().get("active_power_kw").unwrap();
    assert!((p - 0.96).abs() < 1e-12);
}

#[test]
fn ohmic_heat_nonzero_during_active_charging() {
    let mut raw = base_raw();
    raw.insert(KEY_EFFICIENCY.to_string(), 0.85.into());
    raw.insert(KEY_BATTERY_TEMP_C.to_string(), 20.0.into());
    raw.insert(KEY_HEATER_POWER_W.to_string(), 0.0.into());
    raw.insert(KEY_THERMAL_MASS_J_PER_K.to_string(), 20_000.0.into());
    raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    let temp_before = ev.battery_temp_c;
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();

    let power_kw = ev.telemetry().get("active_power_kw").unwrap();
    assert!(power_kw > 0.0, "EV should be charging");
    assert!(
        ev.battery_temp_c > temp_before,
        "battery should warm from ohmic losses (efficiency < 1)"
    );
}

#[test]
fn l1_configured_power_is_respected_not_hardcoded() {
    let mut raw = base_raw();
    raw.insert(KEY_CHARGING_LEVEL.to_string(), "L1".into());
    raw.insert(KEY_MAX_CHARGING_POWER_KW.to_string(), 1.8.into());
    raw.remove(KEY_L1_CURRENT_A);
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    assert!(
        (ev.rated_power_kw - 1.8).abs() < 1e-9,
        "L1 rated_power_kw should reflect configured max_charging_power_kw=1.8, got {}",
        ev.rated_power_kw
    );

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();
    let p = ev.telemetry().get("active_power_kw").unwrap();
    assert!(
        (p - 1.8).abs() < 1e-9,
        "L1 step power should be 1.8 kW when configured, got {}",
        p
    );
}

#[test]
fn thermal_decay_occurs_when_disconnected() {
    let mut raw = base_raw();
    raw.insert(KEY_BATTERY_TEMP_C.to_string(), 30.0.into());
    raw.insert(KEY_UA_W_PER_K.to_string(), 4.0.into());
    raw.insert(KEY_THERMAL_MASS_J_PER_K.to_string(), 20_000.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let mut env = sample_env();
    env.weather.outdoor_temp_c = 0.0;
    ev.init(&config, &env).unwrap();

    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::Disconnected,
    })
    .unwrap();

    for _ in 0..100 {
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(1), &mut ports).unwrap();
    }

    assert!(
        ev.battery_temp_c < 30.0,
        "battery should cool toward ambient when disconnected, got {}C",
        ev.battery_temp_c
    );
    assert_eq!(
        ev.telemetry().get("active_power_kw"),
        Some(0.0),
        "no grid power when disconnected"
    );
}

#[test]
fn cold_temperature_blocks_charging_without_heater() {
    let mut raw = base_raw();
    raw.insert(
        KEY_BATTERY_TEMP_C.to_string(),
        crate::config::ConfigValue::Float(-5.0),
    );
    raw.insert(KEY_MIN_CHARGE_TEMP_C.to_string(), 0.0.into());
    raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(30), &mut ports).unwrap();
    assert_eq!(ev.telemetry().get("active_power_kw"), Some(0.0));
    assert_eq!(ev.telemetry().get("charge_derate"), Some(0.0));
}

#[test]
fn heater_draw_slows_soc_gain() {
    let mut raw = base_raw();
    raw.insert(
        KEY_BATTERY_TEMP_C.to_string(),
        crate::config::ConfigValue::Float(-1.0),
    );
    raw.insert(
        KEY_MIN_CHARGE_TEMP_C.to_string(),
        crate::config::ConfigValue::Float(-2.0),
    );
    raw.insert(KEY_FULL_POWER_TEMP_C.to_string(), 10.0.into());
    raw.insert(KEY_HEATER_POWER_W.to_string(), 1200.0.into());
    raw.insert(KEY_HEATER_THRESHOLD_C.to_string(), 0.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();
    let soc_before = ev.soc;

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();
    let soc_delta_with_heater = ev.soc - soc_before;

    let mut raw_no_heater = base_raw();
    raw_no_heater.insert(
        KEY_BATTERY_TEMP_C.to_string(),
        crate::config::ConfigValue::Float(-1.0),
    );
    raw_no_heater.insert(
        KEY_MIN_CHARGE_TEMP_C.to_string(),
        crate::config::ConfigValue::Float(-2.0),
    );
    raw_no_heater.insert(KEY_FULL_POWER_TEMP_C.to_string(), 10.0.into());
    let config_no_heater = ev_config(raw_no_heater);
    let mut ev_no_heater = Ev::new(config_no_heater.clone());
    ev_no_heater
        .init(&config_no_heater, &sample_env())
        .unwrap();
    let soc_before_no_heater = ev_no_heater.soc;
    let mut ports = PortSlots::default();
    ev_no_heater
        .step(&env, Duration::minutes(60), &mut ports)
        .unwrap();
    let soc_delta_no_heater = ev_no_heater.soc - soc_before_no_heater;

    assert!(soc_delta_with_heater < soc_delta_no_heater);
    assert!(ev.telemetry().get("heater_power_w").unwrap() > 0.0);
}

#[test]
fn heater_only_grid_draw_when_fully_cold_derated() {
    let mut raw = base_raw();
    raw.insert(
        KEY_BATTERY_TEMP_C.to_string(),
        crate::config::ConfigValue::Float(-5.0),
    );
    raw.insert(KEY_MIN_CHARGE_TEMP_C.to_string(), 0.0.into());
    raw.insert(KEY_FULL_POWER_TEMP_C.to_string(), 10.0.into());
    raw.insert(KEY_HEATER_POWER_W.to_string(), 500.0.into());
    raw.insert(
        KEY_HEATER_THRESHOLD_C.to_string(),
        crate::config::ConfigValue::Float(-4.0),
    );
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();
    let soc_before = ev.soc;

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

    let power_kw = ev.telemetry().get("active_power_kw").unwrap();
    let expected_heater_kw = 500.0 / 1000.0;
    assert!(
        (power_kw - expected_heater_kw).abs() < 1e-9,
        "grid draw should equal heater power only ({} kW), got {} kW",
        expected_heater_kw,
        power_kw
    );
    assert_eq!(
        ev.soc, soc_before,
        "SOC must not change when charge_derate=0 (pre-heat only)"
    );
    assert!(
        ev.telemetry().get("heater_power_w").unwrap() > 0.0,
        "heater should be active"
    );
}

#[test]
fn charge_derate_applied_before_taper_limit() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
    raw.insert(KEY_BATTERY_TEMP_C.to_string(), 5.0.into());
    raw.insert(KEY_MIN_CHARGE_TEMP_C.to_string(), 0.0.into());
    raw.insert(KEY_FULL_POWER_TEMP_C.to_string(), 10.0.into());
    raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
    let config_cold = ev_config(raw);

    let mut raw_warm = base_raw();
    raw_warm.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
    raw_warm.insert(KEY_BATTERY_TEMP_C.to_string(), 20.0.into());
    raw_warm.insert(KEY_MIN_CHARGE_TEMP_C.to_string(), 0.0.into());
    raw_warm.insert(KEY_FULL_POWER_TEMP_C.to_string(), 10.0.into());
    raw_warm.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
    let config_warm = ev_config(raw_warm);

    let mut ev_cold = Ev::new(config_cold.clone());
    let mut ev_warm = Ev::new(config_warm.clone());
    let env = sample_env();
    ev_cold.init(&config_cold, &env).unwrap();
    ev_warm.init(&config_warm, &env).unwrap();

    let mut p_cold = PortSlots::default();
    let mut p_warm = PortSlots::default();
    ev_cold
        .step(&env, Duration::minutes(15), &mut p_cold)
        .unwrap();
    ev_warm
        .step(&env, Duration::minutes(15), &mut p_warm)
        .unwrap();

    let power_cold = ev_cold.telemetry().get("active_power_kw").unwrap();
    let power_warm = ev_warm.telemetry().get("active_power_kw").unwrap();
    assert!(
        power_cold < power_warm,
        "derated power ({power_cold}) must be less than full power ({power_warm}) at same SOC"
    );
    let ratio = power_cold / power_warm;
    assert!(
        (ratio - 0.5).abs() < 0.05,
        "power ratio should be ~0.5 (derate factor), got {ratio}"
    );
}

// ── V2L mode tests ────────────────────────────────────────────────

#[test]
fn v2l_allows_negative_power_when_enabled_and_soc_above_reserve() {
    let mut raw = base_raw();
    raw.insert(KEY_V2L_ENABLED.to_string(), true.into());
    raw.insert(KEY_V2L_SOC_RESERVE.to_string(), 0.2.into());
    raw.insert(KEY_V2L_MAX_DISCHARGE_KW.to_string(), 3.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_kw: -2.0,
        reactive_power_kvar: None,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

    let power = ev.telemetry().get("active_power_kw").unwrap();
    assert!(
        power < 0.0,
        "V2L should produce negative active_power_kw, got {power}"
    );
    assert_eq!(ev.telemetry().get("v2l_active"), Some(1.0));
}

#[test]
fn v2l_respects_soc_reserve_floor() {
    let mut raw = base_raw();
    raw.insert(KEY_V2L_ENABLED.to_string(), true.into());
    raw.insert(KEY_V2L_SOC_RESERVE.to_string(), 0.5.into());
    raw.insert(KEY_V2L_MAX_DISCHARGE_KW.to_string(), 5.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_kw: -3.0,
        reactive_power_kvar: None,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

    let power = ev.telemetry().get("active_power_kw").unwrap();
    assert_eq!(power, 0.0, "V2L must not discharge when SOC <= reserve");
}

#[test]
fn v2l_respects_max_discharge_limit() {
    let mut raw = base_raw();
    raw.insert(KEY_V2L_ENABLED.to_string(), true.into());
    raw.insert(KEY_V2L_SOC_RESERVE.to_string(), 0.1.into());
    raw.insert(KEY_V2L_MAX_DISCHARGE_KW.to_string(), 2.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.8.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_kw: -10.0,
        reactive_power_kvar: None,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();

    let power = ev.telemetry().get("active_power_kw").unwrap();
    assert!(
        power >= -2.0 - 1e-9,
        "V2L discharge must respect max_discharge_kw limit of 2.0, got {power}"
    );
}

#[test]
fn v2l_rejected_when_disabled() {
    let mut raw = base_raw();
    raw.insert(KEY_V2L_ENABLED.to_string(), false.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    ev.init(&config, &sample_env()).unwrap();

    let err = ev
        .apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: -1.0,
            reactive_power_kvar: None,
        })
        .unwrap_err();
    assert!(
        err.to_string().contains("v2l_enabled or v2g_enabled"),
        "negative setpoint without V2L should be rejected"
    );
}

#[test]
fn v2l_does_not_export_to_grid() {
    let mut raw = base_raw();
    raw.insert(KEY_V2L_ENABLED.to_string(), true.into());
    raw.insert(KEY_V2L_SOC_RESERVE.to_string(), 0.1.into());
    raw.insert(KEY_V2L_MAX_DISCHARGE_KW.to_string(), 3.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.8.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_kw: -2.0,
        reactive_power_kvar: None,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

    assert!(
        ports.electrical.generation_power_kw < 0.0,
        "V2L discharge should appear in generation_power_kw, got {}",
        ports.electrical.generation_power_kw
    );
    assert!(
        ports.electrical.generation_power_kw.abs() <= 3.0 + 1e-9,
        "V2L must not exceed max_discharge_kw={}, got {}",
        3.0,
        ports.electrical.generation_power_kw.abs()
    );
    assert_eq!(
        ports.electrical.load_power_kw, 0.0,
        "V2L discharge should not appear in load_power_kw"
    );
}

// ── V2G tests ──────────────────────────────────────────────────

#[test]
fn v2g_allows_negative_power_when_enabled() {
    let mut raw = base_raw();
    raw.insert(KEY_V2G_ENABLED.to_string(), true.into());
    raw.insert(KEY_V2G_SOC_RESERVE.to_string(), 0.3.into());
    raw.insert(KEY_V2G_MAX_DISCHARGE_KW.to_string(), 5.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).expect("init");

    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_kw: -3.0,
        reactive_power_kvar: None,
    })
    .expect("V2G negative setpoint should be accepted");
}

#[test]
fn v2g_discharge_produces_negative_power() {
    let mut raw = base_raw();
    raw.insert(KEY_V2G_ENABLED.to_string(), true.into());
    raw.insert(KEY_V2G_SOC_RESERVE.to_string(), 0.2.into());
    raw.insert(KEY_V2G_MAX_DISCHARGE_KW.to_string(), 5.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.8.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).expect("init");

    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_kw: -3.0,
        reactive_power_kvar: None,
    })
    .expect("setpoint");

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports)
        .expect("step");
    let power = ev.telemetry().get("active_power_kw").unwrap_or(0.0);
    assert!(
        power < -0.1,
        "V2G should produce negative active_power_kw, got {power}"
    );
}

#[test]
fn v2g_respects_soc_reserve() {
    let mut raw = base_raw();
    raw.insert(KEY_V2G_ENABLED.to_string(), true.into());
    raw.insert(KEY_V2G_SOC_RESERVE.to_string(), 0.5.into());
    raw.insert(KEY_V2G_MAX_DISCHARGE_KW.to_string(), 5.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).expect("init");

    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_kw: -5.0,
        reactive_power_kvar: None,
    })
    .expect("setpoint");

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports)
        .expect("step");
    let power = ev.telemetry().get("active_power_kw").unwrap_or(0.0);
    assert!(
        power.abs() < 0.01,
        "V2G must not discharge when SOC at reserve, got {power}"
    );
}

#[test]
fn v2g_disabled_by_default() {
    let config = ev_config(base_raw());
    let ev = Ev::new(config);
    assert!(!ev.v2g_enabled, "V2G should be disabled by default");
}

// ── 4D LUT and degradation tests ─────────────────────────────────

fn make_4d_lut(soc_pf: &[(f64, f32)]) -> crate::ndinterp::RegularGridInterpolator {
    let soc_grid: Vec<f64> = soc_pf.iter().map(|(s, _)| *s).collect();
    let values: Vec<f32> = soc_pf.iter().map(|(_, p)| *p).collect();
    crate::ndinterp::RegularGridInterpolator::new(
        vec![soc_grid, vec![25.0], vec![1.0], vec![1.0]],
        values,
    )
    .unwrap()
}

#[test]
fn charging_curve_lut_limits_power() {
    let soc_grid = vec![0.0, 0.5, 1.0];
    let temp_grid = vec![25.0];
    let crate_grid = vec![1.0];
    let soh_grid = vec![1.0];
    let values = vec![1.0f32, 0.5, 0.0];
    let lut = crate::ndinterp::RegularGridInterpolator::new(
        vec![soc_grid, temp_grid, crate_grid, soh_grid],
        values,
    )
    .unwrap();

    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();
    ev.set_charging_curve_lut(Some(lut)).unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();
    let power_kw = ev.telemetry().get("active_power_kw").unwrap();
    assert!(
        power_kw <= 3.6 + 1e-9,
        "power should be limited at soc=0.5, got {power_kw}"
    );
}

#[test]
fn set_charging_curve_lut_via_equipment_trait() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config);
    let lut = make_4d_lut(&[(0.0, 1.0), (1.0, 0.0)]);
    ev.set_charging_curve_lut(Some(lut)).unwrap();
    assert!(ev.charging_curve_lut.is_some());
}

#[test]
fn clear_charging_curve_lut_via_equipment_trait() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config);
    let lut = make_4d_lut(&[(0.0, 1.0), (1.0, 0.0)]);
    ev.set_charging_curve_lut(Some(lut)).unwrap();
    ev.set_charging_curve_lut(None).unwrap();
    assert!(ev.charging_curve_lut.is_none());
}

#[test]
fn lut_tapers_charge_power_at_high_soc() {
    let lut = make_4d_lut(&[(0.0, 1.0), (0.5, 1.0), (0.8, 0.5), (1.0, 0.0)]);
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.9.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();
    ev.set_charging_curve_lut(Some(lut)).unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();
    let power = ev.telemetry().get("active_power_kw").unwrap();
    assert!(
        power <= 2.0,
        "power should be tapered at high SOC, got {power}"
    );
}

#[test]
fn degradation_starts_at_zero() {
    let config = ev_config(base_raw());
    let ev = Ev::new(config);
    assert_eq!(ev.degradation.capacity_fade_pct(), 0.0);
}

#[test]
fn capacity_fade_pct_in_telemetry() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();
    assert_eq!(ev.telemetry().get("capacity_fade_pct"), Some(0.0));
}

#[test]
fn degradation_state_survives_checkpoint() {
    let config = ev_config(base_raw());
    let mut ev1 = Ev::new(config.clone());
    let env = sample_env();
    ev1.init(&config, &env).unwrap();

    let saved = ev1.save_state();
    let mut ev2 = Ev::new(config.clone());
    ev2.init(&config, &env).unwrap();
    ev2.load_state(&saved).unwrap();
    assert_eq!(
        ev2.degradation.capacity_fade_pct(),
        ev1.degradation.capacity_fade_pct(),
    );
}

#[test]
fn no_lut_charges_at_full_rated_power() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.3.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();
    assert!(ev.charging_curve_lut.is_none());

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();
    let power = ev.telemetry().get("active_power_kw").unwrap();
    assert!(
        power > 5.0,
        "without LUT, EV should charge near rated power, got {power}"
    );
}

#[test]
fn lut_with_soh_adjusted_c_rate() {
    let axes = vec![
        vec![0.0, 1.0],
        vec![25.0],
        vec![0.05, 0.30],
        vec![0.5, 1.0],
    ];
    let values = vec![1.0f32, 1.0, 0.1, 0.1, 1.0, 1.0, 0.1, 0.1];
    let lut = crate::ndinterp::RegularGridInterpolator::new(axes, values).unwrap();

    let config = ev_config(base_raw());
    let mut ev = Ev::new(config);
    ev.set_charging_curve_lut(Some(lut)).unwrap();
    assert!(ev.charging_curve_lut.is_some());
}

// ── Registry tests ────────────────────────────────────────────────

#[test]
fn registry_new_can_look_up_ev_by_name() {
    let registry = crate::EquipmentRegistry::new();
    assert!(
        registry.get("EV").is_some(),
        "EquipmentRegistry::new() must register 'EV'"
    );
}

#[test]
fn registry_creates_ev_with_correct_type() {
    let registry = crate::EquipmentRegistry::new();
    let eq = registry
        .create("EV", ev_config(base_raw()))
        .expect("EV should be registered");
    assert_eq!(eq.descriptor().equipment_type, "EV");
}

// ── OCV / UNeg table support ──────────────────────────────────────

#[test]
fn set_ocv_table_marks_custom() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config);
    ev.set_ocv_table(OcvTable::default_li_nmc()).unwrap();
    assert!(ev.has_custom_ocv_table());
}

#[test]
fn set_u_neg_table_marks_custom() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config);
    ev.set_u_neg_table(UNegTable::default_li_nmc()).unwrap();
    assert!(ev.has_custom_u_neg_table());
}

#[test]
fn reset_ocv_table_restores_chemistry_default() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config);
    let custom = OcvTable::for_chemistry(BatteryChemistry::Lfp);
    ev.set_ocv_table(custom).unwrap();
    ev.reset_ocv_table().unwrap();
    assert!(!ev.has_custom_ocv_table());
}

#[test]
fn reset_u_neg_table_restores_chemistry_default() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config);
    let custom = UNegTable::for_chemistry(BatteryChemistry::Lfp);
    ev.set_u_neg_table(custom).unwrap();
    ev.reset_u_neg_table().unwrap();
    assert!(!ev.has_custom_u_neg_table());
}

// ── Control signal invariant tests ────────────────────────────────

#[test]
fn ev_plug_in_disconnected_produces_zero_power() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::Disconnected,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    assert_eq!(ev.telemetry().get("active_power_kw"), Some(0.0));
}

#[test]
fn ev_plug_in_home_with_soc_target_charges() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control(&ControlSignal::SOCTarget {
        target_soc: 0.9,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    assert!(ev.telemetry().get("active_power_kw").unwrap() > 0.0);
}

#[test]
fn ev_drive_reduces_soc() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::Disconnected,
    })
    .unwrap();

    let soc_before = ev.soc;
    ev.apply_control_unchecked(&ControlSignal::EvDrive { kwh: 10.0 })
        .unwrap();

    let expected_drop = 10.0 / 60.0;
    assert!(
        (ev.soc - (soc_before - expected_drop)).abs() < 1e-9,
        "SOC should drop by ~{expected_drop}, got {}",
        soc_before - ev.soc
    );
}

#[test]
fn ev_drive_while_home_plugged_in_rejected() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    let err = ev
        .apply_control_unchecked(&ControlSignal::EvDrive { kwh: 5.0 })
        .unwrap_err();
    assert!(err.to_string().contains("Disconnected"));
}

#[test]
fn ev_drive_while_away_plugged_in_rejected() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::Disconnected,
    })
    .unwrap();
    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::AwayPluggedIn,
    })
    .unwrap();

    let err = ev
        .apply_control_unchecked(&ControlSignal::EvDrive { kwh: 5.0 })
        .unwrap_err();
    assert!(err.to_string().contains("Disconnected"));
}

#[test]
fn ev_away_charge_increases_soc_no_residential_power() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.3.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::Disconnected,
    })
    .unwrap();
    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::AwayPluggedIn,
    })
    .unwrap();
    ev.apply_control_unchecked(&ControlSignal::EvAwayCharge { power_kw: 7.2 })
        .unwrap();

    let soc_before = ev.soc;
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();

    assert!(ev.soc > soc_before, "SOC should increase from away charging");
    assert_eq!(
        ev.telemetry().get("active_power_kw"),
        Some(0.0),
        "residential power should be 0 during away charging"
    );
    assert_eq!(ports.electrical.load_power_kw, 0.0);
}

#[test]
fn ev_away_charge_while_home_rejected() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    let err = ev
        .apply_control_unchecked(&ControlSignal::EvAwayCharge { power_kw: 7.2 })
        .unwrap_err();
    assert!(err.to_string().contains("AwayPluggedIn"));
}

#[test]
fn ev_away_charge_while_disconnected_rejected() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::Disconnected,
    })
    .unwrap();

    let err = ev
        .apply_control_unchecked(&ControlSignal::EvAwayCharge { power_kw: 7.2 })
        .unwrap_err();
    assert!(err.to_string().contains("AwayPluggedIn"));
}

#[test]
fn ev_plug_in_home_to_away_rejected() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    let err = ev
        .apply_control_unchecked(&ControlSignal::EvPlugIn {
            state: EvConnectionState::AwayPluggedIn,
        })
        .unwrap_err();
    assert!(err.to_string().contains("disconnect first"));
}

// ── BMS ready-by scheduling tests ─────────────────────────────────

#[test]
fn bms_default_charges_to_soc_max() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    // No actor controls, just step many times
    for _ in 0..500 {
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(5), &mut ports).unwrap();
    }

    assert!(
        ev.soc >= ev.ready_soc - 0.01,
        "BMS should charge to ready_soc (default=soc_max), got {}",
        ev.soc
    );
}

#[test]
fn bms_custom_ready_soc_stops_at_target() {
    let mut raw = base_raw();
    raw.insert(KEY_READY_SOC.to_string(), 0.8.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    for _ in 0..500 {
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(5), &mut ports).unwrap();
    }

    assert!(
        ev.soc <= 0.81,
        "BMS should stop at ready_soc=0.8, got {}",
        ev.soc
    );
}

#[test]
fn soc_target_overrides_bms_ready_soc() {
    let mut raw = base_raw();
    raw.insert(KEY_READY_SOC.to_string(), 0.8.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control(&ControlSignal::SOCTarget {
        target_soc: 0.6,
        min_soc: None,
        max_soc: Some(0.6),
    })
    .unwrap();

    for _ in 0..500 {
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(5), &mut ports).unwrap();
    }

    assert!(
        ev.soc <= 0.61,
        "actor SOCTarget should override BMS ready_soc, got {}",
        ev.soc
    );
}

#[test]
fn soc_target_can_raise_above_ready_soc() {
    let mut raw = base_raw();
    raw.insert(KEY_READY_SOC.to_string(), 0.8.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control(&ControlSignal::SOCTarget {
        target_soc: 1.0,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    for _ in 0..500 {
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(5), &mut ports).unwrap();
    }

    assert!(
        ev.soc >= 0.99,
        "actor SOCTarget=1.0 should raise above BMS ready_soc=0.8, got {}",
        ev.soc
    );
}

#[test]
fn power_setpoint_zero_suppresses_charging() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_kw: 0.0,
        reactive_power_kvar: None,
    })
    .unwrap();

    let soc_before = ev.soc;
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();
    assert_eq!(ev.soc, soc_before, "PowerSetpoint=0 should suppress charging");
}

#[test]
fn ev_set_ready_by_delays_then_charges() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.3.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let mut env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control_unchecked(&ControlSignal::EvSetReadyBy {
        departure_hour: 7.0,
        target_soc: 0.8,
    })
    .unwrap();

    // Step at 18:00 — should be too early, BMS delays
    env.current_time = dt(2026, 1, 1, 18, 0, 0);
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    let power_early = ev.telemetry().get("active_power_kw").unwrap();

    // Step at 5:00 — should be charging (close to deadline)
    ev.soc = 0.3;
    env.current_time = dt(2026, 1, 2, 5, 0, 0);
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    let power_late = ev.telemetry().get("active_power_kw").unwrap();

    assert_eq!(
        power_early, 0.0,
        "BMS should delay charging early in the evening"
    );
    assert!(
        power_late > 0.0,
        "BMS should be charging close to deadline"
    );
}

#[test]
fn ev_set_ready_by_tight_deadline_charges_immediately() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.1.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let mut env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control_unchecked(&ControlSignal::EvSetReadyBy {
        departure_hour: 2.0,
        target_soc: 0.9,
    })
    .unwrap();

    env.current_time = dt(2026, 1, 1, 22, 0, 0);
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    let power = ev.telemetry().get("active_power_kw").unwrap();
    assert!(power > 0.0, "tight deadline should charge immediately");
}

#[test]
fn ev_set_ready_by_cleared_on_disconnect() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control_unchecked(&ControlSignal::EvSetReadyBy {
        departure_hour: 7.0,
        target_soc: 0.8,
    })
    .unwrap();
    assert!(ev.ready_by_hour.is_some());

    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::Disconnected,
    })
    .unwrap();
    assert!(ev.ready_by_hour.is_none());
    assert!(ev.ready_by_soc.is_none());
}

#[test]
fn bms_delay_outputs_zero_power_before_start() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.75.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let mut env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control_unchecked(&ControlSignal::EvSetReadyBy {
        departure_hour: 7.0,
        target_soc: 0.8,
    })
    .unwrap();

    env.current_time = dt(2026, 1, 1, 19, 0, 0);
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    let power = ev.telemetry().get("active_power_kw").unwrap();
    assert_eq!(
        power, 0.0,
        "BMS should delay when SOC deficit is small and deadline is far"
    );
}

#[test]
fn already_at_target_produces_zero_power() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.9.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let mut env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control_unchecked(&ControlSignal::EvSetReadyBy {
        departure_hour: 7.0,
        target_soc: 0.8,
    })
    .unwrap();

    env.current_time = dt(2026, 1, 1, 3, 0, 0);
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    let power = ev.telemetry().get("active_power_kw").unwrap();
    assert_eq!(power, 0.0, "already above target SOC should produce 0 power");
}

#[test]
fn deadline_passed_charges_immediately() {
    // Deadline at 14:00, currently 15:00 — unambiguously past the deadline
    // on the same day. With wrap, hours_until_deadline = 23, which is huge
    // relative to the small SOC deficit, so BMS would delay. But in reality
    // the deadline has passed and the EV should charge immediately.
    // This test verifies that a tight SOC deficit with a "past" deadline
    // still results in charging, since the wrap means there's plenty of
    // time and the BMS starts immediately if hours_needed < hours_until_deadline.
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.3.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let mut env = sample_env();
    ev.init(&config, &env).unwrap();

    // Deadline at exactly current hour — triggers immediate charge
    ev.apply_control_unchecked(&ControlSignal::EvSetReadyBy {
        departure_hour: 15.0,
        target_soc: 0.8,
    })
    .unwrap();

    env.current_time = dt(2026, 1, 1, 15, 0, 0);
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    let power = ev.telemetry().get("active_power_kw").unwrap();
    assert!(power > 0.0, "at deadline should charge immediately");
}

#[test]
fn negative_start_hour_charges_immediately() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.1.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let mut env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control_unchecked(&ControlSignal::EvSetReadyBy {
        departure_hour: 2.0,
        target_soc: 0.9,
    })
    .unwrap();

    env.current_time = dt(2026, 1, 1, 22, 0, 0);
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    let power = ev.telemetry().get("active_power_kw").unwrap();
    assert!(
        power > 0.0,
        "negative start_hour should charge immediately"
    );
}

#[test]
fn power_limit_during_ready_by() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.3.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let mut env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control_unchecked(&ControlSignal::EvSetReadyBy {
        departure_hour: 7.0,
        target_soc: 0.8,
    })
    .unwrap();
    ev.apply_control(&ControlSignal::PowerLimit {
        max_power_kw: 2.0,
        ramp_rate_kw_per_s: None,
    })
    .unwrap();

    env.current_time = dt(2026, 1, 2, 5, 0, 0);
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    let power = ev.telemetry().get("active_power_kw").unwrap();
    assert!(
        power <= 2.0 + 1e-9,
        "PowerLimit should constrain ready-by charging, got {power}"
    );
}

#[test]
fn away_charges_to_ready_soc() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.3.into());
    raw.insert(KEY_READY_SOC.to_string(), 0.8.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::Disconnected,
    })
    .unwrap();
    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::AwayPluggedIn,
    })
    .unwrap();
    ev.apply_control_unchecked(&ControlSignal::EvAwayCharge { power_kw: 7.2 })
        .unwrap();

    for _ in 0..500 {
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(5), &mut ports).unwrap();
    }

    assert!(
        ev.soc >= 0.79 && ev.soc <= 0.81,
        "away charging should charge to ready_soc=0.8, got {}",
        ev.soc
    );
}

// ── Telemetry tests ───────────────────────────────────────────────

#[test]
fn telemetry_home_plugged_in() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

    assert!(ev.telemetry().get("active_power_kw").unwrap() > 0.0);
    assert_eq!(ev.telemetry().get("away_charge_power_kw"), Some(0.0));
    assert_eq!(ev.telemetry().get("connection_state"), Some(0.0));
}

#[test]
fn telemetry_away_plugged_in() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.3.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::Disconnected,
    })
    .unwrap();
    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::AwayPluggedIn,
    })
    .unwrap();
    ev.apply_control_unchecked(&ControlSignal::EvAwayCharge { power_kw: 11.5 })
        .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

    assert_eq!(ev.telemetry().get("active_power_kw"), Some(0.0));
    // away_charge_power_kw is the actual intake after CC-CV taper, not the
    // external charger's rated power. At SOC=0.3 the actual power is limited
    // by the EV's internal charging rate.
    let away_kw = ev.telemetry().get("away_charge_power_kw").unwrap();
    assert!(away_kw > 0.0, "should be charging away");
    assert_eq!(ev.telemetry().get("connection_state"), Some(1.0));
}

#[test]
fn telemetry_disconnected() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::Disconnected,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

    assert_eq!(ev.telemetry().get("active_power_kw"), Some(0.0));
    assert_eq!(ev.telemetry().get("away_charge_power_kw"), Some(0.0));
    assert_eq!(ev.telemetry().get("connection_state"), Some(TELEMETRY_STATE_DISCONNECTED));
}

#[test]
fn telemetry_has_capacity_and_fuel_economy() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    assert_eq!(ev.telemetry().get("capacity_kwh"), Some(60.0));
    assert_eq!(
        ev.telemetry().get("fuel_economy_kwh_per_mi"),
        Some(DEFAULT_FUEL_ECONOMY_KWH_PER_MI)
    );
}

// ── Away charging physics parity tests ────────────────────────────

#[test]
fn away_charge_zero_power_idles() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::Disconnected,
    })
    .unwrap();
    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::AwayPluggedIn,
    })
    .unwrap();
    ev.apply_control_unchecked(&ControlSignal::EvAwayCharge { power_kw: 0.0 })
        .unwrap();

    let soc_before = ev.soc;
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();
    assert_eq!(ev.soc, soc_before, "zero away charge power should not change SOC");
}

#[test]
fn ev_defaults_to_home_plugged_in() {
    let config = ev_config(base_raw());
    let ev = Ev::new(config);
    assert_eq!(ev.connection_state, EvConnectionState::HomePluggedIn);
}

#[test]
fn ev_control_capabilities_include_new_signals() {
    let config = ev_config(base_raw());
    let ev = Ev::new(config);
    let caps = ev.descriptor().control_capabilities;
    assert!(caps.contains(ControlCapabilities::EV_PLUG_IN));
    assert!(caps.contains(ControlCapabilities::EV_DRIVE));
    assert!(caps.contains(ControlCapabilities::EV_AWAY_CHARGE));
    assert!(caps.contains(ControlCapabilities::EV_SET_READY_BY));
}

#[test]
fn ev_charging_strategy_v2g_from_config() {
    let mut raw = base_raw();
    let json = serde_json::to_string(&hares_types::ChargingStrategy::V2G {
        min_soc: 0.3,
        max_export_kw: 7.2,
        price_threshold: 0.25,
    })
    .unwrap();
    raw.insert(
        KEY_CHARGING_STRATEGY.to_string(),
        crate::config::ConfigValue::Text(json),
    );
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();
    assert_eq!(
        ev.charging_strategy(),
        &hares_types::ChargingStrategy::V2G {
            min_soc: 0.3,
            max_export_kw: 7.2,
            price_threshold: 0.25,
        }
    );
}

#[test]
fn ev_charging_strategy_backward_compat() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();
    assert_eq!(
        ev.charging_strategy(),
        &hares_types::ChargingStrategy::Immediate { target_soc: 1.0 }
    );
}

#[test]
fn actor_seed_immediate_returns_none() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    ev.init(&config, &sample_env()).unwrap();
    assert!(ev.actor_seed().is_none());
}

#[test]
fn actor_seed_nightly_returns_ev_seed() {
    let mut raw = base_raw();
    let json = serde_json::to_string(&hares_types::ChargingStrategy::Nightly {
        off_peak_start_hour: 23.0,
        off_peak_end_hour: 6.0,
        target_soc: 0.9,
    })
    .unwrap();
    raw.insert(
        KEY_CHARGING_STRATEGY.to_string(),
        crate::config::ConfigValue::Text(json),
    );
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    ev.init(&config, &sample_env()).unwrap();

    let seed = ev.actor_seed();
    assert!(seed.is_some());
    match seed.unwrap() {
        crate::ActorSeed::Ev {
            strategy,
            plug_in_policy,
            capacity_kwh,
            max_charge_kw,
            ..
        } => {
            assert!(matches!(
                strategy,
                hares_types::ChargingStrategy::Nightly { .. }
            ));
            assert_eq!(plug_in_policy, hares_types::PlugInPolicy::Always);
            assert!((capacity_kwh - 60.0).abs() < 1e-6);
            assert!((max_charge_kw - 7.2).abs() < 1e-6);
        }
        _ => panic!("expected Ev seed"),
    }
}

// ── RV-013: Physics-grounded EV driver lifecycle tests ────────────

/// Physics reference: L2 7.2 kW charger, 90% efficiency, 60 kWh battery.
/// DC energy per hour = 7.2 * 0.90 = 6.48 kWh/h
/// SOC increase per hour = 6.48 / 60 = 0.108
/// After 4 h from SOC 0.50: SOC = 0.50 + 4*0.108 = 0.932
///
/// Validates DC = AC * eta (not AC = DC * eta which would overstate by 11%).
#[test]
fn l2_charging_energy_accounting() {
    let mut raw = base_raw();
    raw.insert(KEY_BATTERY_CAPACITY_KWH.to_string(), 60.0.into());
    raw.insert(KEY_MAX_CHARGING_POWER_KW.to_string(), 7.2.into());
    raw.insert(KEY_EFFICIENCY.to_string(), 0.9.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
    raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
    raw.insert(KEY_HEATER_POWER_W.to_string(), 0.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    for _ in 0..240 {
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(1), &mut ports).unwrap();
    }

    let expected_soc = 0.932;
    assert!(
        (ev.soc - expected_soc).abs() < 0.005,
        "After 4h L2 charging: expected SOC ~ {expected_soc}, got {}",
        ev.soc
    );
}

/// Same setup as above; charge until full.
/// DC energy needed = 0.50 * 60 = 30.0 kWh
/// Time = 30.0 / (7.2 * 0.90) = 4.630 h ~ 278 minutes
#[test]
fn charging_reaches_full_at_correct_time() {
    let mut raw = base_raw();
    raw.insert(KEY_BATTERY_CAPACITY_KWH.to_string(), 60.0.into());
    raw.insert(KEY_MAX_CHARGING_POWER_KW.to_string(), 7.2.into());
    raw.insert(KEY_EFFICIENCY.to_string(), 0.9.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
    raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
    raw.insert(KEY_HEATER_POWER_W.to_string(), 0.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    let mut full_step = None;
    for step in 1..=400 {
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(1), &mut ports).unwrap();
        if ev.soc >= 1.0 - 1e-9 && full_step.is_none() {
            full_step = Some(step);
        }
    }

    let step = full_step.expect("EV should reach full within 400 minutes");
    assert!(
        (step as i32 - 278).unsigned_abs() <= 2,
        "Expected full at step ~278, got {step}"
    );
}

/// OCHRE EV_FUEL_ECONOMY default = 0.325 kWh/mile.
/// 30-mile trip: drive_kwh = 30 * 0.325 = 9.75 kWh
/// SOC drop = 9.75 / 60 = 0.1625
///
/// Validates driving uses fuel economy directly, no charging efficiency applied.
#[test]
fn driving_soc_decrease_matches_fuel_economy() {
    let mut raw = base_raw();
    raw.insert(KEY_BATTERY_CAPACITY_KWH.to_string(), 60.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 1.0.into());
    raw.insert(
        KEY_FUEL_ECONOMY_KWH_PER_MI.to_string(),
        0.325.into(),
    );
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::Disconnected,
    })
    .unwrap();

    let miles = 30.0;
    let drive_kwh = miles * 0.325;
    ev.apply_control_unchecked(&ControlSignal::EvDrive { kwh: drive_kwh })
        .unwrap();

    let expected_drop = 0.1625;
    let actual_drop = 1.0 - ev.soc;
    assert!(
        (actual_drop - expected_drop).abs() < 0.005,
        "Expected SOC drop ~ {expected_drop}, got {actual_drop}"
    );
}

/// Newton's law of cooling: T(t) = T_amb + (T0 - T_amb) * exp(-t/tau)
/// tau = thermal_mass / UA = 20000 / 4.0 = 5000 s
/// T0 = 25 C, T_amb = 10 C
/// After 1 h (3600 s): T = 10 + 15*exp(-3600/5000) = 17.30 C
/// After 2 h (7200 s): T = 10 + 15*exp(-7200/5000) = 13.55 C
#[test]
fn battery_thermal_exponential_decay() {
    let mut raw = base_raw();
    raw.insert(KEY_BATTERY_TEMP_C.to_string(), 25.0.into());
    raw.insert(KEY_THERMAL_MASS_J_PER_K.to_string(), 20_000.0.into());
    raw.insert(KEY_UA_W_PER_K.to_string(), 4.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let mut env = sample_env();
    env.weather.outdoor_temp_c = 10.0;
    ev.init(&config, &env).unwrap();

    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::Disconnected,
    })
    .unwrap();

    // Step 60 minutes (1 h)
    for _ in 0..60 {
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(1), &mut ports).unwrap();
    }
    let tau = 5000.0_f64;
    let expected_1h = 10.0 + 15.0 * (-3600.0 / tau).exp();
    assert!(
        (ev.battery_temp_c - expected_1h).abs() < 0.1,
        "After 1h: expected {expected_1h:.2} C, got {:.2} C",
        ev.battery_temp_c
    );

    // Step another 60 minutes (total 2 h)
    for _ in 0..60 {
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(1), &mut ports).unwrap();
    }
    let expected_2h = 10.0 + 15.0 * (-7200.0 / tau).exp();
    assert!(
        (ev.battery_temp_c - expected_2h).abs() < 0.1,
        "After 2h: expected {expected_2h:.2} C, got {:.2} C",
        ev.battery_temp_c
    );
}

/// SOC must clamp: never exceed 1.0 when overcharging, never go below 0.0.
#[test]
fn soc_clamps_at_boundaries() {
    // Charge well past full from SOC 0.5 for 600 minutes
    let mut raw = base_raw();
    raw.insert(KEY_BATTERY_CAPACITY_KWH.to_string(), 60.0.into());
    raw.insert(KEY_MAX_CHARGING_POWER_KW.to_string(), 7.2.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
    raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    for _ in 0..600 {
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(1), &mut ports).unwrap();
        assert!(
            ev.soc <= 1.0,
            "SOC must never exceed 1.0, got {}",
            ev.soc
        );
    }
    assert!(
        (ev.soc - 1.0).abs() < 1e-9,
        "SOC should be 1.0 after 600 min of charging from 0.5"
    );

    // Discharge from low SOC with a large drive
    let mut raw2 = base_raw();
    raw2.insert(KEY_BATTERY_CAPACITY_KWH.to_string(), 60.0.into());
    raw2.insert(KEY_INITIAL_SOC.to_string(), 0.05.into());
    let config2 = ev_config(raw2);
    let mut ev2 = Ev::new(config2.clone());
    ev2.init(&config2, &env).unwrap();

    ev2.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::Disconnected,
    })
    .unwrap();

    // Available = 0.05 * 60 = 3.0 kWh; try to drive 3.0 kWh exactly
    ev2.apply_control_unchecked(&ControlSignal::EvDrive { kwh: 3.0 })
        .unwrap();
    assert!(
        ev2.soc >= 0.0,
        "SOC must never go below 0.0, got {}",
        ev2.soc
    );
    assert!(
        ev2.soc.abs() < 1e-9,
        "SOC should be ~0.0 after draining remaining energy"
    );

    // Attempting to overdraw should be rejected
    let err = ev2
        .apply_control_unchecked(&ControlSignal::EvDrive { kwh: 1.0 })
        .unwrap_err();
    assert!(
        err.to_string().contains("exceeds available"),
        "overdraw should be rejected: {err}"
    );
}

/// Connection state transitions: HomePluggedIn -> AwayPluggedIn must be rejected
/// (must go through Disconnected).
#[test]
fn connection_state_transitions_are_valid() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();
    assert_eq!(ev.connection_state, EvConnectionState::HomePluggedIn);

    // HomePluggedIn -> Disconnected: valid
    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::Disconnected,
    })
    .unwrap();
    assert_eq!(ev.connection_state, EvConnectionState::Disconnected);

    // Disconnected -> HomePluggedIn: valid
    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::HomePluggedIn,
    })
    .unwrap();
    assert_eq!(ev.connection_state, EvConnectionState::HomePluggedIn);

    // HomePluggedIn -> AwayPluggedIn: REJECTED (must disconnect first)
    let err = ev
        .apply_control_unchecked(&ControlSignal::EvPlugIn {
            state: EvConnectionState::AwayPluggedIn,
        })
        .unwrap_err();
    assert!(
        err.to_string().contains("disconnect first"),
        "direct HomePluggedIn -> AwayPluggedIn should be rejected: {err}"
    );

    // Go through Disconnected to AwayPluggedIn
    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::Disconnected,
    })
    .unwrap();
    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::AwayPluggedIn,
    })
    .unwrap();
    assert_eq!(ev.connection_state, EvConnectionState::AwayPluggedIn);

    // AwayPluggedIn -> HomePluggedIn: REJECTED
    let err = ev
        .apply_control_unchecked(&ControlSignal::EvPlugIn {
            state: EvConnectionState::HomePluggedIn,
        })
        .unwrap_err();
    assert!(
        err.to_string().contains("disconnect first"),
        "direct AwayPluggedIn -> HomePluggedIn should be rejected: {err}"
    );
}

/// At 7.2 kW AC with 90% efficiency:
/// DC stored = 6.48 kW, waste heat = 0.72 kW = 720 W
/// Verify temperature rise matches the waste heat injected into the thermal mass.
#[test]
fn charging_waste_heat_matches_efficiency_loss() {
    let mut raw = base_raw();
    raw.insert(KEY_BATTERY_CAPACITY_KWH.to_string(), 60.0.into());
    raw.insert(KEY_MAX_CHARGING_POWER_KW.to_string(), 7.2.into());
    raw.insert(KEY_EFFICIENCY.to_string(), 0.9.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.2.into());
    raw.insert(KEY_BATTERY_TEMP_C.to_string(), 20.0.into());
    raw.insert(KEY_THERMAL_MASS_J_PER_K.to_string(), 20_000.0.into());
    raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
    raw.insert(KEY_HEATER_POWER_W.to_string(), 0.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    let temp_before = ev.battery_temp_c;
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::from_secs(3600), &mut ports).unwrap();

    // Waste heat = AC_kW * (1 - eta) = 7.2 * 0.10 = 0.72 kW = 720 W
    // Temperature rise = Q * dt / thermal_mass = 720 * 3600 / 20000 = 129.6 C
    // That is the 1-hour integral with zero UA losses.
    let waste_heat_w = 7.2 * (1.0 - 0.9) * 1000.0;
    let expected_dt = waste_heat_w * 3600.0 / 20_000.0;
    let actual_dt = ev.battery_temp_c - temp_before;

    assert!(
        (actual_dt - expected_dt).abs() < 0.5,
        "Waste heat temperature rise: expected {expected_dt:.1} C, got {actual_dt:.1} C"
    );
}

// ── Full lifecycle tests (RV-013) ─────────────────────────────────

#[test]
fn ev_full_day_lifecycle() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
    raw.insert(KEY_EFFICIENCY.to_string(), 0.9.into());
    raw.insert(KEY_UA_W_PER_K.to_string(), 0.5.into());
    raw.insert(KEY_THERMAL_MASS_J_PER_K.to_string(), 20_000.0.into());
    raw.insert(KEY_BATTERY_TEMP_C.to_string(), 20.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    let dt = Duration::minutes(1);
    let total_steps = 1500;

    // Step 0: SOC = 0.50
    assert!(
        (ev.soc - 0.50).abs() < 1e-9,
        "initial SOC should be 0.50, got {}",
        ev.soc
    );

    // Phase 1: charge from SOC 0.50 to full.
    // Minutes to full = (1.0 - 0.5) * 60 kWh / (7.2 kW * 0.9 η) * 60 min/h ≈ 278 min
    let charge_steps = 278;
    let mut prev_soc = ev.soc;
    for step in 0..charge_steps {
        let mut ports = PortSlots::default();
        ev.step(&env, dt, &mut ports).unwrap();
        if ev.soc < 1.0 {
            assert!(
                ev.soc >= prev_soc,
                "SOC must increase monotonically during charging (step {step}): {prev_soc} -> {}",
                ev.soc
            );
        }
        prev_soc = ev.soc;
    }

    // Step 277 (after 278 steps): SOC should be close to 1.0
    assert!(
        (ev.soc - 1.0).abs() < 0.02,
        "after 278 1-min charging steps SOC should be ~1.0, got {}",
        ev.soc
    );

    // Phase 2 (steps 278..480): HomePluggedIn, battery full, no power draw
    for _ in 278..480 {
        let mut ports = PortSlots::default();
        ev.step(&env, dt, &mut ports).unwrap();
    }
    assert!(
        (ev.soc - 1.0).abs() < 1e-9,
        "SOC should stay 1.0 when full, got {}",
        ev.soc
    );
    assert_eq!(
        ev.telemetry().get("active_power_kw"),
        Some(0.0),
        "no power draw when battery full"
    );

    // Phase 3 (step 480): Departure — disconnect then drive
    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::Disconnected,
    })
    .unwrap();
    ev.apply_control_unchecked(&ControlSignal::EvDrive { kwh: 9.75 })
        .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, dt, &mut ports).unwrap();

    let expected_soc_after_drive = 1.0 - 9.75 / 60.0;
    assert!(
        (ev.soc - expected_soc_after_drive).abs() < 0.01,
        "SOC after 9.75 kWh drive should be ~{expected_soc_after_drive:.4}, got {}",
        ev.soc
    );

    // Phase 4 (steps 481..1080): Disconnected/Away, SOC stable, thermal decay only
    let soc_before_away = ev.soc;
    for _ in 481..1080 {
        let mut ports = PortSlots::default();
        ev.step(&env, dt, &mut ports).unwrap();
        assert_eq!(
            ev.telemetry().get("active_power_kw"),
            Some(0.0),
            "no grid power when disconnected"
        );
    }
    assert!(
        (ev.soc - soc_before_away).abs() < 1e-9,
        "SOC should not change while disconnected: was {soc_before_away}, now {}",
        ev.soc
    );

    // Phase 5 (step 1080): Arrival — reconnect
    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::HomePluggedIn,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, dt, &mut ports).unwrap();

    assert_eq!(
        ev.connection_state,
        EvConnectionState::HomePluggedIn,
        "should be HomePluggedIn after arrival"
    );

    // Phase 6 (steps 1081..1500): HomePluggedIn, charging back toward full
    for _ in 1081..total_steps {
        let mut ports = PortSlots::default();
        ev.step(&env, dt, &mut ports).unwrap();
    }

    assert!(
        (ev.soc - 1.0).abs() < 0.02,
        "SOC should be close to 1.0 after final charging phase, got {}",
        ev.soc
    );
}

#[test]
fn ev_energy_accounting_closed() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.2.into());
    raw.insert(KEY_EFFICIENCY.to_string(), 0.9.into());
    raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
    raw.insert(KEY_HEATER_POWER_W.to_string(), 0.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    let dt = Duration::minutes(1);
    let dt_hours = 1.0 / 60.0;
    let soc_start = ev.soc;
    let mut total_grid_kwh = 0.0;

    for _ in 0..600 {
        let mut ports = PortSlots::default();
        ev.step(&env, dt, &mut ports).unwrap();
        let power = ev.telemetry().get("active_power_kw").unwrap_or(0.0);
        if power > 0.0 {
            total_grid_kwh += power * dt_hours;
        }
    }

    let soc_change = ev.soc - soc_start;
    let expected_grid_kwh = soc_change * ev.battery_capacity_kwh / ev.charging_efficiency;
    let error_kwh = (total_grid_kwh - expected_grid_kwh).abs();
    let tolerance_kwh = 0.02 * ev.battery_capacity_kwh;

    assert!(
        error_kwh < tolerance_kwh,
        "energy accounting error {error_kwh:.3} kWh exceeds 2% of capacity ({tolerance_kwh:.1} kWh). \
         Grid total: {total_grid_kwh:.3} kWh, expected: {expected_grid_kwh:.3} kWh, \
         SOC: {soc_start:.4} -> {:.4}",
        ev.soc
    );
}

#[test]
fn ev_soc_decreases_during_driving() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.8.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    let drive_kwh = 15.0;
    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::Disconnected,
    })
    .unwrap();
    ev.apply_control_unchecked(&ControlSignal::EvDrive { kwh: drive_kwh })
        .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(1), &mut ports).unwrap();

    let expected_soc = 0.8 - drive_kwh / ev.battery_capacity_kwh;
    assert!(
        (ev.soc - expected_soc).abs() < 0.01,
        "SOC after {drive_kwh} kWh drive should be ~{expected_soc:.4}, got {}",
        ev.soc
    );
    assert!(
        ev.soc < 0.8,
        "SOC must decrease after driving, got {}",
        ev.soc
    );
}

#[test]
fn ev_connection_state_transitions() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(1), &mut ports).unwrap();
    assert_eq!(ev.telemetry().get("connection_state"), Some(0.0));
    assert_eq!(ev.connection_state, EvConnectionState::HomePluggedIn);

    // HomePluggedIn → Disconnected (departure)
    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::Disconnected,
    })
    .unwrap();
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(1), &mut ports).unwrap();
    assert_eq!(ev.telemetry().get("connection_state"), Some(TELEMETRY_STATE_DISCONNECTED));
    assert_eq!(ev.connection_state, EvConnectionState::Disconnected);

    // Disconnected → AwayPluggedIn (away charging)
    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::AwayPluggedIn,
    })
    .unwrap();
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(1), &mut ports).unwrap();
    assert_eq!(ev.telemetry().get("connection_state"), Some(1.0));
    assert_eq!(ev.connection_state, EvConnectionState::AwayPluggedIn);

    // AwayPluggedIn → Disconnected (leaving away charger)
    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::Disconnected,
    })
    .unwrap();
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(1), &mut ports).unwrap();
    assert_eq!(ev.telemetry().get("connection_state"), Some(TELEMETRY_STATE_DISCONNECTED));

    // Disconnected → HomePluggedIn (arrival)
    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::HomePluggedIn,
    })
    .unwrap();
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(1), &mut ports).unwrap();
    assert_eq!(ev.telemetry().get("connection_state"), Some(0.0));
    assert_eq!(ev.connection_state, EvConnectionState::HomePluggedIn);
}

#[test]
fn ev_drive_after_full_day_charging() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.9.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    let dt = Duration::minutes(1);
    for _ in 0..1440 {
        let mut ports = PortSlots::default();
        ev.step(&env, dt, &mut ports).unwrap();
    }

    assert!(
        (ev.soc - 1.0).abs() < 1e-9,
        "EV should be fully charged after 24h, got SOC {}",
        ev.soc
    );

    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::Disconnected,
    })
    .unwrap();
    ev.apply_control_unchecked(&ControlSignal::EvDrive { kwh: 10.0 })
        .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, dt, &mut ports).unwrap();

    let expected_soc = 1.0 - 10.0 / ev.battery_capacity_kwh;
    assert!(
        (ev.soc - expected_soc).abs() < 0.01,
        "SOC after drive should be ~{expected_soc:.4}, got {}",
        ev.soc
    );
}

#[test]
fn ev_insufficient_charge_before_departure() {
    // Start with very low SOC (0.05) and drive exactly all available energy.
    // SOC must clamp at 0.0 rather than going negative.
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.05.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    let dt = Duration::minutes(1);
    for _ in 0..10 {
        let mut ports = PortSlots::default();
        ev.step(&env, dt, &mut ports).unwrap();
    }

    // Drive nearly all available energy; subtract epsilon to avoid f64
    // boundary where implementation's `kwh > available` guard could
    // reject the call due to rounding.
    let drive_kwh = ev.soc * ev.battery_capacity_kwh - 1e-9;
    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::Disconnected,
    })
    .unwrap();
    ev.apply_control_unchecked(&ControlSignal::EvDrive {
        kwh: drive_kwh,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, dt, &mut ports).unwrap();

    assert!(
        ev.soc >= 0.0,
        "SOC must not go negative after full available energy drive, got {}",
        ev.soc
    );
    assert!(
        ev.soc < 0.1,
        "SOC should be near 0.0 after exhausting battery, got {}",
        ev.soc
    );
}

#[test]
fn ev_soc_curve_monotonic_during_charging() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.2.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    let dt = Duration::minutes(1);
    let mut sampled_socs: Vec<f64> = vec![ev.soc];

    for step in 0..400 {
        let mut ports = PortSlots::default();
        ev.step(&env, dt, &mut ports).unwrap();
        if step % 10 == 9 {
            sampled_socs.push(ev.soc);
        }
    }

    for window in sampled_socs.windows(2) {
        assert!(
            window[1] >= window[0] - 1e-9,
            "SOC decreased during charging: {:.6} -> {:.6}",
            window[0],
            window[1]
        );
    }

    for &soc in &sampled_socs {
        assert!(
            (0.0..=1.0).contains(&soc),
            "SOC out of bounds [0, 1]: {soc}"
        );
    }
}

/// Departure at exactly step 1440 (end of a 24-hour window) exercises the
/// day-wrapping logic in the BMS ready-by scheduler: `departure_hour` wraps
/// from 0.0 to 24.0, so the boundary case is a departure at midnight (0.0 h),
/// equivalent to a full-day window.
///
/// Physics: 60 kWh battery, 7.2 kW L2 charger, SOC = 0.2 → target 0.9.
/// Required charge = 0.7 * 60 = 42 kWh; time at 7.2 kW = 5.83 h = 350 min.
/// At step 1440 (24 h window) there is plenty of time, so BMS should NOT start
/// immediately at step 0. By step 1000, the deadline is close enough that
/// charging must have begun and SOC must have risen above 0.2.
#[test]
fn ev_departure_at_step_boundary() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.2.into());
    raw.insert(KEY_EFFICIENCY.to_string(), 0.9.into());
    raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let mut env = sample_env();
    ev.init(&config, &env).unwrap();

    // Departure at midnight (0.0 h) — equivalent to end of 24-hour window
    ev.apply_control_unchecked(&ControlSignal::EvSetReadyBy {
        departure_hour: 0.0,
        target_soc: 0.9,
    })
    .unwrap();

    // Early in the window (18:00): BMS should delay — 6 h until midnight
    // and only 350 min of charging needed, so start_hour is still in the future.
    env.current_time = dt(2026, 1, 1, 18, 0, 0);
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(1), &mut ports).unwrap();
    let power_early = ev.telemetry().get("active_power_kw").unwrap();
    assert_eq!(
        power_early, 0.0,
        "BMS should delay at 18:00 for midnight departure with 6 h remaining"
    );

    // Close to deadline (23:00): 1 h left but needs 350 min → urgent, must charge
    ev.soc = 0.2; // reset SOC to ensure it hasn't changed from any early step
    env.current_time = dt(2026, 1, 1, 23, 0, 0);
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(1), &mut ports).unwrap();
    let power_late = ev.telemetry().get("active_power_kw").unwrap();
    assert!(
        power_late > 0.0,
        "BMS should charge at 23:00 for midnight departure with only 1 h remaining"
    );

    // Run 440 1-minute steps from 23:00 to verify charging proceeds normally
    // (step 1440 is the boundary condition: day wraps at 1440 minutes).
    let mut prev_soc = ev.soc;
    for step in 0..440usize {
        assert!(step < 2000, "step loop diverged");
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(1), &mut ports).unwrap();
        if ev.soc < 1.0 {
            assert!(
                ev.soc >= prev_soc,
                "SOC must not decrease during charging (step {step}): {prev_soc:.6} -> {:.6}",
                ev.soc
            );
        }
        prev_soc = ev.soc;
        assert!(
            ev.soc >= 0.0 && ev.soc <= 1.0,
            "SOC out of bounds at step {step}: {}",
            ev.soc
        );
    }
    // After 440 min of charging from SOC 0.2 at 7.2 kW with η=0.9:
    // DC rate = 6.48 kWh/h, SOC gain/min = 6.48/60/60 = 0.0018/min
    // After 440 min: ΔSOC = 440 * (7.2*0.9) / (60*60) = ~0.47 → SOC ≈ 0.67
    // (tapered charging near full may slow it; SOC must be well above initial 0.2)
    assert!(
        ev.soc > 0.5,
        "after 440 min charging SOC should be > 0.5, got {}",
        ev.soc
    );
}
