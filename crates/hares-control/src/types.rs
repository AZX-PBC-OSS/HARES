//! Common types for the control subsystem.

use serde::{Deserialize, Serialize};

/// Price-like external signals consumed by higher-level controllers.
///
/// This is intentionally separate from `ControlSignal`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct PriceSignal {
    pub electricity_price: Option<f64>,
    pub export_price: Option<f64>,
    /// Grid carbon intensity in `kg CO₂e/kWh`.
    pub ghg_intensity: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::PriceSignal;

    #[test]
    fn price_signal_round_trips_through_json() {
        let signal = PriceSignal {
            electricity_price: Some(0.19),
            export_price: Some(0.06),
            ghg_intensity: Some(0.45),
        };

        let json = serde_json::to_string(&signal).expect("serialize price signal");
        let decoded: PriceSignal = serde_json::from_str(&json).expect("deserialize price signal");
        assert_eq!(decoded, signal);
    }
}
