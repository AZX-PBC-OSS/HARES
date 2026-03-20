//! Equipment registry for dynamic dispatch.

use std::collections::HashMap;

use hares_types::HaresError;

use crate::{Equipment, EquipmentConfig};

pub type EquipmentFactory =
    Box<dyn Fn(EquipmentConfig) -> Box<dyn Equipment> + Send + Sync + 'static>;

/// Registry mapping OCHRE class strings to equipment constructors.
///
/// Stage-assignment validation is caller responsibility in v1.
#[derive(Default)]
pub struct EquipmentRegistry {
    factories: HashMap<String, EquipmentFactory>,
}

impl EquipmentRegistry {
    #[must_use]
    pub fn new() -> Self {
        let mut registry = Self {
            factories: HashMap::new(),
        };
        crate::scheduled_load::register_with_registry(&mut registry);
        crate::hvac::furnace::register_with_registry(&mut registry);
        crate::hvac::baseboard::register_with_registry(&mut registry);
        crate::hvac::boiler::register_with_registry(&mut registry);
        crate::hvac::air_conditioner::register_with_registry(&mut registry);
        crate::hvac::dehumidifier::register_with_registry(&mut registry);
        crate::hvac::heat_pump::register_with_registry(&mut registry);
        crate::water_heater::register_with_registry(&mut registry);
        crate::battery::register_with_registry(&mut registry);
        crate::pv::register_with_registry(&mut registry);
        crate::ev::register_with_registry(&mut registry);
        crate::generator::register_with_registry(&mut registry);
        crate::event_load::register_with_registry(&mut registry);
        registry
    }

    pub fn register(&mut self, ochre_class: impl Into<String>, factory: EquipmentFactory) {
        self.factories.insert(ochre_class.into(), factory);
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
        match self.factories.get(ochre_class) {
            Some(factory) => Ok(factory(config)),
            None => Err(HaresError::Equipment(format!(
                "unknown equipment class: {ochre_class}"
            ))),
        }
    }
}
