//! Smoke test — runs real OCHRE vendor fixtures through HARES and prints
//! per-equipment power breakdown for comparison against OCHRE reference.

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;

    use chrono::{Duration, FixedOffset, TimeZone};
    use hares_core::{DwellingConfig, SimStatus, SimulationConfig, SimulationEngine};
    use hares_io::OutputFormat;

    fn unique_temp_name(base: &str, ext: &str) -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock before epoch")
            .as_nanos();
        let tid = std::thread::current().id();
        format!("{base}_{nanos}_{tid:?}.{ext}")
    }

    struct TempFile(std::path::PathBuf);
    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// Parse a CSV into column-name -> Vec<f64> map.
    /// Non-numeric cells (headers, timestamps) are silently skipped per-cell.
    fn parse_csv_columns(path: &PathBuf) -> BTreeMap<String, Vec<f64>> {
        let contents = fs::read_to_string(path).expect("read CSV");
        let mut lines = contents.lines();
        let header = lines.next().expect("header line");
        let columns: Vec<String> = header.split(',').map(|c| c.trim().to_string()).collect();
        let mut data: BTreeMap<String, Vec<f64>> = BTreeMap::new();
        for col in &columns {
            data.insert(col.clone(), Vec::new());
        }
        for line in lines {
            if line.trim().is_empty() {
                continue;
            }
            let fields: Vec<&str> = line.split(',').collect();
            for (i, field) in fields.iter().enumerate() {
                if i < columns.len() {
                    if let Ok(v) = field.trim().parse::<f64>() {
                        data.get_mut(&columns[i]).unwrap().push(v);
                    }
                }
            }
        }
        data
    }

    /// Physics-validated sanity checks on CSV output and metrics.
    ///
    /// These are wide-tolerance bounds designed to catch gross errors
    /// (NaN, 10x energy, sign flips, runaway temps) — not parity tests.
    fn assert_physics_bounds(
        csv_path: &PathBuf,
        total_energy_kwh: f64,
        per_end_use: &BTreeMap<String, f64>,
        duration_hours: f64,
    ) {
        let data = parse_csv_columns(csv_path);

        // --- 1. Zone temperatures in physical bounds [-50, 80]°C ---
        let temp_col_count = data.keys()
            .filter(|col| col.starts_with("Temperature -") && col.ends_with("(C)"))
            .count();
        assert!(
            temp_col_count > 0,
            "No temperature columns found in CSV — physics bounds check would be vacuous"
        );
        for (col, values) in &data {
            if col.starts_with("Temperature -") && col.ends_with("(C)") {
                for (i, &v) in values.iter().enumerate() {
                    assert!(
                        v > -50.0 && v < 80.0,
                        "Zone temp {v:.1}°C outside physical bounds [-50, 80] in '{col}' at row {i}"
                    );
                }
            }
        }

        // --- 2. No NaN values in any numeric column ---
        for (col, values) in &data {
            for (i, &v) in values.iter().enumerate() {
                assert!(
                    v.is_finite(),
                    "Non-finite value ({v}) detected in '{col}' at row {i}"
                );
            }
        }

        // --- 3. HVAC energy non-zero OR zone temps in comfort range ---
        // In mild weather (spring/fall) HVAC may legitimately not run.
        // Assert: either HVAC consumed energy, or zone temps stayed in [15, 30]°C
        // (meaning the dwelling was comfortable without conditioning).
        let hvac_keywords = ["Heater Electric Power", "Cooler Electric Power",
                             "HVAC Heating", "HVAC Cooling", "Air Conditioner",
                             "Furnace Electric Power", "Heat Pump"];
        let hvac_total_kwh: f64 = data.iter()
            .filter(|(col, _)| {
                col.ends_with("(kW)") && hvac_keywords.iter().any(|kw| col.contains(kw))
            })
            .map(|(_, values)| {
                let sum: f64 = values.iter().sum();
                if values.is_empty() { 0.0 } else { sum * (duration_hours / values.len() as f64) }
            })
            .sum();
        if hvac_total_kwh <= 0.0 {
            // HVAC didn't run — verify zone temps are in comfort range
            let indoor_temp_cols: Vec<_> = data.iter()
                .filter(|(col, _)| col.starts_with("Temperature -") && col.ends_with("(C)") && col.contains("Indoor"))
                .collect();
            assert!(
                !indoor_temp_cols.is_empty(),
                "HVAC consumed zero energy but no indoor temperature columns found — check would be vacuous"
            );
            let zone_temps_ok = indoor_temp_cols.iter()
                .all(|(_, values)| values.iter().all(|&v| v > 15.0 && v < 30.0));
            assert!(
                zone_temps_ok,
                "HVAC consumed zero energy AND zone temps are outside comfort range [15, 30]°C"
            );
        }

        // --- 4. Electrical consumption in reasonable range [0, 50] kWh/h ---
        assert!(
            total_energy_kwh >= 0.0,
            "Total electrical energy is negative: {total_energy_kwh:.4} kWh"
        );
        let max_reasonable_kwh = 50.0 * duration_hours;
        assert!(
            total_energy_kwh <= max_reasonable_kwh,
            "Total electrical energy {total_energy_kwh:.4} kWh exceeds reasonable bound \
             of {max_reasonable_kwh:.1} kWh for {duration_hours}h simulation"
        );

        // Also check per-end-use values are non-negative and finite
        for (end_use, &kwh) in per_end_use {
            assert!(
                kwh.is_finite(),
                "Non-finite energy for end-use '{end_use}': {kwh}"
            );
            assert!(
                kwh >= 0.0,
                "Negative energy for end-use '{end_use}': {kwh:.4} kWh"
            );
        }

        // --- 5. Water heater energy is non-negative if present ---
        // A water heater may not cycle in a short (1h) window, so we only
        // assert non-negative (catching sign-flip bugs), not strictly positive.
        let wh_kwh: f64 = data.iter()
            .filter(|(col, _)| {
                col.ends_with("(kW)") && (col.contains("Water Heater") || col.contains("water_heater"))
            })
            .map(|(_, values)| {
                let sum: f64 = values.iter().sum();
                if values.is_empty() { 0.0 } else { sum * (duration_hours / values.len() as f64) }
            })
            .sum();
        let has_water_heater = data.keys().any(|col| {
            col.ends_with("(kW)") && (col.contains("Water Heater") || col.contains("water_heater"))
        });
        if has_water_heater {
            assert!(
                wh_kwh >= 0.0,
                "Water heater energy is negative ({wh_kwh:.6} kWh) — sign-flip bug"
            );
        }

        // --- Verify total power column values are non-negative per-step ---
        if let Some(total_power) = data.get("Total Electric Power (kW)") {
            for (i, &v) in total_power.iter().enumerate() {
                assert!(
                    v >= 0.0,
                    "Negative total electric power {v:.4} kW at row {i}"
                );
            }
        }

        eprintln!("  [physics bounds] all assertions passed");
        eprintln!(
            "    total_energy={total_energy_kwh:.4} kWh, hvac_energy={hvac_total_kwh:.4} kWh, \
             water_heater={wh_kwh:.4} kWh (present={has_water_heater})"
        );
    }

    fn examples_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/examples")
    }

    fn project_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn beopt_config(duration_hours: i64, output_path: PathBuf) -> DwellingConfig {
        DwellingConfig {
            hpxml_path: examples_dir().join("BEopt_example.xml"),
            schedule_path: examples_dir().join("BEopt_example_schedule.csv"),
            weather_path: examples_dir().join("USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
            defaults_path: Some(project_root().join("defaults")),
            sim_config: SimulationConfig {
                // Match OCHRE smoke: May 5, 2019, 12:00 PM Denver (UTC-7)
                start_time: FixedOffset::west_opt(7 * 3600)
                    .expect("Denver UTC-7 offset")
                    .with_ymd_and_hms(2019, 5, 5, 12, 0, 0)
                    .unwrap(),
                duration: Duration::hours(duration_hours),
                time_res: Duration::minutes(1),
                output_verbosity: 3,
                output_path: Some(output_path),
                output_format: OutputFormat::Csv,
                output_chunk_size: 1024,
                setpoint_deadband_c: None,
                master_seed: 42,
                civil_timezone: None,
            },
            overrides: None,
            bldg_id: 1,
            initialization_duration: None,
            resample_overrides: None,
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
        let output_path =
            std::env::temp_dir().join(unique_temp_name("hares_smoke_beopt_1h", "csv"));
        let _guard = TempFile(output_path.clone());

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

            // OCHRE reference values (from OCHRE 0.9.2, same inputs, same time window).
            // Column names differ between OCHRE and HARES for HVAC equipment:
            //   OCHRE: "HVAC Heating Electric Power (kW)"
            //   HARES: "ASHP Heater Electric Power (kW)" (equipment-specific naming)
            // The aliases list maps OCHRE names to possible HARES column names.
            eprintln!("\n  === OCHRE reference (kWh) ===");
            let ochre_ref: &[(&str, f64, &[&str])] = &[
                ("Total Electric Power (kW)", 1.3395, &[]),
                (
                    "HVAC Heating Electric Power (kW)",
                    0.9113,
                    &[
                        "ASHP Heater Electric Power (kW)",
                        "MSHP Heater Electric Power (kW)",
                        "Gas Furnace Electric Power (kW)",
                        "Electric Furnace Electric Power (kW)",
                    ],
                ),
                (
                    "HVAC Cooling Electric Power (kW)",
                    0.0500,
                    &[
                        "ASHP Cooler Electric Power (kW)",
                        "MSHP Cooler Electric Power (kW)",
                        "Air Conditioner Electric Power (kW)",
                        "Room AC Electric Power (kW)",
                    ],
                ),
                (
                    "Indoor Lighting Electric Power (kW)",
                    0.0879,
                    &["Indoor Lighting Electric Power (kW)"],
                ),
                (
                    "Exterior Lighting Electric Power (kW)",
                    0.0064,
                    &["Exterior Lighting Electric Power (kW)"],
                ),
                (
                    "MELs Electric Power (kW)",
                    0.1340,
                    &["MELs Electric Power (kW)"],
                ),
                (
                    "TV Electric Power (kW)",
                    0.0761,
                    &["TV Electric Power (kW)"],
                ),
                (
                    "Refrigerator Electric Power (kW)",
                    0.0540,
                    &["Refrigerator Electric Power (kW)"],
                ),
                (
                    "Ventilation Fan Electric Power (kW)",
                    0.0199,
                    &["Ventilation Fan Electric Power (kW)"],
                ),
            ];
            for (ochre_name, ochre_kwh, aliases) in ochre_ref {
                // Try OCHRE name first, then each alias; sum all matches
                // (e.g., HVAC Heating could be split across heater + aux)
                let hares_kwh = if let Some(&v) = breakdown.get(*ochre_name) {
                    v
                } else {
                    let sum: f64 = aliases.iter().filter_map(|a| breakdown.get(*a)).sum();
                    if aliases.iter().any(|a| breakdown.contains_key(*a)) {
                        sum
                    } else {
                        f64::NAN
                    }
                };
                let ochre_val: f64 = *ochre_kwh;
                let diff_pct = if ochre_val.abs() > 1e-9 {
                    (hares_kwh - ochre_val) / ochre_val * 100.0
                } else if hares_kwh.abs() > 1e-9 {
                    f64::INFINITY
                } else {
                    0.0
                };
                let hares_col = if breakdown.contains_key(*ochre_name) {
                    ochre_name.to_string()
                } else {
                    aliases
                        .iter()
                        .find(|a| breakdown.contains_key(**a))
                        .map(|a| a.to_string())
                        .unwrap_or_else(|| "???".to_string())
                };
                eprintln!(
                    "    {ochre_name:55} OCHRE={ochre_val:8.4}  HARES={hares_kwh:8.4}  diff={diff_pct:+7.1}%  [{hares_col}]"
                );

                // Assert OCHRE parity for non-trivial values.
                // HVAC and total power are physics-critical (±30%).
                // Schedule-based loads (lighting, ventilation, MELs) depend on
                // exact hour alignment and can diverge in a 1-hour window.
                if ochre_val.abs() > 0.01 && !hares_kwh.is_nan() {
                    let is_physics_critical = ochre_name.contains("HVAC")
                        || ochre_name.contains("Total");
                    let tolerance = if is_physics_critical { 30.0 } else { 300.0 };
                    assert!(
                        diff_pct.abs() < tolerance,
                        "OCHRE parity failure: {ochre_name}: \
                         OCHRE={ochre_val:.4} HARES={hares_kwh:.4} diff={diff_pct:+.1}% \
                         (tolerance={tolerance}%)"
                    );
                }
            }

            // Total energy should be positive for a January heating simulation
            assert!(
                result.metrics.annual_energy_kwh.total > 0.0,
                "total energy should be > 0 for a heating simulation, got {}",
                result.metrics.annual_energy_kwh.total
            );

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
            eprintln!(
                "    heater_kw_col={heater_kw_idx:?} zone_temp_col={zone_temp_idx:?} outdoor_col={outdoor_idx:?} mode_col={heater_mode_idx:?} setpoint_col={heater_setpoint_idx:?}"
            );
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
            // --- Physics-validated sanity checks ---
            assert_physics_bounds(
                &csv_path,
                result.metrics.annual_energy_kwh.total,
                &result.metrics.annual_energy_kwh.per_end_use,
                1.0,
            );
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
        let output_path =
            std::env::temp_dir().join(unique_temp_name("hares_smoke_resstock_1h", "csv"));
        let _guard = TempFile(output_path.clone());

        let engine = SimulationEngine::new();
        let config = DwellingConfig {
            hpxml_path: examples_dir().join("bldg0112631-up00.xml"),
            schedule_path: examples_dir().join("bldg0112631_schedule.csv"),
            weather_path: examples_dir().join("USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
            defaults_path: Some(project_root().join("defaults")),
            sim_config: SimulationConfig {
                // Denver local noon (UTC-7)
                start_time: FixedOffset::west_opt(7 * 3600)
                    .expect("Denver UTC-7 offset")
                    .with_ymd_and_hms(2019, 5, 5, 12, 0, 0)
                    .unwrap(),
                duration: Duration::hours(1),
                time_res: Duration::minutes(1),
                output_verbosity: 3,
                output_path: Some(output_path.clone()),
                output_format: OutputFormat::Csv,
                output_chunk_size: 1024,
                setpoint_deadband_c: None,
                master_seed: 42,
                civil_timezone: None,
            },
            overrides: None,
            bldg_id: 2,
            initialization_duration: None,
            resample_overrides: None,
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

        // A 1-hour January Denver simulation must consume some energy
        assert!(
            result.metrics.annual_energy_kwh.total > 0.0,
            "ResStock total energy should be > 0, got {}",
            result.metrics.annual_energy_kwh.total
        );

        if output_path.exists() {
            let breakdown = parse_csv_power_kwh(&output_path, 1.0);
            eprintln!("\n  === ResStock per-equipment kWh (1h) ===");
            for (col, kwh) in &breakdown {
                if !col.contains("[mean_kW]") {
                    eprintln!("    {col:55} {kwh:10.4}");
                }
            }

            // --- Physics-validated sanity checks ---
            assert_physics_bounds(
                &output_path,
                result.metrics.annual_energy_kwh.total,
                &result.metrics.annual_energy_kwh.per_end_use,
                1.0,
            );
        }
    }
}
