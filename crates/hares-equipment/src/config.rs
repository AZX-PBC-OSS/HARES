//! Equipment configuration and parameter types.

/// Common config key for equipment ID, shared across all equipment types.
pub(crate) const KEY_EQUIPMENT_ID: &str = "equipment_id";
/// Common config key for zone ID, shared across equipment types that are zone-attached.
pub(crate) const KEY_ZONE_ID: &str = "zone_id";

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Flexible config value supporting numeric, string, and boolean parameters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ConfigValue {
    Float(f64),
    Text(String),
    Bool(bool),
    FloatArray(Vec<f64>),
}

impl fmt::Display for ConfigValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Float(v) => write!(f, "{v}"),
            Self::Text(v) => write!(f, "{v}"),
            Self::Bool(v) => write!(f, "{v}"),
            Self::FloatArray(v) => write!(
                f,
                "{}",
                v.iter()
                    .map(|x| x.to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        }
    }
}

impl From<f64> for ConfigValue {
    fn from(v: f64) -> Self {
        Self::Float(v)
    }
}

impl From<String> for ConfigValue {
    fn from(v: String) -> Self {
        Self::Text(v)
    }
}

impl From<&str> for ConfigValue {
    fn from(v: &str) -> Self {
        Self::Text(v.to_string())
    }
}

impl From<bool> for ConfigValue {
    fn from(v: bool) -> Self {
        Self::Bool(v)
    }
}

impl From<Vec<f64>> for ConfigValue {
    fn from(v: Vec<f64>) -> Self {
        Self::FloatArray(v)
    }
}

impl ConfigValue {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Float(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Text(v) => Some(v.as_str()),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_f64_array(&self) -> Option<&[f64]> {
        match self {
            Self::FloatArray(v) => Some(v),
            _ => None,
        }
    }
}

/// Initialization parameters for one equipment instance.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EquipmentConfig {
    pub name: String,
    pub ochre_class: String,
    pub raw_config: HashMap<String, ConfigValue>,
}

impl EquipmentConfig {
    /// Extract a numeric value from raw_config.
    pub fn get_f64(&self, key: &str) -> Option<f64> {
        self.raw_config.get(key).and_then(ConfigValue::as_f64)
    }

    /// Extract a string value from raw_config.
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.raw_config.get(key).and_then(ConfigValue::as_str)
    }

    /// Extract a boolean value from raw_config.
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.raw_config.get(key).and_then(ConfigValue::as_bool)
    }

    /// Extract a float array value from raw_config.
    pub fn get_f64_array(&self, key: &str) -> Option<&[f64]> {
        self.raw_config.get(key).and_then(ConfigValue::as_f64_array)
    }
}
