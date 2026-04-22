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
    /// (no input_index / no RC node): all radiant → zone air (no thermal mass
    /// to absorb radiant gain). Kirchhoff's law: thermal absorptance ≈
    /// emissivity for opaque surfaces in the LW band.
    ///
    /// Uses `interior_lwr_zones` surfaces when available (ScriptF mode),
    /// falls back to `interior_solar_zones` surfaces (StarMesh mode).
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

        // Prefer interior_lwr_zones (ScriptF mode, has full surface info with
        // emissivity and driving_temp), fall back to interior_solar_zones
        // (StarMesh mode, has area/absorptance/radiation_frac).
        let lwr_zone = self
            .config
            .interior_lwr_zones
            .iter()
            .find(|z| z.zone_id == indoor_zone);

        if let Some(zone_cfg) = lwr_zone {
            self.distribute_radiant_lwr_surfaces(u, total_radiant_w, indoor_zone, &zone_cfg.surfaces);
            return;
        }

        let solar_zone = self
            .config
            .interior_solar_zones
            .iter()
            .find(|z| z.zone_id == indoor_zone);

        let Some(zone_cfg) = solar_zone else {
            // No surface info at all: dump all radiant gain to zone air.
            if let Some(&idx) = self.wiring.zone_sensible_input_indices.get(&indoor_zone) {
                u[idx] += total_radiant_w;
            }
            return;
        };

        self.distribute_radiant_solar_surfaces(u, total_radiant_w, indoor_zone, &zone_cfg.surfaces);
    }

    /// Distribute radiant gains using InteriorSurfaceInfo (ScriptF/LWR path).
    fn distribute_radiant_lwr_surfaces(
        &self,
        u: &mut DVector<f64>,
        total_radiant_w: f64,
        indoor_zone: hares_types::ZoneId,
        surfaces: &[super::config::InteriorSurfaceInfo],
    ) {
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

    /// Distribute radiant gains using InteriorSolarSurfaceInfo (StarMesh/solar path).
    ///
    /// Uses solar_absorptance as a proxy for thermal absorptance (Kirchhoff's
    /// law: α_thermal ≈ ε for opaque surfaces in the LW band). Windows have
    /// `input_index: None` and are excluded from the TMULT weighting.
    fn distribute_radiant_solar_surfaces(
        &self,
        u: &mut DVector<f64>,
        total_radiant_w: f64,
        indoor_zone: hares_types::ZoneId,
        surfaces: &[super::config::InteriorSolarSurfaceInfo],
    ) {
        let mut total_weight = 0.0;
        let mut weights = Vec::with_capacity(surfaces.len());
        for s in surfaces.iter() {
            // Windows (input_index=None) can't absorb radiant gain into an
            // RC node; skip them from the TMULT weighting.
            let w = if s.input_index.is_some() {
                s.area_m2 * s.solar_absorptance
            } else {
                0.0
            };
            weights.push(w);
            total_weight += w;
        }

        let mut air_from_radiant = 0.0;
        if total_weight > 0.0 {
            for (s, &w) in surfaces.iter().zip(weights.iter()) {
                if w > 0.0 && s.input_index.is_some() {
                    let q = total_radiant_w * w / total_weight;
                    if let Some(idx) = s.input_index {
                        if idx < u.len() {
                            u[idx] += q * s.radiation_frac;
                        }
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
