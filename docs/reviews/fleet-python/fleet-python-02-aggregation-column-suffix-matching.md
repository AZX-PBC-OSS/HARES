# Fleet aggregation: column classification by suffix string matching only
**Review ID**: fleet-python-02
**Category**: fleet-python
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-fleet/src/aggregation.rs` (primary)
- `crates/hares-io/src/output/columns.rs` (column name schema construction)
- `crates/hares-types/src/equipment.rs` (TelemetryField metadata, lines 1080–1086)
- `crates/hares-io/src/output/metrics.rs` (suffix-based column discovery, lines 777–860)
- `scripts/reviews/fleet-python.json` (pre-existing review definition)

## Vendor/Reference Files Consulted
None specified.

## Findings

### Finding 1: [Severity: high]
**Description**: Three column unit suffixes used in the output schema have no explicit match in either `ColumnAggregation::for_column()` or `FleetAggregation::for_column()` and silently fall through to defaults with zero warning. The affected suffixes are `(therms/hour)`, `(kVAR)`, and `(W)`. Each carries distinct aggregation semantics that may differ from the default:

- **`(therms/hour)`**: A fuel-power rate suffix used for `"Total Gas Power (therms/hour)"` and per-equipment gas power columns. Falls through to `ColumnAggregation::Mean` (likely correct for temporal resampling of a rate) and `FleetAggregation::WeightedSum` (likely correct for summing gas power across dwellings), but the correctness is accidental — no review or test confirms this intent.
- **`(kVAR)`**: Reactive power suffix used for `"Total Reactive Power (kVAR)"` and per-equipment reactive power columns. Same accidental fallthrough as `(therms/hour)`.
- **`(W)`**: Watts suffix used across verbosity levels 5–7 for `"Net Sensible Heat Gain - Indoor (W)"`, `"HVAC Heating Delivered - Indoor (W)"`, `"HVAC Duct Losses (W)"`, latent gains, infiltration heat gains, capacity, etc. Fallthrough to `ColumnAggregation::Mean` is probably correct for rate-like columns; fallthrough to `FleetAggregation::WeightedSum` is appropriate for delivered thermal power but potentially wrong for dimensionless metrics. No warning is emitted to call attention to this ambiguity.

**Code Location**: `crates/hares-fleet/src/aggregation.rs:44–61` (`ColumnAggregation::for_column`) and `crates/hares-fleet/src/aggregation.rs:74–87` (`FleetAggregation::for_column`). The output column names are generated in `crates/hares-io/src/output/columns.rs` with the unhandled suffixes at lines 17 (`therms/hour`), 18/165 (`kVAR`), 34/177/196–202/264–269 (`W`).

**Root Cause**: The suffix-to-aggregation mapping is incomplete. Only `(kWh)`, `(kW)`, `(C)`, `(°C)`, and `(-)` are explicitly enumerated. Developers rely on undocumented defaults without any lint, test, or runtime check that would fire when a new suffix appears.

**Impact**: If a column with suffix `(therms/hour)`, `(kVAR)`, or `(W)` requires different aggregation than the default (e.g., `(therms/hour)` gas power columns need `Sum` for temporal energy integration in some contexts), the silently-applied default produces incorrect fleet totals. More critically, any new equipment or output column that uses a suffix outside the recognized set will silently receive defaults with no diagnostic.

---

### Finding 2: [Severity: high]
**Description**: The `TelemetryField` metadata struct — carrying `name`, `unit`, and `description` — exists on every `EquipmentDescriptor` but is completely unused by the aggregation pipeline. Equipment models declare telemetry channels with explicit units (e.g., `unit: "kW"`, `unit: "kWh"`, `unit: "C"`, `unit: "s"`, `unit: "kWh/mi"`, `unit: "enum"`) in `TelemetryField`, but this structured metadata is discarded during output schema construction. Instead, column names are rebuilt from hardcoded string constants in `columns.rs`, and the fleet aggregation reverse-parses those strings with `ends_with()`.

The result is a "round-trip gap": equipment declares metadata → metadata is discarded → column names are reconstructed from hardcoded suffixes → aggregation reverse-parses the column names. Any divergence between the original `TelemetryField` unit and the reconstructed suffix goes undetected.

**Code Location**:
- `TelemetryField` struct: `crates/hares-types/src/equipment.rs:1080–1086`
- `EquipmentDescriptor.telemetry_fields`: `crates/hares-types/src/equipment.rs:1132`
- Hardcoded suffix constants: `crates/hares-io/src/output/columns.rs:24–50`
- Suffix matching: `crates/hares-fleet/src/aggregation.rs:44–61,74–87`

**Root Cause**: Architecture gap between the equipment metadata layer (which knows about units) and the aggregation layer (which needs to know about units). No path exists to carry `TelemetryField` metadata through the output schema (e.g., as Arrow schema metadata annotations or a parallel manifest) into the fleet aggregation code.

**Impact**: Any unit suffix declared by equipment models that doesn't match a hardcoded suffix in `columns.rs` or `aggregation.rs` will silently produce wrong aggregation. For example, EV efficiency columns with `unit: "kWh/mi"` would get `Mean` for temporal resampling at Phase 1 (accidentally reasonable) and `WeightedSum` at Phase 2 (potentially wrong — summing efficiency ratios across dwellings is nonsensical). The system cannot automatically handle new equipment types with non-standard telemetry units.

---

### Finding 3: [Severity: medium]
**Description**: The two aggregation phases maintain independent, partially-overlapping suffix lists. A column suffix that is handled in one phase may not be handled in the other, creating subtle mismatches:

| Suffix | ColumnAggregation (Phase 1, temp. resample) | FleetAggregation (Phase 2, cross-dwelling) |
|---|---|---|
| `(kWh)` | **Sum** (explicit) | WeightedSum (default fallthrough) |
| `(kW)` | Mean (explicit) | WeightedSum (default fallthrough) |
| `(C)` / `(°C)` | Mean (explicit) | **WeightedMean** (explicit) |
| `(-)` | Mean (explicit) | **WeightedMean** (explicit) |
| `(therms/hour)` | Mean (default, silent) | WeightedSum (default, silent) |
| `(kVAR)` | Mean (default, silent) | WeightedSum (default, silent) |
| `(W)` | Mean (default, silent) | WeightedSum (default, silent) |

`(kWh)` is explicitly handled in `ColumnAggregation` (→ Sum) but falls through to the default in `FleetAggregation` (→ WeightedSum). This happens to be correct, but the inconsistency makes it unclear whether the missing explicit `(kWh)` match in `FleetAggregation` is intentional or an oversight. A developer reading one method but not the other would get an incomplete picture of how a given column type is aggregated.

**Code Location**: `crates/hares-fleet/src/aggregation.rs:44–61` vs `crates/hares-fleet/src/aggregation.rs:74–87`.

**Root Cause**: Two separate `for_column()` methods were implemented independently. No shared definition maps suffix → (Phase 1 agg, Phase 2 agg).

**Impact**: Moderate — current behavior is correct for known columns, but the split table invites future drift. A suffix added to one phase but not the other would be silently inconsistent.

---

### Finding 4: [Severity: medium]
**Description**: No test exercises the aggregation behavior of columns with suffixes outside the explicitly-recognized set. All three inline tests (`weighted_sum_uses_sample_weights_and_nulls`, `hourly_resample_applies_suffix_aggregation_rules`, `fleet_weighted_mean_vs_sum_with_unequal_weights`) use only columns with suffixes `(kW)`, `(kWh)`, `(C)`, and `(-)` — all explicitly recognized. Columns with suffixes `(therms/hour)`, `(kVAR)`, or `(W)` are never tested.

**Code Location**: `crates/hares-fleet/src/aggregation.rs:485–729` (all three `#[test]` functions). Column names used: `"Total Electric Power (kW)"`, `"Battery Energy (kWh)"`, `"Temperature - Indoor (C)"`, `"Battery SOC (-)"`.

**Root Cause**: The test suite validates only the explicitly-handled suffixes, missing the silent-fallback code paths for unrecognized suffixes.

**Impact**: A regression that changes the default aggregation behavior (e.g., a refactor that replaces the fallback `Self::Mean` with `Self::Sum`) would not be caught by any test if it only affects unhandled suffixes. The test suite cannot distinguish between "correctly silent fallback" and "incorrectly silent fallback."

---

### Finding 5: [Severity: low]
**Description**: The `ColumnAggregation::for_column()` method at `aggregation.rs:44–61` normalizes the column name by trimming whitespace (`name.trim()`) but does not normalize case. The output schema in `columns.rs` uses mixed-case column names (e.g., `"Total Electric Power (kW)"`) that end with lowercase unit suffixes. If a column were produced with capitalized suffixes (e.g., `"Power (KW)"` or `"Power (Kw)"`), the case-sensitive `ends_with()` would fail to match.

**Code Location**: `crates/hares-fleet/src/aggregation.rs:45–46`.

**Root Cause**: No case normalization is applied alongside the trim normalization.

**Impact**: Low — the current codebase uses consistent casing for all column names and unit suffixes. However, if a Python adapter or external source injects columns with non-standard casing, the aggregation silently falls to defaults.

---

## Summary
- **Total findings**: 5
- **Critical**: 0
- **High**: 2 (unhandled suffixes with silent defaults; unused TelemetryField metadata)
- **Medium**: 2 (split suffix lists; no test coverage for unrecognized suffixes)
- **Low**: 1 (no case normalization)

## Recommendations

1. **Add explicit handling for all known suffixes**: Extend both `ColumnAggregation::for_column()` and `FleetAggregation::for_column()` to explicitly handle `(therms/hour)`, `(kVAR)`, and `(W)`, documenting the rationale for each choice. This removes the accidental-fallback risk for columns already in the output schema.

2. **Add a `tracing::warn!` (or `log::warn!`) for unrecognized suffixes**: In both `for_column()` methods, if no condition matches after the explicit suffix checks, emit a warning log identifying the column name before returning the default. This surfaces new or misnamed columns immediately in production runs rather than silently producing potentially wrong results.

3. **Drive aggregation from `TelemetryField` metadata**: Connect the equipment's `TelemetryField { name, unit, description }` metadata through to the output schema (e.g., as Arrow schema metadata keyed by column index) so the aggregation layer can read `unit` directly rather than parsing column names. Define a `unit → aggregation` mapping (e.g., `kWh → Sum`, `kW → Mean`, `C → Mean`, `- → Mean`, `W → Mean`, `therms/hour → Mean`, `kVAR → Mean`, `kWh/mi → Mean`, `s → Mean`, `enum → Mean` for Phase 1; and complementary mappings for Phase 2). This eliminates the round-trip gap and makes aggregation future-proof against new equipment types.

4. **Add a shared suffix-to-aggregation lookup table**: Consolidate the two independent `for_column()` methods into a single table that maps each suffix to both `ColumnAggregation` and `FleetAggregation` values. This makes it obvious when a suffix is handled in only one phase.

5. **Add test coverage for unhandled suffixes**: Add a test that exercises columns with `(therms/hour)`, `(kVAR)`, and `(W)` suffixes, verifying the current default behavior is intentional. If the behavior is changed per recommendations 1–3, these tests protect against regression.

6. **Normalize case in `for_column()`**: Apply `to_ascii_lowercase()` alongside the existing `trim()` to make suffix matching case-insensitive. The unit parentheses are always ASCII, so `to_ascii_lowercase()` is safe and sufficient.

## References / Citations
- `crates/hares-fleet/src/aggregation.rs:44–61` — `ColumnAggregation::for_column()` suffix matching
- `crates/hares-fleet/src/aggregation.rs:74–87` — `FleetAggregation::for_column()` suffix matching
- `crates/hares-types/src/equipment.rs:1080–1086` — `TelemetryField` struct definition
- `crates/hares-types/src/equipment.rs:1116–1133` — `EquipmentDescriptor` with `telemetry_fields`
- `crates/hares-io/src/output/columns.rs:14–50` — hardcoded column suffix constants
- `crates/hares-io/src/output/columns.rs:60–315` — `build_schema()` constructing column names from constants
- `crates/hares-io/src/output/metrics.rs:777–800` — `discover_end_use_columns()` suffix matching
- `crates/hares-io/src/output/metrics.rs:836–860` — `discover_conditioned_zone_temperatures()` suffix matching
- `scripts/reviews/fleet-python.json:14–19` — pre-existing review definition identifying the fragility
