# EV driver charging strategy composer: policy composition logic
**Review ID**: agents-06
**Category**: agents
**Date**: 2026-05-26

## Files Reviewed
crates/hares-core/src/actors/ev_driver/composer.rs crates/hares-core/src/actors/ev_driver/*.rs

## Vendor/Reference Files Consulted
vendors/OCHRE/ochre/Equipment/EV.py

## Findings
### Finding 1: [Severity: critical]
**Description**: V2G and V2H min_soc values are silently discarded when the resolved vote emits a PowerSetpoint. The `emit_vote` function takes three mutually exclusive paths: if `power_kw` is set (path 2), it emits `PowerSetpoint { active_power_kw, reactive_power_kvar: None }` which carries no `min_soc` or `max_soc` fields. The `min_soc` that V2G/V2H include in their PreferenceVote for the scoring phase is only forwarded to the equipment when the resolved vote takes the SOCTarget path (path 3). The constraint-phase SOC floor check (`current_soc <= min_soc → Override(idle)`) provides per-timestep gating, but once a discharge is authorized, the equipment receives no min_soc constraint in the dispatch signal. If the equipment's BMS does not independently enforce minimum SOC, the battery could be discharged below the floor between dispatch cycles or if the commanded power overshoots due to CC-CV nonlinearities.

**Code Location**: `composer.rs:150-193` (emit_vote paths), `v2g.rs:20-37` (V2G score includes min_soc), `v2h.rs:20-38` (V2H score includes min_soc)

**Root Cause**: The `PowerSetpoint` variant in `ControlSignal` (control_signal.rs:49-52) has no `min_soc` field, so the resolved vote's `min_soc`/`max_soc` metadata cannot be conveyed through that signal type. The composer does not add a separate `SOCTarget` dispatch alongside the `PowerSetpoint` to communicate the floor.

**Impact**: Deep discharge damage to the EV battery if the equipment BMS lacks its own SOC floor enforcement. Contrast with OCHRE (`EV.py:298`) which always clamps `ac_power = min(max(self.p_setpoint, 0), soc_max_power)` at the equipment level.

### Finding 2: [Severity: high]
**Description**: No output power clamping in the composer against `DecisionContext::max_charge_kw` or `max_discharge_kw`. Individual strategies self-clamp (e.g., `SolarTracking:19` uses `surplus.min(ctx.max_charge_kw)`, `V2H:29` uses `deficit.min(self.max_discharge_kw)`), but the resolver's "most conservative power" logic picks the smallest absolute value from among votes. If only one preference specifies a power value and that preference has a bug or is a newly added custom preference without clamping, the composer emits an unbounded power value.

**Code Location**: `composer.rs:100-112` (resolve: most conservative power by smallest absolute value, no clamping against context limits), `composer.rs:150-179` (emit_vote passes resolved power through without bounds check)

**Root Cause**: The resolve function delegates power clamping entirely to individual preferences; the composer has no defense-in-depth clamp at the output boundary. OCHRE centralizes this at the equipment level (`EV.py:298`).

**Impact**: Potential dispatch of power values exceeding the EVSE rating or the vehicle's onboard charger limit if a preference implementation is incorrect.

### Finding 3: [Severity: high]
**Description**: Constraint priority in `evaluate()` is strictly order-dependent with no explicit priority tier. The first Override from the ordered preference list wins (composer.rs:42-51). The priority hierarchy is determined implicitly by the position within `build_preferences()` in mod.rs:73-199. For example, in `TouAware`, PriceOptimizer is at index 0 (no constraint) and DepartureDeadline at index 1 (can Override) — departure urgency correctly takes precedence over price. In `V2H`, SocTarget is at index 0 (no constraint) and V2HDischarge at index 1 (SOC floor Override). If someone rearranges preferences or adds a custom strategy, safety-critical constraints could be demoted below economic signals.

**Code Location**: `composer.rs:42-51` (first-Override-wins loop), `mod.rs:132-154` (TouAware preference ordering), `mod.rs:171-183` (V2H preference ordering), `mod.rs:184-197` (V2G preference ordering)

**Root Cause**: No explicit constraint priority field or tier exists in the `Constraint` enum or the `ChargingPreference` trait. Safety constraints (SOC floor, departure urgency) rely on insertion order for precedence.

**Impact**: Reordering preferences could allow economic signals (price, solar) to override safety constraints (SOC floor, departure deadline). Combined with Finding 1, this could allow deep discharge during high-price periods when SOC is near the floor.

### Finding 4: [Severity: medium]
**Description**: All dispatch requests from the composer use `PriorityTier::Schedule` regardless of the nature of the signal. Urgent departure overrides, SOC floor constraints, and range-anxiety overrides all share the lowest priority tier with economic dispatch signals. The `PriorityTier` enum has `Safety = 3` and `UserOverride = 1` tiers available but unused. If another controller dispatches a conflicting `Schedule`-tier signal to the same EV equipment in the same timestep, there is no priority differentiation to ensure the composer's safety signals win.

**Code Location**: `composer.rs:159` (EvSetReadyBy priority), `composer.rs:176` (PowerSetpoint priority), `composer.rs:190` (SOCTarget priority), `mod.rs:471-483` (range anxiety override also uses Schedule)

**Root Cause**: The composer hard-codes `PriorityTier::Schedule` for all outputs. There is no mechanism to elevate safety-critical dispatch requests to `Safety` tier.

**Impact**: Safety-critical signals (SOC floor blocking, departure urgency) could be preempted by other controllers at the same priority level. Low practical impact currently since the dispatch system processes Schedule-tier signals from all actors, but no defense against future multi-controller conflicts.

### Finding 5: [Severity: medium]
**Description**: When all sub-strategies return idle votes (no `power_kw`, no `target_soc`), the composer produces zero dispatch requests. The `resolve()` function returns a vote with all fields set to `None`/`NEG_INFINITY` and the label `"idle"`. `emit_vote` checks each output path and emits nothing. This is the correct safe default (no power commanded), but it relies on the equipment maintaining its current state when no dispatch arrives. This behavior is not documented.

**Code Location**: `composer.rs:81-148` (resolve produces idle vote when all inputs idle), `composer.rs:150-193` (emit_vote emits nothing for idle vote)

**Root Cause**: The zero-dispatch behavior for all-idle is implicit in the emit_vote control flow, not an explicit `"emit zero power"` or `"maintain current state"` command.

**Impact**: Equipment state during idle periods is undefined at the composer level; correct behavior depends on the equipment's default state machine. No functional bug, but behavior could be surprising to new strategy authors.

### Finding 6: [Severity: medium]
**Description**: SocGate's threshold is enforced as a binary constraint in the `constraint()` method (Override idle when SOC >= threshold), but SocGate does not set `max_soc` in its `score()` output when SOC is below threshold. If another preference scores higher and votes for a `target_soc` above the SocGate threshold, the resolved target_soc can exceed the gate. The threshold is not reflected as a soft cap in the scoring phase.

**Code Location**: `soc_gate.rs:11-17` (constraint phase blocks when above threshold), `soc_gate.rs:19-33` (score method sets `target_soc` but not `max_soc`)

**Root Cause**: SocGate treats the threshold as a hard on/off gate via Override, but does not propagate `max_soc` in the scoring phase. If `QuickThenWait` or `LowSoc` is the only strategy in use (no other preferences), this is fine. But if SocGate is ever composed with other strategies, the threshold could be breached.

**Impact**: In compound strategy configurations, SOC could charge beyond the intended gate threshold. The `build_preferences` function in mod.rs currently only uses SocGate in isolation (LowSoc, QuickThenWait), so this is a latent issue rather than an active bug.

### Finding 7: [Severity: low]
**Description**: The `resolve()` function accumulates `max_min_soc` (maximum of all votes' min_soc) and `max_max_soc` (minimum of all votes' max_soc), but these computed bounds are only included in the output when the resolved vote takes the SOCTarget path (emit_vote path 3). If the resolved vote takes the PowerSetpoint path (path 2), the aggregated min/max SOC constraints from all sub-strategies are lost. The aggregated constraints are a valuable composition output but are only preserved for the SOCTarget signal type.

**Code Location**: `composer.rs:114-128` (resolve computes max_min_soc and max_max_soc), `composer.rs:150-193` (emit_vote paths — only SOCTarget path includes them)

**Root Cause**: Same structural limitation as Finding 1 — PowerSetpoint lacks SOC fields.

**Impact**: Lost information; no runtime bug but reduces the composer's ability to convey all resolved constraints.

### Finding 8: [Severity: low]
**Description**: OCHRE (`EV.py:298`) explicitly enforces `soc_max_power = (self.soc_max_ctrl - self.soc) * self.capacity / hours / EV_EFFICIENCY` at the equipment level to prevent overcharging in the current timestep. HARES has no equivalent energy-rate SOC clamp in composer or in the actor. The actor relies on `ControlSignal::SOCTarget` carrying `max_soc` to the equipment, but the equipment-side clamp is not visible at the composer review scope.

**Code Location**: `EV.py:296-300` (OCHRE SOC rate clamp), `composer.rs:150-193` (HARES emit_vote — no energy-rate clamp)

**Root Cause**: Architectural difference — OCHRE centralizes SOC-based power limiting at equipment level; HARES distributes it across actor (via signals) and equipment (via BMS). The composer does not have an energy-budget-aware clamp.

**Impact**: The composer could command a power level that would overshoot SOC in a single timestep. Mitigated if the equipment BMS performs SOC-rate clamping, but this coupling is not enforced by the composer.

## Summary
- Total findings: 8
- Critical: 1
- High: 2
- Medium: 3
- Low: 2

## Recommendations
1. Add `min_soc` and `max_soc` fields to the `ControlSignal::PowerSetpoint` variant, or emit both a `PowerSetpoint` and a `SOCTarget` when V2G/V2H strategies vote a negative power with SOC constraints. This is the single most impactful fix (Finding 1).
2. Add output power clamping in `emit_vote` (or `resolve`) against `max_charge_kw`/`max_discharge_kw` as a defense-in-depth measure (Finding 2).
3. Add an explicit priority field to `Constraint` or to the preference ordering so safety-critical constraints (SOC floor, departure urgency) are structurally guaranteed to precede economic signals (Finding 3).
4. Elevate safety-critical dispatch requests to `PriorityTier::Safety` (e.g., departure urgency with < 30 min to deadline, SOC floor constraints) (Finding 4).
5. Document the all-idle zero-dispatch behavior in the composer module doc comment (Finding 5).
6. Have `SocGate.score()` set `max_soc: Some(self.threshold)` to propagate the soft cap through the scoring phase (Finding 6).

## References / Citations
- `composer.rs:42-52` — constraint check iteration order
- `composer.rs:100-112` — most-conservative power resolution
- `composer.rs:150-179` — PowerSetpoint emission discards min_soc/max_soc
- `composer.rs:181-193` — SOCTarget emission carries min_soc/max_soc
- `v2g.rs:20-37` — V2G scoring includes min_soc in vote
- `v2h.rs:20-38` — V2H scoring includes min_soc in vote
- `soc_gate.rs:11-33` — SocGate constraint + scoring (threshold as binary gate)
- `mod.rs:73-199` — build_preferences defines strategy composition order
- `mod.rs:471-483` — range anxiety override with Schedule priority
- `EV.py:286-298` — OCHRE centralizes power clamping at equipment level
