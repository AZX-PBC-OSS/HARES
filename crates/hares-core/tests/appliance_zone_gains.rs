//! HPXML appliance event loads deliver their heat to the zone they stand in.
//!
//! Each appliance is taken out of a dwelling assembled from HPXML (so its
//! config carries everything parsing and assembly give it) and stepped
//! alone on the dwelling's environment, one port table per step. The heat
//! it accumulates on the conditioned zone's thermal port must equal its
//! electric plus fuel input times the OpenStudio-HPXML v1.12.0 sensible and
//! latent fractions (`HPXMLtoOpenStudio/resources/hotwater_appliances.rb`:
//! `calc_range_oven_energy`, `calc_clothes_washer_energy_gpd`,
//! `calc_clothes_dryer_energy`, `calc_dishwasher_energy_gpd`), with 0.6 of
//! the sensible heat radiant and the rest convective (`frac_radiant: 0.6 *`
//! the sensible fraction where each appliance is added, lines 80, 121, 172
//! and 309).

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

/// OpenStudio-HPXML's radiant share of an appliance's sensible heat.
const RADIANT_SHARE: f64 = 0.6;

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
    overrides: Option<serde_json::Value>,
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
        overrides: run.overrides.clone(),
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
    convective_wh: f64,
    radiant_wh: f64,
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
            total.convective_wh += thermal.sensible_gain_w * DT_H;
            total.radiant_wh += thermal.radiant_gain_w * DT_H;
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
        let radiant = RADIANT_SHARE * split.sensible;
        for (kind, delivered, fraction) in [
            ("convective", total.convective_wh, split.sensible - radiant),
            ("radiant", total.radiant_wh, radiant),
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
        overrides: None,
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
        overrides: None,
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
        overrides: None,
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

/// One day of the cz2a parity building with `overrides` applied.
fn cz2a_day(overrides: Option<serde_json::Value>) -> Run {
    let fixture = project_root().join("tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh");
    Run {
        weather: fixture.join("weather.epw"),
        fixture,
        hpxml: "building.xml",
        schedule: "schedule.csv",
        start: "2023-01-01T00:00:00-07:00",
        days: 1,
        bldg_id: 1,
        overrides,
    }
}

/// Checks that `name`, stepped through `run`, gives the conditioned zone
/// `[convective, radiant, visible, latent]` fractions of its input.
fn assert_zone_split(run: &Run, name: &str, fractions: [f64; 4]) {
    let zone = conditioned_zone(&run.fixture.join(run.hpxml));
    let mut input = 0.0;
    let mut delivered = [0.0; 4];
    step_appliances(run, &[name], |_, _, ports| {
        input += input_wh(ports, FuelType::Electric);
        for thermal in ports.thermal.iter().filter(|t| t.zone == zone) {
            delivered[0] += thermal.sensible_gain_w * DT_H;
            delivered[1] += thermal.radiant_gain_w * DT_H;
            delivered[2] += thermal.shortwave_gain_w * DT_H;
            delivered[3] += thermal.latent_gain_w * DT_H;
        }
    });
    assert!(input > 0.0, "{name} runs");
    for ((kind, got), fraction) in ["convective", "radiant", "visible", "latent"]
        .iter()
        .zip(delivered)
        .zip(fractions)
    {
        let want = input * fraction;
        assert!(
            (got - want).abs() <= 1e-9 * input,
            "{name} {kind}: {got:.3} Wh, expected {want:.3} Wh ({fraction} of {input:.3} Wh)"
        );
    }
}

/// Indoor lighting of an HPXML building gives the conditioned zone all of
/// its power: 0.2 convective, 0.6 long-wave radiant and 0.2 visible
/// short-wave (OpenStudio-HPXML `model.rb` 249-250, `add_lights`).
#[test]
fn hpxml_lighting_splits_convective_radiant_and_visible() {
    assert_zone_split(&cz2a_day(None), "Indoor Lighting", [0.2, 0.6, 0.2, 0.0]);
}

/// The radiant and visible parts are shares of whatever sensible fraction
/// the load ends up with: an override below or above the default keeps the
/// appliance in the dwelling and scales its split.
#[test]
fn overridden_sensible_fraction_keeps_the_radiant_and_visible_shares() {
    for (name, sensible, latent, visible_share) in [
        ("Cooking Range", 0.3, 0.08, 0.0),
        ("Cooking Range", 0.9, 0.08, 0.0),
        ("Indoor Lighting", 0.5, 0.0, 0.2),
    ] {
        let overrides = serde_json::json!({ name: { "sensible_gain_fraction": sensible } });
        let radiant = 0.6 * sensible;
        let visible = visible_share * sensible;
        assert_zone_split(
            &cz2a_day(Some(overrides)),
            name,
            [sensible - radiant - visible, radiant, visible, latent],
        );
    }
}

/// Equipment whose init fails is a construction error naming it, not a
/// load the dwelling quietly runs without.
#[test]
fn equipment_init_failure_fails_the_dwelling() {
    let overrides = serde_json::json!({ "Cooking Range": { "sensible_gain_fraction": 1.5 } });
    let err = Dwelling::from_config(dwelling_config(&cz2a_day(Some(overrides))))
        .err()
        .expect("an impossible sensible fraction must fail construction");
    let message = err.to_string();
    assert!(
        message.contains("Cooking Range") && message.contains("sensible_gain_fraction"),
        "the error names the equipment and the cause, got: {message}"
    );
}

/// Every gain parameter a load reads takes an override, and the split
/// follows it: the HPXML spellings replace the resolved fraction, an
/// absolute radiant fraction sets the radiant part, and the shares scale
/// the sensible fraction. The range's resolved split is 0.72 sensible, 0.6
/// of it radiant, and 0.08 latent.
#[test]
fn every_gain_override_changes_the_split() {
    for (name, overrides, fractions) in [
        (
            "Cooking Range",
            serde_json::json!({ "frac_sensible": 0.3 }),
            [0.12, 0.18, 0.0, 0.08],
        ),
        (
            "Cooking Range",
            serde_json::json!({ "frac_latent": 0.2 }),
            [0.288, 0.432, 0.0, 0.2],
        ),
        (
            "Cooking Range",
            serde_json::json!({ "radiative_gain_fraction": 0.2 }),
            [0.52, 0.2, 0.0, 0.08],
        ),
        (
            "Cooking Range",
            serde_json::json!({ "radiant_share_of_sensible": 0.9 }),
            [0.072, 0.648, 0.0, 0.08],
        ),
        (
            "Indoor Lighting",
            serde_json::json!({ "visible_share_of_sensible": 0.0 }),
            [0.4, 0.6, 0.0, 0.0],
        ),
        (
            "Indoor Lighting",
            serde_json::json!({
                "radiant_share_of_sensible": 0.5,
                "visible_share_of_sensible": 0.5,
            }),
            [0.0, 0.5, 0.5, 0.0],
        ),
    ] {
        assert_zone_split(
            &cz2a_day(Some(serde_json::json!({ name: overrides }))),
            name,
            fractions,
        );
    }
}

/// An override the load cannot take fails construction naming the
/// parameter and the equipment: a key it does not read (a misspelling, a
/// renamed key, a spelling of the sensible fraction it no longer takes), a
/// gain value that is not a number, and one fraction given twice.
#[test]
fn an_override_the_load_cannot_take_fails_the_build() {
    for (name, overrides, key) in [
        (
            "Cooking Range",
            serde_json::json!({ "sensibel_gain_fraction": 0.1 }),
            "sensibel_gain_fraction",
        ),
        (
            "Indoor Lighting",
            serde_json::json!({ "visible_gain_fraction": 0.1 }),
            "visible_gain_fraction",
        ),
        (
            "Cooking Range",
            serde_json::json!({ "convective_gain_fraction": 0.5 }),
            "convective_gain_fraction",
        ),
        (
            "Cooking Range",
            serde_json::json!({ "radiant_share_of_sensible": "0.9" }),
            "radiant_share_of_sensible",
        ),
        (
            "Cooking Range",
            serde_json::json!({ "radiant_share_of_sensible": null }),
            "radiant_share_of_sensible",
        ),
        (
            "Cooking Range",
            serde_json::json!({ "frac_sensible": 0.3, "sensible_gain_fraction": 0.5 }),
            "frac_sensible",
        ),
    ] {
        let err = Dwelling::from_config(dwelling_config(&cz2a_day(Some(
            serde_json::json!({ name: overrides }),
        ))))
        .err()
        .unwrap_or_else(|| panic!("{name} {key}: the override must fail construction"));
        assert!(
            matches!(
                &err,
                hares_types::HaresError::InvalidEquipmentParameter { equipment, key: k, .. }
                    if equipment == name && k == key
            ),
            "{name} {key}: got {err}"
        );
    }
}

/// Each named load's electric input over the run, in Wh.
fn inputs_wh(run: &Run, names: &[&str]) -> Vec<f64> {
    let mut inputs = vec![0.0; names.len()];
    step_appliances(run, names, |_, idx, ports| {
        inputs[idx] += input_wh(ports, FuelType::Electric);
    });
    inputs
}

/// A scheduled load's monthly scale factor and its gas-schedule unit take an
/// override although the resolver set neither: the month factor scales the
/// load in its month.
#[test]
fn a_scheduled_load_takes_the_parameters_the_resolver_left_out() {
    let base = inputs_wh(&cz2a_day(None), &["Indoor Lighting"])[0];
    let doubled = inputs_wh(
        &cz2a_day(Some(serde_json::json!({
            "Indoor Lighting": { "month_multiplier_0": 2.0, "gas_schedule_is_w": false },
        }))),
        &["Indoor Lighting"],
    )[0];
    assert!(base > 0.0);
    assert!(
        (doubled - 2.0 * base).abs() <= 1e-9 * base,
        "January's factor 2 doubles the lighting: {base} Wh to {doubled} Wh"
    );
}

/// The wildcard, under either spelling, reaches every load that reads its
/// parameter and skips the equipment that does not (the typed furnace and
/// air conditioner, the event-driven appliances): the scheduled loads
/// double and the range does not change.
#[test]
fn a_wildcard_reaches_the_loads_that_read_it() {
    let names = ["Indoor Lighting", "Cooking Range"];
    let base = inputs_wh(&cz2a_day(None), &names);
    for wildcard in ["all", "*"] {
        let scaled = inputs_wh(
            &cz2a_day(Some(
                serde_json::json!({ wildcard: { "usage_multiplier": 2.0 } }),
            )),
            &names,
        );
        assert!(
            (scaled[0] - 2.0 * base[0]).abs() <= 1e-9 * base[0],
            "{wildcard}: the lighting doubles, {} Wh to {} Wh",
            base[0],
            scaled[0]
        );
        assert_eq!(
            scaled[1], base[1],
            "{wildcard}: the range reads no usage multiplier"
        );
    }
}

/// Both wildcard spellings, and a wildcard parameter no equipment reads,
/// fail construction; neither is dropped.
#[test]
fn a_wildcard_no_equipment_can_take_fails_the_build() {
    let both = Dwelling::from_config(dwelling_config(&cz2a_day(Some(serde_json::json!({
        "all": { "usage_multiplier": 2.0 },
        "*": { "usage_multiplier": 3.0 },
    })))))
    .err()
    .expect("two wildcards must fail construction");
    assert!(both.to_string().contains("both 'all' and '*'"), "{both}");

    for wildcard in ["all", "*"] {
        let err = Dwelling::from_config(dwelling_config(&cz2a_day(Some(serde_json::json!({
            wildcard: { "usage_multiplir": 2.0 },
        })))))
        .err()
        .expect("a misspelt wildcard parameter must fail construction");
        assert!(
            matches!(
                &err,
                hares_types::HaresError::InvalidEquipmentParameter { key, .. }
                    if key == "usage_multiplir"
            ) && err.to_string().contains(&format!("'{wildcard}'")),
            "{wildcard}: got {err}"
        );
    }
}

/// A gain parameter given an object fails construction naming it.
#[test]
fn a_nested_value_for_a_load_parameter_fails_the_build() {
    let err = Dwelling::from_config(dwelling_config(&cz2a_day(Some(serde_json::json!({
        "Cooking Range": { "radiant_share_of_sensible": { "value": 0.5 } },
    })))))
    .err()
    .expect("an object is no fraction");
    assert!(
        matches!(
            &err,
            hares_types::HaresError::InvalidEquipmentParameter { key, .. }
                if key == "radiant_share_of_sensible"
        ),
        "got {err}"
    );
}
