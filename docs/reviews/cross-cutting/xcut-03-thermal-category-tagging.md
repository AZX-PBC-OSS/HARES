# ThermalCategory tagging: HvacHeating, HvacCooling, DuctLoss, JacketLoss, InternalGain correctly set
**Review ID**: xcut-03
**Category**: cross-cutting
**Date**: 2026-05-26

## Files Reviewed
crates/hares-types/src/ports.rs

## Additional Files Inspected
- `crates/hares-equipment/src/hvac/duct_distribution.rs` — duct-loss auto-tagging logic in `write_zone_thermal_contributions`
- `crates/hares-equipment/src/hvac/furnace.rs` — `HvacHeating` emission at line 511
- `crates/hares-equipment/src/hvac/air_conditioner.rs` — `HvacCooling` emission at line 807
- `crates/hares-equipment/src/hvac/ideal_hvac.rs` — direct `HvacHeating`/`HvacCooling` at lines 558/563
- `crates/hares-equipment/src/hvac/heat_pump/heater.rs` — `HvacHeating` emission at line 1019
- `crates/hares-equipment/src/hvac/baseboard.rs` — `HvacHeating` emission at line 154
- `crates/hares-equipment/src/hvac/boiler.rs` — `HvacHeating` at line 557; `JacketLoss` at line 573
- `crates/hares-equipment/src/water_heater/resistance.rs` — `JacketLoss` at line 559
- `crates/hares-equipment/src/water_heater/gas.rs` — `JacketLoss` at line 529
- `crates/hares-equipment/src/water_heater/heat_pump_wh.rs` — `HvacDehumidification` at line 758; `JacketLoss` at lines 773, 786
- `crates/hares-equipment/src/hvac/dehumidifier.rs` — `HvacDehumidification` at line 367
- `crates/hares-equipment/src/scheduled_load.rs` — `InternalGain` at line 545
- `crates/hares-equipment/src/event_load.rs` — `InternalGain` at line 417
- `crates/hares-equipment/src/battery/mod.rs` — `InternalGain` at line 1002
- `crates/hares-equipment/src/generator.rs` — `InternalGain` at line 800
- `crates/hares-core/src/dwelling/mod.rs` — `InternalGain` at line 2127 (occupancy, bypasses `PortSlots::accumulate`)
- `crates/hares-envelope/src/thermal_solver/mod.rs` — category consumption at lines 718–744; solar/infiltration at lines 639–716

## Vendor/Reference Files Consulted
None

## Findings

### Finding 1: Solar gains and infiltration are not registered in ThermalCategory [Severity: medium]
**Description**: The `ThermalCategory` enum (`ports.rs:18–53`) has no `SolarGain` or `Infiltration` variant. Solar gains (window transmitted and opaque surface absorbed) and infiltration thermal exchange are computed directly inside `thermal_solver/mod.rs` by modifying the solver input vector `u` and recording totals in `EnvelopeComponentGains` fields (`window_solar_w`, `opaque_solar_w`, `infiltration_w`, `ventilation_w`, `combined_airflow_sensible_w`). No `PortContribution::Thermal` is emitted for either source.  
**Code Location**: `crates/hares-types/src/ports.rs:18–53` (enum definition); `crates/hares-envelope/src/thermal_solver/mod.rs:639–716` (solar, infiltration computation bypasses ports)  
**Root Cause**: Envelope solver predates the ThermalCategory port system. Solar and infiltration were built into the solver directly rather than routed through equipment ports.  
**Impact**: Solar gains and infiltration cannot be disaggregated in the same per-category telemetry as HVAC and internal gains. For retrofit analysis where infiltration can be 20–40% of total heating/cooling load and solar gains have distinct temporal patterns, this limits diagnostic usefulness. The `EnvelopeComponentGains` struct captures these values separately, but they are not surfaced through the `ThermalAccumulator` per-category arrays.

### Finding 2: Jacket losses to conditioned space are tagged JacketLoss, not InternalGain [Severity: low]
**Description**: All water heater tank skin losses and boiler jacket losses emit `ThermalCategory::JacketLoss` regardless of whether the equipment's zone is conditioned or unconditioned. The review criterion states jacket losses to conditioned space should be tagged `InternalGain` ("they reduce the heating load"), but the codebase uniformly uses `JacketLoss`.  
**Code Location**: `crates/hares-equipment/src/hvac/boiler.rs:562–575` (no check for conditioned vs unconditioned zone); `crates/hares-equipment/src/water_heater/resistance.rs:552–561`; `crates/hares-equipment/src/water_heater/gas.rs:522–531`; `crates/hares-equipment/src/water_heater/heat_pump_wh.rs:779–788`  
**Root Cause**: The `ThermalCategory` documentation at `ports.rs:26` defines `JacketLoss` as "Equipment shell/jacket losses (water heaters, boilers)" without distinguishing conditioned from unconditioned space.  
**Impact**: When equipment is inside a conditioned zone, jacket losses reduce the heating load (same net thermal effect as `InternalGain`), but appear as `JacketLoss` in diagnostics. This conflates true envelope losses (tank in unconditioned garage) with "free" heat that offsets HVAC demand. The thermal solver at `thermal_solver/mod.rs:731–733` correctly adds `jacket_loss_w` as a positive sensible gain to the zone air, so the net heat balance is physically correct; the issue is purely diagnostic/telemetry accuracy.

### Finding 3: HPWH compressor wall waste heat conflated with tank JacketLoss [Severity: low]
**Description**: The HPWH emits two separate `JacketLoss` contributions: compressor wall-fraction waste heat (`sensible_to_wall_w`, line 773) and tank skin conduction (`skin_loss_w`, line 786). These are distinct physical phenomena (compressor cycle charge-pipe losses vs passive tank shell losses) with different driving variables, but share one `ThermalCategory`.  
**Code Location**: `crates/hares-equipment/src/water_heater/heat_pump_wh.rs:761–773` (compressor wall heat), `:778–788` (tank skin loss). Both use `ThermalCategory::JacketLoss`.  
**Root Cause**: Acknowledged in code comment at lines 767–772: "A dedicated HvacWasteHeat variant would allow full separation of HP-cycle losses from tank losses."  
**Impact**: Per-category diagnostics cannot distinguish tank losses from compressor waste heat. Users must cross-reference equipment telemetry fields (`skin_loss_w`, `wall_sensible_gain_w`) to disaggregate.

### Finding 4: Basement delivered heat miscategorized as DuctLoss when basement == duct zone [Severity: low]
**Description**: When `basement_zone_id == duct_zone_id`, the heat fractions for basement delivery and duct loss are merged by `deduplicate_zone_heat_fractions()` into a single entry. `write_zone_thermal_contributions()` then tags the entire merged amount as `DuctLoss` because the zone matches `duct_zone_id` and differs from the conditioned zone (`duct_distribution.rs:114–116`). The basement-delivered capacity portion should remain tagged as the caller's category (e.g. `HvacHeating`), but gets conflated.  
**Code Location**: `crates/hares-equipment/src/hvac/duct_distribution.rs:72–88` (deduplication), `:109–120` (tagging logic). Test at lines 494–548 explicitly accepts this as "the watts are physically correct regardless of category."  
**Root Cause**: Inability to tag different fractions of a single zone's heat entry with different categories. The zone-based port system uses one category per zone per contribution, but when two conceptual flows (basement heat + duct loss) target the same zone, they are merged first, then tagged with only one category.  
**Impact**: Telemetry misclassifies a portion of useful basement heating as duct loss. The total watts delivered are physically correct.

### Finding 5: Occupancy gains bypass PortSlots::accumulate() [Severity: low]
**Description**: Occupancy thermal gains (`apply_occupancy_gains`) write directly to `ThermalAccumulator::add()` instead of going through `PortSlots::accumulate()`. While the category (`InternalGain`) is correctly set, this bypasses the undeclared-zone guard in `accumulate()` (`ports.rs:526–532`), which would return an error if the zone accumulator had not been pre-allocated via `from_declarations()`.  
**Code Location**: `crates/hares-core/src/dwelling/mod.rs:2116–2129`  
**Root Cause**: Architectural shortcut — the dwelling-level occupancy model predates the port conformance model. This is a registered finding in a separate architectural review (`arch-04-port-slot-accumulation`).  
**Impact**: If the indoor zone thermal accumulator is not pre-declared (a configuration error), occupancy gains are silently discarded instead of triggering an error. In normal operation, `build_ports()` at dwelling init ensures the indoor zone accumulator exists, so the pragmatic risk is low.

### Finding 6: `#[default]` on InternalGain enables silent misclassification [Severity: low]
**Description**: `ThermalCategory` derives `Default` with `#[default] InternalGain` (`ports.rs:24`). The `category` field in `PortContribution::Thermal` is required (not `Option`), so compile-time safety prevents `None`. However, if any code ever constructs a `ThermalCategory` via `Default::default()`, it silently becomes `InternalGain`.  
**Code Location**: `crates/hares-types/src/ports.rs:23–25`  
**Root Cause**: A default is needed for array initialization, but `InternalGain` was chosen. While `InternalGain` is the "safest" default (it doesn't mislead about HVAC operation), it means an accidental `..Default::default()` where a more specific category was intended would silently classify HVAC output as internal gain.  
**Impact**: Currently no known misuse in the codebase — all equipment explicitly provides a category. The risk is at the API boundary if external equipment plugins fail to specify a category.

## Positive Confirmations (No Issues Found)

### Confirmed: HvacHeating tagging is correct and consistent
All heating equipment (furnace, heat pump heater, boiler, baseboard, ideal HVAC in heating mode) emits `ThermalCategory::HvacHeating`. Auxiliary/backup heating routes through the same equipment step methods and uses the same category. No separate "backup heat" category exists.

### Confirmed: HvacCooling sign convention is consistent
Cooling contributions use negative `sensible_gain_w` (heat removed from zone) with `ThermalCategory::HvacCooling`. Air conditioner (`air_conditioner.rs:805`): `-sensible_cooling_w + fan_heat_w`. Ideal HVAC (`ideal_hvac.rs:563`): `capacity_w * self.shr` (negative capacity, negative sensible). Fan heat offsets are added as positive values. This aligns with EnergyPlus convention where cooling is negative heat addition.

### Confirmed: DuctLoss correctly distinguished from delivered heat
`write_zone_thermal_contributions()` in `duct_distribution.rs:94–131` correctly tags the duct-zone fraction as `DuctLoss` while preserving the original category for the conditioned zone. When ducts are in conditioned space (`duct_zone_id == zone_id`), `update_zone_heat_fractions()` forces `effective_dse = 1.0` so no duct-zone entry is created, and all capacity stays tagged with the original category. Duct losses to unconditioned space are correctly identified.

### Confirmed: HvacDehumidification correctly separated from InternalGain
HPWH compressor zone-air heat (sensible + latent) and standalone dehumidifier output use `ThermalCategory::HvacDehumidification`. This matches EnergyPlus Engineering Reference classification and correctly separates mechanical dehumidification from passive internal gains.

### Confirmed: Sensible/latent split preserved
The `PortContribution::Thermal` variant carries separate `sensible_gain_w`, `radiant_gain_w`, and `latent_gain_w` fields. `ThermalAccumulator` stores per-category sub-totals for all three components via `sensible_by_category`, `radiant_by_category`, and `latent_by_category` arrays. Equipment that produces latent effects (air conditioner, HPWH compressor zone output, dehumidifier) correctly populates the latent field.

### Confirmed: Category naming is consistent
All six `ThermalCategory` variants use PascalCase descriptive names with Rust doc comments explaining expected usage (`ports.rs:18–34`). No inconsistency found. The `index()` method is kept in sync with declaration order.

### Confirmed: Compile-time enforcement of category presence
The `category` field in `PortContribution::Thermal` is a required non-optional `ThermalCategory` (`ports.rs:66`). It is impossible to construct a `PortContribution::Thermal` without specifying a category — the Rust compiler enforces this.

## Summary
- Total findings: 6
- Critical: 0
- High: 0
- Medium: 1
- Low: 5

## Recommendations
1. **Add `SolarGain` and `Infiltration` variants to `ThermalCategory`** (Medium). This allows the per-category telemetry system to capture all major thermal load components. Implementation may require the envelope solver to emit `PortContribution::Thermal` for solar and infiltration, or at minimum populate `ThermalAccumulator` arrays directly from the solver's own computed values so they appear in the same output structure.
2. **Introduce a `JacketLossToConditioned` variant or route conditioned-space jacket losses as `InternalGain`** (Low). This prevents misleading diagnostics where equipment in conditioned space appears to "lose" heat that actually offsets heating demand. The `JacketLoss` variant should be reserved for losses to truly unconditioned buffer spaces.
3. **Consider a `HvacWasteHeat` variant** (Low). As noted in the HPWH code comments, a dedicated variant for compressor/pump waste heat routed to interior walls would allow full separation from passive tank skin losses.
4. **Split basement delivered heat and duct loss when they target the same zone** (Low). Instead of relying on deduplication before tagging, emit two separate `PortContribution::Thermal` entries at `write_zone_thermal_contributions` level — one for basement delivery (original category), one for duct loss (`DuctLoss`) — when `basement_zone_id == duct_zone_id`.
5. **Route occupancy gains through `PortSlots::accumulate()` instead of `ThermalAccumulator::add()`** (Low). This ensures undeclared-zone validation applies uniformly and removes a known architectural bypass.
6. **Consider removing `#[default]` from `InternalGain`** (Low). If a default is required for array initialization, prefer a method that doesn't allow implicit category assignment, or use a lint to flag any `ThermalCategory::default()` usage at the equipment boundary.

## References / Citations
- EnergyPlus Engineering Reference, "Zone Equipment and Zone Forced Air Units" — classification of dehumidifier as zone HVAC equipment (not internal gain)
- OCHRE HVAC.py lines 188–197 — zone heat fraction routing (upstream reference for duct distribution)
- OCHRE HVAC.py line 543 — fan heat waste folded into delivered capacity
- OCHRE WaterHeater.py — internal_sens_gain routing (upstream reference for HPWH zone-air heat, separated to HvacDehumidification in HARES)
- `crates/hares-types/src/ports.rs:18–53` — `ThermalCategory` enum definition and `index()` method
- `crates/hares-types/src/ports.rs:57–91` — `PortContribution` enum with `category: ThermalCategory` in `Thermal` variant
- `crates/hares-types/src/ports.rs:176–252` — `ThermalAccumulator` with `sensible_by_category` / `radiant_by_category` / `latent_by_category` arrays
- `crates/hares-equipment/src/hvac/duct_distribution.rs:90–131` — `write_zone_thermal_contributions()` auto-tags duct losses
- `crates/hares-envelope/src/thermal_solver/mod.rs:639–716` — solar/infiltration bypass port system; category consumption at lines 718–744
