//! Equipment registry for dynamic dispatch.

use std::collections::HashMap;

use hares_types::HaresError;

use crate::{Equipment, EquipmentConfig};

pub type EquipmentFactory =
    Box<dyn Fn(EquipmentConfig) -> Box<dyn Equipment> + Send + Sync + 'static>;

/// Registry mapping OCHRE class strings to equipment constructors.
///
/// Stage-assignment validation is caller responsibility in v1.
pub struct EquipmentRegistry {
    factories: HashMap<String, EquipmentFactory>,
    errors: HashMap<String, &'static str>,
}

impl EquipmentRegistry {
    #[must_use]
    pub fn new() -> Self {
        let mut registry = Self {
            factories: HashMap::new(),
            errors: HashMap::new(),
        };
        crate::scheduled_load::register_with_registry(&mut registry);
        crate::hvac::furnace::register_with_registry(&mut registry);
        crate::hvac::baseboard::register_with_registry(&mut registry);
        crate::hvac::boiler::register_with_registry(&mut registry);
        crate::hvac::air_conditioner::register_with_registry(&mut registry);
        crate::hvac::dehumidifier::register_with_registry(&mut registry);
        crate::hvac::heat_pump::register_with_registry(&mut registry);
        crate::hvac::ideal_hvac::register_with_registry(&mut registry);
        crate::water_heater::register_with_registry(&mut registry);
        crate::battery::register_with_registry(&mut registry);
        crate::pv::register_with_registry(&mut registry);
        crate::ev::register_with_registry(&mut registry);
        crate::generator::register_with_registry(&mut registry);
        crate::event_load::register_with_registry(&mut registry);
        crate::ventilation::register_with_registry(&mut registry);
        registry
    }

    pub fn register(&mut self, ochre_class: impl Into<String>, factory: EquipmentFactory) {
        self.factories.insert(ochre_class.into(), factory);
    }

    /// Register a class name that produces a descriptive error on construction.
    ///
    /// Use this for ambiguous resolver outputs that need user intervention
    /// (e.g. "Water Heating" without a fuel-type distinction).
    pub fn register_error(&mut self, ochre_class: impl Into<String>, message: &'static str) {
        self.errors.insert(ochre_class.into(), message);
    }

    #[must_use]
    pub fn get(&self, ochre_class: &str) -> Option<&EquipmentFactory> {
        self.factories.get(ochre_class)
    }

    pub fn create(
        &self,
        ochre_class: &str,
        config: EquipmentConfig,
    ) -> Result<Box<dyn Equipment>, HaresError> {
        if let Some(msg) = self.errors.get(ochre_class) {
            return Err(HaresError::Equipment((*msg).to_string()));
        }
        match self.factories.get(ochre_class) {
            Some(factory) => Ok(factory(config)),
            None => Err(HaresError::Equipment(format!(
                "unknown equipment class: {ochre_class}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_config(class: &str) -> EquipmentConfig {
        EquipmentConfig {
            name: class.to_string(),
            ochre_class: class.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn unknown_class_returns_err() {
        let registry = EquipmentRegistry::new();
        let result = registry.create("Nonexistent Widget", minimal_config("Nonexistent Widget"));
        let msg = match result {
            Err(e) => e.to_string(),
            Ok(_) => panic!("should fail for unknown class"),
        };
        assert!(
            msg.contains("unknown equipment class"),
            "expected 'unknown equipment class' in: {msg}"
        );
    }

    #[test]
    fn error_registered_class_returns_err() {
        let registry = EquipmentRegistry::new();
        let result = registry.create("Water Heating", minimal_config("Water Heating"));
        let msg = match result {
            Err(e) => e.to_string(),
            Ok(_) => panic!("should fail for ambiguous class"),
        };
        assert!(msg.contains("ambiguous"), "expected 'ambiguous' in: {msg}");
    }

    #[test]
    fn known_aliases_resolve() {
        let registry = EquipmentRegistry::new();
        for class in [
            "Gas Tankless Water Heater",
            "Generic Heater",
            "Generic Cooler",
        ] {
            assert!(
                registry.get(class).is_some(),
                "expected '{class}' to be registered"
            );
        }
    }
}
