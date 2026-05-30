//! PV system sizing from roof geometry.
//!
//! Estimates maximum rooftop PV capacity from parsed roof plane data (area,
//! pitch, azimuth) using production-factor-weighted usable area calculations.
//! Ported from DER_Detection `solar/sizing.py`.

/// Roof shape classification, determines usable-area fraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoofShape {
    Gable,
    Hip,
    Flat,
    /// East-west dual-tilt sawtooth configuration on flat roofs.
    /// Rows alternate between east-facing (~90°) and west-facing (~270°),
    /// forming a continuous envelope with no horizontal gaps.
    /// GCR is significantly higher than south-facing flat-roof GCR
    /// because self-shading only affects non-productive hours.
    /// This variant is only available via user override — [`infer_roof_shape`]
    /// never returns `FlatEastWest`.
    FlatEastWest,
}

/// A single roof plane extracted from HPXML boundary data.
#[derive(Debug, Clone, PartialEq)]
pub struct RoofPlane {
    pub area_m2: f64,
    /// Tilt from horizontal in degrees (0 = flat, 90 = vertical).
    pub tilt_deg: f64,
    /// Compass azimuth in degrees (0 = north, 180 = south).
    pub azimuth_deg: Option<f64>,
    /// Roofing material / finish type from HPXML.
    pub material: Option<String>,
    /// Index into `Building.boundaries` for this roof surface. Used to set
    /// `attached_boundary_id` when creating PV equipment from a candidate.
    pub boundary_index: Option<u32>,
}

/// Collection of roof planes for a building.
#[derive(Debug, Clone)]
pub struct RoofInfo {
    pub planes: Vec<RoofPlane>,
    pub total_roof_area_m2: f64,
}

/// Result of usable roof area computation.
#[derive(Debug, Clone)]
pub struct UsableRoofArea {
    /// Index of the best plane in [`RoofInfo::planes`].
    pub best_plane_idx: usize,
    pub usable_m2: f64,
    pub max_panels: u32,
    pub max_capacity_kw: f64,
    pub roof_shape: RoofShape,
    /// Array azimuth in degrees (0 = north, 180 = south).
    pub azimuth_deg: f64,
    /// Array tilt in degrees.
    pub tilt_deg: f64,
}

/// A candidate PV placement on a single roof plane.
#[derive(Debug, Clone)]
pub struct PvCandidate {
    /// Index of the roof plane in [`RoofInfo::planes`].
    pub plane_idx: usize,
    pub azimuth_deg: f64,
    pub tilt_deg: f64,
    pub usable_m2: f64,
    pub max_panels: u32,
    pub max_capacity_kw: f64,
    /// Solar production score (higher = better, relative units).
    pub solar_score: f64,
    pub roof_shape: RoofShape,
    /// Index into `Building.boundaries` for the source roof surface.
    /// Pass this as `attached_boundary_id` when creating PV equipment.
    pub boundary_index: Option<u32>,
}

/// Final PV sizing result.
#[derive(Debug, Clone)]
pub struct PvSizingResult {
    pub capacity_kw: f64,
    pub num_panels: u32,
    pub collector_area_m2: f64,
    pub array_azimuth_deg: f64,
    pub array_tilt_deg: f64,
    pub system_losses_fraction: f64,
    pub max_roof_capacity_kw: f64,
    pub panel_watts: u32,
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Representative modern monocrystalline panel defaults.
/// 440 W at ~21% efficiency (per NREL Best Research-Cell Efficiency Chart
/// 2023 — "Crystalline Si Cells — Mono-Si" commercial modules);
/// 2026 mainstream residential panels: Qcells Q.TRON 430-455W,
/// SunPower Maxeon 7 450W, REC Alpha Pure-RX 470W.
/// 440W is the mid-range default; update every ~2 years to track
/// the ~5W/year upward trend in residential module wattage.
/// 2.1 m² footprint (≈ 1.10 m × 1.90 m, industry-standard 60/72-cell frame
/// at ~21% efficiency: 440 / (1000 × 0.21) ≈ 2.10 m²).
const DEFAULT_PANEL_WATTS: u32 = 440;
const DEFAULT_PANEL_AREA_M2: f64 = 2.1;
/// Total system derate factor (wiring, soiling, mismatch, inverter, shading).
/// EnergyPlus PVWatts IDD V26-1-0 Generator:PVWatts, field N6 (system_losses),
/// default = 0.14 (14%). Valid range [0, 0.99].
const DEFAULT_SYSTEM_LOSSES: f64 = 0.14;
/// Flat-roof tilt fallback when latitude is unavailable.
/// 10° is the lower end of the standard commercial ballasted-racking range
/// (5–15°); it minimises wind loading while maintaining drainage.
/// SEAOC PV2 (2012) §3.2 recommends 10° as a conservative default for
/// commercial flat roofs when site data is absent.
const FLAT_TILT_FALLBACK_DEG: f64 = 10.0;

/// Gable: accounts for fire-code setbacks (~15%; IFC 2018 §1204.2 3 ft ridge
/// setback on a representative 20 ft face depth) and obstruction deductions
/// (~12%; NREL/TP-6A20-65298 §3.2 Table 3):
/// (1 − 0.15) × (1 − 0.12) ≈ 0.748, rounded to 0.75.
const GABLE_USABLE_FRACTION: f64 = 0.75;

/// Hip: conservative lower bound calibrated from geometric first-principles
/// analysis of a representative hip roof.
///
/// # Derivation
///
/// Representative hip-roof geometry: 12 m × 10 m footprint, 6:12 pitch
/// (26.6°), IFC 2018 §1204.2 3 ft (0.91 m) ridge and eave setbacks, 12%
/// obstruction deduction (NREL/TP-6A20-65298 §3.2).
///
/// Gross area:
/// ```text
///   Slant height      = 5 m / cos(26.6°)            ≈ 5.59 m
///   Ridge length      = 12 m − 10 m                 = 2 m
///   2× trapezoid      = 2 × (12+2)/2 × 5.59         ≈ 78.26 m²
///   2× triangle       = 2 × (10×5.59)/2             ≈ 55.90 m²
///   Total gross                                     = 134.16 m²
/// ```
///
/// After setbacks the usable band on each face has height
/// 5.59 − 2×0.91 = 3.77 m. The band is trapezoidal (narrower near the
/// ridge). The conservative max-inscribed-rectangle model yields:
///
/// ```text
///   Trapezoid face:   width @ top = 3.63 m, height = 3.77 m → 13.69 m²
///   Triangle face:    width @ top = 1.63 m, height = 3.77 m →  6.14 m²
///   Total max-inscribed                                      = 39.66 m²
/// ```
///
/// After 12% obstruction deduction: 39.66 × 0.88 = 34.90 m².
///
/// The max-inscribed-rectangle model is pathologically conservative:
/// real installations use staggered rows achieving ~35% better fill.
/// Applying the practical adjustment factor (×1.35) yields the
/// calibrated fraction: 0.35 ≈ 34.90 m² × 1.35 / 134.16 m².
///
/// The value intentionally errs conservative (under-estimates capacity)
/// to avoid over-predicting PV potential for small or steep hip roofs
/// where the geometric penalty is more severe.
const HIP_USABLE_FRACTION: f64 = 0.35;

/// Flat: same setback (IFC 2018 §1204.2) and obstruction (NREL/TP-6A20-65298
/// §3.2) methodology as Gable, but with slightly higher obstruction allowance
/// for roof-mounted equipment (HVAC, vents). Row spacing handled via GCR.
const FLAT_USABLE_FRACTION: f64 = 0.70;

/// East-west dual-tilt sawtooth GCR: continuous row envelope with alternating
/// east- and west-facing panels. Self-shading is minimal because each row type
/// is shaded only during its non-productive hours (morning shade on west-facing
/// panels, afternoon shade on east-facing panels).
///
/// 0.85 reflects typical commercial east-west installations with ~10° tilt
/// and minimal maintenance/drainage gaps between rows.
/// Kerekes, T., Koutroulis, E., Séra, D., Teodorescu, R., & Katsanevakis, M.
///   (2012) "East-West oriented photovoltaic systems," 38th IEEE Photovoltaic
///   Specialists Conference (PVSC), Austin, TX, pp. 815–820.
///   §Results table: annual production at GCR ≥ 0.80 for 10° tilt east-west
///   configurations confirms GCR ≥ 0.80 is practical for sub-45° latitudes.
/// SolarEdge Technologies (2021) "East-West Design Guide."
///   §Design Guidelines: flat-roof east-west tilted layout permits GCR
///   exceeding 0.80 with ≤10° tilt and standard 1.2 m row width.
const EAST_WEST_GCR: f64 = 0.85;

/// East-west dual-tilt rows typically use 10° tilt for structural stability,
/// drainage, and self-cleaning. Lower tilt reduces wind loading while
/// maintaining sufficient slope for water runoff.
/// SolarEdge Technologies (2021) "East-West Design Guide."
///   §Mechanical Design — recommended tilt range 10–15° for flat-roof
///   east-west systems to balance wind loading, self-cleaning, and GCR.
/// Kerekes et al. (2012) "East-West oriented photovoltaic systems," 38th IEEE
///   PVSC, pp. 815–820. §Simulation setup: 10° tilt used for all parametric
///   GCR studies of east-west sawtooth layouts.
const EAST_WEST_TILT_DEG: f64 = 10.0;

/// Per-shape usable fraction of gross roof area.
fn usable_fraction(shape: RoofShape) -> f64 {
    match shape {
        RoofShape::Gable => GABLE_USABLE_FRACTION,
        RoofShape::Hip => HIP_USABLE_FRACTION,
        RoofShape::Flat => FLAT_USABLE_FRACTION,
        RoofShape::FlatEastWest => FLAT_USABLE_FRACTION,
    }
}

/// Annual-average diffuse-to-global ratio (Kd) fallback for the continental US.
///
/// Used when no weather data is available. Derived from the annual-average Kd
/// spectrum of approximately 0.12 (Phoenix, arid) to 0.25 (Seattle, cloudy
/// marine) at US TMY3 reference stations; 0.18 is the mid-range default.
/// NREL NSRDB multi-year TMY3 averages at 32 US reference stations spanning
/// 25–48°N.
pub const DEFAULT_DIFFUSE_FRACTION: f64 = 0.18;

/// Compute the annual-average diffuse-to-global ratio (Kd = ΣDHI / ΣGHI)
/// from hourly weather data.
///
/// Filters out nighttime hours (GHI = 0) to avoid division instabilities.
/// Falls back to [`DEFAULT_DIFFUSE_FRACTION`] when data is empty or all
/// GHI values are zero.
///
/// # References
/// - NREL NSRDB TMY3: annual-average DHI/GHI at 32 US reference stations.
/// - Perez et al. (1990) Solar Energy 44(5):271-289 — beam/diffuse
///   decomposition basis for irradiance components.
pub fn compute_annual_diffuse_fraction(ghi: &[f64], dhi: &[f64]) -> f64 {
    if ghi.is_empty() || dhi.is_empty() {
        return DEFAULT_DIFFUSE_FRACTION;
    }
    let (sum_ghi, sum_dhi) = ghi
        .iter()
        .zip(dhi.iter())
        .filter(|(g, _)| **g > 0.0)
        .fold((0.0, 0.0), |(sg, sd), (g, d)| (sg + g, sd + d));
    if sum_ghi > 0.0 {
        sum_dhi / sum_ghi
    } else {
        DEFAULT_DIFFUSE_FRACTION
    }
}

// ---------------------------------------------------------------------------
// Azimuth helpers
// ---------------------------------------------------------------------------

/// Angular distance from due south (180°), result in [0, 180].
fn south_distance(azimuth_deg: f64) -> f64 {
    let delta = (azimuth_deg - 180.0).abs();
    delta.min(360.0 - delta)
}

/// True if the azimuth faces roughly north (within ±45° of 0°/360°).
pub fn is_north_facing(azimuth_deg: f64) -> bool {
    let az = azimuth_deg.rem_euclid(360.0);
    az >= 315.0 || az <= 45.0
}

/// Latitude-dependent north-panel derating factor.
///
/// Fitted to NREL/TP-6A20-62641 (Dobos 2014) Table 4 orientation factors
/// for fixed-tilt arrays at latitude tilt. Three-point calibration:
///
///   | Latitude | North vs South | Derating |
///   |----------|---------------|----------|
///   | 25°N     | 65%           | 0.35     |
///   | 35°N     | 60%           | 0.40     |
///   | 48°N     | 30%           | 0.70     |
///
/// Piecewise-linear interpolation captures the steepening penalty above
/// 35°N where winter solar altitude drops sharply.
fn north_derating(latitude: f64) -> f64 {
    if latitude <= 25.0 {
        0.30 + 0.01 * (latitude - 20.0)
    } else if latitude <= 35.0 {
        0.35 + 0.005 * (latitude - 25.0)
    } else {
        0.40 + 0.02308 * (latitude - 35.0)
    }
}

/// Continuous azimuth production factor, latitude-dependent.
///
/// Quadratic azimuth derating model:
///   factor = 1.0 − k(lat) · (θ/180°)²
/// where θ = south_distance(azimuth_deg) and k(lat) = north_derating(lat).
///
/// East/west panels (θ = 90°) retain ~80–91% of south-facing production
/// (at 48°N and 25°N respectively). North-facing panels degrade from
/// 65% (25°N) to 30% (48°N) of south-facing.
///
/// NREL/TP-6A20-62641 §4 (Dobos 2014) Table 4.
/// The quadratic shape captures the empirical observation that azimuth
/// derating accelerates more sharply approaching north than approaching
/// east/west — direct beam reaches east/west panels at favourable
/// incidence angles during morning/afternoon hours while north-facing
/// panels receive diffuse-only radiation.
fn azimuth_production_factor(azimuth_deg: f64, latitude: f64) -> f64 {
    let theta_deg = south_distance(azimuth_deg);
    let x = theta_deg / 180.0;
    let k = north_derating(latitude);
    (1.0 - k * x * x).max(0.0)
}

/// Solar production score for a roof plane, weighting area by usable
/// fraction and azimuth production factor.
///
/// score = area × usable_fraction(shape) × production_factor(az, lat, Kd)
///
/// When `diffuse_fraction` is `Some(Kd)` computed from weather data, the
/// production factor models the beam/diffuse split explicitly:
///
///   factor = Kd + (1 − Kd) · cos(θ)^e
///
/// where θ is the angular distance from due south clamped to [0, π/2] and
/// the exponent e = 1.0 + 0.005·(latitude − 35°) captures the latitude
/// dependence of direct-beam incidence geometry.
///
/// When `diffuse_fraction` is `None`, falls back to the
/// [`azimuth_production_factor`] quadratic model calibrated to NREL PVWatts
/// Table 4 (Dobos 2014) orientation factors.
///
/// This is a relative ranking heuristic, not an absolute production model.
/// Expected errors of ±15–20% relative to TMY3-based transposition simulation
/// for non-south arrays (Perez 1990 anisotropic sky model with weather data).
fn plane_solar_score(
    area_m2: f64,
    azimuth_deg: f64,
    shape: RoofShape,
    latitude: f64,
    diffuse_fraction: Option<f64>,
) -> f64 {
    let production_factor = match diffuse_fraction {
        Some(df) => {
            let deviation_rad =
                (south_distance(azimuth_deg).to_radians()).min(std::f64::consts::PI / 2.0);
            let exponent = 1.0 + 0.005 * (latitude - 35.0);
            df + (1.0 - df) * deviation_rad.cos().powf(exponent)
        }
        None => azimuth_production_factor(azimuth_deg, latitude),
    };
    area_m2 * usable_fraction(shape) * production_factor
}

/// Minimum design-point solar elevation (degrees) for GCR computation.
///
/// Continuous piecewise-linear function of latitude that replaces the old
/// step-table bands. The design-point elevation is the lowest sun angle at
/// which no inter-row shading is acceptable; it decreases with latitude
/// because the winter sun path is lower in the sky.
///
/// Calibration points derived from the old GCR band values for the nominal
/// 25° tilt cap via the Appelbaum self-shading formula:
///   - lat 25°N → min elevation ~21° (old GCR=0.50 at 25° tilt)
///   - lat 35°N → min elevation ~15° (old GCR=0.40 at 25° tilt)
///   - lat 50°N → min elevation ~12° (old GCR=0.35 at 25° tilt)
///
/// Floors at 12° — at 50°N a 12° design solar elevation provides roughly
/// 6-hour winter-solstice operation, balancing self-shading avoidance
/// with practical packing density.
///
/// Appelbaum & Bany (1979) Solar Energy 23(6):497-500 —
/// shadow geometry basis for the GCR formula.
fn min_solar_elevation_deg(lat: f64) -> f64 {
    match lat {
        _ if lat <= 25.0 => 21.0,
        _ if lat <= 35.0 => 21.0 - (lat - 25.0) * 0.6, // 21 → 15
        _ if lat <= 50.0 => 15.0 - (lat - 35.0) * 0.2, // 15 → 12
        _ => 12.0,
    }
}

/// Ground coverage ratio for flat roofs, derived from tilt and latitude.
///
/// Computes GCR from the Appelbaum & Bany (1979) geometric self-shading
/// constraint, parameterized by the actual installed tilt and a
/// latitude-dependent design-point solar elevation:
///
///   GCR = 1 / (cos(β) + sin(β) / tan(α_min))
///
/// where β = tilt and α_min = min_solar_elevation_deg(latitude).
/// Clamped to [0.25, 0.65]:
///   - 0.25 lower bound: minimum economically viable GCR for fixed-tilt flat-roof
///     PV. Below this, each panel requires >4× its own area in roof space and a
///     ground-mount array becomes the preferred alternative. Cf. Appelbaum & Bany
///     (1979) for the geometric formula; EnergyPlus PVWatts IDD V26-1-0
///     Generator:PVWatts N5 defaults GCR=0.4 with valid range [0, 1]. The bound
///     is a HARES engineering guard against uneconomically sparse layouts.
///   - 0.65 upper bound: maximum achievable GCR for a fixed-tilt flat-roof array
///     before year-round morning/afternoon row-to-row self-shading becomes
///     unavoidable. The Appelbaum formula assumes shading-free operation above
///     α_min at a single design-point azimuth (south) and does not account for
///     diffuse shading or ground-reflected component loss — both become
///     significant at close row packing. A full shading-integration model is
///     needed to reliably estimate performance above 0.65.
///   - Neither bound is expected to activate during normal operation with the
///     25° flat-roof tilt cap and piecewise-linear min_solar_elevation_deg; the
///     clamp is a guard against out-of-range parameterization.
///
/// Appelbaum & Bany (1979) Solar Energy 23(6):497-500
fn flat_roof_gcr(latitude: Option<f64>, tilt_deg: f64) -> f64 {
    let lat = latitude.unwrap_or(35.0);
    let min_elev_rad = min_solar_elevation_deg(lat).to_radians();
    let tilt_rad = tilt_deg.to_radians();
    let gcr = 1.0 / (tilt_rad.cos() + tilt_rad.sin() / min_elev_rad.tan());
    gcr.clamp(0.25, 0.65)
}

/// Resolve the azimuth for a roof plane, falling back to the most-southerly
/// wall azimuth, then to 180° (due south).
fn resolve_azimuth(plane: &RoofPlane, wall_azimuths: &[f64]) -> f64 {
    if let Some(az) = plane.azimuth_deg {
        return az;
    }
    if !wall_azimuths.is_empty() {
        // Prefer the wall azimuth closest to south; break ties west-of-south.
        return *wall_azimuths
            .iter()
            .min_by(|a, b| {
                let da = south_distance(**a);
                let db = south_distance(**b);
                da.partial_cmp(&db)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| {
                        // Prefer west-of-south (az > 180) over east-of-south.
                        let wa = if **a > 180.0 { 0 } else { 1 };
                        let wb = if **b > 180.0 { 0 } else { 1 };
                        wa.cmp(&wb)
                    })
            })
            .unwrap();
    }
    180.0
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Compute the usable roof area and maximum PV capacity for a building.
///
/// `wall_azimuths` provides fallback orientation when roof planes lack an
/// explicit azimuth. `latitude` enables latitude-dependent scoring and
/// flat-roof tilt selection. `diffuse_fraction` is the annual-average
/// DHI/GHI ratio from weather data; when `None` the scoring falls back to
/// the NREL PVWatts Table 4 empirical model.
// Why: the parameter count reflects the complete set of tunable PV sizing
// inputs; constructing a builder/params type would add indirection for no
// benefit at this call site.
#[allow(clippy::too_many_arguments)]
pub fn compute_usable_area(
    roof: &RoofInfo,
    roof_shape: RoofShape,
    wall_azimuths: &[f64],
    latitude: Option<f64>,
    panel_watts: Option<u32>,
    panel_area_m2: Option<f64>,
    diffuse_fraction: Option<f64>,
    roof_shape_user_override: bool,
) -> Result<UsableRoofArea, PvSizingError> {
    let panel_watts = panel_watts.unwrap_or(DEFAULT_PANEL_WATTS);
    let panel_area_m2 = panel_area_m2.unwrap_or(DEFAULT_PANEL_AREA_M2);

    if roof_shape_user_override {
        tracing::debug!(
            pv_roof_shape_override = ?roof_shape,
            "PV roof shape is user-specified override, inference skipped"
        );
    }

    // Invariant: panel physical parameters must be in valid ranges.
    // No residential panel exceeds ~3.5 m²; the 5.0 m² upper bound allows
    // for future large-format utility panels without being physically
    // impossible.
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    {
        assert!(
            panel_watts > 0,
            "panel_watts must be positive, got {panel_watts}"
        );
        assert!(
            panel_area_m2 > 0.0,
            "panel_area_m2 must be positive, got {panel_area_m2}"
        );
        assert!(
            panel_area_m2 < 5.0,
            "panel_area_m2 must be < 5.0 m² (no residential panel exceeds ~3.5 m²), got {panel_area_m2}"
        );
    }

    if roof.planes.is_empty() {
        return Err(PvSizingError::NoRoofPlanes);
    }

    // FlatEastWest: return before the north-facing candidate filter.
    // East-west dual-tilt systems fill the entire flat roof regardless of
    // individual plane azimuths — the per-plane azimuth resolution and
    // north-facing check are inapplicable. See T-1928 Known Limitations.
    if roof_shape == RoofShape::FlatEastWest {
        let effective_area = roof.total_roof_area_m2;
        let usable_m2 = effective_area * FLAT_USABLE_FRACTION;
        let tilt_deg = EAST_WEST_TILT_DEG;
        let panel_footprint = panel_area_m2 / EAST_WEST_GCR;

        let total_panels = (usable_m2 / panel_footprint).floor() as u32;
        let per_orientation_panels = total_panels / 2;
        let paired_panels = per_orientation_panels * 2;
        let max_capacity_kw = (paired_panels as f64) * (panel_watts as f64) / 1000.0;

        // best_plane_idx uses the largest-area plane for boundary reference.
        // This matches enumerate_pv_candidates which selects the same plane
        // for the FlatEastWest candidate boundary_index, ensuring consistency
        // between the two functions. The "best plane" is not the solar-optimised
        // choice here — orientation is ignored for east-west — but the convention
        // provides a stable reference for callers that consume best_plane_idx.
        let best_idx = roof
            .planes
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| {
                a.area_m2
                    .partial_cmp(&b.area_m2)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(i, _)| i)
            .unwrap_or(0);

        #[cfg(feature = "observe")]
        {
            tracing::debug!(
                pv_roof_shape = ?roof_shape,
                pv_east_west_gcr = EAST_WEST_GCR,
                pv_east_west_tilt_deg = tilt_deg,
                pv_effective_area_m2 = effective_area,
                pv_usable_m2 = usable_m2,
                pv_panel_footprint_m2 = panel_footprint,
                pv_total_panels = total_panels,
                pv_per_orientation_panels = per_orientation_panels,
                pv_paired_panels = paired_panels,
                pv_max_capacity_kw = max_capacity_kw,
                pv_roof_shape_user_override = roof_shape_user_override,
                "PV East-West dual-tilt flat-roof sizing"
            );
        }

        return Ok(UsableRoofArea {
            best_plane_idx: best_idx,
            usable_m2,
            max_panels: paired_panels,
            max_capacity_kw,
            roof_shape,
            // Primary azimuth is east (morning production slightly exceeds
            // afternoon due to cooler ambient temperatures — Lave & Kleissl 2010).
            azimuth_deg: 90.0,
            tilt_deg,
        });
    }

    // Filter to non-north-facing candidates with resolved azimuths.
    let candidates: Vec<(usize, f64)> = roof
        .planes
        .iter()
        .enumerate()
        .filter_map(|(i, plane)| {
            let az = resolve_azimuth(plane, wall_azimuths);
            if is_north_facing(az) {
                None
            } else {
                Some((i, az))
            }
        })
        .collect();

    if candidates.is_empty() {
        return Err(PvSizingError::AllNorthFacing);
    }

    let lat = latitude.unwrap_or(35.0);

    // Invariant: computed diffuse fraction must be physically valid.
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    if let Some(kd) = diffuse_fraction {
        assert!(
            (0.0..=1.0).contains(&kd),
            "computed diffuse fraction Kd={:.4} out of range [0, 1]",
            kd
        );
        if !(0.10..=0.30).contains(&kd) {
            tracing::warn!(
                pv_diffuse_fraction = kd,
                pv_latitude = lat,
                "computed diffuse fraction Kd={:.4} outside expected continental-US range [0.10, 0.30]",
                kd
            );
        }
    }

    // Diagnostic: log the computed diffuse fraction when available.
    if let Some(kd) = diffuse_fraction {
        tracing::info!(
            pv_diffuse_fraction = kd,
            pv_latitude = lat,
            "PV sizing using location-specific diffuse fraction Kd={:.4}",
            kd
        );
    }

    // Invariant: East-facing panels must not be worse than West-facing.
    // Physical basis: afternoon ambient temperatures are higher than morning
    // temperatures, reducing PV efficiency via negative temperature coefficient
    // (typically -0.3% to -0.5%/°C). Lave & Kleissl (2010) find west-facing
    // panels produce 1–3% less than east-facing annually at most US locations.
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    {
        let east_factor = azimuth_production_factor(90.0, lat);
        let west_factor = azimuth_production_factor(270.0, lat);
        assert!(
            east_factor >= west_factor,
            "East production factor ({:.4}) must be >= West ({:.4}) — afternoon heat penalty",
            east_factor,
            west_factor
        );
    }

    // Invariant: East/West factor must stay in physical range.
    // The old model collapsed to DIFFUSE_FRAC (0.18) for east/west;
    // a latitude-aware model must stay above 0.50 (diffuse + morning/afternoon
    // direct beam) and below 0.95 (always less than south-facing).
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    {
        let east_factor = azimuth_production_factor(90.0, lat);
        assert!(
            east_factor > 0.5,
            "East/West production factor ({:.4}) too low — must exceed 0.5 for lat={}",
            east_factor,
            lat
        );
        assert!(
            east_factor < 0.95,
            "East/West production factor ({:.4}) too close to south — must be < 0.95 for lat={}",
            east_factor,
            lat
        );
    }

    // Invariant: flat-roof GCR monotonically decreases with increasing tilt
    // (fixed latitude) and with increasing latitude (fixed tilt).
    // Appelbaum & Bany (1979) Solar Energy 23(6):497-500.
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    {
        for test_lat in [25.0, 35.0, 50.0] {
            for pair in [(5.0, 25.0), (5.0, 45.0), (25.0, 45.0)] {
                let gcr_lo = flat_roof_gcr(Some(test_lat), pair.0);
                let gcr_hi = flat_roof_gcr(Some(test_lat), pair.1);
                assert!(
                    gcr_lo >= gcr_hi,
                    "GCR must not increase with tilt: lat={test_lat} tilt={},{} → GCR={gcr_lo:.4},{gcr_hi:.4}",
                    pair.0,
                    pair.1,
                );
            }
        }
        for test_tilt in [5.0, 25.0, 45.0] {
            let gcr_lo = flat_roof_gcr(Some(25.0), test_tilt);
            let gcr_hi = flat_roof_gcr(Some(50.0), test_tilt);
            assert!(
                gcr_lo >= gcr_hi,
                "GCR must not increase with latitude: tilt={test_tilt} lat 25→50 gives {gcr_lo:.4},{gcr_hi:.4}",
            );
        }
    }

    // Invariant: GCR × usable_fraction must be in [0.15, 0.55] for flat roofs.
    // Values outside indicate parameterization error.
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    if roof_shape == RoofShape::Flat {
        for test_lat in [10.0, 25.0, 35.0, 50.0] {
            for test_tilt in [5.0, 15.0, 25.0, 45.0] {
                let gcr = flat_roof_gcr(Some(test_lat), test_tilt);
                let effective = gcr * FLAT_USABLE_FRACTION;
                assert!(
                    (0.15..=0.55).contains(&effective),
                    "flat roof GCR×usable_fraction={effective:.4} out of [0.15, 0.55] range at lat={test_lat} tilt={test_tilt}"
                );
            }
        }
    }

    // Select the best plane.
    let (best_idx, best_az) = if roof_shape == RoofShape::Hip {
        // Hip: pick the most-southerly plane (smallest south_distance), breaking
        // ties by largest area then west-of-south preference.
        *candidates
            .iter()
            .min_by(|(ia, az_a), (ib, az_b)| {
                let da = south_distance(*az_a);
                let db = south_distance(*az_b);
                da.partial_cmp(&db)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| {
                        let area_a = roof.planes[*ia].area_m2;
                        let area_b = roof.planes[*ib].area_m2;
                        area_b
                            .partial_cmp(&area_a)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .then_with(|| {
                        let wa = if *az_a > 180.0 { 0 } else { 1 };
                        let wb = if *az_b > 180.0 { 0 } else { 1 };
                        wa.cmp(&wb)
                    })
            })
            .unwrap()
    } else {
        // Gable/Flat: pick the plane with the highest solar score.
        *candidates
            .iter()
            .max_by(|(ia, az_a), (ib, az_b)| {
                let sa = plane_solar_score(
                    roof.planes[*ia].area_m2,
                    *az_a,
                    roof_shape,
                    lat,
                    diffuse_fraction,
                );
                let sb = plane_solar_score(
                    roof.planes[*ib].area_m2,
                    *az_b,
                    roof_shape,
                    lat,
                    diffuse_fraction,
                );
                sa.partial_cmp(&sb).unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap()
    };

    let best_plane = &roof.planes[best_idx];

    if roof_shape == RoofShape::Hip {
        // Aggregate panel capacity across all viable planes, weighted by
        // latitude-dependent azimuth production factor.
        let mut total_weighted_panels: u32 = 0;
        #[cfg(feature = "observe")]
        let mut hip_prod_factor_sum: f64 = 0.0;

        for &(idx, az) in &candidates {
            let plane = &roof.planes[idx];
            let plane_usable = plane.area_m2 * usable_fraction(RoofShape::Hip);
            let plane_panels = (plane_usable / panel_area_m2).floor() as u32;
            let prod_factor = azimuth_production_factor(az, lat);
            total_weighted_panels += ((plane_panels as f64) * prod_factor).floor() as u32;

            #[cfg(feature = "observe")]
            {
                hip_prod_factor_sum += prod_factor;
                tracing::debug!(
                    pv_hip_plane_azimuth = az,
                    pv_hip_plane_prod_factor = prod_factor,
                    pv_hip_plane_panels = plane_panels,
                    pv_hip_plane_weighted = ((plane_panels as f64) * prod_factor).floor() as u32,
                    "PV Hip aggregation per-plane telemetry"
                );
            }
        }

        // Invariant: north-facing production factor decreases with latitude.
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            let low_lat_north = azimuth_production_factor(0.0, 25.0);
            let high_lat_north = azimuth_production_factor(0.0, 48.0);
            assert!(
                low_lat_north > high_lat_north,
                "north-facing production factor must decrease with latitude (25°N: {:.4}, 48°N: {:.4})",
                low_lat_north,
                high_lat_north
            );
        }

        #[cfg(feature = "observe")]
        {
            tracing::debug!(
                pv_hip_latitude = lat,
                pv_hip_prod_factor_sum = hip_prod_factor_sum,
                pv_hip_weighted_panels = total_weighted_panels,
                pv_panel_watts = panel_watts,
                pv_panel_area_m2 = panel_area_m2,
                pv_roof_shape_user_override = roof_shape_user_override,
                "PV Hip aggregation telemetry"
            );
        }

        let tilt_deg = best_plane.tilt_deg;
        let best_usable = best_plane.area_m2 * usable_fraction(RoofShape::Hip);

        return Ok(UsableRoofArea {
            best_plane_idx: best_idx,
            usable_m2: best_usable,
            max_panels: total_weighted_panels,
            max_capacity_kw: (total_weighted_panels as f64) * (panel_watts as f64) / 1000.0,
            roof_shape,
            azimuth_deg: best_az,
            tilt_deg,
        });
    }

    // FlatEastWest handled by early return above (before north-facing filter).
    // The path below is Gable / Flat only.

    // Gable / Flat path.
    let area_m2 = best_plane.area_m2;

    // Gable with a single plane: area is whole-roof footprint, halve for one face.
    let effective_area = if roof_shape == RoofShape::Gable && roof.planes.len() == 1 {
        area_m2 / 2.0
    } else {
        area_m2
    };

    let usable_m2 = effective_area * usable_fraction(roof_shape);

    // Compute tilt before panel_footprint so flat_roof_gcr can use it.
    let tilt_deg = if roof_shape == RoofShape::Flat || best_plane.tilt_deg < 1.0 {
        latitude
            .map(|l| l.min(25.0))
            .unwrap_or(FLAT_TILT_FALLBACK_DEG)
    } else {
        best_plane.tilt_deg
    };

    let panel_footprint = if roof_shape == RoofShape::Flat {
        panel_area_m2 / flat_roof_gcr(latitude, tilt_deg)
    } else {
        panel_area_m2
    };

    #[cfg(feature = "observe")]
    if roof_shape == RoofShape::Flat {
        let gcr = flat_roof_gcr(latitude, tilt_deg);
        let lat_val = latitude.unwrap_or(35.0);
        let min_elev = min_solar_elevation_deg(lat_val);
        tracing::debug!(
            pv_latitude = lat_val,
            pv_tilt_deg = tilt_deg,
            pv_gcr = gcr,
            pv_min_solar_elevation_deg = min_elev,
            pv_panel_footprint_m2 = panel_footprint,
            pv_effective_area_m2 = effective_area,
            "Flat-roof GCR computed from tilt and latitude"
        );
    }

    let max_panels = (usable_m2 / panel_footprint).floor() as u32;
    let max_capacity_kw = (max_panels as f64) * (panel_watts as f64) / 1000.0;

    #[cfg(feature = "observe")]
    {
        let prod_factor = azimuth_production_factor(best_az, lat);
        tracing::debug!(
            pv_latitude = lat,
            pv_azimuth = best_az,
            pv_production_factor = prod_factor,
            pv_roof_shape = ?roof_shape,
            pv_max_panels = max_panels,
            pv_panel_watts = panel_watts,
            pv_panel_area_m2 = panel_area_m2,
            pv_roof_shape_user_override = roof_shape_user_override,
            "PV Gable/Flat plane selected"
        );
    }

    Ok(UsableRoofArea {
        best_plane_idx: best_idx,
        usable_m2,
        max_panels,
        max_capacity_kw,
        roof_shape,
        azimuth_deg: best_az,
        tilt_deg,
    })
}

/// Size a PV system to a target capacity, clamped by roof constraints.
pub fn size_pv_system(
    usable: &UsableRoofArea,
    target_kw: f64,
    min_kw: f64,
    max_kw: f64,
    system_losses: Option<f64>,
    panel_watts: Option<u32>,
    panel_area_m2: Option<f64>,
) -> Result<PvSizingResult, PvSizingError> {
    let panel_watts = panel_watts.unwrap_or(DEFAULT_PANEL_WATTS);
    let panel_area_m2 = panel_area_m2.unwrap_or(DEFAULT_PANEL_AREA_M2);
    let system_losses = system_losses.unwrap_or(DEFAULT_SYSTEM_LOSSES);

    // Invariant: panel physical parameters must be in valid ranges.
    // No residential panel exceeds ~3.5 m²; the 5.0 m² upper bound allows
    // for future large-format panels without being physically impossible.
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    {
        assert!(
            panel_watts > 0,
            "panel_watts must be positive, got {panel_watts}"
        );
        assert!(
            panel_area_m2 > 0.0,
            "panel_area_m2 must be positive, got {panel_area_m2}"
        );
        assert!(
            panel_area_m2 < 5.0,
            "panel_area_m2 must be < 5.0 m² (no residential panel exceeds ~3.5 m²), got {panel_area_m2}"
        );
        assert!(
            (0.0..=0.99).contains(&system_losses),
            "system_losses must be in [0, 0.99], got {system_losses}"
        );
    }

    if usable.max_capacity_kw < min_kw {
        return Err(PvSizingError::InsufficientRoof {
            available_kw: usable.max_capacity_kw,
            min_kw,
        });
    }

    let upper_bound = max_kw.min(usable.max_capacity_kw);
    let clamped_kw = target_kw.clamp(min_kw, upper_bound);
    let num_panels =
        ((clamped_kw * 1000.0 / panel_watts as f64).ceil() as u32).min(usable.max_panels);
    let capacity_kw = (num_panels as f64) * (panel_watts as f64) / 1000.0;
    let collector_area_m2 = (num_panels as f64) * panel_area_m2;

    #[cfg(feature = "observe")]
    {
        tracing::debug!(
            pv_panel_watts = panel_watts,
            pv_panel_area_m2 = panel_area_m2,
            pv_system_losses_fraction = system_losses,
            pv_num_panels = num_panels,
            pv_capacity_kw = capacity_kw,
            pv_collector_area_m2 = collector_area_m2,
            "PV system sizing result"
        );
    }

    Ok(PvSizingResult {
        capacity_kw,
        num_panels,
        collector_area_m2,
        array_azimuth_deg: usable.azimuth_deg,
        array_tilt_deg: usable.tilt_deg,
        system_losses_fraction: system_losses,
        max_roof_capacity_kw: usable.max_capacity_kw,
        panel_watts,
    })
}

/// Enumerate all viable PV candidate placements, one per non-north-facing
/// roof plane. Results are sorted by solar score (best first).
///
/// This lets users see all placement options rather than just the single best.
// Why: the parameter count reflects the complete set of tunable PV sizing
// inputs; constructing a builder/params type would add indirection for no
// benefit at this call site.
#[allow(clippy::too_many_arguments)]
pub fn enumerate_pv_candidates(
    roof: &RoofInfo,
    roof_shape: RoofShape,
    wall_azimuths: &[f64],
    latitude: Option<f64>,
    panel_watts: Option<u32>,
    panel_area_m2: Option<f64>,
    diffuse_fraction: Option<f64>,
    roof_shape_user_override: bool,
) -> Vec<PvCandidate> {
    let panel_watts = panel_watts.unwrap_or(DEFAULT_PANEL_WATTS);
    let panel_area_m2 = panel_area_m2.unwrap_or(DEFAULT_PANEL_AREA_M2);
    let lat = latitude.unwrap_or(35.0);

    if roof_shape_user_override {
        tracing::debug!(
            pv_roof_shape_override = ?roof_shape,
            "PV roof shape is user-specified override in candidate enumeration, inference skipped"
        );
    }

    // FlatEastWest: generate two candidates (east + west) instead of iterating
    // over individual roof planes. Each orientation gets half the total capacity.
    if roof_shape == RoofShape::FlatEastWest {
        let effective_area = roof.total_roof_area_m2;
        let usable_m2 = effective_area * FLAT_USABLE_FRACTION;
        let tilt_deg = EAST_WEST_TILT_DEG;
        let panel_footprint = panel_area_m2 / EAST_WEST_GCR;

        let total_panels = (usable_m2 / panel_footprint).floor() as u32;
        let per_orientation_panels = total_panels / 2;
        let per_orientation_capacity_kw =
            (per_orientation_panels as f64) * (panel_watts as f64) / 1000.0;

        // Use the largest plane's index for both candidates.
        let best_idx = roof
            .planes
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| {
                a.area_m2
                    .partial_cmp(&b.area_m2)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(i, _)| i)
            .unwrap_or(0);
        let boundary_index = roof.planes.get(best_idx).and_then(|p| p.boundary_index);

        let east_score = plane_solar_score(
            effective_area / 2.0,
            90.0,
            RoofShape::Flat,
            lat,
            diffuse_fraction,
        );
        let west_score = plane_solar_score(
            effective_area / 2.0,
            270.0,
            RoofShape::Flat,
            lat,
            diffuse_fraction,
        );

        let mut candidates = vec![
            PvCandidate {
                plane_idx: best_idx,
                azimuth_deg: 90.0,
                tilt_deg,
                usable_m2: usable_m2 / 2.0,
                max_panels: per_orientation_panels,
                max_capacity_kw: per_orientation_capacity_kw,
                solar_score: east_score,
                roof_shape,
                boundary_index,
            },
            PvCandidate {
                plane_idx: best_idx,
                azimuth_deg: 270.0,
                tilt_deg,
                usable_m2: usable_m2 / 2.0,
                max_panels: per_orientation_panels,
                max_capacity_kw: per_orientation_capacity_kw,
                solar_score: west_score,
                roof_shape,
                boundary_index,
            },
        ];

        candidates.sort_by(|a, b| {
            b.solar_score
                .partial_cmp(&a.solar_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        #[cfg(feature = "observe")]
        {
            tracing::debug!(
                pv_roof_shape = ?roof_shape,
                pv_east_west_candidate_count = candidates.len(),
                pv_east_west_per_orientation_panels = per_orientation_panels,
                pv_roof_shape_user_override = roof_shape_user_override,
                "PV East-West dual-tilt candidate enumeration"
            );
        }

        return candidates;
    }

    let mut candidates: Vec<PvCandidate> = roof
        .planes
        .iter()
        .enumerate()
        .filter_map(|(i, plane)| {
            let az = resolve_azimuth(plane, wall_azimuths);
            if is_north_facing(az) {
                return None;
            }

            let area = plane.area_m2;
            // For single-plane gable, halve.
            let effective_area = if roof_shape == RoofShape::Gable && roof.planes.len() == 1 {
                area / 2.0
            } else {
                area
            };

            let usable_m2 = effective_area * usable_fraction(roof_shape);

            let tilt_deg = if roof_shape == RoofShape::Flat || plane.tilt_deg < 1.0 {
                latitude
                    .map(|l| l.min(25.0))
                    .unwrap_or(FLAT_TILT_FALLBACK_DEG)
            } else {
                plane.tilt_deg
            };

            let panel_footprint = if roof_shape == RoofShape::Flat {
                panel_area_m2 / flat_roof_gcr(latitude, tilt_deg)
            } else {
                panel_area_m2
            };
            let max_panels = (usable_m2 / panel_footprint).floor() as u32;
            let max_capacity_kw = (max_panels as f64) * (panel_watts as f64) / 1000.0;

            let solar_score =
                plane_solar_score(effective_area, az, roof_shape, lat, diffuse_fraction);

            Some(PvCandidate {
                plane_idx: i,
                azimuth_deg: az,
                tilt_deg,
                usable_m2,
                max_panels,
                max_capacity_kw,
                solar_score,
                roof_shape,
                boundary_index: plane.boundary_index,
            })
        })
        .collect();

    // Sort by solar score descending (best first).
    candidates.sort_by(|a, b| {
        b.solar_score
            .partial_cmp(&a.solar_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    #[cfg(feature = "observe")]
    {
        tracing::debug!(
            pv_latitude = lat,
            pv_roof_shape = ?roof_shape,
            pv_candidate_count = candidates.len(),
            pv_roof_shape_user_override = roof_shape_user_override,
            "PV candidate enumeration"
        );
        for c in &candidates {
            let prod_factor = azimuth_production_factor(c.azimuth_deg, lat);
            tracing::debug!(
                pv_candidate_azimuth = c.azimuth_deg,
                pv_candidate_production_factor = prod_factor,
                pv_candidate_max_panels = c.max_panels,
                "PV candidate detail"
            );
        }
    }

    candidates
}

/// Errors from PV sizing.
#[derive(Debug, Clone, thiserror::Error)]
pub enum PvSizingError {
    #[error("no roof planes available")]
    NoRoofPlanes,
    #[error("all roof planes are north-facing or missing azimuth data")]
    AllNorthFacing,
    #[error("roof capacity {available_kw:.1} kW below minimum {min_kw} kW")]
    InsufficientRoof { available_kw: f64, min_kw: f64 },
}

// ---------------------------------------------------------------------------
// Roof shape inference
// ---------------------------------------------------------------------------

/// Infer roof shape from building metadata.
///
/// `facility_type` is the HPXML `ResidentialFacilityType` string.
/// Falls back to `Hip` when evidence is ambiguous (the most conservative
/// shape — usable fraction 0.35 — to avoid overestimating PV capacity
/// from HPXML datasets that omit minor roof planes).
pub fn infer_roof_shape(
    roof: &RoofInfo,
    facility_type: Option<&str>,
    latitude: Option<f64>,
) -> RoofShape {
    // Apartments / multifamily 5+ → Flat.
    if let Some(ft) = facility_type {
        let ft_lower = ft.to_lowercase();
        if ft_lower.contains("apartment") || ft_lower.contains("5+") {
            return RoofShape::Flat;
        }
    }

    // All planes have tilt ≈ 0 → Flat.
    if !roof.planes.is_empty() && roof.planes.iter().all(|p| p.tilt_deg < 1.0) {
        return RoofShape::Flat;
    }

    // Multiple planes with distinct azimuths → Hip.
    let distinct_azimuths: std::collections::HashSet<u32> = roof
        .planes
        .iter()
        .filter_map(|p| p.azimuth_deg.map(|a| (a / 45.0).round() as u32))
        .collect();
    if distinct_azimuths.len() >= 3 {
        return RoofShape::Hip;
    }

    // tile/slate is a probabilistic, not deterministic, roof-shape signal.
    // Classification must not use material type as a decision rule; the
    // association (common on hip roofs in Mediterranean / Spanish Colonial
    // styles) is a correlation, not a structural guarantee. Tile-roofed gable
    // homes are common in the Southwestern US. The tile/slate flag is retained
    // for the fallback observer — it records the material hint alongside the
    // selected shape for post-hoc analysis of inference quality.
    let has_tile_slate = roof.planes.iter().any(|p| {
        p.material.as_ref().is_some_and(|m| {
            let ml = m.to_lowercase();
            ml.contains("tile") || ml.contains("slate")
        })
    });

    // Low-latitude regions with 2 planes -- mild hip signal.
    if distinct_azimuths.len() >= 2 && latitude.is_some_and(|l| l < 30.0) {
        return RoofShape::Hip;
    }

    // ---- multi-signal Gable classification ----
    // Gable roofs typically have two opposing planes (azimuth ≈180° apart).
    // Classification requires both geometric evidence (opposing planes, pitched
    // tilt) and supporting context (no tile/slate, latitude ≥35°) to avoid
    // misclassifying 2-plane hip roofs with omitted minor planes.
    //
    // The model uses a weighted gate: opposing planes are necessary but not
    // sufficient — supporting evidence (no tile material, higher latitude,
    // pitched tilt) must accumulate to a confidence score ≥2.
    //
    // Latitude gap design: the low-latitude Hip gate fires at <30°N; the
    // latitude Gable signal only fires at ≥35°N. The 30°–35°N band is
    // intentionally neither a hard Hip zone nor a +1-signal zone. In this
    // band, 2-plane opposing no-tile roofs with tilt ≥10° score 2
    // (no-tile +1, tilt +1) and classify as Gable — sufficient evidence even
    // without the latitude signal. Tile or shallow-tilt roofs in this band
    // score ≤1 and fall through to the conservative Hip fallback below.
    let plane_azimuths: Vec<f64> = roof.planes.iter().filter_map(|p| p.azimuth_deg).collect();
    if plane_azimuths.len() == 2 && roof.planes.len() == 2 {
        let (a1, a2) = (plane_azimuths[0], plane_azimuths[1]);
        let diff = (a1 - a2).abs();
        let short_diff = diff.min(360.0 - diff);
        let is_opposite = (180.0 - short_diff).abs() <= 30.0;

        if is_opposite {
            let mut gable_score: u32 = 0;
            // Absence of tile/slate strengthens Gable confidence — tile/slate is
            // probabilistically correlated with hip roofs (Mediterranean /
            // Spanish Colonial styles) but not deterministic.
            if !has_tile_slate {
                gable_score += 1;
            }
            // At latitudes ≥35°N, gable roofs dominate — hurricane-wind-driven
            // hip adoption concentrates <35°N in the southeastern US.
            // IBHS (2019) "Rating the States: An Assessment of Residential
            // Building Code and Enforcement Systems" documents regional
            // hip roof adoption rates for wind resistance.
            if latitude.is_some_and(|l| l >= 35.0) {
                gable_score += 1;
            }
            // Pitched tilt (≥10°) confirms sloped roof — flat roofs are already
            // classified above. Very shallow tilt (<10°) could indicate a low-slope
            // hip or an artifact, so exclude from Gable.
            if roof.planes.iter().all(|p| p.tilt_deg >= 10.0) {
                gable_score += 1;
            }

            if gable_score >= 2 {
                return RoofShape::Gable;
            }
        }
    }
    // ---- end Gable classification ----

    // Fallback: no classification rule matched. Default to Hip — the most
    // conservative shape — to avoid overestimating PV capacity when HPXML
    // datasets omit minor roof planes (common in NREL ResStock v3.1).
    // Gable (0.75 usable) would overstate usable area by 2.14× vs Hip (0.35).
    let n_planes = roof.planes.len();
    let n_distinct_az = distinct_azimuths.len();

    #[cfg(feature = "observe")]
    {
        tracing::debug!(
            pv_roof_shape_fallback_reason = "no_classification_rule_matched",
            pv_n_planes = n_planes,
            pv_n_distinct_azimuths = n_distinct_az,
            pv_has_tile_slate = has_tile_slate,
            pv_latitude = latitude,
            pv_facility_type = facility_type.map(|s| s.to_string()),
            "roof_shape fallback: defaulting to Hip (conservative)\
             — n_planes={n_planes}, n_distinct_az={n_distinct_az},\
             tile_slate={has_tile_slate}",
        );
    }

    tracing::warn!(
        pv_n_planes = n_planes,
        pv_n_distinct_azimuths = n_distinct_az,
        pv_has_tile_slate = has_tile_slate,
        "Roof shape could not be determined from available data; defaulting to Hip (conservative, 0.35 usable fraction)"
    );

    RoofShape::Hip
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plane(area_m2: f64, tilt_deg: f64, azimuth_deg: Option<f64>) -> RoofPlane {
        RoofPlane {
            area_m2,
            tilt_deg,
            azimuth_deg,
            material: None,
            boundary_index: None,
        }
    }

    #[test]
    fn south_distance_symmetric() {
        assert!((south_distance(180.0) - 0.0).abs() < 1e-10);
        assert!((south_distance(135.0) - 45.0).abs() < 1e-10);
        assert!((south_distance(225.0) - 45.0).abs() < 1e-10);
        assert!((south_distance(0.0) - 180.0).abs() < 1e-10);
    }

    #[test]
    fn north_facing_detection() {
        assert!(is_north_facing(0.0));
        assert!(is_north_facing(45.0));
        assert!(is_north_facing(315.0));
        assert!(is_north_facing(350.0));
        assert!(!is_north_facing(90.0));
        assert!(!is_north_facing(180.0));
        assert!(!is_north_facing(270.0));
    }

    #[test]
    fn single_south_gable() {
        let roof = RoofInfo {
            planes: vec![plane(100.0, 26.0, Some(180.0))],
            total_roof_area_m2: 100.0,
        };
        let usable = compute_usable_area(
            &roof,
            RoofShape::Gable,
            &[],
            Some(40.0),
            None,
            None,
            None,
            false,
        )
        .unwrap();
        // Single gable plane → halved, then ×0.75.
        assert!((usable.usable_m2 - 100.0 / 2.0 * 0.75).abs() < 0.01);
        assert!((usable.azimuth_deg - 180.0).abs() < 0.01);
        assert!((usable.tilt_deg - 26.0).abs() < 0.01);
    }

    #[test]
    fn two_plane_gable_picks_south() {
        let roof = RoofInfo {
            planes: vec![
                plane(50.0, 26.0, Some(180.0)), // south
                plane(50.0, 26.0, Some(0.0)),   // north (filtered)
            ],
            total_roof_area_m2: 100.0,
        };
        let usable = compute_usable_area(
            &roof,
            RoofShape::Gable,
            &[],
            Some(40.0),
            None,
            None,
            None,
            false,
        )
        .unwrap();
        // Two planes → no halving; south plane used directly.
        assert!((usable.usable_m2 - 50.0 * 0.75).abs() < 0.01);
    }

    #[test]
    fn all_north_facing_returns_error() {
        let roof = RoofInfo {
            planes: vec![plane(100.0, 26.0, Some(0.0))],
            total_roof_area_m2: 100.0,
        };
        let result =
            compute_usable_area(&roof, RoofShape::Gable, &[], None, None, None, None, false);
        assert!(result.is_err());
    }

    #[test]
    fn flat_roof_uses_gcr() {
        let roof = RoofInfo {
            planes: vec![plane(200.0, 0.0, Some(180.0))],
            total_roof_area_m2: 200.0,
        };
        let usable = compute_usable_area(
            &roof,
            RoofShape::Flat,
            &[],
            Some(35.0),
            None,
            None,
            None,
            false,
        )
        .unwrap();
        // Flat: effective = 200 × 0.70 = 140 m².
        // Panel footprint = 2.1 / 0.403 ≈ 5.21 m² (lat 35 → geometric GCR ≈ 0.403).
        // Max panels = floor(140 / 5.21) ≈ 26.
        assert_eq!(usable.max_panels, 26);
    }

    #[test]
    fn hip_aggregates_multiple_planes() {
        // Hip roof with south, ENE (60°), and north planes. North is filtered
        // by is_north_facing but south and ENE contribute at latitude-dependent
        // production factors.
        let roof = RoofInfo {
            planes: vec![
                plane(60.0, 26.0, Some(180.0)), // south
                plane(40.0, 26.0, Some(60.0)),  // ENE — not north-facing
                plane(40.0, 26.0, Some(0.0)),   // north (filtered)
            ],
            total_roof_area_m2: 140.0,
        };
        let usable = compute_usable_area(
            &roof,
            RoofShape::Hip,
            &[],
            Some(40.0),
            None,
            None,
            None,
            false,
        )
        .unwrap();
        // South: 60×0.35=21 m² → 10 panels, factor=1.0 → +10
        // ENE (θ=120°): 40×0.35=14 m² → 6 panels, k(40)=0.5154, x=0.667
        //   factor=1-0.5154×0.444=0.771, weighted=floor(6×0.771)=4
        // Total: 14
        assert_eq!(usable.max_panels, 14);
        assert!((usable.azimuth_deg - 180.0).abs() < 0.01);
    }

    #[test]
    fn hip_panel_count_decreases_with_latitude() {
        // Same Hip roof at 25°N vs 48°N. The ENE plane gets a higher
        // production factor at low latitudes, so the weighted panel count
        // should be higher at 25°N than at 48°N.
        let roof = RoofInfo {
            planes: vec![
                plane(60.0, 26.0, Some(180.0)), // south
                plane(40.0, 26.0, Some(60.0)),  // ENE
                plane(40.0, 26.0, Some(0.0)),   // north (filtered)
            ],
            total_roof_area_m2: 140.0,
        };
        let low_lat = compute_usable_area(
            &roof,
            RoofShape::Hip,
            &[],
            Some(25.0),
            None,
            None,
            None,
            false,
        )
        .unwrap();
        let high_lat = compute_usable_area(
            &roof,
            RoofShape::Hip,
            &[],
            Some(48.0),
            None,
            None,
            None,
            false,
        )
        .unwrap();
        assert!(
            low_lat.max_panels > high_lat.max_panels,
            "max_panels at 25°N ({}) should exceed max_panels at 48°N ({})",
            low_lat.max_panels,
            high_lat.max_panels
        );
    }

    #[test]
    fn north_facing_factor_matches_pvwatts_at_35n() {
        // At 35°N, a north-facing panel (azimuth 0°) should produce
        // approximately 60% of a south-facing panel per NREL/TP-6A20-62641
        // Table 4 (Dobos 2014).
        let factor = azimuth_production_factor(0.0, 35.0);
        // PVWatts Table 4: north ≈ 60% of south at 35°N tilt=latitude.
        // Tolerance ±5% absolute to accommodate interpolation.
        assert!((factor - 0.60).abs() < 0.05);
        // Also verify the old LUT value of 0.45 is no longer in use.
        assert!(factor > 0.55);
    }

    #[test]
    fn north_factor_decreases_monotonically_with_latitude() {
        // Invariant: north-facing production factor must decrease strictly
        // as latitude increases. At the equator, orientation matters little;
        // at high latitudes, north-facing panels produce almost nothing.
        let latitudes = [20.0, 25.0, 30.0, 35.0, 40.0, 45.0, 50.0];
        for window in latitudes.windows(2) {
            let lo = azimuth_production_factor(0.0, window[0]);
            let hi = azimuth_production_factor(0.0, window[1]);
            assert!(
                lo > hi,
                "north factor at {}° ({:.4}) should exceed at {}° ({:.4})",
                window[0],
                lo,
                window[1],
                hi
            );
        }
    }

    #[test]
    fn production_factor_identity_and_symmetry() {
        // South is always 1.0.
        assert!((azimuth_production_factor(180.0, 35.0) - 1.0).abs() < 1e-10);

        // East and West are equal (quadratic model is symmetric about south).
        let e = azimuth_production_factor(90.0, 35.0);
        let w = azimuth_production_factor(270.0, 35.0);
        assert!((e - w).abs() < 1e-10);

        // Monotonic: larger south-distance (farther from south) → lower factor.
        // South (θ=0°) → SE (θ=45°) → E (θ=90°) → NE (θ=135°) → N (θ=180°).
        for lat in [25.0, 35.0, 48.0] {
            let pairs = [(180.0, 135.0), (135.0, 90.0), (90.0, 45.0), (45.0, 0.0)];
            for (a1, a2) in &pairs {
                let f1 = azimuth_production_factor(*a1, lat);
                let f2 = azimuth_production_factor(*a2, lat);
                assert!(
                    f1 > f2,
                    "lat={lat} factor({a1})={f1:.4} should exceed factor({a2})={f2:.4}"
                );
            }
        }
    }

    #[test]
    fn azimuth_production_factor_covers_full_circle() {
        // Values should be in [0, 1] and symmetric about the south axis
        // for any latitude.
        for lat in [20.0, 35.0, 50.0] {
            for az in (0..360).step_by(10) {
                let f = azimuth_production_factor(az as f64, lat);
                assert!(f >= 0.0, "negative factor for az={az}, lat={lat}");
                assert!(f <= 1.0, "factor >1 for az={az}, lat={lat}");
            }
        }
    }

    #[test]
    fn east_not_worse_than_west_at_all_latitudes() {
        // East-facing panels must never be worse than West-facing panels.
        // Physical basis: afternoon ambient temperatures are higher than morning
        // temperatures, reducing PV efficiency (Lave & Kleissl 2010).
        for lat in [20.0, 25.0, 35.0, 48.0] {
            let e = azimuth_production_factor(90.0, lat);
            let w = azimuth_production_factor(270.0, lat);
            assert!(
                e >= w,
                "East ({:.4}) must be >= West ({:.4}) at lat={lat}",
                e,
                w
            );
        }
    }

    #[test]
    fn ne_not_worse_than_nw_at_all_latitudes() {
        // NE-facing (45°) panels must never be worse than NW-facing (315°)
        // panels. Same physical basis as east_vs_west.
        for lat in [20.0, 25.0, 35.0, 48.0] {
            let ne = azimuth_production_factor(45.0, lat);
            let nw = azimuth_production_factor(315.0, lat);
            assert!(
                ne >= nw,
                "NE ({:.4}) must be >= NW ({:.4}) at lat={lat}",
                ne,
                nw
            );
        }
    }

    #[test]
    fn azimuth_factors_at_reference_latitude_35n() {
        // At 35°N, the quadratic azimuth derating model gives:
        //   k(35°) = 0.40 (north_derating piecewise: 0.35 + 0.005×10)
        //   East/West (θ=90°, x=0.5): 1 - 0.40 × 0.25 = 0.90
        //   NE/NW   (θ=135°, x=0.75): 1 - 0.40 × 0.5625 = 0.775
        let lat = 35.0;
        assert!((azimuth_production_factor(90.0, lat) - 0.90).abs() < 1e-10);
        assert!((azimuth_production_factor(270.0, lat) - 0.90).abs() < 1e-10);
        assert!((azimuth_production_factor(45.0, lat) - 0.775).abs() < 1e-10);
        assert!((azimuth_production_factor(315.0, lat) - 0.775).abs() < 1e-10);
    }

    #[test]
    fn size_pv_clamps_to_roof() {
        let usable = UsableRoofArea {
            best_plane_idx: 0,
            usable_m2: 37.5,
            max_panels: 18,
            max_capacity_kw: 7.92,
            roof_shape: RoofShape::Gable,
            azimuth_deg: 180.0,
            tilt_deg: 26.0,
        };
        let result = size_pv_system(&usable, 10.0, 2.0, 14.0, None, None, None).unwrap();
        assert!(result.capacity_kw <= usable.max_capacity_kw + 0.01);
        assert!(result.num_panels <= usable.max_panels);
    }

    #[test]
    fn size_pv_errors_below_minimum() {
        let usable = UsableRoofArea {
            best_plane_idx: 0,
            usable_m2: 3.0,
            max_panels: 1,
            max_capacity_kw: 0.42,
            roof_shape: RoofShape::Gable,
            azimuth_deg: 180.0,
            tilt_deg: 26.0,
        };
        let result = size_pv_system(&usable, 6.0, 2.0, 14.0, None, None, None);
        assert!(result.is_err());
    }

    #[test]
    fn infer_flat_from_apartment() {
        let roof = RoofInfo {
            planes: vec![plane(100.0, 0.0, Some(180.0))],
            total_roof_area_m2: 100.0,
        };
        assert_eq!(
            infer_roof_shape(&roof, Some("apartment"), None),
            RoofShape::Flat
        );
    }

    #[test]
    fn infer_flat_from_zero_pitch() {
        let roof = RoofInfo {
            planes: vec![plane(100.0, 0.0, Some(180.0))],
            total_roof_area_m2: 100.0,
        };
        assert_eq!(
            infer_roof_shape(&roof, Some("single-family detached"), None),
            RoofShape::Flat
        );
    }

    /// 4:12 pitch (~18.4°) is the default tilt when HPXML <Pitch> is absent.
    /// A single-plane roof at 18.4° must NOT be classified as Flat — the
    /// boundary for Flat is <1.0°.
    #[test]
    fn infer_not_flat_from_default_4_12_tilt() {
        let default_tilt = (4.0_f64 / 12.0).atan().to_degrees(); // ~18.4°
        let roof = RoofInfo {
            planes: vec![plane(100.0, default_tilt, Some(180.0))],
            total_roof_area_m2: 100.0,
        };
        let shape = infer_roof_shape(&roof, Some("single-family detached"), Some(40.0));
        assert_ne!(
            shape,
            RoofShape::Flat,
            "default 4:12 tilt must not classify as Flat; got {shape:?}"
        );
    }

    /// 2-plane N/S roof (N=0°, S=180°) at lat 40° with no tile and pitched
    /// tilt — strong Gable evidence: opposing planes, no tile, lat ≥35°,
    /// tilt ≥10°. Classified as Gable (0.75 usable).
    #[test]
    fn infer_gable_2plane_ns() {
        let roof = RoofInfo {
            planes: vec![plane(50.0, 26.0, Some(180.0)), plane(50.0, 26.0, Some(0.0))],
            total_roof_area_m2: 100.0,
        };
        assert_eq!(
            infer_roof_shape(&roof, Some("single-family detached"), Some(40.0)),
            RoofShape::Gable
        );
    }

    /// 2-plane E/W roof (E=90°, W=270°) at lat 40° with no tile and pitched
    /// tilt — strong Gable evidence: opposing planes, no tile, lat ≥35°,
    /// tilt ≥10°. Classified as Gable (0.75 usable).
    #[test]
    fn infer_gable_2plane_ew() {
        let roof = RoofInfo {
            planes: vec![
                plane(50.0, 26.0, Some(90.0)),
                plane(50.0, 26.0, Some(270.0)),
            ],
            total_roof_area_m2: 100.0,
        };
        assert_eq!(
            infer_roof_shape(&roof, Some("single-family detached"), Some(40.0)),
            RoofShape::Gable
        );
    }

    /// Integration: exercise `compute_usable_area` end-to-end with 2-plane
    /// gable N/S roof data and verify the capacity estimate uses the Gable
    /// usable fraction (0.75). The two opposing planes with no tile at lat 40°
    /// accumulate sufficient evidence for Gable classification.
    #[test]
    fn pv_capacity_2plane_gable() {
        let roof = RoofInfo {
            planes: vec![plane(60.0, 26.0, Some(180.0)), plane(40.0, 26.0, Some(0.0))],
            total_roof_area_m2: 100.0,
        };
        // Multi-signal classifier: opposing planes, no tile, lat ≥35°, tilt ≥10°
        // → gable_score = 3 → Gable.
        let shape = infer_roof_shape(&roof, Some("single-family detached"), Some(40.0));
        assert_eq!(shape, RoofShape::Gable);

        let usable =
            compute_usable_area(&roof, shape, &[], Some(40.0), None, None, None, false).unwrap();
        // Gable with 2 planes (no halving): south plane (60 m², not north-facing)
        // → 60 × 0.75 = 45 m², 45 / 2.1 (panel_area) = 21 panels,
        // 21 × 440 W / 1000 = 9.24 kW.
        let gable_expected_panels =
            (60.0 * GABLE_USABLE_FRACTION / DEFAULT_PANEL_AREA_M2).floor() as u32;
        assert_eq!(usable.max_panels, gable_expected_panels);
    }

    #[test]
    fn infer_hip_from_many_azimuths() {
        let roof = RoofInfo {
            planes: vec![
                plane(30.0, 26.0, Some(90.0)),
                plane(30.0, 26.0, Some(180.0)),
                plane(30.0, 26.0, Some(270.0)),
            ],
            total_roof_area_m2: 90.0,
        };
        assert_eq!(infer_roof_shape(&roof, None, None), RoofShape::Hip);
    }

    #[test]
    fn enumerate_returns_multiple_candidates() {
        let roof = RoofInfo {
            planes: vec![
                plane(60.0, 26.0, Some(180.0)), // south -- best
                plane(40.0, 26.0, Some(225.0)), // southwest
                plane(30.0, 26.0, Some(90.0)),  // east
                plane(50.0, 26.0, Some(0.0)),   // north -- filtered
            ],
            total_roof_area_m2: 180.0,
        };
        let candidates = enumerate_pv_candidates(
            &roof,
            RoofShape::Gable,
            &[],
            Some(40.0),
            None,
            None,
            None,
            false,
        );
        // 3 non-north planes.
        assert_eq!(candidates.len(), 3);
        // Best first (south with most area).
        assert!((candidates[0].azimuth_deg - 180.0).abs() < 0.01);
        // All have positive capacity.
        for c in &candidates {
            assert!(c.max_capacity_kw > 0.0);
            assert!(c.max_panels > 0);
        }
    }

    #[test]
    fn wall_azimuth_fallback() {
        let roof = RoofInfo {
            planes: vec![plane(100.0, 26.0, None)], // no azimuth
            total_roof_area_m2: 100.0,
        };
        let usable = compute_usable_area(
            &roof,
            RoofShape::Gable,
            &[90.0, 180.0, 270.0],
            Some(40.0),
            None,
            None,
            None,
            false,
        )
        .unwrap();
        assert!((usable.azimuth_deg - 180.0).abs() < 0.01);
    }

    /// Verify the hip usable fraction against an independent geometric
    /// computation for the calibration geometry (12 m × 10 m, 6:12 pitch).
    ///
    /// This test independently recomputes the max-inscribed-rectangle
    /// usable area from first principles and checks that `usable_fraction(Hip)`
    /// lies between the raw geometric lower bound and the adjusted practical
    /// upper bound.
    #[test]
    fn hip_usable_fraction_matches_geometric_derivation() {
        const HALF_WIDTH: f64 = 5.0; // m, half of 10 m width
        const PITCH_RAD: f64 = 26.6 * std::f64::consts::PI / 180.0;
        let slant = HALF_WIDTH / PITCH_RAD.cos(); // ≈ 5.59 m
        const SETBACK: f64 = 0.9144; // 3 ft in m, IFC 2018 §1204.2
        const OBSTRUCTION: f64 = 0.12; // NREL/TP-6A20-65298 §3.2

        // Gross area (12 m × 10 m footprint, ridge = 12 − 10 = 2 m).
        let trapezoid_gross = 2.0 * (12.0 + 2.0) / 2.0 * slant;
        let triangle_gross = 2.0 * (10.0 * slant / 2.0);
        let gross = trapezoid_gross + triangle_gross;

        let usable_height = slant - 2.0 * SETBACK;

        // Trapezoid face: usable band bottom width, top width.
        let t_bottom = 12.0 - (12.0 - 2.0) * SETBACK / slant;
        let t_top = 12.0 - (12.0 - 2.0) * (slant - SETBACK) / slant;
        // Triangle face: usable band bottom width, top width.
        let tri_bottom = 10.0 * (slant - SETBACK) / slant;
        let tri_top = 10.0 * SETBACK / slant;

        // Max inscribed rectangle in each usable-band trapezoid.
        let trapezoid_usable = 2.0 * t_top.min(t_bottom) * usable_height;
        let triangle_usable = 2.0 * tri_top.min(tri_bottom) * usable_height;
        let geometric_usable = (trapezoid_usable + triangle_usable) * (1.0 - OBSTRUCTION);

        // The fraction should be at least the raw geometric lower bound.
        let fraction_lower = geometric_usable / gross;
        assert!(
            usable_fraction(RoofShape::Hip) >= fraction_lower - 0.02,
            "hip fraction {:.4} below geometric lower bound {:.4}",
            usable_fraction(RoofShape::Hip),
            fraction_lower
        );
        // With practical adjustment (≤ 1.5×), fraction should not exceed
        // 1.5× the lower bound.
        assert!(
            usable_fraction(RoofShape::Hip) <= fraction_lower * 1.5,
            "hip fraction {:.4} exceeds 1.5× geometric lower bound {:.4}",
            usable_fraction(RoofShape::Hip),
            fraction_lower
        );

        // Sanity: hip < gable < flat? No, flat (0.70) < gable (0.75).
        assert!(
            usable_fraction(RoofShape::Hip) < usable_fraction(RoofShape::Gable),
            "hip fraction must be less than gable fraction"
        );
    }

    /// Regression: assert the named Hip constant equals the calibrated value
    /// so any future change is intentional and traceable in version control.
    #[test]
    fn hip_usable_fraction_constant_is_calibrated_value() {
        assert!(
            (HIP_USABLE_FRACTION - 0.35).abs() < f64::EPSILON,
            "HIP_USABLE_FRACTION changed from calibrated 0.35"
        );
    }

    /// East/West production factor must be physically reasonable —
    /// not collapsed to 0.18 (the old DIFFUSE_FRAC-only model) and
    /// not approaching 1.0 (south-facing). The quadratic azimuth derating
    /// model gives ~0.83–0.91 across 25°N to 48°N.
    #[test]
    fn east_west_production_factor_in_physical_range() {
        for (lat, expected_min, expected_max) in [
            (25.0, 0.85, 0.95),
            (30.0, 0.85, 0.95),
            (35.0, 0.85, 0.95),
            (40.0, 0.80, 0.90),
            (48.0, 0.78, 0.87),
        ] {
            let f = azimuth_production_factor(90.0, lat);
            assert!(
                f > 0.5,
                "E/W factor ({:.4}) too low — old model collapsed to 0.18",
                f
            );
            assert!(
                f < 0.95,
                "E/W factor ({:.4}) too close to south — should be < 0.95",
                f
            );
            assert!(
                f >= expected_min,
                "E/W factor ({:.4}) below expected minimum ({:.4}) at lat={lat}",
                f,
                expected_min
            );
            assert!(
                f <= expected_max,
                "E/W factor ({:.4}) above expected maximum ({:.4}) at lat={lat}",
                f,
                expected_max
            );
        }
    }

    /// For the same Hip roof input, the plane selection from
    /// `compute_usable_area` and `enumerate_pv_candidates` must agree:
    /// both use the same latitude-dependent azimuth production factor,
    /// so the best-plane index and azimuth should match.
    #[test]
    fn hip_best_plane_consistent_between_compute_and_enumerate() {
        let roof = RoofInfo {
            planes: vec![
                plane(40.0, 26.0, Some(60.0)),  // ENE — moderate area
                plane(60.0, 26.0, Some(180.0)), // south — largest area
                plane(30.0, 26.0, Some(270.0)), // west — small
                plane(50.0, 26.0, Some(0.0)),   // north (filtered)
            ],
            total_roof_area_m2: 180.0,
        };
        let lat = 40.0;
        let usable = compute_usable_area(
            &roof,
            RoofShape::Hip,
            &[],
            Some(lat),
            None,
            None,
            None,
            false,
        )
        .unwrap();
        let candidates = enumerate_pv_candidates(
            &roof,
            RoofShape::Hip,
            &[],
            Some(lat),
            None,
            None,
            None,
            false,
        );

        // The top enumerated candidate should match compute_usable_area's best plane.
        assert!(
            !candidates.is_empty(),
            "expected at least one non-north enumerated candidate"
        );
        assert_eq!(
            candidates[0].plane_idx, usable.best_plane_idx,
            "top enumerated candidate plane {} != best plane {}",
            candidates[0].plane_idx, usable.best_plane_idx
        );
        assert!(
            (candidates[0].azimuth_deg - usable.azimuth_deg).abs() < 0.01,
            "top candidate azimuth {:.1} != best azimuth {:.1}",
            candidates[0].azimuth_deg,
            usable.azimuth_deg
        );

        // Both paths must use identical azimuth_production_factor calls
        // for each plane — verify by recomputing.
        for c in &candidates {
            let factor = azimuth_production_factor(c.azimuth_deg, lat);
            let shape = RoofShape::Hip;
            let area = roof.planes[c.plane_idx].area_m2;
            let expected_score = plane_solar_score(area, c.azimuth_deg, shape, lat, None);
            assert!(
                (c.solar_score - expected_score).abs() < 1e-10,
                "solar_score mismatch for plane {}: {} vs {}",
                c.plane_idx,
                c.solar_score,
                expected_score
            );
            // The production factor used in enumerate must equal what
            // compute_usable_area's Hip aggregation path would use.
            assert!(
                factor > 0.5,
                "factor {:.4} too low for az={:.1}",
                factor,
                c.azimuth_deg
            );
        }
    }

    // -----------------------------------------------------------------------
    // compute_annual_diffuse_fraction tests
    // -----------------------------------------------------------------------

    #[test]
    fn diffuse_fraction_correct_on_known_data() {
        // GHI / DHI pairs with known Kd = 0.20:
        // Sum GHI = 500, Sum DHI = 100 → Kd = 0.20
        let ghi = vec![200.0, 300.0, 0.0, 150.0];
        let dhi = vec![40.0, 60.0, 5.0, 30.0];
        let kd = compute_annual_diffuse_fraction(&ghi, &dhi);
        // (40+60+30) / (200+300+150) = 130/650 = 0.20
        assert!((kd - 0.20).abs() < 1e-10);
    }

    #[test]
    fn diffuse_fraction_fallback_on_empty_input() {
        assert!(
            (compute_annual_diffuse_fraction(&[], &[]) - DEFAULT_DIFFUSE_FRACTION).abs() < 1e-10
        );
        assert!(
            (compute_annual_diffuse_fraction(&[], &[100.0]) - DEFAULT_DIFFUSE_FRACTION).abs()
                < 1e-10
        );
    }

    #[test]
    fn diffuse_fraction_fallback_when_all_ghi_zero() {
        let ghi = vec![0.0, 0.0, 0.0];
        let dhi = vec![50.0, 30.0, 20.0];
        let kd = compute_annual_diffuse_fraction(&ghi, &dhi);
        assert!((kd - DEFAULT_DIFFUSE_FRACTION).abs() < 1e-10);
    }

    #[test]
    fn diffuse_fraction_filters_nighttime_hours() {
        // All GHI zero → should still fall back.
        // Adding one positive GHI hour: GHI=100, DHI=40 at that hour.
        let ghi = vec![0.0, 100.0, 0.0];
        let dhi = vec![10.0, 40.0, 20.0];
        let kd = compute_annual_diffuse_fraction(&ghi, &dhi);
        // Only hour 1 contributes: 40/100 = 0.40
        assert!((kd - 0.40).abs() < 1e-10);
    }

    #[test]
    fn diffuse_fraction_all_direct_beam() {
        // Kd = 0 when DHI = 0 everywhere (completely clear sky).
        let ghi = vec![500.0, 300.0];
        let dhi = vec![0.0, 0.0];
        let kd = compute_annual_diffuse_fraction(&ghi, &dhi);
        assert!((kd - 0.0).abs() < 1e-10);
    }

    #[test]
    fn diffuse_fraction_all_diffuse() {
        // Kd = 1 when all irradiance is diffuse.
        let ghi = vec![500.0, 300.0];
        let dhi = vec![500.0, 300.0];
        let kd = compute_annual_diffuse_fraction(&ghi, &dhi);
        assert!((kd - 1.0).abs() < 1e-10);
    }

    // -----------------------------------------------------------------------
    // plane_solar_score with varying diffuse fraction
    // -----------------------------------------------------------------------

    #[test]
    fn solar_score_monotonic_with_diffuse_fraction() {
        // Higher Kd → score for non-south planes increases because
        // more irradiance arrives as omnidirectional diffuse.
        let area = 100.0;
        let az = 135.0; // SE-facing (45° from south)
        let shape = RoofShape::Gable;
        let lat = 35.0;

        let score_low_kd = plane_solar_score(area, az, shape, lat, Some(0.12)); // Phoenix (arid)
        let score_mid_kd = plane_solar_score(area, az, shape, lat, Some(0.18)); // continental US avg
        let score_high_kd = plane_solar_score(area, az, shape, lat, Some(0.25)); // Seattle (cloudy)

        assert!(
            score_high_kd > score_mid_kd,
            "cloudy (Kd=0.25) score {:.4} should exceed mid (Kd=0.18) score {:.4}",
            score_high_kd,
            score_mid_kd
        );
        assert!(
            score_mid_kd > score_low_kd,
            "mid (Kd=0.18) score {:.4} should exceed arid (Kd=0.12) score {:.4}",
            score_mid_kd,
            score_low_kd
        );
    }

    #[test]
    fn south_facing_score_independent_of_diffuse_fraction() {
        // Due-south: cos(0°) = 1, so factor = Kd + (1-Kd)*1 = 1 regardless of Kd.
        let area = 100.0;
        let az = 180.0; // due south
        let shape = RoofShape::Gable;
        let lat = 35.0;

        let score_low = plane_solar_score(area, az, shape, lat, Some(0.12));
        let score_high = plane_solar_score(area, az, shape, lat, Some(0.25));
        let expected = area * usable_fraction(shape) * 1.0;

        assert!((score_low - expected).abs() < 1e-10);
        assert!((score_high - expected).abs() < 1e-10);
    }

    #[test]
    fn diffuse_fraction_none_uses_pvwatts_model() {
        // When diffuse_fraction is None, plane_solar_score must use the
        // azimuth_production_factor model (backward compatibility).
        let area = 100.0;
        let az = 135.0;
        let shape = RoofShape::Gable;
        let lat = 35.0;

        let score = plane_solar_score(area, az, shape, lat, None);
        let expected = area * usable_fraction(shape) * azimuth_production_factor(az, lat);
        assert!((score - expected).abs() < 1e-10);
    }

    #[test]
    fn east_west_production_increases_with_diffuse_fraction() {
        // E/W panels benefit most from diffuse: at θ=90° the ticket model
        // gives factor = Kd (beam contribution = 0). Higher Kd directly
        // increases the E/W score.
        let area = 100.0;
        let az = 90.0; // due east
        let shape = RoofShape::Gable;
        let lat = 35.0;

        let score_012 = plane_solar_score(area, az, shape, lat, Some(0.12));
        let score_025 = plane_solar_score(area, az, shape, lat, Some(0.25));

        // Kd=0.25 → factor = 0.25, Kd=0.12 → factor = 0.12
        // score ratio ≈ 0.25/0.12 ≈ 2.08
        assert!(score_025 > score_012 * 2.0);
    }

    // -----------------------------------------------------------------------
    // compute_usable_area with varying diffuse fraction (integration)
    // -----------------------------------------------------------------------

    #[test]
    fn plane_selection_differs_with_diffuse_fraction() {
        // Two Gable planes: south plane has moderate area, east plane has 4× area.
        // At low Kd (arid, 0.12), south plane wins because orientation dominates.
        // At high Kd (cloudy, 0.28), east plane wins because area dominates.
        // Two-plane Gable → no halving, each plane used directly.
        let roof = RoofInfo {
            planes: vec![
                plane(28.0, 26.0, Some(180.0)), // south, small
                plane(112.0, 26.0, Some(90.0)), // east, 4× larger
            ],
            total_roof_area_m2: 140.0,
        };

        // Low Kd (0.12): south plane favored because orientation matters more.
        let result_low_kd = compute_usable_area(
            &roof,
            RoofShape::Gable,
            &[],
            Some(35.0),
            None,
            None,
            Some(0.12),
            false,
        )
        .unwrap();
        assert_eq!(
            result_low_kd.best_plane_idx, 0,
            "at Kd=0.12, south plane should be selected"
        );

        // High Kd (0.28): east plane wins because area dominates orientation.
        let result_high_kd = compute_usable_area(
            &roof,
            RoofShape::Gable,
            &[],
            Some(35.0),
            None,
            None,
            Some(0.28),
            false,
        )
        .unwrap();
        assert_eq!(
            result_high_kd.best_plane_idx, 1,
            "at Kd=0.28, larger east plane should be selected"
        );
    }

    #[test]
    fn solar_score_exactly_one_for_south_with_any_kd() {
        // Invariant: south-facing always scores 1.0× the area-weight product.
        let area = 100.0;
        let shape = RoofShape::Gable;
        let base = area * usable_fraction(shape);
        for kd in [0.0, 0.10, 0.18, 0.25, 0.40, 0.60, 1.0] {
            let score = plane_solar_score(area, 180.0, shape, 35.0, Some(kd));
            assert!(
                (score - base).abs() < 1e-10,
                "south score should equal area × usable at Kd={kd}, got {score} vs {base}"
            );
        }
    }

    #[test]
    fn solar_score_bounded_between_zero_and_area() {
        let area = 100.0;
        let shape = RoofShape::Gable;
        let max_score = area * usable_fraction(shape);
        for kd in [0.0, 0.10, 0.18, 0.25, 0.40, 1.0] {
            for az in (0..360).step_by(45) {
                let score = plane_solar_score(area, az as f64, shape, 35.0, Some(kd));
                assert!(
                    score >= 0.0,
                    "score should be non-negative: Kd={kd}, az={az}"
                );
                assert!(
                    score <= max_score + 1e-10,
                    "score should not exceed area × usable: Kd={kd}, az={az}, {score} > {max_score}"
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // flat_roof_gcr — geometric GCR from tilt + latitude
    // -----------------------------------------------------------------------

    /// Low tilt (5°) at moderate latitude (30°N) must yield a high GCR.
    #[test]
    fn flat_roof_gcr_low_tilt_yields_high_gcr() {
        let gcr = flat_roof_gcr(Some(30.0), 5.0);
        assert!(
            gcr >= 0.55,
            "low tilt (5°) at lat=30° should have GCR≥0.55, got {gcr:.4}"
        );
    }

    /// Shallow racking tilt (25°) at high latitude (50°N): GCR should be
    /// approximately 0.35 as the old step table produced, confirming the
    /// low tilt enables workable row spacing. Continuous min_solar_elevation
    /// floors at 12° for physically reasonable shading tolerance.
    #[test]
    fn flat_roof_gcr_moderate_tilt_high_lat_near_old_band() {
        let gcr = flat_roof_gcr(Some(50.0), 25.0);
        // Geometric formula with min_solar_elevation=12° at lat=50° gives
        // GCR ≈ 0.345 — consistent with the old step table's 0.35 band.
        assert!(
            (gcr - 0.345).abs() < 0.01,
            "tilt=25° at lat=50° should have GCR ≈ 0.345, got {gcr:.4}"
        );
    }

    /// Steep tilt (45°) at high latitude (50°N): must hit the GCR floor.
    #[test]
    fn flat_roof_gcr_steep_tilt_high_lat_at_floor() {
        let gcr = flat_roof_gcr(Some(50.0), 45.0);
        assert!(
            gcr <= 0.25 + 0.01,
            "steep tilt (45°) at lat=50° should have GCR≤0.25, got {gcr:.4}"
        );
    }

    /// GCR must monotonically decrease with increasing tilt at fixed latitude.
    #[test]
    fn flat_roof_gcr_monotonic_in_tilt() {
        for lat in [20.0, 30.0, 40.0, 50.0] {
            for pair in [(5.0, 15.0), (15.0, 25.0), (25.0, 35.0), (35.0, 45.0)] {
                let gcr_lo = flat_roof_gcr(Some(lat), pair.0);
                let gcr_hi = flat_roof_gcr(Some(lat), pair.1);
                assert!(
                    gcr_lo >= gcr_hi,
                    "GCR must not increase with tilt: lat={lat} tilt {}→{} gives {gcr_lo:.4},{gcr_hi:.4}",
                    pair.0,
                    pair.1,
                );
            }
        }
    }

    /// GCR must monotonically decrease with increasing latitude at fixed tilt.
    #[test]
    fn flat_roof_gcr_monotonic_in_latitude() {
        for tilt in [5.0, 15.0, 25.0, 45.0] {
            for pair in [(20.0, 35.0), (35.0, 50.0), (20.0, 50.0)] {
                let gcr_lo = flat_roof_gcr(Some(pair.0), tilt);
                let gcr_hi = flat_roof_gcr(Some(pair.1), tilt);
                assert!(
                    gcr_lo >= gcr_hi,
                    "GCR must not increase with latitude: tilt={tilt} lat {}→{} gives {gcr_lo:.4},{gcr_hi:.4}",
                    pair.0,
                    pair.1,
                );
            }
        }
    }

    /// GCR must stay within the clamped range [0.25, 0.65] for all
    /// physically reasonable inputs.
    #[test]
    fn flat_roof_gcr_stays_in_clamped_range() {
        for lat in [0.0, 20.0, 35.0, 50.0, 70.0] {
            for tilt in [0.0, 3.0, 10.0, 25.0, 45.0, 60.0, 90.0] {
                let gcr = flat_roof_gcr(Some(lat), tilt);
                assert!(
                    gcr >= 0.25,
                    "GCR={gcr:.4} below min 0.25 at lat={lat} tilt={tilt}"
                );
                assert!(
                    gcr <= 0.65,
                    "GCR={gcr:.4} above max 0.65 at lat={lat} tilt={tilt}"
                );
            }
        }
    }

    /// GCR values are continuous (no step-cliffs) near the old band boundaries.
    /// Two latitudes 0.1° apart on opposite sides of the old 40.0° boundary
    /// must produce nearly identical GCR.
    #[test]
    fn flat_roof_gcr_continuous_no_step_cliffs() {
        let gcr_399 = flat_roof_gcr(Some(39.9), 25.0);
        let gcr_401 = flat_roof_gcr(Some(40.1), 25.0);
        assert!(
            (gcr_399 - gcr_401).abs() < 0.02,
            "GCR step at 40°N boundary: lat 39.9→{gcr_399:.4}, 40.1→{gcr_401:.4}"
        );

        let gcr_299 = flat_roof_gcr(Some(29.9), 25.0);
        let gcr_301 = flat_roof_gcr(Some(30.1), 25.0);
        assert!(
            (gcr_299 - gcr_301).abs() < 0.02,
            "GCR step at 30°N boundary: lat 29.9→{gcr_299:.4}, 30.1→{gcr_301:.4}"
        );
    }

    /// GCR for `None` latitude falls back to lat=35.0.
    #[test]
    fn flat_roof_gcr_none_latitude_fallback() {
        // None latitude should act the same as lat=35.0
        let gcr_none = flat_roof_gcr(None, 25.0);
        let gcr_35 = flat_roof_gcr(Some(35.0), 25.0);
        assert!(
            (gcr_none - gcr_35).abs() < 1e-10,
            "GCR with None latitude should match lat=35.0: {gcr_none:.4} vs {gcr_35:.4}"
        );
    }

    // -----------------------------------------------------------------------
    // Integration / regression — flat-roof usable area
    // -----------------------------------------------------------------------

    /// The existing flat_roof_uses_gcr test must continue to pass with the
    /// new geometric GCR. Verify explicitly that the exact panel count is
    /// preserved for the test geometry: lat=35°, GCR≈0.40 at tilt=25°.
    #[test]
    fn flat_roof_uses_gcr_regression() {
        let roof = RoofInfo {
            planes: vec![plane(200.0, 0.0, Some(180.0))],
            total_roof_area_m2: 200.0,
        };
        let usable = compute_usable_area(
            &roof,
            RoofShape::Flat,
            &[],
            Some(35.0),
            None,
            None,
            None,
            false,
        )
        .unwrap();
        // 200 × 0.70 = 140 m² usable. GCR ≈ 0.40 at lat=35° tilt=25°.
        // Panel footprint = 2.1 / ~0.403 ≈ 5.21 m². 140 / 5.21 ≈ 26.87 → 26.
        assert_eq!(usable.max_panels, 26);
    }

    /// Flat-roof capacity must decrease monotonically with latitude.
    /// Synthetic 100 m² flat roofs at increasing latitudes should produce
    /// strictly non-increasing panel counts.
    #[test]
    fn flat_roof_capacity_monotonically_decreases_with_latitude() {
        let roof = RoofInfo {
            planes: vec![plane(100.0, 0.0, Some(180.0))],
            total_roof_area_m2: 100.0,
        };
        let mut prev_panels = u32::MAX;
        for lat in [30.0, 35.0, 40.0, 45.0, 50.0] {
            let usable = compute_usable_area(
                &roof,
                RoofShape::Flat,
                &[],
                Some(lat),
                None,
                None,
                None,
                false,
            )
            .unwrap();
            assert!(
                usable.max_panels <= prev_panels,
                "capacity must not increase with latitude: lat={lat} panels={} > prev={prev_panels}",
                usable.max_panels
            );
            assert!(
                usable.max_panels > 0,
                "capacity must be positive at lat={lat}"
            );
            prev_panels = usable.max_panels;
        }
    }

    /// At high latitudes with the 25° tilt cap, the geometric GCR must
    /// produce higher capacity than what the old step table would give,
    /// because the low tilt cap reduces inter-row shading.
    /// Reference: old step table GCR=0.35 at lat>40°; geometric formula
    /// gives higher GCR at low tilt.
    #[test]
    fn flat_roof_geometric_gcr_improves_over_old_step_table_at_high_latitudes() {
        let roof = RoofInfo {
            planes: vec![plane(200.0, 0.0, Some(180.0))],
            total_roof_area_m2: 200.0,
        };

        // New geometric GCR at lat=45°, tilt=25° (the cap)
        let usable_new = compute_usable_area(
            &roof,
            RoofShape::Flat,
            &[],
            Some(45.0),
            None,
            None,
            None,
            false,
        )
        .unwrap();

        // Old step-table GCR=0.35 would give:
        // usable=140, footprint=2.0/0.35=5.714, panels=140/5.714=24.5→24
        let old_gcr = 0.35;
        let old_panels =
            ((200.0 * FLAT_USABLE_FRACTION) / (DEFAULT_PANEL_AREA_M2 / old_gcr)).floor() as u32;

        // The new geometric GCR should produce equal or more panels than
        // the conservative old step table, because the 25° tilt cap
        // is now correctly factored into the GCR computation.
        assert!(
            usable_new.max_panels >= old_panels,
            "new geometric GCR panels ({}) must be ≥ old step table panels ({}) at lat=45°",
            usable_new.max_panels,
            old_panels
        );
    }

    /// Enumerate candidates on a flat roof: each candidate must use the
    /// new tilt-parameterized GCR (consistent with compute_usable_area).
    #[test]
    fn enumerate_candidates_uses_geometric_gcr_for_flat_roof() {
        let roof = RoofInfo {
            planes: vec![
                plane(100.0, 0.0, Some(180.0)),
                plane(100.0, 0.0, Some(225.0)),
            ],
            total_roof_area_m2: 200.0,
        };
        let candidates = enumerate_pv_candidates(
            &roof,
            RoofShape::Flat,
            &[],
            Some(45.0),
            None,
            None,
            None,
            false,
        );
        assert_eq!(candidates.len(), 2);
        for c in &candidates {
            assert!(c.max_panels > 0);
            assert!(c.max_capacity_kw > 0.0);
            // With geometric GCR at lat=45° tilt=25°, the footprint should
            // be consistent with the formula, not the old step table.
            let gcr = flat_roof_gcr(Some(45.0), 25.0);
            let expected_footprint = DEFAULT_PANEL_AREA_M2 / gcr;
            assert!(
                (c.usable_m2 / expected_footprint - c.max_panels as f64).abs() < 1.0,
                "candidate panel count should use geometric GCR"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Panel override round-trip consistency
    // -----------------------------------------------------------------------

    /// Passing explicit overrides equal to the compile-time defaults must
    /// produce exactly the same result as passing `None` — the two paths
    /// are mathematically equivalent.
    #[test]
    fn explicit_overrides_match_default_behavior() {
        let roof = RoofInfo {
            planes: vec![plane(100.0, 26.0, Some(180.0))],
            total_roof_area_m2: 100.0,
        };
        let lat = 40.0;

        // Default path (None for panel params).
        let result_default = compute_usable_area(
            &roof,
            RoofShape::Gable,
            &[],
            Some(lat),
            None,
            None,
            None,
            false,
        )
        .unwrap();

        // Explicit override equal to compile-time constants.
        let result_explicit = compute_usable_area(
            &roof,
            RoofShape::Gable,
            &[],
            Some(lat),
            Some(DEFAULT_PANEL_WATTS),
            Some(DEFAULT_PANEL_AREA_M2),
            None,
            false,
        )
        .unwrap();

        assert_eq!(result_default.max_panels, result_explicit.max_panels);
        assert!(
            (result_default.max_capacity_kw - result_explicit.max_capacity_kw).abs() < 1e-10,
            "max_capacity_kw mismatch: {} vs {}",
            result_default.max_capacity_kw,
            result_explicit.max_capacity_kw
        );
        assert_eq!(
            result_default.best_plane_idx,
            result_explicit.best_plane_idx
        );
    }

    /// When a custom panel spec is provided via overrides, the sizing result
    /// must differ from the default (440 W baseline). A smaller, less powerful
    /// panel yields fewer watts per m² and thus lower capacity for the same
    /// usable area.
    #[test]
    fn custom_panel_override_produces_different_result() {
        let roof = RoofInfo {
            planes: vec![plane(100.0, 26.0, Some(180.0))],
            total_roof_area_m2: 100.0,
        };
        let lat = 40.0;

        let result_440w = compute_usable_area(
            &roof,
            RoofShape::Gable,
            &[],
            Some(lat),
            None,
            None,
            None,
            false,
        )
        .unwrap();

        // Hypothetical low-wattage panel: 300 W, 1.6 m² (older/smaller module).
        let result_300w = compute_usable_area(
            &roof,
            RoofShape::Gable,
            &[],
            Some(lat),
            Some(300),
            Some(1.6),
            None,
            false,
        )
        .unwrap();

        // Same roof, same usable area → panel count differs by area ratio.
        // max_capacity_kw should differ because of wattage change.
        assert!(
            result_440w.max_capacity_kw > result_300w.max_capacity_kw,
            "440W panel ({:.2} kW) should yield higher capacity than 300W ({:.2} kW)",
            result_440w.max_capacity_kw,
            result_300w.max_capacity_kw
        );

        // Result from size_pv_system with explicit losses must match explicit
        // vs None (default losses = 0.14).
        let sizing_default =
            size_pv_system(&result_440w, 5.0, 2.0, 14.0, None, None, None).unwrap();
        let sizing_explicit = size_pv_system(
            &result_440w,
            5.0,
            2.0,
            14.0,
            Some(0.14),
            Some(440),
            Some(2.0),
        )
        .unwrap();
        assert_eq!(
            sizing_default.num_panels, sizing_explicit.num_panels,
            "explicit override matching compile-time default must yield identical result"
        );
        assert!(
            (sizing_default.system_losses_fraction - sizing_explicit.system_losses_fraction).abs()
                < 1e-10
        );
    }

    // -----------------------------------------------------------------------
    // roof_shape_user_override tests
    // -----------------------------------------------------------------------

    /// When `roof_shape_user_override` is `true` with `RoofShape::Hip`, the
    /// Hip usable fraction (0.35) is applied regardless of the roof data that
    /// inference would produce. A roof with single south-facing gable plane
    /// would produce 37.5 m² usable with the Gable fraction (0.75), but with
    /// the Hip override it drops to 17.5 m² (0.35).
    #[test]
    fn compute_usable_area_with_override_uses_specified_shape() {
        let roof = RoofInfo {
            planes: vec![plane(100.0, 26.0, Some(180.0))],
            total_roof_area_m2: 100.0,
        };
        // Gable override (not what the roof data suggests).
        let usable_gable = compute_usable_area(
            &roof,
            RoofShape::Gable,
            &[],
            Some(40.0),
            None,
            None,
            None,
            true,
        )
        .unwrap();
        // Hip override — same roof, same params, different shape.
        let usable_hip = compute_usable_area(
            &roof,
            RoofShape::Hip,
            &[],
            Some(40.0),
            None,
            None,
            None,
            true,
        )
        .unwrap();

        // Gable: single plane → halved → 50 × 0.75 = 37.5
        assert!((usable_gable.usable_m2 - 100.0 / 2.0 * 0.75).abs() < 0.01);
        assert_eq!(usable_gable.roof_shape, RoofShape::Gable);

        // Hip: single plane, no halving in Hip path, but Hip selects best plane
        // and aggregates. Single south plane → 100 × 0.35 = 35 m².
        assert!((usable_hip.usable_m2 - 100.0 * HIP_USABLE_FRACTION).abs() < 0.01);
        assert_eq!(usable_hip.roof_shape, RoofShape::Hip);

        // The two capacities must differ — override changes the outcome.
        assert_ne!(
            usable_gable.max_capacity_kw, usable_hip.max_capacity_kw,
            "Hip override must produce different capacity than Gable for same roof"
        );
        assert!(
            usable_gable.max_capacity_kw > usable_hip.max_capacity_kw,
            "Gable override should produce higher capacity than Hip override"
        );
    }

    /// When `roof_shape_user_override` is `false`, the existing computation
    /// path is followed with no change in behaviour — a regression guard.
    #[test]
    fn compute_usable_area_without_override_unchanged() {
        let roof = RoofInfo {
            planes: vec![plane(100.0, 26.0, Some(180.0))],
            total_roof_area_m2: 100.0,
        };
        // Default path (false) — same as existing single_south_gable test.
        let usable = compute_usable_area(
            &roof,
            RoofShape::Gable,
            &[],
            Some(40.0),
            None,
            None,
            None,
            false,
        )
        .unwrap();
        assert!((usable.usable_m2 - 100.0 / 2.0 * 0.75).abs() < 0.01);
        assert!((usable.azimuth_deg - 180.0).abs() < 0.01);
        assert_eq!(usable.roof_shape, RoofShape::Gable);
    }

    /// Enumerate candidates with an override — verifies the
    /// `roof_shape_user_override` parameter flows through to
    /// `enumerate_pv_candidates` and produces results using the
    /// specified shape rather than what inference would suggest.
    #[test]
    fn enumerate_pv_candidates_with_override() {
        let roof = RoofInfo {
            planes: vec![
                plane(60.0, 26.0, Some(180.0)),
                plane(40.0, 26.0, Some(225.0)),
                plane(30.0, 26.0, Some(90.0)),
            ],
            total_roof_area_m2: 130.0,
        };
        // With override=true, Gable shape: single-plane halving logic may apply.
        // Without override, inference might pick Hip (3 distinct azimuths).
        let candidates = enumerate_pv_candidates(
            &roof,
            RoofShape::Gable,
            &[],
            Some(40.0),
            None,
            None,
            None,
            true,
        );
        assert_eq!(candidates.len(), 3);
        for c in &candidates {
            assert_eq!(c.roof_shape, RoofShape::Gable);
            assert!(c.max_panels > 0);
        }
    }

    /// 2-plane opposite E/W roof with tile material at Phoenix AZ latitude
    /// (33°N). The multi-signal Gable classifier requires score ≥2; this
    /// case scores 1 (tilt only — tile blocks +1, lat <35° blocks +1), so
    /// it correctly falls to the conservative Hip fallback. Demonstrates
    /// that opposing geometry alone is insufficient for Gable classification
    /// when material (tile) and latitude signals both oppose it.
    #[test]
    fn infer_hip_tile_2plane_midlat() {
        let roof = RoofInfo {
            planes: vec![
                RoofPlane {
                    area_m2: 50.0,
                    tilt_deg: 26.0,
                    azimuth_deg: Some(90.0),
                    material: Some("clay tile".into()),
                    boundary_index: None,
                },
                RoofPlane {
                    area_m2: 50.0,
                    tilt_deg: 26.0,
                    azimuth_deg: Some(270.0),
                    material: Some("clay tile".into()),
                    boundary_index: None,
                },
            ],
            total_roof_area_m2: 100.0,
        };
        // At lat=33° (>= 30°) the low-latitude rule does not fire;
        // the function falls through to the conservative fallback (Hip).
        let shape = infer_roof_shape(&roof, Some("single-family detached"), Some(33.0));
        // Falls to fallback, which returns Hip. The material rule is gone
        // — tile/slate no longer forces Hip classification here.
        assert_eq!(shape, RoofShape::Hip);
    }

    /// ≥3 distinct azimuths with tile material still correctly classified
    /// as Hip via the geometry rule (≥3 azimuths), not the removed material
    /// rule. Verifies that legitimate tile hip roofs continue to be
    /// classified correctly by the structural signal.
    #[test]
    fn tile_hip_still_works() {
        let roof = RoofInfo {
            planes: vec![
                RoofPlane {
                    area_m2: 30.0,
                    tilt_deg: 26.0,
                    azimuth_deg: Some(90.0),
                    material: Some("clay tile".into()),
                    boundary_index: None,
                },
                RoofPlane {
                    area_m2: 30.0,
                    tilt_deg: 26.0,
                    azimuth_deg: Some(180.0),
                    material: Some("clay tile".into()),
                    boundary_index: None,
                },
                RoofPlane {
                    area_m2: 30.0,
                    tilt_deg: 26.0,
                    azimuth_deg: Some(270.0),
                    material: Some("clay tile".into()),
                    boundary_index: None,
                },
            ],
            total_roof_area_m2: 90.0,
        };
        assert_eq!(
            infer_roof_shape(&roof, Some("single-family detached"), None),
            RoofShape::Hip
        );
    }

    /// Tile roof with 2 opposite planes at low latitude (< 30°) — the
    /// low-latitude hard rule dominates, returning Hip even though
    /// geometry (opposing planes) and tile material pull in opposite
    /// directions. Demonstrates that geometry does not override the
    /// conservative low-latitude gate.
    #[test]
    fn infer_hip_2plane_tile_low_lat() {
        let roof = RoofInfo {
            planes: vec![
                RoofPlane {
                    area_m2: 50.0,
                    tilt_deg: 26.0,
                    azimuth_deg: Some(90.0),
                    material: Some("clay tile".into()),
                    boundary_index: None,
                },
                RoofPlane {
                    area_m2: 50.0,
                    tilt_deg: 26.0,
                    azimuth_deg: Some(270.0),
                    material: Some("clay tile".into()),
                    boundary_index: None,
                },
            ],
            total_roof_area_m2: 100.0,
        };
        assert_eq!(
            infer_roof_shape(&roof, Some("single-family detached"), Some(25.0)),
            RoofShape::Hip
        );
    }

    /// 2-plane non-opposite roof (S=180°, SW=225°) at lat 40° — the
    /// two azimuths are 45° apart, not approximately opposite. Gable
    /// detection requires opposing planes; non-opposite geometry
    /// suggests hip with omitted minor planes. Falls to conservative
    /// Hip fallback.
    #[test]
    fn infer_hip_2plane_non_opposite() {
        let roof = RoofInfo {
            planes: vec![
                plane(60.0, 26.0, Some(180.0)),
                plane(40.0, 26.0, Some(225.0)),
            ],
            total_roof_area_m2: 100.0,
        };
        assert_eq!(
            infer_roof_shape(&roof, Some("single-family detached"), Some(40.0)),
            RoofShape::Hip
        );
    }

    /// 2-plane opposite roof with shallow tilt (8° < 10° threshold) at
    /// lat 40°. Score = no-tile(+1) + lat≥35°(+1) + tilt(0) = 2 ≥ 2 → Gable.
    /// Even without tilt support, the combination of opposing geometry + no
    /// tile + high latitude provides sufficient evidence for Gable.
    #[test]
    fn infer_gable_2plane_shallow_tilt_no_tile_high_lat() {
        let roof = RoofInfo {
            planes: vec![plane(50.0, 8.0, Some(90.0)), plane(50.0, 8.0, Some(270.0))],
            total_roof_area_m2: 100.0,
        };
        assert_eq!(
            infer_roof_shape(&roof, Some("single-family detached"), Some(40.0)),
            RoofShape::Gable
        );
    }

    /// 2-plane opposite roof with tile and shallow tilt at lat 40°.
    /// Tile blocks +1, shallow tilt blocks +1 — only lat ≥35° contributes.
    /// Score = 1 < 2 → Hip. The combination of tile (hip-associated)
    /// and shallow tilt (could be low-slope hip) is too ambiguous.
    #[test]
    fn infer_hip_2plane_tile_shallow_tilt_high_lat() {
        let roof = RoofInfo {
            planes: vec![
                RoofPlane {
                    area_m2: 50.0,
                    tilt_deg: 8.0,
                    azimuth_deg: Some(90.0),
                    material: Some("clay tile".into()),
                    boundary_index: None,
                },
                RoofPlane {
                    area_m2: 50.0,
                    tilt_deg: 8.0,
                    azimuth_deg: Some(270.0),
                    material: Some("clay tile".into()),
                    boundary_index: None,
                },
            ],
            total_roof_area_m2: 100.0,
        };
        assert_eq!(
            infer_roof_shape(&roof, Some("single-family detached"), Some(40.0)),
            RoofShape::Hip
        );
    }

    /// 3-plane hip roof where only 2 planes have azimuth data. The third
    /// plane's orientation is unknown, so the evidence is ambiguous — must
    /// conservatively fall to Hip, not Gable. Guards against the
    /// `plane_azimuths.len() == 2 && roof.planes.len() == 3` divergence
    /// where the Hip guard (`distinct_azimuths.len() >= 3`) does not fire
    /// because the third plane lacks azimuth.
    #[test]
    fn infer_hip_3plane_one_missing_azimuth_not_gable() {
        let roof = RoofInfo {
            planes: vec![
                plane(50.0, 26.0, Some(90.0)),
                plane(50.0, 26.0, Some(270.0)),
                plane(30.0, 26.0, None),
            ],
            total_roof_area_m2: 130.0,
        };
        assert_eq!(
            infer_roof_shape(&roof, Some("single-family detached"), Some(40.0)),
            RoofShape::Hip
        );
    }

    /// 2-plane opposite roof with no tile and pitched tilt at lat=32°N
    /// (inside the 30°–35° latitude gap). Score = no-tile(+1) + tilt(+1)
    /// = 2 ≥ 2 → Gable. Verifies that reasonable evidence in the gap
    /// band still produces Gable even without the latitude signal.
    #[test]
    fn infer_gable_2plane_no_tile_lat32_gap() {
        let roof = RoofInfo {
            planes: vec![
                plane(50.0, 26.0, Some(90.0)),
                plane(50.0, 26.0, Some(270.0)),
            ],
            total_roof_area_m2: 100.0,
        };
        assert_eq!(
            infer_roof_shape(&roof, Some("single-family detached"), Some(32.0)),
            RoofShape::Gable
        );
    }

    // -----------------------------------------------------------------------
    // East-west dual-tilt flat-roof tests
    // -----------------------------------------------------------------------

    /// East-west dual-tilt on a 200 m² flat roof: total area × 0.70 usable ×
    /// EAST_WEST_GCR (0.85) yields higher capacity than south-facing flat.
    #[test]
    fn east_west_basic_capacity() {
        let roof = RoofInfo {
            planes: vec![plane(200.0, 0.0, Some(180.0))],
            total_roof_area_m2: 200.0,
        };
        let usable = compute_usable_area(
            &roof,
            RoofShape::FlatEastWest,
            &[],
            Some(35.0),
            None,
            None,
            None,
            true,
        )
        .unwrap();

        // 200 × 0.70 = 140 m² usable.
        // Panel footprint = 2.1 / 0.85 ≈ 2.4706 m².
        // Total panels = floor(140 / 2.4706) = 56.
        // Paired = 56 / 2 * 2 = 56.
        assert_eq!(usable.max_panels, 56);
        assert_eq!(usable.roof_shape, RoofShape::FlatEastWest);
        assert!((usable.tilt_deg - 10.0).abs() < 0.01);
        assert!((usable.azimuth_deg - 90.0).abs() < 0.01);
        assert!(usable.max_capacity_kw > 0.0);
    }

    /// East-west yields more panels than south-facing flat for the same roof
    /// because the continuous envelope GCR (0.85) substantially exceeds the
    /// south-facing GCR (~0.40 at 35°N, 25° tilt).
    #[test]
    fn east_west_higher_than_south_facing_flat() {
        let roof = RoofInfo {
            planes: vec![plane(200.0, 0.0, Some(180.0))],
            total_roof_area_m2: 200.0,
        };

        let ew = compute_usable_area(
            &roof,
            RoofShape::FlatEastWest,
            &[],
            Some(35.0),
            None,
            None,
            None,
            true,
        )
        .unwrap();

        let sf = compute_usable_area(
            &roof,
            RoofShape::Flat,
            &[],
            Some(35.0),
            None,
            None,
            None,
            true,
        )
        .unwrap();

        assert!(
            ew.max_panels > sf.max_panels,
            "east-west panels ({}) should exceed south-facing flat panels ({})",
            ew.max_panels,
            sf.max_panels
        );
        assert!(
            ew.max_capacity_kw > sf.max_capacity_kw,
            "east-west capacity ({:.2} kW) should exceed south-facing flat ({:.2} kW)",
            ew.max_capacity_kw,
            sf.max_capacity_kw
        );
    }

    /// East-west candidate enumeration produces exactly two candidates —
    /// east (90°) and west (270°) — each with half the total usable area.
    #[test]
    fn east_west_enumerate_two_candidates() {
        let roof = RoofInfo {
            planes: vec![plane(200.0, 0.0, Some(180.0))],
            total_roof_area_m2: 200.0,
        };
        let candidates = enumerate_pv_candidates(
            &roof,
            RoofShape::FlatEastWest,
            &[],
            Some(35.0),
            None,
            None,
            None,
            true,
        );
        assert_eq!(candidates.len(), 2);

        let azs: Vec<f64> = candidates.iter().map(|c| c.azimuth_deg).collect();
        assert!(azs.contains(&90.0), "east azimuth (90°) missing");
        assert!(azs.contains(&270.0), "west azimuth (270°) missing");

        // Each candidate gets roughly half the capacity.
        for c in &candidates {
            assert!(c.max_panels > 0);
            assert!(c.max_capacity_kw > 0.0);
            assert!((c.tilt_deg - 10.0).abs() < 0.01);
            assert_eq!(c.roof_shape, RoofShape::FlatEastWest);
        }
    }

    /// East-west 50/50 split: east and west candidates must have equal
    /// panel counts (floor of total/2 each).
    #[test]
    fn east_west_equal_split() {
        let roof = RoofInfo {
            planes: vec![plane(200.0, 0.0, Some(180.0))],
            total_roof_area_m2: 200.0,
        };
        let candidates = enumerate_pv_candidates(
            &roof,
            RoofShape::FlatEastWest,
            &[],
            Some(35.0),
            None,
            None,
            None,
            true,
        );

        let east = candidates
            .iter()
            .find(|c| (c.azimuth_deg - 90.0).abs() < 1.0)
            .unwrap();
        let west = candidates
            .iter()
            .find(|c| (c.azimuth_deg - 270.0).abs() < 1.0)
            .unwrap();
        assert_eq!(east.max_panels, west.max_panels);
        assert!((east.max_capacity_kw - west.max_capacity_kw).abs() < 1e-10);
    }

    /// FlatEastWest is never returned by infer_roof_shape — it is only
    /// available via user override.
    #[test]
    fn infer_never_returns_flat_east_west() {
        let roof = RoofInfo {
            planes: vec![plane(200.0, 0.0, Some(180.0))],
            total_roof_area_m2: 200.0,
        };
        let shape = infer_roof_shape(&roof, Some("apartment"), Some(40.0));
        assert_eq!(shape, RoofShape::Flat);
        assert_ne!(shape, RoofShape::FlatEastWest);
    }

    /// East-west with a small roof: total panels may be odd, so paired
    /// panels should be even (per_orientation = total/2, paired = 2×).
    #[test]
    fn east_west_paired_panels_even() {
        let roof = RoofInfo {
            planes: vec![plane(80.0, 0.0, Some(180.0))],
            total_roof_area_m2: 80.0,
        };
        let usable = compute_usable_area(
            &roof,
            RoofShape::FlatEastWest,
            &[],
            Some(35.0),
            None,
            None,
            None,
            true,
        )
        .unwrap();
        // 80 × 0.70 = 56; footprint = 2.1/0.85 = 2.471; total = 22; paired = 22
        assert_eq!(usable.max_panels % 2, 0, "paired panels must be even");
    }

    /// East-west with no latitude provided uses the default panel spec and
    /// produces a valid (nonzero) result.
    #[test]
    fn east_west_no_latitude_defaults() {
        let roof = RoofInfo {
            planes: vec![plane(100.0, 0.0, None)],
            total_roof_area_m2: 100.0,
        };
        let usable = compute_usable_area(
            &roof,
            RoofShape::FlatEastWest,
            &[180.0],
            None,
            None,
            None,
            None,
            true,
        )
        .unwrap();
        assert!(usable.max_panels > 0);
        assert!(usable.max_capacity_kw > 0.0);
    }

    /// FlatEastWest with all-north-azimuth planes and no wall azimuths must
    /// succeed — east-west dual-tilt ignores individual plane orientations
    /// and uses total roof area. Verifies fix for a bug where AllNorthFacing
    /// was returned before the FlatEastWest path could execute.
    #[test]
    fn east_west_all_north_planes_succeeds() {
        let roof = RoofInfo {
            planes: vec![plane(50.0, 0.0, Some(0.0)), plane(50.0, 0.0, Some(0.0))],
            total_roof_area_m2: 100.0,
        };
        let result = compute_usable_area(
            &roof,
            RoofShape::FlatEastWest,
            &[], // no wall azimuths — all planes resolve to 0°
            Some(35.0),
            None,
            None,
            None,
            true,
        );
        assert!(
            result.is_ok(),
            "FlatEastWest must succeed even with all-north planes and no wall azimuths"
        );
        let usable = result.unwrap();
        assert_eq!(usable.roof_shape, RoofShape::FlatEastWest);
        assert!(usable.max_panels > 0);
        assert!(usable.max_capacity_kw > 0.0);
        // 100 × 0.70 = 70 m² usable; footprint = 2.1/0.85 ≈ 2.471;
        // total panels = floor(70/2.471) = 28; paired = 28
        assert_eq!(usable.max_panels, 28);
        assert!((usable.tilt_deg - 10.0).abs() < 0.01);
        assert!((usable.azimuth_deg - 90.0).abs() < 0.01);
    }
}
