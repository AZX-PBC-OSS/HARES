mod common;

use std::num::NonZeroUsize;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use hares_fleet::{Fleet, FleetReport};
use hares_io::{Frequency, Selection};

fn fold_report(frequency: Frequency) -> FleetReport {
    FleetReport {
        selection: Selection::Verbosity(0),
        frequency,
        health_policy: hares_io::HealthPolicy::default(),
    }
}

fn chunk(size: usize) -> NonZeroUsize {
    NonZeroUsize::new(size).expect("nonzero chunk size")
}

/// The peak resident set, read once after the timed region: the kernel's
/// own high-water mark for the process (`VmHWM`, `/proc/self/status`),
/// which `getrusage`'s max RSS equals on Linux.
fn peak_rss_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            let value = rest.trim().trim_end_matches(" kB").trim();
            return value.parse().ok();
        }
    }
    None
}

/// Whether the one-shot mode is requested: `HARES_FLEET_FOLD_ONCE` set to
/// `<chunk_size>/<homes>`.
fn one_shot_requested() -> bool {
    std::env::var("HARES_FLEET_FOLD_ONCE").is_ok()
}

/// The one-shot measurement mode (`HARES_FLEET_FOLD_ONCE` =
/// `<chunk_size>/<homes>`): one fleet run per fresh process, so the
/// process's high-water mark is that one workload's own, never another
/// benchmarking's inherited peak. Prints the folded home count, the run's
/// wall seconds and the `VmHWM` read from `/proc/self/status` at run end.
fn run_fleet_fold_once() {
    let request = std::env::var("HARES_FLEET_FOLD_ONCE").expect("checked above");
    let (chunk_size, homes) = request.split_once('/').expect("<chunk>/<homes>");
    let chunk_size: usize = chunk_size.parse().expect("chunk size parses");
    let homes: usize = homes.parse().expect("home count parses");

    let temp = tempfile::tempdir().expect("benchmark directory");
    let dir = temp.path();
    let schedule_path = common::numbered_path(dir, "hares-bench-fold-schedule", "csv");
    let weather_path = common::numbered_path(dir, "hares-bench-fold-weather", "epw");
    common::write_schedule_csv(&schedule_path);
    common::write_weather_epw(&weather_path);

    let configs = (0..homes)
        .map(|idx| {
            let mut config = common::build_dwelling_config(
                dir,
                idx as i64 + 1,
                schedule_path.clone(),
                weather_path.clone(),
                Some(common::repo_defaults_path()),
                chrono::Duration::hours(24),
            );
            // The production fleet shape: the homes write no own files, so
            // the fleet report is the only report and the fold reads only
            // it.
            config.settings.sim_config.reports.clear();
            config.settings.sim_config.time_res = chrono::Duration::seconds(900);
            config
        })
        .collect::<Vec<_>>();
    let fleet = Fleet::from_buildings(configs).expect("no reserved report or shared destination");
    let started = std::time::Instant::now();
    let results = fleet.simulate(&fold_report(Frequency::Hourly), 8, chunk(chunk_size));
    let wall = started.elapsed();
    std::hint::black_box(&results);
    let folded = results.as_ref().map_or(0, |r| r.homes.len());
    let rss = match peak_rss_kb() {
        Some(kb) => format!("{kb} kB"),
        None => "unavailable".to_string(),
    };
    println!(
        "fleet_fold_once: chunk {chunk_size}, {homes} homes: {folded} folded in {:.3} s, VmHWM {rss}",
        wall.as_secs_f64()
    );
}

fn bench_fleet(c: &mut Criterion) {
    if one_shot_requested() {
        return;
    }
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
                        .expect("no reserved report or shared destination")
                },
                |fleet| {
                    let results = fleet.simulate(&fold_report(Frequency::Hourly), 4, chunk(64));
                    std::hint::black_box(&results);
                },
                criterion::BatchSize::LargeInput,
            );
        });
    }

    group.finish();
}

/// The streaming fold's memory and wall shape: copies of the base
/// fixture at 24 h and 900 s steps, folded at the hour, one group per
/// home count. Run one benchmark (one chunk size at one fleet size) per
/// fresh process, so no other chunking's footprint precedes the reading:
/// `cargo bench -p hares-core --bench fleet -- fleet_fold/chunk_64/10000`
/// and its siblings. The group prints the process's peak RSS (VmHWM)
/// after each run: the fold's high-water mark at that fleet size and
/// chunking. The chunk size rides the benchmark id (`chunk_64`, the
/// Python default, against `chunk_1000`, the one-chunk shape the parent
/// commit's collect ran), so the same fleet's two chunkings measure what
/// each chunk's wait for its slowest home costs.
fn bench_fleet_fold(c: &mut Criterion) {
    if one_shot_requested() {
        run_fleet_fold_once();
        return;
    }
    let temp = tempfile::tempdir().expect("benchmark directory");
    let dir = temp.path();
    let schedule_path = common::numbered_path(dir, "hares-bench-fold-schedule", "csv");
    let weather_path = common::numbered_path(dir, "hares-bench-fold-weather", "epw");
    common::write_schedule_csv(&schedule_path);
    common::write_weather_epw(&weather_path);

    // The home counts the measurement log records; keep the largest
    // fleet in its own process (its RSS is the slope's point).
    let mut group = c.benchmark_group("fleet_fold");
    group.sample_size(10);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(5));

    for n in [100usize, 1000usize, 10_000usize] {
        for (chunk_label, chunk_size) in [("chunk_64", 64usize), ("chunk_1000", 1000usize)] {
            group.bench_with_input(
                BenchmarkId::new(chunk_label, n),
                &n,
                |b, &n| {
                    b.iter_batched(
                        || {
                            let configs = (0..n)
                                .map(|idx| {
                                    let mut config = common::build_dwelling_config(
                                        dir,
                                        idx as i64 + 1,
                                        schedule_path.clone(),
                                        weather_path.clone(),
                                        Some(common::repo_defaults_path()),
                                        chrono::Duration::hours(24),
                                    );
                                    // The production fleet shape: the homes write
                                    // no own files, so the fleet report is the
                                    // only report and the fold reads only it.
                                    config.settings.sim_config.reports.clear();
                                    config.settings.sim_config.time_res = chrono::Duration::seconds(900);
                                    config
                                })
                                .collect::<Vec<_>>();
                            Fleet::from_buildings(configs)
                                .expect("no reserved report or shared destination")
                        },
                        |fleet| {
                            let started = std::time::Instant::now();
                            let results =
                                fleet.simulate(&fold_report(Frequency::Hourly), 8, chunk(chunk_size));
                            let wall = started.elapsed();
                            std::hint::black_box(&results);
                            let homes = results.map_or(0, |r| r.homes.len());
                            let rss = match peak_rss_kb() {
                                Some(kb) => format!("{kb} kB"),
                                None => "unavailable".to_string(),
                            };
                            eprintln!(
                                "fleet_fold/{chunk_label}/{n}: {homes} homes folded in {wall:.1?}, peak RSS (VmHWM) {rss}"
                            );
                        },
                        criterion::BatchSize::LargeInput,
                    );
                },
            );
        }
    }

    group.finish();
}

criterion_group!(benches, bench_fleet, bench_fleet_fold);
criterion_main!(benches);
