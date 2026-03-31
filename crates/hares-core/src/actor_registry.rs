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

use hares_equipment::config::ConfigValue;
use hares_types::HaresError;

use hares_equipment::ev::catalog::archetype_by_id;
use hares_types::{BmsMode, ChargingStrategy, GridExportRule, PlugInPolicy, ScheduleSource};

use crate::Actor;
use crate::actors::{
    AlwaysComply, BatteryManagementActor, DrCompliance, EquipmentBehavior, EvDriverActor,
    IdealThermostat, Occupant, Probabilistic,
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
                    },
                    "backup" | "backup_reserve" => BmsMode::BackupReserve {
                        target_soc: config.get_f64("target_soc").unwrap_or(0.8),
                        charge_from_grid: config.get_bool("charge_from_grid").unwrap_or(true),
                        charge_rate_fraction: config.get_f64("charge_rate_fraction").unwrap_or(1.0),
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
                let actor = BatteryManagementActor::with_name(
                    &config.name,
                    &target,
                    bms_mode,
                    grid_export_rule,
                    max_charge_kw,
                    max_discharge_kw,
                    None,
                    steps_per_day,
                );
                Ok(Box::new(actor))
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
}
