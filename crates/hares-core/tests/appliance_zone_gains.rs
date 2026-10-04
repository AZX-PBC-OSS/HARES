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

/// Steps the dwelling with `expected` taken out of it, stepping each taken
/// appliance alone on the dwelling's environment, and checks what each one
/// delivers to the conditioned zone.
fn assert_appliance_gains_reach_conditioned_zone(
    run: &Run,
    fuel: FuelType,
    expected: &[ExpectedSplit],
) {
    let zone = conditioned_zone(&run.fixture.join(run.hpxml));
    let mut dwelling = Dwelling::from_config(dwelling_config(run)).expect("dwelling builds");
    let mut appliances: Vec<Box<dyn Equipment>> = expected
        .iter()
        .map(|e| {
            dwelling
                .remove_equipment(e.name)
                .unwrap_or_else(|err| panic!("{}: present in the dwelling: {err}", e.name))
        })
        .collect();
    let mut totals: Vec<Totals> = expected.iter().map(|_| Totals::default()).collect();
    let dt = StdDuration::from_secs(TIME_RES_S.unsigned_abs());
    let dt_h = TIME_RES_S as f64 / 3600.0;

    for _ in 0..run.days * STEPS_PER_DAY {
        let env = dwelling.latest_env().clone();
        for (appliance, total) in appliances.iter_mut().zip(&mut totals) {
            let mut ports = PortSlots::from_declarations(appliance.ports());
            appliance
                .step(&env, dt, &mut ports)
                .unwrap_or_else(|err| panic!("{} steps: {err}", appliance.descriptor().name));
            total.input_wh += (ports.electrical.load_power_w + ports.fuel.get(fuel)) * dt_h;
            for thermal in ports.thermal.iter().filter(|t| t.zone == zone) {
                total.sensible_wh += thermal.sensible_gain_w * dt_h;
                total.latent_wh += thermal.latent_gain_w * dt_h;
            }
        }
        dwelling.step().expect("dwelling steps");
    }

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
