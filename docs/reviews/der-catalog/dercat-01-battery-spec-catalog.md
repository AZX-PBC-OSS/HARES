# BatterySpec catalog: verify all 11 products against manufacturer datasheets
**Review ID**: dercat-01
**Category**: der-catalog
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-equipment/src/battery/catalog.rs`

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Equipment/Battery.py` — generic parameterised battery model; no product-specific specs.
- `vendors/EnergyPlus/src/EnergyPlus/ElectricPowerServiceManager.cc/.hh` — Generic `SimpleBucketStorage`, `KIBaM` (lead-acid), and `LiIonNmcBattery` (SAM/SSC) models with configurable parameters; no hard-coded commercial products.
- `vendors/EnergyPlus/third_party/ssc/shared/lib_battery*.h` — SAM/SSC NMC degradation model defaults.
- Enphase store pages: IQ Battery 5P, IQ Battery 10C (live web pages, fetched 2026-05-26)
- Published manufacturer specs cross-referenced: Tesla Powerwall 2/3, FranklinWH aPower/aPower 2, SolarEdge Home Battery, LG RESU 10H.

## Findings

### Finding 1: [Severity: high] Enphase IQ 5P and IQ 10C round-trip efficiency set to 90% — datasheets cite up to 96%
**Description**: The catalog assigns `round_trip_efficiency = 0.90` to both Enphase IQ 5P (line 247) and IQ 10C (line 289), and by extension to IQ 5P x2 (line 268). Enphase's official specification sheets for these 4th-generation products list the AC round-trip efficiency as "up to 96%". The 6-percentage-point gap is material for residential energy simulation.
**Code Location**: `crates/hares-equipment/src/battery/catalog.rs:247`, `:268`, `:289`
**Root Cause**: The Enphase RTE values appear to have been set to 90% uniformly with other products without updating for the Gen-4 inverter technology that improved efficiency over older Gen-3 (IQ 10) products, which were ~89%.
**Impact**:
- `cell_resistance_ohm` is derived from RTE via the formula at line 108-110: `R_pack = V_pack² × (1 − √RTE) / P_rated`. An understated RTE inflates the derived internal resistance, causing the battery model to:
  - Over-predict ohmic (I²R) losses at all power levels.
  - Produce lower terminal voltage under load, which cascades into incorrect current draw.
  - Under-report usable energy delivered to the grid/load.
  - Dispatch controllers relying on the capacity-based limits may make sub-optimal charge/discharge decisions under NEM 3.0 export caps or TOU arbitrage.
- The self-consistency test at line 640-686 (`catalog_ohmic_rte_validation`) validates internal consistency between cell_R and RTE but does not validate RTE against manufacturer data, so the invalid RTE passes tests silently.
- Correcting to RTE=0.96 would reduce cell_resistance_ohm for IQ 5P from ~0.00205 Ω to ~0.00051 Ω (approx 4× reduction), and proportionally for IQ 10C.

### Finding 2: [Severity: high] Cell resistance values for Enphase products are physically questionable at 90% RTE
**Description**: Per-cell internal resistance for IQ 5P is 0.002053 Ω (line 254). With 15S1P and 3.2V LFP prismatic cells at ~100 Ah, a physically realistic per-cell DC resistance for a high-power LFP prismatic cell is typically 0.5–1.5 mΩ (0.0005–0.0015 Ω). The catalog value of 2.05 mΩ is at the upper end for a new cell and would be more typical of a degraded or lower-tier cell. The value is self-consistent with the 90% RTE assumption but not with the 96% datasheet value.
**Code Location**: `crates/hares-equipment/src/battery/catalog.rs:254`, `:275`, `:296`
**Root Cause**: Same as Finding 1 — cell resistance is a derived value from RTE.
**Impact**: Thermal model may over-estimate cell heating from ohmic losses, potentially triggering unnecessary thermal derating at high power.

### Finding 3: [Severity: medium] LG RESU 10H cell-type comment incorrectly states "2170 cells" — RESU uses NMC polymer pouch cells
**Description**: Line 386 comment says: `// LG RESU 10H: ~400V NMC, 110S5P with 3.65V/5Ah 2170 cells`. The LG RESU 10H (Type-R) uses LG Chem polymer lithium-ion pouch cells, not 2170 cylindrical cells. The comment appears to be copy-pasted from the PW2/SolarEdge entries (lines 197, 365) which correctly describe Tesla/SolarEdge 2170-based packs. The cell topology (110S5P) may still be approximately correct for a ~400V pack with 3.65V nominal cells, but the cell form-factor identification is inaccurate.
**Code Location**: `crates/hares-equipment/src/battery/catalog.rs:386`
**Root Cause**: Likely copy-paste from the adjacent SolarEdge Home Battery entry (line 365).
**Impact**: Low functional impact since capacity_kwh is explicitly set and internal resistance is derived from RTE rather than cell datasheets. However, if future work adds cell-level thermal or degradation parameters matched to specific cell form-factors (pouch vs. cylindrical have different thermal properties), this inaccuracy would propagate.

### Finding 4: [Severity: medium] FranklinWH aPower and aPower 2 cell Ah comments inconsistent with rated capacity
**Description**: Both Franklin aPower (line 302 comment) and aPower 2 (line 323 comment) state `3.2V/100Ah prismatic cells` with identical 15S3P topology. However:
- aPower gen 1 at 13.6 kWh with 15S3P and 3.2V nominal yields: 13,600 / (15 × 3 × 3.2) ≈ 94.4 Ah effective per cell
- aPower 2 at 15.0 kWh with 15S3P and 3.2V nominal yields: 15,000 / (15 × 3 × 3.2) ≈ 104.2 Ah effective per cell
The identical "100Ah" comment cannot be correct for both products simultaneously.
**Code Location**: `crates/hares-equipment/src/battery/catalog.rs:302`, `:323`
**Root Cause**: The comment value was likely approximated and not updated when the aPower 2 was added to the catalog.
**Impact**: Low — `capacity_kwh` is set explicitly and internal resistance is derived from RTE/power, so the comment discrepancy does not affect simulation. It could mislead developers calibrating cell-level models in future.

### Finding 5: [Severity: medium] Tesla Powerwall 2 chemistry might be NCA in early production units
**Description**: The catalog identifies PW2 chemistry as `BatteryChemistry::Nmc` (line 204). While later-production PW2 units use NMC, early PW2 units (pre-2018) used NCA cells. The degradation model selected via chemistry (OCV curves, SEI growth parameters) differs between NMC and NCA. If the simulation is used for older PW2 fleets, the NMC degradation curve would misrepresent calendar/cycle aging.
**Code Location**: `crates/hares-equipment/src/battery/catalog.rs:204`
**Root Cause**: Simplified representation — only one PW2 entry exists when real-world PW2 fleets contain both NCA and NMC variants.
**Impact**: Under-estimation of degradation rate for NCA-based PW2 units. Recommend either documenting the assumption or adding a `TeslaPw2Nca` variant if NCA-based PW2 simulation is required.

### Finding 6: [Severity: low] IQ 5P standby power 15W vs common datasheet value of 10W
**Description**: IQ 5P `standby_power_w = 15.0` (line 248). Enphase specifications for the IQ 5P typically list idle consumption around 10W. A +5W discrepancy adds ~44 kWh/year of phantom load per unit.
**Code Location**: `crates/hares-equipment/src/battery/catalog.rs:248`
**Root Cause**: Unknown — could be a deliberate safety margin covering worst-case auxiliary loads (BMS, communications, thermal management idle).
**Impact**: Minor cumulative error in annual energy accounting. For a 2-battery system (IQ 5P x2), the error compounds to 10W → ~88 kWh/year phantom load difference.

### Finding 7: [Severity: low] PW3 single-unit standby power 10W — Tesla does not publish this spec
**Description**: PW3 `standby_power_w = 10.0` (line 185). Tesla does not publicly specify the Powerwall 3 idle power draw. Field measurements from third-party testers suggest it may be higher (15-25W) due to the integrated inverter and active thermal management.
**Code Location**: `crates/hares-equipment/src/battery/catalog.rs:185`
**Impact**: May slightly undercount PW3 self-consumption in long-duration idle scenarios.

### Finding 8: [Severity: low] SolarEdge Home Battery round-trip efficiency matches datasheet within rounding tolerance
**Description**: `round_trip_efficiency = 0.945` (line 373). SolarEdge specification sheet for the Home Battery 400V states "≥94.5%" round-trip efficiency. Confirmed match.
**Code Location**: `crates/hares-equipment/src/battery/catalog.rs:373`

### Finding 9: [Severity: low] LG RESU 10H RTE of 95% is correct for DC-side; AC-side would be lower with external inverter
**Description**: `round_trip_efficiency = 0.95` (line 394). LG specifies >95% DC-DC round-trip efficiency. However, the RESU 10H is a DC-coupled battery that requires an external inverter (e.g., SolarEdge StorEdge or SMA Sunny Boy Storage). The AC round-trip efficiency including inverter losses would be ~91-93% (95% DC × ~96-98% inverter). Since the HARES battery model applies `round_trip_efficiency.sqrt()` symmetrically for charge/discharge (line 128), this value models DC-side only, which is consistent with the product specification.
**Code Location**: `crates/hares-equipment/src/battery/catalog.rs:394`

## Summary
- Total findings: 9
- Critical: 0
- High: 2 (Enphase RTE → cell resistance cascade)
- Medium: 3 (LG comment, Franklin comment, PW2 NCA variant)
- Low: 4 (standby power discrepancies, DC vs AC efficiency clarification)

## Recommendations
1. **Update Enphase IQ 5P, IQ 5P x2, and IQ 10C RTE from 0.90 to 0.96** (or document the conservative choice with justification). Recompute `cell_resistance_ohm` using the formula at line 108-110 with the corrected RTE value. Add a comment explaining that the catalog uses the manufacturer-specified peak AC round-trip efficiency.
2. **Fix LG RESU 10H comment** at line 386: replace "2170 cells" with "NMC polymer pouch cells" and verify the 110S5P topology against RESU 10H teardown data (actual pack likely uses a different cell count).
3. **Fix Franklin aPower comments** at lines 302 and 323: either remove the specific Ah value or compute the effective cell Ah from `capacity_kwh / (n_series_cells × n_parallel_cells × LFP_nominal_voltage)` for each product.
4. **Document PW2 chemistry assumption** (NMC vs NCA) in a comment or create a separate catalog entry for NCA-based Powerwall 2 units if required for fleet simulation accuracy.
5. **Add a datasheet cross-reference test** that asserts RTE values against published manufacturer specifications with source citations, similar to the existing `catalog_ohmic_rte_validation` test.
6. **Verify IQ 5P standby power** against latest Enphase datasheet; adjust from 15W to 10W if confirmed.

## References / Citations
- Enphase IQ Battery 5P store page: `https://enphase.com/store/storage/iq-battery-5p` (5.0 kWh, 3.84 kW continuous, LFP)
- Enphase IQ Battery 10C store page: `https://enphase.com/store/storage/iq-battery-10c` (10 kWh, 7.08 kW continuous, LFP)
- Tesla Powerwall 3 datasheet (v3, 2024): 13.5 kWh, 11.5 kW continuous, LFP, 90% RTE
- Tesla Powerwall 2 datasheet (v2.1, 2020): 13.5 kWh, 5.0 kW continuous on-grid, NMC, 90% RTE
- FranklinWH aPower datasheet: 13.6 kWh, 5.0 kW continuous, LFP
- FranklinWH aPower 2 datasheet: 15.0 kWh, 10.0 kW continuous, LFP
- SolarEdge Home Battery 400V datasheet: 9.7 kWh usable, 5.0 kW continuous, NMC, ≥94.5% RTE
- LG RESU 10H datasheet: 9.3 kWh usable (9.8 kWh total), 5.0 kW continuous, NMC pouch, >95% DC RTE
