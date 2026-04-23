# `WATER_DENSITY_KG_PER_M3 = 1000.0` Overestimates at Tank Temperatures

**Severity**: Low
**Priority**: P3
**Status**: Open
**Areas**: hares-physics, hares-equipment/water_heater

## Problem

`WATER_DENSITY_KG_PER_M3 = 1000.0` (used in water-heater tank mass calculations) overestimates by ~1.2% at typical 50°C tank temperatures. NIST IAPWS-IF97 reference data gives ρ(50°C) ≈ 988.0 kg/m³. The 1.2% bias propagates to tank thermal capacity, recovery time, and standby loss estimates.

## Current Behavior

A constant `WATER_DENSITY_KG_PER_M3 = 1000.0` is used uniformly for tank water mass. No temperature dependence. At a 50°C setpoint, the tank's effective thermal capacity is overstated by 1.2%.

## Required Behavior

Choose one:

A. **Temperature-dependent density** — replace the constant with a function `water_density_kg_m3(t_celsius: f64) -> f64` implementing IAPWS-IF97 or a polynomial fit. Call this from any water-heater code that needs tank mass.

B. **Cite the assumption** — keep the constant but rename to `WATER_DENSITY_REF_4C_KG_M3` and add a doc comment stating it is the 4°C reference density (the value at maximum density of liquid water) and noting the +1.2% bias at typical tank temperatures.

Recommended path: A. The IAPWS polynomial fit is cheap and the 1.2% bias is meaningful for tank thermal modelling. A simple polynomial:

```
ρ(T) = 999.83952 + 16.945176e-3·T - 7.9870401e-3·T² - 46.170461e-6·T³ + 105.56302e-9·T⁴ - 280.54253e-12·T⁵   (Kell 1975)
```

valid for 0-100°C with ±0.001 kg/m³ accuracy.

## Approach

1. Add `water_density_kg_m3(t_celsius: f64) -> f64` to `hares-physics`. Implement Kell's 1975 polynomial fit (cited above).
2. Replace the `WATER_DENSITY_KG_PER_M3 = 1000.0` constant uses in water-heater code with `water_density_kg_m3(tank_temp_c)`.
3. Add a unit test verifying ρ(4°C) ≈ 999.97 kg/m³, ρ(20°C) ≈ 998.21, ρ(50°C) ≈ 988.04, ρ(80°C) ≈ 971.79.
4. Update tank mass and standby loss calculations to consume the temperature-dependent density.

## Definition of Done

- [ ] `water_density_kg_m3(t_celsius)` function exists in `hares-physics`
- [ ] Kell 1975 polynomial implementation with citation
- [ ] Unit test verifies four reference points (4, 20, 50, 80°C)
- [ ] Water heater code consumes the temperature-dependent density
- [ ] No `WATER_DENSITY_KG_PER_M3 = 1000.0` constant used in tank mass calculations
- [ ] Annual standby loss change quantified for a representative tank (expect +1-2% recovery time)

## Verification

```bash
cargo test -p hares-physics water_density
cargo test -p hares-equipment water_heater
```

## References

- Kell, G. S. (1975) *Density, Thermal Expansivity, and Compressibility of Liquid Water from 0° to 150°C*. J. Chem. Eng. Data 20:97–105 — polynomial fit cited above.
- IAPWS-IF97 *Industrial Formulation 1997 for the Thermodynamic Properties of Water and Steam* — primary reference; IAPWS Release on the Industrial Formulation 1997.
- NIST WebBook https://webbook.nist.gov/chemistry/fluid/ — verify polynomial against tabulated data.

## Related Tickets

- 074-hpwh-zone-heat-category-mismatch
- 121-combi-boiler-indirect-tank-support
