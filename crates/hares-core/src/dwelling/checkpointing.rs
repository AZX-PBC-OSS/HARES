//! Checkpoint save, and validated restore that leaves the dwelling unchanged
//! when any part of the checkpoint is rejected.

use std::collections::HashMap;

use hares_envelope::FluidSolver;
use hares_types::{HaresError, ZoneId};
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

use super::Dwelling;
use crate::checkpoint::{
    ActorStateCheckpoint, CHECKPOINT_VERSION, DwellingCheckpoint, EquipmentStateCheckpoint,
};

type Result<T> = std::result::Result<T, HaresError>;

impl Dwelling {
    /// Snapshot current simulation state to an in-memory checkpoint struct.
    pub fn save_checkpoint(&self) -> Result<DwellingCheckpoint> {
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

        let mut equipment_states = Vec::with_capacity(self.equipment.len());
        for eq in &self.equipment {
            match eq.save_state() {
                Ok(blob) => {
                    let desc = eq.descriptor();
                    equipment_states.push(EquipmentStateCheckpoint {
                        name: desc.name.clone(),
                        equipment_id: desc.id.0,
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
        })
    }

    /// Restore simulation state from a checkpoint.
    ///
    /// Every part of the checkpoint is validated against this dwelling
    /// before anything changes. Equipment and actor blobs can only be
    /// checked by decoding them, so they load first; if one fails, every
    /// equipment and actor is reloaded with the state it held before the
    /// call. On any error the dwelling is therefore left as it was.
    pub fn load_checkpoint(&mut self, cp: DwellingCheckpoint) -> Result<()> {
        self.validate_building_state(&cp)?;
        let equipment_blobs = self.matched_equipment_blobs(&cp)?;
        let actor_blobs = self.matched_actor_blobs(&cp)?;
        self.load_component_states(&equipment_blobs, &actor_blobs)?;

        self.apply_building_state(&cp)?;
        self.clock.current_step = cp.timestep_index;
        let mut restored_rng = ChaCha8Rng::from_seed(cp.rng_state);
        restored_rng.set_stream(cp.rng_stream);
        restored_rng.set_word_pos(cp.rng_word_pos);
        self.rng = restored_rng;
        self.prior_electrical_summary = cp.prior_electrical_summary;

        // Populate latest_env.equipment_core and equipment_telemetry from the
        // restored equipment state so that actors read correct SOC, power flows,
        // and connection state on the first post-restore step.
        self.snapshot_equipment_state();
        self.restored_from_checkpoint = true;
        Ok(())
    }

    /// Restore building shell state from a prior-segment checkpoint.
    ///
    /// Transfers the thermal envelope, humidity and fluid solver state and
    /// the prior electrical summary, and resets the clock to the start of
    /// the segment. The RNG, equipment and actor states are not restored:
    /// the current equipment set was freshly built by
    /// `DwellingBlueprint::build()` and already initialized via
    /// `Equipment::init()`. The checkpoint is validated before anything
    /// changes, so on error the dwelling is left as it was.
    pub fn restore_building_state(&mut self, cp: &DwellingCheckpoint) -> Result<()> {
        self.validate_building_state(cp)?;
        self.apply_building_state(cp)?;
        self.prior_electrical_summary = cp.prior_electrical_summary.clone();
        self.clock.current_step = 0;
        self.snapshot_equipment_state();
        Ok(())
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
    fn matched_equipment_blobs<'cp>(&self, cp: &'cp DwellingCheckpoint) -> Result<Vec<&'cp [u8]>> {
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
                Ok(state.blob.as_slice())
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

    /// Loads the equipment and actor blobs; if any load fails, reloads every
    /// equipment and actor with the state it held before and returns the
    /// failure.
    fn load_component_states(
        &mut self,
        equipment_blobs: &[&[u8]],
        actor_blobs: &[&[u8]],
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
        let Err(err) = self.load_component_blobs(equipment_blobs, actor_blobs) else {
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
