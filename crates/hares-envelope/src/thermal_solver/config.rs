use std::collections::HashMap;

use crate::NodeId;
pub use hares_physics::infiltration::OpeningType;
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
/// Computes convective heat transfer from the interior surface to zone air
/// using the TARP natural convection model evaluated per timestep:
///   `T_surface = radiation_frac × T_node + (1 - radiation_frac) × T_zone`
///   `h_conv = tarp_h_natural(tilt_deg, |T_surface - T_zone|, above_hotter)`
///   `Q_conv = h_conv × area_m2 × (T_surface - T_zone)`
///
/// The TARP model (Walton 1983) is the EnergyPlus default for interior
/// convection and scales with the cube root of surface-to-air ΔT.
/// Per-step recomputation fixes the frozen-film-coefficient defect documented
/// in T-0082 (interior film coefficients must recompute per timestep).
///
/// Note: the A-matrix conductance still uses the frozen init-time film
/// resistance; this diagnostic-only fix reports the physically correct
/// convective flux without changing the state-space discretization.
///
/// This matches OCHRE's `H_{surface}_{zone}` energy flow variable.
#[derive(Debug, Clone)]
pub enum BoundaryDiagnosticInfo {
    /// Boundary with RC interior node -- uses surface temperature from state vector.
    /// `T_surface = radiation_frac × T_node + (1 - radiation_frac) × T_zone`
    /// `Q = h_tarp(ΔT) × area × (T_surface - T_zone)`  [per-step TARP, not frozen R_film]
    RCNode {
        inner_state_index: usize,
        area_m2: f64,
        /// Surface tilt from horizontal [°]; 0° = horizontal roof, 90° = vertical wall.
        /// Used to select the correct TARP natural convection formula branch.
        /// Ref: Walton, G. N. 1983. TARP Reference Manual, NBSSIR 83-2655.
        tilt_deg: f64,
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

/// Per-boundary data for per-step interior convection injection.
///
/// Used when [`FilmCoefficientModel::PerStepTarp`] to compute the per-step
/// convective heat transfer correction ΔQ = (h_tarp − h_static) × A × ΔT
/// and inject it into the explicit forcing vector before each step.
#[derive(Debug, Clone)]
pub struct InteriorConvectionInjection {
    /// Row index of the innermost wall-layer node in the state vector.
    pub surface_state_index: usize,
    /// Row index of the zone air node in the state vector.
    pub zone_state_index: usize,
    /// Surface area [m²].
    pub area_m2: f64,
    /// Surface tilt from horizontal [°]; 0° = horizontal, 90° = vertical.
    pub tilt_deg: f64,
    /// Static interior film resistance [m²·K/W] from the A-matrix
    /// (ASHRAE Simple value, frozen at init). The correction subtracts
    /// this path's contribution from the forcing.
    pub static_r_film_int_m2_k_w: f64,
    /// Thermal capacitance of the surface layer node [J/K].
    pub c_surface_j_k: f64,
    /// Thermal capacitance of the zone air node [J/K].
    pub c_zone_j_k: f64,
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
/// Implements the EnergyPlus `ZoneVentilation:WindandStackOpenArea` model:
/// wind and stack flow components are computed separately and combined in
/// quadrature following the EnergyPlus Engineering Reference §15.4 formula
/// `Q = sqrt(Q_wind² + Q_stack²)` and ASHRAE HoF 2009 Ch. 16.14.
///
/// The stack term depends on the [`OpeningType`]:
/// - [`CrossVentilation`](OpeningType::CrossVentilation): `Q_stack = Cd × A × sqrt(2·g·dh_m·|ΔT|/T_zone)`
///   where `dh_m` is the vertical separation between inlet and outlet openings
///   (EnergyPlus `DH` parameter).
/// - [`SingleSided`](OpeningType::SingleSided): `Q_stack = Cd × A × sqrt(2·g·zone_height_m·|ΔT|/T_zone)`
///   where `zone_height_m` is the characteristic opening height.
///
/// The wind term uses the wind-direction-dependent opening effectiveness `Cw`
/// from [`compute_natural_ventilation_cw`](hares_physics::infiltration::compute_natural_ventilation_cw):
/// `Q_wind = Cw × A × U_wind`
/// (ASHRAE HoF 2009 Ch. 16.14, Equation 37).
///
/// Flow is gated by temperature and outdoor humidity conditions per the
/// OCHRE/ResStock natural ventilation model.
///
/// # Defaults
/// - `t_base_c`: 22.778 °C (73 °F) -- OCHRE default comfort base temperature
/// - `max_outdoor_humidity_ratio`: 0.0115 kg/kg -- Building America HSP threshold
/// - `OPEN_AREA_FRACTION`: 0.067 -- matches OCHRE (0.67 × 0.5 × 0.2 of total window area)
/// - `opening_azimuth_deg`: 180° (South-facing, common for passive cooling in northern hemisphere)
/// - `opening_type`: [`CrossVentilation`](OpeningType::CrossVentilation)
/// - `dh_m`: 0.0 -- no stack benefit unless explicitly set
/// - `zone_height_m`: 2.5 m (typical residential ceiling height)
///
/// # References
/// - EnergyPlus `ZoneEquipmentManager.cc:5988–6033` — `WindAndStack` runtime calculation
/// - EnergyPlus `DataHeatBalance.hh:1180–1187` — `WindandStackOpenArea` struct (DH, DiscCoef)
/// - ASHRAE HoF 2009 Ch. 16.14, Equation 37: `Q = Cw × A × U`
/// - EnergyPlus Engineering Reference §15.4: `Q = sqrt(Qw² + Qst²)`
#[derive(Debug, Clone, PartialEq)]
pub struct NaturalVentilationConfig {
    /// Effective operable window area [m²].
    ///
    /// Typically computed as `total_window_area_m2 * OPEN_AREA_FRACTION`.
    /// Use [`NaturalVentilationConfig::from_window_area`] to apply the standard fraction.
    pub open_area_m2: f64,
    /// Vertical separation between inlet and outlet openings [m].
    ///
    /// EnergyPlus `DH` parameter. Used for cross-ventilation stack computation.
    /// Default 0.0 — produces no stack flow unless explicitly set.
    pub dh_m: f64,
    /// Zone height [m]; used as characteristic opening height for single-sided
    /// stack computation. Typical residential value: 2.5 m.
    pub zone_height_m: f64,
    /// Opening type: single-sided or cross-ventilation.
    /// Controls the discharge coefficient Cd and whether `dh_m` drives stack flow.
    pub opening_type: OpeningType,
    /// Comfort base temperature [°C]. Flow is suppressed when `T_zone ≤ t_base_c`.
    pub t_base_c: f64,
    /// Maximum outdoor specific humidity [kg/kg] above which nat vent is suppressed.
    pub max_outdoor_humidity_ratio: f64,
    /// Azimuth of the opening normal [°], 0° = North, clockwise.
    ///
    /// Default 180° (South-facing) is the common orientation for passive cooling
    /// in the northern hemisphere. Used to compute the opening effectiveness Cw
    /// from the angle between wind direction and opening normal per the EnergyPlus
    /// linear interpolation (ASHRAE HoF 2009 Ch. 16.14, Eq. 37).
    pub opening_azimuth_deg: f64,
}

impl NaturalVentilationConfig {
    /// OCHRE default open-window fraction of total window area (0.67 × 0.5 × 0.2).
    pub const OPEN_AREA_FRACTION: f64 = 0.067;
    /// OCHRE default comfort base temperature (73 °F in °C).
    pub const DEFAULT_T_BASE_C: f64 = 22.778;
    /// Building America HSP outdoor humidity threshold [kg/kg].
    pub const DEFAULT_MAX_OUTDOOR_HUMIDITY_RATIO: f64 = 0.0115;
    /// Default opening azimuth [°] -- South-facing, common for passive cooling in the northern hemisphere.
    pub const DEFAULT_OPENING_AZIMUTH_DEG: f64 = 180.0;
    /// Default zone height [m] for single-sided stack computation.
    pub const DEFAULT_ZONE_HEIGHT_M: f64 = 2.5;

    /// Construct from total window area; applies the standard 6.7% open-area fraction.
    ///
    /// Uses [`CrossVentilation`](OpeningType::CrossVentilation) with `dh_m = 0.0` —
    /// no stack benefit until `dh_m` and `zone_height_m` are set explicitly.
    pub fn from_window_area(total_window_area_m2: f64) -> Self {
        Self {
            open_area_m2: total_window_area_m2 * Self::OPEN_AREA_FRACTION,
            dh_m: 0.0,
            zone_height_m: Self::DEFAULT_ZONE_HEIGHT_M,
            opening_type: OpeningType::CrossVentilation,
            t_base_c: Self::DEFAULT_T_BASE_C,
            max_outdoor_humidity_ratio: Self::DEFAULT_MAX_OUTDOOR_HUMIDITY_RATIO,
            opening_azimuth_deg: Self::DEFAULT_OPENING_AZIMUTH_DEG,
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
    /// Surface tilt from horizontal [°]; 0° = horizontal, 90° = vertical.
    /// Used by autosizing to compute per-surface design-day solar irradiance.
    pub tilt_deg: f64,
    /// Surface azimuth [°] clockwise from north.
    /// Used by autosizing to compute per-surface design-day solar irradiance.
    pub azimuth_deg: f64,
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
    /// Whether this surface is a floor. Beam solar is distributed by the
    /// cosine model (`beam_cosine_factor`): each face's weight is
    /// area × absorptance × max(0, cos-of-incidence) against the sun
    /// position — floors dominate near noon, the wall opposite the sun
    /// dominates at low altitude; `is_floor` is retained for reporting and
    /// legacy call paths only.
    pub is_floor: bool,
    /// Boundary tilt [°] (0 = ceiling, 90 = wall, 180 = floor) — used by the
    /// cosine beam-solar distribution.
    pub tilt_deg: f64,
    /// Boundary outward azimuth [°] clockwise from north (180 = south).
    pub azimuth_deg: f64,
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
    /// In production this is always `Some(zone_air_idx)`. Windows are
    /// excluded from the radiant distribution by `solar_absorptance = 0.0`
    /// (set at construction in solver_builder.rs), not by a `None` index.
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
    /// Boundary tilt [°] (0 = horizontal facing up / ceiling, 90 = vertical
    /// wall, 180 = horizontal facing down / floor) — the HPXML convention
    /// shared with `ExteriorSurfaceInfo::tilt_deg`. Used by the cosine
    /// beam-solar distribution: the beam illuminates the interior face
    /// in proportion to `max(0, u_sun · n_in)` with `n_in` the into-room
    /// normal derived from tilt/azimuth.
    pub tilt_deg: f64,
    /// Boundary outward azimuth [°] clockwise from north (180 = south).
    /// See `tilt_deg` for the distribution role.
    pub azimuth_deg: f64,
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
    /// Surface azimuth [°] clockwise from north.
    /// Used by autosizing to compute per-surface design-day solar irradiance.
    pub azimuth_deg: f64,
    /// Radiation fraction: `R_film / (R_film + R_outermost_half)` -- dimensionless [0,1].
    ///
    /// Controls how much the true surface temperature deviates from the RC node
    /// temperature. A value of 0.0 means use the node temperature directly (no
    /// exterior film resistance in the conduction path).
    pub rad_frac: f64,
    /// Radiation resistance [K/W]: the exact eliminated-skin (Thévenin)
    /// resistance `R_film·R_half/(R_film + R_half) / area_m2`, where `R_half`
    /// is the adjacent material half-layer resistance. Identity (pinned by
    /// `skin_rad_coupling_uses_parallel_resistance`):
    /// `rad_res_k_w == rad_frac · R_half / area_m2`.
    ///
    /// Deliberately divergent from OCHRE, whose `radiation_res` is the bare
    /// film `R_film/area` with the exact parallel form commented out in its
    /// own source (Envelope.py:258) under a `res_material >> res_film`
    /// assumption. The bare form over-drives the skin temperature whenever
    /// the half-layer conducts comparably to the film (thin siding, stucco,
    /// metal). See docs/alignment/DIVERGENCES.md.
    ///
    /// Converts net surface heat flux [W] to a temperature perturbation on
    /// the exterior surface.
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
    /// Used as denominator in T_eff = T_air + Δq / h_out. ASHRAE HoF 2021 Ch. 15, Table 1.
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
    /// Per-node thermal capacitance [J/K] for all internal RC nodes.
    ///
    /// Includes zone air nodes, wall-layer nodes, and interior mass nodes.
    /// Populated during `assemble_building_rc` and forwarded through
    /// `solver_builder.rs` to the solver. Used by `integrate_inner` to compute
    /// the full-system energy balance: `Σ C_i × ΔT_i / dt` over all thermal nodes
    /// rather than just zone air nodes. This enables a mathematically exact
    /// closure check that accounts for wall-mass energy redistribution,
    /// conduction through the A-matrix, and all B_d column contributions.
    ///
    /// Reference: EnergyPlus Engineering Reference "Basis for the Zone and Air
    /// System Integration" — the heat balance method requires that the sum of
    /// all thermal energy flows across the system boundary equals the rate of
    /// change of stored energy in all thermal capacitances.
    pub node_capacitances: HashMap<NodeId, f64>,
    /// NodeId → state-vector row index.
    ///
    /// Maps each internal RC node to its row in the state vector `x`. Precomputed
    /// during `assemble_building_rc` from `internal_node_order`.
    pub node_index: HashMap<NodeId, usize>,
}

/// Interior convective film coefficient model selection.
///
/// Controls whether the interior convective coefficient is frozen at init time
/// (ASHRAE Simple, backward-compatible) or recomputed per timestep from the
/// TARP natural convection model (default going forward).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FilmCoefficientModel {
    /// Frozen ASHRAE Simple values by surface orientation only (backward compatible).
    /// h_conv does not depend on surface-to-air ΔT. Same values as EnergyPlus
    /// `CalcASHRAESimpleIntConvCoeff` and OCHRE's init-time calculation.
    AshraeSimple,
    /// Per-step TARP natural convection via [`tarp_h_natural`] (EnergyPlus default).
    /// h_conv ∝ |ΔT|^(1/3), evaluated each timestep from T_surface and T_zone,
    /// injected as explicit forcing rather than a static A-matrix conductance.
    ///
    /// Reference: Walton, G. N. 1983. TARP Reference Manual, NBSSIR 83-2655, Eqs. 90-92.
    #[default]
    PerStepTarp,
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
    /// Interior convective film coefficient model.
    ///
    /// `PerStepTarp` (default): recomputes h_conv from actual surface-to-zone
    /// ΔT each timestep using the TARP natural convection model. The convective
    /// flux is injected as explicit forcing, keeping the A-matrix static.
    /// `AshraeSimple`: frozen init-time coefficients by surface orientation
    /// (backward compatible with prior HARES and OCHRE behavior).
    pub film_coefficient_model: FilmCoefficientModel,
    /// Per-boundary interior convection injection metadata.
    ///
    /// Populated when `film_coefficient_model == PerStepTarp` and the boundary
    /// has RC nodes. Empty vec when using `AshraeSimple` (backward compat).
    pub interior_convection_injections: Vec<InteriorConvectionInjection>,
    /// Maximum number of consecutive solver non-convergence events before the
    /// solver falls back to the last-good capacity value for a zone.
    ///
    /// Default 3 consecutive convergence failures is a reasonable signal that
    /// the solver is unable to reach the setpoint under current conditions
    /// (extreme weather, sizing mismatch, or numerical ill-conditioning).
    pub ideal_capacity_degraded_threshold: usize,
}

impl ThermalSolverConfig {
    /// Validates structural invariants that are otherwise only discoverable
    /// by reading two files side by side. Called by [`ThermalSolver::new`]
    /// with the assembled model's dimensions so mis-wiring fails fast with
    /// the offending surface named, instead of silently double-injecting or
    /// dropping a surface's flux.
    ///
    /// Checks, per exterior surface: finite non-negative area (zero is the
    /// inert-surface marker used by synthetic test buildings — every flux
    /// term scales with area and vanishes), emissivity in
    /// (0, 1], absorptance in [0, 1], `n_iter >= 1`, `rad_frac` in [0, 1]
    /// with a finite non-negative `rad_res_k_w`, and state/input indices
    /// within the model dimensions. Across surfaces: `surface_id` uniqueness
    /// (a duplicate breaks irradiance lookup). `input_index` and
    /// `state_index` are NOT checked for uniqueness here: multiple exterior
    /// faces of one assembly legitimately share a mass node, and surfaces
    /// without their own injection column (windows, fallback-R walls)
    /// legitimately share a zone's sensible-heat column, which is additive.
    /// True double-registration of a dedicated injection column is checked
    /// against the wiring in [`ThermalSolver::new`].
    pub fn validate(&self, n_states: usize, n_inputs: usize) -> std::result::Result<(), String> {
        let mut seen_surface_ids = std::collections::HashSet::new();
        for info in &self.exterior_surfaces {
            let surface = format!("exterior surface {}", info.surface_id);
            // Area 0 is the legitimate "inert surface" marker used by
            // synthetic test buildings (all flux terms scale with area and
            // vanish); negative or non-finite area is a wiring error.
            if !info.area_m2.is_finite() || info.area_m2 < 0.0 {
                return Err(format!(
                    "{surface}: area must be finite and non-negative, got {}",
                    info.area_m2
                ));
            }
            if !(info.emissivity.is_finite() && 0.0 < info.emissivity && info.emissivity <= 1.0) {
                return Err(format!(
                    "{surface}: emissivity must be in (0, 1], got {}",
                    info.emissivity
                ));
            }
            if !info.absorptance.is_finite() || !(0.0..=1.0).contains(&info.absorptance) {
                return Err(format!(
                    "{surface}: solar absorptance must be in [0, 1], got {}",
                    info.absorptance
                ));
            }
            if info.n_iter == 0 {
                return Err(format!("{surface}: n_iter must be >= 1"));
            }
            if !info.rad_frac.is_finite() || !(0.0..=1.0).contains(&info.rad_frac) {
                return Err(format!(
                    "{surface}: rad_frac must be in [0, 1], got {}",
                    info.rad_frac
                ));
            }
            if !info.rad_res_k_w.is_finite() || info.rad_res_k_w < 0.0 {
                return Err(format!(
                    "{surface}: rad_res_k_w must be finite and non-negative, got {}",
                    info.rad_res_k_w
                ));
            }
            if info.state_index >= n_states {
                return Err(format!(
                    "{surface}: state_index {} out of range (model has {n_states} states)",
                    info.state_index
                ));
            }
            if info.input_index >= n_inputs {
                return Err(format!(
                    "{surface}: input_index {} out of range (model has {n_inputs} inputs)",
                    info.input_index
                ));
            }
            if !seen_surface_ids.insert(info.surface_id) {
                return Err(format!(
                    "{surface}: duplicate surface_id in exterior_surfaces \
                     (double registration)"
                ));
            }
        }
        Ok(())
    }
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
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::StarMesh,
            film_coefficient_model: FilmCoefficientModel::default(),
            interior_convection_injections: Vec::new(),
            ideal_capacity_degraded_threshold: 3,
        }
    }
}

#[derive(Debug, Error)]
pub enum ThermalSolverError {
    #[error("invalid zone mapping: zone {zone:?} is missing from {field}")]
    MissingZoneMapping { zone: ZoneId, field: &'static str },
    #[error(
        "indoor zone {id:?} is not registered in zone_state_indices; registered zones: {registered:?}"
    )]
    IndoorZoneIdNotRegistered { id: ZoneId, registered: Vec<ZoneId> },
    #[error(
        "zone {zone_id:?} state index {index} is out of bounds for state dimension {state_dim}"
    )]
    ZoneStateIndexOutOfBounds {
        zone_id: ZoneId,
        index: usize,
        state_dim: usize,
    },
    #[error(
        "zone {zone_id:?} is registered in zone_state_indices but absent from env.zones; \
         zones in env: {present_zones:?}"
    )]
    ZoneNotInEnvironment {
        zone_id: ZoneId,
        present_zones: Vec<ZoneId>,
    },
    #[error("failed to initialize steady-state vector: {0}")]
    Initialization(String),
    #[error(
        "zone {zone_id:?} capacitance {capacitance_j_k:.3e} J/K too small for steady-state pinning: \
            the RC network or discretization parameters need attention"
    )]
    SingularInitialization {
        zone_id: ZoneId,
        capacitance_j_k: f64,
    },
    #[error("{0}")]
    Configuration(String),
}

pub type Result<T> = std::result::Result<T, ThermalSolverError>;

/// Per-timestep envelope component gains [W] for output/diagnostics.
///
/// Values are signed: positive = heat flowing INTO the indoor zone — with two
/// documented exceptions: `opaque_solar_lwr_w`, `opaque_solar_w`, and
/// `exterior_lwr_w` are *gross exterior-skin* fluxes (outside-face balance
/// drivers, OCHRE "Ext. Solar/LWR Gain"), and `interior_lwr_w` is a gross
/// exchange activity metric. None of those are net zone loads; see their
/// field docs.
/// Populated after each `resolve()` call; read via [`ThermalSolver::component_gains`].
#[derive(Debug, Clone, Default)]
pub struct EnvelopeComponentGains {
    /// Window transmitted solar (SHGC × IAM × area × POA) [W].
    pub window_solar_w: f64,
    /// Opaque exterior surface solar + LWR combined — the absorbed gross
    /// at the exterior skins, summed over both application paths
    /// (iterative and non-iterative).
    /// This is the *gross* radiant flux absorbed at the exterior skin — the
    /// outside-face heat balance driver (EnergyPlus ERM 26.1 — "Outside
    /// Surface Heat Balance"; reported by OCHRE as "{boundary} Ext.
    /// Solar/LWR Gain") — applied as a boundary condition on each surface's
    /// own exterior RC node. It is not a load on the conditioned zone: most
    /// of it re-leaves via exterior convection and sky longwave exchange,
    /// and only a small, time-lagged fraction conducts through to zone air.
    /// For the net delivered heat (the inside-face balance, EnergyPlus ERM
    /// 26.1 — "Inside Heat Balance": Interior Convection) see
    /// `wall_heat_gain_w`, `floor_heat_gain_w`, and `roof_heat_gain_w`.
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
    pub port_convective_w: f64,
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
    /// Equipment jacket/shell losses summed across all zone accumulators [W].
    /// Water heaters and boilers in unconditioned zones deposit losses into that
    /// zone's accumulator; this field captures all zones (water heater skin loss,
    /// boiler shell loss, etc.).
    pub jacket_loss_w: f64,
    /// Per-zone jacket loss breakdown [W] — zone ID → sensible watts.
    /// Gated by `observe` feature; complements the aggregate `jacket_loss_w`
    /// for verifying equipment in unconditioned zones produces expected output.
    #[cfg(feature = "observe")]
    pub jacket_loss_by_zone: Vec<(ZoneId, f64)>,
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
    /// Net sensible heat delivered from wall-boundary interior surfaces to
    /// zone air [W] — the inside-face convection term (EnergyPlus ERM 26.1 —
    /// "Inside Heat Balance": Interior Convection; TARP per Walton 1983),
    /// i.e. the time-lagged result of exterior solar, LWR, and ΔT conduction
    /// drivers (OCHRE's "Wall Heat Gain - Indoor").
    pub wall_heat_gain_w: f64,
    /// Net sensible heat delivered from floor-boundary interior surfaces to
    /// zone air [W] (OCHRE's "Floor Heat Gain - Indoor").
    pub floor_heat_gain_w: f64,
    /// Net sensible heat delivered from roof-boundary interior surfaces to
    /// zone air [W] (OCHRE's "Roof Heat Gain - Indoor").
    pub roof_heat_gain_w: f64,
    /// Net sensible heat delivered from window interior surfaces to zone
    /// air [W] (convection; transmitted solar is reported separately as
    /// `window_solar_w`).
    pub window_heat_gain_w: f64,
    /// Net sensible heat delivered from internal-mass surfaces to zone air [W].
    pub internal_mass_heat_gain_w: f64,
    /// Zone air heat-balance residual [W] for the indoor zone, the
    /// difference between stored energy and every accounted term:
    /// `C_zone·ΔT/dt − u[zone] injections − matrix exchange − airflow
    /// terms`. The matrix exchange is computed from
    /// the discrete state equation (A_d−I row + environmental B_d
    /// columns), so it includes interior LWR and steady-state boundary
    /// conduction exactly as the solver moves them. The remaining residual
    /// is the semi-implicit coupling split — at hourly timesteps on a light
    /// air node (d = h·dt/C_zone ≈ 7), the implicit-vs-explicit difference
    /// is legitimately O(100–900 W) on freefloat-class swings. O(kW)
    /// sustained values beyond that envelope indicate mis-wired gains —
    /// the I-02 defect class. Diagnostic only.
    pub zone_air_balance_residual_w: f64,
    /// Absorbed opaque exterior solar [W] — the full skin-absorbed flux
    /// `α·A·POA`, summed over both application paths. The non-iterative
    /// path (rad_frac == 0) injects the full absorbed flux directly
    /// (solar.rs); the iterative path (rad_frac > 0) accumulates its
    /// absorbed solar per surface during the exterior-radiation solve
    /// (longwave.rs), separate from the rad_frac-scaled share injected into
    /// the RC node. OCHRE parity: "{boundary} Ext. Solar Gain (W)". For the
    /// per-surface split see `ExtSurfaceDiag::solar_absorbed_w`
    /// (`observe_detailed`).
    pub opaque_solar_w: f64,
    /// Net exterior longwave radiation exchange at the opaque skins [W],
    /// summed over both application paths: positive when the environment
    /// (air + sky) radiates more into the surface than the surface emits —
    /// typically negative on clear nights. OCHRE parity: "{boundary} Ext.
    /// LWR Gain (W)". A gross exterior-skin quantity (EnergyPlus ERM 26.1 —
    /// "Outside Surface Heat Balance"), not a net load on the conditioned
    /// zone. For the per-surface split see `ExtSurfaceDiag`
    /// (`observe_detailed` feature).
    pub exterior_lwr_w: f64,
    /// Window exterior LWR beyond U-factor assumption [W].
    /// When T_sky < T_air (clear night), windows radiate more to sky than the
    /// U-factor (which assumes T_sky ≈ T_air) accounts for. This field tracks
    /// that additional cooling. Negative = cooling. Zero when T_sky = T_air.
    /// Ref: Walton (1983); E+ Eng.Ref "External Longwave Radiation"; ASHRAE HoF 2021 Ch. 15.
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
    /// Natural ventilation stack-driven flow component [m³/s].
    #[cfg(feature = "observe")]
    pub natural_ventilation_q_stack_m3_s: f64,
    /// Natural ventilation wind-driven flow component [m³/s].
    #[cfg(feature = "observe")]
    pub natural_ventilation_q_wind_m3_s: f64,
    /// Natural ventilation discharge coefficient Cd used this timestep [-] min.
    #[cfg(feature = "observe")]
    pub natural_ventilation_cd_used: f64,
    /// Natural ventilation opening effectiveness Cw [0.0–0.55].
    #[cfg(feature = "observe")]
    pub natural_ventilation_cw: f64,
    /// Angle between wind direction and opening normal [°], in [0, 180].
    #[cfg(feature = "observe")]
    pub natural_ventilation_wind_angle_deg: f64,
    /// Outdoor moist-air density used for infiltration/ventilation mass flow conversion [kg/m³].
    /// Computed per timestep from outdoor T, P, and humidity ratio via ASHRAE HoF 2021 Ch.1 Eq.28.
    /// Included in diagnostic CSV (verbosity ≥ 4) for altitude-aware density verification.
    pub air_density_kg_m3: f64,
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
    /// Exterior film coefficient as received from boundary film resistance [W/(m²·K)].
    /// This is the raw computed value before the guard threshold is applied.
    pub h_out_computed_w_m2_k: f64,
    /// Effective exterior film coefficient after guard threshold [W/(m²·K)].
    /// May differ from `h_out_computed_w_m2_k` when the computed value is below
    /// the 1.0 W/(m²·K) natural convection floor and the ASHRAE fallback (34.0)
    /// is substituted.
    pub h_out_effective_w_m2_k: f64,
    /// Whether the ASHRAE fallback was triggered (computed h_out < 1.0).
    pub h_out_fallback_triggered: bool,
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

    fn valid_exterior_surface(
        surface_id: u32,
        state_index: usize,
        input_index: usize,
    ) -> ExteriorSurfaceInfo {
        ExteriorSurfaceInfo {
            surface_id,
            state_index,
            input_index,
            area_m2: 20.0,
            emissivity: 0.9,
            tilt_deg: 90.0,
            azimuth_deg: 180.0,
            rad_frac: 0.375,
            rad_res_k_w: 9.375e-4,
            n_iter: 3,
            absorptance: 0.7,
            boundary_category: Some(BoundaryCategory::Wall),
            u_factor_w_m2_k: 0.0,
            h_out_w_m2_k: 33.0,
        }
    }

    #[test]
    fn validate_accepts_well_formed_exterior_surfaces() {
        let config = ThermalSolverConfig {
            exterior_surfaces: vec![
                valid_exterior_surface(1, 0, 1),
                valid_exterior_surface(2, 2, 3),
            ],
            ..ThermalSolverConfig::default()
        };
        assert!(config.validate(4, 6).is_ok());
    }

    #[test]
    fn validate_rejects_duplicate_surface_registration_with_surface_named() {
        let config = ThermalSolverConfig {
            exterior_surfaces: vec![
                valid_exterior_surface(7, 0, 1),
                valid_exterior_surface(7, 2, 3),
            ],
            ..ThermalSolverConfig::default()
        };
        let err = config.validate(4, 6).unwrap_err();
        assert!(
            err.contains("exterior surface 7") && err.contains("duplicate surface_id"),
            "error must name the offending surface, got: {err}"
        );
    }

    #[test]
    fn validate_rejects_duplicate_input_and_state_indices() {
        // Duplicate input_index is NOT rejected here by design: surfaces
        // without a dedicated injection column (windows, fallback-R walls)
        // legitimately share a zone's additive sensible-heat column. The
        // true double-registration check — sharing a DEDICATED column — is
        // wiring-aware and lives in `ThermalSolver::new`.
        let dup_input = ThermalSolverConfig {
            exterior_surfaces: vec![
                valid_exterior_surface(1, 0, 2),
                valid_exterior_surface(2, 3, 2),
            ],
            ..ThermalSolverConfig::default()
        };
        assert!(
            dup_input.validate(6, 6).is_ok(),
            "shared input_index is a wiring-level concern (zone columns are \
             additive); validate must not reject it without the wiring"
        );

        let dup_state = ThermalSolverConfig {
            exterior_surfaces: vec![
                valid_exterior_surface(1, 4, 1),
                valid_exterior_surface(2, 4, 3),
            ],
            ..ThermalSolverConfig::default()
        };
        assert!(
            dup_state.validate(6, 6).is_ok(),
            "shared state_index is a legitimate pattern (multiple faces of one \
             assembly share a mass node) and must not be rejected"
        );
    }

    #[test]
    fn validate_rejects_out_of_range_indices_with_dimensions_named() {
        let config = ThermalSolverConfig {
            exterior_surfaces: vec![valid_exterior_surface(9, 0, 5)],
            ..ThermalSolverConfig::default()
        };
        let err = config.validate(4, 4).unwrap_err();
        assert!(
            err.contains("exterior surface 9") && err.contains("input_index 5 out of range"),
            "error must name the surface and the range, got: {err}"
        );
    }

    #[test]
    fn validate_rejects_non_physical_coupling_parameters() {
        let mut surface = valid_exterior_surface(3, 0, 1);
        surface.rad_frac = 1.5;
        let config = ThermalSolverConfig {
            exterior_surfaces: vec![surface],
            ..ThermalSolverConfig::default()
        };
        assert!(config.validate(4, 4).unwrap_err().contains("rad_frac"));

        let mut surface = valid_exterior_surface(3, 0, 1);
        surface.area_m2 = -1.0;
        let config = ThermalSolverConfig {
            exterior_surfaces: vec![surface],
            ..ThermalSolverConfig::default()
        };
        assert!(config.validate(4, 4).unwrap_err().contains("area"));

        let mut surface = valid_exterior_surface(3, 0, 1);
        surface.rad_res_k_w = f64::NAN;
        let config = ThermalSolverConfig {
            exterior_surfaces: vec![surface],
            ..ThermalSolverConfig::default()
        };
        assert!(config.validate(4, 4).unwrap_err().contains("rad_res_k_w"));
    }

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
