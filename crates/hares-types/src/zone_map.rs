//! Zone-to-role mapping for equipment thermal routing.
//!
//! Equipment models that auto-route thermal contributions based on semantic zone
//! type (e.g. "Garage equipment → garage zone") should query the [`ZoneMap`]
//! rather than hardcoding integer [`ZoneId`] values. The map is populated at
//! dwelling construction from the parsed HPXML zone configuration.

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ZoneId;

/// Semantic role of a building zone for equipment routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ZoneRole {
    /// Primary conditioned living space.
    Indoor,
    /// Attached or detached garage.
    Garage,
    /// Basement (below-grade conditioned or unconditioned space).
    Basement,
    /// Vented or unvented attic.
    Attic,
    /// Crawlspace (vented or unvented).
    Crawlspace,
    /// Outdoor environment (not a thermal zone — boundary condition).
    Outdoor,
}

impl fmt::Display for ZoneRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Indoor => write!(f, "Indoor"),
            Self::Garage => write!(f, "Garage"),
            Self::Basement => write!(f, "Basement"),
            Self::Attic => write!(f, "Attic"),
            Self::Crawlspace => write!(f, "Crawlspace"),
            Self::Outdoor => write!(f, "Outdoor"),
        }
    }
}

/// Maps semantic [`ZoneRole`] values to concrete [`ZoneId`] values for a
/// specific dwelling configuration.
///
/// Populated at dwelling construction from the sorted, filtered building zone
/// list. Equipment that needs to route thermal contributions to a specific
/// zone type queries this map rather than assuming a fixed integer `ZoneId`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ZoneMap {
    roles: HashMap<ZoneRole, ZoneId>,
}

impl ZoneMap {
    /// Creates an empty `ZoneMap`.
    pub fn new() -> Self {
        Self {
            roles: HashMap::new(),
        }
    }

    /// Registers a mapping from a [`ZoneRole`] to a specific [`ZoneId`].
    ///
    /// If the role already has a mapping, the new value replaces it.
    pub fn insert(&mut self, role: ZoneRole, id: ZoneId) {
        self.roles.insert(role, id);
    }

    /// Looks up the [`ZoneId`] for a given [`ZoneRole`].
    ///
    /// Returns `None` when the dwelling has no zone of that type.
    pub fn get(&self, role: ZoneRole) -> Option<ZoneId> {
        self.roles.get(&role).copied()
    }

    /// Returns the number of roles registered in this map.
    pub fn len(&self) -> usize {
        self.roles.len()
    }

    /// Returns `true` when no roles have been registered.
    pub fn is_empty(&self) -> bool {
        self.roles.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_map_returns_none() {
        let map = ZoneMap::new();
        assert_eq!(map.get(ZoneRole::Indoor), None);
        assert_eq!(map.get(ZoneRole::Garage), None);
    }

    #[test]
    fn insert_and_retrieve() {
        let mut map = ZoneMap::new();
        map.insert(ZoneRole::Indoor, ZoneId(1));
        map.insert(ZoneRole::Garage, ZoneId(3));
        assert_eq!(map.get(ZoneRole::Indoor), Some(ZoneId(1)));
        assert_eq!(map.get(ZoneRole::Garage), Some(ZoneId(3)));
        assert_eq!(map.get(ZoneRole::Basement), None);
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn insert_overwrites_prior() {
        let mut map = ZoneMap::new();
        map.insert(ZoneRole::Garage, ZoneId(2));
        map.insert(ZoneRole::Garage, ZoneId(5));
        assert_eq!(map.get(ZoneRole::Garage), Some(ZoneId(5)));
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn zone_role_display() {
        assert_eq!(ZoneRole::Indoor.to_string(), "Indoor");
        assert_eq!(ZoneRole::Garage.to_string(), "Garage");
        assert_eq!(ZoneRole::Basement.to_string(), "Basement");
        assert_eq!(ZoneRole::Attic.to_string(), "Attic");
        assert_eq!(ZoneRole::Crawlspace.to_string(), "Crawlspace");
        assert_eq!(ZoneRole::Outdoor.to_string(), "Outdoor");
    }

    #[test]
    fn is_empty_and_len() {
        let map = ZoneMap::new();
        assert!(map.is_empty());
        assert_eq!(map.len(), 0);

        let mut map = ZoneMap::new();
        map.insert(ZoneRole::Indoor, ZoneId(1));
        assert!(!map.is_empty());
        assert_eq!(map.len(), 1);
    }
}
