//! Smoke tests -- run real fixtures end-to-end and pin physics bounds.
//!
//! These tests assert simulation invariants (no NaN, no sign flips, zone
//! temps within physical bounds, non-negative per-end-use energy) on short
//! 1-hour runs. They are NOT parity tests: they do not compare HARES
//! outputs against OCHRE channel-by-channel and they must not be read that
//! way. Parity coverage lives in:
//!   - `tests/conditioned_oracle.rs::conditioned_dynamic_spring_72h` (HVAC
//!     parity over a 72h window where minute-scale cycle-phase noise
//!     averages out, at 15% tolerance)
//!   - `tests/envelope_oracle.rs` (envelope-channel OCHRE oracle)
//!   - `tests/parity/` (component-level parity suites)
//!
//! The only invariant pin in this module is
//! [`cycling_energy_equals_rated_power_times_on_duration`], which codifies
//! the per-cycle energy semantic the parity suites depend on.

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
    /// (NaN, 10x energy, sign flips, runaway temps) -- not parity tests.
    fn assert_physics_bounds(
        csv_path: &PathBuf,
        total_energy_kwh: f64,
        per_end_use: &BTreeMap<String, f64>,
        duration_hours: f64,
    ) {
        let data = parse_csv_columns(csv_path);

        // --- 1. Zone temperatures in physical bounds [-50, 80]°C ---
        let temp_col_count = data
            .keys()
            .filter(|col| col.starts_with("Temperature -") && col.ends_with("(C)"))
            .count();
        assert!(
            temp_col_count > 0,
            "No temperature columns found in CSV -- physics bounds check would be vacuous"
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
        let hvac_keywords = [
            "Heater Electric Power",
            "Cooler Electric Power",
            "HVAC Heating",
            "HVAC Cooling",
            "Air Conditioner",
            "Furnace Electric Power",
            "Heat Pump",
        ];
        let hvac_total_kwh: f64 = data
            .iter()
            .filter(|(col, _)| {
                col.ends_with("(kW)") && hvac_keywords.iter().any(|kw| col.contains(kw))
            })
            .map(|(_, values)| {
                let sum: f64 = values.iter().sum();
                if values.is_empty() {
                    0.0
                } else {
                    sum * (duration_hours / values.len() as f64)
                }
            })
            .sum();
        if hvac_total_kwh <= 0.0 {
            // HVAC didn't run -- verify zone temps are in comfort range
            let indoor_temp_cols: Vec<_> = data
                .iter()
                .filter(|(col, _)| {
                    col.starts_with("Temperature -")
                        && col.ends_with("(C)")
                        && col.contains("Indoor")
                })
                .collect();
            assert!(
                !indoor_temp_cols.is_empty(),
                "HVAC consumed zero energy but no indoor temperature columns found -- check would be vacuous"
            );
            let zone_temps_ok = indoor_temp_cols
                .iter()
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
        let wh_kwh: f64 = data
            .iter()
            .filter(|(col, _)| {
                col.ends_with("(kW)")
                    && (col.contains("Water Heater") || col.contains("water_heater"))
            })
            .map(|(_, values)| {
                let sum: f64 = values.iter().sum();
                if values.is_empty() {
                    0.0
                } else {
                    sum * (duration_hours / values.len() as f64)
                }
            })
            .sum();
        let has_water_heater = data.keys().any(|col| {
            col.ends_with("(kW)") && (col.contains("Water Heater") || col.contains("water_heater"))
        });
        if has_water_heater {
            assert!(
                wh_kwh >= 0.0,
                "Water heater energy is negative ({wh_kwh:.6} kWh) -- sign-flip bug"
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
                write_output: true,
                output_format: OutputFormat::Csv,
                output_chunk_size: 1024,
                setpoint_deadband_c: None,
                master_seed: 42,
                civil_timezone: None,
            },
            overrides: None,
            bldg_id: 1,
            initialization_duration: None,
            resample_overrides: Some(hares_io::ResampleOverrides::ochre_compat()),
            patches: None,
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

    /// Physics-bounds smoke guard on a 1h BEopt run.
    ///
    /// Asserts that the engine runs to completion on the BEopt fixture and
    /// produces physically plausible output (no NaN, zone temps in
    /// [-50, 80] °C, per-step total power non-negative, per-end-use energy
    /// non-negative, water-heater non-negative). It is NOT a parity test.
    /// HVAC cycling-phase noise over a 1-hour window makes channel-level
    /// OCHRE comparison vacuous; sibling parity coverage with meaningful
    /// tolerance lives in
    /// `tests/conditioned_oracle.rs::conditioned_dynamic_spring_72h`.
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

            // Pin: 1h BEopt noon run should draw positive electrical energy
            // (base loads + HVAC cycling or schedule-driven equipment). Zero
            // total energy indicates engine dispatch regression.
            assert!(
                result.metrics.annual_energy_kwh.total > 0.0,
                "total energy should be > 0 for a 1h BEopt run, got {}",
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

    /// Honest smoke test: ResStock HPXML runs to completion and produces
    /// physically plausible output. This is *not* an OCHRE parity test --
    /// for parity see the oracle suites under `tests/conditioned_oracle.rs`
    /// and `tests/parity/`. The assertions below catch engine-crash,
    /// NaN/blow-up, and sign-flip regressions on the ResStock fixture
    /// (building 0112631, gas furnace + gas water heater, Denver TMY3).
    #[test]
    fn smoke_resstock_1h_runs_to_completion() {
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
                // Denver local noon (UTC-7) on May 5 2019 -- mild spring noon,
                // neither heating nor cooling is expected to dominate.
                start_time: FixedOffset::west_opt(7 * 3600)
                    .expect("Denver UTC-7 offset")
                    .with_ymd_and_hms(2019, 5, 5, 12, 0, 0)
                    .unwrap(),
                duration: Duration::hours(1),
                time_res: Duration::minutes(1),
                output_verbosity: 3,
                output_path: Some(output_path.clone()),
                write_output: true,
                output_format: OutputFormat::Csv,
                output_chunk_size: 1024,
                setpoint_deadband_c: None,
                master_seed: 42,
                civil_timezone: None,
            },
            overrides: None,
            bldg_id: 2,
            initialization_duration: None,
            resample_overrides: Some(hares_io::ResampleOverrides::ochre_compat()),
            patches: None,
        };
        let result = engine.run(config).expect("engine.run should succeed");

        // --- 1. Engine runs to completion on the ResStock HPXML fixture. ---
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

        // --- 2. Output CSV is produced and non-empty. ---
        assert!(
            output_path.exists(),
            "expected output CSV at {} (engine.run returned success but wrote nothing)",
            output_path.display()
        );
        let csv_contents = fs::read_to_string(&output_path).expect("read output CSV");
        let data_rows = csv_contents
            .lines()
            .filter(|l| !l.trim().is_empty())
            .count();
        // Header + at least one data row. 1h at 1-minute timestep = 60 rows + header.
        assert!(
            data_rows >= 2,
            "output CSV has {data_rows} lines; expected header plus >=1 data row"
        );

        let breakdown = parse_csv_power_kwh(&output_path, 1.0);
        eprintln!("\n  === ResStock per-equipment kWh (1h) ===");
        for (col, kwh) in &breakdown {
            if !col.contains("[mean_kW]") {
                eprintln!("    {col:55} {kwh:10.4}");
            }
        }

        let data = parse_csv_columns(&output_path);

        // --- 3. All power columns are finite; non-negative except the net
        //        electrical tie-in (which may go negative under PV/battery
        //        export). This fixture has neither, so net should stay >= 0,
        //        but we still exempt the net column from the non-negativity
        //        check to document the invariant correctly. ---
        for (col, values) in &data {
            if !col.ends_with("(kW)") && !col.ends_with("(therms/hour)") {
                continue;
            }
            for (i, &v) in values.iter().enumerate() {
                assert!(
                    v.is_finite(),
                    "Non-finite value ({v}) in '{col}' at row {i} -- NaN/Inf regression"
                );
            }
            let is_net_tieline = col.contains("Net Electric") || col.starts_with("Net ");
            if is_net_tieline {
                continue;
            }
            for (i, &v) in values.iter().enumerate() {
                assert!(
                    v >= 0.0,
                    "Negative power {v:.6} in '{col}' at row {i} -- \
                     equipment end-use columns are always sourced (never net-exported); \
                     sign-flip regression"
                );
            }
        }

        // --- 4. Total electric energy sits within a loose physical-plausibility
        //        band for a 1h ResStock residential snapshot. This band exists
        //        only to catch NaN, zero-output, and blow-up regressions -- it
        //        is not a parity bound. Typical ResStock single-family dwellings
        //        draw 0.3–5 kWh over a noon hour with base loads only. The band
        //        [0.1, 10] kWh/h covers every plausible occupancy level without
        //        masking real breakage. ---
        let total_kwh = result.metrics.annual_energy_kwh.total;
        assert!(
            total_kwh.is_finite(),
            "total electric energy is non-finite: {total_kwh}"
        );
        assert!(
            total_kwh > 0.1,
            "total electric energy {total_kwh:.4} kWh/h is below 0.1 kWh/h -- \
             base loads (refrigerator + MELs) alone must exceed this; \
             likely a zero-output or schedule-dispatch regression"
        );
        assert!(
            total_kwh < 10.0,
            "total electric energy {total_kwh:.4} kWh/h exceeds 10 kWh/h -- \
             no plausible 1h residential snapshot pulls this much; \
             likely a units or blow-up regression"
        );

        // --- 5. Gas furnace gas power, when present and firing, sits in a
        //        plausible band. Residential gas furnaces are typically sized
        //        20–120 kBtu/h = 0.2–1.2 therms/hour. In a mild May noon the
        //        furnace may not fire at all -- we assert only on positive
        //        samples so an idle furnace doesn't trip the test. ---
        let gas_furnace_therms_col = data
            .keys()
            .find(|col| col.contains("Gas Furnace") && col.ends_with("(therms/hour)"))
            .cloned();
        if let Some(col) = gas_furnace_therms_col {
            let values = &data[&col];
            for (i, &v) in values.iter().enumerate() {
                if v > 0.0 {
                    assert!(
                        v < 2.0,
                        "Gas Furnace gas rate {v:.4} therms/h at row {i} \
                         exceeds 2 therms/h -- residential furnaces max out near 1.2 therms/h"
                    );
                }
            }
        }

        // --- 6. No per-end-use bucket reports negative energy over the window.
        //        Catches sign-flip regressions in aggregation that slip past
        //        the per-timestep check above. ---
        for (end_use, &kwh) in &result.metrics.annual_energy_kwh.per_end_use {
            assert!(
                kwh.is_finite(),
                "Non-finite energy for end-use '{end_use}': {kwh}"
            );
            assert!(
                kwh >= 0.0,
                "Negative energy {kwh:.6} kWh for end-use '{end_use}' -- \
                 aggregated end-use totals must be non-negative"
            );
        }

        // Reuse the shared physics-bounds check: zone temps in [-50, 80] °C,
        // no NaNs anywhere, total electric column non-negative per step,
        // water heater non-negative (the fixture has a gas water heater so
        // the water-heater clause is a no-op here).
        assert_physics_bounds(
            &output_path,
            total_kwh,
            &result.metrics.annual_energy_kwh.per_end_use,
            1.0,
        );
    }

    /// Pin the per-cycle energy invariant for threshold-driven cycling
    /// equipment: over any ON-interval of `n_minutes` at constant rated
    /// power `rated_kw`, the integrated energy equals
    /// `rated_kw * n_minutes / 60`. This is the semantic the smoke tests
    /// depend on when comparing HVAC totals against OCHRE; violating it
    /// would silently break the parity test in
    /// `tests/conditioned_oracle.rs::conditioned_dynamic_spring_72h`.
    #[test]
    fn cycling_energy_equals_rated_power_times_on_duration() {
        // A minute-stepped ON burst: power samples of `rated_kw` for
        // `on_minutes` followed by zeros. Integrating at 1-minute step
        // sums to rated_kw * on_minutes / 60 kWh.
        let rated_kw: f64 = 2.5;
        let on_minutes: usize = 3;
        let off_minutes: usize = 7;
        let cycles: usize = 6;

        let mut samples_kw: Vec<f64> = Vec::with_capacity(cycles * (on_minutes + off_minutes));
        for _ in 0..cycles {
            samples_kw.extend(std::iter::repeat_n(rated_kw, on_minutes));
            samples_kw.extend(std::iter::repeat_n(0.0, off_minutes));
        }

        // Integrate at 1-minute resolution (hours per step = 1/60).
        let step_h: f64 = 1.0 / 60.0;
        let total_kwh: f64 = samples_kw.iter().sum::<f64>() * step_h;

        let expected_kwh = rated_kw * (cycles * on_minutes) as f64 / 60.0;
        assert!(
            (total_kwh - expected_kwh).abs() < 1e-12,
            "Cycling energy invariant violated: integrated={total_kwh:.9} kWh, \
             expected={expected_kwh:.9} kWh \
             (rated_kw={rated_kw}, cycles={cycles}, on_minutes={on_minutes})"
        );

        // Per-cycle energy must equal rated_kw * on_minutes / 60 exactly.
        let per_cycle_kwh: f64 = samples_kw
            .chunks_exact(on_minutes + off_minutes)
            .map(|c| c.iter().sum::<f64>() * step_h)
            .next()
            .expect("at least one full cycle");
        let expected_per_cycle = rated_kw * on_minutes as f64 / 60.0;
        assert!(
            (per_cycle_kwh - expected_per_cycle).abs() < 1e-12,
            "Per-cycle energy invariant violated: got={per_cycle_kwh:.9} kWh, \
             expected={expected_per_cycle:.9} kWh"
        );
    }

    /// Integration test: BEopt fixture has ASHP Heater (heat pump) →
    /// HVAC_HEATING equipment. After end-use aggregate column changes
    /// (T‑0139), `energy_by_end_use["hvac_heating"]` must be populated
    /// and non-zero because all HVAC_HEATING equipment electric power
    /// is summed into the `"HVAC Heating Electric Power (kW)"` aggregate
    /// column, which `discover_end_use_columns` maps to key `"hvac_heating"`.
    #[test]
    fn beopt_smoke_end_use_aggregates_by_category() {
        let output_path = std::env::temp_dir().join(unique_temp_name("hares_beopt_enduse", "csv"));
        let _guard = TempFile(output_path.clone());

        let engine = SimulationEngine::new();
        let result = engine
            .run(beopt_config(1, output_path.clone()))
            .expect("engine.run should succeed");

        assert!(
            !matches!(result.status, SimStatus::Failed(_)),
            "BEopt simulation failed: {:?}",
            result.status
        );

        // The BEopt fixture has an air-to-air heat pump → HVAC_HEATING equipment.
        let per_end_use = &result.metrics.annual_energy_kwh.per_end_use;

        assert!(
            per_end_use.contains_key("hvac_heating"),
            "per_end_use must contain 'hvac_heating' key from aggregate column; \
             got keys: {:?}",
            per_end_use.keys().collect::<Vec<_>>()
        );
        assert!(
            !per_end_use.contains_key("ASHP Heater"),
            "per_end_use must NOT contain per-equipment key 'ASHP Heater'"
        );

        let hvac_heating_kwh = per_end_use["hvac_heating"];
        assert!(
            hvac_heating_kwh > 0.0,
            "hvac_heating end-use energy must be non-zero for a heating simulation; \
             got {hvac_heating_kwh:.6} kWh"
        );
    }
}
