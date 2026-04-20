//! PSM3 parser integration tests.
//!
//! Tests loading from a synthetic full-year 5-minute PSM3 fixture,
//! verifying metadata extraction, unit conversions, derived quantities,
//! resolution detection, resampling invariants, and validation rejections.
//!
//! The synthetic fixture lives at `tests/fixtures/weather/synthetic_psm3_5min.csv`
//! and contains 105,120 data rows (365 days × 24 h × 12 intervals/h).

use std::io::Write;

use hares_io::{WeatherTimeSeries, parse_psm3};

/// Absolute path to the synthetic 5-minute PSM3 fixture.
fn fixture_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/weather/synthetic_psm3_5min.csv")
}

/// Load the fixture, panicking on failure.
fn load_fixture() -> WeatherTimeSeries {
    parse_psm3(fixture_path()).expect("fixture should parse without error")
}

/// Write a temporary PSM3 file from a string, returning the path.
/// The file is deleted when the returned handle is dropped.
fn write_temp_psm3(contents: &str) -> tempfile::NamedTempFile {
    let mut f = tempfile::NamedTempFile::with_suffix(".csv").expect("should create temp file");
    f.write_all(contents.as_bytes())
        .expect("should write temp file");
    f.flush().expect("should flush");
    f
}

// ---------------------------------------------------------------------------
// Parsing tests
// ---------------------------------------------------------------------------

#[test]
fn parses_synthetic_psm3_fixture() {
    let ts = load_fixture();

    // 105,120 records for a non-leap year at 5-minute resolution.
    assert_eq!(ts.len(), 105_120);
    assert_eq!(ts.meta.source_step_secs, 300);

    // Metadata extracted from the 3-line header.
    assert!((ts.meta.latitude - 39.74).abs() < 1e-6);
    assert!((ts.meta.longitude - (-104.99)).abs() < 1e-6);
    assert!((ts.meta.timezone_offset_h - (-7.0)).abs() < 1e-6);
    assert!((ts.meta.elevation_m - 1609.0).abs() < 1e-6);
    assert_eq!(ts.meta.location, "TestCity");

    // All column vectors have the same length.
    assert_eq!(ts.dry_bulb_c.len(), 105_120);
    assert_eq!(ts.dew_point_c.len(), 105_120);
    assert_eq!(ts.pressure_kpa.len(), 105_120);
    assert_eq!(ts.ghi_w_m2.len(), 105_120);
    assert_eq!(ts.sky_temp_c.len(), 105_120);
    assert_eq!(ts.ground_temp_c.len(), 105_120);

    // Temperature range check: fixture sinusoid is 15 ± 10 → [5, 25].
    let t_min = ts.dry_bulb_c.iter().cloned().fold(f64::INFINITY, f64::min);
    let t_max = ts
        .dry_bulb_c
        .iter()
        .cloned()
        .fold(f64::NEG_INFINITY, f64::max);
    assert!((4.9..=5.1).contains(&t_min), "unexpected t_min: {t_min}");
    assert!((24.9..=25.1).contains(&t_max), "unexpected t_max: {t_max}");

    // GHI range: [0, 800].
    let ghi_max = ts
        .ghi_w_m2
        .iter()
        .cloned()
        .fold(f64::NEG_INFINITY, f64::max);
    assert!(
        (799.0..=801.0).contains(&ghi_max),
        "unexpected ghi_max: {ghi_max}"
    );
    assert!(
        ts.ghi_w_m2.iter().all(|&v| v >= 0.0),
        "GHI should be non-negative"
    );
}

/// PSM3 pressure is in millibar; the parser must convert to kPa (÷ 10).
/// The fixture uses a constant 1013.25 mbar, so every value should be 101.325 kPa.
#[test]
fn psm3_pressure_converted_to_kpa() {
    let ts = load_fixture();

    for (i, &p) in ts.pressure_kpa.iter().enumerate() {
        assert!(
            (p - 101.325).abs() < 1e-6,
            "row {i}: expected 101.325 kPa, got {p}"
        );
    }
}

/// Ground temperature via the DOE-2.1E sinusoidal model.
///
/// The DOE-2.1E Engineering Manual describes ground temperature as a damped,
/// lagged sinusoidal function of monthly-average dry-bulb temperature.
/// For a synthetic fixture with a uniform daily sinusoid (mean ~15 °C),
/// all monthly averages are approximately equal, so the DOE-2 model should
/// produce ground temps very close to the annual mean with minimal swing.
///
/// This test verifies that ground temp is DIFFERENT from dry bulb -- it should
/// be damped (smaller swing) and lagged relative to air temperature.
#[test]
fn psm3_ground_temp_uses_doe2_model() {
    let ts = load_fixture();

    // With a constant daily sinusoid of 15 ± 10, every month has roughly the
    // same average (~15 °C). Compute monthly averages from the fixture data.
    let mut month_sums = [0.0_f64; 12];
    let mut month_counts = [0_u32; 12];
    // Each 5-min record is (year, month, day, hour, minute) -- we only need month.
    // The fixture has 105,120 records for a 365-day year at 300s intervals.
    let records_per_day = 24 * 12; // 288 records per day at 5-min resolution
    let days_in_month: [u32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut idx = 0_usize;
    for (mi, &ndays) in days_in_month.iter().enumerate() {
        let count = ndays as usize * records_per_day;
        for i in idx..idx + count {
            month_sums[mi] += ts.dry_bulb_c[i];
            month_counts[mi] += 1;
        }
        idx += count;
    }
    let monthly_avg: [f64; 12] =
        std::array::from_fn(|i| month_sums[i] / f64::from(month_counts[i]));

    // All monthly averages should be ~15 °C (uniform sinusoid).
    for (i, &avg) in monthly_avg.iter().enumerate() {
        assert!(
            (avg - 15.0).abs() < 0.5,
            "month {}: expected avg ~15, got {avg:.2}",
            i + 1
        );
    }

    // Ground temp swing should be much smaller than dry-bulb swing (damped).
    let g_min = ts
        .ground_temp_c
        .iter()
        .cloned()
        .fold(f64::INFINITY, f64::min);
    let g_max = ts
        .ground_temp_c
        .iter()
        .cloned()
        .fold(f64::NEG_INFINITY, f64::max);
    let t_min = ts.dry_bulb_c.iter().cloned().fold(f64::INFINITY, f64::min);
    let t_max = ts
        .dry_bulb_c
        .iter()
        .cloned()
        .fold(f64::NEG_INFINITY, f64::max);

    let ground_swing = g_max - g_min;
    let air_swing = t_max - t_min;

    // Ground swing must be strictly smaller than air swing (DOE-2 damping).
    assert!(
        ground_swing < air_swing,
        "ground temp swing ({ground_swing:.1} K) should be damped below air swing ({air_swing:.1} K)"
    );

    // With nearly identical monthly means, ground temp swing should be < 5 K.
    assert!(
        ground_swing < 5.0,
        "ground temp swing {ground_swing:.1} K is too large for uniform monthly means"
    );

    // Ground temps should be centered near the annual mean (~15 °C).
    let g_mean = ts.ground_temp_c.iter().sum::<f64>() / ts.ground_temp_c.len() as f64;
    assert!(
        (g_mean - 15.0).abs() < 1.0,
        "ground temp mean {g_mean:.2} should be near annual mean ~15 °C"
    );
}

// ---------------------------------------------------------------------------
// Resolution detection tests
// ---------------------------------------------------------------------------

/// Construct a full-year 15-minute inline CSV and verify 900s detection.
#[test]
fn psm3_detects_15min_resolution() {
    let csv = build_full_year_csv(15, false);
    let f = write_temp_psm3(&csv);
    let ts = parse_psm3(f.path()).expect("should parse 15-min PSM3");
    assert_eq!(ts.meta.source_step_secs, 900);
    assert_eq!(ts.len(), 35_040);
}

/// Construct a full-year hourly inline CSV and verify 3600s detection.
#[test]
fn psm3_detects_hourly_resolution() {
    let csv = build_full_year_csv(60, false);
    let f = write_temp_psm3(&csv);
    let ts = parse_psm3(f.path()).expect("should parse hourly PSM3");
    assert_eq!(ts.meta.source_step_secs, 3600);
    assert_eq!(ts.len(), 8_760);
}

// ---------------------------------------------------------------------------
// Resampling tests
// ---------------------------------------------------------------------------

/// When the target step matches the source, resample should return identical data.
#[test]
fn psm3_5min_no_resample_at_300s() {
    let ts = load_fixture();
    let resampled = ts.resample(300).expect("no-op resample should succeed");
    assert_eq!(resampled.len(), ts.len());
    assert_eq!(resampled.meta.source_step_secs, 300);
    assert_eq!(resampled.dry_bulb_c, ts.dry_bulb_c);
    assert_eq!(resampled.ghi_w_m2, ts.ghi_w_m2);
    assert_eq!(resampled.pressure_kpa, ts.pressure_kpa);
    assert_eq!(resampled.sky_temp_c, ts.sky_temp_c);
    assert_eq!(resampled.ground_temp_c, ts.ground_temp_c);
}

/// Downsampling from 5-min to hourly must preserve the daily solar energy integral.
///
/// For period-average irradiance, energy = mean_irradiance × duration.
/// Downsampling via mean preserves the average, so
///   sum(GHI_5min) × 300s == sum(GHI_hourly) × 3600s
/// for each 24-hour block.
#[test]
fn psm3_downsample_preserves_daily_solar_integral() {
    let ts = load_fixture();
    let hourly = ts
        .resample(3600)
        .expect("downsample to hourly should succeed");
    assert_eq!(hourly.len(), 8_760);

    // Compare total GHI energy over the full year.
    // Energy_5min = sum(GHI_5min) * 300 [W·s/m²]
    // Energy_hourly = sum(GHI_hourly) * 3600 [W·s/m²]
    let energy_5min: f64 = ts.ghi_w_m2.iter().sum::<f64>() * 300.0;
    let energy_hourly: f64 = hourly.ghi_w_m2.iter().sum::<f64>() * 3600.0;

    let rel_err = (energy_5min - energy_hourly).abs() / energy_5min.max(1.0);
    assert!(
        rel_err < 1e-8,
        "solar integral mismatch: 5min={energy_5min:.1}, hourly={energy_hourly:.1}, rel_err={rel_err:.2e}"
    );

    // Also verify per-day: pick day 1 (indices 0..288 for 5-min, 0..24 for hourly).
    let day1_5min: f64 = ts.ghi_w_m2[..288].iter().sum::<f64>() * 300.0;
    let day1_hourly: f64 = hourly.ghi_w_m2[..24].iter().sum::<f64>() * 3600.0;
    let day1_rel = (day1_5min - day1_hourly).abs() / day1_5min.max(1.0);
    assert!(
        day1_rel < 1e-8,
        "day 1 solar integral mismatch: 5min={day1_5min:.1}, hourly={day1_hourly:.1}"
    );
}

/// Sky temperature is computed via the Clark & Allen (1978) clear-sky emissivity model.
///
/// Clark & Allen (1978): T_sky_K = T_dry_K * (0.787 + 0.764 * ln(T_dew_K / 273.15))^0.25
/// PSM3 has no horizontal IR data, so the parser must always use this model.
#[test]
fn psm3_sky_temp_uses_clark_allen() {
    let ts = load_fixture();

    // Spot-check a few indices using the Clark-Allen formula.
    for &i in &[0, 1000, 50_000, 105_119] {
        let db = ts.dry_bulb_c[i];
        let dp = ts.dew_point_c[i];

        let db_k = db + 273.15;
        let dp_k = dp + 273.15;
        let expected_k = db_k * (0.787 + 0.764 * (dp_k / 273.15).ln()).powf(0.25);
        let expected_c = expected_k - 273.15;

        assert!(
            (ts.sky_temp_c[i] - expected_c).abs() < 1e-10,
            "sky_temp_c[{i}]: expected {expected_c:.6}, got {:.6}",
            ts.sky_temp_c[i]
        );
    }
}

/// Irregular resolution (not 5/15/30/60 min) must be rejected.
#[test]
fn psm3_rejects_irregular_resolution() {
    // Two rows 7 minutes apart -- not a valid PSM3 interval.
    let csv = "\
Source,Location ID,City,State,Country,Latitude,Longitude,Time Zone,Elevation,Local Time Zone
NSRDB,1,City,-,-,39.74,-104.99,-7,1609.0,-7
Year,Month,Day,Hour,Minute,GHI,DNI,DHI,Temperature,Pressure,Dew Point,Relative Humidity,Wind Speed,Wind Direction
2021,1,1,0,0,100,70,30,20.0,1013.25,10.0,50.0,3.0,180
2021,1,1,0,7,100,70,30,20.0,1013.25,10.0,50.0,3.0,180";
    let f = write_temp_psm3(csv);
    let err = parse_psm3(f.path()).expect_err("should reject 7-min step");
    assert!(
        err.to_string().contains("not one of"),
        "unexpected error: {err}"
    );
}

/// Downsampling from 5-minute to hourly resolution.
///
/// Note: PSM3 files do not contain precipitation data. The parser fills
/// `liquid_precip_m` with zeros, so the sum after downsampling is always 0.
#[test]
fn psm3_5min_downsample_to_hourly() {
    let ts = load_fixture();
    assert_eq!(ts.meta.source_step_secs, 300);

    let hourly = ts
        .resample(3600)
        .expect("downsample to hourly should succeed");
    assert_eq!(hourly.len(), 8_760);

    // Dry bulb: mean of each 12-record block.
    // Spot-check first hour: mean of ts.dry_bulb_c[0..12].
    let expected_db_mean: f64 = ts.dry_bulb_c[..12].iter().sum::<f64>() / 12.0;
    assert!(
        (hourly.dry_bulb_c[0] - expected_db_mean).abs() < 1e-10,
        "first-hour dry_bulb: expected {expected_db_mean}, got {}",
        hourly.dry_bulb_c[0]
    );

    // GHI: mean of each 12-record block.
    let expected_ghi_mean: f64 = ts.ghi_w_m2[..12].iter().sum::<f64>() / 12.0;
    assert!(
        (hourly.ghi_w_m2[0] - expected_ghi_mean).abs() < 1e-10,
        "first-hour GHI: expected {expected_ghi_mean}, got {}",
        hourly.ghi_w_m2[0]
    );

    // Precipitation: PSM3 has no precip data, so sum must be 0.
    let total_precip: f64 = hourly.liquid_precip_m.iter().sum();
    assert!(
        total_precip.abs() < 1e-15,
        "PSM3 has no precipitation; sum should be 0, got {total_precip}"
    );
}

/// Dew point exceeding dry bulb must be rejected.
#[test]
fn psm3_rejects_dewpoint_above_drybulb() {
    // Dew point (25.0) > dry bulb (20.0).
    let csv = "\
Source,Location ID,City,State,Country,Latitude,Longitude,Time Zone,Elevation,Local Time Zone
NSRDB,1,City,-,-,39.74,-104.99,-7,1609.0,-7
Year,Month,Day,Hour,Minute,GHI,DNI,DHI,Temperature,Pressure,Dew Point,Relative Humidity,Wind Speed,Wind Direction
2021,1,1,0,0,100,70,30,20.0,1013.25,25.0,50.0,3.0,180
2021,1,1,1,0,100,70,30,20.0,1013.25,25.0,50.0,3.0,180";
    let f = write_temp_psm3(csv);
    let err = parse_psm3(f.path()).expect_err("should reject dew_point > dry_bulb");
    assert!(
        err.to_string().contains("dew point"),
        "unexpected error: {err}"
    );
}

// ---------------------------------------------------------------------------
// Validation rejection tests
// ---------------------------------------------------------------------------

/// Temperature outside [-60, 55] °C must be rejected.
#[test]
fn psm3_rejects_out_of_range_temperature() {
    let csv = build_full_year_csv_with_override(60, false, Override::Temperature(60.0));
    let f = write_temp_psm3(&csv);
    let err = parse_psm3(f.path()).expect_err("should reject temp=60");
    assert!(
        err.to_string().contains("temperature out of range"),
        "unexpected error: {err}"
    );
}

/// Negative GHI must be rejected.
#[test]
fn psm3_rejects_negative_solar() {
    let csv = build_full_year_csv_with_override(60, false, Override::Ghi(-10.0));
    let f = write_temp_psm3(&csv);
    let err = parse_psm3(f.path()).expect_err("should reject GHI=-10");
    assert!(
        err.to_string().contains("GHI out of range"),
        "unexpected error: {err}"
    );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Which field to override in the first data row for validation rejection tests.
enum Override {
    Temperature(f64),
    Ghi(f64),
}

/// Build a full-year PSM3 CSV string at the given minute resolution.
fn build_full_year_csv(step_minutes: u32, is_leap: bool) -> String {
    build_full_year_csv_inner(step_minutes, is_leap, None)
}

/// Build a full-year PSM3 CSV string with a single-row override for rejection tests.
fn build_full_year_csv_with_override(step_minutes: u32, is_leap: bool, ovr: Override) -> String {
    build_full_year_csv_inner(step_minutes, is_leap, Some(ovr))
}

fn build_full_year_csv_inner(step_minutes: u32, is_leap: bool, ovr: Option<Override>) -> String {
    use std::fmt::Write;

    let days_in_month: [u32; 12] = if is_leap {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let total_days: u32 = days_in_month.iter().sum();
    let records = (total_days as usize) * 24 * (60 / step_minutes as usize);
    let year = if is_leap { 2020 } else { 2021 };

    let mut buf = String::with_capacity(records * 70 + 300);

    // 3-line header
    buf.push_str(
        "Source,Location ID,City,State,Country,Latitude,Longitude,Time Zone,Elevation,Local Time Zone\n",
    );
    buf.push_str("NSRDB,12345,TestCity,CO,US,39.74,-104.99,-7,1609.0,-7\n");
    buf.push_str(
        "Year,Month,Day,Hour,Minute,GHI,DNI,DHI,Temperature,Pressure,Dew Point,Relative Humidity,Wind Speed,Wind Direction\n",
    );

    let mut first_row = true;
    for (mi, &ndays) in days_in_month.iter().enumerate() {
        let month = mi as u32 + 1;
        for day in 1..=ndays {
            for hour in 0..24u32 {
                for slot in 0..(60 / step_minutes) {
                    let minute = slot * step_minutes;

                    let (mut temp, mut ghi) = (20.0_f64, 100.0_f64);

                    if first_row {
                        if let Some(ref o) = ovr {
                            match o {
                                Override::Temperature(v) => temp = *v,
                                Override::Ghi(v) => ghi = *v,
                            }
                        }
                        first_row = false;
                    }

                    writeln!(
                        buf,
                        "{year},{month},{day},{hour},{minute},{ghi:.1},{:.1},{:.1},{temp:.2},1013.25,10.00,50.0,3.00,180.0",
                        ghi * 0.7,
                        ghi * 0.3,
                    )
                    .expect("write should succeed");
                }
            }
        }
    }

    // Remove trailing newline to avoid an empty last line.
    if buf.ends_with('\n') {
        buf.pop();
    }

    buf
}
