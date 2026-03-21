//! Heat-pump cooler variants.

use std::borrow::Cow;
use std::time::Duration;

use hares_types::{
    ControlCapabilities, ControlSignal, EndUse, EnvironmentState, EquipmentDescriptor, EquipmentId,
    ExecutionStage, FuelType, HaresError, OperatingMode, PortDeclaration, PortSlots,
};

use crate::{Equipment, EquipmentConfig, Telemetry};

use super::super::air_conditioner::AirConditioner;
use super::super::helpers::{equipment_id_from_config, zone_id_from_config};
use super::constants::{DEFAULT_EQUIPMENT_ID, DEFAULT_ZONE_ID};

/// MSHP crankcase heater: 15 W rated, activates at or below 0 °C.
/// Distinct from central AC default (50 W / 12.8 °C) per OCHRE conventions.
const MSHP_CRANKCASE_HEATER_KW: f64 = 0.015;
const MSHP_CRANKCASE_HEATER_THRESHOLD_C: f64 = 0.0;

pub struct HpCooler {
    inner: AirConditioner,
    descriptor: EquipmentDescriptor,
    ports: Vec<PortDeclaration>,
    is_mshp: bool,
    /// RTF of the companion heating coil from the previous step.
    /// Set by the system coordinator after the heater step so that the cooler
    /// can compute crankcase power using `max(cooling_rtf, heating_rtf)`.
    companion_heating_rtf: Option<f64>,
}

impl HpCooler {
    fn build(config: EquipmentConfig, equipment_type: &'static str, is_mshp: bool) -> Self {
        let mut inner = AirConditioner::new(config.clone());
        if is_mshp {
            inner.core.hvac.equipment_type =
                crate::hvac::hvac_core::HvacEquipmentType::MiniSplitCool;
            // Apply MSHP crankcase defaults at construction so that step() uses
            // correct values (15 W / 0 °C) even if init() has not been called yet.
            // init() re-applies these after re-reading config, so there is no
            // double-override risk.
            inner.set_crankcase_defaults_if_unconfigured(
                &config,
                MSHP_CRANKCASE_HEATER_KW,
                MSHP_CRANKCASE_HEATER_THRESHOLD_C,
            );
        }
        let zone = zone_id_from_config(&config).unwrap_or(hares_types::ZoneId(DEFAULT_ZONE_ID));
        Self {
            descriptor: EquipmentDescriptor {
                id: EquipmentId(equipment_id_from_config(&config).unwrap_or(DEFAULT_EQUIPMENT_ID)),
                name: config.name,
                end_use: EndUse::HvacCooling,
                equipment_type: Cow::Borrowed(equipment_type),
                zone: Some(zone),
                fuel: FuelType::Electric,
                stage: ExecutionStage::Thermal,
                control_capabilities: ControlCapabilities::THERMAL_SETPOINT,
                telemetry_fields: inner.descriptor().telemetry_fields.clone(),
            },
            ports: inner.ports().to_vec(),
            inner,
            is_mshp,
            companion_heating_rtf: None,
        }
    }

    #[must_use]
    pub fn ashp_cooler(config: EquipmentConfig) -> Self {
        Self::build(config, "ASHP Cooler", false)
    }

    #[must_use]
    pub fn mshp_cooler(config: EquipmentConfig) -> Self {
        Self::build(config, "MSHP Cooler", true)
    }

    /// Provide the companion heating coil's RTF from the most recent step.
    ///
    /// Call this after the heater's `step()` completes and before calling this
    /// cooler's `step()`, so that crankcase heater power accounts for HP heating
    /// mode operation. Pass `None` to clear the companion RTF (standalone AC mode).
    pub fn set_companion_heating_rtf(&mut self, rtf: Option<f64>) {
        self.companion_heating_rtf = rtf;
    }

    /// Return the cooling coil's runtime fraction from the most recent step.
    ///
    /// Used by the system coordinator to pass to the companion heater side.
    pub fn last_cooling_rtf(&self) -> f64 {
        self.inner.core.last_cooling_rtf
    }
}

impl Equipment for HpCooler {
    fn descriptor(&self) -> &hares_types::EquipmentDescriptor {
        &self.descriptor
    }

    fn ports(&self) -> &[PortDeclaration] {
        &self.ports
    }

    fn init(&mut self, config: &EquipmentConfig, env: &EnvironmentState) -> crate::Result<()> {
        self.inner.init(config, env)?;
        if self.is_mshp {
            // MSHP crankcase heater: 15 W / 0 °C, overriding central AC defaults
            // (50 W / 12.8 °C) unless the user explicitly configured them.
            self.inner.set_crankcase_defaults_if_unconfigured(
                config,
                MSHP_CRANKCASE_HEATER_KW,
                MSHP_CRANKCASE_HEATER_THRESHOLD_C,
            );
        }
        Ok(())
    }

    fn update_control(&mut self, env: &EnvironmentState) -> OperatingMode {
        self.inner.update_control(env)
    }

    fn step(
        &mut self,
        env: &EnvironmentState,
        dt: Duration,
        ports: &mut PortSlots,
    ) -> std::result::Result<(), HaresError> {
        // Pass companion_heating_rtf so crankcase uses max(cooling_rtf, heating_rtf)
        // when this cooler is part of an HP system.
        self.inner
            .core
            .step(env, dt, ports, self.companion_heating_rtf)
    }

    fn telemetry(&self) -> &Telemetry {
        self.inner.telemetry()
    }

    fn save_state(&self) -> Vec<u8> {
        self.inner.save_state()
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        self.inner.load_state(state)
    }

    fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        self.inner.apply_control_unchecked(signal)
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, time::Duration};

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        ControlCapabilities, ControlSignal, EndUse, EnvironmentState, ExecutionStage, FuelType,
        GridState, PortSlots, ThermalAccumulator, WeatherState, ZoneId, ZoneState,
    };

    use super::{super::super::super::Equipment, super::super::super::EquipmentConfig, HpCooler};

    fn cooling_env(zone_temp_c: f64, outdoor_c: f64) -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: zone_temp_c,
                humidity_ratio: 0.010,
                relative_humidity: 0.50,
                wet_bulb_c: zone_temp_c - 5.0,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: outdoor_c,
                outdoor_humidity_ratio: 0.005,
                wind_speed_m_s: 2.0,
                wind_dir_deg: 0.0,
                ground_temp_c: 15.0,
                sky_temp_c: 20.0,
                pressure_kpa: 101.325,
                solar_irradiance: vec![],
                ghi_w_m2: 0.0,
                dni_w_m2: 0.0,
                dhi_w_m2: 0.0,
                solar_altitude_deg: 0.0,
                ..Default::default()
            },
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
            },
            custom_domains: vec![],
            current_time: FixedOffset::east_opt(0)
                .expect("UTC offset")
                .with_ymd_and_hms(2026, 7, 15, 14, 0, 0)
                .single()
                .expect("valid"),
            time_res: ChronoDuration::minutes(1),
        }
    }

    fn base_config() -> EquipmentConfig {
        let mut raw = HashMap::new();
        raw.insert("zone_id".to_string(), 1.0.into());
        raw.insert("cooling_capacity_w".to_string(), 8_000.0.into());
        raw.insert("eir".to_string(), 0.33.into());
        raw.insert("cooling_setpoint_c".to_string(), 24.0.into());
        raw.insert("heating_setpoint_c".to_string(), 18.0.into());
        raw.insert(
            "capacity_biquadratic_coeffs".to_string(),
            "[1,0,0,0,0,0]".into(),
        );
        raw.insert("eir_biquadratic_coeffs".to_string(), "[1,0,0,0,0,0]".into());
        EquipmentConfig {
            name: "HP Cooler".to_string(),
            ochre_class: "ASHP Cooler".to_string(),
            raw_config: raw,
        }
    }

    /// Zone above cooling setpoint — cooler must remove heat (negative thermal
    /// contribution) and draw positive electrical power.
    #[test]
    fn ashp_cooler_cools_when_zone_above_setpoint() {
        let cfg = base_config();
        let mut eq = HpCooler::ashp_cooler(cfg.clone());
        let env = cooling_env(28.0, 35.0); // zone well above 24 C setpoint
        eq.init(&cfg, &env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        // Cooling removes heat: thermal contribution to zone is negative
        assert!(
            ports.thermal[0].sensible_gain_w < 0.0,
            "expected negative sensible gain (cooling), got {}",
            ports.thermal[0].sensible_gain_w
        );
        // Compressor + fan draws electricity
        assert!(
            ports.electrical.net_active_kw() > 0.0,
            "expected positive electrical draw, got {}",
            ports.electrical.net_active_kw()
        );
    }

    /// mshp_cooler() must produce valid equipment with correct type label and
    /// end-use, proving the constructor path is distinct from ashp_cooler().
    #[test]
    fn mshp_cooler_construction_produces_valid_descriptor() {
        let cfg = base_config();
        let eq = HpCooler::mshp_cooler(cfg);

        assert_eq!(eq.descriptor().equipment_type, "MSHP Cooler");
        assert_eq!(eq.descriptor().end_use, EndUse::HvacCooling);
        assert_eq!(eq.descriptor().fuel, FuelType::Electric);
        assert_eq!(eq.descriptor().stage, ExecutionStage::Thermal);
        assert!(
            eq.descriptor()
                .control_capabilities
                .contains(ControlCapabilities::THERMAL_SETPOINT)
        );
        // Must have at least one port for thermal and one for electrical
        assert!(
            eq.ports().len() >= 2,
            "expected at least 2 ports, got {}",
            eq.ports().len()
        );
    }

    /// MSHP cooler must use MiniSplitCool equipment type with 312 CFM/ton.
    #[test]
    fn mshp_cooler_uses_mini_split_cool_with_312_cfm_per_ton() {
        use hares_physics::constants::{CFM_TO_M3_S, W_PER_TON};
        let cfg = base_config();
        let eq = HpCooler::mshp_cooler(cfg);
        assert_eq!(
            eq.inner.core.hvac.equipment_type,
            crate::hvac::hvac_core::HvacEquipmentType::MiniSplitCool,
        );
        let expected = 312.0 * CFM_TO_M3_S / W_PER_TON;
        assert!(
            (eq.inner.core.hvac.airflow_m3_s_per_w - expected).abs() < 1e-12,
            "MSHP cooler must default to 312 CFM/ton, got {} m3/s/W",
            eq.inner.core.hvac.airflow_m3_s_per_w
        );
    }

    /// Zone between heating and cooling setpoints — deadband — no electrical
    /// draw and no thermal load.
    #[test]
    fn cooler_in_deadband_produces_zero_output() {
        let cfg = base_config();
        let mut eq = HpCooler::ashp_cooler(cfg.clone());
        // Zone at 21 C, well inside the 18–24 C deadband
        let env = cooling_env(21.0, 25.0);
        eq.init(&cfg, &env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        // Outdoor temp is above crankcase heater threshold (12.8 C), so no
        // crankcase draw either. Expect exactly zero electrical and thermal.
        assert_eq!(
            ports.thermal[0].sensible_gain_w, 0.0,
            "deadband: expected zero thermal output"
        );
        assert_eq!(
            ports.electrical.net_active_kw(),
            0.0,
            "deadband: expected zero electrical draw"
        );
    }

    /// Control signal application: sending a ThermalSetpoint signal must be
    /// accepted (capability gate passes) and must not error.
    #[test]
    fn apply_control_accepts_thermal_setpoint_signal() {
        let cfg = base_config();
        let mut eq = HpCooler::ashp_cooler(cfg);
        let signal = ControlSignal::ThermalSetpoint {
            heating_setpoint_c: Some(18.0),
            cooling_setpoint_c: Some(25.0),
            deadband_c: None,
        };
        eq.apply_control(&signal)
            .expect("ThermalSetpoint should be accepted by HpCooler");
    }

    /// OCHRE parity: cooler must be OFF at initialization when zone_temp == cooling_setpoint.
    ///
    /// OCHRE HVAC.py: turn_on = setpoint + deadband * (1 - offset) = 24.4 + 1.0 * 0.8 = 25.2
    /// With zone_temp = 24.4 <= 25.2, OCHRE stays OFF (mode_prev = "Off", neither
    /// turn-on nor turn-off condition fires → keeps current "Off" mode).
    /// HARES must match: thermostat stays in Deadband, no electrical draw, no cooling.
    #[test]
    fn cooler_off_when_zone_temp_at_setpoint_at_init() {
        let mut raw = HashMap::new();
        raw.insert("zone_id".to_string(), 1.0.into());
        raw.insert("cooling_capacity_w".to_string(), 8_000.0.into());
        raw.insert("eir".to_string(), 0.33.into());
        raw.insert("cooling_setpoint_c".to_string(), 24.4.into());
        raw.insert("heating_setpoint_c".to_string(), 18.0.into());
        raw.insert("capacity_biquadratic_coeffs".to_string(), "[1,0,0,0,0,0]".into());
        raw.insert("eir_biquadratic_coeffs".to_string(), "[1,0,0,0,0,0]".into());
        let cfg = EquipmentConfig {
            name: "ASHP Cooler".to_string(),
            ochre_class: "ASHP Cooler".to_string(),
            raw_config: raw,
        };

        // Zone at exactly the cooling setpoint (24.4°C).
        // turn_on = 24.4 + 1.0 * (1 - 0.2) = 25.2 → zone NOT above threshold → cooler must be OFF.
        let env = cooling_env(24.4, 35.0);

        let mut eq = HpCooler::ashp_cooler(cfg.clone());
        eq.init(&cfg, &env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        assert_eq!(
            ports.thermal[0].sensible_gain_w,
            0.0,
            "cooler must produce zero thermal output when zone_temp (24.4°C) == cooling_setpoint; \
             OCHRE turn-on threshold = 25.2°C — got {:.3} W",
            ports.thermal[0].sensible_gain_w
        );
        assert_eq!(
            ports.electrical.net_active_kw(),
            0.0,
            "cooler must draw 0 kW when zone_temp (24.4°C) <= turn-on threshold (25.2°C); \
             got {:.6} kW",
            ports.electrical.net_active_kw()
        );
    }

    /// MSHP crankcase defaults (15 W / 0 °C) must be active even when step() is
    /// called without a prior init().  The distinguishing condition uses an outdoor
    /// temp between the MSHP threshold (0 °C) and the central-AC default (12.8 °C):
    ///   - MSHP correct:  5 °C >= 0 °C  → crankcase = 0.0 kW
    ///   - Central-AC wrong: 5 °C < 12.8 °C → crankcase = 0.05 kW (50 W)
    #[test]
    fn mshp_cooler_uses_correct_crankcase_defaults_without_init() {
        let cfg = base_config();
        let mut eq = HpCooler::mshp_cooler(cfg);

        // Zone in deadband (21 °C, between default heating=20 °C / cooling=24 °C),
        // outdoor at 5 °C — above the MSHP 0 °C crankcase threshold but below the
        // central-AC 12.8 °C threshold. No init() call.
        let env = cooling_env(21.0, 5.0);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        assert_eq!(
            ports.electrical.net_active_kw(),
            0.0,
            "MSHP crankcase must be inactive at 5 °C (threshold 0 °C); \
             central-AC default (12.8 °C) would produce 0.05 kW — got {}",
            ports.electrical.net_active_kw()
        );
    }
}
