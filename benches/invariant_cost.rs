//! The per-step equipment enforcement bench: what the enforcement block
//! costs per step. Per equipment per step the pipeline captures the
//! pre-step rollback ports and, on the step's success, runs the
//! core-output contract check and the port/core electrical consistency
//! check; the block is unconditional in every build profile (the checks
//! compile and run in release too, warm-up included). The bench steps the
//! two shared fixtures with the block on (the default, the untouched
//! path) and off (`set_step_validation_enabled`, the measurement
//! instrument whose witness runs before anything is timed): the paired
//! delta between the arms is the block's per-step cost, and the off arm
//! is the comparison the number is read against.
//!
//! The two rosters bound the cost's shape the way the checkpoint bench's
//! do: the synthetic TOML dwelling (one zone, one furnace, 60 s steps) is
//! the single-equipment floor, and the HPXML base.xml fixture at the
//! fleet shape (900 s steps, no own reports) is the eight-equipment
//! roster the fleet and single_building benches run, so the block runs
//! eight times per step. Both rosters run constant weather, so the
//! march's samples step the same steady state and the arms' medians
//! compare like for like. Each iteration builds its own dwelling (the
//! rl_step bench's construction shape, the setup outside the timed
//! region) and marches it from the run's first step, so every iteration
//! times the same stretch of the same fixture and no iteration meets the
//! run's end.

mod common;

use std::path::Path;
use std::time::Duration;

use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use hares_core::Dwelling;

/// The small roster's per-iteration build: one furnace, 60 s steps.
fn build_small(dir: &Path) -> Dwelling {
    common::invariant_cost_small_dwelling(dir)
}

/// The large roster's per-iteration build: eight equipment, 900 s steps.
fn build_large(dir: &Path) -> Dwelling {
    common::invariant_cost_large_dwelling(dir, 1)
}

fn bench_invariant_cost(c: &mut Criterion) {
    let temp = tempfile::tempdir().expect("benchmark directory");
    let dir = temp.path();

    // The instrument witness, once, outside the timed regions: the
    // enforcement block fails a violation's step in the default and
    // re-enabled states and runs it clean in the off state, for both
    // violation flavors. A bench whose comparison arm silently skipped
    // the block would measure nothing; the same assertions run in the
    // bench_construction suite at suite cost.
    common::assert_validation_witness(dir);

    let mut group = c.benchmark_group("invariant_cost");
    group.sample_size(10);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(2));

    // The arms alternate per case (on first, then off) so each pair
    // sits adjacent in the sitting: a drift between the two cases'
    // phases is seconds wide, while the drift between process launches
    // (the paired protocol's positional effect) is minutes wide. Each
    // iteration builds its own dwelling (the setup outside the timed
    // region) and marches it from the run's first step, so every
    // iteration times the same stretch of the same fixture and no
    // iteration meets the run's end.
    for (size, arm_steps, build) in [
        (
            "small",
            common::INVARIANT_COST_SMALL_ARM_STEPS,
            build_small as fn(&Path) -> Dwelling,
        ),
        (
            "large",
            common::INVARIANT_COST_LARGE_ARM_STEPS,
            build_large as fn(&Path) -> Dwelling,
        ),
    ] {
        for (arm, enabled) in [("on", true), ("off", false)] {
            group.bench_function(BenchmarkId::new(arm, size), |b| {
                b.iter_batched(
                    || {
                        let mut dwelling = build(dir);
                        dwelling.set_step_validation_enabled(enabled);
                        dwelling
                    },
                    |mut dwelling| {
                        for _ in 0..arm_steps {
                            dwelling.step().unwrap_or_else(|err| {
                                panic!("invariant_cost {arm}/{size}: the step runs: {err}")
                            });
                        }
                    },
                    BatchSize::SmallInput,
                );
            });
        }
    }

    group.finish();
}

criterion_group!(benches, bench_invariant_cost);
criterion_main!(benches);
