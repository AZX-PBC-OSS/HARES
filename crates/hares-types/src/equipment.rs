//! Equipment descriptor and identifier types.
//!
//! `EquipmentId`, `EquipmentDescriptor`, and capability flags that identify
//! equipment instances across crate boundaries.

use std::borrow::Cow;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{ControlCapabilities, ZoneId};

/// Stable equipment instance identifier.
#[derive(
    Hash, Eq, PartialEq, Copy, Clone, Debug, Default, Ord, PartialOrd, Serialize, Deserialize,
)]
pub struct EquipmentId(pub u32);

/// High-level end-use categories used for routing and reporting.
///
/// Extensible string-based type supporting both standard predefined categories
/// and custom user-defined end uses. Use the constants for standard types
/// (e.g., `EndUse::HVAC_HEATING`) or create custom ones with `EndUse::custom()`.
///
/// # Examples
///
/// ```
/// use hares_types::EndUse;
///
/// // Standard predefined end use
/// let heating = EndUse::HVAC_HEATING;
/// assert_eq!(heating.as_str(), "hvac_heating");
///
/// // Custom user-defined end use
/// let custom = EndUse::custom("heat_pump_water_heater");
/// assert_eq!(custom.as_str(), "heat_pump_water_heater");
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EndUse(Cow<'static, str>);

impl EndUse {
    /// Standard HVAC heating end use.
    pub const HVAC_HEATING: Self = Self::new("hvac_heating");
    /// Standard HVAC cooling end use.
    pub const HVAC_COOLING: Self = Self::new("hvac_cooling");
    /// Standard water heating end use.
    pub const WATER_HEATING: Self = Self::new("water_heating");
    /// Standard lighting end use.
    pub const LIGHTING: Self = Self::new("lighting");
    /// Standard plug loads end use.
    pub const PLUG_LOADS: Self = Self::new("plug_loads");
    /// Standard refrigeration end use.
    pub const REFRIGERATION: Self = Self::new("refrigeration");
    /// Standard ventilation end use.
    pub const VENTILATION: Self = Self::new("ventilation");
    /// Standard battery storage end use.
    pub const BATTERY: Self = Self::new("battery");
    /// Standard photovoltaic generation end use.
    pub const PV: Self = Self::new("pv");
    /// Standard electric vehicle end use.
    pub const EV: Self = Self::new("ev");
    /// Standard backup generator end use.
    pub const GENERATOR: Self = Self::new("generator");
    /// Standard dehumidification end use.
    pub const DEHUMIDIFIER: Self = Self::new("dehumidifier");
    /// Default fallback/other end use.
    pub const OTHER: Self = Self::new("other");

    /// Creates a new end use from a string.
    #[inline]
    pub const fn new(s: &'static str) -> Self {
        Self(Cow::Borrowed(s))
    }

    /// Creates a custom end use from any string.
    ///
    /// This allows equipment and actors to define their own end use categories
    /// without modifying the core library.
    ///
    /// # Examples
    ///
    /// ```
    /// use hares_types::EndUse;
    ///
    /// let hpwh = EndUse::custom("heat_pump_water_heater");
    /// let ice_storage = EndUse::custom("ice_storage");
    /// ```
    #[inline]
    pub fn custom<S: Into<Cow<'static, str>>>(s: S) -> Self {
        Self(s.into())
    }

    /// Returns the string representation of this end use.
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Checks if this is a standard predefined end use.
    pub fn is_standard(&self) -> bool {
        matches!(
            self.0.as_ref(),
            "hvac_heating"
                | "hvac_cooling"
                | "water_heating"
                | "lighting"
                | "plug_loads"
                | "refrigeration"
                | "ventilation"
                | "battery"
                | "pv"
                | "ev"
                | "generator"
                | "dehumidifier"
                | "other"
        )
    }
}

impl From<&'static str> for EndUse {
    fn from(s: &'static str) -> Self {
        Self::new(s)
    }
}

impl From<String> for EndUse {
    fn from(s: String) -> Self {
        Self::custom(s)
    }
}

/// Ideal capacity mode for HVAC equipment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IdealCapacityMode {
    /// Use ideal capacity when time_res >= 5 min or variable-speed equipment (OCHRE rule).
    #[default]
    Auto,
    /// Force ideal capacity mode regardless of timestep/speed.
    On,
    /// Force dynamic on/off thermostat cycling (duty_cycle = 1.0 when on).
    Off,
}

/// Fuel type consumed by an equipment device.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FuelType {
    Electric,
    Gas,
    Propane,
    Oil,
    #[default]
    None,
}

/// Fixed execution stage ordering for equipment updates.
///
/// Stages 1-3 are for equipment; stage 4 (EnvelopeResolution) runs after all
/// equipment and is used by domain solvers (thermal, humidity, electrical, fluid).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ExecutionStage {
    #[default]
    Independent,
    Electrical,
    Thermal,
    EnvelopeResolution,
}

/// Runtime operating mode reported by equipment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OperatingMode {
    #[default]
    Off,
    Heating,
    Cooling,
    Defrost,
    Standby,
    Charging,
    Discharging,
    HeatingHP,
    HeatingER,
    HeatingHPAndER,
    HeatPumpWH,
    BackupElement,
}

/// Custom domain identifier for extension points in the solver.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct DomainId(pub u16);

/// Identifier for a fluid loop.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct LoopId(pub u16);

/// Identifier for protocol-specific integrations.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct ProtocolId(pub u16);

/// Working fluid categories used by fluid ports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FluidType {
    Water,
    Glycol,
    Refrigerant,
}

/// Lookup table type for battery/EV LUT injection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BatteryLutType {
    ChargingCurve,
    Ocv,
    UNeg,
}

/// Describes one telemetry channel exposed by an equipment model.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TelemetryField {
    pub name: String,
    pub unit: String,
    pub description: String,
}

macro_rules! impl_id_type {
    ($name:ident, $inner:ty) => {
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl From<$inner> for $name {
            fn from(v: $inner) -> Self {
                Self(v)
            }
        }

        impl From<$name> for $inner {
            fn from(id: $name) -> Self {
                id.0
            }
        }
    };
}

impl_id_type!(EquipmentId, u32);
impl_id_type!(DomainId, u16);
impl_id_type!(LoopId, u16);
impl_id_type!(ProtocolId, u16);

/// Static metadata advertised by each equipment model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EquipmentDescriptor {
    pub id: EquipmentId,
    pub name: String,
    pub end_use: EndUse,
    /// Human-readable equipment type string.
    ///
    /// Use `Cow::Borrowed("Literal")` for compile-time constants (zero allocation).
    /// Use `Cow::Owned(string)` for runtime-constructed names (e.g., Python adapters
    /// or config-driven equipment).
    pub equipment_type: Cow<'static, str>,
    pub zone: Option<ZoneId>,
    pub fuel: FuelType,
    pub stage: ExecutionStage,
    pub control_capabilities: ControlCapabilities,
    pub telemetry_fields: Vec<TelemetryField>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ZoneId;

    #[test]
    fn equipment_descriptor_round_trips_through_json() {
        let descriptor = EquipmentDescriptor {
            id: EquipmentId(42),
            name: "Battery #1".to_string(),
            end_use: EndUse::OTHER,
            equipment_type: Cow::Borrowed("Battery"),
            zone: Some(ZoneId(1)),
            fuel: FuelType::Electric,
            stage: ExecutionStage::Electrical,
            control_capabilities: ControlCapabilities::POWER_SETPOINT
                | ControlCapabilities::THERMAL_SETPOINT,
            telemetry_fields: vec![TelemetryField {
                name: "electric_kw".to_string(),
                unit: "kW".to_string(),
                description: "Active electrical power".to_string(),
            }],
        };

        let json = serde_json::to_string(&descriptor).expect("serialize descriptor");
        let decoded: EquipmentDescriptor =
            serde_json::from_str(&json).expect("deserialize descriptor");
        assert_eq!(decoded, descriptor);
    }

    #[test]
    fn value_types_round_trip_through_json() {
        let id_json = serde_json::to_string(&EquipmentId(7)).expect("serialize equipment id");
        let id: EquipmentId = serde_json::from_str(&id_json).expect("deserialize equipment id");
        assert_eq!(id, EquipmentId(7));

        let domain_json = serde_json::to_string(&DomainId(1)).expect("serialize domain id");
        let domain: DomainId = serde_json::from_str(&domain_json).expect("deserialize domain id");
        assert_eq!(domain, DomainId(1));

        let loop_json = serde_json::to_string(&LoopId(2)).expect("serialize loop id");
        let loop_id: LoopId = serde_json::from_str(&loop_json).expect("deserialize loop id");
        assert_eq!(loop_id, LoopId(2));

        let protocol_json = serde_json::to_string(&ProtocolId(3)).expect("serialize protocol id");
        let protocol: ProtocolId =
            serde_json::from_str(&protocol_json).expect("deserialize protocol id");
        assert_eq!(protocol, ProtocolId(3));

        let end_use_json = serde_json::to_string(&EndUse::VENTILATION).expect("serialize end use");
        let end_use: EndUse = serde_json::from_str(&end_use_json).expect("deserialize end use");
        assert_eq!(end_use, EndUse::VENTILATION);

        let fuel_json = serde_json::to_string(&FuelType::None).expect("serialize fuel type");
        let fuel: FuelType = serde_json::from_str(&fuel_json).expect("deserialize fuel type");
        assert_eq!(fuel, FuelType::None);

        let stage_json =
            serde_json::to_string(&ExecutionStage::Thermal).expect("serialize execution stage");
        let stage: ExecutionStage =
            serde_json::from_str(&stage_json).expect("deserialize execution stage");
        assert_eq!(stage, ExecutionStage::Thermal);

        let mode_json = serde_json::to_string(&OperatingMode::Defrost).expect("serialize mode");
        let mode: OperatingMode = serde_json::from_str(&mode_json).expect("deserialize mode");
        assert_eq!(mode, OperatingMode::Defrost);

        let fluid_json = serde_json::to_string(&FluidType::Glycol).expect("serialize fluid type");
        let fluid: FluidType = serde_json::from_str(&fluid_json).expect("deserialize fluid type");
        assert_eq!(fluid, FluidType::Glycol);

        let lut_json =
            serde_json::to_string(&BatteryLutType::ChargingCurve).expect("serialize lut type");
        let lut: BatteryLutType = serde_json::from_str(&lut_json).expect("deserialize lut type");
        assert_eq!(lut, BatteryLutType::ChargingCurve);
    }

    #[test]
    fn new_operating_mode_variants_round_trip_through_json() {
        let modes = vec![
            OperatingMode::Charging,
            OperatingMode::Discharging,
            OperatingMode::HeatingHP,
            OperatingMode::HeatingER,
            OperatingMode::HeatingHPAndER,
            OperatingMode::HeatPumpWH,
            OperatingMode::BackupElement,
        ];
        for mode in modes {
            let json = serde_json::to_string(&mode).expect("serialize mode");
            let decoded: OperatingMode = serde_json::from_str(&json).expect("deserialize mode");
            assert_eq!(decoded, mode);
        }
    }

    #[test]
    fn equipment_descriptor_with_no_zone_round_trips() {
        let descriptor = EquipmentDescriptor {
            id: EquipmentId(1),
            name: "Test".to_string(),
            end_use: EndUse::OTHER,
            equipment_type: Cow::Borrowed("Generic"),
            zone: None,
            fuel: FuelType::None,
            stage: ExecutionStage::Independent,
            control_capabilities: ControlCapabilities::empty(),
            telemetry_fields: vec![],
        };
        let json = serde_json::to_string(&descriptor).expect("serialize");
        let decoded: EquipmentDescriptor = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, descriptor);
    }

    #[test]
    fn end_use_as_str_returns_stable_identifiers() {
        assert_eq!(EndUse::HVAC_HEATING.as_str(), "hvac_heating");
        assert_eq!(EndUse::HVAC_COOLING.as_str(), "hvac_cooling");
        assert_eq!(EndUse::WATER_HEATING.as_str(), "water_heating");
        assert_eq!(EndUse::LIGHTING.as_str(), "lighting");
        assert_eq!(EndUse::PLUG_LOADS.as_str(), "plug_loads");
        assert_eq!(EndUse::REFRIGERATION.as_str(), "refrigeration");
        assert_eq!(EndUse::VENTILATION.as_str(), "ventilation");
        assert_eq!(EndUse::BATTERY.as_str(), "battery");
        assert_eq!(EndUse::PV.as_str(), "pv");
        assert_eq!(EndUse::EV.as_str(), "ev");
        assert_eq!(EndUse::GENERATOR.as_str(), "generator");
        assert_eq!(EndUse::DEHUMIDIFIER.as_str(), "dehumidifier");
        assert_eq!(EndUse::OTHER.as_str(), "other");
    }

    #[test]
    fn custom_end_use_creation_and_comparison() {
        let custom = EndUse::custom("heat_pump_water_heater");
        assert_eq!(custom.as_str(), "heat_pump_water_heater");
        assert!(!custom.is_standard());

        // Two custom end uses with same string are equal
        let custom2 = EndUse::custom("heat_pump_water_heater");
        assert_eq!(custom, custom2);

        // Different custom end uses are not equal
        let different = EndUse::custom("ice_storage");
        assert_ne!(custom, different);
    }

    #[test]
    fn custom_end_use_round_trips_through_json() {
        let custom = EndUse::custom("vehicle_to_grid_charger");
        let json = serde_json::to_string(&custom).expect("serialize custom end use");
        let decoded: EndUse = serde_json::from_str(&json).expect("deserialize custom end use");
        assert_eq!(decoded, custom);
        assert_eq!(decoded.as_str(), "vehicle_to_grid_charger");
    }

    #[test]
    fn standard_end_use_is_standard_returns_true() {
        assert!(EndUse::HVAC_HEATING.is_standard());
        assert!(EndUse::BATTERY.is_standard());
        assert!(EndUse::OTHER.is_standard());
    }

    #[test]
    fn custom_end_use_is_standard_returns_false() {
        let custom = EndUse::custom("novel_equipment_type");
        assert!(!custom.is_standard());
    }

    #[test]
    fn custom_end_use_in_equipment_descriptor_round_trips() {
        let descriptor = EquipmentDescriptor {
            id: EquipmentId(42),
            name: "NovelDevice".to_string(),
            end_use: EndUse::custom("my_custom_category"),
            equipment_type: Cow::Borrowed("CustomType"),
            zone: Some(ZoneId(1)),
            fuel: FuelType::Electric,
            stage: ExecutionStage::Independent,
            control_capabilities: ControlCapabilities::POWER_SETPOINT,
            telemetry_fields: vec![],
        };

        let json =
            serde_json::to_string(&descriptor).expect("serialize descriptor with custom end use");
        let decoded: EquipmentDescriptor = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(decoded.end_use, EndUse::custom("my_custom_category"));
        assert!(!decoded.end_use.is_standard());
    }

    #[test]
    fn end_use_from_string_and_static_str() {
        let from_static: EndUse = "hvac_heating".into();
        assert_eq!(from_static, EndUse::HVAC_HEATING);

        let from_string: EndUse = "custom_from_string".to_string().into();
        assert_eq!(from_string.as_str(), "custom_from_string");
    }

    #[test]
    fn ideal_capacity_mode_default_is_auto() {
        let mode: IdealCapacityMode = Default::default();
        assert_eq!(mode, IdealCapacityMode::Auto);
    }

    #[test]
    fn ideal_capacity_mode_round_trips_through_json() {
        let modes = vec![
            IdealCapacityMode::Auto,
            IdealCapacityMode::On,
            IdealCapacityMode::Off,
        ];
        for mode in modes {
            let json = serde_json::to_string(&mode).expect("serialize mode");
            let decoded: IdealCapacityMode = serde_json::from_str(&json).expect("deserialize mode");
            assert_eq!(decoded, mode);
        }
    }
}
