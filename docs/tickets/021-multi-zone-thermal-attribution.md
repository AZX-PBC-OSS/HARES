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

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-20

### Code Confirmation

- [x] Referenced line numbers still match (with minor corrections noted below)
- [x] Described logic matches current implementation
- [x] OCHRE cross-check result: **matches** — OCHRE HVAC.py lines 188-197 confirmed identical zone-fraction logic; OCHRE also has NO per-zone HVAC output columns (only aggregate "HVAC Heating Delivered (W)" at v4)
- [x] EnergyPlus cross-check result: N/A — EnergyPlus uses per-zone terminal unit models rather than a DSE scalar; there is no EnergyPlus Engineering Reference section defining per-zone DSE attribution columns. The ticket does not cite EnergyPlus as a source for this feature.

**Line number corrections:**

| Ticket claim | Actual location | Notes |
|---|---|---|
| `duct_distribution.rs:22-64` | Lines 22–64 | Confirmed exact match (`update_zone_heat_fractions`) |
| `duct_distribution.rs:87-123` | Lines 87–123 | Confirmed exact match (`write_zone_thermal_contributions`) |
| `duct_distribution.rs:107-119` | Lines 107–119 | Confirmed: duct zone tagged `ThermalCategory::DuctLoss` |
| `duct_distribution.rs:229-233` | Lines 229–233 | Confirmed: fraction-sum assertion in test |
| `columns.rs:129-148` | Lines 129–148 | Confirmed: verbosity-4 block with aggregate HVAC columns |
| `columns.rs:172-196` | Lines 172–196 | Confirmed: verbosity-6 envelope component block |
| `dwelling/mod.rs:2496-2560` | `record_step` starts line 2499 (not 2496) | Off-by-3; line 2496 is the closing `Ok(())` of the prior function. `record_step` body runs 2499–2669. Non-material. |
| `hares-types/src/ports.rs:17` | Line 17 | Confirmed: `ThermalCategory` enum definition |

**Key behavioral facts confirmed by code reading:**

1. `duct_distribution.rs:87–123` (`write_zone_thermal_contributions`) correctly distributes gross capacity × zone fraction to each zone's `ThermalAccumulator`, tagged `HvacHeating`/`HvacCooling` for the conditioned zone and `DuctLoss` for the duct zone.
2. `columns.rs:129–148`: only `"HVAC Heating Delivered (W)"` and `"HVAC Cooling Delivered (W)"` exist at verbosity 4. No per-zone breakdown at any verbosity level.
3. `dwelling/mod.rs:2638–2639` (`record_step`): the two aggregate columns are populated from `gains.hvac_heating_w` / `gains.hvac_cooling_w` (indoor zone only). Per-zone HVAC values are never written to output columns.
4. `record_step` does write per-zone values for infiltration (`zone_infiltration_columns`, line 2658) and interior LWR (`zone_lwr_columns`, line 2664) — confirming the infrastructure for per-zone output exists, but HVAC is not wired into it.
5. `build_schema` signature at all three `dwelling/mod.rs` call sites (lines 1041, 1571, 5413) takes only `(&[EquipmentSpec], u8)` — no `zone_names` parameter yet.

### Web-Verified Citations

The ticket contains no explicit standards citations (ASHRAE, NFRC, DOE, ISO) — it is a pure output-gap feature request referencing only OCHRE and internal code. The OCHRE reference was verified:

- **Citation**: "OCHRE HVAC.py lines 188-197" (stated in `duct_distribution.rs:21` doc comment)
- **Source found**: OCHRE source at `/Users/rich/source/HARES/vendors/OCHRE/ochre/Equipment/HVAC.py`, confirmed via OCHRE ReadTheDocs at <https://ochre-nrel.readthedocs.io/en/stable/ModelingApproach.html> and <https://ochre-nrel.readthedocs.io/en/stable/Outputs.html>
- **Quoted passage** (HVAC.py lines 188-197):
  ```python
  # Determine heat fractions per zone (Indoor zone, duct zone, and basement zone)
  self.zone_fractions = {self.zone: self.duct_dse * (1 - self.basement_heat_frac)}
  if self.duct_dse < 1 and self.duct_zone is not None:
      # if duct_zone is None, DSE losses don't get added to another zone
      self.zone_fractions[self.duct_zone] = 1 - self.duct_dse
  if self.basement_heat_frac > 0:
      if basement_zone == self.duct_zone:
          self.zone_fractions[basement_zone] += self.duct_dse * self.basement_heat_frac
      else:
          self.zone_fractions[basement_zone] = self.duct_dse * self.basement_heat_frac
  ```
- **Verdict**: Confirmed. The HARES `update_zone_heat_fractions()` is a faithful Rust translation of this Python logic.

- **Citation**: OCHRE outputs "HVAC Heating/Cooling Delivered (W)" aggregate only (ticket's claim that OCHRE has no per-zone breakdown)
- **Source found**: OCHRE HVAC.py `generate_results()` lines 568-604, confirmed via ReadTheDocs Outputs page
- **Quoted passage** (HVAC.py line 582):
  ```python
  results[f"{self.end_use} Delivered (W)"] = abs(self.delivered_heat) * self.duct_dse
  ```
  The `add_gains_to_zone()` method (lines 563-566) distributes to zone objects internally but no per-zone column is emitted. ReadTheDocs confirms: "HVAC sensible heat gain delivered to indoor zone."
- **Verdict**: Confirmed. OCHRE emits only the aggregate column. The ticket correctly identifies this as a gap that HARES can optionally surpass.

- **Citation**: ASHRAE 152 (mentioned in OCHRE ReadTheDocs as the DSE methodology source)
- **Source found**: <https://ochre-nrel.readthedocs.io/en/stable/ModelingApproach.html> states "DSE values are calculated according to ASHRAE 152"; ASHRAE 152 title confirmed via OSTI/BNL at <https://www.osti.gov/biblio/15006983>
- **Quoted passage**: OCHRE docs: "DSE values are calculated according to ASHRAE 152 and represent the seasonal DSE in both heating and cooling."
- **Verdict**: The ticket does not cite ASHRAE 152 directly, but the underlying DSE model it relies on is ASHRAE 152-compliant per OCHRE docs. No discrepancy.

### Legitimacy

- **Verdict**: **Legitimate**

- **Rationale**: The core claim is real and code-verified: `write_zone_thermal_contributions()` in `duct_distribution.rs` correctly distributes HVAC heat across up to three zones using `zone_heat_fractions` and tags each contribution with `ThermalCategory` (HvacHeating, HvacCooling, or DuctLoss). The per-zone breakdown is already present in `ThermalAccumulator` after each timestep. However, `build_schema()` has no `zone_names` parameter and emits no per-zone HVAC columns at any verbosity, and `record_step()` in `dwelling/mod.rs:2638-2639` reads only the aggregate indoor-zone values. The three regression tests added by this audit (two failing, one passing) confirm the gap is real. The fraction formulas, OCHRE line citations, and described `record_step` behavior are all accurate. The sole minor inaccuracy is the `record_step` line range (2496 vs the correct 2499), which is immaterial to the bug description. OCHRE itself does not have per-zone HVAC output columns, so this feature would be a HARES extension beyond OCHRE parity — the ticket correctly notes this.

### Proposed Fix Summary

1. Add `zone_names: Vec<(ZoneId, String)>` parameter to `build_schema()` in `columns.rs`.
2. In the `verbosity >= 6` block, iterate `zone_names` and push `"HVAC Heating Delivered - {name} (W)"` and `"HVAC Cooling Delivered - {name} (W)"` fields.
3. Update all 24+ `build_schema()` call sites (see Migration table in ticket) to pass `vec![]` or actual zone names as appropriate.
4. In `record_step()`, after the existing `envelope_cols` loop, iterate over each zone in the building's zone registry and write `acc.sensible_for_category(ThermalCategory::HvacHeating)` / `.abs()` of `HvacCooling` into the pre-resolved per-zone column indices.
5. Add a debug-mode assertion that the sum of per-zone heating values equals `"HVAC Heating Delivered (W)"`.
6. Do NOT add `"HVAC Duct Losses (W)"` here — that is owned by ticket #017.

### Test Written

- **File**: `crates/hares-io/src/output/columns.rs` (within `#[cfg(test)]` module, end of file)
- **Tests added**:
  - `ticket_021_verbosity_6_has_per_zone_hvac_heating_columns` — **FAILS** (documents gap: `"HVAC Heating Delivered - Indoor (W)"` absent at v6)
  - `ticket_021_verbosity_6_has_per_zone_hvac_cooling_columns` — **FAILS** (documents gap: `"HVAC Cooling Delivered - Indoor (W)"` absent at v6)
  - `ticket_021_per_zone_hvac_columns_not_present_below_verbosity_6` — **PASSES** (verifies correct threshold: no per-zone columns at v0–v5)
