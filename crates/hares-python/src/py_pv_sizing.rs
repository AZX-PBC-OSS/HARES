//! Python bindings for PV sizing and roof plane introspection.

use hares_physics::pv_sizing::{self, PvCandidate, PvSizingResult, RoofInfo, RoofPlane, RoofShape};
use pyo3::prelude::*;

/// Roof shape classification — determines usable-area fraction.
#[pyclass(name = "RoofShape", eq, eq_int, skip_from_py_object)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PyRoofShape {
    Gable,
    Hip,
    Flat,
}

impl From<RoofShape> for PyRoofShape {
    fn from(s: RoofShape) -> Self {
        match s {
            RoofShape::Gable => PyRoofShape::Gable,
            RoofShape::Hip => PyRoofShape::Hip,
            RoofShape::Flat => PyRoofShape::Flat,
        }
    }
}

/// A single roof plane from the parsed HPXML building.
#[pyclass(frozen, name = "RoofPlane", skip_from_py_object)]
#[derive(Debug, Clone)]
pub struct PyRoofPlane {
    inner: RoofPlane,
    idx: usize,
}

#[pymethods]
impl PyRoofPlane {
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

    fn __repr__(&self) -> String {
        format!(
            "PvSizingResult(capacity={:.2} kW, panels={}, azimuth={:.0}°, tilt={:.1}°, max_roof={:.1} kW)",
            self.inner.capacity_kw,
            self.inner.num_panels,
            self.inner.array_azimuth_deg,
            self.inner.array_tilt_deg,
            self.inner.max_roof_capacity_kw,
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

pub(crate) fn pv_candidates_from_dwelling(
    roof_info: &RoofInfo,
    roof_shape: RoofShape,
    wall_azimuths: &[f64],
    latitude: Option<f64>,
    diffuse_fraction: Option<f64>,
) -> Vec<PyPvCandidate> {
    pv_sizing::enumerate_pv_candidates(
        roof_info,
        roof_shape,
        wall_azimuths,
        latitude,
        None,
        None,
        diffuse_fraction,
    )
    .into_iter()
    .map(|c| PyPvCandidate { inner: c })
    .collect()
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
) -> Result<PyPvSizingResult, String> {
    let usable = pv_sizing::compute_usable_area(
        roof_info,
        roof_shape,
        wall_azimuths,
        latitude,
        None,
        None,
        diffuse_fraction,
    )
    .map_err(|e| e.to_string())?;
    let result = pv_sizing::size_pv_system(&usable, target_kw, min_kw, max_kw, None, None, None)
        .map_err(|e| e.to_string())?;
    Ok(PyPvSizingResult { inner: result })
}
