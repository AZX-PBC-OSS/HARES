//! The checkpoint save/restore bench: the pinned file-path contract that
//! B5's chunk resume pays. Two roster sizes bound the cost's shape: the
//! synthetic TOML dwelling (one zone, one electric furnace) is the
//! checkpoint floor, and the HPXML base.xml fixture at the fleet shape
//! (900 s steps, no own reports) is the eight-equipment roster the fleet
//! and single_building benches run, four of them RNG-carrying event
//! appliances, so the saved snapshot carries eight equipment blobs and
//! advanced RNG streams.
//!
//! The dwellings are built and prefix-stepped (24 simulated hours) outside
//! the timed regions, and the bench's own fixture handling is validated
//! once per roster size before anything is timed: the saved state restores
//! into a fresh dwelling and re-saves the same checkpoint, and the
//! restored run's continuation holds the continuous run's state
//! (`benches/common.rs::assert_checkpoint_round_trip`; the same assertions
//! run in the bench_construction suite at suite cost).
//!
//! The timed regions keep the dwelling drops out of the measurement (the
//! row-39 record's drop lesson: a large dwelling's drop is allocator work
//! the checkpoint's cost must not carry), and the save's returned
//! checkpoint's drop stays out the same way (`iter_with_large_drop`
//! defers it past the timing; the record's "save times save_checkpoint
//! plus the file write" stays literal). Save borrows one dwelling and
//! times `save_checkpoint` plus the file write (the serde_json
//! serialization, the SHA-256 checksum, the atomic rename) to one path,
//! rewritten each iteration; restore times the file read, the checksum
//! verify, the gated parse and `load_checkpoint` into a long-lived target
//! the restore leaves in the checkpoint's own state (re-restoring the same
//! checkpoint is the same work every iteration).

mod common;

use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use hares_core::DwellingCheckpoint;

fn bench_checkpoint(c: &mut Criterion) {
    let temp = tempfile::tempdir().expect("benchmark directory");
    let dir = temp.path();
    let mut group = c.benchmark_group("checkpoint");
    group.sample_size(10);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(2));

    // The setup assertions, once per roster size, outside the timed
    // regions: the bench fails loudly rather than timing a path that no
    // longer round-trips.
    common::assert_checkpoint_round_trip(
        dir,
        || common::checkpoint_small_dwelling(dir),
        common::CHECKPOINT_SMALL_PREFIX_STEPS,
        common::CHECKPOINT_SMALL_CONTINUATION_STEPS,
        "the checkpoint bench's small roster",
    );
    let schedule_path = common::numbered_path(dir, "hares-bench-checkpoint-schedule", "csv");
    let weather_path = common::numbered_path(dir, "hares-bench-checkpoint-weather", "epw");
    common::write_schedule_csv(&schedule_path);
    common::write_weather_epw(&weather_path);
    common::assert_checkpoint_round_trip(
        dir,
        || common::checkpoint_large_dwelling(dir, 1, &schedule_path, &weather_path),
        common::CHECKPOINT_LARGE_PREFIX_STEPS,
        common::CHECKPOINT_LARGE_CONTINUATION_STEPS,
        "the checkpoint bench's large roster",
    );

    // save: the prefix-stepped dwelling snapshots and the checkpoint file
    // writes, per iteration, to one path the atomic rename replaces.
    {
        let mut dwelling = common::checkpoint_small_dwelling(dir);
        common::step_n(
            &mut dwelling,
            common::CHECKPOINT_SMALL_PREFIX_STEPS,
            "save/small: the prefix",
        );
        let path = common::numbered_path(dir, "hares-bench-checkpoint-save-small", "json");
        group.bench_function(BenchmarkId::new("save", "small"), |b| {
            b.iter_with_large_drop(|| {
                let checkpoint = dwelling
                    .save_checkpoint()
                    .expect("save/small: the state saves");
                checkpoint.save(&path).expect("save/small: the file writes");
                checkpoint
            })
        });
    }
    {
        let mut dwelling = common::checkpoint_large_dwelling(dir, 1, &schedule_path, &weather_path);
        common::step_n(
            &mut dwelling,
            common::CHECKPOINT_LARGE_PREFIX_STEPS,
            "save/large: the prefix",
        );
        let path = common::numbered_path(dir, "hares-bench-checkpoint-save-large", "json");
        group.bench_function(BenchmarkId::new("save", "large"), |b| {
            b.iter_with_large_drop(|| {
                let checkpoint = dwelling
                    .save_checkpoint()
                    .expect("save/large: the state saves");
                checkpoint.save(&path).expect("save/large: the file writes");
                checkpoint
            })
        });
    }

    // restore: the checkpoint file reads back (checksum verify, gated
    // parse) and restores into a fresh dwelling, repeatedly, the target
    // left in the checkpoint's own state each time.
    for (size, prefix_steps) in [
        ("small", common::CHECKPOINT_SMALL_PREFIX_STEPS),
        ("large", common::CHECKPOINT_LARGE_PREFIX_STEPS),
    ] {
        let source = match size {
            "small" => {
                let mut dwelling = common::checkpoint_small_dwelling(dir);
                common::step_n(&mut dwelling, prefix_steps, "restore/small: the prefix");
                dwelling
            }
            _ => {
                let mut dwelling =
                    common::checkpoint_large_dwelling(dir, 1, &schedule_path, &weather_path);
                common::step_n(&mut dwelling, prefix_steps, "restore/large: the prefix");
                dwelling
            }
        };
        let path = common::numbered_path(dir, "hares-bench-checkpoint-restore", "json");
        source
            .save_checkpoint()
            .expect("restore: the prefix state saves")
            .save(&path)
            .expect("restore: the file writes");
        // The page cache is warm and the file is valid before the timed
        // region: the first timed read measures the restore's own work.
        let first = DwellingCheckpoint::load(&path).expect("restore: the file loads");
        let mut target = match size {
            "small" => common::checkpoint_small_dwelling(dir),
            _ => common::checkpoint_large_dwelling(dir, 1, &schedule_path, &weather_path),
        };
        target
            .load_checkpoint(first)
            .expect("restore: the checkpoint restores");
        group.bench_function(BenchmarkId::new("restore", size), |b| {
            b.iter(|| {
                let checkpoint = DwellingCheckpoint::load(&path).expect("restore: the file loads");
                target
                    .load_checkpoint(checkpoint)
                    .expect("restore: the checkpoint restores");
            })
        });
    }

    group.finish();
}

criterion_group!(benches, bench_checkpoint);
criterion_main!(benches);
