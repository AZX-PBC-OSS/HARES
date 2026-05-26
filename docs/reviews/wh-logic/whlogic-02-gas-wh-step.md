# Gas WH step(): burner efficiency, UEF/EF, pilot consumption, flue losses
**Review ID**: whlogic-02
**Category**: wh-logic
**Date**: 2026-05-26

## Files Reviewed
crates/hares-equipment/src/water_heater/gas.rs

## Vendor/Reference Files Consulted
vendors/OCHRE/ochre/Equipment/WaterHeater.py vendors/EnergyPlus/src/EnergyPlus/WaterThermalTanks.cc

## Findings
### Finding 1: [Severity: medium] UEF and EF treated identically — no UEF→EF conversion
**Description**: The model uses Energy Factor (EF) and Uniform Energy Factor (UEF) interchangeably as the burner efficiency fallback. UEF values (post-2015 test procedure) are systematically different from EF (pre-2015). OCHRE applies a UEF→EF conversion (`EF = 0.9066 * UEF + 0.0711`) before deriving parameters, but HARES applies no conversion. For typical gas storage WHs with UEF 0.58–0.70, this underestimates efficiency by 1–3%.
**Code Location**: `gas.rs:317-323`
**Root Cause**: The `init_typed` fallback chain (`conversion_efficiency` → `energy_factor` → `uniform_energy_factor`) treats EF and UEF as equivalent values:
```rust
self.burner_efficiency_constant = c.conversion_efficiency.unwrap_or_else(|| {
    c.energy_factor
        .or(c.uniform_energy_factor)
        .map(|v| v * c.performance_adjustment.unwrap_or(1.0))
        .unwrap_or(DEFAULT_BURNER_EFFICIENCY)
});
```
When `uniform_energy_factor` is provided without `conversion_efficiency` or `energy_factor`, the raw UEF value is used directly as `burner_efficiency_constant`. OCHRE's HPXML parser (`hpxml.py:1115`) converts: `energy_factor = 0.9066 * uniform_energy_factor + 0.0711`. This conversion is absent in HARES.
**Impact**: Users providing UEF values for modern WHs (the common case for HPXML-based workflows) get biased efficiency estimates. Also affects `default_skin_loss_fraction()` at line 330 since it keys off `burner_efficiency_constant`, resulting in an incorrect skin-loss-to-zone fraction.

### Finding 2: [Severity: medium] Flue loss fraction applied on top of combustion efficiency — unclear physical basis
**Description**: The default `flue_loss_fraction` (0.10) is applied multiplicatively to `gross_heat_w` (already derated by `efficiency`). Neither OCHRE nor EnergyPlus applies a secondary flue-loss multiplier on top of combustion efficiency during burner operation. OCHRE computes `fuel = delivered_heat / efficiency`; EnergyPlus computes `FuelRate = Eheater / Efficiency`. Both use a single efficiency parameter that encompasses all burner-side losses (combustion + heat-exchanger losses). The 10% additional loss in HARES produces an effective combined efficiency of `0.78 * 0.90 = 0.702`, which may be reasonable for total system efficiency but conflates two independent physical mechanisms without clear justification.
**Code Location**: `gas.rs:440-443`
```rust
let gross_heat_w = burner_input_w * efficiency;
let flue_loss_w = gross_heat_w * self.flue_loss_fraction;
let tank_heat_w = (gross_heat_w - flue_loss_w).max(0.0);
```
**Root Cause**: The model structure is inverted relative to the reference implementations. OCHRE/E+ compute fuel consumption from delivered heat (`fuel = heat / efficiency`). HARES computes delivered heat from fuel (`heat = fuel * efficiency * (1 - flue_loss_fraction)`). If `efficiency` is truly combustion efficiency (~0.78) and `flue_loss_fraction` represents heat-exchanger stack losses, the split is defensible but should be documented. If `efficiency` already includes stack losses (as in OCHRE/E+), the `flue_loss_fraction` constitutes double-counting.
**Impact**: 10% reduction in heat delivered to water relative to the OCHRE/E+ approach (given same nominal efficiency). Also affects the `ideal_capacity` calculation at line 411-412 where `net_efficiency = efficiency * (1.0 - self.flue_loss_fraction)` is used to compute `effective_capacity_w`.

### Finding 3: [Severity: medium] Pilot fuel consumed but heat not delivered to tank; default pilot power is too low
**Description**: Standing pilot gas consumption is added to `fuel_input_w` (line 488) but not injected as heat into the tank (lines 445-449). In reality, a standing pilot transfers a fraction of its heat to the tank water. The default pilot power of 5.0 W (`DEFAULT_PILOT_POWER_W` at line 35) corresponds to ~17 BTU/h, which is an order of magnitude below typical standing pilot consumption of 200–800 BTU/h (60–230 W). If 5.0 W is intended as the electrical-equivalent rather than thermal input, the value is still too low — 5 W × 100% efficiency = 5 W thermal, vs. typical pilot heat input of 60–230 W.
**Code Location**: `gas.rs:35, 488, 445-449`
```rust
const DEFAULT_PILOT_POWER_W: f64 = 5.0;
// ...
let fuel_input_w = burner_input_w + self.pilot_power_w;
// ...
let heat_injections: Vec<(usize, f64)> = if tank_heat_w > 0.0 {
    vec![(self.burner_node, tank_heat_w)]
} else {
    vec![]
};
```
The pilot power is never included in `heat_injections`. EnergyPlus handles pilot load as `OffCycParaLoad` with `OffCycParaFracToTank` routing a fraction to the tank water (lines 6213-6214). OCHRE's `GasWaterHeater` class does not model pilot explicitly but both references capture pilot effects within the rated efficiency parameters.
**Root Cause**: The pilot is modeled purely as fuel consumption with no associated heat delivery. The separating of pilot from burner heat injection makes the heat balance non-conservative: fuel energy from the pilot "disappears" rather than heating water or ambient.
**Impact**: Underestimates tank heat gain during standby (by the fraction of pilot heat that enters the water) and misclassifies pilot energy as unproductive fuel consumption.

### Finding 4: [Severity: low] No off-cycle flue draft losses modeled
**Description**: EnergyPlus models separate on-cycle and off-cycle tank loss coefficients. For stratified tanks, `OffCycLossCoeff = OnCycLossCoeff + OffCycFlueLossCoeff` (`WaterThermalTanks.cc:6171`), where `OffCycFlueLossCoeff` represents additional stack-effect heat losses through the flue when the burner is off. HARES only applies `flue_loss_fraction` during burner operation (on-cycle only, since `flue_loss_w = 0` when `gross_heat_w = 0`). During standby, only tank jacket UA losses occur.
**Code Location**: `gas.rs:440-443` (only computed when burner fires)
**Root Cause**: The model has no parameter analogous to E+'s `OffCycFlueLossCoeff`. Off-cycle flue draft is a well-documented loss mechanism for atmospheric-vent gas WHs (especially those without flue dampers). The `flue_loss_fraction` parameter captures on-cycle flue losses but is inactive during standby.
**Impact**: Standby losses are underestimated by 5–20% for atmospheric-vent gas WHs without flue dampers, depending on flue geometry and stack effect.

### Finding 5: [Severity: low] No condensing gas WH latent heat recovery model
**Description**: Condensing gas water heaters (UEF > 0.90) recover 8–12% additional energy from water vapor latent heat in flue gases. HARES has no mechanism to model this. The `burner_efficiency_poly` field exists (`gas.rs:81`) and the `burner_efficiency()` function (`gas.rs:202-211`) supports a quadratic part-load efficiency curve, but `burner_efficiency_poly` is always initialized to `None` (line 324) and there is no configuration path to set it. A condensing WH could be approximated with a high constant efficiency (e.g., 0.92), but the part-load dependency (condensing efficiency increases at lower return-water temperatures) would not be captured.
**Code Location**: `gas.rs:81, 202-211, 324`
**Root Cause**: Feature gap. The infrastructure for variable efficiency exists (polynomial curve function, `burner_efficiency_poly` field) but is not wired to any configurable input.
**Impact**: Users must manually adjust `conversion_efficiency` to model condensing WHs, and part-load condensing effects (which depend on tank temperature) are lost.

## Summary
- Total findings: 5
- Critical: 0
- High: 0
- Medium: 3 (Findings 1, 2, 3)
- Low: 2 (Findings 4, 5)

## Recommendations
1. **UEF→EF conversion**: Apply the OCHRE-derived conversion (`EF = 0.9066 * UEF + 0.0711`) when `uniform_energy_factor` is provided without `energy_factor` or `conversion_efficiency`. Alternatively, require `conversion_efficiency` for modern workflows and deprecate direct EF/UEF usage.
2. **Clarify `flue_loss_fraction` semantics**: Document whether `efficiency` represents pure combustion efficiency (in which case `flue_loss_fraction` is a valid additional heat-exchanger loss) or total system efficiency (in which case the default should be `flue_loss_fraction = 0.0`). Consider matching OCHRE/E+ semantics where `efficiency` alone accounts for all burner-side losses.
3. **Route pilot heat to tank**: Add pilot power to `heat_injections` with an appropriate `pilot_fraction_to_tank` parameter (default ~0.8, matching E+'s `OffCycParaFracToTank`). Revisit the default `DEFAULT_PILOT_POWER_W` — if 5.0 W represents electrical input, the gas-equivalent thermal input should be higher; if it represents thermal input, it needs to be 60–230 W for a standing pilot.
4. **Add off-cycle flue loss coefficient**: Add an `off_cyc_flue_loss_coeff_w_per_k` parameter analogous to E+'s `OffCycFlueLossCoeff` to capture stack-effect losses during standby.
5. **Wire `burner_efficiency_poly` to config**: Allow users to specify polynomial coefficients via `GasWaterHeaterConfig` to support part-load efficiency curves for condensing WHs. Add a `condensing: bool` flag that activates a built-in latent heat recovery curve.

## References / Citations
- OCHRE `GasWaterHeater.__init__()`: `vendors/OCHRE/ochre/Equipment/WaterHeater.py:709-723`
- OCHRE `GasWaterHeater.finish_sub_update()`: `vendors/OCHRE/ochre/Equipment/WaterHeater.py:720-723`
- OCHRE HPXML `parse_water_heater()` (EF/UEF to eta_c derivation): `vendors/OCHRE/ochre/utils/hpxml.py:1013-1127`
- OCHRE `WaterHeater.calculate_power_and_heat()` (efficiency application): `vendors/OCHRE/ochre/Equipment/WaterHeater.py:265-296`
- EnergyPlus stratified tank off-cycle flue loss: `vendors/EnergyPlus/src/EnergyPlus/WaterThermalTanks.cc:6169-6172`
- EnergyPlus mixed tank on/off-cycle loss fractions: `vendors/EnergyPlus/src/EnergyPlus/WaterThermalTanks.cc:2580-2584`
- EnergyPlus stratified tank skin/flue loss inputs: `vendors/EnergyPlus/src/EnergyPlus/WaterThermalTanks.cc:3096-3099`
- EnergyPlus ambient zone gain (skin + vent): `vendors/EnergyPlus/src/EnergyPlus/WaterThermalTanks.cc:8656-8659`
- EnergyPlus fuel rate computation: `vendors/EnergyPlus/src/EnergyPlus/WaterThermalTanks.cc:8654`
- EnergyPlus standard ratings (EnergyFactor calculation): `vendors/EnergyPlus/src/EnergyPlus/WaterThermalTanks.cc:12600-12981`
- EnergyPlus CalcWaterThermalTankZoneGains (skin loss routing): `vendors/EnergyPlus/src/EnergyPlus/WaterThermalTanks.cc:589-680`
- Burch & Erickson 2004 (EF-based UA/eta_c derivation): <http://www.nrel.gov/docs/gen/fy04/36035.pdf>
- Maguire & Roberts 2020 (UEF-based UA/eta_c derivation): <https://www.ashrae.org/file%20library/conferences/specialty%20conferences/2020%20building%20performance/papers/d-bsc20-c039.pdf>
- RESNET EF Calculator 2017 (UEF→EF conversion reference): <https://www.resnet.us/wp-content/uploads/RESNET-EF-Calculator-2017.xlsx>
