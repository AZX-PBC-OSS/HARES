# Unify Variable-Speed / Multi-Speed Selection Logic

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-equipment/hvac/air_conditioner, hares-equipment/hvac/staging

## Problem

`CoolingCore::select_variable_speed_cooling` in `air_conditioner.rs` reimplements the same capacity-fraction bracket interpolation as `HvacEquipment::select_multi_speed` in `staging.rs`. The `partition_point` lookup, span calculation, and `SpeedSelection` construction are identical. A bug in the bracket logic (e.g., off-by-one in the `partition_point` boundary condition) would need to be fixed in two places.

## Current Behavior

### `CoolingCore::select_variable_speed_cooling` (air_conditioner.rs:335–397)

```rust
fn select_variable_speed_cooling(
    &mut self,
    requested_capacity_fraction: f64,
) -> SpeedSelection {
    let requested_capacity_fraction = requested_capacity_fraction.clamp(0.0, 1.0);
    let capacities = &self.hvac.cooling_capacities_w;
    // ... empty/single-stage early returns ...
    let max_capacity_w = capacities.last().copied().unwrap_or_default();
    let capacity_fractions: Vec<f64> = capacities
        .iter()
        .map(|capacity_w| capacity_w / max_capacity_w.max(f64::MIN_POSITIVE))
        .collect();
    if requested_capacity_fraction <= capacity_fractions[0] {
        SpeedSelection { speed_index: 0, speed_frac: 0.0,
            part_load_ratio: (requested_capacity_fraction / capacity_fractions[0].max(f64::MIN_POSITIVE)).clamp(0.0, 1.0) }
    } else if requested_capacity_fraction >= *capacity_fractions.last().expect(...) {
        SpeedSelection { speed_index: capacity_fractions.len() - 1, speed_frac: 0.0, part_load_ratio: 1.0 }
    } else {
        let hi = capacity_fractions.partition_point(|&fraction| fraction < requested_capacity_fraction);
        let lo = hi - 1;
        let span = capacity_fractions[hi] - capacity_fractions[lo];
        let speed_frac = if span > f64::EPSILON { (requested_capacity_fraction - capacity_fractions[lo]) / span } else { 0.0 };
        SpeedSelection { speed_index: lo, speed_frac, part_load_ratio: 1.0 }
    }
}
```

Key detail: This method computes `capacity_fractions` inline from `cooling_capacities_w` by normalizing against the maximum capacity. It also has `clamp(0.0, 1.0)` on input and `f64::MIN_POSITIVE` denominators.

### `HvacEquipment::select_multi_speed` (staging.rs:202–249)

```rust
fn select_multi_speed(&self, load_fraction: f64) -> SpeedSelection {
    let caps = /* mode-dependent capacity selection */;
    let cap_fracs = Self::capacity_fractions_for(caps);
    // ... empty/zero early return ...
    if load_fraction <= cap_fracs[0] {
        SpeedSelection { speed_index: 0, speed_frac: 0.0, part_load_ratio: load_fraction / cap_fracs[0] }
    } else if load_fraction >= *cap_fracs.last().expect(...) {
        SpeedSelection { speed_index: cap_fracs.len() - 1, speed_frac: 0.0, part_load_ratio: 1.0 }
    } else {
        let hi = cap_fracs.partition_point(|&f| f < load_fraction);
        let lo = hi - 1;
        let span = cap_fracs[hi] - cap_fracs[lo];
        let frac = if span > f64::EPSILON { (load_fraction - cap_fracs[lo]) / span } else { 0.0 };
        SpeedSelection { speed_index: lo, speed_frac: frac, part_load_ratio: 1.0 }
    }
}
```

Key detail: Uses `Self::capacity_fractions_for(caps)` (staging.rs:~140) to normalize, which is the same max-normalization logic. The `partition_point` / span / `SpeedSelection` construction is identical.

### Substantive differences

1. **Input clamping**: `select_variable_speed_cooling` clamps input to `[0, 1]` (air_conditioner.rs:339); `select_multi_speed` does not (staging.rs:214 checks `<= 0.0` for the empty return but does not clamp above 1.0).
2. **Capacity source**: `select_variable_speed_cooling` always uses `cooling_capacities_w`; `select_multi_speed` chooses heating or cooling based on current mode (staging.rs:203–212).
3. **Empty/single-stage handling**: `select_variable_speed_cooling` has explicit single-capacity early return (air_conditioner.rs:347–352); `select_multi_speed` handles it implicitly through `capacity_fractions_for`.
4. **`f64::MIN_POSITIVE` guards**: `select_variable_speed_cooling` uses `.max(f64::MIN_POSITIVE)` in the PLR denominator (air_conditioner.rs:363–365); `select_multi_speed` does not.
5. **Side effects**: `select_variable_speed_cooling` writes back `self.hvac.last_speed_index` and `self.hvac.last_speed_frac` (air_conditioner.rs:394–395); `select_multi_speed` does not.

## Required Behavior

A single shared function performs the bracket-interpolation algorithm. Both callers delegate to it. Pure refactor with no behavioral change.

## Approach

### Step 1: Add `interpolate_speed_stages` to `staging.rs` (or `speed_control.rs`)

```rust
/// Bracket-interpolation of a requested load fraction against normalized
/// capacity fractions, returning a `SpeedSelection`.
///
/// `capacity_fractions` must be sorted ascending. Returns `None` when
/// `capacity_fractions` is empty.
pub fn interpolate_speed_stages(
    load_fraction: f64,
    capacity_fractions: &[f64],
    clamp_input: bool,
) -> Option<SpeedSelection> {
    let lf = if clamp_input { load_fraction.clamp(0.0, 1.0) } else { load_fraction };
    if capacity_fractions.is_empty() || lf <= 0.0 {
        return Some(SpeedSelection { speed_index: 0, speed_frac: 0.0, part_load_ratio: 0.0 });
    }
    if lf <= capacity_fractions[0] {
        let plr = (lf / capacity_fractions[0].max(f64::MIN_POSITIVE)).clamp(0.0, 1.0);
        return Some(SpeedSelection { speed_index: 0, speed_frac: 0.0, part_load_ratio: plr });
    }
    if lf >= *capacity_fractions.last().expect("non-empty") {
        return Some(SpeedSelection {
            speed_index: capacity_fractions.len() - 1,
            speed_frac: 0.0,
            part_load_ratio: 1.0,
        });
    }
    let hi = capacity_fractions.partition_point(|&f| f < lf);
    let lo = hi - 1;
    let span = capacity_fractions[hi] - capacity_fractions[lo];
    let speed_frac = if span > f64::EPSILON { (lf - capacity_fractions[lo]) / span } else { 0.0 };
    Some(SpeedSelection { speed_index: lo, speed_frac, part_load_ratio: 1.0 })
}
```

Place this in `speed_control.rs` since `SpeedSelection` is defined there and it's already `pub(super)` accessible to both `staging.rs` and `air_conditioner.rs`.

### Step 2: Refactor `HvacEquipment::select_multi_speed`

Replace the inline bracket logic (staging.rs:214–249) with a call to `interpolate_speed_stages(load_fraction, &cap_fracs, false)`. Keep the mode-dependent capacity selection (staging.rs:203–212) and the `capacity_fractions_for` call. Unwrap the `Option` with the existing empty-list fallback.

### Step 3: Refactor `CoolingCore::select_variable_speed_cooling`

Replace the inline bracket logic (air_conditioner.rs:335–397) with:

```rust
fn select_variable_speed_cooling(&mut self, requested_capacity_fraction: f64) -> SpeedSelection {
    let capacities = &self.hvac.cooling_capacities_w;
    let cap_fracs = HvacEquipment::capacity_fractions_for(capacities);
    let selection = interpolate_speed_stages(requested_capacity_fraction, &cap_fracs, true)
        .unwrap_or(SpeedSelection { speed_index: 0, speed_frac: 0.0, part_load_ratio: 0.0 });
    self.hvac.last_speed_index = selection.speed_index;
    self.hvac.last_speed_frac = selection.speed_frac;
    selection
}
```

### Step 4: Make `capacity_fractions_for` accessible

Currently `capacity_fractions_for` is on `HvacEquipment` (staging.rs). If `CoolingCore` needs to call it, either:
- Make it a `pub(super)` method on `HvacEquipment` (it likely already is given the module visibility), or
- Extract it as a free function next to `interpolate_speed_stages`.

### Step 5: Run full test suite

```
cargo test -p hares-equipment
```

## Definition of Done

- [ ] `interpolate_speed_stages` function exists in `speed_control.rs` (or `staging.rs`) with the shared bracket-interpolation logic
- [ ] `select_multi_speed` delegates to `interpolate_speed_stages` instead of inlining the logic
- [ ] `select_variable_speed_cooling` delegates to `interpolate_speed_stages` instead of inlining the logic
- [ ] `capacity_fractions_for` is accessible from both call sites
- [ ] The `clamp_input` / `f64::MIN_POSITIVE` behavioral differences are preserved exactly as they exist today (documented in the function doc)
- [ ] Side effects (writing `last_speed_index`/`last_speed_frac`) remain in the caller, not in the shared function
- [ ] `cargo test -p hares-equipment` passes with no behavioral changes
- [ ] `cargo clippy -p hares-equipment` produces no new warnings

## Verification

1. `cargo test -p hares-equipment` — all existing tests pass unchanged
2. Grep for `partition_point` — should appear only in `interpolate_speed_stages`, not in `select_multi_speed` or `select_variable_speed_cooling`
3. Confirm the `clamp_input` flag preserves the current behavioral difference:
   - `select_variable_speed_cooling` passes `clamp_input: true`
   - `select_multi_speed` passes `clamp_input: false`
4. Confirm `last_speed_index`/`last_speed_frac` are still written in `select_variable_speed_cooling` only

## References

- `air_conditioner.rs:335–397` — `CoolingCore::select_variable_speed_cooling`
- `staging.rs:202–249` — `HvacEquipment::select_multi_speed`
- `staging.rs:~140` — `HvacEquipment::capacity_fractions_for` (normalization helper)
- `speed_control.rs` — `SpeedSelection` definition

## Related Tickets

- 008-decompose-hvacequipment-struct — if `capacity_fractions_for` moves to a shared location, this reduces the method count on `HvacEquipment`

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-20

### Code Confirmation

- [x] Referenced line numbers still match — `select_variable_speed_cooling` at `air_conditioner.rs:335–397`; `select_multi_speed` at `staging.rs:202–249`; `capacity_fractions_for` at `staging.rs:372–378`; `SpeedSelection` at `speed_control.rs:100–110`. All confirmed by direct file read.
- [x] Described logic matches current implementation — both functions implement the same three-branch algorithm: (1) early-return zero for empty/zero-load, (2) PLR-only at or below first stage, (3) full-capacity clamp at or above last stage, (4) `partition_point` binary search → linear interpolation between brackets. Diff verified by inspection.
- [x] OCHRE cross-check: **matches** — OCHRE `HVAC.py:1005–1018` uses `np.searchsorted(capacities, capacity)` and then `frac_high = (capacity - capacities[speed_low]) / (capacities[speed_high] - capacities[speed_low])`, which is algorithmically identical to HARES's `partition_point` + `(lf - cap_fracs[lo]) / span`. OCHRE's below-minimum branch is `speed_idx = capacity / capacities[1]` (a fractional scalar, not PLR), whereas HARES keeps `speed_index=0` and stores the fraction as `part_load_ratio` — this structural difference is intentional and already documented in `SpeedSelection`'s doc-comments.
- [x] EnergyPlus cross-check: **matches** — EnergyPlus Engineering Reference (v8.3 Air System Compound Component Groups) defines SpeedRatio for "Higher Speed Operation" as:
  > `SpeedRatio = ABS(UnitarySystemCoolingLoad − AddedFanHeat − FullCoolOutputSpeed_{n−1}) / ABS(FullCoolOutputSpeed_n − FullCoolOutputSpeed_{n−1})`
  This is the same linear interpolation `(Q_required − Q_low) / (Q_high − Q_low)` that both HARES functions implement. No divergence.

### Web-Verified Citations

The ticket contains **no explicit standards citations** (no ASHRAE, NFRC, DOE, ISO, or EnergyPlus section numbers are cited). The ticket is a pure code-structure/refactoring ticket. The underlying algorithm is validated against EnergyPlus and OCHRE above.

**EnergyPlus speed-ratio formula (cross-reference, not a ticket citation):**
- **Source found**: EnergyPlus 8.3 Engineering Reference — Air System Compound Component Groups (`bigladdersoftware.com/epx/docs/8-3/engineering-reference/air-system-compound-component-groups.html`)
- **Quoted passage**: *"SpeedRatio = ABS(UnitarySystemCoolingLoad − AddedFanHeat − FullCoolOutputSpeed_{n−1}) / ABS(FullCoolOutputSpeed_n − FullCoolOutputSpeed_{n−1})"* — linear capacity interpolation between adjacent speed stages.
- **Verdict**: confirmed — HARES implements this correctly in both duplicated locations.

### Legitimacy

- **Verdict**: Legitimate
- **Rationale**: Direct inspection of `air_conditioner.rs:335–397` and `staging.rs:202–249` confirms that the bracket-interpolation algorithm (`partition_point` → `lo/hi` → span → `speed_frac`) appears verbatim in both functions. The five substantive differences catalogued in the ticket (input clamping, capacity source, empty/single-stage handling, `f64::MIN_POSITIVE` guards, side-effect writes) are real and correctly described. OCHRE uses the same mathematical algorithm (`np.searchsorted` + fractional interpolation). EnergyPlus documents the same linear SpeedRatio formula. The duplication risk is genuine: a future bug fix in one copy will silently leave the other copy broken. No part of the ticket's description is inaccurate.

### Proposed Fix Summary

Add a free function `interpolate_speed_stages(load_fraction: f64, capacity_fractions: &[f64], clamp_input: bool) -> Option<SpeedSelection>` to `speed_control.rs`. Both `select_multi_speed` (passing `clamp_input: false`) and `select_variable_speed_cooling` (passing `clamp_input: true`) replace their inline bracket logic with a call to this shared function. The `capacity_fractions_for` normalization helper and the side-effect writes (`last_speed_index`/`last_speed_frac`) remain in their existing call-sites. No behavioral change.

### Test Written

- **File**: `crates/hares-equipment/src/hvac/air_conditioner.rs` — new `mod speed_selection_parity_tests` at end of file (3 tests added, all passing)
- **What it tests**:
  1. `variable_speed_and_multi_speed_agree_on_bracket_interpolation` — probes three interior brackets (load 0.5, 0.7, 0.9 against 4-stage fractions [0.4, 0.6, 0.8, 1.0]); asserts `speed_index`, `speed_frac`, and `part_load_ratio` are identical between `select_variable_speed_cooling` and `select_multi_speed`.
  2. `variable_speed_and_multi_speed_agree_below_lowest_stage` — load=0.3 < cap_frac[0]=0.4; asserts PLR=0.75 from both paths.
  3. `variable_speed_and_multi_speed_agree_at_full_load` — load=1.0; asserts both return `speed_index=3`, `speed_frac=0.0`, `part_load_ratio=1.0`.
- If the two implementations ever diverge (e.g. a one-sided bug fix), these tests will fail and expose the divergence immediately.
