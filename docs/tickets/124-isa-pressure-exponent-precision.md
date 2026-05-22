# ISA Pressure Exponent Constant Precision Mismatch

**Severity**: Nit
**Priority**: P4
**Status**: Open
**Areas**: hares-physics/constants, hares-io/resstock_csv

## Problem

`ISA_PRESSURE_EXPONENT = 5.2559` is declared at `crates/hares-physics/src/constants.rs:83`, but `crates/hares-io/src/resstock_csv.rs` uses an inline literal `5.25588` instead of the constant. The two values differ in the fifth decimal place (5.2559 vs 5.25588), so a future contributor reading either file cannot tell which is the "correct" value or whether the difference is intentional.

The U.S. Standard Atmosphere 1976 derives the exponent as `g·M / (R*·L)` where `g = 9.80665 m/s²`, `M = 0.0289644 kg/mol`, `R* = 8.31446 J/(mol·K)`, and `L = 0.0065 K/m` for the troposphere. The exact value to higher precision is `5.255876329...`, so `5.25588` (resstock_csv) is the correctly-rounded 6-significant-figure value, while `5.2559` (constants.rs) is an over-rounded 5-significant-figure value.

## Current Behavior

`crates/hares-physics/src/constants.rs:83`:
```rust
pub const ISA_PRESSURE_EXPONENT: f64 = 5.2559;
```

`crates/hares-io/src/resstock_csv.rs` (inline):
```rust
let exponent = 5.25588;
```

Two different values for the same physical constant in two different files; the io file does not import the physics constant.

## Required Behavior

1. `ISA_PRESSURE_EXPONENT` in `hares-physics/constants.rs` must be updated to the higher-precision value `5.25587611` (8 significant figures, matching the derivation precision of the input constants).
2. `crates/hares-io/src/resstock_csv.rs` must remove the inline literal and import `ISA_PRESSURE_EXPONENT` from `hares-physics`.
3. Any other file with an inline `5.2559x` literal for this constant must also be migrated.

## Approach

1. Compute the exponent from primary constants: `5.25587611...` (truncated to 8 sig figs).
2. Update `ISA_PRESSURE_EXPONENT` in `crates/hares-physics/src/constants.rs:83` with a comment citing the U.S. Standard Atmosphere 1976 derivation.
3. `grep` the workspace for any inline `5.2558` or `5.2559` literals; replace each with the imported constant.
4. Add an opt-in `cargo test` that asserts the constant equals the derivation from the underlying primary constants to within `f64` epsilon.

## Definition of Done

- [ ] `ISA_PRESSURE_EXPONENT` updated to `5.25587611` with derivation comment
- [ ] `crates/hares-io/src/resstock_csv.rs` imports the constant; no inline literal
- [ ] Workspace `grep` for `5.2558` and `5.2559` finds only the canonical declaration
- [ ] Unit test verifies the constant matches the derivation from `g`, `M`, `R*`, `L`

## Verification

```bash
cargo test -p hares-physics constants
cargo test -p hares-io resstock_csv
rg '5\.2559|5\.2558' crates/
```

## References

- U.S. Standard Atmosphere 1976 (NOAA-S/T 76-1562), Part 1 §1.2.5 "Pressure" — derivation of the pressure exponent from gravitational acceleration, mean molar mass of dry air, universal gas constant, and tropospheric temperature lapse rate.
- NIST CODATA 2018 — values of universal gas constant `R* = 8.314462618 J/(mol·K)`, standard gravity `g = 9.80665 m/s²`.
- ICAO *Manual of the ICAO Standard Atmosphere* (Doc 7488) — independent derivation matching USSA 1976.

## Related Tickets

- 029-resstock-csv-constant-pressure (related resstock pressure handling)

---

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match (confirmed: `ISA_PRESSURE_EXPONENT = 5.2559` at `crates/hares-physics/src/constants.rs:83`; inline `5.25588` at `crates/hares-io/src/resstock_csv.rs:46`)
- [x] Described logic matches current implementation — `standard_pressure_pa()` in `air_properties.rs:12` uses `ISA_PRESSURE_EXPONENT`; `isa_pressure_kpa()` in `resstock_csv.rs:41–47` uses the inline `5.25588` literal with no import from `hares-physics`
- [x] OCHRE cross-check result: **diverges** — OCHRE's bundled `psychrolib.py` (found in `.venv/lib/python3.14/site-packages/psychrolib.py`) uses `5.2559` (same as the under-precise HARES constant), not the more-precise `5.25588`; the vendor OCHRE Python sources under `vendors/OCHRE/ochre/` contain no atmospheric pressure exponent at all
- [x] EnergyPlus cross-check result: **matches constants.rs** — EnergyPlus uses `5.2559` in `StdPressure = 101325·(1.0−Z·2.25577⁻⁵)^5.2559` (confirmed via the Input Output Reference at `bigladdersoftware.com/epx/docs/9-5/input-output-reference/standard-energyplus-conditions.html`, which states: *"Stdpressure=101325⋅(1.0−Z⋅2.25577⁻⁵)^5.2559 … referencing the ASHRAE 1997 Handbook of Fundamentals (SI edition)"*). So EnergyPlus and ASHRAE both canonically use `5.2559`, while `resstock_csv.rs` uses the more-precise `5.25588`. HARES diverges between its two files.

### Web-Verified Citations

**Citation 1**: U.S. Standard Atmosphere 1976 (NOAA-S/T 76-1562), Part 1 §1.2.5 — derivation of the pressure exponent from g, M, R*, L.

- **Source found**: Wikipedia "Barometric formula" (https://en.wikipedia.org/wiki/Barometric_formula) + USSA 1976 documentation (https://ussa1976.readthedocs.io/en/latest/reference.html)
- **Quoted passage**: Wikipedia "Barometric formula", model equations table for layer 0 (troposphere): *"g₀' = 9.80665 m/s²; M₀ = 28.9644 kg/kmol; R* = 8.31432×10³ J/(kmol·K); L_{M,b} = −6.5 K/km. The exponent g₀M/(R*L) for layer 0 evaluates to **−5.25588**."* The USSA1976 readthedocs reference confirms: `R = 8.31432 J·K⁻¹·mole⁻¹`, `G0 = 9.80665 m/s²`, `M0 = 0.028964425… kg/mole`, troposphere lapse rate `LK = -0.0065 K/m`.
- **Verdict**: **Confirmed**. The primary derivation is correct and the ticket's description of the formula is accurate.
- **Auditor note**: Independent computation: `9.80665 × 0.0289644 / (8.31432 × 0.0065) = 5.2558761133…`, confirming `5.25588` (6 sig figs) is the correctly-rounded value and `5.2559` (5 sig figs) is over-rounded.

**Citation 2**: NIST CODATA 2018 — `R* = 8.314462618 J/(mol·K)`, `g = 9.80665 m/s²`.

- **Source found**: NIST CODATA 2018 recommended values (https://physics.nist.gov/cgi-bin/cuu/Value?r); PMC full text (https://pmc.ncbi.nlm.nih.gov/articles/PMC9888147/)
- **Quoted passage**: NIST CODATA page: *"8.314 462 618… J mol⁻¹ K⁻¹"* (listed as exact under the 2019 SI redefinition). Standard gravity `g = 9.80665 m/s²` is a defined constant (not CODATA-measured).
- **Verdict**: **Confirmed** for the CODATA 2018 value of R. However, **important nuance**: the USSA 1976 deliberately uses `R* = 8.31432` (not CODATA 2018's `8.314462618`), as explicitly acknowledged in the USSA 1976 document (*"R* is slightly greater than 99.998% of the actual value"*). The ticket cites CODATA 2018 as the source for the derivation, but the derivation that produces `5.25587611…` uses the USSA 1976 value of R (8.31432), not CODATA 2018 (8.314462618). Using CODATA 2018 gives a slightly different exponent: `9.80665 × 0.0289644 / (8.314462618 × 0.0065) = 5.2557859592…`. The ticket conflates the two sources.
- **Verdict**: **Partially correct** — the cited CODATA R value is accurate but is not the one used in the USSA 1976 derivation formula. The ticket uses CODATA to motivate the derivation but applies USSA 1976 constants.

**Citation 3**: ICAO *Manual of the ICAO Standard Atmosphere* (Doc 7488) — independent derivation matching USSA 1976.

- **Source found**: ICAO Store listing for Doc 7488 (https://store.icao.int/en/manual-of-the-icao-standard-atmosphere-extended-to-80-kilometres-262500-feet-doc-7488); metanorma atmospheric GitHub (https://github.com/metanorma/atmospheric); Observable ICAO implementation (https://observablehq.com/@mattmyne/icao-standard-atmosphere)
- **Quoted passage**: The ICAO Standard Atmosphere uses the same physical model as the USSA 1976. The full text of Doc 7488 is paywalled; however, the ASHRAE Handbook of Fundamentals (cited by EnergyPlus as its source for the `5.2559` formula) traces back to the same ISA foundation. The 2009 ASHRAE Handbook Fundamentals formula: *"p = 101.325(1 − 2.25577 × 10⁻⁵Z)⁵·²⁵⁵⁹, where Z is altitude in meters"* (sourced via studylib.net summary).
- **Verdict**: **Cannot fully verify** (Doc 7488 is paywalled). The claim that ICAO Doc 7488 independently derives the same exponent is plausible and supported by indirect evidence; the claim is not disputed.

### Legitimacy

- **Verdict**: **Partially Legitimate**

- **Rationale**: The core DRY violation is real and confirmed: `ISA_PRESSURE_EXPONENT = 5.2559` in `constants.rs:83` and the inline `5.25588` in `resstock_csv.rs:46` are two different values for the same physical constant, with no import relationship between them. The derivation claim is correct — `5.25588` is the better-rounded 6-significant-figure value and `5.2559` is over-rounded by one unit in the last place. The regression test (below) fails as expected. However, the ticket contains two inaccuracies that require correction before implementation: (1) it proposes `5.25587611` as *"8 significant figures"* but this number has **9 significant figures** — the correctly-rounded 8-significant-figure value is `5.2558761`; (2) the ticket cites NIST CODATA 2018 as the authoritative source for the derivation, but the USSA 1976 formula uses R* = 8.31432 (a slightly lower value that USSA 1976 itself acknowledges differs from the CODATA measurement) — using CODATA R gives a slightly different exponent (5.2557860). The correct fix is to update to 8 significant figures using USSA 1976 constants: `5.2558761`. Additionally, both EnergyPlus and OCHRE's bundled psychrolib use `5.2559`, so the divergence between HARES and its references is in `resstock_csv.rs`, not in `constants.rs`.

### Proposed Fix Summary

1. Update `ISA_PRESSURE_EXPONENT` in `crates/hares-physics/src/constants.rs:83` to `5.255_876_1` (7 significant figures — one digit less than the ticket proposes, to stay within what USSA 1976 constants support; alternatively `5.255_876_11` for 9 sig figs as the ticket specifies, noting the sig-fig count claim is off by one).
2. Update the comment on line 81 to correctly state derivation: `g/(R_da * L) = 9.80665 / (287.053 * 0.0065)` — the current comment uses `287.058` (CODATA-derived R_da) which is inconsistent with the stored `5.2559` value (USSA 1976-derived). The derivation should be self-consistent.
3. Remove the inline `5.25588` literal in `resstock_csv.rs:46` and import `ISA_PRESSURE_EXPONENT` from `hares-physics`.
4. Add `hares-physics` as a dependency of `hares-io` if not already present (check `crates/hares-io/Cargo.toml`).
5. Do NOT change behavior of `resstock_csv.rs` — both the inline and the updated constant will agree at the precision level that matters (difference at 1000 m is ~0.04 Pa, well within numerical noise).

### Test Written

- **File**: `crates/hares-physics/tests/physics_validation_tests.rs` (appended at end of file)
- **Test name**: `ticket_124_isa_pressure_exponent_matches_ussa76_derivation`
- **What it tests**: Asserts that `ISA_PRESSURE_EXPONENT` agrees with the USSA 1976 derivation (`g₀·M₀/(R*·L) = 5.2558761133…`) to within `1e-5`. The current constant `5.2559` deviates by `2.39e-5`, so the test **fails** with the current code, confirming the bug. Once the constant is updated to ≥ 6 significant figures (`5.25588` or better), the test will pass.
- **Confirmed failing**: `cargo test -p hares-physics ticket_124` exits with code 101, message: *"ISA_PRESSURE_EXPONENT (5.2559) deviates from USSA 1976 derivation (5.2558761133) by 2.39e-5, which exceeds 1e-5."*
