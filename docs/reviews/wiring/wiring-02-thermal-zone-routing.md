# Thermal contribution zone routing: every equipment output reaches correct zone
**Review ID**: wiring-02
**Category**: wiring
**Date**: 2026-05-26

## Files Reviewed
crates/hares-types/src/ports.rs crates/hares-core/src/dwelling/mod.rs crates/hares-envelope/src/thermal_solver/ports.rs

## Vendor/Reference Files Consulted
vendors/OCHRE/ochre/Equipment/Equipment.py vendors/OCHRE/ochre/Equipment/HVAC.py vendors/OCHRE/ochre/Equipment/WaterHeater.py

## Findings

### Finding 1: ZoneId(0) accepted as valid equipment zone input [Severity: medium]
**Description**: The `zone_id_from_config` helper in `crates/hares-equipment/src/hvac/helpers.rs:34-42` and `parse_zone_id_key` at line 45-51 validate zone ID values with `validate_u16_id`, which permits `raw == 0.0`. However, zone enumeration in `crates/hares-core/src/environment.rs:945` starts at `idx + 1`, meaning `ZoneId(0)` is never a valid thermal zone. If a user or HPXML resolver emits `zone_id = 0` in equipment config, the equipment will create a `ThermalAccumulator` for zone 0 via `PortSlots::from_declarations` (which creates accumulators for any declared zone). The thermal solver's `zone_sensible_input_indices` will have no entry for `ZoneId(0)`, so the thermal contribution is silently dropped (the `.get()` returns `None` and the loop continues in `ports.rs:16-20`). While this does not cause a crash or data corruption, it silently discards thermal output — a user would see heating equipment producing `hvac_heating_w = 0` in diagnostics without any error message.
**Code Location**: `crates/hares-equipment/src/hvac/helpers.rs:30-32` (`validate_u16_id`), `crates/hares-envelope/src/thermal_solver/ports.rs:16` (silent skip)
**Root Cause**: Input validation boundary accepts zone ID = 0 as syntactically valid when it is semantically invalid (zones are 1-indexed).
**Impact**: If zone_id=0 is configured, thermal output is silently lost with no runtime error. Mitigating factor: HPXML-based zone IDs come from the envelope builder where zone IDs are always ≥1, so this primarily affects raw-config or custom-equipment paths.

### Finding 2: Jacket loss diagnostic field `jacket_loss_w` reads only indoor zone accumulator [Severity: medium]
**Description**: The `EnvelopeComponentGains::jacket_loss_w` field in `crates/hares-envelope/src/thermal_solver/mod.rs:731-733` reads `sensible_for_category(ThermalCategory::JacketLoss)` from `indoor_acc` (the indoor zone's `ThermalAccumulator`). If a water heater is located in a non-indoor zone (e.g., a gas water heater in a garage or basement), its jacket losses are correctly routed to that zone's accumulator by `water_heater/gas.rs:522-527` and applied to the thermal solver state vector via `apply_port_convective_inputs`. However, the diagnostic output field `jacket_loss_w` reports 0 because the jacket losses are in the non-indoor zone's accumulator, not the indoor one. This makes it appear as though jacket losses are absent when they in fact exist and are correctly applied to the host zone's thermal load.
**Code Location**: `crates/hares-envelope/src/thermal_solver/mod.rs:731-733`, `crates/hares-envelope/src/thermal_solver/config.rs:627-629`
**Root Cause**: The `jacket_loss_w` diagnostic sums only the indoor zone's `JacketLoss` category rather than summing across all zone accumulators (the duct loss diagnostic at line 736-740 correctly sums across all zones).
**Impact**: Diagnostic/monitoring gap only — the thermal loads are correctly applied to the host zone through the state-space input vector. Output records and energy balance calculations miss jacket losses from equipment in unconditioned zones.

### Finding 3: Unrecoverable duct loss fraction has no telemetry or diagnostic output [Severity: low]
**Description**: In `crates/hares-equipment/src/hvac/duct_distribution.rs:57-65`, when `duct_zone_id` is `None` and `effective_dse < 1.0`, the unrecovered fraction `(1.0 - effective_dse)` is never assigned to any zone's `zone_heat_fractions`. The fractions sum to less than 1.0 (as documented in `hvac_core.rs:252` — "fractions sum to <= 1.0; the remainder is unrecoverable duct loss"). The `write_zone_thermal_contributions` method at line 94-131 only writes `fraction > 0.0` entries, so the unrecovered heat is silently discarded. While this is physically correct (heat lost to unconditioned/unmodeled outdoor space should not appear in any zone's thermal balance), there is no mechanism to track or report the magnitude of unrecovered duct losses. In OCHRE (`HVAC.py:587-588`), unrecovered duct losses are explicitly reported as "Duct Losses (W)" in results.
**Code Location**: `crates/hares-equipment/src/hvac/duct_distribution.rs:57-65`, `crates/hares-equipment/src/hvac/furnace.rs:521-522`
**Root Cause**: No telemetry field captures `gross_capacity * (1.0 - sum(zone_heat_fractions))` for diagnostic output.
**Impact**: Users cannot verify the magnitude of unrecovered duct losses from output alone. The thermal balance is correct — these losses are intentionally excluded from zone loading.

### Finding 4: Merged basement+duct zone loses HVAC heating category fidelity [Severity: low]
**Description**: When `basement_zone_id == duct_zone_id`, `deduplicate_zone_heat_fractions` in `duct_distribution.rs:74-88` merges the basement fraction and duct loss fraction into a single entry. In `write_zone_thermal_contributions` at line 114-120, the entire merged contribution is tagged as `ThermalCategory::DuctLoss` because the zone matches `duct_zone_id` and is not the conditioned zone. The basement portion of heating (which the conditioned space intentionally routes to a finished basement as `HvacHeating`) gets the wrong category tag. The test at `duct_distribution.rs:493-494` acknowledges this: "This is accepted behavior: the watts are physically correct regardless of category." The sensor output for basement HVAC heating would under-report and duct losses would over-report.
**Code Location**: `crates/hares-equipment/src/hvac/duct_distribution.rs:114-120`, `crates/hares-equipment/src/hvac/duct_distribution.rs:74-88`
**Root Cause**: Deduplication collapses two semantically distinct categories into one tuple, and the category assignment logic cannot distinguish the merged portions.
**Impact**: Per-category attribution in output columns is incorrect for the basement portion when basement zone == duct zone. The total watts are physically correct. This scenario is realistic for homes where the furnace and ducts are both in the basement.

## Summary
- Total findings: 4
- Critical: 0
- High: 0
- Medium: 2 (ZoneId(0) silently drops output; jacket loss diagnostic gap)
- Low: 2 (unrecoverable duct loss telemetry gap; basement+duct category merge)

## Correctness verification
The following aspects of the thermal routing pipeline were verified and found correct:

- **No off-by-one zone indexing**: `ZoneId` is a `u16` wrapper, not an index. Zone-to-index mapping is consistent across the solver builder, thermal solver, and dwelling output. Zones on each path are identified by ID lookup rather than positional array indexing.
- **No equipment silently writes to zone 0 when zone is None**: Every equipment type checks `if let Some(zone) = self.descriptor.zone` (event loads at `event_load.rs:409`, water heaters at `resistance.rs:552`, `gas.rs:522`, `heat_pump_wh.rs:744`, etc.) before writing thermal contributions. Zone=None equipment (PV, generators, EV, outdoor equipment) correctly emits no thermal output.
- **No equipment writes to zone index larger than number of zones**: `PortSlots::accumulate` at `ports.rs:517-526` returns `Err(HaresError::Equipment("undeclared thermal zone: ..."))` for any contribution targeting a zone not declared in `PortSlots::from_declarations`. The test at `ports.rs:898-907` confirms this rejection.
- **Duct loss routing is correct**: When `duct_zone_id` is set and differs from the conditioned zone, losses are deposited into the duct zone's accumulator tagged as `ThermalCategory::DuctLoss`. When ducts are inside the conditioned space (`duct_zone == zone_id`), DSE is effectively set to 1.0. The thermal solver correctly sums `DuctLoss` across all zone accumulators (not just the indoor one) at `thermal_solver/mod.rs:736-740`.
- **Water heater jacket loss routing is correct**: All three water heater types (resistance, gas, HPWH) write `skin_loss_w` (or compressor wall waste heat) to `self.descriptor.zone` tagged as `ThermalCategory::JacketLoss`, with the zone guard (`if let Some(zone) = ...`) preventing writes when no zone is assigned. The OCHRE reference (`WaterHeater.py:299-300`) follows the same pattern: "heat losses from tank are added to sensible gains."
- **Unconditioned space equipment routing is correct**: Equipment in unconditioned zones (water heater in garage, ducts in attic) correctly writes to the unconditioned zone's accumulator (which exists because the equipment declares a thermal port for that zone). The thermal solver's `apply_port_convective_inputs` and `apply_port_radiant_inputs` iterate ALL zone accumulators, including unconditioned zones. The OCHRE documentation note at `Equipment.py:79-80` confirms this approach: "FUTURE: separate convection and radiation, move radiation gains to the surfaces around the zone."

## Recommendations

1. Tighten `validate_u16_id` to reject `raw < 1.0` (or add a separate `validate_zone_id` check in `zone_id_from_config`), and emit a warning when `ZoneId(0)` appears in config. Zone IDs are always ≥ 1.

2. Change the `jacket_loss_w` diagnostic in `thermal_solver/mod.rs:731` to sum `JacketLoss` across all zone accumulators (matching the `duct_loss_w` pattern at line 736), or add a parallel per-zone jacket loss field.

3. Add a telemetry field on HVAC equipment (e.g. `duct_unrecovered_loss_w`) that captures `gross_capacity_w * max(0.0, 1.0 - sum(zone_heat_fractions))`, matching the OCHRE pattern of explicitly reporting duct losses.

4. When basement_zone == duct_zone, split the merged fraction into two separate `PortContribution` writes with correct categories (basement fraction → `HvacHeating`, duct loss fraction → `DuctLoss`) instead of merging into a single entry.

## References / Citations
- OCHRE `Equipment.py:79-84` — sensible/latent/radiant gain fraction framework and zone-to-surface routing note
- OCHRE `Equipment.py:188-198` — `add_gains_to_zone` method: checks `self.zone is None`, then adds `sensible_gain` and `latent_gain` to `zone.internal_sens_gain`
- OCHRE `HVAC.py:165-178` — `duct_dse`, `duct_zone`, and multi-zone heat fraction pattern
- OCHRE `HVAC.py:587-588` — explicit "Duct Losses (W)" output reporting
- OCHRE `WaterHeater.py:285-300` — sensible gain calculation from tank heating losses: "heat losses from tank are added to sensible gains"
- OCHRE `WaterHeater.py:678-685` — HPWH `add_gains_to_zone` splits gains to zone and interior walls
- EnergyPlus Engineering Reference, "Zone Internal Gains" — TMULT radiant distribution method
- ASHRAE HoF 2021 Ch. 18 §2 — convective and radiant fractions of internal gains belong to host zone
