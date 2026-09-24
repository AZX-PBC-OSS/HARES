# Duct heat fraction basement routing: deduplication of duct zone heat injection
**Review ID**: equip-hvac-07
**Category**: equipment-hvac
**Date**: 2026-05-26

## Files Reviewed
crates/hares-equipment/src/hvac/duct_distribution.rs crates/hares-equipment/src/hvac/furnace.rs crates/hares-equipment/src/hvac/heat_pump.rs

## Vendor/Reference Files Consulted
vendors/OCHRE/ochre/Equipment/HVAC.py

## Findings
### Finding 1: [Severity: high]
**Description**: Energy conservation violation when `basement_heat_frac > 0` but `basement_zone_id` is `None`. The conditioned-zone fraction is reduced by `effective_dse * (1 - basement_frac)`, but the basement portion (`effective_dse * basement_frac`) is silently dropped because the `if let Some(basement_zone)` guard on `basement_zone_id` (line 48) has no `else` branch. Fractions no longer sum to 1.0, resulting in a net deletion of heat from the simulation.

**Code Location**: `duct_distribution.rs:42-55`

**Root Cause**: `basement_heat_frac` and `basement_zone_id` are read from independent config keys (`basement_airflow_ratio` and `basement_zone_id`) in `hvac_core.rs:626-632` with no cross-validation. When the fraction is positive but the zone is absent, the energy is lost. This diverges from OCHRE (HVAC.py lines 181-186), which only sets a positive `basement_heat_frac` when a Foundation zone exists, and would set `basement_heat_frac = 0` otherwise.

**Impact**: In the worst case (e.g., `duct_dse = 0.70`, `basement_heat_frac = 0.30`), `0.70 * 0.30 = 21%` of gross capacity vanishes silently. No warning or error is emitted.

### Finding 2: [Severity: medium]
**Description**: When `basement_zone == duct_zone`, the merged entry in `write_zone_thermal_contributions` is tagged entirely as `ThermalCategory::DuctLoss` even though a portion of the merged wattage originated from the basement heat fraction (which represents intentionally routed heated air, not a duct conduction loss). The category tagging is lossy after deduplication.

**Code Location**: `duct_distribution.rs:113-120` (category logic), `duct_distribution.rs:69` (deduplication call)

**Root Cause**: After `deduplicate_zone_heat_fractions()` merges basement and duct fractions into a single entry, `write_zone_thermal_contributions` compares the zone against `duct_zone_id` to assign `DuctLoss` vs the caller-supplied category. Since the merged zone matches `duct_zone_id` and is not the conditioned zone, the entire merged fraction — including the basement portion — gets tagged `DuctLoss`.

**Impact**: Diagnostic/telemetry category mislabeling only. The energy balance is numerically correct (total watts are preserved). The test at `duct_distribution.rs:490-548` acknowledges and accepts this behavior.

### Finding 3: [Severity: low]
**Description**: `DUCT_LOSS_W` telemetry excludes the fan-heat portion routed to the duct zone. In all three equipment types, the duct loss telemetry is computed as `gross_capacity * (1 - dse)`, but the actual thermal injection distributed via `write_zone_thermal_contributions` uses `total_sensible_w` (which includes fan heat). Consequently, the duct zone receives more thermal energy than `DUCT_LOSS_W` reports by `fan_heat_w * (1 - dse)`.

**Code Location**:
- `furnace.rs:205` — `duct_loss_w = gross_capacity_w * (1.0 - dse)` but injection uses `total_sensible_w` (line 188-193)
- `heater.rs:1076-1077` — same pattern, telemetry vs step.thermal_output_w
- `air_conditioner.rs:861` — `duct_loss_w = gross_cooling_w * (1.0 - dse)` but injection at line 805 includes `fan_heat_w`

**Root Cause**: Fan heat is included in the thermal contribution (`write_zone_thermal_contributions`) following OCHRE convention (HVAC.py:543 includes fan_power in delivered_heat), but the telemetry key `DUCT_LOSS_W` was defined to track only the ASHRAE 152 duct conduction loss from rated capacity, not the full duct contribution including fan heat.

**Impact**: Diagnostic inconsistency only. The thermal solver receives the correct (higher) amount of energy in the duct zone. The `DUCT_LOSS_W` telemetry under-reports by a small amount — typically a few percent of gross capacity (e.g., 400 W fan at DSE=0.8 → 80 W under-reported).

### Finding 4: [Severity: low]
**Description**: The duct-to-zone mapping relies on fragile equality comparisons between `ZoneId` values across three independently configured fields (`zone_id`, `duct_zone_id`, `basement_zone_id`). If config parsing is off-by-one or the zone numbering convention shifts (e.g., conditioned zone ID changes from 1 to 0), the guard conditions at lines 28, 49, and 59 silently route heat to wrong zones rather than raising an error.

**Code Location**: `duct_distribution.rs:28,49,59`

**Root Cause**: Zone identity checks use direct equality (`==`) between `ZoneId` values without a validation step that ensures all three are distinct when they should be. A misconfiguration (e.g., `duct_zone_id = 1, zone_id = 1, basement_zone_id = 1`) would force `effective_dse = 1.0` (line 39), which is correct behavior per OCHRE, but other misconfigurations could produce silent routing errors.

**Impact**: Low likelihood (requires HPXML config error), but the resulting energy routing errors would be hard to diagnose without explicit warnings.

## Summary
- Total findings: 4
- Critical / High / Medium / Low: 0 / 1 / 1 / 2

## Recommendations
1. **Add a warning when `basement_heat_frac > 0` but `basement_zone_id` is None.** Either emit a `tracing::warn!` and clamp the fraction to 0, or default to routing the basement portion to the conditioned zone (treating basement as part of the same thermal zone), so fractions always sum to 1.0. This brings HARES in line with OCHRE's guard (HVAC.py:181-186) where positive basement fraction is only set when a Foundation zone exists.

2. **Consider tracking separate `BasementHeat` vs `DuctLoss` thermal categories** when `basement_zone == duct_zone`. The deduplication logic could retain a per-zone category tag (e.g., store a `Vec<(ZoneId, f64, ThermalCategory)>` instead of `Vec<(ZoneId, f64)>`) or split the zone entry into two contributions with distinct categories. This would improve diagnostic fidelity in the common "ducts in finished basement" configuration.

3. **Add a consistency check during `init()`** that verifies `basement_zone_id` and `duct_zone_id` are distinct from `zone_id` when they're configured. A `tracing::warn!` for each equality case would make misconfigurations immediately visible.

## References / Citations
- OCHRE HVAC.py lines 165-197: duct DSE initialization and zone fraction computation (dict-based, equivalent to HARES vector + dedup)
- OCHRE HVAC.py lines 181-186: guard that only sets positive `basement_heat_frac` when Foundation zone exists
- OCHRE HVAC.py line 543: `delivered_heat = heat_gain * shr + fan_power` — fan heat included in thermal delivery and thus subject to DSE-based zone distribution
- OCHRE HVAC.py lines 563-566: `add_gains_to_zone` distributes `sensible_gain` (which includes fan_power) using zone_fractions
- ASHRAE 152: duct DSE is a combined supply+return efficiency factor; single-value representation is appropriate for this model resolution
- HARES `duct_distribution.rs:22-70`: zone fraction computation and deduplication
- HARES `duct_distribution.rs:69`: `deduplicate_zone_heat_fractions` call site
- HARES `hvac_core.rs:626-632`: independent loading of `basement_zone_id` and `basement_heat_frac` from config
