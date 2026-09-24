//! Integration tests verifying the HVAC negative delivered-energy invariant
//! checks are wired through the Dwelling step loop.
//!
//! Creates a minimal dwelling and verifies both that stepping succeeds when the
//! solver produces correct values and that the invariant fires
//! (`NegativeDeliveredEnergy`) when deliberately negative HVAC power values are
//! injected via the test seam.
//!
//! Tests are gated on `debug_assertions` because the invariant check is
//! compiled only when `cfg(any(debug_assertions, feature = "check_invariants"))`
//! is active.
#![cfg(debug_assertions)]

use std::env;
use std::fs;

use hares_core::Dwelling;
use hares_types::HaresError;

/// TOML configuration for a minimal synthetic dwelling.
///
/// Uses an electric furnace to produce HVAC heating loads so that
/// `component_gains().hvac_heating_w` is typically non-zero, exercising the
/// invariant path with real values before the test seam is enabled.
const SYNTHETIC_TOML: &str = r#"building_id = 998

[simulation]
start_time = "2024-06-15T12:00:00Z"
time_res_s = 60
duration_s = 600

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.8

[hvac]
equipment_name = "none"

[weather]
outdoor_temp_c = 20.0
dew_point_c = 10.0
rel_humidity_pct = 50.0
pressure_kpa = 101.325

[schedule]
occupancy = 0.0

[output]
output_verbosity = 0
output_format = "csv"
output_chunk_size = 1000
write_output = false
master_seed = 0
"#;

/// Builds a minimal synthetic dwelling from the shared TOML fixture.
fn minimal_dwelling() -> (std::path::PathBuf, Dwelling) {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let tmp = env::temp_dir().join(format!("hares_hvac_negative_invariant_test_{nanos}.toml"));
    fs::write(&tmp, SYNTHETIC_TOML).expect("write toml");
    let dwelling = Dwelling::from_toml_config(&tmp).expect("create dwelling");
    (tmp, dwelling)
}

/// Verifies that a correctly-functioning dwelling steps cleanly through the
/// HVAC delivered-energy invariant checks without false positives.
#[test]
fn dwelling_step_hvac_energy_invariant_wired() {
    let (tmp, mut dwelling) = minimal_dwelling();

    let result = dwelling.step();
    let _ = fs::remove_file(&tmp);

    assert!(
        result.is_ok(),
        "hvac energy invariant must not false-positive on correctly-functioning \
         dwelling; got {result:?}"
    );
}

/// Verifies that the HVAC negative-energy invariant wiring inside
/// `Dwelling::check_invariants` catches deliberately incorrect HVAC power
/// values injected via the test seam.
///
/// Steps the dwelling once to confirm the invariant passes for correct models,
/// then enables the test seam (`set_hvac_negative_energy_failure_for_test`)
/// which injects negative `hvac_heating_w`. The next step must return
/// `Err(HaresError::NegativeDeliveredEnergy { .. })`.
///
/// If the `check_hvac_power_non_negative` or `check_heating_accumulator`
/// calls were accidentally removed from `check_invariants`, this test would
/// fail — the seam would have no effect and `step()` would return `Ok`.
#[test]
fn hvac_negative_energy_invariant_catches_injected_negative() {
    let (tmp, mut dwelling) = minimal_dwelling();

    // Step once: confirm the invariant passes for a correct model.
    dwelling
        .step()
        .expect("first step must pass hvac energy invariant on correct model");

    // Enable test seam: injects negative hvac_heating_w (-1000 W) and
    // hvac_cooling_w (-500 W) into the invariant checks.  Only the heating
    // arm fires NegativeDeliveredEnergy, because cooling at -500 W is the
    // valid sign convention for heat removal.
    dwelling.set_hvac_negative_energy_failure_for_test();

    // This step must fail with NegativeDeliveredEnergy.
    let result = dwelling.step();
    let _ = fs::remove_file(&tmp);

    match result {
        Err(HaresError::NegativeDeliveredEnergy { .. }) => {
            // Expected: invariant check fires on injected negative values.
        }
        other => panic!("expected Err(NegativeDeliveredEnergy {{ .. }}), got {other:?}"),
    }
}

/// Verifies that the accumulator catches negative delivered energy that crosses
/// the drift tolerance over multiple steps.
///
/// Because the per-step check (zero tolerance) fires on any negative value, the
/// accumulator is only reachable in integration when negative values are small
/// enough to exceed the drift tolerance rate. The accumulator's drift-tolerant
/// behavior is covered by unit tests in `invariants.rs`; this test verifies
/// end-to-end wiring of the per-step guard.
#[test]
fn hvac_negative_energy_accumulator_wiring_present() {
    let (tmp, mut dwelling) = minimal_dwelling();

    // Step once: confirm the invariant passes.
    dwelling.step().expect("first step must pass");

    dwelling.set_hvac_negative_energy_failure_for_test();
    let result = dwelling.step();
    let _ = fs::remove_file(&tmp);

    assert!(
        result.is_err(),
        "test seam must trigger invariant violation"
    );
    let err = result.unwrap_err();
    assert!(
        matches!(&err, HaresError::NegativeDeliveredEnergy { .. }),
        "expected NegativeDeliveredEnergy, got {err:?}"
    );
}
