mod common;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use hares_core::Dwelling;
use rayon::prelude::*;
use std::time::Duration;

fn bench_rl_step(c: &mut Criterion) {
    let mut group = c.benchmark_group("rl_step");
    group.sample_size(20);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(2));

    group.bench_function("dwelling_step_single", |b| {
        b.iter_batched(
            || {
                let path = common::unique_temp_path("hares-bench-rl-single", "toml");
                common::synthetic_toml_case(&path, 86_400);
                Dwelling::from_toml_config(&path).expect("create dwelling")
            },
            |mut dwelling| {
                let _ = dwelling.step().expect("single dwelling step");
            },
            criterion::BatchSize::SmallInput,
        );
    });

    for n in [10usize, 100usize] {
        group.bench_with_input(BenchmarkId::new("vec_dwelling_step", n), &n, |b, n| {
            b.iter_batched(
                || {
                    (0..*n)
                        .map(|idx| {
                            let path = common::unique_temp_path(
                                &format!("hares-bench-rl-vec-{idx}"),
                                "toml",
                            );
                            common::synthetic_toml_case(&path, 86_400);
                            Dwelling::from_toml_config(&path).expect("create vector dwelling")
                        })
                        .collect::<Vec<_>>()
                },
                |mut dwellings| {
                    dwellings.par_iter_mut().for_each(|dwelling| {
                        let _ = dwelling.step().expect("vector dwelling step");
                    });
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

criterion_group!(benches, bench_rl_step);
criterion_main!(benches);
