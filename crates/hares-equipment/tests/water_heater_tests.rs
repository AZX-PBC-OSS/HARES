//! Integration tests for water heater equipment models.
//!
//! These tests exercise the public API of each WH type through the `Equipment`
//! trait. They assert physics invariants rather than exact floating-point values;
//! tolerances reflect real tank-simulation uncertainty (0.1 °C for temperature,
//! 2% for energy balance given the trapezoidal approximation of per-step averages).

use std::collections::HashMap;
use std::time::Duration;

use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
use hares_equipment::water_heater::gas::GasWH;
use hares_equipment::water_heater::resistance::ResistanceWH;
use hares_equipment::{Equipment, EquipmentConfig};
use hares_types::{
    ControlSignal, EnvironmentState, FuelType, GridState, OperatingMode, PortSlots, WeatherState,
    ZoneId, ZoneState,
};

// ── Shared test helpers ──────────────────────────────────────────────────────

fn make_env(zone_temp_c: f64) -> EnvironmentState {
    EnvironmentState {
        zones: vec![ZoneState {
            id: ZoneId(1),
            temperature_c: zone_temp_c,
            humidity_ratio: 0.008,
            relative_humidity: 0.45,
            wet_bulb_c: 14.0,
            volume_m3: 200.0,
        }],
        weather: WeatherState {
            outdoor_temp_c: zone_temp_c,
            outdoor_humidity_ratio: 0.005,
            wind_speed_m_s: 1.5,
            wind_dir_deg: 0.0,
            ground_temp_c: zone_temp_c,
            sky_temp_c: zone_temp_c - 5.0,
            pressure_kpa: 101.325,
            solar_irradiance: vec![],
            ghi_w_m2: 0.0,
            dni_w_m2: 0.0,
            dhi_w_m2: 0.0,
            solar_altitude_deg: 0.0,
            ..Default::default()
        },
        grid: GridState {
            voltage_pu: 1.0,
            frequency_hz: 60.0,
        },
        custom_domains: vec![],
        current_time: FixedOffset::east_opt(0)
            .expect("UTC offset")
            .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
            .single()
            .expect("valid timestamp"),
        time_res: ChronoDuration::seconds(60),
    }
}

/// Build a resistance WH config.
///
/// `max_tank_temp_c` is set to 300°C so the safety cutout never interferes
/// with the thermostat logic unless a test specifically overrides it.
fn resistance_config(
    setpoint_c: f64,
    deadband_c: f64,
    initial_tank_temp_c: f64,
    draw_flow_rate_kg_s: f64,
) -> EquipmentConfig {
    let mut raw: HashMap<String, hares_equipment::config::ConfigValue> = HashMap::new();
    raw.insert("setpoint_c".to_string(), setpoint_c.into());
    raw.insert("deadband_c".to_string(), deadband_c.into());
    raw.insert(
        "initial_tank_temp_c".to_string(),
        initial_tank_temp_c.into(),
    );
    raw.insert(
        "draw_flow_rate_kg_s".to_string(),
        draw_flow_rate_kg_s.into(),
    );
    raw.insert("max_tank_temp_c".to_string(), 300.0.into());
    EquipmentConfig {
        name: "RWH".to_string(),
        ochre_class: "Resistance Water Heater".to_string(),
        raw_config: raw,
    }
}

fn gas_config(
    setpoint_c: f64,
    deadband_c: f64,
    initial_tank_temp_c: f64,
    draw_flow_rate_kg_s: f64,
) -> EquipmentConfig {
    let mut raw: HashMap<String, hares_equipment::config::ConfigValue> = HashMap::new();
    raw.insert("setpoint_c".to_string(), setpoint_c.into());
    raw.insert("deadband_c".to_string(), deadband_c.into());
    raw.insert(
        "initial_tank_temp_c".to_string(),
        initial_tank_temp_c.into(),
    );
    raw.insert(
        "draw_flow_rate_kg_s".to_string(),
        draw_flow_rate_kg_s.into(),
    );
    raw.insert("max_tank_temp_c".to_string(), 300.0.into());
    // No standing pilot; isolates burner power from fuel_input_w.
    raw.insert("pilot_power_w".to_string(), 0.0.into());
    EquipmentConfig {
        name: "GWH".to_string(),
        ochre_class: "Gas Water Heater".to_string(),
        raw_config: raw,
    }
}

/// Step the WH, resetting port accumulators each call so readings are per-step.
fn step_wh(wh: &mut dyn Equipment, env: &EnvironmentState, ports: &mut PortSlots) {
    ports.electrical = Default::default();
    ports.fuel = Default::default();
    ports.thermal.iter_mut().for_each(|t| t.zero());
    ports.fluid.iter_mut().for_each(|f| f.zero());
    wh.step(env, Duration::from_secs(60), ports).unwrap();
}

// ── Test 1: resistance_wh_heats_to_setpoint ──────────────────────────────────

/// A cold tank heats measurably toward setpoint over multiple 1-minute steps.
///
/// Physics: default 4.5 kW element into a 50-gal (~189 kg) tank. Starting at
/// 40°C and targeting 52°C, the element runs continuously. After 60 steps
/// (1 hour) the tank must be well above the 40°C start and must not have
/// overshot the 52°C setpoint by more than one deadband width.
#[test]
fn resistance_wh_heats_to_setpoint() {
    let env = make_env(21.0);
    let cfg = resistance_config(52.0, 2.0, 40.0, 0.0);

    let mut wh = ResistanceWH::new(cfg.clone());
    wh.init(&cfg, &env).unwrap();
    let mut ports = PortSlots::from_declarations(wh.ports());

    for _ in 0..60 {
        step_wh(&mut wh, &env, &mut ports);
    }

    let temp = wh.telemetry().get("tank_avg_temp_c").unwrap();
    assert!(
        temp > 40.0 + 1.0,
        "tank must heat from 40°C toward 52°C setpoint; got {temp:.3}°C after 60 steps"
    );
    assert!(
        temp <= 52.0 + 0.5,
        "tank must not overshoot setpoint (52°C) significantly; got {temp:.3}°C"
    );

    // Verify the element has actually turned off once setpoint is reached.
    let final_power = wh.telemetry().get("electric_power_w").unwrap_or(0.0);
    if temp >= 52.0 {
        assert_eq!(
            final_power, 0.0,
            "element must be off after reaching setpoint; got {final_power:.2} W at {temp:.3}°C"
        );
    }
}

// ── Test 2: resistance_wh_off_at_setpoint ────────────────────────────────────

/// A tank already at setpoint draws zero power on the first step.
///
/// The hysteresis logic only calls for heat when the sensor node is at or below
/// `setpoint - deadband`. At exactly setpoint the off→on threshold is not met.
#[test]
fn resistance_wh_off_at_setpoint() {
    let env = make_env(21.0);
    let cfg = resistance_config(52.0, 2.0, 52.0, 0.0);

    let mut wh = ResistanceWH::new(cfg.clone());
    wh.init(&cfg, &env).unwrap();
    let mut ports = PortSlots::from_declarations(wh.ports());

    step_wh(&mut wh, &env, &mut ports);

    let power_w = wh.telemetry().get("electric_power_w").unwrap_or(0.0);
    assert_eq!(
        power_w, 0.0,
        "element power must be zero when tank starts at setpoint (52°C); got {power_w:.3} W"
    );
}

// ── Test 3: tank_cools_without_draw_or_heating ────────────────────────────────

/// A hot tank with element disabled loses heat to ambient via UA loss.
///
/// We set the setpoint below the initial temperature so the thermostat never
/// calls for heat, and verify the tank temperature drops over time.
#[test]
fn tank_cools_without_draw_or_heating() {
    let env = make_env(21.0);
    // Setpoint well below initial → element never fires; tank cools passively.
    let cfg = resistance_config(30.0, 2.0, 52.0, 0.0);

    let mut wh = ResistanceWH::new(cfg.clone());
    wh.init(&cfg, &env).unwrap();
    let mut ports = PortSlots::from_declarations(wh.ports());

    // Run for 2 hours to accumulate meaningful UA loss (UA ≈ 2 W/K, ΔT ≈ 31 K → ~62 W).
    for _ in 0..120 {
        step_wh(&mut wh, &env, &mut ports);
    }

    let temp = wh.telemetry().get("tank_avg_temp_c").unwrap();
    assert!(
        temp < 52.0 - 0.1,
        "tank must cool at least 0.1°C from 52°C due to UA losses; got {temp:.4}°C"
    );
}

// ── Test 4: draw_cools_tank_proportionally ────────────────────────────────────

/// A larger draw rate produces a larger temperature drop than a smaller draw.
///
/// Element is disabled by keeping setpoint below initial. Cold mains water
/// (default 15°C) displaces hot water in proportion to flow rate.
#[test]
fn draw_cools_tank_proportionally() {
    let env = make_env(21.0);
    // Setpoint well below initial → element stays off; cold mains replaces hot water.

    let measure = |draw_kg_s: f64| -> f64 {
        let cfg = resistance_config(30.0, 2.0, 52.0, draw_kg_s);
        let mut wh = ResistanceWH::new(cfg.clone());
        wh.init(&cfg, &env).unwrap();
        let mut ports = PortSlots::from_declarations(wh.ports());
        // 10 steps at 0.10 kg/s = 60 L of draw total — well within a 50-gal tank.
        for _ in 0..10 {
            step_wh(&mut wh, &env, &mut ports);
        }
        wh.telemetry().get("tank_avg_temp_c").unwrap()
    };

    let temp_small = measure(0.01);
    let temp_large = measure(0.10);

    assert!(
        temp_large < temp_small,
        "larger draw (0.10 kg/s → {temp_large:.4}°C) must cool tank more than \
         smaller draw (0.01 kg/s → {temp_small:.4}°C)"
    );
    // Both draws must produce a detectable cooling from the 52°C start.
    assert!(
        temp_small < 52.0,
        "small draw must cool below 52°C; got {temp_small:.4}°C"
    );
    assert!(
        temp_large < 52.0,
        "large draw must cool below 52°C; got {temp_large:.4}°C"
    );
}

// ── Test 5: gas_wh_consumes_gas_not_electricity ───────────────────────────────

/// A gas WH draws fuel (gas) when heating, not electricity.
///
/// With pilot_power_w=0 and no fan configured, a cold tank (40°C, setpoint 52°C)
/// must produce positive gas consumption and zero electric draw.
#[test]
fn gas_wh_consumes_gas_not_electricity() {
    let env = make_env(21.0);
    let cfg = gas_config(52.0, 2.0, 40.0, 0.0);

    let mut wh = GasWH::new(cfg.clone());
    wh.init(&cfg, &env).unwrap();
    let mut ports = PortSlots::from_declarations(wh.ports());

    step_wh(&mut wh, &env, &mut ports);

    let gas_w = ports.fuel.get(FuelType::Gas);
    let electric_kw = ports.electrical.load_power_kw;

    assert!(
        gas_w > 0.0,
        "gas WH must report positive gas consumption when heating; got {gas_w:.2} W"
    );
    assert_eq!(
        electric_kw, 0.0,
        "gas WH with no fan must draw zero electricity; got {electric_kw:.6} kW"
    );

    // Telemetry must be consistent with the port report.
    let telemetry_gas = wh.telemetry().get("fuel_input_w").unwrap_or(0.0);
    assert!(
        (telemetry_gas - gas_w).abs() < 1e-6,
        "telemetry fuel_input_w ({telemetry_gas:.2}) must match port value ({gas_w:.2})"
    );
}

// ── Test 6: energy_conservation_over_draw_cycle ───────────────────────────────

/// Energy balance over a draw cycle for a 1-node resistance WH with UA=0.
///
/// With UA=0, the only energy flows are electrical input and draw loss:
///   ΔE_tank = E_in_electrical − E_draw_loss
///
/// Draw loss per step is approximated as:
///   E_draw = m_dot × Cp × (tank_temp − mains_temp) × dt
///
/// The balance is checked within 2% of total electrical input, which accounts
/// for the trapezoidal integration error of using per-step average temperatures.
#[test]
fn energy_conservation_over_draw_cycle() {
    let env = make_env(21.0);

    let mut raw: HashMap<String, hares_equipment::config::ConfigValue> = HashMap::new();
    raw.insert("setpoint_c".to_string(), 80.0.into()); // high setpoint — element runs all 30 steps
    raw.insert("deadband_c".to_string(), 2.0.into());
    // Start above mains temp (15°C) so all draw steps cool the tank —
    // avoids the cold-start regime where incoming water heats the tank.
    raw.insert("initial_tank_temp_c".to_string(), 20.0.into());
    raw.insert("draw_flow_rate_kg_s".to_string(), 0.02.into());
    raw.insert("max_tank_temp_c".to_string(), 300.0.into());
    // Single node for tractable energy accounting.
    raw.insert("tank_nodes".to_string(), 1.0.into());
    // Zero UA to isolate electrical input vs draw loss.
    raw.insert("ua_w_per_k".to_string(), 0.0.into());
    let cfg = EquipmentConfig {
        name: "RWH".to_string(),
        ochre_class: "Resistance Water Heater".to_string(),
        raw_config: raw,
    };

    let mut wh = ResistanceWH::new(cfg.clone());
    wh.init(&cfg, &env).unwrap();
    let mut ports = PortSlots::from_declarations(wh.ports());

    // 50-gal tank: 50 US gal × 3.78541 L/gal = 189.27 L ≈ 189.27 kg of water.
    let water_mass_kg = 189.27_f64;
    let cp_j_per_kg_k = 4183.0_f64;
    let thermal_mass = water_mass_kg * cp_j_per_kg_k;

    let dt_s = 60.0_f64;
    let n_steps = 30_u32;
    let mains_temp_c = 15.0_f64;
    let draw_kg_s = 0.02_f64;

    let initial_temp_c = 20.0_f64;
    let mut total_electric_j = 0.0_f64;
    let mut total_draw_heat_j = 0.0_f64;

    for _ in 0..n_steps {
        ports.electrical = Default::default();
        ports.fluid.iter_mut().for_each(|f| f.zero());

        wh.step(&env, Duration::from_secs(dt_s as u64), &mut ports)
            .unwrap();

        let electric_w = wh.telemetry().get("electric_power_w").unwrap_or(0.0);
        total_electric_j += electric_w * dt_s;

        // Heat removed by draw: m_dot × Cp × (tank_temp − mains_temp) per step.
        let tank_temp = wh.telemetry().get("tank_avg_temp_c").unwrap_or(0.0);
        let draw_heat_w = draw_kg_s * cp_j_per_kg_k * (tank_temp - mains_temp_c).max(0.0);
        total_draw_heat_j += draw_heat_w * dt_s;
    }

    let final_temp_c = wh.telemetry().get("tank_avg_temp_c").unwrap_or(0.0);
    let delta_tank_energy_j = thermal_mass * (final_temp_c - initial_temp_c);

    // Balance: ΔE_tank ≈ E_in_electrical − E_draw_loss  (UA=0 → no standby loss)
    let expected_delta_j = total_electric_j - total_draw_heat_j;

    // 2% tolerance: draw heat estimate uses end-of-step temperature (trapezoidal error).
    let tolerance = 0.02 * total_electric_j.abs().max(1.0);
    assert!(
        (delta_tank_energy_j - expected_delta_j).abs() < tolerance,
        "energy balance violation: ΔE_tank={delta_tank_energy_j:.1} J, \
         expected≈{expected_delta_j:.1} J (E_elec={total_electric_j:.1}, \
         E_draw={total_draw_heat_j:.1}); tolerance={tolerance:.1} J"
    );
}

// ── Test 7: setpoint_control_changes_target ───────────────────────────────────

/// A ThermalSetpoint control signal updates the active setpoint.
///
/// Start with a low setpoint (45°C), run until the tank satisfies it, then
/// raise the setpoint to 55°C and verify the element re-engages.
#[test]
fn setpoint_control_changes_target() {
    let env = make_env(21.0);
    // 40°C start, setpoint 45°C, deadband 2°C → floor = 43°C; 40 < 43 so element fires.
    let cfg = resistance_config(45.0, 2.0, 40.0, 0.0);

    let mut wh = ResistanceWH::new(cfg.clone());
    wh.init(&cfg, &env).unwrap();
    let mut ports = PortSlots::from_declarations(wh.ports());

    // Run until setpoint is satisfied (at most 60 steps).
    for _ in 0..60 {
        step_wh(&mut wh, &env, &mut ports);
        let temp = wh.telemetry().get("tank_avg_temp_c").unwrap_or(0.0);
        if temp >= 45.0 {
            break;
        }
    }

    // Step until element turns off. With a multi-node tank, the average may reach
    // setpoint before the upper-node thermostat sensor does, so allow a few steps
    // for stratification equilibration. In a correct implementation this takes at
    // most 2-3 steps after the average hits setpoint.
    let mut element_off = false;
    for _ in 0..10 {
        step_wh(&mut wh, &env, &mut ports);
        let power = wh.telemetry().get("electric_power_w").unwrap_or(1.0);
        if power < 1.0 {
            element_off = true;
            break;
        }
    }
    assert!(
        element_off,
        "element must turn off within 10 steps of tank average reaching setpoint"
    );

    // Raise setpoint to 55°C; tank is ~45°C which is below the new deadband floor (53°C).
    wh.apply_control(&ControlSignal::ThermalSetpoint {
        heating_setpoint_c: Some(55.0),
        cooling_setpoint_c: None,
        deadband_c: None,
    })
    .unwrap();

    step_wh(&mut wh, &env, &mut ports);

    let power_after = wh.telemetry().get("electric_power_w").unwrap_or(0.0);
    assert!(
        power_after > 0.0,
        "element must re-engage after setpoint raised to 55°C (tank ~45°C < 53°C floor); \
         got {power_after:.2} W"
    );
}

// ── Test 8: max_tank_temp_safety_limit ────────────────────────────────────────

/// The safety cutout prevents any tank node from exceeding max_tank_temp_c.
///
/// We use a single-node tank with setpoint above max_tank_temp_c so the
/// thermostat keeps calling for heat, but the safety mechanism must cut out
/// the element before the temperature exceeds the limit.
#[test]
fn max_tank_temp_safety_limit() {
    let env = make_env(21.0);
    let max_temp_c = 55.0_f64;

    let mut raw: HashMap<String, hares_equipment::config::ConfigValue> = HashMap::new();
    raw.insert("setpoint_c".to_string(), 80.0.into()); // above max
    raw.insert("deadband_c".to_string(), 2.0.into());
    raw.insert("initial_tank_temp_c".to_string(), 40.0.into());
    raw.insert("draw_flow_rate_kg_s".to_string(), 0.0.into());
    raw.insert("max_tank_temp_c".to_string(), max_temp_c.into());
    raw.insert("tank_nodes".to_string(), 1.0.into());
    let cfg = EquipmentConfig {
        name: "RWH".to_string(),
        ochre_class: "Resistance Water Heater".to_string(),
        raw_config: raw,
    };

    let mut wh = ResistanceWH::new(cfg.clone());
    wh.init(&cfg, &env).unwrap();
    let mut ports = PortSlots::from_declarations(wh.ports());

    for step in 0..60 {
        step_wh(&mut wh, &env, &mut ports);

        let tank_temp = wh.telemetry().get("tank_avg_temp_c").unwrap_or(0.0);
        // The safety cutout fires at the START of each step (before heat injection).
        // But the PREVIOUS step may have injected heat that pushed the tank above
        // max_temp_c. One timestep of overshoot is physically unavoidable:
        //   ΔT = P × dt / (m × Cp) = 4500 × 60 / (189.27 × 4183) ≈ 0.341°C
        // Allow exactly one timestep margin.
        let one_step_overshoot_c = 0.35;
        assert!(
            tank_temp <= max_temp_c + one_step_overshoot_c,
            "tank temperature ({tank_temp:.4}°C) must not exceed max_tank_temp_c \
             ({max_temp_c}°C) + {one_step_overshoot_c}°C (one timestep overshoot) \
             at step {step}"
        );
    }

    // After cutout engages, element must be off.
    let final_power = wh.telemetry().get("electric_power_w").unwrap_or(0.0);
    let final_temp = wh.telemetry().get("tank_avg_temp_c").unwrap_or(0.0);

    assert!(
        final_temp >= max_temp_c - 1.0,
        "tank must have approached max_temp_c before cutout; got {final_temp:.4}°C"
    );
    assert_eq!(
        final_power, 0.0,
        "element must be off when safety cutout is active; got {final_power:.2} W"
    );
}

// ── Additional physics invariant tests ───────────────────────────────────────

/// operating_mode telemetry must be consistent with measured electric power.
///
/// mode=1 (Heating) implies power > 0; mode=0 (Off) implies power == 0.
#[test]
fn operating_mode_consistent_with_power() {
    let env = make_env(21.0);
    let cfg = resistance_config(52.0, 2.0, 40.0, 0.0);

    let mut wh = ResistanceWH::new(cfg.clone());
    wh.init(&cfg, &env).unwrap();
    let mut ports = PortSlots::from_declarations(wh.ports());

    for _ in 0..120 {
        step_wh(&mut wh, &env, &mut ports);

        let mode = wh.telemetry().get("operating_mode").unwrap_or(0.0);
        let power = wh.telemetry().get("electric_power_w").unwrap_or(0.0);

        if mode > 0.5 {
            assert!(
                power > 0.0,
                "operating_mode=Heating must imply positive electric_power_w; got {power:.2} W"
            );
        } else {
            assert_eq!(
                power, 0.0,
                "operating_mode=Off must imply zero electric_power_w; got {power:.2} W"
            );
        }
    }
}

/// Gas WH: operating_mode=Heating implies positive fuel_input_w.
#[test]
fn gas_wh_mode_consistent_with_fuel_consumption() {
    let env = make_env(21.0);
    let cfg = gas_config(52.0, 2.0, 40.0, 0.0);

    let mut wh = GasWH::new(cfg.clone());
    wh.init(&cfg, &env).unwrap();
    let mut ports = PortSlots::from_declarations(wh.ports());

    for _ in 0..120 {
        step_wh(&mut wh, &env, &mut ports);

        let mode = wh.telemetry().get("operating_mode").unwrap_or(0.0);
        let burner_w = wh.telemetry().get("burner_power_w").unwrap_or(0.0);
        let gas_w = wh.telemetry().get("fuel_input_w").unwrap_or(0.0);

        if mode > 0.5 {
            assert!(
                burner_w > 0.0,
                "operating_mode=Heating must imply positive burner_power_w; got {burner_w:.2}"
            );
            assert!(
                gas_w > 0.0,
                "operating_mode=Heating must imply positive fuel_input_w; got {gas_w:.2}"
            );
        } else {
            assert_eq!(
                burner_w, 0.0,
                "operating_mode=Off must imply zero burner_power_w; got {burner_w:.2}"
            );
        }
    }
}

/// tank_avg_temp_c must always be finite and within a physically plausible range.
#[test]
fn tank_avg_temp_always_physically_bounded() {
    let env = make_env(21.0);
    let cfg = resistance_config(52.0, 2.0, 40.0, 0.05);

    let mut wh = ResistanceWH::new(cfg.clone());
    wh.init(&cfg, &env).unwrap();
    let mut ports = PortSlots::from_declarations(wh.ports());

    for step in 0..60 {
        step_wh(&mut wh, &env, &mut ports);

        let temp = wh.telemetry().get("tank_avg_temp_c").unwrap_or(f64::NAN);
        assert!(
            temp.is_finite(),
            "tank_avg_temp_c must be finite at step {step}; got {temp}"
        );
        assert!(
            temp >= 0.0 && temp <= 300.0,
            "tank_avg_temp_c ({temp:.4}°C) out of physical range [0, 300] at step {step}"
        );
    }
}

/// ModeOverride::Off suppresses the element even when the tank is very cold.
#[test]
fn mode_override_off_suppresses_element() {
    let env = make_env(21.0);
    let cfg = resistance_config(52.0, 2.0, 10.0, 0.0);

    let mut wh = ResistanceWH::new(cfg.clone());
    wh.init(&cfg, &env).unwrap();
    wh.apply_control(&ControlSignal::ModeOverride {
        mode: OperatingMode::Off,
    })
    .unwrap();

    let mut ports = PortSlots::from_declarations(wh.ports());
    step_wh(&mut wh, &env, &mut ports);

    let power = wh.telemetry().get("electric_power_w").unwrap_or(0.0);
    assert_eq!(
        power, 0.0,
        "ModeOverride::Off must produce zero electric power even with cold tank; got {power:.2} W"
    );
}

/// Gas WH draw_flow_rate_kg_s telemetry must match the configured draw rate
/// when no appliance demand is present on the fluid port.
#[test]
fn gas_wh_draw_rate_telemetry_matches_config() {
    let env = make_env(21.0);
    let draw_rate = 0.03_f64;
    let cfg = gas_config(52.0, 2.0, 52.0, draw_rate);

    let mut wh = GasWH::new(cfg.clone());
    wh.init(&cfg, &env).unwrap();
    let mut ports = PortSlots::from_declarations(wh.ports());

    step_wh(&mut wh, &env, &mut ports);

    let reported = wh.telemetry().get("draw_flow_rate_kg_s").unwrap_or(-1.0);
    assert!(
        (reported - draw_rate).abs() < 1e-6,
        "gas WH draw_flow_rate_kg_s telemetry must match config ({draw_rate}); got {reported:.6}"
    );
}
