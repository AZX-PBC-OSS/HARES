//! Zero-allocation test for `step_into()` with a counting global allocator.
//!
//! This is in a separate test binary because `#[global_allocator]` applies to
//! the entire binary and would interfere with other tests.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use hares_envelope::{OutputMapping, StateSpaceModel};
use nalgebra::{DMatrix, DVector};

struct CountingAllocator;
static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

const UA: f64 = 20.0;
const C: f64 = 200_000.0;
const DT_S: f64 = 60.0;

#[test]
fn test_step_into_zero_allocations() {
    let a_c = DMatrix::from_row_slice(1, 1, &[-UA / C]);
    let b_c = DMatrix::from_row_slice(1, 2, &[UA / C, 1.0 / C]);

    let mapping = OutputMapping {
        output_count: 1,
        node_to_output: vec![(0, 0, 1.0)],
        input_to_output: vec![],
    };

    let model = StateSpaceModel::from_continuous(&a_c, &b_c, DT_S, &mapping).expect("model");

    let mut x = DVector::from_element(1, 20.0);
    let u = DVector::from_column_slice(&[0.0, 0.0]);
    let mut buf = DVector::zeros(1);

    // Warm up: let any lazy initialization happen
    model.step_into(&x, &u, &mut buf);
    std::mem::swap(&mut x, &mut buf);

    // Reset counter and run the hot loop
    ALLOC_COUNT.store(0, Ordering::SeqCst);

    for _ in 0..100 {
        model.step_into(&x, &u, &mut buf);
        std::mem::swap(&mut x, &mut buf);
    }

    let allocs = ALLOC_COUNT.load(Ordering::SeqCst);
    assert!(
        allocs == 0,
        "step_into allocated {allocs} times during 100 steps; expected 0"
    );
}
