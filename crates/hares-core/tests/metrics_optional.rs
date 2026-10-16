// The run metrics are computed only when requested, from one path: a
// fixed metrics input plan built from the catalogue at freeze, fed per
// step from the row plan's producer slots.
//
// These are the RED tests: they are written against the surface before it
// exists. At the parent commit the configuration rejects the `metrics`
// key (unknown key) and the run result's metrics are not an `Option`, so
// every test here fails at the config parse or does not compile until the
// row's change lands.

use hares_core::DwellingSettings;
use hares_io::{DefaultsSource, InputSource, WeatherInput};
use std::path::{Path, PathBuf};

use hares_core::{Dwelling, DwellingConfig, SimulationEngine};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The entry's fixture inputs: the HPXML, schedule and weather of the
/// parity fixture, no warm-up, 900 s steps, over 24 h.
fn dwelling_config(sim_toml: &str) -> DwellingConfig {
    dwelling_config_from(sim_toml, "tests/fixtures/parity/cz4a_ashp_hpwh")
}

/// The same config shape over a stated fixture directory.
fn dwelling_config_from(sim_toml: &str, fixture: &str) -> DwellingConfig {
    let home = repo_root().join(fixture);
    DwellingConfig {
        hpxml: InputSource::Path(home.join("building.xml")),
        schedule: Some(InputSource::Path(home.join("schedule.csv"))),
        weather: WeatherInput::Path(home.join("weather.epw")),
        defaults: DefaultsSource::directory(repo_root().join("defaults")),
        settings: DwellingSettings::new(
            hares_io::SimulationConfig::from_toml(sim_toml).expect("sim config parses"),
            None,
            1,
            None,
        ),
    }
}

fn sim_toml(reports: &str, metrics: bool) -> String {
    format!(
        r#"
start_time = "2024-02-27T00:00:00-07:00"
duration = 86400
time_res = 900
metrics = {metrics}
{reports}
"#
    )
}

fn memory_report(name: &str, verbosity: u8) -> String {
    format!(
        "\n[[reports]]\nname = \"{name}\"\nselection = {{ verbosity = {verbosity} }}\ndestination = \"memory\"\n"
    )
}

fn parquet_report(name: &str, verbosity: u8, path: &Path) -> String {
    format!(
        "\n[[reports]]\nname = \"{name}\"\nselection = {{ verbosity = {verbosity} }}\n\
         destination = {{ parquet = {{ path = \"{}\", overwrite = true }} }}\n",
        path.display()
    )
}

/// `metrics_absent_unless_requested`: a run that does not request metrics
/// returns no metrics.
#[test]
fn metrics_absent_unless_requested() {
    let toml = sim_toml(&memory_report("frame", 2), false);
    let engine = SimulationEngine::new();
    let result = engine.run(dwelling_config(&toml)).expect("run completes");
    assert!(
        result.metrics.is_none(),
        "a run without the metrics request returns no metrics"
    );
}

/// `metrics_present_without_any_report`: metrics are requested, no report
/// is configured: the run still returns metrics, computed from the
/// producer surface (the row plan's full declared surface).
#[test]
fn metrics_present_without_any_report() {
    let toml = sim_toml("", true);
    let engine = SimulationEngine::new();
    let result = engine.run(dwelling_config(&toml)).expect("run completes");
    let metrics = result
        .metrics
        .as_ref()
        .expect("a run that requests metrics returns them without any report");
    assert!(
        (metrics.simulation_duration_hours - 24.0).abs() < 1e-9,
        "the metrics cover the run's 24 h, got {}",
        metrics.simulation_duration_hours
    );
    assert!(metrics.total_energy_kwh.net_energy_kwh.is_finite());
}

/// `metrics_independent_of_destination`: three runs whose reports select
/// the `Verbosity(0)` tier, one `Memory`, one `Parquet`, one with no
/// report at all: the three metrics are bitwise equal.
#[test]
fn metrics_independent_of_destination() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let engine = SimulationEngine::new();

    let memory = engine
        .run(dwelling_config(&sim_toml(&memory_report("frame", 0), true)))
        .expect("memory run completes");
    let file = engine
        .run(dwelling_config(&sim_toml(
            &parquet_report("frame", 0, &tmp.path().join("out.parquet")),
            true,
        )))
        .expect("parquet run completes");
    let none = engine
        .run(dwelling_config(&sim_toml("", true)))
        .expect("reportless run completes");

    let memory_metrics = memory.metrics.as_ref().expect("memory run has metrics");
    let file_metrics = file.metrics.as_ref().expect("parquet run has metrics");
    let none_metrics = none.metrics.as_ref().expect("reportless run has metrics");
    assert_eq!(
        memory_metrics, file_metrics,
        "the metrics are the same under a memory and a parquet report"
    );
    assert_eq!(
        memory_metrics, none_metrics,
        "the metrics are the same with no report at all"
    );
}

/// `metrics_request_does_not_change_the_frame`: one memory report, run
/// with and without the metrics request: the frames are bitwise equal.
#[test]
fn metrics_request_does_not_change_the_frame() {
    let run = |metrics: bool| {
        let mut dwelling = Dwelling::from_config(dwelling_config(&sim_toml(
            &memory_report("frame", 2),
            metrics,
        )))
        .expect("dwelling builds");
        dwelling.simulate().expect("run completes");
        dwelling.flushed_batches().to_vec()
    };
    let without = run(false);
    let with = run(true);

    assert!(!without.is_empty(), "the memory report retains batches");
    assert_eq!(without.len(), with.len(), "the same batch count");
    for (a, b) in without.iter().zip(with.iter()) {
        assert_eq!(a.schema(), b.schema(), "the same schema");
        assert_eq!(a.num_rows(), b.num_rows(), "the same row count");
        for (col_a, col_b) in a.columns().iter().zip(b.columns()) {
            let data_a = col_a.to_data();
            let data_b = col_b.to_data();
            assert_eq!(
                data_a.null_count(),
                data_b.null_count(),
                "every column's null count is bitwise equal"
            );
            assert_eq!(
                data_a.buffers(),
                data_b.buffers(),
                "every column's value bytes are bitwise equal"
            );
        }
    }
}

/// The r38 coupling dies here: a run whose first report is a narrow named
/// selection that lacks the power column builds and runs when no metrics
/// are requested. At the parent this config is refused at freeze, because
/// the metrics calculator rode the first recorder unconditionally.
#[test]
fn narrow_first_report_builds_and_runs_without_the_metrics_request() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let target = tmp.path().join("lean.csv");
    let toml = format!(
        r#"
start_time = "2024-02-27T00:00:00-07:00"
duration = 86400
time_res = 900
metrics = false

[[reports]]
name = "lean"
selection = {{ named = ["Outdoor Dry Bulb (C)"] }}
destination = {{ csv = {{ path = "{}", overwrite = true }} }}
"#,
        target.display()
    );
    let mut dwelling = Dwelling::from_config(dwelling_config(&toml))
        .expect("a narrow first report builds without the metrics request");
    dwelling.simulate().expect("the run completes");
    assert!(target.exists(), "the narrow report published its file");
}

/// The refusal of the decoupling entry dies here: that entry refused a
/// run that requested metrics from a selection union without the power
/// column, because the metrics' inputs resolved from the reports'
/// selections. They resolve from the producer surface now, so a narrow
/// first report no longer starves them, and the same run's metrics equal
/// a verbosity-8 run's.
#[test]
fn narrow_first_report_with_the_metrics_request_returns_full_metrics() {
    let engine = SimulationEngine::new();
    let run = |toml: String| {
        let result = engine.run(dwelling_config(&toml)).expect("run completes");
        result
            .metrics
            .as_ref()
            .expect("the run requested metrics")
            .clone()
    };
    let narrow = r#"
start_time = "2024-02-27T00:00:00-07:00"
duration = 86400
time_res = 900
metrics = true

[[reports]]
name = "lean"
selection = { named = ["Outdoor Dry Bulb (C)"] }
destination = "memory"
"#
    .to_string();
    let wide = r#"
start_time = "2024-02-27T00:00:00-07:00"
duration = 86400
time_res = 900
metrics = true

[[reports]]
name = "frame"
selection = { verbosity = 8 }
destination = "memory"
"#
    .to_string();
    assert_eq!(
        run(narrow),
        run(wide),
        "a narrow report's metrics equal a verbosity-8 run's"
    );
}

/// `metrics_equal_at_every_verbosity_and_with_no_report`. The same
/// dwelling, `metrics = true`, at verbosity 0, 4 and 8 and with no report
/// at all: the four `RunMetrics` are equal. At the parent they are not:
/// the envelope-component metrics compute only when the recorded schema
/// carries the verbosity-4 delivered columns and the verbosity-6 envelope
/// gain columns, and the per-end-use maps and the renewable fraction
/// collapse at verbosity 0, where no aggregate column is selected. The
/// fix resolves the metrics' inputs from the producer surface (the row
/// plan's full declared surface), not from the reports' selected columns.
#[test]
fn metrics_equal_at_every_verbosity_and_with_no_report() {
    let engine = SimulationEngine::new();
    let run = |reports: &str| {
        let toml = format!(
            r#"
start_time = "2023-01-01T00:00:00-07:00"
duration = 604800
time_res = 3600
warmup = "disabled"
master_seed = 0
metrics = true
{reports}
"#
        );
        let result = engine
            .run(dwelling_config_from(
                &toml,
                "tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh",
            ))
            .expect("run completes");
        result
            .metrics
            .as_ref()
            .expect("the run requested metrics")
            .clone()
    };
    let at = |verbosity: u8| {
        run(&format!(
            "\n[[reports]]\nname = \"frame\"\nselection = {{ verbosity = {verbosity} }}\ndestination = \"memory\"\n"
        ))
    };

    let at_0 = at(0);
    let at_4 = at(4);
    let at_8 = at(8);
    let none = run("");

    assert_eq!(
        at_0, at_8,
        "the metrics are equal at verbosity 0 and verbosity 8"
    );
    assert_eq!(
        at_4, at_8,
        "the metrics are equal at verbosity 4 and verbosity 8"
    );
    assert_eq!(
        none, at_8,
        "the metrics are equal with no report at all and at verbosity 8"
    );
}
