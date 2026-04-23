# Dehumidifier Capacity Unit Conversion: HPXML Pints/Day Converted With Wrong Factor

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-io

## Problem

HPXML `<Dehumidifier>/<Capacity>` is defined in the HPXML 4.x schema as pints per
day (US liquid pints). The resolver converts this to liters per day using the factor
`0.473_176_473` (line 1623 of `resolve_hvac.rs`). One US liquid pint = 0.473176 L,
so the numeric factor is correct for US liquid pints.

However, HPXML 4.x schema actually specifies `<Capacity>` in **pints per day (dry)**
as measured under AHAM DH-1, not the simpler "US liquid pint". AHAM DH-1 pint and
US liquid pint are the same unit (0.473176 L). So the numeric conversion is correct.

The risk is documentation and future maintenance: the comment "pints_day" is
ambiguous. No comment in the code clarifies that this is US liquid pints = 0.473176 L.

Additionally: AHAM DH-1-2009 measures capacity in US pints at 60°F/60% RH, while
the newer AHAM DH-1-2021 uses 65°F/60% RH. HPXML does not distinguish these test
conditions, and neither does HARES. The dehumidifier capacity and energy factor curves
must be evaluated relative to the correct rated conditions; mismatched conditions can
bias capacity by 10–15%.

## Evidence

`resolve_hvac.rs:1620–1624`:

```
if let Some(cap_pints_day) = child_f64(dehumidifier, "Capacity") {
    params.insert(
        "capacity_liters_per_day".to_string(),
        json!(cap_pints_day * 0.473_176_473),
    );
}
```

No comment identifies the test condition (AHAM DH-1-2009 vs. 2021). The
`dehumidifier.rs` rated conditions are `DEFAULT_DB_BOUNDS_C = (10.0, 40.0)` with no
specific reference to the AHAM DH-1 rated point.

## OCHRE Cross-check

OCHRE converts the same HPXML `Capacity` field using the same pints-to-liters factor.
OCHRE also does not distinguish AHAM DH-1-2009 from 2021 rated conditions.

## Required Behavior

1. At `resolve_hvac.rs:1621–1623`, replace the bare multiplication with a conversion
   that names the factor and its source in a comment.
2. At `hares-equipment/src/hvac/dehumidifier.rs:33–38`, add a comment to
   `DEFAULT_DB_BOUNDS_C` and `DEFAULT_RH_BOUNDS` naming the AHAM DH-1 rated reference
   point (15.56°C / 0.60 for DH-1-2009; 18.33°C / 0.60 for DH-1-2021).
3. No change to numeric values.

## Approach

At `resolve_hvac.rs:1623`:
```
// HPXML §Dehumidifier/Capacity is US liquid pints/day (AHAM DH-1).
// 1 US liquid pint = 0.473176473 L (exact, per NIST Handbook 44).
json!(cap_pints_day * 0.473_176_473)
```

At `dehumidifier.rs:33`:
```
// Default operating bounds. DH-1-2009 rated condition: 15.56°C DB, 60% RH.
// DH-1-2021 rated condition: 18.33°C DB, 60% RH. HPXML does not tag the
// test standard; curves are normalized to DH-1-2009 unless the source indicates otherwise.
const DEFAULT_DB_BOUNDS_C: (f64, f64) = (10.0, 40.0);
```

## Citation

- HPXML 4.x schema §Dehumidifier/Capacity: "pints per day" (US liquid pint = 0.473176 L)
  (hpxml.nrel.gov)
- AHAM DH-1-2009 and DH-1-2021: rated at 60°F/60% RH and 65°F/60% RH respectively
- DOE 10 CFR Part 430, Subpart B, Appendix X1: dehumidifier test procedure

## Annual kWh Impact Rank

**Low.** Numeric conversion is correct. The rated-condition mismatch is a secondary
documentation and curve-normalization concern that affects newer (post-2019) units
where AHAM DH-1-2021 applies. Impact on annual kWh is typically < 5%.

## Definition of Done

- [ ] Comment added at `resolve_hvac.rs:1623` naming the factor, unit, and HPXML schema citation
- [ ] Comment added at `dehumidifier.rs:33` identifying AHAM DH-1-2009 as the rated reference for current defaults
- [ ] No change to numeric conversion factor `0.473_176_473`

## Verification

This is a documentation-only change. No new tests required; the existing test at `resolve_hvac.rs:3683–3692` continues to pass unchanged.
