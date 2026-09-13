use std::collections::HashMap;
use std::time::Duration;

use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
use hares_types::{
    ControlSignal, DRLevel, EnvironmentState, EvConnectionState, GridState, PortSlots,
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
            ground_t_mean_c: 10.0,
            ground_t_amplitude_c: 0.0,
            ground_phase_day: 35.0,
            day_of_year: 1.0,
        },
        grid: GridState {
            voltage_pu: 1.0,
            frequency_hz: 60.0,
            island_bus_voltage_pu: None,
        },
        custom_domains: vec![],
        equipment_telemetry: std::collections::HashMap::new(),
        equipment_core: std::collections::HashMap::new(),
        current_time: dt(2026, 1, 1, 0, 0, 0),
        time_res: ChronoDuration::minutes(1),
        price_signal: Default::default(),
        electrical: Default::default(),
    }
}

fn ev_config(raw: HashMap<String, crate::config::ConfigValue>) -> EquipmentConfig {
    let get_f64 = |keys: &[&str]| {
        keys.iter()
            .find_map(|key| raw.get(*key).and_then(crate::config::ConfigValue::as_f64))
    };
    let get_bool = |key: &str| raw.get(key).and_then(crate::config::ConfigValue::as_bool);
    let get_str = |keys: &[&str]| {
        keys.iter().find_map(|key| {
            raw.get(*key)
                .and_then(crate::config::ConfigValue::as_str)
                .map(str::to_string)
        })
    };

    let charging_level = get_str(&[KEY_CHARGING_LEVEL, KEY_CHARGING_LEVEL_HPXML]).map(|level| {
        match level.as_str() {
            "Level1" => "L1".to_string(),
            "Level2" => "L2".to_string(),
            _ => level,
        }
    });

    let capacity_kwh = get_f64(&[KEY_BATTERY_CAPACITY_KWH, KEY_BATTERY_CAPACITY_HPXML_KWH])
        .or_else(|| {
            get_f64(&[KEY_RANGE_MILES]).map(|range| {
                let economy = get_f64(&[KEY_FUEL_ECONOMY_KWH_PER_MI])
                    .unwrap_or(DEFAULT_FUEL_ECONOMY_KWH_PER_MI);
                range * economy
            })
        })
        .unwrap_or(DEFAULT_CAPACITY_KWH);

    let default_level = charging_level.clone().unwrap_or_else(|| "L2".to_string());
    let max_charging_power_kw =
        get_f64(&[KEY_MAX_CHARGING_POWER_KW, KEY_MAX_CHARGING_POWER_HPXML_KW]).unwrap_or_else(
            || match default_level.as_str() {
                "L1" => L1_CHARGING_POWER_KW,
                _ => default_max_power_kw(
                    &EquipmentConfig::raw("EV #1".to_string(), "EV".to_string(), raw.clone()),
                    hares_types::ChargingLevel::L2,
                    capacity_kwh,
                ),
            },
        );

    EquipmentConfig::from_typed(
        "EV #1".to_string(),
        "EV".to_string(),
        EvConfig {
            equipment_id: raw
                .get(KEY_EQUIPMENT_ID)
                .and_then(crate::config::ConfigValue::as_f64)
                .map(|value| value as u32),
            capacity_kwh,
            charging_level,
            max_charging_power_kw,
            charging_efficiency: get_f64(&[KEY_EFFICIENCY]),
            l1_current_a: get_f64(&[KEY_L1_CURRENT_A]),
            l1_voltage_v: get_f64(&[KEY_L1_VOLTAGE_V]),
            soc_max: get_f64(&[KEY_SOC_MAX]),
            initial_soc: get_f64(&[KEY_INITIAL_SOC]),
            battery_temp_c: get_f64(&[KEY_BATTERY_TEMP_C]),
            min_charge_temp_c: get_f64(&[KEY_MIN_CHARGE_TEMP_C]),
            full_power_temp_c: get_f64(&[KEY_FULL_POWER_TEMP_C]),
            heater_power_w: get_f64(&[KEY_HEATER_POWER_W]),
            heater_threshold_c: get_f64(&[KEY_HEATER_THRESHOLD_C]),
            thermal_mass_j_per_k: get_f64(&[KEY_THERMAL_MASS_J_PER_K]),
            ua_w_per_k: get_f64(&[KEY_UA_W_PER_K]),
            v2l_enabled: get_bool(KEY_V2L_ENABLED),
            v2l_soc_reserve: get_f64(&[KEY_V2L_SOC_RESERVE]),
            v2l_max_discharge_kw: get_f64(&[KEY_V2L_MAX_DISCHARGE_KW]),
            v2g_enabled: get_bool(KEY_V2G_ENABLED),
            v2g_soc_reserve: get_f64(&[KEY_V2G_SOC_RESERVE]),
            v2g_max_discharge_kw: get_f64(&[KEY_V2G_MAX_DISCHARGE_KW]),
            chemistry: get_str(&[KEY_CHEMISTRY]),
            fuel_economy_kwh_per_mi: get_f64(&[KEY_FUEL_ECONOMY_KWH_PER_MI]),
            ready_soc: get_f64(&[KEY_READY_SOC]),
            charging_strategy: get_str(&[KEY_CHARGING_STRATEGY]),
            plug_in_policy: get_str(&[KEY_PLUG_IN_POLICY]),
            power_limit_kw: get_f64(&[KEY_POWER_LIMIT_KW]),
            initial_connection_state: get_str(&[KEY_INITIAL_CONNECTION_STATE]),
            power_factor: get_f64(&[KEY_POWER_FACTOR]),
            charger_capacity_kva: get_f64(&[KEY_CHARGER_CAPACITY_KVA]),
            cc_cv_transition_soc: get_f64(&[KEY_CC_CV_TRANSITION_SOC]),
            charging_priority: get_str(&[KEY_CHARGING_PRIORITY]).map(|s| match s.as_str() {
                "ExternalAuthority" => ChargingPriority::ExternalAuthority,
                _ => ChargingPriority::DeadlineGuarantee,
            }),
            discharge_respects_deadline: true,
        },
    )
    .unwrap()
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
    assert_eq!(
        ev.telemetry().get("connection_state"),
        Some(TELEMETRY_STATE_DISCONNECTED)
    );
    assert_eq!(ports.electrical.load_power_w, 0.0);
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
            min_soc: None,
            max_soc: None,
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

    let checkpoint = a.save_state().unwrap();
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
    let registry = EquipmentRegistry::new();

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
    raw.insert(KEY_BATTERY_CAPACITY_HPXML_KWH.to_string(), 64.0.into());
    raw.insert(KEY_MAX_CHARGING_POWER_HPXML_KW.to_string(), 11.5.into());
    raw.insert(KEY_CHARGING_LEVEL_HPXML.to_string(), "Level2".into());

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
    ev_no_heater.init(&config_no_heater, &sample_env()).unwrap();
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
        min_soc: None,
        max_soc: None,
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
        min_soc: None,
        max_soc: None,
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
        min_soc: None,
        max_soc: None,
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
            min_soc: None,
            max_soc: None,
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
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

    assert!(
        ports.electrical.generation_power_w < 0.0,
        "V2L discharge should appear in generation_power_w, got {}",
        ports.electrical.generation_power_w
    );
    assert!(
        ports.electrical.generation_power_w.abs() <= 3_000.0 + 10.0,
        "V2L must not exceed max_discharge_kw={}, got {}",
        3.0,
        ports.electrical.generation_power_w.abs()
    );
    assert_eq!(
        ports.electrical.load_power_w, 0.0,
        "V2L discharge should not appear in load_power_w"
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
        min_soc: None,
        max_soc: None,
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
        min_soc: None,
        max_soc: None,
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
        min_soc: None,
        max_soc: None,
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

#[test]
fn v2g_respects_power_setpoint_min_soc_above_reserve() {
    let mut raw = base_raw();
    raw.insert(KEY_V2G_ENABLED.to_string(), true.into());
    raw.insert(KEY_V2G_SOC_RESERVE.to_string(), 0.2.into());
    raw.insert(KEY_V2G_MAX_DISCHARGE_KW.to_string(), 5.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.30.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).expect("init");

    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_kw: -5.0,
        reactive_power_kvar: None,
        min_soc: Some(0.35),
        max_soc: None,
    })
    .expect("setpoint");

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports)
        .expect("step");
    let power = ev.telemetry().get("active_power_kw").unwrap_or(0.0);
    assert!(
        power.abs() < 0.01,
        "V2G must not discharge when SOC=0.30 below power_setpoint_min_soc=0.35 (effective_floor=max(0.2, 0.35)=0.35), got {power}"
    );
}

#[test]
fn v2g_discharges_when_soc_above_power_setpoint_min_soc() {
    let mut raw = base_raw();
    raw.insert(KEY_V2G_ENABLED.to_string(), true.into());
    raw.insert(KEY_V2G_SOC_RESERVE.to_string(), 0.2.into());
    raw.insert(KEY_V2G_MAX_DISCHARGE_KW.to_string(), 5.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.50.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).expect("init");

    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_kw: -5.0,
        reactive_power_kvar: None,
        min_soc: Some(0.35),
        max_soc: None,
    })
    .expect("setpoint");

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports)
        .expect("step");
    let power = ev.telemetry().get("active_power_kw").unwrap_or(0.0);
    assert!(
        power < -0.1,
        "V2G should discharge when SOC=0.50 above power_setpoint_min_soc=0.35, got {power}"
    );
}

#[test]
fn v2l_respects_power_setpoint_min_soc_above_reserve() {
    let mut raw = base_raw();
    raw.insert(KEY_V2L_ENABLED.to_string(), true.into());
    raw.insert(KEY_V2L_SOC_RESERVE.to_string(), 0.2.into());
    raw.insert(KEY_V2L_MAX_DISCHARGE_KW.to_string(), 5.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.25.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).expect("init");

    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_kw: -3.0,
        reactive_power_kvar: None,
        min_soc: Some(0.30),
        max_soc: None,
    })
    .expect("setpoint");

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports)
        .expect("step");
    let power = ev.telemetry().get("active_power_kw").unwrap_or(0.0);
    assert!(
        power.abs() < 0.01,
        "V2L must not discharge when SOC=0.25 below power_setpoint_min_soc=0.30, got {power}"
    );
}

#[test]
fn power_setpoint_max_soc_caps_charging() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.85.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).expect("init");

    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_kw: 7.2,
        reactive_power_kvar: None,
        min_soc: None,
        max_soc: Some(0.90),
    })
    .expect("setpoint");

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports)
        .expect("step");
    let p_full = ev.telemetry().get("active_power_kw").unwrap();
    assert!(
        p_full < 7.2,
        "charging should be capped near power_setpoint_max_soc=0.90, got {p_full}"
    );
}

// ── 4D LUT and degradation tests ─────────────────────────────────

fn make_4d_lut(soc_pf: &[(f64, f32)]) -> crate::ndinterp::RegularGridInterpolator {
    let soc_grid: Vec<f64> = soc_pf.iter().map(|(s, _)| *s).collect();
    let values: Vec<f32> = soc_pf.iter().map(|(_, p)| *p).collect();
    crate::ndinterp::RegularGridInterpolator::new(
        vec![soc_grid, vec![25.0], vec![1.0], vec![1.0]],
        values,
        crate::ndinterp::ExtrapolationStrategy::Clamp,
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
        crate::ndinterp::ExtrapolationStrategy::Clamp,
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
    assert_eq!(ev.degradation.capacity_fade_fraction(), 0.0);
}

#[test]
fn capacity_fade_fraction_in_telemetry() {
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

    let saved = ev1.save_state().unwrap();
    let mut ev2 = Ev::new(config.clone());
    ev2.init(&config, &env).unwrap();
    ev2.load_state(&saved).unwrap();
    assert_eq!(
        ev2.degradation.capacity_fade_fraction(),
        ev1.degradation.capacity_fade_fraction(),
    );
}

/// Ensures the first timestep of each new day is accumulated into the new
/// day's degradation accumulators, not lost to the previous day.
///
/// The pre-fix bug: Ev::update_degradation() ran rainflow.push() and
/// degradation.accumulate() *before* the day-boundary check.  The first
/// step of day 2 was accumulated into day 1's b1_accum, then lost when
/// update_daily() reset it — leaving day 2's b1_accum at zero.
///
/// This test exercises the full Ev::step() code path.  It runs a full day
/// of steps, then one step into the next day, and asserts that b1_accum
/// is non-zero — the first step of the new day was accumulated correctly.
/// A revert of the fix would leave b1_accum at zero because the step
/// content is cleared by the day-boundary reset.
///
/// The Battery equivalent test (`first_timestep_of_new_day_is_not_lost`)
/// is at battery/mod.rs:6662.
#[test]
fn ev_q_li1_independent_of_steps_per_day_across_midnight_boundaries() {
    let dt = Duration::from_secs(300);
    let steps_per_day = 288usize;

    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
    raw.insert(KEY_SOC_MAX.to_string(), 1.0.into());
    raw.insert(KEY_BATTERY_TEMP_C.to_string(), 25.0.into());
    raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
    raw.insert(KEY_HEATER_POWER_W.to_string(), 0.0.into());
    raw.insert(KEY_HEATER_THRESHOLD_C.to_string(), 50.0.into());
    let config = ev_config(raw);
    let mut env = sample_env();
    let mut ev = Ev::new(config.clone());
    ev.init(&config, &env).unwrap();
    ev.last_daily_update_day = {
        use chrono::Datelike;
        env.current_time.date_naive().num_days_from_ce()
    };

    let mut ports = PortSlots::default();

    // Day 1: full day of steps.
    for _ in 0..steps_per_day {
        ports.zero();
        ev.step(&env, dt, &mut ports).unwrap();
        env.current_time += ChronoDuration::seconds(dt.as_secs() as i64);
    }

    // Day 2: one step — this triggers the boundary update for day 1,
    // then accumulates into day 2's b1_accum.
    ports.zero();
    ev.step(&env, dt, &mut ports).unwrap();

    // After the step, b1_accum must be non-zero — the first step of
    // the new day was accumulated into the correct day, not lost.
    assert!(
        ev.degradation.b1_accum.abs() > 0.0,
        "first timestep of new day must accumulate into b1_accum, got {}",
        ev.degradation.b1_accum
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
    let axes = vec![vec![0.0, 1.0], vec![25.0], vec![0.05, 0.30], vec![0.5, 1.0]];
    let values = vec![1.0f32, 1.0, 0.1, 0.1, 1.0, 1.0, 0.1, 0.1];
    let lut = crate::ndinterp::RegularGridInterpolator::new(
        axes,
        values,
        crate::ndinterp::ExtrapolationStrategy::Clamp,
    )
    .unwrap();

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

#[test]
fn ocv_source_hardcoded_on_construction() {
    let config = ev_config(base_raw());
    let ev = Ev::new(config);
    assert_eq!(ev.ocv_source().unwrap(), "hardcoded-NMC-v26.3.0");
}

#[test]
fn ocv_source_changes_after_set_ocv_table() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config);
    let custom = OcvTable::for_chemistry(BatteryChemistry::Lfp);
    ev.set_ocv_table(custom).unwrap();
    assert_eq!(ev.ocv_source().unwrap(), "hardcoded-LFP-v26.3.0");
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

/// Hypothesis: `EvDrive` with negative kWh bypasses the handler's overdraw
/// guard (`kwh > available_kwh` is false for any negative value), so the SOC
/// arithmetic `soc - kwh / battery_capacity_kwh` runs with a negated term and
/// *raises* SOC — energy from nowhere. The equipment's direct control surface
/// (`apply_control_unchecked`, a public trait method) performs no payload
/// validation of its own. Physical invariant: a drive command must never
/// increase SOC, whether the signal is accepted or rejected.
#[test]
fn ev_drive_negative_kwh_must_not_increase_soc() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();
    let soc_before = ev.telemetry().get("soc").unwrap();

    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::Disconnected,
    })
    .unwrap();
    let _ = ev.apply_control_unchecked(&ControlSignal::EvDrive { kwh: -5.0 });

    // Observe through a step while Disconnected (thermal drift only; the
    // step never touches SOC) so telemetry reflects post-drive state.
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();

    let soc_after = ev.telemetry().get("soc").unwrap();
    assert!(
        soc_after <= soc_before + 1e-12,
        "negative drive kWh created energy from nowhere: SOC rose from {soc_before} to {soc_after}"
    );
    assert!(
        soc_after.is_finite() && (0.0..=1.0).contains(&soc_after),
        "SOC must stay finite and within [0, 1], got {soc_after}"
    );
}

/// Hypothesis: `EvDrive` with NaN kWh slips past the `kwh > available_kwh`
/// guard (NaN compares false against everything) and
/// `(soc - NaN / capacity).clamp(0.0, 1.0)` propagates NaN (f64::clamp
/// returns NaN unchanged), permanently poisoning SOC. Manifestations: in
/// debug builds the next step panics in `Telemetry::set` (non-finite SOC);
/// in plain release builds telemetry silently freezes at the last finite
/// SOC while the taper math `(soc_limit - NaN).max(0.0)` collapses to 0 kW,
/// permanently disabling charging of a plugged-in EV below target.
/// Physical invariants: SOC stays finite and within [0, 1]; a plugged-in EV
/// below its target keeps drawing charge power.
#[test]
fn ev_drive_nan_kwh_must_not_poison_soc_or_power() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::Disconnected,
    })
    .unwrap();
    let _ = ev.apply_control_unchecked(&ControlSignal::EvDrive { kwh: f64::NAN });

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();

    let soc = ev.telemetry().get("soc").unwrap();
    assert!(
        soc.is_finite() && (0.0..=1.0).contains(&soc),
        "NaN drive kWh poisoned SOC: got {soc}"
    );

    // The poison must not leak into the power path either: plug back in at
    // home and step — a healthy EV at 20% SOC with no setpoint must charge.
    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::HomePluggedIn,
    })
    .unwrap();
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();

    let soc = ev.telemetry().get("soc").unwrap();
    assert!(
        soc.is_finite() && (0.0..=1.0).contains(&soc),
        "SOC must remain finite after a charging step, got {soc}"
    );
    let power = ev.telemetry().get("active_power_kw").unwrap();
    assert!(
        power.is_finite() && power > 0.0,
        "NaN-poisoned SOC silently disabled charging: plugged-in EV below \
         target draws {power} kW (healthy config draws ~7.2 kW)"
    );
}

/// Hypothesis: the EvDrive overdraw guard `kwh > available_kwh` is one-sided.
/// Positive infinity is rejected (it exceeds available energy), but negative
/// infinity passes (it is never greater than a finite bound) and
/// `(soc - (-inf) / capacity).clamp(0.0, 1.0)` evaluates to exactly 1.0 — a
/// free full charge from negative-infinite "driving". Physical invariant: a
/// drive command must never increase SOC.
#[test]
fn ev_drive_infinite_kwh_must_not_create_energy() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();
    let soc_before = ev.telemetry().get("soc").unwrap();

    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::Disconnected,
    })
    .unwrap();

    // Positive infinity: the available-energy guard must reject it outright.
    let result = ev.apply_control_unchecked(&ControlSignal::EvDrive { kwh: f64::INFINITY });
    assert!(
        result.is_err(),
        "+inf drive kWh must be rejected by the available-energy guard"
    );
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();
    let soc = ev.telemetry().get("soc").unwrap();
    assert!(
        (soc - soc_before).abs() < 1e-12,
        "rejected +inf drive must leave SOC unchanged, got {soc} (was {soc_before})"
    );

    // Negative infinity: the same guard is blind to it — SOC must not rise.
    let _ = ev.apply_control_unchecked(&ControlSignal::EvDrive {
        kwh: f64::NEG_INFINITY,
    });
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();
    let soc = ev.telemetry().get("soc").unwrap();
    assert!(
        soc <= soc_before + 1e-12,
        "-inf drive kWh created a free full charge: SOC rose from {soc_before} to {soc}"
    );
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

    assert!(
        ev.soc > soc_before,
        "SOC should increase from away charging"
    );
    assert_eq!(
        ev.telemetry().get("active_power_kw"),
        Some(0.0),
        "residential power should be 0 during away charging"
    );
    assert_eq!(ports.electrical.load_power_w, 0.0);
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
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    let soc_before = ev.soc;
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();
    assert_eq!(
        ev.soc, soc_before,
        "PowerSetpoint=0 should suppress charging"
    );
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

    // Step at 18:00 -- should be too early, BMS delays
    env.current_time = dt(2026, 1, 1, 18, 0, 0);
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    let power_early = ev.telemetry().get("active_power_kw").unwrap();

    // Step at 5:00 -- should be charging (close to deadline)
    ev.soc = 0.3;
    env.current_time = dt(2026, 1, 2, 5, 0, 0);
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    let power_late = ev.telemetry().get("active_power_kw").unwrap();

    assert_eq!(
        power_early, 0.0,
        "BMS should delay charging early in the evening"
    );
    assert!(power_late > 0.0, "BMS should be charging close to deadline");
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
    assert_eq!(
        power, 0.0,
        "already above target SOC should produce 0 power"
    );
}

#[test]
fn deadline_passed_charges_immediately() {
    // Deadline at 14:00, currently 15:00 -- unambiguously past the deadline
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

    // Deadline at exactly current hour -- triggers immediate charge
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
    assert!(power > 0.0, "negative start_hour should charge immediately");
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

// ── ChargingPriority: deadline‑vs‑setpoint interaction tests ──────

/// With `DeadlineGuarantee` (the default), an urgent deadline overrides
/// a low external PowerSetpoint. The BMS raises power above the setpoint
/// to meet the departure SOC.
///
/// Setup: 60 kWh battery, 7.2 kW L2, SOC=0.2, target_soc=0.9,
/// departure in 2 h. The BMS needs ~6.5 h at full rate, so the deadline
/// is urgent. An external setpoint of 1.0 kW must be overridden.
#[test]
fn deadline_guarantee_raises_power_above_setpoint_when_urgent() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.2.into());
    raw.insert(KEY_EFFICIENCY.to_string(), 0.9.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let mut env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control_unchecked(&ControlSignal::EvSetReadyBy {
        departure_hour: 2.0,
        target_soc: 0.9,
    })
    .unwrap();
    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_kw: 1.0,
        reactive_power_kvar: None,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    env.current_time = dt(2026, 1, 1, 0, 0, 0);
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    let power = ev.telemetry().get("active_power_kw").unwrap();

    assert!(
        power > 1.0,
        "DeadlineGuarantee should raise power above 1.0 kW setpoint when deadline is urgent, got {power}"
    );
    assert!(
        power > 5.0,
        "should be charging near full rate (~7.2 kW), got {power}"
    );
}

/// With `ExternalAuthority`, the same urgent deadline + low setpoint
/// scenario respects the external setpoint — the BMS deadline logic is
/// bypassed. The external controller bears sole responsibility.
#[test]
fn external_authority_respects_setpoint_despite_urgent_deadline() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.2.into());
    raw.insert(KEY_EFFICIENCY.to_string(), 0.9.into());
    raw.insert(
        KEY_CHARGING_PRIORITY.to_string(),
        "ExternalAuthority".into(),
    );
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let mut env = sample_env();
    ev.init(&config, &env).unwrap();
    assert_eq!(ev.charging_priority, ChargingPriority::ExternalAuthority);

    ev.apply_control_unchecked(&ControlSignal::EvSetReadyBy {
        departure_hour: 2.0,
        target_soc: 0.9,
    })
    .unwrap();
    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_kw: 1.0,
        reactive_power_kvar: None,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    env.current_time = dt(2026, 1, 1, 0, 0, 0);
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    let power = ev.telemetry().get("active_power_kw").unwrap();

    assert!(
        (power - 1.0).abs() < 1e-9,
        "ExternalAuthority should respect 1.0 kW setpoint even with urgent deadline, got {power}"
    );
}

/// With `DeadlineGuarantee`, when the deadline is NOT urgent (plenty of
/// time), the external setpoint is honoured unchanged. The BMS reports
/// no urgency (returns 0.0), so max(0.0, setpoint) = setpoint.
#[test]
fn deadline_guarantee_respects_setpoint_when_not_urgent() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.75.into());
    raw.insert(KEY_EFFICIENCY.to_string(), 0.9.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let mut env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control_unchecked(&ControlSignal::EvSetReadyBy {
        departure_hour: 7.0,
        target_soc: 0.8,
    })
    .unwrap();
    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_kw: 1.0,
        reactive_power_kvar: None,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    // 19:00 with departure at 7:00 — 12 hours remaining, SOC deficit
    // is only 0.05, needs < 1 h. BMS reports no urgency.
    env.current_time = dt(2026, 1, 1, 19, 0, 0);
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    let power = ev.telemetry().get("active_power_kw").unwrap();

    assert!(
        (power - 1.0).abs() < 1e-9,
        "DeadlineGuarantee should respect 1.0 kW setpoint when deadline is not urgent, got {power}"
    );
}

/// Default `charging_priority` is `DeadlineGuarantee` — matches real‑world
/// smart EVSE behaviour where cost optimization yields to departure readiness.
#[test]
fn charging_priority_defaults_to_deadline_guarantee() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();
    assert_eq!(ev.charging_priority, ChargingPriority::DeadlineGuarantee);
}

/// `PowerLimit` is still applied as a final cap after the deadline guarantee
/// max operation. An urgent deadline raises power above the setpoint, but
/// a `PowerLimit` below the BMS‑required power caps the result.
#[test]
fn power_limit_caps_after_deadline_guarantee_max() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.2.into());
    raw.insert(KEY_EFFICIENCY.to_string(), 0.9.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let mut env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control_unchecked(&ControlSignal::EvSetReadyBy {
        departure_hour: 2.0,
        target_soc: 0.9,
    })
    .unwrap();
    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_kw: 1.0,
        reactive_power_kvar: None,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();
    ev.apply_control(&ControlSignal::PowerLimit {
        max_power_kw: 3.0,
        ramp_rate_kw_per_s: None,
    })
    .unwrap();

    env.current_time = dt(2026, 1, 1, 0, 0, 0);
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    let power = ev.telemetry().get("active_power_kw").unwrap();

    assert!(
        power > 1.0,
        "deadline guarantee should raise power above 1.0 kW setpoint, got {power}"
    );
    assert!(
        power <= 3.0 + 1e-9,
        "PowerLimit=3.0 should cap the result, got {power}"
    );
}

/// Regression: with `DeadlineGuarantee`, an EV with a low external setpoint
/// close to its departure deadline still charges at full rate when the BMS
/// determines urgency. The SOC gain significantly exceeds what the setpoint
/// alone would deliver, proving the deadline override is active.
///
/// Setup: 60 kWh, 7.2 kW L2, 0.9 η, SOC=0.3, target_soc=0.5,
/// departure at 5:00. At 1.0 kW setpoint alone, 5 h would add only
/// 0.9×5/60 = 0.075 SOC → 0.375. With deadline guarantee, the BMS
/// becomes urgent partway through and charges at full rate, pushing
/// SOC well above 0.375.
#[test]
fn deadline_guarantee_meets_target_soc_despite_low_setpoint() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.3.into());
    raw.insert(KEY_EFFICIENCY.to_string(), 0.9.into());
    raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
    raw.insert(KEY_HEATER_POWER_W.to_string(), 0.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let mut env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control_unchecked(&ControlSignal::EvSetReadyBy {
        departure_hour: 5.0,
        target_soc: 0.5,
    })
    .unwrap();
    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_kw: 1.0,
        reactive_power_kvar: None,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    env.current_time = dt(2026, 1, 1, 0, 0, 0);
    for _ in 0..20 {
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
        env.current_time += ChronoDuration::minutes(15);
    }

    // With setpoint alone (1.0 kW × 0.9 η × 5 h / 60 kWh = 0.075 SOC):
    // SOC would be ~0.375. Deadline guarantee raises power above the
    // setpoint when the BMS determines urgency, so SOC must be
    // significantly higher.
    assert!(
        ev.soc > 0.45,
        "DeadlineGuarantee should charge above setpoint-only rate, got SOC={} (setpoint-only would be ~0.375)",
        ev.soc
    );
}

/// Under `ExternalAuthority` with an active PowerSetpoint, the BMS
/// CC‑CV taper is not applied to the delivered power (the external
/// setpoint is used verbatim). Verify `cc_cv_derating` reports 1.0
/// even at high SOC, preventing a misleading telemetry signal that
/// would incorrectly attribute a power reduction to CC‑CV tapering.
///
/// Setup: ExternalAuthority, SOC=0.95 (> 0.85 transition), setpoint
/// active, Ready‑By deadline set. No LUT present.
#[test]
fn external_authority_reports_no_cc_cv_derating_when_setpoint_active() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.95.into());
    raw.insert(KEY_EFFICIENCY.to_string(), 0.9.into());
    raw.insert(
        KEY_CHARGING_PRIORITY.to_string(),
        "ExternalAuthority".into(),
    );
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let mut env = sample_env();
    ev.init(&config, &env).unwrap();
    assert_eq!(ev.charging_priority, ChargingPriority::ExternalAuthority);

    ev.apply_control_unchecked(&ControlSignal::EvSetReadyBy {
        departure_hour: 5.0,
        target_soc: 1.0,
    })
    .unwrap();
    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_kw: 1.5,
        reactive_power_kvar: None,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    env.current_time = dt(2026, 1, 1, 3, 0, 0);
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

    assert!(
        ev.cc_cv_derating == 1.0,
        "ExternalAuthority + setpoint: CC‑CV derating must be 1.0 (no taper applied to external setpoint), got {}",
        ev.cc_cv_derating
    );
    let power = ev.telemetry().get("active_power_kw").unwrap();
    assert!(
        (power - 1.5).abs() < 1e-9,
        "ExternalAuthority should deliver the raw setpoint power, got {power}"
    );
}

// ── CC‑CV taper tests ────────────────────────────────────────────

#[test]
fn cc_cv_taper_no_derating_below_transition() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
    raw.insert(KEY_EFFICIENCY.to_string(), 0.9.into());
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

    assert!(
        ev.cc_cv_derating == 1.0,
        "SOC=0.5 is below transition, no CC‑CV taper should apply, got derating={}",
        ev.cc_cv_derating
    );
}

#[test]
fn cc_cv_taper_applies_above_transition() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.95.into());
    raw.insert(KEY_EFFICIENCY.to_string(), 0.9.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let mut env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control_unchecked(&ControlSignal::EvSetReadyBy {
        departure_hour: 5.0,
        target_soc: 1.0,
    })
    .unwrap();

    env.current_time = dt(2026, 1, 1, 3, 0, 0);
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

    assert!(
        ev.cc_cv_derating < 1.0,
        "SOC=0.95 is above transition, CC‑CV taper should apply, got derating={}",
        ev.cc_cv_derating
    );
    let expected = 0.533;
    assert!(
        (ev.cc_cv_derating - expected).abs() < 0.01,
        "taper at SOC=0.95 should be ~{expected}, got {}",
        ev.cc_cv_derating
    );
}

#[test]
fn cc_cv_taper_omitted_when_lut_present() {
    let lut = make_4d_lut(&[(0.0, 1.0), (0.5, 1.0), (1.0, 0.0)]);
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.95.into());
    raw.insert(KEY_EFFICIENCY.to_string(), 0.9.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let mut env = sample_env();
    ev.init(&config, &env).unwrap();
    ev.set_charging_curve_lut(Some(lut)).unwrap();

    ev.apply_control_unchecked(&ControlSignal::EvSetReadyBy {
        departure_hour: 5.0,
        target_soc: 1.0,
    })
    .unwrap();

    env.current_time = dt(2026, 1, 1, 3, 0, 0);
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

    assert_eq!(
        ev.cc_cv_derating, 1.0,
        "LUT present: CC‑CV derating must be 1.0 (LUT already captures power roll-off), got {}",
        ev.cc_cv_derating
    );
}

/// Verify the linear CC-CV taper produces the expected derating at SOC 0.88
/// (just above the 0.85 transition point). The old flat CC_CV_MARGIN=0.85
/// under-estimated charge time at this SOC by applying a 15% derate from the
/// start of the CV region; the new SOC-dependent taper at SOC 0.88 gives a
/// multiplier of 0.86 — a small improvement over the old flat value, but
/// indicative of the fix's primary benefit (no derating below the transition,
/// tested separately in `cc_cv_taper_no_derating_below_transition`).
#[test]
fn soc_88_linear_taper_near_transition() {
    let mut raw = base_raw();
    raw.insert(KEY_BATTERY_CAPACITY_KWH.to_string(), 60.0.into());
    raw.insert(KEY_MAX_CHARGING_POWER_KW.to_string(), 7.2.into());
    raw.insert(KEY_EFFICIENCY.to_string(), 0.9.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.88.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let mut env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control_unchecked(&ControlSignal::EvSetReadyBy {
        departure_hour: 2.0,
        target_soc: 0.90,
    })
    .unwrap();

    env.current_time = dt(2026, 1, 1, 0, 0, 0);

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

    assert!(
        ev.cc_cv_derating < 1.0,
        "SOC=0.88 above transition, taper should apply, got derating={}",
        ev.cc_cv_derating
    );
    // Linear taper: t = (0.88 - 0.85) / 0.15 = 0.2
    // multiplier = 1.0 - 0.7 * 0.2 = 0.86
    let expected = 0.86;
    assert!(
        (ev.cc_cv_derating - expected).abs() < 0.01,
        "taper at SOC=0.88 should be ~{expected}, got {}",
        ev.cc_cv_derating
    );
}

/// Verify the taper multiplier helper at boundary points.
#[test]
fn cc_cv_taper_multiplier_boundaries() {
    use super::Ev;

    let ts = 0.85;

    assert_eq!(Ev::cc_cv_taper_multiplier(0.0, ts), 1.0);
    assert_eq!(Ev::cc_cv_taper_multiplier(ts, ts), 1.0);

    let at_full = Ev::cc_cv_taper_multiplier(1.0, ts);
    assert!(
        (at_full - CC_CV_MIN_MULTIPLIER).abs() < 1e-15,
        "taper at SOC=1.0 should be {CC_CV_MIN_MULTIPLIER}, got {at_full}"
    );

    let mid = Ev::cc_cv_taper_multiplier(0.925, ts);
    assert!(mid > CC_CV_MIN_MULTIPLIER && mid < 1.0);

    assert_eq!(Ev::cc_cv_taper_multiplier(1.0, 1.0), 1.0);
    assert_eq!(Ev::cc_cv_taper_multiplier(0.5, 1.0), 1.0);
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
    assert_eq!(
        ev.telemetry().get("connection_state"),
        Some(TELEMETRY_STATE_DISCONNECTED)
    );
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
    assert_eq!(
        ev.soc, soc_before,
        "zero away charge power should not change SOC"
    );
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
    assert!(caps.contains(ControlCapabilities::DEMAND_RESPONSE));
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

// ── Physics-grounded EV driver lifecycle tests ────────────────────

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

    let step: i32 = full_step.expect("EV should reach full within 400 minutes");
    assert!(
        (step - 278).abs() <= 2,
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
    raw.insert(KEY_FUEL_ECONOMY_KWH_PER_MI.to_string(), 0.325.into());
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
        assert!(ev.soc <= 1.0, "SOC must never exceed 1.0, got {}", ev.soc);
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
    ev.step(&env, Duration::from_secs(3600), &mut ports)
        .unwrap();

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

// ── Full lifecycle tests ───────────────────────────────────────────

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

    // Phase 3 (step 480): Departure -- disconnect then drive
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

    // Phase 5 (step 1080): Arrival -- reconnect
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
    assert_eq!(
        ev.telemetry().get("connection_state"),
        Some(TELEMETRY_STATE_DISCONNECTED)
    );
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
    assert_eq!(
        ev.telemetry().get("connection_state"),
        Some(TELEMETRY_STATE_DISCONNECTED)
    );

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
    ev.apply_control_unchecked(&ControlSignal::EvDrive { kwh: drive_kwh })
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

/// Departure at midnight (departure_hour = 0.0) exercises the day-wrapping
/// boundary in the BMS ready-by scheduler.
///
/// Physics: 60 kWh battery, 7.2 kW L2 charger, η = 0.9.
/// SOC = 0.2 is below the CC-CV transition SOC (0.85), so no taper applies.
/// Effective charge rate = 7.2 * 0.9 = 6.48 kW effective throughput.
/// SOC deficit = 0.9 - 0.2 = 0.7; hours_needed = 0.7 * 60 / 6.48 ≈ 6.48 h.
///
/// At 12:00 (noon), hours_until_deadline = 24 - 12 + 0 = 12 h > 6.48 h → BMS delays.
/// At 20:00, hours_until_deadline = 24 - 20 + 0 = 4 h < 6.48 h → BMS charges immediately.
/// The 1440-step boundary corresponds to a full 24-hour simulation day.
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

    // Departure at midnight (0.0 h) -- the step-1440 boundary case
    ev.apply_control_unchecked(&ControlSignal::EvSetReadyBy {
        departure_hour: 0.0,
        target_soc: 0.9,
    })
    .unwrap();

    // At 12:00: 12 h until midnight, only 7.63 h needed → BMS delays
    env.current_time = dt(2026, 1, 1, 12, 0, 0);
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(1), &mut ports).unwrap();
    let power_early = ev.telemetry().get("active_power_kw").unwrap();
    assert_eq!(
        power_early, 0.0,
        "BMS should delay at 12:00 for midnight departure (12 h remaining, needs 7.63 h)"
    );

    // At 20:00: 4 h until midnight but 7.63 h needed → BMS charges immediately
    ev.soc = 0.2;
    env.current_time = dt(2026, 1, 1, 20, 0, 0);
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(1), &mut ports).unwrap();
    let power_urgent = ev.telemetry().get("active_power_kw").unwrap();
    assert!(
        power_urgent > 0.0,
        "BMS should charge at 20:00 for midnight departure (only 4 h remaining, needs 7.63 h)"
    );

    // Run 1440 1-minute steps (one full simulated day boundary) to verify the
    // midnight departure_hour=0.0 wrapping does not corrupt SOC or produce
    // negative power at any step.
    for step in 0..1440usize {
        assert!(step < 2000, "step loop diverged");
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(1), &mut ports).unwrap();
        assert!(
            ev.soc >= 0.0 && ev.soc <= 1.0,
            "SOC out of bounds at step {step}: {}",
            ev.soc
        );
        assert!(
            ports.electrical.load_power_w >= 0.0,
            "load_power_w must be non-negative at step {step}: {}",
            ports.electrical.load_power_w
        );
    }
    // With static env time 20:00, the BMS charges until hours_needed drops below
    // hours_until_deadline (4 h), then delays. SOC must be above initial 0.2 and
    // within [0, 1].
    assert!(
        ev.soc > 0.2,
        "after 1440 min starting from urgent state SOC must be above initial 0.2, got {}",
        ev.soc
    );
    assert!(ev.soc <= 1.0, "SOC must not exceed 1.0, got {}", ev.soc);
}

// EvConfig typed round-trip and validation tests

fn minimal_ev_config() -> EvConfig {
    EvConfig {
        equipment_id: None,
        capacity_kwh: 75.0,
        charging_level: None,
        max_charging_power_kw: 11.5,
        charging_efficiency: None,
        l1_current_a: None,
        l1_voltage_v: None,
        soc_max: None,
        initial_soc: None,
        battery_temp_c: None,
        min_charge_temp_c: None,
        full_power_temp_c: None,
        heater_power_w: None,
        heater_threshold_c: None,
        thermal_mass_j_per_k: None,
        ua_w_per_k: None,
        v2l_enabled: None,
        v2l_soc_reserve: None,
        v2l_max_discharge_kw: None,
        v2g_enabled: None,
        v2g_soc_reserve: None,
        v2g_max_discharge_kw: None,
        chemistry: None,
        fuel_economy_kwh_per_mi: None,
        ready_soc: None,
        charging_strategy: None,
        plug_in_policy: None,
        power_limit_kw: None,
        initial_connection_state: None,
        power_factor: None,
        charger_capacity_kva: None,
        cc_cv_transition_soc: None,
        charging_priority: None,
        discharge_respects_deadline: true,
    }
}

#[test]
fn ev_config_round_trips_via_equipment_config() {
    let cfg = minimal_ev_config();
    let ec =
        crate::EquipmentConfig::from_typed("test_ev".to_string(), "EV".to_string(), cfg.clone())
            .unwrap();
    assert!(ec.is_typed());
    let recovered: EvConfig = ec.typed().unwrap();
    assert_eq!(recovered.capacity_kwh, cfg.capacity_kwh);
}

#[test]
fn ev_config_rejects_unknown_fields() {
    use crate::config::ConfigPayload;
    let json = serde_json::json!({
        "capacity_kwh": 75.0,
        "max_charging_power_kw": 7.2,
        "unexpected_key": "bad"
    });
    let ec = crate::EquipmentConfig::with_payload(
        "ev".to_string(),
        "EV".to_string(),
        ConfigPayload::Typed {
            type_name: "EV".to_string(),
            version: 1,
            data: json,
        },
    );
    let result: crate::Result<EvConfig> = ec.typed();
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("unknown field"));
}

#[test]
fn ev_config_validate_rejects_zero_capacity() {
    let mut cfg = minimal_ev_config();
    cfg.capacity_kwh = 0.0;
    assert!(cfg.validate().is_err());
}

#[test]
fn ev_config_validate_rejects_out_of_range_soc_max() {
    let mut cfg = minimal_ev_config();
    cfg.soc_max = Some(1.5);
    assert!(cfg.validate().is_err());
}

#[test]
fn ev_config_validate_rejects_out_of_range_efficiency() {
    let mut cfg = minimal_ev_config();
    cfg.charging_efficiency = Some(1.5);
    assert!(cfg.validate().is_err());
}

#[test]
fn ev_config_validate_passes_for_valid_config() {
    let cfg = minimal_ev_config();
    assert!(cfg.validate().is_ok());
}

#[test]
fn ev_init_typed_sets_fields_from_ev_config() {
    let cfg = EvConfig {
        capacity_kwh: 60.0,
        charging_level: Some("L2".to_string()),
        max_charging_power_kw: 7.2,
        charging_efficiency: Some(0.92),
        initial_soc: Some(0.5),
        ..minimal_ev_config()
    };
    let ec =
        crate::EquipmentConfig::from_typed("test_ev".to_string(), "EV".to_string(), cfg).unwrap();
    let mut ev = Ev::new(ec.clone());
    let env = sample_env();
    ev.init(&ec, &env).unwrap();
    assert_eq!(ev.battery_capacity_kwh, 60.0);
    assert!((ev.charging_efficiency - 0.92).abs() < 1e-9);
    assert!((ev.soc - 0.5).abs() < 1e-9);
    assert_eq!(ev.charging_level, ChargingLevel::L2);
    assert!((ev.rated_power_kw - 7.2).abs() < 1e-9);
}

// ── Raw config cold-charging field path tests ──────────────────────

#[test]
fn raw_config_reads_cold_charging_and_strategy_fields() {
    let mut raw = HashMap::new();
    raw.insert(KEY_BATTERY_CAPACITY_KWH.to_string(), 60.0.into());
    raw.insert(KEY_CHARGING_LEVEL.to_string(), "L2".into());
    raw.insert(KEY_MAX_CHARGING_POWER_KW.to_string(), 7.2.into());
    raw.insert(
        KEY_MIN_CHARGE_TEMP_C.to_string(),
        crate::config::ConfigValue::Float(-10.0),
    );
    raw.insert(
        KEY_FULL_POWER_TEMP_C.to_string(),
        crate::config::ConfigValue::Float(15.0),
    );
    raw.insert(
        KEY_HEATER_POWER_W.to_string(),
        crate::config::ConfigValue::Float(500.0),
    );
    raw.insert(
        KEY_HEATER_THRESHOLD_C.to_string(),
        crate::config::ConfigValue::Float(5.0),
    );
    raw.insert(
        KEY_THERMAL_MASS_J_PER_K.to_string(),
        crate::config::ConfigValue::Float(30_000.0),
    );
    raw.insert(
        KEY_UA_W_PER_K.to_string(),
        crate::config::ConfigValue::Float(8.0),
    );
    let strategy_json = serde_json::to_string(&ChargingStrategy::Nightly {
        off_peak_start_hour: 22.0,
        off_peak_end_hour: 6.0,
        target_soc: 0.8,
    })
    .unwrap();
    raw.insert(
        KEY_CHARGING_STRATEGY.to_string(),
        crate::config::ConfigValue::Text(strategy_json),
    );

    let config = EquipmentConfig::raw("test_ev".to_string(), "EV".to_string(), raw);
    let ev = Ev::new(config);

    assert_eq!(ev.min_charge_temp_c, -10.0);
    assert_eq!(ev.full_power_temp_c, 15.0);
    assert_eq!(ev.heater_power_w, 500.0);
    assert_eq!(ev.heater_threshold_c, 5.0);
    assert_eq!(ev.thermal_mass_j_per_k, 30_000.0);
    assert_eq!(ev.ua_w_per_k, 8.0);
    assert_eq!(
        ev.charging_strategy(),
        &ChargingStrategy::Nightly {
            off_peak_start_hour: 22.0,
            off_peak_end_hour: 6.0,
            target_soc: 0.8,
        }
    );
}

#[test]
fn raw_config_cold_charging_defaults_preserved() {
    let mut raw = HashMap::new();
    raw.insert(KEY_BATTERY_CAPACITY_KWH.to_string(), 60.0.into());
    raw.insert(KEY_CHARGING_LEVEL.to_string(), "L2".into());
    raw.insert(KEY_MAX_CHARGING_POWER_KW.to_string(), 7.2.into());

    let config = EquipmentConfig::raw("test_ev".to_string(), "EV".to_string(), raw);
    let ev = Ev::new(config);

    assert_eq!(ev.min_charge_temp_c, DEFAULT_MIN_CHARGE_TEMP_C);
    assert_eq!(ev.full_power_temp_c, DEFAULT_FULL_POWER_TEMP_C);
    assert_eq!(ev.heater_power_w, DEFAULT_HEATER_POWER_W);
    assert_eq!(ev.heater_threshold_c, DEFAULT_HEATER_THRESHOLD_C);
    assert_eq!(ev.thermal_mass_j_per_k, DEFAULT_THERMAL_MASS_J_PER_K);
    assert_eq!(ev.ua_w_per_k, DEFAULT_UA_W_PER_K);
    assert_eq!(
        ev.charging_strategy(),
        &ChargingStrategy::Immediate { target_soc: 1.0 }
    );
}

#[test]
fn raw_and_typed_config_cold_derate_equivalence() {
    // Raw path: -15 C min, 10 C full, battery at 0 C (→ derate = 0.6)
    let mut raw_data = HashMap::new();
    raw_data.insert(KEY_BATTERY_CAPACITY_KWH.to_string(), 60.0.into());
    raw_data.insert(KEY_CHARGING_LEVEL.to_string(), "L2".into());
    raw_data.insert(KEY_MAX_CHARGING_POWER_KW.to_string(), 7.2.into());
    raw_data.insert(KEY_INITIAL_SOC.to_string(), 0.3.into());
    raw_data.insert(
        KEY_MIN_CHARGE_TEMP_C.to_string(),
        crate::config::ConfigValue::Float(-15.0),
    );
    raw_data.insert(
        KEY_FULL_POWER_TEMP_C.to_string(),
        crate::config::ConfigValue::Float(10.0),
    );
    raw_data.insert(
        KEY_BATTERY_TEMP_C.to_string(),
        crate::config::ConfigValue::Float(0.0),
    );
    let raw_config = EquipmentConfig::raw("test_ev".to_string(), "EV".to_string(), raw_data);
    let mut raw_ev = Ev::new(raw_config);

    // Typed path: same values
    let cfg = EvConfig {
        capacity_kwh: 60.0,
        max_charging_power_kw: 7.2,
        min_charge_temp_c: Some(-15.0),
        full_power_temp_c: Some(10.0),
        battery_temp_c: Some(0.0),
        initial_soc: Some(0.3),
        ..minimal_ev_config()
    };
    let typed = EquipmentConfig::from_typed("test_ev".to_string(), "EV".to_string(), cfg).unwrap();
    let mut typed_ev = Ev::new(typed.clone());
    let env = sample_env();
    typed_ev.init(&typed, &env).unwrap();

    assert_eq!(raw_ev.min_charge_temp_c, typed_ev.min_charge_temp_c);
    assert_eq!(raw_ev.full_power_temp_c, typed_ev.full_power_temp_c);

    let mut ports = PortSlots::default();
    raw_ev
        .step(&env, Duration::minutes(15), &mut ports)
        .unwrap();
    let mut ports = PortSlots::default();
    typed_ev
        .step(&env, Duration::minutes(15), &mut ports)
        .unwrap();

    assert_eq!(
        raw_ev.telemetry().get("charge_derate"),
        typed_ev.telemetry().get("charge_derate"),
        "raw and typed paths must produce the same charge_derate telemetry"
    );
    let raw_power = raw_ev.telemetry().get("active_power_kw").unwrap();
    let typed_power = typed_ev.telemetry().get("active_power_kw").unwrap();
    assert!(
        (raw_power - typed_power).abs() < 1e-9,
        "raw ({raw_power}) and typed ({typed_power}) paths must produce the same charging power"
    );
}

/// Raw and typed init paths produce the same `battery_temp_c` when the
/// config key is absent from both. This test would have failed before the
/// fix because the raw path defaulted to 20.0 and the typed path defaulted
/// to the environment's outdoor temperature.
#[test]
fn raw_and_typed_default_battery_temp_c_are_consistent() {
    let env = sample_env();

    // Raw path: no battery_temp_c key → DEFAULT_BATTERY_TEMP_C set in Ev::new()
    let raw_config = EquipmentConfig::raw("test_ev".to_string(), "EV".to_string(), base_raw());
    let raw_ev = Ev::new(raw_config);

    // Typed path: None battery_temp_c → DEFAULT_BATTERY_TEMP_C set in init_typed()
    let cfg = EvConfig {
        battery_temp_c: None,
        ..minimal_ev_config()
    };
    let typed = EquipmentConfig::from_typed("test_ev".to_string(), "EV".to_string(), cfg).unwrap();
    let mut typed_ev = Ev::new(typed.clone());
    typed_ev.init(&typed, &env).unwrap();

    assert_eq!(raw_ev.battery_temp_c, DEFAULT_BATTERY_TEMP_C);
    assert_eq!(typed_ev.battery_temp_c, DEFAULT_BATTERY_TEMP_C);
    assert_eq!(
        raw_ev.battery_temp_c, typed_ev.battery_temp_c,
        "raw and typed paths must produce the same battery_temp_c when the key is absent"
    );
}

/// Both paths honour an explicit `battery_temp_c` value when present in config.
#[test]
fn raw_and_typed_explicit_battery_temp_c_is_honoured() {
    let env = sample_env();

    // Raw path: explicit key → 35.0
    let mut raw_data = base_raw();
    raw_data.insert(
        KEY_BATTERY_TEMP_C.to_string(),
        crate::config::ConfigValue::Float(35.0),
    );
    let raw_config = EquipmentConfig::raw("test_ev".to_string(), "EV".to_string(), raw_data);
    let raw_ev = Ev::new(raw_config);

    // Typed path: explicit Some(35.0) → 35.0
    let cfg = EvConfig {
        battery_temp_c: Some(35.0),
        ..minimal_ev_config()
    };
    let typed = EquipmentConfig::from_typed("test_ev".to_string(), "EV".to_string(), cfg).unwrap();
    let mut typed_ev = Ev::new(typed.clone());
    typed_ev.init(&typed, &env).unwrap();

    assert_eq!(raw_ev.battery_temp_c, 35.0);
    assert_eq!(typed_ev.battery_temp_c, 35.0);
    assert_eq!(
        raw_ev.battery_temp_c, typed_ev.battery_temp_c,
        "raw and typed paths must produce the same battery_temp_c when an explicit value is present"
    );
}

// ── DemandResponse tests ──────────────────────────────────────────

#[test]
fn ev_dr_grid_emergency_sheds_charging() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control(&ControlSignal::DemandResponse {
        level: DRLevel::GridEmergency,
        duration_s: None,
    })
    .expect("DemandResponse signal should be accepted by EV");

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();

    assert_eq!(ev.telemetry().get("active_power_kw"), Some(0.0));
    assert_eq!(ev.telemetry().get("dr_level"), Some(4.0));
}

#[test]
fn ev_dr_critical_reduces_charging_power() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    let max_allowed = ev.rated_power_kw * 0.25;

    ev.apply_control(&ControlSignal::DemandResponse {
        level: DRLevel::Critical,
        duration_s: Some(3600.0),
    })
    .expect("DemandResponse signal should be accepted by EV");

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();

    let power = ev
        .telemetry()
        .get("active_power_kw")
        .expect("active_power_kw missing from telemetry");
    assert!(
        power <= max_allowed,
        "Critical DR power {:.3} kW exceeds 25% of rated {:.3} kW",
        power,
        max_allowed
    );
    assert!(power > 0.0, "Critical DR should still allow some charging");
    assert_eq!(ev.telemetry().get("dr_level"), Some(3.0));
}

#[test]
fn ev_dr_timer_reverts_to_normal_after_duration() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control(&ControlSignal::DemandResponse {
        level: DRLevel::High,
        duration_s: Some(120.0),
    })
    .expect("DemandResponse signal should be accepted");

    // advance 60s at a time; timer should hit zero during the second call
    ev.update_control(&env);
    ev.update_control(&env);

    // After timer expiry, DR reverts to Normal and charging returns to full power
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();

    let power = ev
        .telemetry()
        .get("active_power_kw")
        .expect("active_power_kw missing from telemetry");
    assert_eq!(ev.telemetry().get("dr_power_fraction"), Some(1.0));
    assert_eq!(ev.telemetry().get("dr_level"), Some(0.0));
    assert!(
        power >= ev.rated_power_kw * 0.5,
        "post-timer power should be near full rated (was {:.3} kW)",
        power
    );
}

// ── Reactive power / smart-inverter acceptance tests ──────────────
//
// The EV is an inverter-coupled DER (V2G/V2L). IEEE 1547-2018 / SAE J3072
// require reactive capability. These tests mirror the battery's 11-test
// pattern in battery/mod.rs exactly.

fn approx_eq(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-6, "values differ: {a} != {b}");
}

/// Default config (pf=1.0) emits Q == Some(0.0) and real power is
/// unchanged from pre-reactive-control behaviour (bit-identical).
#[test]
fn ev_default_config_emits_zero_reactive_power() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();

    let q_co = ev
        .core_output()
        .flows
        .reactive_power_kvar
        .expect("REACTIVE cap implies Some");
    assert!(
        q_co.abs() < 1e-9,
        "default pf=1.0 should give Q≈0, got {q_co}"
    );
    let q_telem = ev.telemetry().get(tk::REACTIVE_POWER_KVAR).unwrap();
    approx_eq(q_telem, 0.0);
    assert_eq!(
        ev.descriptor().control_capabilities,
        ControlCapabilities::POWER_SETPOINT
            | ControlCapabilities::SOC_TARGET
            | ControlCapabilities::POWER_LIMIT
            | ControlCapabilities::EV_PLUG_IN
            | ControlCapabilities::EV_DRIVE
            | ControlCapabilities::EV_AWAY_CHARGE
            | ControlCapabilities::EV_SET_READY_BY
            | ControlCapabilities::DEMAND_RESPONSE
            | ControlCapabilities::REACTIVE_SETPOINT
            | ControlCapabilities::POWER_FACTOR_SETPOINT
    );
    assert!(
        ev.descriptor()
            .core_capabilities
            .contains(CoreCapabilities::REACTIVE)
    );
}

/// Default config real power is bit-identical to a config with pf=1.0
/// explicitly set (Rule R1: Q-only ZIP never touches real power).
#[test]
fn ev_default_real_power_bit_identical_to_pf_one() {
    let config_default = ev_config(base_raw());
    let mut raw_pf1 = base_raw();
    raw_pf1.insert(KEY_POWER_FACTOR.to_string(), 1.0.into());
    let config_pf1 = ev_config(raw_pf1);

    let env = sample_env();
    let mut ev_default = Ev::new(config_default.clone());
    ev_default.init(&config_default, &env).unwrap();
    let mut ev_pf1 = Ev::new(config_pf1.clone());
    ev_pf1.init(&config_pf1, &env).unwrap();

    let mut p_default = PortSlots::default();
    let mut p_pf1 = PortSlots::default();
    ev_default
        .step(&env, Duration::minutes(60), &mut p_default)
        .unwrap();
    ev_pf1
        .step(&env, Duration::minutes(60), &mut p_pf1)
        .unwrap();

    let p_def = ev_default.telemetry().get(tk::ACTIVE_POWER_KW).unwrap();
    let p_pf1 = ev_pf1.telemetry().get(tk::ACTIVE_POWER_KW).unwrap();
    assert!(
        (p_def - p_pf1).abs() < 1e-12,
        "real power must be bit-identical: default={p_def}, pf=1.0={p_pf1}"
    );
}

/// ReactiveSetpoint overrides baseline pf Q.
#[test]
fn ev_reactive_setpoint_overrides_baseline_q() {
    let mut raw = base_raw();
    raw.insert(KEY_POWER_FACTOR.to_string(), 0.9.into());
    raw.insert(KEY_CHARGER_CAPACITY_KVA.to_string(), 20.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.2.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control(&ControlSignal::ReactiveSetpoint { kvar: 1.5 })
        .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();

    let q = ev.telemetry().get(tk::REACTIVE_POWER_KVAR).unwrap();
    approx_eq(q, 1.5);
}

/// PowerFactorSetpoint updates pf, zeros q_setpoint, and Q follows
/// the new pf baseline while charging.
#[test]
fn ev_power_factor_setpoint_zeros_q_setpoint_and_follows_baseline() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.q_setpoint_kvar = Some(2.0);
    ev.apply_control(&ControlSignal::PowerFactorSetpoint { power_factor: 0.8 })
        .unwrap();

    assert_eq!(ev.q_setpoint_kvar, None);
    approx_eq(ev.power_factor, 0.8);

    // Force charging via PowerSetpoint so P>0.
    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_kw: 3.0,
        reactive_power_kvar: None,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();

    let q = ev.telemetry().get(tk::REACTIVE_POWER_KVAR).unwrap();
    let p_kw = ev.telemetry().get(tk::ACTIVE_POWER_KW).unwrap();
    let tan_phi = 0.8_f64.acos().tan();
    let expected_q = p_kw * tan_phi;
    assert!(
        (q - expected_q).abs() < 1e-9,
        "expected Q={expected_q} (P={p_kw} × tan(acos(0.8))={tan_phi}), got {q}"
    );
    assert!(q > 0.0, "charging should yield Q>0 absorbing");
}

/// PowerSetpoint with reactive_power_kvar is accepted and applied.
#[test]
fn ev_power_setpoint_stores_reactive_q() {
    let mut raw = base_raw();
    raw.insert(KEY_CHARGER_CAPACITY_KVA.to_string(), 20.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_kw: 3.0,
        reactive_power_kvar: Some(0.75),
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    assert_eq!(ev.q_setpoint_kvar, Some(0.75));

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();

    let q = ev.telemetry().get(tk::REACTIVE_POWER_KVAR).unwrap();
    approx_eq(q, 0.75);
}

/// A rejected PowerSetpoint must leave ALL control state unchanged: the
/// whole signal is validated before any mutation, so a negative active
/// power (rejected without v2l/v2g) must not arm the reactive override it
/// carried. Regression test for the partial-mutation bug where
/// `q_setpoint_kvar` was set before the negative-setpoint check.
#[test]
fn ev_rejected_power_setpoint_leaves_reactive_state_unchanged() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    let pf_before = ev.power_factor;
    assert_eq!(ev.q_setpoint_kvar, None);
    assert_eq!(ev.power_setpoint_kw, None);

    // Negative setpoint without v2l/v2g must be rejected in full.
    let err = ev
        .apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: -3.0,
            reactive_power_kvar: Some(1.25),
            min_soc: None,
            max_soc: None,
        })
        .expect_err("negative PowerSetpoint without v2l/v2g must be rejected");
    assert!(
        err.to_string().contains("v2l_enabled or v2g_enabled"),
        "unexpected error: {err}"
    );

    assert_eq!(
        ev.q_setpoint_kvar, None,
        "rejected signal must not arm the var override"
    );
    assert_eq!(ev.power_setpoint_kw, None);
    approx_eq(ev.power_factor, pf_before);
}

/// A commanded ReactiveSetpoint of exactly 0.0 is an absolute override: it
/// must force Q = 0 even though the pf < 1 baseline would otherwise produce
/// nonzero Q while charging (`Some(0.0)` is distinct from `None`).
#[test]
fn ev_reactive_setpoint_zero_forces_q_zero_over_pf_baseline() {
    let mut raw = base_raw();
    raw.insert(KEY_POWER_FACTOR.to_string(), 0.9.into());
    raw.insert(KEY_CHARGER_CAPACITY_KVA.to_string(), 20.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    // Charge so the pf = 0.9 baseline produces Q > 0.
    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_kw: 3.0,
        reactive_power_kvar: None,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();
    let baseline_q = ev.telemetry().get(tk::REACTIVE_POWER_KVAR).unwrap();
    assert!(
        baseline_q > 0.0,
        "pf=0.9 charging baseline must produce Q>0, got {baseline_q}"
    );

    // Commanded zero must beat the baseline.
    ev.apply_control(&ControlSignal::ReactiveSetpoint { kvar: 0.0 })
        .unwrap();
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();
    let q = ev.telemetry().get(tk::REACTIVE_POWER_KVAR).unwrap();
    approx_eq(q, 0.0);
    approx_eq(ports.electrical.reactive_power_kvar, 0.0);
}

/// kVA clamp: when Q is commanded beyond the charger's capability,
/// Q is reduced but P is unchanged (active-power priority).
#[test]
fn ev_kva_clamp_curtails_reactive_not_active_power() {
    let mut raw = base_raw();
    raw.insert(KEY_CHARGER_CAPACITY_KVA.to_string(), 5.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.2.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    // Command 5 kW charge + 5 kvar reactive → S = sqrt(P²+25) > 5 kVA.
    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_kw: 3.6,
        reactive_power_kvar: Some(5.0),
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();

    let p = ev.telemetry().get(tk::ACTIVE_POWER_KW).unwrap();
    let q = ev.telemetry().get(tk::REACTIVE_POWER_KVAR).unwrap();

    let s = ev.charger_capacity_kva;
    let q_max = (s * s - p * p).max(0.0).sqrt();
    assert!(
        q.abs() <= q_max + 1e-9,
        "|Q|={} exceeds sqrt(S²−P²)={} with S={s}, P={p}",
        q.abs(),
        q_max
    );
    assert!(
        q.abs() < 5.0 - 1e-9,
        "Q should be clamped below 5.0, got {q}"
    );
    assert!(p > 0.0, "P must be positive (charging)");
}

/// kVA clamp boundary: P and Q commanded so that S²−P² limits Q.
#[test]
fn ev_kva_clamp_reduces_q_to_charger_limit() {
    let mut raw = base_raw();
    raw.insert(KEY_CHARGER_CAPACITY_KVA.to_string(), 5.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.2.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_kw: 3.6,
        reactive_power_kvar: Some(4.0),
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();

    let p = ev.telemetry().get(tk::ACTIVE_POWER_KW).unwrap();
    let q = ev.telemetry().get(tk::REACTIVE_POWER_KVAR).unwrap();

    assert!(p > 0.0, "P must be positive (charging)");
    let s = ev.charger_capacity_kva;
    let q_max = (s * s - p * p).max(0.0).sqrt();
    assert!(
        q.abs() <= q_max + 1e-9,
        "|Q|={} exceeds sqrt(S²−P²)={}",
        q.abs(),
        q_max
    );
    assert!(q.abs() < 4.0 - 1e-9, "Q should be clamped, got {q}");
}

/// Charging (P>0) with pf<1 yields baseline Q>0 (absorbing vars);
/// V2G discharge (P<0) yields baseline Q<0 (supplying vars).
#[test]
fn ev_charging_vs_v2g_discharge_signs() {
    let mut raw = base_raw();
    raw.insert(KEY_POWER_FACTOR.to_string(), 0.9.into());
    raw.insert(KEY_CHARGER_CAPACITY_KVA.to_string(), 20.0.into());
    raw.insert(KEY_V2G_ENABLED.to_string(), true.into());
    raw.insert(KEY_V2G_SOC_RESERVE.to_string(), 0.2.into());
    raw.insert(KEY_V2G_MAX_DISCHARGE_KW.to_string(), 5.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
    let config = ev_config(raw);

    // Charging
    let mut ev_charge = Ev::new(config.clone());
    let env = sample_env();
    ev_charge.init(&config, &env).unwrap();
    ev_charge
        .apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: 3.0,
            reactive_power_kvar: None,
            min_soc: None,
            max_soc: None,
        })
        .unwrap();
    let mut ports = PortSlots::default();
    ev_charge
        .step(&env, Duration::minutes(15), &mut ports)
        .unwrap();
    let q_charge = ev_charge.telemetry().get(tk::REACTIVE_POWER_KVAR).unwrap();
    let p_charge = ev_charge.telemetry().get(tk::ACTIVE_POWER_KW).unwrap();
    assert!(p_charge > 0.0, "charging P>0");
    assert!(
        q_charge > 0.0,
        "charging should yield Q>0 absorbing, got {q_charge}"
    );
    let expected_q = p_charge * (0.9_f64.acos().tan());
    assert!(
        (q_charge - expected_q).abs() < 1e-9,
        "expected Q={expected_q}, got {q_charge}"
    );

    // V2G discharge
    let mut ev_discharge = Ev::new(config.clone());
    ev_discharge.init(&config, &env).unwrap();
    ev_discharge
        .apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: -3.0,
            reactive_power_kvar: None,
            min_soc: None,
            max_soc: None,
        })
        .unwrap();
    let mut ports = PortSlots::default();
    ev_discharge
        .step(&env, Duration::minutes(15), &mut ports)
        .unwrap();
    let q_discharge = ev_discharge
        .telemetry()
        .get(tk::REACTIVE_POWER_KVAR)
        .unwrap();
    let p_discharge = ev_discharge.telemetry().get(tk::ACTIVE_POWER_KW).unwrap();
    assert!(p_discharge < 0.0, "discharging P<0");
    assert!(
        q_discharge < 0.0,
        "V2G discharge should yield Q<0 supplying, got {q_discharge}"
    );
    let expected_q = p_discharge * (0.9_f64.acos().tan());
    assert!(
        (q_discharge - expected_q).abs() < 1e-9,
        "expected Q={expected_q}, got {q_discharge}"
    );
}

/// Port Q, CoreOutput Q, and telemetry Q are the same signed value.
#[test]
fn ev_port_core_output_telemetry_reactive_consistent() {
    let mut raw = base_raw();
    raw.insert(KEY_CHARGER_CAPACITY_KVA.to_string(), 20.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.2.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control(&ControlSignal::ReactiveSetpoint { kvar: 1.2 })
        .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();

    let q_co = ev
        .core_output()
        .flows
        .reactive_power_kvar
        .expect("REACTIVE cap → Some");
    let q_telem = ev.telemetry().get(tk::REACTIVE_POWER_KVAR).unwrap();
    let q_port = ports.electrical.reactive_power_kvar;

    approx_eq(q_co, q_telem);
    approx_eq(q_telem, q_port);
}

/// validate_core_contract passes after a step with reactive power.
#[test]
fn ev_validate_core_contract_passes_with_reactive() {
    let mut raw = base_raw();
    raw.insert(KEY_CHARGER_CAPACITY_KVA.to_string(), 20.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.2.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control(&ControlSignal::ReactiveSetpoint { kvar: 0.5 })
        .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();

    hares_types::validate_core_contract(ev.descriptor(), ev.core_output())
        .expect("core contract should pass with REACTIVE cap + Some(Q)");
}

/// Checkpoint round-trip preserves q_setpoint_kvar and power_factor.
#[test]
fn ev_checkpoint_preserves_reactive_state() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.q_setpoint_kvar = Some(1.5);
    ev.power_factor = 0.85;

    let state = ev.save_state().unwrap();
    let mut restored = Ev::new(config.clone());
    restored.init(&config, &env).unwrap();
    restored.load_state(&state).unwrap();

    assert_eq!(restored.q_setpoint_kvar, Some(1.5));
    approx_eq(restored.power_factor, 0.85);

    // Double round-trip: bytes identical.
    assert_eq!(state, restored.save_state().unwrap());
}

/// Unplugged (Disconnected) → Q = 0 and CoreOutput Some(0.0).
#[test]
fn ev_unplugged_produces_zero_reactive() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::Disconnected,
    })
    .unwrap();

    // Even with a q_setpoint commanded, Q must be 0 when unplugged
    // (contactor open — no grid connection).
    ev.q_setpoint_kvar = Some(2.0);

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

    let q_telem = ev.telemetry().get(tk::REACTIVE_POWER_KVAR).unwrap();
    approx_eq(q_telem, 0.0);
    let q_co = ev
        .core_output()
        .flows
        .reactive_power_kvar
        .expect("REACTIVE cap → Some even when 0");
    approx_eq(q_co, 0.0);
    approx_eq(ports.electrical.reactive_power_kvar, 0.0);
}

/// Grid outage (de-energized bus): the home EVSE is dead — charging and the
/// battery heater stop, SOC holds, and no vars are produced. An islanded
/// home (backup source holding the bus at nominal) can keep charging from
/// the on-site source.
#[test]
fn grid_outage_stops_home_charging_and_islanded_bus_allows_it() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    // Baseline: charges at home.
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    assert!(ports.electrical.load_power_w > 0.0, "baseline charging");
    let soc_after_baseline = ev.telemetry().get(tk::SOC).unwrap();

    // Utility outage, no backup: no draw, no vars, SOC unchanged.
    let mut env_outage = sample_env();
    env_outage.grid.voltage_pu = 0.0;
    ports.zero();
    ev.step(&env_outage, Duration::minutes(15), &mut ports)
        .unwrap();
    assert_eq!(ports.electrical.load_power_w, 0.0);
    assert_eq!(ports.electrical.reactive_power_kvar, 0.0);
    assert_eq!(ev.telemetry().get(tk::ACTIVE_POWER_KW), Some(0.0));
    assert!(
        (ev.telemetry().get(tk::SOC).unwrap() - soc_after_baseline).abs() < 1e-12,
        "SOC must not change while the EVSE is dead"
    );

    // Islanded: bus energized by a backup source → charging resumes.
    let mut env_islanded = sample_env();
    env_islanded.grid.voltage_pu = 0.0;
    env_islanded.grid.island_bus_voltage_pu = Some(1.0);
    ports.zero();
    ev.step(&env_islanded, Duration::minutes(15), &mut ports)
        .unwrap();
    assert!(
        ports.electrical.load_power_w > 0.0,
        "islanded bus allows charging from the on-site source"
    );
}

/// An EV in V2L/V2G discharge is a source: its discharge is NOT gated by a
/// dead bus, and while discharging it reports island-source availability so
/// the dwelling can hold the bus energized.
#[test]
fn v2l_discharge_not_gated_by_outage_and_counts_as_island_source() {
    let mut raw = base_raw();
    raw.insert(KEY_V2L_ENABLED.to_string(), true.into());
    raw.insert(KEY_V2L_SOC_RESERVE.to_string(), 0.2.into());
    raw.insert(KEY_V2L_MAX_DISCHARGE_KW.to_string(), 3.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();
    assert!(
        !ev.island_source_available(),
        "a plugged-in EV that is not discharging cannot island the home"
    );

    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_kw: -2.0,
        reactive_power_kvar: None,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    // Utility outage with a dead bus: discharge continues — the EV is the
    // source.
    let mut env_outage = sample_env();
    env_outage.grid.voltage_pu = 0.0;
    let mut ports = PortSlots::default();
    ev.step(&env_outage, Duration::minutes(15), &mut ports)
        .unwrap();
    let power = ev.telemetry().get(tk::ACTIVE_POWER_KW).unwrap();
    assert!(
        power < 0.0,
        "V2L discharge must not be gated by the outage, got {power}"
    );
    assert!(
        ev.island_source_available(),
        "a discharging EV reports island-source availability"
    );
}

// ── V2L/V2G Ready‑By deadline interlock tests ────────────────────

#[test]
fn v2l_discharge_stops_at_ready_by_soc() {
    let mut raw = base_raw();
    raw.insert(KEY_V2L_ENABLED.to_string(), true.into());
    raw.insert(KEY_V2L_SOC_RESERVE.to_string(), 0.2.into());
    raw.insert(KEY_V2L_MAX_DISCHARGE_KW.to_string(), 3.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control_unchecked(&ControlSignal::EvSetReadyBy {
        departure_hour: 7.0,
        target_soc: 0.9,
    })
    .unwrap();

    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_kw: -2.0,
        reactive_power_kvar: None,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();

    let power = ev.telemetry().get("active_power_kw").unwrap();
    assert_eq!(
        power, 0.0,
        "V2L discharge must stop at ready_by_soc=0.9 when SOC=0.5 and v2l_soc_reserve=0.2"
    );
}

#[test]
fn v2l_discharge_proceeds_without_ready_by_soc() {
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
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();

    let power = ev.telemetry().get("active_power_kw").unwrap();
    assert!(
        power < -1.0,
        "V2L discharge must proceed to v2l_soc_reserve=0.2 when ready_by_soc is unset, got {power}"
    );
}

#[test]
fn v2g_discharge_stops_at_ready_by_soc() {
    let mut raw = base_raw();
    raw.insert(KEY_V2G_ENABLED.to_string(), true.into());
    raw.insert(KEY_V2G_SOC_RESERVE.to_string(), 0.2.into());
    raw.insert(KEY_V2G_MAX_DISCHARGE_KW.to_string(), 5.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control_unchecked(&ControlSignal::EvSetReadyBy {
        departure_hour: 7.0,
        target_soc: 0.9,
    })
    .unwrap();

    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_kw: -3.0,
        reactive_power_kvar: None,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();

    let power = ev.telemetry().get("active_power_kw").unwrap();
    assert_eq!(
        power, 0.0,
        "V2G discharge must stop at ready_by_soc=0.9 when SOC=0.5 and v2g_soc_reserve=0.2"
    );
}

#[test]
fn v2g_discharge_proceeds_without_ready_by_soc() {
    let mut raw = base_raw();
    raw.insert(KEY_V2G_ENABLED.to_string(), true.into());
    raw.insert(KEY_V2G_SOC_RESERVE.to_string(), 0.2.into());
    raw.insert(KEY_V2G_MAX_DISCHARGE_KW.to_string(), 5.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_kw: -3.0,
        reactive_power_kvar: None,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();

    let power = ev.telemetry().get("active_power_kw").unwrap();
    assert!(
        power < -1.0,
        "V2G discharge must proceed to v2g_soc_reserve=0.2 when ready_by_soc is unset, got {power}"
    );
}

#[test]
fn ev_with_0700_deadline_refuses_discharge_below_90pct_at_2000() {
    let mut raw = base_raw();
    raw.insert(KEY_V2L_ENABLED.to_string(), true.into());
    raw.insert(KEY_V2L_SOC_RESERVE.to_string(), 0.2.into());
    raw.insert(KEY_V2L_MAX_DISCHARGE_KW.to_string(), 3.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let mut env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control_unchecked(&ControlSignal::EvSetReadyBy {
        departure_hour: 7.0,
        target_soc: 0.9,
    })
    .unwrap();

    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_kw: -2.0,
        reactive_power_kvar: None,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    env.current_time = dt(2026, 1, 1, 20, 0, 0);
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();

    let power = ev.telemetry().get("active_power_kw").unwrap();
    assert_eq!(
        power, 0.0,
        "EV with 07:00 deadline at 90% SOC must not discharge below 90% at 20:00, \
         even with reserve=20%. Got {power}"
    );
}

#[test]
fn v2l_discharge_respects_deadline_flag_off_bypasses_interlock() {
    let mut raw = base_raw();
    raw.insert(KEY_V2L_ENABLED.to_string(), true.into());
    raw.insert(KEY_V2L_SOC_RESERVE.to_string(), 0.2.into());
    raw.insert(KEY_V2L_MAX_DISCHARGE_KW.to_string(), 3.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.discharge_respects_deadline = false;

    ev.apply_control_unchecked(&ControlSignal::EvSetReadyBy {
        departure_hour: 7.0,
        target_soc: 0.9,
    })
    .unwrap();

    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_kw: -2.0,
        reactive_power_kvar: None,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();

    let power = ev.telemetry().get("active_power_kw").unwrap();
    assert!(
        power < -1.0,
        "V2L discharge with discharge_respects_deadline=false must proceed \
         to v2l_soc_reserve=0.2 even with ready_by_soc=0.9, got {power}"
    );
}

/// Ages an EV through `days` of pure calendar degradation (disconnected, SOC
/// held constant) so `capacity_fade_fraction()` becomes non-zero and the
/// day-boundary SOH→capacity linkage fires. Returns the aged EV.
///
/// Under the Smith (2017) model the beginning-of-life transient (`q_li3 < 0`)
/// keeps SOH slightly above 1.0 for hundreds of days at mid-SOC, so the usable
/// capacity sits marginally *above* rated here — the linkage is proportional
/// (`capacity = rated · SOH`), not strictly a reduction. Tests therefore assert
/// the algebraic relationship, which is universally valid.
fn aged_ev(days: usize) -> Ev {
    let dt = Duration::from_secs(300);
    let steps_per_day = 288usize;
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
    raw.insert(KEY_BATTERY_TEMP_C.to_string(), 25.0.into());
    raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
    let config = ev_config(raw);
    let mut env = sample_env();
    let mut ev = Ev::new(config.clone());
    ev.init(&config, &env).unwrap();
    ev.last_daily_update_day = {
        use chrono::Datelike;
        env.current_time.date_naive().num_days_from_ce()
    };
    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::Disconnected,
    })
    .unwrap();
    for _ in 0..days {
        for _ in 0..steps_per_day {
            let mut ports = PortSlots::default();
            ev.step(&env, dt, &mut ports).unwrap();
            env.current_time += ChronoDuration::seconds(dt.as_secs() as i64);
        }
    }
    ev
}

/// Unit test for the SOH→capacity feedback (T-0421 directive 2): once the
/// day-boundary degradation update has produced a non-zero capacity fade, the
/// usable pack capacity must equal `rated · (1 − fade)`. Before the fix the EV
/// held `battery_capacity_kwh` constant at the rated value, so SOC arithmetic
/// ignored degradation entirely.
#[test]
fn daily_degradation_rescales_usable_capacity() {
    let ev = aged_ev(30);
    let fade = ev.degradation.capacity_fade_fraction();

    assert!(
        fade.abs() > 1e-6,
        "30 days of aging should produce a non-zero capacity fade, got {fade}"
    );

    let expected = ev.battery_capacity_kwh_rated * (1.0 - fade);
    assert!(
        (ev.battery_capacity_kwh - expected).abs() < 1e-9,
        "usable capacity {} must equal rated·(1−fade) = {expected}",
        ev.battery_capacity_kwh
    );
    // The runtime divisor must differ from the rated value — otherwise SOC
    // arithmetic would still be using the undegraded capacity (the bug).
    assert!(
        (ev.battery_capacity_kwh - ev.battery_capacity_kwh_rated).abs() > 1e-6,
        "aged usable capacity {} must differ from rated {}",
        ev.battery_capacity_kwh,
        ev.battery_capacity_kwh_rated
    );
}

/// Integration test for the runtime effect (T-0421 directive core): a
/// fixed-energy drive moves SOC by `energy / usable_capacity`, i.e. it scales
/// as `1/SOH` relative to a fresh pack. Before the fix a fixed drive always
/// moved SOC by `energy / rated`, independent of the pack's aged state.
#[test]
fn fixed_drive_soc_swing_scales_with_degraded_capacity() {
    let drive_kwh = 6.0;

    // Fresh pack: SOH = 1, capacity = rated.
    let mut fresh = aged_ev(0);
    fresh.soc = 0.9;
    let rated = fresh.battery_capacity_kwh_rated;
    assert!((fresh.battery_capacity_kwh - rated).abs() < 1e-12);
    let soc_before_fresh = fresh.soc;
    fresh
        .apply_control_unchecked(&ControlSignal::EvDrive { kwh: drive_kwh })
        .unwrap();
    let swing_fresh = soc_before_fresh - fresh.soc;

    // Aged pack: SOH ≠ 1, capacity = rated·SOH.
    let mut aged = aged_ev(40);
    aged.soc = 0.9;
    let cap_aged = aged.battery_capacity_kwh;
    let soh = 1.0 - aged.degradation.capacity_fade_fraction();
    let soc_before_aged = aged.soc;
    aged.apply_control_unchecked(&ControlSignal::EvDrive { kwh: drive_kwh })
        .unwrap();
    let swing_aged = soc_before_aged - aged.soc;

    // Each swing equals energy / usable_capacity.
    assert!(
        (swing_fresh - drive_kwh / rated).abs() < 1e-9,
        "fresh swing {swing_fresh} should equal energy/rated"
    );
    assert!(
        (swing_aged - drive_kwh / cap_aged).abs() < 1e-9,
        "aged swing {swing_aged} should equal energy/degraded-capacity"
    );

    // The scaling law: swing ratio equals the capacity ratio = 1/SOH.
    let swing_ratio = swing_aged / swing_fresh;
    assert!(
        (swing_ratio - rated / cap_aged).abs() < 1e-9,
        "swing ratio {swing_ratio} must match rated/aged capacity = 1/SOH"
    );
    assert!(
        (swing_ratio - 1.0 / soh).abs() < 1e-9,
        "swing ratio {swing_ratio} must equal 1/SOH = {}",
        1.0 / soh
    );

    // Degradation must actually change the runtime SOC dynamics: the aged swing
    // differs measurably from what the rated (undegraded) divisor would give.
    assert!(
        (swing_aged - drive_kwh / rated).abs() > 1e-4,
        "aged swing {swing_aged} must differ from the buggy rated-based swing {}",
        drive_kwh / rated
    );
}

/// Checkpoint restore must recompute the degraded usable capacity from the
/// rated capacity (set by `init`) and the restored SOH — mirroring
/// `Battery::load_state`. `battery_capacity_kwh_rated` is not serialized; it is
/// re-established by `init` before `load_state`.
#[test]
fn load_state_recomputes_degraded_capacity_from_rated_and_soh() {
    let source = aged_ev(40);
    let saved = source.save_state().unwrap();

    let config = {
        let mut raw = base_raw();
        raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
        ev_config(raw)
    };
    let env = sample_env();
    let mut restored = Ev::new(config.clone());
    restored.init(&config, &env).unwrap();
    restored.load_state(&saved).unwrap();

    assert!(
        (restored.battery_capacity_kwh_rated - source.battery_capacity_kwh_rated).abs() < 1e-12,
        "rated capacity must survive init across restore"
    );
    assert!(
        (restored.battery_capacity_kwh - source.battery_capacity_kwh).abs() < 1e-9,
        "restored usable capacity {} must match source {}",
        restored.battery_capacity_kwh,
        source.battery_capacity_kwh
    );
    let fade = restored.degradation.capacity_fade_fraction();
    assert!(
        (restored.battery_capacity_kwh - restored.battery_capacity_kwh_rated * (1.0 - fade)).abs()
            < 1e-9,
        "restored usable capacity must equal rated·(1−fade)"
    );
}

// ── Auto-driver attachment: every strategy needs vehicle-use simulation ──

/// `EvAwayCharge` with a non-finite or negative power must be rejected on
/// the unchecked path too: the away-charge gate (`> 0.0`) compares false
/// against NaN, silently dropping the command instead of reporting it.
#[test]
fn ev_away_charge_rejects_non_finite_and_negative_power() {
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

    for bad in [f64::NAN, -1.5, f64::NEG_INFINITY] {
        let err = ev
            .apply_control_unchecked(&ControlSignal::EvAwayCharge { power_kw: bad })
            .expect_err("invalid away-charge power must be rejected");
        assert!(
            format!("{err:?}").contains("EvAwayCharge"),
            "error must name the signal for {bad}, got {err:?}"
        );
    }
}

/// `EvSetReadyBy` with out-of-domain values must be rejected on the
/// unchecked path too: they feed the charging target and deadline pacing,
/// so NaN would silently disable charging and an out-of-range hour would
/// pace the deadline against a time that never arrives.
#[test]
fn ev_set_ready_by_rejects_out_of_domain_values() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    for bad in [
        ControlSignal::EvSetReadyBy {
            departure_hour: f64::NAN,
            target_soc: 0.8,
        },
        ControlSignal::EvSetReadyBy {
            departure_hour: 25.0,
            target_soc: 0.8,
        },
        ControlSignal::EvSetReadyBy {
            departure_hour: 7.0,
            target_soc: 1.5,
        },
    ] {
        let err = ev
            .apply_control_unchecked(&bad)
            .expect_err("out-of-domain EvSetReadyBy must be rejected");
        assert!(
            format!("{err:?}").contains("EvSetReadyBy"),
            "error must name the signal for {bad:?}, got {err:?}"
        );
    }
}

/// `PowerSetpoint` with a non-finite active power must be rejected on the
/// unchecked path too: the battery, PV, and scheduled-load arms all guard
/// this value at the arm level. On the EV a NaN setpoint is silently
/// accepted and then silently disarms charging — `f64::max` swallows NaN,
/// so the setpoint resolves to 0 kW with no error, forever — while −∞
/// satisfies the `< 0.0` discharge gate whenever v2g/v2l is enabled and is
/// silently saturated to the hardware maximum, a garbage request
/// indistinguishable from "discharge everything you can".
#[test]
fn power_setpoint_rejects_non_finite_active_power_on_unchecked_path() {
    let env = sample_env();

    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    ev.init(&config, &env).unwrap();
    let err = ev
        .apply_control_unchecked(&ControlSignal::PowerSetpoint {
            active_power_kw: f64::NAN,
            reactive_power_kvar: None,
            min_soc: None,
            max_soc: None,
        })
        .expect_err("non-finite active setpoint must be rejected");
    assert!(
        format!("{err:?}").contains("active_power_kw"),
        "error must name the offending field for NaN, got {err:?}"
    );

    // With discharge enabled, −∞ passes the negative-power v2g/v2l gate
    // (`< 0.0` holds), so only a finiteness guard can catch it there; NaN
    // bypasses the same gate in every configuration.
    let mut raw = base_raw();
    raw.insert(KEY_V2G_ENABLED.to_string(), true.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    ev.init(&config, &env).unwrap();
    for bad in [f64::NAN, f64::NEG_INFINITY] {
        let err = ev
            .apply_control_unchecked(&ControlSignal::PowerSetpoint {
                active_power_kw: bad,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            })
            .expect_err("non-finite active setpoint must be rejected when v2g is enabled");
        assert!(
            format!("{err:?}").contains("active_power_kw"),
            "error must name the offending field for {bad}, got {err:?}"
        );
    }
}

/// The EV arms' SOC-window guards must also hold on the unchecked path
/// (the checked path is covered by the central validator, but
/// `apply_control_unchecked` bypasses it — the arms are the last line for
/// the window fields they store raw).
#[test]
fn ev_soc_window_fields_reject_non_finite_on_unchecked_path() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    ev.init(&config, &sample_env()).unwrap();

    for signal in [
        ControlSignal::PowerSetpoint {
            active_power_kw: 1.0,
            reactive_power_kvar: None,
            min_soc: Some(f64::NAN),
            max_soc: None,
        },
        ControlSignal::PowerSetpoint {
            active_power_kw: 1.0,
            reactive_power_kvar: None,
            min_soc: None,
            max_soc: Some(f64::INFINITY),
        },
        ControlSignal::SOCTarget {
            target_soc: 0.8,
            min_soc: Some(f64::NAN),
            max_soc: None,
        },
        ControlSignal::SOCTarget {
            target_soc: 0.8,
            min_soc: None,
            max_soc: Some(f64::NEG_INFINITY),
        },
    ] {
        let err = ev
            .apply_control_unchecked(&signal)
            .expect_err("non-finite SOC window must be rejected on the unchecked path");
        assert!(
            format!("{err:?}").contains("soc"),
            "error must name the offending window field for {signal:?}, got {err:?}"
        );
    }
}

/// `SOCTarget` with a non-finite target must be rejected on the unchecked
/// path too: the battery arm rejects it after clamping, while the EV arm's
/// `clamp(0.0, 1.0)` keeps NaN (`f64::clamp` propagates NaN), making
/// `soc >= soc_limit` compare false forever — the EV then charges at full
/// power toward a target it can never reach, even at `soc_max` where the
/// drawn energy vanishes in the SOC clamp. ±∞ silently becomes a plausible
/// target (1.0 / 0.0) standing in for garbage, which the checked path's
/// central validator would have rejected outright.
#[test]
fn soc_target_rejects_non_finite_target_on_unchecked_path() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    ev.init(&config, &sample_env()).unwrap();

    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let err = ev
            .apply_control_unchecked(&ControlSignal::SOCTarget {
                target_soc: bad,
                min_soc: None,
                max_soc: None,
            })
            .expect_err("non-finite SOCTarget must be rejected");
        assert!(
            format!("{err:?}").contains("target_soc"),
            "error must name the offending field for {bad}, got {err:?}"
        );
    }
}

/// `PowerSetpoint`'s SOC window must be validated on the checked path: the
/// central `validate_numeric_bounds` checks only the active and reactive
/// components of this signal, so a non-finite `min_soc`/`max_soc` reaches
/// the arm, is stored raw, and is then silently substituted with defaults
/// downstream (`v2g_soc_reserve.max(NaN)` returns the reserve; the
/// `limit.min(NaN)` cap no-ops) — a requested constraint that quietly
/// never applies, the present-but-invalid silently-substituted shape.
#[test]
fn power_setpoint_rejects_non_finite_soc_window_on_checked_path() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    ev.init(&config, &sample_env()).unwrap();

    for signal in [
        ControlSignal::PowerSetpoint {
            active_power_kw: 1.0,
            reactive_power_kvar: None,
            min_soc: Some(f64::NAN),
            max_soc: None,
        },
        ControlSignal::PowerSetpoint {
            active_power_kw: 1.0,
            reactive_power_kvar: None,
            min_soc: None,
            max_soc: Some(f64::NAN),
        },
    ] {
        let err = ev
            .apply_control(&signal)
            .expect_err("non-finite PowerSetpoint SOC window must be rejected");
        assert!(
            format!("{err:?}").contains("soc"),
            "error must name the offending field for {signal:?}, got {err:?}"
        );
    }
}

/// `SOCTarget` with finite but out-of-domain values must be rejected on the
/// unchecked path too: the arm's new guards stop non-finite values, but a
/// finite `target_soc` outside [0, 1] is silently clamped into a plausible
/// target (5.0 → 1.0, −0.5 → 0.0 — the latter silently disarms charging,
/// this initiative's original symptom class), and an inverted or
/// out-of-domain window silently collapses the charge target
/// (`limit.max(min).min(max)`: min 0.8 / max 0.2 caps a 0.9 target at 0.2;
/// a window value above 1 or below 0 has no SOC meaning at all). The
/// central validator rejects all of these on the checked path, and the
/// battery arm rejects inverted bounds at the arm — the EV arm must not be
/// the one surface where garbage quietly becomes a different constraint.
#[test]
fn soc_target_rejects_out_of_domain_values_on_unchecked_path() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    ev.init(&config, &sample_env()).unwrap();

    for bad in [
        ControlSignal::SOCTarget {
            target_soc: 5.0,
            min_soc: None,
            max_soc: None,
        },
        ControlSignal::SOCTarget {
            target_soc: -0.5,
            min_soc: None,
            max_soc: None,
        },
        ControlSignal::SOCTarget {
            target_soc: 0.9,
            min_soc: Some(0.8),
            max_soc: Some(0.2),
        },
        ControlSignal::SOCTarget {
            target_soc: 0.9,
            min_soc: Some(7.5),
            max_soc: None,
        },
        ControlSignal::SOCTarget {
            target_soc: 0.9,
            min_soc: None,
            max_soc: Some(-0.1),
        },
    ] {
        let err = ev
            .apply_control_unchecked(&bad)
            .expect_err("out-of-domain SOCTarget must be rejected");
        assert!(
            format!("{err:?}").to_lowercase().contains("soc"),
            "error must name the signal or offending field for {bad:?}, got {err:?}"
        );
    }
}

/// `PowerSetpoint`'s SOC window must be domain-checked on the unchecked
/// path too: the arm's new guards stop non-finite values, but a finite
/// out-of-domain or inverted window is stored raw and silently rewrites
/// behaviour — `min_soc` above 1 puts the v2g/v2l discharge floor above
/// any reachable SOC (discharge permanently disabled), an inverted window
/// caps the charge target at the max while flooring at the min, and a
/// negative `max_soc` disarms charging outright. The central validator
/// rejects range and ordering on the checked path; the arm must not be the
/// surface where the same garbage quietly applies as a different
/// constraint.
#[test]
fn power_setpoint_rejects_out_of_domain_soc_window_on_unchecked_path() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    ev.init(&config, &sample_env()).unwrap();

    for signal in [
        ControlSignal::PowerSetpoint {
            active_power_kw: 1.0,
            reactive_power_kvar: None,
            min_soc: Some(7.5),
            max_soc: None,
        },
        ControlSignal::PowerSetpoint {
            active_power_kw: 1.0,
            reactive_power_kvar: None,
            min_soc: Some(0.8),
            max_soc: Some(0.2),
        },
        ControlSignal::PowerSetpoint {
            active_power_kw: 1.0,
            reactive_power_kvar: None,
            min_soc: None,
            max_soc: Some(-0.1),
        },
    ] {
        let err = ev
            .apply_control_unchecked(&signal)
            .expect_err("out-of-domain PowerSetpoint SOC window must be rejected");
        assert!(
            format!("{err:?}").to_lowercase().contains("soc"),
            "error must name the signal or offending field for {signal:?}, got {err:?}"
        );
    }
}

/// An EV built through the ordinary construction path must always be
/// provisioned with an EvDriverActor seed: that actor is the only built-in
/// mechanism that simulates vehicle departures and depletes SOC. Withholding
/// it leaves the EV parked at its initial SOC forever, so it never charges.
#[test]
fn actor_seed_default_strategy_returns_ev_seed() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    ev.init(&config, &sample_env()).unwrap();

    match ev.actor_seed() {
        Some(crate::ActorSeed::Ev { strategy, .. }) => {
            assert_eq!(
                strategy,
                hares_types::ChargingStrategy::Immediate { target_soc: 1.0 }
            );
        }
        other => panic!("expected Some(ActorSeed::Ev) for default strategy, got {other:?}"),
    }
}

/// An explicit `Immediate` override must not suppress the driver either:
/// it selects charging behaviour, not the absence of vehicle use.
#[test]
fn actor_seed_immediate_strategy_returns_ev_seed() {
    let mut raw = base_raw();
    let json = serde_json::to_string(&hares_types::ChargingStrategy::Immediate { target_soc: 0.8 })
        .unwrap();
    raw.insert(
        KEY_CHARGING_STRATEGY.to_string(),
        crate::config::ConfigValue::Text(json),
    );
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    ev.init(&config, &sample_env()).unwrap();

    match ev.actor_seed() {
        Some(crate::ActorSeed::Ev { strategy, .. }) => {
            assert_eq!(
                strategy,
                hares_types::ChargingStrategy::Immediate { target_soc: 0.8 }
            );
        }
        other => panic!("expected Some(ActorSeed::Ev) for Immediate strategy, got {other:?}"),
    }
}

/// Class rule: no `ChargingStrategy` variant may suppress the driver seed.
/// Charging strategy selects *when/how* to charge, never *whether* the
/// vehicle is used.
#[test]
fn actor_seed_returns_ev_seed_for_every_charging_strategy() {
    let strategies = vec![
        hares_types::ChargingStrategy::Immediate { target_soc: 0.9 },
        hares_types::ChargingStrategy::Nightly {
            off_peak_start_hour: 23.0,
            off_peak_end_hour: 6.0,
            target_soc: 0.9,
        },
        hares_types::ChargingStrategy::LowSoc {
            threshold: 0.3,
            target_soc: 0.9,
        },
        hares_types::ChargingStrategy::QuickThenWait { partial_soc: 0.6 },
        hares_types::ChargingStrategy::PreDeparture {
            target_soc: 0.9,
            departure_schedule: vec![],
        },
        hares_types::ChargingStrategy::TouAware {
            target_soc: 0.9,
            departure_schedule: vec![],
            charge_buffer_hours: 2.0,
        },
        hares_types::ChargingStrategy::SolarSurplus {
            min_charge_rate_kw: 1.4,
            departure_schedule: vec![],
        },
        hares_types::ChargingStrategy::V2H {
            discharge_threshold_soc: 0.8,
            min_soc: 0.3,
        },
        hares_types::ChargingStrategy::V2G {
            min_soc: 0.3,
            max_export_kw: 7.2,
            price_threshold: 0.25,
        },
    ];

    for expected in &strategies {
        let mut raw = base_raw();
        let json = serde_json::to_string(expected).unwrap();
        raw.insert(
            KEY_CHARGING_STRATEGY.to_string(),
            crate::config::ConfigValue::Text(json),
        );
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());
        ev.init(&config, &sample_env()).unwrap();

        match ev.actor_seed() {
            Some(crate::ActorSeed::Ev { strategy, .. }) => {
                assert_eq!(&strategy, expected);
            }
            other => {
                panic!("strategy {expected:?} must still produce an Ev driver seed, got {other:?}")
            }
        }
    }

    // Exhaustiveness tripwire: this match names every `ChargingStrategy`
    // variant with no wildcard arm, so adding a variant to the enum fails
    // compilation here until the author adds a sample to the list above —
    // a new variant cannot silently escape the every-variant assertion.
    for s in &strategies {
        match s {
            hares_types::ChargingStrategy::Immediate { .. }
            | hares_types::ChargingStrategy::Nightly { .. }
            | hares_types::ChargingStrategy::LowSoc { .. }
            | hares_types::ChargingStrategy::QuickThenWait { .. }
            | hares_types::ChargingStrategy::PreDeparture { .. }
            | hares_types::ChargingStrategy::TouAware { .. }
            | hares_types::ChargingStrategy::SolarSurplus { .. }
            | hares_types::ChargingStrategy::V2H { .. }
            | hares_types::ChargingStrategy::V2G { .. } => {}
        }
    }
}

/// A malformed `charging_strategy` override must fail loudly at `init()`,
/// never degrade silently: an EV that parsed a mistyped override as the
/// default `Immediate` would run a whole simulation with behaviour the
/// operator never asked for and no signal that anything was wrong.
#[test]
fn init_errors_on_malformed_charging_strategy_override() {
    let mut raw = base_raw();
    raw.insert(
        KEY_CHARGING_STRATEGY.to_string(),
        crate::config::ConfigValue::Text("not valid json".to_string()),
    );
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());

    let err = ev
        .init(&config, &sample_env())
        .expect_err("malformed charging_strategy must fail init, not fall back to a default");
    assert!(
        format!("{err:?}").contains("invalid charging_strategy"),
        "error must name the offending key, got {err:?}"
    );
}

/// A `charging_strategy` override that is well-formed JSON but carries
/// domain-invalid values must fail `init()` just like malformed JSON does.
/// The parse at `init_typed` is the boundary where the value enters the
/// system, and the auto-attached driver acts on these numbers directly: a
/// `target_soc` above 1.0 can never be reached, an off-peak hour >= 24
/// matches no time of day, and a `LowSoc` threshold above 1.0 gates
/// charging on permanently -- each leaves the EV never (or always)
/// charging with no signal that the configured value was nonsense.
#[test]
fn init_errors_on_domain_invalid_charging_strategy_override() {
    let bad_strategies = [
        r#"{"Immediate":{"target_soc":1.5}}"#,
        r#"{"Immediate":{"target_soc":-0.1}}"#,
        r#"{"Nightly":{"off_peak_start_hour":24.0,"off_peak_end_hour":6.0,"target_soc":0.9}}"#,
        r#"{"LowSoc":{"threshold":1.2,"target_soc":0.9}}"#,
    ];

    for bad in bad_strategies {
        let mut raw = base_raw();
        raw.insert(
            KEY_CHARGING_STRATEGY.to_string(),
            crate::config::ConfigValue::Text(bad.to_string()),
        );
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());

        let err = ev
            .init(&config, &sample_env())
            .expect_err("domain-invalid charging_strategy must fail init");
        assert!(
            format!("{err:?}").contains("charging_strategy"),
            "error must name the offending key for override '{bad}', got {err:?}"
        );
    }
}

/// An `initial_connection_state` override that does not name a real
/// `EvConnectionState` must fail `init()` loudly. A caller asking for the
/// vehicle to start away (e.g. `"AwayUnplugged"`, or a typo of a real
/// variant) must not silently get a vehicle parked at home plugged in: the
/// wrong starting condition changes every departure/charge decision the
/// simulation makes and leaves no trace that the configured value was
/// dropped.
#[test]
fn init_errors_on_unparseable_initial_connection_state_override() {
    let bad_states = ["AwayUnplugged", "HomPluggedIn"];

    for bad in bad_states {
        let mut raw = base_raw();
        raw.insert(
            KEY_INITIAL_CONNECTION_STATE.to_string(),
            crate::config::ConfigValue::Text(bad.to_string()),
        );
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());

        let err = ev
            .init(&config, &sample_env())
            .expect_err("unparseable initial_connection_state must fail init");
        assert!(
            format!("{err:?}").contains("initial_connection_state"),
            "error must name the offending key for override '{bad}', got {err:?}"
        );
    }
}

/// A `charging_level` override that names no real level must fail `init()`
/// loudly: the level sizes the EVSE power bounds, so an unrecognised string
/// silently becoming L2 would silently clamp the configured charge power.
/// Both spellings HPXML and the python surface supply must keep working.
#[test]
fn init_errors_on_unknown_charging_level_override() {
    for bad in ["L3", "DC", "Level 12"] {
        let mut raw = base_raw();
        raw.insert(
            KEY_CHARGING_LEVEL.to_string(),
            crate::config::ConfigValue::Text(bad.to_string()),
        );
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());

        let err = ev
            .init(&config, &sample_env())
            .expect_err("unknown charging_level must fail init");
        assert!(
            format!("{err:?}").contains("charging_level"),
            "error must name the offending key for override '{bad}', got {err:?}"
        );
    }
}

#[test]
fn accepted_charging_level_spellings_parse() {
    for (good, expected) in [
        ("L1", ChargingLevel::L1),
        ("Level 1", ChargingLevel::L1),
        ("1", ChargingLevel::L1),
        ("L2", ChargingLevel::L2),
        ("Level 2", ChargingLevel::L2),
        ("2", ChargingLevel::L2),
    ] {
        let mut raw = base_raw();
        raw.insert(
            KEY_CHARGING_LEVEL.to_string(),
            crate::config::ConfigValue::Text(good.to_string()),
        );
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());
        ev.init(&config, &sample_env()).unwrap();
        // No public level accessor: the telemetry code (1 = L1, 2 = L2) is
        // the observable contract.
        let level_code = ev
            .telemetry()
            .get(tk::CHARGING_LEVEL)
            .expect("level telemetry");
        let expected_code = match expected {
            ChargingLevel::L1 => 1.0,
            ChargingLevel::L2 => 2.0,
        };
        assert_eq!(level_code, expected_code, "spelling '{good}'");
    }
}

/// An invalid raw-config value must not silently become its placeholder:
/// `Ev::new` constructs (the factory cannot fail) but defers the error,
/// and the first `init()` surfaces it before any simulation can run on the
/// placeholder state.
#[test]
fn raw_config_invalid_values_defer_error_to_init() {
    let mut raw = HashMap::new();
    raw.insert(KEY_BATTERY_CAPACITY_KWH.to_string(), 60.0.into());
    raw.insert(KEY_MAX_CHARGING_POWER_KW.to_string(), 7.2.into());
    // Domain-invalid strategy: parses as JSON, fails validate().
    raw.insert(
        KEY_CHARGING_STRATEGY.to_string(),
        crate::config::ConfigValue::Text(r#"{"Immediate":{"target_soc":1.5}}"#.to_string()),
    );
    // Unknown charging level.
    raw.insert(
        KEY_CHARGING_LEVEL.to_string(),
        crate::config::ConfigValue::Text("L3".to_string()),
    );
    let config = EquipmentConfig::raw("test_ev".to_string(), "EV".to_string(), raw);
    let ev = Ev::new(config.clone());
    // Constructed with placeholders, not silently valid:
    assert_eq!(
        ev.charging_strategy(),
        &ChargingStrategy::Immediate { target_soc: 1.0 }
    );

    let mut ev = ev;
    let err = ev
        .init(&config, &sample_env())
        .expect_err("deferred raw-config error must surface at init");
    // The first invalid field (charging_level, parsed before strategy) wins.
    assert!(
        format!("{err:?}").contains("charging_level"),
        "error must name the first offending key, got {err:?}"
    );
}

/// An unknown `charging_priority` raw value must defer an error rather than
/// silently becoming `DeadlineGuarantee`.
#[test]
fn raw_config_unknown_charging_priority_defers_error_to_init() {
    let mut raw = HashMap::new();
    raw.insert(KEY_BATTERY_CAPACITY_KWH.to_string(), 60.0.into());
    raw.insert(KEY_MAX_CHARGING_POWER_KW.to_string(), 7.2.into());
    raw.insert(
        KEY_CHARGING_PRIORITY.to_string(),
        crate::config::ConfigValue::Text("ExternaAuthority".to_string()),
    );
    let config = EquipmentConfig::raw("test_ev".to_string(), "EV".to_string(), raw);
    let mut ev = Ev::new(config.clone());
    let err = ev
        .init(&config, &sample_env())
        .expect_err("unknown charging_priority must fail init");
    assert!(
        format!("{err:?}").contains("charging_priority"),
        "error must name the offending key, got {err:?}"
    );
}

/// An unparseable `chemistry` override must fail `init()` loudly: the
/// silent fallback substituted NMC, whose open-circuit-voltage curve then
/// drives every charge-voltage decision for a battery the operator did not
/// configure, with no trace that the supplied value was dropped.
#[test]
fn init_errors_on_unparseable_chemistry_override() {
    let bad_chemistries = ["Graphite", "lfp "]; // trailing space fails FromStr

    for bad in bad_chemistries {
        let mut raw = base_raw();
        raw.insert(
            KEY_CHEMISTRY.to_string(),
            crate::config::ConfigValue::Text(bad.to_string()),
        );
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());

        let err = ev
            .init(&config, &sample_env())
            .expect_err("unparseable chemistry must fail init");
        assert!(
            format!("{err:?}").contains("chemistry"),
            "error must name the offending key for override '{bad}', got {err:?}"
        );
    }
}

/// A `plug_in_policy` override that is well-formed JSON but carries a
/// domain-invalid threshold must fail `init()` for the same reason a
/// domain-invalid `charging_strategy` must: the seed carries the policy
/// into the auto-attached driver, whose `LowSoc` plug-in decision reads
/// the raw threshold. A threshold below 0.0 can never be crossed, so the
/// vehicle never plugs in and never charges -- the silent inert-EV shape,
/// with no signal that the configured value was nonsense. A threshold
/// above 1.0 is crossed permanently and degrades to `Always`.
#[test]
fn init_errors_on_domain_invalid_plug_in_policy_override() {
    let bad_policies = [
        r#"{"LowSoc":{"threshold":1.5}}"#,
        r#"{"LowSoc":{"threshold":-0.1}}"#,
    ];

    for bad in bad_policies {
        let mut raw = base_raw();
        raw.insert(
            KEY_PLUG_IN_POLICY.to_string(),
            crate::config::ConfigValue::Text(bad.to_string()),
        );
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());

        let err = ev
            .init(&config, &sample_env())
            .expect_err("domain-invalid plug_in_policy must fail init");
        assert!(
            format!("{err:?}").contains("plug_in_policy"),
            "error must name the offending key for override '{bad}', got {err:?}"
        );
    }
}

/// An unrecognized `charging_level` override must fail `init()` loudly.
/// Every other string key in this parse rejects a present-but-unknown
/// value; `charging_level` alone still maps anything it does not
/// recognize onto `L2`, including `L3` (a real charging concept this
/// model does not support -- silently simulating it as L2 misconfigures
/// the power clamp) and `Level-1`, an L1-intent spelling the L1 arms do
/// not list, which silently becomes L2 and charges at 4-6x the intended
/// power. An empty string is likewise present-but-meaningless.
#[test]
fn init_errors_on_unrecognized_charging_level_override() {
    let bad_levels = ["L3", "Level-1", ""];

    for bad in bad_levels {
        let mut raw = base_raw();
        raw.insert(
            KEY_CHARGING_LEVEL.to_string(),
            crate::config::ConfigValue::Text(bad.to_string()),
        );
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());

        let err = ev
            .init(&config, &sample_env())
            .expect_err("unrecognized charging_level must fail init");
        assert!(
            format!("{err:?}").contains("charging_level"),
            "error must name the offending key for override '{bad}', got {err:?}"
        );
    }
}

/// A `charging_strategy` override carrying a field name the variant does
/// not define must fail `init()` loudly. Serde silently ignores unknown
/// fields by default, so a typo like `off_peak_statr_hour` drops the
/// operator's intended parameter while the strategy still parses and
/// validates -- the run then executes a different charging schedule than
/// the one configured, with no trace that any field was discarded. This
/// is the field-level form of the "override content is discarded"
/// failure this initiative was opened on.
#[test]
fn init_errors_on_unknown_charging_strategy_fields() {
    let bad_overrides = [
        r#"{"Nightly":{"off_peak_start_hour":22.0,"off_peak_end_hour":6.0,"target_soc":0.9,"off_peak_statr_hour":23.0}}"#,
    ];

    for bad in bad_overrides {
        let mut raw = base_raw();
        raw.insert(
            KEY_CHARGING_STRATEGY.to_string(),
            crate::config::ConfigValue::Text(bad.to_string()),
        );
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());

        let err = ev
            .init(&config, &sample_env())
            .expect_err("unknown charging_strategy fields must fail init");
        assert!(
            format!("{err:?}").contains("charging_strategy"),
            "error must name the offending key for override '{bad}', got {err:?}"
        );
    }
}

/// An unknown field inside a `DepartureConstraint` nested in a strategy's
/// `departure_schedule` must fail `init()` loudly, for the same reason an
/// unknown field on the strategy variant itself must: the
/// `deny_unknown_fields` sweep covered `ChargingStrategy` and its sibling
/// override types but not the struct nested one level deeper inside them.
/// A plausible field the model does not have (a readiness minute such as
/// `ready_by_minut`) is silently dropped while the constraint still
/// parses -- the operator believes they configured a departure-readiness
/// deadline the simulation never sees.
#[test]
fn init_errors_on_unknown_departure_constraint_fields() {
    let bad_overrides = [
        r#"{"PreDeparture":{"target_soc":0.9,"departure_schedule":[{"day_filter":"Any","departure_minute":480,"target_soc":0.8,"ready_by_minut":300}]}}"#,
        r#"{"TouAware":{"target_soc":0.9,"charge_buffer_hours":2.0,"departure_schedule":[{"day_filter":"Any","departure_minute":480,"target_soc":0.8,"priority":1}]}}"#,
    ];

    for bad in bad_overrides {
        let mut raw = base_raw();
        raw.insert(
            KEY_CHARGING_STRATEGY.to_string(),
            crate::config::ConfigValue::Text(bad.to_string()),
        );
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());

        let err = ev
            .init(&config, &sample_env())
            .expect_err("unknown DepartureConstraint fields must fail init");
        assert!(
            format!("{err:?}").contains("charging_strategy"),
            "error must name the offending key for override '{bad}', got {err:?}"
        );
    }
}
