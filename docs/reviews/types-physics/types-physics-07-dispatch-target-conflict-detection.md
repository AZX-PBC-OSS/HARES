# DispatchTarget conflict detection by-name vs by-end-use
**Review ID**: types-physics-07
**Category**: types-physics
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-control/src/dispatch.rs` (203 lines)
- `crates/hares-control/src/signal.rs` (228 lines)
- `crates/hares-core/src/dwelling/mod.rs` (Lines 355–528: ControlDispatcher, drain_tiers, route_request, apply_to_matching)

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Equipment/Equipment.py` (Lines 130–258: `calculate_mode_priority`, `update_model`, `update_external_control` base)
- `vendors/OCHRE/ochre/Dwelling.py` (Lines 139–148: `equipment_by_end_use` routing table; Lines 236–246: `update_model` end-use-to-equipment broadcast)
- `vendors/OCHRE/ochre/Simulator.py` (Lines 234–257: `start_sub_update`, `update_model` sub-equipment dispatch)
- `vendors/OCHRE/ochre/Equipment/Battery.py` (Lines 169–197: `update_external_control` — P Setpoint > SOC > Self-Consumption override chain)

---

## Findings

### Finding 1: [Severity: high]
**Description**: `conflicts_with()` cannot detect cross-variant targeting — a `DispatchTarget::ByName` and a `DispatchTarget::ByEndUse` that resolve to the same physical equipment are reported as non-conflicting.

**Code Location**: `crates/hares-control/src/dispatch.rs:52-58`

```rust
pub fn conflicts_with(&self, other: &Self) -> bool {
    match (self, other) {
        (Self::ByName(a), Self::ByName(b)) => a == b,
        (Self::ByEndUse(a), Self::ByEndUse(b)) => a == b,
        _ => false,
    }
}
```

**Root Cause**: `conflicts_with` only compares same-variant pairs. When variants differ (`ByName` vs `ByEndUse`), it unconditionally returns `false` (line 56). Both variants resolve through `route_request` (`dwelling/mod.rs:466-494`) to the same equipment array — `ByName` matches by `eq.descriptor().name`, `ByEndUse` matches by `eq.descriptor().end_use` — and can target the same physical equipment instance. The unit test at `dispatch.rs:166-170` (`conflicts_with_different_variants_never_conflict`) explicitly asserts this non-detection as a known design invariant.

**Impact**: Priority inversion across dispatch passes. Consider:
1. Pass 1: A Safety-tier signal (e.g., grid emergency disconnect) targets `EndUse::BATTERY` → applied to battery equipment → `seen_targets` records `(ByEndUse(BATTERY), 3)`.
2. Pass 2: A Schedule-tier signal (e.g., internal thermostat schedule) targets `ByName("Battery #1")` → `conflicts_with(ByEndUse(BATTERY), ByName("Battery #1"))` returns `false` → `prior_higher` check (`mod.rs:434-436`) fails to detect the existing entry → the lower-priority Schedule signal is **applied**, overwriting the Safety signal's battery control. This is a genuine priority inversion pathway: a Schedule-tier actor can silently overwrite a Safety-tier grid protection.

The cross-variant gap is architecturally enabled by HARES's dual-target model. In practice, internal actors use `compute_equipment_dispatch_targets` (`mod.rs:523-528`) which always produces `ByName` targets, but nothing prevents an external controller (via `queue_dispatch` at `mod.rs:1498`) or a custom actor from targeting by end-use. The `ByEndUse` variant is fully public and tested (`dispatch.rs:100-112`), making cross-variant conflicts a latent risk.

**OCHRE Comparison**: OCHRE avoids this problem by *not having a dual-target model*. OCHRE's `Dwelling.update_model` (Dwelling.py:236-244) broadcasts signals from end-use keys to individual equipment name keys within the same `control_signal` dict, using the guard `if equipment.name not in control_signal` (line 243) so that per-equipment name keys take precedence over end-use keys. This guarantees that all signals for the same equipment arrive under the same key (the equipment name) before `start_sub_update` dispatches to `Equipment.update_model`. There is no scenario where one signal targets by name and another by end-use for the same equipment — the routing layer normalizes them before dispatch.

---

### Finding 2: [Severity: medium]
**Description**: Conflict detection operates at target granularity only — two signals controlling *different aspects* of the same equipment (e.g., SOC target vs. power setpoint on a battery) are treated as conflicts if they share a dispatch target, even when the underlying `ControlSignal` variants do not conflict.

**Code Location**: `crates/hares-core/src/dwelling/mod.rs:428-462` (`drain_tiers`)

```rust
for (tier_idx, tier_que) in self.by_tier.iter_mut().enumerate() {
    for request in tier_que.drain(..) {
        let prior_higher = self.seen_targets.iter().any(|&(ref t, prev_tier)| {
            t.conflicts_with(&request.target) && tier_idx < prev_tier
        });
        if prior_higher {
            // ... skip entirely ...
            continue;
        }
        // ... apply ...
    }
}
```

**Root Cause**: The `conflicts_with` / `prior_higher` gate operates on the `DispatchTarget`, not on the `ControlSignal` payload. When a higher-tier signal for a target is in the ledger, subsequently queued lower-tier signals for the *same target* are **entirely rejected** (line 437-444), regardless of whether the signals control orthogonal parameters. For example:
- A `Grid`-tier DR event sets `PowerSetpoint(-2.0)` on `ByName("Battery #1")` (registered in `seen_targets` at tier 2).
- A `Schedule`-tier TOU signal sets `SOCTarget(0.8, ...)` on `ByName("Battery #1")` → `conflicts_with` returns `true` (same name), `tier_idx(0) < prev_tier(2)` → **rejected**.

The battery's `apply_control` method handles `PowerSetpoint` and `SOCTarget` as independent fields (`power_setpoint_kw` and `soc_target` respectively), so they could coexist. But the dispatcher prevents it.

**Impact**:
1. **Over-blocking**: Legitimate control stacking is prevented. In a real deployment, a battery might receive a DR power setpoint *and* a TOU SOC schedule simultaneously — the DR takes precedence on power, but the TOU SOC target should persist as a fallback when the DR event expires. With the current dispatcher, the TOU SOC target is silently dropped.
2. **Signal expiry asymmetry**: DR signals expire via `dr_duration_remaining_s` (in battery's `update_control`), reverting to `Normal` level. When the DR event expires, there is no "restore previous" mechanism — the battery reverts to internal control defaults, not the TOU schedule that was previously blocked.

**OCHRE Comparison**: OCHRE handles this at the *signal level within each equipment's `update_external_control`*. In `Battery.update_external_control` (Battery.py:169-197), OCHRE processes `Min SOC`, `Max SOC`, `SOC`, and `P Setpoint` sequentially. The explicit comment at line 191 states `"Note: P Setpoint overrides SOC control"` — this is a *selective* override: if both `SOC` and `P Setpoint` are present in the same call, the `SOC` branch is skipped (SOC is not applied), but `P Setpoint` still gets processed. If only `SOC` is present, the SOC calculation proceeds. This fine-grained, signal-level conflict resolution allows coexistence of partial control without blocking unrelated parameters.

---

### Finding 3: [Severity: medium]
**Description**: No signal-type-aware conflict resolution exists for simultaneous same-tier dispatches — when two signals at the same priority tier target the same equipment, both are applied with a de facto last-writer-wins order that is non-deterministic across passes.

**Code Location**: `crates/hares-core/src/dwelling/mod.rs:434-445` and `dispatch.rs:63-67` (`DispatchRequest` priority field)

```rust
let prior_higher = self.seen_targets.iter().any(|&(ref t, prev_tier)| {
    t.conflicts_with(&request.target) && tier_idx < prev_tier
});
if prior_higher {
    // skip (already rejected Finding 1 scenario)
    continue;
}
// No same-tier rejection — both pass through
```

**Root Cause**: The `prior_higher` gate uses strict less-than (`tier_idx < prev_tier`), not less-than-or-equal. Two signals at the same tier for the same target both enter `route_request` and `apply_to_matching`. The second one overwrites the first's effects on any overlapping equipment fields. Within a single pass, order is deterministic (VecDeque FIFO). Across passes, the order depends on which actor's `decide` is called first.

**Impact**: For energy-drift scenarios:
1. **DR + DR at Grid tier**: Two DR programs targeting the same battery simultaneously (e.g., utility CPP event + aggregator frequency regulation) — the second wins, the first is silently lost.
2. **TOU + TOU at Schedule tier**: Two schedule-based actors targeting the same HVAC (pre-cool via thermostat schedule + duty cycle schedule) — no precedence, arbitrary winner.
3. **Across-pass non-determinism**: If Actor A emits `Schedule` tier signals in pass 2 and Actor B emits different `Schedule` tier signals for the same target in pass 3, both are applied and the second overwrites the first. The ledger does not prevent same-tier cross-write.

The correct behavior for same-tier conflicts is domain-dependent: DR+DR might require economic arbitration (choose the more valuable DR program), while TOU+TOU might require schedule merging. The current dispatcher provides no framework for this — it silently applies both.

**OCHRE Comparison**: OCHRE's same-equipment sequential processing within `update_external_control` is explicit about override chains. In `Battery.py`, the code explicitly checks `"SOC" not in control_signal` before computing SOC setpoint from P Setpoint, and `"P Setpoint" not in control_signal` before computing it from SOC — making the override hierarchy explicit and documented. The `calculate_mode_priority` method (Equipment.py:134-174) provides a formal priority system for duty-cycle-based control, prioritizing the current mode first and then honoring remaining time budgets.

---

### Finding 4: [Severity: medium]
**Description**: The `seen_targets` ledger uses a `Vec<(DispatchTarget, usize)>` with per-target-per-tier duplicate entries, rather than a `HashMap` that collapses to only the maximum tier per target. This causes unbounded O(n) growth per step and lost optimization opportunities.

**Code Location**: `crates/hares-core/src/dwelling/mod.rs:362-366` (definition) and `line 457` (push) and `line 434` (O(n) scan)

```rust
// Definition: Vec accumulates duplicates
seen_targets: Vec<(DispatchTarget, usize)>,
// ...
// Every applied signal pushes a new entry
self.seen_targets.push((request.target.clone(), tier_idx));
// ...
// O(n) scan through accumulated entries
let prior_higher = self.seen_targets.iter().any(|&(ref t, prev_tier)| { ... });
```

**Root Cause**: When a higher-priority signal overwrites a lower-priority one for the same target, the old entry is never removed. Example with one equipment receiving 3 tiers of signals:
```
Schedule(Battery, 0)   → seen_targets = [(Battery, 0)]
Grid(Battery, 2)       → seen_targets = [(Battery, 0), (Battery, 2)]
Safety(Battery, 3)     → seen_targets = [(Battery, 0), (Battery, 2), (Battery, 3)]
```
For `N` equipment × `T` tiers per timestep, size grows to `N × T` entries. The prior_higher check (line 434) and overwrote check (line 447) both scan all entries per request, yielding `O(N × T)` complexity per dispatch pass.

**Impact**: This is primarily a performance and clarity concern rather than a correctness bug. While the priority ordering logic remains correct (the highest-tier entry per target wins), the data structure choice:
- Drives O(n) linear scanning instead of O(1) hash lookups
- Accumulates unnecessary duplicate history entries that serve no purpose after a higher tier overwrites them
- Complicates reasoning about state: only the `max(tier_idx)` per target matters

**OCHRE Comparison**: OCHRE has no equivalent issue because all signals for a timestep arrive in a single dict and are processed in a single call to `update_model` — there is no cross-pass ledger or accumulated target state.

---

### Finding 5: [Severity: low]
**Description**: No infrastructure exists to bridge `ByName` and `ByEndUse` dispatch targets — there is no equipment-name-to-end-use or end-use-to-equipment-names mapping accessible to the conflict detector.

**Code Location**: `crates/hares-core/src/dwelling/mod.rs:523-528` (only path is `ByName` constructor) and absence of any `end_use_by_name` or `names_by_end_use` lookup table.

**Root Cause**: HARES's `EquipmentDescriptor` (`equipment.rs:1115-1133`) contains both `name: String` and `end_use: EndUse`, establishing a structural link. But this link is only used in `route_request` for routing — not for conflict detection. The `ControlDispatcher` only knows about queued `DispatchRequest` targets; it has no access to the equipment list or its descriptors. The `conflicts_with` method on `DispatchTarget` is a pure type-level function with no runtime context.

**Impact**:
1. Fixing Finding 1 requires either: (a) restricting dispatch to `ByName` only (breaking the `ByEndUse` interface), or (b) maintaining runtime mapping between names and end-uses accessible to the dispatcher. Neither exists today.
2. Without this mapping, any attempt to make `conflicts_with` cross-variant-aware would require passing the full equipment array to the method, breaking its zero-allocation promise and coupling it to runtime state.

**OCHRE Comparison**: OCHRE's `equipment_by_end_use` dict (Dwelling.py:139) is the canonical bridge — it is built at Dwelling init by iterating all equipment and grouping by `end_use`. HARES has no equivalent.

---

### Finding 6: [Severity: low]
**Description**: When a lower-priority signal is rejected by `prior_higher`, there is no mechanism to "hold" it and apply it if the higher-priority signal expires. This prevents layered-control designs like "DR overrides TOU, but TOU resumes after DR event."

**Code Location**: `crates/hares-core/src/dwelling/mod.rs:437-444` (rejection path)

```rust
if prior_higher {
    tracing::debug!(...);
    on_signal(&request, false, false, true);  // just logs, discards
    continue;
}
```

**Root Cause**: The dispatcher has no concept of signal expiry, subscription stacking, or fallback. When a lower-priority signal is rejected, it is permanently lost for that timestep. The only expiry mechanism is the battery's internal `dr_duration_remaining_s` counter (battery `update_control`), which operates at the equipment level, not the dispatcher level. When a DR power setpoint expires, the equipment's `self.power_setpoint_kw` is set to `None`, and the equipment falls back to internal (non-TOU) control — the previously blocked TOU schedule is not restored.

**Impact**: For realistic TOU + DR coexistence:
1. A DR event at Grid tier blocks a TOU schedule at Schedule tier for the same equipment.
2. The DR event expires after N timesteps.
3. The TOU schedule was rejected and discarded — the equipment reverts to its base internal control, not the TOU schedule.
4. The TOU actor must re-issue its signal every timestep (which it likely does since actors run each step), but the DR override on that specific timestep is lost permanently.

**OCHRE Comparison**: OCHRE's schedule-based persistence model (`current_schedule` dict) provides a natural fallback: when external control is present, it updates the schedule; when absent, the schedule's default value applies. HARES's persistent-setpoint model (Finding 4 of core-04) is complementary but lacks the "schedule as fallback" concept.

---

## Summary
- **Total findings**: 6
- **High**: 1 (cross-variant `conflicts_with` gap between `ByName` and `ByEndUse`)
- **Medium**: 3 (signal-level vs. target-level granularity mismatch, same-tier non-determinism, Vec ledger accumulation)
- **Low**: 2 (no name↔end-use bridge infrastructure, no fallback for rejected lower-priority signals)

## Recommendations

1. **Fix cross-variant conflict detection** (Finding 1, Finding 5): The simplest high-confidence fix is to deprecate `DispatchTarget::ByEndUse` for internal dispatch and restrict all per-equipment control to `ByName`. `compute_equipment_dispatch_targets` already produces only `ByName` targets, and all internal actors use `ByName`. The `ByEndUse` variant could be retained for broadcast-style signals (e.g., "all batteries") but would not participate in the per-equipment conflict detection mechanism. Alternatively, introduce a `resolve_to_names` method that takes the equipment list and returns the set of equipment names a `DispatchTarget` resolves to, then compare those sets for overlap.

2. **Differentiate between *conflicting* and *coexisting* signals** (Finding 2): Introduce a `ControlSignal::conflicts_with` or `ControlSignal::can_coexist_with` method that checks whether two signals target orthogonal equipment parameters (e.g., `PowerSetpoint` and `SOCTarget` can coexist, but `PowerSetpoint` and `PowerLimit` cannot). Use this in `drain_tiers` to allow lower-priority signals to pass through when they don't actually conflict with the higher-priority signal already applied.

3. **Disambiguate same-tier conflicts** (Finding 3): At minimum, add a deterministic tiebreaker within the same tier. Options include: (a) signal-type priority ordering (e.g., `DutyCycle` before `ModeOverride`), (b) first-wins within tier (reject second+), or (c) require external controllers to assign sub-priorities within tiers. Document the chosen behavior.

4. **Replace Vec ledger with HashMap** (Finding 4): Change `seen_targets: Vec<(DispatchTarget, usize)>` to `seen_targets: HashMap<DispatchTarget, usize>` mapping target → highest tier seen. This collapses duplicates, enables O(1) lookups, and implicitly ensures only the max tier per target is recorded.

5. **Consider signal stacking/failover** (Finding 6): For layered-control designs, investigate a stacking dispatcher that records the chain of overridden signals per target. When a higher-priority signal expires or is removed, the next signal in the stack is automatically promoted. This would enable DR-override-TOU-and-fallback designs without requiring TOU actors to re-issue.

## References / Citations
- HARES `dispatch.rs:52-58` — `conflicts_with` only matches same-variant pairs
- HARES `dispatch.rs:166-170` — test asserting cross-variant non-conflict is design invariant
- HARES `dwelling/mod.rs:428-462` — `drain_tiers` priority-ordered dispatch with cross-pass ledger
- HARES `dwelling/mod.rs:466-494` — `route_request` resolves both `ByName` and `ByEndUse` to equipment array
- HARES `dwelling/mod.rs:523-528` — `compute_equipment_dispatch_targets` always uses `ByName`
- HARES `equipment.rs:1115-1133` — `EquipmentDescriptor` contains both `name` and `end_use`
- OCHRE `Dwelling.py:139` — `equipment_by_end_use` dict as name↔end-use bridge
- OCHRE `Dwelling.py:236-244` — end-use-to-equipment signal broadcast with name-precedence
- OCHRE `Battery.py:169-197` — signal-level override chain (P Setpoint > SOC) within single equipment
- OCHRE `Equipment.py:134-174` — `calculate_mode_priority` formal mode priority system
- Existing review `docs/reviews/core/core-04-control-dispatch-cross-pass-ledger.md` — related findings on cross-variant `conflicts_with` (Finding 1), Vec accumulation (Finding 2), same-tier stacking (Finding 3), persistent control semantics (Finding 4)
