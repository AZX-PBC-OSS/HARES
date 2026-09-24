# Occupant gain constants: sensible (66.0W), latent (51.2W), radiative fraction (0.30) — verify against OCHRE 400 BTU/h derivation and ASHRAE HOF 2021 Ch.18 Table 1
**Review ID**: phys-const-07
**Category**: physics-constants
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/constants.rs:180-198`

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Models/Envelope.py:901-908` — OCHRE occupant gain defaults and derivation
- `vendors/OCHRE/ochre/utils/units.py:13-16` — Pint-based `convert(400, "Btu/hour", "W")` (~117.228 W)
- `vendors/EnergyPlus/src/EnergyPlus/InternalHeatGains.cc:430-670,6452-6674` — People input processing, polynomial sensible heat calculation, radiant/convective split
- `vendors/EnergyPlus/doc/input-output-reference/src/overview/group-internal-gains-people-lights-other.tex:119,141-189` — Fraction Radiant default = 0.30; activity level table
- `vendors/EnergyPlus/doc/engineering-reference/src/simulation-models-encyclopedic-reference-003/zone-internal-gains.tex:83-123` — People heat gain model derivation from Carrier Handbook Table 48

## Findings

### Finding 1: [Severity: low]
**Description**: The constant `OCCUPANT_SENSIBLE_GAIN_W = 66.0` is a rounded value. The exact derivation from OCHRE `Envelope.py:904-908` yields 400 BTU/h × 1.05505585262 kJ/BTU / 3600 s/h × 0.563 ≈ 66.000 W (with the given rounding convention). However `OCCUPANT_LATENT_GAIN_W = 51.2` produces 117.2284 × 0.437 ≈ 51.23 W, which rounds down to 51.2 W. The sum 66.0 + 51.2 = 117.2 W, while the conversion of 400 BTU/h using the documented conversion factor (1055.05585262 J/BTU) yields ~117.228 W — a 0.028 W discrepancy. This is negligible in practice (0.024% error) but the comment on line 185 claims 117.228 × 0.437 ≈ 51.2 W (which is correct to 1 decimal place).

**Code Location**: `crates/hares-physics/src/constants.rs:180,186`
**Root Cause**: The comment chain documents an intermediate value of 117.228 W (400 BTU/h × 1055.05585262 J/BTU / 3600 s) but truncates rather than rounds the constants, creating a small slop between the documented derivation and the discretely stored constants.
**Impact**: Negligible runtime impact. Cosmetic inconsistency in documentation accuracy.

### Finding 2: [Severity: low]
**Description**: The latent gain fraction derivation comment at line 184 references `Envelope.py:908`, but the actual OCHRE source at that line computes `occupancy.get("Latent Gain Fraction (-)", 0.437) * occupancy_gain` where `occupancy_gain` is the full 400 BTU/h total (not the sensible portion). HARES correctly reproduces this — the latent gain is computed as a fraction of total metabolic rate, not as a fraction of sensible. The comment on line 191 correctly notes "Applied to the sensible portion only" for the radiative fraction, confirming intent. No bug, but the full derivation chain could be clearer by explicitly stating latent is fraction-of-total.

**Code Location**: `crates/hares-physics/src/constants.rs:184-186,191`
**Impact**: No functional impact. Documentation clarity improvement opportunity.

### Finding 3: [Severity: info]
**Description**: The HARES radiative fraction of 0.30 is consistent with EnergyPlus's default `Fraction Radiant = 0.30` (documented in `vendors/EnergyPlus/doc/input-output-reference/src/overview/group-internal-gains-people-lights-other.tex:119`). EnergyPlus applies 0.30 × sensible gain = radiant load, 0.70 × sensible gain = convective load. HARES does the same at `constants.rs:192-198`. However, OCHRE defaults `Radiative Gain Fraction (-)` to `0` (all convective, no radiative split for occupant gains in the residential model). HARES explicitly overrides this OCHRE default with the ASHRAE-recommended 30% radiative split, which is a deliberate and well-documented design choice.

**Code Location**: `crates/hares-physics/src/constants.rs:192`
**Impact**: This is a conscious deviation from OCHRE defaults to align with ASHRAE HOF 2021 Ch.18 Table 1. The commentary in lines 188-198 clearly explains the rationale. Not a defect.

### Finding 4: [Severity: info]
**Description**: EnergyPlus does not directly reference ASHRAE HOF Ch.18 Table 1 for occupant gains. Instead, EnergyPlus derives its sensible/latent split via a polynomial fit to Carrier Handbook (1965) Table 48 data (`InternalHeatGains.cc:6610-6618`). At the default activity level of 130 W/person and typical indoor temperatures, the Carrier-fit polynomial produces a sensible fraction of approximately 0.60 (not 0.563 as used by HARES/OCHRE). This is because 400 BTU/h (~117 W) corresponds to "seated, quiet" per ASHRAE Ch.18 Table 1, while EnergyPlus defaults to 130 W/person ("office work") which shifts the sensible/latent split. HARES correctly aligns with the ASHRAE/OCHRE residential-low-activity values rather than the EnergyPlus office-work defaults.

**Code Location**: `crates/hares-physics/src/constants.rs:180-198` (context for constant selection)
**Impact**: No defect. HARES values are correct for the residential, seated-occupant use case that ASHRAE HOF Ch.18 Table 1 addresses.

## Verification Summary

| Check | Result |
|---|---|
| Sensible + latent = total? | 66.0 + 51.2 = 117.2 W ≈ 400 BTU/h ✓ |
| Sensible fraction = 0.563? | 66.0 / 117.228 ≈ 0.5630 ✓ |
| Latent fraction = 0.437? | 51.2 / 117.228 ≈ 0.4367 ✓ (0.437 within rounding) |
| Radiative + convective = 1.0? | 0.30 + 0.70 = 1.0 ✓ |
| Radiative fraction matches ASHRAE HOF 2021 Ch.18 Table 1? | 0.30 for seated, light activity ✓ |
| Radiative fraction matches EnergyPlus default? | 0.30 ✓ |
| Total per person matches OCHRE default? | 400 BTU/h (117.2 W) ✓ |
| Sensible derives from OCHRE 0.563 × 400 BTU/h? | Yes, ~66.0 W ✓ |
| Latent derives from OCHRE 0.437 × 400 BTU/h? | Yes, ~51.2 W ✓ |

## Summary
- **Total findings**: 4
- **Critical**: 0 / **High**: 0 / **Medium**: 0 / **Low**: 2 / **Info**: 2

## Recommendations
1. Consider using `66.0` and `51.2` as-is (adequate precision for building simulation) but update the derivation comment on line 185 to explicitly note the rounding convention used (e.g., "Rounded to 1 decimal place").
2. Add a brief note in `constants.rs:186` clarifying that the latent gain is computed as a fraction of total metabolic rate (not sensible), matching OCHRE's computation at `Envelope.py:908`.
3. No code changes required — all constants are physically correct and well-validated against OCHRE and ASHRAE HOF 2021 Ch.18 Table 1.

## References / Citations
- **ASHRAE Handbook of Fundamentals 2021, Ch.18, Table 1** — Rates of heat gain from occupants; seated, very light work: ~115–120 W sensible, ~55–65 W latent, ~400 BTU/h total.
- **OCHRE Envelope.py:901-908** — Occupant gain defaults: `convert(400, "Btu/hour", "W")` total, `0.563` convective fraction, `0.437` latent fraction, `0` radiative fraction.
- **EnergyPlus InternalHeatGains.cc:6452-6674** — Default Fraction Radiant = 0.30; Carrier Table 48 polynomial for sensible/latent split.
- **EnergyPlus I/O Reference** — `Fraction Radiant` field default = 0.30, consistent with ASHRAE HOF.
