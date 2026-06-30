//! Shared telemetry map type used by equipment and external integrations.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Equipment telemetry payload represented as scalar channels.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Telemetry(pub HashMap<String, f64>);

impl Telemetry {
    #[must_use]
    pub fn new() -> Self {
        Self(HashMap::new())
    }

    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self(HashMap::with_capacity(capacity))
    }

    pub fn insert(&mut self, key: impl Into<String>, value: f64) -> Option<f64> {
        self.0.insert(key.into(), value)
    }

    /// Update an existing key's value without allocating.
    ///
    /// All keys must be pre-populated via `insert()` at init time.
    /// Missing keys are a logic error: in debug/test builds this triggers a
    /// `debug_assert!` panic; in release builds it emits a `tracing::error!`
    /// and the write is silently dropped (no-op).
    #[inline]
    pub fn set(&mut self, key: &str, value: f64) {
        if let Some(v) = self.0.get_mut(key) {
            *v = value;
        } else {
            debug_assert!(
                false,
                "Telemetry::set called with unknown key '{key}'; pre-populate via insert() at init"
            );
            tracing::error!(
                key = %key,
                "Telemetry::set called with unknown key; pre-populate via insert() at init"
            );
        }
    }

    #[inline]
    #[must_use]
    pub fn get(&self, key: &str) -> Option<f64> {
        self.0.get(key).copied()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &f64)> {
        self.0.iter()
    }
}

impl<'a> IntoIterator for &'a Telemetry {
    type Item = (&'a String, &'a f64);
    type IntoIter = std::collections::hash_map::Iter<'a, String, f64>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl FromIterator<(String, f64)> for Telemetry {
    fn from_iter<T: IntoIterator<Item = (String, f64)>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::Telemetry;

    #[test]
    fn telemetry_from_iter_and_accessors_work() {
        let telemetry = Telemetry::from_iter(vec![
            ("power_kw".to_string(), 3.2),
            ("soc".to_string(), 0.8),
        ]);
        assert_eq!(telemetry.get("power_kw"), Some(3.2));
        assert_eq!(telemetry.get("soc"), Some(0.8));
        assert_eq!(telemetry.len(), 2);
    }

    #[test]
    fn set_updates_registered_key() {
        let mut t = Telemetry::new();
        t.insert("v", 1.0);
        t.set("v", 42.0);
        assert_eq!(t.get("v"), Some(42.0));
    }

    #[test]
    #[should_panic(expected = "Telemetry::set called with unknown key")]
    fn set_panics_on_unregistered_key_in_debug() {
        let mut t = Telemetry::new();
        t.set("nonexistent", 1.0);
    }
}
