# Tiered energy cost: block edge cases, zero-consumption, no below-block fees, single-block tariffs
**Review ID**: tariffdp-02
**Category**: tariff-deep
**Date**: 2026-05-26

## Files Reviewed
crates/hares-tariff/src/billing.rs

## Vendor/Reference Files Consulted
None

## Findings
### Finding 1: [Severity: low]
**Description**: No guard against negative `import_kwh` input. The `compute_tiered_energy_cost` function (`billing.rs:282`) receives `import_kwh: f64` without any non-negative assertion. If a negative value were ever passed (e.g., from a net-metering path that incorrectly routes net export through the tiered function), the computation would treat the negative consumption as tier-1 credit at the tier-1 rate rather than using the tariff's export credit rate, producing an incorrect (and potentially over-credited) bill.

**Code Location**: `billing.rs:282-312` (function signature accepts any `f64`; `remaining = import_kwh` at line 295)

**Root Cause**: The function signature is unguarded. `remaining` is set directly to `import_kwh` with no `max(0.0)` clamp. In the loop, `remaining.min(band_width)` selects the negative value (since negative < positive), and `remaining <= 0.0` stops the loop on the first iteration, returning a negative cost (credit) at `block.rates_per_kwh[0]` which is the import tier rate, not the export rate.

**Impact**: Low. Current call sites (`evaluator.rs:331, 440`) always pass `cumulative_import_kwh`, which is accumulated from `net_power_kw.max(0.0)` and is therefore always >= 0. The defect is a latent defense-in-depth gap, not an active bug.

**Recommendation**: Add `debug_assert!(import_kwh >= 0.0, "tiered energy cost expects non-negative import kWh, got {import_kwh}")` at the top of the function, or clamp `import_kwh` with `.max(0.0)` before use.

---

### Finding 2: [Severity: low]
**Description**: `TieredBlock` supports only a single consolidated `rates_per_kwh` per tier (one rate per tier block). Real residential tariffs commonly have separate energy supply and delivery/distribution charges within each tier (e.g., tier-1 energy $0.08/kWh + tier-1 delivery $0.04/kWh = $0.12/kWh effective). The current data model cannot separate these components for reporting, which may reduce auditability.

**Code Location**: `types.rs:118-124` (TieredBlock struct definition), `billing.rs:301` (single-rate multiplication per band)

**Root Cause**: The tiered block model stores `rates_per_kwh: Vec<f64>` — one scalar per tier — rather than a struct with separate energy and delivery components.

**Impact**: Low. A tariff with separate energy and delivery tier charges can be represented by pre-combining them into the single `rates_per_kwh` for correct billing totals, but per-component breakdown on bills is lost. This is a design-expressiveness limitation, not a correctness defect.

---

### Finding 3: [Severity: informational]
**Description**: Time-of-use (TOU) × tier interaction uses an aggregate-monthly-threshold model. The tiered calculation receives total billing-period import kWh (`evaluator.rs:331, 440`) and applies block thresholds to the aggregate. Some real tariffs apply tier thresholds within each TOU period independently (e.g., 500 kWh at off-peak tier-1 rate, separately from 500 kWh at on-peak tier-1 rate). The HARES model cannot represent per-TOU-period tier thresholds.

**Code Location**: `evaluator.rs:331-336` (single call with `cumulative_import_kwh`), `billing.rs:282-312` (no TOU period parameter in function signature)

**Root Cause**: The tiered rate model operates on total import kWh. No per-period kWh accumulation exists in `BillingState` — only per-period demand tracking (`period_peak_demand_kw`). The `update` method (`billing.rs:162`) accumulates energy globally regardless of period index.

**Impact**: Informational. For tariffs that specify per-TOU-period tiers, the model would mischarge by applying a single aggregate threshold instead of separate per-period thresholds. Whether this matters depends on the tariffs actually being modeled. No bug for tariffs with single aggregate monthly thresholds — which is the majority convention in the US.

---

## Summary
- Total findings: 3
- Critical / High / Medium / Low: 0 / 0 / 0 / 2 (plus 1 informational)

## Detailed Analysis of Review-Specified Cases
### (a) Block boundaries — PASS
Consumption exactly at a tier boundary (e.g., 500 kWh with thresholds `[500.0]`) falls entirely in the lower tier. The loop computes `band_width = threshold - prev_threshold = 500.0`, then `usage_in_band = remaining.min(band_width) = 500.0.min(500.0) = 500.0`, billing all 500 kWh at the tier-1 rate. The `remaining <= 0.0` early return prevents the 501st+ kWh from spilling into tier 2. No off-by-one. Verified by test `compute_tiered_energy_cost_blended_across_boundary` at `billing.rs:708-722`.

### (b) Zero consumption — PASS
`import_kwh = 0.0` produces `cost = 0.0`. The first iteration computes `usage_in_band = 0.0.min(band_width) = 0.0`, `remaining` stays `0.0`, and `remaining <= 0.0` triggers immediate return with `cost = 0.0`. Only possible non-zero path is the season-fallback branch (`fallback_flat_cost`), which at the call sites mirrors `cumulative_energy_cost_usd` — also zero for zero import.

### (c) No below-block fees — PASS (by model design)
There is no separate "baseline" or "below-block" allowance concept. The tier system naturally supports a zero-rate first band: if `rates_per_kwh[0] = 0.0`, then `usage_in_band * 0.0 = 0.0`, effectively making the first block a free allowance. Threshold position defines the boundary. This is model-consistent.

### (d) Single-block tariff — PASS
A flat tariff with empty `thresholds_kwh: vec![]` and `rates_per_kwh: vec![0.15]` skips the loop entirely. The post-loop code accesses `block.rates_per_kwh[block.thresholds_kwh.len()]` = `rates_per_kwh[0]` = `0.15`. `cost = 0.0 + remaining * 0.15`, producing the correct flat-rate result. Validation (`validate_tiered`, `types.rs:72-108`) enforces `rates.len() == thresholds.len() + 1`, guaranteeing the index is valid.

### (e) Multi-block arithmetic — PASS
Test `compute_tiered_energy_cost_three_tiers` (`billing.rs:725-740`) confirms correct multi-tier arithmetic: 1000 kWh with thresholds `[300, 800]` and rates `[0.08, 0.12, 0.25]` yields `300×0.08 + 500×0.12 + 200×0.25 = $134.00`. The review's example (800 kWh, thresholds `[500, 1000]`, rates `[0.10, 0.15, 0.20]`) would compute `500×0.10 + 300×0.15 = $95.00` by the same algorithm.

### (f) Rate components — LIMITATION (see Finding 2)
Only a single scalar rate per tier. No separate energy vs. delivery charge decomposition. The combined rate must be pre-calculated into `rates_per_kwh`.

### (g) Time-of-use × tier interaction — LIMITATION (see Finding 3)
Aggregate-monthly-threshold model. TOU period information is not an input to `compute_tiered_energy_cost`.

### (h) Negative consumption — PASS (at call sites), LATENT RISK (see Finding 1)
Current call sites always pass non-negative `cumulative_import_kwh`. The function itself lacks a guard.

## Recommendations
1. Add a `debug_assert!(import_kwh >= 0.0, ...)` or `.max(0.0)` clamp in `compute_tiered_energy_cost` for defense-in-depth (Finding 1).
2. Consider extending `TieredBlock` with optional separate energy/delivery rate fields for bill-audit decomposition (Finding 2).
3. Document that the tiered model applies aggregate monthly thresholds and does not support per-TOU-period independent tiers (Finding 3).

## References / Citations
- `compute_tiered_energy_cost`: `crates/hares-tariff/src/billing.rs:282-312`
- `TieredBlock` definition: `crates/hares-tariff/src/types.rs:118-124`
- Call sites: `crates/hares-tariff/src/evaluator.rs:331-336`, `evaluator.rs:440-445`
- `validate_tiered`: `crates/hares-tariff/src/types.rs:72-108`
- Test coverage: `crates/hares-tariff/src/billing.rs:708-764`, `evaluator.rs:1545-1597`
