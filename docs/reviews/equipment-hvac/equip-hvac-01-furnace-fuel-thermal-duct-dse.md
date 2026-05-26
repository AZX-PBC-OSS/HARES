# Furnace fuel/thermal balance with duct DSE interaction
**Review ID**: equip-hvac-01
**Category**: equipment-hvac
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-equipment/src/hvac/furnace.rs`
- `crates/hares-equipment/src/hvac/duct_distribution.rs`
- `crates/hares-equipment/src/hvac/hvac_core.rs`

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Equipment/HVAC.py`

## Findings

### Finding 1: [Severity: high]
**Description**: Telemetry and CoreOutput report phantom duct losses when ducts are in conditioned space. When `duct_zone_id == zone_id`, `update_zone_heat_fractions()` (duct_distribution.rs:22-41) correctly treats DSE as 1.0 for thermal routing via `effective_dse`, but the furnace `step()` methods use the raw `self.hvac.config.duct_dse` for `thermal_output_w` and `duct_loss_w` telemetry. This causes the reported thermal output to be lower than actual delivery and reports non-existent duct losses.

**Code Location**:
- `duct_distribution.rs:25-39` -- `effective_dse` computed locally but never stored back to `self.config.duct_dse`
- `furnace.rs:203` (ElectricFurnace) and `furnace.rs:520` (GasFurnace) -- `thermal_output_w = total_sensible_w * self.hvac.config.duct_dse` (raw value)
- `furnace.rs:205` (ElectricFurnace) and `furnace.rs:522` (GasFurnace) -- `duct_loss_w = gross_capacity_w * (1.0 - self.hvac.config.duct_dse)` (raw value)
- `furnace.rs:273-278` (ElectricFurnace) and `furnace.rs:592-600` (GasFurnace) -- CoreOutput uses the misreported `thermal_output_w`

**Root Cause**: `update_zone_heat_fractions()` computes an `effective_dse` local variable to handle the "ducts in conditioned space" case, but never mutates `self.config.duct_dse`. OCHRE (HVAC.py:175-177) mutates `self.duct_dse = 1` when ducts are in the conditioned zone, so all downstream consumers see the corrected value. HARES leaves the raw DSE in config and only uses the correction for `zone_heat_fractions`.

**Impact**: Any consumer of `CoreOutput.thermal_output_w` or the `DUCT_LOSS_W` / `THERMAL_OUTPUT_W` telemetry keys will see values that disagree with the actual zone thermal delivery. When ducts are in the conditioned space with e.g. DSE=0.85, the zone actually receives 100% of gross capacity, but the reported `thermal_output_w` shows only 85%. This creates a false energy-balance discrepancy at the simulation framework level.

### Finding 2: [Severity: medium]
**Description**: `duct_loss_w` telemetry value excludes fan heat when computing duct losses to the duct zone, while `write_zone_thermal_contributions` includes fan heat in the duct zone thermal contribution. When a duct zone is present (different from conditioned zone), the duct zone receives `(gross_capacity_w + fan_heat_w) * (1 - dse)` watts via port accumulation, but the `duct_loss_w` telemetry key reports only `gross_capacity_w * (1 - dse)`, omitting `fan_heat_w * (1 - dse)`.

**Code Location**:
- `furnace.rs:187-197` (ElectricFurnace) and `furnace.rs:502-513` (GasFurnace) -- `write_zone_thermal_contributions` is called with `total_sensible_w` (includes `fan_heat_w`)
- `furnace.rs:205` (ElectricFurnace) and `furnace.rs:522` (GasFurnace) -- `duct_loss_w = gross_capacity_w * (1 - dse)` (excludes fan heat)
- `duct_distribution.rs:109-130` -- `write_zone_thermal_contributions` distributes `sensible_gain_w * fraction` to each zone, including fan heat in the duct zone fraction

**Root Cause**: The duct loss telemetry is computed from `gross_capacity_w` only, but the actual thermal energy deposited into the duct zone PortSlots includes fan heat. OCHRE (HVAC.py:589) computes duct losses as `abs(self.delivered_heat) * (1 - self.duct_dse)` where `delivered_heat` includes fan power, so OCHRE is consistent.

**Impact**: When fan power is non-trivial (e.g. 400 W furnace fan, DSE=0.8), the duct zone receives 80 W more thermal energy than the telemetry reports. For typical residential systems this is small (fan power is ~2-5% of capacity), but it creates a bookkeeping discrepancy that could confuse debugging. The zone energy balance is correct; only the diagnostic telemetry key is affected.

### Finding 3: [Severity: medium]
**Description**: `thermal_output_w` telemetry includes basement-routed heat in the reported value, but the telemetry field description claims it represents "Delivered sensible heat to conditioned zone." When `basement_heat_frac > 0`, the conditioned zone receives only `total_sensible_w * dse * (1 - basement_frac)`, but `thermal_output_w` reports `total_sensible_w * dse`, which includes the portion routed to the basement.

**Code Location**:
- `furnace.rs:203` (ElectricFurnace) and `furnace.rs:520` (GasFurnace) -- `thermal_output_w = total_sensible_w * duct_dse`, no basement subtraction
- `furnace.rs:810-811` (electric furnace telemetry fields) and `furnace.rs:903-904` (gas furnace telemetry fields) -- field description: "Delivered sensible heat to conditioned zone after duct DSE"
- `duct_distribution.rs:44-45` -- conditioned zone receives `effective_dse * (1 - basement_frac)` of gross capacity

**Root Cause**: The `thermal_output_w` computation applies DSE to the total sensible output but does not subtract the basement-routed fraction (`dse * basement_frac`). The telemetry field description says "to conditioned zone" but the value includes basement delivery.

**Impact**: Telemetry consumers that compare `thermal_output_w` against the conditioned zone's thermal accumulator will find a discrepancy when basement routing is active. The mismatch is `total_sensible_w * dse * basement_frac`, which can be 20-30% of delivered heat for residential basements.

### Finding 4: [Severity: low]
**Description**: `apply_duct_dse()` helper in `duct_distribution.rs` uses the raw `self.config.duct_dse` directly without accounting for the effective DSE override when `duct_zone_id == zone_id`. While this helper is not called by the furnace `step()` methods (which delegate to `write_zone_thermal_contributions`), other HVAC equipment types or future code paths that call `apply_duct_dse()` would get an incorrect result.

**Code Location**:
- `duct_distribution.rs:133-136` -- `apply_duct_dse()` uses `self.config.duct_dse` raw value
- `duct_distribution.rs:22-41` -- `update_zone_heat_fractions()` computes `effective_dse` but does not store it in config

**Root Cause**: No single source of truth for the effective DSE. The value computed in `update_zone_heat_fractions()` is discarded after building `zone_heat_fractions`. OCHRE (HVAC.py:175-177) solves this by mutating `self.duct_dse` directly.

**Impact**: Minimal for the current furnace code path since furnaces use `write_zone_thermal_contributions` (which uses `zone_heat_fractions` based on effective DSE). However, `apply_duct_dse` returns the wrong value if called after `update_zone_heat_fractions` with `duct_zone_id == zone_id`.

### Finding 5: [Severity: low]
**Description**: When `duct_zone_id` is `None` (duct losses are unrecoverable, lost to outdoors) and DSE < 1.0, the `zone_heat_fractions` sum to less than 1.0, and `thermal_output_w` telemetry correctly reports a reduced value. However, the unrecovered portion (`total_sensible_w * (1 - dse)`) is silently discarded with no telemetry tracking, making it impossible to audit total energy disposition from telemetry alone.

**Code Location**:
- `duct_distribution.rs:57-65` -- duct zone entry only created when `duct_zone_id` is Some and differs from conditioned zone
- `furnace.rs:203-205` (ElectricFurnace) and `furnace.rs:520-522` (GasFurnace) -- `duct_loss_w` computed regardless, but has no corresponding zone when `duct_zone_id` is None
- OCHRE (HVAC.py:188-197) -- same behavior: when `duct_zone is None`, DSE losses are not added to any zone

**Root Cause**: Design choice consistent with OCHRE. When ducts are in unconditioned space with no modeled zone to receive losses, the losses are truly "lost" from the simulation. This is physically reasonable but creates an unrecoverable energy balance gap in telemetry.

**Impact**: When `duct_zone_id` is `None` and DSE < 1.0, `fuel_input_w + electrical_input` will not equal `thermal_output_w + duct_loss_w + other` in telemetry (because `duct_loss_w` has no zone sink). This is by design, but the energy balance cannot be closed from telemetry alone. The `duct_loss_w` telemetry key is still emitted but has no corresponding thermal accumulator in any zone.

## Summary
- **Total findings**: 5
- **Critical**: 0
- **High**: 1 (Finding 1 — telemetry vs routing mismatch when ducts in conditioned space)
- **Medium**: 2 (Findings 2 and 3 — fan heat omission in duct_loss_w, basement fraction in thermal_output_w)
- **Low**: 2 (Findings 4 and 5 — apply_duct_dse stale config, unrecoverable duct losses unlinked to zones)

## Assessment
The core thermal energy balance is **correct** — fuel consumption is independent of DSE, zone heat fractions sum to 1.0 (or less for unrecoverable losses) and properly partition gross capacity across conditioned, duct, and basement zones. No double-counting was found. The zone-level port accumulations receive the physically correct wattage.

However, the telemetry layer has several inconsistencies between what is routed to zones and what is reported via `CoreOutput` and telemetry keys. These findings affect diagnostics, energy balance auditing, and any higher-level system that consumes `thermal_output_w` or `duct_loss_w` as authoritative values.

## Recommendations
1. [Finding 1] Store the effective DSE back to `self.config.duct_dse` in `update_zone_heat_fractions()`, mirroring OCHRE's approach (HVAC.py:175-177). This ensures telemetry, `apply_duct_dse()`, and all downstream consumers use the corrected value.
2. [Finding 2] Compute `duct_loss_w` from `total_sensible_w * (1 - dse)` instead of `gross_capacity_w * (1 - dse)` in both furnace step methods, matching the actual zone-level thermal contribution that includes fan heat. Alternatively, explicitly document that `duct_loss_w` represents air-stream thermal loss only and not total duct zone deposition.
3. [Finding 3] Either (a) subtract the basement-routed fraction from `thermal_output_w` to match the telemetry field description, or (b) update the telemetry field description to clarify that the value represents total post-DSE delivered heat (including basement routing), and add a separate key for conditioned-zone-only delivery.
4. [Finding 4] Unify DSE access: either store effective DSE in config (per Recommendation 1) or have `apply_duct_dse()` use the same effective DSE logic as `update_zone_heat_fractions()`.
5. [Finding 5] Consider adding an `UNRECOVERABLE_DUCT_LOSS_W` telemetry key or logging the unrecovered duct loss when `duct_zone_id` is None, for energy balance auditing.

## References / Citations
- OCHRE HVAC.py lines 165-197: Duct DSE initialization and zone fraction computation
- OCHRE HVAC.py lines 175-177: Override of `self.duct_dse = 1` when ducts are in conditioned zone
- OCHRE HVAC.py lines 526-566: `calculate_power_and_heat()` and `add_gains_to_zone()` — thermal routing order
- OCHRE HVAC.py lines 568-604: `generate_results()` — telemetry computation using `self.duct_dse`
- ASHRAE Standard 152: Duct distribution system efficiency methodology
- ANSI/RESNET/ICC 301-2019: Default heating/cooling system parameters
