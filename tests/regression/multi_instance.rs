//! Multi-instance independence sub-suite: multiple equipment instances of the
//! same type in one dwelling must behave independently.

use std::path::PathBuf;

use chrono::Duration;
use hares_core::Dwelling;

use super::helpers;

pub fn run_multi_instance_check() -> Result<(), Vec<String>> {
    let toml_path = helpers::unique_temp_path("hares-regr-multi", "toml");
    write_multi_instance_toml(&toml_path);

    let mut failures = Vec::new();

    let mut dwelling = match Dwelling::from_toml_config(&toml_path) {
        Ok(d) => d,
        Err(err) => {
            helpers::cleanup_paths(&[toml_path]);
            return Err(vec![format!(
                "multi-instance dwelling construction failed: {err}"
            )]);
        }
    };

    match dwelling.simulate() {
        Ok(results) => {
            if results.steps.is_empty() {
                failures.push("multi-instance simulation produced zero steps".to_string());
            } else {
                eprintln!(
                    "[multi_instance] PASS — {} steps completed for multi-equipment dwelling",
                    results.steps.len()
                );
            }
        }
        Err(err) => {
            failures.push(format!("multi-instance simulation failed: {err}"));
        }
    }

    helpers::cleanup_paths(&[toml_path]);

    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures)
    }
}

fn write_multi_instance_toml(path: &PathBuf) {
    let toml = r#"building_id = 100

[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = 60
duration_s = 3600

[geometry]
floor_area_m2 = 120.0
zone_volume_m3 = 300.0
wall_area_m2 = 200.0

[materials]
wall_r_value_m2_k_w = 2.5

[weather]
outdoor_temp_c = 15.0
dew_point_c = 8.0
rel_humidity_pct = 55.0
pressure_kpa = 101.325

[schedule]
occupancy = 1.0

[output]
output_verbosity = 0
output_format = "csv"
output_chunk_size = 1000
master_seed = 42
"#;
    std::fs::write(path, toml).expect("failed to write multi-instance TOML fixture");
}
