# BackupAnnualHeatingEfficiency Units Element Ignored — EIR Computed From Raw Value

**Severity**: High
**Priority**: P1
**Status**: Open
**Areas**: hares-io

## Problem

The HPXML `<BackupAnnualHeatingEfficiency>` element contains both `<Units>` and
`<Value>` child elements. The Units element specifies whether the value is expressed
as "Percent" (fraction 0–1), "AFUE" (fraction 0–1), or other forms. HARES reads
only `<Value>` and unconditionally inverts it as `EIR = 1 / val`. This is correct
for AFUE expressed as a fraction (e.g., 0.95 → EIR ≈ 1.053), but wrong for
"Percent" when the value is expressed as percent-out-of-100 (e.g., 95 →
EIR = 1/95 = 0.0105 ↔ COP = 95 — physically impossible).

HPXML 4.x schema allows "Percent" with values like 1.0 (meaning 100%) or 95.0
(meaning 95%). If any real file uses "Percent" with a value like 80.0, HARES
would produce an EIR of 0.0125 (COP = 80), making backup heat appear essentially
free and eliminating the thermostat's incentive to limit ER use.

## Evidence

`resolve_hvac.rs:1486–1490`:

```
if let Some(eff_node) = heat_pump.child("BackupAnnualHeatingEfficiency") {
    if let Some(val) = child_f64(eff_node, "Value") {
        // EIR = 1/efficiency for resistance backup
        params.insert("backup_eir".to_string(), json!(1.0 / val.max(0.01)));
    }
}
```

No check of `child_text(eff_node, "Units")` is performed.

Sample file confirms "Percent" units exist in practice
(`base-hvac-air-to-air-heat-pump-1-speed-heating-capacity-17f.xml:336–339`):

```xml
<BackupAnnualHeatingEfficiency>
  <Units>Percent</Units>
  <Value>1.0</Value>
</BackupAnnualHeatingEfficiency>
```

This particular case is accidentally correct (1/1.0 = 1.0). A file with
`<Value>100.0</Value>` under `<Units>Percent</Units>` would yield EIR = 0.01.

The same OCHRE helper (`hpxml.py:946`) also does not check units — however, OCHRE
always receives pre-normalized data from ResStock where backup efficiency is always
a fraction. HARES must be robust to all valid HPXML.

## OCHRE Cross-check

OCHRE `hpxml.py:946–963`: `backup_cop = heat_pump.get("BackupAnnualHeatingEfficiency", {}).get("Value")`
and `"Backup EIR (-)": 1 / backup_cop`. OCHRE also ignores Units but accepts that
ResStock always provides fraction-form values. HARES must handle the full HPXML
value space.

## Required Behavior

Before inverting, check `<Units>`:
- "Percent" with value > 1.0 → normalize by dividing by 100 before inverting
- "AFUE" → value is already a fraction; invert directly
- "COP" → value IS already COP; EIR = 1/value (already correct)
- Unknown units → error loudly rather than silently using raw value

```
let units = child_text(eff_node, "Units").unwrap_or_default().to_ascii_uppercase();
let eir = match units.as_str() {
    "PERCENT" if val > 1.0 => 100.0 / val.max(0.01),
    "PERCENT" => 1.0 / val.max(0.01),
    "AFUE" | "" => 1.0 / val.max(0.01),
    "COP" => 1.0 / val.max(0.01),  // val is COP; same formula
    other => return Err(HpxmlError::Parse(format!(
        "BackupAnnualHeatingEfficiency: unrecognized units '{other}'")))
};
```

## Citation

- HPXML 4.x schema §HeatPump/BackupAnnualHeatingEfficiency (hpxml.nrel.gov): `Units`
  is required alongside `Value`; valid units include "Percent", "AFUE", "COP"
- EnergyPlus Engineering Reference §16.3: backup strip EIR = 1.0 (resistance, COP = 1)
  or 1/AFUE (gas backup)

## Annual kWh Impact Rank

**High.** Wrong EIR on the backup strip makes resistance heating appear nearly free
(COP >> 1), so the heat pump heater never calls backup — or calls it for far longer
than physics allows. Annual ER consumption error can be 100%+ for cold climates
where the backup runs frequently.

## Cross-Cluster Note

This defect is structurally identical to ticket 052 (infiltration ACH50 parsed
without reading the adjacent unit element). Both are HPXML numeric fields where
the resolver reads `<Value>` without checking the sibling `<Units>` or
`<UnitofMeasure>` element. A meta-ticket auditing all such unit-blind reads in
the HPXML parser cluster is recommended (see report).

## Approach

Change location: `resolve_hvac.rs:1486–1490`. Replace the current `if let Some(val)` block with:

```
let units = child_text(eff_node, "Units").unwrap_or_default().to_ascii_uppercase();
let eir = match units.as_str() {
    "PERCENT" if val > 1.0 => 100.0 / val.max(0.01),
    "PERCENT" => 1.0 / val.max(0.01),
    "AFUE" | "" => 1.0 / val.max(0.01),
    "COP" => 1.0 / val.max(0.01),
    other => return Err(HpxmlError::Parse(format!(
        "BackupAnnualHeatingEfficiency: unrecognized units '{other}'")))
};
params.insert("backup_eir".to_string(), json!(eir));
```

## Definition of Done

- [ ] `<Units>` element is read at `resolve_hvac.rs:1487` before inverting `<Value>`
- [ ] "Percent" with value > 1.0 is divided by 100 before EIR computation
- [ ] Unknown units produce a `HpxmlError::Parse` (loud error, no silent default)
- [ ] Test: `<Units>Percent</Units><Value>100.0</Value>` → EIR = 1.0 (COP = 1.0)
- [ ] Test: `<Units>AFUE</Units><Value>0.95</Value>` → EIR ≈ 1.053
- [ ] Test: `<Units>COP</Units><Value>3.5</Value>` → EIR ≈ 0.286
- [ ] Test: `<Units>Joules</Units><Value>1.0</Value>` → parse error

## Verification

```bash
cargo test -p hares-io -- resolve_hvac::tests
```

Test fixture: add HPXML fragments with each units variant (`Percent`/`AFUE`/`COP`/unknown) in `crates/hares-io/tests/fixtures/`. Verify EIR field in the resulting params map.

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation
- [x] Referenced line numbers still match — `resolve_hvac.rs:1486–1490` matches exactly; `eff_node.child("BackupAnnualHeatingEfficiency")` → `child_f64(eff_node, "Value")` → `1.0 / val.max(0.01)` with no `child_text(eff_node, "Units")` call present.
- [x] Described logic matches current implementation — confirmed: HARES reads only `<Value>` and inverts it unconditionally regardless of `<Units>`.
- [x] OCHRE cross-check result: **matches** — `vendors/OCHRE/ochre/utils/hpxml.py` line ~946: `backup_cop = heat_pump.get("BackupAnnualHeatingEfficiency", {}).get("Value")` and line ~963: `"Backup EIR (-)": 1 / backup_cop`. OCHRE likewise ignores `<Units>`, consistent with the ticket's claim. OCHRE's own inline comment reads: `# assumes efficiency units are in Percent or AFUE (0-1)`, confirming the assumption that values arrive as fractions. HARES diverges from OCHRE only by needing to handle the full HPXML value space without that assumption.
- [x] EnergyPlus cross-check result: **matches (partially)** — The EnergyPlus `Coil:Heating:Electric` `Efficiency` field (I/O Reference, multiple versions including 8.2 and 8.8) is documented as a **fraction (0–1)**, where 1.0 means 100% efficient. Source: raw LaTeX from `github.com/NatLabRockies/EnergyPlus/develop` I/O Reference source — *"This is user-inputted efficiency (decimal units, not percent) and can account for any loss. In most cases for the electric coil, this will be 100%."* The EnergyPlus §16.3 citation in the ticket (claiming "backup strip EIR = 1.0 or 1/AFUE") is not precisely locatable by section number in available EnergyPlus 9.x documentation — the supplemental heater concept is real and correct (electric resistance has efficiency=1.0 → EIR=1.0; gas backup uses AFUE as a fraction), but the precise §16.3 numbering could not be verified from web-accessible sources.

### Web-Verified Citations

**Citation 1**: HPXML 4.x schema `§HeatPump/BackupAnnualHeatingEfficiency` — Units is required alongside Value; valid units include "Percent", "AFUE", "COP"

- **Source found**: `raw.githubusercontent.com/hpxmlwg/hpxml/master/schemas/HPXMLDataTypes.xsd`
- **Quoted passage**: The `HeatingEfficiencyUnits_simple` type in `HPXMLDataTypes.xsd`:
  ```xml
  <xs:simpleType name="HeatingEfficiencyUnits_simple">
    <xs:restriction base="xs:string">
      <xs:enumeration value="HSPF"/>
      <xs:enumeration value="HSPF2"/>
      <xs:enumeration value="COP"/>
      <xs:enumeration value="AFUE"/>
      <xs:enumeration value="Percent"/>
    </xs:restriction>
  </xs:simpleType>
  ```
- **Verdict**: **Confirmed** — valid units are HSPF, HSPF2, COP, AFUE, and Percent. The ticket omits HSPF and HSPF2 from the error-handling list, which means the proposed `match` arm `other => return Err(...)` would erroneously reject valid HSPF/HSPF2 units. This is a gap in the proposed fix.

**Citation 2**: OpenStudio-HPXML treatment of `Percent` units as a fraction (0–1)

- **Source found**: `github.com/NREL/OpenStudio-HPXML` — `HPXMLtoOpenStudio/resources/hpxml.rb` (blob `a4cbd31b...`)
- **Quoted passage** (lines 6992–6993 of `hpxml.rb`):
  ```ruby
  :backup_heating_efficiency_percent,    # [Double] BackupAnnualHeatingEfficiency[Units="Percent"]/Value (frac)
  :backup_heating_efficiency_afue,       # [Double] BackupAnnualHeatingEfficiency[Units="AFUE"]/Value (frac)
  ```
  And line 7328: `@backup_heating_efficiency_percent = XMLHelper.get_value(heat_pump, "BackupAnnualHeatingEfficiency[Units='Percent']/Value", :float)`
  And `hvac.rb` line 1096–1100: `heating_capacity / heating_efficiency_percent` — the fraction is used directly (not divided by 100).
- **Verdict**: **Confirmed** — OpenStudio-HPXML (NREL's reference implementation) treats `Percent` values as fractions (0–1), not percent-out-of-100. The `(frac)` annotation in the source comment is definitive.

**Citation 3**: HPXML sample file `base-hvac-air-to-air-heat-pump-1-speed-heating-capacity-17f.xml:336–339` — `<Units>Percent</Units><Value>1.0</Value>`

- **Source found**: `vendors/OCHRE/test/OS-HPXML Sample Files/base-hvac-air-to-air-heat-pump-1-speed-heating-capacity-17f.xml` (local copy)
- **Quoted passage** (lines 336–340):
  ```xml
  <BackupAnnualHeatingEfficiency>
    <Units>Percent</Units>
    <Value>1.0</Value>
  </BackupAnnualHeatingEfficiency>
  ```
- **Verdict**: **Confirmed** — file exists at the cited location. Line numbers are 336–340 (off by one vs. the ticket's 336–339 citation, but the content is accurate).

**Citation 4**: EnergyPlus Engineering Reference §16.3 — backup strip EIR = 1.0 (resistance, COP = 1) or 1/AFUE (gas backup)

- **Source found**: Multiple searches of `bigladdersoftware.com` EnergyPlus Engineering Reference (versions 8.0–24.x) and GitHub LaTeX source
- **Quoted passage**: EnergyPlus I/O Reference LaTeX source (via `github.com/NatLabRockies/EnergyPlus/develop`): *"This is user-inputted efficiency (decimal units, not percent) and can account for any loss. In most cases for the electric coil, this will be 100%."* — confirming electric resistance efficiency = 1.0 fraction, EIR = 1.0.
- **Verdict**: **Partially correct** — the underlying physics claim (EIR = 1.0 for electric resistance, EIR = 1/AFUE for gas backup with AFUE as a fraction) is correct and consistent with EnergyPlus `Coil:Heating:Electric` documentation. However, the specific §16.3 numbering was not verifiable from web-accessible sources; the Engineering Reference is not consistently chapter-numbered across versions. The claim is correct in substance but the section number cannot be confirmed.

### Legitimacy
- **Verdict**: **Partially Legitimate**
- **Rationale**: The core bug is real and actively reproducible. The HARES code at `resolve_hvac.rs:1486–1490` reads `<Value>` without reading `<Units>`, confirmed by direct code inspection. The regression test `backup_eir_percent_out_of_100_must_normalize_to_eir_one` demonstrates the bug precisely: `<Units>Percent</Units><Value>100.0</Value>` produces EIR = 0.01 (COP = 100) instead of EIR = 1.0. All three citations about HPXML schema, OCHRE behavior, and sample file content are confirmed. Two issues lower the verdict from "Legitimate" to "Partially Legitimate": (1) The proposed `match` arm only handles "PERCENT", "AFUE", and "COP" — it would `return Err(...)` for "HSPF" and "HSPF2", which are also valid `HeatingEfficiencyUnits` per the HPXML XSD (though HSPF/HSPF2 are seasonal metrics that are unusual for a backup strip). The fix should either include HSPF/HSPF2 arms or treat them as an error with a specific diagnostic. (2) The EnergyPlus §16.3 citation cannot be verified by section number, though the underlying claim is correct.

### Proposed Fix Summary
At `resolve_hvac.rs:1486–1490`, after obtaining `val` from `child_f64(eff_node, "Value")`, read `child_text(eff_node, "Units")` and apply a match: "PERCENT" with val > 1.0 → EIR = 100.0/val; "PERCENT" with val ≤ 1.0 → EIR = 1.0/val; "AFUE" or "" → EIR = 1.0/val; "COP" → EIR = 1.0/val; "HSPF" | "HSPF2" → return Err (seasonal metric, not valid for backup strip); unknown → return Err. This is the minimal fix. The proposed fix in the ticket is structurally correct but omits HSPF/HSPF2 handling.

### Test Written
- **File**: `crates/hares-io/src/hpxml/resolve_hvac.rs` (inline `#[cfg(test)]` module, appended after ticket-076 tests)
- **Tests added**:
  1. `backup_eir_percent_fraction_form_yields_eir_one` — `Percent`/`1.0` → EIR = 1.0 (passes today, guards regression)
  2. `backup_eir_percent_out_of_100_must_normalize_to_eir_one` — `Percent`/`100.0` → **FAILS** today (EIR = 0.01), demonstrates the bug; will pass after fix
  3. `backup_eir_afue_fraction_yields_correct_eir` — `AFUE`/`0.95` → EIR ≈ 1.0526 (passes today)
  4. `backup_eir_cop_yields_correct_eir` — `COP`/`3.5` → EIR ≈ 0.2857 (passes today)
- **Run**: `cargo test -p hares-io -- backup_eir`
- **Observed output**: 3 passed, 1 failed (`backup_eir_percent_out_of_100_must_normalize_to_eir_one` — `currently 0.01 — bug 077`)
