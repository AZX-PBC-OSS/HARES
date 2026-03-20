//! Thermal domain solver for the building envelope.

use std::collections::HashMap;
use std::time::Duration;

use hares_physics::air_properties::moist_air_density_kg_m3;
use hares_physics::constants::{CP_DRY_AIR_J_KG_K, KJ_TO_J, LATENT_HEAT_VAPORISATION_0C_KJ_KG};
use hares_physics::infiltration::{
    ach_infiltration, ashrae_wind_stack, ela_infiltration, natural_ventilation_flow_m3_s,
};
use hares_physics::solar::{GlazingCurve, window_iam};
use hares_types::{
    DomainId, DomainSolver, DomainUpdate, EnvironmentState, PortSlots, THERMAL, ZoneId,
};
use nalgebra::DVector;
use thiserror::Error;

use crate::longwave_radiation::{
    ExteriorSurface, InteriorSurface, exterior_longwave_w, interior_longwave_linearised_w,
    sky_view_factor,
};
use crate::state_space::{StateSpaceError, StateSpaceModel};

const H_FG_J_PER_KG: f64 = LATENT_HEAT_VAPORISATION_0C_KJ_KG * KJ_TO_J;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InfiltrationMethod {
    AshraeWindStack {
        /// Combined stack coefficient `inf_c × inf_Cs` [m³/s / K^n_i].
        /// n_stories effect is baked into this via infiltration_height during setup.
        c_s: f64,
        /// Combined wind coefficient `inf_c × inf_Cw` [m³/s / (m/s)^(2·n_i)].
        c_w: f64,
        /// Shelter factor (`inf_sft` in OCHRE) [0, 1].
        shielding_coeff: f64,
        /// Pressure exponent in [0.5, 0.7]. 0.65 = typical residential (OCHRE default).
        n_i: f64,
    },
    Ela {
        ela_m2: f64,
        /// ELA stack coefficient [L/(s·cm⁴·K)].
        stack_coeff: f64,
        /// ELA wind coefficient [L/(s·cm⁴·(m/s)²)].
        wind_coeff: f64,
    },
    Ach {
        ach: f64,
    },
}

impl Default for InfiltrationMethod {
    fn default() -> Self {
        Self::Ach { ach: 0.0 }
    }
}

#[derive(Debug, Clone, Default)]
pub struct VentilationConfig {
    pub zone_flow_m3_s: HashMap<ZoneId, f64>,
    pub supply_temp_c: Option<f64>,
    pub supply_humidity_ratio: Option<f64>,
}

/// Configuration for natural ventilation through operable windows.
///
/// Implements the OCHRE/ResStock model: flow is driven by stack effect and wind
/// through operable windows, gated by temperature and outdoor humidity conditions.
///
/// # Defaults
/// - `t_base_c`: 22.778 °C (73 °F) — OCHRE default comfort base temperature
/// - `max_outdoor_humidity_ratio`: 0.0115 kg/kg — Building America HSP threshold
/// - `OPEN_AREA_FRACTION`: 0.067 — matches OCHRE (0.67 × 0.5 × 0.2 of total window area)
#[derive(Debug, Clone, PartialEq)]
pub struct NaturalVentilationConfig {
    /// Effective operable window area [m²].
    ///
    /// Typically computed as `total_window_area_m2 * OPEN_AREA_FRACTION`.
    /// Use [`NaturalVentilationConfig::from_window_area`] to apply the standard fraction.
    pub open_area_m2: f64,
    /// ELA stack coefficient [L/(s·cm⁴·K)] — same table as infiltration ELA coefficients.
    pub stack_coeff: f64,
    /// ELA wind coefficient [L/(s·cm⁴·(m/s)²)] — same table as infiltration ELA coefficients.
    pub wind_coeff: f64,
    /// Comfort base temperature [°C]. Flow is suppressed when `T_zone ≤ t_base_c`.
    pub t_base_c: f64,
    /// Maximum outdoor specific humidity [kg/kg] above which nat vent is suppressed.
    pub max_outdoor_humidity_ratio: f64,
}

impl NaturalVentilationConfig {
    /// OCHRE default open-window fraction of total window area (0.67 × 0.5 × 0.2).
    pub const OPEN_AREA_FRACTION: f64 = 0.067;
    /// OCHRE default comfort base temperature (73 °F in °C).
    pub const DEFAULT_T_BASE_C: f64 = 22.778;
    /// Building America HSP outdoor humidity threshold [kg/kg].
    pub const DEFAULT_MAX_OUTDOOR_HUMIDITY_RATIO: f64 = 0.0115;

    /// Construct from total window area; applies the standard 6.7% open-area fraction.
    pub fn from_window_area(total_window_area_m2: f64, stack_coeff: f64, wind_coeff: f64) -> Self {
        Self {
            open_area_m2: total_window_area_m2 * Self::OPEN_AREA_FRACTION,
            stack_coeff,
            wind_coeff,
            t_base_c: Self::DEFAULT_T_BASE_C,
            max_outdoor_humidity_ratio: Self::DEFAULT_MAX_OUTDOOR_HUMIDITY_RATIO,
        }
    }
}

/// Solar properties for an individual window surface.
///
/// Used by the thermal solver to apply angle-of-incidence (IAM) corrections to
/// window solar gains, following the EnergyPlus angular transmittance model.
/// Construct the matching [`GlazingCurve`] via [`GlazingCurve::from_u_shgc`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowSolarProperties {
    /// Solar heat gain coefficient at normal incidence [dimensionless].
    pub shgc: f64,
    /// Window U-factor [W/(m²·K)], used to select the EnergyPlus glazing curve.
    pub u_factor_w_m2_k: f64,
    /// Glazing area [m²].
    pub area_m2: f64,
}

/// One interior surface participating in intra-zone longwave radiation exchange.
///
/// Each entry describes a surface node within a zone (e.g. ceiling, floor, wall).
/// The linearised longwave heat flux is accumulated into `input_index` of the
/// solver's input vector `u` each timestep.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InteriorSurfaceInfo {
    /// Index into the state vector `x` for the RC node approximating this surface's
    /// temperature.  In a lumped-zone model use the zone air state index; the net
    /// exchange then collapses to zero because all surfaces share the same temperature.
    pub state_index: usize,
    /// Index into the input vector `u` where the net LW heat flux [W] is added.
    pub input_index: usize,
    /// Surface area [m²].
    pub area_m2: f64,
    /// Longwave emissivity [-].
    pub emissivity: f64,
}

/// Interior longwave radiation configuration for one zone.
///
/// Surfaces listed here participate in area-and-emissivity-weighted linearised
/// interior LW exchange each timestep.  An empty `surfaces` list disables interior
/// LW for this zone (backward-compatible default).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InteriorLwrZoneConfig {
    /// Zone this configuration applies to.
    pub zone_id: ZoneId,
    /// Surfaces in this zone participating in interior LW exchange.
    pub surfaces: Vec<InteriorSurfaceInfo>,
}

/// Metadata for one exterior surface used to compute longwave radiation at runtime.
///
/// The sky view factor is derived from `tilt_deg` on each call via [`sky_view_factor`];
/// it is not cached here so that the struct remains `Copy` and free of init ordering.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExteriorSurfaceInfo {
    /// Unique surface identifier matching [`SurfaceIrradiance::surface_id`] in weather data.
    pub surface_id: u32,
    /// Index into the state vector `x` for the RC node whose temperature approximates this surface.
    pub state_index: usize,
    /// Index into the input vector `u` where the longwave flux [W] is accumulated.
    pub input_index: usize,
    /// Surface area [m²].
    pub area_m2: f64,
    /// Longwave emissivity [-]; use [`EMISSIVITY_DEFAULT`] (0.90) for opaque surfaces.
    pub emissivity: f64,
    /// Surface tilt from horizontal [°]; 0° = horizontal roof, 90° = vertical wall.
    pub tilt_deg: f64,
}

#[derive(Debug, Clone, Default)]
pub struct ThermalSolverConfig {
    pub zone_state_indices: HashMap<ZoneId, usize>,
    pub zone_output_indices: HashMap<ZoneId, usize>,
    pub zone_sensible_input_indices: HashMap<ZoneId, usize>,
    pub outdoor_temp_input_indices: Vec<usize>,
    pub indoor_temp_input_indices: Vec<usize>,
    pub solar_input_indices: HashMap<u32, usize>,
    /// Window solar properties keyed by surface_id.
    ///
    /// When a surface_id appears in both `solar_input_indices` and `window_properties`,
    /// the thermal solver applies the EnergyPlus IAM correction (angle-of-incidence
    /// modifier) to beam radiation and a hemispherical average IAM to diffuse and
    /// reflected radiation before multiplying by SHGC and area.
    ///
    /// Surfaces absent from this map are treated as opaque and receive raw POA
    /// irradiance with no SHGC or IAM correction (backward-compatible behaviour).
    pub window_properties: HashMap<u32, WindowSolarProperties>,
    /// Exterior surfaces for longwave radiation; empty = no LW correction (backward compatible).
    pub exterior_surfaces: Vec<ExteriorSurfaceInfo>,
    /// Per-zone interior surface configurations for intra-zone LW radiation.
    ///
    /// Empty = no interior LW correction (backward-compatible default).
    /// When populated, the solver calls [`interior_longwave_linearised_w`] for each zone
    /// using the current zone air temperature as the linearisation point and accumulates
    /// the net surface fluxes into the corresponding `input_index` entries in `u`.
    pub interior_lwr_zones: Vec<InteriorLwrZoneConfig>,
    pub ideal_setpoints_c: HashMap<ZoneId, f64>,
    pub ideal_hvac_zones: Vec<ZoneId>,
    pub infiltration: InfiltrationMethod,
    pub ventilation_flow_m3_s: f64,
    pub ventilation: VentilationConfig,
    /// Natural ventilation through operable windows. `None` disables the feature (default).
    pub natural_ventilation: Option<NaturalVentilationConfig>,
}

#[derive(Debug, Error)]
pub enum ThermalSolverError {
    #[error("invalid zone mapping: zone {zone:?} is missing from {field}")]
    MissingZoneMapping { zone: ZoneId, field: &'static str },
    #[error("failed to initialize steady-state vector: {0}")]
    Initialization(String),
}

pub type Result<T> = std::result::Result<T, ThermalSolverError>;

#[derive(Debug, Clone)]
pub struct ThermalSolver {
    model: StateSpaceModel,
    config: ThermalSolverConfig,
    /// Timestep in seconds that the model was discretized for; runtime dt must match.
    dt_s: f64,
    x: DVector<f64>,
    last_u: DVector<f64>,
    /// Reusable input buffer: swapped out during resolve_internal to avoid per-step allocation.
    u_buf: DVector<f64>,
    /// Reusable latent-load accumulator: cleared at the start of each infiltration pass.
    latent_buf: HashMap<ZoneId, f64>,
}

impl ThermalSolver {
    pub fn state_vector(&self) -> &[f64] {
        self.x.as_slice()
    }

    pub fn config(&self) -> &ThermalSolverConfig {
        &self.config
    }

    pub fn model_dims(&self) -> (usize, usize, usize) {
        (
            self.model.a_d.nrows(),
            self.model.b_d.ncols(),
            self.model.c.nrows(),
        )
    }

    pub fn b_d_column(&self, col: usize) -> Vec<f64> {
        (0..self.model.b_d.nrows())
            .map(|r| self.model.b_d[(r, col)])
            .collect()
    }

    pub fn a_d_diagonal(&self) -> Vec<f64> {
        (0..self.model.a_d.nrows())
            .map(|i| self.model.a_d[(i, i)])
            .collect()
    }

    pub fn new(
        model: StateSpaceModel,
        config: ThermalSolverConfig,
        dt_s: f64,
        env: &EnvironmentState,
        indoor_temp_c: f64,
    ) -> Result<Self> {
        let x = initialize_steady_state(&model, &config, env, indoor_temp_c)?;
        let n_inputs = model.b_d.ncols();
        let last_u = DVector::<f64>::zeros(n_inputs);
        let u_buf = DVector::<f64>::zeros(n_inputs);
        let latent_buf = HashMap::new();

        Ok(Self {
            model,
            config,
            dt_s,
            x,
            last_u,
            u_buf,
            latent_buf,
        })
    }

    #[must_use]
    pub fn state(&self) -> &DVector<f64> {
        &self.x
    }

    /// Returns checkpointable thermal state vectors.
    #[must_use]
    pub fn snapshot_state(&self) -> (Vec<f64>, Vec<f64>) {
        (
            self.x.iter().copied().collect(),
            self.last_u.iter().copied().collect(),
        )
    }

    /// Restores thermal state vectors from checkpoint payloads.
    pub fn restore_state(&mut self, x_state: &[f64], last_u_state: &[f64]) -> Result<()> {
        if x_state.len() != self.x.len() {
            return Err(ThermalSolverError::Initialization(format!(
                "invalid x state length: got {}, expected {}",
                x_state.len(),
                self.x.len()
            )));
        }
        if !last_u_state.is_empty() && last_u_state.len() != self.last_u.len() {
            return Err(ThermalSolverError::Initialization(format!(
                "invalid last_u length: got {}, expected {}",
                last_u_state.len(),
                self.last_u.len()
            )));
        }

        self.x = DVector::from_column_slice(x_state);
        if last_u_state.is_empty() {
            self.last_u.fill(0.0);
        } else {
            self.last_u = DVector::from_column_slice(last_u_state);
        }
        Ok(())
    }

    /// Estimate the ideal HVAC capacity needed to maintain the zone setpoint.
    ///
    /// NOTE: This uses `last_u` (the previous timestep's input vector) as the
    /// background, because it is called by equipment *before* `resolve()` builds
    /// the current-step input vector. The resulting duty-cycle is therefore based
    /// on a one-step-stale load background. This matches OCHRE's behavior but may
    /// drift on transient days. A future improvement could split `resolve_internal`
    /// into background-build and solve phases.
    pub fn solve_ideal_capacity(&self, env: &EnvironmentState, zone: ZoneId) -> f64 {
        let Some(&input_idx) = self.config.zone_sensible_input_indices.get(&zone) else {
            return 0.0;
        };
        let Some(&output_idx) = self.config.zone_output_indices.get(&zone) else {
            return 0.0;
        };
        let y_target = zone_setpoint_c(&self.config, env, zone);
        solve_for_output_input(
            &self.model,
            &self.x,
            &self.last_u,
            y_target,
            output_idx,
            input_idx,
        )
        .unwrap_or(0.0)
    }

    pub fn set_ideal_hvac_zones(&mut self, zones: Vec<ZoneId>) {
        self.config.ideal_hvac_zones = zones;
    }

    fn resolve_internal(
        &mut self,
        ports: &PortSlots,
        env: &EnvironmentState,
        ideal_hvac_zones: &[ZoneId],
    ) -> DomainUpdate {
        // Swap out the reusable buffer so we can call &self methods on the rest of the struct.
        let mut u = std::mem::replace(&mut self.u_buf, DVector::zeros(0));
        let n = self.model.b_d.ncols();
        if u.len() == n {
            u.fill(0.0);
        } else {
            u = DVector::zeros(n);
        }

        self.apply_outdoor_inputs(&mut u, env);
        self.apply_solar_inputs(&mut u, env);
        self.apply_exterior_longwave_inputs(&mut u, env);
        self.apply_interior_longwave_inputs(&mut u, env);
        self.apply_port_sensible_inputs(&mut u, ports);

        // Swap out the latent buffer so we can call &self methods on the rest of the struct.
        let mut latent_by_zone = std::mem::take(&mut self.latent_buf);
        latent_by_zone.clear();
        apply_infiltration_and_ventilation(&self.config, &mut u, env, &mut latent_by_zone);

        for &zone in ideal_hvac_zones {
            let Some(&input_idx) = self.config.zone_sensible_input_indices.get(&zone) else {
                continue;
            };
            let Some(&output_idx) = self.config.zone_output_indices.get(&zone) else {
                continue;
            };
            let target = zone_setpoint_c(&self.config, env, zone);

            if let Ok(q) =
                solve_for_output_input(&self.model, &self.x, &u, target, output_idx, input_idx)
            {
                u[input_idx] = q;
            }
        }

        let x_next = self.model.step(&self.x, &u);
        let y_next = self.model.output(&x_next, &u);
        self.x = x_next;
        self.last_u.clone_from(&u);
        self.u_buf = u;

        let mut zone_temperatures_c: Vec<(ZoneId, f64)> = self
            .config
            .zone_output_indices
            .iter()
            .map(|(zone, output_idx)| (*zone, y_next[*output_idx]))
            .collect();
        zone_temperatures_c.sort_by_key(|(zone, _)| *zone);

        let mut custom_payload = Vec::with_capacity(latent_by_zone.len() * 2);
        let mut latent_pairs: Vec<(ZoneId, f64)> =
            latent_by_zone.iter().map(|(&z, &v)| (z, v)).collect();
        latent_pairs.sort_by_key(|(zone, _)| *zone);
        for (zone, latent) in latent_pairs {
            custom_payload.push(f64::from(zone.0));
            custom_payload.push(latent);
        }

        // Return the latent buffer for reuse next step.
        self.latent_buf = latent_by_zone;

        DomainUpdate {
            domain_id: THERMAL,
            zone_temperatures_c,
            custom_payload: if custom_payload.is_empty() {
                None
            } else {
                Some(custom_payload)
            },
        }
    }

    fn apply_outdoor_inputs(&self, u: &mut DVector<f64>, env: &EnvironmentState) {
        for &idx in &self.config.outdoor_temp_input_indices {
            if idx < u.len() {
                u[idx] = env.weather.outdoor_temp_c;
            }
        }
    }

    fn apply_solar_inputs(&self, u: &mut DVector<f64>, env: &EnvironmentState) {
        for irr in &env.weather.solar_irradiance {
            let Some(&idx) = self.config.solar_input_indices.get(&irr.surface_id) else {
                continue;
            };
            if idx >= u.len() {
                continue;
            }

            if let Some(win) = self.config.window_properties.get(&irr.surface_id) {
                // Window surface: apply EnergyPlus IAM correction.
                // Beam is reduced by the angle-dependent IAM; diffuse and reflected
                // use the pre-integrated hemispherical IAM constant.
                let curve = GlazingCurve::from_u_shgc(win.u_factor_w_m2_k, win.shgc);
                let iam_beam = window_iam(irr.angle_of_incidence_rad, curve);
                let iam_diffuse = curve.diffuse_iam();
                let transmitted_w = win.area_m2
                    * win.shgc
                    * (irr.direct_w_m2 * iam_beam
                        + (irr.diffuse_w_m2 + irr.reflected_w_m2) * iam_diffuse);
                u[idx] = transmitted_w;
            } else {
                // Opaque surface: raw POA irradiance, no SHGC or IAM.
                u[idx] = irr.direct_w_m2 + irr.diffuse_w_m2 + irr.reflected_w_m2;
            }
        }
    }

    fn apply_exterior_longwave_inputs(&self, u: &mut DVector<f64>, env: &EnvironmentState) {
        for info in &self.config.exterior_surfaces {
            if info.input_index >= u.len() || info.state_index >= self.x.len() {
                continue;
            }
            let t_surface_c = self.x[info.state_index];
            let surface = ExteriorSurface {
                area_m2: info.area_m2,
                emissivity: info.emissivity,
                sky_view_factor: sky_view_factor(info.tilt_deg),
            };
            let q_lw = exterior_longwave_w(
                &surface,
                env.weather.sky_temp_c,
                env.weather.ground_temp_c,
                t_surface_c,
            );
            u[info.input_index] += q_lw;
        }
    }

    fn apply_port_sensible_inputs(&self, u: &mut DVector<f64>, ports: &PortSlots) {
        for thermal in &ports.thermal {
            if let Some(&idx) = self.config.zone_sensible_input_indices.get(&thermal.zone)
                && idx < u.len()
            {
                u[idx] += thermal.sensible_gain_w;
            }
        }
    }

    /// Applies linearised interior longwave radiation exchange for each configured zone.
    ///
    /// For each zone in [`ThermalSolverConfig::interior_lwr_zones`], collects current
    /// surface temperatures from the state vector, calls [`interior_longwave_linearised_w`],
    /// and accumulates the resulting per-surface heat fluxes into `u`.
    fn apply_interior_longwave_inputs(&self, u: &mut DVector<f64>, env: &EnvironmentState) {
        for zone_cfg in &self.config.interior_lwr_zones {
            if zone_cfg.surfaces.len() < 2 {
                continue;
            }
            let t_zone_c = env
                .zones
                .iter()
                .find(|z| z.id == zone_cfg.zone_id)
                .map(|z| z.temperature_c)
                .unwrap_or(20.0);

            let surfaces: Vec<InteriorSurface> = zone_cfg
                .surfaces
                .iter()
                .map(|s| InteriorSurface {
                    area_m2: s.area_m2,
                    emissivity: s.emissivity,
                })
                .collect();
            let t_surfaces: Vec<f64> = zone_cfg
                .surfaces
                .iter()
                .map(|s| {
                    if s.state_index < self.x.len() {
                        self.x[s.state_index]
                    } else {
                        t_zone_c
                    }
                })
                .collect();

            let net_lw = interior_longwave_linearised_w(&surfaces, &t_surfaces, t_zone_c);
            for (info, &q) in zone_cfg.surfaces.iter().zip(net_lw.iter()) {
                if info.input_index < u.len() {
                    u[info.input_index] += q;
                }
            }
        }
    }
}

impl DomainSolver for ThermalSolver {
    fn domain_id(&self) -> DomainId {
        THERMAL
    }

    fn resolve(&mut self, ports: &PortSlots, env: &EnvironmentState, dt: Duration) -> DomainUpdate {
        debug_assert!(
            (dt.as_secs_f64() - self.dt_s).abs() < 1e-6,
            "ThermalSolver: runtime dt ({:.3}s) != configured dt ({:.3}s); re-discretize or use constant timestep",
            dt.as_secs_f64(),
            self.dt_s
        );
        // Take the vec to avoid cloning while still being able to call &mut self methods.
        let ideal_hvac_zones = std::mem::take(&mut self.config.ideal_hvac_zones);
        let result = self.resolve_internal(ports, env, &ideal_hvac_zones);
        self.config.ideal_hvac_zones = ideal_hvac_zones;
        result
    }
}

/// Returns the effective zone setpoint from solver config, falling back to the current zone air
/// temperature when no static setpoint is configured.
fn zone_setpoint_c(config: &ThermalSolverConfig, env: &EnvironmentState, zone: ZoneId) -> f64 {
    config
        .ideal_setpoints_c
        .get(&zone)
        .copied()
        .or_else(|| {
            env.zones
                .iter()
                .find(|z| z.id == zone)
                .map(|z| z.temperature_c)
        })
        .unwrap_or_default()
}

/// Populates `u` with infiltration and ventilation sensible gains and accumulates latent loads
/// into `latent_out` (which the caller has already cleared).
fn apply_infiltration_and_ventilation(
    config: &ThermalSolverConfig,
    u: &mut DVector<f64>,
    env: &EnvironmentState,
    latent_out: &mut HashMap<ZoneId, f64>,
) {
    let p_pa = env.weather.pressure_pa();
    let t_out = env.weather.outdoor_temp_c;
    let w_out = env.weather.outdoor_humidity_ratio;
    // Outdoor density is physically correct here: infiltration mass flow = Q * rho_outdoor
    // because the incoming air has outdoor properties (ASHRAE HOF 2021). The humidity solver
    // separately uses zone-side density for its moisture balance (see humidity_solver.rs).
    let rho = moist_air_density_kg_m3(p_pa, t_out, w_out);

    for zone in &env.zones {
        let q_inf_m3_s = match config.infiltration {
            InfiltrationMethod::AshraeWindStack {
                c_s,
                c_w,
                shielding_coeff,
                n_i,
            } => ashrae_wind_stack(
                c_s,
                c_w,
                t_out - zone.temperature_c,
                env.weather.wind_speed_m_s,
                shielding_coeff,
                n_i,
            ),
            InfiltrationMethod::Ela {
                ela_m2,
                stack_coeff,
                wind_coeff,
            } => ela_infiltration(
                ela_m2,
                stack_coeff,
                wind_coeff,
                t_out - zone.temperature_c,
                env.weather.wind_speed_m_s,
            ),
            InfiltrationMethod::Ach { ach } => ach_infiltration(ach, zone.volume_m3),
        };

        let m_dot_inf = rho * q_inf_m3_s;
        let q_inf_sensible = m_dot_inf * CP_DRY_AIR_J_KG_K * (t_out - zone.temperature_c);
        let q_inf_latent = m_dot_inf * H_FG_J_PER_KG * (w_out - zone.humidity_ratio);

        if let Some(&idx) = config.zone_sensible_input_indices.get(&zone.id)
            && idx < u.len()
        {
            u[idx] += q_inf_sensible;
        }
        *latent_out.entry(zone.id).or_insert(0.0) += q_inf_latent;

        let vent_flow = config
            .ventilation
            .zone_flow_m3_s
            .get(&zone.id)
            .copied()
            .unwrap_or(config.ventilation_flow_m3_s);
        if vent_flow > 0.0 {
            let t_supply = config.ventilation.supply_temp_c.unwrap_or(t_out);
            let w_supply = config.ventilation.supply_humidity_ratio.unwrap_or(w_out);
            let m_dot_vent = rho * vent_flow;
            let q_vent_sensible = m_dot_vent * CP_DRY_AIR_J_KG_K * (t_supply - zone.temperature_c);
            let q_vent_latent = m_dot_vent * H_FG_J_PER_KG * (w_supply - zone.humidity_ratio);

            if let Some(&idx) = config.zone_sensible_input_indices.get(&zone.id)
                && idx < u.len()
            {
                u[idx] += q_vent_sensible;
            }
            *latent_out.entry(zone.id).or_insert(0.0) += q_vent_latent;
        }

        if let Some(nv) = &config.natural_ventilation {
            let q_nat_m3_s = natural_ventilation_flow_m3_s(
                nv.open_area_m2,
                zone.temperature_c,
                t_out,
                nv.t_base_c,
                w_out,
                nv.max_outdoor_humidity_ratio,
                env.weather.wind_speed_m_s,
                nv.stack_coeff,
                nv.wind_coeff,
                zone.volume_m3,
            );
            if q_nat_m3_s > 0.0 {
                let m_dot_nat = rho * q_nat_m3_s;
                let q_nat_sensible = m_dot_nat * CP_DRY_AIR_J_KG_K * (t_out - zone.temperature_c);
                let q_nat_latent = m_dot_nat * H_FG_J_PER_KG * (w_out - zone.humidity_ratio);

                if let Some(&idx) = config.zone_sensible_input_indices.get(&zone.id)
                    && idx < u.len()
                {
                    u[idx] += q_nat_sensible;
                }
                *latent_out.entry(zone.id).or_insert(0.0) += q_nat_latent;
            }
        }
    }
}

/// Solves for the scalar input at `input_index` that drives output `output_index` to `y_target`
/// after one step, without cloning A_d or B_d.
fn solve_for_output_input(
    model: &StateSpaceModel,
    x: &DVector<f64>,
    u: &DVector<f64>,
    y_target: f64,
    output_index: usize,
    input_index: usize,
) -> std::result::Result<f64, StateSpaceError> {
    // Zero the target input so we can compute the baseline prediction without it.
    // We operate on a temporary scalar instead of cloning the full u vector.
    let u_i_original = u[input_index];

    // x_next_fixed = A_d * x + B_d * u_fixed  (u_fixed has u[input_index] = 0)
    // Computed as: A_d*x + B_d*u - B_d[:,input_index]*u_i
    let x_next_fixed =
        &model.a_d * x + &model.b_d * u - model.b_d.column(input_index) * u_i_original;

    // y_fixed = C * x_next_fixed + D * u_fixed
    // Use matrix-vector products rather than dot() to avoid shape mismatch between
    // row slices (1×n) and column vectors (n×1) in nalgebra's dot() check.
    let c_row = model.c.row(output_index);
    let d_row = model.d.row(output_index);
    let y_fixed = (c_row * &x_next_fixed)[0] + (d_row * u)[0] - d_row[input_index] * u_i_original;

    // effective_gain = C[output_index, :] * B_d[:, input_index] + D[output_index, input_index]
    let b_col = model.b_d.column(input_index);
    let effective_gain = (c_row * b_col)[0] + model.d[(output_index, input_index)];

    if effective_gain.abs() <= 1.0e-12 {
        return Err(StateSpaceError::ZeroEffectiveGain { input_index });
    }

    Ok((y_target - y_fixed) / effective_gain)
}

fn initialize_steady_state(
    model: &StateSpaceModel,
    config: &ThermalSolverConfig,
    env: &EnvironmentState,
    indoor_temp_c: f64,
) -> Result<DVector<f64>> {
    let mut u = DVector::<f64>::zeros(model.b_d.ncols());
    for &idx in &config.outdoor_temp_input_indices {
        if idx < u.len() {
            u[idx] = env.weather.outdoor_temp_c;
        }
    }
    for &idx in &config.indoor_temp_input_indices {
        if idx < u.len() {
            u[idx] = indoor_temp_c;
        }
    }

    // Initialize all RC nodes to the indoor temperature. The physical
    // assumption is that the building was conditioned before the simulation
    // begins. The previous approach solved for unconditioned steady state
    // (I - A_d)⁻¹ · B_d · u, which yielded outdoor temp when no HVAC input
    // was included — causing a 38°C discontinuity on the first step.
    let x = DVector::from_element(model.a_d.nrows(), indoor_temp_c);
    Ok(x)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use chrono::{TimeZone, Utc};
    use hares_types::{
        DomainSolver, EnvironmentState, GridState, PortSlots, SurfaceIrradiance,
        ThermalAccumulator, WeatherState, ZoneId, ZoneState,
    };
    use nalgebra::DMatrix;

    use crate::state_space::{OutputMapping, StateSpaceModel};
    use crate::thermal_solver::{
        ExteriorSurfaceInfo, InfiltrationMethod, NaturalVentilationConfig, ThermalSolver,
        ThermalSolverConfig, VentilationConfig, WindowSolarProperties,
    };

    fn env_for_temp(zone_temp: f64, outdoor_temp: f64) -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: zone_temp,
                humidity_ratio: 0.008,
                relative_humidity: 0.45,
                wet_bulb_c: zone_temp - 5.0,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: outdoor_temp,
                outdoor_humidity_ratio: 0.004,
                wind_speed_m_s: 3.0,
                wind_dir_deg: 180.0,
                ground_temp_c: outdoor_temp,
                sky_temp_c: outdoor_temp - 5.0,
                pressure_kpa: 101.325,
                solar_irradiance: vec![SurfaceIrradiance {
                    surface_id: 1,
                    direct_w_m2: 0.0,
                    diffuse_w_m2: 0.0,
                    reflected_w_m2: 0.0,
                    angle_of_incidence_rad: 0.0,
                }],
                outdoor_wet_bulb_c: 0.0,
                outdoor_enthalpy_j_kg: 0.0,
                ghi_w_m2: 0.0,
                dni_w_m2: 0.0,
                dhi_w_m2: 0.0,
                solar_altitude_deg: 0.0,
                mains_temp_c: 15.0,
            },
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
            },
            custom_domains: vec![],
            current_time: Utc
                .with_ymd_and_hms(2026, 3, 18, 12, 0, 0)
                .single()
                .expect("valid time"),
            time_res: chrono::Duration::seconds(60),
        }
    }

    fn one_zone_solver(env: &EnvironmentState) -> ThermalSolver {
        let r = 2.0;
        let c = 50_000.0;
        let a_c = DMatrix::from_row_slice(1, 1, &[-1.0 / (r * c)]);
        let b_c = DMatrix::from_row_slice(1, 2, &[1.0 / (r * c), 1.0 / c]); // [T_out, H_zone]
        let mapping = OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();

        let config = ThermalSolverConfig {
            zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
            outdoor_temp_input_indices: vec![0],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            window_properties: HashMap::new(),
            exterior_surfaces: vec![],
            interior_lwr_zones: vec![],
            ideal_setpoints_c: HashMap::new(),
            ideal_hvac_zones: vec![],
            infiltration: InfiltrationMethod::Ach { ach: 0.0 },
            ventilation_flow_m3_s: 0.0,
            ventilation: VentilationConfig::default(),
            natural_ventilation: None,
        };
        ThermalSolver::new(model, config, 60.0, env, env.zones[0].temperature_c).unwrap()
    }

    #[test]
    fn free_float_zone_drifts_toward_outdoor() {
        let mut env = env_for_temp(20.0, 0.0);
        let mut solver = one_zone_solver(&env);
        solver.x[0] = 20.0;
        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };

        let mut t_last = 20.0;
        for _ in 0..60 {
            let update = solver.resolve(&ports, &env, Duration::from_secs(60));
            t_last = update.zone_temperatures_c[0].1;
            env.zones[0].temperature_c = t_last;
        }
        assert!(t_last < 20.0);
        assert!(t_last > 0.0);
    }

    #[test]
    fn sinusoidal_outdoor_has_lag_and_attenuation() {
        let mut env = env_for_temp(15.0, 15.0);
        let mut solver = one_zone_solver(&env);
        solver.x[0] = 15.0;
        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };

        let dt_s = 60.0;
        let period_s = 24.0 * 3600.0;
        let omega = 2.0 * std::f64::consts::PI / period_s;
        let mut outdoor = Vec::new();
        let mut indoor = Vec::new();
        for k in 0..(24 * 60) {
            let t = (k as f64) * dt_s;
            env.weather.outdoor_temp_c = 15.0 + 10.0 * (omega * t).sin();
            outdoor.push(env.weather.outdoor_temp_c);
            let update = solver.resolve(&ports, &env, Duration::from_secs(60));
            let t_zone = update.zone_temperatures_c[0].1;
            env.zones[0].temperature_c = t_zone;
            indoor.push(t_zone);
        }

        let amp_out = (outdoor.iter().copied().fold(f64::MIN, f64::max)
            - outdoor.iter().copied().fold(f64::MAX, f64::min))
            / 2.0;
        let amp_in = (indoor.iter().copied().fold(f64::MIN, f64::max)
            - indoor.iter().copied().fold(f64::MAX, f64::min))
            / 2.0;
        assert!(amp_in < amp_out);

        let idx_out_peak = outdoor
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap()
            .0;
        let idx_in_peak = indoor
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap()
            .0;
        assert!(idx_in_peak > idx_out_peak);
    }

    #[test]
    fn domain_solver_is_object_safe() {
        let env = env_for_temp(20.0, 0.0);
        let solver = one_zone_solver(&env);
        let mut boxed: Box<dyn DomainSolver> = Box::new(solver);
        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };
        let update = boxed.resolve(&ports, &env, Duration::from_secs(60));
        assert_eq!(update.domain_id, hares_types::THERMAL);
    }

    #[test]
    fn steady_state_initialization_sets_all_nodes_to_indoor_temp() {
        let env = env_for_temp(20.0, 0.0);
        let a_c = DMatrix::from_row_slice(2, 2, &[-0.75, 0.5, 0.25, -0.28125]);
        let b_c = DMatrix::from_row_slice(2, 2, &[0.25, 0.0, 0.0, 0.03125]); // [T_out, T_in]
        let mapping = OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();
        let config = ThermalSolverConfig {
            zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
            outdoor_temp_input_indices: vec![0],
            indoor_temp_input_indices: vec![1],
            solar_input_indices: HashMap::new(),
            window_properties: HashMap::new(),
            exterior_surfaces: vec![],
            interior_lwr_zones: vec![],
            ideal_setpoints_c: HashMap::new(),
            ideal_hvac_zones: vec![],
            infiltration: InfiltrationMethod::Ach { ach: 0.0 },
            ventilation_flow_m3_s: 0.0,
            ventilation: VentilationConfig::default(),
            natural_ventilation: None,
        };
        let indoor_temp = 20.0;
        let solver = ThermalSolver::new(model, config, 60.0, &env, indoor_temp).unwrap();
        // All RC nodes initialize to the indoor temperature (conditioned assumption).
        for &val in solver.state().iter() {
            assert!(
                (val - indoor_temp).abs() <= 1e-6,
                "expected {indoor_temp}, got {val}"
            );
        }
    }

    /// Regression: latent heat constant was inconsistent across modules (2450 vs 2501 kJ/kg).
    /// Instead of pinning the constant value, verify the physical consequence:
    /// the infiltration latent load in the custom_payload must imply a physically
    /// correct h_fg when back-computed from the known air mass flow and humidity
    /// difference. With the old 2450 kJ/kg bug, the implied h_fg would fall ~2%
    /// outside the acceptable range.
    #[test]
    fn infiltration_latent_energy_consistent_with_moisture_mass_flow() {
        use hares_physics::air_properties::moist_air_density_kg_m3;
        use hares_physics::infiltration::ach_infiltration;

        let ach = 0.5;
        let w_zone = 0.008;
        let w_outdoor = 0.004;
        let t_outdoor = 5.0;
        let p_kpa = 101.325;
        let volume_m3 = 200.0;

        let mut env = env_for_temp(20.0, t_outdoor);
        env.zones[0].humidity_ratio = w_zone;
        env.weather.outdoor_humidity_ratio = w_outdoor;

        let r = 2.0;
        let c = 50_000.0;
        let a_c = DMatrix::from_row_slice(1, 1, &[-1.0 / (r * c)]);
        let b_c = DMatrix::from_row_slice(1, 2, &[1.0 / (r * c), 1.0 / c]);
        let mapping = crate::state_space::OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model =
            crate::state_space::StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping)
                .unwrap();

        let config = ThermalSolverConfig {
            zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
            outdoor_temp_input_indices: vec![0],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            window_properties: HashMap::new(),
            exterior_surfaces: vec![],
            interior_lwr_zones: vec![],
            ideal_setpoints_c: HashMap::new(),
            ideal_hvac_zones: vec![],
            infiltration: InfiltrationMethod::Ach { ach },
            ventilation_flow_m3_s: 0.0,
            ventilation: VentilationConfig::default(),
            natural_ventilation: None,
        };
        let mut solver = ThermalSolver::new(model, config, 60.0, &env, 20.0).unwrap();
        solver.x[0] = 20.0;

        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };
        let update = solver.resolve(&ports, &env, Duration::from_secs(60));

        // Extract latent load from custom_payload
        let payload = update
            .custom_payload
            .expect("infiltration with humidity diff must produce latent payload");
        assert!(payload.len() >= 2);
        let q_latent_w = payload[1];

        // Independently compute expected moisture mass flow from infiltration
        let rho_outdoor = moist_air_density_kg_m3(p_kpa * 1000.0, t_outdoor, w_outdoor);
        let q_inf_m3_s = ach_infiltration(ach, volume_m3);
        let m_dot_kg_s = rho_outdoor * q_inf_m3_s;
        let delta_w = w_outdoor - w_zone;

        // Back-compute the h_fg implied by the latent output
        let h_fg_implied = q_latent_w / (m_dot_kg_s * delta_w);

        // Must be within 0.4% of 2501 kJ/kg. The old 2450 value is ~2% off.
        assert!(
            (2_490_000.0..=2_510_000.0).contains(&h_fg_implied),
            "implied h_fg = {h_fg_implied:.0} J/kg outside [2490000, 2510000]; \
             latent heat constant is likely wrong"
        );
    }

    #[test]
    fn per_timestep_energy_balance_holds() {
        let mut env = env_for_temp(20.0, 0.0);
        let mut solver = one_zone_solver(&env);
        solver.x[0] = 20.0;
        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };

        let r = 2.0;
        let c = 50_000.0;
        let dt = 60.0;
        for _ in 0..20 {
            let t_prev = solver.state()[0];
            let update = solver.resolve(&ports, &env, Duration::from_secs(60));
            let t_next = update.zone_temperatures_c[0].1;
            let q_gain = 0.0;
            let d_e_storage = c * (t_next - t_prev) / dt;
            let q_loss = (t_prev - env.weather.outdoor_temp_c) / r;
            let lhs = (q_gain - d_e_storage - q_loss).abs();
            let rhs = f64::max(1.0, 1e-6 * q_gain.abs());
            assert!(lhs < rhs, "lhs={lhs}, rhs={rhs}");
            env.zones[0].temperature_c = t_next;
        }
    }

    // -----------------------------------------------------------------------
    // ANSI/ASHRAE Standard 140-2017 (BESTEST) Case 600 validation tests
    //
    // Reference: ANSI/ASHRAE Standard 140-2017, "Standard Method of Test
    // for the Evaluation of Building Energy Analysis Computer Programs",
    // Section 5.2.1 (Case 600: Base Case - Low Mass Building).
    //
    // These tests model the Case 600 lightweight building as a simplified
    // 2R1C RC network and verify that the thermal response matches
    // expected physics: correct time constants, bounded temperatures,
    // and steady-state energy balance.
    //
    // Case 600 parameters:
    //   - Single zone: 8m x 6m x 2.7m = 129.6 m^3
    //   - South-facing windows: 12 m^2, U = 3.0 W/(m^2-K)
    //   - Opaque walls UA ~= 40 W/K (walls + roof + floor combined)
    //   - Walls: U = 0.514 W/(m^2-K), area ~68 m^2 opaque
    //   - Roof:  U = 0.318 W/(m^2-K), area = 48 m^2
    //   - Floor: U = 0.039 W/(m^2-K), area = 48 m^2
    //   - Infiltration: 0.5 ACH
    //   - Internal gains: 200 W continuous sensible
    //   - Zone capacitance: ~1,094,000 J/K (with furniture multiplier)
    // -----------------------------------------------------------------------

    /// Build a BESTEST Case 600 simplified 2R1C model using the RC network.
    ///
    /// Returns (StateSpaceModel, capacitance, r_envelope, r_floor, ua_envelope, ua_floor).
    fn bestest_case_600_model() -> (StateSpaceModel, f64, f64, f64, f64, f64) {
        use crate::rc_network::{NodeId, RCNetwork};

        // BESTEST Case 600 building parameters
        let volume_m3 = 129.6; // 8m x 6m x 2.7m
        let rho_air = 1.2; // kg/m^3 (approximate)
        let cp_air = 1_006.0; // J/(kg-K)
        let furniture_multiplier = 7.0;
        let capacitance = volume_m3 * rho_air * cp_air * furniture_multiplier; // ~1,094,000 J/K

        // Envelope UA values (W/K)
        let ua_windows = 3.0 * 12.0; // 36 W/K
        let ua_walls = 0.514 * 68.0; // 34.95 W/K
        #[allow(clippy::approx_constant)]
        #[allow(clippy::approx_constant)]
        let ua_roof = 0.318 * 48.0; // 15.26 W/K — U-value, not 1/π — U-value, not 1/π
        let ua_envelope = ua_windows + ua_walls + ua_roof; // ~86.2 W/K (walls+roof+windows to outdoor)
        let ua_floor = 0.039 * 48.0; // 1.872 W/K (floor to ground)

        let r_envelope = 1.0 / ua_envelope;
        let r_floor = 1.0 / ua_floor;

        // Build RC network: node 0 = zone air, node 1 = outdoor, node 2 = ground
        let caps = std::collections::HashMap::from([(NodeId(0), capacitance)]);
        let resistances = std::collections::HashMap::from([
            ((NodeId(0), NodeId(1)), r_envelope),
            ((NodeId(0), NodeId(2)), r_floor),
        ]);
        let network =
            RCNetwork::from_elements(caps, resistances, vec![NodeId(1), NodeId(2)]).unwrap();
        let (a_c, b_c_rc) = network.build_matrices().unwrap();

        // Augment B_c with a sensible-gain input column (1/C per watt).
        // RC network gives B_c as 1x2 [outdoor, ground]; we add column for Q_sensible.
        let n_states = a_c.nrows();
        let n_ext = b_c_rc.ncols();
        let n_inputs = n_ext + 1; // +1 for sensible gains
        let mut b_c = DMatrix::zeros(n_states, n_inputs);
        for row in 0..n_states {
            for col in 0..n_ext {
                b_c[(row, col)] = b_c_rc[(row, col)];
            }
        }
        b_c[(0, n_ext)] = 1.0 / capacitance; // sensible gain input

        let dt = 300.0; // 5-minute timestep
        let mapping = OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, dt, &mapping).unwrap();

        (
            model,
            capacitance,
            r_envelope,
            r_floor,
            ua_envelope,
            ua_floor,
        )
    }

    /// ANSI/ASHRAE Standard 140-2017 (BESTEST) Case 600: free-float step response.
    ///
    /// Verifies the transient thermal response of a simplified Case 600
    /// building model after a cold outdoor temperature step.
    #[test]
    fn bestest_case_600_simplified_free_float_step_response() {
        use hares_physics::constants::CP_DRY_AIR_J_KG_K;
        use nalgebra::DVector;

        let (model, capacitance, r_envelope, _r_floor, ua_envelope, ua_floor) =
            bestest_case_600_model();

        let volume_m3 = 129.6;
        let ach = 0.5;
        let q_internal = 200.0; // W
        let t_outdoor = -10.0; // cold step
        let t_ground = 10.0;
        let rho_air = 1.2;

        // Time constant: tau = C / UA_total (dominant path to outdoor)
        let tau = capacitance * r_envelope;

        // Initialize at 20 C everywhere
        let mut x = DVector::from_row_slice(&[20.0]);

        // Step for 24 hours (288 steps at dt=300s)
        let n_steps = 288;
        let mut t_zone = 20.0;
        for _ in 0..n_steps {
            // Infiltration sensible load: m_dot * cp * (T_out - T_zone)
            let q_inf_m3_s = ach * volume_m3 / 3600.0;
            let m_dot = rho_air * q_inf_m3_s;
            let q_infiltration = m_dot * CP_DRY_AIR_J_KG_K * (t_outdoor - t_zone);

            let q_total = q_internal + q_infiltration;

            // Input vector: [outdoor_temp, ground_temp, sensible_gain_w]
            let u = DVector::from_row_slice(&[t_outdoor, t_ground, q_total]);
            x = model.step(&x, &u);
            let y = model.output(&x, &u);
            t_zone = y[0];
        }

        // After 24 hours (86400s), which is ~6.8 time constants (tau ~12700s),
        // the zone should be near steady state.

        // Temperature must have dropped significantly from 20 C
        assert!(
            t_zone < 15.0,
            "zone temp {t_zone:.2} C should be well below initial 20 C after 24h cold step"
        );

        // Temperature must be bounded: above outdoor temp (we have internal gains)
        assert!(
            t_zone > t_outdoor,
            "zone temp {t_zone:.2} C should be above outdoor {t_outdoor} C due to internal gains"
        );

        // Verify time constant is physically reasonable.
        // tau = C * R_envelope ~= 1,094,000 * 0.0116 ~= 12,690 s ~= 3.5 hours
        assert!(
            (3.0..=5.0).contains(&(tau / 3600.0)),
            "time constant {:.1} hours outside expected 3-5 hour range",
            tau / 3600.0
        );

        // After 5+ time constants, zone should be near steady state.
        // Compute expected steady-state analytically (approximate, ignoring
        // infiltration temperature-dependence for the check):
        // At steady state: Q_internal + Q_inf = UA_env*(T_zone - T_out) + UA_floor*(T_zone - T_ground)
        // Q_inf = m_dot * cp * (T_out - T_zone) = -m_dot*cp*(T_zone - T_out)
        // So: Q_internal = (UA_env + UA_floor + m_dot*cp) * (T_zone - T_out) + UA_floor*(T_out - T_ground)
        // Solving: T_zone = T_out + (Q_internal - UA_floor*(T_out - T_ground)) / (UA_env + UA_floor + m_dot*cp)
        let q_inf_m3_s = ach * volume_m3 / 3600.0;
        let m_dot_cp = rho_air * q_inf_m3_s * CP_DRY_AIR_J_KG_K;
        let ua_total = ua_envelope + ua_floor + m_dot_cp;
        let t_ss = t_outdoor + (q_internal - ua_floor * (t_outdoor - t_ground)) / ua_total;

        // Zone temperature should be within 1 C of analytical steady state
        assert!(
            (t_zone - t_ss).abs() < 1.0,
            "zone temp {t_zone:.2} C should be near steady-state {t_ss:.2} C after 24h (~6 tau)"
        );
    }

    /// ANSI/ASHRAE Standard 140-2017 (BESTEST) Case 600: steady-state heat balance.
    ///
    /// Runs the simplified Case 600 model to thermal equilibrium and verifies
    /// that the energy balance is satisfied:
    ///   Q_internal = Q_envelope + Q_floor + Q_infiltration
    /// where each loss term is computed from the final zone temperature.
    #[test]
    fn bestest_case_600_steady_state_heat_balance() {
        use hares_physics::constants::CP_DRY_AIR_J_KG_K;
        use nalgebra::DVector;

        let (model, _capacitance, _r_envelope, _r_floor, ua_envelope, ua_floor) =
            bestest_case_600_model();

        let volume_m3 = 129.6;
        let ach = 0.5;
        let q_internal = 200.0;
        let t_outdoor = -10.0;
        let t_ground = 10.0;
        let rho_air = 1.2;

        let mut x = DVector::from_row_slice(&[20.0]);
        let mut t_zone = 20.0;

        // Run 1200 steps (100 hours) to ensure full convergence
        for _ in 0..1200 {
            let q_inf_m3_s = ach * volume_m3 / 3600.0;
            let m_dot = rho_air * q_inf_m3_s;
            let q_infiltration = m_dot * CP_DRY_AIR_J_KG_K * (t_outdoor - t_zone);
            let q_total = q_internal + q_infiltration;

            let u = DVector::from_row_slice(&[t_outdoor, t_ground, q_total]);
            x = model.step(&x, &u);
            let y = model.output(&x, &u);
            t_zone = y[0];
        }

        // Verify convergence: step one more time and check temperature is stable
        let q_inf_m3_s = ach * volume_m3 / 3600.0;
        let m_dot = rho_air * q_inf_m3_s;
        let q_infiltration_final = m_dot * CP_DRY_AIR_J_KG_K * (t_outdoor - t_zone);
        let q_total_final = q_internal + q_infiltration_final;
        let u_final = DVector::from_row_slice(&[t_outdoor, t_ground, q_total_final]);
        let x_check = model.step(&x, &u_final);
        let y_check = model.output(&x_check, &u_final);
        let t_zone_check = y_check[0];
        assert!(
            (t_zone_check - t_zone).abs() < 1e-6,
            "not converged: T_zone changed by {:.6} C on final step",
            (t_zone_check - t_zone).abs()
        );

        // Compute heat balance terms from the converged zone temperature
        let q_envelope_loss = ua_envelope * (t_zone - t_outdoor);
        let q_floor_loss = ua_floor * (t_zone - t_ground);
        let q_inf_loss = -q_infiltration_final; // flip sign: loss is positive when zone > outdoor

        let q_total_loss = q_envelope_loss + q_floor_loss + q_inf_loss;

        // Energy balance: Q_internal = Q_total_loss at steady state.
        // The discrete-time model introduces a small O(dt^2) error, so we allow
        // a tolerance proportional to the gain magnitude.
        let balance_error = (q_internal - q_total_loss).abs();
        let tolerance = 0.01 * q_internal; // 1% of internal gains
        assert!(
            balance_error < tolerance,
            "steady-state energy balance violated: Q_internal={q_internal:.1} W, \
             Q_loss={q_total_loss:.1} W (envelope={q_envelope_loss:.1}, \
             floor={q_floor_loss:.1}, infiltration={q_inf_loss:.1}), \
             error={balance_error:.3} W > tolerance={tolerance:.3} W"
        );

        // Sanity: zone temperature should be between outdoor and indoor start
        assert!(
            t_zone > t_outdoor && t_zone < 20.0,
            "steady-state zone temp {t_zone:.2} C is out of expected range"
        );
    }

    // -----------------------------------------------------------------------
    // Helper: build a one-zone ThermalSolver with a configurable infiltration
    // method but otherwise identical RC parameters (R=2, C=50_000).
    // -----------------------------------------------------------------------
    fn solver_with_infiltration(
        env: &EnvironmentState,
        method: InfiltrationMethod,
    ) -> ThermalSolver {
        let r = 2.0;
        let c = 50_000.0;
        let a_c = DMatrix::from_row_slice(1, 1, &[-1.0 / (r * c)]);
        let b_c = DMatrix::from_row_slice(1, 2, &[1.0 / (r * c), 1.0 / c]);
        let mapping = crate::state_space::OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();
        let config = ThermalSolverConfig {
            zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
            outdoor_temp_input_indices: vec![0],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            window_properties: HashMap::new(),
            exterior_surfaces: vec![],
            interior_lwr_zones: vec![],
            ideal_setpoints_c: HashMap::new(),
            ideal_hvac_zones: vec![],
            infiltration: method,
            ventilation_flow_m3_s: 0.0,
            ventilation: VentilationConfig::default(),
            natural_ventilation: None,
        };
        let mut solver =
            ThermalSolver::new(model, config, 60.0, env, env.zones[0].temperature_c).unwrap();
        // Pin state to zone temp so the infiltration delta-T is deterministic.
        solver.x[0] = env.zones[0].temperature_c;
        solver
    }

    /// ASHRAE wind+stack infiltration with a warm zone and cold outdoor must
    /// cool the zone compared to a zero-infiltration baseline.
    #[test]
    fn ashrae_wind_stack_infiltration_changes_zone_temperature() {
        let zone_temp = 22.0;
        let outdoor_temp = 5.0;
        let env = env_for_temp(zone_temp, outdoor_temp);

        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };

        let mut solver_no_inf =
            solver_with_infiltration(&env, InfiltrationMethod::Ach { ach: 0.0 });
        let t_no_inf = solver_no_inf
            .resolve(&ports, &env, Duration::from_secs(60))
            .zone_temperatures_c[0]
            .1;

        // OCHRE residential defaults: Cs=0.000290, Cw=0.000231 (ASHRAE HOF Ch.16 1-story).
        let mut solver_inf = solver_with_infiltration(
            &env,
            InfiltrationMethod::AshraeWindStack {
                c_s: 0.000_290,
                c_w: 0.000_231,
                shielding_coeff: 0.7,
                n_i: 0.65,
            },
        );
        let t_with_inf = solver_inf
            .resolve(&ports, &env, Duration::from_secs(60))
            .zone_temperatures_c[0]
            .1;

        // Infiltration draws cold outdoor air in, so zone must be cooler.
        assert!(
            t_with_inf < t_no_inf,
            "ASHRAE wind+stack should cool zone: t_inf={t_with_inf:.4}, t_no_inf={t_no_inf:.4}"
        );
        let delta = t_no_inf - t_with_inf;
        assert!(
            delta > 1e-3,
            "infiltration effect too small: delta={delta:.6} C"
        );
    }

    /// ELA infiltration with non-zero drivers must also shift zone temperature
    /// toward the colder outdoor value vs a zero-infiltration baseline.
    #[test]
    fn ela_infiltration_changes_zone_temperature() {
        let zone_temp = 22.0;
        let outdoor_temp = 5.0;
        let env = env_for_temp(zone_temp, outdoor_temp);

        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };

        let mut solver_no_inf =
            solver_with_infiltration(&env, InfiltrationMethod::Ach { ach: 0.0 });
        let t_no_inf = solver_no_inf
            .resolve(&ports, &env, Duration::from_secs(60))
            .zone_temperatures_c[0]
            .1;

        // ELA ~0.01 m² is a moderately leaky 200 m³ residential zone.
        let mut solver_ela = solver_with_infiltration(
            &env,
            InfiltrationMethod::Ela {
                ela_m2: 0.01,
                stack_coeff: 0.000_145,
                wind_coeff: 0.000_087,
            },
        );
        let t_ela = solver_ela
            .resolve(&ports, &env, Duration::from_secs(60))
            .zone_temperatures_c[0]
            .1;

        assert!(
            t_ela < t_no_inf,
            "ELA infiltration should cool zone: t_ela={t_ela:.4}, t_no_inf={t_no_inf:.4}"
        );
        let delta = t_no_inf - t_ela;
        assert!(delta > 1e-3, "ELA effect too small: delta={delta:.6} C");
    }

    /// solve_ideal_capacity must return a load Q such that, when injected as a
    /// port sensible gain and stepped, the zone output reaches the setpoint to
    /// within 0.05 K (discrete-time one-step residual tolerance).
    #[test]
    fn solve_ideal_capacity_drives_zone_to_setpoint() {
        let zone_temp = 18.0; // below setpoint
        let outdoor_temp = -5.0;
        let setpoint_c = 21.0;
        let env = env_for_temp(zone_temp, outdoor_temp);

        let r = 2.0;
        let c = 50_000.0;
        let a_c = DMatrix::from_row_slice(1, 1, &[-1.0 / (r * c)]);
        let b_c = DMatrix::from_row_slice(1, 2, &[1.0 / (r * c), 1.0 / c]);
        let mapping = crate::state_space::OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();
        let config = ThermalSolverConfig {
            zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
            outdoor_temp_input_indices: vec![0],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            window_properties: HashMap::new(),
            exterior_surfaces: vec![],
            interior_lwr_zones: vec![],
            ideal_setpoints_c: HashMap::from([(ZoneId(1), setpoint_c)]),
            ideal_hvac_zones: vec![],
            infiltration: InfiltrationMethod::Ach { ach: 0.0 },
            ventilation_flow_m3_s: 0.0,
            ventilation: VentilationConfig::default(),
            natural_ventilation: None,
        };
        let mut solver = ThermalSolver::new(model, config, 60.0, &env, zone_temp).unwrap();
        solver.x[0] = zone_temp;

        let q_ideal = solver.solve_ideal_capacity(&env, ZoneId(1));

        // We are below setpoint so heating load must be positive.
        assert!(
            q_ideal > 0.0,
            "ideal capacity for heating must be positive: q={q_ideal:.1}"
        );

        // Inject the computed load and step — zone should reach setpoint.
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };
        ports.thermal[0].sensible_gain_w = q_ideal;
        let update = solver.resolve(&ports, &env, Duration::from_secs(60));
        let t_after = update.zone_temperatures_c[0].1;

        assert!(
            (t_after - setpoint_c).abs() < 0.05,
            "zone should reach setpoint after ideal load: t={t_after:.4}, setpoint={setpoint_c}"
        );
    }

    /// Non-zero solar irradiance routed through solar_input_indices must raise
    /// zone temperature compared to an identical solver with zero irradiance.
    #[test]
    fn solar_input_raises_zone_temperature() {
        let zone_temp = 18.0;
        let outdoor_temp = 5.0;

        let r = 2.0;
        let c = 50_000.0;
        // B has 3 columns: [outdoor_temp, sensible_gain, solar_gain]
        let a_c = DMatrix::from_row_slice(1, 1, &[-1.0 / (r * c)]);
        let b_c = DMatrix::from_row_slice(1, 3, &[1.0 / (r * c), 1.0 / c, 1.0 / c]);
        let mapping = crate::state_space::OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();

        let make_env = |solar_w_m2: f64| -> EnvironmentState {
            EnvironmentState {
                zones: vec![ZoneState {
                    id: ZoneId(1),
                    temperature_c: zone_temp,
                    humidity_ratio: 0.008,
                    relative_humidity: 0.45,
                    wet_bulb_c: zone_temp - 5.0,
                    volume_m3: 200.0,
                }],
                weather: WeatherState {
                    outdoor_temp_c: outdoor_temp,
                    outdoor_humidity_ratio: 0.004,
                    wind_speed_m_s: 0.0,
                    wind_dir_deg: 0.0,
                    ground_temp_c: outdoor_temp,
                    sky_temp_c: outdoor_temp - 5.0,
                    pressure_kpa: 101.325,
                    solar_irradiance: vec![SurfaceIrradiance {
                        surface_id: 42,
                        direct_w_m2: solar_w_m2,
                        diffuse_w_m2: 0.0,
                        reflected_w_m2: 0.0,
                        angle_of_incidence_rad: 0.0,
                    }],
                    outdoor_wet_bulb_c: 0.0,
                    outdoor_enthalpy_j_kg: 0.0,
                    ghi_w_m2: 0.0,
                    dni_w_m2: 0.0,
                    dhi_w_m2: 0.0,
                    solar_altitude_deg: 0.0,
                    mains_temp_c: 15.0,
                },
                grid: GridState {
                    voltage_pu: 1.0,
                    frequency_hz: 60.0,
                },
                custom_domains: vec![],
                current_time: Utc
                    .with_ymd_and_hms(2026, 6, 21, 12, 0, 0)
                    .single()
                    .unwrap(),
                time_res: chrono::Duration::seconds(60),
            }
        };

        let make_solver = |env: &EnvironmentState| -> ThermalSolver {
            let cfg = ThermalSolverConfig {
                zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
                zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
                zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
                outdoor_temp_input_indices: vec![0],
                indoor_temp_input_indices: vec![],
                solar_input_indices: HashMap::from([(42u32, 2usize)]),
                window_properties: HashMap::new(),
                exterior_surfaces: vec![],
                interior_lwr_zones: vec![],
                ideal_setpoints_c: HashMap::new(),
                ideal_hvac_zones: vec![],
                infiltration: InfiltrationMethod::Ach { ach: 0.0 },
                ventilation_flow_m3_s: 0.0,
                ventilation: VentilationConfig::default(),
                natural_ventilation: None,
            };
            let mut s = ThermalSolver::new(model.clone(), cfg, 60.0, env, zone_temp).unwrap();
            s.x[0] = zone_temp;
            s
        };

        let env_no_solar = make_env(0.0);
        let env_solar = make_env(500.0); // 500 W/m² direct through window

        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };

        let mut solver_no = make_solver(&env_no_solar);
        let t_no_solar = solver_no
            .resolve(&ports, &env_no_solar, Duration::from_secs(60))
            .zone_temperatures_c[0]
            .1;

        let mut solver_with = make_solver(&env_solar);
        let t_solar = solver_with
            .resolve(&ports, &env_solar, Duration::from_secs(60))
            .zone_temperatures_c[0]
            .1;

        assert!(
            t_solar > t_no_solar,
            "solar input should warm zone: t_solar={t_solar:.4}, t_no_solar={t_no_solar:.4}"
        );
        let delta = t_solar - t_no_solar;
        assert!(
            delta > 1e-3,
            "solar warming effect too small: delta={delta:.6} C"
        );
    }

    /// Higher wind speed must produce more infiltration-driven cooling for the
    /// AshraeWindStack method, because wind contributes quadratically to the
    /// volumetric flow: q_wind = c_w * shelter * v². A solver with v=8 m/s must
    /// end up cooler than the same solver with v=2 m/s after one step.
    #[test]
    fn ashrae_wind_stack_higher_wind_produces_more_cooling() {
        let zone_temp = 22.0;
        let outdoor_temp = 0.0;

        let method = InfiltrationMethod::AshraeWindStack {
            c_s: 0.000_290,
            c_w: 0.000_231,
            shielding_coeff: 0.7,
            n_i: 0.65,
        };

        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };

        let mut env_low_wind = env_for_temp(zone_temp, outdoor_temp);
        env_low_wind.weather.wind_speed_m_s = 2.0;
        let mut solver_low = solver_with_infiltration(&env_low_wind, method);
        let t_low_wind = solver_low
            .resolve(&ports, &env_low_wind, Duration::from_secs(60))
            .zone_temperatures_c[0]
            .1;

        let mut env_high_wind = env_for_temp(zone_temp, outdoor_temp);
        env_high_wind.weather.wind_speed_m_s = 8.0;
        let mut solver_high = solver_with_infiltration(&env_high_wind, method);
        let t_high_wind = solver_high
            .resolve(&ports, &env_high_wind, Duration::from_secs(60))
            .zone_temperatures_c[0]
            .1;

        assert!(
            t_high_wind < t_low_wind,
            "higher wind must cool zone more: t_high={t_high_wind:.5}, t_low={t_low_wind:.5}"
        );
        // Wind term scales as v², so going from 2 to 8 m/s (4×) raises q_wind by 16×;
        // the total flow increase will be meaningful — require at least 1 mK more cooling.
        let extra_cooling = t_low_wind - t_high_wind;
        assert!(
            extra_cooling > 1e-3,
            "extra cooling from higher wind too small: {extra_cooling:.6} C"
        );
    }

    /// Higher wind speed must produce more infiltration-driven cooling for the
    /// Ela method, because wind enters the driver as wind_coeff * v² and
    /// increases the square-root flow rate monotonically.
    #[test]
    fn ela_higher_wind_produces_more_cooling() {
        let zone_temp = 22.0;
        let outdoor_temp = 0.0;

        let method = InfiltrationMethod::Ela {
            ela_m2: 0.01,
            stack_coeff: 0.000_145,
            wind_coeff: 0.000_087,
        };

        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };

        let mut env_low_wind = env_for_temp(zone_temp, outdoor_temp);
        env_low_wind.weather.wind_speed_m_s = 1.0;
        let mut solver_low = solver_with_infiltration(&env_low_wind, method);
        let t_low_wind = solver_low
            .resolve(&ports, &env_low_wind, Duration::from_secs(60))
            .zone_temperatures_c[0]
            .1;

        let mut env_high_wind = env_for_temp(zone_temp, outdoor_temp);
        env_high_wind.weather.wind_speed_m_s = 6.0;
        let mut solver_high = solver_with_infiltration(&env_high_wind, method);
        let t_high_wind = solver_high
            .resolve(&ports, &env_high_wind, Duration::from_secs(60))
            .zone_temperatures_c[0]
            .1;

        assert!(
            t_high_wind < t_low_wind,
            "higher wind must cool zone more via ELA: t_high={t_high_wind:.5}, t_low={t_low_wind:.5}"
        );
        let extra_cooling = t_low_wind - t_high_wind;
        assert!(
            extra_cooling > 1e-3,
            "extra ELA cooling from higher wind too small: {extra_cooling:.6} C"
        );
    }

    /// ANSI/ASHRAE Standard 140-2017, Case 900: heavyweight longer time constant.
    ///
    /// Same geometry as Case 600 but with heavy construction (10× capacitance).
    /// Verifies that after the same 24h cold step, the heavy-mass zone temperature
    /// is closer to initial than the lightweight Case 600 (slower response).
    #[test]
    fn bestest_case_900_heavyweight_longer_time_constant() {
        use hares_physics::constants::CP_DRY_AIR_J_KG_K;
        use nalgebra::DVector;

        let (model_600, capacitance_600, _, _, ua_envelope, ua_floor) = bestest_case_600_model();

        // Case 900: same geometry, 10× thermal mass
        let capacitance_900 = capacitance_600 * 10.0;

        // Build Case 900 model with higher capacitance
        use crate::rc_network::{NodeId, RCNetwork};

        let r_envelope = 1.0 / ua_envelope;
        let r_floor = 1.0 / ua_floor;

        let caps = std::collections::HashMap::from([(NodeId(0), capacitance_900)]);
        let resistances = std::collections::HashMap::from([
            ((NodeId(0), NodeId(1)), r_envelope),
            ((NodeId(0), NodeId(2)), r_floor),
        ]);
        let network =
            RCNetwork::from_elements(caps, resistances, vec![NodeId(1), NodeId(2)]).unwrap();
        let (a_c, b_c_rc) = network.build_matrices().unwrap();

        let n_states = a_c.nrows();
        let n_ext = b_c_rc.ncols();
        let n_inputs = n_ext + 1;
        let mut b_c = DMatrix::zeros(n_states, n_inputs);
        for row in 0..n_states {
            for col in 0..n_ext {
                b_c[(row, col)] = b_c_rc[(row, col)];
            }
        }
        b_c[(0, n_ext)] = 1.0 / capacitance_900;

        let dt = 300.0;
        let mapping = OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model_900 = StateSpaceModel::from_continuous(&a_c, &b_c, dt, &mapping).unwrap();

        // Run both models through the same 24h cold step
        let volume_m3 = 129.6;
        let ach = 0.5;
        let q_internal = 200.0;
        let t_outdoor = -10.0;
        let t_ground = 10.0;
        let rho_air = 1.2;
        let n_steps = 288; // 24h at 5-min steps

        let run_model = |model: &StateSpaceModel| -> f64 {
            let mut x = DVector::from_row_slice(&[20.0]);
            let mut t_zone = 20.0;
            for _ in 0..n_steps {
                let q_inf_m3_s = ach * volume_m3 / 3600.0;
                let m_dot = rho_air * q_inf_m3_s;
                let q_infiltration = m_dot * CP_DRY_AIR_J_KG_K * (t_outdoor - t_zone);
                let q_total = q_internal + q_infiltration;
                let u = DVector::from_row_slice(&[t_outdoor, t_ground, q_total]);
                x = model.step(&x, &u);
                let y = model.output(&x, &u);
                t_zone = y[0];
            }
            t_zone
        };

        let t_zone_600 = run_model(&model_600);
        let t_zone_900 = run_model(&model_900);

        // Heavier mass = slower response = closer to initial 20°C after 24h
        assert!(
            t_zone_900 > t_zone_600,
            "Case 900 (heavy) should be warmer than Case 600 (light) after cold step: \
             t_900={t_zone_900:.2}, t_600={t_zone_600:.2}"
        );

        // Case 900 should still be noticeably closer to the initial 20°C
        let drift_600 = (20.0 - t_zone_600).abs();
        let drift_900 = (20.0 - t_zone_900).abs();
        assert!(
            drift_900 < drift_600,
            "Case 900 drift ({drift_900:.2}) should be less than Case 600 drift ({drift_600:.2})"
        );
    }

    /// The solar input path is linear: the state-space B matrix column for the
    /// solar input has a fixed gain (1/C), so doubling irradiance must produce
    /// exactly double the temperature rise above the no-solar baseline.
    /// Ratio must be within 0.1% to catch sign errors, gain miscalculations,
    /// or accidental quadratic scaling.
    #[test]
    fn solar_temperature_rise_is_proportional_to_irradiance() {
        let zone_temp = 18.0;
        let outdoor_temp = 5.0;

        let r = 2.0;
        let c = 50_000.0;
        // B: [outdoor_temp, sensible_gain, solar_gain]
        let a_c = DMatrix::from_row_slice(1, 1, &[-1.0 / (r * c)]);
        let b_c = DMatrix::from_row_slice(1, 3, &[1.0 / (r * c), 1.0 / c, 1.0 / c]);
        let mapping = crate::state_space::OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();

        let make_env = |solar_w_m2: f64| -> EnvironmentState {
            EnvironmentState {
                zones: vec![ZoneState {
                    id: ZoneId(1),
                    temperature_c: zone_temp,
                    humidity_ratio: 0.008,
                    relative_humidity: 0.45,
                    wet_bulb_c: zone_temp - 5.0,
                    volume_m3: 200.0,
                }],
                weather: WeatherState {
                    outdoor_temp_c: outdoor_temp,
                    outdoor_humidity_ratio: 0.004,
                    wind_speed_m_s: 0.0,
                    wind_dir_deg: 0.0,
                    ground_temp_c: outdoor_temp,
                    sky_temp_c: outdoor_temp - 5.0,
                    pressure_kpa: 101.325,
                    solar_irradiance: vec![SurfaceIrradiance {
                        surface_id: 7,
                        direct_w_m2: solar_w_m2,
                        diffuse_w_m2: 0.0,
                        reflected_w_m2: 0.0,
                        angle_of_incidence_rad: 0.0,
                    }],
                    outdoor_wet_bulb_c: 0.0,
                    outdoor_enthalpy_j_kg: 0.0,
                    ghi_w_m2: 0.0,
                    dni_w_m2: 0.0,
                    dhi_w_m2: 0.0,
                    solar_altitude_deg: 0.0,
                    mains_temp_c: 15.0,
                },
                grid: GridState {
                    voltage_pu: 1.0,
                    frequency_hz: 60.0,
                },
                custom_domains: vec![],
                current_time: Utc
                    .with_ymd_and_hms(2026, 6, 21, 12, 0, 0)
                    .single()
                    .unwrap(),
                time_res: chrono::Duration::seconds(60),
            }
        };

        let make_solver = |env: &EnvironmentState| -> ThermalSolver {
            let cfg = ThermalSolverConfig {
                zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
                zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
                zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
                outdoor_temp_input_indices: vec![0],
                indoor_temp_input_indices: vec![],
                solar_input_indices: HashMap::from([(7u32, 2usize)]),
                window_properties: HashMap::new(),
                exterior_surfaces: vec![],
                interior_lwr_zones: vec![],
                ideal_setpoints_c: HashMap::new(),
                ideal_hvac_zones: vec![],
                infiltration: InfiltrationMethod::Ach { ach: 0.0 },
                ventilation_flow_m3_s: 0.0,
                ventilation: VentilationConfig::default(),
                natural_ventilation: None,
            };
            let mut s = ThermalSolver::new(model.clone(), cfg, 60.0, env, zone_temp).unwrap();
            s.x[0] = zone_temp;
            s
        };

        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };

        let env_base = make_env(0.0);
        let t_base = make_solver(&env_base)
            .resolve(&ports, &env_base, Duration::from_secs(60))
            .zone_temperatures_c[0]
            .1;

        let env_low = make_env(300.0);
        let t_low = make_solver(&env_low)
            .resolve(&ports, &env_low, Duration::from_secs(60))
            .zone_temperatures_c[0]
            .1;

        let env_high = make_env(600.0);
        let t_high = make_solver(&env_high)
            .resolve(&ports, &env_high, Duration::from_secs(60))
            .zone_temperatures_c[0]
            .1;

        let delta_low = t_low - t_base;
        let delta_high = t_high - t_base;

        assert!(
            delta_low > 1e-6,
            "300 W/m² must produce a measurable rise: {delta_low:.8}"
        );
        assert!(
            delta_high > 1e-6,
            "600 W/m² must produce a measurable rise: {delta_high:.8}"
        );

        // Linear system: 2× irradiance must produce 2× temperature rise.
        // Allow 0.1% tolerance for floating-point discretization round-off.
        let ratio = delta_high / delta_low;
        assert!(
            (ratio - 2.0).abs() < 0.001,
            "solar temperature rise must scale linearly with irradiance: ratio={ratio:.6}, expected 2.0"
        );
    }

    /// Natural ventilation with operable windows should cool a warm zone (above comfort base)
    /// relative to an identical solver without natural ventilation, when outdoor air is
    /// cool, dry, and below the zone temperature.
    #[test]
    fn natural_ventilation_cools_warm_zone_when_conditions_met() {
        // Zone well above comfort base, cool dry outdoor air — nat vent should be active.
        let zone_temp = 27.0; // above t_base (22.778 °C)
        let outdoor_temp = 18.0; // cool outdoor, below zone
        let env = env_for_temp(zone_temp, outdoor_temp);

        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };

        let mut solver_no_nv = solver_with_infiltration(&env, InfiltrationMethod::Ach { ach: 0.0 });
        let t_no_nv = solver_no_nv
            .resolve(&ports, &env, Duration::from_secs(60))
            .zone_temperatures_c[0]
            .1;

        // Build solver with natural ventilation enabled; 6.7% of 12 m² total window area.
        let r = 2.0;
        let c = 50_000.0;
        let a_c = DMatrix::from_row_slice(1, 1, &[-1.0 / (r * c)]);
        let b_c = DMatrix::from_row_slice(1, 2, &[1.0 / (r * c), 1.0 / c]);
        let mapping = crate::state_space::OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();
        let config = ThermalSolverConfig {
            zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
            outdoor_temp_input_indices: vec![0],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            window_properties: HashMap::new(),
            exterior_surfaces: vec![],
            interior_lwr_zones: vec![],
            ideal_setpoints_c: HashMap::new(),
            ideal_hvac_zones: vec![],
            infiltration: InfiltrationMethod::Ach { ach: 0.0 },
            ventilation_flow_m3_s: 0.0,
            ventilation: VentilationConfig::default(),
            natural_ventilation: Some(NaturalVentilationConfig::from_window_area(
                12.0,      // 12 m² total window area
                0.000_106, // ELA stack coeff
                0.000_143, // ELA wind coeff
            )),
        };
        let mut solver_nv = ThermalSolver::new(model, config, 60.0, &env, zone_temp).unwrap();
        solver_nv.x[0] = zone_temp;

        let t_nv = solver_nv
            .resolve(&ports, &env, Duration::from_secs(60))
            .zone_temperatures_c[0]
            .1;

        assert!(
            t_nv < t_no_nv,
            "natural ventilation should cool the zone: t_nv={t_nv:.4}, t_no_nv={t_no_nv:.4}"
        );
        let delta = t_no_nv - t_nv;
        assert!(
            delta > 1e-4,
            "natural ventilation cooling effect too small: {delta:.6} °C"
        );
    }

    /// Natural ventilation must be inactive when outdoor temperature exceeds zone temperature
    /// (no stack buoyancy to drive flow outward). The zone temperature must be identical to
    /// the no-nat-vent baseline.
    #[test]
    fn natural_ventilation_inactive_when_outdoor_warmer_than_zone() {
        let zone_temp = 20.0;
        let outdoor_temp = 30.0; // outdoor > zone → gated off

        let env = env_for_temp(zone_temp, outdoor_temp);

        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };

        // Baseline (no nat vent, pure conduction through envelope)
        let mut solver_no_nv = solver_with_infiltration(&env, InfiltrationMethod::Ach { ach: 0.0 });
        let t_no_nv = solver_no_nv
            .resolve(&ports, &env, Duration::from_secs(60))
            .zone_temperatures_c[0]
            .1;

        // With nat vent configured but gated off by temperature condition
        let r = 2.0;
        let c = 50_000.0;
        let a_c = DMatrix::from_row_slice(1, 1, &[-1.0 / (r * c)]);
        let b_c = DMatrix::from_row_slice(1, 2, &[1.0 / (r * c), 1.0 / c]);
        let mapping = crate::state_space::OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();
        let config = ThermalSolverConfig {
            zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
            outdoor_temp_input_indices: vec![0],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            window_properties: HashMap::new(),
            exterior_surfaces: vec![],
            interior_lwr_zones: vec![],
            ideal_setpoints_c: HashMap::new(),
            ideal_hvac_zones: vec![],
            infiltration: InfiltrationMethod::Ach { ach: 0.0 },
            ventilation_flow_m3_s: 0.0,
            ventilation: VentilationConfig::default(),
            natural_ventilation: Some(NaturalVentilationConfig::from_window_area(
                12.0, 0.000_106, 0.000_143,
            )),
        };
        let mut solver_nv = ThermalSolver::new(model, config, 60.0, &env, zone_temp).unwrap();
        solver_nv.x[0] = zone_temp;

        let t_nv = solver_nv
            .resolve(&ports, &env, Duration::from_secs(60))
            .zone_temperatures_c[0]
            .1;

        // Zone temperatures must be identical (nat vent gated off)
        let diff = (t_nv - t_no_nv).abs();
        assert!(
            diff < 1e-10,
            "nat vent should be inactive (outdoor > zone): t_nv={t_nv:.8}, t_no_nv={t_no_nv:.8}, diff={diff:.2e}"
        );
    }

    /// Exterior LW radiation to a cold night sky must lower zone temperature.
    ///
    /// A warm surface (20 °C) radiating to a sky at −20 °C loses heat via the
    /// Stefan-Boltzmann LW exchange; the zone fed by that surface should end one
    /// step cooler than an identical zone without LW radiation configured.
    #[test]
    fn exterior_lw_cold_sky_cools_zone_vs_no_lw() {
        let zone_temp = 20.0_f64;
        let outdoor_temp = 5.0_f64;

        // 1R1C with 3 inputs: [outdoor_temp, lw_flux, zone_heat]
        // The LW flux (W) is accumulated at input index 1 and drives the zone
        // via a 1/C term, just like the zone heat input.
        let r = 2.0_f64;
        let c = 50_000.0_f64;
        let a_c = DMatrix::from_row_slice(1, 1, &[-1.0 / (r * c)]);
        let b_c = DMatrix::from_row_slice(1, 3, &[1.0 / (r * c), 1.0 / c, 1.0 / c]);
        let mapping = OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();

        let env = {
            let mut e = env_for_temp(zone_temp, outdoor_temp);
            e.weather.sky_temp_c = -20.0;
            e.weather.ground_temp_c = 5.0;
            e
        };

        let make_solver = |with_lw: bool, env: &EnvironmentState| -> ThermalSolver {
            let exterior_surfaces = if with_lw {
                vec![ExteriorSurfaceInfo {
                    surface_id: 0,
                    state_index: 0, // zone state used as surface temperature proxy
                    input_index: 1, // LW flux accumulated here
                    area_m2: 20.0,
                    emissivity: 0.90,
                    tilt_deg: 90.0, // vertical wall
                }]
            } else {
                vec![]
            };
            let cfg = ThermalSolverConfig {
                zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
                zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
                zone_sensible_input_indices: HashMap::from([(ZoneId(1), 2)]),
                outdoor_temp_input_indices: vec![0],
                indoor_temp_input_indices: vec![],
                solar_input_indices: HashMap::new(),
                window_properties: HashMap::new(),
                exterior_surfaces,
                interior_lwr_zones: vec![],
                ideal_setpoints_c: HashMap::new(),
                ideal_hvac_zones: vec![],
                infiltration: InfiltrationMethod::Ach { ach: 0.0 },
                ventilation_flow_m3_s: 0.0,
                ventilation: VentilationConfig::default(),
                natural_ventilation: None,
            };
            let mut s = ThermalSolver::new(model.clone(), cfg, 60.0, env, zone_temp).unwrap();
            s.x[0] = zone_temp;
            s
        };

        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };

        let t_no_lw = make_solver(false, &env)
            .resolve(&ports, &env, Duration::from_secs(60))
            .zone_temperatures_c[0]
            .1;

        let t_with_lw = make_solver(true, &env)
            .resolve(&ports, &env, Duration::from_secs(60))
            .zone_temperatures_c[0]
            .1;

        assert!(
            t_with_lw < t_no_lw,
            "cold sky LW radiation must cool zone below no-LW baseline: \
             t_lw={t_with_lw:.5}, t_no_lw={t_no_lw:.5}"
        );
    }

    /// Window IAM correction must reduce solar gain at oblique angles.
    ///
    /// A 1R1C zone driven purely by solar input through a single window surface is
    /// stepped once under two irradiance conditions that differ only in the angle of
    /// incidence: 5° (near-normal, IAM ≈ 1) vs 70° (oblique, IAM substantially < 1).
    /// The near-normal case must produce a higher zone temperature, and the oblique
    /// gain must be less than 95 % of the normal gain.
    #[test]
    fn window_iam_reduces_gain_at_oblique_aoi() {
        let zone_temp = 18.0_f64;
        let outdoor_temp = 5.0_f64;
        // 1R1C: inputs = [outdoor_temp, sensible_gain, solar_gain]
        let r = 2.0_f64;
        let c = 50_000.0_f64;
        let a_c = DMatrix::from_row_slice(1, 1, &[-1.0 / (r * c)]);
        let b_c = DMatrix::from_row_slice(1, 3, &[1.0 / (r * c), 1.0 / c, 1.0 / c]);
        let mapping = OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();

        let window_surface_id: u32 = 55;
        let win_props = WindowSolarProperties {
            shgc: 0.4,
            u_factor_w_m2_k: 1.8,
            area_m2: 2.0,
        };

        // 500 W/m² direct irradiance, no diffuse or reflected.
        let make_env = |aoi_rad: f64| -> EnvironmentState {
            EnvironmentState {
                zones: vec![ZoneState {
                    id: ZoneId(1),
                    temperature_c: zone_temp,
                    humidity_ratio: 0.008,
                    relative_humidity: 0.45,
                    wet_bulb_c: zone_temp - 5.0,
                    volume_m3: 200.0,
                }],
                weather: WeatherState {
                    outdoor_temp_c: outdoor_temp,
                    outdoor_humidity_ratio: 0.004,
                    wind_speed_m_s: 0.0,
                    wind_dir_deg: 0.0,
                    ground_temp_c: outdoor_temp,
                    sky_temp_c: outdoor_temp - 5.0,
                    pressure_kpa: 101.325,
                    solar_irradiance: vec![SurfaceIrradiance {
                        surface_id: window_surface_id,
                        direct_w_m2: 500.0,
                        diffuse_w_m2: 0.0,
                        reflected_w_m2: 0.0,
                        angle_of_incidence_rad: aoi_rad,
                    }],
                    outdoor_wet_bulb_c: 0.0,
                    outdoor_enthalpy_j_kg: 0.0,
                    ghi_w_m2: 0.0,
                    dni_w_m2: 0.0,
                    dhi_w_m2: 0.0,
                    solar_altitude_deg: 0.0,
                    mains_temp_c: 15.0,
                },
                grid: GridState {
                    voltage_pu: 1.0,
                    frequency_hz: 60.0,
                },
                custom_domains: vec![],
                current_time: Utc
                    .with_ymd_and_hms(2026, 6, 21, 12, 0, 0)
                    .single()
                    .unwrap(),
                time_res: chrono::Duration::seconds(60),
            }
        };

        let make_solver = |env: &EnvironmentState| -> ThermalSolver {
            let cfg = ThermalSolverConfig {
                zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
                zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
                zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
                outdoor_temp_input_indices: vec![0],
                indoor_temp_input_indices: vec![],
                solar_input_indices: HashMap::from([(window_surface_id, 2usize)]),
                window_properties: HashMap::from([(window_surface_id, win_props)]),
                exterior_surfaces: vec![],
                interior_lwr_zones: vec![],
                ideal_setpoints_c: HashMap::new(),
                ideal_hvac_zones: vec![],
                infiltration: InfiltrationMethod::Ach { ach: 0.0 },
                ventilation_flow_m3_s: 0.0,
                ventilation: VentilationConfig::default(),
                natural_ventilation: None,
            };
            let mut s = ThermalSolver::new(model.clone(), cfg, 60.0, env, zone_temp).unwrap();
            s.x[0] = zone_temp;
            s
        };

        let env_normal = make_env(5_f64.to_radians()); // near-normal, IAM ≈ 1
        let env_oblique = make_env(70_f64.to_radians()); // oblique, IAM substantially < 1

        let ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };

        let t_normal = make_solver(&env_normal)
            .resolve(&ports, &env_normal, Duration::from_secs(60))
            .zone_temperatures_c[0]
            .1;

        let t_oblique = make_solver(&env_oblique)
            .resolve(&ports, &env_oblique, Duration::from_secs(60))
            .zone_temperatures_c[0]
            .1;

        assert!(
            t_normal > t_oblique,
            "near-normal incidence must warm zone more than oblique: t_normal={t_normal:.5}, t_oblique={t_oblique:.5}"
        );

        // Compute the transmitted gain in each case and verify oblique < 95 % of normal.
        // gain = area * shgc * direct * iam; with identical irradiance the ratio is just iam.
        // The temperature delta above zone_temp is proportional to gain, so we compare deltas.
        let delta_normal = t_normal - zone_temp;
        let delta_oblique = t_oblique - zone_temp;
        assert!(
            delta_normal > 0.0,
            "near-normal gain must raise zone temperature: delta={delta_normal:.6}"
        );
        assert!(
            delta_oblique < delta_normal * 0.95,
            "oblique gain must be less than 95% of normal gain: delta_oblique={delta_oblique:.6}, delta_normal={delta_normal:.6}"
        );
    }
}
