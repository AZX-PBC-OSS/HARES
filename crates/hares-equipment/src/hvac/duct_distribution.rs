//! Duct distribution and multi-zone heat fraction routing for HVAC equipment.
//!
//! Extracted from `hvac_core.rs` to isolate zone heat routing from speed
//! staging and thermostat control. No speed or staging knowledge here.

use hares_types::{
    HaresError, PortContribution, PortDeclaration, PortSlots, ThermalCategory, ZoneId,
};
use tracing::warn;

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
        // When ducts are located within the conditioned space, losses loop back to
        // the same zone -- effectively DSE = 1.0. Route full gross capacity there.
        let effective_dse = if self
            .config
            .duct_zone_id
            .is_some_and(|dz| dz == self.config.zone_id)
        {
            if self.config.duct_dse < 1.0 {
                warn!(
                    duct_dse = self.config.duct_dse,
                    zone_id = %self.config.zone_id,
                    "duct_zone == conditioned_zone with DSE < 1.0; \
                     treating as DSE=1.0 (losses stay in conditioned space)"
                );
            }
            1.0
        } else {
            self.config.duct_dse.clamp(0.0, 1.0)
        };
        let basement_frac = self.config.basement_heat_frac.clamp(0.0, 1.0);

        let conditioned_frac = effective_dse * (1.0 - basement_frac);
        self.config.zone_heat_fractions = vec![(self.config.zone_id, conditioned_frac)];

        if basement_frac > 0.0 {
            if let Some(basement_zone) = self.config.basement_zone_id {
                if basement_zone != self.config.zone_id {
                    self.config
                        .zone_heat_fractions
                        .push((basement_zone, effective_dse * basement_frac));
                }
            }
        }

        if effective_dse < 1.0 {
            if let Some(duct_zone) = self.config.duct_zone_id {
                if duct_zone != self.config.zone_id {
                    self.config
                        .zone_heat_fractions
                        .push((duct_zone, 1.0 - effective_dse));
                }
            }
        }

        // Deduplicate zone entries when basement_zone == duct_zone: merge by
        // summing fractions so downstream code sees exactly one entry per zone.
        self.deduplicate_zone_heat_fractions();
    }

    /// Merge zone_heat_fractions entries with the same ZoneId by summing their
    /// fractions. Preserves order of first occurrence.
    fn deduplicate_zone_heat_fractions(&mut self) {
        if self.config.zone_heat_fractions.len() <= 1 {
            return;
        }
        let mut merged: Vec<(ZoneId, f64)> =
            Vec::with_capacity(self.config.zone_heat_fractions.len());
        for &(zone, frac) in &self.config.zone_heat_fractions {
            if let Some(entry) = merged.iter_mut().find(|(z, _)| *z == zone) {
                entry.1 += frac;
            } else {
                merged.push((zone, frac));
            }
        }
        self.config.zone_heat_fractions = merged;
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
        if self.config.zone_heat_fractions.is_empty() {
            return Err(HaresError::Equipment(
                "write_zone_thermal_contributions called before update_zone_heat_fractions".into(),
            ));
        }

        let fractions: &[(ZoneId, f64)] = &self.config.zone_heat_fractions;

        for &(zone, fraction) in fractions {
            if fraction > 0.0 {
                // Contributions to the duct zone (and NOT the conditioned zone)
                // are tagged DuctLoss so the thermal solver can distinguish
                // delivered capacity from duct losses in its diagnostics.
                let effective_category = if self.config.duct_zone_id.is_some_and(|dz| dz == zone)
                    && zone != self.config.zone_id
                {
                    ThermalCategory::DuctLoss
                } else {
                    category
                };
                ports.accumulate(&PortContribution::Thermal {
                    zone,
                    sensible_gain_w: sensible_gain_w * fraction,
                    radiant_gain_w: 0.0,
                    latent_gain_w: latent_gain_w * fraction,
                    category: effective_category,
                })?;
            }
        }
        Ok(())
    }

    /// Apply duct distribution system efficiency (DSE) to a capacity value.
    pub fn apply_duct_dse(&self, capacity_w: f64) -> f64 {
        capacity_w * self.config.duct_dse.clamp(0.0, 1.0)
    }

    /// Rebuild thermal port declarations to include all zones referenced by
    /// `zone_heat_fractions`. Call after `update_zone_heat_fractions()` in init
    /// so that the port list matches the zones that will receive contributions.
    ///
    /// `needs_humidity` gates the `PortType::Humidity` declaration. Only
    /// equipment that writes `PortContribution::Humidity` (air conditioners,
    /// heat pump coolers) should pass `true`; heating-only equipment passes
    /// `false` to avoid spurious `HumidityAccumulator` allocations.
    pub fn rebuild_thermal_ports(&self, ports: &mut Vec<PortDeclaration>, needs_humidity: bool) {
        use hares_types::PortType;
        ports.retain(|p| p.port_type != PortType::Thermal && p.port_type != PortType::Humidity);
        for &(zone, _) in &self.config.zone_heat_fractions {
            ports.push(PortDeclaration::thermal(zone));
            if needs_humidity {
                ports.push(PortDeclaration::humidity(zone));
            }
        }
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
        hvac.config.duct_dse = 0.80;
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
        hvac.config.duct_dse = 1.0;
        hvac.config.basement_heat_frac = 0.0;
        hvac.config.duct_zone_id = None;
        hvac.update_zone_heat_fractions();

        assert_eq!(hvac.config.zone_heat_fractions.len(), 1);
        let (zone, frac) = hvac.config.zone_heat_fractions[0];
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
        hvac.config.duct_dse = 0.85;
        hvac.config.basement_heat_frac = 0.30;
        hvac.config.basement_zone_id = Some(ZoneId(2));
        hvac.config.duct_zone_id = Some(ZoneId(3));
        hvac.update_zone_heat_fractions();

        assert_eq!(hvac.config.zone_heat_fractions.len(), 3);

        let cond = hvac
            .config
            .zone_heat_fractions
            .iter()
            .find(|&&(z, _)| z == ZoneId(1))
            .unwrap()
            .1;
        let bsmt = hvac
            .config
            .zone_heat_fractions
            .iter()
            .find(|&&(z, _)| z == ZoneId(2))
            .unwrap()
            .1;
        let duct = hvac
            .config
            .zone_heat_fractions
            .iter()
            .find(|&&(z, _)| z == ZoneId(3))
            .unwrap()
            .1;

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
        hvac.config.duct_dse = 1.0;
        hvac.config.duct_zone_id = Some(ZoneId(3));
        hvac.update_zone_heat_fractions();

        let has_duct = hvac
            .config
            .zone_heat_fractions
            .iter()
            .any(|&(z, _)| z == ZoneId(3));
        assert!(
            !has_duct,
            "DSE=1.0 must not produce a duct zone entry, got {:?}",
            hvac.config.zone_heat_fractions
        );
    }

    /// When duct_zone_id == zone_id (ducts are within the conditioned space), no heat
    /// is lost externally -- the conditioned zone must receive the full gross capacity.
    #[test]
    fn update_zone_heat_fractions_same_duct_zone() {
        let mut hvac = make_hvac();
        hvac.config.duct_dse = 0.85;
        hvac.config.basement_heat_frac = 0.0;
        hvac.config.duct_zone_id = Some(ZoneId(1));
        hvac.update_zone_heat_fractions();

        assert_eq!(
            hvac.config.zone_heat_fractions.len(),
            1,
            "duct_zone == indoor_zone must produce exactly one zone entry; \
             got {:?}",
            hvac.config.zone_heat_fractions
        );
        let (zone, frac) = hvac.config.zone_heat_fractions[0];
        assert_eq!(zone, ZoneId(1));
        assert!(
            (frac - 1.0).abs() < 1e-9,
            "conditioned fraction must be 1.0 when ducts are within the conditioned space, got {frac}"
        );
    }

    /// write_zone_thermal_contributions with a positive sensible gain (heating).
    /// DSE=1.0, single zone → conditioned zone must receive the full gain.
    #[test]
    fn write_zone_thermal_contributions_heating() {
        use hares_types::{PortSlots, ThermalAccumulator, ThermalCategory};

        let mut hvac = make_hvac();
        hvac.config.duct_dse = 1.0;
        hvac.update_zone_heat_fractions();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };

        let sensible_w = 8_000.0_f64;
        let latent_w = 0.0_f64;
        hvac.write_zone_thermal_contributions(
            &mut ports,
            sensible_w,
            latent_w,
            ThermalCategory::HvacHeating,
        )
        .expect("write must succeed for declared zone");

        let acc = ports.thermal.iter().find(|a| a.zone == ZoneId(1)).unwrap();
        assert!(
            (acc.sensible_gain_w - sensible_w).abs() < 1e-9,
            "zone 1 sensible gain: expected {sensible_w} W, got {}",
            acc.sensible_gain_w
        );
        assert!(
            acc.latent_gain_w.abs() < 1e-9,
            "zone 1 latent gain must be zero, got {}",
            acc.latent_gain_w
        );
    }

    /// write_zone_thermal_contributions with a negative sensible gain (cooling).
    /// The sign must pass through unchanged -- cooling is a negative heat contribution.
    #[test]
    fn write_zone_thermal_contributions_cooling() {
        use hares_types::{PortSlots, ThermalAccumulator, ThermalCategory};

        let mut hvac = make_hvac();
        hvac.config.duct_dse = 1.0;
        hvac.update_zone_heat_fractions();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };

        let sensible_w = -9_000.0_f64; // negative = cooling load removed
        let latent_w = -1_500.0_f64;
        hvac.write_zone_thermal_contributions(
            &mut ports,
            sensible_w,
            latent_w,
            ThermalCategory::HvacCooling,
        )
        .expect("write must succeed for declared zone");

        let acc = ports.thermal.iter().find(|a| a.zone == ZoneId(1)).unwrap();
        assert!(
            (acc.sensible_gain_w - sensible_w).abs() < 1e-9,
            "cooling sensible must be negative: expected {sensible_w} W, got {}",
            acc.sensible_gain_w
        );
        assert!(
            acc.sensible_gain_w < 0.0,
            "cooling sensible gain must be negative, got {}",
            acc.sensible_gain_w
        );
        assert!(
            (acc.latent_gain_w - latent_w).abs() < 1e-9,
            "cooling latent: expected {latent_w} W, got {}",
            acc.latent_gain_w
        );
    }

    /// DSE=0.7, duct zone receives DuctLoss category.
    #[test]
    fn duct_zone_tagged_duct_loss() {
        use hares_types::{PortSlots, ThermalAccumulator, ThermalCategory};

        let mut hvac = make_hvac();
        hvac.config.duct_dse = 0.7;
        hvac.config.duct_zone_id = Some(ZoneId(2));
        hvac.update_zone_heat_fractions();

        let mut ports = PortSlots {
            thermal: vec![
                ThermalAccumulator::new(ZoneId(1)),
                ThermalAccumulator::new(ZoneId(2)),
            ],
            ..Default::default()
        };
        hvac.write_zone_thermal_contributions(
            &mut ports,
            10_000.0,
            0.0,
            ThermalCategory::HvacHeating,
        )
        .expect("write ok");

        let duct_acc = ports.thermal.iter().find(|a| a.zone == ZoneId(2)).unwrap();
        assert!(
            (duct_acc.sensible_for_category(ThermalCategory::DuctLoss) - 3_000.0).abs() < 1e-9,
            "duct zone must receive 3_000 W as DuctLoss"
        );
        assert!(
            duct_acc
                .sensible_for_category(ThermalCategory::HvacHeating)
                .abs()
                < 1e-12,
            "duct zone must NOT have HvacHeating"
        );
    }

    /// Conditioned zone keeps the caller's category (HvacHeating), not DuctLoss.
    #[test]
    fn conditioned_zone_not_tagged_duct_loss() {
        use hares_types::{PortSlots, ThermalAccumulator, ThermalCategory};

        let mut hvac = make_hvac();
        hvac.config.duct_dse = 0.7;
        hvac.config.duct_zone_id = Some(ZoneId(2));
        hvac.update_zone_heat_fractions();

        let mut ports = PortSlots {
            thermal: vec![
                ThermalAccumulator::new(ZoneId(1)),
                ThermalAccumulator::new(ZoneId(2)),
            ],
            ..Default::default()
        };
        hvac.write_zone_thermal_contributions(
            &mut ports,
            10_000.0,
            0.0,
            ThermalCategory::HvacHeating,
        )
        .expect("write ok");

        let cond_acc = ports.thermal.iter().find(|a| a.zone == ZoneId(1)).unwrap();
        assert!(
            (cond_acc.sensible_for_category(ThermalCategory::HvacHeating) - 7_000.0).abs() < 1e-9,
            "conditioned zone must receive 7_000 W as HvacHeating"
        );
        assert!(
            cond_acc
                .sensible_for_category(ThermalCategory::DuctLoss)
                .abs()
                < 1e-12,
            "conditioned zone must NOT have DuctLoss"
        );
    }

    /// When basement_zone == duct_zone, deduplication merges into a single entry.
    #[test]
    fn basement_equals_duct_zone_merges() {
        let mut hvac = make_hvac();
        hvac.config.duct_dse = 0.8;
        hvac.config.basement_heat_frac = 0.2;
        hvac.config.basement_zone_id = Some(ZoneId(2));
        hvac.config.duct_zone_id = Some(ZoneId(2)); // same as basement
        hvac.update_zone_heat_fractions();

        // conditioned: 0.8 * 0.8 = 0.64
        // basement:    0.8 * 0.2 = 0.16
        // duct:        0.2
        // merged zone 2: 0.16 + 0.2 = 0.36
        assert_eq!(
            hvac.config.zone_heat_fractions.len(),
            2,
            "basement==duct must produce 2 entries (not 3); got {:?}",
            hvac.config.zone_heat_fractions
        );
        let merged_frac = hvac
            .config
            .zone_heat_fractions
            .iter()
            .find(|&&(z, _)| z == ZoneId(2))
            .unwrap()
            .1;
        assert!(
            (merged_frac - 0.36).abs() < 1e-9,
            "merged zone 2 fraction must be 0.36, got {merged_frac}"
        );
        let total: f64 = hvac.config.zone_heat_fractions.iter().map(|(_, f)| f).sum();
        assert!(
            (total - 1.0).abs() < 1e-9,
            "fractions must sum to 1.0, got {total}"
        );
    }

    /// When basement_zone == duct_zone (both ZoneId(2)), the merged entry receives
    /// the correct total watts. The entire merged contribution is tagged DuctLoss
    /// because the zone matches duct_zone_id and is != conditioned zone. This is
    /// accepted behavior: the watts are physically correct regardless of category.
    #[test]
    fn basement_equals_duct_zone_write_zone_contributions() {
        use hares_types::{PortSlots, ThermalAccumulator, ThermalCategory};

        let mut hvac = make_hvac(); // conditioned = ZoneId(1)
        hvac.config.duct_dse = 0.8;
        hvac.config.basement_heat_frac = 0.2;
        hvac.config.basement_zone_id = Some(ZoneId(2));
        hvac.config.duct_zone_id = Some(ZoneId(2)); // same as basement
        hvac.update_zone_heat_fractions();

        // conditioned: 0.8 * 0.8 = 0.64
        // merged zone 2: basement(0.8*0.2=0.16) + duct(0.2) = 0.36
        let mut ports = PortSlots {
            thermal: vec![
                ThermalAccumulator::new(ZoneId(1)),
                ThermalAccumulator::new(ZoneId(2)),
            ],
            ..Default::default()
        };

        let gross_w = 10_000.0;
        hvac.write_zone_thermal_contributions(
            &mut ports,
            gross_w,
            0.0,
            ThermalCategory::HvacHeating,
        )
        .expect("write must succeed");

        // Conditioned zone: 10000 * 0.64 = 6400 W as HvacHeating
        let cond = ports.thermal.iter().find(|a| a.zone == ZoneId(1)).unwrap();
        assert!(
            (cond.sensible_for_category(ThermalCategory::HvacHeating) - 6_400.0).abs() < 1e-9,
            "conditioned zone must receive 6400 W HvacHeating, got {}",
            cond.sensible_for_category(ThermalCategory::HvacHeating)
        );

        // Merged zone 2: 10000 * 0.36 = 3600 W tagged DuctLoss (because zone ==
        // duct_zone_id and zone != conditioned_zone; the basement portion is also
        // tagged DuctLoss since we only have one merged fraction entry).
        let merged = ports.thermal.iter().find(|a| a.zone == ZoneId(2)).unwrap();
        assert!(
            (merged.sensible_for_category(ThermalCategory::DuctLoss) - 3_600.0).abs() < 1e-9,
            "merged zone 2 must receive 3600 W as DuctLoss, got {}",
            merged.sensible_for_category(ThermalCategory::DuctLoss)
        );

        // Total watts are physically correct: 6400 + 3600 = 10000
        let total = cond.sensible_gain_w + merged.sensible_gain_w;
        assert!(
            (total - gross_w).abs() < 1e-9,
            "total watts must equal gross capacity {gross_w}, got {total}"
        );
    }

    /// Cooling system with finished basement: basement_heat_frac should be 0.0
    /// (cooling doesn't route to basement). Verify no basement zone entry.
    #[test]
    fn cooling_finished_basement_no_basement_frac() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::AcCooler, ZoneId(1));
        hvac.config.duct_dse = 0.85;
        hvac.config.basement_heat_frac = 0.0; // cooling systems don't route to basement
        hvac.config.basement_zone_id = Some(ZoneId(2));
        hvac.config.duct_zone_id = Some(ZoneId(3));
        hvac.update_zone_heat_fractions();

        let has_basement = hvac
            .config
            .zone_heat_fractions
            .iter()
            .any(|&(z, _)| z == ZoneId(2));
        assert!(
            !has_basement,
            "cooling with basement_heat_frac=0 must not route to basement; got {:?}",
            hvac.config.zone_heat_fractions
        );
        assert_eq!(
            hvac.config.zone_heat_fractions.len(),
            2,
            "expected conditioned + duct zones only; got {:?}",
            hvac.config.zone_heat_fractions
        );
    }

    /// After update_zone_heat_fractions with duct zone, rebuild_thermal_ports
    /// must include the duct zone in port declarations.
    #[test]
    fn port_declaration_includes_duct_zone_after_init() {
        use hares_types::{PortDeclaration, PortType};

        let mut hvac = make_hvac();
        hvac.config.duct_dse = 0.8;
        hvac.config.duct_zone_id = Some(ZoneId(3));
        hvac.update_zone_heat_fractions();

        let mut ports = vec![
            PortDeclaration::electrical(),
            PortDeclaration::thermal(ZoneId(1)),
        ];
        hvac.rebuild_thermal_ports(&mut ports, true);

        // Should have electrical + thermal(1) + thermal(3)
        let thermal_ports: Vec<_> = ports
            .iter()
            .filter(|p| p.port_type == PortType::Thermal)
            .collect();
        assert_eq!(
            thermal_ports.len(),
            2,
            "must have thermal ports for conditioned + duct zone; got {:?}",
            thermal_ports
        );
        assert!(
            thermal_ports.iter().any(|p| p.zone == Some(ZoneId(1))),
            "conditioned zone thermal port missing"
        );
        assert!(
            thermal_ports.iter().any(|p| p.zone == Some(ZoneId(3))),
            "duct zone thermal port missing"
        );
        // Non-thermal ports must be preserved.
        assert!(
            ports.iter().any(|p| p.port_type == PortType::Electrical),
            "electrical port must be preserved after rebuild"
        );
    }

    /// rebuild_thermal_ports with needs_humidity=false must produce only
    /// Thermal declarations — no Humidity entries.
    #[test]
    fn rebuild_thermal_ports_without_humidity_omits_humidity_declarations() {
        use hares_types::{PortDeclaration, PortType};

        let mut hvac = make_hvac();
        hvac.config.duct_dse = 1.0;
        hvac.update_zone_heat_fractions();

        let mut ports = vec![
            PortDeclaration::electrical(),
            PortDeclaration::thermal(ZoneId(1)),
        ];
        hvac.rebuild_thermal_ports(&mut ports, false);

        let has_humidity = ports.iter().any(|p| p.port_type == PortType::Humidity);
        assert!(
            !has_humidity,
            "needs_humidity=false must not produce Humidity declarations; got {:?}",
            ports
        );

        let thermal_count = ports
            .iter()
            .filter(|p| p.port_type == PortType::Thermal)
            .count();
        assert_eq!(
            thermal_count, 1,
            "must retain exactly one Thermal declaration for the conditioned zone"
        );
    }

    /// rebuild_thermal_ports with needs_humidity=true must include Humidity
    /// declarations alongside Thermal for every zone_heat_fractions entry.
    #[test]
    fn rebuild_thermal_ports_with_humidity_includes_humidity_declarations() {
        use hares_types::{PortDeclaration, PortType};

        let mut hvac = make_hvac();
        hvac.config.duct_dse = 0.85;
        hvac.config.duct_zone_id = Some(ZoneId(3));
        hvac.update_zone_heat_fractions();

        let mut ports = vec![
            PortDeclaration::electrical(),
            PortDeclaration::thermal(ZoneId(1)),
        ];
        hvac.rebuild_thermal_ports(&mut ports, true);

        let humidity_zones: Vec<_> = ports
            .iter()
            .filter(|p| p.port_type == PortType::Humidity)
            .map(|p| p.zone)
            .collect();
        assert_eq!(
            humidity_zones.len(),
            2,
            "must have Humidity for conditioned + duct zone; got {:?}",
            humidity_zones
        );
        assert!(
            humidity_zones.contains(&Some(ZoneId(1))),
            "conditioned zone humidity port missing"
        );
        assert!(
            humidity_zones.contains(&Some(ZoneId(3))),
            "duct zone humidity port missing"
        );

        let thermal_count = ports
            .iter()
            .filter(|p| p.port_type == PortType::Thermal)
            .count();
        assert_eq!(
            thermal_count, 2,
            "must have Thermal for both zones alongside Humidity"
        );
    }

    /// Regression test: every HvacEquipmentType that is known to write
    /// PortContribution::Humidity passes needs_humidity=true (and therefore
    /// produces Humidity port declarations). Every type that does not write
    /// Humidity passes false and produces no Humidity declarations.
    #[test]
    fn hvac_equipment_types_declare_humidity_only_when_writing_it() {
        use hares_types::{PortDeclaration, PortType};

        use super::super::hvac_core::HvacEquipmentType;

        // Equipment types that write PortContribution::Humidity (air conditioners
        // and heat pump coolers). All other HVAC equipment is heating-only and
        // does not write humidity.
        let humidity_writers: &[HvacEquipmentType] = &[
            HvacEquipmentType::AcCooler,
            HvacEquipmentType::MiniSplitCool,
            HvacEquipmentType::GshpHeatPumpCooling,
            HvacEquipmentType::WshpHeatPumpCooling,
        ];

        // Heating-only types that call rebuild_thermal_ports (via furnace, boiler,
        // or heat pump heater init). They never write PortContribution::Humidity.
        let non_writers: &[HvacEquipmentType] = &[
            HvacEquipmentType::GasFurnace,
            HvacEquipmentType::ElectricFurnace,
            HvacEquipmentType::AshpHeatPumpOnly,
            HvacEquipmentType::AshpHeatPumpAux,
            HvacEquipmentType::MiniSplitHeat,
            HvacEquipmentType::GshpHeatPumpHeating,
            HvacEquipmentType::WshpHeatPumpHeating,
            HvacEquipmentType::Baseboard,
            HvacEquipmentType::Other,
        ];

        for &eq_type in humidity_writers {
            let mut hvac = HvacEquipment::new(eq_type, ZoneId(1));
            hvac.config.zone_heat_fractions = vec![(ZoneId(1), 1.0)];
            let mut ports = vec![PortDeclaration::thermal(ZoneId(1))];
            hvac.rebuild_thermal_ports(&mut ports, true);

            let has_humidity = ports.iter().any(|p| p.port_type == PortType::Humidity);
            assert!(
                has_humidity,
                "{eq_type:?} writes Humidity but humidity port declaration missing"
            );
        }

        for &eq_type in non_writers {
            let mut hvac = HvacEquipment::new(eq_type, ZoneId(1));
            hvac.config.zone_heat_fractions = vec![(ZoneId(1), 1.0)];
            let mut ports = vec![PortDeclaration::thermal(ZoneId(1))];
            hvac.rebuild_thermal_ports(&mut ports, false);

            let has_humidity = ports.iter().any(|p| p.port_type == PortType::Humidity);
            assert!(
                !has_humidity,
                "{eq_type:?} does not write Humidity but humidity port declaration present"
            );
        }
    }
}
