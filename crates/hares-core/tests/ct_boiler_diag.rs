//! Focused observer diagnostic: trace boiler behavior for 2025.1 bldg 2
//! (gas boiler, Connecticut, winter).
//!
//! Answers: is the boiler running at reduced capacity (space_fraction < 1.0),
//! not running at all (thermostat issue), running at full but undersized, or
//! something else?
//!
//! Run with:
//!   cargo test -p hares-core --test ct_boiler_diag --features observe \
//!     -- ct_boiler_diagnostic --nocapture
#![cfg(feature = "observe")]

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use chrono::{Duration, FixedOffset, TimeZone};
    use hares_core::{Dwelling, DwellingConfig, SimulationConfig};
    use hares_io::OutputFormat;

    fn project_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn weather_for(bldg_dir: &Path) -> PathBuf {
        let xml = fs::read_to_string(bldg_dir.join("home.xml")).unwrap();
        let fips = parse_fips(&xml);
        let wdir = project_root().join("tests/fixtures/resstock/2025.1/weather");
        [format!("{fips}_2018.csv"), format!("{fips}.csv")]
            .iter()
            .map(|n| wdir.join(n))
            .find(|p| p.exists())
            .unwrap_or_else(|| {
                wdir.read_dir()
                    .unwrap()
                    .filter_map(|e| e.ok())
                    .find(|e| e.path().is_file())
                    .map(|e| e.path())
                    .expect("no weather file found")
            })
    }

    fn parse_fips(xml: &str) -> String {
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
        "UNKNOWN".into()
    }

    fn utn(base: &str) -> String {
        format!(
            "{}_{}",
            base,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )
    }

    #[test]
    fn ct_boiler_diagnostic() {
        let bldg_dir = project_root().join("tests/fixtures/resstock/2025.1/bldg0000002");

        let output = std::env::temp_dir().join(utn("ct_boiler_diag.csv"));
        let _cleanup = TmpGuard(output.clone());

        let config = DwellingConfig {
            hpxml_path: bldg_dir.join("home.xml"),
            schedule_path: bldg_dir.join("in.schedules.csv"),
            weather_path: weather_for(&bldg_dir),
            defaults_path: Some(project_root().join("defaults")),
            sim_config: SimulationConfig {
                start_time: FixedOffset::west_opt(5 * 3600)
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
            },
            overrides: None,
            bldg_id: 300,
            initialization_duration: Some(std::time::Duration::from_secs(24 * 3600)),
            resample_overrides: Some(hares_io::ResampleOverrides::ochre_compat()),
            patches: None,
        };

        // ── 1. Build dwelling (warmup runs internally) ─────────────────────
        eprintln!("=== CT Boiler Diagnostic: 2025.1 bldg 2 (gas boiler, CT, Jan 15, 24h) ===");
        eprintln!("    Building dwelling with 24h warmup...");
        let mut dwelling = Dwelling::from_config(config).expect("build dwelling");
        eprintln!("    Warmup complete. Enabling observer...");

        // ── 2. Enable observer ────────────────────────────────────────────
        dwelling.enable_observer(500);

        // ── 3. Run 24h simulation ─────────────────────────────────────────
        eprintln!("    Running 24h simulation at 15-min resolution...");
        dwelling.simulate().expect("simulation must succeed");

        // ── 4. Drain observations ─────────────────────────────────────────
        let snapshots = dwelling.drain_observations();
        eprintln!("    Captured {} observer snapshots.", snapshots.len());

        if snapshots.is_empty() {
            eprintln!("ERROR: No observer snapshots captured.");
            return;
        }

        // ── 5. First 10 and last 5 steps: print summary ──────────────────
        print_step_table(&snapshots, "FIRST 10 STEPS", 0..10.min(snapshots.len()));
        let n = snapshots.len();
        let last_start = n.saturating_sub(5);
        print_step_table(&snapshots, "LAST 5 STEPS", last_start..n);

        // ── 6. Find cold steps (indoor < 10°C) ───────────────────────────
        eprintln!("\n=== COLD STEPS (indoor < 10 °C) ===");
        let mut cold_found = false;
        let mut boiler_thermal_outputs: Vec<f64> = Vec::new();
        let mut boiler_fuel_inputs: Vec<f64> = Vec::new();
        let mut boiler_modes: Vec<f64> = Vec::new();
        let mut zone_temps_all: Vec<f64> = Vec::new();
        let mut outdoor_temps_all: Vec<f64> = Vec::new();

        for (i, snap) in snapshots.iter().enumerate() {
            let zone_temp = first_zone_temp_env(snap);
            let outdoor_temp = snap
                .phases
                .post_environment
                .as_ref()
                .map(|e| e.outdoor_temp_c)
                .unwrap_or(f64::NAN);

            zone_temps_all.push(zone_temp);
            outdoor_temps_all.push(outdoor_temp);

            // Collect boiler telemetry
            if let Some(phase) = snap.phases.post_thermal_equipment.as_ref() {
                for eq in &phase.equipment {
                    if eq.name.to_lowercase().contains("boiler") {
                        let thermal_w = eq.telemetry.get("thermal_output_w").unwrap_or(0.0);
                        let fuel_w = eq.telemetry.get("fuel_input_w").unwrap_or(0.0);
                        let mode = eq.telemetry.get("operating_mode").unwrap_or(0.0);
                        boiler_thermal_outputs.push(thermal_w);
                        boiler_fuel_inputs.push(fuel_w);
                        boiler_modes.push(mode);
                    }
                }
            }

            if zone_temp < 10.0 && zone_temp.is_finite() {
                cold_found = true;
                dump_cold_step(i, snap);
            }
        }

        if !cold_found {
            eprintln!("    (No steps with indoor temp < 10 °C found.)");
        }

        // ── 7. Analysis ──────────────────────────────────────────────────
        eprintln!("\n=== ANALYSIS ===");

        // Temperature summary
        let min_zone = zone_temps_all.iter().fold(f64::INFINITY, |a, &b| a.min(b));
        let max_zone = zone_temps_all
            .iter()
            .fold(f64::NEG_INFINITY, |a, &b| a.max(b));
        let avg_zone = zone_temps_all.iter().sum::<f64>() / zone_temps_all.len().max(1) as f64;

        let min_oat = outdoor_temps_all
            .iter()
            .fold(f64::INFINITY, |a, &b| a.min(b));
        let max_oat = outdoor_temps_all
            .iter()
            .fold(f64::NEG_INFINITY, |a, &b| a.max(b));

        eprintln!("  Zone temp:  min={min_zone:.1}°C  max={max_zone:.1}°C  avg={avg_zone:.1}°C");
        eprintln!("  Outdoor:    min={min_oat:.1}°C  max={max_oat:.1}°C");

        // Boiler analysis
        if !boiler_thermal_outputs.is_empty() {
            let max_thermal_w = boiler_thermal_outputs
                .iter()
                .fold(f64::NEG_INFINITY, |a, &b| a.max(b));
            let max_fuel_w = boiler_fuel_inputs
                .iter()
                .fold(f64::NEG_INFINITY, |a, &b| a.max(b));
            let steps_on = boiler_modes.iter().filter(|&&m| m > 0.5).count();
            let steps_off = boiler_modes.iter().filter(|&&m| m < 0.5).count();
            let total_steps = boiler_modes.len();

            eprintln!("\n  Boiler analysis ({total_steps} steps observed):");
            eprintln!("    ON steps:  {steps_on}");
            eprintln!("    OFF steps: {steps_off}");
            eprintln!(
                "    Max thermal_output_w: {max_thermal_w:.0} W ({:.2} kW)",
                max_thermal_w / 1000.0
            );
            eprintln!(
                "    Max fuel_input_w:     {max_fuel_w:.0} W ({:.2} kW)",
                max_fuel_w / 1000.0
            );

            // Expected: HPXML HeatingCapacity = 37881 BTU/hr, AFUE = 0.78
            // Net capacity = 37881 * 0.78 / 3.41214 = ~8658 W (8.66 kW)
            let expected_net_w = 8658.0;
            eprintln!(
                "    Expected net capacity: {expected_net_w:.0} W (8.66 kW from HPXML 37881 BTU/hr * 0.78 AFUE)"
            );

            if max_thermal_w > 0.0 {
                let apparent_sf = max_thermal_w / expected_net_w;
                let sf_verdict = if (apparent_sf - 1.0).abs() < 0.05 {
                    "≈ 1.0 (full capacity)".to_string()
                } else if apparent_sf < 0.9 {
                    format!("≈ {apparent_sf:.2} (REDUCED — check fraction_heating_load_served)")
                } else {
                    format!("≈ {apparent_sf:.2} (close to 1.0)")
                };
                eprintln!("    Apparent space_fraction (max_thermal/expected): {apparent_sf:.3}");
                eprintln!("    => space_fraction {sf_verdict}");
            }

            if steps_on == 0 {
                eprintln!("\n    *** BOILER NEVER FIRED ***");
                eprintln!("        Likely thermostat issue — heating setpoint may be set too low");
                eprintln!("        or the thermostat is not recognizing the heating need.");
            } else if (min_zone - min_oat).abs() < 1.0 {
                eprintln!(
                    "\n    *** INDOOR TEMP TRACKS OUTDOOR — possible no thermal coupling ***"
                );
            } else if min_zone < 12.0 && steps_on > 0 {
                eprintln!("\n    *** BOILER RUNNING BUT HOUSE STILL COLD ***");
                if max_thermal_w < expected_net_w * 0.8 {
                    eprintln!("        Boiler running at reduced capacity (space_fraction < 1.0)");
                } else {
                    eprintln!("        Boiler at full capacity but cannot meet load (undersized)");
                }
                eprintln!(
                    "        Possible causes: poor insulation, high infiltration, duct losses."
                );
            }
        }

        // ── 8. Check boiler equipment directly (post-simulation state) ────
        eprintln!("\n  Equipment inspection (post-simulation):");
        for eq in dwelling.equipment() {
            let name: &str = &eq.descriptor().name;
            let co = eq.core_output();
            let mode = co
                .state
                .operating_mode
                .map(|m| format!("{m:?}"))
                .unwrap_or_default();
            let setpoint = co
                .state
                .setpoint_c
                .map(|s| format!("{s:.1}°C"))
                .unwrap_or_default();
            let elec = co
                .flows
                .electric_kw
                .as_ref()
                .map(|e| format!("{:.3} kW", e.net_consumption_kw()))
                .unwrap_or_default();
            let thermal = co
                .flows
                .thermal_output_w
                .map(|w| format!("{w:.0} W"))
                .unwrap_or_default();
            let fuel = co
                .flows
                .fuel_w
                .as_ref()
                .map(|f| format!("{:.0} W", f.consumption_w))
                .unwrap_or_default();
            eprintln!(
                "    {:30} mode={mode:12} setpoint={setpoint:8} elec={elec:12} thermal={thermal:10} fuel={fuel:10}",
                name
            );

            // Check if boiler has setpoint info
            if name.to_lowercase().contains("boiler") {
                let telemetry = eq.telemetry();
                if let Some(eir) = telemetry.get("eir") {
                    eprintln!(
                        "      telemetry: eir={eir:.4}, rated_capacity (approx via fuel/eir when on)"
                    );
                }
                if telemetry
                    .get("heating_setpoint_c")
                    .unwrap_or(f64::NAN)
                    .is_finite()
                {
                    eprintln!(
                        "      telemetry heating_setpoint_c={:.1}",
                        telemetry.get("heating_setpoint_c").unwrap()
                    );
                }
            }
        }

        eprintln!("\n=== DIAGNOSTIC COMPLETE ===");
        if output.exists() {
            eprintln!("  CSV output at: {}", output.display());
        }
    }

    // ── helpers ──────────────────────────────────────────────────────────

    /// Returns the first zone temperature from the environment phase capture.
    fn first_zone_temp_env(snap: &hares_core::observer::StepSnapshot) -> f64 {
        snap.phases
            .post_environment
            .as_ref()
            .and_then(|e| e.zone_temps_c.first().map(|(_, t)| *t))
            .unwrap_or(f64::NAN)
    }

    /// Returns the first zone temperature from the zone-update phase capture.
    fn first_zone_temp_update(snap: &hares_core::observer::StepSnapshot) -> f64 {
        snap.phases
            .post_zone_update
            .as_ref()
            .and_then(|z| z.zone_temps_c.first().map(|(_, t)| *t))
            .unwrap_or(f64::NAN)
    }

    /// Prints a table of step summaries for a given range.
    fn print_step_table(
        snapshots: &[hares_core::observer::StepSnapshot],
        label: &str,
        range: std::ops::Range<usize>,
    ) {
        eprintln!("\n=== {label} ===");
        eprintln!(
            "{:>4} {:>10} {:>7} {:>7}  {:>8} {:>12} {:>8}  {:>16} {:>16}",
            "idx", "time", "zone_C", "oat_C", "eq_name", "mode", "th_W", "h_sp_C", "c_sp_C"
        );

        for i in range {
            if let Some(snap) = snapshots.get(i) {
                let zone_c = first_zone_temp_env(snap);
                let update_c = first_zone_temp_update(snap);
                let oat_c = snap
                    .phases
                    .post_environment
                    .as_ref()
                    .map(|e| e.outdoor_temp_c)
                    .unwrap_or(f64::NAN);

                if let Some(phase) = snap.phases.post_thermal_equipment.as_ref() {
                    for eq in &phase.equipment {
                        let mode = eq.telemetry.get("operating_mode").unwrap_or(f64::NAN);
                        let thermal_w = eq.telemetry.get("thermal_output_w").unwrap_or(f64::NAN);
                        let elec_kw = eq.telemetry.get("electric_kw").unwrap_or(f64::NAN);
                        let fuel_w = eq.telemetry.get("fuel_input_w").unwrap_or(f64::NAN);
                        let eir = eq.telemetry.get("eir").unwrap_or(f64::NAN);
                        let heat_sp = eq.telemetry.get("heating_setpoint_c").unwrap_or(f64::NAN);
                        let cool_sp = eq.telemetry.get("cooling_setpoint_c").unwrap_or(f64::NAN);

                        eprintln!(
                            "{:>4} {:>10} {:>7.1} {:>7.1}  {:>8} {:>12.0} {:>8.1}  {:>16.1} {:>16.1}",
                            i,
                            snap.timestamp.format("%H:%M"),
                            zone_c,
                            oat_c,
                            &eq.name[..eq.name.len().min(8)],
                            mode,
                            thermal_w,
                            heat_sp,
                            cool_sp,
                        );

                        // Extra detail on first line of block
                        if mode > 0.5 || thermal_w > 0.0 {
                            eprintln!(
                                "                                   fuel_W={:.0}  elec_kW={:.3}  eir={:.4}  zone_post_update={:.1}",
                                fuel_w, elec_kw, eir, update_c
                            );
                        }
                    }
                } else {
                    eprintln!(
                        "{:>4} {:>10} {:>7.1} {:>7.1}  (no equipment)",
                        i,
                        snap.timestamp.format("%H:%M"),
                        zone_c,
                        oat_c
                    );
                }
            }
        }
    }

    /// Dumps full equipment state for a cold step.
    fn dump_cold_step(step_idx: usize, snap: &hares_core::observer::StepSnapshot) {
        let zone_c = first_zone_temp_env(snap);
        let update_c = first_zone_temp_update(snap);
        let oat_c = snap
            .phases
            .post_environment
            .as_ref()
            .map(|e| e.outdoor_temp_c)
            .unwrap_or(f64::NAN);

        eprintln!(
            "\n  ── COLD STEP {step_idx} ({}) ──────────────────────────────",
            snap.timestamp.format("%Y-%m-%d %H:%M")
        );
        eprintln!("    Zone temp (env phase):     {zone_c:.2} °C");
        eprintln!("    Zone temp (post update):   {update_c:.2} °C");
        eprintln!("    Outdoor temp:              {oat_c:.2} °C");

        // Environment phase
        if let Some(env) = snap.phases.post_environment.as_ref() {
            eprintln!("    All zone temps (env):      {:?}", env.zone_temps_c);
            eprintln!(
                "    GHI: {:.0} W/m²  Wind: {:.1} m/s  Mains: {:.1}°C",
                env.ghi_w_m2, env.wind_speed_m_s, env.mains_temp_c
            );
        }

        // Thermal equipment phase
        if let Some(phase) = snap.phases.post_thermal_equipment.as_ref() {
            eprintln!("    ── Thermal equipment ──");
            for eq in &phase.equipment {
                eprintln!("      Name:         {}", eq.name);
                eprintln!(
                    "      Type:         {} (end_use={:?})",
                    eq.equipment_type, eq.end_use
                );

                // All telemetry keys
                eprintln!("      Telemetry:");
                for (key, val) in eq.telemetry.iter() {
                    eprintln!("        {key:30} = {val:.6}");
                }

                // Contribution
                eprintln!("      Contribution:");
                eprintln!("        thermal:          {:?}", eq.contribution.thermal);
                eprintln!(
                    "        electrical_load:  {:.6} kW",
                    eq.contribution.electrical_load_kw
                );
                eprintln!(
                    "        electrical_gen:   {:.6} kW",
                    eq.contribution.electrical_gen_kw
                );
                if !eq.contribution.fuel_consumption_w.is_empty() {
                    eprintln!(
                        "        fuel_consumption: {:?}",
                        eq.contribution.fuel_consumption_w
                    );
                }
                if !eq.contribution.fluid.is_empty() {
                    for f in &eq.contribution.fluid {
                        eprintln!(
                            "        fluid (loop={:?}): flow={:.4} kg/s  supply={:.1}°C  return={:.1}°C",
                            f.loop_id, f.delta_flow_kg_s, f.supply_temp_c, f.return_temp_c
                        );
                    }
                }

                // Port declarations
                eprintln!("      Port declarations:");
                for pd in &eq.port_declarations {
                    eprintln!("        {:?}", pd);
                }

                eprintln!();
            }
        }

        // Solver phase
        if let Some(solvers) = snap.phases.post_solvers.as_ref() {
            eprintln!("    ── Solver ──");
            eprintln!(
                "      envelope_gains: window_solar={:.0}  opaque_solar={:.0}  infiltration={:.0}  \
                 internal={:.0}  hvac_heat={:.0}  hvac_cool={:.0}  port_conv={:.0}",
                solvers.envelope_gains.window_solar_w,
                solvers.envelope_gains.opaque_solar_lwr_w,
                solvers.envelope_gains.infiltration_w,
                solvers.envelope_gains.internal_gain_w,
                solvers.envelope_gains.hvac_heating_w,
                solvers.envelope_gains.hvac_cooling_w,
                solvers.envelope_gains.port_convective_w,
            );
        }
    }

    /// Delete temp file on drop.
    struct TmpGuard(PathBuf);
    impl Drop for TmpGuard {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }
}
