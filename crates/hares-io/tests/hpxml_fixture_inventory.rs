use std::fs;
use std::path::PathBuf;

use hares_io::hpxml::building::{parse_building, parse_xml_document};

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/hpxml/ochre_samples")
}

#[test]
fn curated_fixture_files_exist() {
    let root = fixture_root();
    let expected = [
        "base.xml",
        "base-enclosure-garage.xml",
        "base-enclosure-windows-physical-properties.xml",
        "base-foundation-basement-garage.xml",
        "base-pv.xml",
        "base-battery.xml",
        "base-pv-battery.xml",
        "base-lighting-mixed.xml",
        "base-appliances-dehumidifier.xml",
        "base-misc-loads-large-uncommon.xml",
        "base-hvac-multiple.xml",
        "base-dhw-multiple.xml",
        "base-bldgtype-multifamily.xml",
        "base-bldgtype-multifamily-shared-boiler-only-baseboard.xml",
    ];

    for file in expected {
        assert!(root.join(file).exists(), "missing fixture: {file}");
    }
}

#[test]
fn curated_fixtures_are_well_formed_hpxml_v4() {
    let root = fixture_root();
    let files = [
        "base.xml",
        "base-enclosure-garage.xml",
        "base-enclosure-windows-physical-properties.xml",
        "base-foundation-basement-garage.xml",
        "base-pv.xml",
        "base-battery.xml",
        "base-pv-battery.xml",
        "base-lighting-mixed.xml",
        "base-appliances-dehumidifier.xml",
        "base-misc-loads-large-uncommon.xml",
        "base-hvac-multiple.xml",
        "base-dhw-multiple.xml",
        "base-bldgtype-multifamily.xml",
        "base-bldgtype-multifamily-shared-boiler-only-baseboard.xml",
    ];

    for file in files {
        let xml = fs::read_to_string(root.join(file)).expect("fixture should be readable");
        let root_node = parse_xml_document(&xml).expect("fixture should be valid XML");
        assert_eq!(root_node.name, "HPXML", "unexpected root in {file}");
        assert_eq!(
            root_node.attrs.get("schemaVersion").map(String::as_str),
            Some("4.0"),
            "unexpected schema version in {file}"
        );
    }
}

#[test]
fn curated_set_contains_der_and_end_use_coverage() {
    let root = fixture_root();

    let pv = fs::read_to_string(root.join("base-pv.xml")).expect("pv fixture should load");
    assert!(pv.contains("<PVSystem>"), "PV fixture missing PVSystem tag");

    let battery =
        fs::read_to_string(root.join("base-battery.xml")).expect("battery fixture should load");
    assert!(
        battery.contains("<Battery>") || battery.contains("<Batteries>"),
        "Battery fixture missing battery tag"
    );

    let ev = fs::read_to_string(root.join("base-misc-loads-large-uncommon.xml"))
        .expect("misc fixture should load");
    assert!(
        ev.contains("electric vehicle charging"),
        "EV plug-load marker missing"
    );

    let loads = fs::read_to_string(root.join("base-appliances-dehumidifier.xml"))
        .expect("appliance fixture should load");
    for tag in [
        "<Appliances>",
        "<ClothesWasher>",
        "<ClothesDryer>",
        "<Dishwasher>",
        "<Lighting>",
    ] {
        assert!(loads.contains(tag), "missing expected load tag: {tag}");
    }

    let hvac = fs::read_to_string(root.join("base-hvac-multiple.xml"))
        .expect("hvac-multiple fixture should load");
    assert!(
        hvac.contains("<HeatingSystem>"),
        "HVAC-multiple fixture missing HeatingSystem tag"
    );
    assert!(
        hvac.contains("<CoolingSystem>"),
        "HVAC-multiple fixture missing CoolingSystem tag"
    );
    assert!(
        hvac.contains("<HeatPump>"),
        "HVAC-multiple fixture missing HeatPump tag"
    );
    assert!(
        hvac.contains("<PrimaryHeatingSystem"),
        "HVAC-multiple fixture missing PrimaryHeatingSystem"
    );
    assert!(
        hvac.contains("<PrimaryCoolingSystem"),
        "HVAC-multiple fixture missing PrimaryCoolingSystem"
    );

    let dhw = fs::read_to_string(root.join("base-dhw-multiple.xml"))
        .expect("dhw-multiple fixture should load");
    let wh_count = dhw.match_indices("<WaterHeatingSystem>").count();
    assert_eq!(
        wh_count, 6,
        "base-dhw-multiple.xml must contain exactly 6 WaterHeatingSystem entries, found {wh_count}"
    );
}

#[test]
fn multifamily_fixtures_identify_as_apartment_unit() {
    let root = fixture_root();

    let fixture = "base-bldgtype-multifamily.xml";
    let xml = fs::read_to_string(root.join(fixture)).expect("multifamily fixture should load");
    let building = parse_building(&xml).expect("multifamily fixture should parse");
    assert_eq!(
        building.residential_facility_type.as_deref(),
        Some("apartment unit"),
        "multifamily fixture must have residential_facility_type = 'apartment unit'"
    );
}

#[test]
fn multifamily_shared_boiler_fixture_parses_without_error() {
    let root = fixture_root();

    let fixture = "base-bldgtype-multifamily-shared-boiler-only-baseboard.xml";
    let xml = fs::read_to_string(root.join(fixture))
        .expect("multifamily shared boiler fixture should load");
    let building =
        parse_building(&xml).expect("multifamily shared boiler fixture should parse without error");

    assert_eq!(
        building.residential_facility_type.as_deref(),
        Some("apartment unit"),
        "multifamily shared boiler fixture must have residential_facility_type = 'apartment unit'"
    );

    let has_shared_boiler = xml.contains("<IsSharedSystem>true</IsSharedSystem>");
    assert!(
        has_shared_boiler,
        "shared boiler fixture must contain <IsSharedSystem>true</IsSharedSystem>"
    );
}
