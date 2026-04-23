# Dehumidifier Capacity and Energy Factor Curves Always Identity — No Temperature Dependence

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-equipment

## Problem

`Dehumidifier::init_from_typed` always sets both biquadratic curves to the identity
constant `[1.0, 0.0, 0.0, 0.0, 0.0, 0.0]`, regardless of any curve data that might
be available. This means dehumidifier water removal capacity and energy factor are
constant across all dry-bulb temperatures and relative humidity levels. In reality,
dehumidifier performance varies significantly with conditions: at 60°F/60% RH (AHAM
rated conditions) a unit may remove 30 L/day, but at 50°F it may remove only 20 L/day
(-33%) due to reduced refrigerant cycle effectiveness.

EnergyPlus models this with two biquadratic curves (capacity-vs-conditions,
energy-factor-vs-conditions). HARES provides the curve infrastructure
(`BiquadraticCurve`) but hardcodes identity, masking all temperature/humidity
dependence.

## Evidence

`dehumidifier.rs:241–252`:

```
self.water_removal_curve = BiquadraticCurve {
    coeffs: DEFAULT_NORMALIZED_CURVE,
    x1_bounds: DEFAULT_DB_BOUNDS_C,
    x2_bounds: DEFAULT_RH_BOUNDS,
};
self.energy_factor_curve = BiquadraticCurve {
    coeffs: DEFAULT_NORMALIZED_CURVE,
    x1_bounds: DEFAULT_DB_BOUNDS_C,
    x2_bounds: DEFAULT_RH_BOUNDS,
};
self.water_removal_curve_rated_value = 1.0;
self.energy_factor_curve_rated_value = 1.0;
```

Where `DEFAULT_NORMALIZED_CURVE: [f64; 6] = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0]`.

This is hard-coded even when the legacy (non-typed) init path may have loaded
real curves. The typed-config path unconditionally overwrites with identity.

## OCHRE Cross-check

OCHRE `Equipment/Dehumidifier.py` loads biquadratic curves for water removal and
energy factor as a function of dry-bulb temperature and relative humidity from
default CSV files. The curves are normalised to the AHAM DH-1 rated condition
(60°F / 60% RH for DH-1-2009). HARES has the BiquadraticCurve struct but no
default coefficients for dehumidifiers.

## Cross-Reference

Ticket 010 (default biquadratic performance curves) covers the same defect shape
for HVAC cooling coils. That ticket's solution for loading default curves from a
constants module or CSV data file should be adopted here for dehumidifier curves.
Do not merge — different equipment, different curve variables (dehumidifier curves
use dry-bulb °C and RH fraction; cooling coils use WB and OAT).

Ticket 082 (dehumidifier pint conversion) establishes the rated reference condition
that the normalization value must match.

## Required Behavior

1. Source default biquadratic curve coefficients for dehumidifier water removal
   and energy factor as a function of dry-bulb temperature (°C) and relative humidity
   (fraction). Primary reference: EnergyPlus `ZoneHVAC:Dehumidifier:DX` example file
   coefficients (`WaterRemovalCurve` and `EnergyFactorCurve` from `ExampleFiles/`).
2. Store default coefficients in a constants module inside `hares-equipment/src/hvac/`
   (matching the pattern for cooling coil defaults).
3. Load these defaults in `dehumidifier.rs:init_from_typed` (line 241 area) when no
   explicit curve data is provided in the typed config.
4. Retain identity as a last-resort fallback only.
5. Set `water_removal_curve_rated_value` and `energy_factor_curve_rated_value` to the
   curve output evaluated at the AHAM DH-1-2009 rated condition (15.56°C, 0.60 RH),
   so that `rated_capacity_liters_per_day` passes through unchanged at rated conditions.

## Approach

- In `hares-equipment/src/hvac/dehumidifier_defaults.rs` (new file or addition to existing defaults module), define:
  ```rust
  pub const DEFAULT_WATER_REMOVAL_CURVE: [f64; 6] = [ ... ]; // from EnergyPlus DX dehumidifier dataset
  pub const DEFAULT_ENERGY_FACTOR_CURVE: [f64; 6] = [ ... ];
  pub const AHAM_DH1_2009_RATED_DB_C: f64 = 15.56;
  pub const AHAM_DH1_2009_RATED_RH: f64 = 0.60;
  ```
- In `dehumidifier.rs:init_from_typed` (line 241), replace hardcoded identity:
  ```rust
  self.water_removal_curve = BiquadraticCurve {
      coeffs: DEFAULT_WATER_REMOVAL_CURVE,
      x1_bounds: DEFAULT_DB_BOUNDS_C,
      x2_bounds: DEFAULT_RH_BOUNDS,
  };
  // compute rated value at (AHAM_DH1_2009_RATED_DB_C, AHAM_DH1_2009_RATED_RH)
  self.water_removal_curve_rated_value =
      self.water_removal_curve.evaluate(AHAM_DH1_2009_RATED_DB_C, AHAM_DH1_2009_RATED_RH);
  ```
  Same for `energy_factor_curve`.

## Citation

- EnergyPlus Engineering Reference §16.9: `ZoneHVAC:Dehumidifier:DX` — two
  biquadratic curves (water removal and energy factor vs. dry-bulb and RH)
- AHAM DH-1-2009: rated at 60°F/60% RH; DH-1-2021: 65°F/60% RH
- EnergyPlus example curves for residential dehumidifiers:
  `EnergyPlus/DataSets/DX_Coil_BiQuadratic.csv`

## Annual kWh Impact Rank

**Medium.** In mixed-climate zones (2–4) where dehumidifiers run in shoulder seasons
at below-rated temperatures, identity curves overstate water removal and understate
energy consumption. For a 30 L/day rated unit running in a 60°F basement, real
capacity may be 20 L/day — the identity curve overstates by 50%, causing the
simulation to end dehumidification cycles earlier and undercount runtime.

## Definition of Done

- [ ] Default biquadratic coefficients sourced from EnergyPlus `ZoneHVAC:Dehumidifier:DX` dataset and stored in a named constants module inside `hares-equipment/src/hvac/`
- [ ] `init_from_typed` at `dehumidifier.rs:241` loads defaults instead of identity when no explicit curve is provided
- [ ] `water_removal_curve_rated_value` and `energy_factor_curve_rated_value` computed at AHAM DH-1-2009 rated condition (15.56°C, 0.60 RH), not hardcoded to 1.0
- [ ] Identity retained as fallback only for the case where no defaults module is available
- [ ] Test: at (15.56°C, 0.60 RH), `water_removal_curve.evaluate(...)` × `water_removal_curve_rated_value` normalizes to 1.0 (rated output)
- [ ] Test: at (10.0°C, 0.60 RH), curve output factor < 1.0 (capacity decreases below rated at lower temperature)

## Verification

```bash
cargo test -p hares-equipment -- hvac::dehumidifier
```

Compare capacity prediction at 10°C against EnergyPlus reference output for `ZoneHVAC:Dehumidifier:DX` using the same curve coefficients.
