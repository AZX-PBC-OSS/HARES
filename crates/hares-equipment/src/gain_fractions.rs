//! The share of a load's input that becomes heat in its zone, and its split.

use hares_types::{HaresError, PortContribution, PortSlots, ThermalCategory, ZoneId};

use crate::EquipmentConfig;

pub(crate) const KEY_SENSIBLE: &str = "sensible_gain_fraction";
const KEY_SENSIBLE_HPXML: &str = "frac_sensible";
pub(crate) const KEY_CONVECTIVE: &str = "convective_gain_fraction";
pub(crate) const KEY_RADIANT: &str = "radiative_gain_fraction";
pub(crate) const KEY_LATENT: &str = "latent_gain_fraction";
const KEY_LATENT_HPXML: &str = "frac_latent";
pub(crate) const KEY_RADIANT_SHARE: &str = "radiant_share_of_sensible";
pub(crate) const KEY_VISIBLE_SHARE: &str = "visible_share_of_sensible";
const TOLERANCE: f64 = 1e-9;

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

/// One step's heat from a load into its zone, in watts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ZoneGain {
    pub(crate) convective_w: f64,
    pub(crate) radiant_w: f64,
    pub(crate) shortwave_w: f64,
    pub(crate) latent_w: f64,
}

impl GainFractions {
    /// Reads the fractions from a load's config. The sensible fraction is
    /// required: `sensible_gain_fraction`, its HPXML spelling
    /// `frac_sensible`, or the sum of `convective_gain_fraction` and
    /// `radiative_gain_fraction`. The radiant and visible parts are shares
    /// of the final sensible fraction (`radiant_share_of_sensible`,
    /// `visible_share_of_sensible`), resolved here, after every override has
    /// set the sensible fraction; an explicit `radiative_gain_fraction` sets
    /// the radiant fraction itself instead. An absent share or latent
    /// fraction is none.
    ///
    /// # Errors
    ///
    /// A missing sensible fraction; a non-finite or negative fraction or
    /// share; a share above 1 or shares summing above 1; a sensible fraction
    /// above 1, a sensible and latent sum above 1, or radiant and visible
    /// fractions above the sensible one.
    pub(crate) fn from_config(config: &EquipmentConfig, name: &str) -> crate::Result<Self> {
        let radiant_fraction = config.get_f64(KEY_RADIANT);
        let radiant_share = config.get_f64(KEY_RADIANT_SHARE).unwrap_or(0.0);
        let visible_share = config.get_f64(KEY_VISIBLE_SHARE).unwrap_or(0.0);
        let sensible = config
            .get_f64(KEY_SENSIBLE)
            .or_else(|| config.get_f64(KEY_SENSIBLE_HPXML))
            .or_else(|| {
                let convective = config.get_f64(KEY_CONVECTIVE).unwrap_or(0.0);
                let radiant = radiant_fraction.unwrap_or(0.0);
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
    pub(crate) fn of(&self, input_w: f64) -> ZoneGain {
        let radiant_w = input_w * self.radiant;
        let shortwave_w = input_w * self.visible;
        ZoneGain {
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
    gain: ZoneGain,
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
