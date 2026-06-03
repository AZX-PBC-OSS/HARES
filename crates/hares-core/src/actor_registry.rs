//! Actor registry for dynamic actor creation.
//!
//! Mirrors the [`EquipmentRegistry`](hares_equipment::EquipmentRegistry) pattern
//! for actors. Built-in actors are auto-registered; users can register custom
//! actors for both Rust and Python consumers.
//!
//! # Example
//!
//! ```ignore
//! use hares_core::actor_registry::{ActorRegistry, ActorConfig};
//!
//! let registry = ActorRegistry::new();
//! let config = ActorConfig::new("Thermostat", "IdealThermostat");
//! let actor = registry.create(config)?;
//! ```
//!
//! # ConfigValue Types
//!
//! Actor parameters use [`ConfigValue`] from `hares_equipment::config`:
//!
//! | Variant | Rust Type | Python Type | Example |
//! |---------|-----------|-------------|---------|
//! | `Float` | `f64` | `float` | `20.0` |
//! | `Text` | `String` | `str` | `"HVAC"` |
//! | `Bool` | `bool` | `bool` | `true` |
//! | `FloatArray` | `Vec<f64>` | `list[float]` | `[20.0, 22.0]` |

use std::collections::HashMap;
use std::sync::Arc;

use hares_control::DispatchTarget;
use hares_equipment::config::ConfigValue;
use hares_types::{DRLevel, EndUse, HaresError};

use hares_equipment::ev::catalog::archetype_by_id;
use hares_types::{BmsMode, ChargingStrategy, GridExportRule, PlugInPolicy, ScheduleSource};

use crate::Actor;
use crate::actors::{
    AlwaysComply, BatteryManagementActor, DrAction, DrCompliance, EquipmentBehavior, EvDriverActor,
    IdealThermostat, Occupant, Presence, Probabilistic, SafetyMonitor,
};

pub type ActorFactory =
    Box<dyn Fn(ActorConfig) -> Result<Box<dyn Actor>, HaresError> + Send + Sync + 'static>;

/// Initialization parameters for one actor instance.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ActorConfig {
    pub name: String,
    pub actor_type: String,
    pub parameters: HashMap<String, ConfigValue>,
}

impl ActorConfig {
    pub fn new(name: impl Into<String>, actor_type: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            actor_type: actor_type.into(),
            parameters: HashMap::new(),
        }
    }

    pub fn with_param(mut self, key: impl Into<String>, value: ConfigValue) -> Self {
        self.parameters.insert(key.into(), value);
        self
    }

    pub fn get_f64(&self, key: &str) -> Option<f64> {
        self.parameters.get(key).and_then(ConfigValue::as_f64)
    }

    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.parameters.get(key).and_then(ConfigValue::as_str)
    }

    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.parameters.get(key).and_then(ConfigValue::as_bool)
    }

    pub fn get_f64_array(&self, key: &str) -> Option<&[f64]> {
        self.parameters.get(key).and_then(ConfigValue::as_f64_array)
    }
}

fn parse_dispatch_target(raw: &str) -> Result<DispatchTarget, HaresError> {
    if let Some(rest) = raw.strip_prefix("name:") {
        if rest.is_empty() {
            return Err(HaresError::Control(
                "dispatch target 'name:' requires a non-empty equipment name".into(),
            ));
        }
        Ok(DispatchTarget::ByName(Arc::from(rest)))
    } else if let Some(rest) = raw.strip_prefix("end_use:") {
        if rest.is_empty() {
            return Err(HaresError::Control(
                "dispatch target 'end_use:' requires a non-empty end-use string".into(),
            ));
        }
        Ok(DispatchTarget::ByEndUse(EndUse::custom(rest.to_string())))
    } else {
        Err(HaresError::Control(format!(
            "invalid dispatch target '{raw}': must start with 'name:' or 'end_use:'"
        )))
    }
}

fn parse_dr_action(raw: &str) -> Result<DrAction, HaresError> {
    match raw {
        "TurnOff" => Ok(DrAction::TurnOff),
        "None" => Ok(DrAction::None),
        s if s.starts_with("DemandResponse:") => {
            let rest = s.strip_prefix("DemandResponse:").unwrap();
            let colon = rest.find(':');
            let (level_str, dur_str) = match colon {
                Some(i) => (&rest[..i], Some(&rest[i + 1..])),
                None => (rest, None),
            };
            let level = match level_str {
                "Normal" => DRLevel::Normal,
                "Moderate" => DRLevel::Moderate,
                "High" => DRLevel::High,
                "Critical" => DRLevel::Critical,
                "GridEmergency" => DRLevel::GridEmergency,
                unknown => {
                    return Err(HaresError::Control(format!(
                        "unknown DR level '{unknown}' in DemandResponse for '{raw}': \
                         valid levels are Normal, Moderate, High, Critical, GridEmergency"
                    )));
                }
            };
            let duration_s = match dur_str {
                Some("inf") | Some("Inf") | Some("none") | Some("None") => None,
                Some(s) => Some(s.parse::<f64>().map_err(|_| {
                    HaresError::Control(format!("invalid duration_s for DemandResponse in '{raw}'"))
                })?),
                None => None,
            };
            Ok(DrAction::demand_response(level, duration_s))
        }
        s if s.starts_with("SetpointAdjust:") => {
            let val: f64 = s
                .strip_prefix("SetpointAdjust:")
                .unwrap()
                .parse()
                .map_err(|_| {
                    HaresError::Control(format!("invalid delta_c for SetpointAdjust in '{raw}'"))
                })?;
            Ok(DrAction::setpoint_delta(val))
        }
        s if s.starts_with("LoadCurtailment:") => {
            let val: f64 = s
                .strip_prefix("LoadCurtailment:")
                .unwrap()
                .parse()
                .map_err(|_| {
                    HaresError::Control(format!("invalid fraction for LoadCurtailment in '{raw}'"))
                })?;
            Ok(DrAction::curtail(val))
        }
        s if s.starts_with("PowerLimit:") => {
            let val: f64 = s
                .strip_prefix("PowerLimit:")
                .unwrap()
                .parse()
                .map_err(|_| {
                    HaresError::Control(format!("invalid max_kw for PowerLimit in '{raw}'"))
                })?;
            Ok(DrAction::limit_power(val))
        }
        s if s.starts_with("AbsoluteSetpoint:") => {
            let rest = s.strip_prefix("AbsoluteSetpoint:").unwrap();
            let parts: Vec<&str> = rest.splitn(2, ':').collect();
            if parts.len() != 2 {
                return Err(HaresError::Control(format!(
                    "AbsoluteSetpoint requires heating_c:cooling_c, got '{rest}'"
                )));
            }
            let heat: f64 = parts[0].parse().map_err(|_| {
                HaresError::Control(format!("invalid heating_c for AbsoluteSetpoint in '{raw}'"))
            })?;
            let cool: f64 = parts[1].parse().map_err(|_| {
                HaresError::Control(format!("invalid cooling_c for AbsoluteSetpoint in '{raw}'"))
            })?;
            Ok(DrAction::absolute_setpoint(heat, cool))
        }
        _ => Err(HaresError::Control(format!(
            "unknown DrAction '{raw}': valid actions are TurnOff, SetpointAdjust:<delta_c>, \
             LoadCurtailment:<fraction>, PowerLimit:<max_kw>, \
             AbsoluteSetpoint:<heating_c>:<cooling_c>, \
             DemandResponse:<level>[:<duration_s>], None"
        ))),
    }
}

fn parse_load_target(raw: &str) -> Result<(DispatchTarget, DrAction), HaresError> {
    let (prefix, rest) = raw.split_once(':').ok_or_else(|| {
        HaresError::Control(format!(
            "invalid load target '{raw}': missing target prefix (name: or end_use:)"
        ))
    })?;
    match prefix {
        "name" => {
            let (equip_name, action_str) = rest.split_once(':').ok_or_else(|| {
                HaresError::Control(format!(
                    "load target '{raw}' missing action after equipment name"
                ))
            })?;
            if equip_name.is_empty() {
                return Err(HaresError::Control(format!(
                    "load target '{raw}' has empty equipment name"
                )));
            }
            let target = DispatchTarget::ByName(Arc::from(equip_name));
            let action = parse_dr_action(action_str)?;
            Ok((target, action))
        }
        "end_use" => {
            let (end_use_str, action_str) = rest.split_once(':').ok_or_else(|| {
                HaresError::Control(format!("load target '{raw}' missing action after end_use"))
            })?;
            if end_use_str.is_empty() {
                return Err(HaresError::Control(format!(
                    "load target '{raw}' has empty end_use"
                )));
            }
            let target = DispatchTarget::ByEndUse(EndUse::custom(end_use_str.to_string()));
            let action = parse_dr_action(action_str)?;
            Ok((target, action))
        }
        other => Err(HaresError::Control(format!(
            "load target prefix must be 'name' or 'end_use', got '{other}' in '{raw}'"
        ))),
    }
}

fn parse_load_targets(raw: &str) -> Result<Vec<(DispatchTarget, DrAction)>, HaresError> {
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    raw.split(';')
        .map(|s| parse_load_target(s.trim()))
        .collect()
}

/// Registry mapping actor type strings to actor constructors.
#[derive(Default)]
pub struct ActorRegistry {
    factories: HashMap<String, ActorFactory>,
}

impl ActorRegistry {
    #[must_use]
    pub fn new() -> Self {
        let mut registry = Self {
            factories: HashMap::new(),
        };
        Self::register_builtins(&mut registry);
        registry
    }

    fn register_builtins(registry: &mut Self) {
        registry.register(
            "IdealThermostat",
            Box::new(|config: ActorConfig| {
                let target = config.get_str("target").unwrap_or("HVAC");
                let mut actor = IdealThermostat::new(target).with_name(&config.name);
                if let (Some(heat), Some(cool)) =
                    (config.get_f64("heating_c"), config.get_f64("cooling_c"))
                {
                    actor = actor.with_setpoints(heat, cool);
                } else if let Some(heat) = config.get_f64("heating_c") {
                    actor = actor.with_heating_setpoint(heat);
                } else if let Some(cool) = config.get_f64("cooling_c") {
                    actor = actor.with_cooling_setpoint(cool);
                }
                if let Some(deadband) = config.get_f64("deadband_c") {
                    actor = actor.with_deadband(deadband);
                }
                if let Some(h) = config.get_f64("hysteresis_c") {
                    actor = actor.with_hysteresis(h);
                }
                Ok(Box::new(actor))
            }),
        );

        registry.register(
            "Occupant",
            Box::new(|config: ActorConfig| {
                let mut occupant = Occupant::new(&config.name);
                if let Some(target) = config.get_str("lighting_target") {
                    occupant = occupant
                        .with_lighting(target, EquipmentBehavior::default().off_when_away());
                }
                if let Some(occupancy_column) = config.get_f64_array("occupancy_column") {
                    let presence_schedule: Vec<Presence> = occupancy_column
                        .iter()
                        .map(|&v| {
                            if v > 0.0 {
                                Presence::Home
                            } else {
                                Presence::Away
                            }
                        })
                        .collect();
                    occupant = occupant.with_presence_schedule(presence_schedule);
                }
                Ok(Box::new(occupant))
            }),
        );

        registry.register(
            "EvDriver",
            Box::new(|config: ActorConfig| {
                let target = config
                    .get_str("target")
                    .ok_or_else(|| {
                        HaresError::Control("EvDriver requires 'target' parameter".into())
                    })?
                    .to_string();
                let seed = config.get_f64("seed").ok_or_else(|| {
                    HaresError::Control("EvDriver requires 'seed' parameter".into())
                })? as u64;

                // If a preset is specified, load defaults from the archetype catalog
                let preset = config.get_str("preset").and_then(archetype_by_id);

                let seed_bytes = {
                    let mut b = [0u8; 32];
                    b[..8].copy_from_slice(&seed.to_le_bytes());
                    b
                };

                let strategy = preset
                    .map(|p| p.strategy.clone())
                    .unwrap_or(ChargingStrategy::Immediate { target_soc: 0.9 });
                let policy = preset
                    .map(|p| p.plug_in_policy.clone())
                    .unwrap_or(PlugInPolicy::Always);
                let event_day_ratio = config
                    .get_f64("event_day_ratio")
                    .or_else(|| preset.map(|p| p.event_day_ratio))
                    .unwrap_or(0.8);
                let miles_schedule = preset
                    .map(|p| p.build_miles_schedule(seed_bytes))
                    .unwrap_or_else(|| {
                        let mean = config.get_f64("daily_drive_miles_mean").unwrap_or(30.0);
                        ScheduleSource::Constant(mean)
                    });
                let departure_schedule = preset
                    .map(|p| p.build_departure_schedule(seed_bytes))
                    .unwrap_or(ScheduleSource::Constant(480.0));
                let duration_schedule = preset
                    .map(|p| p.build_duration_schedule(seed_bytes))
                    .unwrap_or(ScheduleSource::Constant(600.0));
                let arrival_schedule = preset.and_then(|p| p.build_arrival_schedule(seed_bytes));
                let fuel_economy = config.get_f64("fuel_economy_kwh_per_mi").unwrap_or(0.3);
                let capacity_kwh = config.get_f64("capacity_kwh").unwrap_or(60.0);
                let avg_speed = config.get_f64("average_speed_mph").unwrap_or(30.0);

                let max_charge_kw = config.get_f64("max_charge_kw").unwrap_or(7.2);
                let actor = EvDriverActor::new(
                    &config.name,
                    &target,
                    strategy,
                    policy,
                    miles_schedule,
                    departure_schedule,
                    duration_schedule,
                    arrival_schedule,
                    event_day_ratio,
                    fuel_economy,
                    capacity_kwh,
                    max_charge_kw,
                    avg_speed,
                    config.get_f64("range_anxiety_miles").unwrap_or(20.0),
                    config.get_f64("away_charge_fraction").unwrap_or(0.0),
                    config.get_f64("away_charge_power_kw").unwrap_or(6.6),
                    seed,
                );
                Ok(Box::new(actor))
            }),
        );

        registry.register(
            "DrCompliance",
            Box::new(|config: ActorConfig| {
                let mut actor = DrCompliance::new(&config.name);
                if let Some(rate) = config.get_f64("compliance_rate") {
                    actor = actor.with_compliance_model(Probabilistic {
                        compliance_rate: rate,
                        seed: config.get_f64("seed").map(|f| f as u64).unwrap_or(0),
                    });
                } else if config.get_bool("always_comply").unwrap_or(false) {
                    actor = actor.with_compliance_model(AlwaysComply);
                }
                if let Some(target_str) = config.get_str("hvac_target") {
                    let target = parse_dispatch_target(target_str)?;
                    actor = actor.with_hvac_target(target);
                }
                if let Some(action_str) = config.get_str("hvac_action") {
                    let action = parse_dr_action(action_str)?;
                    actor = actor.with_hvac_action(action);
                }
                if let Some(load_str) = config.get_str("load_targets") {
                    let targets = parse_load_targets(load_str)?;
                    for (target, action) in targets {
                        actor = actor.with_load_target(target, action);
                    }
                }
                #[cfg(any(debug_assertions, feature = "check_invariants"))]
                {
                    let has_hvac_target = config.get_str("hvac_target").is_some();
                    let has_hvac_action = config.get_str("hvac_action").is_some();
                    if has_hvac_target && !has_hvac_action {
                        tracing::debug!(
                            name = %config.name,
                            "DrCompliance actor has hvac_target but no hvac_action; \
                             dispatch will use DrAction::None (no signal emitted)"
                        );
                    }
                    if has_hvac_action && !has_hvac_target {
                        tracing::debug!(
                            name = %config.name,
                            "DrCompliance actor has hvac_action but no hvac_target; \
                             hvac_action will not dispatch"
                        );
                    }
                }
                Ok(Box::new(actor))
            }),
        );

        registry.register(
            "BatteryManagement",
            Box::new(|config: ActorConfig| {
                let target = config
                    .get_str("target")
                    .ok_or_else(|| {
                        HaresError::Control("BatteryManagement requires 'target' parameter".into())
                    })?
                    .to_string();
                let mode_str = config.get_str("mode").unwrap_or("self_consumption");
                let bms_mode = match mode_str {
                    "self_consumption" => BmsMode::SelfConsumption {
                        min_soc: config.get_f64("min_soc").unwrap_or(0.1),
                        max_soc: config.get_f64("max_soc").unwrap_or(1.0),
                        solar_only_charging: config
                            .get_bool("solar_only_charging")
                            .unwrap_or(false),
                        surplus_deadband_kw: config.get_f64("surplus_deadband_kw").unwrap_or(0.0),
                    },
                    "tou" | "time_of_use" => BmsMode::TimeOfUseOptimization {
                        reserve_soc: config.get_f64("reserve_soc").unwrap_or(0.2),
                        charge_threshold_percentile: config
                            .get_f64("charge_threshold_percentile")
                            .unwrap_or(0.25),
                        discharge_threshold_percentile: config
                            .get_f64("discharge_threshold_percentile")
                            .unwrap_or(0.75),
                        solar_only_charging: config
                            .get_bool("solar_only_charging")
                            .unwrap_or(false),
                        price_deadband: config.get_f64("price_deadband").unwrap_or(0.0),
                        min_duration_steps: config
                            .get_f64("min_duration_steps")
                            .map(|v| if v > 0.0 { Some(v as usize) } else { None })
                            .unwrap_or(None),
                    },
                    "backup" | "backup_reserve" => BmsMode::BackupReserve {
                        target_soc: config.get_f64("target_soc").unwrap_or(0.8),
                        charge_from_grid: config.get_bool("charge_from_grid").unwrap_or(true),
                        charge_rate_fraction: config.get_f64("charge_rate_fraction").unwrap_or(1.0),
                        soc_deadband: config.get_f64("soc_deadband").unwrap_or(0.0),
                    },
                    "manual" => BmsMode::Manual,
                    other => {
                        return Err(HaresError::Control(format!(
                            "Unknown BMS mode: {other:?}. Valid: \
                             self_consumption, tou/time_of_use, \
                             backup/backup_reserve, manual"
                        )));
                    }
                };
                let export_str = config.get_str("grid_export_rule").unwrap_or("unrestricted");
                let grid_export_rule = match export_str {
                    "unrestricted" => GridExportRule::Unrestricted,
                    "solar_only" => GridExportRule::SolarOnly,
                    "disabled" => GridExportRule::Disabled,
                    other => {
                        return Err(HaresError::Control(format!(
                            "Unknown grid export rule: {other:?}. \
                             Valid: unrestricted, solar_only, disabled"
                        )));
                    }
                };
                let max_charge_kw = config.get_f64("max_charge_kw").unwrap_or(5.0);
                let max_discharge_kw = config.get_f64("max_discharge_kw").unwrap_or(5.0);

                // steps_per_day must match the simulation timestep: 86400 / time_res_s.
                // The caller is expected to provide this; no sensible default exists.
                let steps_per_day = config
                    .get_f64("steps_per_day")
                    .map(|v| v as usize)
                    .ok_or_else(|| {
                        HaresError::Control(
                            "BatteryManagement requires 'steps_per_day' parameter \
                             (= 86400 / time_res_s)"
                                .into(),
                        )
                    })?;
                let min_dwell_steps = config
                    .get_f64("min_dwell_steps")
                    .map(|v| v as usize)
                    .unwrap_or(0);
                let actor = BatteryManagementActor::with_name(
                    &config.name,
                    &target,
                    bms_mode,
                    grid_export_rule,
                    max_charge_kw,
                    max_discharge_kw,
                    None,
                    steps_per_day,
                    min_dwell_steps,
                );
                Ok(Box::new(actor))
            }),
        );

        registry.register(
            "SafetyMonitor",
            Box::new(|config: ActorConfig| {
                let mut monitor = SafetyMonitor::new(&config.name);
                if let Some(t) = config.get_f64("freeze_threshold_c") {
                    monitor = monitor.with_freeze_protection_threshold(t);
                }
                if let Some(t) = config.get_f64("over_temp_threshold_c") {
                    monitor = monitor.with_over_temperature_threshold(t);
                }
                if let Some(target_str) = config.get_str("target") {
                    let end_use = match target_str {
                        "HVAC_HEATING" => EndUse::HVAC_HEATING,
                        "HVAC_COOLING" => EndUse::HVAC_COOLING,
                        "WATER_HEATING" => EndUse::WATER_HEATING,
                        other => {
                            return Err(HaresError::Control(format!(
                                "SafetyMonitor: unknown target '{other}'. \
                                 Valid: HVAC_HEATING, HVAC_COOLING, WATER_HEATING"
                            )));
                        }
                    };
                    monitor = monitor.with_target(DispatchTarget::ByEndUse(end_use));
                }
                Ok(Box::new(monitor))
            }),
        );
    }

    pub fn register(&mut self, actor_type: impl Into<String>, factory: ActorFactory) {
        self.factories.insert(actor_type.into(), factory);
    }

    #[must_use]
    pub fn get(&self, actor_type: &str) -> Option<&ActorFactory> {
        self.factories.get(actor_type)
    }

    pub fn create(&self, config: ActorConfig) -> Result<Box<dyn Actor>, HaresError> {
        match self.factories.get(&config.actor_type) {
            Some(factory) => factory(config),
            None => Err(HaresError::Control(format!(
                "unknown actor type: {}",
                config.actor_type
            ))),
        }
    }

    pub fn registered_types(&self) -> impl Iterator<Item = &String> {
        self.factories.keys()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actor_config_new_creates_empty_params() {
        let config = ActorConfig::new("TestActor", "IdealThermostat");
        assert_eq!(config.name, "TestActor");
        assert_eq!(config.actor_type, "IdealThermostat");
        assert!(config.parameters.is_empty());
    }

    #[test]
    fn actor_config_with_param_adds_parameter() {
        let config = ActorConfig::new("Test", "IdealThermostat")
            .with_param("heating_c", ConfigValue::Float(20.0));
        assert_eq!(config.get_f64("heating_c"), Some(20.0));
    }

    #[test]
    fn actor_registry_new_registers_builtins() {
        let registry = ActorRegistry::new();
        let types: Vec<_> = registry.registered_types().collect();
        assert!(types.contains(&&"IdealThermostat".to_string()));
        assert!(types.contains(&&"Occupant".to_string()));
        assert!(types.contains(&&"DrCompliance".to_string()));
        assert!(types.contains(&&"EvDriver".to_string()));
        assert!(types.contains(&&"BatteryManagement".to_string()));
        assert!(types.contains(&&"SafetyMonitor".to_string()));
    }

    #[test]
    fn actor_registry_create_ev_driver() {
        let registry = ActorRegistry::new();
        let config = ActorConfig::new("Driver1", "EvDriver")
            .with_param("target", ConfigValue::Text("EV1".into()))
            .with_param("seed", ConfigValue::Float(42.0));
        let actor = registry.create(config).expect("create ev driver");
        assert_eq!(actor.name(), "Driver1");
    }

    #[test]
    fn actor_registry_create_ev_driver_with_preset() {
        let registry = ActorRegistry::new();
        let config = ActorConfig::new("Driver2", "EvDriver")
            .with_param("target", ConfigValue::Text("EV1".into()))
            .with_param("seed", ConfigValue::Float(42.0))
            .with_param("preset", ConfigValue::Text("daily_commuter_l2".into()));
        let actor = registry
            .create(config)
            .expect("create ev driver with preset");
        assert_eq!(actor.name(), "Driver2");
    }

    #[test]
    fn actor_registry_create_ideal_thermostat() {
        let registry = ActorRegistry::new();
        let config = ActorConfig::new("Thermostat", "IdealThermostat")
            .with_param("target", ConfigValue::Text("MainHVAC".into()))
            .with_param("heating_c", ConfigValue::Float(20.0));

        let actor = registry.create(config).expect("create actor");
        assert_eq!(actor.name(), "Thermostat");
    }

    #[test]
    fn actor_registry_ideal_thermostat_hysteresis_from_config() {
        let registry = ActorRegistry::new();
        // With hysteresis_c=0.5, required gap is 1.0°C.
        // setpoints 20.0/21.0 have gap=1.0 which meets the threshold.
        let config = ActorConfig::new("Thermostat", "IdealThermostat")
            .with_param("target", ConfigValue::Text("MainHVAC".into()))
            .with_param("heating_c", ConfigValue::Float(20.0))
            .with_param("cooling_c", ConfigValue::Float(21.0))
            .with_param("hysteresis_c", ConfigValue::Float(0.5));
        let mut actor = registry.create(config).expect("create actor");
        let env = crate::actor::testing::test_env().build();
        let mut requests = Vec::new();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            actor.decide(&env, &mut requests);
        }));
        if result.is_ok() {
            assert_eq!(
                requests.len(),
                1,
                "gap=1.0 with hysteresis=0.5 should pass validation"
            );
        }
    }

    #[test]
    fn actor_registry_create_occupant() {
        let registry = ActorRegistry::new();
        let config = ActorConfig::new("Resident1", "Occupant");

        let actor = registry.create(config).expect("create actor");
        assert_eq!(actor.name(), "Resident1");
    }

    #[test]
    fn actor_registry_create_dr_compliance() {
        let registry = ActorRegistry::new();
        let config = ActorConfig::new("DR1", "DrCompliance")
            .with_param("always_comply", ConfigValue::Bool(true));

        let actor = registry.create(config).expect("create actor");
        assert_eq!(actor.name(), "DR1");
    }

    #[test]
    fn actor_registry_create_dr_compliance_with_targets_and_actions() {
        let registry = ActorRegistry::new();
        let config = ActorConfig::new("DR2", "DrCompliance")
            .with_param("always_comply", ConfigValue::Bool(true))
            .with_param("hvac_target", ConfigValue::Text("name:MainHVAC".into()))
            .with_param(
                "hvac_action",
                ConfigValue::Text("SetpointAdjust:2.0".into()),
            )
            .with_param(
                "load_targets",
                ConfigValue::Text(
                    "end_use:plug_loads:LoadCurtailment:0.5;name:EV:PowerLimit:5.0".into(),
                ),
            );

        let mut actor = registry
            .create(config)
            .expect("create dr compliance with targets");
        assert_eq!(actor.name(), "DR2");

        // Verify registry-constructed actor exposes telemetry and produces correct
        // initial state through the Actor trait interface. Full dispatch-path
        // verification requires set_dr_level() which is on DrCompliance, not Actor;
        // the direct-construction test below (dispatch_with_targets_and_dr_level)
        // covers the dispatch logic. See Known Limitations: T-1937 (ActorFactory
        // context) would enable dwelling-level validation of registry-constructed
        // actors end-to-end.
        let env = crate::actor::testing::test_env().build();
        let mut requests = Vec::new();
        actor.decide(&env, &mut requests);
        assert!(
            requests.is_empty(),
            "no signals expected at Normal DR level"
        );

        let telemetry = actor
            .telemetry()
            .expect("DrCompliance must expose telemetry");
        assert_eq!(
            telemetry.get("signals_count"),
            Some(0.0),
            "signals_count must be 0 after Normal DR decide with no prior dispatch state"
        );
        assert_eq!(
            telemetry.get("dr_level"),
            Some(0.0),
            "dr_level must be Normal (0.0) after construction"
        );
        assert_eq!(
            telemetry.get("dr_active"),
            Some(0.0),
            "dr_active must be 0 after construction"
        );
        assert_eq!(
            telemetry.get("targets_configured"),
            Some(3.0),
            "targets_configured must be 3 (1 HVAC + 2 load targets); \
             verifies that config parsing wired targets into the actor"
        );
    }

    #[test]
    fn actor_registry_create_dr_compliance_invalid_hvac_target_format() {
        let registry = ActorRegistry::new();
        let config = ActorConfig::new("DR3", "DrCompliance")
            .with_param("always_comply", ConfigValue::Bool(true))
            .with_param("hvac_target", ConfigValue::Text("bad_format".into()));

        let result = registry.create(config);
        assert!(
            result.is_err(),
            "registry creation should fail for invalid hvac_target format"
        );
        match &result {
            Err(HaresError::Control(msg)) => {
                assert!(
                    msg.contains("invalid dispatch target") || msg.contains("must start with"),
                    "error should describe format requirements, got: {msg}"
                );
            }
            _ => panic!("expected Control error"),
        }
    }

    #[test]
    fn actor_registry_create_dr_compliance_invalid_hvac_action_format() {
        let registry = ActorRegistry::new();
        let config = ActorConfig::new("DR4", "DrCompliance")
            .with_param("always_comply", ConfigValue::Bool(true))
            .with_param("hvac_target", ConfigValue::Text("name:HVAC".into()))
            .with_param("hvac_action", ConfigValue::Text("UnknownAction".into()));

        let result = registry.create(config);
        assert!(
            result.is_err(),
            "registry creation should fail for invalid hvac_action"
        );
        match &result {
            Err(HaresError::Control(msg)) => {
                assert!(
                    msg.contains("unknown DrAction") || msg.contains("valid actions"),
                    "error should list valid actions, got: {msg}"
                );
            }
            _ => panic!("expected Control error"),
        }
    }

    #[test]
    fn actor_registry_create_dr_compliance_invalid_load_target_format() {
        let registry = ActorRegistry::new();
        let config = ActorConfig::new("DR5", "DrCompliance")
            .with_param("always_comply", ConfigValue::Bool(true))
            .with_param(
                "load_targets",
                ConfigValue::Text("invalid:missing:parts".into()),
            );

        let result = registry.create(config);
        assert!(
            result.is_err(),
            "registry creation should fail for invalid load_targets prefix"
        );
        match &result {
            Err(HaresError::Control(msg)) => {
                assert!(
                    msg.contains("load target prefix must be 'name' or 'end_use'"),
                    "error should identify bad prefix, got: {msg}"
                );
            }
            _ => panic!("expected Control error"),
        }
    }

    #[test]
    fn actor_registry_create_dr_compliance_empty_hvac_target_name() {
        let registry = ActorRegistry::new();
        let config = ActorConfig::new("DR6", "DrCompliance")
            .with_param("always_comply", ConfigValue::Bool(true))
            .with_param("hvac_target", ConfigValue::Text("name:".into()));

        let result = registry.create(config);
        assert!(
            result.is_err(),
            "registry creation should fail for empty equipment name in hvac_target"
        );
        match &result {
            Err(HaresError::Control(msg)) => {
                assert!(
                    msg.contains("non-empty equipment name"),
                    "error should mention non-empty requirement, got: {msg}"
                );
            }
            _ => panic!("expected Control error"),
        }
    }

    #[test]
    fn actor_registry_create_dr_compliance_empty_hvac_target_end_use() {
        let registry = ActorRegistry::new();
        let config = ActorConfig::new("DR7", "DrCompliance")
            .with_param("always_comply", ConfigValue::Bool(true))
            .with_param("hvac_target", ConfigValue::Text("end_use:".into()));

        let result = registry.create(config);
        assert!(
            result.is_err(),
            "registry creation should fail for empty end_use in hvac_target"
        );
        match &result {
            Err(HaresError::Control(msg)) => {
                assert!(
                    msg.contains("non-empty end-use string"),
                    "error should mention non-empty requirement, got: {msg}"
                );
            }
            _ => panic!("expected Control error"),
        }
    }

    #[test]
    fn actor_registry_create_dr_compliance_dispatch_with_targets_and_dr_level() {
        use crate::actors::DrCompliance;
        use hares_types::DRLevel;

        let mut actor = DrCompliance::new("DR8")
            .with_compliance_model(AlwaysComply)
            .with_hvac_target(DispatchTarget::ByName(Arc::from("HVAC")))
            .with_hvac_action(DrAction::off())
            .with_load_target(
                DispatchTarget::ByEndUse(EndUse::PLUG_LOADS),
                DrAction::curtail(0.5),
            );

        actor.set_dr_level(DRLevel::Critical);

        let env = crate::actor::testing::test_env().build();
        let mut requests = Vec::new();
        actor.decide(&env, &mut requests);

        assert_eq!(
            requests.len(),
            2,
            "should dispatch 2 signals: 1 HVAC TurnOff + 1 plug load curtailment"
        );

        let has_hvac = requests.iter().any(|r| {
            matches!(&r.target, DispatchTarget::ByName(n) if &**n == "HVAC")
                && matches!(
                    &r.signal,
                    hares_types::ControlSignal::ModeOverride {
                        mode: hares_types::OperatingMode::Off
                    }
                )
        });
        assert!(has_hvac, "HVAC TurnOff not dispatched");

        let has_plug = requests.iter().any(|r| {
            matches!(&r.target, DispatchTarget::ByEndUse(eu) if *eu == EndUse::PLUG_LOADS)
                && matches!(
                    &r.signal,
                    hares_types::ControlSignal::LoadFraction { fraction: f }
                    if (f - 0.5).abs() < 0.01
                )
        });
        assert!(has_plug, "plug loads curtailment not dispatched");
    }

    #[test]
    fn actor_registry_create_unknown_type_returns_error() {
        let registry = ActorRegistry::new();
        let config = ActorConfig::new("Test", "UnknownType");

        let result = registry.create(config);
        assert!(result.is_err());
        match result {
            Err(HaresError::Control(msg)) => {
                assert!(msg.contains("unknown actor type"));
            }
            _ => panic!("expected Control error"),
        }
    }
    #[test]
    fn actor_registry_get_returns_factory() {
        let registry = ActorRegistry::new();
        assert!(registry.get("IdealThermostat").is_some());
        assert!(registry.get("UnknownType").is_none());
    }

    #[test]
    fn actor_registry_create_occupant_with_occupancy_column() {
        let registry = ActorRegistry::new();
        let occupancy_column = vec![0.0, 1.0, 0.0];
        let config = ActorConfig::new("Resident1", "Occupant")
            .with_param(
                "occupancy_column",
                ConfigValue::FloatArray(occupancy_column),
            )
            .with_param("lighting_target", ConfigValue::Text("Indoor Lights".into()));
        let mut actor = registry.create(config).expect("create occupant");
        assert_eq!(actor.name(), "Resident1");

        let env = crate::actor::testing::test_env().build();
        let mut requests = Vec::new();

        // Step 0: presence_schedule[0] = Away (threshold 0.0 → Away)
        // + lighting target with off_when_away → ModeOverride(Off) dispatched
        actor.decide(&env, &mut requests);
        assert!(!requests.is_empty(), "step 0 (Away) should emit signals");
        let has_off = requests.iter().any(|r| {
            matches!(
                r.signal,
                hares_types::ControlSignal::ModeOverride {
                    mode: hares_types::OperatingMode::Off
                }
            )
        });
        assert!(has_off, "step 0 (Away) must emit ModeOverride Off");

        // Step 1: presence_schedule[1] = Home (transition Away→Home)
        // Factory configures off_when_away only, not on_when_home, so no signal expected
        requests.clear();
        actor.decide(&env, &mut requests);
        assert!(
            requests.is_empty(),
            "step 1 (Home transition, on_when_home not configured) should emit no signals"
        );

        // Step 2: presence_schedule[2] = Away (transition Home→Away)
        // off_when_away triggers again
        requests.clear();
        actor.decide(&env, &mut requests);
        assert!(
            !requests.is_empty(),
            "step 2 (Away transition) should emit signals"
        );
        let has_off_again = requests.iter().any(|r| {
            matches!(
                r.signal,
                hares_types::ControlSignal::ModeOverride {
                    mode: hares_types::OperatingMode::Off
                }
            )
        });
        assert!(
            has_off_again,
            "step 2 (Away transition) must emit ModeOverride Off"
        );

        // Telemetry: presence_changes should be 3 (Home→Away, Away→Home, Home→Away)
        let telemetry = actor.telemetry().expect("actor must have telemetry");
        let changes = telemetry.get("presence_changes").unwrap_or(0.0);
        assert!(
            (changes - 3.0).abs() < 1e-9,
            "presence_changes must be 3.0 (three transitions); got {changes}"
        );
    }

    #[test]
    fn actor_registry_create_occupant_without_occupancy_column_falls_back_to_default() {
        let registry = ActorRegistry::new();
        let config = ActorConfig::new("Resident2", "Occupant");
        let actor = registry.create(config).expect("create occupant");
        assert_eq!(actor.name(), "Resident2");
    }

    #[test]
    fn actor_registry_occupant_empty_occupancy_column_all_away() {
        let registry = ActorRegistry::new();
        let occupancy_column = vec![0.0, 0.0, 0.0];
        let config = ActorConfig::new("Resident3", "Occupant")
            .with_param(
                "occupancy_column",
                ConfigValue::FloatArray(occupancy_column),
            )
            .with_param("lighting_target", ConfigValue::Text("Lights".into()));
        let mut actor = registry.create(config).expect("create occupant");
        let env = crate::actor::testing::test_env().build();
        let mut requests = Vec::new();

        // All away — each step should emit ModeOverride Off
        actor.decide(&env, &mut requests);
        assert_eq!(requests.len(), 1, "step 0 Away must emit 1 signal");

        requests.clear();
        actor.decide(&env, &mut requests);
        assert_eq!(requests.len(), 1, "step 1 Away must emit 1 signal");

        requests.clear();
        actor.decide(&env, &mut requests);
        assert_eq!(requests.len(), 1, "step 2 Away must emit 1 signal");

        let telemetry = actor.telemetry().expect("actor must have telemetry");
        let changes = telemetry.get("presence_changes").unwrap_or(0.0);
        assert!(
            (changes - 1.0).abs() < 1e-9,
            "presence_changes must be 1.0 (only default Home→Away transition); got {changes}"
        );
    }

    #[test]
    fn actor_registry_occupant_all_home_no_transitions() {
        let registry = ActorRegistry::new();
        let occupancy_column = vec![1.0, 1.0, 1.0];
        let config = ActorConfig::new("Resident4", "Occupant").with_param(
            "occupancy_column",
            ConfigValue::FloatArray(occupancy_column),
        );
        let mut actor = registry.create(config).expect("create occupant");
        let env = crate::actor::testing::test_env().build();
        let mut requests = Vec::new();

        actor.decide(&env, &mut requests);
        assert!(
            requests.is_empty(),
            "no signals expected when always home with no targets"
        );

        let telemetry = actor.telemetry().expect("actor must have telemetry");
        let changes = telemetry.get("presence_changes").unwrap_or(0.0);
        assert!(
            (changes - 0.0).abs() < 1e-9,
            "presence_changes must be 0.0 (no transitions); got {changes}"
        );
    }
}
