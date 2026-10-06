//! Serializable snapshot of a tariff evaluator's full mutable state.
//!
//! The snapshot travels inside `DwellingCheckpoint` so a resumed dwelling
//! with a tariff prices and bills the post-resume steps exactly as the run
//! that wrote the checkpoint: the tariff, the horizon it was built for, the
//! price-index position and the open billing period with its accruals and
//! demand-window history all restore. The precomputed price arrays are not
//! carried: they are a pure function of the tariff, the horizon and the
//! interval, and rebuilding them reproduces them exactly.

use chrono::DateTime;
use chrono_tz::Tz;
use hares_types::{BillingCycle, HaresError};
use serde::{Deserialize, Serialize};

use crate::types::ElectricTariff;

/// Version of the tariff snapshot's schema. Bumped when the snapshot's
/// fields change; restore rejects a snapshot whose version differs, with
/// the checkpoint's own version gate wrapped around this one.
pub const TARIFF_SNAPSHOT_SCHEMA_VERSION: u32 = 1;

/// The demand window's ring buffer and running sum.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DemandWindowSnapshot {
    pub samples: Vec<f64>,
    pub head: usize,
    pub count: usize,
    pub running_sum: f64,
    pub push_count: u64,
}

/// The open billing period's accruals, peak history and window state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BillingStateSnapshot {
    /// RFC 3339 in the evaluator's time zone.
    pub period_start: String,
    /// RFC 3339 in the evaluator's time zone.
    pub period_end: String,
    /// When the tariff became active within the open period (RFC 3339 in
    /// the evaluator's time zone): the period start, or the attach or
    /// switch instant for a period a tariff joined mid-way.
    pub active_since: String,
    pub cumulative_import_kwh: f64,
    pub cumulative_export_kwh: f64,
    pub cumulative_energy_cost_usd: f64,
    pub cumulative_export_credit_usd: f64,
    pub peak_demand_kw: f64,
    pub period_peak_demand_kw: Vec<f64>,
    pub prior_peaks_kw: Vec<f64>,
    pub prior_period_peaks: Vec<Vec<f64>>,
    pub demand_window: DemandWindowSnapshot,
    pub billing_cycle: BillingCycle,
    pub max_prior_periods: usize,
    pub running_cpp_event_hours: u32,
    pub last_cpp_hour: Option<usize>,
    pub cumulative_ev_kwh: f64,
    pub steps_in_period: u64,
}

/// A tariff evaluator's complete mutable state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TariffSnapshot {
    pub schema_version: u32,
    pub tariff: ElectricTariff,
    /// IANA name of the evaluator's time zone.
    pub timezone: String,
    /// The simulation's start instant, RFC 3339 in the evaluator's zone.
    pub simulation_start: String,
    pub interval_seconds: u32,
    /// The number of precomputed steps; the simulation end is reconstructed
    /// from it, which recomputes the price arrays exactly as they were.
    pub total_steps: usize,
    pub step_index: usize,
    pub finished: bool,
    pub finalized: bool,
    pub billing: BillingStateSnapshot,
}

impl TariffSnapshot {
    /// The snapshot as an opaque checkpoint payload: versioned JSON bytes.
    ///
    /// JSON and not postcard because the tariff types' `skip_serializing_if`
    /// fields are asymmetric under a non-self-describing format (a skipped
    /// `None` desyncs a positional reader); serde_json is self-describing,
    /// and the workspace pins its float round-trip exact.
    ///
    /// # Errors
    ///
    /// The payload bytes failed to serialize, which for a struct of plain
    /// containers happens only on an io failure.
    pub fn to_blob(&self) -> Result<Vec<u8>, HaresError> {
        serde_json::to_vec(self)
            .map_err(|err| HaresError::Tariff(format!("tariff snapshot blob: {err}")))
    }

    /// Decodes a snapshot from its checkpoint payload.
    ///
    /// # Errors
    ///
    /// Bytes that are not a JSON snapshot of the current schema.
    pub fn from_blob(bytes: &[u8]) -> Result<Self, HaresError> {
        serde_json::from_slice(bytes)
            .map_err(|err| HaresError::Tariff(format!("tariff snapshot blob: {err}")))
    }
}

pub(crate) fn datetime_to_string(dt: DateTime<Tz>) -> String {
    dt.to_rfc3339()
}

pub(crate) fn datetime_from_string(
    tz: Tz,
    name: &str,
    text: &str,
) -> Result<DateTime<Tz>, HaresError> {
    DateTime::parse_from_rfc3339(text)
        .map_err(|err| HaresError::Tariff(format!("tariff snapshot {name} '{text}': {err}")))
        .map(|dt| dt.with_timezone(&tz))
}

pub(crate) fn parse_zone(name: &str) -> Result<Tz, HaresError> {
    name.parse::<Tz>()
        .map_err(|err| HaresError::Tariff(format!("tariff snapshot timezone '{name}': {err}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evaluator::TariffEvaluator;
    use chrono::{Duration, TimeZone};
    use chrono_tz::America::New_York;
    use hares_types::{DayFilter, SeasonFilter, TimeWindow, TouPeriod};

    use crate::types::{EnergyRate, FixedCharges};

    fn flat_tariff() -> ElectricTariff {
        ElectricTariff {
            name: Some("snapshot-flat".into()),
            tou_schedule: vec![TouPeriod {
                name: "flat".into(),
                schedule: vec![TimeWindow::new(DayFilter::Any, 0, 1440, 0.0)],
                season: SeasonFilter::All,
            }],
            energy_rates: vec![EnergyRate {
                period_name: "flat".into(),
                season: SeasonFilter::All,
                rate_per_kwh: 0.15,
            }],
            fixed_charges: FixedCharges {
                monthly_usd: 10.0,
                daily_usd: 0.0,
            },
            ..Default::default()
        }
    }

    fn make_evaluator() -> TariffEvaluator {
        let start = New_York.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let end = New_York.with_ymd_and_hms(2024, 3, 1, 0, 0, 0).unwrap();
        TariffEvaluator::new(flat_tariff(), start, end, 3600).unwrap()
    }

    #[test]
    fn snapshot_round_trips_and_continues_bitwise() {
        let start = New_York.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let mut original = make_evaluator();
        for i in 0..100 {
            let step_end = start + Duration::seconds((i as i64 + 1) * 3600);
            original.step(2.0 + i as f64 * 0.01, 0.0, 3600.0, step_end);
        }

        let snapshot = original.snapshot_state();
        let mut restored = TariffEvaluator::from_snapshot(&snapshot).unwrap();

        // The snapshots of the two evaluators are identical after restore.
        assert_eq!(restored.snapshot_state(), snapshot);

        // And the two evaluators keep stepping bitwise identically.
        for i in 100..200 {
            let step_end = start + Duration::seconds((i as i64 + 1) * 3600);
            let a = original.step(3.0, 0.0, 3600.0, step_end);
            let b = restored.step(3.0, 0.0, 3600.0, step_end);
            match (a, b) {
                (None, None) => {}
                (Some(a), Some(b)) => {
                    assert_eq!(a.period_start, b.period_start);
                    assert_eq!(a.period_end, b.period_end);
                    assert_eq!(a.energy_charge_usd.to_bits(), b.energy_charge_usd.to_bits());
                    assert_eq!(a.demand_charge_usd.to_bits(), b.demand_charge_usd.to_bits());
                    assert_eq!(a.fixed_charge_usd.to_bits(), b.fixed_charge_usd.to_bits());
                    assert_eq!(a.total_import_kwh.to_bits(), b.total_import_kwh.to_bits());
                }
                (_a, _b) => {
                    panic!("step {i} divergence: one produced a summary, the other did not")
                }
            }
        }
        assert_eq!(
            original.billing_state().cumulative_import_kwh().to_bits(),
            restored.billing_state().cumulative_import_kwh().to_bits()
        );
    }

    #[test]
    fn snapshot_rejects_foreign_schema_version() {
        let mut snapshot = make_evaluator().snapshot_state();
        snapshot.schema_version += 1;
        let err = TariffEvaluator::from_snapshot(&snapshot)
            .map(|_| ())
            .unwrap_err();
        assert!(
            err.to_string().contains("tariff snapshot version mismatch"),
            "expected a snapshot version mismatch, got: {err}"
        );
    }

    #[test]
    fn snapshot_rejects_out_of_bounds_step_index() {
        let mut snapshot = make_evaluator().snapshot_state();
        snapshot.step_index = snapshot.total_steps + 1;
        let err = TariffEvaluator::from_snapshot(&snapshot)
            .map(|_| ())
            .unwrap_err();
        assert!(
            err.to_string().contains("out of bounds"),
            "expected an out-of-bounds error, got: {err}"
        );
    }
}
