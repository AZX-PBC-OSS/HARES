//! The frozen observation plan: every RL observation field resolved once,
//! read every step into a caller-owned buffer.
//!
//! `DwellingTelemetry::to_observation_vec` resolved field names with
//! string parsing, two name-index maps and per-call allocations on every
//! step of an RL episode; the field list is fixed for the life of the
//! environment, so all of that is repeatable work. [`ObservationPlan`]
//! resolves every field once, at plan build, to a typed source (an
//! outdoor scalar, a zone or equipment index, a time feature, an actor
//! channel), failing there on an unknown field and naming it. The
//! executors — [`DwellingTelemetry::observe_into`] on a telemetry
//! snapshot and `Dwelling::observe_into` (in `dwelling`) on the live
//! dwelling — write the values into a caller-owned slice with no
//! allocation, the values and NaN handling `to_observation_vec` has
//! always reported, which is now a thin wrapper over the same plan.

use std::collections::{BTreeMap, HashMap};

use hares_types::{HaresError, ZoneId, normalize_ascii};

use crate::telemetry::{DwellingTelemetry, ObservationWarnOnce};

/// One resolved observation field.
pub(crate) struct ObservationSource {
    /// The field's own spelling, for the executor's error messages.
    pub(crate) field: String,
    pub(crate) kind: ObservationKind,
}

/// Which setpoint column a zone field reads.
pub(crate) enum SetpointSide {
    Heat,
    Cool,
}

/// Where one field's value comes from, resolved at plan build.
pub(crate) enum ObservationKind {
    OutdoorTemp,
    OutdoorHumidityRatio,
    TotalPowerKw,
    ReactivePowerKvar,
    /// The zone at `position` in the sorted-zone-id order; `id` is the
    /// zone's own id, so the dwelling executor finds it without sorting.
    ZoneTemperature {
        id: ZoneId,
        position: usize,
    },
    SetpointHeat {
        id: ZoneId,
        position: usize,
    },
    SetpointCool {
        id: ZoneId,
        position: usize,
    },
    ZoneEnergyBalance {
        id: ZoneId,
        position: usize,
    },
    EquipmentSoc {
        index: usize,
    },
    EquipmentPower {
        index: usize,
    },
    TimeSin,
    TimeCos,
    /// `<actor>.<channel>`: the actor found by name, the channel in its
    /// published telemetry.
    ActorDot {
        actor: String,
        channel: String,
    },
    /// A bare name resolved against the actors in sorted-name order.
    ActorBare {
        channel: String,
    },
}

/// The names a plan resolves against: the zone names in the sorted-zone-id
/// order the telemetry reports (each with its id), the equipment names in
/// dwelling order, and the actors' published channels in the sorted-name
/// order the telemetry map reports.
pub(crate) struct ObservationSurface {
    pub zone_names: Vec<(String, ZoneId)>,
    pub equipment_names: Vec<String>,
    pub actors: Vec<(String, Vec<String>)>,
}

/// The frozen per-step observation layout: every field's typed source,
/// resolved once.
pub struct ObservationPlan {
    sources: Vec<ObservationSource>,
    /// The actor names in the sorted-name order the bare-name fallback
    /// searches.
    actor_order: Vec<String>,
}

impl ObservationPlan {
    /// Resolves every field against a telemetry snapshot's own names: the
    /// zone vectors are already in the sorted-zone-id order, and the zone
    /// ids play no part in the snapshot executor.
    pub(crate) fn from_telemetry(
        telemetry: &DwellingTelemetry,
        fields: &[&str],
        warned: &mut ObservationWarnOnce,
    ) -> Result<Self, HaresError> {
        let surface = telemetry.observation_surface();
        Self::from_surface(&surface, fields, warned)
    }

    /// Resolves every field against the surface, failing on the first
    /// unknown field and naming it. The resolution order and error
    /// messages are `to_observation_vec`'s, so a field the plan rejects,
    /// today's string walk rejects too, and vice versa. `warned` is the
    /// run's own [`ObservationWarnOnce`] state: the actor-telemetry
    /// fallback resolutions consume it, so each warns once per run.
    pub(crate) fn from_surface(
        surface: &ObservationSurface,
        fields: &[&str],
        warned: &mut ObservationWarnOnce,
    ) -> Result<Self, HaresError> {
        let zone_positions: HashMap<String, usize> = surface
            .zone_names
            .iter()
            .enumerate()
            .map(|(idx, (name, _))| (normalize_ascii(name), idx))
            .collect();
        let equip_positions: HashMap<String, usize> = surface
            .equipment_names
            .iter()
            .enumerate()
            .map(|(idx, name)| (normalize_ascii(name), idx))
            .collect();
        let actor_channels: BTreeMap<&str, &Vec<String>> = surface
            .actors
            .iter()
            .map(|(name, channels)| (name.as_str(), channels))
            .collect();

        let mut sources = Vec::with_capacity(fields.len());
        for &field in fields {
            let kind = resolve_field(
                field,
                surface,
                &zone_positions,
                &equip_positions,
                &actor_channels,
                warned,
            )?;
            sources.push(ObservationSource {
                field: field.to_string(),
                kind,
            });
        }
        let actor_order = surface
            .actors
            .iter()
            .map(|(name, _)| name.clone())
            .collect();
        Ok(Self {
            sources,
            actor_order,
        })
    }

    pub(crate) fn sources(&self) -> &[ObservationSource] {
        &self.sources
    }

    pub(crate) fn actor_order(&self) -> &[String] {
        &self.actor_order
    }

    /// The field names the plan was built from, in plan order.
    #[must_use]
    pub fn field_names(&self) -> Vec<String> {
        self.sources
            .iter()
            .map(|source| source.field.clone())
            .collect()
    }

    /// The number of fields the plan fills.
    #[must_use]
    pub fn len(&self) -> usize {
        self.sources.len()
    }

    /// Whether the plan fills nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }
}

/// Resolves one field, in `to_observation_vec`'s order.
fn resolve_field(
    field: &str,
    surface: &ObservationSurface,
    zone_positions: &HashMap<String, usize>,
    equip_positions: &HashMap<String, usize>,
    actor_channels: &BTreeMap<&str, &Vec<String>>,
    warned: &mut ObservationWarnOnce,
) -> Result<ObservationKind, HaresError> {
    if field == "outdoor_temp" {
        return Ok(ObservationKind::OutdoorTemp);
    }
    if field == "outdoor_humidity_ratio" {
        return Ok(ObservationKind::OutdoorHumidityRatio);
    }
    if field == "total_electric_kw" || field == "total_power_w" {
        return Ok(ObservationKind::TotalPowerKw);
    }
    if field == "reactive_power_var" {
        return Ok(ObservationKind::ReactivePowerKvar);
    }

    let zone_bracket = |prefix: &str| -> Option<Result<(ZoneId, usize), HaresError>> {
        bracket_name(field, prefix).map(|name| {
            zone_position(&normalize_ascii(name), zone_positions, field)
                .map(|position| (surface.zone_names[position].1, position))
        })
    };
    if let Some(name) = bracket_name(field, "zone_temp") {
        let position = zone_position(&normalize_ascii(name), zone_positions, field)?;
        return Ok(ObservationKind::ZoneTemperature {
            id: surface.zone_names[position].1,
            position,
        });
    }
    if let Some(resolved) = zone_bracket("setpoint_heat") {
        return resolved.map(|(id, position)| ObservationKind::SetpointHeat { id, position });
    }
    if let Some(resolved) = zone_bracket("setpoint_cool") {
        return resolved.map(|(id, position)| ObservationKind::SetpointCool { id, position });
    }
    if let Some(resolved) = zone_bracket("zone_energy_balance") {
        return resolved.map(|(id, position)| ObservationKind::ZoneEnergyBalance { id, position });
    }
    if let Some(name) = bracket_name(field, "equipment_soc") {
        return equip_position(&normalize_ascii(name), equip_positions, field)
            .map(|index| ObservationKind::EquipmentSoc { index });
    }
    if let Some(name) = bracket_name(field, "equipment_power") {
        return equip_position(&normalize_ascii(name), equip_positions, field)
            .map(|index| ObservationKind::EquipmentPower { index });
    }

    // Flat aliases used in architecture docs for single-instance cases.
    if field == "battery_soc" {
        return single_instance_alias(equip_positions, "Battery", field)
            .map(|index| ObservationKind::EquipmentSoc { index });
    }
    if field == "ev_soc" {
        return single_instance_alias(equip_positions, "Electric Vehicle", field)
            .map(|index| ObservationKind::EquipmentSoc { index });
    }

    // Temporal encoding: sin/cos of fractional hour for diurnal RL patterns.
    if field == "time_sin" {
        return Ok(ObservationKind::TimeSin);
    }
    if field == "time_cos" {
        return Ok(ObservationKind::TimeCos);
    }

    // Actor telemetry fallback: dot-separated <actor>.<channel>, then the
    // bare-name search in sorted-name order. A dot field whose channel the
    // actor does not publish falls through to the bare search of the whole
    // field, exactly as the string walk did.
    if let Some((actor_name, channel_name)) = field.split_once('.')
        && let Some(channels) = actor_channels.get(actor_name)
        && channels.iter().any(|c| c == channel_name)
    {
        warn_actor_resolution(warned, true, field, Some(channel_name));
        return Ok(ObservationKind::ActorDot {
            actor: actor_name.to_string(),
            channel: channel_name.to_string(),
        });
    }
    for channels in actor_channels.values() {
        if channels.iter().any(|c| c == field) {
            warn_actor_resolution(warned, false, field, None);
            return Ok(ObservationKind::ActorBare {
                channel: field.to_string(),
            });
        }
    }

    // A renamed observation key's error names its replacement: the
    // run-state commit (B-14 commit 3) renamed the total power's key.
    let renamed = match field {
        "total_power_kw" | "total_electric_kw" | "total_electric_w" => Some("total_power_w"),
        _ => None,
    };
    if let Some(renamed) = renamed {
        return Err(HaresError::Control(format!(
            "unknown telemetry field key `{field}`: the key is `{renamed}` now, \
             and its value is watts (SI)"
        )));
    }
    Err(HaresError::Control(format!(
        "unknown telemetry field key `{field}`"
    )))
}

fn zone_position(
    name: &str,
    zone_positions: &HashMap<String, usize>,
    field: &str,
) -> Result<usize, HaresError> {
    zone_positions
        .get(name)
        .copied()
        .ok_or_else(|| HaresError::Control(format!("unknown zone in field `{field}`")))
}

fn equip_position(
    name: &str,
    equip_positions: &HashMap<String, usize>,
    field: &str,
) -> Result<usize, HaresError> {
    equip_positions
        .get(name)
        .copied()
        .ok_or_else(|| HaresError::Control(format!("unknown equipment in field `{field}`")))
}

fn bracket_name<'a>(field: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key}[");
    field
        .strip_prefix(&prefix)
        .and_then(|tail| tail.strip_suffix(']'))
}

fn single_instance_alias(
    equip_positions: &HashMap<String, usize>,
    canonical_name: &str,
    field: &str,
) -> Result<usize, HaresError> {
    if let Some(&idx) = equip_positions.get(&normalize_ascii(canonical_name)) {
        return Ok(idx);
    }
    Err(HaresError::Control(format!(
        "field `{field}` alias requires `{canonical_name}` equipment"
    )))
}

/// The throttled resolution warning, raised once per run at plan build:
/// the resolution moved from every step to the plan, and so did the
/// diagnostic, and the gates are the run's own [`ObservationWarnOnce`]
/// state, not process statics — a fresh run's plan build warns again.
fn warn_actor_resolution(
    warned: &mut ObservationWarnOnce,
    dot: bool,
    field: &str,
    key: Option<&str>,
) {
    let already = if dot {
        &mut warned.dot
    } else {
        &mut warned.bare
    };
    if !*already {
        *already = true;
        match key {
            Some(key) => tracing::warn!(
                field = field,
                actor_telemetry_key = key,
                "resolving observation field via actor_telemetry (throttled to once per run)"
            ),
            None => tracing::warn!(
                field = field,
                "resolving observation field via actor_telemetry (bare name, throttled to once per run)"
            ),
        }
    }
}

/// The telemetry executor: fills `out` from the snapshot the telemetry
/// already is. `to_observation_vec` is a thin wrapper over this.
pub(crate) fn telemetry_observe_into(
    telemetry: &DwellingTelemetry,
    plan: &ObservationPlan,
    out: &mut [f64],
) -> Result<(), HaresError> {
    if out.len() != plan.len() {
        return Err(HaresError::Control(format!(
            "the observation buffer holds {} slots for {} planned fields",
            out.len(),
            plan.len()
        )));
    }
    if !telemetry.initialized {
        out.fill(f64::NAN);
        return Ok(());
    }
    for (slot, source) in out.iter_mut().zip(plan.sources()) {
        *slot = telemetry_read(telemetry, source, plan)?;
    }
    Ok(())
}

fn telemetry_read(
    telemetry: &DwellingTelemetry,
    source: &ObservationSource,
    plan: &ObservationPlan,
) -> Result<f64, HaresError> {
    let field = source.field.as_str();
    Ok(match &source.kind {
        ObservationKind::OutdoorTemp => telemetry.outdoor_temp_c,
        ObservationKind::OutdoorHumidityRatio => telemetry.outdoor_humidity_ratio,
        ObservationKind::TotalPowerKw => telemetry.total_power_w,
        ObservationKind::ReactivePowerKvar => telemetry.reactive_power_var,
        ObservationKind::ZoneTemperature { position, .. } => {
            telemetry.zone_temperatures_c[*position]
        }
        ObservationKind::SetpointHeat { position, .. } => telemetry.setpoint_heat_c[*position],
        ObservationKind::SetpointCool { position, .. } => telemetry.setpoint_cool_c[*position],
        ObservationKind::ZoneEnergyBalance { position, .. } => {
            telemetry.energy_balance_residuals[*position]
        }
        ObservationKind::EquipmentSoc { index } => telemetry.equipment_soc[*index],
        ObservationKind::EquipmentPower { index } => telemetry.equipment_power_w[*index],
        ObservationKind::TimeSin => time_feature(telemetry, false),
        ObservationKind::TimeCos => time_feature(telemetry, true),
        ObservationKind::ActorDot { actor, channel } => {
            // allowed: actor_telemetry keys are user-defined strings, not
            // static tk:: constants; the plan froze the field's own
            // spelling at build.
            match telemetry
                .actor_telemetry
                .get(actor)
                .and_then(|channels| channels.get(channel))
            {
                Some(&value) => value,
                // The dot resolution missed at read time (the actor's
                // published channel set moved under the plan): the bare
                // search of the whole field, as the string walk fell
                // through.
                None => bare_actor_value(telemetry, field, field, plan)?,
            }
        }
        ObservationKind::ActorBare { channel } => {
            bare_actor_value(telemetry, channel, field, plan)?
        }
    })
}

/// The bare-name fallback: the first actor in the plan's sorted-name order
/// whose published telemetry holds the channel. A dot resolution whose
/// channel went missing lands here, as the string walk's did.
fn bare_actor_value(
    telemetry: &DwellingTelemetry,
    channel: &str,
    field: &str,
    plan: &ObservationPlan,
) -> Result<f64, HaresError> {
    for actor_name in plan.actor_order() {
        // allowed: actor_telemetry keys are user-defined strings, not
        // static tk:: constants; the plan froze the channel's own
        // spelling at build.
        if let Some(channels) = telemetry.actor_telemetry.get(actor_name)
            && let Some(&value) = channels.get(channel)
        {
            return Ok(value);
        }
    }
    Err(HaresError::Control(format!(
        "unknown telemetry field key `{field}`"
    )))
}

/// The `time_sin` / `time_cos` feature: the fractional hour's phase around
/// the day, in the same arithmetic the string walk used.
fn time_feature(telemetry: &DwellingTelemetry, cos: bool) -> f64 {
    use chrono::Timelike;
    let t = &telemetry.current_time;
    let fhour = t.hour() as f64 + t.minute() as f64 / 60.0 + t.second() as f64 / 3600.0;
    let phase = 2.0 * std::f64::consts::PI * fhour / 24.0;
    if cos { phase.cos() } else { phase.sin() }
}
