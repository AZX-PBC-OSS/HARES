//! The share of a load's input that becomes heat in its zone, and its split.

use hares_types::{HaresError, PortContribution, PortSlots, ThermalCategory, ZoneHeat, ZoneId};
use serde_json::{Map, Value};

use crate::EquipmentConfig;
use crate::config::ConfigValue;
use crate::raw_params::{ParamKind, RawParam};

pub(crate) const KEY_SENSIBLE: &str = "sensible_gain_fraction";
pub(crate) const KEY_RADIANT: &str = "radiative_gain_fraction";
pub(crate) const KEY_LATENT: &str = "latent_gain_fraction";
pub(crate) const KEY_RADIANT_SHARE: &str = "radiant_share_of_sensible";
pub(crate) const KEY_VISIBLE_SHARE: &str = "visible_share_of_sensible";
const TOLERANCE: f64 = 1e-9;

/// The parameters a load's zone heat is read from.
pub const GAIN_KEYS: [&str; 5] = [
    KEY_SENSIBLE,
    KEY_RADIANT,
    KEY_LATENT,
    KEY_RADIANT_SHARE,
    KEY_VISIBLE_SHARE,
];

/// [`GAIN_KEYS`] as entries of a load's parameter list, each a number.
pub(crate) const GAIN_PARAMS: [RawParam; GAIN_KEYS.len()] = {
    let mut params = [RawParam::key(GAIN_KEYS[0], ParamKind::Number); GAIN_KEYS.len()];
    let mut i = 1;
    while i < GAIN_KEYS.len() {
        params[i] = RawParam::key(GAIN_KEYS[i], ParamKind::Number);
        i += 1;
    }
    params
};

/// The HPXML spellings (`extension/FracSensible`, `extension/FracLatent`)
/// and the parameter each one stands for.
pub const GAIN_KEY_ALIASES: [(&str, &str); 2] =
    [("frac_sensible", KEY_SENSIBLE), ("frac_latent", KEY_LATENT)];

/// The parameter `key` names: the gain parameter an HPXML spelling stands
/// for, else `key` itself.
#[must_use]
pub fn canonical_gain_key(key: &str) -> &str {
    GAIN_KEY_ALIASES
        .iter()
        .find_map(|(alias, canonical)| (*alias == key).then_some(*canonical))
        .unwrap_or(key)
}

/// Rewrites the HPXML spellings in `params` to the parameters they stand
/// for, so one key carries each fraction wherever it came from. The values
/// are the load's to check when it reads them.
///
/// # Errors
///
/// A fraction given under both spellings.
pub fn canonicalize_gain_params(
    params: &mut Map<String, Value>,
    equipment: &str,
) -> Result<(), HaresError> {
    check_one_gain_spelling(params, equipment)?;
    for (alias, key) in GAIN_KEY_ALIASES {
        if let Some(value) = params.remove(alias) {
            params.insert(key.to_string(), value);
        }
    }
    Ok(())
}

/// Checks that `params` gives each fraction under one spelling.
///
/// # Errors
///
/// A fraction under both its HPXML spelling and its parameter name, an
/// error naming the HPXML spelling and `equipment`.
pub fn check_one_gain_spelling(
    params: &Map<String, Value>,
    equipment: &str,
) -> Result<(), HaresError> {
    match GAIN_KEY_ALIASES
        .iter()
        .find(|(alias, key)| params.contains_key(*alias) && params.contains_key(*key))
    {
        Some((alias, key)) => Err(HaresError::InvalidEquipmentParameter {
            equipment: equipment.to_string(),
            key: (*alias).to_string(),
            reason: format!("and '{key}' set the same fraction; give one of them"),
        }),
        None => Ok(()),
    }
}

/// A gain parameter of `config`: `None` when absent, an error when present
/// with a value that is not a number.
fn fraction(config: &EquipmentConfig, key: &str, name: &str) -> crate::Result<Option<f64>> {
    match config.raw_value(key, ParamKind::Number) {
        None => Ok(None),
        Some(ConfigValue::Float(value)) => Ok(Some(*value)),
        Some(other) => Err(HaresError::InvalidEquipmentParameter {
            equipment: name.to_string(),
            key: key.to_string(),
            reason: format!("must be a number, got {other}"),
        }),
    }
}

/// Fractions of a load's input that enter its zone: `sensible` in total,
/// of which `radiant` is long-wave radiation, `visible` short-wave
/// radiation and the rest convective; and `latent`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct GainFractions {
    pub(crate) sensible: f64,
    pub(crate) radiant: f64,
    pub(crate) visible: f64,
    pub(crate) latent: f64,
}

impl GainFractions {
    /// Reads the fractions from a load's config. The sensible fraction
    /// (`sensible_gain_fraction`) is required. The radiant and visible parts
    /// are shares of the final sensible fraction (`radiant_share_of_sensible`,
    /// `visible_share_of_sensible`), resolved here, after every override has
    /// set the sensible fraction; an explicit `radiative_gain_fraction` sets
    /// the radiant fraction itself instead. An absent share or latent
    /// fraction is none.
    ///
    /// # Errors
    ///
    /// A missing sensible fraction; a gain parameter that is not a number;
    /// a non-finite or negative fraction or share; a share above 1 or shares
    /// summing above 1; a sensible fraction above 1, a sensible and latent
    /// sum above 1, or radiant and visible fractions above the sensible one.
    pub(crate) fn from_config(config: &EquipmentConfig, name: &str) -> crate::Result<Self> {
        let radiant_fraction = fraction(config, KEY_RADIANT, name)?;
        let radiant_share = fraction(config, KEY_RADIANT_SHARE, name)?.unwrap_or(0.0);
        let visible_share = fraction(config, KEY_VISIBLE_SHARE, name)?.unwrap_or(0.0);
        let sensible = fraction(config, KEY_SENSIBLE, name)?.ok_or_else(|| {
            HaresError::Equipment(format!(
                "sensible_gain_fraction missing for '{name}'; must be specified explicitly"
            ))
        })?;
        let latent = fraction(config, KEY_LATENT, name)?.unwrap_or(0.0);
        for (key, value) in [
            (KEY_RADIANT_SHARE, radiant_share),
            (KEY_VISIBLE_SHARE, visible_share),
        ] {
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                return Err(HaresError::Equipment(format!(
                    "{key} ({value}) must be within [0, 1]"
                )));
            }
        }
        if radiant_share + visible_share > 1.0 + TOLERANCE {
            return Err(HaresError::Equipment(format!(
                "radiant_share_of_sensible ({radiant_share}) + visible_share_of_sensible \
                 ({visible_share}) must not exceed 1.0"
            )));
        }
        let radiant = radiant_fraction.unwrap_or(radiant_share * sensible);
        let visible = visible_share * sensible;
        for (key, value) in [
            ("sensible_gain_fraction", sensible),
            ("radiant_gain_fraction", radiant),
            ("latent_gain_fraction", latent),
        ] {
            if !value.is_finite() || value < 0.0 {
                return Err(HaresError::Equipment(format!(
                    "{key} ({value}) must be finite and must not be negative"
                )));
            }
        }
        if sensible > 1.0 + TOLERANCE {
            return Err(HaresError::Equipment(format!(
                "sensible_gain_fraction ({sensible}) must not exceed 1.0"
            )));
        }
        if sensible + latent > 1.0 + TOLERANCE {
            return Err(HaresError::Equipment(format!(
                "sensible_gain_fraction ({sensible}) + latent_gain_fraction ({latent}) = {} \
                 must not exceed 1.0",
                sensible + latent
            )));
        }
        if radiant + visible > sensible + TOLERANCE {
            return Err(HaresError::Equipment(format!(
                "radiant_gain_fraction ({radiant}) + visible part ({visible}) must not exceed \
                 sensible_gain_fraction ({sensible}) (convective gain would be negative)"
            )));
        }
        Ok(Self {
            sensible,
            radiant,
            visible,
            latent,
        })
    }

    /// Whether any of the load's input becomes heat in a zone.
    pub(crate) fn gives_zone_heat(&self) -> bool {
        self.sensible + self.latent > 0.0
    }

    /// The zone heat from `input_w` of electric and fuel input.
    pub(crate) fn of(&self, input_w: f64) -> ZoneHeat {
        let radiant_w = input_w * self.radiant;
        let shortwave_w = input_w * self.visible;
        ZoneHeat {
            convective_w: input_w * self.sensible - radiant_w - shortwave_w,
            radiant_w,
            shortwave_w,
            latent_w: input_w * self.latent,
        }
    }
}

/// Adds a load's zone heat to its zone's thermal port as an internal gain:
/// the convective part as sensible, the long-wave part on the radiant path
/// and the short-wave part on the short-wave path. A load with no zone, or
/// no heat this step, adds nothing.
///
/// # Errors
///
/// The zone has no declared thermal port.
pub(crate) fn accumulate_zone_gain(
    ports: &mut PortSlots,
    zone: Option<ZoneId>,
    gain: ZoneHeat,
) -> crate::Result<()> {
    let Some(zone) = zone else {
        return Ok(());
    };
    if gain.convective_w != 0.0 || gain.radiant_w != 0.0 || gain.latent_w != 0.0 {
        ports.accumulate(&PortContribution::Thermal {
            zone,
            sensible_gain_w: gain.convective_w,
            radiant_gain_w: gain.radiant_w,
            latent_gain_w: gain.latent_w,
            category: ThermalCategory::InternalGain,
        })?;
    }
    if gain.shortwave_w != 0.0 {
        ports.accumulate(&PortContribution::ShortWave {
            zone,
            shortwave_gain_w: gain.shortwave_w,
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::config::ConfigValue;

    fn config(pairs: &[(&str, f64)]) -> EquipmentConfig {
        let raw: HashMap<String, ConfigValue> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).into()))
            .collect();
        EquipmentConfig::raw("Load".to_string(), "Load".to_string(), raw)
    }

    #[test]
    fn radiant_part_of_sensible_leaves_the_rest_convective() {
        let fractions = GainFractions::from_config(
            &config(&[
                (KEY_SENSIBLE, 0.72),
                (KEY_RADIANT, 0.432),
                (KEY_LATENT, 0.08),
            ]),
            "Load",
        )
        .unwrap();
        let gain = fractions.of(1000.0);
        assert!((gain.radiant_w - 432.0).abs() < 1e-9);
        assert!((gain.convective_w - 288.0).abs() < 1e-9);
        assert!((gain.latent_w - 80.0).abs() < 1e-9);
    }

    /// OpenStudio-HPXML lighting: all of the power sensible, 0.6 long-wave,
    /// 0.2 visible, the remaining 0.2 convective; the visible part goes on
    /// the short-wave path of the zone's port.
    #[test]
    fn visible_part_takes_the_short_wave_path() {
        let fractions = GainFractions::from_config(
            &config(&[
                (KEY_SENSIBLE, 1.0),
                (KEY_RADIANT_SHARE, 0.6),
                (KEY_VISIBLE_SHARE, 0.2),
            ]),
            "Indoor Lighting",
        )
        .unwrap();
        let gain = fractions.of(500.0);
        assert!((gain.convective_w - 100.0).abs() < 1e-9);
        assert!((gain.radiant_w - 300.0).abs() < 1e-9);
        assert!((gain.shortwave_w - 100.0).abs() < 1e-9);

        let zone = ZoneId(1);
        let mut ports =
            PortSlots::from_declarations(&[hares_types::PortDeclaration::thermal(zone)]);
        accumulate_zone_gain(&mut ports, Some(zone), gain).unwrap();
        let thermal = &ports.thermal[0];
        assert!((thermal.sensible_gain_w - 100.0).abs() < 1e-9);
        assert!((thermal.radiant_gain_w - 300.0).abs() < 1e-9);
        assert!((thermal.shortwave_gain_w - 100.0).abs() < 1e-9);
        assert!((thermal.total_gain_w() - 500.0).abs() < 1e-9);
    }

    /// The shares follow the sensible fraction the load ends up with, and an
    /// explicit radiant fraction takes precedence over the radiant share.
    #[test]
    fn shares_scale_with_the_final_sensible_fraction() {
        for sensible in [0.3, 0.72, 0.9] {
            let fractions = GainFractions::from_config(
                &config(&[
                    (KEY_SENSIBLE, sensible),
                    (KEY_RADIANT_SHARE, 0.6),
                    (KEY_VISIBLE_SHARE, 0.2),
                ]),
                "Load",
            )
            .unwrap();
            assert_eq!(fractions.radiant, 0.6 * sensible);
            assert_eq!(fractions.visible, 0.2 * sensible);
        }
        let explicit = GainFractions::from_config(
            &config(&[
                (KEY_SENSIBLE, 0.5),
                (KEY_RADIANT, 0.1),
                (KEY_RADIANT_SHARE, 0.6),
            ]),
            "Load",
        )
        .unwrap();
        assert_eq!(explicit.radiant, 0.1);
    }

    /// A gain parameter whose value is not a number is an error naming the
    /// parameter and the load, never an absent share.
    #[test]
    fn a_gain_parameter_that_is_not_a_number_is_rejected() {
        for key in GAIN_KEYS {
            let mut raw: HashMap<String, ConfigValue> = HashMap::new();
            raw.insert(KEY_SENSIBLE.to_string(), 0.72.into());
            raw.insert(key.to_string(), "0.9".into());
            let config = EquipmentConfig::raw("Range".to_string(), "Range".to_string(), raw);
            let err = GainFractions::from_config(&config, "Range").unwrap_err();
            assert!(
                matches!(
                    &err,
                    HaresError::InvalidEquipmentParameter { equipment, key: k, .. }
                        if equipment == "Range" && k == key
                ),
                "{key}: {err}"
            );
        }
    }

    /// The HPXML spelling becomes the parameter it stands for, so an
    /// override under either key replaces the other; both together are an
    /// error.
    #[test]
    fn hpxml_spellings_collapse_onto_one_parameter() {
        let mut params = Map::new();
        params.insert("frac_sensible".to_string(), serde_json::json!(0.3));
        params.insert("frac_latent".to_string(), serde_json::json!(0.2));
        canonicalize_gain_params(&mut params, "Range").unwrap();
        assert_eq!(params.get(KEY_SENSIBLE), Some(&serde_json::json!(0.3)));
        assert_eq!(params.get(KEY_LATENT), Some(&serde_json::json!(0.2)));
        assert!(!params.contains_key("frac_sensible"));
        assert!(!params.contains_key("frac_latent"));

        let Value::Object(mut both) =
            serde_json::json!({"frac_sensible": 0.3, "sensible_gain_fraction": 0.5})
        else {
            unreachable!()
        };
        let err = canonicalize_gain_params(&mut both, "Range").unwrap_err();
        assert!(
            matches!(&err, HaresError::InvalidEquipmentParameter { key, .. } if key == "frac_sensible"),
            "{err}"
        );
        assert_eq!(canonical_gain_key("frac_latent"), KEY_LATENT);
        assert_eq!(canonical_gain_key("zone_id"), "zone_id");
    }

    #[test]
    fn invalid_fractions_are_rejected() {
        for pairs in [
            vec![],
            vec![(KEY_SENSIBLE, f64::NAN)],
            vec![(KEY_SENSIBLE, -0.1)],
            vec![(KEY_SENSIBLE, 1.1)],
            vec![(KEY_SENSIBLE, 0.7), (KEY_LATENT, 0.4)],
            vec![(KEY_SENSIBLE, 0.5), (KEY_RADIANT, 0.6)],
            vec![
                (KEY_SENSIBLE, 0.5),
                (KEY_RADIANT, 0.4),
                (KEY_VISIBLE_SHARE, 0.4),
            ],
            vec![(KEY_SENSIBLE, 0.5), (KEY_VISIBLE_SHARE, -0.1)],
            vec![(KEY_SENSIBLE, 0.5), (KEY_RADIANT_SHARE, 1.1)],
            vec![
                (KEY_SENSIBLE, 0.5),
                (KEY_RADIANT_SHARE, 0.7),
                (KEY_VISIBLE_SHARE, 0.4),
            ],
            vec![(KEY_SENSIBLE, 0.5), (KEY_RADIANT_SHARE, f64::NAN)],
            vec![(KEY_SENSIBLE, 0.5), (KEY_LATENT, f64::NAN)],
        ] {
            assert!(
                GainFractions::from_config(&config(&pairs), "Load").is_err(),
                "{pairs:?} must be rejected"
            );
        }
    }
}
