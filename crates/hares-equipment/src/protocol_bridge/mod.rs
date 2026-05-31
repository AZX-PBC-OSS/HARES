//! Protocol bridge — canonical consumer of `ProtocolNative` control signals.
//!
//! The protocol bridge receives typed protocol-native commands
//! (`ControlSignal::ProtocolNative`) dispatched through the control system,
//! parses their payloads through registered protocol handlers, and converts
//! them into standard `ControlSignal` variants that the dwelling dispatch
//! loop routes to target equipment within the same timestep.
//!
//! It is the single equipment type that declares `PROTOCOL_NATIVE` capability,
//! eliminating the dead-code path where the variant was constructible but
//! never consumable.
//!
//! ## Architecture
//!
//! ```text
//! ProtocolNative { protocol, payload }
//!         │
//!         ▼
//!   ProtocolBridge.apply_control_unchecked()
//!         │
//!         ▼ lookup handler by protocol id
//!   ProtocolHandler::parse(payload)
//!         │
//!         ▼ returns Vec<EquipmentCommand>
//!   drain_command_signals() → dwelling dispatch loop
//!         │
//!         ▼ standard ControlDispatcher
//!   target equipment (Battery, HVAC, EV, …)
//! ```
//!
//! Handlers are registered via typed configuration. Built-in handlers:
//!
//! | Handler       | Protocol ID | Payload format              |
//! |---------------|------------|-----------------------------|
//! | `JsonHandler` | configurable | UTF-8 JSON `[{target, signal}]` |

pub mod config;
pub mod handler;

use std::borrow::Cow;
use std::time::Duration;

use hares_types::telemetry_keys as tk;
use hares_types::{
    ControlCapabilities, ControlSignal, CoreCapabilities, CoreFlows, CoreOutput, CorePerformance,
    CoreState, EndUse, EnvironmentState, EquipmentDescriptor, EquipmentId, ExecutionStage,
    FuelType, HaresError, OperatingMode, PortDeclaration, PortSlots, ProtocolId, Telemetry,
    TelemetryField,
};
use serde::{Deserialize, Serialize};

use crate::protocol_bridge::handler::{EquipmentCommand, ProtocolHandler};
use crate::{Equipment, EquipmentConfig, EquipmentRegistry, load_postcard, save_postcard};

pub use config::ProtocolBridgeConfig;
pub use handler::JsonHandler;

// ---------------------------------------------------------------------------
// Const
// ---------------------------------------------------------------------------

const KEY_EQUIPMENT_ID: &str = "equipment_id";

// ---------------------------------------------------------------------------
// Checkpoint
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ProtocolBridgeCheckpoint {
    dispatch_count: u32,
    total_commands_parsed: u32,
    parse_error_count: u32,
}

// ---------------------------------------------------------------------------
// Struct
// ---------------------------------------------------------------------------

pub struct ProtocolBridge {
    descriptor: EquipmentDescriptor,
    ports: Vec<PortDeclaration>,
    telemetry: Telemetry,
    core_output: CoreOutput,

    /// Protocol IDs registered with this bridge (empty = accept all).
    registered_protocols: Vec<u16>,

    /// Registered protocol handlers keyed by ProtocolId.
    handlers: Vec<Box<dyn ProtocolHandler>>,

    /// Cumulative count of ProtocolNative signals received.
    dispatch_count: u32,

    /// Cumulative count of equipment commands parsed across all payloads.
    total_commands_parsed: u32,

    /// Cumulative count of handler parse failures.
    parse_error_count: u32,

    /// Commands parsed from the most recent `apply_control_unchecked` call,
    /// pending collection by the dwelling dispatch loop via
    /// `drain_command_signals()`.
    pending_commands: Vec<EquipmentCommand>,

    /// Number of commands parsed in the most recent dispatch. Captured before
    /// `drain_command_signals` drains `pending_commands` so telemetry reports
    /// the correct per-dispatch count even after the dwelling loop has drained
    /// the buffer (the dwelling calls step() after drain_command_signals()).
    last_dispatch_command_count: u32,
}

impl ProtocolBridge {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        let equipment_id = config
            .get_f64(KEY_EQUIPMENT_ID)
            .map(|v| v as u32)
            .unwrap_or(0);

        let descriptor = EquipmentDescriptor {
            id: EquipmentId(equipment_id),
            name: config.name.clone(),
            end_use: EndUse::OTHER,
            equipment_type: Cow::Borrowed("ProtocolBridge"),
            zone: None,
            fuel: FuelType::Electric,
            stage: ExecutionStage::Independent,
            control_capabilities: ControlCapabilities::PROTOCOL_NATIVE,
            core_capabilities: CoreCapabilities::empty(),
            telemetry_fields: telemetry_fields(),
            zone_type: None,
        };

        let ports = Vec::new();

        Self {
            descriptor,
            ports,
            telemetry: default_telemetry(),
            core_output: CoreOutput::default(),
            registered_protocols: Vec::new(),
            handlers: Vec::new(),
            dispatch_count: 0,
            total_commands_parsed: 0,
            parse_error_count: 0,
            pending_commands: Vec::new(),
            last_dispatch_command_count: 0,
        }
    }

    fn init_typed(&mut self, config: &EquipmentConfig) -> crate::Result<()> {
        let c = config.require_typed::<ProtocolBridgeConfig>("ProtocolBridge")?;
        c.validate()?;
        self.registered_protocols = c.registered_protocols.clone();
        self.handlers = c
            .handlers
            .iter()
            .map(|h| -> crate::Result<Box<dyn ProtocolHandler>> {
                match h {
                    config::HandlerConfig::Json { protocol_id } => {
                        Ok(Box::new(JsonHandler::new(ProtocolId(*protocol_id))))
                    }
                }
            })
            .collect::<crate::Result<Vec<_>>>()?;
        Ok(())
    }

    /// Returns true if the protocol ID is registered (or the registry is empty = accept all).
    fn is_registered(&self, protocol_id: u16) -> bool {
        self.registered_protocols.is_empty() || self.registered_protocols.contains(&protocol_id)
    }

    /// Look up the handler for `protocol_id`. Returns `None` if no handler is
    /// registered — the payload will be recorded in telemetry but not parsed.
    fn find_handler(&self, protocol_id: ProtocolId) -> Option<&dyn ProtocolHandler> {
        self.handlers
            .iter()
            .find(|h| h.protocol_id() == protocol_id)
            .map(|h| &**h)
    }

    /// Drain all pending equipment commands accumulated by the most recent
    /// `apply_control_unchecked` call. Called by the dwelling dispatch loop
    /// after each dispatch pass so translated commands take effect within
    /// the same timestep.
    pub fn drain_pending_commands(&mut self) -> Vec<EquipmentCommand> {
        std::mem::take(&mut self.pending_commands)
    }
}

impl Equipment for ProtocolBridge {
    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.descriptor
    }

    fn ports(&self) -> &[PortDeclaration] {
        &self.ports
    }

    fn init(&mut self, config: &EquipmentConfig, _env: &EnvironmentState) -> crate::Result<()> {
        self.init_typed(config)
    }

    fn update_control(&mut self, _env: &EnvironmentState) -> OperatingMode {
        OperatingMode::Standby
    }

    fn step(
        &mut self,
        _env: &EnvironmentState,
        _dt: Duration,
        _ports: &mut PortSlots,
    ) -> std::result::Result<(), HaresError> {
        let cmd_count = self.last_dispatch_command_count as f64;
        self.telemetry.set(tk::PARSED_COMMAND_COUNT, cmd_count);
        self.telemetry
            .set(tk::DISPATCH_COUNT, self.dispatch_count as f64);
        self.telemetry
            .set(tk::PARSE_ERROR_COUNT, self.parse_error_count as f64);
        self.telemetry
            .set(tk::TOTAL_COMMANDS_PARSED, self.total_commands_parsed as f64);
        self.core_output = CoreOutput {
            flows: CoreFlows {
                electric_kw: None,
                reactive_power_kvar: None,
                fuel_w: None,
                thermal_output_w: None,
                sensible_cooling_w: None,
                latent_cooling_w: None,
            },
            state: CoreState {
                operating_mode: None,
                soc: None,
                speed_index: None,
                setpoint_c: None,
            },
            performance: CorePerformance::default(),
        };
        Ok(())
    }

    fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    fn core_output(&self) -> &CoreOutput {
        &self.core_output
    }

    fn save_state(&self) -> Vec<u8> {
        save_postcard(&ProtocolBridgeCheckpoint {
            dispatch_count: self.dispatch_count,
            total_commands_parsed: self.total_commands_parsed,
            parse_error_count: self.parse_error_count,
        })
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        let cp: ProtocolBridgeCheckpoint = load_postcard(state)?;
        self.dispatch_count = cp.dispatch_count;
        self.total_commands_parsed = cp.total_commands_parsed;
        self.parse_error_count = cp.parse_error_count;
        self.telemetry
            .set(tk::DISPATCH_COUNT, self.dispatch_count as f64);
        Ok(())
    }

    fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        match signal {
            ControlSignal::ProtocolNative { protocol, payload } => {
                #[cfg(any(debug_assertions, feature = "check_invariants"))]
                debug_assert!(
                    self.descriptor
                        .control_capabilities
                        .contains(ControlCapabilities::PROTOCOL_NATIVE),
                    "ProtocolBridge received ProtocolNative signal but PROTOCOL_NATIVE \
                     capability is missing — capability-gating logic may be bypassed"
                );

                if payload.is_empty() {
                    return Err(HaresError::Control(
                        "ProtocolNative payload must be non-empty".to_string(),
                    ));
                }

                if !self.is_registered(protocol.0) {
                    self.dispatch_count = self.dispatch_count.wrapping_add(1);
                    self.last_dispatch_command_count = 0;
                    self.telemetry.set(tk::PROTOCOL_ID, protocol.0 as f64);
                    self.telemetry
                        .set(tk::PAYLOAD_SIZE_BYTES, payload.len() as f64);
                    self.telemetry
                        .set(tk::DISPATCH_COUNT, self.dispatch_count as f64);
                    return Ok(());
                }

                self.pending_commands.clear();
                match self.find_handler(*protocol) {
                    Some(handler) => {
                        let commands = match handler.parse(payload) {
                            Ok(c) => c,
                            Err(err) => {
                                self.parse_error_count = self.parse_error_count.wrapping_add(1);
                                return Err(err);
                            }
                        };
                        let count = commands.len();
                        self.last_dispatch_command_count = count as u32;
                        self.pending_commands = commands;
                        self.total_commands_parsed =
                            self.total_commands_parsed.wrapping_add(count as u32);
                    }
                    None => {
                        self.last_dispatch_command_count = 0;
                    }
                }

                self.dispatch_count = self.dispatch_count.wrapping_add(1);
                self.telemetry.set(tk::PROTOCOL_ID, protocol.0 as f64);
                self.telemetry
                    .set(tk::PAYLOAD_SIZE_BYTES, payload.len() as f64);
                self.telemetry
                    .set(tk::DISPATCH_COUNT, self.dispatch_count as f64);
            }
            _ => {
                return Err(HaresError::Control(format!(
                    "ProtocolBridge does not handle control signal: {signal:?}"
                )));
            }
        }
        Ok(())
    }

    fn drain_command_signals(&mut self) -> Vec<(String, ControlSignal)> {
        self.drain_pending_commands()
            .into_iter()
            .map(|cmd| (cmd.target_name, cmd.signal))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

pub fn register_with_registry(registry: &mut EquipmentRegistry) {
    registry.register(
        "Protocol Bridge",
        Box::new(|config| Box::new(ProtocolBridge::new(config))),
    );
}

// ---------------------------------------------------------------------------
// Telemetry
// ---------------------------------------------------------------------------

fn default_telemetry() -> Telemetry {
    let mut t = Telemetry::with_capacity(6);
    t.insert(tk::PROTOCOL_ID, 0.0);
    t.insert(tk::PAYLOAD_SIZE_BYTES, 0.0);
    t.insert(tk::DISPATCH_COUNT, 0.0);
    t.insert(tk::PARSED_COMMAND_COUNT, 0.0);
    t.insert(tk::PARSE_ERROR_COUNT, 0.0);
    t.insert(tk::TOTAL_COMMANDS_PARSED, 0.0);
    t
}

fn telemetry_fields() -> Vec<TelemetryField> {
    vec![
        TelemetryField {
            name: tk::PROTOCOL_ID.to_string(),
            unit: "-".to_string(),
            description: "ProtocolId of the most recent ProtocolNative dispatch".to_string(),
        },
        TelemetryField {
            name: tk::PAYLOAD_SIZE_BYTES.to_string(),
            unit: "bytes".to_string(),
            description: "Size in bytes of the most recent ProtocolNative payload".to_string(),
        },
        TelemetryField {
            name: tk::DISPATCH_COUNT.to_string(),
            unit: "-".to_string(),
            description: "Cumulative count of ProtocolNative dispatch events".to_string(),
        },
        TelemetryField {
            name: tk::PARSED_COMMAND_COUNT.to_string(),
            unit: "-".to_string(),
            description: "Number of EquipmentCommands parsed from the most recent payload"
                .to_string(),
        },
        TelemetryField {
            name: tk::PARSE_ERROR_COUNT.to_string(),
            unit: "-".to_string(),
            description: "Cumulative count of handler parse failures".to_string(),
        },
        TelemetryField {
            name: tk::TOTAL_COMMANDS_PARSED.to_string(),
            unit: "-".to_string(),
            description: "Cumulative count of equipment commands parsed across all payloads"
                .to_string(),
        },
    ]
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ConfigPayload;
    use chrono::TimeZone;

    fn minimal_config() -> EquipmentConfig {
        EquipmentConfig::from_typed(
            "test-bridge".to_string(),
            "ProtocolBridge".to_string(),
            ProtocolBridgeConfig {
                equipment_id: None,
                registered_protocols: vec![17, 42],
                handlers: vec![config::HandlerConfig::Json { protocol_id: 17 }],
            },
        )
    }

    fn empty_env() -> EnvironmentState {
        EnvironmentState {
            zones: vec![],
            weather: Default::default(),
            grid: hares_types::GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
            },
            custom_domains: vec![],
            equipment_telemetry: std::collections::HashMap::new(),
            equipment_core: std::collections::HashMap::new(),
            current_time: chrono::FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
                .single()
                .unwrap(),
            time_res: chrono::Duration::seconds(60),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }

    #[test]
    fn constructible() {
        let bridge = ProtocolBridge::new(minimal_config());
        assert_eq!(bridge.descriptor().equipment_type, "ProtocolBridge");
        assert!(
            bridge
                .descriptor()
                .control_capabilities
                .contains(ControlCapabilities::PROTOCOL_NATIVE)
        );
    }

    #[test]
    fn init_with_typed_config() {
        let mut bridge = ProtocolBridge::new(minimal_config());
        let env = empty_env();
        bridge.init(&minimal_config(), &env).unwrap();
        assert_eq!(bridge.registered_protocols, vec![17, 42]);
        assert_eq!(bridge.handlers.len(), 1);
        assert_eq!(bridge.handlers[0].protocol_id().0, 17);
    }

    #[test]
    fn init_without_typed_config_returns_err() {
        let raw_config = EquipmentConfig::with_payload(
            "test-bridge".to_string(),
            "ProtocolBridge".to_string(),
            ConfigPayload::default(),
        );
        let mut bridge = ProtocolBridge::new(raw_config.clone());
        let env = empty_env();
        let result = bridge.init(&raw_config, &env);
        assert!(
            result.is_err(),
            "init without typed config must propagate require_typed error, not silently fall back"
        );
    }

    #[test]
    fn parses_json_payload_into_commands() {
        let mut bridge = ProtocolBridge::new(minimal_config());
        let env = empty_env();
        bridge.init(&minimal_config(), &env).unwrap();

        let payload: Vec<u8> = br#"[
            {"target": "Battery-1", "signal": {"PowerSetpoint": {"active_power_kw": 5.0, "reactive_power_kvar": null}}},
            {"target": "HVAC-1", "signal": {"ModeOverride": {"mode": "Off"}}}
        ]"#
        .to_vec();

        let signal = ControlSignal::ProtocolNative {
            protocol: ProtocolId(17),
            payload,
        };
        bridge.apply_control(&signal).unwrap();

        let commands = bridge.drain_pending_commands();
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0].target_name, "Battery-1");
        assert_eq!(commands[1].target_name, "HVAC-1");
        assert_eq!(
            commands[0].signal,
            ControlSignal::PowerSetpoint {
                active_power_kw: 5.0,
                reactive_power_kvar: None,
            }
        );
    }

    #[test]
    fn drain_command_signals_returns_and_clears() {
        let mut bridge = ProtocolBridge::new(minimal_config());
        let env = empty_env();
        bridge.init(&minimal_config(), &env).unwrap();

        let payload = br#"[{"target": "Battery-1", "signal": {"PowerSetpoint": {"active_power_kw": 1.0, "reactive_power_kvar": null}}}]"#.to_vec();
        bridge
            .apply_control(&ControlSignal::ProtocolNative {
                protocol: ProtocolId(17),
                payload,
            })
            .unwrap();

        let signals: Vec<_> = bridge.drain_command_signals();
        assert_eq!(signals.len(), 1);

        // Second drain returns empty.
        let signals2: Vec<_> = bridge.drain_command_signals();
        assert!(signals2.is_empty());
    }

    #[test]
    fn no_handler_returns_no_commands() {
        let mut bridge = ProtocolBridge::new(minimal_config());
        let env = empty_env();
        bridge.init(&minimal_config(), &env).unwrap();

        // Protocol 42 has no handler registered.
        let signal = ControlSignal::ProtocolNative {
            protocol: ProtocolId(42),
            payload: vec![0x01, 0x02],
        };
        bridge.apply_control(&signal).unwrap();

        let commands = bridge.drain_pending_commands();
        assert!(commands.is_empty());
        // Dispatch was still counted.
        assert_eq!(bridge.telemetry().get(tk::DISPATCH_COUNT), Some(1.0));
    }

    #[test]
    fn accepts_unregistered_protocol_when_accept_all() {
        let config = EquipmentConfig::from_typed(
            "open-bridge".to_string(),
            "ProtocolBridge".to_string(),
            ProtocolBridgeConfig {
                equipment_id: None,
                registered_protocols: vec![],
                handlers: vec![config::HandlerConfig::Json { protocol_id: 99 }],
            },
        );
        let mut bridge = ProtocolBridge::new(config.clone());
        let env = empty_env();
        bridge.init(&config, &env).unwrap();

        let payload = br#"[{"target": "X", "signal": {"ModeOverride": {"mode": "Off"}}}]"#.to_vec();
        let signal = ControlSignal::ProtocolNative {
            protocol: ProtocolId(99),
            payload,
        };
        let result = bridge.apply_control(&signal);
        assert!(result.is_ok());
        assert_eq!(bridge.drain_pending_commands().len(), 1);
    }

    #[test]
    fn rejects_non_protocol_native_signals() {
        let mut bridge = ProtocolBridge::new(minimal_config());
        let env = empty_env();
        bridge.init(&minimal_config(), &env).unwrap();

        let signal = ControlSignal::PowerSetpoint {
            active_power_kw: 5.0,
            reactive_power_kvar: None,
        };
        let result = bridge.apply_control(&signal);
        assert!(result.is_err());
    }

    #[test]
    fn rejects_empty_payload() {
        let mut bridge = ProtocolBridge::new(minimal_config());
        let env = empty_env();
        bridge.init(&minimal_config(), &env).unwrap();

        let signal = ControlSignal::ProtocolNative {
            protocol: ProtocolId(17),
            payload: vec![],
        };
        let result = bridge.apply_control(&signal);
        assert!(result.is_err());
    }

    #[test]
    fn dispatch_count_persists_across_save_load() {
        let mut bridge = ProtocolBridge::new(minimal_config());
        let env = empty_env();
        bridge.init(&minimal_config(), &env).unwrap();

        let payload = br#"[{"target": "X", "signal": {"ModeOverride": {"mode": "Off"}}}]"#.to_vec();
        for _ in 0..3 {
            bridge
                .apply_control(&ControlSignal::ProtocolNative {
                    protocol: ProtocolId(17),
                    payload: payload.clone(),
                })
                .unwrap();
        }
        let saved = bridge.save_state();

        let mut restored = ProtocolBridge::new(minimal_config());
        restored.init(&minimal_config(), &env).unwrap();
        restored.load_state(&saved).unwrap();
        assert_eq!(restored.dispatch_count, 3);
        assert_eq!(restored.telemetry().get(tk::DISPATCH_COUNT), Some(3.0));
    }

    #[test]
    fn capability_gate_passes_for_protocol_native() {
        let mut bridge = ProtocolBridge::new(minimal_config());
        let env = empty_env();
        bridge.init(&minimal_config(), &env).unwrap();

        let signal = ControlSignal::ProtocolNative {
            protocol: ProtocolId(1),
            payload: vec![0xAA],
        };
        let result = bridge.apply_control(&signal);
        assert!(result.is_ok());
    }

    #[test]
    fn step_updates_telemetry() {
        let mut bridge = ProtocolBridge::new(minimal_config());
        let env = empty_env();
        bridge.init(&minimal_config(), &env).unwrap();

        let payload = br#"[{"target": "X", "signal": {"ModeOverride": {"mode": "Off"}}}]"#.to_vec();
        bridge
            .apply_control(&ControlSignal::ProtocolNative {
                protocol: ProtocolId(17),
                payload,
            })
            .unwrap();

        // Drain signals before step — the dwelling dispatch loop calls
        // drain_command_signals() before step(), and PARSED_COMMAND_COUNT
        // must still report the correct value afterward.
        let _ = bridge.drain_command_signals();

        let mut ports = PortSlots::default();
        bridge
            .step(&env, Duration::from_secs(60), &mut ports)
            .unwrap();

        assert_eq!(bridge.telemetry().get(tk::PROTOCOL_ID), Some(17.0));
        assert_eq!(bridge.telemetry().get(tk::PARSED_COMMAND_COUNT), Some(1.0));
        assert_eq!(bridge.telemetry().get(tk::DISPATCH_COUNT), Some(1.0));
        assert_eq!(bridge.telemetry().get(tk::TOTAL_COMMANDS_PARSED), Some(1.0));
    }

    #[test]
    fn unregistered_protocol_rejected_even_with_handler() {
        // Bridge with registered_protocols: [17] but handler for protocol 99.
        // Protocol 99 dispatch must be rejected by is_registered guard.
        let config = EquipmentConfig::from_typed(
            "restricted-bridge".to_string(),
            "ProtocolBridge".to_string(),
            ProtocolBridgeConfig {
                equipment_id: None,
                registered_protocols: vec![17],
                handlers: vec![config::HandlerConfig::Json { protocol_id: 99 }],
            },
        );
        let mut bridge = ProtocolBridge::new(config.clone());
        let env = empty_env();
        bridge.init(&config, &env).unwrap();

        let payload = br#"[{"target": "X", "signal": {"ModeOverride": {"mode": "Off"}}}]"#.to_vec();
        let signal = ControlSignal::ProtocolNative {
            protocol: ProtocolId(99),
            payload,
        };
        let result = bridge.apply_control(&signal);
        assert!(
            result.is_ok(),
            "protocol rejection must not error — it returns Ok with telemetry"
        );
        // No commands parsed — protocol 99 is not in registered_protocols.
        assert!(bridge.drain_pending_commands().is_empty());
        // Dispatch still counted for observability.
        assert_eq!(bridge.dispatch_count, 1);
        assert_eq!(bridge.telemetry().get(tk::DISPATCH_COUNT), Some(1.0));
        assert_eq!(bridge.telemetry().get(tk::PROTOCOL_ID), Some(99.0));
    }

    #[test]
    fn malformed_json_payload_returns_err_and_increments_parse_error_count() {
        let mut bridge = ProtocolBridge::new(minimal_config());
        let env = empty_env();
        bridge.init(&minimal_config(), &env).unwrap();

        let signal = ControlSignal::ProtocolNative {
            protocol: ProtocolId(17),
            payload: b"not valid json".to_vec(),
        };
        let result = bridge.apply_control(&signal);
        assert!(
            result.is_err(),
            "malformed payload must propagate parse error to caller"
        );
        // Dispatch NOT counted — the entire apply_control returned Err.
        // parse_error_count reflects the failed parse attempt.
        assert_eq!(bridge.parse_error_count, 1);
    }
}
