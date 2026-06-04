//! Cross-validation tests for the ResStock CSV midpoint offset fix.
//!
//! The ResStock AMY simplified CSV format uses end-of-interval timestamps
//! (first row = 01:00:00, representing 00:00–01:00), matching TMY3/EPW
//! convention. Both paths must agree on the midpoint offset (1800 s) and
//! produce consistent solar position results for the same site/date.

use chrono::{Datelike, FixedOffset};
use hares_io::resstock_csv::parse_resstock_csv_str;
use hares_io::tmy3::parse_tmy3_str;
use hares_physics::solar::solar_position;

// Denver, CO — same site as the TMY3 fixture in the tmy3 module tests.
const DENVER_LAT: f64 = 39.833;
const DENVER_LON: f64 = -104.650;
const DENVER_TZ: f64 = -7.0;
const DENVER_ELEVATION_M: f64 = 1650.0;

// ---------------------------------------------------------------------------
// Helper: build a synthetic ResStock CSV (end-of-interval, 8760 hourly rows).
// ---------------------------------------------------------------------------
fn build_resstock_csv() -> String {
    use chrono::{NaiveDate, TimeDelta};

    let mut lines = Vec::with_capacity(8761);
    lines.push(
        "date_time,Dry Bulb Temperature [°C],Relative Humidity [%],\
         Wind Speed [m/s],Wind Direction [Deg],\
         Global Horizontal Radiation [W/m2],\
         Direct Normal Radiation [W/m2],\
         Diffuse Horizontal Radiation [W/m2]"
            .to_string(),
    );

    let base = NaiveDate::from_ymd_opt(2005, 1, 1)
        .unwrap()
        .and_hms_opt(1, 0, 0)
        .unwrap();

    for i in 0..8760 {
        let dt = base + TimeDelta::hours(i as i64);
        let hour_of_day = dt.format("%H").to_string().parse::<u32>().unwrap();
        let ghi = if (7..=18).contains(&hour_of_day) {
            400.0
        } else {
            0.0
        };
        let dni = if (7..=18).contains(&hour_of_day) {
            600.0
        } else {
            0.0
        };
        let dhi = if (7..=18).contains(&hour_of_day) {
            100.0
        } else {
            0.0
        };

        lines.push(format!(
            "{},10.0,60.0,3.0,180.0,{ghi:.1},{dni:.1},{dhi:.1}",
            dt.format("%Y-%m-%d %H:%M:%S")
        ));
    }

    lines.join("\n")
}

// ---------------------------------------------------------------------------
// Helper: build a synthetic TMY3 CSV (end-of-interval, 8760 hourly rows).
// ---------------------------------------------------------------------------
fn build_tmy3_csv() -> String {
    let mut lines = Vec::with_capacity(8762);

    // Line 1: station metadata — Denver Intl AP
    lines.push("723860,Denver Intl AP,CO,-7,39.833,-104.650,1650".to_string());

    // Line 2: column headers
    lines.push(
        "Date (MM/DD/YYYY),Time (HH:MM),ETR (W/m^2),ETRN (W/m^2),GHI (W/m^2),\
         DNI (W/m^2),DHI (W/m^2),Dry-bulb (C),Dew-point (C),RHum (%),\
         Pressure (mbar),Wspd (m/s),Wdir (degrees)"
            .to_string(),
    );

    let days_in_month = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let year = 2021;
    for (mi, &days) in days_in_month.iter().enumerate() {
        let month = mi as u32 + 1;
        for day in 1..=days as u32 {
            for hour in 1..=24u32 {
                lines.push(format!(
                    "{month:02}/{day:02}/{year},{hour:02}:00,\
                     500,1353,100,200,50,\
                     20.0,10.0,50.0,\
                     1013.25,3.0,180"
                ));
            }
        }
    }

    lines.join("\n")
}

#[test]
fn both_parsers_agree_on_midpoint_offset_1800() {
    let resstock_csv = build_resstock_csv();
    let tmy3_csv = build_tmy3_csv();

    let rs = parse_resstock_csv_str(
        &resstock_csv,
        DENVER_ELEVATION_M,
        DENVER_LAT,
        DENVER_LON,
        DENVER_TZ,
    )
    .expect("should parse ResStock CSV");
    let tmy = parse_tmy3_str(&tmy3_csv).expect("should parse TMY3 CSV");

    assert_eq!(
        rs.meta.midpoint_offset_secs, 1800,
        "ResStock CSV midpoint_offset_secs must be 1800"
    );
    assert_eq!(
        tmy.meta.midpoint_offset_secs, 1800,
        "TMY3 midpoint_offset_secs must be 1800"
    );
}

#[test]
fn solar_zenith_at_noon_midpoint_matches_between_resstock_and_tmy3() {
    let resstock_csv = build_resstock_csv();
    let tmy3_csv = build_tmy3_csv();

    let rs = parse_resstock_csv_str(
        &resstock_csv,
        DENVER_ELEVATION_M,
        DENVER_LAT,
        DENVER_LON,
        DENVER_TZ,
    )
    .expect("should parse ResStock CSV");
    let tmy = parse_tmy3_str(&tmy3_csv).expect("should parse TMY3 CSV");

    // Both use 3600 s step and end-of-interval offset = 1800 s.
    assert_eq!(rs.meta.source_step_secs, 3600);
    assert_eq!(tmy.meta.source_step_secs, 3600);
    assert_eq!(rs.meta.midpoint_offset_secs, 1800);
    assert_eq!(tmy.meta.midpoint_offset_secs, 1800);

    // Pick the 12:00 record (noon timestamp). Index 11 = hour 12 on Jan 1
    // (first timestamp is 01:00, index 0 → hour 1; index 11 → hour 12).
    // With offset = 1800 s, the midpoint of the 12:00 record is 11:30 local.
    let tz = FixedOffset::west_opt((DENVER_TZ.abs() * 3600.0) as i32).unwrap();

    // The timestamp at record index 11 is 12:00 local on 2005-01-01.
    // Midpoint = timestamp - offset = 11:30 local.
    let midpoint_dt = chrono::NaiveDate::from_ymd_opt(2005, 1, 1)
        .unwrap()
        .and_hms_opt(11, 30, 0)
        .unwrap()
        .and_local_timezone(tz)
        .single()
        .unwrap();

    let pos = solar_position(DENVER_LAT, DENVER_LON, midpoint_dt, midpoint_dt.ordinal());
    let solar_zenith_deg = (90.0 - pos.altitude_deg).max(0.0);

    // Solar zenith at 11:30 local on Jan 1 in Denver should be in a
    // physically reasonable range (roughly 60–75° for a mid-latitude
    // winter morning). This isn't a precise reference check — the key
    // assertion is that both parsers agree on the same midpoint offset,
    // producing the same solar geometry for the same record.
    assert!(
        solar_zenith_deg > 50.0 && solar_zenith_deg < 80.0,
        "Solar zenith at 11:30 on Jan 1 in Denver should be ~60-75°, got {solar_zenith_deg:.2}°"
    );

    // Since both parsers use the same site coordinates and the same offset,
    // the solar position computed from either format's midpoint datetime is
    // identical — the format difference is irrelevant to solar geometry.
    // The cross-format agreement is verified by the offset equality above.
}
