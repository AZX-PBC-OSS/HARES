use std::collections::HashMap;
use std::time::Duration;

use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
use hares_types::{
    ControlSignal, DRLevel, EnvironmentState, EvConnectionState, GridState, PortSlots,
    SurfaceIrradiance, WeatherState, ZoneState,
};

use super::*;

// The fixture builder's override keys: the typed EvConfig's own field
// names (the raw payload path is gone).
const KEY_CHARGING_LEVEL: &str = "charging_level";
const KEY_CAPACITY_J: &str = "capacity_j";
const KEY_MAX_CHARGING_POWER_W: &str = "max_charging_power_w";
const KEY_RANGE_M: &str = "range_m";
const KEY_INITIAL_SOC: &str = "initial_soc";
const KEY_EFFICIENCY: &str = "charging_efficiency";
const KEY_L1_CURRENT_A: &str = "l1_current_a";
const KEY_L1_VOLTAGE_V: &str = "l1_voltage_v";
const KEY_BATTERY_TEMP_C: &str = "battery_temp_c";
const KEY_MIN_CHARGE_TEMP_C: &str = "min_charge_temp_c";
const KEY_FULL_POWER_TEMP_C: &str = "full_power_temp_c";
const KEY_HEATER_POWER_W: &str = "heater_power_w";
const KEY_HEATER_THRESHOLD_C: &str = "heater_threshold_c";
const KEY_THERMAL_MASS_J_PER_K: &str = "thermal_mass_j_per_k";
const KEY_UA_W_PER_K: &str = "ua_w_per_k";
const KEY_V2L_ENABLED: &str = "v2l_enabled";
const KEY_V2L_SOC_RESERVE: &str = "v2l_soc_reserve";
const KEY_V2L_MAX_DISCHARGE_W: &str = "v2l_max_discharge_w";
const KEY_V2G_ENABLED: &str = "v2g_enabled";
const KEY_V2G_SOC_RESERVE: &str = "v2g_soc_reserve";
const KEY_V2G_MAX_DISCHARGE_W: &str = "v2g_max_discharge_w";
const KEY_N_SERIES: &str = "n_series";
const KEY_N_PARALLEL: &str = "n_parallel";
const KEY_CELL_RESISTANCE_OHM: &str = "cell_resistance_ohm";
const KEY_READY_SOC: &str = "ready_soc";
const KEY_FUEL_ECONOMY_J_PER_M: &str = "fuel_economy_j_per_m";
const KEY_POWER_LIMIT_W: &str = "power_limit_w";
const KEY_CHARGER_CAPACITY_VA: &str = "charger_capacity_va";
const KEY_CC_CV_TRANSITION_SOC: &str = "cc_cv_transition_soc";
const KEY_CHARGING_STRATEGY: &str = "charging_strategy";
const KEY_PLUG_IN_POLICY: &str = "plug_in_policy";
const KEY_INITIAL_CONNECTION_STATE: &str = "initial_connection_state";
const KEY_POWER_FACTOR: &str = "power_factor";
const KEY_CHEMISTRY: &str = "chemistry";
const KEY_CHARGING_PRIORITY: &str = "charging_priority";
const KEY_SOC_MAX: &str = "soc_max";
const DEFAULT_CAPACITY_J: f64 = 75.0 * 3.6e6;
const L1_CHARGING_POWER_W: f64 = 1400.0;

fn dt(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> chrono::DateTime<FixedOffset> {
    FixedOffset::east_opt(0)
        .expect("UTC offset")
        .with_ymd_and_hms(y, mo, d, h, mi, s)
        .unwrap()
}

fn sample_env() -> EnvironmentState {
    EnvironmentState {
        ambient_other_space_c: hares_types::AmbientOtherSpaceTemps::default(),
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
        schedule_row: None,
        unavailable_periods: hares_types::UnavailablePeriodState::default(),
        domains: hares_types::DomainSlots::default(),
        equipment_telemetry: std::collections::HashMap::new(),
        equipment_core: std::collections::HashMap::new(),
        current_time: dt(2026, 1, 1, 0, 0, 0),
        time_res: ChronoDuration::minutes(1),
        price_signal: Default::default(),
        electrical: Default::default(),
    }
}

fn ev_config(raw: HashMap<String, crate::config::ConfigValue>) -> EquipmentConfig {
    // The typed EvConfig built directly, in SI: the raw EV payload path is
    // gone, so the fixture builder overrides the typed fields and applies
    // the default charging-power rule from its typed inputs only.
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

    let charging_level = get_str(&[KEY_CHARGING_LEVEL]).map(|level| match level.as_str() {
        "Level1" => "L1".to_string(),
        "Level2" => "L2".to_string(),
        _ => level,
    });

    let capacity_j = get_f64(&[KEY_CAPACITY_J])
        .or_else(|| {
            // The range rule on its typed inputs: the range enters in m.
            get_f64(&[KEY_RANGE_M]).map(|range_m| {
                let economy =
                    get_f64(&[KEY_FUEL_ECONOMY_J_PER_M]).unwrap_or(DEFAULT_FUEL_ECONOMY_J_PER_M);
                range_m * economy
            })
        })
        .unwrap_or(DEFAULT_CAPACITY_J);

    let default_level = charging_level.clone().unwrap_or_else(|| "L2".to_string());
    let max_charging_power_w = get_f64(&[KEY_MAX_CHARGING_POWER_W]).unwrap_or_else(|| {
        match default_level.as_str() {
            "L1" => L1_CHARGING_POWER_W,
            // The L2 default charging-power rule, typed inputs only: the
            // vehicle number from the capacity (the BEV default; the
            // vehicle_type raw key is gone with the raw path).
            _ => {
                let capacity_kwh = uom::si::f64::Energy::new::<uom::si::energy::joule>(capacity_j)
                    .get::<uom::si::energy::kilowatt_hour>();
                let range_miles = capacity_kwh / 0.325;
                if range_miles < 175.0 { 3600.0 } else { 11500.0 }
            }
        }
    });

    EquipmentConfig::from_typed(
        "EV #1".to_string(),
        "EV".to_string(),
        EvConfig {
            equipment_id: raw
                .get(crate::config::KEY_EQUIPMENT_ID)
                .and_then(crate::config::ConfigValue::as_f64)
                .map(|value| value as u32),
            capacity_j,
            charging_level,
            max_charging_power_w,
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
            n_series: raw
                .get(KEY_N_SERIES)
                .and_then(crate::config::ConfigValue::as_f64)
                .map(|v| v as u32),
            n_parallel: raw
                .get(KEY_N_PARALLEL)
                .and_then(crate::config::ConfigValue::as_f64)
                .map(|v| v as u32),
            cell_resistance_ohm: get_f64(&[KEY_CELL_RESISTANCE_OHM]),
            v2l_enabled: get_bool(KEY_V2L_ENABLED),
            v2l_soc_reserve: get_f64(&[KEY_V2L_SOC_RESERVE]),
            v2l_max_discharge_w: get_f64(&[KEY_V2L_MAX_DISCHARGE_W]),
            v2g_enabled: get_bool(KEY_V2G_ENABLED),
            v2g_soc_reserve: get_f64(&[KEY_V2G_SOC_RESERVE]),
            v2g_max_discharge_w: get_f64(&[KEY_V2G_MAX_DISCHARGE_W]),
            chemistry: get_str(&[KEY_CHEMISTRY]),
            fuel_economy_j_per_m: get_f64(&[KEY_FUEL_ECONOMY_J_PER_M]),
            ready_soc: get_f64(&[KEY_READY_SOC]),
            charging_strategy: get_str(&[KEY_CHARGING_STRATEGY]),
            plug_in_policy: get_str(&[KEY_PLUG_IN_POLICY]),
            power_limit_w: get_f64(&[KEY_POWER_LIMIT_W]),
            initial_connection_state: get_str(&[KEY_INITIAL_CONNECTION_STATE]),
            power_factor: get_f64(&[KEY_POWER_FACTOR]),
            charger_capacity_va: get_f64(&[KEY_CHARGER_CAPACITY_VA]),
            cc_cv_transition_soc: get_f64(&[KEY_CC_CV_TRANSITION_SOC]),
            charging_priority: get_str(&[KEY_CHARGING_PRIORITY]).map(|s| match s.as_str() {
                "ExternalAuthority" => ChargingPriority::ExternalAuthority,
                _ => ChargingPriority::DeadlineGuarantee,
            }),
            discharge_respects_deadline: true,
            der_1547: None,
        },
    )
    .unwrap()
}

/// The default derate curve at one temperature, for deriving exact
/// usable-capacity expectations: `capacity = rated · SOH · derate(temp)`.
fn capacity_derate_at(temp_c: f64) -> f64 {
    crate::battery::CapacityDerateModel::default().evaluate(temp_c)
}

fn base_raw() -> HashMap<String, crate::config::ConfigValue> {
    let mut raw = HashMap::new();
    raw.insert(KEY_CAPACITY_J.to_string(), 216000000.0.into());
    raw.insert(KEY_CHARGING_LEVEL.to_string(), "L2".into());
    raw.insert(KEY_MAX_CHARGING_POWER_W.to_string(), 7200.0.into());
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
fn l1_default_power_is_1p4_w() {
    let mut raw = base_raw();
    raw.insert(KEY_CHARGING_LEVEL.to_string(), "L1".into());
    raw.remove(KEY_MAX_CHARGING_POWER_W);
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();

    let p = ev.telemetry().get("active_power_kw").unwrap();
    assert!((p - 1400.0).abs() < 1e-9);
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
    assert!((p_low_soc - 7200.0).abs() < 1e-6);

    ev.soc = 0.99;
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();
    let p_taper = ev.telemetry().get("active_power_kw").unwrap();
    assert!(p_taper > 0.0);
    assert!(p_taper < 7200.0);

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
            active_power_w: -1000.0,
            reactive_power_var: None,
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
    raw.remove(KEY_CAPACITY_J);
    raw.insert(KEY_RANGE_M.to_string(), 321868.8.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    ev.init(&config, &sample_env()).unwrap();

    assert!((ev.battery_capacity_j - 65.0 * 3.6e6 * capacity_derate_at(10.0)).abs() < 1e-3);
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
    assert!((p - 960.0).abs() < 1e-9);
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

    let power_w = ev.telemetry().get("active_power_kw").unwrap();
    assert!(power_w > 0.0, "EV should be charging");
    assert!(
        ev.battery_temp_c > temp_before,
        "battery should warm from ohmic losses (efficiency < 1)"
    );
}

#[test]
fn l1_configured_power_is_respected_not_hardcoded() {
    let mut raw = base_raw();
    raw.insert(KEY_CHARGING_LEVEL.to_string(), "L1".into());
    raw.insert(KEY_MAX_CHARGING_POWER_W.to_string(), 1800.0.into());
    raw.remove(KEY_L1_CURRENT_A);
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    assert!(
        (ev.rated_power_w - 1800.0).abs() < 1e-9,
        "L1 rated_power_w should reflect configured max_charging_power_w=1800, got {}",
        ev.rated_power_w
    );

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();
    let p = ev.telemetry().get("active_power_kw").unwrap();
    assert!(
        (p - 1800.0).abs() < 1e-6,
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
    // Heater pinned off: the subject is the plating-cutoff charge block
    // itself — the preconditioning path that pairs with it has its own
    // coverage (battery_heater_warms_pack_while_cold_derate_blocks_charging
    // and the heater tests below).
    let mut raw = base_raw();
    raw.insert(
        KEY_BATTERY_TEMP_C.to_string(),
        crate::config::ConfigValue::Float(-5.0),
    );
    raw.insert(KEY_MIN_CHARGE_TEMP_C.to_string(), 0.0.into());
    raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
    raw.insert(KEY_HEATER_POWER_W.to_string(), 0.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(30), &mut ports).unwrap();
    assert_eq!(ev.telemetry().get("active_power_kw"), Some(0.0));
    assert_eq!(ev.telemetry().get("charge_derate"), Some(0.0));
}

/// The additional-load identity (pre-fix pin was the defective physics:
/// heater draw was deducted from the charge leg's DC, slowing SOC gain
/// while the charger billed it). With the heater as a pack-side DC load
/// covered by the charger's raised import, the pack-side charge rate —
/// and therefore the SOC gain — is identical to the no-heater case while
/// the cap is slack, and the port draws the heater's AC-equivalent on top.
#[test]
fn heater_draw_bills_ac_equivalent_without_slowing_pack_charge_rate() {
    // Cold band (−2..10 °C): charging and preconditioning run together;
    // the supply cap stays slack (0.6 kW charge + 1.33 kW heater ≪ 7.2 kW
    // rating), which is the identity's precondition — at a binding cap the
    // clamp legitimately diverges and is covered separately.
    let heater_w = 1200.0_f64;
    let eta = 0.9_f64;

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
    raw.insert(KEY_HEATER_POWER_W.to_string(), heater_w.into());
    raw.insert(KEY_HEATER_THRESHOLD_C.to_string(), 0.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();
    let soc_before = ev.soc;

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();
    let soc_delta_with_heater = ev.soc - soc_before;
    let power_with_heater = ev.telemetry().get("active_power_kw").unwrap();

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
    raw_no_heater.insert(KEY_HEATER_POWER_W.to_string(), 0.0.into());
    let config_no_heater = ev_config(raw_no_heater);
    let mut ev_no_heater = Ev::new(config_no_heater.clone());
    ev_no_heater.init(&config_no_heater, &sample_env()).unwrap();
    let soc_before_no_heater = ev_no_heater.soc;
    let mut ports = PortSlots::default();
    ev_no_heater
        .step(&env, Duration::minutes(60), &mut ports)
        .unwrap();
    let soc_delta_no_heater = ev_no_heater.soc - soc_before_no_heater;
    let power_no_heater = ev_no_heater.telemetry().get("active_power_kw").unwrap();

    // Same pack-side charge rate → same SOC gain.
    assert!(
        (soc_delta_with_heater - soc_delta_no_heater).abs() < 1e-9,
        "SOC gain must equal the no-heater case while the cap is slack: \
         {soc_delta_with_heater} vs {soc_delta_no_heater}"
    );
    // Port power is higher by exactly the heater's AC-equivalent (W).
    let heater_ac_eq_w = heater_w / eta;
    assert!(
        (power_with_heater - power_no_heater - heater_ac_eq_w).abs() < 1e-6,
        "port draw must exceed the no-heater case by the heater's \
         AC-equivalent ({heater_ac_eq_w} kW): {power_with_heater} vs \
         {power_no_heater}"
    );
    assert!(ev.telemetry().get("heater_power_w").unwrap() > 0.0);
}

#[test]
fn heater_only_grid_draw_when_fully_cold_derated() {
    // UA pinned to 0 so the warming assertion is attributable to the heater
    // alone (ambient coupling would otherwise confound it).
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
    raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();
    let soc_before = ev.soc;
    let temp_before = ev.battery_temp_c;

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

    // The heater is a pack-side DC load fed by the charger's raised import:
    // the port bills the heater's AC-equivalent (heater_dc / η), and the
    // pack nets exactly zero — the cells neither gain nor lose while the
    // heater warms them. The pre-fix identity (port = heater nameplate)
    // omitted the η conversion.
    let power_w = ev.telemetry().get("active_power_kw").unwrap();
    let expected_heater_ac_w = 500.0 / 0.9;
    assert!(
        (power_w - expected_heater_ac_w).abs() < 1e-6,
        "grid draw should equal the heater's AC-equivalent ({expected_heater_ac_w} W), \
         got {power_w} W"
    );
    assert_eq!(
        ev.soc, soc_before,
        "SOC must not change when charge_derate=0 (heater-only state: the \
         charger's raised import covers the heater, netting the pack to zero)"
    );
    assert_eq!(
        ev.telemetry().get("heater_power_w").unwrap(),
        500.0,
        "heater telemetry reports the actual post-cap draw"
    );
    // The heater actually warms the pack (a gate that keys heater heat on
    // charge power bills energy while warming nothing exactly when warming
    // is needed — below the charge cutoff).
    // The tiny extra beyond Q_heater·dt/C — there is none: the pack nets
    // zero in this state, so no current crosses the cell internal
    // resistance. The heater is a parallel DC load on the charger bus
    // (not a series element through the cells); its heat reaches the
    // pack through the thermal equation, and a nonzero ohmic loss here
    // would count the heater's current as cell heating while SOC nets it
    // out of the pack — the same heat-misattribution class D2 closed
    // (one effect, attributed once).
    let delta_k = ev.battery_temp_c - temp_before;
    let mass = ev.thermal_mass_j_per_k;
    let ohmic_w = ev.telemetry().get("ohmic_loss_w").unwrap();
    assert_eq!(
        ohmic_w, 0.0,
        "the heater-only state nets the cells to zero current — its I2R must be exactly zero, got {ohmic_w} W"
    );
    let expected_delta_k = 500.0 * 900.0 / mass;
    assert!(
        (delta_k - expected_delta_k).abs() < 1e-9,
        "heater-only step must warm the pack by Q_heater·dt/C = \
         {expected_delta_k} K, got {delta_k} K"
    );
}

/// The charging arm's I²R basis is the cells' net DC power alone: while
/// connected the heater is a parallel DC load on the charger bus, so its
/// current never crosses the cell internal resistance and must not enter
/// the shared ohmic solve — its heat reaches the pack through the thermal
/// equation instead. The heater-only face of this rule is pinned above
/// (ohmic exactly zero); this pins the charging face: with the supply cap
/// slack, a simultaneous charge + heater step reports the same cell ohmic
/// loss as the same step with the heater off. Feeding the heater-inclusive
/// bus draw to the solve (one current, attributed twice — the D2 shape)
/// would add the heater's own I²R (~0.17 W at this 1.2 kW draw) on top of
/// the charge leg's ~0.03 W.
#[test]
fn heater_current_never_enters_cell_ohmic_loss_while_charging() {
    // Premise mirrors `heater_draw_bills_ac_equivalent_without_slowing_pack_charge_rate`:
    // cold derate band (−1 °C, ramp −2..10 °C → charge leg ≈ 0.6 kW), heater
    // 1.2 kW below its 0 °C threshold, supply cap slack (0.6 + 1.33 ≪ 7.2 kW
    // rating) — the identity's precondition, so the charge leg is identical
    // with and without the heater and any ohmic difference is attribution,
    // not allocation.
    let ohmic_after_one_charge_step = |heater_w: f64| -> f64 {
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
        raw.insert(KEY_HEATER_POWER_W.to_string(), heater_w.into());
        raw.insert(KEY_HEATER_THRESHOLD_C.to_string(), 0.0.into());
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());
        let env = sample_env();
        ev.init(&config, &env).unwrap();
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(60), &mut ports).unwrap();
        ev.telemetry().get("ohmic_loss_w").unwrap()
    };

    let with_heater = ohmic_after_one_charge_step(1200.0);
    let without_heater = ohmic_after_one_charge_step(0.0);
    assert!(
        without_heater > 0.0,
        "the charge leg's own I2R must be nonzero (premise guard: the charge leg flows)"
    );
    assert!(
        (with_heater - without_heater).abs() < 1e-6,
        "the heater's current must not enter the cell ohmic loss while connected: \
         charge + heater reported {with_heater} W vs charge-only {without_heater} W — \
         the heater-inclusive bus draw is being fed to the I2R solve again \
         (one current, attributed twice)"
    );
}

// ── Pack thermal / preconditioning mechanism coverage ──────────────

/// A configured charging-curve LUT's temperature axis IS the
/// temperature-dependent charge capability — a measured curve already
/// contains the manufacturer's low-temperature derate — so the linear BMS
/// ramp is not multiplied on top of it. The `min_charge_temp_c` safety
/// cutoff survives unconditionally: a curve cannot grant permission to
/// charge below the plating boundary.
#[test]
fn lut_temperature_axis_is_the_capability_not_the_linear_ramp() {
    // LUT: fraction 0.6 at 0 °C rising to 1.0 at 25 °C, flat in SOC.
    let lut = crate::ndinterp::RegularGridInterpolator::new(
        vec![
            vec![0.0, 1.0],  // soc
            vec![0.0, 25.0], // temperature
            vec![1.0],       // c-rate
            vec![1.0],       // soh
        ],
        vec![0.6f32, 1.0, 0.6, 1.0],
        crate::ndinterp::ExtrapolationStrategy::Clamp,
    )
    .unwrap();

    // Pack at 5 °C (linear ramp would derate to 0.5); LUT axis at 5 °C
    // interpolates to 0.6 + 0.4·(5/25) = 0.68.
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.2.into());
    raw.insert(KEY_BATTERY_TEMP_C.to_string(), 5.0.into());
    raw.insert(KEY_MIN_CHARGE_TEMP_C.to_string(), 0.0.into());
    raw.insert(KEY_FULL_POWER_TEMP_C.to_string(), 10.0.into());
    raw.insert(KEY_HEATER_POWER_W.to_string(), 0.0.into());
    raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();
    ev.set_charging_curve_lut(Some(lut)).unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    let power_w = ev.telemetry().get("active_power_kw").unwrap();
    let expected = 7200.0 * 0.68;
    assert!(
        (power_w - expected).abs() < 1e-3,
        "LUT temperature axis must be the capability: expected {expected} W \
         (rated x LUT fraction), got {power_w} — the linear ramp is being \
         multiplied on top (one physical effect applied twice)"
    );

    // Safety floor: below the plating cutoff the LUT cannot grant
    // permission to charge.
    let mut raw_cold = base_raw();
    raw_cold.insert(KEY_INITIAL_SOC.to_string(), 0.2.into());
    raw_cold.insert(KEY_BATTERY_TEMP_C.to_string(), (-1.0f64).into());
    raw_cold.insert(KEY_MIN_CHARGE_TEMP_C.to_string(), 0.0.into());
    raw_cold.insert(KEY_FULL_POWER_TEMP_C.to_string(), 10.0.into());
    raw_cold.insert(KEY_HEATER_POWER_W.to_string(), 0.0.into());
    raw_cold.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
    let config_cold = ev_config(raw_cold);
    let mut ev_cold = Ev::new(config_cold.clone());
    ev_cold.init(&config_cold, &sample_env()).unwrap();
    let lut2 = crate::ndinterp::RegularGridInterpolator::new(
        vec![vec![0.0, 1.0], vec![0.0, 25.0], vec![1.0], vec![1.0]],
        vec![0.6f32, 1.0, 0.6, 1.0],
        crate::ndinterp::ExtrapolationStrategy::Clamp,
    )
    .unwrap();
    ev_cold.set_charging_curve_lut(Some(lut2)).unwrap();
    let mut ports = PortSlots::default();
    ev_cold
        .step(&sample_env(), Duration::minutes(15), &mut ports)
        .unwrap();
    assert_eq!(
        ev_cold.telemetry().get("active_power_kw").unwrap(),
        0.0,
        "the min_charge_temp_c cutoff must zero charge power below the \
         plating boundary even when a LUT is present"
    );
}

/// Cap binding in the derate band (the normal control path for cold
/// charging with a pack-duty heater): charging draws first within the
/// supply bound, the heater takes the remainder, the port never exceeds
/// the EVSE rating, and the hand-off completes once the pack passes the
/// heater threshold.
#[test]
fn cold_charge_session_allocates_supply_bound_with_charge_priority() {
    // 8 °C pack: derate 0.8 → charge demand 5.76 kW; heater 5 kW → 5.56 kW
    // AC-equivalent; total demand 11.3 kW > the 7.2 kW rating → binding.
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.2.into());
    raw.insert(KEY_BATTERY_TEMP_C.to_string(), 8.0.into());
    raw.insert(KEY_MIN_CHARGE_TEMP_C.to_string(), 0.0.into());
    raw.insert(KEY_FULL_POWER_TEMP_C.to_string(), 10.0.into());
    // Default heater (5 kW) and threshold (5 °C): 8 °C ≤ … no — 8 > 5, the
    // heater is off. Use a threshold above the pack temperature so the
    // heater participates: threshold 10 °C.
    raw.insert(KEY_HEATER_THRESHOLD_C.to_string(), 10.0.into());
    raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    let port_w = ev.telemetry().get("active_power_kw").unwrap();
    let heater_w = ev.telemetry().get("heater_power_w").unwrap();
    let heater_ac_w = heater_w / 0.9;
    let charge_leg_w = port_w - heater_ac_w;

    // The invariant: the heater never takes budget charging could use —
    // the charge leg holds its full derated demand, the heater is
    // throttled to the remainder, and the port respects the rating.
    assert!(
        (charge_leg_w - 5760.0).abs() < 1e-3,
        "charging draws first within the bound: expected the full derated \
         demand 5760 W, got {charge_leg_w}"
    );
    assert!(
        heater_w > 0.0,
        "the heater takes the remainder of the bound"
    );
    assert!(
        port_w <= 7200.0 + 1e-6,
        "port draw must never exceed the EVSE rating, got {port_w}"
    );

    // Hand-off: step until the pack passes the threshold — the heater
    // stops and the full bound returns to charging.
    for _ in 0..40 {
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
        if ev.battery_temp_c > 10.0 {
            break;
        }
    }
    assert!(
        ev.battery_temp_c > 10.0,
        "the heater must warm the pack through the threshold (hand-off)"
    );
    assert_eq!(
        ev.telemetry().get("heater_power_w").unwrap(),
        0.0,
        "above the threshold the heater is off — the whole bound returns \
         to charging"
    );
}

/// The DR fraction is a multiplier on the allocated AC total, never a
/// ceiling term in the bound: with the pack-demand already below the
/// rating (the derated cold regime), `High` halves the *allocated draw*
/// and `GridEmergency` zeroes the port. The withdrawn ceiling form
/// `min(rated, limit, dr·rated)` would have no effect on a demand already
/// below half the rating.
#[test]
fn dr_scales_the_allocated_total_as_a_multiplier() {
    let make_ev = || {
        let mut raw = base_raw();
        raw.insert(KEY_INITIAL_SOC.to_string(), 0.2.into());
        raw.insert(KEY_BATTERY_TEMP_C.to_string(), 8.0.into());
        raw.insert(KEY_MIN_CHARGE_TEMP_C.to_string(), 0.0.into());
        raw.insert(KEY_FULL_POWER_TEMP_C.to_string(), 10.0.into());
        raw.insert(KEY_HEATER_POWER_W.to_string(), 0.0.into());
        raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());
        let env = sample_env();
        ev.init(&config, &env).unwrap();
        ev
    };

    // Baseline: derated demand 5.76 kW (no heater, cap slack).
    let mut ev = make_ev();
    let env = sample_env();
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    let baseline = ev.telemetry().get("active_power_kw").unwrap();
    assert!((baseline - 5760.0).abs() < 1e-3);

    // High (0.5): the allocated total is halved — 2.88 kW, not the ceiling
    // form's min(7.2, 3.6) = 3.6 (which would leave a below-half-rating
    // demand completely uncurtailed).
    let mut ev = make_ev();
    ev.apply_control_unchecked(&ControlSignal::DemandResponse {
        level: DRLevel::High,
        duration_s: None,
    })
    .unwrap();
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    assert!((ev.telemetry().get("active_power_kw").unwrap() - 5760.0 * 0.5).abs() < 1e-3);

    // Critical (0.25): quartered.
    let mut ev = make_ev();
    ev.apply_control_unchecked(&ControlSignal::DemandResponse {
        level: DRLevel::Critical,
        duration_s: None,
    })
    .unwrap();
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    assert!((ev.telemetry().get("active_power_kw").unwrap() - 5760.0 * 0.25).abs() < 1e-3);

    // GridEmergency (0.0): a commanded zero zeroes the port.
    let mut ev = make_ev();
    ev.apply_control_unchecked(&ControlSignal::DemandResponse {
        level: DRLevel::GridEmergency,
        duration_s: None,
    })
    .unwrap();
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    assert_eq!(ev.telemetry().get("active_power_kw").unwrap(), 0.0);
}

/// The taper bound lands the pack exactly on its commanded target with
/// the heater running — the heater's diversion is covered by the charger's
/// raised import, so no overshoot and no systematic shortfall.
#[test]
fn charging_with_heater_lands_exactly_on_target() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.3.into());
    raw.insert(KEY_BATTERY_TEMP_C.to_string(), 2.0.into());
    raw.insert(KEY_MIN_CHARGE_TEMP_C.to_string(), 0.0.into());
    raw.insert(KEY_FULL_POWER_TEMP_C.to_string(), 10.0.into());
    raw.insert(KEY_HEATER_THRESHOLD_C.to_string(), 5.0.into());
    raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let mut env = sample_env();
    env.weather.outdoor_temp_c = 2.0;
    ev.init(&config, &env).unwrap();
    ev.apply_control_unchecked(&ControlSignal::SOCTarget {
        target_soc: 0.9,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    let mut reached = false;
    for _ in 0..200 {
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
        if ev.soc >= 0.9 - 1e-9 {
            reached = true;
            break;
        }
    }
    assert!(reached, "the session must reach the 0.9 target");
    assert!(
        ev.soc <= 0.9 + 1e-9,
        "no overshoot past the commanded target with the heater running: \
         got {}",
        ev.soc
    );
}

/// A commanded `power_setpoint_w` is a total-draw bound: with the heater
/// running, its AC-equivalent is carved out within the command and the
/// charge leg receives the remainder — the port never draws more than the
/// dispatch commands, and once the pack is warm the full command returns
/// to charging.
#[test]
fn commanded_power_setpoint_bounds_total_draw_with_heater_running() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.2.into());
    raw.insert(KEY_BATTERY_TEMP_C.to_string(), 2.0.into());
    raw.insert(KEY_MIN_CHARGE_TEMP_C.to_string(), 0.0.into());
    raw.insert(KEY_FULL_POWER_TEMP_C.to_string(), 10.0.into());
    raw.insert(KEY_HEATER_THRESHOLD_C.to_string(), 5.0.into());
    raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let mut env = sample_env();
    env.weather.outdoor_temp_c = 2.0;
    ev.init(&config, &env).unwrap();
    ev.apply_control_unchecked(&ControlSignal::PowerSetpoint {
        active_power_w: 3000.0,
        reactive_power_var: None,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    let mut charge_leg_seen = 0.0_f64;
    for _ in 0..40 {
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
        let port_w = ev.telemetry().get("active_power_kw").unwrap();
        assert!(
            port_w <= 3000.0 + 1e-6,
            "total draw must never exceed the commanded setpoint, got {port_w}"
        );
        let heater_ac_w = ev.telemetry().get("heater_power_w").unwrap() / 0.9;
        charge_leg_seen = charge_leg_seen.max(port_w - heater_ac_w);
        if ev.battery_temp_c > 5.0 {
            break;
        }
    }
    // Once warm the heater is off and the whole command charges.
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    let port_w = ev.telemetry().get("active_power_kw").unwrap();
    assert!(port_w <= 3000.0 + 1e-6);
    assert!(
        port_w > 2000.0,
        "with the pack warm the commanded setpoint returns to charging, \
         got {port_w}"
    );
    assert!(charge_leg_seen >= 0.0);
}

/// The deadline-raise exception to the setpoint bound, with the heater
/// running — the combination the allocator's `!deadline_raise_active`
/// filter exists for, and the one surface neither sibling test touches:
/// the setpoint-bound-with-heater test has no deadline, and the
/// deadline-raise test has no heater. While a `DeadlineGuarantee` raise
/// is active the commanded setpoint is a soft floor (the total may
/// legitimately exceed it — clamping it would defeat the deadline, the
/// exact contract conflict the resolution recorded), and the
/// supply-bound priority rule governs instead: within the EVSE rating
/// charging draws first and the heater takes the remainder — the heater
/// never takes budget charging could use, deadline or not.
#[test]
fn urgent_deadline_with_heater_exceeds_soft_setpoint_within_the_supply_bound() {
    // Cold derate band: 2 °C → derate 0.2 → charge demand 7.2 × 0.2 =
    // 1.44 kW AC; heater 1.5 kW → 1.667 kW AC-equivalent (its carve-out
    // alone exceeds the 1.0 kW setpoint, so the charge leg under the
    // setpoint would be zero — the deadline raise is what restores it).
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.2.into());
    raw.insert(KEY_EFFICIENCY.to_string(), 0.9.into());
    raw.insert(KEY_BATTERY_TEMP_C.to_string(), 2.0.into());
    raw.insert(KEY_MIN_CHARGE_TEMP_C.to_string(), 0.0.into());
    raw.insert(KEY_FULL_POWER_TEMP_C.to_string(), 10.0.into());
    raw.insert(KEY_HEATER_POWER_W.to_string(), 1500.0.into());
    raw.insert(KEY_HEATER_THRESHOLD_C.to_string(), 5.0.into());
    raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let mut env = sample_env();
    env.weather.outdoor_temp_c = 2.0;
    ev.init(&config, &env).unwrap();

    ev.apply_control_unchecked(&ControlSignal::EvSetReadyBy {
        departure_hour: 2.0,
        target_soc: 0.9,
    })
    .unwrap();
    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_w: 1000.0,
        reactive_power_var: None,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    let port_w = ev.telemetry().get("active_power_kw").unwrap();
    let heater_ac_w = ev.telemetry().get("heater_power_w").unwrap() / 0.9;

    assert_eq!(
        ev.telemetry().get("heater_power_w").unwrap(),
        1500.0,
        "preconditioning continues during a deadline raise — the heater \
         must not be starved by the raise either"
    );
    assert!(
        port_w > 1000.0 + 1e-6,
        "the deadline raise legitimately exceeds the soft 1.0 kW setpoint \
         (clamping it would defeat the DeadlineGuarantee), got {port_w} W"
    );
    assert!(
        port_w <= 7200.0 + 1e-6,
        "the raise is bounded by the supply-bound rule (the EVSE rating), \
         not free — got {port_w} kW"
    );
    // The priority invariant survives the raise: charging draws first —
    // the charge leg is the full derated demand, the heater takes the
    // remainder on top (a heater monopolizing the budget during a
    // deadline fails here).
    let charge_leg_w = port_w - heater_ac_w;
    assert!(
        (charge_leg_w - 7200.0 * 0.2).abs() < 1e-3,
        "charging must draw first at the derated demand (1.44 kW AC), got \
         charge leg {charge_leg_w} W"
    );
    assert!(
        (port_w - (7200.0 * 0.2 + 1500.0 / 0.9)).abs() < 1e-3,
        "the port is the composed total charge + heater AC-equivalent \
         (≈3106.7 W), got {port_w} W"
    );
}

/// A cold V2L step conserves energy across the vehicle boundary: the
/// pack's debit equals the export converted at `charging_efficiency` plus
/// the heater's unconverted DC draw, the port carries the export alone,
/// and at the reserve floor both the export and the heater stop.
#[test]
fn cold_v2l_export_conserves_pack_energy_and_stops_at_floor() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
    raw.insert(KEY_V2L_ENABLED.to_string(), true.into());
    raw.insert(KEY_V2L_SOC_RESERVE.to_string(), 0.2.into());
    raw.insert(KEY_V2L_MAX_DISCHARGE_W.to_string(), 3000.0.into());
    raw.insert(KEY_BATTERY_TEMP_C.to_string(), 2.0.into());
    raw.insert(KEY_MIN_CHARGE_TEMP_C.to_string(), 0.0.into());
    raw.insert(KEY_FULL_POWER_TEMP_C.to_string(), 10.0.into());
    raw.insert(KEY_HEATER_THRESHOLD_C.to_string(), 5.0.into());
    raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let mut env = sample_env();
    env.weather.outdoor_temp_c = 2.0;
    ev.init(&config, &env).unwrap();
    ev.apply_control_unchecked(&ControlSignal::PowerSetpoint {
        active_power_w: -3000.0,
        reactive_power_var: None,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    let soc_before = ev.soc;
    let capacity = ev.battery_capacity_j;
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

    let port_w = ev.telemetry().get("active_power_kw").unwrap();
    let heater_w = ev.telemetry().get("heater_power_w").unwrap();
    assert!(
        (port_w + 3000.0).abs() < 1e-3,
        "the port carries the export alone, got {port_w}"
    );
    assert!(
        heater_w > 0.0,
        "a cold pack discharging still preconditions"
    );
    // Conservation: debit = export/eta + heater (pack-side DC), over the
    // 15-minute step.
    let debit_w = 3000.0 / 0.9 + heater_w;
    let expected_delta = debit_w * 900.0 / capacity;
    assert!(
        (soc_before - ev.soc - expected_delta).abs() < 1e-9,
        "pack debit must equal export/eta + heater draw: expected SOC delta \
         {expected_delta}, got {}",
        soc_before - ev.soc
    );

    // Floor invariant: run to the reserve — the pack never lands below it,
    // and at the floor both the export and the heater stop.
    let mut hit_floor = false;
    for _ in 0..400 {
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
        assert!(
            ev.soc >= 0.2 - 1e-9,
            "the pack must never land below the effective floor, got {}",
            ev.soc
        );
        if ev.soc <= 0.2 + 1e-9 {
            hit_floor = true;
            break;
        }
    }
    assert!(
        hit_floor,
        "the discharge must reach the floor within the run"
    );
    // The landing step legitimately tapers the export to land exactly ON
    // the floor; the step after it is the assertion that both stop.
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    assert_eq!(
        ev.telemetry().get("active_power_kw").unwrap(),
        0.0,
        "at the floor the export stops"
    );
    assert_eq!(
        ev.telemetry().get("heater_power_w").unwrap(),
        0.0,
        "at the floor the heater stops with the export — preconditioning \
         must not drain the pack below the departure floor"
    );
    assert!(ev.soc >= 0.2 - 1e-9);
}

/// The discharge arm is the one state where the heater's current crosses
/// the cell terminals — the pack is the source for both the export
/// conversion and the heater's DC draw — so the shared ohmic solve's
/// basis there is the pack-side total (export/η + heater). The charging
/// arm excludes the heater (pinned in
/// `heater_current_never_enters_cell_ohmic_loss_while_charging` and the
/// heater-only gate above); this pins the deliberate asymmetry from the
/// other side, so a future "symmetry cleanup" that excludes the heater on
/// both arms loses the heater's I²R exactly where the pack genuinely
/// carries the current. The oracle is the same shared solve production
/// uses, evaluated on the pack-side total draw at the post-debit SOC's
/// OCV.
#[test]
fn cold_discharge_ohmic_loss_covers_export_and_heater_terminal_currents() {
    // Premise mirrors `cold_v2l_export_conserves_pack_energy_and_stops_at_floor`:
    // a 2 °C pack (threshold 5 °C) exporting 3 kW with the heater running,
    // UA 0 so the thermal channel carries no confound.
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
    raw.insert(KEY_V2L_ENABLED.to_string(), true.into());
    raw.insert(KEY_V2L_SOC_RESERVE.to_string(), 0.2.into());
    raw.insert(KEY_V2L_MAX_DISCHARGE_W.to_string(), 3000.0.into());
    raw.insert(KEY_BATTERY_TEMP_C.to_string(), 2.0.into());
    raw.insert(KEY_MIN_CHARGE_TEMP_C.to_string(), 0.0.into());
    raw.insert(KEY_FULL_POWER_TEMP_C.to_string(), 10.0.into());
    raw.insert(KEY_HEATER_THRESHOLD_C.to_string(), 5.0.into());
    raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let mut env = sample_env();
    env.weather.outdoor_temp_c = 2.0;
    ev.init(&config, &env).unwrap();
    ev.apply_control_unchecked(&ControlSignal::PowerSetpoint {
        active_power_w: -3000.0,
        reactive_power_var: None,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

    let heater_w = ev.telemetry().get("heater_power_w").unwrap();
    assert!(
        heater_w > 0.0,
        "premise guard: a cold pack discharging still preconditions"
    );
    // The port carries the export alone — the export magnitude observable.
    let export_w = -ev.telemetry().get("active_power_kw").unwrap();

    // Oracle: the shared solve on the pack-side total draw (export
    // converted at η plus the heater's DC draw — both terminal currents in
    // this state) at the post-debit SOC's OCV, exactly what production
    // feeds it on the discharge arm.
    let eta = ev.charging_efficiency.max(0.01);
    let pack_dc_w = export_w / eta + heater_w;
    let ocv = ev.ocv_table.voltage_at_soc(ev.soc);
    let expected_with_heater = ev.pack_electrical().solve(ocv, -pack_dc_w).ohmic_loss_w;
    let published = ev.telemetry().get("ohmic_loss_w").unwrap();
    assert!(
        (published - expected_with_heater).abs() < 1e-6,
        "the discharge arm's ohmic loss must be I2R of the pack-side total draw \
         (export/η + heater): expected {expected_with_heater} W, got {published} W"
    );

    // Non-vacuous discrimination: excluding the heater's current (the
    // regression this gate exists to catch) drops the ohmic loss by the
    // heater's full I²R — the 5 kW heater's ~14 A is a large share of the
    // terminal current at a 3 kW export.
    let export_dc_w = export_w / eta;
    let expected_export_only = ev.pack_electrical().solve(ocv, -export_dc_w).ohmic_loss_w;
    assert!(
        expected_with_heater > expected_export_only + 1.0,
        "the heater's terminal current must contribute measurably on the \
         discharge arm: {expected_with_heater} W with it vs {expected_export_only} W \
         without — a discrimination gap too small to catch the regression"
    );
}

/// A preconditioning pack (charge leg zero, heater drawing, charger import
/// raised to the heater's AC-equivalent) reports `Heating`, not
/// `Charging`, and the mode/flow guard accepts the state.
#[test]
fn preconditioning_pack_reports_heating_mode() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
    raw.insert(KEY_BATTERY_TEMP_C.to_string(), (-5.0f64).into());
    raw.insert(KEY_MIN_CHARGE_TEMP_C.to_string(), 0.0.into());
    raw.insert(KEY_FULL_POWER_TEMP_C.to_string(), 10.0.into());
    raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let mut env = sample_env();
    env.weather.outdoor_temp_c = -5.0;
    ev.init(&config, &env).unwrap();
    ev.apply_control_unchecked(&ControlSignal::SOCTarget {
        target_soc: 0.9,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

    assert_eq!(
        ev.core_output().state.operating_mode,
        Some(OperatingMode::Heating)
    );
    // The production guard (the same validation the dwelling runs on every
    // CoreOutput) accepts the state: an active mode with nonzero electric
    // flow, no thermal sign to check (the EV emits no thermal_output_w).
    hares_types::equipment::validate_core_contract(
        ev.descriptor(),
        ev.core_output(),
        &mut Vec::new(),
    )
    .expect("the preconditioning state must satisfy the core contract");
}

/// An EV commanded to vars at zero real power (plugged in, at target,
/// heater off) is genuinely active — the inverter is exchanging reactive
/// power — and reports `On`, with the guard's active-mode flow rule
/// satisfied through the reactive term.
#[test]
fn commanded_vars_at_zero_real_power_report_on_mode() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.9.into());
    raw.insert(KEY_BATTERY_TEMP_C.to_string(), 20.0.into());
    raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();
    // At target (default ready_soc = soc_max = 1.0 > 0.9 — pin soc_max so
    // 0.9 is the target and no charge demand exists).
    ev.apply_control_unchecked(&ControlSignal::SOCTarget {
        target_soc: 0.9,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();
    ev.apply_control_unchecked(&ControlSignal::ReactiveSetpoint { var: 2000.0 })
        .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

    assert_eq!(
        ev.core_output().state.operating_mode,
        Some(OperatingMode::On)
    );
    assert_eq!(ev.telemetry().get("active_power_kw").unwrap(), 0.0);
    assert_eq!(ev.telemetry().get("reactive_power_kvar").unwrap(), 2000.0);
    hares_types::equipment::validate_core_contract(
        ev.descriptor(),
        ev.core_output(),
        &mut Vec::new(),
    )
    .expect(
        "standby var support must satisfy the core contract (Rule 1 \
                 counts reactive flow)",
    );
}

/// The away arm's CoreOutput is the dwelling-side truth: zero electric
/// flows (an off-site load must not enter the dwelling's electrical
/// summary, which feeds BMS dispatch) with a mode consistent with them —
/// `Off` — whether the vehicle is charging away or preconditioning. The
/// mode/flow guard validates every CoreOutput; an away `Charging`/`Heating`
/// mode with zero flows fails the simulation (caught by the Python
/// integration suite during the fix; pinned here at the equipment level).
/// Away activity stays observable through `AWAY_CHARGE_POWER_KW` and
/// `HEATER_POWER_W`.
#[test]
fn away_activity_reports_off_mode_consistent_with_zero_dwelling_flows() {
    // Cold pack: preconditioning while away-charging — the heater runs.
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.3.into());
    raw.insert(KEY_BATTERY_TEMP_C.to_string(), (-5.0f64).into());
    raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
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
    ev.apply_control_unchecked(&ControlSignal::EvAwayCharge { power_w: 11500.0 })
        .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    assert_eq!(
        ev.core_output().state.operating_mode,
        Some(OperatingMode::Off)
    );
    assert!(
        ev.telemetry().get("heater_power_w").unwrap() > 0.0,
        "the away heater runs (supply = the commanded away charger)"
    );
    assert_eq!(
        ev.telemetry().get("active_power_kw").unwrap(),
        0.0,
        "no residential port contribution while away"
    );
    hares_types::equipment::validate_core_contract(
        ev.descriptor(),
        ev.core_output(),
        &mut Vec::new(),
    )
    .expect(
        "away activity with zero dwelling flows must satisfy the \
                 core contract",
    );

    // Warm pack: away charging — same dwelling-side contract.
    let mut raw_warm = base_raw();
    raw_warm.insert(KEY_INITIAL_SOC.to_string(), 0.3.into());
    raw_warm.insert(KEY_BATTERY_TEMP_C.to_string(), 20.0.into());
    let config_warm = ev_config(raw_warm);
    let mut ev_warm = Ev::new(config_warm.clone());
    ev_warm.init(&config_warm, &sample_env()).unwrap();
    ev_warm
        .apply_control_unchecked(&ControlSignal::EvPlugIn {
            state: EvConnectionState::Disconnected,
        })
        .unwrap();
    ev_warm
        .apply_control_unchecked(&ControlSignal::EvPlugIn {
            state: EvConnectionState::AwayPluggedIn,
        })
        .unwrap();
    ev_warm
        .apply_control_unchecked(&ControlSignal::EvAwayCharge { power_w: 11500.0 })
        .unwrap();
    let mut ports = PortSlots::default();
    ev_warm
        .step(&sample_env(), Duration::minutes(15), &mut ports)
        .unwrap();
    assert_eq!(
        ev_warm.core_output().state.operating_mode,
        Some(OperatingMode::Off)
    );
    assert!(ev_warm.telemetry().get("away_charge_power_kw").unwrap() > 0.0);
    hares_types::equipment::validate_core_contract(
        ev_warm.descriptor(),
        ev_warm.core_output(),
        &mut Vec::new(),
    )
    .expect(
        "away charging with zero dwelling flows must satisfy the core \
             contract",
    );
}

/// Preconditioning needs a supply: the heater gate keys on pack
/// temperature *whenever the vehicle is connected with an energized
/// supply* — so with the contact open (`Disconnected`) or plugged in away
/// with no charger commanded, a cold pack must not draw or warm anything.
/// The D3 fix removed the gate's charge-demand keying; these are the
/// no-supply arms of the same gate, where the correct behavior is still
/// "off" — the inverse regression (a heater that runs from nothing)
/// would bill phantom energy at the port or silently drain the pack to
/// warm itself.
#[test]
fn heater_does_not_run_without_a_supply_disconnected_or_away_idle() {
    // UA pinned to 0 so "warmed nothing" is exact: with no drift term the
    // pack temperature can only move if heat was actually applied.
    let make_cold_ev = || {
        let mut raw = base_raw();
        raw.insert(KEY_BATTERY_TEMP_C.to_string(), (-5.0f64).into());
        raw.insert(KEY_HEATER_POWER_W.to_string(), 1500.0.into());
        raw.insert(KEY_HEATER_THRESHOLD_C.to_string(), 5.0.into());
        raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());
        ev.init(&config, &sample_env()).unwrap();
        ev
    };

    // Disconnected: contact open, drift only.
    let mut ev = make_cold_ev();
    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::Disconnected,
    })
    .unwrap();
    let soc_before = ev.soc;
    let mut ports = PortSlots::default();
    ev.step(&sample_env(), Duration::minutes(15), &mut ports)
        .unwrap();
    assert_eq!(
        ev.telemetry().get("heater_power_w").unwrap(),
        0.0,
        "a disconnected pack has no supply — the heater must stay off"
    );
    assert_eq!(ev.telemetry().get("active_power_kw").unwrap(), 0.0);
    assert_eq!(ev.soc, soc_before, "nothing may debit a disconnected pack");
    assert_eq!(
        ev.battery_temp_c, -5.0,
        "with UA = 0 the temperature can only move if heat was applied — \
         a disconnected pack must warm nothing"
    );

    // Away and idle: plugged in off-site but no away charger commanded —
    // the only modeled away supply is the commanded away charger.
    let mut ev = make_cold_ev();
    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::Disconnected,
    })
    .unwrap();
    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::AwayPluggedIn,
    })
    .unwrap();
    ev.apply_control_unchecked(&ControlSignal::EvAwayCharge { power_w: 0.0 })
        .unwrap();
    let soc_before = ev.soc;
    let mut ports = PortSlots::default();
    ev.step(&sample_env(), Duration::minutes(15), &mut ports)
        .unwrap();
    assert_eq!(
        ev.telemetry().get("heater_power_w").unwrap(),
        0.0,
        "an away pack with no commanded charger has no supply — the heater \
         must stay off"
    );
    assert_eq!(ev.telemetry().get("active_power_kw").unwrap(), 0.0);
    assert_eq!(ev.soc, soc_before);
    assert_eq!(
        ev.battery_temp_c, -5.0,
        "no supply means no warming: the pack must hold its temperature"
    );
}

/// A de-energized home bus removes the EVSE supply: during a utility
/// outage a cold plugged-in pack must not draw the heater (the EVSE is
/// dead — nothing to convert, nothing to bill), while the same pack
/// *discharging* (V2L — the vehicle itself is the supply; the cold-
/// weather outage with the vehicle backing up the home is the flagship
/// collision the floor invariant exists for) keeps preconditioning.
/// Pins both arms of the gate's dead-bus supply resolution.
#[test]
fn grid_outage_suspends_preconditioning_except_while_the_vehicle_discharges() {
    // Idle-at-home segment: cold pack, heater configured, charge
    // commanded, dead bus, no discharge.
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
    raw.insert(KEY_BATTERY_TEMP_C.to_string(), (-5.0f64).into());
    raw.insert(KEY_HEATER_POWER_W.to_string(), 1500.0.into());
    raw.insert(KEY_HEATER_THRESHOLD_C.to_string(), 5.0.into());
    raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    ev.init(&config, &sample_env()).unwrap();
    ev.apply_control_unchecked(&ControlSignal::SOCTarget {
        target_soc: 0.9,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    let mut env_outage = sample_env();
    env_outage.grid.voltage_pu = 0.0;
    let mut ports = PortSlots::default();
    ev.step(&env_outage, Duration::minutes(15), &mut ports)
        .unwrap();
    assert_eq!(
        ports.electrical.load_power_w, 0.0,
        "a dead EVSE bills nothing"
    );
    assert_eq!(
        ev.telemetry().get("heater_power_w").unwrap(),
        0.0,
        "the heater must not draw during an outage — the EVSE supply is \
         dead, so thermal management is suspended (recovering after the \
         event is bounded and modeled)"
    );
    assert_eq!(
        ev.battery_temp_c, -5.0,
        "with UA = 0 the temperature can only move if heat was applied — \
         an outage pack must warm nothing"
    );

    // V2L segment: same dead bus, but the vehicle is discharging — it is
    // its own supply, so preconditioning continues.
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
    raw.insert(KEY_V2L_ENABLED.to_string(), true.into());
    raw.insert(KEY_V2L_SOC_RESERVE.to_string(), 0.2.into());
    raw.insert(KEY_V2L_MAX_DISCHARGE_W.to_string(), 3000.0.into());
    raw.insert(KEY_BATTERY_TEMP_C.to_string(), 2.0.into());
    raw.insert(KEY_HEATER_POWER_W.to_string(), 1500.0.into());
    raw.insert(KEY_HEATER_THRESHOLD_C.to_string(), 5.0.into());
    raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    ev.init(&config, &sample_env()).unwrap();
    ev.apply_control_unchecked(&ControlSignal::PowerSetpoint {
        active_power_w: -3000.0,
        reactive_power_var: None,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();
    let mut ports = PortSlots::default();
    ev.step(&env_outage, Duration::minutes(15), &mut ports)
        .unwrap();
    assert!(
        ev.telemetry().get("active_power_kw").unwrap() < 0.0,
        "V2L discharge is not gated by the dead bus (the EV is the source)"
    );
    assert!(
        ev.telemetry().get("heater_power_w").unwrap() > 0.0,
        "a discharging vehicle is its own supply — a cold pack keeps \
         preconditioning through the outage (the flagship cold-weather \
         outage case), and the floor invariant bounds the draw"
    );
}

/// The heater gate is independent of charge demand: a pack already at
/// its target (zero charge demand — nothing to charge, nothing blocked
/// by the derate) still preconditions while cold. The pre-fix gate keyed
/// heater activity on charge demand (`would_charge_underated`), which is
/// dead exactly when the vehicle is satisfied-and-cold: this is that
/// face of the D3 defect, distinct from the derate-blocked face the
/// reproduction pins.
#[test]
fn heater_preconditions_an_idle_pack_with_no_charge_demand() {
    // SOC at the target (soc_max default 1.0): charge demand is zero.
    // Pack at 3 °C: above the plating cutoff, below the 5 °C heater
    // threshold. UA = 0 isolates the heater as the only thermal term.
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 1.0.into());
    raw.insert(KEY_BATTERY_TEMP_C.to_string(), 3.0.into());
    raw.insert(KEY_HEATER_POWER_W.to_string(), 1500.0.into());
    raw.insert(KEY_HEATER_THRESHOLD_C.to_string(), 5.0.into());
    raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    ev.init(&config, &sample_env()).unwrap();
    let soc_before = ev.soc;

    let mut ports = PortSlots::default();
    ev.step(&sample_env(), Duration::minutes(15), &mut ports)
        .unwrap();

    assert_eq!(
        ev.telemetry().get("heater_power_w").unwrap(),
        1500.0,
        "a cold pack preconditions even with zero charge demand — the \
         heater gate keys on pack temperature, not on charge demand"
    );
    let port_w = ev.telemetry().get("active_power_kw").unwrap();
    let heater_ac_eq_w = 1500.0 / 0.9;
    assert!(
        (port_w - heater_ac_eq_w).abs() < 1e-6,
        "the idle heater-only state bills the heater's AC-equivalent \
         ({heater_ac_eq_w} W) at the port, got {port_w} W"
    );
    assert_eq!(
        ev.soc, soc_before,
        "the charger's raised import covers the heater exactly — the pack \
         nets zero and SOC must not move"
    );
    assert_eq!(
        ev.core_output().state.operating_mode,
        Some(OperatingMode::Heating),
        "the idle heater-only state (charge leg zero, heater drawing) \
         reports Heating, not Charging/Off"
    );
    hares_types::equipment::validate_core_contract(
        ev.descriptor(),
        ev.core_output(),
        &mut Vec::new(),
    )
    .expect("idle preconditioning must satisfy the core contract");
}

/// The DR fraction is a multiplier on the *allocated AC total* — the
/// charge leg plus the heater's AC-equivalent — and a commanded zero
/// (GridEmergency) suspends thermal management entirely: port and heater
/// both go to zero. The heater-composed total is the allocation the cold
/// charging session's normal control path actually runs; the withdrawn
/// ceiling form (`min(rating, limit, dr·rated)`) would have left a
/// below-half-rating demand completely uncurtailed in exactly this
/// regime, and the composition is pinned here against reintroduction.
#[test]
fn dr_scales_the_charge_plus_heater_total_and_grid_emergency_suspends_preconditioning() {
    // Pack at 4 °C → derate 0.4 → charge leg 7.2 × 0.4 = 2.88 kW AC;
    // heater 1.5 kW → 1.667 kW AC-equivalent; total 4.547 kW, cap slack.
    let make_ev = || {
        let mut raw = base_raw();
        raw.insert(KEY_INITIAL_SOC.to_string(), 0.2.into());
        raw.insert(KEY_BATTERY_TEMP_C.to_string(), 4.0.into());
        raw.insert(KEY_MIN_CHARGE_TEMP_C.to_string(), 0.0.into());
        raw.insert(KEY_FULL_POWER_TEMP_C.to_string(), 10.0.into());
        raw.insert(KEY_HEATER_POWER_W.to_string(), 1500.0.into());
        raw.insert(KEY_HEATER_THRESHOLD_C.to_string(), 5.0.into());
        raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());
        ev.init(&config, &sample_env()).unwrap();
        ev
    };
    let expected_total = 7200.0 * 0.4 + 1500.0 / 0.9;

    // Baseline: the composed total, undistorted.
    let mut ev = make_ev();
    let mut ports = PortSlots::default();
    ev.step(&sample_env(), Duration::minutes(15), &mut ports)
        .unwrap();
    let baseline = ev.telemetry().get("active_power_kw").unwrap();
    assert!(
        (baseline - expected_total).abs() < 1e-6,
        "baseline must be the charge+heater composed total {expected_total} \
         kW, got {baseline} kW"
    );

    // High (0.5): the *allocated total* is halved — both legs scale.
    let mut ev = make_ev();
    ev.apply_control_unchecked(&ControlSignal::DemandResponse {
        level: DRLevel::High,
        duration_s: None,
    })
    .unwrap();
    let mut ports = PortSlots::default();
    ev.step(&sample_env(), Duration::minutes(15), &mut ports)
        .unwrap();
    let high = ev.telemetry().get("active_power_kw").unwrap();
    assert!(
        (high - expected_total * 0.5).abs() < 1e-6,
        "High DR must halve the charge+heater total to \
         {} kW, got {high} kW",
        expected_total * 0.5
    );
    assert!(
        ev.telemetry().get("heater_power_w").unwrap() > 0.0,
        "routine DR throttles the heater, it does not starve it — the \
         steady-state hold is comfortably inside High's allocation"
    );

    // GridEmergency (a commanded zero): the port zeroes AND the heater
    // suspends — thermal management is suspended entirely until the
    // event clears, which is commanded behavior, not a defect.
    let mut ev = make_ev();
    ev.apply_control_unchecked(&ControlSignal::DemandResponse {
        level: DRLevel::GridEmergency,
        duration_s: None,
    })
    .unwrap();
    let mut ports = PortSlots::default();
    ev.step(&sample_env(), Duration::minutes(15), &mut ports)
        .unwrap();
    assert_eq!(
        ev.telemetry().get("active_power_kw").unwrap(),
        0.0,
        "a commanded zero zeroes the port"
    );
    assert_eq!(
        ev.telemetry().get("heater_power_w").unwrap(),
        0.0,
        "a commanded zero suspends preconditioning with it — the DR \
         multiplier applies to the allocated total, heater included"
    );
}

/// A commanded zero (`GridEmergency`) during a commanded V2L/V2G discharge
/// zeroes the exported AC power — the port — while the discharge dispatch
/// and the pack-side heater stay active by design (the heater's
/// discharge-side draw is bounded by the floor invariant, not the DR
/// fraction). The pack then nets negative on the heater's DC draw alone,
/// and `classify_mode` — keying on the pack-side net rate — reports
/// `Discharging` with every published flow at zero: the exported AC power
/// is `-0.0`, no reactive is served (unity power factor on a zero inverter
/// leg), and the EV emits no thermal or fuel. The mode-flow guard's Rule 1
/// (an active mode requires a nonzero flow) rejects that (mode, flows)
/// pair, and the dwelling's post-step `validate_core_contract` fails the
/// simulation — the cold-weather V2L backup event, the plan's flagship
/// discharge case, is exactly where a grid emergency and a cold pack
/// collide.
#[test]
fn grid_emergency_discharge_with_running_heater_publishes_a_guard_valid_output() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
    raw.insert(KEY_V2L_ENABLED.to_string(), true.into());
    raw.insert(KEY_V2L_SOC_RESERVE.to_string(), 0.2.into());
    raw.insert(KEY_V2L_MAX_DISCHARGE_W.to_string(), 3000.0.into());
    raw.insert(KEY_BATTERY_TEMP_C.to_string(), 2.0.into());
    raw.insert(KEY_MIN_CHARGE_TEMP_C.to_string(), 0.0.into());
    raw.insert(KEY_FULL_POWER_TEMP_C.to_string(), 10.0.into());
    raw.insert(KEY_HEATER_THRESHOLD_C.to_string(), 5.0.into());
    raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let mut env = sample_env();
    env.weather.outdoor_temp_c = 2.0;
    ev.init(&config, &env).unwrap();
    // The V2L dispatch: a negative setpoint with V2L enabled.
    ev.apply_control_unchecked(&ControlSignal::PowerSetpoint {
        active_power_w: -3000.0,
        reactive_power_var: None,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();
    // The commanded zero: the DR fraction zeroes the export while the
    // heater's pack-side draw is deliberately left DR-unbounded.
    ev.apply_control_unchecked(&ControlSignal::DemandResponse {
        level: DRLevel::GridEmergency,
        duration_s: None,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

    // The corner's observable shape: the heater draws from the pack while
    // the port exports nothing.
    assert_eq!(
        ev.telemetry().get("active_power_kw").unwrap(),
        0.0,
        "the commanded zero must zero the port export"
    );
    assert!(
        ev.telemetry().get("heater_power_w").unwrap() > 0.0,
        "the discharge-side heater is bounded by the floor invariant, not \
         the DR fraction — it keeps preconditioning through the event"
    );
    hares_types::equipment::validate_core_contract(
        ev.descriptor(),
        ev.core_output(),
        &mut Vec::new(),
    )
    .expect(
        "a GridEmergency V2L step with the pack heater running must publish \
             a (mode, flows) pair the core contract accepts",
    );
}

/// The same GridEmergency discharge corner through the V2G leg: both
/// discharge legs share `compute_discharge` and the pack-side heater
/// netting, so the class spans them — a fix keyed to the V2L leg alone
/// would leave the grid-service leg failing the same guard.
#[test]
fn grid_emergency_v2g_discharge_with_running_heater_publishes_a_guard_valid_output() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
    raw.insert(KEY_V2G_ENABLED.to_string(), true.into());
    raw.insert(KEY_V2G_SOC_RESERVE.to_string(), 0.3.into());
    raw.insert(KEY_V2G_MAX_DISCHARGE_W.to_string(), 5000.0.into());
    raw.insert(KEY_BATTERY_TEMP_C.to_string(), 2.0.into());
    raw.insert(KEY_MIN_CHARGE_TEMP_C.to_string(), 0.0.into());
    raw.insert(KEY_FULL_POWER_TEMP_C.to_string(), 10.0.into());
    raw.insert(KEY_HEATER_THRESHOLD_C.to_string(), 5.0.into());
    raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let mut env = sample_env();
    env.weather.outdoor_temp_c = 2.0;
    ev.init(&config, &env).unwrap();
    ev.apply_control_unchecked(&ControlSignal::PowerSetpoint {
        active_power_w: -5000.0,
        reactive_power_var: None,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();
    ev.apply_control_unchecked(&ControlSignal::DemandResponse {
        level: DRLevel::GridEmergency,
        duration_s: None,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

    assert_eq!(
        ev.telemetry().get("active_power_kw").unwrap(),
        0.0,
        "the commanded zero must zero the port export on the V2G leg too"
    );
    assert!(
        ev.telemetry().get("heater_power_w").unwrap() > 0.0,
        "the V2G leg's pack-side heater is bounded by the floor invariant, \
         not the DR fraction"
    );
    hares_types::equipment::validate_core_contract(
        ev.descriptor(),
        ev.core_output(),
        &mut Vec::new(),
    )
    .expect(
        "a GridEmergency V2G step with the pack heater running must publish \
             a (mode, flows) pair the core contract accepts",
    );
}

/// The GridEmergency discharge corner's *label* is the fix's semantic
/// content, and the contract-validity gates above cannot pin it: `Off`
/// with zero flows passes `validate_core_contract` exactly as `Standby`
/// does, so a regression that relabels the pack-fed preconditioning pack
/// (the export commanded to zero while the discharge-side heater draws
/// from the pack) as `Off` would stay green above while losing the label
/// the fix defines — energized and connected, exchanging nothing at the
/// port, the same label the stationary Battery reports for a
/// commanded-zero discharge and the guard's own `resolve_idle` maps
/// active-with-zero-flow states to. This gate pins the mode **value** on
/// the shared `classify_mode` choke point (one call site, both discharge
/// legs through `compute_discharge` — the V2G leg's gate above proves
/// the corner is reached on that leg too), and — in the restore family's
/// round-trip pattern — the checkpointed label at restore: a restore
/// that re-derived the mode from the zero port (`Off`) would silently
/// lose the saved state's discharge dispatch, guard-valid, uncaught by
/// the mid-discharge restore gate (whose premise exports nonzero).
#[test]
fn pack_fed_preconditioning_reports_and_restores_standby_mode() {
    // Premise mirrors `grid_emergency_discharge_with_running_heater…`
    // (the V2L leg): a cold pack discharging with the heater running,
    // the export commanded to zero by a GridEmergency DR event.
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
    raw.insert(KEY_V2L_ENABLED.to_string(), true.into());
    raw.insert(KEY_V2L_SOC_RESERVE.to_string(), 0.2.into());
    raw.insert(KEY_V2L_MAX_DISCHARGE_W.to_string(), 3000.0.into());
    raw.insert(KEY_BATTERY_TEMP_C.to_string(), 2.0.into());
    raw.insert(KEY_MIN_CHARGE_TEMP_C.to_string(), 0.0.into());
    raw.insert(KEY_FULL_POWER_TEMP_C.to_string(), 10.0.into());
    raw.insert(KEY_HEATER_THRESHOLD_C.to_string(), 5.0.into());
    raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let mut env = sample_env();
    env.weather.outdoor_temp_c = 2.0;
    ev.init(&config, &env).unwrap();
    ev.apply_control_unchecked(&ControlSignal::PowerSetpoint {
        active_power_w: -3000.0,
        reactive_power_var: None,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();
    ev.apply_control_unchecked(&ControlSignal::DemandResponse {
        level: DRLevel::GridEmergency,
        duration_s: None,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

    // Non-vacuous preconditions: the corner is really the corner (zero
    // port, running heater — the same shape the gates above assert).
    assert_eq!(
        ev.telemetry().get("active_power_kw").unwrap(),
        0.0,
        "precondition: the commanded zero zeroes the port export"
    );
    assert!(
        ev.telemetry().get("heater_power_w").unwrap() > 0.0,
        "precondition: the discharge-side heater keeps preconditioning \
         through the event"
    );
    // The gate's own content: the step's label is `Standby` — not `Off`,
    // which the contract would accept just as silently.
    assert_eq!(
        ev.core_output().state.operating_mode,
        Some(OperatingMode::Standby),
        "a pack-fed preconditioning pack must report `Standby` — energized \
         and connected, exchanging nothing at the port — not `Off`"
    );

    // The restore face: the checkpointed label survives the round-trip
    // (the restore publishes the checkpointed `last_mode` verbatim, so
    // the restored state matches the saved state — the family's contract).
    let state = ev.save_state().unwrap();
    let mut restored = Ev::new(config.clone());
    restored.init(&config, &env).unwrap();
    restored.load_state(&state).unwrap();
    assert_eq!(
        restored.core_output().state.operating_mode,
        Some(OperatingMode::Standby),
        "a checkpoint saved in the pack-fed preconditioning corner must \
         restore the `Standby` label — a restore that re-derives the mode \
         from the zero port loses the saved state's discharge dispatch"
    );
    hares_types::equipment::validate_core_contract(
        restored.descriptor(),
        restored.core_output(),
        &mut Vec::new(),
    )
    .expect("the restored (mode, flows) pair must satisfy the core contract");
}

/// A charge leg below the mode classifier's `1e-9` kW charging threshold
/// must still publish a (mode, flows) pair the core contract accepts.
/// The taper limit is a continuum — `(soc_limit − soc) · capacity / dt /
/// η` — so any SOC residue below the target produces a nonzero charge
/// leg, and a residue small enough (float dust after a taper landing, or
/// a hand-set SOC 1e-12 below target) puts the pack-side net below the
/// classifier's charging threshold while the published electric flow is
/// still nonzero: `classify_mode` reports `Off`, and the mode-flow
/// guard's Rule 2 (Off forbids non-zero flows) rejects the pair — the
/// dwelling's post-step `validate_core_contract` fails the simulation.
/// Pre-fix the classifier keyed on the port power's sign (any positive
/// power charged — active mode with a nonzero flow, guard-safe); the
/// threshold introduced the window.
#[test]
fn sub_threshold_charge_leg_off_mode_still_publishes_a_nonzero_flow() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.899_999_999_999_f64.into());
    raw.insert(KEY_BATTERY_TEMP_C.to_string(), 20.0.into());
    raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();
    // Target 0.9 with the SOC 1e-12 below it: the taper limit is
    // ~2.7e-10 kW, inside the classifier's sub-threshold window (0, 1e-9]
    // kW — nonzero at the port, below the charging classification.
    ev.apply_control_unchecked(&ControlSignal::SOCTarget {
        target_soc: 0.9,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

    // The corner's observable shape: the charge leg really flowed — a
    // nonzero, sub-threshold port draw.
    let port_w = ev.telemetry().get("active_power_kw").unwrap();
    assert!(
        port_w > 0.0 && port_w <= 1e-6,
        "precondition: the charge leg must be nonzero and within the \
         classifier's sub-threshold window (0, 1e-6] W, got {port_w}"
    );
    hares_types::equipment::validate_core_contract(
        ev.descriptor(),
        ev.core_output(),
        &mut Vec::new(),
    )
    .expect(
        "a sub-threshold charge leg must publish a (mode, flows) pair the \
             core contract accepts — Off with a nonzero flow fails Rule 2",
    );
}

/// The natural-reachability face of the sub-threshold window: a real
/// charging session's last step before the target is taper-limited
/// (headroom below one step's demand-limited SOC gain), and the residue
/// the landing leaves is float-dust — one to a few ULPs of the target
/// SOC. The violation window is `residue ≲ 4.2e-12 SOC` (the charge
/// leg's post-η net ≤ the classifier's 1e-9 kW threshold while the
/// pre-η port flow stays nonzero), so a landing that rounds short by
/// even one ULP of 0.9 (≈1.1e-16) lands inside it. This sweep drives
/// landings from legal starting SOCs across the whole residue range —
/// a single ULP below the target up to demand-limited overshoots —
/// collecting (not failing fast on) any step whose published (mode,
/// flows) pair the dwelling's contract rejects, then asserts none. The
/// failure message is the measured boundary map; the 1e-12 residue is
/// intentionally absent (the dedicated gate above owns it). A clean
/// sweep would be the evidence that the corner needs an adversarial
/// SOC rather than an ordinary charging session.
#[test]
fn taper_landings_across_a_soc_sweep_publish_contract_valid_outputs() {
    let one_ulp_below_target = f64::from_bits(0.9_f64.to_bits() - 1);
    let residues = [
        0.9 - one_ulp_below_target,
        1e-15,
        1e-13,
        4e-12,
        1e-11,
        1e-10,
        1e-8,
        1e-6,
        1e-4,
        1e-3,
        1e-2,
        0.027,
    ];
    let mut violating: Vec<f64> = Vec::new();
    for residue in residues {
        let mut raw = base_raw();
        raw.insert(
            KEY_INITIAL_SOC.to_string(),
            (0.9 - residue).clamp(0.0, 1.0).into(),
        );
        raw.insert(KEY_BATTERY_TEMP_C.to_string(), 20.0.into());
        raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());
        let env = sample_env();
        ev.init(&config, &env).unwrap();
        ev.apply_control_unchecked(&ControlSignal::SOCTarget {
            target_soc: 0.9,
            min_soc: None,
            max_soc: None,
        })
        .unwrap();

        for _ in 0..24 {
            let mut ports = PortSlots::default();
            ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
            if hares_types::equipment::validate_core_contract(
                ev.descriptor(),
                ev.core_output(),
                &mut Vec::new(),
            )
            .is_err()
            {
                violating.push(residue);
                break;
            }
            if ev.soc >= 0.9 {
                break;
            }
        }
        assert!(
            ev.soc >= 0.9,
            "precondition: the charge from residue {residue} must reach \
             the target within the step budget, got {}",
            ev.soc
        );
    }
    assert!(
        violating.is_empty(),
        "taper landings from these SOC residues below the charge target \
         published (mode, flows) pairs the dwelling's core contract \
         rejects (Off mode with a nonzero sub-threshold charge flow — \
         the violation window measured against the classifier's 1e-9 kW \
         threshold): {violating:?}"
    );
}

/// The discharge-side mirror of
/// `sub_threshold_charge_leg_off_mode_still_publishes_a_nonzero_flow`:
/// the same exact-sign keying on the export leg — a V2L landing that
/// rounds short of the reserve floor leaves the next step's export
/// capped to the float-dust headroom (inside (−1e-9, 0) kW, nonzero at
/// the port), and the pre-fix `< −1e-9` epsilon classified it `Off`
/// while the published electric flow carried the value — the same
/// Rule 2 violation (Off forbids nonzero flows) the charge-side gate
/// pins from the other leg. One gate on the shared `classify_mode`
/// choke point: the charge-side gate cannot catch an export-leg epsilon
/// regression (its premise is a charge leg), so this is a distinct face
/// of the same mechanism, not a repeat.
#[test]
fn sub_threshold_export_leg_discharge_mode_still_publishes_a_nonzero_flow() {
    // The floor mirror of the charge gate's premise: the SOC sits 1e-12
    // ABOVE the reserve floor, so the discharge budget caps the export
    // to the float-dust headroom — ~2.2e-10 kW at this capacity, inside
    // (−1e-9, 0) — nonzero at the port, below the old classification
    // threshold.
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.200_000_000_001_f64.into());
    raw.insert(KEY_V2L_ENABLED.to_string(), true.into());
    raw.insert(KEY_V2L_SOC_RESERVE.to_string(), 0.2.into());
    raw.insert(KEY_V2L_MAX_DISCHARGE_W.to_string(), 3000.0.into());
    raw.insert(KEY_BATTERY_TEMP_C.to_string(), 20.0.into());
    raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();
    ev.apply_control_unchecked(&ControlSignal::PowerSetpoint {
        active_power_w: -3000.0,
        reactive_power_var: None,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

    // The corner's observable shape: the export really flowed — a
    // nonzero, sub-threshold negative port flow.
    let port_w = ev.telemetry().get("active_power_kw").unwrap();
    assert!(
        (-1e-6..0.0).contains(&port_w),
        "precondition: the export leg must be nonzero and within the \
         classifier's sub-threshold window (−1e-6, 0] W, got {port_w}"
    );
    // The label: the tiny export is a grid-facing discharge — `Discharging`,
    // never `Off` (which the guard would reject with the nonzero flow).
    assert_eq!(
        ev.core_output().state.operating_mode,
        Some(OperatingMode::Discharging),
        "a sub-threshold export leg must report `Discharging` — not `Off`, \
         which pairs with the nonzero published flow into a Rule 2 violation"
    );
    hares_types::equipment::validate_core_contract(
        ev.descriptor(),
        ev.core_output(),
        &mut Vec::new(),
    )
    .expect(
        "a sub-threshold export leg must publish a (mode, flows) pair the \
             core contract accepts — Off with a nonzero flow fails Rule 2",
    );
}

/// A latched var setpoint must not be served by a de-energized EVSE: the
/// dead-bus gate zeroes the reactive flow (`compute_reactive_var` is
/// skipped when the bus is dead and the leg is not discharging), so an
/// outage step while charging-idle publishes `Off` with all-zero flows —
/// a vars leak here would publish the vars-only `On` mode on a dead bus
/// (physically impossible: a dead EVSE cannot exchange vars), so the
/// mode and the reactive flow are pinned together with the contract.
#[test]
fn dead_bus_suppresses_a_latched_var_setpoint_flows_all_zero() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();
    // Warm pack (ambient-resolved at the 10 °C test environment, above the
    // 5 °C heater threshold): charging-idle, no heater, only the latched
    // var command.
    ev.apply_control_unchecked(&ControlSignal::ReactiveSetpoint { var: 1000.0 })
        .unwrap();
    let mut env_outage = sample_env();
    env_outage.grid.voltage_pu = 0.0;

    let mut ports = PortSlots::default();
    ev.step(&env_outage, Duration::minutes(15), &mut ports)
        .unwrap();

    assert_eq!(
        ev.telemetry().get("reactive_power_kvar").unwrap(),
        0.0,
        "a de-energized EVSE must serve no vars — the dead-bus gate zeroes \
         the latched setpoint"
    );
    assert_eq!(
        ev.core_output().state.operating_mode,
        Some(OperatingMode::Off),
        "with the reactive flow zeroed by the dead bus, the mode must be \
         Off — not the vars-only `On` label a served setpoint would earn"
    );
    hares_types::equipment::validate_core_contract(
        ev.descriptor(),
        ev.core_output(),
        &mut Vec::new(),
    )
    .expect("the dead-bus step must publish a guard-valid pair");
}

/// The away arm zeroes the reactive flow: a latched var setpoint must not
/// leak into the away state, whose mode is `Off` with zero dwelling flows
/// — a leak would pair `Off` with a nonzero reactive flow (Rule 2) and
/// kill every dwelling step while the vehicle is off-site.
#[test]
fn away_arm_zeroes_a_latched_var_setpoint_off_mode_flows_all_zero() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();
    ev.apply_control_unchecked(&ControlSignal::ReactiveSetpoint { var: 1000.0 })
        .unwrap();
    // The away transition goes through Disconnected (the same path the
    // away-charging tests take).
    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::Disconnected,
    })
    .unwrap();
    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::AwayPluggedIn,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

    assert_eq!(
        ev.telemetry().get("reactive_power_kvar").unwrap(),
        0.0,
        "the away arm must zero the latched var setpoint — an off-site \
         vehicle contributes no reactive flow to the dwelling"
    );
    assert_eq!(
        ev.core_output().state.operating_mode,
        Some(OperatingMode::Off),
        "the away mode is `Off`, the dwelling-side truth — and it must \
         pair with the zeroed flows or Rule 2 fails every dwelling step"
    );
    hares_types::equipment::validate_core_contract(
        ev.descriptor(),
        ev.core_output(),
        &mut Vec::new(),
    )
    .expect("the away step must publish a guard-valid pair");
}

/// The out-of-domain whipsaw pathology stays gone: through ten days of
/// physical-temperature charge/drive cycling the usable capacity stays
/// within a physical band of rated and day-over-day changes stay bounded.
/// Pre-fix, misattributed charger losses drove the pack to 165–281 °C and
/// the Smith 2017 fit returned negative fade — capacity whipsawing
/// 75 → 208 → 75 kWh day over day.
#[test]
fn usable_capacity_stays_physical_through_daily_cycling() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.9.into());
    raw.insert(KEY_BATTERY_TEMP_C.to_string(), 20.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let mut env = sample_env();
    // A step helper that advances the clock: the degradation model's
    // capacity recomputation fires at day boundaries, so a test that never
    // crosses midnight exercises a degenerate path (capacity never
    // recomputed) and proves nothing.
    let step = |ev: &mut Ev, env: &mut EnvironmentState| {
        let mut ports = PortSlots::default();
        ev.step(env, Duration::minutes(15), &mut ports).unwrap();
        env.current_time += ChronoDuration::minutes(15);
    };
    ev.init(&config, &env).unwrap();

    let rated = ev.battery_capacity_j;
    let mut prev_capacity = ev.battery_capacity_j;
    for _day in 0..10 {
        // Overnight at target (calendar aging at rest, physical temps).
        for _ in 0..64 {
            step(&mut ev, &mut env);
        }
        // Daily drive: 15 kWh delivered while disconnected (drive I²R
        // through the pack electrical model).
        ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
            state: EvConnectionState::Disconnected,
        })
        .unwrap();
        ev.apply_control_unchecked(&ControlSignal::EvDrive {
            energy_j: 15.0 * 3.6e6,
        })
        .unwrap();
        step(&mut ev, &mut env);
        // Home, charge back to target.
        ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
            state: EvConnectionState::HomePluggedIn,
        })
        .unwrap();
        for _ in 0..32 {
            step(&mut ev, &mut env);
            if ev.soc >= 0.9 - 1e-9 {
                break;
            }
        }
        let capacity = ev.battery_capacity_j;
        assert!(
            (0.5 * rated..=1.5 * rated).contains(&capacity),
            "usable capacity must stay within a physical band of rated \
             ({rated} kWh), got {capacity}"
        );
        assert!(
            // The physical day-over-day bound: the reversible temperature
            // derate spans ≈8% across the residential cold band (0.86 at
            // 0 °C to ~1.0 at 25 °C) plus the break-in loss's first-day
            // step (≤2.8%) — anything beyond ~11% is the whipsaw class.
            (capacity - prev_capacity).abs() <= 0.11 * rated,
            "day-over-day capacity change must stay bounded (<= 11% of \
             rated: the derate span plus the break-in transient), changed \
             by {}",
            capacity - prev_capacity
        );
        prev_capacity = capacity;
    }
}

/// Acceptance: over a year of daily driving, charging and idle at
/// several pack temperatures, the EV battery's capacity never exceeds its
/// rating: the degradation fade never goes negative, the degradation-
/// adjusted rating (`rated · SOH`, the charging-LUT divisor and the seed's
/// counterpart of the Battery's `capacity_kwh_nominal`) never rises above
/// `rated`, and the live usable capacity never exceeds the reversibly
/// derated rating (`rated · derate`). A capacity above the rating is the
/// reversible thermal layer's alone, never the degradation model's.
///
/// Source: NREL SSC's capacity layer applies the lifetime fraction to
/// `qmax_lifetime` only when it decreases from `qmax_init`
/// (`capacity_lithium_ion_t::updateCapacityForLifetime`,
/// `shared/lib_battery_capacity.cpp`), so the reference battery's capacity
/// starts at nameplate and can only fall. Pre-fix, HARES's first midnight
/// degradation update surfaced the model's raw BOL level (min(QLi, Qneg) =
/// 1.009) as a negative fade and the 60 kWh pack's capacity rose to
/// 60.54 kWh.
#[test]
fn ev_capacity_never_exceeds_its_rating() {
    for temp_c in [-15.0, 0.0, 25.0, 45.0] {
        let mut raw = base_raw();
        raw.insert(KEY_INITIAL_SOC.to_string(), 0.9.into());
        raw.insert(KEY_BATTERY_TEMP_C.to_string(), temp_c.into());
        // Hold the pack at its temperature (no thermal coupling) and keep
        // charging physical at the cold end: the subject is the capacity
        // invariant, not the cold-charge derating.
        raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
        raw.insert(KEY_HEATER_POWER_W.to_string(), 0.0.into());
        raw.insert(KEY_MIN_CHARGE_TEMP_C.to_string(), (-25.0).into());
        raw.insert(KEY_FULL_POWER_TEMP_C.to_string(), (-15.0).into());
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());
        let mut env = sample_env();
        ev.init(&config, &env).unwrap();

        let rated = ev.battery_capacity_j_rated;
        let step = |ev: &mut Ev, env: &mut EnvironmentState| {
            let mut ports = PortSlots::default();
            ev.step(env, Duration::from_secs(3600), &mut ports).unwrap();
            env.current_time += ChronoDuration::seconds(3600);

            let fade = ev.degradation.capacity_fade_fraction();
            assert!(
                fade >= 0.0,
                "{temp_c} °C: the degradation fade must never be negative, \
                 got {fade}"
            );
            assert!(
                rated * (1.0 - fade) <= rated + 1e-9,
                "{temp_c} °C: the degradation-adjusted rating \
                 rated·(1−fade) = {} must never exceed rated {rated}",
                rated * (1.0 - fade)
            );
            // The live capacity's excess over the rating must be the
            // reversible thermal layer's alone, at the pack's live
            // temperature: the degradation (SOH) factor contributes nothing
            // above 1.
            let live_derate = capacity_derate_at(ev.battery_temp_c);
            let soh = 1.0 - fade;
            assert!(
                ev.battery_capacity_j <= rated * soh * live_derate + 1e-6,
                "{temp_c} °C: usable capacity {} must never exceed the \
                 reversibly derated rating {} at the live pack temperature \
                 {} °C (the excess must be the thermal layer's alone)",
                ev.battery_capacity_j,
                rated * live_derate,
                ev.battery_temp_c
            );
        };

        for _day in 0..365 {
            // Overnight at home: plug in and charge toward the target (the
            // strategy idles the charge once there).
            ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
                state: EvConnectionState::HomePluggedIn,
            })
            .unwrap();
            for _ in 0..10 {
                step(&mut ev, &mut env);
            }
            // Idle the rest of the evening.
            for _ in 0..2 {
                step(&mut ev, &mut env);
            }
            // Daily drive: 15 kWh delivered while disconnected.
            ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
                state: EvConnectionState::Disconnected,
            })
            .unwrap();
            ev.apply_control_unchecked(&ControlSignal::EvDrive {
                energy_j: 15.0 * 3.6e6,
            })
            .unwrap();
            step(&mut ev, &mut env);
            // Idle daytime hours to complete the 24.
            for _ in 0..11 {
                step(&mut ev, &mut env);
            }
        }
    }
}

#[test]
fn charge_derate_applied_before_taper_limit() {
    // Heater pinned off: the subject is the derate-before-taper ordering,
    // and the heater's AC-equivalent on the port would confound the power
    // comparison.
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
    raw.insert(KEY_BATTERY_TEMP_C.to_string(), 5.0.into());
    raw.insert(KEY_MIN_CHARGE_TEMP_C.to_string(), 0.0.into());
    raw.insert(KEY_FULL_POWER_TEMP_C.to_string(), 10.0.into());
    raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
    raw.insert(KEY_HEATER_POWER_W.to_string(), 0.0.into());
    let config_cold = ev_config(raw);

    let mut raw_warm = base_raw();
    raw_warm.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
    raw_warm.insert(KEY_BATTERY_TEMP_C.to_string(), 20.0.into());
    raw_warm.insert(KEY_MIN_CHARGE_TEMP_C.to_string(), 0.0.into());
    raw_warm.insert(KEY_FULL_POWER_TEMP_C.to_string(), 10.0.into());
    raw_warm.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
    raw_warm.insert(KEY_HEATER_POWER_W.to_string(), 0.0.into());
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
    raw.insert(KEY_V2L_MAX_DISCHARGE_W.to_string(), 3000.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_w: -2000.0,
        reactive_power_var: None,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

    let power = ev.telemetry().get("active_power_kw").unwrap();
    assert!(
        power < 0.0,
        "V2L should produce negative active_power_w, got {power}"
    );
    assert_eq!(ev.telemetry().get("v2l_active"), Some(1.0));
}

#[test]
fn v2l_respects_soc_reserve_floor() {
    let mut raw = base_raw();
    raw.insert(KEY_V2L_ENABLED.to_string(), true.into());
    raw.insert(KEY_V2L_SOC_RESERVE.to_string(), 0.5.into());
    raw.insert(KEY_V2L_MAX_DISCHARGE_W.to_string(), 5000.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_w: -3000.0,
        reactive_power_var: None,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

    let power = ev.telemetry().get("active_power_kw").unwrap();
    assert_eq!(power, 0.0, "V2L must not discharge when SOC <= reserve");
}

/// Discharging toward the reserve must land ON the floor, not below it:
/// the step cap is the AC power the available DC energy can sustain, so
/// the pack draw (AC / efficiency) over the step consumes exactly the
/// energy above the floor. An AC-domain cap (available/dt without the
/// efficiency factor) draws (1/efficiency)x that from the pack and
/// undershoots the floor on every landing.
#[test]
fn v2l_discharge_lands_on_soc_reserve_floor() {
    let mut raw = base_raw();
    raw.insert(KEY_V2L_ENABLED.to_string(), true.into());
    raw.insert(KEY_V2L_SOC_RESERVE.to_string(), 0.1.into());
    raw.insert(KEY_V2L_MAX_DISCHARGE_W.to_string(), 5000.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.12.into());
    raw.insert(KEY_EFFICIENCY.to_string(), 0.9.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    // 1.2 kWh above the floor; a 5 kW request over 1 h far exceeds what
    // that energy sustains (~1.08 kW AC), so the energy cap binds.
    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_w: -5000.0,
        reactive_power_var: None,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();

    assert!(
        ev.soc < 0.12,
        "the step must discharge toward the floor, got soc {}",
        ev.soc
    );
    assert!(
        ev.soc >= 0.1 - 1e-9,
        "V2L discharge must not undershoot the 0.1 reserve floor, got soc {}",
        ev.soc
    );
    assert!(
        ev.soc <= 0.1 + 1e-6,
        "the energy cap should land essentially on the floor, got soc {}",
        ev.soc
    );
}

/// The same floor-landing contract for V2G export — the cap computation is
/// a separate copy in `compute_v2g_discharge`, so it needs its own pin.
#[test]
fn v2g_discharge_lands_on_soc_reserve_floor() {
    let mut raw = base_raw();
    raw.insert(KEY_V2G_ENABLED.to_string(), true.into());
    raw.insert(KEY_V2G_SOC_RESERVE.to_string(), 0.2.into());
    raw.insert(KEY_V2G_MAX_DISCHARGE_W.to_string(), 5000.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.22.into());
    raw.insert(KEY_EFFICIENCY.to_string(), 0.9.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).expect("init");

    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_w: -5000.0,
        reactive_power_var: None,
        min_soc: None,
        max_soc: None,
    })
    .expect("setpoint");

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports)
        .expect("step");

    assert!(
        ev.soc < 0.22,
        "the step must discharge toward the floor, got soc {}",
        ev.soc
    );
    assert!(
        ev.soc >= 0.2 - 1e-9,
        "V2G discharge must not undershoot the 0.2 reserve floor, got soc {}",
        ev.soc
    );
    assert!(
        ev.soc <= 0.2 + 1e-6,
        "the energy cap should land essentially on the floor, got soc {}",
        ev.soc
    );
}

#[test]
fn v2l_respects_max_discharge_limit() {
    let mut raw = base_raw();
    raw.insert(KEY_V2L_ENABLED.to_string(), true.into());
    raw.insert(KEY_V2L_SOC_RESERVE.to_string(), 0.1.into());
    raw.insert(KEY_V2L_MAX_DISCHARGE_W.to_string(), 2000.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.8.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_w: -10.0,
        reactive_power_var: None,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();

    let power = ev.telemetry().get("active_power_kw").unwrap();
    assert!(
        power >= -2000.0 - 1e-6,
        "V2L discharge must respect the 2 kW max_discharge_w limit, got {power}"
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
            active_power_w: -1000.0,
            reactive_power_var: None,
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
    raw.insert(KEY_V2L_MAX_DISCHARGE_W.to_string(), 3000.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.8.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_w: -2000.0,
        reactive_power_var: None,
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
        "V2L must not exceed max_discharge_w={}, got {}",
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
    raw.insert(KEY_V2G_MAX_DISCHARGE_W.to_string(), 5000.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).expect("init");

    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_w: -3000.0,
        reactive_power_var: None,
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
    raw.insert(KEY_V2G_MAX_DISCHARGE_W.to_string(), 5000.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.8.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).expect("init");

    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_w: -3000.0,
        reactive_power_var: None,
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
        "V2G should produce negative active_power_w, got {power}"
    );
}

#[test]
fn v2g_respects_soc_reserve() {
    let mut raw = base_raw();
    raw.insert(KEY_V2G_ENABLED.to_string(), true.into());
    raw.insert(KEY_V2G_SOC_RESERVE.to_string(), 0.5.into());
    raw.insert(KEY_V2G_MAX_DISCHARGE_W.to_string(), 5000.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).expect("init");

    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_w: -5000.0,
        reactive_power_var: None,
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
    raw.insert(KEY_V2G_MAX_DISCHARGE_W.to_string(), 5000.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.30.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).expect("init");

    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_w: -5000.0,
        reactive_power_var: None,
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
    raw.insert(KEY_V2G_MAX_DISCHARGE_W.to_string(), 5000.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.50.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).expect("init");

    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_w: -5000.0,
        reactive_power_var: None,
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
    raw.insert(KEY_V2L_MAX_DISCHARGE_W.to_string(), 5000.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.25.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).expect("init");

    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_w: -3000.0,
        reactive_power_var: None,
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
        active_power_w: 7.2000,
        reactive_power_var: None,
        min_soc: None,
        max_soc: Some(0.90),
    })
    .expect("setpoint");

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports)
        .expect("step");
    let p_full = ev.telemetry().get("active_power_kw").unwrap();
    assert!(
        p_full < 7200.0,
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
    let power_w = ev.telemetry().get("active_power_kw").unwrap();
    assert!(
        power_w <= 3600.0 + 1e-6,
        "power should be limited at soc=0.5, got {power_w}"
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
        power <= 2000.0,
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
    ev.apply_control_unchecked(&ControlSignal::EvDrive {
        energy_j: 10.0 * 3.6e6,
    })
    .unwrap();

    // The drive debits against the usable capacity — rated scaled by the
    // reversible temperature derate at the pack's temperature (the
    // unpinned config initializes at the 10 °C ambient).
    let expected_drop = 10.0 / (60.0 * capacity_derate_at(10.0));
    assert!(
        (ev.soc - (soc_before - expected_drop)).abs() < 1e-9,
        "SOC should drop by ~{expected_drop}, got {}",
        soc_before - ev.soc
    );
}

/// Drive energy heats the pack at its equivalent discharge current
/// through the same shared I²R solve — the drive-side instance of the
/// loss-attribution rule. The `EvDrive` kWh is consumed by the next step
/// as an equivalent power through the pack electrical model. UA pinned to
/// 0 with the pack at ambient so the entire temperature rise is the
/// drive's I²R: a regression that drops the pending-drive consumption (or
/// zeroes its heat) leaves the pack exactly at ambient and fails here.
/// Material for the cold-charge contract: at the post-alignment day-scale
/// time constant, most of a commute's heat is still in the pack at the
/// charge-window start — single-digit kelvin on the 0–10 °C derate ramp.
#[test]
fn drive_energy_heats_the_pack_at_its_equivalent_discharge_current() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.9.into());
    raw.insert(KEY_BATTERY_TEMP_C.to_string(), 10.0.into());
    raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env(); // ambient 10 °C — equal to the pinned pack temperature
    ev.init(&config, &env).unwrap();
    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::Disconnected,
    })
    .unwrap();
    let temp_before = ev.battery_temp_c;
    // 15 kWh in joules.
    ev.apply_control_unchecked(&ControlSignal::EvDrive {
        energy_j: 15.0 * 3.6e6,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

    // Oracle: the same shared solve, on the drive's equivalent discharge
    // power at the post-debit SOC's OCV. The SOC debit happened at signal
    // time and the disconnected step nets zero, so production's solve saw
    // exactly this (SOC, power) pair.
    // The drive's equivalent discharge power: 15 kWh over the 15-min
    // step = 60000 W.
    let drive_dc_w = 15.0 * 3.6e6 / 900.0;
    let expected_ohmic = ev
        .pack_electrical()
        .solve(ev.ocv_table.voltage_at_soc(ev.soc), -drive_dc_w)
        .ohmic_loss_w;
    let expected_rise = expected_ohmic * 900.0 / ev.thermal_mass_j_per_k;
    let rise = ev.battery_temp_c - temp_before;
    assert!(
        (rise - expected_rise).abs() < 1e-9,
        "the drive's temperature rise must equal its I2R·dt/C = {expected_rise} K \
         (I2R {expected_ohmic} W over 900 s into {} J/K), got {rise} K",
        ev.thermal_mass_j_per_k
    );
    assert!(
        rise > 0.1,
        "a 15 kWh drive must warm the pack measurably through its equivalent \
         discharge current (expected ~{expected_rise} K), got {rise} K — the pending-drive \
         I2R mechanism is not reaching the thermal equation"
    );
}

#[test]
fn ev_drive_while_home_plugged_in_rejected() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    let err = ev
        .apply_control_unchecked(&ControlSignal::EvDrive {
            energy_j: 5.0 * 3.6e6,
        })
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
        .apply_control_unchecked(&ControlSignal::EvDrive {
            energy_j: 5.0 * 3.6e6,
        })
        .unwrap_err();
    assert!(err.to_string().contains("Disconnected"));
}

/// Hypothesis: `EvDrive` with negative kWh bypasses the handler's overdraw
/// guard (`kwh > available_j` is false for any negative value), so the SOC
/// arithmetic `soc - kwh / battery_capacity_j` runs with a negated term and
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
    let _ = ev.apply_control_unchecked(&ControlSignal::EvDrive { energy_j: -5.0 });

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

/// Hypothesis: `EvDrive` with NaN kWh slips past the `kwh > available_j`
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
    let _ = ev.apply_control_unchecked(&ControlSignal::EvDrive { energy_j: f64::NAN });

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

/// Hypothesis: the EvDrive overdraw guard `kwh > available_j` is one-sided.
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
    let result = ev.apply_control_unchecked(&ControlSignal::EvDrive {
        energy_j: f64::INFINITY,
    });
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
        energy_j: f64::NEG_INFINITY,
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
    ev.apply_control_unchecked(&ControlSignal::EvAwayCharge { power_w: 7200.0 })
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
        .apply_control_unchecked(&ControlSignal::EvAwayCharge { power_w: 7200.0 })
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
        .apply_control_unchecked(&ControlSignal::EvAwayCharge { power_w: 7200.0 })
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
        active_power_w: 0.0,
        reactive_power_var: None,
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

    // Step at 18:00 -- deadline reads as tomorrow 7:00 (13 h out): the
    // deficit paces a proportional trickle, not the bang-bang zero.
    env.current_time = dt(2026, 1, 1, 18, 0, 0);
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    let power_early = ev.telemetry().get("active_power_kw").unwrap();
    {
        let deficit_j = (0.8 - 0.3) * ev.battery_capacity_j;
        let eta = ev.charging_efficiency.max(0.01);
        let expected_w = (deficit_j / (13.0 * 3600.0) / eta).min(7200.0);
        assert!(
            (power_early - expected_w).abs() < expected_w * 1e-3,
            "a 13-hour deadline with a 0.5-pack deficit must pace a trickle \
             (~{expected_w} W), got {power_early} W"
        );
    }

    // Step at 5:00 -- should be charging (close to deadline)
    ev.soc = 0.3;
    env.current_time = dt(2026, 1, 2, 5, 0, 0);
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    let power_late = ev.telemetry().get("active_power_kw").unwrap();

    assert!(
        power_late > 0.0,
        "BMS should be charging close to deadline: the 2-hour window needs \
         more than rated, so the pace is the rated power"
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
    // Disconnect ends the charging session: actor-writable setpoints and
    // targets must not survive the plug cycle.
    assert!(ev.power_setpoint_w.is_none());
    assert!(ev.soc_target.is_none());
}

/// A fresh `SOCTarget` must supersede a latched zero-power hold: without
/// the clear in the `SOCTarget` arm, `compute_charging_power_w` keeps
/// consulting the stale `power_setpoint_w` first and a strategy going
/// idle → active would trade "always charges" for "never charges".
#[test]
fn soc_target_clears_latched_zero_power_hold() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.2.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    // An idle window latches the hold …
    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_w: 0.0,
        reactive_power_var: None,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();
    assert_eq!(ev.soc, 0.2, "hold must suppress charging");

    // … then the strategy's window opens and it dispatches a target.
    ev.apply_control(&ControlSignal::SOCTarget {
        target_soc: 0.9,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();
    assert!(
        ev.soc > 0.2,
        "a fresh SOCTarget must clear the prior hold and charge, got soc {}",
        ev.soc
    );
}

/// The same hold-clearing rule for `EvSetReadyBy`: a ready-by target means
/// "charge toward this by departure", which a latched hold would veto.
#[test]
fn ready_by_clears_latched_zero_power_hold() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.3.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let mut env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_w: 0.0,
        reactive_power_var: None,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();
    // Tight deadline (2:00) at 22:00 — the BMS must charge immediately.
    env.current_time = dt(2026, 1, 1, 22, 0, 0);
    ev.apply_control_unchecked(&ControlSignal::EvSetReadyBy {
        departure_hour: 2.0,
        target_soc: 0.9,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    assert!(
        ev.soc > 0.3,
        "a fresh EvSetReadyBy must clear the prior hold and charge, got soc {}",
        ev.soc
    );
}

/// The deadline-guarantee exception to the hold: with a ready-by deadline
/// latched and urgent, a zero-power hold dispatched *after* the ready-by
/// (the composer's target-then-rate order means a later idle step's hold
/// lands on top of an earlier `EvSetReadyBy`) must not veto charging —
/// `compute_charging_power_w` raises the request to the BMS's
/// deadline-required power (`requested = bms_power.max(requested)`), so a
/// soft hold never strands the driver. This is the one scenario where an
/// idling strategy legitimately still charges.
#[test]
fn urgent_ready_by_deadline_charges_through_latched_hold() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.3.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let mut env = sample_env();
    ev.init(&config, &env).unwrap();

    // Ready-by first: 0.3 -> 0.9 on a 7.2 kW charger needs 5 h against a
    // 4 h deadline (22:00 -> 02:00), so the BMS must charge immediately.
    env.current_time = dt(2026, 1, 1, 22, 0, 0);
    ev.apply_control_unchecked(&ControlSignal::EvSetReadyBy {
        departure_hour: 2.0,
        target_soc: 0.9,
    })
    .unwrap();
    // … then a later idle step latches the hold on top of it.
    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_w: 0.0,
        reactive_power_var: None,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    assert!(
        ev.soc > 0.3,
        "an urgent ready-by deadline must charge through a latched zero-power hold, got soc {}",
        ev.soc
    );
}

/// A hold latched at checkpoint time must survive an equipment checkpoint
/// round-trip: the v4 checkpoint carries `power_setpoint_w`/`soc_target`,
/// and a field dropped on save or restore would silently resurrect the
/// charge-to-full BMS default after every restart — the exact symptom the
/// explicit-hold contract exists to prevent, resurfacing only in
/// checkpoint-restart runs where no actor re-dispatch is between restore
/// and the next step.
#[test]
fn latched_hold_and_target_survive_equipment_checkpoint_round_trip() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.4.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    // An idle window latches the hold; an earlier strategy step latched a
    // target below the BMS default.
    ev.apply_control(&ControlSignal::SOCTarget {
        target_soc: 0.7,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();
    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_w: 0.0,
        reactive_power_var: None,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    let state = ev.save_state().unwrap();
    let mut restored = Ev::new(config.clone());
    restored.init(&config, &env).unwrap();
    restored.load_state(&state).unwrap();
    // No field dropped on re-save: a silent schema regression shows up as a
    // byte difference, not a default silently standing in.
    assert_eq!(
        state,
        restored.save_state().unwrap(),
        "checkpoint round-trip must preserve the latched hold/target bytes"
    );

    // The restored EV must still be held: charging stays suppressed.
    let soc_before = restored.soc;
    let mut ports = PortSlots::default();
    restored
        .step(&env, Duration::minutes(60), &mut ports)
        .unwrap();
    assert_eq!(
        restored.soc, soc_before,
        "a hold latched before the checkpoint must still suppress charging after restore, \
         got soc {soc_before} → {}",
        restored.soc
    );

    // …and the restored state is live: a fresh target supersedes the hold.
    restored
        .apply_control(&ControlSignal::SOCTarget {
            target_soc: 0.9,
            min_soc: None,
            max_soc: None,
        })
        .unwrap();
    let mut ports = PortSlots::default();
    restored
        .step(&env, Duration::minutes(60), &mut ports)
        .unwrap();
    assert!(
        restored.soc > soc_before,
        "a fresh target on the restored EV must charge (the hold-clear must have been \
         restored too), got soc stuck at {}",
        restored.soc
    );
}

/// The range-anxiety override dispatches `SOCTarget{band}` — no rate — so
/// the `SOCTarget` arm's hold-clear leaves the BMS in full control
/// (`requested = bms_power` on the no-setpoint branch). The BMS's deadline
/// pacing computes its deficit against `soc_target.or(ready_by_soc)` — the
/// override's band target shadows the ready-by's own — so a maximally
/// urgent ready-by deadline (42 kWh needed, 1 h left) recomputes its
/// deficit against the band (6 kWh ≈ 0.93 h < 1 h "plenty of time") and
/// commands zero: the urgent deadline charges nothing, and the override's
/// minimal top-up never flows either. Contrast
/// `urgent_ready_by_deadline_charges_through_latched_hold`, where the
/// latched HOLD leaves `soc_target` unset and the deadline charges — the
/// override's target, not the hold, is what silences it.
#[test]
fn band_soc_target_must_not_veto_urgent_ready_by_charging() {
    // Phase A (precondition): the deadline alone charges — 0.2 → 0.9 needs
    // 42 kWh against a 1 h deadline, so the BMS is maximally urgent.
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.2.into());
    let config = ev_config(raw.clone());
    let mut ev = Ev::new(config.clone());
    let mut env = sample_env();
    ev.init(&config, &env).unwrap();

    env.current_time = dt(2026, 1, 1, 22, 0, 0);
    ev.apply_control_unchecked(&ControlSignal::EvSetReadyBy {
        departure_hour: 23.0,
        target_soc: 0.9,
    })
    .unwrap();
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    assert!(
        ev.soc > 0.2,
        "precondition: an urgent ready-by deadline (0.9 by 23:00, 42 kWh needed, 1 h \
         left) must charge, got soc {}",
        ev.soc
    );

    // Phase B (the attack): the same session, with the range-anxiety
    // override's minimal-charge dispatch (SOCTarget at the band) landing on
    // top of the latched deadline. The override exists to protect a short
    // driver and fires precisely when time is short; the deadline is
    // unmet and urgent. Charging must continue — instead the band target
    // shadows the deadline's deficit basis and the BMS paces to zero.
    let mut ev = Ev::new(config.clone());
    let mut env = sample_env();
    ev.init(&config, &env).unwrap();
    env.current_time = dt(2026, 1, 1, 22, 0, 0);
    ev.apply_control_unchecked(&ControlSignal::EvSetReadyBy {
        departure_hour: 23.0,
        target_soc: 0.9,
    })
    .unwrap();
    ev.apply_control(&ControlSignal::SOCTarget {
        target_soc: 0.3, // the band: (30 mi + 20 mi) × 0.3 kWh/mi / 60 kWh
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    assert!(
        ev.soc > 0.2,
        "an urgent ready-by deadline must keep charging when the range-anxiety override's \
         band target lands on top of it — the override tops up minimally toward the band \
         and the deadline still needs 42 kWh in 1 h — instead the band target shadowed \
         the deadline's deficit basis and charging stopped: soc {}",
        ev.soc
    );
}

/// The same shadowing defect has a second branch: with SOC already above
/// the band target but below the deadline's departure target, the
/// `soc >= soc_limit` early return fired at the band and returned zero
/// before the deadline branch ever ran — the deadline was vetoed by a
/// ceiling it was supposed to outrank. The effective destination while a
/// ready-by is latched is the max of the controller target and the
/// deadline's own, so the early return cannot stop charging short of the
/// departure SOC while it is still unmet.
#[test]
fn band_soc_target_ceiling_must_not_stop_charging_short_of_urgent_deadline() {
    // SOC 0.4 is above the band (0.3) but well short of the deadline's 0.9
    // departure target — 30 kWh needed against a 1 h deadline, urgent.
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.4.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let mut env = sample_env();
    ev.init(&config, &env).unwrap();

    env.current_time = dt(2026, 1, 1, 22, 0, 0);
    ev.apply_control_unchecked(&ControlSignal::EvSetReadyBy {
        departure_hour: 23.0,
        target_soc: 0.9,
    })
    .unwrap();
    ev.apply_control(&ControlSignal::SOCTarget {
        target_soc: 0.3, // the band: (30 mi + 20 mi) × 0.3 kWh/mi / 60 kWh
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    assert!(
        ev.soc > 0.4,
        "an urgent ready-by deadline must keep charging above the band target but short of \
         its own 0.9 departure target — instead the band ceiling stopped charging at the \
         early return: soc {}",
        ev.soc
    );
}

/// The mirror direction of the band-veto fix: the effective destination
/// while a ready-by is latched is the max of the two targets, so a
/// controller target ABOVE the deadline's raises the destination — the
/// deadline's lower departure target must not cap a higher configured
/// ceiling. SOC 0.7 is already above the deadline's 0.6 target; a
/// "deadline target always wins" rule would early-return zero here.
#[test]
fn controller_target_above_ready_by_raises_destination() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.7.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let mut env = sample_env();
    ev.init(&config, &env).unwrap();

    env.current_time = dt(2026, 1, 1, 22, 0, 0);
    ev.apply_control_unchecked(&ControlSignal::EvSetReadyBy {
        departure_hour: 23.0,
        target_soc: 0.6, // deadline already satisfied at SOC 0.7
    })
    .unwrap();
    ev.apply_control(&ControlSignal::SOCTarget {
        target_soc: 0.95, // controller ceiling above the deadline's target
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    assert!(
        ev.soc > 0.7,
        "a controller target (0.95) above the deadline's (0.6) must raise the destination — \
         charging must continue past the satisfied deadline target: soc {}",
        ev.soc
    );
}

/// The ready-by pace is proportional: the average power that lands the
/// deficit at the deadline, clamped to the rated power. The pre-fix model
/// was bang-bang - full rated power only when the deficit could not land
/// in time, zero otherwise - so a pack that could coast to its deadline
/// charged at nothing for hours and then slammed to full rate at the
/// buffer boundary.
#[test]
fn ready_by_power_paces_proportionally_to_the_deadline() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    // Deadline 8 hours out; the deficit is sized to need 2 hours at rated
    // power, so the paced power is rated * 2/8.
    let now = dt(2026, 1, 1, 10, 0, 0);
    ev.ready_by_hour = Some(18.0);
    ev.soc = 0.5;
    let soc_limit = 0.7;
    let deficit_j = (soc_limit - ev.soc) * ev.battery_capacity_j;
    let eta = ev.charging_efficiency.max(0.01);
    let expected = (deficit_j / (8.0 * 3600.0) / eta).min(7200.0);

    let paced = ev.bms_ready_by_power(now, 7200.0, soc_limit);
    assert!(
        (paced - expected).abs() < 1e-6,
        "an 8-hour deadline with a 2-hour deficit must pace at ~{expected} W, got {paced} W"
    );
}

/// A deadline in the past reads as tomorrow's (the ready-by departure
/// recurs daily), and the pre-fix ±1 h past-deadline grace window is
/// gone: a deadline half an hour past paces toward its next occurrence -
/// a trickle, not the grace's full rate.
#[test]
fn ready_by_power_paces_a_past_deadline_at_its_next_occurrence() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    // Half an hour past the deadline (inside the old grace window),
    // deficit open: the pace reads the deadline as tomorrow's (23.5 h
    // out) - no grace rate.
    let now = dt(2026, 1, 1, 10, 0, 0);
    ev.ready_by_hour = Some(9.5);
    ev.soc = 0.5;
    let soc_limit = 0.7;
    let deficit_j = (soc_limit - ev.soc) * ev.battery_capacity_j;
    let eta = ev.charging_efficiency.max(0.01);
    let expected = (deficit_j / (23.5 * 3600.0) / eta).min(7200.0);

    let paced = ev.bms_ready_by_power(now, 7200.0, soc_limit);
    assert!(
        (paced - expected).abs() < 1e-6,
        "a past deadline must pace at its next occurrence (~{expected} W), got {paced} W"
    );
}

/// The witness of the lingering deadline: a latched ready-by whose
/// departure has passed keeps raising the charging destination above the
/// controller's target - the pack charges toward a departure that will
/// not happen. The driver's cancellation path is what must clear it; this
/// half holds the pre-clear state the clear releases.
#[test]
fn a_stale_deadline_keeps_the_pack_charging_past_its_departure() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.6.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let mut env = sample_env();
    ev.init(&config, &env).unwrap();

    env.current_time = dt(2026, 1, 1, 7, 30, 0);
    ev.apply_control_unchecked(&ControlSignal::EvSetReadyBy {
        departure_hour: 7.0, // already past: a trip that did not happen
        target_soc: 0.9,
    })
    .unwrap();
    ev.apply_control(&ControlSignal::SOCTarget {
        target_soc: 0.5, // the controller's own target is BELOW the SOC
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    let power = ev.telemetry().get("active_power_kw").unwrap();
    assert!(
        power > 0.0,
        "the stale deadline (target 0.9 over SOC 0.6) keeps the pack charging \
         for a departure that passed: power {power} kW"
    );
}

/// The clear releases the pack: the deadline gone, the controller's own
/// target governs the destination and the discharge floor loses the
/// deadline's contribution.
#[test]
fn ev_clear_ready_by_signal_clears_the_deadline() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.6.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let mut env = sample_env();
    ev.init(&config, &env).unwrap();

    env.current_time = dt(2026, 1, 1, 7, 30, 0);
    ev.apply_control_unchecked(&ControlSignal::EvSetReadyBy {
        departure_hour: 7.0,
        target_soc: 0.9,
    })
    .unwrap();
    ev.apply_control(&ControlSignal::SOCTarget {
        target_soc: 0.5,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();
    assert!(ev.ready_by_hour.is_some());

    ev.apply_control_unchecked(&ControlSignal::EvClearReadyBy)
        .unwrap();
    assert!(ev.ready_by_hour.is_none());
    assert!(ev.ready_by_soc.is_none());

    let mut ports = PortSlots::default();
    env.current_time = dt(2026, 1, 1, 7, 45, 0);
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    let power = ev.telemetry().get("active_power_kw").unwrap();
    assert!(
        power == 0.0,
        "with the deadline cleared the controller's target (0.5 below SOC) \
         must govern: no charging for a cancelled departure, got {power} kW"
    );
}

#[test]
fn disconnect_clears_latched_hold_for_away_charging() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.3.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    // Home session leaves a hold and a target latched …
    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_w: 0.0,
        reactive_power_var: None,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();
    ev.apply_control(&ControlSignal::SOCTarget {
        target_soc: 0.9,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    // … then the driver departs and later plugs into the away charger.
    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::Disconnected,
    })
    .unwrap();
    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::AwayPluggedIn,
    })
    .unwrap();
    ev.apply_control_unchecked(&ControlSignal::EvAwayCharge { power_w: 7200.0 })
        .unwrap();

    let soc_before = ev.soc;
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();
    assert!(
        ev.soc > soc_before,
        "disconnect must clear the home-session hold so away charging works, got soc {} → {}",
        soc_before,
        ev.soc
    );
}

#[test]
fn bms_paces_small_deficit_low_before_a_far_deadline() {
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
    // Proportional pacing: a small deficit (0.05 of the pack) with 12 h
    // until the deadline paces a trickle, nowhere near the rated power.
    // (The channel's held values are SI watts, the house SI convention
    // for the legacy-named power keys.)
    let deficit_j = (0.8 - 0.75) * ev.battery_capacity_j;
    let eta = ev.charging_efficiency.max(0.01);
    let expected_w = (deficit_j / (12.0 * 3600.0) / eta).min(7200.0);
    assert!(
        (power - expected_w).abs() < expected_w * 1e-6,
        "a small deficit with a far deadline must pace low (~{expected_w} W), got {power}"
    );
    assert!(
        power < 1000.0,
        "the paced power must stay well under the rated power, got {power}"
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
    // Deadline at the current hour (15:00 against a 15.0 ready-by): a past
    // or present deadline reads as tomorrow's (the ready-by departure
    // recurs daily), so the wrap puts the next occurrence 24 h out and the
    // proportional pace charges the open deficit over the day. This test
    // verifies that a "past" deadline still results in charging.
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
        max_power_w: 2000.0,
        ramp_rate_w_per_s: None,
    })
    .unwrap();

    env.current_time = dt(2026, 1, 2, 5, 0, 0);
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    let power = ev.telemetry().get("active_power_kw").unwrap();
    assert!(
        power <= 2000.0 + 1e-6,
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
        active_power_w: 1000.0,
        reactive_power_var: None,
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
        active_power_w: 1000.0,
        reactive_power_var: None,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    env.current_time = dt(2026, 1, 1, 0, 0, 0);
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    let power = ev.telemetry().get("active_power_kw").unwrap();

    assert!(
        (power - 1000.0).abs() < 1e-6,
        "ExternalAuthority should respect the 1.0 kW setpoint even with an urgent deadline, got {power}"
    );
}

/// The controller-vs-deadline destination fix (`target = max(controller,
/// deadline)` in `compute_charging_power_w`) is scoped by its own comment
/// to the default `DeadlineGuarantee` — "under the default … the deadline
/// is the hard one". `ExternalAuthority` is the opposite contract: the
/// external controller bears sole responsibility and the BMS deadline
/// logic is not applied. The max() applies unconditionally, so a deadline
/// target *above* the controller's commanded destination raises the charge
/// destination under ExternalAuthority too — the deadline logic the
/// priority promises not to apply now shapes the destination. A controller
/// commanding "toward 0.3 at 1 kW" with a stale 0.9 ready-by latched must
/// stop at 0.3; instead the pack rides to 0.9.
#[test]
fn external_authority_deadline_target_must_not_raise_controller_destination() {
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

    // The scheduling layer latched a ready-by deadline earlier in the
    // session; the controller then commands its own, lower destination and
    // rate (SOCTarget first — clearing any hold — then the rate).
    env.current_time = dt(2026, 1, 1, 22, 0, 0);
    ev.apply_control_unchecked(&ControlSignal::EvSetReadyBy {
        departure_hour: 23.0,
        target_soc: 0.9,
    })
    .unwrap();
    ev.apply_control(&ControlSignal::SOCTarget {
        target_soc: 0.3,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();
    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_w: 1000.0,
        reactive_power_var: None,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    // Step ~23 h at 15 min. Under ExternalAuthority the pack must stop at
    // the controller's 0.3 destination; the deadline's 0.9 must not raise
    // it (deadline logic not applied), and nothing else may extend it.
    let mut ports = PortSlots::default();
    for step in 0..92 {
        env.current_time = dt(2026, 1, 1, 22, 0, 0) + ChronoDuration::minutes(15 * step);
        ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    }
    assert!(
        ev.soc <= 0.31,
        "under ExternalAuthority the controller's commanded destination (SOCTarget 0.3) \
         must govern — the latched ready-by deadline's 0.9 target must not raise the \
         charge destination when the priority promises the deadline logic is not \
         applied: soc {}",
        ev.soc
    );
}

/// Companion to `external_authority_deadline_target_must_not_raise_controller_destination`:
/// that test commands a rate alongside the target, so the verbatim-setpoint
/// pacing path is what stops at the controller's destination. This one
/// commands the target WITHOUT a rate — under `ExternalAuthority` with no
/// setpoint the BMS still paces (`requested = bms_power`), now against the
/// priority-scoped destination — and the stale higher deadline target must
/// not raise that destination on this path either. Sensitive to the scoped
/// `max()` in `compute_charging_power_w` exactly as the sibling test is.
#[test]
fn external_authority_target_without_rate_paces_to_controller_destination() {
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

    // Stale ready-by deadline latched earlier; the controller commands a
    // destination only (SOCTarget first — clearing any hold — and no rate).
    env.current_time = dt(2026, 1, 1, 22, 0, 0);
    ev.apply_control_unchecked(&ControlSignal::EvSetReadyBy {
        departure_hour: 23.0,
        target_soc: 0.9,
    })
    .unwrap();
    ev.apply_control(&ControlSignal::SOCTarget {
        target_soc: 0.3,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    // Step 2 h at 15 min, staying inside the deadline window. The pack
    // must pace toward the controller's 0.3 and stop there — not ride the
    // deadline's 0.9, and not stall at the start SOC.
    let mut ports = PortSlots::default();
    for step in 0..8 {
        env.current_time = dt(2026, 1, 1, 22, 0, 0) + ChronoDuration::minutes(15 * step);
        ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    }
    assert!(
        ev.soc > 0.25,
        "with no external setpoint the BMS must still pace toward the controller's \
         destination: soc {}",
        ev.soc
    );
    assert!(
        ev.soc <= 0.31,
        "under ExternalAuthority the controller's commanded destination (SOCTarget 0.3) \
         must govern on the BMS-pacing path too — the latched ready-by deadline's 0.9 \
         must not raise it: soc {}",
        ev.soc
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
        active_power_w: 1000.0,
        reactive_power_var: None,
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
        (power - 1000.0).abs() < 1e-6,
        "DeadlineGuarantee should respect the 1.0 kW setpoint when the deadline is not urgent, got {power}"
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
        active_power_w: 1000.0,
        reactive_power_var: None,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();
    ev.apply_control(&ControlSignal::PowerLimit {
        max_power_w: 3000.0,
        ramp_rate_w_per_s: None,
    })
    .unwrap();

    env.current_time = dt(2026, 1, 1, 0, 0, 0);
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    let power = ev.telemetry().get("active_power_kw").unwrap();

    assert!(
        power > 1000.0,
        "deadline guarantee should raise power above the 1.0 kW setpoint, got {power}"
    );
    assert!(
        power <= 3000.0 + 1e-6,
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
        active_power_w: 1000.0,
        reactive_power_var: None,
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
        active_power_w: 1500.0,
        reactive_power_var: None,
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
        (power - 1500.0).abs() < 1e-6,
        "ExternalAuthority should deliver the raw setpoint power (1500 W), got {power}"
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
    raw.insert(KEY_CAPACITY_J.to_string(), 216000000.0.into());
    raw.insert(KEY_MAX_CHARGING_POWER_W.to_string(), 7200.0.into());
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
    ev.apply_control_unchecked(&ControlSignal::EvAwayCharge { power_w: 7200.0 })
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
    ev.apply_control_unchecked(&ControlSignal::EvAwayCharge { power_w: 11500.0 })
        .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

    assert_eq!(ev.telemetry().get("active_power_kw"), Some(0.0));
    // away_charge_power_w is the actual intake after CC-CV taper, not the
    // external charger's rated power. At SOC=0.3 the actual power is limited
    // by the EV's internal charging rate.
    let away_w = ev.telemetry().get("away_charge_power_kw").unwrap();
    assert!(away_w > 0.0, "should be charging away");
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

    // CAPACITY_KWH reports the usable capacity (rated × SOH × the
    // temperature derate at the ambient-resolved pack temperature).
    assert!(
        (ev.telemetry().get("capacity_kwh").unwrap() - 60.0 * 3.6e6 * capacity_derate_at(10.0))
            .abs()
            < 1e-3
    );
    // The telemetry value is the SI fuel economy's own round trip (the
    // config enters J/m; the working value converts back): the pair's
    // last-bit move is the unit move's recorded movement, so the
    // assertion is on the value within its ulp.
    assert!((ev.telemetry().get("fuel_economy_kwh_per_mi").unwrap() - 0.325).abs() < 1e-12,);
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
    ev.apply_control_unchecked(&ControlSignal::EvAwayCharge { power_w: 0.0 })
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
        max_export_w: 7200.0,
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
            max_export_w: 7200.0,
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
            capacity_j,
            max_charge_w,
            ..
        } => {
            assert!(matches!(
                strategy,
                hares_types::ChargingStrategy::Nightly { .. }
            ));
            assert_eq!(plug_in_policy, hares_types::PlugInPolicy::Always);
            // The seed carries the degradation-adjusted rating (rated × SOH,
            // temperature-independent) — never the live usable capacity, so
            // the driver's belief model does not freeze the init-time
            // ambient into the whole run. Fresh pack: SOH 1.0 → exactly the
            // 60 kWh rating, at any init temperature.
            assert!(
                (capacity_j - 60.0 * 3.6e6).abs() < 1e-3,
                "the seed capacity must be the degradation-adjusted rating \
                 (temperature-independent), got {capacity_j} J"
            );
            assert!((max_charge_w - 7200.0).abs() < 1e-6);
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
    raw.insert(KEY_CAPACITY_J.to_string(), 216000000.0.into());
    raw.insert(KEY_MAX_CHARGING_POWER_W.to_string(), 7200.0.into());
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

    // 4 h × 7.2 kW × η 0.9 of DC energy over the usable capacity (rated ×
    // the temperature derate at the unpinned 10 °C ambient, constant via
    // UA = 0), from SOC 0.5.
    let expected_soc = 0.5 + (7.2 * 0.9 * 4.0) / (60.0 * capacity_derate_at(10.0));
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
    raw.insert(KEY_CAPACITY_J.to_string(), 216000000.0.into());
    raw.insert(KEY_MAX_CHARGING_POWER_W.to_string(), 7200.0.into());
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
    // DC energy needed = 0.5 × usable capacity (rated × the temperature
    // derate at the unpinned 10 °C ambient, held constant by UA = 0);
    // time = energy / (7.2 × 0.9).
    let usable_j = 60.0 * capacity_derate_at(10.0);
    let expected_steps = (0.5 * usable_j / (7.2 * 0.9) * 60.0).ceil() as i32;
    assert!(
        (step - expected_steps).abs() <= 2,
        "Expected full at step ~{expected_steps}, got {step}"
    );
}

/// OCHRE EV_FUEL_ECONOMY default = 0.325 kWh/mile.
/// 30-mile trip: 30 × 0.325 = 9.75 kWh = 3.51e7 J.
/// SOC drop = 3.51e7 / (60 × 3.6e6) = 0.1625
///
/// Validates driving uses fuel economy directly, no charging efficiency applied.
#[test]
fn driving_soc_decrease_matches_fuel_economy() {
    let mut raw = base_raw();
    raw.insert(KEY_CAPACITY_J.to_string(), 216000000.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 1.0.into());
    // 0.325 kWh/mi as J/m (the SI raw key, converted once here).
    raw.insert(
        KEY_FUEL_ECONOMY_J_PER_M.to_string(),
        (0.325 * 3.6e6 / 1_609.344).into(),
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
    // 30 mi × 0.325 kWh/mi, in J.
    let drive_j = miles * 0.325 * 3.6e6;
    ev.apply_control_unchecked(&ControlSignal::EvDrive { energy_j: drive_j })
        .unwrap();

    // SOC drop = drive energy / usable capacity (rated × the temperature
    // derate at the unpinned 10 °C ambient initialization).
    let expected_drop = drive_j / (60.0 * 3.6e6 * capacity_derate_at(10.0));
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
    raw.insert(KEY_CAPACITY_J.to_string(), 216000000.0.into());
    raw.insert(KEY_MAX_CHARGING_POWER_W.to_string(), 7200.0.into());
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
    raw2.insert(KEY_CAPACITY_J.to_string(), 216000000.0.into());
    raw2.insert(KEY_INITIAL_SOC.to_string(), 0.05.into());
    let config2 = ev_config(raw2);
    let mut ev2 = Ev::new(config2.clone());
    ev2.init(&config2, &env).unwrap();

    ev2.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::Disconnected,
    })
    .unwrap();

    // Available = 0.05 * 60 = 3.0 kWh; try to drive 3.0 kWh exactly
    ev2.apply_control_unchecked(&ControlSignal::EvDrive {
        energy_j: 3.0 * 3.6e6,
    })
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

    // A finite overdraw is truncated to the pack's remaining energy: SOC
    // lands exactly at 0 (never below), and the undeliverable remainder is
    // accounted as observable drive shortfall — not silently dropped, and
    // not an error that would vanish into a warning string while the day's
    // profile kept reporting the dispatched energy.
    ev2.apply_control_unchecked(&ControlSignal::EvDrive {
        energy_j: 1.0 * 3.6e6,
    })
    .unwrap();
    assert!(
        ev2.soc.abs() < 1e-9,
        "overdraw truncates to available: SOC must land exactly at 0, got {}",
        ev2.soc
    );
    {
        let expected_j = (4.0f64 - 0.05 * 60.0 * capacity_derate_at(10.0)) * 3.6e6;
        assert!(
            (ev2.drive_shortfall_j - expected_j).abs() < 1e-6,
            "the undeliverable remainder must be accounted, not dropped: \
             expected {expected_j}, got {}",
            ev2.drive_shortfall_j
        );
    }
    let mut ports = PortSlots::default();
    ev2.step(&env, Duration::minutes(1), &mut ports).unwrap();
    {
        let want = (4.0f64 - 0.05 * 60.0 * capacity_derate_at(10.0)) * 3.6e6;
        assert!(
            (ev2.telemetry().get("drive_shortfall_kwh").unwrap() - want).abs() < 1e-6,
            "the drive shortfall must be published as observable telemetry: \
             expected {want}, got {}",
            ev2.telemetry().get("drive_shortfall_kwh").unwrap()
        );
    }
}

/// The shortfall is cumulative (successive overdraws sum, not last-write)
/// and survives a checkpoint round-trip — the checkpoint schema gained the
/// field (v4), so a save/load must not silently reset the accounting.
#[test]
fn drive_shortfall_accumulates_and_survives_checkpoint() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.05.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();
    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::Disconnected,
    })
    .unwrap();

    // Available = 0.05 × usable capacity (rated × the temperature derate
    // at the unpinned 10 °C ambient initialization); first drive overdraws,
    // second (at SOC 0) is entirely undeliverable.
    let available = 0.05 * 60.0 * capacity_derate_at(10.0);
    ev.apply_control_unchecked(&ControlSignal::EvDrive {
        energy_j: 4.0 * 3.6e6,
    })
    .unwrap();
    ev.apply_control_unchecked(&ControlSignal::EvDrive {
        energy_j: 2.0 * 3.6e6,
    })
    .unwrap();
    assert!(
        (ev.drive_shortfall_j - (6.0 - available) * 3.6e6).abs() < 1e-3,
        "successive shortfalls must accumulate to 6.0 − available kWh \
         ({available:.4}), got {} J",
        ev.drive_shortfall_j
    );

    let state = ev.save_state().unwrap();
    let mut restored = Ev::new(config.clone());
    restored.init(&config, &env).unwrap();
    restored.load_state(&state).unwrap();
    assert!(
        (restored.drive_shortfall_j - ev.drive_shortfall_j).abs() < 1e-12,
        "checkpoint round-trip must preserve the cumulative shortfall"
    );
    assert_eq!(restored.soc, 0.0);
    // Double round-trip: bytes identical (no field dropped on re-save).
    assert_eq!(state, restored.save_state().unwrap());
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

/// Pack heating during a charge session is I²R through the cell resistance
/// — the only cell heat the electrical model produces — never the charger's
/// AC→DC conversion loss. Pre-fix, `(1−η)·P` (720 W here) was injected into
/// the pack, producing a 129.6 K rise in one hour; the physical I²R at
/// Level 2 currents is O(10 W), a ~1 K rise into this test's 20 kJ/K mass
/// (sub-kelvin into a real ~384 kJ/K pack).
#[test]
fn charge_session_pack_heating_is_i2r_only() {
    let mut raw = base_raw();
    raw.insert(KEY_CAPACITY_J.to_string(), 216000000.0.into());
    raw.insert(KEY_MAX_CHARGING_POWER_W.to_string(), 7200.0.into());
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

    let power_w = ev.telemetry().get("active_power_kw").unwrap();
    assert!(power_w > 0.0, "EV should be charging");

    // The I²R magnitude rule: single-digit watts at Level 2 — two orders of
    // magnitude below the conversion loss (720 W) the pre-fix model
    // attributed to the pack.
    let ohmic_w = ev.telemetry().get("ohmic_loss_w").unwrap();
    assert!(
        (1.0..20.0).contains(&ohmic_w),
        "I2R at Level 2 currents must be O(10 W), got {ohmic_w} W — \
         conversion losses are being attributed to the pack again"
    );

    // ΔT = Q·dt/C exactly, from the reported ohmic loss.
    let actual_dt = ev.battery_temp_c - temp_before;
    let expected_dt = ohmic_w * 3600.0 / 20_000.0;
    assert!(
        (actual_dt - expected_dt).abs() < 1e-9,
        "pack rise must equal I2R·dt/C = {expected_dt} K, got {actual_dt} K"
    );
    // Single-digit kelvin, never the 129.6 K of the conversion-loss model.
    assert!(
        actual_dt < 10.0,
        "a one-hour Level 2 session must warm the pack single-digit kelvin \
         (into this 20 kJ/K mass), got {actual_dt} K"
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
    ev.apply_control_unchecked(&ControlSignal::EvDrive {
        energy_j: 9.75 * 3.6e6,
    })
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
    let soc_start = ev.soc;
    let mut total_grid_j = 0.0;

    for _ in 0..600 {
        let mut ports = PortSlots::default();
        ev.step(&env, dt, &mut ports).unwrap();
        let power = ev.telemetry().get("active_power_kw").unwrap_or(0.0);
        if power > 0.0 {
            total_grid_j += power * dt.as_secs_f64();
        }
    }

    let soc_change = ev.soc - soc_start;
    let expected_grid_j = soc_change * ev.battery_capacity_j / ev.charging_efficiency;
    let error_j = (total_grid_j - expected_grid_j).abs();
    let tolerance_j = 0.02 * ev.battery_capacity_j;

    assert!(
        error_j < tolerance_j,
        "energy accounting error {error_j:.3} kWh exceeds 2% of capacity ({tolerance_j:.1} kWh). \
         Grid total: {total_grid_j:.3} kWh, expected: {expected_grid_j:.3} kWh, \
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

    let drive_j = 15.0 * 3.6e6; // 15 kWh, in J
    // The drive debits against the signal-time usable capacity; its own
    // I²R heat (a 15 kWh one-minute burst is a ~900 kW discharge) warms
    // the pack within the step, so the post-step capacity differs.
    let cap_at_drive = ev.battery_capacity_j;
    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::Disconnected,
    })
    .unwrap();
    ev.apply_control_unchecked(&ControlSignal::EvDrive { energy_j: drive_j })
        .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(1), &mut ports).unwrap();

    let expected_soc = 0.8 - drive_j / cap_at_drive;
    assert!(
        (ev.soc - expected_soc).abs() < 0.01,
        "SOC after the 15 kWh drive should be ~{expected_soc:.4}, got {}",
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

    // The drive debits against the signal-time usable capacity (rated ×
    // SOH × the temperature derate at the ambient-resolved pack temperature).
    let cap_at_drive = ev.battery_capacity_j;
    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::Disconnected,
    })
    .unwrap();
    ev.apply_control_unchecked(&ControlSignal::EvDrive {
        energy_j: 10.0 * 3.6e6,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, dt, &mut ports).unwrap();

    // The drive's own I²R heat is real physics and several kelvin here: a
    // 10 kWh burst in one 1-minute step is a ~600 kW discharge (~1700 A
    // into the pack), so the post-step capacity reflects a warmer pack than
    // the signal-time divisor the debit correctly used.
    let expected_soc = 1.0 - 10.0 * 3.6e6 / cap_at_drive;
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
    let drive_j = ev.soc * ev.battery_capacity_j - 1e-9;
    ev.apply_control_unchecked(&ControlSignal::EvPlugIn {
        state: EvConnectionState::Disconnected,
    })
    .unwrap();
    ev.apply_control_unchecked(&ControlSignal::EvDrive {
        energy_j: drive_j / 3.6e6,
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

    // At 12:00: 12 h until midnight, only 7.63 h needed → the proportional
    // pace is the deficit spread over the remaining 12 h (well under
    // rated; the pre-fix bang-bang model held zero here).
    env.current_time = dt(2026, 1, 1, 12, 0, 0);
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(1), &mut ports).unwrap();
    let power_early = ev.telemetry().get("active_power_kw").unwrap();
    {
        let deficit_j = (0.9 - ev.soc) * ev.battery_capacity_j;
        let expected_w = (deficit_j / (12.0 * 3600.0) / 0.9).min(7200.0);
        assert!(
            (power_early - expected_w).abs() < expected_w * 1e-2,
            "the midnight deadline paces the deficit over the remaining 12 h \
             (~{expected_w} W), got {power_early} W"
        );
    }

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
        capacity_j: 75.0 * 3.6e6,
        charging_level: None,
        max_charging_power_w: 11500.0,
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
        der_1547: None,
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
    assert_eq!(recovered.capacity_j, cfg.capacity_j);
}

// -- The IEEE 1547 response layer (der_1547) on the export function. --

fn v2g_config_with_der1547(settings: Option<crate::der1547::Der1547Settings>) -> EquipmentConfig {
    let cfg = EvConfig {
        der_1547: settings,
        v2g_enabled: Some(true),
        v2g_soc_reserve: Some(0.2),
        v2g_max_discharge_w: Some(5000.0),
        initial_soc: Some(0.8),
        ..minimal_ev_config()
    };
    EquipmentConfig::from_typed("EV #1".to_string(), "EV".to_string(), cfg).unwrap()
}

fn cat2_settings() -> crate::der1547::Der1547Settings {
    crate::der1547::Der1547Settings {
        category: crate::der1547::Der1547Category::CatII,
        overrides: None,
    }
}

fn command_v2g_discharge(ev: &mut Ev, w: f64) {
    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_w: -w,
        reactive_power_var: None,
        min_soc: None,
        max_soc: None,
    })
    .expect("setpoint");
}

/// A pre-1547 EV checkpoint (format v9, before the layer's state existed)
/// is refused with the typed version-mismatch error naming both versions:
/// the layer's state is checkpointed, never re-derived from a save that
/// predates it.
#[test]
fn a_pre_der1547_checkpoint_is_refused_not_migrated() {
    let config = v2g_config_with_der1547(Some(cat2_settings()));
    let mut ev = Ev::new(config.clone());
    let mut blob = ev.save_state().expect("the state saves");
    blob[..4].copy_from_slice(&9u32.to_le_bytes());
    let err = ev.load_state(&blob).expect_err("a v9 blob is refused");
    let msg = err.to_string();
    assert!(msg.contains("blob=9"), "{msg}");
    assert!(
        msg.contains(&format!("expected={}", Ev::checkpoint_version())),
        "{msg}"
    );
}

/// The F8-class defect's EV face: a sustained 0.6 pu sag sits inside
/// UV1's band (0.45 <= V < 0.70); the Category II default clearing time
/// is 10 s, so a 900 s timestep reaches it and the export inverter is
/// tripped for the step. The charge leg is the vehicle's load-side
/// behavior and is not the DER function: it is untouched by the layer.
/// The pre-1547 model exported through the sag unchanged.
#[test]
fn witness_der1547_ev_export_trips_on_a_sustained_06pu_sag_and_charge_is_load_side() {
    let config = v2g_config_with_der1547(Some(cat2_settings()));
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).expect("init");
    command_v2g_discharge(&mut ev, 3000.0);

    let mut sag = sample_env();
    sag.grid.voltage_pu = 0.6;
    let mut ports = PortSlots::default();
    ev.step(&sag, Duration::minutes(15), &mut ports)
        .expect("step");
    assert_eq!(
        ev.telemetry().get(tk::ACTIVE_POWER_KW),
        Some(0.0),
        "the 0.6 pu sag trips the configured export inverter (UV1: 10 s window)"
    );
    assert_eq!(ev.telemetry().get(tk::DER1547_TRIPPED), Some(1.0));
    assert_eq!(
        ev.telemetry().get(tk::DER1547_TRIP_REASON),
        Some(2.0),
        "uv1"
    );

    // The charge leg is load-side: a charging EV keeps charging through
    // the sag (the DER layer governs the export function only).
    let config = v2g_config_with_der1547(Some(cat2_settings()));
    let mut ev = Ev::new(config.clone());
    ev.init(&config, &sample_env()).expect("init");
    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_w: 7200.0,
        reactive_power_var: None,
        min_soc: None,
        max_soc: None,
    })
    .expect("setpoint");
    let mut ports = PortSlots::default();
    ev.step(&sag, Duration::minutes(15), &mut ports)
        .expect("step");
    assert!(
        ev.telemetry().get(tk::ACTIVE_POWER_KW).unwrap_or(0.0) > 1.0,
        "the charge leg is the vehicle's load-side behavior, not the DER \
         function: it continues through the sag"
    );

    // The unconfigured layer's EV face: the pre-1547 behavior, warned and
    // counted.
    let config = v2g_config_with_der1547(None);
    let mut ev = Ev::new(config.clone());
    ev.init(&config, &sample_env()).expect("init");
    command_v2g_discharge(&mut ev, 3000.0);
    let mut ports = PortSlots::default();
    ev.step(&sag, Duration::minutes(15), &mut ports)
        .expect("step");
    assert!(
        ev.telemetry().get(tk::ACTIVE_POWER_KW).unwrap_or(0.0) < -0.1,
        "unconfigured: the pre-1547 pinned-source behavior"
    );
    assert_eq!(
        ev.telemetry().get(tk::DER1547_UNCONFIGURED_EXCURSIONS),
        Some(1.0)
    );
}

/// The F7 guard on the EV's face: on a utility-outage step the export leg
/// can be the island's source, so the layer is bypassed and the export
/// flows; the layer is never tripped off its own island.
#[test]
fn witness_der1547_ev_island_source_is_never_tripped_off_its_island() {
    let config = v2g_config_with_der1547(Some(cat2_settings()));
    let mut ev = Ev::new(config.clone());
    ev.init(&config, &sample_env()).expect("init");
    command_v2g_discharge(&mut ev, 3000.0);

    let mut islanded = sample_env();
    islanded.grid.voltage_pu = 0.0;
    islanded.grid.island_bus_voltage_pu = Some(1.0);
    let mut ports = PortSlots::default();
    ev.step(&islanded, Duration::minutes(15), &mut ports)
        .expect("step");
    assert!(
        ev.telemetry().get(tk::ACTIVE_POWER_KW).unwrap_or(0.0) < -0.1,
        "the island source exports through the outage step"
    );
    assert_eq!(ev.telemetry().get(tk::DER1547_TRIPPED), Some(0.0));
}

/// The frequency-droop shape on the export leg: 60.536 Hz is 0.5 Hz
/// beyond the 60.036 Hz deadband edge; the 5 percent droop slope is
/// 1/(60 x 0.05) = 1/3 pu of the 5000 W export rating per Hz, so a
/// 3000 W (0.6 pu) export ramps down 1/6 pu = 833.333 W to 2166.667 W.
#[test]
fn witness_der1547_ev_export_over_frequency_droop_reduces_export() {
    let config = v2g_config_with_der1547(Some(cat2_settings()));
    let mut ev = Ev::new(config.clone());
    ev.init(&config, &sample_env()).expect("init");
    command_v2g_discharge(&mut ev, 3000.0);

    // A normal step first: the droop's pre-disturbance latch takes the
    // running export.
    let mut ports = PortSlots::default();
    ev.step(&sample_env(), Duration::minutes(15), &mut ports)
        .expect("step");

    let mut hi_f = sample_env();
    hi_f.grid.frequency_hz = 60.536;
    let mut ports = PortSlots::default();
    ev.step(&hi_f, Duration::minutes(15), &mut ports)
        .expect("step");
    let p_w = ev.telemetry().get(tk::ACTIVE_POWER_KW).unwrap_or(0.0);
    let expected_w = -(3000.0 - 833.3333333333334);
    assert!(
        (p_w - expected_w).abs() < 1e-6,
        "the droop sheds 1/6 pu of the 5000 W rating: expected \
         {expected_w} W, got {p_w} W"
    );
    assert_eq!(ev.telemetry().get(tk::DER1547_TRIPPED), Some(0.0));
}

/// The frequency-Watt reference cannot inject more than the demand
/// response level allows. The DR fraction is the operator's cap on the
/// delivered export, the same authority the stationary battery's clamp
/// carries (there the clamp runs after the layer, so the cap binds the
/// droop's reference); the EV passes its post-DR export as the layer's
/// pre-disturbance output, so the availability input must be the DR-scaled
/// ceiling too, or an under-frequency ramp resurrects the shed watts. A
/// GridEmergency (a full shed) holds zero export through the excursion;
/// a High level caps the ramp at half the rating.
#[test]
fn witness_der1547_ev_droop_cannot_exceed_the_demand_response_cap() {
    for (level, expected_w) in [(DRLevel::High, 2500.0), (DRLevel::GridEmergency, 0.0)] {
        let config = v2g_config_with_der1547(Some(cat2_settings()));
        let mut ev = Ev::new(config.clone());
        ev.init(&config, &sample_env()).expect("init");
        ev.apply_control(&ControlSignal::DemandResponse {
            level,
            duration_s: Some(3600.0),
        })
        .expect("DR accepted");
        command_v2g_discharge(&mut ev, 5000.0);

        // 59.5 Hz is 0.464 Hz below the 59.964 Hz deadband edge: the ramp
        // wants p_pre + 0.464/3 pu. High caps it at half the 5000 W
        // rating; GridEmergency caps it at zero.
        let mut low_f = sample_env();
        low_f.grid.frequency_hz = 59.5;
        let mut ports = PortSlots::default();
        ev.step(&low_f, Duration::from_secs(60), &mut ports)
            .expect("step");
        let p_w = ev.telemetry().get(tk::ACTIVE_POWER_KW).unwrap_or(0.0);
        assert!(
            (p_w + expected_w).abs() < 1e-6,
            "{level:?}: the DR cap must bind the under-frequency ramp: \
             expected {} W, got {p_w} W",
            -expected_w
        );
    }
}

/// The droop's anchor is the export the deadband crossing sees, not a
/// latch left over from an earlier excursion: the V2G inverter conducts
/// nothing on the idle steps in between (the layer is not stepped while
/// the export leg is off), and a ramp from a stale anchor would export
/// more than the pre-disturbance output on the second excursion, the
/// wrong direction for an over-frequency droop.
#[test]
fn droop_anchor_is_the_deadband_crossing_export_not_a_stale_latch() {
    let config = v2g_config_with_der1547(Some(cat2_settings()));
    let mut ev = Ev::new(config.clone());
    ev.init(&config, &sample_env()).expect("init");

    // A 3 kW export at nominal frequency, then the excursion: the anchor
    // latches at 0.6 pu of the 5 kW rating.
    command_v2g_discharge(&mut ev, 3000.0);
    let mut ports = PortSlots::default();
    ev.step(&sample_env(), Duration::minutes(15), &mut ports)
        .expect("step");
    let mut hi_f = sample_env();
    hi_f.grid.frequency_hz = 60.536;
    let mut ports = PortSlots::default();
    ev.step(&hi_f, Duration::minutes(15), &mut ports)
        .expect("step");
    assert!(
        ev.telemetry().get(tk::ACTIVE_POWER_KW).unwrap_or(0.0) < -1000.0,
        "the excursion step exports the ramped reference"
    );

    // The driver idles the export for a step at nominal frequency: the
    // inverter conducts nothing.
    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_w: 0.0,
        reactive_power_var: None,
        min_soc: None,
        max_soc: None,
    })
    .expect("setpoint");
    let mut ports = PortSlots::default();
    ev.step(&sample_env(), Duration::minutes(15), &mut ports)
        .expect("step");
    assert_eq!(ev.telemetry().get(tk::ACTIVE_POWER_KW), Some(0.0));

    // A 2 kW export on a fresh excursion: 0.3 Hz beyond the 60.036 Hz
    // edge at 1/3 pu per Hz is 0.1 pu of ramp, so the reference is
    // 0.4 - 0.1 = 0.3 pu = 1500 W. A stale 0.6 pu anchor would give
    // 0.5 pu = 2500 W, above the pre-disturbance output.
    command_v2g_discharge(&mut ev, 2000.0);
    let mut hi_f = sample_env();
    hi_f.grid.frequency_hz = 60.336;
    let mut ports = PortSlots::default();
    ev.step(&hi_f, Duration::minutes(15), &mut ports)
        .expect("step");
    let p_w = ev.telemetry().get(tk::ACTIVE_POWER_KW).unwrap_or(0.0);
    assert!(
        (p_w + 1500.0).abs() < 1e-6,
        "the ramp anchors on the 2 kW the export is delivering: expected \
         -1500 W, got {p_w} W"
    );
}

/// The 1547 run state survives a checkpoint round trip: a trip latched
/// before the save still holds after the restore.
#[test]
fn witness_der1547_ev_trip_latch_survives_checkpoint_round_trip() {
    let config = v2g_config_with_der1547(Some(cat2_settings()));
    let mut ev = Ev::new(config.clone());
    ev.init(&config, &sample_env()).expect("init");
    command_v2g_discharge(&mut ev, 3000.0);

    let mut sag = sample_env();
    sag.grid.voltage_pu = 0.6;
    let mut ports = PortSlots::default();
    ev.step(&sag, Duration::minutes(15), &mut ports)
        .expect("step");
    assert_eq!(ev.telemetry().get(tk::DER1547_TRIPPED), Some(1.0));

    let state = ev.save_state().unwrap();
    let mut restored = Ev::new(config.clone());
    restored.init(&config, &sample_env()).expect("init");
    restored.load_state(&state).expect("load");

    command_v2g_discharge(&mut restored, 3000.0);
    let mut ports = PortSlots::default();
    restored
        .step(&sag, Duration::minutes(15), &mut ports)
        .expect("step");
    assert_eq!(
        restored.telemetry().get(tk::DER1547_TRIPPED),
        Some(1.0),
        "the latch rode the checkpoint"
    );
    assert_eq!(restored.telemetry().get(tk::ACTIVE_POWER_KW), Some(0.0));
}

/// The firing step's ride-through publication: the EV's
/// `der1547_ride_through_s` channel reports the window the firing step
/// reached, as the PV's and the battery's channels do (the step outcome's
/// contract: "the firing step reports the window it reached"). The trip
/// spends the run state's accumulators, so the channel cannot read them
/// on the firing step: a state-based read publishes 0.0 exactly where the
/// other DER classes publish the reached window, and the same telemetry
/// key then means different things per equipment.
#[test]
fn witness_der1547_ev_publishes_the_window_the_firing_step_reached() {
    let config = v2g_config_with_der1547(Some(cat2_settings()));
    let mut ev = Ev::new(config.clone());
    ev.init(&config, &sample_env()).expect("init");
    command_v2g_discharge(&mut ev, 3000.0);

    // The 10 s UV1 window is reached inside the 60 s sag step: the
    // firing step's channel must carry the 10 s it reached.
    let mut sag = sample_env();
    sag.grid.voltage_pu = 0.6;
    let mut ports = PortSlots::default();
    ev.step(&sag, Duration::from_secs(60), &mut ports)
        .expect("step");
    assert_eq!(ev.telemetry().get(tk::DER1547_TRIPPED), Some(1.0));
    assert_eq!(
        ev.telemetry().get(tk::DER1547_RIDE_THROUGH_S),
        Some(10.0),
        "the firing step reports the window it reached"
    );

    // The latched step reports no window (the per-disturbance timers are
    // spent) and the enter-service accumulation is its own channel: 60 s
    // inside the windows at nominal.
    let mut ports = PortSlots::default();
    ev.step(&sample_env(), Duration::from_secs(60), &mut ports)
        .expect("step");
    assert_eq!(ev.telemetry().get(tk::DER1547_TRIPPED), Some(1.0));
    assert_eq!(ev.telemetry().get(tk::DER1547_RIDE_THROUGH_S), Some(0.0));
    assert_eq!(ev.telemetry().get(tk::DER1547_ENTER_SERVICE_S), Some(60.0));
}

/// The shared trip channels report the step the inverter actually took.
/// The layer's fail-safe (a non-finite bus voltage or grid frequency
/// trips the inverter for the step) is that step's outcome and never
/// reaches the latch, so a state-based read publishes "online" for a step
/// the inverter ceased to energize on while the PV and the battery
/// publish the outcome. The EV's channels carry the outcome the step
/// took; the latch, its reason and the enter-service clock remain the
/// run state's on the steps the layer does not step.
#[test]
fn witness_der1547_ev_publishes_the_failsafe_trip_the_step_took() {
    let config = v2g_config_with_der1547(Some(cat2_settings()));
    let mut ev = Ev::new(config.clone());
    ev.init(&config, &sample_env()).expect("init");
    command_v2g_discharge(&mut ev, 3000.0);

    // A poisoned grid reading: the layer's documented fail-safe is a trip
    // for the step (the inverter must not ride a meaningless reading).
    let mut poisoned = sample_env();
    poisoned.grid.frequency_hz = f64::NAN;
    let mut ports = PortSlots::default();
    ev.step(&poisoned, Duration::from_secs(60), &mut ports)
        .expect("step");
    assert_eq!(
        ev.telemetry().get(tk::ACTIVE_POWER_KW),
        Some(0.0),
        "the fail-safe trip zeroes the export"
    );
    assert_eq!(
        ev.telemetry().get(tk::DER1547_TRIPPED),
        Some(1.0),
        "the step tripped the inverter: the channel reports the step it took"
    );
    assert_eq!(
        ev.telemetry().get(tk::DER1547_RIDE_THROUGH_S),
        Some(0.0),
        "a fail-safe trip reaches no window"
    );

    // The fail-safe is per step: the latch was never set, so the next
    // sound step exports again.
    let mut ports = PortSlots::default();
    ev.step(&sample_env(), Duration::from_secs(60), &mut ports)
        .expect("step");
    assert_eq!(ev.telemetry().get(tk::DER1547_TRIPPED), Some(0.0));
    assert!(
        ev.telemetry().get(tk::ACTIVE_POWER_KW).unwrap_or(0.0) < -2900.0,
        "the export resumes on the next sound step"
    );
}

#[test]
fn ev_config_rejects_unknown_fields() {
    use crate::config::ConfigPayload;
    let json = serde_json::json!({
        "capacity_j": 270000000.0,
        "max_charging_power_w": 7200.0,
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
    cfg.capacity_j = 0.0;
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

/// The pack-thermal and topology fields the I-07 alignment added are
/// config-surface boundary inputs: an unphysical value that silently
/// falls through to the physics (a zero thermal mass, a negative
/// resistance, a zero series count) produces NaN temperatures or
/// division-by-zero deep in the step loop instead of a named error at
/// the boundary. Each rule rejects its invalid value, and the paired
/// boundary-legal value passes — so an inverted comparison or a dropped
/// rule fails loudly here, not silently in a 45-day run.
#[test]
fn ev_config_validate_rejects_invalid_pack_thermal_and_topology_fields() {
    let rejects = |mutate: &dyn Fn(&mut EvConfig)| {
        let mut cfg = minimal_ev_config();
        mutate(&mut cfg);
        cfg.validate()
    };

    // Thermal mass: non-positive and non-finite are both unphysical
    // (zero mass means any heat produces infinite temperature).
    assert!(
        rejects(&|c| c.thermal_mass_j_per_k = Some(0.0)).is_err(),
        "thermal_mass_j_per_k = 0 must be rejected (infinite temperature rise)"
    );
    assert!(
        rejects(&|c| c.thermal_mass_j_per_k = Some(f64::NAN)).is_err(),
        "non-finite thermal_mass_j_per_k must be rejected"
    );
    // Boundary-legal: any positive finite mass passes.
    assert!(rejects(&|c| c.thermal_mass_j_per_k = Some(1.0)).is_ok());

    // UA: negative coupling would heat a pack warmer than ambient;
    // zero (perfect insulation) is a legal pinned test premise.
    assert!(rejects(&|c| c.ua_w_per_k = Some(-0.1)).is_err());
    assert!(rejects(&|c| c.ua_w_per_k = Some(0.0)).is_ok());

    // Heater power: negative power is a fridge, not a heater; zero (no
    // heater) is the explicit heaterless configuration.
    assert!(rejects(&|c| c.heater_power_w = Some(-1.0)).is_err());
    assert!(rejects(&|c| c.heater_power_w = Some(0.0)).is_ok());

    // Topology: zero series/parallel counts collapse the pack voltage and
    // resistance to zero; a non-positive cell resistance breaks the
    // terminal-voltage quadratic (division by R).
    assert!(rejects(&|c| c.n_series = Some(0)).is_err());
    assert!(rejects(&|c| c.n_series = Some(1)).is_ok());
    assert!(rejects(&|c| c.n_parallel = Some(0)).is_err());
    assert!(rejects(&|c| c.n_parallel = Some(1)).is_ok());
    assert!(rejects(&|c| c.cell_resistance_ohm = Some(0.0)).is_err());
    assert!(rejects(&|c| c.cell_resistance_ohm = Some(-0.005)).is_err());
    assert!(
        rejects(&|c| c.cell_resistance_ohm = Some(f64::NAN)).is_err(),
        "non-finite cell_resistance_ohm must be rejected"
    );
    assert!(rejects(&|c| c.cell_resistance_ohm = Some(0.005)).is_ok());
}

#[test]
fn ev_config_validate_rejects_non_finite_temperature_fields() {
    let rejects = |mutate: &dyn Fn(&mut EvConfig)| {
        let mut cfg = minimal_ev_config();
        mutate(&mut cfg);
        cfg.validate()
    };

    // A non-finite pack or threshold temperature must fail loudly at the
    // config boundary: NaN falls through both comparison branches of
    // `linear_temp_derate` into the interpolation, so a NaN
    // `battery_temp_c` yields a NaN charge derate and silently poisons
    // charging power, SOC, and every downstream energy total.
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(
            rejects(&|c| c.battery_temp_c = Some(bad)).is_err(),
            "non-finite battery_temp_c ({bad}) must be rejected"
        );
        assert!(
            rejects(&|c| c.min_charge_temp_c = Some(bad)).is_err(),
            "non-finite min_charge_temp_c ({bad}) must be rejected"
        );
        assert!(
            rejects(&|c| c.full_power_temp_c = Some(bad)).is_err(),
            "non-finite full_power_temp_c ({bad}) must be rejected"
        );
        assert!(
            rejects(&|c| c.heater_threshold_c = Some(bad)).is_err(),
            "non-finite heater_threshold_c ({bad}) must be rejected"
        );
    }

    // An inverted derate ramp is a misconfiguration, not a model choice:
    // with min > full, `linear_temp_derate`'s `temp >= temp_max` branch
    // wins everywhere above temp_max, so a swapped pair silently charges
    // at full power across the band the user meant to derate.
    assert!(
        rejects(&|c| {
            c.min_charge_temp_c = Some(20.0);
            c.full_power_temp_c = Some(10.0);
        })
        .is_err(),
        "min_charge_temp_c > full_power_temp_c must be rejected"
    );

    // Boundary-legal: physically extreme but finite temperatures pass,
    // and an equal min/full pair is a legal (if abrupt) step cutoff.
    assert!(rejects(&|c| c.battery_temp_c = Some(-40.0)).is_ok());
    assert!(rejects(&|c| c.min_charge_temp_c = Some(-20.0)).is_ok());
    assert!(rejects(&|c| c.full_power_temp_c = Some(60.0)).is_ok());
    assert!(rejects(&|c| c.heater_threshold_c = Some(45.0)).is_ok());
    assert!(
        rejects(&|c| {
            c.min_charge_temp_c = Some(10.0);
            c.full_power_temp_c = Some(10.0);
        })
        .is_ok()
    );
}

#[test]
fn ev_init_typed_sets_fields_from_ev_config() {
    let cfg = EvConfig {
        capacity_j: 60.0 * 3.6e6,
        charging_level: Some("L2".to_string()),
        max_charging_power_w: 7200.0,
        charging_efficiency: Some(0.92),
        initial_soc: Some(0.5),
        ..minimal_ev_config()
    };
    let ec =
        crate::EquipmentConfig::from_typed("test_ev".to_string(), "EV".to_string(), cfg).unwrap();
    let mut ev = Ev::new(ec.clone());
    let env = sample_env();
    ev.init(&ec, &env).unwrap();
    assert_eq!(ev.battery_capacity_j_rated, 60.0 * 3.6e6);
    // The usable capacity at init: rated × SOH(1) × the temperature derate
    // at the ambient-resolved pack temperature (minimal_ev_config leaves
    // battery_temp_c unset → the 10 °C outdoor ambient).
    assert!((ev.battery_capacity_j - 60.0 * 3.6e6 * capacity_derate_at(10.0)).abs() < 1e-3);
    assert!((ev.charging_efficiency - 0.92).abs() < 1e-9);
    assert!((ev.soc - 0.5).abs() < 1e-9);
    assert_eq!(ev.charging_level, ChargingLevel::L2);
    assert!((ev.rated_power_w - 7200.0).abs() < 1e-6);
}

// ── Typed cold-charging field path tests ───────────────────────────

#[test]
fn typed_config_reads_cold_charging_and_strategy_fields() {
    // The raw payload path is gone: the same field set lands through the
    // typed config.
    let strategy_json = serde_json::to_string(&ChargingStrategy::Nightly {
        off_peak_start_hour: 22.0,
        off_peak_end_hour: 6.0,
        target_soc: 0.8,
    })
    .unwrap();
    let mut raw = base_raw();
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
    raw.insert(
        KEY_CHARGING_STRATEGY.to_string(),
        crate::config::ConfigValue::Text(strategy_json),
    );

    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

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
    // The placeholder constructor holds the typed defaults (the raw
    // payload path is gone; init replaces them from the config or the
    // cascade). The defaults derive from the default capacity (75 kWh).
    let ev = Ev::new(EquipmentConfig::raw(
        "test_ev".to_string(),
        "EV".to_string(),
        HashMap::new(),
    ));

    assert_eq!(ev.min_charge_temp_c, DEFAULT_MIN_CHARGE_TEMP_C);
    assert_eq!(ev.full_power_temp_c, DEFAULT_FULL_POWER_TEMP_C);
    assert_eq!(ev.heater_power_w, DEFAULT_HEATER_POWER_W);
    assert_eq!(ev.heater_threshold_c, DEFAULT_HEATER_THRESHOLD_C);
    // Thermal defaults are capacity-derived (pack mass × cell specific
    // heat; area-scaled UA), not flat constants.
    assert_eq!(
        ev.thermal_mass_j_per_k,
        default_thermal_mass_j_per_k(DEFAULT_CAPACITY_J)
    );
    assert_eq!(ev.ua_w_per_k, default_ua_w_per_k(DEFAULT_CAPACITY_J));
    assert_eq!(
        ev.charging_strategy(),
        &ChargingStrategy::Immediate { target_soc: 1.0 }
    );
}

/// A dispatched `power_limit_w = 0` is the second entry to the
/// commanded-zero invariant (GridEmergency is the first, covered in the
/// DR test): the limit enters through the *supply bound* —
/// `min(rating, power_limit)` at ev/mod.rs — not through the DR
/// multiplier, so a regression that applies the dispatched limit only to
/// the charge leg (leaving the heater drawing under a commanded zero)
/// is invisible to the DR test and caught here. The port zeroes and the
/// heater suspends with it: the commanded zero removes the supply, and
/// spending dispatch budget on warming is not available without one.
#[test]
fn commanded_power_limit_zero_suspends_preconditioning_with_the_port() {
    // Cold derate band (4 °C → charge demand 2.88 kW AC) with the heater
    // on: without the limit this draws the composed total; the commanded
    // zero must take both legs to zero.
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.2.into());
    raw.insert(KEY_BATTERY_TEMP_C.to_string(), 4.0.into());
    raw.insert(KEY_MIN_CHARGE_TEMP_C.to_string(), 0.0.into());
    raw.insert(KEY_FULL_POWER_TEMP_C.to_string(), 10.0.into());
    raw.insert(KEY_HEATER_POWER_W.to_string(), 1500.0.into());
    raw.insert(KEY_HEATER_THRESHOLD_C.to_string(), 5.0.into());
    raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
    raw.insert(KEY_POWER_LIMIT_W.to_string(), 0.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    ev.init(&config, &sample_env()).unwrap();
    let temp_before = ev.battery_temp_c;

    let mut ports = PortSlots::default();
    ev.step(&sample_env(), Duration::minutes(15), &mut ports)
        .unwrap();
    assert_eq!(
        ev.telemetry().get("active_power_kw").unwrap(),
        0.0,
        "a dispatched power_limit_w = 0 must zero the port"
    );
    assert_eq!(
        ev.telemetry().get("heater_power_w").unwrap(),
        0.0,
        "the commanded zero must suspend the heater with the port — the \
         limit bounds the charger's whole AC input (charge leg + heater \
         AC-equivalent), not the charge leg alone"
    );
    assert_eq!(
        ev.battery_temp_c, temp_before,
        "with UA = 0 the temperature can only move if heat was applied — \
         a zero-limited step must warm nothing"
    );
}

/// The capacity-derived thermal defaults carry their cited densities, and
/// the derivation *scales* with capacity — the anti-flat-constant
/// invariant. D1's defect was a flat 20 kJ/K mass for every pack; a
/// regression to any flat constant can pass the 75 kWh parked-pack
/// regression's window while silently mis-scaling every other vehicle
/// size (a 14.8 kWh PHEV is not a 75 kWh BEV). Cited values: pack mass
/// 6.4 kg/kWh × 1000 J/(kg·K) Li-ion cell specific heat → 480 kJ/K at
/// 75 kWh; UA 5.0 W/K × (capacity/13.5)^(2/3) area scaling anchored at
/// the stationary Battery's enclosed-pack default → ≈15.7 W/K at 75 kWh;
/// n_parallel = ceil(pack Ah / 5 Ah 21700 cell) → 43 at 75 kWh on 96S.
/// The paired-capacity assertions pin the scaling law itself: mass
/// linear, UA a 2/3 power law.
#[test]
fn capacity_derived_thermal_defaults_match_cited_densities_and_scale_with_capacity() {
    let mass_75 = default_thermal_mass_j_per_k(75.0 * 3.6e6);
    assert_eq!(
        mass_75, 480_000.0,
        "75 kWh × 6.4 kg/kWh × 1000 J/(kg·K) = 480 kJ/K exactly"
    );
    let ua_75 = default_ua_w_per_k(75.0 * 3.6e6);
    assert!(
        (ua_75 - 15.7).abs() < 0.1,
        "UA at 75 kWh is 5.0 × (75/13.5)^(2/3) ≈ 15.7 W/K, got {ua_75}"
    );
    assert_eq!(
        default_n_parallel(75.0 * 3.6e6),
        43,
        "75 kWh on 96S at 3.7 V nominal is 211 Ah → ceil(211/5 Ah) = 43P"
    );

    // The scaling law, not just one point: half the capacity → half the
    // mass (linear) and UA × (1/2)^(2/3) (area scaling). A flat constant
    // fails both.
    let mass_37 = default_thermal_mass_j_per_k(37.5 * 3.6e6);
    assert_eq!(mass_37, mass_75 / 2.0);
    let ua_37 = default_ua_w_per_k(37.5 * 3.6e6);
    assert!((ua_37 - ua_75 * 0.5f64.powf(2.0 / 3.0)).abs() < 1e-9);
    assert!(mass_37 < mass_75 && ua_37 < ua_75);
}

#[test]
fn raw_and_typed_config_cold_derate_equivalence() {
    // The typed path: -15 C min, 10 C full, battery at 0 C (derate = 0.6).
    // The raw payload path is gone: a raw config is refused at init, so
    // the equivalence test's raw half is the refusal.
    let mut raw_data = HashMap::new();
    raw_data.insert(KEY_CAPACITY_J.to_string(), 216000000.0.into());
    raw_data.insert(KEY_CHARGING_LEVEL.to_string(), "L2".into());
    raw_data.insert(KEY_MAX_CHARGING_POWER_W.to_string(), 7200.0.into());
    raw_data.insert(KEY_INITIAL_SOC.to_string(), 0.3.into());
    let raw_config = EquipmentConfig::raw("test_ev".to_string(), "EV".to_string(), raw_data);
    let mut raw_ev = Ev::new(raw_config.clone());

    let cfg = EvConfig {
        capacity_j: 60.0 * 3.6e6,
        max_charging_power_w: 7200.0,
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

    assert_eq!(typed_ev.min_charge_temp_c, -15.0);
    assert_eq!(typed_ev.full_power_temp_c, 10.0);

    // The derate read post-init (the pack at 0 C, the ramp -15 to 10):
    // the ramp's linear value, and the step charges at the derated rate,
    // below the level's full power.
    let typed_derate = typed_ev.telemetry().get("charge_derate");
    assert_eq!(
        typed_derate,
        Some(0.6),
        "the 0 C pack between -15 and 10 derates to the ramp's linear value"
    );
    raw_ev
        .init(&raw_config, &env)
        .expect_err("a raw EV payload is refused at init");
    let mut ports = PortSlots::default();
    typed_ev
        .step(&env, Duration::minutes(15), &mut ports)
        .unwrap();

    let typed_power = typed_ev.telemetry().get("active_power_kw").unwrap();
    assert!(
        typed_power > 0.0 && typed_power <= 7200.0,
        "the cold pack charges at or below the level's power, got {typed_power}"
    );
}

/// Raw and typed init paths produce the same `battery_temp_c` when the
/// config key is absent from both. This test would have failed before the
/// fix because the raw path defaulted to 20.0 and the typed path defaulted
/// to the environment's outdoor temperature.
#[test]
fn raw_and_typed_default_battery_temp_c_are_consistent() {
    let env = sample_env();

    // Raw construction (registry factory path — cannot init, raw configs
    // are refused at init): no battery_temp_c key → the pre-init
    // placeholder DEFAULT_BATTERY_TEMP_C, which init replaces by cascade.
    let raw_config = EquipmentConfig::raw("test_ev".to_string(), "EV".to_string(), base_raw());
    let raw_ev = Ev::new(raw_config);
    assert_eq!(raw_ev.battery_temp_c, DEFAULT_BATTERY_TEMP_C);

    // Typed path (the production init path): None battery_temp_c →
    // outdoor ambient (the Battery's cascade — explicit config, else
    // ambient; the EV has no zone). The placeholder never survives into
    // the simulation.
    let cfg = EvConfig {
        battery_temp_c: None,
        ..minimal_ev_config()
    };
    let typed = EquipmentConfig::from_typed("test_ev".to_string(), "EV".to_string(), cfg).unwrap();
    let mut typed_ev = Ev::new(typed.clone());
    typed_ev.init(&typed, &env).unwrap();
    assert_eq!(typed_ev.battery_temp_c, env.weather.outdoor_temp_c);
}

/// Both paths honour an explicit `battery_temp_c` value when present in config.
#[test]
fn raw_and_typed_explicit_battery_temp_c_is_honoured() {
    let env = sample_env();

    // The raw path is gone: the placeholder constructor's explicit-key
    // read is the typed default's, and the raw config is refused at
    // init. The explicit value honours only the typed path now.
    let cfg = EvConfig {
        battery_temp_c: Some(35.0),
        ..minimal_ev_config()
    };
    let typed = EquipmentConfig::from_typed("test_ev".to_string(), "EV".to_string(), cfg).unwrap();
    let mut typed_ev = Ev::new(typed.clone());
    typed_ev.init(&typed, &env).unwrap();

    assert_eq!(typed_ev.battery_temp_c, 35.0);

    // The omitted key's cascade: the outdoor ambient (the sample's 10 C).
    let omitted = EvConfig {
        ..minimal_ev_config()
    };
    let typed =
        EquipmentConfig::from_typed("test_ev2".to_string(), "EV".to_string(), omitted).unwrap();
    let mut omitted_ev = Ev::new(typed.clone());
    omitted_ev.init(&typed, &env).unwrap();
    assert_eq!(omitted_ev.battery_temp_c, 10.0);
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

    let max_allowed = ev.rated_power_w * 0.25;

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
        .expect("active_power_w missing from telemetry");
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
        .expect("active_power_w missing from telemetry");
    assert_eq!(ev.telemetry().get("dr_power_fraction"), Some(1.0));
    assert_eq!(ev.telemetry().get("dr_level"), Some(0.0));
    assert!(
        power >= ev.rated_power_w * 0.5,
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
        .reactive_power_var
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
    raw.insert(KEY_CHARGER_CAPACITY_VA.to_string(), 20000.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.2.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control(&ControlSignal::ReactiveSetpoint { var: 1500.0 })
        .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();

    let q = ev.telemetry().get(tk::REACTIVE_POWER_KVAR).unwrap();
    approx_eq(q, 1500.0);
}

/// PowerFactorSetpoint updates pf, zeros q_setpoint, and Q follows
/// the new pf baseline while charging.
#[test]
fn ev_power_factor_setpoint_zeros_q_setpoint_and_follows_baseline() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.q_setpoint_var = Some(2.0);
    ev.apply_control(&ControlSignal::PowerFactorSetpoint { power_factor: 0.8 })
        .unwrap();

    assert_eq!(ev.q_setpoint_var, None);
    approx_eq(ev.power_factor, 0.8);

    // Force charging via PowerSetpoint so P>0.
    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_w: 3000.0,
        reactive_power_var: None,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();

    let q = ev.telemetry().get(tk::REACTIVE_POWER_KVAR).unwrap();
    let p_w = ev.telemetry().get(tk::ACTIVE_POWER_KW).unwrap();
    let tan_phi = 0.8_f64.acos().tan();
    let expected_q = p_w * tan_phi;
    assert!(
        (q - expected_q).abs() < 1e-9,
        "expected Q={expected_q} (P={p_w} × tan(acos(0.8))={tan_phi}), got {q}"
    );
    assert!(q > 0.0, "charging should yield Q>0 absorbing");
}

/// PowerSetpoint with reactive_power_var is accepted and applied.
#[test]
fn ev_power_setpoint_stores_reactive_q() {
    let mut raw = base_raw();
    raw.insert(KEY_CHARGER_CAPACITY_VA.to_string(), 20000.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_w: 3000.0,
        reactive_power_var: Some(750.0),
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    assert_eq!(ev.q_setpoint_var, Some(750.0));

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();

    let q = ev.telemetry().get(tk::REACTIVE_POWER_KVAR).unwrap();
    approx_eq(q, 750.0);
}

/// A rejected PowerSetpoint must leave ALL control state unchanged: the
/// whole signal is validated before any mutation, so a negative active
/// power (rejected without v2l/v2g) must not arm the reactive override it
/// carried. Regression test for the partial-mutation bug where
/// `q_setpoint_var` was set before the negative-setpoint check.
#[test]
fn ev_rejected_power_setpoint_leaves_reactive_state_unchanged() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    let pf_before = ev.power_factor;
    assert_eq!(ev.q_setpoint_var, None);
    assert_eq!(ev.power_setpoint_w, None);

    // Negative setpoint without v2l/v2g must be rejected in full.
    let err = ev
        .apply_control(&ControlSignal::PowerSetpoint {
            active_power_w: -3000.0,
            reactive_power_var: Some(1.25),
            min_soc: None,
            max_soc: None,
        })
        .expect_err("negative PowerSetpoint without v2l/v2g must be rejected");
    assert!(
        err.to_string().contains("v2l_enabled or v2g_enabled"),
        "unexpected error: {err}"
    );

    assert_eq!(
        ev.q_setpoint_var, None,
        "rejected signal must not arm the var override"
    );
    assert_eq!(ev.power_setpoint_w, None);
    approx_eq(ev.power_factor, pf_before);
}

/// A commanded ReactiveSetpoint of exactly 0.0 is an absolute override: it
/// must force Q = 0 even though the pf < 1 baseline would otherwise produce
/// nonzero Q while charging (`Some(0.0)` is distinct from `None`).
#[test]
fn ev_reactive_setpoint_zero_forces_q_zero_over_pf_baseline() {
    let mut raw = base_raw();
    raw.insert(KEY_POWER_FACTOR.to_string(), 0.9.into());
    raw.insert(KEY_CHARGER_CAPACITY_VA.to_string(), 20000.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    // Charge so the pf = 0.9 baseline produces Q > 0.
    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_w: 3000.0,
        reactive_power_var: None,
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
    ev.apply_control(&ControlSignal::ReactiveSetpoint { var: 0.0 })
        .unwrap();
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();
    let q = ev.telemetry().get(tk::REACTIVE_POWER_KVAR).unwrap();
    approx_eq(q, 0.0);
    approx_eq(ports.electrical.reactive_power_var, 0.0);
}

/// kVA clamp: when Q is commanded beyond the charger's capability,
/// Q is reduced but P is unchanged (active-power priority).
#[test]
fn ev_kva_clamp_curtails_reactive_not_active_power() {
    let mut raw = base_raw();
    raw.insert(KEY_CHARGER_CAPACITY_VA.to_string(), 5000.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.2.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    // Command 5 kW charge + 5 kvar reactive → S = sqrt(P²+25) > 5 kVA.
    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_w: 3600.0,
        reactive_power_var: Some(5.0),
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();

    let p = ev.telemetry().get(tk::ACTIVE_POWER_KW).unwrap();
    let q = ev.telemetry().get(tk::REACTIVE_POWER_KVAR).unwrap();

    let s = ev.charger_capacity_va;
    let q_max = (s * s - p * p).max(0.0).sqrt();
    assert!(
        q.abs() <= q_max + 1e-9,
        "|Q|={} exceeds sqrt(S²−P²)={} with S={s}, P={p}",
        q.abs(),
        q_max
    );
    assert!(
        q.abs() < 5000.0 - 1e-6,
        "Q should be clamped below the commanded 5 kvar, got {q}"
    );
    assert!(p > 0.0, "P must be positive (charging)");
}

/// kVA clamp boundary: P and Q commanded so that S²−P² limits Q.
#[test]
fn ev_kva_clamp_reduces_q_to_charger_limit() {
    let mut raw = base_raw();
    raw.insert(KEY_CHARGER_CAPACITY_VA.to_string(), 5000.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.2.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_w: 3600.0,
        reactive_power_var: Some(4.0),
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();

    let p = ev.telemetry().get(tk::ACTIVE_POWER_KW).unwrap();
    let q = ev.telemetry().get(tk::REACTIVE_POWER_KVAR).unwrap();

    assert!(p > 0.0, "P must be positive (charging)");
    let s = ev.charger_capacity_va;
    let q_max = (s * s - p * p).max(0.0).sqrt();
    assert!(
        q.abs() <= q_max + 1e-9,
        "|Q|={} exceeds sqrt(S²−P²)={}",
        q.abs(),
        q_max
    );
    assert!(q.abs() < 4000.0 - 1e-6, "Q should be clamped, got {q}");
}

/// Charging (P>0) with pf<1 yields baseline Q>0 (absorbing vars);
/// V2G discharge (P<0) yields baseline Q<0 (supplying vars).
#[test]
fn ev_charging_vs_v2g_discharge_signs() {
    let mut raw = base_raw();
    raw.insert(KEY_POWER_FACTOR.to_string(), 0.9.into());
    raw.insert(KEY_CHARGER_CAPACITY_VA.to_string(), 20000.0.into());
    raw.insert(KEY_V2G_ENABLED.to_string(), true.into());
    raw.insert(KEY_V2G_SOC_RESERVE.to_string(), 0.2.into());
    raw.insert(KEY_V2G_MAX_DISCHARGE_W.to_string(), 5000.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
    let config = ev_config(raw);

    // Charging
    let mut ev_charge = Ev::new(config.clone());
    let env = sample_env();
    ev_charge.init(&config, &env).unwrap();
    ev_charge
        .apply_control(&ControlSignal::PowerSetpoint {
            active_power_w: 3000.0,
            reactive_power_var: None,
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
            active_power_w: -3000.0,
            reactive_power_var: None,
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
    raw.insert(KEY_CHARGER_CAPACITY_VA.to_string(), 20000.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.2.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control(&ControlSignal::ReactiveSetpoint { var: 1.2 })
        .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();

    let q_co = ev
        .core_output()
        .flows
        .reactive_power_var
        .expect("REACTIVE cap → Some");
    let q_telem = ev.telemetry().get(tk::REACTIVE_POWER_KVAR).unwrap();
    let q_port = ports.electrical.reactive_power_var;

    approx_eq(q_co, q_telem);
    approx_eq(q_telem, q_port);
}

/// validate_core_contract passes after a step with reactive power.
#[test]
fn ev_validate_core_contract_passes_with_reactive() {
    let mut raw = base_raw();
    raw.insert(KEY_CHARGER_CAPACITY_VA.to_string(), 20000.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.2.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control(&ControlSignal::ReactiveSetpoint { var: 500.0 })
        .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();

    hares_types::validate_core_contract(ev.descriptor(), ev.core_output(), &mut Vec::new())
        .expect("core contract should pass with REACTIVE cap + Some(Q)");
}

/// Checkpoint round-trip preserves q_setpoint_var and power_factor.
#[test]
fn ev_checkpoint_preserves_reactive_state() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.q_setpoint_var = Some(1.5);
    ev.power_factor = 0.85;

    let state = ev.save_state().unwrap();
    let mut restored = Ev::new(config.clone());
    restored.init(&config, &env).unwrap();
    restored.load_state(&state).unwrap();

    assert_eq!(restored.q_setpoint_var, Some(1.5));
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
    ev.q_setpoint_var = Some(2.0);

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

    let q_telem = ev.telemetry().get(tk::REACTIVE_POWER_KVAR).unwrap();
    approx_eq(q_telem, 0.0);
    let q_co = ev
        .core_output()
        .flows
        .reactive_power_var
        .expect("REACTIVE cap → Some even when 0");
    approx_eq(q_co, 0.0);
    approx_eq(ports.electrical.reactive_power_var, 0.0);
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
    assert_eq!(ports.electrical.reactive_power_var, 0.0);
    assert_eq!(ev.telemetry().get(tk::ACTIVE_POWER_KW), Some(0.0));
    assert!(
        (ev.telemetry().get(tk::SOC).unwrap() - soc_after_baseline).abs() < 1e-12,
        "SOC must not change while the EVSE is dead"
    );

    // Islanded: bus energized by a backup source → charging resumes,
    // clamped to the visible surplus (the on-site source's generation
    // beyond the loads already on the bus): the EVSE rating exceeds it,
    // so the draw is the surplus, not the rating.
    let mut env_islanded = sample_env();
    env_islanded.grid.voltage_pu = 0.0;
    env_islanded.grid.island_bus_voltage_pu = Some(1.0);
    ports.zero();
    ports.electrical.generation_power_w = -5000.0;
    ev.step(&env_islanded, Duration::minutes(15), &mut ports)
        .unwrap();
    assert!(
        ports.electrical.load_power_w > 0.0,
        "islanded bus allows charging from the on-site source"
    );
    assert!(
        (ports.electrical.load_power_w - 5000.0).abs() <= 1e-6,
        "islanded charging clamps to the visible surplus, drew {} W",
        ports.electrical.load_power_w
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
    raw.insert(KEY_V2L_MAX_DISCHARGE_W.to_string(), 3000.0.into());
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
        active_power_w: -2000.0,
        reactive_power_var: None,
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
    raw.insert(KEY_V2L_MAX_DISCHARGE_W.to_string(), 3000.0.into());
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
        active_power_w: -2000.0,
        reactive_power_var: None,
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
    raw.insert(KEY_V2L_MAX_DISCHARGE_W.to_string(), 3000.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_w: -2000.0,
        reactive_power_var: None,
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
    raw.insert(KEY_V2G_MAX_DISCHARGE_W.to_string(), 5000.0.into());
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
        active_power_w: -3000.0,
        reactive_power_var: None,
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
    raw.insert(KEY_V2G_MAX_DISCHARGE_W.to_string(), 5000.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control(&ControlSignal::PowerSetpoint {
        active_power_w: -3000.0,
        reactive_power_var: None,
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
    raw.insert(KEY_V2L_MAX_DISCHARGE_W.to_string(), 3000.0.into());
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
        active_power_w: -2000.0,
        reactive_power_var: None,
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
    raw.insert(KEY_V2L_MAX_DISCHARGE_W.to_string(), 3000.0.into());
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
        active_power_w: -2000.0,
        reactive_power_var: None,
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
/// held constant). Under the Smith (2017) model the raw level stays above
/// nameplate at mid-SOC for years, so the reported fade floors at 0 and the
/// aged pack equals the fresh one here; tests that need a non-zero fade
/// inject one into `degradation.capacity_fade` (the state is crate-visible,
/// the pattern the precondition test uses).
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

/// Unit test for the SOH→capacity feedback: once a capacity fade is in
/// effect, the day-boundary degradation update must keep
/// it (the fade is monotone non-decreasing; the reference model's capacity
/// layer never lets the capacity rise) and the usable pack capacity must
/// equal `rated · (1 − fade)`. Before the fix the EV held
/// `battery_capacity_j` constant at the rated value, so SOC arithmetic
/// ignored degradation entirely.
#[test]
fn daily_degradation_rescales_usable_capacity() {
    let mut ev = aged_ev(30);
    // Inject the fade the day-boundary update must preserve: 10 %, a state
    // a long-aged pack reaches, unreachable in 30 idle days.
    ev.degradation.capacity_fade = 0.10;
    // Cross one midnight at 5-minute steps so update_degradation's
    // day-boundary update runs on the injected state.
    let dt = Duration::from_secs(300);
    let mut env = sample_env();
    for _ in 0..288 {
        let mut ports = PortSlots::default();
        ev.step(&env, dt, &mut ports).unwrap();
        env.current_time += ChronoDuration::seconds(300);
    }

    let fade = ev.degradation.capacity_fade_fraction();
    assert!(
        (fade - 0.10).abs() < 1e-9,
        "the day-boundary update must preserve the injected fade (the fade \
         is monotone non-decreasing), got {fade}"
    );

    let expected = ev.battery_capacity_j_rated * (1.0 - fade) * capacity_derate_at(25.0);
    assert!(
        (ev.battery_capacity_j - expected).abs() < 1e-6,
        "usable capacity {} must equal rated·(1−fade)·derate = {expected}",
        ev.battery_capacity_j
    );
    // The runtime divisor must differ from the rated value — otherwise SOC
    // arithmetic would still be using the undegraded capacity (the bug).
    assert!(
        (ev.battery_capacity_j - ev.battery_capacity_j_rated).abs() > 1e-6,
        "aged usable capacity {} must differ from rated {}",
        ev.battery_capacity_j,
        ev.battery_capacity_j
    );
}

/// Integration test for the runtime effect (T-0421 directive core): a
/// fixed-energy drive moves SOC by `energy / usable_capacity`, i.e. it scales
/// as `1/SOH` relative to a fresh pack. Before the fix a fixed drive always
/// moved SOC by `energy / rated`, independent of the pack's aged state.
#[test]
fn fixed_drive_soc_swing_scales_with_degraded_capacity() {
    let drive_j = 6.0 * 3.6e6; // 6 kWh, in J (the signal's unit)

    // Fresh pack: SOH = 1, usable capacity = rated × derate (the packs sit
    // at the aged_ev fixture's pinned 25 °C).
    let mut fresh = aged_ev(0);
    fresh.soc = 0.9;
    let rated = fresh.battery_capacity_j_rated;
    let cap_fresh = fresh.battery_capacity_j;
    assert!((cap_fresh - rated * capacity_derate_at(25.0)).abs() < 1.0);
    let soc_before_fresh = fresh.soc;
    fresh
        .apply_control_unchecked(&ControlSignal::EvDrive { energy_j: drive_j })
        .unwrap();
    let swing_fresh = soc_before_fresh - fresh.soc;

    // Aged pack: SOH ≠ 1, usable capacity = rated·SOH·derate. The fade is
    // injected (10 %, a state a long-aged pack reaches; 40 idle days at
    // mid-SOC still floor at the nameplate state) and applied through the
    // same refresh the day-boundary update uses.
    let mut aged = aged_ev(40);
    aged.degradation.capacity_fade = 0.10;
    aged.refresh_usable_capacity();
    aged.soc = 0.9;
    let cap_aged = aged.battery_capacity_j;
    let soh = 1.0 - aged.degradation.capacity_fade_fraction();
    let soc_before_aged = aged.soc;
    aged.apply_control_unchecked(&ControlSignal::EvDrive { energy_j: drive_j })
        .unwrap();
    let swing_aged = soc_before_aged - aged.soc;

    // Each swing equals energy / usable_capacity (both J now).
    assert!(
        (swing_fresh - drive_j / cap_fresh).abs() < 1e-9,
        "fresh swing {swing_fresh} should equal energy/fresh-usable-capacity"
    );
    assert!(
        (swing_aged - drive_j / cap_aged).abs() < 1e-9,
        "aged swing {swing_aged} should equal energy/aged-usable-capacity"
    );

    // The scaling law: both packs share the pinned temperature, so the
    // derate cancels and the swing ratio equals 1/SOH exactly.
    let swing_ratio = swing_aged / swing_fresh;
    assert!(
        (swing_ratio - cap_fresh / cap_aged).abs() < 1e-9,
        "swing ratio {swing_ratio} must match the capacity ratio"
    );
    assert!(
        (swing_ratio - 1.0 / soh).abs() < 1e-9,
        "swing ratio {swing_ratio} must equal 1/SOH = {} (the shared \
         temperature derate cancels)",
        1.0 / soh
    );

    // Degradation must actually change the runtime SOC dynamics: the aged swing
    // differs measurably from what the rated (undegraded) divisor would give.
    assert!(
        (swing_aged - drive_j * 3.6e6 / rated).abs() > 1e-4,
        "aged swing {swing_aged} must differ from the buggy rated-based swing {}",
        drive_j * 3.6e6 / rated
    );
}

/// Checkpoint restore must recompute the degraded usable capacity from the
/// rated capacity (set by `init`) and the restored SOH — mirroring
/// `Battery::load_state`. `battery_capacity_kwh_rated` is not serialized; it is
/// re-established by `init` before `load_state`. The source pack carries an
/// injected 10 % fade (a state a long-aged pack reaches); the restore must
/// reproduce its capacity from the restored fade alone.
#[test]
fn load_state_recomputes_degraded_capacity_from_rated_and_soh() {
    let mut source = aged_ev(40);
    source.degradation.capacity_fade = 0.10;
    source.refresh_usable_capacity();
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
        (restored.battery_capacity_j - source.battery_capacity_j).abs() < 1e-12,
        "rated capacity must survive init across restore"
    );
    assert!(
        (restored.battery_capacity_j - source.battery_capacity_j).abs() < 1e-9,
        "restored usable capacity {} must match source {}",
        restored.battery_capacity_j,
        source.battery_capacity_j
    );
    let fade = restored.degradation.capacity_fade_fraction();
    assert!(
        (restored.battery_capacity_j
            - restored.battery_capacity_j_rated * (1.0 - fade) * capacity_derate_at(25.0))
        .abs()
            < 1e-6,
        "restored usable capacity must equal rated·(1−fade)·derate (the \
         restored 25 °C pack temperature)"
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
            .apply_control_unchecked(&ControlSignal::EvAwayCharge { power_w: bad })
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
            active_power_w: f64::NAN,
            reactive_power_var: None,
            min_soc: None,
            max_soc: None,
        })
        .expect_err("non-finite active setpoint must be rejected");
    assert!(
        format!("{err:?}").contains("active_power_w"),
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
                active_power_w: bad,
                reactive_power_var: None,
                min_soc: None,
                max_soc: None,
            })
            .expect_err("non-finite active setpoint must be rejected when v2g is enabled");
        assert!(
            format!("{err:?}").contains("active_power_w"),
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
            active_power_w: 1000.0,
            reactive_power_var: None,
            min_soc: Some(f64::NAN),
            max_soc: None,
        },
        ControlSignal::PowerSetpoint {
            active_power_w: 1000.0,
            reactive_power_var: None,
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
            active_power_w: 1000.0,
            reactive_power_var: None,
            min_soc: Some(f64::NAN),
            max_soc: None,
        },
        ControlSignal::PowerSetpoint {
            active_power_w: 1000.0,
            reactive_power_var: None,
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
            active_power_w: 1000.0,
            reactive_power_var: None,
            min_soc: Some(7.5),
            max_soc: None,
        },
        ControlSignal::PowerSetpoint {
            active_power_w: 1000.0,
            reactive_power_var: None,
            min_soc: Some(0.8),
            max_soc: Some(0.2),
        },
        ControlSignal::PowerSetpoint {
            active_power_w: 1000.0,
            reactive_power_var: None,
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

/// The driver's seeded capacity is a temperature-independent rating, not
/// the live usable capacity. `refresh_usable_capacity` scales
/// `battery_capacity_j` by the reversible cold-capacity derate each step
/// with the pack's *current* temperature; seeding the actor with that
/// per-step value freezes the init-time ambient into the driver's
/// anxiety/needed-hours/recoup arithmetic for the whole run and makes the
/// seed depend on the weather at init. The degradation-adjusted
/// (temperature-independent) capacity is the value the actor's own docs
/// describe (`observed_pack_j`: "this actor's rated capacity").
#[test]
fn actor_seed_capacity_is_independent_of_init_weather() {
    let config = ev_config(base_raw());
    let mut warm = Ev::new(config.clone());
    warm.init(&config, &sample_env()).unwrap();

    let mut cold_env = sample_env();
    cold_env.weather.outdoor_temp_c = -7.0;
    let mut cold = Ev::new(config.clone());
    cold.init(&config, &cold_env).unwrap();

    let seed_j = |ev: &Ev| match ev.actor_seed() {
        Some(crate::ActorSeed::Ev { capacity_j, .. }) => capacity_j,
        other => panic!("expected ActorSeed::Ev, got {other:?}"),
    };
    let warm_j = seed_j(&warm);
    let cold_j = seed_j(&cold);
    assert!(
        (warm_j - cold_j).abs() < 1e-9,
        "the driver's seeded capacity must be the temperature-independent \
         degradation-adjusted rating, not the live usable capacity: init at \
         10 C seeded {warm_j} kWh but init at -7 C seeded {cold_j} kWh"
    );
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
            min_charge_rate_w: 1400.0,
            departure_schedule: vec![],
        },
        hares_types::ChargingStrategy::V2H {
            discharge_threshold_soc: 0.8,
            min_soc: 0.3,
        },
        hares_types::ChargingStrategy::V2G {
            min_soc: 0.3,
            max_export_w: 7200.0,
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
    // The raw payload path is gone: whatever the payload carries, the
    // EV's init refuses it (a typed config is the only way in), so no
    // invalid value can slip through as a silent default either.
    let mut raw = HashMap::new();
    raw.insert(KEY_CAPACITY_J.to_string(), 216000000.0.into());
    raw.insert(KEY_MAX_CHARGING_POWER_W.to_string(), 7200.0.into());
    let config = EquipmentConfig::raw("test_ev".to_string(), "EV".to_string(), raw);
    let mut ev = Ev::new(config.clone());
    let err = ev
        .init(&config, &sample_env())
        .expect_err("a raw EV payload is refused at init");
    assert!(
        format!("{err:?}").contains("typed config"),
        "the refusal names the typed-config requirement, got {err:?}"
    );
}

/// An unknown `charging_priority` raw value must defer an error rather than
/// silently becoming `DeadlineGuarantee`.
#[test]
fn raw_config_unknown_charging_priority_defers_error_to_init() {
    // The raw payload path is gone: the refusal is the raw config's own,
    // whatever its values, and a typed config's charging priority is the
    // typed enum.
    let mut raw = HashMap::new();
    raw.insert(KEY_CAPACITY_J.to_string(), 216000000.0.into());
    raw.insert(KEY_MAX_CHARGING_POWER_W.to_string(), 7200.0.into());
    let config = EquipmentConfig::raw("test_ev".to_string(), "EV".to_string(), raw);
    let mut ev = Ev::new(config.clone());
    let err = ev
        .init(&config, &sample_env())
        .expect_err("a raw EV payload is refused at init");
    assert!(
        format!("{err:?}").contains("typed config"),
        "the refusal names the typed-config requirement, got {err:?}"
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

/// A checkpoint taken while the EV stands in var support (`On` mode: SOC at
/// target, heater off, a nonzero reactive command served at zero real
/// power) must restore a core output that satisfies the workspace's own
/// mode/flow guard. `load_state` restores the step-outcome `last_mode`
/// (checkpoint v5) but zeroes `reactive_power_var` as a step outcome, so
/// the rebuilt output pairs an active mode with all-zero flows — exactly
/// the pair `validate_core_contract`'s Rule 1 treats as a hard error.
/// Pre-fix, the restored mode was re-derived from the restored port power
/// and could never disagree with its own flows; restore-time consistency
/// is a contract of the state that exists between `load_state` and the
/// first post-restore step (observer snapshots, fleet restart tooling,
/// and `update_control`'s report all read it there).
#[test]
fn var_support_checkpoint_restores_an_output_the_mode_flow_guard_accepts() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 1.0.into());
    let config = ev_config(raw);
    let env = sample_env();
    let mut ev = Ev::new(config.clone());
    ev.init(&config, &env).unwrap();

    // SOC at target → zero charge demand; 10 °C pack → heater off. The
    // only live flow is the commanded reactive power: the standby
    // var-support state, mode `On`.
    ev.apply_control(&ControlSignal::ReactiveSetpoint { var: 1500.0 })
        .unwrap();
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();
    assert_eq!(
        ev.core_output().state.operating_mode,
        Some(OperatingMode::On),
        "precondition: the saved state is the standby var-support mode"
    );

    let state = ev.save_state().unwrap();
    let mut restored = Ev::new(config.clone());
    restored.init(&config, &env).unwrap();
    restored.load_state(&state).unwrap();

    hares_types::validate_core_contract(
        restored.descriptor(),
        restored.core_output(),
        &mut Vec::new(),
    )
    .expect(
        "a restored core output must satisfy the mode/flow guard: the v5 \
              restore re-pairs the step-outcome `On` mode with zeroed flows, \
              which Rule 1 rejects as an active mode with no flow",
    );
}

/// The charging-curve LUT's c-rate input must see the pack's SOH scaling
/// exactly once. `compute_charging_power_w` derives
/// `effective_j = battery_capacity_j · soh`, but `refresh_usable_capacity`
/// has already folded the SOH into `battery_capacity_j` (usable =
/// rated · SOH · temperature derate), so an aged pack's c-rate is computed
/// over the SOH-squared capacity — the degradation factor applied twice,
/// skewing every configured LUT's c-rate axis (the second application grew
/// when this fix layered the temperature derate into the same field's
/// meaning without re-deriving this consumer).
#[test]
fn lut_c_rate_axis_receives_the_soh_scaled_capacity_once() {
    // The c-rate axis steps between the correct single-SOH c-rate
    // (7.2 kW / (60 kWh · 0.9 · derate(25 °C)) = 0.1332 — full power) and
    // the double-SOH c-rate (7.2 kW / (60 · 0.9² · derate(25 °C)) = 0.1480
    // — zero power), so the two derivations are unambiguously
    // distinguishable in the delivered power.
    let lut = crate::ndinterp::RegularGridInterpolator::new(
        vec![
            vec![0.0, 1.0],    // soc (flat)
            vec![25.0],        // temperature (flat at the derate reference)
            vec![0.14, 0.145], // c-rate
            vec![1.0],         // soh (flat)
        ],
        vec![1.0f32, 0.0, 1.0, 0.0],
        crate::ndinterp::ExtrapolationStrategy::Clamp,
    )
    .unwrap();

    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.2.into());
    raw.insert(KEY_BATTERY_TEMP_C.to_string(), 25.0.into());
    raw.insert(KEY_HEATER_POWER_W.to_string(), 0.0.into());
    raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();
    ev.set_charging_curve_lut(Some(lut)).unwrap();
    // 10 % fade, injected directly: the degradation state is otherwise
    // fresh, and no multi-year simulation is needed to age the pack.
    ev.degradation.capacity_fade = 0.10;

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

    let capacity = ev.telemetry().get(tk::CAPACITY_KWH).unwrap();
    // The expected capacity is derived from the pack's ACTUAL post-step
    // temperature (published in the same telemetry): the fix under test
    // delivers full charge power, whose I²R heat warms the pack ~0.012 K
    // over the step, and the usable capacity is temperature-dependent —
    // hardcoding the 25 °C step-start temperature here would fail by
    // ~3.7e-3 kWh under the CORRECT behavior (it only held pre-fix because
    // the defect zeroed the power, so no current flowed and no heat was
    // produced). Deriving from the actual temperature keeps the
    // precondition's purpose — the injected fade must be in effect — at
    // the same 1e-9 tolerance.
    let expected_capacity =
        60.0 * 3.6e6 * 0.9 * capacity_derate_at(ev.telemetry().get(tk::BATTERY_TEMP_C).unwrap());
    assert!(
        (capacity - expected_capacity).abs() < 1e-3,
        "precondition: the pack must carry the injected fade (usable \
         capacity {expected_capacity} J), got {capacity}"
    );

    let power_w = ev
        .telemetry()
        .get("active_power_kw")
        .expect("active power telemetry after a step");
    assert!(
        (power_w - 7200.0).abs() < 1e-3,
        "the LUT's c-rate axis must see the SOH scaling exactly once \
         (c_rate = 7200 W / {expected_capacity:.3} J × 3600 ≈ 0.1332 per \
         hour, inside the LUT's full-power band): expected 7200 W, got \
         {power_w} — the c-rate was computed over the SOH-squared capacity, \
         applying the degradation factor twice"
    );
}

/// The charging-LUT c-rate divisor is the degradation-adjusted rating, and
/// temperature enters the lookup only through the LUT's own temperature
/// axis — never through the divisor. `refresh_usable_capacity` scales
/// `battery_capacity_j` by the reversible cold-capacity derate every step,
/// so dividing by it (the pre-alignment behavior) would apply the
/// temperature twice through two channels — the c-rate axis and the LUT's
/// temperature axis — inflating the c-rate ≈1.3× at 0 °C (≈1.5× at −7 °C)
/// and bin-shifting every configured lookup (the same
/// one-physical-effect-applied-once rule the sibling Battery's LUT follows
/// through the same shared `pack_electrical::charging_lut_c_rate` home).
#[test]
fn lut_c_rate_divisor_is_temperature_independent() {
    // Bands chosen mid-band for both derivations so neither lands on a grid
    // boundary: 7.2 kW over the 60 kWh rating is c_rate = 0.12 (mid-band →
    // half power = 3.6 kW); over the temperature-derated usable capacity
    // (60 × derate(5 °C) ≈ 49.8 kWh) it is ≈0.145 (above the 0.14 axis end →
    // zero power).
    let lut = crate::ndinterp::RegularGridInterpolator::new(
        vec![vec![0.0, 1.0], vec![5.0], vec![0.10, 0.14], vec![1.0]],
        vec![1.0f32, 0.0, 1.0, 0.0],
        crate::ndinterp::ExtrapolationStrategy::Clamp,
    )
    .unwrap();

    let mut raw = base_raw();
    raw.insert(KEY_BATTERY_TEMP_C.to_string(), 5.0.into());
    raw.insert(KEY_HEATER_POWER_W.to_string(), 0.0.into());
    raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();
    ev.set_charging_curve_lut(Some(lut)).unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

    let power_w = ev
        .telemetry()
        .get("active_power_kw")
        .expect("active power telemetry after a step");
    assert!(
        (power_w - 3600.0).abs() < 1e-3,
        "the LUT's c-rate axis must see the temperature-independent rating \
         (c_rate = 7200/60e6×3600 = 0.12 per hour, mid-band → half power = \
         3.6 kW): got {power_w} kW — the divisor was the temperature-derated usable \
         capacity (c_rate ≈ 0.145 → zero power), applying the reversible \
         derate twice through two channels"
    );
}

/// A checkpoint taken mid-preconditioning must restore the reactive flow
/// the step published. The step keys the reactive computation on the
/// *inverter leg* — the charge/export conversion only — because the pack
/// heater is a DC-fed resistive load that is not inverter-coupled: it
/// produces no vars and consumes no kVA headroom (the same rule the
/// stationary Battery's step documents). The restore instead recomputes
/// `compute_reactive_var(self.active_power_w)` over the *port total* —
/// the heater-inclusive basis the step's own rule forbids — so an idle
/// preconditioning checkpoint (inverter leg exactly zero) that published
/// no vars restores 2.7 kVAR of vars from nothing at the default 5 kW
/// heater and pf 0.9.
#[test]
fn heater_active_checkpoint_restores_the_reactive_flow_the_step_published() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 1.0.into());
    // Explicit pack temperature wins the init cascade: 3 °C is above the
    // plating cutoff (charging legal) and below the 5 °C heater threshold
    // (preconditioning active) with SOC at target — the idle-preconditioning
    // state.
    raw.insert(KEY_BATTERY_TEMP_C.to_string(), 3.0.into());
    raw.insert(KEY_POWER_FACTOR.to_string(), 0.9.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    assert_eq!(
        ev.core_output().state.operating_mode,
        Some(OperatingMode::Heating),
        "precondition: the saved state is mid-preconditioning"
    );
    let q_saved = ev.telemetry().get(tk::REACTIVE_POWER_KVAR).unwrap();
    assert_eq!(
        q_saved, 0.0,
        "precondition: the step publishes no vars while preconditioning — \
         the inverter leg is zero and the DC-fed heater produces none"
    );

    let state = ev.save_state().unwrap();
    let mut restored = Ev::new(config.clone());
    restored.init(&config, &env).unwrap();
    restored.load_state(&state).unwrap();

    let q_restored = restored.telemetry().get(tk::REACTIVE_POWER_KVAR).unwrap();
    approx_eq(q_restored, q_saved);
}

/// The heater-draw face of the same restore-fidelity contract the v6
/// reactive fix established: the leg/heater split of the port total is
/// transient step state that is not derivable at restore — the identical
/// underivable-basis argument the v6 comment makes for the reactive flow —
/// so the step-published heater draw must be restored verbatim too, not
/// zeroed. The restore currently zeroes `heater_draw_w` while restoring
/// the heater-funded port power (`active_power_w`), the `Heating` mode,
/// and the verbatim reactive flow — publishing a state no live step can
/// produce: `classify_mode` yields `Heating` only when `heater_draw_w > 0`,
/// so a live step always pairs the mode with a real draw, and its billing
/// rule always decomposes the port as charge leg + heater AC-equivalent.
/// The restored pair (mode `Heating`, port 5.56 kW, heater column 0 W,
/// pack netting zero) is heater-funded port power with the heater column
/// at zero — the same published-state infidelity the reactive fix removed
/// for the q column.
#[test]
fn heater_active_checkpoint_restores_the_heater_draw_the_step_published() {
    // The idle-preconditioning state: SOC at target (no charge demand),
    // 3 °C pack (above the plating cutoff, below the 5 °C heater
    // threshold), default 5 kW heater.
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 1.0.into());
    raw.insert(KEY_BATTERY_TEMP_C.to_string(), 3.0.into());
    raw.insert(KEY_POWER_FACTOR.to_string(), 0.9.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    assert_eq!(
        ev.core_output().state.operating_mode,
        Some(OperatingMode::Heating),
        "precondition: the saved state is mid-preconditioning"
    );
    let port_saved = ev.telemetry().get(tk::ACTIVE_POWER_KW).unwrap();
    let heater_saved = ev.telemetry().get(tk::HEATER_POWER_W).unwrap();
    assert!(
        (heater_saved - 5000.0).abs() < 1e-6 && port_saved > 5.0,
        "precondition: the step publishes the heater's true draw (5000 W) \
         funding the port total, got heater {heater_saved} W, port \
         {port_saved} kW"
    );

    let state = ev.save_state().unwrap();
    let mut restored = Ev::new(config.clone());
    restored.init(&config, &env).unwrap();
    restored.load_state(&state).unwrap();

    let heater_restored = restored.telemetry().get(tk::HEATER_POWER_W).unwrap();
    assert_eq!(
        heater_restored, heater_saved,
        "a heater-active checkpoint must restore the step-published heater \
         draw: the leg/heater split of the restored port power is not \
         derivable at restore (the same underivable-basis argument the v6 \
         reactive restore makes), and zeroing it pairs the `Heating` mode — \
         which `classify_mode` only ever produces with a nonzero draw — \
         with a zero heater column"
    );
}

/// The away-intake face of the same restore-fidelity family: the away arm's
/// actual charge intake (`away_charge_actual_w`, published as
/// `AWAY_CHARGE_POWER_KW` — the documented way away charging is observed)
/// is a step-computed value whose basis (the away charge leg) is not
/// derivable from the checkpoint — `active_power_w` is 0 in the away
/// state, and nothing else checkpointed reconstructs the intake. Its
/// discharge-side sibling (`v2l_power_w`) is checkpointed verbatim (v8)
/// for exactly this reason; the away intake is not, so a checkpoint saved
/// mid-away-session restores publishing 0 kW of intake. The same
/// underivable-basis criterion the v7/v8 fixes applied to the heater draw
/// and the V2L dispatch state.
#[test]
fn away_charging_checkpoint_restores_the_intake_the_step_published() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    // Away, charger commanded at the 7.2 kW rating, SOC below target: the
    // away session charges at the full rating (pack at the 10 °C ambient
    // init → derate ramp fully open). The connection state machine
    // requires the transition to run through Disconnected.
    ev.apply_control(&ControlSignal::EvPlugIn {
        state: EvConnectionState::Disconnected,
    })
    .unwrap();
    ev.apply_control(&ControlSignal::EvPlugIn {
        state: EvConnectionState::AwayPluggedIn,
    })
    .unwrap();
    ev.apply_control(&ControlSignal::EvAwayCharge { power_w: 7200.0 })
        .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    let intake_saved = ev.telemetry().get(tk::AWAY_CHARGE_POWER_KW).unwrap();
    assert!(
        (intake_saved - 7200.0).abs() < 1e-3,
        "precondition: the step publishes the away session's actual intake \
         (7.2 kW), got {intake_saved}"
    );

    let state = ev.save_state().unwrap();
    let mut restored = Ev::new(config.clone());
    restored.init(&config, &env).unwrap();
    restored.load_state(&state).unwrap();

    let intake_restored = restored.telemetry().get(tk::AWAY_CHARGE_POWER_KW).unwrap();
    assert_eq!(
        intake_restored, intake_saved,
        "an away-charging checkpoint must restore the step-published intake: \
         its basis (the away charge leg) is not derivable from the \
         checkpoint — the same underivable-basis criterion the v7/v8 fixes \
         applied to the heater draw and the V2L dispatch state — and the \
         zero it currently publishes misreports an active away session"
    );
}

/// A checkpoint restart that steps across a midnight boundary must be
/// transparent: the restored day anchor (`last_daily_update_day`), the
/// restored rainflow history, and the restored degradation accumulators
/// must drive `update_degradation`'s day-boundary update exactly as the
/// uninterrupted run's own state does — one boundary, fed the pre-restart
/// day's SOC history. Twin equivalence: two EVs stepped in lockstep across
/// midnight, one checkpoint-restored between the steps, must end
/// byte-identical or the restart changed the simulation.
#[test]
fn checkpoint_restart_across_a_day_boundary_matches_the_uninterrupted_twin() {
    let config = ev_config(base_raw());
    let mut env_pre = sample_env();
    env_pre.current_time = dt(2026, 1, 1, 23, 0, 0);
    let mut env_post = sample_env();
    env_post.current_time = dt(2026, 1, 2, 0, 0, 0);

    // Twin A: the uninterrupted run — one pre-midnight step, one
    // post-midnight step (the second fires the day boundary).
    let mut twin_a = Ev::new(config.clone());
    twin_a.init(&config, &env_pre).unwrap();
    let mut ports = PortSlots::default();
    twin_a
        .step(&env_pre, Duration::minutes(15), &mut ports)
        .unwrap();
    let mut ports = PortSlots::default();
    twin_a
        .step(&env_post, Duration::minutes(15), &mut ports)
        .unwrap();

    // Twin B: checkpoint-restart between the same two steps.
    let mut twin_b = Ev::new(config.clone());
    twin_b.init(&config, &env_pre).unwrap();
    let mut ports = PortSlots::default();
    twin_b
        .step(&env_pre, Duration::minutes(15), &mut ports)
        .unwrap();
    let saved = twin_b.save_state().unwrap();
    let mut restored = Ev::new(config.clone());
    restored.init(&config, &env_pre).unwrap();
    restored.load_state(&saved).unwrap();
    let mut ports = PortSlots::default();
    restored
        .step(&env_post, Duration::minutes(15), &mut ports)
        .unwrap();

    assert_eq!(
        restored.save_state().unwrap(),
        twin_a.save_state().unwrap(),
        "a checkpoint restart across a day boundary must be transparent: \
         the restored day anchor and the restored rainflow/degradation \
         state must drive the midnight boundary exactly as the \
         uninterrupted run's own state does — a diverging checkpoint means \
         the restore changed the simulation"
    );
}

/// The discharge face of the same restore-fidelity family: a checkpoint
/// taken mid-V2L-discharge must restore the island-source availability
/// the step published. `island_source_available` reads `v2l_active`, so
/// a restore that leaves it false (the pre-derivation placeholder)
/// reports no island source for the whole restore-to-first-step window —
/// a dwelling mid-outage would conclude its backup vehicle is not a
/// source exactly when it is. The restore now derives the pair losslessly
/// from already-checkpointed state (`v2l_active ⟺ last_mode ==
/// Discharging`, the export being the only negative-net path; and
/// `v2l_power_w = −active_power_w`, the port carrying the export
/// alone) — this gate pins that derivation: no schema change to guard,
/// just the restored-state-matches-saved-state contract.
#[test]
fn mid_discharge_checkpoint_restores_island_source_availability() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
    raw.insert(KEY_V2L_ENABLED.to_string(), true.into());
    raw.insert(KEY_V2L_SOC_RESERVE.to_string(), 0.2.into());
    raw.insert(KEY_V2L_MAX_DISCHARGE_W.to_string(), 3000.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();
    ev.apply_control_unchecked(&ControlSignal::PowerSetpoint {
        active_power_w: -3000.0,
        reactive_power_var: None,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    assert_eq!(
        ev.core_output().state.operating_mode,
        Some(OperatingMode::Discharging),
        "precondition: the saved state is mid-discharge"
    );
    assert!(
        ev.island_source_available(),
        "precondition: the discharging step reports island-source availability"
    );
    let v2l_power_saved = ev.telemetry().get(tk::V2L_POWER_KW).unwrap();
    assert!(
        v2l_power_saved > 0.0,
        "the step publishes the export magnitude"
    );

    let state = ev.save_state().unwrap();
    let mut restored = Ev::new(config.clone());
    restored.init(&config, &env).unwrap();
    restored.load_state(&state).unwrap();

    assert_eq!(
        restored.core_output().state.operating_mode,
        Some(OperatingMode::Discharging),
        "the restored mode is the checkpointed discharge mode"
    );
    assert!(
        restored.island_source_available(),
        "a mid-discharge checkpoint must restore island-source availability — \
         the dwelling's islanding reads it, and a restore window that \
         reports no source is a restored state no live step produced"
    );
    assert_eq!(
        restored.telemetry().get(tk::V2L_ACTIVE).unwrap(),
        1.0,
        "the V2L_ACTIVE telemetry must publish the derived state"
    );
    let v2l_power_restored = restored.telemetry().get(tk::V2L_POWER_KW).unwrap();
    assert_eq!(
        v2l_power_restored, v2l_power_saved,
        "the restored export magnitude is −active_power_w (the port carries \
         the export alone), matching the step-published value"
    );
}

/// The restore-fidelity contract's floor-held face: a discharge stays
/// dispatched (negative setpoint latched, V2L enabled) with the pack at
/// the reserve floor. `compute_discharge` returns (0, 0) at the floor,
/// but the step still publishes `v2l_active = true` — the field keys on
/// the *dispatch* (`is_discharge`, set whenever the discharge leg
/// exists), not the exported power — alongside a zero net rate and
/// therefore `last_mode = Off`. The restore derives `v2l_active` from
/// `last_mode == Discharging`, which is false here: the restored state
/// diverges from the saved one, and `island_source_available` (which
/// reads `v2l_active`) flips false for the restore window. Same family
/// as the F4/F5 restore infidelities — a derivation that approximates
/// rather than reproduces the step-published state; the faithful fix
/// mirrors the v6/v7 pattern (checkpoint the published values verbatim),
/// since the export flow cannot disambiguate this corner either (it is
/// exactly zero). The test pins fidelity, not the mechanism.
#[test]
fn floor_held_discharge_checkpoint_restores_the_published_v2l_state() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.2.into());
    raw.insert(KEY_V2L_ENABLED.to_string(), true.into());
    // SOC exactly at the reserve: the floor-held discharge corner.
    raw.insert(KEY_V2L_SOC_RESERVE.to_string(), 0.2.into());
    raw.insert(KEY_V2L_MAX_DISCHARGE_W.to_string(), 3000.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();
    ev.apply_control_unchecked(&ControlSignal::PowerSetpoint {
        active_power_w: -3000.0,
        reactive_power_var: None,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    assert_eq!(
        ev.telemetry().get(tk::V2L_ACTIVE).unwrap(),
        1.0,
        "precondition: the floor-held step still publishes the dispatch-active state"
    );
    assert_eq!(
        ev.telemetry().get(tk::V2L_POWER_KW).unwrap(),
        0.0,
        "precondition: at the floor the export is exactly zero"
    );
    assert!(
        !ev.island_source_available(),
        "precondition: the floor-held corner publishes the dispatch-active state \
         with zero delivered watts, and the island source is therefore unavailable"
    );

    let state = ev.save_state().unwrap();
    let mut restored = Ev::new(config.clone());
    restored.init(&config, &env).unwrap();
    restored.load_state(&state).unwrap();

    assert_eq!(
        restored.telemetry().get(tk::V2L_ACTIVE).unwrap(),
        ev.telemetry().get(tk::V2L_ACTIVE).unwrap(),
        "the restore must reproduce the step-published V2L_ACTIVE state"
    );
    assert_eq!(
        restored.island_source_available(),
        ev.island_source_available(),
        "island-source availability must survive the checkpoint round-trip"
    );
}

/// The charging-with-heater face of the same restore-basis defect: at a
/// bound-binding cold session the port equals the kVA rating, so the
/// restore's port-total basis leaves zero kVA headroom and a commanded
/// q-setpoint the step served in full restores clamped to zero — the
/// commanded var support vanishes across the checkpoint round-trip.
#[test]
fn commanded_vars_served_at_a_bound_binding_checkpoint_survive_restore() {
    let mut raw = base_raw();
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
    raw.insert(KEY_BATTERY_TEMP_C.to_string(), 3.0.into());
    raw.insert(KEY_POWER_FACTOR.to_string(), 0.9.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();
    ev.apply_control(&ControlSignal::ReactiveSetpoint { var: 6500.0 })
        .unwrap();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    // Cold-derated charge demand 7.2·0.3 = 2.16 kW; heater AC-equivalent
    // 5.56 kW; both fit inside the 7.2 kVA rating's port bound = 7.2 kW.
    let q_saved = ev.telemetry().get(tk::REACTIVE_POWER_KVAR).unwrap();
    assert!(
        (q_saved - 6500.0).abs() < 1e-3,
        "precondition: the step serves the commanded 6.5 kVAR in full (the \
         inverter-leg basis leaves headroom), got {q_saved}"
    );

    let state = ev.save_state().unwrap();
    let mut restored = Ev::new(config.clone());
    restored.init(&config, &env).unwrap();
    restored.load_state(&state).unwrap();

    let q_restored = restored.telemetry().get(tk::REACTIVE_POWER_KVAR).unwrap();
    approx_eq(q_restored, q_saved);
}

/// The Ready-By + over-rated PowerSetpoint divergence reaches the warning
/// channel once per run: one drained warning over three violating steps, and
/// the warn-once flag resets on init.
#[test]
fn ev_setpoint_above_rated_warns_once_per_run() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();

    ev.apply_control_unchecked(&ControlSignal::EvSetReadyBy {
        departure_hour: 12.0,
        target_soc: 0.9,
    })
    .unwrap();

    let mut drained: Vec<hares_types::Warning> = Vec::new();
    for _ in 0..3 {
        ev.apply_control_unchecked(&ControlSignal::PowerSetpoint {
            active_power_w: 20000.0,
            reactive_power_var: None,
            min_soc: None,
            max_soc: None,
        })
        .unwrap();
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
        ev.drain_warnings(&mut drained);
    }
    assert_eq!(drained.len(), 1, "warn once per run, got {drained:?}");
    assert!(
        drained[0].message.contains("rated power"),
        "the warning names the divergence: {}",
        drained[0].message
    );

    // Re-init resets the warn-once flag: the next violating step warns again.
    ev.init(&config, &env).unwrap();
    ev.apply_control_unchecked(&ControlSignal::EvSetReadyBy {
        departure_hour: 12.0,
        target_soc: 0.9,
    })
    .unwrap();
    ev.apply_control_unchecked(&ControlSignal::PowerSetpoint {
        active_power_w: 20000.0,
        reactive_power_var: None,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(15), &mut ports).unwrap();
    let mut again: Vec<hares_types::Warning> = Vec::new();
    ev.drain_warnings(&mut again);
    assert_eq!(
        again.len(),
        1,
        "a re-initialized EV warns once per run again"
    );
}

#[test]
fn dbg_publish_value() {
    let config = ev_config(base_raw());
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(60), &mut ports).unwrap();
    eprintln!(
        "DBG active_power_w={} electrical_port={}",
        ev.telemetry().get("active_power_kw").unwrap(),
        ports.electrical.load_power_w
    );
    eprintln!("DBG p only");
}

// -- island source availability and the islanded charge clamp --

/// The outage environment with an island-formed bus: a utility outage
/// (voltage 0.0) with the dwelling's island resolution holding the bus at
/// nominal, the state `island_source_available` feeds and the clamp
/// keys on.
fn outage_island_env() -> EnvironmentState {
    let mut env = sample_env();
    env.grid = GridState {
        voltage_pu: 0.0,
        frequency_hz: 60.0,
        island_bus_voltage_pu: Some(1.0),
    };
    env
}

fn v2l_ev_at_reserve() -> Ev {
    let mut raw = base_raw();
    raw.insert(KEY_V2L_ENABLED.to_string(), true.into());
    raw.insert(KEY_V2L_SOC_RESERVE.to_string(), 0.2.into());
    raw.insert(KEY_V2L_MAX_DISCHARGE_W.to_string(), 3000.0.into());
    raw.insert(KEY_INITIAL_SOC.to_string(), 0.2.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();
    ev.apply_control_unchecked(&ControlSignal::PowerSetpoint {
        active_power_w: -3000.0,
        reactive_power_var: None,
        min_soc: None,
        max_soc: None,
    })
    .unwrap();
    ev
}

/// Availability keys on delivered watts, not the commanded flag: at the
/// reserve the floor curtails the latched export to zero and the EV is no
/// island source; above the floor the same command delivers and is.
#[test]
fn island_availability_requires_delivered_export() {
    let mut ev = v2l_ev_at_reserve();
    let env = sample_env();

    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(1), &mut ports).unwrap();
    assert!(ev.v2l_active, "the discharge is commanded");
    assert_eq!(
        ev.v2l_power_w, 0.0,
        "the floor curtails the export to zero watts"
    );
    assert!(
        !ev.island_source_available(),
        "a commanded discharge that delivered nothing must not island the home"
    );

    ev.soc = 0.5;
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(1), &mut ports).unwrap();
    assert!(
        ev.v2l_power_w > 0.0,
        "the same command above the floor delivers export"
    );
    assert!(ev.island_source_available());
}

/// A DR full shed zeroes the delivered export the same way: the command
/// stands, the watts do not, and the EV is no island source.
#[test]
fn island_availability_ends_at_dr_full_shed() {
    let mut ev = v2l_ev_at_reserve();
    ev.soc = 0.5;
    let env = sample_env();

    ev.apply_control_unchecked(&ControlSignal::DemandResponse {
        level: DRLevel::GridEmergency,
        duration_s: Some(3600.0),
    })
    .unwrap();
    let mut ports = PortSlots::default();
    ev.step(&env, Duration::minutes(1), &mut ports).unwrap();
    assert!(ev.v2l_active, "the discharge is still commanded");
    assert_eq!(
        ev.v2l_power_w, 0.0,
        "a GridEmergency shed zeroes the delivered export"
    );
    assert!(
        !ev.island_source_available(),
        "a DR-shed EV must not island the home"
    );
}

/// Islanded charging pays the surplus clamp: an 11.5 kW EVSE on an
/// island whose visible surplus is 2.5 kW charges at the surplus, and the
/// SOC credit never exceeds the energy the island's sources delivered.
#[test]
fn islanded_charging_clamps_to_visible_surplus() {
    let mut raw = base_raw();
    raw.insert(KEY_MAX_CHARGING_POWER_W.to_string(), 11500.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = outage_island_env();
    ev.init(&config, &env).unwrap();
    ev.soc = 0.3;

    let mut ports = PortSlots::default();
    // The bus so far this step: 1.5 kW of load and 4 kW of PV, a 2.5 kW
    // surplus the grid would have covered but the island cannot.
    ports.electrical.load_power_w = 1500.0;
    ports.electrical.generation_power_w = -4000.0;
    let soc_before = ev.soc;
    ev.step(&env, Duration::minutes(1), &mut ports).unwrap();

    let charge_w = ev.telemetry().get("active_power_kw").unwrap();
    assert!(
        charge_w <= 2500.0 + 1e-6,
        "islanded charging must clamp to the visible surplus, drew {charge_w} W"
    );
    assert!(
        charge_w > 0.0,
        "a real surplus must still charge the vehicle"
    );
    let soc_after = ev.telemetry().get("soc").unwrap();
    let gain_j = (soc_after - soc_before) * ev.battery_capacity_j;
    assert!(
        gain_j <= 2500.0 * 0.94 * 60.0 + 1.0,
        "the SOC credit must not exceed the clamped charge energy, gained {gain_j} J"
    );
}

/// The clamp is outage-only: the same bus state on a live grid charges at
/// the full EVSE rate.
#[test]
fn grid_connected_charging_is_not_clamped() {
    let mut raw = base_raw();
    raw.insert(KEY_MAX_CHARGING_POWER_W.to_string(), 11500.0.into());
    let config = ev_config(raw);
    let mut ev = Ev::new(config.clone());
    let env = sample_env();
    ev.init(&config, &env).unwrap();
    ev.soc = 0.3;

    let mut ports = PortSlots::default();
    ports.electrical.load_power_w = 1500.0;
    ports.electrical.generation_power_w = -4000.0;
    ev.step(&env, Duration::minutes(1), &mut ports).unwrap();

    let charge_w = ev.telemetry().get("active_power_kw").unwrap();
    assert!(
        (charge_w - 11500.0).abs() < 1e-6,
        "grid-connected charging draws the full EVSE rate, got {charge_w} W"
    );
}
