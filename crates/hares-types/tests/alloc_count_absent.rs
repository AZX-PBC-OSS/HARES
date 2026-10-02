//! `thread_allocations` must read `None` in a binary that did not install
//! the counting allocator: "unmeasured", not a zero that reads as a
//! measurement.
//!
//! Separate from `alloc_count.rs` because that file's `#[global_allocator]`
//! applies to its whole binary, and this binary must have none.

use hares_types::alloc_count::thread_allocations;

#[test]
fn thread_allocations_is_none_without_the_allocator() {
    assert!(thread_allocations().is_none());
}
