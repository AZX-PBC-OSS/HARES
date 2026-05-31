//! Equipment trait, registry, and built-in equipment modules.

#[macro_use]
mod macros;

pub mod battery;
pub mod config;
pub mod ev;
pub mod event_load;
pub mod generator;
pub mod hvac;
pub mod ndinterp;
pub mod protocol_bridge;
pub mod pv;
pub mod registry;
pub(crate) mod schedule_helpers;
pub mod scheduled_load;
pub mod ventilation;
pub mod water_heater;

use std::time::Duration;

use hares_types::{
    BmsMode, ChargingStrategy, ControlSignal, CoreCapabilities, EnvironmentState,
    EquipmentDescriptor, GridExportRule, HaresError, OperatingMode, PlugInPolicy, PortDeclaration,
    PortSlots, ensure_signal_supported,
};
use serde::{Serialize, de::DeserializeOwned};

/// Configuration seed for auto-registering an actor for this equipment.
/// Equipment that wants a built-in actor overrides `actor_seed()`.
#[derive(Clone, Debug)]
pub enum ActorSeed {
    Battery {
        bms_mode: BmsMode,
        grid_export_rule: GridExportRule,
        max_charge_kw: f64,
        max_discharge_kw: f64,
    },
    Ev {
        strategy: ChargingStrategy,
        plug_in_policy: PlugInPolicy,
        capacity_kwh: f64,
        max_charge_kw: f64,
        fuel_economy_kwh_per_mi: f64,
    },
}

pub use battery::{BatteryConfig, BatteryLutType, OcvTable, UNegTable};
pub use config::{ConfigPayload, EquipmentConfig, EquipmentTypedConfig, SetpointReconciliation};
pub use ev::ChargingCurveLut;
pub use ev::EvConfig;
pub use generator::GeneratorConfig;
pub use hares_types::Telemetry;
pub use hares_types::{CoreFlows, CoreOutput, CorePerformance, CoreState};
pub use hvac::cooling_config::{CentralAirConditionerConfig, DehumidifierConfig, RoomAcConfig};
pub use hvac::heat_pump_config::{
    HeatPumpCommonConfig, HeatPumpConfig, HeatPumpCoolerConfig, HeatPumpHeaterConfig,
};
pub use hvac::heating_config::{
    DuctConfig, ElectricBaseboardConfig, ElectricBoilerConfig, ElectricFurnaceConfig,
    GasBoilerConfig, GasFurnaceConfig, HvacSetpointConfig, IdealHvacConfig,
};
pub use hvac::{
    AIRFLOW_CENTRAL_AC_M3_S_PER_W, AIRFLOW_HEATING_M3_S_PER_W, AIRFLOW_MSHP_COOLING_M3_S_PER_W,
    AIRFLOW_ROOM_AC_M3_S_PER_W, EquivalentBatteryModel, HvacConfig, HvacControlState,
    HvacEquipment, HvacEquipmentType, HvacRuntimeState, MAX_SPEEDS, RuntimeSetpointOverride,
};
pub use hvac::{DefrostConfig, DefrostControl, DefrostStrategy};
pub use ndinterp::RegularGridInterpolator;
pub use protocol_bridge::{
    JsonHandler, ProtocolBridgeConfig, config::HandlerConfig, handler::EquipmentCommand,
};
pub use pv::PvConfig;
pub use registry::{CANONICAL_EQUIPMENT_NAMES, EquipmentFactory, EquipmentRegistry};
pub use ventilation::VentilationConfig;
pub use water_heater::DHW_DEMAND_LOOP;
pub use water_heater::wh_config::{
    ElectricResistanceWaterHeaterConfig, GasWaterHeaterConfig, HeatPumpWaterHeaterConfig,
    IndirectTankConfig, TanklessWaterHeaterConfig,
};

/// Equipment-layer result type.
pub type Result<T> = std::result::Result<T, HaresError>;

/// Piecewise-linear temperature derate: returns 0.0 at or below `temp_min`,
/// 1.0 at or above `temp_max`, and linearly interpolates between.
#[inline]
pub(crate) fn linear_temp_derate(temp_c: f64, temp_min: f64, temp_max: f64) -> f64 {
    if temp_c >= temp_max {
        1.0
    } else if temp_c <= temp_min {
        0.0
    } else {
        let span = temp_max - temp_min;
        if span <= 1e-12 {
            0.0
        } else {
            (temp_c - temp_min) / span
        }
    }
}

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
    fn core_output(&self) -> &CoreOutput;

    fn save_state(&self) -> Vec<u8>;
    fn load_state(&mut self, state: &[u8]) -> Result<()>;

    /// Equipment-specific control application (no capability check).
    ///
    /// Implementors: override this method with equipment-specific logic.
    /// Do NOT override `apply_control` -- it provides the capability gate and
    /// delegates to this method after validation.
    fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> Result<()>;

    /// Capability-gated control dispatch boundary.
    fn apply_control(&mut self, signal: &ControlSignal) -> Result<()> {
        ensure_signal_supported(self.descriptor().control_capabilities, signal)?;
        self.apply_control_unchecked(signal)
    }

    /// Read-only pre-flight check: validates capability flags and any
    /// equipment-specific state preconditions without applying side effects.
    ///
    /// The default checks only capability flags. Equipment with state-dependent
    /// preconditions (e.g. EV connection state) must override this method.
    fn validate_signal(&self, signal: &ControlSignal) -> Result<()> {
        ensure_signal_supported(self.descriptor().control_capabilities, signal)
    }

    /// Drains pending `(target_name, ControlSignal)` pairs that this
    /// equipment generated during the most recent `apply_control` call.
    ///
    /// Equipment that translates one control signal into others (e.g.
    /// `ProtocolBridge` converting a `ProtocolNative` payload into standard
    /// `ControlSignal` variants) accumulates the derived signals and returns
    /// them here. The dwelling dispatch loop collects them at the end of each
    /// dispatch pass and routes them immediately — no timestep delay.
    ///
    /// The default implementation returns an empty vec. Override only in
    /// equipment that produces derived signals.
    fn drain_command_signals(&mut self) -> Vec<(String, ControlSignal)> {
        Vec::new()
    }

    /// Returns whether the equipment's `zone_id` was explicitly set in its config
    /// (true) or silently fell back to the default `ZoneId(1)` (false).
    fn zone_id_explicit(&self) -> bool {
        true
    }

    /// Declares the core output capabilities this equipment type supports.
    ///
    /// Used to populate `EquipmentDescriptor::core_capabilities` at
    /// construction time. The default returns an empty set; equipment types
    /// that populate `CoreOutput` fields override this.
    fn core_capabilities() -> CoreCapabilities
    where
        Self: Sized,
    {
        CoreCapabilities::empty()
    }

    /// Returns the zone and ideal heating/cooling capacity (watts) this
    /// equipment wants the thermal solver to back-calculate.
    fn ideal_target(&self) -> Option<(hares_types::ZoneId, f64)> {
        None
    }

    /// Returns an actor seed if this equipment wants a built-in actor
    /// auto-registered during dwelling initialization.
    fn actor_seed(&self) -> Option<ActorSeed> {
        None
    }

    /// Returns the per-timestep effective ventilation recovery efficiencies
    /// `(sensible, latent)` if this equipment is a ventilation device (HRV/ERV).
    ///
    /// These values account for bypass and defrost derating and must be
    /// propagated to `ThermalSolverConfig.ventilation` before each thermal
    /// solver integration step.  Returns `None` for non-ventilation equipment.
    fn effective_ventilation_effectiveness(&self) -> Option<(f64, f64)> {
        None
    }

    // -----------------------------------------------------------------
    // LUT injection (overridden by Battery and EV)
    // -----------------------------------------------------------------

    /// Set or clear the CC-CV charging curve LUT.
    fn set_charging_curve_lut(
        &mut self,
        _lut: Option<ndinterp::RegularGridInterpolator>,
    ) -> Result<()> {
        Err(HaresError::Equipment(format!(
            "equipment '{}' does not support charging curve LUT",
            self.descriptor().name
        )))
    }

    /// Replace the OCV table with a custom one (e.g. LFP, NCA chemistry).
    fn set_ocv_table(&mut self, _table: battery::OcvTable) -> Result<()> {
        Err(HaresError::Equipment(format!(
            "equipment '{}' does not support OCV table",
            self.descriptor().name
        )))
    }

    /// Replace the negative electrode potential table.
    fn set_u_neg_table(&mut self, _table: battery::UNegTable) -> Result<()> {
        Err(HaresError::Equipment(format!(
            "equipment '{}' does not support UNeg table",
            self.descriptor().name
        )))
    }

    /// Reset OCV table to hardcoded Li-NMC default.
    fn reset_ocv_table(&mut self) -> Result<()> {
        Err(HaresError::Equipment(format!(
            "equipment '{}' does not support OCV table",
            self.descriptor().name
        )))
    }

    /// Reset UNeg table to hardcoded Li-NMC default.
    fn reset_u_neg_table(&mut self) -> Result<()> {
        Err(HaresError::Equipment(format!(
            "equipment '{}' does not support UNeg table",
            self.descriptor().name
        )))
    }

    /// Whether a custom charging curve LUT is currently set.
    fn has_charging_curve_lut(&self) -> bool {
        false
    }

    /// Whether a custom (non-default) OCV table is currently set.
    fn has_custom_ocv_table(&self) -> bool {
        false
    }

    /// Whether a custom (non-default) UNeg table is currently set.
    fn has_custom_u_neg_table(&self) -> bool {
        false
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

/// Non-panicking variant for callers that need graceful error handling
/// (e.g., checkpoint paths where a serialization failure should not
/// terminate a long-running simulation).
#[must_use = "serialization errors should be handled, not silently discarded"]
pub fn try_save_postcard<T: Serialize>(state: &T) -> Result<Vec<u8>> {
    postcard::to_allocvec(state)
        .map_err(|e| HaresError::Equipment(format!("state serialization failed: {e}")))
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
    use super::linear_temp_derate;
    use std::borrow::Cow;
    use std::time::Duration;

    use chrono::{FixedOffset, TimeZone};
    use hares_types::{
        ControlCapabilities, ControlSignal, CoreCapabilities, CoreFlows, CoreOutput,
        CorePerformance, CoreState, DRLevel, EndUse, EnvironmentState, EquipmentDescriptor,
        EquipmentId, EvConnectionState, ExecutionStage, FluidType, FuelType, GridState,
        IdealCapacityMode, InverterPriority, LoopId, OperatingMode, PortDeclaration, PortSlots,
        ProtocolId, SurfaceIrradiance, Telemetry, TelemetryField, WeatherState, ZoneId, ZoneState,
    };
    use serde::{Deserialize, Serialize};

    use crate::config::ConfigPayload;
    use crate::{Equipment, EquipmentConfig, EquipmentRegistry, load_postcard, save_postcard};

    #[derive(Clone)]
    struct MockEquipment {
        descriptor: EquipmentDescriptor,
        ports: Vec<PortDeclaration>,
        telemetry: Telemetry,
        mode: OperatingMode,
        state_value: f64,
        core_output: CoreOutput,
    }

    impl MockEquipment {
        fn new(control_capabilities: ControlCapabilities) -> Self {
            Self {
                descriptor: EquipmentDescriptor {
                    id: EquipmentId(1),
                    name: "Mock".to_string(),
                    end_use: EndUse::OTHER,
                    equipment_type: Cow::Borrowed("Mock"),
                    zone: Some(ZoneId(1)),
                    fuel: FuelType::Electric,
                    stage: ExecutionStage::Independent,
                    control_capabilities,
                    core_capabilities: CoreCapabilities::empty(),
                    telemetry_fields: vec![TelemetryField {
                        name: "x".to_string(),
                        unit: "-".to_string(),
                        description: "mock value".to_string(),
                    }],
                    zone_type: None,
                },
                ports: vec![PortDeclaration::electrical()],
                telemetry: Telemetry::with_capacity(2),
                mode: OperatingMode::Off,
                state_value: 0.0,
                core_output: CoreOutput::default(),
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

        fn core_output(&self) -> &CoreOutput {
            &self.core_output
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
            equipment_telemetry: std::collections::HashMap::new(),
            equipment_core: Default::default(),
            current_time: FixedOffset::east_opt(0)
                .expect("UTC offset")
                .with_ymd_and_hms(2026, 3, 18, 12, 0, 0)
                .single()
                .expect("valid"),
            time_res: chrono::Duration::seconds(60),
            price_signal: Default::default(),
            electrical: Default::default(),
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
            &EquipmentConfig::with_payload(
                "Mock".to_string(),
                "Mock".to_string(),
                ConfigPayload::default(),
            ),
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
            EquipmentConfig::with_payload(
                "x".to_string(),
                "x".to_string(),
                ConfigPayload::default(),
            ),
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
                EquipmentConfig::with_payload(
                    "x".to_string(),
                    "Mock".to_string(),
                    ConfigPayload::default(),
                ),
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

    #[test]
    fn equipment_ideal_target_default_returns_none() {
        let eq = MockEquipment::new(ControlCapabilities::POWER_SETPOINT);
        assert!(eq.ideal_target().is_none());
    }

    #[test]
    fn equipment_without_ideal_capacity_capability_rejects_ideal_capacity_signal() {
        let mut eq = MockEquipment::new(ControlCapabilities::POWER_SETPOINT);
        let signal = ControlSignal::IdealCapacity { capacity_w: 1000.0 };
        let result = eq.apply_control(&signal);
        assert!(result.is_err());
    }

    // ---------------------------------------------------------------
    // Equipment trait LUT default tests
    // ---------------------------------------------------------------

    #[test]
    fn default_set_charging_curve_lut_returns_err() {
        let mut eq = MockEquipment::new(ControlCapabilities::POWER_SETPOINT);
        let lut =
            crate::ndinterp::RegularGridInterpolator::new(vec![vec![0.0, 1.0]], vec![1.0, 0.0])
                .unwrap();
        let result = eq.set_charging_curve_lut(Some(lut));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not support"));
    }

    #[test]
    fn default_set_ocv_table_returns_err() {
        let mut eq = MockEquipment::new(ControlCapabilities::POWER_SETPOINT);
        let table = crate::battery::ocv::OcvTable::default_li_nmc();
        let result = eq.set_ocv_table(table);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not support"));
    }

    #[test]
    fn default_set_u_neg_table_returns_err() {
        let mut eq = MockEquipment::new(ControlCapabilities::POWER_SETPOINT);
        let table = crate::battery::ocv::UNegTable::default_li_nmc();
        let result = eq.set_u_neg_table(table);
        assert!(result.is_err());
    }

    #[test]
    fn default_reset_ocv_table_returns_err() {
        let mut eq = MockEquipment::new(ControlCapabilities::POWER_SETPOINT);
        assert!(eq.reset_ocv_table().is_err());
    }

    #[test]
    fn default_reset_u_neg_table_returns_err() {
        let mut eq = MockEquipment::new(ControlCapabilities::POWER_SETPOINT);
        assert!(eq.reset_u_neg_table().is_err());
    }

    #[test]
    fn linear_temp_derate_at_min_returns_zero() {
        assert_eq!(linear_temp_derate(-20.0, -20.0, 10.0), 0.0);
    }

    #[test]
    fn linear_temp_derate_below_min_returns_zero() {
        assert_eq!(linear_temp_derate(-30.0, -20.0, 10.0), 0.0);
    }

    #[test]
    fn linear_temp_derate_at_max_returns_one() {
        assert_eq!(linear_temp_derate(10.0, -20.0, 10.0), 1.0);
    }

    #[test]
    fn linear_temp_derate_above_max_returns_one() {
        assert_eq!(linear_temp_derate(50.0, -20.0, 10.0), 1.0);
    }

    #[test]
    fn linear_temp_derate_midpoint() {
        assert!((linear_temp_derate(-5.0, -20.0, 10.0) - 0.5).abs() < 1e-10);
    }

    #[test]
    fn linear_temp_derate_quarter_point() {
        assert!((linear_temp_derate(-12.5, -20.0, 10.0) - 0.25).abs() < 1e-10);
    }

    #[test]
    fn linear_temp_derate_degenerate_span_returns_zero() {
        assert_eq!(linear_temp_derate(5.0, 10.0, 10.0), 0.0);
    }

    #[test]
    fn equipment_core_output_default_returns_empty() {
        let eq = MockEquipment::new(ControlCapabilities::POWER_SETPOINT);
        let out = eq.core_output();
        assert!(out.flows.electric_kw.is_none());
        assert!(out.flows.reactive_power_kvar.is_none());
        assert!(out.flows.fuel_w.is_none());
        assert!(out.state.operating_mode.is_none());
        assert!(out.state.soc.is_none());
    }

    #[test]
    fn equipment_core_output_returns_cached_field() {
        use hares_types::ElectricPower;
        let mut eq = MockEquipment::new(ControlCapabilities::POWER_SETPOINT);
        eq.core_output = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(1.5)),
                reactive_power_kvar: Some(0.2),
                fuel_w: None,
                thermal_output_w: None,
                sensible_cooling_w: None,
                latent_cooling_w: None,
            },
            state: CoreState {
                operating_mode: Some(OperatingMode::Charging),
                soc: Some(std::convert::TryInto::try_into(0.8).unwrap()),
                speed_index: None,
                setpoint_c: None,
            },
            performance: CorePerformance::default(),
        };
        let out = eq.core_output();
        assert!(
            matches!(out.flows.electric_kw, Some(ElectricPower::Consumption(kw)) if (kw - 1.5).abs() < 1e-10)
        );
        assert_eq!(out.state.operating_mode, Some(OperatingMode::Charging));
    }

    #[test]
    fn every_control_signal_variant_has_at_least_one_equipment_consumer() {
        use std::panic::AssertUnwindSafe;

        let registry = EquipmentRegistry::new();
        let payload = ConfigPayload::default();

        // Collect the union of all declared capabilities across all built-in
        // equipment types. Use catch_unwind because some constructors
        // (e.g. TanklessWH) panic on missing typed config in their `new()`
        // rather than deferring to init().
        let mut all_caps = ControlCapabilities::empty();
        for class in registry.known_names() {
            let raw_cfg = EquipmentConfig::with_payload(
                class.to_string(),
                class.to_string(),
                payload.clone(),
            );
            let caps = std::panic::catch_unwind(AssertUnwindSafe(|| {
                let mut eq = registry.create(class, raw_cfg.clone()).ok()?;
                let _ = eq.init(&raw_cfg, &sample_env());
                Some(eq.descriptor().control_capabilities)
            }));
            if let Ok(Some(caps)) = caps {
                all_caps |= caps;
            }
        }

        // Signal representatives: one per ControlSignal variant with its
        // required capability.
        let representatives: &[(ControlSignal, ControlCapabilities)] = &[
            (
                ControlSignal::ThermalSetpoint {
                    heating_setpoint_c: Some(20.0),
                    cooling_setpoint_c: Some(24.0),
                    deadband_c: Some(1.0),
                },
                ControlCapabilities::THERMAL_SETPOINT,
            ),
            (
                ControlSignal::HumiditySetpoint {
                    target_rh: 0.45,
                    min_rh: None,
                    max_rh: None,
                },
                ControlCapabilities::HUMIDITY_SETPOINT,
            ),
            (
                ControlSignal::PowerSetpoint {
                    active_power_kw: 1.0,
                    reactive_power_kvar: None,
                },
                ControlCapabilities::POWER_SETPOINT,
            ),
            (
                ControlSignal::PowerLimit {
                    max_power_kw: 5.0,
                    ramp_rate_kw_per_s: None,
                },
                ControlCapabilities::POWER_LIMIT,
            ),
            (
                ControlSignal::SOCTarget {
                    target_soc: 0.5,
                    min_soc: None,
                    max_soc: None,
                },
                ControlCapabilities::SOC_TARGET,
            ),
            (
                ControlSignal::ModeOverride {
                    mode: OperatingMode::Off,
                },
                ControlCapabilities::MODE_OVERRIDE,
            ),
            (
                ControlSignal::DutyCycle {
                    on_fraction: 0.5,
                    period_s: None,
                    component: None,
                },
                ControlCapabilities::DUTY_CYCLE,
            ),
            (
                ControlSignal::LoadFraction { fraction: 0.5 },
                ControlCapabilities::LOAD_FRACTION,
            ),
            (
                ControlSignal::GridConnect { connected: true },
                ControlCapabilities::GRID_CONNECT,
            ),
            (
                ControlSignal::SelfConsumption {
                    enabled: true,
                    solar_only_charging: false,
                },
                ControlCapabilities::SELF_CONSUMPTION,
            ),
            (
                ControlSignal::DemandResponse {
                    level: DRLevel::Moderate,
                    duration_s: Some(600.0),
                },
                ControlCapabilities::DEMAND_RESPONSE,
            ),
            (
                ControlSignal::ProtocolNative {
                    protocol: ProtocolId(1),
                    payload: vec![0x01],
                },
                ControlCapabilities::PROTOCOL_NATIVE,
            ),
            (
                ControlSignal::CurtailmentPercent { percent: 50.0 },
                ControlCapabilities::CURTAILMENT_PERCENT,
            ),
            (
                ControlSignal::ReactiveSetpoint { kvar: 1.0 },
                ControlCapabilities::REACTIVE_SETPOINT,
            ),
            (
                ControlSignal::PowerFactorSetpoint { power_factor: 0.95 },
                ControlCapabilities::POWER_FACTOR_SETPOINT,
            ),
            (
                ControlSignal::InverterPriorityMode {
                    priority: InverterPriority::Watt,
                },
                ControlCapabilities::INVERTER_PRIORITY_MODE,
            ),
            (
                ControlSignal::IdealCapacity { capacity_w: 1000.0 },
                ControlCapabilities::IDEAL_CAPACITY,
            ),
            (
                ControlSignal::ThermalSetpointDelta {
                    heating_delta_c: Some(1.0),
                    cooling_delta_c: None,
                },
                ControlCapabilities::THERMAL_SETPOINT_DELTA,
            ),
            (
                ControlSignal::IdealCapacityModeOverride {
                    mode: IdealCapacityMode::On,
                },
                ControlCapabilities::IDEAL_CAPACITY_MODE_OVERRIDE,
            ),
            (
                ControlSignal::EvPlugIn {
                    state: EvConnectionState::HomePluggedIn,
                },
                ControlCapabilities::EV_PLUG_IN,
            ),
            (
                ControlSignal::EvDrive { kwh: 1.0 },
                ControlCapabilities::EV_DRIVE,
            ),
            (
                ControlSignal::EvAwayCharge { power_kw: 1.0 },
                ControlCapabilities::EV_AWAY_CHARGE,
            ),
            (
                ControlSignal::EvSetReadyBy {
                    departure_hour: 7.0,
                    target_soc: 0.8,
                },
                ControlCapabilities::EV_SET_READY_BY,
            ),
            (
                ControlSignal::EventDelay { delay_s: 60.0 },
                ControlCapabilities::EVENT_DELAY,
            ),
            (
                ControlSignal::MaxCapacityFraction { fraction: 0.8 },
                ControlCapabilities::MAX_CAPACITY_FRACTION,
            ),
        ];

        for (signal, required) in representatives {
            assert!(
                all_caps.contains(*required),
                "ControlSignal variant {signal:?} (requires {required:?}) has no equipment \
                 consumer — add an equipment type that declares this capability, \
                 or remove the variant if it is no longer planned"
            );
        }
    }
}
