//! The step pipeline: the nine legs of `run_timestep`, each one owned
//! method, and the phase clock whose transitions pin their order.
//!
//! The legs run in code order and nothing runs between them. The
//! profiling transitions between legs are part of the contract: their
//! order and presence are pinned here, at the leg boundaries, and the
//! interior transitions (the scheduler's plan entries, the second
//! `control` span, the `state_snapshot` and `accounting` entries) stay
//! inside the leg whose work they span. Recording stays in
//! `recording.rs` and `hares-output`; the pipeline owns orchestration
//! only.

use std::time::Duration as StdDuration;

use hares_control::{DispatchRequest, DispatchTarget, PriorityTier};

use super::*;
#[cfg(feature = "observe")]
use crate::observer::{
    DispatchCapture, EquipmentObservation, EquipmentPhaseCapture, PhaseSnapshots, PortsCapture,
    StepSnapshot,
};

/// The phases of `run_timestep`, named only by the step's phase clock.
/// Declaration order is the spans' code order in `run_timestep`.
#[cfg(feature = "profiling")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProfilePhase {
    Environment,
    Control,
    IdealCapacity,
    Actors,
    Dispatch,
    Equipment,
    Envelope,
    Invariants,
    StateSnapshot,
    Output,
    Accounting,
}

#[cfg(feature = "profiling")]
/// One phase clock per step: `Instant::now()` is read once at entry and once
/// at each phase boundary, and each reading closes the open span into its
/// phase and opens the next, so the phases partition the step exactly with
/// no residual. A step that returns early (a failed invariant) records
/// nothing for that step; an unhealthy actor is logged and the step
/// continues.
#[cfg(feature = "profiling")]
struct PhaseClock {
    started: Instant,
    open: Instant,
    phase: ProfilePhase,
    /// The actor slot whose `Actors` span is currently open, with whether
    /// the interest filter called its `decide()` so far. `None` when the
    /// open span is not an `Actors` span. The flag is what keeps the
    /// per-actor totals summing to the `actors` phase exactly: the closed
    /// span is one `Instant` difference routed to both the phase and the
    /// slot.
    open_actor: Option<(usize, bool)>,
}

#[cfg(feature = "profiling")]
impl PhaseClock {
    pub(super) fn start() -> Self {
        let now = Instant::now();
        Self {
            started: now,
            open: now,
            phase: ProfilePhase::Environment,
            open_actor: None,
        }
    }

    /// Closes the open span into its phase and opens `next`.
    pub(super) fn enter(&mut self, profiling: &mut DwellingProfilingSummary, next: ProfilePhase) {
        let now = Instant::now();
        profiling.add_span(self.phase, now - self.open);
        self.open = now;
        self.phase = next;
    }

    /// Scheduler-loop variant of [`Self::enter`]: closes the open span into
    /// its phase, routing an `Actors` span to its actor's slot in
    /// `timings`, then opens `next`. `actor_slot` is `Some` only for an
    /// `ActorDecide` entry, whose span is the actor's scheduler-entry time
    /// (interest filter, `decide()` and health check included: the whole
    /// span the phase clock attributes to `actors`).
    pub(super) fn enter_plan_entry(
        &mut self,
        profiling: &mut DwellingProfilingSummary,
        timings: &mut ActorTimings,
        next: ProfilePhase,
        actor_slot: Option<usize>,
    ) {
        let now = Instant::now();
        let span = now - self.open;
        profiling.add_span(self.phase, span);
        self.route_closed_actor_span(timings, span);
        self.open = now;
        self.phase = next;
        self.open_actor = actor_slot.map(|slot| (slot, false));
    }

    /// Records that the open `Actors` span's filter called `decide()`. Must
    /// run inside the plan entry whose span is open.
    pub(super) fn mark_open_actor_called(&mut self) {
        if let Some((_, called)) = &mut self.open_actor {
            *called = true;
        }
    }

    /// Closes the final span at return and accumulates `step_total` (entry
    /// to return) over the run. The open phase here is always `accounting`
    /// (the last `enter` before return opens it and nothing intervenes), so
    /// no actor span is routed.
    pub(super) fn finish(&mut self, profiling: &mut DwellingProfilingSummary) {
        let now = Instant::now();
        profiling.add_span(self.phase, now - self.open);
        profiling.step_total += now - self.started;
    }

    /// Routes a just-closed span to its actor's slot when it is an `Actors`
    /// span. The same `Instant` difference already went to the phase, so
    /// the per-actor totals sum to the `actors` phase exactly.
    fn route_closed_actor_span(&self, timings: &mut ActorTimings, span: StdDuration) {
        if self.phase == ProfilePhase::Actors
            && let Some((slot, called)) = self.open_actor
        {
            timings.record(slot, span, called);
        }
    }
}
/// The step's orchestration state: the phase clock and the per-step
/// locals the legs hand forward, dropped with the step (an errored step
/// drops the observer slot exactly as the monolithic step body did).
pub(super) struct StepPipeline {
    #[cfg(feature = "profiling")]
    phase_clock: PhaseClock,
    #[cfg(feature = "profiling")]
    alloc_before: Option<u64>,
    #[cfg(debug_assertions)]
    rng_consumed_this_step: bool,
    #[cfg(feature = "observe")]
    obs_slot: Option<StepSnapshot>,
    #[cfg(feature = "observe")]
    obs_phases: PhaseSnapshots,
    #[cfg(feature = "observe")]
    observing: bool,
    #[cfg(feature = "observe")]
    dispatch_capture_buf: DispatchCapture,
    #[cfg(feature = "observe")]
    scheduled_phases: Vec<String>,
    #[cfg(feature = "observe")]
    actor_skips: usize,
    #[cfg(feature = "observe")]
    actor_calls: usize,
    #[cfg(feature = "observe")]
    actor_error_count: usize,
    #[cfg(feature = "observe")]
    thermal_obs: Vec<EquipmentObservation>,
    #[cfg(feature = "observe")]
    thermal_phase_ports: PortsCapture,
    #[cfg(feature = "observe")]
    nonthermal_obs: Vec<EquipmentObservation>,
    #[cfg(feature = "observe")]
    nonthermal_phase_ports: PortsCapture,
    #[cfg(feature = "observe")]
    thermal_obs_used: usize,
    #[cfg(feature = "observe")]
    nonthermal_obs_used: usize,
    /// The step's interval, read once in the environment leg and handed
    /// to the equipment steps and the solvers.
    dt: StdDuration,
}

impl Default for StepPipeline {
    fn default() -> Self {
        Self {
            #[cfg(feature = "profiling")]
            phase_clock: PhaseClock::start(),
            #[cfg(feature = "profiling")]
            alloc_before: None,
            #[cfg(debug_assertions)]
            rng_consumed_this_step: false,
            #[cfg(feature = "observe")]
            obs_slot: None,
            #[cfg(feature = "observe")]
            obs_phases: PhaseSnapshots::default(),
            #[cfg(feature = "observe")]
            observing: false,
            #[cfg(feature = "observe")]
            dispatch_capture_buf: DispatchCapture::default(),
            #[cfg(feature = "observe")]
            scheduled_phases: Vec::new(),
            #[cfg(feature = "observe")]
            actor_skips: 0,
            #[cfg(feature = "observe")]
            actor_calls: 0,
            #[cfg(feature = "observe")]
            actor_error_count: 0,
            #[cfg(feature = "observe")]
            thermal_obs: Vec::new(),
            #[cfg(feature = "observe")]
            thermal_phase_ports: PortsCapture::default(),
            #[cfg(feature = "observe")]
            nonthermal_obs: Vec::new(),
            #[cfg(feature = "observe")]
            nonthermal_phase_ports: PortsCapture::default(),
            #[cfg(feature = "observe")]
            thermal_obs_used: 0,
            #[cfg(feature = "observe")]
            nonthermal_obs_used: 0,
            dt: StdDuration::default(),
        }
    }
}

impl StepPipeline {
    /// The step's nine legs, in code order, with the phase transitions
    /// pinned between them. The pipeline is the step's phase clock: its
    /// construction starts the clock (the `environment` phase opens
    /// there), each transition closes the open span into its phase, and
    /// the final `finish` closes the last span and `step_total`.
    pub(super) fn run(
        &mut self,
        d: &mut Dwelling,
        record_output: bool,
        out: &mut StepResult,
    ) -> Result<()> {
        self.leg_environment_and_control(d)?;
        // End of the `environment` phase: entry through Step 1a' (environment
        // update, queued control dispatch).
        #[cfg(feature = "profiling")]
        self.phase_clock.enter(&mut d.profiling, ProfilePhase::Control);

        self.leg_scheduled_actors(d)?;
        // The `dispatch` phase opens here: the ActorDecide buffer drain, the
        // post-loop actor health checks and Steps 2 and 2b all queue or apply
        // control signals. The plan variant routes the final plan entry's
        // span (an `Actors` span when the plan ends with one) to its slot.
        #[cfg(feature = "profiling")]
        self.phase_clock.enter_plan_entry(
            &mut d.profiling,
            &mut d.actor_timings,
            ProfilePhase::Dispatch,
            None,
        );

        self.leg_dispatch_and_derived(d)?;
        #[cfg(feature = "profiling")]
        self.phase_clock.enter(&mut d.profiling, ProfilePhase::Equipment);

        self.leg_independent_equipment(d)?;
        self.leg_electrical_equipment(d)?;
        self.leg_thermal_equipment(d)?;
        // End of the `equipment` phase (Steps 3 and 3c).
        #[cfg(feature = "profiling")]
        self.phase_clock.enter(&mut d.profiling, ProfilePhase::Envelope);

        self.leg_envelope_and_custom_solvers(d)?;
        // End of the `envelope` phase (Step 4 and its diagnostic capture);
        // the always-on invariant checks and check_step_invariants follow.
        #[cfg(feature = "profiling")]
        self.phase_clock.enter(&mut d.profiling, ProfilePhase::Invariants);

        self.leg_invariants_and_state_snapshot(d, out)?;
        // End of the `state_snapshot` phase (observer push, diagnostic
        // accumulation, end-of-step equipment snapshot, step result).
        #[cfg(feature = "profiling")]
        self.phase_clock.enter(&mut d.profiling, ProfilePhase::Output);

        self.leg_output_and_accounting(d, record_output, out)?;
        // Closes the `accounting` phase (billing, electrical summary, clock
        // advance) and records `step_total` from entry to return.
        #[cfg(feature = "profiling")]
        self.phase_clock.finish(&mut d.profiling);


        Ok(())
    }
    /// The step's environment and control preparation: the dwelling RNG's
    /// advance, the observability and restore checks, the environment
    /// update with the price pipeline's precedence rule and the island
    /// formation, the step-start humidity invariant, the observer's
    /// environment capture, and the step's first dispatch pass (Step 1a',
    /// the externally queued signals) under a fresh cross-pass ledger.
    #[allow(clippy::too_many_lines)]
    fn leg_environment_and_control(&mut self, d: &mut Dwelling) -> Result<()> {
        // Advance the dwelling RNG on every timestep so checkpoint captures
        // reflect simulation progress.  The value is intentionally discarded;
        // stochastic components use independent sub-RNGs derived from the
        // dwelling RNG's seed via stream partitioning.
        #[cfg(debug_assertions)]
        let rng_word_pos_before = d.rng.get_word_pos();
        let _ = advance_dwelling_rng(&mut d.rng);

        #[cfg(debug_assertions)]
        {
            self.rng_consumed_this_step = d.rng.get_word_pos() > rng_word_pos_before;
        }

        #[cfg(debug_assertions)]
        if !self.rng_consumed_this_step {
            debug_assert!(false, "dwelling RNG consumed no bits this step");
        }

        // After the first step, every equipment instance must have a core
        // output entry in the environment: actors observe equipment through
        // these id-keyed entries, and a missing entry means the equipment
        // is unobservable (the no-observation sentinel) for the rest of
        // the run. Known causes this check guards against: an identity
        // refresh that failed to seed a newly entered equipment's entry,
        // and a checkpoint restore that skipped the post-restore
        // snapshot. A tolerated equipment-step failure is NOT a cause:
        // the last committed entry is retained, not dropped.
        // Unconditional in every build profile, over the ids pre-resolved
        // at refresh (no per-equipment hash lookups on the step path).
        if d.clock.current_step() > 0 {
            for (idx, &id) in d.roster.equipment_ids.iter().enumerate() {
                if !d.latest_env.equipment_core.contains_key(&id) {
                    let name = d
                        .equipment
                        .get(idx)
                        .map(|eq| eq.descriptor().name.clone())
                        .unwrap_or_default();
                    return Err(HaresError::InvalidState(format!(
                        "equipment_core missing entry for equipment '{name}' (id={id:?}) \
                         at start of step {}: every active equipment must be \
                         observable; check the identity-refresh seeding and \
                         the checkpoint-restore snapshot (tolerated step \
                         failures retain the last committed entry)",
                        d.clock.current_step(),
                    )));
                }
            }
        }

        // First step after checkpoint restore: double-check that
        // equipment_core is not accidentally empty when we expect
        // restored state. The latch reset runs in every build profile.
        if d.restored_from_checkpoint && !d.equipment.is_empty() {
            if d.latest_env.equipment_core.is_empty() {
                return Err(HaresError::InvalidState(format!(
                    "equipment_core is empty on first post-restore step {}; \
                     load_checkpoint must call snapshot_equipment_state",
                    d.clock.current_step(),
                )));
            }
            d.restored_from_checkpoint = false;
        }

        #[cfg(feature = "profiling")]
        {
            self.alloc_before = thread_allocations();
        }

        #[cfg(feature = "observe")]
        // The recycled slot: when the observer's ring is full, the push
        // at the end of the step would evict its oldest snapshot; the
        // take does that eviction now and hands the slot over, so the
        // step's capture writes reuse the slot's buffers in place (the
        // telemetry maps' reuse is the row's own). The taken slot's
        // scalars are rewritten at the push; a step that errors before
        // it drops the slot and leaves the ring one slot short until the
        // next push (the failed run is over; nothing reads it).
        {
            self.obs_slot = d.observer_buf.as_mut().map(|buf| buf.take_slot());
        }
        #[cfg(feature = "observe")]
        {
            self.obs_phases =
                self.obs_slot
                .as_mut()
                .map(|slot| std::mem::take(&mut slot.phases))
                .unwrap_or_default();
        }

        // Step 1: update environment at current clock state.
        // Feed zone temperatures back first so the borrow on d.latest_env.zones
        // is released before we mutably borrow d.latest_env for update_in_place.
        d.environment.feed_zones(&d.latest_env.zones);
        // The grid voltage the previous step resolved with: the update below
        // overwrites it with this step's. The island's formation condition
        // reads it beside the equipment state (which is likewise
        // end-of-previous-step state), so island formation takes effect with
        // the documented one-step lag: an outage's first step is a dead-bus
        // step, and the island forms on the step after. The capture is a
        // local over the environment, not checkpoint state: a checkpoint
        // carries no grid input (the same external rule as the voltage), so
        // a resumed run's first step reads the grid it is restored into.
        let prior_step_outage = d.latest_env.grid.grid_outage();
        d.environment
            .update_in_place(&mut d.latest_env, &d.clock)?;

        // The price pipeline's precedence rule (docs/PHYSICS_DECISIONS.md):
        // an explicitly applied price signal (the setter, the fleet's
        // `set_price_signal_all`, the HELICS price topics) is the
        // operator's hand and wins for the one step it was applied on; the
        // attached tariff evaluator is the background feed and governs
        // every step no explicit price was applied on. The marker is
        // consumed here every step, so an application overrides exactly
        // the step it landed on and the tariff resumes on the next.
        let explicit_price_applied = std::mem::take(&mut d.price_signal_applied);
        match d.tariff_evaluator.as_ref() {
            Some(evaluator) if !explicit_price_applied => {
                // The tariff feeds the import and export prices only. The
                // environment's price signal was reset by the update just
                // above, so the intensity's unchanged value is the stored
                // signal's, never the view's (which would clear it every
                // tariff step).
                d.latest_env.price_signal = PriceSignal {
                    electricity_price: Some(evaluator.current_price()),
                    export_price: Some(evaluator.current_export_price()),
                    ghg_intensity: d.price_signal.ghg_intensity,
                };
            }
            // An explicit price this step, or no tariff attached: the
            // stored signal is the price.
            Some(_) | None => d.latest_env.price_signal = d.price_signal.clone(),
        }

        // Populate electrical summary from prior step's solver results.
        d.latest_env.electrical = d.prior_electrical_summary.clone();

        // Resolve bus energization for this step. During a utility outage
        // (voltage_pu == 0.0), an island-capable source (battery with usable
        // charge, generator, discharging V2G EV) holds the home bus at
        // nominal voltage so loads keep running; otherwise the bus is dead
        // and every load force-offs at the control level (see
        // `GridState::bus_energized`). The formation condition reads the
        // previous step's outage (the capture above) and the source
        // availability from the equipment state, both end-of-previous-step
        // state, beside the step's own utility voltage. The two lagged terms
        // give an outage's first step the dead bus and formation the step
        // after; the step's own voltage collapses a formed island on the
        // restoration step itself.
        d.latest_env.grid.island_bus_voltage_pu = if prior_step_outage
            && d.latest_env.grid.voltage_pu == 0.0
            && d.equipment.iter().any(|eq| eq.island_source_available())
        {
            Some(ISLAND_BUS_NOMINAL_VOLTAGE_PU)
        } else {
            None
        };

        // Step-start humidity invariant: confirm that the humidity ratio in
        // `latest_env.zones` matches the humidity solver's committed state.
        // Both are updated together at the end of each timestep by
        // `apply_humidity_update_to_zones` → `humidity_solver.resolve`, and
        // Step 1's environment update preserves zone state byte-for-byte via
        // `extend_from_slice`. A mismatch here indicates a zone was added
        // after solver construction without seeding its initial humidity ratio.
        #[cfg(debug_assertions)]
        for zone in &d.latest_env.zones {
            if d.is_warming_up {
                continue;
            }
            let solver_hr = d.humidity_solver.humidity_ratio(zone.id);
            if zone.humidity_ratio.is_nan() && solver_hr.is_nan() {
                continue;
            }
            debug_assert!(
                (zone.humidity_ratio - solver_hr).abs() < f64::EPSILON,
                "zone {} humidity ratio {:.6e} diverged from solver committed {:.6e} at start of step",
                zone.id.0,
                zone.humidity_ratio,
                solver_hr,
            );
        }

        #[cfg(feature = "observe")]
        if d.observer_buf.is_some() {
            // The recycled slot's own environment capture refilled in
            // place (a fresh capture only before the slot first
            // carries one): the three Vecs reuse their capacities.
            #[cfg(test)]
            let env_before = thread_allocations();
            match self.obs_phases.post_environment.as_mut() {
                Some(capture) => observer_capture::capture_environment_into(
                    capture,
                    &d.latest_env,
                    d.environment.weather_meta().wf_allows_leap_years,
                    d.environment.raw_mains_temp_f,
                    d.environment.obs_raw_day_of_year,
                    d.environment.obs_civil_day_of_year,
                    d.environment.obs_utc_day_of_year,
                ),
                None => {
                    self.obs_phases.post_environment = Some(observer_capture::capture_environment(
                        &d.latest_env,
                        d.environment.weather_meta().wf_allows_leap_years,
                        d.environment.raw_mains_temp_f,
                        d.environment.obs_raw_day_of_year,
                        d.environment.obs_civil_day_of_year,
                        d.environment.obs_utc_day_of_year,
                    ));
                }
            }
            #[cfg(test)]
            d.observe_allocations
                .record(env_before, ObserveAllocSite::EnvironmentCapture);
        }

        #[cfg(feature = "observe")]
        {
            self.observing = d.observer_buf.is_some();
        }

        self.dt = chrono_to_std_duration(d.clock.time_res)?;

        // Begin a new dispatch window: clear the cross-pass priority ledger so
        // that lower-priority signals queued late in the step cannot overwrite
        // higher-priority signals applied in an earlier pass.
        d.control_dispatcher.begin_step();

        // Step 1a': dispatch any externally queued control signals (e.g. from
        // `apply_control_validated`) before thermal equipment `update_control`
        // runs. Without this, setpoint/mode overrides queued for the current
        // step would be serviced at Step 2 (after the thermostat FSM has
        // already advanced in Step 1c), and short-cycle protection in
        // `is_cycle_change_allowed` would block the re-evaluation at Step 2a
        // until the next step. Dispatching first lets setpoint and mode
        // overrides take effect on the same step.
        #[cfg(feature = "observe")]
        // The dispatch capture's buffers, taken from the recycled slot's
        // post-dispatch capture (its Vecs retained) or fresh: the signals
        // Vec clears once per step and every self.observing pass appends into
        // it, so the capture the snapshot carries holds every pass's
        // signals in dispatch order over the pre-pass's own conflict
        // list, exactly the merged content the per-pass-fresh-and-extend
        // shape produced, without the fresh Vecs per pass.
        {
            self.dispatch_capture_buf = self.obs_phases.post_dispatch.take().unwrap_or_default();
        }
        #[cfg(feature = "observe")]
        {
            self.dispatch_capture_buf.signals.clear();
        }
        #[cfg(feature = "observe")]
        if d.observer_buf.is_some() {
            #[cfg(test)]
            let dc_before = thread_allocations();
            d.dispatch_pass_observed_into(
                &mut self.dispatch_capture_buf.signals,
                Some(&mut self.dispatch_capture_buf.same_tier_conflicts),
            )?;
            #[cfg(test)]
            d.observe_allocations
                .record(dc_before, ObserveAllocSite::DispatchCaptureBuf);
        } else {
            d.dispatch_pass()?;
        }
        #[cfg(not(feature = "observe"))]
        d.dispatch_pass()?;

        Ok(())
    }
    /// The scheduled actor decisions: the occupancy gains' deposit, the
    /// thermal equipment's pre-actor `update_control`, the thermal
    /// solver's input preparation, and the scheduler plan's phase-ordered
    /// execution (the solver-feedback entry, then every actor's decide
    /// behind its interest filter).
    #[allow(clippy::too_many_lines)]
    fn leg_scheduled_actors(&mut self, d: &mut Dwelling) -> Result<()> {

        // Step 1b: deposit deterministic internal gains (occupancy, plug loads)
        // BEFORE prepare_inputs so the ideal solver sees them when computing
        // required HVAC capacity.
        d.apply_occupancy_gains()?;

        d.step_failed.fill(false);

        // Step 1c: thermal equipment update_control() to determine mode and ideal targets.
        // Must run BEFORE solver feedback actor collects targets.
        for &idx in &d.roster.equipment_execution_order {
            if d.equipment[idx].descriptor().stage == ExecutionStage::Thermal {
                d.equipment[idx].update_control(&d.latest_env);
            }
        }

        // Step 1d: build current-step inputs with all non-HVAC gains already on ports.
        d.thermal_solver
            .prepare_inputs(&d.ports, &d.latest_env)?;

        // Steps 1e to 1f: phase-ordered actor execution driven by the scheduler's
        // plan (explicit phase registration, within-phase priority ordering),
        // rebuilt below only when a registration changed.
        #[cfg(feature = "observe")]
        {
            self.scheduled_phases = Vec::new();
        }
        #[cfg(feature = "observe")]
        {
            self.actor_skips = 0;
        }
        #[cfg(feature = "observe")]
        {
            self.actor_calls = 0;
        }
        #[cfg(feature = "observe")]
        {
            self.actor_error_count = 0;
        }
        #[cfg(debug_assertions)]
        {
            // The step's executed-actor set, the dwelling's persistent
            // debug scratch: cleared per step, so a debug build's
            // steady state allocates nothing for it either.
            d.executed_actors_scratch.clear();
        }

        d.actor_dispatch_buf.clear();
        for entry in d.scheduler.build() {
            // The scheduler loop interleaves `ideal_capacity` and `actors`
            // spans in plan order: each entry closes the previous span and
            // opens its own phase. An `ActorDecide` entry's span (the
            // interest filter, `decide()` and the health check) is routed
            // to that actor's slot in `actor_timings` when it closes, so
            // the per-actor totals sum to the `actors` phase exactly.
            #[cfg(feature = "profiling")]
            match entry.phase {
                ExecutionPhase::SolverFeedback => self.phase_clock.enter_plan_entry(
                    &mut d.profiling,
                    &mut d.actor_timings,
                    ProfilePhase::IdealCapacity,
                    None,
                ),
                ExecutionPhase::ActorDecide => self.phase_clock.enter_plan_entry(
                    &mut d.profiling,
                    &mut d.actor_timings,
                    ProfilePhase::Actors,
                    Some(
                        entry
                            .slot
                            .expect("ActorDecide plan entries must carry a slot")
                            .0,
                    ),
                ),
            }
            match entry.phase {
                ExecutionPhase::SolverFeedback => {
                    // The phase label feeds only the self.observing path's
                    // debug line (compiled out in release): a step whose
                    // observer is never enabled pays nothing for it.
                    #[cfg(feature = "observe")]
                    if self.observing {
                        self.scheduled_phases.push("SolverFeedback".to_string());
                    }

                    // Step 1e: collect ideal targets and solve capacities.
                    d.solver_feedback_actor
                        .collect_and_solve(&d.equipment, &mut d.thermal_solver);
                    // Step 1f: decide and queue solver feedback signals.
                    d.solver_feedback_actor
                        .decide(&d.latest_env, &mut d.actor_dispatch_buf);
                    for req in d.actor_dispatch_buf.drain(..) {
                        d.control_dispatcher.queue(req);
                    }
                }
                ExecutionPhase::ActorDecide => {
                    let idx = entry
                        .slot
                        .expect("ActorDecide plan entries must carry a slot")
                        .0;

                    let interests = d.actors[idx].interests();
                    let filter = InterestFilterState {
                        prev_zone_temps: &d.prev_zone_temps,
                        prior_zone_temps: &d.prior_zone_temps,
                        prev_price_signal: &d.prev_price_signal,
                        prev_equipment_modes: &d.prev_equipment_modes,
                        equipment_id_by_name: &d.equipment_id_by_name,
                        equipment: &d.equipment,
                    };
                    let should_call = interests.is_empty()
                        || interests.iter().any(|interest| {
                            Dwelling::interest_triggered(interest, &d.latest_env, &filter)
                        });

                    #[cfg(feature = "observe")]
                    if should_call {
                        self.actor_calls += 1;
                    } else {
                        self.actor_skips += 1;
                    }

                    if should_call {
                        // The actor label feeds only the self.observing
                        // path's debug line (compiled out in release):
                        // a step whose observer is never enabled pays
                        // nothing for it.
                        #[cfg(feature = "observe")]
                        if self.observing {
                            self.scheduled_phases.push(format!("ActorDecide({})", entry.name));
                        }
                        #[cfg(debug_assertions)]
                        {
                            d.executed_actors_scratch.insert(idx);
                        }

                        // The open `Actors` span (this entry's whole
                        // scheduler time, closed at the next phase
                        // boundary) counts a `decide()` call. No extra
                        // `Instant` read: the span's difference is the
                        // phase clock's own.
                        #[cfg(feature = "profiling")]
                        self.phase_clock.mark_open_actor_called();

                        let emitted_from = d.actor_dispatch_buf.len();
                        d.actors[idx].decide(&d.latest_env, &mut d.actor_dispatch_buf);
                        crate::actor::check_declared_signals(
                            d.actors[idx].as_ref(),
                            &d.actor_dispatch_buf[emitted_from..],
                        )?;

                        if !d.actors[idx].healthy() {
                            #[cfg(feature = "observe")]
                            {
                                self.actor_error_count += 1;
                            }
                            tracing::error!(
                                actor = %d.actors[idx].name(),
                                "actor is unhealthy after decide() — dispatch output may be compromised"
                            );
                        }
                    }
                }
            }
        }

        Ok(())
    }
    /// Dispatch and derived control: the decide buffer's drain, the
    /// every-step actor and post-decide health checks, Step 2's dispatch
    /// pass, Step 2b's derived-signal cascade, and Step 2a's thermal
    /// re-`update_control` under the reopened `control` span.
    #[allow(clippy::too_many_lines)]
    fn leg_dispatch_and_derived(&mut self, d: &mut Dwelling) -> Result<()> {
        // Drain dispatch from all ActorDecide entries.
        for req in d.actor_dispatch_buf.drain(..) {
            d.control_dispatcher.queue(req);
        }

        #[cfg(debug_assertions)]
        {
            for (i, actor) in d.actors.iter().enumerate() {
                let interests = actor.interests();
                let is_every_step = interests.is_empty()
                    || interests
                        .iter()
                        .any(|i| matches!(i, ActorInterest::EveryStep));
                let registered = d
                    .scheduler
                    .plan()
                    .iter()
                    .any(|e| e.slot == Some(crate::scheduler::ActorSlot(i)));
                if registered && is_every_step {
                    debug_assert!(
                        d.executed_actors_scratch.contains(&i),
                        "EveryStep actor '{}' (index {}) was not called during ActorDecide phase",
                        actor.name(),
                        i,
                    );
                }
            }
        }

        // Actor health after the decide phase: an unhealthy actor indicates a
        // swallowed error (Python exception or invalid return type) at the
        // decide() call site: input-triggerable enforcement, so it fails the
        // step in every build profile (the decide site's unconditional
        // tracing::error! already logged it).
        for (i, actor) in d.actors.iter().enumerate() {
            if !actor.healthy() {
                let cause = actor.health_detail().unwrap_or("no cause reported");
                return Err(HaresError::Dwelling(format!(
                    "actor '{}' (index {}) is unhealthy after the ActorDecide phase: \
                     {cause}; dispatch output may be compromised",
                    actor.name(),
                    i
                )));
            }
        }

        #[cfg(feature = "observe")]
        if d.observer_buf.is_some() {
            tracing::debug!(
                phase_count = self.scheduled_phases.len(),
                phases = ?self.scheduled_phases,
                "scheduler phases executed"
            );
        }

        // Step 2: dispatch all actor-generated control signals. The priority
        // ledger from the pre-thermal-FSM pass is preserved so that a
        // lower-priority actor signal cannot overwrite a higher-priority
        // external signal already applied.
        #[cfg(feature = "observe")]
        if d.observer_buf.is_some() {
            // The post-decide pass's conflicts record to the scan's own
            // scratch (`None`): the merged capture keeps the pre-pass's,
            // exactly as the old merge did. The pass appends into the
            // same signals buffer, and the merged capture moves into the
            // phase only after the derived-signal loop below has
            // appended too.
            #[cfg(test)]
            let dc_before = thread_allocations();
            d.dispatch_pass_observed_into(&mut self.dispatch_capture_buf.signals, None)?;
            #[cfg(test)]
            d.observe_allocations
                .record(dc_before, ObserveAllocSite::DispatchCaptureBuf);
        } else {
            d.dispatch_pass()?;
        }
        #[cfg(not(feature = "observe"))]
        d.dispatch_pass()?;

        // Step 2b: collect and dispatch derived control signals from equipment
        // that translate one signal into others (e.g. ProtocolBridge converting
        // a ProtocolNative payload into standard ControlSignal variants).
        //
        // This runs AFTER the actor dispatch pass (Step 2) and BEFORE thermal
        // re-update (Step 2a) so that translated setpoints, mode overrides, and
        // power targets take effect on the CURRENT timestep.
        //
        // A bounded loop (max 3 iterations) handles cascades: a derived signal
        // dispatched to a second ProtocolBridge could produce further derived
        // signals. In practice one iteration suffices because bridges emit
        // standard signals, not ProtocolNative.
        const MAX_DERIVED_ITERATIONS: u32 = 3;
        let mut protocol_native_checked = false;
        for _iteration in 0..MAX_DERIVED_ITERATIONS {
            let mut derived: Vec<DispatchRequest> = Vec::new();
            for eq in d.equipment.iter_mut() {
                for (target_name, signal) in eq.drain_command_signals() {
                    derived.push(DispatchRequest {
                        target: DispatchTarget::ByName(target_name.into()),
                        signal,
                        // Protocol-bridge-translated signals are equipment-internal
                        // commands that should take precedence over schedule-tier
                        // operations but not override grid-level DR signals.
                        priority: PriorityTier::UserOverride,
                    });
                }
            }
            if derived.is_empty() {
                break;
            }
            if !protocol_native_checked {
                protocol_native_checked = true;
                let capabilities: Vec<ControlCapabilities> = d
                    .equipment
                    .iter()
                    .map(|e| e.descriptor().control_capabilities)
                    .collect();
                d.invariant_checker
                    .check_protocol_native_registration(&capabilities)?;
            }
            for req in derived {
                d.control_dispatcher.queue(req);
            }
            #[cfg(feature = "observe")]
            if d.observer_buf.is_some() {
                d.dispatch_pass_observed_into(&mut self.dispatch_capture_buf.signals, None)?;
            } else {
                d.dispatch_pass()?;
            }
            #[cfg(not(feature = "observe"))]
            d.dispatch_pass()?;
        }

        // The dispatch capture moves into the phase here: every pass this
        // step took has appended, so the capture is the merged one.
        #[cfg(feature = "observe")]
        if d.observer_buf.is_some() {
            self.obs_phases.post_dispatch = Some(std::mem::take(&mut self.dispatch_capture_buf));
        }

        // End of the `dispatch` phase (Steps 2 and 2b); Step 2a re-runs
        // `update_control`, the second span of the `control` phase.
        #[cfg(feature = "profiling")]
        self.phase_clock.enter(&mut d.profiling, ProfilePhase::Control);

        // Step 2a: re-run update_control for thermal equipment after control
        // dispatch so ThermalSetpoint / ModeOverride / DR signals take effect
        // on the current timestep, not one step later.
        for &idx in &d.roster.equipment_execution_order {
            if d.equipment[idx].descriptor().stage == ExecutionStage::Thermal {
                d.equipment[idx].update_control(&d.latest_env);
            }
        }

        Ok(())
    }
    /// The independent equipment leg: the observer capture lists' setup,
    /// the stage-rank scratch's reset, Phase 1's Independent-stage steps
    /// (PV, generators), and the PV-BMS re-evaluation pass that
    /// re-dispatches on same-step generation.
    #[allow(clippy::too_many_lines)]
    fn leg_independent_equipment(&mut self, d: &mut Dwelling) -> Result<()> {

        // Step 3: unified equipment step in stage_rank order
        // (Independent → Electrical → Thermal).
        //
        // Independent equipment (PV, generators) steps first, depositing
        // generation data to ports. BMS actors re-evaluate based on actual
        // PV generation. Electrical equipment (battery, EV) steps next
        // with the revised signals. Thermal equipment (HVAC, water heaters,
        // ventilation) steps last, committing heating/cooling power with
        // full visibility of same-step PV generation and storage dispatch.
        //
        // This replaces the prior two-pass structure (Step 3a thermal-first,
        // Step 3b non-thermal-second) that reversed the stage_rank dependency
        // chain and caused a one-step lag for PV-informed HVAC dispatch.
        //
        // The PV-BMS re-evaluation pass is gated behind a timestep threshold
        // (default: ≥ 5 minutes) because 1-minute timesteps have <1%/s PV
        // ramp rates where the one-step PV lag has negligible impact.
        #[cfg(feature = "observe")]
        // The per-equipment observation lists, seeded from the recycled
        // slot's own phase captures: each element's telemetry map and
        // buffers are reused in place (the row's clone_from site) while
        // the equipment count matches, and a roster change falls back to
        // fresh captures beyond the recycled elements. The used counters
        // track this step's processed equipment per list; the stale tail
        // (a roster shrink) truncates before the phase capture. The
        // phase captures' own port snapshots are kept beside the lists:
        // their Vecs are the recycled buffers the assembly refills.
        {
            let (thermal_obs_recycled, thermal_phase_ports_recycled): (
                Vec<EquipmentObservation>,
                PortsCapture,
            ) = match self.obs_phases.post_thermal_equipment.take() {
                Some(capture) => (capture.equipment, capture.ports),
                None => (Vec::new(), PortsCapture::default()),
            };
            self.thermal_obs = thermal_obs_recycled;
            self.thermal_phase_ports = thermal_phase_ports_recycled;
        }
        #[cfg(feature = "observe")]
        {
            let (nonthermal_obs_recycled, nonthermal_phase_ports_recycled): (
                Vec<EquipmentObservation>,
                PortsCapture,
            ) = match self.obs_phases.post_nonthermal_equipment.take() {
                Some(capture) => (capture.equipment, capture.ports),
                None => (Vec::new(), PortsCapture::default()),
            };
            self.nonthermal_obs = nonthermal_obs_recycled;
            self.nonthermal_phase_ports = nonthermal_phase_ports_recycled;
            self.thermal_obs_used = 0;
            self.nonthermal_obs_used = 0;
        }
        #[cfg(feature = "observe")]
        // The pre-step port snapshot: the persistent buffer's in-place
        // fill. The first observed step allocates the buffer's Vecs;
        // every step after reuses them, and the re-writes after each
        // equipment's step keep the buffer current for the next
        // equipment's diff (the fresh `PortSlots` clone per step and the
        // fresh clone after every equipment are gone).
        if self.observing {
            #[cfg(test)]
            let ps_before = thread_allocations();
            d.port_snapshot_buf.copy_into(&d.ports);
            #[cfg(test)]
            d.observe_allocations
                .record(ps_before, ObserveAllocSite::PortSnapshotBuf);
        }

        // Runtime stage-rank tracking: records the actual step execution
        // sequence so the invariant checker validates real loop ordering,
        // not just the static sort of equipment_execution_order.
        //
        // The scratch persists across steps (the dwelling's
        // `stepped_stage_ranks` field) and is re-derived from scratch
        // every step: this clear is the only place that empties it and
        // the three phase loops below are the only places that fill it,
        // so the stage-ordering check at the end of the phase reads this
        // step's ranks and nothing else. That is also why no equipment-
        // or state-mutation point needs to clear it: a stale rank cannot
        // cross a step boundary, a roster change (`install_roster_caches`
        // never reads or restores the buffer; the next step clears it
        // before the first push), or a checkpoint restore (the restore
        // never rebuilds the roster and the buffer is not checkpoint
        // state). The field's doc carries the full invalidation proof.
        #[cfg(test)]
        let stage_rank_alloc_before = thread_allocations();
        d.stepped_stage_ranks.clear();
        #[cfg(test)]
        d.record_snapshot_allocations(
            stage_rank_alloc_before,
            SnapshotAllocSite::StageRankScratch,
        );

        // Phase 1: Independent-stage equipment (PV, generators).
        for _oi in 0..d.roster.equipment_execution_order.len() {
            let idx = d.roster.equipment_execution_order[_oi];
            let stage = d.equipment[idx].descriptor().stage;
            if stage != ExecutionStage::Independent {
                continue;
            }
            #[cfg(feature = "observe")]
            #[cfg(test)]
            let pp_before = thread_allocations();
            #[cfg(feature = "observe")]
            let pre_ports = if self.observing {
                // The recycled path writes the pre-step capture straight
                // into the recycled observation's own port Vecs (the
                // capture must precede the step); a fresh observation (a
                // roster growth) builds its own capture to move in.
                if self.nonthermal_obs_used < self.nonthermal_obs.len() {
                    observer_capture::capture_ports_into(
                        &mut self.nonthermal_obs[self.nonthermal_obs_used].pre_step_ports,
                        &d.port_snapshot_buf,
                    );
                    None
                } else {
                    Some(observer_capture::capture_ports(&d.port_snapshot_buf))
                }
            } else {
                None
            };
            #[cfg(feature = "observe")]
            #[cfg(test)]
            d.observe_allocations
                .record(pp_before, ObserveAllocSite::EquipmentPrePorts);

            let _ = d.equipment[idx].update_control(&d.latest_env);
            if d.step_validation_enabled {
                d.rollback_ports.copy_into(&d.ports);
            }
            // Snapshot the shared electrical bus so the post-step delta is
            // exactly this equipment's contribution (ElectricalAccumulator
            // is Copy).
            let pre_electrical = d.ports.electrical;
            let pre_state_energy_j = d.equipment[idx].stored_energy_j();
            if let Err(err) = d.equipment[idx].step(&d.latest_env, self.dt, &mut d.ports) {
                d.record_equipment_step_failure(idx, err, pre_state_energy_j)?;
            } else {
                if d.step_validation_enabled {
                    d.validate_equipment_contract(idx)?;
                    validate_port_core_electrical_consistency(
                        d.equipment[idx].descriptor(),
                        d.equipment[idx].core_output(),
                        pre_electrical,
                        &d.ports.electrical,
                    )?;
                }
                d.record_equipment_step_success(idx);
                d.stepped_stage_ranks.push(stage_rank(stage));
            }

            #[cfg(feature = "observe")]
            {
                tracing::debug!(
                    stage = ?stage,
                    equipment = %d.equipment[idx].descriptor().name,
                    "equipment step executed"
                );
                if self.observing {
                    let contribution =
                        observer_capture::diff_ports(&d.port_snapshot_buf, &d.ports);
                    if self.nonthermal_obs_used < self.nonthermal_obs.len() {
                        observer_capture::capture_single_equipment_into(
                            &mut self.nonthermal_obs[self.nonthermal_obs_used],
                            d.equipment[idx].as_ref(),
                            contribution,
                            None,
                        );
                    } else {
                        self.nonthermal_obs.push(observer_capture::capture_single_equipment(
                            d.equipment[idx].as_ref(),
                            contribution,
                            pre_ports.expect("a fresh observation carries its pre-step capture"),
                        ));
                    }
                    self.nonthermal_obs_used += 1;
                    #[cfg(test)]
                    let ps_before = thread_allocations();
                    d.port_snapshot_buf.copy_into(&d.ports);
                    #[cfg(test)]
                    d.observe_allocations
                        .record(ps_before, ObserveAllocSite::PortSnapshotBuf);
                }
            }
        }

        // PV-BMS re-evaluation: after Independent-stage equipment has
        // deposited generation data to ports, re-evaluate BMS actors
        // with actual (not prior-step) PV generation so that battery
        // charge power setpoints reflect same-step PV availability.
        {
            let time_step_secs = d.latest_env.time_step_secs();
            if time_step_secs >= PV_RE_EVAL_MIN_STEP_SECS {
                // The actors' pv argument is W (SI), the port's generation
                // read with no conversion.
                let pv_w = -d.ports.electrical.generation_power_w;
                d.actor_dispatch_buf.clear();
                for actor in &mut d.actors {
                    let emitted_from = d.actor_dispatch_buf.len();
                    actor.adjust_for_pv(pv_w, &d.latest_env, &mut d.actor_dispatch_buf);
                    crate::actor::check_declared_signals(
                        actor.as_ref(),
                        &d.actor_dispatch_buf[emitted_from..],
                    )?;
                }
                if !d.actor_dispatch_buf.is_empty() {
                    for req in d.actor_dispatch_buf.drain(..) {
                        d.control_dispatcher.queue(req);
                    }
                    d.dispatch_pass()?;
                }
            }
        }

        Ok(())
    }
    /// The electrical equipment leg: Phase 2's Electrical-stage steps
    /// (battery, EV) with the revised signals.
    fn leg_electrical_equipment(&mut self, d: &mut Dwelling) -> Result<()> {
        // Phase 2: Electrical-stage equipment (Battery, EV).
        for _oi in 0..d.roster.equipment_execution_order.len() {
            let idx = d.roster.equipment_execution_order[_oi];
            let stage = d.equipment[idx].descriptor().stage;
            if stage != ExecutionStage::Electrical {
                continue;
            }
            #[cfg(feature = "observe")]
            #[cfg(test)]
            let pp_before = thread_allocations();
            #[cfg(feature = "observe")]
            let pre_ports = if self.observing {
                // The recycled path writes the pre-step capture straight
                // into the recycled observation's own port Vecs (the
                // capture must precede the step); a fresh observation (a
                // roster growth) builds its own capture to move in.
                if self.nonthermal_obs_used < self.nonthermal_obs.len() {
                    observer_capture::capture_ports_into(
                        &mut self.nonthermal_obs[self.nonthermal_obs_used].pre_step_ports,
                        &d.port_snapshot_buf,
                    );
                    None
                } else {
                    Some(observer_capture::capture_ports(&d.port_snapshot_buf))
                }
            } else {
                None
            };
            #[cfg(feature = "observe")]
            #[cfg(test)]
            d.observe_allocations
                .record(pp_before, ObserveAllocSite::EquipmentPrePorts);

            let _ = d.equipment[idx].update_control(&d.latest_env);
            if d.step_validation_enabled {
                d.rollback_ports.copy_into(&d.ports);
            }
            // Snapshot the shared electrical bus so the post-step delta is
            // exactly this equipment's contribution.
            let pre_electrical = d.ports.electrical;
            let pre_state_energy_j = d.equipment[idx].stored_energy_j();
            if let Err(err) = d.equipment[idx].step(&d.latest_env, self.dt, &mut d.ports) {
                d.record_equipment_step_failure(idx, err, pre_state_energy_j)?;
            } else {
                if d.step_validation_enabled {
                    d.validate_equipment_contract(idx)?;
                    validate_port_core_electrical_consistency(
                        d.equipment[idx].descriptor(),
                        d.equipment[idx].core_output(),
                        pre_electrical,
                        &d.ports.electrical,
                    )?;
                }
                d.record_equipment_step_success(idx);
                d.stepped_stage_ranks.push(stage_rank(stage));
            }

            #[cfg(feature = "observe")]
            {
                tracing::debug!(
                    stage = ?stage,
                    equipment = %d.equipment[idx].descriptor().name,
                    "equipment step executed"
                );
                if self.observing {
                    let contribution =
                        observer_capture::diff_ports(&d.port_snapshot_buf, &d.ports);
                    if self.nonthermal_obs_used < self.nonthermal_obs.len() {
                        observer_capture::capture_single_equipment_into(
                            &mut self.nonthermal_obs[self.nonthermal_obs_used],
                            d.equipment[idx].as_ref(),
                            contribution,
                            None,
                        );
                    } else {
                        self.nonthermal_obs.push(observer_capture::capture_single_equipment(
                            d.equipment[idx].as_ref(),
                            contribution,
                            pre_ports.expect("a fresh observation carries its pre-step capture"),
                        ));
                    }
                    self.nonthermal_obs_used += 1;
                    #[cfg(test)]
                    let ps_before = thread_allocations();
                    d.port_snapshot_buf.copy_into(&d.ports);
                    #[cfg(test)]
                    d.observe_allocations
                        .record(ps_before, ObserveAllocSite::PortSnapshotBuf);
                }
            }
        }

        Ok(())
    }
    /// The thermal equipment leg: Phase 3's Thermal-stage steps (HVAC,
    /// water heaters, ventilation), the observer captures' merge, the
    /// stage-ordering and telemetry invariant checks, the health and
    /// warning drains, the clamp census, and Step 3c's ventilation
    /// propagation into the thermal solver's config.
    #[allow(clippy::too_many_lines)]
    fn leg_thermal_equipment(&mut self, d: &mut Dwelling) -> Result<()> {
        // Phase 3: Thermal-stage equipment (HVAC, water heaters, ventilation).
        // update_control() already ran in Step 2a after control dispatch, so
        // only .step() is called here. Thermal equipment commits heating/cooling
        // power to ports with full visibility of same-step PV generation and
        // storage dispatch from Phases 1–2.
        for _oi in 0..d.roster.equipment_execution_order.len() {
            let idx = d.roster.equipment_execution_order[_oi];
            if d.equipment[idx].descriptor().stage != ExecutionStage::Thermal {
                continue;
            }
            #[cfg(feature = "observe")]
            #[cfg(test)]
            let pp_before = thread_allocations();
            #[cfg(feature = "observe")]
            let pre_ports = if self.observing {
                // The recycled path writes the pre-step capture straight
                // into the recycled observation's own port Vecs (the
                // capture must precede the step); a fresh observation (a
                // roster growth) builds its own capture to move in.
                if self.thermal_obs_used < self.thermal_obs.len() {
                    observer_capture::capture_ports_into(
                        &mut self.thermal_obs[self.thermal_obs_used].pre_step_ports,
                        &d.port_snapshot_buf,
                    );
                    None
                } else {
                    Some(observer_capture::capture_ports(&d.port_snapshot_buf))
                }
            } else {
                None
            };
            #[cfg(feature = "observe")]
            #[cfg(test)]
            d.observe_allocations
                .record(pp_before, ObserveAllocSite::EquipmentPrePorts);

            if d.step_validation_enabled {
                d.rollback_ports.copy_into(&d.ports);
            }
            // Snapshot the shared electrical bus so the post-step delta is
            // exactly this equipment's contribution.
            let pre_electrical = d.ports.electrical;
            let pre_state_energy_j = d.equipment[idx].stored_energy_j();
            if let Err(err) = d.equipment[idx].step(&d.latest_env, self.dt, &mut d.ports) {
                d.record_equipment_step_failure(idx, err, pre_state_energy_j)?;
            } else {
                if d.step_validation_enabled {
                    d.validate_equipment_contract(idx)?;
                    validate_port_core_electrical_consistency(
                        d.equipment[idx].descriptor(),
                        d.equipment[idx].core_output(),
                        pre_electrical,
                        &d.ports.electrical,
                    )?;
                }
                d.record_equipment_step_success(idx);
                d.stepped_stage_ranks
                    .push(stage_rank(ExecutionStage::Thermal));
            }

            #[cfg(feature = "observe")]
            {
                tracing::debug!(
                    stage = ?ExecutionStage::Thermal,
                    equipment = %d.equipment[idx].descriptor().name,
                    "equipment step executed"
                );
                if self.observing {
                    let contribution =
                        observer_capture::diff_ports(&d.port_snapshot_buf, &d.ports);
                    if self.thermal_obs_used < self.thermal_obs.len() {
                        observer_capture::capture_single_equipment_into(
                            &mut self.thermal_obs[self.thermal_obs_used],
                            d.equipment[idx].as_ref(),
                            contribution,
                            None,
                        );
                    } else {
                        self.thermal_obs.push(observer_capture::capture_single_equipment(
                            d.equipment[idx].as_ref(),
                            contribution,
                            pre_ports.expect("a fresh observation carries its pre-step capture"),
                        ));
                    }
                    self.thermal_obs_used += 1;
                    #[cfg(test)]
                    let ps_before = thread_allocations();
                    d.port_snapshot_buf.copy_into(&d.ports);
                    #[cfg(test)]
                    d.observe_allocations
                        .record(ps_before, ObserveAllocSite::PortSnapshotBuf);
                }
            }
        }

        // Observer captures: nonthermal first (Independent + Electrical ran
        // first), then thermal (Thermal ran last). This reverses the prior
        // capture order to match the new execution order. The recycled
        // elements the step did not reach (a roster shrink) truncate away;
        // the reached ones moved through their in-place updates.
        #[cfg(feature = "observe")]
        if self.observing {
            self.nonthermal_obs.truncate(self.nonthermal_obs_used);
            self.thermal_obs.truncate(self.thermal_obs_used);
            // The phase captures' port snapshots refill the recycled
            // Vecs in place; the observation lists move in.
            #[cfg(test)]
            let phase_before = thread_allocations();
            observer_capture::capture_ports_into(&mut self.nonthermal_phase_ports, &d.ports);
            self.obs_phases.post_nonthermal_equipment = Some(EquipmentPhaseCapture {
                equipment: std::mem::take(&mut self.nonthermal_obs),
                ports: std::mem::take(&mut self.nonthermal_phase_ports),
            });
            observer_capture::capture_ports_into(&mut self.thermal_phase_ports, &d.ports);
            self.obs_phases.post_thermal_equipment = Some(EquipmentPhaseCapture {
                equipment: std::mem::take(&mut self.thermal_obs),
                ports: std::mem::take(&mut self.thermal_phase_ports),
            });
            #[cfg(test)]
            d.observe_allocations
                .record(phase_before, ObserveAllocSite::PhaseCaptures);
        }

        // Stage-ordering invariant: verify that equipment step execution order
        // respects stage_rank ordering (non-decreasing ranks). The slice is
        // this step's own ranks only: the persistent scratch was cleared
        // before the phase's first push and nothing else writes it.
        d.invariant_checker
            .check_equipment_step_order(&d.stepped_stage_ranks)?;

        // Non-finite telemetry enforcement (always on): equipment
        // writes are rejected at the Telemetry boundary in every build, and
        // the first rejection is latched; ANY latched map (failed-and-
        // rolled-back equipment included: the port rollback does not undo
        // the telemetry map, and the corruption that failed the step is
        // exactly what produced the write) turns the step into a run
        // failure naming the equipment and the key.
        for eq in &d.equipment {
            if let Some((key, value)) = eq.telemetry().non_finite_latch() {
                return Err(HaresError::NanDetected {
                    step_index: d.clock.current_step(),
                    zone_id: None,
                    value_name: format!(
                        "telemetry key '{key}' on equipment '{}' (rejected value {value})",
                        eq.descriptor().name
                    ),
                });
            }
        }

        // Unknown telemetry keys (always on): a `set` on a key the
        // equipment never registered is a wiring fault. The first one is
        // latched by the telemetry map, and ANY latch (failed-and-rolled-
        // back equipment included, mirroring the non-finite check above)
        // turns the step into a run failure naming the equipment and the
        // key.
        for eq in &d.equipment {
            if let Some(key) = eq.telemetry().unknown_key_latch() {
                return Err(HaresError::Equipment(format!(
                    "telemetry key '{key}' was never registered on equipment \
                     '{}' (register telemetry keys via insert() at init)",
                    eq.descriptor().name
                )));
            }
        }

        // Actor telemetry latches (always on): an actor's telemetry is
        // built and sealed from its declarations, so a non-finite write or
        // an undeclared key (set or insert) is a wiring fault; ANY latch
        // fails the step with InvariantViolation naming the actor and the
        // key.
        for actor in &d.actors {
            let Some(tel) = actor.telemetry() else {
                continue;
            };
            if let Some((key, value)) = tel.non_finite_latch() {
                return Err(HaresError::InvariantViolation {
                    check_name: format!(
                        "actor_telemetry: non-finite value {value} rejected for key '{key}' \
                         on actor '{}'",
                        actor.name()
                    ),
                    value: *value,
                    tolerance: 0.0,
                });
            }
            if let Some(key) = tel.unknown_key_latch() {
                return Err(HaresError::InvariantViolation {
                    check_name: format!(
                        "actor_telemetry: key '{key}' is not declared on actor '{}' \
                         (declare it in output_variables)",
                        actor.name()
                    ),
                    value: 0.0,
                    tolerance: 0.0,
                });
            }
        }

        // Health counters: once per timestep, after the equipment step
        // loops complete, drain each equipment's health counters into the
        // run total. Failed-and-rolled-back equipment are skipped for the
        // counters: take-and-discard would lose nothing (their counts
        // surface on the equipment's next successful step, take semantics
        // reset per take), and their step already failed the port contract.
        let mut step_fluid_payload_absences: u64 = 0;
        let mut step_cross_tier_deadband_events: u64 = 0;
        let mut step_ideal_capacity_unsolved_steps: u64 = 0;
        // The equipment whose curve-index clamps drained this step, with
        // each one's count: the policy is evaluated per source, so a limit
        // set on the counter fails the run naming the equipment.
        let mut curve_clamp_sources: Vec<(String, u64)> = Vec::new();
        for (idx, eq) in d.equipment.iter_mut().enumerate() {
            if !d.step_failed[idx] {
                let counts = eq.take_health_counts();
                if counts.curve_index_clamps > 0 {
                    curve_clamp_sources
                        .push((eq.descriptor().name.clone(), counts.curve_index_clamps));
                }
                step_fluid_payload_absences += counts.fluid_payload_absences;
                step_cross_tier_deadband_events += counts.cross_tier_deadband_events;
                step_ideal_capacity_unsolved_steps += counts.ideal_capacity_unsolved_steps;
            }
            // Warnings raised this step drain beside the health counts,
            // from failed-and-rolled-back equipment too (warnings only, no
            // health counts): their warnings would otherwise sit undrained
            // until the equipment's next successful step, stamped with the
            // later step, or be lost entirely if the equipment never
            // succeeds. The drain happens in the step the warnings fired
            // in, so the stamp the dwelling adds is the step they belong
            // to.
            eq.drain_warnings(&mut d.warning_scratch);
            for mut warning in d.warning_scratch.drain(..) {
                warning.step_index = Some(d.clock.current_step);
                d.warnings.push_warning(warning);
            }
        }
        for (source, added) in curve_clamp_sources {
            d.health.curve_index_clamps += added;
            d.escalate_on_limit(
                HealthEvent::CurveIndexClamp,
                d.health.curve_index_clamps,
                &source,
            )?;
        }
        d.health.fluid_payload_absences += step_fluid_payload_absences;
        // The cross-tier deadband collisions: the run's first raised the
        // equipment's warning beside this drain; the count totals every
        // collision the run hit, with no policy limit (the event escalates
        // nothing: the warning is the signal).
        d.health.cross_tier_deadband_events += step_cross_tier_deadband_events;
        d.health.ideal_capacity_unsolved_steps += step_ideal_capacity_unsolved_steps;

        // The clamp census: once per timestep, after the physics and
        // equipment steps complete, drain the recording thread's per-step
        // tally into the run totals, the same take-and-reset the
        // equipment health counts above use. Diagnostics only: no policy
        // limit and no warning ride the census; the counts surface
        // through the dwelling's `clamp_census` accessor, never through
        // the health totals the frames serialize.
        #[cfg(feature = "observe")]
        {
            let step_clamps = hares_types::clamp_census::take();
            if !step_clamps.is_empty() {
                d.clamp_census.merge(&step_clamps);
            }
        }

        // Step 3c: propagate per-timestep ventilation recovery effectiveness
        // and forced flow from equipment to the thermal solver config.  The
        // Ventilation equipment computes effective sensible/latent
        // effectiveness accounting for bypass and defrost derating, and its
        // effective forced flow (the rated flow times the schedule
        // fraction); the thermal solver's infiltration module must see
        // these dynamic values (not the static rated ones) to correctly
        // compute the ventilation sensible/latent load and its flow.
        //
        // EnergyPlus Engineering Reference, "Heat Exchangers" chapter:
        // per-timestep effectiveness application with bypass suspending heat
        // transfer (effectiveness = 0).  Using stale rated values during bypass
        // underestimates the ventilation load by a factor equal to
        // 1 / (1 - rated_eff), e.g. 4× for a 75% effective HRV. The forced
        // flow is the same one-source rule: a fan scheduled to 12 of 24
        // hours moves air 12 hours, and its envelope load follows.
        for eq in &d.equipment {
            if let Some((eff_s, eff_l)) = eq.effective_ventilation_effectiveness() {
                d.thermal_solver
                    .set_ventilation_recovery(eff_s, eff_l)
                    .map_err(|err| {
                        HaresError::Envelope(format!(
                            "ventilation recovery from '{}' rejected: {err}",
                            eq.descriptor().name
                        ))
                    })?;
                #[cfg(feature = "observe")]
                tracing::debug!(
                    equipment = %eq.descriptor().name,
                    mode = ?eq.core_output().state.operating_mode,
                    eff_s,
                    eff_l,
                    "ventilation effectiveness propagated to thermal solver config"
                );
            }
            if let Some(flow_m3_s) = eq.effective_ventilation_flow_m3_s() {
                d.thermal_solver
                    .set_ventilation_flow(flow_m3_s)
                    .map_err(|err| {
                        HaresError::Envelope(format!(
                            "ventilation flow from '{}' rejected: {err}",
                            eq.descriptor().name
                        ))
                    })?;
                #[cfg(feature = "observe")]
                tracing::debug!(
                    equipment = %eq.descriptor().name,
                    mode = ?eq.core_output().state.operating_mode,
                    flow_m3_s,
                    "ventilation flow propagated to thermal solver config"
                );
            }
        }

        Ok(())
    }
    /// The envelope and custom solvers leg: Step 4's thermal integrate
    /// and zone apply, the humidity, electrical and fluid solves, the
    /// custom domain solvers, the BMS PV-staleness diagnostic, and the
    /// observers' solver captures.
    #[allow(clippy::too_many_lines)]
    fn leg_envelope_and_custom_solvers(&mut self, d: &mut Dwelling) -> Result<()> {

        // Step 4: envelope/domain resolution.
        d.thermal_solver.integrate(
            &d.ports,
            &d.latest_env,
            &mut d.thermal_update_buf,
        )?;

        // Apply zone temps and capture observer data while we still have the borrow.
        apply_thermal_update_to_zones(&mut d.latest_env, &d.thermal_update_buf);

        // Write the thermal domain's slot so humidity/electrical solvers see
        // the updated state. Zone temperatures are already applied above; the
        // slot write copies the full DomainUpdate in place for solvers that
        // read it (humidity).
        #[cfg(test)]
        let thermal_alloc_before = thread_allocations();
        d.latest_env
            .domains
            .thermal
            .set_from(&d.thermal_update_buf);
        #[cfg(test)]
        d.record_snapshot_allocations(thermal_alloc_before, SnapshotAllocSite::ThermalSetFrom);

        // Capture the pre-resolve (committed) humidity ratios so the
        // always-on moisture balance check can diff this step's solver
        // output against them (recorded in every build profile).
        d.prev_humidity_ratios.clear();
        d.prev_humidity_ratios.extend(
            d.humidity_solver
                .humidity_ratios
                .iter()
                .map(|(&z, &w)| (z, w)),
        );

        d.humidity_solver.resolve(
            &d.ports,
            &d.latest_env,
            self.dt,
            &mut d.humidity_update_buf,
        )?;
        d.electrical_solver.resolve(
            &d.ports,
            &d.latest_env,
            self.dt,
            &mut d.electrical_update_buf,
        )?;
        d.fluid_solver.resolve(
            &d.ports,
            &d.latest_env,
            self.dt,
            &mut d.fluid_update_buf,
        )?;

        #[cfg(test)]
        let humidity_alpha_before = thread_allocations();
        apply_humidity_update_to_zones(
            &mut d.latest_env,
            &d.humidity_update_buf,
            &mut d.humidity_alpha_key_scratch,
        );
        #[cfg(test)]
        d.record_snapshot_allocations(
            humidity_alpha_before,
            SnapshotAllocSite::HumidityAlphaRebuild,
        );

        // BMS PV staleness diagnostic: after the electrical solver runs at
        // Step 4 and the timestep is ≥ 5 min, compare the PV generation used
        // in the BMS decision (bms_pv_stale_kw) against the actual PV
        // generation from the current step (bms_pv_actual_kw). If the
        // re-evaluation pass ran, both telemetry fields are populated and
        // the difference is the stale-data error from the one-step ordering.
        {
            if d.latest_env.time_step_secs() >= PV_RE_EVAL_MIN_STEP_SECS {
                for actor in &d.actors {
                    let Some(tel) = actor.telemetry() else {
                        continue;
                    };
                    let stale = tel.get("bms_pv_stale_kw").unwrap_or(0.0);
                    let actual = tel.get("bms_pv_actual_kw").unwrap_or(0.0);
                    let diff = (actual - stale).abs();
                    // Emit diagnostic when the stale-data error exceeds a
                    // configurable fraction of the larger PV value (default 5%).
                    let max_pv = stale.max(actual).max(0.01); // avoid div-by-zero
                    if diff > 0.05 * max_pv {
                        tracing::debug!(
                            actor = actor.name(),
                            bms_pv_stale_kw = stale,
                            bms_pv_actual_kw = actual,
                            diff_kw = diff,
                            pct = diff / max_pv * 100.0,
                            "BMS PV staleness: step-ordering data gap exceeds 5% of PV",
                        );
                    }
                }
            }
        }

        #[cfg(feature = "observe")]
        if d.observer_buf.is_some() {
            let zip_scale = d
                .electrical_solver
                .effective_load_scale(&d.latest_env.grid);
            let port_load_raw_w = d.ports.electrical.load_power_w;
            let port_load_adj_w = port_load_raw_w * zip_scale;
            let port_net_w = port_load_adj_w + d.ports.electrical.generation_power_w;
            let residual = (d.electrical_solver.net_active_w() - port_net_w).abs();
            // The thermal-update buffer's capture clone, deferred to the
            // capture site (the census's A2): the buffer's content is
            // unchanged between the zone apply above and here (the
            // humidity, electrical and fluid solves write their own
            // buffers), so the deferred clone carries the same values,
            // and a step whose observer is never enabled pays nothing.
            // The recycled slot's own solver capture refilled in place
            // (a fresh capture only before the slot first carries one):
            // the four domain updates go through their in-place
            // clone_from and the envelope gains through copy_in_place,
            // so the fill allocates nothing.
            #[cfg(test)]
            let solver_before = thread_allocations();
            match self.obs_phases.post_solvers.as_mut() {
                Some(capture) => {
                    observer_capture::capture_solvers_into(
                        capture,
                        &d.thermal_update_buf,
                        &d.humidity_update_buf,
                        &d.electrical_update_buf,
                        &d.fluid_update_buf,
                        &d.thermal_solver,
                        observer_capture::PortBalance {
                            zip_load_scale: zip_scale,
                            port_load_raw_w,
                            port_load_adjusted_w: port_load_adj_w,
                            residual_w: residual,
                        },
                    );
                }
                None => {
                    self.obs_phases.post_solvers = Some(observer_capture::capture_solvers(
                        &d.thermal_update_buf,
                        &d.humidity_update_buf,
                        &d.electrical_update_buf,
                        &d.fluid_update_buf,
                        &d.thermal_solver,
                        observer_capture::PortBalance {
                            zip_load_scale: zip_scale,
                            port_load_raw_w,
                            port_load_adjusted_w: port_load_adj_w,
                            residual_w: residual,
                        },
                    ));
                }
            }
            #[cfg(test)]
            d.observe_allocations
                .record(solver_before, ObserveAllocSite::SolverCapture);
        }

        #[cfg(test)]
        let humidity_alloc_before = thread_allocations();
        d.latest_env
            .domains
            .humidity
            .set_from(&d.humidity_update_buf);
        #[cfg(test)]
        d.record_snapshot_allocations(humidity_alloc_before, SnapshotAllocSite::HumiditySetFrom);
        #[cfg(test)]
        let electrical_alloc_before = thread_allocations();
        d.latest_env
            .domains
            .electrical
            .set_from(&d.electrical_update_buf);
        #[cfg(test)]
        d.record_snapshot_allocations(
            electrical_alloc_before,
            SnapshotAllocSite::ElectricalSetFrom,
        );
        #[cfg(test)]
        let fluid_alloc_before = thread_allocations();
        d.latest_env
            .domains
            .fluid
            .set_from(&d.fluid_update_buf);
        #[cfg(test)]
        d.record_snapshot_allocations(fluid_alloc_before, SnapshotAllocSite::FluidSetFrom);

        #[cfg(test)]
        let mut custom_alloc_delta = 0u64;
        for (i, solver) in d.custom_domain_solvers.iter_mut().enumerate() {
            solver.resolve(
                &d.ports,
                &d.latest_env,
                self.dt,
                &mut d.custom_update_bufs[i],
            )?;
            #[cfg(test)]
            let alloc_before = thread_allocations();
            d.latest_env
                .domains
                .set_custom(d.custom_solver_slots[i], &d.custom_update_bufs[i]);
            #[cfg(test)]
            {
                if let (Some(before), Some(after)) = (alloc_before, thread_allocations()) {
                    custom_alloc_delta += after.saturating_sub(before);
                }
            }
        }
        #[cfg(test)]
        {
            d.snapshot_allocations.custom_set_from += custom_alloc_delta;
        }

        #[cfg(feature = "observe")]
        if d.observer_buf.is_some() {
            let capture = observer_capture::capture_custom_solvers(&d.custom_domain_solvers);
            debug_assert_eq!(
                capture.solvers.len(),
                d.custom_domain_solvers.len(),
                "custom solver capture count mismatch"
            );
            self.obs_phases.post_custom_solvers = Some(capture);
        }

        Ok(())
    }
    /// The invariants and state snapshot leg: the always-on electrical
    /// and zone-temperature invariant checks, `check_step_invariants`,
    /// the observer's step snapshot push and diagnostic accumulation, the
    /// equipment-mode interest baseline, and the end-of-step equipment
    /// snapshot that fills the step result.
    #[allow(clippy::too_many_lines)]
    fn leg_invariants_and_state_snapshot(
        &mut self,
        d: &mut Dwelling,
        out: &mut StepResult,
    ) -> Result<()> {

        // Always-on invariant checks — unconditional in all build configurations.
        // These catch unrecoverable data corruption (NaN/Inf) in the two domains
        // where silent propagation would corrupt output records and downstream
        // metrics. O(n_zones + O(1)) per step — cheap enough to run every timestep.

        // Electrical finiteness: NaN or Inf in the electrical solver output is
        // unrecoverable data corruption. The check mirrors the one inside the
        // the always-on check_step_invariants() so that plain release builds
        // still catch it.
        {
            let net_kw = d.electrical_solver.net_active_w();
            if !net_kw.is_finite() {
                tracing::error!(
                    electrical_net_kw = net_kw,
                    "electrical solver output is non-finite — quarantining dwelling"
                );
                return Err(HaresError::InvariantViolation {
                    check_name: "electrical_net_finite".to_string(),
                    value: net_kw,
                    tolerance: 0.0,
                });
            }
            let net_kvar = d.electrical_solver.net_reactive_var();
            if !net_kvar.is_finite() {
                tracing::error!(
                    electrical_net_kvar = net_kvar,
                    "electrical solver reactive output is non-finite — quarantining dwelling"
                );
                return Err(HaresError::InvariantViolation {
                    check_name: "electrical_net_finite".to_string(),
                    value: net_kvar,
                    tolerance: 0.0,
                });
            }

            // Islanded-imbalance accounting (honest accounting, not fake
            // physics): while islanded there is no service connection, so any
            // residual net flow the electrical solver reports at the "grid"
            // channel is a power-balance violation inside the island —
            // positive = load the island sources failed to cover (would-be
            // phantom import), negative = surplus generation the island could
            // not absorb (would-be phantom export). Both are 0.0 during
            // normal (grid-connected) operation. See docs/outage-behavior.md.
            if d.latest_env.grid.islanded() {
                d.island_unserved_w = net_kw.max(0.0);
                d.island_excess_w = (-net_kw).max(0.0);
                if d.island_unserved_w > ISLAND_UNSERVED_WARN_THRESHOLD_KW {
                    // Per-run warn-once: the first occurrence records the
                    // scenario warning in the run's warning log (every
                    // build) and logs at warn level; unserved load recurs
                    // every islanded step, so later occurrences log at
                    // debug level. The gate is the dwelling's own, so the
                    // next run (a dwelling rebuilt from its blueprint)
                    // warns again.
                    if !d.island_unserved_warned {
                        d.island_unserved_warned = true;
                        tracing::warn!(
                            step = d.clock.current_step(),
                            island_unserved_w = d.island_unserved_w,
                            "islanded operation: on-site sources are not covering the load \
                             (unserved load; subsequent occurrences logged at debug level)"
                        );
                        d.warnings.push_from(
                            "island",
                            format!(
                                "islanded operation: on-site sources are not covering the load \
                                 (unserved load {} kW; subsequent occurrences logged at debug \
                                 level)",
                                d.island_unserved_w
                            ),
                        );
                    } else {
                        tracing::debug!(
                            step = d.clock.current_step(),
                            island_unserved_w = d.island_unserved_w,
                            "islanded operation: unserved load"
                        );
                    }
                }
            } else {
                d.island_unserved_w = 0.0;
                d.island_excess_w = 0.0;
            }
        }

        // Zone temperature NaN: silently propagating a NaN zone temperature
        // corrupts output recording, equipment control, and downstream metrics
        // for the remainder of the simulation. EVERY zone is checked,
        // warm-up included: there is no warm-up tolerance, because the run
        // must fail loudly instead of propagating a corrupted initial state;
        // and every non-finite zone is logged before quarantining.
        {
            let mut any_nan = false;
            for zone in &d.latest_env.zones {
                if !zone.temperature_c.is_finite() {
                    any_nan = true;
                    tracing::error!(
                        zone_id = %zone.id,
                        temperature_c = zone.temperature_c,
                        "zone temperature is NaN; quarantining dwelling"
                    );
                }
            }
            if any_nan {
                return Err(HaresError::InvariantViolation {
                    check_name: "zone_temperature_nan".to_string(),
                    value: f64::NAN,
                    tolerance: 0.0,
                });
            }
        }

        d.check_step_invariants(self.dt)?;

        // End of the `invariants` phase; the per-step telemetry snapshot
        // (`state_snapshot` phase) follows.
        #[cfg(feature = "profiling")]
        self.phase_clock.enter(&mut d.profiling, ProfilePhase::StateSnapshot);
        #[cfg(test)]
        let state_snapshot_alloc_before = thread_allocations();

        // Push observer step snapshot after invariant checks so moisture invariant
        // capture data (populated in check_step_invariants) is available for the snapshot.
        #[cfg(feature = "observe")]
        if let Some(buf) = &mut d.observer_buf {
            // The recycled slot's own zone-update capture refilled in
            // place (a fresh capture only before the slot first carries
            // one): the two Vecs reuse their capacities.
            #[cfg(test)]
            let zone_before = thread_allocations();
            match self.obs_phases.post_zone_update.as_mut() {
                Some(capture) => {
                    observer_capture::capture_zone_update_into(capture, &d.latest_env)
                }
                None => {
                    self.obs_phases.post_zone_update =
                        Some(observer_capture::capture_zone_update(&d.latest_env));
                }
            }
            #[cfg(test)]
            d.observe_allocations
                .record(zone_before, ObserveAllocSite::ZoneUpdateCapture);
            #[cfg(all(feature = "observe", debug_assertions))]
            let moisture_invariant = if d.invariant_moisture_capture.is_empty() {
                None
            } else {
                Some(MoistureInvariantCapture {
                    zones: std::mem::take(&mut d.invariant_moisture_capture),
                })
            };
            #[cfg(not(all(feature = "observe", debug_assertions)))]
            let moisture_invariant: Option<MoistureInvariantCapture> = None;
            // The recycled slot's scalars are rewritten here; its phase
            // captures were taken at the step start and (for the
            // equipment phases) written in place, so the slot carries
            // this step's data with its old buffers.
            let mut slot = self.obs_slot
                .take()
                .expect("the step's slot was taken at the step start");
            slot.step_index = d.clock.current_step();
            slot.timestamp = d.latest_env.current_time;
            slot.phases = std::mem::take(&mut self.obs_phases);
            slot.actor_skips = self.actor_skips;
            slot.actor_calls = self.actor_calls;
            slot.actor_error_count = self.actor_error_count;
            slot.moisture_invariant = moisture_invariant;
            slot.time_sin = {
                let fhour = d.latest_env.current_time.hour() as f64
                    + d.latest_env.current_time.minute() as f64 / 60.0
                    + d.latest_env.current_time.second() as f64 / 3600.0;
                libm::sin(2.0 * std::f64::consts::PI * fhour / 24.0)
            };
            slot.time_cos = {
                let fhour = d.latest_env.current_time.hour() as f64
                    + d.latest_env.current_time.minute() as f64 / 60.0
                    + d.latest_env.current_time.second() as f64 / 3600.0;
                libm::cos(2.0 * std::f64::consts::PI * fhour / 24.0)
            };
            // The actor telemetry keys: each actor's frozen key list is
            // cached as an Arc at the actor's first observed push (the
            // key set is sealed at the telemetry's construction; an
            // unsealed telemetry never caches) and cloned as an Arc per
            // step, collected into the recycled slot's own Vec (cleared:
            // the capacity stays).
            #[cfg(test)]
            let atk_before = thread_allocations();
            slot.actor_telemetry_keys.clear();
            if d.actor_key_cache.len() != d.actors.len() {
                d.actor_key_cache.clear();
                d.actor_key_cache.resize(d.actors.len(), None);
            }
            for (i, actor) in d.actors.iter().enumerate() {
                let Some(tel) = actor.telemetry() else {
                    continue;
                };
                if let Some(cached) = &d.actor_key_cache[i] {
                    slot.actor_telemetry_keys.push(Arc::clone(cached));
                    continue;
                }
                let mut keys: Vec<String> = tel.0.keys().cloned().collect();
                keys.sort();
                let entry = Arc::new((actor.name().to_string(), keys));
                if tel.is_sealed() {
                    d.actor_key_cache[i] = Some(Arc::clone(&entry));
                }
                slot.actor_telemetry_keys.push(entry);
            }
            #[cfg(test)]
            d.observe_allocations
                .record(atk_before, ObserveAllocSite::ActorTelemetryKeys);
            buf.push(slot);
        }

        // Record per-step diagnostic data for post-hoc checks (unmet hours,
        // short-cycling, freezing, simultaneous heating/cooling).  Runs when
        // the observer is active so check data is available for end-of-run
        // analysis.
        #[cfg(feature = "observe")]
        if let Some(accum) = &mut d.diagnostic_accum {
            accum.record_from_state(&d.latest_env.zones, &d.equipment);
        }

        // Equipment-mode interest baseline: capture the snapshot generation
        // the actors just observed — the map as it stands BEFORE this step's
        // end-of-step population overwrites it — so the next step's
        // `EquipmentModeChange` comparison sees two distinct generations
        // (current = this step's snapshot, prev = the one before). The
        // previous shape captured AFTER the population, storing the new
        // snapshot itself: the comparison then read the map against a copy
        // of itself and the interest never fired on any real transition —
        // a registered interest whose trigger never fires is the "silence
        // is not a decision" failure in its purest form. The zone-temperature
        // interests keep the same one-generation-behind contract via the
        // `prior_zone_temps`/`prev_zone_temps` shift register below;
        // equipment modes need only the single previous generation, so a
        // pre-population capture is the whole mechanism. Mid-run adds still
        // fire on first observation: the identity refresh seeds the joining
        // equipment's `equipment_core` entry between steps, after this
        // capture, so the joining step's comparison reads `Some(mode)`
        // against a `prev` that lacks the id.
        d.prev_equipment_modes.clear();
        d.prev_equipment_modes.extend(
            d.latest_env
                .equipment_core
                .iter()
                .map(|(&id, co)| (id, co.state.operating_mode)),
        );

        // Snapshot end-of-timestep equipment state into latest_env so that the
        // NEXT step's actors see the freshest committed state for every
        // equipment that stepped this timestep. Snapshot runs AFTER all
        // equipment phases (Independent + Electrical + Thermal in Step 3) so
        // nothing is one step stale.
        #[cfg(test)]
        let equipment_alloc_before = thread_allocations();
        d.latest_env
            .equipment_core
            .retain(|id, _| d.active_equipment_ids.contains(id));
        d.latest_env.equipment_core.reserve(d.equipment.len());
        d.latest_env.equipment_telemetry.retain(|name, _| {
            name == hares_types::telemetry_keys::HUMIDITY_SOLVER_TELEMETRY_KEY
                || d.equipment_id_by_name.contains_key(name)
        });
        d.latest_env
            .equipment_telemetry
            .reserve(d.equipment.len());
        for idx in 0..d.equipment.len() {
            if d.step_failed[idx] {
                continue;
            }
            d.snapshot_equipment_entry(idx);
        }
        #[cfg(test)]
        d.record_snapshot_allocations(
            equipment_alloc_before,
            SnapshotAllocSite::SnapshotEquipment,
        );

        for (i, entry) in d.roster.zone_temp_scratch.iter_mut().enumerate() {
            entry.1 = if let Some(env_idx) = d.roster.recording.zones.zone_env_indices[i] {
                d.latest_env.zones[env_idx].temperature_c
            } else {
                // Zone not found in environment -- should not happen in a correctly
                // built dwelling. Use previous value (initialized to 0.0 at
                // construction, updated each step when the zone is present).
                entry.1
            };
        }

        // HVAC thermal delivery: use component_gains which includes both equipment
        // port contributions and ideal HVAC loads from the thermal solver.
        // Sign convention (EnvelopeComponentGains): positive = heat into zone.
        // cooling_w is negative when cooling (heat removed); .abs() converts to
        // delivered-energy magnitude for the StepResult/public API.
        let gains = d.thermal_solver.component_gains();
        let hvac_heating_w = gains.hvac_heating_w;
        let hvac_cooling_w = gains.hvac_cooling_w.abs();
        // Stash the step's HVAC delivery (delivered-energy magnitudes) for
        // the warm-up loop's daily peaks; see `step_hvac_heating_w`.
        d.step_hvac_heating_w = hvac_heating_w;
        d.step_hvac_cooling_w = hvac_cooling_w;
        // The per-zone delivery, the per-zone columns' own values: the
        // thermal port accumulators still hold the step's equipment
        // contributions (ports.zero() runs after record_step). The
        // vectors are reused: `clear` keeps their capacity, so a
        // steady-state step allocates nothing here.
        d.step_hvac_heating_by_zone.clear();
        d.step_hvac_heating_by_zone
            .extend(d.ports.thermal.iter().map(|thermal| {
                (
                    thermal.zone,
                    thermal.sensible_for_category(ThermalCategory::HvacHeating),
                )
            }));
        d.step_hvac_cooling_by_zone.clear();
        d.step_hvac_cooling_by_zone
            .extend(d.ports.thermal.iter().map(|thermal| {
                (
                    thermal.zone,
                    thermal
                        .sensible_for_category(ThermalCategory::HvacCooling)
                        .abs(),
                )
            }));
        if hvac_heating_w < 0.0 {
            tracing::warn!(
                hvac_heating_w = hvac_heating_w,
                "negative HVAC heating delivered — sign error in equipment \
                 port contributions or thermal solver gain inversion"
            );
        }

        let gas_power_w = d.ports.fuel.get(hares_types::FuelType::Gas);

        // The step result written in place: the zone temperatures copy
        // through `clone_from`, which reuses the buffer's allocation
        // once it holds the zone count, so a reused result allocates
        // nothing here after the first step.
        out.timestamp = d.latest_env.current_time.naive_local();
        out.net_electric_power_w = d.electrical_solver.net_active_w();
        out.zone_temperatures_c
            .clone_from(&d.roster.zone_temp_scratch);
        out.hvac_heating_w = hvac_heating_w;
        out.hvac_cooling_w = hvac_cooling_w;
        out.gas_power_w = gas_power_w;
        #[cfg(test)]
        d.record_snapshot_allocations(
            state_snapshot_alloc_before,
            SnapshotAllocSite::StateSnapshotPhase,
        );

        Ok(())
    }
    /// The output and accounting leg: Step 5's output recording (whose
    /// callback error latches the run's end), the hot-path allocation
    /// accounting, the billing step and electrical summary capture, the
    /// interest-filter shift registers, the consistency checks,
    /// `ports.zero`, and the clock's advance.
    #[allow(clippy::too_many_lines)]
    fn leg_output_and_accounting(
        &mut self,
        d: &mut Dwelling,
        record_output: bool,
        out: &mut StepResult,
    ) -> Result<()> {

        // Step 5: record outputs to disk (when enabled) and accumulate step results.
        if record_output && let Err(err) = d.record_step(out) {
            // A callback error ends the run the way the failure budget
            // does: latched, so every later step returns it without
            // stepping anything.
            if matches!(err, HaresError::Callback { .. }) {
                d.terminal_error = Some(err.clone());
            }
            return Err(err);
        }
        // End of the `output` phase (Step 5).
        #[cfg(feature = "profiling")]
        self.phase_clock.enter(&mut d.profiling, ProfilePhase::Accounting);
        #[cfg(feature = "profiling")]
        {
            let alloc_after = thread_allocations();
            if let (Some(before), Some(after)) = (self.alloc_before, alloc_after) {
                // The counter only ever grows, so the delta is the step's
                // allocation count. Without the counting allocator both
                // reads are `None` and no field is touched. A violating
                // step is counted and reported, never asserted: steps
                // allocate today.
                let delta = after.saturating_sub(before);
                d.profiling.hot_path_allocations =
                    Some(d.profiling.hot_path_allocations.unwrap_or(0) + delta);
                d.profiling.hot_path_alloc_violations = Some(
                    d.profiling.hot_path_alloc_violations.unwrap_or(0) + u64::from(delta > 0),
                );
            }
        }

        // Post-solver: accumulate billing state from metered power.
        // The per-end-use signed flows are summed once: they feed both the
        // electrical summary captured below and, for the EV end use, the
        // tariff's EV sub-meter. A configured `ev_tou_period_name` bills
        // the EV's import at its own rate, so the evaluator needs the EV's
        // share of this step's net flow.
        let mut battery_w = 0.0;
        let mut ev_w = 0.0;
        let mut pv_w = 0.0;
        for (idx, eq) in d.equipment.iter().enumerate() {
            let end_use = &eq.descriptor().end_use;
            let Some(co) = d.delivered_output(idx) else {
                continue;
            };
            if *end_use == EndUse::BATTERY {
                battery_w += co.flows.electric_w.map_or(0.0, |e| e.signed_w());
            } else if *end_use == EndUse::EV {
                ev_w += co.flows.electric_w.map_or(0.0, |e| e.signed_w());
            } else if *end_use == EndUse::PV {
                pv_w += co.flows.electric_w.map_or(0.0, |e| e.signed_w());
            }
        }
        if let Some(ref mut evaluator) = d.tariff_evaluator {
            let net_kw = d.electrical_solver.net_active_w();
            let dt_secs = d.latest_env.time_step_secs();
            let tz = evaluator.simulation_start().timezone();
            // The evaluator's contract: step() receives the step's end
            // time, so the period closes on the fold of the step its
            // boundary ends, and the boundary step books to the period it
            // opens.
            let step_end =
                d.latest_env.current_time + Duration::milliseconds((dt_secs * 1000.0) as i64);
            let current_tz = step_end.with_timezone(&tz);
            if let Some(summary) = evaluator.step(net_kw, ev_w, dt_secs, current_tz)? {
                d.billing_summaries.push(summary);
            }
        }

        // Capture electrical summary for next step's EnvironmentState. The
        // summary is W (SI); the actors' pv_kw working value converts once
        // from it until the control layer holds SI.
        let net_grid = d.electrical_solver.net_active_w();
        // The electrical summary observation is checked once here, where it
        // is built (the consumers' per-decide PV re-checks were deleted):
        // non-finite is covered by the solver's unconditional screens above,
        // and a negative PV observation (PV equipment drawing power) is an
        // invariant violation.
        if !(-pv_w).is_finite() || (-pv_w) < 0.0 {
            return Err(HaresError::InvariantViolation {
                check_name: "electrical_summary_pv_observation".to_string(),
                value: -pv_w,
                tolerance: 0.0,
            });
        }
        d.prior_electrical_summary = ElectricalSummary {
            pv_generation_w: -pv_w,
            actual_pv_w: -pv_w,
            // Charge-only subtraction for battery and EV alike: discharge
            // power lands in generation_power_w, never in load_power_w, so
            // subtracting a negative (discharging) value here would inflate
            // the non-dispatchable base load.
            base_load_w: d.ports.electrical.load_power_w - battery_w.max(0.0) - ev_w.max(0.0),
            net_grid_w: net_grid,
            battery_power_w: battery_w,
            ev_power_w: ev_w,
        };

        // Update previous-step state for interest filtering on the next step.
        // Shift-register: prior ← prev (swap, zero alloc), then prev ← current
        // step's zones (clear + re-insert, zero alloc — reuses existing capacity).
        std::mem::swap(&mut d.prior_zone_temps, &mut d.prev_zone_temps);
        d.prev_zone_temps.clear();
        for zone in &d.latest_env.zones {
            d.prev_zone_temps.insert(zone.id, zone.temperature_c);
        }
        d.prev_price_signal = d.latest_env.price_signal.clone();

        // ORDERING: ports.zero() must come AFTER check_step_invariants() (called above)
        // because the electrical balance check reads d.ports.electrical.load_power_w
        // and generation_power_w to compute the ZIP-adjusted port net.
        // Physics checks raised from run_timestep in every build profile: the
        // per-equipment electric sum against the solver total, and the
        // per-zone HVAC port totals against the equipment thermal_output_w
        // sums (over the wiring pre-resolved at assembly). Both sides see
        // only what each equipment delivered this step.
        {
            let total = d.electrical_solver.net_active_w();
            let sum_equip: f64 = (0..d.equipment.len())
                .filter_map(|idx| d.delivered_output(idx))
                .map(|co| co.flows.electric_w.map_or(0.0, |e| e.net_consumption_w()))
                .sum();
            DwellingTelemetry::verify_consistency(d.clock.current_step(), sum_equip, total)?;
            d.verify_per_zone_thermal_consistency(d.clock.current_step())?;
        }
        d.ports.zero();
        let _ = d.clock.next();

        // Invariant: the dwelling RNG must have been consumed during this
        // timestep.  A zero-delta means run_timestep never called
        // advance_dwelling_rng, which would make checkpoint captures
        // reflect a dead RNG state. Debug-build logic check (the advance
        // call above is unconditional and cannot skip).
        #[cfg(debug_assertions)]
        debug_assert!(
            self.rng_consumed_this_step,
            "dwelling RNG was not consumed during timestep"
        );

        // Observe the dwelling RNG state for per-step auditability.
        #[cfg(feature = "observe")]
        tracing::debug!(
            rng_stream = d.rng.get_stream(),
            rng_word_pos = d.rng.get_word_pos(),
            "dwelling RNG state at end of timestep"
        );

        Ok(())
    }
}
