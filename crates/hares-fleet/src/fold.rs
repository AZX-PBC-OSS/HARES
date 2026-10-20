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

use arrow::array::{Array, Float64Builder, StringBuilder, TimestampMillisecondBuilder};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use arrow::record_batch::RecordBatch;
use chrono::{DateTime, FixedOffset, TimeZone};
use hares_io::Frequency;
use hares_io::output::callback::{BatchCallback, BatchContext, CallbackError};
use hares_io::output::reduce::{
    PeriodCalendar, PeriodReducer, PeriodRow, ReduceError, TimeRule, mode_winner,
};
use hares_io::{Selection, classify_sample_weight};
use hares_types::Aggregation;

use crate::fleet::{FleetError, HomeStatus};

/// The fleet's declared time alignment: the calendar the homes'
/// reductions bucket onto and the rows the fold aligns by. The decision's
/// reference zone is a fixed UTC offset, because HARES output time is
/// naive local standard time with no daylight saving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alignment {
    /// Rows align by the period's start instant: every home's reduction
    /// buckets on the declared offset's period boundaries, whatever its
    /// own site offset, and the labels are that instant's wall clock at
    /// the declared offset, a naive `Timestamp` (B-12).
    Instant(FixedOffset),
    /// Rows align by the naive local period start: every home reduces on
    /// its own local calendar and the labels are the local wall clock
    /// each folded home's step starts at, a naive `Timestamp`.
    LocalClock,
}

/// The aggregate's row alignment: the keys the buckets are filed under
/// and the labels they render as. The fleet's declared alignment decides,
/// or, undeclared, the homes' shared site offset keeps today's instant
/// keys and the labels in the homes' local wall clock. Every label is a
/// naive local `Timestamp(Millisecond, None)` at the period's start
/// (`09-numeric-fixes.md` B-12): the instant keys render at the
/// alignment's offset (a declared reference zone, or the homes' shared
/// site offset), the local keys are the wall clock itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FoldRows {
    /// Buckets keyed by the period's start instant; labels render the
    /// instant's wall clock at the offset, naive (the declared reference
    /// zone under a declared alignment, the homes' shared site offset
    /// under the undeclared single-offset default).
    Instant(FixedOffset),
    /// Buckets keyed by the naive local period start; labels are that
    /// wall clock, because the homes share no offset and the row is the
    /// local time each folded home's step starts at.
    LocalClock,
}

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
    /// The `degraded` homes' weight: homes that finished but carry a
    /// warning or a health event. Never part of `weight_ok`.
    pub weight_degraded: f64,
    /// The `failed` homes' weight.
    pub weight_failed: f64,
}

/// The declared code a value carries, or `None` when it is no code in
/// the label table: finite, integral, below the label count. The same
/// in-range rule the reporting reducer's Mode applies to a step value.
fn mode_code(value: f64, labels: usize) -> Option<usize> {
    (value.is_finite() && value >= 0.0 && value.fract() == 0.0 && (value as usize) < labels)
        .then_some(value as usize)
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
    pub status: HomeStatus,
    /// Why the home was degraded or failed; `None` for an `ok` home.
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
    /// The weighted fleet timeseries: `Time` (a naive local
    /// `Timestamp(Millisecond, None)` at each period's start, B-12) then
    /// one `Float64` column per folded variable.
    pub aggregate: RecordBatch,
    /// The weight behind each aggregate cell: `Time` then the same
    /// variable columns.
    pub covered_weight: RecordBatch,
    /// One row per home, in entry order.
    pub homes: Vec<HomeResult>,
    /// The homes' weights by status.
    pub coverage: Coverage,
    /// The `ok` homes' count: a degraded or failed home is never a
    /// success.
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

/// The fleet rules of a report schema field, by the variable's declared
/// kind: the schema builders write the declaration's `kind`, over-time
/// `aggregation` and `across_homes` tags into the field metadata, and
/// both stages read them.
///
/// An Enum-kind column (its label count in the field's metadata, which
/// the schema builders write beside the unit) folds by Mode in both
/// stages, whatever its unit string: the mode columns' unit is the
/// dimensionless `-`, so no unit string identifies them. Over time the
/// rule is the reporting reducer's own Mode (the most frequent code, ties
/// to the lowest code), fed the enum's label count so it can count per
/// code; across homes the fold reads the same label count and takes the
/// code with the greatest summed weight. The across-homes rule returned
/// for an Enum column is therefore inert (the fold reads the label count
/// instead).
///
/// Every other declared field: over time the variable's declared
/// aggregation (an energy or a quantity declared `Sum` sums over time, a
/// running total takes its last value); across homes, energy and power
/// the weighted sum, temperature, ratio and state the weighted mean, a
/// quantity its declared across-homes rule.
///
/// A field without the declared-kind metadata is a contract violation:
/// [`FleetError::UndeclaredKind`] names it. The schema builders write
/// the tag for every catalogue variable, so a field without one is
/// hand-built, and the fold refuses to guess its rules from the column
/// name.
pub(crate) fn field_fleet_rules(
    field: &Field,
) -> Result<(TimeRule, hares_types::AcrossHomes), FleetError> {
    if let Some(labels) = field
        .metadata()
        .get("enum_labels")
        .and_then(|count| count.parse::<usize>().ok())
    {
        return Ok((
            TimeRule {
                aggregation: Aggregation::Mode,
                enum_codes: Some(labels),
            },
            hares_types::AcrossHomes::Mean,
        ));
    }
    if let Some(kind) = field
        .metadata()
        .get("kind")
        .and_then(|tag| hares_types::VariableKind::from_tag(tag.as_str()))
    {
        let over_time = field
            .metadata()
            .get("aggregation")
            .and_then(|tag| hares_types::Aggregation::from_tag(tag.as_str()))
            .unwrap_or(Aggregation::Mean);
        let across = match &kind {
            hares_types::VariableKind::Energy | hares_types::VariableKind::Power => {
                hares_types::AcrossHomes::Sum
            }
            hares_types::VariableKind::Temperature
            | hares_types::VariableKind::Ratio
            | hares_types::VariableKind::State => hares_types::AcrossHomes::Mean,
            hares_types::VariableKind::Quantity { .. } => field
                .metadata()
                .get("across_homes")
                .and_then(|tag| hares_types::AcrossHomes::from_tag(tag.as_str()))
                .unwrap_or(hares_types::AcrossHomes::Sum),
            // The label count above identifies an Enum column; the fold
            // reads it, so the returned across-homes rule is inert.
            hares_types::VariableKind::Enum { .. } => hares_types::AcrossHomes::Mean,
        };
        return Ok((
            TimeRule {
                aggregation: over_time,
                enum_codes: None,
            },
            across,
        ));
    }
    Err(FleetError::UndeclaredKind {
        name: field.name().clone(),
    })
}

/// One cell's running sums: the weighted values and the covered weight,
/// accumulated home by home in entry order. An Enum column's cell carries
/// the summed weight per code instead of a value sum: the folded cell is
/// the code with the greatest summed weight, never a mean over codes.
#[derive(Debug, Clone, Default)]
struct Cell {
    sum_vw: f64,
    weight: f64,
    /// The summed weight per code for an Enum column (empty for every
    /// other kind), sized to the column's label count at the first vote.
    counts: Vec<f64>,
}

/// The running aggregate the fold streams home results into.
#[derive(Debug)]
pub(crate) struct FleetFold {
    /// The row alignment the buckets render as.
    rows: FoldRows,
    /// The folded variables' fields: the union of the folding homes'
    /// report schemas, in order of first appearance in entry order.
    columns: Vec<Field>,
    /// The across-homes rule per column.
    rules: Vec<hares_types::AcrossHomes>,
    /// The label count per column for an Enum column (`None` for every
    /// other kind): the across-homes fold of an Enum column takes the
    /// code with the greatest summed weight, so it needs the code space.
    enum_labels: Vec<Option<usize>>,
    /// Per bucket, per column, the running sums.
    cells: BTreeMap<i64, Vec<Cell>>,
    /// The buckets every folding home has a row for: the aggregate's rows.
    intersection: BTreeSet<i64>,
    /// Homes folded so far (a nonempty reduced table each).
    folded: usize,
}

impl Default for FleetFold {
    fn default() -> Self {
        Self::new(FoldRows::Instant(
            FixedOffset::east_opt(0).expect("a zero offset is representable"),
        ))
    }
}

impl FleetFold {
    pub(crate) fn new(rows: FoldRows) -> Self {
        Self {
            rows,
            columns: Vec::new(),
            rules: Vec::new(),
            enum_labels: Vec::new(),
            cells: BTreeMap::new(),
            intersection: BTreeSet::new(),
            folded: 0,
        }
    }

    /// Folds one home's reduced table into the running aggregate, in entry
    /// order. The columns are the union of the folding homes' variables in
    /// order of first appearance; a variable a home does not declare gets
    /// neither value nor weight from it. Each cell adds `v * w` and `w`
    /// for the home (an Enum column's cell the code's weight and `w`):
    /// the operations and their order the collect-then-aggregate fold
    /// performed.
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
            self.intersection = table.keys().copied().collect();
        } else {
            self.intersection
                .retain(|bucket| table.contains_key(bucket));
        }
        let grew = !fields.is_empty()
            && fields.iter().any(|field| {
                !self
                    .columns
                    .iter()
                    .any(|column| column.name() == field.name())
            });
        for field in fields {
            if !self
                .columns
                .iter()
                .any(|column| column.name() == field.name())
            {
                let (over_time, across) = field_fleet_rules(field)?;
                self.rules.push(across);
                self.enum_labels.push(over_time.enum_codes);
                self.columns.push(field.clone());
                tracing::debug!(
                    column = field.name().as_str(),
                    kind = ?field.metadata().get("kind"),
                    across = ?across,
                    enum_labels = ?over_time.enum_codes,
                    "the fleet fold takes the column's across-homes rule"
                );
            }
        }
        self.folded += 1;
        for (&bucket, values) in table {
            let cells = self
                .cells
                .entry(bucket)
                .or_insert_with(|| vec![Cell::default(); self.columns.len()]);
            if grew {
                cells.resize(self.columns.len(), Cell::default());
            }
            for (home_slot, value) in values.iter().enumerate() {
                let Some(value) = value else {
                    continue;
                };
                let field = &fields[home_slot];
                let slot = self
                    .columns
                    .iter()
                    .position(|column| column.name() == field.name())
                    .expect("the column is unioned above");
                let cell = &mut cells[slot];
                if let Some(labels) = self.enum_labels[slot] {
                    // An Enum column: the home's reduced code votes with
                    // its weight, and a value that is no declared code
                    // votes nothing (no count, no weight), by the same
                    // rule the reducer applies to an out-of-range code.
                    if let Some(code) = mode_code(*value, labels) {
                        if cell.counts.is_empty() {
                            cell.counts = vec![0.0; labels];
                        }
                        cell.counts[code] += weight;
                        cell.weight += weight;
                    }
                } else {
                    cell.sum_vw += *value * weight;
                    cell.weight += weight;
                }
            }
        }
        Ok(())
    }

    /// Closes the fold: the fleet result's aggregate and covered weight,
    /// and the count of aggregate periods. The rows are the periods every
    /// folding home has a row for; the columns are the union.
    pub(crate) fn finish(self) -> (RecordBatch, RecordBatch, usize) {
        if self.folded == 0 {
            // Nothing folded: the emptied tables carry the schema the
            // alignment labels (the `Time` type and its offset metadata).
            return (self.build_table(true), self.build_table(false), 0);
        }
        (
            self.build_table(true),
            self.build_table(false),
            self.intersection.len(),
        )
    }

    /// Builds one of the two per-period tables: the aggregate's cells by
    /// their across-homes rule, or the covered weight behind them. Both
    /// walk the intersection's buckets in ascending order and read the
    /// same running sums. The `Time` column is a naive
    /// `Timestamp(Millisecond, None)` at each period's start (B-12): the
    /// instant keys render at the alignment's offset's wall clock, the
    /// local keys are the wall clock itself, and the schema metadata
    /// records the offset (the declared reference zone, or the homes'
    /// shared site offset) whenever one offset applies.
    fn build_table(&self, aggregate: bool) -> RecordBatch {
        let mut timestamps = TimestampMillisecondBuilder::new();
        let mut builders: Vec<Float64Builder> = (0..self.columns.len())
            .map(|_| Float64Builder::new())
            .collect();

        for &bucket in &self.intersection {
            match self.rows {
                FoldRows::Instant(offset) => {
                    let Some(instant) = DateTime::from_timestamp(bucket, 0) else {
                        continue;
                    };
                    // The instant's wall clock at the offset, stamped
                    // naive: `timestamp_millis` on the shifted datetime
                    // would return the instant, not the label.
                    timestamps.append_value(
                        offset
                            .from_utc_datetime(&instant.naive_utc())
                            .naive_local()
                            .and_utc()
                            .timestamp_millis(),
                    );
                }
                FoldRows::LocalClock => {
                    let Some(instant) = DateTime::from_timestamp(bucket, 0) else {
                        continue;
                    };
                    timestamps.append_value(instant.naive_utc().and_utc().timestamp_millis());
                }
            }
            let cells = self.cells.get(&bucket).map_or(&[][..], Vec::as_slice);
            for (slot, builder) in builders.iter_mut().enumerate() {
                let Some(cell) = cells.get(slot) else {
                    builder.append_null();
                    continue;
                };
                if !aggregate {
                    builder.append_value(cell.weight);
                    continue;
                }
                // A cell whose covered weight is zero is null, whatever
                // its rule.
                if cell.weight <= 0.0 {
                    builder.append_null();
                } else if let Some(labels) = self.enum_labels[slot] {
                    // An Enum column folds to the code with the greatest
                    // summed weight, ties to the lowest code: the
                    // weighted Mode, never a mean over the codes.
                    builder.append_value(mode_winner(&cell.counts[..labels]) as f64);
                } else {
                    match self.rules[slot] {
                        hares_types::AcrossHomes::Sum => builder.append_value(cell.sum_vw),
                        hares_types::AcrossHomes::Mean => {
                            builder.append_value(cell.sum_vw / cell.weight);
                        }
                    }
                }
            }
        }

        let mut fields = vec![Field::new(
            "Time",
            DataType::Timestamp(TimeUnit::Millisecond, None),
            false,
        )];
        fields.extend(self.columns.iter().cloned());
        let mut metadata = std::collections::HashMap::new();
        if let FoldRows::Instant(offset) = self.rows {
            metadata.insert(
                "hares:utc_offset_s".to_string(),
                offset.local_minus_utc().to_string(),
            );
        }
        let schema = Arc::new(Schema::new_with_metadata(fields, metadata));
        let mut arrays: Vec<Arc<dyn Array>> = Vec::with_capacity(self.columns.len() + 1);
        arrays.push(Arc::new(timestamps.finish()));
        for mut builder in builders {
            arrays.push(Arc::new(builder.finish()));
        }
        RecordBatch::try_new(schema, arrays).expect("the fold's table builds")
    }
}

/// One reduced period the home's reducer emitted: the naive local
/// wall-clock start (the calendar's own clock: the home's, or a declared
/// alignment's), the step count that fell in the period, the reduced
/// value per rule and the valid step count per rule.
#[derive(Debug, Clone)]
pub(crate) struct PeriodRowData {
    pub start_local_ms: i64,
    pub step_count: u32,
    pub values: Vec<f64>,
    pub valid_steps: Vec<u32>,
}

impl PeriodRowData {
    fn from_row(row: &PeriodRow<'_>) -> Self {
        Self {
            start_local_ms: row.start_local_ms,
            step_count: row.step_count,
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
    /// The home's step span in milliseconds: the partial-period rule's
    /// step.
    step_ms: i64,
    /// The offset in seconds subtracted from the reduced period's naive
    /// local start to key the fold's rows: the declared alignment's
    /// offset under `Instant` (the row's wall clock is the declared
    /// offset's), the home's own site offset under the undeclared
    /// single-offset default, zero under `LocalClock` (the naive local
    /// start itself).
    key_offset_s: i32,
    /// Whether the fleet declared an `Instant` alignment: a reduced
    /// period whose step count is below the period's full step count is
    /// null for the home, so an edge period some homes cover only in part
    /// carries a reduced covered weight.
    instant_aligned: bool,
    /// The period calendar the home's reducer buckets onto.
    calendar: PeriodCalendar,
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
    pub(crate) fn new(alignment: &HomeFoldAlignment) -> Self {
        Self {
            frequency: alignment.frequency,
            step_ms: alignment.step_ms,
            key_offset_s: alignment.key_offset_s,
            instant_aligned: alignment.instant_aligned,
            calendar: alignment.calendar,
            reducer: None,
            rules: None,
            fields: None,
            rows: Vec::new(),
        }
    }

    /// Closes the home's reducer: the last partial period emits, and the
    /// reduced table is keyed by the alignment the fleet declared: the
    /// period's start instant (today's epoch key) or the naive local
    /// period start.
    pub(crate) fn finish(&mut self) -> (BTreeMap<i64, Vec<Option<f64>>>, Vec<Field>) {
        if let Some(reducer) = self.reducer.take() {
            reducer.finish(&mut |row| self.rows.push(PeriodRowData::from_row(&row)));
        }
        let fields = self.fields.clone().unwrap_or_default();
        let mut table = BTreeMap::new();
        let rows = std::mem::take(&mut self.rows);
        for row in rows {
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
            let values = if self.instant_aligned && self.period_is_partial(&row) {
                vec![None; row.values.len()]
            } else {
                row.values()
            };
            table.insert(naive_secs - i64::from(self.key_offset_s), values);
        }
        (table, fields)
    }

    /// Whether the home covers the period only in part: its step count,
    /// at the step the run ran, spans less than the period's full span on
    /// the calendar the reduction bucketed onto. A `RunPeriod` is never
    /// partial: the period is the run.
    fn period_is_partial(&self, row: &PeriodRowData) -> bool {
        let full_span_ms = match self.frequency {
            Frequency::Timestep | Frequency::RunPeriod => return false,
            Frequency::Minutes(every) => i64::from(every.get()) * 60_000,
            Frequency::Hourly => 3_600_000,
            Frequency::Daily => 86_400_000,
            Frequency::Monthly | Frequency::Annual => {
                let Some(time) = DateTime::from_timestamp_millis(row.start_local_ms) else {
                    return false;
                };
                let naive = time.naive_utc();
                use chrono::Datelike;
                let (start_year, start_month, next_year, next_month) = match self.frequency {
                    Frequency::Monthly => {
                        let month = naive.month();
                        let next_month = if month == 12 { 1 } else { month + 1 };
                        let next_year = if month == 12 {
                            naive.year() + 1
                        } else {
                            naive.year()
                        };
                        (naive.year(), month, next_year, next_month)
                    }
                    _ => (naive.year(), 1, naive.year() + 1, 1),
                };
                let midnight = |year: i32, month: u32| {
                    chrono::NaiveDate::from_ymd_opt(year, month, 1)
                        .expect("a real period start")
                        .and_hms_opt(0, 0, 0)
                        .expect("midnight")
                };
                (midnight(next_year, next_month) - midnight(start_year, start_month))
                    .num_milliseconds()
            }
        };
        i64::from(row.step_count) * self.step_ms < full_span_ms
    }
}

/// One home's fold alignment: the frequency and step the reduction and
/// the partial-period rule read, the period calendar the home's reducer
/// buckets onto, the offset in seconds the reduced rows are keyed by and
/// whether the fleet declared an `Instant` alignment.
#[derive(Debug, Clone)]
pub(crate) struct HomeFoldAlignment {
    pub frequency: Frequency,
    pub step_ms: i64,
    pub calendar: PeriodCalendar,
    pub key_offset_s: i32,
    pub instant_aligned: bool,
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
                .map(|field| field_fleet_rules(field).map(|(over_time, _)| over_time))
                .collect::<Result<Vec<_>, FleetError>>()
                .map_err(|err| CallbackError::new(err.to_string()))?;
            for (field, rule) in state
                .fields
                .as_ref()
                .expect("fields set above")
                .iter()
                .zip(&rules)
            {
                tracing::debug!(
                    column = field.name().as_str(),
                    kind = ?field.metadata().get("kind"),
                    over_time = ?rule.aggregation,
                    enum_labels = ?rule.enum_codes,
                    "the home's fleet reducer takes the column's over-time rule"
                );
            }
            let calendar = state.calendar;
            let reducer = PeriodReducer::new(state.frequency, calendar, rules.clone())
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
pub(crate) fn homes_batch(homes: &[HomeResult]) -> RecordBatch {
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
            HomeStatus::Ok => "ok",
            HomeStatus::Degraded(_) => "degraded",
            HomeStatus::Failed(_) => "failed",
        });
        match &home.status {
            HomeStatus::Ok => reasons.append_null(),
            HomeStatus::Degraded(reason) | HomeStatus::Failed(reason) => {
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
    use arrow::array::{Array, Float64Array, TimestampMillisecondArray};
    use hares_types::Unit;

    fn field(name: &str, unit: &str) -> Field {
        Field::new(name, DataType::Float64, true).with_metadata(
            [("unit".to_string(), unit.to_string())]
                .into_iter()
                .collect(),
        )
    }

    /// Two variables: a power column (weighted sum across homes) and a
    /// temperature column (weighted mean across homes), the declared
    /// fields the schema builders write.
    fn two_fields() -> Vec<Field> {
        vec![
            declared_field(&declared(
                "Total Electric Power (kW)",
                Unit::Kilowatt,
                hares_types::VariableKind::Power,
                Aggregation::Mean,
            )),
            declared_field(&declared(
                "Temperature - Indoor (C)",
                Unit::Celsius,
                hares_types::VariableKind::Temperature,
                Aggregation::Mean,
            )),
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

    fn time_labels(batch: &RecordBatch) -> Vec<i64> {
        let values = batch
            .column(0)
            .as_any()
            .downcast_ref::<TimestampMillisecondArray>()
            .expect("a Timestamp Time column");
        (0..values.len()).map(|row| values.value(row)).collect()
    }

    fn default_fold() -> FleetFold {
        FleetFold::new(FoldRows::Instant(
            FixedOffset::east_opt(0).expect("a zero offset is representable"),
        ))
    }

    /// A declared variable with the given name, unit, kind and over-time
    /// rule, the dwelling-sourced shape every catalogue variable carries.
    fn declared(
        name: &str,
        unit: Unit,
        kind: hares_types::VariableKind,
        aggregation: Aggregation,
    ) -> hares_io::VariableDef {
        hares_io::VariableDef {
            name: std::borrow::Cow::Owned(name.to_string()),
            unit,
            kind,
            aggregation,
            sampling: hares_types::Sampling::InstantAtEnd,
            nullable: true,
            source: hares_io::VariableSource::Dwelling,
            description: std::borrow::Cow::Owned("the test's variable".to_string()),
        }
    }

    /// A declared variable's field, the shape the schema builders write:
    /// the unit, kind, aggregation and across-homes tags in the field
    /// metadata, the declared rules the fold reads.
    fn declared_field(def: &hares_io::VariableDef) -> Field {
        hares_io::output::catalogue::variable_field(def)
            .as_ref()
            .clone()
    }

    /// The cross-home fold follows the declared kinds: three homes with
    /// weights 1.0, 2.5 and 0.5 and one period, with one variable of each
    /// kind whose rule the unit-keyed tables got wrong: a ratio (unit
    /// `fraction`), a binary state (unit `bool`), an intensive quantity
    /// declared to average across homes (unit `V`) and a quantity
    /// declared `Sum` over time and across homes (unit `count`). Each
    /// cell is the declared combination of the homes' period values,
    /// added in entry order and compared on bits: the weighted mean for
    /// the ratio, the state and the voltage; the weighted sum for the
    /// count. The enum kind's Mode fold is the fold's own already and its
    /// tests carry it.
    #[test]
    fn fleet_fold_follows_declared_kinds() {
        let defs = [
            declared(
                "Water Heater Fraction (fraction)",
                Unit::Fraction,
                hares_types::VariableKind::Ratio,
                Aggregation::Mean,
            ),
            declared(
                "Defrost Active (bool)",
                Unit::Boolean,
                hares_types::VariableKind::State,
                Aggregation::Mean,
            ),
            declared(
                "Grid Voltage (V)",
                Unit::Volt,
                hares_types::VariableKind::Quantity {
                    across_homes: hares_types::AcrossHomes::Mean,
                },
                Aggregation::Mean,
            ),
            declared(
                "Signal Count (count)",
                Unit::Count,
                hares_types::VariableKind::Quantity {
                    across_homes: hares_types::AcrossHomes::Sum,
                },
                Aggregation::Sum,
            ),
        ];
        let fields: Vec<Field> = defs.iter().map(declared_field).collect();
        let key = 0_i64;
        let homes: [(f64, Vec<Option<f64>>); 3] = [
            (1.0, vec![Some(0.2), Some(1.0), Some(120.0), Some(10.0)]),
            (2.5, vec![Some(0.3), Some(0.0), Some(121.0), Some(20.0)]),
            (0.5, vec![Some(0.4), Some(1.0), Some(119.0), Some(30.0)]),
        ];

        let mut fold = default_fold();
        for (index, (weight, values)) in homes.iter().enumerate() {
            let mut table = BTreeMap::new();
            table.insert(key, values.clone());
            fold.fold_home(index, *weight, &fields, &table)
                .expect("valid weight");
        }
        let (aggregate, covered_weight, periods) = fold.finish();

        assert_eq!(periods, 1);
        // The declared combination, added in entry order: the weighted
        // mean divides once after the last home, the weighted sum never
        // divides.
        let mean = |values: [f64; 3]| {
            (values[0] * 1.0 + values[1] * 2.5 + values[2] * 0.5) / (1.0 + 2.5 + 0.5)
        };
        let sum = |values: [f64; 3]| values[0] * 1.0 + values[1] * 2.5 + values[2] * 0.5;

        let ratio = column(&aggregate, 1);
        assert_eq!(
            ratio[0].unwrap().to_bits(),
            mean([0.2, 0.3, 0.4]).to_bits(),
            "the ratio folds to the weighted mean, not the weighted sum"
        );
        let state = column(&aggregate, 2);
        assert_eq!(
            state[0].unwrap().to_bits(),
            mean([1.0, 0.0, 1.0]).to_bits(),
            "the state folds to the weighted mean, the duty fraction"
        );
        let voltage = column(&aggregate, 3);
        assert_eq!(
            voltage[0].unwrap().to_bits(),
            mean([120.0, 121.0, 119.0]).to_bits(),
            "the intensive quantity folds to the weighted mean"
        );
        let count = column(&aggregate, 4);
        assert_eq!(
            count[0].unwrap().to_bits(),
            sum([10.0, 20.0, 30.0]).to_bits(),
            "the quantity declared Sum folds to the weighted sum"
        );
        for slot in 1..=4 {
            assert_eq!(
                column(&covered_weight, slot)[0].unwrap().to_bits(),
                4.0_f64.to_bits(),
                "every home's weight is behind every declared cell"
            );
        }
    }

    /// A field without the declared-kind metadata is a contract
    /// violation, not a guess: the typed error names the field, at the
    /// rules function and at the fold that takes its rules from it. The
    /// parent commit's suffix parsing guessed the rules from the column
    /// name's parenthesised suffix here; the fallback is gone.
    #[test]
    fn an_untagged_field_is_a_typed_error_naming_the_field() {
        let untagged = field("Schedule (-)", "-");
        let err = field_fleet_rules(&untagged)
            .expect_err("an untagged field is a contract violation, not a guess");
        assert!(
            matches!(err, FleetError::UndeclaredKind { .. }),
            "the typed error, got: {err:?}"
        );
        assert!(
            err.to_string().contains("Schedule (-)"),
            "the error names the field: {err}"
        );

        let fields = vec![untagged];
        let mut fold = default_fold();
        let mut table = BTreeMap::new();
        table.insert(0_i64, vec![Some(1.0)]);
        let err = fold
            .fold_home(0, 1.0, &fields, &table)
            .expect_err("an untagged field is rejected at the fold");
        assert!(
            matches!(err, FleetError::UndeclaredKind { ref name } if name == "Schedule (-)"),
            "the fold's error names the field: {err:?}"
        );
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

        let mut fold = default_fold();
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
        // The naive local label: the instant key's wall clock at the
        // fold's offset (+00:00 here), in milliseconds.
        assert_eq!(time_labels(&aggregate), vec![1_700_000_000_i64 * 1000]);
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
        let mut unbounded = default_fold();
        for (index, table) in tables.iter().enumerate() {
            unbounded
                .fold_home(index, weights[index], &fields, table)
                .expect("valid weight");
        }
        let (unbounded_aggregate, unbounded_weight, unbounded_periods) = unbounded.finish();

        // The bounded fold: one home's table folded and dropped at a
        // time, chunks of one.
        let mut bounded = default_fold();
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

        let mut fold = default_fold();
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
        let mut fold = default_fold();
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

    /// Homes with different column sets aggregate to the union, in order
    /// of first appearance in entry order; a mismatch no longer empties
    /// the aggregate. A variable only one home declares carries that
    /// home's value with its weight alone; a shared variable combines
    /// both homes by the rule in force.
    #[test]
    fn heterogeneous_columns_aggregate_to_the_union() {
        let key = 0_i64;
        let mut home_a = BTreeMap::new();
        home_a.insert(key, row(Some(1.0), Some(20.0)));
        let different = vec![
            declared_field(&declared(
                "Other Power (kW)",
                Unit::Kilowatt,
                hares_types::VariableKind::Power,
                Aggregation::Mean,
            )),
            declared_field(&declared(
                "Temperature - Indoor (C)",
                Unit::Celsius,
                hares_types::VariableKind::Temperature,
                Aggregation::Mean,
            )),
        ];
        let mut home_b = BTreeMap::new();
        home_b.insert(key, row(Some(2.0), Some(21.0)));

        let mut fold = default_fold();
        fold.fold_home(0, 1.0, &two_fields(), &home_a)
            .expect("valid weight");
        fold.fold_home(1, 1.0, &different, &home_b)
            .expect("valid weight");
        let (aggregate, covered_weight, periods) = fold.finish();

        assert_eq!(periods, 1);
        let names: Vec<&str> = aggregate
            .schema_ref()
            .fields()
            .iter()
            .map(|field| field.name().as_str())
            .collect();
        assert_eq!(
            names,
            [
                "Time",
                "Total Electric Power (kW)",
                "Temperature - Indoor (C)",
                "Other Power (kW)",
            ],
            "the union in order of first appearance"
        );
        // The power column: home a's alone (home b declares the other
        // power column), covered weight 1.
        assert_eq!(
            column(&aggregate, 1)[0].unwrap().to_bits(),
            1.0_f64.to_bits()
        );
        assert_eq!(
            column(&covered_weight, 1)[0].unwrap().to_bits(),
            1.0_f64.to_bits()
        );
        // The shared temperature column means across homes.
        assert_eq!(
            column(&aggregate, 2)[0].unwrap().to_bits(),
            20.5_f64.to_bits()
        );
        assert_eq!(
            column(&covered_weight, 2)[0].unwrap().to_bits(),
            2.0_f64.to_bits()
        );
        // The other power column: home b's alone, covered weight 1.
        assert_eq!(
            column(&aggregate, 3)[0].unwrap().to_bits(),
            2.0_f64.to_bits()
        );
        assert_eq!(
            column(&covered_weight, 3)[0].unwrap().to_bits(),
            1.0_f64.to_bits()
        );
        assert_eq!(covered_weight.num_rows(), 1);
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

        let mut fold = default_fold();
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

        let mut fold = default_fold();
        let err = fold
            .fold_home(7, f64::NAN, &fields, &home)
            .expect_err("a NaN weight is rejected");
        assert!(
            matches!(err, FleetError::InvalidAggregationWeight { index: 7, .. }),
            "got: {err:?}"
        );
    }

    /// An Enum-kind column's field: the unit and the label count in the
    /// field's metadata, the shape the schema builders write. The mode
    /// columns' unit is the dimensionless `-`, so no unit string
    /// identifies an Enum column: the label count does.
    fn enum_field(name: &str, labels: usize) -> Field {
        Field::new(name, DataType::Float64, true).with_metadata(
            [
                ("unit".to_string(), "-".to_string()),
                ("enum_labels".to_string(), labels.to_string()),
            ]
            .into_iter()
            .collect(),
        )
    }

    /// An Enum column's field resolves to the Mode rule with the
    /// metadata's label count over time, whatever its unit string; the
    /// across-homes fold reads the label count, not the across rule.
    #[test]
    fn enum_field_rules_are_mode_with_the_label_count() {
        let (over_time, across) = field_fleet_rules(&enum_field("Mode (-)", 7))
            .expect("the label count is the Enum column's tag");
        assert!(
            matches!(over_time.aggregation, Aggregation::Mode),
            "an Enum column folds by Mode over time"
        );
        assert_eq!(
            over_time.enum_codes,
            Some(7),
            "the label count feeds the rule"
        );
        assert!(
            matches!(across, hares_types::AcrossHomes::Mean),
            "the across rule of an Enum column is inert: the fold reads the label count"
        );
    }

    /// The fleet's Enum column folds to the code with the greatest summed
    /// weight: homes with weights 1, 1 and 3 reporting modes 0, 0 and 2
    /// in one period fold to 2, the weighted Mode, never the weighted
    /// mean (1.2, which is no code).
    #[test]
    fn fleet_mode_uses_summed_weight() {
        let fields = vec![enum_field("Mode (-)", 3)];
        let key = 0_i64;
        let homes: [(f64, Option<f64>); 3] = [(1.0, Some(0.0)), (1.0, Some(0.0)), (3.0, Some(2.0))];

        let mut fold = default_fold();
        for (index, (weight, code)) in homes.iter().enumerate() {
            let mut table = BTreeMap::new();
            table.insert(key, vec![*code]);
            fold.fold_home(index, *weight, &fields, &table)
                .expect("valid weight");
        }
        let (aggregate, covered_weight, _) = fold.finish();

        assert_eq!(
            column(&aggregate, 1)[0].unwrap().to_bits(),
            2.0_f64.to_bits(),
            "the weighted Mode, not the weighted mean 1.2"
        );
        assert_eq!(
            column(&covered_weight, 1)[0].unwrap().to_bits(),
            5.0_f64.to_bits(),
            "every home's weight voted"
        );
    }

    /// Two homes whose modes disagree: the fold is the weighted Mode with
    /// the tie to the lowest code, never the mean of the codes. Weights 1
    /// and 1 reporting codes 2 and 1 tie on summed weight, so the lowest
    /// code wins; their mean, 1.5, is no code at all.
    #[test]
    fn fleet_mode_ties_break_to_the_lowest_code() {
        let fields = vec![enum_field("Mode (-)", 3)];
        let key = 0_i64;

        let mut fold = default_fold();
        for (index, (weight, code)) in [(1.0, 2.0), (1.0, 1.0)].iter().enumerate() {
            let mut table = BTreeMap::new();
            table.insert(key, vec![Some(*code)]);
            fold.fold_home(index, *weight, &fields, &table)
                .expect("valid weight");
        }
        let (aggregate, covered_weight, _) = fold.finish();

        assert_eq!(
            column(&aggregate, 1)[0].unwrap().to_bits(),
            1.0_f64.to_bits(),
            "the tie breaks to the lowest code, not the mean 1.5"
        );
        assert_eq!(
            column(&covered_weight, 1)[0].unwrap().to_bits(),
            2.0_f64.to_bits()
        );
    }

    /// A home whose mode cell is null does not vote; a cell every home
    /// left null is null.
    #[test]
    fn fleet_mode_ignores_null_homes() {
        let fields = vec![enum_field("Mode (-)", 3)];
        let key = 0_i64;

        let mut fold = default_fold();
        for (index, (weight, code)) in [(2.0, None), (1.0, Some(2.0))].iter().enumerate() {
            let mut table = BTreeMap::new();
            table.insert(key, vec![*code]);
            fold.fold_home(index, *weight, &fields, &table)
                .expect("valid weight");
        }
        let (aggregate, covered_weight, _) = fold.finish();
        assert_eq!(
            column(&aggregate, 1)[0].unwrap().to_bits(),
            2.0_f64.to_bits(),
            "the null home does not vote"
        );
        assert_eq!(
            column(&covered_weight, 1)[0].unwrap().to_bits(),
            1.0_f64.to_bits(),
            "only the voting home's weight"
        );

        let mut fold = default_fold();
        for index in 0..2 {
            let mut table = BTreeMap::new();
            table.insert(key, vec![None]);
            fold.fold_home(index, 1.0, &fields, &table)
                .expect("valid weight");
        }
        let (aggregate, covered_weight, _) = fold.finish();
        assert!(column(&aggregate, 1)[0].is_none(), "no vote is null");
        assert_eq!(
            column(&covered_weight, 1)[0].unwrap().to_bits(),
            0.0_f64.to_bits()
        );
    }

    /// A binary State column keeps the weighted mean, which is the duty
    /// fraction: homes with weights 1 and 3 reporting states 1 and 0 fold
    /// to 0.25. The Mode fold never touches a State column.
    #[test]
    fn fleet_binary_state_is_weighted_duty_fraction() {
        let fields = vec![declared_field(&declared(
            "Defrost State (-)",
            Unit::Boolean,
            hares_types::VariableKind::State,
            Aggregation::Mean,
        ))];
        let key = 0_i64;

        let mut fold = default_fold();
        for (index, (weight, state)) in [(1.0, 1.0), (3.0, 0.0)].iter().enumerate() {
            let mut table = BTreeMap::new();
            table.insert(key, vec![Some(*state)]);
            fold.fold_home(index, *weight, &fields, &table)
                .expect("valid weight");
        }
        let (aggregate, covered_weight, _) = fold.finish();

        assert_eq!(
            column(&aggregate, 1)[0].unwrap().to_bits(),
            0.25_f64.to_bits(),
            "the duty fraction"
        );
        assert_eq!(
            column(&covered_weight, 1)[0].unwrap().to_bits(),
            4.0_f64.to_bits()
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

        let mut fold = default_fold();
        fold.fold_home(0, 1.0, &fields, &empty)
            .expect("valid weight");
        fold.fold_home(1, 2.0, &fields, &home_a)
            .expect("valid weight");
        let folded = fold.folded;
        let (aggregate, _, periods) = fold.finish();

        assert_eq!(folded, 1, "only the home with rows folded");
        assert_eq!(periods, 1);
        assert_eq!(
            column(&aggregate, 1)[0].unwrap().to_bits(),
            2.0_f64.to_bits()
        );
    }
}
