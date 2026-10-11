//! The streaming ordered fleet fold.
//!
//! The fleet's result fold streams per-home reduced tables into one
//! running aggregate under a memory bound: a home's result in the fold is
//! its reduced period table (the `fleet` report's batches fed through a
//! [`PeriodReducer`] and dropped), never its frame, and at most one
//! chunk's home results are alive at once. The fold visits homes in the
//! fleet's entry order across every chunk, so the accumulation order per
//! cell is the entry order: the same sequence of floating-point
//! operations the collect-then-aggregate fold performed, which is what
//! keeps every fleet number bitwise equal to its parent commit's.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use arrow::array::{Array, Float64Builder, StringArray, StringBuilder};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use chrono::{DateTime, SecondsFormat};
use hares_io::Frequency;
use hares_io::output::callback::{BatchCallback, BatchContext, CallbackError};
use hares_io::output::reduce::{PeriodCalendar, PeriodReducer, PeriodRow, ReduceError, TimeRule};
use hares_io::{Selection, classify_sample_weight};
use hares_types::Aggregation;

use crate::fleet::{FleetError, SimStatus};

/// The fleet's report request: the variable selection, the reporting
/// frequency the fold reduces onto, and the fleet's
/// [`hares_io::HealthPolicy`], which is applied to every home's run.
#[derive(Debug, Clone, PartialEq)]
pub struct FleetReport {
    /// The variable selection every home's `fleet` report carries,
    /// restricted to the home's catalogue.
    pub selection: Selection,
    /// The frequency the fleet's products are bucketed at.
    pub frequency: Frequency,
    /// The fleet's health policy, applied to every home's run.
    pub health_policy: hares_io::HealthPolicy,
}

/// The fleet's coverage weights, by home status.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Coverage {
    /// Every home's weight.
    pub weight_total: f64,
    /// The `ok` homes' weight.
    pub weight_ok: f64,
    /// The `flagged` homes' weight.
    pub weight_flagged: f64,
    /// The `failed` homes' weight.
    pub weight_failed: f64,
}

/// One home's row of the fleet result: the identity, the run's status and
/// the fold bookkeeping.
#[derive(Debug, Clone, PartialEq)]
pub struct HomeResult {
    /// The home's building id.
    pub bldg_id: i64,
    /// The home's weight.
    pub weight: f64,
    /// The run's status.
    pub status: SimStatus,
    /// Why the home was flagged or failed; `None` for an `ok` home.
    pub reason: Option<String>,
    /// Steps the home ran (the run's configured step count); 0 for a home
    /// that failed to build or run.
    pub steps: u64,
    /// Aggregate periods in which the home entered the fold.
    pub periods_folded: usize,
    /// The run's metrics, when the home's config requested them.
    pub metrics: Option<hares_core::RunMetrics>,
}

/// The fleet's result: the aggregate, the covered weight behind it, the
/// homes table and the coverage weights.
#[derive(Debug, Clone)]
pub struct FleetResult {
    /// The weighted fleet timeseries: `Time` (Utf8 RFC 3339 UTC labels)
    /// then one `Float64` column per folded variable.
    pub aggregate: RecordBatch,
    /// The weight behind each aggregate cell: `Time` then the same
    /// variable columns.
    pub covered_weight: RecordBatch,
    /// One row per home, in entry order.
    pub homes: Vec<HomeResult>,
    /// The homes' weights by status.
    pub coverage: Coverage,
    /// Homes that succeeded: `ok` and `flagged` at this commit.
    pub n_succeeded: usize,
    /// The largest number of home results held at once, bounded by the
    /// chunk size by construction.
    pub peak_live_home_results: usize,
}

impl FleetResult {
    /// The homes table as a batch: one row per home, in entry order, with
    /// the run metrics' columns when any home requested them.
    #[must_use]
    pub fn homes_table(&self) -> RecordBatch {
        homes_batch(&self.homes)
    }
}

/// The two-stage fleet rule of one unit: the over-time rule and the
/// across-homes rule. Keyed on the unit, the same keying the parent
/// commit's two tables used, until FL-5 declares the kinds.
pub(crate) fn unit_keyed_fleet_rules(
    unit: hares_types::Unit,
) -> (TimeRule, hares_types::AcrossHomes) {
    // Over time: `kWh` sums (the period's energy accumulates) and every
    // other unit averages. Across homes: the intensive units take the
    // weighted mean and every other unit the weighted sum.
    let over_time = if matches!(unit, hares_types::Unit::KilowattHour) {
        Aggregation::Sum
    } else {
        Aggregation::Mean
    };
    let across = match unit {
        hares_types::Unit::Celsius
        | hares_types::Unit::Dimensionless
        | hares_types::Unit::WattPerSquareMeter
        | hares_types::Unit::Second
        | hares_types::Unit::Degree
        | hares_types::Unit::KilowattHourPerMile
        | hares_types::Unit::Enum => hares_types::AcrossHomes::Mean,
        _ => hares_types::AcrossHomes::Sum,
    };
    (
        TimeRule {
            aggregation: over_time,
            enum_codes: None,
        },
        across,
    )
}

/// The fleet rules of a report schema field, read from the field's `unit`
/// metadata (the `Unit::symbol()` string the schema builders write). A
/// field without the metadata, or with a symbol outside the table,
/// averages over time and sums across homes: the parent commit's
/// fallback.
pub(crate) fn field_fleet_rules(field: &Field) -> (TimeRule, hares_types::AcrossHomes) {
    use hares_types::Unit;
    let unit = match field.metadata().get("unit").map(String::as_str) {
        Some("kWh") => Unit::KilowattHour,
        Some("kW") => Unit::Kilowatt,
        Some("C") | Some("\u{b0}C") => Unit::Celsius,
        Some("W") => Unit::Watt,
        Some("-") => Unit::Dimensionless,
        Some("therms/hour") => Unit::ThermPerHour,
        Some("kVAR") => Unit::VoltAmpereReactive,
        Some("W/m2") => Unit::WattPerSquareMeter,
        Some("s") => Unit::Second,
        Some("deg") => Unit::Degree,
        Some("kWh/mi") => Unit::KilowattHourPerMile,
        Some("enum") => Unit::Enum,
        // Any other unit, including no metadata and the empty unit.
        _ => Unit::None,
    };
    unit_keyed_fleet_rules(unit)
}

/// One cell's running sums: the weighted values and the covered weight,
/// accumulated home by home in entry order.
#[derive(Debug, Clone, Copy, Default)]
struct Cell {
    sum_vw: f64,
    weight: f64,
}

/// The running aggregate the fold streams home results into.
#[derive(Debug)]
pub(crate) struct FleetFold {
    /// The folded variables' fields, from the first folded home's report
    /// schema: the aggregate's columns.
    columns: Vec<Field>,
    /// The across-homes rule per column.
    rules: Vec<hares_types::AcrossHomes>,
    /// Per bucket, per column, the running sums.
    cells: BTreeMap<i64, Vec<Cell>>,
    /// The buckets every folding home has a row for: the aggregate's rows.
    intersection: BTreeSet<i64>,
    /// Homes folded so far (a nonempty reduced table each).
    folded: usize,
    /// Whether any home's variables differed from the first folded
    /// home's: the aggregate empties, as today, until FL-2.
    mismatched: bool,
    /// Whether any folding home carried a positive weight.
    has_positive_weight: bool,
}

impl Default for FleetFold {
    fn default() -> Self {
        Self::new()
    }
}

impl FleetFold {
    pub(crate) fn new() -> Self {
        Self {
            columns: Vec::new(),
            rules: Vec::new(),
            cells: BTreeMap::new(),
            intersection: BTreeSet::new(),
            folded: 0,
            mismatched: false,
            has_positive_weight: false,
        }
    }

    /// Folds one home's reduced table into the running aggregate, in entry
    /// order. Each cell adds `v * w` and `w` for the home: the operations
    /// and their order the collect-then-aggregate fold performed.
    ///
    /// # Errors
    ///
    /// [`FleetError::InvalidAggregationWeight`] for a weight that is not
    /// finite and non-negative: the primary ingestion boundaries validate
    /// weights already, so this is the defense-in-depth the aggregate
    /// carried.
    pub(crate) fn fold_home(
        &mut self,
        index: usize,
        weight: f64,
        fields: &[Field],
        table: &BTreeMap<i64, Vec<Option<f64>>>,
    ) -> Result<(), FleetError> {
        if classify_sample_weight(weight) == hares_io::SampleWeightClass::Invalid {
            return Err(FleetError::InvalidAggregationWeight {
                index,
                value: weight,
            });
        }
        if table.is_empty() {
            return Ok(());
        }
        if self.folded == 0 {
            self.columns = fields.to_vec();
            self.rules = self
                .columns
                .iter()
                .map(field_fleet_rules)
                .map(|(_, across)| across)
                .collect();
            self.intersection = table.keys().copied().collect();
            self.has_positive_weight = weight > 0.0;
        } else {
            if fields != self.columns {
                self.mismatched = true;
            }
            self.has_positive_weight = self.has_positive_weight || weight > 0.0;
            self.intersection
                .retain(|bucket| table.contains_key(bucket));
        }
        self.folded += 1;
        if self.mismatched {
            return Ok(());
        }
        for (&bucket, values) in table {
            let cells = self
                .cells
                .entry(bucket)
                .or_insert_with(|| vec![Cell::default(); self.columns.len()]);
            for (slot, value) in values.iter().enumerate() {
                let Some(value) = value else {
                    continue;
                };
                let cell = &mut cells[slot];
                cell.sum_vw += *value * weight;
                cell.weight += weight;
            }
        }
        Ok(())
    }

    /// The number of homes that folded at least one value.
    pub(crate) fn folded(&self) -> usize {
        self.folded
    }

    /// Whether any folding home carried a positive weight: the check the
    /// parent commit's aggregate made over its successful homes.
    pub(crate) fn has_positive_weight(&self) -> bool {
        self.has_positive_weight
    }

    /// Closes the fold: the fleet result's aggregate and covered weight,
    /// and the count of aggregate periods (0 when the fold is empty or a
    /// variable mismatch emptied it). A variable mismatch between homes
    /// empties both tables, as today, until FL-2.
    pub(crate) fn finish(self) -> (RecordBatch, RecordBatch, usize) {
        let periods = if self.mismatched {
            0
        } else {
            self.intersection.len()
        };
        if self.folded == 0 || self.mismatched {
            return (empty_table(), empty_table(), periods);
        }
        (self.build_table(true), self.build_table(false), periods)
    }

    /// Builds one of the two per-period tables: the aggregate's cells by
    /// their across-homes rule, or the covered weight behind them. Both
    /// walk the intersection's buckets in ascending order and read the
    /// same running sums.
    fn build_table(&self, aggregate: bool) -> RecordBatch {
        let mut timestamps = StringBuilder::new();
        let mut builders: Vec<Float64Builder> = (0..self.columns.len())
            .map(|_| Float64Builder::new())
            .collect();

        for &bucket in &self.intersection {
            let Some(instant) = DateTime::from_timestamp(bucket, 0) else {
                continue;
            };
            timestamps.append_value(
                instant
                    .fixed_offset()
                    .to_rfc3339_opts(SecondsFormat::Secs, true),
            );
            let cells = self.cells.get(&bucket).map_or(&[][..], Vec::as_slice);
            for (slot, builder) in builders.iter_mut().enumerate() {
                let Some(cell) = cells.get(slot).copied() else {
                    builder.append_null();
                    continue;
                };
                if !aggregate {
                    builder.append_value(cell.weight);
                    continue;
                }
                // A cell whose covered weight is zero is null, whatever
                // its rule.
                match (cell.weight > 0.0, self.rules[slot]) {
                    (false, _) => builder.append_null(),
                    (true, hares_types::AcrossHomes::Sum) => builder.append_value(cell.sum_vw),
                    (true, hares_types::AcrossHomes::Mean) => {
                        builder.append_value(cell.sum_vw / cell.weight);
                    }
                }
            }
        }

        let mut fields = vec![Field::new("Time", DataType::Utf8, false)];
        fields.extend(self.columns.iter().cloned());
        let schema = Arc::new(Schema::new(fields));
        let mut arrays: Vec<Arc<dyn Array>> = Vec::with_capacity(self.columns.len() + 1);
        arrays.push(Arc::new(timestamps.finish()));
        for mut builder in builders {
            arrays.push(Arc::new(builder.finish()));
        }
        RecordBatch::try_new(schema, arrays).unwrap_or_else(|_| empty_table())
    }
}

/// The empty per-period table: the `Time` label only, zero rows. The
/// schema an emptied aggregate carries.
fn empty_table() -> RecordBatch {
    let schema = Arc::new(Schema::new(vec![Field::new("Time", DataType::Utf8, false)]));
    RecordBatch::try_new(
        schema,
        vec![Arc::new(StringArray::from(Vec::<&str>::new()))],
    )
    .expect("empty aggregate table")
}

/// One reduced period the home's reducer emitted: the naive local
/// wall-clock start, the reduced value per rule and the valid step count
/// per rule.
#[derive(Debug, Clone)]
pub(crate) struct PeriodRowData {
    pub start_local_ms: i64,
    pub values: Vec<f64>,
    pub valid_steps: Vec<u32>,
}

impl PeriodRowData {
    fn from_row(row: &PeriodRow<'_>) -> Self {
        Self {
            start_local_ms: row.start_local_ms,
            values: row.values.to_vec(),
            valid_steps: row.valid_steps.to_vec(),
        }
    }

    /// The period's values as the fold reads them: `None` where the rule
    /// carried no valid step. A null value contributes to neither the
    /// cell's weighted sum nor its weight.
    fn values(&self) -> Vec<Option<f64>> {
        self.values
            .iter()
            .zip(&self.valid_steps)
            .map(|(&value, &valid)| (valid > 0).then_some(value))
            .collect()
    }
}

/// One home's fold state: the period reducer the fleet owns and the rows
/// it emitted. Shared between the run's batch callback and the fold.
#[derive(Debug)]
pub(crate) struct HomeFoldState {
    frequency: Frequency,
    /// The reducer, built from the first batch's schema (the report's
    /// columns and their units).
    reducer: Option<PeriodReducer>,
    /// The rules, once known.
    rules: Option<Vec<TimeRule>>,
    /// The folded variables' fields, from the first batch's schema, the
    /// `Time` column excluded.
    fields: Option<Vec<Field>>,
    rows: Vec<PeriodRowData>,
}

impl HomeFoldState {
    pub(crate) fn new(frequency: Frequency) -> Self {
        Self {
            frequency,
            reducer: None,
            rules: None,
            fields: None,
            rows: Vec::new(),
        }
    }

    /// Closes the home's reducer: the last partial period emits, and the
    /// reduced table is keyed by the period start's instant (today's
    /// epoch key: the naive local period start read as UTC seconds, minus
    /// the home's `hares:utc_offset_s`).
    pub(crate) fn finish(
        &mut self,
        utc_offset_s: i32,
    ) -> (BTreeMap<i64, Vec<Option<f64>>>, Vec<Field>) {
        if let Some(reducer) = self.reducer.take() {
            reducer.finish(&mut |row| self.rows.push(PeriodRowData::from_row(&row)));
        }
        let fields = self.fields.clone().unwrap_or_default();
        let mut table = BTreeMap::new();
        for row in self.rows.drain(..) {
            let naive_secs = DateTime::from_timestamp_millis(row.start_local_ms)
                .unwrap_or_else(|| {
                    panic!(
                        "local milliseconds {} out of the chrono range",
                        row.start_local_ms
                    )
                })
                .naive_utc()
                .and_utc()
                .timestamp();
            table.insert(naive_secs - i64::from(utc_offset_s), row.values());
        }
        (table, fields)
    }
}

/// The batch callback the fleet attaches to every home's `fleet` report:
/// it feeds each delivered batch into the home's period reducer and drops
/// the batch, so a home's result in the fold is its reduced period table,
/// never its frame.
#[derive(Debug, Clone)]
pub(crate) struct ReducerFeed {
    state: Arc<Mutex<HomeFoldState>>,
}

impl ReducerFeed {
    pub(crate) fn new(state: Arc<Mutex<HomeFoldState>>) -> Self {
        Self { state }
    }
}

impl BatchCallback for ReducerFeed {
    fn on_batch(
        &mut self,
        _ctx: &BatchContext<'_>,
        batch: &RecordBatch,
    ) -> Result<(), CallbackError> {
        let mut state = self.state.lock().map_err(|poisoned| {
            CallbackError::new(format!("the home's fold state is poisoned: {poisoned}"))
        })?;
        if state.fields.is_none() {
            state.fields = Some(
                batch
                    .schema()
                    .fields()
                    .iter()
                    .skip(1)
                    .map(|field| field.as_ref().clone())
                    .collect(),
            );
        }
        if state.rules.is_none() {
            let rules: Vec<TimeRule> = state
                .fields
                .as_ref()
                .expect("fields set above")
                .iter()
                .map(|field| field_fleet_rules(field).0)
                .collect();
            let reducer = PeriodReducer::new(state.frequency, PeriodCalendar, rules.clone())
                .map_err(|err| CallbackError::new(err.to_string()))?;
            state.rules = Some(rules);
            state.reducer = Some(reducer);
        }
        let mut reducer = state.reducer.take().expect("the reducer is built above");
        let mut closed: Vec<PeriodRowData> = Vec::new();
        let feed = reducer.push_batch(batch, &mut |row: PeriodRow<'_>| {
            closed.push(PeriodRowData::from_row(&row));
        });
        state.reducer = Some(reducer);
        feed.map_err(|err: ReduceError| CallbackError::new(err.to_string()))?;
        state.rows.extend(closed);
        Ok(())
    }
}

/// The homes table as a batch: one row per home, in entry order, with the
/// run metrics' columns when any home requested them.
fn homes_batch(homes: &[HomeResult]) -> RecordBatch {
    let any_metrics = homes.iter().any(|home| home.metrics.is_some());
    let mut bldg_ids = arrow::array::Int64Builder::new();
    let mut weights = Float64Builder::new();
    let mut statuses = StringBuilder::new();
    let mut reasons = StringBuilder::new();
    let mut steps = arrow::array::Int64Builder::new();
    let mut periods = arrow::array::Int64Builder::new();
    let mut total_energy = Float64Builder::new();
    let mut peak_power = Float64Builder::new();

    for home in homes {
        bldg_ids.append_value(home.bldg_id);
        weights.append_value(home.weight);
        statuses.append_value(match &home.status {
            SimStatus::Ok => "ok",
            SimStatus::Flagged(_) => "flagged",
            SimStatus::Failed(_) => "failed",
        });
        match &home.status {
            SimStatus::Ok => reasons.append_null(),
            SimStatus::Flagged(reason) | SimStatus::Failed(reason) => {
                reasons.append_value(reason.as_str())
            }
        }
        steps.append_value(i64::try_from(home.steps).unwrap_or(i64::MAX));
        periods.append_value(home.periods_folded as i64);
        let metrics = home.metrics.as_ref();
        match metrics.map(|m| m.total_energy_kwh.net_energy_kwh) {
            Some(value) => total_energy.append_value(value),
            None if any_metrics => total_energy.append_null(),
            None => total_energy.append_value(0.0),
        }
        match metrics.map(|m| m.grid_interaction_metrics.peak_import_kw) {
            Some(value) => peak_power.append_value(value),
            None if any_metrics => peak_power.append_null(),
            None => peak_power.append_value(0.0),
        }
    }

    let mut fields = vec![
        Field::new("bldg_id", DataType::Int64, false),
        Field::new("weight", DataType::Float64, false),
        Field::new("status", DataType::Utf8, false),
        Field::new("reason", DataType::Utf8, true),
        Field::new("steps", DataType::Int64, false),
        Field::new("periods_folded", DataType::Int64, false),
    ];
    let mut arrays: Vec<Arc<dyn Array>> = vec![
        Arc::new(bldg_ids.finish()),
        Arc::new(weights.finish()),
        Arc::new(statuses.finish()),
        Arc::new(reasons.finish()),
        Arc::new(steps.finish()),
        Arc::new(periods.finish()),
    ];
    if any_metrics {
        fields.push(Field::new("total_energy_kwh", DataType::Float64, true));
        fields.push(Field::new("peak_power_kw", DataType::Float64, true));
        arrays.push(Arc::new(total_energy.finish()));
        arrays.push(Arc::new(peak_power.finish()));
    }
    let schema = Arc::new(Schema::new(fields));
    RecordBatch::try_new(schema, arrays).expect("the homes table is well formed")
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{Array, Float64Array};
    use hares_types::Unit;

    fn field(name: &str, unit: &str) -> Field {
        Field::new(name, DataType::Float64, true).with_metadata(
            [("unit".to_string(), unit.to_string())]
                .into_iter()
                .collect(),
        )
    }

    /// Two variables: a power column (weighted sum across homes) and a
    /// temperature column (weighted mean across homes).
    fn two_fields() -> Vec<Field> {
        vec![
            field("Total Electric Power (kW)", "kW"),
            field("Temperature - Indoor (C)", "C"),
        ]
    }

    /// One period's row for the two variables.
    fn row(power: Option<f64>, temp: Option<f64>) -> Vec<Option<f64>> {
        vec![power, temp]
    }

    fn column(batch: &RecordBatch, index: usize) -> Vec<Option<f64>> {
        let values = batch
            .column(index)
            .as_any()
            .downcast_ref::<Float64Array>()
            .expect("a Float64 column");
        (0..values.len())
            .map(|row| (!values.is_null(row)).then(|| values.value(row)))
            .collect()
    }

    fn time_labels(batch: &RecordBatch) -> Vec<String> {
        let values = batch
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("a Utf8 Time column");
        (0..values.len())
            .map(|row| values.value(row).to_string())
            .collect()
    }

    #[test]
    fn unit_rules_match_the_parent_tables() {
        // Every unit of the census maps to the parent commit's two-stage
        // rule: over time, kWh sums and everything else means; across
        // homes, the intensive units take the weighted mean and every
        // other unit the weighted sum.
        let means_across = [
            ("C", Unit::Celsius),
            ("\u{b0}C", Unit::Celsius),
            ("-", Unit::Dimensionless),
            ("W/m2", Unit::WattPerSquareMeter),
            ("s", Unit::Second),
            ("deg", Unit::Degree),
            ("kWh/mi", Unit::KilowattHourPerMile),
            ("enum", Unit::Enum),
        ];
        for (spelling, unit) in means_across {
            let (over_time, across) = unit_keyed_fleet_rules(unit);
            assert!(
                matches!(over_time.aggregation, Aggregation::Mean),
                "{spelling}"
            );
            assert!(
                matches!(across, hares_types::AcrossHomes::Mean),
                "{spelling}"
            );
        }
        let sums_across = [
            ("kWh", Unit::KilowattHour),
            ("kW", Unit::Kilowatt),
            ("W", Unit::Watt),
            ("therms/hour", Unit::ThermPerHour),
            ("kVAR", Unit::VoltAmpereReactive),
            // The parent commit's fallback: any other unit averages over
            // time and sums across homes.
            ("fraction", Unit::Fraction),
            ("V", Unit::Volt),
            ("count", Unit::Count),
            ("", Unit::None),
        ];
        for (spelling, unit) in sums_across {
            let (over_time, across) = unit_keyed_fleet_rules(unit);
            let expected_over_time = if matches!(unit, Unit::KilowattHour) {
                Aggregation::Sum
            } else {
                Aggregation::Mean
            };
            assert_eq!(over_time.aggregation, expected_over_time, "{spelling}");
            assert!(
                matches!(across, hares_types::AcrossHomes::Sum),
                "{spelling}"
            );
        }
    }

    /// Three homes, one bucket: the fold's cells accumulate `v * w` and
    /// `w` home by home in entry order, and the mean divides once after
    /// the last home. The hand-computed sums use the same order.
    #[test]
    fn fold_accumulates_cells_in_entry_order() {
        let fields = two_fields();
        let key = 1_700_000_000_i64;
        let homes: [(f64, Vec<Option<f64>>); 3] = [
            (1.0, row(Some(0.1), Some(20.0))),
            (2.5, row(Some(1.5), Some(21.0))),
            (0.5, row(Some(10.0), Some(0.5))),
        ];

        let mut fold = FleetFold::new();
        for (index, (weight, values)) in homes.iter().enumerate() {
            let mut table = BTreeMap::new();
            table.insert(key, values.clone());
            fold.fold_home(index, *weight, &fields, &table)
                .expect("valid weight");
        }
        let (aggregate, covered_weight, periods) = fold.finish();

        assert_eq!(periods, 1);
        let power = column(&aggregate, 1);
        let temp = column(&aggregate, 2);
        let expected_power: f64 = 0.1 * 1.0 + 1.5 * 2.5 + 10.0 * 0.5;
        let expected_temp: f64 = (20.0 * 1.0 + 21.0 * 2.5 + 0.5 * 0.5) / (1.0 + 2.5 + 0.5);
        assert_eq!(power[0].unwrap().to_bits(), expected_power.to_bits());
        assert_eq!(temp[0].unwrap().to_bits(), expected_temp.to_bits());
        let weight = column(&covered_weight, 1);
        assert_eq!(weight[0].unwrap().to_bits(), 4.0_f64.to_bits());
        assert_eq!(
            time_labels(&aggregate),
            vec!["2023-11-14T22:13:20Z".to_string()]
        );
    }

    /// The chunk barrier must not disturb the accumulation: the tables
    /// folded from one collected set and the tables folded one at a time
    /// give the same aggregate bits. This is the ordered-merge
    /// differential the row's acceptance test runs at fleet scale; the
    /// unit-level form runs here.
    #[test]
    fn chunked_fold_matches_the_unbounded_fold_on_bits() {
        let fields = two_fields();
        // Values whose entry-order sums are order-sensitive: a fold that
        // broke the entry order cannot pass on bits.
        let powers = [
            0.1,
            1.0,
            1.5,
            2.0,
            10.0,
            0.3,
            7.0,
            1.0 / 3.0,
            5.0,
            0.7,
            2.5,
            3.0,
        ];
        let temps = [
            20.0,
            -5.5,
            21.0,
            1.0 / 7.0,
            0.5,
            19.0,
            4.25,
            30.0,
            18.5,
            2.0,
            22.5,
            17.0,
        ];
        let weights = [1.0, 2.5, 0.5, 3.0, 1.5, 0.25, 4.0, 0.1, 2.0, 1.0, 5.0, 0.75];
        let keys: [i64; 3] = [1_700_000_000, 1_700_000_900, 1_700_001_800];

        let mut tables: Vec<BTreeMap<i64, Vec<Option<f64>>>> = Vec::new();
        for (home, _) in weights.iter().enumerate() {
            let mut table = BTreeMap::new();
            for (period, &key) in keys.iter().enumerate() {
                // One home carries a null in the middle period, to
                // exercise the covered weight.
                let null_period = home == 4 && period == 1;
                let power = Some(powers[home] + period as f64);
                let temp = if null_period {
                    None
                } else {
                    Some(temps[home] + period as f64)
                };
                table.insert(key, row(power, temp));
            }
            tables.push(table);
        }

        // The unbounded reference: every table folded from one collected
        // set, in entry order.
        let mut unbounded = FleetFold::new();
        for (index, table) in tables.iter().enumerate() {
            unbounded
                .fold_home(index, weights[index], &fields, table)
                .expect("valid weight");
        }
        let (unbounded_aggregate, unbounded_weight, unbounded_periods) = unbounded.finish();

        // The bounded fold: one home's table folded and dropped at a
        // time, chunks of one.
        let mut bounded = FleetFold::new();
        for (index, table) in tables.iter().enumerate() {
            bounded
                .fold_home(index, weights[index], &fields, table)
                .expect("valid weight");
        }
        let (bounded_aggregate, bounded_weight, bounded_periods) = bounded.finish();

        assert_eq!(unbounded_periods, bounded_periods);
        assert_eq!(
            unbounded_aggregate.num_columns(),
            bounded_aggregate.num_columns()
        );
        for slot in 0..unbounded_aggregate.num_columns() {
            let left = unbounded_aggregate.column(slot).to_data();
            let right = bounded_aggregate.column(slot).to_data();
            assert_eq!(
                left, right,
                "column {slot} differs between the unbounded and the bounded fold"
            );
        }
        for slot in 0..unbounded_weight.num_columns() {
            let left = unbounded_weight.column(slot).to_data();
            let right = bounded_weight.column(slot).to_data();
            assert_eq!(left, right, "covered weight column {slot} differs");
        }
    }

    /// A null value contributes to neither the cell's weighted sum nor
    /// its weight; a cell whose covered weight is zero is null.
    #[test]
    fn null_values_exclude_weight_and_a_zero_weight_cell_is_null() {
        let fields = two_fields();
        let key = 0_i64;

        let mut home_a = BTreeMap::new();
        home_a.insert(key, row(Some(4.0), Some(20.0)));
        let mut home_b = BTreeMap::new();
        home_b.insert(key, row(None, Some(30.0)));

        let mut fold = FleetFold::new();
        fold.fold_home(0, 1.0, &fields, &home_a)
            .expect("valid weight");
        fold.fold_home(1, 2.0, &fields, &home_b)
            .expect("valid weight");
        let (aggregate, covered_weight, _) = fold.finish();

        // The power cell: home b's null contributes nothing, so the cell
        // is home a's value with covered weight 1.
        assert_eq!(
            column(&aggregate, 1)[0].unwrap().to_bits(),
            4.0_f64.to_bits()
        );
        assert_eq!(
            column(&covered_weight, 1)[0].unwrap().to_bits(),
            1.0_f64.to_bits()
        );
        // The temperature cell: both homes contributed, weight 3.
        let expected: f64 = (20.0 * 1.0 + 30.0 * 2.0) / 3.0;
        assert_eq!(
            column(&aggregate, 2)[0].unwrap().to_bits(),
            expected.to_bits()
        );
        assert_eq!(
            column(&covered_weight, 2)[0].unwrap().to_bits(),
            3.0_f64.to_bits()
        );

        // A cell whose covered weight is zero is null: the only home
        // that reaches the variable carries a null there.
        let mut home_c = BTreeMap::new();
        home_c.insert(key, row(None, None));
        let mut fold = FleetFold::new();
        fold.fold_home(0, 3.0, &fields, &home_c)
            .expect("valid weight");
        let (aggregate, covered_weight, _) = fold.finish();
        assert!(
            column(&aggregate, 1)[0].is_none(),
            "zero covered weight is null"
        );
        assert_eq!(
            column(&covered_weight, 1)[0].unwrap().to_bits(),
            0.0_f64.to_bits()
        );
    }

    /// A variable mismatch between homes empties the aggregate, as today,
    /// until FL-2.
    #[test]
    fn variable_mismatch_empties_the_aggregate() {
        let key = 0_i64;
        let mut home_a = BTreeMap::new();
        home_a.insert(key, row(Some(1.0), Some(20.0)));
        let different = vec![
            field("Other Power (kW)", "kW"),
            field("Temperature - Indoor (C)", "C"),
        ];
        let mut home_b = BTreeMap::new();
        home_b.insert(key, row(Some(2.0), Some(21.0)));

        let mut fold = FleetFold::new();
        fold.fold_home(0, 1.0, &two_fields(), &home_a)
            .expect("valid weight");
        fold.fold_home(1, 1.0, &different, &home_b)
            .expect("valid weight");
        let (aggregate, covered_weight, periods) = fold.finish();

        assert_eq!(periods, 0);
        assert_eq!(aggregate.num_columns(), 1, "the Time label only");
        assert_eq!(aggregate.num_rows(), 0);
        assert_eq!(covered_weight.num_rows(), 0);
    }

    /// A nonempty fold with an empty intersection keeps the full schema
    /// at zero rows.
    #[test]
    fn empty_intersection_keeps_the_schema() {
        let fields = two_fields();
        let mut home_a = BTreeMap::new();
        home_a.insert(0_i64, row(Some(1.0), Some(20.0)));
        let mut home_b = BTreeMap::new();
        home_b.insert(900_i64, row(Some(2.0), Some(21.0)));

        let mut fold = FleetFold::new();
        fold.fold_home(0, 1.0, &fields, &home_a)
            .expect("valid weight");
        fold.fold_home(1, 1.0, &fields, &home_b)
            .expect("valid weight");
        let (aggregate, covered_weight, periods) = fold.finish();

        assert_eq!(periods, 0);
        assert_eq!(aggregate.num_columns(), 3, "Time plus the two variables");
        assert_eq!(aggregate.num_rows(), 0);
        assert_eq!(covered_weight.num_rows(), 0);
    }

    /// An invalid weight among the folding homes is the aggregate's
    /// defense-in-depth error, named with the home's entry index.
    #[test]
    fn invalid_weight_is_named_by_entry_index() {
        let fields = two_fields();
        let mut home = BTreeMap::new();
        home.insert(0_i64, row(Some(1.0), Some(20.0)));

        let mut fold = FleetFold::new();
        let err = fold
            .fold_home(7, f64::NAN, &fields, &home)
            .expect_err("a NaN weight is rejected");
        assert!(
            matches!(err, FleetError::InvalidAggregationWeight { index: 7, .. }),
            "got: {err:?}"
        );
    }

    /// A home with no reduced rows folds no value: the fold's homes count
    /// and the intersection exclude it, and its status stands on its own.
    #[test]
    fn a_home_without_rows_folds_nothing() {
        let fields = two_fields();
        let mut home_a = BTreeMap::new();
        home_a.insert(0_i64, row(Some(1.0), Some(20.0)));
        let empty: BTreeMap<i64, Vec<Option<f64>>> = BTreeMap::new();

        let mut fold = FleetFold::new();
        fold.fold_home(0, 1.0, &fields, &empty)
            .expect("valid weight");
        fold.fold_home(1, 2.0, &fields, &home_a)
            .expect("valid weight");
        let folded = fold.folded();
        let (aggregate, _, periods) = fold.finish();

        assert_eq!(folded, 1, "only the home with rows folded");
        assert_eq!(periods, 1);
        assert_eq!(
            column(&aggregate, 1)[0].unwrap().to_bits(),
            2.0_f64.to_bits()
        );
    }
}
