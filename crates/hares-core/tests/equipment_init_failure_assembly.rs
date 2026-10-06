//! An equipment whose initialisation fails stops construction.
//!
//! Every resolved equipment must join the dwelling or the build fails: a
//! lighting, appliance, plug, or ventilation load the input declares must
//! never silently vanish from the energy totals and the zone gains because
//! its `init` rejected its config. The run errors naming the equipment and
//! the cause. The census pins that every checked-in HPXML fixture builds
//! holding one equipment per resolved spec.

use std::path::{Path, PathBuf};

use chrono::{Duration, FixedOffset, TimeZone};
use hares_core::dwelling::DwellingBlueprint;
use hares_core::{Dwelling, DwellingConfig};
use hares_io::OutputFormat;
use serde_json::{Map, Value, json};

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture(name: &str) -> PathBuf {
    project_root()
        .join("tests/fixtures/hpxml/ochre_samples")
        .join(name)
}

fn config(hpxml: &Path) -> DwellingConfig {
    let tz_offset = FixedOffset::west_opt(7 * 3600).expect("UTC-7 offset is valid");
    let start_time = tz_offset
        .with_ymd_and_hms(2023, 1, 15, 10, 0, 0)
        .single()
        .expect("valid start time");
    DwellingConfig {
        hpxml_path: hpxml.to_path_buf(),
        // No schedule file: the generated schedule is the test's condition.
        schedule_path: None,
        weather_path: project_root().join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        defaults_path: Some(project_root().join("defaults")),
        sim_config: hares_io::SimulationConfig {
            start_time,
            duration: Duration::hours(24),
            time_res: Duration::seconds(3600),
            output_verbosity: 0,
            write_output: false,
            output_path: None,
            output_format: OutputFormat::Csv,
            output_chunk_size: 1024,
            setpoint_deadband_c: None,
            master_seed: 0,
            civil_timezone: None,
            site_location: hares_io::SiteLocationOverride::default(),
            retain_batches: false,
            rotation: hares_io::RotationPolicy::None,
        },
        overrides: None,
        bldg_id: 1,
        initialization_duration: None,
        resample_overrides: Some(hares_io::ResampleOverrides::ochre_compat()),
        patches: None,
    }
}

/// An override payload setting one equipment's `sensible_gain_fraction`.
fn gain_fraction_override(equipment_name: &str, sensible_gain_fraction: f64) -> Value {
    let mut fields = Map::new();
    fields.insert(
        "sensible_gain_fraction".to_string(),
        json!(sensible_gain_fraction),
    );
    let mut root = Map::new();
    root.insert(equipment_name.to_string(), Value::Object(fields));
    Value::Object(root)
}

/// The first scheduled load the input resolves, named from the resolved
/// specs so the override lands on equipment the input declares, not a
/// hardcoded name. Scheduled-load specs carry the gain-fraction
/// parameters the HVAC, water-heating, EV, battery, and PV resolvers do
/// not inject.
fn first_scheduled_load_name(hpxml: &Path) -> String {
    let blueprint =
        DwellingBlueprint::from_config(config(hpxml)).expect("the fixture's blueprint must build");
    blueprint
        .equipment_specs
        .iter()
        .find(|spec| {
            spec.name != "Occupancy" && spec.parameters.contains_key("sensible_gain_fraction")
        })
        .unwrap_or_else(|| panic!("{} must resolve a scheduled load", hpxml.display()))
        .name
        .clone()
}

/// A scheduled load whose `init` fails must fail the BUILD, not skip the
/// equipment and run the dwelling short a load the input declares.
#[test]
fn equipment_init_failure_fails_assembly() {
    let load_name = first_scheduled_load_name(&fixture("base.xml"));

    let mut cfg = config(&fixture("base.xml"));
    // sensible 1.5 plus the declared latent fraction exceeds 1.0, which
    // the scheduled load's init rejects.
    cfg.overrides = Some(gain_fraction_override(&load_name, 1.5));

    let err = Dwelling::from_config(cfg).err().unwrap_or_else(|| {
        panic!(
            "a scheduled load whose init failed ('{load_name}') must fail \
             construction, not skip the equipment and run the dwelling \
             short a declared load"
        )
    });

    let msg = err.to_string();
    assert!(
        msg.contains(load_name.as_str()),
        "the error must name the equipment whose init failed ('{load_name}'), got: {msg}"
    );
    assert!(
        msg.contains("sensible_gain_fraction"),
        "the error must name the init failure's cause, got: {msg}"
    );
}

/// Fixtures the strict HPXML parse rejects before any equipment resolves.
/// Their exclusion is explicit so a future non-building fixture fails
/// here by name instead of silently leaving the corpus; a fixture listed
/// here does not build at all, which no equipment init failure can hide
/// behind.
const NOT_BUILDING_FIXTURES: &[&str] = &["base-enclosure-windows-physical-properties.xml"];

/// Every checked-in HPXML fixture path: the curated `hpxml/` tree (the
/// ochre samples plus the parser-dev fixture), every resstock `home.xml`,
/// and every parity case's `building.xml`.
fn checked_in_hpxml_fixtures() -> Vec<PathBuf> {
    fn walk_xml(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .collect();
        entries.sort();
        for entry in entries {
            if entry.is_dir() {
                walk_xml(&entry, out);
            } else if entry.extension().is_some_and(|ext| ext == "xml") {
                out.push(entry);
            }
        }
    }

    let mut fixtures: Vec<PathBuf> = Vec::new();
    // The curated `hpxml/` tree: every xml is an HPXML fixture.
    walk_xml(&project_root().join("tests/fixtures/hpxml"), &mut fixtures);
    // Resstock: only the per-building `home.xml` files (weather dirs hold
    // no xml, but filter anyway so a stray file cannot enter the corpus).
    let mut resstock: Vec<PathBuf> = Vec::new();
    walk_xml(
        &project_root().join("tests/fixtures/resstock"),
        &mut resstock,
    );
    fixtures.extend(
        resstock
            .into_iter()
            .filter(|path| path.file_name().is_some_and(|name| name == "home.xml")),
    );
    // Parity: each case directory's `building.xml`.
    let mut parity: Vec<PathBuf> = Vec::new();
    walk_xml(&project_root().join("tests/fixtures/parity"), &mut parity);
    fixtures.extend(
        parity
            .into_iter()
            .filter(|path| path.file_name().is_some_and(|name| name == "building.xml")),
    );
    fixtures
}

/// Every checked-in HPXML fixture that builds holds one equipment per
/// resolved spec: an init failure anywhere in a fixture's population is a
/// build failure, never a silent drop, so the dwelling's equipment list
/// must contain every spec the blueprint resolved for the whole fixture
/// corpus. The build may add zero-power equipment beyond the blueprint's
/// spec list (a Microwave column with no HPXML declaration injects a spec at
/// build time, the only auto-created case), so the pinned contract is
/// containment plus build success, not exact equality.
#[test]
fn every_fixture_builds_every_equipment() {
    let mut problems: Vec<String> = Vec::new();

    for fixture_path in checked_in_hpxml_fixtures() {
        let fixture_name = fixture_path
            .strip_prefix(project_root())
            .unwrap_or(&fixture_path)
            .display()
            .to_string();

        let blueprint = match DwellingBlueprint::from_config(config(&fixture_path)) {
            Ok(blueprint) => blueprint,
            Err(err) => {
                let not_building = fixture_path.file_name().is_some_and(|name| {
                    NOT_BUILDING_FIXTURES.contains(&name.to_string_lossy().as_ref())
                });
                if not_building {
                    continue;
                }
                problems.push(format!("{fixture_name}: blueprint failed to build: {err}"));
                continue;
            }
        };
        // equipment_names() already excludes the occupancy handled outside
        // the registry; every name it resolves must join the dwelling.
        let mut expected: Vec<String> = blueprint
            .equipment_names()
            .into_iter()
            .map(str::to_string)
            .collect();
        expected.sort();

        match blueprint.build() {
            Ok(dwelling) => {
                let mut actual: Vec<String> = dwelling
                    .equipment()
                    .iter()
                    .map(|eq| eq.descriptor().name.clone())
                    .collect();
                actual.sort();
                let missing: Vec<&String> = expected
                    .iter()
                    .filter(|name| !actual.contains(name))
                    .collect();
                if !missing.is_empty() {
                    problems.push(format!(
                        "{fixture_name}: the dwelling is missing resolved \
                         specs {missing:?} (it holds {actual:?})"
                    ));
                }
            }
            Err(err) => {
                problems.push(format!(
                    "{fixture_name}: the dwelling failed to build: {err}"
                ));
            }
        }
    }

    assert!(
        problems.is_empty(),
        "the census found fixtures whose dwelling does not hold one \
         equipment per resolved spec:\n{}",
        problems.join("\n")
    );
}
