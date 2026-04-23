# Re-derive Per-Surface UA Expectations From ASHRAE/E+ First Principles, Drop Tolerance Widening

**Severity**: High
**Priority**: P1
**Status**: Open
**Areas**: hares-envelope, tests/parity, tests/structural_envelope_oracle

## Problem

Two parity tolerances were widened to mask a divergence between HARES and OCHRE that the review identified as HARES being more correct, not less:

1. `tests/parity/tolerance.rs:8,30`: MAE was widened from 0.1°C → 0.6°C and peak HVAC power from 2% → 80%.
2. `tests/structural_envelope_oracle.rs`: `beopt_ua_parity` compares HARES UA per surface against hardcoded OCHRE constants (e.g. `OCHRE_TOTAL_UA: f64 = 558.74`).

Both treat OCHRE as the truth. The underlying physics: HARES uses a convection-only film coefficient (R_film_conv ~0.447 m²·K/W at ΔT=5°C, computed via TARP) plus an explicit linearised radiation conductance (h_rad branch) in parallel. OCHRE uses a single combined R_film (~0.12 m²·K/W per ISO 6946) that bundles convection and radiation. The HARES decomposition is closer to ASHRAE/EnergyPlus practice; the per-surface UA naturally differs because the radiation path is now explicit rather than baked into a combined film.

Widening the tolerances normalizes OCHRE divergence as if OCHRE were correct. The correct response is to re-derive expected per-surface UA from first principles using the same convection-only + explicit h_rad network HARES actually solves, then validate against BESTEST ASHRAE 140 reference bands rather than OCHRE constants.

## Current Behavior

`tests/parity/tolerance.rs:8`:
```rust
pub const PARITY_MAE_C: f64 = 0.6;  // widened from 0.1
```

`tests/parity/tolerance.rs:30`:
```rust
pub const PARITY_PEAK_HVAC_PCT: f64 = 80.0;  // widened from 2.0
```

`tests/structural_envelope_oracle.rs`: hardcoded `OCHRE_TOTAL_UA: f64 = 558.74` and per-surface OCHRE constants used as the comparison baseline.

`tests/bestest/mod.rs:77,99,120,143,167`: BESTEST tests `#[ignore]`d — see ticket 094.

## Required Behavior

1. Re-derive expected per-surface UA values from first principles:
   - Per-surface UA = 1 / (R_film_exterior_conv + R_layers + R_film_interior_conv) for the conduction path
   - Per-surface radiation path: explicit linearised h_rad conductance to other interior surfaces (StarMesh)
   - The two paths are parallel branches in the assembled network, not a series film resistance
2. Replace hardcoded `OCHRE_TOTAL_UA` and per-surface OCHRE constants in `tests/structural_envelope_oracle.rs` with values derived from ASHRAE HoF 2021 Ch. 26 / EnergyPlus Engineering Reference §3.5 first principles. Cite the derivation inline.
3. Drop the tolerance widening in `tests/parity/tolerance.rs`: restore PARITY_MAE_C to 0.1 (or document the tightest tolerance achievable while passing BESTEST 140 reference bands), restore PARITY_PEAK_HVAC_PCT to a defensible value (no looser than 5%).
4. Adopt BESTEST ASHRAE 140 reference bands as the validation gate. BESTEST defines pass/fail bands for annual heating, annual cooling, and peak loads. These are the primary acceptance criteria; OCHRE parity is a useful cross-check at best.

## Approach

1. Compute reference per-surface UA values using TARP convection coefficients at design conditions (winter ΔT=20°C indoor-outdoor for heating-design, summer ΔT=11°C for cooling-design — ASHRAE HoF 2021 Ch. 14) and explicit linearised h_rad at the design mean surface temperature.
2. Document the derivation in a comment block at the top of `tests/structural_envelope_oracle.rs` with citations.
3. Replace the OCHRE comparison with first-principles comparison; allow ±2% tolerance to absorb residual modelling differences between TARP and the ASHRAE HoF tabulated value.
4. Tighten `tests/parity/tolerance.rs` to the minimum that passes the existing parity suite once the BESTEST gate is in place.
5. Cross-validate by running BESTEST cases 600, 610, 620, 900, and 940 (or whichever are in `tests/bestest/`) and checking annual heating/cooling totals fall within ASHRAE 140 reference bands.

## Definition of Done

- [ ] `tests/structural_envelope_oracle.rs` no longer references `OCHRE_TOTAL_UA` or any other hardcoded OCHRE constant as the baseline
- [ ] Per-surface UA expectations derived from first principles with inline citations
- [ ] `PARITY_MAE_C` returned to 0.1°C (or to a lower value that passes the suite — never 0.6)
- [ ] `PARITY_PEAK_HVAC_PCT` returned to ≤5% (never 80%)
- [ ] BESTEST cases 600/900 (and others currently `#[ignore]`d) passing within ASHRAE 140 reference bands — see ticket 094
- [ ] Comment block in `tests/structural_envelope_oracle.rs` documents the derivation and explicitly states "OCHRE differs because it uses a combined R_film; HARES is correct"

## Verification

```bash
cargo test -p hares-envelope structural_envelope_oracle
cargo test --test parity
cargo test --test bestest
```

Annual heating and cooling for BESTEST 600/900 must fall within the ASHRAE Standard 140 published reference bands.

## References

- ASHRAE Standard 140-2020 *Standard Method of Test for the Evaluation of Building Energy Analysis Computer Programs* — published reference band data for cases 600, 610, 620, 630, 640, 650, 900, 910, 920, 930, 940, 950, 960.
- ASHRAE Handbook of Fundamentals 2021 Ch. 26 "Heat, Air, and Moisture Control in Building Assemblies — Material Properties" — surface conductance values.
- ASHRAE Handbook of Fundamentals 2021 Ch. 14 "Climatic Design Information" — winter/summer design conditions.
- EnergyPlus Engineering Reference §3.5 "Inside Surface Heat Balance" — TARP convection model and explicit longwave network.
- ISO 6946:2017 — combined surface resistance values (the OCHRE baseline being departed from).

## Related Tickets

- 089-radiation-frac-starmesh-rederivation (related StarMesh derivation work)
- 094-bestest-tests-still-ignored (BESTEST is the validation gate)
- feedback_ashrae_not_ochre — project policy: target ASHRAE/E+, never regress to match OCHRE bugs
