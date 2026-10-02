//! The tool installs the workspace's shared counting allocator only under
//! `-F profiling`, in the library, so every binary linking it (the tool's
//! binary and this test binary) counts, while the plain build keeps the
//! system allocator untouched.

#[cfg(feature = "profiling")]
#[test]
fn frame_golden_installs_the_allocator_only_under_profiling() {
    // Touching the library keeps it linked into this test binary; the
    // `#[global_allocator]` in the lib is what installs the counter.
    let _ = frame_golden::running_features();
    assert!(
        hares_types::alloc_count::thread_allocations().is_some(),
        "the -F profiling build must install the counting allocator"
    );
}

#[cfg(not(feature = "profiling"))]
#[test]
fn frame_golden_installs_the_allocator_only_under_profiling() {
    let _ = frame_golden::running_features();
    assert!(
        hares_types::alloc_count::thread_allocations().is_none(),
        "the plain build must keep the system allocator"
    );
}
