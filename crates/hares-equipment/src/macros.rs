//! Shared procedural macros for equipment boilerplate reduction.

/// Generate a full `impl Equipment for $outer` block that delegates every
/// trait method to `self.$inner`.
///
/// The inner field must expose the same method signatures as the `Equipment`
/// trait, either by implementing `Equipment` itself or by having identically
/// named methods.
///
/// ```ignore
/// delegate_equipment!(AirConditioner, core);
/// delegate_equipment!(HpCooler, inner);
/// ```
macro_rules! delegate_equipment {
    ($outer:ty, $inner:ident) => {
        impl $crate::Equipment for $outer {
            fn descriptor(&self) -> &hares_types::EquipmentDescriptor {
                self.$inner.descriptor()
            }

            fn zone_id_explicit(&self) -> bool {
                self.$inner.zone_id_explicit()
            }

            fn ports(&self) -> &[hares_types::PortDeclaration] {
                self.$inner.ports()
            }

            fn init(
                &mut self,
                config: &$crate::EquipmentConfig,
                env: &hares_types::EnvironmentState,
            ) -> $crate::Result<()> {
                self.$inner.init(config, env)
            }

            fn update_control(
                &mut self,
                env: &hares_types::EnvironmentState,
            ) -> hares_types::OperatingMode {
                self.$inner.update_control(env)
            }

            fn step(
                &mut self,
                env: &hares_types::EnvironmentState,
                dt: std::time::Duration,
                ports: &mut hares_types::PortSlots,
            ) -> std::result::Result<(), hares_types::HaresError> {
                self.$inner.step(env, dt, ports)
            }

            fn telemetry(&self) -> &hares_types::Telemetry {
                self.$inner.telemetry()
            }

            fn core_output(&self) -> &hares_types::CoreOutput {
                self.$inner.core_output()
            }

            fn save_state(&self) -> $crate::Result<Vec<u8>> {
                self.$inner.save_state()
            }

            fn load_state(&mut self, state: &[u8]) -> $crate::Result<()> {
                self.$inner.load_state(state)
            }

            fn apply_control_unchecked(
                &mut self,
                signal: &hares_types::ControlSignal,
            ) -> $crate::Result<()> {
                self.$inner.apply_control_unchecked(signal)
            }

            fn rename(&mut self, name: String) {
                self.$inner.rename(name)
            }

            fn ideal_target(&self) -> Option<(hares_types::ZoneId, f64)> {
                self.$inner.ideal_target()
            }

            fn island_source_available(&self) -> bool {
                self.$inner.island_source_available()
            }

            fn resolved_zip(&self) -> Option<hares_types::zip::ZipLoad> {
                self.$inner.resolved_zip()
            }
        }
    };
}
