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
    pub(super) fn apply_port_convective_inputs(&self, u: &mut DVector<f64>, ports: &PortSlots) {
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
    /// have `solar_absorptance = 0.0` set at construction (solver_builder.rs),
    /// so their weight is zero and all their portion flows to zone air.
    /// Kirchhoff's law: thermal absorptance ≈ emissivity for opaque surfaces
    /// in the LW band.
    ///
    /// Uses `interior_lwr_zones` surfaces when available (ScriptF mode),
    /// falls back to `interior_solar_zones` surfaces (StarMesh mode).
    /// Distributes radiant gains from all zones (not only the configured
    /// indoor zone). Equipment in non-indoor zones (basement, garage, attic)
    /// must have its radiant fraction delivered to the host zone's surfaces
    /// and air node, matching the zone-scoped distribution semantics of
    /// `apply_port_convective_inputs`.
    ///
    /// Per-entry distribution is correct because TMULT weighting is linear:
    /// distributing each entry's radiant gain independently and accumulating
    /// via `+=` into the surface and air-node inputs produces the same result
    /// as summing per-zone first.  This avoids a per-timestep `Vec` allocation
    /// for zone aggregation.
    ///
    /// EnergyPlus Eng. Ref. "Inside Heat Balance": *"The radiative part is
    /// then distributed over the surfaces within the zone in some prescribed
    /// manner."*  ASHRAE HoF 2021 Ch. 18 §2 confirms that both convective and
    /// radiant fractions of internal gains belong to the zone where the
    /// equipment resides.
    pub(super) fn apply_port_radiant_inputs(&mut self, u: &mut DVector<f64>, ports: &PortSlots) {
        for thermal in &ports.thermal {
            let radiant_w = thermal.radiant_gain_w;
            if radiant_w <= 0.0 {
                continue;
            }

            let zone_id = thermal.zone;
            let zone_air_idx = self
                .wiring
                .zone_sensible_input_indices
                .get(&zone_id)
                .copied();

            // Prefer interior_lwr_zones (ScriptF mode, has full surface info
            // with emissivity and driving_temp), fall back to
            // interior_solar_zones (StarMesh mode, has area/absorptance/
            // radiation_frac).
            if let Some(zone_cfg) = self
                .config
                .interior_lwr_zones
                .iter()
                .find(|z| z.zone_id == zone_id)
            {
                distribute_radiant_lwr_surfaces(
                    u,
                    radiant_w,
                    zone_air_idx,
                    &zone_cfg.surfaces,
                    &mut self.radiant_weights_buf,
                );
                continue;
            }

            if let Some(zone_cfg) = self
                .config
                .interior_solar_zones
                .iter()
                .find(|z| z.zone_id == zone_id)
            {
                distribute_radiant_solar_surfaces(
                    u,
                    radiant_w,
                    zone_air_idx,
                    &zone_cfg.surfaces,
                    &mut self.radiant_weights_buf,
                );
                continue;
            }

            // No surface info for this zone: dump all radiant gain to zone air.
            if let Some(idx) = zone_air_idx {
                if idx < u.len() {
                    u[idx] += radiant_w;
                }
            }
        }
    }
}

/// Distribute radiant gains using InteriorSurfaceInfo (ScriptF/LWR path).
fn distribute_radiant_lwr_surfaces(
    u: &mut DVector<f64>,
    total_radiant_w: f64,
    zone_air_idx: Option<usize>,
    surfaces: &[super::config::InteriorSurfaceInfo],
    buf: &mut Vec<f64>,
) {
    buf.clear();
    buf.resize(surfaces.len(), 0.0);

    let mut total_weight = 0.0;
    for (i, s) in surfaces.iter().enumerate() {
        let w = if s.driving_temp.is_none() {
            s.area_m2 * s.emissivity
        } else {
            0.0
        };
        buf[i] = w;
        total_weight += w;
    }

    let mut air_from_radiant = 0.0;
    if total_weight > 0.0 {
        for (s, &w) in surfaces.iter().zip(buf.iter()) {
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

    if let Some(idx) = zone_air_idx {
        if idx < u.len() {
            u[idx] += air_from_radiant;
        }
    }
}

/// Distribute radiant gains using InteriorSolarSurfaceInfo (StarMesh/solar path).
///
/// Uses solar_absorptance as a proxy for thermal absorptance (Kirchhoff's
/// law: α_thermal ≈ ε for opaque surfaces in the LW band). Windows have
/// solar_absorptance = 0.0 set at construction (see solver_builder.rs),
/// so they receive zero weight in the radiant distribution. The input_index
/// field is always Some(zone_air_idx); zero absorptance is the exclusion
/// mechanism, not a None index.
fn distribute_radiant_solar_surfaces(
    u: &mut DVector<f64>,
    total_radiant_w: f64,
    zone_air_idx: Option<usize>,
    surfaces: &[super::config::InteriorSolarSurfaceInfo],
    buf: &mut Vec<f64>,
) {
    buf.clear();
    buf.resize(surfaces.len(), 0.0);

    let mut total_weight = 0.0;
    for (i, s) in surfaces.iter().enumerate() {
        // Windows have solar_absorptance = 0.0 set at construction
        // (solver_builder.rs), producing zero weight. The input_index guard
        // is defensive — in production input_index is always Some(zone_air_idx)
        // — but is retained to avoid injecting into an out-of-range index if
        // the struct is ever constructed outside solver_builder.rs.
        let w = if s.input_index.is_some() {
            s.area_m2 * s.solar_absorptance
        } else {
            0.0
        };
        buf[i] = w;
        total_weight += w;
    }

    let mut air_from_radiant = 0.0;
    if total_weight > 0.0 {
        for (s, &w) in surfaces.iter().zip(buf.iter()) {
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

    if let Some(idx) = zone_air_idx {
        if idx < u.len() {
            u[idx] += air_from_radiant;
        }
    }
}
