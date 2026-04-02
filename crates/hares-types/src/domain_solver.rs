//! Shared domain solver trait and update payload.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{DomainId, EnvironmentState, PortSlots, ZoneId};

pub const THERMAL: DomainId = DomainId(0);
pub const ELECTRICAL: DomainId = DomainId(1);
pub const HUMIDITY: DomainId = DomainId(2);
pub const FLUID: DomainId = DomainId(3);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DomainUpdate {
    pub domain_id: DomainId,
    pub zone_temperatures_c: Vec<(ZoneId, f64)>,
    pub custom_payload: Option<Vec<f64>>,
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
    fn resolve(&mut self, ports: &PortSlots, env: &EnvironmentState, dt: Duration, out: &mut DomainUpdate);

    /// Convenience wrapper that allocates and returns a new `DomainUpdate`.
    /// Prefer passing a reusable buffer via `resolve` in hot loops.
    fn resolve_new(&mut self, ports: &PortSlots, env: &EnvironmentState, dt: Duration) -> DomainUpdate {
        let mut out = DomainUpdate::empty(self.domain_id());
        self.resolve(ports, env, dt, &mut out);
        out
    }
}
