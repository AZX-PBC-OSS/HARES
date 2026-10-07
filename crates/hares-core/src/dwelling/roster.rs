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
use hares_io::{StreamingRecorder, build_schema};
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
    ActorPricing, ActorSeedState, Dwelling, EquipmentColumns, Result, ZoneColumnCaches,
    actor_equipment, build_actor_column_map, build_actors_from_seeds,
    build_end_use_aggregate_indices, build_equipment_column_map, build_hvac_thermal_consistency,
    build_zone_column_caches, compute_equipment_dispatch_targets,
    compute_equipment_execution_order, describe_target, enrich_schema_with_telemetry_units,
    equipment_descriptor_specs, extend_schema_with_actor_columns, target_lost,
    validate_equipment_stage, validate_equipment_zones, zone_display_name,
};

/// The values the per-step path reads that derive from the equipment and
/// actor rosters. Replaced only by [`Dwelling::install_roster_caches`];
/// empty until assembly installs the first plan.
#[derive(Default)]
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
    /// Output column name to position; empty when output is disabled.
    pub(super) output_column_index: HashMap<String, usize>,
    /// Number of value columns the recorder expects (schema minus timestamp).
    pub(super) output_value_count: usize,
    /// Output columns of each equipment, in equipment-vector order.
    pub(super) equipment_column_map: Vec<EquipmentColumns>,
    /// End-use aggregate electric-power column of each equipment, in
    /// equipment-vector order.
    pub(super) end_use_aggregate_indices: Vec<Option<usize>>,
    /// Telemetry key to output column of each actor, in actor order.
    pub(super) actor_column_map: Vec<Vec<(String, usize)>>,
    /// Row buffer for `record_step`, reused every step.
    pub(super) record_scratch: Vec<f64>,
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
    /// Output disabled: nothing to plan or install.
    Disabled,
    /// No rows recorded yet: the schema is rebuilt from the prospective
    /// roster, with a recorder created for it.
    FreshSchema {
        recorder: Box<StreamingRecorder>,
        metrics_warning: Option<String>,
        output_column_index: HashMap<String, usize>,
        equipment_column_map: Vec<EquipmentColumns>,
        end_use_aggregate_indices: Vec<Option<usize>>,
        actor_column_map: Vec<Vec<(String, usize)>>,
    },
    /// Rows already recorded: the schema and column index are frozen with
    /// the output file; only the positional maps into them follow the
    /// prospective roster. The maps resolve columns by name against the
    /// frozen index, so surviving entities keep their own columns and
    /// entities added after recording began get none: missing values,
    /// never values written into another entity's columns.
    FrozenUpdate {
        equipment_column_map: Vec<EquipmentColumns>,
        end_use_aggregate_indices: Vec<Option<usize>>,
        actor_column_map: Vec<Vec<(String, usize)>>,
    },
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
        Ok(())
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
            self.warnings.push(warning);
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
        let parse_warnings = tariff.parse_warnings.clone();
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

    fn zone_names(&self) -> Vec<(ZoneId, String)> {
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

        let output = if !self.write_output {
            OutputCachesPlan::Disabled
        } else if self
            .recorder
            .as_ref()
            .map_or(0, StreamingRecorder::total_rows)
            == 0
        {
            self.plan_fresh_schema(equipment, actors)?
        } else {
            let column_index = &self.roster.output_column_index;
            OutputCachesPlan::FrozenUpdate {
                equipment_column_map: build_equipment_column_map(
                    equipment,
                    column_index,
                    self.output_verbosity,
                )?,
                end_use_aggregate_indices: build_end_use_aggregate_indices(
                    &equipment_descriptor_specs(equipment),
                    column_index,
                ),
                actor_column_map: build_actor_column_map(actors, column_index),
            }
        };

        let column_index = match &output {
            OutputCachesPlan::FreshSchema {
                output_column_index,
                ..
            } => output_column_index,
            OutputCachesPlan::Disabled | OutputCachesPlan::FrozenUpdate { .. } => {
                &self.roster.output_column_index
            }
        };
        let zones = build_zone_column_caches(
            &self.latest_env.zones,
            self.environment.zone_types(),
            self.thermal_solver.config().indoor_zone_id,
            column_index,
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
        })
    }

    /// The output schema rebuilt for the prospective roster, its column
    /// maps, and a recorder created for it. Creating the recorder is the
    /// plan's last fallible step.
    fn plan_fresh_schema(
        &self,
        equipment: &[&dyn Equipment],
        actors: &[&dyn Actor],
    ) -> Result<OutputCachesPlan> {
        let specs = equipment_descriptor_specs(equipment);
        let schema: Schema = build_schema(&specs, self.output_verbosity, &self.zone_names());
        // Enriched before the actor columns are appended: actor telemetry
        // columns carry no declared-unit metadata to validate against.
        let schema = enrich_schema_with_telemetry_units(schema, equipment);
        let schema = extend_schema_with_actor_columns(&schema, actors);
        let output_column_index = build_output_column_index(&schema);
        let equipment_column_map =
            build_equipment_column_map(equipment, &output_column_index, self.output_verbosity)?;
        let end_use_aggregate_indices =
            build_end_use_aggregate_indices(&specs, &output_column_index);
        let actor_column_map = build_actor_column_map(actors, &output_column_index);
        let mut recorder = StreamingRecorder::new(
            schema,
            self.output_chunk_size,
            self.output_format,
            &self.output_path,
            self.retain_batches,
            self.output_rotation,
        )
        .map_err(|err| HaresError::Io(format!("output recorder init failed: {err}")))?;
        // Streaming runs collect run metrics incrementally at flush time; a
        // calculator that cannot initialize zeroes the metrics with a
        // warning rather than rejecting the change.
        let metrics_warning = if self.retain_batches {
            None
        } else {
            recorder
                .enable_metrics(self.sim_config.time_res_secs_u32(), &self.sim_config)
                .err()
                .map(|err| format!("MetricsCalculator init failed: {err} -- metrics are zeroed"))
        };
        Ok(OutputCachesPlan::FreshSchema {
            recorder: Box::new(recorder),
            metrics_warning,
            output_column_index,
            equipment_column_map,
            end_use_aggregate_indices,
            actor_column_map,
        })
    }

    /// Installs a plan computed by [`Self::plan_roster_caches`]. Every
    /// fallible decision, and every read of equipment output, already
    /// happened in the plan, so nothing here can fail or run equipment
    /// code; the only actor code it runs is the name accessor and
    /// [`Actor::resolve_equipment_id`], which must not panic. Every
    /// entrance commits its new equipment and actor rosters immediately
    /// before calling this.
    fn install_roster_caches(&mut self, plan: RosterPlan) {
        self.equipment_id_by_name = plan.equipment_id_by_name;
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
            OutputCachesPlan::Disabled => {}
            OutputCachesPlan::FrozenUpdate {
                equipment_column_map,
                end_use_aggregate_indices,
                actor_column_map,
            } => {
                self.roster.equipment_column_map = equipment_column_map;
                self.roster.end_use_aggregate_indices = end_use_aggregate_indices;
                self.roster.actor_column_map = actor_column_map;
            }
            OutputCachesPlan::FreshSchema {
                recorder,
                metrics_warning,
                output_column_index,
                equipment_column_map,
                end_use_aggregate_indices,
                actor_column_map,
            } => {
                self.roster.output_value_count = recorder.schema().fields().len() - 1;
                self.roster.output_column_index = output_column_index;
                self.roster.equipment_column_map = equipment_column_map;
                self.roster.end_use_aggregate_indices = end_use_aggregate_indices;
                self.roster.actor_column_map = actor_column_map;
                self.roster.record_scratch.clear();
                self.roster
                    .record_scratch
                    .resize(self.roster.output_value_count, 0.0);
                if let Some(warning) = metrics_warning {
                    self.push_warning(warning);
                }
                self.recorder = Some(*recorder);
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
        for cols in &self.roster.equipment_column_map {
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
            .output_column_index
            .contains_key(hares_io::SCHEDULED_HEATING_SETPOINT_COL)
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
                    "output-scoped telemetry key has no output column at verbosity {}",
                    self.output_verbosity,
                );
            }
        }
    }
}
