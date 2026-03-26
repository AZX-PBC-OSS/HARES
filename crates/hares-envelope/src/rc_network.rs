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

    pub fn build_matrices(&self) -> Result<(DMatrix<f64>, DMatrix<f64>)> {
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

        Ok((a_c, b_c))
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

fn sorted_internal_nodes(
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

        let (a_c, b_c) = net.build_matrices().unwrap();
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
        let (a_c, b_c) = net.build_matrices().unwrap();

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

        let (a_reduced, b_reduced) = net_reduced.build_matrices().unwrap();
        let (a_direct, b_direct) = net_direct.build_matrices().unwrap();
        assert_matrix_close(&a_reduced, &a_direct, 1e-12);
        assert_matrix_close(&b_reduced, &b_direct, 1e-12);
    }

    #[test]
    fn dangling_floating_branch_is_removed_silently() {
        let caps = HashMap::from([(n(1), 5.0)]);
        let res = HashMap::from([((n(1), n(2)), 10.0), ((n(1), n(3)), 7.0)]);
        let net = RCNetwork::from_elements(caps, res, vec![n(2)]).unwrap();
        let (a_c, b_c) = net.build_matrices().unwrap();

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
        let (a1, b1) = net_1.build_matrices().unwrap();
        let (a2, b2) = net_2.build_matrices().unwrap();
        assert_matrix_bits_identical(&a1, &a2);
        assert_matrix_bits_identical(&b1, &b2);
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
        let (a1, b1) = net_1.build_matrices().unwrap();
        let (a2, b2) = net_2.build_matrices().unwrap();
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
        let (a_c, b_c) = net.build_matrices().unwrap();

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
        let (_a_c, b_c) = net.build_matrices().unwrap();
        let expected = DMatrix::from_row_slice(1, 2, &[1.0 / (5.0 * 2.0), 1.0 / (2.0 * 2.0)]);
        assert_eq!(b_c, expected);
    }
}
