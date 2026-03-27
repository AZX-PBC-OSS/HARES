//! Shared helpers for schedule-source state capture/restore and config parsing.
//!
//! Used by both `scheduled_load` and `event_load` modules.

use std::collections::HashMap;

use hares_types::{HaresError, ScheduleSource, ZoneId};
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

use serde::{Deserialize, Serialize};

use crate::config::ConfigValue;
use crate::EquipmentConfig;

pub(crate) const KEY_MONTH_MULTIPLIER_PREFIX: &str = "month_multiplier_";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum ScheduleSourceState {
    Stateless,
    Stochastic { draw_count: u64 },
    Shared { cursor: usize },
    /// TimeWindows with noisy windows: save RNG word position directly since
    /// we can't replay (the distribution sampled varies per matching window).
    NoisyTimeWindows {
        draw_count: u64,
        rng_word_pos: u128,
    },
}

pub(crate) fn capture_schedule_source_state(source: &ScheduleSource) -> ScheduleSourceState {
    match source {
        ScheduleSource::Shared { cursor, .. } => ScheduleSourceState::Shared { cursor: *cursor },
        ScheduleSource::Stochastic { draw_count, .. } => ScheduleSourceState::Stochastic {
            draw_count: *draw_count,
        },
        ScheduleSource::TimeWindows { rng_state, .. } => match rng_state.as_ref() {
            Some(st) => ScheduleSourceState::NoisyTimeWindows {
                draw_count: st.draw_count,
                rng_word_pos: st.rng.get_word_pos(),
            },
            None => ScheduleSourceState::Stateless,
        },
        ScheduleSource::Constant(_)
        | ScheduleSource::DailyProfile { .. }
        | ScheduleSource::ColumnRef { .. }
        | ScheduleSource::SolarAware { .. } => ScheduleSourceState::Stateless,
        // Wildcard required: ScheduleSource is #[non_exhaustive].
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
            ScheduleSource::Stochastic {
                kind,
                seed,
                draw_count,
                rng,
                ..
            },
            ScheduleSourceState::Stochastic {
                draw_count: target_draw_count,
            },
        ) => {
            // Re-seed and replay draws to advance the RNG to the correct state.
            // This is safe for all distribution kinds regardless of per-draw
            // RNG word consumption (e.g. Poisson uses rejection sampling).
            *rng = ChaCha8Rng::from_seed(*seed);
            for _ in 0..*target_draw_count {
                kind.sample(rng)?;
            }
            *draw_count = *target_draw_count;
            Ok(())
        }
        (
            ScheduleSource::TimeWindows { rng_state, .. },
            ScheduleSourceState::NoisyTimeWindows {
                draw_count,
                rng_word_pos,
            },
        ) => {
            let st = rng_state.as_mut().ok_or_else(|| {
                HaresError::Equipment(
                    "checkpoint contains NoisyTimeWindows state but source has no rng_state"
                        .into(),
                )
            })?;
            // Restore RNG to exact position via word-level seek.
            // Replay isn't possible because the distribution sampled varies
            // per matching window at each timestep.
            st.rng = ChaCha8Rng::from_seed(st.seed);
            st.rng.set_word_pos(*rng_word_pos);
            st.draw_count = *draw_count;
            Ok(())
        }
        (
            ScheduleSource::Constant(_)
            | ScheduleSource::DailyProfile { .. }
            | ScheduleSource::ColumnRef { .. }
            | ScheduleSource::SolarAware { .. }
            | ScheduleSource::TimeWindows {
                rng_state: None, ..
            },
            ScheduleSourceState::Stateless,
        ) => Ok(()),
        // Wildcard required: ScheduleSource is #[non_exhaustive].
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

/// Parse per-month scale factors from config keys `month_multiplier_0` through
/// `month_multiplier_11`. Returns `None` when no multiplier keys are present.
pub(crate) fn parse_month_multipliers(config: &EquipmentConfig) -> Option<[f64; 12]> {
    let mut found_any = false;
    let mut multipliers = [1.0_f64; 12];
    for (month, slot) in multipliers.iter_mut().enumerate() {
        let key = format!("{KEY_MONTH_MULTIPLIER_PREFIX}{month}");
        if let Some(val) = config.get_f64(&key) {
            *slot = val.max(0.0);
            found_any = true;
        }
    }
    found_any.then_some(multipliers)
}
