//! Integration test: actor telemetry columns appear in diagnostic CSV output.
//!
//! Verifies that when a dwelling runs with actors registered, the output schema
//! includes dynamically-named actor telemetry columns (e.g. `actor:Occupant:presence`)
//! and that those columns are populated with correct values after a multi-step
//! simulation.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use hares_core::Dwelling;
use hares_core::actors::Occupant;
use hares_core::actors::Presence;

fn nanos_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_nanos()
}

fn unique_temp_toml(tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("hares-actor-diag-{tag}-{}.toml", nanos_suffix()));
    path
}

fn unique_temp_csv(tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("hares-actor-diag-{tag}-{}.csv", nanos_suffix()));
    path
}

fn write_minimal_toml_with_output(path: &PathBuf, csv_path: &Path) {
    let content = format!(
        r#"building_id = 4242

[simulation]
start_time = "2024-06-15T12:00:00Z"
time_res_s = 60
duration_s = 600

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
write_output = true
output_path = "{}"
master_seed = 0
"#,
        csv_path.display()
    );
    fs::write(path, content).expect("failed to write synthetic TOML");
}

#[test]
fn actor_telemetry_columns_in_csv_output_after_multi_step_simulation() {
    let csv_path = unique_temp_csv("actor_csv");
    let toml_path = unique_temp_toml("actor_csv");
    write_minimal_toml_with_output(&toml_path, &csv_path);

    let mut dwelling = Dwelling::from_toml_config(&toml_path).expect("synthetic TOML must load");
    let _ = fs::remove_file(&toml_path);

    // Add an Occupant actor with telemetry keys. Schedule covers all 10 steps
    // (600s / 60s = 10 steps). Pattern: Home, Home, Away, Away, Home, Home,
    // Away, Away, Home, Home — two transitions tested.
    let schedule = vec![
        Presence::Home,
        Presence::Home,
        Presence::Away,
        Presence::Away,
        Presence::Home,
        Presence::Home,
        Presence::Away,
        Presence::Away,
        Presence::Home,
        Presence::Home,
    ];
    dwelling.add_actor(Box::new(
        Occupant::new("Occupant").with_presence_schedule(schedule),
    ));

    // Run the full simulation horizon (10 steps at 60s each).
    dwelling
        .simulate()
        .expect("simulation must complete without errors");

    // Read the CSV output and verify it contains actor telemetry columns.
    let csv_content = fs::read_to_string(&csv_path).expect("CSV output must exist and be readable");
    let _ = fs::remove_file(&csv_path);

    // The header line must contain actor telemetry column names.
    let header = csv_content
        .lines()
        .next()
        .expect("CSV must have header row");
    assert!(
        header.contains("actor:Occupant:away"),
        "CSV header must contain 'actor:Occupant:away'; header was: {header}"
    );
    assert!(
        header.contains("actor:Occupant:transition"),
        "CSV header must contain 'actor:Occupant:transition'; header was: {header}"
    );
    assert!(
        header.contains("actor:Occupant:signals_count"),
        "CSV header must contain 'actor:Occupant:signals_count'; header was: {header}"
    );

    // Verify that at least one data row has non-default values.
    let data_rows: Vec<&str> = csv_content.lines().skip(1).collect();
    assert!(
        !data_rows.is_empty(),
        "CSV output must contain at least one data row"
    );

    // Count header columns to find indices of actor telemetry columns.
    let headers: Vec<&str> = header.split(',').collect();
    let away_idx = headers
        .iter()
        .position(|h| *h == "actor:Occupant:away")
        .expect("away column must exist");
    let transition_idx = headers
        .iter()
        .position(|h| *h == "actor:Occupant:transition")
        .expect("transition column must exist");
    let signals_idx = headers
        .iter()
        .position(|h| *h == "actor:Occupant:signals_count")
        .expect("signals_count column must exist");

    // Step 2 (third data row, 0-indexed row 2) transitions Home→Away:
    //   away = 1.0 (occupied), transition = 1.0 (change occurred).
    let row3: Vec<&str> = data_rows[2].split(',').collect();
    let away_val: f64 = row3[away_idx].parse().expect("away column must be numeric");
    let transition_val: f64 = row3[transition_idx]
        .parse()
        .expect("transition column must be numeric");

    // At step 2 (third row), occupant transitions to Away.
    // `away` telemetry key: 1.0 = away, 0.0 = home
    // `transition` telemetry key: 1.0 = transition occurred, 0.0 = no change
    assert!(
        (away_val - 1.0).abs() < 1e-9,
        "step 2 must report away=1.0 (occupant is away); got {away_val}"
    );
    assert!(
        (transition_val - 1.0).abs() < 1e-9,
        "step 2 must report transition=1.0 (transition occurred); got {transition_val}"
    );

    // signals_count is 0 when the actor has no equipment targets to dispatch to,
    // but the column must still be present and parsed as a valid number.
    let signals_val: f64 = row3[signals_idx]
        .parse()
        .expect("signals_count column must be numeric");
    assert!(
        signals_val.is_finite(),
        "step 2 signals_count must be finite; got {signals_val}"
    );

    // Step 3 (row 4) stays away, so no transition.
    let row4: Vec<&str> = data_rows[3].split(',').collect();
    let away_step3: f64 = row4[away_idx].parse().expect("away column must be numeric");
    let transition_step3: f64 = row4[transition_idx]
        .parse()
        .expect("transition column must be numeric");
    assert!(
        (away_step3 - 1.0).abs() < 1e-9,
        "step 3 must report away=1.0; got {away_step3}"
    );
    assert!(
        (transition_step3 - 0.0).abs() < 1e-9,
        "step 3 must report transition=0.0 (no change); got {transition_step3}"
    );
}
