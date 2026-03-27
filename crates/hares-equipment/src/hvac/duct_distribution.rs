//! Duct distribution and multi-zone heat fraction routing for HVAC equipment.
//!
//! Extracted from `hvac_core.rs` to isolate zone heat routing from speed
//! staging and thermostat control. No speed or staging knowledge here.

use hares_types::{PortContribution, PortSlots, ThermalCategory, ZoneId};

use super::hvac_core::HvacEquipment;

impl HvacEquipment {
    /// Compute `zone_heat_fractions` from `duct_dse` and `duct_zone_id`.
    ///
    /// Fractions are absolute multipliers on gross capacity:
    ///   - conditioned zone: `duct_dse * (1 - basement_frac)`
    ///   - basement zone (if any): `duct_dse * basement_frac`
    ///   - duct zone (if any and different from conditioned): `1.0 - duct_dse`
    ///
    /// OCHRE HVAC.py lines 188-197.
    pub fn update_zone_heat_fractions(&mut self) {
        let dse = self.duct_dse.clamp(0.0, 1.0);
        let basement_frac = self.basement_heat_frac.clamp(0.0, 1.0);

        let conditioned_frac = dse * (1.0 - basement_frac);
        self.zone_heat_fractions = vec![(self.zone_id, conditioned_frac)];

        if basement_frac > 0.0 {
            if let Some(basement_zone) = self.basement_zone_id {
                if basement_zone != self.zone_id {
                    self.zone_heat_fractions
                        .push((basement_zone, dse * basement_frac));
                }
            }
        }

        if dse < 1.0 {
            if let Some(duct_zone) = self.duct_zone_id {
                if duct_zone != self.zone_id {
                    self.zone_heat_fractions.push((duct_zone, 1.0 - dse));
                }
            }
        }
    }

    /// Distribute gross capacity across zones using absolute `zone_heat_fractions`.
    ///
    /// Each fraction is a direct multiplier on `sensible_gain_w` / `latent_gain_w`.
    /// Callers must pass gross (pre-DSE) capacity.
    pub fn write_zone_thermal_contributions(
        &self,
        ports: &mut PortSlots,
        sensible_gain_w: f64,
        latent_gain_w: f64,
        category: ThermalCategory,
    ) -> crate::Result<()> {
        let fractions: &[(ZoneId, f64)] = if self.zone_heat_fractions.is_empty() {
            &[(self.zone_id, 1.0)]
        } else {
            &self.zone_heat_fractions
        };

        if fractions.len() == 1 {
            let f = fractions[0].1.max(0.0);
            return ports.accumulate(&PortContribution::Thermal {
                zone: fractions[0].0,
                sensible_gain_w: sensible_gain_w * f,
                latent_gain_w: latent_gain_w * f,
                category,
            });
        }

        for &(zone, fraction) in fractions {
            if fraction > 0.0 {
                ports.accumulate(&PortContribution::Thermal {
                    zone,
                    sensible_gain_w: sensible_gain_w * fraction,
                    latent_gain_w: latent_gain_w * fraction,
                    category,
                })?;
            }
        }
        Ok(())
    }

    /// Apply duct distribution system efficiency (DSE) to a capacity value.
    pub fn apply_duct_dse(&self, capacity_w: f64) -> f64 {
        capacity_w * self.duct_dse.clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use hares_types::ZoneId;

    use super::super::hvac_core::{HvacEquipment, HvacEquipmentType};

    fn make_hvac() -> HvacEquipment {
        HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1))
    }

    /// DSE is a direct capacity multiplier: 10000 * 0.80 = 8000 W.
    #[test]
    fn duct_dse_scales_capacity() {
        let mut hvac = make_hvac();
        hvac.duct_dse = 0.80;
        let result = hvac.apply_duct_dse(10_000.0);
        assert!(
            (result - 8_000.0).abs() < 1e-9,
            "expected 8000 W, got {result}"
        );
    }

    /// DSE = 1.0, no basement, no duct zone: conditioned zone gets 100%.
    #[test]
    fn zone_heat_fractions_single_zone_no_duct_loss() {
        let mut hvac = make_hvac();
        hvac.duct_dse = 1.0;
        hvac.basement_heat_frac = 0.0;
        hvac.duct_zone_id = None;
        hvac.update_zone_heat_fractions();

        assert_eq!(hvac.zone_heat_fractions.len(), 1);
        let (zone, frac) = hvac.zone_heat_fractions[0];
        assert_eq!(zone, ZoneId(1));
        assert!(
            (frac - 1.0).abs() < 1e-12,
            "conditioned zone fraction must be 1.0, got {frac}"
        );
    }

    /// DSE = 0.85, basement_heat_frac = 0.30, with basement and duct zones.
    /// conditioned: 0.85 * (1 - 0.30) = 0.595
    /// basement:    0.85 * 0.30 = 0.255
    /// duct:        1.0 - 0.85 = 0.15
    #[test]
    fn zone_heat_fractions_with_basement() {
        let mut hvac = make_hvac();
        hvac.duct_dse = 0.85;
        hvac.basement_heat_frac = 0.30;
        hvac.basement_zone_id = Some(ZoneId(2));
        hvac.duct_zone_id = Some(ZoneId(3));
        hvac.update_zone_heat_fractions();

        assert_eq!(hvac.zone_heat_fractions.len(), 3);

        let cond = hvac.zone_heat_fractions.iter().find(|&&(z, _)| z == ZoneId(1)).unwrap().1;
        let bsmt = hvac.zone_heat_fractions.iter().find(|&&(z, _)| z == ZoneId(2)).unwrap().1;
        let duct = hvac.zone_heat_fractions.iter().find(|&&(z, _)| z == ZoneId(3)).unwrap().1;

        assert!(
            (cond - 0.595).abs() < 1e-9,
            "conditioned fraction: expected 0.595, got {cond}"
        );
        assert!(
            (bsmt - 0.255).abs() < 1e-9,
            "basement fraction: expected 0.255, got {bsmt}"
        );
        assert!(
            (duct - 0.15).abs() < 1e-9,
            "duct fraction: expected 0.15, got {duct}"
        );

        let total = cond + bsmt + duct;
        assert!(
            (total - 1.0).abs() < 1e-9,
            "fractions must sum to 1.0, got {total}"
        );
    }

    /// DSE = 1.0 means no duct losses, so no duct zone entry should appear.
    #[test]
    fn duct_dse_one_no_duct_zone_entry() {
        let mut hvac = make_hvac();
        hvac.duct_dse = 1.0;
        hvac.duct_zone_id = Some(ZoneId(3));
        hvac.update_zone_heat_fractions();

        let has_duct = hvac.zone_heat_fractions.iter().any(|&(z, _)| z == ZoneId(3));
        assert!(
            !has_duct,
            "DSE=1.0 must not produce a duct zone entry, got {:?}",
            hvac.zone_heat_fractions
        );
    }
}
