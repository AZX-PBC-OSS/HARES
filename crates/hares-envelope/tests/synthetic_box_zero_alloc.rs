//! Zero-allocation test for `step_into()` with a counting global allocator.
//!
//! This is in a separate test binary because `#[global_allocator]` applies to
//! the entire binary and would interfere with other tests.

use hares_envelope::{OutputMapping, StateSpaceModel};
use hares_types::alloc_count::{CountingAllocator, thread_allocations};
use nalgebra::{DMatrix, DVector};

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

    // The per-thread counter cannot be reset, so the hot loop is counted as
    // a delta between two reads on this thread.
    let before = thread_allocations().expect("the test installs the counting allocator");

    for _ in 0..100 {
        model.step_into(&x, &u, &mut buf);
        std::mem::swap(&mut x, &mut buf);
    }

    let after = thread_allocations().expect("the test installs the counting allocator");
    let allocs = after - before;
    assert!(
        allocs == 0,
        "step_into allocated {allocs} times during 100 steps; expected 0"
    );
}
