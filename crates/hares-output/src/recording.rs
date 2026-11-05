//! The output row plan's producer bindings: the recording machinery the
//! step path writes through.
//!
//! A run's output columns are fixed for the life of the run, so the
//! name-to-position mapping is resolved exactly once, at the row plan's
//! freeze, into these binding types; the per-step path writes values by
//! precomputed index and never looks a column name up. The dwelling's
//! recording layer (the step-side caller in `hares-core`) freezes one
//! [`OutputRowBindings`] per plan build and holds it for the run's life.
//!
//! The bindings here are the schema-facing half of the recording: what
//! each producer writes where. The producers' own values (the equipment
//! cores and telemetry, the step's environment and port state, the
//! envelope's component gains) stay with the dwelling; the step-side
//! writer that reads them and the gains-typed envelope extractors live
//! with the gains type in `hares-core`.

use std::collections::{HashMap, HashSet};

use arrow::datatypes::Schema;
use hares_equipment::Equipment;
use hares_types::{HaresError, Telemetry, ZoneId, telemetry_keys as tk};

use crate::Catalogue;
use crate::catalogue::EquipmentColumnFamilies;
use crate::columns::{
    CAPACITY_SUFFIX, COMPRESSOR_POWER_KW_SUFFIX, COMPRESSOR_POWER_W_SUFFIX, COP_SUFFIX,
    DEFROST_STATE_SUFFIX, ELECTRIC_POWER_SUFFIX, ENERGY_SUFFIX, ER_CAPACITY_SUFFIX,
    ER_POWER_SUFFIX, EV_CHARGING_LEVEL_SUFFIX, EV_CONNECTION_STATE_SUFFIX,
    FAN_ELECTRIC_POWER_SUFFIX, FAN_POWER_SUFFIX, FAN_POWER_W_SUFFIX, GAS_POWER_SUFFIX,
    HP_CAPACITY_SUFFIX, HVAC_DUCT_LOSSES_COL, LATENT_GAINS_SUFFIX, MAIN_POWER_SUFFIX,
    MIN_OFF_TIME_SUFFIX, MIN_ON_TIME_SUFFIX, MODE_SUFFIX, PAN_HEATER_POWER_SUFFIX,
    POWER_FACTOR_SUFFIX, PV_DC_POWER_SUFFIX, PV_IRRADIANCE_SUFFIX, REACTIVE_POWER_SUFFIX,
    RETURN_TEMP_SUFFIX, RUNTIME_COOLING_SETPOINT_COL, RUNTIME_FRACTION_SUFFIX,
    RUNTIME_HEATING_SETPOINT_COL, SCHEDULE_SUFFIX, SCHEDULED_COOLING_SETPOINT_COL,
    SCHEDULED_HEATING_SETPOINT_COL, SETPOINT_SUFFIX, SHR_SUFFIX, SOC_SUFFIX, SPEED_SUFFIX,
    SUPPLY_AIR_TEMP_SUFFIX, SUPPLY_TEMP_SUFFIX, end_use_electric_power_column,
    is_reserved_output_column_name,
};

/// The schema's value columns as a name-to-position map: the freeze-time
/// column index every binding resolution below runs against. Recorder rows
/// exclude the timestamp column.
pub fn build_output_column_index(schema: &Schema) -> HashMap<String, usize> {
    schema
        .fields()
        .iter()
        .skip(1)
        .enumerate()
        .map(|(idx, field)| (field.name().to_string(), idx))
        .collect()
}

/// Pre-resolved output column indices for one equipment piece.
#[derive(Debug, Clone, Default)]
pub struct EquipmentColumns {
    pub electric_power: Option<usize>,
    pub gas_power: Option<usize>,
    pub mode: Option<usize>,
    pub setpoint: Option<usize>,
    pub soc: Option<usize>,
    pub capacity: Option<usize>,
    pub cop: Option<usize>,
    pub reactive_power: Option<usize>,
    pub power_factor: Option<usize>,
    pub energy_kwh: Option<usize>,
    pub schedule: Option<usize>,
    pub defrost_state: Option<usize>,
    pub er_power: Option<usize>,
    pub shr: Option<usize>,
    pub speed: Option<usize>,
    pub fan_power: Option<usize>,
    pub main_power: Option<usize>,
    pub runtime_fraction: Option<usize>,
    pub latent_gains: Option<usize>,
    pub duct_losses: Option<usize>,
    /// Verbosity-8 per-equipment telemetry columns, as
    /// (telemetry_key, column_index), split by what the value is: a state
    /// is recorded even for a failed step (it keeps the committed value), a
    /// flow is not (the failed step delivered nothing).
    pub v8_state_columns: Vec<(&'static str, usize)>,
    pub v8_flow_columns: Vec<(&'static str, usize)>,
}

/// What a frame column's value is, which decides what a failed step
/// records in it.
#[derive(Clone, Copy)]
enum RecordedValue {
    State,
    Flow,
}

/// Enriches the catalogue-derived output schema's Arrow field metadata with
/// unit declarations from equipment `TelemetryField` descriptors, closing
/// the round-trip gap between equipment-declared units and the columns the
/// aggregation pipeline reads.
///
/// For every schema column whose name starts with an equipment instance
/// name, the column's unit (the catalogue declaration's `Unit::symbol()`)
/// is verified against the corresponding equipment's `telemetry_fields`.
/// For a column the catalogue maps to a telemetry key
/// ([`Catalogue::telemetry_key`]), the check is per field: the equipment's
/// declared field for that key must carry the column's unit; a mismatch
/// warns and the column goes unvalidated (the assembly check makes this
/// unreachable for a catalogue built from the same equipment; it is the
/// net for a schema built against a different declaration set). For a
/// column with no telemetry key (the core-output columns, read from
/// `CoreOutput`), the unit is checked against the equipment's declared
/// unit set. When the unit is found, the field's metadata is enriched with
/// a `"unit_source"` key set to `"telemetry_field"` to indicate the unit
/// has been validated against the authoritative equipment telemetry
/// declaration.
///
/// A telemetry-keyed column whose owning equipment does not declare its
/// key (the per-capability column exists for the name family; the
/// equipment never claimed the channel) keeps its catalogue-derived
/// metadata unannotated and does not warn.
pub fn enrich_schema_with_telemetry_units(
    schema: Schema,
    equipment: &[&dyn Equipment],
    catalogue: &Catalogue,
) -> Schema {
    // Compute instance-qualified names matching the catalogue's convention.
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for eq in equipment {
        *counts.entry(eq.descriptor().name.as_str()).or_default() += 1;
    }
    let mut indices: HashMap<&str, usize> = HashMap::new();
    let instance_units: Vec<(
        String,
        HashSet<String>,
        HashMap<&str, &hares_types::TelemetryField>,
    )> = equipment
        .iter()
        .map(|eq| {
            let desc = eq.descriptor();
            let base = desc.name.as_str();
            let name = if counts[base] > 1 {
                let idx = indices.entry(base).or_insert(0);
                *idx += 1;
                format!("{base} #{idx}")
            } else {
                base.to_string()
            };
            let units: HashSet<String> = desc
                .telemetry_fields
                .iter()
                .map(|tf| tf.unit.symbol().to_string())
                .collect();
            let by_key: HashMap<&str, &hares_types::TelemetryField> = desc
                .telemetry_fields
                .iter()
                .map(|tf| (tf.name.as_ref(), tf))
                .collect();
            (name, units, by_key)
        })
        .collect();

    // For each schema field that belongs to a known equipment instance,
    // verify the column's catalogue unit against the equipment's declared
    // telemetry field units and enrich metadata.
    let fields: Vec<std::sync::Arc<arrow::datatypes::Field>> = schema
        .fields()
        .iter()
        .map(|field| {
            let col_name = field.name();
            let mut metadata = field.metadata().clone();

            // Check if this column name starts with a known instance name
            // followed by a space (to avoid false prefix matches).
            let owner = instance_units.iter().find(|(name, _, _)| {
                col_name.as_bytes().starts_with(name.as_bytes())
                    && (col_name.len() == name.len() || col_name.as_bytes()[name.len()] == b' ')
            });
            if let Some((_name, units, by_key)) = owner
                && let Some(def) = catalogue.resolve(col_name)
                && def.unit != hares_types::Unit::None
            {
                // A telemetry-keyed column checks its key's declared field
                // (per field, per key); a keyless column (the core-output
                // ones) falls back to the whole-descriptor unit set.
                let keyed = catalogue.telemetry_key(col_name).map(|key| by_key.get(key));
                match keyed {
                    Some(Some(field)) => {
                        if field.unit == def.unit {
                            metadata
                                .insert("unit_source".to_string(), "telemetry_field".to_string());
                        } else {
                            tracing::warn!(
                                column = col_name,
                                unit = def.unit.symbol(),
                                declared = field.unit.symbol(),
                                "column unit disagrees with the equipment's declared \
                                 telemetry field for its key"
                            );
                        }
                    }
                    // The per-capability column exists for the name family;
                    // the owner never declared the channel: keep the
                    // catalogue-derived metadata unannotated.
                    Some(None) => {}
                    None => {
                        if units.contains(def.unit.symbol()) {
                            metadata
                                .insert("unit_source".to_string(), "telemetry_field".to_string());
                        } else {
                            tracing::warn!(
                                column = col_name,
                                unit = def.unit.symbol(),
                                "column unit not found in equipment telemetry fields"
                            );
                        }
                    }
                }
            }

            std::sync::Arc::new(field.as_ref().clone().with_metadata(metadata))
        })
        .collect();

    Schema::new_with_metadata(fields, schema.metadata().clone())
}

/// Build column index maps for each equipment piece using instance-qualified
/// names (matching `crate::columns::instance_qualified_names`).
/// Schema membership rule, single home for the column-map builder and
/// record_step's column invariants: an equipment is schema-known iff the
/// schema emitted its unconditional per-equipment column under its own
/// name (verbosity ≥ 1 always emits `{name} Electric Power (kW)`), and
/// that column is not one of the reserved aggregate names — an equipment
/// named "Total" must not read the aggregate column as evidence of its
/// own membership.
fn is_schema_known_equipment(column_index: &HashMap<String, usize>, name: &str) -> bool {
    let membership_column = format!("{name} {ELECTRIC_POWER_SUFFIX}");
    column_index.contains_key(&membership_column)
        && !is_reserved_output_column_name(&membership_column)
}

/// Resolves the per-equipment column positions of the row plan's producer
/// bindings, against the freeze-time column index of the schema the plan
/// freezes from. Runs once per plan build, never per step.
///
/// `expected` is the set of column names the run's reports promise (the
/// union of the selections resolved against the catalogue): a promised
/// column a schema-known equipment cannot resolve is schema drift and
/// fails the plan; a column no report selects resolves to `None` and the
/// step writes nothing to it.
fn plan_equipment_columns(
    equipment: &[&dyn Equipment],
    column_index: &HashMap<String, usize>,
    expected: &HashSet<String>,
) -> Result<Vec<EquipmentColumns>, HaresError> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for eq in equipment {
        *counts.entry(&eq.descriptor().name).or_default() += 1;
    }
    let mut indices: HashMap<&str, usize> = HashMap::new();
    equipment
        .iter()
        .map(|eq| {
            let desc = eq.descriptor();
            let base = &desc.name;
            let name = if counts[base.as_str()] > 1 {
                let idx = indices.entry(base).or_insert(0);
                *idx += 1;
                format!("{base} #{idx}")
            } else {
                base.clone()
            };
            // `expected` holds only for equipment the schema was built from.
            // The column map is re-derived against a FROZEN column index
            // whenever rows have been recorded (mid-run add/remove/replace),
            // and equipment that joined after the schema was frozen has no
            // columns in it — by design ("missing values, never misattributed
            // ones"). Demanding its columns would fail here; instead it gets
            // an empty map. Equipment the schema DOES know must resolve every
            // applicable column — a miss there is a real schema/map drift.
            // Membership is EXACT — the schema's unconditional
            // `{name} Electric Power (kW)` column at verbosity ≥ 1 — not a
            // name-prefix match: a mid-run equipment whose name is a
            // word-boundary prefix of another equipment's name ("Electric"
            // vs "Electric Resistance Water Heater") must not inherit the
            // other instance's columns as evidence of its own. It also
            // excludes the RESERVED aggregate namespace via the shared
            // membership rule (`is_schema_known_equipment`).
            let in_schema = is_schema_known_equipment(column_index, &name);
            // A schema-known equipment resolves — and demands — every
            // applicable column; a schema-unknown one claims NOTHING, not
            // even a column that happens to exist under its name (the
            // reserved aggregates are exactly that case: present in the
            // index, never this equipment's own).
            let resolve = |col_name: &str| -> Result<Option<usize>, HaresError> {
                if !in_schema {
                    return Ok(None);
                }
                resolve_against_index(col_name, column_index, &name, expected.contains(col_name))
            };
            // Shared predicate with build_schema: the schema emits a
            // "{name} Gas Power (therms/hour)" column iff this returns true,
            // so resolving with the same predicate guarantees every emitted
            // column is populated.
            let families = EquipmentColumnFamilies::of_descriptor(desc);
            let has_gas = families.has_gas;
            let is_hvac = families.is_hvac;
            let is_cooling = families.is_cooling;
            let is_hp_heater = families.is_hp_heater;
            let has_soc = families.has_soc;

            let electric_power = resolve(&format!("{name} {ELECTRIC_POWER_SUFFIX}"))?;
            let gas_power = if has_gas {
                resolve(&format!("{name} {GAS_POWER_SUFFIX}"))?
            } else {
                None
            };
            let mode = resolve(&format!("{name} {MODE_SUFFIX}"))?;
            let setpoint = if is_hvac {
                resolve(&format!("{name} {SETPOINT_SUFFIX}"))?
            } else {
                None
            };
            let soc = if has_soc {
                resolve(&format!("{name} {SOC_SUFFIX}"))?
            } else {
                None
            };
            let capacity = if is_hvac {
                resolve(&format!("{name} {CAPACITY_SUFFIX}"))?
            } else {
                None
            };
            let cop = if is_hvac {
                resolve(&format!("{name} {COP_SUFFIX}"))?
            } else {
                None
            };
            // Reactive/PF columns exist for every equipment at verbosity ≥ 5
            // (mirroring the unconditional Electric Power column): gas
            // equipment with electric parasitics (blower fans, circulation
            // pumps, draft inducers) emits reactive power too.
            let reactive = resolve(&format!("{name} {REACTIVE_POWER_SUFFIX}"))?;
            let pf = resolve(&format!("{name} {POWER_FACTOR_SUFFIX}"))?;
            let energy_kwh = resolve(&format!("{name} {ENERGY_SUFFIX}"))?;
            let schedule = resolve(&format!("{name} {SCHEDULE_SUFFIX}"))?;
            let defrost_state = if is_hp_heater {
                resolve(&format!("{name} {DEFROST_STATE_SUFFIX}"))?
            } else {
                None
            };
            let er_power = if is_hp_heater {
                resolve(&format!("{name} {ER_POWER_SUFFIX}"))?
            } else {
                None
            };
            let shr = if is_cooling {
                resolve(&format!("{name} {SHR_SUFFIX}"))?
            } else {
                None
            };
            let speed = if is_hvac {
                resolve(&format!("{name} {SPEED_SUFFIX}"))?
            } else {
                None
            };
            let fan_power = if is_hvac {
                resolve(&format!("{name} {FAN_POWER_SUFFIX}"))?
            } else {
                None
            };
            let main_power = if is_hvac {
                resolve(&format!("{name} {MAIN_POWER_SUFFIX}"))?
            } else {
                None
            };
            let runtime_fraction = if is_hvac {
                resolve(&format!("{name} {RUNTIME_FRACTION_SUFFIX}"))?
            } else {
                None
            };
            let latent_gains = if is_cooling {
                resolve(&format!("{name} {LATENT_GAINS_SUFFIX}"))?
            } else {
                None
            };
            // `HVAC Duct Losses (W)` is a GLOBAL aggregate column, not a
            // per-equipment one: record_step accumulates every equipment's
            // duct-loss telemetry into the same row slot (`row[idx] +=`),
            // and its invariant demands the column for every
            // equipment when a report selects it. It therefore resolves
            // for ANY equipment when the schema carries it — including
            // equipment added after the schema froze, whose per-equipment
            // columns are missing by design but whose duct contribution
            // must still count toward the aggregate (gating it on
            // `in_schema` would fail the invariant and silently undercount
            // the aggregate in release builds).
            let duct_losses = resolve_against_index(
                HVAC_DUCT_LOSSES_COL,
                column_index,
                &name,
                expected.contains(HVAC_DUCT_LOSSES_COL),
            )?;

            // V8 per-equipment telemetry diagnostic columns: resolved
            // leniently (a column no report selects is skipped), so the
            // run-level tier gate the old verbosity field carried is the
            // selections' own.
            let (v8_state_columns, v8_flow_columns) = resolve_v8_columns(
                &name,
                column_index,
                is_hvac,
                is_hp_heater,
                families.is_pv,
                families.is_ev,
            );

            Ok(EquipmentColumns {
                electric_power,
                gas_power,
                mode,
                setpoint,
                soc,
                capacity,
                cop,
                reactive_power: reactive,
                power_factor: pf,
                energy_kwh,
                schedule,
                defrost_state,
                er_power,
                shr,
                speed,
                fan_power,
                main_power,
                runtime_fraction,
                latent_gains,
                duct_losses,
                v8_state_columns,
                v8_flow_columns,
            })
        })
        .collect()
}

/// Resolve each equipment's EndUse aggregate electric-power column index:
/// positional, one entry per equipment in vector order (`record_step` zips
/// it against the equipment vector, accumulating every member's power into
/// its end-use's single column). The column is the equipment's TYPED end
/// use's aggregate: the rollup groups by the descriptor's declared end
/// use, never by the name string. The positional contract is why the map
/// must be re-derived whenever the equipment vector changes mid-run: a
/// remove that shifts the vector would otherwise leave every survivor
/// writing into its departed neighbour's end-use aggregate. Shared by
/// assembly, the fresh-schema rebuild, and the frozen-schema
/// re-derivation so the three cannot drift.
fn build_end_use_aggregate_indices(
    equipment: &[&dyn Equipment],
    column_index: &HashMap<String, usize>,
) -> Vec<Option<usize>> {
    equipment
        .iter()
        .map(|eq| {
            let col_name = end_use_electric_power_column(&eq.descriptor().end_use);
            column_index.get(&col_name).copied()
        })
        .collect()
}

/// Resolve each actor's telemetry columns against the output column
/// index: `(key, index)` entry lists, one per actor, in actor order.
/// Actors without telemetry get an empty entry list — the positional
/// `zip` against the actor vector in `record_step` requires a per-actor
/// entry regardless. Shared by the fresh-schema and frozen-schema roster
/// plans so the two derivations cannot drift; pre-resolving also keeps the
/// per-step `format!("actor:{name}:{key}")` out of the hot path.
///
/// Each actor enters as its `(name, telemetry)` view: the bindings' build
/// reads no other actor surface, so the caller (the dwelling's recording
/// layer, which owns the actor trait) passes the views instead of the
/// actor trait objects.
fn build_actor_column_map(
    actors: &[(&str, Option<&Telemetry>)],
    column_index: &HashMap<String, usize>,
) -> Vec<Vec<(String, usize)>> {
    actors
        .iter()
        .map(|(actor_name, telemetry)| {
            let Some(tel) = telemetry else {
                return Vec::new();
            };
            let mut entries = Vec::with_capacity(tel.0.len());
            for key in tel.0.keys() {
                let col_name = format!("actor:{actor_name}:{key}");
                if let Some(&idx) = column_index.get(&col_name) {
                    entries.push((key.clone(), idx));
                }
            }
            entries
        })
        .collect()
}

type V8Columns = Vec<(&'static str, usize)>;

/// Resolves one column name against the freeze-time column index. Schema
/// drift for a column the schema owes the equipment (`expected`) is a typed
/// error in every build profile; a missing column that was never demanded
/// resolves to `None` by design ("missing values, never misattributed
/// ones"). Private to the row plan's construction.
fn resolve_against_index(
    col_name: &str,
    column_index: &HashMap<String, usize>,
    equipment_name: &str,
    expected: bool,
) -> Result<Option<usize>, HaresError> {
    match column_index.get(col_name).copied() {
        Some(idx) => Ok(Some(idx)),
        None if expected => {
            tracing::warn!(
                column_name = %col_name,
                equipment_name = %equipment_name,
                "output column index not resolved; data will not be emitted for this column"
            );
            Err(HaresError::InvariantViolation {
                check_name: format!(
                    "output_schema_drift: expected output column '{col_name}' for equipment \
                     '{equipment_name}' not found in schema"
                ),
                value: 0.0,
                tolerance: 0.0,
            })
        }
        None => Ok(None),
    }
}

/// The dwelling-level output columns of the frozen row plan: every column
/// name the step path used to look up, resolved once at freeze to its
/// producer slot. `record_step` writes by these indices only.
///
/// The envelope gain columns are the dwelling-side half of the same
/// freeze: their extractors are typed on the envelope's component-gains
/// struct, so they resolve beside these columns (into the recording
/// layer's envelope column list) against the same index.
#[derive(Debug, Default)]
pub struct DwellingColumns {
    pub total_electric: Option<usize>,
    pub total_gas: Option<usize>,
    pub total_reactive: Option<usize>,
    /// The old-style indoor temperature column, fed from the first zone's
    /// temperature for schemas that still carry it.
    pub indoor_temp_fallback: Option<usize>,
    pub outdoor_dry_bulb: Option<usize>,
    pub ground_temp: Option<usize>,
    pub mains_temp: Option<usize>,
    pub outdoor_humidity_ratio: Option<usize>,
    pub pressure_kpa: Option<usize>,
    pub wind_speed_m_s: Option<usize>,
    pub sky_temp_c: Option<usize>,
    pub ghi_w_m2: Option<usize>,
    pub dni_w_m2: Option<usize>,
    pub dhi_w_m2: Option<usize>,
    /// Position of the first HVAC or water-heater equipment, whose
    /// telemetry feeds the setpoint chain columns.
    pub first_hvac: Option<usize>,
    pub scheduled_heating_setpoint: Option<usize>,
    pub scheduled_cooling_setpoint: Option<usize>,
    pub runtime_heating_setpoint: Option<usize>,
    pub runtime_cooling_setpoint: Option<usize>,
}

/// Resolves the dwelling-level columns of the row plan against the
/// freeze-time column index. Runs once per plan build, never per step.
fn build_dwelling_columns(
    column_index: &HashMap<String, usize>,
    first_hvac: Option<usize>,
) -> DwellingColumns {
    let resolve = |col_name: &str| column_index.get(col_name).copied();
    DwellingColumns {
        total_electric: resolve("Total Electric Power (kW)"),
        total_gas: resolve("Total Gas Power (therms/hour)"),
        total_reactive: resolve("Total Reactive Power (kVAR)"),
        indoor_temp_fallback: resolve("Indoor Temperature (C)"),
        outdoor_dry_bulb: resolve("Outdoor Dry Bulb (C)"),
        ground_temp: resolve("Temperature - Ground (C)"),
        mains_temp: resolve("Hot Water Mains Temperature (C)"),
        outdoor_humidity_ratio: resolve("Outdoor Humidity Ratio (kg/kg)"),
        pressure_kpa: resolve("Outdoor Pressure (kPa)"),
        wind_speed_m_s: resolve("Wind Speed (m/s)"),
        sky_temp_c: resolve("Sky Temperature (C)"),
        ghi_w_m2: resolve("GHI (W/m2)"),
        dni_w_m2: resolve("DNI (W/m2)"),
        dhi_w_m2: resolve("DHI (W/m2)"),
        first_hvac,
        scheduled_heating_setpoint: resolve(SCHEDULED_HEATING_SETPOINT_COL),
        scheduled_cooling_setpoint: resolve(SCHEDULED_COOLING_SETPOINT_COL),
        runtime_heating_setpoint: resolve(RUNTIME_HEATING_SETPOINT_COL),
        runtime_cooling_setpoint: resolve(RUNTIME_COOLING_SETPOINT_COL),
    }
}

/// The row plan's producer bindings: everything `record_step` writes,
/// resolved once at freeze against the schema the plan freezes from.
#[derive(Debug, Default)]
pub struct OutputRowBindings {
    pub dwelling: DwellingColumns,
    /// Output columns of each equipment, in equipment-vector order.
    pub equipment: Vec<EquipmentColumns>,
    /// End-use aggregate electric-power column of each equipment, in
    /// equipment-vector order.
    pub end_use_aggregates: Vec<Option<usize>>,
    /// Telemetry key to output column of each actor, in actor order.
    pub actors: Vec<Vec<(String, usize)>>,
    /// The slots valid every step whose value is the row's zero: the
    /// summed cells whose contributors exist (the duct losses slot when
    /// the dwelling has HVAC equipment; every bound end-use aggregate
    /// slot), and the not-nullable columns a roster change orphaned. The
    /// step marks them valid after the producers write.
    pub always_valid_slots: Vec<usize>,
    /// The diagnostics block's slots, present only when the frozen schema
    /// carries the block (the run requested the diagnostics report). The
    /// per-step gate rides this `Option`: with `None` the step reads and
    /// writes nothing for the diagnostics.
    pub diagnostics: Option<DiagnosticsBindings>,
}

/// The slot of one zone's three diagnostics columns.
#[derive(Debug, Default, Clone)]
pub struct ZoneDiagSlots {
    pub zone: ZoneId,
    pub temp_c: usize,
    pub thermal_gain_w: usize,
    pub latent_gain_w: usize,
}

/// The slots of the diagnostics block's envelope columns, in the block's
/// order.
#[derive(Debug, Default, Clone)]
pub struct DiagEnvelopeSlots {
    pub port_radiant_w: usize,
    pub port_shortwave_w: usize,
    pub port_convective_w: usize,
    pub window_solar_w: usize,
    pub opaque_solar_lwr_w: usize,
    pub interior_lwr_w: usize,
    pub internal_gain_w: usize,
    pub air_density_kg_m3: usize,
    pub ghi_w_m2: usize,
}

/// The diagnostics block's producer bindings: every diagnostics column's
/// slot, resolved once at freeze from the schema's marked fields.
#[derive(Debug, Default, Clone)]
pub struct DiagnosticsBindings {
    pub step: usize,
    pub timestamp_s: usize,
    pub outdoor_temp_c: usize,
    pub zones: Vec<ZoneDiagSlots>,
    pub electrical_net_w: usize,
    pub electrical_net_var: usize,
    pub envelope: DiagEnvelopeSlots,
}

/// Which per-zone diagnostics column a name spells.
#[derive(Debug, PartialEq)]
enum ZoneDiagKind {
    Temp,
    Sensible,
    Latent,
}

/// Parses a diagnostics zone column name: `zone{N}_temp_c`,
/// `zone{N}_thermal_gain_w` or `zone{N}_latent_gain_w`, where N is the
/// zone's id.
fn zone_diag_name(name: &str) -> Option<(u16, ZoneDiagKind)> {
    let rest = name.strip_prefix("zone")?;
    let underscore = rest.find('_')?;
    let id = rest[..underscore].parse().ok()?;
    let kind = match &rest[underscore..] {
        "_temp_c" => ZoneDiagKind::Temp,
        "_thermal_gain_w" => ZoneDiagKind::Sensible,
        "_latent_gain_w" => ZoneDiagKind::Latent,
        _ => return None,
    };
    Some((id, kind))
}

/// Resolves the diagnostics block's producer bindings from the frozen
/// schema: every field carrying the diagnostics marker, by name. `None`
/// when the schema carries no marked field, which is the per-step gate.
fn build_diagnostics_bindings(schema: &Schema) -> Option<DiagnosticsBindings> {
    let mut diag = DiagnosticsBindings::default();
    let mut zones: Vec<Option<ZoneDiagSlots>> = Vec::new();
    let mut any = false;
    for (slot, field) in schema.fields().iter().skip(1).enumerate() {
        if !field
            .metadata()
            .contains_key(crate::DIAGNOSTICS_METADATA_KEY)
        {
            continue;
        }
        any = true;
        let name = field.name().as_str();
        match name {
            "step" => diag.step = slot,
            "timestamp_s" => diag.timestamp_s = slot,
            "outdoor_temp_c" => diag.outdoor_temp_c = slot,
            "electrical_net_w" => diag.electrical_net_w = slot,
            "electrical_net_var" => diag.electrical_net_var = slot,
            "port_radiant_w" => diag.envelope.port_radiant_w = slot,
            "port_shortwave_w" => diag.envelope.port_shortwave_w = slot,
            "port_convective_w" => diag.envelope.port_convective_w = slot,
            "window_solar_w" => diag.envelope.window_solar_w = slot,
            "opaque_solar_lwr_w" => diag.envelope.opaque_solar_lwr_w = slot,
            "interior_lwr_w" => diag.envelope.interior_lwr_w = slot,
            "internal_gain_w" => diag.envelope.internal_gain_w = slot,
            "air_density_kg_m3" => diag.envelope.air_density_kg_m3 = slot,
            "ghi_w_m2" => diag.envelope.ghi_w_m2 = slot,
            other => {
                if let Some((id, kind)) = zone_diag_name(other) {
                    let index = usize::from(id);
                    if zones.len() < index {
                        zones.resize(index, None);
                    }
                    let entry = &mut zones[index - 1];
                    let entry = entry.get_or_insert_with(|| ZoneDiagSlots {
                        zone: ZoneId(id),
                        temp_c: 0,
                        thermal_gain_w: 0,
                        latent_gain_w: 0,
                    });
                    match kind {
                        ZoneDiagKind::Temp => entry.temp_c = slot,
                        ZoneDiagKind::Sensible => entry.thermal_gain_w = slot,
                        ZoneDiagKind::Latent => entry.latent_gain_w = slot,
                    }
                }
            }
        }
    }
    any.then(|| {
        diag.zones = zones.into_iter().flatten().collect();
        diag
    })
}

/// Constructs the row plan's producer bindings from the frozen schema: the
/// dwelling-level columns, the per-equipment columns (an expected variable
/// the schema cannot place is a typed drift error), the end-use aggregates
/// and the actor columns. The name-to-column index is the caller's
/// freeze-time resolution (the same index the dwelling-side envelope
/// freeze resolves against); nothing downstream of this construction can
/// look a column name up.
pub fn build_output_row_bindings(
    schema: &Schema,
    column_index: &HashMap<String, usize>,
    equipment: &[&dyn Equipment],
    actors: &[(&str, Option<&Telemetry>)],
    expected: &HashSet<String>,
) -> Result<OutputRowBindings, HaresError> {
    let first_hvac = equipment
        .iter()
        .position(|eq| eq.descriptor().end_use.is_hvac_or_water_heating());
    let dwelling = build_dwelling_columns(column_index, first_hvac);
    let equipment_columns = plan_equipment_columns(equipment, column_index, expected)?;
    let end_use_aggregates = build_end_use_aggregate_indices(equipment, column_index);
    // The slots valid every step whose value is the row's zero:
    // - the summed cells whose contributors exist: the duct-losses slot
    //   counts only when the dwelling has HVAC equipment (a non-HVAC
    //   equipment binds the aggregate's slot but contributes no duct
    //   distribution); an end-use aggregate slot exists only for an end
    //   use with at least one contributing equipment (the catalogue emits
    //   the column for no other);
    // - a not-nullable column whose producer left the roster: a mid-run
    //   remove against the frozen schema keeps the column (the reports'
    //   schemas never change mid-run) with nothing producing it; the
    //   variable's declaration says never null, so the row's zero stands
    //   (the cell's meaning reverts to the equipment's inactive zero).
    let has_hvac = equipment
        .iter()
        .any(|eq| eq.descriptor().end_use.is_hvac_or_water_heating());
    let mut always_valid_slots: Vec<usize> = end_use_aggregates
        .iter()
        .take(equipment.len())
        .flatten()
        .copied()
        .collect();
    if has_hvac {
        always_valid_slots.extend(equipment_columns.iter().filter_map(|c| c.duct_losses));
    }
    let claimed = claimed_slots(&dwelling, &equipment_columns, &end_use_aggregates);
    for (slot, field) in schema.fields().iter().skip(1).enumerate() {
        if !field.is_nullable() && !claimed.contains(&slot) {
            always_valid_slots.push(slot);
        }
    }

    Ok(OutputRowBindings {
        dwelling,
        equipment: equipment_columns,
        end_use_aggregates,
        actors: build_actor_column_map(actors, column_index),
        always_valid_slots,
        diagnostics: build_diagnostics_bindings(schema),
    })
}

/// The producer slots the current bindings claim: the dwelling totals',
/// every bound equipment column's and every end-use aggregate's. The
/// always-valid slot rule subtracts this set from the frozen schema's
/// not-nullable columns to find the orphans a roster change left.
fn claimed_slots(
    dwelling: &DwellingColumns,
    equipment: &[EquipmentColumns],
    end_use_aggregates: &[Option<usize>],
) -> std::collections::HashSet<usize> {
    let mut claimed = std::collections::HashSet::new();
    claimed.extend(dwelling.total_electric);
    claimed.extend(dwelling.total_gas);
    claimed.extend(dwelling.total_reactive);
    for cols in equipment {
        claimed.extend(cols.electric_power);
        claimed.extend(cols.gas_power);
        claimed.extend(cols.capacity);
        claimed.extend(cols.reactive_power);
        claimed.extend(cols.energy_kwh);
        claimed.extend(cols.schedule);
        claimed.extend(cols.er_power);
        claimed.extend(cols.fan_power);
        claimed.extend(cols.main_power);
        claimed.extend(cols.runtime_fraction);
        claimed.extend(cols.latent_gains);
    }
    claimed.extend(end_use_aggregates.iter().flatten());
    claimed
}

/// Resolves v8 per-equipment telemetry diagnostic columns for the given
/// equipment instance, as (state columns, flow columns) of
/// (telemetry_key, column_index) pairs populated directly from equipment
/// telemetry in record_step.
fn resolve_v8_columns(
    instance_name: &str,
    column_index: &HashMap<String, usize>,
    is_hvac: bool,
    is_hp_heater: bool,
    is_pv: bool,
    is_ev: bool,
) -> (V8Columns, V8Columns) {
    use RecordedValue::{Flow, State};
    const HVAC: &[(&str, &str, RecordedValue)] = &[
        (tk::SUPPLY_TEMP_C, SUPPLY_TEMP_SUFFIX, State),
        (tk::RETURN_TEMP_C, RETURN_TEMP_SUFFIX, State),
        (tk::COMPRESSOR_POWER_W, COMPRESSOR_POWER_W_SUFFIX, Flow),
        (tk::COMPRESSOR_KW, COMPRESSOR_POWER_KW_SUFFIX, Flow),
        (tk::FAN_ELECTRIC_W, FAN_ELECTRIC_POWER_SUFFIX, Flow),
        (tk::FAN_POWER_W, FAN_POWER_W_SUFFIX, Flow),
        (tk::MIN_ON_TIME_S, MIN_ON_TIME_SUFFIX, State),
        (tk::MIN_OFF_TIME_S, MIN_OFF_TIME_SUFFIX, State),
    ];
    const HP_HEATER: &[(&str, &str, RecordedValue)] = &[
        (tk::SUPPLY_AIR_TEMP_C, SUPPLY_AIR_TEMP_SUFFIX, State),
        (tk::PAN_HEATER_KW, PAN_HEATER_POWER_SUFFIX, Flow),
        (tk::HP_CAPACITY_W, HP_CAPACITY_SUFFIX, Flow),
        (tk::ER_CAPACITY_W, ER_CAPACITY_SUFFIX, Flow),
    ];
    const PV: &[(&str, &str, RecordedValue)] = &[
        (tk::DC_POWER_KW, PV_DC_POWER_SUFFIX, Flow),
        // The absorbed (soiled, capacity-weighted) irradiance the step
        // computes: an output of the step, not the array's state.
        (tk::IRRADIANCE_W_M2, PV_IRRADIANCE_SUFFIX, Flow),
    ];
    const EV: &[(&str, &str, RecordedValue)] = &[
        (tk::CONNECTION_STATE, EV_CONNECTION_STATE_SUFFIX, State),
        (tk::CHARGING_LEVEL, EV_CHARGING_LEVEL_SUFFIX, State),
    ];

    let mut state = Vec::new();
    let mut flow = Vec::new();
    for (applies, defs) in [
        (is_hvac, HVAC),
        (is_hp_heater, HP_HEATER),
        (is_pv, PV),
        (is_ev, EV),
    ] {
        if !applies {
            continue;
        }
        for &(key, suffix, value) in defs {
            if let Some(&idx) = column_index.get(&format!("{instance_name} {suffix}")) {
                match value {
                    State => state.push((key, idx)),
                    Flow => flow.push((key, idx)),
                }
            }
        }
    }
    (state, flow)
}

/// Pre-resolved zone column indices for record_step, avoiding per-step format!() allocations.
#[derive(Default)]
pub struct ZoneColumnCaches {
    /// ZoneId → column_index for per-zone infiltration columns (non-indoor only).
    pub infiltration_columns: HashMap<ZoneId, usize>,
    /// ZoneId → column_index for per-zone interior LWR columns.
    pub lwr_columns: HashMap<ZoneId, usize>,
    /// ZoneId → (heating_col, cooling_col) for per-zone HVAC thermal attribution columns.
    pub hvac_columns: HashMap<ZoneId, (usize, usize)>,
    /// Zone IDs in sorted order for StepResult construction.
    pub sorted_zone_ids: Vec<ZoneId>,
    /// For each entry in sorted_zone_ids, the index into EnvironmentState::zones where
    /// that zone lives. Enables O(1) temperature lookup in the hot loop.
    pub zone_env_indices: Vec<Option<usize>>,
    /// For each entry in sorted_zone_ids, the output column index for zone temperature,
    /// or None if no temperature column exists for that zone.
    pub zone_temp_col_indices: Vec<Option<usize>>,
}

/// The zone's output column label: the indoor zone by its role, an attic
/// by its typed zone type, every other zone by its id. The zone-column
/// builders' single naming site, shared with the schema builders.
pub fn zone_display_name(
    zone: ZoneId,
    indoor_zone: ZoneId,
    zone_type: Option<&hares_input::hpxml::ZoneType>,
) -> String {
    if zone == indoor_zone {
        "Indoor".to_string()
    } else if matches!(zone_type, Some(hares_input::hpxml::ZoneType::Attic)) {
        "Attic".to_string()
    } else {
        format!("Zone_{}", zone.0)
    }
}

pub fn build_zone_column_caches(
    zones: &[hares_types::ZoneState],
    zone_types: &[hares_input::hpxml::ZoneType],
    indoor_zone: ZoneId,
    column_index: &HashMap<String, usize>,
) -> ZoneColumnCaches {
    let mut sorted_zone_ids: Vec<ZoneId> = zones.iter().map(|z| z.id).collect();
    sorted_zone_ids.sort();

    let mut infiltration_columns = HashMap::new();
    let mut lwr_columns = HashMap::new();
    let mut hvac_columns = HashMap::new();
    let mut zone_env_indices = Vec::with_capacity(sorted_zone_ids.len());
    let mut zone_temp_col_indices = Vec::with_capacity(sorted_zone_ids.len());

    for &zone_id in &sorted_zone_ids {
        // Pre-compute the index of this zone in the zones slice for O(1) hot-loop access.
        let env_idx = zones.iter().position(|z| z.id == zone_id);
        let zone_type = env_idx.and_then(|idx| zone_types.get(idx));
        let label = zone_display_name(zone_id, indoor_zone, zone_type);
        let temp_key = format!("Temperature - {label} (C)");
        let temp_col = column_index.get(&temp_key).copied();
        if zone_id != indoor_zone {
            let inf_key = format!("Infiltration Heat Gain - {label} (W)");
            if let Some(&idx) = column_index.get(&inf_key) {
                infiltration_columns.insert(zone_id, idx);
            }
        }
        let lwr_key = format!("Interior LWR Exchange - {label} (W)");
        if let Some(&idx) = column_index.get(&lwr_key) {
            lwr_columns.insert(zone_id, idx);
        }
        let heat_key = format!("HVAC Heating Delivered - {label} (W)");
        let cool_key = format!("HVAC Cooling Delivered - {label} (W)");
        if let (Some(&heat_idx), Some(&cool_idx)) =
            (column_index.get(&heat_key), column_index.get(&cool_key))
        {
            hvac_columns.insert(zone_id, (heat_idx, cool_idx));
        }
        zone_env_indices.push(env_idx);
        zone_temp_col_indices.push(temp_col);
    }

    ZoneColumnCaches {
        infiltration_columns,
        lwr_columns,
        hvac_columns,
        sorted_zone_ids,
        zone_env_indices,
        zone_temp_col_indices,
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use arrow::datatypes::{DataType, Field, Schema};
    use hares_equipment::{Equipment, apply_identity_write};
    use hares_input::EquipmentSpec;
    use hares_types::control_signal::ControlSignal;
    use hares_types::{
        CoreCapabilities, CoreOutput, EndUse, EnvironmentState, EquipmentDescriptor, EquipmentId,
        FuelType, PortDeclaration, PortSlots, Telemetry, TelemetryField, ZoneId, ZoneState,
        telemetry_keys as tk,
    };
    use serde_json::Map;

    use super::{
        build_output_column_index, build_output_row_bindings, build_zone_column_caches,
        enrich_schema_with_telemetry_units, plan_equipment_columns,
    };
    use crate::columns::{
        CAPACITY_SUFFIX, COP_SUFFIX, ELECTRIC_POWER_SUFFIX, FAN_POWER_SUFFIX, GAS_POWER_SUFFIX,
        HVAC_DUCT_LOSSES_COL, LATENT_GAINS_SUFFIX, MAIN_POWER_SUFFIX, MODE_SUFFIX,
        RUNTIME_FRACTION_SUFFIX, SETPOINT_SUFFIX, SHR_SUFFIX, SPEED_SUFFIX,
    };

    fn equipment_refs(equipment: &[Box<dyn Equipment>]) -> Vec<&dyn Equipment> {
        equipment.iter().map(AsRef::as_ref).collect()
    }

    /// The binding tests' equipment stub: a descriptor (the column map's
    /// whole input) and a telemetry map, stepping nothing.
    struct TestEquipment {
        descriptor: EquipmentDescriptor,
        telemetry: Telemetry,
        core_output: CoreOutput,
        initialized: bool,
    }

    impl TestEquipment {
        fn new(name: &str) -> Self {
            Self {
                descriptor: EquipmentDescriptor {
                    id: EquipmentId(0),
                    name: name.to_string(),
                    end_use: EndUse::OTHER,
                    equipment_type: Cow::Borrowed("TestEquipment"),
                    zone: Some(ZoneId(1)),
                    fuel: FuelType::Electric,
                    stage: hares_types::ExecutionStage::Independent,
                    control_capabilities: hares_types::ControlCapabilities::empty(),
                    core_capabilities: CoreCapabilities::empty(),
                    telemetry_fields: Vec::new(),
                    zone_type: None,
                },
                telemetry: Telemetry::with_capacity(0),
                core_output: CoreOutput::default(),
                initialized: false,
            }
        }
    }

    impl Equipment for TestEquipment {
        fn descriptor(&self) -> &EquipmentDescriptor {
            &self.descriptor
        }

        fn resolved_parameters(&self) -> Vec<hares_types::ResolvedParameter> {
            Vec::new()
        }

        fn rename(&mut self, name: String) {
            self.descriptor.name = name;
        }

        fn set_equipment_id(
            &mut self,
            id: EquipmentId,
        ) -> std::result::Result<(), hares_types::HaresError> {
            apply_identity_write(self.is_initialized(), &mut self.descriptor, id)
        }

        fn ports(&self) -> &[PortDeclaration] {
            &[]
        }

        fn init(
            &mut self,
            _config: &hares_equipment::EquipmentConfig,
            _env: &EnvironmentState,
        ) -> std::result::Result<(), hares_types::HaresError> {
            Ok(())
        }

        fn update_control(&mut self, _env: &EnvironmentState) -> hares_types::OperatingMode {
            hares_types::OperatingMode::Off
        }

        fn step(
            &mut self,
            _env: &EnvironmentState,
            _dt: std::time::Duration,
            _ports: &mut PortSlots,
        ) -> std::result::Result<(), hares_types::HaresError> {
            Ok(())
        }

        fn telemetry(&self) -> &Telemetry {
            &self.telemetry
        }

        fn core_output(&self) -> &CoreOutput {
            &self.core_output
        }

        fn save_state(&self) -> std::result::Result<Vec<u8>, hares_types::HaresError> {
            Ok(vec![])
        }

        fn load_state(
            &mut self,
            _state: &[u8],
        ) -> std::result::Result<(), hares_types::HaresError> {
            Ok(())
        }

        fn apply_signal(
            &mut self,
            _signal: &ControlSignal,
        ) -> std::result::Result<(), hares_types::HaresError> {
            Ok(())
        }

        fn is_initialized(&self) -> bool {
            self.initialized
        }

        fn mark_initialized(&mut self) {
            self.initialized = true;
        }

        fn unmark_initialized(&mut self) {
            self.initialized = false;
        }
    }

    fn spec(name: &str, fuel: FuelType) -> EquipmentSpec {
        EquipmentSpec {
            instance_name: None,
            name: name.to_string(),
            fuel_type: fuel,
            parameters: Map::new(),
            zip_params: None,
            typed_overrides: serde_json::Map::new(),
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
        }
    }

    /// A schema the row plan freezes from that omits a column the schema
    /// KNOWS it owes (membership = the schema's unconditional
    /// `{name} Electric Power (kW)` column) is a typed error at freeze, in
    /// every build. Only schema-unknown equipment (added after the schema
    /// froze, carrying no columns by design) are exempt.
    #[test]
    fn row_plan_rejects_unplaceable_expected_variable() {
        let mut eq = TestEquipment::new("Battery");
        eq.descriptor.end_use = EndUse::BATTERY;
        eq.descriptor.fuel = FuelType::Electric;
        // The storage capability the SOC column's typed gate reads.
        eq.descriptor.core_capabilities |= CoreCapabilities::HAS_SOC;
        let equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        // A schema-known equipment: the schema carries its membership column
        // (Electric Power, emitted unconditionally at verbosity ≥ 1) and its
        // Mode column, but not the SOC column its declared storage
        // capability demands at verbosity 3. Drift for a known equipment
        // must stay loud.
        let schema = Schema::new(vec![
            Field::new("Time", DataType::Utf8, false),
            Field::new("Battery Electric Power (kW)", DataType::Float64, true),
            Field::new("Battery Mode (-)", DataType::Float64, true),
        ]);
        let column_index = build_output_column_index(&schema);
        let err = build_output_row_bindings(
            &schema,
            &column_index,
            &equipment_refs(&equipment),
            &[],
            &[
                "Battery Electric Power (kW)".to_string(),
                "Battery Mode (-)".to_string(),
                "Battery SOC (-)".to_string(),
            ]
            .into_iter()
            .collect(),
        )
        .expect_err("schema drift for a known equipment must be a typed error");
        assert!(
            matches!(&err, hares_types::HaresError::InvariantViolation { check_name, .. }
                if check_name.contains("Battery SOC (-)")
                    && check_name.contains("Battery")),
            "the error must name the missing column and the equipment, got: {err:?}"
        );
    }

    /// An equipment whose name shadows a reserved aggregate column ("Total")
    /// must claim NOTHING from a schema that carries the aggregate —
    /// not even the column that exists under its own name. Claiming it would
    /// make record_step's later per-equipment write silently replace the
    /// dwelling's total; and treating the aggregate's presence as membership
    /// would fail the plan on the absent Mode column. Both faces are pinned
    /// here: no claim, no panic.
    #[test]
    fn column_map_claims_nothing_for_an_equipment_name_shadowing_the_aggregate_column() {
        let eq = TestEquipment::new("Total");
        let equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        // The schema carries only the level-0 aggregate — the exact state a
        // frozen schema presents to a mid-run equipment named "Total".
        let schema = Schema::new(vec![
            Field::new("Time", DataType::Utf8, false),
            Field::new("Total Electric Power (kW)", DataType::Float64, true),
        ]);
        let column_index = build_output_column_index(&schema);
        let bindings = build_output_row_bindings(
            &schema,
            &column_index,
            &equipment_refs(&equipment),
            &[],
            &["Total Electric Power (kW)".to_string()]
                .into_iter()
                .collect(),
        )
        .expect("reserved-aggregate shadowing must not error");
        let cols = &bindings.equipment[0];
        assert!(
            cols.electric_power.is_none(),
            "an equipment named 'Total' must not claim the aggregate \
             'Total Electric Power (kW)' column as its own"
        );
        assert!(
            cols.mode.is_none(),
            "the aggregate's presence must not be read as schema membership — \
             the absent Mode column must not panic"
        );
    }

    /// At verbosity 7, all per-equipment column indices for applicable equipment
    /// types are resolved to `Some`.
    #[test]
    fn plan_equipment_columns_v7_all_columns_resolved_for_applicable_equipment() {
        let specs = vec![
            spec("ASHP Heater", FuelType::Electric),
            spec("Battery", FuelType::Electric),
        ];
        let schema = crate::build_schema_for_specs(&specs, 7, &[]);
        let column_index = build_output_column_index(&schema);

        let mut eq1 = TestEquipment::new("ASHP Heater");
        eq1.descriptor.end_use = EndUse::HVAC_HEATING;
        eq1.descriptor.fuel = FuelType::Electric;
        // The heat-pump-heater family's defining declaration (the
        // defrost/ER columns' typed gate), as the real class declares it.
        eq1.descriptor
            .telemetry_fields
            .push(TelemetryField::power_kw(
                tk::PAN_HEATER_KW,
                "Minisplit pan/crankcase heater power",
            ));
        let mut eq2 = TestEquipment::new("Battery");
        eq2.descriptor.end_use = EndUse::BATTERY;
        eq2.descriptor.fuel = FuelType::Electric;
        // The storage capability the SOC column family's typed gate reads.
        eq2.descriptor.core_capabilities |= CoreCapabilities::HAS_SOC;
        let equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq1), Box::new(eq2)];
        let col_map = plan_equipment_columns(
            &equipment_refs(&equipment),
            &column_index,
            &column_index.keys().cloned().collect(),
        )
        .expect("column map resolves");

        let ashp = &col_map[0];
        let bat = &col_map[1];

        assert!(ashp.electric_power.is_some());
        assert!(ashp.mode.is_some());
        assert!(ashp.setpoint.is_some());
        assert!(ashp.capacity.is_some());
        assert!(ashp.cop.is_some());
        assert!(ashp.energy_kwh.is_some());
        assert!(ashp.schedule.is_some());
        assert!(ashp.defrost_state.is_some());
        assert!(ashp.er_power.is_some());
        assert!(ashp.speed.is_some());
        assert!(ashp.fan_power.is_some());
        assert!(ashp.main_power.is_some());
        assert!(ashp.runtime_fraction.is_some());
        assert!(ashp.shr.is_none());
        assert!(ashp.latent_gains.is_none());

        assert!(bat.electric_power.is_some());
        assert!(bat.soc.is_some());
        assert!(bat.energy_kwh.is_some());
        assert!(bat.mode.is_some());
        assert!(bat.setpoint.is_none());
        assert!(bat.capacity.is_none());
        assert!(bat.cop.is_none());
    }

    /// For every `FuelType` variant, the schema emits a
    /// `"{name} Gas Power (therms/hour)"` column if and only if
    /// `plan_equipment_columns` resolves it. Both sites share
    /// `crate::fuel_reports_gas_power_column`; this test guards against the
    /// predicate being forked again (previously Wood/Coal/WoodPellet equipment
    /// got a schema column that was never populated).
    #[test]
    fn gas_power_schema_column_and_resolved_column_agree_for_every_fuel_type() {
        let all_fuels = hares_types::ports::ALL_FUEL_TYPES
            .iter()
            .copied()
            .chain(std::iter::once(FuelType::None));
        for fuel in all_fuels {
            let name = "Test Load";
            let specs = vec![spec(name, fuel)];
            let schema = crate::build_schema_for_specs(&specs, 1, &[]);
            let column_index = build_output_column_index(&schema);
            let schema_has_gas_col =
                column_index.contains_key(&format!("{name} {GAS_POWER_SUFFIX}"));

            let mut eq = TestEquipment::new(name);
            eq.descriptor.fuel = fuel;
            let equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
            let col_map = plan_equipment_columns(
                &equipment_refs(&equipment),
                &column_index,
                &column_index.keys().cloned().collect(),
            )
            .expect("column map resolves");
            let resolved_gas_col = col_map[0].gas_power.is_some();

            assert_eq!(
                schema_has_gas_col, resolved_gas_col,
                "fuel {fuel:?}: schema column presence ({schema_has_gas_col}) must match \
                 resolved column presence ({resolved_gas_col})"
            );
            assert_eq!(
                schema_has_gas_col,
                crate::fuel_reports_gas_power_column(fuel),
                "fuel {fuel:?}: schema must follow the shared predicate"
            );
        }
    }

    /// At verbosity 0, per-equipment columns are not in the schema.
    #[test]
    fn plan_equipment_columns_v0_all_per_equipment_columns_none() {
        let schema = crate::build_schema_for_specs(&[], 0, &[]);
        let column_index = build_output_column_index(&schema);
        let eq = TestEquipment::new("ASHP Heater");
        let equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        let col_map = plan_equipment_columns(
            &equipment_refs(&equipment),
            &column_index,
            &column_index.keys().cloned().collect(),
        )
        .expect("column map resolves");
        let cols = &col_map[0];
        assert!(cols.electric_power.is_none());
        assert!(cols.gas_power.is_none());
        assert!(cols.mode.is_none());
        assert!(cols.soc.is_none());
        assert!(cols.capacity.is_none());
        assert!(cols.energy_kwh.is_none());
        assert!(cols.schedule.is_none());
        assert!(cols.defrost_state.is_none());
        assert!(cols.duct_losses.is_none());
    }

    /// Gas-fueled equipment with electric parasitics (gas furnace blower at
    /// pf 0.87, gas boiler pump at 0.84, gas WH draft fan at 0.87) emits
    /// reactive power, so at verbosity ≥ 5 the Reactive Power / Power Factor
    /// columns must exist and resolve for gas equipment too — the columns
    /// mirror the unconditional Electric Power column instead of gating on
    /// fuel type.
    #[test]
    fn gas_equipment_reactive_and_power_factor_columns_resolve_at_verbosity_5() {
        let specs = vec![spec("Gas Furnace", FuelType::Gas)];
        let schema = crate::build_schema_for_specs(&specs, 5, &[]);
        let column_index = build_output_column_index(&schema);
        let mut eq = TestEquipment::new("Gas Furnace");
        eq.descriptor.end_use = EndUse::HVAC_HEATING;
        eq.descriptor.fuel = FuelType::Gas;
        let equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        let col_map = plan_equipment_columns(
            &equipment_refs(&equipment),
            &column_index,
            &column_index.keys().cloned().collect(),
        )
        .expect("column map resolves");
        let cols = &col_map[0];
        assert!(
            cols.reactive_power.is_some(),
            "gas furnace blower emits Q — reactive column must resolve"
        );
        assert!(
            cols.power_factor.is_some(),
            "gas furnace blower emits Q — power factor column must resolve"
        );
        // Below verbosity 5 the columns are absent for all equipment.
        let schema_v4 = crate::build_schema_for_specs(&specs, 4, &[]);
        let column_index_v4 = build_output_column_index(&schema_v4);
        let mut eq_v4 = TestEquipment::new("Gas Furnace");
        eq_v4.descriptor.end_use = EndUse::HVAC_HEATING;
        eq_v4.descriptor.fuel = FuelType::Gas;
        let equipment_v4: Vec<Box<dyn Equipment>> = vec![Box::new(eq_v4)];
        let col_map_v4 = plan_equipment_columns(
            &equipment_refs(&equipment_v4),
            &column_index_v4,
            &column_index_v4.keys().cloned().collect(),
        )
        .expect("column map resolves");
        assert!(col_map_v4[0].reactive_power.is_none());
        assert!(col_map_v4[0].power_factor.is_none());
    }

    /// A typo in a column name string results in `None` index.
    #[test]
    fn column_name_typo_results_in_none_column_index() {
        let specs = vec![spec("ASHP Heater", FuelType::Electric)];
        let schema = crate::build_schema_for_specs(&specs, 1, &[]);
        let column_index = build_output_column_index(&schema);
        assert!(column_index.contains_key("ASHP Heater Electric Power (kW)"));
        assert!(!column_index.contains_key("ASHP Heater Electrix Power (kW)"));
    }

    /// Column suffix constants produce names matching the schema.
    #[test]
    fn column_suffix_constants_match_build_schema_output() {
        let specs = vec![spec("Air Conditioner", FuelType::Electric)];
        let schema = crate::build_schema_for_specs(&specs, 7, &[]);
        let column_index = build_output_column_index(&schema);
        let name = "Air Conditioner";
        assert!(column_index.contains_key(&format!("{name} {ELECTRIC_POWER_SUFFIX}")));
        assert!(column_index.contains_key(&format!("{name} {MODE_SUFFIX}")));
        assert!(column_index.contains_key(&format!("{name} {SETPOINT_SUFFIX}")));
        assert!(column_index.contains_key(&format!("{name} {CAPACITY_SUFFIX}")));
        assert!(column_index.contains_key(&format!("{name} {COP_SUFFIX}")));
        assert!(column_index.contains_key(&format!("{name} {SHR_SUFFIX}")));
        assert!(column_index.contains_key(&format!("{name} {LATENT_GAINS_SUFFIX}")));
        assert!(column_index.contains_key(&format!("{name} {SPEED_SUFFIX}")));
        assert!(column_index.contains_key(&format!("{name} {FAN_POWER_SUFFIX}")));
        assert!(column_index.contains_key(&format!("{name} {MAIN_POWER_SUFFIX}")));
        assert!(column_index.contains_key(&format!("{name} {RUNTIME_FRACTION_SUFFIX}")));
    }

    /// `HVAC_DUCT_LOSSES_COL` constant resolves in the schema at v5+.
    #[test]
    fn duct_losses_column_uses_shared_constant() {
        let schema_v5 = crate::build_schema_for_specs(&[], 5, &[]);
        let col_idx_v5 = build_output_column_index(&schema_v5);
        assert!(col_idx_v5.contains_key(HVAC_DUCT_LOSSES_COL));
        let schema_v4 = crate::build_schema_for_specs(&[], 4, &[]);
        let col_idx_v4 = build_output_column_index(&schema_v4);
        assert!(!col_idx_v4.contains_key(HVAC_DUCT_LOSSES_COL));
    }

    /// Every output-scoped telemetry key in `OUTPUT_SCOPE_KEYS` has a
    /// corresponding column mapping in the output schema when the
    /// relevant equipment type is present at max verbosity (8).
    ///
    /// This is the integration-level counterpart to the unit tests in
    /// `telemetry_keys.rs` — those verify every key constant has a scope
    /// annotation; this one verifies the scope annotations are backed
    /// by actual column definitions.
    #[test]
    fn all_output_scope_keys_have_column_mappings_at_max_verbosity() {
        use hares_types::telemetry_keys as tk;

        let specs: Vec<EquipmentSpec> = vec![
            // HVAC with compressor (heat pump)
            EquipmentSpec {
                instance_name: Some("ASHP Heater".to_string()),
                ..spec("ASHP Heater", FuelType::Electric)
            },
            // PV
            EquipmentSpec {
                instance_name: Some("PV".to_string()),
                ..spec("PV", FuelType::Electric)
            },
            // EV
            EquipmentSpec {
                instance_name: Some("EV".to_string()),
                ..spec("EV", FuelType::Electric)
            },
        ];
        let schema = crate::build_schema_for_specs(&specs, 8, &[]);
        let column_names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();

        // Build the expected column set from the schema and the
        // equipment column map logic. We can't call plan_equipment_columns
        // directly because it requires Box<dyn Equipment> instances, so we
        // verify against the column names in the schema instead.
        //
        // For each output-scoped key, check that the schema contains at
        // least one column whose suffix matches the key's column suffix.
        // The schema should have per-equipment columns for HVAC, PV, and EV.

        // Per-HVAC columns (including heat pump and compressor):
        assert!(
            column_names.contains(&"ASHP Heater Supply Temperature (C)"),
            "missing HVAC supply temp column; got: {column_names:?}"
        );
        assert!(
            column_names.contains(&"ASHP Heater Return Temperature (C)"),
            "missing HVAC return temp column"
        );
        assert!(
            column_names.contains(&"ASHP Heater Compressor Power (W)"),
            "missing compressor power (W) column"
        );
        assert!(
            column_names.contains(&"ASHP Heater Compressor Power (kW)"),
            "missing compressor power (kW) column"
        );
        assert!(
            column_names.contains(&"ASHP Heater Fan Electric Power (W)"),
            "missing fan electric power column"
        );
        assert!(
            column_names.contains(&"ASHP Heater Fan Power (W)"),
            "missing fan power (W) column"
        );
        assert!(
            column_names.contains(&"ASHP Heater Supply Air Temperature (C)"),
            "missing HP supply air temp column"
        );
        assert!(
            column_names.contains(&"ASHP Heater Pan Heater Power (kW)"),
            "missing pan heater power column"
        );
        assert!(
            column_names.contains(&"ASHP Heater Heat Pump Capacity (W)"),
            "missing HP capacity column"
        );
        assert!(
            column_names.contains(&"ASHP Heater ER Capacity (W)"),
            "missing ER capacity column"
        );

        // Per-PV columns:
        assert!(
            column_names.contains(&"PV DC Power (kW)"),
            "missing PV DC power column"
        );
        assert!(
            column_names.contains(&"PV Irradiance (W/m2)"),
            "missing PV irradiance column"
        );

        // Per-EV columns:
        assert!(
            column_names.contains(&"EV Connection State (-)"),
            "missing EV connection state column"
        );
        assert!(
            column_names.contains(&"EV Charging Level (-)"),
            "missing EV charging level column"
        );

        // Setpoint chain columns (dwelling-level, at v7):
        assert!(
            column_names.contains(&"Scheduled Heating Setpoint (C)"),
            "missing scheduled heating setpoint column"
        );
        assert!(
            column_names.contains(&"Scheduled Cooling Setpoint (C)"),
            "missing scheduled cooling setpoint column"
        );
        assert!(
            column_names.contains(&"Runtime Heating Setpoint (C)"),
            "missing runtime heating setpoint column"
        );
        assert!(
            column_names.contains(&"Runtime Cooling Setpoint (C)"),
            "missing runtime cooling setpoint column"
        );

        // Thermostat short-cycle protection columns (per-equipment):
        assert!(
            column_names.contains(&"ASHP Heater Min On Time (s)"),
            "missing min on time column"
        );
        assert!(
            column_names.contains(&"ASHP Heater Min Off Time (s)"),
            "missing min off time column"
        );

        // Verify all 27 output-scope keys are accounted for.
        // 7 legacy (FAN_KW, SHR, DEFROST_CYCLE_STATE, BACKUP_ER_KW,
        //   RUNTIME_FRACTION, LATENT_GAINS_W, DUCT_LOSS_W)
        // + 4 setpoint chain
        // + 3 HVAC temps
        // + 4 compressor/fan detail
        // + 3 HP detail
        // + 2 PV
        // + 2 EV
        // + 2 thermostat short-cycle protection
        // = 27 total keys
        //
        // 7 legacy output keys are verified by existing tests
        // (verbosity_7 tests for fan, SHR, defrost, ER, RTF, latent, duct).
        // The 20 new keys are verified above.
        assert_eq!(
            tk::OUTPUT_SCOPE_KEYS.len(),
            27,
            "OUTPUT_SCOPE_KEYS length changed; ensure all are tested above"
        );
    }

    /// A catalogue built from live equipment descriptors, for the enrich
    /// tests.
    fn catalogue_from_equipment(equipment: &[Box<dyn Equipment>]) -> crate::Catalogue {
        let input = crate::CatalogueInput {
            equipment: equipment_refs(equipment)
                .iter()
                .map(|eq| {
                    let d = eq.descriptor();
                    crate::CatalogueEquipment {
                        name: d.name.clone(),
                        instance_name: None,
                        id: d.id,
                        fuel: d.fuel,
                        end_use: d.end_use.clone(),
                        core_capabilities: d.core_capabilities,
                        telemetry: d.telemetry_fields.clone(),
                    }
                })
                .collect(),
            zones: Vec::new(),
            actors: Vec::new(),
        };
        crate::build_catalogue(&input).expect("the test catalogue builds")
    }

    #[test]
    fn enrich_schema_telemetry_units_tags_matching_column_with_unit_source() {
        let mut eq = TestEquipment::new("TestEq");
        eq.descriptor
            .telemetry_fields
            .push(TelemetryField::power_kvar(
                "test_reactive",
                "test reactive power",
            ));
        let equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];

        let catalogue = catalogue_from_equipment(&equipment);
        let schema = crate::build_schema(&catalogue, 5, false);
        let enriched =
            enrich_schema_with_telemetry_units(schema, &equipment_refs(&equipment), &catalogue);
        let reactive_col = enriched
            .fields()
            .iter()
            .find(|f| f.name() == "TestEq Reactive Power (kVAR)")
            .expect("schema v5 must include per-equipment reactive power column");
        assert_eq!(
            reactive_col
                .metadata()
                .get("unit_source")
                .map(|s| s.as_str()),
            Some("telemetry_field"),
            "reactive power column with matching declared telemetry field unit should carry unit_source"
        );
    }

    #[test]
    fn enrich_schema_telemetry_units_sets_no_unit_source_when_unit_absent_from_descriptor() {
        let mut eq = TestEquipment::new("TestEq");
        eq.descriptor
            .telemetry_fields
            .push(TelemetryField::power_kw("test_kw", "test active power"));
        let equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];

        let catalogue = catalogue_from_equipment(&equipment);
        let schema = crate::build_schema(&catalogue, 5, false);
        let enriched =
            enrich_schema_with_telemetry_units(schema, &equipment_refs(&equipment), &catalogue);
        let reactive_col = enriched
            .fields()
            .iter()
            .find(|f| f.name() == "TestEq Reactive Power (kVAR)")
            .expect("schema v5 must include per-equipment reactive power column");
        assert!(
            reactive_col.metadata().get("unit_source").is_none(),
            "reactive power column should not get unit_source when equipment declares no kVAR field"
        );
    }

    /// The enrichment's per-field check is unmasked: a column whose
    /// telemetry key the owning equipment declares with a DIFFERENT unit
    /// must not validate on the equipment's other declared units (the
    /// whole-set membership that let the EV columns' dimensionless metadata
    /// pass against `code` declarations). The agreeing field validates; the
    /// disagreeing one warns and leaves the column unannotated.
    #[test]
    fn enrich_unit_source_follows_the_key_field_not_the_unit_set() {
        // The EV name puts the name family's telemetry-derived columns in
        // the catalogue; the specs path carries no equipment declarations,
        // so the columns hold the static per-capability table's `code`
        // declaration.
        let specs = [spec("EV", FuelType::Electric)];
        let zones = vec![(ZoneId(1), "Indoor".to_string())];
        let input = crate::CatalogueInput {
            equipment: specs
                .iter()
                .enumerate()
                .map(|(i, s)| crate::CatalogueEquipment {
                    name: s.name.clone(),
                    instance_name: None,
                    id: EquipmentId(u32::try_from(i + 1).expect("equipment count")),
                    fuel: s.fuel_type,
                    end_use: hares_equipment::class_declared_surface(&s.name)
                        .map_or(hares_types::EndUse::OTHER, |surface| surface.end_use),
                    core_capabilities: hares_equipment::class_declared_surface(&s.name)
                        .map_or(hares_types::CoreCapabilities::default(), |surface| {
                            surface.core_capabilities
                        }),
                    telemetry: Vec::new(),
                })
                .collect(),
            zones,
            actors: Vec::new(),
        };
        let catalogue = crate::build_catalogue(&input).expect("the specs catalogue builds");
        let schema = crate::build_schema(&catalogue, 8, false);

        let make = |unit_declaration: TelemetryField| -> Vec<Box<dyn Equipment>> {
            make_multiple(vec![unit_declaration])
        };

        // The agreeing declaration validates the column.
        let agreeing = make(TelemetryField::code_code(
            "connection_state",
            hares_types::EV_CONNECTION_STATE_LABELS,
            "the declaration the column mirrors",
        ));
        let enriched = enrich_schema_with_telemetry_units(
            schema.clone(),
            &equipment_refs(&agreeing),
            &catalogue,
        );
        let col = enriched
            .fields()
            .iter()
            .find(|f| f.name() == "EV Connection State (-)")
            .expect("the schema carries the EV column at verbosity 8");
        assert_eq!(
            col.metadata().get("unit_source").map(String::as_str),
            Some("telemetry_field"),
            "the declared field for the column's key agrees and validates it"
        );

        // The disagreeing declaration must not validate on the unit SET:
        // the equipment also declares an unrelated code-unit field, so the
        // whole-set membership finds the column's unit and would annotate
        // the column despite the key field's disagreement (the trap the EV
        // columns' dimensionless metadata fell into).
        let disagreeing = make_multiple(vec![
            TelemetryField::ratio(
                "connection_state",
                "drifts from the column's code declaration",
            ),
            TelemetryField::code_code(
                "phase",
                hares_types::EV_CHARGING_LEVEL_LABELS,
                "an unrelated code field puts code in the unit set",
            ),
        ]);
        let enriched =
            enrich_schema_with_telemetry_units(schema, &equipment_refs(&disagreeing), &catalogue);
        let col = enriched
            .fields()
            .iter()
            .find(|f| f.name() == "EV Connection State (-)")
            .expect("the schema carries the EV column at verbosity 8");
        assert!(
            col.metadata().get("unit_source").is_none(),
            "a per-field unit disagreement must leave the column unvalidated, whatever the \
             equipment's other declared units are"
        );
    }

    fn make_multiple(declarations: Vec<TelemetryField>) -> Vec<Box<dyn Equipment>> {
        let mut eq = TestEquipment::new("EV");
        eq.descriptor.telemetry_fields = declarations;
        vec![Box::new(eq)]
    }

    /// Zone temperature columns map by the zone's TYPED role, not its id:
    /// the attic's label comes from its `ZoneType`, so an attic with a high
    /// id reads the attic temperature column, not a `Zone_{id}` column.
    #[test]
    fn attic_temperature_column_uses_zone_type_not_zone_id() {
        use hares_input::hpxml::ZoneType;

        let zones = vec![
            ZoneState {
                id: ZoneId(10),
                temperature_c: 21.0,
                humidity_ratio: 0.008,
                volume_m3: 200.0,
            },
            ZoneState {
                id: ZoneId(3),
                temperature_c: 16.0,
                humidity_ratio: 0.008,
                volume_m3: 120.0,
            },
        ];
        let zone_types = vec![ZoneType::Conditioned, ZoneType::Attic];
        let schema = crate::build_schema_for_specs(
            &[],
            2,
            &[
                (ZoneId(10), "Indoor".to_string()),
                (ZoneId(3), "Attic".to_string()),
            ],
        );
        let column_index = build_output_column_index(&schema);
        let caches = build_zone_column_caches(&zones, &zone_types, ZoneId(10), &column_index);

        let attic_idx = column_index
            .get("Temperature - Attic (C)")
            .copied()
            .expect("schema must include attic temperature column");
        assert_eq!(
            caches.zone_temp_col_indices,
            vec![
                Some(attic_idx),
                Some(column_index["Temperature - Indoor (C)"])
            ],
            "zone temperatures should be mapped by zone type, not zone number"
        );
    }

    /// The per-zone HVAC attribution columns bind to the thermal port
    /// accumulators' per-category values: the heat and cool slots differ,
    /// and the ports' accumulated values read back per category unchanged
    /// (the exact reads `record_step`'s per-zone write makes).
    #[test]
    fn per_zone_hvac_columns_written_from_port_accumulators() {
        use hares_input::hpxml::ZoneType;
        use hares_types::ThermalCategory;
        use hares_types::ports::PortContribution;

        let zones = vec![ZoneState {
            id: ZoneId(1),
            temperature_c: 21.0,
            humidity_ratio: 0.008,
            volume_m3: 200.0,
        }];
        let zone_types = vec![ZoneType::Conditioned];
        let schema = crate::build_schema_for_specs(&[], 6, &[(ZoneId(1), "Indoor".to_string())]);
        let column_index = build_output_column_index(&schema);
        let caches = build_zone_column_caches(&zones, &zone_types, ZoneId(1), &column_index);

        let (heat_idx, cool_idx) = caches
            .hvac_columns
            .get(&ZoneId(1))
            .copied()
            .expect("Indoor zone must have HVAC column indices at verbosity 6");
        assert_ne!(
            heat_idx, cool_idx,
            "heating and cooling column indices must differ"
        );

        // Simulate equipment writing HvacHeating and HvacCooling to ports.
        let mut ports = PortSlots::from_declarations(&[PortDeclaration::thermal(ZoneId(1))]);
        ports
            .accumulate(&PortContribution::Thermal {
                zone: ZoneId(1),
                sensible_gain_w: 500.0,
                radiant_gain_w: 0.0,
                latent_gain_w: 0.0,
                category: ThermalCategory::HvacHeating,
            })
            .expect("accumulate must succeed");
        ports
            .accumulate(&PortContribution::Thermal {
                zone: ZoneId(1),
                sensible_gain_w: -200.0,
                radiant_gain_w: 0.0,
                latent_gain_w: 0.0,
                category: ThermalCategory::HvacCooling,
            })
            .expect("accumulate must succeed");

        let indoor_heating = ports.thermal[0].sensible_for_category(ThermalCategory::HvacHeating);
        let indoor_cooling = ports.thermal[0].sensible_for_category(ThermalCategory::HvacCooling);
        assert!(
            (indoor_heating - 500.0).abs() < 1e-9,
            "expected 500.0 W heating, got {indoor_heating}"
        );
        assert!(
            (indoor_cooling - (-200.0)).abs() < 1e-9,
            "expected -200.0 W cooling, got {indoor_cooling}"
        );

        // Build a minimal record context and write to scratch.
        let mut record_scratch = vec![0.0; column_index.len()];
        record_scratch[heat_idx] = indoor_heating;
        record_scratch[cool_idx] = indoor_cooling.abs();
        assert!((record_scratch[heat_idx] - 500.0).abs() < 1e-9);
        assert!((record_scratch[cool_idx] - 200.0).abs() < 1e-9);
    }

    /// The name-to-column index the row plan's freeze resolves against is
    /// built from the schema's value columns alone: the timestamp column
    /// leads and is excluded, so slot 0 is the first value column.
    #[test]
    fn column_index_skips_the_timestamp_column() {
        let schema = Schema::new(vec![
            Field::new("Time", DataType::Utf8, false),
            Field::new("Total Electric Power (kW)", DataType::Float64, true),
        ]);
        let column_index = build_output_column_index(&schema);
        assert_eq!(
            column_index.get("Total Electric Power (kW)"),
            Some(&0),
            "the first value column's slot is 0: recorder rows exclude the timestamp"
        );
        assert!(!column_index.contains_key("Time"));
    }

    /// The actor column map's per-actor entry exists even for an actor
    /// without telemetry: `record_step` zips the map against the actor
    /// vector positionally, so a missing entry would shift every later
    /// actor's columns onto their neighbours.
    #[test]
    fn actor_column_map_keeps_one_entry_per_actor_in_order() {
        let mut tel_actor = TestEquipment::new("ignored");
        tel_actor.telemetry.insert("away", 1.0);
        let schema = Schema::new(vec![
            Field::new("Time", DataType::Utf8, false),
            Field::new("actor:Occupant:away", DataType::Float64, true),
        ]);
        let column_index = build_output_column_index(&schema);
        let actors: Vec<(&str, Option<&Telemetry>)> = vec![
            ("ActorWithoutTelemetry", None),
            ("Occupant", Some(&tel_actor.telemetry)),
        ];
        let map = super::build_actor_column_map(&actors, &column_index);
        assert_eq!(map.len(), 2, "one entry per actor, in actor order");
        assert!(
            map[0].is_empty(),
            "an actor without telemetry binds nothing"
        );
        assert_eq!(
            map[1],
            vec![("away".to_string(), 0)],
            "the telemetry key resolves to its schema column"
        );
    }

    /// Diagnostics zone column names parse by zone id and kind, and a name
    /// that is not a diagnostics zone column parses to `None` (the schema's
    /// other marked fields carry their own slots).
    #[test]
    fn zone_diag_name_parses_id_and_kind() {
        use super::ZoneDiagKind;
        assert_eq!(
            super::zone_diag_name("zone2_temp_c"),
            Some((2, ZoneDiagKind::Temp))
        );
        assert_eq!(
            super::zone_diag_name("zone12_thermal_gain_w"),
            Some((12, ZoneDiagKind::Sensible))
        );
        assert_eq!(
            super::zone_diag_name("zone3_latent_gain_w"),
            Some((3, ZoneDiagKind::Latent))
        );
        assert_eq!(super::zone_diag_name("zone_x_temp_c"), None);
        assert_eq!(super::zone_diag_name("electrical_net_w"), None);
    }

    /// The zone diag slots' zone ids come from the column names, in id
    /// order regardless of the schema's field order.
    #[test]
    fn diagnostics_bindings_collect_zones_in_id_order() {
        let schema = Schema::new(vec![
            Field::new("Time", DataType::Utf8, false),
            Field::new("step", DataType::Float64, true).with_metadata(
                [(
                    crate::DIAGNOSTICS_METADATA_KEY.to_string(),
                    "diagnostics".to_string(),
                )]
                .into_iter()
                .collect(),
            ),
            Field::new("zone7_temp_c", DataType::Float64, true).with_metadata(
                [(
                    crate::DIAGNOSTICS_METADATA_KEY.to_string(),
                    "diagnostics".to_string(),
                )]
                .into_iter()
                .collect(),
            ),
            Field::new("zone2_temp_c", DataType::Float64, true).with_metadata(
                [(
                    crate::DIAGNOSTICS_METADATA_KEY.to_string(),
                    "diagnostics".to_string(),
                )]
                .into_iter()
                .collect(),
            ),
        ]);
        let bindings = super::build_diagnostics_bindings(&schema).expect("the block builds");
        assert_eq!(bindings.step, 0);
        assert_eq!(
            bindings.zones.iter().map(|z| z.zone).collect::<Vec<_>>(),
            vec![ZoneId(2), ZoneId(7)],
            "the zones land in id order, the sparse id gaps closed"
        );
    }

    /// A schema with no diagnostics-marked field builds no bindings: the
    /// per-step gate stays shut and the step reads and writes nothing for
    /// the diagnostics.
    #[test]
    fn diagnostics_bindings_absent_without_the_marker() {
        let schema = Schema::new(vec![
            Field::new("Time", DataType::Utf8, false),
            Field::new("step", DataType::Float64, true),
        ]);
        assert!(super::build_diagnostics_bindings(&schema).is_none());
    }

    /// A telemetry-keyed column whose owning equipment never claimed the
    /// channel keeps its catalogue-derived metadata unannotated and does
    /// not warn (the per-capability column exists for the name family; the
    /// owner's declaration set is the per-field check's scope).
    #[test]
    fn enrich_schema_unclaimed_keyed_column_stays_unannotated() {
        // A descriptor typed as the EV end use that declares NO telemetry
        // fields: the EV name family's per-capability columns exist in the
        // catalogue (the column family follows the typed end use), the
        // owner never claimed any channel.
        let mut eq = TestEquipment::new("EV");
        eq.descriptor.end_use = EndUse::EV;
        let equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        let catalogue = catalogue_from_equipment(&equipment);
        let schema = crate::build_schema(&catalogue, 8, false);
        let enriched =
            enrich_schema_with_telemetry_units(schema, &equipment_refs(&equipment), &catalogue);
        let connection = enriched
            .fields()
            .iter()
            .find(|f| f.name() == "EV Connection State (-)")
            .expect("the schema carries the EV column at verbosity 8");
        assert_eq!(
            connection.metadata().get("unit_source"),
            None,
            "an unclaimed keyed column stays unannotated"
        );
    }

    /// `EquipmentColumnFamilies::of_descriptor` is the single family gate
    /// the column plan resolves through: the HVAC-or-water-heating family
    /// follows the typed end use, the storage family the typed capability,
    /// and the heat-pump-heater family the pan-heater declaration within
    /// the HVAC heating end use.
    #[test]
    fn equipment_column_families_follow_the_descriptor() {
        use crate::EquipmentColumnFamilies;

        let mut eq = TestEquipment::new("ASHP Heater");
        eq.descriptor.end_use = EndUse::HVAC_HEATING;
        let families = EquipmentColumnFamilies::of_descriptor(&eq.descriptor);
        assert!(
            families.is_hvac,
            "the typed HVAC heating end use is in the family"
        );
        assert!(!families.is_cooling);
        assert!(!families.has_gas);
        assert!(!families.has_soc);
        assert!(
            !families.is_hp_heater,
            "no pan-heater declaration, no heat-pump-heater family"
        );

        eq.descriptor
            .telemetry_fields
            .push(TelemetryField::power_kw(
                tk::PAN_HEATER_KW,
                "Minisplit pan/crankcase heater power",
            ));
        let families = EquipmentColumnFamilies::of_descriptor(&eq.descriptor);
        assert!(
            families.is_hp_heater,
            "the pan-heater declaration within the HVAC heating end use defines the family"
        );

        eq.descriptor.core_capabilities |= CoreCapabilities::HAS_SOC;
        let families = EquipmentColumnFamilies::of_descriptor(&eq.descriptor);
        assert!(
            families.has_soc,
            "the storage gate reads the typed capability"
        );
    }
}
