# Scheduled load ZIP model and auto-zone routing by name convention

**Review ID**: equip-loads-02
**Category**: equipment-loads
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-equipment/src/scheduled_load.rs`
- `crates/hares-equipment/src/event_load.rs`

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Equipment/ScheduledLoad.py`
- `vendors/OCHRE/ochre/Equipment/EventBasedLoad.py`
- `vendors/OCHRE/ochre/Equipment/Equipment.py`
- `vendors/OCHRE/ochre/defaults/ZIP Parameters.csv`

## Findings

### Finding 1: [Severity: high]
**Description**: No per-equipment-type default ZIP coefficients — all 20+ scheduled load types default to constant-power (Z=0, I=0, P=1) with zero reactive power (pf=0). OCHRE provides distinct literature-based ZIP coefficients for each load type in `ZIP Parameters.csv`.

**Code Location**: `crates/hares-equipment/src/scheduled_load.rs:79-93` (`impl Default for ZipCoefficients`), `crates/hares-equipment/src/scheduled_load.rs:688-813` (`register_with_registry`)

**Root Cause**: `ScheduledLoad::new()` calls `ZipCoefficients::default()` which hard-codes `z=0.0, i=0.0, p_coeff=1.0, zq=0.0, iq=0.0, pq=1.0, pf=0.0`. Every registered load type (Lighting, Refrigerator, MELs, Ceiling Fan, Pool Pump, etc.) receives this identical default. Unlike OCHRE, which applies type-specific ZIP parameters from `ZIP Parameters.csv` (e.g., Lighting has Zp=0.54, Ip=0.5, Pp=-0.04, pf=1.0; Refrigerator has Zp=5.03, Ip=-8.48, Pp=4.45, pf=0.8; Clothes Washer has pf=0.65), HARES requires all ZIP coefficients to be explicitly supplied by the user via configuration keys.

**Impact**:
1. **Unity power factor overestimation**: Default `pf=0.0` means reactive power is always zero. In bulk simulations with many mixed loads, this overstates the combined load power factor. OCHRE's real-world data shows residential loads range from pf=0.65 (Clothes Washer) to pf=1.0 (resistive loads). Cumulatively, this could significantly distort distribution-level voltage and reactive power analyses.
2. **Missing voltage sensitivity**: Default `Z=0, I=0, P=1` assumes all loads are perfectly constant-power regardless of voltage. In reality, lighting is mostly constant-impedance (Z=0.54 in OCHRE), pumps are a mix, and refrigeration has complex nonlinear coefficients. This zeroes out voltage-dependent load behavior that is critical for voltage stability and conservation voltage reduction (CVR) studies. OCHRE's `run_zip()` method (`Equipment.py:200-218`) applies real and reactive ZIP corrections at every timestep based on actual grid voltage; HARES applies the ZIP model but with coefficients that default to the identity transform.
3. The issue is partially mitigated because HARES supports explicit configuration of all 7 ZIP coefficients (`zip_z`, `zip_i`, `zip_p`, `zip_zq`, `zip_iq`, `zip_pq`, `zip_pf`). However, there is no built-in lookup table or per-type defaults, putting the burden on every simulation input file to supply correct coefficients.

### Finding 2: [Severity: high]
**Description**: EventBasedLoad and WetAppliance have no ZIP model at all — reactive power is hard-coded to zero and voltage-dependent real power adjustment is absent.

**Code Location**: `crates/hares-equipment/src/event_load.rs:396-399` (EventBasedLoad electrical contribution), `crates/hares-equipment/src/event_load.rs:865-869` (WetAppliance electrical contribution)

**Root Cause**: Both `EventBasedLoad::update_outputs()` and `WetAppliance::update_outputs()` write `reactive_power_kvar: 0.0` directly into the electrical port contribution with no ZIP computation. No `ZipCoefficients` struct is ever instantiated for these equipment types. OCHRE's base `Equipment.run_zip()` (`Equipment.py:200-218`) is inherited by all equipment types including event-based loads, applying ZIP corrections to both `electric_kw` and `reactive_kvar` at every step.

**Impact**: Event-based loads (Clothes Washer, Dishwasher, Clothes Dryer, Cooking Range, generic EventBasedLoad) always appear as unity-power-factor, constant-power loads regardless of voltage. For motor-driven appliances like washers and dryers, which typically have significant reactive power draw (OCHRE shows pf=0.65 for Clothes Washer, pf=0.99 for Dryer/Kitchen Range in ZIP Parameters.csv), this misrepresents their contribution to distribution-level reactive power and voltage regulation. The magnitude depends on the penetration of these loads in a simulation.

### Finding 3: [Severity: medium]
**Description**: "Basement MELs" (a distinct scheduled load type in OCHRE) is not registered as a separate type in HARES. It appears in OCHRE's ZIP Parameters.csv (`vendors/OCHRE/ochre/defaults/ZIP Parameters.csv:31`) with ZIP coefficients identical to regular MELs but would route thermal gains to the Foundation zone.

**Code Location**: `crates/hares-equipment/src/scheduled_load.rs:688-813` (`register_with_registry`); `crates/hares-equipment/src/registry.rs:58-78` (`CANONICAL_EQUIPMENT_NAMES`)

**Root Cause**: The scheduled load registry enumerates "MELs" at line 718 but does not include a "Basement MELs" variant. While name-based zone routing (lines 172-187) would correctly route a load named "Basement MELs" to ZoneId(3), the "Basement MELs" class name string is not recognized by the equipment registry. If an HPXML or OCHRE input references "Basement MELs" as an equipment class, it would fail with "unknown equipment class".

**Impact**: HPXML-compatible input files that distinguish between above-grade and basement miscellaneous electric loads would fail to create the basement variant. Workaround: users can use the generic "MELs" class and set the name to "Basement MELs" to trigger auto-routing, but this requires manual intervention.

### Finding 4: [Severity: medium]
**Description**: The `ScheduledLoad` zone routing evaluates "exterior"/"outdoor" BEFORE checking explicit `zone_id`, creating a potential conflict where a user could set an explicit zone for an exterior-named load but see it silently overridden to `None`.

**Code Location**: `crates/hares-equipment/src/scheduled_load.rs:173-177`

**Root Cause**: The zone routing priority chain is:
1. EV end-use → None
2. Name contains "exterior" or "outdoor" → None
3. Explicit `parse_zone_id()` → Some(zone)
4. Name contains "garage" → ZoneId(2)
5. Name contains "basement" → ZoneId(3)
6. Default → ZoneId(1)

The exterior/outdoor check (step 2) takes precedence over the explicit zone check (step 3). In OCHRE (`Equipment.py:43-50`), `zone_name` from kwargs (explicit) is checked first, then name-based routing falls back only when `zone_name is None`.

**Impact**: If a user configures an equipment item named "Outdoor Sauna Heater" and explicitly sets `zone_id=1` (intending indoor heat gains), the name-based "outdoor" match at step 2 forces `zone=None`, suppressing all thermal gains regardless of the user's intent. The test at `scheduled_load.rs:2133-2140` (`outdoor_name_suppresses_auto_routing`) confirms this behavior is intentional and tested. However, OCHRE's priority order (explicit override wins) is more intuitive and user-friendly.

### Finding 5: [Severity: low]
**Description**: Reactive ZIP coefficients are validated only when any reactive coefficient is explicitly set, allowing silent acceptance of partial reactive configurations.

**Code Location**: `crates/hares-equipment/src/scheduled_load.rs:897-905`

**Root Cause**: The validation at lines 897-905 checks `(zq + iq + pq) == 1.0` only when at least one of `zq`, `iq`, or `pq` was explicitly provided. If a user provides `zip_pf=0.9` but no `zip_zq/zip_iq/zip_pq`, the defaults (zq=0, iq=0, pq=1, sum=1.0) are accepted silently and reactive power is computed correctly at nominal voltage. However, if a user provides only `zip_zq=0.5` without the other two reactive coefficients, the sum would be `0.5 + 0.0 + 1.0 = 1.5`, triggering a validation error. A user providing `zip_pq=0.5` alone would also have `0.0 + 0.0 + 0.5 = 0.5`, triggering an error. This is technically correct behavior, but the error message at line 902 only mentions the sum constraint without suggesting which coefficients are missing.

**Impact**: Minor UX friction. Users who understand the ZIP model will supply all three reactive coefficients. The guard correctly prevents misconfigured reactive ZIP sets. The message could suggest the default values applied to unset coefficients.

### Finding 6: [Severity: low]
**Description**: Gas schedule unit default (ThermsPerHour) is documented only in code comments; `gas_schedule_is_w` key is a branch not clearly explained to end users.

**Code Location**: `crates/hares-equipment/src/scheduled_load.rs:242-246`

**Root Cause**: The `gas_schedule_is_w` key at line 242 overrides the gas unit to Watts if set to true, otherwise the `GasScheduleUnit` parsed from the schedule source (which always defaults to `ThermsPerHour` in `parse_optional_gas_schedule_source`) is used. The comment on line 242 only says `// OCHRE convention`, and the key name `gas_schedule_is_w` does not clearly communicate that setting it to true makes the gas constant value interpreted as watts rather than therms/hour.

**Impact**: A user providing gas schedule values in watts without setting `gas_schedule_is_w=true` would have their values multiplied by the therms-to-watts conversion factor (~29,307), producing approximately 30,000x too much gas consumption. The reverse would under-scale by the same factor.

## Summary

- **Total findings**: 6
- **Critical**: 0
- **High**: 2 (Finding 1 — missing per-type ZIP defaults, Finding 2 — no ZIP model in event-based loads)
- **Medium**: 2 (Finding 3 — missing Basement MELs type, Finding 4 — zone routing priority order)
- **Low**: 2 (Finding 5 — reactive ZIP validation UX, Finding 6 — gas unit documentation)

## Recommendations

1. **Implement per-equipment-type ZIP default lookup**: Create a static mapping from equipment class name to `ZipCoefficients` using the OCHRE `ZIP Parameters.csv` values. Apply these defaults in `ScheduledLoad::new()` or `init_from_config()` before user overrides. At minimum, include the types that have `Included in OCHRE = TRUE` in the reference CSV.

2. **Add ZIP model support to EventBasedLoad and WetAppliance**: Extract the ZIP computation from `ScheduledLoad::step()` into a shared utility (`ZipCoefficients::apply()`) and call it in `EventBasedLoad::update_outputs()` and `WetAppliance::update_outputs()`, gated on a `zip` field being Some. Apply the same per-type default lookups.

3. **Register "Basement MELs" as a scheduled load type**: Add `registry.register("Basement MELs", ...)` with `EndUse::PLUG_LOADS` to the `register_with_registry()` function and add it to `CANONICAL_EQUIPMENT_NAMES`.

4. **Reorder zone-routing priority**: Move the `parse_zone_id` check before the "exterior"/"outdoor" name-based check so that explicit user configuration always wins. This aligns with OCHRE's priority order.

5. **Improve reactive ZIP validation error message**: When a subset of reactive ZIP coefficients is provided, include the default values applied to the missing coefficients in the validation error message.

6. **Document `gas_schedule_is_w` behavior**: Add a clear description of the key's effect to the telemetry field descriptions or module-level doc comment, noting that the default unit is therms/hour and that `gas_schedule_is_w=true` switches to watts.

## References / Citations

- OCHRE `Equipment.py:43-50` — zone routing by equipment name convention
- OCHRE `Equipment.py:70-77` — ZIP model initialization from kwargs
- OCHRE `Equipment.py:188-198` — `calculate_power_and_heat()` thermal gain routing gated on `self.zone is not None`
- OCHRE `Equipment.py:200-218` — `run_zip()` ZIP correction logic
- OCHRE `ScheduledLoad.py:38-41` — month multiplier zeroing
- OCHRE `ZIP Parameters.csv` — per-equipment-type ZIP coefficient defaults (38 rows, 14 types marked "Included in OCHRE")
- HARES `scheduled_load.rs:172-187` — zone auto-routing logic
- HARES `scheduled_load.rs:498-509` — ZIP voltage calculation in `step()`
- HARES `scheduled_load.rs:538-548` — thermal gain routing gated on `self.descriptor.zone`
- HARES `event_load.rs:396-399, 865-869` — hard-coded zero reactive power in event-based loads
- Bokhari et al., "Experimental Determination of the ZIP Coefficients for Modern Residential, Commercial, and Industrial Loads," IEEE Trans. Power Delivery, 2014
- Lu et al., "Load component database of household appliances and small office equipment," IEEE PESGM, 2008
