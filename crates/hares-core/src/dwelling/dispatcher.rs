//! The control dispatcher: the tier queues, the cross-pass priority
//! ledger, the routing that applies queued signals to equipment, and the
//! end-use addressing vocabulary the target-conflict checks share with
//! the roster's own target accounting.

use std::collections::VecDeque;
use std::sync::Arc;

use hares_control::{DispatchRequest, DispatchTarget, PRIORITY_TIER_COUNT};
#[cfg(feature = "observe")]
use hares_control::PriorityTier;
use hares_equipment::Equipment;
use hares_types::{ControlSignal, EndUse, ThermostatAxis};

use super::warnings::WarningLog;
use crate::health::RejectionGate;
#[cfg(feature = "observe")]
use crate::observer::{DispatchedSignal, SameTierConflict};

/// One step's queued-request bound: the construction-time capacity the
/// actor dispatch buffer and each priority tier's queue pre-size for.
/// An event's requests (the EV driver's arrival, the BMS's
/// re-evaluation) queue inside the bound, so a steady-state event pays
/// no first-growth fill; `clear` and `drain` keep the capacity, so the
/// fills are once per buffer, per process.
pub(super) const ACTOR_DISPATCH_BOUND: usize = 16;

/// Internal control queue and routing logic.
///
/// # Tier ordering (across priority levels)
///
/// Signals are bucketed into tier queues on `queue()` and drained low→high in
/// `dispatch_into()`. Because every signal fires (no deduplication), the
/// highest-priority tier writes last and wins. Equipment `apply_control` must
/// be overwrite-safe (idempotent set, not accumulate).
///
/// # Same-tier conflict resolution
///
/// Within a single priority tier, signals are dispatched in FIFO order (the
/// order they were queued). When two or more signals at the same tier target
/// the same equipment, the last signal queued within that tier wins because
/// `apply_control` is overwrite-safe — each subsequent `apply_control` call
/// overwrites the state written by the prior call for the same target. There
/// is no deduplication, merging, or composition within a tier.
///
/// **Strategy:** last-write-wins (FIFO queuing order within the tier).
///
/// **Rationale:** Simple, deterministic for a given actor registration order,
/// and requires no per-signal-type composition rules. Composition strategies
/// — most-restrictive (min consumption for curtailment, widest deadband for
/// thermostats), least-restrictive, weighted average — would require
/// signal-type-specific merge logic that must be designed and maintained for
/// every new signal variant. Last-write-wins avoids this complexity while
/// remaining predictable: the outcome is fully determined by the order in
/// which actors emit signals into the same tier.
///
/// **Implications:**
/// - Simulation results depend on actor registration order. Reordering actor
///   registration — e.g. swapping the order in which two DR programs are
///   added — changes which signal wins when both target the same equipment
///   at the same tier.
/// - Reproducibility requires recording actor registration order alongside
///   simulation outputs. The debug/invariant-check log emitted at dwelling
///   initialization captures this order explicitly.
/// - Upstream callers that programmatically register actors must be aware
///   that the last-added actor at a given tier dominates for shared targets.
///
/// # Cross-dispatch ledger
///
/// A single timestep may dispatch multiple times (e.g. once pre-thermal-FSM to
/// flush externally queued setpoints, once post-actor-decide to apply actor
/// signals). `begin_step()` resets the cross-dispatch conflict ledger so that
/// priority ordering holds across passes: a lower-priority signal arriving
/// in a later pass is SKIPPED when a higher-priority signal has already been
/// applied to the same target in an earlier pass.
pub(super) struct ControlDispatcher {
    by_tier: [VecDeque<DispatchRequest>; PRIORITY_TIER_COUNT],
    /// Scratch buffer for conflict detection -- tracks (target, tier_index).
    /// Pre-allocated, cleared at the start of each step via `begin_step()`.
    /// Preserved across multiple dispatch passes within a single step so that
    /// priority inversion cannot occur: once a tier has been seen for a target,
    /// strictly lower tiers are rejected even if they are queued later.
    seen_targets: Vec<(DispatchTarget, usize)>,
    /// The observing conflict scan's own per-pass throwaway: the passes
    /// whose conflicts the merged capture does not keep (the
    /// post-decide pass's and the derived passes', by the merge's own
    /// contract) record into this scratch. Cleared per pass, so the
    /// steady state reuses its capacity.
    #[cfg(feature = "observe")]
    conflict_scratch: Vec<SameTierConflict>,
    /// The observing conflict scan's rented inner signals buffers: a
    /// record rents its `signals` Vec from here and the cleared
    /// records' buffers return to the pool, so the steady state
    /// allocates none.
    #[cfg(feature = "observe")]
    conflict_signal_pool: Vec<Vec<ControlSignal>>,
    /// The observing conflict scan's recorded-from ledger: the indexes
    /// whose conflict groups are already recorded, cleared per tier
    /// scan, so the steady state reuses its capacity.
    #[cfg(feature = "observe")]
    recorded_from_scratch: Vec<usize>,
}

/// Returns true if two dispatch targets route to the same physical equipment,
/// accounting for `ByEndUse` expansion against the equipment list.
///
/// Unlike `DispatchTarget::conflicts_with()`, this resolves `ByEndUse` targets
/// to the equipment names they match, so cross-variant pairs (e.g.
/// `ByEndUse(BATTERY)` vs `ByName("Battery #1")`) are correctly detected as
/// conflicting when they target the same physical equipment.
///
/// Used by the same-tier conflict pre-scans in `dispatch_into_observed`,
/// which operate on raw
/// pre-expansion queue entries where cross-variant calls are expected.
#[cfg(feature = "observe")]
fn targets_conflict(
    a: &DispatchRequest,
    b: &DispatchRequest,
    equipment: &[Box<dyn Equipment>],
) -> bool {
    match (&a.target, &b.target) {
        (DispatchTarget::ByName(an), DispatchTarget::ByName(bn)) => an == bn,
        (DispatchTarget::ByEndUse(ae), DispatchTarget::ByEndUse(be)) => ae == be,
        (DispatchTarget::ByName(name), DispatchTarget::ByEndUse(end_use)) => {
            named_unit_addressed_by_end_use(equipment, name, end_use, &b.signal)
        }
        (DispatchTarget::ByEndUse(end_use), DispatchTarget::ByName(name)) => {
            named_unit_addressed_by_end_use(equipment, name, end_use, &a.signal)
        }
    }
}

#[cfg(feature = "observe")]
fn named_unit_addressed_by_end_use(
    equipment: &[Box<dyn Equipment>],
    name: &str,
    end_use: &EndUse,
    end_use_signal: &ControlSignal,
) -> bool {
    let reach = EndUseReach::of(end_use_signal);
    equipment.iter().any(|eq| {
        eq.descriptor().name.as_str() == name && addressed_by_end_use(&**eq, end_use, reach)
    })
}

impl Default for ControlDispatcher {
    fn default() -> Self {
        Self {
            // Each tier's queue pre-sizes at construction for one step's
            // queued-request bound, the same bound the actor dispatch
            // buffer's capacity carries: an event's requests (the EV
            // driver's arrival, the BMS's re-evaluation) queue inside
            // it, so a steady-state event never pays the queue's first
            // growth. `drain` keeps the capacity, so the fill is once
            // per queue, per process.
            by_tier: std::array::from_fn(|_| VecDeque::with_capacity(ACTOR_DISPATCH_BOUND)),
            seen_targets: Vec::with_capacity(16),
            #[cfg(feature = "observe")]
            conflict_scratch: Vec::new(),
            #[cfg(feature = "observe")]
            conflict_signal_pool: Vec::new(),
            #[cfg(feature = "observe")]
            recorded_from_scratch: Vec::new(),
        }
    }
}

impl ControlDispatcher {
    pub(super) fn queue(&mut self, request: DispatchRequest) {
        self.by_tier[request.priority.index()].push_back(request);
    }

    /// Resets the per-step priority ledger. Must be called once per timestep
    /// before the first `dispatch_into` call; subsequent dispatches within the
    /// step inherit the ledger so priority ordering holds across passes.
    pub(super) fn begin_step(&mut self) {
        self.seen_targets.clear();
    }

    pub(super) fn dispatch_into(
        &mut self,
        equipment: &mut [Box<dyn Equipment>],
        warnings: &mut WarningLog,
        rejected_control_signals: &mut RejectionGate,
    ) {
        self.drain_tiers(
            equipment,
            warnings,
            rejected_control_signals,
            |_, _, _, _| {},
        );
    }

    #[cfg(feature = "observe")]
    pub(super) fn dispatch_into_observed_into(
        &mut self,
        equipment: &mut [Box<dyn Equipment>],
        warnings: &mut WarningLog,
        rejected_control_signals: &mut RejectionGate,
        signals: &mut Vec<DispatchedSignal>,
        conflicts: Option<&mut Vec<SameTierConflict>>,
    ) {
        // The pass's conflict scan replaces the passed list's content
        // (`None`: the dispatcher's own scratch, the throwaway whose
        // records the merged capture never keeps); the signals append:
        // the step's merge target (the holder's retained Vec, cleared
        // once per step) concatenates every observing pass's signals in
        // dispatch order, so the captured content is the pre-pass's
        // conflicts plus every pass's signals, exactly what the
        // per-pass-fresh + merge shape produced.
        self.scan_same_tier_conflicts(conflicts, equipment);

        self.drain_tiers(
            equipment,
            warnings,
            rejected_control_signals,
            |request, delivered, overwrote, skipped| {
                signals.push(DispatchedSignal {
                    target: request.target.clone(),
                    signal: request.signal.clone(),
                    priority: request.priority,
                    overwrote_earlier: overwrote,
                    delivered: delivered && !skipped,
                });
            },
        );
    }

    /// One observing pass's same-tier conflict scan: every tier's
    /// conflicting request groups, recorded once per group with the
    /// group's signals in queue order, into the passed list (or the
    /// dispatcher's own scratch when the caller passes `None`). The
    /// scan's transients are persistent: the records' inner signals
    /// buffers rent from the pool and return from the cleared records,
    /// the recorded-from ledger clears per tier, and the requests are
    /// addressed through the tier's two slices directly, so a
    /// steady-state scan allocates nothing.
    #[cfg(feature = "observe")]
    fn scan_same_tier_conflicts(
        &mut self,
        conflicts: Option<&mut Vec<SameTierConflict>>,
        equipment: &[Box<dyn Equipment>],
    ) {
        let list: &mut Vec<SameTierConflict> = match conflicts {
            Some(list) => list,
            None => &mut self.conflict_scratch,
        };
        // The pass's scan replaces the list's content: the cleared
        // records' inner signals buffers return to the pool, the
        // buffers the next scan's records rent.
        for mut record in list.drain(..) {
            record.signals.clear();
            self.conflict_signal_pool.push(record.signals);
        }
        for (tier_idx, tier_que) in self.by_tier.iter().enumerate() {
            let (head, tail) = tier_que.as_slices();
            let count = head.len() + tail.len();
            // The borrow-free index form: the scan addresses request i
            // through the tier's two slices directly, with no collected
            // Vec of references per tier.
            let request = |i: usize| -> &DispatchRequest {
                if i < head.len() {
                    &head[i]
                } else {
                    &tail[i - head.len()]
                }
            };
            self.recorded_from_scratch.clear();
            for i in 0..count {
                for j in (i + 1)..count {
                    if !targets_conflict(request(i), request(j), equipment) {
                        continue;
                    }
                    let tier = PriorityTier::from_index(tier_idx);
                    let already_recorded = self
                        .recorded_from_scratch
                        .iter()
                        .any(|&k| targets_conflict(request(k), request(i), equipment));
                    if already_recorded {
                        continue;
                    }
                    let mut record_signals = self.conflict_signal_pool.pop().unwrap_or_default();
                    record_signals.clear();
                    record_signals.extend(
                        (0..count)
                            .map(request)
                            .filter(|r| {
                                r.priority == tier && targets_conflict(r, request(i), equipment)
                            })
                            .map(|r| r.signal.clone()),
                    );
                    self.recorded_from_scratch.push(i);
                    list.push(SameTierConflict {
                        tier,
                        target: request(i).target.clone(),
                        signals: record_signals,
                    });
                }
            }
        }
    }

    pub(super) fn drain_tiers(
        &mut self,
        equipment: &mut [Box<dyn Equipment>],
        warnings: &mut WarningLog,
        rejected_control_signals: &mut RejectionGate,
        mut on_signal: impl FnMut(&DispatchRequest, bool, bool, bool),
    ) {
        // NOTE: `seen_targets` is deliberately NOT cleared here. The per-step
        // ledger is reset by `begin_step()` exactly once per timestep so that
        // a lower-priority signal queued after a higher-priority one in an
        // earlier pass does not silently overwrite it.

        for (tier_idx, tier_que) in self.by_tier.iter_mut().enumerate() {
            for request in tier_que.drain(..) {
                match &request.target {
                    DispatchTarget::ByName(_) => {
                        let prior_higher = self.seen_targets.iter().any(|&(ref t, prev_tier)| {
                            t.conflicts_with(&request.target) && tier_idx < prev_tier
                        });
                        if prior_higher {
                            tracing::debug!(
                                target_equipment = ?request.target,
                                priority = ?request.priority,
                                "lower priority signal rejected: a higher priority signal already applied to this target"
                            );
                            on_signal(&request, false, false, true);
                            continue;
                        }

                        let overwrote = self.seen_targets.iter().any(|&(ref t, prev_tier)| {
                            t.conflicts_with(&request.target) && tier_idx > prev_tier
                        });
                        if overwrote {
                            tracing::debug!(
                                target_equipment = ?request.target,
                                priority = ?request.priority,
                                "higher priority signal overwriting earlier signal for same equipment"
                            );
                        }
                        self.seen_targets.push((request.target.clone(), tier_idx));

                        let delivered =
                            route_request(&request, equipment, warnings, rejected_control_signals);
                        on_signal(&request, delivered, overwrote, false);
                    }
                    DispatchTarget::ByEndUse(end_use) => {
                        // Expand ByEndUse to individual ByName targets per
                        // matching equipment. This ensures seen_targets contains
                        // only homogeneous ByName entries, so conflicts_with
                        // comparisons are reliable across dispatch passes.
                        let mut any_matched = false;
                        let mut any_delivered = false;
                        let mut any_overwrote = false;
                        let mut any_skipped = false;
                        let reach = EndUseReach::of(&request.signal);

                        for eq in equipment.iter_mut() {
                            if !addressed_by_end_use(&**eq, end_use, reach) {
                                continue;
                            }
                            // Matched: the signal was routed at a real target.
                            // An apply failure below is a per-equipment
                            // rejection, NOT a "target not found": the
                            // not-found increment must not fire for it
                            // (mirrors `apply_to_matching`, which marks
                            // delivered on match before applying).
                            any_matched = true;
                            let eq_name = eq.descriptor().name.clone();
                            let by_name = DispatchTarget::ByName(Arc::from(eq_name.as_str()));

                            let prior_higher =
                                self.seen_targets.iter().any(|&(ref t, prev_tier)| {
                                    t.conflicts_with(&by_name) && tier_idx < prev_tier
                                });
                            if prior_higher {
                                tracing::debug!(
                                    target_equipment = eq_name,
                                    priority = ?request.priority,
                                    "lower priority ByEndUse signal rejected: a higher priority signal already applied to this target"
                                );
                                any_skipped = true;
                                continue;
                            }

                            let overwrote = self.seen_targets.iter().any(|&(ref t, prev_tier)| {
                                t.conflicts_with(&by_name) && tier_idx > prev_tier
                            });
                            if overwrote {
                                any_overwrote = true;
                                tracing::debug!(
                                    target_equipment = eq_name,
                                    priority = ?request.priority,
                                    "higher priority signal overwriting earlier signal for same equipment via ByEndUse expansion"
                                );
                            }

                            self.seen_targets.push((by_name, tier_idx));

                            if let Err(err) = eq.apply_control(&request.signal) {
                                rejected_control_signals.record(&eq_name);
                                warnings.push_from(
                                    "control",
                                    format!("control apply failed for '{eq_name}' : {err}"),
                                );
                            } else {
                                any_delivered = true;
                            }
                        }

                        if !any_matched && !any_skipped {
                            rejected_control_signals.record(&format!("{end_use:?}"));
                            warnings.push_from(
                                "control",
                                format!("control target not found by end-use: {end_use:?}"),
                            );
                        }

                        on_signal(&request, any_delivered, any_overwrote, any_skipped);
                    }
                }
            }
        }

        // Code-logic check no simulation input can trigger: ByEndUse targets
        // are expanded to ByName unconditionally above, so a non-ByName entry
        // in the ledger can only be a dispatcher bug: the class of check
        // that stays a debug_assert (input-triggerable invariant enforcement
        // belongs to the always-on checks instead).
        #[cfg(debug_assertions)]
        debug_assert!(
            self.seen_targets
                .iter()
                .all(|(t, _)| matches!(t, DispatchTarget::ByName(_))),
            "seen_targets must contain only ByName entries — ByEndUse targets must be expanded before recording"
        );
    }
}

fn route_request(
    request: &DispatchRequest,
    equipment: &mut [Box<dyn Equipment>],
    warnings: &mut WarningLog,
    rejected_control_signals: &mut RejectionGate,
) -> bool {
    match &request.target {
        DispatchTarget::ByName(name) => {
            let delivered = apply_to_matching(
                equipment,
                &request.signal,
                warnings,
                rejected_control_signals,
                |eq| eq.descriptor().name.as_str() == &**name,
            );
            if !delivered {
                rejected_control_signals.record(name);
                let instance_count = equipment
                    .iter()
                    .filter(|eq| {
                        let n = eq.descriptor().name.as_str();
                        n == &**name || n.starts_with(&format!("{name} #"))
                    })
                    .count();
                if instance_count >= 2 {
                    warnings.push_from(
                        "control",
                        format!(
                            "control target '{name}' is ambiguous: {instance_count} instances exist. Use ByEndUse or a qualified name like '{name} #1'"
                        ),
                    );
                } else {
                    warnings.push_from(
                        "control",
                        format!("control target not found by name: {name}"),
                    );
                }
            }
            delivered
        }
        DispatchTarget::ByEndUse(end_use) => {
            let delivered = apply_to_matching(
                equipment,
                &request.signal,
                warnings,
                rejected_control_signals,
                |eq| addressed_by_end_use(eq, end_use, EndUseReach::of(&request.signal)),
            );
            if !delivered {
                rejected_control_signals.record(&format!("{end_use:?}"));
                warnings.push_from(
                    "control",
                    format!("control target not found by end-use: {end_use:?}"),
                );
            }
            delivered
        }
    }
}

fn apply_to_matching(
    equipment: &mut [Box<dyn Equipment>],
    signal: &hares_types::ControlSignal,
    warnings: &mut WarningLog,
    rejected_control_signals: &mut RejectionGate,
    matches: impl Fn(&dyn Equipment) -> bool,
) -> bool {
    let mut delivered = false;
    for eq in equipment.iter_mut() {
        if matches(&**eq) {
            delivered = true;
            if let Err(err) = eq.apply_control(signal) {
                rejected_control_signals.record(&eq.descriptor().name);
                warnings.push_from(
                    "control",
                    format!(
                        "control apply failed for '{}' : {err}",
                        eq.descriptor().name
                    ),
                );
            }
        }
    }
    delivered
}
/// Which units a `ByEndUse` target reaches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EndUseReach {
    /// Units whose current end use is the target's.
    Label,
    /// Also every unit whose thermostat serves the target end use's
    /// setpoint, whatever its current mode.
    ThermostatAxis,
}

impl EndUseReach {
    /// A thermostat-setpoint signal (an override, a delta or the release)
    /// moves only the axis its end use names, so it reaches a unit serving
    /// both setpoints in any mode. Every other signal (a mode override, a
    /// power limit, a DR level) acts on the whole unit, so it reaches a unit
    /// only while its current end use is the target's: a cooling
    /// curtailment must never switch off a unit that is heating.
    pub(super) fn of(signal: &ControlSignal) -> Self {
        match signal {
            ControlSignal::ThermalSetpoint { .. } | ControlSignal::ThermalSetpointDelta { .. } => {
                Self::ThermostatAxis
            }
            _ => Self::Label,
        }
    }
}

/// Whether a `ByEndUse(end_use)` target with `reach` addresses `eq`. A unit
/// serving both setpoints relabels its end use by its mode, so the label
/// alone reaches it only while it is in that mode.
pub(super) fn addressed_by_end_use(eq: &dyn Equipment, end_use: &EndUse, reach: EndUseReach) -> bool {
    eq.descriptor().end_use == *end_use
        || (reach == EndUseReach::ThermostatAxis
            && ThermostatAxis::of_end_use(end_use)
                .zip(eq.thermostat_axes())
                .is_some_and(|(axis, axes)| axes.serves(axis)))
}

/// Whether a change of the equipment from `before` to `after` takes away
/// a target's equipment: every unit it addresses, or every unit it
/// addresses that declares the capabilities it requires. An end-use
/// target takes its widest reach: a unit serving both setpoints serves
/// either end use.
pub(super) fn target_lost(
    target: &crate::ActorTarget,
    before: &[&dyn Equipment],
    after: &[&dyn Equipment],
) -> bool {
    let reached = |equipment: &[&dyn Equipment], with_capabilities: bool| {
        equipment.iter().any(|eq| {
            let addressed = match &target.target {
                DispatchTarget::ByName(name) => eq.descriptor().name.as_str() == &**name,
                DispatchTarget::ByEndUse(end_use) => {
                    addressed_by_end_use(*eq, end_use, EndUseReach::ThermostatAxis)
                }
            };
            addressed
                && (!with_capabilities
                    || eq
                        .descriptor()
                        .control_capabilities
                        .contains(target.required))
        })
    };
    [false, true].into_iter().any(|with_capabilities| {
        reached(before, with_capabilities) && !reached(after, with_capabilities)
    })
}

/// A target as a warning names it.
pub(super) fn describe_target(target: &DispatchTarget) -> String {
    match target {
        DispatchTarget::ByName(name) => format!("'{name}'"),
        DispatchTarget::ByEndUse(end_use) => format!("end use '{}'", end_use.as_str()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::testing::TestEquipment;
    use hares_control::PriorityTier;
    use hares_types::{ControlCapabilities, telemetry_keys as tk};

    #[test]
    fn control_dispatcher_routes_by_tier_schedule_applied_first() {
        let mut dispatcher = ControlDispatcher::default();
        let signal_schedule = ControlSignal::ThermalSetpoint {
            heating_setpoint_c: Some(20.0),
            cooling_setpoint_c: None,
            deadband_c: None,
        };
        let signal_grid = ControlSignal::ThermalSetpoint {
            heating_setpoint_c: Some(18.0),
            cooling_setpoint_c: None,
            deadband_c: None,
        };

        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Test")),
            signal: signal_grid,
            priority: PriorityTier::Grid,
        });
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Test")),
            signal: signal_schedule,
            priority: PriorityTier::Schedule,
        });

        assert_eq!(dispatcher.by_tier[0].len(), 1);
        assert_eq!(dispatcher.by_tier[2].len(), 1);
        assert!(dispatcher.by_tier[1].is_empty());
        assert!(dispatcher.by_tier[3].is_empty());
    }

    #[test]
    fn control_dispatcher_drains_all_tiers_in_order() {
        let mut dispatcher = ControlDispatcher::default();
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("A")),
            signal: ControlSignal::ThermalSetpoint {
                heating_setpoint_c: Some(20.0),
                cooling_setpoint_c: None,
                deadband_c: None,
            },
            priority: PriorityTier::Schedule,
        });
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("B")),
            signal: ControlSignal::ThermalSetpoint {
                heating_setpoint_c: Some(21.0),
                cooling_setpoint_c: None,
                deadband_c: None,
            },
            priority: PriorityTier::Grid,
        });

        let mut warnings = WarningLog::new();
        let equipment: &mut [Box<dyn Equipment>] = &mut [];
        dispatcher.dispatch_into(equipment, &mut warnings, &mut RejectionGate::new(0, None));

        assert!(dispatcher.by_tier.iter().all(|q| q.is_empty()));
    }

    #[test]
    fn control_dispatcher_higher_priority_wins_over_lower() {
        let eq = TestEquipment::new("Heater", ControlCapabilities::POWER_SETPOINT);

        let mut dispatcher = ControlDispatcher::default();

        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Heater")),
            signal: ControlSignal::PowerSetpoint {
                active_power_w: 1000.0,
                reactive_power_var: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Schedule,
        });
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Heater")),
            signal: ControlSignal::PowerSetpoint {
                active_power_w: 5000.0,
                reactive_power_var: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Grid,
        });

        let mut warnings = WarningLog::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        dispatcher.dispatch_into(
            &mut equipment,
            &mut warnings,
            &mut RejectionGate::new(0, None),
        );

        assert!(warnings.is_empty());
        assert_eq!(
            equipment[0].telemetry().get(tk::LAST_POWER_KW),
            Some(5000.0)
        );
    }

    #[test]
    fn control_dispatcher_safety_priority_wins_over_all() {
        let eq = TestEquipment::new("Heater", ControlCapabilities::POWER_SETPOINT);

        let mut dispatcher = ControlDispatcher::default();

        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Heater")),
            signal: ControlSignal::PowerSetpoint {
                active_power_w: 1000.0,
                reactive_power_var: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Schedule,
        });
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Heater")),
            signal: ControlSignal::PowerSetpoint {
                active_power_w: 2000.0,
                reactive_power_var: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::UserOverride,
        });
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Heater")),
            signal: ControlSignal::PowerSetpoint {
                active_power_w: 3000.0,
                reactive_power_var: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Grid,
        });
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Heater")),
            signal: ControlSignal::PowerSetpoint {
                active_power_w: 0.0,
                reactive_power_var: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Safety,
        });

        let mut warnings = WarningLog::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        dispatcher.dispatch_into(
            &mut equipment,
            &mut warnings,
            &mut RejectionGate::new(0, None),
        );

        assert!(warnings.is_empty());
        assert_eq!(equipment[0].telemetry().get(tk::LAST_POWER_KW), Some(0.0));
    }

    #[test]
    fn control_dispatcher_warns_on_missing_target_by_name() {
        let eq = TestEquipment::new("Heater", ControlCapabilities::POWER_SETPOINT);

        let mut dispatcher = ControlDispatcher::default();
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("NonExistent")),
            signal: ControlSignal::PowerSetpoint {
                active_power_w: 1000.0,
                reactive_power_var: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Schedule,
        });

        let mut warnings = WarningLog::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        dispatcher.dispatch_into(
            &mut equipment,
            &mut warnings,
            &mut RejectionGate::new(0, None),
        );

        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0]
                .message
                .contains("control target not found by name")
        );
        assert!(warnings[0].message.contains("NonExistent"));
    }

    #[test]
    fn control_dispatcher_warns_on_missing_target_by_end_use() {
        let eq = TestEquipment::new("Heater", ControlCapabilities::POWER_SETPOINT);

        let mut dispatcher = ControlDispatcher::default();
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByEndUse(EndUse::BATTERY),
            signal: ControlSignal::PowerSetpoint {
                active_power_w: 1000.0,
                reactive_power_var: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Schedule,
        });

        let mut warnings = WarningLog::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        dispatcher.dispatch_into(
            &mut equipment,
            &mut warnings,
            &mut RejectionGate::new(0, None),
        );

        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0]
                .message
                .contains("control target not found by end-use")
        );
        assert!(warnings[0].message.contains("battery"));
    }

    #[test]
    fn control_dispatcher_warns_on_unsupported_signal() {
        let eq = TestEquipment::new("Heater", ControlCapabilities::empty());

        let mut dispatcher = ControlDispatcher::default();
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Heater")),
            signal: ControlSignal::PowerSetpoint {
                active_power_w: 1000.0,
                reactive_power_var: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Schedule,
        });

        let mut warnings = WarningLog::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        dispatcher.dispatch_into(
            &mut equipment,
            &mut warnings,
            &mut RejectionGate::new(0, None),
        );

        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("control apply failed"));
    }

    #[test]
    fn control_dispatcher_routes_to_custom_end_use() {
        // Create equipment with a custom end use
        let mut eq = TestEquipment::new("HPWH", ControlCapabilities::POWER_SETPOINT);
        eq.descriptor.end_use = EndUse::custom("heat_pump_water_heater");

        let mut dispatcher = ControlDispatcher::default();
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByEndUse(EndUse::custom("heat_pump_water_heater")),
            signal: ControlSignal::PowerSetpoint {
                active_power_w: 2500.0,
                reactive_power_var: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Schedule,
        });

        let mut warnings = WarningLog::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        dispatcher.dispatch_into(
            &mut equipment,
            &mut warnings,
            &mut RejectionGate::new(0, None),
        );

        // Should find the equipment and apply the signal
        assert!(warnings.is_empty(), "unexpected warnings: {:?}", warnings);
        assert_eq!(
            equipment[0].telemetry().get(tk::LAST_POWER_KW),
            Some(2500.0)
        );
    }

    #[test]
    fn control_dispatcher_custom_end_use_misses_different_custom() {
        // Equipment with one custom end use, dispatch to a different custom end use
        let mut eq = TestEquipment::new("HPWH", ControlCapabilities::POWER_SETPOINT);
        eq.descriptor.end_use = EndUse::custom("heat_pump_water_heater");

        let mut dispatcher = ControlDispatcher::default();
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByEndUse(EndUse::custom("ice_storage")), // Different custom type
            signal: ControlSignal::PowerSetpoint {
                active_power_w: 1000.0,
                reactive_power_var: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Schedule,
        });

        let mut warnings = WarningLog::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        dispatcher.dispatch_into(
            &mut equipment,
            &mut warnings,
            &mut RejectionGate::new(0, None),
        );

        // Should not find the equipment (different custom end use)
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0]
                .message
                .contains("control target not found by end-use")
        );
        assert!(warnings[0].message.contains("ice_storage"));
    }

    #[test]
    fn control_dispatcher_warns_on_missing_custom_end_use() {
        let eq = TestEquipment::new("Heater", ControlCapabilities::POWER_SETPOINT);
        // Equipment has standard end use, dispatch targets custom end use

        let mut dispatcher = ControlDispatcher::default();
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByEndUse(EndUse::custom("novel_equipment_type")),
            signal: ControlSignal::PowerSetpoint {
                active_power_w: 1000.0,
                reactive_power_var: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Schedule,
        });

        let mut warnings = WarningLog::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        dispatcher.dispatch_into(
            &mut equipment,
            &mut warnings,
            &mut RejectionGate::new(0, None),
        );

        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0]
                .message
                .contains("control target not found by end-use")
        );
        assert!(warnings[0].message.contains("novel_equipment_type"));
    }
}
