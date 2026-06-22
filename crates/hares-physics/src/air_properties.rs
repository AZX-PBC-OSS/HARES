//! Moist-air thermodynamic properties (dry-air basis).

use crate::constants::{
    CELSIUS_TO_KELVIN, DRY_AIR_GAS_CONSTANT_J_KG_K, HUMIDITY_DENSITY_CORRECTION,
    ISA_LAPSE_COEFFICIENT, ISA_PRESSURE_EXPONENT, MIN_HUMIDITY_RATIO_DENSITY,
    SEA_LEVEL_PRESSURE_PA,
};
use crate::units::*;
use uom::si::length::meter;

/// ISA standard pressure at altitude [Pa].
pub fn standard_pressure_pa(elevation_m: f64) -> f64 {
    SEA_LEVEL_PRESSURE_PA * (1.0 - ISA_LAPSE_COEFFICIENT * elevation_m).powf(ISA_PRESSURE_EXPONENT)
}

/// Dry-air-basis density of moist air [kg_da/m³].
///
/// Inverts ASHRAE HOF 2021 Ch.1 Eq.28 moist-air specific volume
/// `v = R_da·T·(1 + W·M_da/M_w) / p  [m³/kg_da]`, yielding kg_da per m³.
/// Uses the EnergyPlus `PsyRhoAirFnPbTdbW` style humidity floor guard.
pub fn moist_air_density_kg_m3(p_pa: f64, t_db_c: f64, w: f64) -> f64 {
    let w_eff = w.max(MIN_HUMIDITY_RATIO_DENSITY);
    p_pa / (DRY_AIR_GAS_CONSTANT_J_KG_K
        * (t_db_c + CELSIUS_TO_KELVIN)
        * (1.0 + HUMIDITY_DENSITY_CORRECTION * w_eff))
}

/// Dry-air density [kg/m^3].
pub fn dry_air_density_kg_m3(p_pa: f64, t_c: f64) -> f64 {
    p_pa / (DRY_AIR_GAS_CONSTANT_J_KG_K * (t_c + CELSIUS_TO_KELVIN))
}

// ── Typed `uom` boundary wrappers ──────────────────────────────────────────

/// ISA standard pressure at altitude (typed wrapper).
pub fn standard_pressure(elevation: Length) -> Pressure {
    let result_pa = standard_pressure_pa(elevation.get::<meter>());
    pressure_from_pascal(result_pa)
}

/// Dry-air-basis density of moist air [kg_da/m³] (typed wrapper).
///
/// Returns raw `f64` because density lacks a type alias.
pub fn moist_air_density(p: Pressure, t_db: Temperature, w: f64) -> f64 {
    moist_air_density_kg_m3(pressure_to_pascal(p), temperature_to_celsius(t_db), w)
}

/// Dry-air density [kg/m³] (typed wrapper).
///
/// Returns raw `f64` because density lacks a type alias.
pub fn dry_air_density(p: Pressure, t: Temperature) -> f64 {
    dry_air_density_kg_m3(pressure_to_pascal(p), temperature_to_celsius(t))
}

/// Invariant: air density must be within plausible atmospheric range [0.8, 1.5] kg/m³
/// and finite. Range covers Death Valley summer (hot, low density) to high-altitude
/// winter (cold, high density). Values outside this range indicate either a unit
/// error (Pa vs kPa) or corrupted weather data.
///
/// Gated behind `debug_assertions` or `check_invariants` feature to avoid per-timestep
/// overhead in production release builds.
#[cfg(any(debug_assertions, feature = "check_invariants"))]
pub fn check_air_density_plausible(rho: f64, context: &str) {
    assert!(
        rho.is_finite(),
        "air density is non-finite: {rho} at {context}"
    );
    assert!(
        (0.8..=1.5).contains(&rho),
        "air density {rho:.5} kg/m³ out of plausible range [0.8, 1.5] at {context}"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::approx_eq;

    #[test]
    fn standard_pressure_matches_isa_reference() {
        approx_eq(standard_pressure_pa(0.0), 101_325.0, 1.0);
        approx_eq(standard_pressure_pa(1000.0), 89_874.6, 400.0);
        approx_eq(standard_pressure_pa(1609.0), 83_431.1, 10.0);
    }

    #[test]
    fn denver_density_is_about_18_percent_lower_than_sea_level() {
        let t_c = 20.0;
        let w = 0.008;

        let rho_sea = moist_air_density_kg_m3(standard_pressure_pa(0.0), t_c, w);
        let rho_denver = moist_air_density_kg_m3(standard_pressure_pa(1609.0), t_c, w);
        let reduction = 1.0 - rho_denver / rho_sea;

        approx_eq(reduction, 0.18, 0.01);
    }

    #[test]
    fn moist_air_density_with_zero_humidity_is_finite_positive() {
        let rho = moist_air_density_kg_m3(101_325.0, 25.0, 0.0);
        assert!(rho.is_finite());
        assert!(rho > 0.0);
    }

    #[test]
    fn dry_and_moist_density_are_consistent_at_low_humidity() {
        let p = 101_325.0;
        let t = 20.0;
        let rho_dry = dry_air_density_kg_m3(p, t);
        let rho_moist = moist_air_density_kg_m3(p, t, 1e-6);
        assert!(rho_moist <= rho_dry);
        approx_eq(rho_moist, rho_dry, 0.001);
    }

    #[test]
    fn typed_standard_pressure_matches_raw() {
        use uom::si::length::meter as m;
        for &elev in &[0.0, 500.0, 1000.0, 1609.0, 3000.0] {
            let raw = standard_pressure_pa(elev);
            let typed = pressure_to_pascal(standard_pressure(Length::new::<m>(elev)));
            approx_eq(typed, raw, 1e-9);
        }
    }

    #[test]
    fn typed_moist_air_density_matches_raw() {
        let p_pa = 101_325.0;
        let t_c = 25.0;
        let w = 0.010;
        let raw = moist_air_density_kg_m3(p_pa, t_c, w);
        let typed = moist_air_density(pressure_from_pascal(p_pa), temperature_from_celsius(t_c), w);
        approx_eq(typed, raw, 1e-12);
    }

    #[test]
    fn isa_sea_level_dry_air_density() {
        // ISA 1976: at 101325 Pa, 15°C → ρ = 1.2250 kg/m³
        let rho = dry_air_density_kg_m3(101_325.0, 15.0);
        assert!(
            (rho - 1.2250).abs() < 0.0005,
            "ISA sea-level density: {rho}, expected 1.2250 ± 0.0005"
        );
    }

    #[test]
    fn standard_pressure_matches_isa_table() {
        // ISA 1976 / ICAO Doc 7488 standard atmosphere table
        let cases: &[(f64, f64, f64)] = &[
            // (altitude_m, expected_pa, tolerance_pa)
            (0.0, 101_325.0, 0.1), // sea level (exact by definition)
            (500.0, 95_461.0, 50.0),
            (1000.0, 89_874.6, 50.0),
            (1609.0, 83_431.1, 10.0), // Denver (ISA 1976 geometric)
            (3000.0, 70_108.0, 100.0),
        ];
        for &(alt, expected, tol) in cases {
            let result = standard_pressure_pa(alt);
            assert!(
                (result - expected).abs() < tol,
                "standard_pressure_pa({alt}) = {result}, expected {expected} ± {tol} (ISA 1976)"
            );
        }
    }

    #[test]
    fn moist_air_density_zero_pressure_does_not_panic() {
        // 0 Pa produces 0 density (0 / finite = 0), should not panic or NaN
        let rho = moist_air_density_kg_m3(0.0, 20.0, 0.01);
        assert!(rho.is_finite(), "density at 0 Pa must be finite, got {rho}");
        approx_eq(rho, 0.0, 1e-15);
    }

    #[test]
    fn typed_dry_air_density_matches_raw() {
        let p_pa = 101_325.0;
        let t_c = 20.0;
        let raw = dry_air_density_kg_m3(p_pa, t_c);
        let typed = dry_air_density(pressure_from_pascal(p_pa), temperature_from_celsius(t_c));
        approx_eq(typed, raw, 1e-12);
    }

    #[test]
    fn dry_air_density_sea_level_20c_matches_expected() {
        // ASHRAE HoF 2021 Ch.1 Eq.28: ρ = P / (R_da × T)
        // At sea level (101325 Pa) and 20°C (293.15 K):
        // ρ = 101325 / (287.058 × 293.15) ≈ 1.2041 kg/m³
        let rho = dry_air_density_kg_m3(101_325.0, 20.0);
        let expected = 101_325.0 / (crate::constants::DRY_AIR_GAS_CONSTANT_J_KG_K * 293.15);
        approx_eq(rho, expected, 1e-12);
        assert!(
            (rho - 1.2041).abs() < 0.001,
            "sea-level dry air density at 20°C: {rho}, expected ~1.2041"
        );
    }

    #[test]
    fn dry_air_density_denver_altitude_correction() {
        // Denver ~1609 m: ISA standard pressure ≈ 83431 Pa
        // ρ_denver = 83431 / (287.058 × 293.15) ≈ 0.992 kg/m³
        // Reduction vs sea level: 1 − 0.992/1.204 ≈ 17.6%
        let p_sea = standard_pressure_pa(0.0);
        let p_denver = standard_pressure_pa(1609.0);
        let t_c = 20.0;

        let rho_sea = dry_air_density_kg_m3(p_sea, t_c);
        let rho_denver = dry_air_density_kg_m3(p_denver, t_c);

        let reduction_pct = (1.0 - rho_denver / rho_sea) * 100.0;
        assert!(
            reduction_pct > 16.0 && reduction_pct < 19.0,
            "Denver density reduction: {reduction_pct:.1}%, expected ~17.6%"
        );
    }

    #[test]
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    fn invariant_check_accepts_plausible_density() {
        check_air_density_plausible(1.2041, "sea level 20°C");
        check_air_density_plausible(0.99, "Denver 20°C");
        check_air_density_plausible(1.30, "cold day");
        check_air_density_plausible(0.85, "Death Valley summer");
    }

    #[test]
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    #[should_panic(expected = "out of plausible range")]
    fn invariant_check_rejects_too_low_density() {
        check_air_density_plausible(0.5, "too low");
    }

    #[test]
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    #[should_panic(expected = "out of plausible range")]
    fn invariant_check_rejects_too_high_density() {
        check_air_density_plausible(2.0, "too high");
    }
}
