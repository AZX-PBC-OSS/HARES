//! The IEEE 1547 layer's non-finite fail-safe: a poisoned grid reading
//! (NaN or infinity on the bus voltage or the frequency) trips the
//! inverter with its own reason code, and the trip latches like the
//! others. The telemetry publishes the reason as a number, so the
//! witnesses pin the published code: 9 is the fail-safe's own code, the
//! vocabulary's extension past the eight standard conditions (0 "none",
//! 1 UV2 ... 8 OF2).

use hares_equipment::der1547::{Der1547Category, Der1547Controller, Der1547Settings};

const INPUT_INVALID: u8 = 9;

fn controller() -> Der1547Controller {
    let settings = Der1547Settings {
        category: Der1547Category::CatII,
        overrides: None,
    };
    Der1547Controller::new(&settings, 5000.0, "TestDER").expect("valid controller")
}

/// A poisoned reading trips with the fail-safe's own code and the trip
/// latches: the next finite step waits on the enter-service windows
/// instead of resuming. The red state at the parent returned reason 0
/// ("none": the vocabulary had no word for a tripped inverter with no
/// fired condition) and left the latch unset, so the next finite step
/// resumed with no enter-service delay at all.
#[test]
fn a_non_finite_grid_reading_trips_with_its_own_code_and_latches() {
    let mut ctrl = controller();
    let nominal = 3000.0;

    // A NaN or infinite voltage or frequency poisons the layer: NaN fails
    // every band comparison (each comparison against a NaN is false) and
    // infinity rides no table's band.
    for (v, f) in [(f64::NAN, 60.0), (1.0, f64::INFINITY), (f64::NAN, f64::NAN)] {
        let outcome = ctrl
            .step(v, f, 1.0, true, nominal, nominal, 0.0)
            .expect("a poisoned reading is not a step error");
        assert!(
            outcome.tripped,
            "a poisoned reading ({v}, {f}) de-energizes the inverter"
        );
        assert_eq!(
            outcome.trip_reason, INPUT_INVALID,
            "the fail-safe trip names its own condition, not 'none'"
        );
        assert_eq!(outcome.p_ref_w, 0.0, "a tripped inverter produces nothing");
    }

    // The latch: the next FINITE step is still tripped, the reason
    // carried, and the enter-service delay accumulates (the Category II
    // default is 300 s; one nominal 1 s step does not reconnect).
    let outcome = ctrl
        .step(1.0, 60.0, 1.0, true, nominal, nominal, 0.0)
        .expect("the finite step");
    assert!(
        outcome.tripped,
        "the fail-safe trip latches like the others"
    );
    assert_eq!(outcome.trip_reason, INPUT_INVALID);
    assert_eq!(outcome.p_ref_w, 0.0);
    assert_eq!(
        outcome.enter_service_s, 1.0,
        "the return to service accumulates"
    );
    assert!(ctrl.state().tripped);
}
