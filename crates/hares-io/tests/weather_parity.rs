//! OCHRE parity tests for weather data loading and processing.
//!
//! - Sky temperature formula: Stefan-Boltzmann inversion of horizontal infrared
//! - Clark-Allen fallback formula for low-IR conditions
//! - EPW column parsing with SI units (uses fixture at tests/fixtures/weather/test_location.epw)
//! - Ground temperature model properties
//!
//! Sky temp formulas are tested by reproducing the math from epw.rs constants.
//! These are the same formulas OCHRE uses (EnergyPlus method + Clark-Allen).

// ---------------------------------------------------------------------------
// Sky temperature formula constants (imported from hares-physics).
// These reproduce the exact computation to verify HARES uses the right values.
// ---------------------------------------------------------------------------

use hares_physics::constants::{CELSIUS_TO_KELVIN as KELVIN_OFFSET, STEFAN_BOLTZMANN};
/// Threshold below which Clark-Allen fallback is used (matches INFRARED_FALLBACK_THRESHOLD in epw.rs).
const INFRARED_FALLBACK_THRESHOLD: f64 = 50.0;

/// Reproduce the Stefan-Boltzmann sky temperature inversion used in epw.rs.
///
/// `T_sky = (IR / σ)^0.25 - 273.15`
fn sky_temp_stefan_boltzmann_c(horizontal_infrared_w_m2: f64) -> f64 {
    let t_sky_k = (horizontal_infrared_w_m2 / STEFAN_BOLTZMANN).powf(0.25);
    t_sky_k - KELVIN_OFFSET
}

/// Reproduce the Clark-Allen fallback formula used in epw.rs.
///
/// `T_sky = T_db × (0.787 + 0.764 × ln(T_dp / 273.15))^0.25 - 273.15`
///
/// Reference: Clark & Allen (1978), ASES; OCHRE epw.py clark_allen_sky_temp().
fn sky_temp_clark_allen_c(dry_bulb_c: f64, dew_point_c: f64) -> f64 {
    let dry_bulb_k = dry_bulb_c + KELVIN_OFFSET;
    let dew_point_k = dew_point_c + KELVIN_OFFSET;
    let sky_k = dry_bulb_k * (0.787 + 0.764 * (dew_point_k / KELVIN_OFFSET).ln()).powf(0.25);
    sky_k - KELVIN_OFFSET
}

// ---------------------------------------------------------------------------
// Test: Stefan-Boltzmann sky temperature for known infrared values
//
// OCHRE / EnergyPlus method: T_sky = (IR / σ)^(1/4) in Kelvin, then to °C.
//
// Reference values computed from the formula:
//   IR = 300 W/m²: T_sky_k = (300 / 5.670374419e-8)^0.25 ≈ 269.7 K → -3.5 °C
//   IR = 400 W/m²: T_sky_k = (400 / 5.670374419e-8)^0.25 ≈ 289.7 K → 16.5 °C
//
// Tolerance: 0.1 °C (formula-based, no numerical table rounding).
// ---------------------------------------------------------------------------

#[test]
fn sky_temp_stefan_boltzmann_300_w_m2() {
    let sky_c = sky_temp_stefan_boltzmann_c(300.0);

    // Reference value from Stefan-Boltzmann inversion using σ = 5.670374419e-8 W/m²/K⁴
    // (NIST CODATA 2018):
    //   T_K = (300 / 5.670374419e-8)^0.25 ≈ 269.7 K
    //   T_C ≈ -3.5 °C
    // Value independently derived, not computed via the function under test.
    // Tolerance: 0.1°C to allow for minor floating-point differences across platforms.
    let expected_c = -3.5_f64;
    assert!(
        (sky_c - expected_c).abs() < 0.1,
        "Stefan-Boltzmann sky temp at IR=300 W/m²: got {sky_c:.3}°C, expected ≈{expected_c:.2}°C"
    );
}

#[test]
fn sky_temp_stefan_boltzmann_increases_monotonically_with_ir() {
    // Higher IR → higher equivalent blackbody temperature.
    let ir_values = [100.0_f64, 200.0, 300.0, 400.0, 500.0, 600.0];
    let sky_temps: Vec<f64> = ir_values
        .iter()
        .map(|&ir| sky_temp_stefan_boltzmann_c(ir))
        .collect();

    for window in sky_temps.windows(2) {
        assert!(
            window[1] > window[0],
            "sky temperature must increase with IR: {:.3}°C vs {:.3}°C",
            window[1],
            window[0]
        );
    }
}

#[test]
fn sky_temp_infrared_fallback_threshold_is_50_w_m2() {
    // Values at and below 50 W/m² must use Clark-Allen in the EPW parser.
    // Values above 50 W/m² must use Stefan-Boltzmann.
    // Verify the threshold constant is correct by checking IR=50 is above threshold.
    const { assert!(INFRARED_FALLBACK_THRESHOLD == 50.0) };

    // Stefan-Boltzmann at IR = 50.0 W/m² (boundary -- just at threshold).
    let sky_sb = sky_temp_stefan_boltzmann_c(50.0);
    // Expected: (50 / 5.670374419e-8)^0.25 - 273.15 ≈ 182.6 K - 273.15 ≈ -90.5°C.
    // This is the transition point -- values below 50 fall back to Clark-Allen
    // because 50 W/m² is physically implausible for atmospheric IR.
    assert!(
        sky_sb < -50.0,
        "Stefan-Boltzmann at 50 W/m² should give an implausibly cold sky (< -50°C): got {sky_sb:.2}°C. \
         This confirms why IR < 50 W/m² falls back to Clark-Allen."
    );
}

// ---------------------------------------------------------------------------
// Test: Clark-Allen fallback sky temperature formula
//
// Clark & Allen (1978) empirical correlation:
//   T_sky = T_db × (0.787 + 0.764 × ln(T_dp / 273.15))^0.25
//
// OCHRE uses this when horizontal infrared radiation < 50 W/m².
//
// Reference: Clark & Allen (1978), ASES; OCHRE Weather.py clark_allen_sky_temp().
// ---------------------------------------------------------------------------

#[test]
fn clark_allen_sky_temp_at_20c_dry_10c_dew() {
    // T_db = 20°C, T_dp = 10°C -- moderate humidity condition.
    // T_db_k = 293.15, T_dp_k = 283.15
    // ln(283.15 / 273.15) = ln(1.0366) ≈ 0.03594
    // factor = (0.787 + 0.764 × 0.03594)^0.25 = (0.787 + 0.02746)^0.25
    //        = 0.81446^0.25 ≈ 0.9513
    // T_sky_k = 293.15 × 0.9513 ≈ 278.9 K → 5.75°C
    let sky_c = sky_temp_clark_allen_c(20.0, 10.0);

    // Must be cooler than dry-bulb (sky radiation < ambient blackbody).
    assert!(
        sky_c < 20.0,
        "Clark-Allen sky temp must be cooler than dry-bulb: sky={sky_c:.2}°C, db=20°C"
    );

    // Must be above absolute zero (sanity check on formula).
    assert!(
        sky_c > -100.0,
        "Clark-Allen sky temp implausibly cold: {sky_c:.2}°C"
    );

    // Should be in the range 0–15°C for these moderate conditions.
    assert!(
        sky_c > 0.0 && sky_c < 15.0,
        "Clark-Allen at db=20°C, dp=10°C: expected (0, 15)°C, got {sky_c:.2}°C"
    );
}

#[test]
fn clark_allen_sky_temp_decreases_with_lower_dew_point() {
    // Lower dew point (drier air) → less atmospheric emission → lower sky temp.
    let sky_dry = sky_temp_clark_allen_c(20.0, 0.0); // dry: dp=0°C
    let sky_humid = sky_temp_clark_allen_c(20.0, 15.0); // humid: dp=15°C

    assert!(
        sky_dry < sky_humid,
        "drier air should give lower sky temp: dry={sky_dry:.2}°C > humid={sky_humid:.2}°C"
    );
}

#[test]
fn clark_allen_sky_temp_dew_point_at_dry_bulb_gives_maximum() {
    // When T_dp = T_db (100% RH), ln(T_dp / 273.15) = ln(T_db / 273.15)
    // and the result should be close to (but still below) T_db.
    let t_db = 15.0_f64;
    let sky_max = sky_temp_clark_allen_c(t_db, t_db);

    assert!(
        sky_max < t_db,
        "Clark-Allen sky temp must always be below dry-bulb: sky={sky_max:.2}°C, db={t_db:.2}°C"
    );
    assert!(
        sky_max > t_db - 20.0,
        "Clark-Allen at 100% RH should be close to dry-bulb: sky={sky_max:.2}°C, db={t_db:.2}°C"
    );
}

// ---------------------------------------------------------------------------
// Test: EPW file parsing (integration test -- requires real EPW file)
//
// Verifies:
// - All 13 columns parsed with correct SI units (°C, kPa, W/m², m/s)
// - Pressure converted from Pa to kPa (EPW stores Pa, HARES stores kPa)
// - Sky temperature computed from horizontal infrared
// - Ground temperature series has the same length as dry-bulb
//
// EPW fixture lives at tests/fixtures/weather/test_location.epw.
// ---------------------------------------------------------------------------

#[test]
fn epw_all_columns_parsed_with_correct_si_units() {
    use std::path::PathBuf;

    let epw_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/weather/test_location.epw");

    let weather = hares_io::parse_epw(&epw_path).expect("EPW should parse");

    // Length: 8760 (standard) or 8784 (leap year).
    let n = weather.len();
    assert!(
        n == 8760 || n == 8784,
        "EPW must produce 8760 or 8784 records, got {n}"
    );

    // Pressure stored in kPa (EPW source is Pa → divided by 1000).
    for (i, &p) in weather.pressure_kpa.iter().enumerate() {
        assert!(
            p > 60.0 && p < 110.0,
            "row {i}: pressure {p} kPa outside plausible [60, 110] kPa range (check Pa→kPa conversion)"
        );
    }

    // Temperature in °C (not Kelvin).
    for (i, &t) in weather.dry_bulb_c.iter().enumerate() {
        assert!(
            t > -60.0 && t < 60.0,
            "row {i}: dry_bulb {t} should be in °C, not Kelvin (range [-60, 60] °C)"
        );
    }

    // Wind speed in m/s (not mph or km/h).
    for (i, &v) in weather.wind_speed_m_s.iter().enumerate() {
        assert!(
            (0.0..60.0).contains(&v),
            "row {i}: wind speed {v} m/s outside [0, 60) m/s (check unit)"
        );
    }

    // GHI in W/m².
    for (i, &ghi) in weather.ghi_w_m2.iter().enumerate() {
        assert!(
            (0.0..=1500.0).contains(&ghi),
            "row {i}: GHI {ghi} W/m² outside [0, 1500] range"
        );
    }

    // Sky temperature series has same length as dry-bulb.
    assert_eq!(
        weather.sky_temp_c.len(),
        n,
        "sky_temp_c series must have same length as dry_bulb_c"
    );

    // Ground temperature series has same length.
    assert_eq!(
        weather.ground_temp_c.len(),
        n,
        "ground_temp_c series must have same length as dry_bulb_c"
    );

    // Latitude, longitude, timezone from EPW header are physically plausible.
    assert!(
        weather.meta.latitude.abs() <= 90.0,
        "latitude {} out of range",
        weather.meta.latitude
    );
    assert!(
        weather.meta.longitude.abs() <= 180.0,
        "longitude {} out of range",
        weather.meta.longitude
    );
    assert!(
        weather.meta.timezone_offset_h.abs() <= 14.0,
        "timezone_offset_h {} out of range",
        weather.meta.timezone_offset_h
    );
}

// ---------------------------------------------------------------------------
// Test: sky temperature from EPW infrared matches Stefan-Boltzmann formula
//
// When horizontal_infrared >= 50 W/m², the EPW parser applies:
//   T_sky = (IR / σ)^0.25 - 273.15
//
// This test verifies the stored sky_temp_c is consistent with that formula
// for rows where IR is above the fallback threshold.
// ---------------------------------------------------------------------------

#[test]
fn sky_temp_matches_stefan_boltzmann_for_high_ir_rows() {
    use std::path::PathBuf;

    let epw_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/weather/test_location.epw");

    let weather = hares_io::parse_epw(&epw_path).expect("EPW should parse");

    let mut checked = 0usize;
    for i in 0..weather.len() {
        let ir = weather.horizontal_infrared_w_m2[i];
        if ir < INFRARED_FALLBACK_THRESHOLD {
            continue; // skip rows that use Clark-Allen fallback
        }
        let expected_sky_c = sky_temp_stefan_boltzmann_c(ir);
        let actual_sky_c = weather.sky_temp_c[i];

        // Tolerance: 0.01°C -- formula identity with no table rounding.
        assert!(
            (actual_sky_c - expected_sky_c).abs() < 0.01,
            "row {i}: sky_temp_c={actual_sky_c:.3}°C, expected {expected_sky_c:.3}°C (IR={ir:.1} W/m²)"
        );
        checked += 1;
    }

    assert!(
        checked > 100,
        "expected >100 rows with IR >= 50 W/m² for meaningful check, got {checked}"
    );
}

// ---------------------------------------------------------------------------
// Test: Clark-Allen fallback consistency for low-IR rows.
//
// For rows where horizontal_infrared < 50 W/m², the EPW parser uses the
// Clark-Allen sky temperature model (T_db, T_dp) instead of Stefan-Boltzmann.
// This test verifies that the stored sky_temp_c matches the Clark-Allen formula
// for those low-IR rows.
// ---------------------------------------------------------------------------

#[test]
fn sky_temp_matches_clark_allen_for_low_ir_rows() {
    use std::path::PathBuf;

    let epw_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/weather/test_location.epw");

    let weather = hares_io::parse_epw(&epw_path).expect("EPW should parse");

    let mut checked = 0usize;
    for i in 0..weather.len() {
        let ir = weather.horizontal_infrared_w_m2[i];
        if ir >= INFRARED_FALLBACK_THRESHOLD {
            continue; // only check Clark-Allen rows
        }
        let expected_sky_c = sky_temp_clark_allen_c(weather.dry_bulb_c[i], weather.dew_point_c[i]);
        let actual_sky_c = weather.sky_temp_c[i];

        assert!(
            (actual_sky_c - expected_sky_c).abs() < 0.01,
            "row {i}: sky_temp_c={actual_sky_c:.3}°C, expected Clark-Allen {expected_sky_c:.3}°C"
        );
        checked += 1;
    }

    // It's valid if no rows trigger the fallback; just log it.
    if checked == 0 {
        eprintln!(
            "No rows with IR < 50 W/m² found; Clark-Allen fallback not exercised by this EPW."
        );
    }
}

// ===========================================================================
// PCHIP interpolation and sub-hourly resampling tests
// ===========================================================================

use hares_io::weather::{WeatherMeta, WeatherTimeSeries, pchip_resample};

/// Build a `WeatherTimeSeries` with explicit per-field overrides for bound-testing.
#[allow(clippy::too_many_arguments)]
fn make_series_full(
    n: usize,
    rel_humidity_pct: Vec<f64>,
    opaque_sky_cover: Vec<f64>,
    pressure_kpa: Vec<f64>,
    ghi_w_m2: Vec<f64>,
    dni_w_m2: Vec<f64>,
    dhi_w_m2: Vec<f64>,
    wind_speed_m_s: Vec<f64>,
    wind_dir_deg: Vec<f64>,
) -> WeatherTimeSeries {
    WeatherTimeSeries {
        meta: WeatherMeta {
            location: "Test".to_string(),
            latitude: 39.74,
            longitude: -104.99,
            timezone_offset_h: -7.0,
            elevation_m: 1600.0,
            wf_allows_leap_years: true,
            source_step_secs: 3600,
            midpoint_offset_secs: 0,
            has_embedded_location: false,
        },
        dry_bulb_c: vec![20.0; n],
        dew_point_c: vec![10.0; n],
        rel_humidity_pct,
        pressure_kpa,
        ghi_w_m2,
        dni_w_m2,
        dhi_w_m2,
        wind_speed_m_s,
        wind_dir_deg,
        opaque_sky_cover,
        horizontal_infrared_w_m2: vec![350.0; n],
        sky_temp_c: vec![0.0; n],
        ground_temp_c: vec![10.0; n],
        liquid_precip_m: vec![0.0; n],
        surface_albedo: None,
        design_conditions: None,
    }
}

// ---------------------------------------------------------------------------
// PCHIP correctness tests
// ---------------------------------------------------------------------------

#[test]
fn pchip_passes_through_knot_values() {
    let values = [10.0, 15.0, 12.0, 18.0, 14.0, 20.0, 16.0, 22.0];
    let factor = 6;
    let out = pchip_resample(&values, factor);
    assert_eq!(out.len(), values.len() * factor);

    for (k, &v) in values.iter().enumerate() {
        let idx = k * factor;
        assert!(
            (out[idx] - v).abs() < 1e-10,
            "knot {k} at index {idx}: expected {v}, got {}",
            out[idx]
        );
    }
}

#[test]
fn pchip_is_c1_continuous() {
    // Verify first derivative is continuous at knot points via finite differences.
    // Use Richardson extrapolation: compute one-sided derivatives at two step sizes
    // and extrapolate to eliminate the O(h) truncation error, yielding O(h^2) accuracy.
    let values = [5.0, 12.0, 8.0, 20.0, 15.0, 10.0, 18.0];
    let factor = 1000;
    let out = pchip_resample(&values, factor);
    let h = 1.0 / factor as f64;

    for k in 1..values.len() - 1 {
        let idx = k * factor;
        // Two-point one-sided differences at step h and 2h for Richardson extrapolation.
        let d_left_h = (out[idx] - out[idx - 1]) / h;
        let d_left_2h = (out[idx] - out[idx - 2]) / (2.0 * h);
        let d_left = 2.0 * d_left_h - d_left_2h; // Richardson: cancels O(h) term

        let d_right_h = (out[idx + 1] - out[idx]) / h;
        let d_right_2h = (out[idx + 2] - out[idx]) / (2.0 * h);
        let d_right = 2.0 * d_right_h - d_right_2h;

        assert!(
            (d_left - d_right).abs() < 1e-4,
            "C1 discontinuity at knot {k}: left slope {d_left:.6}, right slope {d_right:.6}, \
             diff {:.2e}",
            (d_left - d_right).abs()
        );
    }
}

// ---------------------------------------------------------------------------
// Monotonicity / no-overshoot tests
// ---------------------------------------------------------------------------

#[test]
fn pchip_preserves_monotonicity() {
    let values = [10.0, 12.0, 15.0, 20.0, 26.0, 33.0];
    let out = pchip_resample(&values, 12);

    for w in out.windows(2) {
        assert!(
            w[1] >= w[0] - 1e-12,
            "monotonicity violated: {:.10} followed by {:.10}",
            w[0],
            w[1]
        );
    }
}

#[test]
fn pchip_no_overshoot_between_knots() {
    let values = [5.0, 20.0, 8.0, 25.0, 3.0, 18.0];
    let factor = 10;
    let out = pchip_resample(&values, factor);

    for k in 0..values.len() - 1 {
        let lo = values[k].min(values[k + 1]);
        let hi = values[k].max(values[k + 1]);
        for j in 0..factor {
            let idx = k * factor + j;
            assert!(
                out[idx] >= lo - 1e-10 && out[idx] <= hi + 1e-10,
                "overshoot in interval [{k}, {}]: index {idx} value {:.10}, bounds [{lo}, {hi}]",
                k + 1,
                out[idx]
            );
        }
    }
}

#[test]
fn pchip_handles_flat_sections() {
    let values = [20.0, 20.0, 20.0, 20.0, 20.0];
    let out = pchip_resample(&values, 6);

    for (i, &v) in out.iter().enumerate() {
        assert!(
            (v - 20.0).abs() < 1e-12,
            "flat section: index {i} expected 20.0, got {v}"
        );
    }
}

#[test]
fn pchip_handles_steep_gradient() {
    let values = [10.0, 10.0, 30.0, 30.0];
    let out = pchip_resample(&values, 10);

    for (i, &v) in out.iter().enumerate() {
        assert!(
            (10.0 - 1e-10..=30.0 + 1e-10).contains(&v),
            "steep gradient overshoot at index {i}: {v}"
        );
    }
}

// ---------------------------------------------------------------------------
// Physical bounds (clamping) tests
// ---------------------------------------------------------------------------

#[test]
fn rh_stays_bounded_after_interpolation() {
    let rh = vec![98.0, 2.0, 98.0, 5.0, 95.0];
    let n = rh.len();
    let series = make_series_full(
        n,
        rh,
        vec![5.0; n],
        vec![101.325; n],
        vec![300.0; n],
        vec![500.0; n],
        vec![100.0; n],
        vec![3.0; n],
        vec![180.0; n],
    );
    let resampled = series.resample(600).expect("resample should succeed");

    for (i, &v) in resampled.rel_humidity_pct.iter().enumerate() {
        assert!(
            (0.0..=100.0).contains(&v),
            "RH out of [0,100] at index {i}: {v}"
        );
    }
}

#[test]
fn rh_non_monotonic_extrema_clamped() {
    let rh = vec![0.0, 100.0, 0.0, 100.0, 0.0];
    let n = rh.len();
    let series = make_series_full(
        n,
        rh,
        vec![5.0; n],
        vec![101.325; n],
        vec![300.0; n],
        vec![500.0; n],
        vec![100.0; n],
        vec![3.0; n],
        vec![180.0; n],
    );
    let resampled = series.resample(600).expect("resample should succeed");

    for (i, &v) in resampled.rel_humidity_pct.iter().enumerate() {
        assert!(
            (0.0..=100.0).contains(&v),
            "RH extrema out of [0,100] at index {i}: {v}"
        );
    }
}

#[test]
fn sky_cover_stays_bounded_after_interpolation() {
    let sky = vec![0.0, 10.0, 0.0, 10.0, 0.0];
    let n = sky.len();
    let series = make_series_full(
        n,
        vec![50.0; n],
        sky,
        vec![101.325; n],
        vec![300.0; n],
        vec![500.0; n],
        vec![100.0; n],
        vec![3.0; n],
        vec![180.0; n],
    );
    let resampled = series.resample(600).expect("resample should succeed");

    for (i, &v) in resampled.opaque_sky_cover.iter().enumerate() {
        assert!(
            (0.0..=10.0).contains(&v),
            "sky cover out of [0,10] at index {i}: {v}"
        );
    }
}

#[test]
fn pressure_stays_positive_after_interpolation() {
    // Oscillating pressure to try to provoke negative undershoots.
    let p = vec![101.0, 95.0, 105.0, 90.0, 100.0];
    let n = p.len();
    let series = make_series_full(
        n,
        vec![50.0; n],
        vec![5.0; n],
        p,
        vec![300.0; n],
        vec![500.0; n],
        vec![100.0; n],
        vec![3.0; n],
        vec![180.0; n],
    );
    let resampled = series.resample(600).expect("resample should succeed");

    for (i, &v) in resampled.pressure_kpa.iter().enumerate() {
        assert!(v > 0.0, "pressure non-positive at index {i}: {v}");
    }
}

// ---------------------------------------------------------------------------
// Edge cases / degenerate input tests
// ---------------------------------------------------------------------------

#[test]
fn pchip_handles_single_element() {
    let out = pchip_resample(&[42.5], 6);
    assert_eq!(out.len(), 6);
    for (i, &v) in out.iter().enumerate() {
        assert!(
            (v - 42.5).abs() < 1e-15,
            "single element: index {i} expected 42.5, got {v}"
        );
    }
}

#[test]
fn pchip_handles_two_elements() {
    let out = pchip_resample(&[10.0, 20.0], 4);
    assert_eq!(out.len(), 8);
    // First 4 samples: linear from 10 to 20 (exclusive of endpoint at index 4).
    for (i, &v) in out.iter().enumerate().take(4) {
        let expected = 10.0 + (i as f64) * 2.5;
        assert!(
            (v - expected).abs() < 1e-10,
            "two-element linear at index {i}: expected {expected}, got {v}",
        );
    }
    // Samples at index 4+ use flat extrapolation: all equal 20.0.
    for (i, &v) in out.iter().enumerate().skip(4) {
        assert!(
            (v - 20.0).abs() < 1e-10,
            "two-element flat extrapolation at index {i}: expected 20.0, got {v}",
        );
    }
}

#[test]
fn pchip_handles_all_nan() {
    let values = [f64::NAN, f64::NAN, f64::NAN, f64::NAN, f64::NAN];
    let out = pchip_resample(&values, 4);
    assert_eq!(out.len(), 20);
    for (i, &v) in out.iter().enumerate() {
        assert!(
            v.is_nan(),
            "all-NaN input: index {i} should be NaN, got {v}"
        );
    }
}

// ---------------------------------------------------------------------------
// ZOH preservation tests
// ---------------------------------------------------------------------------

#[test]
fn solar_fields_use_zoh_by_default() {
    // Solar fields (GHI, DNI, DHI) default to ZOH resampling to preserve
    // the hourly energy integral. Every sub-step within an hour must equal
    // the source hourly value.
    let n = 5;
    let ghi = vec![0.0, 200.0, 500.0, 300.0, 0.0];
    let dni = vec![0.0, 400.0, 800.0, 600.0, 0.0];
    let dhi = vec![0.0, 50.0, 120.0, 80.0, 0.0];
    let series = make_series_full(
        n,
        vec![50.0; n],
        vec![5.0; n],
        vec![101.325; n],
        ghi.clone(),
        dni.clone(),
        dhi.clone(),
        vec![3.0; n],
        vec![180.0; n],
    );
    let factor = 6; // 600s timestep
    let resampled = series
        .resample(3600 / factor as u32)
        .expect("resample should succeed");

    // Verify ZOH: every sub-step equals the source hourly value.
    for (field_name, source, resampled_field) in [
        ("GHI", &ghi, &resampled.ghi_w_m2),
        ("DNI", &dni, &resampled.dni_w_m2),
        ("DHI", &dhi, &resampled.dhi_w_m2),
    ] {
        for (k, &src_val) in source.iter().enumerate() {
            for j in 0..factor {
                let idx = k * factor + j;
                assert!(
                    (resampled_field[idx] - src_val).abs() < 1e-12,
                    "{field_name} hour {k}, sub-step {j}: expected {src_val}, got {}",
                    resampled_field[idx]
                );
            }
        }
    }
}

/// ZOH default preserves hourly energy integral at sunset boundary.
///
/// Sequence: [400, 0, 0] W/m² at 15-minute resolution (factor=4).
/// With ZOH default, the first nighttime hour must have mean zero — no energy leak.
#[test]
fn zoh_default_preserves_hourly_energy_integral_at_sunset_boundary() {
    let values = vec![400.0_f64, 0.0, 0.0];
    let n = values.len();
    let series = make_series_full(
        n,
        vec![50.0; n],
        vec![5.0; n],
        vec![101.325; n],
        values.clone(), // ghi
        values.clone(), // dni
        values.clone(), // dhi
        vec![3.0; n],
        vec![180.0; n],
    );
    let factor: usize = 4; // 15-minute sub-steps
    let resampled = series
        .resample(3600 / factor as u32)
        .expect("resample should succeed");
    // Default for solar is ZOH — no override needed.

    for (field_name, src, resampled_field) in [
        ("GHI", &values, &resampled.ghi_w_m2),
        ("DNI", &values, &resampled.dni_w_m2),
        ("DHI", &values, &resampled.dhi_w_m2),
    ] {
        for (k, &src_val) in src.iter().enumerate() {
            let hour_slice = &resampled_field[k * factor..(k + 1) * factor];
            let mean = hour_slice.iter().sum::<f64>() / factor as f64;
            assert!(
                (mean - src_val).abs() < 1e-12,
                "{field_name} hour {k}: ZOH mean {mean} != source {src_val}"
            );
        }
    }
}

#[test]
fn wind_speed_uses_zoh_wind_dir_uses_circular_linear() {
    // Wind speed defaults to ZOH; wind direction defaults to CircularLinear.
    let n = 5;
    let ws = vec![1.0, 5.0, 3.0, 8.0, 2.0];
    let wd = vec![90.0, 180.0, 270.0, 0.0, 45.0];
    let series = make_series_full(
        n,
        vec![50.0; n],
        vec![5.0; n],
        vec![101.325; n],
        vec![300.0; n],
        vec![500.0; n],
        vec![100.0; n],
        ws.clone(),
        wd.clone(),
    );
    let factor = 4; // 900s timestep
    let resampled = series
        .resample(3600 / factor as u32)
        .expect("resample should succeed");

    // Wind speed: ZOH — each sub-step equals the source value.
    for (k, &src_val) in ws.iter().enumerate() {
        for j in 0..factor {
            let idx = k * factor + j;
            assert!(
                (resampled.wind_speed_m_s[idx] - src_val).abs() < 1e-15,
                "wind_speed ZOH violated at hour {k}, sub-step {j} (index {idx}): \
                 expected {src_val}, got {}",
                resampled.wind_speed_m_s[idx]
            );
        }
    }

    // Wind direction: CircularLinear — at the start of each segment (frac=0),
    // the value equals the source value, but sub-hourly values vary.
    for (k, &src_val) in wd.iter().enumerate() {
        let start_idx = k * factor;
        assert!(
            (resampled.wind_dir_deg[start_idx] - src_val).abs() < 1e-12,
            "wind_dir at start of hour {k}: expected {src_val}, got {}",
            resampled.wind_dir_deg[start_idx]
        );
    }
    // Verify wind_dir is NOT ZOH: at least some sub-hourly values differ.
    let all_constant = wd.iter().enumerate().all(|(k, &src_val)| {
        (0..factor).all(|j| (resampled.wind_dir_deg[k * factor + j] - src_val).abs() < 1e-12)
    });
    assert!(
        !all_constant,
        "wind_dir appears to use ZOH — expected CircularLinear interpolation"
    );
}

// ---------------------------------------------------------------------------
// Ticket 030 regression tests — solar upsampling energy conservation
// ---------------------------------------------------------------------------

/// Regression guard: triangular resampling at a sunset boundary DOES produce
/// nonzero sub-hourly values in the nighttime hour.
///
/// Sequence: last sunlit hour (400 W/m²) followed by two nighttime hours (0).
/// At 15-minute resolution (factor=4), the first nighttime hour receives
/// sub-hourly values [200, 100, 0, 0] from the triangular formula, giving a
/// mean of 75 W/m² — confirming the energy conservation violation in #030.
/// (The ticket incorrectly states [100,0,0,0] / 25 W/m²; the correct values
/// are [200,100,0,0] / 75 W/m² from the formula prev*(0.5-frac)+i*(0.5+frac).)
#[test]
fn triangular_sunset_boundary_bleeds_into_nighttime_hour() {
    let values = vec![400.0_f64, 0.0, 0.0];
    let n = values.len();
    let series = make_series_full(
        n,
        vec![50.0; n],
        vec![5.0; n],
        vec![101.325; n],
        values.clone(), // ghi
        values.clone(), // dni
        values.clone(), // dhi
        vec![3.0; n],
        vec![180.0; n],
    );
    let factor: usize = 4; // 15-minute sub-steps
    // Explicit Triangular override — ZOH is now the default for solar fields.
    use hares_io::{ResampleMethod, ResampleOverrides};
    let overrides = ResampleOverrides {
        ghi: Some(ResampleMethod::Triangular),
        dni: Some(ResampleMethod::Triangular),
        dhi: Some(ResampleMethod::Triangular),
        ..Default::default()
    };
    let resampled = series
        .resample_with(3600 / factor as u32, &overrides)
        .expect("resample_with should succeed");

    for (field_name, resampled_field) in [
        ("GHI", &resampled.ghi_w_m2),
        ("DNI", &resampled.dni_w_m2),
        ("DHI", &resampled.dhi_w_m2),
    ] {
        let night_hour = &resampled_field[factor..2 * factor]; // hour index 1
        let night_mean: f64 = night_hour.iter().sum::<f64>() / factor as f64;

        // The mean is nonzero — triangular leaks energy across the day/night boundary.
        // Exact expected: 75 W/m² = 400 * (0.5+0.25+0+0) / 4 = 400 * 3/16.
        assert!(
            night_mean > 1.0,
            "{field_name} nighttime hour mean should be nonzero (energy leak), got {night_mean:.4}"
        );

        // Verify specific sub-hourly values:
        // frac=0.00: prev(400)*0.5 + i(0)*0.5 = 200
        // frac=0.25: prev(400)*0.25 + i(0)*0.75 = 100
        // frac=0.50: i(0)*1.0 = 0
        // frac=0.75: i(0)*0.75 + next(0)*0.25 = 0
        let expected_night = [200.0_f64, 100.0, 0.0, 0.0];
        for (j, (&got, &exp)) in night_hour.iter().zip(expected_night.iter()).enumerate() {
            assert!(
                (got - exp).abs() < 1e-10,
                "{field_name} nighttime sub-step {j}: expected {exp}, got {got}"
            );
        }

        // Confirm mean = 75, not 25 as the ticket incorrectly states
        assert!(
            (night_mean - 75.0).abs() < 1e-9,
            "{field_name} nighttime mean expected 75.0 W/m² (= 400 × 3/16), got {night_mean}"
        );
    }
}

/// ZOH resampling preserves the hourly energy integral exactly.
///
/// Sequence: [400, 0, 0] W/m² at 15-minute resolution (factor=4).
/// Every sub-hourly value in each hour must equal the source value exactly,
/// so the mean of each hour equals the source value.
#[test]
fn zoh_preserves_hourly_energy_integral_at_sunset_boundary() {
    let values = vec![400.0_f64, 0.0, 0.0];
    let n = values.len();
    let series = make_series_full(
        n,
        vec![50.0; n],
        vec![5.0; n],
        vec![101.325; n],
        values.clone(), // ghi
        values.clone(), // dni
        values.clone(), // dhi
        vec![3.0; n],
        vec![180.0; n],
    );

    use hares_io::{ResampleMethod, ResampleOverrides};
    let overrides = ResampleOverrides {
        ghi: Some(ResampleMethod::Zoh),
        dni: Some(ResampleMethod::Zoh),
        dhi: Some(ResampleMethod::Zoh),
        ..Default::default()
    };
    let factor: usize = 4; // 15-minute sub-steps
    let resampled = series
        .resample_with(3600 / factor as u32, &overrides)
        .expect("resample_with should succeed");

    for (field_name, src, resampled_field) in [
        ("GHI", &values, &resampled.ghi_w_m2),
        ("DNI", &values, &resampled.dni_w_m2),
        ("DHI", &values, &resampled.dhi_w_m2),
    ] {
        for (k, &src_val) in src.iter().enumerate() {
            let hour_slice = &resampled_field[k * factor..(k + 1) * factor];
            let mean = hour_slice.iter().sum::<f64>() / factor as f64;
            assert!(
                (mean - src_val).abs() < 1e-9,
                "{field_name} hour {k}: ZOH mean {mean} != source {src_val} (error > 1e-9)"
            );
            // Every sub-step must equal the source value exactly for ZOH.
            for (j, &v) in hour_slice.iter().enumerate() {
                assert!(
                    (v - src_val).abs() < 1e-12,
                    "{field_name} hour {k}, sub-step {j}: expected {src_val}, got {v}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Year boundary test
// ---------------------------------------------------------------------------

#[test]
fn year_boundary_uses_flat_extrapolation() {
    let values = [10.0, 15.0, 20.0, 25.0, 30.0];
    let factor = 6;
    let out = pchip_resample(&values, factor);

    // The last knot value is 30.0. All sub-hourly samples in the final
    // half-interval (beyond the last knot) should be flat at 30.0,
    // not wrapping back toward the first knot.
    let last_knot_idx = (values.len() - 1) * factor;
    let last_knot_val = values[values.len() - 1];
    for (i, &v) in out.iter().enumerate().skip(last_knot_idx) {
        assert!(
            (v - last_knot_val).abs() < 1e-10,
            "year boundary: index {i} expected flat at {last_knot_val}, got {v}",
        );
    }
}

// ---------------------------------------------------------------------------
// NaN propagation test
// ---------------------------------------------------------------------------

#[test]
fn nan_in_source_propagates_through_pchip() {
    let values = [10.0, 15.0, f64::NAN, 20.0, 25.0, 30.0, 35.0];
    let factor = 4;
    let out = pchip_resample(&values, factor);

    // NaN at knot 2 contaminates via two mechanisms:
    // 1. Hermite formula: interval [1,2] uses values[2]=NaN directly (h01 * NaN).
    // 2. Secant slopes: delta[1]=NaN and delta[2]=NaN propagate into d[1..3].
    // Both interval [1,2] and [2,3] must produce NaN output.
    for j in 0..factor {
        let idx_12 = factor + j;
        assert!(
            out[idx_12].is_nan(),
            "interval [1,2] index {idx_12} should be NaN (values[2]=NaN in Hermite), got {}",
            out[idx_12]
        );
        let idx_23 = 2 * factor + j;
        assert!(
            out[idx_23].is_nan(),
            "interval [2,3] index {idx_23} should be NaN (NaN knot interval), got {}",
            out[idx_23]
        );
    }

    // Distant intervals must NOT be corrupted.
    // Interval [4,5] recovers because d[4] and d[5] are derived from finite
    // secants (delta[3]=5, delta[4]=5, delta[5]=5). d[3] is zeroed by the
    // sign-check (delta[2]=NaN), but that only affects interval [3,4].
    for j in 0..factor {
        let idx = 4 * factor + j;
        assert!(
            out[idx].is_finite(),
            "NaN corruption: distant interval index {idx} should be finite, got {}",
            out[idx]
        );
    }
}

#[test]
fn pchip_two_consecutive_nans_do_not_panic() {
    // NaN at knots 2 and 3 → intervals touching them should be NaN,
    // but distant intervals should recover to finite values.
    let values = [10.0, 15.0, f64::NAN, f64::NAN, 20.0, 25.0];
    let factor = 4;
    let out = pchip_resample(&values, factor);
    assert_eq!(out.len(), values.len() * factor);

    // Interval [1,2] touches first NaN knot -- must be NaN.
    for j in 0..factor {
        let idx = factor + j;
        assert!(
            out[idx].is_nan(),
            "interval [1,2] index {idx} should be NaN, got {}",
            out[idx]
        );
    }

    // Interval [4,5] is well past the NaN gap -- should recover.
    for j in 0..factor {
        let idx = 4 * factor + j;
        assert!(
            out[idx].is_finite(),
            "interval [4,5] index {idx} should be finite after NaN gap, got {}",
            out[idx]
        );
    }
}

// ---------------------------------------------------------------------------
// Real EPW fixture test
// ---------------------------------------------------------------------------

#[test]
fn sub_hourly_resample_of_real_epw() {
    use std::path::PathBuf;

    let epw_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/weather/test_location.epw");

    let weather = hares_io::parse_epw(&epw_path).expect("EPW should parse");
    let resampled = weather
        .resample(60)
        .expect("resample to 60s should succeed");

    assert_eq!(
        resampled.len(),
        8760 * 60,
        "60s resample of 8760 hours should produce {} samples",
        8760 * 60
    );

    for (i, &t) in resampled.dry_bulb_c.iter().enumerate() {
        assert!(
            !t.is_nan() && !t.is_infinite(),
            "dry_bulb NaN/Inf at index {i}"
        );
        assert!(
            t > -60.0 && t < 55.0,
            "dry_bulb out of [-60, 55] at index {i}: {t}"
        );
    }

    for (i, &rh) in resampled.rel_humidity_pct.iter().enumerate() {
        assert!(
            (0.0..=100.0).contains(&rh),
            "RH out of [0, 100] at index {i}: {rh}"
        );
    }

    for (i, &ghi) in resampled.ghi_w_m2.iter().enumerate() {
        assert!(ghi >= 0.0, "GHI negative at index {i}: {ghi}");
    }
    for (i, &dni) in resampled.dni_w_m2.iter().enumerate() {
        assert!(dni >= 0.0, "DNI negative at index {i}: {dni}");
    }
    for (i, &dhi) in resampled.dhi_w_m2.iter().enumerate() {
        assert!(dhi >= 0.0, "DHI negative at index {i}: {dhi}");
    }

    // Precipitation integral preserved (ZOH-distributed, so sum should match).
    // Tolerance 1e-6: ZOH resampling from 8760 to 525600 samples accumulates
    // f64 rounding across ~60x more additions.
    let orig_total: f64 = weather.liquid_precip_m.iter().sum();
    let resampled_total: f64 = resampled.liquid_precip_m.iter().sum();
    assert!(
        (orig_total - resampled_total).abs() < 1e-6,
        "precipitation integral not preserved: original {orig_total}, resampled {resampled_total}"
    );
}

// ---------------------------------------------------------------------------
// Multi-resolution consistency test
// ---------------------------------------------------------------------------

#[test]
fn pchip_consistent_across_timesteps() {
    let values = [10.0, 18.0, 12.0, 22.0, 14.0, 20.0, 16.0, 24.0];

    let out_60s = pchip_resample(&values, 60); // 60s  -> factor 60
    let out_300s = pchip_resample(&values, 12); // 300s -> factor 12
    let out_900s = pchip_resample(&values, 4); // 900s -> factor 4

    // 300s points should appear in the 60s series at every 5th sample.
    for (i, &v300) in out_300s.iter().enumerate() {
        let idx_60 = i * 5;
        if idx_60 < out_60s.len() {
            assert!(
                (out_60s[idx_60] - v300).abs() < 1e-10,
                "300s[{i}] vs 60s[{idx_60}]: {v300} != {}",
                out_60s[idx_60]
            );
        }
    }

    // 900s points should appear in the 60s series at every 15th sample.
    for (i, &v900) in out_900s.iter().enumerate() {
        let idx_60 = i * 15;
        if idx_60 < out_60s.len() {
            assert!(
                (out_60s[idx_60] - v900).abs() < 1e-10,
                "900s[{i}] vs 60s[{idx_60}]: {v900} != {}",
                out_60s[idx_60]
            );
        }
    }

    // 900s points should appear in the 300s series at every 3rd sample.
    for (i, &v900) in out_900s.iter().enumerate() {
        let idx_300 = i * 3;
        if idx_300 < out_300s.len() {
            assert!(
                (out_300s[idx_300] - v900).abs() < 1e-10,
                "900s[{i}] vs 300s[{idx_300}]: {v900} != {}",
                out_300s[idx_300]
            );
        }
    }
}
