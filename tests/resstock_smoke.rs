//! ResStock integration smoke tests.
//!
//! Runs short (1h) simulations on real ResStock HPXML files from both
//! 2024.2 (TMY3) and 2025.1 (AMY 2018) releases and verifies that the
//! engine completes without error, produces physically plausible output,
//! and that zone temperatures stay within physical bounds.
//!
//! Fixtures are stored in tests/fixtures/resstock/{version}/ and were
//! downloaded from the NREL OEDI data lake.

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::io::Read;
    use std::path::{Path, PathBuf};

    use chrono::{Duration, FixedOffset, TimeZone};
    use hares_core::{DwellingConfig, SimStatus, SimulationConfig, SimulationEngine};
    use hares_io::OutputFormat;
    use sha2::{Digest, Sha256};

    fn project_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn fixture_building_dirs(version: &str) -> Vec<PathBuf> {
        let base = project_root().join("tests/fixtures/resstock").join(version);
        let mut dirs: Vec<_> = std::fs::read_dir(&base)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_type().map(|t| t.is_dir()).unwrap_or(false)
                    && e.path().join("home.xml").exists()
            })
            .map(|e| e.path())
            .collect();
        dirs.sort();
        dirs
    }

    fn weather_path(version: &str, bldg_dir: &Path) -> PathBuf {
        let hpxml = fs::read_to_string(bldg_dir.join("home.xml")).unwrap();
        let fips = parse_fips_from_hpxml(&hpxml);

        let weather_dir = project_root()
            .join("tests/fixtures/resstock")
            .join(version)
            .join("weather");

        if version == "2025.1" {
            let csv = weather_dir.join(format!("{fips}_2018.csv"));
            if csv.exists() {
                return csv;
            }
        }

        let epw = weather_dir.join(format!("{fips}.epw"));
        if epw.exists() {
            return epw;
        }

        weather_dir
            .read_dir()
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| e.path().extension().map(|x| x == "epw").unwrap_or(false))
            .map(|e| e.path())
            .unwrap_or_else(|| panic!("no weather file found in {weather_dir:?}"))
    }

    fn parse_fips_from_hpxml(xml: &str) -> String {
        for line in xml.lines() {
            if let Some(start) = line.find("<Name>") {
                let inner = &line[start + 6..];
                if let Some(end) = inner.find("</Name>") {
                    let name = &inner[..end];
                    if name.starts_with('G') {
                        return name.trim().to_string();
                    }
                }
            }
        }
        panic!("could not find FIPS weather station in HPXML");
    }

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

    fn parse_csv_columns(path: &Path) -> BTreeMap<String, Vec<f64>> {
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

    fn assert_physics_bounds(csv_path: &Path) {
        let data = parse_csv_columns(csv_path);

        for (col, values) in &data {
            if col.starts_with("Temperature -") && col.ends_with("(C)") {
                for (i, &v) in values.iter().enumerate() {
                    assert!(
                        v > -50.0 && v < 80.0,
                        "Zone temp {v:.1}°C outside physical bounds in '{col}' at row {i}"
                    );
                }
            }
        }

        for (col, values) in &data {
            for (i, &v) in values.iter().enumerate() {
                assert!(
                    v.is_finite(),
                    "Non-finite value ({v}) in '{col}' at row {i}"
                );
            }
        }
    }

    fn run_resstock_smoke(version: &str, bldg_dir: &Path) {
        let hpxml_path = bldg_dir.join("home.xml");
        let schedule_path = bldg_dir.join("in.schedules.csv");
        let weather_path = weather_path(version, bldg_dir);
        let output_path =
            std::env::temp_dir().join(unique_temp_name("hares_resstock_smoke", "csv"));
        let _guard = TempFile(output_path.clone());

        let bldg_name = bldg_dir.file_name().unwrap().to_str().unwrap();

        let engine = SimulationEngine::new();
        let config = DwellingConfig {
            hpxml_path,
            schedule_path,
            weather_path,
            defaults_path: Some(project_root().join("defaults")),
            sim_config: SimulationConfig {
                start_time: FixedOffset::west_opt(7 * 3600)
                    .expect("UTC-7 offset")
                    .with_ymd_and_hms(2019, 5, 5, 12, 0, 0)
                    .unwrap(),
                duration: Duration::hours(1),
                time_res: Duration::minutes(1),
                output_verbosity: 1,
                output_path: Some(output_path.clone()),
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
            bldg_id: 100,
            initialization_duration: None,
            resample_overrides: Some(hares_io::ResampleOverrides::ochre_compat()),
            patches: None,
        };

        let result = engine.run(config).expect("engine.run should succeed");

        assert!(
            !matches!(result.status, SimStatus::Failed(_)),
            "[{version}] {bldg_name} simulation failed: {:?}",
            result.status
        );

        eprintln!(
            "[{version}] {bldg_name} OK — status={:?}, elapsed={:?}, energy={:.4} kWh",
            result.status, result.elapsed, result.metrics.total_energy_kwh.net_energy_kwh
        );

        // Net energy is signed by definition: consumption minus PV
        // generation. A PV building exporting more than it consumes over a
        // midday hour (e.g. a 5 kW array against ~1 kWh of load) is
        // physically correct, so a negative net is allowed only when
        // generation actually occurred. The gross terms are non-negative by
        // definition, and the net must equal their difference.
        let energy = &result.metrics.total_energy_kwh;
        assert!(
            energy.gross_consumption_kwh >= 0.0 && energy.gross_pv_generation_kwh >= 0.0,
            "[{version}] {bldg_name} gross energy terms must be non-negative: \
             consumption={:.4} kWh, pv={:.4} kWh",
            energy.gross_consumption_kwh,
            energy.gross_pv_generation_kwh
        );
        assert!(
            (energy.net_energy_kwh
                - (energy.gross_consumption_kwh - energy.gross_pv_generation_kwh))
                .abs()
                < 1e-9,
            "[{version}] {bldg_name} net energy must equal consumption minus \
             generation: net={:.4}, consumption={:.4}, pv={:.4} kWh",
            energy.net_energy_kwh,
            energy.gross_consumption_kwh,
            energy.gross_pv_generation_kwh
        );
        assert!(
            energy.net_energy_kwh >= 0.0 || energy.gross_pv_generation_kwh > 0.0,
            "[{version}] {bldg_name} negative net energy without PV generation \
             is unphysical: net={:.4} kWh",
            energy.net_energy_kwh
        );

        if output_path.exists() {
            assert_physics_bounds(&output_path);
        }
    }

    fn compute_sha256_hex(file_path: &Path) -> String {
        let mut file = fs::File::open(file_path).expect("open fixture file");
        let mut hasher = Sha256::new();
        let mut buf = [0u8; 65536];
        loop {
            let n = file.read(&mut buf).expect("read fixture file");
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        format!("{:x}", hasher.finalize())
    }

    #[test]
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    fn manifest_invariant_fixtures_match_hashes() {
        let fixtures_root = project_root().join("tests/fixtures/resstock");
        let manifest_path = fixtures_root.join("manifest.sha256");
        let manifest =
            fs::read_to_string(&manifest_path).expect("manifest.sha256 must exist and be readable");

        let mut verified = 0usize;
        for (line_no, line) in manifest.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (expected_hash, rel_path) = line.split_once("  ").unwrap_or_else(|| {
                panic!("line {}: invalid manifest format: {line:?}", line_no + 1)
            });

            let file_path = fixtures_root.join(rel_path);
            assert!(
                file_path.exists(),
                "manifest line {}: fixture file missing at {rel_path}",
                line_no + 1,
            );

            let actual_hash = compute_sha256_hex(&file_path);
            assert_eq!(
                actual_hash,
                expected_hash,
                "manifest line {}: SHA256 mismatch for {rel_path}\n  expected: {expected_hash}\n  actual:   {actual_hash}",
                line_no + 1,
            );
            verified += 1;
        }
        assert!(
            verified > 0,
            "manifest.sha256 must contain at least one entry"
        );
    }

    #[test]
    fn parse_fips_preserves_full_8_char_code() {
        let xml = r#"<Site><Address><Name>G0900090</Name></Address></Site>"#;
        assert_eq!(parse_fips_from_hpxml(xml), "G0900090");
    }

    #[test]
    fn parse_fips_preserves_7_char_code() {
        let xml = r#"<Site><Address><Name>G160027</Name></Address></Site>"#;
        assert_eq!(parse_fips_from_hpxml(xml), "G160027");
    }

    #[test]
    fn resstock_2024_2_smoke() {
        for bldg_dir in fixture_building_dirs("2024.2") {
            run_resstock_smoke("2024.2", &bldg_dir);
        }
    }

    #[test]
    fn resstock_2025_1_smoke() {
        for bldg_dir in fixture_building_dirs("2025.1") {
            run_resstock_smoke("2025.1", &bldg_dir);
        }
    }
}
