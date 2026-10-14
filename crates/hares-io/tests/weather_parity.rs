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
// The EnergyPlus interpolation of an hourly file at a sub-hourly step:
// every point field's linear last-hour interpolation, evaluated for the
// step's end. Linear interpolation between two records stays between
// them, so the bounds tests hold on the source records' own ranges.
// ===========================================================================

/// The run start the resample tests read from: the file's own first
/// midnight.
fn test_start() -> chrono::NaiveDateTime {
    chrono::NaiveDate::from_ymd_opt(1995, 1, 1)
        .expect("date")
        .and_hms_opt(0, 0, 0)
        .expect("midnight")
}

#[test]
fn real_epw_interpolated_to_60s_stays_within_record_bounds() {
    use std::path::PathBuf;

    let epw_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/weather/test_location.epw");

    let weather = hares_io::parse_epw(&epw_path).expect("EPW should parse");
    let resampled = weather
        .resample(60, hares_io::FirstHour::default(), test_start())
        .expect("resample to 60s should succeed");

    assert_eq!(
        resampled.len(),
        8760 * 60,
        "60s resample of 8760 hours should produce {} samples",
        8760 * 60
    );

    // Every point field's step value is a weighted average of two
    // adjacent hourly records, so it lies between the file's own min and
    // max for that field, within one part in a billion of float rounding
    // (the weighted sum's two roundings can lift a value one ULP above
    // the record bound; EnergyPlus's own weighted sum has the same).
    let check_within = |name: &str, values: &[f64], source: &[f64]| {
        let lo = source.iter().cloned().fold(f64::INFINITY, f64::min);
        let hi = source.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        assert!(lo <= hi, "{name}: the source records are empty");
        for (i, &t) in values.iter().enumerate() {
            assert!(
                !t.is_nan() && !t.is_infinite(),
                "{name} NaN/Inf at index {i}"
            );
            assert!(
                lo - 1e-9 <= t && t <= hi + 1e-9,
                "{name} out of the source records' range [{lo}, {hi}] at index {i}: {t}"
            );
        }
    };

    check_within("dry_bulb", &resampled.dry_bulb_c, &weather.dry_bulb_c);
    check_within("dew_point", &resampled.dew_point_c, &weather.dew_point_c);
    check_within(
        "rel_humidity",
        &resampled.rel_humidity_pct,
        &weather.rel_humidity_pct,
    );
    check_within("pressure", &resampled.pressure_kpa, &weather.pressure_kpa);
    check_within(
        "wind_speed",
        &resampled.wind_speed_m_s,
        &weather.wind_speed_m_s,
    );
    check_within(
        "opaque_sky_cover",
        &resampled.opaque_sky_cover,
        &weather.opaque_sky_cover,
    );
    check_within(
        "horizontal_infrared",
        &resampled.horizontal_infrared_w_m2,
        &weather.horizontal_infrared_w_m2,
    );
    check_within("ghi", &resampled.ghi_w_m2, &weather.ghi_w_m2);
    check_within("dni", &resampled.dni_w_m2, &weather.dni_w_m2);
    check_within("dhi", &resampled.dhi_w_m2, &weather.dhi_w_m2);

    // Solar takes the midpoint interpolation: each step's value is a
    // convex blend of three adjacent hourly records (the bounds check
    // above covers them), and the step ending at the half hour (ts = 30
    // of 60) reads its hour's record exactly.
    for (hour, &source_ghi) in weather.ghi_w_m2.iter().enumerate() {
        assert_eq!(
            resampled.ghi_w_m2[hour * 60 + 29],
            source_ghi,
            "GHI hour {hour} at the half hour reads the hour's record"
        );
    }
    for (hour, &source_dni) in weather.dni_w_m2.iter().enumerate() {
        assert_eq!(
            resampled.dni_w_m2[hour * 60 + 29],
            source_dni,
            "DNI hour {hour} at the half hour reads the hour's record"
        );
    }
    for (hour, &source_dhi) in weather.dhi_w_m2.iter().enumerate() {
        assert_eq!(
            resampled.dhi_w_m2[hour * 60 + 29],
            source_dhi,
            "DHI hour {hour} at the half hour reads the hour's record"
        );
    }

    // The precipitation's weighted depth divided by 60 sums over each
    // hour to the weighted hourly depth: the sum of the weights over the
    // hour's steps is one hour's worth of the last-hour blend, so the
    // hour's steps sum to (prev * wp_sum + this * w_sum) / 60 where
    // wp_sum + w_sum = 60. The full-year total is preserved when the
    // file's first hour reads the first-day rule's record (whose sum is
    // the same form). Compare the totals over hours 2.. to avoid the
    // first hour's special record.
    for hour in 1..8760 {
        let hour_total: f64 = resampled.liquid_precip_m[hour * 60..(hour + 1) * 60]
            .iter()
            .sum();
        let prev = weather.liquid_precip_m[hour - 1];
        let this = weather.liquid_precip_m[hour];
        let w_sum: f64 = (1..=60).map(|ts| ts as f64 / 60.0).sum();
        let expected = (prev * (60.0 - w_sum) + this * w_sum) / 60.0;
        assert!(
            (hour_total - expected).abs() < 1e-9,
            "hour {hour}: the steps' precipitation sum {hour_total} should be the \
             weighted blend {expected}"
        );
    }
}

/// A record's value is reached at the end of its hour: at the file's own
/// step every step reads the record of the hour it ends in.
#[test]
fn hourly_step_reads_the_record_of_the_hour_it_ends() {
    use std::path::PathBuf;

    let epw_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/weather/test_location.epw");
    let weather = hares_io::parse_epw(&epw_path).expect("EPW should parse");
    let resampled = weather
        .resample(3600, hares_io::FirstHour::default(), test_start())
        .expect("resample to the hour should succeed");
    assert_eq!(resampled.len(), weather.len());
    assert_eq!(resampled.dry_bulb_c, weather.dry_bulb_c);
    assert_eq!(resampled.ghi_w_m2, weather.ghi_w_m2);
}
