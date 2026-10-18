//! Equipment registry for dynamic dispatch.

use std::collections::HashMap;
use std::sync::OnceLock;

use hares_types::{CoreCapabilities, EndUse, HaresError, TelemetryField};

use crate::water_heater::wh_config::TanklessWaterHeaterConfig;
use crate::{Equipment, EquipmentConfig};

pub type EquipmentFactory =
    Box<dyn Fn(EquipmentConfig) -> Box<dyn Equipment> + Send + Sync + 'static>;

/// The typed surface a registered class declares: its end use, its
/// core-output capabilities and its declared telemetry.
///
/// Read by callers that hold a class name but no constructed equipment
/// (the blueprint's end-use removal and the spec-shaped catalogue path);
/// the live path reads the same values off the constructed descriptor.
/// The capabilities are recorded at each class's default construction:
/// only the fuel-independent bits (`HAS_SOC`, the setpoint, capacity,
/// COP and speed flags) gate any column family, and the fuel bit itself
/// comes from the spec's own fuel where it is consumed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassDeclaredSurface {
    /// The class's declared end use.
    pub end_use: EndUse,
    /// The class's declared core-output capabilities.
    pub core_capabilities: CoreCapabilities,
    /// The class's declared telemetry fields (the heat-pump-heater
    /// column family's gate is the pan heater power declaration).
    pub telemetry: Vec<TelemetryField>,
}

/// The registered class's declared surface, read from the class's own
/// constructor at its default configuration: the end use and the
/// core-output capabilities the constructed descriptor carries. `None`
/// for a name that is not a registered class.
///
/// There is no second table to drift: the declaration is the
/// constructor's, and the surface is read from it (once per process,
/// then cached). The two tankless classes require a typed fuel to
/// construct and are read at the gas configuration their canonical
/// resolvers write.
pub fn class_declared_surface(class: &str) -> Option<ClassDeclaredSurface> {
    static SURFACES: OnceLock<HashMap<String, ClassDeclaredSurface>> = OnceLock::new();
    let surfaces = SURFACES.get_or_init(|| {
        let registry = crate::EquipmentRegistry::new();
        registry
            .known_names()
            .into_iter()
            .filter_map(|class| {
                let config = default_construction_config(class);
                // An unconstructable class (the typed-error
                // registrations) declares no surface.
                registry.create(class, config).ok().map(|eq| {
                    let d = eq.descriptor();
                    (
                        class.to_string(),
                        ClassDeclaredSurface {
                            end_use: d.end_use.clone(),
                            core_capabilities: d.core_capabilities,
                            telemetry: d.telemetry_fields.clone(),
                        },
                    )
                })
            })
            .collect()
    });
    surfaces.get(class).cloned()
}

/// The default configuration a class constructs from: the raw default
/// payload, except the two tankless classes, whose constructor requires
/// a typed fuel (their canonical resolvers write gas).
fn default_construction_config(class: &str) -> EquipmentConfig {
    use crate::water_heater::wh_config::TanklessWaterHeaterConfig;
    match class {
        "Tankless Water Heater" | "Gas Tankless Water Heater" => {
            let mut config = EquipmentConfig::from_typed(
                class.to_string(),
                "Tankless Water Heater".to_string(),
                TanklessWaterHeaterConfig {
                    fuel_type: hares_types::FuelType::Gas,
                    ..tankless_field_defaults()
                },
            )
            .expect("the tankless default config converts");
            config.ochre_class = class.to_string();
            config
        }
        _ => EquipmentConfig::with_payload(
            class.to_string(),
            class.to_string(),
            crate::ConfigPayload::default(),
        ),
    }
}

/// The tankless config's fields are all `Option`s but the struct has no
/// `Default` (its fuel is required); this is the all-`None` form whose
/// quantities the constructor resolves.
fn tankless_field_defaults() -> TanklessWaterHeaterConfig {
    TanklessWaterHeaterConfig {
        equipment_id: None,
        zone_id: None,
        loop_id: None,
        fuel_type: hares_types::FuelType::Gas,
        energy_factor: None,
        uniform_energy_factor: None,
        heating_capacity_w: None,
        setpoint_c: None,
        parasitic_power_w: None,
        performance_adjustment: None,
        inlet_temp_c: None,
        draw_flow_rate_kg_s: None,
        draw_flow_rate_source: None,
        mains_temp_c_source: None,
        appliance_draw_flow_rate_source: None,
        avg_water_draw_l_per_day: None,
        min_flow_gpm: None,
        min_flow_kg_s: None,
        zone_type: None,
    }
}

/// Every built-in equipment type name registered by `EquipmentRegistry::new`.
///
/// This is the single source of truth for exhaustive coverage tests and for
/// tooling that needs to enumerate supported types without constructing a full
/// registry.
pub const CANONICAL_EQUIPMENT_NAMES: &[&str] = &[
    // HVAC – heating
    "Gas Furnace",
    "Electric Furnace",
    "Electric Baseboard",
    "Gas Boiler",
    "Electric Boiler",
    // HVAC – cooling
    "Air Conditioner",
    "Room AC",
    "Dehumidifier",
    // HVAC – heat pumps
    "Heat Pump Heater",
    "ASHP Heater",
    "MSHP Heater",
    "GSHP Heater",
    "WSHP Heater",
    "ASHP Cooler",
    "MSHP Cooler",
    "GSHP Cooler",
    "WSHP Cooler",
    // HVAC – ideal
    "Ideal HVAC",
    // Water heaters
    "Gas Water Heater",
    "Resistance Water Heater",
    "Electric Resistance Water Heater",
    "Tankless Water Heater",
    "Gas Tankless Water Heater",
    "Heat Pump Water Heater",
    "HPWH",
    "Indirect Tank",
    // Storage
    "Battery",
    // PV
    "PV",
    // EV
    "EV",
    "Electric Vehicle",
    "Scheduled EV",
    // Generators
    "Gas Generator",
    "Gas Fuel Cell",
    // Scheduled / event-based loads
    "Lighting",
    "Plug Loads",
    "Other",
    "Refrigerator",
    "Freezer",
    "MELs",
    "TV",
    "Well Pump",
    "Pool Pump",
    "Pool Heater",
    "Spa Pump",
    "Spa Heater",
    "Gas Grill",
    "Gas Fireplace",
    "Gas Lighting",
    "Ceiling Fan",
    "Ventilation Fan",
    "Indoor Lighting",
    "Exterior Lighting",
    "Basement Lighting",
    "Garage Lighting",
    // Event-based loads
    "EventBasedLoad",
    "Clothes Washer",
    "Dishwasher",
    "Clothes Dryer",
    "Cooking Range",
    "Microwave",
    // Ventilation
    "HRV",
    "ERV",
    // Protocol bridge
    "Protocol Bridge",
];

/// Registry mapping OCHRE class strings to equipment constructors.
///
/// Stage-assignment validation is caller responsibility in v1.
pub struct EquipmentRegistry {
    factories: HashMap<String, EquipmentFactory>,
    errors: HashMap<String, &'static str>,
}

impl Default for EquipmentRegistry {
    fn default() -> Self {
        Self::new()
    }
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
        crate::protocol_bridge::register_with_registry(&mut registry);
        registry
    }

    pub fn register(&mut self, ochre_class: impl Into<String>, factory: EquipmentFactory) {
        let key = ochre_class.into();
        #[cfg(debug_assertions)]
        assert!(
            !self.factories.contains_key(&key),
            "duplicate registration for {key}"
        );
        self.factories.insert(key, factory);
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

    /// Returns all registered equipment class names in sorted order.
    #[must_use]
    pub fn known_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.factories.keys().map(String::as_str).collect();
        names.sort_unstable();
        names
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
        EquipmentConfig::with_payload(
            class.to_string(),
            class.to_string(),
            crate::config::ConfigPayload::default(),
        )
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
        let class = "Gas Tankless Water Heater";
        assert!(
            registry.get(class).is_some(),
            "expected '{class}' to be registered"
        );
    }

    #[test]
    fn all_built_in_equipment_types_are_registered() {
        let registry = EquipmentRegistry::new();
        for &name in super::CANONICAL_EQUIPMENT_NAMES {
            assert!(
                registry.get(name).is_some(),
                "CANONICAL_EQUIPMENT_NAMES entry '{name}' is not registered in EquipmentRegistry::new()"
            );
        }
    }

    #[test]
    fn all_registered_names_are_in_canonical_list() {
        let registry = EquipmentRegistry::new();
        let canonical: std::collections::HashSet<&str> =
            super::CANONICAL_EQUIPMENT_NAMES.iter().copied().collect();
        for name in registry.known_names() {
            assert!(
                canonical.contains(name),
                "registered name '{name}' is missing from CANONICAL_EQUIPMENT_NAMES"
            );
        }
    }

    #[test]
    fn known_names_returns_sorted_names() {
        let registry = EquipmentRegistry::new();
        let names = registry.known_names();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(
            names, sorted,
            "known_names() must return names in sorted order"
        );
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "duplicate registration")]
    fn duplicate_registration_panics_in_debug() {
        let mut registry = EquipmentRegistry::new();
        registry.register(
            "Test Duplicate Class",
            Box::new(|cfg| Box::new(crate::ev::Ev::new(cfg))),
        );
        registry.register(
            "Test Duplicate Class",
            Box::new(|cfg| Box::new(crate::ev::Ev::new(cfg))),
        );
    }

    #[test]
    fn all_canonical_resolver_names_are_registered() {
        let registry = EquipmentRegistry::new();
        let resolver_names = [
            "Gas Furnace",
            "Electric Furnace",
            "Gas Boiler",
            "Electric Boiler",
            "Electric Baseboard",
            "Ideal HVAC",
            "Air Conditioner",
            "Room AC",
            "ASHP Heater",
            "ASHP Cooler",
            "MSHP Heater",
            "MSHP Cooler",
            "Dehumidifier",
            "Gas Water Heater",
            "Electric Resistance Water Heater",
            "Tankless Water Heater",
            "Gas Tankless Water Heater",
            "Heat Pump Water Heater",
            "Battery",
            "Electric Vehicle",
            "PV",
            "Gas Generator",
            "Ventilation Fan",
        ];
        for name in resolver_names {
            assert!(
                registry.get(name).is_some(),
                "resolver output '{name}' is not registered in EquipmentRegistry::new()"
            );
        }
    }

    #[test]
    fn gas_tankless_water_heater_init_sets_gas_fuel_type() {
        use crate::config::EquipmentTypedConfig;
        use crate::water_heater::wh_config::TanklessWaterHeaterConfig;
        use chrono::{FixedOffset, TimeZone};
        use hares_types::{EnvironmentState, FuelType, GridState, WeatherState, ZoneId, ZoneState};

        let cfg = TanklessWaterHeaterConfig {
            equipment_id: None,
            zone_id: Some(1),
            loop_id: None,
            fuel_type: FuelType::Gas,
            energy_factor: Some(0.82),
            uniform_energy_factor: None,
            heating_capacity_w: Some(20_000.0),
            setpoint_c: Some(51.67),
            performance_adjustment: Some(0.92),
            parasitic_power_w: Some(5.0),
            inlet_temp_c: None,
            draw_flow_rate_kg_s: None,
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
            appliance_draw_flow_rate_source: None,
            avg_water_draw_l_per_day: None,
            zone_type: None,
            min_flow_kg_s: None,
            min_flow_gpm: None,
        };
        let ec = crate::config::EquipmentConfig::from_typed(
            "Gas Tankless Water Heater".to_string(),
            TanklessWaterHeaterConfig::equipment_type_name().to_string(),
            cfg,
        )
        .unwrap();
        let registry = EquipmentRegistry::new();
        let mut eq = registry
            .create("Gas Tankless Water Heater", ec.clone())
            .expect("registry must create Gas Tankless Water Heater");
        let env = EnvironmentState {
            ambient_other_space_c: hares_types::AmbientOtherSpaceTemps::default(),
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: 20.0,
                humidity_ratio: 0.008,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: 10.0,
                outdoor_humidity_ratio: 0.003,
                pressure_kpa: 101.325,
                ..WeatherState::default()
            },
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
                island_bus_voltage_pu: None,
            },
            schedule_row: None,
            domains: hares_types::DomainSlots::default(),
            equipment_telemetry: std::collections::HashMap::new(),
            equipment_core: std::collections::HashMap::new(),
            current_time: FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
                .single()
                .expect("valid"),
            time_res: chrono::Duration::minutes(1),
            price_signal: Default::default(),
            electrical: Default::default(),
        };
        eq.init(&ec, &env).unwrap();
        assert_eq!(
            eq.descriptor().fuel,
            hares_types::FuelType::Gas,
            "fuel_type must be Gas after typed init with fuel_type='Gas'"
        );
    }
}

/// The declared surface is the constructor's declaration: for every
/// registered class, `class_declared_surface` reads the same end use,
/// core capabilities and telemetry the constructed descriptor carries.
/// There is no second table to drift.
#[test]
fn class_declared_surface_matches_the_constructed_descriptors() {
    let registry = EquipmentRegistry::new();
    for class in registry.known_names() {
        let surface = class_declared_surface(class)
            .unwrap_or_else(|| panic!("'{class}' declares no surface"));
        let config = default_construction_config(class);
        let eq = registry
            .create(class, config)
            .unwrap_or_else(|e| panic!("create failed for '{class}': {e}"));
        let d = eq.descriptor();
        assert_eq!(
            surface.end_use, d.end_use,
            "'{class}' declared end use must match the constructed descriptor"
        );
        assert_eq!(
            surface.core_capabilities, d.core_capabilities,
            "'{class}' declared capabilities must match the constructed descriptor"
        );
        assert_eq!(
            surface.telemetry, d.telemetry_fields,
            "'{class}' declared telemetry must match the constructed descriptor"
        );
    }
    // An unknown class declares nothing.
    assert_eq!(class_declared_surface("Television"), None);
    assert_eq!(class_declared_surface("Water Heating"), None);
}

/// The config type a class's overrides are checked against (the
/// removed-keys row's prefix): the same class-to-config mapping the
/// registry's factories implement. `None` for a class whose config type
/// declares no removed-key row, so its overrides carry none either.
#[must_use]
pub fn config_type_for_class(class: &str) -> Option<&'static str> {
    match class {
        "PV" => Some("PvConfig"),
        "Battery" => Some("BatteryConfig"),
        "EV" | "Electric Vehicle" | "Scheduled EV" => Some("EvConfig"),
        "Gas Generator" | "Gas Fuel Cell" => Some("GeneratorConfig"),
        "Air Conditioner" => Some("CentralAirConditionerConfig"),
        "Room AC" => Some("RoomAcConfig"),
        "ASHP Cooler" | "MSHP Cooler" | "GSHP Cooler" | "WSHP Cooler" => {
            Some("HeatPumpCoolerConfig")
        }
        "EventBasedLoad" | "Cooking Range" | "Microwave" => Some("EventBasedLoad"),
        "Clothes Washer" | "Dishwasher" | "Clothes Dryer" => Some("WetAppliance"),
        "Lighting" | "Plug Loads" | "Other" | "Refrigerator" | "Freezer" | "MELs" | "TV"
        | "Well Pump" | "Pool Pump" | "Pool Heater" | "Spa Pump" | "Spa Heater" | "Gas Grill"
        | "Gas Fireplace" | "Gas Lighting" | "Ceiling Fan" | "Indoor Lighting"
        | "Exterior Lighting" | "Basement Lighting" | "Garage Lighting" => Some("ScheduledLoad"),
        _ => None,
    }
}
