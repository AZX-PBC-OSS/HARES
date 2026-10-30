//! Shared telemetry map type used by equipment and external integrations.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Equipment telemetry payload represented as scalar channels.
///
/// The second field is the non-finite latch: the `(key, value)` of the first
/// non-finite write this map saw. Non-finite writes are rejected uniformly in
/// every build profile (reject, latch, log, continue) so the behavior never
/// depends on the build; the value is latched but deliberately never stored in
/// the map.
///
/// The third field is the unknown-key latch: the name of the first key a
/// `set` call saw that was never pre-populated via `insert()`. Like the
/// non-finite latch it is a wiring fault recorded for the step-end check.
///
/// The fourth field marks a map built (and sealed) from typed declarations
/// via [`Telemetry::from_fields`]: a sealed map rejects `insert` of a new
/// key exactly like a `set` of an unregistered one, latching the key so the
/// owner's next step-end check fails the run. The seal is not serialised:
/// a restored map's keys are the declarations' anyway.
///
/// The dwelling turns a latched telemetry into a run failure (a
/// `HaresError` naming the owner and the key); that dwelling check is the
/// only enforcement, and it is always on. The latches join the derived
/// traits naturally: a latched map differs from a clean one, and that is
/// correct. The unknown-key latch carries no simulation state, so it is not
/// serialized.
///
/// `Clone` is hand-written: the derived `clone_from` runs `*self =
/// source.clone()`, which reallocates the map and every key, so a
/// per-step snapshot through `clone_from` would allocate every step.
/// The hand-written `clone_from` overwrites the values in place when
/// both maps hold the same key set (the steady state: every key is
/// registered at init) and falls back to `HashMap::clone_from`
/// otherwise.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Telemetry(
    pub HashMap<String, f64>,
    Option<(String, f64)>,
    #[serde(skip)] Option<String>,
    #[serde(skip)] Option<std::collections::HashSet<String>>,
);

impl Clone for Telemetry {
    fn clone(&self) -> Self {
        Self(
            self.0.clone(),
            self.1.clone(),
            self.2.clone(),
            self.3.clone(),
        )
    }

    /// Copies `source` over `self` in place. When both maps have the same
    /// length and every source key is present in `self` (the steady
    /// state), only the `f64` values are overwritten and nothing
    /// allocates; otherwise `HashMap::clone_from` reuses the table's
    /// capacity where it can. The latches copy through `Option::clone_from`
    /// in both cases, which allocates nothing when both sides agree (the
    /// steady state): a latched source always yields a latched copy, and a
    /// clean source clears a latched destination's latches.
    fn clone_from(&mut self, source: &Self) {
        if self.0.len() == source.0.len() && source.0.keys().all(|key| self.0.contains_key(key)) {
            for (key, value) in &source.0 {
                if let Some(slot) = self.0.get_mut(key) {
                    *slot = *value;
                }
            }
        } else {
            self.0.clone_from(&source.0);
        }
        self.1.clone_from(&source.1);
        self.2.clone_from(&source.2);
        self.3.clone_from(&source.3);
    }
}

impl Telemetry {
    #[must_use]
    pub fn new() -> Self {
        Self(HashMap::new(), None, None, None)
    }

    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self(HashMap::with_capacity(capacity), None, None, None)
    }

    /// Builds a sealed telemetry map from typed declarations: exactly the
    /// declared keys are registered, and the map is sealed against
    /// `insert` of any key the declarations omit (an undeclared key latches
    /// like a `set` of one, failing the owner's step-end check).
    ///
    /// Every built-in and test actor builds its telemetry this way, so an
    /// actor's observable channels are exactly its declarations.
    #[must_use]
    pub fn from_fields(fields: &[crate::output::TelemetryField]) -> Self {
        let mut map = HashMap::with_capacity(fields.len());
        let mut sealed = std::collections::HashSet::with_capacity(fields.len());
        for field in fields {
            let key = field.name.to_string();
            map.insert(key.clone(), 0.0);
            sealed.insert(key);
        }
        Self(map, None, None, Some(sealed))
    }

    /// Whether the map was built from typed declarations and is sealed
    /// against `insert` of new keys.
    #[must_use]
    pub fn is_sealed(&self) -> bool {
        self.3.is_some()
    }

    /// The sealed key set, when the map is sealed.
    #[must_use]
    pub fn sealed_keys(&self) -> Option<&std::collections::HashSet<String>> {
        self.3.as_ref()
    }

    pub fn insert(&mut self, key: impl Into<String>, value: f64) -> Option<f64> {
        let key = key.into();
        if !value.is_finite() {
            // The first bad write is the diagnosis: never overwrite an
            // existing latch with a later rejection.
            if self.1.is_none() {
                self.1 = Some((key.clone(), value));
            }
            tracing::error!(
                key = %key,
                value,
                "Telemetry::insert rejected non-finite value"
            );
            return self.0.get(&key).copied();
        }
        if let Some(sealed) = &self.3
            && !sealed.contains(&key)
        {
            // A sealed map rejects a new key exactly like `set` does:
            // the write is dropped and the first unknown key latches.
            if self.2.is_none() {
                self.2 = Some(key.clone());
            }
            tracing::error!(
                key = %key,
                "Telemetry::insert called with undeclared key on a sealed \
                 map; declare the key in the owner's output_variables"
            );
            return self.0.get(&key).copied();
        }
        self.0.insert(key, value)
    }

    /// Write a channel the owner registers lazily: the value updates the
    /// registered slot in place (never allocating), and an unregistered
    /// key registers through [`Self::insert`], whose finite-rejection and
    /// sealed-map rules apply to the registration exactly as they do to
    /// the by-value insert. A key registered this way keeps the
    /// registered-from-then-on timeline its owner's contract states (a
    /// lazily written channel's output cell reads null until its first
    /// write), and the steady state pays nothing for the write: the
    /// fresh `String` key's allocation is the registration's, paid once
    /// per run, not the write's, paid per step.
    ///
    /// # Errors
    ///
    /// Never: a rejected write (non-finite value, sealed map) behaves
    /// exactly as [`Self::insert`]'s rejection does.
    pub fn set_or_register(&mut self, key: &str, value: f64) {
        if value.is_finite() && self.0.contains_key(key) {
            if let Some(slot) = self.0.get_mut(key) {
                *slot = value;
            }
            return;
        }
        self.insert(key.to_string(), value);
    }

    /// Update an existing key's value without allocating.
    ///
    /// All keys must be pre-populated via `insert()` at init time.
    /// Missing keys are a logic error: the write is dropped, the first
    /// unknown key is latched (see [`Self::unknown_key_latch`]), and an
    /// error is logged. The dwelling turns a latched unknown key into a
    /// run failure naming the equipment and the key; that check is
    /// unconditional.
    ///
    /// Non-finite values are rejected uniformly in every build profile: the
    /// write is dropped, the map is untouched, the first rejection is
    /// latched (see [`Self::non_finite_latch`]), and an error is logged.
    #[inline]
    pub fn set(&mut self, key: &str, value: f64) {
        if !value.is_finite() {
            if self.1.is_none() {
                self.1 = Some((key.to_string(), value));
            }
            tracing::error!(
                key = %key,
                value,
                "Telemetry::set rejected non-finite value"
            );
            return;
        }
        if let Some(v) = self.0.get_mut(key) {
            *v = value;
        } else {
            // The first bad write is the diagnosis: never overwrite an
            // existing latch with a later rejection.
            if self.2.is_none() {
                self.2 = Some(key.to_string());
            }
            tracing::error!(
                key = %key,
                "Telemetry::set called with unknown key; pre-populate via insert() at init"
            );
        }
    }

    /// Withdraws a channel's value for the current step: `get` reports
    /// `None` until the next write. The key stays registered: the slot
    /// holds a NaN payload only `remove` can store (every write rejects
    /// non-finite values and stores nothing it rejects), so the next `set`
    /// takes the value back without latching, and the map's key set never
    /// changes (the snapshot's in-place copy keeps its fast path). This is
    /// how a producer publishes the absence of a quantity it owns (an
    /// actor key it did not emit this step): the output cell reads null,
    /// never a substituted sentinel. A key never registered is logged and
    /// ignored.
    pub fn remove(&mut self, key: &str) {
        if let Some(slot) = self.0.get_mut(key) {
            *slot = f64::NAN;
        } else {
            tracing::debug!(key = %key, "Telemetry::remove of an unregistered key");
        }
    }

    /// Withdraws a channel's registration: the key leaves the map, so
    /// `get` reports `None` and the key set no longer contains it. This is
    /// the restore path's write for a channel whose registration is itself
    /// state (a lazily registered channel whose saved map omitted the key):
    /// a checkpoint replaces the whole telemetry timeline, so the restored
    /// map is the saved map's key set, never the union with the pre-restore
    /// run's. A key not registered is a no-op.
    ///
    /// Intended for lazily registered channels on a map built without
    /// declarations; a sealed map's declared keys are its contract, since a
    /// later write to a removed declared key would latch as undeclared.
    pub fn unregister(&mut self, key: &str) {
        self.0.remove(key);
    }

    #[inline]
    #[must_use]
    pub fn get(&self, key: &str) -> Option<f64> {
        match self.0.get(key).copied() {
            // The withdrawn marker: the channel's value is absent this
            // step.
            Some(value) if value.is_nan() => None,
            Some(value) => {
                #[cfg(debug_assertions)]
                assert!(
                    value.is_finite(),
                    "Telemetry::get found non-finite value {value} for key '{key}'"
                );
                Some(value)
            }
            None => None,
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The first non-finite write this map saw, as `(key, value)`.
    ///
    /// The dwelling fails the step with `HaresError::NanDetected` naming the
    /// owner and the key. The first bad write is the diagnosis: later
    /// rejections never overwrite an existing latch, and the latched value is
    /// deliberately not stored in the map. `clear()` resets the latch: a
    /// cleared map is a clean map.
    #[must_use]
    pub fn non_finite_latch(&self) -> Option<&(String, f64)> {
        self.1.as_ref()
    }

    /// The first unknown key a `set` call saw, if any.
    ///
    /// The dwelling fails the step naming the equipment and the key. The
    /// first bad write is the diagnosis: later rejections never overwrite an
    /// existing latch. `clear()` resets the latch: a cleared map is a clean
    /// map.
    #[must_use]
    pub fn unknown_key_latch(&self) -> Option<&str> {
        self.2.as_deref()
    }

    pub fn clear(&mut self) {
        self.0.clear();
        self.1 = None;
        self.2 = None;
        self.3 = None;
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &f64)> {
        self.0.iter().filter(|(_, value)| !value.is_nan())
    }
}

/// The latched `(key, value)` pair's equality: the latched value may be
/// the non-finite write itself, and NaN is not `==` NaN, so the pair
/// compares by the key and by both-values-NaN or exact equality.
fn latch_eq(a: &Option<(String, f64)>, b: &Option<(String, f64)>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some((key_a, value_a)), Some((key_b, value_b))) => {
            key_a == key_b && (value_a == value_b || (value_a.is_nan() && value_b.is_nan()))
        }
        _ => false,
    }
}

/// Equality reads through the withdrawn marker: two maps agree when their
/// key sets agree and every channel's value agrees, a withdrawn channel
/// (the NaN payload) equal to another withdrawn one.
impl PartialEq for Telemetry {
    fn eq(&self, other: &Self) -> bool {
        self.0.len() == other.0.len()
            && self.0.iter().all(|(key, value)| {
                other.0.get(key).is_some_and(|other_value| {
                    *value == *other_value || (value.is_nan() && other_value.is_nan())
                })
            })
            && latch_eq(&self.1, &other.1)
            && self.2 == other.2
            && self.3 == other.3
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
        Self(iter.into_iter().collect(), None, None, None)
    }
}

#[cfg(test)]
mod tests {
    use super::Telemetry;
    use crate::output::TelemetryField;

    #[test]
    fn clone_impl_covers_every_field() {
        // Destructuring pins the field count: adding a field fails to
        // compile here, so the hand-written `Clone` cannot silently miss
        // it.
        let telemetry = Telemetry::new();
        let Telemetry(_, _, _, _) = telemetry;
    }

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
    fn unregister_drops_the_key_and_its_slot() {
        // A lazily registered channel's registration is its timeline: the
        // restore path removes the key when the saved map omitted it, so
        // the key set (not only `get`) must lose it.
        let mut t = Telemetry::new();
        t.insert("kept", 1.0);
        t.insert("lazy", 2.0);
        t.unregister("lazy");
        assert_eq!(t.get("lazy"), None);
        assert!(!t.0.contains_key("lazy"));
        assert_eq!(t.len(), 1);
        assert_eq!(t.get("kept"), Some(1.0));
        // An absent key is a no-op, not a latch: unregistering a channel
        // the map never held is the "still never registered" state.
        t.unregister("never");
        assert_eq!(t.len(), 1);
        assert_eq!(t.unknown_key_latch(), None);
    }

    #[test]
    fn set_unknown_key_is_latched() {
        let mut t = Telemetry::new();
        t.set("nonexistent", 1.0);
        assert_eq!(
            t.get("nonexistent"),
            None,
            "a set on an unregistered key must not add the key"
        );
        assert_eq!(
            t.unknown_key_latch(),
            Some("nonexistent"),
            "a set on an unregistered key must latch"
        );
    }

    #[test]
    fn unknown_key_latch_keeps_the_first_rejection() {
        let mut t = Telemetry::new();
        t.set("first", 1.0);
        t.set("second", 1.0);
        assert_eq!(
            t.unknown_key_latch(),
            Some("first"),
            "the first bad write is the diagnosis; later rejections must not \
             overwrite the latch"
        );
    }

    #[test]
    fn telemetry_insert_rejects_nan_and_latches() {
        let mut t = Telemetry::new();
        let prev = t.insert("test", f64::NAN);
        assert!(prev.is_none(), "insert of NaN must not modify the map");
        assert!(t.get("test").is_none(), "NaN must not enter the map");
        let latch = t
            .non_finite_latch()
            .expect("the first non-finite write must be latched");
        assert_eq!(latch.0, "test");
        assert!(latch.1.is_nan());
    }

    #[test]
    fn telemetry_insert_rejects_pos_infinity_and_latches() {
        let mut t = Telemetry::new();
        let result = t.insert("test", f64::INFINITY);
        assert!(
            result.is_none(),
            "insert of +Infinity must not modify the map"
        );
        assert_eq!(
            t.non_finite_latch(),
            Some(&("test".to_string(), f64::INFINITY))
        );
    }

    #[test]
    fn telemetry_insert_rejects_neg_infinity_and_latches() {
        let mut t = Telemetry::new();
        let result = t.insert("test", f64::NEG_INFINITY);
        assert!(
            result.is_none(),
            "insert of -Infinity must not modify the map"
        );
        assert_eq!(
            t.non_finite_latch(),
            Some(&("test".to_string(), f64::NEG_INFINITY))
        );
    }

    #[test]
    fn telemetry_insert_accepts_finite() {
        let mut t = Telemetry::new();
        let prev = t.insert("test", 42.0);
        assert!(prev.is_none());
        assert_eq!(t.get("test"), Some(42.0));
    }

    #[test]
    fn telemetry_set_rejects_non_finite_and_latches() {
        let mut t = Telemetry::new();
        t.insert("v", 1.0);
        t.set("v", f64::NAN);
        assert_eq!(
            t.get("v"),
            Some(1.0),
            "set of NaN must not modify existing value"
        );
        let latch = t.non_finite_latch().expect("a rejected set must latch");
        assert_eq!(latch.0, "v");
        assert!(latch.1.is_nan());
    }

    #[test]
    fn telemetry_latch_keeps_the_first_rejection() {
        let mut t = Telemetry::new();
        let _ = t.insert("first", f64::NAN);
        let _ = t.insert("second", f64::INFINITY);
        t.set("third", f64::NEG_INFINITY);
        let latch = t.non_finite_latch().expect("a rejected write must latch");
        assert_eq!(
            latch.0, "first",
            "the first bad write is the diagnosis; later rejections must not \
             overwrite the latch"
        );
        assert!(latch.1.is_nan());
    }

    #[test]
    fn telemetry_latch_value_is_not_stored_in_the_map() {
        let mut t = Telemetry::new();
        let _ = t.insert("k", f64::NAN);
        assert!(t.get("k").is_none());
        assert_eq!(t.len(), 0, "a rejected write must not add the key");
        assert_eq!(t.iter().count(), 0);
    }

    #[test]
    fn telemetry_clear_clears_the_latch() {
        let mut t = Telemetry::new();
        let _ = t.insert("k", f64::NAN);
        t.clear();
        assert!(
            t.non_finite_latch().is_none(),
            "a cleared map is a clean map"
        );
    }

    #[test]
    fn telemetry_latch_participates_in_clone_and_equality() {
        let mut latched = Telemetry::new();
        let _ = latched.insert("k", f64::INFINITY);
        let clean = Telemetry::new();
        assert_ne!(
            latched, clean,
            "a latched map differs from a clean one; the dwelling treats a \
             latched map as a run failure"
        );
        assert_eq!(latched.clone(), latched);
    }

    #[test]
    fn telemetry_json_round_trip_preserves_a_clean_map() {
        let t = Telemetry::from_iter(vec![("power".to_string(), 3.2)]);
        let json = serde_json::to_string(&t).expect("serialize telemetry");
        let decoded: Telemetry = serde_json::from_str(&json).expect("deserialize telemetry");
        assert_eq!(decoded, t);
        assert!(decoded.non_finite_latch().is_none());
    }

    #[test]
    fn telemetry_serialized_shape_carries_the_latch() {
        // The latch is a second tuple-struct field, so a latched map
        // serializes as `[<map>, [key, value-or-null]]` (a latched map
        // differs from a clean one on the wire too). JSON has no Infinity,
        // so serde_json writes the latched value as null; the dwelling
        // reports the latched value through HaresError::NanDetected
        // instead of serializing telemetry.
        let mut t = Telemetry::new();
        let _ = t.insert("power", 3.2);
        t.set("power", f64::INFINITY);
        let value = serde_json::to_value(&t).expect("serialize latched telemetry");
        assert!(
            matches!(
                value,
                serde_json::Value::Array(ref items)
                    if items.len() == 2
                        && items[1].get(0) == Some(&serde_json::Value::String("power".to_string()))
            ),
            "serialized telemetry must be [map, latch], got {value}"
        );
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

    #[test]
    fn from_fields_registers_exactly_the_declared_keys_and_seals() {
        let fields = vec![
            TelemetryField::power_kw("charge_kw", "charging power"),
            TelemetryField::ratio("soc", "state of charge"),
        ];
        let mut t = Telemetry::from_fields(&fields);
        assert!(t.is_sealed());
        assert_eq!(t.len(), 2);
        assert!(t.get("charge_kw").is_some());
        assert!(t.get("soc").is_some());
        // A set of a declared key works.
        t.set("charge_kw", 4.2);
        assert_eq!(t.get("charge_kw"), Some(4.2));
        // An insert of a new key on a sealed map latches like an unknown set.
        t.insert("undeclared", 1.0);
        assert_eq!(t.unknown_key_latch(), Some("undeclared"));
        assert!(t.get("undeclared").is_none());
    }
}
