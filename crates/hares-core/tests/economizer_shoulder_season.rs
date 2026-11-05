//! The outdoor-air economizer's shoulder-season witnesses.
//!
//! The fixture is the CZ2A gas-furnace/AC dwelling on a two-week
//! mid-October Denver window: sunny days whose outdoor dry-bulb swings
//! across the fixture's 20 C changeover setpoint while the zone sits at
//! its 20 C small-commercial cooling setpoint. The AC runs the
//! small-commercial setpoint so the shoulder-season calls exist (a
//! residential 76 F dwelling never calls for cooling at a 5 C outdoor),
//! the mechanical ventilation fan is removed so the unit's damper is the
//! only outdoor-air path, and the witnesses pair the economizer run
//! against the same window without it: the damper's free-cooling steps
//! defer the compressor, the zone holds its setpoint band while they run,
//! and the run's energy shows the free-cooling savings.

use arrow::array::{Array, Float64Array};
use arrow::record_batch::RecordBatch;
use hares_core::{Dwelling, DwellingConfig, DwellingSettings};
use hares_input::{DefaultsSource, InputSource, SimulationConfig, WeatherInput};
use hares_types::OperatingMode;
use serde_json::json;
use std::path::PathBuf;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

const DWELLING_DIR: &str = "tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh";

fn sim_toml() -> String {
    // Two weeks from 2023-10-07 at 900 s: the window's sunny days carry
    // the zone to its 20 C cooling setpoint while the outdoor dry-bulb
    // swings across the fixture's 20 C changeover setpoint.
    r#"
start_time = "2023-10-07T00:00:00-06:00"
duration = 1209600
time_res = 900
output_chunk_size = 128
master_seed = 0
metrics = true
warmup = "disabled"

[[reports]]
name = "frame"
selection = { verbosity = 8 }
destination = "memory"
"#
    .to_string()
}

fn overrides(economizer: bool) -> Option<serde_json::Value> {
    // The shoulder-season premise needs cooling calls while the outdoor air
    // sits below the changeover: the fixture's AC runs the small-commercial
    // cooling setpoint (20 C) and the changeover rides with it (the RTU's
    // aggressive free-cooling setting; the ASHRAE 10 C default stays pinned
    // by the unit-level witnesses).
    let base = json!({
        "Air Conditioner": {
            "setpoint": { "cooling_setpoint_c": 20.0 }
        }
    });
    if !economizer {
        return Some(base);
    }
    Some(json!({
        "Air Conditioner": {
            "setpoint": { "cooling_setpoint_c": 20.0 },
            "economizer": {
                "changeover_setpoint_c": 20.0,
                "damper": {
                    "minimum_oa_flow_m3_s": 0.05,
                    "design_oa_flow_m3_s": 0.5,
                    "max_oa_dew_point_c": 13.0,
                    "min_oa_temp_c": -5.0
                }
            }
        }
    }))
}

/// The dwelling built from the fixture's inputs, the mechanical
/// ventilation removed so the AC's damper is the only outdoor-air path.
fn built_dwelling(economizer: bool, without_ventilation: bool) -> Dwelling {
    let fixture = project_root().join(DWELLING_DIR);
    let config = DwellingConfig {
        hpxml: InputSource::Path(fixture.join("building.xml")),
        schedule: Some(InputSource::Path(fixture.join("schedule.csv"))),
        weather: WeatherInput::Path(fixture.join("weather.epw")),
        defaults: DefaultsSource::directory(project_root().join("python/hares/defaults")),
        settings: DwellingSettings::new(
            SimulationConfig::from_toml(&sim_toml()).expect("sim config parses"),
            overrides(economizer),
            1,
            None,
        ),
    };
    let mut builder = hares_core::dwelling::DwellingBlueprintBuilder::from_config(config)
        .expect("blueprint builds");
    if without_ventilation {
        builder.remove_equipment_by_end_use(&[hares_types::EndUse::VENTILATION]);
    }
    builder
        .freeze()
        .and_then(|blueprint| blueprint.build())
        .expect("dwelling builds")
}

struct Series {
    values: Vec<f64>,
}

fn column(batches: &[RecordBatch], name: &str) -> Series {
    let mut values = Vec::new();
    for b in batches {
        let col = b
            .column_by_name(name)
            .unwrap_or_else(|| panic!("frame carries '{name}'"));
        let arr = col
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap_or_else(|| panic!("'{name}' is f64"));
        for i in 0..arr.len() {
            values.push(if arr.is_valid(i) {
                arr.value(i)
            } else {
                f64::NAN
            });
        }
    }
    Series { values }
}

/// The step interval the fixture runs at.
const STEP_S: f64 = 900.0;

/// The fixture's simulation steps: the 1209600 s window at the 900 s step.
const STEPS: usize = 1344;

fn run_frames(economizer: bool) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
    let mut dwelling = built_dwelling(economizer, true);
    let _catalogue = dwelling.catalogue();
    let result = dwelling.simulate().expect("run completes");
    let batches: Vec<RecordBatch> = result.reports[0].batches().to_vec();
    let zone = column(&batches, "Temperature - Indoor (C)").values;
    let oat = column(&batches, "Outdoor Dry Bulb (C)").values;
    let cooling_kw = column(&batches, "HVAC Cooling End Use Electric Power (kW)").values;
    (zone, oat, cooling_kw)
}

/// One step's AC state as the unit publishes it: the thermostat's mode,
/// the compressor's and fan's draws, the damper's position and flow, and
/// the free-cooling credit. The frame's fixed column catalogue does not
/// carry the economizer's own keys, so the run-level witnesses read the
/// unit's telemetry through the environment the step publishes.
struct EconomizerStep {
    outdoor_c: f64,
    zone_c: f64,
    mode: f64,
    compressor_kw: f64,
    fan_kw: f64,
    damper_position: f64,
    oa_flow_m3_s: f64,
    free_cooling_w: f64,
}

fn run_economizer_steps() -> Vec<EconomizerStep> {
    let mut dwelling = built_dwelling(true, true);
    let _catalogue = dwelling.catalogue();
    let mut rows = Vec::with_capacity(STEPS);
    for _ in 0..STEPS {
        dwelling.step().expect("run completes");
        let env = dwelling.latest_env();
        let tel = env
            .equipment_telemetry
            .get("Air Conditioner")
            .expect("the economizer run carries its Air Conditioner's telemetry");
        let get = |key: &str| {
            tel.get(key)
                .unwrap_or_else(|| panic!("the Air Conditioner publishes '{key}'"))
        };
        rows.push(EconomizerStep {
            outdoor_c: env.weather.outdoor_temp_c,
            zone_c: env.zones[0].temperature_c,
            mode: get("operating_mode"),
            compressor_kw: get("compressor_kw"),
            fan_kw: get("fan_kw"),
            damper_position: get("economizer_oa_damper_position"),
            oa_flow_m3_s: get("economizer_oa_flow_m3_s"),
            free_cooling_w: get("economizer_free_cooling_w"),
        });
    }
    rows
}

#[test]
fn economizer_defers_the_cooling_call_below_the_changeover_and_zone_holds_setpoint() {
    let (_without_zone, without_oat, without_kw) = run_frames(false);
    let (_with_zone, _with_oat, with_kw) = run_frames(true);

    // The damper-free run's cooling-call steps below the changeover: the
    // steps whose compressor the economizer must defer. A cooling-call step
    // is one where the damper-free run drew cooling power; the shoulder-
    // season premise needs those steps to also sit below the changeover.
    let mut deferred_steps = 0usize;
    let mut savings_kwh = 0.0_f64;
    let hours = STEP_S / 3600.0;
    for i in 0..without_kw.len() {
        let was_cooling = without_kw[i] > 0.0;
        let below_changeover = without_oat[i] < 20.0;
        if was_cooling && below_changeover {
            deferred_steps += 1;
            savings_kwh += (without_kw[i] - with_kw[i]) * hours;
        }
    }
    assert!(
        deferred_steps >= 8,
        "the shoulder-season window must exercise the economizer's changeover; \\
         found {deferred_steps} damper-free cooling steps below the changeover"
    );
    assert!(
        savings_kwh > 0.0,
        "the economizer must defer the compressor on the below-changeover \\
         cooling steps: the paired savings sum to {savings_kwh:.4} kWh"
    );

    // The economizer's own free-cooling steps: the damper at its full-open
    // position, the fan running and the compressor deferred to zero while
    // the outdoor air sits below the changeover. On those steps the zone
    // must hold the 20 C call's band: a step that delivered nothing would
    // let the zone fall through the bound, and a step whose credit failed
    // to cover the call would run the compressor (the selection excludes
    // it, so the count assertion below is what pins that case).
    let rows = run_economizer_steps();
    let mut free_cooling_steps = 0usize;
    for (i, row) in rows.iter().enumerate() {
        if row.outdoor_c < 20.0
            && row.damper_position == 1.0
            && row.compressor_kw == 0.0
            && row.fan_kw > 0.0
        {
            free_cooling_steps += 1;
            assert!(
                row.free_cooling_w > 0.0,
                "step {i}: the deferred compressor step must carry the \
                 damper's free-cooling credit, got {}",
                row.free_cooling_w
            );
            assert!(
                (19.0..=21.0).contains(&row.zone_c),
                "step {i}: the zone must hold the 20 C call's band while the \
                 economizer carries the load: zone {:.2} C at OAT {:.2} C",
                row.zone_c,
                row.outdoor_c
            );
        }
    }
    assert!(
        free_cooling_steps >= 8,
        "the economizer's run must defer the compressor on below-changeover \
         steps; found {free_cooling_steps}"
    );
}

#[test]
fn economizer_damper_returns_to_minimum_above_the_changeover() {
    let rows = run_economizer_steps();
    // The damper's two positions: the minimum flow over the design flow
    // (0.05 / 0.5) and the full open.
    let minimum_position = 0.1;
    let mut above_changeover = 0usize;
    let mut below_cooling_open = 0usize;
    for (i, row) in rows.iter().enumerate() {
        if row.outdoor_c > 21.0 {
            above_changeover += 1;
            assert_eq!(
                row.damper_position, minimum_position,
                "step {i}: OAT {:.2} C is above the changeover, the damper \
                 must rest at the minimum position",
                row.outdoor_c
            );
        }
        if row.outdoor_c < 19.0 && row.mode == OperatingMode::Cooling.as_code() {
            below_cooling_open += 1;
            assert_eq!(
                row.damper_position, 1.0,
                "step {i}: OAT {:.2} C is below the changeover with a cooling \
                 call, the damper must open to its free-cooling position",
                row.outdoor_c
            );
            if row.fan_kw > 0.0 {
                assert!(
                    row.oa_flow_m3_s > 0.0,
                    "step {i}: the open damper must pass the design flow while \
                     the fan runs, got {} m3/s",
                    row.oa_flow_m3_s
                );
            }
        }
    }
    assert!(
        above_changeover >= 8 && below_cooling_open >= 8,
        "the window must exercise both sides of the changeover, found \
         {above_changeover} above and {below_cooling_open} below"
    );
}

#[test]
fn economizer_shows_free_cooling_savings_against_the_damper_free_run() {
    // The same window with and without the economizer: the run's cooling
    // electric energy drops (the free-cooling savings), and the economizer's
    // compressor-hours cannot exceed the damper-free run's.
    let (_with_zone, _oat, with_kw) = run_frames(true);
    let (_without_zone, _without_oat, without_kw) = run_frames(false);

    let hours = STEP_S / 3600.0;
    let sum_kwh =
        |kw: &[f64]| -> f64 { kw.iter().filter(|v| v.is_finite()).map(|v| v * hours).sum() };
    let without_kwh = sum_kwh(&without_kw);
    let with_kwh = sum_kwh(&with_kw);
    assert!(
        with_kwh < without_kwh,
        "the economizer must show free-cooling savings: with {with_kwh:.3} kWh \\
         >= without {without_kwh:.3} kWh"
    );

    // The compressor-hours delta: the design's second measurement. The
    // economizer's run keeps the compressor off on the free-cooling steps,
    // so its compressor-hours cannot exceed the damper-free run's.
    let with_hours: f64 = with_kw.iter().filter(|v| **v > 0.0).count() as f64 * hours;
    let without_hours: f64 = without_kw.iter().filter(|v| **v > 0.0).count() as f64 * hours;
    assert!(
        with_hours <= without_hours,
        "the economizer's compressor-hours {with_hours} must not exceed the \\
         damper-free run's {without_hours}"
    );
}
