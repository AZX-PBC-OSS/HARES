//! Air-source and minisplit heat-pump models.

mod constants;
mod cooler;
mod defrost;
mod heater;
mod heater_config;

pub use cooler::HpCooler;
pub use heater::{ASHPHeater, MinisplitHeater};

use crate::EquipmentRegistry;

pub fn register_with_registry(registry: &mut EquipmentRegistry) {
    registry.register(
        "Heat Pump Heater",
        Box::new(|config| Box::new(ASHPHeater::new(config))),
    );
    registry.register(
        "ASHP Heater",
        Box::new(|config| Box::new(ASHPHeater::new(config))),
    );
    registry.register(
        "MSHP Heater",
        Box::new(|config| Box::new(MinisplitHeater::new(config))),
    );
    registry.register(
        "ASHP Cooler",
        Box::new(|config| Box::new(HpCooler::ashp_cooler(config))),
    );
    registry.register(
        "MSHP Cooler",
        Box::new(|config| Box::new(HpCooler::mshp_cooler(config))),
    );
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use hares_types::ExecutionStage;

    use crate::{EquipmentConfig, EquipmentRegistry};

    fn config(name: &str, class: &str) -> EquipmentConfig {
        let mut raw = HashMap::new();
        raw.insert("zone_id".to_string(), 1.0.into());
        raw.insert("heating_setpoint_c".to_string(), 21.0.into());
        raw.insert("cooling_setpoint_c".to_string(), 26.0.into());
        EquipmentConfig::raw(name.to_string(), class.to_string(), raw)
    }

    #[test]
    fn registry_has_all_ochre_heat_pump_names() {
        let registry = EquipmentRegistry::new();

        for class in [
            "Heat Pump Heater",
            "ASHP Heater",
            "MSHP Heater",
            "ASHP Cooler",
            "MSHP Cooler",
        ] {
            let eq = registry.create(class, config(class, class)).unwrap();
            assert_eq!(eq.descriptor().stage, ExecutionStage::Thermal);
        }
    }
}
