//! Physical property calculations for residential building energy simulation.

use hares_types::HaresError;

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
///
/// Unconditional in every build profile: the value is user input, so a
/// violation is a typed error at material parse and build.
pub fn check_specific_heat_plausible(cp: f64, material_name: &str) -> Result<(), HaresError> {
    if !(100.0..=3000.0).contains(&cp) {
        return Err(HaresError::Physics(format!(
            "Implausible specific heat {cp} J/kg-K for material '{material_name}': possible unit mismatch"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specific_heat_in_range_passes() {
        // Gypsum board (ASHRAE HoF 2021 Ch.26 Table 4)
        check_specific_heat_plausible(837.0, "gypsum board").unwrap();
        // Concrete (typical)
        check_specific_heat_plausible(880.0, "concrete").unwrap();
        // Wood (typical)
        check_specific_heat_plausible(1200.0, "wood").unwrap();
    }

    #[test]
    fn specific_heat_below_minimum_is_an_error() {
        let result = check_specific_heat_plausible(50.0, "test material");
        assert!(result.is_err(), "50 J/kg-K is below the plausible range");
    }

    #[test]
    fn specific_heat_above_maximum_is_an_error() {
        let result = check_specific_heat_plausible(5000.0, "test material");
        assert!(result.is_err(), "5000 J/kg-K is above the plausible range");
    }

    #[test]
    fn specific_heat_at_kj_scale_is_an_error() {
        // 837400 J/kg-K would be 837.4 kJ/kg-K — the kind of unit mismatch
        // this check is designed to catch.
        let result = check_specific_heat_plausible(837_400.0, "gypsum (kJ mistaken as J)");
        assert!(result.is_err(), "kJ-scale values must be a typed error");
    }

    #[test]
    fn specific_heat_at_boundaries_passes() {
        // Lower bound
        check_specific_heat_plausible(100.0, "lower bound material").unwrap();
        // Upper bound
        check_specific_heat_plausible(3000.0, "upper bound material").unwrap();
    }

    #[test]
    fn specific_heat_zero_is_an_error() {
        // 0 is below the plausible range; callers should skip the check
        // when zero is intentionally used for massless layers.
        let result = check_specific_heat_plausible(0.0, "massless layer");
        assert!(result.is_err(), "zero specific heat must be a typed error");
    }
}
