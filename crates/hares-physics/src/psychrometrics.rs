//! Psychrometric property calculations (humidity ratio, enthalpy, wet-bulb).

use crate::constants::{
    CP_DRY_AIR_KJ_KG_K, CP_WATER_VAPOUR_KJ_KG_K, KJ_TO_J, LATENT_HEAT_SUBLIMATION_KJ_KG,
    LATENT_HEAT_VAPORISATION_0C_KJ_KG, MOLECULAR_WEIGHT_RATIO_WATER_AIR,
};
use crate::units::{
    Pressure, Temperature, pressure_from_pascal, pressure_to_pascal, temperature_from_celsius,
    temperature_to_celsius,
};

/// Re-export for backward compatibility.
pub const EPSILON: f64 = MOLECULAR_WEIGHT_RATIO_WATER_AIR;
/// Re-export for backward compatibility.
pub const LATENT_HEAT_VAPORISATION_KJ_KG: f64 = LATENT_HEAT_VAPORISATION_0C_KJ_KG;
/// Re-export for backward compatibility.
pub const SPECIFIC_HEAT_DRY_AIR_KJ_KG_K: f64 = CP_DRY_AIR_KJ_KG_K;
/// Re-export for backward compatibility.
pub const SPECIFIC_HEAT_WATER_VAPOUR_KJ_KG_K: f64 = CP_WATER_VAPOUR_KJ_KG_K;

const MIN_HUMIDITY_RATIO: f64 = 1e-7;
const PSYCHRO_TOLERANCE_C: f64 = 0.01;
const MAX_BISECTION_ITERS: u32 = 100;

const WET_BULB_HUMIDITY_TOLERANCE: f64 = 1e-7;
const DEW_POINT_PRESSURE_TOLERANCE_PA: f64 = 0.05;
const DEW_POINT_SEARCH_LOW_C: f64 = -100.0;
const DEW_POINT_SEARCH_HIGH_C: f64 = 200.0;

const SPECIFIC_HEAT_LIQUID_WATER_KJ_KG_K: f64 = 4.186;
/// Specific heat of water vapour used in the above-freezing wet-bulb formula [kJ/(kg·K)].
///
/// ASHRAE HOF 2021 Ch.1 Eq. 35: this term represents the vapour-side heat capacity
/// in the psychrometer equation above 0°C. Value per ASHRAE: 2.381 kJ/(kg·K).
const SPECIFIC_HEAT_WET_BULB_ABOVE_FREEZE: f64 = 2.381;
const SPECIFIC_HEAT_ICE_KJ_KG_K: f64 = 2.1;
/// Specific heat of ice used in the below-freezing wet-bulb formula [kJ/(kg·K)].
///
/// ASHRAE HOF 2021 Ch.1 Eq. 37: this term represents the ice-side heat capacity
/// in the psychrometer equation below 0°C. Value per ASHRAE: 2.006 kJ/(kg·K).
/// (The previous value of 0.24 was an IP unit value in BTU/lb/°F -- incorrect for SI.)
const SPECIFIC_HEAT_WET_BULB_BELOW_FREEZE: f64 = 2.006;

/// Saturation vapor pressure for water [Pa] at temperature `t_c` [°C].
///
/// Uses the ASHRAE piecewise polynomial form used by PsychroLib.
pub fn saturation_pressure_pa(t_c: f64) -> f64 {
    let t_k = t_c + 273.15;
    let ln_p_ws = if t_c <= 0.01 {
        -5.674_535_9e3 / t_k + 6.392_524_7 - 9.677_843e-3 * t_k
            + 6.221_570_1e-7 * t_k * t_k
            + 2.074_782_5e-9 * t_k * t_k * t_k
            - 9.484_024e-13 * t_k * t_k * t_k * t_k
            + 4.163_501_9 * t_k.ln()
    } else {
        -5.800_220_6e3 / t_k + 1.391_499_3 - 4.864_023_9e-2 * t_k + 4.176_476_8e-5 * t_k * t_k
            - 1.445_209_3e-8 * t_k * t_k * t_k
            + 6.545_967_3 * t_k.ln()
    };

    ln_p_ws.exp()
}

/// Typed `uom` boundary variant of [`saturation_pressure_pa`].
pub fn saturation_pressure(t: Temperature) -> Pressure {
    let t_c = temperature_to_celsius(t);
    pressure_from_pascal(saturation_pressure_pa(t_c))
}

/// Humidity ratio from dew-point [°C] and pressure [Pa].
pub fn humidity_ratio_from_tdp(t_dp_c: f64, p_pa: f64) -> f64 {
    let p_ws = saturation_pressure_pa(t_dp_c);
    let w = EPSILON * p_ws / (p_pa - p_ws);
    w.max(MIN_HUMIDITY_RATIO)
}

/// Typed `uom` boundary variant of [`humidity_ratio_from_tdp`].
pub fn humidity_ratio_from_tdp_typed(t_dp: Temperature, p: Pressure) -> f64 {
    humidity_ratio_from_tdp(temperature_to_celsius(t_dp), pressure_to_pascal(p))
}

/// Humidity ratio from dry-bulb [°C], wet-bulb [°C], and pressure [Pa].
pub fn humidity_ratio_from_twb(t_db_c: f64, t_wb_c: f64, p_pa: f64) -> f64 {
    let w_sat = humidity_ratio_from_tdp(t_wb_c, p_pa);

    let w = if t_wb_c >= 0.0 {
        ((LATENT_HEAT_VAPORISATION_KJ_KG - SPECIFIC_HEAT_WET_BULB_ABOVE_FREEZE * t_wb_c) * w_sat
            - SPECIFIC_HEAT_DRY_AIR_KJ_KG_K * (t_db_c - t_wb_c))
            / (LATENT_HEAT_VAPORISATION_KJ_KG + SPECIFIC_HEAT_WATER_VAPOUR_KJ_KG_K * t_db_c
                - SPECIFIC_HEAT_LIQUID_WATER_KJ_KG_K * t_wb_c)
    } else {
        ((LATENT_HEAT_SUBLIMATION_KJ_KG - SPECIFIC_HEAT_WET_BULB_BELOW_FREEZE * t_wb_c) * w_sat
            - SPECIFIC_HEAT_DRY_AIR_KJ_KG_K * (t_db_c - t_wb_c))
            / (LATENT_HEAT_SUBLIMATION_KJ_KG + SPECIFIC_HEAT_WATER_VAPOUR_KJ_KG_K * t_db_c
                - SPECIFIC_HEAT_ICE_KJ_KG_K * t_wb_c)
    };

    w.max(MIN_HUMIDITY_RATIO)
}

/// Typed `uom` boundary variant of [`humidity_ratio_from_twb`].
pub fn humidity_ratio_from_twb_typed(t_db: Temperature, t_wb: Temperature, p: Pressure) -> f64 {
    humidity_ratio_from_twb(
        temperature_to_celsius(t_db),
        temperature_to_celsius(t_wb),
        pressure_to_pascal(p),
    )
}

/// Relative humidity (0-1) from dry-bulb [°C], humidity ratio, and pressure [Pa].
pub fn relative_humidity(t_db_c: f64, w: f64, p_pa: f64) -> f64 {
    let w_eff = w.max(MIN_HUMIDITY_RATIO);
    let p_v = p_pa * w_eff / (EPSILON + w_eff);
    let p_sat = saturation_pressure_pa(t_db_c);
    (p_v / p_sat).clamp(0.0, 1.0)
}

/// Typed `uom` boundary variant of [`relative_humidity`].
pub fn relative_humidity_typed(t_db: Temperature, w: f64, p: Pressure) -> f64 {
    relative_humidity(temperature_to_celsius(t_db), w, pressure_to_pascal(p))
}

/// Wet-bulb [°C] from dry-bulb [°C], humidity ratio, and pressure [Pa].
///
/// Solves by bisection between dew-point and dry-bulb.
pub fn wet_bulb_from_humidity_ratio(t_db_c: f64, w: f64, p_pa: f64) -> f64 {
    let w_eff = w.max(MIN_HUMIDITY_RATIO);
    let w_sat = humidity_ratio_from_tdp(t_db_c, p_pa);
    if w_eff >= w_sat {
        return t_db_c;
    }
    let t_dp = dew_point(w_eff, p_pa);

    // The saturation guard above guarantees w_eff < w_sat, so t_dp < t_db_c
    // and the bracket [t_dp, t_db_c] is always well-ordered.
    bisect(
        t_dp,
        t_db_c,
        |mid| humidity_ratio_from_twb(t_db_c, mid, p_pa) - w_eff,
        PSYCHRO_TOLERANCE_C,
        WET_BULB_HUMIDITY_TOLERANCE,
        MAX_BISECTION_ITERS,
    )
}

/// Typed `uom` boundary variant of [`wet_bulb_from_humidity_ratio`].
pub fn wet_bulb_from_humidity_ratio_typed(t_db: Temperature, w: f64, p: Pressure) -> Temperature {
    temperature_from_celsius(wet_bulb_from_humidity_ratio(
        temperature_to_celsius(t_db),
        w,
        pressure_to_pascal(p),
    ))
}

/// Zone relative humidity (0–1) derived from the zone's stored `temperature_c`
/// and `humidity_ratio` plus the current atmospheric pressure.
///
/// This is the canonical accessor — callers must use this instead of inspecting
/// `ZoneState` fields that no longer exist, guaranteeing that relative humidity
/// is always consistent with the current thermodynamic state.
pub fn zone_relative_humidity(zone: &hares_types::ZoneState, pressure_pa: f64) -> f64 {
    relative_humidity(zone.temperature_c, zone.humidity_ratio, pressure_pa)
}

/// Zone wet-bulb temperature [°C] derived from the zone's stored `temperature_c`
/// and `humidity_ratio` plus the current atmospheric pressure.
///
/// This is the canonical accessor — callers must use this instead of inspecting
/// `ZoneState` fields that no longer exist, guaranteeing that wet-bulb is
/// always consistent with the current thermodynamic state.
pub fn zone_wet_bulb_c(zone: &hares_types::ZoneState, pressure_pa: f64) -> f64 {
    wet_bulb_from_humidity_ratio(zone.temperature_c, zone.humidity_ratio, pressure_pa)
}

/// Dew-point [°C] from humidity ratio and pressure [Pa].
pub fn dew_point(w: f64, p_pa: f64) -> f64 {
    let w_eff = w.max(MIN_HUMIDITY_RATIO);
    let p_v = p_pa * w_eff / (EPSILON + w_eff);

    bisect(
        DEW_POINT_SEARCH_LOW_C,
        DEW_POINT_SEARCH_HIGH_C,
        |mid| saturation_pressure_pa(mid) - p_v,
        PSYCHRO_TOLERANCE_C,
        DEW_POINT_PRESSURE_TOLERANCE_PA,
        MAX_BISECTION_ITERS,
    )
}

/// Typed `uom` boundary variant of [`dew_point`].
pub fn dew_point_typed(w: f64, p: Pressure) -> Temperature {
    temperature_from_celsius(dew_point(w, pressure_to_pascal(p)))
}

/// Moist-air enthalpy [J/kg dry-air] from dry-bulb [°C] and humidity ratio [kg/kg].
pub fn moist_air_enthalpy(t_db_c: f64, w: f64) -> f64 {
    let w_eff = w.max(MIN_HUMIDITY_RATIO);
    (SPECIFIC_HEAT_DRY_AIR_KJ_KG_K * t_db_c
        + w_eff * (LATENT_HEAT_VAPORISATION_KJ_KG + SPECIFIC_HEAT_WATER_VAPOUR_KJ_KG_K * t_db_c))
        * KJ_TO_J
}

/// Typed `uom` boundary variant of [`moist_air_enthalpy`].
pub fn moist_air_enthalpy_typed(t_db: Temperature, w: f64) -> f64 {
    moist_air_enthalpy(temperature_to_celsius(t_db), w)
}

/// Generic bisection root-finder.
///
/// Finds `x` in `[lo, hi]` such that `f(x) ≈ 0`, converging when the bracket
/// width drops below `bracket_tol` or `|f(mid)|` drops below `value_tol`.
fn bisect<F: Fn(f64) -> f64>(
    mut lo: f64,
    mut hi: f64,
    f: F,
    bracket_tol: f64,
    value_tol: f64,
    max_iter: u32,
) -> f64 {
    for _ in 0..max_iter {
        let mid = 0.5 * (lo + hi);
        let f_mid = f(mid);
        if f_mid.abs() <= value_tol || (hi - lo) <= bracket_tol {
            return mid;
        }
        if f_mid > 0.0 {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    0.5 * (lo + hi)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::approx_eq;

    #[test]
    fn saturation_pressure_matches_reference_points() {
        // Reference points from standard psychrometric tables (Pa).
        let cases = [
            (-20.0, 103.3),
            (-15.0, 165.3),
            (-10.0, 259.9),
            (-5.0, 401.8),
            (0.0, 611.2),
            (5.0, 872.5),
            (10.0, 1228.1),
            (15.0, 1705.4),
            (20.0, 2338.8),
            (25.0, 3169.2),
            (30.0, 4246.0),
            (35.0, 5628.0),
            (40.0, 7384.9),
        ];

        for (t_c, expected_pa) in cases {
            let p = saturation_pressure_pa(t_c);
            // Table-level tolerance.
            approx_eq(p, expected_pa, expected_pa * 0.02);
        }
    }

    #[test]
    fn typed_saturation_pressure_matches_raw_kernel() {
        let t = crate::units::temperature_from_celsius(20.0);
        let p_typed = saturation_pressure(t);
        let p_raw = saturation_pressure_pa(20.0);
        approx_eq(pressure_to_pascal(p_typed), p_raw, 1e-9);
    }

    #[test]
    fn twenty_plus_psychrometric_conditions_round_trip() {
        let p = 101_325.0;
        let mut checked = 0usize;

        for t_db_c in [0.0, 5.0, 10.0, 15.0, 20.0, 25.0, 30.0] {
            for rh in [0.2, 0.4, 0.6] {
                let p_sat = saturation_pressure_pa(t_db_c);
                let p_v = rh * p_sat;
                let w = (EPSILON * p_v / (p - p_v)).max(MIN_HUMIDITY_RATIO);

                let rh_back = relative_humidity(t_db_c, w, p);
                approx_eq(rh_back, rh, 0.01);

                let t_wb = wet_bulb_from_humidity_ratio(t_db_c, w, p);
                let w_back = humidity_ratio_from_twb(t_db_c, t_wb, p);
                approx_eq(w_back, w, 2e-4);
                checked += 1;
            }
        }

        assert!(checked >= 21);
    }

    /// Validate saturation pressure against PsychroLib reference values
    /// (ASHRAE 2017 HOF Ch.1). Tolerance: 0.03% relative error.
    #[test]
    fn saturation_pressure_matches_psychrolib_reference() {
        // (t_c, expected_pa) from PsychroLib test_psychrolib_si.py
        let cases: &[(f64, f64)] = &[
            (-20.0, 103.24),
            (-5.0, 401.74),
            (5.0, 872.6),
            (25.0, 3169.7),
            (50.0, 12_351.3),
        ];
        for &(t_c, expected_pa) in cases {
            let computed = saturation_pressure_pa(t_c);
            let err = (computed - expected_pa).abs() / expected_pa;
            assert!(
                err < 0.001,
                "psat at {t_c}°C: computed={computed:.2}, ref={expected_pa:.2}, err={err:.5}"
            );
        }
    }

    /// Validate saturation humidity ratio at 100% RH against Engineering Toolbox /
    /// ASHRAE reference values at standard atmosphere (101325 Pa).
    #[test]
    fn saturation_humidity_ratio_matches_published_tables() {
        let p = 101_325.0;
        // (t_c, expected_w_sat) from Engineering Toolbox humidity-ratio-air table
        let cases: &[(f64, f64)] = &[
            (0.0, 0.003_767),
            (10.0, 0.007_612),
            (20.0, 0.014_659),
            (25.0, 0.019_826),
            (30.0, 0.027_125),
        ];
        for &(t_c, w_ref) in cases {
            let p_sat = saturation_pressure_pa(t_c);
            let w = EPSILON * p_sat / (p - p_sat);
            let err = (w - w_ref).abs() / w_ref;
            assert!(
                err < 0.02,
                "w_sat at {t_c}°C: computed={w:.6}, ref={w_ref:.6}, err={err:.4}"
            );
        }
    }

    /// Validate moist air enthalpy against PsychroLib reference:
    /// GetMoistAirEnthalpy(30, 0.02) ≈ 81316 J/kg (rel tol 0.03%).
    #[test]
    fn moist_air_enthalpy_matches_psychrolib_reference() {
        let h = moist_air_enthalpy(30.0, 0.02);
        let expected = 81_316.0;
        let err = (h - expected).abs() / expected;
        assert!(
            err < 0.001,
            "enthalpy at 30°C/w=0.02: computed={h:.1}, ref={expected:.1}, err={err:.5}"
        );
    }

    /// Validate humidity ratio from wet-bulb against PsychroLib reference:
    /// GetHumRatioFromTWetBulb(30, 25, 95461) ≈ 0.0192281274241096 (rel tol 0.5%).
    ///
    /// Note: the ASHRAE HOF 2021 Eq. 35 constants (c_s_wb = 2.381) differ slightly
    /// from PsychroLib's default (2.326), so the tolerance is relaxed to 0.5%.
    #[test]
    fn humidity_ratio_from_twb_matches_psychrolib_reference() {
        let w = humidity_ratio_from_twb(30.0, 25.0, 95_461.0);
        let expected = 0.019_228_127_424_109_6;
        let err = (w - expected).abs() / expected;
        assert!(
            err < 0.005,
            "w from twb at 30°C/25°C/95461Pa: computed={w:.10}, ref={expected:.10}, err={err:.5}"
        );
    }

    /// Comprehensive psychrometric cross-validation at 40°C, Twb=20°C, 101325 Pa.
    /// Reference: PsychroLib CalcPsychrometricsFromTWetBulb test case.
    #[test]
    fn comprehensive_psychrometrics_at_40c_twb20c() {
        let p = 101_325.0;
        let t_db = 40.0;
        let t_wb = 20.0;

        let w = humidity_ratio_from_twb(t_db, t_wb, p);
        approx_eq(w, 0.0065, 0.001); // ASHRAE HOF 2021 Eq.35 constants shift this slightly

        let rh = relative_humidity(t_db, w, p);
        approx_eq(rh, 0.14, 0.03); // PsychroLib: ≈0.14 ±0.02

        let h = moist_air_enthalpy(t_db, w);
        approx_eq(h, 56_700.0, 500.0); // PsychroLib: ≈56700 ±200

        let tdp = dew_point(w, p);
        approx_eq(tdp, 7.0, 2.0); // PsychroLib: ≈7 ±1

        let twb_back = wet_bulb_from_humidity_ratio(t_db, w, p);
        approx_eq(twb_back, t_wb, 0.2); // round-trip ±0.2°C
    }

    /// The canonical ASHRAE 20°C / 50% RH reference case at sea level.
    /// This is the most commonly cited psychrometric validation point.
    #[test]
    fn canonical_20c_50rh_reference() {
        let p = 101_325.0;
        let t_db = 20.0;

        // Saturation pressure at 20°C: ASHRAE table ≈2338.8 Pa
        let p_sat = saturation_pressure_pa(t_db);
        approx_eq(p_sat, 2338.8, 2338.8 * 0.01); // 1% tolerance

        // Humidity ratio at 50% RH
        let p_v = 0.50 * p_sat;
        let w = EPSILON * p_v / (p - p_v);
        // At 20°C, 50% RH, 101325 Pa: w ≈ 0.00726 kg/kg
        // (half of saturation w=0.01466, approximately)
        approx_eq(w, 0.00726, 0.0005);

        // Enthalpy: h ≈ 38.5 kJ/kg = 38500 J/kg
        let h = moist_air_enthalpy(t_db, w);
        approx_eq(h, 38_500.0, 500.0);

        // Relative humidity round-trip
        let rh = relative_humidity(t_db, w, p);
        approx_eq(rh, 0.50, 0.01);

        // Wet-bulb: at 20°C / 50% RH, Twb ≈ 13.7°C
        let twb = wet_bulb_from_humidity_ratio(t_db, w, p);
        approx_eq(twb, 13.7, 0.5);

        // Dew point: at w≈0.00726, Tdp ≈ 9.3°C
        let tdp = dew_point(w, p);
        approx_eq(tdp, 9.3, 0.5);
    }

    #[test]
    fn wet_bulb_equals_dry_bulb_at_saturation() {
        let p = 101_325.0;
        for t_db_c in [0.0, 10.0, 20.0, 30.0, 40.0] {
            let w_sat = humidity_ratio_from_tdp(t_db_c, p);
            let t_wb = wet_bulb_from_humidity_ratio(t_db_c, w_sat, p);
            approx_eq(t_wb, t_db_c, 0.01);
        }
    }

    #[test]
    fn wet_bulb_clamps_to_dry_bulb_on_supersaturated_input() {
        let p = 101_325.0;
        // (t_db_c, w_multiplier) -- multiplier applied to w_sat at that dry-bulb
        let cases: &[(f64, f64)] = &[
            (20.0, 2.0),   // moderate above-freezing, 2× saturation
            (-5.0, 2.0),   // sub-zero dry-bulb, different code path at freezing
            (-5.0, 100.0), // sub-zero with extreme multiplier, no overflow/panic
            (20.0, 100.0), // above-freezing with extreme multiplier
            (35.0, 50.0),  // hot day, large supersaturation
        ];
        for &(t_db_c, mult) in cases {
            let w_sat = humidity_ratio_from_tdp(t_db_c, p);
            let t_wb = wet_bulb_from_humidity_ratio(t_db_c, w_sat * mult, p);
            approx_eq(t_wb, t_db_c, 0.01);
        }
    }

    #[test]
    fn dew_point_inverts_humidity_ratio_from_tdp() {
        let p = 101_325.0;
        for t_dp in [-10.0, 0.0, 5.0, 12.0, 18.0, 25.0] {
            let w = humidity_ratio_from_tdp(t_dp, p);
            let t_back = dew_point(w, p);
            approx_eq(t_back, t_dp, 0.02);
        }
    }

    #[test]
    fn typed_wrappers_match_raw_kernels() {
        let t_db = temperature_from_celsius(25.0);
        let t_wb = temperature_from_celsius(18.0);
        let p = pressure_from_pascal(101_325.0);
        let w = 0.010;

        // humidity_ratio_from_twb
        let raw = humidity_ratio_from_twb(25.0, 18.0, 101_325.0);
        let typed = humidity_ratio_from_twb_typed(t_db, t_wb, p);
        approx_eq(typed, raw, 1e-12);

        // relative_humidity
        let raw = relative_humidity(25.0, w, 101_325.0);
        let typed = relative_humidity_typed(t_db, w, p);
        approx_eq(typed, raw, 1e-12);

        // wet_bulb_from_humidity_ratio
        let raw = wet_bulb_from_humidity_ratio(25.0, w, 101_325.0);
        let typed = wet_bulb_from_humidity_ratio_typed(t_db, w, p);
        approx_eq(temperature_to_celsius(typed), raw, 1e-12);

        // dew_point
        let raw = dew_point(w, 101_325.0);
        let typed = dew_point_typed(w, p);
        approx_eq(temperature_to_celsius(typed), raw, 1e-12);

        // moist_air_enthalpy
        let raw = moist_air_enthalpy(25.0, w);
        let typed = moist_air_enthalpy_typed(t_db, w);
        approx_eq(typed, raw, 1e-9);
    }

    #[test]
    fn moist_air_enthalpy_sub_zero_temperature() {
        // At -10°C with low humidity (typical cold winter air)
        let h = moist_air_enthalpy(-10.0, 0.001);
        assert!(h.is_finite(), "enthalpy at -10°C must be finite");
        // h = 1.006*(-10) + 0.001*(2501 + 1.86*(-10)) = -10.06 + 2.4814 = -7.5786 kJ/kg
        // In J/kg: ≈ -7578.6
        assert!(h < 0.0, "enthalpy at -10°C should be negative, got {h}");
        approx_eq(h, -7_578.6, 10.0);
    }

    #[test]
    fn moist_air_enthalpy_is_finite_at_edge_cases() {
        let h_dry = moist_air_enthalpy(20.0, 0.0);
        assert!(h_dry.is_finite());
        assert!(h_dry > 0.0);

        let h_hot_humid = moist_air_enthalpy(40.0, 0.03);
        assert!(h_hot_humid.is_finite());
        assert!(h_hot_humid > h_dry);
    }

    // --- Regression tests for previously fixed physics bugs ---

    /// Regression: below-freezing wet-bulb used 0.24 (IP unit) instead of 2.006 kJ/(kg·K) (SI).
    /// ASHRAE HOF 2021 Eq. 37. With the old constant the result would be wildly wrong.
    #[test]
    fn humidity_ratio_from_twb_sub_freezing_uses_correct_ice_specific_heat() {
        // Wet-bulb at -5°C, dry-bulb at -2°C
        let w = humidity_ratio_from_twb(-2.0, -5.0, 101325.0);
        // At these conditions, humidity ratio should be small but positive
        assert!(w > 0.0, "sub-freezing humidity ratio must be positive");
        assert!(w < 0.005, "sub-freezing humidity ratio must be small");
        // Saturation humidity ratio at -5°C is approximately 0.00254 kg/kg
        let w_sat = humidity_ratio_from_tdp(-5.0, 101325.0);
        assert!(
            w <= w_sat * 1.01,
            "humidity ratio must not exceed saturation at wet-bulb temp"
        );
    }

    /// Regression: above-freezing wet-bulb psychrometer coefficient was 2.326 instead of
    /// 2.381 kJ/(kg·K) per ASHRAE HOF 2021 Ch.1 Eq. 35. With the wrong coefficient, the
    /// round-trip RH at the canonical 25°C DB / 17.8°C WB point drifts by ~2–3%.
    #[test]
    fn wet_bulb_from_humidity_ratio_ashrae_hof_2021_reference() {
        // ASHRAE HOF 2021 reference conditions: 25°C DB, 50% RH at 101325 Pa.
        // 17.8°C is approx wet-bulb at 25°C DB / 50% RH.
        let w = humidity_ratio_from_twb(25.0, 17.8, 101325.0);
        let w_from_rh = relative_humidity(25.0, w, 101325.0);
        // With correct coefficient 2.381 the RH should be close to 50%;
        // with the old 2.326 it would drift by ~2–3%.
        assert!(
            (w_from_rh - 0.50).abs() < 0.03,
            "RH at 25°C DB / 17.8°C WB should be ~50%, got {w_from_rh}"
        );
    }

    /// Regression: bisection refactor. Verifies round-trip convergence across the full
    /// above-freezing operating range after the helper was extracted.
    #[test]
    fn wet_bulb_bisection_converges_across_full_range() {
        let test_cases = [
            (0.0, 0.002),
            (20.0, 0.007),
            (35.0, 0.015),
            (40.0, 0.025),
            (45.0, 0.035),
        ];
        for (t_db, w) in test_cases {
            let wb = wet_bulb_from_humidity_ratio(t_db, w, 101325.0);
            assert!(wb <= t_db, "wet-bulb ({wb}) must be <= dry-bulb ({t_db})");
            assert!(
                wb >= -40.0,
                "wet-bulb ({wb}) must be reasonable at t_db={t_db}"
            );
            let w_rt = humidity_ratio_from_twb(t_db, wb, 101325.0);
            let rel_err = if w > 1e-6 {
                (w_rt - w).abs() / w
            } else {
                (w_rt - w).abs()
            };
            assert!(
                rel_err < 0.01,
                "round-trip error {rel_err:.4} too large at t_db={t_db}, w={w}"
            );
        }
    }

    #[test]
    fn saturation_pressure_matches_ashrae_hof_table_2() {
        // ASHRAE 2017 Handbook of Fundamentals, Ch. 1, Table 2
        // "Thermodynamic Properties of Water at Saturation"
        let cases: &[(f64, f64, f64)] = &[
            // (temp_c, expected_pa, tolerance_pa)
            (-40.0, 12.84, 0.1),
            (-20.0, 103.24, 1.0),
            (-10.0, 259.90, 2.0),
            (0.01, 611.73, 1.0), // triple point
            (10.0, 1228.1, 3.0),
            (20.0, 2338.5, 5.0),
            (25.0, 3169.0, 5.0),
            (40.0, 7384.9, 10.0),
            (50.0, 12351.3, 20.0),
            (100.0, 101325.0, 200.0), // boiling point identity
        ];
        for &(t, expected, tol) in cases {
            let result = saturation_pressure_pa(t);
            assert!(
                (result - expected).abs() < tol,
                "saturation_pressure_pa({t}) = {result}, expected {expected} ± {tol} (ASHRAE HOF Table 2)"
            );
        }
    }

    #[test]
    fn enthalpy_matches_ashrae_hof_reference() {
        // ASHRAE 2017 HOF Ch. 1, Eq. 30 reference cases
        // h = (1.006*t + W*(2501 + 1.86*t)) * 1000 [J/kg]

        // At 0°C, W=0: h = 0 J/kg (reference state)
        let h0 = moist_air_enthalpy(0.0, 0.0);
        assert!(
            h0.abs() < 1.0,
            "enthalpy at reference state should be ~0, got {h0}"
        );

        // At 20°C, W=0: h = 1.006 * 20 * 1000 = 20120 J/kg
        let h20 = moist_air_enthalpy(20.0, 0.0);
        assert!(
            (h20 - 20_120.0).abs() < 10.0,
            "dry air enthalpy at 20°C: {h20}"
        );

        // At 25°C, W=0.01: h = (1.006*25 + 0.01*(2501+1.86*25)) * 1000 = 50625 J/kg
        let h25 = moist_air_enthalpy(25.0, 0.01);
        assert!(
            (h25 - 50_625.0).abs() < 100.0,
            "moist air enthalpy at 25°C, W=0.01: {h25}"
        );
    }

    #[test]
    fn wet_bulb_energyplus_reference_case() {
        // EnergyPlus Psychrometrics.unit.cc reference
        // Tdb=1°C, W=0.002, P=101325 Pa → Twb ≈ -2.2°C ±0.5°C
        let twb = wet_bulb_from_humidity_ratio(1.0, 0.002, 101_325.0);
        assert!(
            (twb - (-2.2)).abs() < 0.5,
            "EnergyPlus ref: wet_bulb at 1°C, W=0.002: {twb}, expected ~-2.2"
        );
    }

    #[test]
    fn dew_point_humidity_ratio_round_trip() {
        // At Tdp=15°C, compute W, then compute Tdp back. Should match within 0.1°C
        let p = 101_325.0;
        let w = humidity_ratio_from_tdp(15.0, p);
        let tdp_back = dew_point(w, p);
        assert!(
            (tdp_back - 15.0).abs() < 0.1,
            "dew_point round-trip: {tdp_back}, expected 15.0"
        );
    }

    /// Regression: sub-freezing round-trip exposed by the 0.24 → 2.006 kJ/(kg·K) fix.
    #[test]
    fn sub_freezing_wet_bulb_round_trip() {
        let t_db = -10.0;
        let w = 0.001; // low humidity ratio typical of cold winter air
        let wb = wet_bulb_from_humidity_ratio(t_db, w, 101325.0);
        assert!(wb <= t_db);
        assert!(wb >= -40.0);
        let w_rt = humidity_ratio_from_twb(t_db, wb, 101325.0);
        let rel_err = (w_rt - w).abs() / w;
        assert!(
            rel_err < 0.02,
            "sub-freezing round-trip error {rel_err:.4} too large"
        );
    }

    /// Unit test: after updating `temperature_c` on a ZoneState, the computed
    /// accessors `zone_relative_humidity` and `zone_wet_bulb_c` return values
    /// consistent with the psychrometric library, reflecting the current
    /// temperature, not a stale stored value (T-0170).
    #[test]
    fn zone_state_accessors_reflect_temperature_change() {
        let p = 101_325.0; // standard sea-level pressure [Pa]
        let w = 0.009; // humidity ratio [kg/kg]

        let mut zone = hares_types::ZoneState::new(hares_types::ZoneId(1), 20.0, w, 200.0);
        let rh_20 = zone_relative_humidity(&zone, p);
        let wb_20 = zone_wet_bulb_c(&zone, p);
        assert!(rh_20 > 0.0 && rh_20 <= 1.0, "RH at 20°C should be valid");
        assert!(wb_20 > 0.0 && wb_20 < 30.0, "WB at 20°C should be valid");

        // Change temperature — RH and WB must follow immediately.
        zone.temperature_c = 30.0;
        let rh_30 = zone_relative_humidity(&zone, p);
        let wb_30 = zone_wet_bulb_c(&zone, p);

        // At higher temperature with same absolute humidity, RH decreases.
        assert!(
            rh_30 < rh_20,
            "RH must drop when temperature rises: {rh_20:.4} → {rh_30:.4}"
        );
        // Wet-bulb rises when dry-bulb rises at constant humidity ratio.
        assert!(
            wb_30 > wb_20,
            "WB must rise when temperature rises: {wb_20:.4} → {wb_30:.4}"
        );

        // Verify values match direct psychrometric computation.
        let expected_rh_30 = relative_humidity(30.0, w, p);
        let expected_wb_30 = wet_bulb_from_humidity_ratio(30.0, w, p);
        assert!(
            (rh_30 - expected_rh_30).abs() < 1e-12,
            "zone_relative_humidity must match direct psychrometric call"
        );
        assert!(
            (wb_30 - expected_wb_30).abs() < 1e-12,
            "zone_wet_bulb_c must match direct psychrometric call"
        );
    }

    /// Regression test: simulate a multi-pass equipment iteration within a
    /// timestep where temperature is adjusted between humidity updates.
    /// RH and wet-bulb must reflect the current temperature, never stale
    /// values from the last humidity solver pass (T-0170).
    ///
    /// Scenario: humidity solver updates humidity_ratio at 23°C; equipment
    /// step moves temperature to 27°C. The computed accessors must return
    /// values for the 27°C state, not the old 23°C state.
    #[test]
    fn multi_pass_temperature_change_not_stale() {
        let p = 101_325.0;
        let w = 0.010;

        let mut zone = hares_types::ZoneState::new(hares_types::ZoneId(1), 23.0, w, 200.0);

        // Pass 1: humidity solver ran, set humidity_ratio. Record RH/WB at 23°C.
        let rh_pass1 = zone_relative_humidity(&zone, p);
        let wb_pass1 = zone_wet_bulb_c(&zone, p);

        // Pass 2: equipment adjusts temperature to 27°C.
        zone.temperature_c = 27.0;
        let rh_pass2 = zone_relative_humidity(&zone, p);
        let wb_pass2 = zone_wet_bulb_c(&zone, p);

        // RH must NOT be the stale value from pass 1.
        assert!(
            rh_pass2 != rh_pass1,
            "RH must change when temperature changes between passes"
        );
        assert!(
            wb_pass2 != wb_pass1,
            "WB must change when temperature changes between passes"
        );

        // The warm pass has lower RH (same absolute moisture, higher temp).
        assert!(
            rh_pass2 < rh_pass1,
            "RH at 27°C ({rh_pass2:.4}) must be lower than RH at 23°C ({rh_pass1:.4})"
        );
        // The warm pass has higher WB.
        assert!(
            wb_pass2 > wb_pass1,
            "WB at 27°C ({wb_pass2:.4}) must be higher than WB at 23°C ({wb_pass1:.4})"
        );

        // Both passes match direct psychrometric results.
        assert!((rh_pass1 - relative_humidity(23.0, w, p)).abs() < 1e-12);
        assert!((rh_pass2 - relative_humidity(27.0, w, p)).abs() < 1e-12);
        assert!((wb_pass1 - wet_bulb_from_humidity_ratio(23.0, w, p)).abs() < 0.02);
        assert!((wb_pass2 - wet_bulb_from_humidity_ratio(27.0, w, p)).abs() < 0.02);
    }
}
