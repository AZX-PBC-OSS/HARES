//! Python bindings for the variable catalogue and the table feed:
//! `Catalogue` and `Variable`, the frozen pyclasses over the Rust
//! catalogue (`Dwelling::catalogue()`), and `metrics_from_table`, the
//! Python binding of the core `metrics_from_table` taking any
//! `__arrow_c_stream__` table plus the Python `Catalogue`.

use std::sync::Arc;

use pyo3::exceptions::{PyKeyError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyAnyMethods, PyCapsule, PyList, PyType};
use pyo3_arrow::PyTable;

use hares_types::panic_hook::panic_payload_to_string;
use hares_types::{AcrossHomes, Aggregation, Sampling, VariableKind};

use crate::py_config::WholeSeconds;
use crate::py_dwelling::HaresSimulationError;
use crate::py_metrics::PyRunMetrics;

/// The lowercase kind tag a Variable carries: the digest's canonical kind
/// spellings, with the enum's labels and the quantity's across-homes rule
/// carried by the Variable's own `labels` and `across_homes`.
fn kind_tag(kind: &VariableKind) -> &'static str {
    match kind {
        VariableKind::Energy => "energy",
        VariableKind::Power => "power",
        VariableKind::Temperature => "temperature",
        VariableKind::Ratio => "ratio",
        VariableKind::State => "state",
        VariableKind::Enum { .. } => "enum",
        VariableKind::Quantity { .. } => "quantity",
    }
}

fn aggregation_tag(aggregation: Aggregation) -> &'static str {
    match aggregation {
        Aggregation::Sum => "sum",
        Aggregation::Mean => "mean",
        Aggregation::Mode => "mode",
        Aggregation::Last => "last",
    }
}

fn sampling_tag(sampling: Sampling) -> &'static str {
    match sampling {
        Sampling::IntervalMean => "interval-mean",
        Sampling::InstantAtStart => "instant-at-start",
        Sampling::InstantAtEnd => "instant-at-end",
    }
}

/// Two variable lists read the same: every property the Variable surface
/// carries, name, unit, kind, aggregation, sampling, nullability, source,
/// labels, across-homes rule and description, compared through those
/// spellings.
fn variables_read_the_same(a: &[hares_io::VariableDef], b: &[hares_io::VariableDef]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b.iter()).all(|(x, y)| {
            x.name == y.name
                && x.unit == y.unit
                && x.aggregation == y.aggregation
                && x.sampling == y.sampling
                && x.nullable == y.nullable
                && x.source == y.source
                && x.description == y.description
                && kind_tag(&x.kind) == kind_tag(&y.kind)
                && labels_of(x) == labels_of(y)
                && match (&x.kind, &y.kind) {
                    (
                        VariableKind::Quantity { across_homes: ax },
                        VariableKind::Quantity { across_homes: ay },
                    ) => ax == ay,
                    _ => true,
                }
        })
}

/// The code's label table for an `"enum"` variable, in code order; `None`
/// for every other kind. The view the Variable's `labels` getter carries:
/// a label table read back from a catalogue deserialisation is the same
/// table the declarations carry, whichever variant holds it.
fn labels_of(def: &hares_io::VariableDef) -> Option<Vec<String>> {
    match &def.kind {
        VariableKind::Enum { labels } => Some(
            (0..labels.len())
                .filter_map(|code| labels.get(code).map(str::to_string))
                .collect(),
        ),
        _ => None,
    }
}

/// One catalogue variable: the column's declaration, read only.
#[pyclass(frozen, name = "Variable")]
pub struct PyVariable {
    pub(crate) inner: hares_io::VariableDef,
}

#[pymethods]
impl PyVariable {
    /// The column name.
    #[getter]
    fn name(&self) -> &str {
        &self.inner.name
    }

    /// The variable's reporting unit, the `Unit::symbol()` string.
    #[getter]
    fn unit(&self) -> &str {
        self.inner.unit.symbol()
    }

    /// What the variable measures: `"energy"`, `"power"`,
    /// `"temperature"`, `"ratio"`, `"state"`, `"enum"` or `"quantity"`.
    #[getter]
    fn kind(&self) -> &'static str {
        kind_tag(&self.inner.kind)
    }

    /// The over-time aggregation rule: `"sum"`, `"mean"`, `"mode"` or
    /// `"last"`.
    #[getter]
    fn aggregation(&self) -> &'static str {
        aggregation_tag(self.inner.aggregation)
    }

    /// When in the step the value is taken: `"interval-mean"`,
    /// `"instant-at-start"` or `"instant-at-end"`.
    #[getter]
    fn sampling(&self) -> &'static str {
        sampling_tag(self.inner.sampling)
    }

    /// Whether the variable can be missing (null in the report).
    #[getter]
    fn nullable(&self) -> bool {
        self.inner.nullable
    }

    /// Where the variable comes from: `"dwelling"`, `"zone:<id>"`,
    /// `"equipment:<id>"` or `"actor:<name>"`.
    #[getter]
    fn source(&self) -> String {
        self.inner.source.tag()
    }

    /// The code's label table for an `"enum"` variable, in code order;
    /// `None` for every other kind.
    #[getter]
    fn labels(&self) -> Option<Vec<String>> {
        labels_of(&self.inner)
    }

    /// The across-homes rule for a `"quantity"` variable: `"sum"` or
    /// `"mean"`; `None` for every other kind.
    #[getter]
    fn across_homes(&self) -> Option<&'static str> {
        match &self.inner.kind {
            VariableKind::Quantity { across_homes } => Some(match across_homes {
                AcrossHomes::Sum => "sum",
                AcrossHomes::Mean => "mean",
            }),
            _ => None,
        }
    }

    /// What the variable measures, from the declaring site.
    #[getter]
    fn description(&self) -> &str {
        &self.inner.description
    }

    fn __repr__(&self) -> String {
        format!(
            "Variable('{}' ({}))",
            self.inner.name,
            self.inner.unit.symbol()
        )
    }
}

/// The catalogue of every output variable a built model can produce,
/// declared once, typed, with its source. Readable from build on,
/// before the first step (`Dwelling.catalogue()`), and on every finished
/// run (`RunResult.catalogue`).
#[pyclass(frozen, name = "Catalogue")]
pub struct PyCatalogue {
    pub(crate) inner: Arc<hares_io::Catalogue>,
}

#[pymethods]
impl PyCatalogue {
    /// Every variable, in schema order.
    fn variables(&self) -> Vec<PyVariable> {
        self.inner
            .variables()
            .iter()
            .map(|def| PyVariable { inner: def.clone() })
            .collect()
    }

    /// The variable declared for the named column; `KeyError` when the
    /// catalogue declares none.
    fn __getitem__(&self, name: &str) -> PyResult<PyVariable> {
        self.inner
            .resolve(name)
            .map(|def| PyVariable { inner: def.clone() })
            .ok_or_else(|| PyKeyError::new_err(format!("no catalogue variable '{name}'")))
    }

    /// The number of variables.
    fn __len__(&self) -> usize {
        self.inner.len()
    }

    /// Iterates the variables, in schema order.
    fn __iter__(this: PyRef<'_, Self>) -> PyResult<Py<PyAny>> {
        let py = this.py();
        let list = PyList::new(py, this.variables())?;
        list.as_any().call_method0("__iter__").map(|it| it.unbind())
    }

    /// Two catalogues are equal when their variables read the same: every
    /// property the Variable surface carries, the enum labels' contents
    /// included.
    fn __eq__(&self, other: &Self) -> bool {
        variables_read_the_same(self.inner.variables(), other.inner.variables())
    }

    /// The catalogue's digest: the hex SHA-256 of the catalogue's
    /// canonical encoding (the schema metadata's `hares:catalogue_digest`).
    fn digest(&self) -> String {
        self.inner.digest()
    }

    /// The catalogue's variables as a JSON array of their definitions:
    /// the column name, unit, kind, aggregation, sampling, nullability,
    /// source and description. `from_json` rebuilds a catalogue whose
    /// variables, digest and equality are the originals'.
    fn to_json(&self) -> PyResult<String> {
        serde_json::to_string(self.inner.variables())
            .map_err(|err| PyValueError::new_err(format!("catalogue serialization failed: {err}")))
    }

    /// Builds a catalogue from `to_json`'s JSON array of variable
    /// definitions. A duplicate column name is a `ValueError` naming both
    /// sources.
    #[classmethod]
    fn from_json(_cls: &Bound<'_, PyType>, json_str: &str) -> PyResult<Self> {
        let defs: Vec<hares_io::VariableDef> = serde_json::from_str(json_str)
            .map_err(|err| PyValueError::new_err(format!("JSON parse error: {err}")))?;
        let catalogue = hares_io::Catalogue::from_defs(defs)
            .map_err(|err| PyValueError::new_err(err.to_string()))?;
        Ok(Self {
            inner: Arc::new(catalogue),
        })
    }

    fn __repr__(&self) -> String {
        format!("Catalogue({} variables)", self.inner.len())
    }
}

/// The Python binding of the core table feed: scores a Timestep results
/// table through the catalogue with the run's own accumulator, so a
/// stored table scores the stored run's metrics.
///
/// `table` is any object with `__arrow_c_stream__` (the report tables'
/// `arrow()` export, a polars frame, a pyarrow table), imported with
/// pyo3-arrow's `PyTable::from_arrow_pycapsule`. `time_res` is the step
/// resolution in whole seconds (`int` or `datetime.timedelta`): a table
/// whose consecutive `Time` values are not exactly `time_res` apart, or
/// that carries a `Step Count` column, is an error naming the mismatch,
/// never wrong metrics. Per-batch schema drift below the stream schema
/// (Float32 under Float64) is reinterpreted by the Arrow C stream
/// interface itself (pyarrow's `pa.table` behaves identically); the
/// guarantee covers schema-level mismatches (verified caught).
/// `setpoint_deadband_c` is the config's comfort band when the run
/// declared one.
#[pyfunction(name = "metrics_from_table")]
#[pyo3(signature = (table, catalogue, time_res, setpoint_deadband_c = None))]
pub fn metrics_from_table(
    py: Python<'_>,
    table: &Bound<'_, PyAny>,
    catalogue: &Bound<'_, PyCatalogue>,
    time_res: WholeSeconds,
    setpoint_deadband_c: Option<f64>,
) -> PyResult<PyRunMetrics> {
    let type_name = table
        .get_type()
        .name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    let capsule = table.call_method0("__arrow_c_stream__").map_err(|_| {
        PyTypeError::new_err(format!(
            "metrics_from_table: expected an object with __arrow_c_stream__ \
             (an Arrow stream), got {type_name}"
        ))
    })?;
    let capsule = capsule.cast::<PyCapsule>().map_err(|_| {
        let returned = capsule
            .get_type()
            .name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|_| "<unknown>".to_string());
        PyTypeError::new_err(format!(
            "metrics_from_table: the object's __arrow_c_stream__ returned \
             {returned}, which is not an Arrow stream capsule (the protocol \
             requires __arrow_c_stream__ to return a PyCapsule)"
        ))
    })?;
    // A stream whose batches disagree with the stream's schema trips an
    // arrow-rs assert inside the FFI import, so the error mapping above
    // never sees it and the panic would surface as a PanicException. The
    // import is caught here and the defect reported as the typed error.
    let table = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        PyTable::from_arrow_pycapsule(capsule)
    })) {
        Ok(Ok(table)) => table,
        Ok(Err(err)) => return Err(PyValueError::new_err(format!("metrics_from_table: {err}"))),
        Err(payload) => {
            let defect = panic_payload_to_string(payload);
            return Err(HaresSimulationError::new_err(format!(
                "metrics_from_table: the stream violates the Arrow C stream \
                 contract (a batch does not match the stream's schema): {defect}"
            )));
        }
    };
    let batches: Vec<arrow::record_batch::RecordBatch> = table.batches().to_vec();
    let catalogue: Arc<hares_io::Catalogue> = catalogue.borrow().inner.clone();

    let metrics = py.detach(move || {
        hares_io::metrics_from_table(
            &batches,
            &catalogue,
            chrono::Duration::seconds(time_res.0),
            setpoint_deadband_c,
        )
        .map(hares_core::RunMetrics::from)
    });
    let metrics = metrics.map_err(|err| HaresSimulationError::new_err(err.to_string()))?;
    Ok(PyRunMetrics { inner: metrics })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use arrow::array::Float64Array;
    use arrow::datatypes::Field;
    use arrow::record_batch::{RecordBatch, RecordBatchIterator};
    use hares_core::{Dwelling, DwellingConfig, RunMetrics, SimulationEngine};
    use hares_io::Catalogue;
    use pyo3::Py;
    use pyo3::Python;
    use pyo3::types::{PyAnyMethods, PyCapsule};
    use pyo3_arrow::{PyRecordBatchReader, PyTable};

    use super::{PyCatalogue, metrics_from_table};
    use crate::conversions::batches_to_py_table;
    use crate::py_config::WholeSeconds;
    use crate::py_dwelling::HaresSimulationError;

    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    /// The acceptance fixture's config: the cz4a inputs, start
    /// `2023-01-01T00:00:00-07:00`, 24 h at 900 s, the `Verbosity(8)`
    /// memory report, the metrics request on.
    fn sim_toml() -> String {
        "\nstart_time = \"2023-01-01T00:00:00-07:00\"\nduration = 86400\ntime_res = 900\nmetrics = true\noutput_chunk_size = 40\n\n[[reports]]\nname = \"frame\"\nselection = { verbosity = 8 }\ndestination = \"memory\"\n".to_string()
    }

    /// Runs the acceptance fixture and returns the run's metrics, the
    /// dwelling's catalogue and the report's retained batches.
    fn acceptance_run() -> (RunMetrics, Arc<Catalogue>, Vec<RecordBatch>) {
        let root = repo_root();
        let home = root.join("tests/fixtures/parity/cz4a_ashp_hpwh");
        let sim = hares_io::SimulationConfig::from_toml(&sim_toml()).expect("sim config parses");
        let config = DwellingConfig {
            hpxml_path: home.join("building.xml"),
            schedule_path: Some(home.join("schedule.csv")),
            weather_path: home.join("weather.epw"),
            defaults_path: Some(root.join("defaults")),
            sim_config: sim.clone(),
            overrides: None,
            bldg_id: 1,
            initialization_duration: None,
            patches: None,
        };
        let mut dwelling = Dwelling::from_config(config).expect("dwelling builds");
        let catalogue = dwelling.catalogue();
        let result = SimulationEngine::new()
            .run_dwelling(&mut dwelling, &sim)
            .expect("run completes");
        let batches = result.reports[0].batches().to_vec();
        assert!(!batches.is_empty(), "the memory report retains batches");
        let metrics = result.metrics.as_ref().expect("the run requested metrics");
        (metrics.clone(), catalogue, batches)
    }

    /// Bitwise comparison of two run metrics: every f64 field compared on
    /// bits, so a sign-of-zero or payload difference cannot pass.
    fn assert_run_metrics_bitwise_equal(a: &RunMetrics, b: &RunMetrics, context: &str) {
        use std::collections::BTreeMap;
        fn bits(v: f64) -> u64 {
            v.to_bits()
        }
        fn opt(v: Option<f64>) -> Option<u64> {
            v.map(bits)
        }
        fn map(m: &BTreeMap<String, f64>) -> BTreeMap<String, u64> {
            m.iter().map(|(k, v)| (k.clone(), bits(*v))).collect()
        }
        let (x, y) = (&a.total_energy_kwh, &b.total_energy_kwh);
        assert_eq!(
            bits(x.net_energy_kwh),
            bits(y.net_energy_kwh),
            "{context}: net_energy_kwh"
        );
        assert_eq!(
            bits(x.gross_consumption_kwh),
            bits(y.gross_consumption_kwh),
            "{context}: gross_consumption_kwh"
        );
        assert_eq!(
            bits(x.gross_pv_generation_kwh),
            bits(y.gross_pv_generation_kwh),
            "{context}: gross_pv_generation_kwh"
        );
        assert_eq!(
            map(&x.per_end_use),
            map(&y.per_end_use),
            "{context}: energy per end use"
        );
        assert_eq!(
            bits(x.duration_hours),
            bits(y.duration_hours),
            "{context}: energy duration"
        );

        let (x, y) = (&a.peak_power_kw, &b.peak_power_kw);
        assert_eq!(
            map(&x.per_end_use),
            map(&y.per_end_use),
            "{context}: peak per end use"
        );
        assert_eq!(
            bits(x.rolling.peak_15min_kw),
            bits(y.rolling.peak_15min_kw),
            "{context}: peak 15min"
        );
        assert_eq!(
            bits(x.rolling.peak_30min_kw),
            bits(y.rolling.peak_30min_kw),
            "{context}: peak 30min"
        );
        assert_eq!(
            bits(x.rolling.peak_60min_kw),
            bits(y.rolling.peak_60min_kw),
            "{context}: peak 60min"
        );

        assert_eq!(
            opt(a.comfort_hours),
            opt(b.comfort_hours),
            "{context}: comfort_hours"
        );
        assert_eq!(
            opt(a.unmet_load_hours),
            opt(b.unmet_load_hours),
            "{context}: unmet_load_hours"
        );
        assert_eq!(
            opt(a.renewable_energy_fraction),
            opt(b.renewable_energy_fraction),
            "{context}: renewable_energy_fraction"
        );
        assert_eq!(
            bits(a.grid_interaction_metrics.peak_import_kw),
            bits(b.grid_interaction_metrics.peak_import_kw),
            "{context}: peak_import_kw"
        );
        assert_eq!(
            bits(a.grid_interaction_metrics.peak_export_kw),
            bits(b.grid_interaction_metrics.peak_export_kw),
            "{context}: peak_export_kw"
        );

        match (&a.envelope_loads_kwh, &b.envelope_loads_kwh) {
            (None, None) => {}
            (Some(x), Some(y)) => {
                let pairs = [
                    (x.window_solar_kwh, y.window_solar_kwh, "window_solar_kwh"),
                    (
                        x.window_conduction_kwh,
                        y.window_conduction_kwh,
                        "window_conduction_kwh",
                    ),
                    (
                        x.opaque_conduction_kwh,
                        y.opaque_conduction_kwh,
                        "opaque_conduction_kwh",
                    ),
                    (
                        x.internal_mass_kwh,
                        y.internal_mass_kwh,
                        "internal_mass_kwh",
                    ),
                    (x.interior_lwr_kwh, y.interior_lwr_kwh, "interior_lwr_kwh"),
                    (x.infiltration_kwh, y.infiltration_kwh, "infiltration_kwh"),
                    (x.ventilation_kwh, y.ventilation_kwh, "ventilation_kwh"),
                    (x.hvac_heating_kwh, y.hvac_heating_kwh, "hvac_heating_kwh"),
                    (x.hvac_cooling_kwh, y.hvac_cooling_kwh, "hvac_cooling_kwh"),
                    (
                        x.internal_gains_kwh,
                        y.internal_gains_kwh,
                        "internal_gains_kwh",
                    ),
                    (x.duct_loss_kwh, y.duct_loss_kwh, "duct_loss_kwh"),
                ];
                for (lx, ly, name) in pairs {
                    assert_eq!(bits(lx), bits(ly), "{context}: {name}");
                }
            }
            _ => panic!("{context}: envelope loads present on one side only"),
        }

        let (x, y) = (&a.efficiency, &b.efficiency);
        assert_eq!(
            opt(x.hvac_heating_cop),
            opt(y.hvac_heating_cop),
            "{context}: hvac_heating_cop"
        );
        assert_eq!(
            opt(x.hvac_cooling_cop),
            opt(y.hvac_cooling_cop),
            "{context}: hvac_cooling_cop"
        );
        assert_eq!(
            opt(x.water_heater_cop),
            opt(y.water_heater_cop),
            "{context}: water_heater_cop"
        );
        assert_eq!(
            opt(x.battery_round_trip_efficiency),
            opt(y.battery_round_trip_efficiency),
            "{context}: battery_round_trip_efficiency"
        );

        assert_eq!(
            opt(a.rows_with_partial_setpoint_data_fraction),
            opt(b.rows_with_partial_setpoint_data_fraction),
            "{context}: rows_with_partial_setpoint_data_fraction"
        );
        assert_eq!(
            bits(a.simulation_duration_hours),
            bits(b.simulation_duration_hours),
            "{context}: simulation_duration_hours"
        );
        assert_eq!(a.coverage, b.coverage, "{context}: coverage");
        assert_eq!(
            a.nan_step_count, b.nan_step_count,
            "{context}: nan_step_count"
        );
        assert_eq!(
            a.metrics_reliability, b.metrics_reliability,
            "{context}: metrics_reliability"
        );

        match (&a.gas_energy, &b.gas_energy) {
            (None, None) => {}
            (Some(x), Some(y)) => {
                assert_eq!(
                    bits(x.total_therms),
                    bits(y.total_therms),
                    "{context}: total_therms"
                );
                assert_eq!(
                    bits(x.total_kwh_equivalent),
                    bits(y.total_kwh_equivalent),
                    "{context}: total_kwh_equivalent"
                );
            }
            _ => panic!("{context}: gas energy present on one side only"),
        }
    }

    /// The bitwise differential: the Python-bound `metrics_from_table`
    /// over the capsule stream and the Python `Catalogue` computes the
    /// same metrics as the Rust path over the same table and catalogue,
    /// field for field, every f64 by bit pattern, and both equal the
    /// run's own metrics.
    #[test]
    fn metrics_from_table_binding_matches_the_rust_path_on_bits() {
        let (in_run, catalogue, batches) = acceptance_run();

        let from_rust = hares_io::metrics_from_table(
            &batches,
            &catalogue,
            chrono::Duration::seconds(900),
            None,
        )
        .expect("the run's own table scores");
        assert_run_metrics_bitwise_equal(
            &in_run,
            &RunMetrics::from(from_rust),
            "the rust path vs the run's own",
        );

        Python::attach(|py| {
            let py_table = batches_to_py_table(batches.clone()).expect("the table wraps");
            let py_table = Py::new(py, py_table).expect("the table holds");
            let py_catalogue = Py::new(
                py,
                PyCatalogue {
                    inner: catalogue.clone(),
                },
            )
            .expect("the catalogue holds");

            let capsule = py_table
                .bind(py)
                .call_method1("__arrow_c_stream__", (py.None(),))
                .expect("the table exposes __arrow_c_stream__");
            let capsule = capsule
                .cast::<PyCapsule>()
                .expect("the stream is a capsule");
            let imported = PyTable::from_arrow_pycapsule(capsule).expect("the capsule imports");
            assert_eq!(
                imported.batches().len(),
                batches.len(),
                "the stream carries one chunk per batch"
            );

            let from_binding = metrics_from_table(
                py,
                py_table.bind(py).as_any(),
                py_catalogue.bind(py),
                WholeSeconds(900),
                None,
            )
            .expect("the binding scores the same table");
            assert_run_metrics_bitwise_equal(
                &in_run,
                &from_binding.inner,
                "the python binding vs the run's own",
            );
        });
    }

    /// A stream whose second batch carries a column the stream schema does
    /// not declare violates the Arrow C stream contract: the Arrow FFI
    /// import asserts on the drift. That must surface as the typed
    /// simulation error naming the stream's defect, never a PanicException
    /// through the untyped panic path.
    #[test]
    fn metrics_from_table_flags_a_stream_that_breaks_the_ffi_contract() {
        use arrow::array::ArrayRef;
        use arrow::datatypes::{DataType, Schema};

        let (_, catalogue, batches) = acceptance_run();
        let (batch_schema, mut columns, row_count) = batches[1].clone().into_parts();
        let mut fields: Vec<_> = batch_schema.fields().iter().cloned().collect();
        fields.push(Arc::new(Field::new("Extra", DataType::Float64, false)));
        columns.push(Arc::new(Float64Array::from(vec![0.0; row_count])) as ArrayRef);
        let drifted = RecordBatch::try_new(Arc::new(Schema::new(fields)), columns)
            .expect("the drifted batch builds");
        let reader = RecordBatchIterator::new(
            vec![Ok(batches[0].clone()), Ok(drifted)],
            batches[0].schema(),
        );
        Python::attach(|py| {
            let stream =
                Py::new(py, PyRecordBatchReader::new(Box::new(reader))).expect("the reader holds");
            let py_catalogue =
                Py::new(py, PyCatalogue { inner: catalogue }).expect("the catalogue holds");
            let Err(err) = metrics_from_table(
                py,
                stream.bind(py).as_any(),
                py_catalogue.bind(py),
                WholeSeconds(900),
                None,
            ) else {
                panic!("the contract-violating stream is refused");
            };
            assert!(
                err.is_instance_of::<HaresSimulationError>(py),
                "the FFI assert surfaces as the typed simulation error, got: {err}"
            );
            assert!(
                err.to_string().contains("Arrow C stream contract"),
                "the error names the stream's defect: {err}"
            );
        });
    }
}
