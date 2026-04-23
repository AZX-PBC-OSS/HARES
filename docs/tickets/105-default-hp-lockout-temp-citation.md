# `DEFAULT_HP_LOCKOUT_TEMP_C` Lacks Citation

**Severity**: Low
**Priority**: P3
**Status**: Open
**Areas**: hares-equipment/hvac/constants

## Problem

`DEFAULT_HP_LOCKOUT_TEMP_C = -17.78°C` (0°F) at `crates/hares-equipment/src/hvac/constants.rs:34` has no citation. -17.78°C is a plausible default — many residential heat pump compressors are rated to operate down to roughly 0°F (−17.78°C) — but the constant is presented as authoritative with no source. A future contributor cannot verify or update the value without redoing the literature review.

## Current Behavior

`crates/hares-equipment/src/hvac/constants.rs:34`:
```rust
pub const DEFAULT_HP_LOCKOUT_TEMP_C: f64 = -17.78;
```

No comment, no citation. The value is consumed by `resolve_hvac.rs` when HPXML omits `CompressorLockoutTemperature`.

## Required Behavior

1. Add a documentation comment with a primary-source citation. Candidate sources:
   - AHRI Standard 210/240-2023: minimum operating temperature for residential ASHP test conditions
   - HPXML specification: any documented default for missing `CompressorLockoutTemperature`
   - OCHRE source code: cross-check the OCHRE default and cite if consistent
   - Manufacturer specification sheets (Mitsubishi, Daikin, Carrier): typical residential ASHP minimum operating temperatures
2. If the citation does not support -17.78°C, revise the value to whatever the cited source recommends.
3. The comment must include the source, the value's rationale, and a note on when it is consumed.

## Approach

1. Audit AHRI 210/240-2023 for any defined minimum operating temperature.
2. Cross-check OCHRE: `vendors/OCHRE/ochre/Equipment/Heat_Pump.py` (or equivalent) for the default it uses.
3. Cross-check manufacturer specs for at least three major residential ASHP product lines.
4. Update the constant with a documentation comment citing the chosen source. If sources disagree, choose the AHRI standard value as the authoritative default and document the disagreement.
5. Cross-reference this constant from ticket 095 (BackupHeatingSwitchoverTemperature) which uses it as the default for `hp_lockout_temp_c`.

## Definition of Done

- [ ] `DEFAULT_HP_LOCKOUT_TEMP_C` has a documentation comment with primary-source citation
- [ ] Value verified or revised against the cited source
- [ ] OCHRE cross-check documented in the comment
- [ ] Cross-references to ticket 095 (where the constant is consumed)

## Verification

```bash
cargo test -p hares-equipment hvac
cargo test -p hares-io resolve_hvac
```

## References

- AHRI Standard 210/240-2023 *Performance Rating of Unitary Air-conditioning and Air-source Heat Pump Equipment* — heating test conditions H1, H2, H3 and operating range definitions.
- HPXML Specification v4.x §8.4 "Heat Pumps" — `CompressorLockoutTemperature` definition and any default text.
- OCHRE `vendors/OCHRE/ochre/Equipment/Heat_Pump.py` — comparison default.

## Related Tickets

- 095-backup-switchover-temp-er-lockout-only (consumes this default when switchover does not apply)
- 080-ashp-heating-capacity-17f-silently-dropped
