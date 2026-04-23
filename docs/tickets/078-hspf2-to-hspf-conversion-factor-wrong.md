# HSPF2-to-HSPF Conversion Factor Incorrect (0.95 Used Instead of ~0.85)

**Severity**: High
**Priority**: P1
**Status**: Open
**Areas**: hares-io

## Problem

HARES uses a HSPF2→HSPF conversion factor of `1/0.95` (line 28 of `resolve_hvac.rs`),
matching the SEER2→SEER conversion. The SEER2 factor (≈ 0.95) is correct per DOE
rulemaking for cooling. However, the HSPF2 factor is substantially different: DOE's
final rule (December 2022, 87 FR 74364) established that HSPF2 test procedures
produce ratings approximately 15% lower than HSPF, not 5%. The correct approximation
is HSPF ≈ HSPF2 / 0.85, i.e., `HSPF2_TO_HSPF_FACTOR ≈ 1/0.85 ≈ 1.176`.

Using the wrong factor understates EIR (overstates COP) for HSPF2-rated equipment.
For a heat pump rated HSPF2 = 9.0:
- Current code: HSPF = 9.0 / 0.95 = 9.47 → EIR = 3.412/9.47 = 0.360
- Correct: HSPF = 9.0 / 0.85 = 10.59 → EIR = 3.412/10.59 = 0.322
- Error: COP is overstated by ~10.5% (equipment appears 10% more efficient than it is)

## Evidence

`resolve_hvac.rs:27–28`:

```
const SEER2_TO_SEER_FACTOR: f64 = 1.0 / 0.95;
const HSPF2_TO_HSPF_FACTOR: f64 = 1.0 / 0.95;
```

`resolve_hvac.rs:1771`:

```
"HSPF2" => ("HSPF".to_string(), value * HSPF2_TO_HSPF_FACTOR),
```

## OCHRE Cross-check

OCHRE `hpxml.py` does not perform HSPF2→HSPF conversion; it receives
pre-normalised HSPF from ResStock. OCHRE is not a reference here — the
DOE rulemaking document is.

## Required Behavior

Correct the constant to reflect DOE's actual conversion:

```
const HSPF2_TO_HSPF_FACTOR: f64 = 1.0 / 0.85;
```

The exact factor varies by equipment class (split vs. packaged, single-speed vs.
multi-speed). Using 1/0.85 as the single conversion factor represents the central
estimate from DOE's analysis across residential heat pump classes. If the HPXML
file carries an explicit `<CompressorType>` the equipment-class-specific factor
could be applied; at minimum the constant must be corrected from 0.95 to 0.85.

## Approach

Change location: `resolve_hvac.rs:28`. Replace:

```
const HSPF2_TO_HSPF_FACTOR: f64 = 1.0 / 0.95;
```

with:

```
const HSPF2_TO_HSPF_FACTOR: f64 = 1.0 / 0.85;
```

The `normalize_efficiency_units` call at `resolve_hvac.rs:1771` requires no further change; it already applies this constant to the raw HSPF2 value.

## Citation

- DOE Final Rule, Energy Conservation Standards for Residential Central Air Conditioners
  and Heat Pumps, 87 FR 74364 (December 23, 2022): establishes HSPF2 under the revised
  AHRI 210/240-2023 test procedure; Table IV.4 shows the HSPF2 / HSPF ratio ≈ 0.85
  for split-system heat pumps.
- AHRI Standard 210/240-2023 §11.12: HSPF2 calculation with revised cyclic degradation
  and updated H1/H2/H3 test points produces approximately 15% lower ratings than HSPF
  under AHRI 210/240-2008.
- ResStock v2023.1 documentation: heat pump HSPF2 values in TSV inputs are rated under
  AHRI 210/240-2023; the 1/0.85 factor is required to recover HSPF for legacy model inputs.

## Annual kWh Impact Rank

**High.** Heating energy is the dominant end use in cold climates. Overstating
heat pump COP by ~10% causes the simulation to underpredict heating kWh by the
same proportion — a systematic bias affecting every HSPF2-rated ASHP simulation.

## Definition of Done

- [ ] `HSPF2_TO_HSPF_FACTOR` corrected to `1.0 / 0.85` at `resolve_hvac.rs:28`
- [ ] Test: HSPF2 = 9.0 → normalized HSPF = 9.0 / 0.85 ≈ 10.588 (within 0.1%)
- [ ] Test: HSPF2 = 9.0 → resulting EIR = 3.412 / 10.588 ≈ 0.3222 (within 0.1%); verify this does NOT equal the old value (3.412 / (9.0/0.95) ≈ 0.3602)
- [ ] Existing `normalize_efficiency_units` test updated to use the corrected factor

## Verification

```bash
cargo test -p hares-io -- normalize_efficiency_units
cargo test -p hares-io -- resolve_hvac::tests::hspf2
```
