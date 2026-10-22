mod common;

use std::hint::black_box;
use std::path::Path;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use hares_core::Dwelling;
use hares_types::alloc_count::{CountingAllocator, thread_allocations};
use rayon::prelude::*;

// ---------------------------------------------------------------------------
// Allocation tracking
// ---------------------------------------------------------------------------
// The bench installs the workspace's shared per-thread counting allocator.
// The parallel groups time uncounted; per-episode allocation counts come
// from the sequential counting pass in `bench_episode_vec`, which runs on
// the bench thread outside the timed loop and says so in its label.

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

fn alloc_snapshot() -> u64 {
    thread_allocations().expect("the bench installs the counting allocator")
}

// ---------------------------------------------------------------------------
// Observation field sets
// ---------------------------------------------------------------------------
// Synthetic dwelling (furnace + electricity) produces zone "Indoor" and
// equipment "Electric Furnace" (canonical HPXML heating name).

const NARROW_FIELDS: &[&str] = &[
    "outdoor_temp",
    "zone_temp[Indoor]",
    "total_power_w",
    "equipment_power[Electric Furnace]",
    "setpoint_heat[Indoor]",
];

fn wide_fields() -> Vec<&'static str> {
    let base: &[&str] = &[
        "outdoor_temp",
        "outdoor_humidity_ratio",
        "total_power_w",
        "reactive_power_var",
        "zone_temp[Indoor]",
        "setpoint_heat[Indoor]",
        "setpoint_cool[Indoor]",
        "zone_energy_balance[Indoor]",
        "equipment_soc[Electric Furnace]",
        "equipment_power[Electric Furnace]",
    ];
    let mut fields = Vec::with_capacity(55);
    while fields.len() < 55 {
        fields.extend_from_slice(base);
    }
    fields.truncate(55);
    fields
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const EPISODE_96: usize = 96;
const EPISODE_8760: usize = 8760;

// The bench's synthetic fixture is the shared one (benches/common.rs); its
// one construction path is exercised by the bench_construction suite test.
use common::synthetic_toml;

fn make_dwelling(dir: &Path, duration_s: i64, time_res_s: i64) -> Dwelling {
    let path = common::numbered_path(dir, "hares-bench-rl-episode", "toml");
    std::fs::write(&path, common::synthetic_toml(duration_s, time_res_s)).expect("write TOML");
    Dwelling::from_toml_config_with_reports(&path, Some(Vec::new())).expect("create dwelling")
}

// ---------------------------------------------------------------------------
// Benchmarks
// ---------------------------------------------------------------------------

fn bench_rl_episode(c: &mut Criterion) {
    let temp = tempfile::tempdir().expect("benchmark directory");
    let dir = temp.path();
    bench_reset(c, dir);
    bench_observation_vec(c, dir);
    bench_episode_single(c, dir);
    bench_episode_vec(c, dir);
    bench_batch_step(c, dir);
}

/// Benchmark: Dwelling construction (the "reset" path).
fn bench_reset(c: &mut Criterion, dir: &Path) {
    let mut group = c.benchmark_group("rl_episode/reset");
    group.sample_size(30);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(3));

    group.bench_function("from_toml_config", |b| {
        b.iter_batched(
            || common::numbered_path(dir, "hares-bench-rl-reset", "toml"),
            |path| {
                std::fs::write(&path, synthetic_toml(86_400, 60)).expect("write TOML");
                black_box(
                    Dwelling::from_toml_config_with_reports(&path, Some(Vec::new()))
                        .expect("create dwelling"),
                );
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.finish();
}

/// Benchmark: `to_observation_vec` with narrow (5) and wide (55) field sets.
fn bench_observation_vec(c: &mut Criterion, dir: &Path) {
    let mut group = c.benchmark_group("rl_episode/observation_vec");
    group.sample_size(40);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(2));
    group.throughput(Throughput::Elements(1));

    let mut dwelling = make_dwelling(dir, 86_400, 60);
    dwelling.step().expect("step");
    let telemetry = dwelling.telemetry().unwrap();

    group.bench_function("narrow_5_fields", |b| {
        b.iter(|| {
            black_box(
                telemetry
                    .observation_vec(
                        NARROW_FIELDS,
                        &mut hares_core::ObservationWarnOnce::default(),
                    )
                    .expect("obs"),
            )
        });
    });

    let wide = wide_fields();
    group.bench_function("wide_55_fields", |b| {
        b.iter(|| {
            black_box(
                telemetry
                    .observation_vec(&wide, &mut hares_core::ObservationWarnOnce::default())
                    .expect("obs"),
            )
        });
    });

    group.finish();
}

/// Benchmark: full episode lifecycle for a single dwelling.
///
/// Each episode: create dwelling → step N times (with telemetry + observation
/// after each step).  Measured at 96 steps (1 day / 15-min) and 8 760 steps
/// (1 year / 1-hour).
fn bench_episode_single(c: &mut Criterion, dir: &Path) {
    let mut group = c.benchmark_group("rl_episode/episode_single");
    group.throughput(Throughput::Elements(1));

    for &num_steps in &[EPISODE_96, EPISODE_8760] {
        let (time_res_s, duration_s, label) = if num_steps == EPISODE_96 {
            (900, 86_400, "96_steps")
        } else {
            (3600, 31_536_000, "8760_steps")
        };

        let sample_size = 10;
        let warm_up = if num_steps == EPISODE_96 {
            Duration::from_secs(2)
        } else {
            Duration::from_secs(10)
        };
        let measure = if num_steps == EPISODE_96 {
            Duration::from_secs(5)
        } else {
            Duration::from_secs(30)
        };
        group.sample_size(sample_size);
        group.warm_up_time(warm_up);
        group.measurement_time(measure);

        group.bench_function(label, |b| {
            b.iter_custom(|iters| {
                let before_alloc = alloc_snapshot();
                let start = std::time::Instant::now();
                for _ in 0..iters {
                    let mut dwelling = make_dwelling(dir, duration_s, time_res_s);
                    for _ in 0..num_steps {
                        dwelling.step().expect("step");
                        let t = dwelling.telemetry().unwrap();
                        black_box(
                            t.observation_vec(
                                NARROW_FIELDS,
                                &mut hares_core::ObservationWarnOnce::default(),
                            )
                            .expect("obs"),
                        );
                    }
                }
                let elapsed = start.elapsed();
                let after_alloc = alloc_snapshot();
                let allocs_per_episode = (after_alloc.saturating_sub(before_alloc)) / iters;
                eprintln!(
                    "rl_episode/episode_single/{label}: ~{allocs_per_episode} allocs/episode"
                );
                elapsed
            });
        });
    }

    group.finish();
}

/// Benchmark: full episode lifecycle for a vector of dwellings.
///
/// Each episode: create N dwellings → step all N times (in parallel)
/// → collect observations from all.  Measured at 16, 64, 256 dwellings at
/// both 96-step and 8 760-step lengths.
///
/// Note: `to_observation_vec` is called once per dwelling after all steps complete
/// (not per-step). This measures episode-end observation overhead rather than
/// per-step observation cost. Contrast with `bench_episode_single` where observation
/// is collected at every step. Both are valid measurements for different RL workflows.
fn bench_episode_vec(c: &mut Criterion, dir: &Path) {
    let mut group = c.benchmark_group("rl_episode/episode_vec");

    let configs: &[(usize, i64, i64, &str)] = &[
        (16, 900, 86_400, "16d_96s_15min"),
        (64, 900, 86_400, "64d_96s_15min"),
        (256, 900, 86_400, "256d_96s_15min"),
        (16, 3600, 31_536_000, "16d_8760s_1hr"),
        (64, 3600, 31_536_000, "64d_8760s_1hr"),
        (256, 3600, 31_536_000, "256d_8760s_1hr"),
    ];

    for &(num_dwellings, time_res_s, duration_s, label) in configs {
        let is_large = num_dwellings > 16 && duration_s > 100_000;
        group.throughput(Throughput::Elements(num_dwellings as u64));
        group.sample_size(10);
        group.warm_up_time(Duration::from_secs(if is_large { 10 } else { 3 }));
        group.measurement_time(Duration::from_secs(if is_large { 30 } else { 10 }));

        // Per-thread counting cannot see allocations on rayon worker
        // threads, so the allocations-per-episode figure comes from a
        // separate counting pass that steps the same dwellings sequentially
        // with `iter_mut()` on the bench thread, outside the timed loop.
        // The timed loop below stays parallel and uncounted.
        let num_steps: usize = if duration_s == 86_400 {
            EPISODE_96
        } else {
            EPISODE_8760
        };
        let allocs_per_episode = {
            let before_alloc = alloc_snapshot();
            let mut dwellings: Vec<Dwelling> = (0..num_dwellings)
                .map(|_| make_dwelling(dir, duration_s, time_res_s))
                .collect();
            for _ in 0..num_steps {
                dwellings.iter_mut().for_each(|dwelling| {
                    dwelling.step().expect("step");
                });
            }
            for dwelling in &dwellings {
                let t = dwelling.telemetry().unwrap();
                black_box(
                    t.observation_vec(
                        NARROW_FIELDS,
                        &mut hares_core::ObservationWarnOnce::default(),
                    )
                    .expect("obs"),
                );
            }
            alloc_snapshot().saturating_sub(before_alloc)
        };
        eprintln!(
            "rl_episode/episode_vec/{label}: ~{allocs_per_episode} allocs/episode (counted on the sequential pass outside the timed loop)"
        );

        group.bench_with_input(
            BenchmarkId::new("full_episode", label),
            &label,
            |b, _label| {
                b.iter_custom(|iters| {
                    let start = std::time::Instant::now();
                    for _ in 0..iters {
                        let mut dwellings: Vec<Dwelling> = (0..num_dwellings)
                            .map(|_| make_dwelling(dir, duration_s, time_res_s))
                            .collect();
                        for _ in 0..num_steps {
                            dwellings.par_iter_mut().for_each(|dwelling| {
                                dwelling.step().expect("step");
                            });
                        }
                        for dwelling in &dwellings {
                            let t = dwelling.telemetry().unwrap();
                            black_box(
                                t.observation_vec(
                                    NARROW_FIELDS,
                                    &mut hares_core::ObservationWarnOnce::default(),
                                )
                                .expect("obs"),
                            );
                        }
                    }
                    start.elapsed()
                });
            },
        );
    }

    group.finish();
}

/// Benchmark: batch-step pattern (parallel step + observation) directly in Rust.
///
/// Mirrors what Python `batch_step_py()` does: each dwelling is stepped and its
/// observation vector collected in one parallel pass.  Benchmarked for 16, 64,
/// and 256 dwellings at a single timestep to isolate the batching overhead.
fn bench_batch_step(c: &mut Criterion, dir: &Path) {
    let mut group = c.benchmark_group("rl_episode/batch_step");

    let time_res_s = 900;
    let duration_s = 86_400;

    for &num_dwellings in &[16usize, 64, 256] {
        group.throughput(Throughput::Elements(num_dwellings as u64));
        group.sample_size(20);
        group.warm_up_time(Duration::from_secs(1));
        group.measurement_time(Duration::from_secs(3));

        group.bench_with_input(
            BenchmarkId::new("step_and_observe", num_dwellings),
            &num_dwellings,
            |b, &num_dwellings| {
                b.iter_batched(
                    || {
                        (0..num_dwellings)
                            .map(|_| make_dwelling(dir, duration_s, time_res_s))
                            .collect::<Vec<_>>()
                    },
                    |mut dwellings| {
                        dwellings.par_iter_mut().for_each(|dwelling| {
                            black_box(dwelling.step().expect("step"));
                            let t = dwelling.telemetry().unwrap();
                            black_box(
                                t.observation_vec(
                                    NARROW_FIELDS,
                                    &mut hares_core::ObservationWarnOnce::default(),
                                )
                                .expect("obs"),
                            );
                        });
                    },
                    criterion::BatchSize::LargeInput,
                );
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_rl_episode);
criterion_main!(benches);
