# `BackupHeatingSwitchoverTemperature` Must Map to ER Lockout Only

**Severity**: High
**Priority**: P1
**Status**: Open
**Areas**: hares-io/hpxml/resolve_hvac

## Problem

`crates/hares-io/src/hpxml/resolve_hvac.rs:1497-1521` falls back from missing element to using HPXML `BackupHeatingSwitchoverTemperature` to populate BOTH `hp_lockout_temp_c` (compressor lockout) AND `er_lockout_temp_c` (electric resistance backup lockout). Per HPXML 4.x §8.4 (HeatPump element definitions), `BackupHeatingSwitchoverTemperature` defines the temperature *at which backup heating takes over* — i.e. the ER turns on. It does not define the compressor lockout. OCHRE maps this field only to the backup activation; HARES double-mapping causes:

- The compressor is locked out at the same temperature backup turns on, leaving a thermal capacity gap (no compressor + no backup) at marginal temperatures
- The compressor cannot run alongside backup at low OAT, contradicting the HPXML semantic that backup *adds to* compressor capacity until the compressor's own minimum operating temperature is reached

## Current Behavior

`crates/hares-io/src/hpxml/resolve_hvac.rs:1497-1521`:
```rust
let switchover_c = extract_switchover_temp(...);
hp_lockout_temp_c = switchover_c.or(hp_lockout_temp_c);  // wrong
er_lockout_temp_c = switchover_c.or(er_lockout_temp_c);  // correct (lower bound)
```

Both lockouts inherit the same value. With OAT below the switchover temperature, the simulation reports no heating capacity at all from the heat pump; backup ER must carry the entire load below the switchover even when the compressor would still operate down to its own `MinimumOperatingTemperature`.

## Required Behavior

1. `BackupHeatingSwitchoverTemperature` maps to `er_lockout_temp_c` only (it sets the lower bound for ER activation in some interpretations and the upper bound for ER suppression in others — clarify against HPXML 4.x text and OCHRE).
2. `hp_lockout_temp_c` must come from a different HPXML field — `CompressorLockoutTemperature` (HPXML extension) or the heat pump's `MinimumOperatingTemperature` if exposed — never from the backup switchover field.
3. If neither `hp_lockout_temp_c` source field is present, fall back to the typed config default (`DEFAULT_HP_LOCKOUT_TEMP_C` per ticket TODO; see also ticket 105 G2 for citation).
4. Document the semantic explicitly inline: switchover = backup activation; HP lockout = compressor minimum operating temperature.

## Approach

1. Read the relevant HPXML 4.x specification text for `BackupHeatingSwitchoverTemperature`, `CompressorLockoutTemperature`, and `MinimumOperatingTemperature`. Confirm each field's intended semantic.
2. Cross-check OCHRE `vendors/OCHRE/ochre/utils/hpxml.py` for the field-to-config mapping.
3. Modify `resolve_hvac.rs:1497-1521` to map switchover to `er_lockout_temp_c` only.
4. Add a separate code path that reads `CompressorLockoutTemperature` (or appropriate source) for `hp_lockout_temp_c`.
5. Add fixtures: HPXML with switchover only (HP should still operate down to default lockout), HPXML with separate compressor lockout (both fields populated correctly), HPXML with neither (defaults applied).
6. Add a test asserting: at OAT just below switchover, HP capacity is non-zero (compressor still operating); ER is now active; total capacity = HP + ER.

## Definition of Done

- [ ] `BackupHeatingSwitchoverTemperature` maps to `er_lockout_temp_c` exclusively
- [ ] `hp_lockout_temp_c` populated from `CompressorLockoutTemperature` (or equivalent), never from switchover
- [ ] Fixtures cover switchover-only, separate compressor-lockout, and neither
- [ ] Test: HP capacity > 0 at OAT just below switchover, with ER also active
- [ ] OCHRE parity restored for HPXML files exercising this field
- [ ] Inline documentation explains the semantic difference

## Verification

```bash
cargo test -p hares-io resolve_hvac backup_switchover
cargo test -p hares-io hpxml_parity
```

## References

- HPXML Specification v4.2 §8.4 "Heat Pumps" — `BackupHeatingSwitchoverTemperature` defined as the temperature below which backup begins to operate; `CompressorLockoutTemperature` (extension) defines the minimum compressor operating temperature.
- OCHRE `vendors/OCHRE/ochre/utils/hpxml.py` — backup switchover maps to backup-activation field only.
- ANSI/RESNET/ICC 301-2022 *Standard for the Calculation and Labeling of the Energy Performance of Dwelling and Sleeping Units using an Energy Rating Index* — references HPXML field semantics.
- AHRI Standard 210/240-2023 — heat pump capacity ratings extend below switchover; compressor lockout is independent.

## Related Tickets

- 015-er-on-off-modeling (binary ER on/off behaviour interacts with switchover)
- 023-hpxml-hvac-wiring-gaps (G6: SupplementalHeatingLockoutTemperature wiring)
- 080-ashp-heating-capacity-17f-silently-dropped (related HP cold-OAT capacity handling)
