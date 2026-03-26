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

use crate::Actor;
use crate::actors::{
    AlwaysComply, DrCompliance, EquipmentBehavior, IdealThermostat, Occupant, Probabilistic,
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
                let mut actor = IdealThermostat::new(target);
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
    }

    #[test]
    fn actor_registry_create_ideal_thermostat() {
        let registry = ActorRegistry::new();
        let config = ActorConfig::new("Thermostat", "IdealThermostat")
            .with_param("target", ConfigValue::Text("MainHVAC".into()))
            .with_param("heating_c", ConfigValue::Float(20.0));

        let actor = registry.create(config).expect("create actor");
        assert_eq!(actor.name(), "IdealThermostat");
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
