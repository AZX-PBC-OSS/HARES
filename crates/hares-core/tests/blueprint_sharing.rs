//! The parsed-input sharing acceptance tests: parsed inputs (the HPXML
//! documents, the weather files, the schedules, the defaults) are parsed
//! once and shared immutably (Arc) across the dwellings built from them,
//! as paths or as content.

use std::path::PathBuf;
use std::sync::Arc;

use arrow::array::{Array, Float64Array, RecordBatch, StringArray};
use arrow::datatypes::DataType;
use chrono::{DateTime, Duration, FixedOffset, TimeZone};
use hares_core::{
    Dwelling, DwellingBlueprint, DwellingBlueprintBuilder, DwellingConfig, DwellingSettings,
};
use hares_io::{
    DefaultsSource, Destination, Frequency, InputSource, ParsedDefaults, ParsedHpxml, ParsedInputs,
    ParsedSchedule, ParsedWeather, ReportSpec, Selection, SimulationConfig, SiteLocationOverride,
    WeatherFormat, WeatherInput,
};

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn site_offset() -> FixedOffset {
    FixedOffset::east_opt(-7 * 3600).expect("offset")
}

fn sim_config(start_time: DateTime<FixedOffset>, duration_s: i64, step_s: i64) -> SimulationConfig {
    SimulationConfig {
        start_time,
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
    }
}

/// The parity fixture's config: one HPXML, one schedule, one weather file,
/// the repo's defaults tree.
fn cz4a_config(start: DateTime<FixedOffset>, duration_s: i64) -> DwellingConfig {
    let fixture = project_root().join("tests/fixtures/parity/cz4a_ashp_hpwh");
    DwellingConfig {
        hpxml: InputSource::Path(fixture.join("building.xml")),
        schedule: Some(InputSource::Path(fixture.join("schedule.csv"))),
        weather: WeatherInput::Path(fixture.join("weather.epw")),
        defaults: DefaultsSource::directory(project_root().join("defaults")),
        settings: DwellingSettings {
            sim_config: sim_config(start, duration_s, 900),
            overrides: None,
            bldg_id: 1,
            initialization_duration: None,
            patches: None,
        },
    }
}

fn start_at() -> DateTime<FixedOffset> {
    site_offset()
        .with_ymd_and_hms(2023, 1, 1, 0, 0, 0)
        .single()
        .expect("start")
}

fn run_and_collect(dwelling: &mut Dwelling) -> Vec<RecordBatch> {
    dwelling.simulate().expect("the run completes");
    dwelling.flushed_batches().to_vec()
}

/// Every column of every batch compared bitwise: float64 columns by their
/// bit patterns (so a sign flip or a payload difference cannot hide), Utf8
/// columns by their bytes.
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
                            "{context}: batch {batch_index} column {col} row {row}"
                        );
                    }
                }
                _ => assert!(cx == cy, "{context}: batch {batch_index} column {col}"),
            }
        }
    }
}

/// Two dwellings built from one `Arc<DwellingBlueprint>` produce
/// bitwise-equal runs: the build clones the blueprint's rng and equipment
/// specs and shares every parsed input.
#[test]
fn blueprint_build_is_repeatable() {
    let blueprint = DwellingBlueprint::from_config(cz4a_config(start_at(), 86_400))
        .expect("the blueprint builds");
    let mut d1 = blueprint.build().expect("the first build");
    let mut d2 = blueprint.build().expect("the second build");
    let f1 = run_and_collect(&mut d1);
    let f2 = run_and_collect(&mut d2);
    assert!(!f1.is_empty(), "the run produced frames");
    assert_frames_bitwise_equal(&f1, &f2, "two builds of one blueprint");
}

/// Both dwellings hold the same parsed-input handles (`Arc::ptr_eq`), so
/// no input was parsed twice.
#[test]
fn builds_share_parsed_inputs() {
    let blueprint = DwellingBlueprint::from_config(cz4a_config(start_at(), 3600))
        .expect("the blueprint builds");
    let d1 = blueprint.build().expect("the first build");
    let d2 = blueprint.build().expect("the second build");

    let i1 = d1.blueprint().inputs();
    let i2 = d2.blueprint().inputs();
    assert!(Arc::ptr_eq(&i1.hpxml, &i2.hpxml), "one HPXML parse");
    assert!(Arc::ptr_eq(&i1.weather, &i2.weather), "one weather parse");
    assert!(
        Arc::ptr_eq(&i1.schedule, &i2.schedule),
        "one schedule parse"
    );
    assert!(Arc::ptr_eq(&i1.defaults, &i2.defaults), "one defaults load");
}

/// Blueprints from different HPXML documents, built over one shared
/// weather handle and one shared schedule handle, hold the same weather
/// `Arc` and the same resampled-series `Arc`, and each one's run is
/// bitwise the run of a blueprint that parsed its own inputs.
#[test]
fn blueprints_from_different_hpxml_share_weather() {
    let weather_path = project_root().join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw");
    let schedule_path = project_root().join("data/examples/BEopt_example_schedule.csv");
    let defaults_dir = project_root().join("defaults");
    let weather = Arc::new(
        ParsedWeather::parse(&WeatherInput::Path(weather_path.clone())).expect("weather parses"),
    );
    let defaults = Arc::new(
        ParsedDefaults::parse(&DefaultsSource::directory(defaults_dir.clone()))
            .expect("defaults load"),
    );
    let schedule = Arc::new(
        ParsedSchedule::parse(
            &InputSource::Path(schedule_path.clone()),
            Some(weather.meta()),
        )
        .expect("schedule parses"),
    );

    let start = start_at();
    let settings_for = |bldg_id: i64| DwellingSettings {
        sim_config: sim_config(start, 6 * 3600, 900),
        overrides: None,
        bldg_id,
        initialization_duration: None,
        patches: None,
    };

    let mut shared_blueprints = Vec::new();
    for (name, bldg_id) in [("base.xml", 1), ("base-pv.xml", 2)] {
        let hpxml = Arc::new(
            ParsedHpxml::parse(&InputSource::Path(
                project_root()
                    .join("tests/fixtures/hpxml/ochre_samples")
                    .join(name),
            ))
            .expect("hpxml parses"),
        );
        let inputs = ParsedInputs::from_handles(
            hpxml,
            Some(Arc::clone(&schedule)),
            Arc::clone(&weather),
            Arc::clone(&defaults),
            None,
        )
        .expect("the shared inputs assemble");
        let builder = DwellingBlueprintBuilder::new(inputs, settings_for(bldg_id))
            .expect("the builder builds");
        shared_blueprints.push(builder.freeze().expect("the blueprint freezes"));
    }

    // The shared handles: both blueprints hold the caller's weather handle
    // and the same resampled series.
    assert!(
        Arc::ptr_eq(
            &shared_blueprints[0].inputs().weather,
            &shared_blueprints[1].inputs().weather
        ),
        "one weather parse"
    );
    assert!(
        Arc::ptr_eq(
            &shared_blueprints[0].resampled_weather(),
            &shared_blueprints[1].resampled_weather()
        ),
        "one resampled series"
    );

    // The runs match the own-inputs parses bitwise.
    for (blueprint, name, bldg_id) in [
        (Arc::clone(&shared_blueprints[0]), "base.xml", 1),
        (Arc::clone(&shared_blueprints[1]), "base-pv.xml", 2),
    ] {
        let own = DwellingBlueprint::from_config(DwellingConfig {
            hpxml: InputSource::Path(
                project_root()
                    .join("tests/fixtures/hpxml/ochre_samples")
                    .join(name),
            ),
            schedule: Some(InputSource::Path(schedule_path.clone())),
            weather: WeatherInput::Path(weather_path.clone()),
            defaults: DefaultsSource::directory(defaults_dir.clone()),
            settings: settings_for(bldg_id),
        })
        .expect("the own-inputs blueprint builds");

        let mut d_shared = blueprint.build().expect("the shared build");
        let mut d_own = own.build().expect("the own build");
        let f_shared = run_and_collect(&mut d_shared);
        let f_own = run_and_collect(&mut d_own);
        assert_frames_bitwise_equal(
            &f_shared,
            &f_own,
            &format!("{name}: shared weather vs own parse"),
        );
    }
}

/// HPXML, weather and schedule given as content, and defaults given as the
/// repository directory with `envelope/Envelope Materials.csv` replaced by
/// edited content, produce a run bitwise equal to a run from paths with
/// that file edited in a copied directory.
#[test]
fn content_source_run_equals_path_source_run() {
    let fixture = project_root().join("tests/fixtures/resstock/2025.1/bldg0176227");
    let weather_path =
        project_root().join("tests/fixtures/resstock/2025.1/weather/G0600770_2018.csv");
    let materials_fixture =
        project_root().join("tests/fixtures/golden/consumer_shape_envelope_materials.csv");

    let hpxml_text = std::fs::read_to_string(fixture.join("home.xml")).expect("the home xml reads");
    let schedule_text =
        std::fs::read_to_string(fixture.join("in.schedules.csv")).expect("the schedule reads");
    let weather_text = std::fs::read_to_string(&weather_path).expect("the weather reads");
    let materials_text =
        std::fs::read_to_string(&materials_fixture).expect("the replacement materials read");

    // The path side: a copied defaults tree with the replacement file
    // edited on disk, the frame_golden tool's own recipe.
    let copied = tempfile::tempdir().expect("temp dir");
    copy_tree(&project_root().join("defaults"), copied.path());
    std::fs::copy(
        &materials_fixture,
        copied.path().join("envelope/Envelope Materials.csv"),
    )
    .expect("the replacement copies");

    let start = FixedOffset::east_opt(-8 * 3600)
        .expect("offset")
        .with_ymd_and_hms(2018, 1, 1, 0, 0, 0)
        .single()
        .expect("start");
    let settings =
        DwellingSettings::new(sim_config(start, 24 * 3600, 900), None, 176_227, None, None);

    let mut content_dwelling = Dwelling::from_config(DwellingConfig {
        hpxml: InputSource::Text(hpxml_text.into()),
        schedule: Some(InputSource::Text(schedule_text.into())),
        weather: WeatherInput::Text {
            content: weather_text.into(),
            format: WeatherFormat::ResStockCsv,
        },
        defaults: DefaultsSource {
            base: Some(project_root().join("defaults")),
            files: [(
                "envelope/Envelope Materials.csv".to_string(),
                InputSource::Text(materials_text.clone().into()),
            )]
            .into_iter()
            .collect(),
        },
        settings: settings.clone(),
    })
    .expect("the content config builds");

    let mut path_dwelling = Dwelling::from_config(DwellingConfig {
        hpxml: InputSource::Path(fixture.join("home.xml")),
        schedule: Some(InputSource::Path(fixture.join("in.schedules.csv"))),
        weather: WeatherInput::Path(weather_path.clone()),
        defaults: DefaultsSource::directory(copied.path().to_path_buf()),
        settings,
    })
    .expect("the path config builds");

    let f_content = run_and_collect(&mut content_dwelling);
    let f_path = run_and_collect(&mut path_dwelling);
    assert!(!f_content.is_empty(), "the content run produced frames");
    assert_frames_bitwise_equal(&f_content, &f_path, "content sources vs path sources");
}

/// Replacing an unknown defaults file is an error naming the known files.
#[test]
fn unknown_defaults_file_is_an_error() {
    let parsed = ParsedDefaults::parse(&DefaultsSource::directory(project_root().join("defaults")))
        .expect("the repo defaults load");
    let err = parsed
        .with_file(
            "no-such-directory/no-such-file.csv",
            InputSource::Text("x".into()),
        )
        .expect_err("an unknown relative path is an error");
    let message = err.to_string();
    assert!(
        message.contains("no-such-directory/no-such-file.csv"),
        "the error names the unknown path: {message}"
    );
    assert!(
        message.contains("zip_parameters.toml"),
        "the error names the known files: {message}"
    );
}

/// The fleet's site-offset pre-pass reads content sources from memory: a
/// config whose HPXML and weather are content resolves the same offset the
/// path form resolves.
#[test]
fn site_offset_reads_content_sources() {
    let fixture = project_root().join("tests/fixtures/parity/cz4a_ashp_hpwh");
    let path_config = cz4a_config(start_at(), 3600);
    let content_config = DwellingConfig {
        hpxml: InputSource::Text(
            std::fs::read_to_string(fixture.join("building.xml"))
                .expect("the home xml reads")
                .into(),
        ),
        schedule: None,
        weather: WeatherInput::Text {
            content: std::fs::read_to_string(fixture.join("weather.epw"))
                .expect("the weather reads")
                .into(),
            format: WeatherFormat::Epw,
        },
        defaults: DefaultsSource::directory(project_root().join("defaults")),
        settings: path_config.settings.clone(),
    };
    let from_paths =
        hares_core::dwelling::site_offset(&path_config).expect("the path offset resolves");
    let from_content =
        hares_core::dwelling::site_offset(&content_config).expect("the content offset resolves");
    assert_eq!(
        from_paths, from_content,
        "the pre-pass reads content from memory"
    );
}

fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
    for entry in std::fs::read_dir(src).expect("the source dir reads") {
        let entry = entry.expect("the entry reads");
        let target = dst.join(entry.file_name());
        if entry.file_type().expect("the type reads").is_dir() {
            std::fs::create_dir_all(&target).expect("the dir creates");
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("the file copies");
        }
    }
}
