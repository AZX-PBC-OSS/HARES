//! OD-38's pinning test: cycling HVAC delivers the fraction of the step the
//! zone needs, so the zone holds its setpoint band instead of swinging
//! whole-step capacity around it.
//!
//! `base.xml` (the gas furnace and the central AC sharing the conditioned
//! zone, heating setpoint 20 C, cooling setpoint 25.56 C, Denver TMY3) runs
//! in January (the furnace's class) and July (the AC's class) at 60 s (the
//! cycling path: the thermostat FSM's band-relative runtime fraction), 900 s
//! and 3600 s (the ideal path: the solver's capacity). Before the fix a
//! cycling unit ran whole steps at full capacity or off; the pin has two
//! parts: the units modulate (the runtime fraction strictly between 0 and 1
//! on most delivering steps, observe-gated) and the zone holds the comfort
//! envelope at every resolution with tolerances stated per resolution.

use std::path::PathBuf;

use chrono::{Duration, FixedOffset, TimeZone};
use hares_core::{Dwelling, DwellingConfig, SimulationConfig};
use hares_io::OutputFormat;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn base_xml_config(month: u32, time_res_s: i64) -> DwellingConfig {
    let (start_month_day, offset_hours) = match month {
        1 => ((1, 1), 7),
        7 => ((7, 1), 6),
        _ => panic!("test months are January and July"),
    };
    DwellingConfig {
        hpxml_path: project_root().join("tests/fixtures/hpxml/ochre_samples/base.xml"),
        schedule_path: Some(project_root().join("data/examples/BEopt_example_schedule.csv")),
        weather_path: project_root().join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        defaults_path: Some(project_root().join("defaults")),
        sim_config: SimulationConfig {
            start_time: FixedOffset::east_opt(offset_hours * 3600)
                .unwrap()
                .with_ymd_and_hms(2018, start_month_day.0, start_month_day.1, 0, 0, 0)
                .unwrap(),
            duration: Duration::days(5),
            time_res: Duration::seconds(time_res_s),
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
        bldg_id: 1,
        initialization_duration: Some(std::time::Duration::from_secs(24 * 3600)),
        resample_overrides: Some(hares_io::ResampleOverrides::ochre_compat()),
        patches: None,
    }
}

/// The conditioned zone's min and max over the run.
fn conditioned_zone_range(dwelling: &mut Dwelling) -> (f64, f64) {
    let results = dwelling.simulate().expect("simulate");
    let mut zone_min_c = f64::INFINITY;
    let mut zone_max_c = f64::NEG_INFINITY;
    for step in &results.steps {
        // The conditioned zone only: the unconditioned attic's excursions
        // are not the HVAC's holding band.
        for (zone, temp_c) in &step.zone_temperatures_c {
            if zone.0 != 1 {
                continue;
            }
            zone_min_c = zone_min_c.min(*temp_c);
            zone_max_c = zone_max_c.max(*temp_c);
        }
    }
    (zone_min_c, zone_max_c)
}

/// The zone stays inside the comfort envelope the two units hold: the
/// heating setpoint 20 C (the furnace's band floor 19.2 C) and the cooling
/// setpoint 25.56 C (the AC's band top 26.36 C). Tolerances, stated per
/// resolution: 0.6 C at 60 s and 900 s (the band-relative fraction's
/// within-step transient, the ideal dispatch's persistence error at 900 s);
/// 1.7 C at 3600 s, where the ideal dispatch's non-HVAC estimate rides the
/// hour's solar ramp and the July peak runs the AC at its capacity limit
/// (the same plateau the whole-step cycling produced). Between the bands the
/// zone free-floats on its gains with both units correctly off.
#[test]
fn base_xml_zone_holds_the_comfort_envelope_at_every_resolution() {
    const TOLERANCE_BY_RES_S: [(i64, f64); 3] = [(60, 0.6), (900, 0.6), (3600, 1.7)];
    const LOW_C: f64 = 19.2;
    const HIGH_C: f64 = 26.36;
    for (month, season) in [(1, "heating"), (7, "cooling")] {
        for (time_res_s, tolerance_c) in TOLERANCE_BY_RES_S {
            let mut dwelling = Dwelling::from_config(base_xml_config(month, time_res_s))
                .unwrap_or_else(|err| panic!("{season} at {time_res_s} s: build: {err}"));
            let (zone_min_c, zone_max_c) = conditioned_zone_range(&mut dwelling);
            assert!(
                zone_min_c >= LOW_C - tolerance_c && zone_max_c <= HIGH_C + tolerance_c,
                "{season} at {time_res_s} s: the zone must hold the comfort envelope \
                 ({:.2}..{:.2} C) within the stated {tolerance_c} C tolerance, got \
                 {zone_min_c:.3}..{zone_max_c:.3} C",
                LOW_C,
                HIGH_C
            );
        }
    }
}

/// The cycling path's delivered energy matches the continuous-capacity
/// integral: the same July week at 60 s (the cycling fraction) and at 900 s
/// (the ideal capacity, the continuous hold) delivers the same cooling
/// energy within 5 % - the part-load degradation's share plus the
/// resolution's sampling difference.
#[test]
fn cycling_path_energy_matches_the_ideal_paths_integral_over_a_week() {
    let mut energy_by_res = [0.0_f64; 2];
    for (i, time_res_s) in [60_i64, 900].into_iter().enumerate() {
        let mut dwelling = Dwelling::from_config(base_xml_config(7, time_res_s)).expect("build");
        let results = dwelling.simulate().expect("simulate");
        let dt_hours = time_res_s as f64 / 3600.0;
        for step in &results.steps {
            energy_by_res[i] += step.hvac_cooling_w / 1000.0 * dt_hours;
        }
    }
    let (cycling_kwh, ideal_kwh) = (energy_by_res[0], energy_by_res[1]);
    let rel = (cycling_kwh - ideal_kwh) / ideal_kwh;
    assert!(
        rel.abs() < 0.05,
        "the cycling path's cooling energy ({cycling_kwh:.2} kWh) must match the \
         ideal path's integral ({ideal_kwh:.2} kWh) within 5 %, got {rel:.3}"
    );
}

/// The cycling units modulate: at 60 s the furnace's and the AC's runtime
/// fractions are strictly inside (0, 1) on most delivering steps, and the
/// AC's runtime fraction carries the part-load degradation (RTF = PLR/PLF,
/// EnergyPlus DXCoils.cc:9859) rather than equalling the part-load ratio.
#[cfg(feature = "observe")]
#[test]
fn cycling_units_modulate_their_runtime_fraction_at_60s() {
    use hares_types::telemetry_keys as tk;

    // January: the furnace.
    let mut dwelling = Dwelling::from_config(base_xml_config(1, 60)).expect("build");
    dwelling.enable_observer(6000);
    dwelling.simulate().expect("simulate");
    let snapshots = dwelling.drain_observations();
    let mut furnace_on = 0;
    let mut furnace_modulating = 0;
    let mut furnace_rtf_full = 0;
    for snap in &snapshots {
        let Some(phase) = snap.phases.post_thermal_equipment.as_ref() else {
            continue;
        };
        for eq in &phase.equipment {
            if eq.name.to_lowercase().contains("furnace") {
                let rtf = eq.telemetry.get(tk::RUNTIME_FRACTION).unwrap_or(0.0);
                if rtf > 0.0 {
                    furnace_on += 1;
                    if (0.0..1.0).contains(&rtf) {
                        furnace_modulating += 1;
                    }
                    if (rtf - 1.0).abs() < 1e-9 {
                        furnace_rtf_full += 1;
                    }
                }
            }
        }
    }
    assert!(
        furnace_on > 100,
        "a January week must run the furnace on many steps: {furnace_on} of 6000"
    );
    assert!(
        furnace_modulating > furnace_rtf_full,
        "the cycling furnace must modulate (the runtime fraction strictly inside \
         (0,1)) on most delivering steps: {furnace_modulating} of {furnace_on}, \
         full-duty {furnace_rtf_full}"
    );
    assert!(
        furnace_rtf_full > 0,
        "the coldest hours must still command the full fraction: {furnace_rtf_full}"
    );

    // July: the AC's runtime fraction = PLR / PLF with the degradation
    // coefficient 0.25, so a part-load step's RTF exceeds its PLR.
    let mut dwelling = Dwelling::from_config(base_xml_config(7, 60)).expect("build");
    dwelling.enable_observer(6000);
    dwelling.simulate().expect("simulate");
    let snapshots = dwelling.drain_observations();
    let mut ac_checked = 0;
    for snap in &snapshots {
        let Some(phase) = snap.phases.post_thermal_equipment.as_ref() else {
            continue;
        };
        for eq in &phase.equipment {
            if !eq.name.to_lowercase().contains("air conditioner") {
                continue;
            }
            let rtf = eq.telemetry.get(tk::RUNTIME_FRACTION).unwrap_or(0.0);
            let plr = eq.telemetry.get(tk::PART_LOAD_RATIO).unwrap_or(0.0);
            let plf = eq.telemetry.get(tk::PART_LOAD_FACTOR).unwrap_or(1.0);
            if rtf > 0.0 && plf > 0.0 {
                ac_checked += 1;
                assert!(
                    (rtf - plr / plf).abs() < 1e-6,
                    "the AC's runtime fraction must be PLR/PLF (the part-load \
                     degradation, EnergyPlus DXCoils.cc:9859): rtf={rtf} plr={plr} plf={plf}"
                );
            }
        }
    }
    assert!(
        ac_checked > 0,
        "the July run must deliver AC cooling to pin the degradation relation"
    );
}
