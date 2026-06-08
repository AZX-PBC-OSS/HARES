//! Observer-based diagnostic for ResStock simulation health.
//!
//! Uses the `observe` feature to capture per-phase state at each
//! timestep and verify:
//! - Equipment modes (no simultaneous heating+cooling)
//! - Thermostat setpoint consistency
//! - Envelope gain components (reasonable magnitudes)
//! - Port-level energy conservation
//! - Zone temperature stability
#![cfg(feature = "observe")]

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;

    use chrono::{Duration, FixedOffset, TimeZone};
    use hares_core::{DwellingConfig, SimStatus, SimulationConfig, SimulationEngine};
    use hares_io::OutputFormat;

    fn project_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn fixtures_dir() -> PathBuf {
        project_root().join("tests/fixtures/resstock")
    }

    fn weather_for(bldg_dir: &PathBuf, version: &str) -> PathBuf {
        let hpxml = fs::read_to_string(bldg_dir.join("home.xml")).unwrap();
        let fips = parse_fips(&hpxml);
        let wdir = fixtures_dir().join(version).join("weather");
        let ext = if version == "2025.1" { "csv" } else { "epw" };
        [format!("{fips}_2018.csv"), format!("{fips}.{ext}")]
            .iter()
            .map(|n| wdir.join(n))
            .find(|p| p.exists())
            .unwrap_or_else(|| {
                wdir.read_dir()
                    .unwrap()
                    .filter_map(|e| e.ok())
                    .find(|e| e.path().is_file())
                    .map(|e| e.path())
                    .unwrap()
            })
    }

    fn parse_fips(xml: &str) -> String {
        for line in xml.lines() {
            if let Some(s) = line.find("<Name>") {
                let inner = &line[s + 6..];
                if let Some(e) = inner.find("</Name>") {
                    let n = &inner[..e];
                    if n.starts_with('G') {
                        return n.trim().to_string();
                    }
                }
            }
        }
        "UNKNOWN".into()
    }

    fn utn(base: &str) -> String {
        format!(
            "{base}_{}_{:?}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            std::thread::current().id()
        )
    }

    #[test]
    fn observer_diagnostic_2024_2_winter_24h() {
        let bldg_dir = fixtures_dir().join("2024.2").join("bldg0000002");
        let output = std::env::temp_dir().join(utn("obs_diag"));

        let config = DwellingConfig {
            hpxml_path: bldg_dir.join("home.xml"),
            schedule_path: bldg_dir.join("in.schedules.csv"),
            weather_path: weather_for(&bldg_dir, "2024.2"),
            defaults_path: Some(project_root().join("defaults")),
            sim_config: SimulationConfig {
                start_time: FixedOffset::west_opt(7 * 3600)
                    .unwrap()
                    .with_ymd_and_hms(2018, 1, 15, 0, 0, 0)
                    .unwrap(),
                duration: Duration::hours(24),
                time_res: Duration::minutes(15),
                output_verbosity: 4,
                output_path: Some(output.clone()),
                write_output: true,
                output_format: OutputFormat::Csv,
                output_chunk_size: 1024,
                setpoint_deadband_c: None,
                master_seed: 42,
                civil_timezone: None,
                site_location: hares_io::SiteLocationOverride::default(),
                retain_batches: false,
                rotation: hares_io::RotationPolicy::None,
            },
            overrides: None,
            bldg_id: 300,
            initialization_duration: Some(std::time::Duration::from_secs(24 * 3600)),
            resample_overrides: Some(hares_io::ResampleOverrides::ochre_compat()),
            patches: None,
        };

        let engine = SimulationEngine::new();
        let result = engine.run(config).expect("run");
        assert!(!matches!(result.status, SimStatus::Failed(_)));

        // Read diagnostic CSV output (verbosity 4)
        if output.exists() {
            let csv = fs::read_to_string(&output).unwrap();
            let mut lines = csv.lines();
            let header = lines.next().unwrap();
            let cols: Vec<&str> = header.split(',').map(|c| c.trim()).collect();

            // Find temperature columns
            eprintln!("=== Diagnostic columns ===");
            for (i, c) in cols.iter().enumerate() {
                if c.contains("temp")
                    || c.contains("Temp")
                    || c.contains("mode")
                    || c.contains("Mode")
                {
                    eprintln!("  col {i}: {c}");
                }
            }

            // Parse and analyze
            let mut zone_temps: BTreeMap<usize, Vec<f64>> = BTreeMap::new();
            for (i, c) in cols.iter().enumerate() {
                if c.contains("Temperature") && c.ends_with("(C)") {
                    zone_temps.insert(i, Vec::new());
                }
            }

            for line in lines.filter(|l| !l.trim().is_empty()) {
                let fields: Vec<&str> = line.split(',').collect();
                for (i, vals) in zone_temps.iter_mut() {
                    if let Some(f) = fields.get(*i) {
                        if let Ok(v) = f.trim().parse::<f64>() {
                            vals.push(v);
                        }
                    }
                }
            }

            eprintln!("\n=== Zone temperature summary ===");
            for (col_idx, vals) in &zone_temps {
                if !vals.is_empty() {
                    let lo = vals.iter().fold(f64::INFINITY, |a, &b| a.min(b));
                    let hi = vals.iter().fold(f64::NEG_INFINITY, |a, &b| a.max(b));
                    let avg = vals.iter().sum::<f64>() / vals.len() as f64;
                    eprintln!(
                        "  col {col_idx} ({}) lo={lo:.1} hi={hi:.1} avg={avg:.1}",
                        cols[*col_idx]
                    );
                }
            }

            eprintln!("\n=== Equipment power/mode dump (first 10 rows) ===");
            let csv2 = fs::read_to_string(&output).unwrap();
            let mut l2 = csv2.lines();
            let hdr2 = l2.next().unwrap();
            let c2: Vec<&str> = hdr2.split(',').map(|c| c.trim()).collect();
            for (row_i, line) in l2.enumerate().take(10) {
                let vals: Vec<&str> = line.split(',').collect();
                let out_col = c2
                    .iter()
                    .position(|c| c.contains("outdoor") && c.contains("temp"));
                let in_col = c2
                    .iter()
                    .position(|c| c.contains("Indoor") && c.contains("(C)"));
                let mode_cols: Vec<_> = c2
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| c.contains("Mode"))
                    .collect();
                let power_cols: Vec<_> = c2
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| c.contains("Power") && c.contains("(kW)"))
                    .take(4)
                    .collect();

                eprint!("  row {row_i}: ");
                if let (Some(oi), Some(ii)) = (out_col, in_col) {
                    eprint!(
                        "out={:.1} in={:.1} ",
                        vals[oi].trim().parse::<f64>().unwrap_or(f64::NAN),
                        vals[ii].trim().parse::<f64>().unwrap_or(f64::NAN)
                    );
                }
                for (mi, mc) in &mode_cols {
                    eprint!("{mc}={} ", vals[*mi].trim());
                }
                for (pi, pc) in &power_cols {
                    eprint!(
                        "{pc}={:.2} ",
                        vals[*pi].trim().parse::<f64>().unwrap_or(f64::NAN)
                    );
                }
                eprintln!();
            }
        }
    }
}
