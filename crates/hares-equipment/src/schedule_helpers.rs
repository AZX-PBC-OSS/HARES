//! Shared helpers for schedule-source state capture/restore and config parsing.
//!
//! Used by both `scheduled_load` and `event_load` modules.

use hares_types::{HaresError, ScheduleSource, ZoneId};
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

use serde::{Deserialize, Serialize};

use crate::{ConfigPayload, EquipmentConfig};

pub(crate) const KEY_MONTH_MULTIPLIER_PREFIX: &str = "month_multiplier_";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum ScheduleSourceState {
    Stateless,
    Stochastic {
        draw_count: u64,
    },
    Shared {
        cursor: usize,
    },
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
                    "checkpoint contains NoisyTimeWindows state but source has no rng_state".into(),
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

pub(crate) fn parse_zone_id(config: &EquipmentConfig) -> Option<ZoneId> {
    let zone = parse_u16(config, "zone_id").ok()??;
    Some(ZoneId(zone))
}

pub(crate) fn parse_u16(config: &EquipmentConfig, key: &str) -> crate::Result<Option<u16>> {
    let value = match config.get_f64(key).or_else(|| typed_f64(config, key)) {
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

pub(crate) fn parse_u32(config: &EquipmentConfig, key: &str) -> crate::Result<u32> {
    let value = config
        .get_f64(key)
        .or_else(|| typed_f64(config, key))
        .unwrap_or(0.0);
    if !value.is_finite() || value < 0.0 || value.fract() != 0.0 || value > u32::MAX as f64 {
        return Err(HaresError::Equipment(format!(
            "invalid integer value for key {key}: {value}"
        )));
    }
    Ok(value as u32)
}

pub(crate) fn parse_usize(config: &EquipmentConfig, key: &str) -> crate::Result<Option<usize>> {
    let value = match config.get_f64(key).or_else(|| typed_f64(config, key)) {
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

fn typed_f64(config: &EquipmentConfig, key: &str) -> Option<f64> {
    match &config.payload {
        ConfigPayload::Typed { data, .. } => data.get(key).and_then(|v| v.as_f64()),
        ConfigPayload::Raw { .. } => None,
    }
}

/// Parse per-month scale factors from config keys `month_multiplier_0` through
/// `month_multiplier_11`. Returns `None` when no multiplier keys are present.
/// Non-finite values are rejected: `f64::max` would silently drop a NaN
/// operand and zero the month (a load quietly off for that month), the
/// same silent-absorption class every other schedule-data channel rejects
/// at its boundary.
pub(crate) fn parse_month_multipliers(
    config: &EquipmentConfig,
) -> Result<Option<[f64; 12]>, HaresError> {
    let mut found_any = false;
    let mut multipliers = [1.0_f64; 12];
    for (month, slot) in multipliers.iter_mut().enumerate() {
        let key = format!("{KEY_MONTH_MULTIPLIER_PREFIX}{month}");
        if let Some(val) = config.get_f64(&key) {
            if !val.is_finite() {
                return Err(HaresError::Equipment(format!(
                    "month multiplier {key} is non-finite ({val}); \
                     month multipliers must be finite"
                )));
            }
            *slot = val.max(0.0);
            found_any = true;
        }
    }
    Ok(found_any.then_some(multipliers))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn config_with_month_multiplier(month: usize, value: f64) -> EquipmentConfig {
        let mut raw: HashMap<String, crate::config::ConfigValue> = HashMap::new();
        raw.insert(
            format!("{KEY_MONTH_MULTIPLIER_PREFIX}{month}"),
            value.into(),
        );
        EquipmentConfig::raw("test".to_string(), "Test".to_string(), raw)
    }

    #[test]
    fn non_finite_month_multipliers_are_rejected_naming_the_key() {
        // `val.max(0.0)` would silently drop a NaN operand and zero the
        // month — a load quietly off for that month with no signal, the
        // same silent-absorption class every other schedule-data channel
        // rejects at its boundary. Both NaN and ±inf must fail, and the
        // error must name the offending key.
        for (month, value) in [(3, f64::NAN), (0, f64::INFINITY), (11, f64::NEG_INFINITY)] {
            let config = config_with_month_multiplier(month, value);
            let err = parse_month_multipliers(&config)
                .expect_err("non-finite month multiplier must be rejected");
            let key = format!("{KEY_MONTH_MULTIPLIER_PREFIX}{month}");
            assert!(
                err.to_string().contains(&key),
                "error must name the offending key {key:?}, got: {err}"
            );
        }
    }

    #[test]
    fn absent_month_multipliers_are_none_and_finite_ones_parse() {
        let empty = EquipmentConfig::raw("test".to_string(), "Test".to_string(), HashMap::new());
        assert_eq!(parse_month_multipliers(&empty).unwrap(), None);

        let config = config_with_month_multiplier(6, 0.5);
        let parsed = parse_month_multipliers(&config)
            .unwrap()
            .expect("one key present → Some");
        assert_eq!(parsed[6], 0.5);
        assert!(parsed[..6].iter().all(|v| *v == 1.0));
        assert!(parsed[7..].iter().all(|v| *v == 1.0));
    }
}
