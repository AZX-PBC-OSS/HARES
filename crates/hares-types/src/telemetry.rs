//! Shared telemetry map type used by equipment and external integrations.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[cfg(feature = "observe")]
use std::sync::atomic::AtomicU64;

/// Equipment telemetry payload represented as scalar channels.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Telemetry(pub HashMap<String, f64>);

/// Global counter tracking the number of non-finite value rejections
/// across all Telemetry instances since process start. Accessible
/// via [`Telemetry::non_finite_rejection_count`] when the `observe`
/// feature is enabled — callers can poll this for diagnostic telemetry
/// about which equipment is producing corrupted values.
#[cfg(feature = "observe")]
static NON_FINITE_REJECTIONS: AtomicU64 = AtomicU64::new(0);

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
        let key = key.into();
        if !value.is_finite() {
            #[cfg(any(debug_assertions, feature = "check_invariants"))]
            {
                panic!("Telemetry::insert called with non-finite value {value} for key '{key}'");
            }
            #[cfg(not(any(debug_assertions, feature = "check_invariants")))]
            {
                tracing::error!(
                    key = %key,
                    value,
                    "Telemetry::insert rejected non-finite value"
                );
                #[cfg(feature = "observe")]
                {
                    NON_FINITE_REJECTIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    tracing::warn!(
                        key = %key,
                        value,
                        "telemetry.non_finite_insert_rejected"
                    );
                }
                return self.0.get(&key).copied();
            }
        }
        self.0.insert(key, value)
    }

    /// Update an existing key's value without allocating.
    ///
    /// All keys must be pre-populated via `insert()` at init time.
    /// Missing keys are a logic error: under `debug_assertions` or the
    /// `check_invariants` feature this panics; in plain release builds it
    /// emits a `tracing::error!` and the write is silently dropped (no-op).
    #[inline]
    pub fn set(&mut self, key: &str, value: f64) {
        if !value.is_finite() {
            #[cfg(any(debug_assertions, feature = "check_invariants"))]
            {
                panic!("Telemetry::set called with non-finite value {value} for key '{key}'");
            }
            #[cfg(not(any(debug_assertions, feature = "check_invariants")))]
            {
                tracing::error!(
                    key = %key,
                    value,
                    "Telemetry::set rejected non-finite value"
                );
                #[cfg(feature = "observe")]
                {
                    NON_FINITE_REJECTIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    tracing::warn!(
                        key = %key,
                        value,
                        "telemetry.non_finite_set_rejected"
                    );
                }
                return;
            }
        }
        if let Some(v) = self.0.get_mut(key) {
            *v = value;
        } else {
            #[cfg(any(debug_assertions, feature = "check_invariants"))]
            {
                panic!(
                    "Telemetry::set called with unknown key '{key}'; pre-populate via insert() at init"
                );
            }
            #[cfg(not(any(debug_assertions, feature = "check_invariants")))]
            {
                tracing::error!(
                    key = %key,
                    "Telemetry::set called with unknown key; pre-populate via insert() at init"
                );
            }
        }
    }

    #[inline]
    #[must_use]
    pub fn get(&self, key: &str) -> Option<f64> {
        let value = self.0.get(key).copied();
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        if let Some(v) = value {
            assert!(
                v.is_finite(),
                "Telemetry::get found non-finite value {v} for key '{key}'"
            );
        }
        value
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Returns the global count of non-finite value rejections since
    /// process start. Only available when the `observe` feature is
    /// enabled — callers use this to diagnose which equipment or
    /// actor is producing corrupted telemetry without relying on
    /// tracing log capture.
    #[cfg(feature = "observe")]
    #[must_use]
    pub fn non_finite_rejection_count() -> u64 {
        NON_FINITE_REJECTIONS.load(std::sync::atomic::Ordering::Relaxed)
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
    fn set_panics_on_unregistered_key() {
        let mut t = Telemetry::new();
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                t.set("nonexistent", 1.0);
            }));
            assert!(
                result.is_err(),
                "set on unregistered key must panic in debug/check_invariants"
            );
        }
        #[cfg(not(any(debug_assertions, feature = "check_invariants")))]
        {
            // In plain release, set on unknown key is a silent no-op with error log.
            t.set("nonexistent", 1.0);
        }
    }

    #[test]
    fn telemetry_insert_rejects_nan() {
        let mut t = Telemetry::new();
        // When check_invariants or debug_assertions is active, insert panics.
        // In plain release builds it silently rejects (logs an error, returns
        // the existing value). Both paths reject — the cfg selects the
        // verification strategy.
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                t.insert("test", f64::NAN);
            }));
            assert!(result.is_err(), "insert of NaN must panic");
        }
        #[cfg(not(any(debug_assertions, feature = "check_invariants")))]
        {
            let result = t.insert("test", f64::NAN);
            assert!(result.is_none(), "insert of NaN must not modify the map");
        }
    }

    #[test]
    fn telemetry_insert_rejects_pos_infinity() {
        let mut t = Telemetry::new();
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                t.insert("test", f64::INFINITY);
            }));
            assert!(result.is_err(), "insert of +Infinity must panic");
        }
        #[cfg(not(any(debug_assertions, feature = "check_invariants")))]
        {
            let result = t.insert("test", f64::INFINITY);
            assert!(
                result.is_none(),
                "insert of +Infinity must not modify the map"
            );
        }
    }

    #[test]
    fn telemetry_insert_rejects_neg_infinity() {
        let mut t = Telemetry::new();
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                t.insert("test", f64::NEG_INFINITY);
            }));
            assert!(result.is_err(), "insert of -Infinity must panic");
        }
        #[cfg(not(any(debug_assertions, feature = "check_invariants")))]
        {
            let result = t.insert("test", f64::NEG_INFINITY);
            assert!(
                result.is_none(),
                "insert of -Infinity must not modify the map"
            );
        }
    }

    #[test]
    fn telemetry_insert_accepts_finite() {
        let mut t = Telemetry::new();
        let prev = t.insert("test", 42.0);
        assert!(prev.is_none());
        assert_eq!(t.get("test"), Some(42.0));
    }

    #[test]
    fn telemetry_set_rejects_non_finite() {
        let mut t = Telemetry::new();
        t.insert("v", 1.0);
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                t.set("v", f64::NAN);
            }));
            assert!(result.is_err(), "set of NaN must panic");
        }
        #[cfg(not(any(debug_assertions, feature = "check_invariants")))]
        {
            t.set("v", f64::NAN);
            assert_eq!(
                t.get("v"),
                Some(1.0),
                "set of NaN must not modify existing value"
            );
        }
    }

    #[test]
    fn telemetry_get_returns_finite() {
        let mut t = Telemetry::new();
        t.insert("power", 3.2);
        t.insert("soc", 0.8);
        assert_eq!(t.get("power"), Some(3.2));
        assert_eq!(t.get("soc"), Some(0.8));
        assert!(t.get("nonexistent").is_none());
    }

    #[cfg(all(
        feature = "observe",
        not(any(debug_assertions, feature = "check_invariants"))
    ))]
    #[test]
    fn observe_rejection_counter_increments() {
        let before = Telemetry::non_finite_rejection_count();
        let mut t = Telemetry::new();
        // In plain release (no check_invariants), insert/set silently reject
        // non-finite values and the observe counter increments.
        let _ = t.insert("k", f64::NAN);
        t.set("k", f64::INFINITY);
        let after = Telemetry::non_finite_rejection_count();
        assert!(
            after >= before + 2,
            "observe counter must increment by 2 (insert + set), got {before} → {after}"
        );
    }
}
