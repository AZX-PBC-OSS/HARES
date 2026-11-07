//! The dwelling tests' shared equipment double: a controllable stub the
//! dispatcher, roster, and step tests register and dispatch against.

use std::borrow::Cow;

use hares_equipment::{Equipment, EquipmentConfig};
use hares_types::{
    ControlCapabilities, ControlSignal, CoreCapabilities, CoreOutput, DRLevel, EquipmentDescriptor,
    EquipmentId, EndUse, ExecutionStage, FuelType, OperatingMode, PortDeclaration, PortSlots,
    Telemetry, TelemetryField, ZoneId, telemetry_keys as tk,
};

    pub(super) struct TestEquipment {
        pub(super) descriptor: EquipmentDescriptor,
        pub(super) telemetry: Telemetry,
        pub(super) last_power_kw: f64,
        pub(super) last_soc_target: f64,
        pub(super) last_dr_level: Option<DRLevel>,
        pub(super) core_output: CoreOutput,
        pub(super) initialized: bool,
    }

    impl TestEquipment {
        pub(super) fn new(name: &str, capabilities: ControlCapabilities) -> Self {
            Self {
                descriptor: EquipmentDescriptor {
                    // Unassigned sentinel: when this stub joins a dwelling
                    // through `add_equipment`, the dwelling auto-assigns its
                    // never-reused id — the same path every Python-side
                    // `add_*` equipment takes. A hard-coded explicit id
                    // would collide with the dwelling's assembly-assigned
                    // ids (they start at 1).
                    id: EquipmentId(0),
                    name: name.to_string(),
                    end_use: EndUse::OTHER,
                    equipment_type: Cow::Borrowed("TestEquipment"),
                    zone: Some(ZoneId(1)),
                    fuel: FuelType::Electric,
                    stage: ExecutionStage::Independent,
                    control_capabilities: capabilities,
                    core_capabilities: CoreCapabilities::empty(),
                    telemetry_fields: vec![
                        TelemetryField::power_kw(tk::LAST_POWER_KW, "last applied power"),
                        TelemetryField::ratio(tk::LAST_SOC_TARGET, "last applied SOC target"),
                        TelemetryField::ratio("last_dr_level", "last applied DR level"),
                    ],
                    zone_type: None,
                },
                telemetry: Telemetry::with_capacity(3),
                last_power_kw: 0.0,
                last_soc_target: 0.0,
                last_dr_level: None,
                core_output: CoreOutput::default(),
                initialized: false,
            }
        }
    }

    impl Equipment for TestEquipment {
        fn descriptor(&self) -> &EquipmentDescriptor {
            &self.descriptor
        }

        fn resolved_parameters(&self) -> Vec<hares_types::ResolvedParameter> {
            Vec::new()
        }

        fn rename(&mut self, name: String) {
            self.descriptor.name = name;
        }

        fn set_equipment_id(
            &mut self,
            id: EquipmentId,
        ) -> std::result::Result<(), hares_types::HaresError> {
            hares_equipment::apply_identity_write(self.is_initialized(), &mut self.descriptor, id)
        }

        fn ports(&self) -> &[PortDeclaration] {
            &[]
        }

        fn init(
            &mut self,
            _config: &EquipmentConfig,
            _env: &hares_types::EnvironmentState,
        ) -> std::result::Result<(), hares_types::HaresError> {
            self.telemetry.insert(tk::LAST_POWER_KW, self.last_power_kw);
            self.telemetry
                .insert(tk::LAST_SOC_TARGET, self.last_soc_target);
            Ok(())
        }

        fn update_control(&mut self, _env: &hares_types::EnvironmentState) -> OperatingMode {
            OperatingMode::Off
        }

        fn step(
            &mut self,
            _env: &hares_types::EnvironmentState,
            _dt: std::time::Duration,
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

        fn save_state(&self) -> std::result::Result<Vec<u8>, hares_types::HaresError> {
            Ok(vec![])
        }

        fn load_state(
            &mut self,
            _state: &[u8],
        ) -> std::result::Result<(), hares_types::HaresError> {
            Ok(())
        }

        fn apply_signal(
            &mut self,
            signal: &ControlSignal,
        ) -> std::result::Result<(), hares_types::HaresError> {
            match signal {
                ControlSignal::PowerSetpoint { active_power_w, .. } => {
                    self.last_power_kw = *active_power_w;
                    self.telemetry.insert(tk::LAST_POWER_KW, self.last_power_kw);
                }
                ControlSignal::SOCTarget { target_soc, .. } => {
                    self.last_soc_target = *target_soc;
                    self.telemetry
                        .insert(tk::LAST_SOC_TARGET, self.last_soc_target);
                }
                ControlSignal::DemandResponse { level, .. } => {
                    self.last_dr_level = Some(*level);
                    self.telemetry.insert("last_dr_level", *level as u8 as f64);
                }
                _ => {}
            }
            Ok(())
        }

        fn is_initialized(&self) -> bool {
            self.initialized
        }

        fn mark_initialized(&mut self) {
            self.initialized = true;
        }

        fn unmark_initialized(&mut self) {
            self.initialized = false;
        }
    }
