mod common;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use hares_core::SimulationEngine;
use std::time::Duration;

fn bench_single_building(c: &mut Criterion) {
    let temp = tempfile::tempdir().expect("benchmark directory");
    let dir = temp.path();
    let schedule_path = common::numbered_path(dir, "hares-bench-schedule", "csv");
    let weather_path = common::numbered_path(dir, "hares-bench-weather", "epw");
    common::write_schedule_csv(&schedule_path);
    common::write_weather_epw(&weather_path);

    let engine = SimulationEngine::new();
    let mut group = c.benchmark_group("single_building");
    group.sample_size(10);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(2));

    for (label, duration) in [
        ("30_day", chrono::Duration::days(30)),
        ("1_year", chrono::Duration::days(365)),
    ] {
        group.bench_with_input(
            BenchmarkId::from_parameter(label),
            &duration,
            |b, duration| {
                b.iter(|| {
                    let cfg = common::build_dwelling_config(
                        dir,
                        1,
                        schedule_path.clone(),
                        weather_path.clone(),
                        Some(common::repo_defaults_path()),
                        *duration,
                    );
                    let _ = engine.run(cfg).expect("single building benchmark run");
                });
            },
        );
    }

    group.finish();
}

/// The destinations entry's bench: the output pipeline's share of a run
/// under the four destination shapes the measurement compares.
fn bench_output_destination(c: &mut Criterion) {
    let temp = tempfile::tempdir().expect("benchmark directory");
    let dir = temp.path();
    let schedule_path = common::numbered_path(dir, "hares-bench-schedule", "csv");
    let weather_path = common::numbered_path(dir, "hares-bench-weather", "epw");
    common::write_schedule_csv(&schedule_path);
    common::write_weather_epw(&weather_path);

    let engine = SimulationEngine::new();
    let mut group = c.benchmark_group("output_destination");
    group.sample_size(10);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(2));

    for label in ["no_report", "memory", "csv", "parquet"] {
        group.bench_function(label, |b| {
            b.iter(|| {
                let mut cfg = common::build_dwelling_config(
                    dir,
                    1,
                    schedule_path.clone(),
                    weather_path.clone(),
                    Some(common::repo_defaults_path()),
                    chrono::Duration::days(30),
                );
                cfg.settings.sim_config.reports = match label {
                    "no_report" => Vec::new(),
                    "memory" => vec![hares_io::ReportSpec {
                        name: "frame".to_string(),
                        selection: hares_io::Selection::Verbosity(2),
                        frequency: hares_io::Frequency::default(),
                        destination: hares_io::Destination::Memory,
                    }],
                    other => {
                        let ext = if other == "csv" { "csv" } else { "parquet" };
                        let path = common::numbered_path(dir, "hares-bench-report", ext);
                        let destination = if other == "csv" {
                            hares_io::Destination::Csv {
                                path,
                                overwrite: true,
                            }
                        } else {
                            hares_io::Destination::Parquet {
                                path,
                                overwrite: true,
                            }
                        };
                        vec![hares_io::ReportSpec {
                            name: "file".to_string(),
                            selection: hares_io::Selection::Verbosity(2),
                            frequency: hares_io::Frequency::default(),
                            destination,
                        }]
                    }
                };
                let _ = engine.run(cfg).expect("destination benchmark run");
            });
        });
    }

    group.finish();
}

criterion_group!(benches, bench_single_building, bench_output_destination);
criterion_main!(benches);
