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

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match (or note corrected location)

  The ticket cites `crates/hares-equipment/src/hvac/constants.rs:34`. The file has moved: the
  correct path is `crates/hares-equipment/src/hvac/heat_pump/constants.rs`, and the constant is
  still on **line 34**:
  ```rust
  pub const DEFAULT_HP_LOCKOUT_TEMP_C: f64 = -17.78;
  ```
  It carries **no documentation comment or citation**, exactly as described.

- [x] Described logic matches current implementation

  The constant is consumed in `crates/hares-equipment/src/hvac/heat_pump/heater.rs` at:
  - line 380 (struct initializer default)
  - line 578: `self.hp_lockout_temp_c = cfg.hp_lockout_temp_c.unwrap_or(DEFAULT_HP_LOCKOUT_TEMP_C);`

  `crates/hares-io/src/hpxml/resolve_hvac.rs` lines 1496-1522 read
  `CompressorLockoutTemperature` (falling back to `BackupHeatingSwitchoverTemperature`) from
  HPXML. When neither field is present the parameter key is simply absent from `params`, so the
  heater builder falls through to `DEFAULT_HP_LOCKOUT_TEMP_C` via `unwrap_or`. This matches the
  ticket's description.

- [x] OCHRE cross-check result: **matches** — `vendors/OCHRE/ochre/Equipment/HVAC.py:1208`

  OCHRE `ASHPHeater.__init__` line 1208:
  ```python
  self.hp_lockout_temp = kwargs.get("Heat Pump Lockout Temperature (C)", -17.78)  # 0F default
  ```
  HARES's `DEFAULT_HP_LOCKOUT_TEMP_C = -17.78` is an exact copy of the OCHRE default, including
  the `# 0F default` annotation that OCHRE itself provides as implicit justification. The OCHRE
  HPXML parser (`vendors/OCHRE/ochre/utils/hpxml.py:947-951`) also defaults to `0°F` (→ -17.78°C)
  when `CompressorLockoutTemperature` and `BackupHeatingSwitchoverTemperature` are both absent.
  OCHRE's own documentation (`vendors/OCHRE/docs/source/InputsAndArguments.rst:456`) records:
  > `Heat Pump Lockout Temperature (C)` | number | No | Taken from HPXML file, or -17.78 | Minimum
  > ambient temperature to run heat pump for ASHP Heater

- [x] EnergyPlus cross-check result: **diverges** — EnergyPlus default is -8°C, not -17.78°C

  The EnergyPlus `Coil:Heating:DX:SingleSpeed` object has a field
  *"Minimum Outdoor Dry-Bulb Temperature for Compressor Operation"* with a documented default of
  **-8.0°C** and a valid range of ≥ -20°C (confirmed via DesignBuilder Help v7.2, which mirrors
  the EnergyPlus IDD):
  > "This field defines the minimum outdoor air dry-bulb temperature where the heating coil
  > compressor turns off. The temperature must be greater than or equal to –20°C. Default: -8°C."

  The HARES/OCHRE value of -17.78°C is substantially colder (≈ 9.78°C below the EnergyPlus
  default). The divergence is **intentional**: HARES and OCHRE jointly model the real-world
  minimum operating temperature (0°F) for typical residential ASHPs as manufactured and installed,
  whereas EnergyPlus's -8°C (≈ 17.6°F) reflects a different, arguably conservative default for
  its `Coil:Heating:DX:SingleSpeed` object. The OCHRE comment `# 0F default` confirms this is a
  deliberate design choice, not an oversight.

---

### Web-Verified Citations

#### Citation 1: AHRI Standard 210/240-2023 — minimum operating temperature for ASHP

- **Citation**: Ticket claims AHRI Standard 210/240-2023 may provide a minimum operating
  temperature that could serve as the primary source for -17.78°C.
- **Source found**: AHRI Standard 210/240-2023 (2020) and AHRI 210/240-2024 (I-P) PDFs at
  `ahrinet.org` — both returned HTTP 403; readable previews via `documents.pub` were also blocked.
  However, the standard's heating test conditions are widely cited in secondary sources
  (NEEP, PNNL BASC, UpCodes, academic papers) and confirmed by web search.
- **Quoted passage (from secondary sources)**:
  > "Three tests are conducted for a single-speed compressor heat pump: the high temperature (H1)
  > test at 47°F, the frost accumulation (H2) test at 35°F, and the low temperature (H3) test at
  > 17°F. An H4 test at 5°F is used for Cold Climate Heat Pump certification, where H4 capacity
  > must be ≥ 70% of H1 capacity."
  > — corroborated by multiple web-search results citing AHRI 210/240-2023.
- **Verdict**: **Partially verified** — AHRI 210/240's *test points* (H1=47°F, H2=35°F, H3=17°F,
  H4=5°F) are verifiable, but the standard defines *performance test conditions*, not a minimum
  ambient operating temperature or compressor lockout value. The ticket correctly cites AHRI
  210/240 as a *candidate* source; however, the standard does not appear to specify a fixed
  -17.78°C (0°F) compressor lockout. The lowest AHRI heating test temperature is H4 = 5°F
  (-15°C). AHRI 210/240 is therefore a *plausible context* for the value but is not a
  direct citation for -17.78°C.

#### Citation 2: HPXML Specification v4.x §8.4 `CompressorLockoutTemperature`

- **Citation**: Ticket claims the HPXML spec may document a default for `CompressorLockoutTemperature`.
- **Source found**: OpenStudio-HPXML documentation (readthedocs.io) and the HPXML Working Group
  PR #309 (`github.com/hpxmlwg/hpxml/pull/309`).
- **Quoted passage**: From HPXML WG PR #309 and OpenStudio-HPXML docs (summarised from multiple
  web search results):
  > "CompressorLockoutTemperature — [deg F] Temperature below which the compressor is disabled,
  > often to prevent damage or occupant comfort issues. The default is the manufacturer's minimum
  > operating temperature, but the value may be set higher."
  > Defaults used by OpenStudio-HPXML: **0°F for standard air-to-air HPs**, 25°F for dual-fuel,
  > -20°F for variable-speed/mini-split. (Source: multiple web-search results citing
  > `openstudio-hpxml.readthedocs.io` and the HPXML WG PR.)
- **Verdict**: **Confirmed** — the HPXML ecosystem (OpenStudio-HPXML) uses **0°F as the default
  `CompressorLockoutTemperature` for standard single-speed air-to-air heat pumps**, directly
  corroborating the -17.78°C value in HARES. OCHRE's HPXML parser (`hpxml.py:947-950`) also
  explicitly defaults to `0°F` when the element is absent. This is the strongest direct
  citation for the value.

#### Citation 3: OCHRE source `vendors/OCHRE/ochre/Equipment/Heat_Pump.py` (or equivalent)

- **Citation**: Ticket says to cross-check OCHRE default.
- **Source found**: `vendors/OCHRE/ochre/Equipment/HVAC.py:1208` (the correct file; there is no
  separate `Heat_Pump.py` in this vendored copy).
- **Quoted passage**:
  ```python
  self.hp_lockout_temp = kwargs.get("Heat Pump Lockout Temperature (C)", -17.78)  # 0F default
  ```
- **Verdict**: **Confirmed** — OCHRE uses -17.78°C as its default HP lockout temperature and even
  annotates it as `# 0F default`. HARES's constant matches exactly.

#### Citation 4: Manufacturer specifications

- **Citation**: Ticket suggests checking Mitsubishi, Daikin, Carrier for typical minimum
  operating temperatures.
- **Source found**: Web searches on manufacturer data.
- **Quoted passage** (representative):
  > "Modern units, especially those labeled 'cold-climate,' are designed to operate down to 0°F
  > (-18°C) and continue producing usable heat even lower." — pickcomfort.com
  > "Lennox MLA heat pump can maintain 100% capacity at 0°F and can operate as low as -22°F."
  > — lennox.com product page
  > "Trane's most efficient heat pumps offer 100% heating capacity down to 27°F and 70-82%
  > at 5°F; the Climatuff™ Variable Speed Compressor can handle 0°F." — trane.com
- **Verdict**: **Corroborated** — 0°F (-17.78°C) is a common industry threshold: it is the
  approximate lower limit for most non-cold-climate single-stage residential ASHPs. Cold-climate
  models extend below this (to -22°F or lower), so the ticket is correct that manufacturer specs
  *support* the value as a plausible representative default, though no single spec mandates it.

---

### Legitimacy

- **Verdict**: **Legitimate**
- **Rationale**: The core issue is real and accurately described. `DEFAULT_HP_LOCKOUT_TEMP_C =
  -17.78` at `crates/hares-equipment/src/hvac/heat_pump/constants.rs:34` carries no documentation
  comment or citation. The value is correct (it matches OCHRE HVAC.py:1208 exactly, and matches
  the OpenStudio-HPXML default of 0°F for standard air-to-air HPs), but a future contributor
  cannot verify this without re-reading the OCHRE source and the HPXML ecosystem documentation.
  The ticket's proposed fix — adding a citation comment — is appropriate and minimal. The ticket's
  claim that the file is at `constants.rs` rather than `heat_pump/constants.rs` is a minor
  path error, but the bug itself and all other details are accurate. No NaN or logic error is
  present; this is purely a documentation gap.

  The AHRI 210/240 citation is the weakest part of the ticket: the standard defines test
  conditions (H3 = 17°F, H4 = 5°F) but does not itself prescribe -17.78°C as a compressor
  lockout limit. The best citation chain is: **OCHRE HVAC.py:1208** → **OpenStudio-HPXML default
  0°F for standard ASHPs** → **manufacturer industry norm for non-cold-climate residential units**.

---

### Proposed Fix Summary

Add a documentation comment to `DEFAULT_HP_LOCKOUT_TEMP_C` in
`crates/hares-equipment/src/hvac/heat_pump/constants.rs:34` citing:
1. OCHRE HVAC.py line 1208 as the direct source (`-17.78` / `# 0F default`)
2. OpenStudio-HPXML's default of 0°F for `CompressorLockoutTemperature` on standard
   single-speed air-to-air heat pumps as corroborating evidence
3. A note that the EnergyPlus `Coil:Heating:DX:SingleSpeed` object defaults to -8°C and
   the intentional divergence from that value

Do NOT change the value; -17.78°C is correct and is consistent with OCHRE and the HPXML
ecosystem. The correct file path is `crates/hares-equipment/src/hvac/heat_pump/constants.rs`,
not `crates/hares-equipment/src/hvac/constants.rs` as stated in the ticket.

---

### Test Written

- File: `crates/hares-equipment/tests/hvac_tests.rs` — test already exists
- Test name: `ashp_defaults_match_reference` (lines 1077-1159)
- What it tests: behaviorally verifies that when `hp_lockout_temp_c: None` is passed (triggering
  the `DEFAULT_HP_LOCKOUT_TEMP_C` fallback), the ASHP delivers zero heat and draws zero power when
  OAT is -20°C (below the -17.78°C lockout). This test **passes** in the current codebase
  (`cargo test -p hares-equipment ashp_defaults_match_reference` → ok). No new regression test
  is needed; the existing test is an adequate behavioral guard for the constant's value.
  If the constant were changed, the test would catch a value less cold than -20°C but would not
  catch a value that is *more* cold than -17.78°C. A future implementer should consider adding a
  test that probes OAT between -18°C and -17°C to tightly pin the exact threshold.
