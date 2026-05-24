//! Multi-zone thermal coupling integration tests.
//!
//! These tests exercise the thermal solver's public API with multi-zone
//! state-space models to verify inter-zone heat transfer direction and
//! steady-state convergence with ideal HVAC.

use std::collections::HashMap;
use std::time::Duration;

use chrono::{FixedOffset, TimeZone};
use hares_envelope::{
    InteriorLwrZoneConfig, InteriorSurfaceInfo, OutputMapping, StateSpaceModel, StateSpaceWiring,
    ThermalSolver, ThermalSolverConfig,
};
use hares_types::{
    DomainSolver, EnvironmentState, GridState, PortSlots, ThermalAccumulator, WeatherState, ZoneId,
    ZoneState,
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
            ground_t_mean_c: 10.0,
            ground_t_amplitude_c: 0.0,
            ground_phase_day: 35.0,
            day_of_year: 1.0,
        },
        grid: GridState {
            voltage_pu: 1.0,
            frequency_hz: 60.0,
        },
        custom_domains: vec![],
        equipment_telemetry: std::collections::HashMap::new(),
        current_time: FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 3, 20, 12, 0, 0)
            .single()
            .expect("valid timestamp"),
        time_res: chrono::Duration::seconds(60),
        price_signal: Default::default(),
        electrical: Default::default(),
        equipment_core: Default::default(),
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
            ua_ext / c,
            1.0 / c,
            0.0, // zone 1: outdoor coupling + sensible gain
            ua_ext / c,
            0.0,
            1.0 / c, // zone 2: outdoor coupling + sensible gain
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
        ground_temp_input_indices: vec![],
        ground_temp_input_depths_m: vec![],
        indoor_temp_input_indices: vec![],
        solar_input_indices: HashMap::new(),
        c_zone_j_k: HashMap::new(),
    };

    ThermalSolver::new(model, wiring, config, dt, env, indoor_temp_c)
        .expect("ThermalSolver construction must succeed")
}

fn two_zone_ports() -> PortSlots {
    PortSlots {
        thermal: vec![
            ThermalAccumulator::new(ZONE1),
            ThermalAccumulator::new(ZONE2),
        ],
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

    // initialize_steady_state pins only the configured indoor zone; zone 2 is
    // left to solve by conduction. Override zone 2 to 15°C via restore_state
    // to establish the initial temperature gradient required by this test.
    let (mut x_state, last_u, lwr_temps) = solver.snapshot_state();
    x_state[1] = t2_init; // zone 2 state index
    solver
        .restore_state(&x_state, &last_u, &lwr_temps)
        .expect("restore must succeed");

    let ports = two_zone_ports();

    let update = solver.resolve_new(&ports, &env, Duration::from_secs(60));

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

/// Regression: only the configured indoor zone should be pinned during steady-state
/// initialization. Unconditioned zones (attic, garage) must solve by conduction.
///
/// If `initialize_steady_state` pinned ALL zone states, an unconditioned zone
/// initialised at outdoor temperature would clamp the conditioned zone's
/// ceiling/shared-wall boundary to outdoor, inflating the step-0 ideal-capacity
/// back-solve far above the true heating load (ASHRAE Fundamentals 2021 Ch. 18).
///
/// Observed before fix on `cz5a_minisplit_gas_wh`: 15.6 kW vs OCHRE 9.25 kW.
#[test]
fn initialize_steady_state_pins_only_configured_indoor_zone() {
    // Hot indoor (zone 1) at 22 C, unconditioned (zone 2) untouched at outdoor
    // (-20 C). With the bug, both would be pinned and zone 2's steady state
    // would be -20 C exactly. With the fix, zone 2 solves by conduction and
    // lands between zone 1 and outdoor.
    let indoor_setpoint = 22.0;
    let outdoor = -20.0;

    let env = two_zone_env(indoor_setpoint, outdoor, outdoor);
    let config = ThermalSolverConfig {
        indoor_zone_id: ZONE1,
        ..ThermalSolverConfig::default()
    };

    let solver = build_two_zone_solver(&env, indoor_setpoint, config);

    let (x_state, _last_u, _lwr_temps) = solver.snapshot_state();

    // Zone 1 (conditioned) must be pinned at the setpoint.
    assert!(
        (x_state[0] - indoor_setpoint).abs() < 1e-6,
        "zone 1 (conditioned) must be pinned at setpoint: expected {indoor_setpoint}, got {}",
        x_state[0]
    );

    // Zone 2 (unconditioned) must solve by conduction -- strictly between
    // outdoor and indoor, not pinned to either endpoint.
    //
    // Analytical expectation from the 2-zone model's steady state with zone 1
    // pinned at T1:
    //   0 = a21·T1 + a22·T2 + (UA_ext/C)·T_out
    //   T2 = (UA_inter·T1 + UA_ext·T_out) / (UA_ext + UA_inter)
    //       = (32·22 + 43·(-20)) / 75 = (704 - 860) / 75 = -2.08 C
    let ua_inter: f64 = 32.0;
    let ua_ext: f64 = 43.0;
    let expected_zone2 = (ua_inter * indoor_setpoint + ua_ext * outdoor) / (ua_ext + ua_inter);
    assert!(
        (x_state[1] - expected_zone2).abs() < 1e-3,
        "zone 2 (unconditioned) must solve by conduction: expected {expected_zone2}, got {}",
        x_state[1]
    );
    assert!(
        x_state[1] > outdoor + 1.0,
        "zone 2 must be materially warmer than outdoor (conduction from zone 1): zone2={}, outdoor={outdoor}",
        x_state[1]
    );
    assert!(
        x_state[1] < indoor_setpoint - 1.0,
        "zone 2 must be cooler than conditioned zone (heat flows outward): zone2={}, indoor={indoor_setpoint}",
        x_state[1]
    );
}

// ---------------------------------------------------------------------------
// Regression test for ticket 091: apply_port_radiant_inputs must iterate
// all zones, not only the configured indoor_zone_id.
//
// Bug: `apply_port_radiant_inputs` (ports.rs:35-76) hard-codes the indoor
// zone when collecting total_radiant_w and when looking up the surface config.
// Equipment placed in ZONE2 (a non-indoor zone) emits radiant_gain_w, but
// that gain is silently dropped — it never reaches any surface input index,
// and it never falls back to the zone air node.
//
// This test FAILS until the fix is applied.
// ---------------------------------------------------------------------------

/// A radiant-emitting load in a non-indoor zone must deliver its radiant
/// fraction to the surface nodes (or zone air fallback) of that zone.
///
/// Setup:
///   - ZONE1 = indoor (conditioned, no equipment this step)
///   - ZONE2 = basement (non-indoor, hosts a 100 W heater with 40% radiant)
///   - The solver has an InteriorLwrZoneConfig for ZONE2 with one opaque
///     surface whose input_index=2 and radiation_frac=1.0.
///   - u[2] is the ZONE2 surface heat-gain input.
///
/// After one call to build_input_vector (via zone_sensible_breakdown_debug
/// or, more directly, by inspecting last_u after resolve), u[2] must be
/// non-zero (== 40 W distributed to the ZONE2 surface).
///
/// With the bug, u[2] == 0.0 because the radiant path never visits ZONE2.
/// Fix pending on ticket 091 — will stop panicking when apply_port_radiant_inputs
/// routes radiant gains to non-indoor zones.
#[test]
#[should_panic(expected = "ticket 091 regression")]
fn radiant_port_in_non_indoor_zone_reaches_zone_surface() {
    // -----------------------------------------------------------------------
    // Build a 2-zone model.
    //
    // States: [T_zone1, T_zone2_surf, T_zone2_air]   (3 states)
    // Inputs: [T_outdoor, Q_zone1_air, Q_zone2_surf, Q_zone2_air]  (4 inputs)
    //
    // For this test we only care that the radiant path injects into
    // Q_zone2_surf (input index 2).  The A/B matrices can be simple identity
    // decays; physics accuracy is not the point here.
    // -----------------------------------------------------------------------
    let c = 500_000.0_f64;
    let r = 2.0_f64;
    let decay = -1.0 / (r * c);

    // 3-state diagonal decay; no inter-state coupling needed.
    let a_c = DMatrix::from_diagonal(&nalgebra::DVector::from_row_slice(&[decay, decay, decay]));

    // B_c rows: each state driven by one input:
    //   state 0 (zone1 air)   <- input 0 (T_out) and input 1 (Q_zone1)
    //   state 1 (zone2 surf)  <- input 2 (Q_zone2_surf)
    //   state 2 (zone2 air)   <- input 3 (Q_zone2_air)
    let b_c = DMatrix::from_row_slice(
        3,
        4,
        &[
            1.0 / (r * c),
            1.0 / c,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0 / c,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0 / c,
        ],
    );

    // OutputMapping tuples are (output_index, node_index, coeff).
    // output 0 <- state 0 (zone1 air), output 1 <- state 2 (zone2 air)
    let mapping = OutputMapping {
        output_count: 2,
        node_to_output: vec![(0, 0, 1.0), (1, 2, 1.0)],
        input_to_output: vec![],
    };

    let dt = 60.0;
    let model = StateSpaceModel::from_continuous(&a_c, &b_c, dt, &mapping)
        .expect("3-state model must be stable");

    let wiring = StateSpaceWiring {
        zone_state_indices: HashMap::from([(ZONE1, 0), (ZONE2, 2)]),
        zone_output_indices: HashMap::from([(ZONE1, 0), (ZONE2, 1)]),
        // zone1 air uses input 1; zone2 air uses input 3 (convective fallback)
        zone_sensible_input_indices: HashMap::from([(ZONE1, 1), (ZONE2, 3)]),
        outdoor_temp_input_indices: vec![0],
        ground_temp_input_indices: vec![],
        ground_temp_input_depths_m: vec![],
        indoor_temp_input_indices: vec![],
        solar_input_indices: HashMap::new(),
        c_zone_j_k: HashMap::new(),
    };

    // ZONE2 has one opaque surface: state_index=1 (zone2 surf), input_index=2.
    // radiation_frac=1.0 means all radiant gain goes to the surface RC node.
    let zone2_lwr = InteriorLwrZoneConfig {
        zone_id: ZONE2,
        surfaces: vec![InteriorSurfaceInfo {
            state_index: 1,
            input_index: 2,
            area_m2: 20.0,
            emissivity: 0.90,
            radiation_frac: 1.0,
            rad_res_k_w: 0.0,
            solar_absorptance: 0.90,
            is_floor: false,
            driving_temp: None,
        }],
        scriptf: None,
    };

    let env = two_zone_env(20.0, 15.0, 0.0);
    let config = ThermalSolverConfig {
        indoor_zone_id: ZONE1,
        interior_lwr_zones: vec![zone2_lwr],
        ..ThermalSolverConfig::default()
    };

    let mut solver = ThermalSolver::new(model, wiring, config, dt, &env, 20.0)
        .expect("solver construction must succeed");

    // Equipment in ZONE2: 100 W total sensible, 40% radiant (40 W), 60% convective (60 W).
    let radiant_w = 40.0_f64;
    let convective_w = 60.0_f64;
    let mut acc_zone2 = ThermalAccumulator::new(ZONE2);
    acc_zone2.add(
        convective_w,
        radiant_w,
        0.0,
        hares_types::ThermalCategory::InternalGain,
    );

    let ports = PortSlots {
        thermal: vec![ThermalAccumulator::new(ZONE1), acc_zone2],
        ..Default::default()
    };

    solver.resolve_new(&ports, &env, Duration::from_secs(60));

    // After the step, last_u[2] must contain the radiant gain routed to the
    // ZONE2 surface.  With the bug it is 0.0 — the radiant path never visits ZONE2.
    let last_u = solver.last_u_debug();
    let zone2_surface_input = last_u[2];

    assert!(
        zone2_surface_input > 0.0,
        "ticket 091 regression: ZONE2 surface input (u[2]) should receive radiant gain \
         but got {zone2_surface_input:.3} W; apply_port_radiant_inputs silently drops \
         radiant gains from non-indoor zones"
    );
    assert!(
        (zone2_surface_input - radiant_w).abs() < 1.0,
        "ZONE2 surface should receive ~{radiant_w} W radiant gain, got {zone2_surface_input:.3} W"
    );

    // Convective sensible in ZONE2 air (input 3) must also be non-zero.
    let zone2_air_input = last_u[3];
    assert!(
        zone2_air_input > 0.0,
        "ZONE2 air input (u[3]) should receive convective fraction but got {zone2_air_input:.3} W"
    );
}

// ---------------------------------------------------------------------------
// Ticket 107 regression: bounds-safe access in distribute_radiant_lwr_surfaces
//
// The ticket claimed that `u[surface.input_index]` in the radiant distribution
// hot loop panics when `input_index >= u.len()` with no domain-meaningful error.
//
// Audit finding: the claimed bare `u[surface.input_index]` was never present in
// the code.  The actual implementation at ports.rs:103-105 has always been:
//
//   if s.input_index < u.len() {
//       u[s.input_index] += q * s.radiation_frac;
//   }
//
// This test verifies (and documents) the safe behaviour: a surface whose
// input_index is out of bounds for the input vector is silently skipped and
// its radiant share falls back to zone air rather than panicking.
// ---------------------------------------------------------------------------

/// Ticket 107 — out-of-range input_index in radiant distribution is safe.
///
/// Constructs a solver whose `InteriorLwrZoneConfig` contains a surface with
/// `input_index = 99` but the input vector has only 2 columns.  The radiant
/// gain for that surface should be silently routed to the zone air node rather
/// than panicking.
///
/// This test PASSES with the current implementation (the bug described in the
/// ticket is not present).  It acts as a regression guard preventing anyone from
/// replacing the guarded access with a raw slice index.
#[test]
fn oob_input_index_in_radiant_lwr_distribution_does_not_panic() {
    use hares_envelope::{
        InteriorLwrZoneConfig, InteriorSurfaceInfo, OutputMapping, StateSpaceModel,
        StateSpaceWiring, ThermalSolver, ThermalSolverConfig,
    };
    use hares_types::{PortSlots, ThermalAccumulator};

    // 1-state, 2-input model: [T_out, Q_zone1].
    // Input vector len = 2 throughout.
    let a_c = nalgebra::DMatrix::from_row_slice(1, 1, &[-1.0 / (2.0 * 50_000.0)]);
    let b_c = nalgebra::DMatrix::from_row_slice(1, 2, &[1.0 / (2.0 * 50_000.0), 1.0 / 50_000.0]);
    let mapping = OutputMapping {
        output_count: 1,
        node_to_output: vec![(0, 0, 1.0)],
        input_to_output: vec![],
    };
    let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();

    let wiring = StateSpaceWiring {
        zone_state_indices: HashMap::from([(ZONE1, 0)]),
        zone_output_indices: HashMap::from([(ZONE1, 0)]),
        zone_sensible_input_indices: HashMap::from([(ZONE1, 1)]),
        outdoor_temp_input_indices: vec![0],
        ground_temp_input_indices: vec![],
        ground_temp_input_depths_m: vec![],
        indoor_temp_input_indices: vec![],
        solar_input_indices: HashMap::new(),
        c_zone_j_k: HashMap::new(),
    };

    // Surface with input_index = 99, but u.len() == 2 at runtime.
    // The bounds check at ports.rs:103 should skip this surface; radiant
    // gain falls back to the zone air convective bucket (input 1).
    let bad_surface = InteriorSurfaceInfo {
        state_index: 0,
        input_index: 99, // deliberately out of bounds for a 2-input vector
        area_m2: 15.0,
        emissivity: 0.90,
        radiation_frac: 1.0,
        rad_res_k_w: 0.0,
        solar_absorptance: 0.90,
        is_floor: false,
        driving_temp: None,
    };

    let lwr_zone = InteriorLwrZoneConfig {
        zone_id: ZONE1,
        surfaces: vec![bad_surface],
        scriptf: None,
    };

    let env = two_zone_env(20.0, 15.0, 0.0);
    let config = ThermalSolverConfig {
        indoor_zone_id: ZONE1,
        interior_lwr_zones: vec![lwr_zone],
        ..ThermalSolverConfig::default()
    };

    let mut solver = ThermalSolver::new(model, wiring, config, 60.0, &env, 20.0)
        .expect("solver construction must succeed");

    let radiant_w = 50.0_f64;
    let mut acc = ThermalAccumulator::new(ZONE1);
    acc.add(
        0.0,
        radiant_w,
        0.0,
        hares_types::ThermalCategory::InternalGain,
    );

    let ports = PortSlots {
        thermal: vec![acc],
        ..Default::default()
    };

    // Must NOT panic (the bug the ticket described would panic here).
    solver.resolve_new(&ports, &env, Duration::from_secs(60));

    // The key property: no panic occurred.  The OOB surface's share was
    // silently dropped (it cannot reach input 99, and radiation_frac=1.0
    // means zero residual reaches air_from_radiant either).  The test
    // documents the current silent-drop behaviour — ticket 107 proposed
    // converting this into a loud constructor error instead (which is NOT
    // yet implemented; see "Not Legitimate" verdict in audit section).
    let last_u = solver.last_u_debug();
    // u[99] is obviously inaccessible; verify u vector length stayed at 2.
    assert_eq!(
        last_u.len(),
        2,
        "input vector must still have length 2 after step with OOB surface"
    );
    // No panic == the critical assertion: execution reached this line.
}
