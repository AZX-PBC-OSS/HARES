//! The zone a scheduled or event-driven load gives its heat to.

use hares_types::{EndUse, EnvironmentState, HaresError, ZoneId, ZoneRole};

use crate::EquipmentConfig;
use crate::schedule_helpers::parse_u16;

const KEY_ZONE_ID: &str = "zone_id";

/// Resolves a load's zone once its gain fractions are known.
///
/// An explicit `zone_id` names the zone. EV charging, and a load named
/// exterior or outdoor without one, release their heat outside the building
/// and have no zone. Any other load takes the zone of the role its name
/// implies (garage, basement, crawlspace or attic, else the conditioned
/// zone) from the dwelling's zone map.
///
/// # Errors
///
/// A malformed `zone_id`; a load that gives heat to a zone (`gives_zone_heat`)
/// with no zone to give it to; a zone that is not one of the environment's
/// zones, whose heat would otherwise be dropped every step. Each names the
/// load.
pub(crate) fn resolve_load_zone(
    config: &EquipmentConfig,
    end_use: &EndUse,
    gives_zone_heat: bool,
    env: &EnvironmentState,
) -> crate::Result<Option<ZoneId>> {
    if *end_use == EndUse::EV {
        return Ok(None);
    }
    let name = config.name.to_ascii_lowercase();
    let zone = match parse_u16(config, KEY_ZONE_ID)? {
        Some(id) => Some(ZoneId(id)),
        None if name.contains("exterior") || name.contains("outdoor") => None,
        None => {
            let role = role_from_name(&name);
            let zone = config.zone_map.as_ref().and_then(|map| map.get(role));
            if zone.is_none() && gives_zone_heat {
                return Err(HaresError::Equipment(format!(
                    "{}: gives heat to its {role} zone, but the dwelling has no {role} zone \
                     (no zone_id and no zone map entry)",
                    config.name
                )));
            }
            zone
        }
    };
    if let Some(zone) = zone
        && !env.zones.iter().any(|z| z.id == zone)
    {
        return Err(HaresError::Equipment(format!(
            "{}: assigned zone {zone:?} not found in environment state",
            config.name
        )));
    }
    Ok(zone)
}

fn role_from_name(name_lower: &str) -> ZoneRole {
    if name_lower.contains("garage") {
        ZoneRole::Garage
    } else if name_lower.contains("basement") {
        ZoneRole::Basement
    } else if name_lower.contains("crawlspace") {
        ZoneRole::Crawlspace
    } else if name_lower.contains("attic") {
        ZoneRole::Attic
    } else {
        ZoneRole::Indoor
    }
}
