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
//! The per-equipment column set is decided by the typed declarations:
//! the descriptors' `end_use` (the rollup and the column families) and
//! the declared telemetry and capabilities, through
//! [`EquipmentColumnFamilies`]. No name classifier runs anywhere in the
//! builder.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use serde::{Deserialize, Serialize};

use hares_types::{
    AcrossHomes, Aggregation, CoreCapabilities, EndUse, EquipmentId, FuelType, Sampling,
    TelemetryField, Unit, VariableKind, ZoneId,
};

use crate::hpxml::EquipmentSpec;
use crate::hpxml::equipment::canonical_instance_namer;
use crate::output::columns::{
    end_use_electric_power_column, fuel_reports_gas_power_column, is_reserved_output_column_name,
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
    /// A catalogue built from no variables at all.
    #[error("a catalogue must carry variables: the definitions list is empty")]
    NoVariables,
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

/// The per-equipment column families, decided once per instance from the
/// typed declarations: the descriptor's end use, its core-output
/// capabilities and its declared telemetry, never the equipment's name
/// string. Carries the families the catalogue builder and the dwelling's
/// row plan both read, so the schema's emission and the record path's
/// resolution cannot drift.
#[derive(Debug, Clone)]
pub struct EquipmentColumnFamilies {
    /// Whether the equipment burns a fuel that reports a gas power column.
    pub has_gas: bool,
    /// Whether the typed end use carries the HVAC-or-water-heating column
    /// family (setpoint, capacity, COP, speed, fan, main power, runtime
    /// fraction, the HVAC telemetry blocks). A pool or spa heater's end
    /// use is not in the family; an `Ideal HVAC`'s is.
    pub is_hvac: bool,
    /// Whether the typed end use carries the cooling column family (SHR,
    /// latent gains).
    pub is_cooling: bool,
    /// Whether the equipment declares the heat-pump-heater family's
    /// defining channel (the pan heater power) within the HVAC heating
    /// end use.
    pub is_hp_heater: bool,
    /// Whether the equipment declares a state of charge.
    pub has_soc: bool,
    /// Whether the typed end use is PV.
    pub is_pv: bool,
    /// Whether the typed end use is EV.
    pub is_ev: bool,
    /// The typed end use the rollup groups by.
    pub end_use: EndUse,
}

/// The telemetry key whose declaration defines the heat-pump-heater
/// column family: every heat pump heater declares the pan heater power,
/// and no other heating equipment does.
const HP_HEATER_FAMILY_KEY: &str = "pan_heater_kw";

impl EquipmentColumnFamilies {
    /// The families of one typed descriptor: the end use, capabilities
    /// and declared telemetry the equipment declares.
    #[must_use]
    pub fn of_descriptor(descriptor: &hares_types::EquipmentDescriptor) -> Self {
        Self::of_input(
            &descriptor.end_use,
            descriptor.fuel,
            &descriptor.telemetry_fields,
            descriptor.core_capabilities,
        )
    }

    /// The families of one catalogue input entry.
    #[must_use]
    pub fn of_input(
        end_use: &EndUse,
        fuel: FuelType,
        telemetry: &[TelemetryField],
        core_capabilities: hares_types::CoreCapabilities,
    ) -> Self {
        Self {
            has_gas: fuel_reports_gas_power_column(fuel),
            is_hvac: end_use.is_hvac_or_water_heating(),
            is_cooling: end_use.is_cooling_family(),
            is_hp_heater: *end_use == EndUse::HVAC_HEATING
                && telemetry
                    .iter()
                    .any(|field| field.name.as_ref() == HP_HEATER_FAMILY_KEY),
            has_soc: core_capabilities.contains(hares_types::CoreCapabilities::HAS_SOC),
            is_pv: *end_use == EndUse::PV,
            is_ev: *end_use == EndUse::EV,
            end_use: end_use.clone(),
        }
    }
}

/// One equipment's catalogue input: its base name (the descriptor's name,
/// numbered into an instance name when it repeats), its typed end use and
/// capabilities, fuel, id and typed telemetry declarations.
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
    /// The equipment's typed end use: what the rollup groups by and what
    /// decides the column families. The descriptor's own `end_use`.
    pub end_use: EndUse,
    /// The equipment's declared core-output capabilities (the SOC
    /// column's family gate).
    pub core_capabilities: CoreCapabilities,
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
    /// The schema-derivation tags per def.
    tags: Vec<VarTag>,
    index: HashMap<String, usize>,
    /// Column name → the telemetry key the column's value is read from at
    /// record time, for the per-equipment telemetry-derived columns only.
    telemetry_keys: HashMap<String, &'static str>,
    /// The instance-qualified equipment names the catalogue was built from,
    /// in order; the schema metadata derivation (the lighting-created
    /// flags) reads them.
    equipment_names: Vec<String>,
    /// The instance-qualified equipment names paired with their typed end
    /// uses, in the same order. The metrics' end-use equipment gate and
    /// per-end-use equipment counts read this, so no column name is ever
    /// classified by its own string.
    equipment_end_uses: Vec<(String, EndUse)>,
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

    /// The instance-qualified equipment names paired with their typed end
    /// uses, in roster order. The metrics' end-use equipment gate and
    /// per-end-use counts read this: the classification is the
    /// declaration's, never the column name's.
    #[must_use]
    pub fn equipment_end_uses(&self) -> &[(String, EndUse)] {
        &self.equipment_end_uses
    }

    /// Builds a catalogue from explicit variable definitions, in order:
    /// the shape `to_json` round trips. The schema-derivation tags default
    /// (every variable at minimum verbosity 0, none observe-only, none
    /// diagnostics) and no column reads a telemetry key: the definitions
    /// carry the catalogue's variables alone. Every definition is
    /// validated (`AggregationMismatch` for an aggregation that
    /// contradicts its kind) and an empty definitions list is refused (a
    /// catalogue must carry variables). A duplicate name is an error
    /// naming both sources.
    pub fn from_defs(defs: Vec<VariableDef>) -> Result<Self, CatalogueError> {
        if defs.is_empty() {
            return Err(CatalogueError::NoVariables);
        }
        for def in &defs {
            def.validate()?;
        }
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
        let tags = vec![
            VarTag {
                min_verbosity: 0,
                observe_only: false,
                diagnostics: false,
            };
            defs.len()
        ];
        Ok(Self {
            defs,
            tags,
            index,
            telemetry_keys: HashMap::new(),
            equipment_names: Vec::new(),
            equipment_end_uses: Vec::new(),
        })
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
    /// `observe` feature gate. The diagnostics variables are not a
    /// verbosity tier: they are selected by the diagnostics request alone.
    #[must_use]
    pub fn variables_at_verbosity(&self, verbosity: u8) -> Vec<&VariableDef> {
        let mut selected = Vec::new();
        for i in 0..self.defs.len() {
            let tag = &self.tags[i];
            if tag.diagnostics {
                continue;
            }
            if tag.min_verbosity <= verbosity && (!tag.observe_only || cfg!(feature = "observe")) {
                selected.push(&self.defs[i]);
            }
        }
        selected
    }

    /// The variable's `(min verbosity, observe-only)` tag, resolved by
    /// column name.
    #[must_use]
    pub fn tag(&self, name: &str) -> Option<(u8, bool)> {
        self.index
            .get(name)
            .map(|&i| (self.tags[i].min_verbosity, self.tags[i].observe_only))
    }

    /// The diagnostics variables, in schema order (the block appends last,
    /// after the actor columns).
    #[must_use]
    pub fn diagnostics_variables(&self) -> &[VariableDef] {
        let start = self.diagnostics_offset().unwrap_or(self.defs.len());
        &self.defs[start..]
    }

    /// The catalogue index of the first diagnostics variable, or `None`
    /// when the model declares none. The diagnostics block is a contiguous
    /// suffix of the schema's variable list.
    #[must_use]
    pub fn diagnostics_offset(&self) -> Option<usize> {
        self.tags.iter().position(|tag| tag.diagnostics)
    }

    /// The catalogue's digest: the hex SHA-256 of the catalogue's
    /// canonical encoding.
    ///
    /// The canonical encoding is the ASCII header `hares-catalogue-v1`
    /// followed by a newline, then one record per variable in schema
    /// order. A record is the variable's fields, each preceded by the NUL
    /// byte, terminated with a newline: the column name; the kind
    /// (`energy`, `power`, `temperature`, `ratio`, `state`, `enum:` with
    /// the labels joined by the ASCII unit separator, or `quantity:` with
    /// the across-homes rule); the over-time aggregation (`sum`, `mean`,
    /// `mode`, `last`); the sampling rule (`interval-mean`,
    /// `instant-at-start`, `instant-at-end`); the unit's symbol; and the
    /// nullability (`0` never null, `1` nullable). Every semantic field
    /// the catalogue declares is covered; the prose `description` and the
    /// `source` tag are not, so a digest names the variables a schema
    /// reports, not the sites that declared them.
    #[must_use]
    pub fn digest(&self) -> String {
        use sha2::Digest as _;

        const KIND_SEPARATOR: char = '\u{1f}';
        let mut bytes = Vec::with_capacity(64 + self.defs.len() * 48);
        bytes.extend_from_slice(b"hares-catalogue-v1\n");
        for var in &self.defs {
            bytes.push(0);
            bytes.extend_from_slice(var.name.as_bytes());
            bytes.push(0);
            match &var.kind {
                VariableKind::Energy => bytes.extend_from_slice(b"energy"),
                VariableKind::Power => bytes.extend_from_slice(b"power"),
                VariableKind::Temperature => bytes.extend_from_slice(b"temperature"),
                VariableKind::Ratio => bytes.extend_from_slice(b"ratio"),
                VariableKind::State => bytes.extend_from_slice(b"state"),
                VariableKind::Enum { labels } => {
                    bytes.extend_from_slice(b"enum:");
                    for i in 0..labels.len() {
                        if i > 0 {
                            bytes.extend_from_slice(KIND_SEPARATOR.to_string().as_bytes());
                        }
                        bytes.extend_from_slice(labels.get(i).unwrap_or_default().as_bytes());
                    }
                }
                VariableKind::Quantity { across_homes } => match across_homes {
                    AcrossHomes::Sum => bytes.extend_from_slice(b"quantity:sum"),
                    AcrossHomes::Mean => bytes.extend_from_slice(b"quantity:mean"),
                },
            }
            bytes.push(0);
            bytes.extend_from_slice(
                match var.aggregation {
                    Aggregation::Sum => "sum",
                    Aggregation::Mean => "mean",
                    Aggregation::Mode => "mode",
                    Aggregation::Last => "last",
                }
                .as_bytes(),
            );
            bytes.push(0);
            bytes.extend_from_slice(
                match var.sampling {
                    Sampling::IntervalMean => "interval-mean",
                    Sampling::InstantAtStart => "instant-at-start",
                    Sampling::InstantAtEnd => "instant-at-end",
                }
                .as_bytes(),
            );
            bytes.push(0);
            bytes.extend_from_slice(var.unit.symbol().as_bytes());
            bytes.push(0);
            bytes.push(if var.nullable { b'1' } else { b'0' });
            bytes.push(b'\n');
        }
        let mut hasher = sha2::Sha256::default();
        hasher.update(&bytes);
        hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
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
    let mut tags: Vec<VarTag> = Vec::new();
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
        let families = EquipmentColumnFamilies::of_input(
            &eq.end_use,
            eq.fuel,
            &eq.telemetry,
            eq.core_capabilities,
        );
        let member = format!("{name} {ELECTRIC_SUFFIX}");
        if is_reserved_output_column_name(&member) {
            continue;
        }
        push_traced(
            &mut defs,
            &mut tags,
            variable_for(CORE_OUTPUT_VARIABLES[0].1.clone(), &member, eq.id),
            1,
            false,
        )?;
        if families.has_gas {
            push_traced(
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

    // Per-end-use aggregates: one column per typed end use that has at
    // least one equipment, keyed on the declared end use, in equipment
    // order. The rollup groups by the descriptor's end use, never the
    // name string.
    let mut seen_end_uses: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for eq in &input.equipment {
        let end_use = eq.end_use.clone();
        tracing::debug!(
            equipment = %eq.name,
            end_use = %end_use.as_str(),
            "equipment end use at catalogue build"
        );
        if seen_end_uses.insert(end_use.display_name().to_string()) {
            let name = end_use_electric_power_column(&end_use);
            push_traced(
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
            push_traced(
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
        let families = EquipmentColumnFamilies::of_input(
            &eq.end_use,
            eq.fuel,
            &eq.telemetry,
            eq.core_capabilities,
        );
        let member = format!("{name} {ELECTRIC_SUFFIX}");
        if is_reserved_output_column_name(&member) {
            continue;
        }
        push_traced(
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
            push_traced(
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
            push_traced(
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
        push_traced(
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
        push_traced(
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
        push_traced(
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
            push_traced(
                &mut defs,
                &mut tags,
                zone_variable(&INFILTRATION_TEMPLATE, *zone_id, label),
                6,
                false,
            )?;
            push_traced(
                &mut defs,
                &mut tags,
                zone_variable(&LWR_TEMPLATE, *zone_id, label),
                6,
                false,
            )?;
        }
    }
    for (zone_id, label) in &input.zones {
        push_traced(
            &mut defs,
            &mut tags,
            zone_variable(&HEATING_DELIVERED_TEMPLATE, *zone_id, label),
            6,
            false,
        )?;
        push_traced(
            &mut defs,
            &mut tags,
            zone_variable(&COOLING_DELIVERED_TEMPLATE, *zone_id, label),
            6,
            false,
        )?;
    }

    // ── Block: verbosity 7 — schedule, defrost/ER, HVAC performance ────────
    for (eq, name) in &instances {
        let families = EquipmentColumnFamilies::of_input(
            &eq.end_use,
            eq.fuel,
            &eq.telemetry,
            eq.core_capabilities,
        );
        let member = format!("{name} {ELECTRIC_SUFFIX}");
        if is_reserved_output_column_name(&member) {
            continue;
        }
        push_traced(
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
                push_traced(
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
                    push_traced(
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
        let families = EquipmentColumnFamilies::of_input(
            &eq.end_use,
            eq.fuel,
            &eq.telemetry,
            eq.core_capabilities,
        );
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
                push_traced(
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
            push_traced(
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

    // ── The engine's per-step weather channels: in no verbosity preset ─────
    // In the catalogue (a named selection resolves them) with a minimum
    // verbosity no preset reaches, so no preset's schema carries them.
    push_statics(&mut defs, &mut tags, 9)?;

    // ── Diagnostics block: the per-step diagnostics, appended last ────────
    // Never emitted by a verbosity preset; the diagnostics request selects
    // the whole block. The order is the pre-catalogue CSV writer's column
    // order.
    push_diagnostics(&mut defs, &mut tags, DIAGNOSTICS_STEP.clone())?;
    push_diagnostics(&mut defs, &mut tags, DIAGNOSTICS_TIMESTAMP_S.clone())?;
    push_diagnostics(&mut defs, &mut tags, DIAGNOSTICS_OUTDOOR_TEMP_C.clone())?;
    for (zone_id, _label) in &input.zones {
        for def in diagnostics_zone_variables(*zone_id) {
            push_diagnostics(&mut defs, &mut tags, def)?;
        }
    }
    push_diagnostics(&mut defs, &mut tags, DIAGNOSTICS_ELECTRICAL_NET_KW.clone())?;
    push_diagnostics(
        &mut defs,
        &mut tags,
        DIAGNOSTICS_ELECTRICAL_NET_KVAR.clone(),
    )?;
    for def in [
        DIAGNOSTICS_PORT_RADIANT_W.clone(),
        DIAGNOSTICS_PORT_SHORTWAVE_W.clone(),
        DIAGNOSTICS_PORT_CONVECTIVE_W.clone(),
        DIAGNOSTICS_WINDOW_SOLAR_W.clone(),
        DIAGNOSTICS_OPAQUE_SOLAR_LWR_W.clone(),
        DIAGNOSTICS_INTERIOR_LWR_W.clone(),
        DIAGNOSTICS_INTERNAL_GAIN_W.clone(),
        DIAGNOSTICS_AIR_DENSITY.clone(),
        DIAGNOSTICS_GHI.clone(),
    ] {
        push_diagnostics(&mut defs, &mut tags, def)?;
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
    let equipment_end_uses = instances
        .iter()
        .map(|(eq, name)| (name.clone(), eq.end_use.clone()))
        .collect();
    Ok(Catalogue {
        defs,
        tags,
        index,
        telemetry_keys,
        equipment_names,
        equipment_end_uses,
    })
}

/// Validates and appends one variable with its schema-derivation tags.
/// The schema-derivation tags one catalogue variable carries: the minimum
/// verbosity tier that emits it, whether the variable exists only under the
/// `observe` feature gate, and whether it is a diagnostics variable (the
/// diagnostics report's block: never emitted by a verbosity preset,
/// appended to the schema only when the run requests the diagnostics).
#[derive(Debug, Clone, Copy)]
struct VarTag {
    min_verbosity: u8,
    observe_only: bool,
    diagnostics: bool,
}

/// Emits the variable's nullability at catalogue build, at `debug` (B-6):
/// the catalogue decides, per variable, what a missing value means.
fn push_traced(
    defs: &mut Vec<VariableDef>,
    tags: &mut Vec<VarTag>,
    def: VariableDef,
    min_v: u8,
    observe_only: bool,
) -> Result<(), CatalogueError> {
    tracing::debug!(
        variable = %def.name,
        nullability = if def.nullable { "nullable" } else { "not nullable" },
        "output variable nullability"
    );
    push(defs, tags, def, min_v, observe_only)
}

/// The field-metadata marker a diagnostics column carries in the Arrow
/// schema: the diagnostics block is self-describing, so the row plan's
/// freeze resolves the block from the schema alone.
pub const DIAGNOSTICS_METADATA_KEY: &str = "hares_diagnostics";

fn push(
    defs: &mut Vec<VariableDef>,
    tags: &mut Vec<VarTag>,
    def: VariableDef,
    min_v: u8,
    observe_only: bool,
) -> Result<(), CatalogueError> {
    def.validate()?;
    defs.push(def);
    tags.push(VarTag {
        min_verbosity: min_v,
        observe_only,
        diagnostics: false,
    });
    Ok(())
}

/// Appends one diagnostics variable: never in a verbosity preset, tagged
/// for the diagnostics report's schema block.
fn push_diagnostics(
    defs: &mut Vec<VariableDef>,
    tags: &mut Vec<VarTag>,
    def: VariableDef,
) -> Result<(), CatalogueError> {
    def.validate()?;
    defs.push(def);
    tags.push(VarTag {
        min_verbosity: 0,
        observe_only: false,
        diagnostics: true,
    });
    Ok(())
}

/// Pushes the dwelling-level static variables of one verbosity block.
fn push_statics(
    defs: &mut Vec<VariableDef>,
    tags: &mut Vec<VarTag>,
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
            nullable: false,
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
            nullable: false,
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
            description: Cow::Borrowed("Operating mode code (the catalogue's mode labels)"),
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
            nullable: false,
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
            nullable: false,
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
            nullable: false,
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
            nullable: false,
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
            nullable: false,
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

/// The end-use aggregate electric power column's declaration. The column
/// exists only for an end use with at least one contributing equipment
/// (the catalogue emits no other), so the zeroed accumulator is the value
/// in every step and the cell is never null.
static END_USE_AGGREGATE_VARIABLE: VariableDef = VariableDef {
    name: Cow::Borrowed(""),
    unit: Unit::Kilowatt,
    kind: VariableKind::Power,
    aggregation: Aggregation::Mean,
    sampling: Sampling::IntervalMean,
    nullable: false,
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

/// The diagnostics report's variables: the per-step diagnostics the
/// pre-catalogue CSV writer carried, declared once and appended to the
/// catalogue as one block. A verbosity preset never emits them; the
/// diagnostics request alone selects them. The names are the writer's
/// column names, so the report carries the same surface the CSV did; the
/// per-zone names key the zone's id (`zone1_temp_c` is the zone with
/// `ZoneId(1)`), and their sources are per-zone.
macro_rules! diagnostics_variable {
    ($name:literal, $unit:expr, $kind:expr, $agg:expr, $sampling:expr, $desc:literal) => {
        VariableDef {
            name: Cow::Borrowed($name),
            unit: $unit,
            kind: $kind,
            aggregation: $agg,
            sampling: $sampling,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed($desc),
        }
    };
}

static DIAGNOSTICS_STEP: VariableDef = diagnostics_variable!(
    "step",
    Unit::Count,
    VariableKind::Quantity {
        across_homes: AcrossHomes::Sum,
    },
    Aggregation::Sum,
    Sampling::InstantAtEnd,
    "The row's step index"
);
static DIAGNOSTICS_TIMESTAMP_S: VariableDef = diagnostics_variable!(
    "timestamp_s",
    Unit::Second,
    VariableKind::Quantity {
        across_homes: AcrossHomes::Mean,
    },
    Aggregation::Mean,
    Sampling::InstantAtEnd,
    "The row's start time as UTC epoch seconds"
);
static DIAGNOSTICS_OUTDOOR_TEMP_C: VariableDef = diagnostics_variable!(
    "outdoor_temp_c",
    Unit::Celsius,
    VariableKind::Temperature,
    Aggregation::Mean,
    Sampling::InstantAtEnd,
    "Outdoor dry-bulb temperature"
);
static DIAGNOSTICS_ELECTRICAL_NET_KW: VariableDef = diagnostics_variable!(
    "electrical_net_kw",
    Unit::Kilowatt,
    VariableKind::Power,
    Aggregation::Mean,
    Sampling::IntervalMean,
    "Net electric power from the electrical ports (consumption positive)"
);
static DIAGNOSTICS_ELECTRICAL_NET_KVAR: VariableDef = diagnostics_variable!(
    "electrical_net_kvar",
    Unit::VoltAmpereReactive,
    VariableKind::Power,
    Aggregation::Mean,
    Sampling::IntervalMean,
    "Net reactive power from the electrical ports"
);
static DIAGNOSTICS_WINDOW_SOLAR_W: VariableDef = diagnostics_variable!(
    "window_solar_w",
    Unit::Watt,
    VariableKind::Power,
    Aggregation::Mean,
    Sampling::IntervalMean,
    "Window solar gain from the thermal solver's component gains"
);
static DIAGNOSTICS_OPAQUE_SOLAR_LWR_W: VariableDef = diagnostics_variable!(
    "opaque_solar_lwr_w",
    Unit::Watt,
    VariableKind::Power,
    Aggregation::Mean,
    Sampling::IntervalMean,
    "Opaque exterior solar and long-wave absorption from the component gains"
);
static DIAGNOSTICS_INTERIOR_LWR_W: VariableDef = diagnostics_variable!(
    "interior_lwr_w",
    Unit::Watt,
    VariableKind::Power,
    Aggregation::Mean,
    Sampling::IntervalMean,
    "Interior long-wave radiation exchange activity from the component gains"
);
static DIAGNOSTICS_INTERNAL_GAIN_W: VariableDef = diagnostics_variable!(
    "internal_gain_w",
    Unit::Watt,
    VariableKind::Power,
    Aggregation::Mean,
    Sampling::IntervalMean,
    "Non-HVAC internal gains from the component gains"
);
static DIAGNOSTICS_PORT_CONVECTIVE_W: VariableDef = diagnostics_variable!(
    "port_convective_w",
    Unit::Watt,
    VariableKind::Power,
    Aggregation::Mean,
    Sampling::IntervalMean,
    "Total port convective gain (HVAC and appliances)"
);
static DIAGNOSTICS_PORT_RADIANT_W: VariableDef = diagnostics_variable!(
    "port_radiant_w",
    Unit::Watt,
    VariableKind::Power,
    Aggregation::Mean,
    Sampling::IntervalMean,
    "Total port radiant gain distributed to surfaces (HVAC and appliances)"
);
static DIAGNOSTICS_PORT_SHORTWAVE_W: VariableDef = diagnostics_variable!(
    "port_shortwave_w",
    Unit::Watt,
    VariableKind::Power,
    Aggregation::Mean,
    Sampling::IntervalMean,
    "Total port short-wave gain absorbed by the surfaces"
);
static DIAGNOSTICS_AIR_DENSITY: VariableDef = diagnostics_variable!(
    "air_density_kg_m3",
    Unit::KilogramPerCubicMeter,
    VariableKind::Quantity {
        across_homes: AcrossHomes::Mean,
    },
    Aggregation::Mean,
    Sampling::InstantAtEnd,
    "Outdoor moist-air density used for infiltration mass-flow conversion"
);
static DIAGNOSTICS_GHI: VariableDef = diagnostics_variable!(
    "ghi_w_m2",
    Unit::WattPerSquareMeter,
    VariableKind::Quantity {
        across_homes: AcrossHomes::Mean,
    },
    Aggregation::Mean,
    Sampling::InstantAtEnd,
    "Global horizontal irradiance at this timestep"
);

/// The diagnostics block's per-zone variables: the zone's temperature and
/// the thermal port's sensible and latent gain into it.
fn diagnostics_zone_variables(zone_id: ZoneId) -> Vec<VariableDef> {
    let mut vars = Vec::with_capacity(3);
    let named = |suffix: &str, mut def: VariableDef| {
        def.name = Cow::Owned(format!("zone{}{suffix}", zone_id.0));
        def.source = VariableSource::Zone(zone_id);
        def
    };
    vars.push(named(
        "_temp_c",
        diagnostics_variable!(
            "",
            Unit::Celsius,
            VariableKind::Temperature,
            Aggregation::Mean,
            Sampling::InstantAtEnd,
            "Zone temperature"
        ),
    ));
    vars.push(named(
        "_thermal_gain_w",
        diagnostics_variable!(
            "",
            Unit::Watt,
            VariableKind::Power,
            Aggregation::Mean,
            Sampling::IntervalMean,
            "Total thermal port sensible gain into the zone (HVAC and appliances combined)"
        ),
    ));
    vars.push(named(
        "_latent_gain_w",
        diagnostics_variable!(
            "",
            Unit::Watt,
            VariableKind::Power,
            Aggregation::Mean,
            Sampling::IntervalMean,
            "Thermal port latent gain into the zone"
        ),
    ));
    vars
}

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
    // The not-nullable form: a quantity that is physically zero when its
    // component is inactive, so the producer substitutes 0.0 for an absent
    // channel and a missing value is a wiring defect, never a null.
    ($key:literal, $suffix:literal, $unit:expr, $kind:expr, $agg:expr, $sampling:expr, $desc:literal, not_nullable) => {
        TelemetryColumn {
            key: $key,
            suffix: $suffix,
            def: VariableDef {
                name: Cow::Borrowed($suffix),
                unit: $unit,
                kind: $kind,
                aggregation: $agg,
                sampling: $sampling,
                nullable: false,
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
        "Backup electric resistance power during ER-only or HP+ER modes",
        not_nullable
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
        "Pre-DSE gross latent cooling scaled by space fraction",
        not_nullable
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
        "Fan electric power",
        not_nullable
    ),
    telemetry_column!(
        "runtime_fraction",
        "Runtime Fraction (-)",
        Unit::Dimensionless,
        VariableKind::Ratio,
        Aggregation::Mean,
        Sampling::InstantAtEnd,
        "Runtime fraction (PLR/PLF)",
        not_nullable
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
            nullable: false,
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
            nullable: false,
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
            nullable: false,
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
    // The engine's per-step weather channels (the resample_weather row):
    // in no verbosity preset (no preset's schema carries them, so no
    // golden moves), selected by name.
    (
        "Outdoor Humidity Ratio (kg/kg)",
        VariableDef {
            name: Cow::Borrowed("Outdoor Humidity Ratio (kg/kg)"),
            unit: Unit::KilogramPerKilogram,
            kind: VariableKind::Ratio,
            aggregation: Aggregation::Mean,
            sampling: Sampling::InstantAtEnd,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("Outdoor humidity ratio"),
        },
        9,
        false,
    ),
    (
        "Outdoor Pressure (kPa)",
        VariableDef {
            name: Cow::Borrowed("Outdoor Pressure (kPa)"),
            unit: Unit::Kilopascal,
            kind: VariableKind::Quantity {
                across_homes: AcrossHomes::Mean,
            },
            aggregation: Aggregation::Mean,
            sampling: Sampling::InstantAtEnd,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("Outdoor air pressure"),
        },
        9,
        false,
    ),
    (
        "Wind Speed (m/s)",
        VariableDef {
            name: Cow::Borrowed("Wind Speed (m/s)"),
            unit: Unit::MeterPerSecond,
            kind: VariableKind::Quantity {
                across_homes: AcrossHomes::Mean,
            },
            aggregation: Aggregation::Mean,
            sampling: Sampling::InstantAtEnd,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("Outdoor wind speed"),
        },
        9,
        false,
    ),
    (
        "Sky Temperature (C)",
        VariableDef {
            name: Cow::Borrowed("Sky Temperature (C)"),
            unit: Unit::Celsius,
            kind: VariableKind::Temperature,
            aggregation: Aggregation::Mean,
            sampling: Sampling::InstantAtEnd,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("Sky temperature"),
        },
        9,
        false,
    ),
    (
        "GHI (W/m2)",
        VariableDef {
            name: Cow::Borrowed("GHI (W/m2)"),
            unit: Unit::WattPerSquareMeter,
            kind: VariableKind::Quantity {
                across_homes: AcrossHomes::Mean,
            },
            aggregation: Aggregation::Mean,
            sampling: Sampling::IntervalMean,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("Horizontal global irradiance"),
        },
        9,
        false,
    ),
    (
        "DNI (W/m2)",
        VariableDef {
            name: Cow::Borrowed("DNI (W/m2)"),
            unit: Unit::WattPerSquareMeter,
            kind: VariableKind::Quantity {
                across_homes: AcrossHomes::Mean,
            },
            aggregation: Aggregation::Mean,
            sampling: Sampling::IntervalMean,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("Direct normal irradiance"),
        },
        9,
        false,
    ),
    (
        "DHI (W/m2)",
        VariableDef {
            name: Cow::Borrowed("DHI (W/m2)"),
            unit: Unit::WattPerSquareMeter,
            kind: VariableKind::Quantity {
                across_homes: AcrossHomes::Mean,
            },
            aggregation: Aggregation::Mean,
            sampling: Sampling::IntervalMean,
            nullable: true,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("Diffuse horizontal irradiance"),
        },
        9,
        false,
    ),
];

/// Builds the Arrow output schema from the catalogue at `verbosity`: the
/// `Time` label, then every catalogue variable emitted at the verbosity,
/// preserving catalogue order. With `diagnostics`, the diagnostics block
/// appends last, each of its fields marked so the row plan's freeze
/// resolves the block from the schema alone. Field metadata carries
/// `Unit::symbol()` as the `unit` entry (no metadata for [`Unit::None`]);
/// the schema metadata carries the catalogue digest.
#[must_use]
pub fn build_schema(catalogue: &Catalogue, verbosity: u8, diagnostics: bool) -> Schema {
    let mut fields = vec![time_field()];

    for var in catalogue.variables_at_verbosity(verbosity) {
        fields.push(variable_field(var));
    }

    if diagnostics {
        for var in catalogue.diagnostics_variables() {
            fields.push(diagnostics_field(var));
        }
    }

    Schema::new_with_metadata(fields, schema_metadata(catalogue))
}

/// The output schema's Time field: a millisecond timestamp with no
/// embedded time zone, never null. The value is the row's naive local
/// wall-clock milliseconds; the schema metadata's `hares:utc_offset_s`
/// (added per run at the recorder freeze) recovers the instant.
#[must_use]
pub fn time_field() -> arrow::datatypes::FieldRef {
    std::sync::Arc::new(Field::new(
        TIMESTAMP_COL,
        DataType::Timestamp(TimeUnit::Millisecond, None),
        false,
    ))
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

/// One diagnostics block's Arrow field: the column name, `Float64`, the
/// declaration's nullability, the unit metadata, and the block's own
/// marker the row plan's freeze resolves the selection from.
fn diagnostics_field(var: &VariableDef) -> arrow::datatypes::FieldRef {
    let mut metadata = std::collections::HashMap::new();
    if var.unit != Unit::None {
        metadata.insert("unit".to_string(), var.unit.symbol().to_string());
    }
    metadata.insert(DIAGNOSTICS_METADATA_KEY.to_string(), "true".to_string());
    std::sync::Arc::new(
        Field::new(var.name.as_ref(), DataType::Float64, var.nullable).with_metadata(metadata),
    )
}

/// Builds the Arrow output schema of a `Diagnostics`-selected report: the
/// `Time` label, then the diagnostics block's fields, each marked so the
/// row plan's freeze resolves the block from the schema alone.
#[must_use]
pub fn build_schema_for_diagnostics(catalogue: &Catalogue) -> Schema {
    let mut fields = vec![time_field()];
    for var in catalogue.diagnostics_variables() {
        fields.push(diagnostics_field(var));
    }
    Schema::new_with_metadata(fields, schema_metadata(catalogue))
}

/// The schema metadata every schema builder carries: the catalogue digest
/// naming the catalogue the schema's variables come from. The run's site
/// offset joins it at the recorder freeze (the dwelling's
/// `hares:utc_offset_s`), the one key the schema builders cannot know.
fn schema_metadata(catalogue: &Catalogue) -> std::collections::HashMap<String, String> {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("hares:catalogue_digest".to_string(), catalogue.digest());
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
    let mut fields = vec![time_field()];
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
        // A diagnostics variable keeps the block's marker, so a schema
        // built from explicit columns (the union the reports' selections
        // resolve to) still freezes the block's producer bindings.
        let field = if catalogue
            .diagnostics_variables()
            .iter()
            .any(|d| d.name == var.name)
        {
            diagnostics_field(var)
        } else {
            variable_field(var)
        };
        fields.push(field);
    }
    Schema::new_with_metadata(fields, schema_metadata(catalogue))
}

/// Builds a catalogue from spec-shaped inputs (no live descriptors) and
/// derives the schema from it: the compatibility path the spec-based
/// callers and tests use. Equipment ids are positional.
///
/// The typed declarations come from the class's own declared surface
/// (`hares_equipment::class_declared_surface`): the end use, the core
/// capabilities and the declared telemetry the class's constructor
/// declares. A spec whose name is not a registered class declares no
/// surface and rolls up under `EndUse::OTHER` with no capabilities and
/// no declarations.
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
            .map(|(i, spec)| {
                let (end_use, core_capabilities, telemetry) =
                    match hares_equipment::class_declared_surface(&spec.name) {
                        Some(surface) => (
                            surface.end_use,
                            surface.core_capabilities,
                            surface.telemetry,
                        ),
                        None => (EndUse::OTHER, CoreCapabilities::default(), Vec::new()),
                    };
                CatalogueEquipment {
                    name: spec.name.clone(),
                    instance_name: spec.instance_name.clone(),
                    id: EquipmentId(u32::try_from(i + 1).expect("equipment count")),
                    fuel: spec.fuel_type,
                    end_use,
                    core_capabilities,
                    telemetry,
                }
            })
            .collect(),
        zones: zone_names.to_vec(),
        actors: Vec::new(),
    };
    match build_catalogue(&input) {
        Ok(catalogue) => build_schema(&catalogue, verbosity, false),
        // The spec path cannot produce duplicates the live path could not
        // (the same declared surfaces, the same names); a panic here is a
        // bug in the static tables, not an input error.
        Err(err) => panic!("catalogue build failed from specs: {err}"),
    }
}

/// The digest's canonical encoding, pinned against a byte string the test
/// writes out by the documented rule, over a two-variable catalogue.
#[cfg(test)]
mod digest_tests {
    use std::borrow::Cow;
    use std::collections::HashMap;

    use hares_types::{Aggregation, Sampling, Unit, VariableKind};

    use super::{Catalogue, CatalogueError, VarTag, VariableDef, VariableSource};

    fn def(name: &str, unit: Unit, kind: VariableKind, nullable: bool) -> VariableDef {
        VariableDef {
            name: Cow::Owned(name.to_string()),
            unit,
            aggregation: if matches!(kind, VariableKind::Energy) {
                Aggregation::Sum
            } else {
                Aggregation::Mean
            },
            sampling: if nullable {
                Sampling::InstantAtStart
            } else {
                Sampling::IntervalMean
            },
            nullable,
            source: VariableSource::Dwelling,
            description: Cow::Borrowed("digest test variable"),
            kind,
        }
    }

    fn catalogue_from_defs(defs: Vec<VariableDef>) -> Catalogue {
        let tags: Vec<VarTag> = defs
            .iter()
            .map(|_| VarTag {
                min_verbosity: 0,
                observe_only: false,
                diagnostics: false,
            })
            .collect();
        let index = defs
            .iter()
            .enumerate()
            .map(|(i, def)| (def.name.to_string(), i))
            .collect::<HashMap<_, _>>();
        Catalogue {
            defs,
            tags,
            index,
            telemetry_keys: HashMap::new(),
            equipment_names: Vec::new(),
            equipment_end_uses: Vec::new(),
        }
    }

    fn two_variable_catalogue() -> Catalogue {
        catalogue_from_defs(vec![
            def(
                "Total Electric Power (kW)",
                Unit::Kilowatt,
                VariableKind::Power,
                false,
            ),
            def(
                "Temperature - Indoor (C)",
                Unit::Celsius,
                VariableKind::Temperature,
                true,
            ),
        ])
    }

    #[test]
    fn catalogue_digest_is_canonical() {
        use sha2::Digest as _;

        let catalogue = two_variable_catalogue();
        // The byte string the rule writes for these two variables, by
        // hand: the header line, then per variable its fields, each
        // preceded by the NUL byte, terminated with a newline.
        let canonical: &[u8] = b"hares-catalogue-v1\n\
            \x00Total Electric Power (kW)\x00power\x00mean\x00interval-mean\x00kW\x000\n\
            \x00Temperature - Indoor (C)\x00temperature\x00mean\x00instant-at-start\x00C\x001\n";
        let mut hasher = sha2::Sha256::default();
        hasher.update(canonical);
        let expected = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        assert_eq!(
            catalogue.digest(),
            expected,
            "the digest is the SHA-256 of the canonical byte string"
        );
    }

    #[test]
    fn catalogue_digest_changes_when_a_variable_changes() {
        let catalogue = two_variable_catalogue();
        let changed = catalogue
            .variables()
            .iter()
            .map(|var| {
                let mut var = var.clone();
                if var.name == "Temperature - Indoor (C)" {
                    var.nullable = !var.nullable;
                }
                var
            })
            .collect::<Vec<_>>();
        assert_ne!(
            catalogue.digest(),
            catalogue_from_defs(changed).digest(),
            "a nullability change is a catalogue change and moves the digest"
        );
    }

    #[test]
    fn catalogue_digest_is_order_sensitive() {
        let catalogue = two_variable_catalogue();
        let swapped = catalogue
            .variables()
            .iter()
            .rev()
            .cloned()
            .collect::<Vec<_>>();
        assert_ne!(
            catalogue.digest(),
            catalogue_from_defs(swapped).digest(),
            "the digest covers schema order: the same variables in another order"
        );
    }

    #[test]
    fn catalogue_digest_metadata_matches() {
        let catalogue = two_variable_catalogue();
        let digest = catalogue.digest();
        let schema = super::build_schema(&catalogue, 0, false);
        assert_eq!(
            schema
                .metadata()
                .get("hares:catalogue_digest")
                .map(String::as_str),
            Some(digest.as_str()),
            "the schema's digest metadata is the hex of Catalogue::digest()"
        );
    }

    #[test]
    fn catalogue_from_defs_validates_every_definition() {
        let mut mismatch = def(
            "Total Energy (kWh)",
            Unit::KilowattHour,
            VariableKind::Energy,
            false,
        );
        mismatch.aggregation = Aggregation::Mean;
        let err = Catalogue::from_defs(vec![
            def(
                "Total Electric Power (kW)",
                Unit::Kilowatt,
                VariableKind::Power,
                false,
            ),
            mismatch,
        ])
        .expect_err("an aggregation that contradicts the kind is refused");
        assert!(
            matches!(err, CatalogueError::AggregationMismatch { .. }),
            "the typed error names the variable's mismatch: {err}"
        );
    }

    #[test]
    fn catalogue_from_defs_refuses_an_empty_list() {
        let err = Catalogue::from_defs(Vec::new()).expect_err("a catalogue must carry variables");
        assert!(
            matches!(err, CatalogueError::NoVariables),
            "the empty list is the no-variables error: {err}"
        );
    }
}
