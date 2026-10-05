//! The share of a load's input that becomes heat in its zone, and its split.

use hares_types::{HaresError, PortContribution, PortSlots, ThermalCategory, ZoneId};

use crate::EquipmentConfig;

pub(crate) const KEY_SENSIBLE: &str = "sensible_gain_fraction";
const KEY_SENSIBLE_HPXML: &str = "frac_sensible";
pub(crate) const KEY_CONVECTIVE: &str = "convective_gain_fraction";
pub(crate) const KEY_RADIANT: &str = "radiative_gain_fraction";
pub(crate) const KEY_LATENT: &str = "latent_gain_fraction";
const KEY_LATENT_HPXML: &str = "frac_latent";
const TOLERANCE: f64 = 1e-9;

/// Fractions of a load's input that enter its zone: `sensible` in total,
/// `radiant` the long-wave part of it (the rest is convective), and
/// `latent`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct GainFractions {
    pub(crate) sensible: f64,
    pub(crate) radiant: f64,
    pub(crate) latent: f64,
}

/// One step's heat from a load into its zone, in watts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ZoneGain {
    pub(crate) convective_w: f64,
    pub(crate) radiant_w: f64,
    pub(crate) latent_w: f64,
}

impl GainFractions {
    /// Reads the fractions from a load's config. The sensible fraction is
    /// required: `sensible_gain_fraction`, its HPXML spelling
    /// `frac_sensible`, or the sum of `convective_gain_fraction` and
    /// `radiative_gain_fraction`. An absent radiant or latent fraction is
    /// none.
    ///
    /// # Errors
    ///
    /// A missing sensible fraction; a non-finite or negative fraction; a
    /// sensible fraction above 1, a sensible and latent sum above 1, or a
    /// radiant fraction above the sensible one.
    pub(crate) fn from_config(config: &EquipmentConfig, name: &str) -> crate::Result<Self> {
        let radiant = config.get_f64(KEY_RADIANT).unwrap_or(0.0);
        let sensible = config
            .get_f64(KEY_SENSIBLE)
            .or_else(|| config.get_f64(KEY_SENSIBLE_HPXML))
            .or_else(|| {
                let convective = config.get_f64(KEY_CONVECTIVE).unwrap_or(0.0);
                (convective > 0.0 || radiant > 0.0).then_some(convective + radiant)
            })
            .ok_or_else(|| {
                HaresError::Equipment(format!(
                    "sensible_gain_fraction missing for '{name}'; must be specified explicitly"
                ))
            })?;
        let latent = config
            .get_f64(KEY_LATENT)
            .or_else(|| config.get_f64(KEY_LATENT_HPXML))
            .unwrap_or(0.0);
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
        if radiant > sensible + TOLERANCE {
            return Err(HaresError::Equipment(format!(
                "radiant_gain_fraction ({radiant}) must not exceed sensible_gain_fraction \
                 ({sensible}) (convective gain would be negative)"
            )));
        }
        Ok(Self {
            sensible,
            radiant,
            latent,
        })
    }

    /// Whether any of the load's input becomes heat in a zone.
    pub(crate) fn gives_zone_heat(&self) -> bool {
        self.sensible + self.latent > 0.0
    }

    /// The zone heat from `input_w` of electric and fuel input.
    pub(crate) fn of(&self, input_w: f64) -> ZoneGain {
        let radiant_w = input_w * self.radiant;
        ZoneGain {
            convective_w: input_w * self.sensible - radiant_w,
            radiant_w,
            latent_w: input_w * self.latent,
        }
    }
}

/// Adds a load's zone heat to its zone's thermal port as an internal gain:
/// the convective part as sensible, the radiant part on the radiant path.
/// A load with no zone, or no heat this step, adds nothing.
///
/// # Errors
///
/// The zone has no declared thermal port.
pub(crate) fn accumulate_zone_gain(
    ports: &mut PortSlots,
    zone: Option<ZoneId>,
    gain: ZoneGain,
) -> crate::Result<()> {
    if let Some(zone) = zone
        && (gain.convective_w != 0.0 || gain.radiant_w != 0.0 || gain.latent_w != 0.0)
    {
        ports.accumulate(&PortContribution::Thermal {
            zone,
            sensible_gain_w: gain.convective_w,
            radiant_gain_w: gain.radiant_w,
            latent_gain_w: gain.latent_w,
            category: ThermalCategory::InternalGain,
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

    #[test]
    fn convective_and_radiant_parts_give_the_sensible_fraction() {
        let fractions = GainFractions::from_config(
            &config(&[(KEY_CONVECTIVE, 0.3), (KEY_RADIANT, 0.2)]),
            "Load",
        )
        .unwrap();
        assert!((fractions.sensible - 0.5).abs() < 1e-12);
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
            vec![(KEY_SENSIBLE, 0.5), (KEY_LATENT, f64::NAN)],
        ] {
            assert!(
                GainFractions::from_config(&config(&pairs), "Load").is_err(),
                "{pairs:?} must be rejected"
            );
        }
    }
}
