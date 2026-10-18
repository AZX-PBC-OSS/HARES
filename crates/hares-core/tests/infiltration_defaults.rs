//! The infiltration defaults follow the OpenStudio-HPXML rules (the OD-10
//! numeric-commit acceptance).
//!
//! When the HPXML's air infiltration measurement declares no
//! `<InfiltrationHeight>`, the AIM-2 model runs at OpenStudio-HPXML's
//! estimate (`defaults.rb:5789-5856`, `get_infiltration_height_and_volume`:
//! the average ceiling height per conditioned floor above grade, half a
//! foot of rim joist and subfloor per added storey, the tallest attic and
//! the tallest above-grade foundation within the infiltration volume, the
//! foundation term dropped for a walkout basement). When the measurement
//! declares no `<HasFlueOrChimneyInConditionedSpace>`, the flag is
//! OpenStudio-HPXML's rule over the resolved heating systems and water
//! heaters (`defaults.rb:5096-5132`). When the site declares no
//! `<ShieldingofHome>`, the class is the facility type's default
//! (`defaults.rb:822-830`).
//!
//! The fixture is `tests/fixtures/hpxml/ochre_samples/base.xml`: 2700 ft2
//! conditioned floor area, two conditioned floors with one above grade, an
//! 8 ft average ceiling height, a conditioned basement (foundation wall 8
//! ft high, 7 ft below grade), an unvented attic outside the infiltration
//! volume, ACH 3.0, no infiltration height, no flue element; its furnace
//! is a gas furnace at AFUE 0.92 and its water heater is electric.

use std::path::PathBuf;

use hares_core::{Dwelling, DwellingConfig, DwellingSettings};
use hares_io::{DefaultsSource, InputSource, WeatherInput};

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates dir")
        .parent()
        .expect("repo root")
        .to_path_buf()
}

fn build(hpxml: PathBuf) -> Result<Dwelling, hares_types::HaresError> {
    let sim_toml = r#"
start_time = "2023-01-15T00:00:00-07:00"
duration = 3600
time_res = 900
warmup = "disabled"
master_seed = 0
"#
    .to_string();
    let config = DwellingConfig {
        hpxml: InputSource::Path(hpxml),
        schedule: None,
        weather: WeatherInput::Path(
            project_root().join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        ),
        defaults: DefaultsSource::directory(project_root().join("python/hares/defaults")),
        settings: DwellingSettings::new(
            hares_io::SimulationConfig::from_toml(&sim_toml).expect("the config parses"),
            None,
            1,
            None,
        ),
    };
    Dwelling::from_config(config)
}

fn dwelling(hpxml: PathBuf) -> Dwelling {
    build(hpxml).expect("the dwelling builds")
}

fn build_with_overrides(
    hpxml: PathBuf,
    overrides: serde_json::Value,
) -> Result<Dwelling, hares_types::HaresError> {
    let sim_toml = r#"
start_time = "2023-01-15T00:00:00-07:00"
duration = 3600
time_res = 900
warmup = "disabled"
master_seed = 0
"#
    .to_string();
    let config = DwellingConfig {
        hpxml: InputSource::Path(hpxml),
        schedule: None,
        weather: WeatherInput::Path(
            project_root().join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        ),
        defaults: DefaultsSource::directory(project_root().join("python/hares/defaults")),
        settings: DwellingSettings::new(
            hares_io::SimulationConfig::from_toml(&sim_toml).expect("the config parses"),
            Some(overrides),
            1,
            None,
        ),
    };
    Dwelling::from_config(config)
}

/// base.xml with one element's text replaced, written to a temporary
/// directory.
fn edited_base(dir: &tempfile::TempDir, replace: &str, with: &str) -> PathBuf {
    let original =
        std::fs::read_to_string(project_root().join("tests/fixtures/hpxml/ochre_samples/base.xml"))
            .expect("the fixture reads");
    let edited = original.replace(replace, with);
    assert!(
        edited != original || replace.is_empty(),
        "the edit must change the document: {replace}"
    );
    let path = dir.path().join("base-edited.xml");
    std::fs::write(&path, edited).expect("the edited HPXML writes");
    path
}

/// The AIM-2 infiltration record the dwelling assembled with.
fn infiltration_record(dwelling: &Dwelling) -> (f64, bool, String) {
    let model = dwelling.resolved_model();
    let height_m = model
        .infiltration
        .height_m
        .expect("the AIM-2 ran and recorded its height");
    let has_flue = model
        .infiltration
        .has_flue
        .expect("the AIM-2 ran and recorded its flue flag");
    let shielding = model
        .infiltration
        .shielding
        .expect("the AIM-2 ran and recorded its shielding");
    (height_m, has_flue, format!("{shielding:?}"))
}

/// The fixture's inputs the estimate reads, asserted first so a fixture
/// change cannot leave the expectation stale, and the height they
/// estimate: 9.75 ft (2.9718 m), the value OpenStudio-HPXML v1.12.0
/// defaults for its own base.xml with the height removed
/// (`HPXMLtoOpenStudio/tests/test_defaults.rb`,
/// `test_infiltration_height_and_volume`, within its relative 0.01).
#[test]
fn missing_infiltration_height_follows_openstudio_hpxml() {
    let fixture = project_root().join("tests/fixtures/hpxml/ochre_samples/base.xml");
    let xml = std::fs::read_to_string(&fixture).expect("the fixture reads");
    assert!(
        !xml.contains("InfiltrationHeight"),
        "the fixture declares no InfiltrationHeight"
    );
    assert!(xml.contains("<ConditionedFloorArea>2700.0"), "2700 ft2");
    assert!(
        xml.contains("<NumberofConditionedFloors>2.0"),
        "two conditioned floors"
    );
    assert!(
        xml.contains("<NumberofConditionedFloorsAboveGrade>1.0"),
        "one above grade"
    );
    assert!(
        xml.contains("<AverageCeilingHeight>8.0"),
        "an 8 ft average ceiling"
    );
    assert!(
        xml.contains("<Conditioned>true</Conditioned>"),
        "a conditioned basement"
    );
    assert!(xml.contains("<Vented>false</Vented>"), "an unvented attic");
    assert!(
        xml.contains("<WithinInfiltrationVolume>false</WithinInfiltrationVolume>"),
        "the attic is declared outside the infiltration volume"
    );

    let dwelling = dwelling(fixture);
    let (height_m, _, _) = infiltration_record(&dwelling);
    let expected_m = 9.75_f64 * 0.3048;
    let relative = (height_m - expected_m).abs() / expected_m;
    assert!(
        relative < 0.01,
        "the estimate is 9.75 ft within a relative 0.01: {height_m} m against {expected_m} m"
    );
}

/// The flue rule over the resolved systems: at the fixture's own AFUE 0.92
/// (and an electric water heater) no atmospheric system qualifies and the
/// flag is false; with the furnace's AFUE edited to 0.80 the furnace is
/// atmospheric in conditioned space and the flag is true.
#[test]
fn missing_flue_follows_openstudio_hpxml() {
    let baseline = dwelling(project_root().join("tests/fixtures/hpxml/ochre_samples/base.xml"));
    let (_, has_flue, _) = infiltration_record(&baseline);
    assert!(!has_flue, "AFUE 0.92 and an electric water heater: no flue");

    let dir = tempfile::tempdir().expect("temp dir");
    let edited = edited_base(&dir, "<Value>0.92</Value>", "<Value>0.80</Value>");
    let edited_dwelling = dwelling(edited);
    let (_, has_flue, _) = infiltration_record(&edited_dwelling);
    assert!(
        has_flue,
        "the AFUE 0.80 gas furnace in conditioned space is atmospheric: flue"
    );
}

/// The flue rule reads the POST-merge efficiency: the flag follows the
/// efficiency the engine runs, so an `afue` override that lifts the
/// furnace out of the flue range clears it and one that drops it below
/// sets it. The pre-merge read would leave the flag on the resolved
/// efficiency while the engine ran the override.
#[test]
fn the_flue_rule_reads_the_overridden_efficiency() {
    use serde_json::json;

    // The 0.78 AFUE home: the rule fires. The 0.90 override lifts the
    // furnace out of the flue range: the flag clears.
    let dir = tempfile::tempdir().expect("temp dir");
    let edited = edited_base(&dir, "<Value>0.92</Value>", "<Value>0.78</Value>");
    let (_, has_flue, _) = infiltration_record(&dwelling(edited.clone()));
    assert!(has_flue, "the resolved AFUE 0.78 is below 0.89: flue");
    let overridden = build_with_overrides(edited, json!({"Gas Furnace": {"afue": 0.90}}))
        .expect("the dwelling builds");
    let (_, has_flue, _) = infiltration_record(&overridden);
    assert!(
        !has_flue,
        "the engine runs the overridden AFUE 0.90: no flue"
    );

    // The committed home's AFUE 0.92: no flue. The 0.78 override drops it
    // into the flue range: the flag sets.
    let base = project_root().join("tests/fixtures/hpxml/ochre_samples/base.xml");
    let (_, has_flue, _) = infiltration_record(&dwelling(base.clone()));
    assert!(!has_flue, "the resolved AFUE 0.92 clears 0.89: no flue");
    let overridden = build_with_overrides(base, json!({"Gas Furnace": {"afue": 0.78}}))
        .expect("the dwelling builds");
    let (_, has_flue, _) = infiltration_record(&overridden);
    assert!(has_flue, "the engine runs the overridden AFUE 0.78: flue");
}

/// The shielding follows the facility type when the HPXML declares no
/// `ShieldingofHome`: an apartment unit is well-shielded (the
/// single-family attached home too), a detached home is normal, and a
/// building with neither element fails construction naming the facility
/// type.
#[test]
fn missing_shielding_follows_facility_type() {
    let multifamily =
        project_root().join("tests/fixtures/hpxml/ochre_samples/base-bldgtype-multifamily.xml");
    let xml = std::fs::read_to_string(&multifamily).expect("the fixture reads");
    assert!(
        !xml.contains("ShieldingofHome"),
        "the fixture declares no ShieldingofHome"
    );
    assert!(
        xml.contains("<ResidentialFacilityType>apartment unit</ResidentialFacilityType>"),
        "the fixture is an apartment unit"
    );
    let apartment = dwelling(multifamily);
    let (_, _, shielding) = infiltration_record(&apartment);
    assert_eq!(shielding, "WellShielded", "an apartment unit");

    let detached = dwelling(project_root().join("tests/fixtures/hpxml/ochre_samples/base.xml"));
    let (_, _, shielding) = infiltration_record(&detached);
    assert_eq!(shielding, "Normal", "the detached home");

    let stripped = std::fs::read_to_string(
        project_root().join("tests/fixtures/hpxml/ochre_samples/base-bldgtype-multifamily.xml"),
    )
    .expect("the fixture reads")
    .replace(
        "<ResidentialFacilityType>apartment unit</ResidentialFacilityType>",
        "",
    );
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("no-facility-type.xml");
    std::fs::write(&path, stripped).expect("the stripped HPXML writes");
    let err = match build(path) {
        Err(err) => err,
        Ok(_) => {
            panic!("a building with neither ShieldingofHome nor ResidentialFacilityType must fail")
        }
    };
    assert!(
        err.to_string().contains("ResidentialFacilityType"),
        "the error names the facility type: {err}"
    );
}
