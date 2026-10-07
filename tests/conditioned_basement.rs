//! A conditioned basement is conditioned space.
//!
//! OS-HPXML v1.12.0 merges every conditioned location ("basement - conditioned",
//! "crawlspace - conditioned") into the one conditioned space
//! (`geometry.rb` `create_or_get_space`, 1704-1716; `hpxml.rb`
//! `conditioned_locations`, 12311-12316): one thermal zone served by the
//! HVAC, the basement's surfaces, loads and ducts accounted there. OCHRE
//! keeps the finished basement a separate, unheated Foundation zone
//! (hpxml.py:673-684 builds it; no zone holds a setpoint) -- the
//! simplification this fix supersedes; the divergence is registered in
//! docs/alignment/DIVERGENCES.md.

#[path = "support/denver_offset.rs"]
mod denver_offset;

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use chrono::{Duration, TimeZone};
    use hares_core::{Dwelling, DwellingConfig, SimulationConfig};
    use hares_equipment::hvac::heating_config::IdealCapacityModeConfig;
    use hares_equipment::{
        EquipmentConfig, EquipmentRegistry, HvacSetpointConfig, IdealHvacConfig,
    };
    use hares_io::OutputFormat;
    use hares_types::{ScheduleSourceConfig, ZoneId};

    use super::denver_offset::denver_offset;

    /// The BEopt_example setpoints the conditioned-oracle tests use: 71 °F
    /// heating, 76 °F cooling, constant.
    const HEATING_SETPOINT_C: f64 = 20.0;
    const COOLING_SETPOINT_C: f64 = 24.4;
    /// The Ideal HVAC's default 1.0 °C hysteresis, plus a small step-lag
    /// allowance.
    const DEADBAND_C: f64 = 1.0;

    fn project_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn base_xml() -> PathBuf {
        project_root().join("tests/fixtures/hpxml/ochre_samples/base.xml")
    }

    /// One day, 15 January, hourly steps, on the given HPXML.
    fn january_config(output_path: PathBuf, hpxml_path: &Path) -> DwellingConfig {
        let denver = denver_offset();
        let start_time = denver.with_ymd_and_hms(2019, 1, 15, 0, 0, 0).unwrap();
        DwellingConfig {
            hpxml_path: hpxml_path.to_path_buf(),
            schedule_path: Some(project_root().join("data/examples/BEopt_example_schedule.csv")),
            weather_path: project_root()
                .join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
            defaults_path: Some(project_root().join("defaults")),
            sim_config: SimulationConfig {
                start_time,
                duration: Duration::hours(24),
                time_res: Duration::minutes(1),
                output_verbosity: 6,
                output_path: Some(output_path),
                write_output: false,
                output_format: OutputFormat::Csv,
                output_chunk_size: 1024,
                setpoint_deadband_c: None,
                master_seed: 42,
                civil_timezone: None,
                site_location: hares_io::SiteLocationOverride::default(),
                retain_batches: false,
                rotation: hares_io::RotationPolicy::None,
                max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
            },
            overrides: None,
            bldg_id: 1,
            initialization_duration: None,
            resample_overrides: Some(hares_io::ResampleOverrides::ochre_compat()),
            patches: None,
        }
    }

    fn ideal_hvac_config(zone_id: ZoneId) -> EquipmentConfig {
        let heating = [HEATING_SETPOINT_C; 24];
        let cooling = [COOLING_SETPOINT_C; 24];
        EquipmentConfig::from_typed(
            "Ideal HVAC".to_string(),
            "Ideal HVAC".to_string(),
            IdealHvacConfig {
                equipment_id: None,
                zone_id: Some(zone_id.0),
                ideal_capacity_mode: Some(IdealCapacityModeConfig::On),
                heating_capacity_w: Some(10_000.0),
                cooling_capacity_w: Some(10_000.0),
                setpoint: HvacSetpointConfig {
                    heating_setpoint_source: Some(ScheduleSourceConfig::DailyProfile {
                        weekday: heating,
                        weekend: heating,
                        month_multipliers: [1.0; 12],
                        max_value: 1.0,
                    }),
                    cooling_setpoint_source: Some(ScheduleSourceConfig::DailyProfile {
                        weekday: cooling,
                        weekend: cooling,
                        month_multipliers: [1.0; 12],
                        max_value: 1.0,
                    }),
                    ..HvacSetpointConfig::default()
                },
                ..IdealHvacConfig::default()
            },
        )
        .unwrap()
    }

    /// Runs base.xml (or a variant) for the January day with an ideal HVAC
    /// serving the conditioned zone, and returns, per step, the zone
    /// temperatures and the HVAC's delivered thermal power.
    fn run_with_ideal_hvac(hpxml_path: &Path) -> (Vec<Vec<(ZoneId, f64)>>, Vec<f64>) {
        let output_dir = tempfile::tempdir().expect("temp dir");
        let config = january_config(output_dir.path().join("out.csv"), hpxml_path);
        let mut dwelling = Dwelling::from_config(config).expect("Dwelling::from_config");

        dwelling
            .clear_equipment()
            .expect("clear must refresh caches");
        let hvac = EquipmentRegistry::new()
            .create("Ideal HVAC", ideal_hvac_config(ZoneId(1)))
            .expect("create Ideal HVAC");
        dwelling.add_equipment(hvac).expect("add_equipment");

        let n_steps = 24 * 60;
        let mut zone_temps = Vec::with_capacity(n_steps);
        let mut hvac_w = Vec::with_capacity(n_steps);
        for _ in 0..n_steps {
            dwelling.step().expect("dwelling.step");
            zone_temps.push(
                dwelling
                    .latest_env()
                    .zones
                    .iter()
                    .map(|z| (z.id, z.temperature_c))
                    .collect(),
            );
            if let Some(eq) = dwelling.equipment().first() {
                let t = eq.telemetry();
                hvac_w.push(t.get("thermal_output_w").unwrap_or(f64::NAN));
            }
        }
        (zone_temps, hvac_w)
    }

    /// The conditioned basement is part of the one conditioned zone
    /// (OS-HPXML `create_or_get_space`, geometry.rb:1704-1716): the parse
    /// builds no Foundation zone for it, its surfaces (the foundation wall,
    /// the slab) name the conditioned zone, and the merged zone is held at
    /// the heating setpoint through a January day.
    #[test]
    fn conditioned_basement_is_held_at_the_setpoint() {
        // The merge, at the parse level: no Foundation zone, and the
        // basement's surfaces are the conditioned zone's.
        let building = hares_io::parse_hpxml(&base_xml()).expect("base.xml parses");
        assert!(
            !building
                .zones
                .iter()
                .any(|z| z.zone_type == hares_io::hpxml::ZoneType::Foundation),
            "base.xml's conditioned basement must not build a separate (unheated) \
             Foundation zone: OS-HPXML merges it into the conditioned space"
        );
        for b in &building.boundaries {
            let names_basement = b.id.to_ascii_lowercase().contains("foundation")
                || b.id.to_ascii_lowercase().contains("slab");
            if names_basement {
                assert_eq!(
                    b.interior_zone,
                    Some(hares_io::hpxml::ZoneType::Conditioned),
                    "boundary {} (interior adjacent to the conditioned basement) must be a \
                     conditioned-zone surface",
                    b.id
                );
            }
        }

        // Held at setpoint: the merged zone tracks the heating setpoint
        // (base.xml's 68 °F heating season setpoint, which the building's
        // setpoint profiles impose on the ideal HVAC) within the deadband
        // through the January day.
        let (zone_temps, _hvac_w) = run_with_ideal_hvac(&base_xml());
        assert!(!zone_temps.is_empty(), "the run recorded no zone updates");
        let n_zones = zone_temps[0].len();
        assert_eq!(
            n_zones, 2,
            "base.xml models the merged conditioned zone plus the outdoor boundary \
             zone only; a third zone means the basement was built unheated"
        );
        for (step, temps) in zone_temps.iter().enumerate().skip(60) {
            let (id, t) = temps[0];
            assert_eq!(id, ZoneId(1), "the first zone is the conditioned one");
            assert!(
                (t - HEATING_SETPOINT_C).abs() <= DEADBAND_C + 0.5,
                "step {step}: the conditioned basement's zone at {t:.2} °C is outside the \
                 heating setpoint's {HEATING_SETPOINT_C} °C deadband ± {DEADBAND_C} °C; \
                 the basement's zone is not conditioned space"
            );
        }
    }

    /// The basement's loads reach the HVAC: with the basement conditioned
    /// (merged), the ideal HVAC's delivered heating over the January day
    /// differs from the same model with the basement declared unconditioned
    /// (a separate unheated zone). Before the merge the flag was unread and
    /// the two models were identical, so the delta was exactly zero.
    #[test]
    fn conditioned_basement_loads_reach_the_hvac() {
        let output_dir = tempfile::tempdir().expect("temp dir");
        let patched = output_dir.path().join("base-unconditioned.xml");
        let xml = std::fs::read_to_string(base_xml()).expect("read base.xml");
        let patched_xml = xml
            .replace(
                "<Conditioned>true</Conditioned>",
                "<Conditioned>false</Conditioned>",
            )
            .replace(
                "<Conditioned> true </Conditioned>",
                "<Conditioned> false </Conditioned>",
            );
        assert_ne!(xml, patched_xml, "base.xml must carry a <Conditioned> flag");
        std::fs::write(&patched, patched_xml).expect("write patched base.xml");

        let (merged_temps, merged_w) = run_with_ideal_hvac(&base_xml());
        let (separate_temps, separate_w) = run_with_ideal_hvac(&patched);

        let merged_wh: f64 = merged_w.iter().sum();
        let separate_wh: f64 = separate_w.iter().sum();
        eprintln!(
            "[od39] HVAC heating over the January day: merged {merged_wh:.0} Wh, \
             separate (unconditioned basement) {separate_wh:.0} Wh, \
             delta {:.0} Wh; zone temps merged [{}], separate [{}]",
            merged_wh - separate_wh,
            merged_temps
                .first()
                .map(|z| format!("{:?}", z.iter().map(|(_, t)| *t).collect::<Vec<_>>()))
                .unwrap_or_default(),
            separate_temps
                .first()
                .map(|z| format!("{:?}", z.iter().map(|(_, t)| *t).collect::<Vec<_>>()))
                .unwrap_or_default(),
        );

        assert!(
            (merged_wh - separate_wh).abs() > 100.0,
            "the conditioned-basement flag did not move the HVAC's delivered energy \
             (merged {merged_wh:.0} Wh vs separate {separate_wh:.0} Wh): the basement's \
             loads never reach the HVAC"
        );
    }
}
