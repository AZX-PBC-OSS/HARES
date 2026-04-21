//! Port-to-input-vector aggregation for the thermal solver.
//!
//! Reads equipment thermal contributions from [`PortSlots`] and writes the
//! per-zone sensible and radiant gains into the state-space input vector.

use hares_types::PortSlots;
use nalgebra::DVector;

use super::ThermalSolver;

impl ThermalSolver {
    /// Accumulates per-zone convective sensible heat gains from equipment ports
    /// into the input vector at the corresponding zone sensible input indices.
    pub(super) fn apply_port_sensible_inputs(&self, u: &mut DVector<f64>, ports: &PortSlots) {
        for thermal in &ports.thermal {
            if let Some(&idx) = self.wiring.zone_sensible_input_indices.get(&thermal.zone)
                && idx < u.len()
            {
                u[idx] += thermal.sensible_gain_w;
            }
        }
    }

    /// Distributes radiant sensible gains to interior surface nodes using
    /// thermal-absorptance-weighted area fractions (E+ Eng.Ref "Zone Internal
    /// Gains" TMULT method). Opaque surfaces: radiant × radiation_frac →
    /// surface node, radiant × (1-radiation_frac) → zone air. Window surfaces
    /// (driving_temp.is_some()): all radiant → zone air (no thermal mass to
    /// absorb radiant gain). Kirchhoff's law: thermal absorptance ≈ emissivity
    /// for opaque surfaces in the LW band.
    pub(super) fn apply_port_radiant_inputs(&self, u: &mut DVector<f64>, ports: &PortSlots) {
        let indoor_zone = self.config.indoor_zone_id;
        let total_radiant_w: f64 = ports
            .thermal
            .iter()
            .filter(|t| t.zone == indoor_zone)
            .map(|t| t.radiant_gain_w)
            .sum();

        if total_radiant_w <= 0.0 {
            return;
        }

        let zone_cfg = self
            .config
            .interior_lwr_zones
            .iter()
            .find(|z| z.zone_id == indoor_zone);
        let Some(zone_cfg) = zone_cfg else {
            if let Some(&idx) = self.wiring.zone_sensible_input_indices.get(&indoor_zone) {
                u[idx] += total_radiant_w;
            }
            return;
        };
        let surfaces = &zone_cfg.surfaces;

        let mut total_weight = 0.0;
        let mut weights = Vec::with_capacity(surfaces.len());
        for s in surfaces.iter() {
            let w = if s.driving_temp.is_none() {
                s.area_m2 * s.emissivity
            } else {
                0.0
            };
            weights.push(w);
            total_weight += w;
        }

        let mut air_from_radiant = 0.0;
        if total_weight > 0.0 {
            for (s, &w) in surfaces.iter().zip(weights.iter()) {
                if w > 0.0 {
                    let q = total_radiant_w * w / total_weight;
                    if s.input_index < u.len() {
                        u[s.input_index] += q * s.radiation_frac;
                    }
                    air_from_radiant += q * (1.0 - s.radiation_frac);
                }
            }
        } else {
            air_from_radiant = total_radiant_w;
        }

        if let Some(&idx) = self.wiring.zone_sensible_input_indices.get(&indoor_zone) {
            if idx < u.len() {
                u[idx] += air_from_radiant;
            }
        }
    }
}
