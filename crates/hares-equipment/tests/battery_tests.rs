use std::time::Duration;

use chrono::{FixedOffset, TimeZone};
use hares_equipment::{BatteryConfig, Equipment, EquipmentConfig, battery::Battery};
use hares_types::{
    ControlSignal, EnvironmentState, GridState, PortContribution, PortSlots, WeatherState, ZoneId,
    ZoneState,
};

// ---------------------------------------------------------------------------
// Test fixtures
// ---------------------------------------------------------------------------

fn base_env() -> EnvironmentState {
    EnvironmentState {
        zones: vec![ZoneState {
            id: ZoneId(1),
            temperature_c: 21.0,
            humidity_ratio: 0.008,
            relative_humidity: 0.45,
            wet_bulb_c: 14.0,
            volume_m3: 200.0,
        }],
        weather: WeatherState {
            outdoor_temp_c: 25.0,
            outdoor_humidity_ratio: 0.005,
            outdoor_wet_bulb_c: 15.0,
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
        grid: GridState {
            voltage_pu: 1.0,
            frequency_hz: 60.0,
        },
        custom_domains: vec![],
        equipment_telemetry: std::collections::HashMap::new(),
        equipment_core: Default::default(),
        current_time: FixedOffset::east_opt(0)
            .expect("UTC offset")
            .with_ymd_and_hms(2026, 6, 21, 12, 0, 0)
            .single()
            .expect("valid"),
        time_res: chrono::Duration::seconds(60),
        price_signal: Default::default(),
        electrical: Default::default(),
    }
}

fn base_battery_config() -> BatteryConfig {
    BatteryConfig {
        equipment_id: None,
        zone_id: None,
        capacity_kwh: 13.5,
        max_charge_kw: 5.0,
        max_discharge_kw: 5.0,
        n_series: None,
        n_parallel: None,
        ah_cell: None,
        v_cell: None,
        cell_resistance_ohm: None,
        pack_voltage_v: None,
        chemistry: None,
        standby_power_w: Some(0.0),
        self_discharge_pct_per_day: None,
        min_soc: Some(0.15),
        max_soc: Some(0.95),
        initial_soc: Some(0.5),
        initial_cell_temp_c: None,
        import_limit_w: None,
        export_limit_w: None,
        heater_power_w: None,
        heater_threshold_c: None,
        heater_on_discharge: None,
        min_discharge_temp_c: None,
        full_power_temp_c: None,
        min_charge_temp_c: None,
        cell_thermal_mass_j_per_k: None,
        cell_ua_w_per_k: None,
        inverter_efficiency: Some(0.97),
        charge_efficiency: Some(0.97),
        discharge_efficiency: Some(0.97),
        bms_mode: None,
        grid_export_rule: None,
    }
}

fn battery_config() -> EquipmentConfig {
    EquipmentConfig::from_typed(
        "Battery".to_string(),
        "Battery".to_string(),
        base_battery_config(),
    )
}

/// Initialize a battery with default test config and return it ready to step.
fn make_battery() -> Battery {
    let config = battery_config();
    let env = base_env();
    let mut bat = Battery::new(config.clone());
    bat.init(&config, &env).expect("init");
    bat
}

/// Run N identical steps against the battery, discarding port state between steps.
fn step_n(bat: &mut Battery, n: u32, env: &EnvironmentState) {
    let dt = Duration::from_secs(60);
    for _ in 0..n {
        let mut ports = PortSlots::default();
        bat.step(env, dt, &mut ports).expect("step");
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn charge_increases_soc() {
    let mut bat = make_battery();
    let env = base_env();
    let soc_before = bat.telemetry().get("soc").expect("soc telemetry");

    bat.apply_control(&ControlSignal::PowerSetpoint {
        active_power_kw: 3.0,
        reactive_power_kvar: None,
    })
    .expect("apply charge setpoint");

    step_n(&mut bat, 60, &env);

    let soc_after = bat.telemetry().get("soc").expect("soc telemetry");
    assert!(
        soc_after > soc_before,
        "SOC should increase after charging: before={soc_before:.4} after={soc_after:.4}"
    );

    // Verify ΔSOC magnitude: 60 steps × 60s × 3kW × η=0.97 / 13.5kWh ≈ 0.216
    let delta_soc = soc_after - soc_before;
    let expected_delta = 3.0 * (60.0 * 60.0 / 3600.0) * 0.97 / 13.5;
    assert!(
        (delta_soc - expected_delta).abs() < expected_delta * 0.10,
        "SOC gain {delta_soc:.4} must be within 10% of expected {expected_delta:.4} \
         (3kW × 1h × η=0.97 / 13.5kWh); ohmic losses account for the remainder"
    );
}

#[test]
fn discharge_decreases_soc() {
    let mut bat = make_battery();
    let env = base_env();
    let soc_before = bat.telemetry().get("soc").expect("soc telemetry");

    bat.apply_control(&ControlSignal::PowerSetpoint {
        active_power_kw: -3.0,
        reactive_power_kvar: None,
    })
    .expect("apply discharge setpoint");

    step_n(&mut bat, 60, &env);

    let soc_after = bat.telemetry().get("soc").expect("soc telemetry");
    assert!(
        soc_after < soc_before,
        "SOC should decrease after discharging: before={soc_before:.4} after={soc_after:.4}"
    );

    // Verify ΔSOC magnitude: 60 steps × 60s × 3kW / (η=0.97) / 13.5kWh ≈ 0.229
    let delta_soc = soc_before - soc_after;
    let expected_delta = 3.0 * (60.0 * 60.0 / 3600.0) / 0.97 / 13.5;
    assert!(
        (delta_soc - expected_delta).abs() < expected_delta * 0.10,
        "SOC loss {delta_soc:.4} must be within 10% of expected {expected_delta:.4} \
         (3kW × 1h / η=0.97 / 13.5kWh); ohmic losses account for the remainder"
    );
}

#[test]
fn soc_clamped_at_min_max() {
    let env = base_env();
    let dt = Duration::from_secs(60);

    // Discharge from a low initial SOC -- SOC must not fall below min_soc.
    {
        let config = EquipmentConfig::from_typed(
            "Battery".to_string(),
            "Battery".to_string(),
            BatteryConfig {
                initial_soc: Some(0.16),
                ..base_battery_config()
            },
        );
        let mut bat = Battery::new(config.clone());
        bat.init(&config, &env).expect("init");
        bat.apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: -5.0,
            reactive_power_kvar: None,
        })
        .expect("apply discharge setpoint");

        // Run many steps so the SOC would fall far below min if not clamped.
        for _ in 0..120 {
            let mut ports = PortSlots::default();
            bat.step(&env, dt, &mut ports).expect("step");
        }

        let soc = bat.telemetry().get("soc").expect("soc");
        assert!(
            soc >= 0.15 - 1e-4,
            "SOC must not drop below min_soc=0.15: got {soc:.6}"
        );
    }

    // Charge from a high initial SOC -- SOC must not exceed max_soc.
    {
        let config = EquipmentConfig::from_typed(
            "Battery".to_string(),
            "Battery".to_string(),
            BatteryConfig {
                initial_soc: Some(0.94),
                ..base_battery_config()
            },
        );
        let mut bat = Battery::new(config.clone());
        bat.init(&config, &env).expect("init");
        bat.apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: 5.0,
            reactive_power_kvar: None,
        })
        .expect("apply charge setpoint");

        for _ in 0..120 {
            let mut ports = PortSlots::default();
            bat.step(&env, dt, &mut ports).expect("step");
        }

        let soc = bat.telemetry().get("soc").expect("soc");
        assert!(
            soc <= 0.95 + 1e-4,
            "SOC must not exceed max_soc=0.95: got {soc:.6}"
        );
    }
}

#[test]
fn round_trip_efficiency_below_unity() {
    // Charge for N steps, then discharge for the same N steps.
    // Energy delivered to the grid on discharge must be less than energy drawn during charging.
    let mut bat = make_battery();
    let env = base_env();
    let dt = Duration::from_secs(60);
    let n_steps = 30_u32;
    let charge_kw = 3.0_f64;

    let soc_start = bat.telemetry().get("soc").expect("soc at start");

    bat.apply_control(&ControlSignal::PowerSetpoint {
        active_power_kw: charge_kw,
        reactive_power_kvar: None,
    })
    .expect("charge setpoint");

    let mut energy_in_kwh = 0.0_f64;
    for _ in 0..n_steps {
        let mut ports = PortSlots::default();
        bat.step(&env, dt, &mut ports).expect("step");
        let p_net = ports.electrical.net_active_kw();
        // Only tally positive (charging) contribution.
        if p_net > 0.0 {
            energy_in_kwh += p_net * (60.0 / 3600.0);
        }
    }

    bat.apply_control(&ControlSignal::PowerSetpoint {
        active_power_kw: -charge_kw,
        reactive_power_kvar: None,
    })
    .expect("discharge setpoint");

    let mut energy_out_kwh = 0.0_f64;
    for _ in 0..n_steps {
        let mut ports = PortSlots::default();
        bat.step(&env, dt, &mut ports).expect("step");
        let p_net = ports.electrical.net_active_kw();
        // Only tally negative (discharging) contribution.
        if p_net < 0.0 {
            energy_out_kwh += p_net.abs() * (60.0 / 3600.0);
        }
    }

    assert!(
        energy_in_kwh > 0.0,
        "No energy was recorded during charging phase"
    );
    assert!(
        energy_out_kwh > 0.0,
        "No energy was recorded during discharging phase"
    );
    // With non-unity charge/discharge efficiency, requesting symmetric AC
    // charge/discharge power for equal durations must reduce SOC.
    let soc_end = bat.telemetry().get("soc").expect("soc at end");
    assert!(
        soc_end < soc_start,
        "Round-trip losses must reduce SOC for symmetric AC setpoints: \
         start={soc_start:.6}, end={soc_end:.6}, energy_in={energy_in_kwh:.4} kWh, energy_out={energy_out_kwh:.4} kWh"
    );
}

#[test]
fn self_consumption_charges_when_exporting() {
    // Pre-populate ports with -5 kW (PV generation surplus), then battery step
    // should charge to absorb the export.
    let mut bat = make_battery();
    let env = base_env();

    // Enable self-consumption (default after init, but be explicit).
    bat.apply_control(&ControlSignal::SelfConsumption {
        enabled: true,
        solar_only_charging: false,
    })
    .expect("enable self-consumption");

    let dt = Duration::from_secs(60);
    let mut ports = PortSlots::default();
    // Simulate -5 kW generation from a prior Independent-stage PV unit.
    ports
        .accumulate(&PortContribution::Electrical {
            active_power_kw: -5.0,
            reactive_power_kvar: 0.0,
        })
        .expect("accumulate PV generation");

    bat.step(&env, dt, &mut ports).expect("step");

    // The battery should have charged -- active_power_kw at the port is positive (load).
    // The port net after both PV and battery contributions will still be negative, but
    // battery's own telemetry shows it absorbed power.
    let active_kw = bat
        .telemetry()
        .get("active_power_kw")
        .expect("active_power_kw");
    assert!(
        active_kw > 0.0,
        "Battery should charge (positive active_power_kw) when there is PV export: got {active_kw:.4}"
    );
}

#[test]
fn self_consumption_discharges_when_importing() {
    // Pre-populate ports with +3 kW load, battery step should discharge.
    let mut bat = make_battery();
    let env = base_env();

    bat.apply_control(&ControlSignal::SelfConsumption {
        enabled: true,
        solar_only_charging: false,
    })
    .expect("enable self-consumption");

    let dt = Duration::from_secs(60);
    let mut ports = PortSlots::default();
    // Simulate +3 kW net load from other equipment.
    ports
        .accumulate(&PortContribution::Electrical {
            active_power_kw: 3.0,
            reactive_power_kvar: 0.0,
        })
        .expect("accumulate load");

    bat.step(&env, dt, &mut ports).expect("step");

    // Battery discharges -- active_power_kw in telemetry is negative (generation).
    let active_kw = bat
        .telemetry()
        .get("active_power_kw")
        .expect("active_power_kw");
    assert!(
        active_kw < 0.0,
        "Battery should discharge (negative active_power_kw) when there is net load: got {active_kw:.4}"
    );
}

#[test]
fn inverter_efficiency_applied() {
    // Charge at a known AC setpoint and verify that SOC gain reflects inverter losses.
    // When inverter_efficiency=0.97 is given, the charge direction uses sqrt(0.97) ≈ 0.9849,
    // so DC stored = AC * sqrt(eta), and the round-trip loss is eta = sqrt(eta)^2 = 0.97.
    let mut bat = make_battery();
    let env = base_env();
    let dt = Duration::from_secs(3600); // One hour step for easy energy arithmetic.
    let charge_kw = 2.0_f64;
    let inverter_eta = 0.97_f64;
    let charge_eta = inverter_eta.sqrt(); // per-direction efficiency from symmetric split

    bat.apply_control(&ControlSignal::PowerSetpoint {
        active_power_kw: charge_kw,
        reactive_power_kvar: None,
    })
    .expect("charge setpoint");

    let soc_before = bat.telemetry().get("soc").expect("soc");
    let mut ports = PortSlots::default();
    bat.step(&env, dt, &mut ports).expect("step");
    let soc_after = bat.telemetry().get("soc").expect("soc");

    // AC energy drawn from grid.
    let ac_energy_kwh = charge_kw; // 2 kW × 1 h
    // DC energy into cells ≈ AC × charge_eta (ignoring ohmic losses for this assertion).
    let dc_energy_expected_kwh = ac_energy_kwh * charge_eta;
    let soc_gain = soc_after - soc_before;
    // Energy stored = soc_gain × capacity_kwh. Ohmic losses reduce it slightly, so actual <= dc_expected.
    let energy_stored_kwh = soc_gain * 13.5;
    assert!(
        energy_stored_kwh <= dc_energy_expected_kwh + 1e-4,
        "Energy stored ({energy_stored_kwh:.4} kWh) must not exceed AC×sqrt(eta) \
         ({dc_energy_expected_kwh:.4} kWh)"
    );
    // At 0.148C (2kW/13.5kWh), ohmic losses are <2%. Tighten lower bound
    // to catch incorrect pack resistance computation.
    assert!(
        energy_stored_kwh > dc_energy_expected_kwh * 0.97,
        "Energy stored ({energy_stored_kwh:.4} kWh) should be within 3% of DC expected \
         ({dc_energy_expected_kwh:.4} kWh); ohmic losses at 0.148C are <2%"
    );
    // Verify that losses are present (not a perfect-efficiency no-op).
    assert!(
        energy_stored_kwh < ac_energy_kwh,
        "Energy stored ({energy_stored_kwh:.4} kWh) must be less than AC energy drawn \
         ({ac_energy_kwh:.4} kWh) due to inverter conversion losses"
    );
}

#[test]
fn power_limits_respected() {
    // Apply a setpoint far above max_charge_kw=5.0 kW and verify actual port power
    // is clamped to that hardware limit.
    let mut bat = make_battery();
    let env = base_env();
    let dt = Duration::from_secs(60);

    bat.apply_control(&ControlSignal::PowerSetpoint {
        active_power_kw: 100.0,
        reactive_power_kvar: None,
    })
    .expect("apply oversized setpoint");

    let mut ports = PortSlots::default();
    bat.step(&env, dt, &mut ports).expect("step");

    let active_kw = bat
        .telemetry()
        .get("active_power_kw")
        .expect("active_power_kw");
    assert!(
        active_kw <= 5.0 + 1e-6,
        "Active power must be clamped to max_charge_kw=5.0 kW: got {active_kw:.4}"
    );
    assert!(
        active_kw > 0.0,
        "Battery should be charging: got {active_kw:.4}"
    );
}

#[test]
fn idle_no_power_flow() {
    // No control signal applied (self_consumption is re-enabled after init
    // but with no net load in ports, there is nothing to respond to).
    // active_power_kw should be approximately zero (only standby_power_w=0 in test config).
    let mut bat = make_battery();
    let env = base_env();

    // Explicitly disable self-consumption so there is no background logic.
    // This isolates the no-signal idle path.
    bat.apply_control(&ControlSignal::SelfConsumption {
        enabled: false,
        solar_only_charging: false,
    })
    .expect("disable self-consumption");

    // Also clear power setpoint (disabled by SelfConsumption apply).
    // No PowerSetpoint → no SOC target → no self-consumption → idle.
    let dt = Duration::from_secs(60);
    let mut ports = PortSlots::default();
    bat.step(&env, dt, &mut ports).expect("step");

    let active_kw = bat
        .telemetry()
        .get("active_power_kw")
        .expect("active_power_kw");
    // standby_power_w was set to 0.0 in test config.
    assert!(
        active_kw.abs() < 1e-6,
        "Idle battery with no control and no load should have ~0 kW flow: got {active_kw:.6}"
    );
}

#[test]
fn checkpoint_preserves_soc_and_mode() {
    let mut bat = make_battery();
    let env = base_env();
    let dt = Duration::from_secs(60);

    // Charge for a while to move SOC away from initial 0.5.
    bat.apply_control(&ControlSignal::PowerSetpoint {
        active_power_kw: 4.0,
        reactive_power_kvar: None,
    })
    .expect("charge setpoint");

    step_n(&mut bat, 30, &env);

    let soc_before_save = bat.telemetry().get("soc").expect("soc");

    // Save state.
    let checkpoint = bat.save_state();

    // Continue stepping to mutate state further.
    step_n(&mut bat, 30, &env);
    let soc_after_mutation = bat.telemetry().get("soc").expect("soc");
    assert!(
        soc_after_mutation > soc_before_save,
        "SOC should have increased after further charging"
    );

    // Restore checkpoint.
    bat.load_state(&checkpoint).expect("load_state");
    let soc_after_restore = bat.telemetry().get("soc").expect("soc");

    assert!(
        (soc_after_restore - soc_before_save).abs() < 1e-9,
        "SOC after load_state must match pre-save SOC: expected {soc_before_save:.6}, got {soc_after_restore:.6}"
    );

    // Battery should still function correctly after restore.
    let mut ports = PortSlots::default();
    bat.step(&env, dt, &mut ports).expect("step after restore");
    let soc_stepped = bat.telemetry().get("soc").expect("soc");
    assert!(
        soc_stepped > soc_before_save,
        "Battery should continue charging after restore (soc={soc_stepped:.6} > pre-save={soc_before_save:.6})"
    );
}
