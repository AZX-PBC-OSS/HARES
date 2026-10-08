//! Tests for the per-thread counting allocator, in a binary that installs it.
//!
//! This is a separate test binary because `#[global_allocator]` applies to
//! the whole binary: `alloc_count_absent.rs` must run without it to show the
//! `None` side of `thread_allocations`.

use std::hint::black_box;
use std::sync::Arc;
use std::sync::Barrier;
use std::thread;

use hares_types::alloc_count::{CountingAllocator, thread_allocations};

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

/// Between two reads on one thread, 7 `Box::new` allocations give a delta of
/// exactly 7: every allocating call counts once and deallocation does not
/// decrement. Then a spawned thread and the test thread meet at a barrier;
/// the spawned thread makes 5 allocations between its own two reads while
/// the test thread makes 1 000, and the spawned thread's delta is exactly 5:
/// a process-global counter would see the test thread's allocations in it.
#[test]
fn thread_allocations_counts_exactly_this_threads_allocations() {
    // The flag is set on the allocator's first use; this priming allocation
    // on this thread makes both reads below `Some` regardless of what the
    // test harness has done so far.
    black_box(Box::new(0u8));

    let before = thread_allocations().expect("the test installs the counting allocator");
    for n in 0..7 {
        black_box(Box::new(black_box(n)));
    }
    let after = thread_allocations().expect("the test installs the counting allocator");
    assert_eq!(after - before, 7);

    let barrier = Arc::new(Barrier::new(2));
    let spawned = {
        let barrier = Arc::clone(&barrier);
        thread::spawn(move || {
            barrier.wait();
            let before = thread_allocations().expect("the test installs the counting allocator");
            for n in 0..5 {
                black_box(Box::new(black_box(n)));
            }
            let after = thread_allocations().expect("the test installs the counting allocator");
            after - before
        })
    };

    barrier.wait();
    let main_before = thread_allocations().expect("the test installs the counting allocator");
    for n in 0..1000 {
        black_box(Box::new(black_box(n)));
    }
    let main_after = thread_allocations().expect("the test installs the counting allocator");

    assert_eq!(
        spawned.join().expect("spawned thread must not panic"),
        5,
        "the spawned thread's delta must be exactly its own 5 allocations, \
         not the test thread's too"
    );
    assert_eq!(main_after - main_before, 1000);
}
