use std::collections::HashMap;

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use hares_envelope::{
    NodeId, OutputMapping, RCNetwork, StateSpaceModel, discretize_zoh, van_loan_discretize,
};
use nalgebra::{DMatrix, DVector};

/// Build a 6-node RC network representing a typical single-zone house.
///
/// Topology (representative, not a specific building):
///
///   ext ─ R_wall ─ N1(C_wall_out) ─ R_wall_in ─ N2(C_wall_in)
///                                                    │
///   ext ─ R_roof ─ N3(C_roof_out) ─ R_roof_in ─ N4(C_roof_in)
///                                                    │
///                          N5(C_slab) ─ R_slab ─ N6(C_zone) ─ R_inf ─ ext
///                                                 ├── N2 ─ R_conv ──┘
///                                                 └── N4 ─ R_conv ──┘
///
/// 6 internal nodes, 1 external boundary (outdoor air temperature).
fn build_6node_network() -> RCNetwork {
    // Typical residential values -- representative geometry, not HPXML-derived.
    // Capacitances in J/K, resistances in K/W.
    let caps = HashMap::from([
        (NodeId(1), 150_000.0), // wall outer layer
        (NodeId(2), 80_000.0),  // wall inner layer
        (NodeId(3), 120_000.0), // roof outer layer
        (NodeId(4), 60_000.0),  // roof inner layer
        (NodeId(5), 200_000.0), // slab / floor
        (NodeId(6), 45_000.0),  // conditioned zone air
    ]);

    // NodeId(100) = outdoor air boundary (external, no capacitance)
    let res = HashMap::from([
        ((NodeId(1), NodeId(100)), 0.15), // wall outer boundary
        ((NodeId(1), NodeId(2)), 1.20),   // wall conduction
        ((NodeId(2), NodeId(6)), 0.08),   // wall inner convection
        ((NodeId(3), NodeId(100)), 0.12), // roof outer boundary
        ((NodeId(3), NodeId(4)), 1.80),   // roof conduction
        ((NodeId(4), NodeId(6)), 0.10),   // roof inner convection
        ((NodeId(5), NodeId(6)), 0.50),   // slab conduction to zone
        ((NodeId(5), NodeId(100)), 3.00), // slab ground coupling
        ((NodeId(6), NodeId(100)), 0.40), // infiltration + window
    ]);

    RCNetwork::from_elements(caps, res, vec![NodeId(100)]).expect("valid 6-node network")
}

/// Build a 12-node RC network representing a complex multi-zone building.
///
/// Two thermal zones (conditioned + unconditioned attic) each with wall/roof
/// mass layers, coupled via ceiling assembly.
fn build_12node_network() -> RCNetwork {
    // Zone 1 (conditioned): nodes 1–5 (walls + zone air)
    // Zone 2 (attic):       nodes 6–11 (roof + attic air)
    // Shared ceiling:       node 12 links zone 1 to zone 2
    // External boundary:    NodeId(200)

    let caps = HashMap::from([
        (NodeId(1), 160_000.0), // zone 1 wall outer
        (NodeId(2), 85_000.0),  // zone 1 wall inner
        (NodeId(3), 200_000.0), // zone 1 slab
        (NodeId(4), 50_000.0),  // zone 1 air
        (NodeId(5), 70_000.0),  // ceiling outer (shared)
        (NodeId(6), 40_000.0),  // ceiling inner
        (NodeId(7), 130_000.0), // attic roof outer
        (NodeId(8), 65_000.0),  // attic roof inner
        (NodeId(9), 180_000.0), // attic floor / deck
        (NodeId(10), 55_000.0), // attic wall outer
        (NodeId(11), 30_000.0), // attic wall inner
        (NodeId(12), 25_000.0), // attic air
    ]);

    let res = HashMap::from([
        // Zone 1 walls
        ((NodeId(1), NodeId(200)), 0.15),
        ((NodeId(1), NodeId(2)), 1.20),
        ((NodeId(2), NodeId(4)), 0.08),
        // Zone 1 slab
        ((NodeId(3), NodeId(200)), 3.00),
        ((NodeId(3), NodeId(4)), 0.50),
        // Zone 1 infiltration + windows
        ((NodeId(4), NodeId(200)), 0.40),
        // Ceiling (zone1 ↔ attic)
        ((NodeId(4), NodeId(5)), 0.10),
        ((NodeId(5), NodeId(6)), 0.90),
        ((NodeId(6), NodeId(12)), 0.10),
        // Attic roof
        ((NodeId(7), NodeId(200)), 0.12),
        ((NodeId(7), NodeId(8)), 1.80),
        ((NodeId(8), NodeId(12)), 0.10),
        // Attic floor / insulation
        ((NodeId(9), NodeId(12)), 0.60),
        ((NodeId(9), NodeId(200)), 2.50),
        // Attic walls
        ((NodeId(10), NodeId(200)), 0.18),
        ((NodeId(10), NodeId(11)), 0.80),
        ((NodeId(11), NodeId(12)), 0.12),
        // Attic infiltration
        ((NodeId(12), NodeId(200)), 0.20),
    ]);

    RCNetwork::from_elements(caps, res, vec![NodeId(200)]).expect("valid 12-node network")
}

fn build_state_space(net: &RCNetwork, dt: f64) -> StateSpaceModel {
    let (a_c, b_c, _) = net.build_matrices().expect("matrix assembly");
    let n = a_c.nrows();
    let mapping = OutputMapping {
        output_count: 1,
        // Track the zone air node (last internal node by sorted NodeId)
        node_to_output: vec![(0, n - 1, 1.0)],
        input_to_output: vec![],
    };
    StateSpaceModel::from_continuous(&a_c, &b_c, dt, &mapping).expect("state-space model")
}

fn bench_state_space_step(c: &mut Criterion) {
    let dt = 60.0; // 1-minute timestep (SI: seconds)
    let net = build_6node_network();
    let model = build_state_space(&net, dt);
    let n = model.state_dim();
    let x = DVector::from_element(n, 20.0); // 20°C initial state
    let u = DVector::from_element(model.input_dim(), 5.0); // 5°C outdoor

    c.bench_function("state_space_step_6node", |b| {
        b.iter(|| {
            let x_next = model.step(criterion::black_box(&x), criterion::black_box(&u));
            criterion::black_box(x_next);
        });
    });
}

fn bench_state_space_step_12node(c: &mut Criterion) {
    let dt = 60.0;
    let net = build_12node_network();
    let model = build_state_space(&net, dt);
    let n = model.state_dim();
    let x = DVector::from_element(n, 20.0);
    let u = DVector::from_element(model.input_dim(), 5.0);

    c.bench_function("state_space_step_12node", |b| {
        b.iter(|| {
            let x_next = model.step(criterion::black_box(&x), criterion::black_box(&u));
            criterion::black_box(x_next);
        });
    });
}

fn bench_discretize(c: &mut Criterion) {
    let net = build_6node_network();
    let (a_c, b_c, _) = net.build_matrices().expect("matrix assembly");
    let dt = 60.0;

    let mut group = c.benchmark_group("discretize");

    group.bench_function("zoh_6node", |b| {
        b.iter(|| {
            let result = discretize_zoh(
                criterion::black_box(&a_c),
                criterion::black_box(&b_c),
                criterion::black_box(dt),
            )
            .expect("ZOH discretization");
            criterion::black_box(result);
        });
    });

    group.bench_function("van_loan_6node", |b| {
        b.iter(|| {
            let result = van_loan_discretize(
                criterion::black_box(&a_c),
                criterion::black_box(&b_c),
                criterion::black_box(dt),
            )
            .expect("Van Loan discretization");
            criterion::black_box(result);
        });
    });

    // Vary timestep to capture cache effects across typical HARES resolutions.
    for dt_s in [60.0f64, 300.0, 900.0] {
        group.bench_with_input(
            BenchmarkId::new("zoh_6node_dt", dt_s as u64),
            &dt_s,
            |b, &dt| {
                b.iter(|| {
                    let result = discretize_zoh(
                        criterion::black_box(&a_c),
                        criterion::black_box(&b_c),
                        criterion::black_box(dt),
                    )
                    .expect("ZOH discretization");
                    criterion::black_box(result);
                });
            },
        );
    }

    group.finish();
}

fn bench_rc_network_build_matrices(c: &mut Criterion) {
    let net_6 = build_6node_network();
    let net_12 = build_12node_network();

    let mut group = c.benchmark_group("rc_network");

    group.bench_function("build_matrices_6node", |b| {
        b.iter(|| {
            let result = criterion::black_box(&net_6)
                .build_matrices()
                .expect("matrix assembly");
            criterion::black_box(result);
        });
    });

    group.bench_function("build_matrices_12node", |b| {
        b.iter(|| {
            let result = criterion::black_box(&net_12)
                .build_matrices()
                .expect("matrix assembly");
            criterion::black_box(result);
        });
    });

    group.finish();
}

fn bench_full_construction(c: &mut Criterion) {
    // End-to-end: network construction → matrix assembly → discretization → step.
    let dt = 60.0;

    c.bench_function("full_construction_6node", |b| {
        b.iter(|| {
            let net = build_6node_network();
            let (a_c, b_c, _) = net.build_matrices().expect("matrix assembly");
            let n = a_c.nrows();
            let mapping = OutputMapping {
                output_count: 1,
                node_to_output: vec![(0, n - 1, 1.0)],
                input_to_output: vec![],
            };
            let model = StateSpaceModel::from_continuous(&a_c, &b_c, dt, &mapping).expect("model");
            let x = DVector::from_element(n, 20.0);
            let u = DVector::from_element(b_c.ncols(), 5.0);
            let x_next = model.step(&x, &u);
            criterion::black_box(x_next);
        });
    });
}

fn bench_coupled_step_identity(c: &mut Criterion) {
    // Measures the O(n) closed-form identity-coupled step with 1–3 diagonal
    // coupling entries (typical infiltration coupling for residential models).
    let dt = 60.0;
    let net = build_6node_network();
    let model = build_state_space(&net, dt);
    let x = DVector::from_element(model.state_dim(), 20.0);
    let u = DVector::from_element(model.input_dim(), 5.0);
    let couplings: Vec<(usize, f64, f64)> = vec![(5, 0.3, 10.0), (3, 0.15, 5.0), (0, 0.2, 8.0)];

    assert!(model.m_is_identity());

    let mut buf = black_box(DVector::zeros(model.state_dim()));

    c.bench_function("coupled_step_identity_6node_3couplings", |b| {
        b.iter(|| {
            model.step_with_identity_coupling_into(
                black_box(&x),
                black_box(&u),
                &mut buf,
                black_box(&couplings),
            );
            black_box(&buf);
        });
    });
}

fn bench_coupled_step_lu(c: &mut Criterion) {
    // Measures the O(n³) LU-coupled step (old path) with same coupling config.
    let dt = 60.0;
    let net = build_6node_network();
    let model = build_state_space(&net, dt);
    let x = DVector::from_element(model.state_dim(), 20.0);
    let u = DVector::from_element(model.input_dim(), 5.0);
    let couplings: Vec<(usize, f64, f64)> = vec![(5, 0.3, 10.0), (3, 0.15, 5.0), (0, 0.2, 8.0)];
    let n = model.state_dim();
    let mut m_scratch = DMatrix::zeros(n, n);
    let mut buf = black_box(DVector::zeros(n));

    c.bench_function("coupled_step_lu_6node_3couplings", |b| {
        b.iter(|| {
            let lu = model.build_coupled_lu(&mut m_scratch, black_box(&couplings));
            model.step_with_coupled_lu_into(
                black_box(&x),
                black_box(&u),
                &mut buf,
                &lu,
                black_box(&couplings),
            );
            black_box(&buf);
        });
    });
}

criterion_group!(
    benches,
    bench_state_space_step,
    bench_state_space_step_12node,
    bench_discretize,
    bench_rc_network_build_matrices,
    bench_full_construction,
    bench_coupled_step_identity,
    bench_coupled_step_lu,
);
criterion_main!(benches);
