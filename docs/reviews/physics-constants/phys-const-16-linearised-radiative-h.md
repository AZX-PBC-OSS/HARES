# Linearised radiative h coefficient: verify h_rad = 4·ε·σ·T³ yields ~5.14 W/m²·K at 20°C with ε=0.9
**Review ID**: phys-const-16
**Category**: physics-constants
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/constants.rs:218-220` — `linearised_h_rad` function definition
- `crates/hares-physics/src/constants.rs:200-202` — `STEFAN_BOLTZMANN` constant
- `crates/hares-physics/src/constants.rs:255-306` — unit tests for `linearised_h_rad`
- `crates/hares-envelope/src/longwave_radiation.rs:230-241` — `linearised_h_r` wrapper (Celsius interface)

## Vendor/Reference Files Consulted

### EnergyPlus
- `vendors/EnergyPlus/src/EnergyPlus/ConvectionCoefficients.cc:665-678` — HSky/HGround/HAir: `σ·ε·(T₁⁴−T₂⁴)/(T₁−T₂)` linearised form
- `vendors/EnergyPlus/src/EnergyPlus/ThermalEN673Calc.cc:430` — exact `4·σ·(1/ε₁+1/ε₂−1)⁻¹·T³` (EN673 standard)
- `vendors/EnergyPlus/src/EnergyPlus/EcoRoofManager.cc:458-492` — `4·σ·ε₁·ε₂·T³` form
- `vendors/EnergyPlus/src/EnergyPlus/SolarCollectors.cc:1804-1854` — `σ·(T₁+T₂)(T₁²+T₂²)/(1/ε₁+1/ε₂−1)`
- `vendors/EnergyPlus/src/EnergyPlus/WindowManager.cc:5655-5659` — frame simplifications: `0.5·ε·σ·(T₁+T₂)³`
- `vendors/EnergyPlus/src/EnergyPlus/DataGlobalConstants.hh:618` — Stefan-Boltzmann: `5.6697E-8`

### OCHRE
- `vendors/OCHRE/ochre/Models/Envelope.py:237-249` — exterior linearisation: `dH/dT = 4·e_factor·T³` with `e_factor = ε·σ·A`, T_ref = 15°C
- `vendors/OCHRE/ochre/Models/Envelope.py:1048-1061` — interior linearisation: `1/(4·ε·σ·A·T³)`, T_ref = 20°C
- `vendors/OCHRE/ochre/utils/units.py:25` — `degC_to_K = 273.15`

## Findings

### Finding 1: [Severity: low]
**Description**: The `linearised_h_rad_at_20c_is_physically_reasonable` test (line 262–265) uses an overly wide assertion range `(4.0..=7.0).contains(&h)`. The expected value is 5.143 W/(m²·K), so the test would pass with a value up to 36% above or 22% below the correct result. Given that the subsequent `linearised_h_rad_correctness` test (line 279) has a tight ±0.001 W/(m²·K) tolerance on the same computation, the loose range test is redundant and could mask a regression if the precise test were removed.
**Code Location**: `crates/hares-physics/src/constants.rs:262-265`
**Root Cause**: The imprecise test was likely written before the more rigorous correctness test and not tightened afterward.
**Impact**: Low — no functional impact; the precise test at line 279 catches actual deviation. Risk is only if the precise test is accidentally deleted.

### Finding 2: [Severity: low]
**Description**: The Stefan-Boltzmann constant defined at `constants.rs:201-202` uses the NIST CODATA 2018 value `5.670374419e-8`, while both EnergyPlus (`5.6697e-8`) and OCHRE (`5.6704e-8`) use older/rounded values. The relative difference is ~0.012%, producing a ~0.0006 W/(m²·K) shift at 20°C. This is physically negligible (~1.5 orders of magnitude below typical measurement uncertainty in building simulation), but the constant value merits notation because it diverges from the established industry code bases used for validation comparisons.
**Code Location**: `crates/hares-physics/src/constants.rs:202`
**Root Cause**: NIST 2018 vs ASHRAE-era constants. HARES correctly uses the latest NIST reference; vendors use legacy values.
**Impact**: Low — no correctness concern; a documentation note that inter-model comparisons using different σ values differ by ~0.01% may be useful for users benchmarking against EnergyPlus or OCHRE.

## Summary
- Total findings: 2
- Critical / High / Medium / Low: 0 / 0 / 0 / 2

## Verification — Correctness of Formula & Expected Value

### Formula verification
The function computes:

```
h_rad = 4.0 × ε × σ × T³
```

This is the first-order Taylor expansion of the Stefan-Boltzmann net radiation exchange:

```
q̇ = εσ(T_s⁴ − T_amb⁴)
d(q̇)/dT|_T,ΔT→0 = 4εσT³
```

Confirmed algebraically against the equivalent form used throughout EnergyPlus:

```
σ·ε·(T_s⁴ − T_amb⁴)/(T_s − T_amb) = σ·ε·(T_s² + T_amb²)(T_s + T_amb) → 4εσT³  as T_s, T_amb → T
```

### Expected value at 20°C with ε = 0.9

| Parameter | Value |
|-----------|-------|
| ε | 0.90 |
| σ (NIST 2018) | 5.670374419 × 10⁻⁸ W/(m²·K⁴) |
| T (20°C) | 293.15 K |
| T³ | 25,192,408.831 K³ |
| 4·ε·σ | 2.041334791 × 10⁻⁷ W/(m²·K⁴) |
| **h_rad** | **5.142614 W/(m²·K)** ≈ **5.14 W/(m²·K)** ✓ |

Cross-check with vendor σ values yields identical agreement within stated tolerances:

| Source | σ [W/(m²·K⁴)] | h_rad [W/(m²·K)] | Δ |
|--------|-----------------|-------------------|---|
| HARES (NIST 2018) | 5.670374419e-8 | 5.142614 | — |
| EnergyPlus | 5.6697e-8 | 5.142002 | −0.012% |
| OCHRE | 5.6704e-8 | 5.142637 | +0.0004% |

### Derivation / derivation correctness
The derivation from the Stefan-Boltzmann law is standard and correct:

1. Full non-linear exchange: `q̇ = εσA(T₁⁴ − T₂⁴)` (two gray-body surfaces, one at T₁, one at T₂)
2. Linearise about pivot T with small ΔT: `T₁ = T + ΔT/2`, `T₂ = T − ΔT/2`
3. First-order Taylor: `q̇ ≈ εσA·4T³·ΔT = h_rad·A·ΔT`
4. Therefore `h_rad = 4εσT³`, truncation error `O(ΔT²/T²)`

This matches ASHRAE HoF 2021 Ch.4 §4.3 and Incropera et al. §1.2.3 Eq.1.9 as cited in the doc comment. Both EnergyPlus and OCHRE implement the identical linearisation for their respective reduced-order or simplified radiation models.

### Unit correctness
- Input: `emissivity` [−] (dimensionless), `t_kelvin` [K]
- Output: [−] × [W/(m²·K⁴)] × [K³] = [W/(m²·K)] ✓

## Recommendations

1. Tighten or remove the redundant `linearised_h_rad_at_20c_is_physically_reasonable` test (line 258–265); the `linearised_h_rad_correctness` test (line 278–306) already validates the exact value with a ±0.001 W/(m²·K) tolerance. If kept, narrow the range to e.g. `(5.0..=5.3)`.

2. Add a doc comment note in `constants.rs` acknowledging that the STEFAN_BOLTZMANN constant uses NIST CODATA 2018 and differs slightly (~0.01%) from the values used in EnergyPlus and OCHRE, so users performing cross-model validation should account for this.

## References / Citations
- ASHRAE Handbook of Fundamentals 2021, Ch.4 §4.3 "Radiation Heat Transfer"
- Incropera, DeWitt, Bergman & Lavine. *Fundamentals of Heat and Mass Transfer*, 7th ed., §1.2.3 Eq.1.9
- NIST CODATA 2018: Tiesinga et al. *Rev. Mod. Phys.* 93, 025010 (2021). σ = 5.670374419 × 10⁻⁸ W·m⁻²·K⁻⁴
- EnergyPlus Engineering Reference, §3.2 "Outside Surface Heat Balance"
- EN 673:2011 — Glass in building, determination of thermal transmittance (U value)
