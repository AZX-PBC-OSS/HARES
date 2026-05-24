//! Synthetic box analytical validation tests for the thermal solver.
//!
//! These tests verify the implicit Crank-Nicolson solver against analytical
//! solutions for simple RC geometries. They serve as a permanent regression
//! guard for the thermal solver core.

use std::collections::HashMap;
use std::time::Duration;

use chrono::{FixedOffset, TimeZone};
use hares_envelope::{
    InfiltrationMethod, OutputMapping, StateSpaceModel, StateSpaceWiring, ThermalSolver,
    ThermalSolverConfig, discretize_auto, matrix_exp,
};
use hares_types::{
    DomainSolver, EnvironmentState, GridState, PortSlots, ThermalAccumulator, WeatherState, ZoneId,
    ZoneState,
};
use nalgebra::{DMatrix, DVector};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const ZONE: ZoneId = ZoneId(1);
const UA: f64 = 20.0; // W/K
const C: f64 = 200_000.0; // J/K
const DT_S: f64 = 60.0; // s
const STEPS_24H: usize = 1440; // 24h / 60s

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn one_zone_env(zone_temp_c: f64, outdoor_temp_c: f64, volume_m3: f64) -> EnvironmentState {
    EnvironmentState {
        zones: vec![ZoneState {
            id: ZONE,
            temperature_c: zone_temp_c,
            humidity_ratio: 0.008,
            relative_humidity: 0.45,
            wet_bulb_c: zone_temp_c - 5.0,
            volume_m3,
        }],
        weather: WeatherState {
            outdoor_temp_c,
            outdoor_humidity_ratio: 0.004,
            outdoor_wet_bulb_c: outdoor_temp_c - 5.0,
            outdoor_enthalpy_j_kg: 22_800.0,
            wind_speed_m_s: 0.0,
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

fn build_1r1c_solver(
    env: &EnvironmentState,
    indoor_temp_c: f64,
    config: ThermalSolverConfig,
    ua: f64,
    cap: f64,
) -> ThermalSolver {
    let a_c = DMatrix::from_row_slice(1, 1, &[-ua / cap]);
    let b_c = DMatrix::from_row_slice(1, 2, &[ua / cap, 1.0 / cap]);

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
        c_zone_j_k: HashMap::new(),
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

/// 1R1C exponential decay: zone at 20 C, outdoor at 0 C, no HVAC.
/// Analytical: T(t) = T_out + (T0 - T_out) * exp(-t / RC).
#[test]
fn test_1r1c_exponential_decay() {
    let t_initial = 20.0;
    let t_outdoor = 0.0;
    let tau = C / UA; // 10000 s

    let mut env = one_zone_env(t_initial, t_outdoor, 200.0);
    let config = ThermalSolverConfig {
        indoor_zone_id: ZONE,
        ..ThermalSolverConfig::default()
    };

    let mut solver = build_1r1c_solver(&env, t_initial, config, UA, C);
    let ports = one_zone_ports();

    let mut t_zone = t_initial;

    for step in 1..=STEPS_24H {
        let update = solver.resolve_new(&ports, &env, Duration::from_secs(DT_S as u64));
        t_zone = zone_temp(&update);
        env.zones[0].temperature_c = t_zone;

        // Check every 60 steps (hourly)
        if step % 60 == 0 {
            let t_s = step as f64 * DT_S;
            let t_analytical = t_outdoor + (t_initial - t_outdoor) * (-t_s / tau).exp();
            let err = (t_zone - t_analytical).abs();
            assert!(
                err < 0.05,
                "step {step} (t={t_s}s): HARES={t_zone:.6}, analytical={t_analytical:.6}, err={err:.6} exceeds 0.05°C"
            );
        }
    }

    // Final temperature should be near outdoor (T(24h) ~ 0.003°C)
    let t_final_analytical =
        t_outdoor + (t_initial - t_outdoor) * (-(STEPS_24H as f64 * DT_S) / tau).exp();
    assert!(
        (t_zone - t_final_analytical).abs() < 0.05,
        "final: HARES={t_zone:.6}, analytical={t_final_analytical:.6}"
    );
}

/// 1R1C with 500 W constant solar gain, no HVAC. Outdoor 0 C.
/// Steady-state: T_ss = T_out + Q_solar * R = 0 + 500 * 0.05 = 25 C.
/// At 5tau the analytical step response is T_out + Q/UA * (1 - exp(-5)) ~ 24.83 C.
/// Run 7tau to get within the tolerance.
#[test]
fn test_1r1c_solar_step_response() {
    let t_initial = 0.0;
    let t_outdoor = 0.0;
    let q_solar = 500.0;
    let tau = C / UA; // 10000 s

    let mut env = one_zone_env(t_initial, t_outdoor, 200.0);
    let config = ThermalSolverConfig {
        indoor_zone_id: ZONE,
        ..ThermalSolverConfig::default()
    };

    let mut solver = build_1r1c_solver(&env, t_initial, config, UA, C);
    let mut ports = one_zone_ports();

    // Run 7tau to ensure we're very close to steady state
    let steps = (7.0 * tau / DT_S).ceil() as usize;

    let mut t_zone = t_initial;
    for _ in 0..steps {
        ports.thermal[0] = ThermalAccumulator::new(ZONE);
        ports.thermal[0].sensible_gain_w += q_solar;

        let update = solver.resolve_new(&ports, &env, Duration::from_secs(DT_S as u64));
        t_zone = zone_temp(&update);
        env.zones[0].temperature_c = t_zone;
    }

    let t_ss_expected = t_outdoor + q_solar / UA; // 25 C
    assert!(
        (t_zone - t_ss_expected).abs() < 0.1,
        "after 7tau: HARES={t_zone:.4}°C, expected ~{t_ss_expected}°C"
    );
}

/// 1R1C with moderate infiltration (ACH=0.05, V=400 m³), free cooling.
/// The combined UA (envelope + infiltration) produces a faster time constant than
/// envelope alone. Verify the free-response decay matches analytical with combined UA.
///
/// UA_inf = ρ·cp·ACH·V/3600 ≈ 1207 × 0.05 × 400 / 3600 ≈ 6.71 W/K.
/// τ_combined = C / (UA + UA_inf) = 200000 / 26.71 ≈ 7490s.
/// At 5000s: T ≈ 20 × exp(-5000/7490) ≈ 10.26°C.
/// Without infiltration: T ≈ 20 × exp(-5000/10000) ≈ 12.13°C.
#[test]
fn test_1r1c_with_moderate_infiltration() {
    let t_initial = 20.0;
    let t_outdoor = 0.0;
    let volume_m3 = 400.0;
    let ach = 0.05;

    // Solver WITH infiltration
    let mut env_inf = one_zone_env(t_initial, t_outdoor, volume_m3);
    let config_inf = ThermalSolverConfig {
        indoor_zone_id: ZONE,
        infiltration: vec![(ZONE, InfiltrationMethod::Ach { ach })],
        ..ThermalSolverConfig::default()
    };
    let mut solver_inf = build_1r1c_solver(&env_inf, t_initial, config_inf, UA, C);

    // Solver WITHOUT infiltration
    let mut env_no = one_zone_env(t_initial, t_outdoor, volume_m3);
    let config_no = ThermalSolverConfig {
        indoor_zone_id: ZONE,
        ..ThermalSolverConfig::default()
    };
    let mut solver_no = build_1r1c_solver(&env_no, t_initial, config_no, UA, C);

    let ports = one_zone_ports();
    let check_step = 83; // ~5000s

    let mut t_with_inf = t_initial;
    let mut t_without_inf = t_initial;

    for _ in 0..check_step {
        let u1 = solver_inf.resolve_new(&ports, &env_inf, Duration::from_secs(DT_S as u64));
        t_with_inf = zone_temp(&u1);
        env_inf.zones[0].temperature_c = t_with_inf;

        let u2 = solver_no.resolve_new(&ports, &env_no, Duration::from_secs(DT_S as u64));
        t_without_inf = zone_temp(&u2);
        env_no.zones[0].temperature_c = t_without_inf;
    }

    // Infiltration must accelerate cooling
    assert!(
        t_with_inf < t_without_inf,
        "infiltration must accelerate cooling: with={t_with_inf:.2}, without={t_without_inf:.2}"
    );

    // Verify against analytical combined-UA decay
    let rho_cp = 1207.2; // W·s/(m³·K)
    let ua_inf = rho_cp * ach * volume_m3 / 3600.0;
    let tau_combined = C / (UA + ua_inf);
    let t_analytical = t_initial * (-(check_step as f64 * DT_S) / tau_combined).exp();
    assert!(
        (t_with_inf - t_analytical).abs() < 2.0,
        "with-infiltration T={t_with_inf:.2}°C differs from analytical {t_analytical:.2}°C by > 2°C"
    );
}

/// Extreme infiltration (ACH=50, V=200 m³) with low thermal mass (C=50 kJ/K).
/// UA_inf = 1207 × 50 × 200 / 3600 ≈ 3353 W/K. Combined with UA=20 → UA_total ≈ 3373 W/K.
/// τ = 50000 / 3373 ≈ 14.8s. With dt=60s, dt/τ ≈ 4 -- explicit Euler eigenvalue would be
/// 1 - dt·UA/C = 1 - 60·3373/50000 = -3.05 (magnitude > 1, unstable).
/// The CN solver must keep the zone bounded in [-10, 20]°C and converge monotonically.
#[test]
fn test_implicit_stability_extreme_ach() {
    let t_initial = 20.0;
    let t_outdoor = -10.0;
    let volume_m3 = 200.0;
    let ach = 50.0;
    let c_low = 50_000.0; // Low thermal mass to make τ < dt/2

    let mut env = one_zone_env(t_initial, t_outdoor, volume_m3);
    let config = ThermalSolverConfig {
        indoor_zone_id: ZONE,
        infiltration: vec![(ZONE, InfiltrationMethod::Ach { ach })],
        ..ThermalSolverConfig::default()
    };

    let mut solver = build_1r1c_solver(&env, t_initial, config, UA, c_low);
    let ports = one_zone_ports();

    // Step 1: assert zone stays in [-10, 20]°C
    let update = solver.resolve_new(&ports, &env, Duration::from_secs(DT_S as u64));
    let t_step1 = zone_temp(&update);
    env.zones[0].temperature_c = t_step1;
    assert!(t_step1.is_finite(), "step 1: temperature is NaN/Inf");
    assert!(
        t_step1 >= t_outdoor && t_step1 <= t_initial,
        "step 1: zone {t_step1}°C must be in [{t_outdoor}, {t_initial}]"
    );

    // Steps 2-20: assert monotonic convergence toward outdoor (no oscillation)
    let mut temps = vec![t_step1];
    for _ in 1..20 {
        let update = solver.resolve_new(&ports, &env, Duration::from_secs(DT_S as u64));
        let t = zone_temp(&update);
        env.zones[0].temperature_c = t;
        temps.push(t);
    }

    for (i, &t) in temps.iter().enumerate() {
        assert!(t.is_finite(), "step {}: temperature is NaN/Inf", i + 1);
    }

    // Monotonicity: each step should be closer to outdoor than the previous
    for window in temps.windows(2) {
        let dist_prev = (window[0] - t_outdoor).abs();
        let dist_next = (window[1] - t_outdoor).abs();
        assert!(
            dist_next <= dist_prev + 0.01, // small tolerance for floating point
            "non-monotonic: {:.4}°C → {:.4}°C (outdoor={t_outdoor})",
            window[0],
            window[1]
        );
    }

    // Final temperature should be near outdoor
    let t_final = *temps.last().unwrap();
    assert!(
        (t_final - t_outdoor).abs() < 1.0,
        "after 20 steps with extreme ACH, zone should be near outdoor: T={t_final}, outdoor={t_outdoor}"
    );
}

/// Compare Crank-Nicolson (from_continuous) vs matrix-exponential (discretize_auto + from_discrete).
/// Both should reach the same steady state within 0.01 C and track within 0.1 C during transient.
#[test]
fn test_implicit_vs_explicit_agreement_stable_case() {
    let a_c = DMatrix::from_row_slice(1, 1, &[-UA / C]);
    let b_c = DMatrix::from_row_slice(1, 2, &[UA / C, 1.0 / C]);

    let mapping = OutputMapping {
        output_count: 1,
        node_to_output: vec![(0, 0, 1.0)],
        input_to_output: vec![],
    };

    // CN model
    let cn_model = StateSpaceModel::from_continuous(&a_c, &b_c, DT_S, &mapping).expect("CN model");

    // Matrix-exponential model
    let (a_d, b_d) = discretize_auto(&a_c, &b_c, DT_S).expect("discretize_auto");
    let c_mat = DMatrix::from_row_slice(1, 1, &[1.0]);
    let d_mat = DMatrix::zeros(1, 2);
    let exp_model = StateSpaceModel::from_discrete(a_d, b_d, c_mat, d_mat).expect("discrete model");

    let t_initial = 20.0;
    let t_outdoor = 0.0;
    let mut x_cn = DVector::from_element(1, t_initial);
    let mut x_exp = DVector::from_element(1, t_initial);
    let u = DVector::from_column_slice(&[t_outdoor, 0.0]);

    let mut buf_cn = DVector::zeros(1);
    let mut buf_exp = DVector::zeros(1);

    let steps = 1000;
    for step in 0..steps {
        cn_model.step_into(&x_cn, &u, &mut buf_cn);
        exp_model.step_into(&x_exp, &u, &mut buf_exp);
        std::mem::swap(&mut x_cn, &mut buf_cn);
        std::mem::swap(&mut x_exp, &mut buf_exp);

        let diff = (x_cn[0] - x_exp[0]).abs();
        assert!(
            diff < 0.1,
            "step {step}: CN={:.6}, exp={:.6}, diff={diff:.6} exceeds 0.1°C transient tolerance",
            x_cn[0],
            x_exp[0]
        );
    }

    // After 1000 steps of 60s = 60000s with tau=10000s, both should be near 0°C
    let diff_final = (x_cn[0] - x_exp[0]).abs();
    assert!(
        diff_final < 0.01,
        "steady-state: CN={:.6}, exp={:.6}, diff={diff_final:.6} exceeds 0.01°C",
        x_cn[0],
        x_exp[0]
    );
}

/// 2R2C eigenvalue verification: compare CN A_d eigenvalues against matrix_exp eigenvalues.
#[test]
fn test_2r2c_eigenvalue_verification() {
    let r1: f64 = 0.05; // K/W
    let r2: f64 = 0.1;
    let c1: f64 = 100_000.0; // J/K
    let c2: f64 = 200_000.0;

    let a11 = -1.0 / (r1 * c1) - 1.0 / (r2 * c1);
    let a12 = 1.0 / (r2 * c1);
    let a21 = 1.0 / (r2 * c2);
    let a22 = -1.0 / (r2 * c2);
    let a_c = DMatrix::from_row_slice(2, 2, &[a11, a12, a21, a22]);

    let b_c = DMatrix::from_row_slice(2, 2, &[1.0 / (r1 * c1), 0.0, 0.0, 1.0 / c2]);

    let mapping = OutputMapping {
        output_count: 2,
        node_to_output: vec![(0, 0, 1.0), (1, 1, 1.0)],
        input_to_output: vec![],
    };

    let cn_model =
        StateSpaceModel::from_continuous(&a_c, &b_c, DT_S, &mapping).expect("2R2C CN model");

    // Compute exact A_d via matrix exponential
    let a_c_dt = &a_c * DT_S;
    let a_d_exact = matrix_exp(&a_c_dt).expect("matrix_exp");

    // Compute CN A_d = M^{-1} N
    let eye = DMatrix::<f64>::identity(2, 2);
    let half_dt_a = &a_c * (DT_S / 2.0);
    let m = &eye - &half_dt_a;
    let n = &eye + &half_dt_a;
    let m_inv = m.try_inverse().expect("M must be invertible");
    let a_d_cn = m_inv * n;

    let eigs_cn = a_d_cn.complex_eigenvalues();
    let eigs_exact = a_d_exact.complex_eigenvalues();

    let mut cn_mags: Vec<f64> = eigs_cn.iter().map(|e| e.norm()).collect();
    let mut exact_mags: Vec<f64> = eigs_exact.iter().map(|e| e.norm()).collect();
    cn_mags.sort_by(|a, b| a.partial_cmp(b).unwrap());
    exact_mags.sort_by(|a, b| a.partial_cmp(b).unwrap());

    for (i, (cn_mag, exact_mag)) in cn_mags.iter().zip(exact_mags.iter()).enumerate() {
        let diff = (cn_mag - exact_mag).abs();
        assert!(
            diff < 1e-4,
            "eigenvalue {i}: CN magnitude={cn_mag:.8}, exact magnitude={exact_mag:.8}, diff={diff:.8} exceeds 1e-4"
        );
    }

    let _verdict = cn_model
        .verify_stability()
        .expect("stability check must not error");
}

/// Near-zero thermal mass: C=1 J/K, UA=20 W/K, dt=60s.
/// tau = 0.05s, dt/tau = 1200. The CN discretization is unconditionally
/// A-stable (no amplitude growth), but for extreme dt/tau the discrete
/// eigenvalue approaches -1, causing damped sign-alternating convergence
/// toward steady state. We verify: no NaN, bounded amplitude, convergence.
#[test]
fn test_near_zero_thermal_mass() {
    let t_initial = 20.0;
    let t_outdoor = 0.0;
    let cap = 1.0; // J/K

    // Test at the state-space level (not ThermalSolver) to isolate CN behavior
    let a_c = DMatrix::from_row_slice(1, 1, &[-UA / cap]);
    let b_c = DMatrix::from_row_slice(1, 2, &[UA / cap, 1.0 / cap]);

    let mapping = OutputMapping {
        output_count: 1,
        node_to_output: vec![(0, 0, 1.0)],
        input_to_output: vec![],
    };

    let model =
        StateSpaceModel::from_continuous(&a_c, &b_c, DT_S, &mapping).expect("near-zero C model");

    let mut x = DVector::from_element(1, t_initial);
    let u = DVector::from_column_slice(&[t_outdoor, 0.0]);
    let mut buf = DVector::zeros(1);

    // CN eigenvalue: (1 + dt/2 * a) / (1 - dt/2 * a) where a = -UA/C = -20
    // = (1 - 600) / (1 + 600) = -599/601 ~ -0.9967
    // The magnitude < 1 guarantees convergence, but sign alternation occurs.
    // CN eigenvalue magnitude ~ 0.9967, so convergence is slow in step count.
    // Need ~1600 steps for |T| < 0.1. Run 2000 to be safe.
    let total_steps = 2000;
    let mut converged = false;

    for step in 0..total_steps {
        model.step_into(&x, &u, &mut buf);
        std::mem::swap(&mut x, &mut buf);

        assert!(
            x[0].is_finite(),
            "step {step}: temperature is not finite: {}",
            x[0]
        );

        // Amplitude must be bounded by initial magnitude (no growth)
        assert!(
            x[0].abs() <= t_initial + 0.01,
            "step {step}: |T|={} exceeds initial amplitude {}",
            x[0].abs(),
            t_initial
        );

        // Check convergence to steady state (0°C)
        if x[0].abs() < 0.1 {
            converged = true;
        }
    }

    assert!(
        converged,
        "near-zero thermal mass should converge to steady state within {} steps, final T={}",
        total_steps, x[0]
    );
}

/// Zero-allocation hot loop: verify step_into produces identical results
/// to the allocating step() path, confirming it uses only pre-allocated buffers.
/// The actual zero-allocation property is tested in synthetic_box_zero_alloc.rs
/// with a counting global allocator in an isolated binary.
#[test]
fn test_step_into_zero_allocations() {
    let a_c = DMatrix::from_row_slice(1, 1, &[-UA / C]);
    let b_c = DMatrix::from_row_slice(1, 2, &[UA / C, 1.0 / C]);

    let mapping = OutputMapping {
        output_count: 1,
        node_to_output: vec![(0, 0, 1.0)],
        input_to_output: vec![],
    };

    let model = StateSpaceModel::from_continuous(&a_c, &b_c, DT_S, &mapping).expect("model");

    let mut x = DVector::from_element(1, 20.0);
    let u = DVector::from_column_slice(&[0.0, 0.0]);
    let mut buf = DVector::zeros(1);

    let mut x_alloc = DVector::from_element(1, 20.0);

    for step in 0..100 {
        model.step_into(&x, &u, &mut buf);
        let x_alloc_next = model.step(&x_alloc, &u);

        let diff = (buf[0] - x_alloc_next[0]).abs();
        assert!(
            diff < 1e-12,
            "step {step}: step_into and step disagree: {:.15} vs {:.15}",
            buf[0],
            x_alloc_next[0]
        );

        std::mem::swap(&mut x, &mut buf);
        x_alloc = x_alloc_next;
    }

    // After 100 steps at 60s with u=[0,0], the system decays from the A-matrix only.
    // With tau=10000s, after 6000s: T ~ 20 * exp(-0.6) ~ 10.98. Verify reasonable decay.
    assert!(
        x[0] < 15.0,
        "after 100 steps, temperature should have decayed from 20, got {}",
        x[0]
    );
    assert!(
        x[0] > 5.0,
        "after 100 steps (6000s, tau=10000s), temperature should not have fully decayed, got {}",
        x[0]
    );
}
