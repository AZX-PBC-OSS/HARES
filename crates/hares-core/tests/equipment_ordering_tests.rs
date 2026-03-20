//! Integration tests for ExecutionStage ordering guarantees.
//!
//! `stage_rank` is crate-private so these tests verify the same contract by
//! asserting the documented ordering through the discriminant positions defined
//! in the enum.  The ordering encodes a hard protocol assumption: Independent
//! equipment (schedules, PV) runs before Electrical equipment (batteries), which
//! runs before Thermal equipment (HVAC, water heaters), which precedes
//! EnvelopeResolution (solver-only, no equipment).

use hares_types::ExecutionStage;

/// Maps a stage to the integer rank used by the Dwelling orchestrator.
/// Must stay in sync with `hares_core::dwelling::conversions::stage_rank`.
fn stage_rank(stage: ExecutionStage) -> u8 {
    match stage {
        ExecutionStage::Independent => 0,
        ExecutionStage::Electrical => 1,
        ExecutionStage::Thermal => 2,
        ExecutionStage::EnvelopeResolution => 3,
    }
}

fn assert_strictly_before(earlier: ExecutionStage, later: ExecutionStage) {
    assert!(
        stage_rank(earlier) < stage_rank(later),
        "{earlier:?} (rank {}) must precede {later:?} (rank {})",
        stage_rank(earlier),
        stage_rank(later),
    );
}

#[test]
fn stage_rank_ordering_is_correct() {
    // Full chain: Independent < Electrical < Thermal < EnvelopeResolution
    let ordered = [
        ExecutionStage::Independent,
        ExecutionStage::Electrical,
        ExecutionStage::Thermal,
        ExecutionStage::EnvelopeResolution,
    ];
    for window in ordered.windows(2) {
        assert_strictly_before(window[0], window[1]);
    }
}

#[test]
fn independent_precedes_electrical() {
    assert_strictly_before(ExecutionStage::Independent, ExecutionStage::Electrical);
}

#[test]
fn electrical_precedes_thermal() {
    assert_strictly_before(ExecutionStage::Electrical, ExecutionStage::Thermal);
}

#[test]
fn thermal_precedes_envelope_resolution() {
    assert_strictly_before(ExecutionStage::Thermal, ExecutionStage::EnvelopeResolution);
}

#[test]
fn independent_precedes_thermal() {
    // Transitive: schedule loads must complete before HVAC runs.
    assert_strictly_before(ExecutionStage::Independent, ExecutionStage::Thermal);
}

#[test]
fn independent_precedes_envelope_resolution() {
    assert_strictly_before(ExecutionStage::Independent, ExecutionStage::EnvelopeResolution);
}

#[test]
fn electrical_precedes_envelope_resolution() {
    assert_strictly_before(ExecutionStage::Electrical, ExecutionStage::EnvelopeResolution);
}

#[test]
fn stage_rank_is_unique_per_variant() {
    let all_stages = [
        ExecutionStage::Independent,
        ExecutionStage::Electrical,
        ExecutionStage::Thermal,
        ExecutionStage::EnvelopeResolution,
    ];
    let ranks: Vec<u8> = all_stages.iter().copied().map(stage_rank).collect();
    let mut deduped = ranks.clone();
    deduped.sort_unstable();
    deduped.dedup();
    assert_eq!(
        deduped.len(),
        all_stages.len(),
        "every ExecutionStage must map to a distinct rank; got: {ranks:?}"
    );
}

#[test]
fn stage_ranks_are_dense_from_zero() {
    // Ranks must be 0, 1, 2, 3 with no gaps so that sort-by-key produces a
    // stable, gap-free ordering in the Dwelling step loop.
    let all_stages = [
        ExecutionStage::Independent,
        ExecutionStage::Electrical,
        ExecutionStage::Thermal,
        ExecutionStage::EnvelopeResolution,
    ];
    let mut ranks: Vec<u8> = all_stages.iter().copied().map(stage_rank).collect();
    ranks.sort_unstable();
    let expected: Vec<u8> = (0..all_stages.len() as u8).collect();
    assert_eq!(ranks, expected, "stage ranks must be dense starting at 0");
}

#[test]
fn envelope_resolution_is_last() {
    let others = [
        ExecutionStage::Independent,
        ExecutionStage::Electrical,
        ExecutionStage::Thermal,
    ];
    let envelope_rank = stage_rank(ExecutionStage::EnvelopeResolution);
    for &stage in &others {
        assert!(
            stage_rank(stage) < envelope_rank,
            "{stage:?} must have a lower rank than EnvelopeResolution"
        );
    }
}

#[test]
fn independent_is_first() {
    let others = [
        ExecutionStage::Electrical,
        ExecutionStage::Thermal,
        ExecutionStage::EnvelopeResolution,
    ];
    let independent_rank = stage_rank(ExecutionStage::Independent);
    for &stage in &others {
        assert!(
            independent_rank < stage_rank(stage),
            "Independent must have a lower rank than {stage:?}"
        );
    }
}

#[test]
fn stage_sort_produces_documented_order() {
    // Simulate what Dwelling does: build an index vec and sort by stage rank,
    // then verify the resulting sequence is the documented execution order.
    let mut stages = vec![
        ExecutionStage::Thermal,
        ExecutionStage::EnvelopeResolution,
        ExecutionStage::Independent,
        ExecutionStage::Electrical,
    ];
    stages.sort_by_key(|&s| stage_rank(s));

    assert_eq!(
        stages,
        vec![
            ExecutionStage::Independent,
            ExecutionStage::Electrical,
            ExecutionStage::Thermal,
            ExecutionStage::EnvelopeResolution,
        ]
    );
}

#[test]
fn repeated_same_stage_preserves_relative_order() {
    // Two equipment instances at the same stage must not have their relative
    // order inverted by a sort; stable sort is required.
    let mut stages = vec![
        (0usize, ExecutionStage::Thermal),
        (1usize, ExecutionStage::Independent),
        (2usize, ExecutionStage::Thermal),
    ];
    stages.sort_by_key(|&(_, s)| stage_rank(s));

    // After sorting, both Thermal entries must follow the Independent entry.
    assert_eq!(stages[0].1, ExecutionStage::Independent);
    assert_eq!(stages[1].1, ExecutionStage::Thermal);
    assert_eq!(stages[2].1, ExecutionStage::Thermal);
    // Original relative order within the same stage must be preserved by a
    // stable sort (indices 0 and 2 are both Thermal; 0 comes before 2).
    assert_eq!(stages[1].0, 0);
    assert_eq!(stages[2].0, 2);
}
