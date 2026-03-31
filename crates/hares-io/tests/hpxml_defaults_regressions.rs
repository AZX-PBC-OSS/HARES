use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;

use hares_io::defaults::DefaultsStore;
use hares_io::hpxml::building::parse_building;
use hares_io::hpxml::equipment::resolve_equipment;
use hares_io::hpxml::validation::validate_hpxml_schema;
use hares_io::hpxml::{BoundaryType, HpxmlError, ZoneType, parse_hpxml_str};

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/hpxml/ochre_samples")
}

fn read_fixture(name: &str) -> String {
    fs::read_to_string(fixture_root().join(name)).expect("fixture should be readable")
}

fn repo_defaults() -> DefaultsStore {
    let defaults_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates dir")
        .parent()
        .expect("repo root")
        .join("defaults");
    DefaultsStore::load(&defaults_dir).expect("load defaults")
}

fn minimal_hpxml_with_systems(systems_xml: &str) -> String {
    format!(
        r#"<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site><SiteType>suburban</SiteType></Site>
        <BuildingConstruction>
          <ConditionedFloorArea units="m2">200</ConditionedFloorArea>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure><Walls /></Enclosure>
      {systems_xml}
    </BuildingDetails>
  </Building>
</HPXML>"#
    )
}

#[test]
fn base_fixture_preserves_summary_fields_and_imperial_unit_defaults() {
    let building = parse_hpxml_str(&read_fixture("base.xml")).expect("base fixture should parse");

    assert_eq!(
        building.residential_facility_type.as_deref(),
        Some("single-family detached"),
        "fixture residential facility type should not be silently dropped"
    );
    assert_eq!(building.floors_above_grade, Some(1.0));

    let conditioned = building
        .zones
        .iter()
        .find(|zone| matches!(zone.zone_type, ZoneType::Conditioned))
        .expect("conditioned zone expected");
    let floor_area_m2 = conditioned.floor_area_m2.expect("conditioned area expected");
    let volume_m3 = building
        .conditioned_volume_m3
        .expect("conditioned volume expected");
    let ceiling_height_m = building.ceiling_height_m.expect("ceiling height expected");

    let expected_floor_area_m2 = (2700.0 * 0.092_903_04) * 0.5;
    let expected_volume_m3 = 21600.0 * 0.028_316_846_592;
    let expected_ceiling_height_m = 8.0 * 0.3048;

    assert!(
        (floor_area_m2 - expected_floor_area_m2).abs() < 0.01,
        "Conditioned zone area should use HPXML/OCHRE imperial defaults and basement split"
    );
    assert!(
        (volume_m3 - expected_volume_m3).abs() < 0.01,
        "ConditionedBuildingVolume without explicit units must use HPXML/OCHRE imperial default"
    );
    assert!(
        (ceiling_height_m - expected_ceiling_height_m).abs() < 0.01,
        "ceiling height derived from fixture should remain aligned with the 8 ft OCHRE sample"
    );
}

#[test]
fn windows_physical_properties_fixture_preserves_explicit_fields_without_synthetic_defaults() {
    let building = parse_hpxml_str(&read_fixture("base-enclosure-windows-physical-properties.xml"))
        .expect("window fixture should parse");

    assert_eq!(building.windows.len(), 4, "fixture coverage changed unexpectedly");

    for window in &building.windows {
        assert!(
            window.u_factor_w_m2_k.is_none(),
            "window {} should not silently invent a U-factor when HPXML omitted it",
            window.id
        );
        assert!(
            window.shgc.is_none(),
            "window {} should not silently invent an SHGC when HPXML omitted it",
            window.id
        );
        assert!(
            (window.interior_shading_fraction - 0.70).abs() < f64::EPSILON,
            "window {} should preserve explicit summer shading coefficient",
            window.id
        );
        assert!(
            (window.winter_shading_fraction - 0.85).abs() < f64::EPSILON,
            "window {} should preserve explicit winter shading coefficient",
            window.id
        );
        assert!(
            (window.fraction_operable - 0.67).abs() < f64::EPSILON,
            "window {} should preserve explicit FractionOperable",
            window.id
        );
        assert_eq!(window.attached_to_wall_id.as_deref(), Some("Wall1"));
    }
}

#[test]
fn garage_basement_fixture_preserves_explicit_zone_adjacency() {
    let building =
        parse_hpxml_str(&read_fixture("base-foundation-basement-garage.xml"))
            .expect("garage fixture should parse");

    assert!(
        building.zones.iter().any(|zone| matches!(zone.zone_type, ZoneType::Garage)),
        "garage zone should be created from explicit garage adjacencies"
    );
    assert!(
        building.zones.iter().any(|zone| matches!(zone.zone_type, ZoneType::Foundation)),
        "foundation zone should be created from explicit basement foundation data"
    );

    let wall_to_garage = building
        .boundaries
        .iter()
        .find(|boundary| boundary.id == "Wall3" && matches!(boundary.boundary_type, BoundaryType::Wall))
        .expect("Wall3 should exist in garage fixture");
    assert!(
        matches!(wall_to_garage.interior_zone, Some(ZoneType::Foundation)),
        "Wall3 interior adjacency should stay mapped to the conditioned basement/foundation zone"
    );
    assert!(
        matches!(wall_to_garage.exterior_zone, Some(ZoneType::Garage)),
        "Wall3 exterior adjacency should stay mapped to garage rather than defaulting to outdoor"
    );
}

#[test]
fn missing_conditioned_floor_area_fails_schema_validation_instead_of_defaulting() {
    let xml = r#"
<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site><SiteType>suburban</SiteType></Site>
        <BuildingConstruction>
          <NumberofConditionedFloors>2</NumberofConditionedFloors>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure><Walls /></Enclosure>
    </BuildingDetails>
  </Building>
</HPXML>"#;

    let err = validate_hpxml_schema(xml).expect_err("schema validation must fail");
    assert!(
        err.message.contains("ConditionedFloorArea"),
        "missing ConditionedFloorArea must fail loudly, got: {}",
        err.message
    );
}

#[test]
fn missing_hvac_type_tags_fail_resolution_instead_of_defaulting() {
    let cases = [
        (
            "heating",
            r#"<Systems><HVAC><HeatingSystem><HeatingSystemFuel>natural gas</HeatingSystemFuel></HeatingSystem></HVAC></Systems>"#,
            "HeatingSystemType",
        ),
        (
            "cooling",
            r#"<Systems><HVAC><CoolingSystem><CoolingSystemFuel>electricity</CoolingSystemFuel></CoolingSystem></HVAC></Systems>"#,
            "CoolingSystemType",
        ),
        (
            "heat pump",
            r#"<Systems><HVAC><HeatPump><HeatingCapacity>24000</HeatingCapacity></HeatPump></HVAC></Systems>"#,
            "HeatPumpType",
        ),
    ];

    for (label, systems_xml, missing_field) in cases {
        let xml = minimal_hpxml_with_systems(systems_xml);
        let building = parse_building(&xml).expect("minimal fixture should parse");
        let err = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}))
            .expect_err("resolve_equipment must fail when required HVAC type tags are absent");
        match err {
            HpxmlError::Parse(message) => assert!(
                message.contains(missing_field),
                "{label} case should mention missing {missing_field}, got: {message}"
            ),
            other => panic!("{label} case should return HpxmlError::Parse, got {other:?}"),
        }
    }
}

#[test]
fn base_fixture_resolves_expected_typed_specs_with_repo_defaults() {
    let building = parse_hpxml_str(&read_fixture("base.xml")).expect("base fixture should parse");
    let specs = resolve_equipment(&building, &repo_defaults(), &json!({}))
        .expect("base fixture equipment should resolve with repo defaults");

    let furnace = specs
        .iter()
        .find(|spec| spec.name == "Gas Furnace")
        .expect("gas furnace spec should exist");
    assert!(
        furnace.typed_config.is_some(),
        "Gas Furnace should resolve to a typed config rather than silently degrading to raw params"
    );

    let air_conditioner = specs
        .iter()
        .find(|spec| spec.name == "Air Conditioner")
        .expect("air conditioner spec should exist");
    assert!(
        air_conditioner.typed_config.is_some(),
        "Air Conditioner should resolve to a typed config rather than silently degrading to raw params"
    );

    let water_heater = specs
        .iter()
        .find(|spec| spec.name == "Electric Resistance Water Heater")
        .expect("water heater spec should exist");
    assert!(
        water_heater.typed_config.is_some(),
        "water heater should resolve to a typed config with defaults applied"
    );
}
