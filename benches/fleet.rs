mod common;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use hares_fleet::Fleet;
use std::time::Duration;

fn bench_fleet(c: &mut Criterion) {
    let temp = tempfile::tempdir().expect("benchmark directory");
    let dir = temp.path();
    let schedule_path = common::numbered_path(dir, "hares-bench-fleet-schedule", "csv");
    let weather_path = common::numbered_path(dir, "hares-bench-fleet-weather", "epw");
    common::write_schedule_csv(&schedule_path);
    common::write_weather_epw(&weather_path);

    let mut group = c.benchmark_group("fleet");
    group.sample_size(10);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(5));

    for n in [10usize, 100usize] {
        group.bench_with_input(BenchmarkId::new("simulate_dwellings", n), &n, |b, &n| {
            b.iter_batched(
                || {
                    let configs = (0..n)
                        .map(|idx| {
                            common::build_dwelling_config(
                                dir,
                                idx as i64 + 1,
                                schedule_path.clone(),
                                weather_path.clone(),
                                Some(common::repo_defaults_path()),
                                chrono::Duration::days(1),
                            )
                        })
                        .collect::<Vec<_>>();
                    Fleet::from_buildings(configs)
                },
                |fleet| {
                    // Use a fixed thread count for reproducible, resource-bounded benchmarks.
                    let results = fleet.simulate(4);
                    std::hint::black_box(results);
                },
                criterion::BatchSize::LargeInput,
            );
        });
    }

    group.finish();
}

criterion_group!(benches, bench_fleet);
criterion_main!(benches);
