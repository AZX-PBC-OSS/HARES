//! Integration tests for state-space and RC network abstractions.
//!
//! These tests exercise the public API surface end-to-end — building RC networks,
//! assembling continuous matrices, discretizing, and stepping — rather than
//! duplicating the unit-level arithmetic checks already in the source modules.

use std::collections::HashMap;

use hares_envelope::{
    NodeId, OutputMapping, RCNetwork, StateSpaceModel, discretize_zoh, eigenvalue_check,
    matrix_exp, parallel_resistance,
};
use nalgebra::{DMatrix, DVector};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn node(id: u32) -> NodeId {
    NodeId(id)
}

fn assert_close(actual: f64, expected: f64, tol: f64, label: &str) {
    assert!(
        (actual - expected).abs() <= tol,
        "{label}: actual={actual}, expected={expected}, delta={}",
        (actual - expected).abs()
    );
}

fn assert_matrix_close(actual: &DMatrix<f64>, expected: &DMatrix<f64>, tol: f64) {
    assert_eq!(
        actual.shape(),
        expected.shape(),
        "matrix shape mismatch: actual {:?} vs expected {:?}",
        actual.shape(),
        expected.shape()
    );
    for r in 0..actual.nrows() {
        for c in 0..actual.ncols() {
            let delta = (actual[(r, c)] - expected[(r, c)]).abs();
            assert!(
                delta <= tol,
                "matrix entry ({r},{c}): actual={}, expected={}, delta={delta}",
                actual[(r, c)],
                expected[(r, c)]
            );
        }
    }
}

/// Builds a simple 1R-1C RC network and returns the discrete state-space model
/// with the single interior node mapped to output 0.
fn build_1r1c_model(r: f64, c: f64, dt: f64) -> StateSpaceModel {
    let caps = HashMap::from([(node(1), c)]);
    let res = HashMap::from([((node(1), node(2)), r)]);
    let net = RCNetwork::from_elements(caps, res, vec![node(2)]).expect("valid 1R-1C network");
    let (a_c, b_c) = net.build_matrices().expect("1R-1C matrices");

    let mapping = OutputMapping {
        output_count: 1,
        node_to_output: vec![(0, 0, 1.0)],
        input_to_output: vec![],
    };
    StateSpaceModel::from_continuous(&a_c, &b_c, dt, &mapping)
        .expect("1R-1C state-space model should be stable")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// A node initially at T₀ with no heat input and ambient at 0 °C must decay
/// exponentially toward 0 °C.  After N steps of dt, the analytic solution is:
///   T(t) = T₀ × exp(−t / τ),  τ = R × C.
#[test]
fn single_node_rc_passive_decay() {
    let r = 2.0;
    let c = 500.0;
    let dt = 30.0;
    let t0 = 25.0;
    let n_steps = 200usize;

    let model = build_1r1c_model(r, c, dt);

    let mut x = DVector::from_row_slice(&[t0]);
    let u = DVector::from_row_slice(&[0.0]); // ambient = 0 °C

    for _ in 0..n_steps {
        x = model.step(&x, &u);
    }

    let t_elapsed = dt * n_steps as f64;
    let tau = r * c;
    let expected = t0 * (-t_elapsed / tau).exp();

    // Crank-Nicolson is O(dt²) per step, so accumulated error over 200 steps
    // is small but not at machine epsilon. For dt=30, tau=1000: error ~ O(dt²/tau²) per step.
    assert_close(x[0], expected, 1e-4, "temperature after decay");
    assert!(
        x[0] > 0.0,
        "temperature must still be positive (decaying not reversed)"
    );
    assert!(x[0] < t0, "temperature must be lower than initial");
}

/// With a constant heat injection Q [W] and fixed ambient, the steady-state
/// temperature satisfies Ohm's thermal law: T_ss = T_ambient + Q × R.
#[test]
fn steady_state_matches_ohms_law() {
    let r = 3.0; // K/W
    let c = 1_000.0; // J/K
    let q = 50.0; // W
    let t_ambient = 20.0; // °C
    let dt = 60.0;

    // State: one interior node (node 1).
    // Input 0 = ambient temperature; input 1 = injected heat power [W] normalised by C.
    // We need a model where u = [T_ambient, Q_normalized].
    // Build 1R-1C and add a direct heat injection port at the node.
    let caps = HashMap::from([(node(1), c)]);
    let res = HashMap::from([((node(1), node(2)), r)]);
    let net = RCNetwork::from_elements(caps, res, vec![node(2)]).expect("valid 1R-1C network");
    let (a_c, b_c_base) = net.build_matrices().expect("1R-1C matrices");

    // Extend B_c with a second input column for direct heat injection: dT/dt += Q/C.
    // b_c_base is 1×1 (one state, one external node).  Append a column for heat injection.
    let mut b_c = DMatrix::zeros(1, 2);
    b_c[(0, 0)] = b_c_base[(0, 0)]; // ambient coupling
    b_c[(0, 1)] = 1.0 / c; // direct injection: 1 W → 1/C K/s

    let mapping = OutputMapping {
        output_count: 1,
        node_to_output: vec![(0, 0, 1.0)],
        input_to_output: vec![],
    };

    let model = StateSpaceModel::from_continuous(&a_c, &b_c, dt, &mapping)
        .expect("state-space model must build");

    // Drive to steady state: 10 × τ = 10 × R × C steps at dt = 60 s.
    let tau_steps = ((10.0 * r * c) / dt).ceil() as usize;
    let u = DVector::from_row_slice(&[t_ambient, q]);
    let mut x = DVector::from_row_slice(&[t_ambient]); // start at ambient

    for _ in 0..tau_steps {
        x = model.step(&x, &u);
    }

    let expected_ss = t_ambient + q * r;
    // After 10τ the error is exp(-10) ≈ 4.5e-5 of the total range; well within 0.05 K.
    assert_close(x[0], expected_ss, 0.05, "steady-state temperature");
}

/// The RC time constant τ = R × C is defined as the time at which the step
/// response reaches (1 − 1/e) ≈ 63.2 % of its final value.  Verify this
/// from a cold start driven to steady-state, measuring the fraction reached
/// after exactly τ seconds.
#[test]
fn time_constant_matches_rc() {
    let r = 4.0;
    let c = 250.0;
    let tau = r * c;
    let dt = 1.0; // small dt for step accuracy near τ
    let t_ambient = 30.0;

    let model = build_1r1c_model(r, c, dt);

    let u = DVector::from_row_slice(&[t_ambient]);
    let mut x = DVector::from_row_slice(&[0.0]); // start at 0 °C

    let tau_steps = tau.round() as usize; // dt = 1 s, so steps ≈ τ
    for _ in 0..tau_steps {
        x = model.step(&x, &u);
    }

    // Expected fraction of final value at t = τ.
    let fraction_reached = x[0] / t_ambient;
    let target = 1.0 - (-1.0_f64).exp(); // ≈ 0.6321

    // ZOH is exact for piecewise-constant inputs; 0.5 % tolerance.
    assert!(
        (fraction_reached - target).abs() < 0.005,
        "fraction at τ={tau}: actual={fraction_reached:.6}, expected≈{target:.6}"
    );
}

/// A properly constructed RC model must produce all discrete eigenvalues with
/// |λ| < 1 (strictly stable), confirmed by `eigenvalue_check`.
#[test]
fn eigenvalue_check_stable_model() {
    let r = 2.0;
    let c = 1_000.0;
    let dt = 60.0;

    let caps = HashMap::from([(node(1), c)]);
    let res = HashMap::from([((node(1), node(2)), r)]);
    let net = RCNetwork::from_elements(caps, res, vec![node(2)]).expect("valid 1R-1C network");
    let (a_c, b_c) = net.build_matrices().expect("matrices");

    let (a_d, _) = discretize_zoh(&a_c, &b_c, dt).expect("ZOH discretization");
    let result = eigenvalue_check(&a_c, &a_d).expect("stable 1R-1C must pass eigenvalue_check");

    assert!(
        result.continuous_stable,
        "continuous eigenvalue must be negative real"
    );
    assert!(
        result.discrete_stable,
        "discrete eigenvalue must have |λ| < 1"
    );
}

/// For a 2-node RC network (two internal nodes sharing a resistor, each
/// connected to a separate boundary), eigenvalue_check must still pass
/// and both discrete eigenvalues must be in (0, 1).
#[test]
fn two_node_rc_both_eigenvalues_stable() {
    // Node 1 (C=500) ← R12=1 → Node 2 (C=200)
    // Node 1 ← R1ext=2 → ext node 10
    // Node 2 ← R2ext=3 → ext node 11
    let caps = HashMap::from([(node(1), 500.0), (node(2), 200.0)]);
    let res = HashMap::from([
        ((node(1), node(2)), 1.0),
        ((node(1), node(10)), 2.0),
        ((node(2), node(11)), 3.0),
    ]);
    let net =
        RCNetwork::from_elements(caps, res, vec![node(10), node(11)]).expect("valid 2-node RC");
    let (a_c, b_c) = net.build_matrices().expect("matrices");

    let dt = 30.0;
    let (a_d, _) = discretize_zoh(&a_c, &b_c, dt).expect("ZOH discretization");
    let result = eigenvalue_check(&a_c, &a_d).expect("2-node stable RC must pass eigenvalue_check");

    assert!(
        result.continuous_stable,
        "all continuous eigenvalues must be negative real"
    );
    assert!(
        result.discrete_stable,
        "all discrete eigenvalues must have |λ| < 1"
    );

    let eigs = a_d.complex_eigenvalues();
    for (i, lambda) in eigs.iter().enumerate() {
        assert!(
            lambda.norm() < 1.0,
            "discrete eigenvalue {i}: |λ|={} must be < 1",
            lambda.norm()
        );
        assert!(
            lambda.norm() > 0.0,
            "discrete eigenvalue {i}: |λ|={} must be > 0 (RC decay, not dead-beat)",
            lambda.norm()
        );
    }
}

/// `parallel_resistance(R1, R2)` must equal R1×R2/(R1+R2) exactly for
/// representative values, including the limit cases of equal and very
/// asymmetric resistors.
#[test]
fn parallel_resistance_calculation() {
    // Exact rational: 3 ∥ 6 = 18/9 = 2
    assert_close(
        parallel_resistance(3.0, 6.0),
        2.0,
        f64::EPSILON * 4.0,
        "3 ∥ 6",
    );

    // 1 ∥ 1 = 0.5
    assert_close(
        parallel_resistance(1.0, 1.0),
        0.5,
        f64::EPSILON * 2.0,
        "1 ∥ 1",
    );

    // Very different values: 1 ∥ 1000 ≈ 0.999
    let expected_asymmetric = 1.0 * 1_000.0 / (1.0 + 1_000.0);
    assert_close(
        parallel_resistance(1.0, 1_000.0),
        expected_asymmetric,
        f64::EPSILON * 4.0,
        "1 ∥ 1000",
    );

    // Commutativity: R1 ∥ R2 = R2 ∥ R1 to within floating-point rounding.
    assert_close(
        parallel_resistance(3.0, 7.0),
        parallel_resistance(7.0, 3.0),
        f64::EPSILON * 4.0,
        "parallel_resistance commutativity",
    );
}

/// `matrix_exp(0)` must return the identity matrix.  The Padé implementation
/// short-circuits on zero norm, so this also validates that branch.
#[test]
fn matrix_exponential_identity() {
    let zero_2x2 = DMatrix::<f64>::zeros(2, 2);
    let result = matrix_exp(&zero_2x2).unwrap();
    let expected = DMatrix::<f64>::identity(2, 2);
    assert_matrix_close(&result, &expected, f64::EPSILON * 4.0);

    // Larger zero matrix
    let zero_4x4 = DMatrix::<f64>::zeros(4, 4);
    let result4 = matrix_exp(&zero_4x4).unwrap();
    let expected4 = DMatrix::<f64>::identity(4, 4);
    assert_matrix_close(&result4, &expected4, f64::EPSILON * 4.0);
}

/// A continuous-time stable RC model must produce a discrete model that is
/// also stable regardless of the timestep, as long as dt is positive.
/// Tests several dt values to confirm stability is preserved under ZOH.
#[test]
fn discretization_preserves_stability() {
    let caps = HashMap::from([(node(1), 800.0), (node(2), 400.0)]);
    let res = HashMap::from([
        ((node(1), node(2)), 1.5),
        ((node(1), node(10)), 3.0),
        ((node(2), node(11)), 2.0),
    ]);
    let net =
        RCNetwork::from_elements(caps, res, vec![node(10), node(11)]).expect("valid 2-node RC");
    let (a_c, b_c) = net.build_matrices().expect("matrices");

    // Confirm continuous stability (negative real eigenvalues).
    let cont_eigs = a_c.clone().complex_eigenvalues();
    for (i, lambda) in cont_eigs.iter().enumerate() {
        assert!(
            lambda.re < 0.0,
            "continuous eigenvalue {i} must have negative real part, got re={}",
            lambda.re
        );
    }

    // ZOH discretization must yield |λ_d| < 1 for a range of timesteps.
    for &dt in &[1.0_f64, 30.0, 60.0, 300.0, 900.0] {
        let (a_d, _) = discretize_zoh(&a_c, &b_c, dt)
            .unwrap_or_else(|e| panic!("discretize_zoh failed for dt={dt}: {e}"));

        let disc_eigs = a_d.complex_eigenvalues();
        for (i, lambda) in disc_eigs.iter().enumerate() {
            assert!(
                lambda.norm() < 1.0,
                "dt={dt}: discrete eigenvalue {i} |λ|={} must be < 1",
                lambda.norm()
            );
        }
    }
}

/// With a stiff RC node (τ=10s, dt=300s) and high infiltration, the zone
/// temperature must converge toward outdoor temp when infiltration dominates.
///
/// The implicit coupling in the solver uses B_c (continuous input gain) rather
/// than B_d (ZOH-discretized). For stiff systems, B_d saturates toward the
/// steady-state gain and materially under-represents the physical conductance,
/// causing the coupled step to under-weight infiltration. Using B_c produces
/// the correct coupling strength.
#[test]
fn stiff_infiltration_coupling_uses_b_c_not_b_d() {
    // Stiff 1R-1C: τ = R·C = 10 s, dt = 300 s → dt/τ = 30 (deeply stiff).
    let r = 1.0; // K/W
    let c_cap = 10.0; // J/K  → τ = 10 s
    let dt = 300.0; // s

    let caps = HashMap::from([(node(1), c_cap)]);
    let res = HashMap::from([((node(1), node(2)), r)]);
    let net = RCNetwork::from_elements(caps, res, vec![node(2)]).expect("valid 1R-1C network");
    let (a_c, b_c) = net.build_matrices().expect("1R-1C matrices");

    let mapping = OutputMapping {
        output_count: 1,
        node_to_output: vec![(0, 0, 1.0)],
        input_to_output: vec![],
    };
    let model =
        StateSpaceModel::from_continuous(&a_c, &b_c, dt, &mapping).expect("should build");

    // Verify B_d and B_c differ materially for this stiff system.
    let b_c_val = model.b_c().expect("continuous-path model has B_c")[(0, 0)];
    let b_d_val = model.b_eff()[(0, 0)];
    assert!(
        (b_c_val - b_d_val).abs() / b_c_val.abs() > 0.5,
        "B_c and B_d should differ substantially for stiff system; \
         b_c={b_c_val}, b_d={b_d_val}"
    );

    // Simulate infiltration coupling like stepping.rs does:
    //   d = h_inf * b_coeff
    //   forcing = h_inf * t_outdoor * b_coeff + d * x[state_idx]
    let t_indoor_init = 22.0;
    let t_outdoor = 0.0;
    let h_inf = 50.0; // W/K — large infiltration conductance

    let state_idx = 0_usize;
    let mut x = DVector::from_row_slice(&[t_indoor_init]);
    let u = DVector::from_row_slice(&[t_outdoor]); // ambient already in u
    let mut buf = DVector::zeros(1);
    let mut m_scratch = DMatrix::zeros(1, 1);

    // Run 20 steps with B_c coupling (the correct path).
    for _ in 0..20 {
        let d = h_inf * b_c_val;
        let forcing = h_inf * t_outdoor * b_c_val + d * x[state_idx];
        let couplings = vec![(state_idx, d, forcing)];
        model.step_with_coupling_into(&x, &u, &mut buf, &mut m_scratch, &couplings);
        x.copy_from(&buf);
    }

    // With strong infiltration, zone temp must converge close to outdoor.
    assert!(
        (x[0] - t_outdoor).abs() < 1.0,
        "with B_c coupling, zone should converge toward outdoor; got T={:.2}",
        x[0]
    );

    // Now verify that using B_d (the old buggy path) fails to converge.
    let mut x_bad = DVector::from_row_slice(&[t_indoor_init]);
    for _ in 0..20 {
        let d = h_inf * b_d_val;
        let forcing = h_inf * t_outdoor * b_d_val + d * x_bad[state_idx];
        let couplings = vec![(state_idx, d, forcing)];
        model.step_with_coupling_into(&x_bad, &u, &mut buf, &mut m_scratch, &couplings);
        x_bad.copy_from(&buf);
    }

    // B_d under-weights infiltration for stiff systems, so temperature stays higher.
    assert!(
        (x_bad[0] - t_outdoor).abs() > 3.0,
        "with B_d coupling, zone should NOT converge as well; got T={:.2} (too close to outdoor)",
        x_bad[0]
    );
}

/// Building a `StateSpaceModel` via the full RC → continuous → discrete
/// pipeline and then stepping it must match the analytic step-response formula
/// for a simple 1R-1C circuit after an arbitrary number of steps.
/// This validates the end-to-end integration across all three public modules.
#[test]
fn rc_network_to_state_space_pipeline_matches_analytic() {
    let r = 1.0;
    let c = 1_000.0;
    let dt = 60.0;
    let t0 = 20.0;
    let t_ambient = 5.0;
    let n_steps = 50usize;

    let model = build_1r1c_model(r, c, dt);
    let mut x = DVector::from_row_slice(&[t0]);
    let u = DVector::from_row_slice(&[t_ambient]);

    for _ in 0..n_steps {
        x = model.step(&x, &u);
    }

    let t_elapsed = dt * n_steps as f64;
    let tau = r * c;
    // Analytic: T(t) = T_amb + (T₀ − T_amb) × exp(−t/τ)
    let expected = t_ambient + (t0 - t_ambient) * (-t_elapsed / tau).exp();

    assert_close(x[0], expected, 0.01, "end-to-end pipeline temperature");
}
