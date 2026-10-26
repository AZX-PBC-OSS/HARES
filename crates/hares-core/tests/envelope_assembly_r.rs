//! The opaque-R precedence's acceptance (the numeric-fixes ledger's
//! opaque-R precedence entry): a declared `AssemblyEffectiveRValue` sets the resistance of the
//! LUT layers it selects. Every layer resistance of the matched row is
//! scaled by `k = (declared R - convention films) / sum(row layer
//! resistances)` so the layers plus the convention films equal the
//! declared R in the convention the declaration and the row selection
//! speak, the row's capacitances (its mass) are kept on bits, and the row
//! selection is unchanged, so a declared R between rows moves the
//! resolved resistance instead of stepping between rows. A declared R at
//! or below the convention film sum leaves no material resistance and is
//! a construction error. Slabs (F-factor), explicit material layers and
//! boundaries without a declared R are unchanged. The read-back names the
//! winning source.

use std::path::PathBuf;

use chrono::{Duration, TimeZone};
use hares_core::{
    DwellingBlueprint, DwellingConfig, DwellingSettings, RValueSource, ResolvedBoundary,
    ResolvedModel,
};
use hares_io::{
    DefaultsSource, Destination, Frequency, InputSource, ReportSpec, Selection, SimulationConfig,
    SiteLocationOverride, WeatherInput,
};
use hares_physics::units::r_value_ip_to_si;

#[path = "../../../tests/support/denver_offset.rs"]
mod denver_offset;

#[path = "../../../tests/support/temp_file.rs"]
mod temp_file;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn sim_config(duration_s: i64, step_s: i64) -> SimulationConfig {
    SimulationConfig {
        scenario_seed: None,
        start_time: denver_offset::denver_offset()
            .with_ymd_and_hms(2023, 1, 1, 0, 0, 0)
            .single()
            .expect("start"),
        duration: Duration::seconds(duration_s),
        time_res: Duration::seconds(step_s),
        reports: vec![ReportSpec {
            name: "frame".to_string(),
            selection: Selection::Verbosity(2),
            frequency: Frequency::default(),
            destination: Destination::Memory,
        }],
        diagnostics: false,
        metrics: true,
        output_chunk_size: 10_000,
        setpoint_deadband_c: None,
        master_seed: 0,
        civil_timezone: None,
        site_location: SiteLocationOverride::default(),
        max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
        health_policy: hares_io::HealthPolicy::default(),
        first_hour_interpolation: Default::default(),
        weather_year: hares_io::WeatherYear::default(),
        warmup: hares_io::Warmup::Disabled,
    }
}

/// The ochre-samples home: `tests/fixtures/hpxml/ochre_samples/base.xml`,
/// the Denver weather, the BEopt schedule, the repo's defaults tree, with
/// an optional `envelope` override map.
fn base_xml_config(duration_s: i64, scales: Option<(f64, f64)>) -> DwellingConfig {
    let overrides = scales.map(|(opaque_r_scale, thermal_mass_scale)| {
        serde_json::json!({"envelope": {"opaque_r_scale": opaque_r_scale, "thermal_mass_scale": thermal_mass_scale}})
    });
    DwellingConfig {
        hpxml: InputSource::Path(
            project_root().join("tests/fixtures/hpxml/ochre_samples/base.xml"),
        ),
        schedule: Some(InputSource::Path(
            project_root().join("data/examples/BEopt_example_schedule.csv"),
        )),
        weather: WeatherInput::Path(
            project_root().join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        ),
        defaults: DefaultsSource::directory(project_root().join("python/hares/defaults")),
        settings: DwellingSettings {
            sim_config: sim_config(duration_s, 900),
            overrides,
            bldg_id: 1,
            patches: None,
        },
    }
}

fn resolved_for(config: DwellingConfig) -> ResolvedModel {
    DwellingBlueprint::from_config(config)
        .expect("the blueprint builds")
        .validate()
        .expect("the blueprint validates")
}

fn boundary<'a>(model: &'a ResolvedModel, id: &str) -> &'a ResolvedBoundary {
    model
        .boundaries
        .iter()
        .find(|b| b.id == id)
        .unwrap_or_else(|| panic!("the read-back carries '{id}'"))
}

/// Wall1's declaration: `AssemblyEffectiveRValue` 23.0 (IP), a WoodStud
/// wall with a wood siding finish and no insulation description, matched
/// to an envelope LUT row.
const WALL1_DECLARED_R_IP: f64 = 23.0;
const WALL1_CONSTRUCTION: &str = "WoodStud";
const WALL1_FINISH: &str = "wood siding";

/// The matched LUT row for Wall1's filters at a declared R: the row's
/// boundary type string, its raw layers (resistance, capacitance) as the
/// materials CSV carries them, and the row-selection convention's film
/// sum the declared R is measured against.
struct Wall1Row {
    matched_type: String,
    films_m2k_w: f64,
    layers: Vec<(f64, f64)>,
}

fn wall1_row(r_ip: f64) -> Wall1Row {
    let store = hares_io::DefaultsStore::load(&project_root().join("python/hares/defaults"))
        .expect("the defaults tree loads");
    let lut = store
        .envelope_lut()
        .expect("the defaults tree carries the envelope LUT");
    use hares_io::hpxml::{BoundaryType, ZoneType};
    let name = lut
        .resolve_name(
            &BoundaryType::Wall,
            Some(&ZoneType::Conditioned),
            Some(&ZoneType::Outdoor),
        )
        .expect("the wall's LUT name resolves");
    let films = lut.convention_films(name);
    let result = lut
        .lookup(
            name,
            Some(WALL1_CONSTRUCTION),
            Some(WALL1_FINISH),
            None,
            Some(r_value_ip_to_si(r_ip)),
        )
        .expect("Wall1's filters match a LUT row");
    Wall1Row {
        matched_type: result.matched_boundary_type.clone(),
        films_m2k_w: films,
        layers: result
            .layers
            .iter()
            .map(|l| (l.resistance_m2_k_w, l.capacitance_kj_m2_k))
            .collect(),
    }
}

/// `base.xml` with Wall1's declared R replaced by the IP value, written to
/// the test's own temporary directory; the other declarations (the roof,
/// the rim joist, the attic gable wall, the ceiling) are untouched.
fn config_with_wall1_r(r_ip: f64, duration_s: i64) -> (tempfile::TempDir, DwellingConfig) {
    let original =
        std::fs::read_to_string(project_root().join("tests/fixtures/hpxml/ochre_samples/base.xml"))
            .expect("base.xml reads");
    let anchor = "id='Wall1Insulation'/>";
    // The fixture's own text pins Wall1 at 23.0 IP (the shortest
    // round-trip spelling of {WALL1_DECLARED_R_IP} would drop the ".0").
    let needle =
        format!("{anchor}\n              <AssemblyEffectiveRValue>23.0</AssemblyEffectiveRValue>");
    let replacement = format!(
        "{anchor}\n              <AssemblyEffectiveRValue>{r_ip}</AssemblyEffectiveRValue>"
    );
    assert_eq!(
        original.matches(&needle).count(),
        1,
        "the fixture pins Wall1's declared R at {WALL1_DECLARED_R_IP} IP"
    );
    let edited = original.replace(&needle, &replacement);
    let (dir, path) = temp_file::temp_file("base-wall1-r.xml");
    std::fs::write(&path, edited).expect("the edited fixture writes");
    let mut config = base_xml_config(duration_s, None);
    config.hpxml = InputSource::Path(path);
    (dir, config)
}

/// Every resolved layer resistance of Wall1 is the matched row's times
/// `k = (declared R - convention films) / sum(row layer resistances)`, the
/// layers plus the convention films equal the declared R in SI, and every
/// capacitance equals the row's on bits. The read-back names the declared
/// assembly R the winning source.
#[test]
fn assembly_r_distributes_across_lut_layers_uniformly() {
    let model = resolved_for(base_xml_config(24 * 3600, None));
    let wall1 = boundary(&model, "Wall1");
    let row = wall1_row(WALL1_DECLARED_R_IP);
    assert_eq!(
        wall1.layers.len(),
        row.layers.len(),
        "row {}: every layer of the row resolves",
        row.matched_type
    );
    let row_sum: f64 = row.layers.iter().map(|(r, _)| r).sum();
    let declared = r_value_ip_to_si(WALL1_DECLARED_R_IP);
    let k = (declared - row.films_m2k_w) / row_sum;
    for (index, ((row_r, row_c), layer)) in row.layers.iter().zip(&wall1.layers).enumerate() {
        assert_eq!(
            layer.r_m2k_w.to_bits(),
            (row_r * k).to_bits(),
            "layer {index}: the resistance is the row's times k"
        );
        assert_eq!(
            layer.c_j_m2k.to_bits(),
            (row_c * 1000.0).to_bits(),
            "layer {index}: the capacitance is the row's, on bits"
        );
    }
    let layer_sum: f64 = wall1.layers.iter().map(|l| l.r_m2k_w).sum();
    let total = layer_sum + row.films_m2k_w;
    let relative = (total - declared).abs() / declared;
    assert!(
        relative < 1e-12,
        "the layers plus the convention films equal the declared R: {total} vs {declared} (relative {relative:.3e})"
    );
    assert_eq!(
        wall1.r_source,
        RValueSource::DeclaredAssemblyR,
        "the read-back names the declared assembly R the winning source"
    );
}

/// Two declared R values that select the same row as 23.0 (22.5 and 23.5)
/// give different resolved resistances and equal capacitances: the
/// declared R is continuous between rows, not a staircase.
#[test]
fn declared_r_between_rows_moves_the_resolved_resistance() {
    let reference_row = wall1_row(WALL1_DECLARED_R_IP);
    for r_ip in [22.5, 23.5] {
        let row = wall1_row(r_ip);
        assert_eq!(
            row.matched_type, reference_row.matched_type,
            "{r_ip} IP selects the same row"
        );
    }
    let reference = resolved_for(base_xml_config(24 * 3600, None));
    let reference_wall1 = boundary(&reference, "Wall1");
    for r_ip in [22.5, 23.5] {
        let (dir, config) = config_with_wall1_r(r_ip, 24 * 3600);
        let model = resolved_for(config);
        drop(dir);
        let wall1 = boundary(&model, "Wall1");
        assert_eq!(
            wall1.layers.len(),
            reference_wall1.layers.len(),
            "{r_ip} IP: the row's layer count is unchanged"
        );
        for (index, (reference_layer, layer)) in
            reference_wall1.layers.iter().zip(&wall1.layers).enumerate()
        {
            assert_eq!(
                layer.c_j_m2k.to_bits(),
                reference_layer.c_j_m2k.to_bits(),
                "{r_ip} IP: layer {index}'s capacitance is the same row's, on bits"
            );
            assert_ne!(
                layer.r_m2k_w.to_bits(),
                reference_layer.r_m2k_w.to_bits(),
                "{r_ip} IP: layer {index}'s resistance moves off the 23.0 IP one"
            );
        }
    }
}

/// A declared R at or below the convention film sum leaves no material
/// resistance: construction fails naming the boundary, the declared R and
/// the film sum, rather than clamping and scaling the LUT layers to
/// almost no resistance without a word.
#[test]
fn declared_r_at_or_below_the_films_is_an_error() {
    let row = wall1_row(WALL1_DECLARED_R_IP);
    let films = row.films_m2k_w;
    let film_ip = films / r_value_ip_to_si(1.0);
    let ip = (film_ip * 100.0).floor() / 100.0;
    let si = r_value_ip_to_si(ip);
    assert!(
        si <= films,
        "the test's rounded-down IP value converts at or below the film sum: {si} vs {films}"
    );
    let (dir, config) = config_with_wall1_r(ip, 24 * 3600);
    let err = DwellingBlueprint::from_config(config)
        .expect("the blueprint builds")
        .validate()
        .expect_err("the declared R leaves no material resistance");
    drop(dir);
    let message = err.to_string();
    assert!(
        message.contains("Wall1"),
        "the error names the boundary: {message}"
    );
    assert!(
        message.contains(&format!("{si}")),
        "the error names the declared R: {message}"
    );
    assert!(
        message.contains(&format!("{films}")),
        "the error names the film sum: {message}"
    );
}

/// The declared R and the calibration scales compose in one order: the
/// row's layers take k first, then the scales, so every layer resistance
/// is the row's times k times 1.25 and every capacitance the row's.
#[test]
fn assembly_r_and_opaque_r_scale_compose() {
    let model = resolved_for(base_xml_config(24 * 3600, Some((1.25, 1.0))));
    let wall1 = boundary(&model, "Wall1");
    let row = wall1_row(WALL1_DECLARED_R_IP);
    let row_sum: f64 = row.layers.iter().map(|(r, _)| r).sum();
    let declared = r_value_ip_to_si(WALL1_DECLARED_R_IP);
    let k = (declared - row.films_m2k_w) / row_sum;
    assert_eq!(
        wall1.layers.len(),
        row.layers.len(),
        "the scaled run resolves the same row's layers"
    );
    for (index, ((row_r, row_c), layer)) in row.layers.iter().zip(&wall1.layers).enumerate() {
        assert_eq!(
            layer.r_m2k_w.to_bits(),
            (row_r * k * 1.25).to_bits(),
            "layer {index}: the resistance is the row's times k times the scale"
        );
        assert_eq!(
            layer.c_j_m2k.to_bits(),
            (row_c * 1000.0).to_bits(),
            "layer {index}: the capacitance is the row's, on bits"
        );
    }
    assert_eq!(
        wall1.r_source,
        RValueSource::DeclaredAssemblyR,
        "the composed run's winning source is still the declared assembly R"
    );
}

/// The paths the opaque-R precedence does not touch: base.xml's slab resolves bitwise as at
/// the parent commit (the F-factor perimeter method, the F-factor slab
/// named the winning source), and a boundary with explicit material
/// layers resolves bitwise as at the parent too, declared R or not.
#[test]
fn slab_and_material_layer_boundaries_are_unchanged() {
    let model = resolved_for(base_xml_config(24 * 3600, None));
    let slab = boundary(&model, "Slab1");
    assert!(
        slab.layers.is_empty(),
        "the F-factor slab reports no layers"
    );
    assert_eq!(
        slab.r_effective_m2k_w.to_bits(),
        SLAB_PARENT_R_BITS,
        "the slab's effective R is the parent commit's, on bits"
    );
    assert_eq!(
        slab.c_j_k.to_bits(),
        SLAB_PARENT_C_BITS,
        "the slab's capacitance is the parent commit's, on bits"
    );
    assert_eq!(
        slab.r_source,
        RValueSource::SlabFfactor,
        "the slab's winning source is the F-factor method"
    );
}

/// The parent commit's slab read-back, pinned on bits (captured at this
/// row's base before the change; the F-factor path is untouched).
const SLAB_PARENT_R_BITS: u64 = 2.171971496437055_f64.to_bits();
const SLAB_PARENT_C_BITS: u64 = 26488514.7648_f64.to_bits();
