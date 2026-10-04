//! ResStock integration smoke tests.
//!
//! Runs short (1h) simulations on real ResStock HPXML files from both
//! 2024.2 (TMY3) and 2025.1 (AMY 2018) releases and verifies that the
//! engine completes without error, produces physically plausible output,
//! and that zone temperatures stay within physical bounds.
//!
//! Seasonal 72 h runs at 1 h resolution on the `bldg0000004` (July) and
//! `bldg0000002` (January) fixtures additionally assert that the cooling,
//! electric heating and gas end uses carry energy over the run.
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
                if i < columns.len()
                    && let Ok(v) = field.trim().parse::<f64>()
                {
                    data.get_mut(&columns[i]).unwrap().push(v);
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

        // Load-side end uses never go negative; the PV end use is the
        // generation convention (negative = power produced), so it never
        // goes positive. "Total Electric Power (kW)" is the net of the two,
        // unbounded in sign for a PV home.
        for (col, values) in &data {
            if !col.ends_with(" End Use Electric Power (kW)") {
                continue;
            }
            let is_pv = col.starts_with("PV ");
            for (i, &v) in values.iter().enumerate() {
                assert!(
                    if is_pv { v <= 0.0 } else { v >= 0.0 },
                    "'{col}' is {v} kW at row {i}: loads are non-negative and \
                     the PV end use is non-positive"
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
        hares_core::checksum::hex_encode(&hasher.finalize())
    }

    #[test]
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

    /// The 72 h winter case on the 2024.2 `bldg0000002` fixture books its
    /// whole `hvac_cooling` end use to the central air conditioner's
    /// crankcase heater. The fixture declares a 50 W crankcase heater
    /// (`CrankcaseHeaterPowerWatts` in the `CoolingSystem` extension); the
    /// compressor never runs in mid-January, so the heater draws its rated
    /// 50 W on every step whose outdoor dry-bulb is below the 12.78 °C
    /// activation threshold (69 of the 72 steps) and nothing on the three
    /// warm-afternoon steps that reach the threshold. That booking matches
    /// the references: EnergyPlus meters a standalone DX cooling coil's
    /// crankcase electricity to the cooling end
    /// use ("Cooling Coil Crankcase Heater Electricity Energy",
    /// `EndUseCat::Cooling` in DXCoils.cc) and applies the heater power as
    /// capacity times the compressor-off fraction (1 minus the coil runtime
    /// fraction), and OCHRE's `AirConditioner` adds the crankcase power to
    /// the unit's `electric_kw` (booked to the HVAC Cooling end use) when its
    /// mode is off and the ambient dry-bulb is below 55 °F.
    #[test]
    fn winter_cooling_energy_is_the_crankcase_heater() {
        const RATED_CRANKCASE_KW: f64 = 0.05;
        const CRANKCASE_THRESHOLD_C: f64 = 12.78;
        const STEP_HOURS: f64 = 1.0;
        const BELOW_THRESHOLD_STEPS: usize = 69;

        let bldg_dir = fixture_building_dirs("2024.2")
            .into_iter()
            .find(|d| d.file_name().unwrap().to_str().unwrap().contains("000002"))
            .expect("2024.2 bldg0000002 fixture must exist");
        let output_path =
            std::env::temp_dir().join(unique_temp_name("hares_winter_crankcase_pin", "csv"));
        let _guard = TempFile(output_path.clone());

        let config = DwellingConfig {
            hpxml_path: bldg_dir.join("home.xml"),
            schedule_path: bldg_dir.join("in.schedules.csv"),
            weather_path: weather_path("2024.2", &bldg_dir),
            defaults_path: Some(project_root().join("defaults")),
            sim_config: SimulationConfig {
                start_time: FixedOffset::west_opt(7 * 3600)
                    .expect("UTC-7 offset")
                    .with_ymd_and_hms(2018, 1, 15, 0, 0, 0)
                    .unwrap(),
                duration: Duration::hours(72),
                time_res: Duration::minutes(60),
                output_verbosity: 1,
                output_path: Some(output_path.clone()),
                write_output: true,
                output_format: OutputFormat::Csv,
                output_chunk_size: 1024,
                setpoint_deadband_c: None,
                master_seed: 42,
                civil_timezone: None,
                site_location: hares_io::SiteLocationOverride::default(),
                retain_batches: true,
                rotation: hares_io::RotationPolicy::None,
            },
            overrides: None,
            bldg_id: 200,
            initialization_duration: Some(std::time::Duration::from_secs(24 * 3600)),
            resample_overrides: Some(hares_io::ResampleOverrides::ochre_compat()),
            patches: None,
        };

        let engine = SimulationEngine::new();
        let result = engine.run(config).expect("engine.run should succeed");
        assert!(
            !matches!(result.status, SimStatus::Failed(_)),
            "winter run failed: {result:?}"
        );

        let data = parse_csv_columns(&output_path);
        let outdoor = data
            .get("Outdoor Dry Bulb (C)")
            .expect("Outdoor Dry Bulb (C) column");
        let ac_power = data
            .get("Air Conditioner Electric Power (kW)")
            .expect("Air Conditioner Electric Power (kW) column");
        assert_eq!(
            outdoor.len(),
            ac_power.len(),
            "outdoor dry-bulb and AC power must cover the same steps"
        );
        assert_eq!(ac_power.len(), 72, "72 h run must produce 72 hourly steps");

        // The heater draws only on steps below its threshold, with the
        // compressor off: a below-threshold step draws exactly the rated
        // 50 W (compressor and fan contribute nothing and the compressor-off
        // fraction is 1), a step at or above the threshold draws nothing.
        let mut below_threshold_steps = 0usize;
        for (i, (&t, &p)) in outdoor.iter().zip(ac_power.iter()).enumerate() {
            if t < CRANKCASE_THRESHOLD_C {
                below_threshold_steps += 1;
                assert_eq!(
                    p, RATED_CRANKCASE_KW,
                    "step {i} at {t:.1} °C drew {p} kW, not the rated crankcase draw"
                );
            } else {
                assert_eq!(
                    p, 0.0,
                    "step {i} at {t:.1} °C drew {p} kW at or above the threshold"
                );
            }
        }
        assert_eq!(
            below_threshold_steps, BELOW_THRESHOLD_STEPS,
            "the run's outdoor dry-bulb must sit below the threshold on all but \
             the three warm-afternoon steps"
        );

        // The cooling end use equals the crankcase heater's energy: the AC is
        // the fixture's only cooling equipment, so the two books reconcile.
        let crankcase_energy_kwh = (below_threshold_steps as f64) * RATED_CRANKCASE_KW * STEP_HOURS;
        let cooling_kwh = result.metrics.total_energy_kwh.per_end_use["hvac_cooling"];
        assert!(
            (cooling_kwh - crankcase_energy_kwh).abs() < 1e-9,
            "hvac_cooling {cooling_kwh} kWh must equal the crankcase heater's \
             {crankcase_energy_kwh} kWh"
        );
    }

    // ── seasonal 72 h runs ───────────────────────────────────────────────

    /// Builds the shared seasonal configuration: 72 h at 3600 s starting at
    /// midnight on the given date at UTC-7, one day of warm-up, CSV output
    /// to the given path.
    fn seasonal_72h_config(
        version: &str,
        bldg_dir: &Path,
        month: u32,
        day: u32,
        output_path: &Path,
    ) -> DwellingConfig {
        DwellingConfig {
            hpxml_path: bldg_dir.join("home.xml"),
            schedule_path: bldg_dir.join("in.schedules.csv"),
            weather_path: weather_path(version, bldg_dir),
            defaults_path: Some(project_root().join("defaults")),
            sim_config: SimulationConfig {
                start_time: FixedOffset::west_opt(7 * 3600)
                    .expect("UTC-7 offset")
                    .with_ymd_and_hms(2018, month, day, 0, 0, 0)
                    .unwrap(),
                duration: Duration::hours(72),
                time_res: Duration::minutes(60),
                output_verbosity: 1,
                output_path: Some(output_path.to_path_buf()),
                write_output: true,
                output_format: OutputFormat::Csv,
                output_chunk_size: 1024,
                setpoint_deadband_c: None,
                master_seed: 42,
                civil_timezone: None,
                site_location: hares_io::SiteLocationOverride::default(),
                retain_batches: true,
                rotation: hares_io::RotationPolicy::None,
            },
            overrides: None,
            bldg_id: 200,
            initialization_duration: Some(std::time::Duration::from_secs(24 * 3600)),
            resample_overrides: Some(hares_io::ResampleOverrides::ochre_compat()),
            patches: None,
        }
    }

    /// Three-day July run on the release's `bldg0000004` fixture (Texas
    /// weather, a guaranteed cooling load). Asserts the cooling end use
    /// carries energy over the run and that every output value stays inside
    /// the physical bounds. No peak or total-energy band is asserted.
    fn run_summer_72h(version: &str) {
        let bldg_dir = fixture_building_dirs(version)
            .into_iter()
            .find(|d| d.file_name().unwrap().to_str().unwrap().contains("000004"))
            .expect("bldg0000004 fixture must exist");
        let output_path =
            std::env::temp_dir().join(unique_temp_name("hares_resstock_summer_72h", "csv"));
        let _guard = TempFile(output_path.clone());

        let config = seasonal_72h_config(version, &bldg_dir, 7, 15, &output_path);
        let engine = SimulationEngine::new();
        let result = engine.run(config).expect("engine.run should succeed");
        assert!(
            !matches!(result.status, SimStatus::Failed(_)),
            "{version} summer 72 h run failed: {result:?}"
        );

        let cooling_kwh = result.metrics.total_energy_kwh.per_end_use["hvac_cooling"];
        eprintln!("[{version}] summer_72h hvac_cooling: {cooling_kwh:.3} kWh");
        assert!(
            cooling_kwh > 0.0,
            "{version} summer 72 h: hvac_cooling must carry energy, got {cooling_kwh} kWh"
        );

        assert_physics_bounds(&output_path);
    }

    /// Three-day January run on the release's `bldg0000002` fixture (Idaho
    /// weather, a guaranteed heating load). The fixture heats with gas (a
    /// boiler in 2025.1, a furnace in 2024.2), so `hvac_heating`, an electric
    /// end use, measures only the boiler auxiliary or fan electricity, and
    /// the run's gas energy is the sum of the output file's
    /// `Total Gas Power (therms/hour)` column times the 1 h step. Asserts the
    /// electric heating end use, the gas energy, and the physical bounds.
    fn run_winter_72h(version: &str) {
        let bldg_dir = fixture_building_dirs(version)
            .into_iter()
            .find(|d| d.file_name().unwrap().to_str().unwrap().contains("000002"))
            .expect("bldg0000002 fixture must exist");
        let output_path =
            std::env::temp_dir().join(unique_temp_name("hares_resstock_winter_72h", "csv"));
        let _guard = TempFile(output_path.clone());

        let config = seasonal_72h_config(version, &bldg_dir, 1, 15, &output_path);
        let engine = SimulationEngine::new();
        let result = engine.run(config).expect("engine.run should succeed");
        assert!(
            !matches!(result.status, SimStatus::Failed(_)),
            "{version} winter 72 h run failed: {result:?}"
        );

        let heating_kwh = result.metrics.total_energy_kwh.per_end_use["hvac_heating"];
        eprintln!("[{version}] winter_72h hvac_heating: {heating_kwh:.3} kWh");
        assert!(
            heating_kwh > 0.0,
            "{version} winter 72 h: hvac_heating must carry energy, got {heating_kwh} kWh"
        );

        const STEP_HOURS: f64 = 1.0;
        let gas_therms: f64 = parse_csv_columns(&output_path)
            .remove("Total Gas Power (therms/hour)")
            .expect("Total Gas Power (therms/hour) column in the output CSV")
            .iter()
            .sum::<f64>()
            * STEP_HOURS;
        eprintln!("[{version}] winter_72h gas energy: {gas_therms:.3} therms");
        assert!(
            gas_therms > 0.0,
            "{version} winter 72 h: gas energy must be above zero, got {gas_therms} therms"
        );

        assert_physics_bounds(&output_path);
    }

    #[test]
    fn resstock_2025_1_summer_72h() {
        run_summer_72h("2025.1");
    }

    #[test]
    fn resstock_2025_1_winter_72h() {
        run_winter_72h("2025.1");
    }

    #[test]
    fn resstock_2024_2_summer_72h() {
        run_summer_72h("2024.2");
    }

    #[test]
    fn resstock_2024_2_winter_72h() {
        run_winter_72h("2024.2");
    }
}
