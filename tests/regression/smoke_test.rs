//! Smoke test — runs real OCHRE vendor fixtures through HARES and prints
//! per-equipment power breakdown for comparison against OCHRE reference.

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;

    use chrono::{Duration, TimeZone, Utc};
    use hares_core::{DwellingConfig, SimStatus, SimulationConfig, SimulationEngine};
    use hares_io::OutputFormat;

    fn vendor_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vendors/OCHRE")
    }

    fn project_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn beopt_config(duration_hours: i64, output_path: PathBuf) -> DwellingConfig {
        DwellingConfig {
            hpxml_path: vendor_dir().join("ochre/defaults/Input Files/BEopt_example.xml"),
            schedule_path: vendor_dir()
                .join("ochre/defaults/Input Files/BEopt_example_schedule.csv"),
            weather_path: vendor_dir()
                .join("ochre/defaults/Weather/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
            defaults_path: Some(project_root().join("defaults")),
            sim_config: SimulationConfig {
                // Match OCHRE smoke: May 5, 2019, 12:00 PM Denver (UTC-7) = 19:00 UTC
                start_time: Utc.with_ymd_and_hms(2019, 5, 5, 19, 0, 0).unwrap(),
                duration: Duration::hours(duration_hours),
                time_res: Duration::minutes(1),
                output_verbosity: 3,
                output_path: Some(output_path),
                output_format: OutputFormat::Csv,
                output_chunk_size: 1024,
                setpoint_deadband_c: None,
                master_seed: 42,
            },
            overrides: None,
            bldg_id: 1,
            initialization_duration: None,
        }
    }

    /// Parse CSV output and compute per-column kWh (sum * timestep_hours).
    fn parse_csv_power_kwh(path: &PathBuf, timestep_minutes: f64) -> BTreeMap<String, f64> {
        let contents = fs::read_to_string(path).expect("read output CSV");
        let mut lines = contents.lines();
        let header = lines.next().expect("header line");
        let columns: Vec<&str> = header.split(',').collect();

        let mut sums: Vec<f64> = vec![0.0; columns.len()];
        let mut count = 0usize;

        for line in lines {
            if line.trim().is_empty() {
                continue;
            }
            let fields: Vec<&str> = line.split(',').collect();
            for (i, field) in fields.iter().enumerate() {
                if i < sums.len() {
                    if let Ok(v) = field.trim().parse::<f64>() {
                        sums[i] += v;
                    }
                }
            }
            count += 1;
        }

        let timestep_h = timestep_minutes / 60.0;
        let mut result = BTreeMap::new();
        for (i, col) in columns.iter().enumerate() {
            let col = col.trim();
            if col.ends_with("(kW)") || col.ends_with("(therms/hour)") {
                let kwh = sums[i] * timestep_h;
                result.insert(col.to_string(), kwh);
            }
        }
        // Also add mean power for comparison
        if count > 0 {
            for (i, col) in columns.iter().enumerate() {
                let col = col.trim();
                if col.ends_with("(kW)") {
                    result.insert(format!("{col} [mean_kW]"), sums[i] / count as f64);
                }
            }
        }
        result
    }

    #[test]
    fn smoke_beopt_1h() {
        let output_path = std::env::temp_dir().join("hares_smoke_beopt_1h.csv");

        let engine = SimulationEngine::new();
        let result = engine
            .run(beopt_config(1, output_path.clone()))
            .expect("engine.run should succeed");

        // The engine may return a different resolved path
        let actual_output = result
            .timeseries_path
            .as_ref()
            .cloned()
            .unwrap_or(output_path.clone());
        eprintln!("  output_path requested: {}", output_path.display());
        eprintln!("  timeseries_path returned: {}", actual_output.display());
        eprintln!("  output_path exists: {}", output_path.exists());
        eprintln!("  timeseries_path exists: {}", actual_output.exists());

        assert!(
            !matches!(result.status, SimStatus::Failed(_)),
            "BEopt simulation failed: {:?}",
            result.status
        );

        eprintln!(
            "[smoke_beopt_1h] status={:?} elapsed={:?}",
            result.status, result.elapsed
        );
        eprintln!("  warnings:");
        for w in &result.warnings {
            eprintln!("    {w}");
        }
        eprintln!(
            "  total kWh (from metrics): {:.4}",
            result.metrics.annual_energy_kwh.total
        );

        // Parse output CSV for per-equipment breakdown
        let csv_path = if actual_output.exists() {
            actual_output.clone()
        } else {
            output_path.clone()
        };
        if csv_path.exists() {
            let breakdown = parse_csv_power_kwh(&csv_path, 1.0);
            eprintln!("\n  === HARES per-equipment kWh (1h) ===");
            for (col, kwh) in &breakdown {
                if !col.contains("[mean_kW]") {
                    eprintln!("    {col:55} {kwh:10.4}");
                }
            }
            eprintln!("\n  === HARES mean kW ===");
            for (col, mean_kw) in &breakdown {
                if col.contains("[mean_kW]") {
                    eprintln!("    {col:55} {mean_kw:10.4}");
                }
            }

            // OCHRE reference values (from OCHRE 0.9.2, same inputs, same time window)
            eprintln!("\n  === OCHRE reference (kWh) ===");
            let ochre_ref = [
                ("Total Electric Power (kW)", 1.3395),
                ("HVAC Heating Electric Power (kW)", 0.9113),
                ("HVAC Cooling Electric Power (kW)", 0.0500),
                ("Indoor Lighting Electric Power (kW)", 0.0879),
                ("Exterior Lighting Electric Power (kW)", 0.0064),
                ("MELs Electric Power (kW)", 0.1340),
                ("TV Electric Power (kW)", 0.0761),
                ("Refrigerator Electric Power (kW)", 0.0540),
                ("Ventilation Fan Electric Power (kW)", 0.0199),
            ];
            for (name, ochre_kwh) in &ochre_ref {
                let hares_kwh = breakdown.get(*name).copied().unwrap_or(f64::NAN);
                let ochre_val: f64 = *ochre_kwh;
                let diff_pct = if ochre_val.abs() > 1e-9 {
                    (hares_kwh - ochre_val) / ochre_val * 100.0
                } else if hares_kwh.abs() > 1e-9 {
                    f64::INFINITY
                } else {
                    0.0
                };
                eprintln!(
                    "    {name:55} OCHRE={ochre_kwh:8.4}  HARES={hares_kwh:8.4}  diff={diff_pct:+7.1}%"
                );
            }

            // Debug: dump heater stats from CSV
            let contents = fs::read_to_string(&csv_path).unwrap();
            let mut csv_lines = contents.lines();
            let hdr = csv_lines.next().unwrap();
            let cols: Vec<&str> = hdr.split(',').collect();
            // Find column indices for heater debug
            let find_col = |name: &str| cols.iter().position(|c| c.trim() == name);
            let heater_kw_idx = find_col("ASHP Heater Electric Power (kW)");
            let zone_temp_idx = find_col("Temperature - Indoor (C)");
            let outdoor_idx = find_col("Temperature - Outdoor (C)");
            let heater_mode_idx = find_col("ASHP Heater Mode (-)");
            let heater_setpoint_idx = find_col("ASHP Heater Setpoint (C)");
            eprintln!("\n  === Heater debug (first 10 + last 5 timesteps) ===");
            eprintln!("    heater_kw_col={heater_kw_idx:?} zone_temp_col={zone_temp_idx:?} outdoor_col={outdoor_idx:?} mode_col={heater_mode_idx:?} setpoint_col={heater_setpoint_idx:?}");
            let data_lines: Vec<&str> = csv_lines.filter(|l| !l.trim().is_empty()).collect();
            let get = |line: &str, idx: Option<usize>| -> String {
                idx.and_then(|i| line.split(',').nth(i))
                    .unwrap_or("N/A")
                    .trim()
                    .to_string()
            };
            for (i, line) in data_lines.iter().enumerate() {
                if i < 10 || i >= data_lines.len().saturating_sub(5) {
                    eprintln!(
                        "    t={i:>3} heater_kw={:>8} zone_C={:>8} outdoor_C={:>8} mode={:>4} setpoint_C={:>8}",
                        get(line, heater_kw_idx),
                        get(line, zone_temp_idx),
                        get(line, outdoor_idx),
                        get(line, heater_mode_idx),
                        get(line, heater_setpoint_idx),
                    );
                }
            }
            let _ = fs::remove_file(&csv_path);
        } else {
            eprintln!(
                "  [no output CSV at {} or {}]",
                output_path.display(),
                actual_output.display()
            );
        }
    }

    #[test]
    fn smoke_resstock_1h() {
        let output_path = std::env::temp_dir().join("hares_smoke_resstock_1h.csv");
        let _ = fs::remove_file(&output_path);

        let engine = SimulationEngine::new();
        let config = DwellingConfig {
            hpxml_path: vendor_dir().join("ochre/defaults/Input Files/bldg0112631-up00.xml"),
            schedule_path: vendor_dir().join("ochre/defaults/Input Files/bldg0112631_schedule.csv"),
            weather_path: vendor_dir()
                .join("ochre/defaults/Weather/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
            defaults_path: Some(project_root().join("defaults")),
            sim_config: SimulationConfig {
                start_time: Utc.with_ymd_and_hms(2019, 5, 5, 12, 0, 0).unwrap(),
                duration: Duration::hours(1),
                time_res: Duration::minutes(1),
                output_verbosity: 3,
                output_path: Some(output_path.clone()),
                output_format: OutputFormat::Csv,
                output_chunk_size: 1024,
                setpoint_deadband_c: None,
                master_seed: 42,
            },
            overrides: None,
            bldg_id: 2,
            initialization_duration: None,
        };
        let result = engine.run(config).expect("engine.run should succeed");
        assert!(
            !matches!(result.status, SimStatus::Failed(_)),
            "ResStock simulation failed: {:?}",
            result.status
        );

        eprintln!(
            "[smoke_resstock_1h] status={:?} elapsed={:?}",
            result.status, result.elapsed
        );
        eprintln!("  total kWh: {:.4}", result.metrics.annual_energy_kwh.total);

        if output_path.exists() {
            let breakdown = parse_csv_power_kwh(&output_path, 1.0);
            eprintln!("\n  === ResStock per-equipment kWh (1h) ===");
            for (col, kwh) in &breakdown {
                if !col.contains("[mean_kW]") {
                    eprintln!("    {col:55} {kwh:10.4}");
                }
            }
            let _ = fs::remove_file(&output_path);
        }
    }
}
