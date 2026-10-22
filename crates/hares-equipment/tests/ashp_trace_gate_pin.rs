//! The ASHP control-trace gate's read-once pin.
//!
//! `ashp_control_trace_enabled` (`src/hvac/heat_pump/heater.rs`) reads
//! `HARES_DEBUG_ASHP_CONTROL` once per process into a `OnceLock` and the
//! per-step gate reads the cache: first read wins, later environment
//! changes have no effect, and the trace content is unchanged by the
//! caching. No earlier test pinned those semantics (the suite had no
//! reader of the variable, so reverting the cache to a per-call environ
//! read failed zero tests); this file is the permanent pin.
//!
//! The mechanics are the self-spawning pattern, and the helper's privacy
//! plus the variable's process-global state are the reasons. The helper
//! is private, so the child drives the gate through the public
//! `Equipment::update_control` surface, the call site the trace's gate
//! guards, and the trace's own stderr output is the observable; the
//! variable is process-global, so mutating it in-process would race every
//! other test in the binary that drives a heat-pump step (`env::set_var`
//! is `unsafe` in the 2024 edition for exactly that reason). The parent
//! test spawns this test binary filtered to the child-mode test under a
//! sentinel variable; the child exists to mutate its own environment: it
//! sets `HARES_DEBUG_ASHP_CONTROL`, drives one gate read (the trace
//! prints and the cache settles), unsets the variable, and drives a
//! second, identical gate read on a fresh heater. The child asserts both
//! reads agree on the control outcome; the parent asserts the child's
//! exit, that both trace lines printed (the second after the variable was
//! unset, where a per-call read would print nothing) and that the two
//! lines are byte-identical (the trace content is unchanged by the
//! caching). Together that is the helper's documented contract: set the
//! variable before the run and the same control dump prints, and the read
//! is settled by the first gate read.

use std::process::{Command, Stdio};

use chrono::{FixedOffset, TimeZone};
use hares_equipment::hvac::heat_pump::ASHPHeater;
use hares_equipment::{
    DefrostConfig, Equipment, EquipmentConfig, HeatPumpCommonConfig, HeatPumpHeaterConfig,
    HvacSetpointConfig, RejectUnknownKeys,
};
use hares_types::{EnvironmentState, OperatingMode};

/// The sentinel: its presence marks the process as the child mode the
/// parent test below spawns. Its name stays clear of the `HARES_` runtime
/// namespace the engine reads.
const CHILD_SENTINEL: &str = "ASHP_TRACE_GATE_PIN_CHILD";

/// The child-mode test's name, the one the parent's filter selects.
const CHILD_TEST: &str = "ashp_trace_gate_reads_the_variable_once_child";

/// The trace's own stderr marker, the output the gate guards.
const TRACE_MARKER: &str = "[ASHP-CTL]";

/// The variable the gate reads.
const TRACE_VAR: &str = "HARES_DEBUG_ASHP_CONTROL";

/// The pin's ASHP heater: the shipped single-speed ASHP shape (the same
/// one the heater's own tests drive), so the control resolution runs and
/// the gated site is reached.
fn ashp_heater_config() -> EquipmentConfig {
    let typed = HeatPumpHeaterConfig {
        reject_unknown_keys: RejectUnknownKeys,
        common: HeatPumpCommonConfig {
            equipment_id: None,
            zone_id: Some(1),
            heating_capacity_w: Some(8_000.0),
            heating_eir: Some(0.33),
            stage_heating_capacities_w: None,
            stage_heating_eirs: None,
            backup_fuel: None,
            backup_capacity_w: Some(4_000.0),
            backup_eir: None,
            fraction_heating_load_served: None,
            cooling_capacity_w: None,
            cooling_eir: None,
            stage_cooling_capacities_w: None,
            stage_cooling_eirs: None,
            fraction_cooling_load_served: None,
            number_of_speeds: 1,
            is_mini_split: false,
            shr: None,
            fan_power_w: None,
            fan_power_w_per_cfm: None,
            airflow_m3_s_per_w: None,
            setpoint: HvacSetpointConfig {
                heating_setpoint_c: Some(21.0),
                cooling_setpoint_c: Some(26.0),
                ..Default::default()
            },
            hysteresis_c: Some(1.0),
            duct: Default::default(),
            biquadratic_x1_min: None,
            biquadratic_x1_max: None,
            biquadratic_x2_min: None,
            biquadratic_x2_max: None,
            ff_min: None,
            ff_max: None,
            min_compressor_fraction: None,
            eir_part_load_benefit: None,
            er_stages: 1,
            charge_defect_ratio: None,
            ..Default::default()
        },
        hp_lockout_temp_c: None,
        er_lockout_temp_c: None,
        max_oat_supplemental_c: None,
        er_setpoint_offset_c: None,
        er_hard_lockout_time_s: None,
        er_soft_lockout_time_s: None,
        heating_shr: None,
        capacity_ratio_at_17f: None,
        defrost: DefrostConfig::default(),
    };
    EquipmentConfig::from_typed(
        "ashp_trace_gate_pin".to_string(),
        "ASHP Heater".to_string(),
        typed,
    )
    .expect("the pin's ASHP heater config constructs")
}

/// The pin's environment state: one zone held under the heating setpoint
/// (18 C against 21 C with a 1 C hysteresis) at mild outdoor conditions,
/// the same shape the equipment tests' shared builder produces, so the
/// control resolution runs and the gated site is reached on every read.
fn pin_env() -> EnvironmentState {
    EnvironmentState {
        ambient_other_space_c: hares_types::AmbientOtherSpaceTemps::default(),
        zones: vec![hares_types::ZoneState {
            id: hares_types::ZoneId(1),
            temperature_c: 18.0,
            humidity_ratio: 0.008,
            volume_m3: 200.0,
        }],
        weather: hares_types::WeatherState {
            outdoor_temp_c: 10.0,
            outdoor_humidity_ratio: 0.005,
            outdoor_wet_bulb_c: 7.0,
            outdoor_enthalpy_j_kg: 22_800.0,
            wind_speed_m_s: 2.0,
            wind_dir_deg: 0.0,
            ground_temp_c: 12.0,
            sky_temp_c: 8.0,
            pressure_kpa: 101.325,
            solar_irradiance: vec![],
            ghi_w_m2: 0.0,
            dni_w_m2: 0.0,
            dhi_w_m2: 0.0,
            solar_altitude_deg: 0.0,
            solar_azimuth_deg: 180.0,
            mains_temp_c: 15.0,
            rainfall_m: 0.0,
            ground_albedo: 0.2,
            ground_t_mean_c: 10.0,
            ground_t_amplitude_c: 0.0,
            ground_phase_day: 35.0,
            day_of_year: 1.0,
        },
        grid: hares_types::GridState {
            voltage_pu: 1.0,
            frequency_hz: 60.0,
            island_bus_voltage_pu: None,
        },
        schedule_row: None,
        unavailable_periods: hares_types::UnavailablePeriodState::default(),
        domains: hares_types::DomainSlots::default(),
        equipment_telemetry: std::collections::HashMap::new(),
        equipment_core: Default::default(),
        current_time: FixedOffset::east_opt(0)
            .expect("UTC offset")
            .with_ymd_and_hms(2026, 6, 21, 12, 0, 0)
            .single()
            .expect("valid timestamp"),
        time_res: chrono::Duration::seconds(60),
        price_signal: Default::default(),
        electrical: Default::default(),
    }
}

/// One gate read through the public surface: an initialized ASHP heater's
/// control resolution, where the gated trace prints to the process's
/// stderr exactly when the gate is on.
fn drive_one_gate_read(cfg: &EquipmentConfig, env: &EnvironmentState) -> OperatingMode {
    let mut heater = ASHPHeater::new(cfg.clone());
    heater.init(cfg, env).expect("the pin's heater initializes");
    heater.update_control(env)
}

/// The parent: spawns this test binary in child mode and asserts the
/// read-once semantics from the child's trace output and exit.
#[test]
fn ashp_trace_gate_reads_the_variable_once() {
    if std::env::var_os(CHILD_SENTINEL).is_some() {
        // Already the spawned process of an outer parent run: never
        // recurse. The parent's filter selects only the child-mode test,
        // so this fires only under a misnamed spawn.
        return;
    }

    let exe = std::env::current_exe().expect("this test binary's path");
    let output = Command::new(exe)
        .args(["--exact", CHILD_TEST, "--nocapture"])
        .env(CHILD_SENTINEL, "1")
        // A clean start for the variable itself: the child's first read
        // must be the one its own set performs, not an ambient one.
        .env_remove(TRACE_VAR)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn this test binary in child mode");

    assert!(
        output.status.success(),
        "the child-mode gate reads must pass ({}):\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    let lines: Vec<&str> = stderr
        .lines()
        .filter(|line| line.contains(TRACE_MARKER))
        .collect();
    assert_eq!(
        lines.len(),
        2,
        "exactly two trace lines must print, one per gate read, the second after \
         the variable was unset (first-read-wins); the child's stderr:\n{stderr}"
    );
    assert_eq!(
        lines[0], lines[1],
        "the trace content must be unchanged by the caching: both reads see the \
         same state, so the lines are byte-identical"
    );
    assert!(
        lines[0].starts_with(TRACE_MARKER),
        "the printed line is the ASHP control dump: {}",
        lines[0]
    );
}

/// The child mode: run alone in the process the parent spawned for the
/// environment mutation. Run directly by the suite (no sentinel), it is a
/// no-op pass; the parent test is the real assertion.
#[test]
fn ashp_trace_gate_reads_the_variable_once_child() {
    if std::env::var_os(CHILD_SENTINEL).is_none() {
        return;
    }

    let cfg = ashp_heater_config();
    // Identical states on both reads: the zone sits under the heating
    // setpoint, so the control resolution runs on both sides, and each
    // read drives a fresh heater so the two dumps see the same inputs.
    let env = pin_env();

    // Safety: this test body runs alone in the dedicated child process
    // the parent test spawned for exactly this mutation, filtered to this
    // one test; no other thread or test in the process reads the
    // environment concurrently (the same one-test-per-process isolation
    // the suite's other env-mutating tests rely on).
    unsafe { std::env::set_var(TRACE_VAR, "1") };
    let mode_with_var = drive_one_gate_read(&cfg, &env);

    // Safety: as above; the removal also restores the ambient state.
    unsafe { std::env::remove_var(TRACE_VAR) };
    let mode_after_unset = drive_one_gate_read(&cfg, &env);

    assert_eq!(
        mode_with_var, mode_after_unset,
        "the control outcome must be unchanged by the caching"
    );
}
