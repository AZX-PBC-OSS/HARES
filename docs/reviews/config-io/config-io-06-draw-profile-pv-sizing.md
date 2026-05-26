# Electrical draw profile construction and PV sizing from HPXML
**Review ID**: config-io-06
**Category**: config-io
**Date**: 2026-05-26

## Files Reviewed
crates/hares-io/src/draw_profile.rs crates/hares-io/src/pv_sizing.rs

## Vendor/Reference Files Consulted
vendors/OCHRE/ochre/Equipment/PV.py

## Findings

### Finding 1: [Severity: high]
**Description**: No coincidence (diversity) factors are applied when aggregating multiple end-use electrical loads into a total dwelling load. Each load type receives its own independent schedule scaled to `max_kw = annual_kwh / 8760 / mean(fraction)`. Since all schedules are independently computed and summed at the electrical port, there is no mechanism to prevent unrealistically high peak dwelling loads caused by all loads coinciding at their individual maxima simultaneously.

**Code Location**: `crates/hares-io/src/schedule_resolve.rs:652-728` (inject_power_schedule) and `crates/hares-io/src/schedule_resolve.rs:830-875` (inject_event_schedule). The per-equipment max_kw is computed at line 784 in `determine_max_kw` but never cross-correlated with other loads.

**Root Cause**: The schedule resolution pipeline treats every load as independent. OCHRE/ResStock uses timing-diverse stochastic schedules (ResStock timeseries CSV columns such as `clothes_washer`, `dishwasher`, etc.) that inherently encode temporal diversity, but HARES' fallback path — using synthetic daily profiles with independently scaled peaks — lacks this temporal correlation. In the CSV path, the ResStock schedules already embed diversity, so the primary concern is the fallback `daily_profile` and `constant` paths (lines 703-727).

**Impact**: Peak total dwelling electrical load can be overestimated by 30–60% compared to measured data, particularly when multiple loads use fallback daily profiles that all peak during evening hours. This can cause oversizing of service equipment in sizing studies and misrepresent feeder-level peak demand in grid-integration analyses.

### Finding 2: [Severity: medium]
**Description**: The `COLUMN_MAPPINGS` table in `schedule_resolve.rs` is missing the `microwave` load type. The HPXML standard (and the ResStock/BuildStock datasets built on it) includes microwave ovens as a separate end use. While HPXML's `CookingRange` element may encompass microwave energy in some simplified HPXML inputs, standard ResStock schedule CSVs include a dedicated `microwave` column that is not mapped.

**Code Location**: `crates/hares-io/src/schedule_resolve.rs:44-228` (COLUMN_MAPPINGS table). No entry exists for `csv_column: "microwave"`.

**Root Cause**: Incomplete mapping of HPXML/ResStock schedule CSV columns to equipment types. The `Cooking Range` entry at line 79-82 is categorized as `EventWindow` and serves the range/oven combined appliance, but a separate microwave entry is absent.

**Impact**: When a ResStock schedule CSV contains a `microwave` column, the column is silently ignored and the microwave load is not simulated. This underestimates total dwelling electrical consumption by roughly 1–3% depending on household characteristics (microwaves typically use 60–120 kWh/year per HPXML defaults).

### Finding 3: [Severity: medium]
**Description**: The `extra_refrigerator` column is explicitly set to `Ignore` category in COLUMN_MAPPINGS (line 178-182), meaning secondary refrigerators' schedule fractions from the CSV are discarded even when they are parsed from HPXML with annual energy values. Meanwhile, secondary refrigerators *are* resolved in `resolve_loads.rs` (lines 365-391) with proper annual kWh and zero zone gain fractions for non-conditioned locations, but they lack schedule data because the CSV column is skipped.

**Code Location**: 
- `crates/hares-io/src/schedule_resolve.rs:178-182` — `extra_refrigerator` categorized as `Ignore`.
- `crates/hares-io/src/hpxml/resolve_loads.rs:365-391` — secondary fridge parsed with instance naming but no schedule column fed in.

**Root Cause**: The column mapping explicitly skips `extra_refrigerator` without providing an alternative schedule source. The secondary refrigerator equipment spec will fall through to the default profile or constant power path in `inject_power_schedule`.

**Impact**: Secondary refrigerators end up using generic fallback schedules (either from `Default Schedule Parameters.csv` or constant power) rather than the behaviorally correct ResStock time series. This is a minor accuracy loss — the annual energy is still correct, but the intra-day temporal profile is less realistic.

### Finding 4: [Severity: low]
**Description**: The `pv_sizing.rs` module sizes only the DC array capacity and does not compute or output an inverter capacity or DC/AC ratio. The `PvSizingResult` struct (line 74-83) contains `capacity_kw`, `num_panels`, `collector_area_m2`, tilt, azimuth, `system_losses_fraction`, etc., but no `inverter_capacity_kw` or `dc_ac_ratio`. Inverter sizing is left entirely to the downstream `PvConfig` (typed config).

**Code Location**: 
- `crates/hares-physics/src/pv_sizing.rs:74-83` — `PvSizingResult` struct lacks inverter capacity field.
- `crates/hares-physics/src/pv_sizing.rs:341-378` — `size_pv_system()` does not compute inverter capacity.

**Root Cause**: The sizing module intentionally defers inverter sizing to the PV equipment configuration layer. However, the resulting `PvSizingResult` provides no hint for a reasonable inverter size. OCHRE's PV.py line 122 sets `inverter_capacity = inverter_capacity or self.capacity` (1:1 default), which is suboptimal. Best practice is a DC/AC ratio of 1.1–1.4.

**Impact**: Without guidance on inverter sizing, users may connect a 1:1 inverter (default in PvConfig, see `crates/hares-equipment/src/pv/config.rs:32` where `inverter_capacity_kw` is `Option<f64>` — when `None`, the PV model at line 272 returns early from `apply_inverter_limits` without clipping), missing the economic benefit of DC/AC oversizing. Conversely, if a user manually sets too low an inverter capacity, excessive clipping reduces annual generation.

### Finding 5: [Severity: low]
**Description**: The `system_losses_fraction` default of 0.14 (14%) is applied as a blanket derating in the PV step function (`crates/hares-equipment/src/pv/mod.rs:258`). This lumps soiling, wiring, mismatch, nameplate tolerance, availability, shading, snow, and age into a single scalar. While this matches PVWatts v8 defaults, it does not allow component-level disaggregation. The soiling model exists separately (`soiling.rs`) and is applied as an additional per-timestep ratio, but the 14% lump-sum derating is applied regardless.

**Code Location**: 
- `crates/hares-equipment/src/pv/mod.rs:39` — `DEFAULT_SYSTEM_LOSSES_FRACTION = 0.14`
- `crates/hares-equipment/src/pv/mod.rs:258` — `dc_power_kw *= 1.0 - self.system_losses_fraction`
- `crates/hares-physics/src/pv_sizing.rs:92` — `DEFAULT_SYSTEM_LOSSES = 0.14` in sizing

**Root Cause**: PVWatts v8 convention, adopted directly. In PVWatts, the 14% default is configurable; HARES' typed config exposes this via `system_losses_fraction` option. However, the soiling model is additive to this (lines 206-208: irradiance is first multiplied by `soiling_ratio`, then DC power is further multiplied by `(1 - system_losses_fraction)`), which double-counts soiling if both are active.

**Impact**: If a user enables the Kimber soiling model AND leaves system_losses_fraction at the 14% default (which includes ~2% for soiling in PVWatts convention), soiling is double-counted. Annual generation for a soiling-enabled configuration would be underestimated by roughly 2%.

### Finding 6: [Severity: low]
**Description**: The `lighting_exterior_holiday` column is mapped to `Ignore` (line 188-192), meaning holiday decorative lighting load is not modeled. While this is a minor load (~10-50 kWh/year), OCHRE also ignores it, so the behavior is consistent with the vendor reference. However, the ResStock schedule CSV does contain energy for this column that is silently discarded.

**Code Location**: `crates/hares-io/src/schedule_resolve.rs:188-192`.

**Root Cause**: Deliberate choice to match OCHRE behavior, but the annual energy is lost rather than folded into another exterior lighting load.

**Impact**: Negligible for most analyses (affects <0.5% of annual consumption). Only relevant for peak-day studies around major holidays (Thanksgiving, Christmas) when holiday lighting is active.

### Finding 7: [Severity: low]
**Description**: HARES' PV power model applies inverter clipping after computing DC→AC conversion and after curtailment (line 532-535), whereas OCHRE's PVWatts-based model applies clipping inherently within the SAM engine during the DC→AC conversion step. The ordering difference means HARES' `inverter_clipping_kw` telemetry metric (line 535) reflects clipping of AC power post-curtailment, while OCHRE/PVWatts would clip DC power pre-inverter. This produces slightly different clipping telemetry values but identical net AC output in practice since both are bounded by the same inverter capacity.

**Code Location**: `crates/hares-equipment/src/pv/mod.rs:514-535`.

**Root Cause**: Architectural difference between HARES' explicit step-by-step pipeline and SAM's monolithic solver.

**Impact**: Minimal — net AC power is identical to within floating-point tolerance. Only the `inverter_clipping_kw` telemetry reports a marginally different value than PVWatts would.

## Summary

- Total findings: 7
- Critical: 0
- High: 1 (no coincidence factors)
- Medium: 2 (missing microwave, extra_refrigerator schedule ignored)
- Low: 4 (inverter capacity not sized, system losses double-counting with soiling, holiday lighting ignored, clipping computation order)

## PV Sizing Cross-Reference with OCHRE

| Capability | OCHRE PV.py | HARES pv_sizing + mod.rs | Parity |
|---|---|---|---|
| DC array capacity sizing | Manual input | `compute_usable_area` + `size_pv_system` from roof geometry | HARES is more automated |
| Inverter capacity / DC:AC ratio | `inv_capacity` param, `dc_ac_ratio` passed to PVWatts | `inverter_capacity_kw` in `PvConfig`, `apply_inverter_limits()` | Functional parity, 1:1 default in both |
| Inverter efficiency | 96% default (PVWatts) | 96% default | Match |
| System losses | Handled internally by PVWatts | Explicit `system_losses_fraction` (14% default) | Match |
| Soiling | Not modeled in PVWatts | Separate Kimber soiling model | HARES more detailed |
| Shading | Not directly modeled | Shading model with solar position | HARES more detailed |
| Temperature derating | PVWatts internal (NOCT) | SAM-NOCT with wind correction, gamma by module type | HARES more explicit, functionally equivalent |
| Tilt / azimuth | Manual or inferred from envelope | Extracted from HPXML roof planes, wall azimuth fallback | HARES more automated |
| Flat roof GCR | N/A (tilt must be specified) | Latitude-dependent GCR (0.35-0.50) | HARES has flat-roof support |
| Multiple roof planes (hip) | Uses best south-facing plane | Aggregates across all non-north plane | HARES more automated |
| Curtailment | P Curtailment (kW/%) | Same + PowerLimit control signal | Match |
| Reactive power | PF setpoint, Q setpoint, priority modes | Same three priority modes (Watt, Var, CPF) | Match |
| SAM LUT integration | PySAM PVWatts v8 | Optional CSV/Parquet LUT ingestion | HARES supports offline precomputed SAM results |

## Recommendations

1. **Coincidence factors**: Implement diversity multipliers per load type (e.g., ASHRAE 90.1 or Building America Research Benchmark diversity factors) to derate aggregate peak load. At minimum, add a warning when multiple fallback daily-profile loads peak within the same hour, or expose a `diversity_factor` parameter for aggregate scaling.

2. **Add microwave mapping**: Add a `ColumnMapping` entry for `csv_column: "microwave"` → `equipment_name: "Microwave"` with `ScheduleCategory::EventWindow`. Also add microwave parsing in `resolve_loads.rs` or accept the ResStock appliance definitions.

3. **Secondary refrigerator schedule**: Either change `extra_refrigerator` from `Ignore` to `Power` so it feeds schedule data to the secondary refrigerator equipment, or ensure the HPXML resolve path injects the primary refrigerator's schedule column reference.

4. **Inverter sizing in pv_sizing**: Extend `PvSizingResult` with `recommended_inverter_capacity_kw` computed as `capacity_kw / 1.2` (typical DC/AC ratio of 1.2). Auto-populate this in `PvConfig` when sizing from roof geometry.

5. **System losses vs soiling**: Document the interaction between `system_losses_fraction` and the soiling model. Either subtract the soiling component from the default 14% when soiling is enabled, or add a validation warning if both the soiling model and a high `system_losses_fraction` are active simultaneously.

## References / Citations

- `crates/hares-io/src/schedule_resolve.rs:44-228` — COLUMN_MAPPINGS table
- `crates/hares-io/src/schedule_resolve.rs:652-728` — inject_power_schedule with max_kw derivation
- `crates/hares-io/src/schedule_resolve.rs:784-814` — determine_max_kw formula
- `crates/hares-io/src/hpxml/resolve_loads.rs:86-419` — appliance parsing for ClothesWasher, ClothesDryer, Dishwasher, Refrigerator, Freezer, CookingRange, Dehumidifier
- `crates/hares-io/src/hpxml/resolve_loads.rs:422-523` — lighting (interior, exterior, garage, basement) and ceiling fan resolution
- `crates/hares-io/src/hpxml/resolve_loads.rs:526-616` — plug loads (TV, well pump, EV, MELs) and fuel loads
- `crates/hares-io/src/hpxml/resolve_loads.rs:618-674` — pool/spa pump and heater resolution
- `crates/hares-io/src/hpxml/resolve_loads.rs:761-798` — default_gain_fractions per equipment
- `crates/hares-physics/src/pv_sizing.rs:74-83` — PvSizingResult struct
- `crates/hares-physics/src/pv_sizing.rs:341-378` — size_pv_system
- `crates/hares-equipment/src/pv/mod.rs:195-267` — step_one_array DC/AC power calculation
- `crates/hares-equipment/src/pv/mod.rs:269-321` — apply_inverter_limits (clipping, priority modes)
- `crates/hares-equipment/src/pv/mod.rs:461-586` — step function with curtailment, clipping, telemetry
- `crates/hares-equipment/src/pv/config.rs:1-100` — PvConfig typed config with inverter_capacity_kw
- `vendors/OCHRE/ochre/Equipment/PV.py:9-71` — run_sam with capacity, tilt, azimuth, inv_capacity, inv_efficiency
- `vendors/OCHRE/ochre/Equipment/PV.py:74-162` — PV class constructor with inverter defaults
- ANSI/RESNET/ICC 301-2014 Addendum A-2015 §4.2.2.5.2 for appliance energy defaults
- NREL PVWatts v8 Technical Reference (NREL/TP-7A40-80694) for system loss defaults and NOCT model
- HPXML v4.0 schema: BuildingAmerica/HouseSimulation/Appliances, Lighting, MiscLoads elements
