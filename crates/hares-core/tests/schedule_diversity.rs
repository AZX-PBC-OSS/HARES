//! The generated-schedule family's per-dwelling behavioral diversity: two
//! homes under one master seed and different building ids draw different
//! behavioral schedules, the same home reproduces its draws bitwise, and a
//! declared scenario moves the draws (the diversity's key is the
//! scenario-mixed per-dwelling key, so the scenario rides it). One
//! blueprint carries one building id, so two builds of one blueprint under
//! one seed are the same home and are identical by design.
//!
//! The diversity layer lives on the GENERATED schedule only: a schedule
//! CSV is measured or upstream data and is never diversified. Every
//! witness here builds the same HPXML home with no schedule source, so
//! the schedule is the generated family's.

use std::path::PathBuf;

use chrono::{Duration, TimeZone};

use hares_core::{Dwelling, DwellingBlueprint, DwellingConfig, DwellingSettings};
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
/// builds the same home twice and the schedules are bit-identical. One
/// blueprint carries one building id, so two builds of one blueprint under
/// one seed are that same home: the key names the boundary, and only a new
/// seed (`build_with_seed`) or a new blueprint re-derives the schedule.
#[test]
fn same_seed_same_home_reproduces_bitwise() {
    let a = Dwelling::from_config(config(1, 42, None)).expect("home a builds");
    let b = Dwelling::from_config(config(1, 42, None)).expect("home a builds again");
    assert_eq!(schedule_rows(&a), schedule_rows(&b));
    let blueprint =
        DwellingBlueprint::from_config(config(1, 42, None)).expect("the blueprint builds");
    let first = blueprint.build().expect("the blueprint's first build");
    let second = blueprint.build().expect("the blueprint's second build");
    assert_eq!(
        schedule_rows(&first),
        schedule_rows(&second),
        "one blueprint is one building: its two builds under one seed are \
         the same home's schedule"
    );
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

/// The master seed's reach covers the generated schedule: the schedule's
/// draws key on the scenario-mixed per-dwelling key, whose master seed is
/// the seed the dwelling is built with. A blueprint frozen under one seed
/// and built with another (`DwellingBlueprint::build_with_seed`, the
/// Python `reset_with_seed`'s full rebuild) must therefore draw the new
/// seed's schedule and equal a dwelling constructed from a config that
/// carried that seed; keeping the blueprint's construction seed's draws
/// would leave the dwelling's schedule and its seeded streams describing
/// two different seeds.
#[test]
fn build_with_seed_draws_the_new_seeds_schedule() {
    let blueprint =
        DwellingBlueprint::from_config(config(1, 1, None)).expect("the blueprint builds");
    let rebuilt = blueprint
        .build_with_seed(2)
        .expect("the blueprint builds under the second seed");
    let direct = Dwelling::from_config(config(1, 2, None)).expect("the seed-2 dwelling builds");
    let original = blueprint
        .build()
        .expect("the blueprint builds under its own seed");

    assert_eq!(
        schedule_rows(&rebuilt),
        schedule_rows(&direct),
        "a dwelling built with master seed 2 must run seed 2's behavioral \
         schedule, not the blueprint's construction seed's"
    );
    assert_eq!(
        rebuilt.seeded_streams(),
        direct.seeded_streams(),
        "the rebuilt dwelling's seeded streams are the seed-2 build's"
    );
    assert_ne!(
        schedule_rows(&rebuilt),
        schedule_rows(&original),
        "the two seeds must draw different schedules for the witness to \
         distinguish the paths"
    );
}

/// The resampled weather's columns as their raw bits: the witness
/// compares these bitwise, so a substituted value at any step of any
/// column fails.
fn weather_bits(series: &hares_io::WeatherTimeSeries) -> Vec<(&'static str, Vec<u64>)> {
    let bits = |column: &Vec<f64>| column.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
    vec![
        ("dry_bulb_c", bits(&series.dry_bulb_c)),
        ("dew_point_c", bits(&series.dew_point_c)),
        ("rel_humidity_pct", bits(&series.rel_humidity_pct)),
        ("pressure_kpa", bits(&series.pressure_kpa)),
        ("ghi_w_m2", bits(&series.ghi_w_m2)),
        ("dni_w_m2", bits(&series.dni_w_m2)),
        ("dhi_w_m2", bits(&series.dhi_w_m2)),
        ("wind_speed_m_s", bits(&series.wind_speed_m_s)),
        ("wind_dir_deg", bits(&series.wind_dir_deg)),
        ("opaque_sky_cover", bits(&series.opaque_sky_cover)),
        (
            "horizontal_infrared_w_m2",
            bits(&series.horizontal_infrared_w_m2),
        ),
        ("sky_temp_c", bits(&series.sky_temp_c)),
        ("ground_temp_c", bits(&series.ground_temp_c)),
        ("liquid_precip_m", bits(&series.liquid_precip_m)),
    ]
}

/// A seed swap changes the schedule and NOTHING else: the re-frozen
/// blueprint's weather is the parent's series bitwise (the resample is
/// cached on the blueprint's shared weather handle, keyed on the step,
/// the first-hour rule, the start and the weather-year rule, none of
/// which the seed touches), so a build under another seed re-derives
/// the schedule alone. The schedule differs (the re-derivation
/// happened; `build_with_seed_draws_the_new_seeds_schedule` holds
/// that), which makes the weather's equality a statement about the
/// swap and not about two identical builds.
#[test]
fn a_seed_swap_moves_the_schedule_and_nothing_else() {
    let blueprint =
        DwellingBlueprint::from_config(config(1, 1, None)).expect("the blueprint builds");
    let rebuilt = blueprint
        .build_with_seed(2)
        .expect("the blueprint builds under the second seed");
    let original = blueprint
        .build()
        .expect("the blueprint builds under its own seed");
    assert_ne!(
        schedule_rows(&rebuilt),
        schedule_rows(&original),
        "the swap must re-derive the schedule for the weather pin to mean \
         anything"
    );
    let parent = blueprint.resampled_weather();
    let reseeded = rebuilt.blueprint().resampled_weather();
    assert_eq!(
        parent.len(),
        reseeded.len(),
        "the two builds share the weather file's span and step"
    );
    for ((name, parent_bits), (_, reseeded_bits)) in weather_bits(&parent)
        .into_iter()
        .zip(weather_bits(&reseeded))
    {
        assert_eq!(
            parent_bits, reseeded_bits,
            "the weather's {name} column moved under a seed swap: the \
             weather is the environment's, not the schedule's"
        );
    }
}

/// The CSV path's seed swap: a schedule CSV is measured or upstream
/// data, so a build under another seed runs the blueprint's own
/// schedule bitwise (the guard takes the in-place build) while its
/// seeded streams are the new seed's: the swap happened, and it moved
/// the schedule not at all.
#[test]
fn a_csv_homes_seed_swap_changes_nothing_in_the_schedule() {
    let csv = repo_root().join("tests/fixtures/resstock/2025.1/bldg0000002/in.schedules.csv");
    let mut cfg = config(1, 1, None);
    cfg.schedule = Some(InputSource::Path(csv));
    let blueprint = DwellingBlueprint::from_config(cfg).expect("the CSV blueprint builds");
    let first = blueprint
        .build_with_seed(1)
        .expect("the construction seed's build");
    let second = blueprint
        .build_with_seed(2)
        .expect("the second seed's build");
    assert_eq!(
        schedule_rows(&first),
        schedule_rows(&second),
        "a CSV schedule's rows must not move under a seed swap: the \
         measured schedule is no household's draw"
    );
    assert_ne!(
        first.seeded_streams(),
        second.seeded_streams(),
        "the swap must reseed the streams: the schedule's equality above \
         would otherwise compare two identical builds"
    );
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

/// The checkpoint carries no schedule: a diversified home restored from a
/// checkpoint holds the draws its build produced, not a re-draw and not
/// the shared profile. The restored home's schedule is the checkpointed
/// home's bitwise, and its continuation is bitwise the uninterrupted
/// home's, so the drawn dimensions survive the restore.
#[test]
fn a_checkpoint_restore_keeps_the_drawn_schedule() {
    let mut original = Dwelling::from_config(config(1, 42, None)).expect("the home builds");
    for _ in 0..2 {
        original.step().expect("the home steps");
    }
    let checkpoint = original.save_checkpoint().expect("the checkpoint saves");

    let mut restored = Dwelling::from_config(config(1, 42, None)).expect("a second build");
    restored
        .load_checkpoint(checkpoint)
        .expect("the checkpoint restores");

    assert_eq!(
        schedule_rows(&restored),
        schedule_rows(&original),
        "the restored home must hold the checkpointed home's drawn schedule"
    );
    let mut off_cfg = config(1, 42, None);
    off_cfg.settings.sim_config.schedule_diversity = hares_io::ScheduleDiversity::Off;
    let off = Dwelling::from_config(off_cfg).expect("the undiversified home builds");
    assert_ne!(
        schedule_rows(&restored),
        schedule_rows(&off),
        "the restored schedule must be the drawn one, not the shared profile"
    );

    // The continuation: the two homes' remaining steps agree bitwise, which
    // the schedule (the loads' driver) would break if the restore re-drew
    // it.
    let original_steps: Vec<_> = (0..20)
        .map(|_| original.step().expect("the checkpointed home steps"))
        .collect();
    let restored_steps: Vec<_> = (0..20)
        .map(|_| restored.step().expect("the restored home steps"))
        .collect();
    assert_eq!(
        restored_steps, original_steps,
        "the restored home's continuation must be the checkpointed home's"
    );
}

/// The diversity layer adds no runtime stochastic state: its draws ride the
/// build-time profile transform, so the dwelling's seeded-stream ledger (the
/// key, the nonce and the word position of every stream the build assigns)
/// is identical with the layer on and off, and so is it warm-up or not. The
/// schedules differ, which is what makes the ledger's equality a statement
/// about the layer's state and not about two identical builds.
#[test]
fn the_diversity_adds_no_runtime_stochastic_state() {
    let mut off_cfg = config(1, 42, None);
    off_cfg.settings.sim_config.schedule_diversity = hares_io::ScheduleDiversity::Off;
    let on = Dwelling::from_config(config(1, 42, None)).expect("the diversified dwelling builds");
    let off = Dwelling::from_config(off_cfg).expect("the undiversified dwelling builds");
    assert_eq!(
        on.seeded_streams(),
        off.seeded_streams(),
        "the layer's draws must not appear in the dwelling's seeded-stream ledger"
    );
    assert_ne!(
        schedule_rows(&on),
        schedule_rows(&off),
        "the knob must move the schedule: the ledger's equality above would \
         otherwise compare two identical builds"
    );
    // Warm-up replays the run's first day; it neither adds a stream nor
    // moves the layer's build-time draws.
    let mut cfg_warm = config(1, 42, None);
    cfg_warm.settings.sim_config.warmup = hares_io::Warmup::default();
    let mut cfg_nowarm = config(1, 42, None);
    cfg_nowarm.settings.sim_config.warmup = hares_io::Warmup::Disabled;
    let warm = Dwelling::from_config(cfg_warm).expect("the warm-up dwelling builds");
    let nowarm = Dwelling::from_config(cfg_nowarm).expect("the no-warm-up dwelling builds");
    assert_eq!(
        warm.seeded_streams(),
        nowarm.seeded_streams(),
        "the warm-up and no-warm-up builds must hold the same seeded streams"
    );
    // The schedules are the same bitwise: the draws are build-time
    // constants, not per-day state the warm-up replays or shifts.
    assert_eq!(schedule_rows(&warm), schedule_rows(&nowarm));
}

/// A runtime reseed re-keys the dwelling's streams and does NOT re-draw the
/// schedule: the behavioral draws are build-time constants, the blueprint's,
/// so the reset that reseeds an episode keeps the home's behavior and moves
/// its stochastic state. The full re-derivation under another seed is
/// `DwellingBlueprint::build_with_seed` (`build_with_seed_draws_the_new_seeds_schedule`).
#[test]
fn a_runtime_reseed_keeps_the_build_time_schedule() {
    let mut dwelling = Dwelling::from_config(config(1, 42, None)).expect("the home builds");
    let before_schedule = schedule_rows(&dwelling);
    let before_streams = dwelling.seeded_streams();
    dwelling.reseed(9).expect("the reseed re-keys the streams");
    assert_ne!(
        dwelling.seeded_streams(),
        before_streams,
        "the reseed must move the seeded streams for this pin to mean anything"
    );
    assert_eq!(
        schedule_rows(&dwelling),
        before_schedule,
        "a runtime reseed must keep the build-time behavioral draws: only a \
         rebuild under another seed re-derives the schedule"
    );
}
