# Radiant distribution port inputs to all zones simultaneously
**Review ID**: envelope-05
**Category**: envelope
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-envelope/src/thermal_solver/ports.rs`
- `crates/hares-envelope/src/thermal_solver/config.rs`
- `crates/hares-envelope/src/thermal_solver/mod.rs`

Supporting files audited for full call path:
- `crates/hares-types/src/ports.rs` (definition of `PortSlots`, `ThermalAccumulator`, `PortContribution`)
- `crates/hares-core/src/dwelling/solver_builder.rs` (construction of `interior_lwr_zones` and `interior_solar_zones`)
- `crates/hares-equipment/src/scheduled_load.rs` (sole equipment setting non-zero `radiant_gain_w`)
- `crates/hares-equipment/src/hvac/duct_distribution.rs` (multi-zone duct thermal contributions)
- `crates/hares-envelope/src/thermal_solver/solar.rs` (parallel solar distribution using same surface configs)

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Models/Envelope.py`
- `vendors/EnergyPlus/src/EnergyPlus/InternalHeatGains.hh`
- `vendors/EnergyPlus/src/EnergyPlus/HeatBalanceIntRadExchange.hh`

## Findings

### Finding 1: Per-zone radiant scoping is correct -- no cross-zone contamination [Severity: low / FYI]
**Description**: The concern that "radiant gain ports apply their full magnitude to every zone simultaneously" is **not reflected in the current code**. Each `ThermalAccumulator` in `ports.thermal` corresponds to a single `ZoneId`. In `apply_port_radiant_inputs` (`ports.rs:52-109`), the function iterates all thermal ports and for each port:

1. Reads `thermal.zone` (the zone this port belongs to)
2. Finds the matching zone config via `self.config.interior_lwr_zones.iter().find(|z| z.zone_id == zone_id)` (line 74) or falls back to `interior_solar_zones` (line 90)
3. Distributes the port's `radiant_gain_w` **only** to surfaces within that matched zone's surface list (lines 76-83, 92-99)

A port for Zone A that carries 1000W radiant does **not** inject into Zone B's surfaces. The `continue` at lines 83/99 ensures only one zone config is queried per port, and the fallback at lines 102-107 writes to `zone_air_idx` scoped to `thermal.zone` via `self.wiring.zone_sensible_input_indices.get(&zone_id)` (line 61-64).

**Code Location**: `crates/hares-envelope/src/thermal_solver/ports.rs:52-109`
**Root Cause**: N/A -- The code correctly scopes distribution by port zone. The reported concern does not match the actual implementation.
**Impact**: No multi-zone energy conservation violation from radiant port distribution. Energy is conserved per zone: `Σ(surface × radiation_frac) + air_residual = total_radiant_w` (verified by test at `mod.rs:5400-5421`).

---

### Finding 2: No multi-zone radiant distribution regression test [Severity: low]
**Description**: All existing tests for `apply_port_radiant_inputs` use a single zone (`ZoneId(1)`) with one or two surfaces. There is no test that:
1. Constructs a multi-zone solver with `interior_solar_zones` or `interior_lwr_zones` populated for multiple distinct zones
2. Places non-zero `radiant_gain_w` on only one zone's `ThermalAccumulator`
3. Validates that the other zone's surface and air input indices remain at zero after `apply_port_radiant_inputs` is called

Without such a test, a future refactor of the zone-config lookup or the distribution functions could inadvertently introduce cross-zone contamination.

**Code Location**: `crates/hares-envelope/src/thermal_solver/mod.rs:5320-5422` (single-zone test)
**Root Cause**: Existing test suite only exercises single-zone radiant distribution.
**Impact**: No production impact; missing defense against a conceivable regression.

---

### Finding 3: `InteriorSolarSurfaceInfo.input_index` doc comment is misleading [Severity: low / FYI]
**Description**: The doc comment at `config.rs:308-313` states:

> `/// Index into input vector u for the surface RC node.
> /// In production this is always Some(zone_air_idx). Windows are
> /// excluded from the radiant distribution by solar_absorptance = 0.0
> /// (set at construction in solver_builder.rs), not by a None index.`

This implies `input_index == zone_sensible_input_indices[zone]` for all surfaces in a zone. If true, ALL surfaces in a zone would write to the same `u` position, collapsing surface-level distribution into a single overwrite. The test at `mod.rs:5338-5349` contradicts this: two surfaces have `input_index` values `Some(1)` and `Some(2)` while `zone_sensible_input_indices[ZoneId(1)]` is `3`. The actual behavior (confirmed by `solver_builder.rs:891`) is that each surface gets its own RC-node input index from the RC network topology.

**Code Location**: `crates/hares-envelope/src/thermal_solver/config.rs:308-312`
**Root Cause**: Internal documentation inaccuracy. The term "zone_air_idx" in the comment is being used loosely to mean "some valid (non-None) index" rather than the literal zone sensible input index.
**Impact**: Could mislead a reviewer or developer into thinking surface-level distribution is just overwriting the zone air input. This may have contributed to the concern raised in this review.

---

### Finding 4: Duct-loss thermal contributions always set `radiant_gain_w = 0.0` [Severity: low / confirmation]
**Description**: The only code paths that set non-zero `radiant_gain_w` are:
- `ScheduledLoad` (`scheduled_load.rs:533-548`): writes to a single configured zone
- Occupancy gains (`dwelling/mod.rs:2112-2128`): writes to the indoor zone only

All other equipment types (including HVAC via `duct_distribution.rs:121-127`) explicitly set `radiant_gain_w: 0.0`. This means:
- HVAC duct losses to non-conditioned zones are 100% convective (no radiant)
- Only scheduled internal loads and occupancy produce radiant gains
- Even across multi-zone duct distribution, no zone receives radiant from equipment that wasn't specifically placed in that zone

This design is physically defensible (ducts heat surrounding air, not directly radiate to surfaces), but is worth noting because any future equipment that DOES produce radiant gains will automatically use the correct per-zone scoping in `apply_port_radiant_inputs`.

**Code Location**: `crates/hares-equipment/src/hvac/duct_distribution.rs:121-127`
**Root Cause**: Intentional design choice -- all equipment radiant is gas/appliance/occupant internal gains, not HVAC.
**Impact**: No current issue. Validates that multi-zone radiant cross-contamination cannot arise from HVAC equipment.

---

### Finding 5: Comparison with OCHRE and EnergyPlus [Severity: FYI]
**Description**: Both OCHRE and EnergyPlus scope internal gain radiant distribution to a single zone/space:

- **OCHRE** (`Envelope.py:1173-1198`): The `update_radiation()` method iterates `for zone in self.zones.values()` and distributes window-transmitted solar and interior LWR gains per zone. Equipment writes to `surface.internal_gain` (per-surface, `Envelope.py:196`) and `zone.internal_sens_gain` (per-zone air, `Envelope.py:427`). At model update (`Envelope.py:1283-1300`), surface-level gains are combined with inputs_init and passed to the RC solver.

- **EnergyPlus** (`InternalHeatGains.hh:200-201, 210-213`): `SumAllSpaceInternalRadiationGains` and `SumEnclosureInternalRadiationGainsByTypes` aggregate radiant gains per space/enclosure. The `CalcInteriorRadExchange` function (`HeatBalanceIntRadExchange.hh:70-75`) performs intra-zone ScriptF exchange with an optional `ZoneToResimulate` parameter -- radiation exchange is always bounded within a single zone/enclosure.

HARES follows the same zone-scoped paradigm. The TMULT (area × absorptivity) distribution in HARES is mathematically equivalent to EnergyPlus's zone internal gains distribution method (E+ Eng. Ref. "Zone Internal Gains -- TMULT method"), with the same per-zone bounding.

---

## Summary
- Total findings: 5
- Critical: 0
- High: 0
- Medium: 0
- Low / FYI: 5

## Recommendations

1. **No code change needed for per-zone radiant distribution.** The current implementation correctly scopes radiant port gains to each port's zone via `interior_lwr_zones`/`interior_solar_zones` zone_id lookup.

2. **Add a multi-zone radiant distribution regression test** to `mod.rs` tests: construct a 2-zone solver, place radiant gain on zone A only, verify zone B's input indices remain unchanged after `apply_port_radiant_inputs`.

3. **Fix the misleading doc comment** on `InteriorSolarSurfaceInfo.input_index` (`config.rs:308-312`). Replace `"zone_air_idx"` with a more accurate description like `"the surface RC node's position in the input vector"`.

4. **Consider adding an `assert!` or `debug_assert!`** in `apply_port_radiant_inputs` that verifies no other zone's surface inputs were modified by the distribution (compare pre- and post-distribution `u` values at indices belonging to other zones). This would provide defense-in-depth against future wiring bugs.

## References / Citations

- EnergyPlus Engineering Reference, "Zone Internal Gains" chapter, TMULT (total multiplier) method for distributing radiant gains to surfaces within a zone.
- EnergyPlus Engineering Reference, "Inside Heat Balance": *"The radiative part is then distributed over the surfaces within the zone in some prescribed manner."*
- ASHRAE HoF 2021 Ch. 18 §2: Convective and radiant fractions of internal gains belong to the zone where the equipment resides.
- OCHRE Envelope.py: lines 1173-1198 (per-zone radiation update), lines 1283-1300 (equipment gain combination with RC inputs).
- HARES ports.rs: line 52 doc comment references E+ TMULT method and correctly cites: "both convective and radiant fractions of internal gains belong to the zone where the equipment resides."
