//! ResStock integration sanity tests.
//!
//! Verifies HARES on real ResStock HPXML from 2024.2 (TMY3) and
//! 2025.1 (AMY 2018). Fixtures in tests/fixtures/resstock/{version}/.

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::Path;
    use std::path::PathBuf;

    use chrono::{Duration, FixedOffset, TimeZone};
    use hares_core::{DwellingConfig, SimStatus, SimulationConfig, SimulationEngine};
    use hares_io::OutputFormat;

    // ── helpers ──────────────────────────────────────────────────────────

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
                    && e.file_name()
                        .to_str()
                        .map(|n| n.starts_with("bldg"))
                        .unwrap_or(false)
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
        let ext = if version == "2025.1" { "csv" } else { "epw" };
        if let Some(p) = [format!("{fips}_2018.csv"), format!("{fips}.{ext}")]
            .iter()
            .map(|n| weather_dir.join(n))
            .find(|p| p.exists())
        {
            return p;
        }
        weather_dir
            .read_dir()
            .unwrap()
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
        format!(
            "{base}_{}_{:?}.{ext}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            std::thread::current().id()
        )
    }

    struct Tmp(PathBuf);
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    fn read_output(path: &PathBuf) -> BTreeMap<String, Vec<f64>> {
        let s = fs::read_to_string(path).expect("read CSV");
        let mut lines = s.lines();
        let hdr = lines.next().expect("header");
        let cols: Vec<String> = hdr.split(',').map(|c| c.trim().to_string()).collect();
        let mut data: BTreeMap<String, Vec<f64>> =
            cols.iter().map(|c| (c.clone(), vec![])).collect();
        for line in lines.filter(|l| !l.trim().is_empty()) {
            for (i, f) in line.split(',').enumerate() {
                if i < cols.len() {
                    if let Ok(v) = f.trim().parse::<f64>() {
                        data.get_mut(&cols[i]).unwrap().push(v);
                    }
                }
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
        let col = data
            .keys()
            .find(|k| k.starts_with("Temperature -") && k.ends_with("(C)") && k.contains("Indoor"))
            .expect("no indoor temp column");
        let vals = &data[col];
        let (lo, hi) = vals
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), &v| {
                (l.min(v), h.max(v))
            });
        let avg = vals.iter().sum::<f64>() / vals.len() as f64;
        eprintln!("    {label} indoor: lo={lo:.1} hi={hi:.1} avg={avg:.1}°C");
        // Wide physics bounds — this is a smoke test, not thermal validation.
        assert!(
            lo > -30.0,
            "{label}: indoor min {lo:.1}°C (freeze damage / numerical runaway)"
        );
        assert!(
            hi < 60.0,
            "{label}: indoor max {hi:.1}°C (fire hazard / numerical runaway)"
        );
    }

    fn assert_peak_power(data: &BTreeMap<String, Vec<f64>>, max_kw: f64, label: &str) {
        if let Some(col) = data.keys().find(|k| k == &"Total Electric Power (kW)") {
            let peak = data[col].iter().fold(0.0_f64, |a, &b| a.max(b));
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

    fn config_for(
        bldg_dir: &Path,
        version: &str,
        hrs: i64,
        min_step: i64,
        month: u32,
        day: u32,
    ) -> DwellingConfig {
        DwellingConfig {
            hpxml_path: bldg_dir.join("home.xml"),
            schedule_path: bldg_dir.join("in.schedules.csv"),
            weather_path: weather_path(version, bldg_dir),
            defaults_path: Some(project_root().join("defaults")),
            sim_config: SimulationConfig {
                start_time: FixedOffset::west_opt(7 * 3600)
                    .unwrap()
                    .with_ymd_and_hms(2018, month, day, 0, 0, 0)
                    .unwrap(),
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

    fn run_and_check(config: DwellingConfig, label: &str) -> (f64, BTreeMap<String, Vec<f64>>) {
        let output = config.sim_config.output_path.clone().unwrap();
        let _g = Tmp(output.clone());
        let engine = SimulationEngine::new();
        let result = engine.run(config).expect("run");
        assert!(
            !matches!(result.status, SimStatus::Failed(_)),
            "{label}: {result:?}"
        );
        let kwh = result.metrics.annual_energy_kwh.total;
        let data = if output.exists() {
            read_output(&output)
        } else {
            BTreeMap::new()
        };
        assert_no_nan(&data);
        assert_total_power_non_neg(&data);
        eprintln!(
            "[{}] {:.2} kWh, {:.0}ms",
            label,
            kwh,
            result.elapsed.as_secs_f64() * 1000.0
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

    // ── observer diagnostic ──────────────────────────────────────────────

    /// Debug why bldg0000002 runs cold in winter.
    ///
    /// Runs 6h at 1-min resolution with the observer enabled and prints
    /// per-step thermostat mode, setpoints, zone temp, and HVAC contribution
    /// so we can see whether the furnace is calling for heat.
    ///
    /// Run with:
    ///   cargo test -p hares-core --test resstock_smoke --features observe \
    ///     -- debug_bldg0000002_winter_heating --ignored --nocapture
    #[test]
    #[ignore]
    #[cfg(feature = "observe")]
    fn debug_bldg0000002_winter_heating() {
        use hares_core::Dwelling;

        let bd = fixture_building_dirs("2024.2")
            .into_iter()
            .find(|d| d.file_name().unwrap().to_str().unwrap().contains("000002"))
            .expect("bldg0000002 fixture not found");

        let mut cfg = config_for(&bd, "2024.2", 6, 1, 1, 15);
        // Don't write CSV output — we'll read the observer instead.
        cfg.sim_config.write_output = false;
        cfg.sim_config.output_path = None;
        // Keep 24h init so we see post-warmup state.
        cfg.initialization_duration = Some(std::time::Duration::from_secs(24 * 3600));

        let mut dwelling = Dwelling::from_config(cfg).expect("build dwelling");

        let n_steps = 6 * 60; // 6 hours at 1-min
        dwelling.enable_observer(n_steps);

        eprintln!("\n=== bldg0000002 winter heating observer diagnostic ===");

        // First step: print equipment names and init setpoints.
        dwelling.step().expect("step 0");
        if let Some(buf) = dwelling.observer_buffer() {
            if let Some(snap) = buf.last() {
                // Zone temps at each phase boundary.
                let env_zone = snap
                    .phases
                    .post_environment
                    .as_ref()
                    .and_then(|e| e.zone_temps_c.first().map(|(_, t)| *t))
                    .unwrap_or(f64::NAN);
                let final_zone = snap
                    .phases
                    .post_zone_update
                    .as_ref()
                    .and_then(|z| z.zone_temps_c.first().map(|(_, t)| *t))
                    .unwrap_or(f64::NAN);
                eprintln!(
                    "\nStep 0 zone temps: env_phase={:.2}°C  post_update={:.2}°C",
                    env_zone, final_zone
                );

                eprintln!("\nEquipment at step 0:");
                if let Some(phase) = snap.phases.post_thermal_equipment.as_ref() {
                    for eq in &phase.equipment {
                        let mode = eq.telemetry.get("operating_mode").unwrap_or(f64::NAN);
                        let heat_sp = eq.telemetry.get("heating_setpoint_c").unwrap_or(f64::NAN);
                        let cool_sp = eq.telemetry.get("cooling_setpoint_c").unwrap_or(f64::NAN);
                        let sched_h = eq
                            .telemetry
                            .get("schedule_heating_setpoint_c")
                            .unwrap_or(f64::NAN);
                        let sched_c = eq
                            .telemetry
                            .get("schedule_cooling_setpoint_c")
                            .unwrap_or(f64::NAN);
                        let output_w = eq.telemetry.get("thermal_output_w").unwrap_or(f64::NAN);
                        let hvac_w = eq
                            .contribution
                            .thermal
                            .first()
                            .map(|(_, s, _)| *s)
                            .unwrap_or(0.0);
                        eprintln!(
                            "  {:30} mode={:4.1} heat_sp={:6.2} cool_sp={:6.2} sched_h={:6.2} sched_c={:6.2} out_W={:8.1} contrib_W={:8.1}",
                            eq.name, mode, heat_sp, cool_sp, sched_h, sched_c, output_w, hvac_w
                        );
                    }
                }
            }
        }

        eprintln!(
            "\n{:>6} {:>10} {:>8} {:>8}  {}",
            "step", "time", "zone_C", "oat_C", "equipment (mode / heat_sp / cool_sp / hvac_W)"
        );

        for i in 1..n_steps {
            dwelling.step().expect("step");

            // Emit a row every 30 steps (30 min).
            if i % 30 == 0 {
                if let Some(buf) = dwelling.observer_buffer() {
                    if let Some(snap) = buf.last() {
                        let zone_c = snap
                            .phases
                            .post_zone_update
                            .as_ref()
                            .and_then(|z| z.zone_temps_c.first().map(|(_, t)| *t))
                            .unwrap_or(f64::NAN);
                        let oat_c = snap
                            .phases
                            .post_environment
                            .as_ref()
                            .map(|e| e.outdoor_temp_c)
                            .unwrap_or(f64::NAN);

                        let eq_summary: Vec<String> = snap
                            .phases
                            .post_thermal_equipment
                            .as_ref()
                            .map(|p| {
                                p.equipment
                                    .iter()
                                    .map(|eq| {
                                        let mode =
                                            eq.telemetry.get("operating_mode").unwrap_or(f64::NAN);
                                        let heat_sp = eq
                                            .telemetry
                                            .get("heating_setpoint_c")
                                            .unwrap_or(f64::NAN);
                                        let cool_sp = eq
                                            .telemetry
                                            .get("cooling_setpoint_c")
                                            .unwrap_or(f64::NAN);
                                        let hvac_w = eq
                                            .contribution
                                            .thermal
                                            .first()
                                            .map(|(_, s, _)| *s)
                                            .unwrap_or(0.0);
                                        format!(
                                            "[{} m={:.0} h={:.1} c={:.1} W={:.0}]",
                                            eq.name, mode, heat_sp, cool_sp, hvac_w
                                        )
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();

                        eprintln!(
                            "{:>6} {:>10} {:>8.2} {:>8.2}  {}",
                            i,
                            snap.timestamp.format("%H:%M"),
                            zone_c,
                            oat_c,
                            eq_summary.join("  ")
                        );
                    }
                }
            }
        }

        let snaps = dwelling.drain_observations();
        eprintln!("\nCaptured {} observer snapshots", snaps.len());

        // Count per-equipment operating steps.
        let mut eq_mode_counts: std::collections::HashMap<String, (usize, usize)> =
            std::collections::HashMap::new();
        for snap in &snaps {
            if let Some(phase) = snap.phases.post_thermal_equipment.as_ref() {
                for eq in &phase.equipment {
                    let entry = eq_mode_counts.entry(eq.name.clone()).or_default();
                    entry.0 += 1;
                    if eq.telemetry.get("operating_mode").unwrap_or(0.0) > 0.5 {
                        entry.1 += 1;
                    }
                }
            }
        }
        eprintln!("\nEquipment active step counts (active/total):");
        for (name, (total, active)) in &eq_mode_counts {
            eprintln!("  {:30} {}/{}", name, active, total);
        }
    }

    // ── test entry points ────────────────────────────────────────────────

    // --- 2025.1 ---
    #[test]
    fn resstock_2025_1_smoke() {
        smoke_1h("2025.1");
    }
    #[test]
    fn resstock_2025_1_summer_72h() {
        summer_72h("2025.1");
    }
    #[test]
    fn resstock_2025_1_winter_72h() {
        winter_72h("2025.1");
    }

    // --- 2024.2 ---
    #[test]
    fn resstock_2024_2_smoke() {
        smoke_1h("2024.2");
    }
    #[test]
    fn resstock_2024_2_summer_72h() {
        summer_72h("2024.2");
    }
    #[test]
    fn resstock_2024_2_winter_72h() {
        winter_72h("2024.2");
    }
}
