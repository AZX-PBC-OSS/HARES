//! The parameters a raw-parameter load reads, one list per load kind.
//!
//! The lists sit beside the constants the loads read through. The dwelling
//! admits an override only of a parameter on its load's list, and a
//! parameter's value only of the kind the load reads it as, so an override
//! cannot land in a key nothing reads or read as absent. In debug and test
//! builds every raw read of a listed class checks the key against its list,
//! so a reader that starts reading a new key fails its tests until the key
//! is listed.

use std::fmt::Write as _;

use serde_json::Value;

use crate::config::{ConfigValue, KEY_EQUIPMENT_ID, KEY_USAGE_MULTIPLIER, KEY_ZONE_ID};
use crate::schedule_helpers::KEY_MONTH_MULTIPLIER_PREFIX;

/// A parameter's name: one key, or a numbered family `{prefix}{n}{suffix}`
/// for each `n` the count admits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamForm {
    Key(&'static str),
    Indexed {
        prefix: &'static str,
        suffix: &'static str,
        count: IndexCount,
    },
}

/// A load's parameter values by name, as the parameters a count refers to
/// are looked up.
pub type ParamLookup<'v> = dyn Fn(&str) -> Option<&'v Value> + 'v;

/// How many members a numbered family has: `n` runs from 0 to below it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexCount {
    Fixed(usize),
    /// The value of another parameter of the same load; none when absent.
    Param(&'static str),
}

/// The kind of value a load reads a parameter as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamKind {
    Number,
    Text,
    Bool,
    NumberList,
}

impl ParamKind {
    /// Whether `value` is of this kind.
    #[must_use]
    pub fn holds(self, value: &ConfigValue) -> bool {
        matches!(
            (self, value),
            (Self::Number, ConfigValue::Float(_))
                | (Self::Text, ConfigValue::Text(_))
                | (Self::Bool, ConfigValue::Bool(_))
                | (Self::NumberList, ConfigValue::FloatArray(_))
        )
    }

    /// The kind as an error names it.
    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            Self::Number => "a number",
            Self::Text => "text",
            Self::Bool => "true or false",
            Self::NumberList => "a list of numbers",
        }
    }
}

/// One parameter a load reads, and the kind it reads it as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawParam {
    pub form: ParamForm,
    pub kind: ParamKind,
}

impl RawParam {
    #[must_use]
    pub const fn key(name: &'static str, kind: ParamKind) -> Self {
        Self {
            form: ParamForm::Key(name),
            kind,
        }
    }

    #[must_use]
    pub const fn indexed(
        prefix: &'static str,
        suffix: &'static str,
        count: IndexCount,
        kind: ParamKind,
    ) -> Self {
        Self {
            form: ParamForm::Indexed {
                prefix,
                suffix,
                count,
            },
            kind,
        }
    }

    /// The member number `key` has in this family, or `None` when `key` is
    /// not of its form.
    fn index_of(self, key: &str) -> Option<usize> {
        match self.form {
            ParamForm::Key(name) => (key == name).then_some(0),
            ParamForm::Indexed { prefix, suffix, .. } => {
                let digits = key.strip_prefix(prefix)?.strip_suffix(suffix)?;
                if digits.is_empty()
                    || !digits.bytes().all(|b| b.is_ascii_digit())
                    || (digits.len() > 1 && digits.starts_with('0'))
                {
                    return None;
                }
                digits.parse().ok()
            }
        }
    }

    /// Whether this parameter is `key` for a load whose parameter values
    /// `lookup` gives: the key is of its form, and a numbered key's number
    /// is below its family's count.
    fn matches(self, key: &str, lookup: &ParamLookup<'_>) -> bool {
        self.index_of(key).is_some_and(|index| match self.form {
            ParamForm::Key(_) => true,
            ParamForm::Indexed { count, .. } => index < count.resolve(lookup),
        })
    }

    /// Whether this parameter can be `key` for some load: as
    /// [`Self::matches`], with a count set by another parameter taken as
    /// unbounded.
    fn may_match(self, key: &str) -> bool {
        self.index_of(key).is_some_and(|index| match self.form {
            ParamForm::Indexed {
                count: IndexCount::Fixed(count),
                ..
            } => index < count,
            _ => true,
        })
    }

    fn describe(self, out: &mut String) {
        let _ = match self.form {
            ParamForm::Key(name) => write!(out, "{name}"),
            ParamForm::Indexed {
                prefix,
                suffix,
                count: IndexCount::Fixed(n),
            } => write!(out, "{prefix}<n>{suffix} (n below {n})"),
            ParamForm::Indexed {
                prefix,
                suffix,
                count: IndexCount::Param(key),
            } => write!(out, "{prefix}<n>{suffix} (n below {key})"),
        };
    }
}

impl IndexCount {
    fn resolve(self, lookup: &ParamLookup<'_>) -> usize {
        match self {
            Self::Fixed(count) => count,
            Self::Param(key) => lookup(key)
                .and_then(Value::as_f64)
                .filter(|count| count.is_finite() && *count >= 0.0 && count.fract() == 0.0)
                .map_or(0, |count| count as usize),
        }
    }
}

/// The parameters one kind of raw-parameter load reads.
#[derive(Debug)]
pub struct RawParams {
    /// The load kind, as errors name it.
    pub kind: &'static str,
    groups: &'static [&'static [RawParam]],
}

impl RawParams {
    /// Every parameter on the list.
    pub fn params(&self) -> impl Iterator<Item = RawParam> + '_ {
        self.groups.iter().flat_map(|group| group.iter().copied())
    }

    /// The parameter `key` is for a load whose parameter values `lookup`
    /// gives, or `None` when the load does not read `key`.
    #[must_use]
    pub fn param(&self, key: &str, lookup: &ParamLookup<'_>) -> Option<RawParam> {
        self.params().find(|param| param.matches(key, lookup))
    }

    /// Whether the load reads `key` when `lookup` gives its parameter
    /// values: the key is on the list, and a numbered key's number is below
    /// its family's count.
    #[must_use]
    pub fn reads(&self, key: &str, lookup: &ParamLookup<'_>) -> bool {
        self.param(key, lookup).is_some()
    }

    /// The parameter `key` can be for some load of this kind: on the list
    /// and within a fixed count, a count set by another parameter taken as
    /// unbounded.
    #[must_use]
    pub fn named(&self, key: &str) -> Option<RawParam> {
        self.params().find(|param| param.may_match(key))
    }

    /// Whether some load of this kind reads `key` ([`Self::named`]).
    #[must_use]
    pub fn names(&self, key: &str) -> bool {
        self.named(key).is_some()
    }

    /// The list, for an error to show: `a, b, phase_<n>_power_kw (n below
    /// phase_len), month_multiplier_<n> (n below 12)`.
    #[must_use]
    pub fn describe(&self) -> String {
        let mut out = String::new();
        for (i, param) in self.params().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            param.describe(&mut out);
        }
        out
    }
}

/// What every raw-parameter load reads beside its gain fractions: its
/// identity and zone, and its monthly scale factors.
pub(crate) const LOAD_COMMON: &[RawParam] = &[
    RawParam::key(KEY_EQUIPMENT_ID, ParamKind::Number),
    RawParam::key(KEY_ZONE_ID, ParamKind::Number),
    RawParam::indexed(
        KEY_MONTH_MULTIPLIER_PREFIX,
        "",
        IndexCount::Fixed(12),
        ParamKind::Number,
    ),
];

/// A scheduled load: lighting, plug loads, refrigeration, pumps, pool and
/// spa heaters, gas loads and fans.
pub static SCHEDULED_LOAD: RawParams = RawParams {
    kind: "scheduled load",
    groups: &[
        LOAD_COMMON,
        &crate::gain_fractions::GAIN_PARAMS,
        &[RawParam::key(KEY_USAGE_MULTIPLIER, ParamKind::Number)],
        crate::scheduled_load::SCHEDULE_PARAMS,
    ],
};

/// An event-driven load with one active power: the cooking range, the
/// microwave and the generic event load.
pub static EVENT_LOAD: RawParams = RawParams {
    kind: "event load",
    groups: &[
        LOAD_COMMON,
        &crate::gain_fractions::GAIN_PARAMS,
        crate::event_load::EVENT_PARAMS,
        crate::event_load::EVENT_LOAD_PARAMS,
    ],
};

/// A wet appliance, whose events run a cycle of phases: the clothes washer,
/// the dishwasher and the clothes dryer.
pub static WET_APPLIANCE: RawParams = RawParams {
    kind: "wet appliance",
    groups: &[
        LOAD_COMMON,
        &crate::gain_fractions::GAIN_PARAMS,
        crate::event_load::EVENT_PARAMS,
        crate::event_load::WET_APPLIANCE_PARAMS,
    ],
};

/// The parameter list of the raw-parameter load registered as
/// `ochre_class`, or `None` for a class that is not a raw-parameter load.
#[must_use]
pub fn raw_params_for_class(ochre_class: &str) -> Option<&'static RawParams> {
    if crate::scheduled_load::CLASSES
        .iter()
        .any(|(class, _)| *class == ochre_class)
    {
        return Some(&SCHEDULED_LOAD);
    }
    if crate::event_load::EVENT_LOAD_CLASSES.contains(&ochre_class) {
        return Some(&EVENT_LOAD);
    }
    if crate::event_load::WET_APPLIANCE_CLASSES.contains(&ochre_class) {
        return Some(&WET_APPLIANCE);
    }
    None
}

/// Every load kind's list.
pub const ALL: [&RawParams; 3] = [&SCHEDULED_LOAD, &EVENT_LOAD, &WET_APPLIANCE];

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Map, json};

    fn lookup<'v>(params: &'v Map<String, Value>) -> impl Fn(&str) -> Option<&'v Value> + 'v {
        move |key| params.get(key)
    }

    #[test]
    fn a_numbered_family_admits_members_below_its_count() {
        let wet = &WET_APPLIANCE;
        let four_phases = json!({ "phase_len": 4 }).as_object().cloned().unwrap();
        let none = Map::new();
        assert!(wet.reads("phase_3_power_kw", &lookup(&four_phases)));
        assert!(!wet.reads("phase_4_power_kw", &lookup(&four_phases)));
        assert!(!wet.reads("phase_0_power_kw", &lookup(&none)));
        assert!(wet.names("phase_9_power_kw"));
        assert!(SCHEDULED_LOAD.reads("month_multiplier_11", &lookup(&none)));
        assert!(!SCHEDULED_LOAD.reads("month_multiplier_12", &lookup(&none)));
        assert!(!SCHEDULED_LOAD.names("month_multiplier_12"));
    }

    #[test]
    fn a_numbered_key_needs_a_plain_number() {
        for key in [
            "month_multiplier_",
            "month_multiplier_01",
            "month_multiplier_-1",
            "month_multiplier_1x",
            "month_multiplier_ 1",
        ] {
            assert!(!SCHEDULED_LOAD.names(key), "{key}");
        }
    }

    #[test]
    fn a_kind_holds_only_its_values() {
        assert!(ParamKind::Number.holds(&ConfigValue::Float(1.0)));
        assert!(!ParamKind::Number.holds(&ConfigValue::Text("1".to_string())));
        assert!(!ParamKind::Bool.holds(&ConfigValue::Float(1.0)));
        assert!(ParamKind::NumberList.holds(&ConfigValue::FloatArray(vec![])));
        assert!(!ParamKind::Text.holds(&ConfigValue::Bool(true)));
    }

    #[test]
    fn every_registered_load_class_has_its_list() {
        let registry = crate::EquipmentRegistry::new();
        for (class, _) in crate::scheduled_load::CLASSES {
            assert!(registry.get(class).is_some(), "{class} is not registered");
            assert!(std::ptr::eq(
                raw_params_for_class(class).expect("listed"),
                &SCHEDULED_LOAD
            ));
        }
        for class in crate::event_load::EVENT_LOAD_CLASSES {
            assert!(registry.get(class).is_some(), "{class} is not registered");
            assert!(std::ptr::eq(
                raw_params_for_class(class).expect("listed"),
                &EVENT_LOAD
            ));
        }
        for class in crate::event_load::WET_APPLIANCE_CLASSES {
            assert!(registry.get(class).is_some(), "{class} is not registered");
            assert!(std::ptr::eq(
                raw_params_for_class(class).expect("listed"),
                &WET_APPLIANCE
            ));
        }
        assert!(raw_params_for_class("Gas Furnace").is_none());
    }

    #[test]
    fn the_description_names_every_parameter() {
        let described = WET_APPLIANCE.describe();
        for param in WET_APPLIANCE.params() {
            let shown = match param.form {
                ParamForm::Key(name) => name.to_string(),
                ParamForm::Indexed { prefix, suffix, .. } => format!("{prefix}<n>{suffix}"),
            };
            assert!(
                described.contains(&shown),
                "{shown} missing from {described}"
            );
        }
    }
}
