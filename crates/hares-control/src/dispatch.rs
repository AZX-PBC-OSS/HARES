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

    /// Constructs a `PriorityTier` from its index (0 = Schedule, 3 = Safety).
    ///
    /// # Panics
    ///
    /// Panics if `idx >= PRIORITY_TIER_COUNT`.
    #[inline]
    #[must_use]
    pub const fn from_index(idx: usize) -> Self {
        match idx {
            0 => PriorityTier::Schedule,
            1 => PriorityTier::UserOverride,
            2 => PriorityTier::Grid,
            3 => PriorityTier::Safety,
            _ => panic!("invalid PriorityTier index"),
        }
    }
}

impl From<&ControlSignal> for PriorityTier {
    /// Centralised signal-to-tier mapping.
    ///
    /// Every `ControlSignal` variant is assigned to a default priority tier
    /// based on its domain semantics. This match is **exhaustive** — adding a
    /// new variant without an arm produces a compile error, enforcing explicit
    /// tier assignment at the point of definition.
    ///
    /// Actors that need a different tier for a specific signal (e.g. DR
    /// compliance raising everything to `Grid`) should override explicitly
    /// with a comment justifying the override.
    ///
    /// | Tier          | Signals                                                                 |
    /// |---------------|-------------------------------------------------------------------------|
    /// | Safety (3)    | (reserved — no signal maps here)                                        |
    /// | Grid (2)      | DemandResponse, CurtailmentPercent, ReactiveSetpoint, PowerFactorSetpoint, InverterPriorityMode, PowerLimit |
    /// | UserOverride (1) | ThermalSetpoint, HumiditySetpoint, ModeOverride, ThermalSetpointDelta |
    /// | Schedule (0)  | PowerSetpoint, SOCTarget, DutyCycle, LoadFraction, GridConnect, SelfConsumption, ProtocolNative, IdealCapacity, IdealCapacityModeOverride, EvPlugIn, EvDrive, EvAwayCharge, EvSetReadyBy, EventDelay, MaxCapacityFraction |
    fn from(signal: &ControlSignal) -> Self {
        match signal {
            ControlSignal::ThermalSetpoint { .. }
            | ControlSignal::HumiditySetpoint { .. }
            | ControlSignal::ModeOverride { .. }
            | ControlSignal::ThermalSetpointDelta { .. } => PriorityTier::UserOverride,

            ControlSignal::DemandResponse { .. }
            | ControlSignal::CurtailmentPercent { .. }
            | ControlSignal::ReactiveSetpoint { .. }
            | ControlSignal::PowerFactorSetpoint { .. }
            | ControlSignal::InverterPriorityMode { .. }
            | ControlSignal::PowerLimit { .. } => PriorityTier::Grid,

            ControlSignal::PowerSetpoint { .. }
            | ControlSignal::SOCTarget { .. }
            | ControlSignal::DutyCycle { .. }
            | ControlSignal::LoadFraction { .. }
            | ControlSignal::GridConnect { .. }
            | ControlSignal::SelfConsumption { .. }
            | ControlSignal::ProtocolNative { .. }
            | ControlSignal::IdealCapacity { .. }
            | ControlSignal::IdealCapacityModeOverride { .. }
            | ControlSignal::EvPlugIn { .. }
            | ControlSignal::EvDrive { .. }
            | ControlSignal::EvAwayCharge { .. }
            | ControlSignal::EvSetReadyBy { .. }
            | ControlSignal::EventDelay { .. }
            | ControlSignal::MaxCapacityFraction { .. } => PriorityTier::Schedule,
        }
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
    /// within a single timestep. Zero-allocation -- compares inner references.
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
            priority: PriorityTier::Grid,
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
            priority: PriorityTier::Schedule,
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

#[cfg(test)]
mod tier_mapping_tests {
    use super::*;
    use crate::ControlSignalConstructors;
    use hares_types::{
        ControlSignal, DRLevel, EvConnectionState, IdealCapacityMode, InverterPriority,
        OperatingMode, ProtocolId,
    };

    #[test]
    fn user_override_signals_map_to_user_override_tier() {
        let signals: [ControlSignal; 4] = [
            ControlSignal::thermal_setpoint(Some(20.0), Some(24.0), Some(1.0)),
            ControlSignal::humidity_setpoint(0.45, Some(0.30), Some(0.60)),
            ControlSignal::mode_override(OperatingMode::Off),
            ControlSignal::ThermalSetpointDelta {
                heating_delta_c: Some(2.0),
                cooling_delta_c: Some(-2.0),
            },
        ];
        for s in &signals {
            assert_eq!(
                PriorityTier::from(s),
                PriorityTier::UserOverride,
                "signal {s:?} must map to UserOverride"
            );
        }
    }

    #[test]
    fn grid_signals_map_to_grid_tier() {
        let signals: [ControlSignal; 6] = [
            ControlSignal::demand_response(DRLevel::High, Some(3600.0)),
            ControlSignal::CurtailmentPercent { percent: 25.0 },
            ControlSignal::ReactiveSetpoint { kvar: 10.0 },
            ControlSignal::PowerFactorSetpoint { power_factor: 0.95 },
            ControlSignal::InverterPriorityMode {
                priority: InverterPriority::Watt,
            },
            ControlSignal::power_limit(5.0, Some(0.1)),
        ];
        for s in &signals {
            assert_eq!(
                PriorityTier::from(s),
                PriorityTier::Grid,
                "signal {s:?} must map to Grid"
            );
        }
    }

    #[test]
    fn schedule_signals_map_to_schedule_tier() {
        let signals: [ControlSignal; 15] = [
            ControlSignal::power_setpoint(3.0, None),
            ControlSignal::soc_target(0.7, Some(0.2), Some(0.9)),
            ControlSignal::duty_cycle(0.5, Some(900.0), None),
            ControlSignal::load_fraction(0.8),
            ControlSignal::grid_connect(true),
            ControlSignal::self_consumption(true, false),
            ControlSignal::protocol_native(ProtocolId(1), vec![]),
            ControlSignal::IdealCapacity { capacity_w: 3500.0 },
            ControlSignal::IdealCapacityModeOverride {
                mode: IdealCapacityMode::On,
            },
            ControlSignal::EvPlugIn {
                state: EvConnectionState::HomePluggedIn,
            },
            ControlSignal::EvDrive { kwh: 5.0 },
            ControlSignal::EvAwayCharge { power_kw: 11.5 },
            ControlSignal::EvSetReadyBy {
                departure_hour: 7.0,
                target_soc: 0.8,
            },
            ControlSignal::EventDelay { delay_s: 300.0 },
            ControlSignal::MaxCapacityFraction { fraction: 0.5 },
        ];
        for s in &signals {
            assert_eq!(
                PriorityTier::from(s),
                PriorityTier::Schedule,
                "signal {s:?} must map to Schedule"
            );
        }
    }

    #[test]
    fn all_25_variants_are_covered_by_mapping() {
        // If this test compiles, the exhaustive match covers all variants.
        // We construct every variant to prove the match arms compile.
        let all: [ControlSignal; 25] = [
            ControlSignal::thermal_setpoint(Some(20.0), Some(24.0), Some(1.0)),
            ControlSignal::humidity_setpoint(0.45, Some(0.30), Some(0.60)),
            ControlSignal::power_setpoint(3.0, None),
            ControlSignal::power_limit(5.0, Some(0.1)),
            ControlSignal::soc_target(0.7, Some(0.2), Some(0.9)),
            ControlSignal::mode_override(OperatingMode::Off),
            ControlSignal::duty_cycle(0.5, Some(900.0), None),
            ControlSignal::load_fraction(0.8),
            ControlSignal::grid_connect(true),
            ControlSignal::self_consumption(true, false),
            ControlSignal::demand_response(DRLevel::High, Some(3600.0)),
            ControlSignal::protocol_native(ProtocolId(1), vec![]),
            ControlSignal::CurtailmentPercent { percent: 25.0 },
            ControlSignal::ReactiveSetpoint { kvar: 10.0 },
            ControlSignal::PowerFactorSetpoint { power_factor: 0.95 },
            ControlSignal::InverterPriorityMode {
                priority: InverterPriority::Watt,
            },
            ControlSignal::IdealCapacity { capacity_w: 3500.0 },
            ControlSignal::ThermalSetpointDelta {
                heating_delta_c: Some(2.0),
                cooling_delta_c: Some(-2.0),
            },
            ControlSignal::IdealCapacityModeOverride {
                mode: IdealCapacityMode::On,
            },
            ControlSignal::EvPlugIn {
                state: EvConnectionState::HomePluggedIn,
            },
            ControlSignal::EvDrive { kwh: 5.0 },
            ControlSignal::EvAwayCharge { power_kw: 11.5 },
            ControlSignal::EvSetReadyBy {
                departure_hour: 7.0,
                target_soc: 0.8,
            },
            ControlSignal::EventDelay { delay_s: 300.0 },
            ControlSignal::MaxCapacityFraction { fraction: 0.5 },
        ];

        let mut tiers_seen = std::collections::HashSet::new();
        for s in &all {
            let tier = PriorityTier::from(s);
            tiers_seen.insert(tier);
        }
        assert!(
            tiers_seen.contains(&PriorityTier::Schedule),
            "at least one signal must map to Schedule"
        );
        assert!(
            tiers_seen.contains(&PriorityTier::UserOverride),
            "at least one signal must map to UserOverride"
        );
        assert!(
            tiers_seen.contains(&PriorityTier::Grid),
            "at least one signal must map to Grid"
        );

        // No signal maps to Safety — this tier is reserved for future use.
        assert!(
            !tiers_seen.contains(&PriorityTier::Safety),
            "no current signal should map to Safety"
        );

        assert_eq!(
            all.len(),
            25,
            "verify the array has exactly 25 elements — one per ControlSignal variant"
        );
    }

    #[test]
    fn safety_tier_is_highest_priority() {
        assert!(PriorityTier::Safety > PriorityTier::Grid);
        assert!(PriorityTier::Safety > PriorityTier::UserOverride);
        assert!(PriorityTier::Safety > PriorityTier::Schedule);
    }
}
