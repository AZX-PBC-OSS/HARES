use std::collections::HashMap;

use hares_physics::solar::GlazingCurve;
use hares_types::ZoneId;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BoundaryCategory {
    Wall,
    Floor,
    Roof,
    Window,
    InternalMass,
}

/// Per-boundary info needed for conduction heat flow diagnostics.
///
/// Computes convective heat transfer from the interior surface to zone air:
///   `T_surface = radiation_frac × T_node + (1 - radiation_frac) × T_zone`
///   `Q_conv = (T_surface - T_zone) × area_m2 / r_film_int_m2_k_w`
///
/// This matches OCHRE's `H_{surface}_{zone}` energy flow variable.
#[derive(Debug, Clone)]
pub enum BoundaryDiagnosticInfo {
    /// Boundary with RC interior node -- uses surface temperature from state vector.
    /// `T_surface = radiation_frac × T_node + (1 - radiation_frac) × T_zone`
    /// `Q = (T_surface - T_zone) × area / R_film_int`
    RCNode {
        inner_state_index: usize,
        area_m2: f64,
        r_film_int_m2_k_w: f64,
        radiation_frac: f64,
        category: BoundaryCategory,
    },
    /// Boundary without RC node (window, fallback-R) -- uses steady-state UA.
    /// `Q = UA × (T_driving - T_zone)` where T_driving is outdoor or ground.
    SteadyState {
        ua_w_k: f64,
        driving_temp: DrivingTemp,
        category: BoundaryCategory,
    },
}

/// Which environmental temperature drives conduction for a non-RC boundary.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DrivingTemp {
    Outdoor,
    /// Ground temperature at a specific foundation depth.
    ///
    /// The Kusuda-Achenbach model is used at runtime to compute the
    /// depth-attenuated, phase-shifted ground temperature at `depth_m`
    /// metres below grade. Depth 0.0 = surface (DOE-2 model).
    Ground {
        depth_m: f64,
    },
}

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
    /// Zero air-changes-per-hour — hermetically sealed building.
    ///
    /// This is a **conservative sentinel** used when a zone is absent from
    /// `ThermalSolverConfig::infiltration`.  Zero ACH eliminates all
    /// wind/stack-driven infiltration and the associated sensible and latent
    /// loads, which is safe for a "no data" baseline (it will not invent
    /// fictitious loads) but is physically unrealistic for any occupied
    /// building.  Production code should override this via the per-zone
    /// infiltration map.
    fn default() -> Self {
        Self::Ach { ach: 0.0 }
    }
}

/// Runtime parameters for the thermal solver's ventilation/infiltration calculation.
///
/// Distinct from the equipment-level `VentilationConfig` in `hares-equipment`, which owns
/// the full ERV/HRV specification. This struct holds only the parameters the infiltration
/// solver needs: flow distribution per zone, whether the system is balanced, and recovery
/// efficiencies.
///
/// # Default recovery efficiencies
///
/// `sensible_recovery_efficiency` and `latent_recovery_efficiency` both default to `0.0`
/// — i.e. no heat/moisture recovery from the exhaust stream. This is the correct default
/// for dwellings without an ERV/HRV (unbalanced or natural-ventilation-only systems).
/// When a balanced ERV/HRV is configured, both efficiencies must be set explicitly to
/// the manufacturer-rated values (typically 0.65–0.85 for modern units).
#[derive(Debug, Clone, Default)]
pub struct MechanicalVentilationParams {
    pub zone_flow_m3_s: HashMap<ZoneId, f64>,
    /// Whether the ventilation system is balanced (ERV, HRV, or balanced fan).
    /// Recovery efficiencies are only applied for balanced systems.
    /// Unbalanced fans use quadrature combination with infiltration.
    pub balanced: bool,
    /// Sensible heat recovery efficiency [0.0–1.0].
    /// Reduces the sensible ventilation load for balanced systems.
    pub sensible_recovery_efficiency: f64,
    /// Latent heat recovery efficiency [0.0–1.0].
    /// Reduces the latent ventilation load for balanced systems.
    pub latent_recovery_efficiency: f64,
}

/// Configuration for natural ventilation through operable windows.
///
/// Implements the OCHRE/ResStock model: flow is driven by stack effect and wind
/// through operable windows, gated by temperature and outdoor humidity conditions.
///
/// # Defaults
/// - `t_base_c`: 22.778 °C (73 °F) -- OCHRE default comfort base temperature
/// - `max_outdoor_humidity_ratio`: 0.0115 kg/kg -- Building America HSP threshold
/// - `OPEN_AREA_FRACTION`: 0.067 -- matches OCHRE (0.67 × 0.5 × 0.2 of total window area)
#[derive(Debug, Clone, PartialEq)]
pub struct NaturalVentilationConfig {
    /// Effective operable window area [m²].
    ///
    /// Typically computed as `total_window_area_m2 * OPEN_AREA_FRACTION`.
    /// Use [`NaturalVentilationConfig::from_window_area`] to apply the standard fraction.
    pub open_area_m2: f64,
    /// ELA stack coefficient [L/(s·cm⁴·K)] -- same table as infiltration ELA coefficients.
    pub stack_coeff: f64,
    /// ELA wind coefficient [L/(s·cm⁴·(m/s)²)] -- same table as infiltration ELA coefficients.
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
///
/// Pre-computed `transmittance` and `radiation_frac` decompose SHGC into
/// transmitted and absorbed fractions per EnergyPlus Steps 4–5.
/// The `glazing_curve` is precomputed from `u_factor_w_m2_k` and `shgc` at
/// construction time to avoid recomputing it every timestep.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowSolarProperties {
    /// Summer effective SHGC at normal incidence (SHGC × summer shading coefficient).
    pub shgc: f64,
    /// Winter effective SHGC at normal incidence (SHGC × winter shading coefficient).
    pub winter_shgc: f64,
    /// Window U-factor [W/(m²·K)], used to select the EnergyPlus glazing curve.
    pub u_factor_w_m2_k: f64,
    /// Glazing area [m²].
    pub area_m2: f64,
    /// Summer solar transmittance at normal incidence [dimensionless].
    pub transmittance: f64,
    /// Winter solar transmittance at normal incidence [dimensionless].
    pub winter_transmittance: f64,
    /// Inward-flowing fraction of absorbed solar [dimensionless].
    pub radiation_frac: f64,
    /// Precomputed EnergyPlus glazing curve from `u_factor_w_m2_k` and `shgc`.
    pub glazing_curve: GlazingCurve,
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
    /// Fraction of the true surface temperature attributable to the RC node [-].
    ///
    /// Defined as `R_film / (R_film + R_material)` where `R_film` is the interior
    /// convective film resistance and `R_material` is the resistance from the film
    /// to the capacitor node.  The true interior surface temperature is then:
    ///
    ///   `T_surf = radiation_frac × T_node + (1 - radiation_frac) × T_zone`
    ///
    /// A value of `1.0` means the node temperature *is* the surface temperature
    /// (film R dominates: surface sits at the wall node, not at zone air).
    /// A value of `0.0` means the surface temp equals zone air temp
    /// (no film R: node is on the zone side).
    /// For lightweight boundaries with significant film resistance the correction
    /// can be several degrees.
    ///
    /// Range: `[0, 1]`.  Default: `1.0`.
    pub radiation_frac: f64,
    /// Interior radiative film resistance converted to K/W.
    ///
    /// Used by the iterative interior LWR solver to convert net radiative
    /// surface flux [W] into a surface-temperature perturbation:
    /// `T_surf,new = T_surf,base + Q_lwr * rad_res_k_w`.
    pub rad_res_k_w: f64,
    /// Solar absorptance [-] for interior solar distribution.
    ///
    /// Fraction of incident solar radiation absorbed by this surface.
    /// Per-surface value from HPXML `SolarAbsorptance`, or
    /// `INTERIOR_SOLAR_ABSORPTANCE_DEFAULT` (0.70, EnergyPlus Material IDD default).
    /// `0.0` means the surface is excluded from solar distribution.
    pub solar_absorptance: f64,
    /// Whether this surface is a floor (receives beam solar preferentially).
    ///
    /// Beam solar is split between floors and non-floors via [`beam_floor_fraction`]
    /// (a `sin(altitude)` heuristic); within each group, absorbed energy is
    /// weighted by `area × solar_absorptance`. Floors receive more beam at
    /// high solar altitudes, walls/ceiling more at low altitudes.
    pub is_floor: bool,
    /// Optional driving temperature for surface temperature computation.
    ///
    /// For window surfaces without RC nodes, the interior surface temperature is
    /// driven by outdoor conduction through the glass:
    ///   `T_surf = radiation_frac × T_driving + (1 - radiation_frac) × T_zone`
    /// where `radiation_frac = R_film_int / R_total` and `T_driving` comes from
    /// the environment (outdoor or ground temp).
    ///
    /// When `None`, `state_index` is used to read T_node from the state vector
    /// (standard RC-node behavior).
    pub driving_temp: Option<DrivingTemp>,
}

/// Per-zone interior solar distribution configuration.
///
/// Independent of `InteriorLwrZoneConfig` so that solar distribution works
/// in both `StarMesh` and `ScriptF` interior LWR modes. In `StarMesh` mode,
/// `interior_lwr_zones` is empty (radiation is in the A-matrix), but solar
/// still needs to be distributed to surface nodes via `radiation_frac`.
#[derive(Debug, Clone, Default)]
pub struct InteriorSolarZoneConfig {
    pub zone_id: ZoneId,
    pub surfaces: Vec<InteriorSolarSurfaceInfo>,
}

/// Surface info needed for interior solar distribution only.
///
/// A subset of `InteriorSurfaceInfo` — just the fields required to split
/// absorbed solar between the surface RC node and zone air.
#[derive(Debug, Clone)]
pub struct InteriorSolarSurfaceInfo {
    /// Index into input vector `u` for the surface RC node.
    /// `None` for windows (no RC node — all solar goes to zone air).
    pub input_index: Option<usize>,
    /// Surface area [m²].
    pub area_m2: f64,
    /// Solar absorptance [-].
    pub solar_absorptance: f64,
    /// Fraction of absorbed solar deposited to the RC node.
    /// Remainder `(1 - radiation_frac)` goes to zone air.
    pub radiation_frac: f64,
    /// Whether this surface is a floor (receives beam solar preferentially).
    pub is_floor: bool,
}

/// Interior longwave radiation configuration for one zone.
///
/// Surfaces listed here participate in grey interchange (ScriptF) interior LW
/// exchange each timestep. The Gebhart factors are pre-computed at init from
/// approximate view factors and emissivities (EnergyPlus method).
#[derive(Debug, Clone, Default)]
pub struct InteriorLwrZoneConfig {
    /// Zone this configuration applies to.
    pub zone_id: ZoneId,
    /// Surfaces in this zone participating in interior LW exchange.
    pub surfaces: Vec<InteriorSurfaceInfo>,
    /// Pre-computed ScriptF grey interchange factors (computed at init).
    /// `None` until `compute_scriptf()` is called.
    pub scriptf: Option<crate::longwave_radiation::ScriptFCoefficients>,
}

impl InteriorLwrZoneConfig {
    /// Pre-compute interior LWR exchange factors from surface properties.
    ///
    /// Caches ε·σ·A factors and view factors for the exact T⁴ radiosity path.
    /// Call after populating `surfaces`. Enables the exact T⁴ solver path
    /// in `apply_interior_longwave_inputs()` (falls back to linearized if not called).
    pub fn compute_scriptf(&mut self) {
        if self.surfaces.len() >= 2 {
            let interior_surfaces: Vec<crate::longwave_radiation::InteriorSurface> = self
                .surfaces
                .iter()
                .map(|s| crate::longwave_radiation::InteriorSurface {
                    area_m2: s.area_m2,
                    emissivity: s.emissivity,
                })
                .collect();
            self.scriptf = Some(crate::longwave_radiation::ScriptFCoefficients::compute(
                &interior_surfaces,
            ));
        }
    }
}

/// Metadata for one exterior surface used to compute longwave radiation at runtime.
///
/// The sky view factor is derived from `tilt_deg` on each call via [`sky_view_factor`];
/// it is not cached here so that the struct remains free of init ordering.
///
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
    /// Radiation fraction: `R_film / (R_film + R_outermost_half)` -- dimensionless [0,1].
    ///
    /// Controls how much the true surface temperature deviates from the RC node
    /// temperature. A value of 0.0 means use the node temperature directly (no
    /// exterior film resistance in the conduction path).
    pub rad_frac: f64,
    /// Radiation resistance: `R_film / area_m2` -- K/W.
    ///
    /// Converts net surface heat flux [W] to a temperature perturbation on the
    /// exterior surface.
    pub rad_res_k_w: f64,
    /// Number of sub-iterations per timestep: `floor(dt_s / 300.0) + 1`.
    pub n_iter: u32,
    /// Solar absorptance [-] (0–1). Default 0.70 for opaque surfaces (EnergyPlus
    /// Material IDD default), 0.05 for radiant barriers.
    pub absorptance: f64,
    /// Boundary type for per-component heat flow tracking.
    /// `None` means the surface is not attributed to a named component category.
    pub boundary_category: Option<BoundaryCategory>,
    /// Window U-factor [W/(m²·K)]; 0.0 for opaque surfaces (which have RC nodes).
    /// Used by the window exterior LWR correction: when T_sky < T_air the window
    /// radiates more to sky than the U-factor assumes, producing additional cooling.
    pub u_factor_w_m2_k: f64,
    /// Exterior combined film coefficient h_out [W/(m²·K)] for window LWR correction.
    /// Computed from boundary input r_film_exterior at build time. 0.0 for opaque surfaces.
    /// Used as denominator in T_eff = T_air + Δq / h_out. Ref: NFRC 100-2020.
    pub h_out_w_m2_k: f64,
}

/// Index mappings derived from the state-space model structure.
///
/// These map zone IDs to row/column indices in the discretized A_d, B_d, C, D
/// matrices. Constructed by `build_default_solvers` from the RC network topology
/// during model assembly -- not user-supplied configuration.
#[derive(Debug, Clone, Default)]
pub struct StateSpaceWiring {
    pub zone_state_indices: HashMap<ZoneId, usize>,
    pub zone_output_indices: HashMap<ZoneId, usize>,
    pub zone_sensible_input_indices: HashMap<ZoneId, usize>,
    pub outdoor_temp_input_indices: Vec<usize>,
    /// B-matrix columns driving ground-connected boundaries [°C].
    /// Each entry is a column index in B_ext. The parallel vec
    /// `ground_temp_input_depths_m` holds the foundation depth for each column.
    pub ground_temp_input_indices: Vec<usize>,
    /// Foundation depth [m] for each entry in `ground_temp_input_indices`.
    ///
    /// Same length as `ground_temp_input_indices`. Each depth is used to
    /// evaluate `kusuda_achenbach_temp` for the corresponding B-matrix column
    /// at each timestep. 0.0 = grade surface.
    pub ground_temp_input_depths_m: Vec<f64>,
    /// B-matrix columns carrying indoor air temperature [°C] as an external input.
    ///
    /// Empty in the standard RC topology where zone air nodes are internal states
    /// (coupled through the A-matrix). Only populated when a model routes indoor
    /// temperature as a fixed external boundary via B-matrix columns.
    /// Used by `initialize_steady_state()` to set initial boundary conditions.
    pub indoor_temp_input_indices: Vec<usize>,
    pub solar_input_indices: HashMap<u32, usize>,
    /// Zone air node thermal capacitance [J/K] per conditioned zone.
    ///
    /// Populated from the zone air node's diagonal capacitance in the RC network
    /// during model assembly. Used by `integrate_inner` to compute per-step energy
    /// balance residuals without re-deriving from the A/C matrices each timestep.
    pub c_zone_j_k: HashMap<ZoneId, f64>,
}

#[derive(Debug, Clone)]
pub struct ThermalSolverConfig {
    /// Primary conditioned zone for diagnostics and HVAC port lookups.
    pub indoor_zone_id: ZoneId,
    /// Window solar properties keyed by surface_id.
    ///
    /// When a surface_id appears in both `solar_input_indices` and `window_properties`,
    /// the thermal solver applies the EnergyPlus IAM correction (angle-of-incidence
    /// modifier) to beam radiation and a hemispherical average IAM to diffuse and
    /// reflected radiation before multiplying by SHGC and area.
    ///
    /// Surfaces absent from this map are treated as opaque; opaque solar gain
    /// is delivered via [`ExteriorSurfaceInfo::absorptance`] in `exterior_surfaces`.
    pub window_properties: HashMap<u32, WindowSolarProperties>,
    /// Map from window surface_id to the zone it belongs to.
    /// Used by the interior solar distribution to route transmitted solar
    /// to the correct zone's interior surfaces.
    pub window_zone_ids: HashMap<u32, ZoneId>,
    /// Exterior surfaces for LWR and opaque solar gain.
    pub exterior_surfaces: Vec<ExteriorSurfaceInfo>,
    /// Per-zone interior surface configurations for intra-zone LW radiation.
    ///
    /// Empty = no interior LW correction (backward-compatible default).
    /// When populated, the solver calls [`interior_longwave_linearised_w`] for each zone
    /// using the current zone air temperature as the linearisation point and accumulates
    /// the net surface fluxes into the corresponding `input_index` entries in `u`.
    pub interior_lwr_zones: Vec<InteriorLwrZoneConfig>,
    /// Per-zone interior solar distribution (works in both StarMesh and ScriptF modes).
    pub interior_solar_zones: Vec<InteriorSolarZoneConfig>,
    /// Per-zone infiltration methods. Zones not listed default to zero ACH.
    /// Uses a `Vec` instead of `HashMap` for cache-friendly hot-loop iteration.
    pub infiltration: Vec<(ZoneId, InfiltrationMethod)>,
    /// Supply duct leakage during fan operation [m³/s].
    ///
    /// Pre-computed as `supply_leakage_frac × rated_fan_flow_m3_s`.  Zero means
    /// no duct adjustment is applied.  Set from HPXML duct data in solver_builder.
    pub supply_duct_leakage_m3_s: f64,
    /// Return duct leakage during fan operation [m³/s].
    ///
    /// Pre-computed as `return_leakage_frac × rated_fan_flow_m3_s`.  Zero means
    /// no duct adjustment is applied.
    pub return_duct_leakage_m3_s: f64,
    pub ventilation_flow_m3_s: f64,
    pub ventilation: MechanicalVentilationParams,
    /// Natural ventilation through operable windows. `None` disables the feature (default).
    pub natural_ventilation: Option<NaturalVentilationConfig>,
    /// Per-boundary conduction diagnostics for BESTEST per-component heat gain tracking.
    pub boundary_diagnostics: Vec<BoundaryDiagnosticInfo>,
    /// Interior longwave radiation method.
    ///
    /// `StarMesh` (default): linearized inter-surface conductances baked into
    /// the A-matrix at construction time. No iterative LWR injection needed.
    /// `ScriptF`: iterative T⁴ radiosity injection each timestep (legacy mode).
    pub interior_lwr_method: crate::boundary_rc::InteriorLwrMethod,
}

impl Default for ThermalSolverConfig {
    fn default() -> Self {
        Self {
            indoor_zone_id: ZoneId(1),
            window_properties: HashMap::new(),
            window_zone_ids: HashMap::new(),
            exterior_surfaces: Vec::new(),
            interior_lwr_zones: Vec::new(),
            interior_solar_zones: Vec::new(),
            infiltration: Vec::new(),
            supply_duct_leakage_m3_s: 0.0,
            return_duct_leakage_m3_s: 0.0,
            ventilation_flow_m3_s: 0.0,
            ventilation: MechanicalVentilationParams::default(),
            natural_ventilation: None,
            boundary_diagnostics: Vec::new(),
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::default(),
        }
    }
}

#[derive(Debug, Error)]
pub enum ThermalSolverError {
    #[error("invalid zone mapping: zone {zone:?} is missing from {field}")]
    MissingZoneMapping { zone: ZoneId, field: &'static str },
    #[error("failed to initialize steady-state vector: {0}")]
    Initialization(String),
    #[error("{0}")]
    Configuration(String),
}

pub type Result<T> = std::result::Result<T, ThermalSolverError>;

/// Per-timestep envelope component gains [W] for output/diagnostics.
///
/// All values are signed: positive = heat flowing INTO the indoor zone.
/// Populated after each `resolve()` call; read via [`ThermalSolver::component_gains`].
#[derive(Debug, Clone, Default)]
pub struct EnvelopeComponentGains {
    /// Window transmitted solar (SHGC × IAM × area × POA) [W].
    pub window_solar_w: f64,
    /// Opaque exterior surface solar + LWR combined injection [W].
    /// Includes surfaces routed through both iterative and non-iterative paths.
    pub opaque_solar_lwr_w: f64,
    /// Total interior longwave radiation exchange activity [W].
    /// Computed as Σ|q_i|/2 over all surfaces in the zone, where q_i is the
    /// net LWR flux into surface i. By energy conservation, Σ q_i = 0, so the
    /// signed sum is always zero and useless. The absolute-value sum divided
    /// by 2 (avoiding double-counting each radiative pair) indicates how much
    /// radiation energy is actively being exchanged between surfaces.
    /// For StarMesh mode this is always 0 (radiation is baked into the A-matrix
    /// at construction time and no per-timestep injection occurs).
    pub interior_lwr_w: f64,
    /// Infiltration sensible heat gain (indoor zone only) [W].
    /// After unbalanced ventilation scaling: `raw_inf × nat_flow_ratio`.
    pub infiltration_w: f64,
    /// Forced mechanical ventilation sensible heat gain (indoor zone only) [W].
    /// For unbalanced systems this is the forced component of the quadrature
    /// combination `sqrt(nat² + forced²)`. OCHRE may report this differently.
    pub ventilation_w: f64,
    /// Natural ventilation sensible heat gain [W].
    pub natural_ventilation_w: f64,
    /// Combined infiltration + ventilation sensible gain [W].
    /// `= ρ × sqrt(nat² + forced²) × cp × ΔT` for unbalanced systems.
    /// Compare this to OCHRE's sum of infiltration + forced + natural ventilation.
    pub combined_airflow_sensible_w: f64,
    /// Total convective sensible gains from all equipment ports (HVAC + appliances) [W].
    /// Only the convective portion that goes directly to zone air.
    pub port_sensible_w: f64,
    /// Total radiant sensible gains from equipment ports distributed to surfaces [W].
    /// Distributed via E+ TMULT method; some reaches zone air via radiation_frac split.
    pub port_radiant_w: f64,
    /// HVAC heating contribution to the indoor zone [W].
    pub hvac_heating_w: f64,
    /// HVAC cooling contribution to the indoor zone [W].
    pub hvac_cooling_w: f64,
    /// Non-HVAC internal gains (appliances, lighting, occupancy) [W].
    /// Equals the `InternalGain` category total for the indoor zone,
    /// including both convective and radiant components.
    pub internal_gain_w: f64,
    /// Equipment jacket/shell losses to the indoor zone [W].
    /// Equals `JacketLoss` category total (water heater skin loss, etc.).
    pub jacket_loss_w: f64,
    /// Duct distribution losses [W] -- heat deposited into the duct zone, removed from delivered capacity.
    pub duct_loss_w: f64,
    /// Dehumidifier sensible heat gain to the indoor zone [W].
    /// Standalone dehumidifiers are zone HVAC equipment (EnergyPlus Eng. Ref.,
    /// Zone Equipment and Zone Forced Air Units), not passive internal gains.
    pub hvac_dehumidification_w: f64,
    /// Per-zone infiltration sensible heat gains [W].
    /// `infiltration_w` is the indoor-zone alias for backward compatibility.
    pub infiltration_by_zone: Vec<(ZoneId, f64)>,
    /// Per-zone interior LWR total exchange activity [W] (Σ|q_i|/2 per zone).
    /// See [`interior_lwr_w`] for the physical meaning.
    pub interior_lwr_by_zone: Vec<(ZoneId, f64)>,
    /// Heat gain through wall boundaries (conduction + solar + LWR) [W].
    pub wall_heat_gain_w: f64,
    /// Heat gain through floor boundaries [W].
    pub floor_heat_gain_w: f64,
    /// Heat gain through roof boundaries [W].
    pub roof_heat_gain_w: f64,
    /// Heat gain through window boundaries (transmitted + absorbed) [W].
    pub window_heat_gain_w: f64,
    /// Heat gain from internal mass surfaces [W].
    pub internal_mass_heat_gain_w: f64,
    /// Opaque exterior surface solar absorptance gain only [W].
    pub opaque_solar_w: f64,
    /// Exterior longwave radiation exchange only [W].
    pub exterior_lwr_w: f64,
    /// Window exterior LWR beyond U-factor assumption [W].
    /// When T_sky < T_air (clear night), windows radiate more to sky than the
    /// U-factor (which assumes T_sky ≈ T_air) accounts for. This field tracks
    /// that additional cooling. Negative = cooling. Zero when T_sky = T_air.
    /// Ref: Walton (1983); E+ Eng.Ref "External Longwave Radiation"; NFRC.
    pub window_exterior_lwr_w: f64,
    /// Outdoor driving temperature used this timestep [°C].
    pub driving_outdoor_temp_c: f64,
    /// Ground driving temperature used this timestep [°C].
    pub driving_ground_temp_c: f64,
    /// Total combined air flow rate (infiltration + ventilation) [m³/s].
    pub total_airflow_m3_s: f64,
    /// Raw AIM-2 infiltration flow rate before ventilation interaction [m³/s].
    pub raw_infiltration_m3_s: f64,
    /// Forced mechanical ventilation flow rate [m³/s].
    pub forced_vent_m3_s: f64,
    /// Natural ventilation flow rate [m³/s].
    pub natural_vent_m3_s: f64,
    /// Per-exterior-surface energy diagnostics (solar absorbed, LWR, surface temp).
    /// Parallel to `ThermalSolverConfig::exterior_surfaces`.
    #[cfg(any(debug_assertions, feature = "observe_detailed"))]
    pub ext_surface_diag: Vec<ExtSurfaceDiag>,
    /// Per-interior-surface diagnostics (surface temp, LWR flux).
    #[cfg(any(debug_assertions, feature = "observe_detailed"))]
    pub int_surface_diag: Vec<IntSurfaceDiag>,
    #[cfg(any(debug_assertions, feature = "observe_detailed"))]
    pub window_solar_diag: Vec<WindowSolarDiag>,
}

/// Per-exterior-surface energy diagnostic snapshot.
#[cfg(any(debug_assertions, feature = "observe_detailed"))]
#[derive(Debug, Clone, Copy, Default)]
pub struct ExtSurfaceDiag {
    pub surface_id: u32,
    pub category: Option<BoundaryCategory>,
    /// Solar absorbed at the exterior face [W] = absorptance × area × POA.
    pub solar_absorbed_w: f64,
    /// Net exterior longwave radiation gain [W] (negative = cooling to sky).
    pub lwr_gain_w: f64,
    /// Converged exterior surface temperature [°C].
    pub surface_temp_c: f64,
    /// Heat actually injected into the RC node [W] = (solar + lwr) × rad_frac.
    pub injected_w: f64,
}

/// Per-interior-surface diagnostic snapshot.
#[cfg(any(debug_assertions, feature = "observe_detailed"))]
#[derive(Debug, Clone, Copy, Default)]
pub struct IntSurfaceDiag {
    /// Interpolated interior surface temperature [°C].
    /// = radiation_frac × T_node + (1 - radiation_frac) × T_zone.
    pub surface_temp_c: f64,
    /// Net interior LWR flux for this surface [W].
    pub lwr_flux_w: f64,
}

/// Per-window solar diagnostic snapshot.
#[cfg(any(debug_assertions, feature = "observe_detailed"))]
#[derive(Debug, Clone, Copy, Default)]
pub struct WindowSolarDiag {
    pub surface_id: u32,
    /// Beam POA after IAM correction [W/m²].
    pub poa_beam_w_m2: f64,
    /// Diffuse POA after IAM correction [W/m²].
    pub poa_diffuse_w_m2: f64,
    /// Beam IAM factor (0–1).
    pub iam_beam: f64,
    /// Diffuse IAM factor (0–1).
    pub iam_diffuse: f64,
    /// Transmitted beam solar [W].
    pub transmitted_beam_w: f64,
    /// Transmitted diffuse solar [W].
    pub transmitted_diffuse_w: f64,
    /// Absorbed inward-flowing solar [W] (glass absorptance × N_i × area × POA).
    pub absorbed_zone_w: f64,
    /// SHGC used for this window.
    pub shgc: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mechanical_ventilation_params_fields_accessible() {
        let params = MechanicalVentilationParams {
            zone_flow_m3_s: HashMap::from([(ZoneId(1), 0.035)]),
            balanced: true,
            sensible_recovery_efficiency: 0.75,
            latent_recovery_efficiency: 0.50,
        };

        assert_eq!(params.zone_flow_m3_s[&ZoneId(1)], 0.035);
        assert!(params.balanced);
        assert_eq!(params.sensible_recovery_efficiency, 0.75);
        assert_eq!(params.latent_recovery_efficiency, 0.50);

        let default = MechanicalVentilationParams::default();
        assert!(default.zone_flow_m3_s.is_empty());
        assert!(!default.balanced);
        assert_eq!(default.sensible_recovery_efficiency, 0.0);
        assert_eq!(default.latent_recovery_efficiency, 0.0);
    }

    /// OCHRE computes open_window_area = total_window_area × 0.67 × 0.5 × 0.2.
    /// When FractionOperable = 1.0, total_operable_area == total_window_area, so
    /// open_area must equal total_window_area × OPEN_AREA_FRACTION (= 0.067).
    #[test]
    fn open_area_fraction_matches_ochre_formula() {
        let total_window_area_m2 = 10.0_f64;
        let ochre_open_area = total_window_area_m2 * 0.67 * 0.5 * 0.2;
        let hares_open_area = total_window_area_m2 * NaturalVentilationConfig::OPEN_AREA_FRACTION;
        assert!(
            (ochre_open_area - hares_open_area).abs() < 1e-9,
            "HARES open area {hares_open_area} does not match OCHRE {ochre_open_area}"
        );
        // When FractionOperable = 1.0, operable area equals total area;
        // the solver_builder formula must include the 0.67 factor.
        let operable_area = total_window_area_m2 * 1.0_f64;
        let solver_builder_open_area = operable_area * NaturalVentilationConfig::OPEN_AREA_FRACTION;
        assert!(
            (solver_builder_open_area - ochre_open_area).abs() < 1e-9,
            "solver_builder open area {solver_builder_open_area} does not match OCHRE {ochre_open_area}"
        );
    }
}
