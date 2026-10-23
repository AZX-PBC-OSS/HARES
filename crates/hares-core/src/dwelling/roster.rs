//! Equipment and actor roster changes.
//!
//! Every entrance that changes the equipment or actor roster (assembly
//! included, through [`Dwelling::auto_register_actors`]) follows one
//! discipline: validate the candidate, plan every derived cache against the
//! prospective roster with [`Dwelling::plan_roster_caches`] (which reads
//! `self` and may fail), and only on `Ok` mutate the roster and install the
//! plan with [`Dwelling::install_roster_caches`] (which cannot fail). A
//! rejected change therefore leaves the dwelling exactly as it was.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use arrow::datatypes::Schema;
use chrono_tz::Tz;
use hares_control::DispatchTarget;
use hares_equipment::Equipment;
use hares_io::{
    Catalogue, CatalogueInput, MetricsCalculator, RowPlan, Selection, StreamingRecorder,
    build_catalogue, build_schema as build_schema_from_catalogue, build_schema_for_columns,
    build_schema_for_diagnostics, resolve_selection,
};
use hares_tariff::{ElectricTariff, TariffEvaluator};
use hares_types::{
    AmbientLocation, CoreOutput, CustomAccumulator, EndUse, EquipmentId, FluidType, HaresError,
    HumidityAccumulator, LoopId, PortDeclaration, PortSlots, ThermalAccumulator, Warning, ZoneId,
    telemetry_keys as tk,
};

use crate::Actor;
#[cfg(feature = "profiling")]
use crate::health::ActorTimings;
use crate::scheduler::{ActorSlot, ExecutionPhase};

use super::conversions::build_output_column_index;
use super::{
    ActorPricing, ActorSeedState, Dwelling, OutputRowBindings, Result, ZoneColumnCaches,
    actor_equipment, build_actors_from_seeds, build_hvac_thermal_consistency,
    build_output_row_bindings, build_zone_column_caches, compute_equipment_dispatch_targets,
    compute_equipment_execution_order, describe_target, enrich_schema_with_telemetry_units,
    target_lost, validate_equipment_stage, validate_equipment_zones, zone_display_name,
};
/// The values the per-step path reads that derive from the equipment and
/// actor rosters. Replaced only by [`Dwelling::install_roster_caches`];
/// empty until assembly installs the first plan.
pub(super) struct RosterCaches {
    /// Equipment indices in stage-rank order.
    pub(super) equipment_execution_order: Vec<usize>,
    /// Equipment ids in equipment-vector order, for the start-of-step
    /// `equipment_core` presence check.
    pub(super) equipment_ids: Vec<EquipmentId>,
    /// For each thermal port accumulator (in `ports.thermal` order), the
    /// indices of the HVAC heating or cooling equipment assigned to that
    /// zone, for the per-zone thermal consistency check.
    pub(super) hvac_thermal_consistency: Vec<Vec<usize>>,
    /// Zone positions in the environment and in the output schema.
    pub(super) zones: ZoneColumnCaches,
    /// Zone temperatures for the step result, reused every step.
    pub(super) zone_temp_scratch: Vec<(ZoneId, f64)>,
    /// The frozen output row plan: the reused row `record_step` fills, its
    /// validity bitset and local timestamp, and each selection's projection.
    /// Empty until assembly installs the first plan; every write on the step
    /// path goes through its precomputed indices.
    pub(super) row_plan: RowPlan,
    /// The row plan's per-report projections, copied at freeze: record_step
    /// pushes each report's projected row to its recorder through these
    /// indices without borrowing the plan.
    pub(super) record_projections: Vec<Vec<usize>>,
    /// The union schema the producer bindings resolved against: the columns
    /// every report's selections span, first-seen across the reports. A
    /// mid-run roster change rebuilds the bindings against this frozen
    /// schema.
    pub(super) union_schema: Schema,
    /// The columns the reports promise (the union of the selections
    /// resolved against the catalogue): an expected column the frozen
    /// schema cannot place is schema drift.
    pub(super) expected_columns: std::collections::HashSet<String>,
    /// The row plan's producer bindings, resolved at freeze: the
    /// dwelling-level columns, the per-equipment columns, the end-use
    /// aggregates and the actor columns.
    pub(super) bindings: OutputRowBindings,
    /// The output variable catalogue of the current roster: every variable
    /// the model can produce, declared once, typed. Readable from build on,
    /// before the first step.
    pub(super) catalogue: Arc<Catalogue>,
}

impl Default for RosterCaches {
    fn default() -> Self {
        Self {
            equipment_execution_order: Vec::new(),
            equipment_ids: Vec::new(),
            hvac_thermal_consistency: Vec::new(),
            zones: ZoneColumnCaches::default(),
            zone_temp_scratch: Vec::new(),
            row_plan: RowPlan::default(),
            record_projections: Vec::new(),
            union_schema: Schema::empty(),
            expected_columns: std::collections::HashSet::new(),
            bindings: OutputRowBindings::default(),
            catalogue: Arc::new(Catalogue::default()),
        }
    }
}

/// The port-slot tables and fluid loop-type map, re-derived from the
/// prospective roster's declarations while no step has run. Once stepping
/// has begun they are frozen, and a candidate is checked against them by
/// [`Dwelling::ensure_ports_satisfied`] instead.
struct PreStepRosterCaches {
    ports: PortSlots,
    rollback_ports: PortSlots,
    fluid_loop_types: HashMap<LoopId, FluidType>,
}

/// The output-pipeline part of a roster plan.
enum OutputCachesPlan {
    /// No report is configured and no metrics requested: nothing to plan
    /// or install.
    Disabled,
    /// No rows recorded yet: the schema is rebuilt from the prospective
    /// roster, with the row plan, its producer bindings and one recorder
    /// per report created for it.
    FreshSchema(Box<FreshOutputCaches>),
    /// Rows already recorded: the schema and the row plan are frozen with
    /// the reports' destinations; only the producer bindings follow the
    /// prospective roster. The bindings resolve columns by name against
    /// the frozen union schema (rebuilt here, at freeze time, never
    /// stored), so surviving entities keep their own columns and entities
    /// added after recording began get none: missing values, never values
    /// written into another entity's columns.
    FrozenUpdate(Box<FrozenOutputCaches>),
}

/// The frozen-update plan's payload: the producer bindings rebuilt against
/// the frozen union schema.
struct FrozenOutputCaches {
    bindings: OutputRowBindings,
}

/// The fresh-schema plan's payload: one recorder per report, the row plan
/// and its producer bindings, and the union schema the bindings resolved
/// against with the columns the reports promise. The metrics accumulator,
/// built when the run requested metrics, rides the same freeze.
struct FreshOutputCaches {
    recorders: Vec<StreamingRecorder>,
    row_plan: RowPlan,
    bindings: OutputRowBindings,
    union_schema: Schema,
    expected_columns: std::collections::HashSet<String>,
    metrics_calculator: Option<MetricsCalculator>,
}

/// Everything [`Dwelling::plan_roster_caches`] derives from a prospective
/// roster, installed verbatim by [`Dwelling::install_roster_caches`].
struct RosterPlan {
    equipment_id_by_name: HashMap<String, EquipmentId>,
    /// The live equipment ids as a set: the per-step snapshot's retain
    /// reads it directly, instead of rebuilding the set every step.
    active_equipment_ids: HashSet<EquipmentId>,
    dispatch_targets: Vec<DispatchTarget>,
    ambient_locations: Vec<AmbientLocation>,
    pre_step: Option<PreStepRosterCaches>,
    equipment_execution_order: Vec<usize>,
    equipment_ids: Vec<EquipmentId>,
    /// The current core output of each equipment the environment snapshot
    /// has no entry for yet (equipment joining the dwelling).
    joining_core_outputs: Vec<(EquipmentId, CoreOutput)>,
    hvac_thermal_consistency: Vec<Vec<usize>>,
    zones: ZoneColumnCaches,
    output: OutputCachesPlan,
    /// The catalogue built from the prospective roster's declarations.
    catalogue: Arc<Catalogue>,
}

/// The actor roster a roster change commits: which live actors stay (by
/// position), the built-in actors seeded for equipment joining the
/// dwelling, and actors supplied by the caller.
///
/// Built-in actors are placed after the leading run of kept built-in
/// actors, so built-in actors stay ahead of user actors in registration
/// order; caller-supplied actors are appended as user actors.
struct ActorRosterChange {
    keep: Vec<bool>,
    /// The warning for each live actor the change evicts because it leaves
    /// one of the actor's targets with no equipment to act on.
    orphaned: Vec<String>,
    insert_at: usize,
    built_in: Vec<Box<dyn Actor>>,
    supplied: Vec<Box<dyn Actor>>,
    next_ev_driver_stream: u64,
}

impl ActorRosterChange {
    /// The committed order: the first `insert_at` kept actors, the
    /// built-in actors, the remaining kept actors, the supplied actors.
    /// Generic so the prospective (borrowed) roster the plan reads and the
    /// committed (owned) roster are arranged by the same code.
    fn arrange<T>(
        kept: impl Iterator<Item = T>,
        insert_at: usize,
        built_in: impl IntoIterator<Item = T>,
        supplied: impl IntoIterator<Item = T>,
    ) -> Vec<T> {
        let mut kept = kept;
        let mut arranged: Vec<T> = kept.by_ref().take(insert_at).collect();
        arranged.extend(built_in);
        arranged.extend(kept);
        arranged.extend(supplied);
        arranged
    }

    fn prospective<'a>(&'a self, live: &'a [Box<dyn Actor>]) -> Vec<&'a dyn Actor> {
        Self::arrange(
            live.iter()
                .zip(&self.keep)
                .filter_map(|(actor, &keep)| keep.then_some(actor.as_ref())),
            self.insert_at,
            self.built_in.iter().map(AsRef::as_ref),
            self.supplied.iter().map(AsRef::as_ref),
        )
    }
}

/// The error every rejected add or replace returns: the reason, with the
/// warnings the rejected candidate raised during its own `init` (possibly
/// none). The candidate never joined the dwelling, so its warnings belong
/// to the caller's error, not to the run's log.
fn reject_candidate(err: HaresError, candidate: &mut dyn Equipment) -> HaresError {
    let mut drained: Vec<Warning> = Vec::new();
    candidate.drain_warnings(&mut drained);
    HaresError::RejectedEquipment {
        reason: Box::new(err),
        warnings: drained.iter().map(Warning::to_string).collect(),
    }
}

impl Dwelling {
    fn equipment_refs(&self) -> Vec<&dyn Equipment> {
        self.equipment.iter().map(AsRef::as_ref).collect()
    }

    /// Registers an actor. Duplicate names are rejected, mirroring
    /// [`Self::add_equipment`]: two same-named actors would both dispatch
    /// every step (double-driving an EV, double-managing a battery) with no
    /// signal that one of them is unwanted.
    ///
    /// Actors are called in registration order each timestep. They emit
    /// dispatch requests that are routed through the control dispatcher
    /// by [`hares_control::PriorityTier`].
    ///
    /// # Errors
    ///
    /// `HaresError::Control` when an actor with the same name is already
    /// registered; the roster plan's error otherwise, which includes the
    /// actor's own when it cannot act on its targets
    /// ([`Actor::validate_equipment`]). On `Err` the actor roster and
    /// schedule are exactly as before the call.
    pub fn add_actor(&mut self, actor: Box<dyn Actor>) -> Result<()> {
        self.ensure_actor_names_free(std::slice::from_ref(&actor))?;
        let mut change = self.actor_change(vec![true; self.actors.len()]);
        change.supplied.push(actor);
        let plan =
            self.plan_roster_caches(&self.equipment_refs(), &change.prospective(&self.actors))?;
        self.commit_actor_change(change);
        self.install_roster_caches(plan);
        self.record_replay_blocker(hares_types::ReplayBlocker::CallerActor);
        Ok(())
    }

    /// The dwelling's output variable catalogue: every variable the built
    /// model can produce, declared once, typed, with its source. Readable
    /// from build on, before the first step; rebuilt on every equipment or
    /// actor roster change.
    ///
    /// The catalogue does not depend on the selected reports: an output
    /// verbosity filters the catalogue's variables to the schema's columns,
    /// it never changes what the model can produce.
    #[must_use]
    pub fn catalogue(&self) -> Arc<Catalogue> {
        Arc::clone(&self.roster.catalogue)
    }

    /// The first report's recorded output schema's column names, in order
    /// (the `Time` label first, then one column per selected variable).
    /// `None` when no report is configured.
    #[must_use]
    pub fn output_column_names(&self) -> Option<Vec<String>> {
        self.recorders.first().map(|r| {
            r.schema()
                .fields()
                .iter()
                .map(|f| f.name().clone())
                .collect()
        })
    }

    /// The run's report requests: what each report records and where it
    /// goes.
    #[must_use]
    pub fn reports(&self) -> &[hares_io::ReportSpec] {
        &self.reports
    }

    /// Creates and adds an actor from the registry using the provided config.
    ///
    /// # Errors
    ///
    /// Returns an error if the actor type is not registered, or as
    /// [`Self::add_actor`].
    pub fn add_actor_by_name(
        &mut self,
        registry: &crate::actor_registry::ActorRegistry,
        config: crate::actor_registry::ActorConfig,
    ) -> Result<()> {
        let actor = registry.create(config)?;
        self.add_actor(actor)
    }
    fn ensure_actor_names_free(&self, actors: &[Box<dyn Actor>]) -> Result<()> {
        for (i, actor) in actors.iter().enumerate() {
            let name = actor.name();
            if self.actors.iter().any(|a| a.name() == name)
                || actors[..i].iter().any(|a| a.name() == name)
            {
                return Err(HaresError::Control(format!(
                    "duplicate actor name '{name}' is not allowed"
                )));
            }
        }
        Ok(())
    }

    /// Rebuilds the actor execution plan from current actor registrations.
    ///
    /// Auto-registered actors (BMS, EV driver) receive priority 0 within
    /// [`ExecutionPhase::ActorDecide`]; user-added actors receive priority 10.
    /// The solver feedback actor is registered in
    /// [`ExecutionPhase::SolverFeedback`] to run before all others.
    fn rebuild_schedule(&mut self) {
        self.scheduler.clear();
        self.scheduler.register_solver_feedback();
        for (i, actor) in self.actors.iter().enumerate() {
            let priority = if self.auto_registered_actor_names.contains(actor.name()) {
                0
            } else {
                10
            };
            self.scheduler.register_actor(
                ActorSlot(i),
                ExecutionPhase::ActorDecide,
                priority,
                actor.name(),
            );
        }
    }

    /// An actor roster change that keeps the live actors `keep` selects
    /// and adds nothing yet.
    fn actor_change(&self, keep: Vec<bool>) -> ActorRosterChange {
        let insert_at = self
            .actors
            .iter()
            .zip(&keep)
            .filter(|(_, keep)| **keep)
            .take_while(|(actor, _)| self.auto_registered_actor_names.contains(actor.name()))
            .count();
        ActorRosterChange {
            keep,
            orphaned: Vec::new(),
            insert_at,
            built_in: Vec::new(),
            supplied: Vec::new(),
            next_ev_driver_stream: self.next_ev_driver_stream,
        }
    }

    /// An actor roster change for the equipment becoming `after`: a live
    /// actor stays when `keep` selects it, unless the change takes away
    /// the equipment of one of its targets ([`Actor::dispatch_targets`]);
    /// such an actor is evicted with a warning.
    fn equipment_change_actors(
        &self,
        after: &[&dyn Equipment],
        keep: impl Fn(&dyn Actor) -> bool,
    ) -> ActorRosterChange {
        let before = self.equipment_refs();
        let mut orphaned = Vec::new();
        let keep_mask = self
            .actors
            .iter()
            .map(|actor| {
                let lost = actor
                    .dispatch_targets()
                    .iter()
                    .find(|t| target_lost(t, &before, after));
                match lost {
                    Some(lost) => {
                        orphaned.push(format!(
                            "actor '{}' is evicted: the roster change leaves its target {} \
                             with no equipment that accepts its signals ({:?})",
                            actor.name(),
                            describe_target(&lost.target),
                            lost.required
                        ));
                        false
                    }
                    None => keep(actor.as_ref()),
                }
            })
            .collect();
        let mut change = self.actor_change(keep_mask);
        change.orphaned = orphaned;
        change
    }

    /// Adds to `change` the built-in actors the actor seeds of `equipment`
    /// build, priced by `tariff`. A seed whose actor name the changed
    /// roster already carries builds nothing; a driver rebuilt under the
    /// name of one of `rebuilt` takes over its state.
    fn seed_built_in_actors(
        &self,
        change: &mut ActorRosterChange,
        equipment: &[Box<dyn Equipment>],
        tariff: Option<&TariffEvaluator>,
        rebuilt: &[&dyn Actor],
    ) -> Result<()> {
        let interval_secs = self.clock.time_res.num_seconds() as u32;
        assert!(
            interval_secs > 0,
            "time resolution must be positive; got 0 seconds"
        );
        let steps_per_day = 86_400 / interval_secs as usize;
        let price_schedule: Option<Arc<[f64]>> =
            tariff.and_then(|te| te.price_slice(0, te.total_steps()).map(Arc::from));
        let mut seed_state = ActorSeedState {
            rng: &self.rng,
            next_stream: change.next_ev_driver_stream,
            rebuilt,
        };
        let built_in = build_actors_from_seeds(
            equipment,
            &change.prospective(&self.actors),
            ActorPricing {
                has_tariff: tariff.is_some(),
                price_schedule,
                steps_per_day,
            },
            &self.equipment_id_by_name,
            &mut seed_state,
        )?;
        change.next_ev_driver_stream = seed_state.next_stream;
        change.built_in.extend(built_in);
        Ok(())
    }

    fn commit_actor_change(&mut self, change: ActorRosterChange) {
        let built_in_names: Vec<String> = change
            .built_in
            .iter()
            .map(|a| a.name().to_string())
            .collect();
        let evicted: Vec<&str> = self
            .actors
            .iter()
            .zip(&change.keep)
            .filter_map(|(actor, &keep)| (!keep).then_some(actor.name()))
            .collect();
        if !evicted.is_empty() {
            tracing::info!(actors = ?evicted, built_in = ?built_in_names, "actors evicted");
        }
        // An actor the change leaves without a target is evicted with a
        // warning naming it and the target it lost; the eviction is counted
        // in the run's health.
        for warning in change.orphaned {
            self.health.evicted_actors += 1;
            self.warnings.push_from("actors", warning);
        }
        let live = std::mem::take(&mut self.actors);
        self.actors = ActorRosterChange::arrange(
            live.into_iter()
                .zip(change.keep)
                .filter_map(|(actor, keep)| keep.then_some(actor)),
            change.insert_at,
            change.built_in,
            change.supplied,
        );
        self.next_ev_driver_stream = change.next_ev_driver_stream;
        let actors = &self.actors;
        self.auto_registered_actor_names
            .retain(|name| actors.iter().any(|a| a.name() == name));
        self.auto_registered_actor_names.extend(built_in_names);
        // The seeded-stream record refreshes with the roster: the
        // auto-registered actors that hold a dwelling-derived stream (an
        // EV driver's own, fresh or taken over from the driver it
        // replaced). reseed re-derives exactly these; a user actor's own
        // seeded generator (a registry-built driver's config seed) is
        // never the dwelling's to reseed.
        self.seeded_stream_actors = self
            .actors
            .iter()
            .filter(|actor| {
                actor.rng_stream_state().is_some()
                    && self.auto_registered_actor_names.contains(actor.name())
            })
            .map(|actor| actor.name().to_string())
            .collect();
        self.rebuild_schedule();
    }

    /// The change that rebuilds every built-in actor from the equipment's
    /// actor seeds, priced by `tariff`, keeping every user actor. A rebuilt
    /// actor takes over the state of the actor it replaces.
    fn plan_built_in_actor_rebuild(
        &self,
        tariff: Option<&TariffEvaluator>,
    ) -> Result<ActorRosterChange> {
        let keep: Vec<bool> = self
            .actors
            .iter()
            .map(|a| !self.auto_registered_actor_names.contains(a.name()))
            .collect();
        let rebuilt: Vec<&dyn Actor> = self
            .actors
            .iter()
            .zip(&keep)
            .filter_map(|(actor, &keep)| (!keep).then_some(actor.as_ref()))
            .collect();
        let mut change = self.actor_change(keep);
        self.seed_built_in_actors(&mut change, &self.equipment, tariff, &rebuilt)?;
        Ok(change)
    }

    /// Rebuilds the built-in BMS and EV driver actors from the equipment's
    /// actor seeds with the current tariff's prices.
    ///
    /// Built-in actors run ahead of user actors. A seed whose actor name a
    /// user actor already holds builds nothing. A rebuilt actor takes over
    /// the decision state and telemetry of the actor it replaces: an EV
    /// driver its RNG stream and position too, a battery management actor
    /// everything but the price thresholds of its current day, which it
    /// recomputes from the new prices.
    ///
    /// # Errors
    ///
    /// The roster plan's error; on `Err` the actor roster and schedule are
    /// exactly as before the call.
    pub fn auto_register_actors(&mut self) -> Result<()> {
        let change = self.plan_built_in_actor_rebuild(self.tariff_evaluator.as_ref())?;
        let plan =
            self.plan_roster_caches(&self.equipment_refs(), &change.prospective(&self.actors))?;
        self.commit_actor_change(change);
        self.install_roster_caches(plan);
        Ok(())
    }

    /// Configures a tariff evaluator from an electric tariff definition and
    /// rebuilds the built-in actors with its prices.
    ///
    /// The evaluator precomputes prices for the entire simulation horizon so
    /// that `run_timestep` can populate `EnvironmentState.price_signal` before
    /// actors run. Billing accumulation happens post-solver each step.
    ///
    /// Attached after step 0, the tariff bills from the period containing
    /// the attach step: the period keeps the billing cycle's boundary
    /// structure and its fixed and demand charges prorate to the span from
    /// the attach step to the period end. Replacing a tariff closes the
    /// outgoing open period with its accruals (the bill exists) and opens
    /// the new one at the switch step.
    ///
    /// # Errors
    ///
    /// The evaluator's or the roster plan's error. On `Err` the dwelling
    /// keeps its previous tariff, actors, billing summaries and warning log.
    pub fn set_tariff(&mut self, tariff: ElectricTariff, tz: Tz) -> Result<()> {
        let attached_tariff = tariff.clone();
        let parse_warnings = tariff.parse_warnings.clone();
        let attach_step = self.clock.current_step;
        let replacing = self.tariff_evaluator.is_some();
        let start = self.clock.start_time.with_timezone(&tz);
        let end = (self.clock.start_time + self.clock.duration).with_timezone(&tz);
        let interval_secs = self.clock.time_res.num_seconds() as u32;
        let mut evaluator = TariffEvaluator::new(tariff, start, end, interval_secs)?;
        // The evaluator prices step by step from the simulation start; a
        // tariff attached mid-run prices (and bills) from the current step.
        for _ in 0..self.clock.current_step {
            evaluator.advance();
        }
        // The attach instant is the current step's start: the first step the
        // evaluator will fold is this one.
        let attach_time = start
            + chrono::Duration::seconds(self.clock.current_step as i64 * interval_secs as i64);
        if self.clock.current_step > 0 {
            evaluator.activate_at(attach_time);
        }
        let change = self.plan_built_in_actor_rebuild(Some(&evaluator))?;
        let plan =
            self.plan_roster_caches(&self.equipment_refs(), &change.prospective(&self.actors))?;
        // Replacing a tariff closes the outgoing open period with its
        // accruals: the bill exists. This commits only after both plans
        // succeeded, so a rejected set_tariff leaves the billing summaries
        // as they were.
        let outgoing_close = match self.tariff_evaluator.take() {
            Some(mut outgoing) => outgoing.close_open_period(attach_time),
            None => None,
        };
        // Parse warnings report in every run the tariff is attached to: a
        // tariff parsed once and attached to many dwellings never warns
        // silently.
        for message in parse_warnings {
            self.warnings.push_warning(Warning::new("tariff", message));
        }
        if let Some(summary) = outgoing_close {
            self.billing_summaries.push(summary);
        }
        self.tariff_evaluator = Some(evaluator);
        self.commit_actor_change(change);
        self.install_roster_caches(plan);
        self.tariff_record = Some(crate::TariffRecord {
            tariff: attached_tariff,
            zone: tz.name().to_string(),
            attach_step,
        });
        // A replacement closes the outgoing period with the outgoing
        // tariff's accruals through the run's own steps: a replay cannot
        // rebuild that close without those steps, so the run's export
        // refuses.
        if replacing {
            self.record_replay_blocker(hares_types::ReplayBlocker::TariffReplaced);
        }
        Ok(())
    }

    /// Adds equipment to the dwelling, with the built-in actor its actor
    /// seed builds (a battery's BMS, an EV's driver).
    ///
    /// Duplicate equipment names are rejected: the caller must provide
    /// unique names, as construction requires.
    ///
    /// Identity contract (mirrors assembly's entrance validation): the
    /// unassigned sentinel id 0 never survives into the equipment vector
    /// (unassigned equipment receives the dwelling's next never-reused id),
    /// and an explicitly-set id that collides with equipment already in the
    /// dwelling is rejected, naming both. The identity write is verified
    /// (the descriptor must report exactly the assigned id) before the
    /// equipment joins the vector, so a broken `set_equipment_id`
    /// implementation fails registration loudly instead of collapsing
    /// every `EquipmentId`-keyed map onto a shared id.
    ///
    /// # Errors
    ///
    /// [`HaresError::RejectedEquipment`], whose reason is the entrance
    /// validation's or the roster plan's error and which carries the
    /// warnings the equipment raised during its own `init`. On `Err` the
    /// dwelling is exactly as before the call.
    pub fn add_equipment(&mut self, eq: Box<dyn Equipment>) -> Result<()> {
        self.add_equipment_with_actors(eq, Vec::new())
    }

    /// Adds equipment together with user actors that drive it, as one
    /// change: either both join or neither does. A supplied actor holding
    /// the name of the equipment's built-in actor replaces it.
    ///
    /// # Errors
    ///
    /// As [`Self::add_equipment`]; the reason is `HaresError::Control` when
    /// the equipment passes its own validation but a supplied actor's name
    /// is already registered.
    pub fn add_equipment_with_actors(
        &mut self,
        mut eq: Box<dyn Equipment>,
        actors: Vec<Box<dyn Actor>>,
    ) -> Result<()> {
        let supplies_actors = !actors.is_empty();
        let planned = self
            .validate_candidate_equipment(&mut eq)
            .and_then(|next_equipment_id| {
                self.ensure_actor_names_free(&actors)?;
                let mut change = self.actor_change(vec![true; self.actors.len()]);
                change.supplied = actors;
                self.seed_built_in_actors(
                    &mut change,
                    std::slice::from_ref(&eq),
                    self.tariff_evaluator.as_ref(),
                    &[],
                )?;
                let mut equipment = self.equipment_refs();
                equipment.push(eq.as_ref());
                let plan =
                    self.plan_roster_caches(&equipment, &change.prospective(&self.actors))?;
                Ok((next_equipment_id, change, plan))
            });
        let (next_equipment_id, change, plan) =
            planned.map_err(|err| reject_candidate(err, eq.as_mut()))?;
        let mut joined_warnings: Vec<Warning> = Vec::new();
        eq.drain_warnings(&mut joined_warnings);
        // Post-registration LUT mutation through the Equipment trait
        // setters is rejected from here on.
        eq.mark_initialized();

        self.next_equipment_id = next_equipment_id;
        for warning in joined_warnings {
            self.warnings.push_warning(warning);
        }
        self.equipment.push(eq);
        self.commit_actor_change(change);
        self.install_roster_caches(plan);
        // The caller's supplied actors register with the equipment: the
        // same caller-actor surface `add_actor` records, beside the
        // equipment change the call is also one.
        if supplies_actors {
            self.record_replay_blocker(hares_types::ReplayBlocker::CallerActor);
        }
        self.record_replay_blocker(hares_types::ReplayBlocker::EquipmentChanged);
        Ok(())
    }

    /// The entrance validation `add_equipment` runs before a candidate may
    /// join the dwelling: duplicate-name rejection, identity assignment,
    /// declared-zone and ambient-location validation, and (once stepping
    /// has begun) frozen port-slot satisfaction. Returns the `next_equipment_id` counter
    /// value this assignment plans to advance to; the caller writes it only
    /// once the roster change is committed (see
    /// [`Self::assign_equipment_identity`]).
    fn validate_candidate_equipment(&self, eq: &mut Box<dyn Equipment>) -> Result<u32> {
        let name = eq.descriptor().name.clone();
        if self.equipment.iter().any(|e| e.descriptor().name == name) {
            return Err(HaresError::Equipment(format!(
                "duplicate equipment name '{}' is not allowed",
                name
            )));
        }
        let planned_next_equipment_id = self.assign_equipment_identity(eq, &name, None)?;
        self.validate_candidate_ports(eq.as_ref())?;
        Ok(planned_next_equipment_id)
    }

    /// The entrance validation `replace_equipment` runs before the swap:
    /// the duplicate-surviving-name contract (the evictee exempt), identity
    /// assignment, declared-zone and ambient-location validation, and (once
    /// stepping has begun) frozen port-slot satisfaction.
    fn validate_replacement_equipment(
        &self,
        eq: &mut Box<dyn Equipment>,
        replaced_name: &str,
        pos: usize,
    ) -> Result<u32> {
        let new_name = eq.descriptor().name.clone();
        // A replacement may keep the replaced equipment's own name
        // (replace-in-kind), but a name held by any surviving equipment
        // would collapse `equipment_id_by_name` and misbind every
        // name-keyed snapshot lookup.
        if self
            .equipment
            .iter()
            .enumerate()
            .any(|(i, e)| i != pos && e.descriptor().name == new_name)
        {
            return Err(HaresError::Equipment(format!(
                "duplicate equipment name '{new_name}' is not allowed: it is held by \
                 a surviving equipment; a replacement may reuse only the replaced \
                 equipment's own name"
            )));
        }
        let planned_next_equipment_id =
            self.assign_equipment_identity(eq, &new_name, Some(replaced_name))?;
        self.validate_candidate_ports(eq.as_ref())?;
        Ok(planned_next_equipment_id)
    }

    /// A declared zone must exist in the environment model (a contribution
    /// to an unknown zone would be silently dropped), the building must be
    /// able to supply the candidate's ambient air, and once stepping has
    /// begun the frozen port-slot table must already carry an accumulator
    /// for every declared port.
    fn validate_candidate_ports(&self, eq: &dyn Equipment) -> Result<()> {
        let env_zone_ids: HashSet<ZoneId> = self.latest_env.zones.iter().map(|z| z.id).collect();
        validate_equipment_zones(eq.ports(), &env_zone_ids)?;
        validate_equipment_stage(eq)?;
        if let Some(location) = eq.ambient_location() {
            self.environment.check_ambient_location(location)?;
        }
        if self.clock.current_step > 0 {
            self.ensure_ports_satisfied(eq)?;
        }
        Ok(())
    }

    /// Assign and validate one equipment's identity before it joins the
    /// equipment vector: the shared entrance logic of `add_equipment` and
    /// `replace_equipment`.
    ///
    /// - Unassigned (descriptor id 0, whether never set or explicitly
    ///   zeroed): auto-assign the dwelling's next never-reused id.
    /// - Explicit non-zero id: rejected if it collides with an id already
    ///   in the vector (skipping `replace_name`'s evictee on the replace
    ///   path, whose id leaves with it; the add path passes `None` and
    ///   exempts nobody).
    /// - The write is guard-checked inside the equipment (`set_equipment_id`
    ///   rejects post-registration mutation) and lands **before**
    ///   `mark_initialized`, so the authorized path never trips the guard.
    ///   A write that would not change the id (the explicit path, where the
    ///   descriptor already holds the collision-checked value) is skipped;
    ///   otherwise the guard would refuse the *re*-registration of a removed
    ///   equipment, which still carries the initialized mark from its first
    ///   registration while being in no dwelling at the moment of the write.
    /// - Postcondition: `descriptor().id` must equal the assigned value
    ///   exactly. Equality subsumes non-zero (assigned ids start at 1) and
    ///   uniqueness (the counter never issues an in-use id), and catches a
    ///   body that wrote nothing, the wrong field, or a wrong-but-plausible
    ///   value the counter cannot have advanced past.
    ///
    /// Returns the `next_equipment_id` counter value this assignment would
    /// advance to. Does not write it, so a candidate rejected later in its
    /// entrance never consumes an id; the entrance writes it on commit.
    fn assign_equipment_identity(
        &self,
        eq: &mut Box<dyn Equipment>,
        name: &str,
        replace_name: Option<&str>,
    ) -> Result<u32> {
        let requested = eq.descriptor().id;
        let assigned = if requested.0 == 0 {
            // Auto-assign from the never-reused counter, skipping any id
            // already in the vector. The counter is monotonic past every id
            // it issued and every explicit id that could advance it, so the
            // skip is normally zero iterations. The single id that can be
            // in use at the counter is an explicit `u32::MAX` (a checked
            // advance cannot pass it, so the counter stays below), and the
            // skip hands the auto-assign the next genuinely free id instead
            // of re-issuing the in-use boundary value. Exhaustion (every id
            // from here to u32::MAX in use) is a loud typed error, never a
            // silent duplicate.
            let mut candidate = self.next_equipment_id;
            while self
                .equipment
                .iter()
                .any(|e| e.descriptor().id.0 == candidate)
            {
                candidate = candidate.checked_add(1).ok_or_else(|| {
                    HaresError::Equipment(format!(
                        "equipment '{name}': no free equipment id: the never-reused \
                         id counter is exhausted at u32::MAX and every remaining id \
                         is already in use; the dwelling cannot accept more equipment"
                    ))
                })?;
            }
            EquipmentId(candidate)
        } else {
            // The exemption applies only on the replace path, where it
            // excludes exactly the evictee (names are unique). The add path
            // has no evictee, so `None` exempts nobody.
            let collides_with = self.equipment.iter().find(|e| {
                e.descriptor().id == requested
                    && replace_name.is_none_or(|evictee| e.descriptor().name.as_str() != evictee)
            });
            if let Some(existing) = collides_with {
                return Err(HaresError::Equipment(format!(
                    "duplicate equipment id {:?}: equipment '{}' cannot join \
                     a dwelling that already has '{}' on that id; ids are \
                     dwelling-assigned; omit the explicit id and the dwelling \
                     will assign one",
                    requested,
                    name,
                    existing.descriptor().name
                )));
            }
            requested
        };
        // Write only when the id actually changes: an equipment being
        // re-registered after `remove_equipment` still carries the
        // initialized mark of its first registration, and a no-op write
        // would trip the identity guard and break the remove then re-add
        // flow for guard-tracking types (EV, Battery).
        if eq.descriptor().id != assigned {
            eq.set_equipment_id(assigned).map_err(|err| {
                HaresError::Equipment(format!(
                    "equipment '{name}': identity assignment rejected: {err}"
                ))
            })?;
        }
        if eq.descriptor().id != assigned {
            return Err(HaresError::Equipment(format!(
                "equipment '{name}': identity write did not take the assigned id \
                 {assigned:?} (descriptor still reports {:?}); check its \
                 `Equipment::set_equipment_id` implementation",
                eq.descriptor().id
            )));
        }
        // Advance past every entering id, checked rather than saturating:
        // an explicit `u32::MAX` cannot be advanced past (saturating would
        // pin the counter on the in-use id and reissue it), so the counter
        // stays where it is and the skip loop above keeps the boundary safe.
        Ok(match assigned.0.checked_add(1) {
            Some(next) => self.next_equipment_id.max(next),
            None => self.next_equipment_id,
        })
    }

    /// Removes all equipment. Every actor with a target the equipment
    /// served is evicted with it (see [`Self::remove_equipment`]).
    ///
    /// # Errors
    ///
    /// The roster plan's error; on `Err` the equipment and actor rosters
    /// are exactly as before the call.
    pub fn clear_equipment(&mut self) -> Result<()> {
        let change = self.equipment_change_actors(&[], |_| true);
        let plan = self.plan_roster_caches(&[], &change.prospective(&self.actors))?;
        self.equipment.clear();
        self.commit_actor_change(change);
        self.install_roster_caches(plan);
        self.record_replay_blocker(hares_types::ReplayBlocker::EquipmentChanged);
        Ok(())
    }

    /// Removes equipment by name and returns it.
    ///
    /// Every actor the removal leaves with a target no remaining equipment
    /// serves ([`Actor::dispatch_targets`]: a unit it names, or the last
    /// unit of an end use it addresses) is evicted, with a dwelling warning
    /// counted in [`crate::RunHealth::evicted_actors`]: a controller without
    /// its equipment is an orphan whose every signal would be rejected, and
    /// its survival would block re-adding a same-named replacement (see
    /// [`Self::add_actor`]). A removal is never refused for an actor's
    /// sake.
    ///
    /// # Errors
    ///
    /// `HaresError::Dwelling` when no equipment has the name; the roster
    /// plan's error otherwise. On `Err` the equipment and actor rosters are
    /// exactly as before the call.
    pub fn remove_equipment(&mut self, name: &str) -> Result<Box<dyn Equipment>> {
        let pos = self
            .equipment
            .iter()
            .position(|e| e.descriptor().name == name)
            .ok_or_else(|| HaresError::Dwelling(format!("equipment '{}' not found", name)))?;
        let (change, plan) = {
            let mut equipment = self.equipment_refs();
            equipment.remove(pos);
            let change = self.equipment_change_actors(&equipment, |_| true);
            let plan = self.plan_roster_caches(&equipment, &change.prospective(&self.actors))?;
            (change, plan)
        };
        let removed = self.equipment.remove(pos);
        self.commit_actor_change(change);
        self.install_roster_caches(plan);
        self.record_replay_blocker(hares_types::ReplayBlocker::EquipmentChanged);
        Ok(removed)
    }

    /// Removes all equipment whose end use is one of `end_uses`, returning
    /// how many were removed. Actors left without a target are evicted
    /// (see [`Self::remove_equipment`]).
    ///
    /// # Errors
    ///
    /// The roster plan's error when any equipment matched; on `Err` the
    /// equipment and actor rosters are exactly as before the call.
    pub fn remove_equipment_by_end_use(&mut self, end_uses: &[EndUse]) -> Result<usize> {
        let removed: Vec<bool> = self
            .equipment
            .iter()
            .map(|e| end_uses.contains(&e.descriptor().end_use))
            .collect();
        let removed_count = removed.iter().filter(|&&r| r).count();
        if removed_count == 0 {
            return Ok(0);
        }
        let (change, plan) = {
            let equipment: Vec<&dyn Equipment> = self
                .equipment
                .iter()
                .zip(&removed)
                .filter_map(|(e, &r)| (!r).then_some(e.as_ref()))
                .collect();
            let change = self.equipment_change_actors(&equipment, |_| true);
            let plan = self.plan_roster_caches(&equipment, &change.prospective(&self.actors))?;
            (change, plan)
        };
        self.equipment
            .retain(|e| !end_uses.contains(&e.descriptor().end_use));
        self.commit_actor_change(change);
        self.install_roster_caches(plan);
        self.record_replay_blocker(hares_types::ReplayBlocker::EquipmentChanged);
        Ok(removed_count)
    }

    /// Replaces equipment by name with new equipment, returning the old
    /// equipment.
    ///
    /// Identity and registration mirror [`Self::add_equipment`]: the
    /// replacement's unassigned sentinel id 0 never survives (it is
    /// auto-assigned), an explicit id may not collide with any *other*
    /// equipment (the evictee's id leaves with it), the identity write is
    /// verified, and the replacement's name may not collide with any
    /// surviving equipment's (the evictee's own name is exempt:
    /// replace-in-kind).
    ///
    /// Actors follow the name while the replacement accepts their signals:
    /// when the replacement keeps the replaced name, the actors targeting
    /// it stay and re-bind to the replacement unless the replacement lacks
    /// a control capability they require ([`Actor::dispatch_targets`]), in
    /// which case they are evicted with a warning, as on removal. An actor
    /// that stays is checked against the replacement
    /// ([`Actor::validate_equipment`]) and refuses it when it cannot act on
    /// it; removing the unit and adding the replacement instead evicts that
    /// actor. A built-in actor also stays only when the replacement's
    /// actor seed equals the replaced equipment's, since the seed carries
    /// the parameters the actor was built with (an EV's capacity, a
    /// battery's mode); otherwise it is evicted and the replacement's seed
    /// builds a fresh one, as if the equipment had been removed and the
    /// replacement added. When the replacement takes another name, every
    /// actor left without a target is evicted, as on removal. The
    /// replacement's actor seed builds its built-in actor unless an actor
    /// already holds that actor's name.
    ///
    /// # Errors
    ///
    /// [`HaresError::RejectedEquipment`], whose reason is
    /// `HaresError::Dwelling` when no equipment has the name, and the
    /// entrance validation's or the roster plan's error otherwise. On `Err`
    /// the dwelling is exactly as before the call.
    pub fn replace_equipment(
        &mut self,
        name: &str,
        mut new_equipment: Box<dyn Equipment>,
    ) -> Result<Box<dyn Equipment>> {
        let planned = self
            .equipment
            .iter()
            .position(|e| e.descriptor().name == name)
            .ok_or_else(|| HaresError::Dwelling(format!("equipment '{name}' not found")))
            .and_then(|pos| {
                let next_equipment_id =
                    self.validate_replacement_equipment(&mut new_equipment, name, pos)?;
                Ok((pos, next_equipment_id))
            })
            .and_then(|(pos, next_equipment_id)| {
                let seed_unchanged = self.equipment[pos].actor_seed() == new_equipment.actor_seed();
                let mut equipment = self.equipment_refs();
                equipment[pos] = new_equipment.as_ref();
                let mut change = self.equipment_change_actors(&equipment, |a| {
                    seed_unchanged
                        || !self.auto_registered_actor_names.contains(a.name())
                        || !a
                            .dispatch_targets()
                            .iter()
                            .any(|t| matches!(&t.target, DispatchTarget::ByName(n) if **n == *name))
                });
                self.seed_built_in_actors(
                    &mut change,
                    std::slice::from_ref(&new_equipment),
                    self.tariff_evaluator.as_ref(),
                    &[],
                )?;
                let plan =
                    self.plan_roster_caches(&equipment, &change.prospective(&self.actors))?;
                Ok((pos, next_equipment_id, change, plan))
            });
        let (pos, next_equipment_id, change, plan) =
            planned.map_err(|err| reject_candidate(err, new_equipment.as_mut()))?;
        // The evicted equipment's deferred step warnings drain before it
        // leaves: unstamped before the first step, stamped with the step in
        // flight after. Every call into equipment code happens before the
        // change is committed.
        let mut drained: Vec<Warning> = Vec::new();
        self.equipment[pos].drain_warnings(&mut drained);
        if self.clock.current_step > 0 {
            for warning in &mut drained {
                warning.step_index = Some(self.clock.current_step);
            }
        }
        new_equipment.drain_warnings(&mut drained);
        new_equipment.mark_initialized();

        self.next_equipment_id = next_equipment_id;
        for warning in drained {
            self.warnings.push_warning(warning);
        }
        let old = std::mem::replace(&mut self.equipment[pos], new_equipment);
        self.commit_actor_change(change);
        self.install_roster_caches(plan);
        self.record_replay_blocker(hares_types::ReplayBlocker::EquipmentChanged);
        Ok(old)
    }

    /// Monotonic pre-step port-slot rebuild: the layout
    /// [`PortSlots::from_declarations`] derives from the live declarations,
    /// extended with every zone and domain accumulator the previous table
    /// already carried. Never drops zone/domain coverage (the envelope
    /// solvers step against env-zone thermal/humidity accumulators
    /// regardless of which equipment currently declares them, e.g. after
    /// `clear_equipment`), and never reuses the previous accumulators'
    /// *values*: before the first step the table is pure layout.
    ///
    /// Fluid accumulators are the one exception to the monotonic rule: a
    /// loop no live equipment declares is dropped, not retained, so the
    /// table's fluid accumulators agree with the loop-type map
    /// [`Self::plan_roster_caches`] derives from the live declarations.
    fn rebuild_port_slots(declarations: &[PortDeclaration], previous: &PortSlots) -> PortSlots {
        let mut rebuilt = PortSlots::from_declarations(declarations);
        for acc in &previous.thermal {
            if !rebuilt.thermal.iter().any(|t| t.zone == acc.zone) {
                rebuilt.thermal.push(ThermalAccumulator::new(acc.zone));
            }
        }
        for acc in &previous.humidity {
            if !rebuilt.humidity.iter().any(|h| h.zone == acc.zone) {
                rebuilt.humidity.push(HumidityAccumulator::new(acc.zone));
            }
        }
        for acc in &previous.custom {
            if !rebuilt.custom.iter().any(|c| c.domain_id == acc.domain_id) {
                rebuilt.custom.push(CustomAccumulator::new(acc.domain_id));
            }
        }
        rebuilt
    }

    /// Post-step port-slot guard for equipment joining after stepping has
    /// begun.
    ///
    /// Before the first step every roster plan rebuilds the port-slot table
    /// from the prospective declarations, so every declaration is satisfied
    /// by construction. Once stepping has begun the table is frozen
    /// (per-step accumulator state and solver wiring are live), so
    /// equipment joining mid-run must declare only ports the frozen table
    /// already carries an accumulator for; an unsatisfied declaration is
    /// rejected, naming the equipment and the orphaned port.
    fn ensure_ports_satisfied(&self, eq: &dyn Equipment) -> Result<()> {
        for decl in eq.ports() {
            let satisfied = match decl.port_type {
                // Singletons: always present via `PortSlots::default` parts.
                hares_types::PortType::Electrical | hares_types::PortType::Fuel => true,
                hares_types::PortType::Thermal => decl
                    .zone
                    .is_some_and(|z| self.ports.thermal.iter().any(|t| t.zone == z)),
                hares_types::PortType::Humidity => decl
                    .zone
                    .is_some_and(|z| self.ports.humidity.iter().any(|h| h.zone == z)),
                hares_types::PortType::Fluid => match (decl.loop_id, decl.fluid_type) {
                    (Some(loop_id), Some(fluid_type)) => {
                        let node_id = decl.fluid_node_id.unwrap_or(hares_types::FluidNodeId(0));
                        self.ports.fluid.iter().any(|f| {
                            f.loop_id == loop_id
                                && f.fluid_type == fluid_type
                                && f.node_id == node_id
                        })
                    }
                    // A declaration without loop/type is inert; the
                    // assembly-time validation is its home.
                    (None, _) | (_, None) => true,
                },
                hares_types::PortType::Custom => decl
                    .domain_id
                    .is_some_and(|d| self.ports.custom.iter().any(|c| c.domain_id == d)),
            };
            if !satisfied {
                return Err(HaresError::Equipment(format!(
                    "equipment '{}' declares a {:?} port (zone={:?}, loop={:?}, \
                     domain={:?}) with no accumulator in the dwelling's port-slot \
                     table, and the table is frozen because stepping has begun \
                     (step {}): its contributions through that port would be \
                     silently miswired or dropped. Add the equipment before the \
                     first step, or declare it against a zone/loop/domain the \
                     dwelling already carries.",
                    eq.descriptor().name,
                    decl.port_type,
                    decl.zone,
                    decl.loop_id,
                    decl.domain_id,
                    self.clock.current_step
                )));
            }
        }
        Ok(())
    }

    pub(super) fn zone_names(&self) -> Vec<(ZoneId, String)> {
        let indoor_zone = self.thermal_solver.config().indoor_zone_id;
        self.latest_env
            .zones
            .iter()
            .enumerate()
            .map(|(idx, z)| {
                (
                    z.id,
                    zone_display_name(z.id, indoor_zone, self.environment.zone_types().get(idx)),
                )
            })
            .collect()
    }

    /// The catalogue input for a prospective roster: every equipment's
    /// base name, instance override, id, typed end use and capabilities,
    /// fuel and typed telemetry declarations; the zone display names;
    /// every actor's registered name and typed declarations.
    fn catalogue_input(
        equipment: &[&dyn Equipment],
        actors: &[&dyn Actor],
        zones: Vec<(ZoneId, String)>,
    ) -> CatalogueInput {
        let equipment_input = equipment
            .iter()
            .map(|eq| {
                let desc = eq.descriptor();
                hares_io::CatalogueEquipment {
                    name: desc.name.clone(),
                    instance_name: None,
                    id: desc.id,
                    fuel: desc.fuel,
                    end_use: desc.end_use.clone(),
                    core_capabilities: desc.core_capabilities,
                    telemetry: desc.telemetry_fields.clone(),
                }
            })
            .collect();
        let actor_input = actors
            .iter()
            .map(|a| hares_io::CatalogueActor {
                name: Arc::from(a.name()),
                output_variables: a.output_variables().to_vec(),
            })
            .collect();
        CatalogueInput {
            equipment: equipment_input,
            zones,
            actors: actor_input,
        }
    }

    /// Rejects an actor whose telemetry is not built (and sealed) from its
    /// declarations, or whose telemetry holds a key the declarations omit,
    /// naming the actor and the key. Runs at every registration and
    /// auto-registration, so no per-step check is needed.
    fn validate_actor_telemetry(actor: &dyn Actor) -> Result<()> {
        let declared: std::collections::HashSet<&str> = actor
            .output_variables()
            .iter()
            .map(|f| f.name.as_ref())
            .collect();
        match actor.telemetry() {
            None => {
                if declared.is_empty() {
                    Ok(())
                } else {
                    Err(HaresError::Control(format!(
                        "actor '{}' declares {} output variables but exposes no telemetry map; \
                         build it with Telemetry::from_fields(declared)",
                        actor.name(),
                        declared.len(),
                    )))
                }
            }
            Some(telemetry) => {
                let name = actor.name();
                if !telemetry.is_sealed() {
                    return Err(HaresError::Control(format!(
                        "actor '{name}' telemetry is not sealed: build it with \
                         Telemetry::from_fields(declared) so its keys are its declarations"
                    )));
                }
                let sealed = telemetry
                    .sealed_keys()
                    .expect("a sealed telemetry map has its key set");
                for key in sealed {
                    if !declared.contains(key.as_str()) {
                        return Err(HaresError::Control(format!(
                            "actor '{name}' telemetry holds undeclared key '{key}'; \
                             declare it in output_variables or drop it"
                        )));
                    }
                }
                for key in &declared {
                    if !sealed.contains(*key) {
                        return Err(HaresError::Control(format!(
                            "actor '{name}' declares output variable '{key}' but its \
                             telemetry does not hold it"
                        )));
                    }
                }
                Ok(())
            }
        }
    }

    /// Derives every roster cache from the prospective `equipment` and
    /// `actors` without touching `self`. Every fallible decision of a
    /// roster change happens here: an actor that cannot act on the
    /// prospective equipment, an output-schema drift, a fluid-loop
    /// conflict, and the creation of the output recorder (which creates
    /// the output file).
    fn plan_roster_caches(
        &self,
        equipment: &[&dyn Equipment],
        actors: &[&dyn Actor],
    ) -> Result<RosterPlan> {
        let prospective = actor_equipment(equipment);
        for actor in actors {
            actor.validate_equipment(&prospective)?;
        }
        let equipment_id_by_name: HashMap<String, EquipmentId> = equipment
            .iter()
            .map(|eq| {
                let desc = eq.descriptor();
                (desc.name.clone(), desc.id)
            })
            .collect();
        let ambient_locations = self
            .environment
            .plan_ambient_locations(equipment.iter().filter_map(|eq| eq.ambient_location()))?;

        let pre_step = if self.clock.current_step == 0 {
            let declarations: Vec<PortDeclaration> = equipment
                .iter()
                .flat_map(|eq| eq.ports())
                .copied()
                .collect();
            let fluid_loop_types = hares_envelope::fluid_solver::plan_loop_types(
                &hares_types::ports::fluid_loop_declarations(&declarations),
            )?;
            Some(PreStepRosterCaches {
                ports: Self::rebuild_port_slots(&declarations, &self.ports),
                rollback_ports: Self::rebuild_port_slots(&declarations, &self.rollback_ports),
                fluid_loop_types,
            })
        } else {
            None
        };
        let ports = pre_step.as_ref().map_or(&self.ports, |p| &p.ports);
        let hvac_thermal_consistency = build_hvac_thermal_consistency(ports, equipment);

        // An actor whose telemetry is not built from its declarations, or
        // whose telemetry holds a key the declarations omit, is rejected at
        // registration: no per-step check is needed.
        for &actor in actors {
            Self::validate_actor_telemetry(actor)?;
        }

        // The catalogue merges every source's typed declarations. Built at
        // every plan point (assembly, and again on an equipment mutation or
        // actor registration), readable from build on, before the first
        // step; a duplicate name or a contradicting aggregation fails the
        // change here.
        let catalogue = Arc::new(
            build_catalogue(&Self::catalogue_input(equipment, actors, self.zone_names())).map_err(
                |err| HaresError::InvalidState(format!("output variable catalogue: {err}")),
            )?,
        );

        let output = if self.reports.is_empty() && !self.sim_config.metrics {
            OutputCachesPlan::Disabled
        } else if self
            .recorders
            .first()
            .map_or(0, StreamingRecorder::total_rows)
            .max(
                self.metrics_calculator
                    .as_ref()
                    .map_or(0, |c| c.rows_fed() as usize),
            )
            == 0
        {
            self.plan_fresh_schema(equipment, actors, &catalogue)?
        } else {
            OutputCachesPlan::FrozenUpdate(Box::new(FrozenOutputCaches {
                bindings: build_output_row_bindings(
                    &self.roster.union_schema,
                    equipment,
                    actors,
                    &self.roster.expected_columns,
                )?,
            }))
        };

        // The freeze-time column index of the schema the zone columns
        // resolve against, rebuilt here (this is the plan builder; the step
        // path never sees one).
        let column_index = match &output {
            OutputCachesPlan::FreshSchema(fresh) => build_output_column_index(&fresh.union_schema),
            OutputCachesPlan::Disabled | OutputCachesPlan::FrozenUpdate(_) => {
                build_output_column_index(&self.roster.union_schema)
            }
        };
        let zones = build_zone_column_caches(
            &self.latest_env.zones,
            self.environment.zone_types(),
            self.thermal_solver.config().indoor_zone_id,
            &column_index,
        );

        let joining_core_outputs = equipment
            .iter()
            .map(|eq| eq.descriptor().id)
            .zip(equipment)
            .filter(|(id, _)| !self.latest_env.equipment_core.contains_key(id))
            .map(|(id, eq)| (id, eq.core_output().clone()))
            .collect();

        Ok(RosterPlan {
            equipment_id_by_name,
            active_equipment_ids: equipment.iter().map(|eq| eq.descriptor().id).collect(),
            dispatch_targets: compute_equipment_dispatch_targets(equipment),
            ambient_locations,
            pre_step,
            equipment_execution_order: compute_equipment_execution_order(equipment),
            equipment_ids: equipment.iter().map(|eq| eq.descriptor().id).collect(),
            joining_core_outputs,
            hvac_thermal_consistency,
            zones,
            output,
            catalogue,
        })
    }

    /// The output schemas rebuilt for the prospective roster from its
    /// catalogue — the union schema the bindings bind against, and one
    /// schema per report — the row plan and producer bindings frozen from
    /// them, and one recorder per report. Creating the recorders is the
    /// plan's last fallible step, and each recorder's freeze-time checks
    /// (the existing-target and parent-directory checks of its file
    /// destination) run here, before the first step.
    fn plan_fresh_schema(
        &self,
        equipment: &[&dyn Equipment],
        actors: &[&dyn Actor],
        catalogue: &Catalogue,
    ) -> Result<OutputCachesPlan> {
        // Every report's selection resolves against the catalogue first:
        // the variables each report records, in its own column order. The
        // diagnostics request appends the catalogue's diagnostics block to
        // every report's columns as a suffix (the block is contiguous
        // last in the report's schema, after the selection's own slots).
        let diagnostics_names = self
            .diagnostics
            .then(|| {
                resolve_selection(&Selection::Diagnostics, catalogue)
                    .map_err(|err| HaresError::Dwelling(err.to_string()))
            })
            .transpose()?;
        let resolved: Vec<Vec<String>> = self
            .reports
            .iter()
            .map(|report| {
                let mut names = resolve_selection(&report.selection, catalogue)
                    .map_err(|err| HaresError::Dwelling(err.to_string()))?;
                if let Some(block) = &diagnostics_names {
                    names.extend(block.iter().cloned());
                }
                Ok(names)
            })
            .collect::<Result<_>>()?;
        // The union surface: every selected variable once, first-seen
        // across the reports in `reports` order, each report's resolved
        // names in its own order inside its report. The producer bindings
        // bind against it; each report reads its own columns out of the
        // union row through its projection.
        //
        // When the run requests metrics, the union is instead the row
        // plan's full declared surface: every catalogue variable the
        // producers can produce (the diagnostics block appended when the
        // run requests it), whatever the reports select. The metrics'
        // inputs resolve from that producer surface, so every metric
        // computes identically at every verbosity and with no report at
        // all; the reports project their own columns out of the full row
        // unchanged, and a run that does not request metrics keeps the
        // reports' union alone.
        let mut union: Vec<String> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for names in &resolved {
            for name in names {
                if seen.insert(name.clone()) {
                    union.push(name.clone());
                }
            }
        }
        if self.sim_config.metrics {
            union = catalogue
                .variables_at_verbosity(u8::MAX)
                .into_iter()
                .map(|def| def.name.to_string())
                .collect();
            if let Some(block) = &diagnostics_names {
                union.extend(block.iter().cloned());
            }
        }
        let union_schema = build_schema_for_columns(catalogue, &union);
        let union_schema = enrich_schema_with_telemetry_units(union_schema, equipment, catalogue);
        let expected_columns: std::collections::HashSet<String> = union.into_iter().collect();

        // One row plan: the union of every report's selected variables
        // mapped to producer slots, one projection per report.
        let name_slots: HashMap<&str, usize> = union_schema
            .fields()
            .iter()
            .skip(1)
            .enumerate()
            .map(|(slot, field)| (field.name().as_str(), slot))
            .collect();
        let projections: Vec<Vec<usize>> = resolved
            .iter()
            .map(|names| {
                names
                    .iter()
                    .map(|name| {
                        name_slots.get(name.as_str()).copied().unwrap_or_else(|| {
                            panic!(
                                "report column '{name}' resolved against the catalogue \
                                 is absent from the union schema"
                            )
                        })
                    })
                    .collect()
            })
            .collect();
        // The union schema's value columns' declared units ride the plan:
        // the write converts each producer's SI value once.
        let units: Vec<hares_types::Unit> = union_schema
            .fields()
            .iter()
            .skip(1)
            .map(|field| {
                field
                    .metadata()
                    .get("unit")
                    .and_then(|symbol| hares_types::Unit::from_symbol(symbol))
                    .unwrap_or(hares_types::Unit::None)
            })
            .collect();
        let row_plan = RowPlan::from_resolved_projections(
            union_schema.fields().len().saturating_sub(1),
            units,
            projections,
        );

        // The producer bindings, against the union schema.
        let bindings =
            build_output_row_bindings(&union_schema, equipment, actors, &expected_columns)?;

        // One recorder per report, each with its own schema: a verbosity
        // selection's schema is the catalogue's schema at that tier; a
        // named or pattern selection's schema spans its resolved columns.
        let mut recorders: Vec<StreamingRecorder> = self
            .reports
            .iter()
            .zip(&resolved)
            .map(|(report, names)| {
                let schema = match &report.selection {
                    Selection::Verbosity(verbosity) => {
                        build_schema_from_catalogue(catalogue, *verbosity, self.diagnostics)
                    }
                    Selection::Diagnostics => build_schema_for_diagnostics(catalogue),
                    Selection::Named(_) | Selection::Pattern(_) => {
                        build_schema_for_columns(catalogue, names)
                    }
                };
                let schema = enrich_schema_with_telemetry_units(schema, equipment, catalogue);
                // The run's site offset joins the catalogue digest in the
                // schema metadata: the one key the builders cannot know,
                // the sign that recovers an instant from the Time column's
                // naive local wall clock.
                let mut metadata = (*schema.metadata()).clone();
                metadata.insert(
                    "hares:utc_offset_s".to_string(),
                    i64::from(self.utc_offset_s).to_string(),
                );
                let schema = schema.with_metadata(metadata);
                StreamingRecorder::new(schema, self.output_chunk_size, report.destination.clone())
            })
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|err| HaresError::Io(format!("output recorder init failed: {err}")))?;

        // The run metrics' one path: when the run requests them, a fixed
        // metrics input plan resolves the metrics' columns against the
        // producer surface (the row plan's full declared surface, built
        // above), and the accumulator is fed per step from the row plan's
        // producer slots. A plan that cannot resolve fails the freeze
        // (the run's numbers are either computed or the run is refused);
        // a run without the request builds no plan and no accumulator and
        // pays nothing.
        let metrics_calculator = if self.sim_config.metrics {
            let available: Vec<String> = union_schema
                .fields()
                .iter()
                .skip(1)
                .map(|f| f.name().clone())
                .collect();
            let time_res_secs = self.sim_config.time_res_secs_u32().map_err(|err| {
                HaresError::InvalidState(format!("metrics calculator time resolution: {err}"))
            })?;
            let plan = hares_io::MetricsInputPlan::build(
                &available,
                &self.sim_config,
                catalogue.equipment_end_uses(),
            )
            .map_err(|err| {
                HaresError::InvalidState(format!("metrics calculator init failed: {err}"))
            })?;
            Some(
                MetricsCalculator::new(plan, time_res_secs, &self.sim_config).map_err(|err| {
                    HaresError::InvalidState(format!("metrics calculator init failed: {err}"))
                })?,
            )
        } else {
            None
        };
        // An attached batch callback survives the rebuild: the dwelling
        // holds the attachment and hands a clone to the named report's
        // fresh recorder (the recorders are in `reports` order).
        if let Some(attachment) = &self.attached_callback {
            let report_index = self
                .reports
                .iter()
                .position(|spec| spec.name == attachment.report());
            if let Some(recorder) = report_index.and_then(|index| recorders.get_mut(index)) {
                recorder.attach_callback(attachment.clone());
            }
        }
        Ok(OutputCachesPlan::FreshSchema(Box::new(FreshOutputCaches {
            recorders,
            row_plan,
            bindings,
            union_schema,
            expected_columns,
            metrics_calculator,
        })))
    }

    /// Installs a plan computed by [`Self::plan_roster_caches`]. Every
    /// fallible decision, and every read of equipment output, already
    /// happened in the plan, so nothing here can fail or run equipment
    /// code; the only actor code it runs is the name accessor and
    /// [`Actor::resolve_equipment_id`], which must not panic. Every
    /// entrance commits its new equipment and actor rosters immediately
    /// before calling this.
    fn install_roster_caches(&mut self, plan: RosterPlan) {
        // Every roster change funnels here (add, remove, replace, actor
        // changes, the assembly install). The step path's persistent
        // stage-rank scratch (`stepped_stage_ranks`) is deliberately NOT
        // part of the plan and is not cleared here: the scratch is
        // write-only within a step and re-derived before every read (the
        // next step's equipment phase clears it before its first push),
        // so no rank recorded under the departing roster can reach the
        // stage-ordering check under the new one. Clearing here would
        // not strengthen that guarantee, and the roster changes that
        // arrive mid-run keep the buffer's capacity warm.
        self.equipment_id_by_name = plan.equipment_id_by_name;
        self.roster.catalogue = plan.catalogue;
        self.active_equipment_ids = plan.active_equipment_ids;
        // Every actor's equipment binding goes stale on any identity change
        // (a replacement receives a new never-reused id, and an actor
        // registered before its equipment existed had no binding), and a
        // stale binding silently reads the old id's core output.
        let id_by_name = &self.equipment_id_by_name;
        let equipment_refs: Vec<&dyn Equipment> =
            self.equipment.iter().map(|eq| eq.as_ref()).collect();
        let equipment = actor_equipment(&equipment_refs);
        for actor in &mut self.actors {
            actor.resolve_equipment_id(id_by_name);
            actor.resolve_equipment(&equipment);
        }
        // One timing slot per registered actor, in the live actor order the
        // scheduler's slots index into; totals restart with the roster.
        #[cfg(feature = "profiling")]
        {
            self.actor_timings = ActorTimings::new(self.actors.iter().map(|a| Arc::from(a.name())));
        }
        self.roster.equipment_execution_order = plan.equipment_execution_order;
        // Each surviving equipment carries its failure streak across the
        // change (an entering one has failed no step yet), and the
        // latest-step failure flags clear.
        let streak_by_id: HashMap<EquipmentId, u32> = self
            .roster
            .equipment_ids
            .iter()
            .copied()
            .zip(self.consecutive_step_failures.iter().copied())
            .collect();
        self.consecutive_step_failures = plan
            .equipment_ids
            .iter()
            .map(|id| streak_by_id.get(id).copied().unwrap_or(0))
            .collect();
        self.step_failed = vec![false; plan.equipment_ids.len()];
        self.roster.equipment_ids = plan.equipment_ids;
        // The environment snapshot follows the roster at once: removed
        // equipment leaves it, and joining equipment is seeded with its own
        // current core output (identical to what the end-of-step snapshot
        // will write) so actors reading it are not blind for the joining
        // step.
        let live_ids = &self.roster.equipment_ids;
        self.latest_env
            .equipment_core
            .retain(|id, _| live_ids.contains(id));
        let live_names = &self.equipment_id_by_name;
        self.latest_env.equipment_telemetry.retain(|name, _| {
            name == tk::HUMIDITY_SOLVER_TELEMETRY_KEY || live_names.contains_key(name)
        });
        self.latest_env
            .equipment_core
            .extend(plan.joining_core_outputs);
        self.solver_feedback_actor
            .set_dispatch_targets(plan.dispatch_targets);
        self.environment
            .install_ambient_locations(plan.ambient_locations);
        if let Some(pre_step) = plan.pre_step {
            self.ports = pre_step.ports;
            self.rollback_ports = pre_step.rollback_ports;
            self.fluid_solver
                .install_loop_types(pre_step.fluid_loop_types);
        }
        self.roster.hvac_thermal_consistency = plan.hvac_thermal_consistency;
        self.roster.zone_temp_scratch = plan
            .zones
            .sorted_zone_ids
            .iter()
            .map(|&z| (z, 0.0))
            .collect();
        self.roster.zones = plan.zones;

        match plan.output {
            OutputCachesPlan::Disabled => {
                self.metrics_calculator = None;
            }
            OutputCachesPlan::FrozenUpdate(frozen) => {
                self.roster.bindings = frozen.bindings;
            }
            OutputCachesPlan::FreshSchema(fresh) => {
                self.recorders = fresh.recorders;
                self.metrics_calculator = fresh.metrics_calculator;
                self.roster.row_plan = fresh.row_plan;
                self.roster.record_projections = self.roster.row_plan.projections().to_vec();
                self.roster.union_schema = fresh.union_schema;
                self.roster.expected_columns = fresh.expected_columns;
                self.roster.bindings = fresh.bindings;
                #[cfg(feature = "observe")]
                self.log_output_keys_without_columns();
            }
        }
    }

    /// Reports output-scoped telemetry keys that have no output column, so
    /// operators can audit coverage: a key marked `scope: output` that no
    /// equipment publishes to a visible column is silently absent from
    /// output. Not necessarily a bug (the publishing equipment may be
    /// absent, or the verbosity too low).
    #[cfg(feature = "observe")]
    fn log_output_keys_without_columns(&self) {
        let mut covered: HashSet<&str> = HashSet::new();
        for cols in &self.roster.bindings.equipment {
            for &(key, _idx) in cols.v8_state_columns.iter().chain(&cols.v8_flow_columns) {
                covered.insert(key);
            }
            let named = [
                (cols.defrost_state.is_some(), tk::DEFROST_CYCLE_STATE),
                (cols.er_power.is_some(), tk::BACKUP_ER_KW),
                (cols.shr.is_some(), tk::SHR),
                (cols.fan_power.is_some(), tk::FAN_KW),
                (cols.runtime_fraction.is_some(), tk::RUNTIME_FRACTION),
                (cols.latent_gains.is_some(), tk::LATENT_GAINS_W),
                (cols.duct_losses.is_some(), tk::DUCT_LOSS_W),
            ];
            covered.extend(
                named
                    .into_iter()
                    .filter_map(|(has, key)| has.then_some(key)),
            );
        }
        if self
            .roster
            .bindings
            .dwelling
            .scheduled_heating_setpoint
            .is_some()
        {
            covered.insert(tk::SCHEDULE_HEATING_SETPOINT_C);
            covered.insert(tk::SCHEDULE_COOLING_SETPOINT_C);
            covered.insert(tk::RUNTIME_HEATING_SETPOINT_C);
            covered.insert(tk::RUNTIME_COOLING_SETPOINT_C);
        }
        for actor in &self.actors {
            if let Some(tel) = actor.telemetry() {
                covered.extend(tel.0.keys().map(String::as_str));
            }
        }
        for &key in tk::OUTPUT_SCOPE_KEYS {
            if !covered.contains(key) {
                tracing::info!(
                    target: "hares::output::coverage",
                    key = key,
                    "output-scoped telemetry key has no output column in the run's reports",
                );
            }
        }
    }
}
