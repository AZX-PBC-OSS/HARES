# Port slot accumulation: declaration vs runtime, zone indexing, category tagging
**Review ID**: arch-04
**Category**: architecture
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-types/src/ports.rs`
- `crates/hares-core/src/dwelling/mod.rs`

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Dwelling.py`
- `vendors/OCHRE/ochre/Equipment/Equipment.py`
- `vendors/OCHRE/ochre/Models/Envelope.py`

## Findings

### Finding 1: [Severity: medium]
**Description**: `PortSlots::from_declarations()` weakens the declarative wiring model by unconditionally creating thermal accumulators for every zone in `initial_env.zones`, regardless of whether any equipment has declared a thermal port for that zone. This means the runtime check at `PortSlots::accumulate()` (line 526–531) only rejects contributions to truly unknown `ZoneId` values, not to zones that exist in the environment but are undeclared by equipment.
**Code Location**: `crates/hares-core/src/dwelling/mod.rs:1148–1157`
**Root Cause**: After collecting equipment port declarations, the loop at lines 1148–1156 pushes a `PortDeclaration::thermal` for every zone in `initial_env.zones`. This is a deliberate defense-in-depth measure (preventing the thermal solver from reading zero-gain accumulators that don't exist), but it eliminates the wire-to-slot safety check for any zone that exists in the environment model.
**Impact**: Misconfigured equipment that targets the wrong zone will silently succeed rather than error. For example, if an HVAC unit's port declaration misspecifies its conditioned zone, the accumulator is guaranteed to exist (because the env zone list includes all zones), so `accumulate()` succeeds and contributions go to the wrong zone without any validation error. OCHRE avoids this class of error entirely because equipment references zone objects by name (`self.zone = envelope_model.zones.get(self.zone_name)` at `Equipment.py:54`), not by opaque ID.

### Finding 2: [Severity: medium]
**Description**: `apply_occupancy_gains()` bypasses `PortSlots::accumulate()` and writes directly to `ports.thermal` via `find()`, silently dropping gains when no thermal accumulator matches `indoor_zone`.
**Code Location**: `crates/hares-core/src/dwelling/mod.rs:2117–2129`
**Root Cause**: The method uses `self.ports.thermal.iter_mut().find(|t| t.zone == indoor_zone)` directly (line 2117) instead of calling `self.ports.accumulate(&PortContribution::Thermal { zone: indoor_zone, ... })`. If `indoor_zone` somehow has no accumulator — e.g., the zone was added after port construction but before warm-up convergence — the gains are silently discarded via the `if let Some(..)` pattern with no error or warning. A mismatch between `indoor_zone` and the port accumulator set should be an invariant error.
**Impact**: Occupant heat gains could be silently zero during simulation. Because this is the indoor zone and construction guarantees it exists in `initial_env.zones`, the risk is low in practice, but the inconsistency in the API usage (direct access vs. `accumulate()`) is a maintenance hazard.

### Finding 3: [Severity: low]
**Description**: `refresh_equipment_caches()` does not rebuild `PortSlots` accumulators, so dynamically added equipment that declares new zones/loops/domains will fail at runtime when its `step()` method calls `ports.accumulate()` with a previously undeclared target.
**Code Location**: `crates/hares-core/src/dwelling/mod.rs:1684–1781`
**Root Cause**: The `add_equipment()`, `clear_equipment()`, and `replace_equipment()` methods all call `refresh_equipment_caches()` (lines 1637, 1643, 1674), which rebuilds execution order, dispatch targets, output schema, and zone caches — but never calls `PortSlots::from_declarations()` to rebuild accumulators. Any new equipment targeting a zone that happens to already be in the env (Finding 1) will work by coincidence. Equipment targeting a new `LoopId` or `DomainId` will always fail.
**Impact**: Runtime failures for dynamically added equipment targeting new domains. This is a known gap in the dynamic-equipment API and is unlikely to surface in static HPXML-based simulations, but blocks test frameworks that add equipment programmatically.

### Finding 4: [Severity: low]
**Description**: HPWH compressor waste heat routed to an interior wall face is tagged `ThermalCategory::JacketLoss`, conflating HP-cycle losses with tank skin conduction. The total `JacketLoss` sensible value cannot distinguish between compressor waste heat and genuine standby losses.
**Code Location**: `crates/hares-equipment/src/water_heater/heat_pump_wh.rs:761–774`
**Root Cause**: The HPWH adds two separate thermal contributions for zone air (`HvacDehumidification`, line 758) and wall surface (`JacketLoss`, line 773). Tank skin losses at line 781 also use `JacketLoss`. The code comment at lines 768–773 explicitly notes this limitation and proposes a future `HvacWasteHeat` ThermalCategory variant. This is a design choice, not a defect, but the reviewer was asked to audit category tagging correctness.
**Impact**: Per-category diagnostics cannot disaggregate compressor waste heat from tank standby losses. The thermal solver correctly sums them (both are raw Watts), so energy balance is unaffected; only diagnostic attribution is conflated.

### Finding 5: [Severity: medium] — OCHRE parity: zone-gain model differs in granularity
**Description**: OCHRE accumulates equipment heat gains into only two per-zone buckets: `internal_sens_gain` (non-HVAC) and `hvac_sens_gain` (HVAC), plus `internal_latent_gain` and `hvac_latent_gain` (`Envelope.py:427–430`). HARES classifies contributions into six `ThermalCategory` variants. The HARES model provides finer attribution but introduces a risk: the OCHRE enrichment loop folds *all* non-HVAC heat (occupancy, lighting, appliances, jacket losses) into a single `internal_sens_gain` pool that the solver reads directly; HARES must verify that the thermal solver reads all six category subtotals and sums them correctly for the zone air energy balance.
**Code Location**:  
- OCHRE: `vendors/OCHRE/ochre/Models/Envelope.py:427–430, 1274, 1290`  
- HARES: `crates/hares-envelope/src/thermal_solver/mod.rs:698–743` (confirmed: solver reads all six categories correctly, summing `HvacHeating`, `HvacCooling`, `InternalGain`, `JacketLoss`, `DuctLoss`, and `HvacDehumidification` from the per-category arrays)
**Impact**: The HARES thermal solver correctly reads all six categories at `thermal_solver/mod.rs:698–743`, so the energy balance is preserved. This is noted for completeness in the OCHRE parity comparison but is not a defect.

### Finding 6: [Severity: low]
**Description**: No explicit validation at construction time that equipment-declared zone IDs reference zones that exist in the building/environment model. If equipment declares a thermal port for `ZoneId(999)` and that zone is absent from `initial_env.zones`, `from_declarations()` creates the accumulator (it's in the equipment port declarations list) and runtime writes to it will succeed — but the thermal solver has no corresponding zone and the contribution is silently orphaned.
**Code Location**: `crates/hares-core/src/dwelling/mod.rs:1144–1157` and `crates/hares-types/src/ports.rs:441–498`
**Root Cause**: `from_declarations()` does not cross-reference declared zones against `initial_env.zones`. It accepts any `ZoneId` from any source.
**Impact**: Potential silent accumulation to orphaned zones with no effect on the simulation. Low severity because HPXML-derived ZoneId values are validated by the HPXML parser; this would only trigger with synthetic TOML configs or manual equipment construction.

## Verification

### Declarative wiring prevents accumulation to undeclared slots
**Confirmed**: `PortSlots::accumulate()` (lines 517–597) returns `Err(HaresError::Equipment(...))` for thermal zones, fluid loops, custom domains, and humidity zones that have no accumulator. Tests at `ports.rs:898–937` validate all four port types. The protection is weakened by Finding 1 (all env zones get accumulators), but the mechanics work correctly for truly unknown identifiers.

### ThermalCategory tagging audit — equipment by equipment

| Equipment | Category Used | Correct? | Notes |
|-----------|--------------|----------|-------|
| IdealHvac heating | `HvacHeating` | Yes | `ideal_hvac.rs:563` |
| IdealHvac cooling | `HvacCooling` | Yes | `ideal_hvac.rs:563` |
| HeatPumpHeater | `HvacHeating` | Yes | `heater.rs:1020` |
| AirConditioner | `HvacCooling` | Yes | `air_conditioner.rs:807` |
| Baseboard | `HvacHeating` | Yes | `baseboard.rs:154` |
| Furnace | `HvacHeating` | Yes | `furnace.rs:195` |
| Boiler (zone thermal) | `HvacHeating` | Yes | `boiler.rs:253,557` |
| Boiler jacket loss | `JacketLoss` | Yes | `boiler.rs:573` |
| Duct distribution (to duct zone) | `DuctLoss` | Yes | `duct_distribution.rs:117` |
| Duct distribution (to conditioned) | passed-through | Yes | Inherits caller's category (`duct_distribution.rs:119`) |
| Resist. WH jacket loss | `JacketLoss` | Yes | `resistance.rs:559` |
| Gas WH jacket loss | `JacketLoss` | Yes | `gas.rs:529` |
| HPWH evaporator sensible | `HvacDehumidification` | Yes | `heat_pump_wh.rs:758` |
| HPWH wall heat fraction | `JacketLoss` | *Conflated* | See Finding 4; physically correct total, diagnostically conflated |
| HPWH skin loss | `JacketLoss` | Yes | `heat_pump_wh.rs:786` |
| Tankless WH | N/A (no thermal port) | N/A | Design intent: tankless has negligible standby loss |
| ScheduledLoad | `InternalGain` | Yes | `scheduled_load.rs:545` |
| EventLoad | `InternalGain` | Yes | `event_load.rs:417` |
| Battery (ohmic losses) | `InternalGain` | Yes | `battery/mod.rs:1002` |
| Generator (waste heat) | `InternalGain` | Yes | `generator.rs:800` |
| Dehumidifier | `HvacDehumidification` | Yes | `dehumidifier.rs:367` |
| Occupancy (direct) | `InternalGain` | Yes | `dwelling/mod.rs:2127` |

### Multi-equipment accumulation to same zone
**Confirmed**: The `ThermalAccumulator::add()` method (lines 214–227) sums contributions from multiple sources into per-category arrays. Each `equipment.step()` call writes to `ports` in sequence, and all writes accumulate correctly. The `zero()` method (lines 229–236) resets the accumulator at the end of each timestep (line 2810). Multi-source accumulation is tested in `ports.rs:649–727` (mixed categories) and `ports.rs:1038–1064` (from_declarations + multi-zone).

### Zone ID indexing consistency
**Verified**: `ZoneId` is a `u16` newtype (`environment.rs:58`). Port declarations reference zones by `ZoneId`, and environment zones are stored in `EnvironmentState.zones: Vec<ZoneState>` where `ZoneState.id: ZoneId`. No array-index dependence exists; zone lookup is always via `find()` or `iter().position()` with explicit `ZoneId` equality comparison. There are no off-by-one risks because `ZoneId` values are opaque and not used as slice indices. The OCHRE comparison reveals OCHRE uses string-based zone names (`zone.name`) rather than numeric IDs, which eliminates any indexing concerns entirely but adds string-allocation overhead.

## Summary
- Total findings: 6
- High: 0
- Medium: 3 (Findings 1, 2, 5)
- Low: 3 (Findings 3, 4, 6)

## Recommendations
1. **Validate equipment-declared zones against the environment at construction time.** Add a cross-reference check in `from_preparsed()` (after line 1156) that validates every `ZoneId` in equipment port declarations exists in `initial_env.zones`. This catches configuration errors at startup rather than silently accumulating to wrong zones.
2. **Route `apply_occupancy_gains()` through `PortSlots::accumulate()`.** Replace the direct `ports.thermal.iter_mut().find()` call with a proper `accumulate(&PortContribution::Thermal { ... })` call so that missing accumulators produce explicit errors consistent with all other equipment.
3. **Rebuild `PortSlots` in `refresh_equipment_caches()`** by calling `from_declarations()` with the updated equipment list + env zones. This ensures dynamically added equipment targeting new ports does not crash at runtime.
4. **Consider a dedicated `HvacWasteHeat` ThermalCategory** to separate HPWH compressor waste heat from tank jacket losses in per-category diagnostics. Low priority — energy balance is unaffected.
5. **The thermal solver correctly reads all six categories** (`thermal_solver/mod.rs:698–743`); no action needed, but this complex multi-category sum is worth a dedicated regression test.

## References / Citations
- OCHRE `Dwelling.py`: electrical power accumulation via per-equipment attributes (lines 289–293); no formal port-slot model.
- OCHRE `Equipment.py`: base equipment writes to `zone.internal_sens_gain` / `zone.internal_latent_gain` directly (lines 197–198); no undeclared-slot protection. HARES improves on this with the `PortSlots` receive-side error model.
- OCHRE `Envelope.py`: two-attribute zone gain model (`internal_sens_gain` + `hvac_sens_gain`, lines 427–430). HARES replaces this with the 6-category model.
- HARES `ports.rs`: `from_declarations()` at 441–498, `accumulate()` at 517–597, `add()` at 214–227.
- HARES `dwelling/mod.rs`: port construction at 1144–1157, occupancy gains at 2117–2129, `refresh_equipment_caches` at 1684–1781.
