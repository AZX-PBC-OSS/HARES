//! The resolved-parameter read-back's acceptance tests: the parameters the
//! physics actually uses, read back from the built dwelling, each with the
//! value the assembly resolved. The read-back is computed at assembly,
//! after defaults, autosizing and overrides, before warm-up, so the value
//! before the first step is the value after a full run.

use std::path::{Path, PathBuf};

use chrono::{Duration, TimeZone};
use hares_core::{Dwelling, DwellingConfig, DwellingSettings};
use hares_io::{
    DefaultsSource, InputSource, SetpointSource, SimulationConfig, SiteLocationOverride,
    WeatherInput,
};

#[path = "../../../tests/support/denver_offset.rs"]
mod denver_offset;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture(name: &str) -> PathBuf {
    project_root()
        .join("tests/fixtures/hpxml/ochre_samples")
        .join(name)
}

fn config(hpxml: &Path) -> DwellingConfig {
    let start_time = denver_offset::denver_offset()
        .with_ymd_and_hms(2023, 1, 15, 10, 0, 0)
        .single()
        .expect("valid start time");
    DwellingConfig {
        hpxml: InputSource::Path(hpxml.to_path_buf()),
        schedule: Some(InputSource::Path(
            project_root().join("data/examples/BEopt_example_schedule.csv"),
        )),
        weather: WeatherInput::Path(
            project_root().join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        ),
        defaults: DefaultsSource::directory(project_root().join("defaults")),
        settings: DwellingSettings::new(
            SimulationConfig {
                start_time,
                duration: Duration::hours(2),
                time_res: Duration::seconds(900),
                reports: Vec::new(),
                diagnostics: false,
                metrics: false,
                output_chunk_size: 1024,
                setpoint_deadband_c: None,
                master_seed: 0,
                civil_timezone: None,
                site_location: SiteLocationOverride::default(),
                max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
                health_policy: hares_io::HealthPolicy::default(),
            },
            None,
            1,
            None,
            None,
        ),
    }
}

/// The parameter named `name` on the named equipment, from the read-back.
fn parameter<'a>(
    model: &'a hares_core::ResolvedModel,
    equipment: &str,
    name: &str,
) -> &'a hares_types::ResolvedParameter {
    let eq = model
        .equipment
        .iter()
        .find(|eq| eq.name == equipment)
        .unwrap_or_else(|| panic!("the read-back carries '{equipment}'"));
    eq.parameters
        .iter()
        .find(|p| p.name == name)
        .unwrap_or_else(|| panic!("'{equipment}' reports no '{name}' parameter"))
}

/// The read-back is available from build on, before the first step, and
/// the value a full run leaves is the value the build computed: the
/// assembly-time read-back is not rebuilt or mutated by stepping.
#[test]
fn resolved_model_available_before_simulating() {
    let mut dwelling =
        Dwelling::from_config(config(&fixture("base.xml"))).expect("the base dwelling builds");
    let before = dwelling.resolved_model().clone();

    dwelling.simulate().expect("the run completes");

    assert_eq!(
        &before,
        dwelling.resolved_model(),
        "the resolved model read after a full run equals the build-time read-back"
    );
}

/// Changing the HPXML's furnace efficiency changes only that furnace's
/// AFUE in the read-back, to the edited value: every other reported
/// parameter, every boundary, window and the infiltration are the same
/// values the unedited input resolves.
#[test]
fn resolved_model_reflects_hpxml_efficiency_edit() {
    let baseline = std::fs::read_to_string(fixture("base.xml")).expect("base.xml reads");
    // The one AFUE Value element in the fixture (the emittance elements
    // carry bare numbers, not Value children).
    assert_eq!(
        baseline.matches("<Value>0.92</Value>").count(),
        1,
        "the edit anchor must match exactly the furnace's efficiency element"
    );
    let edited_xml = baseline.replacen("<Value>0.92</Value>", "<Value>0.80</Value>", 1);

    let dir = tempfile::tempdir().expect("temp dir");
    let edited_path = dir.path().join("edited.xml");
    std::fs::write(&edited_path, edited_xml).expect("the edited HPXML writes");

    let baseline_dwelling =
        Dwelling::from_config(config(&fixture("base.xml"))).expect("the baseline builds");
    let edited_dwelling =
        Dwelling::from_config(config(&edited_path)).expect("the edited dwelling builds");

    let before = baseline_dwelling.resolved_model().clone();
    let after = edited_dwelling.resolved_model().clone();

    assert_eq!(
        before.equipment.len(),
        after.equipment.len(),
        "the edit resolves the same equipment"
    );
    let mut changed: Vec<String> = Vec::new();
    for (b, a) in before.equipment.iter().zip(&after.equipment) {
        assert_eq!(b.name, a.name, "the same equipment order");
        assert_eq!(b.heating_setpoint, a.heating_setpoint);
        assert_eq!(b.cooling_setpoint, a.cooling_setpoint);
        for (pb, pa) in b.parameters.iter().zip(&a.parameters) {
            if pb.name == pa.name && pb.value == pa.value && pb.unit == pa.unit {
                continue;
            }
            let site = format!("{}.{}", b.name, pb.name);
            assert_eq!(
                pb.name, "afue",
                "only the furnace's AFUE may move; {site} moved too"
            );
            changed.push(site);
        }
    }
    assert_eq!(
        changed,
        vec!["Gas Furnace.afue".to_string()],
        "exactly the furnace's AFUE changes"
    );
    let afue = parameter(&after, "Gas Furnace", "afue");
    assert_eq!(afue.value, 0.80, "the AFUE reads back the edited value");
    assert_eq!(before.boundaries, after.boundaries, "boundaries unchanged");
    assert_eq!(before.windows, after.windows, "windows unchanged");
    assert_eq!(
        before.infiltration, after.infiltration,
        "infiltration unchanged"
    );
}

/// A fixture without HeatingCapacity reports the autosized capacity the
/// equipment uses: the read-back's value is the resolved one, finite and
/// positive, not a zero placeholder.
#[test]
fn resolved_model_reports_autosized_capacity() {
    let dir = tempfile::tempdir().expect("temp dir");
    let hpxml_path = dir.path().join("home.xml");
    std::fs::write(&hpxml_path, furnace_without_heating_capacity_hpxml())
        .expect("the fixture HPXML writes");

    let dwelling =
        Dwelling::from_config(config(&hpxml_path)).expect("the autosized dwelling builds");
    let capacity = parameter(dwelling.resolved_model(), "Gas Furnace", "rated_capacity_w");
    assert!(
        capacity.value.is_finite() && capacity.value > 0.0,
        "the autosized capacity the physics uses reads back finite and positive, got {}",
        capacity.value
    );
    assert_eq!(capacity.unit, hares_types::Unit::Watt);
}

/// Every non-slab opaque boundary's layer resistances plus the film
/// resistances equal the effective resistance to 1e-12 relative: the
/// read-back's layers are the layers the RC network builds, and the
/// effective R is films included. Slabs (the F-factor perimeter method)
/// report no layers. The same-zone surfaces the parser synthesizes (the
/// interior partition wall, the furniture mass) report the layers their
/// construction keeps, and their effective R is those layers' series plus
/// the one interior film, the resistance the physics conducts through.
#[test]
fn resolved_layers_sum_to_the_boundary_resistance() {
    let dwelling =
        Dwelling::from_config(config(&fixture("base.xml"))).expect("the base dwelling builds");
    let model = dwelling.resolved_model();
    assert!(
        !model.boundaries.is_empty(),
        "the read-back carries the assembled boundaries"
    );
    let mut checked = 0;
    for boundary in &model.boundaries {
        if boundary.kind == hares_io::hpxml::BoundaryType::Slab {
            assert!(
                boundary.layers.is_empty(),
                "the F-factor slab reports no layers (the plan's shape), got {}",
                boundary.layers.len()
            );
            continue;
        }
        if matches!(
            boundary.kind,
            hares_io::hpxml::BoundaryType::Window | hares_io::hpxml::BoundaryType::Skylight
        ) {
            continue;
        }
        let layers: f64 = boundary.layers.iter().map(|l| l.r_m2k_w).sum();
        let expected = layers + boundary.films_m2k_w;
        let relative = ((expected - boundary.r_effective_m2k_w) / boundary.r_effective_m2k_w).abs();
        assert!(
            relative <= 1e-12,
            "boundary '{}': layers ({layers}) plus films ({}) must equal the \
             effective resistance {} to 1e-12 relative, got {relative}",
            boundary.id,
            boundary.films_m2k_w,
            boundary.r_effective_m2k_w
        );
        checked += 1;
    }
    assert!(
        checked >= 4,
        "base.xml's outside-facing opaque boundaries must be checked, checked {checked}"
    );
}

/// A cited default's parameter reads back as the default with the
/// default's value; an input's as the input with the input's value:
/// base.xml's declared furnace efficiency and capacity read back at the
/// input's values with the HPXML setpoints the input declares, while a
/// furnace the input gives no setpoints for reads back the
/// manual-thermostat default profile the rule cites.
#[test]
fn resolved_parameters_read_back_the_input_value_and_the_cited_default() {
    let dwelling =
        Dwelling::from_config(config(&fixture("base.xml"))).expect("the base dwelling builds");
    let model = dwelling.resolved_model();

    // The input's values, read back as the input's: the AFUE the element
    // declares, and the HeatingCapacity converted from its Btu/h input
    // unit to the SI watts the physics uses (the same conversion the
    // resolver applies at the parse).
    assert_eq!(parameter(model, "Gas Furnace", "afue").value, 0.92);
    let capacity = parameter(model, "Gas Furnace", "rated_capacity_w");
    assert!(
        (capacity.value - hares_physics::units::power_btu_h_to_w(36_000.0)).abs()
            < 1e-9 * capacity.value,
        "the declared capacity reads back in SI watts, got {}",
        capacity.value
    );
    let furnace = model
        .equipment
        .iter()
        .find(|eq| eq.name == "Gas Furnace")
        .expect("the furnace is in the read-back");
    match &furnace.heating_setpoint {
        Some(SetpointSource::Hpxml {
            weekday_c,
            weekend_c,
        }) => {
            assert_eq!(weekday_c.len(), 24, "24 hourly weekday values");
            assert_eq!(weekend_c.len(), 24, "24 hourly weekend values");
            assert!(
                weekday_c.iter().all(|v| (*v - 20.0).abs() < 1e-9),
                "the HPXML's 68 F heating setpoint reads back in Celsius: {weekday_c:?}"
            );
        }
        other => panic!("the declared HPXML setpoints read back as input, got {other:?}"),
    }

    // The cited default, read back as the default: the minimal furnace
    // fixture declares no HVACControl and its schedule carries no setpoint
    // column, so the OS-HPXML manual-thermostat default is what applies.
    let dir = tempfile::tempdir().expect("temp dir");
    let hpxml_path = dir.path().join("home.xml");
    std::fs::write(&hpxml_path, furnace_without_heating_capacity_hpxml())
        .expect("the fixture HPXML writes");
    let dwelling = Dwelling::from_config(config(&hpxml_path)).expect("the minimal dwelling builds");
    let furnace = dwelling
        .resolved_model()
        .equipment
        .iter()
        .find(|eq| eq.name == "Gas Furnace")
        .expect("the minimal dwelling's furnace is in the read-back");
    assert_eq!(
        furnace.heating_setpoint,
        Some(SetpointSource::DefaultProfile {
            profile: SetpointSource::MANUAL_THERMOSTAT.to_string(),
        }),
        "the cited default reads back as the default profile"
    );
}

/// The minimal furnace fixture, verbatim from `autosize_from_config.rs`'s
/// builder so the read-back and the autosize row describe one input: no
/// HeatingCapacity, no HVACControl, one wall, one roof, one window.
fn furnace_without_heating_capacity_hpxml() -> &'static str {
    r#"<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site>
          <SiteType>suburban</SiteType>
          <Latitude>39.74</Latitude>
          <Longitude>-104.99</Longitude>
        </Site>
        <BuildingConstruction>
          <NumberofBedrooms>3</NumberofBedrooms>
          <ConditionedFloorArea units="ft2">2000</ConditionedFloorArea>
          <ConditionedBuildingVolume units="ft3">16000</ConditionedBuildingVolume>
          <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure>
        <AirInfiltrationMeasurement>
          <BuildingAirLeakage>
            <UnitofMeasure>ACH</UnitofMeasure>
            <AirLeakage>0</AirLeakage>
          </BuildingAirLeakage>
        </AirInfiltrationMeasurement>
        <Walls>
          <Wall>
            <SystemIdentifier id="Wall1"/>
            <InteriorAdjacentTo>living space</InteriorAdjacentTo>
            <ExteriorAdjacentTo>outside</ExteriorAdjacentTo>
            <Area units="ft2">1200.0</Area>
            <Azimuth>180</Azimuth>
            <WallType><WoodStud/></WallType>
            <Insulation>
              <AssemblyEffectiveRValue>13.0</AssemblyEffectiveRValue>
            </Insulation>
          </Wall>
        </Walls>
        <Roofs>
          <Roof>
            <SystemIdentifier id="Roof1"/>
            <InteriorAdjacentTo>living space</InteriorAdjacentTo>
            <ExteriorAdjacentTo>outside</ExteriorAdjacentTo>
            <Area units="ft2">2000.0</Area>
            <RoofType>asphalt or fiberglass shingles</RoofType>
            <Pitch>4</Pitch>
            <Insulation>
              <AssemblyEffectiveRValue>30.0</AssemblyEffectiveRValue>
            </Insulation>
          </Roof>
        </Roofs>
        <Windows>
          <Window>
            <SystemIdentifier id="Win1"/>
            <Area units="ft2">40.0</Area>
            <Azimuth>180</Azimuth>
            <UFactor units="Btu/(h-ft2-F)">0.30</UFactor>
            <SHGC>0.40</SHGC>
            <InteriorShading>
              <SummerShadingCoefficient>0.70</SummerShadingCoefficient>
              <WinterShadingCoefficient>0.85</WinterShadingCoefficient>
            </InteriorShading>
            <AttachedToWall idref="Wall1"/>
          </Window>
        </Windows>
      </Enclosure>
      <Systems>
        <HVAC>
          <HeatingSystem>
            <HeatingSystemType><Furnace/></HeatingSystemType>
            <HeatingSystemFuel>natural gas</HeatingSystemFuel>
            <AnnualHeatingEfficiency>
              <Units>AFUE</Units>
              <Value>0.80</Value>
            </AnnualHeatingEfficiency>
          </HeatingSystem>
        </HVAC>
      </Systems>
    </BuildingDetails>
  </Building>
</HPXML>"#
}
