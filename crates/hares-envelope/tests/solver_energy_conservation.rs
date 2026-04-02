//! Energy conservation integration tests for the thermal solver.
//!
//! These tests run the solver over extended periods and verify that the energy
//! balance closes: the change in stored thermal energy must equal the net heat
//! flow through the envelope (and HVAC, when active).

use std::collections::HashMap;
use std::time::Duration;

use chrono::{FixedOffset, TimeZone};
use hares_envelope::{
    OutputMapping, StateSpaceModel, StateSpaceWiring, ThermalSolver, ThermalSolverConfig,
};
use hares_types::{
    DomainSolver, EnvironmentState, GridState, PortSlots, ThermalAccumulator, WeatherState, ZoneId,
    ZoneState,
};
use nalgebra::DMatrix;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const ZONE: ZoneId = ZoneId(1);
// BESTEST Case 600 derived values (ASHRAE 140-2017).
// 8m × 6m × 2.7m zone, 129.6 m³, ρ_air ≈ 1.2 kg/m³, cp ≈ 1006 J/(kg·K),
// furniture multiplier 7× → C ≈ 1,094,000 J/K.
// Envelope: windows 36 + walls 35 + roof 15.3 ≈ 86.2 W/K; floor 1.87 W/K.
const C: f64 = 1_094_000.0; // J/K — BESTEST Case 600 zone thermal capacitance
const UA: f64 = 88.0; // W/K — total envelope conductance (walls+roof+windows+floor)
const DT_S: f64 = 300.0; // s — 5-minute timestep (BESTEST standard)
const STEPS_24H: usize = 288; // 24h / 300s

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn one_zone_env(zone_temp_c: f64, outdoor_temp_c: f64) -> EnvironmentState {
    EnvironmentState {
        zones: vec![ZoneState {
            id: ZONE,
            temperature_c: zone_temp_c,
            humidity_ratio: 0.008,
            relative_humidity: 0.45,
            wet_bulb_c: zone_temp_c - 5.0,
            volume_m3: 200.0,
        }],
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
        equipment_telemetry: std::collections::HashMap::new(),
        current_time: FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 3, 20, 12, 0, 0)
            .single()
            .expect("valid timestamp"),
        time_res: chrono::Duration::seconds(DT_S as i64),
        price_signal: Default::default(),
        electrical: Default::default(),
        equipment_core: Default::default(),
    }
}

/// Build a 1R1C thermal solver.
///
/// State: [zone_air_temp]
/// Inputs: [outdoor_temp, sensible_gain]
///
/// dT/dt = -UA/C * T + UA/C * T_out + Q/C
fn build_1r1c_solver(
    env: &EnvironmentState,
    indoor_temp_c: f64,
    config: ThermalSolverConfig,
) -> ThermalSolver {
    let a_c = DMatrix::from_row_slice(1, 1, &[-UA / C]);
    let b_c = DMatrix::from_row_slice(1, 2, &[UA / C, 1.0 / C]);

    let mapping = OutputMapping {
        output_count: 1,
        node_to_output: vec![(0, 0, 1.0)],
        input_to_output: vec![],
    };

    let model = StateSpaceModel::from_continuous(&a_c, &b_c, DT_S, &mapping)
        .expect("1R1C state-space model must be stable");

    let wiring = StateSpaceWiring {
        zone_state_indices: HashMap::from([(ZONE, 0)]),
        zone_output_indices: HashMap::from([(ZONE, 0)]),
        zone_sensible_input_indices: HashMap::from([(ZONE, 1)]),
        outdoor_temp_input_indices: vec![0],
        ground_temp_input_indices: vec![],
        indoor_temp_input_indices: vec![],
        solar_input_indices: HashMap::new(),
    };

    ThermalSolver::new(model, wiring, config, DT_S, env, indoor_temp_c)
        .expect("ThermalSolver construction must succeed")
}

fn one_zone_ports() -> PortSlots {
    PortSlots {
        thermal: vec![ThermalAccumulator::new(ZONE)],
        ..Default::default()
    }
}

fn zone_temp(update: &hares_types::DomainUpdate) -> f64 {
    update
        .zone_temperatures_c
        .iter()
        .find(|(id, _)| *id == ZONE)
        .expect("zone must be in output")
        .1
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// 1R1C model with no HVAC, zone at 20 C, outdoor at 0 C.
/// Run 24h and verify energy balance: |delta_E + Q_loss| / |Q_loss| < 0.1%.
#[test]
fn test_energy_conservation_1r1c_no_hvac() {
    let t_initial = 20.0;
    let t_outdoor = 0.0;

    let mut env = one_zone_env(t_initial, t_outdoor);
    let config = ThermalSolverConfig {
        indoor_zone_id: ZONE,
        ..ThermalSolverConfig::default()
    };

    let mut solver = build_1r1c_solver(&env, t_initial, config);
    let ports = one_zone_ports();

    let mut q_loss_total = 0.0;
    let mut t_zone = t_initial;

    for _ in 0..STEPS_24H {
        let t_before = t_zone;
        let update = solver.resolve_new(&ports, &env, Duration::from_secs(DT_S as u64));
        t_zone = zone_temp(&update);
        env.zones[0].temperature_c = t_zone;

        // Trapezoidal integration of heat loss over the step
        let t_avg = 0.5 * (t_before + t_zone);
        let q_loss_step = UA * (t_avg - t_outdoor) * DT_S;
        q_loss_total += q_loss_step;
    }

    let delta_e = C * (t_zone - t_initial); // negative (zone cooled)

    // Energy balance: delta_E + Q_loss should be ~0
    // (energy lost from thermal mass = energy conducted out)
    let balance_error = (delta_e + q_loss_total).abs();
    let relative_error = balance_error / q_loss_total.abs();

    assert!(
        relative_error < 0.001,
        "energy balance relative error {:.4}% exceeds 0.1% threshold; \
         delta_E={delta_e:.2} J, Q_loss={q_loss_total:.2} J, balance_error={balance_error:.2} J",
        relative_error * 100.0
    );

    // Sanity checks
    assert!(
        t_zone < t_initial,
        "zone must have cooled: initial={t_initial}, final={t_zone}"
    );
    assert!(
        t_zone > t_outdoor,
        "zone must remain above outdoor: final={t_zone}, outdoor={t_outdoor}"
    );
}
