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
                first_hour_interpolation: Default::default(),
                weather_year: hares_io::WeatherYear::default(),
                warmup: hares_io::Warmup::Disabled,
            },
            None,
            1,
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

/// Every non-slab opaque boundary's effective resistance equals the
/// network's discrete chain to 1e-12 relative, recomputed from the
/// read-back's own disclosure. An outside-facing boundary's chain is the
/// layers' full series plus the films. A same-zone fin's chain is the
/// kept layers' series minus the half-resistor the network drops: the
/// precomputed path (which every base.xml same-zone surface takes) pops
/// the innermost kept layer's inner half, and the one interior film rides
/// the zone-facing edge. The diagnostic's same-zone total (the full stack
/// halved plus the film) is a different record and equals neither.
/// Slabs (the F-factor perimeter method) report no layers.
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
        let expected = if boundary.same_zone {
            let dropped = boundary
                .layers
                .last()
                .map(|l| l.r_m2k_w / 2.0)
                .unwrap_or(0.0);
            layers + boundary.films_m2k_w - dropped
        } else {
            layers + boundary.films_m2k_w
        };
        let relative = ((expected - boundary.r_effective_m2k_w) / boundary.r_effective_m2k_w).abs();
        assert!(
            relative <= 1e-12,
            "boundary '{}': the network's chain (layers, minus the fin's dropped \
             half-resistor, plus films) must equal the effective resistance {} \
             to 1e-12 relative, got {relative}",
            boundary.id,
            boundary.r_effective_m2k_w
        );
        checked += 1;
    }
    assert!(
        checked >= 4,
        "base.xml's outside-facing opaque boundaries must be checked, checked {checked}"
    );
}

/// The same-zone fins' effective resistance pinned to the network's
/// actual chain sums, the values the assembled legs conduct through: the
/// furniture mass's one kept LUT layer contributes its inner half at the
/// zone-facing edge, the partition wall's two kept layers drop the
/// innermost layer's inner half. The read-back these pins replaced
/// reported the kept layers' full series plus the film (1.645 for the
/// furniture), two thirds above the network's chain.
#[test]
fn same_zone_fin_r_pins_the_network_chain() {
    let dwelling =
        Dwelling::from_config(config(&fixture("base.xml"))).expect("the base dwelling builds");
    let model = dwelling.resolved_model();
    let fin = |id: &str| {
        let boundary = model
            .boundaries
            .iter()
            .find(|b| b.id == id)
            .unwrap_or_else(|| panic!("'{id}' is in the read-back"));
        assert!(boundary.same_zone, "'{id}' is a same-zone fin");
        boundary
    };
    let furniture = fin("conditioned_furniture");
    assert!(
        (furniture.r_effective_m2k_w - 0.985_097_529_258_777_6).abs()
            < 1e-12 * furniture.r_effective_m2k_w,
        "the furniture mass's effective R is the network's chain, got {}",
        furniture.r_effective_m2k_w
    );
    let wall = fin("interior_wall");
    assert!(
        (wall.r_effective_m2k_w - 0.591_612_529_258_777_6).abs() < 1e-12 * wall.r_effective_m2k_w,
        "the interior partition wall's effective R is the network's chain, got {}",
        wall.r_effective_m2k_w
    );
}

/// The parity furnace fixture's dwelling, loaded the way the core output
/// regressions load their parity fixtures: the fixture's own sim config,
/// the reports dropped (these tests read in-memory state).
fn parity_furnace_dwelling() -> Dwelling {
    let root = project_root()
        .join("tests/fixtures/parity")
        .join("cz2a_gas_furnace_ac_res_wh");
    let config_value: toml::Value = toml::from_str(
        &std::fs::read_to_string(root.join("config.toml")).expect("the fixture config.toml reads"),
    )
    .expect("the fixture config.toml parses");
    let sim_table = config_value
        .get("simulation")
        .and_then(|value| value.as_table())
        .expect("the fixture config.toml has [simulation]");
    let mut sim_config =
        SimulationConfig::from_toml(&toml::to_string(sim_table).expect("the table serializes"))
            .expect("the fixture sim config parses");
    sim_config.reports = Vec::new();
    Dwelling::from_config(DwellingConfig {
        hpxml: InputSource::Path(root.join("building.xml")),
        schedule: Some(InputSource::Path(root.join("schedule.csv"))),
        weather: WeatherInput::Path(root.join("weather.epw")),
        defaults: DefaultsSource::directory(project_root().join("defaults")),
        settings: DwellingSettings::new(sim_config, None, 1, None),
    })
    .expect("the fixture dwelling builds")
}

/// The read-back's values are the ones the steps use. At the run's first
/// full-fire step (pinned at step 6: the furnace comes out of warm-up
/// partial, then meets a load above its capacity; the EnergyPlus weather
/// phase correction moved the arrival from 4 to 6) the furnace's step
/// telemetry fuel_input_w is the read-back's rated capacity over its
/// AFUE, bit for bit: the burner burns exactly the resolved values'
/// ratio.
#[test]
fn step_telemetry_uses_the_read_back_furnace_values() {
    use hares_types::telemetry_keys::{DUTY_CYCLE, FUEL_INPUT_W};

    let mut dwelling = parity_furnace_dwelling();
    let model = dwelling.resolved_model();
    let capacity = parameter(model, "Gas Furnace", "rated_capacity_w").value;
    let afue = parameter(model, "Gas Furnace", "afue").value;
    let full_fire_fuel_w = capacity / afue;

    let fuel_duty_at = |dwelling: &Dwelling| {
        let eq = dwelling
            .equipment()
            .iter()
            .find(|eq| eq.descriptor().name == "Gas Furnace")
            .expect("the fixture carries the gas furnace");
        let telemetry = eq.telemetry();
        (
            telemetry.get(FUEL_INPUT_W).unwrap_or_default(),
            telemetry.get(DUTY_CYCLE).unwrap_or_default(),
        )
    };
    let mut pinned_step = None;
    for step in 0..48 {
        dwelling.step().expect("the fixture steps");
        let (fuel, duty) = fuel_duty_at(&dwelling);
        if fuel > 0.0 && duty == 1.0 {
            pinned_step = Some((step, fuel));
            break;
        }
    }
    let (step, fuel) =
        pinned_step.expect("the furnace reaches full fire within the pinned horizon");
    assert_eq!(step, 6, "the first full-fire step is pinned");
    assert_eq!(
        fuel.to_bits(),
        full_fire_fuel_w.to_bits(),
        "the step's fuel input is the read-back's capacity over AFUE, bit for bit"
    );
}

/// The network's conductance is the reported R's reciprocal for one
/// envelope case: the diagnostic's per-boundary UA (area over the total
/// the network was built from) equals area over the read-back's
/// effective R, bit for bit, for the fixture's first outside-facing
/// precomputed boundary.
#[test]
#[cfg(debug_assertions)]
fn network_conductance_matches_the_reported_r() {
    use hares_envelope::boundary_rc::RCPath;

    let dwelling = parity_furnace_dwelling();
    let model = dwelling.resolved_model();
    let diagnostics = dwelling.envelope_diagnostics();
    assert_eq!(
        model.boundaries.len(),
        diagnostics.boundaries.len(),
        "the read-back and the diagnostics list the same boundaries in the same order"
    );
    let case = model
        .boundaries
        .iter()
        .zip(&diagnostics.boundaries)
        .find(|(b, d)| d.path == RCPath::Precomputed && !b.same_zone)
        .expect("the fixture carries an outside-facing precomputed boundary");
    let (boundary, diagnostic) = case;
    let from_read_back = boundary.area_m2 / boundary.r_effective_m2k_w;
    assert_eq!(
        diagnostic.ua_w_per_k.to_bits(),
        from_read_back.to_bits(),
        "boundary '{}': the network's conductance is the reported R's reciprocal",
        boundary.id
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
