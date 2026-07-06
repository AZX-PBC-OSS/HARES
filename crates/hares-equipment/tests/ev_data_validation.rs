//! Data validation tests for EV charging defaults.
//!
//! Verifies the Level 1 charging power convention documented in
//! `defaults/ev/README.md`: `avg_power_kw` represents battery-side delivered
//! power after charging losses, not wall-side power. At SAE J1772 L1 baseline
//! (12 A at 120 V = 1.44 kW wall), typical grid-to-battery efficiency of
//! 85-88% yields 1.22-1.27 kW battery-side. The CSV value 1.26 kW corresponds
//! to ~87.5% efficiency at 1.44 kW wall, consistent with
//! EVI-Pro/EVERMI L1 efficiency assumptions.
//!
//! Source: SAE J1772 Standard (Level 1: 120 V AC, 12-16 A = 1.44-1.92 kW)
//! Source: EVI-Pro/EVERMI L1 grid-to-battery efficiency ~85-88%

use std::fs;
use std::time::Duration;

use chrono::{FixedOffset, TimeZone};
use hares_equipment::{Equipment, EquipmentConfig, EvConfig, ev::Ev};
use hares_types::{EnvironmentState, GridState, PortSlots, WeatherState, ZoneId, ZoneState};

// ─── CSV data validation helpers ────────────────────────────────────────

struct CsvIndex {
    vehicle_id: usize,
    avg_power_kw: usize,
    charge_time: usize,
    total_charge: usize,
    capacity_kwh: usize,
}

/// Parse a single CSV line into fields, handling quoted fields containing commas.
fn parse_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;

    for ch in line.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                fields.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    fields.push(current.trim().to_string());
    fields
}

fn read_csv_rows(path: &str) -> Vec<Vec<String>> {
    let content = fs::read_to_string(path).expect("should be able to read CSV file");
    let mut lines: Vec<&str> = content.lines().collect();
    if !lines.is_empty() {
        lines.remove(0);
    }
    lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|line| parse_csv_line(line))
        .collect()
}

fn parse_header_indices(path: &str) -> CsvIndex {
    let content = fs::read_to_string(path).expect("should be able to read CSV file");
    let header_line = content.lines().next().expect("CSV should have a header");
    let headers = parse_csv_line(header_line);

    let find = |name: &str| -> usize {
        headers
            .iter()
            .position(|h| *h == name)
            .unwrap_or_else(|| panic!("column '{name}' not found in header of {path}"))
    };

    CsvIndex {
        vehicle_id: find("vehicle_id"),
        avg_power_kw: find("avg_power_kw"),
        charge_time: find("charge_time"),
        total_charge: find("total_charge"),
        capacity_kwh: find("Capacity (kWh)"),
    }
}

fn csv_path(filename: &str) -> String {
    format!(
        "{}/../../defaults/ev/{filename}",
        env!("CARGO_MANIFEST_DIR")
    )
}

// ─── Integration test fixtures ──────────────────────────────────────────

fn default_env() -> EnvironmentState {
    EnvironmentState {
        zones: vec![ZoneState {
            id: ZoneId(1),
            temperature_c: 21.0,
            humidity_ratio: 0.008,
            volume_m3: 200.0,
        }],
        weather: WeatherState {
            outdoor_temp_c: 20.0,
            outdoor_humidity_ratio: 0.008,
            outdoor_wet_bulb_c: 15.0,
            outdoor_enthalpy_j_kg: 40_000.0,
            wind_speed_m_s: 0.0,
            wind_dir_deg: 0.0,
            ground_temp_c: 15.0,
            sky_temp_c: 10.0,
            pressure_kpa: 101.325,
            solar_irradiance: vec![],
            ghi_w_m2: 0.0,
            dni_w_m2: 0.0,
            dhi_w_m2: 0.0,
            solar_altitude_deg: 0.0,
            solar_azimuth_deg: 180.0,
            mains_temp_c: 15.0,
            rainfall_m: 0.0,
            ground_albedo: 0.2,
            ground_t_mean_c: 15.0,
            ground_t_amplitude_c: 0.0,
            ground_phase_day: 35.0,
            day_of_year: 1.0,
        },
        grid: GridState {
            voltage_pu: 1.0,
            frequency_hz: 60.0,
        },
        custom_domains: vec![],
        equipment_telemetry: std::collections::HashMap::new(),
        equipment_core: Default::default(),
        current_time: FixedOffset::east_opt(0)
            .expect("UTC offset")
            .with_ymd_and_hms(2026, 6, 21, 12, 0, 0)
            .single()
            .expect("valid timestamp"),
        time_res: chrono::Duration::seconds(60),
        price_signal: Default::default(),
        electrical: Default::default(),
    }
}

fn l1_ev_config(
    wall_kw: f64,
    efficiency: f64,
    capacity_kwh: f64,
    initial_soc: f64,
) -> EquipmentConfig {
    EquipmentConfig::from_typed(
        "EV-L1-Test".to_string(),
        "EV".to_string(),
        EvConfig {
            equipment_id: None,
            capacity_kwh,
            charging_level: Some("L1".to_string()),
            max_charging_power_kw: wall_kw,
            charging_efficiency: Some(efficiency),
            l1_current_a: None,
            l1_voltage_v: None,
            soc_max: None,
            initial_soc: Some(initial_soc),
            battery_temp_c: None,
            min_charge_temp_c: None,
            full_power_temp_c: None,
            heater_power_w: None,
            heater_threshold_c: None,
            thermal_mass_j_per_k: None,
            ua_w_per_k: None,
            v2l_enabled: None,
            v2l_soc_reserve: None,
            v2l_max_discharge_kw: None,
            v2g_enabled: None,
            v2g_soc_reserve: None,
            v2g_max_discharge_kw: None,
            chemistry: None,
            fuel_economy_kwh_per_mi: None,
            ready_soc: None,
            charging_strategy: None,
            plug_in_policy: None,
            power_limit_kw: None,
            initial_connection_state: None,
            power_factor: None,
            charger_capacity_kva: None,
        },
    )
    .unwrap()
}

// ────────────────────────────────────────────────────────────────────────
// CSV data validation tests
// ────────────────────────────────────────────────────────────────────────

#[test]
fn bev_level1_avg_power_kw_is_1_26_kw_battery_side() {
    let path = csv_path("BEV_level_1.csv");
    let idx = parse_header_indices(&path);
    let rows = read_csv_rows(&path);

    assert!(
        rows.len() > 100,
        "BEV_level_1.csv should have >100 data rows"
    );

    for row in &rows {
        let avg_power: f64 = row[idx.avg_power_kw]
            .parse()
            .expect("avg_power_kw should parse as f64");
        assert!(
            (avg_power - 1.26).abs() < 1e-9,
            "vehicle {}: expected avg_power_kw = 1.26 (battery-side), got {avg_power}",
            row[idx.vehicle_id]
        );
    }
}

#[test]
fn phev_level1_avg_power_kw_is_1_26_kw_battery_side() {
    let path = csv_path("PHEV_level_1.csv");
    let idx = parse_header_indices(&path);
    let rows = read_csv_rows(&path);

    assert!(
        rows.len() > 100,
        "PHEV_level_1.csv should have >100 data rows"
    );

    for row in &rows {
        let avg_power: f64 = row[idx.avg_power_kw]
            .parse()
            .expect("avg_power_kw should parse as f64");
        assert!(
            (avg_power - 1.26).abs() < 1e-9,
            "vehicle {}: expected avg_power_kw = 1.26 (battery-side), got {avg_power}",
            row[idx.vehicle_id]
        );
    }
}

#[test]
fn bev_level1_total_charge_consistent_with_avg_power() {
    // total_charge = avg_power_kw × charge_time / 60 (exact, because
    // EVI-Pro pre-computed these values at 100% apparent efficiency).
    let path = csv_path("BEV_level_1.csv");
    let idx = parse_header_indices(&path);
    let rows = read_csv_rows(&path);

    for row in &rows {
        let avg_power: f64 = row[idx.avg_power_kw]
            .parse()
            .expect("avg_power_kw should parse as f64");
        let charge_time: f64 = row[idx.charge_time]
            .parse()
            .expect("charge_time should parse as f64");
        let total_charge: f64 = row[idx.total_charge]
            .parse()
            .expect("total_charge should parse as f64");

        let expected = avg_power * charge_time / 60.0;
        let delta = (total_charge - expected).abs();
        assert!(
            delta < 1e-3 || delta / total_charge.max(1e-9) < 1e-10,
            "vehicle {}: total_charge={total_charge}, expected {expected}",
            row[idx.vehicle_id]
        );
    }
}

#[test]
fn phev_level1_total_charge_consistent_with_avg_power() {
    let path = csv_path("PHEV_level_1.csv");
    let idx = parse_header_indices(&path);
    let rows = read_csv_rows(&path);

    for row in &rows {
        let avg_power: f64 = row[idx.avg_power_kw]
            .parse()
            .expect("avg_power_kw should parse as f64");
        let charge_time: f64 = row[idx.charge_time]
            .parse()
            .expect("charge_time should parse as f64");
        let total_charge: f64 = row[idx.total_charge]
            .parse()
            .expect("total_charge should parse as f64");

        let expected = avg_power * charge_time / 60.0;
        let delta = (total_charge - expected).abs();
        assert!(
            delta < 1e-3 || delta / total_charge.max(1e-9) < 1e-10,
            "vehicle {}: total_charge={total_charge}, expected {expected}",
            row[idx.vehicle_id]
        );
    }
}

#[test]
fn bev_level1_capacity_kwh_is_117_6() {
    let path = csv_path("BEV_level_1.csv");
    let idx = parse_header_indices(&path);
    let rows = read_csv_rows(&path);

    for row in &rows {
        let capacity: f64 = row[idx.capacity_kwh]
            .parse()
            .expect("Capacity (kWh) should parse as f64");
        assert!(
            (capacity - 117.6).abs() < 1e-6,
            "vehicle {}: expected Capacity (kWh) = 117.6, got {capacity}",
            row[idx.vehicle_id]
        );
    }
}

#[test]
fn phev_level1_capacity_kwh_is_14_8() {
    let path = csv_path("PHEV_level_1.csv");
    let idx = parse_header_indices(&path);
    let rows = read_csv_rows(&path);

    for row in &rows {
        let capacity: f64 = row[idx.capacity_kwh]
            .parse()
            .expect("Capacity (kWh) should parse as f64");
        assert!(
            (capacity - 14.8).abs() < 1e-6,
            "vehicle {}: expected Capacity (kWh) = 14.8, got {capacity}",
            row[idx.vehicle_id]
        );
    }
}

// ────────────────────────────────────────────────────────────────────────
// Integration tests: wall-power to battery-power relationship
// ────────────────────────────────────────────────────────────────────────

/// SAE J1772 Level 1 baseline: 12 A at 120 V = 1.44 kW wall power.
/// Source: SAE J1772 Standard.
const SAE_L1_BASELINE_WALL_KW: f64 = 1.44;

/// Typical L1 grid-to-battery efficiency from EVI-Pro/EVERMI.
/// Source: EVI-Pro/EVERMI L1 efficiency assumptions ~85-88%.
const TYPICAL_L1_EFFICIENCY: f64 = 0.875;

/// Verify that an EV configured with the SAE J1772 L1 baseline wall power
/// reports the expected wall-side active power, and that the battery SOC
/// increase matches the wall-to-battery efficiency relationship:
///
///   battery_energy = wall_power × time × efficiency
#[test]
fn l1_sae_baseline_charging_preserves_energy_relationship() {
    let capacity_kwh = 117.6;
    let initial_soc = 0.5;
    let config = l1_ev_config(
        SAE_L1_BASELINE_WALL_KW,
        TYPICAL_L1_EFFICIENCY,
        capacity_kwh,
        initial_soc,
    );
    let mut ev = Ev::new(config.clone());
    let env = default_env();
    ev.init(&config, &env).unwrap();

    let soc_before = ev.telemetry().get("soc").unwrap_or(initial_soc);
    assert!(
        (soc_before - initial_soc).abs() < 1e-12,
        "SOC should start at initial value"
    );

    let dt = Duration::from_secs(60 * 60); // 1 hour
    let mut ports = PortSlots::default();
    ev.step(&env, dt, &mut ports).unwrap();

    let wall_kw = ev
        .telemetry()
        .get("active_power_kw")
        .expect("active_power_kw should be present in telemetry");

    assert!(
        (wall_kw - SAE_L1_BASELINE_WALL_KW).abs() < 1e-9,
        "wall power should be {SAE_L1_BASELINE_WALL_KW} kW, got {wall_kw}"
    );

    let soc_after = ev
        .telemetry()
        .get("soc")
        .expect("soc should be present in telemetry");

    let dt_hours = dt.as_secs_f64() / 3600.0;
    let expected_soc_delta = wall_kw * dt_hours * TYPICAL_L1_EFFICIENCY / capacity_kwh;
    let actual_soc_delta = soc_after - soc_before;

    assert!(
        (actual_soc_delta - expected_soc_delta).abs() < 1e-9,
        "SOC delta {actual_soc_delta} should match wall_power × time × efficiency / capacity = {expected_soc_delta}"
    );

    // Verify the battery-side effective power matches the CSV convention:
    // 1.44 kW wall × 0.875 efficiency = 1.26 kW battery-side.
    let battery_side_kw = wall_kw * TYPICAL_L1_EFFICIENCY;
    assert!(
        (battery_side_kw - 1.26).abs() < 1e-9,
        "battery-side effective power should be 1.26 kW, got {battery_side_kw}"
    );

    // Port contribution should reflect wall-side load.
    assert!(
        ports.electrical.load_power_w > 0.0,
        "should draw power from the grid"
    );
}

/// Verify that the CSV convention (1.26 kW battery-side) is consistent
/// with SAE J1772 L1 baseline wall power at the documented efficiency range.
#[test]
fn csv_convention_matches_sae_l1_baseline() {
    // The CSV documentation states avg_power_kw = 1.26 is battery-side.
    // SAE J1772 L1 minimum: 12 A × 120 V = 1.44 kW wall.
    // EVI-Pro/EVERMI efficiency range: 85-88%.
    //
    // At 1.44 kW wall × 0.875 efficiency = 1.26 kW battery → exact match.
    // At 1.44 kW wall × 0.85  efficiency = 1.224 kW battery.
    // At 1.44 kW wall × 0.88  efficiency = 1.267 kW battery.

    let csv_battery_kw = 1.26;
    let wall_at_85pct = SAE_L1_BASELINE_WALL_KW * 0.85; // 1.224
    let wall_at_88pct = SAE_L1_BASELINE_WALL_KW * 0.88; // 1.2672

    assert!(
        csv_battery_kw >= wall_at_85pct && csv_battery_kw <= wall_at_88pct,
        "CSV avg_power_kw = {csv_battery_kw} kW should fall in [{wall_at_85pct}, {wall_at_88pct}] for SAE L1 at 85-88% efficiency"
    );
}
