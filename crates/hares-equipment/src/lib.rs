//! Equipment trait, registry, and built-in equipment modules.

#[macro_use]
mod macros;

pub mod battery;
pub mod config;
pub mod ev;
pub mod event_load;
pub mod generator;
pub mod hvac;
pub mod pv;
pub mod registry;
pub(crate) mod schedule_helpers;
pub mod scheduled_load;
pub mod water_heater;

use std::time::Duration;

use hares_types::{
    ControlSignal, EnvironmentState, EquipmentDescriptor, HaresError, OperatingMode,
    PortDeclaration, PortSlots, ensure_signal_supported,
};
use serde::{Serialize, de::DeserializeOwned};

pub use config::EquipmentConfig;
pub use hares_types::Telemetry;
pub use water_heater::DHW_DEMAND_LOOP;
pub use hvac::{
    EquivalentBatteryModel, HvacEquipment, HvacEquipmentType, RuntimeSetpointOverride,
};
pub use registry::{EquipmentFactory, EquipmentRegistry};

/// Equipment-layer result type.
pub type Result<T> = std::result::Result<T, HaresError>;

/// Common interface implemented by all equipment models.
pub trait Equipment: Send + Sync {
    fn descriptor(&self) -> &EquipmentDescriptor;
    fn ports(&self) -> &[PortDeclaration];
    fn init(&mut self, config: &EquipmentConfig, env: &EnvironmentState) -> Result<()>;
    fn update_control(&mut self, env: &EnvironmentState) -> OperatingMode;
    fn step(
        &mut self,
        env: &EnvironmentState,
        dt: Duration,
        ports: &mut PortSlots,
    ) -> std::result::Result<(), HaresError>;
    fn telemetry(&self) -> &Telemetry;
    fn save_state(&self) -> Vec<u8>;
    fn load_state(&mut self, state: &[u8]) -> Result<()>;

    /// Equipment-specific control application (no capability check).
    ///
    /// Implementors: override this method with equipment-specific logic.
    /// Do NOT override `apply_control` — it provides the capability gate and
    /// delegates to this method after validation.
    fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> Result<()>;

    /// Capability-gated control dispatch boundary.
    fn apply_control(&mut self, signal: &ControlSignal) -> Result<()> {
        ensure_signal_supported(self.descriptor().control_capabilities, signal)?;
        self.apply_control_unchecked(signal)
    }
}

/// Serialize checkpoint state via postcard.
///
/// # Safety (panic-freedom)
///
/// `postcard::to_allocvec` only fails for types that use unsupported serde
/// features (e.g. `i128`, nested enums with data in certain configurations).
/// All HARES state types derive `Serialize` with postcard-compatible field
/// types (primitives, `Vec`, `Option`, flat enums), so this path is infallible
/// in practice.  The `#[cold]` hint marks the panic branch as unlikely so the
/// compiler can optimise for the success path.
#[must_use]
pub fn save_postcard<T: Serialize>(state: &T) -> Vec<u8> {
    match postcard::to_allocvec(state) {
        Ok(bytes) => bytes,
        Err(e) => panic_serialize(e),
    }
}

#[cold]
#[inline(never)]
fn panic_serialize(e: postcard::Error) -> ! {
    panic!("equipment state serialization failed: {e}")
}

/// Deserialize checkpoint state via postcard.
pub fn load_postcard<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    postcard::from_bytes(bytes)
        .map_err(|e| HaresError::Equipment(format!("equipment state deserialization failed: {e}")))
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::time::Duration;

    use hares_types::{
        ControlCapabilities, ControlSignal, EndUse, EnvironmentState, EquipmentDescriptor,
        EquipmentId, ExecutionStage, FluidType, FuelType, GridState, LoopId, OperatingMode,
        PortDeclaration, PortSlots, PortType, ProtocolId, SurfaceIrradiance, Telemetry,
        TelemetryField, WeatherState, ZoneId, ZoneState,
    };
    use serde::{Deserialize, Serialize};

    use crate::{Equipment, EquipmentConfig, EquipmentRegistry, load_postcard, save_postcard};

    #[derive(Clone)]
    struct MockEquipment {
        descriptor: EquipmentDescriptor,
        ports: Vec<PortDeclaration>,
        telemetry: Telemetry,
        mode: OperatingMode,
        state_value: f64,
    }

    impl MockEquipment {
        fn new(control_capabilities: ControlCapabilities) -> Self {
            Self {
                descriptor: EquipmentDescriptor {
                    id: EquipmentId(1),
                    name: "Mock".to_string(),
                    end_use: EndUse::Other,
                    equipment_type: Cow::Borrowed("Mock"),
                    zone: Some(ZoneId(1)),
                    fuel: FuelType::Electric,
                    stage: ExecutionStage::Independent,
                    control_capabilities,
                    telemetry_fields: vec![TelemetryField {
                        name: "x".to_string(),
                        unit: "-".to_string(),
                        description: "mock value".to_string(),
                    }],
                },
                ports: vec![PortDeclaration {
                    port_type: PortType::Electrical,
                    zone: None,
                    loop_id: None,
                    domain_id: None,
                    fluid_type: None,
                }],
                telemetry: Telemetry::with_capacity(2),
                mode: OperatingMode::Off,
                state_value: 0.0,
            }
        }
    }

    impl Equipment for MockEquipment {
        fn descriptor(&self) -> &EquipmentDescriptor {
            &self.descriptor
        }

        fn ports(&self) -> &[PortDeclaration] {
            &self.ports
        }

        fn init(
            &mut self,
            _config: &EquipmentConfig,
            _env: &EnvironmentState,
        ) -> crate::Result<()> {
            self.telemetry.insert("x", self.state_value);
            Ok(())
        }

        fn update_control(&mut self, _env: &EnvironmentState) -> OperatingMode {
            self.mode
        }

        fn step(
            &mut self,
            _env: &EnvironmentState,
            _dt: Duration,
            _ports: &mut PortSlots,
        ) -> std::result::Result<(), hares_types::HaresError> {
            Ok(())
        }

        fn telemetry(&self) -> &Telemetry {
            &self.telemetry
        }

        fn save_state(&self) -> Vec<u8> {
            #[derive(Serialize)]
            struct State {
                state_value: f64,
            }
            save_postcard(&State {
                state_value: self.state_value,
            })
        }

        fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
            #[derive(Deserialize)]
            struct State {
                state_value: f64,
            }
            let decoded: State = load_postcard(state)?;
            self.state_value = decoded.state_value;
            Ok(())
        }

        fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
            if let ControlSignal::PowerSetpoint {
                active_power_kw, ..
            } = signal
            {
                self.state_value = *active_power_kw;
                self.telemetry.insert("x", self.state_value);
            }
            Ok(())
        }
    }

    fn sample_env() -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: 21.0,
                humidity_ratio: 0.008,
                relative_humidity: 0.45,
                wet_bulb_c: 14.0,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: 10.0,
                outdoor_humidity_ratio: 0.005,
                wind_speed_m_s: 2.0,
                wind_dir_deg: 0.0,
                ground_temp_c: 12.0,
                sky_temp_c: 8.0,
                pressure_kpa: 101.325,
                solar_irradiance: vec![SurfaceIrradiance {
                    surface_id: 1,
                    direct_w_m2: 0.0,
                    diffuse_w_m2: 0.0,
                    reflected_w_m2: 0.0,
                    angle_of_incidence_rad: 0.0,
                }],
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
            current_time: chrono::Utc::now(),
            time_res: chrono::Duration::seconds(60),
        }
    }

    #[test]
    fn dyn_equipment_is_object_safe() {
        let e: Box<dyn Equipment> =
            Box::new(MockEquipment::new(ControlCapabilities::POWER_SETPOINT));
        assert_eq!(e.descriptor().name, "Mock");
    }

    #[test]
    fn apply_control_rejects_unsupported_signals() {
        let mut eq = MockEquipment::new(ControlCapabilities::MODE_OVERRIDE);
        let signal = ControlSignal::PowerSetpoint {
            active_power_kw: 3.0,
            reactive_power_kvar: None,
        };
        let err = eq.apply_control(&signal).unwrap_err();
        assert!(err.to_string().contains("unsupported control signal"));
    }

    #[test]
    fn telemetry_returns_reference_and_explicit_clone_is_independent() {
        let mut eq = MockEquipment::new(ControlCapabilities::POWER_SETPOINT);
        eq.init(
            &EquipmentConfig {
                name: "Mock".to_string(),
                ochre_class: "Mock".to_string(),
                raw_config: Default::default(),
            },
            &sample_env(),
        )
        .unwrap();
        // Reference reflects the live state.
        assert_eq!(eq.telemetry().get("x"), Some(0.0));
        // Explicit clone is independent; mutations don't affect the original.
        let mut t1 = eq.telemetry().clone();
        t1.insert("x", 99.0);
        assert_ne!(t1.get("x"), eq.telemetry().get("x"));
    }

    #[test]
    fn registry_create_unknown_returns_err() {
        let registry = EquipmentRegistry::new();
        let result = registry.create(
            "DoesNotExist",
            EquipmentConfig {
                name: "x".to_string(),
                ochre_class: "x".to_string(),
                raw_config: Default::default(),
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn registry_factory_is_send_sync_and_constructs_uninitialized_equipment() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<crate::EquipmentFactory>();

        let mut registry = EquipmentRegistry::new();
        registry.register(
            "Mock",
            Box::new(|_cfg| Box::new(MockEquipment::new(ControlCapabilities::POWER_SETPOINT))),
        );

        let equipment = registry
            .create(
                "Mock",
                EquipmentConfig {
                    name: "x".to_string(),
                    ochre_class: "Mock".to_string(),
                    raw_config: Default::default(),
                },
            )
            .unwrap();
        assert_eq!(equipment.descriptor().equipment_type, "Mock");
    }

    #[test]
    fn postcard_helpers_round_trip() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct S {
            a: u32,
            b: f64,
            c: ProtocolId,
            d: LoopId,
            e: FluidType,
        }
        let s = S {
            a: 3,
            b: 4.2,
            c: ProtocolId(7),
            d: LoopId(9),
            e: FluidType::Water,
        };
        let bytes = save_postcard(&s);
        let decoded: S = load_postcard(&bytes).unwrap();
        assert_eq!(decoded, s);
    }
}
