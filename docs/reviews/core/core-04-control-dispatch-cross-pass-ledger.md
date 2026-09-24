# Control dispatch cross-pass ledger state consistency
**Review ID**: core-04
**Category**: core
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-control/src/dispatch.rs`
- `crates/hares-control/src/types.rs`
- `crates/hares-core/src/engine.rs`
- `crates/hares-core/src/dwelling/mod.rs`
- `crates/hares-core/tests/dispatch_ordering_regressions.rs`
- `crates/hares-equipment/src/battery/mod.rs`
- `crates/hares-equipment/src/pv/mod.rs`

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Equipment/Equipment.py` — the requested `Controller.py` does not exist in the vendor tree. The closest analog is `Equipment.py`, which implements OCHRE's single-pass control model via `update_model(control_signal=None)`. The OCHRE `ControllerIntegration.rst` documentation (`vendors/OCHRE/docs/source/ControllerIntegration.rst`) was also consulted for the `Resets?` column describing ephemeral vs. persistent control signal semantics.

---

## Findings

### Finding 1: [Severity: medium]
**Description**: `conflicts_with()` cannot detect cross-variant targeting — a `DispatchTarget::ByName` and a `DispatchTarget::ByEndUse` that resolve to the *same physical equipment* are reported as non-conflicting.

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

**Root Cause**: `conflicts_with` only compares same-variant pairs. When the variants differ (`ByName` vs `ByEndUse`), it returns `false` unconditionally, despite both variants resolving through `route_request` (`dwelling/mod.rs:446-474`) which dispatches `ByName` targets by equipment name and `ByEndUse` targets by end-use category to the same equipment array.

**Impact**: Priority inversion across dispatch passes. Scenario:
1. Pass 1: a `Safety`-tier signal targets `EndUse::BATTERY` → applied to battery → `seen_targets` ledger records `(ByEndUse(BATTERY), 3)`.
2. Pass 2: a `Schedule`-tier signal from an actor targets `ByName("Battery #1")` → `conflicts_with` returns `false` (different variants) → `prior_higher` check (line 414-415 of `dwelling/mod.rs`) fails to detect the existing higher-priority entry → the lower-priority `Schedule` signal is **applied**, overwriting the `Safety` signal.

The `conflicts_with_different_variants_never_conflict` unit test at `dispatch.rs:166-170` explicitly asserts this non-detection, suggesting this is a known design assumption rather than an oversight. However, nothing prevents external controllers from targeting by end-use while internal actors target by name, creating a real pathway for undetected conflicts.

**OCHRE Comparison**: OCHRE does not have an equivalent multi-pass priority mechanism; its `update_model(control_signal)` is single-pass per step. However, OCHRE's `"Resets?"` column in the Controller Integration docs indicates that most signals are ephemeral (reset each step if not re-issued), which side-steps the cross-pass accumulation problem entirely — a stale signal from a previous step cannot leak. HARES's persistent-control model (signals retain effect until explicitly overwritten) requires a consistent conflict detector, which the current `conflicts_with` does not provide.

---

### Finding 2: [Severity: medium]
**Description**: The `seen_targets` ledger accumulates unbounded duplicate entries for the same target across multiple tiers within a single timestep.

**Code Location**: `crates/hares-core/src/dwelling/mod.rs:437`

```rust
self.seen_targets.push((request.target.clone(), tier_idx));
```

**Root Cause**: When a higher-priority signal overwrites a lower-priority one for the same target, the old (lower-tier) entry is **never removed** from the `Vec`. Both entries persist. Example evolution:
```
Pass 1: Schedule(Battery, 0)  → seen_targets = [(Battery, 0)]
Pass 1: Safety(Battery, 3)    → seen_targets = [(Battery, 0), (Battery, 3)]
Pass 2: Grid(Battery, 2)      → scans 2 entries, correctly rejected (3 > 2)
```

**Impact**: While the priority check remains **correct** (the O(n) linear scan checks all entries and the highest-tier entry wins), this is a resource leak within the step:
- For `N` equipment items and `T` tiers, `seen_targets` can grow to `N × T` entries.
- The `prior_higher` check at line 414 and `overwrote` check at line 427 both perform O(seen_targets.len()) scans per dispatch request.
- Old entries for the same target serve no purpose — only the *maximum* tier per target matters for rejecting lower-priority signals.
- The `overwrote` check at line 427 iterates the entire vector looking for any entry with `tier_idx > prev_tier`, which will always find such an entry if one exists — scanning past the first match is wasted work.

A `HashMap<DispatchTarget, usize>` tracking only the highest tier per target would be both O(1) and self-limiting in size.

---

### Finding 3: [Severity: low]
**Description**: Same-tier duplicate signals for the same target across passes are not deduplicated, causing both signals to be dispatched to the equipment.

**Code Location**: `crates/hares-core/src/dwelling/mod.rs:414-416`

```rust
let prior_higher = self.seen_targets.iter().any(|&(ref t, prev_tier)| {
    t.conflicts_with(&request.target) && tier_idx < prev_tier
});
```

**Root Cause**: The `prior_higher` gate only rejects signals when a **strictly higher** tier was already seen (`tier_idx < prev_tier`, not `tier_idx <= prev_tier`). Two same-tier signals (e.g., `Schedule` in pass 1 and `Schedule` in pass 2 for the same target) both pass through and are applied.

**Impact**: This is partially by design (documented at lines 330-332: "Because every signal fires (no deduplication), the highest-priority tier writes last and wins"). Equipment `apply_control` methods are idempotent set operations (verified: battery uses `self.power_setpoint_kw = Some(...)`, PV uses `self.power_limit_kw = Some(...)`), so same-tier stacking does not cause physical double-counting. However:
- The dispatcher performs redundant `apply_control` calls for the same target at the same tier.
- In edge cases where an equipment's `apply_control` has a side effect beyond simple assignment (e.g., logging, counter updates), same-tier double-dispatch could cause observable artifacts.
- The `seen_targets` vector, lacking deduplication for same-tier entries, grows faster than necessary.

---

### Finding 4: [Severity: low]
**Description**: No `begin_step` / per-step control-state reset exists on equipment — control signals persist indefinitely across timestep boundaries with no automatic expiry (except DR timers).

**Code Location**: Battery `update_control`: `crates/hares-equipment/src/battery/mod.rs:820-830`; PV `step`: `crates/hares-equipment/src/pv/mod.rs:461-586`

**Root Cause**: Equipment `apply_control_unchecked` methods use simple field assignment (`self.power_setpoint_kw = Some(...)`, etc.) with no step-counting or expiration. The `step` method does not clear or reset any control fields. Only the battery's `update_control` method auto-expires DR timers (`dr_duration_remaining_s` → `DRLevel::Normal`). All other control fields persist indefinitely.

**Impact**: If an external controller sets `PowerSetpoint(5.0)` on a battery and never issues another control signal, the battery will remain at 5 kW for every subsequent timestep of the simulation. This is the **opposite** of OCHRE's behavior, where the `"Resets?"` column in the Controller Integration docs specifies `True` for `P Setpoint`, meaning the signal is ephemeral and the equipment reverts to its internal control algorithm on the next step if no new signal arrives.

**Design Assessment**: This is an intentional architectural choice — HARES uses persistent setpoints (similar to a PID controller's output register) while OCHRE uses ephemeral commands (similar to a PLC scan cycle). The design is internally consistent: the dispatcher's cross-pass priority ledger protects against priority inversion, and the model assumes controllers will issue new signals when they want to change behavior. However, this implicit "sticky signal" behavior should be clearly documented, as it differs from OCHRE and could surprise integrators who expect signals to auto-reset.

---

### Finding 5: [Severity: low]
**Description**: The `ControlDispatcher` struct resides in a local scope within `dwelling/mod.rs` (line 339: `struct ControlDispatcher`) — it is not unit-testable in isolation, and no integration tests exercise the ledger accumulation path where a target receives signals at >2 different tiers within a single step.

**Code Location**: `crates/hares-core/src/dwelling/mod.rs:339-444`

**Root Cause**: The struct is `struct` (not `pub(crate)`), with private fields, defined inside a non-test module. The existing regression tests in `dispatch_ordering_regressions.rs` test the two-pass scenario (pre-thermal + post-actor) but only with two tiers per target per test (`Safety` vs `Schedule`). No test exercises the case where three or more tiers target the same equipment in a single step.

**Impact**: The vector accumulation behavior (Finding 2) and same-tier deduplication gap (Finding 3) are unlikely to regress in normal CI runs. A test that queues `Schedule → Safety → Grid` signals for the same target in a single step, then verifies that only `Safety` takes effect, would provide coverage for the multi-tier accumulation path.

---

## Summary
- **Total findings**: 5
- **Medium**: 2 (cross-variant conflict detection gap, seen_targets unbounded accumulation)
- **Low**: 3 (same-tier stacking, persistent vs. ephemeral control paradigm, insufficient test coverage)
- **Critical / High**: 0

## Recommendations

1. **Fix cross-variant conflict detection** (Finding 1): Extend `conflicts_with` to return `true` when both variants would route to overlapping equipment sets. This requires either: (a) maintaining a bidirectional name-to-enduse and enduse-to-names mapping in the dispatcher, or (b) restricting all external dispatch to a single target variant. Option (b) is simpler and already the de facto pattern — `apply_control_validated` always uses `ByName`.

2. **Replace `Vec` ledger with `HashMap`** (Finding 2, 3): Change `seen_targets: Vec<(DispatchTarget, usize)>` to `seen_targets: HashMap<DispatchTarget, usize>` mapping target → highest tier seen. This eliminates duplicate entries, makes `prior_higher` checks O(1) amortized, and implicitly deduplicates same-tier entries (only the first same-tier entry is recorded; subsequent same-tier signals for the same target would still be applied since `tier_idx < prev_tier` is still false — which is the desired behavior).

3. **Document persistent vs. ephemeral control semantics** (Finding 4): Add a section to the control dispatcher docstring or the `Equipment` trait docs clarifying that control signals are persistent (sticky) across timesteps until explicitly overwritten. Contrast with OCHRE's ephemeral "resets each step" model.

4. **Add multi-tier integration test** (Finding 5): Extend `dispatch_ordering_regressions.rs` with a test that queues signals at 3+ distinct tiers for the same target within a single step and verifies the highest-tier signal wins.

## References / Citations
- OCHRE Controller Integration docs — signal `Resets?` column (`vendors/OCHRE/docs/source/ControllerIntegration.rst`): Battery `P Setpoint` `Resets?` = `True`, HVAC `Setpoint` `Resets?` = `True`, etc.
- OCHRE `Equipment.py:220-258` (`update_model`): When `control_signal` is `None`, calls `update_internal_control()` (reverts to default); when provided, calls `update_external_control(control_signal)`.
- HARES `dwelling/mod.rs:326-347` (`ControlDispatcher` docstring): documents the priority-tiered, idempotent-set dispatch model.
- HARES `dwelling/mod.rs:403-406` (inline comment): explicitly notes `seen_targets` is NOT cleared between passes within a step.
- HARES `dwelling/mod.rs:2289-2292` (`begin_step` call): ledger clears exactly once per timestep, before the first dispatch pass.
- HARES `dispatch.rs:166-170` (`conflicts_with_different_variants_never_conflict` test): encodes the design assumption that named and end-use targeting never overlap.
