//! Premise-level aggregate ZIP: the dwelling's real voltage response.
//!
//! Per-equipment `Equipment::resolved_zip()` answers "how does *this*
//! equipment respond to voltage" — but for an entire category of equipment
//! (Rule R1 physics models: HVAC, water heaters, ventilation; and DER) the
//! published real coefficients are a structural `(0, 0, 1)` pin, because
//! real power comes from the equipment's own physics or controller and is
//! deliberately voltage-invariant. Summing the published coefficients over
//! such equipment predicts zero premise sensitivity, while the dwelling's
//! ZIP-governed load (scheduled and event loads) measurably does respond.
//!
//! The aggregate here answers the premise question without simulation: the
//! power-weighted mean ZIP over the equipment whose real-power coefficients
//! genuinely govern their real power, plus an explicit roster of what is and
//! is not covered. It cannot know the *share* of total draw that
//! physics-driven equipment takes (that depends on weather, occupancy and
//! control, knowable only by simulation), so the aggregate describes the
//! ZIP-governed mix, and the caller combines it with physics-equipment
//! energy shares when they need whole-premise %P/%V.

use hares_equipment::{Equipment, ExpectedMeanPower};
use hares_io::schedule::ScheduleTimeSeries;
use hares_types::zip::ZipLoad;

/// The premise-level aggregate ZIP and the equipment roster it was formed
/// from. See [`crate::dwelling::Dwelling::premise_zip`].
#[derive(Clone, Debug, PartialEq)]
pub struct PremiseZip {
    /// Power-weighted aggregate ZIP over the ZIP-governed equipment with a
    /// computable expected draw (positive `governing` weights). The
    /// real-power coefficients sum to 1 by construction (each included
    /// equipment's row is init-validated to sum to 1). The reactive side
    /// (`zq`/`iq`/`pq`, `pf` via the mean of `tan(acos(pf))`) is a
    /// first-order aggregate, not an exact reduction.
    pub zip: ZipLoad,
    /// `(equipment name, expected mean power [kW])` for every ZIP-governed
    /// equipment. `None` when the expected draw cannot be determined
    /// without simulation (stochastic event scheduling without a kW
    /// series, or a schedule column absent from the loaded data); such
    /// equipment is listed but contributes no weight to [`PremiseZip::zip`].
    pub governing: Vec<(String, Option<f64>)>,
    /// Equipment whose real power is physics- or DER-driven
    /// (`real_power_zip_applies == false`): voltage-invariant by
    /// construction, so it contributes no voltage sensitivity and is
    /// excluded from the aggregate.
    pub constant_power: Vec<String>,
}

/// Mean of a schedule column over the loaded schedule data [kW], matching
/// the load-side semantics (finite values at or below zero draw no power).
/// `None` when the column index is out of range, the column is empty, or
/// the column contains non-finite (NaN/±inf) values: corrupt schedule data
/// makes the expected draw not computable, which the aggregation rosters
/// as `mean_power_kw: None` — distinguishable from a real weight — instead
/// of silently skewing the mean (`f64::max` would drop a NaN operand and
/// average it in as zero draw).
fn schedule_column_mean_kw(schedule: &ScheduleTimeSeries, col_idx: usize) -> Option<f64> {
    let column = schedule.columns.get(col_idx)?;
    if column.is_empty() || !column.iter().all(|v| v.is_finite()) {
        return None;
    }
    let sum: f64 = column.iter().map(|v| v.max(0.0)).sum();
    Some(sum / column.len() as f64)
}

/// Aggregate the premise-level ZIP over a dwelling's equipment. `None` when
/// no ZIP-governed equipment has a computable, positive expected draw — an
/// aggregate formed from no information would be indistinguishable from
/// "the premise is constant power", the exact conflation the
/// `real_power_zip_applies` flag exists to prevent.
pub(crate) fn aggregate_premise_zip(
    equipment: &[Box<dyn Equipment>],
    schedule: &ScheduleTimeSeries,
) -> Option<PremiseZip> {
    let mut governing: Vec<(String, Option<f64>)> = Vec::new();
    let mut constant_power: Vec<String> = Vec::new();

    // Weighted sums over the governing equipment with a known positive
    // expected draw: real/reactive coefficients, tan(acos(pf)) and v0.
    let mut total_weight = 0.0;
    let mut zp = 0.0;
    let mut ip = 0.0;
    let mut pp = 0.0;
    let mut zq = 0.0;
    let mut iq = 0.0;
    let mut pq = 0.0;
    let mut tan_phi_sum = 0.0;
    let mut v0 = 0.0;

    for eq in equipment {
        let Some(resolved) = eq.resolved_zip() else {
            // No electrical ZIP concept at all (generator, bridge, tank).
            continue;
        };
        let name = eq.descriptor().name.clone();
        if !resolved.real_power_zip_applies {
            constant_power.push(name);
            continue;
        }
        let mean_power_kw = match eq.expected_mean_power_kw() {
            Some(ExpectedMeanPower::Kw(kw)) => Some(kw),
            Some(ExpectedMeanPower::ScheduleColumn(col_idx)) => {
                schedule_column_mean_kw(schedule, col_idx)
            }
            None => None,
        };
        // The roster must publish only weights the aggregate actually
        // used: a load whose ZIP coefficients are non-finite (bypassing
        // the loader and init validation, e.g. a hand-built config) is
        // excluded from the weighted sums, so rostering its real positive
        // draw would claim a weight the aggregate silently ignored and
        // break the caller-side audit (recomputing the weighted mean from
        // the rostered positive weights must reproduce the aggregate).
        // Such a load takes the established unknown-mean semantics:
        // rostered by name with an explicitly absent weight.
        let zip = resolved.zip;
        let zip_is_finite = [
            zip.zp, zip.ip, zip.pp, zip.zq, zip.iq, zip.pq, zip.pf, zip.v0,
        ]
        .iter()
        .all(|v| v.is_finite());
        let roster_weight = if zip_is_finite { mean_power_kw } else { None };
        governing.push((name, roster_weight));
        if let Some(w) = mean_power_kw {
            if w > 0.0 && w.is_finite() && zip_is_finite {
                total_weight += w;
                zp += w * zip.zp;
                ip += w * zip.ip;
                pp += w * zip.pp;
                zq += w * zip.zq;
                iq += w * zip.iq;
                pq += w * zip.pq;
                tan_phi_sum += w * zip.tan_phi();
                v0 += w * zip.v0;
            }
        }
    }

    // `matches!(...partial_cmp...)` rather than `> 0.0`: a NaN total weight
    // (NaN schedule data) must land here — not positive — instead of
    // silently aggregating garbage coefficients.
    if !matches!(
        total_weight.partial_cmp(&0.0),
        Some(std::cmp::Ordering::Greater)
    ) {
        return None;
    }

    // pf aggregated through tan(acos(pf)): reactive power is proportional
    // to tan(phi), so the power-weighted mean of tan(phi) is the first-order
    // aggregate; pf = cos(atan(x)) = 1/sqrt(1 + x^2) recovers the magnitude.
    let tan_phi_agg = tan_phi_sum / total_weight;
    let pf_agg = 1.0 / (1.0 + tan_phi_agg * tan_phi_agg).sqrt();

    Some(PremiseZip {
        zip: ZipLoad {
            zp: zp / total_weight,
            ip: ip / total_weight,
            pp: pp / total_weight,
            zq: zq / total_weight,
            iq: iq / total_weight,
            pq: pq / total_weight,
            pf: pf_agg,
            v0: v0 / total_weight,
        },
        governing,
        constant_power,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use hares_types::{CoreOutput, EnvironmentState, OperatingMode, PortDeclaration, Telemetry};

    /// Minimal `Equipment` impl for exercising the aggregation across the
    /// trait boundary: a ZIP-governed or constant-power (Rule R1) load with
    /// a fixed expected mean draw.
    struct StubLoad {
        descriptor: hares_types::EquipmentDescriptor,
        resolved: Option<hares_types::zip::ResolvedZip>,
        mean: Option<ExpectedMeanPower>,
    }

    impl StubLoad {
        fn new(
            name: &str,
            resolved: Option<hares_types::zip::ResolvedZip>,
            mean: Option<ExpectedMeanPower>,
        ) -> Self {
            Self {
                descriptor: hares_types::EquipmentDescriptor {
                    id: hares_types::EquipmentId(0),
                    name: name.to_string(),
                    end_use: hares_types::EndUse::OTHER,
                    equipment_type: std::borrow::Cow::Borrowed("StubLoad"),
                    zone: None,
                    fuel: hares_types::FuelType::Electric,
                    stage: hares_types::ExecutionStage::Independent,
                    control_capabilities: hares_types::ControlCapabilities::empty(),
                    core_capabilities: hares_types::CoreCapabilities::ELECTRIC,
                    telemetry_fields: Vec::new(),
                    zone_type: None,
                },
                resolved,
                mean,
            }
        }
    }

    impl Equipment for StubLoad {
        fn descriptor(&self) -> &hares_types::EquipmentDescriptor {
            &self.descriptor
        }
        fn ports(&self) -> &[PortDeclaration] {
            unimplemented!()
        }
        fn init(
            &mut self,
            _config: &hares_equipment::EquipmentConfig,
            _env: &EnvironmentState,
        ) -> hares_equipment::Result<()> {
            unimplemented!()
        }
        fn update_control(&mut self, _env: &EnvironmentState) -> OperatingMode {
            unimplemented!()
        }
        fn step(
            &mut self,
            _env: &EnvironmentState,
            _dt: Duration,
            _ports: &mut hares_types::PortSlots,
        ) -> std::result::Result<(), hares_types::HaresError> {
            unimplemented!()
        }
        fn telemetry(&self) -> &Telemetry {
            unimplemented!()
        }
        fn core_output(&self) -> &CoreOutput {
            unimplemented!()
        }
        fn save_state(&self) -> hares_equipment::Result<Vec<u8>> {
            unimplemented!()
        }
        fn load_state(&mut self, _state: &[u8]) -> hares_equipment::Result<()> {
            unimplemented!()
        }
        fn rename(&mut self, _name: String) {
            unimplemented!()
        }
        fn apply_control_unchecked(
            &mut self,
            _signal: &hares_types::ControlSignal,
        ) -> hares_equipment::Result<()> {
            unimplemented!()
        }
        fn resolved_zip(&self) -> Option<hares_types::zip::ResolvedZip> {
            self.resolved
        }
        fn expected_mean_power_kw(&self) -> Option<ExpectedMeanPower> {
            self.mean
        }
    }

    fn governing_load(name: &str, zip: ZipLoad, mean: Option<ExpectedMeanPower>) -> Box<StubLoad> {
        Box::new(StubLoad::new(
            name,
            Some(hares_types::zip::ResolvedZip::governing(zip)),
            mean,
        ))
    }

    fn pinned_load(name: &str) -> Box<StubLoad> {
        Box::new(StubLoad::new(
            name,
            Some(hares_types::zip::ResolvedZip::reactive_only(
                ZipLoad::constant_power(),
            )),
            None,
        ))
    }

    fn empty_schedule() -> ScheduleTimeSeries {
        ScheduleTimeSeries {
            timestamps: Vec::new(),
            column_names: vec!["lighting".to_string()],
            columns: vec![vec![1.0, 3.0]],
            column_index: std::collections::HashMap::new(),
            source_step_secs: 60,
            column_aggregations: Vec::new(),
        }
    }

    #[test]
    fn aggregate_weights_by_expected_mean_and_rosters_regimes() {
        // Two governing loads: lighting (1 kW, zp=1 → constant impedance) and
        // a 3 kW constant-power scheduled load, plus a Rule R1 HVAC unit.
        let lighting = hares_types::zip::zip_defaults_for_class("Lighting").expect("row");
        let equipment: Vec<Box<dyn Equipment>> = vec![
            governing_load(
                "Indoor Lighting",
                lighting,
                Some(ExpectedMeanPower::ScheduleColumn(0)),
            ),
            governing_load(
                "Always On",
                ZipLoad::constant_power(),
                Some(ExpectedMeanPower::Kw(3.0)),
            ),
            pinned_load("Gas Furnace"),
        ];
        // Column mean = (1 + 3)/2 = 2 kW vs 3 kW for the constant load.
        let premise = aggregate_premise_zip(&equipment, &empty_schedule())
            .expect("positive governing weights aggregate");
        let total = 5.0;
        assert_eq!(premise.zip.zp, (2.0 * lighting.zp + 3.0 * 0.0) / total);
        assert_eq!(premise.zip.pp, (2.0 * lighting.pp + 3.0 * 1.0) / total);
        assert!((premise.zip.zp + premise.zip.ip + premise.zip.pp - 1.0).abs() < 1e-12);
        assert_eq!(
            premise.governing,
            vec![
                ("Indoor Lighting".to_string(), Some(2.0)),
                ("Always On".to_string(), Some(3.0)),
            ]
        );
        assert_eq!(premise.constant_power, vec!["Gas Furnace".to_string()]);
    }

    #[test]
    fn unknown_mean_power_is_listed_but_not_weighted() {
        let lighting = hares_types::zip::zip_defaults_for_class("Lighting").expect("row");
        let equipment: Vec<Box<dyn Equipment>> = vec![
            governing_load("MELs", lighting, None),
            governing_load(
                "Always On",
                ZipLoad::constant_power(),
                Some(ExpectedMeanPower::Kw(2.0)),
            ),
        ];
        let premise = aggregate_premise_zip(&equipment, &empty_schedule()).expect("aggregates");
        // Only the known-weight load shapes the aggregate: pure constant power.
        assert_eq!(
            (premise.zip.zp, premise.zip.ip, premise.zip.pp),
            (0.0, 0.0, 1.0)
        );
        assert_eq!(premise.governing[0], ("MELs".to_string(), None));
    }

    #[test]
    fn no_computable_draw_is_none_not_constant_power() {
        // Every governing load has an unknowable expected draw: an aggregate
        // formed from no information must not present itself as constant
        // power — the conflation the real_power_zip_applies flag prevents.
        let lighting = hares_types::zip::zip_defaults_for_class("Lighting").expect("row");
        let equipment: Vec<Box<dyn Equipment>> = vec![
            governing_load("MELs", lighting, None),
            pinned_load("Air Conditioner"),
        ];
        assert!(aggregate_premise_zip(&equipment, &empty_schedule()).is_none());
    }

    #[test]
    fn zero_expected_draw_is_none_not_constant_power() {
        let lighting = hares_types::zip::zip_defaults_for_class("Lighting").expect("row");
        let equipment: Vec<Box<dyn Equipment>> = vec![governing_load(
            "Seasonal Pump",
            lighting,
            Some(ExpectedMeanPower::Kw(0.0)),
        )];
        assert!(aggregate_premise_zip(&equipment, &empty_schedule()).is_none());
    }

    #[test]
    fn equipment_without_zip_concept_is_silent() {
        let equipment: Vec<Box<dyn Equipment>> =
            vec![Box::new(StubLoad::new("Generator", None, None))];
        assert!(aggregate_premise_zip(&equipment, &empty_schedule()).is_none());
    }

    #[test]
    fn schedule_column_mean_clamps_and_averages() {
        let schedule = ScheduleTimeSeries {
            timestamps: Vec::new(),
            column_names: vec!["lighting".to_string()],
            columns: vec![vec![2.0, -1.0, 4.0]],
            column_index: std::collections::HashMap::new(),
            source_step_secs: 60,
            column_aggregations: Vec::new(),
        };
        // Negative placeholder values draw no power, matching load semantics.
        assert_eq!(schedule_column_mean_kw(&schedule, 0), Some(2.0));
        assert_eq!(schedule_column_mean_kw(&schedule, 1), None);
    }

    #[test]
    fn nan_schedule_data_must_not_degrade_to_a_silently_skewed_mean() {
        // A schedule column containing NaN (reachable: the CSV loader parses
        // with `str::parse::<f64>()`, which accepts "nan") must not be
        // averaged as if the NaN timesteps were zero draw — that publishes a
        // plausible but wrong premise weight with no signal to the caller.
        // The honest degradation is `None` (expected draw not computable):
        // the equipment is then rostered with `mean_power_kw: None`, which
        // is distinguishable from a real weight.
        let schedule = ScheduleTimeSeries {
            timestamps: Vec::new(),
            column_names: vec!["lighting".to_string()],
            columns: vec![vec![2.0, f64::NAN, 4.0]],
            column_index: std::collections::HashMap::new(),
            source_step_secs: 60,
            column_aggregations: Vec::new(),
        };
        assert_eq!(
            schedule_column_mean_kw(&schedule, 0),
            None,
            "NaN schedule data must degrade to 'no computable mean', not a \
             silently undercounted average"
        );
    }

    #[test]
    fn infinite_schedule_data_is_also_not_a_computable_mean() {
        // "inf" parses through the CSV loader exactly like "nan"; the
        // non-finite guard must reject it the same way.
        for bad in [f64::INFINITY, f64::NEG_INFINITY] {
            let schedule = ScheduleTimeSeries {
                timestamps: Vec::new(),
                column_names: vec!["lighting".to_string()],
                columns: vec![vec![2.0, bad, 4.0]],
                column_index: std::collections::HashMap::new(),
                source_step_secs: 60,
                column_aggregations: Vec::new(),
            };
            assert_eq!(schedule_column_mean_kw(&schedule, 0), None);
        }
    }

    #[test]
    fn nan_column_rosters_equipment_without_weight_not_as_zero_draw() {
        // Observable at the aggregate level: an equipment whose schedule
        // column contains NaN stays on the governing roster with an
        // explicitly absent weight (`mean_power_kw: None`), so the caller
        // can see the degradation — it must not silently vanish from the
        // roster, nor contribute a clamped-to-zero weight.
        let lighting = hares_types::zip::zip_defaults_for_class("Lighting").expect("row");
        let nan_schedule = ScheduleTimeSeries {
            timestamps: Vec::new(),
            column_names: vec!["lighting".to_string()],
            columns: vec![vec![1.0, f64::NAN]],
            column_index: std::collections::HashMap::new(),
            source_step_secs: 60,
            column_aggregations: Vec::new(),
        };
        let equipment: Vec<Box<dyn Equipment>> = vec![
            governing_load(
                "Indoor Lighting",
                lighting,
                Some(ExpectedMeanPower::ScheduleColumn(0)),
            ),
            governing_load(
                "Always On",
                ZipLoad::constant_power(),
                Some(ExpectedMeanPower::Kw(2.0)),
            ),
        ];
        let premise = aggregate_premise_zip(&equipment, &nan_schedule).expect("aggregates");
        assert_eq!(
            premise.governing[0],
            ("Indoor Lighting".to_string(), None),
            "NaN schedule data must surface as an explicitly absent weight"
        );
        // Only the 2 kW constant-power load shapes the aggregate.
        assert_eq!(
            (premise.zip.zp, premise.zip.ip, premise.zip.pp),
            (0.0, 0.0, 1.0)
        );
    }

    #[test]
    fn non_finite_coefficients_are_never_published_as_an_aggregate() {
        // The aggregation guards its weights against NaN (the
        // `total_weight.partial_cmp` arm) but not its coefficients: a
        // governing load with a NaN coefficient (reachable — the defaults
        // TOML sidecar parses `nan` and `validate_zip_sums`'s
        // comparison-based checks pass NaN) yields a finite, positive
        // total_weight, so the weight guard does not trip and the
        // published aggregate carries NaN where numbers belong — a
        // normal-looking dict (roster, weights, `real_power_zip_applies`)
        // wrapping garbage, the exact "silently aggregating garbage
        // coefficients" hazard the weight guard's comment names. A
        // published `Some` must be all-finite; refusing (None) is the
        // honest alternative.
        let lighting = hares_types::zip::zip_defaults_for_class("Lighting").expect("row");
        let corrupt = hares_types::zip::ZipLoad {
            zp: f64::NAN,
            ..lighting
        };
        let equipment: Vec<Box<dyn Equipment>> = vec![
            governing_load(
                "Corrupt Sidecar Load",
                corrupt,
                Some(ExpectedMeanPower::Kw(2.0)),
            ),
            governing_load(
                "Always On",
                ZipLoad::constant_power(),
                Some(ExpectedMeanPower::Kw(1.0)),
            ),
        ];
        match aggregate_premise_zip(&equipment, &empty_schedule()) {
            None => {}
            Some(premise) => {
                for (label, value) in [
                    ("zp", premise.zip.zp),
                    ("ip", premise.zip.ip),
                    ("pp", premise.zip.pp),
                    ("zq", premise.zip.zq),
                    ("iq", premise.zip.iq),
                    ("pq", premise.zip.pq),
                    ("pf", premise.zip.pf),
                    ("v0", premise.zip.v0),
                ] {
                    assert!(
                        value.is_finite(),
                        "premise aggregate {label} = {value}: a non-finite coefficient must \
                         not be published inside a structurally normal aggregate"
                    );
                }
            }
        }
    }

    #[test]
    fn rostered_positive_weights_are_the_weights_the_aggregate_used() {
        // The audit contract a caller relies on (pinned end-to-end by
        // `test_premise_aggregate_is_weighted_mean_of_governing_
        // coefficients` in tests/python): recomputing the weighted mean
        // from the rostered positive `mean_power_kw` entries and each
        // equipment's published ZIP must reproduce the published
        // aggregate — "any divergence means the aggregate silently used
        // different coefficients or weights than it published". A
        // corrupt-coefficient load excluded from the weighted sums but
        // rostered with its real, positive draw (`Some(2.0)`) breaks that
        // contract: the roster claims a weight the aggregate did not use,
        // and the caller's recomputation diverges from the published
        // coefficients. The honest degradation matches the established
        // `None` semantics — rostered with an explicitly absent weight
        // ("contributes no weight") — or exclusion, never a positive
        // weight that was silently ignored.
        let lighting = hares_types::zip::zip_defaults_for_class("Lighting").expect("row");
        let corrupt = hares_types::zip::ZipLoad {
            zp: f64::NAN,
            ..lighting
        };
        let honest = ZipLoad::constant_power();
        let zips_by_name = [("Corrupt Sidecar Load", corrupt), ("Honest Load", honest)];
        let equipment: Vec<Box<dyn Equipment>> = vec![
            governing_load(
                "Corrupt Sidecar Load",
                corrupt,
                Some(ExpectedMeanPower::Kw(2.0)),
            ),
            governing_load("Honest Load", honest, Some(ExpectedMeanPower::Kw(1.0))),
        ];
        let premise = aggregate_premise_zip(&equipment, &empty_schedule())
            .expect("the honest load has a positive computable draw");

        // Caller-side audit: weighted mean over the rostered positive
        // weights, using each rostered equipment's ZIP (the per-equipment
        // API publishes the same coefficients the aggregation saw).
        let mut total = 0.0;
        let mut expected_zp = 0.0;
        for (name, weight) in &premise.governing {
            if let Some(w) = weight
                && *w > 0.0
                && w.is_finite()
            {
                let zip = zips_by_name
                    .iter()
                    .find(|(n, _)| n == name)
                    .expect("rostered names come from the equipment list")
                    .1;
                total += w;
                expected_zp += w * zip.zp;
            }
        }
        assert!(
            expected_zp.is_finite(),
            "a rostered positive weight ({:?}) includes a load whose coefficients \
             poison the recomputation — the roster publishes a weight the \
             aggregate silently did not use",
            premise.governing
        );
        assert!(
            (expected_zp / total - premise.zip.zp).abs() < 1e-12,
            "published aggregate zp = {} diverges from the weighted mean of \
             the rostered weights ({}): the roster and the aggregate must \
             tell one story",
            premise.zip.zp,
            expected_zp / total
        );
    }

    #[test]
    fn empty_column_has_no_mean() {
        let schedule = ScheduleTimeSeries {
            timestamps: Vec::new(),
            column_names: Vec::new(),
            columns: vec![Vec::new()],
            column_index: std::collections::HashMap::new(),
            source_step_secs: 60,
            column_aggregations: Vec::new(),
        };
        assert_eq!(schedule_column_mean_kw(&schedule, 0), None);
    }
}
