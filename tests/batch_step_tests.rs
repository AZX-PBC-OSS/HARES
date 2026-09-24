//! Integration tests for batch-step observation collection.
//!
//! Verifies the batch-step pattern that mirrors Python `batch_step_py()`:
//! for N dwellings, stepping each and collecting `to_observation_vec()` yields
//! exactly N observation vectors.

#[cfg(test)]
mod tests {
    use hares_core::Dwelling;
    use rayon::prelude::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static NEXT_ID: AtomicU64 = AtomicU64::new(1);

    // Synthetic dwelling (furnace + electricity) produces zone "Indoor" and
    // equipment "Electric Furnace" (canonical HPXML heating name).
    const NARROW_FIELDS: &[&str] = &[
        "outdoor_temp",
        "zone_temp[Indoor]",
        "total_power_kw",
        "equipment_power[Electric Furnace]",
        "setpoint_heat[Indoor]",
    ];

    fn temp_path(prefix: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before UNIX epoch")
            .as_nanos();
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        path.push(format!("hares-test-batchstep-{prefix}-{nanos}-{id}.toml"));
        path
    }

    fn synthetic_toml(duration_s: i64, time_res_s: i64) -> String {
        format!(
            r#"building_id = 1

[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = {time_res_s}
duration_s = {duration_s}

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0
wall_area_m2 = 145.0

[materials]
wall_r_value_m2_k_w = 2.0

[hvac]
equipment_name = "Furnace"
fuel = "electricity"
heating_capacity_kbtu_h = 25.0

[weather]
outdoor_temp_c = 8.0
dew_point_c = 4.0
rel_humidity_pct = 55.0
pressure_kpa = 101.325

[schedule]
occupancy = 1.0

[output]
output_verbosity = 0
output_format = "csv"
output_chunk_size = 1000
master_seed = 0
write_output = false
"#
        )
    }

    fn make_dwelling(duration_s: i64, time_res_s: i64, idx: usize) -> Dwelling {
        let path = temp_path(&format!("{idx}"));
        fs::write(&path, synthetic_toml(duration_s, time_res_s)).expect("write TOML");
        Dwelling::from_toml_config_with_write_output(&path, Some(false)).expect("create dwelling")
    }

    #[test]
    fn batch_step_n_dwellings_yields_n_observations() {
        for n in [1usize, 4, 16] {
            let mut dwellings: Vec<Dwelling> =
                (0..n).map(|i| make_dwelling(3_600, 900, i)).collect();

            // Batch-step: step all in parallel, then collect observations.
            dwellings.par_iter_mut().for_each(|dwelling| {
                dwelling.step().expect("step");
            });
            let obs: Vec<_> = dwellings
                .iter()
                .map(|dwelling| {
                    dwelling
                        .telemetry()
                        .to_observation_vec(NARROW_FIELDS)
                        .expect("observation")
                })
                .collect();

            assert_eq!(
                obs.len(),
                n,
                "batch-step should yield one observation per dwelling"
            );
            for o in &obs {
                assert_eq!(
                    o.len(),
                    NARROW_FIELDS.len(),
                    "each observation vector should have narrow field count"
                );
            }
        }
    }

    #[test]
    fn batch_step_single_pass_step_and_observe() {
        // Mirrors the inner loop of batch_step_py: step + observe in one pass.
        let n = 4usize;
        let mut dwellings: Vec<Dwelling> = (0..n).map(|i| make_dwelling(3_600, 900, i)).collect();

        dwellings.par_iter_mut().for_each(|dwelling| {
            let _ = dwelling.step().expect("step");
            let t = dwelling.telemetry();
            let obs = t.to_observation_vec(NARROW_FIELDS).expect("obs");
            assert_eq!(
                obs.len(),
                NARROW_FIELDS.len(),
                "observation vector length must match field count inside parallel context"
            );
        });

        // After stepping, all dwellings should have advanced by one timestep.
        for dwelling in &dwellings {
            let t = dwelling.telemetry();
            assert_eq!(
                t.timestep_index, 1,
                "dwelling should have advanced to timestep 1 after a single step"
            );
        }
    }
}
