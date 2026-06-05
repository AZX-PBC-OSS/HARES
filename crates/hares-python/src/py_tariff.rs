use hares_tariff::types::{
    DemandRate, ElectricTariff, EnergyRate, ExportMode, ExportRate, FixedCharges, GasTariff,
    GasTieredBlock, RatchetConfig, TieredBlock,
};
use hares_types::{BillingCycle, SeasonFilter, SeasonalSplit, TimeWindow, TouPeriod};
use pyo3::IntoPyObjectExt;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyType};

fn parse_season(s: &str) -> PyResult<SeasonFilter> {
    match s.to_lowercase().as_str() {
        "all" => Ok(SeasonFilter::All),
        "summer" => Ok(SeasonFilter::Summer),
        "winter" => Ok(SeasonFilter::Winter),
        _ => Err(PyValueError::new_err(format!(
            "unknown season '{s}', expected 'all', 'summer', or 'winter'"
        ))),
    }
}

use crate::utils::parse_day_filter as parse_day;

fn parse_window(d: &Bound<'_, PyDict>) -> PyResult<TimeWindow> {
    let day_str: String = d
        .get_item("day")?
        .ok_or_else(|| PyValueError::new_err("window dict missing 'day'"))?
        .extract()?;
    let day = parse_day(&day_str)?;

    let (start_minute, end_minute) = if let Some(sh) = d.get_item("start_hour")? {
        let start_h: f64 = sh.extract()?;
        let end_h: f64 = d
            .get_item("end_hour")?
            .ok_or_else(|| PyValueError::new_err("window has 'start_hour' but missing 'end_hour'"))?
            .extract()?;
        if !(0.0..=24.0).contains(&start_h) {
            return Err(PyValueError::new_err(format!(
                "start_hour {start_h} out of range (must be 0..=24)"
            )));
        }
        if !(0.0..=24.0).contains(&end_h) {
            return Err(PyValueError::new_err(format!(
                "end_hour {end_h} out of range (must be 0..=24; use 24 for midnight end)"
            )));
        }
        ((start_h * 60.0) as u16, (end_h * 60.0) as u16)
    } else {
        let sm: u16 = d
            .get_item("start_minute")?
            .ok_or_else(|| {
                PyValueError::new_err("window dict must have 'start_hour' or 'start_minute'")
            })?
            .extract()?;
        let em: u16 = d
            .get_item("end_minute")?
            .ok_or_else(|| PyValueError::new_err("window dict missing 'end_minute'"))?
            .extract()?;
        (sm, em)
    };

    if start_minute >= 1440 {
        return Err(PyValueError::new_err(format!(
            "start_minute {start_minute} out of range (must be < 1440)"
        )));
    }
    if end_minute > 1440 || end_minute == 0 {
        return Err(PyValueError::new_err(format!(
            "end_minute {end_minute} out of range (must be 1..=1440; use end_hour=24 or end_minute=1440 for midnight)"
        )));
    }
    if start_minute == end_minute {
        return Err(PyValueError::new_err(
            "start_minute and end_minute must differ (zero-width window)",
        ));
    }

    Ok(TimeWindow::new(day, start_minute, end_minute, 0.0))
}

fn json_value_to_py<'py>(py: Python<'py>, v: &serde_json::Value) -> PyResult<Py<pyo3::PyAny>> {
    match v {
        serde_json::Value::Null => Ok(py.None()),
        serde_json::Value::Bool(b) => (*b).into_py_any(py),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.into_py_any(py)
            } else {
                n.as_f64().unwrap_or(0.0).into_py_any(py)
            }
        }
        serde_json::Value::String(s) => s.as_str().into_py_any(py),
        serde_json::Value::Array(arr) => {
            let list = PyList::empty(py);
            for item in arr {
                list.append(json_value_to_py(py, item)?)?;
            }
            Ok(list.into_any().unbind())
        }
        serde_json::Value::Object(map) => {
            let dict = PyDict::new(py);
            for (k, val) in map {
                dict.set_item(k, json_value_to_py(py, val)?)?;
            }
            Ok(dict.into_any().unbind())
        }
    }
}

fn py_to_json_value(obj: &Bound<'_, pyo3::PyAny>) -> PyResult<serde_json::Value> {
    if obj.is_none() {
        return Ok(serde_json::Value::Null);
    }
    if let Ok(b) = obj.extract::<bool>() {
        return Ok(serde_json::Value::Bool(b));
    }
    if let Ok(i) = obj.extract::<i64>() {
        return Ok(serde_json::json!(i));
    }
    if let Ok(f) = obj.extract::<f64>() {
        return Ok(serde_json::json!(f));
    }
    if let Ok(s) = obj.extract::<String>() {
        return Ok(serde_json::Value::String(s));
    }
    if let Ok(list) = obj.cast::<PyList>() {
        let arr: Result<Vec<_>, _> = list.iter().map(|item| py_to_json_value(&item)).collect();
        return Ok(serde_json::Value::Array(arr?));
    }
    if let Ok(dict) = obj.cast::<PyDict>() {
        let mut map = serde_json::Map::new();
        for (k, v) in dict.iter() {
            let key: String = k.extract()?;
            map.insert(key, py_to_json_value(&v)?);
        }
        return Ok(serde_json::Value::Object(map));
    }
    Err(PyValueError::new_err(format!(
        "cannot convert Python object of type '{}' to JSON",
        obj.get_type().name()?
    )))
}

// ---------------------------------------------------------------------------
// PyElectricTariff
// ---------------------------------------------------------------------------

#[pyclass(name = "ElectricTariff", from_py_object)]
#[derive(Debug, Clone)]
pub struct PyElectricTariff {
    pub(crate) inner: ElectricTariff,
}

#[pymethods]
impl PyElectricTariff {
    #[classmethod]
    pub fn from_urdb_json(_cls: &Bound<'_, PyType>, path: &str) -> PyResult<Self> {
        let contents = std::fs::read_to_string(path)
            .map_err(|e| PyValueError::new_err(format!("failed to read '{path}': {e}")))?;
        let tariff = hares_tariff::parse_urdb(&contents)
            .map_err(|e| PyValueError::new_err(format!("URDB parse error: {e}")))?;
        Ok(Self { inner: tariff })
    }

    #[classmethod]
    pub fn from_json(_cls: &Bound<'_, PyType>, json_str: &str) -> PyResult<Self> {
        let tariff: ElectricTariff = serde_json::from_str(json_str)
            .map_err(|e| PyValueError::new_err(format!("JSON parse error: {e}")))?;
        tariff
            .validate()
            .map_err(|e| PyValueError::new_err(format!("tariff validation failed: {e}")))?;
        Ok(Self { inner: tariff })
    }

    #[classmethod]
    pub fn from_dict(_cls: &Bound<'_, PyType>, dict: &Bound<'_, PyDict>) -> PyResult<Self> {
        let val = py_to_json_value(&dict.clone().into_any())?;
        let json_str = serde_json::to_string(&val)
            .map_err(|e| PyValueError::new_err(format!("serialization error: {e}")))?;
        let tariff: ElectricTariff = serde_json::from_str(&json_str)
            .map_err(|e| PyValueError::new_err(format!("deserialization error: {e}")))?;
        tariff
            .validate()
            .map_err(|e| PyValueError::new_err(format!("tariff validation failed: {e}")))?;
        Ok(Self { inner: tariff })
    }

    fn to_dict<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let val = serde_json::to_value(&self.inner)
            .map_err(|e| PyValueError::new_err(format!("serialization error: {e}")))?;
        let obj = json_value_to_py(py, &val)?;
        let bound = obj.bind(py);
        let dict: &Bound<'py, PyDict> = bound
            .cast()
            .map_err(|_| PyValueError::new_err("internal: to_dict produced non-dict"))?;
        Ok(dict.clone())
    }

    #[classmethod]
    fn builder(_cls: &Bound<'_, PyType>) -> PyTariffBuilder {
        PyTariffBuilder::new()
    }

    #[getter]
    fn name(&self) -> Option<&str> {
        self.inner.name.as_deref()
    }

    fn __repr__(&self) -> String {
        let name = self.inner.name.as_deref().unwrap_or("<unnamed>");
        let periods = self.inner.tou_schedule.len();
        let rates = self.inner.energy_rates.len();
        format!("ElectricTariff(name='{name}', tou_periods={periods}, energy_rates={rates})")
    }

    fn __eq__(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

// ---------------------------------------------------------------------------
// PyTariffBuilder
// ---------------------------------------------------------------------------

#[pyclass(name = "TariffBuilder", skip_from_py_object)]
#[derive(Debug, Clone)]
pub struct PyTariffBuilder {
    name: Option<String>,
    tou_periods: Vec<TouPeriod>,
    demand_tou_periods: Vec<TouPeriod>,
    energy_rates: Vec<EnergyRate>,
    demand_rates: Vec<DemandRate>,
    tiered_rates: Vec<TieredBlock>,
    export_rate: ExportRate,
    fixed_charges: FixedCharges,
    minimum_charge: Option<f64>,
    minimum_charge_excludes_export: bool,
    billing_cycle: BillingCycle,
    seasonal_split: Option<SeasonalSplit>,
    demand_window_minutes: u32,
}

impl PyTariffBuilder {
    fn new() -> Self {
        Self {
            name: None,
            tou_periods: Vec::new(),
            demand_tou_periods: Vec::new(),
            energy_rates: Vec::new(),
            demand_rates: Vec::new(),
            tiered_rates: Vec::new(),
            export_rate: ExportRate::default(),
            fixed_charges: FixedCharges::default(),
            minimum_charge: None,
            minimum_charge_excludes_export: true,
            billing_cycle: BillingCycle::Monthly,
            seasonal_split: None,
            demand_window_minutes: 15,
        }
    }
}

#[pymethods]
impl PyTariffBuilder {
    fn set_name(slf: Py<Self>, py: Python<'_>, name: String) -> Py<Self> {
        slf.borrow_mut(py).name = Some(name);
        slf
    }

    fn add_tou_period(
        slf: Py<Self>,
        py: Python<'_>,
        name: String,
        windows: &Bound<'_, PyList>,
        season: &str,
    ) -> PyResult<Py<Self>> {
        let season = parse_season(season)?;
        let mut parsed_windows = Vec::with_capacity(windows.len());
        for item in windows.iter() {
            let dict = item
                .cast::<PyDict>()
                .map_err(|_| PyValueError::new_err("each window must be a dict"))?;
            parsed_windows.push(parse_window(dict)?);
        }
        slf.borrow_mut(py).tou_periods.push(TouPeriod {
            name,
            schedule: parsed_windows,
            season,
        });
        Ok(slf)
    }

    fn add_energy_rate(
        slf: Py<Self>,
        py: Python<'_>,
        period_name: String,
        season: &str,
        rate: f64,
    ) -> PyResult<Py<Self>> {
        if !rate.is_finite() || rate < 0.0 {
            return Err(PyValueError::new_err(format!(
                "rate must be finite and >= 0, got {rate}"
            )));
        }
        let season = parse_season(season)?;
        slf.borrow_mut(py).energy_rates.push(EnergyRate {
            period_name,
            season,
            rate_per_kwh: rate,
        });
        Ok(slf)
    }

    #[pyo3(signature = (rate_per_kw, season, period_name=None, ratchet_fraction=None, lookback_months=None))]
    fn add_demand_rate(
        slf: Py<Self>,
        py: Python<'_>,
        rate_per_kw: f64,
        season: &str,
        period_name: Option<String>,
        ratchet_fraction: Option<f64>,
        lookback_months: Option<u8>,
    ) -> PyResult<Py<Self>> {
        if !rate_per_kw.is_finite() || rate_per_kw < 0.0 {
            return Err(PyValueError::new_err(format!(
                "rate_per_kw must be finite and >= 0, got {rate_per_kw}"
            )));
        }
        let season = parse_season(season)?;
        let ratchet = match (ratchet_fraction, lookback_months) {
            (Some(frac), Some(months)) => Some(RatchetConfig {
                lookback_months: months,
                minimum_fraction: frac,
            }),
            (None, None) => None,
            _ => {
                return Err(PyValueError::new_err(
                    "ratchet_fraction and lookback_months must both be set or both be None",
                ));
            }
        };
        slf.borrow_mut(py).demand_rates.push(DemandRate {
            period_name,
            season,
            rate_per_kw,
            ratchet,
        });
        Ok(slf)
    }

    fn set_tiered_rates(
        slf: Py<Self>,
        py: Python<'_>,
        season: &str,
        thresholds_kwh: Vec<f64>,
        rates_per_kwh: Vec<f64>,
    ) -> PyResult<Py<Self>> {
        let season = parse_season(season)?;
        let block = TieredBlock {
            season,
            thresholds_kwh,
            rates_per_kwh,
        };
        block
            .validate()
            .map_err(|e| PyValueError::new_err(format!("{e}")))?;
        slf.borrow_mut(py).tiered_rates.push(block);
        Ok(slf)
    }

    #[pyo3(signature = (monthly_usd=0.0, daily_usd=0.0))]
    fn set_fixed_charges(
        slf: Py<Self>,
        py: Python<'_>,
        monthly_usd: f64,
        daily_usd: f64,
    ) -> PyResult<Py<Self>> {
        if !monthly_usd.is_finite() || monthly_usd < 0.0 {
            return Err(PyValueError::new_err(format!(
                "monthly_usd must be finite and >= 0, got {monthly_usd}"
            )));
        }
        if !daily_usd.is_finite() || daily_usd < 0.0 {
            return Err(PyValueError::new_err(format!(
                "daily_usd must be finite and >= 0, got {daily_usd}"
            )));
        }
        slf.borrow_mut(py).fixed_charges = FixedCharges {
            monthly_usd,
            daily_usd,
        };
        Ok(slf)
    }

    #[pyo3(signature = (minutes=15))]
    fn set_demand_window_minutes(
        slf: Py<Self>,
        py: Python<'_>,
        minutes: u32,
    ) -> PyResult<Py<Self>> {
        if !(5..=60).contains(&minutes) {
            return Err(PyValueError::new_err(format!(
                "demand_window_minutes must be in [5, 60], got {minutes}"
            )));
        }
        slf.borrow_mut(py).demand_window_minutes = minutes;
        Ok(slf)
    }

    fn set_export_net_metering(slf: Py<Self>, py: Python<'_>) -> Py<Self> {
        slf.borrow_mut(py).export_rate = ExportRate {
            mode: ExportMode::NetMetering,
            tou_credits: Vec::new(),
        };
        slf
    }

    fn set_export_net_billing(
        slf: Py<Self>,
        py: Python<'_>,
        tou_credits: &Bound<'_, PyList>,
    ) -> PyResult<Py<Self>> {
        let mut credits = Vec::with_capacity(tou_credits.len());
        for item in tou_credits.iter() {
            let d = item
                .cast::<PyDict>()
                .map_err(|_| PyValueError::new_err("each tou_credit must be a dict"))?;
            let period_name: String = d
                .get_item("period_name")?
                .ok_or_else(|| PyValueError::new_err("tou_credit missing 'period_name'"))?
                .extract()?;
            let season_str: String = d
                .get_item("season")?
                .ok_or_else(|| PyValueError::new_err("tou_credit missing 'season'"))?
                .extract()?;
            let rate: f64 = d
                .get_item("rate")?
                .ok_or_else(|| PyValueError::new_err("tou_credit missing 'rate'"))?
                .extract()?;
            credits.push(EnergyRate {
                period_name,
                season: parse_season(&season_str)?,
                rate_per_kwh: rate,
            });
        }
        slf.borrow_mut(py).export_rate = ExportRate {
            mode: ExportMode::NetBilling,
            tou_credits: credits,
        };
        Ok(slf)
    }

    fn set_export_flat_rate(slf: Py<Self>, py: Python<'_>, rate: f64) -> PyResult<Py<Self>> {
        if !rate.is_finite() || rate < 0.0 {
            return Err(PyValueError::new_err(format!(
                "rate must be finite and >= 0, got {rate}"
            )));
        }
        slf.borrow_mut(py).export_rate = ExportRate {
            mode: ExportMode::FlatRate(rate),
            tou_credits: Vec::new(),
        };
        Ok(slf)
    }

    fn set_minimum_charge(slf: Py<Self>, py: Python<'_>, amount: f64) -> PyResult<Py<Self>> {
        if !amount.is_finite() || amount < 0.0 {
            return Err(PyValueError::new_err(format!(
                "amount must be finite and >= 0, got {amount}"
            )));
        }
        slf.borrow_mut(py).minimum_charge = Some(amount);
        Ok(slf)
    }

    fn set_minimum_charge_excludes_export(
        slf: Py<Self>,
        py: Python<'_>,
        excludes_export: bool,
    ) -> Py<Self> {
        slf.borrow_mut(py).minimum_charge_excludes_export = excludes_export;
        slf
    }

    /// Add a demand-specific TOU period (separate from energy TOU periods).
    /// If no demand TOU periods are added, demand charges use energy TOU periods.
    fn add_demand_tou_period(
        slf: Py<Self>,
        py: Python<'_>,
        name: String,
        windows: &Bound<'_, PyList>,
        season: String,
    ) -> PyResult<Py<Self>> {
        let season_filter = parse_season(&season)?;
        let mut schedule = Vec::new();
        for item in windows.iter() {
            let d: &Bound<'_, PyDict> = item.cast()?;
            schedule.push(parse_window(d)?);
        }
        slf.borrow_mut(py).demand_tou_periods.push(TouPeriod {
            name,
            schedule,
            season: season_filter,
        });
        Ok(slf)
    }

    /// Set billing cycle: "monthly" (default) or a custom number of days.
    #[pyo3(signature = (cycle_days=None))]
    fn set_billing_cycle(slf: Py<Self>, py: Python<'_>, cycle_days: Option<u32>) -> Py<Self> {
        slf.borrow_mut(py).billing_cycle = match cycle_days {
            None => BillingCycle::Monthly,
            Some(days) => BillingCycle::Custom(days),
        };
        slf
    }

    /// Set custom summer/winter boundary months (1-indexed, inclusive).
    fn set_seasonal_split(
        slf: Py<Self>,
        py: Python<'_>,
        summer_start_month: u8,
        summer_end_month: u8,
    ) -> PyResult<Py<Self>> {
        let split = SeasonalSplit::new(summer_start_month, summer_end_month)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        slf.borrow_mut(py).seasonal_split = Some(split);
        Ok(slf)
    }

    fn build(&self) -> PyResult<PyElectricTariff> {
        let tariff = ElectricTariff {
            name: self.name.clone(),
            tou_schedule: self.tou_periods.clone(),
            demand_tou_schedule: self.demand_tou_periods.clone(),
            energy_rates: self.energy_rates.clone(),
            demand_rates: self.demand_rates.clone(),
            tiered_rates: self.tiered_rates.clone(),
            export_rate: self.export_rate.clone(),
            fixed_charges: self.fixed_charges.clone(),
            minimum_charge: self.minimum_charge,
            minimum_charge_excludes_export: self.minimum_charge_excludes_export,
            billing_cycle: self.billing_cycle,
            seasonal_split: self.seasonal_split,
            demand_window_minutes: self.demand_window_minutes,
            rtp_schedule: None,
            cpp_config: None,
            ev_tou_period_name: None,
        };
        tariff
            .validate()
            .map_err(|e| PyValueError::new_err(format!("tariff validation failed: {e}")))?;
        Ok(PyElectricTariff { inner: tariff })
    }

    fn __repr__(&self) -> String {
        let name = self.name.as_deref().unwrap_or("<unnamed>");
        format!(
            "TariffBuilder(name={name:?}, tou_periods={}, energy_rates={}, demand_rates={})",
            self.tou_periods.len(),
            self.energy_rates.len(),
            self.demand_rates.len(),
        )
    }
}

// ---------------------------------------------------------------------------
// PyGasTariff
// ---------------------------------------------------------------------------

#[pyclass(name = "GasTariff", from_py_object)]
#[derive(Debug, Clone)]
pub struct PyGasTariff {
    pub(crate) inner: GasTariff,
}

#[pymethods]
impl PyGasTariff {
    #[classmethod]
    pub fn from_dict(_cls: &Bound<'_, PyType>, dict: &Bound<'_, PyDict>) -> PyResult<Self> {
        let val = py_to_json_value(&dict.clone().into_any())?;
        let json_str = serde_json::to_string(&val)
            .map_err(|e| PyValueError::new_err(format!("serialization error: {e}")))?;
        let tariff: GasTariff = serde_json::from_str(&json_str)
            .map_err(|e| PyValueError::new_err(format!("deserialization error: {e}")))?;
        tariff
            .validate()
            .map_err(|e| PyValueError::new_err(format!("gas tariff validation failed: {e}")))?;
        Ok(Self { inner: tariff })
    }

    #[classmethod]
    pub fn from_json(_cls: &Bound<'_, PyType>, json_str: &str) -> PyResult<Self> {
        let tariff: GasTariff = serde_json::from_str(json_str)
            .map_err(|e| PyValueError::new_err(format!("JSON parse error: {e}")))?;
        tariff
            .validate()
            .map_err(|e| PyValueError::new_err(format!("gas tariff validation failed: {e}")))?;
        Ok(Self { inner: tariff })
    }

    fn to_dict<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let val = serde_json::to_value(&self.inner)
            .map_err(|e| PyValueError::new_err(format!("serialization error: {e}")))?;
        let obj = json_value_to_py(py, &val)?;
        let bound = obj.bind(py);
        let dict: &Bound<'py, PyDict> = bound
            .cast()
            .map_err(|_| PyValueError::new_err("internal: to_dict produced non-dict"))?;
        Ok(dict.clone())
    }

    #[classmethod]
    fn builder(_cls: &Bound<'_, PyType>) -> PyGasTariffBuilder {
        PyGasTariffBuilder::new()
    }

    #[getter]
    fn name(&self) -> Option<&str> {
        self.inner.name.as_deref()
    }

    fn __repr__(&self) -> String {
        let name = self.inner.name.as_deref().unwrap_or("<unnamed>");
        let tiers = self.inner.tiered_rates.len();
        format!("GasTariff(name='{name}', tiered_rates={tiers})")
    }

    fn __eq__(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

// ---------------------------------------------------------------------------
// PyGasTariffBuilder
// ---------------------------------------------------------------------------

#[pyclass(name = "GasTariffBuilder", skip_from_py_object)]
#[derive(Debug, Clone)]
pub struct PyGasTariffBuilder {
    name: Option<String>,
    tiered_rates: Vec<GasTieredBlock>,
    fixed_charges: FixedCharges,
    billing_cycle: BillingCycle,
    seasonal_split: Option<SeasonalSplit>,
}

impl PyGasTariffBuilder {
    fn new() -> Self {
        Self {
            name: None,
            tiered_rates: Vec::new(),
            fixed_charges: FixedCharges::default(),
            billing_cycle: BillingCycle::Monthly,
            seasonal_split: None,
        }
    }
}

#[pymethods]
impl PyGasTariffBuilder {
    fn set_name(slf: Py<Self>, py: Python<'_>, name: String) -> Py<Self> {
        slf.borrow_mut(py).name = Some(name);
        slf
    }

    fn set_tiered_rates(
        slf: Py<Self>,
        py: Python<'_>,
        season: &str,
        thresholds_therms: Vec<f64>,
        rates_per_therm: Vec<f64>,
    ) -> PyResult<Py<Self>> {
        let season = parse_season(season)?;
        let block = GasTieredBlock {
            season,
            thresholds_therms,
            rates_per_therm,
        };
        block
            .validate()
            .map_err(|e| PyValueError::new_err(format!("{e}")))?;
        slf.borrow_mut(py).tiered_rates.push(block);
        Ok(slf)
    }

    #[pyo3(signature = (monthly_usd=0.0, daily_usd=0.0))]
    fn set_fixed_charges(
        slf: Py<Self>,
        py: Python<'_>,
        monthly_usd: f64,
        daily_usd: f64,
    ) -> Py<Self> {
        slf.borrow_mut(py).fixed_charges = FixedCharges {
            monthly_usd,
            daily_usd,
        };
        slf
    }

    #[pyo3(signature = (cycle_days=None))]
    fn set_billing_cycle(slf: Py<Self>, py: Python<'_>, cycle_days: Option<u32>) -> Py<Self> {
        slf.borrow_mut(py).billing_cycle = match cycle_days {
            None => BillingCycle::Monthly,
            Some(days) => BillingCycle::Custom(days),
        };
        slf
    }

    fn set_seasonal_split(
        slf: Py<Self>,
        py: Python<'_>,
        summer_start_month: u8,
        summer_end_month: u8,
    ) -> PyResult<Py<Self>> {
        let split = SeasonalSplit::new(summer_start_month, summer_end_month)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        slf.borrow_mut(py).seasonal_split = Some(split);
        Ok(slf)
    }

    fn build(&self) -> PyResult<PyGasTariff> {
        let tariff = GasTariff {
            name: self.name.clone(),
            tiered_rates: self.tiered_rates.clone(),
            fixed_charges: self.fixed_charges.clone(),
            billing_cycle: self.billing_cycle,
            seasonal_split: self.seasonal_split,
        };
        tariff
            .validate()
            .map_err(|e| PyValueError::new_err(format!("tariff validation failed: {e}")))?;
        Ok(PyGasTariff { inner: tariff })
    }

    fn __repr__(&self) -> String {
        let name = self.name.as_deref().unwrap_or("<unnamed>");
        format!(
            "GasTariffBuilder(name={name:?}, tiered_rates={})",
            self.tiered_rates.len(),
        )
    }
}
