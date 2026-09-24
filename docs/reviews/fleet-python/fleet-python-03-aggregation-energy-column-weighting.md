# Fleet aggregation: energy column weighting with non-unit sample weights
**Review ID**: fleet-python-03
**Category**: fleet-python
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-fleet/src/aggregation.rs`
- `crates/hares-fleet/src/fleet.rs` (weight plumbing only)

## Vendor/Reference Files Consulted
None (no vendor reference provided).

## Findings

### Finding 1: [Severity: information]
**Description**: The review concern that `ColumnAggregation::Sum` for energy (kWh) columns combined with `FleetAggregation::WeightedSum` creates a "double-weighting" issue is unfounded. The system has three distinct aggregation layers with different responsibilities, and each layer's treatment of energy columns is mathematically correct.

**Code Location**: `crates/hares-fleet/src/aggregation.rs:38-87`

**Root Cause**: The two enums share the word "Aggregation" in their names but serve entirely different purposes:

- **`ColumnAggregation`** (private, lines 38-61): Phase 2 — *per-dwelling* intra-bucket resampling. Combines sub-timestep values for a single dwelling within a time bucket (e.g., four 15-min kWh values summed to one hourly total). The only variants are `Sum` and `Mean`; there is no `WeightedSum` because sample weights are meaningless within a single dwelling. `Sum` is correct for energy because energy is additive across time within one dwelling.

- **`FleetAggregation`** (lines 69-87): Phase 3 — *cross-dwelling* fleet aggregation. Applies `sample_weight` (population scaling factor) to each dwelling's per-bucket value. `WeightedSum` is correct for energy and power columns regardless of whether weights are all 1.0 (degenerate case: `sum(v_i * 1.0) == sum(v_i)`) or vary (`sum(v_i * w_i)` correctly scales each dwelling by its population representation).

**Impact**: None. The existing implementation is correct. The full pipeline for a kWh column is:

1. **Phase 1** (`hares_io::schedule::ColumnAggregation`, defaults to `Mean`): Temporal downsampling within a dwelling (e.g., 1-min → 15-min). No suffix classification. No weight involved.
2. **Phase 2** (`aggregation.rs`, private `ColumnAggregation::Sum` for kWh): Combines sub-timestep values within a bucket per dwelling. No weight involved. Correct: `kWh_bucket = sum(kWh_sub_timesteps)`.
3. **Phase 3** (`aggregation.rs`, `FleetAggregation::WeightedSum`): Cross-dwelling weighted sum. Weight applied: `fleet_total = sum(kWh_bucket_i * sample_weight_i)`. Correct: weight scales each dwelling's contribution.

The ordering (sum per dwelling first, then weigh) is mathematically equivalent to weighing each sub-timestep value then summing: `sum(sum(v_i_t * w_i) for t in bucket) = w_i * sum(v_i_t for t in bucket)`.

Tests confirm correctness: `hourly_resample_applies_suffix_aggregation_rules` (line 573) tests kWh column aggregation with weights 1.0 and 2.0, producing the expected weighted total. `fleet_weighted_mean_vs_sum_with_unequal_weights` (line 677) tests with weights 1, 2, 3, again producing correct weighted results.

**Sample weight flow verified**: The fleet runner (`fleet.rs:488-511`) does NOT pre-apply sample weights. It passes `entry.sample_weight` (from ResStock metadata at line 152, or from `with_sample_weights()` at lines 184-193, or defaulting to 1.0 at line 109) into `DwellingOutcome.sample_weight` at line 502. The `aggregate()` function (aggregation.rs:131) collects per-dwelling metrics (line 141) and passes `sample_weight` to `build_aggregate_batch()` (line 175), which applies it in the weighted aggregation loop (lines 349-359). This design is correct: aggregation, not the runner, is responsible for weight application.

---

### Finding 2: [Severity: high]
**Description**: Sample weights are applied in `build_aggregate_batch()` without any runtime validation. Zero, NaN, or negative sample weights silently corrupt fleet-level totals with no diagnostic.

**Code Location**: `crates/hares-fleet/src/aggregation.rs:358`
```rust
weighted_values[idx] += *v * *sample_weight;
```

**Root Cause**: No guard or validation exists anywhere in the sample weight pipeline:
- `resstock.rs:274` parses `sample_weight` from Parquet metadata with no range check (non-negative, finite).
- `py_fleet.rs:99-106` validates length equality for user-provided weights but not their values.
- `fleet.rs:104-110` (`from_buildings()`) defaults to 1.0 — safe.
- `aggregation.rs:358` multiplies without checking.

**Impact**: Each invalid weight scenario:

| Invalid weight | Effect on fleet total | Detection |
|---|---|---|
| `0.0` | Building contributes nothing regardless of energy use | Silent |
| `NaN` | Entire fleet total becomes NaN (NaN * value = NaN; NaN + value = NaN) | Silent — all output corrupted |
| `-1.0` | Building contributes negative energy, lowering fleet total | Silent |
| `f64::INFINITY` | Fleet total becomes +∞ | Silent |
| `f64::NEG_INFINITY` | Fleet total becomes -∞ | Silent |
| `< 0.0` | Building acts as energy sink, physically nonsensical | Silent |

For ResStock deployments, sample weights are sourced from the NREL ResStock dataset and are typically valid positive floats. However, data corruption (truncated Parquet files, schema evolution between ResStock versions, manual metadata editing) could introduce invalid values that would silently corrupt all downstream fleet-level statistics, plots, and reports. For Python deployments with user-provided weights via `with_sample_weights()`, there is zero validation.

---

### Finding 3: [Severity: medium]
**Description**: `FleetAggregation::for_column()` (lines 74-87) maps all columns not ending with `(C)`, `(°C)`, or `(-)` to `WeightedSum` as the catch-all default. Energy columns ending in `(kWh)` correctly receive `WeightedSum`, but this is incidental — they match the default path, not an explicit rule. Columns with unanticipated unit suffixes that should use `WeightedMean` (e.g., dimensionless ratios like `Efficiency (%)` or `Power Factor`) are silently aggregated as `WeightedSum`, producing erroneous fleet-level statistics.

**Code Location**: `crates/hares-fleet/src/aggregation.rs:74-87`
```rust
fn for_column(name: &str) -> Self {
    let normalized = name.trim();

    if normalized.ends_with("(C)")
        || normalized.ends_with("(°C)")
        || normalized.ends_with("(-)")
    {
        return Self::WeightedMean;
    }

    Self::WeightedSum  // catch-all: kW, kWh, and anything unrecognized
}
```

**Root Cause**: The catch-all default to `WeightedSum` is the opposite of the safer approach (defaulting to `WeightedMean`). By silently defaulting unrecognized suffixes to `WeightedSum`, any column whose naming convention doesn't match the expected pattern produces inflated fleet totals (scaled by the sum of weights across dwellings instead of the weighted average).

**Impact**:
- The private `ColumnAggregation::for_column()` (line 44) has the inverse default: unrecognized suffixes default to `Mean`, not `Sum`. This inconsistency between the two classification functions creates confusion about the intended default behavior.
- If the telemetry system ever emits a column with a non-standard suffix (e.g., `Efficiency (%)`, `COP (W/W)`, `EER`), it would silently receive `WeightedSum` in fleet aggregation, producing values scaled by total fleet weight rather than an appropriate weighted average.
- The lack of a `tracing::warn!` call for unrecognized suffixes means violations of the naming contract produce no diagnostic.

---

### Finding 4: [Severity: low]
**Description**: The `build_aggregate_batch()` function exhaustively categorizes columns and applies weighted aggregation for every timestep, even when all `sample_weight` values are `1.0`. There is no fast path that detects the all-1.0 case and performs a simpler unweighted sum. This is a performance optimization opportunity, not a correctness issue.

**Code Location**: `crates/hares-fleet/src/aggregation.rs:345-382`
```rust
let mut weighted_values = vec![0.0; column_count];
let mut total_weight = vec![0.0; column_count];
// ... loop per dwelling ...
weighted_values[idx] += *v * *sample_weight;   // redundant multiply-by-1
total_weight[idx] += *sample_weight;           // redundant add-1
```

**Impact**: For unweighted fleets (the common case when using `from_buildings()` without explicit weights), the inner loop performs redundant multiplication (`v * 1.0`) and addition (`total + 1.0`). For a fleet of 100,000 dwellings with 1+ year of hourly timesteps, this represents N × 8760 operations that could be skipped. However, the primary cost in fleet simulation is the per-dwelling EnergyPlus run, not the aggregation math — this is likely noise relative to simulation time.

---

## Summary
- **Total findings**: 4
- **Information**: 1 (Finding 1: no double-weighting bug exists)
- **Critical**: 0
- **High**: 1 (Finding 2: silent corruption from invalid sample weights)
- **Medium**: 1 (Finding 3: overly broad WeightedSum default)
- **Low**: 1 (Finding 4: redundant math for unweighted fleets)

## Recommendations

1. **Add sample weight validation at the data-ingestion boundary** (Priority: high). In `parse_resstock_metadata()` (`crates/hares-io/src/resstock.rs:274`) or at fleet construction, validate that each parsed `sample_weight` is finite, non-NaN, and > 0.0. Emit `tracing::error!` and reject the building (or return an `Err`) on invalid weights. For Python user-provided weights in `py_fleet.rs`, add the same validation before assigning to fleet entries.

2. **Add a `tracing::warn!` for unrecognized column suffixes** (Priority: medium). In `FleetAggregation::for_column()` and `ColumnAggregation::for_column()`, log a warning when a column name does not match any recognized suffix, so users know the classification is based on the default fallthrough.

3. **Consider inverting the default for `FleetAggregation::for_column()`** (Priority: medium). Default to `WeightedMean` instead of `WeightedSum` for unrecognized suffixes. `WeightedMean` is the safer default because it converges to a reasonable value regardless of fleet size, whereas `WeightedSum` scales with fleet weight and produces nonsensical results for intensive quantities.

4. **Document the column-naming contract** (Priority: low). The suffix-based classification depends on telemetry column names following the pattern `"Description (unit)"`. This contract should be documented in the telemetry system (column naming conventions) and in the aggregation module's doc comment so that future telemetry additions respect it.

## References / Citations
- `crates/hares-fleet/src/aggregation.rs` — All three aggregation layers and tests
- `crates/hares-fleet/src/fleet.rs:488-511` — `run_entry()`: sample weight plumbing from `FleetEntry` to `DwellingOutcome`
- `crates/hares-fleet/src/fleet.rs:104-110` — `from_buildings()`: default `sample_weight = 1.0`
- `crates/hares-fleet/src/fleet.rs:129-161` — `from_resstock()`: sample weight from ResStock metadata (line 152)
- `crates/hares-fleet/src/fleet.rs:184-193` — `with_sample_weights()`: user-provided weight override
- `crates/hares-python/src/py_fleet.rs:99-106` — Python weight length validation (no value validation)
- `crates/hares-io/src/resstock.rs:86-126` — ResStock metadata parsing (no weight range validation)
