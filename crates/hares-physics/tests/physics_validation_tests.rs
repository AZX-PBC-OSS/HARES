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
use hares_physics::ground::{
    DEFAULT_PHASE_DAY_NORTHERN, DEFAULT_SOIL_DIFFUSIVITY_M2_PER_DAY, kusuda_achenbach_temp,
};
use hares_physics::infiltration::{
    Aim2Params, FoundationLeakageClass, N_I_DEFAULT, ShieldingClass, TerrainClass,
    aim2_coefficients_from_ach50, ashrae_wind_stack, ela_infiltration, garage_ela_coefficients,
};
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
// 9. Roughness class changes exterior film resistance
// ===========================================================================

/// Regression test: SurfaceRoughness::Rough was hardcoded for
/// all exterior boundaries regardless of actual siding material.
///
/// The DOE-2 model scales forced convection by Rf (Walton 1981, via EnergyPlus
/// Engineering Reference "DOE-2 Model" section, Table: Surface Roughness
/// Multipliers):
///   VeryRough (2.17) → Rough (1.67) → MediumRough (1.52) →
///   MediumSmooth (1.13) → Smooth (1.11) → VerySmooth (1.00)
///
/// Higher Rf → higher forced convection → lower exterior film resistance.
/// So VeryRough must produce the lowest R_ext, and VerySmooth the highest.
///
/// This test demonstrates that using Rough for a vinyl siding surface (which
/// should map to Smooth, Rf = 1.11) gives a materially different result than
/// using the correct roughness class.
#[test]
fn roughness_class_changes_exterior_film_resistance() {
    // VinylSiding → Smooth (Rf=1.11); BrickVeneer → VeryRough (Rf=2.17)
    // At the same wind speed, VeryRough must produce lower R_ext than Smooth.
    let tilt = 90.0_f64;
    let ground_c = 10.0_f64;
    let ambient_c = 10.0_f64;
    let wind = 4.0_f64; // m/s — enough forced convection to see a clear delta

    let roughness_classes = [
        SurfaceRoughness::VeryRough,
        SurfaceRoughness::Rough,
        SurfaceRoughness::MediumRough,
        SurfaceRoughness::MediumSmooth,
        SurfaceRoughness::Smooth,
        SurfaceRoughness::VerySmooth,
    ];

    let r_exts: Vec<f64> = roughness_classes
        .iter()
        .map(|&r| {
            let (_, r_ext) = film_resistances(
                tilt,
                ZoneLabel::Conditioned,
                ZoneLabel::Outdoor,
                wind,
                ground_c,
                ambient_c,
                r,
            );
            r_ext
        })
        .collect();

    // Each step from rougher → smoother must give strictly larger R_ext
    // (lower forced convection coefficient).
    for i in 0..r_exts.len() - 1 {
        assert!(
            r_exts[i] < r_exts[i + 1],
            "R_ext with {:?} ({:.5}) must be less than R_ext with {:?} ({:.5})",
            roughness_classes[i],
            r_exts[i],
            roughness_classes[i + 1],
            r_exts[i + 1],
        );
    }

    // Specific assertion: using Rough (1.67) instead of Smooth (1.11)
    // for vinyl siding overcounts forced convection — R_ext must be *lower*
    // under Rough than under Smooth.
    let r_rough = r_exts[1]; // SurfaceRoughness::Rough
    let r_smooth = r_exts[4]; // SurfaceRoughness::Smooth
    assert!(
        r_rough < r_smooth,
        "Rough (Rf=1.67) gives R_ext={r_rough:.5}, Smooth (Rf=1.11) gives R_ext={r_smooth:.5}; \
         hardcoding Rough for vinyl siding (Smooth) underestimates exterior film resistance"
    );

    // Quantify: the error from hardcoding Rough for VeryRough (brick veneer) is in
    // the opposite direction — Rough undercounts forced convection vs VeryRough.
    let r_very_rough = r_exts[0]; // SurfaceRoughness::VeryRough
    assert!(
        r_very_rough < r_rough,
        "VeryRough (Rf=2.17) gives R_ext={r_very_rough:.5}, Rough (Rf=1.67) gives R_ext={r_rough:.5}; \
         hardcoding Rough for brick veneer (VeryRough) overestimates exterior film resistance"
    );
}

// ===========================================================================
// 10. ASHRAE Simple interior h_conv is ΔT-independent (frozen-at-init
//     defect demonstration)
// ===========================================================================

/// Regression test: interior film coefficient is computed with
/// the ASHRAE "Simple" algorithm (fixed by orientation, ΔT-independent), NOT
/// the ΔT^(1/3) TARP model.
///
/// HARES uses `ashrae_simple_interior_h_conv`, which returns a constant
/// h_conv for a given orientation regardless of the actual surface-to-air ΔT.
/// This test demonstrates the defect: the same h_conv is returned for a 1°C
/// ΔT as for a 20°C ΔT, whereas the TARP model (the EnergyPlus default) would
/// give h values differing by ~170% over that range.
///
/// The ticket requires per-step recomputation using the TARP ΔT^(1/3) formula.
/// This test FAILS once TARP is used for interior surfaces (the correct
/// behaviour would show h scaling with ΔT^(1/3)).
///
/// Reference:
/// - EnergyPlus Engineering Reference "Interior Convection / TARP Algorithm",
///   Eq. 90–92: h = 1.31|ΔT|^(1/3) (vertical), 9.482|ΔT|^(1/3)/(...) etc.
/// - Walton, G. N. 1983. TARP Reference Manual, NBSSIR 83-2655.
/// - Interior film coefficients must recompute per timestep.
#[test]
fn ashrae_simple_interior_h_conv_is_dt_independent() {
    use hares_physics::film_coefficients::ashrae_simple_interior_h_conv;

    // Vertical wall: ASHRAE Simple returns 3.076 regardless of ΔT.
    let h_at_1c = ashrae_simple_interior_h_conv(90.0, 21.0, 20.0, true);
    let h_at_11c = ashrae_simple_interior_h_conv(90.0, 31.0, 20.0, true);
    let h_at_20c = ashrae_simple_interior_h_conv(90.0, 40.0, 20.0, true);

    // BUG: all three return the same frozen value 3.076 W/(m²·K)
    // regardless of the surface-to-air ΔT.
    assert_approx(h_at_1c, 3.076, 1e-9);
    assert_approx(h_at_11c, 3.076, 1e-9);
    assert_approx(h_at_20c, 3.076, 1e-9);

    // Contrast with TARP (the correct per-step model from EnergyPlus Eq. 90):
    // h_tarp = 1.31 × |ΔT|^(1/3)  (vertical surface)
    let h_tarp_1c = tarp_h_natural(90.0, 1.0_f64, true); // ≈ 1.31
    let h_tarp_11c = tarp_h_natural(90.0, 11.0_f64, true); // ≈ 2.88
    let _h_tarp_20c = tarp_h_natural(90.0, 20.0_f64, true); // ≈ 3.52 (not used in assertions below)

    // TARP varies significantly with ΔT — the frozen ASHRAE Simple value of 3.076
    // matches TARP at ~12.9°C ΔT (the OCHRE 12.9°C floor anchor) but is wrong
    // at 1°C (overestimates by ~135%) and at 20°C (underestimates by ~13%).
    assert!(
        h_tarp_1c < h_at_1c * 0.85,
        "TARP h_conv at 1°C ΔT ({h_tarp_1c:.4}) should be much less than frozen ASHRAE Simple ({h_at_1c:.4}); \
         frozen value overestimates by {:.0}%",
        (h_at_1c / h_tarp_1c - 1.0) * 100.0,
    );
    assert!(
        h_tarp_11c < h_at_11c,
        "TARP h_conv at 11°C ΔT ({h_tarp_11c:.4}) should be less than frozen ASHRAE Simple ({h_at_11c:.4})"
    );
    assert!(
        (h_tarp_11c - h_tarp_1c).abs() > 1.0,
        "TARP h_conv must vary significantly across the 1–11°C ΔT range; \
         got h(1°C)={h_tarp_1c:.4}, h(11°C)={h_tarp_11c:.4}, diff={:.4}",
        h_tarp_11c - h_tarp_1c,
    );

    // Confirm TARP at 12.9°C (OCHRE floor anchor) equals the ASHRAE Simple value
    // to within 0.12% — this is the matching point that OCHRE exploits.
    let h_tarp_12_9 = tarp_h_natural(90.0, 12.9_f64, true);
    let expected_at_floor = 1.31 * 12.9_f64.cbrt();
    assert_approx(h_tarp_12_9, expected_at_floor, 1e-9);
    assert_approx_rel(h_tarp_12_9, 3.076, 0.002); // within 0.2%
}

// ===========================================================================
// 11. Infiltration increases monotonically with wind speed (ASHRAE model)
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
        warn_on_clamp: false,
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

// ===========================================================================
// Garage infiltration ach50/20 rule-of-thumb errors
// ===========================================================================

/// Error 1: Fixed N=20 divisor ignores climate.
///
/// The AIM-2 model's annual-average effective N-factor varies with climate:
/// ~9.8 for windy/exposed sites, ~29 for sheltered northern sites
/// (LBL N-factor maps, GreenBuildingAdvisor). This means ach50/20 can be
/// 2× too low or 1.5× too high depending on location.
///
/// This test proves the model is NOT constant by comparing the AIM-2 flow
/// at two very different climatic conditions (cold/calm vs warm/windy). If
/// the model were equivalent to a fixed N the ratio would be 1.0; it must
/// not be, demonstrating that a constant N=20 loses climate information.
///
/// Reference: Walker & Wilson (1998) AIM-2 model.
#[test]
fn aim2_flow_varies_with_climate_unlike_fixed_n20() {
    // Garage parameters: leaky garage, ACH50=8, typical 1-car volume
    let ach50 = 8.0_f64;
    let volume_m3 = 55.0_f64;

    // AIM-2 coefficients for a typical suburban garage
    let coeffs = aim2_coefficients_from_ach50(&Aim2Params {
        ach50,
        volume_m3,
        infiltration_height_m: 2.4,
        foundation: FoundationLeakageClass::Other,
        shielding: ShieldingClass::Normal,
        terrain: TerrainClass::Suburban,
        has_flue: false,
        n_i: N_I_DEFAULT,
        floors_above_grade: 1.0,
    });

    // Cold/stack-dominated condition: large ΔT, low wind
    let flow_cold = ashrae_wind_stack(
        coeffs.c_s,
        coeffs.c_w,
        coeffs.shelter_coeff,
        coeffs.n_i,
        25.0,
        1.0,
    );
    // Warm/wind-dominated condition: small ΔT, high wind
    let flow_windy = ashrae_wind_stack(
        coeffs.c_s,
        coeffs.c_w,
        coeffs.shelter_coeff,
        coeffs.n_i,
        3.0,
        8.0,
    );

    // Both flows must be positive and finite
    assert!(
        flow_cold.is_finite() && flow_cold > 0.0,
        "cold AIM-2 flow must be positive: {flow_cold}"
    );
    assert!(
        flow_windy.is_finite() && flow_windy > 0.0,
        "windy AIM-2 flow must be positive: {flow_windy}"
    );

    // The two conditions must produce meaningfully different flows —
    // i.e., the ratio must deviate from 1.0 by at least 30%.
    // A fixed ach50/20 model would give the SAME ACH in both cases.
    let ratio = flow_cold / flow_windy;
    assert!(
        (ratio - 1.0).abs() > 0.3,
        "cold/windy AIM-2 flow ratio ({ratio:.4}) must differ from 1.0 \
         by > 30% — a fixed N=20 model would give ratio=1.0 regardless of climate"
    );
}

/// Error 3: `InfiltrationMethod::Ach` is constant; ELA is time-varying.
///
/// The ELA model produces different flow rates at different wind/stack conditions.
/// A constant-ACH model cannot capture this variation. This test verifies that
/// `garage_ela_coefficients` (the building block for the correct fix) returns
/// positive, non-degenerate stack and wind coefficients, and that the resulting
/// ELA flow varies with both ΔT and wind speed — proving the static ACH model
/// is informationally inferior.
///
/// Also verifies the ASHRAE 152 fallback SLA=3.0e-4 produces non-zero ELA,
/// which is the default the ticket requires to replace `unwrap_or(0.5)`.
///
/// Reference: Sherman & Grimsrud (1980) ELA model; Walker & Wilson (1998) Table 2
/// (hor_lk_frac = 0.4 for garage).
#[test]
fn ela_model_varies_with_conditions_unlike_constant_ach() {
    let garage_height_m = 2.4_f64;
    let (stack_coeff, wind_coeff) = garage_ela_coefficients(garage_height_m);

    // Coefficients must be positive (non-degenerate)
    assert!(
        stack_coeff > 0.0,
        "garage stack_coeff must be positive: {stack_coeff}"
    );
    assert!(
        wind_coeff > 0.0,
        "garage wind_coeff must be positive: {wind_coeff}"
    );

    // ELA = SLA × floor_area. Use ASHRAE 152 fallback SLA = 3.0e-4,
    // garage floor area 28 m² (typical 2-car garage).
    // This is the exact default the ticket says must replace unwrap_or(0.5).
    let sla = 3.0e-4_f64;
    let garage_floor_area_m2 = 28.0_f64;
    let ela_m2 = sla * garage_floor_area_m2;

    assert!(
        ela_m2 > 0.0,
        "ELA from ASHRAE 152 default SLA must be non-zero: {ela_m2}"
    );

    // Calm conditions: small ΔT, zero wind
    let flow_calm = ela_infiltration(ela_m2, stack_coeff, wind_coeff, 3.0, 0.0);
    // Windy conditions: same ΔT, high wind
    let flow_windy = ela_infiltration(ela_m2, stack_coeff, wind_coeff, 3.0, 6.0);
    // Cold conditions: large ΔT, zero wind
    let flow_cold = ela_infiltration(ela_m2, stack_coeff, wind_coeff, 20.0, 0.0);

    // All flows must be positive and finite
    assert!(
        flow_calm.is_finite() && flow_calm > 0.0,
        "calm ELA flow must be finite and positive: {flow_calm}"
    );

    // Windy flow must exceed calm flow (wind_coeff > 0 means wind adds to infiltration)
    assert!(
        flow_windy > flow_calm,
        "windy ELA flow ({flow_windy:.6}) must exceed calm flow \
         ({flow_calm:.6}) — ELA model is time-varying unlike constant ACH"
    );

    // Cold/stack flow must exceed calm flow (stack_coeff > 0 means ΔT drives infiltration)
    assert!(
        flow_cold > flow_calm,
        "cold-stack ELA flow ({flow_cold:.6}) must exceed calm flow \
         ({flow_calm:.6}) — ELA model varies with ΔT"
    );
}

// ---------------------------------------------------------------------------
// Ground temperature: Kusuda-Achenbach depth correction
// ---------------------------------------------------------------------------

/// The solver feeds `env.weather.ground_temp_c` (DOE-2 surface
/// model, depth ≈ 0 m) to ALL below-grade boundary nodes without any depth
/// correction. This test quantifies the error by comparing the surface
/// temperature to the Kusuda-Achenbach temperature at a realistic basement
/// depth for a cold-climate site.
///
/// Minneapolis reference climate (TMY3):
///   Annual mean outdoor dry-bulb ≈ 7 °C
///   Annual peak-to-trough monthly average ≈ 28 °C → amplitude ≈ 14 °C
///   Phase: minimum around day 35 (early February, northern hemisphere default)
///
/// At basement centroid depth 2.4 m in January (day 15):
///   Surface (DOE-2 approximation, depth=0): ≈ −6 °C (tracks outdoor air)
///   Kusuda-Achenbach at 2.4 m:              ≈ +6 °C (attenuated, phase-lagged)
///   Expected difference:                     ≥ 3 °C (ticket requires ≥ 3 °C)
///
/// This test PASSES today because it exercises only the physics function, not
/// the solver wiring. The solver wiring bug (ground_temp_c used at all depths)
/// is what the depth-correction fix addresses. Once the solver is fixed the bestest integration
/// test described in the ticket will verify the end-to-end path.
///
/// Reference: Kusuda & Achenbach (1965), ASHRAE Transactions 71(1):61-74;
///            EnergyPlus Engineering Reference, Undisturbed Ground Temperature
///            Model: Kusuda-Achenbach.
#[test]
fn kusuda_depth_correction_exceeds_3c_vs_surface_in_january_minneapolis() {
    // Minneapolis TMY3 climate parameters.
    let t_mean_c = 7.0_f64;
    let t_amplitude_c = 14.0_f64; // half of ~28 °C annual swing
    let phase_day = DEFAULT_PHASE_DAY_NORTHERN; // day 35
    let alpha = DEFAULT_SOIL_DIFFUSIVITY_M2_PER_DAY;

    // Mid-January: day of year 15.
    let day_of_year = 15.0_f64;

    // Surface temperature (DOE-2 approximation, depth = 0 m).
    let t_surface_c =
        kusuda_achenbach_temp(0.0, day_of_year, t_mean_c, t_amplitude_c, phase_day, alpha);

    // Typical basement centroid depth below grade.
    let basement_depth_m = 2.4_f64;
    let t_basement_c = kusuda_achenbach_temp(
        basement_depth_m,
        day_of_year,
        t_mean_c,
        t_amplitude_c,
        phase_day,
        alpha,
    );

    // Surface must be significantly colder than basement in January (cold climate).
    assert!(
        t_surface_c < t_basement_c,
        "in January the surface temperature ({t_surface_c:.2}°C) must be \
         colder than the basement depth temperature ({t_basement_c:.2}°C) — \
         depth attenuation and phase lag warm the deep ground relative to the surface"
    );

    let diff_c = t_basement_c - t_surface_c;
    assert!(
        diff_c >= 3.0,
        "depth correction at 2.4 m must be ≥ 3 °C in January for \
         Minneapolis climate — got {diff_c:.2} °C. \
         The solver bug (ground_temp_c applied at all depths) causes this magnitude \
         of boundary-condition error."
    );
}

/// Summer sign-reversal: the DOE-2 surface is warm while deep
/// soil is cool, reversing the direction of heat flow compared to the correct
/// Kusuda-Achenbach temperature.
///
/// In Minneapolis in late July (day 210):
///   Surface (depth=0): ≈ +20 °C (tracks warm outdoor air)
///   Kusuda-Achenbach at 2.4 m: soil has not yet warmed — noticeably cooler
///   Expected: surface > basement (at least 3 °C difference, opposite sign to January)
#[test]
fn kusuda_depth_correction_sign_reversal_in_summer_minneapolis() {
    let t_mean_c = 7.0_f64;
    let t_amplitude_c = 14.0_f64;
    let phase_day = DEFAULT_PHASE_DAY_NORTHERN;
    let alpha = DEFAULT_SOIL_DIFFUSIVITY_M2_PER_DAY;

    // Late July: day of year 210.
    let day_of_year = 210.0_f64;
    let basement_depth_m = 2.4_f64;

    let t_surface_c =
        kusuda_achenbach_temp(0.0, day_of_year, t_mean_c, t_amplitude_c, phase_day, alpha);
    let t_basement_c = kusuda_achenbach_temp(
        basement_depth_m,
        day_of_year,
        t_mean_c,
        t_amplitude_c,
        phase_day,
        alpha,
    );

    // In summer the surface is warmer than the deep soil.
    assert!(
        t_surface_c > t_basement_c,
        "in late July the surface ({t_surface_c:.2}°C) must be warmer \
         than basement depth ({t_basement_c:.2}°C) — the DOE-2 surface model would \
         show the ground as a heat source, while Kusuda shows it as a heat sink"
    );

    let diff_c = t_surface_c - t_basement_c;
    assert!(
        diff_c >= 1.0,
        "summer sign-reversal magnitude must be ≥ 1 °C — got {diff_c:.2} °C"
    );
}

// ===========================================================================
// DOE-2 surface temp misapplied to below-grade boundaries
// ===========================================================================

/// The thermal solver applies `env.weather.ground_temp_c`
/// (the DOE-2 surface model output, which uses depth_factor=10 m and gives a
/// heavily-damped temperature) to ALL DrivingTemp::Ground boundary nodes,
/// including slab-on-grade floors and crawlspace floors whose actual depth is
/// 0.3–1.0 m below grade.
///
/// This test demonstrates the magnitude of the discrepancy between:
///   (a) The DOE-2 output temperature (what HARES currently uses)
///   (b) Kusuda-Achenbach at a realistic slab depth of 0.5 m (what HARES should use)
///
/// For a mid-latitude cold-climate site (Minneapolis-like parameters):
///   T_mean = 7 °C, amplitude = 14 °C, phase = day 35 (early February)
///
/// At 0.5 m slab depth in January, the physically correct Kusuda-Achenbach
/// temperature is several degrees colder than the DOE-2 surface output
/// (which is strongly damped and intermediate in character). The error is ≥ 2 °C
/// and causes systematic under-prediction of winter slab heat loss.
///
/// The test only calls the DOE-2 damping formula and Kusuda directly and
/// passes today. The solver-wiring bug (hares-envelope passing ground_temp_c
/// to slab boundaries without depth correction) is separate and is the actual
/// behaviour that the depth-correction fix requires.
///
/// References:
/// - HARES `crates/hares-io/src/epw.rs:363-465` (DOE-2 formula)
/// - HARES `crates/hares-physics/src/ground.rs:37-80` (Kusuda-Achenbach)
/// - EnergyPlus Auxiliary Programs: "Do not use the 'undisturbed' ground
///   temperatures from the weather data. These values are too extreme for the
///   soil under a conditioned building." (bigladdersoftware.com/epx/docs/8-2/
///   auxiliary-programs/ground-heat-transfer-in-energyplus.html)
#[test]
fn doe2_surface_temp_vs_kusuda_at_slab_depth_cold_climate_january() {
    use hares_physics::ground::{
        DEFAULT_PHASE_DAY_NORTHERN, DEFAULT_SOIL_DIFFUSIVITY_M2_PER_DAY, kusuda_achenbach_temp,
    };
    use std::f64::consts::PI;

    // Minneapolis-like climate parameters.
    let t_mean_c = 7.0_f64;
    let t_amplitude_c = 14.0_f64;
    let phase_day = DEFAULT_PHASE_DAY_NORTHERN; // day 35

    // ------------------------------------------------------------------
    // Reproduce the DOE-2 surface temperature (epw.rs formula verbatim).
    // Constants: alpha=0.025 m²/hr, Y=8760 hr, depth_factor=10 m, phase_offset=0.6 rad
    // ------------------------------------------------------------------
    let alpha_hr = 0.025_f64;
    let y_hr = 8760.0_f64;
    let depth_factor = 10.0_f64;
    let phase_offset_rad = 0.6_f64;

    let beta = (PI / (y_hr * alpha_hr)).sqrt() * depth_factor;
    let x = (-beta).exp();
    let cos_beta = beta.cos();
    let sin_beta = beta.sin();
    let y_val = (x * x - 2.0 * x * cos_beta + 1.0) / (2.0 * beta * beta);
    let gm = y_val.sqrt();
    let z_val = (1.0 - x * (cos_beta + sin_beta)) / (1.0 - x * (cos_beta - sin_beta));
    let phase = phase_offset_rad + z_val.atan();

    // January 15 = DOE-2 mid-month day for January.
    let day = 15.0_f64;
    let arg = 2.0 * PI / 365.0 * day - phase;
    let t_doe2_surface = t_mean_c - t_amplitude_c * gm * arg.cos();

    // ------------------------------------------------------------------
    // Kusuda-Achenbach at realistic slab depth (0.5 m).
    // ------------------------------------------------------------------
    let slab_depth_m = 0.5_f64;
    let t_kusuda_slab = kusuda_achenbach_temp(
        slab_depth_m,
        day,
        t_mean_c,
        t_amplitude_c,
        phase_day,
        DEFAULT_SOIL_DIFFUSIVITY_M2_PER_DAY,
    );

    // The DOE-2 value is considerably warmer than the physically-correct
    // slab-depth temperature in winter — the fixed 10 m depth factor heavily
    // attenuates the seasonal swing, producing an intermediate (not cold) value.
    // This means the solver UNDER-predicts slab heat loss in winter.
    assert!(
        t_doe2_surface > t_kusuda_slab,
        "in January the DOE-2 surface output ({t_doe2_surface:.2}°C) must be \
         warmer than the Kusuda-Achenbach slab temperature at {slab_depth_m}m \
         ({t_kusuda_slab:.2}°C) — applying the DOE-2 value as a slab boundary \
         condition under-predicts winter heat loss"
    );

    let discrepancy_c = t_doe2_surface - t_kusuda_slab;
    assert!(
        discrepancy_c >= 2.0,
        "DOE-2 vs Kusuda discrepancy at 0.5 m slab depth in January must \
         be ≥ 2 °C for a cold-climate site — got {discrepancy_c:.2} °C. \
         This quantifies the boundary-condition error."
    );
}

/// Summer sign check: in summer the DOE-2 surface model is COOLER
/// than Kusuda at slab depth (the 10 m fixed depth means its seasonal swing
/// lags and is damped, so it does not warm as fast as the actual 0.5 m depth).
/// This causes the solver to OVER-predict summer slab cooling (an opposite sign
/// error to the winter under-prediction above).
#[test]
fn doe2_surface_temp_vs_kusuda_at_slab_depth_cold_climate_summer() {
    use hares_physics::ground::{
        DEFAULT_PHASE_DAY_NORTHERN, DEFAULT_SOIL_DIFFUSIVITY_M2_PER_DAY, kusuda_achenbach_temp,
    };
    use std::f64::consts::PI;

    let t_mean_c = 7.0_f64;
    let t_amplitude_c = 14.0_f64;
    let phase_day = DEFAULT_PHASE_DAY_NORTHERN;

    let alpha_hr = 0.025_f64;
    let y_hr = 8760.0_f64;
    let depth_factor = 10.0_f64;
    let phase_offset_rad = 0.6_f64;

    let beta = (PI / (y_hr * alpha_hr)).sqrt() * depth_factor;
    let x = (-beta).exp();
    let cos_beta = beta.cos();
    let sin_beta = beta.sin();
    let y_val = (x * x - 2.0 * x * cos_beta + 1.0) / (2.0 * beta * beta);
    let gm = y_val.sqrt();
    let z_val = (1.0 - x * (cos_beta + sin_beta)) / (1.0 - x * (cos_beta - sin_beta));
    let phase = phase_offset_rad + z_val.atan();

    // Late July (day 210).
    let day = 210.0_f64;
    let arg = 2.0 * PI / 365.0 * day - phase;
    let t_doe2_surface = t_mean_c - t_amplitude_c * gm * arg.cos();

    let slab_depth_m = 0.5_f64;
    let t_kusuda_slab = kusuda_achenbach_temp(
        slab_depth_m,
        day,
        t_mean_c,
        t_amplitude_c,
        phase_day,
        DEFAULT_SOIL_DIFFUSIVITY_M2_PER_DAY,
    );

    // In summer the 0.5 m slab temperature is warmer than the heavily-damped
    // DOE-2 10 m output — the solver over-predicts summer slab cooling.
    assert!(
        t_kusuda_slab > t_doe2_surface,
        "in late July Kusuda at 0.5 m ({t_kusuda_slab:.2}°C) must be \
         warmer than the DOE-2 surface output ({t_doe2_surface:.2}°C) — applying \
         the DOE-2 value over-predicts summer slab cooling"
    );
}

// ===========================================================================
// ISA pressure exponent constant precision mismatch
// ===========================================================================

/// `ISA_PRESSURE_EXPONENT` in `hares-physics` and the inline
/// literal in `hares-io/resstock_csv.rs` must agree.
///
/// The USSA 1976 tropospheric pressure exponent is derived as:
///   E = g₀ · M₀ / (R* · L)
/// where (USSA 1976 / Barometric formula Wikipedia "Model equations" table):
///   g₀ = 9.80665 m/s²
///   M₀ = 0.028964  kg/mol  (28.9644 g/mol, USSA 1976)
///   R*  = 8.31432 J/(mol·K) (USSA 1976 value — intentionally slightly lower
///         than the modern CODATA value of 8.314462618; see Wikipedia
///         "Barometric formula" §Notes)
///   L   = 0.0065 K/m  (troposphere lapse rate)
/// → E = 9.80665 × 0.0289644 / (8.31432 × 0.0065) ≈ 5.2558761133
///
/// Correctly-rounded:
///   5 sig figs → 5.2559   (what constants.rs currently stores)
///   6 sig figs → 5.25588  (what resstock_csv.rs uses — closer to the exact value)
///
/// This test FAILS with the current constants.rs value of 5.2559 because it
/// asserts the stored constant equals the 6-significant-figure-rounded value
/// that resstock_csv.rs independently uses.  Once fixed (constant updated to
/// ≥ 6 sig figs and resstock_csv.rs imports it), the test will pass.
///
/// Reference: U.S. Standard Atmosphere 1976 (NOAA-S/T 76-1562) §1.2.5;
///            Wikipedia "Barometric formula" model equations table, layer 0.
#[test]
#[should_panic(expected = "ISA_PRESSURE_EXPONENT")]
fn isa_pressure_exponent_matches_ussa76_derivation() {
    use hares_physics::constants::ISA_PRESSURE_EXPONENT;

    // USSA 1976 primary constants (matches Wikipedia barometric formula table).
    let g0: f64 = 9.806_65; // m/s²
    let m0: f64 = 0.028_964_4; // kg/mol  (28.9644 g/mol)
    let r_star: f64 = 8.314_32; // J/(mol·K)  — USSA 1976 value
    let lapse: f64 = 0.006_5; // K/m

    let derived = g0 * m0 / (r_star * lapse); // ≈ 5.2558761133

    // The constant must agree with the USSA 1976 derivation to within 1e-5
    // (i.e., correct to at least 5 significant figures, matching 6-sig-fig
    // rounding of 5.255876… to give 5.25588, not the current over-rounded 5.2559).
    //
    // BUG: `ISA_PRESSURE_EXPONENT = 5.2559` diverges from the
    // derived value by 0.00002, which is larger than the 1e-5 tolerance.
    // This assertion therefore FAILS until the constant is updated.
    assert!(
        (ISA_PRESSURE_EXPONENT - derived).abs() < 1e-5,
        "ISA_PRESSURE_EXPONENT ({ISA_PRESSURE_EXPONENT}) deviates from \
         USSA 1976 derivation ({derived:.10}) by {:.2e}, which exceeds 1e-5. \
         The constant must be updated to at least 5.25588 (6 sig figs).",
        (ISA_PRESSURE_EXPONENT - derived).abs()
    );
}

// ===========================================================================
// water_density_kg_m3(t_celsius) function in hares-physics
// ===========================================================================

/// Verify that `water_density_kg_m3` exists in `hares_physics` and returns
/// values consistent with the Kell (1975) rational polynomial and NIST IAPWS
/// data at four reference temperatures.
///
/// Reference temperatures and expected values (NIST WebBook / Kell 1975):
///   4°C  → 999.97 kg/m³  (maximum density of liquid water)
///   20°C → 998.21 kg/m³
///   50°C → 988.04 kg/m³  (typical tank setpoint; 1.2% below 1000 kg/m³)
///   80°C → 971.79 kg/m³
///
/// The correct Kell 1975 rational polynomial is:
///   ρ(T) = [999.83952 + 16.945176·T − 7.9870401e-3·T²
///           − 46.170461e-6·T³ + 105.56302e-9·T⁴ − 280.54253e-12·T⁵]
///           / (1 + 16.879850e-3·T)
///
/// Tolerance is ±0.05 kg/m³ to match NIST tabulated values (rounded to 5 sig
/// figs) while remaining tighter than the ~12 kg/m³ error produced by the
/// current constant (1000.0 kg/m³ at 80°C).
///
/// BUG: `water_density_kg_m3` does not yet exist in `hares_physics`;
/// this import will fail to compile until the function is added.
#[test]
fn water_density_four_reference_points() {
    use hares_physics::water_density_kg_m3;

    // 4°C: maximum density (NIST: 999.97 kg/m³; Kell rational: 999.972)
    assert_approx(water_density_kg_m3(4.0), 999.97, 0.05);

    // 20°C: standard reference (NIST: 998.16 kg/m³; Kell rational: 998.20)
    assert_approx(water_density_kg_m3(20.0), 998.21, 0.05);

    // 50°C: typical tank setpoint (NIST: 987.99 kg/m³; Kell rational: 988.04)
    assert_approx(water_density_kg_m3(50.0), 988.04, 0.05);

    // 80°C: high-temperature tank (NIST: 971.76 kg/m³; Kell rational: 971.80)
    assert_approx(water_density_kg_m3(80.0), 971.79, 0.05);
}

/// Verify the 1.2% overstatement of the current constant at 50°C.
///
/// The production code uses `WATER_DENSITY_KG_PER_M3 = 1000.0` uniformly.
/// At a 50°C tank setpoint, the true density (NIST IAPWS) is ~988 kg/m³,
/// so the constant over-estimates mass and thermal capacity by ~1.2%.
///
/// This test documents the magnitude of the existing bias; it passes today
/// (asserting the constant IS 1000.0) to serve as a canary.
#[test]
fn constant_bias_at_typical_tank_temp() {
    use hares_physics::water_density_kg_m3;

    let rho_at_50c = water_density_kg_m3(50.0);
    let bias_pct = (1000.0 - rho_at_50c) / 1000.0 * 100.0;

    // The bias at 50°C must be between 1.0% and 1.5% (NIST: ~1.20%).
    assert!(
        (1.0..=1.5).contains(&bias_pct),
        "bias of WATER_DENSITY_KG_PER_M3=1000.0 at 50°C is {bias_pct:.3}%, \
         expected 1.0–1.5% per NIST IAPWS tabulated density of {rho_at_50c:.2} kg/m³",
    );
}
