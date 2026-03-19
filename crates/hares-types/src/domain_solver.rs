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

pub trait DomainSolver: Send + Sync {
    fn domain_id(&self) -> DomainId;
    fn resolve(&mut self, ports: &PortSlots, env: &EnvironmentState, dt: Duration) -> DomainUpdate;
}
