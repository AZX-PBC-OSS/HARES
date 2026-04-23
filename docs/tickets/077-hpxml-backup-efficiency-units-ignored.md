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
