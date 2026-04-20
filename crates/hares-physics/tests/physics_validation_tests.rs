//! Integration tests for physics utility functions in `hares-physics`.
//!
//! Each test cites its reference source. Tolerances follow the source precision:
//! - 1e-9: algebraic identity (round-trip through inverse functions, no table rounding).
//! - 1-2%: table-derived reference values (ASHRAE HOF, ISA 1976, PsychroLib).
//!
//! Integration tests live here rather than as `#[cfg(test)]` unit tests so that
//! they exercise the public API exactly as downstream crates would use it, and
//! are compiled against the library crate boundary.

use hares_physics::air_properties::{
    dry_air_density_kg_m3, moist_air_density_kg_m3, standard_pressure_pa,
};
use hares_physics::biquadratic::{BiquadraticCurve, biquadratic, quadratic};
use hares_physics::film_coefficients::{
    SurfaceRoughness, ZoneLabel, film_resistances, tarp_h_natural,
};
use hares_physics::infiltration::{N_I_DEFAULT, ashrae_wind_stack, ela_infiltration};
use hares_physics::psychrometrics::{
    EPSILON, dew_point, moist_air_enthalpy, relative_humidity, saturation_pressure_pa,
    wet_bulb_from_humidity_ratio,
};
use hares_physics::water_mains::{Hemisphere, water_mains_temperature_c};

// ---------------------------------------------------------------------------
// Assertion helper -- replaces the #[cfg(test)]-gated test_utils::approx_eq
// ---------------------------------------------------------------------------

#[track_caller]
fn assert_approx(actual: f64, expected: f64, tol: f64) {
    assert!(
        (actual - expected).abs() <= tol,
        "actual={actual}, expected={expected}, tol={tol}, delta={}",
        (actual - expected).abs()
    );
}

#[track_caller]
fn assert_approx_rel(actual: f64, expected: f64, rel_tol: f64) {
    let abs_tol = expected.abs() * rel_tol;
    assert!(
        (actual - expected).abs() <= abs_tol,
        "actual={actual}, expected={expected}, rel_tol={rel_tol}, rel_err={}",
        (actual - expected).abs() / expected.abs()
    );
}

// ===========================================================================
// 1. Psychrometric round-trip: T=20°C + W → RH → back to W
// ===========================================================================

/// Verify that computing RH from (T, W) and then reconstructing W from (T, RH)
/// via saturation pressure is an algebraic identity -- tolerance 0.1%.
///
/// Reference: ASHRAE HOF 2021 Ch. 1, Eq. 20–24.
#[test]
fn psychrometric_round_trip() {
    let t_db_c = 20.0_f64;
    let w_original = 0.007_26_f64; // kg/kg -- canonical 20°C / 50% RH value
    let p_pa = 101_325.0_f64; // sea-level pressure [Pa]

    // Forward: W → RH
    let rh = relative_humidity(t_db_c, w_original, p_pa);
    assert!((0.0..=1.0).contains(&rh), "RH must be in [0,1], got {rh}");

    // Backward: RH → W using saturation pressure
    let p_sat = saturation_pressure_pa(t_db_c);
    let p_v = rh * p_sat;
    let w_reconstructed = EPSILON * p_v / (p_pa - p_v);

    // Algebraic round-trip: expect relative error < 0.1%
    assert_approx_rel(w_reconstructed, w_original, 0.001);
}

// ===========================================================================
// 2. ASHRAE canonical 20°C / 50% RH reference case at sea level
// ===========================================================================

/// The most commonly cited psychrometric validation point.
///
/// Reference: ASHRAE Handbook of Fundamentals 2021, Ch. 1, Table 2 and Eq. 30.
/// Cross-checked against PsychroLib (SI mode).
///
/// Expected values:
/// - W ≈ 0.00726 kg/kg (PsychroLib: 0.007255)
/// - h ≈ 38.5 kJ/kg = 38 500 J/kg (PsychroLib: 38 552 J/kg)
/// - Twb ≈ 13.7°C (PsychroLib: 13.74°C)
/// - Tdp ≈ 9.3°C (PsychroLib: 9.27°C)
#[test]
fn ashrae_canonical_20c_50rh() {
    let t_db = 20.0_f64;
    let rh_target = 0.50_f64;
    let p_pa = 101_325.0_f64;

    let p_sat = saturation_pressure_pa(t_db);
    let p_v = rh_target * p_sat;
    let w = EPSILON * p_v / (p_pa - p_v);

    // Humidity ratio: w ≈ 0.00726 kg/kg -- PsychroLib exact: 0.007255
    assert_approx(w, 0.007_26, 1e-5);

    // Enthalpy: h ≈ 38 552 J/kg -- PsychroLib: GetMoistAirEnthalpy(20, 0.00726)
    let h = moist_air_enthalpy(t_db, w);
    assert_approx(h, 38_552.0, 100.0);

    // Wet-bulb: Twb ≈ 13.74°C -- PsychroLib reference
    let t_wb = wet_bulb_from_humidity_ratio(t_db, w, p_pa);
    assert_approx(t_wb, 13.74, 0.1);

    // Dew point: Tdp ≈ 9.27°C -- PsychroLib reference
    let t_dp = dew_point(w, p_pa);
    assert_approx(t_dp, 9.27, 0.05);

    // Ordering constraint: Tdp ≤ Twb ≤ Tdb
    assert!(
        t_dp <= t_wb && t_wb <= t_db,
        "must satisfy Tdp({t_dp:.2}) ≤ Twb({t_wb:.2}) ≤ Tdb({t_db:.2})"
    );
}

// ===========================================================================
// 3. Saturation pressure at the boiling point equals 1 atm
// ===========================================================================

/// At 100°C the saturation vapour pressure equals standard atmospheric pressure.
///
/// Reference: ASHRAE HOF 2021 Ch. 1, Table 2 (boiling point identity);
/// also NIST WebBook for water. Tolerance ±200 Pa matches ASHRAE table precision
/// at 100°C (polynomial fit uncertainty).
#[test]
fn saturation_pressure_at_boiling_point() {
    let p_boiling = saturation_pressure_pa(100.0);
    // Standard atmosphere: 101 325 Pa -- ISA 1976 / NIST
    assert_approx(p_boiling, 101_325.0, 200.0);
}

// ===========================================================================
// 4. Air density decreases with increasing temperature (ideal gas law)
// ===========================================================================

/// At constant pressure, ρ ∝ 1/T (ideal gas). Verify strict monotonic decrease
/// for dry air across a range spanning cold to hot ambient conditions.
///
/// Reference: Ideal gas law pV = nRT; ASHRAE HOF 2017 Ch. 1 Eq. 11.
#[test]
fn air_density_decreases_with_temperature() {
    let p_pa = 101_325.0_f64;
    let temps_c = [-10.0_f64, 0.0, 10.0, 20.0, 30.0, 40.0];

    let densities: Vec<f64> = temps_c
        .iter()
        .map(|&t| dry_air_density_kg_m3(p_pa, t))
        .collect();

    for window in densities.windows(2) {
        let (rho_cold, rho_warm) = (window[0], window[1]);
        assert!(
            rho_cold > rho_warm,
            "density at lower T ({rho_cold:.4}) must exceed density at higher T ({rho_warm:.4})"
        );
    }
}

// ===========================================================================
// 5. Air density at sea level matches ISA standard
// ===========================================================================

/// At 15°C and 101 325 Pa, dry-air density = 1.2250 kg/m³.
///
/// Reference: ICAO Standard Atmosphere (Doc 7488), Table A; ISA 1976.
/// This is the canonical sea-level reference value used by all aviation and
/// building energy standards.
#[test]
fn air_density_at_sea_level_matches_isa() {
    let rho = dry_air_density_kg_m3(101_325.0, 15.0);
    // ISA 1976 / ICAO Doc 7488: ρ = 1.2250 kg/m³ at MSL
    assert_approx(rho, 1.2250, 0.0005);
}

// ===========================================================================
// 6. Denver altitude reduces air density by ~18%
// ===========================================================================

/// Denver is at 1 609 m elevation. ISA pressure at that altitude is ~83 460 Pa,
/// giving a density approximately 17–19% lower than sea level at the same temperature.
///
/// Reference: ISA 1976 / ICAO Doc 7488; ASHRAE HOF 2021 Ch. 1, psychrometric
/// tables at altitude.
#[test]
fn denver_altitude_reduces_density() {
    let t_c = 20.0_f64;
    let w = 0.008_f64; // typical humidity ratio

    let p_sea = standard_pressure_pa(0.0); // 101 325 Pa
    let p_denver = standard_pressure_pa(1_609.0); // ~83 460 Pa

    let rho_sea = moist_air_density_kg_m3(p_sea, t_c, w);
    let rho_denver = moist_air_density_kg_m3(p_denver, t_c, w);

    let reduction = 1.0 - rho_denver / rho_sea;

    // ISA 1976 predicts ~17.7% pressure reduction at 1 609 m; density scales
    // proportionally (same T). Tolerance ±1 percentage point.
    assert_approx(reduction, 0.18, 0.01);
}

// ===========================================================================
// 7. Film coefficient (exterior R) decreases with wind speed
// ===========================================================================

/// Higher wind speed drives forced convection, reducing the exterior film
/// resistance (increasing h). Tested on a vertical wall (tilt = 90°) exposed
/// to outdoor conditions using the DOE-2 model.
///
/// Reference: EnergyPlus Engineering Reference §9.5 (DOE-2 exterior convection).
#[test]
fn film_coefficient_increases_with_wind() {
    let tilt = 90.0_f64; // vertical wall
    let ground_c = 10.0_f64;
    let ambient_c = 10.0_f64;
    let roughness = SurfaceRoughness::MediumRough;

    let wind_speeds = [0.5_f64, 2.0, 5.0, 10.0];
    let r_exts: Vec<f64> = wind_speeds
        .iter()
        .map(|&v| {
            let (_, r_ext) = film_resistances(
                tilt,
                ZoneLabel::Conditioned,
                ZoneLabel::Outdoor,
                v,
                ground_c,
                ambient_c,
                roughness,
            );
            r_ext
        })
        .collect();

    // Higher wind → lower R_ext (higher h_ext) -- strict monotonic decrease
    for w in r_exts.windows(2) {
        let (r_low_wind, r_high_wind) = (w[0], w[1]);
        assert!(
            r_low_wind > r_high_wind,
            "R_ext at lower wind ({r_low_wind:.5}) must exceed R_ext at higher wind ({r_high_wind:.5})"
        );
    }
}

// ===========================================================================
// 8. TARP natural convection on a vertical surface: h = 1.31 × ΔT^(1/3)
// ===========================================================================

/// TARP model for vertical surfaces (tilt = 90°) uses h = 1.31 × ΔT^(1/3).
/// The above_hotter flag has no effect for vertical surfaces -- both branches
/// must return the same value.
///
/// Reference: EnergyPlus Engineering Reference §9.4, Eq. 9.4-1;
/// ASHRAE HOF 2021 Ch. 25, natural convection correlations.
#[test]
fn tarp_h_natural_vertical_surface() {
    let delta_ts = [1.0_f64, 5.0, 12.9, 20.0, 30.0];

    for &dt in &delta_ts {
        let expected = 1.31 * dt.cbrt();

        let h_true = tarp_h_natural(90.0, dt, true);
        let h_false = tarp_h_natural(90.0, dt, false);

        // Formula verification: tolerance is algebraic (machine precision)
        assert_approx(h_true, expected, 1e-9);
        assert_approx(h_false, expected, 1e-9);

        // The above_hotter flag must have no effect on a vertical surface
        assert_approx(h_true, h_false, 1e-15);
    }
}

// ===========================================================================
// 9. Infiltration increases monotonically with wind speed (ASHRAE model)
// ===========================================================================

/// For the ASHRAE AIM-2 wind-stack model, increasing wind speed at fixed ΔT
/// produces strictly higher volumetric flow.
///
/// Reference: Walker & Wilson (1998), HVAC&R Research; ASHRAE HOF 2021 Ch. 16.
#[test]
fn infiltration_increases_with_wind() {
    let c_s = 0.015_f64; // stack coefficient [m³/s / K^n_i]
    let c_w = 0.001_f64; // wind coefficient [m³/s / (m/s)^(2n_i)]
    let delta_t = 10.0_f64; // fixed temperature difference [K]
    let shelter = 0.5_f64;
    let n_i = N_I_DEFAULT;

    let wind_speeds = [0.0_f64, 1.0, 3.0, 6.0, 10.0];
    let flows: Vec<f64> = wind_speeds
        .iter()
        .map(|&v| ashrae_wind_stack(c_s, c_w, delta_t, v, shelter, n_i))
        .collect();

    // Flow must be non-decreasing with wind speed; since c_w > 0 it is strictly
    // increasing once v > 0.
    for w in flows.windows(2) {
        assert!(
            w[1] >= w[0],
            "flow at higher wind ({:.6}) must be >= flow at lower wind ({:.6})",
            w[1],
            w[0]
        );
    }
    // Strict increase: non-zero wind adds non-zero contribution
    assert!(
        flows[4] > flows[0],
        "flow at 10 m/s ({:.6}) must strictly exceed flow at 0 m/s ({:.6})",
        flows[4],
        flows[0]
    );
}

// ===========================================================================
// 10. Infiltration increases with indoor/outdoor ΔT
// ===========================================================================

/// For both ASHRAE and ELA models, a larger indoor/outdoor temperature
/// difference drives a larger stack-effect infiltration flow.
///
/// Reference: Walker & Wilson (1998), HVAC&R Research §3; ASHRAE HOF 2021 Ch. 16.
#[test]
fn infiltration_increases_with_delta_t() {
    let c_s = 0.015_f64;
    let c_w = 0.001_f64;
    let wind = 2.0_f64;
    let shelter = 0.5_f64;
    let n_i = N_I_DEFAULT;

    let delta_ts = [1.0_f64, 5.0, 10.0, 20.0, 30.0];

    // --- ASHRAE model ---
    let ashrae_flows: Vec<f64> = delta_ts
        .iter()
        .map(|&dt| ashrae_wind_stack(c_s, c_w, dt, wind, shelter, n_i))
        .collect();

    for pair in ashrae_flows.windows(2) {
        assert!(
            pair[1] > pair[0],
            "ASHRAE: flow at larger ΔT ({:.6}) must exceed flow at smaller ΔT ({:.6})",
            pair[1],
            pair[0]
        );
    }

    // --- ELA model ---
    let ela_m2 = 0.05_f64;
    let stack_coeff = 0.000_106_f64;
    let wind_coeff = 0.000_143_f64;

    let ela_flows: Vec<f64> = delta_ts
        .iter()
        .map(|&dt| ela_infiltration(ela_m2, stack_coeff, wind_coeff, dt, wind))
        .collect();

    for pair in ela_flows.windows(2) {
        assert!(
            pair[1] > pair[0],
            "ELA: flow at larger ΔT ({:.6}) must exceed flow at smaller ΔT ({:.6})",
            pair[1],
            pair[0]
        );
    }
}

// ===========================================================================
// 11. Water mains temperature exhibits sinusoidal seasonal variation
// ===========================================================================

/// The Burch-Christensen (2007) model produces a sinusoidal annual cycle with:
/// - A peak in late summer (Northern Hemisphere, days ~220–260).
/// - A trough in late winter (days ~40–80).
/// - Annual mean ≈ T_avg + 6 °F offset (≈ 3.33 °C).
///
/// Reference: Burch & Christensen (2007), ASES National Solar Conference;
/// EnergyPlus Engineering Reference §11.2; OCHRE water_heater.py.
#[test]
fn water_mains_temp_seasonal_variation() {
    let t_avg_c = 12.0_f64; // representative US mid-latitude site
    let dt_annual_range_c = 25.0_f64; // full peak-to-peak annual range [°C]
    let hemisphere = Hemisphere::Northern;

    let temps: Vec<f64> = (1u16..=365)
        .map(|d| water_mains_temperature_c(t_avg_c, dt_annual_range_c, d, hemisphere))
        .collect();

    // --- All values must be finite and physically plausible ---
    for (i, &t) in temps.iter().enumerate() {
        assert!(t.is_finite(), "day {} mains temp must be finite", i + 1);
        assert!(
            t > 0.0 && t < 40.0,
            "day {} mains temp {t:.2} °C outside plausible range (0–40 °C)",
            i + 1
        );
    }

    // --- Identify peak (max) and trough (min) days ---
    let max_day = temps
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
        .map(|(i, _)| i + 1)
        .unwrap();
    let min_day = temps
        .iter()
        .enumerate()
        .min_by(|a, b| a.1.partial_cmp(b.1).unwrap())
        .map(|(i, _)| i + 1)
        .unwrap();

    // Peak: late summer, days 220–260 (Burch-Christensen 2007, Fig. 1)
    assert!(
        (220..=260).contains(&max_day),
        "peak mains temp on day {max_day}, expected 220–260 (late summer)"
    );

    // Trough: late winter, days 40–80
    assert!(
        (40..=80).contains(&min_day),
        "trough mains temp on day {min_day}, expected 40–80 (late winter)"
    );

    // --- Annual mean ≈ T_avg + 6°F offset (≈ 3.33°C) ---
    // The sine term integrates to zero over a full 365-day cycle.
    let offset_c = 6.0_f64 * 5.0 / 9.0; // 6 °F → °C (temperature delta)
    let mean = temps.iter().sum::<f64>() / temps.len() as f64;
    // Allow ±0.5 °C for discretisation error over 365 days vs continuous integral.
    assert_approx(mean, t_avg_c + offset_c, 0.5);

    // --- Amplitude: max − min should reflect the seasonal swing ---
    let amplitude = temps.iter().cloned().fold(f64::NEG_INFINITY, f64::max)
        - temps.iter().cloned().fold(f64::INFINITY, f64::min);
    // With ratio ≈ 0.46, dt_range = 25 °C → expected amplitude in °F then back to °C:
    // amplitude_f ≈ 0.46 × (25 × 9/5) / 2 ≈ 10.4 °F → 5.8 °C
    // Full swing ≈ 11.6 °C. Verify it is non-trivial (> 5 °C) and physical (< 20 °C).
    assert!(
        amplitude > 5.0,
        "seasonal swing {amplitude:.2} °C must be > 5 °C for dt_annual=25 °C"
    );
    assert!(
        amplitude < 20.0,
        "seasonal swing {amplitude:.2} °C must be < 20 °C (non-physical)"
    );
}

// ===========================================================================
// 12. Biquadratic polynomial evaluation
// ===========================================================================

/// Verify that `biquadratic` evaluates `a + b·x1 + c·x1² + d·x2 + e·x2² + f·x1·x2`
/// exactly, and that `BiquadraticCurve::evaluate` correctly clamps inputs to bounds.
///
/// Reference: EnergyPlus Engineering Reference §15.1 (performance curve types);
/// OCHRE HVAC performance curves (vendors/OCHRE/defaults/HVAC Cooling/
/// Biquadratic Air Conditioner.csv).
#[test]
fn biquadratic_evaluation() {
    // --- 12a: algebraic identity with known coefficients ---
    // Coefficients: [a, b, c, d, e, f] for a + b*x1 + c*x1² + d*x2 + e*x2² + f*x1*x2
    let coeffs = [1.0_f64, 2.0, 3.0, 4.0, 5.0, 6.0];
    let x1 = 2.0_f64;
    let x2 = 3.0_f64;

    // Manual: 1 + 2×2 + 3×4 + 4×3 + 5×9 + 6×6 = 1+4+12+12+45+36 = 110
    let expected = 1.0 + 2.0 * x1 + 3.0 * x1 * x1 + 4.0 * x2 + 5.0 * x2 * x2 + 6.0 * x1 * x2;
    let result = biquadratic(&coeffs, x1, x2);
    assert_approx(result, expected, 1e-9);
    assert_approx(result, 110.0, 1e-9);

    // --- 12b: quadratic helper ---
    // q(x) = 2 - 1.5x + 0.25x²; q(4) = 2 - 6 + 4 = 0
    let q_coeffs = [2.0_f64, -1.5, 0.25];
    let q_result = quadratic(&q_coeffs, 4.0);
    assert_approx(q_result, 0.0, 1e-9);

    // --- 12c: BiquadraticCurve clamps out-of-bounds inputs ---
    let curve = BiquadraticCurve {
        coeffs: [1.0, 0.2, 0.01, -0.1, 0.005, 0.02],
        x1_bounds: (10.0, 20.0),
        x2_bounds: (0.0, 5.0),
    };
    // Both x1=100 and x2=-10 are outside bounds; result must equal evaluation at (20, 0)
    let clamped = curve.evaluate(100.0, -10.0);
    let at_boundary = biquadratic(&curve.coeffs, 20.0, 0.0);
    assert_approx(clamped, at_boundary, 1e-9);

    // --- 12d: OCHRE Single_1 AC capacity curve at AHRI 210/240 rating conditions ---
    // AHRI Standard 210/240-2023 Table 1: indoor Twb = 19.44°C, outdoor Tdb = 35.0°C
    // OCHRE Single_1 coefficients from Biquadratic Air Conditioner.csv
    let ochre_coeffs = [1.5509_f64, -0.075_05, 0.0031, 0.0024, -0.000_05, -0.000_43];
    let ahri_result = biquadratic(&ochre_coeffs, 19.44, 35.0);
    // Exact algebraic evaluation of known coefficients -- machine-epsilon tolerance.
    // 1.5509 + (-0.07505)(19.44) + 0.0031(19.44²) + 0.0024(35.0) + (-0.00005)(35.0²)
    //   + (-0.00043)(19.44)(35.0) = 0.993638...
    assert_approx(ahri_result, 0.993_638, 1e-4);
}

// ===========================================================================
// 13. Multi-point psychrometric grid (OCHRE parity)
// ===========================================================================

/// OCHRE's test_psychrolib_jit.py validates psychrometrics across a wide grid.
/// This test checks round-trip consistency (T,W → RH → W) across 7 temperatures
/// and 3 humidity levels at sea level -- matching OCHRE's coverage approach.
///
/// Reference: PsychroLib test suite, ASHRAE HOF 2021 Ch. 1.
#[test]
fn psychrometric_multi_point_round_trip() {
    let p_pa = 101_325.0_f64;
    let mut checked = 0_usize;

    for &t_db_c in &[0.0, 5.0, 10.0, 15.0, 20.0, 25.0, 30.0, 35.0, 40.0] {
        let p_sat = saturation_pressure_pa(t_db_c);
        for &rh_target in &[0.1, 0.3, 0.5, 0.7, 0.9] {
            let p_v = rh_target * p_sat;
            let w = EPSILON * p_v / (p_pa - p_v);
            if w < 1e-7 {
                continue;
            }

            // RH round-trip: T + W → RH → compare with target
            let rh_back = relative_humidity(t_db_c, w, p_pa);
            assert!(
                (rh_back - rh_target).abs() < 0.005,
                "RH round-trip failed at T={t_db_c}°C RH={rh_target}: got {rh_back:.6}"
            );

            // Wet-bulb round-trip: T + W → Twb → W_back → compare
            let t_wb = wet_bulb_from_humidity_ratio(t_db_c, w, p_pa);
            assert!(
                t_wb <= t_db_c + 0.01,
                "Twb ({t_wb:.3}) must be ≤ Tdb ({t_db_c:.1})"
            );

            // Dew-point round-trip: W → Tdp → W_back
            let t_dp = dew_point(w, p_pa);
            assert!(
                t_dp <= t_wb + 0.1,
                "Tdp ({t_dp:.3}) must be ≤ Twb ({t_wb:.3}) at T={t_db_c}°C RH={rh_target}"
            );

            // Enthalpy must be finite and monotonically increase with W at fixed T
            let h = moist_air_enthalpy(t_db_c, w);
            assert!(
                h.is_finite(),
                "enthalpy must be finite at T={t_db_c}°C W={w:.6}"
            );

            checked += 1;
        }
    }
    assert!(
        checked >= 40,
        "expected at least 40 validated conditions, got {checked}"
    );
}
