//! The price signal pipeline: the setter and the per-field update forms,
//! the applied-price precedence against the tariff evaluator, and the
//! environment view the price consumers read (the EV driver's V2G and
//! price logic among them, `actors/ev_driver/v2g.rs`).

use hares_core::{Dwelling, DwellingConfig, DwellingSettings};
use hares_input::{DefaultsSource, InputSource, WeatherInput};
use hares_types::PriceSignal;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn sim_config() -> hares_input::SimulationConfig {
    hares_input::SimulationConfig::from_toml(
        r#"
start_time = "2023-06-01T00:00:00-07:00"
duration = 21600
time_res = 900
"#,
    )
    .expect("the sim config parses")
}

fn dwelling() -> Dwelling {
    let root = repo_root();
    Dwelling::from_config(DwellingConfig {
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
        settings: DwellingSettings::new(
            sim_config(),
            Some(serde_json::json!({"Battery": {"bms_mode": "\"Manual\""}})),
            1,
            None,
        ),
    })
    .expect("construction")
}

/// An update touching one field preserves the stored others: the export
/// price moves and the import price survives, and the merged signal flows
/// to the environment view whole. The red state at the parent ran this
/// same one-field act through the only form that existed (the
/// whole-struct replace) and wiped the import price.
#[test]
fn a_one_field_price_update_preserves_the_stored_fields() {
    let mut dwelling = dwelling();
    dwelling.set_price_signal(PriceSignal {
        electricity_price: Some(0.2),
        export_price: Some(0.05),
        ghg_intensity: None,
    });
    dwelling.update_price_signal(PriceSignal {
        electricity_price: None,
        export_price: Some(0.06),
        ghg_intensity: None,
    });
    dwelling.step().expect("step");
    let applied = &dwelling.latest_env().price_signal;
    assert_eq!(
        applied.electricity_price,
        Some(0.2),
        "the export-only update preserves the stored import price"
    );
    assert_eq!(
        applied.export_price,
        Some(0.06),
        "the update's named field lands in the environment view"
    );

    // The replace form stays the replace form: the caller holding the
    // whole signal replaces whole.
    dwelling.set_price_signal(PriceSignal {
        electricity_price: Some(0.21),
        export_price: None,
        ghg_intensity: None,
    });
    dwelling.step().expect("step");
    let replaced = &dwelling.latest_env().price_signal;
    assert_eq!(
        replaced.electricity_price,
        Some(0.21),
        "the replace form sets the named field"
    );
    assert_eq!(
        replaced.export_price, None,
        "the replace form clears the fields the caller did not carry"
    );
}
