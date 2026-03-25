//! Control signal dispatch and routing.

use std::sync::Arc;

use hares_types::{ControlSignal, EndUse};
use serde::{Deserialize, Serialize};

/// Number of priority tiers. Must match variant count of [`PriorityTier`].
pub const PRIORITY_TIER_COUNT: usize = 4;

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[repr(u8)]
pub enum PriorityTier {
    #[default]
    Schedule = 0,
    UserOverride = 1,
    Grid = 2,
    Safety = 3,
}

impl PriorityTier {
    /// Returns the index into a tier-indexed array.
    #[inline]
    pub const fn index(self) -> usize {
        self as usize
    }
}

/// Control dispatch target. Routing logic is implemented in `hares-core`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DispatchTarget {
    ByName(#[serde(serialize_with = "ser_arc_str", deserialize_with = "de_arc_str")] Arc<str>),
    ByEndUse(EndUse),
}

fn ser_arc_str<S: serde::Serializer>(val: &Arc<str>, ser: S) -> Result<S::Ok, S::Error> {
    ser.serialize_str(val)
}

fn de_arc_str<'de, D: serde::Deserializer<'de>>(de: D) -> Result<Arc<str>, D::Error> {
    let s: String = serde::Deserialize::deserialize(de)?;
    Ok(Arc::from(s))
}

impl DispatchTarget {
    /// Returns true if two targets route to the same equipment.
    ///
    /// Used by the dispatcher to detect and log control signal conflicts
    /// within a single timestep. Zero-allocation — compares inner references.
    pub fn conflicts_with(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::ByName(a), Self::ByName(b)) => a == b,
            (Self::ByEndUse(a), Self::ByEndUse(b)) => a == b,
            _ => false,
        }
    }
}

/// One control dispatch request containing target and signal payload.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DispatchRequest {
    pub target: DispatchTarget,
    pub signal: ControlSignal,
    pub priority: PriorityTier,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::ControlSignalConstructors;
    use hares_types::EndUse;

    #[test]
    fn dispatch_target_construction_for_both_variants() {
        let by_name = DispatchTarget::ByName(Arc::from("Battery #1"));
        let by_end_use = DispatchTarget::ByEndUse(EndUse::HVAC_HEATING);

        assert_eq!(by_name, DispatchTarget::ByName(Arc::from("Battery #1")));
        assert_eq!(by_end_use, DispatchTarget::ByEndUse(EndUse::HVAC_HEATING));
    }

    #[test]
    fn dispatch_request_by_name_round_trips_through_json() {
        let request = DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("PV South")),
            signal: hares_types::ControlSignal::power_limit(2.5, Some(0.1)),
            priority: PriorityTier::Schedule,
        };

        let json = serde_json::to_string(&request).expect("serialize dispatch request");
        let decoded: DispatchRequest =
            serde_json::from_str(&json).expect("deserialize dispatch request");
        assert_eq!(decoded, request);
    }

    #[test]
    fn dispatch_request_by_end_use_round_trips_through_json() {
        let request = DispatchRequest {
            target: DispatchTarget::ByEndUse(EndUse::BATTERY),
            signal: hares_types::ControlSignal::power_setpoint(3.0, None),
            priority: PriorityTier::Grid,
        };

        let json = serde_json::to_string(&request).expect("serialize dispatch request");
        let decoded: DispatchRequest =
            serde_json::from_str(&json).expect("deserialize dispatch request");
        assert_eq!(decoded, request);
    }

    #[test]
    fn priority_tier_ordering() {
        assert!(PriorityTier::Safety > PriorityTier::Grid);
        assert!(PriorityTier::Grid > PriorityTier::UserOverride);
        assert!(PriorityTier::UserOverride > PriorityTier::Schedule);
    }

    #[test]
    fn priority_tier_count_matches_variants() {
        assert_eq!(
            PRIORITY_TIER_COUNT, 4,
            "PRIORITY_TIER_COUNT must match PriorityTier variant count"
        );
    }

    #[test]
    fn priority_tier_index_returns_correct_value() {
        assert_eq!(PriorityTier::Schedule.index(), 0);
        assert_eq!(PriorityTier::UserOverride.index(), 1);
        assert_eq!(PriorityTier::Grid.index(), 2);
        assert_eq!(PriorityTier::Safety.index(), 3);
    }

    #[test]
    fn conflicts_with_same_name() {
        let a = DispatchTarget::ByName(Arc::from("HVAC"));
        let b = DispatchTarget::ByName(Arc::from("HVAC"));
        assert!(a.conflicts_with(&b));
    }

    #[test]
    fn conflicts_with_different_name() {
        let a = DispatchTarget::ByName(Arc::from("Battery"));
        let b = DispatchTarget::ByName(Arc::from("PV"));
        assert!(!a.conflicts_with(&b));
    }

    #[test]
    fn conflicts_with_same_end_use() {
        let a = DispatchTarget::ByEndUse(EndUse::HVAC_HEATING);
        let b = DispatchTarget::ByEndUse(EndUse::HVAC_HEATING);
        assert!(a.conflicts_with(&b));
    }

    #[test]
    fn conflicts_with_different_end_use() {
        let a = DispatchTarget::ByEndUse(EndUse::HVAC_HEATING);
        let b = DispatchTarget::ByEndUse(EndUse::BATTERY);
        assert!(!a.conflicts_with(&b));
    }

    #[test]
    fn conflicts_with_different_variants_never_conflict() {
        let by_name = DispatchTarget::ByName(Arc::from("hvac_heating"));
        let by_end_use = DispatchTarget::ByEndUse(EndUse::HVAC_HEATING);
        assert!(!by_name.conflicts_with(&by_end_use));
    }

    #[test]
    fn conflicts_with_custom_end_use() {
        let hpwh = DispatchTarget::ByEndUse(EndUse::custom("heat_pump_water_heater"));
        let hpwh2 = DispatchTarget::ByEndUse(EndUse::custom("heat_pump_water_heater"));
        let ice = DispatchTarget::ByEndUse(EndUse::custom("ice_storage"));

        assert!(hpwh.conflicts_with(&hpwh2));
        assert!(!hpwh.conflicts_with(&ice));
    }

    #[test]
    fn custom_end_use_dispatch_request_round_trips_through_json() {
        let request = DispatchRequest {
            target: DispatchTarget::ByEndUse(EndUse::custom("vehicle_to_grid")),
            signal: hares_types::ControlSignal::power_setpoint(3.0, None),
            priority: PriorityTier::Grid,
        };

        let json = serde_json::to_string(&request)
            .expect("serialize dispatch request with custom end use");
        let decoded: DispatchRequest = serde_json::from_str(&json).expect("deserialize");

        match decoded.target {
            DispatchTarget::ByEndUse(end_use) => {
                assert_eq!(end_use.as_str(), "vehicle_to_grid");
                assert!(!end_use.is_standard());
            }
            _ => panic!("expected ByEndUse target"),
        }
        assert_eq!(decoded.priority, PriorityTier::Grid);
    }
}
