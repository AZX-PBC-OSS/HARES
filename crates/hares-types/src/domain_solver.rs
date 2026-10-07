//! Shared domain solver trait and update payload.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{DomainId, EnvironmentState, HaresError, PortSlots, ZoneId};

pub const THERMAL: DomainId = DomainId(0);
pub const ELECTRICAL: DomainId = DomainId(1);
pub const HUMIDITY: DomainId = DomainId(2);
pub const FLUID: DomainId = DomainId(3);

/// Canonical domain id for the mains-water payload slot. Lives here with
/// the other fixed ids because the domain slots' install guard in this
/// crate must reject it.
pub const MAINS_WATER_DOMAIN_ID: DomainId = DomainId(u16::MAX - 1);

/// The fixed slot ids no custom solver may claim: the four solver
/// domains plus the two payload slots the environment manager writes
/// every step.
const FIXED_DOMAIN_IDS: [DomainId; 6] = [
    THERMAL,
    ELECTRICAL,
    HUMIDITY,
    FLUID,
    crate::SCHEDULE_DOMAIN_ID,
    MAINS_WATER_DOMAIN_ID,
];

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct DomainUpdate {
    pub domain_id: DomainId,
    pub zone_temperatures_c: Vec<(ZoneId, f64)>,
    pub custom_payload: Option<Vec<f64>>,
}

impl Clone for DomainUpdate {
    fn clone(&self) -> Self {
        Self {
            domain_id: self.domain_id,
            zone_temperatures_c: self.zone_temperatures_c.clone(),
            custom_payload: self.custom_payload.clone(),
        }
    }

    /// Copies `source` over `self` in place: both vectors are reused
    /// (`Vec::clone_from` keeps their capacity when it suffices), so a
    /// steady-state write allocates nothing.
    fn clone_from(&mut self, source: &Self) {
        self.domain_id = source.domain_id;
        self.zone_temperatures_c
            .clone_from(&source.zone_temperatures_c);
        self.custom_payload.clone_from(&source.custom_payload);
    }
}

/// The handle [`DomainSlots::install_custom`] returns: the position of a
/// custom domain's slot, resolved once at installation. Readers pass it
/// to [`DomainSlots::custom`]. The field is private, so a reader cannot
/// name a slot that was never installed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CustomDomainSlot(usize);

/// One domain's per-step update slot: the retained [`DomainUpdate`] this
/// step's writer filled and whether it was written at all. `present` is
/// what readers see: [`DomainSlot::get`] returns `None` between the
/// step's clear and the slot's write, exactly where a drained
/// `Vec<DomainUpdate>` used to hold no entry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DomainSlot {
    update: DomainUpdate,
    present: bool,
}

impl DomainSlot {
    /// An unwritten slot carrying `id`.
    fn empty(id: DomainId) -> Self {
        Self {
            update: DomainUpdate::empty(id),
            present: false,
        }
    }
    /// The update written this step, or `None` when the slot has not
    /// been written since it was last cleared.
    #[must_use]
    pub fn get(&self) -> Option<&DomainUpdate> {
        self.present.then_some(&self.update)
    }

    /// Copies `source` over the slot's retained update in place and
    /// marks the slot written. The copy goes through the hand-written
    /// `DomainUpdate::clone_from`, which reuses both vectors, so a
    /// steady-state write allocates nothing.
    pub fn set_from(&mut self, source: &DomainUpdate) {
        self.update.clone_from(source);
        self.present = true;
    }

    /// The payload vector to write in place (the schedule and mains
    /// slots' contract): materializes it on the first write and marks
    /// the slot written.
    pub fn payload_mut(&mut self) -> &mut Vec<f64> {
        self.present = true;
        self.update.custom_payload.get_or_insert_with(Vec::new)
    }

    /// Marks the slot unwritten: `get` returns `None` until the slot is
    /// written again.
    pub fn clear(&mut self) {
        self.present = false;
    }
}

/// The fixed per-step domain-update slots an [`crate::EnvironmentState`]
/// carries: one per built-in domain plus one per installed custom
/// solver. Replaces the drained-and-repushed `Vec<DomainUpdate>`, whose
/// every write and read was a linear search by domain id and whose every
/// upsert went through a full clone of the update's vectors.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DomainSlots {
    pub thermal: DomainSlot,
    pub humidity: DomainSlot,
    pub electrical: DomainSlot,
    pub fluid: DomainSlot,
    pub schedule: DomainSlot,
    pub mains_water: DomainSlot,
    /// One per installed custom solver, in installation order.
    custom: Vec<DomainSlot>,
}

impl Default for DomainSlots {
    /// Every slot starts unwritten and carries its own fixed id.
    fn default() -> Self {
        Self {
            thermal: DomainSlot::empty(THERMAL),
            humidity: DomainSlot::empty(HUMIDITY),
            electrical: DomainSlot::empty(ELECTRICAL),
            fluid: DomainSlot::empty(FLUID),
            schedule: DomainSlot::empty(crate::SCHEDULE_DOMAIN_ID),
            mains_water: DomainSlot::empty(MAINS_WATER_DOMAIN_ID),
            custom: Vec::new(),
        }
    }
}

impl DomainSlots {
    /// Reserves a custom domain's slot and returns its handle. Errors
    /// naming `id` when it is one of the fixed ids or a custom domain
    /// already installed: the step path's slot set is fixed once
    /// stepping starts.
    pub fn install_custom(&mut self, id: DomainId) -> Result<CustomDomainSlot, HaresError> {
        if FIXED_DOMAIN_IDS.contains(&id) {
            return Err(HaresError::InvalidState(format!(
                "domain {id:?} is a fixed slot and cannot be installed as a custom domain"
            )));
        }
        if self.custom.iter().any(|slot| slot.update.domain_id == id) {
            return Err(HaresError::InvalidState(format!(
                "custom domain {id:?} is already installed"
            )));
        }
        self.custom.push(DomainSlot {
            update: DomainUpdate::empty(id),
            present: false,
        });
        Ok(CustomDomainSlot(self.custom.len() - 1))
    }

    /// The custom domain's update written this step, or `None` when the
    /// slot has not been written since it was last cleared.
    #[must_use]
    pub fn custom(&self, slot: CustomDomainSlot) -> Option<&DomainUpdate> {
        self.custom.get(slot.0).and_then(DomainSlot::get)
    }

    /// Copies `source` over the custom slot's retained update in place
    /// and marks the slot written. Panics when `slot` did not come from
    /// [`Self::install_custom`] on this state: a handle is only
    /// constructed there.
    pub fn set_custom(&mut self, slot: CustomDomainSlot, source: &DomainUpdate) {
        self.custom
            .get_mut(slot.0)
            .expect("custom domain slot handle out of range")
            .set_from(source);
    }

    /// Marks every slot unwritten. The step's environment update calls
    /// this before its writers run, so a reader between the clear and a
    /// slot's write sees `None` where the drained `Vec` held no entry.
    pub fn clear_step(&mut self) {
        self.thermal.clear();
        self.humidity.clear();
        self.electrical.clear();
        self.fluid.clear();
        self.schedule.clear();
        self.mains_water.clear();
        for slot in &mut self.custom {
            slot.clear();
        }
    }
}

impl DomainUpdate {
    /// Create an empty update for the given domain, suitable for reuse via `resolve`.
    pub fn empty(domain_id: DomainId) -> Self {
        Self {
            domain_id,
            zone_temperatures_c: Vec::new(),
            custom_payload: None,
        }
    }

    /// Clear data vecs without deallocating, keeping domain_id unchanged.
    pub fn clear(&mut self) {
        self.zone_temperatures_c.clear();
        if let Some(ref mut p) = self.custom_payload {
            p.clear();
        }
    }
}

pub trait DomainSolver: Send + Sync {
    fn domain_id(&self) -> DomainId;
    /// Advance the solver one step. Physics, conservation, non-finite and
    /// wiring checks on the run path are unconditional and surface as a
    /// typed error instead of panicking; a failing step must leave the
    /// simulation state untouched for the caller to report.
    fn resolve(
        &mut self,
        ports: &PortSlots,
        env: &EnvironmentState,
        dt: Duration,
        out: &mut DomainUpdate,
    ) -> Result<(), HaresError>;

    /// Optional observation state for the observer framework.
    ///
    /// Returns a snapshot of the solver's internal state vector or key output
    /// scalars. The default returns an empty vector; override to provide
    /// domain-specific observability.
    fn observation_state(&self) -> Vec<f64> {
        Vec::new()
    }

    /// Convenience wrapper that allocates and returns a new `DomainUpdate`.
    /// Prefer passing a reusable buffer via `resolve` in hot loops.
    fn resolve_new(
        &mut self,
        ports: &PortSlots,
        env: &EnvironmentState,
        dt: Duration,
    ) -> Result<DomainUpdate, HaresError> {
        let mut out = DomainUpdate::empty(self.domain_id());
        self.resolve(ports, env, dt, &mut out)?;
        Ok(out)
    }
}
