//! Default biquadratic performance curves for standalone dehumidifiers.
//!
//! When no user-supplied curves are provided (via HPXML, config, or test
//! extras), the dehumidifier init path substitutes these defaults instead of
//! the identity placeholder `[1, 0, 0, 0, 0, 0]`.
//!
//! Coefficients are sourced from EnergyPlus `ZoneHVAC:Dehumidifier:DX` test
//! files (`WindACRHControl.idf`, `SingleFamilyHouse_HP_Slab_Dehumidification.idf`)
//! and adapted for HARES's internal representation:
//!
//! EnergyPlus encodes relative humidity as *percent* (0–100) in its
//! biquadratic curve independent variable.  HARES encodes relative humidity
//! as a *fraction* (0–1).  The adapted coefficients below are scaled so that
//! `f(T, rh_fraction)` = `f_ep(T, rh_fraction * 100)` for all (T, rh) in
//! the domain: a, b, c are unchanged; d *= 100; e *= 10000; f *= 100.
//!
//! Rated conditions per EnergyPlus `ZoneDehumidifier.cc`:
//!   `RatedInletAirTemp = 26.7` (°C) and `RatedInletAirRH = 60.0` (%).
//! The rated-temperature constant uses 26.666… = 80°F exactly for numerical
//! symmetry in test assertions.

/// Default water-removal biquadratic curve coefficients, adapted from EnergyPlus
/// `ZoneHVAC:Dehumidifier:DX` `WaterRemovalCurve` in `WindACRHControl.idf`.
///
/// Original EnergyPlus coefficients (RH in %):
///   [-2.724878664080, 0.100711983591, -0.000990538285,
///    0.050053043874, -0.000203629282, -0.000341750531]
pub(super) const DEFAULT_WATER_REMOVAL_CURVE: [f64; 6] = [
    -2.724_878_664_080,
    0.100_711_983_591,
    -0.000_990_538_285,
    5.005_304_387_4,
    -2.036_292_82,
    -0.034_175_053_1,
];

/// Default energy-factor biquadratic curve coefficients, adapted from EnergyPlus
/// `ZoneHVAC:Dehumidifier:DX` `EnergyFactorCurve` in `WindACRHControl.idf`.
///
/// Original EnergyPlus coefficients (RH in %):
///   [-2.388319068955, 0.093047739452, -0.001369700327,
///    0.066533716758, -0.000343198063, -0.000562490295]
pub(super) const DEFAULT_ENERGY_FACTOR_CURVE: [f64; 6] = [
    -2.388_319_068_955,
    0.093_047_739_452,
    -0.001_369_700_327,
    6.653_371_675_8,
    -3.431_980_63,
    -0.056_249_029_5,
];

/// Dry-bulb temperature at the EnergyPlus `ZoneHVAC:Dehumidifier:DX` rated
/// condition: 26.7°C = 80°F.  Used with `RATED_RH` to normalise the curve
/// output so that the rated capacity passes through unchanged at the design
/// condition.
///
/// Source: EnergyPlus `ZoneDehumidifier.cc` constant `RatedInletAirTemp(26.7)`.
pub(super) const RATED_DB_C: f64 = 26.666_666_666_666_7;

/// Relative humidity (fraction) at the EnergyPlus `ZoneHVAC:Dehumidifier:DX`
/// rated condition: 60 %.  Used with `RATED_DB_C` to normalise the curve output.
///
/// Source: EnergyPlus `ZoneDehumidifier.cc` constant `RatedInletAirRH(60.0)`.
pub(super) const RATED_RH: f64 = 0.60;

/// Dry-bulb temperature bounds from the EnergyPlus default dehumidifier curves
/// (`Minimum Value of x` / `Maximum Value of x` in `Curve:Biquadratic`).
/// Original: (21.0, 32.22) °C.  Used only in self-tests.
#[cfg(test)]
const DEFAULT_DB_BOUNDS_FROM_CURVE_C: (f64, f64) = (21.0, 32.22);

/// Relative-humidity bounds from the EnergyPlus default dehumidifier curves
/// (`Minimum Value of y` / `Maximum Value of y` in `Curve:Biquadratic`).
/// Original: (40, 80) %  →  (0.40, 0.80) fraction.  Used only in self-tests.
#[cfg(test)]
const DEFAULT_RH_BOUNDS_FROM_CURVE: (f64, f64) = (0.40, 0.80);

#[cfg(test)]
mod tests {
    use hares_physics::biquadratic::BiquadraticCurve;

    use super::*;

    /// At the rated condition the raw curve output must be reasonably close to
    /// 1.0 so the normalisation divisor does not distort the rated capacity.
    /// EnergyPlus validates 0.90 ≤ curve_val ≤ 1.10 at (26.7°C, 60 %RH).
    #[test]
    fn water_removal_at_rated_is_near_unity() {
        let wr = BiquadraticCurve {
            coeffs: DEFAULT_WATER_REMOVAL_CURVE,
            x1_bounds: DEFAULT_DB_BOUNDS_FROM_CURVE_C,
            x2_bounds: DEFAULT_RH_BOUNDS_FROM_CURVE,
            warn_on_clamp: false,
        };
        let val = wr.evaluate(RATED_DB_C, RATED_RH);
        assert!(
            (0.90..=1.10).contains(&val),
            "water removal curve at rated ({},{}) = {val:.6}, expected 0.90–1.10",
            RATED_DB_C,
            RATED_RH,
        );
    }

    /// At the rated condition the energy-factor curve must also be near unity.
    #[test]
    fn energy_factor_at_rated_is_near_unity() {
        let ef = BiquadraticCurve {
            coeffs: DEFAULT_ENERGY_FACTOR_CURVE,
            x1_bounds: DEFAULT_DB_BOUNDS_FROM_CURVE_C,
            x2_bounds: DEFAULT_RH_BOUNDS_FROM_CURVE,
            warn_on_clamp: false,
        };
        let val = ef.evaluate(RATED_DB_C, RATED_RH);
        assert!(
            (0.90..=1.10).contains(&val),
            "energy factor curve at rated ({},{}) = {val:.6}, expected 0.90–1.10",
            RATED_DB_C,
            RATED_RH,
        );
    }

    /// At 10°C / 60 %RH the water removal curve must output substantially less
    /// than at the rated condition (26.7°C / 60 %RH) — this proves the curve
    /// encodes temperature dependence.
    #[test]
    fn water_removal_decreases_at_lower_temperature() {
        let wr = BiquadraticCurve {
            coeffs: DEFAULT_WATER_REMOVAL_CURVE,
            x1_bounds: DEFAULT_DB_BOUNDS_FROM_CURVE_C,
            x2_bounds: DEFAULT_RH_BOUNDS_FROM_CURVE,
            warn_on_clamp: false,
        };
        let val_rated = wr.evaluate(RATED_DB_C, RATED_RH);
        let val_cold = wr.evaluate(10.0, 0.60);
        assert!(
            val_cold < val_rated,
            "water removal at 10°C ({val_cold:.6}) must be less than at rated ({val_rated:.6})"
        );
    }
}
