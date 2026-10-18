//! PV roof attachment and coverage, read from the typed `PvConfig`:
//! the roof a PV attaches to is matched by the orientation the envelope
//! already registered for that roof's solar gains, the coverage the
//! environment scales is the attached capacity's collector area over the
//! roof's area, and every failure names its inputs.

use hares_core::{COLLECTOR_AREA_PER_W, Dwelling, DwellingConfig, DwellingSettings};
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

/// The base-pv sample's config from the given HPXML text: the file is
/// written to a temp path named for the test, so tests editing the
/// document (the roof's azimuth, a system's tilt or capacity) never share
/// a file.
fn base_pv_config(name: &str, xml: &str) -> DwellingConfig {
    let root = repo_root();
    let path = std::env::temp_dir().join(format!(
        "hares-pv-roof-coverage-{name}-{}.xml",
        std::process::id()
    ));
    std::fs::write(&path, xml).expect("write the edited base-pv.xml");
    DwellingConfig {
        hpxml: InputSource::Path(path),
        schedule: Some(InputSource::Path(
            root.join("data/examples/BEopt_example_schedule.csv"),
        )),
        weather: WeatherInput::Path(
            root.join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        ),
        defaults: DefaultsSource::directory(root.join("python/hares/defaults")),
        settings: DwellingSettings::new(sim_config(), Some(json!({})), 1, None),
    }
}

fn base_pv_xml() -> String {
    let root = repo_root();
    std::fs::read_to_string(root.join("tests/fixtures/hpxml/ochre_samples/base-pv.xml"))
        .expect("base-pv.xml reads")
}

/// The base-pv sample's roof in the environment's registered geometry: the
/// one omnidirectional surface pitched like the roof (the envelope
/// registers a roof with no azimuth as omnidirectional; walls are tilt 90).
fn omni_roof(dwelling: &Dwelling) -> hares_core::environment::SurfaceGeometry {
    pitched_roof(dwelling)
        .find(|s| s.omni_directional)
        .cloned()
        .expect("the sample's roof is registered omnidirectional")
}

/// The registered surfaces pitched like the sample's roof (tilt
/// `atan(6 / 12)` from its `<Pitch>6.0</Pitch>`): the roof among them,
/// walls being tilt 90.
fn pitched_roof(
    dwelling: &Dwelling,
) -> impl Iterator<Item = &hares_core::environment::SurfaceGeometry> {
    let roof_tilt = (6.0_f64 / 12.0).atan().to_degrees();
    dwelling
        .environment
        .surface_geometry()
        .iter()
        .filter(move |s| s.area_m2 > 50.0 && (s.tilt_deg - roof_tilt).abs() < 1.0)
}

/// Every warning the build raised, construction-time warnings included.
fn build_warnings(dwelling: &mut Dwelling) -> Vec<String> {
    let mut messages: Vec<String> = dwelling
        .warnings
        .run_record()
        .0
        .iter()
        .map(|w| w.message.clone())
        .collect();
    messages.extend(dwelling.take_warnings().iter().map(|w| w.message.clone()));
    messages
}

/// `base-pv.xml`, whose single roof carries no azimuth: both arrays
/// (tilt 20° against the roof's 26.6°) attach to the roof through their
/// typed config, no warning names either, and the environment's coverage
/// of the roof's surface equals both capacities' collector area over the
/// roof's area.
#[test]
fn hpxml_pv_registers_roof_coverage() {
    let dwelling =
        Dwelling::from_config(base_pv_config("omni", &base_pv_xml())).expect("the dwelling builds");
    let roof = omni_roof(&dwelling);
    let expected =
        4000.0 * COLLECTOR_AREA_PER_W / roof.area_m2 + 1500.0 * COLLECTOR_AREA_PER_W / roof.area_m2;
    let coverage = dwelling.environment.pv_roof_coverage(roof.surface_id);
    assert_eq!(
        coverage,
        Some(expected),
        "both arrays' collector area must cover the roof: surface {} area {} m²",
        roof.surface_id,
        roof.area_m2
    );
}

/// The same run raises no attachment warning: both arrays attach, so the
/// warning log names neither.
#[test]
fn hpxml_pv_attachment_raises_no_warning() {
    let mut dwelling =
        Dwelling::from_config(base_pv_config("omni-quiet", &base_pv_xml())).expect("builds");
    let warnings = build_warnings(&mut dwelling);
    for pv in ["PV #1", "PV #2"] {
        assert!(
            !warnings
                .iter()
                .any(|m| m.contains(pv) && m.contains("attaches to no roof")),
            "no warning may name {pv} when it attaches: {warnings:?}"
        );
    }
}

/// `base-pv.xml` with `<Azimuth>180</Azimuth>` added to its roof: the
/// 180° array attaches and the roof's coverage equals its capacity's
/// collector area over the roof's area; the 90° array attaches to no
/// roof, and the run's warning log names it with an azimuth difference
/// of 90°.
#[test]
fn pv_on_an_oriented_roof_matches_by_azimuth() {
    let xml = base_pv_xml().replacen(
        "<Pitch>6.0</Pitch>",
        "<Pitch>6.0</Pitch><Azimuth>180</Azimuth>",
        1,
    );
    let mut dwelling = Dwelling::from_config(base_pv_config("oriented", &xml)).expect("builds");
    let roof = pitched_roof(&dwelling)
        .find(|s| !s.omni_directional)
        .cloned()
        .expect("the oriented roof is registered");
    let expected = 4000.0 * COLLECTOR_AREA_PER_W / roof.area_m2;
    assert_eq!(
        dwelling.environment.pv_roof_coverage(roof.surface_id),
        Some(expected),
        "only the 180° array covers the oriented roof"
    );
    let warnings = build_warnings(&mut dwelling);
    assert!(
        warnings
            .iter()
            .any(|m| m.contains("PV #2") && m.contains("90")),
        "the warning must name the unattached array and its 90° azimuth \
         difference, got: {warnings:?}"
    );
    assert!(
        !warnings
            .iter()
            .any(|m| m.contains("PV #1") && m.contains("attaches to no roof")),
        "the 180° array attaches: {warnings:?}"
    );
}

/// `base-pv.xml` with the first PV's `ArrayTilt` removed fails
/// construction naming the PV and the element.
#[test]
fn pv_without_orientation_is_an_assembly_error() {
    let xml = base_pv_xml().replacen("<ArrayTilt>20.0</ArrayTilt>", "", 1);
    let err = Dwelling::from_config(base_pv_config("no-tilt", &xml))
        .map(|_| ())
        .expect_err("a PV without ArrayTilt fails construction");
    let message = err.to_string();
    assert!(
        message.contains("ArrayTilt"),
        "the error must name the HPXML element, got: {message}"
    );
    assert!(
        message.contains("PVSystem1"),
        "the error must name the PV, got: {message}"
    );
}

/// `base-pv.xml` with the first PV's `MaxPowerOutput` raised until the two
/// arrays' summed collector area exceeds the roof's fails construction
/// naming the roof, its area and both PVs.
#[test]
fn roof_coverage_above_one_is_an_assembly_error() {
    let xml = base_pv_xml().replacen(
        "<MaxPowerOutput>4000.0</MaxPowerOutput>",
        "<MaxPowerOutput>30000.0</MaxPowerOutput>",
        1,
    );
    let err = Dwelling::from_config(base_pv_config("over", &xml))
        .map(|_| ())
        .expect_err("the over-subscribed roof fails construction");
    let message = err.to_string();
    assert!(
        message.contains("Roof1"),
        "the error must name the roof, got: {message}"
    );
    assert!(
        message.contains("PV #1") && message.contains("PV #2"),
        "the error must name both arrays on the roof, got: {message}"
    );
    assert!(
        message.contains("m²"),
        "the error must state the roof's area, got: {message}"
    );
}
