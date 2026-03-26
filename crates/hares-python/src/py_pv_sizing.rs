//! Python bindings for PV sizing and roof plane introspection.

use hares_physics::pv_sizing::{self, PvCandidate, PvSizingResult, RoofInfo, RoofPlane, RoofShape};
use pyo3::prelude::*;

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
) -> Vec<PyPvCandidate> {
    pv_sizing::enumerate_pv_candidates(roof_info, roof_shape, wall_azimuths, latitude, None, None)
        .into_iter()
        .map(|c| PyPvCandidate { inner: c })
        .collect()
}

pub(crate) fn size_pv_from_dwelling(
    roof_info: &RoofInfo,
    roof_shape: RoofShape,
    wall_azimuths: &[f64],
    latitude: Option<f64>,
    target_kw: f64,
    min_kw: f64,
    max_kw: f64,
) -> Result<PyPvSizingResult, String> {
    let usable =
        pv_sizing::compute_usable_area(roof_info, roof_shape, wall_azimuths, latitude, None, None)
            .map_err(|e| e.to_string())?;
    let result = pv_sizing::size_pv_system(&usable, target_kw, min_kw, max_kw, None, None, None)
        .map_err(|e| e.to_string())?;
    Ok(PyPvSizingResult { inner: result })
}
