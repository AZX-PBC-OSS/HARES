# HVAC and water heater autosizing methodology and iteration
**Review ID**: core-11
**Category**: core
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-core/src/dwelling/autosize.rs` (primary: autosizing orchestrator)
- `crates/hares-core/src/dwelling/solver_builder.rs` (builds the ThermalSolver that provides the autosize methods)
- `crates/hares-envelope/src/thermal_solver/stepping.rs` (actual `autosize_capacity` / `autosize_capacity_cooling` implementations)
- `crates/hares-envelope/src/state_space.rs` (`solve_for_scalar_input` — the one-step algebraic solve)
- `crates/hares-io/src/hpxml/resolve_water_heater.rs` (water heater config, checked for autosizing)
- `crates/hares-core/src/dwelling/mod.rs:1020-1040` (call site invoking `autosize_equipment_capacities`)

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/SizingManager.cc` (lines 1–700; in particular lines 122–126, 285–390, 786–787)

## Findings

### Finding 1: No iterative design-day simulation — single algebraic solve instead [Severity: high]
**Description**: EnergyPlus (SizingManager.cc:122–126) runs full design-day simulations where each zone is equipped with an "Ideal Loads" HVAC system. The simulation runs through actual ASHRAE design-day weather profiles (24-hour temperature/humidity/solar curves over multiple days, including warmup). The peak zone load observed across all timesteps becomes the equipment sizing value. HARES instead uses a single-step algebraic solve: it constructs a static input vector at one condition (design outdoor DB +, for cooling, July 21 solar noon) and calls `solve_for_scalar_input` (`state_space.rs:545–594`) to compute the one-timestep HVAC input needed to drive zone temperature from its current state to the target setpoint.

**Code Location**: `autosize.rs:130–132` (heating), `autosize.rs:205–213` (cooling); `thermal_solver/stepping.rs:32–76` (heating solver), `thermal_solver/stepping.rs:103–235` (cooling solver); `state_space.rs:545–594` (the algebraic solve).

**Root Cause**: The design decision to use a single-condition algebraic solve rather than running a design-day time series. A single-condition solve cannot capture:
- Diurnal variation in outdoor dry-bulb temperature (peak often occurs at 3–4 PM, not solar noon)
- Thermal mass time-lag effects (building mass delays peak load up to 2–3 hours)
- Coincidence of outdoor temperature peak with solar gain over the day

**Impact**: Cooling sizing at a single solar noon condition may underestimate or overestimate the true design load. The omission of thermal time-lag effects is particularly significant for high-mass buildings where the peak cooling load lags significantly behind peak solar gain. EnergyPlus captures these effects naturally by running full 24-hour design-day simulations with warmup.

### Finding 2: No internal gains in cooling sizing [Severity: high]
**Description**: The cooling autosizing path (`autosize_capacity_cooling` in `thermal_solver/stepping.rs:103–235`) computes solar gains through windows (clear-sky Perez model) and opaque surfaces but does not include internal heat gains from occupancy, lighting, or appliances. Per ACCA Manual J-2016, cooling design loads must include sensible and latent internal gains because they add to the total cooling requirement. EnergyPlus includes these automatically during its design-day simulations because internal gain schedules are part of the heat balance.

**Code Location**: `thermal_solver/stepping.rs:121–122` — `u_design.fill(0.0)` zeros all inputs before selectively setting outdoor temp, ground temp, and solar gains. No internal gain inputs are populated. Contrast with the heating path at line 41–43 which explicitly zeros all inputs including internal gains (correct for heating, incorrect for cooling).

**Root Cause**: The cooling autosize method copies the same "zero all inputs" pattern from the heating method, then selectively adds back solar. It does not add default internal gain assumptions (e.g., 2 people + 1.5 W/ft² lighting/appliance at 15 W/ft² for a typical residence).

**Impact**: Cooling equipment may be undersized by the omitted internal gains (approximately 0.5–1.5 kW for a typical 150 m² residence), potentially leading to insufficient cooling under design conditions. This is more likely to manifest in internal-load-dominated buildings (small homes with many occupants/appliances, mild climates).

### Finding 3: No water heater autosizing methodology [Severity: high]
**Description**: The autosizing module (`autosize.rs`) handles only HVAC equipment capacities. There is no water heater capacity or storage volume autosizing logic. The `FirstHourRating` field is parsed from HPXML (`resolve_water_heater.rs:43, 58`) and passed through to the water heater config but is never computed or derived from building characteristics. There is no `autosize_water_heater` flag, no `autosize_water_heater_factor`, and no analogous water heater sizing methodology in the codebase.

**Code Location**: `autosize.rs:85–331` — `autosize_equipment_capacities` iterates over equipment specs checking only `autosize_heating`, `autosize_cooling`, and `autosize_backup` flags. No water heater path exists. `solver_builder.rs` builds thermal solvers for HVAC sizing only. No ASHRAE 124 / DOE test procedure methodology for First-Hour Rating (FHR) or storage-volume-based sizing is implemented.

**Root Cause**: The autosizing scope was limited to HVAC equipment. Water heater sizing is not part of the autosize module design.

**Impact**: Users must always provide explicit water heater capacity and storage volume. Unlike EnergyPlus which can size water heaters from draw profiles and temperature rise (though this is also partially manual in E+), HARES offers no automated water heater sizing guidance. This means HPXML files without explicit water heater capacities will lack that equipment entirely (or fall back to defaults that may be inappropriate for the building).

### Finding 4: Cooling sizing at solar noon only misses combined thermal peak [Severity: medium]
**Description**: `autosize_capacity_cooling` (`thermal_solver/stepping.rs:141–152`) computes solar position for July 21 at local solar noon. However, the peak cooling load in a building is often the combined effect of near-peak outdoor dry-bulb (which typically occurs at 3–4 PM) and residual solar gains through west-facing glazing. West-facing windows, in particular, produce maximum solar gain in the late afternoon when outdoor temperature is still near its peak. At solar noon, west windows receive negligible direct beam radiation.

**Code Location**: `thermal_solver/stepping.rs:141–152` — hardcoded to July 21 at solar noon (`12, 0, 0`); `thermal_solver/stepping.rs:184` — uses POA × SHGC × area (single-orientation calculation, no diurnal scan).

**Root Cause**: The approach of computing a single solar radiation condition rather than scanning through the diurnal cycle misses orientation-specific peak timing. Manual J-2016 §7–8 requires evaluating loads at the hour-by-hour solar profiles for each exposure (N, NE, E, SE, S, SW, W, NW, horizontal) and selecting the maximum.

**Impact**: West-facing windows may be severely undercounted in the design cooling load, leading to undersized equipment for buildings with significant west glazing. Conversely, for buildings dominated by east/south glazing, the noon-only calculation may be adequate.

### Finding 5: Oversizing factors and limits are correctly configurable [Severity: low]
**Description**: The oversizing factor logic correctly implements configurable HPXML overrides (`<HeatingAutosizingFactor>`, `<CoolingAutosizingFactor>`, `<BackupHeatingAutosizingFactor>`) with Manual S defaults (1.4 heating, 1.15 cooling, 1.0 backup) as fallback. HPXML `<AutosizingLimits>` min/max bounds are applied as a clamp after oversizing. These are correctly consumed and removed from params after use.

**Code Location**: `autosize.rs:32–38` (factor constants); `autosize.rs:136–167` (heating factor + limits); `autosize.rs:217–248` (cooling factor + limits); `autosize.rs:291–300` (backup factor).

**Root Cause**: This is correctly implemented per HPXML v4.0 schema and ACCA Manual S-2017.

**Impact**: Positive — users have correct control over oversizing behavior with well-documented defaults. No fix needed.

### Finding 6: Heating sizing correctly ignores solar and internal gains [Severity: low]
**Description**: `autosize_capacity` (`thermal_solver/stepping.rs:40–43`) zeros all non-outdoor inputs (solar gains, internal gains, infiltration). This is correct per ACCA Manual J-2016 which specifies that winter heating design conditions should use no solar credit (conservative worst case: cloudy winter night/morning) and no internal gains (building may be unoccupied during extreme cold). The `AutosizingContext` constructor comment (autosize.rs:69) explicitly documents "zero solar, zero internal gains" for heating. EnergyPlus similarly does not apply solar or internal heat gains during heating design day sizing.

**Code Location**: `autosize.rs:69` (documentation); `thermal_solver/stepping.rs:40–58`.

**Root Cause**: Intentional design choice aligned with Manual J. Not a defect.

**Impact**: Positive — heating sizing is conservative and standard-compliant.

## Summary
- Total findings: 6
- Critical / High / Medium / Low: 0 / 3 / 1 / 2

## Recommendations

1. **Add design-day simulation for HVAC sizing (High priority)**. Replace or augment the single algebraic `solve_for_output_input` with a short 1–3 day design-day simulation using the ASHRAE design-day temperature profile. The energy balance model already handles timestepping; the main change is plumbing an ideal-loads system into the zone loop during a sizing pre-pass. Run the cooling design day (July 21, ASHRAE 1% DB) and heating design day (January 21, ASHRAE 99% DB), record the peak hourly load, and use that as the `raw_capacity` input to the existing oversizing-factor pipeline. This would align HARES with EnergyPlus's `ZoneSizingCalc` methodology (SizingManager.cc:285–390).

2. **Include internal gains in cooling autosizing (High priority)**. Add default internal gain assumptions to the cooling `u_design` vector. Sensible gains per ASHRAE Standard 62.2-2022 Appendix B for a typical residence: 2 occupants at 70 W/person sensible = 140 W, plus 5 W/m² for lights and plug loads. These should be configurable via HPXML or overrides. Add latent gains (45 W/person latent) for latent load estimation.

3. **Implement water heater autosizing (High priority)**. Add a water heater sizing methodology. Options:
   - **FHR-based** (Manual J/S for storage water heaters): compute FHR from daily hot water draw profile, temperature rise (setpoint − mains temp), and recovery capacity.
   - **Simplified storage-volume-based**: size tank volume as a function of number of bedrooms (e.g., 40 gal for 1–2 BR, 50 gal for 3–4 BR, per building code thumb rules) and capacity from recovery rate based on FHR methodology.
   - At minimum, add an `autosize_water_heater` flag and a documented warning when water heater specs lack capacity.

4. **Scan diurnal solar profile for cooling sizing (Medium priority)**. Instead of computing solar at solar noon only, evaluate the `autosize_capacity_cooling` solve at 3–5 solar positions (10 AM, 12 PM, 2 PM, 4 PM, 6 PM) with the corresponding outdoor DB from the design-day profile. Select the maximum. This captures orientation-specific peak timing without requiring a full simulation. Alternatively, run a full 24-hour design day as recommended in item 1, which naturally captures this.

## References / Citations
- EnergyPlus Engineering Reference, §17.5.5 "Zone and System Sizing" — describes the Ideal Loads design-day simulation approach used by SizingManager.
- ACCA Manual J-2016 (Residential Load Calculation), §4–5 (design conditions), §7 (cooling load), §8 (heating load).
- ACCA Manual S-2017 (Residential Equipment Selection), §4 "Sizing Based on Design Loads" — oversizing factors 1.15 cooling, 1.40 heating.
- ASHRAE Handbook of Fundamentals 2021, Chapter 14 "Climatic Design Information" — design day temperature profiles.
- ASHRAE Standard 90.1-2019 §6.4.3.1.1 — indoor design setpoints.
- DOE 10 CFR Part 430 Subpart B Appendix E — Uniform Energy Factor / First-Hour Rating methodology for water heaters.
- EnergyPlus SizingManager.cc:122–126 — Ideal Loads design-day methodology for zone/system sizing.
