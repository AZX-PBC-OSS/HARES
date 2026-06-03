//! SolverFeedbackActor: bridges thermal solver to IdealHvac equipment.
//!
//! Collects ideal capacity targets from equipment (via `ideal_target()`),
//! calls the solver to back-compute the required capacity, and dispatches
//! `IdealCapacity` signals through the standard Actor::decide() interface.
//!
//! Uses equipment indices internally and pre-cached `DispatchTarget`s
//! (set once via `set_dispatch_targets()`) to minimize per-step work.
//! `decide()` clones cached `DispatchTarget`s (cheap Arc refcount bump).

use hares_control::{DispatchRequest, DispatchTarget, PriorityTier};
use hares_envelope::ThermalSolver;
use hares_equipment::Equipment;
use hares_types::{ControlSignal, EnvironmentState};

use crate::Actor;

/// Actor that bridges thermal solver back-solve to IdealHvac equipment.
///
/// Stored as a typed field on `Dwelling` (not in the generic `actors` Vec)
/// so the dwelling can call `collect_and_solve()` with typed access before
/// the actor decision phase.
pub struct SolverFeedbackActor {
    /// Pre-allocated buffer for pending ideal capacity dispatches.
    /// Each entry is (equipment_index, capacity_w, degraded).
    pending: Vec<(usize, f64, bool)>,
    /// Pre-cached dispatch targets indexed by equipment position.
    /// Set once at init via `set_dispatch_targets()`, avoids per-step name cloning.
    dispatch_targets: Vec<DispatchTarget>,
}

impl SolverFeedbackActor {
    #[must_use]
    pub fn new() -> Self {
        Self {
            pending: Vec::with_capacity(4),
            dispatch_targets: Vec::new(),
        }
    }

    /// Set pre-cached dispatch targets indexed by equipment position.
    /// Called by the dwelling when equipment changes.
    pub fn set_dispatch_targets(&mut self, targets: Vec<DispatchTarget>) {
        self.dispatch_targets = targets;
    }

    /// Collect ideal targets from equipment and solve for capacities.
    ///
    /// For each equipment with `ideal_target() -> Some((zone, target_c))`,
    /// calls the solver to compute the required capacity and queues it
    /// for dispatch. After solving, checks whether the solver declared
    /// the capacity as degraded (last-good fallback) and records the flag.
    pub fn collect_and_solve(
        &mut self,
        equipment: &[Box<dyn Equipment>],
        solver: &mut ThermalSolver,
    ) {
        self.pending.clear();
        for (idx, eq) in equipment.iter().enumerate() {
            if let Some((zone, target_c)) = eq.ideal_target() {
                let capacity_w = solver.solve_ideal_capacity_for_target(zone, target_c);
                let degraded = solver.zone_capacity_degraded(zone);
                self.pending.push((idx, capacity_w, degraded));
            }
        }
    }

    /// Test-friendly variant: collect ideal targets and resolve capacities via closure.
    #[cfg(test)]
    pub(crate) fn collect_and_solve_test(
        &mut self,
        equipment: &[Box<dyn Equipment>],
        solve: impl FnMut(hares_types::ZoneId, f64) -> f64,
    ) {
        self.collect_with(equipment, solve);
    }

    /// Test-friendly: directly push a capacity value with its degradation status
    /// into the pending buffer. Used to test degraded-capacity signal propagation
    /// without a real ThermalSolver.
    #[cfg(test)]
    pub(crate) fn push_pending_test(&mut self, equipment_index: usize, capacity_w: f64, degraded: bool) {
        self.pending.push((equipment_index, capacity_w, degraded));
    }

    #[cfg(test)]
    fn collect_with(
        &mut self,
        equipment: &[Box<dyn Equipment>],
        mut solve: impl FnMut(hares_types::ZoneId, f64) -> f64,
    ) {
        self.pending.clear();
        for (idx, eq) in equipment.iter().enumerate() {
            if let Some((zone, target_c)) = eq.ideal_target() {
                let capacity_w = solve(zone, target_c);
                self.pending.push((idx, capacity_w, false));
            }
        }
    }
}

impl Default for SolverFeedbackActor {
    fn default() -> Self {
        Self::new()
    }
}

impl Actor for SolverFeedbackActor {
    fn name(&self) -> &str {
        "SolverFeedbackActor"
    }

    fn decide(&mut self, _env: &EnvironmentState, out: &mut Vec<DispatchRequest>) {
        for (idx, capacity_w, degraded) in self.pending.drain(..) {
            let Some(target) = self.dispatch_targets.get(idx) else {
                tracing::warn!(
                    idx,
                    len = self.dispatch_targets.len(),
                    "dispatch_targets not synced with equipment"
                );
                continue;
            };
            // `Schedule` tier — matches the central `From<&ControlSignal>
            // for PriorityTier` mapping. Solver feedback operates at the
            // normal schedule level; user overrides and grid DR signals
            // take precedence over solver-computed ideal capacity.
            out.push(DispatchRequest {
                target: target.clone(),
                signal: ControlSignal::IdealCapacity { capacity_w, degraded },
                priority: PriorityTier::Schedule,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use hares_control::PriorityTier;
    use hares_types::ControlSignal;

    use super::*;

    fn mock_targets() -> Vec<DispatchTarget> {
        vec![
            DispatchTarget::ByName(Arc::from("HVAC_0")),
            DispatchTarget::ByName(Arc::from("HVAC_1")),
        ]
    }

    #[test]
    fn name_returns_expected_value() {
        let actor = SolverFeedbackActor::new();
        assert_eq!(actor.name(), "SolverFeedbackActor");
    }

    #[test]
    fn decide_produces_correct_dispatch_request() {
        let mut actor = SolverFeedbackActor::new();
        actor.set_dispatch_targets(mock_targets());
        actor.pending.push((0, 5000.0, false));

        let env = crate::actor::testing::test_env().build();
        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].target, DispatchTarget::ByName(Arc::from("HVAC_0")));
        assert_eq!(out[0].priority, PriorityTier::Schedule);
        match &out[0].signal {
            ControlSignal::IdealCapacity { capacity_w, degraded } => {
                assert!((*capacity_w - 5000.0).abs() < 1e-9);
                assert!(!degraded);
            }
            _ => panic!("expected IdealCapacity signal"),
        }
    }

    #[test]
    fn decide_drains_pending_buffer() {
        let mut actor = SolverFeedbackActor::new();
        actor.set_dispatch_targets(mock_targets());
        actor.pending.push((0, 5000.0, false));
        actor.pending.push((1, -3000.0, false));

        let env = crate::actor::testing::test_env().build();
        let mut out = Vec::new();
        actor.decide(&env, &mut out);
        assert_eq!(out.len(), 2);

        out.clear();
        actor.decide(&env, &mut out);
        assert!(out.is_empty(), "second decide should produce nothing");
    }

    #[test]
    fn decide_with_no_pending_produces_nothing() {
        let mut actor = SolverFeedbackActor::new();
        actor.set_dispatch_targets(mock_targets());

        let env = crate::actor::testing::test_env().build();
        let mut out = Vec::new();
        actor.decide(&env, &mut out);
        assert!(out.is_empty());
    }
}
