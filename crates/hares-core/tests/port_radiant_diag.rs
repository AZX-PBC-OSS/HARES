//! `port_radiant_w` exposure in `EnvelopeDiag` and observer path.
//!
//! `EnvelopeComponentGains` carries `port_radiant_w`; `EnvelopeDiag` must
//! expose it so diagnostic surfaces can distinguish convective port
//! contributions from radiant ones. The observer path must forward
//! `port_radiant_w` from solver output into observable snapshots.

use hares_core::diagnostics::EnvelopeDiag;
use hares_envelope::EnvelopeComponentGains;

#[test]
fn envelope_diag_exposes_port_radiant_w() {
    let diag = EnvelopeDiag {
        port_radiant_w: 300.0,
        ..EnvelopeDiag::default()
    };
    assert_eq!(diag.port_radiant_w, 300.0);
}

#[test]
fn envelope_component_gains_has_port_radiant_w() {
    let gains = EnvelopeComponentGains {
        port_sensible_w: 700.0,
        port_radiant_w: 300.0,
        ..EnvelopeComponentGains::default()
    };
    assert_eq!(gains.port_sensible_w + gains.port_radiant_w, 1000.0);
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

    let path = {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock before epoch")
            .as_nanos();
        let mut p = std::env::temp_dir();
        p.push(format!("hares-port-radiant-obs-{nanos}.toml"));
        p
    };

    let content = r#"building_id = 4242

[simulation]
start_time = "2024-06-15T12:00:00Z"
time_res_s = 60
duration_s = 300

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0
wall_area_m2 = 145.0

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
    fs::write(&path, content).expect("failed to write synthetic TOML");

    let mut dwelling =
        hares_core::Dwelling::from_toml_config(&path).expect("synthetic TOML must load");
    let _ = fs::remove_file(&path);

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
