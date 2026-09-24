# Autosizing oversizing factors: 1.4x heating, 1.15x cooling, ACCA Manual S-2017 compliance
**Review ID**: coredeep-07
**Category**: core-deep
**Date**: 2026-05-26

## Files Reviewed
crates/hares-core/src/dwelling/autosize.rs

## Vendor/Reference Files Consulted
None

## Findings

### Finding 1: [Severity: medium]
**Description**: Oversizing factors lack climate zone awareness. The same factors (1.4x heating, 1.15x cooling) are applied regardless of climate. ACCA Manual S-2017 Table 4-1 permits cooling oversizing up to 1.3x in hot-dry climates (IECC zones 2B, 3B, 4B) where latent load is low and short-cycling risk from sensible overshoot is reduced. Conversely, hot-humid climates (zones 1A, 2A) warrant the conservative 1.15x limit—or even 1.10x—to maintain adequate dehumidification. The codebase has no climate zone classification, so the same 1.15x factor applies in Phoenix (dry) and Miami (humid) alike. In dry climates this may undersize equipment relative to what Manual S permits; in humid climates it is compliant but could be tightened.
**Code Location**: `crates/hares-core/src/dwelling/autosize.rs:32-33` (constant definitions), `:125-129` (heating factor application), `:206-210` (cooling factor application)
**Root Cause**: The `HEATING_OVERSIZE_FACTOR` and `COOLING_OVERSIZE_FACTOR` constants are single scalar values with no mapping to climate zone, design humidity ratio, or latent/sensible load split.
**Impact**: Equipment sized with the uniform 1.15x cooling factor may be one capacity increment smaller than permitted in dry climates, risking unmet cooling setpoints on design days. In humid climates the factor is appropriate, but the code cannot distinguish between the two regimes.

### Finding 2: [Severity: medium]
**Description**: Oversizing factors do not differentiate by equipment type or staging capability. ACCA Manual S-2017 §4-3 through §4-6 specify different oversizing limits by equipment class:

| Equipment type | Manual S max oversizing | Code default | Gap |
|---|---|---|---|
| Single-stage furnace | 1.4x (up to 2.0x cold-climate) | 1.4x | Compliant |
| Single-stage AC/HP (cooling) | 1.15x | 1.15x | Compliant |
| Variable-speed AC/HP (cooling) | Up to 1.3x | 1.15x | Conservative |
| Heat pump (heating) | ~1.25x | 1.4x | **Potentially oversized** |

Variable-speed and multi-stage equipment self-modulate and tolerate more oversizing without short-cycling. Conversely, heat pumps in heating mode should be limited to approximately 1.25x because oversized heat pumps cycle excessively during mild weather, degrading COP and increasing auxiliary heat runtime. The current `COOLING_OVERSIZE_FACTOR = 1.15` is appropriate for single-stage ACs but unnecessarily conservative for variable-speed units. The `HEATING_OVERSIZE_FACTOR = 1.4` is fine for furnaces but may be excessive for heat pumps.

The `EquipmentSpec` carries a `name` field (e.g., `"Gas Furnace"`, `"Air Conditioner"`, `"Air Source Heat Pump"`) and a `system_type` parameter in `parameters`, but neither is consulted during factor selection.
**Code Location**: `crates/hares-core/src/dwelling/autosize.rs:32-33` (constants), `:96-110` (loop that reads `autosize_heating`/`autosize_cooling` flags but ignores `system_type` and `name`)
**Root Cause**: The autosizing loop receives `EquipmentSpec` objects that contain `name`, `fuel_type`, and `parameters` (which includes `system_type`), but the factor-selection logic only looks for the optional HPXML override values—it never inspects the equipment classification to adjust the default factor.
**Impact**: Variable-speed equipment sized at 1.15x may be one capacity increment smaller than permitted by Manual S, leaving capacity headroom unused. Heat pumps sized at 1.4x for heating may short-cycle during shoulder seasons and have elevated auxiliary heat consumption.

### Finding 3: [Severity: low]
**Description**: Oversizing factors do not account for building thermal mass. High-mass buildings (concrete, brick, stone construction) have greater thermal inertia that buffers temperature swings and reduces short-cycling risk. ACCA Manual S does not prescribe a mass-dependent factor adjustment, but engineering best practice (and ASHRAE Handbook—HVAC Applications §19) recognizes that high-mass buildings can tolerate larger equipment because the thermal time constant of the structure damps the on/off cycling frequency. The codebase already computes a `mass_multiplier` per zone (`crates/hares-core/src/dwelling/conversions.rs:34`), which feeds into the RC model's zone capacitance, but the autosizing module does not read this multiplier to relax oversizing limits for high-mass construction.
**Code Location**: `crates/hares-core/src/dwelling/autosize.rs:96-272` (loop body lacks any mass inspection), `crates/hares-envelope/src/boundary_rc.rs:423` (mass_multiplier used for zone capacitance but not exposed back to autosizing)
**Root Cause**: The autosizing function receives the `ThermalSolver` but only calls its `autosize_capacity` / `autosize_capacity_cooling` methods; it does not query zone capacitance or mass characteristics.
**Impact**: High-mass buildings may be fitted with more conservative equipment than necessary, though the practical energy penalty is small because the building's thermal mass already provides a buffer. The risk is primarily missed opportunity for cost savings on equipment.

### Finding 4: [Severity: medium]
**Description**: Oversizing factor is applied to the continuous design load value, but there is no discrete sizing step that rounds up to the next available manufacturer capacity. ACCA Manual S-2017 §5 describes the procedure as: (1) compute design load, (2) apply oversizing factor to get target capacity, (3) select the next-larger available equipment size that meets or exceeds the target. The code computes `sized_capacity = raw_capacity * factor` (a floating-point value) and writes it directly to `heating_capacity_w` / `cooling_capacity_w`. Real HVAC equipment is available in discrete capacity increments (e.g., 18k, 24k, 30k, 36k, 42k, 48k, 60k BTU/h for residential units). Without a round-up step, the effective oversizing factor may deviate from the intended factor when the computed capacity falls between available sizes. For example, a design load of 8,500 W with 1.15x factor yields 9,775 W target. If the nearest available sizes are 8,800 W (30k BTU/h) and 10,500 W (36k BTU/h), the code's 9,775 W value is below the next available size, and the simulation will use 9,775 W as the equipment capacity—which does not correspond to any real unit.
**Code Location**: `crates/hares-core/src/dwelling/autosize.rs:131` (`let mut sized_capacity = raw_capacity * factor;`), `:164` (capacity written directly to parameters), `:212` (same for cooling)
**Root Cause**: The autosizing module computes a theoretical continuous capacity without a manufacturer catalog lookup or discrete capacity selection step. This is a design simplification rather than a bug.
**Impact**: In simulation, equipment capacity is treated as continuously variable, which may not reflect real-world equipment selection constraints. The energy consumption predictions may be optimistic if the model assumes the equipment can operate at the exact calculated capacity. Downstream interpolation between discrete capacity points in performance maps may also produce results that differ from real equipment behavior.

### Finding 5: [Severity: low]
**Description**: Default oversizing factors are hardcoded as `const` values with no configuration-file override mechanism. While HPXML `<HeatingAutosizingFactor>` / `<CoolingAutosizingFactor>` elements allow per-equipment override, there is no global configuration point (e.g., simulation settings JSON, environment variable, or CLI flag) to change the defaults without modifying every HPXML file. Users who want a site-wide policy (e.g., "use 1.0x for all cooling because we size to design load exactly") must edit every equipment entry in every HPXML file.
**Code Location**: `crates/hares-core/src/dwelling/autosize.rs:32-33` (hardcoded `const` values), `:129` (`unwrap_or(HEATING_OVERSIZE_FACTOR)`), `:210` (`unwrap_or(COOLING_OVERSIZE_FACTOR)`)
**Root Cause**: The defaults are defined as module-level `const` declarations with no indirection through a configuration struct or settings provider.
**Impact**: Workflow friction for users who want non-Manual-S defaults; no programmatic risk.

## Summary
- Total findings: 5
- Critical / High / Medium / Low: 0 / 0 / 3 / 2

## Recommendations

1. **Add climate zone awareness to cooling oversizing (Finding 1)**. Derive an IECC climate zone from the design outdoor dry-bulb and humidity ratio (or from the weather file location). Map zones 2B/3B/4B to a 1.3x cooling factor per Manual S Table 4-1. Consider tightening to 1.10x for zones 1A/2A where dehumidification is critical. This can be implemented as a lookup table keyed by climate zone classification.

2. **Differentiate oversizing by equipment type (Finding 2)**. Inspect `spec.name` and/or `spec.parameters["system_type"]` to identify equipment class. Cap heat pump heating at 1.25x. Allow variable-speed cooling equipment up to 1.3x. For furnaces, retain 1.4x but consider allowing up to 2.0x for cold-climate zones per Manual S §4-3.

3. **Expose thermal mass to the autosizing module (Finding 3)**. Query the `ThermalSolver` for zone capacitance or mass classification. For high-mass buildings (e.g., zone capacitance >3× the standard furniture+air baseline), relax the oversizing limit by 5–10% to reflect reduced short-cycling risk.

4. **Add a global defaults override mechanism (Finding 5)**. Introduce an `AutosizeDefaults` struct (fields: `heating_factor`, `cooling_factor`) that can be set via simulation configuration and passed to `autosize_equipment_capacities`. The HPXML per-equipment override should still take precedence.

5. **Consider discrete equipment sizing (Finding 4)**. If manufacturer capacity catalogs are available (e.g., from the defaults store), add a round-up step after the factor is applied: compute `target = raw_capacity * factor`, then select the smallest available capacity ≥ target. If no catalog is available, retain the current continuous approach but document the assumption.

## References / Citations

- ACCA Manual S-2017 (Residential Equipment Selection), especially §4 (Oversizing Limits) and Table 4-1 (Maximum Oversizing by Climate and Equipment Type).
- ACCA Manual J-2017 (Residential Load Calculation), §2 (Design Conditions) — the load calculation that supplies the design load to Manual S.
- ASHRAE Handbook—HVAC Applications 2019, Chapter 19 (Residential Cooling and Heating Load Calculations), discussion of thermal mass effects on equipment sizing.
- ANSI/RESNET/ICC 301-2019 §4.4.5 (Standard for the Calculation and Labeling of the Energy Performance of Dwelling and Sleeping Units using an Energy Rating Index) — references ACCA Manual S for equipment sizing in rating software.
- HPXML Data Dictionary v4.2: `<HeatingAutosizingFactor>`, `<CoolingAutosizingFactor>`, `<AutosizingLimits>` elements.
- Manual S §4-3: Furnace oversizing up to 2.0x permitted in cold climates with appropriate duct sizing.
- Manual S §4-5: Heat pump heating mode oversizing limited to 1.25x to prevent excessive cycling.
- Manual S §4-6: Variable-speed equipment may use the next-larger nominal size if it can modulate below the design load.
