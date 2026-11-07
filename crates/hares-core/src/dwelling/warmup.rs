//! The warm-up convergence runner: EnergyPlus's warm-up criteria over the
//! first day's replay, the outcome writes on every return path, and the
//! state-restored replay blocker the public entry records.

use hares_types::{HealthEvent, HaresError, ZoneId};

use crate::health::{WarmupOutcome, WarmupResiduals};
use super::{Dwelling, Result, StepResult, steps_per_day_exact};

/// The dwelling's warm-up convergence runner.
///
/// The runner owns the convergence loop and the outcome writes: the
/// per-day records, the EnergyPlus comparison, the mid-warm-up failure
/// paths (a step error, a non-finite residual), the convergence return,
/// the non-convergence warning and policy violation, and the
/// `RunHealth::warmup` outcome on every return path once warm-up runs.
/// The outcome and residual types are the health module's own
/// ([`WarmupOutcome`], [`WarmupResiduals`]); the runner invents no second
/// representation.
pub(super) struct WarmupRunner;

impl WarmupRunner {
    /// The public entry's body: a state-restored replay blocker first
    /// (a warm-up replay is not a production step the checkpoint's
    /// audit may count), then the convergence loop.
    pub(super) fn run_converged(
        dwelling: &mut Dwelling,
        warmup: &hares_input::WarmupConfig,
    ) -> Result<u32> {
        dwelling.record_replay_blocker(hares_types::ReplayBlocker::StateRestored);
        Self::run_silent(dwelling, warmup)
    }

    /// The convergence loop's body, the record nothing path: the build's
    /// own warm-up runs through here (a caller's
    /// [`Dwelling::run_warmup_converged`] records the state-restored replay
    /// blocker first, the build's own warm-up records nothing).
    pub(super) fn run_silent(dwelling: &mut Dwelling, warmup: &hares_input::WarmupConfig) -> Result<u32> {
        let steps_per_day = steps_per_day_exact(dwelling.clock.time_res.num_milliseconds())?;

        dwelling.is_warming_up = true;

        // Previous day's per-zone daily max/min temperatures and daily peak
        // HVAC delivery: the residual baseline. EnergyPlus keeps one record
        // per zone (`MaxTempZone`, `MinTempZone`, `MaxHeatLoadZone`,
        // `MaxCoolLoadZone` and their `PrevDay` pairs,
        // `HeatBalanceManager.cc:3237-3247`, `:2888-2890`).
        let mut prev_day: Option<WarmupDayRecord> = None;
        // Residuals of the final day (recorded on health at the return paths)
        // and the zones whose final-day comparison failed any of the four
        // checks (named in the non-convergence warning).
        let mut final_residuals: Option<WarmupResiduals> = None;
        let mut failing_zones: Vec<ZoneId> = Vec::new();

        // One step result for the whole warm-up: the loop drops each
        // step's result unread (the convergence reads the dwelling's own
        // per-step fields below, never the step result), and every field
        // is overwritten per step, so the loop's steady state allocates
        // nothing for it (the `simulate_inner` loop's shape).
        let mut step_result = StepResult::default();

        for day in 1..=warmup.max_days {
            // Record the day: per-zone daily extremes and daily peak HVAC
            // delivery, accumulated per step below.
            let mut record = WarmupDayRecord::new(&dwelling.latest_env.zones);

            // Reset clock to start of first day for weather replay.
            // Thermal state carries forward from previous iteration:
            // initial conditions for each warmup day are the final
            // conditions from the previous warmup day.
            dwelling.clock.current_step = 0;

            for _ in 0..steps_per_day {
                if let Err(err) = dwelling.run_timestep_into(false, &mut step_result) {
                    // The run failed mid-warm-up: exit warm-up mode and
                    // record the outcome honestly (stopped during
                    // `day`, not converged) before propagating.
                    dwelling.is_warming_up = false;
                    dwelling.health.warmup = WarmupOutcome::Ran {
                        days_run: day,
                        converged: false,
                        residuals: final_residuals.clone(),
                    };
                    return Err(err);
                }
                for (i, zone) in dwelling.latest_env.zones.iter().enumerate() {
                    record.max_t[i] = record.max_t[i].max(zone.temperature_c);
                    record.min_t[i] = record.min_t[i].min(zone.temperature_c);
                }
                for &(zone, heating_w) in &dwelling.step_hvac_heating_by_zone {
                    if let Some(i) = record.zone_index(zone) {
                        record.peak_heating_w[i] = record.peak_heating_w[i].max(heating_w);
                    }
                }
                for &(zone, cooling_w) in &dwelling.step_hvac_cooling_by_zone {
                    if let Some(i) = record.zone_index(zone) {
                        record.peak_cooling_w[i] = record.peak_cooling_w[i].max(cooling_w);
                    }
                }
            }

            // The test hook: a recorded day quantity overwritten with a
            // non-finite value after the day's record completes, while the
            // zone's own state stays finite, so the next day's comparison
            // meets the NaN residual.
            #[cfg(test)]
            warmup_test_hooks::overwrite_recorded_day(day, &mut record);

            if let Some(prev) = &prev_day {
                let comparison = compare_warmup_days(
                    prev,
                    &record,
                    dwelling.latest_env.zones.iter().map(|z| z.id),
                    day,
                    warmup,
                );
                let comparison = match comparison {
                    Ok(comparison) => comparison,
                    // The comparison itself failed (a non-finite residual):
                    // record the outcome honestly before the failure
                    // propagates, the way the mid-day failure path does.
                    Err(err) => {
                        dwelling.is_warming_up = false;
                        dwelling.health.warmup = WarmupOutcome::Ran {
                            days_run: day,
                            converged: false,
                            residuals: final_residuals.clone(),
                        };
                        return Err(err);
                    }
                };
                let DayComparison {
                    converged,
                    residuals,
                    failing,
                } = comparison;
                final_residuals = Some(residuals);
                failing_zones = failing;
                // EnergyPlus's per-day trace: the four residuals of every
                // zone.
                if tracing::enabled!(tracing::Level::DEBUG) {
                    for (i, zone) in dwelling.latest_env.zones.iter().enumerate() {
                        tracing::debug!(
                            day,
                            zone = %zone.id,
                            max_temperature_c = (record.max_t[i] - prev.max_t[i]).abs(),
                            min_temperature_c = (record.min_t[i] - prev.min_t[i]).abs(),
                            heating_load = warmup_load_residual(
                                record.peak_heating_w[i],
                                prev.peak_heating_w[i],
                            ),
                            cooling_load = warmup_load_residual(
                                record.peak_cooling_w[i],
                                prev.peak_cooling_w[i],
                            ),
                            converged,
                            "warm-up day comparison"
                        );
                    }
                }

                if converged && day >= warmup.min_days {
                    tracing::info!(days = day, "warm-up converged by the EnergyPlus criteria");
                    dwelling.is_warming_up = false;
                    dwelling.health.warmup = WarmupOutcome::Ran {
                        days_run: day,
                        converged: true,
                        residuals: final_residuals.clone(),
                    };
                    return Ok(day);
                }
            }

            prev_day = Some(record);
        }

        dwelling.is_warming_up = false;

        // Honest warning surface: the run proceeded without a converged
        // initial state, so the non-convergence is counted in health AND
        // pushed as a run warning naming the zones whose checks failed
        // (the worst-residual zone when none individually fails).
        dwelling.health.warmup = WarmupOutcome::Ran {
            days_run: warmup.max_days,
            converged: false,
            residuals: final_residuals.clone(),
        };
        let warned_zones = if failing_zones.is_empty() {
            final_residuals
                .as_ref()
                .map(|r| vec![r.worst_zone])
                .unwrap_or_default()
        } else {
            failing_zones
        };
        dwelling.warnings.push_from(
            "warmup",
            format!(
                "warm-up failed to converge within {} days by the EnergyPlus criteria \
                 (temperature tolerance {} delta C, loads tolerance {}); proceeding with \
                 current thermal state; zones failing a check: {warned_zones:?}",
                warmup.max_days, warmup.temperature_tolerance_c, warmup.loads_tolerance
            ),
        );
        tracing::warn!(
            days = warmup.max_days,
            "warm-up failed to converge within {} days by the EnergyPlus criteria; \
             proceeding with current thermal state",
            warmup.max_days
        );
        // The policy's warm-up requirement is checked when warm-up ends:
        // a non-converged warm-up fails the run (counted first, as above),
        // with the step index None (warm-up is not a step of the run).
        if dwelling.sim_config.health_policy.require_warmup_convergence {
            return Err(
                dwelling.latch_health_violation(HaresError::HealthPolicyViolation {
                    event: HealthEvent::WarmupNotConverged,
                    step_index: None,
                    origin: "warmup".to_string(),
                    count: 1,
                    limit: 0,
                }),
            );
        }
        Ok(warmup.max_days)
    }
}

/// EnergyPlus's `MinLoad` for the warm-up convergence check: both days'
/// daily peak loads are floored here before the relative comparison, so a
/// low-load day cannot produce a huge percentage difference
/// (`HeatBalanceManager.cc:3320`).
const WARMUP_MIN_LOAD_W: f64 = 100.0;

/// The peak load at or below which the load check passes without a
/// comparison: "make sure load big enough to divide"
/// (`HeatBalanceManager.cc:3350`, `:3366`).
const WARMUP_LOAD_FLOOR_W: f64 = 1e-4;

/// One warm-up day's per-zone record: the daily maximum and minimum zone
/// air temperature and the daily peak HVAC heating and cooling delivered
/// to the zone, the per-zone values behind the run's
/// `HVAC Heating Delivered - {zone} (W)` and `HVAC Cooling Delivered -
/// {zone} (W)` columns. Indexed with `latest_env.zones`; `zone_ids` is
/// captured alongside for the per-zone lookup by id.
pub(crate) struct WarmupDayRecord {
    max_t: Vec<f64>,
    min_t: Vec<f64>,
    peak_heating_w: Vec<f64>,
    peak_cooling_w: Vec<f64>,
    zone_ids: Vec<ZoneId>,
}

impl WarmupDayRecord {
    fn new(zones: &[hares_types::ZoneState]) -> Self {
        Self {
            max_t: vec![f64::NEG_INFINITY; zones.len()],
            min_t: vec![f64::INFINITY; zones.len()],
            peak_heating_w: vec![0.0; zones.len()],
            peak_cooling_w: vec![0.0; zones.len()],
            zone_ids: zones.iter().map(|z| z.id).collect(),
        }
    }

    fn zone_index(&self, zone: ZoneId) -> Option<usize> {
        self.zone_ids.iter().position(|&z| z == zone)
    }
}

/// EnergyPlus's load residual for one zone's daily peak: when today's peak
/// exceeds the 1e-4 W floor-of-record threshold, both days' peaks are
/// floored at 100 W and the relative change is the residual; at or below
/// it, the check passes and the residual is 0.0
/// (`HeatBalanceManager.cc:3350-3364`, `:3366-3380`).
fn warmup_load_residual(today_w: f64, yesterday_w: f64) -> f64 {
    if today_w > WARMUP_LOAD_FLOOR_W {
        let today = today_w.max(WARMUP_MIN_LOAD_W).abs();
        let yesterday = yesterday_w.max(WARMUP_MIN_LOAD_W).abs();
        (today - yesterday).abs() / today
    } else {
        0.0
    }
}

/// One day's four zone checks: whether every zone's checks pass, the
/// recorded residuals of the worst zone (largest daily-max temperature
/// residual, its own loads beside it), and the zones failing a check.
struct DayComparison {
    converged: bool,
    residuals: WarmupResiduals,
    failing: Vec<ZoneId>,
}

/// Compares two warm-up days' records by the EnergyPlus criteria, every
/// zone (`HeatBalanceManager.cc:3330-3380`): the daily maximum and minimum
/// temperature change against the temperature tolerance and the daily peak
/// heating and cooling delivered against the loads tolerance with the
/// 100 W floor. A non-finite residual is not a comparison result: the
/// comparison fails the run with `NanDetected` naming the zone and the
/// warm-up day.
fn compare_warmup_days(
    prev: &WarmupDayRecord,
    today: &WarmupDayRecord,
    zone_ids: impl Iterator<Item = ZoneId>,
    day: u32,
    warmup: &hares_input::WarmupConfig,
) -> Result<DayComparison> {
    let mut converged = true;
    let mut failing = Vec::new();
    let mut worst_index = 0usize;
    let mut worst_max_t = f64::MIN;
    let mut residuals_of_worst: Option<WarmupResiduals> = None;

    for (i, zone_id) in zone_ids.enumerate() {
        let d_max = (today.max_t[i] - prev.max_t[i]).abs();
        let d_min = (today.min_t[i] - prev.min_t[i]).abs();
        let heating = warmup_load_residual(today.peak_heating_w[i], prev.peak_heating_w[i]);
        let cooling = warmup_load_residual(today.peak_cooling_w[i], prev.peak_cooling_w[i]);
        if !d_max.is_finite() || !d_min.is_finite() {
            return Err(HaresError::NanDetected {
                step_index: u64::from(day),
                zone_id: Some(zone_id),
                value_name: format!(
                    "warm-up day {day} daily temperature extreme against the previous day"
                ),
            });
        }
        if !heating.is_finite() || !cooling.is_finite() {
            return Err(HaresError::NanDetected {
                step_index: u64::from(day),
                zone_id: Some(zone_id),
                value_name: format!("warm-up day {day} daily peak HVAC delivery"),
            });
        }
        let zone_converged = d_max <= warmup.temperature_tolerance_c
            && d_min <= warmup.temperature_tolerance_c
            && heating <= warmup.loads_tolerance
            && cooling <= warmup.loads_tolerance;
        if !zone_converged {
            converged = false;
            failing.push(zone_id);
        }
        if d_max > worst_max_t {
            worst_max_t = d_max;
            worst_index = i;
            residuals_of_worst = Some(WarmupResiduals {
                worst_zone: zone_id,
                max_temperature_c: d_max,
                min_temperature_c: d_min,
                heating_load: heating,
                cooling_load: cooling,
            });
        }
    }

    let residuals = residuals_of_worst.unwrap_or_else(|| {
        // No zones: nothing to compare; the record names zone 0 with zero
        // residuals so the outcome still carries the type's invariants.
        WarmupResiduals {
            worst_zone: today
                .zone_ids
                .first()
                .copied()
                .unwrap_or(ZoneId(u16::try_from(worst_index).unwrap_or(u16::MAX))),
            max_temperature_c: 0.0,
            min_temperature_c: 0.0,
            heating_load: 0.0,
            cooling_load: 0.0,
        }
    });
    Ok(DayComparison {
        converged,
        residuals,
        failing,
    })
}

/// The warm-up tests' injection hook: a recorded day quantity overwritten
/// with NaN while the zone's own state stays finite, so the day-over-day
/// comparison (not the per-step NaN check) meets the non-finite residual.
/// The state is thread-local: a dwelling's warm-up runs on its building
/// thread, and two tests' warm-ups must never share an arming.
#[cfg(test)]
pub(crate) mod warmup_test_hooks {
    use std::cell::Cell;

    thread_local! {
        static NAN_MAX_DAY: Cell<u32> = const { Cell::new(0) };
        static NAN_MAX_ZONE: Cell<u32> = const { Cell::new(u32::MAX) };
    }

    /// Arms the hook: on warm-up day `day`, the named zone's recorded daily
    /// maximum temperature becomes NaN.
    pub(crate) fn arm_nan_daily_max(day: u32, zone: u16) {
        NAN_MAX_DAY.with(|cell| cell.set(day));
        NAN_MAX_ZONE.with(|cell| cell.set(u32::from(zone)));
    }

    /// The hook itself, called at the day record's completion.
    pub(crate) fn overwrite_recorded_day(record_day: u32, record: &mut super::WarmupDayRecord) {
        let armed = NAN_MAX_DAY.with(Cell::get);
        if armed != 0 && armed == record_day {
            let zone = NAN_MAX_ZONE.with(Cell::get);
            if let Some(i) = record.zone_ids.iter().position(|z| u32::from(z.0) == zone) {
                record.max_t[i] = f64::NAN;
            }
            NAN_MAX_DAY.with(|cell| cell.set(0));
        }
    }
}

