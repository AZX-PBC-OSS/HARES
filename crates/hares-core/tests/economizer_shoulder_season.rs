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

#[test]
fn economizer_defers_the_cooling_call_below_the_changeover_and_zone_holds_setpoint() {
    let (_without_zone, without_oat, without_kw) = run_frames(false);
    let (with_zone, _with_oat, with_kw) = run_frames(true);

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

    // The zone holds on the economizer's own cooling-call steps: a step
    // where the run sits at or above the cooling call's edge with the
    // compressor deferred must not run away. The thermostat's latched hold
    // and the damper's free cooling together bound the zone near the call's
    // edge (measured 23.1 C worst-case on this window); the assert pins
    // that hold with margin rather than a runaway.
    let mut with_call_steps = 0usize;
    for i in 0..with_zone.len() {
        if with_zone[i] > 20.9 && with_kw[i] == 0.0 && _with_oat[i] < 20.0 {
            with_call_steps += 1;
            assert!(
                with_zone[i] <= 23.5,
                "step {i}: the zone must hold setpoint while the economizer \\
                 carries the load: zone {:.2} C at OAT {:.2} C",
                with_zone[i],
                _with_oat[i]
            );
        }
    }
    assert!(
        with_call_steps >= 8,
        "the economizer's run must hold the zone at the cooling call's edge \\
         on below-changeover steps; found {with_call_steps}"
    );
}

#[test]
fn economizer_damper_returns_to_minimum_above_the_changeover() {
    let mut dwelling = built_dwelling(true, true);
    let _catalogue = dwelling.catalogue();
    let result = dwelling.simulate().expect("run completes");
    let batches: Vec<RecordBatch> = result.reports[0].batches().to_vec();

    let oat = column(&batches, "Outdoor Dry Bulb (C)");
    let forced_vent = column(&batches, "Forced Ventilation Heat Gain - Indoor (W)");

    // Above the changeover the damper rests at its minimum position: the
    // damper's implied outdoor-air flow (the forced-vent column's magnitude
    // over the density/heat-capacity/temperature-difference product) must
    // sit at the minimum position's share of the design flow, never the
    // full-open's.
    let rho = 1.2_f64;
    let cp = 1005.0_f64;
    let minimum_flow_m3_s = 0.05_f64;
    let design_flow_m3_s = 0.5_f64;
    for i in 0..oat.values.len() {
        if oat.values[i] > 22.0 && forced_vent.values[i] < 0.0 && forced_vent.values[i].is_finite()
        {
            let load_per_flow_w = rho * cp * (26.0 - oat.values[i]);
            if load_per_flow_w > 1.0 {
                let implied_flow_m3_s = forced_vent.values[i].abs() / load_per_flow_w;
                assert!(
                    implied_flow_m3_s < (minimum_flow_m3_s + design_flow_m3_s) * 0.5,
                    "step {i}: OAT {:.2} C is above the changeover, the damper's \\
                     implied flow {:.4} m3/s must be the minimum position's share",
                    oat.values[i],
                    implied_flow_m3_s
                );
            }
        }
    }
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
