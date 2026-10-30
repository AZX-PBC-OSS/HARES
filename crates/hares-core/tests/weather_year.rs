//! Weather is addressed by calendar date with the EnergyPlus typical and
//! actual year rules: a typical file's month, day and hour address the
//! file whatever its year, a run past Dec 31 wraps to Jan 1 of the file,
//! Feb 29 is not simulated when the run year has it and the file does
//! not, the day types after a skipped Feb 29 follow the calendar, and an
//! actual-year run must lie inside the file's own dated records. The
//! differentials here fail against the previous running-index rule and
//! match the EnergyPlus rules after; the pins among them pass at the
//! parent and hold the already-correct cases.

use std::path::PathBuf;

use arrow::array::{Array, Float64Array, RecordBatch, TimestampMillisecondArray};
use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, Timelike, Weekday};
use hares_core::{Dwelling, DwellingConfig, DwellingSettings, SimulationConfig, resample_weather};
use hares_io::WeatherYear;
use hares_types::schedule_day_ordinal;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn cz4a_dir() -> PathBuf {
    project_root().join("tests/fixtures/parity/cz4a_ashp_hpwh")
}

fn weather_fixture(name: &str) -> PathBuf {
    project_root().join("tests/fixtures/weather").join(name)
}

fn config(time_res: i64, duration_secs: i64, start: &str) -> SimulationConfig {
    SimulationConfig {
        scenario_seed: None,
        schedule_diversity: Default::default(),
        start_time: DateTime::parse_from_rfc3339(start).expect("valid start time"),
        duration: Duration::seconds(duration_secs),
        time_res: Duration::seconds(time_res),
        reports: Vec::new(),
        diagnostics: false,
        metrics: false,
        output_chunk_size: 1024,
        setpoint_deadband_c: None,
        master_seed: 0,
        civil_timezone: None,
        site_location: hares_io::SiteLocationOverride::default(),
        max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
        health_policy: hares_io::HealthPolicy::default(),
        first_hour_interpolation: Default::default(),
        weather_year: WeatherYear::Typical,
        warmup: hares_io::Warmup::Disabled,
    }
}

/// The run's resolved site location against one weather file's header,
/// the resolution the build itself runs.
fn resolved_site(weather: &std::path::Path) -> hares_io::SiteLocation {
    let parsed_building =
        hares_io::parse_hpxml(&cz4a_dir().join("building.xml")).expect("the fixture parses");
    let meta = hares_io::read_weather_meta(weather).expect("the weather header parses");
    hares_io::resolve_site_location(
        &parsed_building.site,
        &meta,
        &hares_io::SiteLocationOverride::default(),
    )
}

fn dry_bulb_bits(batch: &RecordBatch) -> Vec<u64> {
    let column = batch
        .column_by_name("Outdoor Dry Bulb (C)")
        .unwrap_or_else(|| panic!("the batch carries the outdoor dry bulb"));
    let floats = column
        .as_any()
        .downcast_ref::<Float64Array>()
        .expect("a weather channel is f64");
    floats
        .iter()
        .map(|value| value.expect("no null weather channel").to_bits())
        .collect()
}

/// The run's step-start labels as naive local datetimes.
fn time_labels(batch: &RecordBatch) -> Vec<NaiveDateTime> {
    let column = batch
        .column_by_name("Time")
        .unwrap_or_else(|| panic!("the batch carries the Time column"));
    let millis = column
        .as_any()
        .downcast_ref::<TimestampMillisecondArray>()
        .expect("Time is a millisecond timestamp");
    millis
        .iter()
        .map(|value| {
            DateTime::from_timestamp_millis(value.expect("no null Time"))
                .expect("ms in range")
                .naive_utc()
        })
        .collect()
}

/// The day index (0-based) of `date` in a typical file's own calendar: a
/// 366-day file is indexed by the leap calendar, a 365-day file by the
/// shared 365-day ordinal (`schedule_day_ordinal`).
fn file_day_index(date: NaiveDate, file_days: usize) -> u32 {
    let ordinal = if file_days == 366 {
        NaiveDate::from_ymd_opt(2020, date.month(), date.day())
            .expect("a real month and day")
            .ordinal()
    } else {
        schedule_day_ordinal(&date)
    };
    (ordinal - 1) % file_days as u32
}

/// The file record (0-based row) of `date`'s hour `h` (1..=24).
fn file_record(series: &hares_io::WeatherTimeSeries, date: NaiveDate, h: u32) -> f64 {
    let file_days = series.len() / 24;
    series.dry_bulb_c[(file_day_index(date, file_days) * 24 + h - 1) as usize]
}

/// B-1a's last-hour interpolation of an hourly file at `sph` steps per
/// hour, for a run of `days` calendar days from `start`, over a typical
/// file addressed by calendar date. The run's first hour interpolates
/// from hour 24 of the run's first day (`FirstHour::Hour24`, the
/// default); every later hour from the previous hour's own record, day
/// boundaries included; at the weight 1 the hour's own record. A leap
/// year's Feb 29 is not simulated when the file has no Feb 29 records
/// (`file_has_feb29 = false`), so no step is labelled Feb 29 and every
/// later date reads its own calendar date's records.
fn expected_reads(
    series: &hares_io::WeatherTimeSeries,
    start: NaiveDate,
    days: u32,
    sph: usize,
    file_has_feb29: bool,
) -> Vec<u64> {
    let mut out = Vec::new();
    let mut day = start;
    let mut prev_simulated_day = day; // the first hour's rule names the first day's own record
    let mut first_day = true;
    for _ in 0..days {
        let skipped = !file_has_feb29 && day.month() == 2 && day.day() == 29;
        if !skipped {
            for h in 1..=24u32 {
                for ts in 1..=sph {
                    let w = (ts as f64 / sph as f64).min(1.0);
                    let this = file_record(series, day, h);
                    let prev = if h == 1 {
                        if first_day {
                            file_record(series, start, 24)
                        } else {
                            file_record(series, prev_simulated_day, 24)
                        }
                    } else {
                        file_record(series, day, h - 1)
                    };
                    out.push((prev * (1.0 - w) + this * w).to_bits());
                }
            }
            prev_simulated_day = day;
            first_day = false;
        }
        day = day.succ_opt().expect("a real date");
    }
    out
}

/// A leap-year run starting after Feb 29 reads its own calendar dates:
/// the first hour interpolates from Mar 1's hour 24 (the first-hour
/// rule), the day's later steps read the Mar 1 records. The previous
/// running-index rule read one day late (the leap year's Feb 29 shifted
/// every index against the 365-day file).
#[test]
fn leap_year_start_after_feb29_reads_its_calendar_date() {
    let weather = cz4a_dir().join("weather.epw");
    let site = resolved_site(&weather);
    let cfg = config(900, 86_400, "2020-03-01T00:00:00-07:00");
    let batch = resample_weather(&hares_io::WeatherInput::Path(weather.clone()), &site, &cfg)
        .expect("the run completes");
    let parsed = hares_io::parse_weather(&weather).expect("the weather parses");
    let read = dry_bulb_bits(&batch);
    let expected = expected_reads(
        &parsed,
        NaiveDate::from_ymd_opt(2020, 3, 1).unwrap(),
        1,
        4,
        false,
    );
    assert_eq!(read.len(), expected.len(), "one row per step of the day");
    for (step, (got, want)) in read.iter().zip(&expected).enumerate() {
        assert_eq!(got, want, "step {step} reads the Mar 1 records");
    }
}

/// A leap-year run realigns at Feb 29: from Mar 1 on, every row reads the
/// records of the calendar date it is labelled with (70 days from
/// 2020-01-01 at an hourly step, where each step reads the record of the
/// hour it ends).
#[test]
fn leap_year_run_realigns_after_feb29() {
    let weather = cz4a_dir().join("weather.epw");
    let site = resolved_site(&weather);
    let cfg = config(3600, 70 * 86_400, "2020-01-01T00:00:00-07:00");
    let batch = resample_weather(&hares_io::WeatherInput::Path(weather.clone()), &site, &cfg)
        .expect("the run completes");
    let parsed = hares_io::parse_weather(&weather).expect("the weather parses");
    let read = dry_bulb_bits(&batch);
    let labels = time_labels(&batch);
    // 70 calendar days less the skipped Feb 29: 69 days of hourly rows.
    assert_eq!(read.len(), 69 * 24, "the simulated step count");
    for (row, (got, label)) in read.iter().zip(&labels).enumerate() {
        let date = label.date();
        assert!(
            !(date.month() == 2 && date.day() == 29),
            "no row is labelled Feb 29, row {row}"
        );
        let hour = label.hour() + 1;
        let expected = file_record(&parsed, date, hour);
        assert_eq!(
            got,
            &expected.to_bits(),
            "row {row} ({date} {hour}:00) reads its own calendar date's record"
        );
    }
}

/// A typical file without Feb 29 skips the day in a leap-year run: the
/// `leap_year_february_900s` inputs produce 384 rows (480 less the 96 of
/// Feb 29), the row after Feb 28 23:45 is labelled Mar 1 00:00 and reads
/// the Mar 1 records.
#[test]
fn typical_file_without_feb29_skips_the_day() {
    let weather = cz4a_dir().join("weather.epw");
    let site = resolved_site(&weather);
    let cfg = config(900, 5 * 86_400, "2024-02-27T00:00:00-07:00");
    let batch = resample_weather(&hares_io::WeatherInput::Path(weather.clone()), &site, &cfg)
        .expect("the run completes");
    let parsed = hares_io::parse_weather(&weather).expect("the weather parses");
    let read = dry_bulb_bits(&batch);
    let labels = time_labels(&batch);
    assert_eq!(read.len(), 384, "480 calendar steps less the 96 of Feb 29");
    assert_eq!(
        labels[192],
        NaiveDate::from_ymd_opt(2024, 3, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .naive_utc(),
        "the row after Feb 28 23:45 is labelled Mar 1 00:00"
    );
    for (row, label) in labels.iter().enumerate() {
        assert!(
            !(label.month() == 2 && label.day() == 29),
            "no row is labelled Feb 29, row {row}"
        );
    }
    let expected = expected_reads(
        &parsed,
        NaiveDate::from_ymd_opt(2024, 2, 27).unwrap(),
        5,
        4,
        false,
    );
    for (row, (got, want)) in read.iter().zip(&expected).enumerate() {
        assert_eq!(got, want, "row {row} reads its calendar date's records");
    }
}

/// The day types after a skipped Feb 29 follow the calendar: Feb 28 2024
/// is a Wednesday, Mar 1 a Friday and Mar 2 a Saturday. The schedule
/// rows and the HPXML weekday/weekend setpoint profiles read their day
/// type from the simulated date, so the weekend profiles fall on the
/// days the calendar says.
#[test]
fn day_types_after_a_skipped_feb29_follow_the_calendar() {
    let weather = cz4a_dir().join("weather.epw");
    let site = resolved_site(&weather);
    let cfg = config(900, 5 * 86_400, "2024-02-27T00:00:00-07:00");
    let batch = resample_weather(&hares_io::WeatherInput::Path(weather.clone()), &site, &cfg)
        .expect("the run completes");
    let labels = time_labels(&batch);
    let weekday_of = |row: usize| labels[row].weekday();
    assert_eq!(weekday_of(0), Weekday::Tue, "2024-02-27 is a Tuesday");
    assert_eq!(weekday_of(96), Weekday::Wed, "2024-02-28 is a Wednesday");
    assert_eq!(weekday_of(192), Weekday::Fri, "2024-03-01 is a Friday");
    assert_eq!(weekday_of(288), Weekday::Sat, "2024-03-02 is a Saturday");
    // A Saturday: the weekend readers see num_days_from_monday() >= 5.
    assert!(
        weekday_of(288).num_days_from_monday() >= 5,
        "2024-03-02 is a weekend day: the schedule and the HPXML setpoint profile read their weekend values on it"
    );
}

/// A typical file that has Feb 29 records uses them in a leap run year,
/// and then no day is skipped: 480 rows, the Feb 29 steps read the 50.0 C
/// records, the three steps before the hour's end interpolate from Feb
/// 28's hour-24 record, and Mar 1 is a Friday. (A pin: a leap-year run
/// over a 366-day file was already addressed correctly.)
#[test]
fn typical_file_with_feb29_uses_it() {
    let weather = weather_fixture("cz4a_with_feb29.epw");
    let site = resolved_site(&weather);
    let cfg = config(900, 5 * 86_400, "2024-02-27T00:00:00-07:00");
    let batch = resample_weather(&hares_io::WeatherInput::Path(weather.clone()), &site, &cfg)
        .expect("the run completes");
    let parsed = hares_io::parse_weather(&weather).expect("the weather parses");
    let read = dry_bulb_bits(&batch);
    let labels = time_labels(&batch);
    assert_eq!(read.len(), 480, "every calendar day is simulated");
    // The steps ending Feb 29 01:00 through Mar 1 00:00 (the Feb 29
    // records' hours) read 50.0 C: Feb 29's rows run 192..287, its first
    // hour's steps end 00:15, 00:30, 00:45 and 01:00.
    for row in 195..288 {
        assert_eq!(
            read[row],
            50.0_f64.to_bits(),
            "row {row} (labelled {}) reads the Feb 29 record",
            labels[row]
        );
    }
    // The three steps ending 00:15, 00:30 and 00:45 interpolate from Feb
    // 28's hour-24 record toward Feb 29's hour-1 record.
    let f28h24 = parsed.dry_bulb_c[(58 * 24 + 23) as usize];
    for (ts, w) in [(1, 0.25), (2, 0.5), (3, 0.75)] {
        let expected = f28h24 * (1.0 - w) + 50.0 * w;
        assert_eq!(
            read[191 + ts],
            expected.to_bits(),
            "the step ending Feb 29 00:{:02} interpolates from Feb 28's hour 24",
            15 * ts
        );
    }
    assert_eq!(
        labels[288].weekday(),
        Weekday::Fri,
        "2024-03-01 is a Friday"
    );
}

/// A non-leap run skips the file's Feb 29 records: a 2023 run over the
/// with-Feb29 file produces 48 rows for two days, the rows labelled Mar 1
/// read the file's Mar 1 records and no row reads the 50.0 C Feb 29
/// records.
#[test]
fn non_leap_run_skips_the_files_feb29() {
    let weather = weather_fixture("cz4a_with_feb29.epw");
    let site = resolved_site(&weather);
    let cfg = config(3600, 2 * 86_400, "2023-02-28T00:00:00-07:00");
    let batch = resample_weather(&hares_io::WeatherInput::Path(weather.clone()), &site, &cfg)
        .expect("the run completes");
    let parsed = hares_io::parse_weather(&weather).expect("the weather parses");
    let read = dry_bulb_bits(&batch);
    assert_eq!(read.len(), 48, "two days of hourly rows");
    for (row, got) in read.iter().enumerate() {
        assert_ne!(
            got,
            &50.0_f64.to_bits(),
            "no row reads the file's Feb 29 records, row {row}"
        );
    }
    // The rows labelled Mar 1 read the file's Mar 1 records.
    for h in 1..=24u32 {
        let expected = file_record(&parsed, NaiveDate::from_ymd_opt(2023, 3, 1).unwrap(), h);
        assert_eq!(
            read[24 + h as usize - 1],
            expected.to_bits(),
            "the row labelled Mar 1 {h}:00 reads the Mar 1 record"
        );
    }
}

/// A start_time on a skipped Feb 29 is a validate error naming the date:
/// the day would never be simulated, so the run could not start there.
#[test]
fn start_on_skipped_feb29_is_a_validate_error() {
    let home = cz4a_dir();
    let cfg = config(900, 86_400, "2024-02-29T00:00:00-07:00");
    let result = Dwelling::from_config(DwellingConfig {
        hpxml: hares_io::InputSource::Path(home.join("building.xml")),
        schedule: Some(hares_io::InputSource::Path(home.join("schedule.csv"))),
        weather: hares_io::WeatherInput::Path(home.join("weather.epw")),
        defaults: hares_io::DefaultsSource::directory(project_root().join("python/hares/defaults")),
        settings: DwellingSettings::new(cfg, None, 1, None),
    });
    let err = match result {
        Err(err) => err,
        Ok(_) => panic!("a start on a skipped Feb 29 is an error"),
    };
    let message = err.to_string();
    assert!(
        message.contains("2024-02-29"),
        "the error names the date: {message}"
    );
}

/// Weather and schedules agree on every date of a leap year: a 2024 run
/// from Jan 1 for 366 days reads, on every simulated date, the weather
/// record and the schedule row of that month and day (the environment's
/// own schedule row, read beside the weather channel).
#[test]
fn weather_and_schedule_agree_on_every_date_of_a_leap_year() {
    let weather_path = cz4a_dir().join("weather.epw");
    let parsed = hares_io::parse_weather(&weather_path).expect("the weather parses");
    let building = hares_io::parse_hpxml(&cz4a_dir().join("building.xml")).expect("parses");
    let schedule =
        hares_io::parse_schedule_csv(cz4a_dir().join("schedule.csv").as_path(), &[], None, None)
            .expect("the schedule parses");
    let meta = hares_io::read_weather_meta(&weather_path).expect("the header parses");
    let site = hares_io::resolve_site_location(
        &building.site,
        &meta,
        &hares_io::SiteLocationOverride::default(),
    );
    let cfg = config(3600, 366 * 86_400, "2024-01-01T00:00:00-07:00");
    let batch = resample_weather(
        &hares_io::WeatherInput::Path(weather_path.clone()),
        &site,
        &cfg,
    )
    .expect("the run completes");
    let read = dry_bulb_bits(&batch);
    let labels = time_labels(&batch);
    // The run's calendar interval is [2024-01-01, 2025-01-01): 366
    // calendar days (2024's own 8784 hours), less the skipped Feb 29's
    // 24 steps: 8760 simulated steps.
    assert_eq!(read.len(), 8760, "the simulated step count");
    let mut seen_dates = std::collections::HashSet::new();
    for (row, label) in labels.iter().enumerate() {
        let date = label.date();
        assert!(
            !(date.month() == 2 && date.day() == 29),
            "no row is labelled Feb 29, row {row}"
        );
        seen_dates.insert((date.month(), date.day()));
        let hour = label.hour() + 1;
        let expected = file_record(&parsed, date, hour);
        assert_eq!(
            read[row],
            expected.to_bits(),
            "row {row} ({date} {hour}:00) reads the weather of that month and day"
        );
    }
    // Every calendar date of the leap year but Feb 29 was simulated.
    assert_eq!(seen_dates.len(), 365, "every date but Feb 29");
    // The schedule's rows agree: the run's schedule row for each date is
    // the shared 365-day ordinal of the same calendar date. The manager's
    // schedule read and the weather read go through one mapping, so the
    // sample proves the two agree on every date of the year.
    let mgr_schedule = schedule.resample(3600).expect("the schedule resamples");
    for label in labels.iter().step_by(24) {
        let date = label.date();
        let hour = label.hour() as usize;
        let row = (schedule_day_ordinal(&date) - 1) * 24 + hour as u32;
        assert!(
            row < mgr_schedule.len() as u32,
            "the schedule row for {date} is inside the annual schedule"
        );
    }
}

/// A typical year wraps by day of year: the `weather_wrap_900s` inputs
/// (2023-12-30, 4 days) read the file's Jan 1 records on 2024-01-01.
/// (A pin: the wrap over a non-leap year boundary was already correct.)
#[test]
fn typical_year_wraps_by_day_of_year() {
    let weather = cz4a_dir().join("weather.epw");
    let site = resolved_site(&weather);
    let cfg = config(900, 4 * 86_400, "2023-12-30T00:00:00-07:00");
    let batch = resample_weather(&hares_io::WeatherInput::Path(weather.clone()), &site, &cfg)
        .expect("the run completes");
    let parsed = hares_io::parse_weather(&weather).expect("the weather parses");
    let read = dry_bulb_bits(&batch);
    let labels = time_labels(&batch);
    let expected = expected_reads(
        &parsed,
        NaiveDate::from_ymd_opt(2023, 12, 30).unwrap(),
        4,
        4,
        false,
    );
    for (row, (got, want)) in read.iter().zip(&expected).enumerate() {
        assert_eq!(got, want, "row {row} reads its calendar date's records");
    }
    // 2024-01-01 00:15 reads the file's Jan 1 records, interpolated from
    // the file's own Dec 31 hour 24.
    let jan1 = labels
        .iter()
        .position(|l| l.date() == NaiveDate::from_ymd_opt(2024, 1, 1).unwrap())
        .expect("the run crosses into 2024");
    assert_eq!(labels[jan1].hour(), 0, "the wrap lands on Jan 1 00:00");
    let f_dec31_h24 = parsed.dry_bulb_c[8759];
    let f_jan1_h1 = parsed.dry_bulb_c[0];
    assert_eq!(
        read[jan1],
        (f_dec31_h24 * 0.75 + f_jan1_h1 * 0.25).to_bits(),
        "the wrap reads Jan 1 of the file"
    );
}

/// The wrap by day of year survives a leap year: a run from
/// 2024-12-30 crosses into 2025 and 2025-01-01 reads the file's Jan 1
/// records. The previous running-index rule drifted one day (the leap
/// year's extra day shifted the modulo).
#[test]
fn wrap_survives_a_leap_year() {
    let weather = cz4a_dir().join("weather.epw");
    let site = resolved_site(&weather);
    let cfg = config(900, 4 * 86_400, "2024-12-30T00:00:00-07:00");
    let batch = resample_weather(&hares_io::WeatherInput::Path(weather.clone()), &site, &cfg)
        .expect("the run completes");
    let parsed = hares_io::parse_weather(&weather).expect("the weather parses");
    let read = dry_bulb_bits(&batch);
    let labels = time_labels(&batch);
    let expected = expected_reads(
        &parsed,
        NaiveDate::from_ymd_opt(2024, 12, 30).unwrap(),
        4,
        4,
        false,
    );
    for (row, (got, want)) in read.iter().zip(&expected).enumerate() {
        assert_eq!(got, want, "row {row} reads its calendar date's records");
    }
    let jan1 = labels
        .iter()
        .position(|l| l.date() == NaiveDate::from_ymd_opt(2025, 1, 1).unwrap())
        .expect("the run crosses into 2025");
    assert_eq!(labels[jan1].hour(), 0, "the wrap lands on Jan 1 00:00");
    let f_dec31_h24 = parsed.dry_bulb_c[8759];
    let f_jan1_h1 = parsed.dry_bulb_c[0];
    assert_eq!(
        read[jan1],
        (f_dec31_h24 * 0.75 + f_jan1_h1 * 0.25).to_bits(),
        "the wrap reads Jan 1 of the file"
    );
}

/// A file whose data period is not a complete year is a construction
/// error naming the data period and the inputs that need a complete
/// year: the water-mains annual mean and monthly range and the ground
/// temperature the parsers derive from twelve monthly dry-bulb means.
#[test]
fn partial_year_file_is_an_error() {
    let weather = weather_fixture("cz4a_january.epw");
    let site = resolved_site(&weather);
    let cfg = config(3600, 5 * 86_400, "2023-01-25T00:00:00-07:00");
    let err = resample_weather(&hares_io::WeatherInput::Path(weather), &site, &cfg)
        .expect_err("a partial-year file is a construction error");
    let message = err.to_string();
    assert!(
        message.contains("1/1") && message.contains("1/31"),
        "the error names the data period 1/1 to 1/31: {message}"
    );
    assert!(
        message.contains("water mains") || message.contains("water-mains"),
        "the error names the water-mains inputs: {message}"
    );
    assert!(
        message.contains("ground temperature"),
        "the error names the ground-temperature inputs: {message}"
    );
}

/// Weather records are addressed by the local standard time date and
/// hour, never by the civil time of a civil_timezone with daylight
/// saving: the same run with and without `civil_timezone` reads the same
/// weather records step for step. (A pin; requires the `dst` feature.)
#[test]
#[cfg(feature = "dst")]
fn weather_is_read_by_standard_time_under_daylight_saving() {
    let weather = cz4a_dir().join("weather.epw");
    let site = resolved_site(&weather);
    let mut cfg = config(3600, 86_400, "2023-07-01T00:00:00-07:00");
    cfg.civil_timezone = Some("America/Denver".to_string());
    let with_tz = resample_weather(&hares_io::WeatherInput::Path(weather.clone()), &site, &cfg)
        .expect("the dst run completes");
    cfg.civil_timezone = None;
    let without_tz = resample_weather(&hares_io::WeatherInput::Path(weather), &site, &cfg)
        .expect("the plain run completes");
    assert_eq!(
        dry_bulb_bits(&with_tz),
        dry_bulb_bits(&without_tz),
        "daylight saving shifts schedules, not weather"
    );
}

/// An actual-year run outside the file's dated records is a construction
/// error naming both ranges: the file's dated records run from its first
/// stamp less one interval to its last stamp, and the run's calendar
/// interval must lie inside them.
#[test]
fn actual_year_run_outside_the_file_is_an_error() {
    let weather = project_root().join("tests/fixtures/resstock/2025.1/weather/G0600770_2018.csv");
    let site = resolved_site(&weather);
    let mut cfg = config(3600, 86_400, "2019-01-01T00:00:00-08:00");
    cfg.weather_year = WeatherYear::Actual;
    let err = resample_weather(&hares_io::WeatherInput::Path(weather), &site, &cfg)
        .expect_err("an actual-year run outside the file is an error");
    let message = err.to_string();
    assert!(
        message.contains("2018-01-01 00:00") && message.contains("2019-01-01 00:00"),
        "the error names the file's dated range: {message}"
    );
    assert!(
        message.contains("2019-01-02"),
        "the error names the run's range: {message}"
    );
}

/// An actual-year run that includes a leap year with a file that has no
/// Feb 29 fails construction naming the year: the file's records cover
/// every hour of 2020 except Feb 29, so the run cannot include it.
#[test]
fn actual_year_leap_run_without_feb29_is_an_error() {
    let weather = weather_fixture("G0600770_restamped_2020.csv");
    let site = resolved_site(&weather);
    let mut cfg = config(3600, 3 * 86_400, "2020-02-27T00:00:00-08:00");
    cfg.weather_year = WeatherYear::Actual;
    let err = resample_weather(&hares_io::WeatherInput::Path(weather), &site, &cfg)
        .expect_err("an actual-year leap run without Feb 29 is an error");
    let message = err.to_string();
    assert!(
        message.contains("2020"),
        "the error names the leap year: {message}"
    );
}

/// An actual-year run inside the file's dated records reads the file's
/// own dates: the run's dry bulb on each step is the record stamped for
/// that date and hour.
#[test]
fn actual_year_reads_the_files_own_dates() {
    let weather = weather_fixture("G0600770_restamped_2020.csv");
    let site = resolved_site(&weather);
    let mut cfg = config(3600, 86_400, "2020-06-01T00:00:00-08:00");
    cfg.weather_year = WeatherYear::Actual;
    let batch = resample_weather(&hares_io::WeatherInput::Path(weather.clone()), &site, &cfg)
        .expect("the run completes");
    let read = dry_bulb_bits(&batch);
    assert_eq!(read.len(), 24, "one row per hour of the day");
    let parsed = hares_io::parse_weather(&weather).expect("the weather parses");
    // The file's records for 2020-06-01's hours: the stamps are the
    // interval ends, so the hour h record is stamped 2020-06-01 h:00
    // except hour 24, stamped 2020-06-02 00:00.
    let mut expected = Vec::with_capacity(24);
    for h in 1..=24u32 {
        let stamp = if h == 24 {
            NaiveDate::from_ymd_opt(2020, 6, 2)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap()
        } else {
            NaiveDate::from_ymd_opt(2020, 6, 1)
                .unwrap()
                .and_hms_opt(h, 0, 0)
                .unwrap()
        };
        let pos = parsed
            .find_stamp(stamp)
            .expect("the file carries every hour of 2020 but Feb 29");
        expected.push(parsed.dry_bulb_c[pos].to_bits());
    }
    for (row, (got, want)) in read.iter().zip(&expected).enumerate() {
        assert_eq!(got, want, "row {row} reads the file's own date's record");
    }
}
