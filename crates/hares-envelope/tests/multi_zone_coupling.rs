//! Multi-zone thermal coupling integration tests.
//!
//! These tests exercise the thermal solver's public API with multi-zone
//! state-space models to verify inter-zone heat transfer direction and
//! steady-state convergence with ideal HVAC.

use std::collections::HashMap;
use std::time::Duration;

use chrono::{FixedOffset, TimeZone};
use hares_envelope::{OutputMapping, StateSpaceModel, StateSpaceWiring, ThermalSolver, ThermalSolverConfig};
use hares_types::{
    DomainSolver, EnvironmentState, GridState, PortSlots, ThermalAccumulator, WeatherState,
    ZoneId, ZoneState,
};
use nalgebra::DMatrix;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const ZONE1: ZoneId = ZoneId(1);
const ZONE2: ZoneId = ZoneId(2);

fn two_zone_env(zone1_temp_c: f64, zone2_temp_c: f64, outdoor_temp_c: f64) -> EnvironmentState {
    EnvironmentState {
        zones: vec![
            ZoneState {
                id: ZONE1,
                temperature_c: zone1_temp_c,
                humidity_ratio: 0.008,
                relative_humidity: 0.45,
                wet_bulb_c: zone1_temp_c - 5.0,
                volume_m3: 200.0,
            },
            ZoneState {
                id: ZONE2,
                temperature_c: zone2_temp_c,
                humidity_ratio: 0.008,
                relative_humidity: 0.45,
                wet_bulb_c: zone2_temp_c - 5.0,
                volume_m3: 200.0,
            },
        ],
        weather: WeatherState {
            outdoor_temp_c,
            outdoor_humidity_ratio: 0.004,
            outdoor_wet_bulb_c: outdoor_temp_c - 5.0,
            outdoor_enthalpy_j_kg: 22_800.0,
            wind_speed_m_s: 2.0,
            wind_dir_deg: 0.0,
            ground_temp_c: outdoor_temp_c,
            sky_temp_c: outdoor_temp_c - 5.0,
            pressure_kpa: 101.325,
            solar_irradiance: vec![],
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
        current_time: FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 3, 20, 12, 0, 0)
            .single()
            .expect("valid timestamp"),
        time_res: chrono::Duration::seconds(60),
    }
}

/// Build a 2-zone coupled thermal model.
///
/// States: [zone1_air, zone2_air]
/// Inputs: [outdoor_temp, zone1_sensible_gain, zone2_sensible_gain]
///
/// Physics:
///   dT1/dt = -(UA_ext + UA_inter)/C * T1 + UA_inter/C * T2 + UA_ext/C * T_out + Q1/C
///   dT2/dt = UA_inter/C * T1 - (UA_ext + UA_inter)/C * T2 + UA_ext/C * T_out + Q2/C
fn build_two_zone_solver(
    env: &EnvironmentState,
    indoor_temp_c: f64,
    config: ThermalSolverConfig,
) -> ThermalSolver {
    // Realistic residential values derived from BESTEST Case 600 split into 2 zones.
    // BESTEST total envelope UA ≈ 86 W/K; split evenly → 43 W/K per zone to outdoor.
    // Interior partition: ~16 m² drywall wall at R ≈ 0.5 m²·K/W → UA ≈ 32 W/K.
    // Zone thermal mass: BESTEST C ≈ 1,094,000 J/K total; ~547,000 per zone.
    let c = 547_000.0; // J/K per zone
    let ua_inter = 32.0; // W/K inter-zone partition wall
    let ua_ext = 43.0; // W/K per zone to outdoor

    // A_c: 2x2
    let a11 = -(ua_ext + ua_inter) / c;
    let a12 = ua_inter / c;
    let a21 = ua_inter / c;
    let a22 = -(ua_ext + ua_inter) / c;
    let a_c = DMatrix::from_row_slice(2, 2, &[a11, a12, a21, a22]);

    // B_c: 2x3 [outdoor_temp, Q1, Q2]
    let b_c = DMatrix::from_row_slice(
        2,
        3,
        &[
            ua_ext / c, 1.0 / c, 0.0,     // zone 1: outdoor coupling + sensible gain
            ua_ext / c, 0.0,     1.0 / c,  // zone 2: outdoor coupling + sensible gain
        ],
    );

    let mapping = OutputMapping {
        output_count: 2,
        node_to_output: vec![(0, 0, 1.0), (1, 1, 1.0)],
        input_to_output: vec![],
    };

    let dt = 60.0;
    let model = StateSpaceModel::from_continuous(&a_c, &b_c, dt, &mapping)
        .expect("2-zone state-space model must be stable");

    let wiring = StateSpaceWiring {
        zone_state_indices: HashMap::from([(ZONE1, 0), (ZONE2, 1)]),
        zone_output_indices: HashMap::from([(ZONE1, 0), (ZONE2, 1)]),
        zone_sensible_input_indices: HashMap::from([(ZONE1, 1), (ZONE2, 2)]),
        outdoor_temp_input_indices: vec![0],
        indoor_temp_input_indices: vec![],
        solar_input_indices: HashMap::new(),
    };

    ThermalSolver::new(model, wiring, config, dt, env, indoor_temp_c)
        .expect("ThermalSolver construction must succeed")
}

fn two_zone_ports() -> PortSlots {
    PortSlots {
        thermal: vec![ThermalAccumulator::new(ZONE1), ThermalAccumulator::new(ZONE2)],
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Zone 1 at 25 C, zone 2 at 15 C, connected by a wall. After one step,
/// zone 1 must have cooled and zone 2 must have warmed (heat flows hot to cold).
#[test]
fn test_two_zone_coupled_wall_heat_direction() {
    let t1_init = 25.0;
    let t2_init = 15.0;
    let outdoor = 10.0;

    let env = two_zone_env(t1_init, t2_init, outdoor);
    let config = ThermalSolverConfig {
        indoor_zone_id: ZONE1,
        ..ThermalSolverConfig::default()
    };

    let mut solver = build_two_zone_solver(&env, t1_init, config);

    // initialize_steady_state pins ALL zone states to indoor_temp_c (25°C).
    // Override zone 2 to 15°C via restore_state to establish the temperature gradient.
    let (mut x_state, last_u, lwr_temps) = solver.snapshot_state();
    x_state[1] = t2_init; // zone 2 state index
    solver
        .restore_state(&x_state, &last_u, &lwr_temps)
        .expect("restore must succeed");

    let ports = two_zone_ports();

    let update = solver.resolve(&ports, &env, Duration::from_secs(60));

    let t1_after = update
        .zone_temperatures_c
        .iter()
        .find(|(id, _)| *id == ZONE1)
        .expect("zone 1 must be in output")
        .1;
    let t2_after = update
        .zone_temperatures_c
        .iter()
        .find(|(id, _)| *id == ZONE2)
        .expect("zone 2 must be in output")
        .1;

    assert!(
        t1_after < t1_init,
        "zone 1 (hot) must cool after one step: initial={t1_init}, after={t1_after}"
    );
    assert!(
        t2_after > t2_init,
        "zone 2 (cold) must warm after one step: initial={t2_init}, after={t2_after}"
    );

    // Sanity: both temperatures must still be between outdoor and their initial values
    assert!(
        t1_after > outdoor,
        "zone 1 must remain above outdoor temp: t1={t1_after}, outdoor={outdoor}"
    );
    assert!(
        t2_after < t1_init,
        "zone 2 must remain below zone 1 initial: t2={t2_after}, t1_init={t1_init}"
    );
}

/// Two zones coupled with outdoor at 0 C, ideal HVAC setpoint 20 C in zone 1.
/// Run to steady state. Assert convergence and HVAC power > 0.
#[test]
fn test_two_zone_coupled_wall_steady_state() {
    let outdoor = 0.0;
    let initial_indoor = 20.0;

    let mut env = two_zone_env(initial_indoor, initial_indoor, outdoor);
    let config = ThermalSolverConfig {
        indoor_zone_id: ZONE1,
        ideal_setpoints_c: HashMap::from([(ZONE1, 20.0)]),
        ideal_hvac_zones: vec![ZONE1],
        ..ThermalSolverConfig::default()
    };

    let mut solver = build_two_zone_solver(&env, initial_indoor, config);
    let ports = two_zone_ports();

    // Run 2000 steps at 60s (~33 hours) to reach steady state
    for _ in 0..2000 {
        let update = solver.resolve(&ports, &env, Duration::from_secs(60));
        let t1_step = update
            .zone_temperatures_c
            .iter()
            .find(|(id, _)| *id == ZONE1)
            .unwrap()
            .1;
        let t2_step = update
            .zone_temperatures_c
            .iter()
            .find(|(id, _)| *id == ZONE2)
            .unwrap()
            .1;
        env.zones[0].temperature_c = t1_step;
        env.zones[1].temperature_c = t2_step;
    }

    // Run two more steps and check convergence
    let update_prev = solver.resolve(&ports, &env, Duration::from_secs(60));
    let t1_prev = update_prev
        .zone_temperatures_c
        .iter()
        .find(|(id, _)| *id == ZONE1)
        .unwrap()
        .1;
    let t2_prev = update_prev
        .zone_temperatures_c
        .iter()
        .find(|(id, _)| *id == ZONE2)
        .unwrap()
        .1;
    env.zones[0].temperature_c = t1_prev;
    env.zones[1].temperature_c = t2_prev;

    let update_last = solver.resolve(&ports, &env, Duration::from_secs(60));
    let t1_last = update_last
        .zone_temperatures_c
        .iter()
        .find(|(id, _)| *id == ZONE1)
        .unwrap()
        .1;
    let t2_last = update_last
        .zone_temperatures_c
        .iter()
        .find(|(id, _)| *id == ZONE2)
        .unwrap()
        .1;

    assert!(
        (t1_last - t1_prev).abs() < 0.01,
        "zone 1 must converge: t_prev={t1_prev}, t_last={t1_last}, delta={}",
        (t1_last - t1_prev).abs()
    );
    assert!(
        (t2_last - t2_prev).abs() < 0.01,
        "zone 2 must converge: t_prev={t2_prev}, t_last={t2_last}, delta={}",
        (t2_last - t2_prev).abs()
    );

    // Zone 1 must be near its setpoint (ideal HVAC maintaining temperature)
    assert!(
        (t1_last - 20.0).abs() < 0.5,
        "zone 1 must be near setpoint 20 C at steady state, got {t1_last}"
    );

    // Zone 2 must be between outdoor and zone 1 (heated indirectly via wall coupling)
    assert!(
        t2_last > outdoor,
        "zone 2 must be above outdoor temp: t2={t2_last}, outdoor={outdoor}"
    );
    assert!(
        t2_last < t1_last,
        "zone 2 must be below zone 1 (no direct HVAC): t2={t2_last}, t1={t1_last}"
    );

    // HVAC must be actively heating (zone 1 loses heat to zone 2 and outdoor).
    // Ideal HVAC writes directly to u[sensible_input_idx], not to port accumulators,
    // so read from the solver's last_u via snapshot.
    let (_, last_u, _) = solver.snapshot_state();
    let hvac_power_w = last_u[1]; // zone 1 sensible input index
    assert!(
        hvac_power_w > 0.0,
        "HVAC must be heating zone 1 at steady state, got {hvac_power_w} W"
    );
}
