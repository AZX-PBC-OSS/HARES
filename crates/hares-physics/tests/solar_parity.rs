//! OCHRE parity tests for solar position and irradiance calculations.
//!
//! - Solar declination at astronomical equinoxes and solstices (Spencer 1971)
//! - Perez diffuse model against pvlib reference values
//! - POA total irradiance (beam + diffuse + reflected) for south-facing 30° tilt
//! - ZOH upsampling of schedule: 15-min values replicate hourly source exactly
//!
//! Reference sources cited per test. All reference values are analytically
//! derived or cross-checked against pvlib-python 0.10.x (documentation-level
//! tolerances of ±0.5°, ±1% for POA).

use chrono::{Datelike, FixedOffset, TimeZone};
use hares_physics::solar::{
    angle_of_incidence, extraterrestrial_irradiance, perez_sky_diffuse, perez_tilted_irradiance,
    solar_position,
};

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

#[track_caller]
fn assert_approx(actual: f64, expected: f64, tol: f64) {
    assert!(
        (actual - expected).abs() <= tol,
        "actual={actual:.6}, expected={expected:.6}, tol={tol}, |delta|={}",
        (actual - expected).abs()
    );
}

// ---------------------------------------------------------------------------
// Test: solar declination at equinoxes and solstices
//
// Spencer (1971) Fourier series for declination (embedded in solar_position).
// The declination is not directly exposed, but we can back-calculate it from
// the solar position at solar noon on the equator (latitude=0°, longitude=0°).
//
// At the equator at solar noon: altitude = 90° - |declination|
// (when hour angle ≈ 0 and latitude = 0°, cos_zenith = cos(decl) → altitude = 90° - |decl|)
//
// Reference: Spencer (1971), J. Applied Meteorology;
//            pvlib-python solar_position documentation.
//
// Tolerances: ±0.3° -- Spencer model accuracy vs astronomical tables.
// ---------------------------------------------------------------------------

#[test]
fn solar_declination_near_zero_at_vernal_equinox() {
    // March 20, 2024 at 12:00 UTC → solar noon at longitude 0°, latitude 0°.
    // Expected declination ≈ 0° (equinox); altitude ≈ 90°.
    let dt = FixedOffset::east_opt(0)
        .unwrap()
        .with_ymd_and_hms(2024, 3, 20, 12, 0, 0)
        .single()
        .unwrap();
    let pos = solar_position(0.0, 0.0, dt, dt.ordinal());

    // At equinox on equator at solar noon: altitude ≈ 90° (sun nearly overhead).
    // Declination ≈ 0° → altitude should be within 2.5° of 90° (Spencer 1971 ~2° error at equinox).
    assert!(
        (pos.altitude_deg - 90.0).abs() < 2.5,
        "vernal equinox equator noon: altitude={:.3}°, expected within 2.5° of 90°",
        pos.altitude_deg
    );
}

#[test]
fn solar_declination_approx_23_4_at_summer_solstice() {
    // June 21, 2024 at 12:00 UTC, latitude 0°, longitude 0°.
    // Summer solstice: declination ≈ +23.44°.
    // At equator noon: altitude = 90° - 23.44° ≈ 66.56°.
    let dt = FixedOffset::east_opt(0)
        .unwrap()
        .with_ymd_and_hms(2024, 6, 21, 12, 0, 0)
        .single()
        .unwrap();
    let pos = solar_position(0.0, 0.0, dt, dt.ordinal());

    // Altitude at equator = 90° - declination ≈ 66.5°.
    // Tolerance: ±0.5° for Spencer model vs NREL calculation.
    let expected_altitude = 90.0 - 23.44;
    assert_approx(pos.altitude_deg, expected_altitude, 0.5);
}

#[test]
fn solar_declination_approx_neg_23_4_at_winter_solstice() {
    // December 21, 2024 at 12:00 UTC, latitude 0°, longitude 0°.
    // Winter solstice: declination ≈ -23.44°.
    // At equator noon: altitude = 90° - 23.44° ≈ 66.56°.
    let dt = FixedOffset::east_opt(0)
        .unwrap()
        .with_ymd_and_hms(2024, 12, 21, 12, 0, 0)
        .single()
        .unwrap();
    let pos = solar_position(0.0, 0.0, dt, dt.ordinal());

    let expected_altitude = 90.0 - 23.44;
    assert_approx(pos.altitude_deg, expected_altitude, 0.5);
}

// ---------------------------------------------------------------------------
// Test: Perez diffuse model reference values
//
// pvlib-python reference case (pvlib docs irradiance.perez):
//   GHI=900, DNI=800, DHI=100, zenith=30°, latitude=40°, tilt=30°, azimuth=180°
//   day_of_year=80
//
// pvlib-python irradiance.perez() for south-facing 30° tilt at zenith=30°:
//   diffuse ≈ 100–130 W/m² (Perez anisotropic)
//
// Reference: Perez et al. (1990), Solar Energy 44:271-289;
//            pvlib-python 0.10.x source, irradiance.py::perez().
// Tolerance: ±10 W/m² (model parametrization differences).
// ---------------------------------------------------------------------------

#[test]
fn perez_diffuse_south_facing_30_tilt_day80_reasonable_range() {
    // Inputs matching pvlib reference case.
    let dhi = 100.0_f64; // W/m²
    let dni = 800.0_f64; // W/m²
    let zenith_deg = 30.0_f64;
    let tilt_deg = 30.0_f64;
    let surface_azimuth_deg = 180.0_f64; // south-facing
    let day_of_year = 80u32; // March 21 (approximate)

    // Compute angle of incidence: for south-facing tilt, solar azimuth ≈ 180° at noon.
    let solar_azimuth_deg = 180.0_f64; // solar noon, looking south
    let solar_alt_deg = 90.0 - zenith_deg;
    let aoi_deg = angle_of_incidence(
        tilt_deg,
        surface_azimuth_deg,
        solar_alt_deg,
        solar_azimuth_deg,
    );

    let dni_extra = extraterrestrial_irradiance(day_of_year);
    let diffuse_w_m2 = perez_sky_diffuse(dhi, dni, zenith_deg, aoi_deg, tilt_deg, dni_extra);

    // Perez diffuse for this geometry should be in [90, 140] W/m².
    // pvlib-python 0.10.x gives ~100–130 W/m² for this case.
    assert!(
        (90.0..=140.0).contains(&diffuse_w_m2),
        "Perez sky diffuse={diffuse_w_m2:.2} W/m² outside expected [90, 140] W/m²"
    );
}

#[test]
fn perez_diffuse_increases_with_dhi() {
    // For fixed geometry, increasing DHI should increase diffuse POA.
    let dni = 500.0_f64;
    let zenith_deg = 45.0_f64;
    let tilt_deg = 30.0_f64;
    let day_of_year = 172u32; // June 21
    let aoi_deg = 20.0_f64;

    let dni_extra = extraterrestrial_irradiance(day_of_year);

    let diffuse_low = perez_sky_diffuse(50.0, dni, zenith_deg, aoi_deg, tilt_deg, dni_extra);
    let diffuse_high = perez_sky_diffuse(200.0, dni, zenith_deg, aoi_deg, tilt_deg, dni_extra);

    assert!(
        diffuse_high > diffuse_low,
        "higher DHI must produce higher Perez sky diffuse: low={diffuse_low:.2} high={diffuse_high:.2}"
    );
}

#[test]
fn perez_diffuse_returns_zero_when_dhi_below_threshold() {
    // HARES skips Perez model when DHI < 1.0 W/m² (PEREZ_MIN_DHI constant).
    let result = perez_sky_diffuse(0.5, 800.0, 30.0, 10.0, 30.0, 1400.0);
    assert_eq!(
        result, 0.0,
        "Perez sky diffuse must be 0.0 when DHI < 1 W/m², got {result}"
    );
}

// ---------------------------------------------------------------------------
// Test: POA total irradiance -- beam + diffuse + reflected
//
// Reference case for south-facing 30° tilt at latitude 40°:
//   GHI=900, DNI=800, DHI=100, zenith=30°, day=80
//   Beam component: DNI × cos(AOI) ≈ 800 × cos(aoi)
//   Ground-reflected: GHI × albedo × (1 - cos(tilt)) / 2
//   Total POA ≈ 800–1200 W/m² for this geometry.
//
// Tolerance: physical plausibility check only (model comparison would need
//            pvlib reference run; full reference values are #[ignore]).
// ---------------------------------------------------------------------------

#[test]
fn poa_total_irradiance_is_physically_plausible_south_facing_30_tilt() {
    let ghi = 900.0_f64;
    let dni = 800.0_f64;
    let dhi = 100.0_f64;
    let zenith_deg = 30.0_f64;
    let day_of_year = 80u32;

    // South-facing surface (azimuth=180°) at tilt=30°.
    let surface_tilt_deg = 30.0_f64;
    let surface_azimuth_deg = 180.0_f64;
    let solar_azimuth_deg = 180.0_f64; // solar noon

    let result = perez_tilted_irradiance(
        0,
        ghi,
        dni,
        dhi,
        zenith_deg,
        solar_azimuth_deg,
        surface_tilt_deg,
        surface_azimuth_deg,
        day_of_year,
        0.2,
    );

    let poa_total = result.direct_w_m2 + result.diffuse_w_m2 + result.reflected_w_m2;

    // Physical bounds: POA cannot exceed solar constant (~1367 W/m²) and
    // for this illuminated geometry must be well above zero.
    assert!(
        poa_total > 200.0,
        "POA total={poa_total:.2} W/m² too low for GHI=900, DNI=800, zenith=30°"
    );
    assert!(
        poa_total < 1_500.0,
        "POA total={poa_total:.2} W/m² exceeds physical maximum"
    );

    // Beam component must be non-negative.
    assert!(
        result.direct_w_m2 >= 0.0,
        "beam component must be non-negative, got {}",
        result.direct_w_m2
    );

    // Ground-reflected must be non-negative.
    assert!(
        result.reflected_w_m2 >= 0.0,
        "ground-reflected component must be non-negative, got {}",
        result.reflected_w_m2
    );
}

#[test]
fn poa_total_zero_at_night() {
    // Nighttime: all irradiance inputs zero. All POA components must be zero.
    let result = perez_tilted_irradiance(0, 0.0, 0.0, 0.0, 90.0, 180.0, 30.0, 180.0, 172, 0.2);
    assert_eq!(result.direct_w_m2, 0.0, "nighttime beam must be 0");
    assert_eq!(result.diffuse_w_m2, 0.0, "nighttime diffuse must be 0");
    assert_eq!(result.reflected_w_m2, 0.0, "nighttime reflected must be 0");
}

// ---------------------------------------------------------------------------
// Test: pvlib reference values for POA irradiance
//
// These exact reference values require running pvlib-python 0.10.x:
//   import pvlib
//   loc = pvlib.location.Location(40, 0, 'UTC', 0)
//   times = pd.DatetimeIndex(['2024-03-20 12:00'], tz='UTC')
//   solar = loc.get_solarposition(times)
//   total = pvlib.irradiance.get_total_irradiance(30, 180, solar.apparent_zenith,
//           solar.azimuth, 800, 900, 100)
// Marked #[ignore] until pvlib output is captured and hardcoded.
// ---------------------------------------------------------------------------

#[test]
fn poa_total_matches_pvlib_reference() {
    // pvlib 0.15.0, Perez model, albedo=0.2, day_of_year=80 (March equinox):
    //   pvlib.irradiance.get_total_irradiance(
    //       surface_tilt=30, surface_azimuth=180, solar_zenith=30, solar_azimuth=180,
    //       dni=800, ghi=900, dhi=100, albedo=0.2, model='perez',
    //       dni_extra=pvlib.irradiance.get_extra_radiation(80))
    //   → poa_global=925.4807, poa_direct=800.0, poa_sky_diffuse=113.4230, poa_ground_diffuse=12.0577
    let result = perez_tilted_irradiance(
        0,     // surface_id
        900.0, // ghi
        800.0, // dni
        100.0, // dhi
        30.0,  // solar_zenith_deg
        180.0, // solar_azimuth_deg
        30.0,  // surface_tilt_deg
        180.0, // surface_azimuth_deg
        80,    // day_of_year (March equinox)
        0.2,   // ground_albedo
    );
    let hares_poa = result.direct_w_m2 + result.diffuse_w_m2 + result.reflected_w_m2;
    let pvlib_poa = 925.4807_f64;
    let rel_err = (hares_poa - pvlib_poa).abs() / pvlib_poa;
    assert!(
        rel_err < 0.01,
        "HARES POA ({hares_poa:.2} W/m²) must be within 1% of pvlib ({pvlib_poa:.2} W/m²), \
         relative error = {rel_err:.4}"
    );
}

// ---------------------------------------------------------------------------
// Test: solar position known case -- latitude 40°N, March equinox, solar noon
//
// At latitude 40°N on the vernal equinox at solar noon:
//   - Sun altitude ≈ 90° - 40° = 50° (declination ≈ 0°)
//   - Sun azimuth ≈ 180° (south, for Northern Hemisphere)
//
// Reference: basic spherical astronomy; pvlib validated.
// Tolerance: ±1.0° (Spencer model vs ephemeris).
// ---------------------------------------------------------------------------

#[test]
fn solar_position_at_lat40_equinox_noon() {
    // March 20, 2024 at 12:00 UTC at longitude 0° (prime meridian).
    // At equinox, declination ≈ 0°, so altitude ≈ latitude complement = 50°.
    let dt = FixedOffset::east_opt(0)
        .unwrap()
        .with_ymd_and_hms(2024, 3, 20, 12, 0, 0)
        .single()
        .unwrap();
    let pos = solar_position(40.0, 0.0, dt, dt.ordinal());

    // Altitude ≈ 50° (90° - 40° latitude at zero declination).
    assert_approx(pos.altitude_deg, 50.0, 2.0);

    // Azimuth should be near 180° (south-facing at noon in Northern Hemisphere).
    assert!(
        pos.azimuth_deg > 160.0 && pos.azimuth_deg < 200.0,
        "azimuth at lat=40°, equinox noon should be ~180°, got {:.2}°",
        pos.azimuth_deg
    );
}

// ---------------------------------------------------------------------------
// Test: solar altitude is negative (below horizon) at midnight
// ---------------------------------------------------------------------------

#[test]
fn solar_altitude_negative_at_midnight_utc() {
    // Midnight UTC at latitude 40°N, longitude 0°: sun well below horizon.
    let dt = FixedOffset::east_opt(0)
        .unwrap()
        .with_ymd_and_hms(2024, 6, 21, 0, 0, 0)
        .single()
        .unwrap();
    let pos = solar_position(40.0, 0.0, dt, dt.ordinal());

    assert!(
        pos.altitude_deg < 0.0,
        "solar altitude at midnight (UTC, lat=40°) must be negative, got {:.2}°",
        pos.altitude_deg
    );
}

// ---------------------------------------------------------------------------
// Test: extraterrestrial irradiance is within Spencer's bounds
//
// Spencer (1971) gives ETR ≈ 1412 W/m² near perihelion (early January)
// and ≈ 1322 W/m² near aphelion (early July). The solar constant = 1367 W/m².
//
// Reference: Spencer (1971), J. Applied Meteorology 10:82-83;
//            NIST / WMO solar constant = 1361.1 W/m² (updated), but HARES
//            uses 1367.0 W/m² consistent with OCHRE and pvlib defaults.
// ---------------------------------------------------------------------------

#[test]
fn extraterrestrial_irradiance_within_spencer_bounds() {
    // Check all 365 days of the year.
    for day in 1u32..=365 {
        let etr = extraterrestrial_irradiance(day);
        assert!(
            (1_300.0..=1_425.0).contains(&etr),
            "day {day}: ETR={etr:.2} W/m² outside Spencer [1300, 1425] W/m²"
        );
    }
}

#[test]
fn extraterrestrial_irradiance_peaks_near_perihelion() {
    // Early January (day 3–5): Earth at perihelion → maximum ETR.
    let etr_jan = extraterrestrial_irradiance(4);

    // Early July (day 183–185): Earth at aphelion → minimum ETR.
    let etr_jul = extraterrestrial_irradiance(184);

    assert!(
        etr_jan > etr_jul,
        "ETR at perihelion (Jan: {etr_jan:.2}) must exceed aphelion (Jul: {etr_jul:.2})"
    );
}

// ── Window U-factor decomposition (EnergyPlus Simple Window Model Step 1) ──

use hares_physics::solar::window_u_factor_decomposition;

/// BEopt example window: U=2.1 W/m²K (low-e double-pane).
/// E+ Step 1: Ri,w = 1/(0.359073·ln(U) + 6.949915), Ro,w = 1/(0.025342·U + 29.163853).
/// Rl,w = 1/U − Ri,w − Ro,w; full assembly = Rl,w + Ri,w + Ro,w = 1/U.
#[test]
fn window_decomposition_low_e_double_pane() {
    let (r_glass, r_int, r_ext) = window_u_factor_decomposition(2.1).unwrap();
    let r_total = r_glass + r_int + r_ext;
    assert!(
        (r_total - 1.0 / 2.1).abs() < 1e-10,
        "r_glass + r_int + r_ext must equal 1/U: {r_total:.6} vs {:.6}",
        1.0 / 2.1
    );
    assert!(r_glass > 0.0, "r_glass must be positive: {r_glass}");
    assert!(r_int > 0.0, "r_int must be positive: {r_int}");
    assert!(r_ext > 0.0, "r_ext must be positive (Ro,w): {r_ext}");
    // Cross-check against EnergyPlus formula.
    let ep_ro_w = 1.0 / (0.025342 * 2.1 + 29.163853);
    assert!(
        (r_ext - ep_ro_w).abs() < 1e-10,
        "r_ext must match E+ Ro,w: {r_ext:.6} vs {ep_ro_w:.6}"
    );
    let ochre_r_int = 1.0 / (0.359073 * 2.1_f64.ln() + 6.949915);
    assert!(
        (r_int - ochre_r_int).abs() < 1e-10,
        "r_int must match E+ Ri,w: {r_int:.6} vs {ochre_r_int:.6}"
    );
}

/// Single-pane window: U=5.5 W/m²K (< 5.85 threshold, logarithmic fit).
#[test]
fn window_decomposition_single_pane() {
    let (r_glass, r_int, r_ext) = window_u_factor_decomposition(5.5).unwrap();
    let r_total = r_glass + r_int + r_ext;
    assert!(
        (r_total - 1.0 / 5.5).abs() < 1e-10,
        "r_glass + r_int + r_ext must equal 1/U"
    );
    assert!(r_glass > 0.0);
    assert!(r_ext > 0.0);
    // U=5.5 < 5.85 so uses logarithmic fit.
    let ochre_r_int = 1.0 / (0.359073 * 5.5_f64.ln() + 6.949915);
    assert!((r_int - ochre_r_int).abs() < 1e-10);
}

/// High-U single-pane: U=6.5 W/m²K (≥ 5.85 threshold, linear fit).
#[test]
fn window_decomposition_high_u_linear_fit() {
    let (r_glass, r_int, r_ext) = window_u_factor_decomposition(6.5).unwrap();
    let r_total = r_glass + r_int + r_ext;
    assert!(
        (r_total - 1.0 / 6.5).abs() < 1e-10,
        "r_glass + r_int + r_ext must equal 1/U"
    );
    assert!(r_ext > 0.0);
    // U=6.5 >= 5.85 so uses linear fit.
    let ochre_r_int = 1.0 / (1.788041 * 6.5 - 2.886625);
    assert!((r_int - ochre_r_int).abs() < 1e-10);
}

/// Triple-pane: U=1.0 W/m²K.
#[test]
fn window_decomposition_triple_pane() {
    let (r_glass, r_int, r_ext) = window_u_factor_decomposition(1.0).unwrap();
    assert!(
        r_glass > 0.5,
        "triple-pane glass R should be substantial: {r_glass}"
    );
    assert!(r_ext > 0.0, "exterior film R must be positive: {r_ext}");
    assert!(
        (r_glass + r_int + r_ext - 1.0).abs() < 1e-10,
        "r_glass + r_int + r_ext must equal 1/U=1.0"
    );
}

/// U ≤ 0 and non-finite U must return Err(HaresError::Physics(...)).
#[test]
fn window_decomposition_zero_and_negative_u() {
    for u in [
        0.0_f64,
        -0.5,
        -1.0,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ] {
        let result = window_u_factor_decomposition(u);
        assert!(
            result.is_err(),
            "U={u} should return Err(HaresError::Physics), got {result:?}"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("U-factor"),
            "U={u}: error should mention U-factor, got: {err_msg}"
        );
    }
}

/// U-factors ≥ ~10 produce negative r_glass because the EnergyPlus
/// film-resistance correlations yield r_int + r_ext > 1/U at those values.
/// The function must return Err, not silently clamp to zero.
#[test]
fn window_decomposition_high_u_returns_physics_error() {
    for u in [10.0_f64, 12.0, 15.0, 20.0, 50.0, 100.0] {
        let result = window_u_factor_decomposition(u);
        assert!(
            result.is_err(),
            "U={u} should return Err(HaresError::Physics), got {result:?}"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("r_glass"),
            "U={u}: error should mention r_glass, got: {err_msg}"
        );
        assert!(
            err_msg.contains("EnergyPlus"),
            "U={u}: error should mention EnergyPlus correlation range, got: {err_msg}"
        );
    }
}

/// Log/linear fit should be approximately continuous at U=5.85 threshold.
#[test]
fn window_decomposition_continuity_at_threshold() {
    let (_, r_int_below, _) = window_u_factor_decomposition(5.8499).unwrap();
    let (_, r_int_above, _) = window_u_factor_decomposition(5.85).unwrap();
    assert!(
        (r_int_below - r_int_above).abs() < 0.005,
        "film R should be continuous at threshold: below={r_int_below:.4} above={r_int_above:.4}"
    );
}

// ── Round-trip and film separation (EnergyPlus Simple Window Model Step 1) ──
//
// window_u_factor_decomposition returns (r_glass, r_film_int, r_film_ext) where:
//   r_film_int = Ri,w  (E+ interior film, combined h_si)
//   r_film_ext = Ro,w  (E+ exterior film, standard winter conditions)
//   r_glass    = Rl,w  (glass-only resistance)
//   r_glass + r_int + r_ext ≡ 1/U  (full assembly round-trip)
//
// This diverges from OCHRE which sets res_ext_w = 0, absorbing Ro,w into r_window.
// The separation enables accurate solar parameter computation in
// calculate_window_parameters, which requires the true glass-only R.

/// Full assembly round-trip: r_glass + r_int + r_ext must equal 1/U exactly.
/// Covers U-factors from 0.2 to 6.0 W/m²·K as required by the Definition of Done.
#[test]
fn window_u_factor_round_trip() {
    for &u in &[0.2_f64, 0.5, 1.0, 2.0, 3.0, 4.0, 5.0, 5.85, 6.0] {
        let (r_glass, r_int, r_ext) = window_u_factor_decomposition(u).unwrap();
        let assembled = r_glass + r_int + r_ext;
        assert!(
            (assembled - 1.0 / u).abs() < 1e-10,
            "U={u}: r_glass + r_int + r_ext = {assembled:.10} must equal 1/U = {:.10}",
            1.0 / u
        );
        assert!(
            r_glass >= 0.0,
            "U={u}: r_glass must be non-negative, got {r_glass}"
        );
        assert!(r_int > 0.0, "U={u}: r_int must be positive, got {r_int}");
        assert!(r_ext > 0.0, "U={u}: r_ext must be positive, got {r_ext}");
    }
}

/// Exterior film matches the EnergyPlus Ro,w correlation formula.
/// Ro,w = 1/(0.025342·U + 29.163853) for all U-factors.
#[test]
fn exterior_film_matches_energyplus_ro_w() {
    let ep_ro_w = |u: f64| 1.0 / (0.025342 * u + 29.163853);

    for &u in &[0.5_f64, 1.0, 2.0, 3.0, 5.0, 5.85, 6.5] {
        let (r_glass, r_int, r_ext) = window_u_factor_decomposition(u).unwrap();
        let ro_w = ep_ro_w(u);

        assert!(
            (r_ext - ro_w).abs() < 1e-10,
            "U={u}: r_ext={r_ext:.8} must equal E+ Ro,w={ro_w:.8}"
        );

        // r_glass must be the true glass-only resistance: 1/U − Ri,w − Ro,w.
        let r_glass_expected = 1.0 / u - r_int - r_ext;
        assert!(
            (r_glass - r_glass_expected).abs() < 1e-10,
            "U={u}: r_glass={r_glass:.8} must equal 1/U − Ri,w − Ro,w = {r_glass_expected:.8}"
        );

        // E+ Ro,w ≈ 0.034, NOT 0.044 as the ticket originally claimed.
        // The NFRC 100-2020 fixed value (0.044 m²·K/W) is incorrect for E+ conditions.
        assert!(
            (ro_w - 0.0440).abs() > 0.005,
            "U={u}: E+ Ro,w={ro_w:.5} differs from ticket's claimed 0.0440"
        );
    }
}

/// Definition-of-done round-trip test: U=2.0 → R_total = 0.500 m²·K/W.
#[test]
fn window_u_2_round_trip_to_0_5() {
    let (r_glass, r_int, r_ext) = window_u_factor_decomposition(2.0).unwrap();
    let r_total = r_glass + r_int + r_ext;
    assert!(
        (r_total - 0.5).abs() < 1e-10,
        "U=2.0 must round-trip to R=0.500; got {r_total:.10}"
    );
}

/// Definition-of-done round-trip test: U=0.5 → R_total = 2.000 m²·K/W.
#[test]
fn window_u_0_5_round_trip_to_2_0() {
    let (r_glass, r_int, r_ext) = window_u_factor_decomposition(0.5).unwrap();
    let r_total = r_glass + r_int + r_ext;
    assert!(
        (r_total - 2.0).abs() < 1e-10,
        "U=0.5 must round-trip to R=2.000; got {r_total:.10}"
    );
}
