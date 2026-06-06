//! Fluid-domain shared state payload types and hydraulic network model.

use serde::{Deserialize, Serialize};

use crate::{FluidType, HaresError, LoopId};

/// Stable fluid node identifier within a loop topology.
#[derive(
    Hash, Eq, PartialEq, Copy, Clone, Debug, Default, Ord, PartialOrd, Serialize, Deserialize,
)]
pub struct FluidNodeId(pub u16);

/// Role of a node in the hydraulic network topology.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FluidNodeRole {
    /// Series pass-through node — one inlet, one outlet.
    Serial,
    /// Source node — injects heat into the loop (e.g. boiler, heat pump).
    Source,
    /// Sink node — extracts heat from the loop (e.g. distribution coil).
    Sink,
    /// Splitter — one inlet, multiple outlets (parallel branches).
    Splitter,
    /// Mixer — multiple inlets, one outlet (parallel branch convergence).
    Mixer,
}

/// A single node in the hydraulic network topology.
///
/// Tracks total inflow and outflow mass flow rates [kg/s] for mass-conservation
/// verification.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FluidNode {
    pub node_id: FluidNodeId,
    pub role: FluidNodeRole,
    /// Sum of inflows [kg/s] from upstream nodes or equipment.
    pub total_inflow_kg_s: f64,
    /// Sum of outflows [kg/s] to downstream nodes or equipment.
    pub total_outflow_kg_s: f64,
}

impl FluidNode {
    #[must_use]
    pub fn new(node_id: FluidNodeId, role: FluidNodeRole) -> Self {
        Self {
            node_id,
            role,
            total_inflow_kg_s: 0.0,
            total_outflow_kg_s: 0.0,
        }
    }

    /// Returns the absolute mass imbalance at this node [kg/s].
    #[must_use]
    pub fn mass_imbalance_kg_s(&self) -> f64 {
        (self.total_inflow_kg_s - self.total_outflow_kg_s).abs()
    }

    /// Resets inflow/outflow accumulators to zero for the next timestep.
    pub fn zero(&mut self) {
        self.total_inflow_kg_s = 0.0;
        self.total_outflow_kg_s = 0.0;
    }
}

/// A branch exiting a splitter node, carrying flow to a downstream equipment
/// or sub-branch node.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SplitterBranch {
    /// Downstream node that receives flow from this branch.
    pub node_id: FluidNodeId,
    /// Requested mass flow rate [kg/s] for this branch, populated by the fluid
    /// solver at each timestep from per-node accumulator totals (`node_flows`).
    /// Zero when the branch node has no accumulator contributions.
    #[serde(skip, default)]
    pub requested_flow_kg_s: f64,
    /// Resistance coefficient [dimensionless].
    ///
    /// Default 1.0 = equal resistance across all branches. Used for
    /// proportional allocation; future pump-curve-based allocation will use
    /// this to weight flow distribution.
    #[serde(default = "default_resistance")]
    pub resistance_coefficient: f64,
}

fn default_resistance() -> f64 {
    1.0
}

/// A splitter node in the hydraulic network: one inlet, N outlet branches.
///
/// The splitter distributes the incoming mass flow among its outlet branches
/// according to the flow-splitting resolution algorithm.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SplitterNode {
    /// The splitter's own FluidNodeId (must match a node in `LoopTopology::nodes`
    /// with role `FluidNodeRole::Splitter`).
    pub node_id: FluidNodeId,
    /// Upstream node feeding the splitter inlet.
    pub inlet_node_id: FluidNodeId,
    /// Outlet branches. Each branch leads to a downstream equipment node.
    pub branches: Vec<SplitterBranch>,
}

/// A branch entering a mixer node from upstream equipment or sub-branch node.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MixerBranch {
    /// Upstream node that feeds flow into this mixer inlet branch.
    pub node_id: FluidNodeId,
}

/// A mixer node in the hydraulic network: N inlet branches, one outlet.
///
/// The mixer combines mass flow from its inlet branches into a single
/// outlet flow.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MixerNode {
    /// The mixer's own FluidNodeId (must match a node in `LoopTopology::nodes`
    /// with role `FluidNodeRole::Mixer`).
    pub node_id: FluidNodeId,
    /// Downstream node receiving the mixer outlet flow.
    pub outlet_node_id: FluidNodeId,
    /// Inlet branches. Each branch feeds flow from upstream equipment.
    pub branches: Vec<MixerBranch>,
}

/// Topology of one hydronic loop: the set of nodes and their connections.
///
/// Mass-conservation checks iterate all loop nodes, computing
/// `|∑ inflow − ∑ outflow|` and verifying it is below `MASS_FLOW_TOLERANCE`.
///
/// The topology supports a single loop with one pump, one heat source, and one
/// or more parallel heat sinks (distribution coils). Splitter and mixer nodes
/// distribute/collect flow across parallel branches.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LoopTopology {
    pub loop_id: LoopId,
    pub nodes: Vec<FluidNode>,
    /// Directed edges: `(from_node_id, to_node_id)`.
    /// Each edge represents a pipe segment carrying mass flow from the source
    /// node to the target node. The set of edges defines which accumulators
    /// feed into and out of each node for conservation checking.
    pub edges: Vec<(FluidNodeId, FluidNodeId)>,
    /// Splitter nodes with branch data for flow-splitting resolution.
    /// When present, the solver uses this to allocate total pump flow across
    /// parallel branches. When absent for a loop with no splitters, a simple
    /// serial-flow model applies.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub splitters: Vec<SplitterNode>,
    /// Mixer nodes with branch data for flow convergence verification.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mixers: Vec<MixerNode>,
}

impl LoopTopology {
    /// Creates a loop topology with nodes and edges. Use
    /// [`with_splitters`](Self::with_splitters) and
    /// [`with_mixers`](Self::with_mixers) to attach branch data for
    /// flow-splitting resolution.
    #[must_use]
    pub fn new(
        loop_id: LoopId,
        nodes: Vec<FluidNode>,
        edges: Vec<(FluidNodeId, FluidNodeId)>,
    ) -> Self {
        Self {
            loop_id,
            nodes,
            edges,
            splitters: Vec::new(),
            mixers: Vec::new(),
        }
    }

    /// Attaches splitter branch data to this topology.
    #[must_use]
    pub fn with_splitters(mut self, splitters: Vec<SplitterNode>) -> Self {
        self.splitters = splitters;
        self
    }

    /// Attaches mixer branch data to this topology.
    #[must_use]
    pub fn with_mixers(mut self, mixers: Vec<MixerNode>) -> Self {
        self.mixers = mixers;
        self
    }

    /// Returns the maximum absolute mass imbalance across all nodes [kg/s].
    #[must_use]
    pub fn max_mass_imbalance_kg_s(&self) -> f64 {
        self.nodes
            .iter()
            .map(FluidNode::mass_imbalance_kg_s)
            .fold(0.0, f64::max)
    }

    /// Returns the number of nodes where imbalance exceeds `MASS_FLOW_TOLERANCE`.
    #[must_use]
    pub fn num_conservation_violations(&self) -> u32 {
        self.nodes
            .iter()
            .filter(|n| n.mass_imbalance_kg_s() > MASS_FLOW_TOLERANCE)
            .count() as u32
    }

    /// Resets all node flow accumulators to zero.
    pub fn zero_nodes(&mut self) {
        for node in &mut self.nodes {
            node.zero();
        }
    }
}

/// Mass-flow tolerance for conservation checks [kg/s].
///
/// EnergyPlus `DataBranchAirLoopPlant::MassFlowTolerance` = 1e-9 kg/s
/// (`vendors/EnergyPlus/src/EnergyPlus/DataBranchAirLoopPlant.hh:64`).
/// HARES uses the same tolerance so that conservation enforcement matches
/// the EnergyPlus reference implementation.
pub const MASS_FLOW_TOLERANCE: f64 = 1e-9;

/// Temperature limits for a fluid loop [°C].
///
/// When temperatures computed by the fluid solver exceed these bounds,
/// the solver clamps them and emits a throttled warning. Defaults are
/// fluid-type-specific and represent physically reasonable operating
/// ranges for residential hydronic and refrigerant systems.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct FluidTempLimits {
    pub min_temp_c: f64,
    pub max_temp_c: f64,
}

impl FluidTempLimits {
    /// Sensible defaults per fluid type.
    ///
    /// - Water: 0–100°C (freezing/boiling at atmospheric pressure).
    ///   ASHRAE HoF 2021 Ch.1 §1.2: water is liquid at 0–100°C at 101.325 kPa.
    /// - Glycol: −40 to +120°C (practical range for 50% propylene glycol
    ///   in HVAC applications per EnergyPlus FluidProperties.cc).
    /// - Refrigerant: −50 to +150°C (typical saturated working range for
    ///   R-134a and residential heat-pump refrigerants per ASHRAE Handbook
    ///   of Refrigeration 2010, Ch.30, Table 9).
    #[must_use]
    pub fn default_for(fluid_type: FluidType) -> Self {
        match fluid_type {
            FluidType::Water => Self {
                min_temp_c: 0.0,
                max_temp_c: 100.0,
            },
            FluidType::Glycol => Self {
                min_temp_c: -40.0,
                max_temp_c: 120.0,
            },
            FluidType::Refrigerant => Self {
                min_temp_c: -50.0,
                max_temp_c: 150.0,
            },
        }
    }
}

/// Fluid loop state resolved each timestep by the fluid domain solver.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FluidLoopState {
    pub loop_id: LoopId,
    pub fluid_type: FluidType,
    /// Total heat injected by heat sources (boilers, heat pumps in heating
    /// mode) [W]. Always non-negative.
    pub heating_power_w: f64,
    /// Total heat extracted by heat sinks (distribution coils, radiators,
    /// cooling coils) [W]. Always non-negative.
    pub cooling_power_w: f64,
    /// Algebraic sum `heating_power_w - cooling_power_w` [W]. Should be near
    /// zero for a balanced loop and reflect the net imbalance otherwise.
    pub net_power_w: f64,
    pub mean_supply_temp_c: f64,
    pub mean_return_temp_c: f64,
}

/// Encode/decode `Vec<FluidLoopState>` to/from `Vec<f64>` for `DomainUpdate::custom_payload`.
///
/// Layout per loop:
/// `[loop_id as f64, fluid_type as f64, heating_power_w, cooling_power_w,
///   net_power_w, mean_supply_temp_c, mean_return_temp_c]`.
pub struct FluidDomainPayload;

impl FluidDomainPayload {
    #[must_use]
    pub fn encode(states: &[FluidLoopState]) -> Option<Vec<f64>> {
        if states.is_empty() {
            return None;
        }
        let mut payload = Vec::with_capacity(states.len() * 7);
        for state in states {
            payload.push(f64::from(state.loop_id.0));
            payload.push(fluid_type_to_f64(state.fluid_type));
            payload.push(state.heating_power_w);
            payload.push(state.cooling_power_w);
            payload.push(state.net_power_w);
            payload.push(state.mean_supply_temp_c);
            payload.push(state.mean_return_temp_c);
        }
        Some(payload)
    }

    pub fn decode(payload: &[f64]) -> Result<Vec<FluidLoopState>, HaresError> {
        if !payload.len().is_multiple_of(7) {
            return Err(HaresError::Envelope(format!(
                "invalid fluid payload length {}, expected multiple of 7",
                payload.len()
            )));
        }
        let mut states = Vec::with_capacity(payload.len() / 7);
        for chunk in payload.chunks_exact(7) {
            let loop_id = f64_to_loop_id(chunk[0])?;
            let fluid_type = f64_to_fluid_type(chunk[1])?;
            states.push(FluidLoopState {
                loop_id,
                fluid_type,
                heating_power_w: chunk[2],
                cooling_power_w: chunk[3],
                net_power_w: chunk[4],
                mean_supply_temp_c: chunk[5],
                mean_return_temp_c: chunk[6],
            });
        }
        Ok(states)
    }
}

fn fluid_type_to_f64(fluid_type: FluidType) -> f64 {
    match fluid_type {
        FluidType::Water => 0.0,
        FluidType::Glycol => 1.0,
        FluidType::Refrigerant => 2.0,
    }
}

fn f64_to_fluid_type(value: f64) -> Result<FluidType, HaresError> {
    #[allow(clippy::cast_possible_truncation)]
    match value.round() as i64 {
        0 => Ok(FluidType::Water),
        1 => Ok(FluidType::Glycol),
        2 => Ok(FluidType::Refrigerant),
        _ => Err(HaresError::Envelope(format!(
            "invalid fluid type discriminator {value}"
        ))),
    }
}

fn f64_to_loop_id(value: f64) -> Result<LoopId, HaresError> {
    if !value.is_finite() || value < 0.0 || value > f64::from(u16::MAX) || value.fract() != 0.0 {
        return Err(HaresError::Envelope(format!(
            "invalid loop id value {value}"
        )));
    }
    Ok(LoopId(value as u16))
}

#[cfg(test)]
mod tests {
    use super::{FluidDomainPayload, FluidLoopState};
    use crate::{FluidType, LoopId};

    #[test]
    fn payload_round_trip() {
        let states = vec![
            FluidLoopState {
                loop_id: LoopId(1),
                fluid_type: FluidType::Water,
                heating_power_w: 41860.0,
                cooling_power_w: 0.0,
                net_power_w: 41860.0,
                mean_supply_temp_c: 60.0,
                mean_return_temp_c: 40.0,
            },
            FluidLoopState {
                loop_id: LoopId(2),
                fluid_type: FluidType::Glycol,
                heating_power_w: 1200.0,
                cooling_power_w: 0.0,
                net_power_w: 1200.0,
                mean_supply_temp_c: 45.0,
                mean_return_temp_c: 42.5,
            },
        ];
        let payload = FluidDomainPayload::encode(&states).unwrap();
        let decoded = FluidDomainPayload::decode(&payload).unwrap();
        assert_eq!(decoded, states);
    }

    #[test]
    fn encode_empty_returns_none() {
        assert_eq!(FluidDomainPayload::encode(&[]), None);
    }

    #[test]
    fn decode_rejects_invalid_length() {
        let err = FluidDomainPayload::decode(&[1.0, 0.0, 2.0]).unwrap_err();
        assert!(err.to_string().contains("invalid fluid payload length"));
    }

    #[test]
    fn decode_rejects_invalid_fluid_type_discriminator() {
        // Valid length (7), but fluid_type discriminator 99.0 is invalid
        let payload = vec![1.0, 99.0, 500.0, 0.0, 500.0, 60.0, 40.0];
        let err = FluidDomainPayload::decode(&payload).unwrap_err();
        assert!(
            err.to_string().contains("invalid fluid type discriminator"),
            "expected fluid type error, got: {err}"
        );
    }
}
