# BESTEST Conformance Tests Still `#[ignore]`d After S4/S5 Fixes

**Severity**: High
**Priority**: P1
**Status**: Open
**Areas**: hares-envelope, tests/bestest

## Problem

All five BESTEST tests at `tests/bestest/mod.rs:77,99,120,143,167` carry `#[ignore]` attributes despite the S4 (StarMesh longwave) and S5 (linearised h_rad consolidation) fixes being merged. BESTEST conformance is the primary acceptance gate for the entire RC envelope solver change set — without it, no RC physics fix can be considered validated against an external reference.

Project policy `feedback_no_ignore_tests` explicitly forbids `#[ignore]` on failing tests; the correct response is to debug and fix the physics, not skip the gate. Per `feedback_ashrae_not_ochre`, BESTEST against the ASHRAE Standard 140 reference bands is the right gate, not OCHRE parity.

## Current Behavior

```
tests/bestest/mod.rs:77    #[ignore]  // BESTEST 600
tests/bestest/mod.rs:99    #[ignore]  // BESTEST 610
tests/bestest/mod.rs:120   #[ignore]  // BESTEST 620
tests/bestest/mod.rs:143   #[ignore]  // BESTEST 900
tests/bestest/mod.rs:167   #[ignore]  // BESTEST 940 (or similar)
```

The actual case identifiers and which currently fail must be enumerated in the fix; the line numbers above are the entry points. Each test currently does not run in CI.

## Required Behavior

1. Remove `#[ignore]` from all five tests.
2. Each test must pass within the ASHRAE Standard 140 published reference band for its case (annual heating, annual cooling, peak heating, peak cooling).
3. If a test fails after `#[ignore]` removal, debug the underlying physics — do not re-add `#[ignore]`. Possible debugging avenues:
   - Per-surface UA derivation (ticket 090)
   - StarMesh `radiation_frac` re-derivation (ticket 089)
   - Interior LWR last-step temperature lag (ticket 047)
   - Window SHGC absorbed inward (ticket 049)
   - Ground temperature depth correction (ticket 055)

## Approach

1. Run each BESTEST case standalone to identify the magnitude and direction of any deviation from the ASHRAE 140 reference band.
2. For each failing case, attribute the deviation to a specific physics path using diagnostic CSV output and the post-S1/S4/S5 zone-air gain breakdown (see ticket 092).
3. Address the underlying physics defect — typically by completing one of the related tickets above.
4. Once all five cases pass, remove the `#[ignore]` attributes in a single commit so the BESTEST gate is part of the standard test run.

## Definition of Done

- [ ] All five BESTEST tests at `tests/bestest/mod.rs:77,99,120,143,167` pass without `#[ignore]`
- [ ] Each passing test asserts annual heating, annual cooling, peak heating, and peak cooling fall within the ASHRAE Standard 140 published reference band for its case
- [ ] No `#[ignore]` attribute remains in `tests/bestest/`
- [ ] Test data references the published BESTEST reference band values (not OCHRE-derived constants)
- [ ] CI run includes BESTEST as a gating check for the RC envelope solver

## Verification

```bash
cargo test --test bestest -- --include-ignored   # before fix: confirms current state
cargo test --test bestest                        # after fix: passes without override
```

Each annual heating/cooling total and peak load must fall within the ASHRAE 140 published reference band.

## References

- ASHRAE Standard 140-2020 *Standard Method of Test for the Evaluation of Building Energy Analysis Computer Programs* — definitive reference bands for cases 600, 610, 620, 630, 640, 650, 900, 910, 920, 930, 940, 950, 960.
- Judkoff & Neymark *International Energy Agency Building Energy Simulation Test (BESTEST) and Diagnostic Method* (NREL/TP-472-6231, 1995) — original BESTEST methodology.
- Project policy `feedback_no_ignore_tests.md` — never `#[ignore]` failing tests; debug and fix the physics.
- Project policy `feedback_ashrae_not_ochre.md` — target ASHRAE/E+, not OCHRE.

## Related Tickets

- 089-radiation-frac-starmesh-rederivation
- 090-rederive-per-surface-ua-from-first-principles
- 091-port-radiant-inputs-all-zones
- 092-zone-sensible-breakdown-debug-includes-radiant
- 047-interior-lwr-uses-last-step-zone-temp
- 049-window-solar-shgc-vs-transmittance-absorbed-inward
- 055-ground-temp-no-depth-correction
