//! Serializable snapshot of a tariff evaluator's full mutable state.
//!
//! The snapshot travels inside `DwellingCheckpoint` so a resumed dwelling
//! with a tariff prices and bills the post-resume steps exactly as the run
//! that wrote the checkpoint: the tariff, the horizon it was built for, the
//! price-index position and the open billing period with its accruals and
//! demand-window history all restore. The precomputed price arrays are not
//! carried: they are a pure function of the tariff, the horizon and the
//! interval, and rebuilding them reproduces them exactly.
//!
//! Every `DateTime<Tz>` bound travels as a [`LocalInstant`]: the local
//! wall-clock time and the zone's offset at that time, never a bare UTC
//! instant, so the bounds a resumed evaluator holds are the written run's
//! own wall-clock forms.

use chrono::{DateTime, NaiveDateTime, Offset, TimeZone};
use chrono_tz::Tz;
use hares_types::{BillingCycle, HaresError};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::types::ElectricTariff;

/// Version of the tariff snapshot's schema. Bumped when the snapshot's
/// fields change; restore rejects a snapshot whose version differs, with
/// the checkpoint's own version gate wrapped around this one.
///
/// v2: every `DateTime<Tz>` bound became a [`LocalInstant`] (the local
/// wall clock plus the zone's offset at that time, in place of the RFC
/// 3339 strings), and [`SnapshotError`] replaced the opaque string errors
/// on the restore path.
pub const TARIFF_SNAPSHOT_SCHEMA_VERSION: u32 = 2;

/// Why a tariff snapshot failed to restore. Every variant names its
/// values: the zone that cannot be parsed, the zone a summary was written
/// in against the zone it restores into, or the local wall-clock time that
/// does not exist in the zone it claims.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum SnapshotError {
    /// The snapshot's schema version is not this build's.
    #[error("tariff snapshot version mismatch: snapshot={snapshot}, expected={expected}")]
    Version { snapshot: u32, expected: u32 },
    /// The snapshot's zone name parses to no IANA zone.
    #[error("tariff snapshot: unknown time zone '{0}'")]
    UnknownZone(String),
    /// A closed period's summary was written in one zone and restores
    /// into an evaluator running in another.
    #[error(
        "tariff snapshot: summary zone '{snapshot_zone}' does not match the evaluator's zone '{live_zone}'"
    )]
    ZoneMismatch {
        snapshot_zone: String,
        live_zone: String,
    },
    /// The local wall-clock time and offset do not resolve in the zone: a
    /// spring-forward gap names the wall clock, the offset it was written
    /// with and the zone it fell in.
    #[error(
        "tariff snapshot: local instant {local} at offset {offset_seconds} s does not exist in '{zone}' \
         (a gap in the zone's local time, such as a spring-forward hour)"
    )]
    UnresolvedLocalInstant {
        local: String,
        offset_seconds: i32,
        zone: String,
    },
    /// The snapshot bytes, a bound's wall clock or a field of the tariff
    /// itself did not decode or validate.
    #[error("tariff snapshot: {0}")]
    Decode(String),
    /// The tariff, the horizon or a peak vector behind the snapshot was
    /// refused by the evaluator's own construction.
    #[error(transparent)]
    Tariff(#[from] HaresError),
}

impl From<SnapshotError> for HaresError {
    fn from(err: SnapshotError) -> Self {
        match err {
            SnapshotError::Tariff(inner) => inner,
            other => HaresError::Tariff(other.to_string()),
        }
    }
}

/// A local wall-clock time and the zone offset it was written with: the
/// checkpoint's timestamp form. A bare UTC instant would lose the wall
/// clock's own form; a bare wall clock would lose the offset a DST
/// fall-back ambiguity needs. Both travel, and the restore resolves the
/// wall clock in the zone with the offset picking the fall-back variant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalInstant {
    /// The local wall-clock time, `YYYY-MM-DDTHH:MM:SS`, no offset.
    pub wall_clock: String,
    /// The zone offset the wall clock was written with, seconds east of
    /// UTC.
    pub offset_seconds: i32,
}

impl LocalInstant {
    /// The local instant of a zone time: its wall clock and the offset it
    /// renders with.
    pub fn new<Tz: TimeZone>(dt: &DateTime<Tz>) -> Self {
        Self {
            wall_clock: dt.naive_local().format("%Y-%m-%dT%H:%M:%S").to_string(),
            offset_seconds: dt.offset().fix().local_minus_utc(),
        }
    }

    /// Resolves the local instant in `tz`: the wall clock's time in the
    /// zone, the stored offset picking the variant of a fall-back
    /// ambiguity. A gap (a wall clock the zone skips) is
    /// [`SnapshotError::UnresolvedLocalInstant`].
    ///
    /// # Errors
    ///
    /// A wall clock that is not `YYYY-MM-DDTHH:MM:SS`, or a wall clock the
    /// zone's local time skips.
    pub fn resolve(&self, tz: Tz) -> Result<DateTime<Tz>, SnapshotError> {
        let naive = NaiveDateTime::parse_from_str(&self.wall_clock, "%Y-%m-%dT%H:%M:%S").map_err(
            |err| SnapshotError::Decode(format!("local instant '{}': {err}", self.wall_clock)),
        )?;
        match tz.from_local_datetime(&naive) {
            chrono::LocalResult::None => Err(SnapshotError::UnresolvedLocalInstant {
                local: self.wall_clock.clone(),
                offset_seconds: self.offset_seconds,
                zone: tz.name().to_string(),
            }),
            chrono::LocalResult::Single(dt) => Ok(dt),
            chrono::LocalResult::Ambiguous(earlier, later) => {
                if earlier.offset().fix().local_minus_utc() == self.offset_seconds {
                    Ok(earlier)
                } else {
                    // A snapshot this file wrote always carries the zone's
                    // own offset (the offset the original instant rendered
                    // with), which matches one of the two variants; a
                    // foreign offset returns the later occurrence
                    // deterministically.
                    Ok(later)
                }
            }
        }
    }
}

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
    pub period_start: LocalInstant,
    pub period_end: LocalInstant,
    /// When the tariff became active within the open period: the period
    /// start, or the attach or switch instant for a period a tariff joined
    /// mid-way.
    pub active_since: LocalInstant,
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

/// A tariff evaluator's complete billing state: every field the stepping
/// and billing arithmetic reads or writes, so a resumed run's prices and
/// bills are bitwise the original run's. The observe feature's counters are
/// deliberately excluded: they report on the run, they do not feed it, and
/// a resumed run starts them at zero.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TariffSnapshot {
    pub schema_version: u32,
    pub tariff: ElectricTariff,
    /// IANA name of the evaluator's time zone.
    pub timezone: String,
    /// The simulation's start instant, in the evaluator's zone.
    pub simulation_start: LocalInstant,
    pub interval_seconds: u32,
    /// The number of precomputed steps; the simulation end is reconstructed
    /// from it, which recomputes the price arrays exactly as they were.
    pub total_steps: usize,
    pub step_index: usize,
    pub finished: bool,
    pub finalized: bool,
    pub billing: BillingStateSnapshot,
}

/// One closed billing period's summary as a checkpoint section: every
/// field of [`crate::billing::BillingPeriodSummary`], the bounds as local
/// instants in the zone they were written in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BillingPeriodSummarySnapshot {
    /// IANA name of the zone the summary's bounds were written in; the
    /// restore refuses a summary whose zone differs from the evaluator's.
    pub zone: String,
    pub period_start: LocalInstant,
    pub period_end: LocalInstant,
    pub energy_charge_usd: f64,
    pub demand_charge_usd: f64,
    pub fixed_charge_usd: f64,
    pub export_credit_usd: f64,
    pub net_bill_usd: f64,
    pub peak_demand_kw: f64,
    pub total_import_kwh: f64,
    pub total_export_kwh: f64,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::billing::BillingPeriodSummary;
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

    fn denver_summary_snapshot() -> BillingPeriodSummarySnapshot {
        use chrono_tz::America::Denver;
        let start = Denver.with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap();
        let end = Denver.with_ymd_and_hms(2023, 2, 1, 0, 0, 0).unwrap();
        BillingPeriodSummarySnapshot {
            zone: "America/Denver".into(),
            period_start: LocalInstant::new(&start),
            period_end: LocalInstant::new(&end),
            energy_charge_usd: 12.345_678_9,
            demand_charge_usd: 3.5,
            fixed_charge_usd: 10.0,
            export_credit_usd: 1.25,
            net_bill_usd: 24.595_678_9,
            peak_demand_kw: 4.75,
            total_import_kwh: 103.25,
            total_export_kwh: 21.5,
        }
    }

    /// The snapshot restore's three typed failures name their values: an
    /// unknown zone name, a summary restored into an evaluator in another
    /// zone, and a local instant falling in a spring-forward gap.
    #[test]
    fn snapshot_errors_name_the_zone_and_instant() {
        let mut snapshot = make_evaluator().snapshot_state();
        snapshot.timezone = "Not/AZone".to_string();
        let err = TariffEvaluator::from_snapshot(&snapshot)
            .map(|_| ())
            .unwrap_err();
        match err {
            SnapshotError::UnknownZone(zone) => {
                assert_eq!(zone, "Not/AZone", "the error names the zone's value");
            }
            other => panic!("expected UnknownZone, got: {other}"),
        }

        let err = BillingPeriodSummary::from_snapshot(
            denver_summary_snapshot(),
            chrono_tz::America::Chicago,
        )
        .map(|_| ())
        .unwrap_err();
        match err {
            SnapshotError::ZoneMismatch {
                snapshot_zone,
                live_zone,
            } => {
                assert_eq!(snapshot_zone, "America/Denver");
                assert_eq!(live_zone, "America/Chicago");
            }
            other => panic!("expected ZoneMismatch, got: {other}"),
        }

        let gap = LocalInstant {
            wall_clock: "2023-03-12T02:30:00".into(),
            offset_seconds: -25200,
        };
        let err = gap.resolve(chrono_tz::America::Denver).unwrap_err();
        match err {
            SnapshotError::UnresolvedLocalInstant {
                local,
                offset_seconds,
                zone,
            } => {
                assert_eq!(local, "2023-03-12T02:30:00");
                assert_eq!(offset_seconds, -25200);
                assert_eq!(zone, "America/Denver");
            }
            other => panic!("expected UnresolvedLocalInstant, got: {other}"),
        }
    }

    /// A closed billing period's summary round-trips through its snapshot
    /// on bits: every charge, the bounds as local wall-clock instants.
    #[test]
    fn billing_period_summary_round_trips_bitwise() {
        let snapshot = denver_summary_snapshot();
        let json = serde_json::to_string(&snapshot).expect("serialize the summary snapshot");
        let denver = chrono_tz::America::Denver;
        let original: BillingPeriodSummary =
            BillingPeriodSummary::from_snapshot(snapshot.clone(), denver).expect("restore");
        let decoded: BillingPeriodSummarySnapshot =
            serde_json::from_str(&json).expect("decode the summary snapshot");
        let restored = BillingPeriodSummary::from_snapshot(decoded, denver).expect("restore");

        assert_eq!(restored.period_start, original.period_start);
        assert_eq!(restored.period_end, original.period_end);
        assert_eq!(
            restored.energy_charge_usd.to_bits(),
            original.energy_charge_usd.to_bits()
        );
        assert_eq!(
            restored.demand_charge_usd.to_bits(),
            original.demand_charge_usd.to_bits()
        );
        assert_eq!(
            restored.fixed_charge_usd.to_bits(),
            original.fixed_charge_usd.to_bits()
        );
        assert_eq!(
            restored.export_credit_usd.to_bits(),
            original.export_credit_usd.to_bits()
        );
        assert_eq!(
            restored.net_bill_usd.to_bits(),
            original.net_bill_usd.to_bits()
        );
        assert_eq!(
            restored.peak_demand_kw.to_bits(),
            original.peak_demand_kw.to_bits()
        );
        assert_eq!(
            restored.total_import_kwh.to_bits(),
            original.total_import_kwh.to_bits()
        );
        assert_eq!(
            restored.total_export_kwh.to_bits(),
            original.total_export_kwh.to_bits()
        );
        // And the snapshot of the restored summary is the original snapshot.
        assert_eq!(restored.snapshot(), snapshot);
    }

    /// A local instant written from a real zone time resolves back to the
    /// same instant, and a fall-back ambiguity resolves by the stored
    /// offset.
    #[test]
    fn local_instant_resolves_by_its_stored_offset() {
        use chrono_tz::America::Denver;
        // A plain winter instant: round trip.
        let winter = Denver.with_ymd_and_hms(2023, 1, 28, 16, 30, 0).unwrap();
        let instant = LocalInstant::new(&winter);
        assert_eq!(instant.offset_seconds, -25200);
        assert_eq!(instant.wall_clock, "2023-01-28T16:30:00");
        assert_eq!(instant.resolve(Denver).expect("resolves"), winter);

        // The fall-back hour exists twice; the stored offset picks the
        // standard-time occurrence the original instant rendered with.
        let fall_back = Denver
            .with_ymd_and_hms(2023, 11, 5, 1, 30, 0)
            .latest()
            .expect("the standard-time variant");
        let ambiguous = LocalInstant::new(&fall_back);
        assert_eq!(ambiguous.offset_seconds, -25200);
        let resolved = ambiguous.resolve(Denver).expect("resolves");
        assert_eq!(
            resolved.offset().fix().local_minus_utc(),
            -25200,
            "the stored offset picks the standard-time variant"
        );
    }

    /// The fall-back's daylight-time occurrence resolves to itself by its
    /// stored offset (the ambiguity's other side, beside the
    /// standard-time one the test above pins), and a wall clock carrying
    /// a foreign offset (one the zone never rendered that hour) returns
    /// the later occurrence, as the resolution's own comment states.
    #[test]
    fn local_instant_resolves_the_dst_occurrence_and_a_foreign_offset_to_the_later() {
        use chrono_tz::America::Denver;
        // The fall-back hour's daylight-time occurrence (UTC-6): its own
        // offset picks it back out of the ambiguity.
        let dst = Denver
            .with_ymd_and_hms(2023, 11, 5, 1, 30, 0)
            .earliest()
            .expect("the daylight-time variant");
        let instant = LocalInstant::new(&dst);
        assert_eq!(instant.offset_seconds, -21600);
        let resolved = instant.resolve(Denver).expect("resolves");
        assert_eq!(
            resolved.offset().fix().local_minus_utc(),
            -21600,
            "the stored offset picks the daylight-time variant"
        );

        // A foreign offset: neither variant's, so the later occurrence.
        let foreign = LocalInstant {
            wall_clock: "2023-11-05T01:30:00".to_string(),
            offset_seconds: 3_600,
        };
        let resolved = foreign.resolve(Denver).expect("resolves");
        assert_eq!(
            resolved.offset().fix().local_minus_utc(),
            -25200,
            "a foreign offset returns the later occurrence"
        );
    }

    /// Every float in a full snapshot round-trips through the blob on
    /// bits, the accruals included (float_roundtrip parsing).
    #[test]
    fn snapshot_blob_round_trips_floats_bitwise() {
        let mut evaluator = make_evaluator();
        for i in 0..48 {
            let step_end = New_York.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap()
                + Duration::seconds((i as i64 + 1) * 3600);
            evaluator.step(2.5 + i as f64 * 0.031, 0.25, 3600.0, step_end);
        }
        let snapshot = evaluator.snapshot_state();
        let blob = snapshot.to_blob().expect("encode");
        let decoded = TariffSnapshot::from_blob(&blob).expect("decode");
        TariffEvaluator::from_snapshot(&decoded).expect("restore");
        let bits_of = |s: &TariffSnapshot| {
            (
                s.billing.cumulative_import_kwh.to_bits(),
                s.billing.cumulative_energy_cost_usd.to_bits(),
                s.billing.demand_window.running_sum.to_bits(),
            )
        };
        assert_eq!(bits_of(&decoded), bits_of(&snapshot));
    }
}
