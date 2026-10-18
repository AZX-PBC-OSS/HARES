//! The PV capacity enters and is checked in watts (SI), end to end:
//! the HPXML `MaxPowerOutput` value equals the config's `capacity_w` on
//! bits; an override carrying the removed `capacity_kw` spelling fails
//! the dwelling's construction naming `PvConfig.capacity_w`.

use hares_core::{Dwelling, DwellingConfig, DwellingSettings};
use hares_io::{DefaultsSource, InputSource, WeatherInput};
use serde_json::json;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn sim_config() -> hares_io::SimulationConfig {
    hares_io::SimulationConfig::from_toml(
        r#"
start_time = "2023-06-01T00:00:00-07:00"
duration = 21600
time_res = 900
"#,
    )
    .expect("the sim config parses")
}

/// The base-pv-battery sample dwelling with the battery's declared
/// self-consumption control (the DER golden fixture's inputs, run for a
/// few steps).
fn pv_battery_config(overrides: serde_json::Value) -> DwellingConfig {
    let root = repo_root();
    DwellingConfig {
        hpxml: InputSource::Path(
            root.join("tests/fixtures/hpxml/ochre_samples/base-pv-battery.xml"),
        ),
        schedule: Some(InputSource::Path(
            root.join("data/examples/BEopt_example_schedule.csv"),
        )),
        weather: WeatherInput::Path(
            root.join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        ),
        defaults: DefaultsSource::directory(root.join("python/hares/defaults")),
        settings: DwellingSettings::new(sim_config(), Some(overrides), 1, None),
    }
}

/// An override carrying the removed `capacity_kw` spelling fails the
/// construction naming `PvConfig.capacity_w`: end to end, the removed-key
/// walk at the override merge runs before any equipment is built. The
/// battery's declared control rides along (the OD-8 rule) so the walk's
/// error is the construction's own failure.
#[test]
fn pv_capacity_kw_override_fails_construction() {
    let config = pv_battery_config(json!({
        "PV": {"capacity_kw": 5.0},
        "Battery": {"bms_mode": "\"Manual\""},
    }));
    let error = Dwelling::from_config(config)
        .map(|_| ())
        .expect_err("the removed key fails construction");
    assert!(
        error.to_string().contains("PvConfig.capacity_w (W)"),
        "the error must name the replacement, got: {error}"
    );
}

/// The HPXML PV systems enter in watts: `base-pv.xml`'s two PV systems'
/// `MaxPowerOutput` values (4000 W and 1500 W) equal their resolved
/// `nameplate_w` (read back through the blueprint's validate, the same
/// read the golden's resolved product captures) on bits.
#[test]
fn hpxml_pv_capacity_enters_in_watts() {
    let root = repo_root();
    let hpxml = root.join("tests/fixtures/hpxml/ochre_samples/base-pv.xml");
    let schedule = root.join("data/examples/BEopt_example_schedule.csv");
    let weather = root.join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw");
    let config = DwellingConfig {
        hpxml: InputSource::Path(hpxml),
        schedule: Some(InputSource::Path(schedule)),
        weather: WeatherInput::Path(weather),
        defaults: DefaultsSource::directory(root.join("python/hares/defaults")),
        settings: DwellingSettings::new(sim_config(), Some(json!({})), 1, None),
    };
    let blueprint = hares_core::DwellingBlueprint::from_config(config).expect("the input parses");
    let model = blueprint.validate().expect("the assembly resolves");
    let pvs: Vec<(String, f64)> = model
        .equipment
        .iter()
        .filter(|eq| eq.parameters.iter().any(|p| p.name == "nameplate_w"))
        .map(|eq| {
            (
                eq.name.clone(),
                eq.parameters
                    .iter()
                    .find(|p| p.name == "nameplate_w")
                    .expect("the nameplate filter above")
                    .value,
            )
        })
        .collect();
    assert_eq!(pvs.len(), 2, "base-pv.xml declares two PV systems");
    // 4000.0 and 1500.0 W, the document's MaxPowerOutput values, on bits.
    let mut nameplates: Vec<u64> = pvs.iter().map(|(_, w)| w.to_bits()).collect();
    nameplates.sort_by(|a, b| a.cmp(b).then(std::cmp::Ordering::Equal));
    assert_eq!(nameplates, vec![1500.0f64.to_bits(), 4000.0f64.to_bits()]);
}
