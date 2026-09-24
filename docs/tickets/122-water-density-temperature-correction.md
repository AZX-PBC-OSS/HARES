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

---

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-22

### Code Confirmation

- [x] **Referenced line numbers still match** — `WATER_DENSITY_KG_PER_M3 = 1000.0` is defined at `crates/hares-equipment/src/water_heater/mod.rs:14`. Imported and used in `tank.rs` at lines 10, 280, 412, 437, 454, 575–577, 606–607, 615, and 19 further call sites (24 uses in tank.rs total). Also used in `gas.rs:475–476`, `heat_pump_wh.rs:684–685`, `resistance.rs:517–518`.
- [x] **Described logic matches current implementation** — No temperature correction exists. Every thermal-mass, energy-accounting, and draw-conversion calculation in the water-heater stack uses the fixed constant 1000.0 kg/m³ regardless of temperature.
- [x] **OCHRE cross-check** — `vendors/OCHRE/ochre/Models/Water.py:8` defines `water_density = 1000  # kg/m^3` (constant). OCHRE also applies no temperature correction. **HARES matches OCHRE**; both share the same simplifying assumption. The divergence the ticket proposes would be an intentional improvement over OCHRE.
- [x] **Existing test note** — `crates/hares-equipment/tests/oracle_24h.rs:566` uses `let water_density_kg_m3 = 998.0` in one analytical cross-check (corresponds to ~21°C per Kell 1975), but this is local to that test and does not affect production code.
- [x] **EnergyPlus cross-check** — EnergyPlus water-heater models (e.g., `WaterHeaterMixed`) use a fixed liquid water density constant (typically 1000 kg/m³ or the IAPWS value at the reference temperature, not a temperature-varying function in the thermal-mass loop). This is consistent with HARES's current approach; the ticket proposes an accuracy improvement beyond the EnergyPlus baseline.

### Web-Verified Citations

#### Citation 1 — Kell 1975 polynomial

- **What the ticket claims**: A 5th-degree polynomial `ρ(T) = 999.83952 + 16.945176e-3·T − 7.9870401e-3·T² − 46.170461e-6·T³ + 105.56302e-9·T⁴ − 280.54253e-12·T⁵` from *J. Chem. Eng. Data* 20:97–105.
- **Source found**: Kell, G. S. (1975), [Semantic Scholar record](https://www.semanticscholar.org/paper/Density,-thermal-expansivity,-and-compressibility-Kell/52be0e74ef74dc20504bde2f492c03924427c44d); cross-checked via [Zelentech water-density tool](https://www.zelentech.co/en/tools/water-density/) which cites Kell (1975) and IAPWS data.
- **Quoted passage** (Zelentech, citing Kell 1975 and IAPWS):
  > Temperature Correction Formula (Fresh Water):
  > ρ(T) = [999.83952 + 16.945176·T − 7.9870401e-3·T² − 46.170461e-6·T³ + 105.56302e-9·T⁴ − 280.54253e-12·T⁵] / [1 + 16.879850e-3·T]
  > (valid 0–100°C, accuracy ±0.02 kg/m³)
- **Verdict**: ⚠️ **PARTIALLY CORRECT — TICKET FORMULA HAS TWO ERRORS**:
  1. **Missing denominator**: The correct Kell 1975 formula is a *rational* polynomial divided by `(1 + 16.879850e-3·T)`. The ticket omits this denominator entirely.
  2. **Wrong T¹ coefficient**: The ticket writes `16.945176e-3·T` (= 0.016945176·T) but the correct value is `16.945176·T` — the coefficient is 1000× too small.

  **Impact**: The formula as written in the ticket, if implemented literally, would produce wildly incorrect densities (e.g., 975.52 kg/m³ at 50°C instead of 988.04 kg/m³). The four reference *values* the ticket quotes (999.97, 998.21, 988.04, 971.79) are consistent with the **correct** rational form and match NIST to within 0.05 kg/m³.

#### Citation 2 — IAPWS-IF97, ρ(50°C) ≈ 988 kg/m³

- **What the ticket claims**: NIST IAPWS-IF97 gives ρ(50°C) ≈ 988.0 kg/m³.
- **Source found**: NIST WebBook saturation table — [Saturation Properties for Water](https://webbook.nist.gov/cgi/fluid.cgi?TLow=0&THigh=100&TInc=10&Digits=5&ID=C7732185&Action=Load&Type=SatP&TUnit=C&PUnit=MPa&DUnit=kg%2Fm3&HUnit=kJ%2Fkg&WUnit=m%2Fs&VisUnit=uPa*s&STUnit=N%2Fm) (based on IAPWS-95 formulation).
- **Quoted passage** (NIST WebBook saturation table, fetched 2026-05-22):

  | Temperature | Liquid density (kg/m³) |
  |---|---|
  | 0°C  | 999.79 |
  | 20°C | 998.16 |
  | 50°C | 987.99 |
  | 80°C | 971.76 |
  | 100°C| 958.34 |

- **Verdict**: **CONFIRMED**. ρ(50°C) = 987.99 kg/m³ per NIST (the ticket rounds to 988.0). The claimed 1.2% bias is correct: (1000.0 − 987.99) / 1000.0 = 1.20%.

#### Citation 3 — NIST WebBook URL

- **What the ticket claims**: https://webbook.nist.gov/chemistry/fluid/ as a verification source.
- **Source found**: [NIST WebBook Thermophysical Properties of Fluid Systems](https://webbook.nist.gov/chemistry/fluid/)
- **Verdict**: **CONFIRMED**. The URL is valid and the tool provides the IAPWS-based saturation data used above.

#### Citation 4 — Four reference values (4°C, 20°C, 50°C, 80°C)

The ticket specifies `ρ(4°C) ≈ 999.97`, `ρ(20°C) ≈ 998.21`, `ρ(50°C) ≈ 988.04`, `ρ(80°C) ≈ 971.79` as unit-test targets.

Evaluated against the **correct** Kell rational polynomial and NIST WebBook:

| T    | Kell rational | NIST WebBook | Ticket claim | Match? |
|------|---------------|-------------|--------------|--------|
| 4°C  | 999.972       | ~999.97     | 999.97       | ✓      |
| 20°C | 998.204       | 998.16      | 998.21       | ✓      |
| 50°C | 988.036       | 987.99      | 988.04       | ✓      |
| 80°C | 971.798       | 971.76      | 971.79       | ✓      |

- **Verdict**: **CONFIRMED**. All four reference values are correct and match both Kell (rational form) and NIST to within the ticket's stated accuracy of ±0.001 kg/m³ (Kell) / measurement uncertainty (NIST). Note that NIST WebBook does not tabulate 4°C exactly at standard increments; the ~999.97 value is consistent with the known maximum density of water at 3.98°C.

### Legitimacy

- **Verdict**: **Partially Legitimate**
- **Rationale**: The core bug is real and well-evidenced — `WATER_DENSITY_KG_PER_M3 = 1000.0` is used in all 28+ call sites across the water-heater stack with no temperature correction, producing a confirmed 1.20% overstatement of thermal mass at the typical 50°C setpoint. The NIST IAPWS value (987.99 kg/m³ at 50°C) and the Kell (1975) rational polynomial both independently confirm this bias. OCHRE shares the same simplification (also uses 1000 kg/m³ constant), so this would be an intentional accuracy improvement, not a parity fix. The four reference test values in the ticket are correct. However, the **polynomial formula as typeset in the ticket is wrong in two ways**: (1) it omits the denominator `(1 + 16.879850e-3·T)`, making it not a rational polynomial but a plain polynomial numerator; and (2) the T¹ coefficient is written as `16.945176e-3` (1000× too small) instead of `16.945176`. Implementing the ticket's formula verbatim would introduce a far larger error (~12 kg/m³ at 50°C, ~4.9% error) than the bug it is trying to fix. The ticket must be corrected before implementation.

### Proposed Fix Summary

1. Add `pub fn water_density_kg_m3(t_celsius: f64) -> f64` to `crates/hares-physics/src/` (new file `water_density.rs` or appended to `constants.rs`). Implement the **correct** Kell 1975 rational polynomial:
   ```rust
   pub fn water_density_kg_m3(t_celsius: f64) -> f64 {
       let t = t_celsius;
       let num = 999.83952 + 16.945176 * t - 7.987_040_1e-3 * t * t
           - 46.170_461e-6 * t.powi(3) + 105.563_02e-9 * t.powi(4)
           - 280.542_53e-12 * t.powi(5);
       let den = 1.0 + 16.879_850e-3 * t;
       num / den
   }
   ```
2. Expose it from `hares-physics/src/lib.rs` as a public symbol.
3. Replace `WATER_DENSITY_KG_PER_M3` in the water-heater stack (all 28+ call sites) with `water_density_kg_m3(node_temp_c)` or `water_density_kg_m3(tank_avg_temp_c)` as appropriate. Do NOT change uses in flow-unit conversions (kg/s ↔ m³/s) that operate at mains temperature — use the corresponding node temperature there.
4. **Do NOT implement the formula as written in the ticket** — correct the T¹ coefficient and add the denominator first.

### Test Written

- **File**: `crates/hares-physics/tests/physics_validation_tests.rs` (appended two tests)
- **What they test**:
  - `ticket_122_water_density_four_reference_points` — asserts `water_density_kg_m3` returns values within ±0.05 kg/m³ of NIST/Kell at 4°C, 20°C, 50°C, 80°C. **Currently fails to compile** because `water_density_kg_m3` is not yet exported from `hares_physics`.
  - `ticket_122_constant_bias_at_typical_tank_temp` — asserts the bias of using 1000.0 kg/m³ at 50°C is in the range 1.0–1.5% (per NIST: 1.20%). Also fails to compile for the same reason.
- **Run with**: `cargo test -p hares-physics water_density`
