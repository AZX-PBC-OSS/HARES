//! Shared helpers for schedule-source state capture/restore and config parsing.
//!
//! Used by both `scheduled_load` and `event_load` modules.

pub(crate) use hares_types::ScheduleSourceState;

use hares_types::{HaresError, ScheduleSource, ZoneId};

use crate::{ConfigPayload, EquipmentConfig};

pub(crate) const KEY_MONTH_MULTIPLIER_PREFIX: &str = "month_multiplier_";

pub(crate) fn capture_schedule_source_state(source: &ScheduleSource) -> ScheduleSourceState {
    source.capture_state()
}

pub(crate) fn restore_schedule_source_state(
    source: &mut ScheduleSource,
    saved: &ScheduleSourceState,
) -> crate::Result<()> {
    source.restore_state(saved)
}

/// Restores an optional schedule source's captured position. The saved
/// state and the live source must agree on existing: a checkpoint
/// carrying a state for a source the equipment lacks, or a stateful
/// source whose position the checkpoint dropped, is an error naming the
/// mismatch, never a silent position loss. A stateless source (a
/// constant, a profile, a column reference) restores nothing.
pub(crate) fn restore_optional_source(
    source: &mut Option<ScheduleSource>,
    saved: Option<ScheduleSourceState>,
) -> crate::Result<()> {
    match (source.as_mut(), saved) {
        (Some(source), Some(state)) => source.restore_state(&state),
        (None, None) => Ok(()),
        (None, Some(state)) => Err(HaresError::Equipment(format!(
            "checkpoint carries schedule source state ({state:?}) but the equipment has \
             no such source"
        ))),
        (Some(source), None) => {
            if matches!(source.capture_state(), ScheduleSourceState::Stateless) {
                Ok(())
            } else {
                Err(HaresError::Equipment(format!(
                    "checkpoint is missing schedule state for a stateful {} source",
                    source.kind_name()
                )))
            }
        }
    }
}

pub(crate) fn parse_zone_id(config: &EquipmentConfig) -> Option<ZoneId> {
    let zone = parse_u16(config, crate::config::KEY_ZONE_ID).ok()??;
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
/// A multiplier must be finite and not negative: a load does not generate,
/// and taking a negative or NaN month as "off" would hide the input error.
pub(crate) fn parse_month_multipliers(
    config: &EquipmentConfig,
) -> Result<Option<[f64; 12]>, HaresError> {
    let mut found_any = false;
    let mut multipliers = [1.0_f64; 12];
    for (month, slot) in multipliers.iter_mut().enumerate() {
        let key = format!("{KEY_MONTH_MULTIPLIER_PREFIX}{month}");
        if let Some(val) = config.get_f64(&key) {
            if !(val.is_finite() && val >= 0.0) {
                return Err(HaresError::InvalidEquipmentParameter {
                    equipment: config.name.clone(),
                    key,
                    reason: format!("must be a finite number not below 0, got {val}"),
                });
            }
            *slot = val;
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
    fn negative_and_non_finite_month_multipliers_are_parameter_errors() {
        for (month, value) in [
            (3, f64::NAN),
            (0, f64::INFINITY),
            (11, f64::NEG_INFINITY),
            (5, -1.0),
            (7, -1e-9),
        ] {
            let config = config_with_month_multiplier(month, value);
            let err = parse_month_multipliers(&config)
                .expect_err("a negative or non-finite month multiplier must be rejected");
            let HaresError::InvalidEquipmentParameter { equipment, key, .. } = &err else {
                panic!("expected a parameter error, got {err:?}");
            };
            assert_eq!(equipment, "test");
            assert_eq!(*key, format!("{KEY_MONTH_MULTIPLIER_PREFIX}{month}"));
        }
        let zero = parse_month_multipliers(&config_with_month_multiplier(2, 0.0))
            .expect("zero turns a month off")
            .expect("one key present");
        assert_eq!(zero[2], 0.0);
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
