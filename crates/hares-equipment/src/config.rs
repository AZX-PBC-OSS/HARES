//! Equipment configuration and parameter types.

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Machine-readable record of per-hour setpoint widening performed by
/// HPXML setpoint reconciliation during parsing.  Carried on `EquipmentConfig`
/// so downstream consumers (dashboards, Python introspection, CSV diagnostics)
/// can detect and report that the values they see are not the values the user
/// supplied.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SetpointReconciliation {
    pub day: String,
    pub original_heating_c: [f64; 24],
    pub original_cooling_c: [f64; 24],
    pub adjusted_heating_c: [f64; 24],
    pub adjusted_cooling_c: [f64; 24],
}

/// Common config key for equipment ID in `ConfigPayload::Raw` payloads.
/// Typed configs carry this as a struct field instead.
pub const KEY_EQUIPMENT_ID: &str = "equipment_id";
/// Common config key for zone ID in `ConfigPayload::Raw` payloads.
/// Typed configs carry this as a struct field instead.
pub const KEY_ZONE_ID: &str = "zone_id";

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

/// Marker trait for per-equipment typed config structs.
/// All structs that implement this must derive Serialize + Deserialize
/// and use #[serde(deny_unknown_fields)].
pub trait EquipmentTypedConfig: Serialize + for<'de> Deserialize<'de> + Clone + fmt::Debug {
    /// Equipment canonical name this config belongs to.
    /// Must match the string registered in EquipmentRegistry.
    fn equipment_type_name() -> &'static str;

    /// Schema version for forward compatibility. Default 1.
    fn schema_version() -> u32 {
        1
    }
}

/// Config payload for one equipment instance.
/// Typed variant is used by built-in equipment after migration.
/// Raw variant is used by custom Python equipment and unmigrated built-ins.
///
/// Uses explicit tagging (not #[serde(untagged)]) to avoid ambiguous
/// deserialization between Raw and Typed variants.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum ConfigPayload {
    #[serde(rename = "raw")]
    Raw { data: HashMap<String, ConfigValue> },
    #[serde(rename = "typed")]
    Typed {
        /// Canonical equipment type name -- must match EquipmentTypedConfig::equipment_type_name().
        type_name: String,
        /// Schema version -- must match EquipmentTypedConfig::schema_version().
        version: u32,
        /// The typed config data as a JSON object.
        data: serde_json::Value,
    },
}

impl Default for ConfigPayload {
    fn default() -> Self {
        Self::Raw {
            data: HashMap::new(),
        }
    }
}

/// Initialization parameters for one equipment instance.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EquipmentConfig {
    pub name: String,
    pub ochre_class: String,
    pub payload: ConfigPayload,
    /// HPXML setpoint reconciliation records for this equipment, if any
    /// setpoint hours were widened during HPXML parsing.  `None` means
    /// no reconciliation occurred (all setpoint pairs satisfied the gap).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setpoints_reconciled: Option<Vec<SetpointReconciliation>>,
    /// ZIP/power-factor sidecar for this equipment instance.
    ///
    /// Carried outside the typed payload so that
    /// `#[serde(deny_unknown_fields)]` typed config structs never see it.
    /// Populated from `EquipmentSpec::zip_params` (the
    /// `defaults/zip_parameters.toml` lookup) and from the reserved `"zip"`
    /// override object merged field-wise over that base. `None` means "no
    /// instance-specific ZIP configured"; consumers resolve the effective
    /// value through [`resolve_zip`], which falls back to the class-table
    /// defaults and finally to [`hares_types::zip::ZipLoad::constant_power`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zip: Option<hares_types::zip::ZipLoad>,
    /// Zone-to-role map for auto-routing equipment thermal contributions.
    /// Populated by the dwelling at construction time and injected before
    /// `init()`.  Reconstructed every run from the HPXML-derived building
    /// envelope; not persisted in checkpoints or serialized output.
    /// `None` for synthetic or test configs that do not have a building envelope.
    #[serde(skip, default)]
    pub zone_map: Option<hares_types::ZoneMap>,
    /// Pre-derived RNG seed injected by the dwelling's hierarchical RNG
    /// system at construction time.  When `Some`, stochastic equipment
    /// uses this seed directly instead of hashing `master_seed` /
    /// `building_id` / `name`.  Stream partitioning guarantees that sibling
    /// equipment streams are non-overlapping and deterministic.
    #[serde(skip, default)]
    pub rng_seed: Option<[u8; 32]>,
    /// Zone thermal capacitance [kWh/K] for the equivalent battery model.
    /// Populated by the dwelling from envelope solver zone capacitances at
    /// construction time; 0.0 means EBM is disabled for this equipment.
    #[serde(skip, default)]
    pub zone_capacitance_kwh_per_k: f64,
    #[cfg(test)]
    #[serde(skip, default)]
    test_extras: HashMap<String, ConfigValue>,
}

impl EquipmentConfig {
    pub fn with_payload(name: String, ochre_class: String, payload: ConfigPayload) -> Self {
        Self {
            name,
            ochre_class,
            payload,
            setpoints_reconciled: None,
            zip: None,
            zone_map: None,
            rng_seed: None,
            zone_capacitance_kwh_per_k: 0.0,
            #[cfg(test)]
            test_extras: HashMap::new(),
        }
    }

    /// Attach setpoint reconciliation records extracted from HPXML parsing.
    pub fn with_setpoints_reconciled(
        mut self,
        reconciliations: Option<Vec<SetpointReconciliation>>,
    ) -> Self {
        self.setpoints_reconciled = reconciliations;
        self
    }

    /// Attach a pre-derived RNG seed for stochastic equipment that should
    /// use the dwelling's hierarchical RNG stream partitioning instead of
    /// the legacy name-based hash.
    pub fn with_rng_seed(mut self, seed: [u8; 32]) -> Self {
        self.rng_seed = Some(seed);
        self
    }

    /// Deserialize a built-in equipment payload, surfacing a clearer error when
    /// a caller accidentally passes a raw payload to a typed-only init path.
    pub fn require_typed<T: EquipmentTypedConfig>(
        &self,
        equipment_label: &str,
    ) -> crate::Result<T> {
        self.typed::<T>().map_err(|err| match &self.payload {
            ConfigPayload::Raw { .. } => hares_types::HaresError::Equipment(format!(
                "{equipment_label} requires typed config (was Raw). \
                 Did you use EquipmentConfig::from_typed()? Error: {err}"
            )),
            ConfigPayload::Typed { .. } => hares_types::HaresError::Equipment(format!(
                "{equipment_label} typed config validation failed: {err}"
            )),
        })
    }

    /// Extract a numeric value from the `Raw` payload.
    pub fn get_f64(&self, key: &str) -> Option<f64> {
        self.raw_data()
            .and_then(|data| data.get(key).and_then(ConfigValue::as_f64))
    }

    /// Extract a string value from the `Raw` payload.
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.raw_data()
            .and_then(|data| data.get(key).and_then(ConfigValue::as_str))
    }

    /// Extract a boolean value from the `Raw` payload.
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.raw_data()
            .and_then(|data| data.get(key).and_then(ConfigValue::as_bool))
    }

    /// Extract a float array value from the `Raw` payload.
    pub fn get_f64_array(&self, key: &str) -> Option<&[f64]> {
        self.raw_data()
            .and_then(|data| data.get(key).and_then(ConfigValue::as_f64_array))
    }

    /// Extract the zone ID from either `Raw` or `Typed` payloads.
    /// Returns `None` when `zone_id` is absent or invalid (0, non-integer, out of range).
    pub fn zone_id(&self) -> Option<hares_types::ZoneId> {
        let raw = self.get_f64(KEY_ZONE_ID).or_else(|| match &self.payload {
            ConfigPayload::Typed { data, .. } => data.get(KEY_ZONE_ID).and_then(|v| v.as_f64()),
            ConfigPayload::Raw { .. } => None,
        })?;
        if raw == 0.0
            || !raw.is_finite()
            || raw.fract() != 0.0
            || raw < 0.0
            || raw > u16::MAX as f64
        {
            return None;
        }
        Some(hares_types::ZoneId(raw as u16))
    }

    /// Test-only mutable extras for fixture tweaks.
    #[cfg(test)]
    pub fn test_extras_mut(&mut self) -> &mut HashMap<String, ConfigValue> {
        match &mut self.payload {
            ConfigPayload::Raw { data } => data,
            ConfigPayload::Typed { .. } => &mut self.test_extras,
        }
    }

    /// Get the raw config data, or None if typed.
    pub fn raw_data(&self) -> Option<&HashMap<String, ConfigValue>> {
        #[cfg(test)]
        if let ConfigPayload::Typed { .. } = &self.payload
            && !self.test_extras.is_empty()
        {
            return Some(&self.test_extras);
        }
        match &self.payload {
            ConfigPayload::Raw { data } => Some(data),
            ConfigPayload::Typed { .. } => None,
        }
    }

    /// Get the raw config data. Panics if called on a `Typed` payload.
    pub fn raw_data_or_empty(&self) -> &HashMap<String, ConfigValue> {
        #[cfg(test)]
        if let ConfigPayload::Typed { .. } = &self.payload {
            return &self.test_extras;
        }
        match &self.payload {
            ConfigPayload::Raw { data } => data,
            ConfigPayload::Typed { .. } => {
                unreachable!("raw_data_or_empty called on typed config")
            }
        }
    }

    /// Whether the payload is already in typed (JSON object) form.
    pub fn is_typed(&self) -> bool {
        matches!(self.payload, ConfigPayload::Typed { .. })
    }

    /// Deserialize the payload as a typed config struct T.
    /// Returns Err if the payload does not match T's schema.
    pub fn typed<T: EquipmentTypedConfig>(&self) -> crate::Result<T> {
        match &self.payload {
            ConfigPayload::Typed {
                type_name,
                version,
                data,
            } => {
                let expected_type = T::equipment_type_name();
                if type_name != expected_type {
                    return Err(hares_types::HaresError::Equipment(format!(
                        "config type mismatch: expected {expected_type}, got {type_name}"
                    )));
                }
                let expected_version = T::schema_version();
                if *version != expected_version {
                    return Err(hares_types::HaresError::Equipment(format!(
                        "config schema version mismatch: expected {expected_version}, got {version}"
                    )));
                }
                serde_json::from_value(data.clone()).map_err(|e| {
                    hares_types::HaresError::Equipment(format!(
                        "typed config deserialization failed: {e}"
                    ))
                })
            }
            ConfigPayload::Raw { .. } => Err(hares_types::HaresError::Equipment(format!(
                "equipment {} was not initialized with typed config",
                self.name
            ))),
        }
    }

    /// Construct from a typed config struct.
    pub fn from_typed<T: EquipmentTypedConfig>(
        name: String,
        ochre_class: String,
        config: T,
    ) -> crate::Result<Self> {
        let data = serde_json::to_value(config).map_err(|e| {
            hares_types::HaresError::Equipment(format!(
                "typed config serialization failed for {}: {e}",
                T::equipment_type_name()
            ))
        })?;
        Ok(Self {
            name,
            ochre_class,
            payload: ConfigPayload::Typed {
                type_name: T::equipment_type_name().to_string(),
                version: T::schema_version(),
                data,
            },
            setpoints_reconciled: None,
            zip: None,
            zone_map: None,
            rng_seed: None,
            zone_capacitance_kwh_per_k: 0.0,
            #[cfg(test)]
            test_extras: HashMap::new(),
        })
    }

    /// Constructor for Python adapter layer custom equipment.
    /// Built-in equipment uses from_typed() instead.
    pub fn raw(name: String, ochre_class: String, data: HashMap<String, ConfigValue>) -> Self {
        Self {
            name,
            ochre_class,
            payload: ConfigPayload::Raw { data },
            setpoints_reconciled: None,
            zip: None,
            zone_map: None,
            rng_seed: None,
            zone_capacitance_kwh_per_k: 0.0,
            #[cfg(test)]
            test_extras: HashMap::new(),
        }
    }
}

/// Resolve the effective ZIP load model for one equipment instance.
///
/// Precedence, highest first:
/// 1. The [`EquipmentConfig::zip`] sidecar (from `EquipmentSpec::zip_params`
///    plus any `"zip"` override object merged upstream). This is the single
///    canonical channel for instance-specific ZIP for raw and typed
///    equipment alike.
/// 2. Class-table defaults via
///    [`hares_types::zip::zip_defaults_for_class`] on
///    [`EquipmentConfig::ochre_class`].
/// 3. [`hares_types::zip::ZipLoad::constant_power`] (no reactive power,
///    real power untouched).
#[must_use]
pub fn resolve_zip(config: &EquipmentConfig) -> hares_types::zip::ZipLoad {
    config
        .zip
        .or_else(|| hares_types::zip::zip_defaults_for_class(&config.ochre_class))
        .unwrap_or_else(hares_types::zip::ZipLoad::constant_power)
}

/// The ZIP polynomial coefficient sums must equal 1.0 so that the model is
/// a pure redistribution at reference voltage.
pub(crate) const ZIP_SUM_TARGET: f64 = 1.0;
/// Tolerance for the coefficient-sum invariant (floating-point roundoff on
/// literature coefficient sets).
pub(crate) const ZIP_SUM_TOLERANCE: f64 = 1e-9;

/// Resolve the Rule R1 reactive-only ZIP model for typed equipment.
///
/// The effective ZIP is resolved through [`resolve_zip`] (instance sidecar →
/// class-table defaults → constant power), then the real-power side is forced
/// to constant power `(0, 0, 1)`. Typed equipment computes its real electric
/// power through its own physics and must keep it bit-identical at all
/// voltages; only the reactive side of the ZIP model is used, via
/// [`hares_types::zip::ZipLoad::reactive_kvar`] on the already-computed
/// power (`Q = P · tan(acos(pf)) · (zq·V² + iq·V + pq)`).
///
/// Errors when a coefficient-sum invariant is violated (e.g. a bad `"zip"`
/// config override), so misconfiguration fails at init instead of silently
/// skewing power. This is the canonical Rule R1 helper for all typed
/// equipment (HVAC, water heaters, fans, pumps).
pub(crate) fn resolve_reactive_zip(
    config: &EquipmentConfig,
) -> crate::Result<hares_types::zip::ZipLoad> {
    let zip = hares_types::zip::ZipLoad {
        zp: 0.0,
        ip: 0.0,
        pp: 1.0,
        ..resolve_zip(config)
    };
    validate_zip_sums(&zip, &config.name)?;
    Ok(zip)
}

/// Validate the coefficient-sum invariants of a resolved [`ZipLoad`].
///
/// The real-power sum `zp + ip + pp` must always be ≈ 1.0. The reactive sum
/// `zq + iq + pq` must be ≈ 1.0 whenever `pf != 0.0` (with the `pf = 0.0`
/// sentinel the reactive polynomial is never evaluated, so it is not
/// constrained). Called at equipment init so a bad `"zip"` override fails
/// fast with a config error instead of skewing power silently.
pub(crate) fn validate_zip_sums(
    zip: &hares_types::zip::ZipLoad,
    equipment_name: &str,
) -> crate::Result<()> {
    let real_sum = zip.zp + zip.ip + zip.pp;
    if (real_sum - ZIP_SUM_TARGET).abs() > ZIP_SUM_TOLERANCE {
        return Err(hares_types::HaresError::Equipment(format!(
            "{equipment_name}: invalid ZIP coefficients: zp + ip + pp = {real_sum} \
             (zp={}, ip={}, pp={}), expected {ZIP_SUM_TARGET}",
            zip.zp, zip.ip, zip.pp
        )));
    }
    if zip.pf != 0.0 {
        let reactive_sum = zip.zq + zip.iq + zip.pq;
        if (reactive_sum - ZIP_SUM_TARGET).abs() > ZIP_SUM_TOLERANCE {
            return Err(hares_types::HaresError::Equipment(format!(
                "{equipment_name}: invalid reactive ZIP coefficients: \
                 zq + iq + pq = {reactive_sum} (zq={}, iq={}, pq={}), \
                 expected {ZIP_SUM_TARGET}",
                zip.zq, zip.iq, zip.pq
            )));
        }
    }
    Ok(())
}

/// Debug-build twin of [`validate_zip_sums`]: panics with the validation
/// error message when the coefficient-sum invariant is violated at step
/// time. Shared by the full-ZIP consumers (ScheduledLoad, EventBasedLoad,
/// WetAppliance) so the per-step invariant lives in exactly one place;
/// typed equipment validates once at init via [`resolve_reactive_zip`].
#[cfg(any(debug_assertions, feature = "check_invariants"))]
pub(crate) fn debug_assert_zip_sums(
    zip: &hares_types::zip::ZipLoad,
    kind: &str,
    equipment_name: &str,
) {
    if let Err(err) = validate_zip_sums(zip, &format!("{kind} '{equipment_name}'")) {
        panic!("{err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
    #[serde(deny_unknown_fields)]
    struct TestConfig {
        value: f64,
        name: String,
    }

    impl EquipmentTypedConfig for TestConfig {
        fn equipment_type_name() -> &'static str {
            "TestEquipment"
        }
    }

    #[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
    #[serde(deny_unknown_fields)]
    struct OtherConfig {
        count: u32,
    }

    impl EquipmentTypedConfig for OtherConfig {
        fn equipment_type_name() -> &'static str {
            "OtherEquipment"
        }
    }

    #[test]
    fn from_typed_round_trips_correctly() {
        let config = TestConfig {
            value: 42.5,
            name: "test".to_string(),
        };
        let ec = EquipmentConfig::from_typed(
            "test_name".to_string(),
            "TestClass".to_string(),
            config.clone(),
        )
        .unwrap();
        assert!(ec.is_typed());
        let recovered: TestConfig = ec.typed().unwrap();
        assert_eq!(recovered, config);
    }

    #[test]
    fn typed_on_raw_payload_returns_err() {
        let ec = EquipmentConfig::raw("test".to_string(), "Test".to_string(), HashMap::new());
        let result: Result<TestConfig, _> = ec.typed();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("not initialized with typed config")
        );
    }

    #[test]
    fn typed_with_unknown_field_returns_err() {
        let json = serde_json::json!({
            "value": 10.0,
            "name": "test",
            "unknown_field": "should_fail"
        });
        let ec = EquipmentConfig::with_payload(
            "test".to_string(),
            "Test".to_string(),
            ConfigPayload::Typed {
                type_name: "TestEquipment".to_string(),
                version: 1,
                data: json,
            },
        );
        let result: Result<TestConfig, _> = ec.typed();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unknown field"));
    }

    #[test]
    fn typed_with_wrong_type_name_returns_err() {
        let config = TestConfig {
            value: 42.5,
            name: "test".to_string(),
        };
        let ec =
            EquipmentConfig::from_typed("test_name".to_string(), "TestClass".to_string(), config)
                .unwrap();
        let result: Result<OtherConfig, _> = ec.typed();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("config type mismatch"));
    }

    #[test]
    fn typed_with_wrong_version_returns_err() {
        let config = TestConfig {
            value: 42.5,
            name: "test".to_string(),
        };
        let ec = EquipmentConfig::with_payload(
            "test".to_string(),
            "TestClass".to_string(),
            ConfigPayload::Typed {
                type_name: "TestEquipment".to_string(),
                version: 99,
                data: serde_json::to_value(&config).unwrap(),
            },
        );
        let result: Result<TestConfig, _> = ec.typed();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("schema version mismatch"));
    }

    #[test]
    fn raw_accessors_work_on_raw_payload() {
        let mut data = HashMap::new();
        data.insert("num".to_string(), ConfigValue::Float(3.125));
        data.insert("text".to_string(), ConfigValue::Text("hello".to_string()));
        data.insert("flag".to_string(), ConfigValue::Bool(true));
        data.insert(
            "arr".to_string(),
            ConfigValue::FloatArray(vec![1.0, 2.0, 3.0]),
        );

        let ec = EquipmentConfig::raw("test".to_string(), "Test".to_string(), data);

        assert_eq!(ec.get_f64("num"), Some(3.125));
        assert_eq!(ec.get_str("text"), Some("hello"));
        assert_eq!(ec.get_bool("flag"), Some(true));
        assert_eq!(ec.get_f64_array("arr"), Some(&[1.0, 2.0, 3.0][..]));
        assert_eq!(ec.get_f64("missing"), None);
        assert!(!ec.is_typed());
    }

    #[test]
    fn raw_accessors_return_none_on_typed_payload() {
        let config = TestConfig {
            value: 42.5,
            name: "test".to_string(),
        };
        let ec =
            EquipmentConfig::from_typed("test".to_string(), "Test".to_string(), config).unwrap();

        assert!(ec.is_typed());
        assert_eq!(ec.get_f64("any"), None);
        assert_eq!(ec.get_str("any"), None);
        assert_eq!(ec.get_bool("any"), None);
        assert_eq!(ec.get_f64_array("any"), None);
    }

    /// Type with a custom Serialize impl that always returns an error,
    /// used to verify that `from_typed()` returns `Err` rather than panicking.
    #[derive(Clone, Debug, Deserialize, PartialEq)]
    struct FailSerialize;

    impl Serialize for FailSerialize {
        fn serialize<S: serde::Serializer>(
            &self,
            _serializer: S,
        ) -> std::result::Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom(
                "intentional serialization failure",
            ))
        }
    }

    impl EquipmentTypedConfig for FailSerialize {
        fn equipment_type_name() -> &'static str {
            "FailEquipment"
        }
    }

    #[test]
    fn from_typed_valid_config_returns_ok() {
        let config = TestConfig {
            value: 1.0,
            name: "valid".to_string(),
        };
        let result =
            EquipmentConfig::from_typed("test_valid".to_string(), "TestClass".to_string(), config);
        assert!(result.is_ok());
    }

    #[test]
    fn from_typed_serialization_failure_returns_err() {
        let config = FailSerialize;
        let result =
            EquipmentConfig::from_typed("test_fail".to_string(), "FailClass".to_string(), config);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("typed config serialization failed"));
        assert!(err.contains("FailEquipment"));
    }

    #[test]
    fn from_typed_produces_correct_config_values() {
        let config = TestConfig {
            value: 73.3,
            name: "sensor_a".to_string(),
        };
        let ec = EquipmentConfig::from_typed(
            "sensor".to_string(),
            "SensorClass".to_string(),
            config.clone(),
        )
        .unwrap();
        let recovered: TestConfig = ec.typed().unwrap();
        assert_eq!(recovered.value, 73.3);
        assert_eq!(recovered.name, "sensor_a");
        assert_eq!(ec.name, "sensor");
        assert_eq!(ec.ochre_class, "SensorClass");
    }

    // ── resolve_zip precedence ──────────────────────────────────────────

    use hares_types::zip::{ZipLoad, zip_defaults_for_class};

    fn raw_cfg(class: &str, pairs: &[(&str, f64)]) -> EquipmentConfig {
        let data = pairs
            .iter()
            .map(|&(k, v)| (k.to_string(), ConfigValue::Float(v)))
            .collect();
        EquipmentConfig::raw(class.to_string(), class.to_string(), data)
    }

    #[test]
    fn resolve_zip_falls_back_to_constant_power_for_unknown_class() {
        let cfg = raw_cfg("Totally Unknown Class", &[]);
        assert_eq!(super::resolve_zip(&cfg), ZipLoad::constant_power());
    }

    #[test]
    fn resolve_zip_uses_class_defaults_when_no_sidecar_or_raw_keys() {
        let cfg = raw_cfg("ASHP Heater", &[]);
        assert_eq!(
            super::resolve_zip(&cfg),
            zip_defaults_for_class("ASHP Heater").expect("class row")
        );
    }

    #[test]
    fn resolve_zip_sidecar_beats_class_defaults() {
        let mut cfg = raw_cfg("ASHP Heater", &[]);
        let sidecar = ZipLoad::reactive_only(0.5, 0.62, -0.12, 0.87);
        cfg.zip = Some(sidecar);
        assert_eq!(super::resolve_zip(&cfg), sidecar);
    }

    #[test]
    fn resolve_zip_ignores_raw_payload_keys_entirely() {
        // The legacy raw `zip_*` config-key channel is gone: only the sidecar
        // and the class table feed the resolver.
        let cfg = raw_cfg("ASHP Heater", &[("zip_pf", 0.5)]);
        assert_eq!(
            super::resolve_zip(&cfg),
            zip_defaults_for_class("ASHP Heater").expect("class row")
        );
    }

    // ── validate_zip_sums ───────────────────────────────────────────────

    #[test]
    fn validate_zip_sums_accepts_all_class_rows() {
        for name in hares_types::zip::ZIP_CLASS_NAMES {
            let zip = zip_defaults_for_class(name).expect("row");
            super::validate_zip_sums(&zip, name).expect("class row must satisfy sum invariants");
        }
        super::validate_zip_sums(&ZipLoad::constant_power(), "cp").expect("constant power");
    }

    #[test]
    fn validate_zip_sums_rejects_bad_real_sum() {
        let mut zip = ZipLoad::constant_power();
        zip.pp = 0.9;
        let err = super::validate_zip_sums(&zip, "eq").unwrap_err();
        assert!(
            err.to_string().contains("invalid ZIP coefficients"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_zip_sums_rejects_bad_reactive_sum_when_pf_nonzero() {
        let zip = ZipLoad::reactive_only(0.3, 0.3, 0.3, 0.9);
        let err = super::validate_zip_sums(&zip, "eq").unwrap_err();
        assert!(
            err.to_string()
                .contains("invalid reactive ZIP coefficients"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_zip_sums_ignores_reactive_sum_with_pf_zero_sentinel() {
        // pf = 0 means the reactive polynomial is never evaluated.
        let zip = ZipLoad::reactive_only(0.3, 0.3, 0.3, 0.0);
        super::validate_zip_sums(&zip, "eq").expect("pf=0 sentinel skips reactive sum");
    }

    // ── zip sidecar serde ───────────────────────────────────────────────

    #[test]
    fn equipment_config_round_trips_with_zip_sidecar() {
        let mut cfg = raw_cfg("ASHP Heater", &[]);
        cfg.zip = Some(zip_defaults_for_class("ASHP Heater").expect("class row"));
        let json = serde_json::to_string(&cfg).expect("serialize");
        assert!(
            json.contains("\"zip\""),
            "zip sidecar must serialize: {json}"
        );
        let back: EquipmentConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.zip, cfg.zip);
        assert_eq!(back, cfg);
    }

    #[test]
    fn equipment_config_without_zip_omits_key_and_round_trips() {
        let cfg = raw_cfg("ASHP Heater", &[]);
        let json = serde_json::to_string(&cfg).expect("serialize");
        assert!(
            !json.contains("\"zip\""),
            "None sidecar must be skipped: {json}"
        );
        let back: EquipmentConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.zip, None);
        assert_eq!(back, cfg);
    }

    #[test]
    fn old_serialized_configs_without_zip_key_still_deserialize() {
        // JSON captured from the pre-sidecar schema: no "zip" key anywhere.
        let json = r#"{
            "name": "ASHP Heater",
            "ochre_class": "ASHP Heater",
            "payload": {"kind": "raw", "data": {"power_constant_kw": 1.5}}
        }"#;
        let cfg: EquipmentConfig = serde_json::from_str(json).expect("legacy deserialize");
        assert_eq!(cfg.zip, None);
        assert_eq!(cfg.get_f64("power_constant_kw"), Some(1.5));
    }
}
