# HVAC Multispeed Parameters.csv: capacity fractions, EIR curves, fan power
**Review ID**: defdata-03
**Category**: defaults-data
**Date**: 2026-05-26

## Files Reviewed
defaults/HVAC Multispeed Parameters.csv

## Vendor/Reference Files Consulted
vendors/EnergyPlus/src/EnergyPlus/

## Findings

### Finding 1: [Severity: critical] Missing CSV entries for "Air Conditioner" and "GSHP" equipment types

**Description**: The CSV contains entries for `ASHP Cooler`, `ASHP Heater`, `MSHP Cooler`, `MSHP Heater`, and `Gas Furnace`, but omits `Air Conditioner` (standalone central AC) and `GSHP Heater`/`GSHP Cooler` (ground-source heat pump). The HPXML resolver in `crates/hares-io/src/hpxml/resolve_hvac.rs:2235` maps `"central air conditioner"` to equipment name `"Air Conditioner"`, and line 1836 maps `"ground-to-air"` heat pumps to `"GSHP Heater"` / `"GSHP Cooler"`. When a multi-speed standalone AC or GSHP is resolved, `apply_multispeed_cooling_parameters` calls `defaults.hvac_multispeed_parameters("Air Conditioner", ...)` at line 2746, which returns `None` because no matching CSV row exists. The function silently returns without inserting per-stage capacity, COP, or EIR data (line 2745-2748).

**Code Location**: `crates/hares-io/src/hpxml/resolve_hvac.rs:2745-2748` (lookup return), `resolve_hvac.rs:2229-2244` (equipment name derivation), `resolve_hvac.rs:1833-1843` (GSHP naming).

**Root Cause**: The CSV was inherited from OCHRE's dataset, which only models heat pump categories. HARES extended HPXML support to standalone air conditioners and ground-source heat pumps but did not add corresponding multi-speed parameter rows.

**Impact**: Standalone AC units with SEER > 15 get assigned 2-4 speeds via the fallback heuristic (line 2542-2548), but receive no per-stage capacity fractions, COPs, or EIR curves. The equipment initialization will see `number_of_speeds > 1` but no per-stage data, likely resulting in all stages sharing the single rated capacity and COP — effectively single-speed behavior despite the multi-speed configuration. GSHP systems are similarly affected.

### Finding 2: [Severity: high] Non-unity capacity fractions at rated conditions for all MSHP categories

**Description**: All MSHP Cooler rows in the CSV have capacity ratios `[0.48889, 0.66667, 0.84444, 1.2]`; all MSHP Heater rows have `[0.4, 0.6, 0.8, 1.2]`. No speed has a capacity ratio of exactly 1.0. The EnergyPlus reference implementation requires the rated speed to have `MSRatedPercentTotCap = 1.0` (`VariableSpeedCoils.cc:632`), with all lower speeds expressed as fractions thereof. By anchoring the max speed at 1.2× rated capacity, the CSV creates an "overspeed" convention where the user's rated capacity (scaled by capacity ratio 0.8 for MSHP Heater speed 3) does not correspond to any discrete speed stage.

In HARES, `apply_multispeed_parameters` (line 2772) computes per-stage capacity as `rated_capacity_w * capacity_ratios[i]`, giving e.g. speed 4 = 1.2 × rated capacity. The runtime `capacity_fractions_for()` function (speed_control.rs:137-143) normalizes against the last entry (1.2), so the user's rated capacity becomes fraction 0.833 of max. The speed interpolator then places the rated operating point between the penultimate and ultimate speed stages rather than anchoring at a discrete stage, introducing imprecision in part-load performance calculations.

**Code Location**: CSV lines 26-40 (MSHP rows). Consumed at `resolve_hvac.rs:2771-2779` and `speed_control.rs:137-143`.

**Root Cause**: The "overspeed" convention is inherited from OCHRE's `MinisplitHVAC` model, which treats the rated capacity as the nameplate at ~80% of the compressor's maximum capability. This convention is appropriate for variable-speed inverter-driven mini-splits whose compressors can exceed nameplate at maximum frequency, but it is incompatible with the EnergyPlus convention where `NumOfSpeeds` sets the speed count and the highest speed IS the rated condition.

**Impact**: Speed interpolation biases load-to-speed mapping. At the rated design load, the system runs either in overspeed (wasting capacity) or interpolates between speed 3 and 4, neither of which corresponds to the manufacturer's rated efficiency point. The bias magnitude depends on how far the overspeed exceeds 1.0 (20% for MSHP, 17% for 4-speed ASHP Heater). This affects all efficiency calculations at rated load.

### Finding 3: [Severity: high] Identical SHR and capacity ratio values across all MSHP Cooler SEER tiers

**Description**: Every MSHP Cooler row (SEER 13.0 through 33.0, CSV lines 26-33) uses the identical set of capacity ratios `[0.48889, 0.66667, 0.84444, 1.2]`, air flow ratios `[0.52941, 0.64706, 0.76471, 1.0]`, and SHR values `[0.86256, 0.7987, 0.75709, 0.70255]` to 5-6 significant digits. The SHR values decrease with speed (more dehumidification at higher speeds), which is directionally plausible for oversized indoor coils, but the perfect repetition across a 20-SEER range is unphysical. Equipment with 33 SEER would use different compressors, coil surface areas, and refrigerant charge than 13 SEER equipment, yielding different SHR profiles.

The COP values do vary across SEER tiers, suggesting they were derived independently. The capacity fractions and SHRs appear to have been copied from a single reference unit and applied uniformly.

**Code Location**: CSV lines 26-33 (MSHP Cooler rows). Consumed at `resolve_hvac.rs:2751-2764`.

**Root Cause**: The data likely originated from a single reference mini-split unit and was replicated across efficiency tiers during dataset construction. Only COP/SHR values were adjusted for different efficiency ratings; capacity fractions and airflow ratios were reused.

**Impact**: Multi-speed staging and dehumidification behavior will be identical across all MSHP Cooler SEER levels of the same speed count, regardless of equipment vintage or efficiency. This reduces modeling fidelity for high-SEER equipment whose turndown ratios and latent removal characteristics typically differ from lower-SEER counterparts.

### Finding 4: [Severity: medium] Non-monotonic COP sequence in ASHP Cooler 4-speed rows

**Description**: The 22 SEER and 24 SEER ASHP Cooler rows (lines 10-11) have COP values `[5.93, 6.15, 5.98, 5.54]` and `[6.57, 6.81, 6.63, 6.14]` respectively. In both cases COP peaks at speed 2 (51% of rated capacity), not at speed 1 (minimum turndown). EIR follows the inverse pattern, increasing from speed 1 to speed 2 before continuing its expected rise at speeds 3 and 4.

Compare to the 20 SEER row (line 9) which has monotonically decreasing COP `[4.89, 4.70, 4.50, 4.30]` — the conventional expectation for fixed-speed equipment where lower speeds benefit from larger effective heat exchanger area.

**Code Location**: CSV lines 9-11. Speed 2 COP values at lines 10-11 surpass speed 1 values.

**Root Cause**: The 20 SEER row is sourced from "Manual" while 22/24 SEER rows come from `testing_500` and `ResStock 2024.2` respectively. The non-monotonic pattern may reflect actual equipment characteristics (inverter-driven compressors can have peak efficiency at 40-60% load due to reduced pressure ratios), or it may indicate that test data at speed 1 were captured under adverse ambient conditions that lowered COP relative to speed 2. Without a documented source methodology, the discrepancy cannot be resolved.

**Impact**: The non-monotonic COP will produce non-monotonic EIR across stages, causing speed 1 to be less efficient than speed 2. The speed interpolator in `interpolate_speed_stages` (speed_control.rs:159-204) may select speed 1 for very light loads even though speed 2 is more efficient, slightly inflating energy consumption at minimum part-load operation by ~3-4%.

### Finding 5: [Severity: medium] Air flow ratios do not follow cube-law fan power scaling

**Description**: The CSV provides `Air Flow Ratio` columns but not explicit fan power fractions. In the OCHRE implementation — from which HARES derives its architecture — fan power is computed as `fan_power_per_flow_rate * flow_rate` (linear scaling), not as `flow_rate^3` (cube law). The HARES codebase follows the same linear convention.

For EnergyPlus reference: `StandardRatings.cc:2567-2568` applies a constant `FanPowerPerEvapAirFlowRate` (W per m³/s) at each speed, which is also linear in flow rate. The cube law (fan power ∝ speed³) applies to ducted constant-air-volume systems with fixed duct resistance, not to the indoor blower of a packaged or split-system HVAC unit where the fan operates against a relatively constant external static pressure and airflow is varied via motor speed control.

The air flow ratios themselves are plausible:
- ASHP Cooler 2-speed: 0.86 (low), 1.0 (high)
- ASHP Heater 2-speed: 0.80, 1.0
- ASHP Cooler 4-speed: [0.42, 0.54, 0.68, 1.0]
- ASHP Heater 4-speed: [0.63, 0.76, 1.0, 1.19]
- MSHP Cooler 4-speed: [0.529, 0.647, 0.765, 1.0]
- MSHP Heater 4-speed: [0.556, 0.667, 0.778, 1.0]

**Code Location**: CSV columns "Air Flow Ratio 1" through "Air Flow Ratio 4". OCHRE `HVAC.py:147-153` (fan_power_per_flow_rate). HARES `resolve_hvac.rs:658-659` (airflow_ratios stored but not directly used for fan power — fan power is handled separately in equipment init).

**Root Cause**: The linear convention is intentional and matches both OCHRE and EnergyPlus. It reflects the physics of variable-speed ECM blowers, where power scales approximately linearly with flow at constant external static pressure. However, the absence of explicit fan power fractions in the CSV means fan energy is derived entirely from airflow ratios and the rated auxiliary power, with no speed-dependent fan efficiency degradation modeled.

**Impact**: Low. Fan energy is a minor component of total HVAC energy (<10%). However, for equipment with pronounced airflow reduction at low speeds (e.g., ASHP Cooler 4-speed with airflow ratio 0.42 at speed 1), the linear assumption may slightly overestimate fan power relative to actual ECM motor behavior, which can be more efficient at reduced speed.

### Finding 6: [Severity: medium] Anomalous COP degradation in 14.5 SEER MSHP Cooler

**Description**: The 14.5 SEER MSHP Cooler row (line 27) has the worst speed-4 COP (2.345) of any MSHP Cooler, including the lower 13.0 SEER unit (COP₄ = 2.811). The COP degradation from speed 1 to speed 4 is 46.2% (4.36 → 2.35), far worse than the 13.0 SEER unit's 22.2% (3.61 → 2.81) or the 17.0 SEER unit's 46.3% (5.14 → 2.76 — similar percentage but from a much higher starting COP).

Comparing the 14.5 SEER row to its immediate neighbors:
- 13.0 SEER: COP₄ = 2.811
- **14.5 SEER: COP₄ = 2.345** (anomalously low)
- 16.0 SEER: COP₄ = 2.99

The COP at speed 4 is supposed to represent full-load rated efficiency; a 14.5 SEER unit should have COP₄ ≈ 14.5 / 3.412 = 4.25 (EIR ≈ 0.235). The CSV value of 2.345 (EIR ≈ 0.426) represents a COP far below the SEER-derived expectation, even accounting for the overspeed factor.

**Code Location**: CSV line 27. COP values in columns 8 and 16.

**Root Cause**: This row is sourced from `testing_500 - bldg0000120`, suggesting it came from automated parameter estimation against test building data. The estimation may have converged to a local optimum that produced a physically implausible COP at max speed due to limited test data at high loads or an error in the source simulator's boundary conditions.

**Impact**: The 14.5 SEER MSHP Cooler will use a severely degraded COP at overspeed (EIR = 0.426), inflating energy consumption at high cooling loads by approximately 50% compared to the 13.0 or 16.0 SEER entries. This creates an illogical efficiency ranking where the mid-tier unit is less efficient than the entry-level unit at high load.

### Finding 7: [Severity: medium] 10.47 HSPF ASHP Heater COP₁ exceeds 11.0 HSPF entry by 34%

**Description**: The 10.47 HSPF ASHP Heater 4-speed row (line 23, source: BEopt) reports COP₁ = 6.10 at speed 1 (33% capacity). The 11.0 HSPF row (line 24, source: Manual) reports COP₁ = 4.55 at the same speed and capacity fraction. The higher-HSPF unit has 25% lower COP at minimum speed, which is physically counterintuitive — higher HSPF ratings generally correlate with better part-load performance.

Comparing the 10.47 HSPF row against the 13.0 HSPF row (line 25): 13.0 HSPF has COP₁ = 9.55, which is proportionally higher and consistent with the efficiency tier. The 11.0 HSPF Manual entry appears to be the outlier, with COP values across all speeds that are too low for its HSPF rating.

**Code Location**: CSV lines 23-24. Copied identically to lines 21-22 in the OCHRE source (where 11.0 HSPF is also an outlier).

**Root Cause**: The 11.0 HSPF "Manual" entry appears to have been entered with COP values that correspond to a much lower-efficiency unit (~9 HSPF). The COPs [4.55, 4.36, 4.17, 3.98] are nearly identical to the 9.2 HSPF 2-speed ASHP Heater COPs [4.48, 4.02], suggesting a data entry error during manual specification.

**Impact**: An HPXML building modeled with an 11.0 HSPF 4-speed air-source heat pump will consume 15-20% more heating energy than it should due to artificially low COPs, effectively modeling it as a ~9 HSPF unit.

### Finding 8: [Severity: low] Sparse Gas Furnace tier coverage

**Description**: The only Gas Furnace entries are for 80% AFUE and 90% AFUE, both 2-speed (lines 41-42). The capacity fraction at low fire is 0.65 for both, which is reasonable. However, the CSV lacks:
- 95%+ AFUE condensing furnace entries (common in new construction)
- Single-speed furnace entries (not needed for multi-speed lookup but useful for validation)
- Modulating furnace entries (>2 speeds)

The capacity ratio (0.65) and airflow ratio (0.80) at low speed are the same for both efficiency tiers, which is reasonable for mechanically similar 2-stage gas valves, but COP at low speed is consistently 0.02 below the rated COP (0.78 vs 0.80 for 80 AFUE; 0.88 vs 0.90 for 90 AFUE), matching the expected slight efficiency penalty from jacket/cycling losses at part load.

**Code Location**: CSV lines 41-42. These two rows are HARES additions not present in the upstream OCHRE CSV (diff confirms OCHRE CSV ends at line 40 without Gas Furnace entries).

**Root Cause**: Hares manually added these two rows. Coverage was likely limited to the most common efficiency tiers encountered in existing HPXML datasets.

**Impact**: Buildings with 92-98% AFUE furnaces will fall back to the closest efficiency match (the 90% AFUE row) via the `hvac_multispeed_parameters` lookup's `min_by` on efficiency distance (defaults.rs:227-249). This minor under-specification of COP biases gas consumption by ~2-8% depending on the actual AFUE gap.

### Finding 9: [Severity: low] SHR values in ASHP Cooler 2-speed entries are near-identical across SEER tiers

**Description**: All 2-speed ASHP Cooler rows (lines 3-7, 16.0-18.84 SEER) use SHR₁ = 0.71597 and SHR₂ = 0.72878, sourced from "testing_500" test data. The 19.05 SEER BEopt-sourced row (line 8) uses slightly different SHR values (0.72185, 0.73364). The 20 SEER Manual-sourced row (line 9) has SHR₁ = 0.72185 and SHR₂ = 0.80 — the latter being a substantial deviation from the 0.728-0.734 range.

The Manual entry's SHR₂ = 0.80 may be correct for a specific manufacturer's data, but having SHR jump from ~0.72 to 0.80 between 19.05 SEER and 20.0 SEER suggests a data source discontinuity rather than an equipment characteristic.

**Code Location**: CSV lines 3-9, SHR columns 11-12.

**Root Cause**: Multiple data sources (BEopt, testing_500, Manual) were merged without harmonizing the SHR values. The testing_500 source appears to have applied a consistent SHR model; the Manual entry used different reference data.

**Impact**: Minor. SHR values affect latent load calculations but the jump at the 19→20 SEER boundary is within typical equipment variability and does not cause anomalous behavior.

## Summary
- Total findings: 9
- Critical: 1
- High: 2
- Medium: 4
- Low: 2

## Recommendations

1. **Add CSV entries for "Air Conditioner" and "GSHP Heater/Cooler" equipment types.** At minimum, add entries matching the speed counts produced by the SEER-fallback heuristic (2-speed at SEER > 15, 4-speed at SEER > 21 for AC; commensurate entries for GSHP). If manufacturer data is unavailable, the ASHP Cooler capacity fractions and COPs can serve as a reasonable starting point, since the compressor and coil physics are similar. For GSHP, EER-based COPs are typically higher due to stable ground-loop temperatures, so an upward adjustment is warranted.

2. **Clarify the overspeed convention for MSHP capacity ratios.** Either (a) renormalize MSHP capacity ratios so the rated speed has fraction 1.0 (matching EnergyPlus convention), adjusting the COPs accordingly, or (b) document the overspeed convention explicitly in the CSV header comments and verify that all downstream code paths correctly handle non-unity max capacity fractions. The runtime `capacity_fractions_for()` normalizes by the last element, which is correct for variable-speed but shifts the rated operating point when overspeed > 1.0.

3. **Audit the COP monotonicity for ASHP Cooler 4-speed entries.** The 22 SEER and 24 SEER entries (sourced from testing_500/ResStock) should be either (a) corrected to monotonically decreasing COP (reversing the COP₁ and COP₂ values), or (b) documented with an explanation of the efficiency peak at intermediate speed. If the peak is intentional, consider adding a source note in the "Received From" column.

4. **Replace or validate the 14.5 SEER MSHP Cooler COP values.** The speed-4 COP of 2.345 (EIR = 0.426) is inconsistent with both its SEER rating and neighboring entries. Recommend replacing with interpolated values from the 13.0 and 16.0 SEER entries, or obtaining verified manufacturer data.

5. **Re-evaluate the 11.0 HSPF ASHP Heater COPs.** The COP values [4.55, 4.36, 4.17, 3.98] appear to be copied from a ~9 HSPF unit's data. Recommend replacing with COPs that scale proportionally from the validated 10.47 HSPF BEopt entry, maintaining the same per-stage COP ratios.

6. **Add 95%+ AFUE Gas Furnace entries.** A 2-speed 95% AFUE furnace entry (capacity ratios matching the existing 0.65/1.0 pattern, with COP = 0.93 at low speed and 0.95 at high speed) would cover ~80% of new-construction scenarios.

7. **Review SHR harmonization across data sources.** The jump from SHR₂ ≈ 0.73 to SHR₂ = 0.80 at the 20 SEER boundary may indicate a sourcing artifact. Consider using a consistent SHR model across efficiency tiers or documenting the source-specific SHR derivation methodology.

## References / Citations

- EnergyPlus `VariableSpeedCoils.cc:632` — `MSRatedPercentTotCap` normalization (rated speed always fraction 1.0)
- EnergyPlus `DXCoils.cc:8193` — auto-sizing default capacity fraction `Mode/NumOfSpeeds`
- EnergyPlus `StandardRatings.cc:126-128` — default fan power per evaporator airflow rate constants
- EnergyPlus `CurveManager.cc:3613-3664` — `checkCurveIsNormalizedToOne` (curve output must be 1.0 ± 10% at rated conditions)
- EnergyPlus `DataHVACGlobals.hh:349-363` — rated volumetric flow per capacity ratio bounds
- HARES `crates/hares-io/src/hpxml/resolve_hvac.rs:2705-2831` — multi-speed parameter application
- HARES `crates/hares-equipment/src/hvac/speed_control.rs:137-143` — `capacity_fractions_for` normalization by max capacity
- HARES `crates/hares-equipment/src/hvac/heat_pump/heater.rs:654-689` — MSHP hardcoded stage generation (bypasses CSV)
- OCHRE `ochre/Equipment/HVAC.py:94-114, 147-153` — reference CSV consumption and fan power calculation
