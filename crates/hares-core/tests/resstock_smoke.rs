//! ResStock integration sanity tests.
//!
//! Verifies HARES on real ResStock HPXML from 2024.2 (TMY3) and
//! 2025.1 (AMY 2018). Fixtures in tests/fixtures/resstock/{version}/.

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;

    use chrono::{Duration, FixedOffset, TimeZone};
    use hares_core::{DwellingConfig, SimStatus, SimulationConfig, SimulationEngine};
    use hares_io::OutputFormat;

    // ── helpers ──────────────────────────────────────────────────────────

    fn project_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn fixture_building_dirs(version: &str) -> Vec<PathBuf> {
        let base = project_root()
            .join("tests/fixtures/resstock")
            .join(version);
        let mut dirs: Vec<_> = std::fs::read_dir(&base)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_type().map(|t| t.is_dir()).unwrap_or(false)
                    && e.file_name().to_str()
                        .map(|n| n.starts_with("bldg"))
                        .unwrap_or(false)
            })
            .map(|e| e.path())
            .collect();
        dirs.sort();
        dirs
    }

    fn weather_path(version: &str, bldg_dir: &PathBuf) -> PathBuf {
        let hpxml = fs::read_to_string(bldg_dir.join("home.xml")).unwrap();
        let fips = parse_fips_from_hpxml(&hpxml);
        let weather_dir = project_root()
            .join("tests/fixtures/resstock")
            .join(version)
            .join("weather");
        let ext = if version == "2025.1" { "csv" } else { "epw" };
        if let Some(p) = [format!("{fips}_2018.csv"), format!("{fips}.{ext}")]
            .iter()
            .map(|n| weather_dir.join(n))
            .find(|p| p.exists())
        {
            return p;
        }
        weather_dir.read_dir().unwrap()
            .filter_map(|e| e.ok())
            .find(|e| e.path().is_file())
            .map(|e| e.path())
            .unwrap_or_else(|| panic!("no weather in {weather_dir:?}"))
    }

    fn parse_fips_from_hpxml(xml: &str) -> String {
        for line in xml.lines() {
            if let Some(s) = line.find("<Name>") {
                let inner = &line[s + 6..];
                if let Some(e) = inner.find("</Name>") {
                    let n = &inner[..e];
                    if n.starts_with('G') && n.len() >= 7 {
                        return n[..7].to_string();
                    }
                }
            }
        }
        panic!("no FIPS weather station in HPXML");
    }

    fn utn(base: &str, ext: &str) -> String {
        format!("{base}_{}_{:?}.{ext}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
            std::thread::current().id())
    }

    struct Tmp(PathBuf);
    impl Drop for Tmp { fn drop(&mut self) { let _ = fs::remove_file(&self.0); } }

    fn read_output(path: &PathBuf) -> BTreeMap<String, Vec<f64>> {
        let s = fs::read_to_string(path).expect("read CSV");
        let mut lines = s.lines();
        let hdr = lines.next().expect("header");
        let cols: Vec<String> = hdr.split(',').map(|c| c.trim().to_string()).collect();
        let mut data: BTreeMap<String, Vec<f64>> = cols.iter().map(|c| (c.clone(), vec![])).collect();
        for line in lines.filter(|l| !l.trim().is_empty()) {
            for (i, f) in line.split(',').enumerate() {
                if i < cols.len() { if let Ok(v) = f.trim().parse::<f64>() { data.get_mut(&cols[i]).unwrap().push(v); } }
            }
        }
        data
    }

    // ── assertions ───────────────────────────────────────────────────────

    fn assert_no_nan(data: &BTreeMap<String, Vec<f64>>) {
        for (col, vals) in data {
            for (i, &v) in vals.iter().enumerate() {
                assert!(v.is_finite(), "NaN/Inf in '{col}' row {i}");
            }
        }
    }

    fn assert_indoor_temp_bounds(data: &BTreeMap<String, Vec<f64>>, label: &str) {
        let col = data.keys().find(|k|
            k.starts_with("Temperature -") && k.ends_with("(C)") && k.contains("Indoor")
        ).expect("no indoor temp column");
        let vals = &data[col];
        let (lo, hi) = vals.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(l,h),&v| (l.min(v), h.max(v)));
        let avg = vals.iter().sum::<f64>() / vals.len() as f64;
        eprintln!("    {label} indoor: lo={lo:.1} hi={hi:.1} avg={avg:.1}°C");
        // Wide physics bounds — this is a smoke test, not thermal validation.
        assert!(lo > -30.0, "{label}: indoor min {lo:.1}°C (freeze damage / numerical runaway)");
        assert!(hi < 60.0,  "{label}: indoor max {hi:.1}°C (fire hazard / numerical runaway)");
    }

    fn assert_peak_power(data: &BTreeMap<String, Vec<f64>>, max_kw: f64, label: &str) {
        if let Some(col) = data.keys().find(|k| k == &"Total Electric Power (kW)") {
            let peak = data[col].iter().fold(0.0_f64, |a,&b| a.max(b));
            eprintln!("    {label} peak power: {peak:.2} kW");
            assert!(peak < max_kw, "{label}: peak {peak:.1} kW > {max_kw} kW");
        }
    }

    fn assert_energy_range(total_kwh: f64, lo: f64, hi: f64, label: &str) {
        eprintln!("    {label} energy: {total_kwh:.2} kWh");
        assert!(total_kwh > lo, "{label}: {total_kwh:.2} kWh < {lo} kWh");
        assert!(total_kwh < hi, "{label}: {total_kwh:.2} kWh > {hi} kWh");
    }

    fn assert_total_power_non_neg(data: &BTreeMap<String, Vec<f64>>) {
        if let Some(col) = data.keys().find(|k| k == &"Total Electric Power (kW)") {
            for (i, &v) in data[col].iter().enumerate() {
                assert!(v >= 0.0, "negative total power {v} at row {i}");
            }
        }
    }

    // ── config builder ───────────────────────────────────────────────────

    fn config_for(bldg_dir: &PathBuf, version: &str, hrs: i64, min_step: i64,
                  month: u32, day: u32) -> DwellingConfig {
        DwellingConfig {
            hpxml_path: bldg_dir.join("home.xml"),
            schedule_path: bldg_dir.join("in.schedules.csv"),
            weather_path: weather_path(version, bldg_dir),
            defaults_path: Some(project_root().join("defaults")),
            sim_config: SimulationConfig {
                start_time: FixedOffset::west_opt(7 * 3600).unwrap()
                    .with_ymd_and_hms(2018, month, day, 0, 0, 0).unwrap(),
                duration: Duration::hours(hrs),
                time_res: Duration::minutes(min_step),
                output_verbosity: 1,
                output_path: Some(std::env::temp_dir().join(utn("rs", "csv"))),
                write_output: true,
                output_format: OutputFormat::Csv,
                output_chunk_size: 1024,
                setpoint_deadband_c: None,
                master_seed: 42,
                civil_timezone: None,
            },
            overrides: None,
            bldg_id: 200,
            initialization_duration: Some(std::time::Duration::from_secs(24 * 3600)),
            resample_overrides: Some(hares_io::ResampleOverrides::ochre_compat()),
        }
    }

    fn run_and_check(config: DwellingConfig, label: &str) -> (f64, BTreeMap<String, Vec<f64>>) {
        let output = config.sim_config.output_path.clone().unwrap();
        let _g = Tmp(output.clone());
        let engine = SimulationEngine::new();
        let result = engine.run(config).expect("run");
        assert!(!matches!(result.status, SimStatus::Failed(_)), "{label}: {result:?}");
        let kwh = result.metrics.annual_energy_kwh.total;
        let data = if output.exists() { read_output(&output) } else { BTreeMap::new() };
        assert_no_nan(&data);
        assert_total_power_non_neg(&data);
        eprintln!(
            "[{}] {:.2} kWh, {:.0}ms",
            label, kwh, result.elapsed.as_secs_f64() * 1000.0
        );
        (kwh, data)
    }

    // ── tests ────────────────────────────────────────────────────────────

    /// 1h fast smoke — catches parse/startup errors (all buildings).
    fn smoke_1h(version: &str) {
        for bd in fixture_building_dirs(version) {
            let name = bd.file_name().unwrap().to_str().unwrap();
            let cfg = config_for(&bd, version, 1, 1, 5, 5);
            let (kwh, _data) = run_and_check(cfg, &format!("{version}/{name}/1h"));
            assert!(kwh >= 0.0);
        }
    }

    /// 3-day summer week at 1h res — verifies AC runs, reasonable peak.
    fn summer_72h(version: &str) {
        // Use bldg 4 (TX weather → hot summer = guaranteed AC load).
        let bd = fixture_building_dirs(version)
            .into_iter()
            .find(|d| d.file_name().unwrap().to_str().unwrap().contains("000004"))
            .unwrap_or_else(|| fixture_building_dirs(version)[0].clone());
        let cfg = config_for(&bd, version, 72, 60, 7, 15);
        let (kwh, data) = run_and_check(cfg, "summer_72h");
        assert_indoor_temp_bounds(&data, "summer_72h");
        assert_peak_power(&data, 20.0, "summer_72h");
        assert_energy_range(kwh, 5.0, 400.0, "summer_72h");
    }

    /// 3-day winter week at 1h res — verifies heating runs, no freeze.
    fn winter_72h(version: &str) {
        // Use bldg 2 (Idaho weather → cold winter = guaranteed heat).
        let bd = fixture_building_dirs(version)
            .into_iter()
            .find(|d| d.file_name().unwrap().to_str().unwrap().contains("000002"))
            .unwrap_or_else(|| fixture_building_dirs(version)[0].clone());
        let cfg = config_for(&bd, version, 72, 60, 1, 15);
        let (kwh, data) = run_and_check(cfg, "winter_72h");
        assert_indoor_temp_bounds(&data, "winter_72h");
        assert_peak_power(&data, 20.0, "winter_72h");
        assert_energy_range(kwh, 5.0, 600.0, "winter_72h");
    }

    // ── test entry points ────────────────────────────────────────────────

    // --- 2025.1 ---
    #[test] fn resstock_2025_1_smoke() { smoke_1h("2025.1"); }
    #[test] fn resstock_2025_1_summer_72h() { summer_72h("2025.1"); }
    #[test] fn resstock_2025_1_winter_72h() { winter_72h("2025.1"); }

    // --- 2024.2 ---
    #[test] fn resstock_2024_2_smoke() { smoke_1h("2024.2"); }
    #[test] fn resstock_2024_2_summer_72h() { summer_72h("2024.2"); }
    #[test] fn resstock_2024_2_winter_72h() { winter_72h("2024.2"); }
}
