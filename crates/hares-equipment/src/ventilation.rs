//! Mechanical ventilation equipment: exhaust fans, HRV, and ERV.
//!
//! Models residential mechanical ventilation per ASHRAE 62.2 with optional
//! heat/energy recovery. Better than OCHRE (which uses ScheduledLoad) by
//! computing actual supply air conditions from recovery effectiveness.
//!
//! EnergyPlus-grade physics:
//! - `T_supply = T_outdoor + ε_sensible × (T_indoor - T_outdoor)`
//! - `W_supply = W_outdoor + ε_latent × (W_indoor - W_outdoor)` (ERV only)
//! - Bypass mode for free cooling when outdoor conditions are favorable
//! - Defrost derating at low outdoor temperatures

use std::borrow::Cow;
use std::time::Duration;

use hares_physics::constants::{CP_DRY_AIR_J_KG_K, LATENT_HEAT_VAPORISATION_J_KG};
use hares_types::{
    ControlCapabilities, ControlSignal, DRLevel, EndUse, EnvironmentState, EquipmentDescriptor,
    EquipmentId, ExecutionStage, FuelType, HaresError, OperatingMode, PortContribution,
    PortDeclaration, PortSlots, ScheduleSource, Telemetry, TelemetryField, ThermalCategory, ZoneId,
};
use serde::{Deserialize, Serialize};

use crate::schedule_helpers::{
    ScheduleSourceState, capture_schedule_source_state, restore_schedule_source_state,
};
use crate::{Equipment, EquipmentConfig, EquipmentRegistry, load_postcard, save_postcard};

const KEY_EQUIPMENT_ID: &str = "equipment_id";
const KEY_ZONE_ID: &str = "zone_id";
const KEY_FAN_POWER_W: &str = "fan_power_w";
const KEY_FLOW_RATE_M3_S: &str = "flow_rate_m3_s";
const KEY_SENSIBLE_EFFECTIVENESS: &str = "sensible_effectiveness";
const KEY_LATENT_EFFECTIVENESS: &str = "latent_effectiveness";
const KEY_BYPASS_TEMP_MIN_C: &str = "bypass_temp_min_c";
const KEY_BYPASS_TEMP_MAX_C: &str = "bypass_temp_max_c";
const KEY_DEFROST_TEMP_C: &str = "defrost_temp_c";
const KEY_DEFROST_EFFECTIVENESS_FRACTION: &str = "defrost_effectiveness_fraction";
const KEY_SCHEDULE_SOURCE: &str = "schedule_source";
const KEY_SCHEDULE_CONSTANT: &str = "schedule_constant";

const DEFAULT_FAN_POWER_W: f64 = 50.0;
const DEFAULT_FLOW_RATE_M3_S: f64 = 0.035; // ~75 CFM, typical residential
const DEFAULT_SENSIBLE_EFFECTIVENESS: f64 = 0.70;
const DEFAULT_LATENT_EFFECTIVENESS: f64 = 0.0; // HRV default (no latent recovery)
const DEFAULT_BYPASS_TEMP_MIN_C: f64 = 18.0;
const DEFAULT_BYPASS_TEMP_MAX_C: f64 = 24.0;
const DEFAULT_DEFROST_TEMP_C: f64 = -5.0;
const DEFAULT_DEFROST_EFFECTIVENESS_FRACTION: f64 = 0.5;

/// Standard air density [kg/m³] at sea level, 20°C.
const AIR_DENSITY_KG_M3: f64 = 1.2;

/// Ventilation type determines whether latent recovery is modeled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum VentilationType {
    /// Simple exhaust fan — no recovery.
    ExhaustFan,
    /// Heat recovery ventilator — sensible recovery only.
    Hrv,
    /// Energy recovery ventilator — sensible + latent recovery.
    Erv,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct VentilationCheckpoint {
    mode: OperatingMode,
    dr_level: DRLevel,
    dr_duration_remaining_s: Option<f64>,
    schedule_source_state: ScheduleSourceState,
}

pub struct Ventilation {
    descriptor: EquipmentDescriptor,
    ports: Vec<PortDeclaration>,
    telemetry: Telemetry,

    ventilation_type: VentilationType,
    zone_id: ZoneId,
    fan_power_w: f64,
    flow_rate_m3_s: f64,
    sensible_effectiveness: f64,
    latent_effectiveness: f64,

    // Bypass: when outdoor temp is within comfort range, bypass recovery (free cooling).
    bypass_temp_min_c: f64,
    bypass_temp_max_c: f64,

    // Defrost: at low outdoor temps, reduce effectiveness.
    defrost_temp_c: f64,
    defrost_effectiveness_fraction: f64,

    schedule_source: ScheduleSource,

    mode: OperatingMode,
    dr_level: DRLevel,
    dr_duration_remaining_s: Option<f64>,
}

impl Ventilation {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        let equipment_id = config
            .get_f64(KEY_EQUIPMENT_ID)
            .map(|v| v as u32)
            .unwrap_or(0);
        let zone_id = config
            .get_f64(KEY_ZONE_ID)
            .map(|v| ZoneId(v as u16))
            .unwrap_or(ZoneId(1));

        let ventilation_type = match config.get_str("ventilation_type").unwrap_or("hrv") {
            "exhaust_fan" | "ExhaustFan" => VentilationType::ExhaustFan,
            "erv" | "ERV" => VentilationType::Erv,
            _ => VentilationType::Hrv,
        };

        let end_use = match ventilation_type {
            VentilationType::ExhaustFan => EndUse::VENTILATION,
            VentilationType::Hrv | VentilationType::Erv => EndUse::VENTILATION,
        };

        let descriptor = EquipmentDescriptor {
            id: EquipmentId(equipment_id),
            name: config.name.clone(),
            end_use,
            equipment_type: Cow::Borrowed(match ventilation_type {
                VentilationType::ExhaustFan => "ExhaustFan",
                VentilationType::Hrv => "HRV",
                VentilationType::Erv => "ERV",
            }),
            zone: Some(zone_id),
            fuel: FuelType::Electric,
            stage: ExecutionStage::Thermal,
            control_capabilities: ControlCapabilities::MODE_OVERRIDE
                | ControlCapabilities::DEMAND_RESPONSE
                | ControlCapabilities::LOAD_FRACTION,
            telemetry_fields: telemetry_fields(),
        };

        let ports = vec![
            PortDeclaration::electrical(),
            PortDeclaration::thermal(zone_id),
        ];

        Self {
            descriptor,
            ports,
            telemetry: default_telemetry(),
            ventilation_type,
            zone_id,
            fan_power_w: DEFAULT_FAN_POWER_W,
            flow_rate_m3_s: DEFAULT_FLOW_RATE_M3_S,
            sensible_effectiveness: DEFAULT_SENSIBLE_EFFECTIVENESS,
            latent_effectiveness: DEFAULT_LATENT_EFFECTIVENESS,
            bypass_temp_min_c: DEFAULT_BYPASS_TEMP_MIN_C,
            bypass_temp_max_c: DEFAULT_BYPASS_TEMP_MAX_C,
            defrost_temp_c: DEFAULT_DEFROST_TEMP_C,
            defrost_effectiveness_fraction: DEFAULT_DEFROST_EFFECTIVENESS_FRACTION,
            schedule_source: ScheduleSource::Constant(1.0),
            mode: OperatingMode::Off,
            dr_level: DRLevel::Normal,
            dr_duration_remaining_s: None,
        }
    }

    /// Effective sensible effectiveness after bypass and defrost adjustments.
    fn effective_sensible_effectiveness(&self, t_outdoor_c: f64) -> f64 {
        if self.ventilation_type == VentilationType::ExhaustFan {
            return 0.0;
        }
        // Bypass: when outdoor is within comfort range, bypass recovery entirely.
        if t_outdoor_c >= self.bypass_temp_min_c && t_outdoor_c <= self.bypass_temp_max_c {
            return 0.0;
        }
        // Defrost: at very low outdoor temps, reduce effectiveness.
        let base = self.sensible_effectiveness;
        if t_outdoor_c < self.defrost_temp_c {
            base * self.defrost_effectiveness_fraction
        } else {
            base
        }
    }

    /// Effective latent effectiveness (ERV only).
    fn effective_latent_effectiveness(&self, t_outdoor_c: f64) -> f64 {
        if self.ventilation_type != VentilationType::Erv {
            return 0.0;
        }
        if t_outdoor_c >= self.bypass_temp_min_c && t_outdoor_c <= self.bypass_temp_max_c {
            return 0.0;
        }
        let base = self.latent_effectiveness;
        if t_outdoor_c < self.defrost_temp_c {
            base * self.defrost_effectiveness_fraction
        } else {
            base
        }
    }
}

impl Equipment for Ventilation {
    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.descriptor
    }

    fn ports(&self) -> &[PortDeclaration] {
        &self.ports
    }

    fn init(&mut self, config: &EquipmentConfig, _env: &EnvironmentState) -> crate::Result<()> {
        self.fan_power_w = config
            .get_f64(KEY_FAN_POWER_W)
            .unwrap_or(DEFAULT_FAN_POWER_W);
        self.flow_rate_m3_s = config
            .get_f64(KEY_FLOW_RATE_M3_S)
            .unwrap_or(DEFAULT_FLOW_RATE_M3_S);
        self.sensible_effectiveness = config
            .get_f64(KEY_SENSIBLE_EFFECTIVENESS)
            .unwrap_or(DEFAULT_SENSIBLE_EFFECTIVENESS)
            .clamp(0.0, 1.0);
        self.latent_effectiveness = config
            .get_f64(KEY_LATENT_EFFECTIVENESS)
            .unwrap_or(DEFAULT_LATENT_EFFECTIVENESS)
            .clamp(0.0, 1.0);
        self.bypass_temp_min_c = config
            .get_f64(KEY_BYPASS_TEMP_MIN_C)
            .unwrap_or(DEFAULT_BYPASS_TEMP_MIN_C);
        self.bypass_temp_max_c = config
            .get_f64(KEY_BYPASS_TEMP_MAX_C)
            .unwrap_or(DEFAULT_BYPASS_TEMP_MAX_C);
        self.defrost_temp_c = config
            .get_f64(KEY_DEFROST_TEMP_C)
            .unwrap_or(DEFAULT_DEFROST_TEMP_C);
        self.defrost_effectiveness_fraction = config
            .get_f64(KEY_DEFROST_EFFECTIVENESS_FRACTION)
            .unwrap_or(DEFAULT_DEFROST_EFFECTIVENESS_FRACTION)
            .clamp(0.0, 1.0);

        if self.flow_rate_m3_s < 0.0 || !self.flow_rate_m3_s.is_finite() {
            return Err(HaresError::Equipment(
                "ventilation flow_rate_m3_s must be finite and >= 0".to_string(),
            ));
        }
        if self.fan_power_w < 0.0 || !self.fan_power_w.is_finite() {
            return Err(HaresError::Equipment(
                "ventilation fan_power_w must be finite and >= 0".to_string(),
            ));
        }

        self.schedule_source = match config.get_str(KEY_SCHEDULE_SOURCE) {
            Some("constant") | None => {
                let v = config.get_f64(KEY_SCHEDULE_CONSTANT).unwrap_or(1.0);
                ScheduleSource::Constant(v)
            }
            Some(other) => {
                return Err(HaresError::Equipment(format!(
                    "ventilation: unsupported schedule_source '{other}' (only 'constant' supported)"
                )));
            }
        };

        self.mode = OperatingMode::Standby;
        Ok(())
    }

    fn update_control(&mut self, env: &EnvironmentState) -> OperatingMode {
        if let Some(remaining) = self.dr_duration_remaining_s.as_mut() {
            *remaining -= env.time_res.num_seconds() as f64;
            if *remaining <= 0.0 {
                self.dr_level = DRLevel::Normal;
                self.dr_duration_remaining_s = None;
            }
        }
        self.mode
    }

    fn step(
        &mut self,
        env: &EnvironmentState,
        _dt: Duration,
        ports: &mut PortSlots,
    ) -> std::result::Result<(), HaresError> {
        let is_running = self.mode != OperatingMode::Off && self.dr_level != DRLevel::GridEmergency;

        if !is_running {
            self.telemetry.set("fan_power_w", 0.0);
            self.telemetry.set("sensible_recovery_w", 0.0);
            self.telemetry.set("latent_recovery_w", 0.0);
            self.telemetry
                .set("supply_temp_c", env.weather.outdoor_temp_c);
            self.telemetry.set("bypass_active", 0.0);
            return Ok(());
        }

        let schedule_frac = self.schedule_source.value_at(env)?.clamp(0.0, 1.0);
        let effective_flow_rate_m3_s = self.flow_rate_m3_s * schedule_frac;
        let effective_fan_power_w = self.fan_power_w * schedule_frac;

        if effective_flow_rate_m3_s <= 0.0 {
            self.telemetry.set("fan_power_w", 0.0);
            self.telemetry.set("sensible_recovery_w", 0.0);
            self.telemetry.set("latent_recovery_w", 0.0);
            self.telemetry
                .set("supply_temp_c", env.weather.outdoor_temp_c);
            self.telemetry.set("bypass_active", 0.0);
            return Ok(());
        }

        let t_outdoor_c = env.weather.outdoor_temp_c;
        let zone = env.zones.iter().find(|z| z.id == self.zone_id);
        let t_indoor_c = zone.map(|z| z.temperature_c).unwrap_or(20.0);
        let w_indoor = zone.map(|z| z.humidity_ratio).unwrap_or(0.008);
        let w_outdoor = env.weather.outdoor_humidity_ratio;

        let eff_s = self.effective_sensible_effectiveness(t_outdoor_c);
        let eff_l = self.effective_latent_effectiveness(t_outdoor_c);
        let bypass_active = eff_s == 0.0
            && t_outdoor_c >= self.bypass_temp_min_c
            && t_outdoor_c <= self.bypass_temp_max_c;

        // Supply air conditions after heat recovery
        let t_supply_c = t_outdoor_c + eff_s * (t_indoor_c - t_outdoor_c);
        let w_supply = w_outdoor + eff_l * (w_indoor - w_outdoor);

        // Mass flow rate [kg/s]
        let m_dot_kg_s = effective_flow_rate_m3_s * AIR_DENSITY_KG_M3;

        // Sensible ventilation load to zone [W]:
        // Positive = heating the zone (supply warmer than outdoor but still cooler than indoor).
        // The load the zone experiences is the difference between supply air and indoor air.
        let q_sensible_w = m_dot_kg_s * CP_DRY_AIR_J_KG_K * (t_supply_c - t_indoor_c);

        // Latent ventilation load to zone [W]:
        // Uses latent heat of vaporization (~2,450,000 J/kg).
        let q_latent_w = m_dot_kg_s * LATENT_HEAT_VAPORISATION_J_KG * (w_supply - w_indoor);

        // Sensible recovery [W] — how much the HRV/ERV saved vs raw ventilation
        let q_recovery_sensible_w =
            m_dot_kg_s * CP_DRY_AIR_J_KG_K * eff_s * (t_indoor_c - t_outdoor_c);
        let q_recovery_latent_w =
            m_dot_kg_s * LATENT_HEAT_VAPORISATION_J_KG * eff_l * (w_indoor - w_outdoor);

        // Fan electrical power [kW]
        let fan_kw = effective_fan_power_w / 1000.0;

        // Write ports
        ports.accumulate(&PortContribution::Electrical {
            active_power_kw: fan_kw,
            reactive_power_kvar: 0.0,
        })?;

        ports.accumulate(&PortContribution::Thermal {
            zone: self.zone_id,
            sensible_gain_w: q_sensible_w,
            latent_gain_w: q_latent_w,
            category: ThermalCategory::InternalGain,
        })?;

        // Telemetry
        self.telemetry.set("fan_power_w", effective_fan_power_w);
        self.telemetry
            .set("sensible_recovery_w", q_recovery_sensible_w);
        self.telemetry.set("latent_recovery_w", q_recovery_latent_w);
        self.telemetry.set("supply_temp_c", t_supply_c);
        self.telemetry
            .set("bypass_active", if bypass_active { 1.0 } else { 0.0 });

        Ok(())
    }

    fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    fn save_state(&self) -> Vec<u8> {
        save_postcard(&VentilationCheckpoint {
            mode: self.mode,
            dr_level: self.dr_level,
            dr_duration_remaining_s: self.dr_duration_remaining_s,
            schedule_source_state: capture_schedule_source_state(&self.schedule_source),
        })
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        let cp: VentilationCheckpoint = load_postcard(state)?;
        self.mode = cp.mode;
        self.dr_level = cp.dr_level;
        self.dr_duration_remaining_s = cp.dr_duration_remaining_s;
        restore_schedule_source_state(&mut self.schedule_source, &cp.schedule_source_state)?;
        Ok(())
    }

    fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        match signal {
            ControlSignal::ModeOverride { mode } => {
                self.mode = *mode;
            }
            ControlSignal::DemandResponse { level, duration_s } => {
                self.dr_level = *level;
                self.dr_duration_remaining_s = *duration_s;
            }
            ControlSignal::LoadFraction { fraction } => {
                if *fraction <= 0.0 {
                    self.mode = OperatingMode::Off;
                } else {
                    self.mode = OperatingMode::Standby;
                }
            }
            _ => {
                return Err(HaresError::Control(format!(
                    "Ventilation does not handle control signal: {signal:?}"
                )));
            }
        }
        Ok(())
    }
}

pub fn register_with_registry(registry: &mut EquipmentRegistry) {
    registry.register(
        "Ventilation Fan",
        Box::new(|config| Box::new(Ventilation::new(config))),
    );
    registry.register("HRV", Box::new(|config| Box::new(Ventilation::new(config))));
    registry.register("ERV", Box::new(|config| Box::new(Ventilation::new(config))));
}

fn default_telemetry() -> Telemetry {
    let mut t = Telemetry::with_capacity(5);
    t.insert("fan_power_w", 0.0);
    t.insert("sensible_recovery_w", 0.0);
    t.insert("latent_recovery_w", 0.0);
    t.insert("supply_temp_c", 20.0);
    t.insert("bypass_active", 0.0);
    t
}

fn telemetry_fields() -> Vec<TelemetryField> {
    vec![
        TelemetryField {
            name: "fan_power_w".to_string(),
            unit: "W".to_string(),
            description: "Fan electrical power consumption".to_string(),
        },
        TelemetryField {
            name: "sensible_recovery_w".to_string(),
            unit: "W".to_string(),
            description: "Sensible heat recovered by HRV/ERV".to_string(),
        },
        TelemetryField {
            name: "latent_recovery_w".to_string(),
            unit: "W".to_string(),
            description: "Latent heat recovered by ERV".to_string(),
        },
        TelemetryField {
            name: "supply_temp_c".to_string(),
            unit: "C".to_string(),
            description: "Supply air temperature after recovery".to_string(),
        },
        TelemetryField {
            name: "bypass_active".to_string(),
            unit: "-".to_string(),
            description: "Bypass mode active (1 = bypassing recovery)".to_string(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, TimeZone};
    use hares_types::{GridState, PortSlots, ThermalAccumulator, WeatherState, ZoneState};

    fn env(outdoor_c: f64, indoor_c: f64) -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: indoor_c,
                humidity_ratio: 0.008,
                relative_humidity: 0.5,
                wet_bulb_c: 14.0,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: outdoor_c,
                outdoor_humidity_ratio: 0.003,
                ..Default::default()
            },
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
            },
            custom_domains: vec![],
            current_time: FixedOffset::east_opt(0)
                .expect("UTC")
                .with_ymd_and_hms(2026, 1, 15, 12, 0, 0)
                .single()
                .expect("valid time"),
            time_res: chrono::TimeDelta::minutes(5),
        }
    }

    fn hrv_config() -> EquipmentConfig {
        let mut raw = std::collections::HashMap::new();
        raw.insert("zone_id".to_string(), 1.0.into());
        raw.insert("ventilation_type".to_string(), "hrv".into());
        raw.insert(KEY_SENSIBLE_EFFECTIVENESS.to_string(), 0.70.into());
        raw.insert(KEY_FLOW_RATE_M3_S.to_string(), 0.035.into());
        raw.insert(KEY_FAN_POWER_W.to_string(), 50.0.into());
        EquipmentConfig {
            name: "HRV".to_string(),
            ochre_class: "HRV".to_string(),
            raw_config: raw,
        }
    }

    fn erv_config() -> EquipmentConfig {
        let mut raw = std::collections::HashMap::new();
        raw.insert("zone_id".to_string(), 1.0.into());
        raw.insert("ventilation_type".to_string(), "erv".into());
        raw.insert(KEY_SENSIBLE_EFFECTIVENESS.to_string(), 0.70.into());
        raw.insert(KEY_LATENT_EFFECTIVENESS.to_string(), 0.50.into());
        raw.insert(KEY_FLOW_RATE_M3_S.to_string(), 0.035.into());
        raw.insert(KEY_FAN_POWER_W.to_string(), 60.0.into());
        EquipmentConfig {
            name: "ERV".to_string(),
            ochre_class: "ERV".to_string(),
            raw_config: raw,
        }
    }

    #[test]
    fn hrv_supply_temp_with_70pct_effectiveness_no_defrost() {
        // Use 0°C outdoor (above defrost threshold of -5°C) so full 70% effectiveness applies.
        let cfg = hrv_config();
        let mut hrv = Ventilation::new(cfg.clone());
        let e = env(0.0, 20.0);
        hrv.init(&cfg, &e).expect("init");

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        hrv.step(&e, Duration::from_secs(300), &mut ports)
            .expect("step");

        // T_supply = 0 + 0.70 * (20 - 0) = 14°C
        let t_supply = hrv.telemetry().get("supply_temp_c").expect("supply_temp_c");
        assert!(
            (t_supply - 14.0).abs() < 0.5,
            "HRV supply at 0°C outdoor / 20°C indoor / 70% eff should be ~14°C, got {t_supply}"
        );
    }

    #[test]
    fn hrv_supply_temp_at_minus_20c_with_defrost_derating() {
        // At -20°C (below defrost threshold -5°C), effectiveness is halved to 35%.
        let cfg = hrv_config();
        let mut hrv = Ventilation::new(cfg.clone());
        let e = env(-20.0, 20.0);
        hrv.init(&cfg, &e).expect("init");

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        hrv.step(&e, Duration::from_secs(300), &mut ports)
            .expect("step");

        // T_supply = -20 + 0.35 * (20 - (-20)) = -20 + 14 = -6°C
        let t_supply = hrv.telemetry().get("supply_temp_c").expect("supply_temp_c");
        assert!(
            (t_supply - (-6.0)).abs() < 0.5,
            "HRV at -20°C with defrost (35% eff) should give ~-6°C supply, got {t_supply}"
        );
    }

    #[test]
    fn hrv_reduces_ventilation_heating_load() {
        // Use 0°C outdoor (above defrost threshold) so full 70% effectiveness.
        let cfg = hrv_config();
        let mut hrv = Ventilation::new(cfg.clone());
        let e = env(0.0, 20.0);
        hrv.init(&cfg, &e).expect("init");

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        hrv.step(&e, Duration::from_secs(300), &mut ports)
            .expect("step");

        let sensible = ports.thermal[0].sensible_gain_w;
        assert!(
            sensible < 0.0,
            "ventilation should cool the zone (outdoor colder)"
        );

        let recovery = hrv
            .telemetry()
            .get("sensible_recovery_w")
            .expect("recovery");
        assert!(recovery > 0.0, "HRV should recover positive sensible heat");

        // m_dot = 0.035 * 1.2 = 0.042 kg/s
        // Raw load = 0.042 * 1006 * 20 = 845 W
        // Recovery = 0.042 * 1006 * 0.70 * 20 = 591 W
        // Reduction = 591/845 = 70%
        let m_dot = 0.035 * AIR_DENSITY_KG_M3;
        let raw_load = m_dot * CP_DRY_AIR_J_KG_K * 20.0;
        let reduction = recovery / raw_load;
        assert!(
            reduction > 0.6 && reduction < 0.8,
            "HRV should reduce heating load by 60-80%, got {:.0}%",
            reduction * 100.0
        );
    }

    #[test]
    fn erv_also_reduces_latent_load() {
        let cfg = erv_config();
        let mut erv = Ventilation::new(cfg.clone());
        let e = env(0.0, 20.0); // above defrost threshold
        erv.init(&cfg, &e).expect("init");

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        erv.step(&e, Duration::from_secs(300), &mut ports)
            .expect("step");

        let latent_recovery = erv.telemetry().get("latent_recovery_w").expect("latent");
        assert!(
            latent_recovery > 0.0,
            "ERV should recover positive latent heat, got {latent_recovery}"
        );
    }

    #[test]
    fn fan_power_appears_in_electrical_port() {
        let cfg = hrv_config();
        let mut hrv = Ventilation::new(cfg.clone());
        let e = env(5.0, 20.0);
        hrv.init(&cfg, &e).expect("init");

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        hrv.step(&e, Duration::from_secs(300), &mut ports)
            .expect("step");

        let electric_kw = ports.electrical.net_active_kw();
        assert!(
            (electric_kw - 0.05).abs() < 0.001,
            "fan power should be 50W = 0.05 kW, got {electric_kw}"
        );
    }

    #[test]
    fn bypass_activates_in_comfort_range() {
        let cfg = hrv_config();
        let mut hrv = Ventilation::new(cfg.clone());
        let e = env(21.0, 22.0); // outdoor within [18, 24] comfort range
        hrv.init(&cfg, &e).expect("init");

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        hrv.step(&e, Duration::from_secs(300), &mut ports)
            .expect("step");

        let bypass = hrv.telemetry().get("bypass_active").expect("bypass");
        assert!(
            (bypass - 1.0).abs() < 0.01,
            "bypass should be active when outdoor is in comfort range"
        );
        let recovery = hrv
            .telemetry()
            .get("sensible_recovery_w")
            .expect("recovery");
        assert!(
            recovery.abs() < 0.1,
            "no recovery expected during bypass, got {recovery}"
        );
    }

    #[test]
    fn defrost_reduces_effectiveness_at_low_temps() {
        let cfg = hrv_config();
        let mut hrv = Ventilation::new(cfg.clone());
        let cold_env = env(-20.0, 20.0); // below defrost threshold (-5°C)
        let mild_env = env(0.0, 20.0); // above defrost threshold
        hrv.init(&cfg, &cold_env).expect("init");

        let mut ports_cold = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        hrv.step(&cold_env, Duration::from_secs(300), &mut ports_cold)
            .expect("step cold");
        let _recovery_cold = hrv.telemetry().get("sensible_recovery_w").expect("cold");

        let mut ports_mild = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        hrv.step(&mild_env, Duration::from_secs(300), &mut ports_mild)
            .expect("step mild");

        let eff_cold = hrv.effective_sensible_effectiveness(-20.0);
        let eff_mild = hrv.effective_sensible_effectiveness(0.0);
        assert!(
            eff_cold < eff_mild,
            "defrost should reduce effectiveness: cold={eff_cold}, mild={eff_mild}"
        );
    }

    #[test]
    fn exhaust_fan_has_no_recovery() {
        let mut raw = std::collections::HashMap::new();
        raw.insert("zone_id".to_string(), 1.0.into());
        raw.insert("ventilation_type".to_string(), "exhaust_fan".into());
        raw.insert(KEY_FAN_POWER_W.to_string(), 30.0.into());
        raw.insert(KEY_FLOW_RATE_M3_S.to_string(), 0.025.into());
        let cfg = EquipmentConfig {
            name: "Exhaust".to_string(),
            ochre_class: "Ventilation Fan".to_string(),
            raw_config: raw,
        };
        let mut fan = Ventilation::new(cfg.clone());
        let e = env(-10.0, 20.0);
        fan.init(&cfg, &e).expect("init");

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        fan.step(&e, Duration::from_secs(300), &mut ports)
            .expect("step");

        let recovery = fan
            .telemetry()
            .get("sensible_recovery_w")
            .expect("recovery");
        assert!(
            recovery.abs() < 0.01,
            "exhaust fan should have zero recovery, got {recovery}"
        );
    }

    #[test]
    fn registry_includes_ventilation_types() {
        let registry = EquipmentRegistry::new();
        assert!(registry.get("HRV").is_some());
        assert!(registry.get("ERV").is_some());
        assert!(registry.get("Ventilation Fan").is_some());
    }

    #[test]
    fn constant_half_schedule_halves_flow_and_power() {
        let mut raw = std::collections::HashMap::new();
        raw.insert("zone_id".to_string(), 1.0.into());
        raw.insert("ventilation_type".to_string(), "hrv".into());
        raw.insert(KEY_FAN_POWER_W.to_string(), 50.0.into());
        raw.insert(KEY_FLOW_RATE_M3_S.to_string(), 0.035.into());
        raw.insert(KEY_SENSIBLE_EFFECTIVENESS.to_string(), 0.70.into());
        raw.insert(KEY_SCHEDULE_SOURCE.to_string(), "constant".into());
        raw.insert(KEY_SCHEDULE_CONSTANT.to_string(), 0.5.into());
        let cfg = EquipmentConfig {
            name: "HRV-half".to_string(),
            ochre_class: "HRV".to_string(),
            raw_config: raw,
        };

        let e = env(0.0, 20.0);
        let mut hrv = Ventilation::new(cfg.clone());
        hrv.init(&cfg, &e).expect("init");

        // Run full-schedule reference first (separate instance)
        let mut raw_full = std::collections::HashMap::new();
        raw_full.insert("zone_id".to_string(), 1.0.into());
        raw_full.insert("ventilation_type".to_string(), "hrv".into());
        raw_full.insert(KEY_FAN_POWER_W.to_string(), 50.0.into());
        raw_full.insert(KEY_FLOW_RATE_M3_S.to_string(), 0.035.into());
        raw_full.insert(KEY_SENSIBLE_EFFECTIVENESS.to_string(), 0.70.into());
        let cfg_full = EquipmentConfig {
            name: "HRV-full".to_string(),
            ochre_class: "HRV".to_string(),
            raw_config: raw_full,
        };
        let mut hrv_full = Ventilation::new(cfg_full.clone());
        hrv_full.init(&cfg_full, &e).expect("init full");

        let mut ports_half = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        hrv.step(&e, Duration::from_secs(300), &mut ports_half)
            .expect("step half");

        let mut ports_full = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        hrv_full
            .step(&e, Duration::from_secs(300), &mut ports_full)
            .expect("step full");

        let power_half = hrv
            .telemetry()
            .get("fan_power_w")
            .expect("fan_power_w half");
        let power_full = hrv_full
            .telemetry()
            .get("fan_power_w")
            .expect("fan_power_w full");
        assert!(
            (power_half - power_full * 0.5).abs() < 0.01,
            "half-schedule should halve fan power: got {power_half}, expected {}",
            power_full * 0.5
        );

        // Thermal port sensible gain should also be halved
        let sensible_half = ports_half.thermal[0].sensible_gain_w;
        let sensible_full = ports_full.thermal[0].sensible_gain_w;
        assert!(
            (sensible_half - sensible_full * 0.5).abs() < 0.1,
            "half-schedule should halve thermal gain: got {sensible_half}, expected {}",
            sensible_full * 0.5
        );

        // Electrical port should also be halved
        let elec_half = ports_half.electrical.net_active_kw();
        let elec_full = ports_full.electrical.net_active_kw();
        assert!(
            (elec_half - elec_full * 0.5).abs() < 0.001,
            "half-schedule should halve electrical draw: got {elec_half}, expected {}",
            elec_full * 0.5
        );
    }
}
