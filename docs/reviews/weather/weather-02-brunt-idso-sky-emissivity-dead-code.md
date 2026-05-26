# Brunt and Idso sky emissivity models implemented but unwired

**Review ID**: weather-02
**Category**: weather
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-io/src/epw.rs` (lines 617–681, specifically 627–651)

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc` (lines 121–130, 163–174, 3192–3217, 6695–6892)
- `vendors/EnergyPlus/src/EnergyPlus/WeatherManager.hh` (lines 163–174, 207–208)

## Findings

### Finding 1: [Severity: medium] Brunt and Idso emissivity functions are dead code with no configuration pathway

**Description**: `brunt_sky_emissivity()` (`epw.rs:629–632`) and `idso_sky_emissivity()` (`epw.rs:647–651`) are fully implemented clear-sky emissivity functions but are marked `#[allow(dead_code)]` and are never called anywhere in the HARES codebase. Their helper `magnus_saturation_pressure_hpa()` (`epw.rs:680–682`) is also dead code, used only by these two functions. The doc comments on each state "Not wired into the compute_sky_temp_c cascade; available for future model selection."

EnergyPlus exposes all four emissivity models (Clark-Allen, Brunt, Idso, Berdahl-Martin) as user-selectable options through the `WeatherProperty:SkyTemperature` input object (`WeatherManager.cc:6699–6892`). The model is chosen via the `SkyTempModel` enum (`WeatherManager.hh:163–174`) with default `ClarkAllen`. All four models are applied in the unified `CalcSkyEmissivity()` function (`WeatherManager.cc:3192–3217`).

HARES has no equivalent selection mechanism. The `compute_sky_temp_c()` cascade (`epw.rs:558–577`) is hardcoded: IR inversion → Berdahl-Martin with Walton correction → Clark-Allen fallback. There is no YAML/TOML config key, no `WeatherProperty:SkyTemperature` analogue, and no CLI flag to select a sky emissivity model.

**Code Location**:
- `crates/hares-io/src/epw.rs:627–632` (`brunt_sky_emissivity`, `#[allow(dead_code)]`)
- `crates/hares-io/src/epw.rs:645–651` (`idso_sky_emissivity`, `#[allow(dead_code)]`)
- `crates/hares-io/src/epw.rs:678–682` (`magnus_saturation_pressure_hpa`, `#[allow(dead_code)]`)
- `crates/hares-io/src/epw.rs:558–577` (`compute_sky_temp_c`, hardcoded cascade)

**Root Cause**: The functions appear to have been implemented speculatively in anticipation of a sky-model selection feature that was never completed. They were left in place as future expansion points (as stated in the doc comments) but with `#[allow(dead_code)]` to suppress compiler warnings.

**Impact**: Maintenance burden from unused code (3 functions, ~35 lines of implementation, ~25 lines of doc comments). Code bloat with no runtime benefit. Risk of bit-rot: the implementations may diverge from reference (see Finding 2) without tests catching it. The `#[allow(dead_code)]` annotations could mask legitimate warnings during refactoring.

### Finding 2: [Severity: medium] Brunt and Idso implementations use a different water-vapor pressure source than EnergyPlus

**Description**: EnergyPlus computes water vapor partial pressure for Brunt and Idso using the *dry-bulb temperature* as the saturation temperature, scaled by relative humidity:
```cpp
// EnergyPlus: CalcSkyEmissivity() at WeatherManager.cc:3204–3209
double const PartialPress = RelHum * Psychrometrics::PsyPsatFnTemp(state, DryBulb) * 0.01;
ESky = 0.618 + 0.056 * pow(PartialPress, 0.5);       // Brunt
ESky = 0.685 + 0.000032 * PartialPress * exp(1699 / (DryBulb + Constant::Kelvin));  // Idso
```

HARES computes vapor pressure from the *dew-point temperature* via the Magnus formula, assuming saturation at the dew point:
```rust
// HARES: epw.rs:629–631, 647–650
let p_wv_hpa = magnus_saturation_pressure_hpa(t_dp_c);  // saturation at T_dp
0.618 + 0.056 * p_wv_hpa.sqrt();       // Brunt
0.685 + 3.2e-5 * p_wv_hpa * (1699.0 / t_db_k).exp();   // Idso
```

These are two physically distinct approaches:
- The EnergyPlus method uses the *actual vapor pressure* in the ambient air (saturation at T_db × RH), which is the standard definition.
- The HARES method uses the *saturation pressure at the dew point*, which is mathematically equivalent (saturation at T_dp = actual vapor pressure) *if* the dew point is correctly defined. However, EPW dew-point values come from weather files, not from psychrometric calculation, and in dry conditions may differ from the equilibrium dew point implied by T_db and RH.

**Impact**: If the Brunt or Idso functions were ever wired into the cascade, their output would differ from EnergyPlus's Brunt/Idso output for the same EPW row. This would be a silent correctness bug. The models would need to accept `rel_humidity_pct` as an additional parameter (Brunt already only takes `t_dp_c`; Idso takes `t_db_c` and `t_dp_c`). A redesign of the function signatures would be needed before activation.

### Finding 3: [Severity: low] No design document or tracking issue for sky model selection feature

**Description**: The doc comments state the functions are "available for future model selection," but there is no corresponding ticket, design document, or roadmap item in the repository that tracks when or how sky model selection would be implemented. The architecture review (arch-02-core-state-structs.md, Finding 3) identifies the broader architectural limitation — `opaque_sky_cover` is not exposed on `WeatherState` — but does not specifically mention the need to wire Brunt/Idso.

**Code Location**: Commit log and ticket index; `docs/reviews/architecture/arch-02-core-state-structs.md:72–76`

**Impact**: Without a tracking artifact, the dead code may persist indefinitely. Future contributors cannot determine whether the functions are vestigial, planned, or abandoned.

## Summary

- Total findings: 3
- Critical: 0
- High: 0
- Medium: 2
- Low: 1

## Recommendations

1. **Decide on sky model selection**: The project should either (a) commit to implementing user-selectable sky temperature models (requiring a YAML config key, `SkyTempModel` enum, and wiring into `compute_sky_temp_c`), or (b) acknowledge that the current hardcoded cascade (IR → Berdahl-Martin+Walton → Clark-Allen) is sufficient for residential simulation and remove the dead code.

2. **If option (b) — Remove dead code**: Delete `brunt_sky_emissivity()`, `idso_sky_emissivity()`, and `magnus_saturation_pressure_hpa()` from `epw.rs`. Remove the test imports of `brunt_sky_emissivity` and `idso_sky_emissivity` from the test module (`epw.rs:948, 949`). This eliminates ~60 lines of maintenance burden with no loss of functionality.

3. **If option (a) — Wire the models**: Before activation, update the Brunt and Idso implementations to match EnergyPlus's water-vapor pressure calculation method (using `dry_bulb_c` + `rel_humidity_pct` rather than dew-point Magnus saturation). This requires changing the function signatures to accept `rel_humidity_pct`. A `SkyTempModel` enum and configuration pathway would need to be designed, along with exposing `opaque_sky_cover` on `WeatherState` (as noted in arch-02-core-state-structs.md).

4. **Either way — Create a tracking artifact**: If the models are retained, open a ticket describing the sky-model-selection feature, its prerequisites (signature redesign, config plumbing, `WeatherState` fields), and its priority relative to other work.

## References / Citations

- Clark, G. and Allen, C. (1978). "The Estimation of Atmospheric Radiation for Clear and Cloudy Skies." Proc. 2nd National Passive Solar Conference (AS/ISES), pp. 675-678.
- Berdahl, P. and Martin, M. (1984). "Emissivity of Clear Skies." Solar Energy, 32(5), 663-664.
- Li, M., Jiang, Y. & Coimbra, C.F.M. (2017). "On the determination of atmospheric longwave irradiance under all-sky conditions." Solar Energy, 144, 40-48.
- Brunt, D. (1932). "Notes on radiation in the atmosphere." Q.J.R. Meteorol. Soc., 58, 389-420.
- Idso, S.B. (1981). "A set of equations for full spectrum and 8- to 14-μm and 10.5- to 12.5-μm thermal radiation from cloudless skies." Water Resources Research, 17(2), 295-304.
- EnergyPlus WeatherManager.cc:121–130 (SkyTempModel enum), 3192–3217 (CalcSkyEmissivity), 6699–6892 (WeatherProperty:SkyTemperature parsing)
- `docs/reviews/architecture/arch-02-core-state-structs.md:72–76` — prior architectural finding on sky model selection limitation
