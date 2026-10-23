//! Checkpoint save, and validated restore that leaves the dwelling unchanged
//! when any part of the checkpoint is rejected.

use std::collections::HashMap;

use chrono_tz::Tz;
use hares_envelope::FluidSolver;
use hares_io::{Destination, MetricsCalculator};
use hares_types::{EnvironmentState, HaresError, ZoneId};
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

use super::Dwelling;
use crate::checkpoint::{
    ActorStateCheckpoint, CHECKPOINT_VERSION, DwellingCheckpoint, EquipmentStateCheckpoint,
    ReportSection,
};
use crate::rng::{EV_DRIVER_STREAM_COUNT, RNG_STREAM_EV_DRIVER_BASE};
use hares_tariff::{BillingPeriodSummary, TariffEvaluator, TariffSnapshot};

type Result<T> = std::result::Result<T, HaresError>;

impl Dwelling {
    /// Snapshot current simulation state to an in-memory checkpoint struct.
    pub fn save_checkpoint(&self) -> Result<DwellingCheckpoint> {
        if let Some(ended) = &self.terminal_error {
            return Err(HaresError::Simulation(format!(
                "the run ended part way through a step, so its state is not a \
                 resumable checkpoint: {ended}"
            )));
        }
        // humidity_ratios is a HashMap; sort by zone so serialized checkpoints
        // of identical state are byte-identical (restore is order-insensitive).
        let mut humidity_states: Vec<(ZoneId, f64)> = self
            .humidity_solver
            .humidity_ratios
            .iter()
            .map(|(zone, value)| (*zone, *value))
            .collect();
        humidity_states.sort_by_key(|&(zone, _)| zone);
        let fluid_states = self.fluid_solver.snapshot_payload();

        if self.consecutive_step_failures.len() != self.equipment.len() {
            return Err(HaresError::InvalidState(format!(
                "{} failure streaks for {} equipment: the streaks are index-aligned \
                 with the equipment",
                self.consecutive_step_failures.len(),
                self.equipment.len()
            )));
        }
        let mut equipment_states = Vec::with_capacity(self.equipment.len());
        for (eq, &consecutive_step_failures) in
            self.equipment.iter().zip(&self.consecutive_step_failures)
        {
            match eq.save_state() {
                Ok(blob) => {
                    let desc = eq.descriptor();
                    equipment_states.push(EquipmentStateCheckpoint {
                        name: desc.name.clone(),
                        equipment_id: desc.id.0,
                        consecutive_step_failures,
                        blob,
                    });
                }
                Err(e) => {
                    #[cfg(feature = "observe")]
                    tracing::error!(
                        equipment_name = %eq.descriptor().name,
                        equipment_type = %eq.descriptor().equipment_type,
                        equipment_id = %eq.descriptor().id,
                        error = %e,
                        bldg_id = self.bldg_id,
                        "checkpoint save_state failed for equipment",
                    );
                    return Err(e);
                }
            }
        }

        Ok(DwellingCheckpoint {
            format_version: CHECKPOINT_VERSION,
            bldg_id: self.bldg_id,
            timestep_index: self.clock.current_step(),
            env_equipment_telemetry: self.latest_env.equipment_telemetry.clone(),
            env_equipment_core: self.latest_env.equipment_core.clone(),
            equipment_states,
            rng_state: self.rng.get_seed(),
            thermal: self.thermal_solver.snapshot_state(),
            humidity_states,
            fluid_states,
            rng_stream: self.rng.get_stream(),
            rng_word_pos: self.rng.get_word_pos(),
            actor_states: self
                .actors
                .iter()
                .map(|a| {
                    a.save_state().map(|blob| ActorStateCheckpoint {
                        name: a.name().to_string(),
                        schema_version: a.checkpoint_version(),
                        blob,
                    })
                })
                .collect::<Result<Vec<_>>>()?,
            prior_electrical_summary: self.prior_electrical_summary.clone(),
            next_ev_driver_stream: self.next_ev_driver_stream,
            tariff_state: match self.tariff_evaluator.as_ref() {
                Some(evaluator) => Some(evaluator.snapshot_state().to_blob()?),
                None => None,
            },
            billing_summaries: self
                .billing_summaries
                .iter()
                .map(BillingPeriodSummary::snapshot)
                .collect(),
            metrics: self
                .metrics_calculator
                .as_ref()
                .map(MetricsCalculator::snapshot),
            reports: self
                .recorders
                .iter()
                .zip(self.reports.iter())
                .map(|(recorder, spec)| ReportSection {
                    name: spec.name.clone(),
                    rows: recorder.rows_snapshot(),
                })
                .collect(),
            health: self.health.clone(),
            warnings: self.warnings.snapshot(),
        })
    }

    /// Restore simulation state from a checkpoint.
    ///
    /// Every part of the checkpoint is validated against this dwelling
    /// before anything changes. Equipment and actor blobs can only be
    /// checked by decoding them, so they load first; if one fails, every
    /// equipment and actor is reloaded with the state it held before the
    /// call. On any error the dwelling is therefore left as it was.
    ///
    /// The restore replaces all of a run's state, so it also resumes a run
    /// that ended part way through a step (the failure budget): the
    /// half-stepped port contributions are dropped and stepping is allowed
    /// again once the restore has succeeded. The finished-run latch is
    /// part of that state: a restored dwelling's later `finish()`
    /// finalises the restored run and returns its own result, never a
    /// result recorded before the restore.
    pub fn load_checkpoint(&mut self, cp: DwellingCheckpoint) -> Result<()> {
        // The stage-rank scratch needs no invalidation here: the restore
        // validates the checkpoint's equipment against this dwelling's
        // live roster (`matched_equipment_states`) and never rebuilds the
        // roster, the buffer is not checkpoint state, and the next step's
        // equipment phase clears the buffer before its first push (the
        // field's doc carries the full proof). A restored step therefore
        // cannot read a rank from the pre-restore run.
        self.validate_building_state(&cp)?;
        // The run-surface sections validate against this dwelling before
        // anything changes: a report the dwelling cannot resume, a
        // section without its live counterpart (or the reverse), a metrics
        // plan that resolves differently, or a summary in a foreign zone
        // is a typed error leaving the dwelling as it was. The tariff
        // snapshot decodes here too; the evaluator rebuild below reuses it.
        let (tariff_snapshot, evaluator_tz) = self.validate_run_sections(&cp)?;
        let equipment_states = self.matched_equipment_states(&cp)?;
        let equipment_blobs: Vec<&[u8]> = equipment_states
            .iter()
            .map(|state| state.blob.as_slice())
            .collect();
        let actor_blobs = self.matched_actor_blobs(&cp)?;
        // The tariff evaluator rebuilds from its decoded snapshot before
        // anything mutates, so a snapshot that fails to restore leaves the
        // dwelling as it was.
        let tariff_evaluator = tariff_snapshot
            .as_ref()
            .map(TariffEvaluator::from_snapshot)
            .transpose()?;
        // The closed periods' summaries resolve in the restored tariff's
        // zone before anything changes.
        let billing_summaries = cp
            .billing_summaries
            .iter()
            .map(|summary| BillingPeriodSummary::from_snapshot(summary.clone(), evaluator_tz))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let last_step_env = self.last_step_environment(cp.timestep_index)?;
        self.load_component_states(&equipment_blobs, &actor_blobs, |dwelling| {
            dwelling.check_ev_driver_cursor(cp.next_ev_driver_stream)
        })?;
        self.consecutive_step_failures = equipment_states
            .iter()
            .map(|state| state.consecutive_step_failures)
            .collect();

        if let Some(env) = last_step_env {
            self.latest_env = env;
        }
        self.apply_building_state(&cp)?;
        self.clock.current_step = cp.timestep_index;
        let mut restored_rng = ChaCha8Rng::from_seed(cp.rng_state);
        restored_rng.set_stream(cp.rng_stream);
        restored_rng.set_word_pos(cp.rng_word_pos);
        self.rng = restored_rng;
        self.prior_electrical_summary = cp.prior_electrical_summary;
        self.next_ev_driver_stream = cp.next_ev_driver_stream;
        // The tariff evaluator's state restores exactly: a resumed dwelling
        // with a tariff prices and bills the post-restore steps as the
        // continuous run would, open period and accruals included. A
        // checkpoint without a tariff leaves the dwelling without one. The
        // prior-segment path (`restore_building_state`) does not restore the
        // tariff: it resets the clock to step 0 and the segment attaches its
        // own tariff as assembly does.
        self.tariff_evaluator = tariff_evaluator;
        self.billing_summaries = billing_summaries;
        // The metrics accumulator's state restores exactly: the fresh
        // calculator this run built keeps its own plan and config, and the
        // checkpoint's accumulators overwrite its (zeroed) state, so the
        // resumed run's finish() is the continuous run's. The plan was
        // validated equal before anything changed.
        if let (Some(calculator), Some(snapshot)) =
            (self.metrics_calculator.as_mut(), cp.metrics.as_ref())
        {
            calculator
                .apply_snapshot(snapshot)
                .map_err(|err| HaresError::InvalidState(format!("metrics restore: {err}")))?;
        }
        // The reports' row streams restore: every `Memory` report holds the
        // written run's rows, the dwelling's own rows past the checkpoint
        // step discarded with them, so the resumed run's frame is the
        // continuous run's.
        for (recorder, section) in self.recorders.iter_mut().zip(&cp.reports) {
            recorder.restore_rows(&section.rows).map_err(|err| {
                HaresError::Io(format!("report '{}' restore: {err}", section.name))
            })?;
        }
        self.health = cp.health;
        self.warnings.restore(cp.warnings.clone());

        // Populate latest_env.equipment_core and equipment_telemetry from the
        // restored equipment state so that actors read correct SOC, power flows,
        // and connection state on the first post-restore step; then overwrite
        // with the checkpoint's own carried maps: the actor-facing snapshots
        // the saved run's actors read are the exact state to resume from,
        // wherever an equipment's rebuilt telemetry differs from its
        // committed one.
        self.snapshot_equipment_state();
        self.latest_env.equipment_telemetry = cp.env_equipment_telemetry;
        self.latest_env.equipment_core = cp.env_equipment_core;
        self.restored_from_checkpoint = true;
        self.step_failed.fill(false);
        // The restore replaces all of a run's state, the finished-run latch
        // included: without this, a finished dwelling that loads a mid-run
        // checkpoint stays finished forever, every step a typed error and
        // finish() handing back the pre-restore result.
        self.run_result = None;
        if self.terminal_error.take().is_some() {
            self.ports.zero();
        }
        self.record_replay_blocker(hares_types::ReplayBlocker::StateRestored);
        Ok(())
    }

    /// Restore building shell state from a prior-segment checkpoint.
    ///
    /// Transfers the thermal envelope, humidity and fluid solver state and
    /// the prior electrical summary, and resets the clock to the start of
    /// the segment. The RNG, equipment and actor states are not restored:
    /// the current equipment set was freshly built by
    /// `DwellingBlueprint::build()` and already initialized via
    /// `Equipment::init()`. The tariff evaluator's state is not restored
    /// either (the clock resets to step 0; the segment attaches its own
    /// tariff as assembly does). The checkpoint is validated before anything
    /// changes, so on error the dwelling is left as it was. The segment
    /// re-runs its steps from the start, so a finished run's recorded
    /// result is cleared with the rest of the run's state: the segment's
    /// own `finish()` finalises the re-run.
    ///
    /// Refused on a run that ended part way through a step: its equipment
    /// is half-stepped and this restore keeps it (`load_checkpoint` resumes
    /// such a run).
    pub fn restore_building_state(&mut self, cp: &DwellingCheckpoint) -> Result<()> {
        // The stage-rank scratch needs no invalidation here: the equipment
        // set was freshly built by `DwellingBlueprint::build()` and is
        // untouched by this restore, the buffer is not checkpoint state,
        // and the next step's equipment phase clears it before its first
        // push (the field's doc carries the full proof).
        if let Some(ended) = &self.terminal_error {
            return Err(HaresError::Simulation(format!(
                "the run ended part way through a step and its equipment is half-stepped; \
                 restore the whole checkpoint with load_checkpoint: {ended}"
            )));
        }
        self.validate_building_state(cp)?;
        self.apply_building_state(cp)?;
        self.prior_electrical_summary = cp.prior_electrical_summary.clone();
        self.clock.current_step = 0;
        self.snapshot_equipment_state();
        self.run_result = None;
        self.record_replay_blocker(hares_types::ReplayBlocker::StateRestored);
        Ok(())
    }

    /// The environment snapshot of the step before `timestep_index`, as the
    /// run that wrote the checkpoint holds it between steps: equipment
    /// initialised against it before the next step (an add after a resume)
    /// sees the same time and weather as in that run. Computed on a copy,
    /// so a failure leaves the dwelling's snapshot unchanged. `None` for a
    /// checkpoint taken before the first step.
    fn last_step_environment(&mut self, timestep_index: u64) -> Result<Option<EnvironmentState>> {
        let Some(last_step) = timestep_index.checked_sub(1) else {
            return Ok(None);
        };
        let mut last_step_clock = self.clock.clone();
        last_step_clock.current_step = last_step;
        let mut env = self.latest_env.clone();
        self.environment
            .update_in_place(&mut env, &last_step_clock)?;
        Ok(Some(env))
    }

    /// The run-surface sections' validation, before anything changes: the
    /// reports' destinations and names, the row streams' fit, the metrics
    /// section's presence and plan, and the billing summaries' presence
    /// against the tariff section.
    ///
    /// Returns the decoded tariff snapshot (the evaluator rebuild reuses
    /// it) and the zone its bounds resolve in (the closed summaries'
    /// restore zone); the zone is unused when the checkpoint carries no
    /// tariff and no summaries.
    fn validate_run_sections(
        &self,
        cp: &DwellingCheckpoint,
    ) -> Result<(Option<TariffSnapshot>, Tz)> {
        // A file or callback destination report cannot resume: the written
        // run's rows stream into a partial file (or a caller's callback)
        // no checkpoint carries, so a resumed run's file would hold only
        // the post-resume rows. The refusal names the report.
        for spec in &self.reports {
            if !matches!(spec.destination, Destination::Memory) {
                return Err(HaresError::Io(format!(
                    "checkpoint restore: report '{}' writes to a file or callback \
                     destination, and a checkpoint carries no file state: restore \
                     into a dwelling whose reports are memory reports",
                    spec.name
                )));
            }
        }
        // The sections' names must match this dwelling's reports, in order.
        let checkpoint_names: Vec<&str> = cp.reports.iter().map(|s| s.name.as_str()).collect();
        let dwelling_names: Vec<&str> = self.reports.iter().map(|s| s.name.as_str()).collect();
        if checkpoint_names != dwelling_names {
            return Err(HaresError::Io(format!(
                "checkpoint restore: the reports changed between save and restore: \
                 the checkpoint carries sections for {checkpoint_names:?}, the dwelling \
                 configures {dwelling_names:?}"
            )));
        }
        // The row streams must fit this dwelling's recorders.
        for (section, recorder) in cp.reports.iter().zip(&self.recorders) {
            let width = recorder.schema().fields().len().saturating_sub(1);
            let rows = section.rows.timestamps.len();
            if section.rows.chunk_size != recorder.chunk_size() {
                return Err(HaresError::Io(format!(
                    "checkpoint restore: report '{}' was written with chunk size {}, \
                     this run uses {}",
                    section.name,
                    section.rows.chunk_size,
                    recorder.chunk_size()
                )));
            }
            if section.rows.values.len() != rows * width {
                return Err(HaresError::Io(format!(
                    "checkpoint restore: report '{}' carries {} values for {} rows of \
                     {} columns",
                    section.name,
                    section.rows.values.len(),
                    rows,
                    width
                )));
            }
            if section.rows.buffered_rows > rows {
                return Err(HaresError::Io(format!(
                    "checkpoint restore: report '{}' buffers {} rows beyond its {}",
                    section.name, section.rows.buffered_rows, rows
                )));
            }
            // The batch count must agree with the flushed shape the rows
            // rebuild: the restore would refuse it, and here is before
            // anything changed.
            let expected_batches = section.rows.expected_batches(recorder.chunk_size());
            if section.rows.batches_built != expected_batches {
                return Err(HaresError::Io(format!(
                    "checkpoint restore: report '{}' names {} built batches but its \
                     rows rebuild {expected_batches}",
                    section.name, section.rows.batches_built
                )));
            }
        }
        // The metrics section's presence must match the request, and its
        // plan must resolve the same slots the fresh calculator's does;
        // its time resolution must be this run's (the plan is
        // resolution-free and two resolutions can size the rolling
        // peak's ring identically, so the resolution itself is the
        // gate), and the ring must be the resolution's (the corrupted
        // section the resolution gate cannot see).
        match (&cp.metrics, &self.metrics_calculator) {
            (Some(snapshot), Some(calculator)) => {
                if snapshot.plan != calculator.plan_snapshot() {
                    return Err(HaresError::InvalidState(
                        "checkpoint restore: the metrics section's input plan does not \
                         resolve the same producer slots this run's does"
                            .to_string(),
                    ));
                }
                if snapshot.time_res_secs != calculator.time_res_secs() {
                    return Err(HaresError::InvalidState(format!(
                        "checkpoint restore: the metrics section was written at time \
                         resolution {} s, this run's is {} s",
                        snapshot.time_res_secs,
                        calculator.time_res_secs()
                    )));
                }
                let ring = calculator.rolling_peak_ring_len();
                if snapshot.accumulators.rolling_peak.buffer.len() != ring {
                    return Err(HaresError::InvalidState(format!(
                        "checkpoint restore: the metrics section's rolling peak ring \
                         holds {} readings, this run's time resolution needs {ring}",
                        snapshot.accumulators.rolling_peak.buffer.len()
                    )));
                }
            }
            (Some(_), None) => {
                return Err(HaresError::InvalidState(
                    "checkpoint restore: the checkpoint carries a metrics section but \
                     this run did not request metrics"
                        .to_string(),
                ));
            }
            (None, Some(_)) => {
                return Err(HaresError::InvalidState(
                    "checkpoint restore: this run requested metrics but the checkpoint \
                     carries no metrics section"
                        .to_string(),
                ));
            }
            (None, None) => {}
        }
        // The closed summaries' bounds resolve in the tariff's zone, so a
        // summaries section without a tariff section has no zone to
        // resolve in.
        if !cp.billing_summaries.is_empty() && cp.tariff_state.is_none() {
            return Err(HaresError::Io(
                "checkpoint restore: the checkpoint carries billing summaries without a \
                 tariff state: the summaries' bounds resolve in the tariff's zone"
                    .to_string(),
            ));
        }
        match &cp.tariff_state {
            Some(blob) => {
                let snapshot = TariffSnapshot::from_blob(blob)?;
                let tz: Tz = snapshot.timezone.parse().map_err(|_| {
                    HaresError::Tariff(format!(
                        "tariff snapshot: unknown time zone '{}'",
                        snapshot.timezone
                    ))
                })?;
                Ok((Some(snapshot), tz))
            }
            None => Ok((None, chrono_tz::Tz::UTC)),
        }
    }

    /// Checks the format version and the building physics part of `cp`
    /// (thermal snapshot, a finite non-negative humidity ratio for every
    /// zone, fluid payload) against this dwelling without changing it.
    fn validate_building_state(&self, cp: &DwellingCheckpoint) -> Result<()> {
        if cp.format_version != CHECKPOINT_VERSION {
            return Err(HaresError::Io(format!(
                "checkpoint version mismatch: file={}, expected={}",
                cp.format_version, CHECKPOINT_VERSION
            )));
        }
        self.thermal_solver
            .validate_snapshot(&cp.thermal)
            .map_err(|err| HaresError::Envelope(format!("restore thermal state failed: {err}")))?;
        for zone in &self.latest_env.zones {
            let Some(&(_, humidity)) = cp.humidity_states.iter().find(|(id, _)| *id == zone.id)
            else {
                return Err(HaresError::Io(format!(
                    "checkpoint missing humidity state for zone {:?}",
                    zone.id
                )));
            };
            if !(humidity.is_finite() && humidity >= 0.0) {
                return Err(HaresError::Io(format!(
                    "checkpoint humidity ratio {humidity} for zone {:?} is not a finite \
                     non-negative ratio",
                    zone.id
                )));
            }
        }
        FluidSolver::validate_payload(&cp.fluid_states)
            .map_err(|err| HaresError::Envelope(format!("restore fluid state failed: {err}")))
    }

    /// Applies the building physics part of a checkpoint that
    /// [`Self::validate_building_state`] accepted.
    fn apply_building_state(&mut self, cp: &DwellingCheckpoint) -> Result<()> {
        self.thermal_solver
            .restore_state(&cp.thermal)
            .map_err(|err| HaresError::Envelope(format!("restore thermal state failed: {err}")))?;
        for (zone_id, temp_c) in self.thermal_solver.zone_temperatures_c() {
            if let Some(zone) = self.latest_env.zones.iter_mut().find(|z| z.id == zone_id) {
                zone.temperature_c = temp_c;
            }
        }
        let checkpoint_zones: HashMap<ZoneId, f64> = cp.humidity_states.iter().copied().collect();
        for zone in &mut self.latest_env.zones {
            let humidity = *checkpoint_zones.get(&zone.id).ok_or_else(|| {
                HaresError::Io(format!(
                    "checkpoint missing humidity state for zone {:?}",
                    zone.id
                ))
            })?;
            self.humidity_solver
                .humidity_ratios
                .insert(zone.id, humidity);
            zone.humidity_ratio = humidity;
        }
        self.fluid_solver
            .restore_from_payload(&cp.fluid_states)
            .map_err(|err| HaresError::Envelope(format!("restore fluid state failed: {err}")))
    }

    /// The checkpoint's equipment blobs in this dwelling's equipment order.
    ///
    /// Equipment state restores are identity-keyed, not positional: each
    /// saved blob carries the equipment's name and id, and both must match
    /// the live equipment. A spec reorder between save and restore (or any
    /// other identity drift) fails here, naming both sides, instead of
    /// loading one equipment's state into another.
    fn matched_equipment_states<'cp>(
        &self,
        cp: &'cp DwellingCheckpoint,
    ) -> Result<Vec<&'cp EquipmentStateCheckpoint>> {
        if cp.equipment_states.len() != self.equipment.len() {
            return Err(HaresError::Io(format!(
                "checkpoint equipment count mismatch: checkpoint has {} equipment states, dwelling has {} equipment",
                cp.equipment_states.len(),
                self.equipment.len()
            )));
        }
        let by_name: HashMap<&str, &EquipmentStateCheckpoint> = cp
            .equipment_states
            .iter()
            .map(|state| (state.name.as_str(), state))
            .collect();
        self.equipment
            .iter()
            .map(|eq| {
                let desc = eq.descriptor();
                let Some(&state) = by_name.get(desc.name.as_str()) else {
                    return Err(HaresError::Io(format!(
                        "checkpoint missing equipment state for '{}': the dwelling \
                         and the checkpoint were built from different equipment sets",
                        desc.name
                    )));
                };
                if state.equipment_id != desc.id.0 {
                    return Err(HaresError::Io(format!(
                        "checkpoint equipment id mismatch for '{}': checkpoint id={}, \
                         dwelling id={}: the equipment set or its order changed between \
                         save and restore, and loading state positionally would hand one \
                         equipment another's state",
                        desc.name, state.equipment_id, desc.id.0
                    )));
                }
                Ok(state)
            })
            .collect()
    }

    /// The checkpoint's actor blobs in this dwelling's actor order.
    ///
    /// The saved schema version must equal the live actor's
    /// `checkpoint_version()`, so a blob written against a different
    /// snapshot schema is rejected with a version mismatch naming the actor
    /// rather than as a decode failure. An empty blob is the "no mutable
    /// state" convention, valid only for an actor whose own `save_state` is
    /// empty: for a stateful actor it can only be truncation or corruption,
    /// and loading it would silently discard the decision state.
    fn matched_actor_blobs<'cp>(&self, cp: &'cp DwellingCheckpoint) -> Result<Vec<&'cp [u8]>> {
        let by_name: HashMap<&str, &ActorStateCheckpoint> = cp
            .actor_states
            .iter()
            .map(|state| (state.name.as_str(), state))
            .collect();
        let mut blobs = Vec::with_capacity(self.actors.len());
        for actor in &self.actors {
            let Some(state) = by_name.get(actor.name()) else {
                continue;
            };
            let expected = actor.checkpoint_version();
            if state.schema_version != expected {
                return Err(HaresError::Io(format!(
                    "checkpoint actor schema version mismatch: actor='{}', blob={}, expected={}",
                    actor.name(),
                    state.schema_version,
                    expected
                )));
            }
            if state.blob.is_empty() {
                let saves_empty = actor
                    .save_state()
                    .map(|blob| blob.is_empty())
                    .map_err(|e| {
                        HaresError::Io(format!(
                            "checkpoint cannot verify empty state blob for actor '{}': {e}",
                            actor.name()
                        ))
                    })?;
                if !saves_empty {
                    return Err(HaresError::Io(format!(
                        "checkpoint actor state blob for '{}' is empty but the actor has persistent state: truncated or corrupted checkpoint",
                        actor.name()
                    )));
                }
            }
            blobs.push(state.blob.as_slice());
        }
        if blobs.len() != self.actors.len() {
            return Err(HaresError::Io(format!(
                "checkpoint actor count mismatch: checkpoint has {} actor states, dwelling has {} actors",
                blobs.len(),
                self.actors.len()
            )));
        }
        Ok(blobs)
    }

    /// Every restored built-in driver's stream must lie behind the restored
    /// cursor, or the next driver built would share it. Reads the drivers'
    /// streams as their loaded state holds them.
    fn check_ev_driver_cursor(&self, cursor: u64) -> Result<()> {
        let driver_streams =
            RNG_STREAM_EV_DRIVER_BASE..RNG_STREAM_EV_DRIVER_BASE + EV_DRIVER_STREAM_COUNT;
        let Some(driver) = self.actors.iter().find(|actor| {
            self.auto_registered_actor_names.contains(actor.name())
                && actor.rng_pair().is_some_and(|(_, stream)| {
                    driver_streams.contains(&stream) && stream - RNG_STREAM_EV_DRIVER_BASE >= cursor
                })
        }) else {
            return Ok(());
        };
        Err(HaresError::Io(format!(
            "checkpoint EV driver stream cursor {cursor} is not past the restored stream of \
             '{}': the next driver built would share that stream",
            driver.name()
        )))
    }

    /// Loads the equipment and actor blobs, then runs `check_loaded` on the
    /// loaded dwelling; if any load or the check fails, reloads every
    /// equipment and actor with the state it held before and returns the
    /// failure.
    fn load_component_states(
        &mut self,
        equipment_blobs: &[&[u8]],
        actor_blobs: &[&[u8]],
        check_loaded: impl FnOnce(&Self) -> Result<()>,
    ) -> Result<()> {
        let saved_equipment = self
            .equipment
            .iter()
            .map(|eq| eq.save_state())
            .collect::<Result<Vec<_>>>()?;
        let saved_actors = self
            .actors
            .iter()
            .map(|actor| actor.save_state())
            .collect::<Result<Vec<_>>>()?;
        let Err(err) = self
            .load_component_blobs(equipment_blobs, actor_blobs)
            .and_then(|()| check_loaded(self))
        else {
            return Ok(());
        };
        let saved_equipment: Vec<&[u8]> = saved_equipment.iter().map(Vec::as_slice).collect();
        let saved_actors: Vec<&[u8]> = saved_actors.iter().map(Vec::as_slice).collect();
        self.load_component_blobs(&saved_equipment, &saved_actors)
            .map_err(|rollback_err| {
                HaresError::InvalidState(format!(
                    "checkpoint load failed ({err}), and reloading the previous equipment \
                     and actor state also failed ({rollback_err})"
                ))
            })?;
        Err(err)
    }

    fn load_component_blobs(&mut self, equipment: &[&[u8]], actors: &[&[u8]]) -> Result<()> {
        for (eq, blob) in self.equipment.iter_mut().zip(equipment) {
            eq.load_state(blob)?;
        }
        for (actor, blob) in self.actors.iter_mut().zip(actors) {
            actor.load_state(blob)?;
        }
        Ok(())
    }
}
