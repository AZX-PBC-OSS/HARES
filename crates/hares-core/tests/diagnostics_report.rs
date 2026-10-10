//! The diagnostics report: the per-step diagnostics columns as catalogue
//! variables, created only when the run requests them.
//!
//! The request is `SimulationConfig::diagnostics`. Without it, a run pays
//! nothing for the diagnostics: no schema columns, no per-step capture, no
//! writer, no file. With it, the run's report carries the diagnostics
//! columns, whose values equal, row for row and to the printed precision,
//! the CSV the pre-change writer produced for the same fixture (pinned at
//! `fixtures/diagnostics_cz4a.csv`).

use std::path::PathBuf;

use chrono::{Duration, FixedOffset, TimeZone};
use hares_core::{Dwelling, DwellingConfig};
use hares_io::{OutputFormat, RotationPolicy, SimulationConfig, SiteLocationOverride};

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn cz4a_config(sim: SimulationConfig) -> DwellingConfig {
    let fixture = project_root().join("tests/fixtures/parity/cz4a_ashp_hpwh");
    DwellingConfig {
        hpxml_path: fixture.join("building.xml"),
        schedule_path: Some(fixture.join("schedule.csv")),
        weather_path: fixture.join("weather.epw"),
        sim_config: sim,
        defaults_path: Some(project_root().join("defaults")),
        overrides: None,
        bldg_id: 1,
        initialization_duration: None,
        patches: None,
    }
}

fn cz4a_sim_config() -> SimulationConfig {
    let start = FixedOffset::east_opt(-7 * 3600)
        .expect("offset")
        .with_ymd_and_hms(2023, 1, 1, 0, 0, 0)
        .single()
        .expect("start");
    SimulationConfig {
        start_time: start,
        duration: Duration::hours(1),
        time_res: Duration::seconds(60),
        output_verbosity: 0,
        write_output: false,
        output_path: None,
        output_format: OutputFormat::Csv,
        output_chunk_size: 1024,
        setpoint_deadband_c: None,
        master_seed: 0,
        civil_timezone: None,
        site_location: SiteLocationOverride::default(),
        retain_batches: false,
        rotation: RotationPolicy::None,
        max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
        diagnostics: false,
    }
}

/// The diagnostics columns, in the block order the run's report carries
/// them (the CSV's column order): three dwelling scalars, the per-zone
/// triples in zone order, the electrical pair, the envelope block.
fn diagnostics_columns(n_zones: usize) -> Vec<String> {
    let mut cols = vec![
        "step".to_string(),
        "timestamp_s".to_string(),
        "outdoor_temp_c".to_string(),
    ];
    for i in 1..=n_zones {
        cols.push(format!("zone{i}_temp_c"));
        cols.push(format!("zone{i}_thermal_gain_w"));
        cols.push(format!("zone{i}_latent_gain_w"));
    }
    cols.extend([
        "electrical_net_kw".to_string(),
        "electrical_net_kvar".to_string(),
        "port_radiant_w".to_string(),
        "port_shortwave_w".to_string(),
        "port_convective_w".to_string(),
        "window_solar_w".to_string(),
        "opaque_solar_lwr_w".to_string(),
        "interior_lwr_w".to_string(),
        "internal_gain_w".to_string(),
        "air_density_kg_m3".to_string(),
        "ghi_w_m2".to_string(),
    ]);
    cols
}

/// A `Verbosity(8)` report and no diagnostics request leave nothing behind
/// and record no diagnostics variable: the temporary output root is empty
/// after the run (the pre-change verbosity gate created a diagnostics CSV
/// beside the output path whether or not anything requested it), and the
/// recorded schema at verbosity 8 carries no diagnostics column.
#[test]
fn diagnostics_absent_without_a_diagnostics_report() {
    let dir = tempfile::tempdir().expect("temp dir");
    let output = dir.path().join("verb8.csv");

    let mut sim = cz4a_sim_config();
    sim.output_verbosity = 8;
    sim.write_output = false;
    sim.output_path = Some(output);
    let mut dwelling = Dwelling::from_config(cz4a_config(sim)).expect("fixture dwelling builds");
    dwelling.simulate().expect("run succeeds");
    drop(dwelling);

    let left: Vec<_> = std::fs::read_dir(dir.path())
        .expect("output root readable")
        .collect();
    assert!(
        left.is_empty(),
        "a run without a diagnostics request must create nothing, found: {left:?}"
    );

    // The recorded schema half: with output on and no diagnostics request,
    // no diagnostics column is recorded, at verbosity 8 included.
    let dir = tempfile::tempdir().expect("temp dir");
    let output = dir.path().join("verb8b.csv");
    let mut sim = cz4a_sim_config();
    sim.output_verbosity = 8;
    sim.write_output = true;
    sim.output_path = Some(output);
    let mut dwelling = Dwelling::from_config(cz4a_config(sim)).expect("fixture dwelling builds");
    dwelling.simulate().expect("run succeeds");
    let columns = dwelling
        .output_column_names()
        .expect("the recorded output schema");
    for name in diagnostics_columns(2) {
        assert!(
            !columns.iter().any(|c| c == &name),
            "'{name}' must not be recorded without a diagnostics request"
        );
    }
}

/// A `Verbosity(0)` report plus the diagnostics request yields the
/// diagnostics columns, and their values equal, row for row and to the
/// printed precision, the CSV the pre-change writer produced for the same
/// fixture (pinned at `fixtures/diagnostics_cz4a.csv`).
#[test]
fn diagnostics_in_memory_with_verbosity_0_report() {
    let dir = tempfile::tempdir().expect("temp dir");
    let output = dir.path().join("diag.csv");

    let mut sim = cz4a_sim_config();
    sim.diagnostics = true;
    sim.retain_batches = true;
    sim.write_output = true;
    sim.output_path = Some(output);
    let mut dwelling = Dwelling::from_config(cz4a_config(sim)).expect("fixture dwelling builds");
    dwelling.simulate().expect("run succeeds");

    let columns = dwelling
        .output_column_names()
        .expect("the recorded output schema");
    let expected = diagnostics_columns(2);
    for name in &expected {
        assert!(
            columns.iter().any(|c| c == name),
            "the requested diagnostics report must record '{name}'"
        );
    }

    // The pinned CSV: comment lines dropped, then the header, then the data
    // rows.
    let pin = std::fs::read_to_string(
        project_root().join("crates/hares-core/tests/fixtures/diagnostics_cz4a.csv"),
    )
    .expect("the pinned diagnostics CSV");
    let lines: Vec<&str> = pin
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .collect();
    let header: Vec<&str> = lines[0].split(',').collect();
    assert_eq!(
        header, expected,
        "the pin's column order is the block order"
    );
    let data: Vec<Vec<&str>> = lines[1..].iter().map(|l| l.split(',').collect()).collect();
    assert_eq!(data.len(), 60, "the pin holds the fixture's 60 steps");

    // The frame's diagnostics columns, row for row, at the CSV's printed
    // precision.
    let batches = dwelling.flushed_batches();
    let frame_rows: usize = batches
        .iter()
        .map(arrow::record_batch::RecordBatch::num_rows)
        .sum();
    assert_eq!(frame_rows, data.len(), "one frame row per pin row");

    // The printed precision the pre-change writer used per column.
    let precision = |name: &str| -> usize {
        match name {
            "step" => 0,
            "timestamp_s" => 1,
            "outdoor_temp_c" => 2,
            "air_density_kg_m3" => 5,
            n if n.ends_with("_temp_c")
                || n == "electrical_net_kw"
                || n == "electrical_net_kvar" =>
            {
                4
            }
            _ => 1,
        }
    };

    let mut row_cursor = 0usize;
    for batch in batches {
        let values_by_name: Vec<(&str, &arrow::array::Float64Array)> = expected
            .iter()
            .map(|name| {
                let column = batch
                    .column_by_name(name)
                    .unwrap_or_else(|| panic!("diagnostics column '{name}' missing"));
                (
                    name.as_str(),
                    column
                        .as_any()
                        .downcast_ref::<arrow::array::Float64Array>()
                        .expect("diagnostics columns are Float64"),
                )
            })
            .collect();
        for row_idx in 0..batch.num_rows() {
            let pinned_row = &data[row_cursor];
            row_cursor += 1;
            for (col_idx, (name, values)) in values_by_name.iter().enumerate() {
                let value = values.value(row_idx);
                let digits = precision(name);
                let printed = if digits == 0 {
                    format!("{}", value as i64)
                } else {
                    format!("{value:.digits$}")
                };
                let pin_cell = pinned_row[col_idx];
                assert!(
                    !pin_cell.is_empty(),
                    "pin cell ({row_cursor}, {name}) is empty but the run's screens keep the \
                     frame's values finite"
                );
                assert_eq!(
                    printed, pin_cell,
                    "row {row_cursor}, column '{name}': the frame must carry the pin's value"
                );
            }
        }
    }
}

/// A run without the diagnostics request produces the frames the requested
/// run produces, minus the diagnostics columns: the shared columns are
/// bitwise equal, in order.
#[test]
fn without_the_request_frames_equal_the_requested_frames_minus_diagnostics() {
    let run = |diagnostics: bool, root: &tempfile::TempDir| -> (Vec<String>, Vec<Vec<u64>>) {
        let output = root.path().join("frame.csv");
        let mut sim = cz4a_sim_config();
        sim.output_verbosity = 2;
        sim.diagnostics = diagnostics;
        sim.retain_batches = true;
        sim.write_output = true;
        sim.output_path = Some(output);
        let mut dwelling =
            Dwelling::from_config(cz4a_config(sim)).expect("fixture dwelling builds");
        dwelling.simulate().expect("run succeeds");
        let names = dwelling
            .output_column_names()
            .expect("the recorded output schema");
        let batches = dwelling.flushed_batches();
        let rows: Vec<Vec<u64>> = (0..batches[0].num_rows())
            .map(|r| {
                (1..names.len())
                    .map(|c| {
                        batches[0]
                            .column(c)
                            .as_any()
                            .downcast_ref::<arrow::array::Float64Array>()
                            .expect("value columns are Float64")
                            .value(r)
                            .to_bits()
                    })
                    .collect()
            })
            .collect();
        (names, rows)
    };

    let without_root = tempfile::tempdir().expect("temp dir");
    let (without_names, without_rows) = run(false, &without_root);
    let with_root = tempfile::tempdir().expect("temp dir");
    let (with_names, with_rows_full) = run(true, &with_root);

    // The requested run's schema is the other's plus the diagnostics block,
    // appended last.
    let diag_cols = diagnostics_columns(2);
    assert_eq!(
        with_names.len(),
        without_names.len() + diag_cols.len(),
        "the requested run's schema adds exactly the diagnostics columns"
    );
    assert_eq!(&with_names[without_names.len()..], &diag_cols);
    assert_eq!(without_names, with_names[..without_names.len()]);

    // The shared columns: bitwise equal, every row.
    let shared = without_names.len() - 1;
    let with_rows: Vec<Vec<u64>> = with_rows_full
        .iter()
        .map(|row| row[..shared].to_vec())
        .collect();
    assert_eq!(without_rows, with_rows, "the shared columns must not move");
}
