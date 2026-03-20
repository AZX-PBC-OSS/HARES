//! Shared helpers for schedule-source state capture/restore and config parsing.
//!
//! Used by both `scheduled_load` and `event_load` modules.

use std::collections::HashMap;

use hares_types::{HaresError, ScheduleSource, ZoneId};
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

use crate::config::ConfigValue;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum ScheduleSourceState {
    Stateless,
    SeededNoise { draw_count: u64 },
    Shared { cursor: usize },
}

pub(crate) fn capture_schedule_source_state(source: &ScheduleSource) -> ScheduleSourceState {
    match source {
        ScheduleSource::Shared { cursor, .. } => ScheduleSourceState::Shared { cursor: *cursor },
        ScheduleSource::SeededNoise { draw_count, .. } => ScheduleSourceState::SeededNoise {
            draw_count: *draw_count,
        },
        ScheduleSource::Constant(_)
        | ScheduleSource::DailyProfile { .. }
        | ScheduleSource::ColumnRef { .. }
        | ScheduleSource::SolarAware { .. } => ScheduleSourceState::Stateless,
        // Wildcard required: ScheduleSource is #[non_exhaustive].
        // New variants must be handled explicitly here.
        _ => ScheduleSourceState::Stateless,
    }
}

pub(crate) fn restore_schedule_source_state(
    source: &mut ScheduleSource,
    saved: &ScheduleSourceState,
) -> crate::Result<()> {
    match (source, saved) {
        (ScheduleSource::Shared { cursor, .. }, ScheduleSourceState::Shared { cursor: saved }) => {
            *cursor = *saved;
            Ok(())
        }
        (
            ScheduleSource::SeededNoise {
                seed,
                draw_count,
                rng,
                ..
            },
            ScheduleSourceState::SeededNoise {
                draw_count: target_draw_count,
            },
        ) => {
            *rng = ChaCha8Rng::from_seed(*seed);
            rng.set_word_pos((*target_draw_count as u128) * 2);
            *draw_count = *target_draw_count;
            Ok(())
        }
        (
            ScheduleSource::Constant(_)
            | ScheduleSource::DailyProfile { .. }
            | ScheduleSource::ColumnRef { .. }
            | ScheduleSource::SolarAware { .. },
            ScheduleSourceState::Stateless,
        ) => Ok(()),
        // Wildcard required: ScheduleSource is #[non_exhaustive].
        // New variants must be handled explicitly here.
        _ => Err(HaresError::Equipment(
            "checkpoint schedule source state does not match current schedule source variant"
                .to_string(),
        )),
    }
}

pub(crate) fn parse_zone_id(raw: &HashMap<String, ConfigValue>) -> Option<ZoneId> {
    let zone = parse_u16(raw, "zone_id").ok()??;
    Some(ZoneId(zone))
}

pub(crate) fn parse_u16(
    raw: &HashMap<String, ConfigValue>,
    key: &str,
) -> crate::Result<Option<u16>> {
    let value = match raw.get(key).and_then(|v| v.as_f64()) {
        Some(value) => value,
        None => return Ok(None),
    };
    if !value.is_finite() || value < 0.0 || value.fract() != 0.0 || value > u16::MAX as f64 {
        return Err(HaresError::Equipment(format!(
            "invalid integer value for key {key}: {value}"
        )));
    }
    Ok(Some(value as u16))
}

pub(crate) fn parse_u32(raw: &HashMap<String, ConfigValue>, key: &str) -> crate::Result<u32> {
    let value = raw.get(key).and_then(|v| v.as_f64()).unwrap_or(0.0);
    if !value.is_finite() || value < 0.0 || value.fract() != 0.0 || value > u32::MAX as f64 {
        return Err(HaresError::Equipment(format!(
            "invalid integer value for key {key}: {value}"
        )));
    }
    Ok(value as u32)
}

pub(crate) fn parse_usize(
    raw: &HashMap<String, ConfigValue>,
    key: &str,
) -> crate::Result<Option<usize>> {
    let value = match raw.get(key).and_then(|v| v.as_f64()) {
        Some(value) => value,
        None => return Ok(None),
    };
    if !value.is_finite() || value < 0.0 || value.fract() != 0.0 {
        return Err(HaresError::Equipment(format!(
            "invalid usize value for key {key}: {value}"
        )));
    }
    Ok(Some(value as usize))
}
