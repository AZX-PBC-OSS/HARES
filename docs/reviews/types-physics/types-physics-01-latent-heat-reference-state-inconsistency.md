# Latent heat of vaporization reference-state inconsistency (2450 vs 2501 kJ/kg)
**Review ID**: types-physics-01
**Category**: types-physics
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/psychrometrics.rs` — psychrometric enthalpy, wet-bulb, humidity ratio kernels
- `crates/hares-physics/src/constants.rs` — shared physical constants with citations
- `crates/hares-equipment/src/hvac/dehumidifier.rs` — dehumidifier latent heat constant usage
- `crates/hares-envelope/src/humidity_solver.rs` — humidity solver h_fg configuration
- `crates/hares-core/src/dwelling/mod.rs` — dwelling-level moisture mass balance
- `crates/hares-core/src/invariants.rs` — moisture mass balance invariants
- `crates/hares-envelope/src/thermal_solver/mod.rs` — thermal domain latent heat output

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/Psychrometrics.cc` — `PsyTwbFnTdbWPb_raw` (lines 339–558), `PsyPsatFnTemp` (lines 640–773)
- `vendors/EnergyPlus/src/EnergyPlus/Psychrometrics.hh` — `PsyHFnTdbW` (lines 646–675), `PsyHfgAirFnWTdb` (lines 596–619), `PsyHgAirFnWTdb` (lines 621–644), `PsyWFnTdbTwbPb` (lines 1442–1458)

## Findings

### Finding 1: [Severity: medium]
**Description**: `LATENT_HEAT_VAPORISATION_J_KG = 2_450_000` (the ~20°C reference) is defined as a public constant but is never imported or used by any production or test code. Its presence is a latent copy-paste hazard: a developer adding a new moisture source (e.g. a humidifier or evaporative cooler) could mistakenly import this constant and introduce a ~1.9% systematic error in moisture mass balance that would survive all existing regression tests.

**Code Location**:
- Definition: `crates/hares-physics/src/constants.rs:63`
- Module-level doc comment: `crates/hares-physics/src/constants.rs:9–10` (correctly warns against use, but the per-item doc on lines 54–62 misleadingly says "Used in moisture balance calculations" — this is no longer true since the codebase migrated all moisture balance to the 0°C reference.)

**Root Cause**: The HARES codebase originally used ~2450 kJ/kg in moisture balance calculations (the ~20°C reference, rounded down from the ASHRAE Table 2 interpolated value of 2454 kJ/kg). During infrastructure migration, all consumers were switched to `LATENT_HEAT_VAPORISATION_0C_J_KG` (2,501 kJ/kg) for consistency with the enthalpy formula and humidity solver. The old constant was retained with a warning comment but never removed. A `grep` across the entire `crates/` tree confirms zero imports of `LATENT_HEAT_VAPORISATION_J_KG` outside of `constants.rs` itself.

**Impact**: Currently zero — no runtime code uses this constant. However, a single mistaken `use hares_physics::constants::LATENT_HEAT_VAPORISATION_J_KG;` in a new evaporative cooler, ERV, or humidifier module would silently create a ~2% latent load error. All existing regression tests for enthalpy and moisture balance would still pass because they use the correct constant.

**Recommendation**: Either delete the constant entirely or mark it `#[deprecated]` / `#[allow(dead_code)]` with a doc comment explicitly stating the exact error magnitude if misused.

### Finding 2: [Severity: medium]
**Description**: EnergyPlus has an internal reference-state inconsistency between its own enthalpy and wet-bulb formulas. `PsyHFnTdbW` uses h_fg = 2500.94 kJ/kg, cp_da = 1004.84 J/(kg·K), and cp_v = 1858.95 J/(kg·K), while `PsyTwbFnTdbWPb_raw` uses h_fg = 2501.0 kJ/kg, cp_da = 1.006 kJ/(kg·K), and cp_v = 1.86 kJ/(kg·K). This means E+ cannot be used as an authoritative reference for which constant set is "correct" — it uses both itself.

**Code Location**:
- `PsyHFnTdbW` (energy balance enthalpy): `vendors/EnergyPlus/src/EnergyPlus/Psychrometrics.hh:663`
  ```cpp
  return 1.00484e3 * TDB + max(dW, 1.0e-5) * (2.50094e6 + 1.85895e3 * TDB);
  ```
- `PsyTwbFnTdbWPb_raw` (psychrometrics): `vendors/EnergyPlus/src/EnergyPlus/Psychrometrics.cc:492`
  ```cpp
  newW = ((2501.0 - 2.326 * WBT) * Wstar - 1.006 * (TDB - WBT)) / (2501.0 + 1.86 * TDB - 4.186 * WBT);
  ```
- `PsyHfgAirFnWTdb` (temperature-dependent latent heat): `vendors/EnergyPlus/src/EnergyPlus/Psychrometrics.hh:618`
  ```cpp
  return (2500940.0 + 1858.95 * Temperature) - (4180.0 * Temperature);
  // At 0°C: 2500.94 kJ/kg; at 20°C: ~2454.5 kJ/kg
  ```

**Root Cause**: EnergyPlus evolved over decades with contributions from multiple authors. The wet-bulb function (authored 1976 by George Shih) uses ASHRAE round constants including cp_da = 1.006 for the dry-air term, while the enthalpy function uses 1.00484e3 from a different reference. The codebase comments cite ASHRAE HOF 1972 different editions for different functions. The `PsyHfgAirFnWTdb` function in the header computes a temperature-linearized h_fg that yields 2454.5 kJ/kg at 20°C — this is the origin of the 2454/2450 convention, not an ASHRAE standard value for moisture balance.

**Impact on HARES**: None directly — HARES does not replicate E+ numerical output. However, any benchmark comparison between HARES and E+ that involves moisture mass balance will diverge by up to ~0.1% from E+'s own inconsistency, not from a HARES error. This finding is documented here to prevent future debugging where an engineer expects HARES to match E+ latent load values exactly.

### Finding 3: [Severity: low]
**Description**: The ASHRAE standard enthalpy formula h = cp_da·T + W·(h_fg_at_0C + cp_v·T) is correctly implemented in HARES with the internally consistent constant set: cp_da = 1.006 kJ/(kg·K), h_fg = 2501 kJ/kg, cp_v = 1.86 kJ/(kg·K). This set forms a proper 0°C reference state (h = 0 J/kg at T = 0°C, W = 0). The test `enthalpy_matches_ashrae_hof_reference` at `psychrometrics.rs:599–623` validates this explicitly at multiple operating points (0°C dry, 20°C dry, 25°C at W=0.01).

**Code Location**:
- Enthalpy kernel: `crates/hares-physics/src/psychrometrics.rs:176–181`
- Constants used: `crates/hares-physics/src/constants.rs:28` (CP_DRY_AIR_KJ_KG_K), `:48` (LATENT_HEAT_VAPORISATION_0C_KJ_KG), `:71` (CP_WATER_VAPOUR_KJ_KG_K)
- Validation: `crates/hares-physics/src/psychrometrics.rs:599–623`

**Impact**: Positive — the reference-state convention is self-consistent. No corrective action needed.

### Finding 4: [Severity: low]
**Description**: EnergyPlus's newer `PsyWFnTdbTwbPb` function (in the header) uses a formula that omits the cp_da coefficient, writing:
```cpp
W = ((2501.0 - 2.381 * TWB) * WET - (TDB - TWB)) / (2501.0 + 1.805 * TDB - 4.186 * TWB)
```
The standard ASHRAE HOF 2021 Ch.1 Eq.35 form should be `(TDB - TWB)` multiplied by cp_da (1.006), i.e., `1.006 * (TDB - TWB)`. HARES correctly includes the cp_da coefficient in its own wet-bulb kernel at `psychrometrics.rs:88`:
```rust
- SPECIFIC_HEAT_DRY_AIR_KJ_KG_K * (t_db_c - t_wb_c)
```
which resolves to `- 1.006 * (t_db_c - t_wb_c)`. HARES also uses the ASHRAE HOF 2021 psychrometer coefficient 2.381 kJ/(kg·K) (`SPECIFIC_HEAT_WET_BULB_ABOVE_FREEZE`) and the standard cp_v = 1.86 kJ/(kg·K) (`SPECIFIC_HEAT_WATER_VAPOUR_KJ_KG_K`), whereas the E+ header variant uses 1.805 for cp_v.

**Code Location**:
- E+ new formula (buggy): `vendors/EnergyPlus/src/EnergyPlus/Psychrometrics.hh:1448`
- HARES correct formula: `crates/hares-physics/src/psychrometrics.rs:87–90`

**Impact on HARES**: None — HARES correctly includes the cp_da coefficient. This finding confirms that HARES should not blindly align with E+ without verifying which E+ function variant is being compared against (the `.cc` raw variant is correct; the `.hh` variant has two issues).

## Summary
- Total findings: 4
- Medium: 2 (Findings 1, 2)
- Low: 2 (Findings 3, 4)
- Critical / High: 0

**The core concern of this review — that mixing a 2450 kJ/kg latent heat with cp_v·T terms calibrated for a 2501 kJ/kg reference would produce systematic enthalpy errors — is not an active bug in HARES.** All production code uses the 2501 kJ/kg reference consistently, and the 2450 kJ/kg constant (`LATENT_HEAT_VAPORISATION_J_KG`) is dead code. Historical bugs matching this pattern were already fixed (see regression tests in `dehumidifier.rs:925–968`, `humidity_solver.rs:499–510`, `thermal_solver/mod.rs:1360–1454`). The remaining risk is solely the silent reintroduction hazard from the lingering dead constant.

## Recommendations
1. Delete `LATENT_HEAT_VAPORISATION_J_KG` from `constants.rs` or gate it behind `#[cfg(test)]` / `#[deprecated]` so it cannot be imported accidentally by new production code.
2. Update the misleading comment at `constants.rs:57–58` ("Used in moisture balance calculations") — this statement is no longer accurate.
3. Document in the project AGENTS.md or architecture doc that HARES intentionally uses ASHRAE round values (1.006, 2501, 1.86) rather than the E+ higher-precision values (1.00484, 2500.94, 1.85895), so E+ benchmark comparisons should expect ~0.1% enthalpy divergence from this source alone.
4. Ensure any new moisture source module (humidifier, evaporative cooler) goes through a `Cargo.toml`-level `no_include` audit or compile-time check to prevent importing `LATENT_HEAT_VAPORISATION_J_KG` if it is not deleted.

## References / Citations
- ASHRAE 2017 Handbook of Fundamentals, Chapter 1, Eq. 30: `h = cp_da·T + W·(h_fg_0 + cp_v·T)`
- ASHRAE 2017 HOF Ch. 1, Table 2: h_fg at 0°C = 2501 kJ/kg, h_fg at 20°C = 2454 kJ/kg
- ASHRAE HOF 2021 Ch. 1, Eq. 35: psychrometer equation above-freezing wet-bulb with coefficient 2.381 kJ/(kg·K)
- EnergyPlus Psychrometrics.hh:663: `PsyHFnTdbW` uses h_fg = 2,500,940 J/kg (~2500.94 kJ/kg)
- EnergyPlus Psychrometrics.cc:492: `PsyTwbFnTdbWPb_raw` uses h_fg = 2501.0 kJ/kg
- EnergyPlus Psychrometrics.hh:618: `PsyHfgAirFnWTdb` computes temperature-dependent h_fg(T) = 2,500,940 − 2,321.05·T
- EnergyPlus Psychrometrics.hh:1448: `PsyWFnTdbTwbPb` omitted cp_da coefficient in numerator
- HARES psychrometrics.rs:176–181: `moist_air_enthalpy` implementation
- HARES constants.rs:48: `LATENT_HEAT_VAPORISATION_0C_KJ_KG = 2501.0`
- HARES constants.rs:63: `LATENT_HEAT_VAPORISATION_J_KG = 2_450_000` (dead constant)
