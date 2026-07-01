//! Python bindings for PV sizing and roof plane introspection.

use std::collections::HashMap;

use hares_io::PvPanelDefaults;
use hares_physics::pv_sizing::{
    self, PvCandidate, PvSizingError, PvSizingResult, RoofInfo, RoofPlane, RoofShape,
    UsableRoofArea,
};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

/// Roof shape classification — determines usable-area fraction.
#[pyclass(name = "RoofShape", eq, eq_int, from_py_object)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PyRoofShape {
    Gable,
    Hip,
    Flat,
    FlatEastWest,
}

impl From<RoofShape> for PyRoofShape {
    fn from(s: RoofShape) -> Self {
        match s {
            RoofShape::Gable => PyRoofShape::Gable,
            RoofShape::Hip => PyRoofShape::Hip,
            RoofShape::Flat => PyRoofShape::Flat,
            RoofShape::FlatEastWest => PyRoofShape::FlatEastWest,
        }
    }
}

impl From<PyRoofShape> for RoofShape {
    fn from(s: PyRoofShape) -> Self {
        match s {
            PyRoofShape::Gable => RoofShape::Gable,
            PyRoofShape::Hip => RoofShape::Hip,
            PyRoofShape::Flat => RoofShape::Flat,
            PyRoofShape::FlatEastWest => RoofShape::FlatEastWest,
        }
    }
}

/// A single roof plane — constructable directly from Python.
#[pyclass(frozen, name = "RoofPlane", from_py_object)]
#[derive(Debug, Clone)]
pub struct PyRoofPlane {
    inner: RoofPlane,
    idx: usize,
}

#[pymethods]
impl PyRoofPlane {
    /// Construct a roof plane with area, tilt, and optional azimuth.
    #[new]
    #[pyo3(signature = (area_m2, tilt_deg=0.0, azimuth_deg=None, material=None, boundary_index=None, index=0))]
    fn new(
        area_m2: f64,
        tilt_deg: f64,
        azimuth_deg: Option<f64>,
        material: Option<String>,
        boundary_index: Option<u32>,
        index: usize,
    ) -> Self {
        Self {
            inner: RoofPlane {
                area_m2,
                tilt_deg,
                azimuth_deg,
                material,
                boundary_index,
            },
            idx: index,
        }
    }

    #[getter]
    fn index(&self) -> usize {
        self.idx
    }

    /// Area in square meters.
    #[getter]
    fn area_m2(&self) -> f64 {
        self.inner.area_m2
    }

    /// Tilt from horizontal in degrees (0 = flat, 90 = vertical).
    #[getter]
    fn tilt_deg(&self) -> f64 {
        self.inner.tilt_deg
    }

    /// Compass azimuth in degrees (0 = north, 180 = south), or None.
    #[getter]
    fn azimuth_deg(&self) -> Option<f64> {
        self.inner.azimuth_deg
    }

    /// Roofing material / finish type, or None.
    #[getter]
    fn material(&self) -> Option<String> {
        self.inner.material.clone()
    }

    /// Index into Building.boundaries for this roof surface. Pass as
    /// `attached_boundary_id` when creating PV equipment from this plane.
    #[getter]
    fn boundary_index(&self) -> Option<u32> {
        self.inner.boundary_index
    }

    fn __repr__(&self) -> String {
        format!(
            "RoofPlane(index={}, area_m2={:.1}, tilt_deg={:.1}, azimuth_deg={:?})",
            self.idx, self.inner.area_m2, self.inner.tilt_deg, self.inner.azimuth_deg,
        )
    }
}

/// A candidate PV placement on a roof plane.
#[pyclass(frozen, name = "PvCandidate", skip_from_py_object)]
#[derive(Debug, Clone)]
pub struct PyPvCandidate {
    inner: PvCandidate,
}

#[pymethods]
impl PyPvCandidate {
    /// Index of the roof plane.
    #[getter]
    fn plane_idx(&self) -> usize {
        self.inner.plane_idx
    }

    /// Array azimuth in degrees (0 = north, 180 = south).
    #[getter]
    fn azimuth_deg(&self) -> f64 {
        self.inner.azimuth_deg
    }

    /// Array tilt in degrees.
    #[getter]
    fn tilt_deg(&self) -> f64 {
        self.inner.tilt_deg
    }

    /// Usable roof area in square meters.
    #[getter]
    fn usable_m2(&self) -> f64 {
        self.inner.usable_m2
    }

    /// Maximum number of panels that fit.
    #[getter]
    fn max_panels(&self) -> u32 {
        self.inner.max_panels
    }

    /// Maximum DC capacity in kW.
    #[getter]
    fn max_capacity_kw(&self) -> f64 {
        self.inner.max_capacity_kw
    }

    /// Solar production score (higher = better).
    #[getter]
    fn solar_score(&self) -> f64 {
        self.inner.solar_score
    }

    /// Roof shape classification for this candidate.
    #[getter]
    fn roof_shape(&self) -> PyRoofShape {
        self.inner.roof_shape.into()
    }

    /// Index into Building.boundaries for the source roof surface.
    /// Pass as `attached_boundary_id` when creating PV equipment.
    #[getter]
    fn boundary_index(&self) -> Option<u32> {
        self.inner.boundary_index
    }

    fn __repr__(&self) -> String {
        format!(
            "PvCandidate(plane_idx={}, azimuth={:.0}°, tilt={:.1}°, max={:.1} kW, panels={})",
            self.inner.plane_idx,
            self.inner.azimuth_deg,
            self.inner.tilt_deg,
            self.inner.max_capacity_kw,
            self.inner.max_panels,
        )
    }
}

/// PV system sizing result.
#[pyclass(frozen, name = "PvSizingResult", skip_from_py_object)]
#[derive(Debug, Clone)]
pub struct PyPvSizingResult {
    inner: PvSizingResult,
}

#[pymethods]
impl PyPvSizingResult {
    #[getter]
    fn capacity_kw(&self) -> f64 {
        self.inner.capacity_kw
    }

    #[getter]
    fn num_panels(&self) -> u32 {
        self.inner.num_panels
    }

    #[getter]
    fn collector_area_m2(&self) -> f64 {
        self.inner.collector_area_m2
    }

    #[getter]
    fn array_azimuth_deg(&self) -> f64 {
        self.inner.array_azimuth_deg
    }

    #[getter]
    fn array_tilt_deg(&self) -> f64 {
        self.inner.array_tilt_deg
    }

    #[getter]
    fn system_losses_fraction(&self) -> f64 {
        self.inner.system_losses_fraction
    }

    #[getter]
    fn max_roof_capacity_kw(&self) -> f64 {
        self.inner.max_roof_capacity_kw
    }

    #[getter]
    fn panel_watts(&self) -> u32 {
        self.inner.panel_watts
    }

    #[getter]
    fn electrical_constraint_binding(&self) -> bool {
        self.inner.electrical_constraint_binding
    }

    #[getter]
    fn max_ac_kw(&self) -> Option<f64> {
        self.inner.max_ac_kw
    }

    #[getter]
    fn max_backfeed_amps(&self) -> Option<f64> {
        self.inner.max_backfeed_amps
    }

    fn __repr__(&self) -> String {
        let mut s = format!(
            "PvSizingResult(capacity={:.2} kW, panels={}, azimuth={:.0}°, tilt={:.1}°, max_roof={:.1} kW",
            self.inner.capacity_kw,
            self.inner.num_panels,
            self.inner.array_azimuth_deg,
            self.inner.array_tilt_deg,
            self.inner.max_roof_capacity_kw,
        );
        if self.inner.electrical_constraint_binding {
            s.push_str(&format!(
                ", elec_constraint=bind, max_ac={:.1} kW",
                self.inner.max_ac_kw.unwrap_or(0.0),
            ));
        } else if self.inner.max_ac_kw.is_some() {
            s.push_str(&format!(
                ", elec_constraint=ok, max_ac={:.1} kW",
                self.inner.max_ac_kw.unwrap_or(0.0),
            ));
        }
        s.push(')');
        s
    }
}

/// Result of usable roof area computation from `compute_usable_area`.
#[pyclass(frozen, name = "UsableRoofArea", skip_from_py_object)]
#[derive(Debug, Clone)]
pub struct PyUsableRoofArea {
    inner: UsableRoofArea,
}

#[pymethods]
impl PyUsableRoofArea {
    /// Index of the best roof plane in the supplied plane list.
    #[getter]
    fn best_plane_idx(&self) -> usize {
        self.inner.best_plane_idx
    }

    /// Usable area in square meters.
    #[getter]
    fn usable_m2(&self) -> f64 {
        self.inner.usable_m2
    }

    /// Maximum number of panels that fit.
    #[getter]
    fn max_panels(&self) -> u32 {
        self.inner.max_panels
    }

    /// Maximum DC capacity in kW.
    #[getter]
    fn max_capacity_kw(&self) -> f64 {
        self.inner.max_capacity_kw
    }

    /// Roof shape classification used.
    #[getter]
    fn roof_shape(&self) -> PyRoofShape {
        self.inner.roof_shape.into()
    }

    /// Array azimuth in degrees (0 = north, 180 = south).
    #[getter]
    fn azimuth_deg(&self) -> f64 {
        self.inner.azimuth_deg
    }

    /// Array tilt in degrees.
    #[getter]
    fn tilt_deg(&self) -> f64 {
        self.inner.tilt_deg
    }

    fn __repr__(&self) -> String {
        format!(
            "UsableRoofArea(usable_m2={:.1}, max={:.1} kW, panels={}, azimuth={:.0}°, tilt={:.1}°)",
            self.inner.usable_m2,
            self.inner.max_capacity_kw,
            self.inner.max_panels,
            self.inner.azimuth_deg,
            self.inner.tilt_deg,
        )
    }
}

// ---------------------------------------------------------------------------
// Standalone pyfunctions — callable directly from Python
// ---------------------------------------------------------------------------

/// Compute the annual-average diffuse-to-global ratio (Kd = ΣDHI / ΣGHI)
/// from hourly weather data.
///
/// Filters out nighttime hours (GHI = 0) to avoid division instabilities.
/// Falls back to the continental-US default (0.18) when data is empty or
/// all GHI values are zero.
///
/// Args:
///     ghi: Global Horizontal Irradiance values (W/m²). Typically 8760
///          hourly values from a TMY3/EPW weather file.
///     dhi: Diffuse Horizontal Irradiance values (W/m²), same length.
///
/// Returns:
///     Kd fraction in [0, 1].
///
/// Example:
///     >>> weather = hares.parse_weather("TMY3.epw")
///     >>> kd = hares.compute_annual_diffuse_fraction(
///     ...     weather.ghi_w_m2, weather.dhi_w_m2
///     ... )
#[pyfunction]
pub fn compute_annual_diffuse_fraction(ghi: Vec<f64>, dhi: Vec<f64>) -> f64 {
    hares_physics::pv_sizing::compute_annual_diffuse_fraction(&ghi, &dhi)
}

/// Annual-average diffuse-to-global ratio (Kd) fallback for the continental US.
///
/// Returns 0.18 — the mid-range default derived from NREL NSRDB TMY3
/// annual-average DHI/GHI across 32 US reference stations
/// (Phoenix ≈ 0.12, Seattle ≈ 0.25).
#[pyfunction]
pub fn default_diffuse_fraction() -> f64 {
    hares_physics::pv_sizing::DEFAULT_DIFFUSE_FRACTION
}

/// True if the azimuth faces roughly north (within ±45° of 0°/360°).
///
/// Args:
///     azimuth_deg: Compass azimuth in degrees (0 = north, 180 = south).
#[pyfunction]
pub fn is_north_facing(azimuth_deg: f64) -> bool {
    hares_physics::pv_sizing::is_north_facing(azimuth_deg)
}

// ---------------------------------------------------------------------------
// Standalone pyfunctions for roof analysis — callable without a Dwelling
// ---------------------------------------------------------------------------

fn roof_info_from_py_planes(planes: &[PyRoofPlane]) -> RoofInfo {
    let total = planes.iter().map(|p| p.inner.area_m2).sum();
    RoofInfo {
        planes: planes.iter().map(|p| p.inner.clone()).collect(),
        total_roof_area_m2: total,
    }
}

/// Infer roof shape from building metadata without requiring a Dwelling.
///
/// Args:
///     roof_planes: List of ``RoofPlane`` objects describing the roof.
///     facility_type: Optional HPXML ResidentialFacilityType string
///         (e.g. ``"single-family detached"``, ``"apartment"``).
///         ``"apartment"`` or ``"5+"`` keywords force ``Flat``.
///     latitude: Optional decimal degrees for latitude-dependent classification.
///     wall_azimuths: Optional fallback orientations when roof planes lack
///         explicit azimuth.
///
/// Returns:
///     ``RoofShape`` — one of ``Gable``, ``Hip``, or ``Flat``
///     (``FlatEastWest`` is never inferred; it is only available via user override).
#[pyfunction]
#[pyo3(signature = (roof_planes, facility_type=None, latitude=None, wall_azimuths=None))]
pub fn infer_roof_shape(
    roof_planes: Vec<PyRoofPlane>,
    facility_type: Option<String>,
    latitude: Option<f64>,
    wall_azimuths: Option<Vec<f64>>,
) -> PyRoofShape {
    let roof = roof_info_from_py_planes(&roof_planes);
    let azimuths: Vec<f64> = wall_azimuths.unwrap_or_default();
    let result = hares_physics::pv_sizing::infer_roof_shape(
        &roof,
        facility_type.as_deref(),
        latitude,
        &azimuths,
    );
    #[cfg(feature = "observe")]
    tracing::info!(
        target: "hares.observe.pv_sizing.infer_roof_shape",
        latitude = ?latitude,
        facility_type = ?facility_type,
        roof_plane_count = roof_planes.len(),
        roof_shape = ?result,
        "infer_roof_shape called"
    );
    result.into()
}

/// Compute usable roof area from roof planes and shape classification.
///
/// Identifies the single best roof plane (by solar production score) and
/// returns the usable area, maximum panel count, and capacity. This is the
/// first stage of PV sizing — pipe the result into ``size_pv_system()`` to
/// get the final sized array.
///
/// Args:
///     roof_planes: List of ``RoofPlane`` objects.
///     roof_shape: Roof shape classification (``RoofShape``). Use
///         ``infer_roof_shape()`` to infer from building metadata, or
///         pass a user override.
///     wall_azimuths: Optional fallback orientations.
///     latitude: Optional decimal degrees for scoring and tilt selection.
///     panel_watts: Optional panel wattage override (W). Default: 440.
///     panel_area_m2: Optional panel area override (m²). Default: 2.1.
///     diffuse_fraction: Optional annual-average DHI/GHI ratio. ``None``
///         falls back to the NREL PVWatts empirical model.
///     roof_shape_user_override: Whether ``roof_shape`` was user-specified
///         (affects aspect ratio selection for east-west placement).
///
/// Returns:
///     ``UsableRoofArea`` on success, or ``ValueError`` if no viable planes exist.
#[pyfunction]
#[pyo3(signature = (roof_planes, roof_shape, *, wall_azimuths=None, latitude=None,
    panel_watts=None, panel_area_m2=None, diffuse_fraction=None, roof_shape_user_override=false))]
// Why: the parameter count reflects the complete set of tunable PV sizing
// inputs; constructing a builder/params type would add indirection for no
// benefit at this binding layer.
#[allow(clippy::too_many_arguments)]
pub fn compute_usable_area(
    roof_planes: Vec<PyRoofPlane>,
    roof_shape: PyRoofShape,
    wall_azimuths: Option<Vec<f64>>,
    latitude: Option<f64>,
    panel_watts: Option<u32>,
    panel_area_m2: Option<f64>,
    diffuse_fraction: Option<f64>,
    roof_shape_user_override: bool,
) -> PyResult<PyUsableRoofArea> {
    let roof = roof_info_from_py_planes(&roof_planes);
    let azimuths: Vec<f64> = wall_azimuths.unwrap_or_default();
    let result = hares_physics::pv_sizing::compute_usable_area(
        &roof,
        roof_shape.into(),
        &azimuths,
        latitude,
        panel_watts,
        panel_area_m2,
        diffuse_fraction,
        roof_shape_user_override,
    )
    .map_err(|e: PvSizingError| PyValueError::new_err(e.to_string()))?;
    Ok(PyUsableRoofArea { inner: result })
}

/// Enumerate all viable PV placement candidates from roof planes.
///
/// Returns one candidate per non-north-facing roof plane, sorted by solar
/// production score (best first). Use to inspect all placement options
/// before selecting one for ``size_pv_system()``.
///
/// Args:
///     roof_planes: List of ``RoofPlane`` objects.
///     roof_shape: Roof shape classification.
///     wall_azimuths: Optional fallback orientations.
///     latitude: Optional decimal degrees.
///     panel_watts: Optional panel wattage override (default 440 W).
///     panel_area_m2: Optional panel area override (default 2.1 m²).
///     diffuse_fraction: Optional annual-average DHI/GHI ratio.
///     roof_shape_user_override: Whether roof_shape was user-specified.
///
/// Returns:
///     List of ``PvCandidate`` objects, sorted by ``solar_score`` (best first).
///     Empty list if no viable candidates exist.
#[pyfunction]
#[pyo3(signature = (roof_planes, roof_shape, *, wall_azimuths=None, latitude=None,
    panel_watts=None, panel_area_m2=None, diffuse_fraction=None, roof_shape_user_override=false))]
// Why: the parameter count reflects the complete set of tunable PV sizing
// inputs; constructing a builder/params type would add indirection for no
// benefit at this binding layer.
#[allow(clippy::too_many_arguments)]
pub fn enumerate_pv_candidates(
    roof_planes: Vec<PyRoofPlane>,
    roof_shape: PyRoofShape,
    wall_azimuths: Option<Vec<f64>>,
    latitude: Option<f64>,
    panel_watts: Option<u32>,
    panel_area_m2: Option<f64>,
    diffuse_fraction: Option<f64>,
    roof_shape_user_override: bool,
) -> PyResult<Vec<PyPvCandidate>> {
    let roof = roof_info_from_py_planes(&roof_planes);
    let azimuths: Vec<f64> = wall_azimuths.unwrap_or_default();
    let result = hares_physics::pv_sizing::enumerate_pv_candidates(
        &roof,
        roof_shape.into(),
        &azimuths,
        latitude,
        panel_watts,
        panel_area_m2,
        diffuse_fraction,
        roof_shape_user_override,
    )
    .map_err(|e: PvSizingError| PyValueError::new_err(e.to_string()))?;
    Ok(result
        .into_iter()
        .map(|c| PyPvCandidate { inner: c })
        .collect())
}

/// Size a PV system from a usable roof area result.
///
/// Takes the output of ``compute_usable_area()`` and sizes a PV array to
/// the target capacity, respecting roof geometry limits and optional
/// inverter-side DC capacity clamp.
///
/// Args:
///     usable: ``UsableRoofArea`` from ``compute_usable_area()``.
///     target_kw: Desired PV capacity in kW.
///     min_kw: Minimum acceptable capacity (default 2.0).
///     max_kw: Maximum capacity limit (default 14.0).
///     system_losses: Optional system losses fraction (0–1, default 0.14).
///     panel_watts: Optional panel wattage override (default 440 W).
///     panel_area_m2: Optional panel area override (default 2.1 m²).
///     inverter_kw_ac: Optional inverter AC power rating for DC-side clamp.
///         Must pair with ``max_dc_ac_ratio`` to activate.
///     max_dc_ac_ratio: Optional maximum DC:AC oversizing ratio.
///         Typical residential values: 1.1–1.3 (NREL SAM default = 1.2).
///     main_panel_ampacity: Optional main electrical panel ampacity (amps).
///         When provided, the NEC 120% busbar backfeed rule is applied to
///         constrain the system to electrically-safe backfeed limits.
///         For example, a 100 A panel limits backfeed to ~4.8 kW AC;
///         a 200 A panel allows up to ~9.6 kW AC.
///     main_breaker_ampacity: Optional main breaker ampacity (amps). When
///         ``None`` (default), the main breaker is assumed to equal the
///         main panel ampacity. Override this for non-typical
///         configurations (e.g. 200 A panel with a 150 A main breaker).
///
/// Returns:
///     ``PvSizingResult`` on success, or ``ValueError`` if roof capacity
///     is below the minimum.
#[pyfunction]
#[pyo3(signature = (usable, target_kw, *, min_kw=2.0, max_kw=14.0,
    system_losses=None, panel_watts=None, panel_area_m2=None,
    inverter_kw_ac=None, max_dc_ac_ratio=None, main_panel_ampacity=None,
    main_breaker_ampacity=None))]
// Why: the parameter count reflects the complete set of tunable PV sizing
// inputs; constructing a builder/params type would add indirection for no
// benefit at this binding layer.
#[allow(clippy::too_many_arguments)]
pub fn size_pv_system(
    usable: &PyUsableRoofArea,
    target_kw: f64,
    min_kw: f64,
    max_kw: f64,
    system_losses: Option<f64>,
    panel_watts: Option<u32>,
    panel_area_m2: Option<f64>,
    inverter_kw_ac: Option<f64>,
    max_dc_ac_ratio: Option<f64>,
    main_panel_ampacity: Option<u32>,
    main_breaker_ampacity: Option<u32>,
) -> PyResult<PyPvSizingResult> {
    let result = hares_physics::pv_sizing::size_pv_system(
        &usable.inner,
        target_kw,
        min_kw,
        max_kw,
        system_losses,
        panel_watts,
        panel_area_m2,
        inverter_kw_ac,
        max_dc_ac_ratio,
        main_panel_ampacity,
        main_breaker_ampacity,
    )
    .map_err(|e: PvSizingError| PyValueError::new_err(e.to_string()))?;
    Ok(PyPvSizingResult { inner: result })
}

/// Compute the minimum main panel ampacity required to legally accommodate
/// a given PV system AC output under the NEC 120% busbar backfeed rule.
///
/// This is the inverse of the electrical constraint in ``size_pv_system``:
/// rather than taking a panel ampacity and clamping the PV size, it takes
/// a target AC-side output and returns the smallest standard residential
/// main panel rating that can support it.
///
/// Args:
///     target_ac_kw: Desired AC-side PV output in kW.
///     main_breaker_amps: Optional main breaker ampacity. When ``None``
///         (default), the main breaker is assumed to match the panel
///         ampacity (the standard residential configuration). Override
///         for non-standard configurations (e.g. 200 A panel with
///         150 A main breaker).
///
/// Returns:
///     The minimum standard panel ampacity (100, 125, 150, 200, 225, or
///     400 A) that satisfies the NEC 120% backfeed rule. If the required
///     ampacity exceeds 400 A, returns 400 A (caller should evaluate
///     whether the installation is feasible).
///
/// NEC 705.12(B)(2)(3)(b) (NFPA 70, NEC 2023).
#[pyfunction]
#[pyo3(signature = (target_ac_kw, *, main_breaker_amps=None))]
pub fn required_main_panel_ampacity(target_ac_kw: f64, main_breaker_amps: Option<u32>) -> u32 {
    hares_physics::pv_sizing::required_main_panel_ampacity(target_ac_kw, main_breaker_amps)
}

// ---------------------------------------------------------------------------
// Helpers called from py_dwelling
// ---------------------------------------------------------------------------

pub(crate) fn roof_planes_from_dwelling(roof_info: &RoofInfo) -> Vec<PyRoofPlane> {
    roof_info
        .planes
        .iter()
        .enumerate()
        .map(|(i, plane)| PyRoofPlane {
            inner: plane.clone(),
            idx: i,
        })
        .collect()
}

/// Resolve panel parameters from the defaults store when all three are None.
///
/// When the caller provides no explicit panel overrides, consult the
/// `PvPanelDefaults` store loaded from `defaults/pv/*.toml`. Uses the
/// lexicographically first key for deterministic behaviour regardless of
/// HashMap iteration order. If the store is empty, None propagates to the
/// physics layer where compile-time constants apply.
fn resolve_panel_defaults(
    panel_watts: Option<u32>,
    panel_area_m2: Option<f64>,
    system_losses: Option<f64>,
    store: &HashMap<String, PvPanelDefaults>,
) -> (Option<u32>, Option<f64>, Option<f64>) {
    if panel_watts.is_none() && panel_area_m2.is_none() && system_losses.is_none() {
        if let Some(spec) = store.keys().min().and_then(|k| store.get(k)) {
            return (
                Some(spec.panel_watts),
                Some(spec.panel_area_m2),
                Some(spec.system_losses_fraction),
            );
        }
        // Store is empty — compile-time constants apply downstream.
        tracing::warn!(
            "pv_panel_defaults store is empty; falling back to compile-time constants \
             (440 W / 2.1 m²). Load defaults/pv/ to configure panel specs."
        );
    }
    (panel_watts, panel_area_m2, system_losses)
}

/// Internal helper – mirrors the full Rust API surface for PV candidate
/// enumeration including all optional parameters.
// Why: the parameter count reflects the complete set of tunable PV sizing
// inputs; constructing a builder/params type would add indirection for no
// benefit at this binding layer.
#[allow(clippy::too_many_arguments)]
pub(crate) fn pv_candidates_from_dwelling(
    roof_info: &RoofInfo,
    roof_shape: RoofShape,
    wall_azimuths: &[f64],
    latitude: Option<f64>,
    diffuse_fraction: Option<f64>,
    panel_watts: Option<u32>,
    panel_area_m2: Option<f64>,
    pv_panel_defaults: &HashMap<String, PvPanelDefaults>,
    roof_shape_user_override: bool,
) -> PyResult<Vec<PyPvCandidate>> {
    let (panel_watts, panel_area_m2, _) =
        resolve_panel_defaults(panel_watts, panel_area_m2, None, pv_panel_defaults);
    let result = pv_sizing::enumerate_pv_candidates(
        roof_info,
        roof_shape,
        wall_azimuths,
        latitude,
        panel_watts,
        panel_area_m2,
        diffuse_fraction,
        roof_shape_user_override,
    )
    .map_err(|e: PvSizingError| PyValueError::new_err(e.to_string()))?;
    Ok(result
        .into_iter()
        .map(|c| PyPvCandidate { inner: c })
        .collect())
}

/// Internal helper – mirrors the full Rust API surface for PV sizing
/// including all optional parameters. The argument count reflects the
/// complete set of tunable inputs; constructing a builder/params type
/// would add indirection for no benefit at this binding layer.
#[allow(clippy::too_many_arguments)]
pub(crate) fn size_pv_from_dwelling(
    roof_info: &RoofInfo,
    roof_shape: RoofShape,
    wall_azimuths: &[f64],
    latitude: Option<f64>,
    target_kw: f64,
    min_kw: f64,
    max_kw: f64,
    diffuse_fraction: Option<f64>,
    panel_watts: Option<u32>,
    panel_area_m2: Option<f64>,
    system_losses: Option<f64>,
    inverter_kw_ac: Option<f64>,
    max_dc_ac_ratio: Option<f64>,
    main_panel_ampacity: Option<u32>,
    main_breaker_ampacity: Option<u32>,
    pv_panel_defaults: &HashMap<String, PvPanelDefaults>,
    roof_shape_user_override: bool,
) -> Result<PyPvSizingResult, String> {
    let (panel_watts, panel_area_m2, system_losses) =
        resolve_panel_defaults(panel_watts, panel_area_m2, system_losses, pv_panel_defaults);
    let usable = pv_sizing::compute_usable_area(
        roof_info,
        roof_shape,
        wall_azimuths,
        latitude,
        panel_watts,
        panel_area_m2,
        diffuse_fraction,
        roof_shape_user_override,
    )
    .map_err(|e| e.to_string())?;
    let result = pv_sizing::size_pv_system(
        &usable,
        target_kw,
        min_kw,
        max_kw,
        system_losses,
        panel_watts,
        panel_area_m2,
        inverter_kw_ac,
        max_dc_ac_ratio,
        main_panel_ampacity,
        main_breaker_ampacity,
    )
    .map_err(|e| e.to_string())?;
    Ok(PyPvSizingResult { inner: result })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_roof() -> RoofInfo {
        RoofInfo {
            planes: vec![RoofPlane {
                area_m2: 100.0,
                tilt_deg: 26.0,
                azimuth_deg: Some(180.0),
                material: None,
                boundary_index: None,
            }],
            total_roof_area_m2: 100.0,
        }
    }

    #[test]
    fn size_pv_from_dwelling_respects_panel_overrides() {
        let roof = make_roof();

        // Default path (no overrides, no store) — uses compile-time 440W / 2.1 m².
        let result_default = size_pv_from_dwelling(
            &roof,
            RoofShape::Gable,
            &[],
            Some(40.0),
            5.0,
            2.0,
            14.0,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            &HashMap::new(),
            false,
        )
        .expect("default sizing");

        // Custom 300W / 1.6 m² panel — must produce different (lower) capacity.
        let result_custom = size_pv_from_dwelling(
            &roof,
            RoofShape::Gable,
            &[],
            Some(40.0),
            5.0,
            2.0,
            14.0,
            None,
            Some(300),
            Some(1.6),
            Some(0.14),
            None,
            None,
            None,
            None,
            &HashMap::new(),
            false,
        )
        .expect("custom sizing");

        assert_ne!(
            result_default.inner.panel_watts, result_custom.inner.panel_watts,
            "custom panel_watts override must differ from default"
        );
        assert!(
            result_default.inner.panel_watts > result_custom.inner.panel_watts,
            "default 440W panel must have higher wattage than 300W override"
        );
    }

    #[test]
    fn pv_candidates_respects_panel_overrides() {
        let roof = make_roof();

        let candidates_default = pv_candidates_from_dwelling(
            &roof,
            RoofShape::Gable,
            &[],
            Some(40.0),
            None,
            None,
            None,
            &HashMap::new(),
            false,
        )
        .unwrap();
        let candidates_custom = pv_candidates_from_dwelling(
            &roof,
            RoofShape::Gable,
            &[],
            Some(40.0),
            None,
            Some(470),
            Some(2.0),
            &HashMap::new(),
            false,
        )
        .unwrap();

        assert_eq!(candidates_default.len(), candidates_custom.len());
        // max_capacity_kw should differ because panel wattage differs.
        let def_cap = candidates_default[0].inner.max_capacity_kw;
        let cust_cap = candidates_custom[0].inner.max_capacity_kw;
        assert_ne!(
            def_cap, cust_cap,
            "custom panel (470W / 2.0 m²) must produce different capacity than default (440W / 2.0 m²)"
        );
        assert!(
            cust_cap > def_cap,
            "custom 470W panel should yield higher capacity than 440W default"
        );
    }

    #[test]
    fn store_consults_defaults_when_all_params_none() {
        let roof = make_roof();
        let mut store = HashMap::new();
        store.insert(
            "300w_panel".to_string(),
            PvPanelDefaults {
                name: "300W Test Panel".to_string(),
                panel_watts: 300,
                panel_area_m2: 1.6,
                noct_c: 45.0,
                module_type: "standard".to_string(),
                system_losses_fraction: 0.14,
            },
        );

        let candidates = pv_candidates_from_dwelling(
            &roof,
            RoofShape::Gable,
            &[],
            Some(40.0),
            None,
            None,
            None,
            &store,
            false,
        )
        .unwrap();

        // When all panel params are None, the store should be consulted.
        // A 300W/1.6m² panel produces a different max_capacity_kw than the
        // compile-time 440W/2.1m² default.
        assert_eq!(candidates.len(), 1);
        let def_candidates = pv_candidates_from_dwelling(
            &roof,
            RoofShape::Gable,
            &[],
            Some(40.0),
            None,
            None,
            None,
            // Empty store → compile-time defaults (440W / 2.1 m²)
            &HashMap::new(),
            false,
        )
        .unwrap();
        assert_ne!(
            candidates[0].inner.max_capacity_kw, def_candidates[0].inner.max_capacity_kw,
            "store-supplied 300W panel must differ from compile-time 440W default"
        );
        assert!(
            def_candidates[0].inner.max_capacity_kw > candidates[0].inner.max_capacity_kw,
            "440W default should yield higher capacity than 300W store panel"
        );
    }

    #[test]
    fn size_pv_inverter_clamp_through_binding() {
        let roof = make_roof();

        // No inverter constraint
        let result_no_inv = size_pv_from_dwelling(
            &roof,
            RoofShape::Gable,
            &[],
            Some(40.0),
            8.0,
            2.0,
            14.0,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            &HashMap::new(),
            false,
        )
        .expect("no inverter");

        // With inverter: 5.0 kW AC × 1.2 ratio → max 6.0 kW DC
        let result_with_inv = size_pv_from_dwelling(
            &roof,
            RoofShape::Gable,
            &[],
            Some(40.0),
            8.0,
            2.0,
            14.0,
            None,
            None,
            None,
            None,
            Some(5.0),
            Some(1.2),
            None,
            None,
            &HashMap::new(),
            false,
        )
        .expect("with inverter");

        // Inverter-limited capacity must be ≤ unconstrained capacity.
        assert!(
            result_with_inv.inner.capacity_kw <= result_no_inv.inner.capacity_kw,
            "inverter-limited capacity ({:.2}) must be <= unconstrained ({:.2})",
            result_with_inv.inner.capacity_kw,
            result_no_inv.inner.capacity_kw
        );
        // With 5.0 kW AC × 1.2 = 6.0 kW DC max, target 8 kW must be clamped.
        let max_dc = 5.0 * 1.2;
        assert!(
            result_with_inv.inner.capacity_kw <= max_dc + 0.5,
            "inverter-limited capacity {:.2} should approximate max DC {:.2}",
            result_with_inv.inner.capacity_kw,
            max_dc
        );
    }

    // -------------------------------------------------------------------
    // Standalone pyfunction tests — callable without a Dwelling
    // -------------------------------------------------------------------

    #[test]
    fn infer_roof_shape_from_gable_planes() {
        // Two opposing planes at latitude 40°N, 26° tilt → Gable.
        let planes = vec![
            PyRoofPlane {
                inner: RoofPlane {
                    area_m2: 100.0,
                    tilt_deg: 26.0,
                    azimuth_deg: Some(180.0),
                    material: None,
                    boundary_index: None,
                },
                idx: 0,
            },
            PyRoofPlane {
                inner: RoofPlane {
                    area_m2: 100.0,
                    tilt_deg: 26.0,
                    azimuth_deg: Some(0.0),
                    material: None,
                    boundary_index: None,
                },
                idx: 1,
            },
        ];
        let result = infer_roof_shape(planes, None, Some(40.0), None);
        assert_eq!(result, PyRoofShape::Gable);
    }

    #[test]
    fn infer_roof_shape_apartment_returns_flat() {
        let planes = vec![PyRoofPlane {
            inner: RoofPlane {
                area_m2: 100.0,
                tilt_deg: 26.0,
                azimuth_deg: Some(180.0),
                material: None,
                boundary_index: None,
            },
            idx: 0,
        }];
        let result = infer_roof_shape(planes, Some("apartment".to_string()), None, None);
        assert_eq!(result, PyRoofShape::Flat);
    }

    #[test]
    fn infer_roof_shape_all_flat_tilt_returns_flat() {
        let planes = vec![PyRoofPlane {
            inner: RoofPlane {
                area_m2: 50.0,
                tilt_deg: 0.5,
                azimuth_deg: Some(180.0),
                material: None,
                boundary_index: None,
            },
            idx: 0,
        }];
        let result = infer_roof_shape(planes, None, Some(40.0), None);
        assert_eq!(result, PyRoofShape::Flat);
    }

    #[test]
    fn compute_usable_area_pyfunction_works_without_dwelling() {
        let planes = vec![PyRoofPlane {
            inner: RoofPlane {
                area_m2: 100.0,
                tilt_deg: 26.0,
                azimuth_deg: Some(180.0),
                material: None,
                boundary_index: None,
            },
            idx: 0,
        }];
        let result = compute_usable_area(
            planes,
            PyRoofShape::Gable,
            None,
            Some(40.0),
            None,
            None,
            None,
            false,
        )
        .expect("compute_usable_area should succeed for valid roof plane");
        assert!(result.usable_m2() > 0.0);
        assert!(result.max_capacity_kw() > 0.0);
        assert!(result.max_panels() > 0);
        assert_eq!(result.best_plane_idx(), 0);
    }

    #[test]
    fn compute_usable_area_empty_planes_returns_error() {
        let result = compute_usable_area(
            vec![],
            PyRoofShape::Gable,
            None,
            Some(40.0),
            None,
            None,
            None,
            false,
        );
        assert!(result.is_err());
    }

    #[test]
    fn enumerate_candidates_pyfunction_works_without_dwelling() {
        let planes = vec![PyRoofPlane {
            inner: RoofPlane {
                area_m2: 100.0,
                tilt_deg: 26.0,
                azimuth_deg: Some(180.0),
                material: None,
                boundary_index: None,
            },
            idx: 0,
        }];
        let candidates = enumerate_pv_candidates(
            planes,
            PyRoofShape::Gable,
            None,
            Some(40.0),
            None,
            None,
            None,
            false,
        )
        .unwrap();
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].max_capacity_kw() > 0.0);
    }

    #[test]
    fn enumerate_candidates_all_north_facing_raises_error() {
        let planes = vec![PyRoofPlane {
            inner: RoofPlane {
                area_m2: 100.0,
                tilt_deg: 26.0,
                azimuth_deg: Some(0.0),
                material: None,
                boundary_index: None,
            },
            idx: 0,
        }];
        let result = enumerate_pv_candidates(
            planes,
            PyRoofShape::Gable,
            None,
            Some(40.0),
            None,
            None,
            None,
            false,
        );
        assert!(
            result.is_err(),
            "all-north-facing planes must raise an error, not return empty list"
        );
    }

    #[test]
    fn size_pv_system_chains_with_compute_usable_area() {
        let planes = vec![PyRoofPlane {
            inner: RoofPlane {
                area_m2: 100.0,
                tilt_deg: 26.0,
                azimuth_deg: Some(180.0),
                material: None,
                boundary_index: None,
            },
            idx: 0,
        }];
        let usable = compute_usable_area(
            planes,
            PyRoofShape::Gable,
            None,
            Some(40.0),
            None,
            None,
            None,
            false,
        )
        .expect("compute should succeed");
        let result = size_pv_system(
            &usable, 6.0, 2.0, 14.0, None, None, None, None, None, None, None,
        )
        .expect("size_pv_system should succeed");
        assert!(result.capacity_kw() > 0.0);
        assert!(result.num_panels() > 0);
    }

    #[test]
    fn size_pv_system_errors_on_insufficient_roof() {
        let planes = vec![PyRoofPlane {
            inner: RoofPlane {
                area_m2: 5.0,
                tilt_deg: 26.0,
                azimuth_deg: Some(180.0),
                material: None,
                boundary_index: None,
            },
            idx: 0,
        }];
        let usable = compute_usable_area(
            planes,
            PyRoofShape::Gable,
            None,
            Some(40.0),
            None,
            None,
            None,
            false,
        )
        .expect("compute should succeed");
        let result = size_pv_system(
            &usable, 10.0, 10.0, 14.0, None, None, None, None, None, None, None,
        );
        assert!(result.is_err());
    }
}
