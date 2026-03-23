//! Integration tests for the fluid solver.
//!
//! These tests exercise the public API through the crate re-exports and
//! cross-check solver behaviour against first-principles calculations.

use std::time::Duration;

use chrono::{FixedOffset, TimeZone};
use hares_envelope::{FluidSolver, FluidSolverConfig};
use hares_types::{
    DomainSolver, EnvironmentState, FluidAccumulator, FluidDomainPayload, FluidType, GridState,
    LoopId, PortContribution, PortSlots, SurfaceIrradiance, WeatherState, ZoneId, ZoneState,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn env() -> EnvironmentState {
    EnvironmentState {
        zones: vec![ZoneState {
            id: ZoneId(1),
            temperature_c: 21.0,
            humidity_ratio: 0.008,
            relative_humidity: 0.45,
            wet_bulb_c: 14.0,
            volume_m3: 200.0,
        }],
        weather: WeatherState {
            outdoor_temp_c: 10.0,
            outdoor_humidity_ratio: 0.005,
            outdoor_wet_bulb_c: 7.0,
            outdoor_enthalpy_j_kg: 22_800.0,
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

fn make_ports_with_flow(
    loop_id: LoopId,
    fluid_type: FluidType,
    flow_rate_kg_s: f64,
    supply_temp_c: f64,
    return_temp_c: f64,
) -> PortSlots {
    let mut ports = PortSlots {
        fluid: vec![FluidAccumulator::new(loop_id, fluid_type)],
        ..Default::default()
    };
    ports
        .accumulate(&PortContribution::Fluid {
            loop_id,
            flow_rate_kg_s,
            supply_temp_c,
            return_temp_c,
            fluid_type,
        })
        .expect("accumulate must not fail for valid contribution");
    ports
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// P = ṁ × c_p × ΔT.
///
/// With flow_rate=0.8 kg/s, supply=70 °C, return=50 °C (ΔT=20 K) and
/// c_p=4186 J/(kg·K), the expected net power is 0.8 × 4186 × 20 = 66 976 W.
#[test]
fn net_power_from_flow_and_temp_delta() {
    let cp = 4186.0;
    let flow = 0.8;
    let delta_t = 20.0;
    let expected_w = flow * cp * delta_t;

    let mut solver = FluidSolver::new(
        FluidSolverConfig {
            cp_water_j_kg_k: cp,
        },
        &[(LoopId(1), FluidType::Water)],
    )
    .expect("FluidSolver::new must succeed for valid config");

    let ports = make_ports_with_flow(LoopId(1), FluidType::Water, flow, 70.0, 50.0);
    let update = solver.resolve(&ports, &env(), Duration::from_secs(60));

    let payload = update
        .custom_payload
        .expect("payload must be Some when flow > 0");
    let states = FluidDomainPayload::decode(&payload).expect("payload must decode");
    assert_eq!(states.len(), 1, "exactly one loop state expected");

    let delta = (states[0].net_power_w - expected_w).abs();
    assert!(
        delta < 1e-6,
        "net_power_w={} expected={expected_w}, delta={delta}",
        states[0].net_power_w
    );
}

/// When no flow accumulator contributes, the solver must produce zero net power.
/// This is distinct from the unit test which uses an empty PortSlots; here we
/// provide an accumulator for the loop but contribute zero flow explicitly.
#[test]
fn zero_flow_zero_power() {
    let mut solver = FluidSolver::new(
        FluidSolverConfig::default(),
        &[(LoopId(2), FluidType::Water)],
    )
    .expect("FluidSolver::new must succeed");

    // Accumulator present, but no PortContribution::Fluid added — total_flow stays 0.
    let ports = PortSlots {
        fluid: vec![FluidAccumulator::new(LoopId(2), FluidType::Water)],
        ..Default::default()
    };

    let update = solver.resolve(&ports, &env(), Duration::from_secs(60));
    let payload = update
        .custom_payload
        .expect("payload must be Some: accumulator is present");
    let states = FluidDomainPayload::decode(&payload).expect("payload must decode");
    assert_eq!(states.len(), 1);

    assert_eq!(
        states[0].net_power_w, 0.0,
        "zero flow must produce exactly 0 W net power, got {}",
        states[0].net_power_w
    );
}

/// `snapshot_payload` / `restore_from_payload` must be an exact round-trip:
/// after restoring, a subsequent step with zero flow must produce the same
/// supply and return temperatures as before the snapshot.
#[test]
fn checkpoint_round_trip() {
    let supply = 65.0;
    let ret = 48.0;

    let mut solver = FluidSolver::new(
        FluidSolverConfig::default(),
        &[(LoopId(3), FluidType::Water)],
    )
    .expect("FluidSolver::new must succeed");

    // Prime the solver: one step with real flow so last_known_temps is populated.
    let ports_with_flow = make_ports_with_flow(LoopId(3), FluidType::Water, 1.0, supply, ret);
    solver.resolve(&ports_with_flow, &env(), Duration::from_secs(60));

    // Snapshot.
    let payload = solver.snapshot_payload();
    assert!(
        !payload.is_empty(),
        "snapshot must be non-empty after flow step"
    );

    // Build a fresh solver and restore.
    let mut restored = FluidSolver::new(
        FluidSolverConfig::default(),
        &[(LoopId(3), FluidType::Water)],
    )
    .expect("FluidSolver::new must succeed");
    restored
        .restore_from_payload(&payload)
        .expect("restore_from_payload must succeed for valid payload");

    // Both solvers should now report the same temperatures on a zero-flow step.
    let zero_ports = PortSlots {
        fluid: vec![FluidAccumulator::new(LoopId(3), FluidType::Water)],
        ..Default::default()
    };

    let update_orig = solver.resolve(&zero_ports, &env(), Duration::from_secs(60));
    let update_restored = restored.resolve(&zero_ports, &env(), Duration::from_secs(60));

    let states_orig = FluidDomainPayload::decode(
        update_orig
            .custom_payload
            .as_deref()
            .expect("orig payload Some"),
    )
    .expect("orig decode");
    let states_restored = FluidDomainPayload::decode(
        update_restored
            .custom_payload
            .as_deref()
            .expect("restored payload Some"),
    )
    .expect("restored decode");

    assert_eq!(states_orig.len(), 1);
    assert_eq!(states_restored.len(), 1);

    assert_eq!(
        states_orig[0].mean_supply_temp_c, states_restored[0].mean_supply_temp_c,
        "supply temp after restore must match original"
    );
    assert_eq!(
        states_orig[0].mean_return_temp_c, states_restored[0].mean_return_temp_c,
        "return temp after restore must match original"
    );

    // Also verify the restored temperatures match what was originally injected.
    let delta_supply = (states_restored[0].mean_supply_temp_c - supply).abs();
    let delta_ret = (states_restored[0].mean_return_temp_c - ret).abs();
    assert!(
        delta_supply < 1e-9,
        "restored supply_temp={} expected={supply}",
        states_restored[0].mean_supply_temp_c
    );
    assert!(
        delta_ret < 1e-9,
        "restored return_temp={} expected={ret}",
        states_restored[0].mean_return_temp_c
    );
}
