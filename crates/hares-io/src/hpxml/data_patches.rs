//! HPXML data-quality patches for fields that may be missing or invalid in
//! the source HPXML document. Populated from ResStock metadata or other
//! external data sources that provide ground-truth values.
use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Typed patches that supplement or correct HPXML-parsed data.
///
/// When the HPXML parser encounters a missing or invalid field, the
/// resolver consults these patches as a fallback before using a
/// documented default. This struct carries **no** ResStock-specific
/// knowledge — column-to-field mapping is handled by the constructor.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HpxmlDataPatches {
    pub number_of_bedrooms: Option<f64>,
}

impl HpxmlDataPatches {
    /// Build patches from ResStock metadata characteristics.
    ///
    /// This is the **only** place that knows about ResStock column names.
    /// When ResStock adds new columns, add the mapping here and to the struct,
    /// not to individual resolvers.
    pub fn from_resstock_characteristics(chars: &HashMap<String, String>) -> Self {
        Self {
            number_of_bedrooms: chars
                .get("in.bedrooms")
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|v| v.is_finite() && *v >= 0.5),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scenario_patch_fields_are_this_structs_fields() {
        // The scenario spec's patch-target table (hares_types::PATCH_FIELDS)
        // claims to name the fields this struct carries. The serde shape is
        // the struct's field list, so a field added here without the
        // scenario table (or the reverse) fails: the spec would refuse a
        // patch the assembly can carry, or admit one it cannot.
        let serialized = serde_json::to_value(HpxmlDataPatches::default())
            .expect("the patches serialize to their field map");
        let mut names: Vec<&str> = serialized
            .as_object()
            .expect("the patches serialize to an object")
            .keys()
            .map(String::as_str)
            .collect();
        names.sort_unstable();
        let mut recorded = hares_types::PATCH_FIELDS.to_vec();
        recorded.sort_unstable();
        assert_eq!(
            names, recorded,
            "the scenario's patch-field table must name exactly this struct's fields"
        );
    }

    #[test]
    fn from_resstock_characteristics_extracts_bedrooms() {
        let mut chars = HashMap::new();
        chars.insert("in.bedrooms".to_string(), "3".to_string());
        chars.insert("in.state".to_string(), "CO".to_string());
        chars.insert("in.vintage".to_string(), "1960s".to_string());

        let patches = HpxmlDataPatches::from_resstock_characteristics(&chars);
        assert_eq!(patches.number_of_bedrooms, Some(3.0));
    }

    #[test]
    fn from_resstock_characteristics_filters_invalid_bedrooms() {
        let mut chars = HashMap::new();
        chars.insert("in.bedrooms".to_string(), "-1".to_string());
        let patches = HpxmlDataPatches::from_resstock_characteristics(&chars);
        assert_eq!(patches.number_of_bedrooms, None);

        let mut chars = HashMap::new();
        chars.insert("in.bedrooms".to_string(), "NaN".to_string());
        let patches = HpxmlDataPatches::from_resstock_characteristics(&chars);
        assert_eq!(patches.number_of_bedrooms, None);

        let mut chars = HashMap::new();
        chars.insert("in.bedrooms".to_string(), "0.4".to_string());
        let patches = HpxmlDataPatches::from_resstock_characteristics(&chars);
        assert_eq!(patches.number_of_bedrooms, None);
    }

    #[test]
    fn from_resstock_characteristics_missing_key_returns_none() {
        let chars: HashMap<String, String> = HashMap::new();
        let patches = HpxmlDataPatches::from_resstock_characteristics(&chars);
        assert_eq!(patches.number_of_bedrooms, None);
    }

    #[test]
    fn from_resstock_characteristics_filters_non_finite() {
        let mut chars = HashMap::new();
        chars.insert("in.bedrooms".to_string(), f64::INFINITY.to_string());
        let patches = HpxmlDataPatches::from_resstock_characteristics(&chars);
        assert_eq!(patches.number_of_bedrooms, None);

        let mut chars = HashMap::new();
        chars.insert("in.bedrooms".to_string(), f64::NAN.to_string());
        let patches = HpxmlDataPatches::from_resstock_characteristics(&chars);
        assert_eq!(patches.number_of_bedrooms, None);
    }
}
