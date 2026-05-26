# Lighting thermal gain fraction defaults and location routing
**Review ID**: equip-loads-04
**Category**: equipment-loads
**Date**: 2026-05-26

## Files Reviewed
crates/hares-io/src/hpxml/resolve_loads.rs crates/hares-equipment/src/scheduled_load.rs

## Vendor/Reference Files Consulted
vendors/OCHRE/ochre/utils/hpxml.py

## Findings
### Finding 1: [Severity: medium]
**Description**: Lighting thermal gain fractions are not differentiated by technology type (LED, CFL, incandescent). All lighting equipment receives a default of `(sensible=1.00, latent=0.00, radiative=0.00)`, treating all lighting heat gain as purely convective. Physically, LED fixtures convert ~70% of input power to visible light (which becomes radiant heat at illuminated surfaces) with only ~30% as convective fixture heat, while incandescent fixtures produce ~90% direct convective/radiant heat at the fixture and only ~10% visible light. The current lumped 100%-convective model cannot capture this distinction, which matters for radiant heat exchange and thermal comfort calculations.

**Code Location**: `crates/hares-io/src/hpxml/resolve_loads.rs:784-786` (`default_gain_fractions()` match arm for all lighting types).

**Root Cause**: HARES follows OCHRE's pattern exactly (OCHRE `hpxml.py:1522-1527` sets `"Convective Gain Fraction (-)": 1` for all lighting), which is a known simplification. OCHRE has a TODO comment at `hpxml.py:1522` acknowledging this gap: `# TODO: get default fractions/multipliers for lighting`. Neither HARES nor OCHRE exposes the per-technology gain fractions via `FracSensible`/`FracRadiant` extension nodes in the XML.

**Impact**: Low for total energy balance (all electrical energy ultimately becomes heat in the zone), but medium for thermal comfort physics — the convective/radiant split affects zone air temperature vs. mean radiant temperature, which in turn affects HVAC setpoint satisfaction. LED-dominated houses would underestimate radiant gains from lighting absorbed by thermal mass.

### Finding 2: [Severity: low]
**Description**: Unrecognized `LightingType` strings from HPXML `LightingGroup` elements are silently discarded with zero contribution to any technology fraction bucket. The implicit remainder-handling logic `f_inc = (1.0 - f_led - f_flr).max(0.0)` (line 812) treats the discarded fraction as incandescent only if the total of recognized fractions sums to less than 1.0. A single unrecognized type with `FractionofUnitsInLocation=0.5` would lose that 50% entirely from the annual kWh calculation since the unrecognized fraction is never accumulated into `led`, `compact_fluorescent`, or `fluorescent_tube`.

**Code Location**: `crates/hares-io/src/hpxml/resolve_loads.rs:430-445` (the `lighting_group_type()` → match fallthrough paths).

**Root Cause**: The `lighting_group_type()` function returns `None` when the first child element's name is not recognized. In the match at lines 436-441, only `"lightemittingdiode"`, `"compactfluorescent"`, and `"fluorescenttube"` are matched; everything else falls to `_ => {}`. There is no warning or error for unrecognized types.

**Impact**: Low — HPXML files from ResStock and BEopt use these three canonical type names consistently. However, hand-authored HPXML or future schema extensions could introduce new types (`"led"`, `"halogen"`, etc.) that would silently produce incorrect annual kWh.

### Finding 3: [Severity: low]
**Description**: `Gas Lighting` equipment is registered with `EndUse::LIGHTING` and defaults to `(sensible=0.00, latent=0.00)` — zero zone gain. This is correct for exterior gas lighting (torches, landscape gas lamps) which exhaust combustion products outdoors. However, the OCHRE `Default Schedule Parameters.csv` includes a full indoor-style weekday/weekend schedule and month multipliers for gas lighting (lines 61-63), suggesting OCHRE may have intended gas lighting to have some indoor gain but never implemented it. This is not a functional difference — both codebases agree — but worth noting.

**Code Location**: `crates/hares-io/src/hpxml/resolve_loads.rs:788` (`default_gain_fractions()` match arm for `"Gas Lighting"`) and `crates/hares-equipment/src/scheduled_load.rs:757-760` (registry entry).

**Root Cause**: OCHRE's `parse_mel()` at `hpxml.py:1550-1555` sets `sensible_gain_default = 0` for non-"other" and non-"fireplace" loads, which includes gas lighting. HARES matches this.

**Impact**: Negligible — gas lighting is rare in modern residential buildings.

### Finding 4: [Severity: medium]
**Description**: Basement lighting is always created when HPXML contains a basement lighting group, regardless of whether the basement is conditioned or unconditioned. OCHRE only creates `Basement Lighting` equipment when `Foundation Type == "Finished Basement"` (OCHRE `hpxml.py:1695-1698`). HARES unconditionally creates basement lighting equipment and routes it to `FOUNDATION_ZONE_ID` (ZoneId 3). For unconditioned basements, this would incorrectly route lighting heat gains to a zone that may not be thermally modeled as conditioned space.

**Code Location**: `crates/hares-io/src/hpxml/resolve_loads.rs:448-454` (location → name mapping creates `"Basement Lighting"` unconditionally); `crates/hares-equipment/src/scheduled_load.rs:182-183` (auto-routes `"basement"` named equipment to `FOUNDATION_ZONE_ID`).

**Root Cause**: HARES does not inspect `FoundationType` when deciding whether to emit basement lighting equipment. OCHRE gates on `construction["Foundation Type"] == "Finished Basement"`.

**Impact**: Medium — for unconditioned basements (crawlspaces, vented basements), lighting installed in the basement should not route internal gains to the foundation zone. The total energy balance is preserved (the losses still go somewhere in the model), but zone-level HVAC loads would be incorrectly distributed.

### Finding 5: [Severity: low]
**Description**: The `derive_lighting_annual_kwh()` function uses `garage_area_ft2` calculated from `garage_floor_area_m2()` (lines 879-886), which returns 0.0 when no Garage zone exists in the building model. For buildings without garages, garage lighting annual kWh is computed as `100.0 * adj`, which uses only the efficiency adjustment and ignores area. OCHRE explicitly skips garage lighting when no garage is modeled (hpxml.py:1703-1709, prints a warning). HARES silently creates garage lighting with a floor-area-independent annual kWh even when no garage exists.

**Code Location**: `crates/hares-io/src/hpxml/resolve_loads.rs:425` (area calculation), line 825 (garage formula), lines 448-454 (name assignment).

**Root Cause**: HARES emits `"Garage Lighting"` for any `LightingGroup` with `Location="garage"` regardless of whether a garage zone exists in the building model.

**Impact**: Low — HPXML files typically only contain garage lighting groups when a garage is present. A malformed HPXML could create phantom lighting loads.

## Summary
- Total findings: 5
- Critical / High / Medium / Low: 0 / 0 / 2 / 3

## Recommendations
1. **Technology-differentiated lighting gain fractions**: Add per-technology `radiative_gain_fraction` defaults (e.g., LED ~0.70 radiative, incandescent ~0.10 radiative, CFL ~0.30 radiative). This requires the `build_spec()` flow to support technology-type-dependent defaults, which it currently does not — `default_gain_fractions()` only sees the equipment name, not the technology mix. One approach is to compute a weighted-average radiative fraction from the `LightingFractions` struct during HPXML resolution and inject it as a `radiative_gain_fraction` parameter. This would improve radiant heat exchange fidelity.

2. **Warn on unrecognized LightingType**: Add a `tracing::warn!()` in the wildcard match arm at `resolve_loads.rs:440` to surface unknown lighting type strings, preventing silent data loss.

3. **Gate basement lighting on foundation type**: Add logic to check whether the foundation is conditioned before creating `Basement Lighting` equipment. For unconditioned foundations, set `sensible_gain_fraction=0.0` (as done for non-conditioned refrigerators at line 376-378) or skip the equipment entirely.

4. **Gate garage lighting on garage existence**: Skip garage lighting equipment creation when `garage_floor_area_m2()` returns 0.0, matching OCHRE's behavior.

5. **Add basement lighting schedule default**: OCHRE's `schedule.py:390-391` copies the interior lighting schedule to basement lighting when no basement-specific schedule exists. Verify HARES' `schedule_resolve.rs` handles this fallback.

## References / Citations
- OCHRE `parse_lighting()`: `vendors/OCHRE/ochre/utils/hpxml.py:1486-1531`
- OCHRE lighting equipment creation: `vendors/OCHRE/ochre/utils/hpxml.py:1684-1711`
- OCHRE `LightingLoad` class: `vendors/OCHRE/ochre/Equipment/ScheduledLoad.py:96-97`
- OCHRE `Equipment.__init__` gain fraction handling: `vendors/OCHRE/ochre/Equipment/Equipment.py:83-87`
- OCHRE default schedule parameters: `vendors/OCHRE/ochre/defaults/Default Schedule Parameters.csv` lines 8-18 (ANSI/RESNET/ICC 301-2022 Addendum C, Table C.3(3) and C.3(4))
- HARES `default_gain_fractions()`: `crates/hares-io/src/hpxml/resolve_loads.rs:763-799`
- HARES zone auto-routing: `crates/hares-equipment/src/scheduled_load.rs:168-187`
- HARES lighting schedule resolution: `crates/hares-io/src/schedule_resolve.rs:83-101`
- HARES default schedule parameters: `defaults/Default Schedule Parameters.csv` lines 8-10
- ANSI/RESNET/ICC 301-2014 §4.2.2.5.2 (lighting energy calculation methodology)
- ASHRAE Handbook — Fundamentals, Chapter 18 (lighting heat gain split between convective and radiant for different lighting technologies)
