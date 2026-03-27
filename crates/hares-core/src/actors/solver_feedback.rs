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
use hares_types::{ControlSignal, EnvironmentState, ZoneId};

use crate::Actor;

/// Actor that bridges thermal solver back-solve to IdealHvac equipment.
///
/// Stored as a typed field on `Dwelling` (not in the generic `actors` Vec)
/// so the dwelling can call `collect_and_solve()` with typed access before
/// the actor decision phase.
pub struct SolverFeedbackActor {
    /// Pre-allocated buffer for pending ideal capacity dispatches.
    /// Each entry is (equipment_index, capacity_w).
    pending: Vec<(usize, f64)>,
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
    /// for dispatch. Uses equipment indices (zero allocation).
    pub fn collect_and_solve(&mut self, equipment: &[Box<dyn Equipment>], solver: &ThermalSolver) {
        self.collect_with(equipment, |zone, target_c| {
            solver.solve_ideal_capacity_for_target(zone, target_c)
        });
    }

    /// Test-friendly variant: collect ideal targets and resolve capacities via closure.
    pub fn collect_and_solve_test(
        &mut self,
        equipment: &[Box<dyn Equipment>],
        solve: impl Fn(ZoneId, f64) -> f64,
    ) {
        self.collect_with(equipment, solve);
    }

    fn collect_with(
        &mut self,
        equipment: &[Box<dyn Equipment>],
        solve: impl Fn(ZoneId, f64) -> f64,
    ) {
        self.pending.clear();
        for (idx, eq) in equipment.iter().enumerate() {
            if let Some((zone, target_c)) = eq.ideal_target() {
                let capacity_w = solve(zone, target_c);
                self.pending.push((idx, capacity_w));
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
        for (idx, capacity_w) in self.pending.drain(..) {
            let Some(target) = self.dispatch_targets.get(idx) else {
                tracing::warn!(
                    idx,
                    len = self.dispatch_targets.len(),
                    "dispatch_targets not synced with equipment"
                );
                continue;
            };
            out.push(DispatchRequest {
                target: target.clone(),
                signal: ControlSignal::IdealCapacity { capacity_w },
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
        actor.pending.push((0, 5000.0));

        let env = crate::actor::testing::test_env().build();
        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].target, DispatchTarget::ByName(Arc::from("HVAC_0")));
        assert_eq!(out[0].priority, PriorityTier::Schedule);
        match &out[0].signal {
            ControlSignal::IdealCapacity { capacity_w } => {
                assert!((*capacity_w - 5000.0).abs() < 1e-9);
            }
            _ => panic!("expected IdealCapacity signal"),
        }
    }

    #[test]
    fn decide_drains_pending_buffer() {
        let mut actor = SolverFeedbackActor::new();
        actor.set_dispatch_targets(mock_targets());
        actor.pending.push((0, 5000.0));
        actor.pending.push((1, -3000.0));

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
