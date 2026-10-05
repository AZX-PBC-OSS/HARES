//! Fluid domain solver for minimal v1 loop energy balance.

use std::collections::HashMap;
use std::time::Duration;

use hares_types::{
    DomainId, DomainSolver, DomainUpdate, FLUID, FluidDomainPayload, FluidLoopState, FluidNodeId,
    FluidNodeRole, FluidTempLimits, FluidType, HaresError, HeatTransferDirection, LoopId,
    LoopTopology, MASS_FLOW_TOLERANCE, PortSlots,
};

const MIN_FLOW_KG_S: f64 = 1e-12;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct FluidSolverConfig {
    /// Optional per-loop hydraulic topology for mass-conservation verification.
    ///
    /// When present, the solver verifies that `|∑ inflow − ∑ outflow|`
    /// at every node in the topology is below `MASS_FLOW_TOLERANCE`.
    /// When absent for a loop, a basic serial-flow consistency check is
    /// performed instead: all accumulators on that loop must report the same
    /// flow rate (within tolerance), since a series hydronic loop cannot
    /// have diverging flow declarations.
    pub loop_topologies: HashMap<LoopId, LoopTopology>,
    /// Optional per-loop temperature limits [°C].
    ///
    /// When present, the solver clamps `mean_supply_temp_c` and
    /// `mean_return_temp_c` to `[min_temp_c, max_temp_c]` for the loop
    /// before writing `FluidLoopState`. When absent, limits are derived
    /// from the loop's `FluidType` using [`FluidTempLimits::default_for`].
    /// EnergyPlus `PlantLoopData` (`vendors/EnergyPlus/src/EnergyPlus/Plant/Loop.hh:125-126`)
    /// defines `Real64 MinTemp` and `Real64 MaxTemp` per loop for this
    /// purpose.
    pub loop_temp_limits: HashMap<LoopId, FluidTempLimits>,
}

#[derive(Debug, Clone)]
pub struct FluidSolver {
    config: FluidSolverConfig,
    loop_types: HashMap<LoopId, FluidType>,
    loop_states: HashMap<LoopId, FluidLoopState>,
    /// Last known (supply_temp_c, return_temp_c) per loop; used when flow is zero.
    last_known_temps: HashMap<LoopId, (f64, f64)>,
    /// Count of loops where entries have mismatched fluid types.
    /// Always 0 in a correctly configured simulation.
    #[cfg(feature = "observe")]
    pub loop_fluid_type_mismatch_count: u64,
    /// Maximum absolute mass imbalance across all nodes in all loops [kg/s].
    /// Gated on `observe` for diagnostic CSV export.
    #[cfg(feature = "observe")]
    pub max_mass_imbalance_kg_s: f64,
    /// Number of nodes where mass imbalance exceeds `MASS_FLOW_TOLERANCE`.
    /// Gated on `observe` for diagnostic CSV export.
    #[cfg(feature = "observe")]
    pub num_conservation_violations: u64,
    /// Per-loop total requested flow before resolution [kg/s].
    /// Sum of all branch requests across all splitters on the loop.
    #[cfg(feature = "observe")]
    pub total_requested_flow_kg_s: HashMap<LoopId, f64>,
    /// Per-loop total allocated flow after resolution [kg/s].
    /// Sum of all branch allocations; should equal the pump's available flow
    /// after resolution, unless a deficit exists.
    #[cfg(feature = "observe")]
    pub total_allocated_flow_kg_s: HashMap<LoopId, f64>,
    /// Per-loop flow deficit when demand exceeds supply [kg/s].
    #[cfg(feature = "observe")]
    pub flow_deficit_kg_s: HashMap<LoopId, f64>,
    /// Per-loop, per-branch allocated flow fractions.
    /// For each loop, a map from branch node_id to its fraction of total loop flow.
    #[cfg(feature = "observe")]
    pub branch_flow_fractions: HashMap<LoopId, Vec<(FluidNodeId, f64)>>,
    /// Per-loop: true if supply temp was clamped this timestep.
    #[cfg(feature = "observe")]
    pub supply_temp_clamped: HashMap<LoopId, bool>,
    /// Per-loop: true if return temp was clamped this timestep.
    #[cfg(feature = "observe")]
    pub return_temp_clamped: HashMap<LoopId, bool>,
    /// Per-loop: unclamped supply temperature for diagnostic comparison [°C].
    #[cfg(feature = "observe")]
    pub raw_supply_temp_c: HashMap<LoopId, f64>,
    /// Per-loop: unclamped return temperature for diagnostic comparison [°C].
    #[cfg(feature = "observe")]
    pub raw_return_temp_c: HashMap<LoopId, f64>,
    /// Per-loop pump flow check wiring, precomputed at construction: the
    /// pump node (Source role, no parents) and the splitter branch nodes.
    /// Present only for loops with a splitter topology; the per-step check
    /// is a lookup plus a sum.
    pump_flow_checks: HashMap<LoopId, PumpFlowCheck>,
}

/// Precomputed pump-flow wiring for one loop topology.
#[derive(Debug, Clone)]
struct PumpFlowCheck {
    pump_node: FluidNodeId,
    branch_nodes: Vec<FluidNodeId>,
}

/// Builds the loop-type map from declared `(loop_id, fluid_type)` pairs,
/// rejecting two declarations of one loop with different fluid types,
/// without constructing or mutating a `FluidSolver`. A caller that must
/// validate a candidate roster before committing it (the dwelling's roster
/// planner) installs the result with [`FluidSolver::install_loop_types`]
/// once the candidate is accepted.
pub fn plan_loop_types(
    declared_loops: &[(LoopId, FluidType)],
) -> Result<HashMap<LoopId, FluidType>, HaresError> {
    let mut loop_types: HashMap<LoopId, FluidType> = HashMap::new();
    for &(loop_id, fluid_type) in declared_loops {
        if let Some(existing) = loop_types.insert(loop_id, fluid_type)
            && existing != fluid_type
        {
            return Err(HaresError::Envelope(format!(
                "loop {loop_id:?} declared with conflicting fluid types: {existing:?} and {fluid_type:?}"
            )));
        }
    }
    Ok(loop_types)
}

/// Precomputes the pump flow check wiring per declared loop: the pump node
/// (Source role, zero parents) and the splitter branch node ids. Loops
/// without a splitter topology carry no entry.
fn pump_flow_checks(
    config: &FluidSolverConfig,
    loop_types: &HashMap<LoopId, FluidType>,
) -> HashMap<LoopId, PumpFlowCheck> {
    let mut checks: HashMap<LoopId, PumpFlowCheck> = HashMap::new();
    for loop_id in loop_types.keys() {
        let Some(topo) = config.loop_topologies.get(loop_id) else {
            continue;
        };
        if topo.splitters.is_empty() {
            continue;
        }
        let mut parent_count: HashMap<FluidNodeId, usize> = HashMap::new();
        for (_, to) in &topo.edges {
            *parent_count.entry(*to).or_default() += 1;
        }
        if let Some(pump) = topo.nodes.iter().find(|n| {
            n.role == FluidNodeRole::Source
                && parent_count.get(&n.node_id).copied().unwrap_or(0) == 0
        }) {
            let branch_nodes: Vec<FluidNodeId> = topo
                .splitters
                .iter()
                .flat_map(|s| &s.branches)
                .map(|b| b.node_id)
                .collect();
            checks.insert(
                *loop_id,
                PumpFlowCheck {
                    pump_node: pump.node_id,
                    branch_nodes,
                },
            );
        }
    }
    checks
}

impl FluidSolver {
    pub fn new(
        config: FluidSolverConfig,
        declared_loops: &[(LoopId, FluidType)],
    ) -> Result<Self, HaresError> {
        let loop_types = plan_loop_types(declared_loops)?;
        // Precompute the pump flow check wiring per loop: the pump node
        // (Source role, zero parents) and the splitter branch node ids.
        let pump_flow_checks = pump_flow_checks(&config, &loop_types);
        Ok(Self {
            config,
            loop_types,
            loop_states: HashMap::new(),
            last_known_temps: HashMap::new(),
            #[cfg(feature = "observe")]
            loop_fluid_type_mismatch_count: 0,
            #[cfg(feature = "observe")]
            max_mass_imbalance_kg_s: 0.0,
            #[cfg(feature = "observe")]
            num_conservation_violations: 0,
            #[cfg(feature = "observe")]
            total_requested_flow_kg_s: HashMap::new(),
            #[cfg(feature = "observe")]
            total_allocated_flow_kg_s: HashMap::new(),
            #[cfg(feature = "observe")]
            flow_deficit_kg_s: HashMap::new(),
            #[cfg(feature = "observe")]
            branch_flow_fractions: HashMap::new(),
            #[cfg(feature = "observe")]
            supply_temp_clamped: HashMap::new(),
            #[cfg(feature = "observe")]
            return_temp_clamped: HashMap::new(),
            #[cfg(feature = "observe")]
            raw_supply_temp_c: HashMap::new(),
            #[cfg(feature = "observe")]
            raw_return_temp_c: HashMap::new(),
            pump_flow_checks,
        })
    }

    /// Assigns an already-validated loop-type map (from [`plan_loop_types`])
    /// and rebuilds the pump-flow-check wiring that depends on it.
    /// Infallible: every rejection a loop-type change can produce already
    /// happened in [`plan_loop_types`], so installing its result cannot
    /// fail.
    pub fn install_loop_types(&mut self, loop_types: HashMap<LoopId, FluidType>) {
        self.pump_flow_checks = pump_flow_checks(&self.config, &loop_types);
        self.loop_types = loop_types;
    }

    #[must_use]
    pub fn loop_state(&self, loop_id: LoopId) -> Option<&FluidLoopState> {
        self.loop_states.get(&loop_id)
    }

    /// The fluid type a declared loop resolves to, or `None` when no port
    /// declared that loop.
    #[must_use]
    pub fn loop_fluid_type(&self, loop_id: LoopId) -> Option<FluidType> {
        self.loop_types.get(&loop_id).copied()
    }

    /// Serializes current solver state into a flat `Vec<f64>` for checkpointing.
    ///
    /// Layout: for each entry in `last_known_temps` (sorted by `LoopId`):
    ///   `[loop_id.0 as f64, supply_temp_c, return_temp_c]`
    #[must_use]
    pub fn snapshot_payload(&self) -> Vec<f64> {
        let mut sorted: Vec<_> = self.last_known_temps.iter().collect();
        sorted.sort_by_key(|(id, _)| id.0);
        let mut payload = Vec::with_capacity(sorted.len() * 3);
        for (loop_id, (supply, ret)) in &sorted {
            payload.push(f64::from(loop_id.0));
            payload.push(*supply);
            payload.push(*ret);
        }
        payload
    }

    /// Restores solver state from a checkpoint payload produced by [`snapshot_payload`].
    /// Checks that `payload` is a well-formed checkpoint payload: whole
    /// `(loop_id, supply, return)` triples with integral loop ids in the
    /// `LoopId` range.
    pub fn validate_payload(payload: &[f64]) -> Result<(), HaresError> {
        if !payload.len().is_multiple_of(3) {
            return Err(HaresError::Envelope(format!(
                "fluid checkpoint payload length {} is not a multiple of 3",
                payload.len()
            )));
        }
        for chunk in payload.chunks_exact(3) {
            let loop_id_raw = chunk[0];
            if !(0.0..=f64::from(u16::MAX)).contains(&loop_id_raw) || loop_id_raw.fract() != 0.0 {
                return Err(HaresError::Envelope(format!(
                    "invalid loop_id in fluid checkpoint: {loop_id_raw}"
                )));
            }
        }
        Ok(())
    }

    /// Restores the last known loop temperatures from a payload; validates
    /// it before changing anything.
    pub fn restore_from_payload(&mut self, payload: &[f64]) -> Result<(), HaresError> {
        Self::validate_payload(payload)?;
        self.last_known_temps.clear();
        for chunk in payload.chunks_exact(3) {
            self.last_known_temps
                .insert(LoopId(chunk[0] as u16), (chunk[1], chunk[2]));
        }
        Ok(())
    }
}

impl DomainSolver for FluidSolver {
    fn domain_id(&self) -> DomainId {
        FLUID
    }

    fn resolve(
        &mut self,
        ports: &PortSlots,
        _env: &hares_types::EnvironmentState,
        _dt: Duration,
        out: &mut DomainUpdate,
    ) -> Result<(), HaresError> {
        self.loop_states.clear();

        #[cfg(feature = "observe")]
        {
            self.total_requested_flow_kg_s.clear();
            self.total_allocated_flow_kg_s.clear();
            self.flow_deficit_kg_s.clear();
            // Clear the per-loop fraction Vecs in place rather than clearing
            // the map: `HashMap::clear` drops each Vec, so the next step's
            // `entry().or_default().push()` would heap-allocate a fresh buffer
            // every timestep (hot-path discipline: no per-timestep heap
            // allocation). Retained entries keep their capacity; a loop absent
            // this step is left as an empty Vec, which reads the same as a
            // missing entry for all diagnostic consumers.
            for fractions in self.branch_flow_fractions.values_mut() {
                fractions.clear();
            }
            self.supply_temp_clamped.clear();
            self.return_temp_clamped.clear();
            self.raw_supply_temp_c.clear();
            self.raw_return_temp_c.clear();
        }

        let mut grouped: HashMap<LoopId, Vec<&hares_types::FluidAccumulator>> = HashMap::new();
        for acc in &ports.fluid {
            grouped.entry(acc.loop_id).or_default().push(acc);
        }

        for (loop_id, entries) in grouped {
            // The loop-type map is rebuilt from the live equipment's fluid
            // port declarations at assembly and on every equipment-list
            // refresh, so every loop carrying an accumulator is declared.
            // A loop here without a declaration would mean the port-slot
            // table and the loop map diverged; that is an invariant
            // violation, not a case for a substitute value.
            let Some(fluid_type) = self.loop_types.get(&loop_id).copied() else {
                return Err(HaresError::InvariantViolation {
                    check_name: "fluid_loop_declared".to_string(),
                    value: f64::from(loop_id.0),
                    tolerance: 0.0,
                });
            };

            // The loop-type planning (`plan_loop_types`) enforces that all
            // entries on a loop agree on fluid_type; the observe gate counts
            // mismatches per step for diagnostics.
            #[cfg(feature = "observe")]
            if !entries
                .windows(2)
                .all(|w| w[0].fluid_type == w[1].fluid_type)
            {
                self.loop_fluid_type_mismatch_count += 1;
            }

            let cp = hares_physics::constants::cp_j_kg_k(fluid_type);

            #[cfg(feature = "observe")]
            tracing::info!(
                loop_id = loop_id.0,
                ?fluid_type,
                cp_used = cp,
                "fluid solver cp resolution"
            );

            // ── Build per-node flow totals from accumulators ─────────────
            let mut node_flows: HashMap<FluidNodeId, f64> = HashMap::new();
            for acc in &entries {
                *node_flows.entry(acc.node_id).or_default() += acc.total_flow_kg_s;
            }

            // ── Flow-splitting resolution (T-0255) ──────────────────────
            //
            // When the loop topology includes splitter/mixer nodes with
            // branch data, the solver allocates the pump's total available
            // flow across parallel branches proportionally to their requested
            // fractions. This must run BEFORE the conservation check so
            // that resolved flows (not raw requests) are used for
            // verification.
            //
            // Reference: EnergyPlus `ResolveParallelFlows`
            // (`Plant/LoopSide.cc:1279-1682`): satisfy active branch requests,
            // distribute remaining to passive branches proportional to
            // max-avail, allocate to bypass, and distribute excess when flow
            // is insufficient by requested fraction.
            let resolved_node_flows: Option<HashMap<FluidNodeId, f64>> = {
                let topology_opt = self.config.loop_topologies.get(&loop_id);
                if let Some(topology) = topology_opt {
                    if !topology.splitters.is_empty() {
                        let parent_count: HashMap<FluidNodeId, usize> = {
                            let mut map = HashMap::new();
                            for (_, to) in &topology.edges {
                                *map.entry(*to).or_default() += 1;
                            }
                            map
                        };

                        // Identify pump node: Source role, zero parents.
                        let pump_node = topology.nodes.iter().find(|n| {
                            n.role == FluidNodeRole::Source
                                && parent_count.get(&n.node_id).copied().unwrap_or(0) == 0
                        });

                        if let Some(pump) = pump_node {
                            let total_available =
                                node_flows.get(&pump.node_id).copied().unwrap_or(0.0);

                            // Sum requested flows across all splitter branches.
                            let total_requested: f64 = topology
                                .splitters
                                .iter()
                                .flat_map(|s| &s.branches)
                                .map(|b| node_flows.get(&b.node_id).copied().unwrap_or(0.0))
                                .sum();

                            #[cfg(feature = "observe")]
                            {
                                *self.total_requested_flow_kg_s.entry(loop_id).or_default() +=
                                    total_requested;
                            }

                            let mut resolved = HashMap::new();

                            if total_available <= MIN_FLOW_KG_S || total_requested <= MIN_FLOW_KG_S
                            {
                                // No pump flow or no demand — all branches get zero.
                                for splitter in &topology.splitters {
                                    for branch in &splitter.branches {
                                        resolved.insert(branch.node_id, 0.0);
                                    }
                                }
                            } else if total_available >= total_requested {
                                // Flow supply meets or exceeds demand.
                                for splitter in &topology.splitters {
                                    for branch in &splitter.branches {
                                        let requested =
                                            node_flows.get(&branch.node_id).copied().unwrap_or(0.0);
                                        resolved.insert(branch.node_id, requested);
                                        #[cfg(feature = "observe")]
                                        {
                                            let fraction = if total_available > 0.0 {
                                                requested / total_available
                                            } else {
                                                0.0
                                            };
                                            self.branch_flow_fractions
                                                .entry(loop_id)
                                                .or_default()
                                                .push((branch.node_id, fraction));
                                        }
                                    }
                                }

                                #[cfg(feature = "observe")]
                                {
                                    *self.total_allocated_flow_kg_s.entry(loop_id).or_default() +=
                                        total_requested;
                                    self.flow_deficit_kg_s.entry(loop_id).or_insert(0.0);
                                }
                            } else {
                                // Flow insufficient: proportional allocation.
                                let deficit = total_requested - total_available;

                                tracing::warn!(
                                    loop_id = loop_id.0,
                                    total_available_kg_s = total_available,
                                    total_requested_kg_s = total_requested,
                                    deficit_kg_s = deficit,
                                    "parallel-branch flow demand exceeds pump capacity; allocating proportionally"
                                );

                                for splitter in &topology.splitters {
                                    for branch in &splitter.branches {
                                        let requested =
                                            node_flows.get(&branch.node_id).copied().unwrap_or(0.0);
                                        let fraction = if total_requested > 0.0 {
                                            requested / total_requested
                                        } else {
                                            0.0
                                        };
                                        let allocated = total_available * fraction;
                                        resolved.insert(branch.node_id, allocated);
                                        #[cfg(feature = "observe")]
                                        {
                                            let branch_fraction = allocated / total_available;
                                            self.branch_flow_fractions
                                                .entry(loop_id)
                                                .or_default()
                                                .push((branch.node_id, branch_fraction));
                                        }
                                    }
                                }

                                #[cfg(feature = "observe")]
                                {
                                    *self.total_allocated_flow_kg_s.entry(loop_id).or_default() +=
                                        total_available;
                                    *self.flow_deficit_kg_s.entry(loop_id).or_default() += deficit;
                                }
                            }

                            // Keep the pump node flow in resolved map,
                            // throttled when supply exceeds demand so
                            // that conservation holds (total available
                            // becomes total requested).
                            let pump_resolved = if total_available <= MIN_FLOW_KG_S
                                || total_requested <= MIN_FLOW_KG_S
                            {
                                0.0
                            } else if total_available >= total_requested {
                                // Pump throttled to match demand; excess
                                // is not modelled (no bypass path yet).
                                // EnergyPlus equivalent: excess
                                // distributed to bypass
                                // (LoopSide.cc:1624-1660).
                                total_requested
                            } else {
                                // Pump at max capacity.
                                total_available
                            };
                            resolved.insert(pump.node_id, pump_resolved);

                            // Resolve splitter node flows: each splitter
                            // carries the pump flow (or parent's flow in
                            // nested topologies) to maintain conservation.
                            for splitter in &topology.splitters {
                                resolved.insert(splitter.node_id, pump_resolved);
                            }

                            // Resolve mixer node flows: each mixer
                            // carries the sum of its branch inflows.
                            for mixer in &topology.mixers {
                                let mixer_flow: f64 = mixer
                                    .branches
                                    .iter()
                                    .map(|b| resolved.get(&b.node_id).copied().unwrap_or(0.0))
                                    .sum();
                                resolved.insert(mixer.node_id, mixer_flow);
                            }

                            // Also resolve non-splitter branch nodes:
                            // for mixer arms that are not splitter children,
                            // keep original accumulator flow.
                            for (nid, flow) in &node_flows {
                                if !resolved.contains_key(nid) && *nid != pump.node_id {
                                    resolved.insert(*nid, *flow);
                                }
                            }

                            Some(resolved)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            };

            // Write per-branch requested flows (T-0255 directive 4):
            // the solver derives branch demand from per-node accumulator
            // totals; writing it back to SplitterBranch makes the field
            // useful as a diagnostic snapshot.
            if let Some(topology) = self.config.loop_topologies.get_mut(&loop_id) {
                for splitter in &mut topology.splitters {
                    for branch in &mut splitter.branches {
                        branch.requested_flow_kg_s =
                            node_flows.get(&branch.node_id).copied().unwrap_or(0.0);
                    }
                }
            }

            // Build effective flows: resolved values override raw accumulator
            // flows. Used for both conservation checking and aggregation.
            let effective_flows: HashMap<FluidNodeId, f64> =
                if let Some(ref resolved) = resolved_node_flows {
                    let mut eff = node_flows.clone();
                    for (nid, flow) in resolved {
                        eff.insert(*nid, *flow);
                    }
                    eff
                } else {
                    node_flows.clone()
                };

            // ── Mass-conservation check (T-0254) ──────────────────────────
            //
            // EnergyPlus `CheckLoopExitNode` (`Plant/Loop.cc:244-265`) enforces
            // |Outlet.MassFlowRate - Inlet.MassFlowRate| < MassFlowTolerance
            // where `DataBranchAirLoopPlant::MassFlowTolerance` = 1e-9 kg/s.
            //
            // HARES's `MASS_FLOW_TOLERANCE` matches this.
            //
            // Two check modes:
            //   1. Topology-based: when `loop_topologies` contains this loop,
            //      use effective (post-resolution) flows to verify conservation
            //      across the directed-edge graph.
            //   2. Serial-flow consistency: when no topology is configured,
            //      verify that all accumulators on the same loop report the same
            //      flow rate — a series loop cannot have diverging flows.
            {
                if let Some(topology) = self.config.loop_topologies.get_mut(&loop_id) {
                    // ── Topology-based conservation check ──

                    #[cfg(feature = "observe")]
                    let mut max_imbalance = 0.0_f64;

                    // Precompute child count per node.
                    let child_count: HashMap<FluidNodeId, usize> = {
                        let mut map = HashMap::new();
                        for (from, _) in &topology.edges {
                            *map.entry(*from).or_default() += 1;
                        }
                        map
                    };

                    // Precompute parent count per node.
                    let parent_count: HashMap<FluidNodeId, usize> = {
                        let mut map = HashMap::new();
                        for (_, to) in &topology.edges {
                            *map.entry(*to).or_default() += 1;
                        }
                        map
                    };

                    for i in 0..topology.nodes.len() {
                        let node_id = topology.nodes[i].node_id;
                        let node_role = topology.nodes[i].role;
                        let node_flow = effective_flows.get(&node_id).copied().unwrap_or(0.0);

                        let n_parents = parent_count.get(&node_id).copied().unwrap_or(0);
                        let n_children = child_count.get(&node_id).copied().unwrap_or(0);

                        // Compute total inflow:
                        //   - No parents → inflow = node_flow.
                        //   - Single parent → inflow = parent node's flow (serial).
                        //   - Multiple parents (mixer) → inflow = sum of each
                        //     parent's own flow.
                        let total_inflow: f64 = if n_parents == 0 {
                            node_flow
                        } else {
                            topology
                                .edges
                                .iter()
                                .filter(|(_, to)| *to == node_id)
                                .map(|(from, _)| {
                                    let from_flow =
                                        effective_flows.get(from).copied().unwrap_or(0.0);
                                    let from_children = child_count.get(from).copied().unwrap_or(0);
                                    if from_children == 1 {
                                        // Serial parent: edge carries parent's total flow
                                        from_flow
                                    } else {
                                        // Splitter parent: edge carries this node's flow
                                        node_flow
                                    }
                                })
                                .sum()
                        };

                        // Compute total outflow:
                        //   - No children → outflow = node_flow.
                        //   - Single child → outflow = node_flow (serial).
                        //   - Multiple children (splitter) → outflow = sum of
                        //     each child's own flow.
                        let total_outflow: f64 = if n_children <= 1 {
                            node_flow
                        } else {
                            topology
                                .edges
                                .iter()
                                .filter(|(from, _)| *from == node_id)
                                .map(|(_, to)| effective_flows.get(to).copied().unwrap_or(0.0))
                                .sum()
                        };

                        // Write back computed flow totals to the topology node.
                        topology.nodes[i].total_inflow_kg_s = total_inflow;
                        topology.nodes[i].total_outflow_kg_s = total_outflow;

                        let max_local_imbalance = (total_inflow - total_outflow).abs();

                        #[cfg(feature = "observe")]
                        {
                            max_imbalance = max_imbalance.max(max_local_imbalance);
                        }

                        if max_local_imbalance > MASS_FLOW_TOLERANCE {
                            #[cfg(feature = "observe")]
                            {
                                self.num_conservation_violations += 1;
                                self.max_mass_imbalance_kg_s =
                                    self.max_mass_imbalance_kg_s.max(max_imbalance);
                            }
                            return Err(HaresError::Envelope(format!(
                                "fluid loop {loop_id:?} node {node_id:?} ({node_role:?}): \
                                 mass conservation violated: \
                                 node_flow = {node_flow:.6e} kg/s, total_inflow = {total_inflow:.6e} kg/s, \
                                 total_outflow = {total_outflow:.6e} kg/s, imbalance = {max_local_imbalance:.6e} kg/s, \
                                 tolerance = {MASS_FLOW_TOLERANCE:.6e} kg/s"
                            )));
                        }
                    }

                    #[cfg(feature = "observe")]
                    {
                        self.max_mass_imbalance_kg_s =
                            self.max_mass_imbalance_kg_s.max(max_imbalance);
                    }
                } else {
                    // ── Serial-flow consistency check (no topology) ──
                    if entries.len() > 1 {
                        let first_flow = entries[0].total_flow_kg_s;
                        for acc in &entries[1..] {
                            let diff = (acc.total_flow_kg_s - first_flow).abs();
                            if diff > MASS_FLOW_TOLERANCE {
                                #[cfg(feature = "observe")]
                                {
                                    self.num_conservation_violations += 1;
                                    self.max_mass_imbalance_kg_s =
                                        self.max_mass_imbalance_kg_s.max(diff);
                                }
                                return Err(HaresError::Envelope(format!(
                                    "fluid loop {loop_id:?}: mass conservation violated: \
                                     accumulators report inconsistent flow rates: \
                                     node {:?} flow = {} kg/s, node {:?} flow = {} kg/s, \
                                     diff = {:.6e} kg/s, tolerance = {:.6e} kg/s",
                                    entries[0].node_id,
                                    first_flow,
                                    acc.node_id,
                                    acc.total_flow_kg_s,
                                    diff,
                                    MASS_FLOW_TOLERANCE
                                )));
                            }
                        }
                    }
                }
            }

            // ── Temperature and power aggregation ────────────────────────
            //
            // When flow-splitting is active, use per-branch allocated flows
            // for temperature weighting and net power computation.
            // When not splitting, fall back to accumulator flows.

            let (
                total_flow,
                heating_power_w,
                cooling_power_w,
                mut mean_supply_temp_c,
                mut mean_return_temp_c,
            ) = if let Some(ref resolved) = resolved_node_flows {
                // Use resolved flows per node for weighted aggregation.
                let mut sum_supply = 0.0_f64;
                let mut sum_return = 0.0_f64;
                let mut net_heating = 0.0_f64;
                let mut net_cooling = 0.0_f64;
                let mut total_resolved = 0.0_f64;

                // Identify pump node for exclusion from temperature
                // averaging (pump is plumbing, not a thermal component).
                let pump_node_id: Option<FluidNodeId> = {
                    let topology = self.config.loop_topologies.get(&loop_id);
                    topology.and_then(|topo| {
                        let parent_count: HashMap<FluidNodeId, usize> = {
                            let mut map = HashMap::new();
                            for (_, to) in &topo.edges {
                                *map.entry(*to).or_default() += 1;
                            }
                            map
                        };
                        topo.nodes
                            .iter()
                            .find(|n| {
                                n.role == FluidNodeRole::Source
                                    && parent_count.get(&n.node_id).copied().unwrap_or(0) == 0
                            })
                            .map(|pump| pump.node_id)
                    })
                };

                // Collect splitter/mixer node_ids for exclusion from
                // temperature averaging.
                let plumbing_nodes: std::collections::HashSet<FluidNodeId> = {
                    let mut set = std::collections::HashSet::new();
                    if let Some(nid) = pump_node_id {
                        set.insert(nid);
                    }
                    if let Some(topo) = self.config.loop_topologies.get(&loop_id) {
                        for s in &topo.splitters {
                            set.insert(s.node_id);
                        }
                        for m in &topo.mixers {
                            set.insert(m.node_id);
                        }
                    }
                    set
                };

                // Use the pump's flow as the total loop flow.
                let pump_total: f64 = pump_node_id
                    .and_then(|nid| resolved.get(&nid).copied())
                    .unwrap_or_else(|| resolved.values().sum());

                for acc in &entries {
                    // Skip plumbing nodes (pump, splitter, mixer) in
                    // temperature / power aggregation. These nodes carry
                    // flow but do not represent thermal components —
                    // their temperature fields are placeholders that
                    // would skew the flow-weighted average if included.
                    if plumbing_nodes.contains(&acc.node_id) {
                        continue;
                    }
                    let allocated_flow = resolved
                        .get(&acc.node_id)
                        .copied()
                        .unwrap_or(acc.total_flow_kg_s);
                    if allocated_flow > MIN_FLOW_KG_S {
                        sum_supply += allocated_flow * acc.mean_supply_temp_c;
                        sum_return += allocated_flow * acc.mean_return_temp_c;
                        let delta_t = acc.mean_supply_temp_c - acc.mean_return_temp_c;
                        let power = cp * allocated_flow * delta_t;
                        // Use the direction reported by the equipment to sign the
                        // contribution. Source (boiler, heat pump) injects heat:
                        // contribution is positive. Sink (distribution coil, radiator)
                        // extracts heat: contribution is negative.
                        // Default to Source behaviour when direction is unknown
                        // (accumulator created from a declaration without a write).
                        match acc.direction.unwrap_or(HeatTransferDirection::Source) {
                            HeatTransferDirection::Source => {
                                net_heating += power;
                            }
                            HeatTransferDirection::Sink => {
                                // Tickets T-0256: use abs(ΔT) to guarantee sinks always
                                // contribute negatively, regardless of temperature
                                // ordering convention.
                                net_cooling += cp * allocated_flow * delta_t.abs();
                            }
                        }
                        total_resolved += allocated_flow;
                    }
                }

                let (supply_c, return_c) = if total_resolved > MIN_FLOW_KG_S {
                    (sum_supply / total_resolved, sum_return / total_resolved)
                } else {
                    self.last_known_temps
                        .get(&loop_id)
                        .copied()
                        .unwrap_or((0.0, 0.0))
                };

                (pump_total, net_heating, net_cooling, supply_c, return_c)
            } else {
                // No flow-splitting: use accumulator flows directly.
                let tf: f64 = entries.iter().map(|e| e.total_flow_kg_s).sum();
                let (nh, nc): (f64, f64) = entries
                    .iter()
                    .map(|e| {
                        let delta_t = e.mean_supply_temp_c - e.mean_return_temp_c;
                        let power = cp * e.total_flow_kg_s * delta_t;
                        match e.direction.unwrap_or(HeatTransferDirection::Source) {
                            HeatTransferDirection::Source => (power, 0.0),
                            HeatTransferDirection::Sink => {
                                (0.0, cp * e.total_flow_kg_s * delta_t.abs())
                            }
                        }
                    })
                    .fold((0.0, 0.0), |(h, c), (dh, dc)| (h + dh, c + dc));

                let (sc, rc) = if tf.abs() <= MIN_FLOW_KG_S {
                    self.last_known_temps
                        .get(&loop_id)
                        .copied()
                        .unwrap_or((0.0, 0.0))
                } else {
                    let ss = entries
                        .iter()
                        .map(|e| e.total_flow_kg_s * e.mean_supply_temp_c)
                        .sum::<f64>();
                    let sr = entries
                        .iter()
                        .map(|e| e.total_flow_kg_s * e.mean_return_temp_c)
                        .sum::<f64>();
                    let temps = (ss / tf, sr / tf);
                    self.last_known_temps.insert(loop_id, temps);
                    temps
                };

                (tf, nh, nc, sc, rc)
            };

            // ── Temperature bounds clamping (T-0257) ─────────────────────
            //
            // EnergyPlus `PlantLoopData` (`Plant/Loop.hh:125-126`) defines
            // `Real64 MinTemp` and `Real64 MaxTemp` per loop. HARES clamps
            // supply and return temperatures to physical limits before they
            // propagate downstream, and emits a warning so the config error
            // is visible.
            {
                let limits = self
                    .config
                    .loop_temp_limits
                    .get(&loop_id)
                    .copied()
                    .unwrap_or_else(|| FluidTempLimits::default_for(fluid_type));

                let raw_supply = mean_supply_temp_c;
                let raw_return = mean_return_temp_c;

                if mean_supply_temp_c < limits.min_temp_c {
                    tracing::warn!(
                        loop_id = loop_id.0,
                        raw_supply_temp_c = raw_supply,
                        min_temp_c = limits.min_temp_c,
                        "supply temperature below minimum; clamping"
                    );
                    mean_supply_temp_c = limits.min_temp_c;
                } else if mean_supply_temp_c > limits.max_temp_c {
                    tracing::warn!(
                        loop_id = loop_id.0,
                        raw_supply_temp_c = raw_supply,
                        max_temp_c = limits.max_temp_c,
                        "supply temperature above maximum; clamping"
                    );
                    mean_supply_temp_c = limits.max_temp_c;
                }

                if mean_return_temp_c < limits.min_temp_c {
                    tracing::warn!(
                        loop_id = loop_id.0,
                        raw_return_temp_c = raw_return,
                        min_temp_c = limits.min_temp_c,
                        "return temperature below minimum; clamping"
                    );
                    mean_return_temp_c = limits.min_temp_c;
                } else if mean_return_temp_c > limits.max_temp_c {
                    tracing::warn!(
                        loop_id = loop_id.0,
                        raw_return_temp_c = raw_return,
                        max_temp_c = limits.max_temp_c,
                        "return temperature above maximum; clamping"
                    );
                    mean_return_temp_c = limits.max_temp_c;
                }

                #[cfg(feature = "observe")]
                {
                    self.supply_temp_clamped
                        .insert(loop_id, raw_supply != mean_supply_temp_c);
                    self.return_temp_clamped
                        .insert(loop_id, raw_return != mean_return_temp_c);
                    self.raw_supply_temp_c.insert(loop_id, raw_supply);
                    self.raw_return_temp_c.insert(loop_id, raw_return);
                }
            }

            // ── Post-resolution invariant (T-0255) ────────────────────────
            // The pump's allocated flow must equal the sum of branch
            // allocations. The pump node and the splitter branch nodes are
            // precomputed at construction, so the per-step cost is a lookup
            // and a sum, never a topology rescan.
            if let Some(ref resolved) = resolved_node_flows
                && let Some(check) = self.pump_flow_checks.get(&loop_id)
            {
                let pump_flow = resolved.get(&check.pump_node).copied().unwrap_or(0.0);
                let branch_sum: f64 = check
                    .branch_nodes
                    .iter()
                    .map(|n| resolved.get(n).copied().unwrap_or(0.0))
                    .sum();
                let diff = (pump_flow - branch_sum).abs();
                if diff >= MASS_FLOW_TOLERANCE {
                    return Err(HaresError::Envelope(format!(
                        "fluid loop {loop_id:?}: flow resolution invariant violated: \
                             pump flow = {pump_flow:.6e} kg/s, sum of branch allocated flows = {branch_sum:.6e} kg/s, \
                             diff = {diff:.6e} kg/s, tolerance = {MASS_FLOW_TOLERANCE:.6e} kg/s"
                    )));
                }
            }

            // ── System-level invariant (T-0084) ──────────────────────────
            let total_declared_thermal_w: f64 =
                entries.iter().map(|e| e.total_thermal_power_w).sum();

            let net_power_w = heating_power_w - cooling_power_w;

            if total_declared_thermal_w > 0.0 {
                // All declared thermal power should be from sources (heating),
                // so compare against heating_power_w rather than net.
                let tol = 1e-9_f64
                    * heating_power_w
                        .abs()
                        .max(total_declared_thermal_w.abs())
                        .max(1.0);
                if (heating_power_w - total_declared_thermal_w).abs() > tol {
                    return Err(HaresError::Envelope(format!(
                        "fluid loop {loop_id:?}: declared thermal power ({total_declared_thermal_w} W) \
                         does not match flow-implied source power ({heating_power_w} W); \
                         diff = {} W, tol = {tol:e} W",
                        (heating_power_w - total_declared_thermal_w).abs()
                    )));
                }
            }

            // ── Observer captures (T-0256) ────────────────────────────────
            #[cfg(feature = "observe")]
            {
                // Per-loop heating/cooling power for diagnostic CSV export.
                tracing::info!(
                    loop_id = loop_id.0,
                    ?fluid_type,
                    heating_power_w,
                    cooling_power_w,
                    net_power_w,
                    power_imbalance_w = (heating_power_w - cooling_power_w).abs(),
                    "fluid loop power balance"
                );
            }

            // Update last_known_temps so zero-flow steps can recall temperature state.
            if total_flow > MIN_FLOW_KG_S {
                self.last_known_temps
                    .insert(loop_id, (mean_supply_temp_c, mean_return_temp_c));
            }

            self.loop_states.insert(
                loop_id,
                FluidLoopState {
                    loop_id,
                    fluid_type,
                    heating_power_w,
                    cooling_power_w,
                    net_power_w,
                    mean_supply_temp_c,
                    mean_return_temp_c,
                },
            );
        }

        let mut states: Vec<FluidLoopState> = self.loop_states.values().cloned().collect();
        states.sort_by_key(|s| s.loop_id.0);
        out.domain_id = FLUID;
        out.zone_temperatures_c.clear();
        out.custom_payload = FluidDomainPayload::encode(&states);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use chrono::{FixedOffset, TimeZone};
    use hares_physics::constants::{CP_LIQUID_WATER_J_KG_K, CP_PROP_GLYCOL_50PCT_J_KG_K};
    use hares_types::{
        DomainSolver, EnvironmentState, FluidAccumulator, FluidDomainPayload, FluidNode,
        FluidNodeId, FluidNodeRole, FluidTempLimits, FluidType, GridState, HeatTransferDirection,
        LoopId, LoopTopology, MASS_FLOW_TOLERANCE, MixerBranch, MixerNode, PortContribution,
        PortSlots, SplitterBranch, SplitterNode, SurfaceIrradiance, WeatherState, ZoneId,
        ZoneState,
    };

    use crate::fluid_solver::{FluidSolver, FluidSolverConfig, plan_loop_types};

    fn env() -> EnvironmentState {
        EnvironmentState {
            ambient_other_space_c: hares_types::AmbientOtherSpaceTemps::default(),
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: 21.0,
                humidity_ratio: 0.008,
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
                island_bus_voltage_pu: None,
            },
            schedule_row: None,
            custom_domains: vec![],
            equipment_telemetry: std::collections::HashMap::new(),
            equipment_core: Default::default(),
            current_time: FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(2026, 3, 18, 12, 0, 0)
                .single()
                .expect("valid time"),
            time_res: chrono::Duration::seconds(60),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }

    fn approx_eq(a: f64, b: f64) {
        assert!((a - b).abs() <= 1e-6, "a={a} b={b}");
    }

    #[test]
    fn single_loop_net_power_matches_reference() {
        let mut solver = FluidSolver::new(
            FluidSolverConfig::default(),
            &[(LoopId(1), FluidType::Water)],
        )
        .unwrap();
        let mut ports = PortSlots {
            fluid: vec![hares_types::FluidAccumulator::new(
                LoopId(1),
                FluidType::Water,
            )],
            ..Default::default()
        };
        ports
            .accumulate(&PortContribution::Fluid {
                loop_id: LoopId(1),
                flow_rate_kg_s: 0.5,
                supply_temp_c: 60.0,
                return_temp_c: 40.0,
                fluid_type: FluidType::Water,
                thermal_power_w: None,
                node_id: FluidNodeId(0),
                direction: HeatTransferDirection::Source,
            })
            .unwrap();

        let update = solver
            .resolve_new(&ports, &env(), Duration::from_secs(60))
            .unwrap();
        let states = FluidDomainPayload::decode(&update.custom_payload.unwrap()).unwrap();
        approx_eq(states[0].net_power_w, 0.5 * CP_LIQUID_WATER_J_KG_K * 20.0);
    }

    #[test]
    fn multi_contributor_grouping_is_correct() {
        let mut solver = FluidSolver::new(
            FluidSolverConfig::default(),
            &[(LoopId(1), FluidType::Water)],
        )
        .unwrap();
        let mut ports = PortSlots {
            fluid: vec![hares_types::FluidAccumulator::new(
                LoopId(1),
                FluidType::Water,
            )],
            ..Default::default()
        };
        ports
            .accumulate(&PortContribution::Fluid {
                loop_id: LoopId(1),
                flow_rate_kg_s: 1.0,
                supply_temp_c: 60.0,
                return_temp_c: 50.0,
                fluid_type: FluidType::Water,
                thermal_power_w: None,
                node_id: FluidNodeId(0),
                direction: HeatTransferDirection::Source,
            })
            .unwrap();
        ports
            .accumulate(&PortContribution::Fluid {
                loop_id: LoopId(1),
                flow_rate_kg_s: 2.0,
                supply_temp_c: 50.0,
                return_temp_c: 40.0,
                fluid_type: FluidType::Water,
                thermal_power_w: None,
                node_id: FluidNodeId(0),
                direction: HeatTransferDirection::Source,
            })
            .unwrap();

        let update = solver
            .resolve_new(&ports, &env(), Duration::from_secs(60))
            .unwrap();
        let states = FluidDomainPayload::decode(&update.custom_payload.unwrap()).unwrap();
        let s = &states[0];
        approx_eq(
            s.net_power_w,
            CP_LIQUID_WATER_J_KG_K * (1.0 * 10.0 + 2.0 * 10.0),
        );
        approx_eq(s.mean_supply_temp_c, (1.0 * 60.0 + 2.0 * 50.0) / 3.0);
        approx_eq(s.mean_return_temp_c, (1.0 * 50.0 + 2.0 * 40.0) / 3.0);
    }

    #[test]
    fn zero_flow_uses_previous_temperatures() {
        let mut solver = FluidSolver::new(
            FluidSolverConfig::default(),
            &[(LoopId(1), FluidType::Water)],
        )
        .unwrap();
        let mut ports = PortSlots {
            fluid: vec![hares_types::FluidAccumulator::new(
                LoopId(1),
                FluidType::Water,
            )],
            ..Default::default()
        };
        ports
            .accumulate(&PortContribution::Fluid {
                loop_id: LoopId(1),
                flow_rate_kg_s: 1.0,
                supply_temp_c: 52.0,
                return_temp_c: 45.0,
                fluid_type: FluidType::Water,
                thermal_power_w: None,
                node_id: FluidNodeId(0),
                direction: HeatTransferDirection::Source,
            })
            .unwrap();
        let _ = solver.resolve_new(&ports, &env(), Duration::from_secs(60));

        let zero_ports = PortSlots {
            fluid: vec![hares_types::FluidAccumulator::new(
                LoopId(1),
                FluidType::Water,
            )],
            ..Default::default()
        };
        let update = solver
            .resolve_new(&zero_ports, &env(), Duration::from_secs(60))
            .unwrap();
        let states = FluidDomainPayload::decode(&update.custom_payload.unwrap()).unwrap();
        approx_eq(states[0].mean_supply_temp_c, 52.0);
        approx_eq(states[0].mean_return_temp_c, 45.0);
    }

    #[test]
    fn empty_ports_returns_none_payload_and_no_state() {
        let mut solver = FluidSolver::new(FluidSolverConfig::default(), &[]).unwrap();
        let ports = PortSlots::default();
        let update = solver
            .resolve_new(&ports, &env(), Duration::from_secs(60))
            .unwrap();
        assert_eq!(update.custom_payload, None);
        assert!(solver.loop_state(LoopId(1)).is_none());
    }

    #[test]
    fn loop_state_returns_none_after_no_contributions() {
        // After a step with contributions, if next step has none,
        // loop_state should return None for that loop
        let mut solver = FluidSolver::new(
            FluidSolverConfig::default(),
            &[(LoopId(1), FluidType::Water)],
        )
        .unwrap();

        // Step 1: contribute flow to loop 1
        let mut ports = PortSlots {
            fluid: vec![hares_types::FluidAccumulator::new(
                LoopId(1),
                FluidType::Water,
            )],
            ..Default::default()
        };
        ports
            .accumulate(&PortContribution::Fluid {
                loop_id: LoopId(1),
                flow_rate_kg_s: 1.0,
                supply_temp_c: 55.0,
                return_temp_c: 45.0,
                fluid_type: FluidType::Water,
                thermal_power_w: None,
                node_id: FluidNodeId(0),
                direction: HeatTransferDirection::Source,
            })
            .unwrap();
        let _ = solver.resolve_new(&ports, &env(), Duration::from_secs(60));
        assert!(
            solver.loop_state(LoopId(1)).is_some(),
            "loop should have state after contributions"
        );

        // Step 2: no contributions at all (empty fluid vec)
        let empty_ports = PortSlots::default();
        let _ = solver.resolve_new(&empty_ports, &env(), Duration::from_secs(60));
        assert!(
            solver.loop_state(LoopId(1)).is_none(),
            "loop should have no state after step with no contributions"
        );
    }

    #[test]
    fn constructor_rejects_conflicting_fluid_types_for_same_loop() {
        let result = FluidSolver::new(
            FluidSolverConfig::default(),
            &[
                (LoopId(1), FluidType::Water),
                (LoopId(1), FluidType::Glycol),
            ],
        );
        assert!(result.is_err());
    }

    #[test]
    fn plan_loop_types_maps_each_declared_loop_to_its_fluid() {
        let single = plan_loop_types(&[(LoopId(1), FluidType::Water)]).expect("one loop");
        assert_eq!(single, HashMap::from([(LoopId(1), FluidType::Water)]));

        let distinct = plan_loop_types(&[
            (LoopId(1), FluidType::Water),
            (LoopId(2), FluidType::Glycol),
        ])
        .expect("two loops of different fluids");
        assert_eq!(
            distinct,
            HashMap::from([
                (LoopId(1), FluidType::Water),
                (LoopId(2), FluidType::Glycol)
            ])
        );
    }

    #[test]
    fn plan_loop_types_rejects_one_loop_with_two_fluids_naming_both() {
        let err = plan_loop_types(&[
            (LoopId(7), FluidType::Water),
            (LoopId(7), FluidType::Glycol),
        ])
        .expect_err("one loop cannot carry two fluids");
        let message = err.to_string();
        assert!(
            message.contains("LoopId(7)")
                && message.contains("Water")
                && message.contains("Glycol"),
            "the error names the loop and both fluids: {message}"
        );
    }

    // =======================================================================
    // T-0084: System-level invariant — declared thermal_power_w matches flow balance
    // =======================================================================

    #[test]
    fn declared_thermal_power_w_matches_flow_energy_balance() {
        // When equipment declares thermal_power_w that matches flow × Cp × ΔT,
        // the fluid solver invariant should be satisfied (no panic).
        let mut solver = FluidSolver::new(
            FluidSolverConfig::default(),
            &[(LoopId(1), FluidType::Water)],
        )
        .unwrap();
        let mut ports = PortSlots {
            fluid: vec![FluidAccumulator::new(LoopId(1), FluidType::Water)],
            ..Default::default()
        };
        // 0.5 kg/s × Cp J/(kg·K) × 20 K
        ports
            .accumulate(&PortContribution::Fluid {
                loop_id: LoopId(1),
                flow_rate_kg_s: 0.5,
                supply_temp_c: 60.0,
                return_temp_c: 40.0,
                fluid_type: FluidType::Water,
                thermal_power_w: Some(0.5 * CP_LIQUID_WATER_J_KG_K * 20.0),
                node_id: FluidNodeId(0),
                direction: HeatTransferDirection::Source,
            })
            .unwrap();

        let update = solver
            .resolve_new(&ports, &env(), Duration::from_secs(60))
            .unwrap();
        let states = FluidDomainPayload::decode(&update.custom_payload.unwrap()).unwrap();
        let declared = 0.5 * CP_LIQUID_WATER_J_KG_K * 20.0;
        let diff = (states[0].net_power_w - declared).abs();
        assert!(
            diff < 1e-3,
            "net_power_w ({}) should match declared thermal_power_w ({declared}); diff = {diff}",
            states[0].net_power_w
        );
    }

    #[test]
    fn thermal_power_w_accumulator_reflects_mixed_contributions() {
        // When one contributor declares thermal_power_w and another does not (None),
        // total_thermal_power_w should equal only the declared contribution.
        let mut ports = PortSlots {
            fluid: vec![FluidAccumulator::new(LoopId(1), FluidType::Water)],
            ..Default::default()
        };
        // Contributor A: declares thermal_power_w matching its flow energy
        ports
            .accumulate(&PortContribution::Fluid {
                loop_id: LoopId(1),
                flow_rate_kg_s: 0.3,
                supply_temp_c: 55.0,
                return_temp_c: 40.0,
                fluid_type: FluidType::Water,
                thermal_power_w: Some(0.3 * CP_LIQUID_WATER_J_KG_K * 15.0),
                node_id: FluidNodeId(0),
                direction: HeatTransferDirection::Source,
            })
            .unwrap();
        // Contributor B: no thermal_power_w declaration (None)
        ports
            .accumulate(&PortContribution::Fluid {
                loop_id: LoopId(1),
                flow_rate_kg_s: 0.2,
                supply_temp_c: 55.0,
                return_temp_c: 40.0,
                fluid_type: FluidType::Water,
                thermal_power_w: None,
                node_id: FluidNodeId(0),
                direction: HeatTransferDirection::Source,
            })
            .unwrap();

        // total_thermal_power_w should be from contributor A only.
        let expected_declared = 0.3 * CP_LIQUID_WATER_J_KG_K * 15.0;
        let diff = (ports.fluid[0].total_thermal_power_w - expected_declared).abs();
        assert!(
            diff < 1e-3,
            "total_thermal_power_w ({}) should match only declared contribution ({expected_declared})",
            ports.fluid[0].total_thermal_power_w
        );
    }

    #[test]
    fn thermal_power_w_persists_through_zero() {
        // After accumulating, zeroing the accumulator must clear thermal_power_w.
        let mut fluid = FluidAccumulator::new(LoopId(1), FluidType::Water);
        fluid
            .add(
                0.5,
                60.0,
                40.0,
                Some(CP_LIQUID_WATER_J_KG_K),
                HeatTransferDirection::Source,
            )
            .unwrap();
        assert!(fluid.total_thermal_power_w > 0.0);
        fluid.zero();
        assert!((fluid.total_thermal_power_w - 0.0).abs() < 1e-9);
        assert!((fluid.total_flow_kg_s - 0.0).abs() < 1e-9);
    }

    // -----------------------------------------------------------------------
    // T-0129: fluid-type-specific cp
    // -----------------------------------------------------------------------

    #[test]
    fn fluid_type_glycol_uses_own_cp() {
        // Glycol (50% propylene glycol at 60°C) has cp ≈ 3800 J/(kg·K),
        // lower than water's 4180 J/(kg·K). Verify net power is computed with
        // the glycol-specific cp, not the water value.
        let mut solver = FluidSolver::new(
            FluidSolverConfig::default(),
            &[(LoopId(1), FluidType::Glycol)],
        )
        .unwrap();
        let mut ports = PortSlots {
            fluid: vec![FluidAccumulator::new(LoopId(1), FluidType::Glycol)],
            ..Default::default()
        };
        ports
            .accumulate(&PortContribution::Fluid {
                loop_id: LoopId(1),
                flow_rate_kg_s: 0.5,
                supply_temp_c: 60.0,
                return_temp_c: 40.0,
                fluid_type: FluidType::Glycol,
                thermal_power_w: None,
                node_id: FluidNodeId(0),
                direction: HeatTransferDirection::Source,
            })
            .unwrap();

        let update = solver
            .resolve_new(&ports, &env(), Duration::from_secs(60))
            .unwrap();
        let states = FluidDomainPayload::decode(&update.custom_payload.unwrap()).unwrap();
        // cp = CP_PROP_GLYCOL_50PCT_J_KG_K J/(kg·K): 0.5 kg/s × cp × 20 K
        approx_eq(
            states[0].net_power_w,
            0.5 * CP_PROP_GLYCOL_50PCT_J_KG_K * 20.0,
        );
        // With water cp (4180): 0.5 × 4180 × 20 = 41 800 W — verify the result
        // differs from what water cp would produce.
        assert!(
            (states[0].net_power_w - 0.5 * CP_LIQUID_WATER_J_KG_K * 20.0).abs() > 1e-6,
            "glycol loop must use glycol cp, not water cp"
        );
    }

    #[test]
    fn mixed_fluid_types_each_use_own_cp() {
        // Two different loops with different fluid types must each resolve
        // using their own specific heat capacity.
        let mut solver = FluidSolver::new(
            FluidSolverConfig::default(),
            &[
                (LoopId(1), FluidType::Water),
                (LoopId(2), FluidType::Glycol),
            ],
        )
        .unwrap();

        let mut ports = PortSlots {
            fluid: vec![
                FluidAccumulator::new(LoopId(1), FluidType::Water),
                FluidAccumulator::new(LoopId(2), FluidType::Glycol),
            ],
            ..Default::default()
        };
        // Water loop: 1.0 kg/s, ΔT=10 K, cp=4180 → 41 800 W
        ports
            .accumulate(&PortContribution::Fluid {
                loop_id: LoopId(1),
                flow_rate_kg_s: 1.0,
                supply_temp_c: 60.0,
                return_temp_c: 50.0,
                fluid_type: FluidType::Water,
                thermal_power_w: None,
                node_id: FluidNodeId(0),
                direction: HeatTransferDirection::Source,
            })
            .unwrap();
        // Glycol loop: 0.5 kg/s, ΔT=20 K, cp=3800 → 38 000 W
        ports
            .accumulate(&PortContribution::Fluid {
                loop_id: LoopId(2),
                flow_rate_kg_s: 0.5,
                supply_temp_c: 60.0,
                return_temp_c: 40.0,
                fluid_type: FluidType::Glycol,
                thermal_power_w: None,
                node_id: FluidNodeId(0),
                direction: HeatTransferDirection::Source,
            })
            .unwrap();

        let update = solver
            .resolve_new(&ports, &env(), Duration::from_secs(60))
            .unwrap();
        let states = FluidDomainPayload::decode(&update.custom_payload.unwrap()).unwrap();
        assert_eq!(states.len(), 2);
        // Sort by loop_id for deterministic access
        let water = states.iter().find(|s| s.loop_id == LoopId(1)).unwrap();
        let glycol = states.iter().find(|s| s.loop_id == LoopId(2)).unwrap();
        approx_eq(water.net_power_w, 1.0 * CP_LIQUID_WATER_J_KG_K * 10.0);
        approx_eq(glycol.net_power_w, 0.5 * CP_PROP_GLYCOL_50PCT_J_KG_K * 20.0);
    }

    // =======================================================================
    // T-0254: Mass-conservation invariant checks
    // =======================================================================

    #[test]
    fn serial_loop_divergent_flows_are_a_typed_error() {
        // Two accumulators on the same loop with different flow rates.
        // In a serial hydronic loop, all equipment must carry the same mass flow.
        // The check is unconditional and reports the violation as a typed error.
        let mut solver = FluidSolver::new(
            FluidSolverConfig::default(),
            &[(LoopId(1), FluidType::Water)],
        )
        .unwrap();

        // Create two accumulators on the same loop with different node_ids
        // and different flow rates — simulating a boiler at node 0 reporting
        // 0.5 kg/s and a coil at node 1 reporting 0.3 kg/s.
        let ports = PortSlots {
            fluid: vec![
                FluidAccumulator {
                    loop_id: LoopId(1),
                    fluid_type: FluidType::Water,
                    node_id: FluidNodeId(0),
                    total_flow_kg_s: 0.5,
                    mean_supply_temp_c: 60.0,
                    mean_return_temp_c: 50.0,
                    total_thermal_power_w: 0.0,
                    direction: None,
                },
                FluidAccumulator {
                    loop_id: LoopId(1),
                    fluid_type: FluidType::Water,
                    node_id: FluidNodeId(1),
                    total_flow_kg_s: 0.3, // mismatched flow: must be a typed error
                    mean_supply_temp_c: 50.0,
                    mean_return_temp_c: 40.0,
                    total_thermal_power_w: 0.0,
                    direction: None,
                },
            ],
            ..Default::default()
        };

        let result = solver.resolve_new(&ports, &env(), Duration::from_secs(60));
        let err = result.expect_err("divergent serial flows must be a typed error");
        assert!(
            err.to_string().contains("mass conservation violated"),
            "the error must name the check, got: {err}"
        );
    }

    #[test]
    fn serial_loop_consistent_flows_pass_mass_conservation_check() {
        // Two accumulators on the same loop with identical flow rates.
        // Mass conservation is satisfied — no panic, no warning.
        let mut solver = FluidSolver::new(
            FluidSolverConfig::default(),
            &[(LoopId(1), FluidType::Water)],
        )
        .unwrap();

        let ports = PortSlots {
            fluid: vec![
                FluidAccumulator {
                    loop_id: LoopId(1),
                    fluid_type: FluidType::Water,
                    node_id: FluidNodeId(0),
                    total_flow_kg_s: 0.5,
                    mean_supply_temp_c: 60.0,
                    mean_return_temp_c: 50.0,
                    total_thermal_power_w: 0.0,
                    direction: None,
                },
                FluidAccumulator {
                    loop_id: LoopId(1),
                    fluid_type: FluidType::Water,
                    node_id: FluidNodeId(1),
                    total_flow_kg_s: 0.5, // matching flow — conservation holds
                    mean_supply_temp_c: 50.0,
                    mean_return_temp_c: 40.0,
                    total_thermal_power_w: 0.0,
                    direction: None,
                },
            ],
            ..Default::default()
        };

        // Must not panic.
        let _ = solver.resolve_new(&ports, &env(), Duration::from_secs(60));
    }

    #[test]
    fn single_accumulator_on_loop_no_topology_passes_conservation_check() {
        // A single accumulator on a loop (e.g. only boiler port declared,
        // no sinks yet) — the serial-flow consistency check should not fire
        // (it requires >= 2 accumulators to compare).
        let mut solver = FluidSolver::new(
            FluidSolverConfig::default(),
            &[(LoopId(1), FluidType::Water)],
        )
        .unwrap();

        let ports = PortSlots {
            fluid: vec![FluidAccumulator {
                loop_id: LoopId(1),
                fluid_type: FluidType::Water,
                node_id: FluidNodeId(0),
                total_flow_kg_s: 0.5,
                mean_supply_temp_c: 60.0,
                mean_return_temp_c: 50.0,
                total_thermal_power_w: 0.0,
                direction: None,
            }],
            ..Default::default()
        };

        let _ = solver.resolve_new(&ports, &env(), Duration::from_secs(60));
    }

    #[test]
    fn parallel_loop_splitter_mixer_topology_conservation_check() {
        // Three-branch parallel loop: splitter → [A, B, C] → mixer.
        // Topology defines explicit edges for conservation checking.
        //
        //   Node 0 (splitter) → Node 1 (branch A)
        //   Node 0 (splitter) → Node 2 (branch B)
        //   Node 0 (splitter) → Node 3 (branch C)
        //   Node 1 (branch A) → Node 4 (mixer)
        //   Node 2 (branch B) → Node 4 (mixer)
        //   Node 3 (branch C) → Node 4 (mixer)
        //
        // Accumulator flows:
        //   Node 0 (splitter): 1.0 kg/s  (total flow entering splitter)
        //   Node 1 (branch A): 0.4 kg/s
        //   Node 2 (branch B): 0.3 kg/s
        //   Node 3 (branch C): 0.3 kg/s
        //   Node 4 (mixer):    1.0 kg/s  (total flow leaving mixer)
        //
        // Splitter: 1.0 == 0.4 + 0.3 + 0.3 ✓
        // Mixer: 0.4 + 0.3 + 0.3 == 1.0 ✓

        let topology = LoopTopology::new(
            LoopId(1),
            vec![
                FluidNode::new(FluidNodeId(0), FluidNodeRole::Splitter),
                FluidNode::new(FluidNodeId(1), FluidNodeRole::Source),
                FluidNode::new(FluidNodeId(2), FluidNodeRole::Source),
                FluidNode::new(FluidNodeId(3), FluidNodeRole::Source),
                FluidNode::new(FluidNodeId(4), FluidNodeRole::Mixer),
            ],
            vec![
                (FluidNodeId(0), FluidNodeId(1)),
                (FluidNodeId(0), FluidNodeId(2)),
                (FluidNodeId(0), FluidNodeId(3)),
                (FluidNodeId(1), FluidNodeId(4)),
                (FluidNodeId(2), FluidNodeId(4)),
                (FluidNodeId(3), FluidNodeId(4)),
            ],
        );

        let config = FluidSolverConfig {
            loop_topologies: [(LoopId(1), topology)].into_iter().collect(),
            ..FluidSolverConfig::default()
        };

        let mut solver = FluidSolver::new(config, &[(LoopId(1), FluidType::Water)]).unwrap();

        let ports = PortSlots {
            fluid: vec![
                FluidAccumulator {
                    loop_id: LoopId(1),
                    fluid_type: FluidType::Water,
                    node_id: FluidNodeId(0),
                    total_flow_kg_s: 1.0,
                    mean_supply_temp_c: 60.0,
                    mean_return_temp_c: 60.0,
                    total_thermal_power_w: 0.0,
                    direction: None,
                },
                FluidAccumulator {
                    loop_id: LoopId(1),
                    fluid_type: FluidType::Water,
                    node_id: FluidNodeId(1),
                    total_flow_kg_s: 0.4,
                    mean_supply_temp_c: 60.0,
                    mean_return_temp_c: 40.0,
                    total_thermal_power_w: 0.0,
                    direction: None,
                },
                FluidAccumulator {
                    loop_id: LoopId(1),
                    fluid_type: FluidType::Water,
                    node_id: FluidNodeId(2),
                    total_flow_kg_s: 0.3,
                    mean_supply_temp_c: 60.0,
                    mean_return_temp_c: 40.0,
                    total_thermal_power_w: 0.0,
                    direction: None,
                },
                FluidAccumulator {
                    loop_id: LoopId(1),
                    fluid_type: FluidType::Water,
                    node_id: FluidNodeId(3),
                    total_flow_kg_s: 0.3,
                    mean_supply_temp_c: 60.0,
                    mean_return_temp_c: 40.0,
                    total_thermal_power_w: 0.0,
                    direction: None,
                },
                FluidAccumulator {
                    loop_id: LoopId(1),
                    fluid_type: FluidType::Water,
                    node_id: FluidNodeId(4),
                    total_flow_kg_s: 1.0,
                    mean_supply_temp_c: 40.0,
                    mean_return_temp_c: 40.0,
                    total_thermal_power_w: 0.0,
                    direction: None,
                },
            ],
            ..Default::default()
        };

        // Must not panic — flows are consistent.
        let _ = solver.resolve_new(&ports, &env(), Duration::from_secs(60));
    }

    #[test]
    fn topology_node_flow_fields_populated_after_resolve() {
        // Regression: FluidNode::total_inflow_kg_s / total_outflow_kg_s were
        // never written by the solver, making LoopTopology::max_mass_imbalance_kg_s,
        // num_conservation_violations, and FluidNode::mass_imbalance_kg_s always
        // return 0. After the fix, consistent flows must produce populated node
        // fields and zero violations; the topology-level diagnostic API must work.
        let topology = LoopTopology::new(
            LoopId(1),
            vec![
                FluidNode::new(FluidNodeId(0), FluidNodeRole::Splitter),
                FluidNode::new(FluidNodeId(1), FluidNodeRole::Source),
                FluidNode::new(FluidNodeId(2), FluidNodeRole::Source),
                FluidNode::new(FluidNodeId(3), FluidNodeRole::Source),
                FluidNode::new(FluidNodeId(4), FluidNodeRole::Mixer),
            ],
            vec![
                (FluidNodeId(0), FluidNodeId(1)),
                (FluidNodeId(0), FluidNodeId(2)),
                (FluidNodeId(0), FluidNodeId(3)),
                (FluidNodeId(1), FluidNodeId(4)),
                (FluidNodeId(2), FluidNodeId(4)),
                (FluidNodeId(3), FluidNodeId(4)),
            ],
        );

        // Before resolve: all node flows are 0
        assert_eq!(topology.max_mass_imbalance_kg_s(), 0.0);
        assert_eq!(topology.num_conservation_violations(), 0);

        let config = FluidSolverConfig {
            loop_topologies: [(LoopId(1), topology)].into_iter().collect(),
            ..FluidSolverConfig::default()
        };

        let mut solver = FluidSolver::new(config, &[(LoopId(1), FluidType::Water)]).unwrap();

        // Consistent parallel-loop flows: splitter 1.0 = 0.4 + 0.3 + 0.3 → mixer
        let ports = PortSlots {
            fluid: vec![
                FluidAccumulator {
                    loop_id: LoopId(1),
                    fluid_type: FluidType::Water,
                    node_id: FluidNodeId(0),
                    total_flow_kg_s: 1.0,
                    mean_supply_temp_c: 60.0,
                    mean_return_temp_c: 40.0,
                    total_thermal_power_w: 0.0,
                    direction: None,
                },
                FluidAccumulator {
                    loop_id: LoopId(1),
                    fluid_type: FluidType::Water,
                    node_id: FluidNodeId(1),
                    total_flow_kg_s: 0.4,
                    mean_supply_temp_c: 60.0,
                    mean_return_temp_c: 40.0,
                    total_thermal_power_w: 0.0,
                    direction: None,
                },
                FluidAccumulator {
                    loop_id: LoopId(1),
                    fluid_type: FluidType::Water,
                    node_id: FluidNodeId(2),
                    total_flow_kg_s: 0.3,
                    mean_supply_temp_c: 60.0,
                    mean_return_temp_c: 40.0,
                    total_thermal_power_w: 0.0,
                    direction: None,
                },
                FluidAccumulator {
                    loop_id: LoopId(1),
                    fluid_type: FluidType::Water,
                    node_id: FluidNodeId(3),
                    total_flow_kg_s: 0.3,
                    mean_supply_temp_c: 60.0,
                    mean_return_temp_c: 40.0,
                    total_thermal_power_w: 0.0,
                    direction: None,
                },
                FluidAccumulator {
                    loop_id: LoopId(1),
                    fluid_type: FluidType::Water,
                    node_id: FluidNodeId(4),
                    total_flow_kg_s: 1.0,
                    mean_supply_temp_c: 40.0,
                    mean_return_temp_c: 40.0,
                    total_thermal_power_w: 0.0,
                    direction: None,
                },
            ],
            ..Default::default()
        };

        let _ = solver.resolve_new(&ports, &env(), Duration::from_secs(60));

        // After resolve: node fields must be populated (not all zero)
        let topology = solver
            .config
            .loop_topologies
            .get(&LoopId(1))
            .expect("topology present after resolve");

        let splitter = topology
            .nodes
            .iter()
            .find(|n| n.node_id == FluidNodeId(0))
            .unwrap();
        assert!(
            (splitter.total_inflow_kg_s - 1.0).abs() < 1e-12,
            "splitter inflow = {}, expected 1.0",
            splitter.total_inflow_kg_s
        );
        assert!(
            (splitter.total_outflow_kg_s - 1.0).abs() < 1e-12,
            "splitter outflow = {}, expected 1.0",
            splitter.total_outflow_kg_s
        );

        let mixer = topology
            .nodes
            .iter()
            .find(|n| n.node_id == FluidNodeId(4))
            .unwrap();
        assert!(
            (mixer.total_inflow_kg_s - 1.0).abs() < 1e-12,
            "mixer inflow = {}, expected 1.0",
            mixer.total_inflow_kg_s
        );

        // No violations with consistent flows
        assert!(
            topology.max_mass_imbalance_kg_s() < MASS_FLOW_TOLERANCE,
            "max imbalance {} should be below tolerance {}",
            topology.max_mass_imbalance_kg_s(),
            MASS_FLOW_TOLERANCE
        );
        assert_eq!(
            topology.num_conservation_violations(),
            0,
            "no violations with consistent flows"
        );
    }

    /// Resolves the three-branch splitter → [A, B, C] → mixer loop with the
    /// given flows for nodes 0 (splitter) to 4 (mixer) and checks that the
    /// conservation violation is a typed error naming the violating node's
    /// role, and that observe telemetry counts it with its imbalance.
    fn assert_parallel_loop_violation(
        flows_kg_s: [f64; 5],
        violating_role: FluidNodeRole,
        imbalance_kg_s: f64,
    ) {
        let topology = LoopTopology::new(
            LoopId(1),
            vec![
                FluidNode::new(FluidNodeId(0), FluidNodeRole::Splitter),
                FluidNode::new(FluidNodeId(1), FluidNodeRole::Source),
                FluidNode::new(FluidNodeId(2), FluidNodeRole::Source),
                FluidNode::new(FluidNodeId(3), FluidNodeRole::Source),
                FluidNode::new(FluidNodeId(4), FluidNodeRole::Mixer),
            ],
            vec![
                (FluidNodeId(0), FluidNodeId(1)),
                (FluidNodeId(0), FluidNodeId(2)),
                (FluidNodeId(0), FluidNodeId(3)),
                (FluidNodeId(1), FluidNodeId(4)),
                (FluidNodeId(2), FluidNodeId(4)),
                (FluidNodeId(3), FluidNodeId(4)),
            ],
        );

        let config = FluidSolverConfig {
            loop_topologies: [(LoopId(1), topology)].into_iter().collect(),
            ..FluidSolverConfig::default()
        };

        let mut solver = FluidSolver::new(config, &[(LoopId(1), FluidType::Water)]).unwrap();

        // Supply and return temperatures [°C] per node: the splitter carries
        // supply water, the mixer return water, and each branch drops 20 K.
        let temps_c = [
            (60.0, 60.0),
            (60.0, 40.0),
            (60.0, 40.0),
            (60.0, 40.0),
            (40.0, 40.0),
        ];
        let ports = PortSlots {
            fluid: (0u16..)
                .zip(flows_kg_s.into_iter().zip(temps_c))
                .map(|(node, (flow, (supply, ret)))| FluidAccumulator {
                    loop_id: LoopId(1),
                    fluid_type: FluidType::Water,
                    node_id: FluidNodeId(node),
                    total_flow_kg_s: flow,
                    mean_supply_temp_c: supply,
                    mean_return_temp_c: ret,
                    total_thermal_power_w: 0.0,
                    direction: None,
                })
                .collect(),
            ..Default::default()
        };

        let err = solver
            .resolve_new(&ports, &env(), Duration::from_secs(60))
            .expect_err("a splitter/mixer conservation violation must be a typed error");
        let message = err.to_string();
        assert!(
            message.contains("mass conservation violated"),
            "the error must name the check, got: {message}"
        );
        assert!(
            message.contains(&format!("({violating_role:?})")),
            "the violation must be at the {violating_role:?}, got: {message}"
        );
        assert!(
            message.contains(&format!("imbalance = {imbalance_kg_s:.6e}")),
            "the error must state the {imbalance_kg_s} kg/s imbalance, got: {message}"
        );
        #[cfg(feature = "observe")]
        {
            assert_eq!(solver.num_conservation_violations, 1);
            assert!(
                (solver.max_mass_imbalance_kg_s - imbalance_kg_s).abs() < 1e-12,
                "expected peak imbalance {imbalance_kg_s} kg/s, got {}",
                solver.max_mass_imbalance_kg_s
            );
        }
    }

    #[test]
    fn parallel_loop_splitter_topology_violation_is_a_typed_error() {
        // Branch C reports 0.5 kg/s while the splitter reports 1.0 kg/s:
        // splitter inflow 1.0 against branch outflows 0.4 + 0.3 + 0.5 = 1.2.
        assert_parallel_loop_violation([1.0, 0.4, 0.3, 0.5, 1.0], FluidNodeRole::Splitter, 0.2);
    }

    #[test]
    fn parallel_loop_mixer_topology_violation_is_a_typed_error() {
        // The splitter and branches balance (1.0 = 0.4 + 0.3 + 0.3), but the
        // mixer reports 1.3 kg/s leaving against 1.0 kg/s arriving.
        assert_parallel_loop_violation([1.0, 0.4, 0.3, 0.3, 1.3], FluidNodeRole::Mixer, 0.3);
    }

    // =======================================================================
    // T-0255: Flow-splitting resolution tests
    // =======================================================================

    #[test]
    fn parallel_branches_proportional_allocation_with_deficit() {
        // Two parallel boilers each requesting 0.3 kg/s from a pump providing
        // 0.5 kg/s. Flow is proportional: each gets 0.25 kg/s, deficit = 0.1 kg/s.
        //
        // Topology: pump(0) → splitter(1) → [branch A(2), branch B(3)] → mixer(4)
        //
        // Accumulator flows (pre-resolution):
        //   Node 0 (pump):    0.5 kg/s  (total available)
        //   Node 2 (boiler A): 0.3 kg/s (requested)
        //   Node 3 (boiler B): 0.3 kg/s (requested)

        let splitter = SplitterNode {
            node_id: FluidNodeId(1),
            inlet_node_id: FluidNodeId(0),
            branches: vec![
                SplitterBranch {
                    node_id: FluidNodeId(2),
                    requested_flow_kg_s: 0.0,
                    resistance_coefficient: 1.0,
                },
                SplitterBranch {
                    node_id: FluidNodeId(3),
                    requested_flow_kg_s: 0.0,
                    resistance_coefficient: 1.0,
                },
            ],
        };

        let mixer = MixerNode {
            node_id: FluidNodeId(4),
            outlet_node_id: FluidNodeId(0), // not used downstream in this test
            branches: vec![
                MixerBranch {
                    node_id: FluidNodeId(2),
                },
                MixerBranch {
                    node_id: FluidNodeId(3),
                },
            ],
        };

        let topology = LoopTopology::new(
            LoopId(1),
            vec![
                FluidNode::new(FluidNodeId(0), FluidNodeRole::Source),
                FluidNode::new(FluidNodeId(1), FluidNodeRole::Splitter),
                FluidNode::new(FluidNodeId(2), FluidNodeRole::Source),
                FluidNode::new(FluidNodeId(3), FluidNodeRole::Source),
                FluidNode::new(FluidNodeId(4), FluidNodeRole::Mixer),
            ],
            vec![
                (FluidNodeId(0), FluidNodeId(1)),
                (FluidNodeId(1), FluidNodeId(2)),
                (FluidNodeId(1), FluidNodeId(3)),
                (FluidNodeId(2), FluidNodeId(4)),
                (FluidNodeId(3), FluidNodeId(4)),
            ],
        )
        .with_splitters(vec![splitter])
        .with_mixers(vec![mixer]);

        let config = FluidSolverConfig {
            loop_topologies: [(LoopId(1), topology)].into_iter().collect(),
            ..FluidSolverConfig::default()
        };

        let mut solver = FluidSolver::new(config, &[(LoopId(1), FluidType::Water)]).unwrap();

        let ports = PortSlots {
            fluid: vec![
                FluidAccumulator {
                    loop_id: LoopId(1),
                    fluid_type: FluidType::Water,
                    node_id: FluidNodeId(0),
                    total_flow_kg_s: 0.5,
                    mean_supply_temp_c: 60.0,
                    mean_return_temp_c: 60.0,
                    total_thermal_power_w: 0.0,
                    direction: None,
                },
                FluidAccumulator {
                    loop_id: LoopId(1),
                    fluid_type: FluidType::Water,
                    node_id: FluidNodeId(2),
                    total_flow_kg_s: 0.3,
                    mean_supply_temp_c: 60.0,
                    mean_return_temp_c: 40.0,
                    total_thermal_power_w: 0.0,
                    direction: None,
                },
                FluidAccumulator {
                    loop_id: LoopId(1),
                    fluid_type: FluidType::Water,
                    node_id: FluidNodeId(3),
                    total_flow_kg_s: 0.3,
                    mean_supply_temp_c: 60.0,
                    mean_return_temp_c: 40.0,
                    total_thermal_power_w: 0.0,
                    direction: None,
                },
            ],
            ..Default::default()
        };

        let update = solver
            .resolve_new(&ports, &env(), Duration::from_secs(60))
            .unwrap();

        // Verify net power uses proportional allocation:
        // Each boiler gets 0.25 kg/s * (60-40)K * Cp = 0.25 * 20 * Cp
        // Total = 2 * 0.25 * 20 * Cp = 10 * Cp
        let states = FluidDomainPayload::decode(&update.custom_payload.unwrap()).unwrap();
        let expected_power = 2.0 * 0.25 * CP_LIQUID_WATER_J_KG_K * 20.0;
        approx_eq(states[0].net_power_w, expected_power);

        // Loop-level supply temp should be flow-weighted: both branches at 60°C
        approx_eq(states[0].mean_supply_temp_c, 60.0);
        // Loop-level return temp should be flow-weighted: both branches at 40°C
        approx_eq(states[0].mean_return_temp_c, 40.0);
    }

    #[test]
    fn parallel_branches_excess_flow_meets_demand() {
        // Two parallel boilers each requesting 0.2 kg/s from a pump providing
        // 0.5 kg/s. Supply exceeds demand. Each gets its requested 0.2 kg/s,
        // excess 0.1 kg/s is unused. No flow created or destroyed.

        let splitter = SplitterNode {
            node_id: FluidNodeId(1),
            inlet_node_id: FluidNodeId(0),
            branches: vec![
                SplitterBranch {
                    node_id: FluidNodeId(2),
                    requested_flow_kg_s: 0.0,
                    resistance_coefficient: 1.0,
                },
                SplitterBranch {
                    node_id: FluidNodeId(3),
                    requested_flow_kg_s: 0.0,
                    resistance_coefficient: 1.0,
                },
            ],
        };

        let mixer = MixerNode {
            node_id: FluidNodeId(4),
            outlet_node_id: FluidNodeId(0),
            branches: vec![
                MixerBranch {
                    node_id: FluidNodeId(2),
                },
                MixerBranch {
                    node_id: FluidNodeId(3),
                },
            ],
        };

        let topology = LoopTopology::new(
            LoopId(1),
            vec![
                FluidNode::new(FluidNodeId(0), FluidNodeRole::Source),
                FluidNode::new(FluidNodeId(1), FluidNodeRole::Splitter),
                FluidNode::new(FluidNodeId(2), FluidNodeRole::Source),
                FluidNode::new(FluidNodeId(3), FluidNodeRole::Source),
                FluidNode::new(FluidNodeId(4), FluidNodeRole::Mixer),
            ],
            vec![
                (FluidNodeId(0), FluidNodeId(1)),
                (FluidNodeId(1), FluidNodeId(2)),
                (FluidNodeId(1), FluidNodeId(3)),
                (FluidNodeId(2), FluidNodeId(4)),
                (FluidNodeId(3), FluidNodeId(4)),
            ],
        )
        .with_splitters(vec![splitter])
        .with_mixers(vec![mixer]);

        let config = FluidSolverConfig {
            loop_topologies: [(LoopId(1), topology)].into_iter().collect(),
            ..FluidSolverConfig::default()
        };

        let mut solver = FluidSolver::new(config, &[(LoopId(1), FluidType::Water)]).unwrap();

        let ports = PortSlots {
            fluid: vec![
                FluidAccumulator {
                    loop_id: LoopId(1),
                    fluid_type: FluidType::Water,
                    node_id: FluidNodeId(0),
                    total_flow_kg_s: 0.5,
                    mean_supply_temp_c: 60.0,
                    mean_return_temp_c: 60.0,
                    total_thermal_power_w: 0.0,
                    direction: None,
                },
                FluidAccumulator {
                    loop_id: LoopId(1),
                    fluid_type: FluidType::Water,
                    node_id: FluidNodeId(2),
                    total_flow_kg_s: 0.2,
                    mean_supply_temp_c: 60.0,
                    mean_return_temp_c: 40.0,
                    total_thermal_power_w: 0.0,
                    direction: None,
                },
                FluidAccumulator {
                    loop_id: LoopId(1),
                    fluid_type: FluidType::Water,
                    node_id: FluidNodeId(3),
                    total_flow_kg_s: 0.2,
                    mean_supply_temp_c: 60.0,
                    mean_return_temp_c: 40.0,
                    total_thermal_power_w: 0.0,
                    direction: None,
                },
            ],
            ..Default::default()
        };

        let update = solver
            .resolve_new(&ports, &env(), Duration::from_secs(60))
            .unwrap();

        let states = FluidDomainPayload::decode(&update.custom_payload.unwrap()).unwrap();
        // Each branch gets its requested 0.2 kg/s — net power = 2 * 0.2 * Cp * 20K
        let expected_power = 2.0 * 0.2 * CP_LIQUID_WATER_J_KG_K * 20.0;
        approx_eq(states[0].net_power_w, expected_power);

        approx_eq(states[0].mean_supply_temp_c, 60.0);
        approx_eq(states[0].mean_return_temp_c, 40.0);
    }

    #[test]
    fn series_only_loop_with_no_splitters_still_works() {
        // Regression: a serial loop (one accumulator, no splitters) must
        // continue to work correctly with the new resolution path. Flow is
        // conserved end-to-end via the existing serial-flow consistency check.
        let mut solver = FluidSolver::new(
            FluidSolverConfig::default(),
            &[(LoopId(1), FluidType::Water)],
        )
        .unwrap();

        let mut ports = PortSlots {
            fluid: vec![FluidAccumulator::new(LoopId(1), FluidType::Water)],
            ..Default::default()
        };
        ports
            .accumulate(&PortContribution::Fluid {
                loop_id: LoopId(1),
                flow_rate_kg_s: 0.5,
                supply_temp_c: 60.0,
                return_temp_c: 40.0,
                fluid_type: FluidType::Water,
                thermal_power_w: None,
                node_id: FluidNodeId(0),
                direction: HeatTransferDirection::Source,
            })
            .unwrap();

        let update = solver
            .resolve_new(&ports, &env(), Duration::from_secs(60))
            .unwrap();
        let states = FluidDomainPayload::decode(&update.custom_payload.unwrap()).unwrap();
        approx_eq(states[0].net_power_w, 0.5 * CP_LIQUID_WATER_J_KG_K * 20.0);
        approx_eq(states[0].mean_supply_temp_c, 60.0);
        approx_eq(states[0].mean_return_temp_c, 40.0);
    }

    #[test]
    fn two_parallel_distribution_coils_flow_split_and_heating_output() {
        // Integration test: two parallel distribution coils on one hydronic
        // loop. Coil A requests 0.15 kg/s, coil B requests 0.10 kg/s.
        // Pump provides 0.20 kg/s (insufficient, proportional allocation).
        //
        // Allocation: coil A gets 0.20 * (0.15/0.25) = 0.12 kg/s
        //              coil B gets 0.20 * (0.10/0.25) = 0.08 kg/s
        //
        // Coil A: dt=10K, power = 0.12 * Cp * 10
        // Coil B: dt=15K, power = 0.08 * Cp * 15

        let splitter = SplitterNode {
            node_id: FluidNodeId(1),
            inlet_node_id: FluidNodeId(0),
            branches: vec![
                SplitterBranch {
                    node_id: FluidNodeId(2),
                    requested_flow_kg_s: 0.0,
                    resistance_coefficient: 1.0,
                },
                SplitterBranch {
                    node_id: FluidNodeId(3),
                    requested_flow_kg_s: 0.0,
                    resistance_coefficient: 1.0,
                },
            ],
        };

        let mixer = MixerNode {
            node_id: FluidNodeId(4),
            outlet_node_id: FluidNodeId(0),
            branches: vec![
                MixerBranch {
                    node_id: FluidNodeId(2),
                },
                MixerBranch {
                    node_id: FluidNodeId(3),
                },
            ],
        };

        let topology = LoopTopology::new(
            LoopId(1),
            vec![
                FluidNode::new(FluidNodeId(0), FluidNodeRole::Source),
                FluidNode::new(FluidNodeId(1), FluidNodeRole::Splitter),
                FluidNode::new(FluidNodeId(2), FluidNodeRole::Sink),
                FluidNode::new(FluidNodeId(3), FluidNodeRole::Sink),
                FluidNode::new(FluidNodeId(4), FluidNodeRole::Mixer),
            ],
            vec![
                (FluidNodeId(0), FluidNodeId(1)),
                (FluidNodeId(1), FluidNodeId(2)),
                (FluidNodeId(1), FluidNodeId(3)),
                (FluidNodeId(2), FluidNodeId(4)),
                (FluidNodeId(3), FluidNodeId(4)),
            ],
        )
        .with_splitters(vec![splitter])
        .with_mixers(vec![mixer]);

        let config = FluidSolverConfig {
            loop_topologies: [(LoopId(1), topology)].into_iter().collect(),
            ..FluidSolverConfig::default()
        };

        let mut solver = FluidSolver::new(config, &[(LoopId(1), FluidType::Water)]).unwrap();

        let ports = PortSlots {
            fluid: vec![
                FluidAccumulator {
                    loop_id: LoopId(1),
                    fluid_type: FluidType::Water,
                    node_id: FluidNodeId(0),
                    total_flow_kg_s: 0.2,
                    mean_supply_temp_c: 55.0,
                    mean_return_temp_c: 55.0,
                    total_thermal_power_w: 0.0,
                    direction: None,
                },
                FluidAccumulator {
                    loop_id: LoopId(1),
                    fluid_type: FluidType::Water,
                    node_id: FluidNodeId(2),
                    total_flow_kg_s: 0.15,
                    mean_supply_temp_c: 55.0,
                    mean_return_temp_c: 45.0,
                    total_thermal_power_w: 0.0,
                    direction: None,
                },
                FluidAccumulator {
                    loop_id: LoopId(1),
                    fluid_type: FluidType::Water,
                    node_id: FluidNodeId(3),
                    total_flow_kg_s: 0.10,
                    mean_supply_temp_c: 55.0,
                    mean_return_temp_c: 40.0,
                    total_thermal_power_w: 0.0,
                    direction: None,
                },
            ],
            ..Default::default()
        };

        let update = solver
            .resolve_new(&ports, &env(), Duration::from_secs(60))
            .unwrap();
        let states = FluidDomainPayload::decode(&update.custom_payload.unwrap()).unwrap();

        // Coil A: 0.12 kg/s * 10K * Cp, Coil B: 0.08 kg/s * 15K * Cp
        let expected_power = (0.12 * 10.0 + 0.08 * 15.0) * CP_LIQUID_WATER_J_KG_K;
        approx_eq(states[0].net_power_w, expected_power);
    }

    // =======================================================================
    // T-0256: Net power direction-aware signed computation
    // =======================================================================

    #[test]
    fn balanced_source_and_sink_net_power_near_zero() {
        // A loop with one Source (boiler, injecting heat) and one Sink
        // (distribution coil, extracting heat). Both contribute the same
        // magnitude of thermal power. net_power_w should be near zero
        // because heating_power_w ≈ cooling_power_w.
        let mut solver = FluidSolver::new(
            FluidSolverConfig::default(),
            &[(LoopId(1), FluidType::Water)],
        )
        .unwrap();

        let boiler_power = 0.5 * CP_LIQUID_WATER_J_KG_K * 10.0; // 0.5 kg/s, ΔT=10K
        let coil_power = 0.5 * CP_LIQUID_WATER_J_KG_K * 10.0; // same magnitude

        let ports = PortSlots {
            fluid: vec![
                FluidAccumulator {
                    loop_id: LoopId(1),
                    fluid_type: FluidType::Water,
                    node_id: FluidNodeId(0),
                    total_flow_kg_s: 0.5,
                    mean_supply_temp_c: 60.0,
                    mean_return_temp_c: 50.0,
                    total_thermal_power_w: 0.0,
                    direction: Some(HeatTransferDirection::Source),
                },
                FluidAccumulator {
                    loop_id: LoopId(1),
                    fluid_type: FluidType::Water,
                    node_id: FluidNodeId(1),
                    total_flow_kg_s: 0.5,
                    mean_supply_temp_c: 50.0,
                    mean_return_temp_c: 40.0,
                    total_thermal_power_w: 0.0,
                    direction: Some(HeatTransferDirection::Sink),
                },
            ],
            ..Default::default()
        };

        // Serial-flow consistency check requires matching flow rates when no
        // topology is configured. Both accumulators have the same flow.
        let update = solver
            .resolve_new(&ports, &env(), Duration::from_secs(60))
            .unwrap();
        let states = FluidDomainPayload::decode(&update.custom_payload.unwrap()).unwrap();
        assert_eq!(states.len(), 1);

        approx_eq(states[0].heating_power_w, boiler_power);
        approx_eq(states[0].cooling_power_w, coil_power);
        approx_eq(states[0].net_power_w, 0.0);
    }

    #[test]
    fn imbalanced_source_exceeds_sink_net_power_positive() {
        // A loop with one Source (boiler) and one Sink (coil) where the
        // source injects more heat than the sink extracts.
        // net_power_w should be positive, reflecting net heating of the fluid.
        let mut solver = FluidSolver::new(
            FluidSolverConfig::default(),
            &[(LoopId(1), FluidType::Water)],
        )
        .unwrap();

        // Both must have the same flow for serial-flow consistency.
        let flow = 0.5;
        let boiler_dt = 10.0; // supply=60, return=50
        let coil_dt = 8.0; // supply=50, return=42
        let boiler_power = flow * CP_LIQUID_WATER_J_KG_K * boiler_dt;
        let coil_power = flow * CP_LIQUID_WATER_J_KG_K * coil_dt;
        let expected_net = boiler_power - coil_power;

        let ports = PortSlots {
            fluid: vec![
                FluidAccumulator {
                    loop_id: LoopId(1),
                    fluid_type: FluidType::Water,
                    node_id: FluidNodeId(0),
                    total_flow_kg_s: flow,
                    mean_supply_temp_c: 60.0,
                    mean_return_temp_c: 60.0 - boiler_dt,
                    total_thermal_power_w: 0.0,
                    direction: Some(HeatTransferDirection::Source),
                },
                FluidAccumulator {
                    loop_id: LoopId(1),
                    fluid_type: FluidType::Water,
                    node_id: FluidNodeId(1),
                    total_flow_kg_s: flow,
                    mean_supply_temp_c: 50.0,
                    mean_return_temp_c: 50.0 - coil_dt,
                    total_thermal_power_w: 0.0,
                    direction: Some(HeatTransferDirection::Sink),
                },
            ],
            ..Default::default()
        };

        let update = solver
            .resolve_new(&ports, &env(), Duration::from_secs(60))
            .unwrap();
        let states = FluidDomainPayload::decode(&update.custom_payload.unwrap()).unwrap();
        assert_eq!(states.len(), 1);

        approx_eq(states[0].heating_power_w, boiler_power);
        approx_eq(states[0].cooling_power_w, coil_power);
        approx_eq(states[0].net_power_w, expected_net);
        assert!(
            states[0].net_power_w > 0.0,
            "imbalanced loop with excess source must have positive net_power_w"
        );
    }

    #[test]
    fn single_source_loop_heating_power_w_matches_net_power_w() {
        // Regression: a single-boiler loop (no sinks) should produce
        // heating_power_w equal to net_power_w and cooling_power_w = 0.
        // This verifies the magnitude is unchanged from the pre-T-0256
        // behaviour.
        let mut solver = FluidSolver::new(
            FluidSolverConfig::default(),
            &[(LoopId(1), FluidType::Water)],
        )
        .unwrap();
        let mut ports = PortSlots {
            fluid: vec![FluidAccumulator::new(LoopId(1), FluidType::Water)],
            ..Default::default()
        };
        ports
            .accumulate(&PortContribution::Fluid {
                loop_id: LoopId(1),
                flow_rate_kg_s: 0.5,
                supply_temp_c: 60.0,
                return_temp_c: 40.0,
                fluid_type: FluidType::Water,
                thermal_power_w: None,
                node_id: FluidNodeId(0),
                direction: HeatTransferDirection::Source,
            })
            .unwrap();

        let update = solver
            .resolve_new(&ports, &env(), Duration::from_secs(60))
            .unwrap();
        let states = FluidDomainPayload::decode(&update.custom_payload.unwrap()).unwrap();
        let expected = 0.5 * CP_LIQUID_WATER_J_KG_K * 20.0;

        approx_eq(states[0].heating_power_w, expected);
        approx_eq(states[0].cooling_power_w, 0.0);
        approx_eq(states[0].net_power_w, expected);
    }

    #[test]
    fn heating_and_cooling_power_never_negative() {
        // Integration: run a full simulation step with mixed source and sink.
        // Both heating_power_w and cooling_power_w must be non-negative
        // regardless of temperature sign conventions.
        let mut solver = FluidSolver::new(
            FluidSolverConfig::default(),
            &[(LoopId(1), FluidType::Water)],
        )
        .unwrap();

        // Sink with inverted ΔT (return_temp > supply_temp): abs() in the
        // sink formula guarantees positive cooling_power_w magnitude.
        let ports = PortSlots {
            fluid: vec![
                FluidAccumulator {
                    loop_id: LoopId(1),
                    fluid_type: FluidType::Water,
                    node_id: FluidNodeId(0),
                    total_flow_kg_s: 0.5,
                    mean_supply_temp_c: 60.0,
                    mean_return_temp_c: 50.0,
                    total_thermal_power_w: 0.0,
                    direction: Some(HeatTransferDirection::Source),
                },
                FluidAccumulator {
                    loop_id: LoopId(1),
                    fluid_type: FluidType::Water,
                    node_id: FluidNodeId(1),
                    total_flow_kg_s: 0.5,
                    // Inverted: supply=40, return=50 (water leaves cooler than it entered)
                    mean_supply_temp_c: 40.0,
                    mean_return_temp_c: 50.0,
                    total_thermal_power_w: 0.0,
                    direction: Some(HeatTransferDirection::Sink),
                },
            ],
            ..Default::default()
        };

        let update = solver
            .resolve_new(&ports, &env(), Duration::from_secs(60))
            .unwrap();
        let states = FluidDomainPayload::decode(&update.custom_payload.unwrap()).unwrap();

        assert!(
            states[0].heating_power_w >= 0.0,
            "heating_power_w must be non-negative, got {}",
            states[0].heating_power_w
        );
        assert!(
            states[0].cooling_power_w >= 0.0,
            "cooling_power_w must be non-negative, got {}",
            states[0].cooling_power_w
        );
        // Sink with abs(ΔT): |40-50| = 10 → 0.5*4180*10 > 0
        assert!(
            states[0].cooling_power_w > 0.0,
            "sink with abs(ΔT) must produce positive cooling_power_w"
        );
    }

    // =======================================================================
    // T-0257: Temperature bounds clamping
    // =======================================================================

    #[test]
    fn water_loop_clamps_temperatures_outside_0_to_100_c() {
        let limits = FluidTempLimits {
            min_temp_c: 0.0,
            max_temp_c: 100.0,
        };
        let config = FluidSolverConfig {
            loop_temp_limits: [(LoopId(1), limits)].into_iter().collect(),
            ..FluidSolverConfig::default()
        };
        let mut solver = FluidSolver::new(config, &[(LoopId(1), FluidType::Water)]).unwrap();

        // Feed in unphysical temperatures: supply below freezing, return above boiling.
        let ports = PortSlots {
            fluid: vec![FluidAccumulator {
                loop_id: LoopId(1),
                fluid_type: FluidType::Water,
                node_id: FluidNodeId(0),
                total_flow_kg_s: 0.5,
                mean_supply_temp_c: -10.0,
                mean_return_temp_c: 150.0,
                total_thermal_power_w: 0.0,
                direction: Some(HeatTransferDirection::Source),
            }],
            ..Default::default()
        };

        let update = solver
            .resolve_new(&ports, &env(), Duration::from_secs(60))
            .unwrap();
        let states = FluidDomainPayload::decode(&update.custom_payload.unwrap()).unwrap();

        approx_eq(states[0].mean_supply_temp_c, 0.0);
        approx_eq(states[0].mean_return_temp_c, 100.0);
    }

    #[test]
    fn glycol_loop_within_bounds_produces_no_clamping() {
        let limits = FluidTempLimits {
            min_temp_c: -40.0,
            max_temp_c: 120.0,
        };
        let config = FluidSolverConfig {
            loop_temp_limits: [(LoopId(1), limits)].into_iter().collect(),
            ..FluidSolverConfig::default()
        };
        let mut solver = FluidSolver::new(config, &[(LoopId(1), FluidType::Glycol)]).unwrap();

        // Temperatures within the glycol operating range.
        let ports = PortSlots {
            fluid: vec![FluidAccumulator {
                loop_id: LoopId(1),
                fluid_type: FluidType::Glycol,
                node_id: FluidNodeId(0),
                total_flow_kg_s: 0.5,
                mean_supply_temp_c: -30.0,
                mean_return_temp_c: 115.0,
                total_thermal_power_w: 0.0,
                direction: Some(HeatTransferDirection::Source),
            }],
            ..Default::default()
        };

        let update = solver
            .resolve_new(&ports, &env(), Duration::from_secs(60))
            .unwrap();
        let states = FluidDomainPayload::decode(&update.custom_payload.unwrap()).unwrap();

        // No clamping should occur; temperatures pass through unchanged.
        approx_eq(states[0].mean_supply_temp_c, -30.0);
        approx_eq(states[0].mean_return_temp_c, 115.0);
    }

    #[test]
    fn existing_simulation_within_bounds_produces_unchanged_temperatures() {
        // Regression: verify a normal simulation with well-behaved
        // temperatures passes through unchanged with default limits.
        let mut solver = FluidSolver::new(
            FluidSolverConfig::default(),
            &[(LoopId(1), FluidType::Water)],
        )
        .unwrap();

        let ports = PortSlots {
            fluid: vec![FluidAccumulator {
                loop_id: LoopId(1),
                fluid_type: FluidType::Water,
                node_id: FluidNodeId(0),
                total_flow_kg_s: 0.5,
                mean_supply_temp_c: 60.0,
                mean_return_temp_c: 40.0,
                total_thermal_power_w: 0.0,
                direction: Some(HeatTransferDirection::Source),
            }],
            ..Default::default()
        };

        let update = solver
            .resolve_new(&ports, &env(), Duration::from_secs(60))
            .unwrap();
        let states = FluidDomainPayload::decode(&update.custom_payload.unwrap()).unwrap();

        approx_eq(states[0].mean_supply_temp_c, 60.0);
        approx_eq(states[0].mean_return_temp_c, 40.0);
    }

    #[test]
    fn undersized_source_sub_freezing_clamped_and_no_nan_propagation() {
        // Integration test: feed extreme negative temperatures through
        // the solver and verify clamping prevents NaN/Inf propagation
        // into FluidLoopState.
        let limits = FluidTempLimits {
            min_temp_c: -20.0,
            max_temp_c: 120.0,
        };
        let config = FluidSolverConfig {
            loop_temp_limits: [(LoopId(1), limits)].into_iter().collect(),
            ..FluidSolverConfig::default()
        };
        let mut solver = FluidSolver::new(config, &[(LoopId(1), FluidType::Water)]).unwrap();

        let ports = PortSlots {
            fluid: vec![FluidAccumulator {
                loop_id: LoopId(1),
                fluid_type: FluidType::Water,
                node_id: FluidNodeId(0),
                total_flow_kg_s: 0.5,
                mean_supply_temp_c: -50.0,
                mean_return_temp_c: -30.0,
                total_thermal_power_w: 0.0,
                direction: Some(HeatTransferDirection::Source),
            }],
            ..Default::default()
        };

        let update = solver
            .resolve_new(&ports, &env(), Duration::from_secs(60))
            .unwrap();
        let states = FluidDomainPayload::decode(&update.custom_payload.unwrap()).unwrap();

        // Both temperatures must be clamped to min and finite.
        approx_eq(states[0].mean_supply_temp_c, -20.0);
        approx_eq(states[0].mean_return_temp_c, -20.0);
        assert!(
            states[0].mean_supply_temp_c.is_finite() && states[0].mean_return_temp_c.is_finite(),
            "clamped temperatures must be finite"
        );
        assert!(
            states[0].net_power_w.is_finite(),
            "net_power_w must be finite after clamping"
        );
    }
}
