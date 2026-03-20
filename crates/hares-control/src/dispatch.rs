//! Control signal dispatch and routing.

use hares_types::{ControlSignal, EndUse};
use serde::{Deserialize, Serialize};

/// Control dispatch target. Routing logic is implemented in `hares-core`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DispatchTarget {
    ByName(String),
    ByEndUse(EndUse),
}

/// One control dispatch request containing target and signal payload.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DispatchRequest {
    pub target: DispatchTarget,
    pub signal: ControlSignal,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ControlSignalConstructors;
    use hares_types::EndUse;

    #[test]
    fn dispatch_target_construction_for_both_variants() {
        let by_name = DispatchTarget::ByName("Battery #1".to_string());
        let by_end_use = DispatchTarget::ByEndUse(EndUse::HvacHeating);

        assert_eq!(by_name, DispatchTarget::ByName("Battery #1".to_string()));
        assert_eq!(by_end_use, DispatchTarget::ByEndUse(EndUse::HvacHeating));
    }

    #[test]
    fn dispatch_request_by_name_round_trips_through_json() {
        let request = DispatchRequest {
            target: DispatchTarget::ByName("PV South".to_string()),
            signal: hares_types::ControlSignal::power_limit(2.5, Some(0.1)),
        };

        let json = serde_json::to_string(&request).expect("serialize dispatch request");
        let decoded: DispatchRequest =
            serde_json::from_str(&json).expect("deserialize dispatch request");
        assert_eq!(decoded, request);
    }

    #[test]
    fn dispatch_request_by_end_use_round_trips_through_json() {
        let request = DispatchRequest {
            target: DispatchTarget::ByEndUse(EndUse::Battery),
            signal: hares_types::ControlSignal::power_setpoint(3.0, None),
        };

        let json = serde_json::to_string(&request).expect("serialize dispatch request");
        let decoded: DispatchRequest =
            serde_json::from_str(&json).expect("deserialize dispatch request");
        assert_eq!(decoded, request);
    }
}
