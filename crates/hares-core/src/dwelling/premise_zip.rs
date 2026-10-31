//! Premise-level aggregate ZIP: the dwelling's real voltage response.
//!
//! Per-equipment `Equipment::resolved_zip()` answers "how does *this*
//! equipment respond to voltage": every ZIP-bearing equipment class
//! carries its class row, and the row's real coefficients scale that
//! equipment's draw at the bus voltage (the scheduled loads scale their
//! scheduled draw; the typed physics equipment scales its
//! physics-computed draw through the recorded deep-sag form; the DER
//! controllers are reactive-only and never scale).
//!
//! The aggregate here answers the premise question: the power-weighted
//! mean over the equipment whose real-power coefficients genuinely
//! govern their real power, plus an explicit roster of what is and is not
//! covered. The weights are the moment's raw draws when the dwelling has
//! stepped (each equipment's last core-output electric draw: a consumed
//! value, a bidirectional flow's positive part, or zero for a valid
//! generation report; a value outside its own flow's contract is broken
//! core state and carries no weight), un-scaled through the equipment's
//! assembled deep-sag response at the bus voltage (the equipment's own
//! nominal draw when it reports one: a multi-component unit, or a state
//! its row does not scale such as a commanded power override; the
//! resolved row's un-scale otherwise, the moment's composition)),
//! falling back to the expected mean draw over the loaded schedule for a
//! dwelling that has not stepped (the static route). The aggregate
//! cannot know the *share* of total draw that physics-driven equipment
//! takes without simulation, so before the first step the weights are the
//! schedule's expected means; after it, the aggregate is the moment's
//! composition and `deep_sag_scale` is the number the stepping engine
//! answers at the current bus voltage, with each component's deep-sag
//! form applied (a held refrigerator row included), which no single
//! published coefficient row can reproduce when evaluated raw.
//!
//! At the de-energized boundary (the channel's admitted 0.0 pu outage
//! value) the aggregate reports the de-energized state: a load that
//! stepped and drew zero carries that zero weight, and no expected mean
//! stands in for any load: the engine draws zero from every load on a
//! de-energized bus, so no governing equipment carries a positive weight
//! and the aggregate is `None`. On an energized bus the same answer
//! follows from a non-positive row evaluation: the classes that publish an
//! expectation scale their reference draw by the row and report the draw
//! clamped at zero, so an expectation whose row evaluates at or below zero
//! at the bus voltage (the near-dead edge) carries no weight either. A
//! stepped report that is not a draw (a negative watt value, a non-finite
//! one, or a positive draw whose assembled response is not a finite positive
//! ratio: broken core state, or a report and base of different magnitudes)
//! is no evidence of an observed zero either: it carries no weight and no
//! expectation stands in for it.

use hares_equipment::{Equipment, ExpectedMeanPower};
use hares_io::schedule::ScheduleTimeSeries;
use hares_types::zip::ZipLoad;

/// The premise-level aggregate ZIP and the equipment roster it was formed
/// from. See [`crate::dwelling::Dwelling::premise_zip`].
#[derive(Clone, Debug, PartialEq)]
pub struct PremiseZip {
    /// Power-weighted aggregate ZIP over the ZIP-governing equipment with a
    /// computable weight (a positive moment draw, or a positive expected
    /// mean when the dwelling has not stepped). The real-power coefficients
    /// sum to 1 by construction (each included equipment's row is
    /// init-validated to sum to 1). The reactive side
    /// (`zq`/`iq`/`pq`, `pf` via the mean of `tan(acos(pf))`) is a
    /// first-order aggregate, not an exact reduction.
    pub zip: ZipLoad,
    /// `(equipment name, weighting draw [W])` for every ZIP-governing
    /// equipment. The weight is the moment's raw draw (the last step's
    /// core-output electric draw: a consumed value, a bidirectional flow's
    /// positive part, or zero for a valid generation report, un-scaled
    /// through the equipment's assembled deep-sag response at the bus
    /// voltage: the equipment's own nominal draw when it reports one (a
    /// multi-component unit, or a state its row does not scale such as a
    /// commanded power override), the resolved row's un-scale otherwise,
    /// which recovers the reference draw
    /// to within the rounding of one division) when the dwelling has
    /// stepped (a stepped zero draw is a zero weight, the 0.0 pu outage
    /// boundary included), else, on an energized bus only, the expected
    /// mean draw over the loaded schedule; `None` when neither is
    /// computable (stochastic event scheduling without a kW series, a
    /// schedule column absent from the loaded data, a de-energized bus with
    /// no stepped draw, a row evaluation the bus cannot supply a draw
    /// at, a stepped report that is not a finite non-negative draw, or a
    /// stepped report whose assembled response is not a finite positive
    /// ratio); such equipment is listed but contributes no weight to
    /// [`PremiseZip::zip`].
    pub governing: Vec<(String, Option<f64>)>,
    /// Equipment whose real power is DER-controller-driven
    /// (`real_power_zip_applies == false`): voltage-invariant by
    /// construction, so it contributes no voltage sensitivity and is
    /// excluded from the aggregate.
    pub constant_power: Vec<String>,
    /// The assembled governed response at the bus voltage the aggregation
    /// ran at: the weighting-draw-weighted mean of each governing
    /// equipment's own deep-sag response: the components' actual evaluated
    /// draws over the components' nominal draws for a stepped
    /// multi-component unit, the resolved row's recorded deep-sag form for
    /// a row-scaled step and on the static route, and the flat identity for
    /// a step the row does not scale (a commanded power override). This is
    /// the number the stepping engine answers for the moment's composition;
    /// no single published coefficient row can reproduce it when evaluated
    /// raw, because a held deep-sag row is not a quadratic and a
    /// multi-component unit's blend is not any one of its rows. Every
    /// weighted response is finite and positive, so a published scale is a
    /// positive multiplier, never a negative or non-finite one. `None` when
    /// no governing equipment carries a positive weight.
    pub deep_sag_scale: Option<f64>,
}

/// Mean of a schedule column over the loaded schedule data [kW], matching
/// the load-side semantics (finite values at or below zero draw no power).
/// `None` when the column index is out of range, the column is empty, or
/// the column contains non-finite (NaN/±inf) values: corrupt schedule data
/// makes the expected draw not computable, which the aggregation rosters
/// as `mean_power_w: None` — distinguishable from a real weight — instead
/// of silently skewing the mean (`f64::max` would drop a NaN operand and
/// average it in as zero draw).
fn schedule_column_mean_w(
    schedule: &ScheduleTimeSeries,
    col_idx: usize,
    upsample_factor: u32,
) -> Option<f64> {
    debug_assert!(upsample_factor >= 1, "the rows' expansion is at least 1");
    let column = schedule.columns.get(col_idx)?;
    if column.is_empty() || !column.iter().all(|v| v.is_finite()) {
        return None;
    }
    // The rows' expansion: a held row stands for that many sim steps, so
    // the mean runs over the per-step sequence -- each row's value repeated
    // in order, the sum the materialised rows produced.
    let factor = upsample_factor as usize;
    let sum: f64 = column
        .iter()
        .flat_map(|v| std::iter::repeat_n(v.max(0.0), factor))
        .sum();
    Some(sum / (column.len() * factor) as f64)
}

/// The step's own report about its electric draw, as the aggregation sees
/// it. `Stepped` records that the equipment published a consumption this
/// step, even when the value is not a draw or its assembled response is not
/// a usable ratio: the expected mean is pre-step evidence, so it may stand
/// in only for a load that reported no consumption at all.
#[derive(Clone, Copy)]
enum MomentDraw {
    /// The equipment published no consumption this step (it has not
    /// stepped, or it reports another flow): the expectation route is the
    /// only evidence.
    NotReported,
    /// The equipment published a consumption: the moment's reference draw
    /// when it is usable, and the assembled response that weights it. Both
    /// are `None` when the report is not a draw (a negative watt value,
    /// NaN, an infinite draw) or its assembled response is not a finite
    /// positive ratio.
    Stepped {
        weight_w: Option<f64>,
        response: Option<f64>,
    },
}

/// Aggregate the premise-level ZIP over a dwelling's equipment. `None` when
/// no ZIP-governing equipment has a computable, positive weight, or when the
/// weighted sums are not finite (a report large enough to overflow them): an
/// aggregate formed from no information would be indistinguishable from
/// "the premise is constant power", the exact conflation the
/// `real_power_zip_applies` flag exists to prevent, and a non-finite mean is
/// no mean.
pub(crate) fn aggregate_premise_zip(
    equipment: &[Box<dyn Equipment>],
    schedule: &ScheduleTimeSeries,
    upsample_factor: u32,
    bus_voltage_pu: f64,
) -> Option<PremiseZip> {
    let mut governing: Vec<(String, Option<f64>)> = Vec::new();
    let mut constant_power: Vec<String> = Vec::new();

    // Weighted sums over the governing equipment with a known positive
    // weight: real/reactive coefficients, tan(acos(pf)) and v0.
    let mut total_weight = 0.0;
    let mut zp = 0.0;
    let mut ip = 0.0;
    let mut pp = 0.0;
    let mut zq = 0.0;
    let mut iq = 0.0;
    let mut pq = 0.0;
    let mut tan_phi_sum = 0.0;
    let mut v0 = 0.0;
    let mut deep_sag_weighted = 0.0;

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
        // The moment's raw draw (the weight) and the deep-sag response it
        // weights with. A stepped dwelling aggregates the moment's
        // composition: the weight is the stepped draw un-scaled through its
        // ASSEMBLED deep-sag response: the equipment's own nominal draw
        // when it reports one, else the resolved row's un-scale, which
        // recovers a single-row equipment's reference draw to within the
        // rounding of one division (a multi-component unit evaluates each
        // component through its own row, so the unit row's multiplier alone
        // does not un-scale the blend). The response weighted with it is
        // the assembled ratio or the row's evaluation. A stepped zero
        // draw is a real zero weight at any bus voltage, the 0.0 pu outage
        // boundary included, where the stepped state is zero draw; a
        // positive draw cannot be un-scaled there (a de-energized bus
        // evaluates no draw response), and an unstepped load falls back to
        // the expected mean over the loaded schedule, on an energized bus
        // only: at the boundary the engine draws zero from every load, and
        // an expected mean published as the weight would report a draw the
        // bus cannot supply. On an energized bus a row evaluation that is
        // not finite and positive is a draw the bus cannot supply too: the
        // classes that publish an expectation scale their reference draw by
        // the row and report it clamped at zero, so the expectation carries
        // no weight.
        // The equipment's reported consumption this step. Every flow variant
        // is a report of the moment: a bidirectional flow consumes its
        // positive part (a zero or negative value is an idle or exporting
        // moment that draws nothing), and a generation report is the load
        // exporting, so it consumed nothing either. Only an absent flow is
        // "not reported", the one state the expectation route may cover. A
        // broken value (a non-finite one, a negative consumption, or a
        // generation value outside its non-negative export contract) stays
        // broken, so the roster's filter refuses it rather than laundering
        // it into an observed zero.
        let reported_draw_w = match eq.core_output().flows.electric_w {
            Some(hares_types::ElectricPower::Consumption(w)) => Some(w),
            Some(hares_types::ElectricPower::Bidirectional(w)) => {
                Some(if w.is_finite() { w.max(0.0) } else { w })
            }
            // A valid generation report is an observed zero consumption. A
            // value that violates the type's non-negative export contract
            // is broken core state, not a smaller export (the type's own
            // `net_consumption_w` reads a negative export as a
            // consumption): the report travels to the roster's
            // finite-and-non-negative filter unchanged, so it publishes no
            // weight rather than a fabricated observed zero.
            Some(hares_types::ElectricPower::Generation(w)) => {
                Some(if w.is_finite() && w >= 0.0 { 0.0 } else { w })
            }
            None => None,
        };
        let moment = match reported_draw_w {
            Some(w) if bus_voltage_pu > 0.0 && w > 0.0 => {
                // A composed base is usable only when it is a finite
                // positive draw: the trait is a public hook, and a report of
                // zero, NaN or infinity would divide the draw into a
                // non-finite "response" published inside a structurally
                // normal aggregate. Any other report is no assembled base,
                // and the row's evaluation is the only single-row route.
                match eq
                    .nominal_electric_draw_w()
                    .filter(|nominal_w| nominal_w.is_finite() && *nominal_w > 0.0)
                {
                    Some(nominal_w) => {
                        let response = w / nominal_w;
                        if response.is_finite() && response > 0.0 {
                            MomentDraw::Stepped {
                                weight_w: Some(nominal_w),
                                response: Some(response),
                            }
                        } else {
                            // The report dwarfs or is dwarfed by its base:
                            // the ratio is not a response the engine can
                            // answer. The equipment still observed this
                            // draw, so it carries no weight and no
                            // expectation stands in for it.
                            MomentDraw::Stepped {
                                weight_w: None,
                                response: None,
                            }
                        }
                    }
                    None => {
                        let multiplier = resolved.deep_sag_real_multiplier(bus_voltage_pu);
                        if multiplier.is_finite() && multiplier > 0.0 {
                            MomentDraw::Stepped {
                                weight_w: Some(w / multiplier),
                                response: Some(multiplier),
                            }
                        } else {
                            MomentDraw::Stepped {
                                weight_w: None,
                                response: None,
                            }
                        }
                    }
                }
            }
            // An energized bus's observed zero is a zero weight. A negative
            // or non-finite stepped report is broken, not an observed zero:
            // it keeps its own value so the roster's finite-and-non-negative
            // filter publishes no weight, and the expected mean never stands
            // in for a stepped moment (a fabricated zero would tell the
            // caller the moment was inspected and came up empty).
            Some(w) if bus_voltage_pu > 0.0 => MomentDraw::Stepped {
                weight_w: Some(w),
                response: None,
            },
            // The de-energized boundary: the bus draws zero from every
            // load, so the stepped state is a zero weight whatever the
            // report carries.
            Some(_) => MomentDraw::Stepped {
                weight_w: Some(0.0),
                response: None,
            },
            None => MomentDraw::NotReported,
        };
        let (moment_raw_draw_w, moment_response) = match moment {
            MomentDraw::Stepped { weight_w, response } => (weight_w, response),
            MomentDraw::NotReported => (None, None),
        };
        let mean_power_w = match moment {
            MomentDraw::Stepped {
                weight_w: Some(w), ..
            } => Some(w),
            // A stepped moment's expectation never stands in for its own
            // report, whatever the report carries.
            MomentDraw::Stepped { weight_w: None, .. } => None,
            MomentDraw::NotReported if bus_voltage_pu > 0.0 => match eq.expected_mean_power_w() {
                Some(ExpectedMeanPower::W(w)) => Some(w),
                Some(ExpectedMeanPower::ScheduleColumn(col_idx)) => {
                    schedule_column_mean_w(schedule, col_idx, upsample_factor)
                }
                None => None,
            },
            MomentDraw::NotReported => None,
        };
        // The response the weight weights with: the moment's assembled ratio
        // (the stepped draw over the moment's nominal draw) for a stepped
        // load, the resolved row's evaluation on the static route. A row
        // evaluation that is not finite and positive is a load the engine
        // draws nothing from at this bus: every class that publishes an
        // expectation (the scheduled and event loads) scales its reference
        // draw by the row and publishes the draw clamped at zero, so the
        // expectation carries no weight, the answer the de-energized
        // boundary gives one voltage down.
        let deep_sag_response = match moment_response {
            Some(response) => Some(response),
            None => {
                let row = resolved.deep_sag_real_multiplier(bus_voltage_pu);
                (row.is_finite() && row > 0.0).then_some(row)
            }
        };
        // The roster must publish only weights the aggregate actually used:
        // a load whose ZIP coefficients are non-finite (bypassing the loader
        // and init validation, e.g. a hand-built config) is excluded from
        // the weighted sums, so rostering its real positive draw would claim
        // a weight the aggregate silently ignored and break the caller-side
        // audit (recomputing the weighted mean from the rostered positive
        // weights must reproduce the aggregate). Such a load takes the
        // established unknown-mean semantics: rostered by name with an
        // explicitly absent weight. A load whose reference draw the bus
        // cannot supply carries no weight for the same reason: the
        // expectation was not used.
        let zip = resolved.zip;
        let zip_is_finite = [
            zip.zp, zip.ip, zip.pp, zip.zq, zip.iq, zip.pq, zip.pf, zip.v0,
        ]
        .iter()
        .all(|v| v.is_finite());
        let roster_weight = if !zip_is_finite {
            None
        } else {
            match moment_raw_draw_w {
                // A stepped moment is evidence of its own weight, a zero
                // draw included; a report that is not a finite
                // non-negative draw (a negative watt value, NaN, or an
                // infinite draw) is broken: no weight, and no expected
                // mean stands in for a stepped moment.
                Some(w) => (w.is_finite() && w >= 0.0).then_some(w),
                // An expectation is a weight only when the row's response is
                // a draw the energized bus can supply.
                None => mean_power_w
                    .filter(|w| w.is_finite())
                    .filter(|_| deep_sag_response.is_some()),
            }
        };
        governing.push((name, roster_weight));
        if let Some(w) = roster_weight
            && w > 0.0
            && let Some(deep_sag_response) = deep_sag_response
        {
            total_weight += w;
            zp += w * zip.zp;
            ip += w * zip.ip;
            pp += w * zip.pp;
            zq += w * zip.zq;
            iq += w * zip.iq;
            pq += w * zip.pq;
            tan_phi_sum += w * zip.tan_phi();
            v0 += w * zip.v0;
            deep_sag_weighted += w * deep_sag_response;
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

    // A published aggregate is finite: the weights are finite positive
    // draws and the responses finite positive multipliers, but a weighted
    // product or a sum can still overflow (a huge draw times a legal large
    // coefficient, or enough finite loads to overflow the total). A
    // non-finite numerator divides into NaN or infinity inside a
    // structurally normal struct whose fields claim a weighted mean and a
    // positive finite scale. A non-finite mean is not a mean: refuse, the
    // same no-computable-weight answer the total-weight guard gives,
    // rather than publish garbage.
    if ![
        total_weight,
        zp,
        ip,
        pp,
        zq,
        iq,
        pq,
        tan_phi_sum,
        v0,
        deep_sag_weighted,
    ]
    .iter()
    .all(|value| value.is_finite())
    {
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
        deep_sag_scale: Some(deep_sag_weighted / total_weight),
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
        core: hares_types::CoreOutput,
        nominal: Option<f64>,
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
                core: hares_types::CoreOutput::default(),
                nominal: None,
            }
        }

        fn with_last_draw(mut self, w: f64) -> Self {
            self.core.flows.electric_w = Some(hares_types::ElectricPower::Consumption(w));
            self
        }

        fn with_last_flow(mut self, power: hares_types::ElectricPower) -> Self {
            self.core.flows.electric_w = Some(power);
            self
        }

        fn with_nominal_draw(mut self, w: f64) -> Self {
            self.nominal = Some(w);
            self
        }
    }

    impl Equipment for StubLoad {
        fn descriptor(&self) -> &hares_types::EquipmentDescriptor {
            &self.descriptor
        }

        fn nominal_electric_draw_w(&self) -> Option<f64> {
            self.nominal
        }

        fn resolved_parameters(&self) -> Vec<hares_types::ResolvedParameter> {
            Vec::new()
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
            &self.core
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
        fn set_equipment_id(
            &mut self,
            id: hares_types::EquipmentId,
        ) -> hares_equipment::Result<()> {
            hares_equipment::apply_identity_write(self.is_initialized(), &mut self.descriptor, id)
        }
        fn apply_signal(
            &mut self,
            _signal: &hares_types::ControlSignal,
        ) -> std::result::Result<(), hares_types::HaresError> {
            unimplemented!()
        }
        fn resolved_zip(&self) -> Option<hares_types::zip::ResolvedZip> {
            self.resolved
        }
        fn expected_mean_power_w(&self) -> Option<ExpectedMeanPower> {
            self.mean
        }
    }

    fn governing_load(
        name: &str,
        zip: ZipLoad,
        mean: Option<ExpectedMeanPower>,
    ) -> Box<dyn Equipment> {
        Box::new(StubLoad::new(
            name,
            Some(hares_types::zip::ResolvedZip::governing(zip)),
            mean,
        ))
    }

    fn governing_stepped_load(
        name: &str,
        zip: ZipLoad,
        mean: Option<ExpectedMeanPower>,
        last_raw_draw_w: f64,
    ) -> Box<dyn Equipment> {
        Box::new(
            StubLoad::new(
                name,
                Some(hares_types::zip::ResolvedZip::governing(zip)),
                mean,
            )
            .with_last_draw(last_raw_draw_w),
        )
    }

    fn pinned_load(name: &str) -> Box<dyn Equipment> {
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
    fn stepped_dwelling_weights_are_the_moment_composition() {
        // F10's fix: a dwelling that has stepped aggregates the moment's
        // composition: each equipment's weight is its last raw draw (the
        // scaled core draw un-scaled through its own row at the bus
        // voltage), and `deep_sag_scale` is the assembled response the
        // stepping engine answers, with each row's deep-sag form applied.
        // At the parent the weights were the expected mean draws of loads
        // that were off at the moment (the dryer, the range), and the
        // reported aggregate understated the assembled deep-sag response
        // by up to 24.8 percent.
        let refrigerator = hares_types::zip::zip_defaults_for_class("Refrigerator").expect("row");
        let lighting = hares_types::zip::zip_defaults_for_class("Lighting").expect("row");
        // Two governing loads at the noon moment: the refrigerator drawing
        // raw 100 W and the lighting raw 200 W, both stepped at 0.7 pu.
        // The refrigerator's row is held at its stationary point below
        // 0.843 pu, so its 0.7 pu evaluation is the held value, not the
        // raw polynomial's 0.9787.
        let bus_voltage = 0.7;
        let m_fridge = refrigerator.deep_sag_real_multiplier(bus_voltage, false);
        let m_lighting = lighting.deep_sag_real_multiplier(bus_voltage, false);
        let equipment: Vec<Box<dyn Equipment>> = vec![
            governing_stepped_load(
                "Refrigerator",
                refrigerator,
                Some(ExpectedMeanPower::W(10.0)),
                100.0 * m_fridge,
            ),
            governing_stepped_load(
                "Indoor Lighting",
                lighting,
                Some(ExpectedMeanPower::W(500.0)),
                200.0 * m_lighting,
            ),
        ];
        let premise = aggregate_premise_zip(&equipment, &empty_schedule(), 1, bus_voltage)
            .expect("both loads carry positive moment weights");
        // The weights are the RAW draws (the expected means, 10 W and 500
        // W, must be ignored when the moment's draws exist).
        assert_eq!(
            premise.governing[0],
            ("Refrigerator".to_string(), Some(100.0))
        );
        assert_eq!(
            premise.governing[1],
            ("Indoor Lighting".to_string(), Some(200.0))
        );
        // The assembled response: the raw-draw-weighted mean of the rows'
        // deep-sag evaluations: the engine's answer for this composition,
        // NOT the raw evaluation of the aggregate coefficient row (the
        // refrigerator's hold is unreachable from a single quadratic).
        let expected_scale = (100.0 * m_fridge + 200.0 * m_lighting) / 300.0;
        let scale = premise.deep_sag_scale.expect("stepped weights");
        assert!(
            (scale - expected_scale).abs() < 1e-12,
            "deep_sag_scale must be the assembled response {expected_scale}, got {scale}"
        );
        // The raw evaluation of the published coefficient row differs (the
        // hold's effect): the field is the deep-sag consumer's answer.
        let raw_row_evaluation = premise.zip.zp * bus_voltage * bus_voltage
            + premise.zip.ip * bus_voltage
            + premise.zip.pp;
        assert!(raw_row_evaluation != scale);
    }

    #[test]
    fn de_energized_boundary_reports_the_stepped_state_not_expected_means() {
        // Z3's witness: at the admitted 0.0 pu outage value the aggregate
        // reports the de-energized state. A Lighting load that stepped at
        // the outage drew zero: its weight is that zero. At the parent the
        // stepped zero draw fell through the `bus_voltage_pu > 0.0` guard
        // to the expected-mean fallback, and the aggregate published a
        // 500 W weight and the Lighting row's 0.0 evaluation (-0.04) as
        // the scale for a load the outage had turned off, while the
        // engine drew zero on the same step.
        let lighting = hares_types::zip::zip_defaults_for_class("Lighting").expect("row");
        let equipment: Vec<Box<dyn Equipment>> = vec![governing_stepped_load(
            "Indoor Lighting",
            lighting,
            Some(ExpectedMeanPower::W(500.0)),
            0.0,
        )];
        let premise = aggregate_premise_zip(&equipment, &empty_schedule(), 1, 0.0);
        assert!(
            premise.is_none(),
            "the de-energized boundary must report the stepped state (a \
             zero weight, no positive weight, no scale): the parent \
             weighted the stepped-off load by its 500 W expected mean and \
             published the Lighting row's 0.0 evaluation, -0.04, as the \
             scale"
        );
    }

    #[test]
    fn static_route_never_weights_an_expectation_at_a_de_energized_bus() {
        // The boundary contract for the static route: an expected mean is
        // an energized-bus expectation. At 0.0 pu the engine draws zero
        // from every load, so weighting the expectation and evaluating the
        // row there would publish the row's 0.0 residue (Lighting -0.04)
        // as the scale of a draw the bus cannot supply. The aggregate is
        // None, the same answer the no-computable-weight contract gives.
        let lighting = hares_types::zip::zip_defaults_for_class("Lighting").expect("row");
        let equipment: Vec<Box<dyn Equipment>> = vec![governing_load(
            "Indoor Lighting",
            lighting,
            Some(ExpectedMeanPower::W(500.0)),
        )];
        let premise = aggregate_premise_zip(&equipment, &empty_schedule(), 1, 0.0);
        assert!(
            premise.is_none(),
            "the static route must not weight an expected mean at the \
             de-energized boundary: the parent published the Lighting \
             row's 0.0 evaluation, -0.04, as the scale of a 500 W \
             expectation on a dead bus"
        );
    }

    #[test]
    fn near_dead_static_route_weights_no_load_the_engine_draws_nothing_from() {
        // The near-dead edge of the same boundary: at 0.01 pu the Lighting
        // row evaluates 0.54 x 0.0001 + 0.5 x 0.01 - 0.04 = -0.034946. The
        // classes that publish an expectation (scheduled and event loads)
        // multiply their reference draw by that evaluation and publish the
        // draw clamped at zero (no electrical port), so the engine draws
        // nothing from the load at this bus. Weighting the expectation
        // would publish a negative "response" and a 500 W weight for a draw
        // the engine never takes; the honest answer is the same one the
        // de-energized boundary gives: no weight, no aggregate.
        let lighting = hares_types::zip::zip_defaults_for_class("Lighting").expect("row");
        assert!(
            lighting.deep_sag_real_multiplier(0.01, false) < 0.0,
            "fixture: the Lighting row must evaluate negative at 0.01 pu"
        );
        let equipment: Vec<Box<dyn Equipment>> = vec![governing_load(
            "Indoor Lighting",
            lighting,
            Some(ExpectedMeanPower::W(500.0)),
        )];
        assert!(
            aggregate_premise_zip(&equipment, &empty_schedule(), 1, 0.01).is_none(),
            "a non-positive row evaluation is a load the engine draws \
             nothing from: the expectation must not be weighted"
        );
    }

    #[test]
    fn near_dead_static_route_keeps_the_loads_the_engine_still_draws_from() {
        // The other half of the near-dead contract: the gate is the row's
        // response, not the voltage. A constant-power row evaluates 1.0 at
        // 0.01 pu, so the engine does draw the scheduled load and the
        // aggregate weights it. The Lighting expectation is rostered with no
        // weight (the aggregate did not use it): a positive weight the
        // aggregate ignored would break the caller-side audit that recomputes
        // the weighted mean from the rostered positive weights.
        let lighting = hares_types::zip::zip_defaults_for_class("Lighting").expect("row");
        let equipment: Vec<Box<dyn Equipment>> = vec![
            governing_load(
                "Indoor Lighting",
                lighting,
                Some(ExpectedMeanPower::W(500.0)),
            ),
            governing_load(
                "Always On",
                ZipLoad::constant_power(),
                Some(ExpectedMeanPower::W(3.0)),
            ),
        ];
        let premise = aggregate_premise_zip(&equipment, &empty_schedule(), 1, 0.01)
            .expect("the constant-power load still draws");
        assert_eq!(
            premise.governing,
            vec![
                ("Indoor Lighting".to_string(), None),
                ("Always On".to_string(), Some(3.0)),
            ],
            "the load whose row evaluates non-positively must carry no weight"
        );
        assert_eq!(
            (premise.zip.zp, premise.zip.ip, premise.zip.pp),
            (0.0, 0.0, 1.0),
            "only the constant-power load shapes the aggregate"
        );
        assert!(
            premise.deep_sag_scale.is_some_and(|s| s > 0.0),
            "a published scale is a positive multiplier, got {:?}",
            premise.deep_sag_scale
        );
    }

    #[test]
    fn single_row_un_scale_recovers_the_reference_draw_to_the_division_rounding() {
        // The single-row route's contract: the weight is the stepped draw
        // un-scaled through the row, `(raw x m) / m`. The double round-trip
        // is exact for many pairs (1234.5 W under the Lighting row at 0.5 pu
        // below) but NOT for every one: 100 W under the same row recovers
        // one ulp low (99.99999999999999), so the honest claim is "within
        // one rounding of the division", never "bit-for-bit". The response
        // is the row's evaluation either way.
        let lighting = hares_types::zip::zip_defaults_for_class("Lighting").expect("row");
        let bus_voltage = 0.5;
        let multiplier = lighting.deep_sag_real_multiplier(bus_voltage, false);
        let weight_for = |raw_draw_w: f64| -> f64 {
            let equipment: Vec<Box<dyn Equipment>> = vec![governing_stepped_load(
                "Indoor Lighting",
                lighting,
                None,
                raw_draw_w * multiplier,
            )];
            let premise = aggregate_premise_zip(&equipment, &empty_schedule(), 1, bus_voltage)
                .expect("the stepped draw carries a positive weight");
            let scale = premise.deep_sag_scale.expect("stepped weight");
            assert_eq!(
                scale.to_bits(),
                multiplier.to_bits(),
                "the single-row response is the resolved row's evaluation"
            );
            premise.governing[0]
                .1
                .expect("the roster publishes the un-scaled weight")
        };

        // A pair whose round-trip is bitwise exact.
        let exact = weight_for(1234.5);
        assert_eq!(
            exact.to_bits(),
            1234.5_f64.to_bits(),
            "the un-scale of 1234.5 W must be bitwise the reference draw"
        );

        // A pair whose round-trip lands one ulp low: the documented bound is
        // one ulp, not bit-for-bit equality.
        let near = weight_for(100.0);
        assert_eq!(
            near.to_bits(),
            100.0_f64.next_down().to_bits(),
            "hand: 100 x 0.34500000000000003 / 0.34500000000000003 = \
             99.99999999999999 (one ulp low), got {near}"
        );
        assert!(
            (near - 100.0).abs() <= f64::EPSILON * 100.0,
            "the un-scale must recover the reference draw to one rounding, \
             got {near}"
        );

        // The bound over the family's parameter range, not just the two
        // witness points: every evaluation route the multiplier has and
        // every real coefficient pattern a row can carry, at every voltage
        // of the admitted [0.5, 1.5] pu band with both edges included,
        // over the realistic draw magnitudes (sub-watt standby electronics
        // up to a 100 kW whole-premise peak). The routes: the Lighting row
        // (mixed signs, monotone form) and the pure impedance, pure
        // current and pure constant-power overrides and a mixed row
        // evaluating below the class rows' floor evaluate raw; a row
        // override with an in-band stationary point (zp 0.5, ip -0.75,
        // pp 1.25, summing to one, v* = 0.75) resolves through the hold
        // arm below nominal (0.5 x 0.75² - 0.75 x 0.75 + 1.25 = 0.96875 at
        // 0.5 and 0.75 pu); the air conditioner's row as fitted evaluates
        // the raw Hajagos & Danai polynomial. The raw draw is the
        // row-scaled reference (one rounding) and the un-scale divides the
        // same multiplier out (one rounding), so the recovered weight must
        // land on the reference draw or an immediate neighbour, whatever
        // the mantissa the magnitude puts the pair in. (A product or
        // quotient that leaves the normal range loses the bound; the
        // band's multipliers and these draws keep every product and
        // quotient normal.)
        let ac_row = hares_types::zip::zip_defaults_for_class("Air Conditioner").expect("row");
        let fitted_ac = hares_types::zip::ResolvedZip {
            real_shape_as_fitted: true,
            ..hares_types::zip::ResolvedZip::governing(ac_row)
        };
        // A row override may carry any real coefficient pattern summing to
        // one: the pure forms and a mixed row evaluating to 0.3 at 0.5 pu,
        // between the passive impedance floor and the fitted motor rows'
        // elevations.
        let pure = |zp: f64, ip: f64, pp: f64| {
            hares_types::zip::ResolvedZip::governing(ZipLoad {
                zp,
                ip,
                pp,
                zq: 0.0,
                iq: 0.0,
                pq: 1.0,
                pf: 0.0,
                v0: 1.0,
            })
        };
        // Every row shape at every band voltage: two literal arrays, no
        // sampling, the completeness structural (a nested loop cannot skip
        // a shape-voltage pair).
        let hold = pure(0.5, -0.75, 1.25);
        // The route each shape takes is part of the sweep's claim, so pin
        // it: the hold row resolves through the hold arm at 0.5 pu (the raw
        // polynomial evaluates exactly 1.0 there, the held one 0.96875), and
        // the AC row through the raw as-fitted polynomial (the monotone hold
        // would evaluate 0.959359375 at its own stationary point). A shape
        // whose arm silently changed would leave that route unexercised
        // while the sweep below still passed.
        assert_eq!(
            hold.deep_sag_real_multiplier(0.5).to_bits(),
            0.96875_f64.to_bits(),
            "fixture: the hold shape must evaluate the hold arm at 0.5 pu"
        );
        assert!(
            (fitted_ac.deep_sag_real_multiplier(0.5) - 1.145).abs() < 1e-12,
            "fixture: the AC shape must evaluate the raw as-fitted polynomial \
             (1.145 at 0.5 pu), got {}",
            fitted_ac.deep_sag_real_multiplier(0.5)
        );
        let shapes = [
            hares_types::zip::ResolvedZip::governing(lighting), // raw, mixed signs
            fitted_ac,                                          // the as-fitted polynomial
            pure(1.0, 0.0, 0.0), // pure impedance: 0.25 at 0.5 pu, 2.25 at 1.5 pu
            pure(0.0, 1.0, 0.0), // pure current: 0.5 at 0.5 pu, 1.5 at 1.5 pu
            pure(0.0, 0.0, 1.0), // pure constant power: 1.0 at every voltage
            pure(0.8, 0.2, 0.0), // mixed low: 0.3 at 0.5 pu, 2.1 at 1.5 pu
            hold,                // the hold arm: 0.96875 at 0.5 pu, 1.25 at 1.5 pu
        ];
        for voltage_pu in [0.5, 0.75, 1.0, 1.1, 1.5] {
            for resolved in shapes {
                let multiplier = resolved.deep_sag_real_multiplier(voltage_pu);
                assert!(
                    multiplier.is_finite() && multiplier > 0.0,
                    "fixture: the row must evaluate positive at {voltage_pu} pu"
                );
                for reference_w in [0.5, 1.0, 7.3, 100.0, 1234.5, 9999.0, 48213.7, 100000.0] {
                    let equipment: Vec<Box<dyn Equipment>> = vec![Box::new(
                        StubLoad::new("Sweep", Some(resolved), None)
                            .with_last_draw(reference_w * multiplier),
                    )];
                    let premise =
                        aggregate_premise_zip(&equipment, &empty_schedule(), 1, voltage_pu)
                            .expect("the stepped draw carries a positive weight");
                    let weight = premise
                        .governing
                        .first()
                        .and_then(|(_, w)| *w)
                        .expect("the roster publishes the un-scaled weight");
                    let ulps = weight.to_bits() as i64 - reference_w.to_bits() as i64;
                    assert!(
                        ulps.abs() <= 1,
                        "the un-scale of {reference_w} W at {voltage_pu} pu must \
                         land within one ulp of the reference draw, got {weight} \
                         ({ulps} ulp)"
                    );
                }
            }
        }
    }

    #[test]
    fn a_stepped_zero_draw_rosters_its_zero_beside_a_live_load() {
        // The stepped-zero contract, observable: beside a load that still
        // draws, the roster reports the off load's zero (the moment was
        // inspected and the load was off), never its 500 W expectation. The
        // expectation is the pre-step route's evidence and must not stand in
        // for a moment that was observed.
        let lighting = hares_types::zip::zip_defaults_for_class("Lighting").expect("row");
        let equipment: Vec<Box<dyn Equipment>> = vec![
            governing_stepped_load(
                "Indoor Lighting",
                lighting,
                Some(ExpectedMeanPower::W(500.0)),
                0.0,
            ),
            governing_stepped_load(
                "Always On",
                ZipLoad::constant_power(),
                Some(ExpectedMeanPower::W(300.0)),
                200.0,
            ),
        ];
        let premise = aggregate_premise_zip(&equipment, &empty_schedule(), 1, 1.0)
            .expect("the live load carries a positive weight");
        assert_eq!(
            premise.governing,
            vec![
                ("Indoor Lighting".to_string(), Some(0.0)),
                // The constant-power row evaluates 1.0 at the reference
                // voltage, so the un-scale recovers the 200 W draw exactly.
                ("Always On".to_string(), Some(200.0)),
            ],
            "an observed zero draw is a zero weight, not the expectation"
        );
        // A zero real draw carries no reactive coefficient into the aggregate
        // either: the Lighting row's zq/iq (0.46/0.51) would poison the
        // reactive blend of a load that drew nothing at the moment, and its
        // model derives Q from the real draw, so it has no var to contribute.
        assert_eq!(
            (premise.zip.zq, premise.zip.iq, premise.zip.pq),
            (0.0, 0.0, 1.0),
            "only the live load's reactive coefficients shape the aggregate"
        );
        assert_eq!(
            premise.deep_sag_scale,
            Some(1.0),
            "the live load's reference-voltage response is the identity"
        );
    }

    #[test]
    fn a_broken_stepped_draw_rosters_no_weight_not_a_fabricated_zero() {
        // A stepped report that is not a draw (a negative watt value,
        // violating `ElectricPower::Consumption`'s "must be >= 0" invariant,
        // or a NaN from a broken core state) is not evidence that the load
        // drew zero. The roster must publish no weight for it (the
        // established unknown-mean semantics), and the expected mean must
        // not stand in for the moment: a fabricated zero would tell the
        // caller the moment was inspected and came up empty, and the 500 W
        // expectation would claim a draw neither the broken report nor the
        // schedule observed.
        let lighting = hares_types::zip::zip_defaults_for_class("Lighting").expect("row");
        for broken in [-3.0, f64::NAN] {
            let equipment: Vec<Box<dyn Equipment>> = vec![
                governing_stepped_load(
                    "Broken Report",
                    lighting,
                    Some(ExpectedMeanPower::W(500.0)),
                    broken,
                ),
                governing_stepped_load(
                    "Always On",
                    ZipLoad::constant_power(),
                    Some(ExpectedMeanPower::W(300.0)),
                    200.0,
                ),
            ];
            let premise = aggregate_premise_zip(&equipment, &empty_schedule(), 1, 1.0)
                .expect("the live load carries a positive weight");
            assert_eq!(
                premise.governing[0],
                ("Broken Report".to_string(), None),
                "a broken (negative or NaN) stepped report is not an observed \
                 zero: the roster must publish no weight, the expectation \
                 must not stand in for the moment"
            );
            assert_eq!(
                premise.governing[1],
                ("Always On".to_string(), Some(200.0)),
                "the live load's weight is untouched by the broken report"
            );
            assert_eq!(
                (premise.zip.zp, premise.zip.ip, premise.zip.pp),
                (0.0, 0.0, 1.0),
                "only the live load shapes the aggregate"
            );
        }
    }

    #[test]
    fn a_stepped_draw_whose_response_is_not_a_ratio_carries_no_weight() {
        // The assembled base's own acceptance rule ("finite and positive")
        // bounds the base, not the ratio. A stepped report that dwarfs its
        // base divides to infinity, and one the base dwarfs divides to
        // zero; either way the equipment observed its draw this step, so
        // the pre-step expectation may not stand in for it. The roster
        // must publish no weight for the broken ratio (the unknown-mean
        // semantics), never the 500 W expected mean, and the aggregate
        // must weight only the load it can see.
        let lighting = hares_types::zip::zip_defaults_for_class("Lighting").expect("row");
        for (draw_w, nominal_w) in [(f64::INFINITY, 1200.0), (1e300, 1e-300), (1e-300, 1e300)] {
            let equipment: Vec<Box<dyn Equipment>> = vec![
                Box::new(
                    StubLoad::new(
                        "Broken Ratio",
                        Some(hares_types::zip::ResolvedZip::governing(lighting)),
                        Some(ExpectedMeanPower::W(500.0)),
                    )
                    .with_last_draw(draw_w)
                    .with_nominal_draw(nominal_w),
                ),
                governing_stepped_load(
                    "Always On",
                    ZipLoad::constant_power(),
                    Some(ExpectedMeanPower::W(300.0)),
                    200.0,
                ),
            ];
            let premise = aggregate_premise_zip(&equipment, &empty_schedule(), 1, 1.0)
                .expect("the live load carries a positive weight");
            assert_eq!(
                premise.governing[0],
                ("Broken Ratio".to_string(), None),
                "a stepped report of {draw_w} W over a {nominal_w} W base is \
                 not a usable response ratio: the 500 W expectation must not \
                 stand in for the moment the load observed"
            );
            assert_eq!(
                premise.governing[1],
                ("Always On".to_string(), Some(200.0)),
                "the live load's weight is untouched by the broken ratio"
            );
            assert_eq!(
                (premise.zip.zp, premise.zip.ip, premise.zip.pp),
                (0.0, 0.0, 1.0),
                "only the live load shapes the aggregate"
            );
        }
    }

    #[test]
    fn the_de_energized_arm_zeroes_every_stepped_report() {
        // The de-energized arm is the broken-draw class's other sibling: at
        // the admitted 0.0 pu outage the bus draws zero from every load, so
        // the stepped state is a zero weight whatever the last core report
        // carries. The pin sweeps the arm's reports: an observed zero, a
        // stale positive draw, and the broken values F3's witness pins one
        // voltage up (a negative watt value, NaN, an infinite draw). None
        // of them, and none of the loads' expectations, may enter the
        // aggregate: a weight here would report a draw a dead bus cannot
        // supply.
        let lighting = hares_types::zip::zip_defaults_for_class("Lighting").expect("row");
        for reported_w in [0.0, 300.0, -3.0, f64::NAN, f64::INFINITY] {
            let equipment: Vec<Box<dyn Equipment>> = vec![
                governing_stepped_load(
                    "Indoor Lighting",
                    lighting,
                    Some(ExpectedMeanPower::W(500.0)),
                    reported_w,
                ),
                governing_stepped_load(
                    "Always On",
                    ZipLoad::constant_power(),
                    Some(ExpectedMeanPower::W(300.0)),
                    200.0,
                ),
            ];
            assert!(
                aggregate_premise_zip(&equipment, &empty_schedule(), 1, 0.0).is_none(),
                "at 0.0 pu the engine draws zero from every load: a stepped \
                 report of {reported_w} W is a zero weight, never the report \
                 and never an expected mean"
            );
        }
    }

    #[test]
    fn a_weighted_sum_that_overflows_is_no_aggregate_not_a_non_finite_one() {
        // The published aggregate must be finite by construction: every
        // weight is a finite positive draw, every response a finite
        // positive multiplier, and every coefficient legal (the loader
        // admits magnitudes up to 100). The weighted PRODUCTS and the sums
        // can still overflow: two finite draws of 1e308 W overflow the
        // weight sum, and a legal zp = 100 row times a 1e307 W draw
        // overflows a single product. The division then publishes NaN or
        // infinity inside a structurally normal struct (the fields claim a
        // weighted mean and a positive finite scale). A non-finite mean is
        // not a mean: the honest answer is `None`, the same
        // no-computable-weight answer the NaN-total guard gives.
        let legal_large_row = ZipLoad {
            zp: 100.0,
            ip: -99.0,
            pp: 0.0,
            zq: 0.0,
            iq: 0.0,
            pq: 1.0,
            pf: 0.0,
            v0: 1.0,
        };
        let fixtures: [(&str, Vec<Box<dyn Equipment>>); 2] = [
            (
                "two finite draws that overflow the weight sum",
                vec![
                    governing_stepped_load("Huge A", ZipLoad::constant_power(), None, 1e308),
                    governing_stepped_load("Huge B", ZipLoad::constant_power(), None, 1e308),
                ],
            ),
            (
                "a legal large coefficient that overflows one product",
                vec![governing_stepped_load(
                    "Huge Product",
                    legal_large_row,
                    None,
                    1e307,
                )],
            ),
        ];
        for (label, equipment) in fixtures {
            match aggregate_premise_zip(&equipment, &empty_schedule(), 1, 1.0) {
                None => {}
                Some(premise) => panic!(
                    "{label}: a non-finite aggregate was published \
                     (zp {:?}, ip {:?}, pp {:?}, scale {:?})",
                    premise.zip.zp, premise.zip.ip, premise.zip.pp, premise.deep_sag_scale
                ),
            }
        }
    }

    #[test]
    fn a_bidirectional_or_generating_moment_is_its_report_not_the_expectation() {
        // `CoreFlows::electric_w` is an `ElectricPower`: a governing load may
        // report a bidirectional flow (positive = consuming) or a generation
        // flow (it exported and drew nothing). Both are the moment's report,
        // not an absent one. Reading only `Consumption` handed the moment to
        // the static route, and the roster published the 500 W expectation
        // for a load that reported a 200 W draw, or for one that reported an
        // export and drew nothing.
        let lighting = hares_types::zip::zip_defaults_for_class("Lighting").expect("row");
        let run = |power| {
            let equipment: Vec<Box<dyn Equipment>> = vec![
                Box::new(
                    StubLoad::new(
                        "Reported Moment",
                        Some(hares_types::zip::ResolvedZip::governing(lighting)),
                        Some(ExpectedMeanPower::W(500.0)),
                    )
                    .with_last_flow(power),
                ),
                governing_stepped_load(
                    "Always On",
                    ZipLoad::constant_power(),
                    Some(ExpectedMeanPower::W(300.0)),
                    200.0,
                ),
            ];
            aggregate_premise_zip(&equipment, &empty_schedule(), 1, 1.0)
                .expect("the live load carries a positive weight")
        };

        // A bidirectional draw consumes its positive value: the weight is
        // the report un-scaled by the row (1.0 at the reference voltage).
        let consuming = run(hares_types::ElectricPower::Bidirectional(200.0));
        assert_eq!(
            consuming.governing[0],
            ("Reported Moment".to_string(), Some(200.0)),
            "a bidirectional draw is a 200 W consumption, not the 500 W \
             expectation"
        );

        // A generation report consumes nothing: the moment's weight is the
        // observed zero, not the expectation.
        let generating = run(hares_types::ElectricPower::Generation(200.0));
        assert_eq!(
            generating.governing[0],
            ("Reported Moment".to_string(), Some(0.0)),
            "a generating moment drew nothing: its weight is the observed \
             zero, not the 500 W expectation"
        );
    }

    #[test]
    fn a_malformed_generation_report_is_no_weight_not_an_observed_zero() {
        // A generation report's value is an export magnitude, and a valid
        // one (finite, non-negative) means the load consumed exactly
        // nothing: its weight is the observed zero. A value that violates
        // the type's contract is broken core state, not a smaller export:
        // the type's own `net_consumption_w` reads `Generation(-5.0)` as
        // +5 W of consumption, so the report is no evidence that the load
        // drew nothing, and folding it into a zero weight would tell the
        // caller the moment was inspected and came up empty. The roster
        // must publish no weight for it (the broken-draw semantics), and
        // the expectation must not stand in for the moment either.
        let lighting = hares_types::zip::zip_defaults_for_class("Lighting").expect("row");
        let run = |power| {
            let equipment: Vec<Box<dyn Equipment>> = vec![
                Box::new(
                    StubLoad::new(
                        "Broken Export",
                        Some(hares_types::zip::ResolvedZip::governing(lighting)),
                        Some(ExpectedMeanPower::W(500.0)),
                    )
                    .with_last_flow(power),
                ),
                governing_stepped_load(
                    "Always On",
                    ZipLoad::constant_power(),
                    Some(ExpectedMeanPower::W(300.0)),
                    200.0,
                ),
            ];
            aggregate_premise_zip(&equipment, &empty_schedule(), 1, 1.0)
                .expect("the live load carries a positive weight")
        };
        for broken in [-5.0, f64::NAN, f64::INFINITY] {
            let premise = run(hares_types::ElectricPower::Generation(broken));
            assert_eq!(
                premise.governing[0],
                ("Broken Export".to_string(), None),
                "a generation report of {broken} violates the type's \
                 non-negative export contract: it is broken core state, not \
                 an observed zero, so the roster must publish no weight and \
                 the 500 W expectation must not stand in for it"
            );
            assert_eq!(
                premise.governing[1],
                ("Always On".to_string(), Some(200.0)),
                "the live load's weight is untouched by the broken report"
            );
        }

        // The valid export is the observed zero: the moment was inspected
        // and the load drew nothing (its net flow is an injection).
        let valid = run(hares_types::ElectricPower::Generation(200.0));
        assert_eq!(
            valid.governing[0],
            ("Broken Export".to_string(), Some(0.0)),
            "a valid generation report consumed nothing: its weight is the \
             observed zero, never the 500 W expectation"
        );
    }

    #[test]
    fn live_bus_with_every_load_stepped_off_reports_zero_weights_and_no_aggregate() {
        // The third boundary class: an energized bus where every governing
        // load stepped and drew nothing (a control decision, not an outage).
        // Each load carries a real zero weight (the roster tells the caller
        // the moment was inspected and was empty), no load carries reactive
        // power either (Q is derived from the real draw, and a zero-draw
        // component contributes no var), and the aggregate is None rather
        // than a fabricated constant-power answer.
        let lighting = hares_types::zip::zip_defaults_for_class("Lighting").expect("row");
        let equipment: Vec<Box<dyn Equipment>> = vec![
            governing_stepped_load(
                "Indoor Lighting",
                lighting,
                Some(ExpectedMeanPower::W(500.0)),
                0.0,
            ),
            governing_stepped_load(
                "Always On",
                ZipLoad::constant_power(),
                Some(ExpectedMeanPower::W(300.0)),
                0.0,
            ),
        ];
        assert!(
            aggregate_premise_zip(&equipment, &empty_schedule(), 1, 1.0).is_none(),
            "every stepped load drew zero: there is no positive weight to report"
        );
    }

    #[test]
    fn a_non_finite_component_base_never_publishes_a_non_finite_scale() {
        // `nominal_electric_draw_w` is a public trait hook: a report of zero,
        // NaN or infinity would make the aggregate's division publish
        // inf/NaN in `deep_sag_scale` (and a non-finite weight in the
        // roster) inside a structurally normal aggregate. A base is usable
        // only when it is finite and positive; otherwise the aggregate falls
        // back to the resolved row's un-scale, the documented single-row
        // route.
        let ac_row = hares_types::zip::zip_defaults_for_class("Air Conditioner").expect("row");
        for broken in [0.0, f64::NAN, f64::INFINITY] {
            let equipment: Vec<Box<dyn Equipment>> = vec![Box::new(
                StubLoad::new(
                    "Air Conditioner",
                    Some(hares_types::zip::ResolvedZip {
                        real_shape_as_fitted: true,
                        ..hares_types::zip::ResolvedZip::governing(ac_row)
                    }),
                    None,
                )
                .with_last_draw(1216.0)
                .with_nominal_draw(broken),
            )];
            let premise = aggregate_premise_zip(&equipment, &empty_schedule(), 1, 0.5)
                .expect("the positive draw still carries a weight");
            let multiplier = ac_row.deep_sag_real_multiplier(0.5, true);
            assert_eq!(
                premise.governing[0],
                ("Air Conditioner".to_string(), Some(1216.0 / multiplier)),
                "a base of {broken} is not usable: the row's evaluation is the \
                 only single-row route"
            );
            let scale = premise.deep_sag_scale.expect("stepped weight");
            assert!(
                scale.is_finite() && (scale - multiplier).abs() < 1e-12,
                "the published scale must be the row's finite response \
                 {multiplier}, got {scale}"
            );
        }
    }

    #[test]
    fn a_non_finite_moment_draw_never_publishes_a_non_finite_scale() {
        // A broken core draw (inf) with a usable base would divide to inf.
        // The aggregate must not publish it: the aggregate is None, the
        // no-computable-positive-weight answer.
        let ac_row = hares_types::zip::zip_defaults_for_class("Air Conditioner").expect("row");
        let equipment: Vec<Box<dyn Equipment>> = vec![Box::new(
            StubLoad::new(
                "Air Conditioner",
                Some(hares_types::zip::ResolvedZip {
                    real_shape_as_fitted: true,
                    ..hares_types::zip::ResolvedZip::governing(ac_row)
                }),
                None,
            )
            .with_last_draw(f64::INFINITY)
            .with_nominal_draw(1200.0),
        )];
        assert!(
            aggregate_premise_zip(&equipment, &empty_schedule(), 1, 0.5).is_none(),
            "a non-finite draw is not a positive weight"
        );
    }

    #[test]
    fn multi_component_moment_weights_are_the_assembled_nominal_draw() {
        // Z2's witness (the attack's air-conditioner number): a unit whose
        // draw is the sum of components with different rows must be
        // weighted by the components' nominal draw and report the assembled
        // response, not the primary row's polynomial. The probe's
        // composition: 1000 W of compressor and 200 W of blower at 0.5 pu
        // draw 1000 x 1.145 + 200 x 0.355 = 1216 W against 1200 W nominal:
        // an assembled response of 1216/1200. The primary row's own
        // multiplier at 0.5 pu is 1.145 (the compressor row, as fitted:
        // 1.6 x 0.25 - 2.69 x 0.5 + 2.09), a 13 percent overstatement of
        // the response, and the parent weighted the composition 1062 W
        // against the true 1200 W.
        let ac_row = hares_types::zip::zip_defaults_for_class("Air Conditioner").expect("row");
        let equipment: Vec<Box<dyn Equipment>> = vec![Box::new(
            StubLoad::new(
                "Air Conditioner",
                Some(hares_types::zip::ResolvedZip {
                    real_shape_as_fitted: true,
                    ..hares_types::zip::ResolvedZip::governing(ac_row)
                }),
                None,
            )
            .with_last_draw(1216.0)
            .with_nominal_draw(1200.0),
        )];
        let premise = aggregate_premise_zip(&equipment, &empty_schedule(), 1, 0.5)
            .expect("the stepped composition carries a positive weight");
        assert_eq!(
            premise.governing[0],
            ("Air Conditioner".to_string(), Some(1200.0)),
            "the weight must be the components' nominal draw, not the \
             stepped draw un-scaled through the primary row's multiplier \
             (1062.004...)"
        );
        let scale = premise.deep_sag_scale.expect("stepped weight");
        let expected_scale = 1216.0 / 1200.0;
        assert!(
            (scale - expected_scale).abs() < 1e-12,
            "deep_sag_scale must be the assembled response {expected_scale} \
             (the components' actual evaluated draws over the components' \
             nominal draws), got {scale} (the primary row's 1.145)"
        );
        assert!(
            (scale - ac_row.deep_sag_real_multiplier(0.5, true)).abs() > 1e-3,
            "the assembled response must not be the primary row's polynomial"
        );
    }

    #[test]
    fn backup_only_heat_pump_water_heater_reports_the_impedance_response() {
        // Z2's witness (the attack's heat-pump water heater number): a
        // backup-only unit's draw is the backup element's impedance share,
        // P = V²/R: 1000 W nominal at 0.5 pu draws 250 W. The HPWH row's
        // own multiplier at 0.5 pu is 0.825 x 0.25 - 0.44 x 0.5 + 0.615 =
        // 0.60125; the parent published it as the scale, a 2.4 times
        // overstatement of the moment's response, weighting the
        // composition 415.8 W against the true 1000 W.
        let hpwh_row =
            hares_types::zip::zip_defaults_for_class("Heat Pump Water Heater").expect("row");
        let equipment: Vec<Box<dyn Equipment>> = vec![Box::new(
            StubLoad::new(
                "Heat Pump Water Heater",
                Some(hares_types::zip::ResolvedZip::governing(hpwh_row)),
                None,
            )
            .with_last_draw(250.0)
            .with_nominal_draw(1000.0),
        )];
        let premise = aggregate_premise_zip(&equipment, &empty_schedule(), 1, 0.5)
            .expect("the stepped composition carries a positive weight");
        assert_eq!(
            premise.governing[0],
            ("Heat Pump Water Heater".to_string(), Some(1000.0)),
            "the weight must be the components' nominal draw, not the \
             stepped draw un-scaled through the unit row's 0.60125 \
             (415.801...)"
        );
        let scale = premise.deep_sag_scale.expect("stepped weight");
        assert!(
            (scale - 0.25).abs() < 1e-12,
            "deep_sag_scale must be the backup element's impedance response \
             0.25 (P = V²/R at 0.5 pu), got {scale}"
        );
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
                Some(ExpectedMeanPower::W(3.0)),
            ),
            pinned_load("Gas Furnace"),
        ];
        // Column mean = (1 + 3)/2 = 2 kW vs 3 kW for the constant load.
        let premise = aggregate_premise_zip(&equipment, &empty_schedule(), 1, 1.0)
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
                Some(ExpectedMeanPower::W(2.0)),
            ),
        ];
        let premise =
            aggregate_premise_zip(&equipment, &empty_schedule(), 1, 1.0).expect("aggregates");
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
        assert!(aggregate_premise_zip(&equipment, &empty_schedule(), 1, 1.0).is_none());
    }

    #[test]
    fn zero_expected_draw_is_none_not_constant_power() {
        let lighting = hares_types::zip::zip_defaults_for_class("Lighting").expect("row");
        let equipment: Vec<Box<dyn Equipment>> = vec![governing_load(
            "Seasonal Pump",
            lighting,
            Some(ExpectedMeanPower::W(0.0)),
        )];
        assert!(aggregate_premise_zip(&equipment, &empty_schedule(), 1, 1.0).is_none());
    }

    #[test]
    fn equipment_without_zip_concept_is_silent() {
        let equipment: Vec<Box<dyn Equipment>> =
            vec![Box::new(StubLoad::new("Generator", None, None))];
        assert!(aggregate_premise_zip(&equipment, &empty_schedule(), 1, 1.0).is_none());
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
        assert_eq!(schedule_column_mean_w(&schedule, 0, 1), Some(2.0));
        assert_eq!(schedule_column_mean_w(&schedule, 1, 1), None);
    }

    #[test]
    fn nan_schedule_data_must_not_degrade_to_a_silently_skewed_mean() {
        // A schedule column containing NaN (reachable: the CSV loader parses
        // with `str::parse::<f64>()`, which accepts "nan") must not be
        // averaged as if the NaN timesteps were zero draw — that publishes a
        // plausible but wrong premise weight with no signal to the caller.
        // The honest degradation is `None` (expected draw not computable):
        // the equipment is then rostered with `mean_power_w: None`, which
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
            schedule_column_mean_w(&schedule, 0, 1),
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
            assert_eq!(schedule_column_mean_w(&schedule, 0, 1), None);
        }
    }

    #[test]
    fn nan_column_rosters_equipment_without_weight_not_as_zero_draw() {
        // Observable at the aggregate level: an equipment whose schedule
        // column contains NaN stays on the governing roster with an
        // explicitly absent weight (`mean_power_w: None`), so the caller
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
                Some(ExpectedMeanPower::W(2.0)),
            ),
        ];
        let premise = aggregate_premise_zip(&equipment, &nan_schedule, 1, 1.0).expect("aggregates");
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
                Some(ExpectedMeanPower::W(2.0)),
            ),
            governing_load(
                "Always On",
                ZipLoad::constant_power(),
                Some(ExpectedMeanPower::W(1.0)),
            ),
        ];
        match aggregate_premise_zip(&equipment, &empty_schedule(), 1, 1.0) {
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
        // from the rostered positive `mean_power_w` entries and each
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
                Some(ExpectedMeanPower::W(2.0)),
            ),
            governing_load("Honest Load", honest, Some(ExpectedMeanPower::W(1.0))),
        ];
        let premise = aggregate_premise_zip(&equipment, &empty_schedule(), 1, 1.0)
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
        assert_eq!(schedule_column_mean_w(&schedule, 0, 1), None);
    }
}
