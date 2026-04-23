# Berdahl-Martin Sky Emissivity Coefficient Citation Wrong

**Severity**: Nit
**Priority**: P4
**Status**: Open
**Areas**: hares-io/epw

## Problem

`crates/hares-io/src/epw.rs:529-531` uses Berdahl-Martin clear-sky emissivity coefficients `0.758`, `0.521`, and `0.625` and cites them to "Martin & Berdahl (1984)". This citation is incorrect: the original 1984 paper by Berdahl & Martin gives coefficients `0.711`, `0.56`, and `0.73`. The coefficients actually used (`0.758`, `0.521`, `0.625`) are the recalibrated values used by EnergyPlus 9.6 and reported by Li et al. (2017, Solar Energy). The wrong citation misleads any future contributor who tries to verify the numbers against the original paper.

## Current Behavior

`crates/hares-io/src/epw.rs:529-531`:
```rust
// Berdahl-Martin clear-sky emissivity (Martin & Berdahl 1984)
let eps_clear = 0.758 + 0.521 * t_dp_c / 100.0 + 0.625 * (t_dp_c / 100.0).powi(2);
```

Comment cites the wrong source for the coefficients.

## Required Behavior

Update the citation to reflect the actual provenance of the coefficients:

1. Cite EnergyPlus Engineering Reference §3.7.4 "Sky Emissivity" or §3.5.6 (depending on E+ version) for the recalibrated form actually used.
2. Cite Li, Coimbra & Walsh (2017) "On the determination of atmospheric longwave irradiance under all-sky conditions" *Solar Energy* 144:40-48 for the recalibrated coefficient set.
3. Optionally retain a secondary reference to the original Berdahl & Martin (1984) paper, but make clear the coefficient values are the recalibrated set, not the original.

## Approach

1. Update the inline comment at `crates/hares-io/src/epw.rs:529-531` to:
```rust
// Berdahl-Martin form, recalibrated coefficients per Li et al. (2017)
// and EnergyPlus 9.6+ (see EnergyPlus Engineering Reference §3.5.6).
// Original Berdahl & Martin (1984) gave 0.711 / 0.56 / 0.73 — superseded.
let eps_clear = 0.758 + 0.521 * t_dp_c / 100.0 + 0.625 * (t_dp_c / 100.0).powi(2);
```
2. Verify no other file in the workspace cites the same coefficients with the wrong attribution.
3. Cross-check the EnergyPlus 9.6 source to confirm the coefficient set matches before committing the new citation.

## Definition of Done

- [ ] Citation comment at `crates/hares-io/src/epw.rs:529-531` updated to cite Li et al. (2017) and EnergyPlus Engineering Reference
- [ ] Comment notes the original Berdahl & Martin (1984) values for historical clarity
- [ ] No other workspace file cites these coefficients with the wrong attribution

## Verification

```bash
cargo test -p hares-io epw
rg '0\.758.*0\.521.*0\.625' crates/
```

## References

- Li, M., Coimbra, C.F.M. & Walsh, P. (2017). "On the determination of atmospheric longwave irradiance under all-sky conditions." *Solar Energy* 144:40-48. DOI: 10.1016/j.solener.2017.01.006 — recalibrated coefficient set actually used.
- EnergyPlus Engineering Reference (DOE), §3.5.6 "Sky Emissivity Calculations" — documents the recalibrated form used by EnergyPlus 9.6+.
- Berdahl, P. & Martin, M. (1984). "Emissivity of clear skies." *Solar Energy* 32(5):663-664 — original 1984 paper with `0.711 / 0.56 / 0.73` coefficients (superseded).

## Related Tickets

- 025-psm3-sky-temp-clark-allen-only (related sky temperature model)
