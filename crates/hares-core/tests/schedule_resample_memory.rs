//! The schedule resample's memory, counted by the I-08 allocation-accounting
//! counters.
//!
//! This test binary installs the workspace's shared counting allocator (a
//! separate binary because `#[global_allocator]` applies to the whole test
//! executable). The workload is the resample the fleet runs per dwelling: a
//! 15-minute, 16-column ResStock schedule CSV (35040 rows) taken to the
//! fleet's 1-minute step, then the per-dwelling copy the environment build
//! holds. The counters' byte channel pins the schedule table's net growth;
//! the value pin asserts the zero-order hold every representation must
//! preserve bitwise.

use std::path::PathBuf;

use chrono::DateTime;
use hares_io::{ColumnAggregation, ScheduleTimeSeries, parse_schedule_csv};
use hares_types::alloc_count::{CountingAllocator, thread_alloc_bytes, thread_allocations};

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

/// The 15-minute, 16-column ResStock schedule the fleet fixtures ship: the
/// shape the review's resample-memory finding cites.
fn resstock_schedule() -> ScheduleTimeSeries {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/resstock/2025.1/bldg0000006/in.schedules.csv");
    parse_schedule_csv(&path, &[], None, None).expect("the ResStock schedule CSV parses")
}

/// The net live bytes and the allocation calls the closure makes on this
/// thread.
fn measured<T>(f: impl FnOnce() -> T) -> (i64, u64, T) {
    let bytes_before = thread_alloc_bytes().expect("the counting allocator is installed");
    let calls_before = thread_allocations().expect("the counting allocator is installed");
    let out = f();
    (
        thread_alloc_bytes().expect("the counting allocator is installed") - bytes_before,
        thread_allocations().expect("the counting allocator is installed") - calls_before,
        out,
    )
}

/// The per-step values the environment's division indexing reads from the
/// resampled series: row `i / (the resampled rows' step / the sim step)` --
/// the zero-order hold of the source row the step falls in. The derivation is
/// the environment's own (`compute_schedule_idx`'s row read); asserting it
/// against the source table directly pins the value identity that holds
/// whatever representation the resampled series uses.
fn assert_zero_order_hold(
    source: &ScheduleTimeSeries,
    resampled: &ScheduleTimeSeries,
    sim_step_secs: u32,
) {
    let source_factor = source.source_step_secs / sim_step_secs;
    let read_factor = (resampled.source_step_secs / sim_step_secs).max(1);
    let per_step_rows = resampled.timestamps.len() * read_factor as usize;
    assert_eq!(
        per_step_rows,
        source.timestamps.len() * source_factor as usize,
        "the resampled series must span the source's per-step row count"
    );
    for (col_idx, column) in resampled.columns.iter().enumerate() {
        for step in 0..per_step_rows {
            let read = column[step / read_factor as usize];
            let expected = source.columns[col_idx][step / source_factor as usize];
            assert_eq!(
                read.to_bits(),
                expected.to_bits(),
                "column {col_idx} step {step}: the resampled series must be the \
                 source's zero-order hold, bitwise"
            );
        }
    }
}

#[test]
fn resample_holds_the_source_resolution_not_an_expanded_copy() {
    let (parse_bytes, parse_calls, source) = measured(resstock_schedule);
    assert_eq!(source.source_step_secs, 900);
    assert_eq!(source.timestamps.len(), 35_040);
    assert_eq!(source.columns.len(), 16);
    println!("parse: {parse_bytes} bytes, {parse_calls} calls");

    let (resample_bytes, resample_calls, resampled) = measured(|| source.resample(60).unwrap());
    println!(
        "resample 900s -> 60s: {resample_bytes} bytes, {resample_calls} calls, \
         {} rows x {} columns, per-row timestamp {} bytes",
        resampled.timestamps.len(),
        resampled.columns.len(),
        std::mem::size_of::<DateTime<chrono::FixedOffset>>(),
    );

    // The fleet's per-dwelling cost: the resample plus the copy each
    // dwelling's environment build holds (the blueprint's series is cloned
    // per dwelling because the unavailable-period zeroing and the schedule
    // injection write the build's own copy).
    let (clone_bytes, clone_calls, clones) = measured(|| {
        let clones: Vec<ScheduleTimeSeries> =
            (0..10).map(|_| resampled.clone()).collect::<Vec<_>>();
        clones
    });
    let per_dwelling_bytes = (resample_bytes + clone_bytes) / 10;
    println!(
        "10 per-dwelling copies: {clone_bytes} bytes, {clone_calls} calls; \
         per dwelling (resample + one copy): {per_dwelling_bytes} bytes; \
         fleet scale (10 dwellings): {} bytes",
        resample_bytes + clone_bytes,
    );
    drop(clones);

    // The held source rows cost about a fifteenth of the expanded copy's
    // columns plus its per-row timestamps; the pin allows the held table
    // twice its expected size so the counter's own overhead cannot flake.
    let source_table_bytes = (source.timestamps.len() * source.columns.len() * 8
        + source.timestamps.len() * std::mem::size_of::<DateTime<chrono::FixedOffset>>())
        as i64;
    assert!(
        per_dwelling_bytes < 2 * source_table_bytes,
        "the schedule table costs {per_dwelling_bytes} bytes per dwelling at the \
         1-minute step; holding the source-resolution rows costs about \
         {source_table_bytes} bytes -- the resample must not materialise an \
         upsampled copy per dwelling"
    );

    assert_zero_order_hold(&source, &resampled, 60);
}

#[test]
fn a_stamp_unaligned_to_its_own_step_still_expands() {
    // An hourly schedule stamped on the half hour: a row's sub-steps spill
    // into the next civil hour, and the unavailable-period flags read
    // hour-of-day per row, so the compact hold cannot represent the
    // per-sub-step zeroing the periods apply. The resample falls back to the
    // materialised expansion -- the pre-change behaviour, exactly.
    let start = DateTime::parse_from_rfc3339("2021-01-01T00:30:00-07:00")
        .expect("a valid offset timestamp");
    let source = ScheduleTimeSeries {
        timestamps: (0..48)
            .map(|i| start + chrono::Duration::hours(i as i64))
            .collect(),
        column_names: vec!["occupants".to_string()],
        columns: vec![(0..48).map(|i| f64::from(i) / 48.0).collect()],
        column_index: [("occupants".to_string(), 0_usize)].into_iter().collect(),
        source_step_secs: 3600,
        column_aggregations: vec![ColumnAggregation::Mean],
    };

    let resampled = source.resample(900).expect("upsample should work");
    // The expansion materialised: one row per 15-minute step, each source
    // value repeated four times.
    assert_eq!(resampled.timestamps.len(), 48 * 4);
    assert_eq!(resampled.source_step_secs, 900);
    assert_zero_order_hold(&source, &resampled, 900);
}
