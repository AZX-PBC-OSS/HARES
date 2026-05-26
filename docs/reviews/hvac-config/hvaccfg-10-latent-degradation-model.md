# Henderson-Rengarajan latent degradation: parameters and EnergyPlus alignment
**Review ID**: hvaccfg-10
**Category**: hvac-config
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-equipment/src/hvac/latent_degradation.rs`
- `crates/hares-equipment/src/hvac/coil_physics.rs` (contains the actual model: `LatentDegradationParams` struct and `effective_shr_with_latent_degradation()` function)
- `crates/hares-equipment/src/hvac/air_conditioner.rs` (parameter instantiation and call site)

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/DXCoils.cc` — `CalcEffectiveSHR()` at L12529–L12679
- `vendors/EnergyPlus/idd/versions/V26-1-0-Energy+.idd` — parameter suggestions for `Coil:Cooling:DX` at L52638–L52674 and `Coil:Cooling:DX:CurveFit:OperatingMode` at L52242–L52278
- `vendors/EnergyPlus/src/EnergyPlus/Coils/CoilCoolingDXCurveFitSpeed.cc` — curve-fit coil variant at L780–L887
- `vendors/EnergyPlus/src/EnergyPlus/WaterToAirHeatPumpSimple.cc` — WHP variant at L3433–L3572
- `vendors/EnergyPlus/src/EnergyPlus/VariableSpeedCoils.cc` — VS coil variant at L7629–L7751

## Findings

### Finding 1: `twet_rated_s` is hardcoded to 1500 s; EnergyPlus IDD suggests 1000 s
**Severity**: medium  
**Code Location**: `crates/hares-equipment/src/hvac/air_conditioner.rs:611–613`  
**Description**: The latent degradation parameters are assigned at startup:
```rust
self.latent_degradation = LatentDegradationParams {
    twet_rated_s: 1500.0,   // ← claimed to be "EnergyPlus residential default"
    gamma_rated: 1.5,
    max_cycling_rate: 3.0,
    latent_time_constant_s: 45.0,
};
```
The accompanying comment on L611 states "EnergyPlus residential DX coil defaults (Engineering Reference §16.5)."

However, the EnergyPlus IDD (`V26-1-0-Energy+.idd:52648`) for `Coil:Cooling:DX` specifies a **suggested value of 1000 s** for the "Nominal Time for Condensate Removal to Begin" field (range 0–3000 s, programmatic default 0.0). The `Coil:Cooling:DX:CurveFit:OperatingMode` object likewise suggests 1000 s (`V26-1-0-Energy+.idd:52273`). Every EnergyPlus test file and unit test that enables the latent degradation model uses 1000 s, not 1500 s.

The test at `air_conditioner.rs:4480–4482` asserts the value *must* be 1500 s:
```rust
assert!(
    (eq.core.latent_degradation.twet_rated_s - 1500.0).abs() < 1e-9,
    "twet_rated_s must be 1500 s (EnergyPlus residential default), got {}",
    eq.core.latent_degradation.twet_rated_s
);
```
This assertion message is factually incorrect; EnergyPlus has no "1500 s residential default."

**Root Cause**: The value 1500 s appears to have been chosen without confirming against the EnergyPlus source code or IDD. The source-of-truth comment references "Engineering Reference §16.5" but the EnergyPlus IDD and all test files consistently use 1000 s. The reference does not exist anywhere in the vendored EnergyPlus source tree.

**Impact**: At very low part-load ratios (RTF ≤ ~0.35), the `toff_capped` is bound by `2*twet/gamma` rather than `toff_base`. A larger `twet` (1500 vs 1000) means the cap is at 2000 s instead of 1333 s (at `gamma=1.5`), allowing a longer effective off-cycle and therefore slightly more latent degradation. At moderate-to-high RTFs, `toff_base` is always below the `2*twet/gamma` threshold so the twet value has no effect. The practical impact is limited to extremely low part-load conditions where degradation is generally already at maximum (SHR→1.0), but the parameter deviation means the model does not match EnergyPlus's intended default behavior.

### Finding 2: Rated wet-bulb depression uses 7.222 °C instead of EnergyPlus's 7.3 °C
**Severity**: low  
**Code Location**: `crates/hares-equipment/src/hvac/coil_physics.rs:25–26, 127`  
**Description**: HARES computes the rated wet-bulb depression as the difference of its AHRI temperature constants:
```rust
const HR_RATED_DB_C: f64 = 26.666_666_7;
const HR_RATED_WB_C: f64 = 19.444_444_4;
// rated_depression = 26.666_666_7 - 19.444_444_4 = 7.222_222_3 K
```
This is used to normalise the `gamma` parameter at line 128–129:
```rust
let gamma = params.gamma_rated * lat_ratio * (db_wb_depression / rated_depression);
```

EnergyPlus hardcodes the rated depression as `(26.7 - 19.4) = 7.3` K in every `CalcEffectiveSHR` variant (`DXCoils.cc:12618`, `CoilCoolingDXCurveFitSpeed.cc:826`, `WaterToAirHeatPumpSimple.cc:3520`, `VariableSpeedCoils.cc:7700`):
```cpp
Gamma = Gamma_Rated * QLatRated * (EnteringDB - EnteringWB) / ((26.7 - 19.4) * QLatActual + 1.e-10);
```

**Root Cause**: HARES uses a different conversion of 80°F/67°F to Celsius (26.67/19.44) than EnergyPlus (26.7/19.4). The more precise conversion differs by ~0.078 K in the depression.

**Impact**: The gamma parameter differs by a factor of 7.3/7.222 = 1.011 (~1.1%). This propagates to the `aa` term in the To fixed-point solver, causing a proportional (small) deviation in the effective SHR. The effect is negligible for practical purposes.

### Finding 3: Floating-point overflow/underflow guards are absent from the exponential calls
**Severity**: low  
**Code Location**: `crates/hares-equipment/src/hvac/coil_physics.rs:173–179, 184`  
**Description**: EnergyPlus safeguards the exponential argument in the To fixed-point solver and the LHR multiplier denominator to prevent floating overflow/underflow:
```cpp
// EnergyPlus DXCoils.cc L12656:
To2 = aa - Tcl * std::expm1(min(700.0, -To1 / Tcl));
// EnergyPlus DXCoils.cc L12664:
aa = std::exp(max(-700.0, -Ton / Tcl));
```

HARES calls the exponential functions without equivalent argument capping:
```rust
// coil_physics.rs L174:
let to_new = aa - tau * ((-to / tau).exp() - 1.0);
// coil_physics.rs L184:
let aa_exp = (-ton / tau).exp();
```

**Root Cause**: The guards were omitted during the translation from the C++ reference implementation.

**Impact**: At extremely large or small `-To1/Tcl` ratios (near the limits of f64 exponent range, ~±709), these unguarded `exp()` calls could produce `inf` or `0.0` values. In practice, with `Tcl` (tau) values of ~45 s and typical `To1`/`ton` values of tens to thousands of seconds, the arguments stay well within the safe range. No observable impact under normal operating conditions.

### Finding 4: Companion heating coil RTF handling differs from EnergyPlus
**Severity**: low  
**Code Location**: `crates/hares-equipment/src/hvac/coil_physics.rs:161–162`  
**Description**: When a companion heating coil runs during cooling off-cycles, HARES scales the effective off-time proportionally:
```rust
let toff_effective = toff_capped * rtf / heating_rtf_val;  // L162
```

EnergyPlus recalculates the heating coil's cycle times and adjusts the off-time via a cycle-overlap formula (`DXCoils.cc:12640–12646`):
```cpp
if (HeatingRTF < 1.0 && HeatingRTF > RTF) {
    Ton_heating = 3600.0 / (4.0 * Nmax * (1.0 - HeatingRTF));
    Toff_heating = 3600.0 / (4.0 * Nmax * HeatingRTF);
    Ton_heating += max(0.0, min(Ton_heating, (Ton + Toffa) - (Ton_heating + Toff_heating)));
    Toffa = min(Toffa, Ton_heating - Ton);
}
```

**Root Cause**: The HARES simplification uses proportional scaling (`rtf / heating_rtf_val`) rather than the full cycle-overlap adjustment in EnergyPlus.

**Impact**: Currently the heating RTF parameter is always passed as `None` from the call site (`air_conditioner.rs:1287`), so this code path is never exercised. If it were activated (e.g., for heat pump + auxiliary heat configurations), the HARES approximation would produce different results from EnergyPlus.

### Finding 5: Module `latent_degradation.rs` does not contain the latent degradation model
**Severity**: low  
**Code Location**: `crates/hares-equipment/src/hvac/latent_degradation.rs` (entire 54-line file)  
**Description**: The file header states `//! Henderson-Rengarajan latent degradation model and coil Ao computation.` but the file only contains the `compute_coil_ao_by_stage()` function and AHRI condition constants. The actual `LatentDegradationParams` struct, `is_active()` check, and `effective_shr_with_latent_degradation()` function all reside in `coil_physics.rs`. The file/module naming is misleading.

**Root Cause**: The file was created during a structural refactor (the "Extracted from `air_conditioner.rs`" comment on line 3 suggests this) but the latent degradation model itself was not moved into this module.

**Impact**: Code navigation confusion only; no functional impact. Someone searching for the model by module name will find the wrong file.

### Formula Verification (informational)

The core algorithm in `effective_shr_with_latent_degradation()` (`coil_physics.rs:96–194`) was compared against the EnergyPlus `CalcEffectiveSHR()` (`DXCoils.cc:12529–12679`). The following implementation steps are mathematically correct:

| Step | HARES (`coil_physics.rs`) | EnergyPlus (`DXCoils.cc`) | Match |
|------|--------------------------|---------------------------|-------|
| Early exit if `RTF >= 1.0` | L109–111 | L12609–12612 | Yes |
| Twet adjustment for actual conditions | L118–122 | L12617 | Yes |
| Gamma adjustment for actual conditions | L124–132 | L12618¹ | Partial (see Finding 2) |
| Ton / Toff from cycling rate | L141–150 | L12621–12622 | Yes |
| Toff capped at `2*Twet/Gamma` | L155–158 | L12625–12629 | Yes |
| `aa` computation | L168 | L12650 | Yes |
| To fixed-point solver | L172–180 | L12651–12658² | Partial (see Finding 3) |
| LHR multiplier | L184–190 | L12664–12666 | Yes |
| SHR_eff = 1 – (1 – SHRss) × LHRmult | L192 | L12669 | Yes |
| Clamp to [SHRss, 1.0] | L193 | L12671–12676 | Yes |
| RTF=1.0 → SHR unchanged | L109–111 | L12609–12612 | Yes |
| SHR increases as PLR decreases | Verified by Test 5,6 | Per Henderson 1996 | Yes |

¹ EnergyPlus uses `(26.7 - 19.4)` depression; HARES uses `(26.666... - 19.444...)`
² EnergyPlus caps `expm1` argument at 700; HARES omits the cap

## Summary
- **Total findings**: 5
- **Medium**: 1 (twet_rated_s parameter mismatch)
- **Low**: 4 (rated depression mismatch, missing overflow guards, heating RTF divergence, misleading module name)
- **Informational**: Formula verification confirms core implementation aligns with EnergyPlus

## Recommendations
1. **Change `twet_rated_s` from 1500 to 1000** to match the EnergyPlus IDD suggested value for `Coil:Cooling:DX`. Update the comment at `air_conditioner.rs:611` to cite the specific EnergyPlus IDD source. Update the test assertion at `air_conditioner.rs:4480–4482`.
2. **Align the rated wet-bulb depression to 7.3 °C** by using `(26.7 - 19.4)` to match EnergyPlus's hardcoded constant, or document the rationale for using 7.222 °C.
3. **Add overflow/underflow guards** to the `exp()` calls in the To fixed-point solver: clamp `-to/tau` at 700 and `-ton/tau` at -700.
4. **Consider relocating** `LatentDegradationParams` and `effective_shr_with_latent_degradation()` to `latent_degradation.rs` to match the module name, or rename the module to reflect its actual scope.

## References / Citations
- Henderson, H.I. and Rengarajan, K. "A Model to Predict the Latent Capacity of Air Conditioners and Heat Pumps at Part-Load Conditions with Constant Fan Operation." ASHRAE Transactions, Vol. 102, Part 1, pp. 266–274 (1996).
- EnergyPlus `CalcEffectiveSHR()`: `vendors/EnergyPlus/src/EnergyPlus/DXCoils.cc:12529–12679`
- EnergyPlus IDD `Coil:Cooling:DX` parameters: `vendors/EnergyPlus/idd/versions/V26-1-0-Energy+.idd:52638–52674`
- EnergyPlus IDD `Coil:Cooling:DX:CurveFit:OperatingMode` parameters: `vendors/EnergyPlus/idd/versions/V26-1-0-Energy+.idd:52242–52278`
