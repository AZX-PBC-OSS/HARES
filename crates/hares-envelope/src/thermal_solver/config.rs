use std::collections::HashMap;

use hares_types::ZoneId;
use thiserror::Error;

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
    /// Whether the ventilation system is balanced (ERV, HRV, or balanced fan).
    /// Recovery efficiencies are only applied for balanced systems.
    /// Unbalanced fans use quadrature combination with infiltration.
    /// OCHRE Envelope.py:59-87 and hpxml.py:556.
    pub balanced: bool,
    /// Sensible heat recovery efficiency [0.0–1.0].
    /// Reduces the sensible ventilation load for balanced systems.
    /// Parsed from HPXML `SensibleRecoveryEfficiency`.
    pub sensible_recovery_efficiency: f64,
    /// Latent heat recovery efficiency [0.0–1.0].
    /// Reduces the latent ventilation load for balanced systems.
    /// Derived as `TotalRecoveryEfficiency - SensibleRecoveryEfficiency` (OCHRE hpxml.py:560).
    pub latent_recovery_efficiency: f64,
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
///
/// Pre-computed `transmittance` and `radiation_frac` decompose SHGC into
/// transmitted and absorbed fractions per EnergyPlus Steps 4–5.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowSolarProperties {
    /// Solar heat gain coefficient at normal incidence [dimensionless].
    pub shgc: f64,
    /// Window U-factor [W/(m²·K)], used to select the EnergyPlus glazing curve.
    pub u_factor_w_m2_k: f64,
    /// Glazing area [m²].
    pub area_m2: f64,
    /// Solar transmittance at normal incidence [dimensionless].
    /// Fraction of incident solar that passes directly through the glass.
    pub transmittance: f64,
    /// Inward-flowing fraction of absorbed solar [dimensionless].
    /// Fraction of glass-absorbed heat that reaches the interior zone;
    /// `(1 - radiation_frac)` is lost to the exterior.
    pub radiation_frac: f64,
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
    /// (no film resistance, or film resistance already lumped into the node).
    /// For lightweight boundaries with significant film resistance the correction
    /// can be several degrees.
    ///
    /// Range: `[0, 1]`.  Default: `1.0`.
    pub radiation_frac: f64,
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
/// it is not cached here so that the struct remains free of init ordering.
///
/// `t_prev_c` is mutable persistent state updated each timestep, so `Copy` is not derived.
#[derive(Debug, Clone, PartialEq)]
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
    /// Radiation fraction: `R_film / (R_film + R_outermost_half)` — dimensionless [0,1].
    ///
    /// Controls how much the true surface temperature deviates from the RC node
    /// temperature. A value of 0.0 means use the node temperature directly (no
    /// exterior film resistance in the conduction path).
    pub rad_frac: f64,
    /// Radiation resistance: `R_film / area_m2` — K/W.
    ///
    /// Converts net surface heat flux [W] to a temperature perturbation on the
    /// exterior surface.
    pub rad_res_k_w: f64,
    /// Number of sub-iterations per timestep: `ceil(dt_s / 300.0).max(1)`.
    pub n_iter: u32,
    /// Previous-timestep converged exterior surface temperature [°C].
    ///
    /// Persistent state updated each timestep for heavy-ball damping.
    pub t_prev_c: f64,
    /// Solar absorptance [-] (0–1). Default 0.60 for opaque surfaces, 0.05 for
    /// radiant barriers. Ref: OCHRE `Envelope.py:222`.
    pub absorptance: f64,
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
    /// Surfaces absent from this map are treated as opaque; opaque solar gain
    /// is delivered via [`ExteriorSurfaceInfo::absorptance`] in `exterior_surfaces`.
    pub window_properties: HashMap<u32, WindowSolarProperties>,
    /// Exterior surfaces for LWR and opaque solar gain.
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
    /// Per-zone infiltration methods. Zones not listed default to zero ACH.
    /// Uses a `Vec` instead of `HashMap` for cache-friendly hot-loop iteration.
    pub infiltration: Vec<(ZoneId, InfiltrationMethod)>,
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
    /// Interior longwave radiation exchange net to indoor zone [W].
    pub interior_lwr_w: f64,
    /// Infiltration sensible heat gain (indoor zone only) [W].
    pub infiltration_w: f64,
    /// Forced mechanical ventilation sensible heat gain (indoor zone only) [W].
    pub ventilation_w: f64,
    /// Natural ventilation sensible heat gain [W].
    pub natural_ventilation_w: f64,
    /// Total sensible gains from all equipment ports (HVAC + appliances) [W].
    pub port_sensible_w: f64,
    /// HVAC heating contribution to the indoor zone [W].
    pub hvac_heating_w: f64,
    /// HVAC cooling contribution to the indoor zone [W].
    pub hvac_cooling_w: f64,
    /// Non-HVAC internal gains (appliances, lighting, occupancy, jacket losses) [W].
    /// Equals `InternalGain + JacketLoss` category totals.
    pub internal_gain_w: f64,
    /// Duct distribution losses to the indoor zone [W].
    pub duct_loss_w: f64,
    /// Per-zone infiltration sensible heat gains [W].
    /// `infiltration_w` is the indoor-zone alias for backward compatibility.
    pub infiltration_by_zone: Vec<(ZoneId, f64)>,
    /// Per-zone interior LWR net heat gains [W].
    pub interior_lwr_by_zone: Vec<(ZoneId, f64)>,
}
