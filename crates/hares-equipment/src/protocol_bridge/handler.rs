//! Protocol-specific handler trait and built-in implementations.
//!
//! Each `ProtocolHandler` converts a raw binary payload from a specific
//! protocol into typed `EquipmentCommand`s that the dwelling dispatch loop
//! routes to target equipment within the same timestep.
//!
//! Handlers are registered with a `ProtocolBridge` at configuration time.
//! When a `ProtocolNative` signal arrives, the bridge looks up the handler
//! matching `signal.protocol` and delegates parsing.

use hares_types::{ControlSignal, ProtocolId};
use serde::Deserialize;

/// A command parsed from a protocol payload, ready for dispatch.
///
/// Each `EquipmentCommand` names a target equipment instance and carries a
/// typed `ControlSignal` to apply to that equipment. The dwelling loop
/// collects these via `drain_command_signals()` and dispatches them
/// through the standard `ControlDispatcher` — preserving priority ordering
/// and capability gating.
#[derive(Clone, Debug, PartialEq)]
pub struct EquipmentCommand {
    pub target_name: String,
    pub signal: ControlSignal,
}

impl EquipmentCommand {
    #[must_use]
    pub fn new(target_name: impl Into<String>, signal: ControlSignal) -> Self {
        Self {
            target_name: target_name.into(),
            signal,
        }
    }
}

/// Protocol-specific handler that parses raw binary payloads into
/// typed equipment commands.
///
/// Implementations must be `Send + Sync` so they can be shared across
/// threads in fleet-mode simulations. The `Debug` bound provides usable
/// diagnostics when handler lookup fails.
pub trait ProtocolHandler: Send + Sync + std::fmt::Debug {
    /// The protocol ID this handler accepts. The bridge uses this to
    /// match incoming `ProtocolNative { protocol, .. }` signals to
    /// the correct handler.
    fn protocol_id(&self) -> ProtocolId;

    /// Parse a raw protocol payload into zero or more equipment commands.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the payload is malformed or cannot be interpreted
    /// under this protocol. Failed parses are recorded in telemetry but
    /// do not abort the simulation — the bridge rejects the signal and
    /// continues.
    fn parse(&self, payload: &[u8]) -> crate::Result<Vec<EquipmentCommand>>;
}

// ---------------------------------------------------------------------------
// JSON payload handler — "made-up" protocol for testing and demonstration
// ---------------------------------------------------------------------------

/// JSON-serialised `[{"target": "Battery-1", "signal": {"PowerSetpoint": {"active_power_kw": 5.0}}}]`
/// payload format.
#[derive(Deserialize)]
struct JsonCommand {
    target: String,
    signal: serde_json::Value,
}

/// A protocol handler that parses JSON payloads into equipment commands.
///
/// Payload format (UTF-8 JSON):
/// ```json
/// [
///   {
///     "target": "Battery-1",
///     "signal": {
///       "PowerSetpoint": {"active_power_kw": 5.0, "reactive_power_kvar": null}
///     }
///   },
///   {
///     "target": "HVAC-1",
///     "signal": {
///       "ModeOverride": {"mode": "Off"}
///     }
///   }
/// ]
/// ```
///
/// The `"signal"` field must deserialise to a `ControlSignal` variant via
/// serde. Any deserialisation error for an individual command is reported
/// as an `Err` — the bridge rejects the entire payload rather than
/// partially applying commands.
///
/// This handler provides a machine-readable JSON payload format for
/// integration testing and external controller communication. Additional
/// protocol-specific handlers are added by implementing `ProtocolHandler`
/// and registering them in `ProtocolBridgeConfig::handlers`.
#[derive(Debug, Default)]
pub struct JsonHandler {
    protocol_id: ProtocolId,
}

impl JsonHandler {
    #[must_use]
    pub fn new(protocol_id: ProtocolId) -> Self {
        Self { protocol_id }
    }
}

impl ProtocolHandler for JsonHandler {
    fn protocol_id(&self) -> ProtocolId {
        self.protocol_id
    }

    fn parse(&self, payload: &[u8]) -> crate::Result<Vec<EquipmentCommand>> {
        let raw: Vec<JsonCommand> = serde_json::from_slice(payload).map_err(|e| {
            hares_types::HaresError::Control(format!(
                "JSON protocol handler (id={}) failed to parse payload: {e}",
                self.protocol_id.0
            ))
        })?;

        let mut commands = Vec::with_capacity(raw.len());
        for cmd in raw {
            let signal: ControlSignal = serde_json::from_value(cmd.signal).map_err(|e| {
                hares_types::HaresError::Control(format!(
                    "JSON protocol handler (id={}) failed to deserialise signal for target '{}': {e}",
                    self.protocol_id.0, cmd.target
                ))
            })?;
            commands.push(EquipmentCommand {
                target_name: cmd.target,
                signal,
            });
        }
        Ok(commands)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use hares_types::OperatingMode;

    fn handler() -> JsonHandler {
        JsonHandler::new(ProtocolId(1))
    }

    #[test]
    fn parses_power_setpoint_from_json() {
        let payload = br#"[
            {"target": "Battery-1", "signal": {"PowerSetpoint": {"active_power_kw": 5.0, "reactive_power_kvar": null}}}
        ]"#;

        let commands = handler().parse(payload).unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].target_name, "Battery-1");
        assert_eq!(
            commands[0].signal,
            ControlSignal::PowerSetpoint {
                active_power_kw: 5.0,
                reactive_power_kvar: None,
            }
        );
    }

    #[test]
    fn parses_mode_override_from_json() {
        let payload = br#"[
            {"target": "HVAC-1", "signal": {"ModeOverride": {"mode": "Off"}}}
        ]"#;

        let commands = handler().parse(payload).unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].target_name, "HVAC-1");
        assert_eq!(
            commands[0].signal,
            ControlSignal::ModeOverride {
                mode: OperatingMode::Off,
            }
        );
    }

    #[test]
    fn parses_multiple_commands_from_json() {
        let payload = br#"[
            {"target": "Battery-1", "signal": {"PowerSetpoint": {"active_power_kw": 3.0, "reactive_power_kvar": null}}},
            {"target": "HVAC-1", "signal": {"ThermalSetpoint": {"heating_setpoint_c": 20.0, "cooling_setpoint_c": 24.0, "deadband_c": null}}}
        ]"#;

        let commands = handler().parse(payload).unwrap();
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0].target_name, "Battery-1");
        assert_eq!(commands[1].target_name, "HVAC-1");

        assert_eq!(
            commands[0].signal,
            ControlSignal::PowerSetpoint {
                active_power_kw: 3.0,
                reactive_power_kvar: None,
            }
        );
        assert_eq!(
            commands[1].signal,
            ControlSignal::ThermalSetpoint {
                heating_setpoint_c: Some(20.0),
                cooling_setpoint_c: Some(24.0),
                deadband_c: None,
            }
        );
    }

    #[test]
    fn empty_payload_returns_empty_vec() {
        let payload = b"[]";
        let commands = handler().parse(payload).unwrap();
        assert!(commands.is_empty());
    }

    #[test]
    fn invalid_json_returns_err() {
        let payload = b"not json";
        let result = handler().parse(payload);
        assert!(result.is_err());
    }

    #[test]
    fn unknown_signal_variant_returns_err() {
        let payload = br#"[
            {"target": "Bad", "signal": {"Nonexistent": {}}}
        ]"#;
        let result = handler().parse(payload);
        assert!(result.is_err());
    }

    #[test]
    fn handler_reports_protocol_id() {
        let h = JsonHandler::new(ProtocolId(7));
        assert_eq!(h.protocol_id().0, 7);
    }
}
