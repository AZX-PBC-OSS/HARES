# Event-based load wet appliance model completeness
**Review ID**: equip-loads-03
**Category**: equipment-loads
**Date**: 2026-05-26

## Files Reviewed
crates/hares-equipment/src/event_load.rs

## Vendor/Reference Files Consulted
vendors/OCHRE/ochre/Equipment/EventBasedLoad.py vendors/OCHRE/ochre/Equipment/WetAppliance.py

## Findings

### Finding 1: [Severity: high]
**Description**: The `WetAppliance` model in `event_load.rs` treats hot water draw as a uniform rate spread across the entire cycle duration (line 971–977: `hot_water_draw_volume_l / total_cycle_duration_s`), with no per-phase water draw distinction. In physical appliances, hot water is drawn only during fill and wash phases — not during spin, drain, or dry phases. This means the model under-reports hot water draw during fill/wash (by spreading it thinly across the full cycle) and falsely draws hot water during spin/dry phases where none occurs. OCHRE's modern model (`EventBasedLoad` + `ScheduledLoad`) avoids this by providing independent time-series columns for `hot_water_clothes_washer` and `hot_water_dishwasher`, which track actual draw timing. OCHRE's legacy `WetAppliance` class does not model hot water at all.

**Code Location**: `event_load.rs:971–977` (hot_water_draw_rate_kg_s initialisation), `event_load.rs:891–899` (uniform draw emission across all phases)

**Root Cause**: The model stores a single `hot_water_draw_rate_kg_s` scalar (line 205). Per-phase water draw would require extending `CyclePhase` to include a `water_draw_rate_kg_s` field or a `has_water_draw` boolean that gates fluid accumulation per phase.

**Impact**: Water heater demand profiles will not align with appliance cycle phases. During spin/drain phases, the water heater will receive phantom draw demand, prematurely cooling the tank. During fill/wash phases, the peak draw rate will be artificially low, potentially masking short-duration high-flow demands that can cause tank depletion.

### Finding 2: [Severity: high]
**Description**: The `WetAppliance` struct (used for "Clothes Dryer") has no concept of `vented` vs `unvented` dryer type within the runtime model itself. The `vented` flag is parsed in the HPXML layer (`resolve_loads.rs:189–192`) and used to compute gain fractions (`frac_lost`), but `event_load.rs` stores only `sensible_gain_fraction` and `latent_gain_fraction` and treats all dryer instances identically. There is no dryer type enum, no exhaust port, and no differentiation between vented gas, vented electric, and unvented condenser dryers in the runtime model.

**Code Location**: `event_load.rs:182–219` (WetAppliance struct definition — no `vented` field, no `dryer_type` field), `event_load.rs:833–889` (update_outputs — no dryer-type-specific routing)

**Root Cause**: The HPXML parser (resolve_loads.rs:194–212) pre-computes `frac_lost` (85% for vented, 0% for unvented) and reduces `sensible_gain_fraction` and `latent_gain_fraction` accordingly before passing them to the runtime model. This approach correctly eliminates the exhausted energy from the zone heat balance but does not distinguish between exhaust routing and condensation recovery at the physics level. For condenser dryers, the latent heat released by condensing moisture should become sensible heat in the zone, but the model's `frac_lost = 0.0` approach reports the original latent fraction without accounting for condensation recovery.

**Impact**: 
- **Vented gas dryers**: The combustion latent heat that should be exhausted outdoors (85% of fuel energy) is correctly excluded from zone gains via `frac_lost`, but the remaining 15% still includes a latent component from gas combustion humidity. OCHRE distinguishes the sensible fraction for gas vs electric based on a BTU-weighted blend of 0.90 (electric parasitic) and 0.8894 (gas combustion); HARES uses a constant 0.89 for gas. This is documented as "stable across CEF values" but is a simplification.
- **Unvented condenser dryers**: These should reject the latent heat of condensation to the condenser (converting it to sensible heat) and drain the liquid water. OCHRE's model (`frac_lat = 0.10` for unvented electric, `0.11` for unvented gas) reports latent gain even for unvented models, which does not match condenser dryer physics. HARES inherits this same issue since both parsers compute `frac_lat = 1 - frac_sens - frac_lost` identically. The moisture that should condense and go to drain is instead routed to the indoor humidity solver as latent gain.

### Finding 3: [Severity: medium]
**Description**: OCHRE's `parse_clothes_dryer` in `hpxml.py:1283–1286` converts `EnergyFactor` to `CombinedEnergyFactor` using `CEF = EF / 1.15` when only EF is present. HARES implements this conversion in `resolve_loads.rs:226–229` (`params.get("energy_factor").map(|ef| ef / 1.15)`), which is correct. However, the conversion is only applied in the `annual_electric_kwh` default computation, not as a general parameter translation. The `energy_factor` key remains in the params alongside `combined_energy_factor`, and `event_load.rs` does not read either key — it operates on pre-computed phase power values. This means the CEF/EF distinction has no effect on the runtime model behaviour, which is correct since the phase power values should already account for efficiency.

**Code Location**: `resolve_loads.rs:218–230` (CEF/EF conversion in annual energy defaults), `event_load.rs:946–1018` (WetAppliance init — reads phase power, not efficiency metrics)

**Root Cause**: Architectural separation: efficiency metrics inform annual energy estimates used for HPXML compliance and sizing defaults, but the event-based runtime model is driven by per-phase power (kW) and duration (s), making CEF/EF translation a parser-layer concern only.

**Impact**: Low. The CEF/EF conversion is correctly applied for annual energy estimation. The runtime model does not need efficiency metrics. However, if the system ever needs to auto-derive phase power from CEF (e.g., when HPXML provides only CEF without explicit phase profiles), the current code has no derivation path — it would fall back to the default single-phase 1.0 kW / 900 s cycle.

### Finding 4: [Severity: medium]
**Description**: The dishwasher's hot water draw is communicated to the water heater model via `PortContribution::Fluid` with `DHW_DEMAND_LOOP` (line 892), which triggers water heater operation. However, the fluid contribution sends `supply_temp_c: 0.0` and `return_temp_c: 0.0` (lines 895–896), leaving the water heater to infer the draw temperature from its own configuration (`hot_draw_temp_c`). OCHRE's schedule-based model uses separate hot water time-series columns that vary in flow rate, aligned with actual appliance cycle demands. HARES's uniform-rate approach does not capture the bursty nature of dishwasher fill cycles, which can draw 1–3 L/min for 1–5 minutes at the fill valve's full flow rate.

**Code Location**: `event_load.rs:891–899` (fluid port contribution), `event_load.rs:968–972` (draw rate from total volume / total duration)

**Root Cause**: The single scalar `hot_water_draw_rate_kg_s` represents the time-averaged draw rate across all phases. Dishwasher fill phases typically draw at the plumbing fixture's flow rate (~6–12 L/min) for short durations, not at the ~0.1–0.3 L/min that the uniform spread would produce for a ~60-minute cycle with 6 L total draw.

**Impact**: Water heater transient response (tank temperature stratification, element cycling) will not respond to realistic draw patterns. Short-duration, high-flow fill events are replaced by low-flow, continuous draw that is easier for the water heater to satisfy, potentially underestimating comfort issues (e.g., the last rinse being cold because the tank hasn't recovered from the fill draw).

### Finding 5: [Severity: medium]
**Description**: The clothes dryer model in `event_load.rs` uses the same `CyclePhase` structure (`power_kw`, `duration_s`) for all dryer types, but there is no support for the multiphase per-second time-series profiles that OCHRE's `EventDataLoad` class loads from CSV. OCHRE's `Clothes Dryer/Event Schedules.csv` defines 10 distinct cycle types (low_heat, delicate, cotton, permanent press, high_heat, etc.) with sub-minute resolution power traces. HARES stores similar CSV data at `defaults/loads/clothes_dryer_events.csv` but the `WetAppliance` model does not load it. The `event_power_kw_series` mechanism (extract_events_from_kw_series) can ingest a scalar kW time series but does not select from multiple cycle types as OCHRE's `add_event_types()` does.

**Code Location**: `event_load.rs:55–78` (extract_events_from_kw_series — averages contiguous non-zero regions), `event_load.rs:1006–1017` (event_power_kw_series ingestion in WetAppliance::init), `event_load.rs:1198–1218` (register_with_registry — all Clothes Dryer instances use plain WetAppliance)

**Root Cause**: The HARES `WetAppliance` uses a different model architecture than OCHRE's `EventDataLoad`. OCHRE assigns each event a cycle type probabilistically, then steps through a per-second power trace. HARES uses per-phase summary values (`phase_0_power_kw`, `phase_0_duration_s`, etc.) that average each phase's aggregate power and duration. The HARES CSV data in `defaults/loads/` exists but is not wired to the `WetAppliance` init path.

**Impact**: DR strategies that rely on shifting end times of specific cycle types, or analysing submetered power during specific dryer cycle phases, cannot differentiate between dryer modes. All cycles collapse to the same per-phase parameters. Multi-cycle-type diversity (essential for aggregate load diversity across a housing stock) is lost.

### Finding 6: [Severity: low]
**Description**: The `Clothes Washer` and `Dishwasher` registrations (lines 1203–1210) use the `WetAppliance` struct with per-phase power/duration config keys. However, OCHRE's modern model handles clothes washer and dishwasher via `ScheduledLoad` (simple time-series schedule) rather than `EventDataLoad` (event-based with per-second profiles). The `EventDataLoad` class in OCHRE is only used for Clothes Dryer and Cooking Range, which have complex multi-mode event traces. For clothes washer and dishwasher, OCHRE uses the analytical approach in `hpxml.py` to derive annual energy and water draw, then applies a simple schedule. The HARES decision to model all three via `WetAppliance` with per-phase power/duration is a superset of OCHRE's capability (providing more detail than OCHRE's scheduled approach for washer/dishwasher), but the default single-phase cycle (1.0 kW / 900 s) may not produce realistic per-phase behaviour unless comprehensive phase parameters are provided.

**Code Location**: `event_load.rs:1203–1210` (Clothes Washer and Dishwasher registered as WetAppliance), `event_load.rs:1344–1351` (parse_cycle_phases — falls back to single default phase)

**Root Cause**: The model is intentionally more capable than OCHRE's for these appliance types, but default parameters are minimal. Realistic modelling requires HPXML-derived phase parameters, which the HPXML parser does not generate for washer/dishwasher (it only produces annual kWh and water draw volume).

**Impact**: Without properly parameterised multi-phase cycles, the clothes washer and dishwasher will simulate as single-phase constant-power loads with uniform water draw, missing the fill/wash/rinse/spin transitions that matter for both electrical demand shape and water heater interaction timing.

### Finding 7: [Severity: low]
**Description**: OCHRE's `WetAppliance.py` (the legacy Monte Carlo profile generator) supports reactive power (kVAr) alongside real power (kW) via a 2-column `PQ_Demand_Profile`. The HARES `WetAppliance::update_outputs()` always reports `reactive_power_kvar: 0.0` (line 867) and does not accept per-phase reactive power parameters. OCHRE's ZIP parameter defaults at `ZIP Parameters.csv:20` give clothes dryers a power factor of 0.99 (nearly unity), so this omission is minor for dryers, but clothes washers at `ZIP Parameters.csv:19` have `pf = 0.65` — a significant reactive component that is not modelled.

**Code Location**: `event_load.rs:866–869` (fixed reactive_power_kvar: 0.0), `event_load.rs:136` (CyclePhase has only power_kw and duration_s)

**Root Cause**: `CyclePhase` stores only `power_kw` and `duration_s` with no reactive power field. The `PortContribution::Electrical` always reports zero reactive power.

**Impact**: Power factor distortions from clothes washer operation (significant inductive motor loads during spin cycles) are not captured in electrical panel modelling or feeder-level power quality analysis.

### Finding 8: [Severity: low]
**Description**: `update_outputs` for both `EventBasedLoad` and `WetAppliance` reports `radiant_gain_w: 0.0` (lines 416, 885). OCHRE similarly sets `"Radiative Gain Fraction (-)": 0` in its HPXML parser for all wet appliances (hpxml.py:1330 for dryers, and equivalent for washers/ dishwashers). This is physically reasonable since most heat from these appliances is convective, but clothes dryers with exposed drums and cooking ranges with radiant elements do emit some radiative heat. The omission is consistent with OCHRE's approach.

**Code Location**: `event_load.rs:416` (EventBasedLoad radiant_gain_w: 0.0), `event_load.rs:885` (WetAppliance radiant_gain_w: 0.0)

**Root Cause**: Both HARES and OCHRE use zero radiative gain fraction for these appliance types. This is a modelling simplification from the reference implementation.

**Impact**: Minor; zone surface temperature response from appliance radiative heating is not captured, but the convective model captures the bulk of the thermal effect.

## Summary
- Total findings: 8
- Critical: 0
- High: 2
- Medium: 3
- Low: 3

## Recommendations
1. **Add per-phase hot water draw** (Finding 1, 4): Extend `CyclePhase` to include an `has_water_draw: bool` field (or `water_draw_fraction: f64`) that gates whether the `hot_water_draw_rate_kg_s` is active during that phase. Compute the per-phase draw rate only for phases where `has_water_draw` is true, dividing the total water volume by the sum of water-draw-enabled phase durations. This eliminates phantom hot water draws during spin/drain stages and concentrates the draw where it physically occurs, producing more realistic peak draw rates.

2. **Add dryer type awareness to the runtime model** (Finding 2): Introduce a `DryerType` enum (VentedElectric, VentedGas, UnventedCondenser) to `WetAppliance`. For unvented condenser dryers, set `latent_gain_fraction = 0.0` unconditionally (condensate goes to drain, not air), and add the latent heat of condensation to `sensible_gain_fraction` (since the condenser rejects both sensible and latent energy as sensible heat to the zone). OCHRE's model does not make this distinction either, but correcting it here would improve HARES's fidelity beyond the reference.

3. **Wire the clothes dryer event CSV profiles** (Finding 5): Implement cycle type selection and per-second profile stepping for the Clothes Dryer equipment type, loading from `defaults/loads/clothes_dryer_events.csv`. Alternatively, pre-process the CSV into per-phase summary parameters (averaging each distinct operating segment) and surface those as the config keys already supported by `parse_cycle_phases`. The key improvement is supporting multiple cycle types with probabilistic selection per event.

4. **Generate default washer/dishwasher phase parameters from HPXML** (Finding 6): Extend the HPXML parser in `resolve_loads.rs` to produce multi-phase parameters for clothes washer and dishwasher, using reference data from ENERGY STAR test procedures or AHAM standards to derive per-phase power, duration, and water draw timing from the annual energy and water draw values already computed.

5. **Add reactive power support** (Finding 7): Add an optional `reactive_power_kvar` field to `CyclePhase` and report it in `PortContribution::Electrical`. Use the ZIP parameter defaults to estimate per-phase reactive power for clothes washers (pf ~0.65) and other appliances.

## References / Citations
- OCHRE `hpxml.py:1281–1333` — `parse_clothes_dryer`: CEF/EF conversion, vented/unvented fraction logic, electric vs gas sensible fraction
- OCHRE `hpxml.py:1243–1278` — `parse_clothes_washer`: annual energy, water draw, heat gain fractions (frac_lost=0.70, frac_sens=0.27, frac_lat=0.03)
- OCHRE `hpxml.py:1336–1370` — `parse_dishwasher`: annual energy, water draw, heat gain fractions (frac_lost=0.40, frac_sens=0.30, frac_lat=0.30)
- OCHRE `EventDataLoad.__init__` and `add_event_types` (EventBasedLoad.py:320–376) — multi-cycle-type selection and duration/energy multiplier logic
- OCHRE `ZIP Parameters.csv:19–20` — clothes washer pf=0.65, clothes dryer pf=0.99
- HARES `resolve_loads.rs:24–35` — dryer exhaust and sensible gain constants (DRYER_EXHAUST_FRACTION_VENTED=0.85, DRYER_GAS_SENSIBLE_GAIN=0.89, DRYER_ELECTRIC_SENSIBLE_GAIN=0.90)
- HARES `resolve_loads.rs:183–259` — HPXML dryer parsing with CEF/EF conversion, vented flag, annual energy defaults
- HARES `resolve_loads.rs:261–302` — HPXML dishwasher parsing with annual energy and water draw defaults
- HARES `event_load.rs:55–78` — extract_events_from_kw_series (deterministic event extraction from kW time series)
- HARES `event_load.rs:968–977` — hot water draw rate computed as uniform spread across full cycle duration
- HARES `event_load.rs:891–899` — fluid port contribution to DHW_DEMAND_LOOP
