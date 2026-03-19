---
id: HARES-013
title: "hares-envelope — RC Network Construction"
kind: implement
depends_on: [HARES-012]
files_to_touch:
  - crates/hares-envelope/src/rc_network.rs
  - crates/hares-envelope/src/lib.rs
references:
  - vendors/OCHRE/ochre/Models/RCModel.py
  - docs/architecture/01-sim-core-and-solver.md
verification:
  - cargo check -p hares-envelope
  - cargo test -p hares-envelope
  - cargo clippy -p hares-envelope -- -D warnings
---

## Background/Context

Before the state-space solver can run, the continuous-time matrices `A_c` and `B_c` must be assembled from the building's RC network topology. This is a Rust reimplementation of OCHRE's `RCModel` (vendors/OCHRE/ochre/Models/RCModel.py, 417 lines). The key challenge is handling "floating" nodes — intermediate mass nodes with no capacitance — via star-mesh (Kron) reduction, which OCHRE applies in `RCModel.transform_floating_node`. Correctness is validated against OCHRE's `create_rc_matrices` output for the same network.

## Work to Do

- [ ] Define `NodeId(u32)` newtype
- [ ] Define `RCNetwork` struct holding: `capacitances: HashMap<NodeId, f64>`, `resistances: HashMap<(NodeId, NodeId), f64>`, `external_nodes: Vec<NodeId>`
- [ ] Implement `RCNetwork::from_elements(capacitances: HashMap<NodeId, f64>, resistances: HashMap<(NodeId, NodeId), f64>, external_nodes: Vec<NodeId>) -> Result<Self>` — validates all R > 0 and all C > 0, returns `Err` with a descriptive message on any violation. Also returns `Err` if any node appears in `capacitances` or `external_nodes` but has zero degree in `resistances` (completely disconnected from the network).
- [ ] Floating node detection: a node present in `resistances` but absent from both `capacitances` and `external_nodes` is a floating node eligible for Kron reduction, regardless of how many neighbours it has. Floating nodes may form chains (A→B→C); reduce iteratively in sorted order. Detect and collect all such nodes before building matrices. Every node in `resistances` must be classifiable as exactly one of: internal (has capacitance, not external), external (in `external_nodes`), or floating (absent from both).
- [ ] Implement `RCNetwork::build_matrices(&self) -> Result<(DMatrix<f64>, DMatrix<f64>)>` — produces `(A_c, B_c)` in continuous time. State rows are ordered by sorting internal nodes ascending by `NodeId` to ensure deterministic `A_c`/`B_c` across runs. Internal nodes (non-external, capacitance > 0) populate state rows; external nodes populate input columns, also sorted ascending by `NodeId`. Follow OCHRE's nodal admittance formulation: diagonal `A_c[i,i] = -sum(1/(R_ij * C_i))` for all neighbours `j`; off-diagonal `A_c[i,j] = 1/(R_ij * C_i)`; `B_c[i,k] = 1/(R_ik * C_i)` for each external node `k` connected to internal node `i`. Neighbour iteration during matrix assembly must also be sorted by `NodeId` to ensure bit-identical `A_c`/`B_c` regardless of `HashMap` insertion order.
- [ ] Implement star-mesh (Kron) reduction for floating nodes per OCHRE's `RCModel.transform_floating_node`. Use conductance form: for each floating node `f` with neighbours `i, j, ...`, compute `G_ij_new = G_if * G_jf / sum_k(G_kf)` where `G = 1/R`. If an existing edge `(i, j)` already exists, add conductances in parallel: `G_total = G_existing + G_ij_new` (equivalent to `parallel_resistance`). Remove node `f` and all its edges from the map after reduction.
- [ ] Degenerate floating node: if a floating node has exactly one neighbour (dangling branch), it contributes no new edges after Kron reduction. Remove the node and its single edge silently (no error); the branch is simply disconnected.
- [ ] Implement `parallel_resistance(r1: f64, r2: f64) -> f64` helper — returns `(r1 * r2) / (r1 + r2)`
- [ ] Expose `RCNetwork::node_count(&self) -> usize` for tests

## Files to Touch

- `crates/hares-envelope/src/rc_network.rs`: new file — `NodeId`, `RCNetwork`, `parallel_resistance`
- `crates/hares-envelope/src/lib.rs`: add `pub mod rc_network` and re-export public types

## Measures of Success

- [ ] 1R1C (one internal node with `C`, one external node, one resistance): `A_c = [[-1.0 / (R * C)]]`, `B_c = [[1.0 / (R * C)]]` exactly
- [ ] Multi-boundary 3R2C (two internal nodes, two external nodes, three resistances) — `A_c` and `B_c` match hand-calculated values to within 1e-12; also verified against OCHRE `RCModel.create_rc_matrices` golden output provided as test constants
- [ ] Star-mesh test: network with one floating intermediate node (no capacitance) reduces to an equivalent two-node network; result `A_c`/`B_c` match the directly-constructed equivalent within 1e-12
- [ ] Dangling branch test: floating node with exactly one neighbour is removed without error; resulting `A_c`/`B_c` dimensions reflect the reduced network
- [ ] `from_elements` returns `Err` when any R ≤ 0 or any C ≤ 0 — validated with dedicated negative-value and zero-value test cases
- [ ] `from_elements` returns `Err` when a node appears in `capacitances` or `external_nodes` but has zero degree in `resistances` (completely disconnected)
- [ ] External node input columns in `B_c` are ordered by `NodeId` ascending, matching the deterministic ordering used for internal state rows
- [ ] State row order is deterministic: two calls to `build_matrices` with nodes inserted in different `HashMap` iteration order produce bit-identical `A_c`/`B_c` (sort internal nodes by `NodeId` ascending)
- [ ] Matrix assembly with neighbours iterated in two different insertion orders produces bit-identical `A_c` and `B_c`
- [ ] `parallel_resistance(2.0, 2.0)` returns `1.0`

## Verification

- [ ] `cargo check -p hares-envelope` passes
- [ ] `cargo test -p hares-envelope` passes
- [ ] `cargo clippy -p hares-envelope -- -D warnings` passes
