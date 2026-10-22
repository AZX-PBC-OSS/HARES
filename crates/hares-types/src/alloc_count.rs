//! A per-thread allocation counter shared across the workspace's binaries.
//!
//! [`CountingAllocator`] wraps [`std::alloc::System`] and counts, per thread,
//! every allocating call (`alloc`, `alloc_zeroed`, `realloc`); deallocation
//! does not decrement, so the count is the number of allocations, not net
//! memory. The same allocator also tracks the thread's live requested bytes
//! ([`thread_alloc_bytes`]), where deallocation does decrement. A binary that
//! wants counts installs it with `#[global_allocator]`;
//! a library only defines the type. [`thread_allocations`] reads the calling
//! thread's count, or `None` when no binary installed the allocator, so
//! profiles can report "unmeasured" instead of a zero that reads as a
//! measurement.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};

/// A [`GlobalAlloc`] counting each allocating call on the thread that made
/// it, delegating the allocation itself to [`std::alloc::System`].
///
/// Install it in a binary with `#[global_allocator] static GLOBAL:
/// CountingAllocator = CountingAllocator;` and never in a library, where it
/// would silently apply to every downstream binary.
pub struct CountingAllocator;

/// Set the first time the allocator performs an allocating call in this
/// process. `Relaxed` is enough: the flag only distinguishes "installed"
/// from "never installed", and any thread that allocated has already made
/// the set visible to itself.
static INSTALLED: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// The calling thread's allocation count. Const-initialised so that
    /// accessing it from inside the allocator never allocates or runs a
    /// lazy initializer: a lazy `thread_local!` here would recurse into
    /// the allocator it sits in.
    static THREAD_ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
    /// The calling thread's live requested bytes: the net of every
    /// allocating call's size and every deallocation's size, so a delta
    /// between two reads is the measured region's net growth on the
    /// thread. A separate metric from the allocation count above, whose
    /// deallocations do not decrement.
    static THREAD_ALLOC_BYTES: Cell<i64> = const { Cell::new(0) };
}

/// Records one allocating call on the current thread and marks the
/// allocator installed. A thread in teardown has no thread-local storage;
/// `try_with` skips the count there instead of panicking inside the
/// allocator. `bytes` is the call's net effect on the thread's live
/// requested bytes.
fn count_allocation(bytes: i64) {
    if !INSTALLED.load(Ordering::Relaxed) {
        INSTALLED.store(true, Ordering::Relaxed);
    }
    let _ = THREAD_ALLOCATIONS.try_with(|count| count.set(count.get().wrapping_add(1)));
    record_bytes(bytes);
}

/// Records the byte effect of a deallocation: the live bytes decrement,
/// and the allocation count does not move (it counts allocating calls
/// only).
fn count_deallocation(size: usize) {
    record_bytes(-(size as i64));
}

/// The thread's live requested bytes move by `bytes`, wrapping.
fn record_bytes(bytes: i64) {
    let _ = THREAD_ALLOC_BYTES
        .try_with(|bytes_so_far| bytes_so_far.set(bytes_so_far.get().wrapping_add(bytes)));
}

/// The calling thread's live requested bytes so far, or `None` when no
/// binary has installed [`CountingAllocator`] in this process. The net of
/// allocations and deallocations on the thread, in requested (not
/// allocator-rounded) bytes: a delta between two reads is the measured
/// region's net growth. Wrapping arithmetic, like the allocation count's.
#[must_use]
pub fn thread_alloc_bytes() -> Option<i64> {
    if !INSTALLED.load(Ordering::Relaxed) {
        return None;
    }
    // A thread in teardown has no storage to read; it can make no
    // allocation either, so 0 is the honest count.
    Some(
        THREAD_ALLOC_BYTES
            .try_with(|bytes| bytes.get())
            .unwrap_or(0),
    )
}

/// The calling thread's allocation count so far, or `None` when no binary
/// has installed [`CountingAllocator`] in this process. The count only ever
/// grows: deallocation does not decrement, so a delta between two reads is
/// the number of allocations the thread made in between.
#[must_use]
pub fn thread_allocations() -> Option<u64> {
    if !INSTALLED.load(Ordering::Relaxed) {
        return None;
    }
    // A thread in teardown has no storage to read; it can make no
    // allocation either, so 0 is the honest count.
    Some(
        THREAD_ALLOCATIONS
            .try_with(|count| count.get())
            .unwrap_or(0),
    )
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count_allocation(layout.size() as i64);
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count_allocation(layout.size() as i64);
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        count_deallocation(layout.size());
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        count_allocation(new_size as i64 - layout.size() as i64);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[cfg(test)]
mod tests {
    use std::hint::black_box;

    use super::{CountingAllocator, thread_allocations};

    // The lib's own unit tests run without the allocator installed (the
    // crate stays a library); `thread_allocations` must read `None` here,
    // in-process, not only in the dedicated integration-test binaries.
    #[test]
    fn thread_allocations_is_none_in_the_library_itself() {
        assert!(thread_allocations().is_none());
    }

    #[test]
    fn counting_allocator_is_a_zero_sized_type() {
        assert_eq!(size_of::<CountingAllocator>(), 0);
        black_box(CountingAllocator);
    }
}
