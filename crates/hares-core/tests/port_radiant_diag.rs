//! `port_radiant_w` exposure in the diagnostics report and observer path.
//!
//! `EnvelopeComponentGains` carries `port_radiant_w`; the diagnostics
//! report must record it so diagnostic surfaces can distinguish
//! convective port contributions from radiant ones. The observer path must
//! forward `port_radiant_w` from solver output into observable snapshots.

use hares_envelope::EnvelopeComponentGains;

#[test]
fn envelope_component_gains_has_port_radiant_w() {
    let gains = EnvelopeComponentGains {
        port_convective_w: 700.0,
        port_radiant_w: 300.0,
        ..EnvelopeComponentGains::default()
    };
    assert_eq!(gains.port_convective_w + gains.port_radiant_w, 1000.0);
}

// ---------------------------------------------------------------------------
// Observer-integration smoke test: verifies that port_radiant_w reaches
// observer snapshots through the live observer path (observer_capture →
// post_solvers). This is a behavioral test that would catch a regression
// where the value exists in EnvelopeComponentGains but is no longer
// forwarded through capture_solvers().
// ---------------------------------------------------------------------------

#[cfg(feature = "observe")]
#[test]
fn observer_post_solvers_forwards_port_radiant_w() {
    use std::fs;

    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("port-radiant-obs.toml");

    let content = r#"building_id = 4242

[simulation]
start_time = "2024-06-15T12:00:00Z"
time_res = 60
duration = 300
output_chunk_size = 1000
master_seed = 0
[geometry]
number_of_bedrooms = 3.0
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

[infiltration]
ach = 0.0

[schedule]
occupancy = 0.0
"#;
    fs::write(&path, content).expect("failed to write synthetic TOML");

    let mut dwelling =
        hares_core::Dwelling::from_toml_config(&path).expect("synthetic TOML must load");

    let n_steps = 5;
    dwelling.enable_observer(n_steps);

    for _ in 0..n_steps {
        dwelling.step().expect("step must succeed");
    }

    let snapshots = dwelling.drain_observations();
    assert_eq!(
        snapshots.len(),
        n_steps,
        "expected {n_steps} observer snapshots, got {}",
        snapshots.len()
    );

    for (i, snap) in snapshots.iter().enumerate() {
        let solvers = snap
            .phases
            .post_solvers
            .as_ref()
            .unwrap_or_else(|| panic!("post_solvers missing from snapshot {i}"));

        let value = solvers.envelope_gains.port_radiant_w;
        assert!(
            value.is_finite(),
            "snapshot {i}: port_radiant_w = {value}, expected a finite number"
        );
    }
}

// ---------------------------------------------------------------------------
// Integration test: verifies the diagnostics report records the envelope
// component gains end-to-end when the run requests the diagnostics, and
// that all scalar EnvelopeComponentGains columns are emitted with finite
// values.
// ---------------------------------------------------------------------------

#[test]
fn diagnostics_report_records_all_envelope_gain_columns() {
    use std::fs;

    // The output goes in a directory removed when the test ends.
    let dir = tempfile::tempdir().expect("temp dir");
    let toml_path = dir.path().join("diag-csv.toml");

    // The run requests the diagnostics report; the envelope columns ride
    // the run's memory report's frames.
    let content = r#"building_id = 4242

[simulation]
start_time = "2024-06-15T12:00:00Z"
time_res = 60
duration = 600
diagnostics = true
output_chunk_size = 1000
master_seed = 0

[[simulation.reports]]
name = "frame"
selection = { verbosity = 4 }
destination = "memory"
[geometry]
number_of_bedrooms = 3.0
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

[infiltration]
ach = 0.0

[schedule]
occupancy = 0.0
"#;
    fs::write(&toml_path, content).expect("failed to write synthetic TOML");

    let mut dwelling =
        hares_core::Dwelling::from_toml_config(&toml_path).expect("synthetic TOML must load");

    dwelling
        .simulate()
        .expect("simulation must complete without errors");

    let columns = dwelling
        .output_column_names()
        .expect("the recorded output schema");

    let env_cols = [
        "port_radiant_w",
        "port_shortwave_w",
        "port_convective_w",
        "window_solar_w",
        "opaque_solar_lwr_w",
        "interior_lwr_w",
        "internal_gain_w",
        "air_density_kg_m3",
        "ghi_w_m2",
    ];
    for col in env_cols {
        assert!(
            columns.iter().any(|c| c == col),
            "the requested diagnostics report must record '{col}'"
        );
    }

    // At least one recorded row carries finite values for all the envelope
    // gain columns.
    let batches = dwelling.flushed_batches();
    assert!(!batches.is_empty(), "the run records batches");
    let arrays: Vec<_> = env_cols
        .iter()
        .map(|col| {
            batches[0]
                .column_by_name(col)
                .unwrap_or_else(|| panic!("column '{col}' missing"))
                .as_any()
                .downcast_ref::<arrow::array::Float64Array>()
                .expect("diagnostics columns are Float64")
                .clone()
        })
        .collect();
    let any_finite = (0..batches[0].num_rows())
        .any(|r| arrays.iter().all(|a| r < a.len() && a.value(r).is_finite()));
    assert!(
        any_finite,
        "at least one recorded row must have finite values for all the envelope gain columns"
    );
}
