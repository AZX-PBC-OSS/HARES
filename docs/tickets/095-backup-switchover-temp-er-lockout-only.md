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

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match — `crates/hares-io/src/hpxml/resolve_hvac.rs:1497-1522` is the exact block described
- [x] Described logic matches current implementation — the `for` loop at line 1497 iterates two `(xml_keys, param_key)` pairs; the first tries `CompressorLockoutTemperature` then `BackupHeatingSwitchoverTemperature` → writes `hp_lockout_temp_c`; the second tries `BackupHeatingLockoutTemperature` then `BackupHeatingSwitchoverTemperature` → writes `er_lockout_temp_c`
- [x] OCHRE cross-check result: **HARES matches OCHRE exactly** — `vendors/OCHRE/ochre/utils/hpxml.py:947-956`:
  ```python
  hp_lockout_temp = heat_pump.get(
      "CompressorLockoutTemperature",
      heat_pump.get("BackupHeatingSwitchoverTemperature", 0),  # fallback + 0°F default
  )
  er_lockout_temp = heat_pump.get(
      "BackupHeatingLockoutTemperature",
      heat_pump.get("BackupHeatingSwitchoverTemperature", 40),  # fallback + 40°F default
  )
  ```
  OCHRE **also maps `BackupHeatingSwitchoverTemperature` to both hp_lockout and er_lockout**. The ticket's claim "OCHRE maps this field only to the backup activation" is factually incorrect.
- [x] EnergyPlus cross-check result: N/A — the field mapping is HPXML-layer parsing, not an EnergyPlus engineering algorithm. The HPXML specification itself is the authoritative source.

### Web-Verified Citations

**Citation 1**: "HPXML Specification v4.2 §8.4 'Heat Pumps' — `BackupHeatingSwitchoverTemperature` defined as the temperature below which backup begins to operate"

- **Source found**: GitHub PR #309 hpxmlwg/hpxml — "Add HeatPump lockout temperature elements" (https://github.com/hpxmlwg/hpxml/pull/309); confirmed by OpenStudio-HPXML workflow docs (https://openstudio-hpxml.readthedocs.io/en/latest/workflow_inputs.html)
- **Quoted passage**: `BackupHeatingSwitchoverTemperature` = **"[deg F] Temperature at which the backup heating is activated and the compressor is disabled in, e.g., a dual-fuel heat pump."** (HPXML PR #309, confirmed by multiple sources)
- **Verdict**: **Partially correct but critically incomplete.** The spec says this field disables BOTH the backup activation (ER turns on) AND the compressor (HP turns off) simultaneously at the switchover temperature. The ticket omits the second half of the definition. The field is **explicitly a compressor-disabling temperature**, making its use as `hp_lockout_temp_c` spec-correct, not a bug.

**Citation 2**: "OCHRE `vendors/OCHRE/ochre/utils/hpxml.py` — backup switchover maps to backup-activation field only"

- **Source found**: `vendors/OCHRE/ochre/utils/hpxml.py:947-956` (local submodule, read directly)
- **Quoted passage**:
  ```python
  hp_lockout_temp = heat_pump.get(
      "CompressorLockoutTemperature",
      heat_pump.get("BackupHeatingSwitchoverTemperature", 0),
  )
  er_lockout_temp = heat_pump.get(
      "BackupHeatingLockoutTemperature",
      heat_pump.get("BackupHeatingSwitchoverTemperature", 40),
  )
  ```
- **Verdict**: **Incorrect.** OCHRE maps `BackupHeatingSwitchoverTemperature` as a fallback to **both** `hp_lockout_temp` (line 949) **and** `er_lockout_temp` (line 954). HARES replicates this behaviour precisely. The ticket's claim is the opposite of what the OCHRE code does.

**Citation 3**: "ANSI/RESNET/ICC 301-2022 — references HPXML field semantics"

- **Source found**: RESNET publishes the standard at https://www.resnet.us/wp-content/uploads/ANSIRESNETICC301-2022_resnetpblshd.pdf; search confirmed via https://codes.iccsafe.org/content/RESNET3012022P1
- **Quoted passage**: Search and fetch of publicly accessible sections did not find any normative text in 301-2022 that defines or constrains the semantics of `BackupHeatingSwitchoverTemperature` vs `CompressorLockoutTemperature`. RESNET 301 references HPXML as the data format but does not redefine field semantics.
- **Verdict**: **Cannot confirm** — the citation is plausible in that 301-2022 uses HPXML, but no passage in 301-2022 overrides the HPXML spec's definition of this field. The HPXML schema definition (PR #309) is the binding source for field semantics. This citation does not support the ticket's position.

**Citation 4**: "AHRI Standard 210/240-2023 — heat pump capacity ratings extend below switchover; compressor lockout is independent"

- **Source found**: AHRI 210/240 standard summary via https://www.ahrinet.org/search-standards/ahri-210240-2023-2020-performance-rating-unitary-air-conditioning-and-air-source-heat-pump-equipment and search results; full PDF is paywalled (HTTP 403)
- **Quoted passage**: From accessible summaries — AHRI 210/240-2023 defines a **Cold Climate Heat Pump** as one "for which both low-temperature compressor cut-out and cut-in temperatures are specified to be less than 5°F and for which capacity for the H4full test (at 5°F) is certified to be at least 70% of the capacity for the nominal full capacity test conducted at 47°F." Industry practice cites the backup heat lockout as typically set 5°F above the balance point, with dual-fuel switchover typically 35–40°F.
- **Verdict**: **Partially correct but not the claim made.** AHRI 210/240 does define cold-climate HP capacity down to low temperatures. However, the ticket uses this to claim "compressor lockout is independent" of `BackupHeatingSwitchoverTemperature` — this is true for all-electric cold-climate HPs where `CompressorLockoutTemperature` and `BackupHeatingSwitchoverTemperature` are distinct concepts. But the HPXML spec is explicit that `BackupHeatingSwitchoverTemperature` _does_ disable the compressor in dual-fuel systems. The two concepts are not contradictory; they apply to different system types.

### Legitimacy

- **Verdict**: **Not Legitimate**

- **Rationale**: The ticket is built on two factual errors. First, it incorrectly states that OCHRE maps `BackupHeatingSwitchoverTemperature` only to the ER/backup lockout — the OCHRE source at `hpxml.py:947-956` shows it is used as a fallback for **both** `hp_lockout_temp` (compressor lockout) and `er_lockout_temp` (ER lockout), exactly as HARES does. Second, the ticket misreads the HPXML specification: `BackupHeatingSwitchoverTemperature` is formally defined as "Temperature at which the backup heating is activated **and the compressor is disabled**" — meaning its use as `hp_lockout_temp_c` is spec-correct, not a bug. The alleged "thermal capacity gap" does not exist: when `BackupHeatingSwitchoverTemperature` populates both lockouts at the same value (e.g. 4.44°C), OAT below that value locks out the HP (correct) and permits ER (because `er_lockout_temp_c` in HARES means ER is allowed when `OAT < er_lockout_temp_c`). There is no temperature range where neither system is available. Finally, the ticket's section number "§8.4" for the HPXML specification cannot be verified — the HPXML schema does not publish numbered sections in the accessible online documentation; the normative definition is in the XSD annotation, not a numbered prose section.

### Proposed Fix Summary

No production-code fix is warranted. The current implementation at `resolve_hvac.rs:1497-1522` correctly replicates OCHRE's `hpxml.py:947-956` mapping for `BackupHeatingSwitchoverTemperature`. If a future enhancement is desired for all-electric cold-climate heat pumps where compressor and ER operate independently below freezing, that is a distinct feature request requiring new HPXML semantics (e.g., the all-electric `CompressorLockoutTemperature` + `BackupHeatingLockoutTemperature` pair, which HARES already handles as the primary path at lines 1498-1511).

### Test Written

- File: `crates/hares-io/tests/hpxml_parity.rs` (appended after existing `ashp_backup_lockout_temperature_extracted` test)
- What it tests:
  - **Scenario A** (`ticket_095_switchover_only_maps_to_both_lockouts_ochre_parity`): With only `BackupHeatingSwitchoverTemperature` present, both `hp_lockout_temp_c` and `er_lockout_temp_c` are set to the converted value — confirming OCHRE parity
  - **Scenario B** (`ticket_095_separate_compressor_and_backup_lockouts_independent`): `CompressorLockoutTemperature` + `BackupHeatingLockoutTemperature` map independently; `hp_lockout` < `er_lockout` invariant holds
  - **Scenario C** (`ticket_095_no_lockout_fields_emits_no_lockout_params`): When neither field is present, no lockout params are written, so the equipment falls back to its own `DEFAULT_HP_LOCKOUT_TEMP_C` / `DEFAULT_ER_LOCKOUT_TEMP_C` constants
- Note: All three tests pass logically but cannot be run to completion because of pre-existing compile errors (`charge_defect_ratio` field missing from config structs) at lines ~1284-1456 of the test file — errors that are unrelated to this ticket and pre-date this audit.
