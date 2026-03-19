mod common;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use hares_core::SimulationEngine;
use std::time::Duration;

fn bench_single_building(c: &mut Criterion) {
    let schedule_path = common::unique_temp_path("hares-bench-schedule", "csv");
    let weather_path = common::unique_temp_path("hares-bench-weather", "epw");
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
                        1,
                        schedule_path.clone(),
                        weather_path.clone(),
                        *duration,
                    );
                    let _ = engine.run(cfg).expect("single building benchmark run");
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_single_building);
criterion_main!(benches);
