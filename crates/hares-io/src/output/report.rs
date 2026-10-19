//! The output row plan: selections frozen once, written by index per step.
//!
//! A run's output columns are fixed for the life of the run, so the
//! name-to-position mapping is resolved exactly once, at freeze, into a
//! [`RowPlan`]; the per-step path writes values by precomputed index and
//! never looks a column name up. The plan-input surface is
//! [`VariableSurface`]: the catalogue's variable names in producer-slot
//! order. The catalogue derives the run's schema, and [`SchemaSurface`]
//! provides the surface from it; the catalogue can implement the typed
//! form of the same seam without touching the plan builder.

use std::num::NonZeroU32;

use arrow::datatypes::Schema;
use chrono::Duration;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::catalogue::Catalogue;
use hares_types::Unit;

use crate::config::ConfigError;

/// What a report selects (`plan.md` 2.2). The `Diagnostics` variant
/// selects the per-step diagnostics variables, declared in the catalogue
/// as one block and appended to the run's schema only when a run requests
/// them; the period frequencies arrive with the reporting-frequency entry.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Selection {
    /// The verbosity preset: every variable the selected verbosity tier
    /// emits. The preset is materialized by the variable surface: the
    /// schema (or the catalogue view of a run) is built at the selected
    /// verbosity, so the selection resolves to all of it.
    Verbosity(u8),
    /// Explicit variable names, resolved exactly and case-sensitively.
    Named(Vec<String>),
    /// A glob over variable names: `*` matches any run of characters,
    /// `?` matches exactly one, everything else is literal (brackets have
    /// no special meaning), case-sensitive.
    Pattern(String),
    /// The diagnostics block: the per-step diagnostics variables, appended
    /// to the run's schema when the run requests the diagnostics. The
    /// schema carries the block only for a run that requested it, so this
    /// selection resolves to the block's slots in the freeze.
    Diagnostics,
}

/// The reporting frequency of a selection. The `Timestep` frequency is one
/// row per step, the recorder's own shape; the calendar periods reduce the
/// step stream through a `PeriodReducer`, each variable by its declared
/// aggregation. Every non-timestep variant is externally tagged in TOML
/// with its snake_case name, so a config carries `"hourly"`,
/// `{ minutes = 15 }` and so on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Frequency {
    /// One row per step: no reduction.
    #[default]
    Timestep,
    /// Sub-hourly periods of `n` minutes: `n` divides the hour, the period
    /// is a whole multiple of the timestep, and the periods start on the
    /// local clock hour.
    Minutes(NonZeroU32),
    /// Calendar hours in the naive local wall clock.
    Hourly,
    /// Calendar days in the naive local wall clock.
    Daily,
    /// Calendar months in the naive local wall clock.
    Monthly,
    /// Calendar years in the naive local wall clock.
    Annual,
    /// One row covering the whole run.
    RunPeriod,
}

impl Frequency {
    /// The frequency's own checks against the run's time resolution: the
    /// checks the configuration's validate runs for every report whose
    /// frequency is not the default. `Minutes(n)` requires `n` to divide
    /// the hour; every calendar period must be a whole multiple of the
    /// timestep, so a period holds a whole number of steps and the steps
    /// tile it exactly.
    ///
    /// # Errors
    ///
    /// A `Minutes(n)` whose `n` does not divide 60 (the value and the rule
    /// named), and a calendar period that is not a whole multiple of
    /// `time_res`. Months and years are whole numbers of days, so for them
    /// the check is that the timestep divides a day.
    pub fn validate(self, time_res: Duration) -> Result<(), ConfigError> {
        let step = time_res.num_seconds();
        let reject = |period_s: i64, name: &str| {
            Err(ConfigError::Validation(format!(
                "frequency {name}: the period ({period_s} s) is not a whole \
                 multiple of the time_res ({step} s)"
            )))
        };
        match self {
            Self::Timestep | Self::RunPeriod => Ok(()),
            Self::Minutes(n) => {
                let n = n.get();
                if 60 % n != 0 {
                    return Err(ConfigError::Validation(format!(
                        "frequency Minutes({n}) is not a valid reporting \
                         frequency: n must divide the hour (60)"
                    )));
                }
                let period_s = i64::from(n) * 60;
                if period_s % step != 0 {
                    reject(period_s, &format!("Minutes({n})"))
                } else {
                    Ok(())
                }
            }
            Self::Hourly => {
                if 3600 % step != 0 {
                    reject(3600, "Hourly")
                } else {
                    Ok(())
                }
            }
            Self::Daily => {
                if 86_400 % step != 0 {
                    reject(86_400, "Daily")
                } else {
                    Ok(())
                }
            }
            Self::Monthly | Self::Annual => {
                if 86_400 % step != 0 {
                    Err(ConfigError::Validation(format!(
                        "frequency {}: every calendar period is a whole number \
                         of days, so the time_res ({step} s) must divide a day \
                         (86 400 s)",
                        if matches!(self, Self::Monthly) {
                            "Monthly"
                        } else {
                            "Annual"
                        }
                    )))
                } else {
                    Ok(())
                }
            }
        }
    }
}

/// Where a report's batches go (`plan.md` 2.2). `Callback` arrives with the
/// Python-attach entry; every variant this entry adds can run.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Destination {
    /// Every flushed batch is retained in memory: the report's frame.
    Memory,
    /// Stream to a Parquet file, published at the run's end.
    Parquet {
        path: PathBuf,
        #[serde(default)]
        overwrite: bool,
    },
    /// Stream to a CSV file, published at the run's end.
    Csv {
        path: PathBuf,
        #[serde(default)]
        overwrite: bool,
    },
    /// The batch callback: every built batch is handed to the attached
    /// callback, in flush order, and nothing is retained. As a config
    /// value it is the serialisable marker a run's provenance records for
    /// a report whose batches go to the attached callback.
    Callback,
}

impl Destination {
    /// The file path of a file destination; `None` for [`Destination::Memory`]
    /// and [`Destination::Callback`].
    #[must_use]
    pub fn path(&self) -> Option<&PathBuf> {
        match self {
            Destination::Memory | Destination::Callback => None,
            Destination::Parquet { path, .. } | Destination::Csv { path, .. } => Some(path),
        }
    }
}

/// One report request: what a run records and where it goes (`plan.md`
/// 2.2). A run declares the reports it wants and each one's destination;
/// nothing is recorded or written that no report requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportSpec {
    /// The report's name, unique within a config.
    pub name: String,
    /// What the report selects.
    pub selection: Selection,
    /// The reporting frequency.
    #[serde(default)]
    pub frequency: Frequency,
    /// Where the report's batches go.
    pub destination: Destination,
}

/// The variables a row plan freezes from, in producer-slot (schema) order.
/// The minimal plan-input seam: the catalogue's typed surface implements
/// this over its schema-ordered `VariableDef` slice; [`SchemaSurface`]
/// provides it from the output schema.
pub trait VariableSurface {
    /// The variable names, in producer-slot (schema) order.
    fn variable_names(&self) -> Vec<&str>;

    /// The variables' declared reporting units, in the same order: the
    /// row plan converts each written value from SI with the column's
    /// own unit (the one reporting conversion).
    fn variable_units(&self) -> Vec<Unit>;

    /// The producer-slot index of the diagnostics block's first variable,
    /// when the surface carries the block. The block is a contiguous
    /// suffix of the schema's variables; a verbosity preset spans the
    /// slots before it, the diagnostics selection the slots from it.
    fn diagnostics_offset(&self) -> Option<usize>;
}

/// The schema-built variable surface: the output schema's value columns in
/// schema order (the timestamp column excluded). The diagnostics block,
/// when the schema carries it, is reported through
/// [`VariableSurface::diagnostics_offset`]: the schema's diagnostics
/// fields carry the block's metadata marker.
pub struct SchemaSurface {
    names: Vec<String>,
    /// The variables' declared reporting units, in the names' order: the
    /// schema fields' own `unit` metadata.
    units: Vec<Unit>,
    /// The index of the first diagnostics field among the value columns,
    /// when the schema carries the block.
    diagnostics_start: Option<usize>,
}

impl SchemaSurface {
    /// Builds the surface from a schema, skipping the timestamp column:
    /// recorder rows address the value columns only. The diagnostics
    /// offset is read from the schema's own diagnostics field marker.
    pub fn new(schema: &Schema) -> Self {
        let mut diagnostics_start = None;
        let mut names = Vec::new();
        let mut units = Vec::new();
        for (idx, field) in schema.fields().iter().skip(1).enumerate() {
            if diagnostics_start.is_none()
                && field
                    .metadata()
                    .contains_key(super::catalogue::DIAGNOSTICS_METADATA_KEY)
            {
                diagnostics_start = Some(idx);
            }
            names.push(field.name().clone());
            units.push(
                field
                    .metadata()
                    .get("unit")
                    .and_then(|symbol| Unit::from_symbol(symbol))
                    .unwrap_or(Unit::None),
            );
        }
        Self {
            names,
            units,
            diagnostics_start,
        }
    }
}

impl VariableSurface for SchemaSurface {
    fn variable_names(&self) -> Vec<&str> {
        self.names.iter().map(String::as_str).collect()
    }

    fn variable_units(&self) -> Vec<Unit> {
        self.units.clone()
    }

    fn diagnostics_offset(&self) -> Option<usize> {
        self.diagnostics_start
    }
}

/// Why a selection list cannot freeze into a row plan.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RowPlanError {
    /// A `Named` selection carried names the surface does not carry, each
    /// with up to three nearest surface names by edit distance (ties by
    /// surface order).
    #[error("unknown output variables: {0}")]
    UnknownNames(String),
    /// A `Pattern` selection matched no variable.
    #[error("output pattern '{0}' matches no variable")]
    EmptyPattern(String),
    /// A `Diagnostics` selection against a surface that carries no
    /// diagnostics block: the run did not request the diagnostics, so the
    /// schema has none to select.
    #[error("diagnostics selection: the schema carries no diagnostics block")]
    NoDiagnostics,
}

/// The frozen write plan for a run's output rows.
///
/// Built once at freeze from a list of selections: the union of the
/// selected variables mapped to producer slots, one reused row, a validity
/// bitset and the row's local timestamp, addressed by index. Each selection
/// holds its projection (the slot indices of its variables, in selection
/// order). The producers write the union row through the plan; nothing on
/// the write path sees a column name or a hash map.
///
/// A slot the producers do not write in a step stays invalid and reads as
/// null downstream: a not-applicable value is null, never a substituted
/// 0.0. A declared-not-nullable variable a producer claims and did not
/// write is a wiring defect the step reports (the producer bindings carry
/// that gate's slots).
#[derive(Debug, Default)]
pub struct RowPlan {
    slots: usize,
    /// Per-slot declared reporting unit: the write applies the unit's
    /// one `from_si` conversion (B-14's run-state commit), so every
    /// producer writes SI and every report row reads its column's
    /// declared unit.
    units: Vec<Unit>,
    /// The reused union row: one slot per producer slot, zeroed per step.
    row: Vec<f64>,
    /// Per-slot validity over the row, cleared with it each step: a clear
    /// bit reads as null in every batch the step flushes.
    validity: Vec<u64>,
    /// The current row's timestamp as UTC epoch seconds, stamped per step.
    /// Not a local-epoch stamp: the offset the run's row timestamps carry
    /// is not applied here (the row's own timestamp column carries it);
    /// nothing reading this field may assume local epoch seconds.
    local_timestamp: i64,
    /// One projection per selection, in selection order: slot indices.
    projections: Vec<Vec<usize>>,
}

impl RowPlan {
    /// Freezes a row plan: resolves every selection against `surface` and
    /// unions the selected variables into producer slots. At this entry the
    /// producer slots are the output schema's value columns, in schema
    /// order, so each projection lists slot indices into the run row.
    ///
    /// # Errors
    ///
    /// [`RowPlanError::UnknownNames`] when a `Named` selection names a
    /// variable the surface does not carry, and
    /// [`RowPlanError::EmptyPattern`] when a `Pattern` selection matches
    /// nothing.
    pub fn build(
        selections: &[Selection],
        surface: &dyn VariableSurface,
    ) -> Result<Self, RowPlanError> {
        let names = surface.variable_names();
        let diagnostics_start = surface.diagnostics_offset();
        let mut projections = Vec::with_capacity(selections.len());
        for selection in selections {
            let projection = match selection {
                // The preset is materialized by the surface: the schema (or
                // catalogue view) is built at the selected verbosity, so the
                // selection spans the slots before the diagnostics block.
                Selection::Verbosity(_) => (0..diagnostics_start.unwrap_or(names.len())).collect(),
                Selection::Named(selected) => resolve_named(selected, &names)?,
                Selection::Pattern(pattern) => {
                    let matched: Vec<usize> = names
                        .iter()
                        .enumerate()
                        .filter(|(_, name)| glob_match(pattern, name))
                        .map(|(slot, _)| slot)
                        .collect();
                    if matched.is_empty() {
                        return Err(RowPlanError::EmptyPattern(pattern.clone()));
                    }
                    matched
                }
                // The diagnostics block: the slots from the block's first
                // variable to the end of the schema, in schema order.
                Selection::Diagnostics => match diagnostics_start {
                    Some(start) => (start..names.len()).collect(),
                    None => return Err(RowPlanError::NoDiagnostics),
                },
            };
            projections.push(projection);
        }
        let units = surface.variable_units();
        let slots = names.len();
        Ok(Self {
            slots,
            units,
            row: vec![0.0; slots],
            validity: vec![0; slots.div_ceil(usize::BITS as usize)],
            local_timestamp: 0,
            projections,
        })
    }

    /// The producer-slot count (the union row's width).
    pub fn slots(&self) -> usize {
        self.slots
    }

    /// The projection of selection `selection`, as slot indices.
    ///
    /// # Panics
    ///
    /// When `selection` names no projection the plan was built with.
    pub fn projection(&self, selection: usize) -> &[usize] {
        &self.projections[selection]
    }

    /// The projection count (one per selection the plan was built from).
    pub fn projection_count(&self) -> usize {
        self.projections.len()
    }

    /// Begins a row: zeroes the union row, clears the validity bitset and
    /// stamps the row's timestamp (UTC epoch seconds). Called once per
    /// step, before the producers write.
    pub fn begin_row(&mut self, local_timestamp: i64) {
        self.row.fill(0.0);
        self.validity.fill(0);
        self.local_timestamp = local_timestamp;
    }

    /// The per-slot declared reporting units.
    pub fn units(&self) -> &[Unit] {
        &self.units
    }

    /// The union row as recorded.
    pub fn row(&self) -> &[f64] {
        &self.row
    }

    /// The current row's timestamp as UTC epoch seconds. The run's UTC
    /// offset is not applied: a local-epoch stamp must not be assumed from
    /// this field (the row's timestamp column carries the offset).
    pub fn local_timestamp(&self) -> i64 {
        self.local_timestamp
    }

    /// Marks a slot produced. The nullability that reads this arrives with
    /// the batch builders; the producers write every slot today.
    pub fn set_valid(&mut self, slot: usize) {
        self.validity[slot / usize::BITS as usize] |= 1 << (slot % usize::BITS as usize);
    }

    /// Whether a slot was marked produced.
    pub fn valid(&self, slot: usize) -> bool {
        self.validity[slot / usize::BITS as usize] & (1 << (slot % usize::BITS as usize)) != 0
    }

    /// The row and its validity as one writer: the producers' index-write
    /// surface for the step. Every `write` marks the slot produced; a slot
    /// never written stays invalid and reads as null downstream.
    #[must_use]
    pub fn writer(&mut self) -> RowWriter<'_> {
        RowWriter {
            row: &mut self.row,
            units: &self.units,
            validity: &mut self.validity,
        }
    }

    /// Freezes a row plan from per-report projections already resolved
    /// against the union surface: the destinations entry's freeze resolves
    /// every report's selection against the catalogue first, the union of
    /// the resolved columns is the producer surface, and each projection
    /// lists the union slots of one report's columns, in the report's own
    /// column order.
    #[must_use]
    pub fn from_resolved_projections(
        slots: usize,
        units: Vec<Unit>,
        projections: Vec<Vec<usize>>,
    ) -> Self {
        Self {
            slots,
            units,
            row: vec![0.0; slots],
            validity: vec![0; slots.div_ceil(usize::BITS as usize)],
            local_timestamp: 0,
            projections,
        }
    }

    /// Every selection's projection, in selection (report) order.
    #[must_use]
    pub fn projections(&self) -> &[Vec<usize>] {
        &self.projections
    }
}

/// The step's index-write surface over one row plan's row and validity: the
/// writers' `write` marks the slot produced, an unwritten slot stays
/// invalid and reads as null downstream.
pub struct RowWriter<'a> {
    row: &'a mut [f64],
    units: &'a [Unit],
    validity: &'a mut [u64],
}

impl RowWriter<'_> {
    /// Writes a produced value and marks the slot valid. The value is SI
    /// (every run-state value is, from B-14's run-state commit); the
    /// slot's declared reporting unit converts it once here.
    pub fn write(&mut self, slot: usize, value: f64) {
        let unit = self.units.get(slot).copied().unwrap_or(Unit::None);
        self.row[slot] = unit.from_si(value);
        self.mark_valid(slot);
    }

    /// Writes a value the producer has, or leaves the slot invalid when it
    /// has none: the not-applicable cell is null, never a substituted 0.0.
    pub fn write_opt(&mut self, slot: usize, value: Option<f64>) {
        if let Some(value) = value {
            self.write(slot, value);
        }
    }

    /// Accumulates into a summed cell and marks the slot valid (the
    /// zeroed accumulator is the value). The value is SI; the slot's
    /// declared unit converts it once.
    pub fn add(&mut self, slot: usize, value: f64) {
        let unit = self.units.get(slot).copied().unwrap_or(Unit::None);
        self.row[slot] += unit.from_si(value);
        self.mark_valid(slot);
    }

    /// Marks a slot produced without writing it: the summed cells' zeroed
    /// accumulator is the value.
    pub fn mark_valid(&mut self, slot: usize) {
        self.validity[slot / usize::BITS as usize] |= 1 << (slot % usize::BITS as usize);
    }

    /// Whether a slot was marked produced.
    #[must_use]
    pub fn valid(&self, slot: usize) -> bool {
        self.validity[slot / usize::BITS as usize] & (1 << (slot % usize::BITS as usize)) != 0
    }

    /// The row as written, for the readers that follow the producers.
    #[must_use]
    pub fn row(&self) -> &[f64] {
        self.row
    }
}

/// Resolves one report's selection against the catalogue: the variable
/// names the report records, in the order its columns carry.
///
/// A `Verbosity` selection resolves to every catalogue variable emitted at
/// the tier (the observe gate honoured, as the schema builder honours it);
/// a `Named` selection resolves exactly and case-sensitively, the error
/// listing each unknown name with its nearest catalogue names; a `Pattern`
/// selection globs over the catalogue names and errors when it matches
/// nothing.
///
/// # Errors
///
/// [`RowPlanError::UnknownNames`] and [`RowPlanError::EmptyPattern`], the
/// same errors [`RowPlan::build`] raises against a surface.
pub fn resolve_selection(
    selection: &Selection,
    catalogue: &Catalogue,
) -> Result<Vec<String>, RowPlanError> {
    let names: Vec<&str> = catalogue
        .variables()
        .iter()
        .map(|def| def.name.as_ref())
        .collect();
    match selection {
        Selection::Verbosity(verbosity) => Ok(catalogue
            .variables_at_verbosity(*verbosity)
            .iter()
            .map(|def| def.name.to_string())
            .collect()),
        Selection::Diagnostics => Ok(catalogue
            .diagnostics_variables()
            .iter()
            .map(|def| def.name.to_string())
            .collect()),
        Selection::Named(selected) => resolve_named(selected, &names)
            .map(|slots| slots.iter().map(|&s| names[s].to_string()).collect()),
        Selection::Pattern(pattern) => {
            let matched: Vec<String> = names
                .iter()
                .filter(|name| glob_match(pattern, name))
                .map(|name| (*name).to_string())
                .collect();
            if matched.is_empty() {
                Err(RowPlanError::EmptyPattern(pattern.clone()))
            } else {
                Ok(matched)
            }
        }
    }
}

/// Resolves a `Named` selection: every name must resolve exactly and
/// case-sensitively; the error lists each unknown name with up to three
/// nearest surface names by edit distance, ties broken by surface order.
fn resolve_named(selected: &[String], names: &[&str]) -> Result<Vec<usize>, RowPlanError> {
    let mut projection = Vec::with_capacity(selected.len());
    let mut unknown: Vec<String> = Vec::new();
    for name in selected {
        match names.iter().position(|candidate| candidate == name) {
            Some(slot) => projection.push(slot),
            None => unknown.push(format_unknown(name, names)),
        }
    }
    if unknown.is_empty() {
        Ok(projection)
    } else {
        Err(RowPlanError::UnknownNames(unknown.join("; ")))
    }
}

/// Formats one unknown name with its up to three nearest catalogue names
/// (edit distance, ties by catalogue order).
fn format_unknown(name: &str, names: &[&str]) -> String {
    let mut nearest: Vec<(usize, &str)> = names
        .iter()
        .map(|candidate| (edit_distance(name, candidate), *candidate))
        .collect();
    nearest.sort();
    let suggestions: Vec<String> = nearest
        .iter()
        .take(3)
        .map(|(_, candidate)| format!("'{candidate}'"))
        .collect();
    format!("'{name}' (nearest: {})", suggestions.join(", "))
}

/// Levenshtein edit distance over single characters, two-row dynamic
/// programming, no allocation beyond the second row.
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current: Vec<usize> = vec![0; b.len() + 1];
    for (i, ca) in a.chars().enumerate() {
        current[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(ca != *cb);
            current[j + 1] = (previous[j + 1] + 1).min(current[j] + 1).min(substitution);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

/// Glob match over variable names: `*` matches any run of characters
/// (including the empty run), `?` exactly one, everything else is literal
/// (brackets have no special meaning), case-sensitive. Iterative
/// backtracking over the last `*`.
fn glob_match(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
    let (mut p, mut n) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut star_name = 0usize;
    while n < name.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == name[n]) {
            p += 1;
            n += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            star_name = n;
            p += 1;
        } else if let Some(star_p) = star {
            star_name += 1;
            n = star_name;
            p = star_p + 1;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == '*' {
        p += 1;
    }
    p == pattern.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::datatypes::{DataType, Field};
    use chrono::Duration;

    /// A surface over the level-0 and verbosity-2 columns of a small
    /// schema, for the selection-resolution tests.
    fn test_surface() -> SchemaSurface {
        let schema = Schema::new(vec![
            Field::new("Time", DataType::Utf8, false),
            Field::new("Total Electric Power (kW)", DataType::Float64, true),
            Field::new("Total Gas Power (therms/hour)", DataType::Float64, true),
            Field::new("Outdoor Dry Bulb (C)", DataType::Float64, true),
            Field::new("Temperature - Indoor (C)", DataType::Float64, true),
            Field::new("Temperature - Attic (C)", DataType::Float64, true),
        ]);
        SchemaSurface::new(&schema)
    }

    #[test]
    fn unknown_selection_name_errors_listing_unknown_names() {
        let surface = test_surface();

        // A mixed selection: the known name resolves, the unknown one is
        // named by the error.
        let err = RowPlan::build(
            &[
                Selection::Named(vec![
                    "Total Electric Power (kW)".to_string(),
                    "Nope".to_string(),
                ]),
                Selection::Verbosity(2),
            ],
            &surface,
        )
        .expect_err("an unknown name must fail the plan");
        assert!(
            err.to_string().contains("Nope"),
            "the error must name the unknown variable, got: {err}"
        );

        // A case-typo'd name is unknown (resolution is case-sensitive) and
        // the error's first suggestion is the catalogue name nearest by
        // edit distance.
        let err = RowPlan::build(
            &[Selection::Named(vec![
                "Total Electric Power (kw)".to_string(),
            ])],
            &surface,
        )
        .expect_err("a case-typo'd name must fail the plan");
        let message = err.to_string();
        assert!(
            message.contains("'Total Electric Power (kw)'"),
            "the error must name the requested variable, got: {message}"
        );
        let first_suggestion = message
            .split("(nearest: ")
            .nth(1)
            .and_then(|rest| rest.split('\'').nth(1));
        assert_eq!(
            first_suggestion,
            Some("Total Electric Power (kW)"),
            "the first suggestion must be the nearest catalogue name by edit \
             distance, got: {message}"
        );
    }

    #[test]
    fn pattern_matching_nothing_is_an_error() {
        let surface = test_surface();
        let err = RowPlan::build(
            &[Selection::Pattern("no such variable *".to_string())],
            &surface,
        )
        .expect_err("a pattern matching nothing must fail the plan");
        assert!(
            matches!(&err, RowPlanError::EmptyPattern(pattern) if pattern == "no such variable *"),
            "got: {err:?}"
        );
    }

    #[test]
    fn pattern_glob_semantics() {
        let surface = test_surface();
        let build = |pattern: &str| {
            RowPlan::build(&[Selection::Pattern(pattern.to_string())], &surface)
                .map(|plan| plan.projection(0).to_vec())
        };

        // `*` matches every variable.
        assert_eq!(
            build("*").expect("star matches"),
            (0..5).collect::<Vec<usize>>(),
        );
        // A suffix star matches by prefix.
        assert_eq!(
            build("Temperature - *").expect("prefix star matches"),
            vec![3, 4],
        );
        // `?` matches exactly one character.
        assert_eq!(
            build("Temperature - ?ndoor (C)").expect("question mark matches"),
            vec![3],
        );
        // Brackets are literal, not a character class: the pattern matches
        // only a name that carries the brackets.
        assert!(build("Temperature - [I]ndoor (C)").is_err());
        // Case-sensitive: no match for a case-typo'd pattern.
        assert!(build("temperature - *").is_err());
    }

    #[test]
    fn projections_share_the_union_row_and_begin_row_resets_it() {
        let surface = test_surface();
        let plan = RowPlan::build(
            &[
                Selection::Named(vec!["Total Electric Power (kW)".to_string()]),
                Selection::Verbosity(2),
            ],
            &surface,
        )
        .expect("plan builds");
        assert_eq!(plan.slots(), 5);
        assert_eq!(plan.projection_count(), 2);
        // The named projection is the single slot the verbosity projection
        // also carries, at the same position in the union row.
        assert_eq!(plan.projection(0), &[0]);
        assert!(plan.projection(1).contains(&0));

        let mut plan = plan;
        plan.begin_row(1_700_000_000);
        assert_eq!(plan.local_timestamp(), 1_700_000_000);
        assert!(plan.row().iter().all(|&v| v == 0.0));
        assert!(!plan.valid(0));
        plan.writer().write(0, 4.2);
        assert_eq!(plan.row()[0], 4.2);
        assert!(plan.valid(0));
        assert!(!plan.valid(1));
        plan.begin_row(1_700_000_060);
        assert_eq!(plan.row()[0], 0.0);
        assert!(!plan.valid(0));
        assert_eq!(plan.local_timestamp(), 1_700_000_060);
    }

    #[test]
    fn edit_distance_orders_suggestions() {
        assert_eq!(edit_distance("kw", "kW"), 1);
        assert_eq!(edit_distance("abc", "abc"), 0);
        assert_eq!(edit_distance("flat", "kart"), 3);
    }

    #[test]
    fn frequency_serde_round_trips_every_variant() {
        // The unit variants are their snake_case strings; `Minutes` is a
        // one-key table. Both spellings are what a config's TOML carries,
        // so the round-trip runs through the embedded form: a table's
        // `frequency` key.
        #[derive(Deserialize)]
        struct Wrapped {
            #[serde(default)]
            frequency: Frequency,
        }
        let cases: [(Frequency, &str); 7] = [
            (Frequency::Timestep, "\"timestep\""),
            (Frequency::Hourly, "\"hourly\""),
            (Frequency::Daily, "\"daily\""),
            (Frequency::Monthly, "\"monthly\""),
            (Frequency::Annual, "\"annual\""),
            (Frequency::RunPeriod, "\"run_period\""),
            (
                Frequency::Minutes(NonZeroU32::new(15).expect("nonzero")),
                "{ minutes = 15 }",
            ),
        ];
        for (frequency, toml) in cases {
            let document = format!("frequency = {toml}");
            let wrapped: Wrapped = toml::from_str(&document).expect("the TOML spelling parses");
            assert_eq!(wrapped.frequency, frequency, "{document}");
        }
        // The bare form, as a JSON document of the same externally tagged
        // shape.
        for (frequency, json) in [
            (Frequency::Timestep, "\"timestep\""),
            (Frequency::Hourly, "\"hourly\""),
            (Frequency::RunPeriod, "\"run_period\""),
            (
                Frequency::Minutes(NonZeroU32::new(15).expect("nonzero")),
                "{\"minutes\":15}",
            ),
        ] {
            assert_eq!(
                serde_json::from_str::<Frequency>(json).expect("the JSON spelling parses"),
                frequency,
                "{json}"
            );
        }
        // The default is the timestep.
        assert_eq!(Frequency::default(), Frequency::Timestep);
        // A minutes value that is not a whole number of minutes is refused
        // by the type: zero does not construct a NonZeroU32, and the
        // frequency's own rule rejects the rest.
        assert!(toml::from_str::<Wrapped>("frequency = { minutes = 0 }").is_err());
    }

    #[test]
    fn minutes_not_dividing_the_hour_is_a_validate_error() {
        let step = Duration::seconds(900);
        for n in [7_u32, 90] {
            let err = Frequency::Minutes(NonZeroU32::new(n).expect("nonzero"))
                .validate(step)
                .expect_err("minutes that do not divide the hour are a validate error");
            let message = err.to_string();
            assert!(
                message.contains(&n.to_string()) && message.contains("divide the hour"),
                "the error names the value and the rule, got: {message}"
            );
        }
        // A whole divisor of the hour whose period still misses the step:
        // 15 minutes at a 600 s step is 1.5 steps.
        let err = Frequency::Minutes(NonZeroU32::new(15).expect("nonzero"))
            .validate(Duration::seconds(600))
            .expect_err("a period that is not a whole number of steps is a validate error");
        let message = err.to_string();
        assert!(message.contains("not a whole multiple"), "got: {message}");
        // The same period at a 300 s and a 900 s step is fine.
        for step_s in [300_i64, 900] {
            Frequency::Minutes(NonZeroU32::new(15).expect("nonzero"))
                .validate(Duration::seconds(step_s))
                .expect("the period tiles the step");
        }
    }

    #[test]
    fn frequency_not_multiple_of_step_is_a_validate_error() {
        // An hourly period at a 7200 s step cannot tile the hour.
        let err = Frequency::Hourly
            .validate(Duration::seconds(7200))
            .expect_err("an hour at a 7200 s step is a validate error");
        assert!(
            err.to_string().contains("not a whole multiple"),
            "got: {err}"
        );
        // Every calendar period is whole days: the timestep must divide a
        // day, so a 7000 s step fails the month and the day alike.
        for frequency in [Frequency::Daily, Frequency::Monthly, Frequency::Annual] {
            frequency
                .validate(Duration::seconds(7000))
                .expect_err("a 7000 s step does not tile a day");
        }
        // The timestep and the run period carry no tiling requirement.
        Frequency::Timestep
            .validate(Duration::seconds(7000))
            .expect("the timestep frequency takes no period check");
        Frequency::RunPeriod
            .validate(Duration::seconds(7000))
            .expect("the run period is one row over whatever the run spans");
        // The ordinary resolutions pass everywhere.
        for frequency in [
            Frequency::Hourly,
            Frequency::Daily,
            Frequency::Monthly,
            Frequency::Annual,
            Frequency::RunPeriod,
        ] {
            frequency
                .validate(Duration::seconds(900))
                .expect("a 900 s step tiles every period");
        }
    }
}
