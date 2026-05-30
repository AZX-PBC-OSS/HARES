//! Python bindings for PV sizing and roof plane introspection.

use std::collections::HashMap;

use hares_io::PvPanelDefaults;
use hares_physics::pv_sizing::{self, PvCandidate, PvSizingResult, RoofInfo, RoofPlane, RoofShape};
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
) -> Vec<PyPvCandidate> {
    let (panel_watts, panel_area_m2, _) =
        resolve_panel_defaults(panel_watts, panel_area_m2, None, pv_panel_defaults);
    pv_sizing::enumerate_pv_candidates(
        roof_info,
        roof_shape,
        wall_azimuths,
        latitude,
        panel_watts,
        panel_area_m2,
        diffuse_fraction,
        roof_shape_user_override,
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
    panel_watts: Option<u32>,
    panel_area_m2: Option<f64>,
    system_losses: Option<f64>,
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
        );
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
        );

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
        );

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
        );
        assert_ne!(
            candidates[0].inner.max_capacity_kw, def_candidates[0].inner.max_capacity_kw,
            "store-supplied 300W panel must differ from compile-time 440W default"
        );
        assert!(
            def_candidates[0].inner.max_capacity_kw > candidates[0].inner.max_capacity_kw,
            "440W default should yield higher capacity than 300W store panel"
        );
    }
}
