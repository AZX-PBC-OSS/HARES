//! The output variable catalogue, built from typed declarations.
//!
//! Every variable the engine can produce is declared once,
//! typed (`Unit`, `VariableKind`, `Aggregation`, `Sampling`, nullability),
//! and the catalogue merges the declarations: the dwelling-level static
//! table, a static per-capability table for the core-output columns, a
//! static per-zone-template table, each equipment's typed declarations and
//! each actor's. `build_schema` derives the Arrow schema from the catalogue,
//! so a variable the engine produces but nothing declares cannot silently
//! lose its column: the schema pins compare the catalogue-derived schema
//! against the recorded one at every verbosity.
//!
//! The equipment declarations are merged, not discarded: one telemetry key
//! has one declaration across the workspace. Two equipment declaring the
//! same key with different properties contradict each other, and a static
//! telemetry-derived column whose key the equipment declare differently
//! contradicts the declaration; both are assembly errors naming both sides.
//! The static tables are corrected to the declarations where they disagree
//! (the declarations are the authority; the tables mirror them).
//!
//! The per-equipment column set stays bug-compatible with the pre-catalogue
//! classifier surface: the same name-classifier functions
//! (`equipment_name_to_end_use`, `is_hvac_or_wh`, `is_ev`, and kin) are
//! called from one function here — the catalogue builder's per-equipment
//! expansion — until the descriptors' `end_use` replaces them and the
//! classifiers are deleted.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use arrow::datatypes::{DataType, Field, Schema};
use serde::{Deserialize, Serialize};

use hares_types::{
    AcrossHomes, Aggregation, EndUse, EquipmentId, FuelType, Sampling, TelemetryField, Unit,
    VariableKind, ZoneId,
};

use crate::hpxml::EquipmentSpec;
use crate::hpxml::equipment::canonical_instance_namer;
use crate::output::columns::{
    end_use_display_name, end_use_electric_power_column, equipment_name_to_end_use,
    fuel_reports_gas_power_column, has_soc, is_cooling_equipment, is_ev, is_heat_pump_heater,
    is_hvac_or_wh, is_pv, is_reserved_output_column_name,
};

/// An error raised while building a [`Catalogue`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CatalogueError {
    /// Two variables share one name.
    #[error("duplicate output variable name '{name}' (sources: {first}, then {second})")]
    DuplicateName {
        /// The colliding variable name.
        name: String,
        /// Where the first declaration came from.
        first: String,
        /// Where the second declaration came from.
        second: String,
    },
    /// An aggregation that contradicts the variable's kind.
    #[error(
        "variable '{name}' declares aggregation {aggregation:?}, which contradicts its kind {kind}"
    )]
    AggregationMismatch {
        /// The variable name.
        name: String,
        /// The declared aggregation.
        aggregation: Aggregation,
        /// The declared kind, as the catalogue sees it.
        kind: String,
    },
    /// Two declarations of one telemetry key disagree on its properties:
    /// two equipment declaring the same channel differently, or a static
    /// telemetry-derived column whose declaration contradicts the merged
    /// declaration of the key it reads. Names both sides.
    #[error("declared channel '{key}' has contradicting declarations: {first}; {second}")]
    DeclarationConflict {
        /// The telemetry key both declarations describe.
        key: String,
        /// The first declaration's source and properties.
        first: String,
        /// The second declaration's source and properties.
        second: String,
    },
}

/// Where a catalogue variable comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum VariableSource {
    /// A dwelling-level column (totals, context, envelope gains, setpoints).
    Dwelling,
    /// A per-zone column for the zone with this id.
    Zone(ZoneId),
    /// A per-equipment column owned by the equipment with this id.
    Equipment(EquipmentId),
    /// An actor column owned by the actor registered under this name.
    Actor(Arc<str>),
}

impl VariableSource {
    /// The lowercase source tag (`dwelling`, `zone:<id>`, `equipment:<id>`,
    /// `actor:<name>`).
    #[must_use]
    pub fn tag(&self) -> String {
        match self {
            Self::Dwelling => "dwelling".to_string(),
            Self::Zone(id) => format!("zone:{}", id.0),
            Self::Equipment(id) => format!("equipment:{}", id.0),
            Self::Actor(name) => format!("actor:{name}"),
        }
    }
}

impl std::fmt::Display for VariableSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.tag())
    }
}

fn kind_tag(kind: &VariableKind) -> String {
    match kind {
        VariableKind::Energy => "energy".to_string(),
        VariableKind::Power => "power".to_string(),
        VariableKind::Temperature => "temperature".to_string(),
        VariableKind::Ratio => "ratio".to_string(),
        VariableKind::State => "state".to_string(),
        VariableKind::Enum { labels } => format!("enum({} labels)", labels.len()),
        VariableKind::Quantity { across_homes } => match across_homes {
            AcrossHomes::Sum => "quantity(sum across homes)".to_string(),
            AcrossHomes::Mean => "quantity(mean across homes)".to_string(),
        },
    }
}

/// One output variable: a column the engine can produce, declared once with
/// its unit, kind, aggregation, sampling, nullability and source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VariableDef {
    /// The column name. Borrowed in the static tables, owned for
    /// per-equipment expansions.
    pub name: Cow<'static, str>,
    /// The variable's reporting unit; serialises as `Unit::symbol()`.
    pub unit: Unit,
    /// The variable's kind.
    pub kind: VariableKind,
    /// The over-time aggregation rule.
    pub aggregation: Aggregation,
    /// The sampling rule.
    pub sampling: Sampling,
    /// Whether the variable can be missing (null in the report).
    pub nullable: bool,
    /// Where the variable comes from.
    pub source: VariableSource,
    /// What the variable measures, from the declaring site.
    pub description: Cow<'static, str>,
}

impl VariableDef {
    /// Validates the declaration's internal consistency: the aggregation
    /// must not contradict the kind. `Energy` sums (the energy of the step
    /// accumulates); `Enum` takes `Mode`; `State` averages (a duty
    /// fraction); `Last` is legal only for a `Quantity` (a running total's
    /// value at period end).
    pub fn validate(&self) -> Result<(), CatalogueError> {
        use VariableKind as K;
        let kind = kind_tag(&self.kind);
        let ok = matches!(
            (&self.kind, self.aggregation),
            (K::Energy, Aggregation::Sum)
                | (K::Enum { .. }, Aggregation::Mode)
                | (K::State, Aggregation::Mean)
                | (
                    K::Quantity { .. },
                    Aggregation::Sum | Aggregation::Mean | Aggregation::Last
                )
                | (K::Power, Aggregation::Mean)
                | (K::Temperature, Aggregation::Mean)
                | (K::Ratio, Aggregation::Mean)
        );
        if ok {
            Ok(())
        } else {
            Err(CatalogueError::AggregationMismatch {
                name: self.name.to_string(),
                aggregation: self.aggregation,
                kind,
            })
        }
    }
}

/// The per-equipment column families, decided once per instance by the ONE
/// classifier function the catalogue builder is allowed to call. Carries
/// today's outcomes (which families exist for which name) so the
/// catalogue-derived schema is bug-compatible with the pre-catalogue one.
#[derive(Debug, Clone)]
pub struct EquipmentColumnFamilies {
    /// Whether the equipment burns a fuel that reports a gas power column.
    pub has_gas: bool,
    /// Whether the name classifies as HVAC or water heating.
    pub is_hvac: bool,
    /// Whether the name classifies as cooling equipment (SHR, latent gains).
    pub is_cooling: bool,
    /// Whether the name classifies as a heat pump heater (defrost, ER).
    pub is_hp_heater: bool,
    /// Whether the name classifies as storage (SOC column).
    pub has_soc: bool,
    /// Whether the name classifies as PV.
    pub is_pv: bool,
    /// Whether the name classifies as an EV.
    pub is_ev: bool,
    /// The end use the name resolves to.
    pub end_use: EndUse,
}

impl EquipmentColumnFamilies {
    /// Classifies one equipment name. The single home of the classifier
    /// calls inside the catalogue builder.
    #[must_use]
    pub fn of(name: &str, fuel: FuelType) -> Self {
        Self {
            has_gas: fuel_reports_gas_power_column(fuel),
            is_hvac: is_hvac_or_wh(name),
            is_cooling: is_cooling_equipment(name),
            is_hp_heater: is_heat_pump_heater(name),
            has_soc: has_soc(name),
            is_pv: is_pv(name),
            is_ev: is_ev(name),
            end_use: equipment_name_to_end_use(name),
        }
    }
}

/// One equipment's catalogue input: its base name (the descriptor's name,
/// numbered into an instance name when it repeats), fuel, id and typed
/// telemetry declarations.
#[derive(Debug, Clone)]
pub struct CatalogueEquipment {
    /// The equipment's base name (`"Battery"`; the builder numbers
    /// repeats).
    pub name: String,
    /// The spec-level instance name (`"PV #1"`), which overrides the
    /// numbering when present.
    pub instance_name: Option<String>,
    /// The equipment's id.
    pub id: EquipmentId,
    /// The equipment's fuel.
    pub fuel: FuelType,
    /// The equipment's typed telemetry declarations
    /// (`descriptor().telemetry_fields`).
    pub telemetry: Vec<TelemetryField>,
}

/// One actor's catalogue input: its registered name and typed declarations.
#[derive(Debug, Clone)]
pub struct CatalogueActor {
    /// The registered name (`Actor::name()`).
    pub name: Arc<str>,
    /// The actor's typed declarations (`Actor::output_variables()`).
    pub output_variables: Vec<TelemetryField>,
}

/// Everything the catalogue is built from, in equipment and zone order.
#[derive(Debug, Clone, Default)]
pub struct CatalogueInput {
    /// The equipment, in roster order.
    pub equipment: Vec<CatalogueEquipment>,
    /// The zones, in schema order (`(id, display name)`, Indoor included).
    pub zones: Vec<(ZoneId, String)>,
    /// The actors, in registration order.
    pub actors: Vec<CatalogueActor>,
}

/// The catalogue of every output variable a built model can produce.
///
/// Variables are held in schema order (the order the schema blocks emit
/// in), so the Arrow schema at any verbosity is the catalogue filtered to
/// the variables' minimum verbosity, preserving order.
#[derive(Debug, Clone, Default)]
pub struct Catalogue {
    defs: Vec<VariableDef>,
    /// `(min verbosity, observe-only)` per def.
    tags: Vec<(u8, bool)>,
    index: HashMap<String, usize>,
    /// Column name → the telemetry key the column's value is read from at
    /// record time, for the per-equipment telemetry-derived columns only.
    telemetry_keys: HashMap<String, &'static str>,
    /// The instance-qualified equipment names the catalogue was built from,
    /// in order; the schema metadata derivation (the lighting-created
    /// flags) reads them.
    equipment_names: Vec<String>,
}

impl Catalogue {
    /// Every variable, in schema order.
    #[must_use]
    pub fn variables(&self) -> &[VariableDef] {
        &self.defs
    }

    /// Resolves a variable by column name.
    #[must_use]
    pub fn resolve(&self, name: &str) -> Option<&VariableDef> {
        self.index.get(name).map(|&i| &self.defs[i])
    }

    /// The telemetry key the named column's value is read from at record
    /// time, for the per-equipment telemetry-derived columns. The core
    /// output, dwelling, zone and actor columns read no telemetry key.
    #[must_use]
    pub fn telemetry_key(&self, column: &str) -> Option<&'static str> {
        self.telemetry_keys.get(column).copied()
    }

    /// The instance-qualified equipment names the catalogue was built from.
    #[must_use]
    pub fn equipment_names(&self) -> &[String] {
        &self.equipment_names
    }

    /// The number of variables.
    #[must_use]
    pub fn len(&self) -> usize {
        self.defs.len()
    }

    /// Whether the catalogue is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }

    /// The variables emitted at `verbosity`, in schema order, honouring the
    /// `observe` feature gate.
    #[must_use]
    pub fn variables_at_verbosity(&self, verbosity: u8) -> Vec<&VariableDef> {
        let mut selected = Vec::new();
        for i in 0..self.defs.len() {
            let (min_v, observe_only) = self.tags[i];
            if min_v <= verbosity && (!observe_only || cfg!(feature = "observe")) {
                selected.push(&self.defs[i]);
            }
        }
        selected
    }

    /// The variable's `(min verbosity, observe-only)` tag, resolved by
    /// column name.
    #[must_use]
    pub fn tag(&self, name: &str) -> Option<(u8, bool)> {
        self.index.get(name).map(|&i| self.tags[i])
    }
}

/// Builds the catalogue: the dwelling statics, the per-equipment columns in
/// schema-block order, the per-zone columns, and the actor columns.
///
/// A duplicate name, or an aggregation that contradicts its kind, is an
/// assembly error.
#[allow(clippy::too_many_lines)]
pub fn build_catalogue(input: &CatalogueInput) -> Result<Catalogue, CatalogueError> {
    let mut defs: Vec<VariableDef> = Vec::new();
    let mut tags: Vec<(u8, bool)> = Vec::new();
    let mut telemetry_keys: HashMap<String, &'static str> = HashMap::new();

    // The merged declaration table: one telemetry key, one declaration. Two
    // equipment declaring the same key with different properties contradict
    // each other: an assembly error naming both.
    let mut declared: HashMap<&str, (String, &TelemetryField)> = HashMap::new();
    for eq in &input.equipment {
        let tag = equipment_tag(eq);
        for field in &eq.telemetry {
            match declared.get(field.name.as_ref()) {
                Some((first_tag, first)) if fields_contradict(first, field) => {
                    return Err(CatalogueError::DeclarationConflict {
                        key: field.name.to_string(),
                        first: format!("{first_tag} declares {}", field_summary(first)),
                        second: format!("{tag} declares {}", field_summary(field)),
                    });
                }
                _ => {
                    declared.insert(field.name.as_ref(), (tag.clone(), field));
                }
            }
        }
    }

    // Instance-qualified names, deduplicated and reserved-filtered, exactly
    // as the pre-catalogue build_schema computed them.
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for eq in &input.equipment {
        *counts.entry(eq.name.as_str()).or_insert(0) += 1;
    }
    let mut indices: HashMap<&str, usize> = HashMap::new();
    let instances: Vec<(&CatalogueEquipment, String)> = input
        .equipment
        .iter()
        .map(|eq| {
            let name = if let Some(instance) = &eq.instance_name {
                instance.clone()
            } else if counts[eq.name.as_str()] > 1 {
                let idx = indices.entry(eq.name.as_str()).or_insert(0);
                *idx += 1;
                canonical_instance_namer(eq.name.as_str(), *idx)
            } else {
                eq.name.clone()
            };
            (eq, name)
        })
        .collect();

    // ── Block: verbosity 0 — totals and always-present context ─────────────
    push_statics(&mut defs, &mut tags, 0)?;

    // ── Block: verbosity 1 — per-equipment power ───────────────────────────
    for (eq, name) in &instances {
        let families = EquipmentColumnFamilies::of(name, eq.fuel);
        let member = format!("{name} {ELECTRIC_SUFFIX}");
        if is_reserved_output_column_name(&member) {
            continue;
        }
        push(
            &mut defs,
            &mut tags,
            variable_for(CORE_OUTPUT_VARIABLES[0].1.clone(), &member, eq.id),
            1,
            false,
        )?;
        if families.has_gas {
            push(
                &mut defs,
                &mut tags,
                variable_for(
                    CORE_OUTPUT_VARIABLES[1].1.clone(),
                    &format!("{name} {GAS_SUFFIX}"),
                    eq.id,
                ),
                1,
                false,
            )?;
        }
    }

    // Per-end-use aggregates: one column per end use that has at least one
    // equipment, keyed on the spec's base name, in spec order.
    let mut seen_end_uses: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for eq in &input.equipment {
        let end_use = equipment_name_to_end_use(&eq.name);
        if seen_end_uses.insert(end_use_display_name(&end_use).to_string()) {
            let name = end_use_electric_power_column(&end_use);
            push(
                &mut defs,
                &mut tags,
                variable_for(END_USE_AGGREGATE_VARIABLE.clone(), &name, eq.id),
                1,
                false,
            )?;
        }
    }

    // ── Block: verbosity 2 — per-zone temperatures, unmet load, ground ─────
    for (zone_id, label) in &input.zones {
        if label != "Indoor" {
            push(
                &mut defs,
                &mut tags,
                zone_variable(&ZONE_TEMP_TEMPLATE, *zone_id, label),
                2,
                false,
            )?;
        }
    }
    push_statics(&mut defs, &mut tags, 2)?;

    // ── Block: verbosity 3 — per-equipment state ───────────────────────────
    for (eq, name) in &instances {
        let families = EquipmentColumnFamilies::of(name, eq.fuel);
        let member = format!("{name} {ELECTRIC_SUFFIX}");
        if is_reserved_output_column_name(&member) {
            continue;
        }
        push(
            &mut defs,
            &mut tags,
            variable_for(
                CORE_OUTPUT_VARIABLES[2].1.clone(),
                &format!("{name} {MODE_SUFFIX}"),
                eq.id,
            ),
            3,
            false,
        )?;
        if families.is_hvac {
            push(
                &mut defs,
                &mut tags,
                variable_for(
                    CORE_OUTPUT_VARIABLES[3].1.clone(),
                    &format!("{name} {SETPOINT_SUFFIX}"),
                    eq.id,
                ),
                3,
                false,
            )?;
        }
        if families.has_soc {
            push(
                &mut defs,
                &mut tags,
                variable_for(
                    CORE_OUTPUT_VARIABLES[4].1.clone(),
                    &format!("{name} {SOC_SUFFIX}"),
                    eq.id,
                ),
                3,
                false,
            )?;
        }
    }

    // ── Block: verbosity 4 — per-equipment energy, HVAC delivery ───────────
    for (eq, name) in &instances {
        let member = format!("{name} {ELECTRIC_SUFFIX}");
        if is_reserved_output_column_name(&member) {
            continue;
        }
        push(
            &mut defs,
            &mut tags,
            variable_for(
                CORE_OUTPUT_VARIABLES[5].1.clone(),
                &format!("{name} {ENERGY_SUFFIX}"),
                eq.id,
            ),
            4,
            false,
        )?;
    }
    push_statics(&mut defs, &mut tags, 4)?;

    // ── Block: verbosity 5 — reactive power, power factor, gains ───────────
    for (eq, name) in &instances {
        let member = format!("{name} {ELECTRIC_SUFFIX}");
        if is_reserved_output_column_name(&member) {
            continue;
        }
        push(
            &mut defs,
            &mut tags,
            variable_for(
                CORE_OUTPUT_VARIABLES[6].1.clone(),
                &format!("{name} {REACTIVE_SUFFIX}"),
                eq.id,
            ),
            5,
            false,
        )?;
        push(
            &mut defs,
            &mut tags,
            variable_for(
                CORE_OUTPUT_VARIABLES[7].1.clone(),
                &format!("{name} {POWER_FACTOR_SUFFIX}"),
                eq.id,
            ),
            5,
            false,
        )?;
    }
    push_statics(&mut defs, &mut tags, 5)?;

    // ── Block: verbosity 6 — envelope gains and per-zone attribution ───────
    push_statics(&mut defs, &mut tags, 6)?;
    for (zone_id, label) in &input.zones {
        if label != "Indoor" {
            push(
                &mut defs,
                &mut tags,
                zone_variable(&INFILTRATION_TEMPLATE, *zone_id, label),
                6,
                false,
            )?;
            push(
                &mut defs,
                &mut tags,
                zone_variable(&LWR_TEMPLATE, *zone_id, label),
                6,
                false,
            )?;
        }
    }
    for (zone_id, label) in &input.zones {
        push(
            &mut defs,
            &mut tags,
            zone_variable(&HEATING_DELIVERED_TEMPLATE, *zone_id, label),
            6,
            false,
        )?;
        push(
            &mut defs,
            &mut tags,
            zone_variable(&COOLING_DELIVERED_TEMPLATE, *zone_id, label),
            6,
            false,
        )?;
    }

    // ── Block: verbosity 7 — schedule, defrost/ER, HVAC performance ────────
    for (eq, name) in &instances {
        let families = EquipmentColumnFamilies::of(name, eq.fuel);
        let member = format!("{name} {ELECTRIC_SUFFIX}");
        if is_reserved_output_column_name(&member) {
            continue;
        }
        push(
            &mut defs,
            &mut tags,
            variable_for(
                CORE_OUTPUT_VARIABLES[8].1.clone(),
                &format!("{name} {SCHEDULE_SUFFIX}"),
                eq.id,
            ),
            7,
            false,
        )?;
        if families.is_hp_heater {
            for t in HP_HEATER_V7_COLUMNS {
                push(
                    &mut defs,
                    &mut tags,
                    telemetry_variable(t, name, eq, &declared, &mut telemetry_keys)?,
                    7,
                    false,
                )?;
            }
        }
        if families.is_hvac {
            if families.is_cooling {
                for t in COOLING_V7_COLUMNS {
                    push(
                        &mut defs,
                        &mut tags,
                        telemetry_variable(t, name, eq, &declared, &mut telemetry_keys)?,
                        7,
                        false,
                    )?;
                }
            }
            // Speed and Main Power read CoreOutput; Fan Power and Runtime
            // Fraction read telemetry. The interleaving is today's column
            // order (columns.rs's v7 block): speed, main, fan, runtime
            // fraction, capacity, COP.
            let core = |i: usize| {
                let t = &CORE_OUTPUT_VARIABLES[i].1;
                variable_for(t.clone(), &format!("{name} {}", t.name), eq.id)
            };
            let mut hvac_v7: Vec<VariableDef> = Vec::with_capacity(6);
            hvac_v7.push(core(HVAC_V7_CORE_COLUMNS_START));
            hvac_v7.push(core(HVAC_V7_CORE_COLUMNS_START + 1));
            for t in HVAC_V7_TELEMETRY_COLUMNS {
                hvac_v7.push(telemetry_variable(
                    t,
                    name,
                    eq,
                    &declared,
                    &mut telemetry_keys,
                )?);
            }
            hvac_v7.push(core(HVAC_V7_CORE_COLUMNS_START + 2));
            hvac_v7.push(core(HVAC_V7_CORE_COLUMNS_START + 3));
            for def in hvac_v7 {
                push(&mut defs, &mut tags, def, 7, false)?;
            }
        }
    }
    push_statics(&mut defs, &mut tags, 7)?;

    // ── Block: verbosity 8 — per-equipment telemetry diagnostics ───────────
    for (eq, name) in &instances {
        let families = EquipmentColumnFamilies::of(name, eq.fuel);
        let member = format!("{name} {ELECTRIC_SUFFIX}");
        if is_reserved_output_column_name(&member) {
            continue;
        }
        for (applies, table) in [
            (families.is_hvac, &HVAC_V8_COLUMNS),
            (families.is_hp_heater, &HP_HEATER_V8_COLUMNS),
            (families.is_pv, &PV_V8_COLUMNS),
            (families.is_ev, &EV_V8_COLUMNS),
        ] {
            if !applies {
                continue;
            }
            for t in table.iter() {
                push(
                    &mut defs,
                    &mut tags,
                    telemetry_variable(t, name, eq, &declared, &mut telemetry_keys)?,
                    8,
                    false,
                )?;
            }
        }
    }

    // ── Actor columns: appended last, present at every verbosity ───────────
    for actor in &input.actors {
        let mut keys: Vec<&TelemetryField> = actor.output_variables.iter().collect();
        keys.sort_by(|a, b| a.name.as_ref().cmp(b.name.as_ref()));
        for field in keys {
            push(
                &mut defs,
                &mut tags,
                VariableDef {
                    name: Cow::Owned(format!("actor:{}:{}", actor.name, field.name)),
                    unit: field.unit,
                    kind: field.kind.clone(),
                    aggregation: field.aggregation,
                    sampling: field.sampling,
                    nullable: true,
                    source: VariableSource::Actor(actor.name.clone()),
                    description: field.description.clone(),
                },
                0,
                false,
            )?;
        }
    }

    // A duplicate name is an assembly error naming both sources.
    let mut index: HashMap<String, usize> = HashMap::with_capacity(defs.len());
    for (i, def) in defs.iter().enumerate() {
        let name = def.name.to_string();
        if let Some(&first) = index.get(&name) {
            return Err(CatalogueError::DuplicateName {
                name,
                first: defs[first].source.tag(),
                second: def.source.tag(),
            });
        }
        index.insert(name, i);
    }

    let equipment_names = instances.iter().map(|(_, name)| name.clone()).collect();
    Ok(Catalogue {
        defs,
        tags,
        index,
        telemetry_keys,
        equipment_names,
    })
}

/// Validates and appends one variable with its schema-derivation tags.
fn push(
    defs: &mut Vec<VariableDef>,
    tags: &mut Vec<(u8, bool)>,
    def: VariableDef,
    min_v: u8,
    observe_only: bool,
) -> Result<(), CatalogueError> {
    def.validate()?;
    defs.push(def);
    tags.push((min_v, observe_only));
    Ok(())
}

/// Pushes the dwelling-level static variables of one verbosity block.
fn push_statics(
    defs: &mut Vec<VariableDef>,
    tags: &mut Vec<(u8, bool)>,
    min_verbosity: u8,
) -> Result<(), CatalogueError> {
    for (_name, def, level, observe_only) in DWELLING_VARIABLES {
        if *level == min_verbosity {
            push(defs, tags, def.clone(), *level, *observe_only)?;
        }
    }
    Ok(())
}

/// Overrides a template's name and source for one equipment instance.
fn variable_for(mut def: VariableDef, name: &str, id: EquipmentId) -> VariableDef {
    def.name = Cow::Owned(name.to_string());
    def.source = VariableSource::Equipment(id);
    def
}

/// The source tag for a catalogue input equipment, naming id and base name.
fn equipment_tag(eq: &CatalogueEquipment) -> String {
    format!("equipment:{} ({})", eq.id.0, eq.name)
}

/// A declaration's semantic properties, for a conflict message. The
/// description is prose and may differ between declaring sites; the unit,
/// kind (with its label table), aggregation, sampling and nullability
/// may not.
fn field_summary(field: &TelemetryField) -> String {
    format!(
        "unit '{}', kind {}, aggregation {:?}, sampling {:?}, nullable {}",
        field.unit.symbol(),
        kind_summary(&field.kind),
        field.aggregation,
        field.sampling,
        field.nullable,
    )
}

fn def_summary(def: &VariableDef) -> String {
    format!(
        "unit '{}', kind {}, aggregation {:?}, sampling {:?}, nullable {}",
        def.unit.symbol(),
        kind_summary(&def.kind),
        def.aggregation,
        def.sampling,
        def.nullable,
    )
}

fn kind_summary(kind: &VariableKind) -> String {
    match kind {
        VariableKind::Enum { labels } => {
            let spellings: Vec<&str> = (0..labels.len())
                .map(|i| labels.get(i).unwrap_or_default())
                .collect();
            format!("enum {spellings:?}")
        }
        other => kind_tag(other),
    }
}

/// Whether two declarations of one telemetry key disagree on any semantic
/// property. Prose (the description) may differ; the unit, kind,
/// aggregation, sampling and nullability may not.
fn fields_contradict(a: &TelemetryField, b: &TelemetryField) -> bool {
    a.unit != b.unit
        || a.kind != b.kind
        || a.aggregation != b.aggregation
        || a.sampling != b.sampling
        || a.nullable != b.nullable
}

/// Builds one telemetry-derived column for one equipment instance: the
/// static table's declaration, checked against the merged declaration of
/// the telemetry key the column reads. A contradiction is an assembly
/// error naming both sides. Records the column's telemetry key for the
/// schema-enrichment check.
fn telemetry_variable(
    t: &TelemetryColumn,
    name: &str,
    eq: &CatalogueEquipment,
    declared: &HashMap<&str, (String, &TelemetryField)>,
    telemetry_keys: &mut HashMap<String, &'static str>,
) -> Result<VariableDef, CatalogueError> {
    let def = variable_for(t.def.clone(), &format!("{name} {}", t.suffix), eq.id);
    if let Some((first_tag, field)) = declared.get(t.key)
        && column_field_contradiction(&def, field)
    {
        return Err(CatalogueError::DeclarationConflict {
            key: t.key.to_string(),
            first: format!(
                "the static per-capability column '{}' ({}) declares {}",
                def.name,
                def.source.tag(),
                def_summary(&def)
            ),
            second: format!("{first_tag} declares {}", field_summary(field)),
        });
    }
    telemetry_keys.insert(def.name.to_string(), t.key);
    Ok(def)
}

/// Whether a column's declaration and the merged declaration of the
/// telemetry key it reads disagree on any semantic property. Prose (the
/// description) may differ; the unit, kind, aggregation, sampling and
/// nullability may not.
fn column_field_contradiction(def: &VariableDef, field: &TelemetryField) -> bool {
    def.unit != field.unit
        || def.kind != field.kind
        || def.aggregation != field.aggregation
        || def.sampling != field.sampling
        || def.nullable != field.nullable
}

/// Builds a per-zone variable from a zone-template declaration.
fn zone_variable(template: &ZoneTemplate, zone_id: ZoneId, label: &str) -> VariableDef {
    let mut def = template.def.clone();
    def.name = Cow::Owned(format!("{}{label}{}", template.prefix, template.suffix));
    def.source = VariableSource::Zone(zone_id);
    def
}

/// The timestamp column: the row label, prefixed to every schema. Not a
/// catalogue variable (it is the row's index, not an observable; the row
/// plan that replaces per-step column lookup carries it as the i64 local
/// timestamp).
const TIMESTAMP_COL: &str = "Time";

const ELECTRIC_SUFFIX: &str = "Electric Power (kW)";
const GAS_SUFFIX: &str = "Gas Power (therms/hour)";
const MODE_SUFFIX: &str = "Mode (-)";
const SETPOINT_SUFFIX: &str = "Setpoint (C)";
const SOC_SUFFIX: &str = "SOC (-)";
const ENERGY_SUFFIX: &str = "Energy (kWh)";
const REACTIVE_SUFFIX: &str = "Reactive Power (kVAR)";
const POWER_FACTOR_SUFFIX: &str = "Power Factor (-)";
const SCHEDULE_SUFFIX: &str = "Schedule (-)";

/// The static per-capability table: the per-equipment core-output columns
/// every equipment produces from `CoreOutput`/`CoreCapabilities`, as
/// `(suffix, def)` pairs. The per-equipment `Mode (-)` column's labels are
/// `EnumLabels::Static`, so the table compiles as a `static`. The table's
/// order is the family order build_catalogue expands it in.
#[allow(clippy::type_complexity)]
static CORE_OUTPUT_VARIABLES: &[(&str, VariableDef)] = &[
    (
        ELECTRIC_SUFFIX,
        VariableDef {
            name: Cow::Borrowed(ELECTRIC_SUFFIX),
            unit: Unit::Kilowatt,
            kind: VariableKind::Power,
            aggregation: Aggregation::Mean,
            sampling: Sampling::IntervalMean,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("Net electric power (consumption positive)"),
        },
    ),
    (
        GAS_SUFFIX,
        VariableDef {
            name: Cow::Borrowed(GAS_SUFFIX),
            unit: Unit::ThermPerHour,
            kind: VariableKind::Power,
            aggregation: Aggregation::Mean,
            sampling: Sampling::IntervalMean,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed(
                "Fuel input as gas power (combustion fuels; historical name)",
            ),
        },
    ),
    (
        MODE_SUFFIX,
        VariableDef {
            name: Cow::Borrowed(MODE_SUFFIX),
            unit: Unit::Dimensionless,
            kind: VariableKind::Enum {
                labels: hares_types::EnumLabels::Static(hares_types::MODE_LABELS),
            },
            aggregation: Aggregation::Mode,
            sampling: Sampling::InstantAtEnd,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("Operating mode code (the hares_mode_map metadata table)"),
        },
    ),
    (
        SETPOINT_SUFFIX,
        VariableDef {
            name: Cow::Borrowed(SETPOINT_SUFFIX),
            unit: Unit::Celsius,
            kind: VariableKind::Temperature,
            aggregation: Aggregation::Mean,
            sampling: Sampling::InstantAtEnd,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("The active thermostat setpoint"),
        },
    ),
    (
        SOC_SUFFIX,
        VariableDef {
            name: Cow::Borrowed(SOC_SUFFIX),
            unit: Unit::Dimensionless,
            kind: VariableKind::Ratio,
            aggregation: Aggregation::Mean,
            sampling: Sampling::InstantAtEnd,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("State of charge"),
        },
    ),
    (
        ENERGY_SUFFIX,
        VariableDef {
            name: Cow::Borrowed(ENERGY_SUFFIX),
            unit: Unit::KilowattHour,
            kind: VariableKind::Energy,
            aggregation: Aggregation::Sum,
            sampling: Sampling::IntervalMean,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed(
                "The energy of the step (net electric power times the step)",
            ),
        },
    ),
    (
        REACTIVE_SUFFIX,
        VariableDef {
            name: Cow::Borrowed(REACTIVE_SUFFIX),
            unit: Unit::VoltAmpereReactive,
            kind: VariableKind::Power,
            aggregation: Aggregation::Mean,
            sampling: Sampling::IntervalMean,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("Reactive power (positive = inductive/lagging)"),
        },
    ),
    (
        POWER_FACTOR_SUFFIX,
        VariableDef {
            name: Cow::Borrowed(POWER_FACTOR_SUFFIX),
            unit: Unit::Dimensionless,
            kind: VariableKind::Ratio,
            aggregation: Aggregation::Mean,
            sampling: Sampling::InstantAtEnd,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("Power factor (P/S, magnitude)"),
        },
    ),
    (
        SCHEDULE_SUFFIX,
        VariableDef {
            name: Cow::Borrowed(SCHEDULE_SUFFIX),
            unit: Unit::Dimensionless,
            kind: VariableKind::State,
            aggregation: Aggregation::Mean,
            sampling: Sampling::InstantAtEnd,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed(
                "1 when the equipment is scheduled on (mode other than Off)",
            ),
        },
    ),
    (
        "Speed (-)",
        VariableDef {
            name: Cow::Borrowed("Speed (-)"),
            unit: Unit::Dimensionless,
            kind: VariableKind::Enum {
                labels: hares_types::EnumLabels::Static(hares_types::SPEED_INDEX_LABELS),
            },
            aggregation: Aggregation::Mode,
            sampling: Sampling::InstantAtEnd,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("Active compressor speed stage index (0-based)"),
        },
    ),
    (
        "Main Power (kW)",
        VariableDef {
            name: Cow::Borrowed("Main Power (kW)"),
            unit: Unit::Kilowatt,
            kind: VariableKind::Power,
            aggregation: Aggregation::Mean,
            sampling: Sampling::IntervalMean,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("Total input power minus fan power (OCHRE HVAC.py:575)"),
        },
    ),
    (
        "Capacity (W)",
        VariableDef {
            name: Cow::Borrowed("Capacity (W)"),
            unit: Unit::Watt,
            kind: VariableKind::Power,
            aggregation: Aggregation::Mean,
            sampling: Sampling::IntervalMean,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("Delivered capacity magnitude (positive)"),
        },
    ),
    (
        "COP (-)",
        VariableDef {
            name: Cow::Borrowed("COP (-)"),
            unit: Unit::Dimensionless,
            kind: VariableKind::Ratio,
            aggregation: Aggregation::Mean,
            sampling: Sampling::InstantAtEnd,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("Coefficient of performance (AHRI: excludes fan)"),
        },
    ),
];

/// The v7 core-output families after the cooling block: Speed, Main Power,
/// Capacity, COP.
const HVAC_V7_CORE_COLUMNS_START: usize = 9;

/// The end-use aggregate electric power column's declaration.
static END_USE_AGGREGATE_VARIABLE: VariableDef = VariableDef {
    name: Cow::Borrowed(""),
    unit: Unit::Kilowatt,
    kind: VariableKind::Power,
    aggregation: Aggregation::Mean,
    sampling: Sampling::IntervalMean,
    nullable: true,
    source: VariableSource::Dwelling,
    description: Cow::Borrowed("End-use aggregate electric power"),
};

/// A static per-zone-template table entry: the column name is
/// `{prefix}{zone label}{suffix}`.
struct ZoneTemplate {
    prefix: &'static str,
    suffix: &'static str,
    def: VariableDef,
}

macro_rules! zone_template {
    ($prefix:literal, $suffix:literal, $unit:expr, $kind:expr, $agg:expr, $sampling:expr, $desc:literal) => {
        ZoneTemplate {
            prefix: $prefix,
            suffix: $suffix,
            def: VariableDef {
                name: Cow::Borrowed(""),
                unit: $unit,
                kind: $kind,
                aggregation: $agg,
                sampling: $sampling,
                nullable: true,
                source: VariableSource::Dwelling,
                description: Cow::Borrowed($desc),
            },
        }
    };
}

/// The static per-zone-template table, keyed by the zone display name.
static ZONE_TEMP_TEMPLATE: ZoneTemplate = zone_template!(
    "Temperature - ",
    " (C)",
    Unit::Celsius,
    VariableKind::Temperature,
    Aggregation::Mean,
    Sampling::InstantAtEnd,
    "Zone temperature"
);
static INFILTRATION_TEMPLATE: ZoneTemplate = zone_template!(
    "Infiltration Heat Gain - ",
    " (W)",
    Unit::Watt,
    VariableKind::Power,
    Aggregation::Mean,
    Sampling::IntervalMean,
    "Infiltration heat gain in the zone"
);
static LWR_TEMPLATE: ZoneTemplate = zone_template!(
    "Interior LWR Exchange - ",
    " (W)",
    Unit::Watt,
    VariableKind::Power,
    Aggregation::Mean,
    Sampling::IntervalMean,
    "Interior long-wave radiation exchange activity (gross)"
);
static HEATING_DELIVERED_TEMPLATE: ZoneTemplate = zone_template!(
    "HVAC Heating Delivered - ",
    " (W)",
    Unit::Watt,
    VariableKind::Power,
    Aggregation::Mean,
    Sampling::IntervalMean,
    "HVAC heating delivered to the zone"
);
static COOLING_DELIVERED_TEMPLATE: ZoneTemplate = zone_template!(
    "HVAC Cooling Delivered - ",
    " (W)",
    Unit::Watt,
    VariableKind::Power,
    Aggregation::Mean,
    Sampling::IntervalMean,
    "HVAC cooling delivered to the zone"
);

/// A static per-equipment telemetry-derived column: the telemetry key the
/// value is read from at record time, and the column's declaration. The
/// declaration must agree with the merged declaration of that key (an
/// equipment's `TelemetryField`); a disagreement is an assembly error.
struct TelemetryColumn {
    /// The telemetry key the column's value is read from.
    key: &'static str,
    /// The column name suffix after the instance name.
    suffix: &'static str,
    /// The declaration.
    def: VariableDef,
}

macro_rules! telemetry_column {
    ($key:literal, $suffix:literal, $unit:expr, $kind:expr, $agg:expr, $sampling:expr, $desc:literal) => {
        TelemetryColumn {
            key: $key,
            suffix: $suffix,
            def: VariableDef {
                name: Cow::Borrowed($suffix),
                unit: $unit,
                kind: $kind,
                aggregation: $agg,
                sampling: $sampling,
                nullable: true,
                source: VariableSource::Dwelling,
                description: Cow::Borrowed($desc),
            },
        }
    };
}

/// Verbosity 7: heat-pump-heater telemetry columns (defrost, ER power).
static HP_HEATER_V7_COLUMNS: &[TelemetryColumn] = &[
    telemetry_column!(
        "defrost_cycle_state",
        "Defrost State (-)",
        Unit::Dimensionless,
        VariableKind::State,
        Aggregation::Mean,
        Sampling::InstantAtEnd,
        "Defrost cycle state code"
    ),
    telemetry_column!(
        "backup_er_kw",
        "ER Power (kW)",
        Unit::Kilowatt,
        VariableKind::Power,
        Aggregation::Mean,
        Sampling::IntervalMean,
        "Backup electric resistance power during ER-only or HP+ER modes"
    ),
];

/// Verbosity 7: cooling-equipment telemetry columns (SHR, latent gains).
static COOLING_V7_COLUMNS: &[TelemetryColumn] = &[
    telemetry_column!(
        "shr",
        "SHR (-)",
        Unit::Dimensionless,
        VariableKind::Ratio,
        Aggregation::Mean,
        Sampling::InstantAtEnd,
        "Sensible heat ratio (post-degradation)"
    ),
    telemetry_column!(
        "latent_gains_w",
        "Latent Gains (W)",
        Unit::Watt,
        VariableKind::Power,
        Aggregation::Mean,
        Sampling::IntervalMean,
        "Pre-DSE gross latent cooling scaled by space fraction"
    ),
];

/// Verbosity 7: per-equipment HVAC telemetry columns (fan power, runtime
/// fraction); the CoreOutput-read v7 families are in CORE_OUTPUT_VARIABLES.
static HVAC_V7_TELEMETRY_COLUMNS: &[TelemetryColumn] = &[
    telemetry_column!(
        "fan_kw",
        "Fan Power (kW)",
        Unit::Kilowatt,
        VariableKind::Power,
        Aggregation::Mean,
        Sampling::IntervalMean,
        "Fan electric power"
    ),
    telemetry_column!(
        "runtime_fraction",
        "Runtime Fraction (-)",
        Unit::Dimensionless,
        VariableKind::Ratio,
        Aggregation::Mean,
        Sampling::InstantAtEnd,
        "Runtime fraction (PLR/PLF)"
    ),
];

/// Verbosity 8: HVAC telemetry diagnostic columns.
static HVAC_V8_COLUMNS: &[TelemetryColumn] = &[
    telemetry_column!(
        "supply_temp_c",
        "Supply Temperature (C)",
        Unit::Celsius,
        VariableKind::Temperature,
        Aggregation::Mean,
        Sampling::InstantAtEnd,
        "Supply temperature"
    ),
    telemetry_column!(
        "return_temp_c",
        "Return Temperature (C)",
        Unit::Celsius,
        VariableKind::Temperature,
        Aggregation::Mean,
        Sampling::InstantAtEnd,
        "Return temperature"
    ),
    telemetry_column!(
        "compressor_power_w",
        "Compressor Power (W)",
        Unit::Watt,
        VariableKind::Power,
        Aggregation::Mean,
        Sampling::IntervalMean,
        "Compressor electric power"
    ),
    telemetry_column!(
        "compressor_kw",
        "Compressor Power (kW)",
        Unit::Kilowatt,
        VariableKind::Power,
        Aggregation::Mean,
        Sampling::IntervalMean,
        "Compressor electric power (space-fraction scaled)"
    ),
    telemetry_column!(
        "fan_electric_w",
        "Fan Electric Power (W)",
        Unit::Watt,
        VariableKind::Power,
        Aggregation::Mean,
        Sampling::IntervalMean,
        "Fan electric power (grid-side)"
    ),
    telemetry_column!(
        "fan_power_w",
        "Fan Power (W)",
        Unit::Watt,
        VariableKind::Power,
        Aggregation::Mean,
        Sampling::IntervalMean,
        "Fan power (air-side)"
    ),
    telemetry_column!(
        "min_on_time_s",
        "Min On Time (s)",
        Unit::Second,
        VariableKind::Quantity {
            across_homes: AcrossHomes::Mean,
        },
        Aggregation::Mean,
        Sampling::InstantAtEnd,
        "Minimum compressor on-time from the thermostat FSM"
    ),
    telemetry_column!(
        "min_off_time_s",
        "Min Off Time (s)",
        Unit::Second,
        VariableKind::Quantity {
            across_homes: AcrossHomes::Mean,
        },
        Aggregation::Mean,
        Sampling::InstantAtEnd,
        "Minimum compressor off-time from the thermostat FSM"
    ),
];

/// Verbosity 8: heat-pump-heater telemetry diagnostic columns.
static HP_HEATER_V8_COLUMNS: &[TelemetryColumn] = &[
    telemetry_column!(
        "supply_air_temp_c",
        "Supply Air Temperature (C)",
        Unit::Celsius,
        VariableKind::Temperature,
        Aggregation::Mean,
        Sampling::InstantAtEnd,
        "Supply air temperature"
    ),
    telemetry_column!(
        "pan_heater_kw",
        "Pan Heater Power (kW)",
        Unit::Kilowatt,
        VariableKind::Power,
        Aggregation::Mean,
        Sampling::IntervalMean,
        "Pan heater electric power"
    ),
    telemetry_column!(
        "hp_capacity_w",
        "Heat Pump Capacity (W)",
        Unit::Watt,
        VariableKind::Power,
        Aggregation::Mean,
        Sampling::IntervalMean,
        "Heat pump capacity delivered"
    ),
    telemetry_column!(
        "er_capacity_w",
        "ER Capacity (W)",
        Unit::Watt,
        VariableKind::Power,
        Aggregation::Mean,
        Sampling::IntervalMean,
        "Electric resistance capacity delivered"
    ),
];

/// Verbosity 8: PV telemetry diagnostic columns.
static PV_V8_COLUMNS: &[TelemetryColumn] = &[
    telemetry_column!(
        "dc_power_kw",
        "DC Power (kW)",
        Unit::Kilowatt,
        VariableKind::Power,
        Aggregation::Mean,
        Sampling::IntervalMean,
        "Total PV DC output power before inverter efficiency and curtailment"
    ),
    telemetry_column!(
        "irradiance_w_m2",
        "Irradiance (W/m2)",
        Unit::WattPerSquareMeter,
        VariableKind::Quantity {
            across_homes: AcrossHomes::Mean,
        },
        Aggregation::Mean,
        Sampling::IntervalMean,
        "Capacity-weighted average incident irradiance"
    ),
];

/// Verbosity 8: EV telemetry diagnostic columns. The unit is the equipment
/// declarations' canonical symbol (`ev/telemetry.rs` declares both channels
/// `code_code`): the declaration is the authority and this table mirrors
/// it. The fleet's unit-keyed rules resolve `code` through their fallback
/// (Mean over time, WeightedSum across homes) until they fold by kind.
static EV_V8_COLUMNS: &[TelemetryColumn] = &[
    telemetry_column!(
        "connection_state",
        "Connection State (-)",
        Unit::Code,
        VariableKind::Enum {
            labels: hares_types::EnumLabels::Static(hares_types::EV_CONNECTION_STATE_LABELS),
        },
        Aggregation::Mode,
        Sampling::InstantAtEnd,
        "Connection state (0=home plugged in, 1=away plugged in, 2=disconnected)"
    ),
    telemetry_column!(
        "charging_level",
        "Charging Level (-)",
        Unit::Code,
        VariableKind::Enum {
            labels: hares_types::EnumLabels::Static(hares_types::EV_CHARGING_LEVEL_LABELS),
        },
        Aggregation::Mode,
        Sampling::InstantAtEnd,
        "Charging level (1=L1, 2=L2)"
    ),
];

/// The dwelling-level static table as `(name, def, min_verbosity,
/// observe_only)` triples, in block order within their verbosity level.
#[allow(clippy::type_complexity)]
static DWELLING_VARIABLES: &[(&str, VariableDef, u8, bool)] = &[
    (
        "Total Electric Power (kW)",
        VariableDef {
            name: Cow::Borrowed("Total Electric Power (kW)"),
            unit: Unit::Kilowatt,
            kind: VariableKind::Power,
            aggregation: Aggregation::Mean,
            sampling: Sampling::IntervalMean,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("Net electric power for the whole dwelling"),
        },
        0,
        false,
    ),
    (
        "Total Gas Power (therms/hour)",
        VariableDef {
            name: Cow::Borrowed("Total Gas Power (therms/hour)"),
            unit: Unit::ThermPerHour,
            kind: VariableKind::Power,
            aggregation: Aggregation::Mean,
            sampling: Sampling::IntervalMean,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("Gas power for the whole dwelling"),
        },
        0,
        false,
    ),
    (
        "Total Reactive Power (kVAR)",
        VariableDef {
            name: Cow::Borrowed("Total Reactive Power (kVAR)"),
            unit: Unit::VoltAmpereReactive,
            kind: VariableKind::Power,
            aggregation: Aggregation::Mean,
            sampling: Sampling::IntervalMean,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("Net reactive power for the whole dwelling"),
        },
        0,
        false,
    ),
    (
        "Outdoor Dry Bulb (C)",
        VariableDef {
            name: Cow::Borrowed("Outdoor Dry Bulb (C)"),
            unit: Unit::Celsius,
            kind: VariableKind::Temperature,
            aggregation: Aggregation::Mean,
            sampling: Sampling::InstantAtEnd,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("Outdoor dry-bulb temperature"),
        },
        0,
        false,
    ),
    (
        "Temperature - Indoor (C)",
        VariableDef {
            name: Cow::Borrowed("Temperature - Indoor (C)"),
            unit: Unit::Celsius,
            kind: VariableKind::Temperature,
            aggregation: Aggregation::Mean,
            sampling: Sampling::InstantAtEnd,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("Indoor zone temperature"),
        },
        0,
        false,
    ),
    (
        "Unmet HVAC Load (C)",
        VariableDef {
            name: Cow::Borrowed("Unmet HVAC Load (C)"),
            unit: Unit::Celsius,
            kind: VariableKind::Temperature,
            aggregation: Aggregation::Mean,
            sampling: Sampling::InstantAtEnd,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("Unmet HVAC load, expressed as the temperature gap"),
        },
        2,
        false,
    ),
    (
        "Temperature - Ground (C)",
        VariableDef {
            name: Cow::Borrowed("Temperature - Ground (C)"),
            unit: Unit::Celsius,
            kind: VariableKind::Temperature,
            aggregation: Aggregation::Mean,
            sampling: Sampling::InstantAtEnd,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("Ground temperature"),
        },
        2,
        false,
    ),
    (
        "HVAC Heating Delivered (W)",
        VariableDef {
            name: Cow::Borrowed("HVAC Heating Delivered (W)"),
            unit: Unit::Watt,
            kind: VariableKind::Power,
            aggregation: Aggregation::Mean,
            sampling: Sampling::IntervalMean,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("HVAC heating delivered to the indoor zone"),
        },
        4,
        false,
    ),
    (
        "HVAC Cooling Delivered (W)",
        VariableDef {
            name: Cow::Borrowed("HVAC Cooling Delivered (W)"),
            unit: Unit::Watt,
            kind: VariableKind::Power,
            aggregation: Aggregation::Mean,
            sampling: Sampling::IntervalMean,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("HVAC cooling delivered to the indoor zone"),
        },
        4,
        false,
    ),
    (
        "Net Sensible Heat Gain - Indoor (W)",
        VariableDef {
            name: Cow::Borrowed("Net Sensible Heat Gain - Indoor (W)"),
            unit: Unit::Watt,
            kind: VariableKind::Power,
            aggregation: Aggregation::Mean,
            sampling: Sampling::IntervalMean,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("Direct heat injections on the indoor zone air"),
        },
        5,
        false,
    ),
    (
        "HVAC Duct Losses (W)",
        VariableDef {
            name: Cow::Borrowed("HVAC Duct Losses (W)"),
            unit: Unit::Watt,
            kind: VariableKind::Power,
            aggregation: Aggregation::Mean,
            sampling: Sampling::IntervalMean,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed(
                "Duct losses summed across every HVAC equipment (ASHRAE 152)",
            ),
        },
        5,
        false,
    ),
    (
        "Window Transmitted Solar Gain (W)",
        VariableDef {
            name: Cow::Borrowed("Window Transmitted Solar Gain (W)"),
            unit: Unit::Watt,
            kind: VariableKind::Power,
            aggregation: Aggregation::Mean,
            sampling: Sampling::IntervalMean,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("Through-glass transmitted solar gain (Envelope.py:1160)"),
        },
        6,
        false,
    ),
    (
        "Infiltration Heat Gain - Indoor (W)",
        VariableDef {
            name: Cow::Borrowed("Infiltration Heat Gain - Indoor (W)"),
            unit: Unit::Watt,
            kind: VariableKind::Power,
            aggregation: Aggregation::Mean,
            sampling: Sampling::IntervalMean,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("Infiltration heat gain in the indoor zone"),
        },
        6,
        false,
    ),
    (
        "Forced Ventilation Heat Gain - Indoor (W)",
        VariableDef {
            name: Cow::Borrowed("Forced Ventilation Heat Gain - Indoor (W)"),
            unit: Unit::Watt,
            kind: VariableKind::Power,
            aggregation: Aggregation::Mean,
            sampling: Sampling::IntervalMean,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("Forced ventilation heat gain in the indoor zone"),
        },
        6,
        false,
    ),
    (
        "Natural Ventilation Heat Gain - Indoor (W)",
        VariableDef {
            name: Cow::Borrowed("Natural Ventilation Heat Gain - Indoor (W)"),
            unit: Unit::Watt,
            kind: VariableKind::Power,
            aggregation: Aggregation::Mean,
            sampling: Sampling::IntervalMean,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("Natural ventilation heat gain in the indoor zone"),
        },
        6,
        false,
    ),
    (
        "Natural Ventilation Cw (-)",
        VariableDef {
            name: Cow::Borrowed("Natural Ventilation Cw (-)"),
            unit: Unit::Dimensionless,
            kind: VariableKind::Quantity {
                across_homes: AcrossHomes::Mean,
            },
            aggregation: Aggregation::Mean,
            sampling: Sampling::InstantAtEnd,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed(
                "Wind-direction coefficient used by the natural ventilation model",
            ),
        },
        6,
        true,
    ),
    (
        "Natural Ventilation Wind Angle (deg)",
        VariableDef {
            name: Cow::Borrowed("Natural Ventilation Wind Angle (deg)"),
            unit: Unit::Degree,
            kind: VariableKind::Quantity {
                across_homes: AcrossHomes::Mean,
            },
            aggregation: Aggregation::Mean,
            sampling: Sampling::InstantAtEnd,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("Wind angle the natural ventilation step used"),
        },
        6,
        true,
    ),
    (
        "Internal Heat Gain - Indoor (W)",
        VariableDef {
            name: Cow::Borrowed("Internal Heat Gain - Indoor (W)"),
            unit: Unit::Watt,
            kind: VariableKind::Power,
            aggregation: Aggregation::Mean,
            sampling: Sampling::IntervalMean,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("Internal heat gain in the indoor zone"),
        },
        6,
        false,
    ),
    (
        "Interior LWR Exchange - Indoor (W)",
        VariableDef {
            name: Cow::Borrowed("Interior LWR Exchange - Indoor (W)"),
            unit: Unit::Watt,
            kind: VariableKind::Power,
            aggregation: Aggregation::Mean,
            sampling: Sampling::IntervalMean,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("Interior long-wave radiation exchange activity (gross)"),
        },
        6,
        false,
    ),
    (
        "Opaque Surface Heat Gain - Indoor (W)",
        VariableDef {
            name: Cow::Borrowed("Opaque Surface Heat Gain - Indoor (W)"),
            unit: Unit::Watt,
            kind: VariableKind::Power,
            aggregation: Aggregation::Mean,
            sampling: Sampling::IntervalMean,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed(
                "Net conduction from opaque envelope surfaces into the indoor zone",
            ),
        },
        6,
        false,
    ),
    (
        "Duct Loss Heat Gain - Indoor (W)",
        VariableDef {
            name: Cow::Borrowed("Duct Loss Heat Gain - Indoor (W)"),
            unit: Unit::Watt,
            kind: VariableKind::Power,
            aggregation: Aggregation::Mean,
            sampling: Sampling::IntervalMean,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("Duct loss heat gain in the indoor zone"),
        },
        6,
        false,
    ),
    (
        "Roof Heat Gain - Indoor (W)",
        VariableDef {
            name: Cow::Borrowed("Roof Heat Gain - Indoor (W)"),
            unit: Unit::Watt,
            kind: VariableKind::Power,
            aggregation: Aggregation::Mean,
            sampling: Sampling::IntervalMean,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("Roof heat gain in the indoor zone"),
        },
        6,
        false,
    ),
    (
        "Floor Heat Gain - Indoor (W)",
        VariableDef {
            name: Cow::Borrowed("Floor Heat Gain - Indoor (W)"),
            unit: Unit::Watt,
            kind: VariableKind::Power,
            aggregation: Aggregation::Mean,
            sampling: Sampling::IntervalMean,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("Floor heat gain in the indoor zone"),
        },
        6,
        false,
    ),
    (
        "Wall Heat Gain - Indoor (W)",
        VariableDef {
            name: Cow::Borrowed("Wall Heat Gain - Indoor (W)"),
            unit: Unit::Watt,
            kind: VariableKind::Power,
            aggregation: Aggregation::Mean,
            sampling: Sampling::IntervalMean,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("Wall heat gain in the indoor zone"),
        },
        6,
        false,
    ),
    (
        "Window Heat Gain - Indoor (W)",
        VariableDef {
            name: Cow::Borrowed("Window Heat Gain - Indoor (W)"),
            unit: Unit::Watt,
            kind: VariableKind::Power,
            aggregation: Aggregation::Mean,
            sampling: Sampling::IntervalMean,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("Window heat gain in the indoor zone"),
        },
        6,
        false,
    ),
    (
        "Internal Mass Heat Gain - Indoor (W)",
        VariableDef {
            name: Cow::Borrowed("Internal Mass Heat Gain - Indoor (W)"),
            unit: Unit::Watt,
            kind: VariableKind::Power,
            aggregation: Aggregation::Mean,
            sampling: Sampling::IntervalMean,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("Internal mass heat gain in the indoor zone"),
        },
        6,
        false,
    ),
    (
        "Zone Air Heat Balance Residual (W)",
        VariableDef {
            name: Cow::Borrowed("Zone Air Heat Balance Residual (W)"),
            unit: Unit::Watt,
            kind: VariableKind::Power,
            aggregation: Aggregation::Mean,
            sampling: Sampling::IntervalMean,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed(
                "Zone air heat-balance residual (the E+ Output:Diagnostics analogue)",
            ),
        },
        6,
        false,
    ),
    (
        "Hot Water Mains Temperature (C)",
        VariableDef {
            name: Cow::Borrowed("Hot Water Mains Temperature (C)"),
            unit: Unit::Celsius,
            kind: VariableKind::Temperature,
            aggregation: Aggregation::Mean,
            sampling: Sampling::InstantAtEnd,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("Hot water mains temperature"),
        },
        7,
        false,
    ),
    (
        "Scheduled Heating Setpoint (C)",
        VariableDef {
            name: Cow::Borrowed("Scheduled Heating Setpoint (C)"),
            unit: Unit::Celsius,
            kind: VariableKind::Temperature,
            aggregation: Aggregation::Mean,
            sampling: Sampling::InstantAtEnd,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("Schedule-stage heating setpoint, before runtime override"),
        },
        7,
        false,
    ),
    (
        "Scheduled Cooling Setpoint (C)",
        VariableDef {
            name: Cow::Borrowed("Scheduled Cooling Setpoint (C)"),
            unit: Unit::Celsius,
            kind: VariableKind::Temperature,
            aggregation: Aggregation::Mean,
            sampling: Sampling::InstantAtEnd,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("Schedule-stage cooling setpoint, before runtime override"),
        },
        7,
        false,
    ),
    (
        "Runtime Heating Setpoint (C)",
        VariableDef {
            name: Cow::Borrowed("Runtime Heating Setpoint (C)"),
            unit: Unit::Celsius,
            kind: VariableKind::Temperature,
            aggregation: Aggregation::Mean,
            sampling: Sampling::InstantAtEnd,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("Runtime-override heating setpoint"),
        },
        7,
        false,
    ),
    (
        "Runtime Cooling Setpoint (C)",
        VariableDef {
            name: Cow::Borrowed("Runtime Cooling Setpoint (C)"),
            unit: Unit::Celsius,
            kind: VariableKind::Temperature,
            aggregation: Aggregation::Mean,
            sampling: Sampling::InstantAtEnd,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("Runtime-override cooling setpoint"),
        },
        7,
        false,
    ),
];

/// Builds the Arrow output schema from the catalogue at `verbosity`: the
/// `Time` label, then every catalogue variable emitted at the verbosity,
/// preserving catalogue order. Field metadata carries `Unit::symbol()` as
/// the `unit` entry (no metadata for [`Unit::None`]); the schema metadata
/// is the pre-catalogue one (verbosity, mode map, lighting-created flags).
#[must_use]
pub fn build_schema(catalogue: &Catalogue, verbosity: u8) -> Schema {
    let mut fields = vec![Field::new(TIMESTAMP_COL, DataType::Utf8, false)];

    for var in catalogue.variables_at_verbosity(verbosity) {
        fields.push(variable_field(var).as_ref().clone());
    }

    Schema::new_with_metadata(fields, schema_metadata(catalogue, verbosity))
}

/// One variable's Arrow field: the column name, `Float64`, the
/// declaration's nullability, and the unit metadata (no metadata for
/// [`Unit::None`]).
#[must_use]
pub fn variable_field(var: &VariableDef) -> arrow::datatypes::FieldRef {
    let mut field = Field::new(var.name.as_ref(), DataType::Float64, var.nullable);
    if var.unit != Unit::None {
        let mut metadata = std::collections::HashMap::new();
        metadata.insert("unit".to_string(), var.unit.symbol().to_string());
        field = field.with_metadata(metadata);
    }
    std::sync::Arc::new(field)
}

/// The schema metadata every schema builder carries: the mode-ordinal map,
/// the lighting-created flags, and the verbosity the schema spans (for a
/// named column set, the deepest tier the columns are drawn from).
fn schema_metadata(
    catalogue: &Catalogue,
    verbosity: u8,
) -> std::collections::HashMap<String, String> {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("hares_verbosity".to_string(), verbosity.to_string());
    metadata.insert("hares_mode_map".to_string(), mode_ordinal_json());

    let basement_lighting_created = catalogue
        .equipment_names()
        .iter()
        .any(|n| n == "Basement Lighting");
    metadata.insert(
        "basement_lighting_created".to_string(),
        basement_lighting_created.to_string(),
    );
    let garage_lighting_created = catalogue
        .equipment_names()
        .iter()
        .any(|n| n == "Garage Lighting");
    metadata.insert(
        "garage_lighting_created".to_string(),
        garage_lighting_created.to_string(),
    );
    metadata
}

/// Builds the Arrow output schema from an explicit column set: the `Time`
/// label, then every named catalogue variable in the given order. The
/// schema of a `Named` or `Pattern` report's selection; unknown names are
/// skipped (the resolver never hands one over), and so are columns the
/// active feature set cannot emit (the observe gate, as
/// [`build_schema`] honours it).
#[must_use]
pub fn build_schema_for_columns(catalogue: &Catalogue, names: &[String]) -> Schema {
    let mut fields = vec![Field::new(TIMESTAMP_COL, DataType::Utf8, false)];
    let mut max_verbosity = 0u8;
    for name in names {
        let Some(var) = catalogue.resolve(name) else {
            continue;
        };
        let Some((min_v, observe_only)) = catalogue.tag(name) else {
            continue;
        };
        if observe_only && !cfg!(feature = "observe") {
            continue;
        }
        max_verbosity = max_verbosity.max(min_v);
        fields.push(variable_field(var).as_ref().clone());
    }
    Schema::new_with_metadata(fields, schema_metadata(catalogue, max_verbosity))
}

/// Builds a catalogue from spec-shaped inputs (no live descriptors) and
/// derives the schema from it: the compatibility path the spec-based
/// callers and tests use. Equipment ids are positional.
#[must_use]
pub fn build_schema_for_specs(
    equipment_list: &[EquipmentSpec],
    verbosity: u8,
    zone_names: &[(ZoneId, String)],
) -> Schema {
    let input = CatalogueInput {
        equipment: equipment_list
            .iter()
            .enumerate()
            .map(|(i, spec)| CatalogueEquipment {
                name: spec.name.clone(),
                instance_name: spec.instance_name.clone(),
                id: EquipmentId(u32::try_from(i + 1).expect("equipment count")),
                fuel: spec.fuel_type,
                telemetry: Vec::new(),
            })
            .collect(),
        zones: zone_names.to_vec(),
        actors: Vec::new(),
    };
    match build_catalogue(&input) {
        Ok(catalogue) => build_schema(&catalogue, verbosity),
        // The spec path cannot produce duplicates the live path could not
        // (same classifiers, same names); a panic here is a bug in the
        // static tables, not an input error.
        Err(err) => panic!("catalogue build failed from specs: {err}"),
    }
}

/// JSON encoding of the `OperatingMode` → ordinal mapping, embedded as
/// Parquet custom metadata so output files are self-describing. Derived
/// from the mode labels' order (label `i` is ordinal `i`), so a mode
/// variant without a label fails the catalogue's mode-labels test instead
/// of shipping an unmapped mode.
fn mode_ordinal_json() -> String {
    let pairs: Vec<(&str, u8)> = hares_types::MODE_LABELS
        .iter()
        .enumerate()
        .map(|(i, label)| (*label, i as u8))
        .collect();
    serde_json::to_string(&pairs).expect("static map serialises")
}
