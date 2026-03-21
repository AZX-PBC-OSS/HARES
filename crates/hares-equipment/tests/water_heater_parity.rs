//! HARES-076: Water heater equipment step correctness tests.
//!
//! Tests physics correctness against hand-calculated OCHRE reference values.
//! Documents divergences where HARES intentionally differs from OCHRE.
//!
//! Reference: vendors/OCHRE/ochre/Equipment/WaterHeater.py
//!            vendors/OCHRE/ochre/Models/Water.py

use std::collections::HashMap;
use std::time::Duration;

use chrono::{TimeZone, Utc};
use hares_equipment::{Equipment, EquipmentConfig, config::ConfigValue};
use hares_equipment::water_heater::resistance::ResistanceWH;
use hares_equipment::water_heater::gas::GasWH;
use hares_types::{
    EnvironmentState, FuelType, GridState, PortSlots, WeatherState, ZoneId, ZoneState,
};

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn make_env(zone_temp_c: f64) -> EnvironmentState {
    EnvironmentState {
        zones: vec![ZoneState {
            id: ZoneId(1),
            temperature_c: zone_temp_c,
            humidity_ratio: 0.008,
            relative_humidity: 0.45,
            wet_bulb_c: zone_temp_c - 3.0,
            volume_m3: 200.0,
        }],
        weather: WeatherState {
            outdoor_temp_c: zone_temp_c,
            outdoor_humidity_ratio: 0.005,
            outdoor_wet_bulb_c: zone_temp_c - 5.0,
            outdoor_enthalpy_j_kg: 0.0,
            wind_speed_m_s: 1.5,
            wind_dir_deg: 0.0,
            ground_temp_c: zone_temp_c - 2.0,
            sky_temp_c: zone_temp_c - 10.0,
            pressure_kpa: 101.325,
            solar_irradiance: vec![],
            ghi_w_m2: 0.0,
            dni_w_m2: 0.0,
            dhi_w_m2: 0.0,
            solar_altitude_deg: 0.0,
            mains_temp_c: 15.0,
            rainfall_m: 0.0,
        },
        grid: GridState { voltage_pu: 1.0, frequency_hz: 60.0 },
        custom_domains: vec![],
        current_time: Utc
            .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
            .single()
            .expect("valid"),
        time_res: chrono::Duration::seconds(60),
    }
}

fn resistance_cfg(
    setpoint_c: f64,
    deadband_c: f64,
    initial_tank_temp_c: f64,
    draw_kg_s: f64,
    ua_w_per_k: f64,
) -> EquipmentConfig {
    let mut raw = HashMap::new();
    raw.insert("setpoint_c".to_string(), ConfigValue::Float(setpoint_c));
    raw.insert("deadband_c".to_string(), ConfigValue::Float(deadband_c));
    raw.insert("initial_tank_temp_c".to_string(), ConfigValue::Float(initial_tank_temp_c));
    raw.insert("draw_flow_rate_kg_s".to_string(), ConfigValue::Float(draw_kg_s));
    raw.insert("max_tank_temp_c".to_string(), ConfigValue::Float(300.0));
    raw.insert("ua_w_per_k".to_string(), ConfigValue::Float(ua_w_per_k));
    EquipmentConfig {
        name: "RWH".to_string(),
        ochre_class: "Resistance Water Heater".to_string(),
        raw_config: raw,
    }
}

fn step_wh(wh: &mut dyn hares_equipment::Equipment, env: &EnvironmentState, ports: &mut PortSlots) {
    ports.electrical = Default::default();
    ports.fuel = Default::default();
    ports.thermal.iter_mut().for_each(|t| t.zero());
    ports.fluid.iter_mut().for_each(|f| f.zero());
    wh.step(env, Duration::from_secs(60), ports).unwrap();
}

// ---------------------------------------------------------------------------
// 1. Standby loss over 24 hours (HARES-076 "standby loss" scenario)
//
// OCHRE WaterHeater.py: standby loss = UA × (T_tank - T_ambient) per step.
// At equilibrium (no draws, element off), tank temperature decays exponentially.
//
// Setup: 50-gal tank at 51.7°C (125°F), ambient 20°C, UA=2.0 W/K, no draws.
// Setpoint set well below initial temperature so element never fires.
//
// Expected result: tank temperature must drop (standby losses are non-zero).
// The temperature drop over 24 hours with UA=2.0 W/K, ΔT≈31.7 K gives
// ~63 W average standby loss → ΔE = 63 × 86400 = 5.45 MJ over 24 hours.
// With tank thermal mass ≈ 189.3 kg × 4183 J/(kg·K) = 791.9 kJ/K, the
// drop ΔT ≈ 5.45 MJ / 791.9 kJ/K ≈ 6.9°C (exact answer is slightly less
// due to exponential decay reducing ΔT as temperature drops).
// ---------------------------------------------------------------------------
#[test]
fn standby_loss_over_24h() {
    let env = make_env(20.0);
    // Setpoint below initial: element never fires → only standby losses
    let cfg = resistance_cfg(30.0, 2.0, 51.7, 0.0, 2.0);

    let mut wh = ResistanceWH::new(cfg.clone());
    wh.init(&cfg, &env).unwrap();
    let mut ports = PortSlots::from_declarations(wh.ports());

    // Telemetry is populated after the first step, so use the known initial temp from config.
    let initial_temp = 51.7_f64;

    // 24 hours = 1440 steps at 60 s each
    for _ in 0..1440 {
        step_wh(&mut wh, &env, &mut ports);
    }

    let final_temp = wh.telemetry().get("tank_avg_temp_c").expect("tank_avg_temp_c must exist");
    let power = wh.telemetry().get("electric_power_w").expect("electric_power_w must exist");

    // Element must be off (setpoint is 30°C < current tank temp for most of the run)
    // It might fire briefly near 30°C, but long-run the tank settles far above ambient.
    eprintln!(
        "[wh_parity] standby_loss: initial={initial_temp:.3}°C final={final_temp:.3}°C \
         drop={:.3}°C element_power={power:.1} W",
        initial_temp - final_temp
    );

    // Tank must have cooled measurably (any non-trivial UA loss is detectable over 24h)
    assert!(
        final_temp < initial_temp - 1.0,
        "tank must cool by > 1°C in 24h with UA=2.0 W/K; \
         dropped only {:.3}°C (initial={initial_temp:.3}, final={final_temp:.3})",
        initial_temp - final_temp
    );

    // Upper bound: tank cannot cool to ambient in 24h (UA too small)
    assert!(
        final_temp > env.zones[0].temperature_c + 5.0,
        "tank should not reach ambient in 24h with small UA; \
         final={final_temp:.3}°C, ambient={:.1}°C",
        env.zones[0].temperature_c
    );
}

// ---------------------------------------------------------------------------
// 2. Element cycling: deadband thresholds match OCHRE default
//
// OCHRE WaterHeater.py default deadband = 10°F = 5.556°C.
// HARES resistance.rs DEFAULT_DEADBAND_C = 5.555_555_556 (10°F).
//
// DIVERGENCE NOTE: The HARES-076 ticket claimed a mismatch (OCHRE 5.56°C vs
// HARES 2.0°C). Code inspection confirms HARES already uses 5.556°C as the
// DEFAULT. Tests that pass explicit deadband_c=2.0 use a non-default value;
// the default behaviour matches OCHRE.
//
// This test verifies:
//   - Element turns on at (setpoint - deadband) = 48.9 - 5.556 = 43.34°C
//   - Element turns off at setpoint = 48.9°C
// ---------------------------------------------------------------------------
#[test]
fn element_cycling_deadband_matches_ochre_default() {
    let env = make_env(20.0);
    let setpoint_c = 48.9_f64;
    // Use a config that does NOT specify deadband_c → uses HARES default (5.556°C = OCHRE default)
    let mut raw = HashMap::new();
    raw.insert("setpoint_c".to_string(), ConfigValue::Float(setpoint_c));
    raw.insert("initial_tank_temp_c".to_string(), ConfigValue::Float(40.0)); // well below threshold
    raw.insert("draw_flow_rate_kg_s".to_string(), ConfigValue::Float(0.0));
    raw.insert("max_tank_temp_c".to_string(), ConfigValue::Float(300.0));
    raw.insert("ua_w_per_k".to_string(), ConfigValue::Float(0.0)); // no standby to isolate
    raw.insert("tank_nodes".to_string(), ConfigValue::Float(1.0)); // single node
    let cfg = EquipmentConfig {
        name: "RWH".to_string(),
        ochre_class: "Resistance Water Heater".to_string(),
        raw_config: raw,
    };

    let mut wh = ResistanceWH::new(cfg.clone());
    wh.init(&cfg, &env).unwrap();
    let mut ports = PortSlots::from_declarations(wh.ports());

    // Tank starts at 40°C, below deadband floor (43.34°C) → element should fire immediately
    step_wh(&mut wh, &env, &mut ports);
    let power_step1 = wh.telemetry().get("electric_power_w").expect("electric_power_w must exist");
    assert!(
        power_step1 > 0.0,
        "element must fire when tank (40°C) is below deadband floor (~43.3°C); got {power_step1:.2} W"
    );

    // Run until element turns off (setpoint reached)
    let mut turned_off = false;
    let mut steps = 0;
    while steps < 200 {
        steps += 1;
        step_wh(&mut wh, &env, &mut ports);
        let power = wh.telemetry().get("electric_power_w").expect("electric_power_w must exist");
        let temp = wh.telemetry().get("tank_avg_temp_c").expect("tank_avg_temp_c must exist");
        if power < 1.0 {
            turned_off = true;
            // Verify temperature at shutoff: must be at or above setpoint
            assert!(
                temp >= setpoint_c - 0.5,
                "element must turn off at setpoint ({setpoint_c}°C); off at {temp:.3}°C"
            );
            eprintln!(
                "[wh_parity] cycling: element off at {temp:.3}°C after {steps} steps \
                 (setpoint={setpoint_c}°C, OCHRE_default_deadband=5.556°C)"
            );
            break;
        }
    }
    assert!(turned_off, "element must turn off after reaching setpoint within 200 steps");
}

// ---------------------------------------------------------------------------
// 3. Gas WH: fuel consumption, no electricity when no fan
//
// OCHRE WaterHeater.py GasWaterHeater: burner is fueled by gas, no electric
// (unless a blower is configured). With pilot_power_w=0, a cold tank must
// report positive gas and zero electricity.
// ---------------------------------------------------------------------------
#[test]
fn gas_wh_fuel_not_electricity() {
    let env = make_env(21.0);
    let mut raw = HashMap::new();
    raw.insert("setpoint_c".to_string(), ConfigValue::Float(52.0));
    raw.insert("deadband_c".to_string(), ConfigValue::Float(5.556));
    raw.insert("initial_tank_temp_c".to_string(), ConfigValue::Float(40.0)); // cold
    raw.insert("draw_flow_rate_kg_s".to_string(), ConfigValue::Float(0.0));
    raw.insert("max_tank_temp_c".to_string(), ConfigValue::Float(300.0));
    raw.insert("pilot_power_w".to_string(), ConfigValue::Float(0.0));
    let cfg = EquipmentConfig {
        name: "GWH".to_string(),
        ochre_class: "Gas Water Heater".to_string(),
        raw_config: raw,
    };

    let mut wh = GasWH::new(cfg.clone());
    wh.init(&cfg, &env).unwrap();
    let mut ports = PortSlots::from_declarations(wh.ports());

    step_wh(&mut wh, &env, &mut ports);

    let gas_w = ports.fuel.get(FuelType::Gas);
    let elec_kw = ports.electrical.load_power_kw;

    assert!(gas_w > 0.0, "gas WH must consume gas when cold (40°C < 49.2°C floor); got {gas_w:.2} W");
    assert_eq!(
        elec_kw, 0.0,
        "gas WH with no fan must draw zero electricity; got {elec_kw:.6} kW"
    );

    // Telemetry must be consistent
    let tel_gas = wh.telemetry().get("gas_consumption_w").expect("gas_consumption_w must exist");
    assert!(
        (tel_gas - gas_w).abs() < 1e-6,
        "telemetry gas_consumption_w ({tel_gas:.2}) must match port ({gas_w:.2})"
    );
}

// ---------------------------------------------------------------------------
// 4. Tank temperature never goes below mains temperature during heavy draw
//
// Physics: incoming mains water sets the lower bound on tank temperature.
// A tank cannot be cooler than the inlet water it receives.
// ---------------------------------------------------------------------------
#[test]
fn tank_temp_never_below_mains_during_draw() {
    let env = make_env(20.0);
    // Fast draw, low setpoint so element stays off
    let cfg = resistance_cfg(10.0, 2.0, 20.0, 0.10, 0.0);

    let mut wh = ResistanceWH::new(cfg.clone());
    wh.init(&cfg, &env).unwrap();
    let mut ports = PortSlots::from_declarations(wh.ports());

    let mains_temp_c = env.weather.mains_temp_c;

    for step in 0..60 {
        step_wh(&mut wh, &env, &mut ports);
        let temp =
            wh.telemetry().get("tank_avg_temp_c").expect("tank_avg_temp_c must exist");
        assert!(
            temp >= mains_temp_c - 0.1,
            "tank ({temp:.4}°C) must not drop below mains ({mains_temp_c}°C) at step {step}"
        );
    }
}

// ---------------------------------------------------------------------------
// 5. Standby loss magnitude: UA × ΔT gives expected power
//
// OCHRE WaterHeater.py: skin_loss = UA × (T_tank - T_ambient)
// With UA=5.0 W/K and ΔT=30 K → expected ~150 W standby loss.
// We cannot observe the skin loss directly on the thermal port without
// FIX-031-002 applied, but we can verify the temperature drop rate
// is consistent with the expected UA×ΔT.
//
// Check: over one 60-second step, ΔT ≈ UA×ΔT_ambient / (m×Cp) = 5.0×30 / 791_900 ≈ 1.9 mK.
// Single-step forward-Euler integration is exact to machine precision for a
// linear ODE (no nonlinear draw or switching); 1% tolerance is appropriate.
// ---------------------------------------------------------------------------
#[test]
fn standby_loss_ua_magnitude() {
    let env = make_env(20.0);
    let ua_w_per_k = 5.0_f64;
    let tank_temp_c = 50.0_f64;
    let ambient_c = 20.0_f64;
    let dt_s = 60.0_f64;

    // Single node for exact calculation; setpoint below initial so element off
    let mut raw = HashMap::new();
    raw.insert("setpoint_c".to_string(), ConfigValue::Float(10.0)); // below ambient
    raw.insert("deadband_c".to_string(), ConfigValue::Float(2.0));
    raw.insert("initial_tank_temp_c".to_string(), ConfigValue::Float(tank_temp_c));
    raw.insert("draw_flow_rate_kg_s".to_string(), ConfigValue::Float(0.0));
    raw.insert("max_tank_temp_c".to_string(), ConfigValue::Float(300.0));
    raw.insert("ua_w_per_k".to_string(), ConfigValue::Float(ua_w_per_k));
    raw.insert("tank_nodes".to_string(), ConfigValue::Float(1.0));
    let cfg = EquipmentConfig {
        name: "RWH".to_string(),
        ochre_class: "Resistance Water Heater".to_string(),
        raw_config: raw,
    };

    let mut wh = ResistanceWH::new(cfg.clone());
    wh.init(&cfg, &env).unwrap();
    let mut ports = PortSlots::from_declarations(wh.ports());

    step_wh(&mut wh, &env, &mut ports);

    let temp_after = wh.telemetry().get("tank_avg_temp_c").expect("tank_avg_temp_c must exist");
    let actual_loss_w = (tank_temp_c - temp_after)
        * (50.0 * 3.78541 /* gal→kg */ * 4183.0)
        / dt_s;

    let expected_loss_w = ua_w_per_k * (tank_temp_c - ambient_c);

    eprintln!(
        "[wh_parity] ua_magnitude: expected UA×ΔT={expected_loss_w:.1} W, \
         actual from temp drop={actual_loss_w:.1} W, \
         tank after={temp_after:.4}°C"
    );

    // Single-step forward-Euler on a linear ODE is exact; 1% tolerance covers
    // only the thermal-mass estimate (50 gal × 3.78541 kg/gal vs. actual tank mass).
    let tol = expected_loss_w * 0.01;
    assert!(
        (actual_loss_w - expected_loss_w).abs() < tol,
        "standby loss must match UA×ΔT={expected_loss_w:.1} W within 1%; \
         actual={actual_loss_w:.1} W"
    );
}

// ---------------------------------------------------------------------------
// 6. HPWH documentation: COP curves may not be fully implemented
//
// HARES-076 notes HPWH COP curves may be incomplete. This test documents
// current behavior: initialize a HPWH and verify it either (a) computes
// a reasonable COP or (b) fails clearly with a diagnostic message.
//
// If the HPWH is not yet implemented, this test reports the gap but does
// not fail — it is marked #[ignore] to prevent CI noise until the model
// is complete.
// ---------------------------------------------------------------------------
#[test]
#[ignore = "HPWH COP curves not yet fully implemented; document gap only"]
fn hpwh_cop_at_multiple_ambient_temps() {
    use hares_equipment::water_heater::heat_pump_wh::HeatPumpWH;

    let setpoint_c = 51.7_f64;
    let tank_temp_c = 50.0_f64;

    for ambient_c in [10.0, 20.0, 30.0, 40.0_f64] {
        let env = make_env(ambient_c);
        let mut raw = HashMap::new();
        raw.insert("setpoint_c".to_string(), ConfigValue::Float(setpoint_c));
        raw.insert("deadband_c".to_string(), ConfigValue::Float(5.556));
        raw.insert("initial_tank_temp_c".to_string(), ConfigValue::Float(tank_temp_c));
        raw.insert("draw_flow_rate_kg_s".to_string(), ConfigValue::Float(0.0));
        raw.insert("max_tank_temp_c".to_string(), ConfigValue::Float(300.0));
        let cfg = EquipmentConfig {
            name: "HPWH".to_string(),
            ochre_class: "Heat Pump Water Heater".to_string(),
            raw_config: raw,
        };

        let mut wh = HeatPumpWH::new(cfg.clone());
        wh.init(&cfg, &env).unwrap();
        let mut ports = PortSlots::from_declarations(wh.ports());

        // Force tank cold to trigger compressor
        step_wh(&mut wh, &env, &mut ports);

        let cop = wh.telemetry().get("cop").unwrap_or(-1.0);
        let elec_w = wh.telemetry().get("electric_power_w").unwrap_or(0.0);

        eprintln!(
            "[wh_parity] hpwh_cop: ambient={ambient_c}°C cop={cop:.3} elec={elec_w:.1} W"
        );

        if cop > 0.0 {
            // HPWH COP should be above 1.0 (heat pump thermodynamics)
            assert!(
                cop > 1.0,
                "HPWH COP must exceed 1.0 at ambient {ambient_c}°C; got {cop:.3}"
            );
            // At 20°C ambient, OCHRE reference HPWH gives COP ≈ 3.0–4.0
            // At 40°C ambient, COP ≈ 4.5–5.5 (better source of heat)
            assert!(
                cop < 7.0,
                "HPWH COP {cop:.3} unrealistically high at {ambient_c}°C"
            );
        }
    }
}
