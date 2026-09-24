mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use hares_core::Dwelling;
use rayon::prelude::*;

// ---------------------------------------------------------------------------
// Allocation tracking
// ---------------------------------------------------------------------------
// Benchmarks instrument allocations via a global allocator wrapper.
// Snapshot before / after each iter_custom measurement to compute
// per-episode allocation counts reported alongside wall-clock timing.

#[global_allocator]
static GLOBAL: TrackingAllocator = TrackingAllocator;

static ALLOC_COUNT: AtomicU64 = AtomicU64::new(0);

struct TrackingAllocator;

unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

fn alloc_snapshot() -> u64 {
    ALLOC_COUNT.load(Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// Observation field sets
// ---------------------------------------------------------------------------
// Synthetic dwelling (furnace + electricity) produces zone "Indoor" and
// equipment "Electric Furnace" (canonical HPXML heating name).

const NARROW_FIELDS: &[&str] = &[
    "outdoor_temp",
    "zone_temp[Indoor]",
    "total_power_kw",
    "equipment_power[Electric Furnace]",
    "setpoint_heat[Indoor]",
];

fn wide_fields() -> Vec<&'static str> {
    let base: &[&str] = &[
        "outdoor_temp",
        "outdoor_humidity_ratio",
        "total_power_kw",
        "reactive_power_kvar",
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

fn synthetic_toml(duration_s: i64, time_res_s: i64) -> String {
    format!(
        r#"building_id = 1

[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = {time_res_s}
duration_s = {duration_s}

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0
wall_area_m2 = 145.0

[materials]
wall_r_value_m2_k_w = 2.0

[hvac]
equipment_name = "Furnace"
fuel = "electricity"
heating_capacity_kbtu_h = 25.0

[weather]
outdoor_temp_c = 8.0
dew_point_c = 4.0
rel_humidity_pct = 55.0
pressure_kpa = 101.325

[schedule]
occupancy = 1.0

[output]
output_verbosity = 0
output_format = "csv"
output_chunk_size = 1000
master_seed = 0
write_output = false
"#
    )
}

fn make_dwelling(duration_s: i64, time_res_s: i64) -> Dwelling {
    let path = common::unique_temp_path("hares-bench-rl-episode", "toml");
    std::fs::write(&path, synthetic_toml(duration_s, time_res_s)).expect("write TOML");
    Dwelling::from_toml_config_with_write_output(&path, Some(false)).expect("create dwelling")
}

// ---------------------------------------------------------------------------
// Benchmarks
// ---------------------------------------------------------------------------

fn bench_rl_episode(c: &mut Criterion) {
    bench_reset(c);
    bench_observation_vec(c);
    bench_episode_single(c);
    bench_episode_vec(c);
    bench_batch_step(c);
}

/// Benchmark: Dwelling construction (the "reset" path).
fn bench_reset(c: &mut Criterion) {
    let mut group = c.benchmark_group("rl_episode/reset");
    group.sample_size(30);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(3));

    group.bench_function("from_toml_config", |b| {
        b.iter_batched(
            || common::unique_temp_path("hares-bench-rl-reset", "toml"),
            |path| {
                std::fs::write(&path, synthetic_toml(86_400, 60)).expect("write TOML");
                black_box(
                    Dwelling::from_toml_config_with_write_output(&path, Some(false))
                        .expect("create dwelling"),
                );
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.finish();
}

/// Benchmark: `to_observation_vec` with narrow (5) and wide (55) field sets.
fn bench_observation_vec(c: &mut Criterion) {
    let mut group = c.benchmark_group("rl_episode/observation_vec");
    group.sample_size(40);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(2));
    group.throughput(Throughput::Elements(1));

    let mut dwelling = make_dwelling(86_400, 60);
    dwelling.step().expect("step");
    let telemetry = dwelling.telemetry();

    group.bench_function("narrow_5_fields", |b| {
        b.iter(|| black_box(telemetry.to_observation_vec(NARROW_FIELDS).expect("obs")));
    });

    let wide = wide_fields();
    group.bench_function("wide_55_fields", |b| {
        b.iter(|| black_box(telemetry.to_observation_vec(&wide).expect("obs")));
    });

    group.finish();
}

/// Benchmark: full episode lifecycle for a single dwelling.
///
/// Each episode: create dwelling → step N times (with telemetry + observation
/// after each step).  Measured at 96 steps (1 day / 15-min) and 8 760 steps
/// (1 year / 1-hour).
fn bench_episode_single(c: &mut Criterion) {
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
                    let mut dwelling = make_dwelling(duration_s, time_res_s);
                    for _ in 0..num_steps {
                        dwelling.step().expect("step");
                        let t = dwelling.telemetry();
                        black_box(t.to_observation_vec(NARROW_FIELDS).expect("obs"));
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
fn bench_episode_vec(c: &mut Criterion) {
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

        group.bench_with_input(
            BenchmarkId::new("full_episode", label),
            &label,
            |b, _label| {
                b.iter_custom(|iters| {
                    let before_alloc = alloc_snapshot();
                    let start = std::time::Instant::now();
                    let num_steps: usize = if duration_s == 86_400 {
                        EPISODE_96
                    } else {
                        EPISODE_8760
                    };
                    for _ in 0..iters {
                        let mut dwellings: Vec<Dwelling> = (0..num_dwellings)
                            .map(|_| make_dwelling(duration_s, time_res_s))
                            .collect();
                        for _ in 0..num_steps {
                            dwellings.par_iter_mut().for_each(|dwelling| {
                                dwelling.step().expect("step");
                            });
                        }
                        for dwelling in &dwellings {
                            let t = dwelling.telemetry();
                            black_box(t.to_observation_vec(NARROW_FIELDS).expect("obs"));
                        }
                    }
                    let elapsed = start.elapsed();
                    let after_alloc = alloc_snapshot();
                    let allocs_per_episode = (after_alloc.saturating_sub(before_alloc)) / iters;
                    eprintln!(
                        "rl_episode/episode_vec/{label}: ~{allocs_per_episode} allocs/episode"
                    );
                    elapsed
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
fn bench_batch_step(c: &mut Criterion) {
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
                            .map(|_| make_dwelling(duration_s, time_res_s))
                            .collect::<Vec<_>>()
                    },
                    |mut dwellings| {
                        dwellings.par_iter_mut().for_each(|dwelling| {
                            black_box(dwelling.step().expect("step"));
                            let t = dwelling.telemetry();
                            black_box(t.to_observation_vec(NARROW_FIELDS).expect("obs"));
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
