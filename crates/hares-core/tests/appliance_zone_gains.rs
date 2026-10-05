//! HPXML appliance event loads deliver their heat to the zone they stand in.
//!
//! Each appliance is taken out of a dwelling assembled from HPXML (so its
//! config carries everything parsing and assembly give it) and stepped
//! alone on the dwelling's environment, one port table per step. The heat
//! it accumulates on the conditioned zone's thermal port must equal its
//! electric plus fuel input times the OpenStudio-HPXML v1.12.0 sensible and
//! latent fractions (`HPXMLtoOpenStudio/resources/hotwater_appliances.rb`:
//! `calc_range_oven_energy`, `calc_clothes_washer_energy_gpd`,
//! `calc_clothes_dryer_energy`, `calc_dishwasher_energy_gpd`).

use std::path::{Path, PathBuf};
use std::time::Duration as StdDuration;

use chrono::{DateTime, Duration, FixedOffset};
use hares_core::{Dwelling, DwellingConfig, SimulationConfig};
use hares_equipment::Equipment;
use hares_io::OutputFormat;
use hares_io::hpxml::building::parse_building;
use hares_types::{FuelType, PortSlots, ZoneId};

const TIME_RES_S: i64 = 900;
const STEPS_PER_DAY: i64 = 86_400 / TIME_RES_S;

/// The OpenStudio-HPXML split of an appliance's input in conditioned space.
struct ExpectedSplit {
    name: &'static str,
    sensible: f64,
    latent: f64,
}

/// Electric range, washer, vented dryer and dishwasher, all in
/// conditioned space: `frac_lost` 0.20, 0.70, 0.85 and 0.40, with 0.90,
/// 0.90, 0.90 and 0.50 of the rest sensible.
const CONDITIONED_SPACE_SPLITS: [ExpectedSplit; 4] = [
    ExpectedSplit {
        name: "Cooking Range",
        sensible: 0.72,
        latent: 0.08,
    },
    ExpectedSplit {
        name: "Clothes Washer",
        sensible: 0.27,
        latent: 0.03,
    },
    ExpectedSplit {
        name: "Clothes Dryer",
        sensible: 0.135,
        latent: 0.015,
    },
    ExpectedSplit {
        name: "Dishwasher",
        sensible: 0.30,
        latent: 0.30,
    },
];

/// One HPXML run: its inputs, start and length.
struct Run {
    fixture: PathBuf,
    hpxml: &'static str,
    schedule: &'static str,
    weather: PathBuf,
    start: &'static str,
    days: i64,
    bldg_id: i64,
}

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn dwelling_config(run: &Run) -> DwellingConfig {
    DwellingConfig {
        hpxml_path: run.fixture.join(run.hpxml),
        schedule_path: Some(run.fixture.join(run.schedule)),
        weather_path: run.weather.clone(),
        defaults_path: Some(project_root().join("defaults")),
        sim_config: SimulationConfig {
            start_time: DateTime::<FixedOffset>::parse_from_rfc3339(run.start)
                .expect("start time parses"),
            duration: Duration::days(run.days),
            time_res: Duration::seconds(TIME_RES_S),
            output_verbosity: 0,
            write_output: false,
            output_path: None,
            output_format: OutputFormat::Csv,
            output_chunk_size: 1024,
            setpoint_deadband_c: None,
            master_seed: 0,
            civil_timezone: None,
            site_location: hares_io::SiteLocationOverride::default(),
            retain_batches: false,
            rotation: hares_io::RotationPolicy::None,
        },
        overrides: None,
        bldg_id: run.bldg_id,
        initialization_duration: None,
        resample_overrides: None,
        patches: None,
    }
}

fn conditioned_zone(hpxml: &Path) -> ZoneId {
    let xml = std::fs::read_to_string(hpxml).expect("HPXML reads");
    let building = parse_building(&xml).expect("HPXML parses");
    let idx = building
        .conditioned_zone_index()
        .expect("one conditioned zone")
        .expect("the building has a conditioned zone");
    ZoneId(u16::try_from(idx + 1).expect("zone index fits u16"))
}

/// Run totals for one appliance, in Wh.
#[derive(Default)]
struct Totals {
    input_wh: f64,
    sensible_wh: f64,
    latent_wh: f64,
}

const DT_H: f64 = TIME_RES_S as f64 / 3600.0;

/// Steps the dwelling for the run with the named appliances taken out of it,
/// stepping each one alone on the dwelling's environment and handing
/// `visit` the step index, the appliance's index in `names` and its ports.
fn step_appliances(run: &Run, names: &[&str], mut visit: impl FnMut(i64, usize, &PortSlots)) {
    let mut dwelling = Dwelling::from_config(dwelling_config(run)).expect("dwelling builds");
    let mut appliances: Vec<Box<dyn Equipment>> = names
        .iter()
        .map(|name| {
            dwelling
                .remove_equipment(name)
                .unwrap_or_else(|err| panic!("{name}: present in the dwelling: {err}"))
        })
        .collect();
    let dt = StdDuration::from_secs(TIME_RES_S.unsigned_abs());
    for step in 0..run.days * STEPS_PER_DAY {
        let env = dwelling.latest_env().clone();
        for (idx, appliance) in appliances.iter_mut().enumerate() {
            let mut ports = PortSlots::from_declarations(appliance.ports());
            appliance
                .step(&env, dt, &mut ports)
                .unwrap_or_else(|err| panic!("{} steps: {err}", appliance.descriptor().name));
            visit(step, idx, &ports);
        }
        dwelling.step().expect("dwelling steps");
    }
}

/// An appliance's electric plus fuel input this step, in Wh.
fn input_wh(ports: &PortSlots, fuel: FuelType) -> f64 {
    (ports.electrical.load_power_w + ports.fuel.get(fuel)) * DT_H
}

/// Checks what each appliance of `expected` delivers to the conditioned zone.
fn assert_appliance_gains_reach_conditioned_zone(
    run: &Run,
    fuel: FuelType,
    expected: &[ExpectedSplit],
) {
    let zone = conditioned_zone(&run.fixture.join(run.hpxml));
    let names: Vec<&str> = expected.iter().map(|e| e.name).collect();
    let mut totals: Vec<Totals> = expected.iter().map(|_| Totals::default()).collect();
    step_appliances(run, &names, |_, idx, ports| {
        let total = &mut totals[idx];
        total.input_wh += input_wh(ports, fuel);
        for thermal in ports.thermal.iter().filter(|t| t.zone == zone) {
            total.sensible_wh += thermal.sensible_gain_w * DT_H;
            total.latent_wh += thermal.latent_gain_w * DT_H;
        }
    });

    for (split, total) in expected.iter().zip(&totals) {
        assert!(
            total.input_wh > 0.0,
            "{}: the run must operate the appliance",
            split.name
        );
        let tolerance = 1e-9 * total.input_wh;
        for (kind, delivered, fraction) in [
            ("sensible", total.sensible_wh, split.sensible),
            ("latent", total.latent_wh, split.latent),
        ] {
            let want = total.input_wh * fraction;
            assert!(
                (delivered - want).abs() <= tolerance,
                "{}: {kind} gain to the zone {delivered:.3} Wh, expected {want:.3} Wh \
                 ({fraction} of {:.3} Wh input)",
                split.name,
                total.input_wh
            );
        }
    }
}

/// An OpenStudio-HPXML building with an electric range and dryer; its
/// first nine days are the first span of its schedule in which all four
/// appliances run.
#[test]
fn hpxml_appliance_gains_reach_the_conditioned_zone() {
    let fixture = project_root().join("tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh");
    let run = Run {
        weather: fixture.join("weather.epw"),
        fixture,
        hpxml: "building.xml",
        schedule: "schedule.csv",
        start: "2023-01-01T00:00:00-07:00",
        days: 9,
        bldg_id: 1,
    };
    assert_appliance_gains_reach_conditioned_zone(
        &run,
        FuelType::Electric,
        &CONDITIONED_SPACE_SPLITS,
    );
}

/// A ResStock home replays its appliance events from the schedule file;
/// the replayed events deliver their heat too, the vented gas dryer's
/// combustion included (OpenStudio-HPXML applies the same split to the
/// dryer's electric and fuel input).
#[test]
fn resstock_event_load_replay_delivers_its_gains() {
    let run = Run {
        fixture: project_root().join("tests/fixtures/resstock/2025.1/bldg0176227"),
        hpxml: "home.xml",
        schedule: "in.schedules.csv",
        weather: project_root().join("tests/fixtures/resstock/2025.1/weather/G0600770_2018.csv"),
        start: "2018-01-01T00:00:00-08:00",
        days: 1,
        bldg_id: 176_227,
    };
    assert_appliance_gains_reach_conditioned_zone(&run, FuelType::Gas, &CONDITIONED_SPACE_SPLITS);
}

const REPLAYED: [&str; 4] = [
    "Cooking Range",
    "Clothes Washer",
    "Clothes Dryer",
    "Dishwasher",
];

/// Each replayed appliance's electric input per day of a run of the parity
/// building starting at `start`, in Wh: `[day][appliance]`.
fn daily_replay_wh(start: &'static str, days: i64) -> Vec<[f64; 4]> {
    let fixture = project_root().join("tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh");
    let run = Run {
        weather: fixture.join("weather.epw"),
        fixture,
        hpxml: "building.xml",
        schedule: "schedule.csv",
        start,
        days,
        bldg_id: 1,
    };
    let mut daily = vec![[0.0; 4]; usize::try_from(days).expect("days fit usize")];
    step_appliances(&run, &REPLAYED, |step, idx, ports| {
        let day = usize::try_from(step / STEPS_PER_DAY).expect("day fits usize");
        daily[day][idx] += input_wh(ports, FuelType::Electric);
    });
    daily
}

fn assert_same_day(label: &str, got: [f64; 4], want: [f64; 4]) {
    assert!(
        want.iter().sum::<f64>() > 0.0,
        "{label}: the reference day must run an appliance"
    );
    for (name, (g, w)) in REPLAYED.iter().zip(got.iter().zip(want)) {
        assert!(
            (g - w).abs() <= 1e-9 * w.abs().max(1.0),
            "{label}: {name} replays {g:.3} Wh, the schedule's date gives {w:.3} Wh"
        );
    }
}

/// A run replays the appliance events of its own dates: a run starting on
/// 7 January and one starting on 13 January each replay that day's events,
/// the same as a run from 1 January replays on those days.
#[test]
fn event_replay_follows_the_calendar_date() {
    let from_jan_1 = daily_replay_wh("2023-01-01T00:00:00-07:00", 13);
    let jan_7 = daily_replay_wh("2023-01-07T00:00:00-07:00", 1)[0];
    let jan_13 = daily_replay_wh("2023-01-13T00:00:00-07:00", 1)[0];
    assert_same_day("7 January", jan_7, from_jan_1[6]);
    assert_same_day("13 January", jan_13, from_jan_1[12]);
    assert_ne!(jan_7, jan_13, "different dates replay different events");
}

/// The annual schedule has 365 days: 1 March of a leap year replays the
/// schedule's 1 March, not 29 February's row.
#[test]
fn event_replay_skips_february_29() {
    let leap_mar_1 = daily_replay_wh("2024-03-01T00:00:00-07:00", 1)[0];
    assert_same_day(
        "1 March 2024",
        leap_mar_1,
        daily_replay_wh("2023-03-01T00:00:00-07:00", 1)[0],
    );
    assert_ne!(
        leap_mar_1,
        daily_replay_wh("2023-02-28T00:00:00-07:00", 1)[0],
        "1 March must not replay 28 February"
    );
}

/// A run crossing the new year wraps to the schedule's first day.
#[test]
fn event_replay_wraps_at_the_year_end() {
    let across = daily_replay_wh("2023-12-31T00:00:00-07:00", 2);
    let jan_1 = daily_replay_wh("2023-01-01T00:00:00-07:00", 1)[0];
    assert_same_day("1 January after the wrap", across[1], jan_1);
    assert_ne!(across[0], jan_1, "31 December must not replay 1 January");
}
