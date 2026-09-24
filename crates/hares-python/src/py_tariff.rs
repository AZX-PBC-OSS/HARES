use chrono::{DateTime, FixedOffset, TimeZone};
use hares_tariff::TariffEvaluator;
use hares_tariff::types::{
    DemandRate, ElectricTariff, EnergyRate, ExportMode, ExportRate, FixedCharges, GasTariff,
    GasTieredBlock, RatchetConfig, TieredBlock,
};
use hares_types::{BillingCycle, SeasonFilter, SeasonalSplit, TimeWindow, TouPeriod};
use pyo3::IntoPyObjectExt;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyType};

use crate::py_telemetry::PyBillingPeriodSummary;
use crate::utils::extract_datetime;
use crate::utils::extract_seconds;

fn parse_season(s: &str) -> PyResult<SeasonFilter> {
    match s.to_lowercase().as_str() {
        "all" => Ok(SeasonFilter::All),
        "summer" => Ok(SeasonFilter::Summer),
        "winter" => Ok(SeasonFilter::Winter),
        "shoulder" => Ok(SeasonFilter::Shoulder),
        _ => Err(PyValueError::new_err(format!(
            "unknown season '{s}', expected 'all', 'summer', 'winter', or 'shoulder'"
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
// PyTariffEvaluator
// ---------------------------------------------------------------------------

#[pyclass(name = "TariffEvaluator")]
pub struct PyTariffEvaluator {
    evaluator: TariffEvaluator,
    tariff_name: String,
    start_time: DateTime<FixedOffset>,
    end_time: DateTime<FixedOffset>,
}

fn fixed_offset_to_tz(dt: DateTime<FixedOffset>) -> DateTime<chrono_tz::Tz> {
    let naive = dt.naive_utc();
    chrono_tz::UTC.from_utc_datetime(&naive)
}

#[pymethods]
impl PyTariffEvaluator {
    #[new]
    fn new(
        tariff: &PyElectricTariff,
        start: &Bound<'_, pyo3::PyAny>,
        end: &Bound<'_, pyo3::PyAny>,
        interval: &Bound<'_, pyo3::PyAny>,
    ) -> PyResult<Self> {
        let start_dt: DateTime<FixedOffset> = extract_datetime(start)?;
        let end_dt: DateTime<FixedOffset> = extract_datetime(end)?;
        let interval_s = extract_seconds(interval)?;
        if interval_s <= 0 {
            return Err(PyValueError::new_err("interval must be > 0 seconds"));
        }
        let interval_s = interval_s as u32;

        let tz_start = fixed_offset_to_tz(start_dt);
        let tz_end = fixed_offset_to_tz(end_dt);

        let evaluator = TariffEvaluator::new(tariff.inner.clone(), tz_start, tz_end, interval_s)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;

        let tariff_name = tariff
            .inner
            .name
            .clone()
            .unwrap_or_else(|| "<unnamed>".to_string());

        Ok(Self {
            evaluator,
            tariff_name,
            start_time: start_dt,
            end_time: end_dt,
        })
    }

    fn step(
        &mut self,
        net_power_kw: f64,
        dt_seconds: f64,
        current_time: &Bound<'_, pyo3::PyAny>,
    ) -> PyResult<Option<PyBillingPeriodSummary>> {
        let dt: DateTime<FixedOffset> = extract_datetime(current_time)?;
        let tz_dt = fixed_offset_to_tz(dt);
        Ok(self
            .evaluator
            .step(net_power_kw, 0.0, dt_seconds, tz_dt)
            .map(|s| PyBillingPeriodSummary::from_rust(&s)))
    }

    fn finalize(
        &mut self,
        sim_end: &Bound<'_, pyo3::PyAny>,
    ) -> PyResult<Option<PyBillingPeriodSummary>> {
        let end: DateTime<FixedOffset> = extract_datetime(sim_end)?;
        let tz_end = fixed_offset_to_tz(end);
        Ok(self
            .evaluator
            .finalize(tz_end)
            .map(|s| PyBillingPeriodSummary::from_rust(&s)))
    }

    #[getter]
    fn current_metrics(&self) -> PyBillingPeriodSummary {
        PyBillingPeriodSummary::from_rust(&self.evaluator.current_metrics())
    }

    #[getter]
    fn tariff_name(&self) -> &str {
        &self.tariff_name
    }

    #[getter]
    fn start_time<'py>(&self, py: Python<'py>) -> PyResult<Py<pyo3::PyAny>> {
        let datetime = py.import("datetime")?.getattr("datetime")?;
        let obj = datetime.call_method1("fromisoformat", (self.start_time.to_rfc3339(),))?;
        Ok(obj.unbind())
    }

    #[getter]
    fn end_time<'py>(&self, py: Python<'py>) -> PyResult<Py<pyo3::PyAny>> {
        let datetime = py.import("datetime")?.getattr("datetime")?;
        let obj = datetime.call_method1("fromisoformat", (self.end_time.to_rfc3339(),))?;
        Ok(obj.unbind())
    }

    fn __repr__(&self) -> String {
        format!(
            "TariffEvaluator(tariff='{}', steps={}/{})",
            self.tariff_name,
            self.evaluator.step_index(),
            self.evaluator.total_steps(),
        )
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

    fn set_export_hourly_schedule(
        slf: Py<Self>,
        py: Python<'_>,
        schedule: Vec<f64>,
    ) -> PyResult<Py<Self>> {
        if schedule.len() != 8760 {
            return Err(PyValueError::new_err(format!(
                "schedule must have exactly 8760 entries, got {}",
                schedule.len()
            )));
        }
        for (i, &price) in schedule.iter().enumerate() {
            if !price.is_finite() || price < 0.0 {
                return Err(PyValueError::new_err(format!(
                    "schedule[{i}] must be finite and >= 0, got {price}"
                )));
            }
        }
        slf.borrow_mut(py).export_rate = ExportRate {
            mode: ExportMode::HourlySchedule(schedule),
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
    /// Optionally configure a shoulder (spring/fall) season range.
    #[pyo3(signature = (summer_start_month, summer_end_month, shoulder_start_month=None, shoulder_end_month=None))]
    fn set_seasonal_split(
        slf: Py<Self>,
        py: Python<'_>,
        summer_start_month: u8,
        summer_end_month: u8,
        shoulder_start_month: Option<u8>,
        shoulder_end_month: Option<u8>,
    ) -> PyResult<Py<Self>> {
        let split = match (shoulder_start_month, shoulder_end_month) {
            (Some(s), Some(e)) => {
                SeasonalSplit::with_shoulder(summer_start_month, summer_end_month, Some(s), Some(e))
                    .map_err(|e| PyValueError::new_err(e.to_string()))?
            }
            _ => SeasonalSplit::new(summer_start_month, summer_end_month)
                .map_err(|e| PyValueError::new_err(e.to_string()))?,
        };
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

    /// Set custom summer/winter boundary months (1-indexed, inclusive).
    /// Optionally configure a shoulder (spring/fall) season range.
    #[pyo3(signature = (summer_start_month, summer_end_month, shoulder_start_month=None, shoulder_end_month=None))]
    fn set_seasonal_split(
        slf: Py<Self>,
        py: Python<'_>,
        summer_start_month: u8,
        summer_end_month: u8,
        shoulder_start_month: Option<u8>,
        shoulder_end_month: Option<u8>,
    ) -> PyResult<Py<Self>> {
        let split = match (shoulder_start_month, shoulder_end_month) {
            (Some(s), Some(e)) => {
                SeasonalSplit::with_shoulder(summer_start_month, summer_end_month, Some(s), Some(e))
                    .map_err(|e| PyValueError::new_err(e.to_string()))?
            }
            _ => SeasonalSplit::new(summer_start_month, summer_end_month)
                .map_err(|e| PyValueError::new_err(e.to_string()))?,
        };
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use pyo3::types::PyList;

    fn make_datetime(py: Python<'_>, iso: &str) -> PyResult<Py<pyo3::PyAny>> {
        let dt_mod = py.import("datetime")?;
        let dt_cls = dt_mod.getattr("datetime")?;
        let obj = dt_cls.call_method1("fromisoformat", (iso,))?;
        Ok(obj.unbind())
    }

    fn make_timedelta(py: Python<'_>, hours: i64) -> PyResult<Py<pyo3::PyAny>> {
        let dt_mod = py.import("datetime")?;
        let td_cls = dt_mod.getattr("timedelta")?;
        let obj = td_cls.call1((0, hours * 3600))?;
        Ok(obj.unbind())
    }

    fn advance_datetime(
        py: Python<'_>,
        base_dt: &Py<pyo3::PyAny>,
        step: usize,
        interval_seconds: i64,
    ) -> PyResult<Py<pyo3::PyAny>> {
        let dt_mod = py.import("datetime")?;
        let td_cls = dt_mod.getattr("timedelta")?;
        let offset = td_cls.call1((0, step as i64 * interval_seconds))?;

        let result = base_dt.bind(py).call_method1("__add__", (offset,))?;
        Ok(result.unbind())
    }

    fn build_flat_tariff(py: Python<'_>, rate: f64) -> PyResult<Py<PyElectricTariff>> {
        let builder = PyTariffBuilder::new();
        let mut builder = Py::new(py, builder)?;

        builder = PyTariffBuilder::set_name(builder, py, "flat-test".into());

        let windows: Vec<Py<pyo3::PyAny>> = {
            let d = PyDict::new(py);
            d.set_item("day", "any")?;
            d.set_item("start_hour", 0.0_f64)?;
            d.set_item("end_hour", 24.0_f64)?;
            vec![d.into_any().unbind()]
        };
        let py_windows = PyList::new(py, windows)?;
        builder = PyTariffBuilder::add_tou_period(builder, py, "flat".into(), &py_windows, "all")?;

        builder = PyTariffBuilder::add_energy_rate(builder, py, "flat".into(), "all", rate)?;

        let tariff = PyTariffBuilder::build(&builder.bind(py).borrow())?;
        Py::new(py, tariff)
    }

    fn step_evaluator(
        py: Python<'_>,
        ev: &mut PyTariffEvaluator,
        start: &Py<pyo3::PyAny>,
        power_fn: impl Fn(usize) -> f64,
        interval_s: i64,
        n_steps: usize,
    ) -> PyResult<Vec<PyBillingPeriodSummary>> {
        let mut summaries = Vec::new();
        for i in 0..n_steps {
            let ct = advance_datetime(py, start, i + 1, interval_s)?;
            if let Some(s) = ev.step(power_fn(i), interval_s as f64, ct.bind(py).as_any())? {
                summaries.push(s);
            }
        }
        Ok(summaries)
    }

    #[test]
    fn tariff_evaluator_standalone_flat_rate() {
        pyo3::Python::attach(|py| {
            let tariff_py = build_flat_tariff(py, 0.12).unwrap();
            let tariff = tariff_py.bind(py).borrow();
            let start = make_datetime(py, "2025-01-01T00:00:00Z").unwrap();
            let end = make_datetime(py, "2026-01-01T00:00:00Z").unwrap();
            let interval = make_timedelta(py, 1).unwrap();

            let mut ev = PyTariffEvaluator::new(
                &tariff,
                start.bind(py).as_any(),
                end.bind(py).as_any(),
                interval.bind(py).as_any(),
            )
            .unwrap();

            let summaries = step_evaluator(py, &mut ev, &start, |_| 1.0, 3600, 8760).unwrap();

            let final_summary = ev
                .finalize(end.bind(py).as_any())
                .unwrap()
                .expect("finalize should return summary");

            let total_energy: f64 = summaries
                .iter()
                .map(|s| s.total_import_kwh)
                .chain(std::iter::once(final_summary.total_import_kwh))
                .sum();
            let total_cost: f64 = summaries
                .iter()
                .map(|s| s.energy_charge_usd)
                .chain(std::iter::once(final_summary.energy_charge_usd))
                .sum();

            let expected_kwh = 8760.0;
            let expected_cost = expected_kwh * 0.12;
            assert!(
                (total_energy - expected_kwh).abs() < 1e-6,
                "expected {expected_kwh} kWh, got {total_energy}"
            );
            assert!(
                (total_cost - expected_cost).abs() < 1e-6,
                "expected ${expected_cost}, got ${total_cost}"
            );
        });
    }

    #[test]
    fn tariff_evaluator_tou_pricing() {
        pyo3::Python::attach(|py| {
            let builder = PyTariffBuilder::new();
            let mut builder = Py::new(py, builder)?;

            builder = PyTariffBuilder::set_name(builder, py, "tou-test".into());

            let on_peak_windows: Vec<Py<pyo3::PyAny>> = {
                let d = PyDict::new(py);
                d.set_item("day", "weekdays")?;
                d.set_item("start_hour", 16.0_f64)?;
                d.set_item("end_hour", 21.0_f64)?;
                vec![d.into_any().unbind()]
            };
            let py_windows = PyList::new(py, on_peak_windows)?;
            builder =
                PyTariffBuilder::add_tou_period(builder, py, "on-peak".into(), &py_windows, "all")?;

            let off_peak_windows: Vec<Py<pyo3::PyAny>> = {
                let d = PyDict::new(py);
                d.set_item("day", "any")?;
                d.set_item("start_hour", 0.0_f64)?;
                d.set_item("end_hour", 24.0_f64)?;
                vec![d.into_any().unbind()]
            };
            let py_windows = PyList::new(py, off_peak_windows)?;
            builder = PyTariffBuilder::add_tou_period(
                builder,
                py,
                "off-peak".into(),
                &py_windows,
                "all",
            )?;

            builder = PyTariffBuilder::add_energy_rate(builder, py, "on-peak".into(), "all", 0.35)?;
            builder =
                PyTariffBuilder::add_energy_rate(builder, py, "off-peak".into(), "all", 0.10)?;

            let tariff = PyTariffBuilder::build(&builder.bind(py).borrow())?;
            let tariff = Py::new(py, tariff)?;
            let tariff_ref = tariff.bind(py).borrow();

            // Jan 6 2025 is a Monday; simulate 24 hours
            let start = make_datetime(py, "2025-01-06T00:00:00Z")?;
            let end = make_datetime(py, "2025-01-07T00:00:00Z")?;
            let interval = make_timedelta(py, 1)?;

            let mut ev = PyTariffEvaluator::new(
                &tariff_ref,
                start.bind(py).as_any(),
                end.bind(py).as_any(),
                interval.bind(py).as_any(),
            )?;

            let summaries = step_evaluator(
                py,
                &mut ev,
                &start,
                |hour| {
                    if (16..21).contains(&(hour as u32)) {
                        2.0
                    } else {
                        1.0
                    }
                },
                3600,
                24,
            )?;

            let final_s = ev
                .finalize(end.bind(py).as_any())?
                .expect("should have summary");

            let total_import: f64 = summaries
                .iter()
                .map(|s| s.total_import_kwh)
                .chain(std::iter::once(final_s.total_import_kwh))
                .sum();
            let total_cost: f64 = summaries
                .iter()
                .map(|s| s.energy_charge_usd)
                .chain(std::iter::once(final_s.energy_charge_usd))
                .sum();

            // 19 off-peak hours * 1 kW + 5 peak hours * 2 kW = 29 kWh
            assert!(
                (total_import - 29.0).abs() < 1e-6,
                "expected 29 kWh, got {total_import}"
            );
            // 19 * 1 * 0.10 + 10 * 0.35 = 1.90 + 3.50 = 5.40
            let expected_cost = 19.0 * 0.10 + 10.0 * 0.35;
            assert!(
                (total_cost - expected_cost).abs() < 1e-6,
                "expected ${expected_cost}, got ${total_cost}"
            );

            Ok::<_, pyo3::PyErr>(())
        })
        .unwrap();
    }

    #[test]
    fn tariff_evaluator_idempotent() {
        pyo3::Python::attach(|py| {
            let tariff_py = build_flat_tariff(py, 0.12)?;
            let tariff_ref = tariff_py.bind(py).borrow();
            let start = make_datetime(py, "2025-01-01T00:00:00Z")?;
            let end = make_datetime(py, "2025-01-02T00:00:00Z")?;
            let interval = make_timedelta(py, 1)?;

            let run = || -> PyResult<Vec<PyBillingPeriodSummary>> {
                let mut ev = PyTariffEvaluator::new(
                    &tariff_ref,
                    start.bind(py).as_any(),
                    end.bind(py).as_any(),
                    interval.bind(py).as_any(),
                )?;

                let mut summaries = Vec::new();
                for i in 0..24 {
                    let load = if i % 2 == 0 { 2.0 } else { 1.0 };
                    let ct = advance_datetime(py, &start, i + 1, 3600)?;
                    if let Some(s) = ev.step(load, 3600.0, ct.bind(py).as_any())? {
                        summaries.push(s);
                    }
                }
                if let Some(s) = ev.finalize(end.bind(py).as_any())? {
                    summaries.push(s);
                }
                Ok(summaries)
            };

            let first = run()?;
            let second = run()?;

            assert_eq!(first.len(), second.len(), "summary count should match");
            for (i, (a, b)) in first.iter().zip(second.iter()).enumerate() {
                assert!(
                    (a.total_import_kwh - b.total_import_kwh).abs() < 1e-10,
                    "summary[{i}] total_import_kwh mismatch: {} vs {}",
                    a.total_import_kwh,
                    b.total_import_kwh
                );
                assert!(
                    (a.energy_charge_usd - b.energy_charge_usd).abs() < 1e-10,
                    "summary[{i}] energy_charge_usd mismatch: {} vs {}",
                    a.energy_charge_usd,
                    b.energy_charge_usd
                );
                assert!(
                    (a.net_bill_usd - b.net_bill_usd).abs() < 1e-10,
                    "summary[{i}] net_bill_usd mismatch: {} vs {}",
                    a.net_bill_usd,
                    b.net_bill_usd
                );
            }

            Ok::<_, PyErr>(())
        })
        .unwrap();
    }
}
