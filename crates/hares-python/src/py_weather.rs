use hares_io::{WeatherTimeSeries, epw, psm3, resstock_csv, tmy3, weather};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyDict};

#[pyclass(name = "WeatherTimeSeries")]
pub struct PyWeatherTimeSeries {
    inner: WeatherTimeSeries,
}

#[pymethods]
impl PyWeatherTimeSeries {
    fn to_polars(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let n = self.inner.dry_bulb_c.len();
        let data = PyDict::new(py);

        let row_index: Vec<u32> = (0..n as u32).collect();
        data.set_item("index", row_index)?;
        data.set_item("dry_bulb_c", &self.inner.dry_bulb_c)?;
        data.set_item("dew_point_c", &self.inner.dew_point_c)?;
        data.set_item("rel_humidity_pct", &self.inner.rel_humidity_pct)?;
        data.set_item("pressure_kpa", &self.inner.pressure_kpa)?;
        data.set_item("ghi_w_m2", &self.inner.ghi_w_m2)?;
        data.set_item("dni_w_m2", &self.inner.dni_w_m2)?;
        data.set_item("dhi_w_m2", &self.inner.dhi_w_m2)?;
        data.set_item("wind_speed_m_s", &self.inner.wind_speed_m_s)?;
        data.set_item("wind_dir_deg", &self.inner.wind_dir_deg)?;
        data.set_item("opaque_sky_cover", &self.inner.opaque_sky_cover)?;
        data.set_item(
            "horizontal_infrared_w_m2",
            &self.inner.horizontal_infrared_w_m2,
        )?;
        data.set_item("sky_temp_c", &self.inner.sky_temp_c)?;
        data.set_item("ground_temp_c", &self.inner.ground_temp_c)?;
        data.set_item("liquid_precip_m", &self.inner.liquid_precip_m)?;

        if let Some(ref albedo) = self.inner.surface_albedo {
            data.set_item("surface_albedo", albedo)?;
        } else {
            let none = py.None();
            data.set_item("surface_albedo", none)?;
        }

        let polars = py.import("polars")?;
        let df = polars.getattr("DataFrame")?.call1((data,))?;
        Ok(df.unbind())
    }

    fn __len__(&self) -> usize {
        self.inner.dry_bulb_c.len()
    }

    fn __repr__(&self) -> String {
        format!(
            "WeatherTimeSeries(n_records={})",
            self.inner.dry_bulb_c.len()
        )
    }

    #[getter]
    fn location(&self) -> String {
        self.inner.meta.location.clone()
    }

    #[getter]
    fn latitude(&self) -> f64 {
        self.inner.meta.latitude
    }

    #[getter]
    fn longitude(&self) -> f64 {
        self.inner.meta.longitude
    }

    #[getter]
    fn elevation_m(&self) -> f64 {
        self.inner.meta.elevation_m
    }

    #[getter]
    fn timezone_offset_h(&self) -> f64 {
        self.inner.meta.timezone_offset_h
    }

    #[getter]
    fn source_step_secs(&self) -> u32 {
        self.inner.meta.source_step_secs
    }

    #[getter]
    fn midpoint_offset_secs(&self) -> u32 {
        self.inner.meta.midpoint_offset_secs
    }

    #[getter]
    fn surface_albedo(&self) -> Option<Vec<f64>> {
        self.inner.surface_albedo.clone()
    }
}

impl PyWeatherTimeSeries {
    pub fn new(inner: WeatherTimeSeries) -> Self {
        Self { inner }
    }
}

fn weather_error_to_py_err(e: weather::WeatherError) -> PyErr {
    PyValueError::new_err(e.to_string())
}

#[pyfunction]
pub fn parse_weather(path: &str) -> PyResult<PyWeatherTimeSeries> {
    weather::parse_weather(path)
        .map(PyWeatherTimeSeries::new)
        .map_err(weather_error_to_py_err)
}

#[pyfunction]
pub fn parse_epw(path: &str) -> PyResult<PyWeatherTimeSeries> {
    epw::parse_epw(path)
        .map(PyWeatherTimeSeries::new)
        .map_err(weather_error_to_py_err)
}

#[pyfunction]
pub fn parse_psm3(path: &str) -> PyResult<PyWeatherTimeSeries> {
    psm3::parse_psm3(path)
        .map(PyWeatherTimeSeries::new)
        .map_err(weather_error_to_py_err)
}

#[pyfunction]
pub fn parse_tmy3(path: &str) -> PyResult<PyWeatherTimeSeries> {
    tmy3::parse_tmy3(path)
        .map(PyWeatherTimeSeries::new)
        .map_err(weather_error_to_py_err)
}

#[pyfunction]
pub fn parse_resstock_csv(
    path: &str,
    elevation_m: f64,
    latitude: f64,
    longitude: f64,
    timezone_offset_h: f64,
) -> PyResult<PyWeatherTimeSeries> {
    resstock_csv::parse_resstock_csv(path, elevation_m, latitude, longitude, timezone_offset_h)
        .map(PyWeatherTimeSeries::new)
        .map_err(weather_error_to_py_err)
}
