//! Pinning test: ResStock 2025.1 bldg0000002 (a gas boiler with
//! hydronic baseboard distribution and a room AC sharing the conditioned
//! zone, Connecticut weather G0900090, January).
//!
//! Two behaviours found by the investigation and fixed are pinned here:
//!
//! 1. The boiler's equivalent-battery telemetry publishes its energy window
//!    every step (OCHRE publishes each end use's EBM window every step
//!    regardless of the thermostat's call, HVAC.py:601-602 and 620-641).
//!    The window used to vanish to zeros whenever the thermostat FSM rested
//!    in Deadband, which at coarse resolution is every step of an ideal-run
//!    dwelling. (`boiler_ebm_window_publishes_while_delivering`, observe.)
//! 2. The room AC never reaches cooling mode in January: the ideal dispatch
//!    delivers the HVAC's own share of the zone sensible column (the
//!    non-HVAC gains no longer double-counted), so the zone tracks the
//!    heating setpoint profile instead of riding the gains' bias above it
//!    and crossing the AC's cooling turn-on. (`boiler_ebm_publishes_and_the_ac_never_cools_in_january`.)

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
        .unwrap_or_else(|| wdir.join(format!("{fips}_2018.csv")))
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

fn bldg2_january_config() -> DwellingConfig {
    let bldg_dir = project_root().join("tests/fixtures/resstock/2025.1/bldg0000002");
    DwellingConfig {
        hpxml_path: bldg_dir.join("home.xml"),
        schedule_path: Some(bldg_dir.join("in.schedules.csv")),
        weather_path: weather_for(&bldg_dir),
        defaults_path: Some(project_root().join("defaults")),
        sim_config: SimulationConfig {
            start_time: FixedOffset::west_opt(5 * 3600)
                .unwrap()
                .with_ymd_and_hms(2018, 1, 1, 0, 0, 0)
                .unwrap(),
            duration: Duration::days(7),
            time_res: Duration::hours(1),
            output_verbosity: 2,
            output_path: None,
            write_output: false,
            output_format: OutputFormat::Csv,
            output_chunk_size: 1024,
            setpoint_deadband_c: None,
            master_seed: 0,
            civil_timezone: None,
            site_location: hares_io::SiteLocationOverride::default(),
            retain_batches: false,
            rotation: hares_io::RotationPolicy::None,
            max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
        },
        overrides: None,
        bldg_id: 2,
        initialization_duration: Some(std::time::Duration::from_secs(7 * 24 * 3600)),
        resample_overrides: Some(hares_io::ResampleOverrides::ochre_compat()),
        patches: None,
    }
}

/// The zone tracks the heating setpoint profile (67-70 F, reconciled by the
/// deadband) and never crosses the AC's cooling turn-on: no cooling energy
/// is delivered in January.
#[test]
fn the_ac_never_cools_bldg2_in_january_and_the_zone_holds_the_heating_band() {
    let mut dwelling = Dwelling::from_config(bldg2_january_config()).expect("build dwelling");
    let results = dwelling.simulate().expect("simulate");
    assert!(results.steps.len() >= 168, "a week at 1 h");

    let mut zone_min_c = f64::INFINITY;
    let mut zone_max_c = f64::NEG_INFINITY;
    let mut cooling_kwh = 0.0;
    for step in &results.steps {
        for (_, temp_c) in &step.zone_temperatures_c {
            zone_min_c = zone_min_c.min(*temp_c);
            zone_max_c = zone_max_c.max(*temp_c);
        }
        cooling_kwh += step.hvac_cooling_w / 1000.0;
    }
    assert!(
        cooling_kwh.abs() < 1e-9,
        "the room AC must deliver no cooling in a heating-dominated January, \
         got {cooling_kwh:.3} kWh"
    );
    assert!(
        zone_max_c < 21.0,
        "the zone must stay below the AC's cooling turn-on (21.1 C) in January, \
         max {zone_max_c:.3}"
    );
    assert!(
        zone_min_c > 15.0,
        "the zone must not crash below the heating band in January, min {zone_min_c:.3}"
    );
}

/// The boiler's EBM window publishes on every step it delivers.
#[cfg(feature = "observe")]
#[test]
fn boiler_ebm_window_publishes_while_delivering() {
    use hares_types::telemetry_keys as tk;

    let mut dwelling = Dwelling::from_config(bldg2_january_config()).expect("build dwelling");
    dwelling.enable_observer(500);
    dwelling.simulate().expect("simulate");
    let snapshots = dwelling.drain_observations();
    assert!(
        snapshots.len() >= 160,
        "a week at 1 h: {} snapshots",
        snapshots.len()
    );

    let mut boiler_delivery_steps = 0;
    let mut boiler_delivered_with_ebm_window = 0;
    let mut ac_cooling_mode_steps = 0;

    for snap in &snapshots {
        let Some(phase) = snap.phases.post_thermal_equipment.as_ref() else {
            continue;
        };
        for eq in &phase.equipment {
            if eq.name == "Gas Boiler" {
                let delivering = eq
                    .telemetry
                    .get(tk::THERMAL_OUTPUT_W)
                    .is_some_and(|w| w > 0.0);
                if delivering {
                    boiler_delivery_steps += 1;
                    // The EBM window publishes while the unit delivers: the
                    // energy state (capacitance x (zone temp - 10 C)) is
                    // nonzero for any zone temp above 10 C and the max power
                    // is the rated capacity.
                    let energy = eq.telemetry.get(tk::EBM_ENERGY_KWH);
                    let max_power = eq.telemetry.get(tk::EBM_MAX_POWER_KW);
                    if energy.is_some_and(|e| e != 0.0) && max_power.is_some_and(|p| p > 0.0) {
                        boiler_delivered_with_ebm_window += 1;
                    }
                }
            }
            if eq.name == "Room AC"
                && eq
                    .telemetry
                    .get(tk::OPERATING_MODE)
                    .is_some_and(|m| m == 2.0)
            {
                ac_cooling_mode_steps += 1;
            }
        }
    }

    assert!(
        boiler_delivery_steps > 100,
        "a January week must call the boiler most steps, got {boiler_delivery_steps}"
    );
    assert_eq!(
        boiler_delivered_with_ebm_window, boiler_delivery_steps,
        "the boiler's EBM window must publish on every step it delivers \
         (OCHRE HVAC.py:601-602, 620-641): {boiler_delivered_with_ebm_window} of \
         {boiler_delivery_steps}"
    );
    assert_eq!(
        ac_cooling_mode_steps, 0,
        "the room AC must never reach cooling mode in a heating-dominated January"
    );
}
