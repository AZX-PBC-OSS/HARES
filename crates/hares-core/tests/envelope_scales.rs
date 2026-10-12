//! The envelope scales' acceptance: the reserved `envelope` override's two
//! factors act on the selected LUT row's values, never on the selection
//! itself. The resistance scale multiplies every opaque layer's resistance
//! and the thermal-mass scale every such layer's capacitance, films
//! unscaled and the F-factor slab outside both, so the resolved R is
//! continuous in the scale where the row's capacitance never moves.

use std::path::PathBuf;

use arrow::array::{Array, Float64Array, RecordBatch, StringArray};
use arrow::datatypes::DataType;
use chrono::{DateTime, Duration, FixedOffset, TimeZone};
use hares_core::{
    DwellingBlueprint, DwellingConfig, DwellingSettings, ResolvedBoundary, ResolvedModel,
};
use hares_io::{
    DefaultsSource, Destination, Frequency, InputSource, ReportSpec, Selection, SimulationConfig,
    SiteLocationOverride, WeatherInput,
};

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn denver_offset() -> FixedOffset {
    FixedOffset::east_opt(-7 * 3600).expect("offset")
}

fn start_at() -> DateTime<FixedOffset> {
    denver_offset()
        .with_ymd_and_hms(2023, 1, 1, 0, 0, 0)
        .single()
        .expect("start")
}

fn sim_config(duration_s: i64, step_s: i64) -> SimulationConfig {
    SimulationConfig {
        start_time: start_at(),
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
    }
}

/// The ochre-samples home the never-reselects and continuity tests read:
/// `tests/fixtures/hpxml/ochre_samples/base.xml`, the Denver weather, the
/// BEopt schedule, the repo's defaults tree.
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
        defaults: DefaultsSource::directory(project_root().join("defaults")),
        settings: DwellingSettings {
            sim_config: sim_config(duration_s, 900),
            overrides,
            bldg_id: 1,
            initialization_duration: None,
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

/// The opaque boundaries the scales act on: every boundary that is neither
/// the F-factor slab nor fenestration.
fn scaled_boundaries(model: &ResolvedModel) -> Vec<&ResolvedBoundary> {
    model
        .boundaries
        .iter()
        .filter(|b| {
            !matches!(
                b.kind,
                hares_io::hpxml::BoundaryType::Slab
                    | hares_io::hpxml::BoundaryType::Window
                    | hares_io::hpxml::BoundaryType::Skylight
            )
        })
        .collect()
}

/// Unit scales are bitwise today's run: on every M-2 input set this test
/// can build from a config, the run with
/// `envelope = { opaque_r_scale = 1.0, thermal_mass_scale = 1.0 }` equals
/// the run without the override, every column of every batch by bits and
/// the metrics read the same.
#[test]
fn unit_envelope_scales_are_bitwise_neutral() {
    for (name, scales) in [
        ("cz2a", DwellingConfigCase::Cz2a),
        ("cz4a", DwellingConfigCase::Cz4a),
    ] {
        let baseline = run_and_collect(config_case(scales, 48 * 3600, 900, None));
        let unit = run_and_collect(config_case(scales, 48 * 3600, 900, Some((1.0, 1.0))));
        assert!(!baseline.is_empty(), "{name}: the run produced frames");
        assert_frames_bitwise_equal(&baseline, &unit, name);
    }
}

/// Which M-2 fixture's inputs a config case builds from.
#[derive(Clone, Copy)]
enum DwellingConfigCase {
    Cz2a,
    Cz4a,
}

fn config_case(
    case: DwellingConfigCase,
    duration_s: i64,
    step_s: i64,
    scales: Option<(f64, f64)>,
) -> DwellingConfig {
    match case {
        DwellingConfigCase::Cz2a => cz2a_config(duration_s, step_s, scales),
        DwellingConfigCase::Cz4a => cz4a_config(duration_s, step_s, scales),
    }
}

/// The resistance scale never re-selects the LUT row: across 0.5, 0.8,
/// 1.0, 1.25 and 2.0, every opaque boundary's resolved layer capacitances
/// equal the unscaled ones on bits (a moved row would carry different
/// ones) and its layer resistances are the unscaled ones times the scale.
#[test]
fn opaque_r_scale_never_reselects_the_row() {
    let base = resolved_for(base_xml_config(24 * 3600, Some((1.0, 1.0))));
    for scale in [0.5, 0.8, 1.0, 1.25, 2.0] {
        let model = resolved_for(base_xml_config(24 * 3600, Some((scale, 1.0))));
        for (unscaled, scaled) in scaled_boundaries(&base)
            .into_iter()
            .zip(scaled_boundaries(&model))
        {
            assert_eq!(
                unscaled.id, scaled.id,
                "the boundaries read in the same order"
            );
            assert_eq!(
                unscaled.layers.len(),
                scaled.layers.len(),
                "boundary {}: the layer count never moves with the scale",
                unscaled.id
            );
            for (u, s) in unscaled.layers.iter().zip(scaled.layers.iter()) {
                assert_eq!(
                    u.c_j_m2k.to_bits(),
                    s.c_j_m2k.to_bits(),
                    "boundary {}: the capacitance is the selected row's, on bits",
                    unscaled.id
                );
                let expected = u.r_m2k_w * scale;
                assert_eq!(
                    s.r_m2k_w.to_bits(),
                    expected.to_bits(),
                    "boundary {}: the layer resistance is the unscaled one times {scale}",
                    unscaled.id
                );
            }
        }
    }
}

/// The thermal-mass scale moves capacitance only: at
/// `thermal_mass_scale = 2.0` every opaque layer's capacitance doubles on
/// bits and its resistance is bitwise the unscaled one.
#[test]
fn thermal_mass_scale_moves_capacitance_only() {
    let base = resolved_for(base_xml_config(24 * 3600, Some((1.0, 1.0))));
    let model = resolved_for(base_xml_config(24 * 3600, Some((1.0, 2.0))));
    for (unscaled, scaled) in scaled_boundaries(&base)
        .into_iter()
        .zip(scaled_boundaries(&model))
    {
        assert_eq!(unscaled.id, scaled.id);
        for (u, s) in unscaled.layers.iter().zip(scaled.layers.iter()) {
            assert_eq!(
                u.r_m2k_w.to_bits(),
                s.r_m2k_w.to_bits(),
                "boundary {}: the resistance never moves with the mass scale",
                unscaled.id
            );
            let expected = u.c_j_m2k * 2.0;
            assert_eq!(
                s.c_j_m2k.to_bits(),
                expected.to_bits(),
                "boundary {}: the capacitance is the unscaled one times 2",
                unscaled.id
            );
        }
    }
}

/// The resolved R is continuous in the scale: across 200 scales from 0.5
/// to 2.0, every opaque non-slab boundary's `r_effective_m2k_w` minus its
/// films is proportional to the scale, with no step at a row boundary, and
/// the boundaries the scales do not act on (the F-factor slab, the
/// fenestration) do not move at all.
#[test]
fn resolved_r_is_continuous_in_the_scale() {
    let base = resolved_for(base_xml_config(24 * 3600, Some((1.0, 1.0))));
    let scales: Vec<f64> = (0..200).map(|i| 0.5 + (1.5 * i as f64 / 199.0)).collect();
    for scale in scales {
        let model = resolved_for(base_xml_config(24 * 3600, Some((scale, 1.0))));
        for (unscaled, scaled) in base.boundaries.iter().zip(model.boundaries.iter()) {
            assert_eq!(unscaled.id, scaled.id);
            let unscaled_material = unscaled.r_effective_m2k_w - unscaled.films_m2k_w;
            let material = scaled.r_effective_m2k_w - scaled.films_m2k_w;
            let opaque = !matches!(
                scaled.kind,
                hares_io::hpxml::BoundaryType::Slab
                    | hares_io::hpxml::BoundaryType::Window
                    | hares_io::hpxml::BoundaryType::Skylight
            );
            if opaque {
                let relative = (material / unscaled_material - scale).abs() / scale;
                assert!(
                    relative < 1e-9,
                    "boundary {} at scale {scale}: the material R moved {relative:.3e} off \
                     proportional; the selection re-ran",
                    scaled.id
                );
            } else {
                assert_eq!(
                    scaled.r_effective_m2k_w.to_bits(),
                    unscaled.r_effective_m2k_w.to_bits(),
                    "boundary {}: a boundary the scales do not act on never moves",
                    scaled.id
                );
            }
        }
    }
}

/// An out-of-bounds scale fails naming the field, its value and the bounds,
/// through the assembly the build runs.
#[test]
fn envelope_scale_out_of_bounds_is_rejected() {
    for (field, value) in [("opaque_r_scale", 0.0), ("thermal_mass_scale", 10.5)] {
        let overrides = serde_json::json!({"envelope": {field: value}});
        let mut config = base_xml_config(24 * 3600, None);
        config.settings.overrides = Some(overrides);
        let err = DwellingBlueprint::from_config(config)
            .expect("the blueprint builds")
            .validate()
            .expect_err("the scale is out of bounds");
        let message = err.to_string();
        assert!(
            message.contains(field) && message.contains("(0, 10]"),
            "the error names the field and the bounds: {message}"
        );
        assert!(
            message.contains(&format!("{value}")),
            "the error names the value: {message}"
        );
    }
}

/// An unknown field in the `envelope` override is rejected, naming it.
#[test]
fn unknown_envelope_override_field_is_rejected() {
    let overrides = serde_json::json!({"envelope": {"opaque_r_scales": 2.0}});
    let mut config = base_xml_config(24 * 3600, None);
    config.settings.overrides = Some(overrides);
    let err = DwellingBlueprint::from_config(config)
        .expect("the blueprint builds")
        .validate()
        .expect_err("the field is unknown");
    assert!(
        err.to_string().contains("opaque_r_scales"),
        "the error names the unknown field: {err}"
    );
}

/// The M-2 fixtures' configs with an optional envelope override.
fn cz2a_config(duration_s: i64, step_s: i64, scales: Option<(f64, f64)>) -> DwellingConfig {
    let fixture = project_root().join("tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh");
    config_for(
        fixture.join("building.xml"),
        fixture.join("schedule.csv"),
        fixture.join("weather.epw"),
        duration_s,
        step_s,
        scales,
    )
}

fn cz4a_config(duration_s: i64, step_s: i64, scales: Option<(f64, f64)>) -> DwellingConfig {
    let fixture = project_root().join("tests/fixtures/parity/cz4a_ashp_hpwh");
    config_for(
        fixture.join("building.xml"),
        fixture.join("schedule.csv"),
        fixture.join("weather.epw"),
        duration_s,
        step_s,
        scales,
    )
}

fn config_for(
    hpxml: PathBuf,
    schedule: PathBuf,
    weather: PathBuf,
    duration_s: i64,
    step_s: i64,
    scales: Option<(f64, f64)>,
) -> DwellingConfig {
    let overrides = scales.map(|(opaque_r_scale, thermal_mass_scale)| {
        serde_json::json!({"envelope": {"opaque_r_scale": opaque_r_scale, "thermal_mass_scale": thermal_mass_scale}})
    });
    DwellingConfig {
        hpxml: InputSource::Path(hpxml),
        schedule: Some(InputSource::Path(schedule)),
        weather: WeatherInput::Path(weather),
        defaults: DefaultsSource::directory(project_root().join("defaults")),
        settings: DwellingSettings {
            sim_config: sim_config(duration_s, step_s),
            overrides,
            bldg_id: 1,
            initialization_duration: None,
            patches: None,
        },
    }
}

fn run_and_collect(config: DwellingConfig) -> Vec<RecordBatch> {
    let mut dwelling = DwellingBlueprint::from_config(config)
        .expect("the blueprint builds")
        .build()
        .expect("the dwelling builds");
    dwelling.simulate().expect("the run completes");
    dwelling.flushed_batches().to_vec()
}

/// Every column of every batch compared bitwise: float64 columns by their
/// bit patterns, string columns by their bytes.
fn assert_frames_bitwise_equal(a: &[RecordBatch], b: &[RecordBatch], context: &str) {
    assert_eq!(a.len(), b.len(), "{context}: batch count");
    for (batch_index, (x, y)) in a.iter().zip(b.iter()).enumerate() {
        assert_eq!(
            x.num_rows(),
            y.num_rows(),
            "{context}: batch {batch_index} rows"
        );
        assert_eq!(
            x.num_columns(),
            y.num_columns(),
            "{context}: batch {batch_index} columns"
        );
        for (col, (cx, cy)) in x.columns().iter().zip(y.columns()).enumerate() {
            assert_eq!(
                cx.data_type(),
                cy.data_type(),
                "{context}: batch {batch_index} column {col} type"
            );
            match cx.data_type() {
                DataType::Float64 => {
                    let xa = cx
                        .as_any()
                        .downcast_ref::<Float64Array>()
                        .expect("f64 column");
                    let ya = cy
                        .as_any()
                        .downcast_ref::<Float64Array>()
                        .expect("f64 column");
                    for row in 0..x.num_rows() {
                        assert_eq!(
                            xa.is_null(row),
                            ya.is_null(row),
                            "{context}: batch {batch_index} column {col} row {row} null"
                        );
                        if !xa.is_null(row) {
                            assert_eq!(
                                xa.value(row).to_bits(),
                                ya.value(row).to_bits(),
                                "{context}: batch {batch_index} column {col} row {row} bits"
                            );
                        }
                    }
                }
                DataType::Utf8 | DataType::LargeUtf8 => {
                    let xa = cx
                        .as_any()
                        .downcast_ref::<StringArray>()
                        .expect("utf8 column");
                    let ya = cy
                        .as_any()
                        .downcast_ref::<StringArray>()
                        .expect("utf8 column");
                    for row in 0..x.num_rows() {
                        assert_eq!(
                            xa.value(row),
                            ya.value(row),
                            "{context}: batch {batch_index} column {col} row {row} bytes"
                        );
                    }
                }
                _ => assert!(cx == cy, "{context}: batch {batch_index} column {col}"),
            }
        }
    }
}
