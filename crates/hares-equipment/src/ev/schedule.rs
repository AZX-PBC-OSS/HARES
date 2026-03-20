use std::path::Path;

use hares_types::HaresError;
use rand::RngExt;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

use crate::EquipmentConfig;

use super::archetype::{DriverArchetype, default_distribution};
use super::config::{KEY_SCHEDULE_CSV_PATH, KEY_SCHEDULE_CSV_REF, KEY_SCHEDULE_LEN};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(super) struct EventDistributionRow {
    pub(super) arrival_minute: u16,
    pub(super) duration_minutes: u16,
    pub(super) start_soc: f64,
    pub(super) weight: f64,
}

pub(super) fn parse_distribution_rows(
    config: &EquipmentConfig,
    archetype: DriverArchetype,
) -> crate::Result<Vec<EventDistributionRow>> {
    if let Some(path) = config
        .get_str(KEY_SCHEDULE_CSV_PATH)
        .or_else(|| config.get_str(KEY_SCHEDULE_CSV_REF))
    {
        let from_csv = parse_distribution_csv(Path::new(path))?;
        if !from_csv.is_empty() {
            return Ok(from_csv);
        }
    }

    let len = config.get_f64(KEY_SCHEDULE_LEN).unwrap_or_default() as usize;
    if len == 0 {
        return Ok(default_distribution(archetype));
    }

    let mut rows = Vec::with_capacity(len);
    for idx in 0..len {
        let arr = key_with_idx("schedule_arrival_minute_", idx);
        let dur = key_with_idx("schedule_duration_minute_", idx);
        let soc = key_with_idx("schedule_start_soc_", idx);
        let w = key_with_idx("schedule_weight_", idx);

        let arrival = config.get_f64(&arr).ok_or_else(|| {
            HaresError::Equipment(format!("missing EV schedule field '{arr}' for row {idx}"))
        })?;
        let duration = config.get_f64(&dur).ok_or_else(|| {
            HaresError::Equipment(format!("missing EV schedule field '{dur}' for row {idx}"))
        })?;
        let start_soc = config.get_f64(&soc).ok_or_else(|| {
            HaresError::Equipment(format!("missing EV schedule field '{soc}' for row {idx}"))
        })?;
        let weight = config.get_f64(&w).unwrap_or(1.0);

        rows.push(build_distribution_row(
            arrival, duration, start_soc, weight,
        )?);
    }

    Ok(rows)
}

fn parse_distribution_csv(path: &Path) -> crate::Result<Vec<EventDistributionRow>> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        HaresError::Equipment(format!(
            "failed to read EV schedule CSV '{}': {e}",
            path.display()
        ))
    })?;

    let mut lines = text.lines();
    let Some(header_line) = lines.next() else {
        return Ok(Vec::new());
    };

    let headers: Vec<_> = header_line.split(',').map(str::trim).collect();
    let start_time_idx = headers
        .iter()
        .position(|h| *h == "start_time")
        .ok_or_else(|| {
            HaresError::Equipment(format!(
                "EV schedule CSV '{}' missing required column 'start_time'",
                path.display()
            ))
        })?;
    let duration_idx = headers
        .iter()
        .position(|h| *h == "duration")
        .ok_or_else(|| {
            HaresError::Equipment(format!(
                "EV schedule CSV '{}' missing required column 'duration'",
                path.display()
            ))
        })?;
    let start_soc_idx = headers
        .iter()
        .position(|h| *h == "start_soc")
        .ok_or_else(|| {
            HaresError::Equipment(format!(
                "EV schedule CSV '{}' missing required column 'start_soc'",
                path.display()
            ))
        })?;
    let weight_idx = headers.iter().position(|h| *h == "weight");

    let mut rows = Vec::new();
    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let cols: Vec<_> = line.split(',').map(str::trim).collect();
        if cols.len() <= start_soc_idx || cols.len() <= duration_idx || cols.len() <= start_time_idx
        {
            continue;
        }

        let arrival = parse_csv_float(cols[start_time_idx], "start_time", path)?;
        let duration = parse_csv_float(cols[duration_idx], "duration", path)?;
        let start_soc_raw = parse_csv_float(cols[start_soc_idx], "start_soc", path)?;
        let start_soc = if start_soc_raw > 1.0 {
            start_soc_raw / 100.0
        } else {
            start_soc_raw
        };

        let weight = match weight_idx.and_then(|idx| cols.get(idx)) {
            Some(v) => parse_csv_float(v, "weight", path)?,
            None => 1.0,
        };

        rows.push(build_distribution_row(
            arrival, duration, start_soc, weight,
        )?);
    }

    Ok(rows)
}

fn parse_csv_float(value: &str, col: &str, path: &Path) -> crate::Result<f64> {
    value.parse::<f64>().map_err(|e| {
        HaresError::Equipment(format!(
            "failed to parse EV schedule CSV '{}' column '{col}' value '{value}': {e}",
            path.display()
        ))
    })
}

fn build_distribution_row(
    arrival_minute: f64,
    duration_minutes: f64,
    start_soc: f64,
    weight: f64,
) -> crate::Result<EventDistributionRow> {
    if !arrival_minute.is_finite() || !(0.0..=1440.0).contains(&arrival_minute) {
        return Err(HaresError::Equipment(
            "EV schedule arrival_minute must be finite and within [0, 1440]".to_string(),
        ));
    }
    if !duration_minutes.is_finite() || duration_minutes <= 0.0 || duration_minutes > 1440.0 {
        return Err(HaresError::Equipment(
            "EV schedule duration_minutes must be finite and within (0, 1440]".to_string(),
        ));
    }
    if !start_soc.is_finite() || !(0.0..=1.0).contains(&start_soc) {
        return Err(HaresError::Equipment(
            "EV schedule start_soc must be finite and within [0, 1]".to_string(),
        ));
    }
    if !weight.is_finite() || weight <= 0.0 {
        return Err(HaresError::Equipment(
            "EV schedule weight must be finite and > 0".to_string(),
        ));
    }

    Ok(EventDistributionRow {
        arrival_minute: arrival_minute.round() as u16,
        duration_minutes: duration_minutes.round() as u16,
        start_soc,
        weight,
    })
}

fn key_with_idx(prefix: &str, idx: usize) -> String {
    format!("{prefix}{idx}")
}

pub(super) fn sample_distribution(
    rng: &mut ChaCha8Rng,
    rng_draws: &mut u64,
    rows: &[EventDistributionRow],
) -> Option<EventDistributionRow> {
    if rows.is_empty() {
        return None;
    }

    let total_weight: f64 = rows.iter().map(|r| r.weight).sum();
    if !total_weight.is_finite() || total_weight <= 0.0 {
        return None;
    }

    *rng_draws = rng_draws.saturating_add(1);
    let draw = rng.random::<f64>() * total_weight;

    let mut cumulative = 0.0;
    for row in rows {
        cumulative += row.weight;
        if draw <= cumulative {
            return Some(*row);
        }
    }

    rows.last().copied()
}
