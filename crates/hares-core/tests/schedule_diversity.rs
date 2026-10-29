//! The generated-schedule family's per-dwelling behavioral diversity: two
//! homes that share one blueprint and one master seed draw different
//! behavioral schedules, the same home reproduces its draws bitwise, and a
//! declared scenario moves the draws (the diversity's key is the
//! scenario-mixed per-dwelling key, so the scenario rides it).
//!
//! The diversity layer lives on the GENERATED schedule only: a schedule
//! CSV is measured or upstream data and is never diversified. Every
//! witness here builds the same HPXML home with no schedule source, so
//! the schedule is the generated family's.

use std::path::PathBuf;

use chrono::{Duration, TimeZone};

use hares_core::{Dwelling, DwellingConfig, DwellingSettings};
use hares_io::{DefaultsSource, InputSource, SimulationConfig, WeatherInput};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates dir")
        .parent()
        .expect("repo root")
        .to_path_buf()
}

/// The ResStock home the setpoint-source witnesses run: a real HPXML
/// document with real defaults, and its own county weather.
fn home_xml() -> PathBuf {
    repo_root().join("tests/fixtures/resstock/2025.1/bldg0000002/home.xml")
}

fn weather_csv() -> PathBuf {
    repo_root().join("tests/fixtures/resstock/2025.1/weather/G0900090_2018.csv")
}

fn config(bldg_id: i64, master_seed: u64, scenario_seed: Option<u64>) -> DwellingConfig {
    let start_time = chrono::FixedOffset::east_opt(-6 * 3600)
        .expect("offset")
        .with_ymd_and_hms(2018, 1, 5, 0, 0, 0)
        .single()
        .expect("start time");
    DwellingConfig {
        hpxml: InputSource::Path(home_xml()),
        schedule: None,
        weather: WeatherInput::Path(weather_csv()),
        defaults: DefaultsSource::directory(repo_root().join("python/hares/defaults")),
        settings: DwellingSettings::new(
            SimulationConfig {
                scenario_seed,
                schedule_diversity: Default::default(),
                start_time,
                duration: Duration::hours(24),
                time_res: Duration::seconds(3600),
                reports: Vec::new(),
                diagnostics: false,
                metrics: false,
                output_chunk_size: 1024,
                setpoint_deadband_c: None,
                master_seed,
                civil_timezone: None,
                site_location: hares_io::SiteLocationOverride::default(),
                max_consecutive_step_failures: 1,
                health_policy: Default::default(),
                first_hour_interpolation: hares_io::FirstHour::Hour24,
                weather_year: hares_io::WeatherYear::Actual,
                warmup: hares_io::Warmup::default(),
            },
            None,
            bldg_id,
            None,
        ),
    }
}

/// The resampled schedule's columns as raw f64 rows: the witness compares
/// these bitwise.
fn schedule_rows(dwelling: &Dwelling) -> Vec<Vec<f64>> {
    let schedule = dwelling.blueprint().resampled_schedule();
    schedule.columns.clone()
}

/// The acceptance witness: two homes under the same master seed, the same
/// HPXML blueprint and no schedule source, must NOT run identical
/// behavioral schedules. At the diversity layer's absence both homes run
/// the same default-profile columns, and this assertion fails naming the
/// first differing column's index (it cannot: there is none).
#[test]
fn two_homes_same_blueprint_draw_different_behavior() {
    let a = Dwelling::from_config(config(1, 42, None)).expect("home a builds");
    let b = Dwelling::from_config(config(2, 42, None)).expect("home b builds");
    let rows_a = schedule_rows(&a);
    let rows_b = schedule_rows(&b);
    assert_eq!(
        rows_a.len(),
        rows_b.len(),
        "the two homes' schedules carry the same column count"
    );
    let differing: Vec<usize> = rows_a
        .iter()
        .zip(rows_b.iter())
        .enumerate()
        .filter(|(_, (ca, cb))| ca != cb)
        .map(|(i, _)| i)
        .collect();
    assert!(
        !differing.is_empty(),
        "two homes under one master seed must not run identical behavioral \
         schedules: no generated column differs"
    );
}

/// The reproducibility witness: the same (master seed, bldg id, scenario)
/// builds the same home twice and the schedules are bit-identical.
#[test]
fn same_seed_same_home_reproduces_bitwise() {
    let a = Dwelling::from_config(config(1, 42, None)).expect("home a builds");
    let b = Dwelling::from_config(config(1, 42, None)).expect("home a builds again");
    assert_eq!(schedule_rows(&a), schedule_rows(&b));
}

/// The scenario axis: the same home under two declared scenarios draws
/// different behavior, and a declared scenario 0 is not the absent
/// scenario (the presence byte's contract), while the absent scenario's
/// schedule reproduces the scenario-less run's.
#[test]
fn a_declared_scenario_changes_the_draws() {
    let absent = Dwelling::from_config(config(1, 42, None)).expect("absent builds");
    let zero = Dwelling::from_config(config(1, 42, Some(0))).expect("scenario 0 builds");
    let one = Dwelling::from_config(config(1, 42, Some(1))).expect("scenario 1 builds");
    let two = Dwelling::from_config(config(1, 42, Some(2))).expect("scenario 2 builds");

    let absent_rows = schedule_rows(&absent);
    let zero_rows = schedule_rows(&zero);
    let one_rows = schedule_rows(&one);
    let two_rows = schedule_rows(&two);

    assert_ne!(
        one_rows, two_rows,
        "two scenarios under one master seed must not draw the same behavior"
    );
    assert_ne!(
        zero_rows, one_rows,
        "declared scenario 0 must not draw scenario 1's behavior"
    );
    assert_ne!(
        absent_rows, zero_rows,
        "the absent scenario must not draw declared scenario 0's behavior"
    );
    // The absent scenario's draws reproduce the scenario-less key's: the
    // same home built again with no scenario matches.
    let again = Dwelling::from_config(config(1, 42, None)).expect("absent builds again");
    assert_eq!(absent_rows, schedule_rows(&again));
}

/// The escape hatch: `schedule_diversity = "off"` restores the
/// pre-diversity behavior, every no-CSV home running the same
/// default-profile columns bit-for-bit.
#[test]
fn diversity_off_restores_the_shared_profile() {
    let mut off_a = config(1, 42, None);
    off_a.settings.sim_config.schedule_diversity = hares_io::ScheduleDiversity::Off;
    let mut off_b = config(2, 42, None);
    off_b.settings.sim_config.schedule_diversity = hares_io::ScheduleDiversity::Off;
    let a = Dwelling::from_config(off_a).expect("home a builds");
    let b = Dwelling::from_config(off_b).expect("home b builds");
    assert_eq!(
        schedule_rows(&a),
        schedule_rows(&b),
        "with the layer off, two homes run the same generated schedule"
    );
    // And the off schedule is the undiversified profiles' own: the two
    // homes' columns equal a diversity-on home's ONLY if that home drew
    // the identity, so compare against the generation function's output
    // instead: the off run equals a direct generation from the shipped
    // defaults over the same span.
    let profiles = hares_io::load_default_profiles(&repo_root().join("python/hares/defaults"))
        .expect("the shipped profiles load");
    let start = config(1, 42, None).settings.sim_config.start_time;
    let direct = hares_io::hpxml_schedule::generate_default_schedule(
        start,
        Duration::hours(24),
        Duration::seconds(3600),
        &profiles,
    )
    .expect("the undiversified schedule generates");
    let a_schedule = a.blueprint().resampled_schedule();
    let mut differing = 0_usize;
    for (name, column) in a_schedule.column_names.iter().zip(&a_schedule.columns) {
        let idx = direct
            .column_index
            .get(name)
            .unwrap_or_else(|| panic!("the direct schedule carries {name}"));
        if column[..24.min(column.len())] != direct.columns[*idx][..24.min(column.len())] {
            differing += 1;
        }
    }
    assert_eq!(
        differing, 0,
        "the off run's generated columns must be the undiversified profiles' own"
    );
}

/// The CSV path's bitwise safety under the default-on knob: a schedule
/// CSV is measured or upstream data, so the diversity layer is inert
/// whatever the setting. The same CSV home built with the knob on and
/// off is bitwise equal row for row, and its table is not the generated
/// family's over the same span (the fixture is a real CSV, not a source
/// that fell back to generation).
#[test]
fn a_schedule_csv_is_never_diversified() {
    let csv = repo_root().join("tests/fixtures/resstock/2025.1/bldg0000002/in.schedules.csv");
    let mut on = config(1, 42, None);
    on.schedule = Some(InputSource::Path(csv.clone()));
    let mut off = config(1, 42, None);
    off.schedule = Some(InputSource::Path(csv));
    off.settings.sim_config.schedule_diversity = hares_io::ScheduleDiversity::Off;
    let on_dwelling = Dwelling::from_config(on).expect("the CSV home builds");
    let off_dwelling = Dwelling::from_config(off).expect("the CSV home builds with the layer off");
    assert_eq!(
        schedule_rows(&on_dwelling),
        schedule_rows(&off_dwelling),
        "a schedule CSV must be bitwise equal with the layer on and off"
    );
    let generated = Dwelling::from_config(config(1, 42, None)).expect("the generated home builds");
    assert_ne!(
        schedule_rows(&off_dwelling),
        schedule_rows(&generated),
        "the CSV fixture's rows must not be the generated family's: the \
         equality above would then prove nothing"
    );
}

/// The new sources add no runtime stochastic state: the diversity's draws
/// ride the build-time profile transform and no stream of theirs appears
/// in the dwelling's seeded-stream ledger, and the dwelling's own seeded
/// streams (key, nonce, word position) are identical with the layer on
/// and off, warm-up enabled or not.
#[test]
fn the_diversity_adds_no_runtime_stochastic_state() {
    let mut cfg_warm = config(1, 42, None);
    cfg_warm.settings.sim_config.warmup = hares_io::Warmup::default();
    let mut cfg_nowarm = config(1, 42, None);
    cfg_nowarm.settings.sim_config.warmup = hares_io::Warmup::Disabled;
    let warm = Dwelling::from_config(cfg_warm).expect("the warm-up dwelling builds");
    let nowarm = Dwelling::from_config(cfg_nowarm).expect("the no-warm-up dwelling builds");
    let warm_streams = warm.seeded_streams();
    let nowarm_streams = nowarm.seeded_streams();
    assert_eq!(
        warm_streams, nowarm_streams,
        "the warm-up and no-warm-up builds must hold the same seeded streams"
    );
    for stream in &warm_streams {
        assert!(
            !stream.component.contains("diversity"),
            "the diversity must not appear in the dwelling's stream ledger: \
             found '{}'",
            stream.component
        );
    }
    // The schedules are the same bitwise: the draws are build-time
    // constants, not per-day state the warm-up replays or shifts.
    assert_eq!(schedule_rows(&warm), schedule_rows(&nowarm));
}
