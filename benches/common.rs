// Each bench binary compiles this module and uses a different subset of it.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::Duration;
use hares_core::Dwelling;
use hares_core::{DwellingConfig, SimulationConfig};

use hares_core::DwellingSettings;
use hares_io::{DefaultsSource, InputSource, WeatherInput};

#[path = "../tests/support/fixture_start.rs"]
mod fixture_start;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// `dir/<prefix>-<n>.<ext>` with a process-wide counter `n`, so every call
/// names a new file. `dir` is the benchmark's own directory, which each
/// benchmark function creates with `tempfile::tempdir()` before any timed
/// region and which is removed with everything in it when dropped.
pub fn numbered_path(dir: &Path, prefix: &str, ext: &str) -> PathBuf {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    dir.join(format!("{prefix}-{id}.{ext}"))
}

pub fn fixture_hpxml_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/hpxml/ochre_samples/base.xml")
}

/// The shipped defaults directory: the benchmark schedule has no lighting or
/// plug-load columns, so those loads take their default profiles from here.
pub fn repo_defaults_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../python/hares/defaults")
}

pub fn write_schedule_csv(path: &PathBuf) {
    // The canonical schedule column names the resolver's mapping table
    // reads: every event appliance the HPXML fixture declares needs its
    // own fraction column (the constant-window fallback for a missing
    // column is a construction error), and the file carries no setpoint
    // columns, so the HVAC setpoints come from the HPXML's own profiles.
    // The shipped defaults directory supplies the lighting and plug-load
    // profiles the file omits.
    let rows = [
        "Time,clothes_washer,clothes_dryer,dishwasher,cooking_range,hot_water_clothes_washer,hot_water_dishwasher",
        "2021-01-01T00:00:00-07:00,0.10,0.0,0.0,0.0,0.1,0.0",
        "2021-01-01T00:15:00-07:00,0.20,0.5,0.0,0.0,0.2,0.0",
        "2021-01-01T00:30:00-07:00,0.15,0.5,0.3,0.0,0.15,0.3",
        "2021-01-01T00:45:00-07:00,0.12,0.0,0.0,0.6,0.12,0.0",
    ]
    .join("\n");
    fs::write(path, rows).expect("failed to write schedule fixture");
}

pub fn write_weather_epw(path: &PathBuf) {
    use chrono::{Datelike, Duration as ChronoDuration, NaiveDate, Timelike};

    let mut lines = vec![
        "LOCATION,Benchmark,CZ4A,USA,TMY3,999999,39.74,-104.99,-7.0,1609.3".to_string(),
        "DESIGN CONDITIONS,0".to_string(),
        "GROUND TEMPERATURES,0".to_string(),
        "TYPICAL/EXTREME PERIODS,0".to_string(),
        "HOLIDAYS/DAYLIGHT SAVINGS,No,0,0,0".to_string(),
        "COMMENTS 1,synthetic benchmark weather".to_string(),
        "COMMENTS 2,synthetic benchmark weather".to_string(),
        "DATA PERIODS,1,1,Data,Sunday, 1/ 1,12/31".to_string(),
    ];

    let start_date = NaiveDate::from_ymd_opt(2021, 1, 1).expect("valid date");
    let start_time = start_date.and_hms_opt(0, 0, 0).expect("valid time");
    for i in 0..8760 {
        let timestamp = start_time + ChronoDuration::hours(i as i64);
        let year = timestamp.year();
        let month = timestamp.month();
        let day = timestamp.day();
        let hour = timestamp.hour() + 1;

        let row = [
            year.to_string(),
            month.to_string(),
            day.to_string(),
            hour.to_string(),
            "0".to_string(),
            "A0A0A0A0*0*0*0*0*0*0*0*0*0*0".to_string(),
            "19.0".to_string(),
            "10.0".to_string(),
            "50".to_string(),
            "101325".to_string(),
            "0".to_string(),
            "0".to_string(),
            "300".to_string(),
            "100".to_string(),
            "200".to_string(),
            "50".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "180".to_string(),
            "3.5".to_string(),
            "4".to_string(),
            "4".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
        ];
        lines.push(row.join(","));
    }

    fs::write(path, lines.join("\n")).expect("failed to write weather fixture");
}

pub fn build_dwelling_config(
    dir: &Path,
    bldg_id: i64,
    schedule_path: PathBuf,
    weather_path: PathBuf,
    defaults_path: PathBuf,
    duration: Duration,
) -> DwellingConfig {
    DwellingConfig {
        hpxml: InputSource::Path(fixture_hpxml_path()),
        schedule: Some(InputSource::Path(schedule_path)),
        weather: WeatherInput::Path(weather_path),
        defaults: DefaultsSource::directory(defaults_path),
        settings: DwellingSettings::new(
            SimulationConfig {
                scenario_seed: None,
                schedule_diversity: Default::default(),
                start_time: fixture_start::fixture_start(),
                duration,
                time_res: Duration::minutes(1),
                reports: vec![hares_io::ReportSpec {
                    name: "file".to_string(),
                    selection: hares_io::Selection::Verbosity(0),
                    frequency: hares_io::Frequency::default(),
                    destination: hares_io::Destination::Csv {
                        path: numbered_path(dir, "hares-bench-output", "csv"),
                        overwrite: true,
                    },
                }],
                diagnostics: false,
                metrics: false,
                output_chunk_size: 1024,
                setpoint_deadband_c: None,
                master_seed: 0,
                civil_timezone: None,
                site_location: hares_io::SiteLocationOverride::default(),
                max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
                health_policy: hares_io::HealthPolicy::default(),
                first_hour_interpolation: hares_io::FirstHour::default(),
                weather_year: hares_io::WeatherYear::default(),
                warmup: hares_io::Warmup::Disabled,
            },
            None,
            bldg_id,
            None,
        ),
    }
}

pub fn synthetic_toml(duration_s: i64, time_res_s: i64) -> String {
    synthetic_toml_with_start(duration_s, time_res_s, "2024-01-01T00:00:00Z")
}

/// The same fixture at a caller's start: the year-episode witness's
/// non-leap case starts 2023-01-01, where the run year carries no Feb 29
/// for the typical-weather rule to skip.
pub fn synthetic_toml_with_start(duration_s: i64, time_res_s: i64, start: &str) -> String {
    format!(
        r#"building_id = 1

[simulation]
start_time = "{start}"
time_res = {time_res_s}
duration = {duration_s}
output_chunk_size = 1000
master_seed = 0
[geometry]
number_of_bedrooms = 3.0
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.0

[hvac]
equipment_name = "Furnace"
fuel = "electricity"
heating_capacity_kbtu_h = 25.0

[weather]
outdoor_temp_c = 8.0
dew_point_c = 4.0
rel_humidity_pct = 55.0
pressure_kpa = 101.325

# The infiltration measurement the airflow assembly requires (OS-HPXML
# errors on a dwelling with neither a measurement nor a constant rate):
# 3 ACH, a conventional continuous rate for the leaky tier this
# synthetic single-zone dwelling models (R-2 walls, no controlled
# ventilation). The bench's workload is the stepping cost, unchanged
# from what the fixture intended before the assembly hardened.
[infiltration]
ach = 3.0

[schedule]
occupancy = 1.0
"#
    )
}

pub fn synthetic_toml_case(path: &PathBuf, duration_s: i64) {
    let toml = synthetic_toml(duration_s, 60);
    fs::write(path, toml).expect("failed to write synthetic TOML fixture");
}

/// The checkpoint bench's small roster: the synthetic TOML dwelling (one
/// zone, one electric furnace, no event loads, no actors), the checkpoint
/// floor, the same fixture the rl_step bench builds. The run's 26-hour
/// duration covers the bench's 24-hour prefix and its continuation with
/// margin, so the assertion's steps never meet the run's end.
pub const CHECKPOINT_SMALL_PREFIX_STEPS: u64 = 1_440; // 24 h at 60 s
pub const CHECKPOINT_SMALL_CONTINUATION_STEPS: u64 = 60; // 1 h

/// The checkpoint bench's large roster: the HPXML base.xml fixture at the
/// fleet shape (900 s steps, no own reports), the eight-equipment roster
/// the fleet and single_building benches run (heating, cooling, water
/// heating, clothes washer, clothes dryer, dishwasher, cooking range,
/// refrigerator; four of them RNG-carrying event appliances). The run's
/// 48-hour duration covers the 24-hour prefix and the continuation with
/// margin.
pub const CHECKPOINT_LARGE_PREFIX_STEPS: u64 = 96; // 24 h at 900 s
pub const CHECKPOINT_LARGE_CONTINUATION_STEPS: u64 = 48; // 12 h

/// The rl_episode bench's year-episode shape, shared with the
/// bench_construction suite's year witnesses: 366 days at 3600 s steps.
/// The leap-year start's Feb 29 is skipped under the typical weather
/// rule, so the duration holds exactly 8,760 steps with the skip and
/// 8,784 without it (the non-leap start).
pub const YEAR_EPISODE_DURATION_S: i64 = 31_622_400;
pub const YEAR_EPISODE_TIME_RES_S: i64 = 3_600;
pub const YEAR_EPISODE_STEPS: usize = 8_760;

/// The checkpoint bench's small-roster dwelling: the shared synthetic
/// TOML through `from_toml_config_with_reports`, the bench's own
/// construction call, with no reports (the restore refuses non-memory
/// report destinations, and the bench measures the checkpoint, not the
/// report row streams).
pub fn checkpoint_small_dwelling(dir: &Path) -> Dwelling {
    let path = numbered_path(dir, "hares-bench-checkpoint-small", "toml");
    synthetic_toml_case(&path, 93_600);
    Dwelling::from_toml_config_with_reports(&path, Some(Vec::new()))
        .expect("the checkpoint bench's small dwelling builds")
}

/// The checkpoint bench's large-roster dwelling config: the shared HPXML
/// fixture through `build_dwelling_config` at the fleet shape.
pub fn checkpoint_large_config(
    dir: &Path,
    bldg_id: i64,
    schedule_path: &Path,
    weather_path: &Path,
) -> DwellingConfig {
    let mut config = build_dwelling_config(
        dir,
        bldg_id,
        schedule_path.to_path_buf(),
        weather_path.to_path_buf(),
        repo_defaults_path(),
        Duration::hours(48),
    );
    config.settings.sim_config.reports.clear();
    config.settings.sim_config.time_res = Duration::seconds(900);
    config
}

/// The checkpoint bench's large-roster dwelling: the config above through
/// `Dwelling::from_config`, the construction the restore target repeats.
pub fn checkpoint_large_dwelling(
    dir: &Path,
    bldg_id: i64,
    schedule_path: &Path,
    weather_path: &Path,
) -> Dwelling {
    let config = checkpoint_large_config(dir, bldg_id, schedule_path, weather_path);
    Dwelling::from_config(config).expect("the checkpoint bench's large dwelling builds")
}

/// Steps `n` times, failing with `what` named: the bench's prefix and
/// continuation loops, and the same loops in the suite's copy.
pub fn step_n(dwelling: &mut Dwelling, n: u64, what: &str) {
    for _ in 0..n {
        dwelling
            .step()
            .unwrap_or_else(|err| panic!("{what}: the step fails: {err}"));
    }
}

/// The checkpoint's canonical JSON bytes: the `serde_json::Value` form,
/// whose objects are BTreeMaps (serde_json without `preserve_order`), so
/// every map's keys render in sorted order whatever iteration order the
/// snapshot's own HashMap instances happened to hold. The canonical form
/// is the bench's bitwise witness: equal bytes pin equal structure and,
/// for finite values, equal float bits (the shortest round-trip form is
/// injective per value, minus-zero rendering apart from zero).
/// Non-finite values render as serde_json `null` and cannot be
/// distinguished in the bytes; the `PartialEq` leg catches the NaN cases
/// anyway (a NaN never compares equal).
pub fn canonical_checkpoint_bytes(checkpoint: &hares_core::DwellingCheckpoint) -> Vec<u8> {
    let value = serde_json::to_value(checkpoint).expect("the checkpoint converts to a JSON value");
    serde_json::to_vec(&value).expect("the canonical form serializes")
}

/// The checkpoint bench's setup assertions, run once per roster size
/// outside the timed regions (and by the bench_construction suite at
/// suite cost), failing loudly if either breaks: a bench that cannot
/// detect a deliberate regression in the path it times is a finding.
///
/// 1. State bit-identity after restore: the prefix-stepped state saves,
///    serializes, parses through the gated reader and restores into a
///    freshly built dwelling of the same fixture; the restored dwelling
///    re-saves a checkpoint equal to the saved one field for field (the
///    type derives `PartialEq`, the maps comparing order-independently)
///    and equal in canonical bytes (every finite float bit preserved;
///    the non-finite values render as `null` there, the `PartialEq` leg
///    catching the NaN cases).
/// 2. The restored run's continuation equivalence: the restored dwelling
///    and a fresh continuous dwelling step the same further steps; the
///    two end states are equal field for field and in canonical bytes,
///    the one named actor-facing divergence narrowed to its key (see the
///    compare's comment). The full output-surface acceptance stays with
///    the checkpoint_resume suite; this is the bench's own short witness.
pub fn assert_checkpoint_round_trip(
    dir: &Path,
    build: impl Fn() -> Dwelling,
    prefix_steps: u64,
    continuation_steps: u64,
    label: &str,
) {
    let mut source = build();
    step_n(&mut source, prefix_steps, &format!("{label}: the prefix"));
    let checkpoint = source
        .save_checkpoint()
        .unwrap_or_else(|err| panic!("{label}: the prefix state saves: {err}"));
    // The pinned file path: the serde_json serialization, the SHA-256
    // checksum and the atomic rename, then the checksum verify and the
    // gated parse back.
    let file = numbered_path(dir, "hares-bench-checkpoint-assert", "json");
    checkpoint
        .save(&file)
        .unwrap_or_else(|err| panic!("{label}: the checkpoint file writes: {err}"));
    let loaded = hares_core::DwellingCheckpoint::load(&file)
        .unwrap_or_else(|err| panic!("{label}: the checkpoint file loads: {err}"));

    // Bit-identity: the gated parse restores into a fresh dwelling of the
    // same fixture, and that dwelling re-saves the same state.
    let mut restored = build();
    restored
        .load_checkpoint(loaded.clone())
        .unwrap_or_else(|err| panic!("{label}: the checkpoint restores: {err}"));
    let re_saved = restored
        .save_checkpoint()
        .unwrap_or_else(|err| panic!("{label}: the restored state saves: {err}"));
    assert_eq!(
        loaded, checkpoint,
        "{label}: the file round trip changes the checkpoint"
    );
    assert_eq!(
        re_saved, loaded,
        "{label}: the restored dwelling re-saves a different checkpoint"
    );
    assert_eq!(
        canonical_checkpoint_bytes(&re_saved),
        canonical_checkpoint_bytes(&checkpoint),
        "{label}: the re-saved checkpoint's canonical bytes differ from the saved run's"
    );

    // Continuation equivalence: the restored run and the continuous run
    // step the same further steps and hold the same committed state. The
    // one named actor-facing divergence is narrowed to its exact key
    // instead of excluding the maps wholesale: the air conditioner's
    // `load_state` inserts the `shr` telemetry key at the restore (the
    // checkpoint's decoded value, air_conditioner.rs's load_state), and
    // the first post-restore step's snapshot copies it into the
    // actor-facing map; the continuous run's never-cooled air
    // conditioner's step never published the key, so the key exists in
    // the restored run's map only. Exactly that key is removed from the
    // restored side where the continuous side lacks it (at most one
    // registration: a second would be some other equipment's shr, not
    // the named divergence), and the two actor-facing maps otherwise
    // compare in full — any other key registered, any value differing,
    // fails, as does an equipment entry present on one side only. The
    // committed state the checkpoint carries (the equipment blobs, the
    // thermal, humidity and fluid state, the RNG, the electrical
    // summary, the health and warning records) compares in full.
    step_n(
        &mut restored,
        continuation_steps,
        &format!("{label}: the continuation"),
    );
    let mut continuous = build();
    step_n(
        &mut continuous,
        prefix_steps + continuation_steps,
        &format!("{label}: the continuous run"),
    );
    let continuous_end = continuous
        .save_checkpoint()
        .unwrap_or_else(|err| panic!("{label}: the continuous state saves: {err}"));
    let mut restored_end = restored
        .save_checkpoint()
        .unwrap_or_else(|err| panic!("{label}: the continued state saves: {err}"));
    let mut narrowed = 0usize;
    for (name, telemetry) in restored_end.env_equipment_telemetry.iter_mut() {
        let Some(continuous_telemetry) = continuous_end.env_equipment_telemetry.get(name) else {
            continue;
        };
        if telemetry.0.contains_key("shr") && !continuous_telemetry.0.contains_key("shr") {
            telemetry.0.remove("shr");
            narrowed += 1;
        }
    }
    assert!(
        narrowed <= 1,
        "{label}: {narrowed} equipments' `shr` keys are restorations' own; \
         the named divergence is one registration"
    );
    assert_eq!(
        restored_end, continuous_end,
        "{label}: the resumed run's committed state after {continuation_steps} further \
         steps is not the continuous run's"
    );
    assert_eq!(
        canonical_checkpoint_bytes(&restored_end),
        canonical_checkpoint_bytes(&continuous_end),
        "{label}: the resumed run's committed state after {continuation_steps} further \
         steps is not the continuous run's, byte for byte"
    );
}
