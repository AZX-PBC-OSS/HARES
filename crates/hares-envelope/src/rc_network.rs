//! RC thermal network construction from envelope data.

use std::collections::{HashMap, HashSet};

use nalgebra::DMatrix;
use thiserror::Error;

/// Node identifier in the RC network graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(
    any(debug_assertions, feature = "observe_detailed"),
    derive(serde::Serialize)
)]
pub struct NodeId(pub u32);

impl From<u32> for NodeId {
    fn from(value: u32) -> Self {
        Self(value)
    }
}

#[derive(Debug, Error)]
pub enum RCNetworkError {
    #[error("invalid capacitance at node {node:?}: expected > 0, got {value}")]
    NonPositiveCapacitance { node: NodeId, value: f64 },
    #[error("invalid resistance between nodes {a:?} and {b:?}: expected > 0, got {value}")]
    NonPositiveResistance { a: NodeId, b: NodeId, value: f64 },
    #[error("invalid resistor edge ({node:?}, {node:?}): self-loops are not allowed")]
    SelfLoopEdge { node: NodeId },
    #[error(
        "node {node:?} is disconnected: present in capacitances/external_nodes but in no resistance"
    )]
    DisconnectedNode { node: NodeId },
    #[error("node {node:?} appears in both capacitances (internal) and external_nodes")]
    NodeInInternalAndExternal { node: NodeId },
    #[error("duplicate external node {node:?}")]
    DuplicateExternalNode { node: NodeId },
    #[error("node {node:?} in reduced graph is neither internal nor external")]
    UnclassifiedReducedNode { node: NodeId },
}

pub type Result<T> = std::result::Result<T, RCNetworkError>;

/// Compute equivalent parallel resistance for two resistors.
#[must_use]
pub fn parallel_resistance(r1: f64, r2: f64) -> f64 {
    (r1 * r2) / (r1 + r2)
}

/// RC graph used to assemble continuous-time state matrices.
#[derive(Debug, Clone)]
pub struct RCNetwork {
    pub capacitances: HashMap<NodeId, f64>,
    pub resistances: HashMap<(NodeId, NodeId), f64>,
    pub external_nodes: Vec<NodeId>,
}

impl RCNetwork {
    pub fn from_elements(
        capacitances: HashMap<NodeId, f64>,
        resistances: HashMap<(NodeId, NodeId), f64>,
        external_nodes: Vec<NodeId>,
    ) -> Result<Self> {
        for (&node, &c) in &capacitances {
            if c <= 0.0 {
                return Err(RCNetworkError::NonPositiveCapacitance { node, value: c });
            }
        }

        let mut external_set = HashSet::new();
        for &node in &external_nodes {
            if !external_set.insert(node) {
                return Err(RCNetworkError::DuplicateExternalNode { node });
            }
            if capacitances.contains_key(&node) {
                return Err(RCNetworkError::NodeInInternalAndExternal { node });
            }
        }

        let mut normalized_resistances: HashMap<(NodeId, NodeId), f64> = HashMap::new();
        let mut degree: HashMap<NodeId, usize> = HashMap::new();

        for (&(a, b), &r) in &resistances {
            if r <= 0.0 {
                return Err(RCNetworkError::NonPositiveResistance { a, b, value: r });
            }
            if a == b {
                return Err(RCNetworkError::SelfLoopEdge { node: a });
            }
            let edge = canonical_edge(a, b);
            *degree.entry(edge.0).or_insert(0) += 1;
            *degree.entry(edge.1).or_insert(0) += 1;
            if let Some(existing) = normalized_resistances.get_mut(&edge) {
                *existing = parallel_resistance(*existing, r);
            } else {
                normalized_resistances.insert(edge, r);
            }
        }

        let classified_nodes: HashSet<NodeId> = capacitances
            .keys()
            .copied()
            .chain(external_nodes.iter().copied())
            .collect();
        for node in classified_nodes {
            if !degree.contains_key(&node) {
                return Err(RCNetworkError::DisconnectedNode { node });
            }
        }

        let reduced = reduce_floating_nodes(
            normalized_resistances,
            capacitances.keys().copied().collect(),
            external_set,
        );

        // All nodes in reduced resistances must now be known internal or external.
        let known_nodes: HashSet<NodeId> = capacitances
            .keys()
            .copied()
            .chain(external_nodes.iter().copied())
            .collect();
        for &(a, b) in reduced.keys() {
            if !known_nodes.contains(&a) {
                return Err(RCNetworkError::UnclassifiedReducedNode { node: a });
            }
            if !known_nodes.contains(&b) {
                return Err(RCNetworkError::UnclassifiedReducedNode { node: b });
            }
        }

        Ok(Self {
            capacitances,
            resistances: reduced,
            external_nodes,
        })
    }

    pub fn build_matrices(&self) -> Result<(DMatrix<f64>, DMatrix<f64>, Vec<NodeId>)> {
        let internal_nodes = sorted_internal_nodes(&self.capacitances, &self.external_nodes);
        let mut external_nodes = self.external_nodes.clone();
        external_nodes.sort_unstable();

        let mut internal_index: HashMap<NodeId, usize> = HashMap::new();
        for (idx, &node) in internal_nodes.iter().enumerate() {
            internal_index.insert(node, idx);
        }
        let mut external_index: HashMap<NodeId, usize> = HashMap::new();
        for (idx, &node) in external_nodes.iter().enumerate() {
            external_index.insert(node, idx);
        }

        let mut adjacency: HashMap<NodeId, Vec<(NodeId, f64)>> = HashMap::new();
        for (&(a, b), &r) in &self.resistances {
            adjacency.entry(a).or_default().push((b, r));
            adjacency.entry(b).or_default().push((a, r));
        }
        for neighbors in adjacency.values_mut() {
            neighbors.sort_by_key(|(neighbor, _)| *neighbor);
        }

        let mut a_c = DMatrix::<f64>::zeros(internal_nodes.len(), internal_nodes.len());
        let mut b_c = DMatrix::<f64>::zeros(internal_nodes.len(), external_nodes.len());

        for (i, &node_i) in internal_nodes.iter().enumerate() {
            let c_i = self.capacitances[&node_i];
            if let Some(neighbors) = adjacency.get(&node_i) {
                for &(node_j, r_ij) in neighbors {
                    let coeff = 1.0 / (r_ij * c_i);
                    a_c[(i, i)] -= coeff;
                    if let Some(&j) = internal_index.get(&node_j) {
                        a_c[(i, j)] += coeff;
                    } else if let Some(&k) = external_index.get(&node_j) {
                        b_c[(i, k)] += coeff;
                    } else {
                        return Err(RCNetworkError::UnclassifiedReducedNode { node: node_j });
                    }
                }
            }
        }

        Ok((a_c, b_c, internal_nodes))
    }

    #[must_use]
    pub fn node_count(&self) -> usize {
        let mut nodes: HashSet<NodeId> = HashSet::new();
        for &node in self.capacitances.keys() {
            nodes.insert(node);
        }
        for &node in &self.external_nodes {
            nodes.insert(node);
        }
        for &(a, b) in self.resistances.keys() {
            nodes.insert(a);
            nodes.insert(b);
        }
        nodes.len()
    }
}

fn canonical_edge(a: NodeId, b: NodeId) -> (NodeId, NodeId) {
    if a <= b { (a, b) } else { (b, a) }
}

pub(crate) fn sorted_internal_nodes(
    capacitances: &HashMap<NodeId, f64>,
    external_nodes: &[NodeId],
) -> Vec<NodeId> {
    let external: HashSet<NodeId> = external_nodes.iter().copied().collect();
    let mut internal: Vec<NodeId> = capacitances
        .keys()
        .copied()
        .filter(|node| !external.contains(node))
        .collect();
    internal.sort_unstable();
    internal
}

fn reduce_floating_nodes(
    mut resistances: HashMap<(NodeId, NodeId), f64>,
    internal_nodes: HashSet<NodeId>,
    external_nodes: HashSet<NodeId>,
) -> HashMap<(NodeId, NodeId), f64> {
    loop {
        let mut current_nodes: HashSet<NodeId> = HashSet::new();
        for &(a, b) in resistances.keys() {
            current_nodes.insert(a);
            current_nodes.insert(b);
        }

        let mut floating_nodes: Vec<NodeId> = current_nodes
            .into_iter()
            .filter(|node| !internal_nodes.contains(node) && !external_nodes.contains(node))
            .collect();
        if floating_nodes.is_empty() {
            break;
        }
        floating_nodes.sort_unstable();

        for node_f in floating_nodes {
            let adjacent = adjacent_edges(&resistances, node_f);
            if adjacent.is_empty() {
                continue;
            }

            remove_node_edges(&mut resistances, node_f);

            // Degenerate floating node with one connection: branch contributes nothing.
            if adjacent.len() <= 1 {
                continue;
            }

            let mut neighbors: Vec<(NodeId, f64)> = adjacent;
            neighbors.sort_by_key(|(node, _)| *node);

            let sum_g: f64 = neighbors.iter().map(|(_, r)| 1.0 / r).sum();
            for i in 0..neighbors.len() {
                for j in (i + 1)..neighbors.len() {
                    let (ni, r_if) = neighbors[i];
                    let (nj, r_jf) = neighbors[j];
                    let g_new = (1.0 / r_if) * (1.0 / r_jf) / sum_g;
                    let edge = canonical_edge(ni, nj);
                    if let Some(existing_r) = resistances.get_mut(&edge) {
                        let g_total = (1.0 / *existing_r) + g_new;
                        *existing_r = 1.0 / g_total;
                    } else {
                        resistances.insert(edge, 1.0 / g_new);
                    }
                }
            }
        }
    }

    resistances
}

fn adjacent_edges(
    resistances: &HashMap<(NodeId, NodeId), f64>,
    node: NodeId,
) -> Vec<(NodeId, f64)> {
    let mut adjacent = Vec::new();
    for (&(a, b), &r) in resistances {
        if a == node {
            adjacent.push((b, r));
        } else if b == node {
            adjacent.push((a, r));
        }
    }
    adjacent
}

fn remove_node_edges(resistances: &mut HashMap<(NodeId, NodeId), f64>, node: NodeId) {
    resistances.retain(|(a, b), _| *a != node && *b != node);
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use nalgebra::DMatrix;

    use crate::rc_network::{NodeId, RCNetwork, RCNetworkError, parallel_resistance};

    fn n(id: u32) -> NodeId {
        NodeId(id)
    }

    fn assert_matrix_close(actual: &DMatrix<f64>, expected: &DMatrix<f64>, tol: f64) {
        assert_eq!(actual.shape(), expected.shape());
        for i in 0..actual.nrows() {
            for j in 0..actual.ncols() {
                let diff = (actual[(i, j)] - expected[(i, j)]).abs();
                assert!(
                    diff <= tol,
                    "entry ({i}, {j}) mismatch: actual={} expected={} diff={diff}",
                    actual[(i, j)],
                    expected[(i, j)]
                );
            }
        }
    }

    fn assert_matrix_bits_identical(a: &DMatrix<f64>, b: &DMatrix<f64>) {
        assert_eq!(a.shape(), b.shape());
        for i in 0..a.nrows() {
            for j in 0..a.ncols() {
                assert_eq!(a[(i, j)].to_bits(), b[(i, j)].to_bits());
            }
        }
    }

    #[test]
    fn parallel_resistance_equal_values() {
        assert_eq!(parallel_resistance(2.0, 2.0), 1.0);
    }

    #[test]
    fn one_r_one_c_matrices_are_exact() {
        let caps = HashMap::from([(n(1), 2.0)]);
        let res = HashMap::from([((n(1), n(2)), 4.0)]);
        let net = RCNetwork::from_elements(caps, res, vec![n(2)]).unwrap();

        let (a_c, b_c, _internal_nodes) = net.build_matrices().unwrap();
        let expected_a = DMatrix::from_row_slice(1, 1, &[-1.0 / 8.0]);
        let expected_b = DMatrix::from_row_slice(1, 1, &[1.0 / 8.0]);
        assert_eq!(a_c, expected_a);
        assert_eq!(b_c, expected_b);
    }

    #[test]
    fn multi_boundary_3r2c_matches_hand_and_golden_values() {
        let caps = HashMap::from([(n(1), 2.0), (n(2), 4.0)]);
        let res = HashMap::from([
            ((n(1), n(2)), 1.0),
            ((n(1), n(10)), 2.0),
            ((n(2), n(11)), 8.0),
        ]);
        let net = RCNetwork::from_elements(caps, res, vec![n(10), n(11)]).unwrap();
        let (a_c, b_c, _internal_nodes) = net.build_matrices().unwrap();

        // Golden constants from OCHRE-equivalent create_rc_matrices setup.
        let a_golden = DMatrix::from_row_slice(2, 2, &[-0.75, 0.5, 0.25, -0.28125]);
        let b_golden = DMatrix::from_row_slice(2, 2, &[0.25, 0.0, 0.0, 0.03125]);

        assert_matrix_close(&a_c, &a_golden, 1e-12);
        assert_matrix_close(&b_c, &b_golden, 1e-12);
    }

    #[test]
    fn star_mesh_floating_node_matches_direct_equivalent() {
        let caps = HashMap::from([(n(1), 2.0)]);
        let with_floating = HashMap::from([((n(1), n(2)), 3.0), ((n(2), n(3)), 6.0)]);
        let direct = HashMap::from([((n(1), n(3)), 9.0)]);

        let net_reduced =
            RCNetwork::from_elements(caps.clone(), with_floating, vec![n(3)]).unwrap();
        let net_direct = RCNetwork::from_elements(caps, direct, vec![n(3)]).unwrap();

        let (a_reduced, b_reduced, _) = net_reduced.build_matrices().unwrap();
        let (a_direct, b_direct, _) = net_direct.build_matrices().unwrap();
        assert_matrix_close(&a_reduced, &a_direct, 1e-12);
        assert_matrix_close(&b_reduced, &b_direct, 1e-12);
    }

    #[test]
    fn dangling_floating_branch_is_removed_silently() {
        let caps = HashMap::from([(n(1), 5.0)]);
        let res = HashMap::from([((n(1), n(2)), 10.0), ((n(1), n(3)), 7.0)]);
        let net = RCNetwork::from_elements(caps, res, vec![n(2)]).unwrap();
        let (a_c, b_c, _) = net.build_matrices().unwrap();

        let expected_a = DMatrix::from_row_slice(1, 1, &[-1.0 / 50.0]);
        let expected_b = DMatrix::from_row_slice(1, 1, &[1.0 / 50.0]);
        assert_matrix_close(&a_c, &expected_a, 1e-12);
        assert_matrix_close(&b_c, &expected_b, 1e-12);
        assert_eq!(net.node_count(), 2);
    }

    #[test]
    fn from_elements_rejects_non_positive_r_or_c() {
        let bad_c = RCNetwork::from_elements(
            HashMap::from([(n(1), 0.0)]),
            HashMap::from([((n(1), n(2)), 1.0)]),
            vec![n(2)],
        )
        .unwrap_err();
        assert!(matches!(
            bad_c,
            RCNetworkError::NonPositiveCapacitance { .. }
        ));

        let bad_r = RCNetwork::from_elements(
            HashMap::from([(n(1), 1.0)]),
            HashMap::from([((n(1), n(2)), -1.0)]),
            vec![n(2)],
        )
        .unwrap_err();
        assert!(matches!(
            bad_r,
            RCNetworkError::NonPositiveResistance { .. }
        ));
    }

    #[test]
    fn from_elements_rejects_unclassified_or_disconnected_cases() {
        let disconnected = RCNetwork::from_elements(
            HashMap::from([(n(1), 1.0)]),
            HashMap::from([((n(2), n(3)), 5.0)]),
            vec![n(3)],
        )
        .unwrap_err();
        assert!(matches!(disconnected, RCNetworkError::DisconnectedNode { node } if node == n(1)));

        let self_loop = RCNetwork::from_elements(
            HashMap::from([(n(1), 1.0)]),
            HashMap::from([((n(2), n(2)), 5.0)]),
            vec![n(3)],
        )
        .unwrap_err();
        assert!(matches!(self_loop, RCNetworkError::SelfLoopEdge { node } if node == n(2)));
    }

    #[test]
    fn state_row_order_is_deterministic_across_hashmap_insertion_order() {
        let mut caps_1 = HashMap::new();
        caps_1.insert(n(2), 4.0);
        caps_1.insert(n(1), 2.0);
        let mut res_1 = HashMap::new();
        res_1.insert((n(1), n(2)), 1.0);
        res_1.insert((n(1), n(10)), 2.0);
        res_1.insert((n(2), n(11)), 8.0);

        let mut caps_2 = HashMap::new();
        caps_2.insert(n(1), 2.0);
        caps_2.insert(n(2), 4.0);
        let mut res_2 = HashMap::new();
        res_2.insert((n(2), n(11)), 8.0);
        res_2.insert((n(1), n(10)), 2.0);
        res_2.insert((n(1), n(2)), 1.0);

        let net_1 = RCNetwork::from_elements(caps_1, res_1, vec![n(10), n(11)]).unwrap();
        let net_2 = RCNetwork::from_elements(caps_2, res_2, vec![n(10), n(11)]).unwrap();
        let (a1, b1, nodes1) = net_1.build_matrices().unwrap();
        let (a2, b2, nodes2) = net_2.build_matrices().unwrap();
        assert_matrix_bits_identical(&a1, &a2);
        assert_matrix_bits_identical(&b1, &b2);
        assert_eq!(nodes1, nodes2);
    }

    #[test]
    fn neighbor_iteration_order_is_deterministic() {
        let caps = HashMap::from([(n(1), 2.0), (n(2), 3.0), (n(3), 4.0)]);

        let mut res_1 = HashMap::new();
        res_1.insert((n(1), n(2)), 5.0);
        res_1.insert((n(1), n(3)), 7.0);
        res_1.insert((n(1), n(10)), 11.0);

        let mut res_2 = HashMap::new();
        res_2.insert((n(1), n(10)), 11.0);
        res_2.insert((n(1), n(3)), 7.0);
        res_2.insert((n(1), n(2)), 5.0);

        let net_1 = RCNetwork::from_elements(caps.clone(), res_1, vec![n(10)]).unwrap();
        let net_2 = RCNetwork::from_elements(caps, res_2, vec![n(10)]).unwrap();
        let (a1, b1, _) = net_1.build_matrices().unwrap();
        let (a2, b2, _) = net_2.build_matrices().unwrap();
        assert_matrix_bits_identical(&a1, &a2);
        assert_matrix_bits_identical(&b1, &b2);
    }

    #[test]
    fn one_r_one_c_exact_discrete_coefficients() {
        // For R=1 K/W, C=1000 J/K, dt=60s:
        // A_d = exp(-dt/(R*C)) = exp(-0.06) = 0.94176453...
        // B_d = 1 - A_d = 0.05823547...
        // Standard RC circuit step response (textbook exact solution)
        use crate::state_space::discretize_zoh;

        let r = 1.0;
        let c = 1000.0;
        let dt = 60.0;

        let caps = HashMap::from([(n(1), c)]);
        let res = HashMap::from([((n(1), n(2)), r)]);
        let net = RCNetwork::from_elements(caps, res, vec![n(2)]).unwrap();
        let (a_c, b_c, _) = net.build_matrices().unwrap();

        let (a_d, b_d) = discretize_zoh(&a_c, &b_c, dt).unwrap();

        let expected_a_d = (-dt / (r * c)).exp();
        let expected_b_d = 1.0 - expected_a_d;

        assert!(
            (a_d[(0, 0)] - expected_a_d).abs() < 1e-12,
            "A_d: {}, expected {}",
            a_d[(0, 0)],
            expected_a_d
        );
        assert!(
            (b_d[(0, 0)] - expected_b_d).abs() < 1e-12,
            "B_d: {}, expected {}",
            b_d[(0, 0)],
            expected_b_d
        );
    }

    #[test]
    fn external_input_columns_are_sorted_by_node_id() {
        let caps = HashMap::from([(n(1), 2.0)]);
        let res = HashMap::from([((n(1), n(10)), 2.0), ((n(1), n(5)), 5.0)]);
        let net = RCNetwork::from_elements(caps, res, vec![n(10), n(5)]).unwrap();
        let (_a_c, b_c, _) = net.build_matrices().unwrap();
        let expected = DMatrix::from_row_slice(1, 2, &[1.0 / (5.0 * 2.0), 1.0 / (2.0 * 2.0)]);
        assert_eq!(b_c, expected);
    }

    // ── Star-mesh (Y-Δ) floating node elimination ────────────────────────

    /// Verify the star-mesh transform: 3-branch star (A-F, B-F, C-F) with
    /// floating node F eliminated produces correct pairwise conductances.
    ///
    /// After eliminating F, the direct conductances must satisfy:
    ///   G_AB = G_AF × G_BF / (G_AF + G_BF + G_CF)
    ///   G_AC = G_AF × G_CF / (G_AF + G_BF + G_CF)
    ///   G_BC = G_BF × G_CF / (G_AF + G_BF + G_CF)
    ///
    /// This is the standard Y-Δ (star-mesh) transform used in linearized
    /// inter-surface radiation. Reference: TRNSYS Type 56, ESP-r,
    /// OCHRE Envelope.py:1048-1061.
    #[test]
    fn star_mesh_3_branch_star_elimination_matches_formula() {
        // Star: A--R1--F--R2--B, F--R3--C.  F is floating (no capacitance).
        // R_AF = 2.0, R_BF = 3.0, R_CF = 6.0
        let r_af = 2.0_f64;
        let r_bf = 3.0;
        let r_cf = 6.0;
        let g_af = 1.0 / r_af;
        let g_bf = 1.0 / r_bf;
        let g_cf = 1.0 / r_cf;
        let sum_g = g_af + g_bf + g_cf;

        // Capacitance-bearing internal nodes A, B, C.
        let caps = HashMap::from([(n(1), 100.0), (n(2), 200.0), (n(3), 300.0)]);
        let res = HashMap::from([
            ((n(1), n(100)), r_af), // A ↔ F
            ((n(2), n(100)), r_bf), // B ↔ F
            ((n(3), n(100)), r_cf), // C ↔ F
        ]);
        // F (node 100) has no capacitance → floating, will be eliminated.
        let net = RCNetwork::from_elements(caps, res, vec![]).unwrap();

        // After elimination: direct edges (A,B), (A,C), (B,C).
        let g_ab_expected = g_af * g_bf / sum_g;
        let g_ac_expected = g_af * g_cf / sum_g;
        let g_bc_expected = g_bf * g_cf / sum_g;

        let r_ab = net.resistances[&(n(1).min(n(2)), n(1).max(n(2)))];
        let r_ac = net.resistances[&(n(1).min(n(3)), n(1).max(n(3)))];
        let r_bc = net.resistances[&(n(2).min(n(3)), n(2).max(n(3)))];

        assert!(
            (1.0 / r_ab - g_ab_expected).abs() < 1e-9,
            "G_AB: got {}, expected {g_ab_expected}",
            1.0 / r_ab
        );
        assert!(
            (1.0 / r_ac - g_ac_expected).abs() < 1e-9,
            "G_AC: got {}, expected {g_ac_expected}",
            1.0 / r_ac
        );
        assert!(
            (1.0 / r_bc - g_bc_expected).abs() < 1e-9,
            "G_BC: got {}, expected {g_bc_expected}",
            1.0 / r_bc
        );

        // Floating node F should no longer appear in the reduced resistances.
        for &(a, b) in net.resistances.keys() {
            assert!(
                a != n(100) && b != n(100),
                "floating node F should be eliminated"
            );
        }
    }

    /// Verify cascading floating node elimination: A—F1—F2—B where both
    /// F1 and F2 are floating (zero capacitance).
    ///
    /// After reduction, a direct edge (A,B) must exist with conductance
    /// equal to the series combination through F1 and F2:
    ///   G_AB = 1 / (R_AF1 + R_F1F2 + R_F2B)
    ///
    /// This exercises the iterative loop in `reduce_floating_nodes` which
    /// must first eliminate one floating node, then find and eliminate the
    /// remaining one on the next pass.
    #[test]
    fn cascading_floating_nodes_eliminate_to_direct_edge() {
        let r_a_f1 = 2.0_f64;
        let r_f1_f2 = 3.0;
        let r_f2_b = 5.0;
        let r_total = r_a_f1 + r_f1_f2 + r_f2_b;

        let caps = HashMap::from([(n(1), 100.0), (n(2), 200.0)]);
        let res = HashMap::from([
            ((n(1), n(100)), r_a_f1),    // A ↔ F1
            ((n(100), n(101)), r_f1_f2), // F1 ↔ F2
            ((n(101), n(2)), r_f2_b),    // F2 ↔ B
        ]);
        let net = RCNetwork::from_elements(caps, res, vec![]).unwrap();

        let r_ab = net.resistances[&(n(1), n(2))];
        let g_ab = 1.0 / r_ab;
        let g_expected = 1.0 / r_total;

        assert!(
            (g_ab - g_expected).abs() < 1e-9,
            "G_AB: got {g_ab}, expected {g_expected}"
        );

        // No floating nodes should remain.
        for &(a, b) in net.resistances.keys() {
            assert!(a != n(100) && b != n(100), "F1 should be eliminated");
            assert!(a != n(101) && b != n(101), "F2 should be eliminated");
        }
    }

    /// 4-node zone with radiation star node + floating window node.
    ///
    /// Network topology (mimics a single BESTEST 600 zone):
    ///
    ///   ZoneAir (1) ←R_conv→ WallInner (2) ←R_mat→ Outdoor (EXT)
    ///       ↑                          ↑
    ///       R_conv                     R_rad_wall ──→ Star (200)
    ///       ↓                          ↓                ↓
    ///   FloorInner (3) ←R_mat→ GND    R_rad_floor       R_rad_win
    ///                                                    ↓
    ///                                              WinFloat (201)
    ///                                                    ↓
    ///                                              R_u_win
    ///                                                    ↓
    ///                                              Outdoor (EXT)
    ///
    /// ZoneAir (1) ←R_conv→ FloorInner (3)
    /// ZoneAir (1) ←R_conv→ WinFloat (201)  [window convection to zone air]
    ///
    /// After `reduce_floating_nodes`:
    ///   - Star (200) is eliminated → pairwise conductances between
    ///     WallInner, FloorInner, and WinFloat
    ///   - WinFloat (201) is now only connected to WallInner, FloorInner,
    ///     ZoneAir, and Outdoor — but it has no capacitance, so it is also
    ///     eliminated → direct conductances from Outdoor to WallInner,
    ///     FloorInner, and ZoneAir (via window U-factor and radiation paths)
    ///
    /// This is the exact topology that HARES constructs for a zone with
    /// 2 opaque surfaces + 1 window, using the star-mesh linearized
    /// radiation method. The cascading elimination of Star then WinFloat
    /// tests the multi-pass loop in `reduce_floating_nodes`.
    ///
    /// Reference: OCHRE `add_radiation_resistances()` at
    /// `Envelope.py:1048-1061`, TRNSYS Type 56 star network,
    /// EN ISO 52016-1:2017 Annex E "detailed" RC method.
    #[test]
    fn zone_with_star_and_window_cascading_elimination() {
        const T_REF_K: f64 = 293.15;
        // ASHRAE 140-2017 §5.3.1.9, Table 24: ε_ir = 0.9 for ALL interior
        // surfaces including windows.
        const EPS_INTERIOR: f64 = 0.90;

        let a_wall = 21.6_f64;
        let a_floor = 48.0;
        let a_window = 12.0;

        // Linearized radiation conductances: G_i = 4·ε·σ·A·T_ref³
        // Stefan-Boltzmann linearisation: h_rad = 4·ε·σ·T_ref³  [W/(m²·K)]
        let g_rad_wall = hares_physics::constants::linearised_h_rad(EPS_INTERIOR, T_REF_K) * a_wall;
        let g_rad_floor =
            hares_physics::constants::linearised_h_rad(EPS_INTERIOR, T_REF_K) * a_floor;
        let g_rad_window =
            hares_physics::constants::linearised_h_rad(EPS_INTERIOR, T_REF_K) * a_window;
        let r_rad_wall = 1.0 / g_rad_wall;
        let r_rad_floor = 1.0 / g_rad_floor;
        let r_rad_window = 1.0 / g_rad_window;

        // Convection film resistances (TARP conv-only for vertical/horizontal)
        // In Option 2: R_film = 1/h_conv (convection only, no h_rad).
        let r_conv_wall = 0.3255;
        let r_conv_floor = 0.2805;
        // Window h_conv from U-factor decomposition:
        // h_si = 1/r_film_int ≈ 7.33 W/m²K (for U=3.0 window)
        // h_rad = 4×0.9×σ×T_ref³ ≈ 5.14
        // h_conv = h_si - h_rad ≈ 2.19
        // R_conv = 1/h_conv ≈ 0.457 m²K/W (per unit area)
        let r_conv_window_per_m2 = 1.0 / (7.33 - 5.14);
        let r_conv_window = r_conv_window_per_m2 / a_window;

        // Material resistances (K/W)
        let r_mat_wall = 1.789 * a_wall;
        let r_mat_floor = 24.8 * a_floor;

        // Window glass + exterior film resistance: r_glass/A
        // For U=3.0: r_total = 1/3.0 = 0.333, r_film_int ≈ 0.136, r_glass = 0.333 - 0.136 = 0.197
        // r_glass_ext = r_glass / A (no separate exterior film for windows)
        let r_glass_ext = 0.197 / a_window;

        // ── Build the RC network ──
        //
        // Option 2 (EnergyPlus/TRNSYS/ESP-r) architecture:
        //   ZoneAir(1) ←R_conv→ WallInner(2) ←R_mat→ Outdoor(EXT)
        //       ↑                          ↑
        //       R_conv                     R_rad_wall ──→ Star (200)
        //       ↓                          ↓                ↓
        //   FloorInner(3) ←R_mat→ GND    R_rad_floor       R_rad_win
        //                                                    ↓
        //                                              WinFloat (201)
        //                                              ↙          ↘
        //                                    R_conv(→zone)    R_glass_ext(→outdoor)
        //
        // ZoneAir (1) ←R_conv→ FloorInner (3)
        // ZoneAir (1) ←R_conv→ WinFloat (201)  [window convection to zone air]
        //
        // Key difference from combined-film model: NO parallel R_rad from
        // zone_air to window_node. Radiation goes through star-mesh ONLY.
        let caps = HashMap::from([
            (n(1), 500_000.0), // zone air thermal mass
            (n(2), 200_000.0), // wall inner layer
            (n(3), 800_000.0), // floor slab
        ]);

        let ext = vec![n(9000), n(9001)]; // Outdoor, Ground

        let mut res = HashMap::new();

        // Zone air ↔ opaque surfaces (convection-only R_film)
        res.insert((n(1), n(2)), r_conv_wall);
        res.insert((n(1), n(3)), r_conv_floor);

        // Opaque surface inner nodes ↔ material → outdoor/ground
        res.insert((n(2), n(9000)), r_mat_wall); // wall → outdoor
        res.insert((n(3), n(9001)), r_mat_floor); // floor → ground

        // Star node (200) ↔ each surface via linearized radiation
        res.insert((n(2), n(200)), r_rad_wall);
        res.insert((n(3), n(200)), r_rad_floor);
        res.insert((n(201), n(200)), r_rad_window);

        // Window float (201) ↔ zone air (convection ONLY, no parallel R_rad)
        // and outdoor (glass + ext film resistance)
        res.insert((n(1), n(201)), r_conv_window);
        res.insert((n(201), n(9000)), r_glass_ext);

        let net = RCNetwork::from_elements(caps, res, ext).unwrap();

        // ── Verify: no floating nodes remain ──
        for &(a, b) in net.resistances.keys() {
            assert!(
                a != n(200) && b != n(200),
                "star node 200 should be eliminated"
            );
            assert!(
                a != n(201) && b != n(201),
                "window float 201 should be eliminated"
            );
        }

        // ── Verify: expected pairwise conductances from star-mesh ──
        //
        // After star elimination, the direct conductances between the star's
        // neighbors (WallInner=2, FloorInner=3, WinFloat=201) are:
        //   G_ij = G_i,star × G_j,star / Σ G_k,star
        let sum_g_star = g_rad_wall + g_rad_floor + g_rad_window;

        // Wall ↔ Floor (direct from star elimination, both are internal)
        let g_wall_floor_star = g_rad_wall * g_rad_floor / sum_g_star;
        // This edge should exist in the reduced network between nodes 2 and 3
        // (possibly in parallel with other paths)
        let r_wf = net.resistances[&(n(2).min(n(3)), n(2).max(n(3)))];
        let g_wf = 1.0 / r_wf;
        // The star-mesh contribution is g_wall_floor_star; there may be other
        // paths (e.g. through zone air) that add conductance in parallel.
        // The reduced conductance should be >= the star-mesh value.
        assert!(
            g_wf >= g_wall_floor_star * (1.0 - 1e-9),
            "G(Wall↔Floor) = {g_wf}, expected at least {g_wall_floor_star} from star-mesh"
        );

        // ── Verify: window pathways ──
        //
        // After cascading elimination of star then WinFloat, there must be
        // direct conductances from Outdoor (9000) to WallInner (2),
        // FloorInner (3), and ZoneAir (1), representing the combined
        // window U-factor + radiation redistribution path.
        //
        // The elimination of WinFloat distributes its U-factor conductance
        // to Outdoor across ALL of WinFloat's other neighbors (WallInner,
        // FloorInner, ZoneAir) proportionally to their conductances to
        // WinFloat. The zone-air-to-outdoor edge is NOT simply the series
        // r_conv_window + r_u_win — it's a fraction of that, with the rest
        // going to WallInner and FloorInner via radiation.
        //
        // We verify that the outdoor node is connected to zone air and
        // that the total conductance is physically reasonable.
        let r_zone_outdoor = net.resistances.get(&(n(1).min(n(9000)), n(1).max(n(9000))));
        assert!(
            r_zone_outdoor.is_some(),
            "zone air must have a direct path to outdoor after window elimination"
        );
        // The wall also has a direct conductance to outdoor that includes
        // both the material path and the radiation-redistributed window path.
        let r_wall_outdoor = net.resistances.get(&(n(2).min(n(9000)), n(2).max(n(9000))));
        assert!(
            r_wall_outdoor.is_some(),
            "wall inner must have a direct path to outdoor after window elimination"
        );

        // ── Verify: energy conservation at uniform temperature ──
        //
        // When all temperatures are equal, the net conductive heat flow on
        // every internal node must be zero. This is trivially true by
        // construction (ΔT = 0 → q = 0), but we verify it numerically
        // by stepping the model from a uniform initial condition and
        // checking that the state doesn't change.
        let (a_c, b_c, _) = net.build_matrices().unwrap();

        // All external inputs at the same temperature (20°C)
        let x0 = vec![20.0_f64; a_c.nrows()];
        let u_ext = vec![20.0_f64; b_c.ncols()];

        // dx/dt = A·x + B·u at uniform temp should give dx/dt ≈ 0
        let x = DMatrix::from_column_slice(a_c.nrows(), 1, &x0);
        let u = DMatrix::from_column_slice(b_c.ncols(), 1, &u_ext);
        let dx = &a_c * &x + &b_c * &u;

        for i in 0..dx.nrows() {
            assert!(
                dx[(i, 0)].abs() < 1e-9,
                "node {i}: dx/dt = {} at uniform T, energy not conserved",
                dx[(i, 0)]
            );
        }
    }

    /// Linearization sensitivity of h_rad at T_ref = 20°C (293.15 K).
    ///
    /// The star-mesh method linearizes the Stefan-Boltzmann T⁴ radiation
    /// law around T_ref = 293.15 K, giving h_rad = 4·ε·σ·T_ref³.
    /// At other surface temperatures the true h_rad differs. This test
    /// documents the error envelope for typical residential conditions.
    ///
    /// When someone asks "why is summer cooling slightly off in hot
    /// climates", the answer is: the linearization at 20°C understates
    /// h_rad at higher surface temps. The fix path is Option 3 (explicit
    /// surface DOFs with iterated T⁴), which can be activated when the
    /// 1-3% error becomes material for the use case.
    #[test]
    fn linearized_h_rad_sensitivity_at_reference_temperature() {
        const EPS: f64 = 0.9;
        const T_REF_K: f64 = 293.15;

        let h_rad_ref = hares_physics::constants::linearised_h_rad(EPS, T_REF_K);

        let test_temps_k: [f64; 4] = [285.0, 295.0, 305.0, 315.0];
        let expected_pcts = [-4.2, 1.0, 6.3, 11.8]; // approximate

        for (i, &t_k) in test_temps_k.iter().enumerate() {
            // Exact (non-linearised) radiation coefficient:
            //   εσ(T₁² + T₂²)(T₁ + T₂)
            // which reduces to 4εσT³ when T₁ = T₂.
            let h_rad_true = EPS
                * hares_physics::constants::STEFAN_BOLTZMANN
                * (t_k.powi(2) + T_REF_K.powi(2))
                * (t_k + T_REF_K);
            let pct_error = (h_rad_true - h_rad_ref) / h_rad_ref * 100.0;
            assert!(
                (pct_error - expected_pcts[i]).abs() < 1.0,
                "at T={t_k}K: h_rad error = {pct_error:.1}%, expected ~{}%",
                expected_pcts[i]
            );
        }

        // The linearization is within 5% for surfaces between 12°C and 32°C
        // (285-305 K), which covers most residential conditions. For extreme
        // cases (sunlit surfaces > 42°C, cold windows < 5°C), the error
        // reaches 10-12%, which is still within BESTEST tolerance (~10% on
        // annual loads) but will be noticeable in detailed comfort calcs.
    }

    /// Verify that `build_matrices()` returns a sorted internal node list
    /// that agrees with `sorted_internal_nodes()` element-for-element.
    #[test]
    fn build_matrices_returns_sorted_node_list() {
        let caps = HashMap::from([(n(2), 4.0), (n(1), 2.0)]);
        let res = HashMap::from([
            ((n(1), n(2)), 1.0),
            ((n(1), n(10)), 2.0),
            ((n(2), n(11)), 8.0),
        ]);
        let net = RCNetwork::from_elements(caps, res, vec![n(10), n(11)]).unwrap();
        let (a_c, b_c, nodes) = net.build_matrices().unwrap();

        let expected = super::sorted_internal_nodes(&net.capacitances, &net.external_nodes);
        assert_eq!(
            nodes, expected,
            "returned node list must match sorted_internal_nodes"
        );
        assert_eq!(
            nodes.len(),
            a_c.nrows(),
            "node count must equal A_c row count"
        );
        assert_eq!(
            nodes.len(),
            b_c.nrows(),
            "node count must equal B_c row count"
        );
    }

    /// Verify that the node_index independently built from `capacitances.keys()`
    /// (as `assemble_building_rc()` formerly did) agrees with the ordering from
    /// `sorted_internal_nodes()`. This protects against the dual-sort-path bug.
    #[test]
    fn node_index_agrees_with_sorted_internal_nodes() {
        let caps = HashMap::from([(n(2), 4.0), (n(1), 2.0), (n(3), 6.0)]);
        let res = HashMap::from([
            ((n(1), n(2)), 1.0),
            ((n(2), n(3)), 3.0),
            ((n(1), n(10)), 2.0),
            ((n(3), n(10)), 5.0),
        ]);
        let net = RCNetwork::from_elements(caps, res, vec![n(10)]).unwrap();

        // The unified sort from build_matrices → sorted_internal_nodes.
        let (_a_c, _b_c, unified_order) = net.build_matrices().unwrap();

        // The old assemble_building_rc() path: raw capacitances keys, sorted.
        let mut old_style_order: Vec<NodeId> = net.capacitances.keys().copied().collect();
        old_style_order.sort_unstable();
        let old_index: HashMap<NodeId, usize> = old_style_order
            .iter()
            .enumerate()
            .map(|(idx, &nid)| (nid, idx))
            .collect();

        // When no external nodes have capacitances (validated by from_elements),
        // old_style_order and unified_order must be element-for-element identical.
        assert_eq!(
            old_style_order, unified_order,
            "raw capacitance key sort must match sorted_internal_nodes when external nodes have no capacitances"
        );

        // Also verify that building node_index from unified_order produces
        // the same mapping as building from old_style_order.
        let unified_index: HashMap<NodeId, usize> = unified_order
            .iter()
            .enumerate()
            .map(|(idx, &nid)| (nid, idx))
            .collect();
        assert_eq!(old_index, unified_index);
    }
}
