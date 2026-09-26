//! Regression coverage for the EV pack thermal model.
//!
//! The EV equipment reimplemented the stationary Battery equipment's pack
//! thermal architecture, and the reimplementation diverged on three counts:
//!
//! 1. Pack thermal mass defaulted to 20 kJ/K for a 75 kWh pack — ~24x below
//!    the physically grounded ~480 kJ/K (≈6.4-6.7 kJ/K per kWh of capacity,
//!    the density the Battery model already carries with citations). With
//!    UA = 4 W/K the pack relaxes to ambient with a ~1.4 h time constant, so
//!    every sub-freezing night freezes the pack to ambient and the 0 °C
//!    cold-charge cutoff (real physics — Li-plating protection) blacks out
//!    the entire charge window.
//! 2. The entire AC→DC charger conversion loss ((1−η)·P ≈ 1.15 kW at 11.5 kW
//!    Level 2) was injected into the pack as heat instead of the physical
//!    I²R cell heating (~O(100 W) at Level 2 currents), driving the pack to
//!    165-281 °C on routine sessions and throwing the Arrhenius degradation
//!    model out of its validity domain (negative capacity fade).
//! 3. The battery heater's heat was applied only while charge power flowed,
//!    so a pack whose charging is derated to zero by the cold cutoff could
//!    not be preconditioned by any configuration — the heater billed energy
//!    without warming anything.
//!
//! Four invariants pin the class, each from a different face:
//!
//! - `l2_charge_session_keeps_pack_temperature_physical` — loss
//!   attribution: a Level 2 session must not cook the pack (defect 2).
//! - `battery_heater_warms_pack_while_cold_derate_blocks_charging` —
//!   preconditioning must work precisely when charge power is derated to
//!   zero, which is when it is needed (defect 3).
//! - `parked_pack_stays_above_charge_cutoff_after_a_cold_night` — pack
//!   thermal inertia is day-scale, not hour-scale: one sub-freezing night
//!   must not bring a warm pack down to the charge cutoff (defect 1).
//! - `nightly_strategy_keeps_ev_charged_through_a_sustained_cold_snap` —
//!   the integrated observable contract through the real dwelling assembly
//!   path: a plugged-in EV under a Nightly strategy must keep charging and
//!   keep driving through sustained sub-freezing weather (all three
//!   defects together; real vehicles precondition while plugged in, and
//!   OCHRE — the feature floor — has no cold-charge block at all).

use std::path::PathBuf;

use chrono::{Duration, FixedOffset, TimeZone};
use hares_core::{DwellingConfig, SimStatus, SimulationConfig, SimulationEngine};
use hares_equipment::{Equipment, EvConfig};
use hares_io::OutputFormat;

mod common;

use common::ev_charge_days_and_peak_cancelled;

/// 900 s timestep → 96 rows per simulated day in the output CSV.
const STEPS_PER_DAY: usize = 96;
/// A 900 s step is 0.25 h (900 s / 3600 s per hour) — kWh = kW × 0.25.
const STEP_HOURS: f64 = 0.25;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// ResStock bldg0000007 fixture directory.
fn fixture_dir() -> PathBuf {
    project_root().join("tests/fixtures/resstock/2025.1/bldg0000007")
}

/// Injects a 75 kWh / 11.5 kW Level-2 EV into the fixture's `<Systems>`
/// element as raw HPXML text carrying a fresh SystemIdentifier — the
/// reported scenario's entry path. The EV reaches the engine through the
/// same real assembly path as any other equipment.
fn inject_ev(hpxml: &str) -> String {
    let ev = concat!(
        "<ElectricVehicles>",
        "<ElectricVehicle>",
        "<SystemIdentifier id=\"ReproEV1\"/>",
        "<ChargingLevel>Level 2</ChargingLevel>",
        "<MaxChargingPower>11.5</MaxChargingPower>",
        "<BatteryCapacity><Value>75</Value><Units>kWh</Units></BatteryCapacity>",
        "</ElectricVehicle>",
        "</ElectricVehicles>"
    );
    let marker = "</Systems>";
    assert!(hpxml.contains(marker), "fixture must contain </Systems>");
    hpxml.replacen(marker, &format!("{ev}{marker}"), 1)
}

/// Writes a synthetic constant-temperature weather CSV (hourly, the fixture
/// weather format; rows run 01:00..00:00 with 00:00 belonging to the
/// following date). A flat sub-freezing profile removes the daytime
/// ambient rescue a diurnal cycle would provide, isolating the sustained
/// cold-snap behavior.
fn write_constant_weather(dir: &std::path::Path, days: usize, temp_c: f64) -> PathBuf {
    let path = dir.join("constant_weather.csv");
    let mut out = String::from(
        "date_time,Dry Bulb Temperature [°C],Relative Humidity [%],Wind Speed [m/s],Wind Direction [Deg],Global Horizontal Radiation [W/m2],Direct Normal Radiation [W/m2],Diffuse Horizontal Radiation [W/m2]\n",
    );
    for day in 0..days {
        // 2018-01-01 + `day` days, manual arithmetic (non-leap year; the
        // scenarios stay within Jan/Feb).
        let dom = 1 + day as u32;
        let (month, day_of_month) = if dom <= 31 { (1, dom) } else { (2, dom - 31) };
        for hour in 0..24u32 {
            let (m, d, h) = if hour == 23 {
                let nd = day_of_month + 1;
                if month == 1 && nd > 31 {
                    (2, 1, 0)
                } else {
                    (month, nd, 0)
                }
            } else {
                (month, day_of_month, hour + 1)
            };
            out.push_str(&format!(
                "2018-{m:02}-{d:02} {h:02}:00:00,{temp_c:.2},80.0,3.0,180.0,0.0,0.0,0.0\n"
            ));
        }
    }
    std::fs::write(&path, out).expect("write synthetic weather");
    path
}

/// Builds the reported EV (75 kWh / 11.5 kW Level 2) with the probe's
/// thermal knobs; every other field takes the production default.
fn probe_ev(
    battery_temp_c: Option<f64>,
    heater_power_w: Option<f64>,
    heater_threshold_c: Option<f64>,
    initial_connection_state: Option<&str>,
    initial_soc: Option<f64>,
) -> (hares_equipment::ev::Ev, hares_equipment::EquipmentConfig) {
    let cfg = hares_equipment::EquipmentConfig::from_typed(
        "EV".to_string(),
        "EV".to_string(),
        EvConfig {
            equipment_id: None,
            capacity_kwh: 75.0,
            charging_level: Some("Level 2".to_string()),
            max_charging_power_kw: 11.5,
            charging_efficiency: None,
            l1_current_a: None,
            l1_voltage_v: None,
            soc_max: None,
            initial_soc,
            battery_temp_c,
            min_charge_temp_c: None,
            full_power_temp_c: None,
            heater_power_w,
            heater_threshold_c,
            thermal_mass_j_per_k: None,
            ua_w_per_k: None,
            n_series: None,
            n_parallel: None,
            cell_resistance_ohm: None,
            v2l_enabled: None,
            v2l_soc_reserve: None,
            v2l_max_discharge_kw: None,
            v2g_enabled: None,
            v2g_soc_reserve: None,
            v2g_max_discharge_kw: None,
            chemistry: None,
            fuel_economy_kwh_per_mi: None,
            ready_soc: None,
            charging_strategy: None,
            plug_in_policy: None,
            power_limit_kw: None,
            initial_connection_state: initial_connection_state.map(|s| s.to_string()),
            power_factor: None,
            charger_capacity_kva: None,
            cc_cv_transition_soc: None,
            charging_priority: None,
            discharge_respects_deadline: true,
        },
    )
    .expect("typed EV config");
    let ev = hares_equipment::ev::Ev::new(cfg.clone());
    (ev, cfg)
}

/// A test environment at `outdoor_temp_c` with `current_time` pinned to the
/// given local instant (UTC-10, the fixture's own civil zone).
fn env_at(
    outdoor_temp_c: f64,
    (y, mo, d, h): (i32, u32, u32, u32),
) -> hares_types::EnvironmentState {
    let mut env = hares_core::actor::testing::TestEnvBuilder::new()
        .outdoor_temp(outdoor_temp_c)
        .hour(h as u8)
        .build();
    env.current_time = FixedOffset::west_opt(10 * 3600)
        .expect("UTC-10 offset is valid")
        .with_ymd_and_hms(y, mo, d, h, 0, 0)
        .unwrap();
    env
}

/// One 900 s equipment step against `env`, advancing the environment clock.
fn step_900s<E: Equipment>(equipment: &mut E, env: &mut hares_types::EnvironmentState) {
    let mut ports = hares_types::PortSlots::default();
    equipment
        .step(env, std::time::Duration::from_secs(900), &mut ports)
        .expect("equipment step succeeds");
    env.current_time += Duration::seconds(900);
}

/// A Level 2 (11.5 kW) charge session must keep the pack in the physical
/// temperature range. Pack self-heating during Level 2 charging is I²R from
/// cell/pack resistance — O(100 W) at Level 2 currents into a ~480 kJ/K
/// pack, i.e. a low single-digit kelvin rise over a full session. The
/// defect this pins: the charger's AC→DC conversion loss ((1−η)·P ≈ 1.15
/// kW) was attributed to the pack as heat, producing +40-50 K per 900 s
/// step and 165-281 °C peaks on routine sessions — thermal-runaway
/// territory that also drives the Arrhenius degradation model out of its
/// validity domain (measured negative capacity fade). 40 °C from a 20 °C
/// start is a generous ~20x bound on the physical rise.
#[test]
fn l2_charge_session_keeps_pack_temperature_physical() {
    // Start pinned explicitly: the documented "from a 20 °C start" premise
    // must not rest on the coincidence that ambient-aware initialization
    // would happen to resolve the same value from this probe's 20 °C
    // ambient.
    let (mut ev, cfg) = probe_ev(Some(20.0), None, None, Some("HomePluggedIn"), Some(0.5));
    let mut env = env_at(20.0, (2018, 1, 1, 22));
    ev.init(&cfg, &env).expect("EV init");
    ev.apply_signal(&hares_types::ControlSignal::SOCTarget {
        target_soc: 0.9,
        min_soc: None,
        max_soc: None,
    })
    .expect("SOCTarget signal");

    let mut t_peak = f64::NEG_INFINITY;
    for _ in 0..16 {
        step_900s(&mut ev, &mut env);
        let t = ev.telemetry().get("battery_temp_c").unwrap_or(f64::NAN);
        t_peak = t_peak.max(t);
    }
    let soc_end = ev.telemetry().get("soc").unwrap_or(f64::NAN);
    // The session must actually have run — otherwise the temperature bound
    // would hold vacuously with no charge power applied.
    assert!(
        soc_end >= 0.85,
        "the probe session must charge the pack toward its 0.9 target for \
         the temperature bound to be meaningful; ended at soc {soc_end}"
    );
    assert!(
        t_peak <= 40.0,
        "EV pack temperature during a Level 2 charge session must stay \
         physical; peaked at {t_peak:.1} C from a 20 C start — charger \
         conversion losses are being misattributed to the pack as heat and/or \
         the pack thermal mass is orders of magnitude too small"
    );
}

/// A configured battery heater must warm the pack while the cold-charge
/// derate holds charge power at zero — that is the preconditioning path
/// that makes sub-0 °C charging possible at all, and it is needed exactly
/// when charging is blocked. The defect this pins: heater heat was applied
/// only inside the charge-power-positive branch of the thermal update, so
/// below the minimum charge temperature the heater drew and billed power
/// without warming the pack by a single kelvin.
#[test]
fn battery_heater_warms_pack_while_cold_derate_blocks_charging() {
    let (mut ev, cfg) = probe_ev(
        Some(-5.0),
        Some(1500.0),
        Some(5.0),
        Some("HomePluggedIn"),
        Some(0.5),
    );
    let mut env = env_at(-5.0, (2018, 1, 6, 22));
    ev.init(&cfg, &env).expect("EV init");
    ev.apply_signal(&hares_types::ControlSignal::SOCTarget {
        target_soc: 0.9,
        min_soc: None,
        max_soc: None,
    })
    .expect("SOCTarget signal");

    let start_c = ev
        .telemetry()
        .get("battery_temp_c")
        .expect("battery_temp_c telemetry before stepping");
    let mut billed_kwh = 0.0;
    for _ in 0..16 {
        // 4 h of preconditioning at 1.5 kW.
        step_900s(&mut ev, &mut env);
        billed_kwh += ev.telemetry().get("active_power_kw").unwrap_or(0.0) * STEP_HOURS;
    }
    let end_c = ev
        .telemetry()
        .get("battery_temp_c")
        .expect("battery_temp_c telemetry after stepping");

    assert!(
        billed_kwh > 0.5,
        "the heater should draw power while preconditioning, billed \
         {billed_kwh:.2} kWh"
    );
    assert!(
        end_c > start_c + 1.0,
        "a running battery heater must warm the pack even while the \
         cold-charge derate holds charge power at zero; pack went \
         {start_c:.2} C -> {end_c:.2} C while billing {billed_kwh:.2} kWh of \
         heater energy that warmed nothing"
    );
}

/// Pack thermal inertia is day-scale, not hour-scale. A real 75 kWh pack
/// carries ~480 kJ/K (≈6.4-6.7 kJ/K per kWh — the density the stationary
/// Battery model defaults to with citations); against a few W/K of ambient
/// coupling its relaxation time constant is on the order of a day, so one
/// sub-freezing night cools a warm pack by a handful of kelvin. The defect
/// this pins: the 20 kJ/K default gave a ~1.4 h time constant, so the pack
/// flash-froze to ambient overnight and every sub-freezing night became a
/// full-window charging blackout. Parked and unplugged (no charging, no
/// heater — pure ambient relaxation), a pack at 20 °C must still sit above
/// the 0 °C minimum charge temperature after 8 h at −7 °C, while cooling
/// measurably toward ambient (the relaxation must exist, just not at
/// hour-scale).
#[test]
fn parked_pack_stays_above_charge_cutoff_after_a_cold_night() {
    let (mut ev, cfg) = probe_ev(Some(20.0), None, None, Some("Disconnected"), None);
    let mut env = env_at(-7.0, (2018, 1, 5, 22));
    ev.init(&cfg, &env).expect("EV init");

    let start_c = ev
        .telemetry()
        .get("battery_temp_c")
        .expect("battery_temp_c telemetry before stepping");
    // One night: 8 h at a constant −7 °C ambient.
    for _ in 0..32 {
        step_900s(&mut ev, &mut env);
    }
    let end_c = ev
        .telemetry()
        .get("battery_temp_c")
        .expect("battery_temp_c telemetry after stepping");

    assert!(
        end_c < start_c,
        "the pack must relax toward ambient overnight (UA > 0); went \
         {start_c:.2} C -> {end_c:.2} C at -7 C ambient"
    );
    assert!(
        end_c > 0.0,
        "a warm (20 C) parked pack must stay above the 0 C minimum charge \
         temperature after one -7 C night — day-scale thermal inertia, not \
         hour-scale; went {start_c:.2} C -> {end_c:.2} C in 8 h"
    );
}

/// The integrated observable contract, through the real dwelling assembly
/// path: the reported fixture home with the EV injected as raw HPXML, a
/// Nightly(22:00-06:00, target 0.90) strategy, and a sustained −7 °C cold
/// snap. A plugged-in EV must keep charging on most days and must never
/// strand its driver: real vehicles precondition the pack while plugged in
/// (the BMS sub-0 °C charge cutoff is paired with pack heating), and OCHRE
/// — the feature floor — has no cold-charge block at all. Pre-fix this run
/// charges on zero of 45 days, drains the pack within four days, and
/// cancels every subsequent trip.
#[test]
fn nightly_strategy_keeps_ev_charged_through_a_sustained_cold_snap() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let weather = write_constant_weather(tmp.path(), 46, -7.0);
    let src = std::fs::read_to_string(fixture_dir().join("home.xml")).expect("fixture home.xml");
    let hpxml_path = tmp.path().join("bldg0000007_ev.xml");
    std::fs::write(&hpxml_path, inject_ev(&src)).expect("write injected HPXML");
    let output_path = tmp.path().join("ev_cold_snap_45day.csv");

    let nightly = serde_json::json!({"Nightly": {
        "off_peak_start_hour": 22.0,
        "off_peak_end_hour": 6.0,
        "target_soc": 0.90,
    }});
    let overrides = serde_json::json!({
        "EV": { "charging_strategy": nightly.to_string() }
    });

    let sim = SimulationConfig {
        start_time: FixedOffset::west_opt(10 * 3600)
            .expect("UTC-10 offset is valid")
            .with_ymd_and_hms(2018, 1, 1, 0, 0, 0)
            .unwrap(),
        duration: Duration::days(45),
        time_res: Duration::seconds(900),
        output_verbosity: 5,
        write_output: true,
        output_path: Some(output_path.clone()),
        output_format: OutputFormat::Csv,
        output_chunk_size: 1024,
        setpoint_deadband_c: None,
        master_seed: 0,
        civil_timezone: None,
        site_location: hares_io::SiteLocationOverride::default(),
        retain_batches: false,
        rotation: hares_io::RotationPolicy::None,
    };
    let config = DwellingConfig {
        hpxml_path,
        schedule_path: fixture_dir().join("in.schedules.csv"),
        weather_path: weather,
        defaults_path: Some(project_root().join("defaults")),
        sim_config: sim,
        overrides: Some(overrides),
        bldg_id: 100,
        initialization_duration: None,
        resample_overrides: Some(hares_io::ResampleOverrides::ochre_compat()),
        patches: None,
    };
    let result = SimulationEngine::new()
        .run(config)
        .expect("engine.run should succeed");
    assert!(
        !matches!(result.status, SimStatus::Failed(_)),
        "simulation failed: {:?}",
        result.status
    );

    let (charge_days, cancelled_max) =
        ev_charge_days_and_peak_cancelled(&output_path, STEPS_PER_DAY);
    assert!(
        charge_days >= 30,
        "Nightly(22:00-06:00, target 0.90) in a sustained -7 C cold snap must \
         still charge on most of 45 days (preconditioning while plugged in); \
         got {charge_days} charge-days"
    );
    assert_eq!(
        cancelled_max, 0.0,
        "no trip should be cancelled: a plugged-in EV must not sit uncharged \
         until the pack cannot cover a 30-mile trip; drive_cancelled peaked \
         at {cancelled_max}"
    );
}

/// An overrides key that matches no equipment name — e.g. the HPXML
/// `SystemIdentifier` of a raw-injected EV (`"ReproEV1"`), which never
/// matches a spec name because overrides are matched by equipment name —
/// must fail the build loudly at the assembly boundary. Before the
/// validation existed it was a silent no-op: the strategy override quietly
/// missed and the vehicle ran its default behavior while the caller
/// believed their override applied (keying by an HPXML SystemIdentifier
/// instead of the equipment name is the natural mistake — the id lands in
/// `spec.system_id`, which the name-matching override channel never
/// consults).
#[test]
fn override_keyed_by_unknown_equipment_name_errors_loudly() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let weather = write_constant_weather(tmp.path(), 2, 10.0);
    let src = std::fs::read_to_string(fixture_dir().join("home.xml")).expect("fixture home.xml");
    let hpxml_path = tmp.path().join("bldg0000007_ev.xml");
    std::fs::write(&hpxml_path, inject_ev(&src)).expect("write injected HPXML");

    let nightly = serde_json::json!({"Nightly": {
        "off_peak_start_hour": 22.0,
        "off_peak_end_hour": 6.0,
        "target_soc": 0.90,
    }});
    let overrides = serde_json::json!({
        "ReproEV1": { "charging_strategy": nightly.to_string() }
    });

    let sim = SimulationConfig {
        start_time: FixedOffset::west_opt(10 * 3600)
            .expect("UTC-10 offset is valid")
            .with_ymd_and_hms(2018, 1, 1, 0, 0, 0)
            .unwrap(),
        duration: Duration::days(2),
        time_res: Duration::seconds(900),
        output_verbosity: 5,
        write_output: false,
        output_path: None,
        output_format: OutputFormat::Csv,
        output_chunk_size: 1024,
        setpoint_deadband_c: None,
        master_seed: 0,
        civil_timezone: None,
        site_location: hares_io::SiteLocationOverride::default(),
        retain_batches: false,
        rotation: hares_io::RotationPolicy::None,
    };
    let config = DwellingConfig {
        hpxml_path,
        schedule_path: fixture_dir().join("in.schedules.csv"),
        weather_path: weather,
        defaults_path: Some(project_root().join("defaults")),
        sim_config: sim,
        overrides: Some(overrides),
        bldg_id: 100,
        initialization_duration: None,
        resample_overrides: Some(hares_io::ResampleOverrides::ochre_compat()),
        patches: None,
    };
    let err = SimulationEngine::new()
        .run(config)
        .expect_err("an unknown override key must fail the build");
    let msg = err.to_string();
    assert!(
        msg.contains("unknown equipment override key 'ReproEV1'"),
        "the error must name the unknown key and the matching rule, got: {msg}"
    );
    assert!(
        msg.contains("EV"),
        "the error must list the overridable equipment names (the injected \
         EV is named 'EV'), got: {msg}"
    );
}

/// The L1 cold contract. At a *marginal* cold night (−2 °C — where an L1
/// circuit can physically work), the vehicle must accept real charge
/// energy: preconditioning warms the pack through the cutoff at L1 power
/// and then the whole circuit charges. The bound is a stated fraction of
/// the circuit's energy capability — a degenerate failure (a heater
/// monopolizing the budget, or a mass/UA pair that flash-freezes the
/// pack) delivers ~zero stored energy and goes red.
#[test]
fn l1_cold_night_still_accepts_charge_energy() {
    // A 75 kWh / L1 (1.4 kW) EV — the standard probe vehicle on a Level 1
    // circuit, built from the probe config with the level and
    // EVSE rating overridden.
    let (_, base_cfg) = probe_ev(Some(-2.0), None, None, Some("HomePluggedIn"), Some(0.3));
    let cfg = hares_equipment::EquipmentConfig::from_typed(
        "EV".to_string(),
        "EV".to_string(),
        EvConfig {
            charging_level: Some("Level 1".to_string()),
            max_charging_power_kw: 1.4,
            ..typed_config_of(&base_cfg)
        },
    )
    .expect("typed L1 EV config");
    let mut ev = hares_equipment::ev::Ev::new(cfg.clone());
    let mut env = env_at(-2.0, (2018, 1, 6, 22));
    ev.init(&cfg, &env).expect("EV init");
    ev.apply_signal(&hares_types::ControlSignal::SOCTarget {
        target_soc: 0.9,
        min_soc: None,
        max_soc: None,
    })
    .expect("SOCTarget signal");

    // One 8 h night window (the Nightly strategy's shape). Stored energy
    // is measured through the pack's SOC — the charging-isolating
    // observable — and the port peak through the AC input.
    let soc_start = ev.telemetry().get("soc").unwrap_or(f64::NAN);
    let mut port_peak_kw = 0.0_f64;
    for _ in 0..32 {
        step_900s(&mut ev, &mut env);
        let port_kw = ev.telemetry().get("active_power_kw").unwrap_or(0.0);
        port_peak_kw = port_peak_kw.max(port_kw);
    }
    let soc_end = ev.telemetry().get("soc").unwrap_or(f64::NAN);
    let stored_kwh = (soc_end - soc_start) * 75.0;

    assert!(
        port_peak_kw <= 1.8 + 1e-9,
        "the L1 circuit rating bounds the whole draw (charger + heater \
         AC-equivalent), peaked at {port_peak_kw} kW"
    );
    assert!(
        stored_kwh >= 4.0,
        "an L1 vehicle on a marginal (-2 C) cold night must accept real \
         charge energy (>= 4 kWh stored over the 8 h window — >= 36% of the \
         circuit's 11.2 kWh capability), stored {stored_kwh:.2} kWh"
    );
}

/// An overrides key naming a spec that exists in the dwelling population
/// but is consumed *outside* the equipment override channel — the
/// `Occupancy` spec, whose internal-gain load is applied directly in the
/// simulation loop — is the same silent no-op as an unknown key: before
/// the assembly-boundary validation existed it quietly missed while the
/// caller believed it applied. The build must fail with the *reasoned*
/// error ("handled outside the equipment override path"), not the
/// generic unknown-key text: the name is real, so "unknown equipment"
/// would misdirect the caller toward hunting for a typo instead of the
/// missing channel.
#[test]
fn override_keyed_by_spec_handled_outside_registry_errors_loudly() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let weather = write_constant_weather(tmp.path(), 2, 10.0);
    let src = std::fs::read_to_string(fixture_dir().join("home.xml")).expect("fixture home.xml");
    let hpxml_path = tmp.path().join("bldg0000007_ev.xml");
    std::fs::write(&hpxml_path, inject_ev(&src)).expect("write injected HPXML");

    // The field is irrelevant — the validation keys on the override name;
    // `number_of_occupants` is the natural mistake (it IS an Occupancy
    // parameter, just not reachable through this channel).
    let overrides = serde_json::json!({
        "Occupancy": { "number_of_occupants": 3 }
    });

    let sim = SimulationConfig {
        start_time: FixedOffset::west_opt(10 * 3600)
            .expect("UTC-10 offset is valid")
            .with_ymd_and_hms(2018, 1, 1, 0, 0, 0)
            .unwrap(),
        duration: Duration::days(2),
        time_res: Duration::seconds(900),
        output_verbosity: 5,
        write_output: false,
        output_path: None,
        output_format: OutputFormat::Csv,
        output_chunk_size: 1024,
        setpoint_deadband_c: None,
        master_seed: 0,
        civil_timezone: None,
        site_location: hares_io::SiteLocationOverride::default(),
        retain_batches: false,
        rotation: hares_io::RotationPolicy::None,
    };
    let config = DwellingConfig {
        hpxml_path,
        schedule_path: fixture_dir().join("in.schedules.csv"),
        weather_path: weather,
        defaults_path: Some(project_root().join("defaults")),
        sim_config: sim,
        overrides: Some(overrides),
        bldg_id: 100,
        initialization_duration: None,
        resample_overrides: Some(hares_io::ResampleOverrides::ochre_compat()),
        patches: None,
    };
    let err = SimulationEngine::new()
        .run(config)
        .expect_err("an override keyed by a spec handled outside the registry must fail the build");
    let msg = err.to_string();
    assert!(
        msg.contains("'Occupancy'") && msg.contains("cannot be overridden"),
        "the error must name the key and state the reason (handled outside \
         the equipment override path), got: {msg}"
    );
    assert!(
        !msg.contains("unknown equipment override key"),
        "the name is a real spec name — the generic unknown-key text would \
         misdirect the caller; got: {msg}"
    );
}

/// A non-object `overrides` payload must fail the build loudly, not
/// silently do nothing. `DwellingConfig.overrides` is a free-form
/// `serde_json::Value` and the Python surface converts any Python object
/// into it, so a bare string, array, or number is representable — and the
/// SystemIdentifier-as-override mistake this initiative documented (an
/// override keyed `"ReproEV1"`, matched by nothing) is exactly the shape a
/// string payload carries. The override key validator only inspects object
/// payloads (`let Value::Object(root) = … else { return Ok(()) }`) and the
/// override applier no-ops the same shapes, so the payload sails through
/// the assembly with zero effect and zero complaint — the silent no-op the
/// validator was added to reject, reopened by a type the validator never
/// sees keys from.
#[test]
fn non_object_overrides_payload_fails_the_build_loudly() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let weather = write_constant_weather(tmp.path(), 2, 10.0);
    let src = std::fs::read_to_string(fixture_dir().join("home.xml")).expect("fixture home.xml");
    let hpxml_path = tmp.path().join("bldg0000007_ev.xml");
    std::fs::write(&hpxml_path, inject_ev(&src)).expect("write injected HPXML");

    let sim = SimulationConfig {
        start_time: FixedOffset::west_opt(10 * 3600)
            .expect("UTC-10 offset is valid")
            .with_ymd_and_hms(2018, 1, 1, 0, 0, 0)
            .unwrap(),
        duration: Duration::days(2),
        time_res: Duration::seconds(900),
        output_verbosity: 5,
        write_output: false,
        output_path: None,
        output_format: OutputFormat::Csv,
        output_chunk_size: 1024,
        setpoint_deadband_c: None,
        master_seed: 0,
        civil_timezone: None,
        site_location: hares_io::SiteLocationOverride::default(),
        retain_batches: false,
        rotation: hares_io::RotationPolicy::None,
    };
    let base = DwellingConfig {
        hpxml_path: hpxml_path.clone(),
        schedule_path: fixture_dir().join("in.schedules.csv"),
        weather_path: weather,
        defaults_path: Some(project_root().join("defaults")),
        sim_config: sim,
        overrides: None,
        bldg_id: 100,
        initialization_duration: None,
        resample_overrides: Some(hares_io::ResampleOverrides::ochre_compat()),
        patches: None,
    };

    // Control: the identical build with no overrides payload must succeed —
    // any failure below is attributable to the payload's type, not the
    // fixture.
    SimulationEngine::new()
        .run(base.clone())
        .expect("the no-overrides control build must succeed");

    for payload in [
        serde_json::json!("ReproEV1"),
        serde_json::json!(["ReproEV1"]),
        serde_json::json!(3.0),
        // `Some(Value::Null)` — the fourth non-object face, and the
        // boundary the Python translation fix had to reason about:
        // Python `None` is converted to `Option::None` (the canonical
        // absent, which builds — pinned by the control above), but a
        // Rust caller passing `Some(Null)` means "I provided an
        // overrides payload" and provided nothing; treating it as
        // absent would silently no-op the channel, the exact class this
        // gate exists for.
        serde_json::json!(null),
    ] {
        let config = DwellingConfig {
            overrides: Some(payload.clone()),
            ..base.clone()
        };
        let err = SimulationEngine::new().run(config).expect_err(
            "a non-object overrides payload must fail the build loudly, \
                 not silently no-op: the validator's own contract (every key \
                 must name an overridable equipment) is bypassed wholesale \
                 when the payload has no keys at all",
        );
        let msg = err.to_string();
        assert!(
            msg.to_lowercase().contains("override"),
            "the error must identify the overrides payload as the problem, \
             got: {msg}"
        );
    }
}

/// Extracts the typed `EvConfig` payload from an `EquipmentConfig` built
/// by `probe_ev` so L1 variants can override single fields without
/// duplicating the 40-field literal.
fn typed_config_of(cfg: &hares_equipment::EquipmentConfig) -> EvConfig {
    let typed: EvConfig = cfg.typed().expect("probe configs are typed");
    typed
}
