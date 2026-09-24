//! Physical property calculations for residential building energy simulation.

pub mod air_properties;
pub mod ashrae152;
pub mod biquadratic;
pub mod borehole;
pub mod constants;
pub mod film_coefficients;
pub mod ground;
pub mod infiltration;
pub mod psychrometrics;
pub mod pump;
pub mod pv_sizing;
pub mod solar;
pub mod units;
pub mod water_mains;

#[cfg(test)]
pub mod test_utils;

pub use constants::*;

/// Verify that a specific heat value is physically plausible for building
/// materials (100–3000 J/(kg·K)).  Values outside this range indicate either
/// unit confusion (kJ vs J) or data corruption.
///
/// Range derived from ASHRAE HoF 2021 Ch.26 material properties table:
/// common structural materials span ~450 J/(kg·K) (steel) to
/// ~2400 J/(kg·K) (wood).  100–3000 J/(kg·K) provides a generous margin
/// that catches kJ/kg-K values mistakenly entered as J/kg-K (1000× off).
#[cfg(any(debug_assertions, feature = "check_invariants"))]
pub fn check_specific_heat_plausible(cp: f64, material_name: &str) {
    assert!(
        (100.0..=3000.0).contains(&cp),
        "Implausible specific heat {cp} J/kg-K for material '{material_name}' — possible unit mismatch"
    );
}

#[cfg(test)]
#[cfg(any(debug_assertions, feature = "check_invariants"))]
mod tests {
    use super::*;

    #[test]
    fn specific_heat_in_range_passes() {
        // Gypsum board (ASHRAE HoF 2021 Ch.26 Table 4)
        check_specific_heat_plausible(837.0, "gypsum board");
        // Concrete (typical)
        check_specific_heat_plausible(880.0, "concrete");
        // Wood (typical)
        check_specific_heat_plausible(1200.0, "wood");
    }

    #[test]
    #[should_panic(expected = "Implausible specific heat")]
    fn specific_heat_below_minimum_panics() {
        check_specific_heat_plausible(50.0, "test material");
    }

    #[test]
    #[should_panic(expected = "Implausible specific heat")]
    fn specific_heat_above_maximum_panics() {
        check_specific_heat_plausible(5000.0, "test material");
    }

    #[test]
    #[should_panic(expected = "Implausible specific heat")]
    fn specific_heat_at_kj_scale_panics() {
        // 837400 J/kg-K would be 837.4 kJ/kg-K — the kind of unit mismatch
        // this check is designed to catch.
        check_specific_heat_plausible(837_400.0, "gypsum (kJ mistaken as J)");
    }

    #[test]
    fn specific_heat_at_boundaries_passes() {
        // Lower bound
        check_specific_heat_plausible(100.0, "lower bound material");
        // Upper bound
        check_specific_heat_plausible(3000.0, "upper bound material");
    }

    #[test]
    #[should_panic(expected = "Implausible specific heat")]
    fn specific_heat_zero_panics() {
        // 0 is below the plausible range; callers should skip the check
        // when zero is intentionally used for massless layers.
        check_specific_heat_plausible(0.0, "massless layer");
    }
}
