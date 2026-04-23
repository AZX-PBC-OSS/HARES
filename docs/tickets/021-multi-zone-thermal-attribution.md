# Multi-Zone Thermal Attribution Output Columns

**Severity**: Low
**Priority**: P2
**Status**: Open
**Areas**: hares-io/output, hares-equipment/hvac, hares-core

## Problem

HVAC thermal output to multiple zones (conditioned zone, duct zone, basement zone) is routed correctly via `zone_heat_fractions` in the port system, but the per-zone breakdown is not available in any output column. For multi-zone buildings, you can only see total delivered HVAC power (e.g., "HVAC Heating Delivered (W)" at verbosity 4), not which zones received it and how much.

The thermal solver distributes gross HVAC output across zones using `write_zone_thermal_contributions()` at `duct_distribution.rs:87-123`, which tags each zone's contribution with a `ThermalCategory` (`HvacHeating`, `HvacCooling`, or `DuctLoss`). But this per-zone detail is lost when recording output — only the aggregate delivered value is reported.

## Current Behavior

### Output columns at `columns.rs:129-148`

Verbosity 4 adds two aggregate columns:
- `"HVAC Heating Delivered (W)"` — total heating delivered
- `"HVAC Cooling Delivered (W)""` — total cooling delivered

No per-zone breakdown exists at any verbosity level.

### Zone heat fraction routing at `duct_distribution.rs:22-64`

`update_zone_heat_fractions()` computes fractions for up to 3 zones:

- **Conditioned zone**: `effective_dse * (1 - basement_frac)` — e.g., 0.595 for DSE=0.85, basement=0.30
- **Basement zone**: `effective_dse * basement_frac` — e.g., 0.255
- **Duct zone**: `1.0 - effective_dse` — e.g., 0.15

Fractions always sum to 1.0 (verified in test at `duct_distribution.rs:229-233`).

### Per-zone thermal contributions at `duct_distribution.rs:87-123`

`write_zone_thermal_contributions()` writes per-zone contributions to port slots, tagged with `ThermalCategory`:

- Conditioned zone receives `HvacHeating` or `HvacCooling` category
- Duct zone receives `ThermalCategory::DuctLoss` category (lines 107-112)
- Each zone gets `sensible_gain_w * fraction` and `latent_gain_w * fraction`

### DuctLoss thermal category at `duct_distribution.rs:107-119`

Duct losses are already categorized separately via `ThermalCategory::DuctLoss`, but this category has no corresponding output column. The `"Duct Loss Heat Gain - Indoor (W)"` column at verbosity 6 (line 184) is from the envelope solver, not from HVAC duct distribution.

### Record step at `dwelling/mod.rs:2496-2560`

Output recording reads from `CoreOutput` and telemetry. The aggregate "HVAC Heating Delivered (W)" column is populated from the conditioned zone's thermal accumulator after the solver resolves. No per-zone breakdown is extracted.

## Required Behavior

At output verbosity 6+, add per-zone thermal attribution columns:

1. `HVAC Heating Delivered - {zone_name} (W)` for each zone in `zone_heat_fractions`
2. `HVAC Cooling Delivered - {zone_name} (W)` for each zone in `zone_heat_fractions`

> Note: `HVAC Duct Losses (W)` at verbosity 5 is owned by ticket 017. This ticket does NOT add that column. See "Dependency on 017" below.

The per-zone columns enable diagnosing questions like:
- "How much of the furnace output went to the finished basement?"
- "How much cooling was lost to the attic duct zone?"

## Approach

### Step 1: Plumb zone names into the output recording system

The output column schema is built before simulation starts (in `build_schema()` at `columns.rs:53`). At that point, the building's zone names and IDs are known from the HPXML parse. Add a `zone_names: Vec<(ZoneId, String)>` parameter to `build_schema()` (or pass it through the existing `EquipmentSpec` mechanism).

> **Schema build timing**: `build_schema()` in `columns.rs` receives equipment
> names but not zone names. Zone names come from the HPXML building parse
> which happens in a different phase. Resolution: pass `zone_names:
> Vec<(ZoneId, String)>` to `build_schema()` from the dwelling initialization
> path, or add a post-init schema update mechanism that appends zone-specific
> columns after the building is parsed.

### Step 2: Add per-zone HVAC output columns at verbosity 6

In `columns.rs`, after the existing v6 section (lines 172-196):

```rust
if verbosity >= 6 {
    // Per-zone HVAC thermal attribution columns
    for (zone_id, zone_name) in &zone_names {
        fields.push(Field::new(
            format!("HVAC Heating Delivered - {zone_name} (W)"),
            DataType::Float64,
            true,
        ));
        fields.push(Field::new(
            format!("HVAC Cooling Delivered - {zone_name} (W)"),
            DataType::Float64,
            true,
        ));
    }
}
```

### Step 3: (Deferred to ticket 017) Duct losses column

The `"HVAC Duct Losses (W)"` column at verbosity 5 is owned by ticket 017, which also defines the `DUCT_LOSS_W` telemetry key. Do not add this column here. Ticket 017 must ship before this ticket. Once 017 has shipped, this ticket's Step 4 may read the aggregate duct loss from the `DUCT_LOSS_W` telemetry key for consistency checks but must not re-declare the column.

### Step 4: Record per-zone HVAC values in `record_step()`

After the thermal solver resolves, each zone's `ThermalAccumulator` contains per-category sensible and latent gains. In `record_step()`, for each zone:

1. Read `acc.sensible_for_category(ThermalCategory::HvacHeating)` → "HVAC Heating Delivered - {zone_name} (W)"
2. Read `acc.sensible_for_category(ThermalCategory::HvacCooling).abs()` → "HVAC Cooling Delivered - {zone_name} (W)"

Do not write to "HVAC Duct Losses (W)" here — that column is owned by ticket 017.

### Step 5: Pre-compute column indices for per-zone columns

Extend `EquipmentColumns` or create a new `ZoneColumns` struct in `conversions.rs` to hold pre-resolved indices for per-zone HVAC columns, avoiding per-step `format!()` allocations.

### Step 6: Verify per-zone values sum to total delivered

Add a consistency check: sum of per-zone "HVAC Heating Delivered" must equal the aggregate "HVAC Heating Delivered (W)" column (up to floating-point precision). This can be a debug-mode assertion.

### Step 7: Column count and dynamic schema handling

**Column count**: For a typical residential building (1-3 zones), this adds 2-6 columns at v6. For multi-zone buildings with more than 3 zones, consider a separate zone-attribution output file rather than multiplying main output columns. Handle missing zones by omitting their columns (dynamic schema). Buildings where a zone has no HVAC contribution should still have the column present (with 0.0 value), since the schema is fixed at init time. If the schema cannot be built with zone names at init, use a post-init schema update mechanism as described in Step 1.

## Dependency on 017

This ticket requires ticket 017 to ship first. 017 owns the `DUCT_LOSS_W` telemetry key and the `"HVAC Duct Losses (W)"` column at verbosity 5. This ticket adds only the per-zone attribution columns at verbosity 6 and must not introduce a duplicate duct-losses column.

## Migration: `build_schema()` API change

The proposed `zone_names: Vec<(ZoneId, String)>` parameter is a breaking change to `build_schema()`. All call sites must be updated. Current call sites (verified by source grep):

| File | Line | Change required |
|---|---|---|
| `hares-core/src/dwelling/mod.rs` | 1041 | Add `zone_names` argument from dwelling's zone registry |
| `hares-core/src/dwelling/mod.rs` | 1571 | Add `zone_names` argument (schema rebuild path) |
| `hares-core/src/dwelling/mod.rs` | 5413 | Pass empty `vec![]` (test helper; no zones needed) |
| `hares-io/src/output/metrics.rs` | 1188 | Pass empty `vec![]` (metrics test; no zone columns) |
| `hares-io/src/output/columns.rs` | 384, 412, 425, 433, 442, 449, 457, 476, 486, 505 | Unit test call sites — pass `vec![]` where zone columns not under test; create zone-aware test helpers for the v6 zone-column tests |
| `hares-io/tests/comprehensive_tests.rs` | 98, 113, 130, 146, 161, 189, 228, 229, 241, 242, 254, 255, 267, 278, 289 | Same: pass `vec![]` for non-zone tests; add zone-name fixtures for v6 tests |

The implementer must audit all 24+ call sites before merging. A compile-error-driven approach is acceptable: change the signature, fix every compiler error.

## Attribution Math Invariant

The `conditioned_frac + basement_frac + duct_frac = 1.0` invariant is verified at `duct_distribution.rs:229-233` (confirmed by source read). This test covers the fraction sum; add a companion test verifying that per-zone output values sum to the aggregate "HVAC Heating Delivered (W)" column.

## Definition of Done

- [ ] Per-zone HVAC output columns present at verbosity 6 for each zone in the building
- [ ] `HVAC Duct Losses (W)` column at verbosity 5 is NOT added here (owned by ticket 017; 017 must ship first)
- [ ] Per-zone heating values populated from `ThermalAccumulator` sensible gains for `HvacHeating` category
- [ ] Per-zone cooling values populated from `ThermalAccumulator` sensible gains for `HvacCooling` category
- [ ] Per-zone values sum to aggregate total (verified in debug builds)
- [ ] Single-zone buildings produce exactly 2 per-zone columns (one for the only zone)
- [ ] All `build_schema()` call sites updated (see Migration section); no compile errors
- [ ] Existing tests pass; new schema tests added for v6 with zone names

## Verification

1. Run a multi-zone simulation (conditioned zone + attic duct zone + basement zone) with `duct_dse = 0.80` at verbosity 6. Verify:
   - Per-zone heating values: conditioned zone gets `gross * dse * (1 - basement_frac)`, duct zone gets `gross * (1 - dse)`, basement gets `gross * dse * basement_frac`
   - Sum equals aggregate "HVAC Heating Delivered (W)"
2. Run a single-zone simulation (DSE=1.0, no duct zone) at verbosity 6. Verify one per-zone column pair exists and matches the aggregate.
3. Verify "HVAC Duct Losses (W)" = 0 for DSE=1.0 and > 0 for DSE < 1.0.

## References

- `duct_distribution.rs:22-64`: `update_zone_heat_fractions()` — fraction computation
- `duct_distribution.rs:87-123`: `write_zone_thermal_contributions()` — per-zone thermal writing with `DuctLoss` category
- `duct_distribution.rs:107-119`: `ThermalCategory::DuctLoss` tagging for duct zone
- `columns.rs:129-148`: Current v4 HVAC aggregate columns
- `columns.rs:172-196`: Current v6 envelope component columns
- `dwelling/mod.rs:2496-2560`: `record_step()` output recording
- `hares-types/src/ports.rs:17`: `ThermalCategory` enum (confirmed location)

## Related Tickets

- #017 — Missing output columns v7 (DUCT_LOSS_W telemetry key and Duct Losses output column)
- #018 — CoreOutput HVAC promotion (thermal_output_w in CoreOutput may enable cleaner per-zone routing)
