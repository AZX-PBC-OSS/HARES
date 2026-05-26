# VehicleSpec for all 13 vehicles: capacity, charging power, efficiency, PHEV vs BEV
**Review ID**: dercat-04
**Category**: der-catalog
**Date**: 2026-05-26

## Files Reviewed
crates/hares-equipment/src/ev/catalog.rs

## Vendor/Reference Files Consulted
vendors/OCHRE/ochre/Equipment/EV.py

## Findings

### Finding 1: [Severity: critical]
**Description**: Fuel economy (`fuel_economy_kwh_per_mi`) is derived as `capacity_kwh / range_miles` (line 120), which gives DC battery-to-wheels efficiency. This systematically underestimates true wall-to-wheels energy consumption by 10–20% because it does not account for onboard charger losses, battery charge/discharge losses, or auxiliary load. Across all 13 vehicles, the HARES-derived efficiency is consistently lower (more optimistic) than EPA wall-measured ratings. For example: Tesla Model Y LR computes as 77.0/310 = 0.248 kWh/mi (24.8 kWh/100mi) vs. EPA 28 kWh/100mi; Chevy Volt Gen1 computes as 10.9/38 = 0.287 kWh/mi (28.7 kWh/100mi) vs. EPA 35 kWh/100mi. This is a systemic error affecting every vehicle.
**Code Location**: `crates/hares-equipment/src/ev/catalog.rs:120` (`self.capacity_kwh / self.range_miles`)
**Root Cause**: The `fuel_economy_kwh_per_mi` field is set to `capacity_kwh / range_miles`, which is the DC (battery output) efficiency. The EPA rating includes wall-to-wheels AC consumption measured at the meter, which includes ~10–15% charging losses. OCHRE's `EV.py` separates the two concerns: `EV_FUEL_ECONOMY = 1/325 * 1000` (mi/kWh at the battery) and `EV_EFFICIENCY = 0.9` (charging efficiency applied separately). HARES has no equivalent charging efficiency field in the config (`charging_efficiency: None` at line 129).
**Impact**: Compounding over year-long simulations, this misestimates total EV load by 10–20% (toward the low side). Every vehicle's annual charging energy is systematically underpredicted. The magnitude varies by vehicle: 12% for Model Y LR, 22% for Volt Gen1, 14% for Bolt EV, 15% for Lightning.

### Finding 2: [Severity: critical]
**Description**: Hyundai IONIQ 5 LR AWD range is 303 miles, approximately 17% higher than the EPA AWD rating of ~260 miles (2023). The 303 value is consistent with the RWD EPA rating or WLTP cycle, not the AWD combined EPA rating. With the already-optimistic efficiency derivation (Finding 1), this produces a compound efficiency error of ~29%: HARES computes 0.255 kWh/mi (77.4/303) vs. real-world AC consumption ~0.31–0.34 kWh/mi.
**Code Location**: `crates/hares-equipment/src/ev/catalog.rs:260` (`range_miles: 303.0`)
**Root Cause**: Range value likely sourced from WLTP cycle (which gives ~487 km = 303 mi) or from the RWD variant's EPA rating rather than the AWD variant's EPA rating. The IONIQ 5 AWD EPA rating is 256–266 miles depending on model year.
**Impact**: The IONIQ 5 LR AWD will be simulated as consuming ~29% less energy per mile than reality, leading to significantly underestimated charging demand and overestimated effective range. This vehicle would appear to need charging much less frequently than a real-world equivalent.

### Finding 3: [Severity: critical]
**Description**: Ford Mustang Mach-E SR and ER onboard charger (OBC) power listed as 11.5 kW, but Ford's actual OBC is rated at 10.5 kW. This applies to both the SR LFP variant (line 224) and the ER NMC variant (line 235). A 1.0 kW overestimation on a 10.5 kW charger is a ~10% error in charging power, which directly affects building panel load calculations.
**Code Location**: `crates/hares-equipment/src/ev/catalog.rs:224,235` (both entries: `max_l2_power_kw: 11.5`)
**Root Cause**: The value 11.5 kW is a common Tesla OBC rating. Ford's onboard charger is 10.5 kW (48A at 240V accounting for derating or 30A × 350V internally). Confusion between manufacturer OBC ratings is the likely cause.
**Impact**: Vehicles with incorrectly high charging power overestimate building panel draw. In simulations with multiple EVs (or combined EV + PV + battery + HVAC), the panel-level aggregate power may exceed real-world draw by 1–2 kW per Mach-E. Over an annual simulation, this overstates peak building load.

### Finding 4: [Severity: high]
**Description**: Nissan Leaf S 30kWh OBC power listed as 6.6 kW (line 268), but the S trim came standard with a 3.3 kW (3.6 kW) onboard charger. The 6.6 kW charger was standard only on SV and SL trims, or available as part of the Quick Charge Package option on the S trim. The catalog label explicitly says "S" (base trim), so the OBC power is nominal 2× too high for the standard-configuration vehicle.
**Code Location**: `crates/hares-equipment/src/ev/catalog.rs:268` (`max_l2_power_kw: 6.6`)
**Root Cause**: The OBC rating from the SV/SL trim was incorrectly applied to the S trim entry.
**Impact**: The Leaf S standard configuration would draw at most ~3.3 kW from the building on L2, not 6.6 kW. In simulations, this Leaf would appear to charge at double its real rate, completing charging in half the actual time, and overestimating instantaneous panel draw by ~3.3 kW. In aggregated residential simulations with Leaf vehicles, this inflates evening peak load.

### Finding 5: [Severity: high]
**Description**: Jeep Wrangler 4xe PHEV all-electric range listed as 25 miles (line 282), but EPA all-electric range is 21 miles. This is a 19% overestimate of PHEV electric range. For a PHEV, range errors directly affect charging frequency (shorter real range means more frequent charging events and more gas-engine operation).
**Code Location**: `crates/hares-equipment/src/ev/catalog.rs:282` (`range_miles: 25.0`)
**Root Cause**: The 25-mile figure may come from Jeep's initial marketing claims or the WLTP rating (which gives ~40 km ≈ 25 mi for PHEVs), not the EPA combined electric range of 21 miles.
**Impact**: The simulated Wrangler 4xe will appear to cover ~19% more distance on electric power than in reality, reducing the modeled frequency of charging events and gas-mode operation. For a daily commute of 25 miles, this vehicle would appear to complete the trip on electric-only when in reality it would switch to gas mid-trip. This misclassification of charging patterns is exactly the PHEV misclassification risk described in the review brief.

### Finding 6: [Severity: high]
**Description**: Toyota RAV4 Prime OBC power listed as 3.3 kW (line 290), but the standard OBC for 2022+ model years (all trims) and 2021 XSE is 6.6 kW. Only the 2021 SE base model had 3.3 kW. Assuming a typical current model year, the RAV4 Prime's charging capability is understated by 50%.
**Code Location**: `crates/hares-equipment/src/ev/catalog.rs:290` (`max_l2_power_kw: 3.3`)
**Root Cause**: OBC rating from the lowest-spec 2021 SE trim was applied as the universal value for the RAV4 Prime.
**Impact**: The RAV4 Prime will be simulated as taking ~4.4 hours to fully charge (14.4 kWh ÷ 3.3 kW) instead of the actual ~2.2 hours (14.4 kWh ÷ 6.6 kW). This doubles the modeled charging duration, potentially missing charging completion within available windows (e.g., 10 PM–6 AM off-peak TOU). Since the RAV4 Prime is a PHEV with shallow daily cycles, this error directly impacts the accuracy of charging schedule simulation and TOU optimization.

### Finding 7: [Severity: medium]
**Description**: Tesla Model Y LR AWD battery capacity (77.0 kWh, line 168) and Model 3 LR AWD capacity (75.0 kWh, line 190) are below current production values. The 2023+ Model Y LR has ~81 kWh usable; the 2023+ Model 3 LR has ~78 kWh usable. The values in the catalog correspond to pre-2022 production. Similarly, the Model Y LR range of 310 miles (line 172) is conservative vs. 2023 EPA rating of 330 miles.
**Code Location**: `crates/hares-equipment/src/ev/catalog.rs:168,190` (capacity); `crates/hares-equipment/src/ev/catalog.rs:172` (range)
**Root Cause**: These appear to be 2020–2022 model year specifications. Without explicit model year annotations in the catalog, it is unclear whether the values are intentionally conservative or stale.
**Impact**: The effective DC efficiency (capacity/range) for these vehicles is 77/310 = 0.248 kWh/mi vs. 81/330 = 0.245 kWh/mi, so the combined error is modest (~1%). However, the absolute capacity error means the battery state is undersized by ~5%.

### Finding 8: [Severity: medium]
**Description**: Tesla Model Y SR LFP specs match the 2021–2022 era variants rather than the 2023+ LFP variant. The catalog lists 57.5 kWh / 260 mi (lines 179, 183), whereas the 2023 Model Y SR AWD LFP has ~60 kWh / 279 mi EPA. As a BEV, the range underestimate makes this vehicle appear to charge more frequently.
**Code Location**: `crates/hares-equipment/src/ev/catalog.rs:179,183`
**Root Cause**: Same model-year ambiguity as Finding 7. The 57.5 kWh capacity and 260 mi range match the earlier Standard Range variant configuration.
**Impact**: Range is underestimated by ~7%, meaning simulated daily driving distances will cause deeper cycling and more frequent charging events than a current-year Model Y SR LFP would experience.

### Finding 9: [Severity: medium]
**Description**: Battery chemistry assignments are partially inaccurate for Tesla vehicles. The Model 3 LR is listed as NMC (line 193) but Panasonic-produced NCA cells are also used extensively. The Model Y LR is listed as NCA (line 171) but LG-sourced packs use NMC chemistry. Tesla sources cells from multiple suppliers and the chemistry varies by production batch and market.
**Code Location**: `crates/hares-equipment/src/ev/catalog.rs:171,193`
**Root Cause**: Battery chemistry specified as a single value when in reality it depends on the battery cell supplier. Tesla has used Panasonic NCA, LG NMC, and CATL LFP depending on factory and model variant.
**Impact**: Low direct impact on charging simulation if chemistry only drives degradation rate (which already varies per model via `degradation_per_year`). However, if chemistry is later used to parameterize thermal behavior, cold-weather performance, or charging curve tapering, incorrect chemistry would produce inaccurate results.

### Finding 10: [Severity: low]
**Description**: Ford Mustang Mach-E SR capacity (72.0 kWh, line 223) and ER capacity (88.0 kWh, line 234) deviate slightly from official figures. The SR LFP variant is ~70 kWh usable; the ER NMC variant is ~91 kWh usable for 2023+. These are 2.9% and 3.3% deviations, respectively.
**Code Location**: `crates/hares-equipment/src/ev/catalog.rs:223,234`
**Root Cause**: These may reflect earlier model year specifications, pre-refresh battery capacities, or rounding to convenient values.
**Impact**: Minor efficiency calculation error (~3% for each variant). Combined with the Mach-E OBC error (Finding 3), the Mach-E configurations have accumulated errors.

### Finding 11: [Severity: low]
**Description**: Ford F-150 Lightning ER OBC power listed as 11.5 kW (line 246). While this is correct for the standard configuration, the ER version can optionally be equipped with a dual onboard charger providing 19.2 kW (80A). The catalog does not distinguish between standard and optional OBC configurations.
**Code Location**: `crates/hares-equipment/src/ev/catalog.rs:246` (`max_l2_power_kw: 11.5`)
**Root Cause**: Conservative choice of the standard 48A configuration; the 80A dual charger is an option that not all ER Lightnings have.
**Impact**: Low. The standard 11.5 kW is a safe default. Users modeling a Lightning with the 19.2 kW charger option will see charger-limited charging in simulation.

### Finding 12: [Severity: low]
**Description**: No model year is specified for any vehicle in the catalog. The `VehicleSpec` struct (line 106) has no model year field, and the label strings (e.g., "Tesla Model Y LR AWD") do not include a year. This makes it impossible to determine which EPA rating cycle and battery revision the specs target without external knowledge.
**Code Location**: `crates/hares-equipment/src/ev/catalog.rs:106–116` (struct lacks year field); `crates/hares-equipment/src/ev/catalog.rs:164–308` (catalog entries lack year metadata)
**Root Cause**: The `VehicleSpec` struct was designed without a model year field, presumably under the assumption that each vehicle has one canonical spec.
**Impact**: Low for end users but high for maintenance. Without model years, future developers cannot determine whether specs need updating, and reviewers cannot definitively classify deviations as errors vs. intentional targeting of older model years.

## Summary
- Total findings: 12
- Critical: 3 / High: 3 / Medium: 3 / Low: 3

## Recommendations

1. **Apply a charging efficiency factor to `fuel_economy_kwh_per_mi`** (Finding 1). Either (a) divide `capacity_kwh / range_miles` by a per-vehicle or default charging efficiency (e.g., 0.88–0.90), or (b) populate the `charging_efficiency` config field and apply it at runtime during power/energy calculations. OCHRE's approach of separating fuel economy from `EV_EFFICIENCY = 0.9` provides a reference pattern. Without this fix, total annual EV load is underpredicted by 10–20% for every vehicle.

2. **Correct Hyundai IONIQ 5 LR AWD range** (Finding 2). Change `range_miles` from 303.0 to ~260.0 (or 266.0 for 2023 model year). Verify the intended model year and consult fueleconomy.gov for the EPA combined AWD rating.

3. **Correct Ford Mustang Mach-E OBC power** (Finding 3). Change `max_l2_power_kw` from 11.5 to 10.5 on both the SR (line 224) and ER (line 235) Mach-E entries.

4. **Correct Nissan Leaf S 30kWh OBC power** (Finding 4). Change `max_l2_power_kw` from 6.6 to 3.3 to match the S trim default. If the intent is to model the SV/SL trim or S with Quick Charge Package, rename the label to "Nissan Leaf SV/SL 30kWh" or "Nissan Leaf 30kWh (6.6 kW)".

5. **Correct Jeep Wrangler 4xe all-electric range** (Finding 5). Change `range_miles` from 25.0 to 21.0 to match EPA all-electric range. This is critical for PHEV charging behavior accuracy.

6. **Correct Toyota RAV4 Prime OBC power** (Finding 6). Change `max_l2_power_kw` from 3.3 to 6.6 to reflect the standard OBC for all trims from 2022+ and 2021 XSE. If targeting the base 2021 SE specifically, add a note or label annotation.

7. **Add a `model_year` field to `VehicleSpec`** (Finding 12). This eliminates ambiguity and enables automated validation against fueleconomy.gov API. Without it, future spec updates may regress to incorrect values.

8. **Audit Tesla and Mach-E battery capacities against the target model year** (Findings 7, 8, 10). Once a model year is established, update capacity and range values to match EPA for that year.

9. **Add chemistry notes for Tesla vehicles** (Finding 9). Consider using a chemistry of `Nca` with an annotation that LG packs use `Nmc`, or vice versa, to avoid implying a single universal chemistry.

## References / Citations

- **OCHRE EV.py** (`vendors/OCHRE/ochre/Equipment/EV.py:10–16`): Uses `EV_FUEL_ECONOMY = 1/325 * 1000` (mi/kWh DC) and `EV_EFFICIENCY = 0.9` separately. Max AC charging power by vehicle class: L2 = [3.6, 3.6, 7.2, 11.5] kW.
- **fueleconomy.gov**: Tesla Model Y LR AWD 2023: 28 kWh/100mi, 330 mi. Tesla Model Y AWD (SR): 123 MPGe, 279 mi. Tesla Model 3 LR AWD 2023: 26 kWh/100mi, 358 mi. Chevy Bolt EV 2023: 28 kWh/100mi, 259 mi. Chevy Bolt EUV 2023: 29 kWh/100mi, 247 mi. Ford Mach-E RWD LFP 2023: 33 kWh/100mi, 250 mi. Ford Mach-E RWD Extended 2023: 34 kWh/100mi, 310 mi. Ford F-150 Lightning 4WD ER 2023: 48 kWh/100mi, 320 mi. Nissan Leaf 30kWh 2016: 30 kWh/100mi, 107 mi. Jeep Wrangler 4xe 2024: 68 kWh/100mi elec, 21 mi all-electric. Chevy Volt 2013–2015: 35 kWh/100mi, 38 mi elec.
- **Wikipedia**: Tesla Model Y: 11.5 kW OBC; LFP variant ~60 kWh. Hyundai IONIQ 5: 77.4 kWh, 11 kW OBC, AWD EPA 412–418 km (256–260 mi). Toyota RAV4 Prime: 18.1 kWh gross battery, 6.6 kW OBC (XSE/2022+). Chevy Volt Gen1: 16.5 kWh gross, ~10.8 kWh usable.
- **Ford specifications**: Mach-E onboard charger rated at 10.5 kW. F-150 Lightning standard charger 11.5 kW; optional dual charger 19.2 kW.
- **Nissan Leaf specifications**: S trim standard OBC is 3.6 kW (3.3 kW net). SV/SL trims and Quick Charge Package provide 6.6 kW.
