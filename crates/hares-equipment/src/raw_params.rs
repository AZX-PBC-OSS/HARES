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

    fn raw_load(class: &str, params: &[(&str, ConfigValue)]) -> crate::EquipmentConfig {
        let data = params
            .iter()
            .map(|(key, value)| ((*key).to_string(), value.clone()))
            .collect();
        crate::EquipmentConfig::raw(class.to_string(), class.to_string(), data).with_rng_stream(
            hares_types::rng::RngStream::event_load(hares_types::rng::dwelling_seed(1, 0), class),
        )
    }

    fn panic_message(result: std::thread::Result<()>) -> String {
        let payload = result.expect_err("the read must panic");
        payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_string()))
            .unwrap_or_default()
    }

    fn one_class_per_kind() -> [(&'static str, &'static RawParams); 3] {
        [
            ("Lighting", &SCHEDULED_LOAD),
            ("Cooking Range", &EVENT_LOAD),
            ("Clothes Washer", &WET_APPLIANCE),
        ]
    }

    #[cfg(debug_assertions)]
    #[test]
    fn a_read_off_its_list_panics_in_a_debug_build() {
        for (class, params) in one_class_per_kind() {
            let config = raw_load(class, &[]);
            assert!(!params.names("no_such_parameter"));
            let message = panic_message(std::panic::catch_unwind(|| {
                let _ = config.get_f64("no_such_parameter");
            }));
            assert!(message.contains("does not name"), "{class}: {message}");
            assert!(message.contains("no_such_parameter"), "{class}: {message}");
        }
    }

    #[cfg(debug_assertions)]
    #[test]
    fn a_listed_key_read_as_another_kind_panics_in_a_debug_build() {
        for (class, params) in one_class_per_kind() {
            let config = raw_load(class, &[]);
            assert_eq!(
                params.named(KEY_ZONE_ID).map(|param| param.kind),
                Some(ParamKind::Number)
            );
            let message = panic_message(std::panic::catch_unwind(|| {
                let _ = config.get_str(KEY_ZONE_ID);
            }));
            assert!(message.contains("does not name"), "{class}: {message}");
            assert!(message.contains("text"), "{class}: {message}");
        }
    }

    #[test]
    fn a_listed_key_read_as_its_kind_is_answered() {
        for (class, _) in one_class_per_kind() {
            let config = raw_load(class, &[(KEY_ZONE_ID, ConfigValue::Float(2.0))]);
            assert_eq!(config.get_f64(KEY_ZONE_ID), Some(2.0), "{class}");
            assert_eq!(config.get_f64("month_multiplier_11"), None, "{class}");
        }
        let unlisted = raw_load("Not A Load", &[("anything", ConfigValue::Text("x".into()))]);
        assert_eq!(unlisted.get_str("anything"), Some("x"));
    }

    fn number(value: f64) -> ConfigValue {
        ConfigValue::Float(value)
    }

    fn text(value: &str) -> ConfigValue {
        ConfigValue::Text(value.to_string())
    }

    fn numbers(len: usize, value: f64) -> ConfigValue {
        ConfigValue::FloatArray(vec![value; len])
    }

    fn months() -> Vec<(String, ConfigValue)> {
        (0..12)
            .map(|month| (format!("{KEY_MONTH_MULTIPLIER_PREFIX}{month}"), number(1.0)))
            .collect()
    }

    /// The configs that, between them, take every read path a kind's
    /// loads have: each schedule source, each event window and probability
    /// source, a phased and an unphased wet cycle.
    fn configs_reaching_every_read(class: &str, params: &RawParams) -> Vec<crate::EquipmentConfig> {
        let common: Vec<(&str, ConfigValue)> = vec![
            (KEY_EQUIPMENT_ID, number(1.0)),
            (KEY_ZONE_ID, number(1.0)),
            ("sensible_gain_fraction", number(0.5)),
            ("latent_gain_fraction", number(0.1)),
        ];
        let variants: Vec<Vec<(&str, ConfigValue)>> = if std::ptr::eq(params, &SCHEDULED_LOAD) {
            let profile = |prefix: &'static str, max: &'static str| {
                vec![(prefix, text("daily_profile")), (max, number(1.0))]
            };
            vec![
                vec![
                    ("power_schedule_source", text("column")),
                    ("power_schedule_col", number(0.0)),
                    ("gas_schedule_source", text("column")),
                    ("gas_schedule_col", number(1.0)),
                    ("gas_schedule_is_w", ConfigValue::Bool(true)),
                    (KEY_USAGE_MULTIPLIER, number(2.0)),
                ],
                [
                    profile("power_schedule_source", "power_profile_max_kw"),
                    profile("gas_schedule_source", "gas_profile_max"),
                    vec![
                        ("power_profile_weekday", numbers(24, 1.0)),
                        ("power_profile_weekend", numbers(24, 1.0)),
                        ("power_profile_month", numbers(12, 1.0)),
                        ("gas_profile_weekday", numbers(24, 1.0)),
                        ("gas_profile_weekend", numbers(24, 1.0)),
                        ("gas_profile_month", numbers(12, 1.0)),
                    ],
                ]
                .concat(),
                vec![
                    ("power_schedule_source", text("constant")),
                    ("power_constant_kw", number(0.1)),
                    ("gas_schedule_source", text("constant")),
                    ("gas_constant", number(0.1)),
                ],
            ]
        } else {
            let column_sources = vec![
                ("event_window_schedule_col", number(0.0)),
                ("event_probability_schedule_col", number(1.0)),
                ("fuel_type", text("electricity")),
                ("event_power_kw_series", numbers(4, 0.0)),
            ];
            let constant_sources = vec![
                ("event_window_source", text("constant")),
                ("event_probability_source", text("constant")),
                ("event_probability_constant", number(0.5)),
            ];
            let single_cycle = vec![
                ("active_power_kw", number(1.0)),
                ("active_duration_s", number(60.0)),
                ("cooldown_duration_s", number(60.0)),
            ];
            if std::ptr::eq(params, &EVENT_LOAD) {
                vec![[column_sources, single_cycle].concat(), constant_sources]
            } else {
                let phased = vec![
                    ("n_units", number(1.0)),
                    ("hot_water_draw_volume_l", number(10.0)),
                    ("phase_len", number(1.0)),
                    ("phase_0_power_kw", number(1.0)),
                    ("phase_0_duration_s", number(60.0)),
                    ("phase_0_has_water_draw", ConfigValue::Bool(true)),
                ];
                vec![
                    [column_sources, phased].concat(),
                    [constant_sources, single_cycle].concat(),
                ]
            }
        };
        let months = months();
        variants
            .into_iter()
            .map(|variant| {
                let mut all: Vec<(&str, ConfigValue)> = common.clone();
                all.extend(variant);
                all.extend(
                    months
                        .iter()
                        .map(|(key, value)| (key.as_str(), value.clone())),
                );
                raw_load(class, &all)
            })
            .collect()
    }

    fn init_env() -> hares_types::EnvironmentState {
        hares_types::EnvironmentState {
            zones: vec![hares_types::ZoneState::new(
                hares_types::ZoneId(1),
                21.0,
                0.008,
                200.0,
            )],
            weather: hares_types::WeatherState::default(),
            grid: hares_types::GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
                island_bus_voltage_pu: None,
            },
            domains: hares_types::DomainSlots::default(),
            schedule_row: Some(0),
            ambient_other_space_c: hares_types::AmbientOtherSpaceTemps::default(),
            equipment_telemetry: std::collections::HashMap::new(),
            equipment_core: std::collections::HashMap::new(),
            current_time: chrono::DateTime::parse_from_rfc3339("2026-03-18T00:00:00+00:00")
                .expect("valid time"),
            time_res: chrono::Duration::minutes(15),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }

    /// The readers and the lists agree both ways: a read of a key off its
    /// list panics (above), and every parameter on a list is read by its
    /// loads, so an override the dwelling admits always reaches a reader.
    #[test]
    fn every_listed_parameter_has_a_reader() {
        let registry = crate::EquipmentRegistry::new();
        let env = init_env();
        for (class, params) in one_class_per_kind() {
            crate::config::RAW_READS.with_borrow_mut(std::collections::HashSet::clear);
            for config in configs_reaching_every_read(class, params) {
                let mut load = registry.create(class, config.clone()).expect("registered");
                load.init(&config, &env)
                    .unwrap_or_else(|err| panic!("{class} init: {err}"));
                crate::config::equipment_id_from_config(&config).expect("valid id");
            }
            let reads = crate::config::RAW_READS.with_borrow(Clone::clone);
            let unread: Vec<RawParam> = params
                .params()
                .filter(|param| !reads.iter().any(|key| param.may_match(key)))
                .collect();
            assert!(unread.is_empty(), "{class}: nothing reads {unread:?}");
        }
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
