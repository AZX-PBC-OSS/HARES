# ThermalAccumulator: No Per-Category Latent Breakdown

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-types, hares-envelope
**Note**: Land ticket 071 (adds `HvacDehumidification`) before this ticket so the new category is immediately covered in `latent_by_category`.

## Problem

`ThermalAccumulator` tracks per-category sensible and radiant heat in fixed-size arrays (`sensible_by_category`, `radiant_by_category`) but `latent_gain_w` is a single scalar with no per-category breakdown. This makes it impossible to separate HVAC latent extraction (negative, from AC or dehumidifier) from internal-gain latent additions (positive, from occupants or humidifiers) in zone energy-balance diagnostics.

Without a per-category latent breakdown:

1. The sensible/latent closure check — `Q_sens + Q_lat must equal total zone enthalpy change` — cannot be attributed by source. Energy balance violations (ticket 048) cannot be isolated to specific equipment.
2. It is impossible to verify that cooling-coil latent extraction and dehumidifier latent extraction sum correctly against the humidity solver's computed moisture removal.
3. Diagnostic output cannot separately report "HVAC latent removal" from "occupant latent addition", preventing EnergyPlus-parity reporting.

EnergyPlus Engineering Reference §3.2 "Zone Air Moisture Balance": the moisture balance equation distinguishes `ΣQlatent_HVAC` from `ΣQlatent_internal` as separate source terms. ASHRAE Handbook of Fundamentals 2021 Ch. 1 Eq. 41: latent load = `Σ(m_dot_i × h_fg × Δω_i)` per source term.

## Current Behavior

`hares-types/src/ports.rs:164–172`:

```rust
pub struct ThermalAccumulator {
    pub zone: ZoneId,
    pub sensible_gain_w: f64,
    pub radiant_gain_w: f64,
    pub latent_gain_w: f64,                              // scalar only
    pub sensible_by_category: [f64; THERMAL_CATEGORY_COUNT],
    pub radiant_by_category: [f64; THERMAL_CATEGORY_COUNT],
    // latent_by_category does not exist
}
```

`ports.rs:186–198` `add()` updates aggregate totals and per-category arrays for sensible and radiant, but only the aggregate total for latent. No `latent_by_category` update.

The humidity solver (`humidity_solver.rs:116–122`) consumes `latent_gain_w` as a single sum across all thermal accumulators for a zone. It cannot distinguish HVAC extraction from occupant addition when the net total is ambiguous.

## Required Behavior

Add `pub latent_by_category: [f64; THERMAL_CATEGORY_COUNT]` to `ThermalAccumulator`, updated in `add()` consistently with `sensible_by_category`. Add a `latent_for_category()` accessor parallel to `sensible_for_category()`.

Invariant: `sum(latent_by_category) == latent_gain_w` must hold after every `add()` call and after `zero()`. This invariant must be verified by a test.

`zero()` must reset the new array to `[0.0; THERMAL_CATEGORY_COUNT]`.

Reference: EnergyPlus Engineering Reference §3.2 "Zone Air Moisture Balance"; OCHRE results dictionary — per-end-use latent gain reporting (e.g., `"HVAC Cooling Latent Gains (W)"` separate from `"Internal Gains Latent Gains (W)"`).

## Approach

1. In `ports.rs`, add `pub latent_by_category: [f64; THERMAL_CATEGORY_COUNT]` to `ThermalAccumulator`.
2. Initialize to `[0.0; THERMAL_CATEGORY_COUNT]` in `ThermalAccumulator::new()` (or the equivalent constructor site).
3. In `add()`, append `self.latent_by_category[category.index()] += latent_gain_w;`.
4. In `zero()`, add `self.latent_by_category = [0.0; THERMAL_CATEGORY_COUNT];`.
5. Add `pub fn latent_for_category(&self, cat: ThermalCategory) -> f64 { self.latent_by_category[cat.index()] }`.
6. Update the struct doc comment to include the latent invariant.

## Definition of Done

- [ ] `ThermalAccumulator` has `pub latent_by_category: [f64; THERMAL_CATEGORY_COUNT]`
- [ ] `add()` updates `latent_by_category[category.index()]`
- [ ] `zero()` resets `latent_by_category` to all zeros
- [ ] `latent_for_category()` accessor exists and returns the correct bucket
- [ ] Test: `sum(latent_by_category) == latent_gain_w` after a sequence of mixed-category `add()` calls (numeric equality, not approximate)
- [ ] Test: `zero()` resets `latent_by_category` — every element is 0.0
- [ ] No existing tests broken

## Verification

```bash
cargo test -p hares-types
cargo test -p hares-envelope
```

## References

- EnergyPlus Engineering Reference §3.2 "Zone Air Moisture Balance" — per-source latent term attribution
- ASHRAE Handbook of Fundamentals 2021 Ch. 1 Eq. 41 — latent load = Σ(m_dot_i × h_fg × Δω_i) per source term
- OCHRE results dictionary — per-end-use latent gain reporting
- `hares-types/src/ports.rs:164–205` — `ThermalAccumulator` struct and `add()` / `zero()`
