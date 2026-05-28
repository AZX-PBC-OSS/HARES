//! Typed configuration for the protocol bridge.
//!
//! The protocol bridge receives `ProtocolNative` control signals and routes
//! their payloads through registered protocol handlers. Parsed commands are
//! collected by the dwelling dispatch loop and routed to target equipment
//! within the same timestep.
//!
//! Built-in handler: `Json` — parses UTF-8 JSON payloads of the form
//! `[{"target": "...", "signal": {...}}]` via serde. Additional handler
//! variants (Modbus register maps, SunSpec point models, EEBUS SHIP
//! function clusters) are added here as new `HandlerConfig` enum variants.

use serde::{Deserialize, Serialize};

use crate::config::EquipmentTypedConfig;

/// Declares a handler that the bridge should instantiate at init time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HandlerConfig {
    /// JSON payload handler. Accepts protocol ID `protocol_id` and parses
    /// UTF-8 JSON payloads of the form
    /// `[{"target": "...", "signal": {...}}]`.
    Json { protocol_id: u16 },
}

impl HandlerConfig {
    /// The protocol ID this handler configuration will serve.
    #[must_use]
    pub fn protocol_id(&self) -> u16 {
        match self {
            Self::Json { protocol_id } => *protocol_id,
        }
    }
}

/// Typed configuration for the protocol bridge equipment.
///
/// `registered_protocols` is the set of `ProtocolId` values this bridge
/// recognises. An unrecognised protocol produces a warning but is still
/// recorded in telemetry so downstream observers can detect unexpected
/// protocol traffic.
///
/// `handlers` are the protocol-specific parsers instantiated at init time.
/// Each handler's `protocol_id` maps to the matching `ProtocolNative`
/// variant field. Multiple handlers can serve different protocol IDs
/// attached to the same bridge instance.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolBridgeConfig {
    pub equipment_id: Option<u32>,
    /// `ProtocolId` values registered with this bridge.
    /// Omitting this field or providing an empty vec means the bridge
    /// accepts all protocol IDs (no filtering).
    #[serde(default)]
    pub registered_protocols: Vec<u16>,
    /// Protocol-specific handler configurations. Each entry is
    /// instantiated into a runtime parser at `init()` time.
    #[serde(default)]
    pub handlers: Vec<HandlerConfig>,
}

impl EquipmentTypedConfig for ProtocolBridgeConfig {
    fn equipment_type_name() -> &'static str {
        "ProtocolBridge"
    }
}

impl ProtocolBridgeConfig {
    /// Validate the config for physical plausibility.
    pub fn validate(&self) -> crate::Result<()> {
        Ok(())
    }

    /// Returns true if `protocol_id` is registered (or the registry is empty,
    /// meaning accept-all).
    #[must_use]
    pub fn is_protocol_registered(&self, protocol_id: u16) -> bool {
        self.registered_protocols.is_empty() || self.registered_protocols.contains(&protocol_id)
    }
}
