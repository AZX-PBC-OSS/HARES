//! Event scheduler for time-triggered actions.
//!
//! Actors register for named execution phases with an optional priority within
//! each phase. The scheduler builds a deterministic, sorted plan each time
//! registrations change, and the [`Dwelling`] drives actor execution from that
//! plan at each timestep.
//!
//! # Phases (execution order)
//!
//! | Phase | Purpose |
//! |-------|---------|
//! | [`ExecutionPhase::SolverFeedback`] | Solver feedback actor runs `collect_and_solve` followed by `decide`. |
//! | [`ExecutionPhase::ActorDecide`] | All registered actors observe environment and emit control signals. |

/// Named execution phases within a timestep.
///
/// Phases execute in ordinal order (lower → higher). Within a phase, actors
/// execute in `(priority, registration_index)` order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExecutionPhase {
    /// Solver feedback: collect ideal HVAC targets from thermal equipment,
    /// solve for capacities, and emit setpoint signals via `decide`.
    SolverFeedback = 0,
    /// Actor decision: each registered actor observes environment state
    /// and emits control signals through the dispatch buffer.
    ActorDecide = 1,
}

impl ExecutionPhase {
    /// Number of distinct execution phases.
    pub const VARIANT_COUNT: usize = 2;

    /// All phases in guaranteed execution order.
    pub const fn all() -> [ExecutionPhase; Self::VARIANT_COUNT] {
        [ExecutionPhase::SolverFeedback, ExecutionPhase::ActorDecide]
    }

    /// Ordinal for sorting and range checks (lower = earlier execution).
    #[must_use]
    pub const fn ordinal(self) -> u8 {
        self as u8
    }
}

/// Index into a Dwelling's actor vector identifying a specific actor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActorSlot(pub usize);

/// A binding of an actor to a phase with a priority for within-phase ordering.
#[derive(Clone, Debug)]
pub struct Registration {
    /// Which phase this actor participates in.
    pub phase: ExecutionPhase,
    /// Index into the Dwelling's actor vector.
    pub slot: ActorSlot,
    /// Within-phase ordering: lower values execute first.
    /// Ties are broken by registration order (earlier registration first).
    pub priority: i32,
    /// Actor name for diagnostics and observability.
    pub name: String,
}

/// One entry in the per-timestep execution plan.
///
/// [`ExecutionPhase::SolverFeedback`] entries have `slot: None` — they
/// represent the solver feedback actor, which is stored separately from the
/// actor vector. [`ExecutionPhase::ActorDecide`] entries carry the slot
/// identifying which actor to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanEntry {
    pub phase: ExecutionPhase,
    pub slot: Option<ActorSlot>,
    pub name: String,
}

/// Scheduler that builds deterministic per-timestep execution plans from actor
/// phase registrations.
///
/// # Design
///
/// 1. Actors register with a phase and a priority.
/// 2. Calling [`build`] produces a sorted [`Vec<PlanEntry>`] ordered by
///    `(phase.ordinal, priority, registration_index)`.
/// 3. The plan is consumed once per timestep and rebuilt when registrations
///    change (actor added/removed).
///
/// # Example
///
/// ```rust
/// use hares_core::scheduler::{ExecutionPhase, ActorSlot, StepScheduler};
///
/// let mut s = StepScheduler::default();
/// s.register_actor(ActorSlot(0), ExecutionPhase::ActorDecide, 10, "dr_compliance");
/// s.register_actor(ActorSlot(1), ExecutionPhase::ActorDecide, 0, "battery_bms");
/// s.register_solver_feedback();
/// let plan = s.build();
///
/// // SolverFeedback runs first (ordinal 0).
/// // Then ActorDecide: battery_bms (priority 0), dr_compliance (priority 10).
/// assert_eq!(plan[0].phase, ExecutionPhase::SolverFeedback);
/// assert_eq!(plan[1].name, "battery_bms");
/// assert_eq!(plan[2].name, "dr_compliance");
/// ```
#[derive(Clone, Debug, Default)]
pub struct StepScheduler {
    registrations: Vec<Registration>,
    solver_feedback_registered: bool,
    /// Built plan; cleared when registrations change.
    plan: Vec<PlanEntry>,
    /// Whether registrations have changed since last `build`.
    dirty: bool,
}

impl StepScheduler {
    /// Registers an actor for a specific execution phase.
    ///
    /// Actors execute in `(phase.ordinal, priority, registration_index)` order.
    /// Multiple registrations of the same actor (same slot) in the same phase
    /// are allowed — each is a separate entry.
    pub fn register_actor(
        &mut self,
        slot: ActorSlot,
        phase: ExecutionPhase,
        priority: i32,
        name: &str,
    ) {
        self.registrations.push(Registration {
            phase,
            slot,
            priority,
            name: name.to_string(),
        });
        self.dirty = true;
    }

    /// Marks the solver feedback actor as participating in
    /// [`ExecutionPhase::SolverFeedback`]. Must be called exactly once before
    /// `build` if the solver feedback actor is in use.
    pub fn register_solver_feedback(&mut self) {
        self.solver_feedback_registered = true;
        self.dirty = true;
    }

    /// Registers multiple actors by iterating a collection of (slot, name)
    /// pairs, binding each to the given phase with the specified priority.
    pub fn register_actors_in_phase<'a, I>(
        &mut self,
        slots: I,
        phase: ExecutionPhase,
        priority: i32,
    ) where
        I: IntoIterator<Item = (ActorSlot, &'a str)>,
    {
        for (slot, name) in slots {
            self.register_actor(slot, phase, priority, name);
        }
    }

    /// Returns the number of registrations (excluding solver feedback marker).
    #[must_use]
    pub fn registration_count(&self) -> usize {
        self.registrations.len()
    }

    /// Returns true if no actor registrations exist and solver feedback is not
    /// registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.registrations.is_empty() && !self.solver_feedback_registered
    }

    /// Removes all registrations and the solver feedback marker.
    pub fn clear(&mut self) {
        self.registrations.clear();
        self.solver_feedback_registered = false;
        self.plan.clear();
        self.dirty = false;
    }

    /// Builds (or returns cached) the execution plan sorted by
    /// `(phase.ordinal, priority, registration_index)`.
    ///
    /// Solver feedback entries (if registered) appear first. Actor entries
    /// follow, grouped by phase and ordered within each phase.
    #[must_use]
    pub fn build(&mut self) -> &[PlanEntry] {
        if !self.dirty {
            return &self.plan;
        }
        self.plan.clear();

        if self.solver_feedback_registered {
            self.plan.push(PlanEntry {
                phase: ExecutionPhase::SolverFeedback,
                slot: None,
                name: "solver_feedback".to_string(),
            });
        }

        // Sort registrations by (phase.ordinal, priority, index_in_vec).
        // Using stable sort on (phase, priority) preserves registration order
        // for ties (Vec sort_by is stable).
        let mut sorted: Vec<(usize, &Registration)> =
            self.registrations.iter().enumerate().collect();
        sorted.sort_by(|(ai, a), (bi, b)| {
            a.phase
                .ordinal()
                .cmp(&b.phase.ordinal())
                .then_with(|| a.priority.cmp(&b.priority))
                .then_with(|| ai.cmp(bi))
        });

        for (_idx, reg) in sorted {
            self.plan.push(PlanEntry {
                phase: reg.phase,
                slot: Some(reg.slot),
                name: reg.name.clone(),
            });
        }

        self.dirty = false;
        &self.plan
    }

    /// Returns a reference to the currently cached plan, or `None` if no plan
    /// has been built.
    #[must_use]
    pub fn plan(&self) -> &[PlanEntry] {
        &self.plan
    }

    /// Returns an iterator over the plan entries for the specified phase.
    /// Panics in debug if the plan has not been built.
    pub fn iter_phase(&self, phase: ExecutionPhase) -> impl Iterator<Item = &PlanEntry> {
        debug_assert!(
            !self.dirty,
            "iter_phase called before plan was built; call build() first"
        );
        self.plan.iter().filter(move |e| e.phase == phase)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_scheduler_builds_empty_plan() {
        let mut s = StepScheduler::default();
        let plan = s.build();
        assert!(plan.is_empty());
    }

    #[test]
    fn solver_feedback_only_produces_one_entry() {
        let mut s = StepScheduler::default();
        s.register_solver_feedback();
        let plan = s.build();
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].phase, ExecutionPhase::SolverFeedback);
        assert!(plan[0].slot.is_none());
    }

    #[test]
    fn single_actor_produces_correct_entry() {
        let mut s = StepScheduler::default();
        s.register_actor(
            ActorSlot(0),
            ExecutionPhase::ActorDecide,
            0,
            "dr_compliance",
        );
        let plan = s.build();
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].phase, ExecutionPhase::ActorDecide);
        assert_eq!(plan[0].slot, Some(ActorSlot(0)));
        assert_eq!(plan[0].name, "dr_compliance");
    }

    #[test]
    fn phases_execute_in_ordinal_order() {
        let mut s = StepScheduler::default();
        s.register_solver_feedback();
        s.register_actor(ActorSlot(0), ExecutionPhase::ActorDecide, 0, "bms");
        s.register_actor(ActorSlot(1), ExecutionPhase::ActorDecide, 0, "ev");
        let plan = s.build();

        assert_eq!(plan.len(), 3);
        // Solver feedback (ordinal 0) before ActorDecide (ordinal 1).
        assert_eq!(plan[0].phase, ExecutionPhase::SolverFeedback);
        assert_eq!(plan[1].phase, ExecutionPhase::ActorDecide);
        assert_eq!(plan[2].phase, ExecutionPhase::ActorDecide);
    }

    #[test]
    fn within_phase_ordered_by_priority() {
        let mut s = StepScheduler::default();
        s.register_actor(ActorSlot(0), ExecutionPhase::ActorDecide, 20, "last");
        s.register_actor(ActorSlot(1), ExecutionPhase::ActorDecide, 0, "first");
        s.register_actor(ActorSlot(2), ExecutionPhase::ActorDecide, 10, "middle");
        let plan = s.build();

        assert_eq!(plan[0].name, "first"); // priority 0
        assert_eq!(plan[1].name, "middle"); // priority 10
        assert_eq!(plan[2].name, "last"); // priority 20
    }

    #[test]
    fn priority_ties_resolved_by_registration_order() {
        let mut s = StepScheduler::default();
        // Same priority 0, registration order determines execution order.
        s.register_actor(ActorSlot(0), ExecutionPhase::ActorDecide, 0, "alpha");
        s.register_actor(ActorSlot(1), ExecutionPhase::ActorDecide, 0, "beta");
        s.register_actor(ActorSlot(2), ExecutionPhase::ActorDecide, 0, "gamma");
        let plan = s.build();

        assert_eq!(plan[0].name, "alpha");
        assert_eq!(plan[1].name, "beta");
        assert_eq!(plan[2].name, "gamma");
    }

    #[test]
    fn mixed_phase_with_priorities_orders_correctly() {
        let mut s = StepScheduler::default();
        s.register_solver_feedback();
        s.register_actor(ActorSlot(0), ExecutionPhase::ActorDecide, 5, "actor_a");
        s.register_actor(ActorSlot(1), ExecutionPhase::ActorDecide, 0, "actor_b");

        let plan = s.build();
        assert_eq!(plan.len(), 3);
        assert_eq!(plan[0].phase, ExecutionPhase::SolverFeedback); // ordinal 0
        assert_eq!(plan[1].name, "actor_b"); // ActorDecide, priority 0
        assert_eq!(plan[2].name, "actor_a"); // ActorDecide, priority 5
    }

    #[test]
    fn clear_resets_all_state() {
        let mut s = StepScheduler::default();
        s.register_solver_feedback();
        s.register_actor(ActorSlot(0), ExecutionPhase::ActorDecide, 0, "actor");
        let _ = s.build();

        s.clear();
        assert!(s.plan().is_empty());
        assert!(s.is_empty());
        assert_eq!(s.registration_count(), 0);
    }

    #[test]
    fn plan_is_cached_when_not_dirty() {
        let mut s = StepScheduler::default();
        s.register_actor(ActorSlot(0), ExecutionPhase::ActorDecide, 0, "actor");

        let plan1 = s.build() as *const [PlanEntry];
        let plan2 = s.build() as *const [PlanEntry];
        // Same pointer = plan was cached, not rebuilt.
        assert_eq!(plan1, plan2);
    }

    #[test]
    fn plan_rebuilt_when_registration_added() {
        let mut s = StepScheduler::default();
        s.register_actor(ActorSlot(0), ExecutionPhase::ActorDecide, 0, "alpha");
        let plan1 = s.build() as *const [PlanEntry];

        s.register_actor(ActorSlot(1), ExecutionPhase::ActorDecide, 0, "beta");
        let plan2 = s.build() as *const [PlanEntry];
        // Different pointer = plan was rebuilt.
        assert_ne!(plan1, plan2);
    }

    #[test]
    fn iter_phase_returns_only_entries_for_that_phase() {
        let mut s = StepScheduler::default();
        s.register_solver_feedback();
        s.register_actor(ActorSlot(0), ExecutionPhase::ActorDecide, 0, "actor");
        let _ = s.build();

        let sf: Vec<_> = s.iter_phase(ExecutionPhase::SolverFeedback).collect();
        assert_eq!(sf.len(), 1);
        assert!(sf[0].slot.is_none());

        let ad: Vec<_> = s.iter_phase(ExecutionPhase::ActorDecide).collect();
        assert_eq!(ad.len(), 1);
        assert_eq!(ad[0].slot, Some(ActorSlot(0)));
    }

    #[test]
    fn register_actors_in_phase_bulk_registers() {
        let mut s = StepScheduler::default();
        let slots = [
            (ActorSlot(0), "bms"),
            (ActorSlot(1), "ev_driver"),
            (ActorSlot(2), "thermostat"),
        ];
        s.register_actors_in_phase(slots, ExecutionPhase::ActorDecide, 5);
        let plan = s.build();

        assert_eq!(plan.len(), 3);
        assert_eq!(plan[0].name, "bms");
        assert_eq!(plan[1].name, "ev_driver");
        assert_eq!(plan[2].name, "thermostat");
        assert!(plan.iter().all(|e| e.phase == ExecutionPhase::ActorDecide));
    }

    #[test]
    fn registration_count_tracks_actors() {
        let mut s = StepScheduler::default();
        assert_eq!(s.registration_count(), 0);

        s.register_actor(ActorSlot(0), ExecutionPhase::ActorDecide, 0, "a");
        s.register_actor(ActorSlot(1), ExecutionPhase::ActorDecide, 0, "b");
        assert_eq!(s.registration_count(), 2);

        // solver feedback is a marker, not a registration
        s.register_solver_feedback();
        assert_eq!(s.registration_count(), 2);
    }

    #[test]
    fn is_empty_true_for_default() {
        let s = StepScheduler::default();
        assert!(s.is_empty());
    }

    #[test]
    fn is_empty_false_with_solver_feedback_only() {
        let mut s = StepScheduler::default();
        s.register_solver_feedback();
        assert!(!s.is_empty());
    }

    #[test]
    fn no_solver_feedback_when_not_registered() {
        let mut s = StepScheduler::default();
        s.register_actor(ActorSlot(0), ExecutionPhase::ActorDecide, 0, "actor");
        let plan = s.build();

        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].phase, ExecutionPhase::ActorDecide);
    }
}
