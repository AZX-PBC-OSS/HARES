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
