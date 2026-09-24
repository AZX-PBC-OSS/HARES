//! Sample-weight validity classification for fleet population weighting.
//!
//! A dwelling's `sample_weight` scales its contribution to fleet-level weighted
//! aggregation (`weighted_value += value * sample_weight`). Only finite,
//! non-negative weights are meaningful: a NaN, infinite, or negative weight
//! silently corrupts every weighted sum and total it touches, making the fleet
//! result indistinguishable from a simulation failure. A zero weight is valid
//! but contributes nothing, so it is flagged for a warning while still being
//! accepted (the dwelling may still be useful for standalone simulation).
//!
//! This module is the single source of truth for that rule. Every ingestion
//! boundary that can introduce a `sample_weight` — ResStock parquet parsing,
//! caller-supplied fleet weight overrides, and fleet aggregation — classifies
//! weights through [`classify_sample_weight`] so their validity semantics
//! cannot drift apart.

/// Validity class of a `sample_weight` for fleet population weighting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleWeightClass {
    /// Finite and strictly positive: contributes to weighted aggregation.
    Positive,
    /// Exactly zero: valid but contributes nothing. Warn, do not reject.
    Zero,
    /// NaN, infinite, or negative: corrupts aggregation and must be rejected.
    Invalid,
}

/// Classify a `sample_weight` value for fleet population weighting.
///
/// See [`SampleWeightClass`] for the meaning of each variant.
#[must_use]
pub fn classify_sample_weight(weight: f64) -> SampleWeightClass {
    if !weight.is_finite() || weight < 0.0 {
        SampleWeightClass::Invalid
    } else if weight == 0.0 {
        SampleWeightClass::Zero
    } else {
        SampleWeightClass::Positive
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positive_weights_are_positive() {
        assert_eq!(classify_sample_weight(1.0), SampleWeightClass::Positive);
        assert_eq!(classify_sample_weight(42.5), SampleWeightClass::Positive);
        assert_eq!(
            classify_sample_weight(f64::MIN_POSITIVE),
            SampleWeightClass::Positive
        );
    }

    #[test]
    fn zero_is_its_own_class() {
        assert_eq!(classify_sample_weight(0.0), SampleWeightClass::Zero);
        // Negative zero is numerically zero and contributes nothing.
        assert_eq!(classify_sample_weight(-0.0), SampleWeightClass::Zero);
    }

    #[test]
    fn nan_infinite_and_negative_are_invalid() {
        assert_eq!(classify_sample_weight(f64::NAN), SampleWeightClass::Invalid);
        assert_eq!(
            classify_sample_weight(f64::INFINITY),
            SampleWeightClass::Invalid
        );
        assert_eq!(
            classify_sample_weight(f64::NEG_INFINITY),
            SampleWeightClass::Invalid
        );
        assert_eq!(classify_sample_weight(-1.0), SampleWeightClass::Invalid);
    }
}
